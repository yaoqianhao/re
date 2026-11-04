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
    Aabb, AssetId, EnvironmentSettings, Frustum, Light, LocalLight, MeshRef, ProceduralProvider,
    Scene, Sh9, MAX_CASCADES,
};
use winit::window::Window;

use crate::mesh::{vertex_layout, InstanceRaw};
use crate::pbr::{MaterialUniform, SurfaceKey};
use crate::postfx::{PostFxPass, PostFxStats};
use crate::ssao::{SsaoPass, SsaoStats};
use crate::text::{TextPass, TextStats};
use crate::ui::{UiImageProvider, UiPass, UiStats};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const INITIAL_INSTANCE_CAPACITY: u64 = 512;
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
    env_sh: [[f32; 4]; r3e_core::env::SH_COEFFICIENTS],
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
    /// 扩展十六起每级各跑一遍 pass，因此开启 N 级时约为实例批次数 × N。
    pub shadow_draws: u32,
    /// 本帧生效的阴影级联数（扩展十六）。1 = 单级（等价阶段 D），0 = 关闭。
    pub shadow_cascades: u32,
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
    stats: RenderStats,
}

/// `prepare_instances` 的返回值：绘制分组 + 本帧剔除统计。
///
/// 分组键是 `(网格, 物件级 PBR 材质)`：同一网格的物件若 metallic / roughness
/// 不同，会被拆成多个批次，各自绑定自己的材质 uniform。
type InstancePlan = (
    Vec<(MeshRef, std::ops::Range<u32>, Option<SurfaceKey>)>,
    u32,
    u32,
    Option<Aabb>,
);

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
            multisample: wgpu::MultisampleState::default(),
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

        let text = TextPass::new(&device, config.format);
        let ui = UiPass::new(&device, config.format);
        // 后处理链与窗口同级：主通道从此写离屏 HDR 目标，不再直接写 surface
        let postfx = PostFxPass::new(&device, config.format, (width, height));
        // 遮蔽吃主通道深度、产出与 HDR 同尺寸的 AO 图，所以与窗口同级创建
        let ssao = SsaoPass::new(&device, (width, height), &depth_view);

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
            stats: RenderStats::default(),
        })
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
        self.depth_view = create_depth_view(&self.device, width, height);
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
        let (groups, culled, cull_unknown, bounds) = self.prepare_instances(scene);
        self.prepare_materials(&groups);
        // 阶段 D：先把本帧的阴影贴图渲好，主通道再采样它
        let shadow_levels = self.prepare_shadow(&groups, bounds, scene.light, &scene.camera);
        // 扩展十五：局部光源表每帧重传（灯可以被脚本逐帧移动，缓存反而要判等）
        let local_lights = self.prepare_local_lights(scene);
        // 扩展十八 C：本帧要跑深度 pass 的局部阴影层（空 = 没有灯投影）
        let (local_shadow_layers, local_shadow_casters) = self.prepare_local_shadow(scene);

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

        // 阶段 D / 扩展十六：方向光阴影。每级一次 pass，只写进对应层的深度图，
        // 不触碰颜色附件；级数 = 1 时与阶段 D 的单一全场景阴影图完全等价。
        for level in 0..shadow_levels as usize {
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-shadow-pass",
                &self.shadow_layer_views[level],
                &self.shadow_pass_bind_groups[level],
                &groups,
                stride,
            );
        }

        // 扩展十八 C：局部光源阴影。层号由 `plan_local_shadows` 的固定槽位决定
        // （点光 6 面、聚光 1 面），每层一次 pass、一次矩阵 uniform。
        // 没有任何灯投影时 `local_shadow_layers` 为空，这一段整体跳过。
        for layer in &local_shadow_layers {
            let idx = *layer as usize;
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-local-shadow-pass",
                &self.local_shadow_layer_views[idx],
                &self.local_shadow_pass_bind_groups[idx],
                &groups,
                stride,
            );
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-main-pass"),
                // 扩展十三：主通道写离屏 HDR 目标（Rgba16Float，线性），
                // 高亮不再被 sRGB 8 位目标直接切平；贴到屏幕由 composite 完成。
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.postfx.hdr_view(),
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

            for (key, range, surface) in &groups {
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

            self.stats = RenderStats {
                draw_calls,
                instances,
                culled,
                cull_unknown,
                shadow_draws,
                shadow_cascades: shadow_levels,
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
            &view,
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
                &view,
                uis,
                self.size,
            );
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                texts,
                self.size,
            );
        } else {
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                texts,
                self.size,
            );
            self.ui.render(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
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
        self.queue.present(frame);
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

    fn prepare_shadow(
        &mut self,
        groups: &[(MeshRef, std::ops::Range<u32>, Option<SurfaceKey>)],
        bounds: Option<Aabb>,
        light: Light,
        camera: &r3e_core::Camera,
    ) -> u32 {
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
            let fit = r3e_core::fit_cascade(light.direction, &corners, shadow.size);

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

        if shadow.enabled && !groups.is_empty() {
            levels as u32
        } else {
            0
        }
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
    fn draw_shadow_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        pass_bind_group: &wgpu::BindGroup,
        groups: &[(MeshRef, std::ops::Range<u32>, Option<SurfaceKey>)],
        stride: u64,
    ) -> u32 {
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
        for (key, range, _) in groups {
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
        let mut sh = [[0.0_f32; 4]; r3e_core::env::SH_COEFFICIENTS];
        for (dst, src) in sh.iter_mut().zip(self.env_sh.coeffs.iter()) {
            *dst = [src.x, src.y, src.z, 0.0];
        }
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
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // 分组键：网格 + 物件级 PBR 材质。内置网格与资产网格统一处理，按首次出现顺序稳定排列
        let mut keys: Vec<(MeshRef, Option<SurfaceKey>)> = Vec::new();
        for node in scene.iter() {
            if !node.visible {
                continue;
            }
            let group_key = (node.mesh, node.material.map(SurfaceKey::new));
            if !keys.contains(&group_key) {
                keys.push(group_key);
            }
        }

        let mut all: Vec<InstanceRaw> = Vec::new();
        let mut groups: Vec<(MeshRef, std::ops::Range<u32>, Option<SurfaceKey>)> = Vec::new();
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

        for (key, surface) in &keys {
            let start = all.len() as u32;
            for node in scene.iter() {
                if !node.visible || node.mesh != *key {
                    continue;
                }
                // 同一网格不同 PBR 材质的物件分属不同批次
                if node.material.map(SurfaceKey::new) != *surface {
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
                groups.push((*key, start..all.len() as u32, *surface));
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
        for (key, _, _) in &groups {
            if self.mesh_cache.ensure(&self.device, &self.queue, *key) {
                uploaded.push(*key);
            }
        }
        // 上传失败的网格从绘制分组中剔除
        if uploaded.len() != groups.len() {
            groups.retain(|(key, _, _)| uploaded.contains(key));
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
                match self.texture_view(device, queue, asset, index, library) {
                    Some(view) => (view, true),
                    None => (self.white.clone(), false),
                }
            }
            _ => (self.white.clone(), false),
        };

        let uniform = MaterialUniform::build(
            data.base_color,
            data.metallic,
            data.roughness,
            surface.map(|s| s.emissive()).unwrap_or([0.0; 3]),
            has_texture,
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
        // 主通道写这个深度附件，SSAO（扩展十七）还要把它当纹理采样：
        // 少了 TEXTURE_BINDING，遮蔽通道的绑定组会在真机上直接校验失败。
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
