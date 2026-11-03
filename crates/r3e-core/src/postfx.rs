//! 后处理契约（扩展十三）：曝光、色调映射、泛光提取与 FXAA 判定。
//!
//! 扩展十二把环境光换成了 HDR 辐亮度（天空 + 太阳），但渲染目标仍是 sRGB 表面：
//! 任何超过 1.0 的线性亮度在写进 8 位目标时被直接切平，高亮区（太阳、自发光、
//! 金属高光）全都糊成一片死白，IBL 带来的动态范围红利完全浪费。本模块是
//! 「HDR 后处理链」的 CPU 侧契约：
//!
//! * [`PostFxSettings`]：场景 `postfx` 段，带 [`PostFxSettings::sanitized`] 写入前收敛；
//! * [`expose`]：曝光（EV 制，`2^ev` 线性乘子）；
//! * [`bright_pass`]：软膝亮部提取，只把阈值以上的部分喂给泛光；
//! * [`tonemap`]：Reinhard / ACES（Narkowicz 2015 拟合）/ 直通 clamp 三档；
//! * [`fxaa_edge_visible`]：FXAA 的边缘判定（对比度是否越过阈值），
//!   着色器里的分支条件与这里同源；
//! * [`resolve`]：`hdr + bloom → 曝光 → 色调映射` 的完整 CPU 参考实现。
//!
//! 与 [`crate::env`] 一样，本模块零 wgpu 依赖：GPU 侧只是把同样的公式搬到 WGSL，
//! `crates/r3e-render/tests/shader_wgsl.rs` 用常量比对锁住两边不漂移。

use crate::ssao::SsaoSettings;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// 曝光下限（EV）：`2^-8 ≈ 0.0039`，足够暗到只剩自发光。
pub const MIN_EXPOSURE_EV: f32 = -8.0;

/// 曝光上限（EV）：`2^16 = 65536`，再往上是纯粹的过曝白，没有信息量。
pub const MAX_EXPOSURE_EV: f32 = 16.0;

/// 缺省曝光：0 EV = 不缩放，与扩展十二的画面逐像素等价。
pub const DEFAULT_EXPOSURE_EV: f32 = 0.0;

/// 泛光阈值缺省值：线性 HDR 里 1.0 以上才算「比漫反射白更亮」。
pub const DEFAULT_BLOOM_THRESHOLD: f32 = 1.0;

/// 泛光阈值上限：再高就只有太阳盘能被提取出来。
pub const MAX_BLOOM_THRESHOLD: f32 = 64.0;

/// 软膝宽度缺省值：阈值上下各 0.5 的过渡带，避免硬阈值导致的光晕边缘台阶。
pub const DEFAULT_BLOOM_KNEE: f32 = 0.5;

/// 软膝宽度上限：过渡带比阈值本身还宽就没意义了。
pub const MAX_BLOOM_KNEE: f32 = 8.0;

/// 泛光强度缺省值：0.18 是「能看出光晕但不糊画面」的经验值。
pub const DEFAULT_BLOOM_INTENSITY: f32 = 0.18;

/// 泛光强度上限：4.0 已经强到近似辉光滤镜，再高就是纯风格化。
pub const MAX_BLOOM_INTENSITY: f32 = 4.0;

/// 亮部提取里判定 `knee == 0` 的容差：低于它就走硬阈值分支，避免除零。
pub const KNEE_EPSILON: f32 = 1e-4;

/// ACES 曲线拟合常数（Narkowicz 2015，"ACES Filmic Tone Mapping Curve"）。
pub const ACES_A: f32 = 2.51;
pub const ACES_B: f32 = 0.03;
pub const ACES_C: f32 = 2.43;
pub const ACES_D: f32 = 0.59;
pub const ACES_E: f32 = 0.14;

/// Rec.709 亮度权重：FXAA 只用亮度判断边缘，不逐通道比较。
pub const LUMA_WEIGHTS: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// FXAA 绝对边缘阈值（低于它的对比度一律认为不是边缘，压掉暗部噪点）。
pub const FXAA_EDGE_THRESHOLD_MIN: f32 = 1.0 / 32.0;

/// FXAA 相对边缘阈值：与局部最亮值相乘，亮区需要更强的对比度才算边缘。
pub const FXAA_EDGE_THRESHOLD_RELATIVE: f32 = 1.0 / 8.0;

