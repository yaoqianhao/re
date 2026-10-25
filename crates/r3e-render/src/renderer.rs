//! wgpu 渲染器：单一前向管线 + 实例化绘制。
//!
//! 场景（`r3e_core::Scene`）是唯一真源，渲染器每帧从场景重建实例数据，
//! 因此脚本或 JSON 对物件的任何修改都会在下一帧直接反映到画面上。

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use r3e_core::mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage, Vertex};
use r3e_core::{Aabb, AssetId, Frustum, Light, MeshRef, ProceduralProvider, Scene};
use winit::window::Window;

use crate::mesh::{vertex_layout, InstanceRaw};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const INITIAL_INSTANCE_CAPACITY: u64 = 512;
/// 蒙皮网格做视锥剔除时的包围盒外扩比例：动画形变会超出静止姿态的 AABB。
const SKIN_CULL_MARGIN: f32 = 0.25;

/// 材质 uniform：base color + PBR 标量。32 字节，16 字节对齐。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct MaterialUniform {
    base_color: [f32; 4],
    /// x = metallic，y = roughness，z = 是否有贴图（0/1），w 保留。
    params: [f32; 4],
}

impl Default for MaterialUniform {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            params: [0.0, 0.6, 0.0, 0.0],
        }
    }
}

/// 材质键：默认材质，或「某资产内第 N 个材质」。
///
/// 渲染层只认这个键，具体材质参数通过 `MeshProvider::material` 取，
/// 因此这里依旧不依赖任何模型文件格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MaterialKey {
    Default,
    Asset { asset: AssetId, index: u32 },
}

/// 全局相机 / 光照 uniform。128 字节，16 字节对齐。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    /// rgb = 光色，a = 强度
    light_color: [f32; 4],
    ambient: [f32; 4],
}

impl Default for CameraUniform {
    fn default() -> Self {
        Self {
            view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos: [0.0, 0.0, 0.0, 1.0],
            light_dir: [0.0, -1.0, 0.0, 0.0],
            light_color: [1.0, 1.0, 1.0, 1.0],
            ambient: [0.1, 0.1, 0.1, 1.0],
        }
    }
}

/// 阶段 D：阴影通道的 uniform —— 光源正交矩阵 + 采样参数。
///
/// `params`：x = 1/size（纹素尺寸），y = 深度比较偏移，z = 是否启用（0/1），
/// w = 法线方向偏移（世界单位，已按纹素与世界尺寸换算）。
/// `params2`：x = 阴影强度。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShadowUniform {
    light_view_proj: [[f32; 4]; 4],
    params: [f32; 4],
    params2: [f32; 4],
}

/// 阴影通道（深度 pass）专用的最小 uniform：只含光源正交矩阵。
///
/// 为什么不复用 `ShadowUniform`：深度 pass 正在往阴影贴图里写像素，
/// 若它的顶点着色器还绑定同一组的「阴影贴图 + 比较采样器」，wgpu 会判定
/// 同一张纹理既被采样又被写入而拒绝（逻辑上也是循环依赖）。因此拆成
/// 独立 bind group：group(3) binding(3) 只放矩阵，主通道用 binding(0..=2)。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShadowPassUniform {
    light_view_proj: [[f32; 4]; 4],
    /// 凑满 16 字节对齐（mat4 已是 64B，这里只是显式占位，便于将来扩展）。
    pad: [f32; 4],
}

impl Default for ShadowPassUniform {
    fn default() -> Self {
        Self {
            light_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            pad: [0.0; 4],
        }
    }
}

