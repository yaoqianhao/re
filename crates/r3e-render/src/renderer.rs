//! wgpu 渲染器：单一前向管线 + 实例化绘制。
//!
//! 场景（`r3e_core::Scene`）是唯一真源，渲染器每帧从场景重建实例数据，
//! 因此脚本或 JSON 对物件的任何修改都会在下一帧直接反映到画面上。

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use r3e_core::mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage, Vertex};
use r3e_core::{
    Aabb, AssetId, CasterNode, CasterTree, EnvironmentSettings, Frustum, Light, LocalLight,
    MeshRef, ProceduralProvider, Scene, Sh9, MAX_CASCADES,
};
use winit::window::Window;

use crate::envprobe::{self, EnvProbeSource, EnvProbeStats};
use crate::mesh::{vertex_layout, InstanceRaw};
use crate::occlusion::{OcclusionCuller, OcclusionStats};
use crate::pbr::{MaterialUniform, PbrMapFlags, SurfaceKey};
use crate::postfx::{PostFxPass, PostFxStats};
use crate::ssao::{SsaoPass, SsaoStats};
use crate::text::{TextPass, TextStats};
use crate::transparency;
use crate::ui::{UiImageProvider, UiPass, UiStats};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const INITIAL_INSTANCE_CAPACITY: u64 = 512;
/// 主通道的 MSAA 采样数（扩展十九）。
///
/// 4× 是 wgpu 三大后端（Vulkan / Metal / DX12）对 `Rgba16Float` 的共同下限，
/// 取它才能保证「同一份代码在哪都能跑」。主通道在多重采样附件上光栅化，
/// pass 结束时由硬件 resolve 到单采样 HDR 目标 —— 后处理链仍然只看到单采样纹理。
pub const MSAA_SAMPLE_COUNT: u32 = 4;
/// 蒙皮网格做视锥剔除时的包围盒外扩比例：动画形变会超出静止姿态的 AABB。
const SKIN_CULL_MARGIN: f32 = 0.25;

/// 材质键：默认材质，或「某资产内第 N 个材质」，可再叠加「物件级 PBR 覆盖」。
///
/// 渲染层只认这个键，具体材质参数通过 `MeshProvider::material` 取，
/// 因此这里依旧不依赖任何模型文件格式。物件级覆盖（场景 / 脚本写入的
/// metallic / roughness / emissive）用 [`SurfaceKey`] 参与键值，保证同一网格的
/// 不同 PBR 参数各自成批、互不串味。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MaterialKey {
    Default,
    Asset {
        asset: AssetId,
        index: u32,
    },
    /// 纯粹的物件级材质（无资产材质可继承，base color 用 uniform 常量）
    Surface(SurfaceKey),
    /// 资产材质 + 物件级覆盖
    AssetSurface {
        asset: AssetId,
        index: u32,
        surface: SurfaceKey,
    },
}

impl MaterialKey {
    /// 叠加物件级 PBR 覆盖；`None` 原样返回。
    fn with_surface(self, surface: Option<SurfaceKey>) -> Self {
        match (self, surface) {
            (key, None) => key,
            (MaterialKey::Asset { asset, index }, Some(surface)) => MaterialKey::AssetSurface {
                asset,
                index,
                surface,
            },
            (MaterialKey::Default, Some(surface)) | (MaterialKey::Surface(_), Some(surface)) => {
                MaterialKey::Surface(surface)
            }
            (key @ MaterialKey::AssetSurface { .. }, Some(_)) => key,
        }
    }

    /// 拆成「资产材质来源 + 物件级覆盖」两部分。
    fn split(self) -> (Option<(AssetId, u32)>, Option<SurfaceKey>) {
        match self {
            MaterialKey::Default => (None, None),
            MaterialKey::Asset { asset, index } => (Some((asset, index)), None),
            MaterialKey::Surface(surface) => (None, Some(surface)),
            MaterialKey::AssetSurface {
                asset,
                index,
                surface,
            } => (Some((asset, index)), Some(surface)),
        }
    }
}

/// 全局相机 / 光照 uniform。288 字节，16 字节对齐。
///
/// 布局必须与 `shader.wgsl` 的 `Camera` 逐字段一致：相机矩阵 → 光照 → 环境探针 → 9 项球谐。
/// 环境探针部分只有在 `env_horizon.w > 0.5` 时才被着色器采用，否则退回 `ambient` 常数项，
/// 因此老场景（不写 `environment`）的画面与扩展十二之前逐像素一致。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    /// rgb = 光色，a = 强度
    light_color: [f32; 4],
    ambient: [f32; 4],
    /// rgb = 天顶色，a = IBL 总强度
    env_zenith: [f32; 4],
    /// rgb = 地平线色，a = 是否启用 IBL（0/1）
    env_horizon: [f32; 4],
    /// rgb = 地面色，a 保留
    env_ground: [f32; 4],
    /// rgb = 太阳色 × 太阳强度，a 保留
    env_sun: [f32; 4],
    /// xyz = 指向太阳的单位向量，a 保留
    env_sun_dir: [f32; 4],
    /// 9 项球谐：rgb = 系数（已是辐亮度量纲），a 保留。由 `env::Sh9::project` 在 CPU 侧投影。
    /// 扩展二十三：配了 HDR 探针时，这里换成探针自身投影出的辐照度系数（仍是 9 项），
    /// 漫反射分支因此无需改动；未配探针时与扩展十二逐字节相同。
    env_sh: [[f32; 4]; r3e_core::env::SH_COEFFICIENTS],
    /// 扩展二十三：HDR 立方体反射探针。
    /// x = 是否启用（0/1），y = 镜面 mip 层数，z = 探针强度，w = 面边长（诊断用）。
    env_probe: [f32; 4],
}

impl Default for CameraUniform {
    fn default() -> Self {
        let env = EnvironmentSettings::default();
        Self {
            view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos: [0.0, 0.0, 0.0, 1.0],
            light_dir: [0.0, -1.0, 0.0, 0.0],
            light_color: [1.0, 1.0, 1.0, 1.0],
            ambient: [0.1, 0.1, 0.1, 1.0],
            env_zenith: [
                env.sky_zenith[0],
                env.sky_zenith[1],
                env.sky_zenith[2],
                env.intensity,
            ],
            env_horizon: [
                env.sky_horizon[0],
                env.sky_horizon[1],
                env.sky_horizon[2],
                0.0,
            ],
            env_ground: [env.ground[0], env.ground[1], env.ground[2], 0.0],
            env_sun: [
                env.sun_color[0] * env.sun_intensity,
                env.sun_color[1] * env.sun_intensity,
                env.sun_color[2] * env.sun_intensity,
                0.0,
            ],
            env_sun_dir: [env.sun_dir[0], env.sun_dir[1], env.sun_dir[2], 0.0],
            env_sh: [[0.0; 4]; r3e_core::env::SH_COEFFICIENTS],
            // 缺省 = 没有探针：着色器不采样立方体，镜面走解析天空
            env_probe: [0.0; 4],
        }
    }
}

/// 扩展十五：单盏局部光源的 GPU 布局（64 字节 = 4 个 vec4）。
///
/// 与 `shader.wgsl` 的 `struct LocalLight` 逐字段一致：
/// `pos_range`（xyz 位置 + w 半径）、`color_power`（rgb 颜色 + w 强度）、
/// `dir_inner`（xyz 方向 + w 内角余弦）、`outer_kind`（x 外角余弦 + y 类型码）。
///
/// 为什么打包成 vec4 而不是 `[f32; 16]`：GPU 侧的 uniform 读取本就按 16 字节对齐，
/// 拆成 vec4 后 wgsl 里可以直接 `.xyz` / `.w` 取，省掉 CPU 侧的偏移手算。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LocalLightRaw {
    pub pos_range: [f32; 4],
    pub color_power: [f32; 4],
    pub dir_inner: [f32; 4],
    pub outer_kind: [f32; 4],
}

/// 单盏灯的字节步长。契约测试锁死它，防止哪天加字段忘了同步 wgsl 的数组长度。
pub const LOCAL_LIGHT_STRIDE: u64 = std::mem::size_of::<LocalLightRaw>() as u64;

/// 局部光源表 uniform：1 个计数 vec4 + 定长 16 盏灯。
///
/// `count.x` 是**实际参与计算**的灯数：总开关关闭时写 0（数据仍留在场景里，
/// 再打开即恢复），wgsl 的循环上界取 `min(count.x, MAX_LOCAL_LIGHTS)`。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LightsUniform {
    /// x = 有效灯数，yzw 保留
    pub count: [f32; 4],
    pub items: [LocalLightRaw; r3e_core::MAX_LOCAL_LIGHTS],
}

/// 局部光源表的 uniform 总字节数。
pub const LIGHTS_UNIFORM_SIZE: u64 = std::mem::size_of::<LightsUniform>() as u64;

impl Default for LightsUniform {
    fn default() -> Self {
        Self {
            count: [0.0; 4],
            items: [LocalLightRaw::zeroed(); r3e_core::MAX_LOCAL_LIGHTS],
        }
    }
}

impl LightsUniform {
    /// 把场景里的灯打包成 uniform。
    ///
    /// 约定：
    /// * `enabled == false` → 计数写 0，GPU 整个循环跳过（数据不动，打开即恢复）；
    /// * 只取前 `MAX_LOCAL_LIGHTS` 盏（场景层已经截断过，这里再夹一次防越界）；
    /// * 每盏灯先跑一次 `sanitized()`，保证漫延到 GPU 的数值一定合法；
    /// * 余弦在 CPU 侧算好（`cos_inner` / `cos_outer`），省掉片元里两次 `cos`。
    pub fn from_lights(lights: &[LocalLight], enabled: bool) -> Self {
        let mut out = Self::default();
        if !enabled {
            return out;
        }
        let count = lights.len().min(r3e_core::MAX_LOCAL_LIGHTS);
        for (i, light) in lights.iter().take(count).enumerate() {
            let l = light.clone().sanitized();
            out.items[i] = LocalLightRaw {
                pos_range: [l.pos[0], l.pos[1], l.pos[2], l.range],
                color_power: [l.color[0], l.color[1], l.color[2], l.intensity],
                dir_inner: [l.dir[0], l.dir[1], l.dir[2], l.cos_inner()],
                outer_kind: [l.cos_outer(), l.kind.gpu_code(), 0.0, 0.0],
            };
        }
        out.count = [count as f32, 0.0, 0.0, 0.0];
        out
    }
}

/// 阶段 D / 扩展十六：阴影通道的 uniform —— 每级一个光源正交矩阵 + 采样参数。
///
/// `light_view_proj` 按级下标存放（未使用的级复制最后一级的矩阵，避免留垃圾）；
/// `split_ndc` 存各级**远端**在相机裁剪空间里的 z（NDC，0..1）：像素拿自己的
/// `ndc_z` 从左往右找第一个 `ndc_z <= split_ndc[i]` 的 i，就是它该采样的级。
/// `params`：x = 1/size（纹素尺寸），y = 深度比较偏移，z = 是否启用（0/1），
/// w = 第 0 级的法线方向偏移（世界单位，已按纹素与世界尺寸换算）。
/// `params2`：x = 阴影强度，y = 本帧实际级数，z = 级间混合带宽（占该级跨度的比例）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ShadowUniform {
    light_view_proj: [[[f32; 4]; 4]; MAX_CASCADES],
    split_ndc: [f32; 4],
    params: [f32; 4],
    params2: [f32; 4],
}

// ---------------- 扩展十八 C：局部光源阴影 ----------------

/// 局部阴影 uniform 里「每盏灯一项」的字节步长（vec4）。
pub const LOCAL_SHADOW_SLOT_STRIDE: u64 = 16;

/// 局部阴影的法线偏移系数：偏移量 = 系数 × 该灯单纹素世界尺寸。
/// 与方向光的 `ShadowSettings::normal_bias` 同一思路，但按灯各自的 range 换算。
pub const LOCAL_SHADOW_NORMAL_BIAS: f32 = 1.5;

/// 局部光源阴影（扩展十八 C）的 uniform。
///
/// 与方向光的 `ShadowUniform` 完全分开：方向光存「正交矩阵 + 级联切分」，
/// 这里存「每层透视矩阵 + 每盏灯的槽位」。分开的理由是语义不同——局部阴影
/// 没有级联概念，层号由 `plan_local_shadows` 的固定槽位决定，而不是按视距选。
///
/// 字段约定：
/// * `params`：x = 本帧投影灯数，y = 1/size（纹素尺寸），z = 是否启用（0/1），
///   w = 保留（每灯的法线偏移在 `slots[i].w`，逐灯不同，放不进全局 vec4）；
/// * `params2`：x = 阴影强度，y = 本帧占用的层数，zw 保留；
/// * `view_proj[layer]`：该层「光源 → 裁剪」矩阵，层号 = `layer_base + face`；
/// * `slots[i]`：x = `layer_base`（-1 表示这盏灯本帧不投影），y = 面数（点光 6 / 聚光 1），
///   z = 该灯的深度偏移，w = 该灯的法线偏移（世界单位）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LocalShadowUniform {
    pub params: [f32; 4],
    pub params2: [f32; 4],
    pub view_proj: [[[f32; 4]; 4]; r3e_core::MAX_LOCAL_SHADOW_LAYERS],
    pub slots: [[f32; 4]; r3e_core::MAX_LOCAL_LIGHTS],
}

/// 局部阴影 uniform 的总字节数（契约测试锁死，防止与 wgsl 数组长度脱节）。
pub const LOCAL_SHADOW_UNIFORM_SIZE: u64 = std::mem::size_of::<LocalShadowUniform>() as u64;

impl Default for LocalShadowUniform {
    fn default() -> Self {
        Self {
            params: [0.0; 4],
            params2: [0.0; 4],
            view_proj: [[[0.0; 4]; 4]; r3e_core::MAX_LOCAL_SHADOW_LAYERS],
            // 全部槽位默认 -1（不投影）：片元读到即早退，不必等数组清零
            slots: [[-1.0, 0.0, 0.0, 0.0]; r3e_core::MAX_LOCAL_LIGHTS],
        }
    }
}