/// 色调映射曲线。
///
/// 缺省 [`TonemapMode::None`]：只做 clamp，与没有后处理的旧画面逐像素一致——
/// 老场景不加 `postfx` 段时行为不变，是本扩展的兼容性保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TonemapMode {
    /// 直通：HDR 直接 clamp 到 [0,1]，高亮区仍是硬切平。
    #[default]
    None,
    /// Reinhard：`x / (1 + x)`，压缩平缓但大面积高光会发灰。
    Reinhard,
    /// ACES（Narkowicz 拟合）：对比更强，保留更多高光细节，缺省推荐给 HDR 场景。
    Aces,
}

impl TonemapMode {
    /// 从脚本 / RPC 的字符串名解析（大小写与空白不敏感），无法识别时返回 `None`。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "clamp" => Some(Self::None),
            "reinhard" => Some(Self::Reinhard),
            "aces" | "aces_fitted" | "filmic" => Some(Self::Aces),
            _ => None,
        }
    }

    /// 规范名（写回场景 / 快照时用，与 `serde` 的 `lowercase` 重命名一致）。
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Reinhard => "reinhard",
            Self::Aces => "aces",
        }
    }

    /// 传进 GPU uniform 的编码（0 = none / 1 = reinhard / 2 = aces）。
    pub fn code(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Reinhard => 1,
            Self::Aces => 2,
        }
    }
}

/// 泛光（bloom）参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BloomSettings {
    /// 是否启用泛光。缺省关闭：泛光是风格化效果，不该在老场景上默认打开。
    #[serde(default)]
    pub enabled: bool,
    /// 亮部提取阈值（线性 HDR 单位）。只有亮度超过它的像素才参与光晕。
    #[serde(default = "default_bloom_threshold")]
    pub threshold: f32,
    /// 软膝宽度：阈值上下各这么多单位内做平滑过渡，0 表示硬阈值。
    #[serde(default = "default_bloom_knee")]
    pub knee: f32,
    /// 泛光叠加强度：0 = 光晕不可见，0.1 左右为真实观感。
    #[serde(default = "default_bloom_intensity")]
    pub intensity: f32,
}

fn default_bloom_threshold() -> f32 {
    DEFAULT_BLOOM_THRESHOLD
}

fn default_bloom_knee() -> f32 {
    DEFAULT_BLOOM_KNEE
}

fn default_bloom_intensity() -> f32 {
    DEFAULT_BLOOM_INTENSITY
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: DEFAULT_BLOOM_THRESHOLD,
            knee: DEFAULT_BLOOM_KNEE,
            intensity: DEFAULT_BLOOM_INTENSITY,
        }
    }
}

impl BloomSettings {
    /// 收敛到合法区间，并在一次写入里做完：
    ///
    /// * 任何字段 NaN → 回落到缺省值（不是 0——0 阈值会把整幅画面当亮部）；
    /// * `threshold` 夹进 `[0, MAX_BLOOM_THRESHOLD]`，`knee` 夹进 `[0, MAX_BLOOM_KNEE]`；
    /// * `intensity` 夹进 `[0, MAX_BLOOM_INTENSITY]`（负强度会让泛光变成减光）。
    pub fn sanitized(self) -> Self {
        Self {
            enabled: self.enabled,
            threshold: sanitize_range(
                self.threshold,
                default_bloom_threshold(),
                0.0,
                MAX_BLOOM_THRESHOLD,
            ),
            knee: sanitize_range(self.knee, default_bloom_knee(), 0.0, MAX_BLOOM_KNEE),
            intensity: sanitize_range(
                self.intensity,
                default_bloom_intensity(),
                0.0,
                MAX_BLOOM_INTENSITY,
            ),
        }
    }
}

/// HDR 后处理链参数（场景 JSON 的 `postfx` 段）。
///
/// 场景里缺这个段（`Scene::postfx == None`）等价于 [`PostFxSettings::default`]：
/// 曝光 0、不调色、不开泛光、不开 FXAA —— 即「只有离屏 HDR 目标 + composite 直通」，
/// 画面与扩展十二一致。这样老场景一行不改也能继续跑。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PostFxSettings {
    /// 曝光（EV）。0 = 不缩放。
    #[serde(default = "default_exposure")]
    pub exposure: f32,
    /// 色调映射曲线。
    #[serde(default)]
    pub tonemap: TonemapMode,
    /// 泛光参数。
    #[serde(default)]
    pub bloom: BloomSettings,
    /// 是否开启 FXAA：在 composite 里按亮度梯度做一次边缘内插，
    /// 弥补引擎没有 MSAA 的锯齿。
    #[serde(default)]
    pub fxaa: bool,
    /// 屏幕空间环境光遮蔽（扩展十七）：缺省关闭，composite 乘 1。
    #[serde(default)]
    pub ssao: SsaoSettings,
}

