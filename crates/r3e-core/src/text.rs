//! 2D 屏幕空间文本：字体集合、字形度量、排版与换行。
//!
//! 这一层是纯 CPU 的：它回答「每个字符用哪个字体的第几个字形、占多宽、怎么换行、
//! 对齐之后每个字形摆在哪」。字形位图的栅格化由 [`FontSet`] 借助 `fontdue` 完成，
//! 但把位图搬上 GPU、开渲染通道画四边形是 `r3e-render` 的事。
//! 因此本模块**不依赖任何图形 API**（core 不引 wgpu 的约束得以保持）。
//!
//! 排版逻辑通过 [`MetricsSource`] 抽象取度量，测试里可以塞一个纯算术的假字体，
//! 不依赖系统里装没装中文字体。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 文本默认字号（像素）。
pub const DEFAULT_SIZE: f32 = 24.0;
/// 允许的最小字号；再小的字形栅格化出来只剩噪声。
pub const MIN_SIZE: f32 = 4.0;
/// 允许的最大字号；再大单张位图可能超过图集页。
pub const MAX_SIZE: f32 = 512.0;
/// 缺字（所有字体都没有该字形）时按这个比例估宽，避免整行塌陷。
pub const TOFU_ADVANCE_RATIO: f32 = 0.5;

/// 水平对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    /// 解析脚本 / 命令行里的小写名字；也接受常见别名。
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "left" | "l" | "start" => Some(TextAlign::Left),
            "center" | "centre" | "c" | "middle" => Some(TextAlign::Center),
            "right" | "r" | "end" => Some(TextAlign::Right),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        }
    }
}

/// 字体的纵向度量（像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// 基线到行顶（正数）。
    pub ascent: f32,
    /// 基线到行底（负数，沿用 fontdue 的约定）。
    pub descent: f32,
    pub line_gap: f32,
}

impl LineMetrics {
    /// 无字体可用时的保守估算：0.8 / 0.2 em，够排版脚本在缺字体时仍然能跑。
    pub fn fallback(px: f32) -> Self {
        Self {
            ascent: px * 0.8,
            descent: -px * 0.2,
            line_gap: 0.0,
        }
    }

    /// 自然行高（行顶到行底）。
    pub fn natural_height(self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

/// 单个字符的排版度量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphMetric {
    /// 字体面索引（`FontSet` 内部下标）。
    pub face: u16,
    /// 该字体里的字形索引。
    pub glyph: u16,
    /// 前进宽度（像素）。
    pub advance: f32,
}

/// 排版所需的「度量来源」。
///
/// 生产环境由 [`FontSet`] 实现；测试可以用纯算术的假字体实现，
/// 这样排版、换行、对齐的用例不依赖机器上有没有中文字体。
pub trait MetricsSource {
    /// 查询字符的字形度量；`None` 表示**所有**字体面都缺这个字。
    fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric>;

    /// 行度量；默认给保守估算。
    fn line_metrics(&self, px: f32) -> LineMetrics {
        LineMetrics::fallback(px)
    }
}

/// 排版后的一枚字形。`x` 是相对文本块左边界的笔位置（不是位图左边！）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub ch: char,
    pub x: f32,
    pub advance: f32,
    pub face: u16,
    pub glyph: u16,
}

/// 排版后的一行。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaidOutLine {
    pub glyphs: Vec<PlacedGlyph>,
    /// 本行宽度，含末尾空格。
    pub width: f32,
    /// 本行基线相对文本块顶部的纵向偏移（像素，向下为正）。
    ///
    /// 加上 [`ScreenText::y`] 就是屏幕上的基线 y；字形位图的左上角再减去 bearing 即可落位。
    pub baseline: f32,
}

/// 整段文本的排版结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextLayout {
    pub lines: Vec<LaidOutLine>,
    /// 最长一行的宽度（不换行时即整段宽度）。
    pub width: f32,
    /// 对齐基准宽度：开了换行就是换行宽度，否则等于 `width`。
    pub block_width: f32,
    /// 多行总高度。
    pub height: f32,
    /// 行高（像素）。
    pub line_height: f32,
    /// 首行基线相对文本顶部的偏移。
    pub ascent: f32,
    /// 缺字数量（这些字符不会画出来，只按 tofu 宽度占位）。
    pub missing: usize,
    /// 实际可绘制的字形数量。
    pub glyph_count: usize,
}