impl LocalShadowUniform {
    /// 把场景里的灯打包成本帧的局部阴影 uniform。
    ///
    /// 约定与 `LightsUniform::from_lights` 对齐：
    /// * `enabled == false`（或局部光总开关关闭）→ 灯数写 0、全部槽位保持 -1，
    ///   渲染层不提交任何深度 pass，画面与没做阴影完全一致；
    /// * 只有 `cast_shadow` 的灯参与，排序与槽位由 `r3e_core::plan_local_shadows` 决定，
    ///   渲染层不重复实现选择口径；
    /// * 矩阵一次性在这里算好（`shadow_face_view_projection`），着色器不再重算投影，
    ///   深度 pass 与主通道因此必然用同一个矩阵，不会出现「写读不一致」的半像素偏移。
    pub fn from_lights(lights: &[LocalLight], enabled: bool) -> Self {
        let mut out = Self::default();
        let texel = 1.0 / r3e_core::DEFAULT_LOCAL_SHADOW_SIZE as f32;
        out.params = [0.0, texel, if enabled { 1.0 } else { 0.0 }, 0.0];
        out.params2 = [1.0, 0.0, 0.0, 0.0];
        if !enabled {
            return out;
        }
        let casters = r3e_core::plan_local_shadows(lights);
        let mut layers = 0u32;
        for caster in &casters {
            let Some(light) = lights.get(caster.light_index).cloned() else {
                continue;
            };
            let light = light.sanitized();
            let texel_world =
                (2.0 * light.range / r3e_core::DEFAULT_LOCAL_SHADOW_SIZE as f32).max(1e-5);
            for face in caster.layer_range() {
                let matrix = light
                    .shadow_face_view_projection((face - caster.layer_base) as usize)
                    .to_cols_array_2d();
                out.view_proj[face as usize] = matrix;
            }
            out.slots[caster.light_index] = [
                caster.layer_base as f32,
                caster.faces as f32,
                light.shadow_bias,
                texel_world * LOCAL_SHADOW_NORMAL_BIAS,
            ];
            layers = layers.max(caster.layer_base + caster.faces);
        }
        out.params[0] = casters.len() as f32;
        out.params2[1] = layers as f32;
        out
    }

    /// 本帧真正要跑深度 pass 的层号（升序）。渲染层据此逐个提交 pass。
    pub fn layers(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for slot in &self.slots {
            let base = slot[0];
            let faces = slot[1];
            if base < 0.0 || faces <= 0.0 {
                continue;
            }
            let base = base as u32;
            for face in 0..faces as u32 {
                out.push(base + face);
            }
        }
        out.sort_unstable();
        out
    }
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
            light_view_proj: [Mat4::IDENTITY.to_cols_array_2d(); MAX_CASCADES],
            split_ndc: [1.0; 4],
            params: [0.0, 0.0, 0.0, 0.0],
            params2: [1.0, 1.0, 0.0, 0.0],
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