impl Default for ShadowUniform {
    fn default() -> Self {
        Self {
            light_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            params: [0.0, 0.0, 0.0, 0.0],
            params2: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

pub struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    /// 材质分区（P2 起）。至少一段；无材质信息时是整网格一段的 `Default`。
    /// 各段 `index_count` 之和即本网格的三角形索引总数。
    parts: Vec<GpuPart>,
}

/// GPU 侧的一个材质分区。
#[derive(Debug, Clone, Copy)]
struct GpuPart {
    first_index: u32,
    index_count: u32,
    material: MaterialKey,
}

/// 分层提供者：**资产优先、程序化兜底**。
///
/// 这是「分级分层」在输出侧的唯一实现点——不是两条并行取数路径，而是同一接口上的
/// 有序回退：上层（导入资产）命中就用，未命中落到下层（内置图元）。
pub struct LayeredProvider {
    primary: Arc<dyn MeshProvider>,
    fallback: Arc<dyn MeshProvider>,
}

impl LayeredProvider {
    pub fn new(primary: Arc<dyn MeshProvider>, fallback: Arc<dyn MeshProvider>) -> Self {
        Self { primary, fallback }
    }
}

impl MeshProvider for LayeredProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData> {
        self.primary
            .mesh_data(key)
            .or_else(|| self.fallback.mesh_data(key))
    }

    fn mesh_aabb(&self, key: MeshRef) -> Option<Aabb> {
        self.primary
            .mesh_aabb(key)
            .or_else(|| self.fallback.mesh_aabb(key))
    }

    fn mesh_parts(&self, key: MeshRef) -> Vec<MeshPartData> {
        let parts = self.primary.mesh_parts(key);
        if parts.is_empty() {
            self.fallback.mesh_parts(key)
        } else {
            parts
        }
    }

    fn material(&self, asset: AssetId, index: u32) -> Option<MaterialData> {
        self.primary
            .material(asset, index)
            .or_else(|| self.fallback.material(asset, index))
    }

    fn texture(&self, asset: AssetId, index: u32) -> Option<TextureImage> {
        self.primary
            .texture(asset, index)
            .or_else(|| self.fallback.texture(asset, index))
    }
}

/// 网格缓存：按 `MeshRef` 惰性上传 GPU 网格。
///
/// **单一取数通道**：内置图元与导入资产都走 `provider.mesh_data(key)`，渲染层不再按
/// `MeshRef` 的来源分支。分层只体现在 [`LayeredProvider`] 的有序回退上：
///
/// * 应用层未注入资产库 —— 回退层只有 `ProceduralProvider`（内置图元）；
/// * 注入资产库后 —— 资产优先，未命中再落到内置图元。
///
/// 渲染层不认识文件 IO；`dyn MeshProvider` 定义在 `r3e-core`，因此本 crate 的依赖图里
/// 不含 `ufbx` / `gltf`。
pub struct MeshCache {
    meshes: HashMap<MeshRef, GpuMesh>,
    /// 网格局部包围盒缓存（视锥剔除与上传都要用）。`None` = 该网格确实取不到包围盒。
    ///
    /// 缓存的意义：包围盒是剔除热路径，只做一次哈希查找，避免每帧走 `dyn` 调用。
    bounds: HashMap<MeshRef, Option<Aabb>>,
    /// 唯一的数据来源；永不缺席（最底层是内置图元）。
    provider: Arc<dyn MeshProvider>,
    /// 已注入的资产层（`None` 表示尚未加载任何外部资产）。
    assets: Option<Arc<dyn MeshProvider>>,
}

impl Default for MeshCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshCache {
    /// 未注入资产库时的缓存：只有内置图元层。
    pub fn new() -> Self {
        Self {
            meshes: HashMap::new(),
            bounds: HashMap::new(),
            provider: Arc::new(ProceduralProvider::new()),
            assets: None,
        }
    }

    /// 以给定资产库构造（`LayeredProvider`：资产优先、内置兜底）。
    pub fn with_library(library: Arc<dyn MeshProvider>) -> Self {
        let mut cache = Self::new();
        cache.set_library(library);
        cache
    }

    /// 注入（或替换）资产库；替换时丢弃此前上传的**资产**网格与包围盒，避免引用失效。
    ///
    /// 内置图元的数据与资产库无关，可以保留缓存（避免重复上传）。
    // 渲染器整体在主线程上跑，`MeshProvider` 刻意不加 `Send + Sync`：
    // 内部持有 `Rc<RefCell<..>>` 共享场景/资源，跨线程共享本就不成立。
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn set_library(&mut self, library: Arc<dyn MeshProvider>) {
        self.assets = Some(library.clone());
        self.provider = Arc::new(LayeredProvider::new(
            library,
            Arc::new(ProceduralProvider::new()),
        ));
        // 单一 upload 路径下不再按来源判断能否复用：整表清空，由 `ensure` 重新上传。
        // 代价只是几个内置图元的顶点数据重传（微秒级），换来零来源分支。
        self.meshes.clear();
        self.bounds.clear();
    }

    /// 当前的数据来源（分层后的组合提供者）。材质 / 贴图也经它取。
    pub fn provider(&self) -> &Arc<dyn MeshProvider> {
        &self.provider
    }

