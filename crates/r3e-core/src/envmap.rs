//! HDR 环境图（等距柱状 / 立方体六面）到预滤波反射探针的 CPU 实现。
//!
//! 扩展十二给引擎装上了「SH 9 系数 + 解析天空」的轻量 IBL：不需要 cubemap，
//! 但高频反射（光滑金属里能看到的天空轮廓、窗户、地面层次）被彻底抹平。
//! 本模块补上高清反射所需的三件事，且全部是纯 CPU、零 wgpu 依赖：
//!
//! * [`decode_radiance`]：Radiance `.hdr`（RGBE / 新旧两种扫描行编码）解码为
//!   [`HdrImage`]（线性 f32，未做色调映射）；
//! * [`equirect_to_cubemap`]：等距柱状图按方向重采样成六面立方体图，
//!   六面顺序与 WebGPU / DirectX 一致（+X, -X, +Y, -Y, +Z, -Z）；
//! * [`prefilter_specular`]：按粗糙度分层做余弦幂卷积，生成镜面 mip 链；
//!   [`mip_for_roughness`] 给出「粗糙度 → mip」的映射，shader 侧逐位照抄同一公式；
//! * [`Cubemap::project_irradiance`]：带立体角权重的球谐投影，给出与
//!   [`crate::env::Sh9`] 完全同构的漫反射辐照度系数。
//!
//! 约定（与 `env.rs` 一致）：颜色是线性辐亮度，不做 sRGB 解码、不做曝光；
//! 方向一律世界空间单位向量，+Y 朝上；面内 `u` 向右、`v` 向下（图像行方向）。

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::env::{sh_basis, Sh9, SH_COEFFICIENTS};
use crate::material::DEFAULT_ROUGHNESS;

/// 立方体面数。
pub const CUBE_FACES: usize = 6;

/// 单面边长上限（像素）。2048² × 6 面 × 4 字节 ≈ 100 MB，这里刻意压到 1024。
pub const MAX_CUBE_FACE_SIZE: usize = 1024;

/// 默认单面边长。128 面再往上对镜面反射的可见收益快速衰减，但 CPU 预滤波成本线性涨。
pub const DEFAULT_CUBE_FACE_SIZE: usize = 128;

/// 镜面 mip 链默认层数：6 层 = 128, 64, 32, 16, 8, 4。
pub const DEFAULT_SPECULAR_MIPS: u32 = 6;

/// 镜面 mip 链层数上限（GPU 上限内，且保证最小面 1×1 时有意义）。
pub const MAX_SPECULAR_MIPS: u32 = 10;

/// 预滤波时每像素的余弦幂重要采样数。层越小采样数不变，总成本仍随面积 4 倍递减。
pub const PREFILTER_SAMPLES: usize = 32;

/// 环境图尺寸上限（宽 × 高），挡住恶意超大分辨率把内存吃光。
pub const MAX_EQUIRECT_PIXELS: usize = 64 * 1024 * 1024;

/// RGBE 解码的指数偏置：`value = mantissa * 2^(e - 136)`。
const RGBE_BIAS: f32 = 136.0;

/// 线性 f32 的等距柱状环境图。
#[derive(Debug, Clone, PartialEq)]
pub struct HdrImage {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
}