impl TextLayout {
    /// 第 `line` 行第 `glyph` 枚字形相对**文本块左上角**的屏幕位置（像素，向下为正）。
    ///
    /// `y` 是该行的基线；把字形位图贴上去时，左上角 = `(x + bearing.left, y + bearing.top)`。
    /// 越界返回 `None` —— 上层排版探针不必自己算行高，也就不用重复发明这套账。
    pub fn glyph_offset(&self, line: usize, glyph: usize) -> Option<(f32, f32)> {
        let line = self.lines.get(line)?;
        let glyph = line.glyphs.get(glyph)?;
        Some((glyph.x, line.baseline))
    }
}

/// 纯文本度量结果（不涉及定位）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextMeasure {
    pub width: f32,
    pub height: f32,
    pub lines: usize,
    pub missing: usize,
}

/// 一条屏幕空间文本。
///
/// 这些字段全部来自场景文件 / 脚本，渲染层每帧按当前值重新排版 —— 场景是唯一真源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenText {
    /// 唯一键：脚本用它来更新同一条文本（同名即覆盖）。
    pub key: String,
    pub content: String,
    /// 文本块左上角（像素，窗口坐标系，原点在左上）。
    pub x: f32,
    pub y: f32,
    /// 字号（像素）。
    pub size: f32,
    /// sRGB 颜色 + 透明度。
    pub color: [f32; 4],
    pub align: TextAlign,
    /// 自动换行的最大宽度（像素）；0 表示不自动换行，只按 `\n` 硬换行。
    pub wrap: f32,
    /// 行高倍数（相对字体自然行高）；<= 0 表示用自然行高。
    pub line_height: f32,
    pub visible: bool,
}

impl Default for ScreenText {
    fn default() -> Self {
        Self {
            key: String::new(),
            content: String::new(),
            x: 0.0,
            y: 0.0,
            size: DEFAULT_SIZE,
            color: [1.0, 1.0, 1.0, 1.0],
            align: TextAlign::Left,
            wrap: 0.0,
            line_height: 0.0,
            visible: true,
        }
    }
}

impl ScreenText {
    pub fn new(
        key: impl Into<String>,
        content: impl Into<String>,
        x: f32,
        y: f32,
        size: f32,
    ) -> Self {
        Self {
            key: key.into(),
            content: content.into(),
            x,
            y,
            size,
            ..Default::default()
        }
    }

    /// 把外部（脚本 / 场景文件）来的数值收敛到合法范围。
    pub fn sanitized(mut self) -> Self {
        self.size = sanitize_size(self.size);
        if !self.x.is_finite() {
            self.x = 0.0;
        }
        if !self.y.is_finite() {
            self.y = 0.0;
        }
        if !self.wrap.is_finite() || self.wrap < 0.0 {
            self.wrap = 0.0;
        }
        if !self.line_height.is_finite() || self.line_height < 0.0 {
            self.line_height = 0.0;
        }
        for c in &mut self.color {
            if !c.is_finite() {
                *c = 0.0;
            }
            *c = c.clamp(0.0, 1.0);
        }
        self
    }
}

/// 字号收敛：非法值回落默认，超范围夹紧。
pub fn sanitize_size(size: f32) -> f32 {
    if !size.is_finite() || size <= 0.0 {
        return DEFAULT_SIZE;
    }
    size.clamp(MIN_SIZE, MAX_SIZE)
}

#[derive(Debug, Clone, Copy)]
struct CharCell {
    ch: char,
    advance: f32,
    metric: Option<GlyphMetric>,
}