    /// 已注入的资产层。
    pub fn assets(&self) -> Option<&Arc<dyn MeshProvider>> {
        self.assets.as_ref()
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    pub fn get(&self, key: MeshRef) -> Option<&GpuMesh> {
        self.meshes.get(&key)
    }

    /// 网格局部包围盒（几何空间）。取不到时返回 `None`，调用方必须按「不剔除」处理。
    ///
    /// 顺序：缓存 → `mesh_aabb`（零拷贝）→ 兜底 `mesh_data` 现算（仅当提供者没有实现
    /// `mesh_aabb`，且只会发生一次）。
    pub fn bounds(&mut self, key: MeshRef) -> Option<Aabb> {
        if let Some(cached) = self.bounds.get(&key) {
            return *cached;
        }
        let provider = self.provider.clone();
        let computed = provider
            .mesh_aabb(key)
            .or_else(|| provider.mesh_data(key).map(|data| data.aabb_box()))
            .filter(|aabb| !aabb.is_empty());
        self.bounds.insert(key, computed);
        computed
    }

    /// 确保某个网格已在 GPU 上；返回是否可用。
    pub fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: MeshRef) -> bool {
        if self.meshes.contains_key(&key) {
            return true;
        }
        let Some(data) = self.provider.mesh_data(key) else {
            log::warn!("网格 {key:?} 数据不可用，跳过绘制");
            return false;
        };

        // 材质分区：由提供者给出三角形区间；空表示整网格一段默认材质。
        let parts: Vec<GpuPart> = self
            .provider
            .mesh_parts(key)
            .into_iter()
            .map(|part| GpuPart {
                first_index: part.first_index,
                index_count: part.index_count,
                material: match part.material {
                    Some((asset, index)) => MaterialKey::Asset { asset, index },
                    None => MaterialKey::Default,
                },
            })
            .collect();

        let gpu = upload_mesh(device, queue, &data, parts);
        log::debug!(
            "网格已上传 GPU: {key:?} — {} 顶点 / {} 三角",
            data.vertices.len(),
            data.indices.len() / 3
        );
        self.meshes.insert(key, gpu);
        true
    }
}

/// 一帧的绘制统计，便于日志 / HUD 观测。
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub instances: u32,
    /// 本帧被视锥剔除的实例数（阶段三 A）。
    pub culled: u32,
    /// 想剔除但拿不到包围盒、只能照画的实例数。持续非零说明资产层没喂 AABB。
    pub cull_unknown: u32,
    /// 本帧阴影通道提交的 draw call 数（阶段 D）。0 表示阴影关闭或贴图未就绪。
    pub shadow_draws: u32,
    pub frame_index: u64,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    mesh_cache: MeshCache,
    material_cache: MaterialCache,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    /// P4：全体蒙皮实例共用的关节矩阵 storage buffer（容量固定为
    /// `MAX_JOINT_MATRICES` 个 mat4，约 256 KiB）。
    joint_buffer: wgpu::Buffer,
    joint_bind_group: wgpu::BindGroup,
    /// 每帧收集的关节矩阵，扁平存 16 个 f32 / 矩阵，直接 memcpy 上传。
    joint_scratch: Vec<[f32; 16]>,
    /// 阶段 D：方向光阴影贴图（深度图 + 比较采样器 + 光源正交矩阵 uniform）。
    shadow_size: u32,
    _shadow_texture: wgpu::Texture,
    shadow_view: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_bind_group: wgpu::BindGroup,
    shadow_uniform: wgpu::Buffer,
    /// 阴影通道专用：只含光源正交矩阵的 uniform 与绑定组（见 `ShadowPassUniform`）。
    shadow_pass_bind_group: wgpu::BindGroup,
    shadow_pass_uniform: wgpu::Buffer,
    shadow_pipeline: wgpu::RenderPipeline,
    stats: RenderStats,
}