impl HdrImage {
    /// 由 RGB 浮点像素构造。维度为 0、像素数与 `width * height` 不符时返回 `None`。
    ///
    /// 非有限分量与负值就地收敛为 0：HDR 文件里偶发 NaN 会让整条 mip 链变黑，
    /// 在入口一次过滤比事后每层检查便宜得多。
    pub fn new(width: usize, height: usize, mut pixels: Vec<[f32; 3]>) -> Option<Self> {
        if width == 0 || height == 0 || width.checked_mul(height)? != pixels.len() {
            return None;
        }
        if width.checked_mul(height)? > MAX_EQUIRECT_PIXELS {
            return None;
        }
        for px in pixels.iter_mut() {
            for c in px.iter_mut() {
                if !c.is_finite() || *c < 0.0 {
                    *c = 0.0;
                }
            }
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// 纯色图（测试与占位用）。
    pub fn constant(width: usize, height: usize, rgb: [f32; 3]) -> Option<Self> {
        let n = width.checked_mul(height)?;
        Self::new(width, height, vec![rgb; n])
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        self.pixels[y * self.width + x]
    }

    /// 双线性 + 水平环绕采样等距柱状图。
    ///
    /// `dir` 不必归一化。方向约定：`+Y` 对应图像顶部（`v = 0`），
    /// `atan2(x, -z)` 决定经度——与 [`cube_face_dir`] 的面朝向自洽，`+Z` 落在 u = 0.5。
    pub fn sample_dir(&self, dir: Vec3) -> Vec3 {
        let (u, v) = equirect_uv(dir);
        self.sample_uv(u, v)
    }

    /// 按 `[0,1]²` 归一化 uv 双线性采样（u 环绕，v 夹取）。
    pub fn sample_uv(&self, u: f32, v: f32) -> Vec3 {
        let fx = u * self.width as f32 - 0.5;
        let fy = v * self.height as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let x0 = x0 as i64;
        let y0 = y0 as i64;
        let x1 = x0 + 1;
        let y1 = y0 + 1;
        let wrap_x = |x: i64| -> usize {
            let w = self.width as i64;
            (((x % w) + w) % w) as usize
        };
        let clamp_y = |y: i64| -> usize { y.clamp(0, self.height as i64 - 1) as usize };
        let p00 = Vec3::from(self.pixels[clamp_y(y0) * self.width + wrap_x(x0)]);
        let p10 = Vec3::from(self.pixels[clamp_y(y0) * self.width + wrap_x(x1)]);
        let p01 = Vec3::from(self.pixels[clamp_y(y1) * self.width + wrap_x(x0)]);
        let p11 = Vec3::from(self.pixels[clamp_y(y1) * self.width + wrap_x(x1)]);
        let top = p00.lerp(p10, tx);
        let bottom = p01.lerp(p11, tx);
        top.lerp(bottom, ty)
    }

    /// 立体角加权（等距柱状图权重 `sinθ`）的平均辐亮度。
    pub fn average_radiance(&self) -> Vec3 {
        let mut acc = Vec3::ZERO;
        let mut weight = 0.0_f32;
        for y in 0..self.height {
            // 行中心纬度：v ∈ [0,1] → θ ∈ [0, π]
            let theta = (y as f32 + 0.5) / self.height as f32 * std::f32::consts::PI;
            let w = theta.sin().max(1e-6);
            for x in 0..self.width {
                acc += Vec3::from(self.pixels[y * self.width + x]) * w;
                weight += w;
            }
        }
        if weight <= 0.0 {
            Vec3::ZERO
        } else {
            acc / weight
        }
    }
}

/// 等距柱状图方向 → 归一化 uv。`+Y` 在顶部，`+Z` 落在 u = 0.5。
pub fn equirect_uv(dir: Vec3) -> (f32, f32) {
    let d = if dir.is_finite() && dir.length_squared() > 1e-12 {
        dir.normalize()
    } else {
        Vec3::Y
    };
    let v = d.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
    let u = 0.5 + d.x.atan2(d.z) / (2.0 * std::f32::consts::PI);
    (u, v)
}

/// 立方体某面内的归一化 uv（`u` 向右、`v` 向下，均 ∈ `[0,1]`）→ 方向。
///
/// 面顺序与 WebGPU / DirectX 一致：0 = +X，1 = -X，2 = +Y，3 = -Y，4 = +Z，5 = -Z。
/// 返回值未归一化的可能性为零：每个面的映射本身即单位球上的点，但重采样时
/// 调用方通常会再归一化一次，这里直接给出单位向量避免两处约定漂移。
pub fn cube_face_dir(face: usize, u: f32, v: f32) -> Vec3 {
    // 面内坐标转到 [-1, 1]
    let s = 2.0 * u - 1.0;
    let t = 2.0 * v - 1.0;
    let dir = match face % CUBE_FACES {
        0 => Vec3::new(1.0, -t, -s),
        1 => Vec3::new(-1.0, -t, s),
        2 => Vec3::new(s, 1.0, t),
        3 => Vec3::new(s, -1.0, -t),
        4 => Vec3::new(s, -t, 1.0),
        _ => Vec3::new(-s, -t, -1.0),
    };
    dir.normalize()
}

/// 方向 → 所属面与该面内的归一化 uv（[`cube_face_dir`] 的逆映射）。
pub fn cube_dir_face(dir: Vec3) -> (usize, f32, f32) {
    let d = if dir.is_finite() && dir.length_squared() > 1e-12 {
        dir.normalize()
    } else {
        Vec3::Y
    };
    let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
    let (face, sc, tc) = if ax >= ay && ax >= az {
        if d.x > 0.0 {
            (0, -d.z / ax, -d.y / ax)
        } else {
            (1, d.z / ax, -d.y / ax)
        }
    } else if ay >= az {
        if d.y > 0.0 {
            (2, d.x / ay, d.z / ay)
        } else {
            (3, d.x / ay, -d.z / ay)
        }
    } else if d.z > 0.0 {
        (4, d.x / az, -d.y / az)
    } else {
        (5, -d.x / az, -d.y / az)
    };
    (face, (sc + 1.0) * 0.5, (tc + 1.0) * 0.5)
}

/// 六面立方体环境图（每面同边长的 HdrImage）。
#[derive(Debug, Clone, PartialEq)]
pub struct Cubemap {
    face_size: usize,
    faces: Vec<HdrImage>,
}

impl Cubemap {
    /// 由六面构造。面数不为 6、边长不是 2 的幂、或某面尺寸不符时返回 `None`。
    ///
    /// 边长要求 2 的幂是为了让 mip 链每一层都精确减半（3, 5 之类的边长会在
    /// 第 2 层出现 1×1 与 2×2 混排，GPU 采样会取到错位的 mip）。
    pub fn new(face_size: usize, faces: Vec<HdrImage>) -> Option<Self> {
        if faces.len() != CUBE_FACES {
            return None;
        }
        if face_size == 0 || face_size > MAX_CUBE_FACE_SIZE || !face_size.is_power_of_two() {
            return None;
        }
        if faces
            .iter()
            .any(|f| f.width() != face_size || f.height() != face_size)
        {
            return None;
        }
        Some(Self { face_size, faces })
    }

    pub fn face_size(&self) -> usize {
        self.face_size
    }

    pub fn face(&self, index: usize) -> &HdrImage {
        &self.faces[index % CUBE_FACES]
    }

    pub fn faces(&self) -> &[HdrImage] {
        &self.faces
    }

    /// 按方向双线性采样（跨面边界时退化为面内夹取，避免 6 面拼接处的重复采样开销）。
    pub fn sample_dir(&self, dir: Vec3) -> Vec3 {
        let (face, u, v) = cube_dir_face(dir);
        self.sample_face_uv(face, u, v)
    }

    /// 指定面内归一化 uv 采样。
    pub fn sample_face_uv(&self, face: usize, u: f32, v: f32) -> Vec3 {
        let img = self.face(face);
        let size = self.face_size as f32;
        let fx = u.clamp(0.0, 1.0) * size - 0.5;
        let fy = v.clamp(0.0, 1.0) * size - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let x0 = x0 as i64;
        let y0 = y0 as i64;
        let cx = |x: i64| -> usize { x.clamp(0, self.face_size as i64 - 1) as usize };
        let p00 = Vec3::from(img.pixel(cx(x0), cx(y0)));
        let p10 = Vec3::from(img.pixel(cx(x0 + 1), cx(y0)));
        let p01 = Vec3::from(img.pixel(cx(x0), cx(y0 + 1)));
        let p11 = Vec3::from(img.pixel(cx(x0 + 1), cx(y0 + 1)));
        p00.lerp(p10, tx).lerp(p01.lerp(p11, tx), ty)
    }

    /// 按立体角加权求平均辐亮度（用于「彻底模糊」的兜底项与单测上界）。
    pub fn average_radiance(&self) -> Vec3 {
        let mut acc = Vec3::ZERO;
        let mut weight = 0.0_f32;
        let size = self.face_size;
        let inv = 2.0 / size as f32;
        for face in 0..CUBE_FACES {
            let img = self.face(face);
            for y in 0..size {
                for x in 0..size {
                    let u = (x as f32 + 0.5) * inv - 1.0;
                    let v = (y as f32 + 0.5) * inv - 1.0;
                    let solid = inv * inv / (1.0 + u * u + v * v).powf(1.5);
                    acc += Vec3::from(img.pixel(x, y)) * solid;
                    weight += solid;
                }
            }
        }
        if weight <= 0.0 {
            Vec3::ZERO
        } else {
            acc / weight
        }
    }

    /// 带立体角权重的球谐投影，得到与程序化天空完全同构的漫反射辐照度系数。
    ///
    /// 权重 `dω = du·dv / (1 + u² + v²)^{3/2}` 是立方体面的精确立体角微元，
    /// 因此常量图的 `coeffs[0]` 应等于 `L × 0.2820948 × 4π`，`irradiance` 应等于 `L × π`
    /// （见单测）。相比在球面撒 Fibonacci 点，这里直接复用已有像素，无重采样噪声。
    pub fn project_irradiance(&self) -> Sh9 {
        let mut acc = [Vec3::ZERO; SH_COEFFICIENTS];
        let size = self.face_size;
        let inv = 2.0 / size as f32;
        let duv = inv * inv;
        for face in 0..CUBE_FACES {
            for y in 0..size {
                for x in 0..size {
                    let u = (x as f32 + 0.5) * inv - 1.0;
                    let v = (y as f32 + 0.5) * inv - 1.0;
                    let dir = cube_face_dir(face, (u + 1.0) * 0.5, (v + 1.0) * 0.5);
                    let solid = duv / (1.0 + u * u + v * v).powf(1.5);
                    let radiance = Vec3::from(self.faces[face].pixel(x, y));
                    let basis = sh_basis(dir);
                    for (slot, b) in acc.iter_mut().zip(basis.iter()) {
                        *slot += radiance * (*b * solid);
                    }
                }
            }
        }
        Sh9 { coeffs: acc }
    }

    /// 写进 GPU 的 RGBA 布局：逐面、逐 mip 层、逐行、逐像素 4 个 f32（a = 1）。
    ///
    /// 渲染层拿到的是「一层内的全部面连续排列」，可直接按层 `write_texture`，
    /// 也可整块上传——额外的 alpha 通道在这里统一补齐，避免上层两处各自约定。
    pub fn face_rgba(&self, face: usize) -> Vec<f32> {
        let img = self.face(face);
        let mut out = Vec::with_capacity(self.face_size * self.face_size * 4);
        for px in img.pixels() {
            out.extend_from_slice(&[px[0], px[1], px[2], 1.0]);
        }
        out
    }
}

/// 立方体六面的名字（WebGPU / DirectX 顺序）。日志与错误信息直接用它。
pub const CUBE_FACE_NAMES: [&str; CUBE_FACES] = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"];

/// 场景 `environment_map` 段（扩展二十三）：HDR 立方体反射探针的资产声明与旋钮。
///
/// 未配置（`Scene::environment_map == None`）、`enabled = false`、或者两种来源都没写
/// 路径时，[`EnvironmentMapSettings::is_active`] 为 false，渲染层完全沿用扩展十二的
/// 解析 IBL（三色天空 + SH 9 系数），画面逐像素不变。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EnvironmentMapSettings {
    /// 是否启用。缺省 true——只要写了 `equirect` / `faces` 就生效；
    /// 想临时关掉探针不必删路径，翻这个开关即可（也方便 A/B 对比）。
    #[serde(default = "default_probe_enabled")]
    pub enabled: bool,
    /// 等距柱状 HDR 图路径（`.hdr`，Radiance RGBE）。相对路径以场景文件所在目录为基准，
    /// 由应用层负责解析成绝对路径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equirect: Option<String>,
    /// 立方体六面路径，顺序固定为 [+X, -X, +Y, -Y, +Z, -Z]（见 [`CUBE_FACE_NAMES`]）。
    /// 与 `equirect` 二选一；两者都写时 `faces` 优先——省掉一次重采样，保真度更高。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<Vec<String>>,
    /// 预滤波基准面边长（像素，收敛到 `[4, MAX_CUBE_FACE_SIZE]` 的 2 的幂）。
    /// 减小它只降低显存与预滤波耗时，不影响画面「有没有反射」。
    #[serde(default = "default_probe_face_size")]
    pub face_size: usize,
    /// 镜面 mip 链层数（`1..=MAX_SPECULAR_MIPS`）。层数越多，粗糙度过渡越细腻。
    #[serde(default = "default_specular_mips")]
    pub mip_levels: u32,
    /// 探针总强度：只乘在环境反射项上，`1.0` 为物理值（与 `EnvironmentSettings::intensity`
    /// 同语义，但两者互不影响，可分别调解析天空与 HDR 探针）。
    #[serde(default = "default_probe_intensity")]
    pub intensity: f32,
}

fn default_probe_enabled() -> bool {
    true
}

fn default_probe_face_size() -> usize {
    DEFAULT_CUBE_FACE_SIZE
}

fn default_specular_mips() -> u32 {
    DEFAULT_SPECULAR_MIPS
}

fn default_probe_intensity() -> f32 {
    1.0
}

impl Default for EnvironmentMapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            equirect: None,
            faces: None,
            face_size: DEFAULT_CUBE_FACE_SIZE,
            mip_levels: DEFAULT_SPECULAR_MIPS,
            intensity: 1.0,
        }
    }
}

