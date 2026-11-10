//! 遮挡剔除（扩展二十二）的 CPU 侧判定。
//!
//! 视锥剔除（[`crate::frustum`]）只能扔掉「镜头外」的物件；镜头里被前面的墙
//! 完全挡住的物件照样会进实例缓冲、跑完整条顶点 + 片元流水线，最后一个像素不剩
//! 地被深度测试丢掉。遮挡剔除要做的就是把这类「画了也看不见」的物件提前摘掉。
//!
//! 这里实现的是 NVIDIA 遮挡查询式两阶段的 **CPU 参照版**：不向 GPU 发查询，
//! 而是把**上一帧**深度图（深度预通道写的单采样 `Depth32Float`）压成 tile 金字塔
//! （HZB 的保守替代），再用它对本帧的 AABB 做屏幕空间遮挡测试。
//!
//! * **阶段一（粗筛）**：[`project_aabb`] 把世界 AABB 的 8 个角点投影到屏幕，得到
//!   包围矩形与「最近深度」；[`DepthPyramid::is_occluded`] 用矩形覆盖的 tile 判定：
//!   只要有一个 tile 的**最远**深度比物件最近点还近，就说明那里有更近的表面、
//!   物件可能露出来 → 「可能可见」。
//! * **阶段二（复核）**：[`test_aabb`] 对粗筛判为遮挡的候选，用**不外扩**、
//!   **深度偏置放大 4 倍**的严格参数再测一次；两次都判遮挡才真剔除，否则回退绘制
//!   （[`OcclusionVerdict::Rejected`]）。金字塔只有「最远深度」，替不了一个像素一个
//!   像素的解析，边缘物件、细长物、相机刚转过来的一帧都可能被误判——复核就是兜底。
//!
//! 保守性约定（宁可多画，不可让物件凭空消失）：
//!
//! * 金字塔为空 / 尺寸对不上（首帧、刚 resize）→ 一律判可见；
//! * AABB 8 角点里只要有 `w <= 0`（跨近平面 / 在相机背后）→ 直接 fail-open；
//! * 矩形完全落在屏幕外 → 交给视锥，判可见；
//! * 深度比较用 NDC 0..1（wgpu 约定，近 = 0），与深度预通道写出的值同域。
//!
//! 本模块零 GPU 依赖：金字塔是纯 `Vec<f32>`，判定是纯算术，因此全部可单测。

use crate::aabb::Aabb;
use crate::shadow_cull::{CasterTree, LevelCounts};
use glam::{Mat4, Vec2, Vec3, Vec4};
use serde::{Deserialize, Serialize};

/// 缺省 tile 边长（像素）。8 让 640×360 压成 80×45 个 tile，单次判定只需几十次比较。
pub const DEFAULT_TILE_SIZE: u32 = 8;

/// tile 边长下限：1 像素的 tile 等于不压缩，判定成本退化成扫像素。
pub const MIN_TILE_SIZE: u32 = 2;

/// tile 边长上限：32 像素的 tile 在 1080p 下只剩 60×34，遮挡关系会被糊掉。
pub const MAX_TILE_SIZE: u32 = 32;

/// 缺省深度偏置：相对**剩余深度余量**（`1 - nearest`）的比例。
///
/// 不用 NDC 绝对差值的原因：透视投影的 NDC 深度在远处被压得极扁——相机在 12 米
/// 处时物件与墙面只差 1e-4 量级，而近处的两个相邻表面能差 0.1。固定绝对偏置在
/// 远处会一口吃掉全部可剔机会（实测 0.01 的绝对偏置让所有候选全部判可见）。
/// 按 `(1 - nearest)` 缩放后，偏置等价于「视空间深度的相对误差」，远近一致。
pub const DEFAULT_DEPTH_BIAS: f32 = 0.05;

/// 深度偏置上限：0.5 已经等于要求遮挡面领先物件半个深度余量，再大就剔不动。
pub const MAX_DEPTH_BIAS: f32 = 0.5;

/// 缺省屏幕外扩（像素）：矩形向外长这么多再测，抵消投影取整误差。
pub const DEFAULT_MARGIN_PX: u32 = 2;

/// 外扩上限：外扩越大越保守（越不剔除），超过 64 像素基本剔不动东西。
pub const MAX_MARGIN_PX: u32 = 64;

/// 缺省单次判定最多遍历的 tile 数：超了就换更粗的一层，防止近处大物件扫全屏。
pub const DEFAULT_MAX_TILES: u32 = 256;

/// 单次判定 tile 数下限。
pub const MIN_MAX_TILES: u32 = 16;

/// 单次判定 tile 数上限。
pub const MAX_MAX_TILES: u32 = 4096;

/// 金字塔最大层数：每层边长翻倍，8 层足以从 2 像素 tile 收敛到 1 个 tile。
pub const MAX_PYRAMID_LEVELS: u32 = 8;

/// 复核阶段的深度偏置放大倍数：比粗筛严格 4 倍，遮挡间隙明显才真剔除。
const REVIEW_BIAS_SCALE: f32 = 4.0;

/// 场景 `occlusion` 段（扩展二十二）。
///
/// 缺省 [`OcclusionSettings::default`]（`enabled = false`）与场景里完全不写这一段等价：
/// 渲染层不建金字塔、不发深度读回，剔除结果与「只做视锥剔除」逐像素一致。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OcclusionSettings {
    /// 是否启用。缺省关闭——遮挡剔除会改变 GPU 侧提交量，不该在老场景上默认生效。
    #[serde(default)]
    pub enabled: bool,
    /// 深度偏置（相对比例，夹进 `[0, 0.5]`）：tile 里的表面要领先物件最近点
    /// `depth_bias × (1 - nearest)` 才算「更近」，避免同表面浮点抖动被当遮挡间隙。
    #[serde(default = "default_depth_bias")]
    pub depth_bias: f32,
    /// 金字塔 tile 边长（像素，夹进 `[2, 32]`）。
    #[serde(default = "default_tile_size")]
    pub tile_size: u32,
    /// 屏幕外扩（像素，夹进 `[0, 64]`）。
    #[serde(default = "default_margin_px")]
    pub margin_px: u32,
    /// 单次判定最多遍历的 tile 数（夹进 `[16, 4096]`）。
    #[serde(default = "default_max_tiles")]
    pub max_tiles: u32,
}

