//! 像素级视觉回归：基准图 ↔ 实际截图的比对、差异统计与失败产物。
//!
//! 定位：`capture.frame` 落下的 PNG 是「这一帧画面长什么样」的客观记录，
//! 本模块把它变成**可自动判定**的信号 —— 基准图 + 容差 + 差异统计 + 失败差异信息。
//!
//! 三层职责：
//!
//! * [`RgbaFrame`]：一张 RGBA8 图的 CPU 表示，负责 PNG 读写（复用 `image`，与
//!   [`crate::write_png_rgba`] 同一编码后端）；
//! * [`compare`]：逐像素逐通道比差，产出 [`DiffReport`]（坏点数 / 坏点比例 /
//!   最大与平均差 / 差异包围盒 / 最差若干像素）；
//! * [`assert_png_matches`]：文件级入口，比较两张 PNG，失败时把差异图与文本报告
//!   写到指定目录，供人直接打开定位。
//!
//! **纯 CPU 逻辑，零 wgpu 依赖**：因此比对与容差语义可以在任何机器上单测，
//! 不需要 GPU；需要 GPU 的只有「产出一张新截图」那一步（见 `r3e` 的
//! `--capture-frame` 与集成用例）。
//!
//! 判定语义（刻意简单、可解释）：
//!
//! * 单像素差 = 四个通道 `|基准 - 实际|` 的**最大值**（alpha 也算，截图 alpha 恒为 255，
//!   一旦某条链路忘了写不透明，这里会立刻炸出来）；
//! * 单像素差 > [`Tolerance::channel_delta`] 即判为**坏点**（坏点掩码用于画差异图）；
//! * 判定通过 ⇔ `坏点比例 <= Tolerance::bad_pixel_ratio`。单点噪声被容忍，
//!   而「整屏偏色 / 整体缺失」这种大范围变化必然超比例。

use std::path::{Path, PathBuf};

use thiserror::Error;

/// 默认的单通道容差：0..255 上的绝对差，2 大致相当于 8bit 量化的两级抖动。
pub const DEFAULT_CHANNEL_DELTA: u8 = 2;

/// 默认的坏点比例上限：0.1%。1280×720 下约 922 个像素，足以吃掉 MSAA 抖动
/// 与浮点重排带来的零星差异，但吃不下一块可见的画错区域。
pub const DEFAULT_BAD_PIXEL_RATIO: f32 = 0.001;

/// [`DiffReport::worst`] 里保留的最差像素条数上限（失败信息够定位即可，不刷屏）。
pub const WORST_LIMIT: usize = 8;

/// 差异图里坏点的标记色（品红）：正常像素会被压暗，坏点保持高饱和，肉眼一眼可见。
const DIFF_MARK: [u8; 4] = [255, 0, 255, 255];

/// 差异图里非坏点的压暗系数（除以该值），避免整张图抢眼睛。
const DIFF_DIM: u8 = 4;

/// 容差配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// 单通道允许的绝对差（0..=255）；超过即判为坏点。
    pub channel_delta: u8,
    /// 允许的坏点比例上限（0.0..=1.0）；坏点比例不超过它才算通过。
    pub bad_pixel_ratio: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            channel_delta: DEFAULT_CHANNEL_DELTA,
            bad_pixel_ratio: DEFAULT_BAD_PIXEL_RATIO,
        }
    }
}

impl Tolerance {
    /// 显式给定两个阈值（构造时不做收敛，判定前会经 [`Self::sanitized`]）。
    pub const fn new(channel_delta: u8, bad_pixel_ratio: f32) -> Self {
        Self {
            channel_delta,
            bad_pixel_ratio,
        }
    }

    /// 逐字节完全一致的严格口径：任何像素任何通道差 1 都算失败。
    pub const fn exact() -> Self {
        Self {
            channel_delta: 0,
            bad_pixel_ratio: 0.0,
        }
    }

