//! 环境光照（IBL）的 CPU 参考实现：程序化天空、球谐辐照度、高光解析近似。
//!
//! 阶段十一给引擎装上了 Cook-Torrance 直接光照，但「环境光」还只是一个常数
//! (`Light::ambient`)，金属物体因此只有一盏方向光的反射，暗部一片死黑。
//! 本模块提供不依赖任何 GPU 能力的环境探针数学：
//!
//! * [`EnvironmentSettings`]：程序化天空（天顶 / 地平线 / 地面三色 + 太阳项），
//!   带 [`EnvironmentSettings::sanitized`] 写入前收敛；
//! * [`sky_radiance`] / [`sky_radiance_blurred`]：按方向求环境辐亮度，后者按粗糙度
//!   把高频（太阳盘）压掉、趋向方向无关的环境平均；
//! * [`Sh9`]：球谐 9 系数，[`Sh9::project`] 把天空投成低频系数，
//!   [`Sh9::irradiance`] 用 Lambertian 余弦卷积求任意法线方向的辐照度
//!   （漫反射 IBL 只需这 9 个 float3，不必生成 cubemap）；
//! * [`env_brdf_approx`]：Karis 的解析环境 BRDF 近似，给高光 IBL 用，
//!   省掉预积分 BRDF LUT 纹理；
//! * [`specular_dominant_dir`]：粗糙度把反射方向拉向法线的近似，决定采样哪个方向的环境。
//!
//! 这套「SH + 解析近似」的组合是移动端常用路线：一次上传 27 个 f32 就能开 IBL，
//! 没有 cubemap 通道、没有预过滤 mip 链、没有额外渲染目标。代价是高频反射
//! （清晰镜面里看到的天空轮廓）被抹平，后续要升级成全预过滤环境图时，
//! 只需替换 [`sky_radiance_blurred`] 的取值来源，着色器接口不变。
//!
//! 本模块同样零 wgpu 依赖，`r3e-core` 对图形 API 的零依赖边界不变。

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::material::{DEFAULT_ROUGHNESS, MAX_EMISSIVE, PI};

/// 环境辐亮度上限（线性 HDR）。与自发光共用同一个量级上限，挡住 inf 与离谱数值。
pub const MAX_ENV_RADIANCE: f32 = MAX_EMISSIVE;

/// IBL 总强度上限：允许过曝一点做风格化，但要挡住 1e30。
pub const MAX_IBL_INTENSITY: f32 = 8.0;

/// 球谐投影的默认采样数。天空函数是低频的（三色插值 + 一个太阳盘），
/// 2048 个 Fibonacci 均匀方向已足够，投影只在场景变更时做一次。
pub const DEFAULT_SH_SAMPLES: usize = 2048;

/// 太阳盘形状指数：`cos^k` 里的 k。k = 8 给出约 40° 的柔和光晕，
/// 太小会让太阳占满半球、太大则在球谐里变成采样噪声。
pub const SUN_SHAPE_EXPONENT: i32 = 8;

/// 球谐基函数个数（l = 0..2，共 9 项）。
pub const SH_COEFFICIENTS: usize = 9;

/// Lambertian 余弦卷积的三档衰减系数（Ramamoorthi & Hanrahan 2001）。
const LAMBERT_A0: f32 = PI;
const LAMBERT_A1: f32 = 2.0 * PI / 3.0;
const LAMBERT_A2: f32 = PI / 4.0;