/// `prepare_instances` 的返回值：绘制分组 `(网格, 实例区间)` + 本帧剔除统计。
type InstancePlan = (Vec<(MeshRef, std::ops::Range<u32>)>, u32, u32, Option<Aabb>);

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|e| format!("创建 surface 失败: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("找不到可用的图形适配器: {e}"))?;

        let info = adapter.get_info();
        log::info!(
            "GPU: {} ({:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("r3e-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| format!("请求设备失败: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let mut config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| "当前适配器不支持该窗口 surface".to_string())?;
        // 优先使用 sRGB 目标，避免颜色发灰。
        if let Some(fmt) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = fmt;
        }
        surface.configure(&device, &config);

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-camera-ubo"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-camera-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-camera-bg"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let material_cache = MaterialCache::new(&device);

        // P4：group(2) —— 关节矩阵 storage buffer，所有蒙皮实例共用一段连续槽位
        let joint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-joint-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let joint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-joint-buffer"),
            size: (r3e_core::MAX_JOINT_MATRICES * 16 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let joint_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-joint-bg"),
            layout: &joint_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: joint_buffer.as_entire_binding(),
            }],
        });

        // 阶段 D：group(3) —— 方向光阴影贴图。布局与纹理在管线之前建好，
        // 尺寸按场景配置在 render() 里按需重建（改 `shadow.size` 不必重建整机）。
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-shadow-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // 深度 pass 的独立 group(3)：只有 binding(3) 一个矩阵 uniform，
        // 与主通道采样的 binding(0..=2) 完全分开，避免「写的同时采样」。
        let shadow_pass_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("r3e-shadow-pass-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let shadow_pass_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-shadow-pass-ubo"),
            size: std::mem::size_of::<ShadowPassUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-shadow-pass-bg"),
            layout: &shadow_pass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 3,
                resource: shadow_pass_uniform.as_entire_binding(),
            }],
        });

        let shadow_size = r3e_core::ShadowSettings::default().size;
        let (shadow_texture, shadow_view) = create_shadow_target(&device, shadow_size);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-shadow-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // 比较采样：一次采样即完成硬件 PCF 的「小于等于」深度测试
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-shadow-ubo"),
            size: std::mem::size_of::<ShadowUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-shadow-bg"),
            layout: &shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadow_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-pipeline-layout"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&material_cache.layout),
                Some(&joint_layout),
                Some(&shadow_layout),
            ],
            immediate_size: 0,
        });

        // 阴影通道专用布局：group(3) 换成 `shadow_pass_layout`（只有矩阵 uniform），
        // 不含「阴影贴图 + 比较采样器」——深度 pass 不能一边写一边采样同一张图。
        // 注意 wgpu 要求布局里每个组都被 set，所以 0/1/2 仍要在 pass 里逐个绑定。
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("r3e-shadow-pipeline-layout"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&material_cache.layout),
                    Some(&joint_layout),
                    Some(&shadow_pass_layout),
                ],
                immediate_size: 0,
            });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let depth_view = create_depth_view(&device, width, height);

        // 阶段 D：阴影通道管线 —— 无颜色目标，只写深度。
        // 顶点布局与主通道完全一致（同一份实例缓冲直接复用），
        // 因此阴影通道不需要任何额外 CPU 侧顶点处理。
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-shadow-pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // 斜率相关偏移：斜射面最容易出 acne，硬件斜率偏移比纯常数偏移稳
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-instance-buffer"),
            size: INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            surface,
            config,
            size: (width, height),
            pipeline,
            camera_buffer,
            camera_bind_group,
            depth_view,
            mesh_cache: MeshCache::new(),
            material_cache,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            joint_buffer,
            joint_bind_group,
            joint_scratch: Vec::new(),
            shadow_size,
            _shadow_texture: shadow_texture,
            shadow_view,
            shadow_sampler,
            shadow_layout,
            shadow_bind_group,
            shadow_uniform: shadow_uniform_buffer,
            shadow_pass_bind_group,
            shadow_pass_uniform,
            shadow_pipeline,
            stats: RenderStats::default(),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, width, height);
        log::debug!("窗口尺寸变化: {width}x{height}");
    }

    /// 绘制一帧。场景的任何变化都会在下一帧生效，不需要重建管线。
    pub fn render(&mut self, scene: &Scene) {
        let (groups, culled, cull_unknown, bounds) = self.prepare_instances(scene);
        self.prepare_materials(&groups);
        // 阶段 D：先把本帧的阴影贴图渲好，主通道再采样它
        let shadow_on = self.prepare_shadow(&groups, bounds, scene.light);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.surface.configure(&self.device, &self.config);
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("surface 已失效，重新配置");
                self.surface.configure(&self.device, &self.config);
                return;
            }
            other => {
                log::debug!("跳帧: {other:?}");
                return;
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let bg = scene.background;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("r3e-encoder"),
            });

        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        let mut shadow_draws = 0u32;

        // 阶段 D：阴影通道。只画进深度图，不触碰颜色附件。
        if shadow_on {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.shadow_pipeline);
            // 阴影管线只真正用到 group(2)（蒙皮关节矩阵），但 wgpu 要求管线布局里
            // 低索引的绑定组必须逐个 set（否则报 "expects a BindGroup at index 0"），
            // 所以这里顺带绑上相机与默认材质 —— 顶点着色器不采样它们，纯占位。
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            if let Some(default_material) = self.material_cache.get(MaterialKey::Default) {
                pass.set_bind_group(1, &default_material.bind_group, &[]);
            }
            pass.set_bind_group(2, &self.joint_bind_group, &[]);
            // group(3) 这里是「只含光源矩阵」的阴影通道专用绑定组，
            // 不是主通道那张比较采样器 —— 深度图此刻正被本 pass 写入。
            pass.set_bind_group(3, &self.shadow_pass_bind_group, &[]);
            for (key, range) in &groups {
                let Some(gpu) = self.mesh_cache.get(*key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
                pass.set_vertex_buffer(
                    1,
                    self.instance_buffer
                        .slice(range.start as u64 * stride..range.end as u64 * stride),
                );
                pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for part in &gpu.parts {
                    pass.draw_indexed(
                        part.first_index..part.first_index + part.index_count,
                        0,
                        0..1,
                    );
                    shadow_draws += 1;
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.x as f64,
                            g: bg.y as f64,
                            b: bg.z as f64,
                            a: bg.w as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(2, &self.joint_bind_group, &[]);
            // 阶段 D：主通道采样上一步渲好的阴影贴图
            pass.set_bind_group(3, &self.shadow_bind_group, &[]);
            let mut draw_calls = 0u32;
            let mut instances = 0u32;

            for (key, range) in &groups {
                let Some(gpu) = self.mesh_cache.get(*key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
                pass.set_vertex_buffer(
                    1,
                    self.instance_buffer
                        .slice(range.start as u64 * stride..range.end as u64 * stride),
                );
                pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                // 每个材质分区一次 draw：bind group 1 换成该分区的材质。
                for part in &gpu.parts {
                    let Some(material) = self
                        .material_cache
                        .get(part.material)
                        .or_else(|| self.material_cache.get(MaterialKey::Default))
                    else {
                        continue;
                    };
                    pass.set_bind_group(1, &material.bind_group, &[]);
                    pass.draw_indexed(
                        part.first_index..part.first_index + part.index_count,
                        0,
                        0..1,
                    );
                    draw_calls += 1;
                }
                instances += range.end - range.start;
            }

            self.stats = RenderStats {
                draw_calls,
                instances,
                culled,
                cull_unknown,
                shadow_draws,
                frame_index: scene.frame,
            };
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
    }

    /// 阶段 D：把阴影贴图尺寸对齐场景配置；返回本帧是否要跑阴影通道。
    fn prepare_shadow(
        &mut self,
        groups: &[(MeshRef, std::ops::Range<u32>)],
        bounds: Option<Aabb>,
        light: Light,
    ) -> bool {
        let shadow = light.shadow.sanitized();

        // 尺寸变化就重建纹理 + bind group（贴图句柄必须换新，不能原地 resize）
        if shadow.size != self.shadow_size {
            self.resize_shadow(shadow.size);
        }

        // 拟合光源正交相机：用全部可见物件的包围盒中心 + 半径，留 5% 余量防边缘裁切
        let (center, radius) = match bounds {
            Some(b) if !b.is_empty() => {
                let center = b.center();
                let radius = (b.size().length() * 0.5).max(0.25);
                (center, radius)
            }
            // 空场景也要给出合法矩阵，否则 uniform 里是恒等矩阵、采样会乱
            _ => (Vec3::ZERO, 8.0),
        };
        let view_proj = light.shadow_view_projection(center, radius * 1.05);
        // ortho_size = 0 表示按场景包围球自适应，否则用用户给定值（仍是半宽）
        let half = if shadow.ortho_size > 0.0 {
            shadow.ortho_size
        } else {
            radius * 1.05
        };
        let texel_world = (2.0 * half / shadow.size as f32).max(1e-5);

        let uniform = ShadowUniform {
            light_view_proj: view_proj.to_cols_array_2d(),
            params: [
                1.0 / shadow.size as f32,
                shadow.bias,
                if shadow.enabled { 1.0 } else { 0.0 },
                shadow.normal_bias * texel_world,
            ],
            params2: [shadow.strength, 0.0, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.shadow_uniform, 0, bytemuck::bytes_of(&uniform));

        // 深度 pass 只认这一份矩阵（不碰会被主通道采样的 ShadowUniform）
        let pass_uniform = ShadowPassUniform {
            light_view_proj: view_proj.to_cols_array_2d(),
            pad: [0.0; 4],
        };
        self.queue.write_buffer(
            &self.shadow_pass_uniform,
            0,
            bytemuck::bytes_of(&pass_uniform),
        );

        shadow.enabled && !groups.is_empty()
    }

    /// 重建阴影深度图与对应的 bind group（改 `shadow.size` 时触发）。
    fn resize_shadow(&mut self, size: u32) {
        let (texture, view) = create_shadow_target(&self.device, size);
        self.shadow_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-shadow-bg"),
            layout: &self.shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.shadow_uniform.as_entire_binding(),
                },
            ],
        });
        self.shadow_view = view;
        self._shadow_texture = texture;
        self.shadow_size = size;
        log::info!("阴影贴图重建: {size}x{size}");
    }

    /// 注入资产库（由应用层加载 FBX 后调用）。渲染层本身不认识文件 IO。
    pub fn set_asset_library(&mut self, library: Arc<dyn MeshProvider>) {
        self.mesh_cache.set_library(library);
        // 资产库换了，材质 / 贴图句柄可能失效，整批丢弃重来。
        self.material_cache.materials.clear();
        self.material_cache.textures.clear();
    }

    /// 收齐本帧要用到的材质并建好 bind group（默认材质也要有一份）。
    fn prepare_materials(&mut self, groups: &[(MeshRef, std::ops::Range<u32>)]) {
        let mut keys: Vec<MaterialKey> = vec![MaterialKey::Default];
        for (key, _) in groups {
            let Some(gpu) = self.mesh_cache.get(*key) else {
                continue;
            };
            for part in &gpu.parts {
                if !keys.contains(&part.material) {
                    keys.push(part.material);
                }
            }
        }
        let library = self.mesh_cache.provider().clone();
        for key in keys {
            self.material_cache
                .ensure(&self.device, &self.queue, Some(&library), key);
        }
    }

    pub fn mesh_cache(&self) -> &MeshCache {
        &self.mesh_cache
    }

    /// 把场景里可见的物件整理成按 `MeshRef` 分组的实例区间，并上传到 GPU。
    ///
    /// 返回值附带本帧的剔除统计
    /// `(绘制分组, 剔除数, 缺包围盒数, 全部可见物件的世界包围盒)`；
    /// 最后一项供阴影通道生成光源正交相机（阶段 D）。
    fn prepare_instances(&mut self, scene: &Scene) -> InstancePlan {
        // 更新相机 / 光照 uniform
        let mut camera = scene.camera;
        camera.aspect = self.aspect();
        let light = scene.light;
        let uniform = CameraUniform {
            view_proj: camera.view_projection().to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_dir: [light.direction.x, light.direction.y, light.direction.z, 0.0],
            light_color: [light.color.x, light.color.y, light.color.z, light.intensity],
            ambient: [light.ambient.x, light.ambient.y, light.ambient.z, 1.0],
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // 分组键改为 MeshRef：内置网格与资产网格统一处理，按首次出现顺序稳定排列
        let mut keys: Vec<MeshRef> = Vec::new();
        for node in scene.iter() {
            if !node.visible {
                continue;
            }
            if !keys.contains(&node.mesh) {
                keys.push(node.mesh);
            }
        }

        let mut all: Vec<InstanceRaw> = Vec::new();
        let mut groups: Vec<(MeshRef, std::ops::Range<u32>)> = Vec::new();
        // 每帧重算关节槽位：同一块 storage buffer 按实例依次排布（P4）
        self.joint_scratch.clear();
        let mut joint_overflow = 0usize;
        // 阶段三 A：先用 6 个视锥平面过一遍节点，被剔除的实例不占实例槽位、也不占关节槽位
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let mut culled = 0u32;
        let mut cull_unknown = 0u32;
        // 阶段 D：把「所有可见（含被视锥剔除）物件」的包围盒并起来，
        // 交给阴影通道拟合正交相机 —— 阴影要考虑镜头外的遮挡物，
        // 所以这里统计在剔除判断之前。
        let mut scene_bounds: Option<Aabb> = None;

        for key in &keys {
            let start = all.len() as u32;
            for node in scene.iter() {
                if !node.visible || node.mesh != *key {
                    continue;
                }
                match self.mesh_cache.bounds(*key) {
                    Some(local) => {
                        // 蒙皮网格的形变会跑出静止姿态包围盒，按比例留余量再测
                        let world = local.transformed(&node.world);
                        let world = if scene.skin_of(node.id).is_some() {
                            world.inflate_relative(SKIN_CULL_MARGIN)
                        } else {
                            world
                        };
                        scene_bounds = Some(match scene_bounds {
                            Some(b) => b.union(&world),
                            None => world,
                        });
                        if !frustum.intersects_aabb(&world) {
                            culled += 1;
                            continue;
                        }
                    }
                    // 拿不到包围盒就不剔除（fail-open），宁可多画也不让物件凭空消失
                    None => cull_unknown += 1,
                }
                let color: Vec4 = node.color;
                let joint_base = match scene.skin_of(node.id) {
                    Some(skin) if skin.is_solved() => {
                        let need = skin.joint_matrices.len();
                        if self.joint_scratch.len() + need > r3e_core::MAX_JOINT_MATRICES {
                            joint_overflow += need;
                            0
                        } else {
                            let base = self.joint_scratch.len() as u32;
                            for m in &skin.joint_matrices {
                                self.joint_scratch.push(m.to_cols_array());
                            }
                            base
                        }
                    }
                    _ => 0,
                };
                all.push(InstanceRaw {
                    model: node.world.to_cols_array_2d(),
                    color: color.to_array(),
                    joint_base,
                    _pad: [0; 3],
                });
            }
            if all.len() as u32 > start {
                groups.push((*key, start..all.len() as u32));
            }
        }

        if joint_overflow > 0 {
            log::error!(
                "关节矩阵槽位不足（缺 {joint_overflow} 个，上限 {}），超出部分实例按非蒙皮绘制",
                r3e_core::MAX_JOINT_MATRICES
            );
        }
        if !self.joint_scratch.is_empty() {
            self.queue.write_buffer(
                &self.joint_buffer,
                0,
                bytemuck::cast_slice(&self.joint_scratch),
            );
        }

        // 确保用到的网格都已上传（内置即时生成，资产从注入的 MeshProvider 取）
        let mut uploaded: Vec<MeshRef> = Vec::new();
        for (key, _) in &groups {
            if self.mesh_cache.ensure(&self.device, &self.queue, *key) {
                uploaded.push(*key);
            }
        }
        // 上传失败的网格从绘制分组中剔除
        if uploaded.len() != groups.len() {
            groups.retain(|(key, _)| uploaded.contains(key));
        }

        if all.is_empty() {
            return (groups, culled, cull_unknown, scene_bounds);
        }

        // 容量不足则扩容（按 2 倍增长，避免每帧重建）
        if all.len() as u64 > self.instance_capacity {
            let mut cap = self.instance_capacity;
            while cap < all.len() as u64 {
                cap *= 2;
            }
            self.instance_capacity = cap;
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-instance-buffer"),
                size: cap * std::mem::size_of::<InstanceRaw>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&all));

        (groups, culled, cull_unknown, scene_bounds)
    }
}