fn is_wrap_space(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

/// 把一行字符切成若干「行内区间」。
///
/// 换行规则刻意做得简单可预测：超宽就断，拉丁文优先断在空格处（空格被吞掉），
/// 中日韩没有空格，就按字符边界断 —— 每个区间一定落在 `char` 边界上，
/// 绝不会把一个多字节 UTF-8 序列劈开。
fn wrap_ranges(cells: &[CharCell], wrap: f32) -> Vec<(usize, usize)> {
    if cells.is_empty() {
        return vec![(0, 0)];
    }
    // `is_finite` 已经罩住 NaN，因此这里直接比较即可（NaN 会先被挡掉）
    if !wrap.is_finite() || wrap <= 0.0 {
        return vec![(0, cells.len())];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut width = 0.0f32;
    let mut last_break: Option<usize> = None;

    while i < cells.len() {
        let advance = cells[i].advance;
        if width + advance > wrap && i > start {
            // 优先断在最近的空格：空格本身不进下一行
            let (brk, next) = match last_break {
                Some(b) if b > start => (b, b + 1),
                _ => (i, i),
            };
            out.push((start, brk));
            start = next;
            i = next;
            width = 0.0;
            last_break = None;
            continue;
        }
        if is_wrap_space(cells[i].ch) {
            last_break = Some(i);
        }
        width += advance;
        i += 1;
    }
    out.push((start, cells.len()));
    out
}

/// 按 [`ScreenText`] 的样式排版整段文本。
///
/// 泛型而非 `&dyn MetricsSource`：这样 `&dyn GlyphSource` 也能直接传进来
/// （`GlyphSource` 有到 `MetricsSource` 的 blanket 实现），不用做 trait upcast。
pub fn layout_text<S: MetricsSource + ?Sized>(src: &S, text: &ScreenText) -> TextLayout {
    let px = sanitize_size(text.size);
    let lm = src.line_metrics(px);
    let natural = lm.natural_height().max(px);
    let line_height = if text.line_height > 0.0 {
        text.line_height * px
    } else {
        natural
    };
    let wrap = if text.wrap.is_finite() && text.wrap > 0.0 {
        text.wrap
    } else {
        0.0
    };

    let mut missing = 0usize;
    let mut glyph_count = 0usize;
    let mut lines: Vec<LaidOutLine> = Vec::new();
    let mut cells: Vec<CharCell> = Vec::new();

    for raw in text.content.split('\n') {
        cells.clear();
        for ch in raw.chars() {
            let metric = src.glyph_metric(ch, px);
            let advance = metric
                .map(|m| m.advance)
                .filter(|a| a.is_finite() && *a >= 0.0)
                .unwrap_or(px * TOFU_ADVANCE_RATIO);
            cells.push(CharCell {
                ch,
                advance,
                metric,
            });
        }

        for (start, end) in wrap_ranges(&cells, wrap) {
            let mut line = LaidOutLine::default();
            for cell in &cells[start..end] {
                if let Some(m) = cell.metric {
                    line.glyphs.push(PlacedGlyph {
                        ch: cell.ch,
                        x: line.width,
                        advance: cell.advance,
                        face: m.face,
                        glyph: m.glyph,
                    });
                } else {
                    missing += 1;
                }
                line.width += cell.advance;
            }
            glyph_count += line.glyphs.len();
            line.baseline = lm.ascent + lines.len() as f32 * line_height;
            lines.push(line);
        }
    }

    let width = lines.iter().fold(0.0f32, |acc, l| acc.max(l.width));
    let block_width = if wrap > 0.0 { wrap.max(width) } else { width };
    for line in &mut lines {
        let offset = match text.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => ((block_width - line.width) * 0.5).max(0.0),
            TextAlign::Right => (block_width - line.width).max(0.0),
        };
        if offset != 0.0 {
            for glyph in &mut line.glyphs {
                glyph.x += offset;
            }
        }
    }

    TextLayout {
        height: line_height * lines.len() as f32,
        lines,
        width,
        block_width,
        line_height,
        ascent: lm.ascent,
        missing,
        glyph_count,
    }
}

/// 只量宽高（不关心定位），用于脚本 / 布局预留空间。
pub fn measure_text<S: MetricsSource + ?Sized>(src: &S, content: &str, px: f32) -> TextMeasure {
    let text = ScreenText {
        content: content.to_string(),
        size: px,
        ..Default::default()
    };
    let layout = layout_text(src, &text);
    TextMeasure {
        width: layout.width,
        height: layout.height,
        lines: layout.lines.len(),
        missing: layout.missing,
    }
}

// ---------------------------------------------------------------------------
// 字体集合：真实字体（fontdue），负责字形查找与位图栅格化
// ---------------------------------------------------------------------------

/// 栅格化后的字形位图。
///
/// 刻意不复用 `fontdue::Metrics`：渲染层只需要「多大、贴在哪、覆盖度」，
/// 不该被迫依赖字体库的类型。字形解析的实现细节留在 core 内部。
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBitmap {
    /// 位图宽高（像素），0 表示空字形（如空格）。
    pub width: u16,
    pub height: u16,
    /// 位图左边缘相对笔位置的偏移（向右为正）。
    pub left: i16,
    /// 位图顶边缘相对基线的偏移（**屏幕坐标，向下为正**）。
    pub top: i16,
    /// 前进宽度。
    pub advance: f32,
    /// 单通道覆盖度，行优先，长度 = width * height。
    pub data: Vec<u8>,
}

impl GlyphBitmap {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.data.is_empty()
    }
}