/// 程序化天空参数。
///
/// 刻意不用 HDR 图片：场景是纯文本可编程的，环境也应该能被脚本一行改掉，
/// 而且省掉一次外部资产依赖。三色球面插值足以给出「天、地、太阳」三要素。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSettings {
    /// 天顶辐亮度（线性 RGB）。
    #[serde(default = "default_zenith")]
    pub sky_zenith: [f32; 3],
    /// 地平线辐亮度（线性 RGB）。天与地的过渡都经过它，通常最亮。
    #[serde(default = "default_horizon")]
    pub sky_horizon: [f32; 3],
    /// 地面辐亮度（线性 RGB），只影响朝下的方向。
    #[serde(default = "default_ground")]
    pub ground: [f32; 3],
    /// 太阳方向（世界空间，指向太阳）。写入前会被归一化，零向量回退为正上方。
    #[serde(default = "default_sun_dir")]
    pub sun_dir: [f32; 3],
    /// 太阳颜色（线性 RGB）。
    #[serde(default = "default_sun_color")]
    pub sun_color: [f32; 3],
    /// 太阳强度：0 表示阴天（只剩三色天空），数值越大太阳盘越亮。
    #[serde(default = "default_sun_intensity")]
    pub sun_intensity: f32,
    /// IBL 总强度：最终环境项的整体乘数，1.0 为物理值，调它是给美术留的旋钮。
    #[serde(default = "default_intensity")]
    pub intensity: f32,
}

fn default_zenith() -> [f32; 3] {
    [0.18, 0.24, 0.36]
}

fn default_horizon() -> [f32; 3] {
    [0.30, 0.33, 0.38]
}

fn default_ground() -> [f32; 3] {
    [0.10, 0.09, 0.08]
}

fn default_sun_dir() -> [f32; 3] {
    // 与 assets/scene.json 的主光反方向一致（场景光是「光线传播方向」，这里要「指向太阳」），
    // 已归一化到 f32 可表示精度，保证 default() 在 sanitized() 下基本是定点不动
    [0.422_390_9, 0.844_781_9, 0.328_526_3]
}

fn default_sun_color() -> [f32; 3] {
    [1.0, 0.96, 0.9]
}

fn default_sun_intensity() -> f32 {
    1.15
}

fn default_intensity() -> f32 {
    1.0
}

impl Default for EnvironmentSettings {
    fn default() -> Self {
        Self {
            sky_zenith: default_zenith(),
            sky_horizon: default_horizon(),
            ground: default_ground(),
            sun_dir: default_sun_dir(),
            sun_color: default_sun_color(),
            sun_intensity: default_sun_intensity(),
            intensity: default_intensity(),
        }
    }
}

impl EnvironmentSettings {
    /// 参数收敛：NaN 回退缺省、颜色分量夹到 `0..=MAX_ENV_RADIANCE`、
    /// 太阳方向归一化、强度夹到合法区间。
    ///
    /// 与 [`crate::material::SurfaceMaterial::sanitized`] 一样，这是唯一收敛入口：
    /// 场景 JSON、脚本、编辑器 RPC 三条路径都必须经过它。
    pub fn sanitized(self) -> Self {
        Self {
            sky_zenith: sanitize_rgb(self.sky_zenith),
            sky_horizon: sanitize_rgb(self.sky_horizon),
            ground: sanitize_rgb(self.ground),
            sun_dir: sanitize_dir(self.sun_dir),
            sun_color: sanitize_rgb(self.sun_color),
            sun_intensity: sanitize_scale(
                self.sun_intensity,
                default_sun_intensity(),
                MAX_ENV_RADIANCE,
            ),
            intensity: sanitize_scale(self.intensity, default_intensity(), MAX_IBL_INTENSITY),
        }
    }

    /// 是否与缺省完全一致（场景保存时用来决定是否省略该段）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 天空三色的方向无关平均值，即「彻底模糊」后的环境辐亮度。
    ///
    /// 上下半球各按一半权重：天空平均 = (天顶 + 地平线) / 2，
    /// 地面平均 = (地平线 + 地面) / 2，再各取一半。太阳项另按球面平均份额计入。
    pub fn average_radiance(&self) -> Vec3 {
        let s = self.sanitized();
        let sky = (Vec3::from(s.sky_zenith) + Vec3::from(s.sky_horizon)) * 0.5;
        let ground = (Vec3::from(s.sky_horizon) + Vec3::from(s.ground)) * 0.5;
        let base = (sky + ground) * 0.5;
        base + sun_average_radiance(&s)
    }

