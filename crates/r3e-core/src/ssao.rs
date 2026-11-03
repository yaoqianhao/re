//! 屏幕空间环境光遮蔽（扩展十七）的 CPU 侧契约。
//!
//! 扩展十六把直接光分成了四级级联，暗部层次只解决了「被挡住的光」这一半；
//! 另一half——环境光（`env` 的球谐 IBL 与 `light.ambient` 常数项）——
//! 仍然平均地照进每个缝隙，于是墙角、桌底、物件接触处全都发亮，
//! 画面因此「没有体积感」。SSAO 用主通道的深度缓冲反推视空间位置，
//! 在半球核内数一遍遮挡，把环境光的缺失量烘成一张 `[0,1]` 的遮蔽图，
//! 再在后处理链里乘回颜色。
//!
//! 本模块与 [`crate::postfx`] 一样零 wgpu 依赖，GPU 侧
//! （`crates/r3e-render/src/ssao.wgsl`）只把这里的公式搬到 WGSL：
//!
//! * [`SsaoSettings`]：场景 `postfx.ssao` 段，带 [`SsaoSettings::sanitized`] 写入前收敛；
//! * [`hemisphere_kernel`]：确定性的半球采样核（同一 `index` 永远同一方向，
//!   因此画面不抖、单测可断言）；
//! * [`reconstruct_view_pos`]：深度 → 视空间位置的参考实现；
//! * [`is_occluded`]：单次采样的遮挡判定（范围检查 + 法线半球 + 偏置）；
//! * [`ao_from_occlusion`]：遮挡比例 → 遮蔽系数（含强度与下限收敛）。
//!
//! **边界**：本扩展只在 composite 里把遮蔽乘到最终颜色上，而正确的做法是
//! 只乘间接光分量。要做到后者需要把环境项单独渲一张 G-buffer 或做延迟着色，
//! 那是另一个量级的改动，故此处作为已知近似记录在
//! `ARCHITECTURE.md` 的「已知边界」里。

use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// 半球核上限：16 个样本已经能盖住 0.6 单位的半径，再多是纯噪声。
pub const MAX_SSAO_SAMPLES: u8 = 16;

/// 半球核下限：少于 4 个样本的遮蔽图只有条纹，没有可用信息。
pub const MIN_SSAO_SAMPLES: u8 = 4;

/// 缺省样本数：8 是「看得出接触阴影、开销又只有 16 的一半」的折中。
pub const DEFAULT_SSAO_SAMPLES: u8 = 8;

/// 缺省采样半径（视空间世界单位）。1 米级别的物件用 0.6 刚好贴住接缝。
pub const DEFAULT_SSAO_RADIUS: f32 = 0.6;

/// 半径下限：再小就退化成深度值本身的自遮挡噪声。
pub const MIN_SSAO_RADIUS: f32 = 0.01;

/// 半径上限：超过 8 单位的遮蔽在房间尺度上已经等于全屏压暗。
pub const MAX_SSAO_RADIUS: f32 = 8.0;

/// 缺省深度偏置：视空间里 2 厘米的容差，压掉同一表面上的自遮挡（acne）。
pub const DEFAULT_SSAO_BIAS: f32 = 0.02;

/// 偏置下限：0 表示不做偏置（薄表面会立即出现条纹）。
pub const MIN_SSAO_BIAS: f32 = 0.0;

/// 偏置上限：偏置比半径还大就没东西能遮挡了。
pub const MAX_SSAO_BIAS: f32 = 0.5;

/// 缺省强度：1.0 = 完全按采样比例压暗。
pub const DEFAULT_SSAO_INTENSITY: f32 = 1.0;

/// 强度上限：4.0 已经能把缝隙压成纯黑，再高只是画面脏。
pub const MAX_SSAO_INTENSITY: f32 = 4.0;

/// 遮蔽系数下限：再暗也留一点环境光，否则阴影里彻底看不见物件轮廓。
pub const MIN_AO_FACTOR: f32 = 0.0;

/// 半球核的伪随机散列（splitmix32 变体）。
///
/// 用它而不是 `rand` 的原因有两条：核必须**每帧一致**（否则遮蔽图逐帧闪烁），
/// 且必须能在 WGSL 里用同样的整数运算复现。
pub fn hash_u32(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x21F0_AAAD);
    x ^= x >> 15;
    x = x.wrapping_mul(0x735A_2D97);
    x ^= x >> 15;
    x
}