fn default_depth_bias() -> f32 {
    DEFAULT_DEPTH_BIAS
}

fn default_tile_size() -> u32 {
    DEFAULT_TILE_SIZE
}

fn default_margin_px() -> u32 {
    DEFAULT_MARGIN_PX
}

fn default_max_tiles() -> u32 {
    DEFAULT_MAX_TILES
}

impl Default for OcclusionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            depth_bias: DEFAULT_DEPTH_BIAS,
            tile_size: DEFAULT_TILE_SIZE,
            margin_px: DEFAULT_MARGIN_PX,
            max_tiles: DEFAULT_MAX_TILES,
        }
    }
}

impl OcclusionSettings {
    /// 收敛到合法区间：`NaN` 偏置回落缺省值，其余夹进各自区间。
    pub fn sanitized(self) -> Self {
        Self {
            enabled: self.enabled,
            depth_bias: if self.depth_bias.is_nan() {
                DEFAULT_DEPTH_BIAS
            } else {
                self.depth_bias.clamp(0.0, MAX_DEPTH_BIAS)
            },
            tile_size: self.tile_size.clamp(MIN_TILE_SIZE, MAX_TILE_SIZE),
            margin_px: self.margin_px.min(MAX_MARGIN_PX),
            max_tiles: self.max_tiles.clamp(MIN_MAX_TILES, MAX_MAX_TILES),
        }
    }

    /// 关闭状态：渲染层整条跳过（不建金字塔、不读回深度）。
    pub fn is_neutral(&self) -> bool {
        !self.enabled
    }

    /// 复核阶段用的严格参数：不外扩、深度偏置放大。
    ///
    /// 粗筛为了保守会把矩形往外长一圈；复核反过来收紧，只有「连着外扩都还被完全
    /// 挡住」的候选才真剔除，边缘物件的误剔代价因此压在 0。
    pub fn review(self) -> Self {
        Self {
            enabled: self.enabled,
            depth_bias: (self.depth_bias * REVIEW_BIAS_SCALE).min(MAX_DEPTH_BIAS),
            tile_size: self.tile_size,
            margin_px: 0,
            max_tiles: self.max_tiles,
        }
    }
}

/// 世界 AABB 投影到屏幕后的包围矩形（像素，左上原点）+ 最近的 NDC 深度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    /// AABB 8 角点里最小的 NDC 深度（0 = 近平面，1 = 远平面）。
    pub nearest: f32,
}

impl ScreenBounds {
    pub fn new(min_x: f32, min_y: f32, max_x: f32, max_y: f32, nearest: f32) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            max_x: max_x.max(min_x),
            max_y: max_y.max(min_y),
            nearest,
        }
    }

    /// 矩形宽高（像素）。
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.max_x - self.min_x, self.max_y - self.min_y)
    }
}

/// 单帧遮挡判定的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcclusionVerdict {
    /// 金字塔不可用 / 物件可能露出来：照画。
    Visible,
    /// 粗筛 + 复核都判遮挡：本帧不画。
    Occluded,
    /// 粗筛判遮挡但复核没通过：疑罪从无，回退绘制（计入统计，便于调参）。
    Rejected,
}

impl OcclusionVerdict {
    /// 是否应当真正剔掉这次绘制。
    pub fn is_culled(self) -> bool {
        matches!(self, Self::Occluded)
    }
}

/// 一层 tile 的「最远可见深度」。
#[derive(Debug, Clone, PartialEq)]
struct PyramidLevel {
    cols: u32,
    rows: u32,
    /// 长度 = `cols * rows`；每格是该 tile 覆盖像素里最大的 NDC 深度。
    farthest: Vec<f32>,
}

/// 上一帧深度的 tile 金字塔（HZB 的保守替代）。
///
/// 只存「每 tile 最远深度」而不存完整层级的原因：判定只需要回答「这块屏幕区域里
/// 有没有比物件更近的表面」，最远值就是那个界限。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DepthPyramid {
    width: u32,
    height: u32,
    tile: u32,
    levels: Vec<PyramidLevel>,
}

impl DepthPyramid {
    /// 由一张 `width × height` 的 NDC 深度图建金字塔；数据不足时返回 `None`。
    ///
    /// `depths` 按行优先排列，长度必须 ≥ `width * height`。非有限值（NaN / inf）
    /// 当作 1.0（最远）处理：它们多半是没写过深度的像素，不当成遮挡物才安全。
    pub fn build(width: u32, height: u32, tile: u32, depths: &[f32]) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let need = (width as usize).checked_mul(height as usize)?;
        if depths.len() < need {
            return None;
        }
        let tile = tile.clamp(MIN_TILE_SIZE, MAX_TILE_SIZE);
        let cols = width.div_ceil(tile);
        let rows = height.div_ceil(tile);
        let mut farthest = vec![0.0f32; (cols as usize) * (rows as usize)];
        for y in 0..height {
            let row_base = (y as usize) * (width as usize);
            let tile_row = (y / tile) as usize * cols as usize;
            for x in 0..width {
                let mut d = depths[row_base + x as usize];
                if !d.is_finite() {
                    d = 1.0;
                }
                let idx = tile_row + (x / tile) as usize;
                if d > farthest[idx] {
                    farthest[idx] = d;
                }
            }
        }