    /// 网格材质自带的不透明度（`base_color.a` 的最小值）。
    ///
    /// 一个网格可以有多段材质分区：只要其中任一段是半透明的，整网格就必须走
    /// 半透明通道 —— 否则该分区的混合会在不透明段里被深度写入锁死。取最小值
    /// 而非平均值，正是为了不让「一段透明一段不透明」被平均成不透明。
    ///
    /// 无材质分区（内置图元 / 无材质的资产段）按 1.0 计；NaN 也按 1.0 兜底，
    /// 免得一个坏材质把整批物件拽进半透明通道。取不到任何分区时返回 `None`。
    pub fn material_alpha(&self, key: MeshRef) -> Option<f32> {
        let parts = self.provider.mesh_parts(key);
        if parts.is_empty() {
            return None;
        }
        let mut alpha = 1.0_f32;
        for part in &parts {
            let Some((asset, index)) = part.material else {
                continue;
            };
            // 材质取不到时保持 1.0：宁可当成不透明，也不要凭空捏造一个透明度
            if let Some(material) = self.provider.material(asset, index) {
                let value = material.base_color[3];
                if value.is_finite() {
                    alpha = alpha.min(value.clamp(0.0, 1.0));
                }
            }
        }
        Some(alpha)
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

/// 一帧的离屏读回结果（P0-4 截图）。
///
/// 只给「已转成 RGBA8、行填充已去掉」的紧密像素，编码成什么图片格式由上层决定
/// （编码要引 `image` 之类的依赖，不该压进渲染层）。
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// 紧密排列的 RGBA8，长度恒为 `width * height * 4`；alpha 恒为 255。
    pub rgba8: Vec<u8>,
    /// 读回时用的目标纹理格式（排查偏色用）。
    pub format: wgpu::TextureFormat,
}

impl CapturedFrame {
    /// 像素总字节数，交给调用方做落盘前自检。
    pub fn byte_len(&self) -> usize {
        self.rgba8.len()
    }
}

/// 主通道视锥剔除的层级化读数（扩展二十九）。
///
/// 语义与阴影通道那套（`shadow_pruned` / `shadow_box_tests`）同构，只是下标从
/// 「(级, 遮挡体)」换成「实例」：`instance_tests + pruned` 恒等于「过层级筛的实例数」。
#[derive(Debug, Clone, Copy, Default)]
pub struct CullStats {
    /// 累计重建主通道视锥树的次数。稳态下应保持不变。
    pub tree_builds: u64,
    /// 本帧做的层级盒判定数（被下探的树节点 + 叶子判定）。
    pub box_tests: u32,
    /// 本帧真正落到单个实例上的判定数。
    pub instance_tests: u32,
    /// 没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁）。
    pub pruned: u32,
    /// `pruned` 里来自多实例子树的部分 —— 层级剔除的净收益。
    pub branch_pruned: u32,
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
    /// 扩展十六起每级各跑一遍 pass，因此开启 N 级时约为实例批次数 × N。
    pub shadow_draws: u32,
    /// 本帧生效的阴影级联数（扩展十六）。1 = 单级（等价阶段 D），0 = 关闭。
    pub shadow_cascades: u32,
    /// 本帧阴影按级裁剪判过的 (级, 遮挡体) 对数（扩展二十四）。
    /// 0 = 阴影关闭、没有遮挡体，或 `caster_cull` 被关掉。
    pub shadow_casters: u32,
    /// 其中被裁掉的 (级, 遮挡体) 对数（扩展二十四）。越接近 `shadow_casters`，
    /// 说明远景级剔得越多；恒为 0 说明 `caster_extend` 把范围铺得过大或裁剪关闭。
    pub shadow_culled: u32,
    /// 扩展二十五：没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁）。
    /// `shadow_casters + shadow_pruned` 恒等于「级数 × 遮挡体数」。
    pub shadow_pruned: u32,
    /// 扩展二十五：`shadow_pruned` 里来自多实例子树的部分 —— **层级剔除的净收益**。
    /// 叶子被裁在逐实例判定里也是一次 AABB，省不下来，所以只有这部分算赚。
    pub shadow_branch_pruned: u32,
    /// 扩展二十五：本帧真正做的层级盒判定数。约等于「被下探的树节点数 × 级数」，
    /// 远小于实例数说明树的层级在起作用。
    pub shadow_box_tests: u32,
    /// 扩展二十五：累计重建遮挡体层级树的次数。稳态下应保持不变 —— 逐帧增长
    /// 说明 `Scene::layout_version` 在每帧被推进（增删节点），缓存没生效。
    pub caster_tree_builds: u64,
    /// 扩展二十九：主通道视锥剔除的层级化读数。
    pub cull: CullStats,
    pub frame_index: u64,
    /// 本帧文本通道画出的字形四边形数（M1 文字渲染）。
    pub text_glyphs: u32,
    /// 本帧文本通道的 draw call 数。0 表示文本关闭 / 无字体 / 无文本。
    pub text_draws: u32,
    /// 本帧缺字数（该字符所有已加载字体面都没有），这些字符只占位不绘制。
    pub text_missing: u32,
    /// 本帧 UI 通道画出的四边形数（M1 最小 UI）。
    pub ui_quads: u32,
    /// 本帧 UI 通道的 draw call 数。0 表示 UI 关闭 / 无元素。
    pub ui_draws: u32,
    /// 本帧 HDR 后处理链的 pass 统计（扩展十三）。
    pub postfx: PostFxStats,
    /// 本帧屏幕空间遮蔽的统计（扩展十七）。`passes = 0` 表示整条跳过。
    pub ssao: SsaoStats,
    /// 本帧真正上传并参与计算的局部光源数（扩展十五）。0 = 场景没有灯 / 总开关关闭。
    pub local_lights: u32,
    /// 本帧投影的局部光源数（扩展十八 C，最多 `MAX_LOCAL_SHADOW_CASTERS`）。
    pub local_shadow_casters: u32,
    /// 本帧局部光源阴影提交的深度 pass 层数（点光 6 / 聚光 1）。
    /// 0 = 没有灯开 `cast_shadow`，此时渲染开销与扩展十七完全一致。
    pub local_shadow_layers: u32,
    /// 本帧走半透明通道的 draw call 数（扩展十九）。0 = 场景没有半透明物件。
    pub transparent_draws: u32,
    /// 本帧走半透明通道的实例数。与 `transparent_draws` 一起区分
    /// 「没有半透明物件」与「有但被剔除」。
    pub transparent_instances: u32,
    /// 本帧深度预通道的 draw call 数（扩展十九）。MSAA 之后主通道的深度是
    /// 多重采样的、不能给遮蔽通道用，因此多跑一条单采样深度 pass。
    /// 恒等于不透明批次的 draw call 数；0 表示本帧没有不透明物件。
    pub depth_prepass_draws: u32,
    /// 本帧遮挡剔除的统计（扩展二十二）。`enabled = false` 表示开关关闭或
    /// 深度金字塔还没就绪，此时 `occluded` 恒为 0、画面与关闭前完全一致。
    pub occlusion: OcclusionStats,
    /// 本帧 HDR 反射探针的状态（扩展二十三）。`active = false` 表示没配探针，
    /// 镜面 IBL 走解析天空、画面与扩展十二逐像素一致。
    pub env_probe: EnvProbeStats,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    /// 半透明管线（扩展十九）：与 `pipeline` 同着色器、同 alpha 混合，
    /// 唯一差别是 `depth_write_enabled = false` —— 半透明物件要参与深度测试
    /// （近处仍能挡住远处），但不能把深度写进去，否则会挡掉后画的同类物件。
    transparent_pipeline: wgpu::RenderPipeline,
    /// 深度预通道（扩展十九）：单采样、无颜色目标，只给遮蔽通道供深度。
    depth_prepass_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    /// 单采样深度纹理本体（`depth_view` 的来源）。扩展二十二要把它整张 copy
    /// 到回读缓冲，所以除了 view 还得握着 texture。
    depth_texture: wgpu::Texture,
    /// 遮挡剔除的深度读回与金字塔缓存（扩展二十二）。开关关闭时它一个 GPU
    /// 命令都不产生。
    occlusion: OcclusionCuller,
    /// MSAA 多重采样颜色附件（`MSAA_SAMPLE_COUNT` 样本，`HDR_FORMAT`）。
    /// 主通道写它，pass 结束时 resolve 到 `postfx.hdr_view()` 的单采样目标。
    _msaa_color: wgpu::Texture,
    msaa_color_view: wgpu::TextureView,
    /// MSAA 深度附件。**不 resolve**：深度不能跨样本解析，需要单采样深度的
    /// 遮蔽通道改吃深度预通道写出的 `depth_view`。
    _msaa_depth: wgpu::Texture,
    msaa_depth_view: wgpu::TextureView,
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
    /// 阶段 D / 扩展十六：方向光阴影贴图数组（`MAX_CASCADES` 层）+ 比较采样器。
    shadow_size: u32,
    _shadow_texture: wgpu::Texture,
    /// 整张纹理数组的视图：主通道按级下标采样。
    shadow_view: wgpu::TextureView,
    /// 每层的单独视图：第 i 层作为第 i 级深度 pass 的渲染目标。
    shadow_layer_views: Vec<wgpu::TextureView>,
    shadow_sampler: wgpu::Sampler,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_bind_group: wgpu::BindGroup,
    shadow_uniform: wgpu::Buffer,
    /// 扩展十五：局部光源表 uniform（1 个计数 + 16 盏灯）——随 `shadow_bind_group`
    /// 一起以 group(3) binding(4) 绑定，不单开绑定组。
    lights_buffer: wgpu::Buffer,
    /// 阴影通道专用：每级一份「只含光源正交矩阵」的 uniform 与绑定组
    /// （见 `ShadowPassUniform`）。每级一次 pass，矩阵各自独立。
    shadow_pass_bind_groups: Vec<wgpu::BindGroup>,
    shadow_pass_uniforms: Vec<wgpu::Buffer>,
    /// 扩展十八 C：局部光源阴影图（`MAX_LOCAL_SHADOW_LAYERS` 层）+ 每层视图。
    /// 尺寸固定为 `DEFAULT_LOCAL_SHADOW_SIZE`，不跟方向光的 `shadow.size` 联动。
    local_shadow_size: u32,
    _local_shadow_texture: wgpu::Texture,
    local_shadow_view: wgpu::TextureView,
    local_shadow_layer_views: Vec<wgpu::TextureView>,
    local_shadow_uniform: wgpu::Buffer,
    /// 局部阴影每层一份的「单矩阵」uniform 与绑定组（深度 pass 用）。
    local_shadow_pass_uniforms: Vec<wgpu::Buffer>,
    local_shadow_pass_bind_groups: Vec<wgpu::BindGroup>,
    shadow_pipeline: wgpu::RenderPipeline,
    /// M1 文字渲染：屏幕空间 2D 文本通道，与 3D 主通道完全独立。
    text: TextPass,
    /// M1 最小 UI：屏幕空间 2D UI 通道（矩形 / 边框 / 图片），与文本通道同构但独立。
    ui: UiPass,
    /// 离屏 HDR 目标 + 泛光链 + composite（扩展十三）。常驻：即使场景没有
    /// `postfx` 段，主通道也要先写进 HDR 目标再由它直通到 surface。
    postfx: PostFxPass,
    /// 屏幕空间环境光遮蔽（扩展十七）：吃主通道的深度，产出 AO 图给 composite。
    /// 常驻但只在场景开了 `postfx.ssao` 时提交命令。
    ssao: SsaoPass,
    /// 环境探针（扩展十二）的球谐缓存。`Sh9::project` 要做 2048 次天空采样，
    /// 每帧重算是纯浪费——只在场景 `environment` 段真的变了才重投影。
    env_cache: Option<EnvironmentSettings>,
    env_sh: Sh9,
    /// 累计做过几次球谐投影（只增不减）。正常连续帧里应保持不变——
    /// 若它逐帧增长，说明缓存键比较失效、正在每帧重算。
    env_builds: u64,
    /// 扩展二十三：当前生效的 HDR 反射探针。
    ///
    /// `None` = 没有探针，着色器 `env_probe.x = 0`，镜面走解析天空（画面与扩展十二一致）。
    /// `Some` 里握着 GPU 立方体的所有权（view 已交给材质缓存）与本帧统计。
    env_probe: Option<EnvProbeGpu>,
    /// 扩展二十五：遮挡体的层级树缓存。
    ///
    /// 拓扑（父子关系）随场景结构走，只在 `Scene::layout_version` 前进时重建；
    /// 每帧只刷新各节点的实例包围盒并重聚合子树盒。零 GPU 依赖，纯 CPU 判定。
    caster_tree: Option<CasterTree>,
    /// 节点 id → 树节点下标（与 `caster_tree` 同寿命）。
    caster_slots: HashMap<r3e_core::NodeId, usize>,
    /// `caster_slots` / `caster_tree` 对应的场景结构版本。
    caster_layout: u64,
    /// 本帧各节点自身的实例包围盒（长度 = 树节点数，逐帧覆写）。
    caster_node_bounds: Vec<Aabb>,
    /// 本帧各节点自身的实例数（长度 = 树节点数，逐帧覆写）。
    caster_node_casts: Vec<u32>,
    /// 本帧实例 → 树节点下标的映射（与实例缓冲同序；映射不到的填 `usize::MAX`，
    /// 由 `expand_masks` 的 fail-open 兜成全保留）。
    caster_instance_nodes: Vec<usize>,
    /// 累计重建层级树的次数（只增不减，观测缓存是否真的生效）。
    caster_tree_builds: u64,
    /// 扩展二十九：主通道视锥剔除的层级树缓存。
    ///
    /// 与 `caster_tree` **分开一棵**：那棵只装「进桶的遮挡体」（被视锥剔掉的实例根本
    /// 不在里面），这棵要装「场景里所有可见节点」—— 整枝预筛必须看得见会被剔的实例，
    /// 否则子树盒会缩水成「只剩可见实例」，反而把本该保留的枝剪掉。两者语义不同，
    /// 共用一棵会互相踩。拓扑同样只在 `Scene::layout_version` 前进时重建。
    cull_tree: Option<CasterTree>,
    /// 节点 id → 视锥树节点下标（与 `cull_tree` 同寿命）。
    cull_slots: HashMap<r3e_core::NodeId, usize>,
    /// `cull_tree` / `cull_slots` 对应的场景结构版本。
    cull_layout: u64,
    /// 本帧各节点自身的实例包围盒（长度 = 树节点数，逐帧覆写）。
    cull_node_bounds: Vec<Aabb>,
    /// 本帧各节点自身的实例数（长度 = 树节点数，逐帧覆写）。
    cull_node_casts: Vec<u32>,
    /// 累计重建主通道视锥树的次数。
    cull_tree_builds: u64,
    stats: RenderStats,
}

/// 已上传到 GPU 的反射探针（扩展二十三）。纹理本体必须由渲染器持有：
/// 材质缓存只存 `TextureView`，若纹理被提前释放，视图就会悬空。
struct EnvProbeGpu {
    /// 立方体纹理本体（`Rgba16Float`，`mip_level_count` 层）。
    _texture: wgpu::Texture,
    /// 本帧进 `RenderStats` 的统计。
    stats: EnvProbeStats,
    /// 漫反射辐照度：由基准层（最清晰那层）投影出的 9 项球谐。
    /// 上传到 `Camera::env_sh`，于是着色器的漫反射分支一行都不用改。
    irradiance: Sh9,
}

/// 一个绘制批次：网格 + 实例区间 + 物件级 PBR 覆盖。
///
/// 不透明批次与半透明批次共用同一份实例缓冲，区别只有「落在 `InstancePlan`
/// 的哪一段」以及绘制时用哪条管线（见 [`InstancePlan`] 的透明段说明）。
type DrawGroup = (MeshRef, std::ops::Range<u32>, Option<SurfaceKey>);

/// `prepare_instances` 的返回值。
///
/// 分组键是 `(网格, 物件级 PBR 材质)`：同一网格的物件若 metallic / roughness
/// 不同，会被拆成多个批次，各自绑定自己的材质 uniform。
///
/// 半透明批次（扩展十九）**一律排在分组末尾**：
/// `groups[..transparent_start]` 走不透明管线（写深度），
/// `groups[transparent_start..]` 走半透明管线（不写深度、由远及近）。
/// `transparent_start == groups.len()` 表示本帧没有半透明物件。
struct InstancePlan {
    groups: Vec<DrawGroup>,
    transparent_start: usize,
    culled: u32,
    cull_unknown: u32,
    scene_bounds: Option<Aabb>,
    /// 与实例缓冲同序的遮挡体包围盒（扩展二十四）。
    ///
    /// 阴影通道按级裁剪时要用它；拿不到网格 AABB 的实例填
    /// [`r3e_core::fail_open_bounds`]，语义是「一律不裁」，宁可多画也不少画。
    caster_bounds: Vec<Aabb>,
    /// 与实例缓冲同序的节点 id（扩展二十五）。层级剔除用它把实例挂回场景树：
    /// 按实例顺序展开成树节点掩码时，下标一一对应。
    caster_nodes: Vec<r3e_core::NodeId>,
    /// 扩展二十九：主通道视锥树本帧的层级判定读数（进 `RenderStats::cull`）。
    cull_box_tests: u32,
    cull_instance_tests: u32,
    cull_pruned: u32,
    cull_branch_pruned: u32,
}

/// 一次阴影提交的实例区间：`(groups 下标, 实例缓冲区间)`。
///
/// 扩展二十四之前这里是「每个分组一段全量」，现在可能被级联掩码切成若干段。
type ShadowRun = (usize, std::ops::Range<u32>);

/// 本帧方向光阴影的执行计划（扩展二十四）。
struct ShadowPlan {
    /// 生效级数，0 = 本帧阴影关闭。
    levels: u32,
    /// 每级一份「保留」掩码，下标与实例缓冲一致；按级裁剪关闭时为空。
    masks: Vec<Vec<bool>>,
    /// 判过的 (级, 实例) 对数，进 `RenderStats::shadow_casters`。
    tested: u32,
    /// 被裁掉的 (级, 实例) 对数，进 `RenderStats::shadow_culled`。
    culled: u32,
    /// 扩展二十五：没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁），
    /// 进 `RenderStats::shadow_pruned`。0 = 树为空 / 层级剔除关闭。
    pruned: u32,
    /// 扩展二十五：其中来自多实例子树的部分，进 `RenderStats::shadow_branch_pruned`。
    /// 这才是层级剔除相对「逐实例判定」的净收益。
    branch_pruned: u32,
    /// 扩展二十五：真正做的层级盒判定数，进 `RenderStats::shadow_box_tests`。
    box_tests: u32,
}

/// 把掩码落成实例区间。
///
/// `mask = None` 表示不裁剪（局部光源阴影、或按级裁剪被关掉）：每个分组一段全量，
/// 行为与扩展二十三逐字相同。`Some` 时按组切出保留的连续段。
fn shadow_runs(groups: &[DrawGroup], mask: Option<&[bool]>) -> Vec<ShadowRun> {
    match mask {
        None => groups
            .iter()
            .enumerate()
            .map(|(index, (_, range, _))| (index, range.clone()))
            .collect(),
        Some(mask) => {
            let mut runs = Vec::new();
            for (index, (_, range, _)) in groups.iter().enumerate() {
                let start = range.start as usize;
                let end = (range.end as usize).min(mask.len());
                if start >= end {
                    continue;
                }
                for kept in r3e_core::split_runs(range.start, &mask[start..end]) {
                    runs.push((index, kept));
                }
            }
            runs
        }
    }
}

/// 一次阴影深度 pass 要提交的内容（扩展二十四）。
///
/// 参数已经到 7 个，再逐个传会过 clippy 的 `too_many_arguments`；而且这三样
/// 在语义上是一体的：**画什么**（分组）、**画其中哪几段**（区间）、**实例间步子多大**。
struct ShadowSubmit<'a> {
    groups: &'a [DrawGroup],
    runs: &'a [ShadowRun],
    stride: u64,
}

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

        let material_cache = MaterialCache::new(&device, &queue);

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
                        // 扩展十六：阴影图是 `MAX_CASCADES` 层的纹理数组，主通道按级取层
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                // 扩展十五：局部光源表（点光 / 聚光）。只被片元着色器读取。
                // 塞进 group(3) 而不是另开 group(4)：WebGPU core 规范里
                // `max_bind_groups` 就是 4，第 5 个组（索引 4）直接过不了校验。
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 扩展十八 C：局部光源阴影图（点光立方 6 面 / 聚光单面共用一张分层数组）。
                // 与 binding(0) 的方向光阴影图分开绑定：两者采样语义不同（一个按级联选层，
                // 一个按灯的槽位选层），合并在同一张图上会让层号空间互相踩。
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // 光源表 uniform：尺寸固定（1 个计数 vec4 + 16 × 64B），改灯数不需要重建。
        let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-lights-ubo"),
            size: std::mem::size_of::<LightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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
        // 每级一份矩阵 uniform / 绑定组：深度 pass 逐级复用同一份管线布局
        let shadow_pass_uniforms: Vec<wgpu::Buffer> = (0..MAX_CASCADES)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("r3e-shadow-pass-ubo"),
                    size: std::mem::size_of::<ShadowPassUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let shadow_pass_bind_groups: Vec<wgpu::BindGroup> = shadow_pass_uniforms
            .iter()
            .map(|uniform| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("r3e-shadow-pass-bg"),
                    layout: &shadow_pass_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    }],
                })
            })
            .collect();

        let shadow_size = r3e_core::ShadowSettings::default().size;
        let (shadow_texture, shadow_view, shadow_layer_views) =
            create_shadow_target(&device, shadow_size);
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

        // 扩展十八 C：局部光源阴影。尺寸固定（局部阴影不跟方向光的 `shadow.size`
        // 联动：一盏点光要占 6 层，跟着 4096 走会直接把显存打爆），
        // 每层一张 512×512 深度图，共 24 层（4 盏灯 × 6 面）。
        let local_shadow_size = r3e_core::DEFAULT_LOCAL_SHADOW_SIZE;
        let (local_shadow_texture, local_shadow_view, local_shadow_layer_views) =
            create_local_shadow_target(&device, local_shadow_size);
        let local_shadow_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-local-shadow-ubo"),
            size: LOCAL_SHADOW_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 每层一份「只含该层投影矩阵」的 uniform / 绑定组：与方向光的深度 pass
        // 共用同一个 `shadow_pass_layout`，因此也能共用同一份深度管线。
        let local_shadow_pass_uniforms: Vec<wgpu::Buffer> = (0..r3e_core::MAX_LOCAL_SHADOW_LAYERS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("r3e-local-shadow-pass-ubo"),
                    size: std::mem::size_of::<ShadowPassUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let local_shadow_pass_bind_groups: Vec<wgpu::BindGroup> = local_shadow_pass_uniforms
            .iter()
            .map(|uniform| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("r3e-local-shadow-pass-bg"),
                    layout: &shadow_pass_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    }],
                })
            })
            .collect();

        let shadow_bind_group = create_shadow_bind_group(
            &device,
            &shadow_layout,
            ShadowBindInputs {
                view: &shadow_view,
                sampler: &shadow_sampler,
                uniform: &shadow_uniform_buffer,
                lights: &lights_buffer,
                local_view: &local_shadow_view,
                local_uniform: &local_shadow_uniform,
            },
        );

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
            multisample: wgpu::MultisampleState {
                // 扩展十九：主通道开通 MSAA。光栅化在 4× 附件上进行，
                // pass 结束时 resolve 到单采样 HDR 目标（见 `render_to_view`）。
                count: MSAA_SAMPLE_COUNT,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    // 扩展十三：主通道不再直接写 surface，目标换成后处理的
                    // 线性 HDR 离屏纹理（见 `PostFxPass::hdr_view`）。输出仍是线性值，
                    // sRGB 编码交给 composite 在写 surface 时一次性完成。
                    format: crate::postfx::HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let (depth_texture, depth_view) = create_depth_target(&device, width, height);
        // 扩展十九：MSAA 附件。颜色附件由硬件 resolve 进 HDR 目标，
        // 深度附件不做 resolve（深度跨样本没有物理意义），单采样深度改由预通道提供。
        let (msaa_color, msaa_color_view, msaa_depth, msaa_depth_view) =
            create_msaa_targets(&device, width, height);

        // 扩展十九：半透明管线。除「不写深度」外与主管线逐字段一致——
        // 同一份着色器、同一套混合、同一个 HDR 目标格式，
        // 只是半透明物件不能把深度写下去，否则会挡掉后面同样半透明的物件。
        let transparent_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-transparent-pipeline"),
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
                // 半透明面片多半是单层薄面（玻璃 / 树叶 / 水面），
                // 一旦背面被剔除，从另一侧看过去整块物件就凭空消失
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLE_COUNT,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::postfx::HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // 扩展十九：深度预通道。主通道的深度挂在 4× 附件上，无法给遮蔽通道
        // 当输入（多重采样纹理不能直接采样），所以在主通道之前先用单采样
        // `depth_view` 铺一遍不透明批次的深度。只写深度、无颜色目标，fragment 为 None。
        let depth_prepass_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("r3e-depth-prepass-pipeline"),
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
                // 单样本：目标就是 `depth_view`，与主通道的多重采样附件互不相干
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

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

        let text = TextPass::new(&device, config.format);
        let ui = UiPass::new(&device, config.format);
        // 后处理链与窗口同级：主通道从此写离屏 HDR 目标，不再直接写 surface
        let postfx = PostFxPass::new(&device, &queue, config.format, (width, height));
        // 遮蔽吃主通道深度、产出与 HDR 同尺寸的 AO 图，所以与窗口同级创建
        let ssao = SsaoPass::new(&device, (width, height), &depth_view);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            size: (width, height),
            pipeline,
            transparent_pipeline,
            depth_prepass_pipeline,
            camera_buffer,
            camera_bind_group,
            depth_view,
            depth_texture,
            occlusion: OcclusionCuller::new(),
            _msaa_color: msaa_color,
            msaa_color_view,
            _msaa_depth: msaa_depth,
            msaa_depth_view,
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
            shadow_layer_views,
            shadow_sampler,
            shadow_layout,
            shadow_bind_group,
            shadow_uniform: shadow_uniform_buffer,
            lights_buffer,
            shadow_pass_bind_groups,
            shadow_pass_uniforms,
            local_shadow_size,
            _local_shadow_texture: local_shadow_texture,
            local_shadow_view,
            local_shadow_layer_views,
            local_shadow_uniform,
            local_shadow_pass_uniforms,
            local_shadow_pass_bind_groups,
            shadow_pipeline,
            text,
            ui,
            postfx,
            ssao,
            env_cache: None,
            env_sh: Sh9::default(),
            env_builds: 0,
            env_probe: None,
            caster_tree: None,
            caster_slots: HashMap::new(),
            caster_layout: 0,
            caster_node_bounds: Vec::new(),
            caster_node_casts: Vec::new(),
            caster_instance_nodes: Vec::new(),
            caster_tree_builds: 0,
            cull_tree: None,
            cull_slots: HashMap::new(),
            cull_layout: 0,
            cull_node_bounds: Vec::new(),
            cull_node_casts: Vec::new(),
            cull_tree_builds: 0,
            stats: RenderStats::default(),
        })
    }

    /// 扩展二十三：装载 HDR 反射探针。
    ///
    /// 传 `None` 等价于 [`Self::clear_env_probe`]（退回解析 IBL）。上传整条 mip 链是
    /// 一次性开销：每层每面一块 `write_texture`，因此只在场景加载 / 换探针时调用，
    /// 不要放进每帧循环。
    pub fn set_env_probe(&mut self, source: Option<&EnvProbeSource>) {
        let Some(source) = source else {
            self.clear_env_probe();
            return;
        };
        let prefiltered = source.prefiltered();
        let face_size = prefiltered.base().face_size() as u32;
        let mip_levels = prefiltered.mip_levels().max(1);
        if face_size == 0 {
            log::warn!("反射探针面边长为 0，忽略并退回解析 IBL");
            self.clear_env_probe();
            return;
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r3e-env-probe-cube"),
            size: wgpu::Extent3d {
                width: face_size,
                height: face_size,
                depth_or_array_layers: envprobe::CUBE_FACES as u32,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // 线性 HDR：辐亮度可以远大于 1，8 位会被截断成白斑
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for upload in envprobe::encode_faces(prefiltered) {
            // 预滤波层数可能多于纹理层数（配置被收敛过），多出来的层直接丢弃
            if upload.mip >= mip_levels {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: upload.mip,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: upload.face,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.bytes_per_row()),
                    rows_per_image: Some(upload.size),
                },
                wgpu::Extent3d {
                    width: upload.size,
                    height: upload.size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("r3e-env-probe-cube-view"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            mip_level_count: Some(mip_levels),
            ..Default::default()
        });

        // 换图必须重建全部材质 bind group（视图写在组里）
        self.material_cache.set_env_cube(view);
        self.env_probe = Some(EnvProbeGpu {
            _texture: texture,
            stats: source.stats(),
            // 漫反射用最清晰那层的辐照度投影：模糊层已经被粗糙度卷积过，再积分会双重衰减
            irradiance: prefiltered.base().project_irradiance(),
        });
    }

    /// 扩展二十三：卸载探针，退回扩展十二的解析 IBL（画面逐像素不变）。
    pub fn clear_env_probe(&mut self) {
        if self.env_probe.take().is_none() {
            return;
        }
        self.material_cache.reset_env_cube();
    }

    /// 当前生效的探针统计；没配探针时 `active = false`。
    pub fn env_probe_stats(&self) -> EnvProbeStats {
        self.env_probe.as_ref().map(|p| p.stats).unwrap_or_default()
    }

    /// HDR 后处理链（扩展十三）：离屏目标尺寸、泛光层级、最近一帧生效的设置。
    pub fn postfx(&self) -> &PostFxPass {
        &self.postfx
    }

    /// 屏幕空间遮蔽通道（扩展十七）：遮蔽图尺寸、本帧统计、最近一帧生效的设置。
    pub fn ssao(&self) -> &SsaoPass {
        &self.ssao
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    /// 环境探针的累计球谐投影次数。连续帧之间不该增长（缓存命中），
    /// 供集成测试与遥测确认「没有每帧重算」。
    pub fn environment_builds(&self) -> u64 {
        self.env_builds
    }

    /// 当前生效的环境探针 `Some` = 用 IBL，`None` = 用 `light.ambient` 常数项。
    pub fn environment(&self) -> Option<EnvironmentSettings> {
        self.env_cache
    }

    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// 文本通道（只读）：查统计、查字体来源。
    pub fn text(&self) -> &TextPass {
        &self.text
    }

    /// 文本通道（可变）：开关、换字体。
    pub fn text_mut(&mut self) -> &mut TextPass {
        &mut self.text
    }

    pub fn text_stats(&self) -> TextStats {
        self.text.stats()
    }

    /// 独立开关 2D 文本通道；关掉后主通道 / 阴影通道行为完全不变。
    pub fn set_text_enabled(&mut self, enabled: bool) {
        self.text.set_enabled(enabled);
    }

    /// 用指定字体文件（可多个）替换文本字体集，CJK 字体走这里。
    pub fn load_text_fonts(&mut self, paths: &[std::path::PathBuf]) -> Result<(), String> {
        self.text.load_fonts(paths)
    }

    /// UI 通道（只读）：查统计、查已失败的图片。
    pub fn ui(&self) -> &UiPass {
        &self.ui
    }

    /// UI 通道（可变）：开关、注入图片提供者、清图片缓存。
    pub fn ui_mut(&mut self) -> &mut UiPass {
        &mut self.ui
    }

    pub fn ui_stats(&self) -> UiStats {
        self.ui.stats()
    }

    /// 独立开关 2D UI 通道；关掉后主通道 / 阴影 / 文本行为完全不变。
    pub fn set_ui_enabled(&mut self, enabled: bool) {
        self.ui.set_enabled(enabled);
    }

    /// 注入 UI 图片解码器（应用层复用 asset 的 image 依赖实现）。
    pub fn set_ui_image_provider(&mut self, provider: std::sync::Arc<dyn UiImageProvider>) {
        self.ui.set_image_provider(provider);
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
        let (depth_texture, depth_view) = create_depth_target(&self.device, width, height);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        // 扩展二十二：金字塔绑的是旧尺寸的深度，换尺寸后必须作废，
        // 否则会拿旧画面的 tile 去剔新画面的物件（误剔）
        self.occlusion.reset();
        // 扩展十九：MSAA 附件与窗口同尺寸，必须一起重建，否则 resolve 目标尺寸对不上
        let (msaa_color, msaa_color_view, msaa_depth, msaa_depth_view) =
            create_msaa_targets(&self.device, width, height);
        self._msaa_color = msaa_color;
        self.msaa_color_view = msaa_color_view;
        self._msaa_depth = msaa_depth;
        self.msaa_depth_view = msaa_depth_view;
        // 后处理的离屏目标必须一起重建：纹理不能原地 resize，
        // 不重建就会继续按旧尺寸采样，画面被拉成旧分辨率的内容。
        self.postfx.resize(&self.device, width, height);
        // 遮蔽图与深度图同尺寸，必须跟着一起换
        self.ssao
            .resize(&self.device, width, height, &self.depth_view);
        log::debug!("窗口尺寸变化: {width}x{height}");
    }

    /// 绘制一帧。场景的任何变化都会在下一帧生效，不需要重建管线。
    pub fn render(&mut self, scene: &Scene) {
        let Some(frame) = self.acquire_surface_frame() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(scene, &view);
        self.queue.present(frame);
    }

    /// 取一帧 swapchain 纹理。`None` = 本帧应跳过（surface 失效 / 非致命错误）。
    ///
    /// 抽出来是为了让 [`Self::render`] 与 [`Self::capture_frame`] 共用同一段
    /// 「拿到可绘制目标」前后的错误处理，而不是各抄一份。
    fn acquire_surface_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.surface.configure(&self.device, &self.config);
                Some(f)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("surface 已失效，重新配置");
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                log::debug!("跳帧: {other:?}");
                None
            }
        }
    }

    /// 把场景渲染到任意目标视图（窗口 swapchain 纹理或离屏纹理）。
    ///
    /// 这是**唯一**的绘制实现：窗口渲染与截图都走这里，所以截图内容与用户看到的
    /// 画面逐像素同源（阴影 / 遮蔽 / 后处理 / 文本 / UI 全部包含）。
    fn render_to_view(&mut self, scene: &Scene, view: &wgpu::TextureView) {
        // 扩展二十二：先把上一轮读回的深度取回来（非阻塞），再看本帧有没有金字塔可用。
        // 关闭时这里只做一次 `enabled` 判断，不 poll、不动 buffer。
        let occlusion_settings = scene.occlusion.sanitized();
        self.occlusion.begin_frame(&self.device, occlusion_settings);
        let plan = self.prepare_instances(scene);
        let groups = plan.groups;
        let transparent_start = plan.transparent_start;
        let (culled, cull_unknown) = (plan.culled, plan.cull_unknown);
        self.prepare_materials(&groups);
        // 阶段 D：先把本帧的阴影贴图渲好，主通道再采样它
        // 扩展二十四：多带一份「每级保留哪些实例」的掩码
        let shadow_plan = self.prepare_shadow(
            scene,
            &groups,
            plan.scene_bounds,
            scene.light,
            &scene.camera,
            &plan.caster_bounds,
            &plan.caster_nodes,
        );
        // 扩展十五：局部光源表每帧重传（灯可以被脚本逐帧移动，缓存反而要判等）
        let local_lights = self.prepare_local_lights(scene);
        // 扩展十八 C：本帧要跑深度 pass 的局部阴影层（空 = 没有灯投影）
        let (local_shadow_layers, local_shadow_casters) = self.prepare_local_shadow(scene);

        let bg = scene.background;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("r3e-encoder"),
            });

        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        let mut shadow_draws = 0u32;

        // 阶段 D / 扩展十六：方向光阴影。每级一次 pass，只写进对应层的深度图，
        // 不触碰颜色附件；级数 = 1 时与阶段 D 的单一全场景阴影图完全等价。
        // 扩展二十四：每级只提交落在该级覆盖范围内的遮挡体（掩码为空即全量提交）。
        for level in 0..shadow_plan.levels as usize {
            let runs = shadow_runs(&groups, shadow_plan.masks.get(level).map(|m| m.as_slice()));
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-shadow-pass",
                &self.shadow_layer_views[level],
                &self.shadow_pass_bind_groups[level],
                ShadowSubmit {
                    groups: &groups,
                    runs: &runs,
                    stride,
                },
            );
        }

        // 扩展十八 C：局部光源阴影。层号由 `plan_local_shadows` 的固定槽位决定
        // （点光 6 面、聚光 1 面），每层一次 pass、一次矩阵 uniform。
        // 没有任何灯投影时 `local_shadow_layers` 为空，这一段整体跳过。
        // 局部阴影不做按级裁剪：它们的视锥互不重叠且每盏灯覆盖范围独立，
        // 用方向光的级联掩码套上去只会更慢（扩展二十四明确不覆盖这条路径）。
        let local_runs = shadow_runs(&groups, None);
        for layer in &local_shadow_layers {
            let idx = *layer as usize;
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-local-shadow-pass",
                &self.local_shadow_layer_views[idx],
                &self.local_shadow_pass_bind_groups[idx],
                ShadowSubmit {
                    groups: &groups,
                    runs: &local_runs,
                    stride,
                },
            );
        }

        // 扩展十九：深度预通道。主通道的深度挂在 4× 附件上、不能给遮蔽通道采样，
        // 所以先用单采样 `depth_view` 把不透明批次的深度铺一遍。
        // 只画不透明段：半透明物件本来就不写深度。
        let depth_prepass_draws =
            self.draw_depth_prepass(&mut encoder, &groups[..transparent_start]);

        // 扩展二十二：把这张单采样深度整张 copy 到回读缓冲，供**后续帧**做遮挡判定。
        // 读回是低频的（默认每 4 帧一次），且在飞期间不再发起新的 copy；
        // 关闭时 `encode_readback` 根本不会被调用，命令缓冲与之前逐字节一致。
        let depth_readback = occlusion_settings.enabled
            && self.occlusion.encode_readback(
                &self.device,
                &mut encoder,
                &self.depth_texture,
                self.size.0,
                self.size.1,
                scene.frame,
            );

        let (transparent_draws, transparent_instances);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-main-pass"),
                // 扩展十三：主通道写离屏 HDR 目标（Rgba16Float，线性），
                // 高亮不再被 sRGB 8 位目标直接切平；贴到屏幕由 composite 完成。
                // 扩展十九：颜色先光栅化到 4× 多重采样附件上，由硬件 resolve
                // 进同一个 HDR 目标 —— 后处理链照旧只看到单采样纹理。
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_color_view,
                    depth_slice: None,
                    resolve_target: Some(self.postfx.hdr_view()),
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
                    // 多重采样深度：只服务于本通道的深度测试，
                    // 内容用完即弃（单采样深度已由预通道写好）
                    view: &self.msaa_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
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
            // 不透明段：写深度、顺序无关，分组顺序即首次出现顺序
            let (opaque_draws, opaque_instances) =
                self.draw_group_range(&mut pass, &groups[..transparent_start]);

            // 扩展十九：半透明段。绘制顺序已在 `prepare_instances` 里排成
            // 「由远及近」，这里换成不写深度的管线按同一顺序提交，混合才正确。
            pass.set_pipeline(&self.transparent_pipeline);
            let transparent = self.draw_group_range(&mut pass, &groups[transparent_start..]);
            transparent_draws = transparent.0;
            transparent_instances = transparent.1;

            let draw_calls = opaque_draws + transparent_draws;
            let instances = opaque_instances + transparent_instances;

            self.stats = RenderStats {
                draw_calls,
                instances,
                culled,
                cull_unknown,
                shadow_draws,
                shadow_cascades: shadow_plan.levels,
                shadow_casters: shadow_plan.tested,
                shadow_culled: shadow_plan.culled,
                shadow_pruned: shadow_plan.pruned,
                shadow_branch_pruned: shadow_plan.branch_pruned,
                shadow_box_tests: shadow_plan.box_tests,
                caster_tree_builds: self.caster_tree_builds,
                cull: CullStats {
                    tree_builds: self.cull_tree_builds,
                    box_tests: plan.cull_box_tests,
                    instance_tests: plan.cull_instance_tests,
                    pruned: plan.cull_pruned,
                    branch_pruned: plan.cull_branch_pruned,
                },
                frame_index: scene.frame,
                text_glyphs: 0,
                text_draws: 0,
                text_missing: 0,
                ui_quads: 0,
                ui_draws: 0,
                postfx: PostFxStats::default(),
                ssao: SsaoStats::default(),
                local_lights,
                local_shadow_casters,
                local_shadow_layers: local_shadow_layers.len() as u32,
                transparent_draws,
                transparent_instances,
                depth_prepass_draws,
                occlusion: self.occlusion.stats(),
                // 扩展二十三：探针状态在装载时就定了，逐帧原样带出
                env_probe: self.env_probe_stats(),
            };
        }

        // 扩展十三：后处理链在文本 / UI 之前收口。
        // 顺序很关键——composite 是唯一写 surface 的通道，文本与 UI 随后往它上面
        // 用 LoadOp::Load 叠加，这样 HUD 的颜色不会被曝光与色调映射二次调色。
        let postfx_settings = scene.postfx.unwrap_or_default();
        // 扩展十七：遮蔽夹在「主通道画完」与「composite 写 surface」之间。
        // 早一步深度还没写满，晚一步就没有第二个通道能把它乘回颜色了。
        self.ssao.render(
            &self.queue,
            &mut encoder,
            &scene.camera,
            &postfx_settings.ssao,
        );
        self.stats.ssao = self.ssao.stats();
        // 世代号没变就不会重建绑定组，可以放心每帧调
        let (ao_key, ao_view) = if self.stats.ssao.enabled {
            (self.ssao.generation(), Some(self.ssao.ao_view()))
        } else {
            (0, None)
        };
        self.postfx.bind_ao(&self.device, ao_key, ao_view);
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            view,
            &postfx_settings,
        );
        self.stats.postfx = self.postfx.stats();

        // M1 文字渲染：主通道画完后叠加屏幕空间文本。
        // 独立 pass（LoadOp::Load）+ 独立管线，不共享深度与相机，关掉即彻底不跑。
        // 场景开关（脚本 set_text_enabled）优先：关掉时只传空切片，通道一次 GPU 命令都不发
        let texts = if scene.text_enabled {
            &scene.texts[..]
        } else {
            &[][..]
        };

        // M1 最小 UI：与文本同构的独立 passes。层次关系只需调换两个通道的提交顺序：
        // 默认 UI 先画、文本后画 = 文本压在 UI 之上（HUD 数字盖在血条上）；
        // 脚本 set_ui_above_text(true) 则反过来。两者都写 LoadOp::Load，顺序即层次。
        let uis = if scene.ui_enabled {
            &scene.ui[..]
        } else {
            &[][..]
        };
        if scene.ui_above_text {
            self.ui.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                uis,
                self.size,
            );
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                texts,
                self.size,
            );
        } else {
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                texts,
                self.size,
            );
            self.ui.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                uis,
                self.size,
            );
        }
        // 统计在真正提交后读取，避免拿到上一帧的数值
        let text_stats = self.text.stats();
        self.stats.text_glyphs = text_stats.glyphs;
        self.stats.text_draws = text_stats.draw_calls;
        self.stats.text_missing = text_stats.missing;
        let ui_stats = self.ui.stats();
        self.stats.ui_quads = ui_stats.quads;
        self.stats.ui_draws = ui_stats.draw_calls;

        self.queue.submit(Some(encoder.finish()));
        // 扩展二十二：命令提交后才挂 map_async —— 此时 copy 已入队，
        // 回调在下一次 `poll` 里被触发，本帧绝不等待 GPU。
        if depth_readback {
            self.occlusion.after_submit();
        }
    }

    /// 离屏渲染一帧并把像素读回 CPU（P0-4 截图）。
    ///
    /// 走的是 [`Self::render_to_view`]，因此画面与窗口里看到的同源：阴影 / 遮蔽 /
    /// 后处理 / 文本 / UI 全部在内。区别只有两点：目标是普通纹理（不碰 swapchain，
    /// 不依赖窗口是否可见），以及读回后 alpha 一律写成 255（surface 的 alpha 模式
    /// 可能是 Opaque，原样落盘会得到一张全透明 PNG）。
    ///
    /// 本方法会推进一帧：`frame_index`、阴影 pass、后处理累积历史都会前进一格。
    pub fn capture_frame(&mut self, scene: &Scene) -> Result<CapturedFrame, String> {
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return Err(format!("绘制尺寸异常（{width}x{height}），无法截图"));
        }
        let format = self.config.format;
        // 复制粒度决定了读回的排布，非 8bit RGBA/BGRA 的 surface 这里直接拒绝，
        // 而不是给出一张被解读错位的花屏图。
        let swizzle_bgra = match format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            other => {
                return Err(format!(
                    "surface 格式 {other:?} 不是 8bit RGBA/BGRA，暂不支持截图"
                ))
            }
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r3e-capture-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(scene, &view);

        // copy_texture_to_buffer 要求每行按 256 字节对齐：行尾填充必须自己丢掉，
        // 否则图像会沿对角线逐行倾斜。
        let unpadded_row = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-capture-readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("r3e-capture-blit"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| format!("等待 GPU 读回失败: {err:?}"))?;
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(format!("映射截图缓冲失败: {err:?}")),
            Err(err) => return Err(format!("截图回调未送达: {err}")),
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|err| format!("读取截图缓冲失败: {err:?}"))?;
        let mut rgba8 = Vec::with_capacity((unpadded_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_row) as usize;
            let line = &mapped[start..start + unpadded_row as usize];
            for px in line.chunks_exact(4) {
                if swizzle_bgra {
                    rgba8.extend_from_slice(&[px[2], px[1], px[0], 255]);
                } else {
                    rgba8.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        drop(mapped);
        buffer.unmap();

        Ok(CapturedFrame {
            width,
            height,
            rgba8,
            format,
        })
    }

    /// 阶段 D：把阴影贴图尺寸对齐场景配置；返回本帧是否要跑阴影通道。
    /// 扩展十五：把场景里的局部光源打包上传，返回本帧生效的灯数。
    ///
    /// 每帧都传（而不是像球谐那样缓存）：灯可以被脚本逐帧移动，判等成本反而更高。
    /// 总开关关闭时计数写 0，GPU 侧整个循环跳过——数据仍留在场景里，打开即恢复。
    fn prepare_local_lights(&mut self, scene: &Scene) -> u32 {
        let uniform = LightsUniform::from_lights(&scene.lights, scene.lights_enabled);
        let count = uniform.count[0] as u32;
        self.queue
            .write_buffer(&self.lights_buffer, 0, bytemuck::bytes_of(&uniform));
        count
    }

    /// 把一批分组画进一个已经绑好相机 / 关节 / 阴影的通道。
    ///
    /// 返回 `(draw call 数, 实例数)`：`draw call` 按材质分区计数，
    /// 实例数按区间长度累加，两者都不含被剔掉的网格（`mesh_cache` 里没有的）。
    /// 不透明段与半透明段共用这段逻辑，区别只在调用前 `set_pipeline` 的是哪条管线。
    fn draw_group_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        groups: &[DrawGroup],
    ) -> (u32, u32) {
        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        let mut draw_calls = 0u32;
        let mut instances = 0u32;
        for (key, range, surface) in groups {
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

            // 每个材质分区一次 draw：bind group 1 换成该分区材质（含物件级 PBR 覆盖）。
            for part in &gpu.parts {
                let material_key = part.material.with_surface(*surface);
                let Some(material) = self
                    .material_cache
                    .get(material_key)
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
        (draw_calls, instances)
    }

    /// 扩展十九：深度预通道。只写单采样 `depth_view`、无颜色目标。
    ///
    /// 存在的理由是 MSAA：主通道的深度挂在 4× 附件上，多重采样纹理不能直接
    /// 采样，而遮蔽通道要的恰恰是一张可采样的单采样深度图。这里只画不透明段
    /// ——半透明物件不写深度，画了也不会留下有效数据。
    ///
    /// 没有不透明批次时整个通道跳过（返回 0），不提交空 pass。
    fn draw_depth_prepass(&self, encoder: &mut wgpu::CommandEncoder, groups: &[DrawGroup]) -> u32 {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("r3e-depth-prepass"),
            color_attachments: &[],
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

        pass.set_pipeline(&self.depth_prepass_pipeline);
        // 一帧里没有可画的不透明批次时仍保留这次 pass：它的作用是清空
        // 单采样深度，否则遮蔽通道会拿上一帧的深度继续算
        if groups.is_empty() {
            return 0;
        }
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.joint_bind_group, &[]);
        // 顶点阶段不采阴影贴图，但管线布局里有这一组，必须绑上才合法
        pass.set_bind_group(3, &self.shadow_bind_group, &[]);
        // 无颜色目标：占位绑定默认材质，避免依赖「驱动是否容忍空 group1」
        if let Some(default) = self.material_cache.get(MaterialKey::Default) {
            pass.set_bind_group(1, &default.bind_group, &[]);
        }

        self.draw_group_range(&mut pass, groups).0
    }

    // 参数虽多，但每个都来自调用方现成的字段；包进结构体只会让调用点更绕。
    #[allow(clippy::too_many_arguments)]
    fn prepare_shadow(
        &mut self,
        scene: &Scene,
        groups: &[DrawGroup],
        bounds: Option<Aabb>,
        light: Light,
        camera: &r3e_core::Camera,
        caster_bounds: &[Aabb],
        caster_nodes: &[r3e_core::NodeId],
    ) -> ShadowPlan {
        let shadow = light.shadow.sanitized();

        // 尺寸变化就重建纹理 + bind group（贴图句柄必须换新，不能原地 resize）
        if shadow.size != self.shadow_size {
            self.resize_shadow(shadow.size);
        }

        let levels = shadow.cascade.levels();
        // 用户没指定 max_distance 时，用「场景包围球最远点」当上限：几何之外的视距
        // 本来就没有可投影的东西，把级联摊到那里纯属浪费纹素。
        let mut cascade = shadow.cascade;
        if cascade.max_distance == 0.0 {
            if let Some(b) = bounds.filter(|b| !b.is_empty()) {
                let radius = (b.size().length() * 0.5).max(0.25);
                let reach = (b.center() - camera.position).length() + radius * 1.05;
                cascade.max_distance =
                    reach.clamp(camera.znear * 2.0, camera.zfar.max(camera.znear * 2.0));
            }
        }
        // 切分点长度 = 级数 + 1，首尾钉在 znear / 「max_distance 或 zfar」上
        let splits = r3e_core::cascade_splits(camera, cascade);
        let cam_vp = camera.view_projection();

        let mut matrices = [[[0.0f32; 4]; 4]; MAX_CASCADES];
        let mut split_ndc = [1.0f32; 4];
        let mut texel_world = 1e-5;
        // 按下标写回定长数组，用索引循环最直观；clippy 的迭代器建议在这里反而更难读。
        #[allow(clippy::needless_range_loop)]
        for level in 0..MAX_CASCADES {
            // 未使用的级复制最后一级矩阵：shader 若因浮点误差选到它也不会采到垃圾
            let i = level.min(levels - 1);
            let (near, far) = (splits[i], splits[i + 1]);
            let corners = r3e_core::frustum_slice_corners(camera, near, far);
            // 扩展二十四：把级联的近端朝光源方向推 `caster_extend`，
            // 让「站在级边界外侧、却把影子投进本级」的遮挡体不被裁掉。
            // extend = 0 时与 `fit_cascade` 逐位相同（有单测钉住）。
            let fit = r3e_core::fit_cascade_extended(
                light.direction,
                &corners,
                shadow.size,
                cascade.caster_extend,
            );

            let mut view_proj = fit.view_proj;
            let mut radius = fit.radius;
            // ortho_size > 0：作为单级覆盖半宽的上限（等价于用户手动收紧覆盖范围）
            if shadow.ortho_size > 0.0 && fit.radius > shadow.ortho_size {
                let scale = shadow.ortho_size / fit.radius;
                view_proj = Mat4::from_scale(Vec3::new(scale, scale, 1.0)) * view_proj;
                radius = shadow.ortho_size;
            }
            matrices[level] = view_proj.to_cols_array_2d();
            if level == 0 {
                texel_world = (2.0 * radius / shadow.size as f32).max(1e-5);
            }
            if level < levels {
                // 该级远端在相机 NDC 里的 z：远平面四角中心的投影深度
                let far_center = (corners[4] + corners[5] + corners[6] + corners[7]) * 0.25;
                let clip = cam_vp * far_center.extend(1.0);
                split_ndc[level] = if clip.w.abs() > 1e-6 {
                    (clip.z / clip.w).clamp(0.0, 1.0)
                } else {
                    1.0
                };
            }
        }

        let uniform = ShadowUniform {
            light_view_proj: matrices,
            split_ndc,
            params: [
                1.0 / shadow.size as f32,
                shadow.bias,
                if shadow.enabled { 1.0 } else { 0.0 },
                shadow.normal_bias * texel_world,
            ],
            params2: [shadow.strength, levels as f32, shadow.cascade.blend, 0.0],
        };
        self.queue
            .write_buffer(&self.shadow_uniform, 0, bytemuck::bytes_of(&uniform));

        // 深度 pass 只认矩阵（不碰会被主通道采样的 ShadowUniform），且每级一份
        for (level, matrix) in matrices.iter().enumerate().take(MAX_CASCADES) {
            let pass_uniform = ShadowPassUniform {
                light_view_proj: *matrix,
                pad: [0.0; 4],
            };
            self.queue.write_buffer(
                &self.shadow_pass_uniforms[level],
                0,
                bytemuck::bytes_of(&pass_uniform),
            );
        }

        // 扩展二十四：按级决定「这一级要提交哪些遮挡体」。
        // 关闭裁剪 / 阴影关闭 / 没有遮挡体时 `masks` 为空，调用方据此走全量提交，
        // 与扩展二十三的开销与画面逐字相同。
        let mut plan = ShadowPlan {
            levels: 0,
            masks: Vec::new(),
            tested: 0,
            culled: 0,
            pruned: 0,
            branch_pruned: 0,
            box_tests: 0,
        };
        if shadow.enabled && !groups.is_empty() {
            plan.levels = levels as u32;
            if cascade.caster_cull && !caster_bounds.is_empty() {
                let level_mats: Vec<Mat4> = (0..levels)
                    .map(|level| Mat4::from_cols_array_2d(&matrices[level]))
                    .collect();
                // 扩展二十五：先按场景树做层级判定——整枝落在级外的子树直接跳过，
                // 只有子树盒与级相交才下探，叶子才逐实例判。树不可用时（场景结构
                // 刚变、实例挂不上节点）退回扩展二十四的逐实例路径，掩码结果相同。
                match self.plan_casters_hierarchical(
                    scene,
                    caster_nodes,
                    caster_bounds,
                    &level_mats,
                ) {
                    Some(hierarchy) => {
                        plan.masks = hierarchy.masks;
                        plan.tested = hierarchy.instance_tests;
                        plan.pruned = hierarchy.pruned;
                        plan.branch_pruned = hierarchy.branch_pruned;
                        plan.box_tests = hierarchy.box_tests;
                    }
                    None => {
                        plan.masks = r3e_core::plan_casters(&level_mats, caster_bounds);
                        plan.tested = (levels as u32).saturating_mul(caster_bounds.len() as u32);
                    }
                }
                plan.culled = plan
                    .masks
                    .iter()
                    .map(|mask| mask.iter().filter(|keep| !**keep).count() as u32)
                    .sum();
            }
        }
        plan
    }

    /// 扩展二十五：把本帧的遮挡体挂回场景树，做层级判定。
    ///
    /// 返回 `None` 表示层级树不可用（场景里一个节点都没有），调用方退回逐实例判定。
    /// 拓扑只在 `Scene::layout_version` 前进时重建，其余帧只刷新节点盒 —— 因此
    /// 稳态下的额外开销就是「把实例盒写回对应节点」这一遍 O(实例) 的搬运。
    fn plan_casters_hierarchical(
        &mut self,
        scene: &Scene,
        caster_nodes: &[r3e_core::NodeId],
        caster_bounds: &[Aabb],
        level_mats: &[Mat4],
    ) -> Option<r3e_core::CasterHierarchyPlan> {
        self.sync_caster_layout(scene);
        let len = self.caster_tree.as_ref()?.len();
        if self.caster_node_bounds.len() != len {
            self.caster_node_bounds.resize(len, Aabb::EMPTY);
            self.caster_node_casts.resize(len, 0);
        }
        for bounds in self.caster_node_bounds.iter_mut() {
            *bounds = Aabb::EMPTY;
        }
        for casts in self.caster_node_casts.iter_mut() {
            *casts = 0;
        }
        self.caster_instance_nodes.clear();
        self.caster_instance_nodes.reserve(caster_nodes.len());
        for (id, bounds) in caster_nodes.iter().zip(caster_bounds.iter()) {
            match self.caster_slots.get(id) {
                Some(&slot) => {
                    self.caster_node_bounds[slot] = *bounds;
                    self.caster_node_casts[slot] = 1;
                    self.caster_instance_nodes.push(slot);
                }
                // 映射不到的实例填越界下标：`expand_masks` 会 fail-open 成全保留
                None => self.caster_instance_nodes.push(usize::MAX),
            }
        }
        let tree = self.caster_tree.as_mut()?;
        tree.refresh(&self.caster_node_bounds, &self.caster_node_casts);
        let hierarchy = tree.plan(level_mats);
        let masks = r3e_core::expand_masks(&hierarchy.masks, &self.caster_instance_nodes);
        Some(r3e_core::CasterHierarchyPlan {
            masks,
            box_tests: hierarchy.box_tests,
            instance_tests: hierarchy.instance_tests,
            pruned: hierarchy.pruned,
            branch_pruned: hierarchy.branch_pruned,
        })
    }

    /// 把场景树压成「扁平节点表 + id → 下标表」，供层级剔除建树用。
    ///
    /// `Scene` 按创建序迭代，父节点通常先于子节点入表，但兄弟间顺序不保证，
    /// 所以分两趟：先占位拿全 id，第二趟才回填父指针。
    fn layout_slots(scene: &Scene) -> (Vec<CasterNode>, HashMap<r3e_core::NodeId, usize>) {
        let mut nodes: Vec<CasterNode> = Vec::with_capacity(scene.len());
        let mut slots: HashMap<r3e_core::NodeId, usize> = HashMap::with_capacity(scene.len());
        for node in scene.iter() {
            slots.insert(node.id, nodes.len());
            nodes.push(CasterNode {
                parent: None,
                bounds: Aabb::EMPTY,
                casts: 0,
            });
        }
        for (index, node) in scene.iter().enumerate() {
            nodes[index].parent = node.parent.and_then(|parent| slots.get(&parent).copied());
        }
        (nodes, slots)
    }

    /// 场景结构变了才重建层级树（`caster_tree` 与 `caster_slots` 同寿命）。
    fn sync_caster_layout(&mut self, scene: &Scene) {
        if self.caster_tree.is_some() && self.caster_layout == scene.layout_version() {
            return;
        }
        let (nodes, slots) = Self::layout_slots(scene);
        self.caster_node_bounds = vec![Aabb::EMPTY; nodes.len()];
        self.caster_node_casts = vec![0; nodes.len()];
        self.caster_tree = Some(CasterTree::new(nodes));
        self.caster_slots = slots;
        self.caster_layout = scene.layout_version();
        self.caster_tree_builds = self.caster_tree_builds.saturating_add(1);
    }

    /// 场景结构变了才重建主通道视锥树（扩展二十九，与 `sync_caster_layout` 同套路）。
    fn sync_cull_layout(&mut self, scene: &Scene) {
        if self.cull_tree.is_some() && self.cull_layout == scene.layout_version() {
            return;
        }
        let (nodes, slots) = Self::layout_slots(scene);
        self.cull_node_bounds = vec![Aabb::EMPTY; nodes.len()];
        self.cull_node_casts = vec![0; nodes.len()];
        self.cull_tree = Some(CasterTree::new(nodes));
        self.cull_slots = slots;
        self.cull_layout = scene.layout_version();
        self.cull_tree_builds = self.cull_tree_builds.saturating_add(1);
    }

    /// 重建阴影深度图数组与对应的 bind group（改 `shadow.size` 时触发）。
    ///
    /// 局部光源阴影图不在这里重建：它的尺寸固定（24 层 × 512²），
    /// 跟着方向光的 `shadow.size` 走会让显存按平方增长（4096 时 24 层 ≈ 1.6 GiB）。
    fn resize_shadow(&mut self, size: u32) {
        let (texture, view, layer_views) = create_shadow_target(&self.device, size);
        self.shadow_bind_group = create_shadow_bind_group(
            &self.device,
            &self.shadow_layout,
            ShadowBindInputs {
                view: &view,
                sampler: &self.shadow_sampler,
                uniform: &self.shadow_uniform,
                lights: &self.lights_buffer,
                // 局部阴影贴图与 uniform 都没变，重建时按原样重新绑定
                local_view: &self.local_shadow_view,
                local_uniform: &self.local_shadow_uniform,
            },
        );
        self.shadow_view = view;
        self.shadow_layer_views = layer_views;
        self._shadow_texture = texture;
        self.shadow_size = size;
        log::info!("阴影贴图重建: {size}x{size} × {MAX_CASCADES} 层");
    }

    /// 提交一次阴影深度 pass（方向光级联与局部光源共用同一份管线）。
    ///
    /// 两者的顶点布局、实例缓冲、剔除结果完全一致，差别只有「写哪一层 + 用哪份矩阵」，
    /// 因此抽成一个方法：任何阴影管线调整（斜坡偏移、剔除策略）只需改一处。
    /// 返回本次提交的 draw call 数，供 `RenderStats::shadow_draws` 累计。
    ///
    /// `runs` 由 [`shadow_runs`] 生成：扩展二十四起，方向光的每一级可能只提交
    /// 分组内的一段实例（其余落在本级覆盖范围外，被裁掉）。
    fn draw_shadow_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        pass_bind_group: &wgpu::BindGroup,
        submit: ShadowSubmit<'_>,
    ) -> u32 {
        let ShadowSubmit {
            groups,
            runs,
            stride,
        } = submit;
        let mut draws = 0u32;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view,
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
        // group(3) 这里是「只含该层矩阵」的阴影通道专用绑定组，
        // 不是主通道那张比较采样器 —— 深度图此刻正被本 pass 写入。
        pass.set_bind_group(3, pass_bind_group, &[]);
        for (group_index, range) in runs {
            let Some((key, _, _)) = groups.get(*group_index) else {
                continue;
            };
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
                draws += 1;
            }
        }
        draws
    }

    /// 局部光源阴影图的边长（扩展十八 C）。固定 `DEFAULT_LOCAL_SHADOW_SIZE`，
    /// 不随方向光 `shadow.size` 变化：一盏点光要 6 层，跟着 4096 走会吃掉 1 GiB 以上。
    pub fn local_shadow_size(&self) -> u32 {
        self.local_shadow_size
    }

    /// 局部光源阴影图的总层数（`MAX_LOCAL_SHADOW_LAYERS` = 4 灯 × 6 面）。
    pub fn local_shadow_layer_count(&self) -> usize {
        self.local_shadow_layer_views.len()
    }

    /// 扩展十八 C：打包本帧的局部光源阴影 uniform，返回需要渲染的层号。
    ///
    /// 返回空 vec 表示「本帧没有任何灯投影」：渲染层据此整段跳过深度 pass，
    /// 与 `cast_shadow == false` 的旧场景完全等价（开销为零）。
    fn prepare_local_shadow(&mut self, scene: &Scene) -> (Vec<u32>, u32) {
        let enabled = scene.lights_enabled && !scene.lights.is_empty();
        let uniform = LocalShadowUniform::from_lights(&scene.lights, enabled);
        let layers = uniform.layers();
        let casters = uniform.params[0].max(0.0) as u32;
        self.queue
            .write_buffer(&self.local_shadow_uniform, 0, bytemuck::bytes_of(&uniform));
        // 每层一份矩阵 uniform：深度 pass 只认 group(3) binding(3)，与主通道解耦
        for layer in &layers {
            let matrix = uniform.view_proj[*layer as usize];
            let pass_uniform = ShadowPassUniform {
                light_view_proj: matrix,
                pad: [0.0; 4],
            };
            self.queue.write_buffer(
                &self.local_shadow_pass_uniforms[*layer as usize],
                0,
                bytemuck::bytes_of(&pass_uniform),
            );
        }
        (layers, casters)
    }

    /// 注入资产库（由应用层加载 FBX 后调用）。渲染层本身不认识文件 IO。
    pub fn set_asset_library(&mut self, library: Arc<dyn MeshProvider>) {
        self.mesh_cache.set_library(library);
        // 资产库换了，材质 / 贴图句柄可能失效，整批丢弃重来。
        self.material_cache.materials.clear();
        self.material_cache.textures.clear();
    }

    /// 收齐本帧要用到的材质并建好 bind group（默认材质也要有一份）。
    fn prepare_materials(
        &mut self,
        groups: &[(MeshRef, std::ops::Range<u32>, Option<SurfaceKey>)],
    ) {
        let mut keys: Vec<MaterialKey> = vec![MaterialKey::Default];
        for (key, _, surface) in groups {
            let Some(gpu) = self.mesh_cache.get(*key) else {
                continue;
            };
            for part in &gpu.parts {
                // 网格分区材质 + 物件级覆盖 → 本批次真正要绑的材质键
                let material = part.material.with_surface(*surface);
                if !keys.contains(&material) {
                    keys.push(material);
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
        // 环境探针：场景里写了 `environment` 才启用；球谐只在配置真变了时重投影
        let environment = scene.environment.map(|e| e.sanitized());
        if environment != self.env_cache {
            self.env_cache = environment;
            self.env_sh = match environment {
                Some(env) => Sh9::project(&env),
                None => Sh9::default(),
            };
            self.env_builds += 1;
        }
        let env = environment.unwrap_or_default();
        // 扩展二十三：配了 HDR 探针时，漫反射系数换成探针投影出的辐照度；
        // 否则维持 `environment` 段（或全零）—— 未配探针时这条路径与扩展十二逐字节相同。
        let sh_source = match self.env_probe.as_ref() {
            Some(probe) => probe.irradiance,
            None => self.env_sh,
        };
        let mut sh = [[0.0_f32; 4]; r3e_core::env::SH_COEFFICIENTS];
        for (dst, src) in sh.iter_mut().zip(sh_source.coeffs.iter()) {
            *dst = [src.x, src.y, src.z, 0.0];
        }
        // 扩展二十三：探针开关 + mip 层数 + 强度 + 面边长，着色器据此决定镜面路径
        let probe = self.env_probe.as_ref().map(|p| p.stats).unwrap_or_default();
        let probe_uniform = [
            if probe.active { 1.0 } else { 0.0 },
            probe.mip_levels as f32,
            probe.intensity,
            probe.face_size as f32,
        ];
        let uniform = CameraUniform {
            view_proj: camera.view_projection().to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_dir: [light.direction.x, light.direction.y, light.direction.z, 0.0],
            light_color: [light.color.x, light.color.y, light.color.z, light.intensity],
            ambient: [light.ambient.x, light.ambient.y, light.ambient.z, 1.0],
            env_zenith: [
                env.sky_zenith[0],
                env.sky_zenith[1],
                env.sky_zenith[2],
                env.intensity,
            ],
            // w 位就是「本帧用哪套环境光」的开关：0 = 退回常数 ambient
            env_horizon: [
                env.sky_horizon[0],
                env.sky_horizon[1],
                env.sky_horizon[2],
                if environment.is_some() { 1.0 } else { 0.0 },
            ],
            env_ground: [env.ground[0], env.ground[1], env.ground[2], 0.0],
            env_sun: [
                env.sun_color[0] * env.sun_intensity,
                env.sun_color[1] * env.sun_intensity,
                env.sun_color[2] * env.sun_intensity,
                0.0,
            ],
            env_sun_dir: [env.sun_dir[0], env.sun_dir[1], env.sun_dir[2], 0.0],
            env_sh: sh,
            env_probe: probe_uniform,
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // 分组键：网格 + 物件级 PBR 材质 + 是否半透明。内置网格与资产网格统一处理。
        //
        // 扩展十九：半透明的判定 = 节点颜色 alpha × 网格材质 alpha（两者都可能 < 1）。
        // 半透明键集中在 `transparent_keys`，排到所有不透明键之后 —— 于是
        // `groups` 天然形成「不透明段 + 半透明段」，绘制时只需切换一次管线。
        //
        // 扩展二十七：旧写法是「外层遍历批次键、内层遍历全场景」，同一份场景要按
        // `键数 × 节点数` 反复过滤一遍；键数（网格 × 材质组合）几十上百时，这层过滤
        // 的开销盖过了它省下的东西。现在改成**单次遍历 + 分桶**：每个实例只在自己的
        // 桶里被收一次，视锥/遮挡剔除与关节槽位分配一并跟进这一遍。桶内顺序仍是场景
        // 迭代顺序，键的登记顺序仍是「首次出现」且不透明在前，所以摊平出来的
        // `groups` 区间与旧实现逐个相同（像素基线不动）。
        let mut opaque_keys: Vec<(MeshRef, Option<SurfaceKey>)> = Vec::new();
        let mut transparent_keys: Vec<(MeshRef, Option<SurfaceKey>)> = Vec::new();
        // 桶键带「段」标志：同一个 (网格, 材质) 组合可能同时落在不透明段与半透明段
        // （同一网格的不同物件 alpha 不同），两段各自成桶、各自摊平 —— 与旧实现的
        // 「两段各自全场景过滤一遍」等价，但每个实例只被剔除/构造成一次。
        let mut buckets: HashMap<(MeshRef, Option<SurfaceKey>, bool), Vec<u32>> = HashMap::new();
        // 全部待摊平实例，按场景迭代顺序；桶里只存下标，两段共享同一份实例数据
        let mut staged_all: Vec<StagedInstance> = Vec::new();

        // 一个待摊平实例：实例数据 + 阴影用包围盒 + 归属节点 + 半透明排序深度
        struct StagedInstance {
            instance: InstanceRaw,
            bounds: Aabb,
            node: r3e_core::NodeId,
            depth: f32,
        }

        // 每帧重算关节槽位：同一块 storage buffer 按实例依次排布（P4）
        self.joint_scratch.clear();
        let mut joint_overflow = 0usize;
        // 阶段三 A：先用 6 个视锥平面过一遍节点，被剔除的实例不占实例槽位、也不占关节槽位
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let mut culled = 0u32;
        let mut cull_unknown = 0u32;
        // 扩展二十二：遮挡剔除。判定用的是**上一轮**读回的金字塔（比画面滞后几帧），
        // 开关关闭或金字塔未就绪时 `occlusion_ready == false`，下面这段整体跳过，
        // 实例数与关闭前逐个一致。
        let occlusion_settings = scene.occlusion.sanitized();
        let occlusion_ready = occlusion_settings.enabled && self.occlusion.pyramid().is_some();
        let view_projection = camera.view_projection();
        let (screen_width, screen_height) = self.size;
        // 阶段 D：把「所有可见（含被视锥剔除）物件」的包围盒并起来，
        // 交给阴影通道拟合正交相机 —— 阴影要考虑镜头外的遮挡物，
        // 所以这里统计在剔除判断之前。
        let mut scene_bounds: Option<Aabb> = None;
        // 同一网格常被多次引用，材质 alpha 只查一次
        let mut mesh_alphas: HashMap<MeshRef, f32> = HashMap::new();

        // 第一遍的中间态：判剔除 + 构造实例所需的全部信息。
        // 存下来是为了第二遍不必回查场景（树节点下标也算好了）。
        struct PreparedNode {
            id: r3e_core::NodeId,
            /// 场景树节点下标；`None` = 节点不在树里（不该发生，按保留处理）
            tree_slot: Option<usize>,
            /// 世界包围盒；`None` = 网格没有 AABB（fail-open，永不剔除）
            world: Option<Aabb>,
            key: (MeshRef, Option<SurfaceKey>),
            world_matrix: Mat4,
            color: Vec4,
        }

        // 扩展二十九：视锥剔除从「逐实例判定」升级成「按子树预筛」，复用扩展二十五
        // 在阴影通道验证过的 `CasterTree` 套路。办法是把原来的单遍拆成两遍：
        //   第一遍只收集 —— 算世界包围盒、并进 `scene_bounds`、把盒写回层级树节点。
        //     这一遍**不看视锥**：子树盒要等所有实例的盒都写进去才聚合得出来。
        //   第二遍才判视锥 —— 拿掩码，被整枝剪掉的实例连一次 AABB 测试都不做。
        // 中间态 `prepared` 保持场景迭代顺序，所以桶内顺序、键的首次出现顺序与
        // 单遍实现逐个相同，摊平出来的 `groups` 与像素基线都不动。
        let mut prepared: Vec<PreparedNode> = Vec::with_capacity(scene.len());
        self.sync_cull_layout(scene);
        let tree_len = self.cull_tree.as_ref().map_or(0, CasterTree::len);
        if self.cull_node_bounds.len() != tree_len {
            self.cull_node_bounds.resize(tree_len, Aabb::EMPTY);
            self.cull_node_casts.resize(tree_len, 0);
        }
        for bounds in self.cull_node_bounds.iter_mut() {
            *bounds = Aabb::EMPTY;
        }
        for casts in self.cull_node_casts.iter_mut() {
            *casts = 0;
        }

        for node in scene.iter() {
            if !node.visible {
                continue;
            }
            let key = (node.mesh, node.material.map(SurfaceKey::new));
            let world_bounds = match self.mesh_cache.bounds(node.mesh) {
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
                    Some(world)
                }
                // 拿不到包围盒就不剔除（fail-open），宁可多画也不让物件凭空消失
                None => {
                    cull_unknown += 1;
                    None
                }
            };
            // 写回层级树：缺盒的实例按 fail-open 巨盒登记 —— 它所在的那整枝都不能被
            // 预筛剪掉，与逐实例实现里「拿不到包围盒就不剔」是同一句话。
            //
            // 注：r3e 没有「纯分组节点」这一说（`ObjectDesc.mesh` 缺省就是内置 cube，
            // `MeshRef` 也没有空变体），所以每个节点自身都至少有一个实例、都是 `casts=1`。
            let tree_slot = self.cull_slots.get(&node.id).copied();
            if let Some(tree_slot) = tree_slot {
                self.cull_node_bounds[tree_slot] =
                    world_bounds.unwrap_or_else(r3e_core::fail_open_bounds);
                self.cull_node_casts[tree_slot] = 1;
            }
            prepared.push(PreparedNode {
                id: node.id,
                tree_slot,
                world: world_bounds,
                key,
                world_matrix: node.world,
                color: node.color,
            });
        }

        // 树从他「全场景可见节点」的盒聚合出子树盒，自顶向下筛一遍
        let cull_plan = match self.cull_tree.as_mut() {
            Some(tree) => {
                tree.refresh(&self.cull_node_bounds, &self.cull_node_casts);
                Some(tree.plan_frustum(&frustum))
            }
            None => None,
        };
        let (cull_box_tests, cull_instance_tests, cull_pruned, cull_branch_pruned) =
            match &cull_plan {
                Some(plan) => (
                    plan.counts.box_tests,
                    plan.counts.instance_tests,
                    plan.counts.pruned,
                    plan.counts.branch_pruned,
                ),
                None => (0, 0, 0, 0),
            };

        // 第二遍：判剔除 + 入桶
        for item in &prepared {
            // 层级预筛：拿不到掩码（树没建起来 / 节点不在表里）一律保留，宁可多画
            if let Some(plan) = &cull_plan {
                let keep = item
                    .tree_slot
                    .and_then(|slot| plan.mask.get(slot).copied())
                    .unwrap_or(true);
                if !keep {
                    culled += 1;
                    continue;
                }
            }
            let key = item.key;
            let mesh_alpha = *mesh_alphas
                .entry(key.0)
                .or_insert_with(|| self.mesh_material_alpha(key.0));
            let transparent = transparency::is_transparent(item.color.w * mesh_alpha);
            // 键顺序与旧实现同构：不透明键一段、半透明键一段，各段内按首次出现
            let bucket_keys = if transparent {
                &mut transparent_keys
            } else {
                &mut opaque_keys
            };
            if !bucket_keys.contains(&key) {
                bucket_keys.push(key);
            }
            let mut occluded = false;
            let world_bounds = item.world;
            // 扩展二十二：视锥内还要再问一句「是不是被前面的东西挡满了」。
            // 只剔不透明批次：半透明物件不写深度，金字塔里根本没有它们的贡献，
            // 用不透明深度去剔半透明会让玻璃 / 水面整层凭空消失。
            //
            // 遮挡金字塔是上一帧的读回，得按**逐实例**问才准（同一棵子树里近处可能
            // 挡住远处，整枝跳过的前提不成立），所以这一层不做层级化。
            if occlusion_ready {
                if let Some(world) = world_bounds {
                    if self.occlusion.test(
                        &occlusion_settings,
                        &world,
                        &view_projection,
                        screen_width,
                        screen_height,
                    ) {
                        // 同一实例还会进半透明段（半透明不写深度，金字塔里没有它的
                        // 贡献），所以这里只标记「不透明段不要它」，不整体丢弃
                        occluded = true;
                    }
                }
            }
            // 扩展二十四：阴影裁剪同样 fail-open —— 没有 AABB 的实例
            // 一律当成「可能在任何一级里」，绝不因为缺数据把影子弄丢。
            let caster_box = world_bounds.unwrap_or_else(r3e_core::fail_open_bounds);
            let color: Vec4 = item.color;
            let joint_base = match scene.skin_of(item.id) {
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
            let instance = InstanceRaw {
                model: item.world_matrix.to_cols_array_2d(),
                color: color.to_array(),
                joint_base,
                _pad: [0; 3],
            };
            // 平移列就是实例的世界位置；用它到相机的距离平方当半透明排序键
            let depth =
                transparency::view_depth(item.world_matrix.w_axis.truncate(), camera.position);
            let slot = staged_all.len() as u32;
            staged_all.push(StagedInstance {
                instance,
                bounds: caster_box,
                node: item.id,
                depth,
            });
            // 扩展二十八：实例**只进自己那一段**的桶。段归属由节点自身的透明性决定，
            // 不再"两段各来一份"。批次键是 (网格, 材质)，不含透明度，段内又收"所有匹配
            // 该键的节点"，于是同一个网格若既有不透明物件又有玻璃物件，不透明段会把玻璃
            // 也当不透明画一遍、半透明段会把实心物件当玻璃再画一遍 —— 一份几何，两次绘制。
            if transparent {
                // 半透明物件不写深度，金字塔里没有它的贡献，因此永不被遮挡剔除
                buckets.entry((key.0, key.1, true)).or_default().push(slot);
            } else if !occluded {
                buckets.entry((key.0, key.1, false)).or_default().push(slot);
            }
        }

        // 摊平：不透明批次先入组，半透明批次攒到最后统一排序
        let opaque_key_count = opaque_keys.len();
        let mut keys = opaque_keys;
        keys.extend(transparent_keys);

        let mut all: Vec<InstanceRaw> = Vec::new();
        let mut groups: Vec<DrawGroup> = Vec::new();
        // 扩展二十四：与实例缓冲同序的遮挡体包围盒，供阴影按级裁剪使用
        let mut caster_bounds: Vec<Aabb> = Vec::new();
        // 扩展二十五：与实例缓冲同序的节点 id —— 层级剔除靠它把实例挂回场景树
        let mut caster_nodes: Vec<r3e_core::NodeId> = Vec::new();
        // 半透明批次 (组内最远深度, 网格, 材质, 实例)：等全部实例算完后统一排序
        type TransparentBatch = (f32, MeshRef, Option<SurfaceKey>, Vec<u32>);
        let mut pending_transparent: Vec<TransparentBatch> = Vec::new();
        for (index, entry) in keys.iter().enumerate() {
            let segment_transparent = index >= opaque_key_count;
            let (key, surface) = *entry;
            let Some(order_list) = buckets.remove(&(key, surface, segment_transparent)) else {
                continue;
            };
            if order_list.is_empty() {
                continue;
            }
            if segment_transparent {
                // 组间排序键 = 组内最远实例的深度：一次排序决定「哪个网格先画」，
                // 组内顺序则再按 `back_to_front` 排细
                let farthest = order_list.iter().fold(0.0_f32, |far, &slot| {
                    let depth = staged_all[slot as usize].depth;
                    far.max(if depth.is_nan() { f32::INFINITY } else { depth })
                });
                pending_transparent.push((farthest, key, surface, order_list));
                continue;
            }
            let start = all.len() as u32;
            for slot in order_list {
                let item = &staged_all[slot as usize];
                all.push(item.instance);
                caster_bounds.push(item.bounds);
                caster_nodes.push(item.node);
            }
            if all.len() as u32 > start {
                groups.push((key, start..all.len() as u32, surface));
            }
        }

        // 半透明段起点：不透明批次全部入组之后的位置
        let mut transparent_start = groups.len();
        // 组间：远组先画（`total_cmp` 下降序，同深度保持首次出现顺序）
        pending_transparent.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, key, surface, order_list) in pending_transparent {
            let start = all.len() as u32;
            let depths: Vec<f32> = order_list
                .iter()
                .map(|&slot| staged_all[slot as usize].depth)
                .collect();
            let order = transparency::back_to_front(&depths);
            // 按「由远及近」的下标顺序把实例搬进实例缓冲
            for slot in order {
                let item = &staged_all[order_list[slot as usize] as usize];
                all.push(item.instance);
                caster_bounds.push(item.bounds);
                caster_nodes.push(item.node);
            }
            groups.push((key, start..all.len() as u32, surface));
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
        for (key, _, _) in &groups {
            if self.mesh_cache.ensure(&self.device, &self.queue, *key) {
                uploaded.push(*key);
            }
        }
        // 上传失败的网格从绘制分组中剔除；
        // 组数变化时透明段起点要跟着前移，否则会把不透明批次当成半透明来画。
        if uploaded.len() != groups.len() {
            let mut removed_opaque = 0usize;
            let mut index = 0usize;
            groups.retain(|(key, _, _)| {
                let keep = uploaded.contains(key);
                if !keep && index < transparent_start {
                    removed_opaque += 1;
                }
                index += 1;
                keep
            });
            transparent_start -= removed_opaque;
        }

        if all.is_empty() {
            return InstancePlan {
                groups,
                transparent_start,
                culled,
                cull_unknown,
                scene_bounds,
                caster_bounds,
                caster_nodes,
                cull_box_tests,
                cull_instance_tests,
                cull_pruned,
                cull_branch_pruned,
            };
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

        InstancePlan {
            groups,
            transparent_start,
            culled,
            cull_unknown,
            scene_bounds,
            caster_bounds,
            caster_nodes,
            cull_box_tests,
            cull_instance_tests,
            cull_pruned,
            cull_branch_pruned,
        }
    }

    /// 取网格材质自身的 alpha（无材质 / 无贴图时按 1.0 处理）。
    ///
    /// 半透明判定要同时看两处 alpha：节点颜色带一个，网格 PBR 材质还能带一个。
    /// 只看节点颜色会让「材质设成半透明、颜色保持不透明」的物件排错批次。
    fn mesh_material_alpha(&self, mesh: MeshRef) -> f32 {
        self.mesh_cache
            .material_alpha(mesh)
            .map_or(1.0, |alpha| alpha.clamp(0.0, 1.0))
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
    /// 未挂法线贴图时绑定的中性法线（0.5, 0.5, 1.0）。着色器按开关跳过采样，
    /// 这里只是让 bind group 永远填满 —— 万一开关与绑定不同步，中性法线也不会改外观。
    flat_normal: wgpu::TextureView,
    /// 扩展二十三：HDR 反射探针的立方体视图。没有探针时是 1×1 全黑兜底图；
    /// 换探针时整个材质缓存被清空重建，因此这里存的是「当前这一份」。
    env_cube: wgpu::TextureView,
    /// 兜底黑立方体（卸载探针时用它填回 binding 6）。
    fallback_env_cube: wgpu::TextureView,
    materials: HashMap<MaterialKey, GpuMaterial>,
    textures: HashMap<(AssetId, u32, TextureColorSpace), wgpu::TextureView>,
    max_texture_dim: u32,
}

impl MaterialCache {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
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
                // 扩展二十一：法线 / 粗糙度 / AO 三张数据贴图，复用 binding(2) 的采样器
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 扩展二十三：HDR 反射探针立方体贴图。`Rgba16Float` + 线性过滤，
                // 因此必须是 `Float { filterable: true }`；没有探针时绑 1×1 全黑兜底图，
                // 绑定组永远填满，着色器靠 `env_probe.x` 决定要不要采样。
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
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
        let flat_normal = create_flat_normal_texture(device, queue);
        let env_cube = create_black_cube_texture(device, queue);
        let fallback_env_cube = env_cube.clone();

        let mut cache = Self {
            layout,
            sampler,
            white,
            flat_normal,
            env_cube,
            fallback_env_cube,
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

    /// 扩展二十三：换探针立方体。bind group 里的视图是写死的，所以换图必须
    /// 让所有材质重新建组 —— 探针只在场景加载时换一次，这个代价可以接受。
    fn set_env_cube(&mut self, view: wgpu::TextureView) {
        self.env_cube = view;
        self.materials.clear();
    }

    /// 扩展二十三：卸探针，回到兜底黑立方体。同样清空材质缓存。
    fn reset_env_cube(&mut self) {
        self.env_cube = self.fallback_env_cube.clone();
        self.materials.clear();
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

        let (asset_material, surface) = key.split();
        let data = match (asset_material, library) {
            (Some((asset, index)), Some(library)) => {
                library.material(asset, index).unwrap_or_else(|| {
                    log::warn!("资产 {} 缺少材质 {}，回退默认材质", asset, index);
                    MaterialData::default()
                })
            }
            _ => MaterialData::default(),
        };
        // 物件级 PBR 覆盖只改 metallic / roughness，资产的颜色与贴图保持不变
        let data = match surface {
            Some(surface) => surface.overlay(&data),
            None => data,
        };

        let (view, has_texture) = match (data.base_color_texture, library) {
            (Some((asset, index)), Some(library)) => {
                match self.texture_view(
                    device,
                    queue,
                    asset,
                    index,
                    library,
                    TextureColorSpace::Srgb,
                ) {
                    Some(view) => (view, true),
                    None => (self.white.clone(), false),
                }
            }
            _ => (self.white.clone(), false),
        };

        // 扩展二十一：三张数据贴图各自解析。开关只认「真的绑上了图」的那张 ——
        // 贴图超限/像素不全会让 texture_view 返回 None，此时开关必须是 0，
        // 否则着色器会去采样我们垫的中性图，材质行为就悄悄变了。
        let (normal_view, normal_map) = self.resolve_pbr_map(
            device,
            queue,
            data.normal_texture,
            library,
            TextureColorSpace::Linear,
            self.flat_normal.clone(),
        );
        let (roughness_view, roughness_map) = self.resolve_pbr_map(
            device,
            queue,
            data.roughness_texture,
            library,
            TextureColorSpace::Linear,
            self.white.clone(),
        );
        let (occlusion_view, occlusion_map) = self.resolve_pbr_map(
            device,
            queue,
            data.occlusion_texture,
            library,
            TextureColorSpace::Linear,
            self.white.clone(),
        );
        let maps = PbrMapFlags {
            normal: normal_map,
            roughness: roughness_map,
            occlusion: occlusion_map,
        };

        let uniform = MaterialUniform::build_with_maps(
            data.base_color,
            data.metallic,
            data.roughness,
            surface.map(|s| s.emissive()).unwrap_or([0.0; 3]),
            has_texture,
            maps,
        );
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&roughness_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&occlusion_view),
                },
                // 扩展二十三：探针立方体。没有探针时是兜底黑图，不影响画面。
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.env_cube),
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
        space: TextureColorSpace,
    ) -> Option<wgpu::TextureView> {
        if let Some(view) = self.textures.get(&(asset, index, space)) {
            return Some(view.clone());
        }
        let image = library.texture(asset, index)?;
        let view = upload_texture(device, queue, &image, self.max_texture_dim, space)?;
        log::info!(
            "贴图已上传 GPU: asset {} tex {} — {}x{} ({:?})",
            asset,
            index,
            image.width,
            image.height,
            space
        );
        self.textures.insert((asset, index, space), view.clone());
        Some(view)
    }

    /// 解析一张 PBR 数据贴图：成功返回 (视图, true)，否则返回 (兜底视图, false)。
    ///
    /// 兜底视图的作用只是把 bind group 填满 —— 开关为 false 时着色器根本不采样，
    /// 所以「没图」不等于「画面变了」。
    fn resolve_pbr_map(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: Option<(AssetId, u32)>,
        library: Option<&Arc<dyn MeshProvider>>,
        space: TextureColorSpace,
        placeholder: wgpu::TextureView,
    ) -> (wgpu::TextureView, bool) {
        let (Some((asset, index)), Some(library)) = (slot, library) else {
            return (placeholder, false);
        };
        match self.texture_view(device, queue, asset, index, library, space) {
            Some(view) => (view, true),
            None => (placeholder, false),
        }
    }
}

/// 创建 1x1 中性法线贴图：(0.5, 0.5, 1.0) 对应切线空间里的「正上方」。
/// 用线性格式上传，避免 0.5 被 sRGB 解码成 0.21 而把法线整体压弯。
fn create_flat_normal_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-flat-normal-1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
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
        &[128u8, 128, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
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

/// 扩展二十三：1×1 全黑立方体贴图，没有探针时占住 binding 6。
///
/// 用 `Rgba16Float` 与真探针同格式，避免切场景时 bind group 的采样类型变化；
/// 六个面都填 0，配合 `env_probe.x = 0` 双保险 —— 即使开关误开也只会得到黑色镜面。
fn create_black_cube_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 6,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-env-cube-fallback"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6u32 {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8; 8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("r3e-env-cube-fallback-view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// 贴图的色彩空间（扩展二十一）。
///
/// 决定上传成哪种 GPU 格式：颜色类贴图走 `Rgba8UnormSrgb`（采样时自动线性化），
/// 数据类贴图（法线 / 粗糙度 / AO）走 `Rgba8Unorm`（原值直读）。
/// 缓存键带上它，同一张图被当颜色和当数据各用一次时不会串。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TextureColorSpace {
    Srgb,
    Linear,
}

/// 把 RGBA8 像素上传成贴图。`space` 决定是否做 sRGB 自动线性化。
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &TextureImage,
    max_dim: u32,
    space: TextureColorSpace,
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
    let format = match space {
        TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-asset-texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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

/// 阶段 D / 扩展十六：创建正方形阴影深度**数组**（`MAX_CASCADES` 层，可作渲染目标，
/// 也可被比较采样）。
///
/// 返回「整张数组的视图」与「每层各自的视图」：主通道要 D2Array 视图才能按级取层，
/// 深度 pass 要 D2 单层视图才能把某一层当渲染目标。
fn create_shadow_target(
    device: &wgpu::Device,
    size: u32,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    create_shadow_array(
        device,
        size,
        MAX_CASCADES as u32,
        "r3e-shadow-map",
        "r3e-shadow-array-view",
        "r3e-shadow-layer-view",
    )
}

/// 扩展十八 C：创建局部光源阴影深度数组（`MAX_LOCAL_SHADOW_LAYERS` 层）。
///
/// 与方向光那张图结构相同，但层数由「灯数 × 面数」决定：点光 6 面、聚光 1 面，
/// 每盏灯固定占 6 层的槽位（聚光的其余 5 层留空），这样层号可以纯算术得到。
fn create_local_shadow_target(
    device: &wgpu::Device,
    size: u32,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    create_shadow_array(
        device,
        size,
        r3e_core::MAX_LOCAL_SHADOW_LAYERS as u32,
        "r3e-local-shadow-map",
        "r3e-local-shadow-array-view",
        "r3e-local-shadow-layer-view",
    )
}

/// 创建可写可采样的分层深度数组（方向光级联与局部阴影共用）。
fn create_shadow_array(
    device: &wgpu::Device,
    size: u32,
    layers: u32,
    label: &str,
    array_label: &str,
    layer_label: &str,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    let size = size.max(1);
    let layers = layers.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(array_label),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let layer_views = (0..layers)
        .map(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(layer_label),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    (texture, array_view, layer_views)
}

/// group(3) 的全部资源（方向光阴影 + 局部光源表 + 局部光源阴影）。
///
/// 抽成结构体是因为这张绑定组要在两处创建（启动、`resize_shadow`），
/// 参数列表一长就容易在某一处漏绑某个 binding —— 漏一次就是整帧画面的静默错误。
struct ShadowBindInputs<'a> {
    view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
    uniform: &'a wgpu::Buffer,
    lights: &'a wgpu::Buffer,
    local_view: &'a wgpu::TextureView,
    local_uniform: &'a wgpu::Buffer,
}

fn create_shadow_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    inputs: ShadowBindInputs<'_>,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("r3e-shadow-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(inputs.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(inputs.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: inputs.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: inputs.lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(inputs.local_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(inputs.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: inputs.local_uniform.as_entire_binding(),
            },
        ],
    })
}