/// 「能排版 + 能出位图」的字体来源。
///
/// 渲染层依赖这个抽象拿字形，测试里可以塞一个假实现，不需要真的装字体。
/// 所有 `GlyphSource` 都自动获得 [`MetricsSource`] 实现（见下方 blanket impl），
/// 因此可以直接传给 [`layout_text`] / [`measure_text`]。
pub trait GlyphSource {
    /// 字符 → 字形度量；`None` 表示所有字体面都缺这个字。
    fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric>;
    /// 按 (字体面, 字形索引) 栅格化位图。
    fn glyph_bitmap(&self, face: u16, glyph: u16, px: f32) -> Option<GlyphBitmap>;
    /// 行度量；默认给保守估算。
    fn line_metrics(&self, px: f32) -> LineMetrics {
        LineMetrics::fallback(px)
    }
}

impl<T: GlyphSource + ?Sized> MetricsSource for T {
    fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric> {
        GlyphSource::glyph_metric(self, ch, px)
    }

    fn line_metrics(&self, px: f32) -> LineMetrics {
        GlyphSource::line_metrics(self, px)
    }
}

/// 一个已加载的字体面。
pub struct FontFace {
    /// 来源文件名（日志与调试用）。
    pub source: String,
    /// TTC 里的集合下标（单独 .ttf 恒为 0）。
    pub collection_index: u32,
    pub font: fontdue::Font,
}

/// 一组字体面，按顺序做**缺字回退**：前面的面没有该字符就往后找。
///
/// 典型用法：第 0 面放中文字体（微软雅黑 / 文泉驿），后面接拉丁或图标字体。
pub struct FontSet {
    faces: Vec<FontFace>,
}

impl FontSet {
    /// 空集合：逐面追加用（典型场景是「主中文字体 + emoji / 图标回退字体」）。
    pub fn empty() -> Self {
        Self { faces: Vec::new() }
    }

    /// 从若干文件加载（第一个文件的所有字面优先）。
    pub fn load_files(paths: &[PathBuf]) -> Result<Self, String> {
        let mut set = Self::empty();
        for path in paths {
            set.add_file(path)?;
        }
        if set.faces.is_empty() {
            return Err("没有加载到任何字体".to_string());
        }
        Ok(set)
    }