        let mut levels = vec![PyramidLevel {
            cols,
            rows,
            farthest,
        }];
        while levels.len() < MAX_PYRAMID_LEVELS as usize {
            let last = levels.last().expect("至少一层");
            if last.cols <= 1 && last.rows <= 1 {
                break;
            }
            let cols = last.cols.div_ceil(2);
            let rows = last.rows.div_ceil(2);
            let mut merged = vec![f32::NEG_INFINITY; (cols as usize) * (rows as usize)];
            for y in 0..last.rows {
                for x in 0..last.cols {
                    let v = last.farthest[(y as usize) * (last.cols as usize) + x as usize];
                    let idx = (y / 2) as usize * (cols as usize) + (x / 2) as usize;
                    if v > merged[idx] {
                        merged[idx] = v;
                    }
                }
            }
            // 合并层用子层最大值：越远越难被当成遮挡物，判定只会更保守
            let farthest = merged
                .into_iter()
                .map(|v| if v.is_finite() { v } else { 1.0 })
                .collect();
            levels.push(PyramidLevel {
                cols,
                rows,
                farthest,
            });
        }

        Some(Self {
            width,
            height,
            tile,
            levels,
        })
    }

    /// 源深度图尺寸。
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 金字塔层数（0 表示未构建）。
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// 0 层的 tile 边长。
    pub fn tile_size(&self) -> u32 {
        self.tile
    }

    /// 是否可用（尺寸与 0 层都合理）。
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .levels
                .first()
                .is_some_and(|l| l.cols > 0 && l.rows > 0 && !l.farthest.is_empty())
    }

    /// 第 `level` 层 `(col, row)` 格的最远深度；越界返回 `None`。
    pub fn tile_farthest(&self, level: usize, col: u32, row: u32) -> Option<f32> {
        let l = self.levels.get(level)?;
        if col >= l.cols || row >= l.rows {
            return None;
        }
        Some(l.farthest[(row as usize) * (l.cols as usize) + col as usize])
    }

    /// 粗筛：矩形覆盖的所有 tile 里，是否每块都有「比物件最近点还近」的表面。
    ///
    /// 返回 `true` 表示矩形范围内的表面整体都比物件更近 → 大概率被完全挡住。
    /// 覆盖 tile 数超过 `settings.max_tiles` 时逐层上浮，用更粗的一层（合并层取
    /// 最大值，仍然保守）。
    pub fn is_occluded(&self, bounds: &ScreenBounds, settings: &OcclusionSettings) -> bool {
        if !self.is_valid() {
            return false;
        }
        let Some((level_index, range)) = self.tile_range(bounds, settings) else {
            return false;
        };
        let Some(level) = self.levels.get(level_index) else {
            return false;
        };
        // 偏置按剩余深度余量缩放：NDC 深度在透视投影下非线性，用绝对差值会让远处
        // 的候选全部落回「可见」。near 处余量大、偏置也大，far 处同步收窄。
        let bias = settings.depth_bias * (1.0 - bounds.nearest);
        let threshold = bounds.nearest - bias;
        if !threshold.is_finite() {
            return false;
        }
        for row in range.row0..=range.row1 {
            for col in range.col0..=range.col1 {
                let farthest =
                    level.farthest[(row as usize) * (level.cols as usize) + col as usize];
                // tile 里存在「不比物件更近」的表面 → 物件可能在那里露出来
                if farthest > threshold {
                    return false;
                }
            }
        }
        true
    }

    /// 计算 `bounds` 覆盖的 tile 范围与所选层号。
    fn tile_range(
        &self,
        bounds: &ScreenBounds,
        settings: &OcclusionSettings,
    ) -> Option<(usize, TileRange)> {
        let margin = settings.margin_px as f32;
        // 退化矩形（宽或高为 0）说明 AABB 被压成了一条线 / 一个点，覆盖哪些像素
        // 说不准，fail-open 更安全；外扩只用于「有面积的矩形再长一圈」。
        if bounds.max_x <= bounds.min_x || bounds.max_y <= bounds.min_y {
            return None;
        }
        let min_x = (bounds.min_x - margin).clamp(0.0, self.width as f32);
        let max_x = (bounds.max_x + margin).clamp(0.0, self.width as f32);
        let min_y = (bounds.min_y - margin).clamp(0.0, self.height as f32);
        let max_y = (bounds.max_y + margin).clamp(0.0, self.height as f32);
        // 完全在屏幕外或退化：交给视锥判定，这里一律「可见」
        if max_x <= min_x || max_y <= min_y {
            return None;
        }
        let max_tiles = settings
            .max_tiles
            .clamp(MIN_MAX_TILES, MAX_MAX_TILES)
            .max(1);
        let mut chosen = self.levels.len() - 1;
        for (index, level) in self.levels.iter().enumerate() {
            let cell = self.tile as f32 * (1u32 << index) as f32;
            let range = Self::range_for(min_x, min_y, max_x, max_y, cell, level.cols, level.rows);
            let count = (range.col1 - range.col0 + 1) * (range.row1 - range.row0 + 1);
            if count <= max_tiles {
                chosen = index;
                break;
            }
        }
        let level = &self.levels[chosen];
        let cell = self.tile as f32 * (1u32 << chosen) as f32;
        let range = Self::range_for(min_x, min_y, max_x, max_y, cell, level.cols, level.rows);
        Some((chosen, range))
    }

    fn range_for(
        min_x: f32,
        min_y: f32,
        max_x: f32,
        max_y: f32,
        cell: f32,
        cols: u32,
        rows: u32,
    ) -> TileRange {
        let last_col = cols as i64 - 1;
        let last_row = rows as i64 - 1;
        let col0 = ((min_x / cell).floor() as i64).clamp(0, last_col) as u32;
        let col1 = (((max_x / cell).ceil() as i64) - 1).clamp(0, last_col) as u32;
        let row0 = ((min_y / cell).floor() as i64).clamp(0, last_row) as u32;
        let row1 = (((max_y / cell).ceil() as i64) - 1).clamp(0, last_row) as u32;
        TileRange {
            col0,
            col1: col1.max(col0),
            row0,
            row1: row1.max(row0),
        }
    }
}

/// `tile_range` 覆盖的 tile 下标区间（闭区间）。
#[derive(Debug, Clone, Copy)]
struct TileRange {
    col0: u32,
    col1: u32,
    row0: u32,
    row1: u32,
}