impl EnvironmentMapSettings {
    /// 以等距柱状图构造（脚本 `set_env_map` 的落点）。
    pub fn with_equirect(path: impl Into<String>) -> Self {
        Self {
            equirect: Some(path.into()),
            ..Self::default()
        }
    }

    /// 以立方体六面构造。长度不是 6 时同样受理，交给 [`Self::sanitized`] 判为无效来源。
    pub fn with_faces(paths: impl IntoIterator<Item = String>) -> Self {
        Self {
            faces: Some(paths.into_iter().collect()),
            ..Self::default()
        }
    }

    /// 路径收敛：去首尾空白，空串视为「没写」。
    ///
    /// 六面路径必须恰好 6 条且条条非空，否则整段 `faces` 作废——半套面拼不出 cubemap，
    /// 与其猜缺哪一面，不如让它明确失效并退回解析 IBL。
    pub fn sanitized(mut self) -> Self {
        self.equirect = self.equirect.as_deref().and_then(sanitize_probe_path);
        self.faces = self.faces.and_then(|faces| {
            if faces.len() != CUBE_FACES {
                return None;
            }
            let mut out = Vec::with_capacity(CUBE_FACES);
            for path in faces.iter() {
                out.push(sanitize_probe_path(path)?);
            }
            Some(out)
        });
        self.face_size = sanitize_face_size(self.face_size);
        self.mip_levels = self.mip_levels.clamp(1, MAX_SPECULAR_MIPS);
        self.intensity = if self.intensity.is_nan() {
            default_probe_intensity()
        } else {
            self.intensity.clamp(0.0, crate::env::MAX_IBL_INTENSITY)
        };
        self
    }

    /// 本帧是否真的启用 HDR 探针：开关打开且至少有一个可用来源。
    pub fn is_active(&self) -> bool {
        self.enabled && (self.faces.is_some() || self.equirect.is_some())
    }

    /// 与缺省完全一致（场景保存时决定是否省略该段）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 实际生效的来源名，供探针日志使用。
    pub fn source_label(&self) -> &'static str {
        match (self.enabled, self.faces.is_some(), self.equirect.is_some()) {
            (false, _, _) => "disabled",
            (true, true, _) => "faces",
            (true, false, true) => "equirect",
            (true, false, false) => "none",
        }
    }
}

