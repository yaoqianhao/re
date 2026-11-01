//! PBR 表面材质与直接光照 BRDF（CPU 参考实现）。
//!
//! 材质参数只有三个旋钮：`metallic`（金属度）、`roughness`（粗糙度）、
//! `emissive`（自发光）。base color 仍是物件颜色 × 材质染色因子，语义不变。
//!
//! 这里的函数是着色器 `shader.wgsl` 里同名逻辑的 CPU 版本，一一对应地实现
//! Cook-Torrance 三件套（GGX 法线分布 / Smith 遮蔽 / Schlick 菲涅尔），
//! 目的有两个：
//!
//! * 参数收敛（[`SurfaceMaterial::sanitized`]）在写入场景前就做完，不靠着色器兜底；
//! * 纯函数可单测：反射率区间、菲涅尔边界、能量不超发、退化参数不出 NaN。
//!
//! 该模块不依赖任何图形 API，`r3e-core` 仍然对 wgpu 零依赖。

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// 粗糙度缺省值（与 `MaterialData::default` 的 roughness 一致）。
pub const DEFAULT_ROUGHNESS: f32 = 0.6;

/// 粗糙度下限。GGX 的 α = roughness²，roughness 趋近 0 时 D 项分母趋近 0，
/// 直接光会出现除零 / 爆白；夹到 0.045 后 D 峰值有限。
pub const MIN_ROUGHNESS: f32 = 0.045;

/// 绝缘体基础反射率 F0（4%）。
pub const DIELECTRIC_F0: f32 = 0.04;

/// 自发光上限（线性 HDR 亮度）。只加不减，但要挡住 inf / 离谱数值。
pub const MAX_EMISSIVE: f32 = 16.0;

/// 圆周率（写死成 f32 常量，方便和 WGSL 侧逐字对齐）。
pub const PI: f32 = std::f32::consts::PI;

/// 物件级 PBR 材质参数。
///
/// 只描述「表面如何反射光」，不含颜色：base color 由物件颜色（乘材质染色因子）
/// 提供，因此一个材质可以被多个不同颜色的物件复用。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceMaterial {
    /// 金属度 0..1：0 = 绝缘体（塑料 / 石头），1 = 纯金属（漫反射归零）。
    #[serde(default)]
    pub metallic: f32,
    /// 粗糙度 0..1：0 = 镜面，1 = 完全漫反射。
    #[serde(
        default = "default_roughness",
        skip_serializing_if = "is_default_roughness"
    )]
    pub roughness: f32,
    /// 自发光（线性 RGB）：直接加到出射亮度上，不参与任何光照计算。
    #[serde(default, skip_serializing_if = "is_zero_emissive")]
    pub emissive: [f32; 3],
}

fn default_roughness() -> f32 {
    DEFAULT_ROUGHNESS
}

fn is_default_roughness(v: &f32) -> bool {
    *v == DEFAULT_ROUGHNESS
}

fn is_zero_emissive(v: &[f32; 3]) -> bool {
    v.iter().all(|c| *c == 0.0)
}

impl Default for SurfaceMaterial {
    fn default() -> Self {
        Self {
            metallic: 0.0,
            roughness: DEFAULT_ROUGHNESS,
            emissive: [0.0, 0.0, 0.0],
        }
    }
}

impl SurfaceMaterial {
    /// 新建一份材质（写入前请调用 [`Self::sanitized`]，或用 [`Self::new_checked`]）。
    pub fn new(metallic: f32, roughness: f32) -> Self {
        Self {
            metallic,
            roughness,
            emissive: [0.0, 0.0, 0.0],
        }
    }

    /// 新建并立即收敛：场景 / 脚本入口都用这条路径，非法值不进场景。
    pub fn new_checked(metallic: f32, roughness: f32) -> Self {
        Self::new(metallic, roughness).sanitized()
    }

    /// 设置自发光并收敛。
    pub fn with_emissive(mut self, emissive: [f32; 3]) -> Self {
        self.emissive = emissive;
        self.sanitized()
    }