    /// 把比例夹回 `0.0..=1.0`（NaN 视作 0，避免「NaN 比较恒假」把判定变成永远通过）。
    pub fn sanitized(self) -> Self {
        let ratio = if self.bad_pixel_ratio.is_nan() {
            0.0
        } else {
            self.bad_pixel_ratio.clamp(0.0, 1.0)
        };
        Self {
            channel_delta: self.channel_delta,
            bad_pixel_ratio: ratio,
        }
    }
}

/// 视觉回归过程中的失败原因。
#[derive(Debug, Error)]
pub enum VisualDiffError {
    #[error("读取图像 `{path}` 失败: {message}")]
    Load { path: String, message: String },
    #[error("写入图像 `{path}` 失败: {message}")]
    Save { path: String, message: String },
    #[error("像素缓冲区大小与尺寸不符：{width}x{height} 需要 {expected} 字节，收到 {actual}")]
    Buffer {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
    #[error(
        "尺寸不一致：基准 {baseline_width}x{baseline_height}，实际 {actual_width}x{actual_height}"
    )]
    SizeMismatch {
        baseline_width: u32,
        baseline_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
}

/// 一张 RGBA8 图像（行优先、紧密排布，无行填充）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    /// 长度恒为 `width * height * 4`。
    pub pixels: Vec<u8>,
}

impl RgbaFrame {
    /// 用一块已有缓冲区建图；尺寸为 0 或字节数不符时直接报错（绝不产出错位图）。
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, VisualDiffError> {
        let expected = expected_len(width, height);
        if width == 0 || height == 0 {
            return Err(VisualDiffError::Buffer {
                width,
                height,
                expected,
                actual: pixels.len(),
            });
        }
        if pixels.len() != expected {
            return Err(VisualDiffError::Buffer {
                width,
                height,
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// 从切片复制建图。
    pub fn from_slice(width: u32, height: u32, pixels: &[u8]) -> Result<Self, VisualDiffError> {
        Self::new(width, height, pixels.to_vec())
    }

    /// 读一张 PNG。只有 PNG 走这条路径 —— 基准图与截图都是 PNG，不做格式嗅探，
    /// 免得把一张 JPEG 当基准读进来再「因为量化差」误判。
    pub fn load_png(path: impl AsRef<Path>) -> Result<Self, VisualDiffError> {
        let path = path.as_ref();
        let fail = |message: String| VisualDiffError::Load {
            path: path.display().to_string(),
            message,
        };
        let bytes = std::fs::read(path).map_err(|err| fail(format!("读文件失败: {err}")))?;
        let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|err| fail(format!("PNG 解码失败: {err}")))?;
        let rgba = decoded.to_rgba8();
        Self::new(rgba.width(), rgba.height(), rgba.into_raw())
    }

    /// 写一张 PNG（目录不存在会自动创建）。
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), VisualDiffError> {
        let path = path.as_ref();
        let fail = |message: String| VisualDiffError::Save {
            path: path.display().to_string(),
            message,
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| fail(format!("创建目录失败: {err}")))?;
            }
        }
        let image = image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or_else(|| fail("像素缓冲区与尺寸不符".to_string()))?;
        image
            .save(path)
            .map_err(|err| fail(format!("PNG 编码失败: {err}")))
    }

    /// 像素总数。
    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// 取某个像素的 RGBA；越界返回 `None`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let base = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        Some([
            self.pixels[base],
            self.pixels[base + 1],
            self.pixels[base + 2],
            self.pixels[base + 3],
        ])
    }
}

/// 差异包围盒（闭区间：`x1` / `y1` 是**包含**的最后一个坏点坐标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBox {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl PixelBox {
    /// 包围盒宽度（像素数，闭区间口径）。
    pub fn width(&self) -> u32 {
        self.x1 - self.x0 + 1
    }

    /// 包围盒高度（像素数，闭区间口径）。
    pub fn height(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// 单个坏点的明细（失败时给到「哪个像素、基准是什么色、实际是什么色」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelDiff {
    pub x: u32,
    pub y: u32,
    pub baseline: [u8; 4],
    pub actual: [u8; 4],
    /// 四通道绝对差的最大值。
    pub delta: u8,
}