/// `hash_u32` 归一化到 `[0, 1]`。
fn hash_unit(x: u32) -> f32 {
    (hash_u32(x) >> 8) as f32 / ((1u32 << 24) - 1) as f32
}

/// 核内第 `index` 个样本的**方向**：单位半球（`z >= 0`）上的确定性点。
///
/// 用「方位角 + 余弦加权半径」的极坐标构造：`theta = 2π·h1`、`r = sqrt(h2)`，
/// 这样点在半球面上均匀分布，不会像随机取 xyz 那样在中心堆积。
pub fn kernel_dir(index: usize) -> Vec3 {
    let i = index as u32;
    let theta = std::f32::consts::TAU * hash_unit(i.wrapping_mul(2).wrapping_add(1));
    let r = hash_unit(i.wrapping_mul(2).wrapping_add(2)).sqrt();
    let xy = Vec2::new(theta.cos(), theta.sin()) * r;
    Vec3::new(xy.x, xy.y, (1.0 - r * r).max(0.0).sqrt())
}

/// 核内第 `index` 个样本的**长度比例**：靠近原点的样本密、远处的稀。
///
/// `lerp(0.1, 1.0, t)²` 让前几个样本贴着表面（抓接缝），后几个覆盖整个半径
/// （抓大尺度遮蔽）。指数 2 来自常见做法（Crytek / Unity SSAO），
/// 目的是把有限样本数的误差压到接缝附近——那里才是最需要精度的地方。
pub fn kernel_scale(index: usize, count: usize) -> f32 {
    let n = count.max(1);
    let t = (index as f32 + 1.0) / n as f32;
    let lerp = 0.1 + 0.9 * t;
    lerp * lerp
}

/// 确定性半球核：`count` 个长度为 `[0, radius]` 的视空间采样偏移。
///
/// `count` 会被夹进 `[MIN_SSAO_SAMPLES, MAX_SSAO_SAMPLES]`——这是 GPU 侧
/// uniform 数组的容量，越界写会直接把参数块顶坏。
pub fn hemisphere_kernel(count: usize, radius: f32) -> Vec<Vec3> {
    let n = count.clamp(MIN_SSAO_SAMPLES as usize, MAX_SSAO_SAMPLES as usize);
    let radius = if radius.is_nan() {
        DEFAULT_SSAO_RADIUS
    } else {
        radius.clamp(MIN_SSAO_RADIUS, MAX_SSAO_RADIUS)
    };
    (0..n)
        .map(|i| kernel_dir(i) * (kernel_scale(i, n) * radius))
        .collect()
}

/// 深层拷贝进 uniform 的核：定长 `[Vec4; MAX_SSAO_SAMPLES]`，`w` 留空。
///
/// 固定长度是 wgpu 的硬要求（uniform 里的数组长度必须与 WGSL 一致），
/// 未用到的槽位填 0——着色器按 `params.sample_count` 截断，不会读到它们。
pub fn kernel_uniform(count: usize, radius: f32) -> [Vec3; MAX_SSAO_SAMPLES as usize] {
    let kernel = hemisphere_kernel(count, radius);
    let mut out = [Vec3::ZERO; MAX_SSAO_SAMPLES as usize];
    for (dst, src) in out.iter_mut().zip(kernel) {
        *dst = src;
    }
    out
}

/// 深度 → 视空间位置（参考实现，GPU 侧用 `inv_proj` 做同样的变换）。
///
/// `ndc` 是 `[-1, 1]` 的归一化设备坐标（y 向上），`depth` 是 wgpu 的
/// `[0, 1]` 深度值。透视投影下齐次除法不可省：`inv_proj * (x, y, z, 1)`
/// 得到的是 `(x·w, y·w, z·w, w)`，必须除以 `w` 才是位置。
pub fn reconstruct_view_pos(inv_proj: Mat4, ndc: Vec2, depth: f32) -> Vec3 {
    let clip = inv_proj * glam::Vec4::new(ndc.x, ndc.y, depth, 1.0);
    if clip.w.abs() < f32::EPSILON {
        return Vec3::ZERO;
    }
    clip.truncate() / clip.w
}