    /// 参数收敛：NaN 退回缺省、其余夹到合法区间（±inf 由 `clamp` 处理）。
    ///
    /// 这是唯一的收敛入口，场景文件、场景 API、脚本三条写入路径都必须经过它，
    /// 保证「非法值在写入前消失」，而不是靠 GPU 侧夹取兜底。
    pub fn sanitized(self) -> Self {
        Self {
            metallic: sanitize_unit(self.metallic, 0.0),
            roughness: sanitize_unit(self.roughness, DEFAULT_ROUGHNESS),
            emissive: [
                sanitize_emissive(self.emissive[0]),
                sanitize_emissive(self.emissive[1]),
                sanitize_emissive(self.emissive[2]),
            ],
        }
    }

    /// 是否与缺省材质完全一致（场景保存时用来决定是否省略该字段）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 0..1 标量收敛：NaN → `fallback`，±inf 由 `clamp` 自动收到端点，
/// `-0.0` 归一成 `0.0`（避免 `to_bits` 做键时出现两个等价键）。
fn sanitize_unit(v: f32, fallback: f32) -> f32 {
    if v.is_nan() {
        return fallback;
    }
    let clamped = v.clamp(0.0, 1.0);
    if clamped == 0.0 {
        0.0
    } else {
        clamped
    }
}

/// 自发光分量收敛：NaN → 0，负值 → 0（只加不减），上限 [`MAX_EMISSIVE`]。
fn sanitize_emissive(v: f32) -> f32 {
    if v.is_nan() {
        return 0.0;
    }
    let clamped = v.clamp(0.0, MAX_EMISSIVE);
    if clamped == 0.0 {
        0.0
    } else {
        clamped
    }
}

/// 把任意粗糙度夹进 `[MIN_ROUGHNESS, 1]`（NaN 按缺省处理）。
pub fn clamp_roughness(roughness: f32) -> f32 {
    if roughness.is_nan() {
        return DEFAULT_ROUGHNESS;
    }
    roughness.clamp(MIN_ROUGHNESS, 1.0)
}

/// GGX 的 α 参数：α = roughness²（Disney / Filament 的感知粗糙度约定）。
pub fn ggx_alpha(roughness: f32) -> f32 {
    let r = clamp_roughness(roughness);
    r * r
}

/// GGX / Trowbridge-Reitz 法线分布 D(θh)。
///
/// `n_dot_h` 是法线与半程向量夹角的余弦。因为 α² > 0，分母
/// `(n·h)²(α²-1)+1` 恒 ≥ α²，不会出现除零。
pub fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = ggx_alpha(roughness);
    let a2 = a * a;
    let ndh = n_dot_h.clamp(0.0, 1.0);
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d)
}

/// Schlick-GGX 的单方向遮蔽项 G1（直接光形式，k = (r+1)²/8）。
fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let r = clamp_roughness(roughness);
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    let nx = n_dot_x.clamp(0.0, 1.0);
    nx / (nx * (1.0 - k) + k)
}

/// Smith 遮蔽项：视线方向与光线方向两个 G1 相乘。
pub fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness)
}

/// Schlick 菲涅尔近似：正入射返回 F0，掠射趋近 1。
pub fn fresnel_schlick(cos_theta: f32, f0: Vec3) -> Vec3 {
    let c = (1.0 - cos_theta).clamp(0.0, 1.0);
    f0 + (Vec3::ONE - f0) * c.powi(5)
}

/// 材质的基础反射率 F0：绝缘体 0.04，金属用 base color 染色。
pub fn fresnel_f0(metallic: f32, base_color: Vec3) -> Vec3 {
    let m = if metallic.is_nan() {
        0.0
    } else {
        metallic.clamp(0.0, 1.0)
    };
    Vec3::splat(DIELECTRIC_F0).lerp(base_color, m)
}

/// 一次 BRDF 求值的结果，分为漫反射与镜面两半（两者都未乘 `n·l`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrdfSample {
    pub diffuse: Vec3,
    pub specular: Vec3,
}

impl BrdfSample {
    /// 总 BRDF 值（每单位入射辐照度的出射辐亮度，尚未乘 `n·l`）。
    pub fn total(&self) -> Vec3 {
        self.diffuse + self.specular
    }