impl PixelDiff {
    /// 单行文本，直接进失败信息。
    pub fn describe(&self) -> String {
        format!(
            "({}, {}) 基准 {:?} 实际 {:?} 最大差 {}",
            self.x, self.y, self.baseline, self.actual, self.delta
        )
    }
}

/// 一次比对的完整结果。
#[derive(Debug, Clone)]
pub struct DiffReport {
    pub width: u32,
    pub height: u32,
    pub total_pixels: usize,
    /// 坏点数（单像素差 > `tolerance.channel_delta`）。
    pub bad_pixels: usize,
    /// 坏点比例（`bad_pixels / total_pixels`，0.0..=1.0）。
    pub bad_ratio: f32,
    /// 全图最大的单像素差（即使全部通过也值得看一眼）。
    pub max_delta: u8,
    /// 全图平均的单像素差。
    pub mean_delta: f32,
    /// 坏点包围盒；没有坏点时为 `None`。
    pub bbox: Option<PixelBox>,
    /// 最差的前 [`WORST_LIMIT`] 个坏点，按差降序。
    pub worst: Vec<PixelDiff>,
    /// 判定所用的容差（已收敛）。
    pub tolerance: Tolerance,
    /// 坏点掩码：每像素 1 字节，255 = 坏点，0 = 正常。
    mask: Vec<u8>,
    /// 失败时落盘的差异产物（差异图 / 文本报告），用于在失败信息里指路。
    pub artifacts: Vec<PathBuf>,
}

impl DiffReport {
    /// 判定是否通过（坏点比例不超过容差）。
    pub fn passed(&self) -> bool {
        self.bad_ratio <= self.tolerance.bad_pixel_ratio
    }

    /// 坏点掩码（长度 = 像素数）。
    pub fn diff_mask(&self) -> &[u8] {
        &self.mask
    }

    /// 把差异可视化：坏点成品红，其余按基准压暗，一眼看出「哪里变了、变了多大一片」。
    pub fn diff_image(&self, baseline: &RgbaFrame) -> RgbaFrame {
        let mut pixels = baseline.pixels.clone();
        for (index, byte) in pixels.iter_mut().enumerate() {
            let pixel = index / 4;
            if self.mask.get(pixel).is_some_and(|flag| *flag != 0) {
                *byte = DIFF_MARK[index % 4];
            } else {
                *byte /= DIFF_DIM;
                if index % 4 == 3 {
                    *byte = 255;
                }
            }
        }
        RgbaFrame {
            width: baseline.width,
            height: baseline.height,
            pixels,
        }
    }

    /// 多行失败信息：判定阈值、坏点统计、差异范围、最差像素、差异产物路径。
    ///
    /// 纯文本，既能进 `panic!` 也能直接写进 `.txt` 报告，不需要再加工。
    pub fn summary(&self) -> String {
        let verdict = if self.passed() { "通过" } else { "未通过" };
        let mut out = String::new();
        out.push_str(&format!(
            "视觉回归{verdict}：{width}x{height}，坏点 {bad}/{total}（{ratio:.4}%，阈值 {limit:.4}%），\
             单通道容差 {delta}，最大差 {max}，平均差 {mean:.3}\n",
            width = self.width,
            height = self.height,
            bad = self.bad_pixels,
            total = self.total_pixels,
            ratio = self.bad_ratio * 100.0,
            limit = self.tolerance.bad_pixel_ratio * 100.0,
            delta = self.tolerance.channel_delta,
            max = self.max_delta,
            mean = self.mean_delta,
        ));
        match self.bbox {
            Some(bbox) => out.push_str(&format!(
                "差异范围：x {}..={} / y {}..={}（{}x{} 像素）\n",
                bbox.x0,
                bbox.x1,
                bbox.y0,
                bbox.y1,
                bbox.width(),
                bbox.height()
            )),
            None => out.push_str("差异范围：无坏点\n"),
        }
        if !self.worst.is_empty() {
            out.push_str("最差像素：\n");
            for diff in &self.worst {
                out.push_str(&format!("  - {}\n", diff.describe()));
            }
        }
        if !self.artifacts.is_empty() {
            out.push_str("差异产物：\n");
            for path in &self.artifacts {
                out.push_str(&format!("  - {}\n", path.display()));
            }
        }
        out
    }
}