/// 上传一份 CPU 网格到 GPU。`parts` 为空时按「整网格一段默认材质」补齐。
fn upload_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &MeshData,
    mut parts: Vec<GpuPart>,
) -> GpuMesh {
    let index_count = data.indices.len() as u32;
    if parts.is_empty() && index_count > 0 {
        parts.push(GpuPart {
            first_index: 0,
            index_count,
            material: MaterialKey::Default,
        });
    } else {
        // 资产层给的分区若越界（数据错误），裁到索引范围内，避免 draw 越界。
        for part in &mut parts {
            if part.first_index >= index_count {
                part.index_count = 0;
                continue;
            }
            part.index_count = part
                .index_count
                .min(index_count.saturating_sub(part.first_index));
        }
        parts.retain(|p| p.index_count > 0);
        if parts.is_empty() && index_count > 0 {
            parts.push(GpuPart {
                first_index: 0,
                index_count,
                material: MaterialKey::Default,
            });
        }
    }
    let vertex_size = (data.vertices.len().max(1) * std::mem::size_of::<Vertex>()) as u64;
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r3e-vertex-buffer"),
        size: vertex_size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !data.vertices.is_empty() {
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&data.vertices));
    }

    let index_size = (data.indices.len().max(1) * std::mem::size_of::<u32>()) as u64;
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r3e-index-buffer"),
        size: index_size,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !data.indices.is_empty() {
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&data.indices));
    }

    GpuMesh {
        vertex_buffer,
        index_buffer,
        parts,
    }
}