fn sanitize_probe_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 等距柱状 HDR 图重采样成立方体图。
///
/// `face_size` 会被收敛到 `[4, MAX_CUBE_FACE_SIZE]` 且向上取到 2 的幂；
/// 采样采用方向驱动的双线性（[`HdrImage::sample_dir`]），不做超采样——
/// 基础层交给 GPU 的 trilinear + mip 处理更划算。
pub fn equirect_to_cubemap(image: &HdrImage, face_size: usize) -> Cubemap {
    let size = sanitize_face_size(face_size);
    let inv = 2.0 / size as f32;
    let mut faces = Vec::with_capacity(CUBE_FACES);
    for face in 0..CUBE_FACES {
        let mut pixels = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let u = (x as f32 + 0.5) * inv - 1.0;
                let v = (y as f32 + 0.5) * inv - 1.0;
                let dir = cube_face_dir(face, (u + 1.0) * 0.5, (v + 1.0) * 0.5);
                let c = image.sample_dir(dir);
                pixels.push([c.x, c.y, c.z]);
            }
        }
        // 尺寸与像素数必然自洽，unwrap 不可达；仍用 expect 保留失败语义
        faces.push(HdrImage::new(size, size, pixels).expect("cubemap face 尺寸与像素数一致"));
    }
    Cubemap::new(size, faces).expect("尺寸已收敛为合法 2 的幂、面数恒为 6")
}

fn sanitize_face_size(face_size: usize) -> usize {
    let clamped = face_size.clamp(4, MAX_CUBE_FACE_SIZE);
    clamped.next_power_of_two().min(MAX_CUBE_FACE_SIZE)
}

/// 预滤波后的环境探针：`mips[0]` 是清晰原图，其后每层对应更大粗糙度。
#[derive(Debug, Clone, PartialEq)]
pub struct PrefilteredEnv {
    mips: Vec<Cubemap>,
}

impl PrefilteredEnv {
    /// mip 层数。
    pub fn mip_levels(&self) -> u32 {
        self.mips.len() as u32
    }

    /// 清晰层（`roughness = 0`）。
    pub fn base(&self) -> &Cubemap {
        &self.mips[0]
    }

    /// 指定层的立方体图；越界时夹到最后一层。
    pub fn mip(&self, level: u32) -> &Cubemap {
        let idx = (level as usize).min(self.mips.len() - 1);
        &self.mips[idx]
    }

    /// 全部层。
    pub fn mips(&self) -> &[Cubemap] {
        &self.mips
    }

    /// 第 `level` 层对应的粗糙度（`mip_for_roughness` 的逆）。
    pub fn roughness_of(level: u32, mip_levels: u32) -> f32 {
        if mip_levels <= 1 {
            return 0.0;
        }
        (level as f32 / (mip_levels - 1) as f32).clamp(0.0, 1.0)
    }

    /// 按粗糙度取（浮点）mip 层号，供 GPU `textureSampleLevel` 做三线性过滤。
    pub fn mip_for_roughness(roughness: f32, mip_levels: u32) -> f32 {
        if mip_levels <= 1 {
            return 0.0;
        }
        let r = if roughness.is_nan() {
            DEFAULT_ROUGHNESS
        } else {
            roughness.clamp(0.0, 1.0)
        };
        r * (mip_levels - 1) as f32
    }
}

/// 由清晰立方体图生成本模块的探针（含清晰层）。
///
/// `mip_levels` 会被收敛到 `1..=MAX_SPECULAR_MIPS`，且不超过「面长能减半的次数 + 1」，
/// 保证每层至少 1 像素。第 0 层直接复用输入，不再卷积（保清晰反射）。
pub fn prefilter_specular(base: &Cubemap, mip_levels: u32) -> PrefilteredEnv {
    let max_by_size = base.face_size().trailing_zeros() + 1;
    let levels = mip_levels.clamp(1, MAX_SPECULAR_MIPS).min(max_by_size);
    let mut mips = Vec::with_capacity(levels as usize);
    mips.push(base.clone());
    for level in 1..levels {
        let roughness = PrefilteredEnv::roughness_of(level, levels);
        let size = (base.face_size() >> level).max(1).next_power_of_two();
        mips.push(prefilter_level(base, size, roughness));
    }
    PrefilteredEnv { mips }
}

/// 单独一层：每个像素用余弦幂重要采样卷积原图。
///
/// 权重指数由 GGX 的 `α = r²` 反解成 Phong 指数 `k = 2/α² - 2`，
/// 即「同样粗糙度的 GGX 旁瓣宽度」。`r = 0` 时直接拷贝原图（重要采样退化成单点）。
fn prefilter_level(base: &Cubemap, size: usize, roughness: f32) -> Cubemap {
    let inv = 2.0 / size as f32;
    let alpha = (roughness * roughness).max(1e-4);
    // k = 2/α² - 2；α → 1（r = 1）时 k → 0，退化为余弦加权半球
    let exponent = (2.0 / (alpha * alpha) - 2.0).max(0.0);
    // 余弦幂重要采样：θ = acos(u1^(1/(k+1)))
    let theta_power = 1.0 / (exponent + 1.0);
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    let mut faces = Vec::with_capacity(CUBE_FACES);
    for face in 0..CUBE_FACES {
        let mut pixels = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let u = (x as f32 + 0.5) * inv - 1.0;
                let v = (y as f32 + 0.5) * inv - 1.0;
                let n = cube_face_dir(face, (u + 1.0) * 0.5, (v + 1.0) * 0.5);
                let (tangent, bitangent) = tangent_frame(n);
                let mut acc = Vec3::ZERO;
                for i in 0..PREFILTER_SAMPLES {
                    // 分层抖动：两层错开取样，避免同一 φ 上全落在格点造成条纹
                    let u1 = (i as f32 + 0.5) / PREFILTER_SAMPLES as f32;
                    let u2 = ((i as f32 * golden) / (2.0 * std::f32::consts::PI)
                        + 0.5 / PREFILTER_SAMPLES as f32)
                        .fract();
                    let theta = u1.powf(theta_power).clamp(0.0, 1.0).acos();
                    let phi = 2.0 * std::f32::consts::PI * u2;
                    let l = tangent * (theta.sin() * phi.cos())
                        + bitangent * (theta.sin() * phi.sin())
                        + n * theta.cos();
                    acc += base.sample_dir(l);
                }
                let c = acc / PREFILTER_SAMPLES as f32;
                pixels.push([c.x, c.y, c.z]);
            }
        }
        faces.push(HdrImage::new(size, size, pixels).expect("预滤波层尺寸与像素数一致"));
    }
    Cubemap::new(size, faces).expect("预滤波层尺寸为 2 的幂且面数恒为 6")
}

/// 绕法线构造正交基：优先用 +Y 做参考，接近极点时换 +X，避免叉积退化。
fn tangent_frame(n: Vec3) -> (Vec3, Vec3) {
    let up = if n.y.abs() < 0.99 { Vec3::Y } else { Vec3::X };
    let tangent = up.cross(n).normalize();
    let bitangent = n.cross(tangent).normalize();
    (tangent, bitangent)
}