/// 逐像素比对两张同尺寸图像。
///
/// 尺寸不一致直接报错（缩放过/窗口尺寸变了不是「像素回归」而是换了口径，
/// 不该用容差糊过去）；比较本身不分配整图副本，只保留坏点掩码与最差若干条明细。
pub fn compare(
    baseline: &RgbaFrame,
    actual: &RgbaFrame,
    tolerance: Tolerance,
) -> Result<DiffReport, VisualDiffError> {
    if baseline.width != actual.width || baseline.height != actual.height {
        return Err(VisualDiffError::SizeMismatch {
            baseline_width: baseline.width,
            baseline_height: baseline.height,
            actual_width: actual.width,
            actual_height: actual.height,
        });
    }
    let tolerance = tolerance.sanitized();
    let total = baseline.pixel_count();
    let mut mask = vec![0u8; total];
    let mut bad_pixels = 0usize;
    let mut max_delta = 0u8;
    let mut sum_delta: u64 = 0;
    let mut bbox: Option<PixelBox> = None;
    let mut worst: Vec<PixelDiff> = Vec::with_capacity(WORST_LIMIT + 1);

    for y in 0..baseline.height {
        for x in 0..baseline.width {
            let index = ((y as usize) * (baseline.width as usize) + (x as usize)) * 4;
            let base = &baseline.pixels[index..index + 4];
            let real = &actual.pixels[index..index + 4];
            let delta = (0..4)
                .map(|c| base[c].abs_diff(real[c]))
                .max()
                .unwrap_or_default();
            sum_delta += u64::from(delta);
            max_delta = max_delta.max(delta);
            if delta <= tolerance.channel_delta {
                continue;
            }
            bad_pixels += 1;
            mask[y as usize * baseline.width as usize + x as usize] = 255;
            bbox = Some(match bbox {
                Some(current) => PixelBox {
                    x0: current.x0.min(x),
                    y0: current.y0.min(y),
                    x1: current.x1.max(x),
                    y1: current.y1.max(y),
                },
                None => PixelBox {
                    x0: x,
                    y0: y,
                    x1: x,
                    y1: y,
                },
            });
            let entry = PixelDiff {
                x,
                y,
                baseline: [base[0], base[1], base[2], base[3]],
                actual: [real[0], real[1], real[2], real[3]],
                delta,
            };
            push_worst(&mut worst, entry);
        }
    }
    worst.sort_by_key(|a| std::cmp::Reverse(a.delta));

    let bad_ratio = if total == 0 {
        0.0
    } else {
        (bad_pixels as f32) / (total as f32)
    };
    let mean_delta = if total == 0 {
        0.0
    } else {
        (sum_delta as f32) / (total as f32)
    };
    Ok(DiffReport {
        width: baseline.width,
        height: baseline.height,
        total_pixels: total,
        bad_pixels,
        bad_ratio,
        max_delta,
        mean_delta,
        bbox,
        worst,
        tolerance,
        mask,
        artifacts: Vec::new(),
    })
}

/// 读两张 PNG 并比对（文件级入口，测试与脚本都走这里）。
pub fn compare_png(
    baseline_path: impl AsRef<Path>,
    actual_path: impl AsRef<Path>,
    tolerance: Tolerance,
) -> Result<DiffReport, VisualDiffError> {
    let baseline = RgbaFrame::load_png(baseline_path)?;
    let actual = RgbaFrame::load_png(actual_path)?;
    compare(&baseline, &actual, tolerance)
}