/// 扩展十九：创建 MSAA 附件组（4× 颜色 + 4× 深度）。
///
/// 返回 `(颜色纹理, 颜色视图, 深度纹理, 深度视图)`。颜色纹理由硬件 resolve 到
/// HDR 目标，因此只要 `RENDER_ATTACHMENT`；深度则完全不参与采样 —— 遮蔽通道要的
/// 单采样深度由深度预通道写进 `create_depth_view` 那张纹理，两者互不干扰。
///
/// 纹理句柄必须随视图一起返回：wgpu 的视图不持有纹理所有权，提前 drop 会让
/// 视图引用悬空，真机上表现为每帧校验失败。
fn create_msaa_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-msaa-color"),
        size,
        mip_level_count: 1,
        sample_count: MSAA_SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: crate::postfx::HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-msaa-depth"),
        size,
        mip_level_count: 1,
        sample_count: MSAA_SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth, depth_view)
}

/// 建单采样深度目标，返回「纹理 + 视图」。
///
/// 两个都要：pass 用视图，遮挡剔除（扩展二十二）的整帧 readback 用纹理本体
/// （`copy_texture_to_buffer` 只能针对纹理，不能针对视图）。
fn create_depth_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
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
        // 主通道写这个深度附件，SSAO（扩展十七）还要把它当纹理采样：
        // 少了 TEXTURE_BINDING，遮蔽通道的绑定组会在真机上直接校验失败。
        // 扩展二十二再加 COPY_SRC：整张深度要能被 copy 进回读缓冲。
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}
