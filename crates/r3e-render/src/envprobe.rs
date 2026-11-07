//! HDR 立方体反射探针（扩展二十三）的渲染侧纯逻辑。
//!
//! 解码 / 重采样 / 预滤波 / 球谐投影全在 [`r3e_core::envmap`]（纯算术、可单测，
//! 不依赖图形 API）；这里只做「把 CPU 探针搬上 GPU」这一件事：
//!
//! * **像素编码**：`PrefilteredEnv` 的每层每面是 `Vec<[f32; 3]>`，要转成 GPU 的
//!   `Rgba16Float` 字节流。f32 → f16 的转换自己实现（一次 `to_bits` 位运算），
//!   免得为一行代码引入 `half` 依赖。
//! * **mip 选择**：与着色器 `probe_specular` 用同一条映射（`roughness × (层数-1)`），
//!   两边都锁在契约测试里，防止 CPU 统计与 GPU 采样各算各的。
//! * **统计**：进 [`crate::RenderStats::env_probe`]，供探针日志与 HUD 观测。
//!
//! 本模块**不引用 wgpu**：所有 wgpu 类型留在 `renderer.rs`，这样这些逻辑在没有
//! 图形设备的环境里也能单测。

use r3e_core::envmap::{EnvironmentMapSettings, PrefilteredEnv};

/// 立方体面数（+X, -X, +Y, -Y, +Z, -Z）。与 `r3e_core::envmap::CUBE_FACES` 同值，
/// 但在此重新声明一次，导入时不必走 core 的私有导入路径。
pub const CUBE_FACES: usize = r3e_core::envmap::CUBE_FACES;

/// 探针立方体纹理在材质绑定组里的 binding 编号（`group(1)`，紧接 AO 贴图的 5）。
///
/// 为什么挂在 group(1) 而不是新开一组：WebGPU core 的 `max_bind_groups = 4`，
/// `0..=3` 已被相机 / 材质 / 关节 / 阴影占满，新开组会直接超出设备上限。
pub const ENV_CUBE_BINDING: u32 = 6;

/// 立方体纹理的边长下限（像素）。与 `r3e_core::envmap::MAX_CUBE_FACE_SIZE` 无关，
/// 这里只用于兜底纹理：没有探针时绑一张 1×1 全黑的立方体，保证 bind group 永远填满。
pub const DUMMY_CUBE_FACE_SIZE: u32 = 1;

/// 一帧的探针状态（进 [`crate::RenderStats::env_probe`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvProbeStats {
    /// 本帧是否真的在用 HDR 探针。`false` = 走扩展十二的解析 IBL，画面逐像素不变。
    pub active: bool,
    /// 探针来源标签：`"none"` / `"equirect"` / `"faces"`，与场景段
    /// [`EnvironmentMapSettings::source_label`] 同口径。
    pub source: &'static str,
    /// 基准面边长（像素）。0 表示没有探针。
    pub face_size: u32,
    /// 镜面 mip 层数。0 表示没有探针。
    pub mip_levels: u32,
    /// 探针总强度（只乘在镜面反射项上）。
    pub intensity: f32,
}

impl Default for EnvProbeStats {
    fn default() -> Self {
        Self {
            active: false,
            source: "none",
            face_size: 0,
            mip_levels: 0,
            intensity: 0.0,
        }
    }
}

impl EnvProbeStats {
    /// 是否等价于「没配探针」。日志侧据此决定要不要打印探针摘要。
    pub fn is_idle(&self) -> bool {
        !self.active
    }
}

/// 待上传的探针：CPU 预滤波结果 + 生效强度 + 来源标签。
///
/// 由应用层（资产加载处）构造后交给 [`crate::Renderer::set_env_probe`]；
/// 渲染层只认这个结构，不认识文件 IO，也不认识 `.hdr` 格式。
#[derive(Debug, Clone, PartialEq)]
pub struct EnvProbeSource {
    prefiltered: PrefilteredEnv,
    intensity: f32,
    source: &'static str,
}

impl EnvProbeSource {
    /// 用预滤波结果 + 场景段配置构造。`settings.intensity` 已被 `sanitized` 夹紧。
    pub fn from_settings(prefiltered: PrefilteredEnv, settings: &EnvironmentMapSettings) -> Self {
        Self {
            prefiltered,
            intensity: if settings.intensity.is_finite() {
                settings.intensity
            } else {
                1.0
            },
            source: settings.source_label(),
        }
    }