    /// 把一个方向上的环境辐亮度写进缓存，供 shader 常量兜底或调试输出。
    pub fn radiance_at(&self, dir: Vec3) -> Vec3 {
        sky_radiance(dir, &self.sanitized())
    }
}

fn sanitize_rgb(rgb: [f32; 3]) -> [f32; 3] {
    [
        sanitize_component(rgb[0]),
        sanitize_component(rgb[1]),
        sanitize_component(rgb[2]),
    ]
}

fn sanitize_component(v: f32) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    let clamped = v.clamp(0.0, MAX_ENV_RADIANCE);
    if clamped == 0.0 {
        0.0
    } else {
        clamped
    }
}

fn sanitize_scale(v: f32, fallback: f32, max: f32) -> f32 {
    if v.is_nan() {
        return fallback;
    }
    let clamped = v.clamp(0.0, max);
    if clamped == 0.0 {
        0.0
    } else {
        clamped
    }
}

fn sanitize_dir(dir: [f32; 3]) -> [f32; 3] {
    let v = Vec3::from(dir);
    if !v.is_finite() || v.length_squared() < 1e-12 {
        return [0.0, 1.0, 0.0];
    }
    let n = v.normalize();
    [n.x, n.y, n.z]
}

/// 方向归一化：零向量 / 非有限值回退到 `fallback`，绝不让 NaN 流进光照计算。
fn normalize_or(dir: Vec3, fallback: Vec3) -> Vec3 {
    if !dir.is_finite() || dir.length_squared() < 1e-12 {
        return fallback;
    }
    dir.normalize()
}

/// 太阳盘亮度：`cos^k` 形状乘颜色与强度，只加不减。
fn sun_radiance(dir: Vec3, s: &EnvironmentSettings) -> Vec3 {
    if s.sun_intensity <= 0.0 {
        return Vec3::ZERO;
    }
    let axis = Vec3::from(s.sun_dir);
    let cos = dir.dot(axis).clamp(-1.0, 1.0);
    if cos <= 0.0 {
        return Vec3::ZERO;
    }
    let shape = cos.powi(SUN_SHAPE_EXPONENT);
    Vec3::from(s.sun_color) * (s.sun_intensity * shape)
}

/// 太阳项在整球上的平均辐亮度。
///
/// `cos^k` 在半球上的平均值是 `1 / (2(k+1))`（k = 8 时约 5.6%），
/// 天空模糊到极限时太阳不会整块消失，而是收敛到这个份额——这比直接归零更接近真实观感。
fn sun_average_radiance(s: &EnvironmentSettings) -> Vec3 {
    if s.sun_intensity <= 0.0 {
        return Vec3::ZERO;
    }
    let share = 1.0 / (2.0 * (SUN_SHAPE_EXPONENT as f32 + 1.0));
    Vec3::from(s.sun_color) * (s.sun_intensity * share)
}

/// 天空三色插值（不含太阳项）。
fn sky_base(dir: Vec3, s: &EnvironmentSettings) -> Vec3 {
    let zenith = Vec3::from(s.sky_zenith);
    let horizon = Vec3::from(s.sky_horizon);
    let ground = Vec3::from(s.ground);
    let up = dir.y.clamp(-1.0, 1.0);
    if up >= 0.0 {
        // 开方让地平线附近的过渡带更宽，纯线性插值会让天顶色压到地平线上
        horizon.lerp(zenith, up.sqrt())
    } else {
        horizon.lerp(ground, (-up).sqrt())
    }
}

/// 给定方向的环境辐亮度（线性 RGB）：三色天空 + 太阳项。
///
/// 传入的 `settings` 不必预先收敛，内部会收敛一次（几次 clamp，成本可忽略）。
pub fn sky_radiance(dir: Vec3, settings: &EnvironmentSettings) -> Vec3 {
    let s = settings.sanitized();
    let d = normalize_or(dir, Vec3::Y);
    sky_base(d, &s) + sun_radiance(d, &s)
}