fn default_exposure() -> f32 {
    DEFAULT_EXPOSURE_EV
}

impl Default for PostFxSettings {
    fn default() -> Self {
        Self {
            exposure: DEFAULT_EXPOSURE_EV,
            tonemap: TonemapMode::None,
            bloom: BloomSettings::default(),
            fxaa: false,
            ssao: SsaoSettings::default(),
        }
    }
}

impl PostFxSettings {
    /// 收敛到合法区间（见 [`sanitize_range`] 的 NaN 策略）。
    pub fn sanitized(self) -> Self {
        Self {
            exposure: sanitize_range(
                self.exposure,
                DEFAULT_EXPOSURE_EV,
                MIN_EXPOSURE_EV,
                MAX_EXPOSURE_EV,
            ),
            tonemap: self.tonemap,
            bloom: self.bloom.sanitized(),
            fxaa: self.fxaa,
            ssao: self.ssao.sanitized(),
        }
    }

    /// 全部效果都关着（曝光 0、直通色调映射、无泛光、无 FXAA）。
    ///
    /// 这种配置下 composite 只剩 `clamp`，渲染层的泛光 pass 可以整条跳过。
    pub fn is_neutral(&self) -> bool {
        self.exposure == DEFAULT_EXPOSURE_EV
            && self.tonemap == TonemapMode::None
            && !self.bloom.enabled
            && !self.fxaa
            && self.ssao.is_neutral()
    }
}

/// `NaN` 回落 `fallback`，其余夹进 `[min, max]`（并顺手把 `-0.0` 归成 `0.0`）。
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

/// Rec.709 相对亮度。FXAA 与调试探针都用它。
pub fn luminance(color: Vec3) -> f32 {
    color.dot(LUMA_WEIGHTS)
}

/// 曝光乘子：`2^ev`，EV 先夹进 `[MIN_EXPOSURE_EV, MAX_EXPOSURE_EV]`（NaN 按 0 EV）。
pub fn exposure_scale(ev: f32) -> f32 {
    let ev = sanitize_range(ev, DEFAULT_EXPOSURE_EV, MIN_EXPOSURE_EV, MAX_EXPOSURE_EV);
    ev.exp2()
}

/// 线性 HDR 颜色乘上曝光（负值先归零：HDR 颜色不该有负分量）。
pub fn expose(color: Vec3, ev: f32) -> Vec3 {
    color.max(Vec3::ZERO) * exposure_scale(ev)
}

/// 软膝亮部提取：返回参与泛光的部分（已经乘上 0..1 的比例，颜色比例不变）。
///
/// 曲线来自 Unity Post-processing 的软膝做法：
///
/// ```text
/// soft = clamp(brightness - threshold + knee, 0, 2·knee)
/// soft = soft² / (4·knee)
/// w    = max(soft, brightness - threshold) / brightness
/// ```
///
/// 于是 `brightness ≤ threshold - knee` 时全黑、`brightness ≥ threshold + knee` 时
/// 退化成 `(b - threshold)/b`（硬阈值的解析形式），中间是二次过渡。
/// `knee = 0` 时直接走硬阈值（不做除法）。
pub fn bright_pass(color: Vec3, threshold: f32, knee: f32) -> Vec3 {
    let threshold = sanitize_range(threshold, DEFAULT_BLOOM_THRESHOLD, 0.0, MAX_BLOOM_THRESHOLD);
    let knee = sanitize_range(knee, DEFAULT_BLOOM_KNEE, 0.0, MAX_BLOOM_KNEE);
    let color = color.max(Vec3::ZERO);
    // 三通道取最大当作亮度：给每个通道乘同一个权重，光晕不会串色
    let brightness = color.max_element();
    if brightness <= 1e-6 {
        return Vec3::ZERO;
    }
    let contribution = if knee <= KNEE_EPSILON {
        (brightness - threshold).max(0.0) / brightness
    } else {
        let soft = (brightness - threshold + knee).clamp(0.0, 2.0 * knee);
        let soft = soft * soft / (4.0 * knee);
        let hard = (brightness - threshold).max(0.0);
        soft.max(hard) / brightness
    };
    color * contribution.clamp(0.0, 1.0)
}