/// 深度重建出来的法线（由左右 / 上下邻域位置叉乘得到）。
///
/// 退化情况（邻域共线、深度为无穷远）返回 `None`，调用方据此跳过该像素的
/// 遮蔽计算——比返回一个随意方向的法线更安全，否则会在天空区域刷出黑边。
pub fn normal_from_neighbors(center: Vec3, right: Vec3, up: Vec3) -> Option<Vec3> {
    let n = (right - center).cross(up - center);
    let len = n.length();
    if !len.is_finite() || len < 1e-6 {
        return None;
    }
    Some(n / len)
}

/// 单次采样的遮挡判定。
///
/// 三条全部满足才算遮挡：
///
/// 1. 样本位置在**法线半球内**（`dot(sample - pos, normal) > 0`）——背面样本是
///    同一面墙的自身厚度，不能算遮蔽；
/// 2. 样本到中心的距离小于半径——超出半径的样本属于别的物件，隔着房间也会误判；
/// 3. 深度差超过偏置——偏置以内是浮点误差与表面曲率，不是真的凸起。
pub fn is_occluded(
    view_pos: Vec3,
    normal: Vec3,
    sample_view_pos: Vec3,
    radius: f32,
    bias: f32,
) -> bool {
    let delta = sample_view_pos - view_pos;
    let dist = delta.length();
    if !dist.is_finite() || dist <= bias || dist >= radius {
        return false;
    }
    if delta.dot(normal) <= 0.0 {
        return false;
    }
    true
}

/// 遮挡比例 → 遮蔽系数（`[MIN_AO_FACTOR, 1]`）。
///
/// `occluded / total` 是遮挡率，乘强度后从 1 里扣掉。强度 1 时全遮挡 → 0，
/// 强度 4 时少量遮挡就能压到下限；下限本身是 0（阴影里允许纯黑），
/// 但 `MIN_AO_FACTOR` 一常量收口，后续想抬亮阴影只需改一处。
pub fn ao_from_occlusion(occluded: usize, total: usize, intensity: f32) -> f32 {
    if total == 0 {
        return 1.0;
    }
    let ratio = (occluded as f32 / total as f32).clamp(0.0, 1.0);
    let intensity = if intensity.is_nan() {
        DEFAULT_SSAO_INTENSITY
    } else {
        intensity.clamp(0.0, MAX_SSAO_INTENSITY)
    };
    (1.0 - ratio * intensity).clamp(MIN_AO_FACTOR, 1.0)
}

/// 场景 `postfx.ssao` 段。
///
/// 缺省 [`SsaoSettings::default`]（`enabled = false`）与场景里完全不写这一段等价：
/// 老场景不加 `ssao` 时，composite 采样的是一张 1×1 的白色占位纹理，
/// 画面与扩展十六逐像素一致。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SsaoSettings {
    /// 是否启用。缺省关闭——遮蔽是「加分项」，不该在老场景上默认改变观感。
    #[serde(default)]
    pub enabled: bool,
    /// 采样半径（视空间世界单位）。
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// 深度偏置：压掉同一表面上的自遮挡。
    #[serde(default = "default_bias")]
    pub bias: f32,
    /// 强度：遮挡率乘上它再从 1 里扣。
    #[serde(default = "default_intensity")]
    pub intensity: f32,
    /// 半球核样本数（会被夹进 `[4, 16]`）。
    #[serde(default = "default_samples")]
    pub samples: u8,
}

fn default_radius() -> f32 {
    DEFAULT_SSAO_RADIUS
}

fn default_bias() -> f32 {
    DEFAULT_SSAO_BIAS
}

fn default_intensity() -> f32 {
    DEFAULT_SSAO_INTENSITY
}

fn default_samples() -> u8 {
    DEFAULT_SSAO_SAMPLES
}

impl Default for SsaoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: DEFAULT_SSAO_RADIUS,
            bias: DEFAULT_SSAO_BIAS,
            intensity: DEFAULT_SSAO_INTENSITY,
            samples: DEFAULT_SSAO_SAMPLES,
        }
    }
}