/// 按粗糙度模糊后的环境辐亮度：高光 IBL 采样用。
///
/// `roughness = 0` 时与 [`sky_radiance`] 完全相同；`roughness = 1` 时退化成
/// [`EnvironmentSettings::average_radiance`]（方向无关）——粗糙表面本来就看不到
/// 清晰的环境轮廓。中间用 `roughness²` 插值，与 GGX 峰值展宽一致；太阳盘额外
/// 按 `(1 - r²)²` 衰减，避免粗糙面还留着一个锐利亮点。
pub fn sky_radiance_blurred(dir: Vec3, roughness: f32, settings: &EnvironmentSettings) -> Vec3 {
    let s = settings.sanitized();
    let d = normalize_or(dir, Vec3::Y);
    let r = if roughness.is_nan() {
        DEFAULT_ROUGHNESS
    } else {
        roughness.clamp(0.0, 1.0)
    };
    let t = r * r;
    let attenuation = (1.0 - t) * (1.0 - t);
    let sharp = sky_base(d, &s) + sun_radiance(d, &s) * attenuation;
    sharp.lerp(s.average_radiance(), t)
}

/// 球谐 9 系数（l ≤ 2），每项一个线性 RGB。
///
/// 只用二阶是因为 Lambertian 余弦核把更高阶项几乎全滤掉了（`l = 3` 的衰减系数
/// 不到 0.1），多存系数对漫反射没有可见收益。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sh9 {
    /// 9 个系数，顺序与 [`sh_basis`] 一致。
    pub coeffs: [Vec3; SH_COEFFICIENTS],
}

impl Default for Sh9 {
    fn default() -> Self {
        Self {
            coeffs: [Vec3::ZERO; SH_COEFFICIENTS],
        }
    }
}

impl Sh9 {
    /// 用默认采样数投影环境。
    pub fn project(settings: &EnvironmentSettings) -> Self {
        Self::project_with_samples(settings, DEFAULT_SH_SAMPLES)
    }

    /// 指定采样数投影（测试用；采样数下限 16，避免退化）。
    pub fn project_with_samples(settings: &EnvironmentSettings, samples: usize) -> Self {
        let s = settings.sanitized();
        let n = samples.max(16);
        // 均匀球面采样：每个方向的立体角都是 4π/N
        let solid_angle = 4.0 * PI / n as f32;
        let mut acc = [Vec3::ZERO; SH_COEFFICIENTS];
        for i in 0..n {
            let d = fibonacci_direction(i, n);
            let radiance = sky_base(d, &s) + sun_radiance(d, &s);
            let basis = sh_basis(d);
            for (slot, b) in acc.iter_mut().zip(basis.iter()) {
                *slot += radiance * *b;
            }
        }
        Self {
            coeffs: acc.map(|v| v * solid_angle),
        }
    }

    /// 把系数整体乘上 IBL 强度（GPU 侧也可做，放这里便于 CPU 侧对比）。
    pub fn scaled(self, intensity: f32) -> Self {
        let k = if intensity.is_nan() {
            1.0
        } else {
            intensity.clamp(0.0, MAX_IBL_INTENSITY)
        };
        Self {
            coeffs: self.coeffs.map(|c| c * k),
        }
    }

    /// 给定法线方向的辐照度 `∫ L(ω) (n·ω) dω`（**不含** `1/π`）。
    ///
    /// 着色器里漫反射项应当是 `albedo * irradiance / π`，所以这里不预先除 π，
    /// 保持与定义一致，除不除由调用方决定。
    pub fn irradiance(&self, normal: Vec3) -> Vec3 {
        // Lambertian 卷积系数，顺序与 sh_basis 一致：[A0, A1, A1, A1, A2 ×5]
        const WEIGHTS: [f32; SH_COEFFICIENTS] = [
            LAMBERT_A0, LAMBERT_A1, LAMBERT_A1, LAMBERT_A1, LAMBERT_A2, LAMBERT_A2, LAMBERT_A2,
            LAMBERT_A2, LAMBERT_A2,
        ];
        let n = normalize_or(normal, Vec3::Y);
        let basis = sh_basis(n);
        let mut acc = Vec3::ZERO;
        for ((c, b), w) in self.coeffs.iter().zip(basis.iter()).zip(WEIGHTS.iter()) {
            acc += *c * *b * *w;
        }
        // 截断的二阶近似在极暗方向可能给出极小负值，夹到 0 防止加法结果变暗
        Vec3::new(acc.x.max(0.0), acc.y.max(0.0), acc.z.max(0.0))
    }