/// Reinhard 色调映射：`x / (1 + x)`，逐通道。
pub fn tonemap_reinhard(color: Vec3) -> Vec3 {
    let c = color.max(Vec3::ZERO);
    Vec3::new(c.x / (1.0 + c.x), c.y / (1.0 + c.y), c.z / (1.0 + c.z))
}

/// ACES 色调映射（Narkowicz 2015 有理拟合）：
/// `(x(ax+b)) / (x(cx+d) + e)`，逐通道，结果夹进 [0,1]。
pub fn tonemap_aces(color: Vec3) -> Vec3 {
    let c = color.max(Vec3::ZERO);
    let curve = |x: f32| {
        let num = x * (ACES_A * x + ACES_B);
        let den = x * (ACES_C * x + ACES_D) + ACES_E;
        (num / den).clamp(0.0, 1.0)
    };
    Vec3::new(curve(c.x), curve(c.y), curve(c.z))
}

/// 按模式做色调映射。`None` 分支是 clamp 而不是恒等——composite 写的是
/// 8 位 sRGB 目标，超过 1.0 的部分在 GPU 侧同样会被切平，CPU 参考实现必须一致。
pub fn tonemap(color: Vec3, mode: TonemapMode) -> Vec3 {
    match mode {
        TonemapMode::None => color.max(Vec3::ZERO).min(Vec3::ONE),
        TonemapMode::Reinhard => tonemap_reinhard(color),
        TonemapMode::Aces => tonemap_aces(color),
    }
}

/// FXAA 是否需要处理这个像素：局部对比度同时越过绝对阈值与相对阈值。
///
/// `luma_min` / `luma_max` 是 3x3（本项目取十字五抽样）邻域的亮度极值。
/// 两条阈值一起用是 FXAA 的标准做法：暗部靠绝对阈值压噪点，
/// 亮部靠相对阈值避免把渐变当边缘。
pub fn fxaa_edge_visible(luma_min: f32, luma_max: f32) -> bool {
    if !luma_min.is_finite() || !luma_max.is_finite() {
        return false;
    }
    let contrast = luma_max - luma_min;
    let floor = FXAA_EDGE_THRESHOLD_MIN.max(luma_max * FXAA_EDGE_THRESHOLD_RELATIVE);
    contrast >= floor
}