    /// 乘上 `n·l` 之后的反射率贡献——半球积分与能量检查都看这个量。
    pub fn reflected(&self, n_dot_l: f32) -> Vec3 {
        self.total() * n_dot_l.max(0.0)
    }
}

/// Cook-Torrance 直接光照 BRDF。
///
/// * 镜面：`D·G·F / (4·(n·v)·(n·l))`
/// * 漫反射：能量守恒形式 `(1-metallic)·(1-F)·albedo/π`
///   —— `1-metallic` 让金属没有漫反射，`1-F` 扣掉镜面已经拿走的份额。
///
/// 返回的是「每单位入射辐照度」的 BRDF 值，`n·l` 由调用方乘（见
/// [`BrdfSample::reflected`] 与 [`direct_light`]）。`n·l ≤ 0` 时返回全零（背面不收光）。
/// 一次着色用到的四个余弦项。
///
/// 打包传入而不是平铺成四个参数：`cook_torrance_brdf` / `direct_light` 的参数
/// 列表本来就已经够长，再平铺两个就触到 clippy 的阈值，也不利于对照着色器。
/// 四项都是「未夹取的原始点积」，夹取在函数内部各自完成，NaN 在入口统一剔除。
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GeoTerms {
    /// n·l：法线与光方向。
    pub n_dot_l: f32,
    /// n·v：法线与视线。
    pub n_dot_v: f32,
    /// n·h：法线与半程向量。
    pub n_dot_h: f32,
    /// v·h：视线与半程向量。
    pub v_dot_h: f32,
}

impl GeoTerms {
    /// 显式构造。
    pub const fn new(n_dot_l: f32, n_dot_v: f32, n_dot_h: f32, v_dot_h: f32) -> Self {
        Self {
            n_dot_l,
            n_dot_v,
            n_dot_h,
            v_dot_h,
        }
    }

    /// 四项同值：正入射这类对称情形与单测图省事时用。
    pub const fn all(v: f32) -> Self {
        Self::new(v, v, v, v)
    }
}

pub fn cook_torrance_brdf(
    base_color: Vec3,
    material: SurfaceMaterial,
    geo: GeoTerms,
) -> BrdfSample {
    let zero = BrdfSample {
        diffuse: Vec3::ZERO,
        specular: Vec3::ZERO,
    };
    let ndl = geo.n_dot_l.clamp(0.0, 1.0);
    // 背光直接早退；NaN 也走这条分支，避免脏值流进后面的除法
    if !ndl.is_finite() || ndl <= 0.0 {
        return zero;
    }
    // n·v 理论上可以到 0（掠射），留一个极小正数避免除零；
    // 掠射时 G1 本身也趋于 0，所以不会因此爆亮。
    let ndv = geo.n_dot_v.clamp(0.0, 1.0).max(1e-4);
    let ndh = geo.n_dot_h.clamp(0.0, 1.0);
    let vdh = geo.v_dot_h.clamp(0.0, 1.0);

    let mat = material.sanitized();
    // 反照率为负会把光"减掉"，统一按 0 处理
    let albedo = base_color.max(Vec3::ZERO);

    let f0 = fresnel_f0(mat.metallic, albedo);
    let f = fresnel_schlick(vdh, f0);
    let d = distribution_ggx(ndh, mat.roughness);
    let g = geometry_smith(ndv, ndl, mat.roughness);

    let specular = f * (d * g / (4.0 * ndv * ndl));
    let kd = (Vec3::ONE - f) * (1.0 - mat.metallic);
    let diffuse = kd * albedo / PI;

    BrdfSample { diffuse, specular }
}

/// 单方向光的出射辐亮度：`(漫反射 + 镜面) · 光色 · 强度 · (n·l) · 阴影`。
///
/// 光色 / 强度语义与改造前完全一致：`light_color` 是线性 RGB，`intensity` 是乘数。
pub fn direct_light(
    base_color: Vec3,
    material: SurfaceMaterial,
    geo: GeoTerms,
    light_color: Vec3,
    intensity: f32,
    shadow: f32,
) -> Vec3 {
    let ndl = geo.n_dot_l.clamp(0.0, 1.0);
    let brdf = cook_torrance_brdf(base_color, material, geo);
    brdf.reflected(ndl) * light_color * intensity.max(0.0) * shadow.clamp(0.0, 1.0)
}