/// 把已解码的 HDR 图直接做成探针：重采样 + 预滤波一步到位。
pub fn build_probe(image: &HdrImage, face_size: usize, mip_levels: u32) -> PrefilteredEnv {
    let base = equirect_to_cubemap(image, face_size);
    prefilter_specular(&base, mip_levels)
}

/// Radiance `.hdr`（RGBE）解码。支持 `32-bit_rle_rgbe` 新旧两种扫描行编码。
///
/// 不支持旧式 `32-bit_rle_xyze`（XYZ 色彩空间）与老版 `pixelsize` 头：
/// 前者需要额外色域转换矩阵，后者在真实资产里几乎绝迹，遇到一律返回 `None`
/// 由上层降级到解析 IBL，绝不静默给出错误颜色。
pub fn decode_radiance(bytes: &[u8]) -> Option<HdrImage> {
    let (header_end, format_rgbe) = parse_header(bytes)?;
    if !format_rgbe {
        return None;
    }
    let (width, height, flip_x, flip_y, mut pos) = parse_resolution(bytes, header_end)?;
    if width.checked_mul(height)? > MAX_EQUIRECT_PIXELS || width == 0 || height == 0 {
        return None;
    }
    let mut pixels = vec![[0.0_f32; 3]; width * height];
    let mut scanline = vec![[0u8; 4]; width];
    for row in 0..height {
        decode_scanline(bytes, &mut pos, width, &mut scanline)?;
        let dst_row = if flip_y { height - 1 - row } else { row };
        for (col, rgbe) in scanline.iter().enumerate() {
            let dst_col = if flip_x { width - 1 - col } else { col };
            pixels[dst_row * width + dst_col] = rgbe_to_linear(*rgbe);
        }
    }
    HdrImage::new(width, height, pixels)
}

/// 解析头：返回「分辨率行起始偏移」与「是否 32-bit_rle_rgbe」。
fn parse_header(bytes: &[u8]) -> Option<(usize, bool)> {
    if !bytes.starts_with(b"#?") {
        return None;
    }
    let mut pos = 0;
    let mut format_rgbe = false;
    while pos < bytes.len() {
        let line_end = bytes[pos..].iter().position(|b| *b == b'\n')? + pos;
        let raw = &bytes[pos..line_end];
        let line = std::str::from_utf8(raw).ok()?.trim_end_matches('\r');
        if line.is_empty() {
            return Some((line_end + 1, format_rgbe));
        }
        if let Some(value) = line.strip_prefix("FORMAT=") {
            let value = value.trim();
            if value != "32-bit_rle_rgbe" {
                return Some((line_end + 1, false));
            }
            format_rgbe = true;
        }
        pos = line_end + 1;
    }
    None
}

/// 解析分辨率行（如 `-Y 256 +X 512`），返回宽高与是否需翻转。
fn parse_resolution(bytes: &[u8], start: usize) -> Option<(usize, usize, bool, bool, usize)> {
    let line_end = bytes[start..].iter().position(|b| *b == b'\n')? + start;
    let line = std::str::from_utf8(&bytes[start..line_end])
        .ok()?
        .trim_end_matches('\r');
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 4 {
        return None;
    }
    let axis0 = parts[0];
    let len0: usize = parts[1].parse().ok()?;
    let axis1 = parts[2];
    let len1: usize = parts[3].parse().ok()?;
    if len0 == 0 || len1 == 0 {
        return None;
    }
    // 第一个轴必须是 Y（Radiance 规范要求），其长度即高度
    let vertical = axis0.to_ascii_uppercase().ends_with('Y');
    let (height, width) = if vertical { (len0, len1) } else { (len1, len0) };
    let (v_axis, h_axis) = if vertical {
        (axis0, axis1)
    } else {
        (axis1, axis0)
    };
    let flip_y = v_axis.starts_with('+');
    let flip_x = h_axis.starts_with('-');
    Some((width, height, flip_x, flip_y, line_end + 1))
}

/// 解码一条扫描行：自动识别新式 RLE（`2 2 hi lo` 前缀）与旧式平面数据。
fn decode_scanline(bytes: &[u8], pos: &mut usize, width: usize, out: &mut [[u8; 4]]) -> Option<()> {
    let head = bytes.get(*pos..*pos + 4)?;
    let is_rle = (8..32768).contains(&width) && head[0] == 2 && head[1] == 2 && head[2] & 0x80 == 0;
    if !is_rle {
        // 旧式：直接 width 个 RGBE 四元组
        for px in out.iter_mut().take(width) {
            let chunk = bytes.get(*pos..*pos + 4)?;
            *px = [chunk[0], chunk[1], chunk[2], chunk[3]];
            *pos += 4;
        }
        return Some(());
    }
    let hi = bytes[*pos + 2] as usize;
    let lo = bytes[*pos + 3] as usize;
    if (hi << 8 | lo) != width {
        return None;
    }
    *pos += 4;
    for ch in 0..4 {
        let mut x = 0;
        while x < width {
            let code = *bytes.get(*pos)?;
            *pos += 1;
            if code > 128 {
                let count = (code - 128) as usize;
                let value = *bytes.get(*pos)?;
                *pos += 1;
                if count == 0 || x + count > width {
                    return None;
                }
                for px in out.iter_mut().skip(x).take(count) {
                    px[ch] = value;
                }
                x += count;
            } else {
                let count = code as usize;
                if count == 0 || x + count > width {
                    return None;
                }
                let chunk = bytes.get(*pos..*pos + count)?;
                *pos += count;
                for (px, value) in out.iter_mut().skip(x).zip(chunk.iter()) {
                    px[ch] = *value;
                }
                x += count;
            }
        }
    }
    Some(())
}

/// RGBE 四元组 → 线性浮点 RGB：`value = mantissa × 2^(e - 136)`。
fn rgbe_to_linear(rgbe: [u8; 4]) -> [f32; 3] {
    if rgbe[3] == 0 {
        return [0.0; 3];
    }
    let scale = (rgbe[3] as f32 - RGBE_BIAS).exp2();
    [
        (rgbe[0] as f32) * scale,
        (rgbe[1] as f32) * scale,
        (rgbe[2] as f32) * scale,
    ]
}