    /// 直接给定强度与来源标签（测试与非场景来源用）。
    pub fn new(prefiltered: PrefilteredEnv, intensity: f32, source: &'static str) -> Self {
        Self {
            prefiltered,
            intensity: if intensity.is_finite() {
                intensity
            } else {
                1.0
            },
            source,
        }
    }

    pub fn prefiltered(&self) -> &PrefilteredEnv {
        &self.prefiltered
    }

    pub fn intensity(&self) -> f32 {
        self.intensity
    }

    pub fn source(&self) -> &'static str {
        self.source
    }

    pub fn stats(&self) -> EnvProbeStats {
        EnvProbeStats {
            active: true,
            source: self.source,
            face_size: self.prefiltered.base().face_size() as u32,
            mip_levels: self.prefiltered.mip_levels(),
            intensity: self.intensity,
        }
    }
}

/// 一层一面待上传的像素块。
#[derive(Debug, Clone, PartialEq)]
pub struct EnvProbeFaceUpload {
    /// mip 层号（0 = 清晰层）。
    pub mip: u32,
    /// 立方体面号，顺序同 [`r3e_core::envmap::CUBE_FACE_NAMES`]（+X, -X, +Y, -Y, +Z, -Z）。
    pub face: u32,
    /// 该层该面的边长（像素）。
    pub size: u32,
    /// `Rgba16Float` 像素流：每像素 8 字节（RGB = 线性辐亮度，A = 1）。
    /// 行距无 padding（`bytes_per_row = size * 8`），`write_texture` 直接吃。
    pub bytes: Vec<u8>,
}

impl EnvProbeFaceUpload {
    /// 每行字节数（`write_texture` 的 `bytes_per_row`）。
    pub fn bytes_per_row(&self) -> u32 {
        self.size * 8
    }
}

/// 把整个预滤波 mip 链摊成「逐层逐面」的上传清单，顺序为 `mip 外层、face 内层`。
///
/// 之所以摊平而不是让调用方二层循环：上传路径要按 `(mip, face)` 逐块 `write_texture`，
/// 摊平后清单本身就是上传顺序，测试也能直接比对条数与尺寸递减。
pub fn encode_faces(prefiltered: &PrefilteredEnv) -> Vec<EnvProbeFaceUpload> {
    let mut out = Vec::with_capacity(prefiltered.mip_levels() as usize * CUBE_FACES);
    for (level, cubemap) in prefiltered.mips().iter().enumerate() {
        for face in 0..CUBE_FACES {
            let image = cubemap.face(face);
            let size = image.width() as u32;
            let mut bytes = Vec::with_capacity(image.pixels().len() * 8);
            for pixel in image.pixels() {
                bytes.extend_from_slice(&f32_to_f16(pixel[0]).to_le_bytes());
                bytes.extend_from_slice(&f32_to_f16(pixel[1]).to_le_bytes());
                bytes.extend_from_slice(&f32_to_f16(pixel[2]).to_le_bytes());
                // A = 1.0：探针本身不透明，采样后透明度不参与任何计算
                bytes.extend_from_slice(&f32_to_f16(1.0).to_le_bytes());
            }
            out.push(EnvProbeFaceUpload {
                mip: level as u32,
                face: face as u32,
                size,
                bytes,
            });
        }
    }
    out
}

/// 按粗糙度取 mip 层号，与着色器 `probe_specular` 的 `lod` 计算逐位一致。
///
/// 映射 = `clamp(roughness, 0, 1) × (mip_levels - 1)`：层数只有 1 时恒为 0。
/// 返回浮点值，GPU 会在相邻两层间做三线性插值，因此粗糙度过渡是连续的。
pub fn mip_lod(roughness: f32, mip_levels: u32) -> f32 {
    let levels = mip_levels.max(1);
    if levels == 1 {
        return 0.0;
    }
    let r = if roughness.is_nan() {
        1.0
    } else {
        roughness.clamp(0.0, 1.0)
    };
    (r * (levels - 1) as f32).clamp(0.0, (levels - 1) as f32)
}