/// 保持 [`WORST_LIMIT`] 条：满了就把当前最差的一条挤出去（列表很短，线性扫足够）。
fn push_worst(worst: &mut Vec<PixelDiff>, entry: PixelDiff) {
    if worst.len() < WORST_LIMIT {
        worst.push(entry);
        return;
    }
    if let Some((index, lowest)) = worst.iter().enumerate().min_by_key(|(_, diff)| diff.delta) {
        if entry.delta > lowest.delta {
            worst[index] = entry;
        }
    }
}

/// 断言两张 PNG 在容差内一致；不一致时把**差异图 + 文本报告**写到 `diff_dir`。
///
/// 这是给回归用例用的总入口：用例只需要一句 `assert!(report.passed(), "{}", report.summary())`，
/// 失败信息里就带着差异产物的绝对路径与最差像素明细。
pub fn assert_png_matches(
    baseline_path: impl AsRef<Path>,
    actual_path: impl AsRef<Path>,
    tolerance: Tolerance,
    diff_dir: impl AsRef<Path>,
) -> Result<DiffReport, VisualDiffError> {
    let baseline_path = baseline_path.as_ref();
    let actual_path = actual_path.as_ref();
    let mut report = compare_png(baseline_path, actual_path, tolerance)?;
    if report.passed() {
        return Ok(report);
    }

    let stem = actual_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "capture".to_string());
    let diff_dir = diff_dir.as_ref();
    let baseline = RgbaFrame::load_png(baseline_path)?;
    let image_path = diff_dir.join(format!("{stem}.diff.png"));
    report.diff_image(&baseline).save_png(&image_path)?;
    let text_path = diff_dir.join(format!("{stem}.diff.txt"));
    let mut text = format!(
        "基准: {}\n实际: {}\n",
        baseline_path.display(),
        actual_path.display()
    );
    text.push_str(&report.summary());
    std::fs::write(&text_path, text).map_err(|err| VisualDiffError::Save {
        path: text_path.display().to_string(),
        message: err.to_string(),
    })?;
    report.artifacts = vec![image_path, text_path];
    Ok(report)
}

