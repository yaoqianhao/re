use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use r3e_core::{AssetId, EnvironmentSettings, LocalLight, MAX_CASCADES};

use crate::pbr::SurfaceKey;

/// 世界矩阵三条轴的缩放模长最大值。
///
/// 形变外扩（扩展三十六）按它把「局部空间的位移」换算到世界空间。非等比缩放取最大值：
/// 宁可盒算大一点多画一个物件，也不能因为某条轴缩放小就把它剔掉。
pub(crate) fn max_axis_scale(world: &glam::Mat4) -> f32 {
    let x = world.x_axis.truncate().length();
    let y = world.y_axis.truncate().length();
    let z = world.z_axis.truncate().length();
    x.max(y).max(z)
}

/// 材质键：默认材质，或「某资产内第 N 个材质」，可再叠加「物件级 PBR 覆盖」。
///
/// 渲染层只认这个键，具体材质参数通过 `MeshProvider::material` 取，
/// 因此这里依旧不依赖任何模型文件格式。物件级覆盖（场景 / 脚本写入的
/// metallic / roughness / emissive）用 [`SurfaceKey`] 参与键值，保证同一网格的
/// 不同 PBR 参数各自成批、互不串味。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MaterialKey {
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
    pub(crate) fn with_surface(self, surface: Option<SurfaceKey>) -> Self {
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
    pub(crate) fn split(self) -> (Option<(AssetId, u32)>, Option<SurfaceKey>) {
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
pub(crate) struct CameraUniform {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) camera_pos: [f32; 4],
    pub(crate) light_dir: [f32; 4],
    /// rgb = 光色，a = 强度
    pub(crate) light_color: [f32; 4],
    pub(crate) ambient: [f32; 4],
    /// rgb = 天顶色，a = IBL 总强度
    pub(crate) env_zenith: [f32; 4],
    /// rgb = 地平线色，a = 是否启用 IBL（0/1）
    pub(crate) env_horizon: [f32; 4],
    /// rgb = 地面色，a 保留
    pub(crate) env_ground: [f32; 4],
    /// rgb = 太阳色 × 太阳强度，a 保留
    pub(crate) env_sun: [f32; 4],
    /// xyz = 指向太阳的单位向量，a 保留
    pub(crate) env_sun_dir: [f32; 4],
    /// 9 项球谐：rgb = 系数（已是辐亮度量纲），a 保留。由 `env::Sh9::project` 在 CPU 侧投影。
    /// 扩展二十三：配了 HDR 探针时，这里换成探针自身投影出的辐照度系数（仍是 9 项），
    /// 漫反射分支因此无需改动；未配探针时与扩展十二逐字节相同。
    pub(crate) env_sh: [[f32; 4]; r3e_core::env::SH_COEFFICIENTS],
    /// 扩展二十三：HDR 立方体反射探针。
    /// x = 是否启用（0/1），y = 镜面 mip 层数，z = 探针强度，w = 面边长（诊断用）。
    pub(crate) env_probe: [f32; 4],
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
pub(crate) struct ShadowUniform {
    pub(crate) light_view_proj: [[[f32; 4]; 4]; MAX_CASCADES],
    pub(crate) split_ndc: [f32; 4],
    pub(crate) params: [f32; 4],
    pub(crate) params2: [f32; 4],
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
pub(crate) struct ShadowPassUniform {
    pub(crate) light_view_proj: [[f32; 4]; 4],
    /// 凑满 16 字节对齐（mat4 已是 64B，这里只是显式占位，便于将来扩展）。
    pub(crate) pad: [f32; 4],
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