    /// 漫反射 IBL 用的辐亮度：辐照度除以 π，可直接乘 albedo 得到出射亮度。
    pub fn diffuse_radiance(&self, normal: Vec3) -> Vec3 {
        self.irradiance(normal) / PI
    }

    /// 所有系数是否有限（GPU 上传前的最后一道检查）。
    pub fn is_finite(&self) -> bool {
        self.coeffs.iter().all(|c| c.is_finite())
    }
}

/// 实球谐基函数（l = 0..2），参数是单位方向。
///
/// 归一化约定与 Ramamoorthi & Hanrahan 一致：投影与求值用同一组基，
/// 自洽即可，不依赖外部约定。
pub fn sh_basis(dir: Vec3) -> [f32; SH_COEFFICIENTS] {
    let (x, y, z) = (dir.x, dir.y, dir.z);
    [
        0.282_094_8,
        0.488_602_8 * y,
        0.488_602_8 * z,
        0.488_602_8 * x,
        1.092_548_4 * x * y,
        1.092_548_4 * y * z,
        0.315_391_6 * (3.0 * z * z - 1.0),
        1.092_548_4 * x * z,
        0.546_274_2 * (x * x - y * y),
    ]
}

/// Fibonacci 球面均匀采样：第 `i` 个（共 `n` 个）方向。
///
/// 均匀性对球谐投影很关键——极点聚集的采样会让 l = 1 的系数偏斜，
/// 环境光就会莫名其妙地往某个方向偏色。
fn fibonacci_direction(i: usize, n: usize) -> Vec3 {
    let n_f = n as f32;
    let idx = i as f32;
    let y = 1.0 - 2.0 * (idx + 0.5) / n_f;
    let radius = (1.0 - y * y).max(0.0).sqrt();
    let golden_angle = PI * (3.0 - 5.0_f32.sqrt());
    let phi = golden_angle * idx;
    Vec3::new(phi.cos() * radius, y, phi.sin() * radius)
}

/// Karis 的解析环境 BRDF 近似（`EnvBRDFApprox`，SIGGRAPH 2013 Mobile）。
///
/// 返回 `(scale, bias)`，高光 IBL 亮度按 `F0 * scale + bias` 取值。
/// 它替代了预积分 BRDF LUT：省一张纹理和一次采样，代价是 `roughness → 0` 时
/// 正入射方向会过冲几个百分点（能量上界见单测）。
///
/// `ndv` 是 `n·v`，内部夹到 `1e-4..1`，避免掠射时 `exp2(-9.28 * 0)` 之外的分支
/// 与除零擦边。
pub fn env_brdf_approx(roughness: f32, ndv: f32) -> (f32, f32) {
    let r = if roughness.is_nan() {
        DEFAULT_ROUGHNESS
    } else {
        roughness.clamp(0.0, 1.0)
    };
    let ndv = if ndv.is_nan() {
        1.0
    } else {
        ndv.clamp(1e-4, 1.0)
    };
    // Karis 2013 的拟合常数，逐位照抄以保证与着色器一致
    const C0: [f32; 4] = [-1.0, -0.0275, -0.572, 0.022];
    const C1: [f32; 4] = [1.0, 0.0425, 1.04, -0.04];
    let a = r * C0[0] + C1[0];
    let b = r * C0[1] + C1[1];
    let c = r * C0[2] + C1[2];
    let d = r * C0[3] + C1[3];
    let a004 = (a * a).min((-9.28 * ndv).exp2()) * a + b;
    let scale = -1.04 * a004 + c;
    // 原始拟合在 roughness → 1 时会给出 -0.002 量级的 bias，环境反射亮度不可能为负，
    // 这里夹到 0；着色器侧逐字照抄同一个夹取，保证 CPU / GPU 完全一致。
    let bias = (1.04 * a004 + d).max(0.0);
    (scale, bias)
}