/// `width * height * 4`，溢出的极端尺寸返回 0（后续按大小不符报错）。
fn expected_len(width: u32, height: u32) -> usize {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张纯色图，便于逐像素改值。
    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaFrame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&color);
        }
        RgbaFrame::new(width, height, pixels).expect("纯色图应可构造")
    }

    fn set_pixel(frame: &mut RgbaFrame, x: u32, y: u32, color: [u8; 4]) {
        let index = ((y as usize) * (frame.width as usize) + (x as usize)) * 4;
        frame.pixels[index..index + 4].copy_from_slice(&color);
    }

    #[test]
    fn buffer_size_must_match_dimensions() {
        assert!(matches!(
            RgbaFrame::new(2, 2, vec![0; 15]),
            Err(VisualDiffError::Buffer { .. })
        ));
        assert!(matches!(
            RgbaFrame::new(0, 4, vec![]),
            Err(VisualDiffError::Buffer { .. })
        ));
        let frame = RgbaFrame::new(2, 2, vec![0; 16]).expect("16 字节刚好是 2x2 RGBA");
        assert_eq!(frame.pixel_count(), 4);
        assert_eq!(frame.pixel(1, 1), Some([0, 0, 0, 0]));
        assert_eq!(frame.pixel(2, 0), None, "越界必须给 None 而不是 panic");
    }

    #[test]
    fn identical_frames_pass_with_zero_delta() {
        let a = solid(8, 6, [10, 20, 30, 255]);
        let b = a.clone();
        let report = compare(&a, &b, Tolerance::exact()).expect("同尺寸可比");
        assert!(report.passed());
        assert_eq!(report.bad_pixels, 0);
        assert_eq!(report.max_delta, 0);
        assert!(report.bbox.is_none());
        assert!(report.worst.is_empty());
        assert!(report.summary().contains("差异范围：无坏点"));
    }

    #[test]
    fn channel_delta_within_tolerance_is_not_a_bad_pixel() {
        let a = solid(4, 4, [100, 100, 100, 255]);
        let mut b = a.clone();
        set_pixel(&mut b, 2, 1, [102, 98, 101, 255]);
        let report = compare(&a, &b, Tolerance::new(2, 0.0)).expect("同尺寸可比");
        assert_eq!(report.bad_pixels, 0, "差 2 恰好等于阈值，不算坏点");
        assert_eq!(report.max_delta, 2);
        assert!(report.passed());

        let report = compare(&a, &b, Tolerance::new(1, 0.0)).expect("同尺寸可比");
        assert_eq!(report.bad_pixels, 1, "阈值降到 1 后同一像素变坏点");
        assert!(!report.passed(), "比例为 0 时一个坏点都不许有");
    }

    #[test]
    fn single_bad_pixel_reports_location_and_colors() {
        let a = solid(16, 16, [0, 0, 0, 255]);
        let mut b = a.clone();
        set_pixel(&mut b, 3, 5, [200, 10, 10, 255]);
        let report = compare(&a, &b, Tolerance::default()).expect("同尺寸可比");
        assert_eq!(report.bad_pixels, 1);
        assert_eq!(report.max_delta, 200);
        assert_eq!(
            report.bbox,
            Some(PixelBox {
                x0: 3,
                y0: 5,
                x1: 3,
                y1: 5
            })
        );
        assert_eq!(report.worst.len(), 1);
        let worst = report.worst[0];
        assert_eq!((worst.x, worst.y), (3, 5));
        assert_eq!(worst.actual, [200, 10, 10, 255]);
        let summary = report.summary();
        assert!(summary.contains("视觉回归未通过"), "{summary}");
        assert!(summary.contains("(3, 5)"), "{summary}");
    }

    #[test]
    fn bad_ratio_boundary_decides_the_verdict() {
        let a = solid(4, 4, [50, 50, 50, 255]);
        let mut b = a.clone();
        // 4x4 = 16 像素，坏 4 个 = 25%
        for x in 0..4 {
            set_pixel(&mut b, x, 0, [255, 255, 255, 255]);
        }
        let pass = compare(&a, &b, Tolerance::new(2, 0.25)).expect("同尺寸可比");
        assert_eq!(pass.bad_pixels, 4);
        assert!((pass.bad_ratio - 0.25).abs() < 1e-6);
        assert!(pass.passed(), "比例恰好等于阈值时应通过");

        let fail = compare(&a, &b, Tolerance::new(2, 0.24)).expect("同尺寸可比");
        assert!(!fail.passed(), "比例略低于阈值应失败");
    }

    #[test]
    fn size_mismatch_is_an_error_not_a_tolerance_case() {
        let a = solid(4, 4, [0, 0, 0, 255]);
        let b = solid(4, 5, [0, 0, 0, 255]);
        let err = compare(&a, &b, Tolerance::default()).expect_err("尺寸不同必须报错");
        let message = err.to_string();
        assert!(message.contains("尺寸不一致"), "{message}");
        assert!(message.contains("4x4"), "{message}");
        assert!(message.contains("4x5"), "{message}");
    }

    #[test]
    fn diff_image_marks_bad_pixels_and_dims_the_rest() {
        let a = solid(3, 3, [80, 80, 80, 255]);
        let mut b = a.clone();
        set_pixel(&mut b, 1, 1, [200, 0, 0, 255]);
        let report = compare(&a, &b, Tolerance::default()).expect("同尺寸可比");
        assert_eq!(report.diff_mask().len(), 9);
        let diff = report.diff_image(&a);
        assert_eq!(diff.pixel(1, 1), Some(DIFF_MARK), "坏点标成品红");
        assert_eq!(
            diff.pixel(0, 0),
            Some([20, 20, 20, 255]),
            "正常像素压暗但保持不透明"
        );
    }

    #[test]
    fn tolerance_sanitizes_nan_and_out_of_range_ratio() {
        let fixed = Tolerance::new(3, f32::NAN).sanitized();
        assert_eq!(
            fixed.bad_pixel_ratio, 0.0,
            "NaN 比例按 0 处理，绝不静默通过"
        );
        assert_eq!(Tolerance::new(3, -1.0).sanitized().bad_pixel_ratio, 0.0);
        assert_eq!(Tolerance::new(3, 2.0).sanitized().bad_pixel_ratio, 1.0);
    }

    #[test]
    fn worst_list_keeps_only_the_largest_deltas() {
        let a = solid(16, 16, [0, 0, 0, 255]);
        let mut b = a.clone();
        // 造 20 个坏点，差值递增：只留最大的 8 个
        for index in 0..20u8 {
            let x = u32::from(index) % 16;
            let y = u32::from(index) / 16;
            set_pixel(&mut b, x, y, [index * 10 + 5, 0, 0, 255]);
        }
        let report = compare(&a, &b, Tolerance::default()).expect("同尺寸可比");
        assert_eq!(report.bad_pixels, 20);
        assert_eq!(report.worst.len(), WORST_LIMIT);
        let deltas: Vec<u8> = report.worst.iter().map(|diff| diff.delta).collect();
        assert_eq!(deltas.first(), Some(&195), "最差的一条必须排在最前");
        assert!(
            deltas.windows(2).all(|pair| pair[0] >= pair[1]),
            "最差列表必须按差降序：{deltas:?}"
        );
    }

    #[test]
    fn png_round_trip_and_file_level_compare() {
        let dir = unique_dir("round_trip");
        let baseline_path = dir.join("baseline.png");
        let actual_path = dir.join("actual.png");
        let baseline = solid(5, 4, [12, 34, 56, 255]);
        baseline.save_png(&baseline_path).expect("基准图落盘");
        baseline.save_png(&actual_path).expect("实际图落盘");

        let report = assert_png_matches(
            &baseline_path,
            &actual_path,
            Tolerance::default(),
            dir.join("diff"),
        )
        .expect("文件级比对应成功");
        assert!(report.passed());
        assert!(report.artifacts.is_empty(), "通过时不该产出差异文件");

        let loaded = RgbaFrame::load_png(&baseline_path).expect("读回 PNG");
        assert_eq!(loaded, baseline, "PNG 往返必须逐字节一致");

        let mut changed = baseline.clone();
        set_pixel(&mut changed, 4, 3, [250, 34, 56, 255]);
        changed.save_png(&actual_path).expect("覆盖实际图");

        let report = assert_png_matches(
            &baseline_path,
            &actual_path,
            Tolerance::default(),
            dir.join("diff"),
        )
        .expect("文件级比对应成功");
        assert!(!report.passed());
        assert_eq!(report.artifacts.len(), 2, "失败时必须产出差异图与文本报告");
        for path in &report.artifacts {
            assert!(path.is_file(), "差异产物应真实落盘: {}", path.display());
        }
        let text = std::fs::read_to_string(&report.artifacts[1]).expect("读报告");
        assert!(text.contains("视觉回归未通过"), "{text}");
        assert!(text.contains("基准:"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_a_missing_or_non_png_file_reports_load_error() {
        let dir = unique_dir("load_error");
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let missing = dir.join("nope.png");
        let err = RgbaFrame::load_png(&missing).expect_err("缺文件必须报错");
        assert!(matches!(err, VisualDiffError::Load { .. }), "{err}");

        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"not a png").expect("写伪 PNG");
        let err = RgbaFrame::load_png(&fake).expect_err("坏 PNG 必须报错");
        assert!(matches!(err, VisualDiffError::Load { .. }), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 每个用例一个独立目录，避免并行测试互相踩文件。
    fn unique_dir(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("r3e_visual_{tag}_{}_{stamp}", std::process::id()))
    }
}