/// f32 → IEEE 754 binary16 位模式（round-to-nearest-even）。
///
/// 自实现而非引依赖：只需要这一个函数，且行为要能被单测逐位钉住。
/// 关键分支：
/// * 溢出（`|x| ≥ 65520`）→ `±inf`，不做饱和 —— 探针是 HDR 数据，饱和会把
///   太阳盘的峰值压扁，反而失真；
/// * 次正规与下溢按 IEEE 逐位舍入，`< 2^-25` 直接落 0；
/// * NaN 保留为非零尾数的 NaN（`0x7e00 | 尾数`），不会被压成 inf。
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x007f_ffff;

    if exp == 0xff {
        // Inf / NaN
        if man == 0 {
            return sign | 0x7c00;
        }
        // 保留高 10 位尾数；全 0 时补一个最低位，确保结果仍是 NaN
        let payload = ((man >> 13) as u16) | 0x0200;
        return sign | 0x7c00 | (payload & 0x03ff);
    }

    // 目标指数：-14 是 f16 的最小正规指数
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            // |x| < 2^-25，舍入到 0
            return sign;
        }
        // 次正规：把隐含位补回来后右移
        let m = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let mut v = m >> shift;
        let rem = m & ((1u32 << shift) - 1);
        if rem > half || (rem == half && (v & 1) == 1) {
            v += 1;
        }
        return sign | v as u16;
    }

    let mut v = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    // 进位溢到指数位时结果正好是「进到下一档」，无需特殊处理
    if rem > 0x1000 || (rem == 0x1000 && (v & 1) == 1) {
        v += 1;
    }
    sign | v as u16
}