/// 把世界 AABB 投影成屏幕矩形 + 最近深度。
///
/// 8 个角点逐个投到 NDC：`clip = vp * (p, 1)`，`ndc = clip.xyz / clip.w`。
/// 只要有一个角点 `w <= 0`（跨近平面 / 在相机背后），整个投影就不可信，
/// 直接返回 `None` —— 调用方按「可见」处理（fail-open）。
pub fn project_aabb(
    aabb: &Aabb,
    view_projection: &Mat4,
    width: u32,
    height: u32,
) -> Option<ScreenBounds> {
    if aabb.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    let mut nearest = f32::INFINITY;
    for corner in 0..8u32 {
        let p = Vec3::new(
            if corner & 1 == 0 {
                aabb.min.x
            } else {
                aabb.max.x
            },
            if corner & 2 == 0 {
                aabb.min.y
            } else {
                aabb.max.y
            },
            if corner & 4 == 0 {
                aabb.min.z
            } else {
                aabb.max.z
            },
        );
        let clip: Vec4 = *view_projection * p.extend(1.0);
        // 近平面附近 w 趋近 0，除法会把坐标炸到无穷；阈值比 0 略大以免除出 inf
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if !ndc.is_finite() {
            return None;
        }
        min = min.min(Vec2::new(ndc.x, ndc.y));
        max = max.max(Vec2::new(ndc.x, ndc.y));
        nearest = nearest.min(ndc.z);
    }
    if !nearest.is_finite() {
        return None;
    }
    // NDC（-1..1，y 向上）→ 像素（0..宽高，y 向下）
    let to_px = |v: Vec2, w: u32, h: u32| {
        Vec2::new((v.x * 0.5 + 0.5) * w as f32, (0.5 - v.y * 0.5) * h as f32)
    };
    let a = to_px(min, width, height);
    let b = to_px(max, width, height);
    Some(ScreenBounds::new(a.x, a.y, b.x, b.y, nearest))
}

/// 两阶段判定：粗筛 + 复核。
///
/// 阶段一用 `settings` 判；粗筛通过后再用 [`OcclusionSettings::review`] 收紧一次。
/// 金字塔不可用 / AABB 投影失败时一律 [`OcclusionVerdict::Visible`]。
pub fn test_aabb(
    pyramid: &DepthPyramid,
    aabb: &Aabb,
    view_projection: &Mat4,
    width: u32,
    height: u32,
    settings: &OcclusionSettings,
) -> OcclusionVerdict {
    if settings.is_neutral() || !pyramid.is_valid() {
        return OcclusionVerdict::Visible;
    }
    let Some(bounds) = project_aabb(aabb, view_projection, width, height) else {
        return OcclusionVerdict::Visible;
    };
    if !pyramid.is_occluded(&bounds, settings) {
        return OcclusionVerdict::Visible;
    }
    // 阶段二：收紧参数复核，避免边缘 / 细长物被误剔
    if pyramid.is_occluded(&bounds, &settings.review()) {
        OcclusionVerdict::Occluded
    } else {
        OcclusionVerdict::Rejected
    }
}

/// 遮挡剔除的层级预筛结果（扩展三十）。
///
/// 与 [`crate::shadow_cull::FrustumCullPlan`] 同构：掩码按 [`CasterTree`] 节点下标，
/// `true` = 该节点的实例本帧照画（没被遮挡）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcclusionCullPlan {
    /// 按 [`CasterTree`] 节点下标的「照画」标记。
    pub mask: Vec<bool>,
    /// 判定成本与省下的量（`pruned` = 没做实例级判定就被判遮挡的实例数）。
    pub counts: LevelCounts,
    /// 因遮挡被剔掉的实例数（= 掩码为 `false` 的节点实例数之和）。
    pub occluded: u32,
}

impl OcclusionCullPlan {
    /// 某节点是否照画。越界（节点不在表里）按**照画**处理 —— 掩码错位的代价是
    /// 多画一个物件，而不是物件凭空消失。
    pub fn keeps(&self, node: usize) -> bool {
        self.mask.get(node).copied().unwrap_or(true)
    }
}