    /// 追加一个字体文件（`.ttf` / `.otf` / `.ttc`）。
    ///
    /// `.ttc` 是字体集合，按 0,1,2,3 顺序尝试解析，取第一个成功的字面。
    pub fn add_file(&mut self, path: impl AsRef<Path>) -> Result<usize, String> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| format!("读取字体 {} 失败: {e}", path.display()))?;
        let mut last_err = String::new();
        for collection_index in 0..4u32 {
            match self.push_face(path, bytes.as_slice(), collection_index) {
                Ok(index) => return Ok(index),
                Err(e) => last_err = e,
            }
        }
        Err(format!(
            "解析字体 {} 失败（已尝试 4 个字面）: {last_err}",
            path.display()
        ))
    }

    /// 按**指定字面下标**加载单个字体面。
    ///
    /// `.ttf` / `.otf` 只有 0 号字面；`.ttc`（如 `msyh.ttc` 的「雅黑 / 雅黑 UI」）
    /// 需要指定下标才能取到想要的字面。下标越界时返回错误而不是 panic。
    pub fn add_file_index(
        &mut self,
        path: impl AsRef<Path>,
        collection_index: u32,
    ) -> Result<usize, String> {
        let path = path.as_ref();
        let bytes =
            std::fs::read(path).map_err(|e| format!("读取字体 {} 失败: {e}", path.display()))?;
        self.push_face(path, bytes.as_slice(), collection_index)
            .map_err(|e| {
                format!(
                    "解析字体 {} 的字面 #{collection_index} 失败: {e}",
                    path.display()
                )
            })
    }

    /// 解析单个字面并登记进集合；成功返回它在集合里的下标。
    fn push_face(
        &mut self,
        path: &Path,
        bytes: &[u8],
        collection_index: u32,
    ) -> Result<usize, String> {
        let settings = fontdue::FontSettings {
            collection_index,
            ..Default::default()
        };
        let font = fontdue::Font::from_bytes(bytes, settings).map_err(|e| e.to_string())?;
        let source = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        self.faces.push(FontFace {
            source,
            collection_index,
            font,
        });
        Ok(self.faces.len() - 1)
    }

    /// 按平台常见中文系统字体顺序尝试加载，第一个成功即用。
    ///
    /// 这样引擎在没显式指定 `--font` 时也能直接画中文，而不是一片方框。
    pub fn load_system_default() -> Result<Self, String> {
        let mut tried: Vec<String> = Vec::new();
        for candidate in system_font_candidates() {
            if !candidate.exists() {
                tried.push(format!("{}（不存在）", candidate.display()));
                continue;
            }
            match Self::load_files(std::slice::from_ref(&candidate)) {
                Ok(set) => {
                    log::info!(
                        "文本：使用系统字体 {}（charmap {} 项）",
                        candidate.display(),
                        set.faces[0].font.chars().len()
                    );
                    return Ok(set);
                }
                Err(e) => tried.push(format!("{}（{e}）", candidate.display())),
            }
        }
        Err(format!(
            "未找到可用的系统字体，尝试过：{}",
            tried.join("; ")
        ))
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn face_sources(&self) -> Vec<String> {
        self.faces
            .iter()
            .map(|f| format!("{}#{}", f.source, f.collection_index))
            .collect()
    }

    pub fn face(&self, index: u16) -> Option<&FontFace> {
        self.faces.get(index as usize)
    }

    /// 字符 → (字体面, 字形索引)。所有面都缺字时返回 `None`。
    pub fn resolve(&self, ch: char) -> Option<(u16, u16)> {
        for (index, face) in self.faces.iter().enumerate() {
            let glyph = face.font.lookup_glyph_index(ch);
            // glyph 0 恒为 .notdef（豆腐块），出现它就说明这个面没有该字符
            if glyph != 0 {
                return Some((index as u16, glyph));
            }
        }
        None
    }

    /// 栅格化一个字形（按字形索引，不再查 charmap）。
    pub fn rasterize(&self, face: u16, glyph: u16, px: f32) -> Option<(fontdue::Metrics, Vec<u8>)> {
        let face = self.face(face)?;
        Some(face.font.rasterize_indexed(glyph, px))
    }

    /// 字符 → 度量 + 灰度位图（一步到位）。
    ///
    /// 这是「给定字符与字号，产出位图」的正门：内部先按 [`FontSet::resolve`] 做缺字回退，
    /// 找不到字形时返回 `None`（上层按 tofu 占位），绝不 panic。
    pub fn glyph_for_char(&self, ch: char, px: f32) -> Option<(GlyphMetric, GlyphBitmap)> {
        let metric = GlyphSource::glyph_metric(self, ch, px)?;
        let bitmap = GlyphSource::glyph_bitmap(self, metric.face, metric.glyph, px)?;
        Some((metric, bitmap))
    }

    /// 字体集合里是否有这个字符的字形（等价于 `resolve(ch).is_some()`）。
    pub fn has_char(&self, ch: char) -> bool {
        self.resolve(ch).is_some()
    }

    /// 某个字体面的行度量。
    pub fn line_metrics_of(&self, face: u16, px: f32) -> LineMetrics {
        let Some(face) = self.face(face) else {
            return LineMetrics::fallback(px);
        };
        match face.font.horizontal_line_metrics(px) {
            Some(lm) => LineMetrics {
                ascent: lm.ascent,
                descent: lm.descent,
                line_gap: lm.line_gap,
            },
            None => LineMetrics::fallback(px),
        }
    }
}

impl GlyphSource for FontSet {
    fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric> {
        let (face_index, glyph) = self.resolve(ch)?;
        let face = self.face(face_index)?;
        let metrics = face.font.metrics_indexed(glyph, px);
        Some(GlyphMetric {
            face: face_index,
            glyph,
            advance: metrics.advance_width,
        })
    }

    fn glyph_bitmap(&self, face: u16, glyph: u16, px: f32) -> Option<GlyphBitmap> {
        let (metrics, data) = self.rasterize(face, glyph, px)?;
        Some(GlyphBitmap {
            width: metrics.width.min(u16::MAX as usize) as u16,
            height: metrics.height.min(u16::MAX as usize) as u16,
            left: clamp_i16(metrics.xmin),
            // fontdue 的 ymin 是「基线到位图底边」的距离（向上为正），
            // 屏幕坐标向下为正 → top = -(ymin + height)
            top: clamp_i16(-(metrics.ymin + metrics.height as i32)),
            advance: metrics.advance_width,
            data,
        })
    }

    fn line_metrics(&self, px: f32) -> LineMetrics {
        self.line_metrics_of(0, px)
    }
}

/// 不依赖任何字体文件的**近似度量**。
///
/// 用途：脚本 / 布局代码在拿不到 [`FontSet`]（例如纯逻辑层、无窗口单测）时
/// 也要估一个像样的宽度，用来给 HUD 留位、判断要不要换行。
/// 规则刻意简单可预测：东亚宽字符按 1em，空格 0.28em，其余（拉丁 / 数字）0.55em。
///
/// 注意：**这不是真实字体度量**。真实绘制永远走 [`FontSet`]；
/// 这里只用于估算，误差在 ±15% 量级。
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproxMetrics;