impl SsaoSettings {
    /// 收敛到合法区间：NaN → 缺省值（不是 0——0 半径的遮蔽图恒为 1，
    /// 会让人误以为「SSAO 开了但没用」），其余夹进各自区间。
    pub fn sanitized(self) -> Self {
        Self {
            enabled: self.enabled,
            radius: sanitize_range(
                self.radius,
                DEFAULT_SSAO_RADIUS,
                MIN_SSAO_RADIUS,
                MAX_SSAO_RADIUS,
            ),
            bias: sanitize_range(self.bias, DEFAULT_SSAO_BIAS, MIN_SSAO_BIAS, MAX_SSAO_BIAS),
            intensity: sanitize_range(
                self.intensity,
                DEFAULT_SSAO_INTENSITY,
                0.0,
                MAX_SSAO_INTENSITY,
            ),
            samples: self.samples.clamp(MIN_SSAO_SAMPLES, MAX_SSAO_SAMPLES),
        }
    }

    /// 关闭状态：遮蔽 pass 可以整条跳过，composite 乘 1。
    ///
    /// 强度为 0 也算关闭：此时 `ao_factor` 恒为 1，乘上去等于没乘，
    /// 视觉结果与关掉逐像素一致，没必要再白跑两遍全尺寸 pass。
    pub fn is_neutral(&self) -> bool {
        !self.enabled || self.intensity <= 0.0
    }

    /// 实际生效的样本数（已夹进 `[4, 16]`）。
    pub fn effective_samples(&self) -> usize {
        self.samples.clamp(MIN_SSAO_SAMPLES, MAX_SSAO_SAMPLES) as usize
    }
}