/// 粗糙表面的「主导反射方向」：把镜面反射方向朝法线拉，粗糙度越大越靠向法线。
///
/// GGX 的峰值随粗糙度向内偏，所以用 `roughness²` 插值；`roughness = 0` 得到
/// 纯反射方向（镜面），`roughness = 1` 得到法线本身（方向无关的环境）。
pub fn specular_dominant_dir(reflect_dir: Vec3, normal: Vec3, roughness: f32) -> Vec3 {
    let n = normalize_or(normal, Vec3::Y);
    let r = normalize_or(reflect_dir, n);
    let t = if roughness.is_nan() {
        DEFAULT_ROUGHNESS
    } else {
        roughness.clamp(0.0, 1.0)
    };
    normalize_or(r.lerp(n, t * t), n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::MIN_ROUGHNESS;

    fn constant_sky(level: f32) -> EnvironmentSettings {
        EnvironmentSettings {
            sky_zenith: [level, level, level],
            sky_horizon: [level, level, level],
            ground: [level, level, level],
            sun_intensity: 0.0,
            ..EnvironmentSettings::default()
        }
    }

    #[test]
    fn default_is_already_sanitized() {
        let d = EnvironmentSettings::default();
        let s = d.sanitized();
        assert!(d.is_default());
        // 参数逐个相等；方向只要求「归一化后仍在同一点」，f32 归一化会有末位误差
        assert_eq!(s.sky_zenith, d.sky_zenith);
        assert_eq!(s.sky_horizon, d.sky_horizon);
        assert_eq!(s.ground, d.ground);
        assert_eq!(s.sun_color, d.sun_color);
        assert_eq!(s.sun_intensity, d.sun_intensity);
        assert_eq!(s.intensity, d.intensity);
        let dir = Vec3::from(d.sun_dir);
        assert!(
            (dir.length() - 1.0).abs() < 1e-4,
            "缺省太阳方向应为单位向量"
        );
        assert!((Vec3::from(s.sun_dir) - dir).length() < 1e-5);
        // 缺省环境亮度与旧常数环境光同量级（0.16 ~ 0.23），切换后不会突变
        let avg = d.average_radiance();
        assert!(
            avg.x > 0.1 && avg.x < 0.4,
            "缺省平均辐亮度 {avg:?} 偏离旧 ambient 量级"
        );
    }

    #[test]
    fn sanitize_converges_bad_values() {
        let bad = EnvironmentSettings {
            sky_zenith: [f32::NAN, -3.0, 1e9],
            sky_horizon: [f32::INFINITY, f32::NEG_INFINITY, 0.5],
            ground: [-0.0, 0.0, 0.0],
            sun_dir: [0.0, 0.0, 0.0],
            sun_color: [f32::NAN, 1.0, -1.0],
            sun_intensity: f32::NAN,
            intensity: 1e9,
        }
        .sanitized();
        assert_eq!(bad.sky_zenith[0], 0.0);
        assert_eq!(bad.sky_zenith[1], 0.0);
        assert_eq!(bad.sky_zenith[2], MAX_ENV_RADIANCE);
        assert_eq!(bad.sky_horizon[0], MAX_ENV_RADIANCE);
        assert_eq!(bad.sky_horizon[1], 0.0);
        assert_eq!(bad.ground[0], 0.0);
        // 零向量太阳方向回退到正上方
        assert_eq!(bad.sun_dir, [0.0, 1.0, 0.0]);
        assert_eq!(bad.intensity, MAX_IBL_INTENSITY);
        // NaN 强度退回缺省而不是 0——缺省才是「正常天色」，0 会让太阳凭空消失
        assert_eq!(bad.sun_intensity, default_sun_intensity());
        assert!(bad.sun_color.iter().all(|c| c.is_finite() && *c >= 0.0));
    }

    #[test]
    fn constant_sky_irradiance_is_conserved() {
        // 常数天空 L = k 的辐照度解析解是 π·k（∫ L·cosθ dω = k·π）
        let k = 0.5_f32;
        let probe = Sh9::project(&constant_sky(k));
        for normal in [
            Vec3::Y,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.4, 0.5, -0.7).normalize(),
        ] {
            let e = probe.irradiance(normal);
            let expected = PI * k;
            assert!(
                (e.x - expected).abs() < 0.02 * expected,
                "期望 {expected}，实际 {e:?}"
            );
            assert!((e.y - e.x).abs() < 1e-5 && (e.z - e.x).abs() < 1e-5);
        }
    }

    #[test]
    fn irradiance_never_exceeds_pi_times_max_radiance() {
        let s = EnvironmentSettings::default().sanitized();
        let probe = Sh9::project(&s);
        let max_radiance = s
            .sky_zenith
            .iter()
            .chain(s.sky_horizon.iter())
            .chain(s.ground.iter())
            .copied()
            .fold(0.0_f32, f32::max)
            + s.sun_color.iter().copied().fold(0.0_f32, f32::max) * s.sun_intensity;
        let bound = PI * max_radiance;
        for i in 0..64 {
            let d = fibonacci_direction(i, 64);
            let e = probe.irradiance(d);
            assert!(e.x <= bound + 1e-3, "辐照度 {} 超过上界 {bound}", e.x);
            assert!(e.is_finite());
        }
    }

    #[test]
    fn sun_term_only_adds_light() {
        let base = EnvironmentSettings {
            sun_intensity: 0.0,
            ..EnvironmentSettings::default()
        };
        let lit = EnvironmentSettings {
            sun_intensity: 4.0,
            ..EnvironmentSettings::default()
        };
        for i in 0..32 {
            let d = fibonacci_direction(i, 32);
            let a = sky_radiance(d, &base);
            let b = sky_radiance(d, &lit);
            assert!(b.x >= a.x - 1e-6 && b.y >= a.y - 1e-6 && b.z >= a.z - 1e-6);
        }
        // 太阳方向上明显更亮
        let sun = Vec3::from(lit.sanitized().sun_dir);
        assert!(sky_radiance(sun, &lit).x > sky_radiance(sun, &base).x + 0.5);
    }

    #[test]
    fn sky_radiance_ignores_direction_length() {
        let s = EnvironmentSettings::default();
        let unit = Vec3::new(0.3, 0.7, -0.2).normalize();
        let scaled = unit * 37.0;
        let a = sky_radiance(unit, &s);
        let b = sky_radiance(scaled, &s);
        assert!((a - b).length() < 1e-5);
        // 零向量不 panic，回退到正上方
        let z = sky_radiance(Vec3::ZERO, &s);
        let up = sky_radiance(Vec3::Y, &s);
        assert!((z - up).length() < 1e-5);
    }

    #[test]
    fn blurred_sky_interpolates_towards_average() {
        let s = EnvironmentSettings::default();
        let dir = Vec3::new(0.2, 0.9, 0.3).normalize();
        let sharp = sky_radiance(dir, &s);
        let exact = sky_radiance_blurred(dir, 0.0, &s);
        assert!((sharp - exact).length() < 1e-4);
        let flat = sky_radiance_blurred(dir, 1.0, &s);
        assert!((flat - s.average_radiance()).length() < 1e-4);
        // 中间粗糙度落在两者之间（逐通道）
        let mid = sky_radiance_blurred(dir, 0.5, &s);
        for c in 0..3 {
            let lo = sharp[c].min(flat[c]) - 1e-4;
            let hi = sharp[c].max(flat[c]) + 1e-4;
            assert!(mid[c] >= lo && mid[c] <= hi);
        }
    }

    #[test]
    fn env_brdf_energy_stays_bounded() {
        // Karis 近似在 roughness → 0 且极端掠射时会过冲约 4%，这里按 1.05 抽样检查上界
        for f0 in [0.0_f32, 0.04, 0.5, 1.0] {
            for r in [0.0_f32, MIN_ROUGHNESS, 0.25, 0.6, 1.0] {
                for ndv in [0.001_f32, 0.05, 0.3, 0.7, 1.0] {
                    let (scale, bias) = env_brdf_approx(r, ndv);
                    let value = f0 * scale + bias;
                    assert!(
                        value.is_finite() && value <= 1.05,
                        "f0={f0} r={r} ndv={ndv} → {value}"
                    );
                    assert!(value >= 0.0, "f0={f0} r={r} ndv={ndv} → {value}");
                }
            }
        }
    }

    #[test]
    fn env_brdf_grazing_is_brighter() {
        // 菲涅尔在掠射方向必然抬升：中低粗糙度下 bias 单调变大
        for r in [0.045_f32, 0.1, 0.3, 0.6] {
            let (_, normal_bias) = env_brdf_approx(r, 1.0);
            let (_, grazing_bias) = env_brdf_approx(r, 0.02);
            assert!(grazing_bias > normal_bias, "r={r} 掠射应更亮");
        }
        // 粗糙度接近 1 时方向性已被抹平，两者应基本一致（这正是「全漫反射」的期望）
        for r in [0.9_f32, 1.0] {
            let (_, normal_bias) = env_brdf_approx(r, 1.0);
            let (_, grazing_bias) = env_brdf_approx(r, 0.02);
            assert!(
                (grazing_bias - normal_bias).abs() < 0.01,
                "r={r} 应已方向无关"
            );
        }
    }

    #[test]
    fn env_brdf_handles_degenerate_input() {
        let (s, b) = env_brdf_approx(f32::NAN, f32::NAN);
        assert!(s.is_finite() && b.is_finite());
        let (s2, b2) = env_brdf_approx(-5.0, 0.0);
        assert!(s2.is_finite() && b2.is_finite());
        let (s3, b3) = env_brdf_approx(9.0, 9.0);
        assert!(s3.is_finite() && b3.is_finite());
    }

    #[test]
    fn dominant_dir_moves_from_reflection_to_normal() {
        let n = Vec3::Y;
        let v = Vec3::new(0.0, 1.0, -1.0).normalize();
        let r = v - 2.0 * v.dot(n) * n; // 关于法线的镜面反射
        let at_mirror = specular_dominant_dir(r, n, 0.0);
        assert!((at_mirror - r).length() < 1e-5);
        let at_flat = specular_dominant_dir(r, n, 1.0);
        assert!((at_flat - n).length() < 1e-5);
        let mid = specular_dominant_dir(r, n, 0.5);
        assert!(mid.dot(n) > r.dot(n) && mid.dot(n) < 1.0);
        // 退化输入不产生 NaN
        let degenerate = specular_dominant_dir(Vec3::ZERO, Vec3::ZERO, 0.5);
        assert!(degenerate.is_finite());
    }

    #[test]
    fn sh_projections_are_finite_and_scalable() {
        let s = EnvironmentSettings::default();
        let probe = Sh9::project_with_samples(&s, 256);
        assert!(probe.is_finite());
        let doubled = probe.scaled(2.0);
        assert!((doubled.coeffs[0] - probe.coeffs[0] * 2.0).length() < 1e-6);
        // 强度收敛到上限，NaN 不影响
        assert!(probe.scaled(f32::NAN).is_finite());
        assert!(Sh9::default().is_finite());
    }

    #[test]
    fn diffuse_radiance_is_irradiance_over_pi() {
        let probe = Sh9::project(&EnvironmentSettings::default());
        let n = Vec3::new(0.2, 0.9, 0.1).normalize();
        let irr = probe.irradiance(n);
        let diff = probe.diffuse_radiance(n);
        assert!((irr / PI - diff).length() < 1e-6);
    }
}