/// 遮挡剔除的层级预筛（扩展三十）：把「逐实例判 [`test_aabb`]」换成「按子树下探」。
///
/// 复用 [`crate::shadow_cull::CasterTree`] 的自顶向下逻辑：子树盒（枝内所有实例盒
/// 的并集）被判遮挡就整枝跳过，枝内实例连一次金字塔查询都不做。省下的是
/// `project_aabb` 的 8 次投影 + 两层 tile 遍历 —— 遮挡判定里最贵的一段。
///
/// 保守性：子盒 `A ⊆ B` 时，`B` 的屏幕矩形包含 `A` 的矩形、`A` 的最近深度不小于
/// `B` 的（`A` 更靠后），偏置 `bias = d × (1 - nearest)` 与阈值
/// `nearest - bias = nearest × (1 + d) - d` 都随 `nearest` 单调，因此
/// **`B` 判遮挡 ⇒ `A` 判遮挡**，整枝跳过不会误剔。
///
/// 唯一不完全一致的情形是 [`test_aabb`] 自己 **fail-open** 的那些实例（投影跨
/// 近平面 / 矩形退化成线 / 完全落在屏幕外）：它们单独判定时一律「可见」，层级却
/// 可能随子树盒一起被剔。这类实例的屏幕覆盖面积恒为 0 —— 一个投影面积为 0 的
/// AABB 里装不出投影面积非 0 的三角形 —— 光栅化本来就不产生片元，所以**画面逐
/// 像素不变**。差分测试 [`plan_occlusion_matches_per_instance`] 把这条口径钉住。
///
/// 调用方约定：本函数只负责遮挡，**不含视锥**。先过视锥再进这里，「完全在屏幕外」
/// 这一族 fail-open 根本到不了；金字塔没就绪 / 开关关闭时直接返回全保留。
pub fn plan_occlusion(
    tree: &CasterTree,
    pyramid: &DepthPyramid,
    view_projection: &Mat4,
    width: u32,
    height: u32,
    settings: &OcclusionSettings,
) -> OcclusionCullPlan {
    if settings.is_neutral() || !pyramid.is_valid() || tree.is_empty() {
        return OcclusionCullPlan {
            mask: vec![true; tree.len()],
            counts: LevelCounts::default(),
            occluded: 0,
        };
    }
    let mut mask = vec![false; tree.len()];
    let counts = tree.descend(&mut mask, |aabb| {
        !test_aabb(pyramid, aabb, view_projection, width, height, settings).is_culled()
    });
    let occluded = tree
        .nodes()
        .iter()
        .enumerate()
        .filter(|(index, _)| !mask[*index])
        .map(|(_, node)| node.casts)
        .fold(0u32, u32::saturating_add);
    OcclusionCullPlan {
        mask,
        counts,
        occluded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow_cull::CasterNode;

    /// 单位立方体。
    fn unit_aabb() -> Aabb {
        Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5))
    }

    fn enabled() -> OcclusionSettings {
        OcclusionSettings {
            enabled: true,
            ..OcclusionSettings::default()
        }
    }

    /// 俯视相机：位于 +Z，看向原点，右手系 + wgpu 深度区间 [0, 1]。
    fn camera() -> Mat4 {
        let mut cam = crate::camera::Camera::new(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        cam.aspect = 1.0;
        cam.view_projection()
    }

    /// 全屏深度都等于 `d` 的金字塔。
    fn flat_pyramid(w: u32, h: u32, d: f32) -> DepthPyramid {
        let depths = vec![d; (w as usize) * (h as usize)];
        DepthPyramid::build(w, h, DEFAULT_TILE_SIZE, &depths).expect("金字塔")
    }

    #[test]
    fn settings_default_is_disabled_and_neutral() {
        let s = OcclusionSettings::default();
        assert!(!s.enabled);
        assert!(s.is_neutral());
    }

    #[test]
    fn settings_sanitized_clamps_ranges() {
        let s = OcclusionSettings {
            enabled: true,
            depth_bias: f32::NAN,
            tile_size: 0,
            margin_px: 999,
            max_tiles: 1,
        }
        .sanitized();
        assert_eq!(s.depth_bias, DEFAULT_DEPTH_BIAS);
        assert_eq!(s.tile_size, MIN_TILE_SIZE);
        assert_eq!(s.margin_px, MAX_MARGIN_PX);
        assert_eq!(s.max_tiles, MIN_MAX_TILES);

        let big = OcclusionSettings {
            enabled: true,
            depth_bias: 10.0,
            tile_size: 999,
            margin_px: 0,
            max_tiles: 99_999,
        }
        .sanitized();
        assert_eq!(big.depth_bias, MAX_DEPTH_BIAS);
        assert_eq!(big.tile_size, MAX_TILE_SIZE);
        assert_eq!(big.max_tiles, MAX_MAX_TILES);
    }

    #[test]
    fn settings_review_is_stricter() {
        let s = OcclusionSettings {
            enabled: true,
            depth_bias: 0.001,
            margin_px: 8,
            ..OcclusionSettings::default()
        };
        let r = s.review();
        assert_eq!(r.margin_px, 0, "复核不外扩");
        assert!(r.depth_bias > s.depth_bias, "复核偏置更大 = 更难判遮挡");
        assert_eq!(r.tile_size, s.tile_size);
    }

    #[test]
    fn settings_json_roundtrip_with_defaults() {
        // 老场景只写 enabled：其余字段应取缺省，不需要手工迁移
        let s: OcclusionSettings = serde_json::from_str(r#"{"enabled":true}"#).expect("解析");
        assert!(s.enabled);
        assert_eq!(s.tile_size, DEFAULT_TILE_SIZE);
        assert_eq!(s.margin_px, DEFAULT_MARGIN_PX);
        assert!((s.depth_bias - DEFAULT_DEPTH_BIAS).abs() < 1e-6);
    }

    #[test]
    fn empty_pyramid_is_never_occluded() {
        let p = DepthPyramid::default();
        assert!(!p.is_valid());
        assert_eq!(p.level_count(), 0);
        let bounds = ScreenBounds::new(0.0, 0.0, 64.0, 64.0, 0.5);
        assert!(!p.is_occluded(&bounds, &enabled()));
    }

    #[test]
    fn build_rejects_short_or_degenerate_input() {
        assert!(DepthPyramid::build(0, 64, 8, &[]).is_none());
        assert!(DepthPyramid::build(64, 0, 8, &[]).is_none());
        assert!(
            DepthPyramid::build(64, 64, 8, &[0.5; 8]).is_none(),
            "数据不足"
        );
        let ok = DepthPyramid::build(64, 64, 8, &[0.5; 4096]).expect("足够的数据");
        assert!(ok.is_valid());
        assert_eq!(ok.size(), (64, 64));
        assert_eq!(ok.tile_size(), DEFAULT_TILE_SIZE);
    }

    #[test]
    fn build_clamps_tile_size() {
        let p = DepthPyramid::build(32, 32, 1, &[0.5; 1024]).expect("金字塔");
        assert_eq!(p.tile_size(), MIN_TILE_SIZE);
        let p = DepthPyramid::build(256, 256, 1024, &[0.5; 65536]).expect("金字塔");
        assert_eq!(p.tile_size(), MAX_TILE_SIZE);
    }

    #[test]
    fn build_takes_farthest_per_tile() {
        // 8×8 → 1 个 tile；最远值必须是 0.9 而不是 0.1
        let mut depths = vec![0.1f32; 64];
        depths[17] = 0.9;
        let p = DepthPyramid::build(8, 8, 8, &depths).expect("金字塔");
        assert_eq!(p.tile_farthest(0, 0, 0), Some(0.9));
    }

    #[test]
    fn build_treats_non_finite_as_farthest() {
        let depths = vec![f32::NAN; 64];
        let p = DepthPyramid::build(8, 8, 8, &depths).expect("金字塔");
        assert_eq!(
            p.tile_farthest(0, 0, 0),
            Some(1.0),
            "NaN 当最远 = 不当遮挡物"
        );
    }

    #[test]
    fn pyramid_levels_shrink_and_stay_conservative() {
        let p = flat_pyramid(64, 32, 0.4);
        assert!(p.level_count() >= 2);
        // 0 层 8×4 tile，1 层 4×2
        assert_eq!(p.tile_farthest(0, 0, 0), Some(0.4));
        assert_eq!(p.tile_farthest(1, 0, 0), Some(0.4));
        // 越界 tile 取不到值
        assert_eq!(p.tile_farthest(0, 99, 0), None);
        assert_eq!(p.tile_farthest(99, 0, 0), None);
    }

    #[test]
    fn pyramid_levels_stop_at_one_tile() {
        let p = flat_pyramid(8, 8, 0.5);
        // 单 tile 金字塔再往上没有可合并的格子
        assert_eq!(p.level_count(), 1);
    }

    #[test]
    fn is_occluded_true_for_closer_flat_surface() {
        // 全屏深度 0.2；物件最近点 0.5 → 前面全是更近的表面
        let p = flat_pyramid(64, 64, 0.2);
        let bounds = ScreenBounds::new(16.0, 16.0, 48.0, 48.0, 0.5);
        assert!(p.is_occluded(&bounds, &enabled()));
    }

    #[test]
    fn is_occluded_false_when_object_is_in_front() {
        // 全屏深度 0.8；物件最近点 0.1 → 物件在更前面
        let p = flat_pyramid(64, 64, 0.8);
        let bounds = ScreenBounds::new(16.0, 16.0, 48.0, 48.0, 0.1);
        assert!(!p.is_occluded(&bounds, &enabled()));
    }

    #[test]
    fn is_occluded_respects_bias() {
        // 深度差 0.001，比缺省偏置（0.05 × 余量 0.5 = 0.025）还小 → 不算遮挡
        let p = flat_pyramid(64, 64, 0.5);
        let bounds = ScreenBounds::new(0.0, 0.0, 64.0, 64.0, 0.501);
        assert!(!p.is_occluded(&bounds, &enabled()), "差距小于偏置要判可见");
        let no_bias = OcclusionSettings {
            depth_bias: 0.0,
            ..enabled()
        };
        assert!(p.is_occluded(&bounds, &no_bias), "零偏置时同一表面也算遮挡");
    }

    #[test]
    fn is_occluded_works_far_from_camera() {
        // 回归：NDC 深度在远处被压扁（0.9955 vs 0.9957 只差 2e-4），
        // 绝对偏置时代这里一律判可见、真机上永远剔不掉任何东西。
        let p = flat_pyramid(64, 64, 0.995_54);
        let bounds = ScreenBounds::new(0.0, 0.0, 64.0, 64.0, 0.995_66);
        let settings = OcclusionSettings {
            depth_bias: 0.01,
            ..enabled()
        };
        assert!(
            p.is_occluded(&bounds, &settings),
            "远处 1e-4 量级的深度差也该能判出遮挡"
        );
        // 偏置按余量缩放：0.5 的绝对值在这里会吃掉整个差距
        let too_big = OcclusionSettings {
            depth_bias: 0.5,
            ..enabled()
        };
        assert!(!p.is_occluded(&bounds, &too_big), "偏置过大要回退到可见");
    }

    #[test]
    fn is_occluded_false_when_any_tile_is_empty() {
        // 只有左上角 tile 有近表面，其余是远背景
        let mut depths = vec![0.9f32; 64 * 64];
        for y in 0..8 {
            for x in 0..8 {
                depths[y * 64 + x] = 0.1;
            }
        }
        let p = DepthPyramid::build(64, 64, 8, &depths).expect("金字塔");
        let bounds = ScreenBounds::new(0.0, 0.0, 64.0, 64.0, 0.5);
        assert!(!p.is_occluded(&bounds, &enabled()), "有 tile 露空就不能剔");
    }

    #[test]
    fn is_occluded_false_outside_screen() {
        let p = flat_pyramid(64, 64, 0.1);
        let bounds = ScreenBounds::new(-200.0, -200.0, -100.0, -100.0, 0.5);
        assert!(!p.is_occluded(&bounds, &enabled()));
        let degenerate = ScreenBounds::new(32.0, 32.0, 32.0, 32.0, 0.5);
        assert!(!p.is_occluded(&degenerate, &enabled()));
    }

    #[test]
    fn is_occluded_uses_coarser_level_past_max_tiles() {
        // 256×256 + 2 像素 tile → 0 层 tile 很多；max_tiles 小则必须上浮层
        let p = flat_pyramid(256, 256, 0.2);
        let bounds = ScreenBounds::new(0.0, 0.0, 256.0, 256.0, 0.9);
        let tight = OcclusionSettings {
            tile_size: MIN_TILE_SIZE,
            max_tiles: MIN_MAX_TILES,
            ..enabled()
        };
        assert!(p.is_occluded(&bounds, &tight), "上浮层后仍应判遮挡");
    }

    #[test]
    fn project_aabb_centers_box_on_screen() {
        let vp = camera();
        let bounds = project_aabb(&unit_aabb(), &vp, 800, 800).expect("投影");
        let center = (bounds.min_x + bounds.max_x) * 0.5;
        assert!((center - 400.0).abs() < 1.0, "中心应落在屏幕中间");
        let cy = (bounds.min_y + bounds.max_y) * 0.5;
        assert!((cy - 400.0).abs() < 1.0);
        assert!(
            bounds.nearest > 0.0 && bounds.nearest < 1.0,
            "深度应落在 NDC 0..1"
        );
        let size = bounds.size();
        assert!(size.x > 0.0 && size.y > 0.0);
    }

    #[test]
    fn project_aabb_rejects_degenerate_input() {
        let vp = camera();
        let empty = Aabb::EMPTY;
        assert!(project_aabb(&empty, &vp, 800, 800).is_none(), "空盒");
        assert!(project_aabb(&unit_aabb(), &vp, 0, 800).is_none(), "零宽");
    }

    #[test]
    fn project_aabb_rejects_box_behind_camera() {
        let vp = camera();
        let behind = Aabb::new(Vec3::new(-0.5, -0.5, 8.0), Vec3::new(0.5, 0.5, 9.0));
        assert!(project_aabb(&behind, &vp, 800, 800).is_none(), "相机背后");
    }

    #[test]
    fn project_aabb_wraps_box_crossing_camera_plane() {
        // 跨过相机位置：有些角点 w <= 0，投影不可信 → fail-open
        let vp = camera();
        let straddle = Aabb::new(Vec3::new(-0.5, -0.5, 3.0), Vec3::new(0.5, 0.5, 7.0));
        assert!(project_aabb(&straddle, &vp, 800, 800).is_none());
    }

    #[test]
    fn test_aabb_visible_when_settings_disabled() {
        let p = flat_pyramid(64, 64, 0.1);
        let vp = camera();
        let verdict = test_aabb(
            &p,
            &unit_aabb(),
            &vp,
            800,
            800,
            &OcclusionSettings::default(),
        );
        assert_eq!(verdict, OcclusionVerdict::Visible, "关闭时必须一颗不剔");
        assert!(!verdict.is_culled());
    }

    #[test]
    fn test_aabb_visible_when_pyramid_missing() {
        let empty = DepthPyramid::default();
        let vp = camera();
        let verdict = test_aabb(&empty, &unit_aabb(), &vp, 800, 800, &enabled());
        assert_eq!(verdict, OcclusionVerdict::Visible);
    }

    #[test]
    fn test_aabb_culls_box_behind_flat_wall() {
        // 全屏深度贴得很近（0.1），立方体在 5 米开外 → 应该被剔
        let p = flat_pyramid(800, 800, 0.1);
        let vp = camera();
        let verdict = test_aabb(&p, &unit_aabb(), &vp, 800, 800, &enabled());
        assert_eq!(verdict, OcclusionVerdict::Occluded);
        assert!(verdict.is_culled());
    }

    #[test]
    fn test_aabb_visible_when_depth_matches_object() {
        // 与物件自身深度同量级：偏置吃掉差距 → 疑罪从无
        let vp = camera();
        let bounds = project_aabb(&unit_aabb(), &vp, 800, 800).expect("投影");
        let p = flat_pyramid(800, 800, bounds.nearest);
        let verdict = test_aabb(&p, &unit_aabb(), &vp, 800, 800, &enabled());
        assert_eq!(verdict, OcclusionVerdict::Visible);
    }

    #[test]
    fn test_aabb_rejects_when_review_fails() {
        // 构造「粗筛过、复核不过」：深度差落在 1×偏置与 4×偏置之间
        let vp = camera();
        let bounds = project_aabb(&unit_aabb(), &vp, 800, 800).expect("投影");
        let settings = OcclusionSettings {
            enabled: true,
            depth_bias: 0.05,
            margin_px: 0,
            ..OcclusionSettings::default()
        };
        // 偏置是相对余量的比例，构造深度差也得跟着余量走
        let margin = 1.0 - bounds.nearest;
        let p = flat_pyramid(800, 800, bounds.nearest - margin * 0.1);
        assert!(p.is_occluded(&bounds, &settings), "粗筛应判遮挡");
        assert!(
            !p.is_occluded(&bounds, &settings.review()),
            "复核偏置 4 倍后应判可见"
        );
        // margin 为 0 时粗筛与复核矩形一致，结论仍应是「回退绘制」
        let verdict = test_aabb(&p, &unit_aabb(), &vp, 800, 800, &settings);
        assert_eq!(verdict, OcclusionVerdict::Rejected);
        assert!(!verdict.is_culled());
    }

    #[test]
    fn is_occluded_false_for_invalid_pyramid() {
        let invalid = DepthPyramid::default();
        let bounds = ScreenBounds::new(0.0, 0.0, 8.0, 8.0, 1.0);
        assert!(!invalid.is_occluded(&bounds, &enabled()));
    }

    // ---- 扩展三十：遮挡剔除的层级预筛 ----

    /// 把一组实例盒装成「一个纯分组根 + 每实例一个叶」的两级树，并刷新成当前盒。
    /// 返回的树里实例 `i` 挂在节点 `i + 1` 上（节点 0 是根，`casts = 0`）。
    fn hierarchy_tree(bounds: &[Aabb]) -> CasterTree {
        let mut nodes = vec![CasterNode::group(None)];
        for &bounds in bounds {
            nodes.push(CasterNode::leaf(bounds).under(0));
        }
        let mut tree = CasterTree::new(nodes);
        let boxes: Vec<Aabb> = std::iter::once(Aabb::EMPTY)
            .chain(bounds.iter().copied())
            .collect();
        let casts: Vec<u32> = std::iter::once(0u32)
            .chain(std::iter::repeat_n(1u32, bounds.len()))
            .collect();
        tree.refresh(&boxes, &casts);
        tree
    }

    /// 一个盒在屏幕上是不是「恒零像素覆盖」—— 即 [`test_aabb`] 会 fail-open 的那族：
    /// 投影失败（跨近平面 / 相机背后）、矩形退化成线、完全落在屏幕外。
    fn covers_no_pixels(bounds: &Aabb, vp: &Mat4, width: u32, height: u32) -> bool {
        let Some(s) = project_aabb(bounds, vp, width, height) else {
            return true;
        };
        s.max_x <= s.min_x
            || s.max_y <= s.min_y
            || s.max_x <= 0.0
            || s.max_y <= 0.0
            || s.min_x >= width as f32
            || s.min_y >= height as f32
    }

    /// 一份混合夹具：视锥内外、远近、退化、跨近平面都在里面。
    fn mixed_boxes() -> Vec<Aabb> {
        vec![
            // 0-2：屏幕中央、远近不同的正常盒
            Aabb::new(Vec3::new(-0.5, -0.5, 2.0), Vec3::new(0.5, 0.5, 3.0)),
            Aabb::new(Vec3::new(-0.4, -0.4, 4.0), Vec3::new(0.4, 0.4, 4.5)),
            Aabb::new(Vec3::new(-0.2, -0.2, 0.5), Vec3::new(0.2, 0.2, 1.0)),
            // 3：屏幕右侧很远，投影落在屏幕外
            Aabb::new(Vec3::new(40.0, -0.5, 2.0), Vec3::new(41.0, 0.5, 3.0)),
            // 4：x 方向压成零厚的薄片 → 投影矩形宽为 0
            Aabb::new(Vec3::new(0.0, -0.5, 2.0), Vec3::new(0.0, 0.5, 3.0)),
            // 5：相机背后（相机在 z = +5）→ 角点 w < 0
            Aabb::new(Vec3::new(-0.5, -0.5, 6.0), Vec3::new(0.5, 0.5, 7.0)),
            // 6：缺 AABB 的 fail-open 巨盒
            crate::shadow_cull::fail_open_bounds(),
        ]
    }

    #[test]
    fn plan_occlusion_matches_per_instance() {
        // 差分：层级预筛与逐实例判定必须一致；唯一允许的差异只出现在
        // 「屏幕覆盖恒为零像素」的实例上（fail-open 族，光栅化不产生片元）。
        let vp = camera();
        let (width, height) = (800u32, 800u32);
        let settings = enabled();
        let bounds = mixed_boxes();
        let tree = hierarchy_tree(&bounds);
        let mut occluded_agree = 0u32;
        // 两份金字塔：一近（全遮）、一远（全不遮）—— 把两种结论都覆盖到
        for depth in [0.01f32, 0.99] {
            let pyramid = flat_pyramid(width, height, depth);
            let plan = plan_occlusion(&tree, &pyramid, &vp, width, height, &settings);
            assert_eq!(plan.mask.len(), tree.len());
            for (index, aabb) in bounds.iter().enumerate() {
                let individual =
                    test_aabb(&pyramid, aabb, &vp, width, height, &settings).is_culled();
                let hierarchical = !plan.keeps(index + 1);
                if individual && hierarchical {
                    occluded_agree += 1;
                }
                if hierarchical && !individual {
                    assert!(
                        covers_no_pixels(aabb, &vp, width, height),
                        "金字塔 {depth} 下实例 {index} 被层级剔掉、逐实例判定却保留，且有像素覆盖"
                    );
                }
                if individual {
                    assert!(
                        hierarchical,
                        "金字塔 {depth} 下实例 {index} 逐实例判遮挡但层级没剔 —— 层级树漏判"
                    );
                }
            }
        }
        // 夹具里至少要有实例在两种路径下都判遮挡，否则这条差分是空转
        assert!(occluded_agree >= 2, "两种路径都判遮挡的实例太少");
    }

    #[test]
    fn plan_occlusion_prunes_multi_instance_branch() {
        // 一整枝远物件都被挡在墙后：根子树盒直接判遮挡，枝内一次实例判定都不做
        let vp = camera();
        let (width, height) = (800u32, 800u32);
        let bounds: Vec<Aabb> = (0..6)
            .map(|i| {
                let z = 1.6 + i as f32 * 0.1;
                Aabb::new(Vec3::new(-0.1, -0.1, z), Vec3::new(0.1, 0.1, z + 0.05))
            })
            .collect();
        let tree = hierarchy_tree(&bounds);
        let pyramid = flat_pyramid(width, height, 0.01);
        let plan = plan_occlusion(&tree, &pyramid, &vp, width, height, &enabled());
        assert_eq!(plan.occluded, bounds.len() as u32, "整枝都被遮挡");
        assert_eq!(
            plan.counts.instance_tests, 0,
            "根子树盒一次判过就不用下探到实例"
        );
        assert_eq!(plan.counts.branch_pruned, bounds.len() as u32);
        assert!(plan.counts.box_tests <= 2, "只需判根子树盒");
    }

    #[test]
    fn plan_occlusion_fails_open_without_pyramid() {
        // 金字塔没就绪 / 开关关闭 → 全保留，与关闭遮挡剔除前逐个一致
        let vp = camera();
        let bounds = mixed_boxes();
        let tree = hierarchy_tree(&bounds);
        for plan in [
            plan_occlusion(&tree, &DepthPyramid::default(), &vp, 800, 800, &enabled()),
            plan_occlusion(
                &tree,
                &flat_pyramid(800, 800, 0.01),
                &vp,
                800,
                800,
                &OcclusionSettings::default(),
            ),
        ] {
            assert!(plan.mask.iter().all(|keep| *keep), "一律照画");
            assert_eq!(plan.occluded, 0);
            assert_eq!(plan.counts, LevelCounts::default());
        }
    }

    #[test]
    fn plan_occlusion_counts_wiring() {
        // 掩码 / 计数 / occluded 三者自洽；越界与空树都要 fail-open
        let vp = camera();
        let (width, height) = (800u32, 800u32);
        let bounds = mixed_boxes();
        let tree = hierarchy_tree(&bounds);
        let pyramid = flat_pyramid(width, height, 0.01);
        let plan = plan_occlusion(&tree, &pyramid, &vp, width, height, &enabled());
        assert_eq!(plan.mask.len(), tree.len());
        // occluded = 掩码为 false 的节点实例数之和
        let expected: u32 = tree
            .nodes()
            .iter()
            .enumerate()
            .filter(|(index, _)| !plan.mask[*index])
            .map(|(_, node)| node.casts)
            .sum();
        assert_eq!(plan.occluded, expected);
        // 判定过的实例 + 没判就剔掉的实例 = 全部实例
        assert_eq!(
            plan.counts.instance_tests + plan.counts.pruned,
            tree.total_casts()
        );
        assert!(plan.counts.branch_pruned <= plan.counts.pruned);
        // 越界下标按照画处理（掩码错位的代价是多画，不是少画）
        assert!(plan.keeps(tree.len()));
        assert!(plan.keeps(usize::MAX));
    }

    #[test]
    fn plan_occlusion_empty_tree_keeps_everything() {
        let vp = camera();
        let tree = CasterTree::default();
        let pyramid = flat_pyramid(64, 64, 0.01);
        let plan = plan_occlusion(&tree, &pyramid, &vp, 64, 64, &enabled());
        assert!(plan.mask.is_empty());
        assert_eq!(plan.occluded, 0);
    }
}