/// 自发光贡献：原样加到出射亮度，不乘 `n·l`、不受阴影影响、不参与光照。
pub fn emissive_radiance(material: SurfaceMaterial) -> Vec3 {
    let m = material.sanitized();
    Vec3::new(m.emissive[0], m.emissive[1], m.emissive[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    /// 场景 / 脚本写入前必须已经收敛：NaN、±inf、超界值都被夹回合法区间。
    #[test]
    fn sanitize_clamps_every_illegal_input() {
        let m = SurfaceMaterial {
            metallic: f32::NAN,
            roughness: f32::INFINITY,
            emissive: [f32::NAN, -3.0, 1e9],
        }
        .sanitized();
        assert_eq!(m.metallic, 0.0, "metallic 的 NaN 退回 0");
        assert_eq!(m.roughness, 1.0, "roughness 的 +inf 收到 1");
        assert_eq!(m.emissive, [0.0, 0.0, MAX_EMISSIVE]);

        let m = SurfaceMaterial {
            metallic: -2.0,
            roughness: f32::NEG_INFINITY,
            emissive: [0.5, f32::INFINITY, 0.0],
        }
        .sanitized();
        assert_eq!(m.metallic, 0.0);
        assert_eq!(
            m.roughness, 0.0,
            "roughness 的 -inf 收到 0（渲染侧再有下限）"
        );
        assert_eq!(m.emissive, [0.5, MAX_EMISSIVE, 0.0]);

        // 全部合法值原样保留
        let m = SurfaceMaterial::new(0.25, 0.75).with_emissive([1.0, 2.0, 3.0]);
        assert_eq!(
            m,
            SurfaceMaterial {
                metallic: 0.25,
                roughness: 0.75,
                emissive: [1.0, 2.0, 3.0],
            }
        );
    }

    /// NaN 参与比较永远为 false，收敛函数必须显式吃掉它（否则会污染整个着色结果）。
    #[test]
    fn sanitize_output_is_always_finite_and_ordered() {
        let probes = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0,
            0.0,
            0.5,
            1.0,
            2.0,
            f32::MIN,
            f32::MAX,
        ];
        for &v in &probes {
            let m = SurfaceMaterial {
                metallic: v,
                roughness: v,
                emissive: [v, v, v],
            }
            .sanitized();
            assert!(m.metallic.is_finite() && (0.0..=1.0).contains(&m.metallic));
            assert!(m.roughness.is_finite() && (0.0..=1.0).contains(&m.roughness));
            for c in m.emissive {
                assert!(c.is_finite() && (0.0..=MAX_EMISSIVE).contains(&c));
            }
        }
    }

    /// 菲涅尔边界：正入射等于 F0，掠射收敛到 1（白色）。
    #[test]
    fn fresnel_boundaries_are_f0_and_one() {
        let f0 = Vec3::splat(DIELECTRIC_F0);
        let head_on = fresnel_schlick(1.0, f0);
        assert!(
            approx(head_on.x, DIELECTRIC_F0, 1e-6),
            "正入射 = {head_on:?}"
        );

        let grazing = fresnel_schlick(0.0, f0);
        assert!(approx(grazing.x, 1.0, 1e-6), "掠射 = {grazing:?}");

        // 单调：夹角余弦越小（越掠射），反射越强
        let mut prev = 0.0;
        for i in 0..=10 {
            let c = 1.0 - i as f32 / 10.0;
            let v = fresnel_schlick(c, f0).x;
            assert!(v >= prev - 1e-6, "c={c} 时反射率非单调");
            prev = v;
        }
    }

    /// F0：绝缘体恒为 4%，金属取 base color（100% 金属纯白时 F0 = 1）。
    #[test]
    fn f0_blends_between_dielectric_and_base_color() {
        let base = Vec3::new(1.0, 0.8, 0.2);
        let dielectric = fresnel_f0(0.0, base);
        assert!(approx(dielectric.x, DIELECTRIC_F0, 1e-6));
        assert!(approx(dielectric.y, DIELECTRIC_F0, 1e-6));

        let metal = fresnel_f0(1.0, base);
        assert!(approx(metal.x, 1.0, 1e-6) && approx(metal.y, 0.8, 1e-6));
    }

    /// 金属度 = 1 时漫反射必须归零（能量全走镜面分支）。
    #[test]
    fn metallic_one_kills_diffuse() {
        let mat = SurfaceMaterial::new(1.0, 0.3);
        let s = cook_torrance_brdf(Vec3::splat(1.0), mat, GeoTerms::all(1.0));
        assert!(s.diffuse.length() < 1e-9, "diffuse = {:?}", s.diffuse);
        assert!(s.specular.length() > 0.1, "镜面分支应当有能量");

        let mat = SurfaceMaterial::new(0.0, 0.3);
        let s = cook_torrance_brdf(Vec3::splat(1.0), mat, GeoTerms::all(1.0));
        assert!(s.diffuse.length() > 0.2, "绝缘体应当有漫反射");
    }

    /// 退化参数（roughness = 0、n·v = 0、半程向量与法线垂直）不得产生 NaN / inf。
    #[test]
    fn degenerate_inputs_stay_finite() {
        let cases = [
            (0.0f32, 0.0f32, 0.0f32, 0.0f32),
            (1.0, 0.0, 0.0, 0.0),
            (1e-9, 1e-9, 0.0, 1.0),
            (-1.0, -1.0, -1.0, -1.0),
            (f32::NAN, f32::NAN, f32::NAN, f32::NAN),
        ];
        for (nl, nv, nh, vh) in cases {
            for roughness in [0.0, 1e-6, MIN_ROUGHNESS, 0.5, 1.0, f32::NAN] {
                let mat = SurfaceMaterial::new(0.5, roughness);
                let s = cook_torrance_brdf(Vec3::splat(0.7), mat, GeoTerms::new(nl, nv, nh, vh));
                assert!(
                    s.diffuse.is_finite(),
                    "diffuse={:?} case={:?}",
                    s.diffuse,
                    (nl, nv, nh, vh)
                );
                assert!(
                    s.specular.is_finite(),
                    "spec={:?} r={roughness}",
                    s.specular
                );
                assert!(s.diffuse.min_element() >= 0.0 && s.specular.min_element() >= 0.0);
            }
        }

        // 直接传入未夹取的 roughness = 0，D 项也必须有限（内部走 clamp_roughness）
        let d = distribution_ggx(1.0, 0.0);
        assert!(d.is_finite() && d > 0.0, "D(0) = {d}");
        let d = distribution_ggx(0.0, 0.0);
        assert!(d.is_finite(), "D(ndh=0) = {d}");
    }

    /// 白炉测试：均匀环境（L=1）下的半球反射率不得超发（≤ 1）。
    ///
    /// 单散射 GGX 会因缺多次散射而略低于 1，但无论如何不能超过 1。
    #[test]
    fn white_furnace_reflectance_never_exceeds_one() {
        const N: usize = 24;
        const M: usize = 48;
        let d_theta = std::f32::consts::FRAC_PI_2 / N as f32;
        let d_phi = (2.0 * std::f32::consts::PI) / M as f32;

        for roughness in [0.045, 0.1, 0.3, 0.5, 0.8, 1.0] {
            for metallic in [0.0, 0.5, 1.0] {
                let mat = SurfaceMaterial::new(metallic, roughness);
                let albedo = Vec3::splat(1.0);
                let mut total = Vec3::ZERO;
                for i in 0..N {
                    let theta = (i as f32 + 0.5) * d_theta;
                    let (sin_t, cos_t) = theta.sin_cos();
                    for j in 0..M {
                        let phi = (j as f32 + 0.5) * d_phi;
                        let l = Vec3::new(sin_t * phi.cos(), sin_t * phi.sin(), cos_t);
                        let v = Vec3::Z;
                        let h = (l + v).normalize();
                        let s = cook_torrance_brdf(
                            albedo,
                            mat,
                            GeoTerms::new(cos_t, 1.0, h.z, v.dot(h)),
                        );
                        let dw = sin_t * d_theta * d_phi;
                        total += s.reflected(cos_t) * dw;
                    }
                }
                assert!(
                    total.x <= 1.0 + 1e-3,
                    "roughness={roughness} metallic={metallic} 反射率超发: {total:?}"
                );
                assert!(
                    total.x >= 0.0 && total.x.is_finite(),
                    "roughness={roughness} 反射率异常: {total:?}"
                );
            }
        }
    }

    /// 反射率的合理区间：完全漫反射的绝缘体，半球积分反射率约等于 albedo（±10%）。
    ///
    /// 漫反射 BRDF 是 `kd·albedo/π`，乘 π 才是「半球反射率」这个物理量；
    /// 单点求值看不出量级，这里同时校验 BRDF 数值与积分后的反射率。
    #[test]
    fn rough_dielectric_reflects_like_albedo() {
        let mat = SurfaceMaterial::new(0.0, 1.0);
        let albedo = Vec3::splat(0.8);
        let s = cook_torrance_brdf(albedo, mat, GeoTerms::all(1.0));
        // 单点 BRDF：漫反射项 = (1-F0)·albedo/π ≈ 0.2445
        assert!(
            (s.diffuse.x - (1.0 - DIELECTRIC_F0) * 0.8 / PI).abs() < 1e-5,
            "漫反射 BRDF 数值不符: {:?}",
            s.diffuse
        );
        // 半球反射率 = BRDF·π（漫反射占主导），应接近 kd·albedo ≈ 0.768
        let reflectance = s.total().x * PI;
        assert!(
            (reflectance - 0.8).abs() < 0.08,
            "白炉反射率应接近 albedo，实际 {reflectance}"
        );
        assert!(s.reflected(1.0).x > 0.0);
    }

    /// 直接光的量级：光强 1、阴影 1、正入射时不超过光色本身。
    #[test]
    fn direct_light_is_bounded_by_light_color() {
        let mat = SurfaceMaterial::new(0.0, 0.5);
        let light = Vec3::new(1.0, 0.95, 0.9);
        let lit = direct_light(Vec3::splat(0.6), mat, GeoTerms::all(1.0), light, 1.0, 1.0);
        for (c, l) in lit.to_array().iter().zip(light.to_array()) {
            assert!(*c >= 0.0 && *c <= l + 1e-4, "直接光超发: {lit:?}");
        }
        // 阴影为 0 时直接光贡献为 0
        let dark = direct_light(Vec3::splat(0.6), mat, GeoTerms::all(1.0), light, 1.0, 0.0);
        assert_eq!(dark, Vec3::ZERO);
        // 背面（n·l = 0）不收光
        let back = direct_light(
            Vec3::splat(0.6),
            mat,
            GeoTerms::new(0.0, 1.0, 1.0, 1.0),
            light,
            1.0,
            1.0,
        );
        assert_eq!(back, Vec3::ZERO);
    }

    /// 自发光只加不减：与直接光相加后必然不小于直接光本身。
    #[test]
    fn emissive_only_adds() {
        let mat = SurfaceMaterial::new(0.2, 0.4).with_emissive([0.5, 0.25, 0.0]);
        let emissive = emissive_radiance(mat);
        assert_eq!(emissive, Vec3::new(0.5, 0.25, 0.0));

        let light = Vec3::splat(1.0);
        let lit = direct_light(Vec3::splat(0.5), mat, GeoTerms::all(1.0), light, 1.0, 1.0);
        let total = lit + emissive;
        for i in 0..3 {
            assert!(total[i] >= lit[i], "emissive 不应减少亮度");
        }

        // 无光 / 背光时自发光依然可见（不参与光照）
        let unlit = direct_light(
            Vec3::ZERO,
            mat,
            GeoTerms::new(0.0, 1.0, 1.0, 1.0),
            Vec3::ZERO,
            0.0,
            0.0,
        );
        assert_eq!(unlit, Vec3::ZERO);
        assert!(emissive.length() > 0.0);
    }

    /// 粗糙度越大，镜面高光越散（同角度下峰值单调下降）。
    #[test]
    fn rougher_surface_has_lower_specular_peak() {
        let mut prev = f32::INFINITY;
        for roughness in [0.045, 0.1, 0.3, 0.6, 1.0] {
            let d = distribution_ggx(1.0, roughness);
            assert!(d < prev, "roughness={roughness} 的 D 峰值未下降");
            prev = d;
        }
    }
}