impl ApproxMetrics {
    /// 单个字符的近似前进宽度（em 倍数）。
    pub fn advance_ratio(ch: char) -> f32 {
        if ch == ' ' {
            0.28
        } else if ch == '\t' {
            1.12
        } else if (ch as u32) >= 0x2E80 {
            // CJK 部首扩展起：汉字 / 假名 / 全角标点基本都是方块字
            1.0
        } else {
            0.55
        }
    }
}

impl MetricsSource for ApproxMetrics {
    fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric> {
        Some(GlyphMetric {
            face: 0,
            glyph: 1,
            advance: px * Self::advance_ratio(ch),
        })
    }
}

/// 不依赖字体文件的宽度估算（多行取最宽一行，含 `\n` 语义）。
pub fn estimate_text_width(content: &str, px: f32) -> f32 {
    measure_text(&ApproxMetrics, content, px).width
}

/// 不依赖字体文件的高度估算（行数 × 自然行高）。
pub fn estimate_text_height(content: &str, px: f32) -> f32 {
    measure_text(&ApproxMetrics, content, px).height
}

/// 把任意 i32 安全夹进 i16（超大字号时 bearing 可能越界）。
fn clamp_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// 各平台常见的中文系统字体（按「好看程度」排序）。
pub fn system_font_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/msyhbd.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/msyhl.ttc",
        "C:/Windows/Fonts/Deng.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/segoeui.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ] {
        out.push(PathBuf::from(path));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯算术假字体：CJK 每字 1 em，ASCII 每字 0.5 em，`missing` 里的字视为缺字。
    struct MockFont {
        cjk: f32,
        ascii: f32,
        missing: Vec<char>,
    }

    impl MockFont {
        fn new() -> Self {
            Self {
                cjk: 1.0,
                ascii: 0.5,
                missing: Vec::new(),
            }
        }

        fn without(mut self, chars: &[char]) -> Self {
            self.missing.extend_from_slice(chars);
            self
        }
    }

    impl GlyphSource for MockFont {
        fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric> {
            if self.missing.contains(&ch) {
                return None;
            }
            let ratio = if (ch as u32) >= 0x2E80 {
                self.cjk
            } else {
                self.ascii
            };
            Some(GlyphMetric {
                face: 0,
                glyph: (ch as u32 & 0xFFFF) as u16,
                advance: px * ratio,
            })
        }

        fn glyph_bitmap(&self, face: u16, glyph: u16, px: f32) -> Option<GlyphBitmap> {
            let _ = (face, glyph);
            let side = px.max(1.0).round() as usize;
            Some(GlyphBitmap {
                width: side as u16,
                height: side as u16,
                left: 0,
                top: -(side as i16),
                advance: px,
                data: vec![255; side * side],
            })
        }

        fn line_metrics(&self, px: f32) -> LineMetrics {
            LineMetrics {
                ascent: px * 0.8,
                descent: -px * 0.2,
                line_gap: 0.0,
            }
        }
    }

    #[test]
    fn cjk_advance_is_square_em() {
        let font = MockFont::new();
        let cjk = measure_text(&font, "中文", 40.0);
        assert_eq!(cjk.width, 80.0, "两个全角字 = 2 em");
        assert_eq!(cjk.lines, 1);
        assert_eq!(cjk.missing, 0);

        let ascii = measure_text(&font, "AB", 40.0);
        assert_eq!(ascii.width, 40.0, "两个半角字 = 1 em");

        let mixed = measure_text(&font, "中A", 40.0);
        assert_eq!(mixed.width, 60.0, "全角 + 半角 = 1.5 em");
    }

    #[test]
    fn empty_text_has_one_empty_line_and_zero_size() {
        let font = MockFont::new();
        let layout = layout_text(&font, &ScreenText::new("k", "", 0.0, 0.0, 24.0));
        assert_eq!(layout.lines.len(), 1, "空文本也要有一行，方便定位光标");
        assert_eq!(layout.glyph_count, 0);
        assert_eq!(layout.width, 0.0);
        assert!(layout.height > 0.0, "空文本仍占一行高度");
    }

    #[test]
    fn newline_splits_lines() {
        let font = MockFont::new();
        let layout = layout_text(&font, &ScreenText::new("k", "中文\nAB", 0.0, 0.0, 20.0));
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.lines[0].width, 40.0);
        assert_eq!(layout.lines[1].width, 20.0);
        assert_eq!(layout.glyph_count, 4);
        assert_eq!(layout.width, 40.0, "整段宽度取最长行");
    }

    #[test]
    fn wrap_utf8_multibyte_never_panics_and_keeps_chars() {
        let font = MockFont::new();
        // 字号 20，CJK 一字 20px，ASCII 10px；换行宽度只够一个字
        let text = ScreenText {
            content: "a中😀é中".to_string(),
            size: 20.0,
            wrap: 21.0,
            ..Default::default()
        };
        let layout = layout_text(&font, &text);
        let rendered: String = layout
            .lines
            .iter()
            .flat_map(|l| l.glyphs.iter().map(|g| g.ch))
            .collect();
        assert_eq!(
            rendered, "a中😀é中",
            "换行只能切在 char 边界上，且不能吞字：{rendered}"
        );
        assert!(layout.lines.len() >= 3, "每个字都放不下时应分成多行");
        assert_eq!(layout.missing, 0, "😀 在假字体里也有度量（真实字体才会缺）");
    }

    #[test]
    fn wrap_prefers_space_for_latin() {
        let font = MockFont::new();
        // ASCII 一字 10px，宽度 100 → 一行最多 10 个字符
        let text = ScreenText {
            content: "hello world foo".to_string(),
            size: 20.0,
            wrap: 65.0,
            ..Default::default()
        };
        let layout = layout_text(&font, &text);
        assert!(layout.lines.len() >= 2);
        let first: String = layout.lines[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(first, "hello", "拉丁文应断在空格处而不是硬切单词");
        for line in &layout.lines {
            assert!(
                line.width <= 65.0 + 0.001,
                "每行都不得超过换行宽度：{}",
                line.width
            );
        }
    }

    #[test]
    fn wrap_keeps_oversized_single_char_on_own_line() {
        let font = MockFont::new();
        // 换行宽度比一个字还窄：必须仍然每行一个字，不能死循环也不能吞字
        let text = ScreenText {
            content: "中文".to_string(),
            size: 40.0,
            wrap: 10.0,
            ..Default::default()
        };
        let layout = layout_text(&font, &text);
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.lines[0].glyphs.len(), 1);
        assert_eq!(layout.lines[1].glyphs.len(), 1);
    }

    #[test]
    fn missing_glyph_is_counted_and_reserves_tofu_width() {
        let font = MockFont::new().without(&['中']);
        let text = ScreenText {
            content: "中文".to_string(),
            size: 20.0,
            ..Default::default()
        };
        let layout = layout_text(&font, &text);
        assert_eq!(layout.missing, 1, "缺字必须被统计出来，方便上层告警");
        assert_eq!(layout.glyph_count, 1, "缺字不产生绘制字形");
        assert_eq!(layout.lines[0].glyphs[0].ch, '文');
        let expected = 20.0 + 20.0 * TOFU_ADVANCE_RATIO;
        assert!(
            (layout.width - expected).abs() < 0.001,
            "缺字按 tofu 比例占位，宽度 {}/{}",
            layout.width,
            expected
        );
    }

    #[test]
    fn align_offsets_are_relative_to_wrap_block() {
        let font = MockFont::new();
        let base = ScreenText {
            content: "中".to_string(),
            size: 20.0,
            wrap: 100.0,
            ..Default::default()
        };

        let left = layout_text(&font, &base);
        assert_eq!(left.lines[0].glyphs[0].x, 0.0);

        let mut center = base.clone();
        center.align = TextAlign::Center;
        let centered = layout_text(&font, &center);
        assert!((centered.lines[0].glyphs[0].x - 40.0).abs() < 0.001);

        let mut right = base.clone();
        right.align = TextAlign::Right;
        let righted = layout_text(&font, &right);
        assert!((righted.lines[0].glyphs[0].x - 80.0).abs() < 0.001);
        assert_eq!(righted.block_width, 100.0);
    }

    #[test]
    fn align_without_wrap_uses_longest_line_as_block() {
        let font = MockFont::new();
        let mut text = ScreenText {
            content: "中文\nA".to_string(),
            size: 20.0,
            align: TextAlign::Right,
            ..Default::default()
        };
        text.align = TextAlign::Right;
        let layout = layout_text(&font, &text);
        // 第一行最宽 = 40，占满 block；第二行 10 → 右对齐后 x = 30
        assert_eq!(layout.block_width, 40.0);
        assert_eq!(layout.lines[0].glyphs[0].x, 0.0);
        assert!((layout.lines[1].glyphs[0].x - 30.0).abs() < 0.001);
    }

    #[test]
    fn line_height_multiplier_and_default() {
        let font = MockFont::new();
        let mut text = ScreenText::new("k", "中\n文", 0.0, 0.0, 20.0);
        let natural = layout_text(&font, &text);
        assert_eq!(natural.line_height, 20.0, "自然行高 = ascent - descent");

        text.line_height = 2.0;
        let doubled = layout_text(&font, &text);
        assert_eq!(doubled.line_height, 40.0);
        assert_eq!(doubled.height, 80.0, "两行 × 行高 40");
    }

    #[test]
    fn sanitize_clamps_weird_values() {
        let text = ScreenText {
            size: -5.0,
            wrap: -1.0,
            line_height: f32::NAN,
            color: [2.0, -1.0, f32::INFINITY, 0.5],
            x: f32::NAN,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(text.size, DEFAULT_SIZE);
        assert_eq!(text.wrap, 0.0);
        assert_eq!(text.line_height, 0.0);
        assert_eq!(text.color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(text.x, 0.0);

        assert_eq!(sanitize_size(1.0), MIN_SIZE);
        assert_eq!(sanitize_size(9999.0), MAX_SIZE);
        assert_eq!(sanitize_size(32.0), 32.0);
    }

    #[test]
    fn align_parse_accepts_aliases() {
        assert_eq!(TextAlign::parse("LEFT"), Some(TextAlign::Left));
        assert_eq!(TextAlign::parse("center"), Some(TextAlign::Center));
        assert_eq!(TextAlign::parse("end"), Some(TextAlign::Right));
        assert_eq!(TextAlign::parse("up"), None);
        assert_eq!(TextAlign::Center.name(), "center");
    }

    /// 真实字体验收：机器上有中文系统字体时，确认中文能解析出字形、位图非空。
    ///
    /// 没有字体就跳过（CI 无头环境常见），不会让测试变红。
    #[test]
    fn system_cjk_font_produces_non_empty_bitmaps() {
        let Some(path) = system_font_candidates().into_iter().find(|p| p.exists()) else {
            eprintln!("跳过：本机没有候选中文字体");
            return;
        };
        let set = match FontSet::load_files(std::slice::from_ref(&path)) {
            Ok(set) => set,
            Err(e) => {
                eprintln!("跳过：{} 加载失败（{e}）", path.display());
                return;
            }
        };

        // 只要这个字体能画出「中文」两个字，CJK 路径就是通的
        for ch in ['中', '文'] {
            let (face, glyph) = set.resolve(ch).unwrap_or_else(|| panic!("{ch} 应有字形"));
            let (metrics, bitmap) = set.rasterize(face, glyph, 32.0).expect("栅格化");
            assert!(
                metrics.width > 0 && metrics.height > 0,
                "{} 的位图尺寸不能为 0",
                ch
            );
            assert_eq!(bitmap.len(), metrics.width * metrics.height);
            assert!(bitmap.iter().any(|b| *b > 0), "{} 的位图不能全透明", ch);
        }

        let measure = measure_text(&set, "中文测试", 32.0);
        assert!(measure.width > 0.0 && measure.missing == 0);

        // 私用区字符没有任何字体，必须走缺字分支而不是 panic
        let missing = measure_text(&set, "\u{E000}", 32.0);
        assert_eq!(missing.missing, 1);
    }

    #[test]
    fn line_baseline_and_glyph_offset_follow_line_height() {
        let font = MockFont::new();
        let layout = layout_text(&font, &ScreenText::new("k", "中\nAB\n文", 0.0, 0.0, 20.0));
        // 首行基线 = ascent；之后逐行 +line_height（MockFont 自然行高 = 20）
        assert_eq!(layout.lines[0].baseline, 16.0);
        assert_eq!(layout.lines[1].baseline, 36.0);
        assert_eq!(layout.lines[2].baseline, 56.0);

        assert_eq!(layout.glyph_offset(1, 1), Some((10.0, 36.0)));
        assert_eq!(layout.glyph_offset(9, 0), None);
        assert_eq!(layout.glyph_offset(0, 9), None);
    }

    #[test]
    fn empty_font_set_reports_no_glyphs_instead_of_panicking() {
        let set = FontSet::empty();
        assert_eq!(set.face_count(), 0);
        assert!(!set.has_char('中'));
        assert!(set.glyph_for_char('中', 24.0).is_none());
        assert!(set.resolve('中').is_none());

        // 空集合也能排版：全部字符算缺字，宽度按 tofu 估算
        let layout = layout_text(&set, &ScreenText::new("k", "中A", 0.0, 0.0, 20.0));
        assert_eq!(layout.missing, 2);
        assert_eq!(layout.glyph_count, 0);
        assert!(layout.width > 0.0, "缺字仍要占位，不能塌成 0 宽");
    }
}