/// composite 的 CPU 参考实现：`(hdr + bloom·强度) → 曝光 → 色调映射`。
///
/// 泛光在**曝光之前**叠加，与着色器一致：radiance 相加后再统一曝光，
/// 才是「同一场景、同一台相机、不同曝光」的物理含义。泛光关闭时 `bloom` 被忽略。
pub fn resolve(hdr: Vec3, bloom: Vec3, settings: &PostFxSettings) -> Vec3 {
    let mut color = hdr.max(Vec3::ZERO);
    if settings.bloom.enabled {
        color += bloom.max(Vec3::ZERO) * settings.bloom.intensity;
    }
    tonemap(expose(color, settings.exposure), settings.tonemap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn defaults_are_fixed_points_of_sanitizer() {
        let d = PostFxSettings::default();
        assert_eq!(d.sanitized(), d, "缺省值必须已经是收敛后的定点");
        assert!(d.is_neutral(), "缺省配置应为直通（不加 postfx 段的老场景）");
        assert_eq!(d.tonemap.name(), "none");
        assert_eq!(d.bloom.intensity, DEFAULT_BLOOM_INTENSITY);
        assert!(!d.bloom.enabled);
        assert!(!d.fxaa);
    }

    #[test]
    fn nan_falls_back_and_ranges_clamp() {
        let s = PostFxSettings {
            exposure: f32::NAN,
            tonemap: TonemapMode::Aces,
            bloom: BloomSettings {
                enabled: true,
                threshold: f32::NAN,
                knee: -3.0,
                intensity: 99.0,
            },
            fxaa: true,
            ssao: Default::default(),
        }
        .sanitized();
        assert_eq!(s.exposure, DEFAULT_EXPOSURE_EV);
        assert_eq!(s.bloom.threshold, DEFAULT_BLOOM_THRESHOLD);
        assert_eq!(s.bloom.knee, 0.0);
        assert_eq!(s.bloom.intensity, MAX_BLOOM_INTENSITY);

        let clamped = PostFxSettings {
            exposure: 1000.0,
            ..PostFxSettings::default()
        }
        .sanitized();
        assert_eq!(clamped.exposure, MAX_EXPOSURE_EV);
        let clamped = PostFxSettings {
            exposure: -1000.0,
            ..PostFxSettings::default()
        }
        .sanitized();
        assert_eq!(clamped.exposure, MIN_EXPOSURE_EV);
    }

    #[test]
    fn tonemap_names_roundtrip() {
        for mode in [TonemapMode::None, TonemapMode::Reinhard, TonemapMode::Aces] {
            assert_eq!(TonemapMode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(TonemapMode::from_name(" ACES "), Some(TonemapMode::Aces));
        assert_eq!(TonemapMode::from_name("bogus"), None);
        // GPU 编码必须严格递增且各不相同，着色器与之对应
        assert_eq!(TonemapMode::None.code(), 0);
        assert_eq!(TonemapMode::Reinhard.code(), 1);
        assert_eq!(TonemapMode::Aces.code(), 2);
    }

    #[test]
    fn exposure_is_power_of_two() {
        assert_eq!(exposure_scale(0.0), 1.0);
        assert!(approx(exposure_scale(1.0), 2.0, 1e-6));
        assert!(approx(exposure_scale(-1.0), 0.5, 1e-6));
        // NaN / 越界都被收敛，不会产生 inf 或 NaN
        assert_eq!(exposure_scale(f32::NAN), 1.0);
        let bright = expose(Vec3::splat(0.5), MAX_EXPOSURE_EV);
        assert!(bright.is_finite() && bright.x > 0.5);
        // 负的 HDR 分量先归零，避免乘出负数
        assert_eq!(
            expose(Vec3::new(-1.0, 0.5, -1.0), 0.0),
            Vec3::new(0.0, 0.5, 0.0)
        );
    }

    #[test]
    fn bright_pass_respects_threshold_and_knee() {
        // 阈值以下（含软膝下沿）全黑
        assert_eq!(bright_pass(Vec3::splat(0.4), 1.0, 0.5), Vec3::ZERO);
        // 阈值以上：按「最亮通道」算出的比例整体缩放，所以颜色比例不变（光晕不串色）
        let c = bright_pass(Vec3::new(4.0, 2.0, 1.5), 1.0, 0.0);
        assert!(approx(c.x, 3.0, 1e-4), "实际 {c:?}");
        assert!(approx(c.y, 1.5, 1e-4), "实际 {c:?}");
        assert!(approx(c.z, 1.125, 1e-4), "实际 {c:?}");
        // 之所以不是逐通道减阈值：那样会让 1.5 的蓝通道被削到 0.5，
        // 亮部整体偏色（红黄光晕的蓝分量被吃光）。这里用 max 通道定比例：
        // w = (4-1)/4 = 0.75，三通道同乘 0.75。
        // 软膝：阈值处应有半程左右的过渡，而不是 0
        let mid = bright_pass(Vec3::splat(1.0), 1.0, 0.5);
        assert!(
            mid.x > 0.0 && mid.x < 1.0,
            "软膝在阈值处应给出过渡，实际 {mid:?}"
        );
        // 软膝上沿之后与硬阈值同解
        let soft = bright_pass(Vec3::splat(3.0), 1.0, 0.5);
        let hard = bright_pass(Vec3::splat(3.0), 1.0, 0.0);
        assert!(approx(soft.x, hard.x, 1e-4), "soft {soft:?} hard {hard:?}");
        // 单调：亮度越高，提取出的能量越多
        let mut last = 0.0;
        for i in 1..=20 {
            let v = i as f32 * 0.5;
            let b = bright_pass(Vec3::splat(v), 0.8, 0.4).x;
            assert!(b >= last, "亮度 {v} 时能量 {b} 反而小于前一档 {last}");
            last = b;
        }
        // 全黑不会出现 NaN（除零保护）
        let zero = bright_pass(Vec3::ZERO, 0.0, 0.0);
        assert_eq!(zero, Vec3::ZERO);
        assert!(zero.is_finite());
    }

    #[test]
    fn tonemap_curves_are_monotonic_and_bounded() {
        for mode in [TonemapMode::Reinhard, TonemapMode::Aces] {
            assert_eq!(tonemap(Vec3::ZERO, mode), Vec3::ZERO);
            let mut last = -1.0;
            for i in 0..40 {
                let x = i as f32 * 0.5;
                let y = tonemap(Vec3::splat(x), mode).x;
                assert!(y >= last, "{mode:?} 在 x={x} 处非单调");
                assert!((0.0..=1.0).contains(&y), "{mode:?} 在 x={x} 处越界: {y}");
                last = y;
            }
            // 极端高亮必须被压进 1.0，而不是继续增长
            let hot = tonemap(Vec3::splat(1000.0), mode);
            assert!(
                hot.x <= 1.0 && hot.x > 0.9,
                "{mode:?} 高亮收敛异常: {hot:?}"
            );
        }
        // Reinhard 在 1.0 处正好给 0.5，是最好的解析对照点
        assert!(approx(tonemap_reinhard(Vec3::ONE).x, 0.5, 1e-6));
        // ACES 在 0 处必须严格为 0（B 项不能被漏掉）
        assert_eq!(tonemap_aces(Vec3::ZERO), Vec3::ZERO);
        // None 分支是 clamp，不是恒等
        assert_eq!(tonemap(Vec3::splat(3.0), TonemapMode::None), Vec3::ONE);
    }

    #[test]
    fn resolve_matches_manual_chain() {
        let settings = PostFxSettings {
            exposure: 1.0,
            tonemap: TonemapMode::Reinhard,
            bloom: BloomSettings {
                enabled: true,
                threshold: 1.0,
                knee: 0.5,
                intensity: 0.5,
            },
            fxaa: true,
            ssao: Default::default(),
        }
        .sanitized();
        let hdr = Vec3::new(2.0, 0.5, 0.25);
        let bloom = Vec3::new(1.0, 0.0, 0.0);
        let got = resolve(hdr, bloom, &settings);
        let manual = tonemap_reinhard(expose(hdr + bloom * 0.5, 1.0));
        assert!(approx(got.x, manual.x, 1e-6));
        assert!(approx(got.y, manual.y, 1e-6));
        assert!(approx(got.z, manual.z, 1e-6));
        // 泛光关闭时 bloom 参数必须被忽略
        let off = PostFxSettings {
            bloom: BloomSettings {
                enabled: false,
                ..settings.bloom
            },
            ..settings
        }
        .sanitized();
        let a = resolve(hdr, Vec3::ZERO, &off);
        let b = resolve(hdr, Vec3::splat(5.0), &off);
        assert_eq!(a, b, "bloom 关闭时不应再有叠加");
        // 直通配置下 resolve 就是 clamp
        let neutral = PostFxSettings::default();
        assert_eq!(resolve(Vec3::splat(3.0), Vec3::ZERO, &neutral), Vec3::ONE);
    }

    #[test]
    fn fxaa_edge_thresholds() {
        // 纯色区域不是边缘
        assert!(!fxaa_edge_visible(0.5, 0.5));
        // 对比度极大的明暗交界是边缘
        assert!(fxaa_edge_visible(0.0, 1.0));
        // 亮区里的小起伏：相对阈值 1/8 挡掉（0.9→1.0 只有 0.1，不到最亮值 1.0 的 1/8）
        assert!(!fxaa_edge_visible(0.9, 1.0));
        // 亮区里足够强的交界仍判为边缘
        assert!(fxaa_edge_visible(0.5, 1.0));
        // NaN 不得判为边缘（否则整屏都去模糊）
        assert!(!fxaa_edge_visible(f32::NAN, 1.0));
        assert!(!fxaa_edge_visible(0.0, f32::INFINITY));
    }

    #[test]
    fn luminance_uses_rec709_weights() {
        assert!(approx(luminance(Vec3::ONE), 1.0, 1e-6));
        assert!(approx(luminance(Vec3::new(0.0, 1.0, 0.0)), 0.7152, 1e-6));
        assert!(approx(luminance(Vec3::ZERO), 0.0, 1e-6));
    }
}