/// RGB 浮点 → RGBE 四元组（仅测试与夹具生成用）。
#[doc(hidden)]
pub fn linear_to_rgbe(rgb: [f32; 3]) -> [u8; 4] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    if max <= 1e-32 {
        return [0, 0, 0, 0];
    }
    // frexp 等价实现：max = f × 2^e，f ∈ [0.5, 1)
    let e = max.log2().floor() as i32 + 1;
    let scale = 2.0_f32.powi(-e) * 256.0;
    let quant = |v: f32| -> u8 { (v * scale).clamp(0.0, 255.0) as u8 };
    [
        quant(rgb[0]),
        quant(rgb[1]),
        quant(rgb[2]),
        (e + 128).clamp(0, 255) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::MIN_ROUGHNESS;

    fn gray(v: f32, size: usize) -> Cubemap {
        let faces = (0..CUBE_FACES)
            .map(|_| HdrImage::constant(size, size, [v, v, v]).unwrap())
            .collect();
        Cubemap::new(size, faces).unwrap()
    }

    /// 由行文本拼一个最小 Radiance 文件。
    fn radiance_file(resolution: &str, scanlines: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n");
        out.extend_from_slice(resolution.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(scanlines);
        out
    }

    #[test]
    fn rgbe_roundtrip_preserves_radiance() {
        for value in [0.0_f32, 0.001, 0.18, 0.5, 1.0, 4.0, 64.0, 1200.0] {
            let rgbe = linear_to_rgbe([value, value * 0.5, value * 0.25]);
            let back = rgbe_to_linear(rgbe);
            assert!(
                (back[0] - value).abs() <= value * 0.01 + 1e-6,
                "{value} → {back:?}"
            );
            assert!(back[1] <= back[0] + 1e-6 && back[2] <= back[1] + 1e-6);
        }
        // 全黑必须编码成 e = 0，解码回 0
        assert_eq!(linear_to_rgbe([0.0; 3]), [0, 0, 0, 0]);
        assert_eq!(rgbe_to_linear([0, 0, 0, 0]), [0.0; 3]);
    }

    #[test]
    fn decode_radiance_flat_scanlines() {
        // 宽 2（< 8）走老式平面编码，逐像素 RGBE
        let mut scan = Vec::new();
        for _ in 0..4 {
            scan.extend_from_slice(&[128, 128, 128, 128]);
        }
        let bytes = radiance_file("-Y 2 +X 2", &scan);
        let img = decode_radiance(&bytes).expect("应能解码老式扫描行");
        assert_eq!((img.width(), img.height()), (2, 2));
        for px in img.pixels() {
            for c in px {
                assert!((*c - 0.5).abs() < 1e-6, "像素 {px:?} 应为 0.5");
            }
        }
    }

    #[test]
    fn decode_radiance_rle_scanlines() {
        let width = 8_usize;
        let mut scan = vec![2u8, 2, 0, width as u8];
        // 4 个通道各一条 literal run：code = 8 后跟 8 个值
        for _ in 0..4 {
            scan.push(width as u8);
            scan.extend(vec![128u8; width].iter());
        }
        let bytes = radiance_file("-Y 1 +X 8", &scan);
        let img = decode_radiance(&bytes).expect("应能解码 RLE 扫描行");
        assert_eq!((img.width(), img.height()), (8, 1));
        for px in img.pixels() {
            assert!((px[0] - 0.5).abs() < 1e-6);
        }
        // 重复计数分支：128 + count 表示一个值重复 count 次
        let mut rle = vec![2u8, 2, 0, width as u8];
        for _ in 0..4 {
            rle.push(128 + width as u8);
            rle.push(128);
        }
        let img2 = decode_radiance(&radiance_file("-Y 1 +X 8", &rle)).expect("RLE 重复分支");
        assert!((img2.pixel(7, 0)[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn decode_radiance_rejects_unsupported_headers() {
        // XYZE 色彩空间不做色域转换，直接拒绝
        let mut xyze = Vec::new();
        xyze.extend_from_slice(b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 1 +X 8\n");
        xyze.extend([0u8; 64].iter());
        assert!(decode_radiance(&xyze).is_none());
        // 非 Radiance 文件
        assert!(decode_radiance(b"not a hdr").is_none());
        // 头正常但数据被截断
        assert!(decode_radiance(&radiance_file("-Y 2 +X 2", &[1, 2, 3, 4])).is_none());
        // 分辨率行缺项
        assert!(decode_radiance(&radiance_file("-Y 2", &[0; 32])).is_none());
    }

    #[test]
    fn decode_radiance_honours_flip_axes() {
        // "-Y 1 -X 2"：x 方向翻转，第一列数据落到最右列
        let mut scan = Vec::new();
        scan.extend_from_slice(&[255, 0, 0, 129]); // 亮红
        scan.extend_from_slice(&[0, 0, 0, 0]); // 黑
        let img = decode_radiance(&radiance_file("-Y 1 -X 2", &scan)).unwrap();
        assert!(
            img.pixel(1, 0)[0] > img.pixel(0, 0)[0],
            "翻转后亮点应在右侧"
        );
    }

    #[test]
    fn hdr_image_rejects_inconsistent_sizes() {
        assert!(HdrImage::new(0, 4, Vec::new()).is_none());
        assert!(HdrImage::new(2, 2, vec![[0.0; 3]; 3]).is_none());
        // 非有限 / 负分量在入口即被收敛
        let img = HdrImage::new(1, 1, vec![[f32::NAN, -3.0, 2.0]]).unwrap();
        assert_eq!(img.pixel(0, 0), [0.0, 0.0, 2.0]);
    }

    #[test]
    fn cube_face_uv_roundtrip_is_stable() {
        for face in 0..CUBE_FACES {
            for i in 0..5 {
                for j in 0..5 {
                    let u = (i as f32 + 0.5) / 5.0;
                    let v = (j as f32 + 0.5) / 5.0;
                    let dir = cube_face_dir(face, u, v);
                    assert!((dir.length() - 1.0).abs() < 1e-5);
                    let (back_face, bu, bv) = cube_dir_face(dir);
                    assert_eq!(back_face, face, "面 {face} 往返变成 {back_face}");
                    assert!((bu - u).abs() < 1e-4 && (bv - v).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn cube_axis_centres_map_to_principal_axes() {
        let cases = [
            (0, Vec3::X),
            (1, Vec3::NEG_X),
            (2, Vec3::Y),
            (3, Vec3::NEG_Y),
            (4, Vec3::Z),
            (5, Vec3::NEG_Z),
        ];
        for (face, axis) in cases {
            let dir = cube_face_dir(face, 0.5, 0.5);
            assert!((dir - axis).length() < 1e-5, "面 {face} 中心应为 {axis:?}");
            let (f, u, v) = cube_dir_face(axis);
            assert_eq!((f, u), (face, 0.5));
            assert!((v - 0.5).abs() < 1e-5);
        }
        // 退化输入不 panic，回退 +Y 面中心
        let (f, _, _) = cube_dir_face(Vec3::ZERO);
        assert_eq!(f, 2);
    }

    #[test]
    fn equirect_uv_places_poles_and_front() {
        let (u, v) = equirect_uv(Vec3::Y);
        assert!((v - 0.0).abs() < 1e-5);
        // 极点是经度奇异点，u 落在那条经线上都合法（此处是环绕后的 1.0 ≡ 0.0）
        assert!(u.is_finite() && (0.0..=1.0).contains(&u));
        let (_, v_bottom) = equirect_uv(Vec3::NEG_Y);
        assert!((v_bottom - 1.0).abs() < 1e-5);
        let (u_front, v_front) = equirect_uv(Vec3::Z);
        assert!((u_front - 0.5).abs() < 1e-5 && (v_front - 0.5).abs() < 1e-5);
        // 水平环绕：采样 u = 0 与 u = 1 应当一致
        let img = HdrImage::new(4, 2, vec![[1.0, 0.0, 0.0]; 8]).unwrap();
        assert!((img.sample_uv(0.0, 0.5) - img.sample_uv(1.0, 0.5)).length() < 1e-5);
        // 常量图按方向采样恒为常量
        assert!((img.sample_dir(Vec3::new(0.3, -0.2, 0.9)).x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn equirect_to_cubemap_preserves_constant_and_gradient() {
        let flat = HdrImage::constant(16, 8, [0.25, 0.25, 0.25]).unwrap();
        let cube = equirect_to_cubemap(&flat, 16);
        assert_eq!(cube.face_size(), 16);
        for face in 0..CUBE_FACES {
            for px in cube.face(face).pixels() {
                assert!((px[0] - 0.25).abs() < 1e-5);
            }
        }
        // 上下渐变：+Y 面亮、-Y 面暗，中间面居中
        let mut pixels = Vec::new();
        for y in 0..32 {
            let t = 1.0 - y as f32 / 31.0;
            for _ in 0..16 {
                pixels.push([t, t, t]);
            }
        }
        let grad = HdrImage::new(16, 32, pixels).unwrap();
        let cube = equirect_to_cubemap(&grad, 16);
        let up = cube.sample_dir(Vec3::Y).x;
        let down = cube.sample_dir(Vec3::NEG_Y).x;
        let side = cube.sample_dir(Vec3::Z).x;
        assert!(up > 0.95, "天顶应接近 1，实际 {up}");
        assert!(down < 0.05, "地面应接近 0，实际 {down}");
        assert!(side > 0.4 && side < 0.6, "地平线应居中，实际 {side}");
    }

    #[test]
    fn face_size_is_sanitized_to_power_of_two() {
        let img = HdrImage::constant(8, 4, [1.0; 3]).unwrap();
        assert_eq!(equirect_to_cubemap(&img, 100).face_size(), 128);
        assert_eq!(equirect_to_cubemap(&img, 1).face_size(), 4);
        assert_eq!(
            equirect_to_cubemap(&img, MAX_CUBE_FACE_SIZE * 4).face_size(),
            MAX_CUBE_FACE_SIZE
        );
    }

    #[test]
    fn cubemap_rejects_invalid_layout() {
        let face = HdrImage::constant(8, 8, [1.0; 3]).unwrap();
        assert!(Cubemap::new(8, vec![face.clone(); 5]).is_none());
        assert!(Cubemap::new(7, vec![face.clone(); 6]).is_none());
        assert!(Cubemap::new(16, vec![face; 6]).is_none());
        assert!(Cubemap::new(
            MAX_CUBE_FACE_SIZE * 2,
            vec![HdrImage::constant(1, 1, [0.0; 3]).unwrap(); 6]
        )
        .is_none());
    }

    #[test]
    fn project_irradiance_of_constant_is_pi_times_constant() {
        let level = 0.4_f32;
        let cube = gray(level, 16);
        let probe = cube.project_irradiance();
        for normal in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.3, 0.5, -0.8)] {
            let e = probe.irradiance(normal);
            let expected = std::f32::consts::PI * level;
            assert!(
                (e.x - expected).abs() < expected * 0.02,
                "常量图辐照度应为 πL = {expected}，实际 {e:?}"
            );
            assert!((e.y - e.x).abs() < 1e-4 && (e.z - e.x).abs() < 1e-4);
        }
        // 与程序化天空的常量解一致（同一套 Sh9::irradiance 契约）
        let projected = Sh9::project(&crate::env::EnvironmentSettings {
            sky_zenith: [level; 3],
            sky_horizon: [level; 3],
            ground: [level; 3],
            sun_intensity: 0.0,
            ..crate::env::EnvironmentSettings::default()
        });
        let a = probe.irradiance(Vec3::Y);
        let b = projected.irradiance(Vec3::Y);
        assert!(
            (a.x - b.x).abs() < b.x * 0.05,
            "立方体投影与解析投影应同量级"
        );
    }

    #[test]
    fn cubemap_solid_angle_sums_to_four_pi() {
        let size = 32_usize;
        let inv = 2.0 / size as f32;
        let mut total = 0.0_f32;
        for _ in 0..CUBE_FACES {
            for y in 0..size {
                for x in 0..size {
                    let u = (x as f32 + 0.5) * inv - 1.0;
                    let v = (y as f32 + 0.5) * inv - 1.0;
                    total += inv * inv / (1.0 + u * u + v * v).powf(1.5);
                }
            }
        }
        let four_pi = 4.0 * std::f32::consts::PI;
        assert!(
            (total - four_pi).abs() < four_pi * 0.01,
            "立体角和应约等于 4π，实际 {total}"
        );
    }

    #[test]
    fn prefilter_chain_shrinks_and_converges_to_average() {
        // 左右两半不同亮度，预滤波后高层应趋近立体角平均值
        let size = 32_usize;
        let mut pixels = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let c = if x < size / 2 { 1.0 } else { 0.0 };
                pixels.push([c, c, c]);
                let _ = y;
            }
        }
        let faces = (0..CUBE_FACES)
            .map(|_| HdrImage::new(size, size, pixels.clone()).unwrap())
            .collect();
        let cube = Cubemap::new(size, faces).unwrap();
        let chain = prefilter_specular(&cube, 6);
        assert_eq!(chain.mip_levels(), 6);
        assert_eq!(chain.base().face_size(), 32);
        for level in 1..6 {
            assert_eq!(chain.mip(level).face_size(), 32 >> level);
            assert_eq!(
                chain.mip(level).face(0).width(),
                (32 >> level).max(1) as usize
            );
        }
        // 对比度单调下降：mip5 的邻域差应远小于原图
        let spread = |c: &Cubemap| -> f32 {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for px in c.face(4).pixels() {
                lo = lo.min(px[0]);
                hi = hi.max(px[0]);
            }
            hi - lo
        };
        assert!(spread(chain.mip(5)) < spread(chain.base()) * 0.8);
        // 层数受面长限制：32 面最多 6 层（32→16→8→4→2→1）
        assert_eq!(prefilter_specular(&cube, MAX_SPECULAR_MIPS).mip_levels(), 6);
        assert_eq!(prefilter_specular(&cube, 0).mip_levels(), 1);
    }

    #[test]
    fn mip_for_roughness_matches_layer_roughness() {
        let levels = 6_u32;
        assert_eq!(PrefilteredEnv::mip_for_roughness(0.0, levels), 0.0);
        assert_eq!(
            PrefilteredEnv::mip_for_roughness(1.0, levels),
            (levels - 1) as f32
        );
        for level in 0..levels {
            let r = PrefilteredEnv::roughness_of(level, levels);
            let mip = PrefilteredEnv::mip_for_roughness(r, levels);
            assert!((mip - level as f32).abs() < 1e-4);
        }
        // 单层链永远采样第 0 层；越界粗糙度被夹住；NaN 用缺省粗糙度
        assert_eq!(PrefilteredEnv::mip_for_roughness(0.7, 1), 0.0);
        assert_eq!(PrefilteredEnv::mip_for_roughness(-3.0, levels), 0.0);
        assert_eq!(
            PrefilteredEnv::mip_for_roughness(9.0, levels),
            (levels - 1) as f32
        );
        let nan = PrefilteredEnv::mip_for_roughness(f32::NAN, levels);
        assert!((nan - DEFAULT_ROUGHNESS * (levels - 1) as f32).abs() < 1e-6);
        assert_eq!(PrefilteredEnv::roughness_of(0, 1), 0.0);
    }

    #[test]
    fn build_probe_composes_stages() {
        let img = HdrImage::constant(16, 8, [0.5, 0.4, 0.3]).unwrap();
        let probe = build_probe(&img, 16, 4);
        assert_eq!(probe.mip_levels(), 4);
        assert_eq!(probe.base().face_size(), 16);
        // 每一层都保留常量特性（卷积不改变常量图）
        for level in 0..4 {
            let c = probe.mip(level).sample_dir(Vec3::new(0.5, 0.5, 0.5));
            assert!((c.x - 0.5).abs() < 0.02, "第 {level} 层 {c:?}");
        }
        // 低粗糙度几乎不模糊：镜面方向附近与原图同值
        assert!(MIN_ROUGHNESS < PrefilteredEnv::roughness_of(1, 4));
    }

    #[test]
    fn face_rgba_layout_is_flat_rgba() {
        let cube = gray(0.5, 4);
        let data = cube.face_rgba(2);
        assert_eq!(data.len(), 4 * 4 * 4);
        assert_eq!(&data[0..4], &[0.5, 0.5, 0.5, 1.0]);
        assert_eq!(data[3], 1.0);
        assert!(data.chunks(4).all(|px| px[3] == 1.0));
    }

    #[test]
    fn settings_default_is_inactive() {
        let s = EnvironmentMapSettings::default();
        // 缺省即「没写路径」：开关是开的，但没有来源 → 不激活，画面保持解析 IBL
        assert!(s.enabled);
        assert!(!s.is_active());
        assert!(s.is_default());
        assert_eq!(s.source_label(), "none");
        assert_eq!(s.face_size, DEFAULT_CUBE_FACE_SIZE);
        assert_eq!(s.mip_levels, DEFAULT_SPECULAR_MIPS);
    }

    #[test]
    fn settings_sanitize_paths_and_ranges() {
        let s = EnvironmentMapSettings {
            enabled: true,
            equirect: Some("  assets/sky.hdr  ".to_string()),
            faces: Some(vec![" a.hdr".to_string(), "  ".to_string()]),
            face_size: 3,
            mip_levels: 99,
            intensity: f32::NAN,
        }
        .sanitized();
        // 路径去空白；只有两条面的 faces 整段作废（半套面拼不出 cubemap）
        assert_eq!(s.equirect.as_deref(), Some("assets/sky.hdr"));
        assert!(s.faces.is_none());
        // 3 → 收敛到合法区间的 2 的幂；层数夹到上限；NaN 强度回落 1.0
        assert_eq!(s.face_size, 4);
        assert_eq!(s.mip_levels, MAX_SPECULAR_MIPS);
        assert_eq!(s.intensity, 1.0);
        assert!(s.is_active());
        assert_eq!(s.source_label(), "equirect");

        // 全空白的路径等于没写
        let empty = EnvironmentMapSettings {
            equirect: Some("   ".to_string()),
            ..EnvironmentMapSettings::default()
        }
        .sanitized();
        assert!(empty.equirect.is_none());
        assert!(!empty.is_active());
        assert!(empty.is_default());

        // 强度夹到 IBL 上限，负值归零
        assert_eq!(
            EnvironmentMapSettings {
                intensity: -5.0,
                ..EnvironmentMapSettings::default()
            }
            .sanitized()
            .intensity,
            0.0
        );
        assert_eq!(
            EnvironmentMapSettings {
                intensity: 1e9,
                ..EnvironmentMapSettings::default()
            }
            .sanitized()
            .intensity,
            crate::env::MAX_IBL_INTENSITY
        );
    }

    #[test]
    fn settings_faces_take_priority_and_need_six() {
        let faces = (0..CUBE_FACES)
            .map(|i| format!("face{i}.hdr"))
            .collect::<Vec<_>>();
        let s = EnvironmentMapSettings {
            equirect: Some("sky.hdr".to_string()),
            ..EnvironmentMapSettings::with_faces(faces.clone())
        }
        .sanitized();
        assert_eq!(s.faces.as_ref().unwrap(), &faces);
        assert_eq!(s.source_label(), "faces");
        assert!(s.is_active());

        // 关掉开关后即使有路径也不激活，但配置原样保留
        let off = EnvironmentMapSettings {
            enabled: false,
            ..s.clone()
        };
        assert!(!off.is_active());
        assert_eq!(off.source_label(), "disabled");
        assert_eq!(off.faces.as_ref().unwrap(), &faces);
    }

    #[test]
    fn settings_json_roundtrip_omits_empty_sources() {
        let text =
            serde_json::to_string(&EnvironmentMapSettings::with_equirect("sky.hdr")).unwrap();
        assert!(text.contains("\"equirect\":\"sky.hdr\""));
        assert!(!text.contains("faces"));
        let back: EnvironmentMapSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.equirect.as_deref(), Some("sky.hdr"));
        assert_eq!(back.face_size, DEFAULT_CUBE_FACE_SIZE);

        // 只写一个 path 字段，其余旋钮全部走缺省
        let sparse: EnvironmentMapSettings =
            serde_json::from_str("{\"faces\":[\"a\",\"b\"]}").unwrap();
        assert!(sparse.enabled);
        assert_eq!(sparse.faces.unwrap().len(), 2);
    }
}