/// f16 位模式 → f32（仅用于测试与诊断回读，正确性是 [`f32_to_f16`] 的对照）。
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let man = (bits & 0x03ff) as u32;
    if exp == 0 {
        if man == 0 {
            return f32::from_bits(sign);
        }
        // 次正规：手动规格化
        let mut man = man;
        let mut e: i32 = -1;
        while man & 0x0400 == 0 {
            man <<= 1;
            e -= 1;
        }
        let man = (man & 0x03ff) << 13;
        let exp = ((127 - 15 + e + 1) as u32) << 23;
        return f32::from_bits(sign | exp | man);
    }
    if exp == 0x1f {
        return f32::from_bits(sign | 0x7f80_0000 | (man << 13));
    }
    let exp = (exp + (127 - 15)) << 23;
    f32::from_bits(sign | exp | (man << 13))
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::envmap::{prefilter_specular, Cubemap, HdrImage};

    fn constant_cubemap(size: usize, rgb: [f32; 3]) -> Cubemap {
        let faces = (0..CUBE_FACES)
            .map(|_| HdrImage::constant(size, size, rgb).expect("常量面构造"))
            .collect::<Vec<_>>();
        Cubemap::new(size, faces).expect("六面齐全")
    }

    #[test]
    fn f16_round_trip_keeps_common_values() {
        for value in [
            0.0_f32, 1.0, -1.0, 0.5, 0.25, 2.0, 4.0, -8.0, 100.0, 1000.0, 1.5, 0.1,
        ] {
            let round = f16_to_f32(f32_to_f16(value));
            assert!(
                (round - value).abs() <= value.abs().max(1.0) * 1e-3,
                "{value} → {round} 超出 f16 精度"
            );
        }
        // 0.5 是精确可表示值，必须逐位相等
        assert_eq!(f16_to_f32(f32_to_f16(0.5)), 0.5);
        assert_eq!(f32_to_f16(0.0), 0x0000);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-1.0), 0xbc00);
    }

    #[test]
    fn f16_saturates_to_inf_and_keeps_nan() {
        assert_eq!(f32_to_f16(70000.0), 0x7c00, "溢出必须是 +inf 而不是饱和值");
        assert_eq!(f32_to_f16(-70000.0), 0xfc00);
        let nan = f32_to_f16(f32::NAN);
        assert!(nan & 0x7c00 == 0x7c00, "NaN 指数位必须全 1");
        assert!(nan & 0x03ff != 0, "NaN 尾数必须非零，否则退化成 inf");
        assert_eq!(f32_to_f16(1e-9), 0, "极小值舍入到 0");
    }

    #[test]
    fn f16_subnormal_boundary() {
        // 最小正规 f16 = 2^-14；最小次正规 = 2^-24
        assert_eq!(f32_to_f16(2f32.powi(-14)), 0x0400);
        assert_eq!(f32_to_f16(2f32.powi(-24)), 0x0001);
        // 半途值 2^-25 就近舍到偶数 → 0
        assert_eq!(f32_to_f16(2f32.powi(-25)), 0x0000);
    }

    #[test]
    fn encode_faces_covers_every_mip_and_face() {
        let base = constant_cubemap(8, [4.0, 2.0, 1.0]);
        let probe = prefilter_specular(&base, 3);
        let uploads = encode_faces(&probe);
        assert_eq!(uploads.len(), 3 * CUBE_FACES);
        // 每层 6 面齐全，且层号 / 面号严格按 mip 外层、face 内层递增
        for (i, item) in uploads.iter().enumerate() {
            assert_eq!(item.mip, (i / CUBE_FACES) as u32);
            assert_eq!(item.face, (i % CUBE_FACES) as u32);
        }
        // 尺寸逐层减半，字节数 = size² × 8
        let sizes: Vec<u32> = uploads
            .iter()
            .filter(|item| item.face == 0)
            .map(|item| item.size)
            .collect();
        assert_eq!(sizes, vec![8, 4, 2]);
        for item in &uploads {
            assert_eq!(item.bytes.len() as u32, item.size * item.size * 8);
            assert_eq!(item.bytes_per_row(), item.size * 8);
        }
        // 常量面编码后逐像素等于原值（4.0 / 2.0 / 1.0 在 f16 里都精确）
        let first = &uploads[0];
        assert_eq!(
            f16_to_f32(u16::from_le_bytes([first.bytes[0], first.bytes[1]])),
            4.0
        );
        assert_eq!(
            f16_to_f32(u16::from_le_bytes([first.bytes[2], first.bytes[3]])),
            2.0
        );
        assert_eq!(
            f16_to_f32(u16::from_le_bytes([first.bytes[4], first.bytes[5]])),
            1.0
        );
        assert_eq!(
            f16_to_f32(u16::from_le_bytes([first.bytes[6], first.bytes[7]])),
            1.0
        );
    }

    #[test]
    fn mip_lod_matches_layer_layout() {
        assert_eq!(mip_lod(0.0, 6), 0.0);
        assert_eq!(mip_lod(1.0, 6), 5.0);
        assert!((mip_lod(0.5, 6) - 2.5).abs() < 1e-6);
        assert_eq!(mip_lod(0.5, 1), 0.0, "单层链只能取 0 层");
        // 越界与 NaN 都收敛到合法区间，绝不让 GPU 读到不存在的层
        assert_eq!(mip_lod(9.0, 6), 5.0);
        assert_eq!(mip_lod(-9.0, 6), 0.0);
        assert!((0.0..=5.0).contains(&mip_lod(f32::NAN, 6)));
        // 与 core 的粗糙度↔层号映射互为逆运算
        for level in 0..6u32 {
            let r = PrefilteredEnv::roughness_of(level, 6);
            assert!((mip_lod(r, 6) - level as f32).abs() < 1e-5);
        }
    }

    #[test]
    fn source_stats_reflect_settings() {
        let base = constant_cubemap(8, [1.0, 1.0, 1.0]);
        let probe = prefilter_specular(&base, 4);
        let settings = EnvironmentMapSettings::with_equirect("sky.hdr");
        let source = EnvProbeSource::from_settings(probe, &settings);
        let stats = source.stats();
        assert!(stats.active);
        assert!(!stats.is_idle());
        assert_eq!(stats.source, "equirect");
        assert_eq!(stats.face_size, 8);
        assert_eq!(stats.mip_levels, 4);
        assert_eq!(stats.intensity, 1.0);

        // 关掉开关的配置不该被当成激活来源
        let disabled = EnvironmentMapSettings {
            enabled: false,
            ..settings
        };
        assert_eq!(disabled.source_label(), "disabled");
    }

    #[test]
    fn idle_stats_are_the_fallback_baseline() {
        let stats = EnvProbeStats::default();
        assert!(stats.is_idle());
        assert_eq!(stats.source, "none");
        assert_eq!(stats.face_size, 0);
        assert_eq!(stats.mip_levels, 0);
    }
}