/// `NaN` 回落 `fallback`，其余夹进 `[min, max]`（顺手把 `-0.0` 归成 `0.0`）。
fn sanitize_range(v: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if v.is_nan() {
        return fallback;
    }
    let clamped = v.clamp(min, max);
    if clamped == 0.0 {
        0.0
    } else {
        clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_spread() {
        assert_eq!(hash_u32(0), hash_u32(0));
        assert_ne!(hash_u32(0), hash_u32(1));
        // 24 位归一化：结果必须严格落在 [0, 1]
        for i in 0..64 {
            let v = hash_unit(i);
            assert!((0.0..=1.0).contains(&v), "hash_unit({i}) = {v}");
        }
    }

    #[test]
    fn kernel_dirs_are_unit_hemisphere() {
        for i in 0..MAX_SSAO_SAMPLES as usize {
            let d = kernel_dir(i);
            assert!(
                (d.length() - 1.0).abs() < 1e-5,
                "dir {i} 长度 {}",
                d.length()
            );
            assert!(d.z >= -1e-6, "dir {i} 跑到了下半球: {d}");
        }
    }

    #[test]
    fn kernel_lengths_grow_with_index() {
        let mut prev = 0.0;
        for i in 0..8 {
            let s = kernel_scale(i, 8);
            assert!(s > prev, "scale 应单调增: {i} -> {s} vs {prev}");
            assert!((0.0..=1.0).contains(&s));
            prev = s;
        }
    }

    #[test]
    fn kernel_is_deterministic_across_calls() {
        assert_eq!(hemisphere_kernel(8, 0.6), hemisphere_kernel(8, 0.6));
    }

    #[test]
    fn kernel_clamps_count_and_radius() {
        assert_eq!(hemisphere_kernel(1, 1.0).len(), MIN_SSAO_SAMPLES as usize);
        assert_eq!(hemisphere_kernel(999, 1.0).len(), MAX_SSAO_SAMPLES as usize);
        let far = hemisphere_kernel(8, 1e6);
        let max = far.iter().map(|v| v.length()).fold(0.0, f32::max);
        assert!(max <= MAX_SSAO_RADIUS + 1e-4, "半径没被夹住: {max}");
        let nan = hemisphere_kernel(8, f32::NAN);
        assert_eq!(nan, hemisphere_kernel(8, DEFAULT_SSAO_RADIUS));
    }

    #[test]
    fn kernel_uniform_fills_tail_with_zero() {
        let u = kernel_uniform(4, 0.5);
        assert_eq!(u.len(), MAX_SSAO_SAMPLES as usize);
        assert_ne!(u[0], Vec3::ZERO);
        for slot in u.iter().skip(4) {
            assert_eq!(*slot, Vec3::ZERO);
        }
    }

    #[test]
    fn reconstruct_round_trips_projection() {
        let proj = crate::camera::Camera::default().projection();
        let inv = proj.inverse();
        for (x, y, z) in [(-0.5, 0.25, -0.3), (0.0, 0.0, -5.0), (0.8, -0.8, -20.0)] {
            let view = Vec3::new(x, y, z);
            let clip = proj * view.extend(1.0);
            let ndc = Vec2::new(clip.x / clip.w, clip.y / clip.w);
            let depth = clip.z / clip.w;
            let back = reconstruct_view_pos(inv, ndc, depth);
            // f32 逆矩阵的精度随深度衰减，故用相对容差（z=-20 时绝对误差已到 1.8e-3）
            let tol = 1e-3 * view.length() + 1e-4;
            assert!(
                (back - view).length() < tol,
                "重建 ({x},{y},{z}) 得到 {back}"
            );
        }
    }

    #[test]
    fn normal_from_neighbors_rejects_degenerate() {
        let c = Vec3::new(0.0, 0.0, -1.0);
        let n = normal_from_neighbors(c, Vec3::new(1.0, 0.0, -1.0), Vec3::new(0.0, 1.0, -1.0))
            .expect("平面法线");
        assert!((n.z.abs() - 1.0).abs() < 1e-5);
        // 完全共线 → None
        assert!(normal_from_neighbors(c, c + Vec3::X, c + Vec3::X * 2.0).is_none());
        // 非有限 → None
        assert!(normal_from_neighbors(c, Vec3::new(f32::NAN, 0.0, 0.0), Vec3::Y).is_none());
    }

    #[test]
    fn occlusion_needs_hemisphere_range_and_bias() {
        let pos = Vec3::new(0.0, 0.0, -2.0);
        let n = Vec3::Z;
        let radius = 0.6;
        let bias = 0.02;
        // 正上方、半径内 → 遮挡
        assert!(is_occluded(
            pos,
            n,
            pos + Vec3::new(0.1, 0.0, 0.3),
            radius,
            bias
        ));
        // 背面 → 不遮挡
        assert!(!is_occluded(
            pos,
            n,
            pos + Vec3::new(0.1, 0.0, -0.3),
            radius,
            bias
        ));
        // 超出半径 → 不遮挡
        assert!(!is_occluded(
            pos,
            n,
            pos + Vec3::new(0.0, 0.0, 1.5),
            radius,
            bias
        ));
        // 偏置以内 → 不遮挡（自遮挡）
        assert!(!is_occluded(
            pos,
            n,
            pos + Vec3::new(0.0, 0.0, 0.01),
            radius,
            bias
        ));
    }

    #[test]
    fn ao_factor_maps_ratio_and_clamps() {
        assert_eq!(ao_from_occlusion(0, 8, 1.0), 1.0);
        assert!((ao_from_occlusion(8, 8, 1.0) - 0.0).abs() < 1e-6);
        assert!((ao_from_occlusion(4, 8, 1.0) - 0.5).abs() < 1e-6);
        // 强度 2 时一半遮挡就压到下限
        assert_eq!(ao_from_occlusion(4, 8, 2.0), MIN_AO_FACTOR);
        // 0 样本 → 1，不能除零
        assert_eq!(ao_from_occlusion(0, 0, 1.0), 1.0);
        // NaN 强度按缺省处理
        assert!((ao_from_occlusion(4, 8, f32::NAN) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn settings_sanitize_and_clamp() {
        let s = SsaoSettings {
            enabled: true,
            radius: f32::NAN,
            bias: -1.0,
            intensity: 1e9,
            samples: 200,
        }
        .sanitized();
        assert_eq!(s.radius, DEFAULT_SSAO_RADIUS);
        assert_eq!(s.bias, MIN_SSAO_BIAS);
        assert_eq!(s.intensity, MAX_SSAO_INTENSITY);
        assert_eq!(s.samples, MAX_SSAO_SAMPLES);
        assert_eq!(s.effective_samples(), MAX_SSAO_SAMPLES as usize);
    }

    #[test]
    fn default_settings_are_neutral() {
        let s = SsaoSettings::default();
        assert!(s.is_neutral());
        assert_eq!(s.sanitized(), s);
        assert_eq!(s.effective_samples(), DEFAULT_SSAO_SAMPLES as usize);
    }
}