/// 一个材质在 GPU 侧的状态。
struct GpuMaterial {
    bind_group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
}

/// 材质缓存：材质键 → bind group；贴图按 `(资产, 贴图下标)` 单独缓存，避免重复上传。
///
/// 任何一步取不到数据（材质缺失 / 贴图解码失败 / 超出纹理尺寸限制）都不会 panic，
/// 而是回退到「默认白色 + 纯色」的兜底材质，保证画面继续跑。
struct MaterialCache {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    materials: HashMap<MaterialKey, GpuMaterial>,
    textures: HashMap<(AssetId, u32), wgpu::TextureView>,
    max_texture_dim: u32,
}

impl MaterialCache {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-material-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-material-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let white = create_white_texture(device);

        let mut cache = Self {
            layout,
            sampler,
            white,
            materials: HashMap::new(),
            textures: HashMap::new(),
            max_texture_dim: 8192,
        };
        cache.max_texture_dim = device.limits().max_texture_dimension_2d;
        cache
    }

    fn get(&self, key: MaterialKey) -> Option<&GpuMaterial> {
        self.materials.get(&key)
    }

    /// 确保材质已就绪；失败时退回默认材质（调用方永远能拿到一组 bind group）。
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        library: Option<&Arc<dyn MeshProvider>>,
        key: MaterialKey,
    ) {
        if self.materials.contains_key(&key) {
            return;
        }

        let data = match (key, library) {
            (MaterialKey::Asset { asset, index }, Some(library)) => {
                library.material(asset, index).unwrap_or_else(|| {
                    log::warn!("资产 {} 缺少材质 {}，回退默认材质", asset, index);
                    MaterialData::default()
                })
            }
            _ => MaterialData::default(),
        };

        let (view, has_texture) = match (data.base_color_texture, library) {
            (Some((asset, index)), Some(library)) => {
                match self.texture_view(device, queue, asset, index, library) {
                    Some(view) => (view, true),
                    None => (self.white.clone(), false),
                }
            }
            _ => (self.white.clone(), false),
        };

        let uniform = MaterialUniform {
            base_color: data.base_color,
            params: [
                data.metallic,
                data.roughness,
                if has_texture { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-material-ubo"),
            size: std::mem::size_of::<MaterialUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&uniform));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-material-bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.materials.insert(
            key,
            GpuMaterial {
                bind_group,
                _uniform: buffer,
            },
        );
    }

    fn texture_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        asset: AssetId,
        index: u32,
        library: &Arc<dyn MeshProvider>,
    ) -> Option<wgpu::TextureView> {
        if let Some(view) = self.textures.get(&(asset, index)) {
            return Some(view.clone());
        }
        let image = library.texture(asset, index)?;
        let view = upload_texture(device, queue, &image, self.max_texture_dim)?;
        log::info!(
            "贴图已上传 GPU: asset {} tex {} — {}x{}",
            asset,
            index,
            image.width,
            image.height
        );
        self.textures.insert((asset, index), view.clone());
        Some(view)
    }
}

fn create_white_texture(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-white-1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// 把 RGBA8 像素上传成 sRGB 贴图（源色即 sRGB，采样后自动线性化）。
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &TextureImage,
    max_dim: u32,
) -> Option<wgpu::TextureView> {
    if !image.is_valid() {
        log::warn!(
            "贴图像素数据不完整（{}x{}），跳过",
            image.width,
            image.height
        );
        return None;
    }
    if image.width > max_dim || image.height > max_dim {
        log::warn!(
            "贴图 {}x{} 超过设备上限 {max_dim}，跳过",
            image.width,
            image.height
        );
        return None;
    }
    let size = wgpu::Extent3d {
        width: image.width,
        height: image.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-asset-texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        size,
    );
    Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// 阶段 D：创建正方形阴影深度图（可作渲染目标，也可被比较采样）。
fn create_shadow_target(device: &wgpu::Device, size: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let size = size.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-shadow-map"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
