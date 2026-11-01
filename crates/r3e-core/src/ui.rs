//! 最小 2D UI 元素模型（M1「最小 UI」）。
//!
//! 这一层只有**纯数据 + 纯算术**，不依赖任何图形 API：
//! - [`UiElement`]：场景里的唯一真源，脚本层只改它，渲染层每帧重新解析。
//! - [`Anchor`] / [`UiValue`]：锚点定位与「像素 / 屏幕百分比」两种单位。
//! - [`resolve_rect`]：把元素换算成当帧的像素矩形（窗口 resize 后重算一次即可）。
//!
//! # 坐标系
//! 屏幕像素坐标，原点在左上角，x 向右、y 向下，与文本层（[`crate::text`]）一致。
//!
//! # 同名 key 语义
//! [`UiElement::key`] 唯一，两种写入语义都在 core 里，调用方不要再自己实现：
//! - [`upsert`]：裸全量覆盖。同 key 整体换成新值（顺序不变、不累积），新 key 追加。
//!   适合「程序化重建一份完整元素」的场景。
//! - [`upsert_styled`]：脚本 / 每帧提交用的唯一写点。同 key 走 [`merge_same_key`]——
//!   几何与内容取新值，`layer` / `visible` / `uv` 这类**样式字段沿用旧值**（每帧重发
//!   位置尺寸不该把样式重置掉）。
//!
//! 数量上限见 [`MAX_UI_ELEMENTS`]：只拦新增、不拦覆盖，两种写入都一样。超出返回
//! [`UiUpsert::Rejected`]，由调用方决定怎么对外暴露（脚本层返回 `false` 并记账）。
//!
//! # 九宫格（9-slice）
//! 图片元素的 [`UiElement::slice_margin`] 是「左 / 上 / 右 / 下」四条边的边距，
//! 与 [`UiElement::uv`] 一样支持像素或百分比（百分比以**元素自身**宽 / 高为基准）。
//! 边距求解走 [`slice_margins`]：按元素当帧尺寸夹取（`左 + 右 <= 宽`、`上 + 下 <= 高`，
//! 超出时两边等比缩放），非法值一律折成 `0`。全 `0` 表示关闭九宫格。
//!
//! 本模块只负责「边距是多少像素」，切 9 块四边形由渲染侧的 `plan_ui` 完成——
//! 布局与切分同源，避免边距出现两套算法。
//!
//! # 扩展位（当前不做，结构已留口）
//! - 圆角：`UiElement` 现在没有 radius 字段，加圆角时在渲染侧的 shader 里按局部 uv
//!   做 SDF 即可，无需改动本模块的布局算法。
//! - 新元素类型：在 [`UiKind`] 上加分支，并在渲染侧 `plan_ui` 里补一个四边形生成器。

use serde::{Deserialize, Serialize};

/// 单帧 UI 元素数量上限（防止脚本每帧新建 key 导致场景无限膨胀）。
pub const MAX_UI_ELEMENTS: usize = 1024;

/// 边框元素的默认线宽（新建边框、或提交值非法时回落到这里）。
pub const DEFAULT_BORDER_WIDTH: f32 = 2.0;

/// 锚点：元素矩形在屏幕上的对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// 左上角（默认）
    #[default]
    TopLeft,
    /// 上边中点
    TopCenter,
    /// 右上角
    TopRight,
    /// 左边中点
    MiddleLeft,
    /// 屏幕正中
    Center,
    /// 右边中点
    MiddleRight,
    /// 左下角
    BottomLeft,
    /// 下边中点
    BottomCenter,
    /// 右下角
    BottomRight,
}

impl Anchor {
    /// 解析脚本里写的锚点名（大小写不敏感，接受常见别名）。
    pub fn parse(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Some(match key.as_str() {
            "top_left" | "topleft" | "left_top" | "tl" | "左上" => Self::TopLeft,
            "top_center" | "topcentre" | "topcenter" | "top_middle" | "top" | "tc" | "上中"
            | "上" => Self::TopCenter,
            "top_right" | "topright" | "right_top" | "tr" | "右上" => Self::TopRight,
            "middle_left" | "center_left" | "centre_left" | "middleleft" | "centerleft"
            | "left" | "ml" | "左中" | "左" => Self::MiddleLeft,
            "center" | "centre" | "middle" | "c" | "居中" | "正中" | "中" => Self::Center,
            "middle_right" | "center_right" | "centre_right" | "middleright" | "centerright"
            | "right" | "mr" | "右中" | "右" => Self::MiddleRight,
            "bottom_left" | "bottomleft" | "left_bottom" | "bl" | "左下" => Self::BottomLeft,
            "bottom_center" | "bottomcentre" | "bottomcenter" | "bottom_middle" | "bottom"
            | "bc" | "下中" | "下" => Self::BottomCenter,
            "bottom_right" | "bottomright" | "right_bottom" | "br" | "右下" => Self::BottomRight,
            _ => return None,
        })
    }

    /// 规范化名字（写回脚本 / 日志用）。
    pub fn name(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopCenter => "top_center",
            Self::TopRight => "top_right",
            Self::MiddleLeft => "middle_left",
            Self::Center => "center",
            Self::MiddleRight => "middle_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomCenter => "bottom_center",
            Self::BottomRight => "bottom_right",
        }
    }

    /// 锚点在屏幕上的归一化位置 `(ax, ay)`，取值 `[0, 1]`。
    ///
    /// 定位公式：`x = ax * (screen_w - w) + offset_x`，所以 `ax = 0.5` 是水平居中、
    /// `ax = 1` 是右边缘对齐，锚点会随尺寸自适应。
    pub fn factors(self) -> (f32, f32) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::TopCenter => (0.5, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::MiddleLeft => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::MiddleRight => (1.0, 0.5),
            Self::BottomLeft => (0.0, 1.0),
            Self::BottomCenter => (0.5, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

/// 长度单位：像素或屏幕百分比。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiUnit {
    /// 像素（默认）
    #[default]
    Px,
    /// 屏幕尺寸的百分比，`100` 表示整屏宽 / 整屏高
    Percent,
}

impl UiUnit {
    /// 解析脚本里写的单位名（`px` / `pct` / `percent` / `%`）。
    pub fn parse(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase();
        Some(match key.as_str() {
            "px" | "pixel" | "pixels" | "像素" => Self::Px,
            "pct" | "percent" | "%" | "百分比" => Self::Percent,
            _ => return None,
        })
    }

    /// 规范化名字。
    pub fn name(self) -> &'static str {
        match self {
            Self::Px => "px",
            Self::Percent => "pct",
        }
    }
}

/// 带单位的长度值（偏移或尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UiValue {
    pub value: f32,
    pub unit: UiUnit,
}

impl Default for UiValue {
    fn default() -> Self {
        Self::px(0.0)
    }
}

impl UiValue {
    /// 像素值。
    pub const fn px(value: f32) -> Self {
        Self {
            value,
            unit: UiUnit::Px,
        }
    }

    /// 屏幕百分比（`100` = 整屏）。
    pub const fn pct(value: f32) -> Self {
        Self {
            value,
            unit: UiUnit::Percent,
        }
    }

    /// 换成当帧像素值。`base` 是该轴的屏幕像素尺寸；非法值一律折成 `0`。
    pub fn resolve(self, base: f32) -> f32 {
        if !self.value.is_finite() {
            return 0.0;
        }
        match self.unit {
            UiUnit::Px => self.value,
            UiUnit::Percent => {
                let base = if base.is_finite() && base > 0.0 {
                    base
                } else {
                    0.0
                };
                base * self.value / 100.0
            }
        }
    }

    /// 收敛非法值（NaN / 无穷 → 0），供场景加载与脚本写入共用。
    pub fn sanitized(self) -> Self {
        if self.value.is_finite() {
            self
        } else {
            Self {
                value: 0.0,
                unit: self.unit,
            }
        }
    }
}

/// 元素类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiKind {
    /// 纯色矩形（支持 alpha 混合）
    #[default]
    Rect,
    /// 空心边框（四条边，`border_width` 为粗细）
    Border,
    /// 纹理图片（`image` 为 png 等文件路径，`color` 作为染色 / 回退色）
    Image,
}

impl UiKind {
    /// 解析脚本里写的类型名。
    pub fn parse(name: &str) -> Option<Self> {
        let key = name.trim().to_ascii_lowercase();
        Some(match key.as_str() {
            "rect" | "rectangle" | "矩形" => Self::Rect,
            "border" | "frame" | "边框" => Self::Border,
            "image" | "img" | "texture" | "图片" => Self::Image,
            _ => return None,
        })
    }

    /// 规范化名字。
    pub fn name(self) -> &'static str {
        match self {
            Self::Rect => "rect",
            Self::Border => "border",
            Self::Image => "image",
        }
    }
}

/// 一个屏幕空间 UI 元素。字段复用同一结构（按 `kind` 取用），脚本层改起来最简单。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiElement {
    /// 脚本侧主键。同名重复提交 = 整体覆盖（不会累积）。
    pub key: String,
    pub kind: UiKind,
    /// 锚点
    pub anchor: Anchor,
    /// 相对锚点的偏移；`Percent` 以屏幕宽 / 高为基准
    pub offset: [UiValue; 2],
    /// 元素尺寸；`Percent` 以屏幕宽 / 高为基准
    pub size: [UiValue; 2],
    /// 颜色（RGBA，0..1）；图片元素里用作染色，图片加载失败时作为回退填充色
    pub color: [f32; 4],
    /// 边框粗细（像素），仅 `UiKind::Border` 使用；`<= 0` 不画
    pub border_width: f32,
    /// 图片路径（png / jpeg 等），仅 `UiKind::Image` 使用
    pub image: String,
    /// 图片 uv 子区间 `[u0, v0, u1, v1]`，默认整张；九宫格切分时作为**整体**外边界
    pub uv: [f32; 4],
    /// 九宫格边距 `[左, 上, 右, 下]`，仅 `UiKind::Image` 使用；全 `0` = 关闭九宫格。
    ///
    /// 单位与 [`UiValue`] 一致：`Px` 是元素像素，`Percent` 以元素**自身**宽 / 高为基准
    /// （左右按宽、上下按高）。求解见 [`slice_margins`]：超出元素尺寸时等比夹取，
    /// 所以不会画出负宽度的中间块。
    pub slice_margin: [UiValue; 4],
    /// 层级：小的先画（会被大的盖住）；同层按加入顺序稳定排序
    pub layer: i32,
    /// 是否绘制
    pub visible: bool,
}

impl Default for UiElement {
    fn default() -> Self {
        Self {
            key: String::new(),
            kind: UiKind::Rect,
            anchor: Anchor::TopLeft,
            offset: [UiValue::px(0.0), UiValue::px(0.0)],
            size: [UiValue::px(100.0), UiValue::px(100.0)],
            color: [1.0, 1.0, 1.0, 1.0],
            border_width: DEFAULT_BORDER_WIDTH,
            image: String::new(),
            uv: [0.0, 0.0, 1.0, 1.0],
            slice_margin: [
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
            ],
            layer: 0,
            visible: true,
        }
    }
}

impl UiElement {
    /// 新建一个元素（其余字段走默认值）。
    pub fn new(key: impl Into<String>, kind: UiKind, anchor: Anchor) -> Self {
        Self {
            key: key.into(),
            kind,
            anchor,
            ..Default::default()
        }
    }

    /// 收敛非法值：颜色 / uv 夹到合法区间，粗细与偏移尺寸折成有限值。
    pub fn sanitized(mut self) -> Self {
        for c in self.color.iter_mut() {
            *c = if c.is_finite() {
                c.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        self.offset = [self.offset[0].sanitized(), self.offset[1].sanitized()];
        self.size = [self.size[0].sanitized(), self.size[1].sanitized()];
        self.border_width = if self.border_width.is_finite() {
            self.border_width.max(0.0)
        } else {
            0.0
        };
        let uv = self.uv;
        let (u0, u1) = if uv[0] <= uv[2] {
            (uv[0], uv[2])
        } else {
            (uv[2], uv[0])
        };
        let (v0, v1) = if uv[1] <= uv[3] {
            (uv[1], uv[3])
        } else {
            (uv[3], uv[1])
        };
        let clamp01 = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        self.uv = [clamp01(u0), clamp01(v0), clamp01(u1), clamp01(v1)];
        // 九宫格边距：非有限值折 0，负边距无意义也折 0（单位保留，便于脚本读回意图）
        for m in self.slice_margin.iter_mut() {
            let v = m.sanitized();
            *m = if v.value > 0.0 { v } else { UiValue::px(0.0) };
        }
        self
    }

    /// 本帧的九宫格边距 `[左, 上, 右, 下]`（像素，已按 `rect` 夹取）。
    ///
    /// 只是 [`slice_margins`] 的便捷入口；全 `0` 表示这个元素不切九宫格。
    pub fn slice_margins_px(&self, rect: &UiRect) -> [f32; 4] {
        slice_margins(self.slice_margin, rect)
    }

    /// 本帧是否需要切九宫格（图片元素且四条边距不全为 `0`）。
    pub fn has_slice(&self, rect: &UiRect) -> bool {
        self.kind == UiKind::Image && self.slice_margins_px(rect).iter().any(|m| *m > 0.0)
    }
}

/// 把九宫格边距换算成像素并按元素尺寸夹取。
///
/// - 单位：`Px` 直接用；`Percent` 左右以 `rect.width`、上下以 `rect.height` 为基准
///   （与偏移 / 尺寸「以屏幕为基准」不同——边距描述的是元素内部切分点，跟屏幕无关）。
/// - 非法值（NaN / 无穷 / 负数）一律 `0`。
/// - 夹取：`左 + 右 > 宽` 时两边**等比**缩放到和恰为宽（中心列宽收敛为 0），上下同理。
///   等比而不是逐边截断，是为了保留左右（上下）边距的相对比例，视觉上不偏。
pub fn slice_margins(margin: [UiValue; 4], rect: &UiRect) -> [f32; 4] {
    let base_w = if rect.width.is_finite() && rect.width > 0.0 {
        rect.width
    } else {
        0.0
    };
    let base_h = if rect.height.is_finite() && rect.height > 0.0 {
        rect.height
    } else {
        0.0
    };
    let side = |v: UiValue, base: f32| {
        let px = v.resolve(base);
        if px.is_finite() && px > 0.0 {
            px
        } else {
            0.0
        }
    };
    let mut out = [
        side(margin[0], base_w),
        side(margin[1], base_h),
        side(margin[2], base_w),
        side(margin[3], base_h),
    ];
    shrink_pair(&mut out, 0, 2, base_w);
    shrink_pair(&mut out, 1, 3, base_h);
    out
}

/// 把一对边距（下标 `a` / `b`，如左右、上下）等比压到和不超过 `limit`。
fn shrink_pair(m: &mut [f32; 4], a: usize, b: usize, limit: f32) {
    if limit <= 0.0 {
        m[a] = 0.0;
        m[b] = 0.0;
        return;
    }
    let sum = m[a] + m[b];
    if sum > limit {
        let scale = limit / sum;
        m[a] *= scale;
        m[b] *= scale;
    }
}

/// 解析后的像素矩形（左上角 + 宽高，屏幕像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl UiRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// 尺寸是否可画（宽高必须为正的有限值）。
    pub fn is_drawable(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
}

/// 把元素换算成当帧像素矩形。
///
/// 返回 `None` 的情形（都不 panic，渲染层直接跳过）：
/// - 屏幕尺寸为 0（最小化窗口）；
/// - 尺寸解析后 `<= 0` 或不是有限值（脚本写 0 / 负数 / NaN）。
pub fn resolve_rect(el: &UiElement, screen: (f32, f32)) -> Option<UiRect> {
    let (sw, sh) = screen;
    if !sw.is_finite() || !sh.is_finite() || sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    let w = el.size[0].resolve(sw);
    let h = el.size[1].resolve(sh);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (ax, ay) = el.anchor.factors();
    let ox = el.offset[0].resolve(sw);
    let oy = el.offset[1].resolve(sh);
    let rect = UiRect::new(ax * (sw - w) + ox, ay * (sh - h) + oy, w, h);
    if rect.is_drawable() {
        Some(rect)
    } else {
        None
    }
}

/// 按「层级升序 + 同层保持加入顺序」返回要绘制的元素引用（已过滤 `visible == false`）。
///
/// `sort_by_key` 是稳定排序，所以同 layer 的元素不会互相换位，脚本里
/// 「先加的先画」这一直觉成立。
pub fn sorted_for_draw(elements: &[UiElement]) -> Vec<&UiElement> {
    let mut list: Vec<&UiElement> = elements.iter().filter(|e| e.visible).collect();
    list.sort_by_key(|e| e.layer);
    list
}

/// [`upsert`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiUpsert {
    /// 新 key，追加到末尾
    Inserted,
    /// 同 key，整体覆盖原槽位（顺序不变、不累积）
    Replaced,
    /// 已达 [`MAX_UI_ELEMENTS`]，新增被拒绝（覆盖不受此限）
    Rejected,
}

/// 按 key 查元素（渲染与脚本更新前的定位）。
pub fn find<'a>(elements: &'a [UiElement], key: &str) -> Option<&'a UiElement> {
    elements.iter().find(|e| e.key == key)
}

/// 按 key 查可变元素（脚本 setter / 渲染层写入用）。
pub fn find_mut<'a>(elements: &'a mut [UiElement], key: &str) -> Option<&'a mut UiElement> {
    elements.iter_mut().find(|e| e.key == key)
}

/// 边框线宽收敛：非边框元素一律 `0`（不画）；边框元素非法 / 未给出时回落到默认。
fn normalize_border_width(mut element: UiElement) -> UiElement {
    element.border_width = if element.kind == UiKind::Border {
        if element.border_width.is_finite() && element.border_width > 0.0 {
            element.border_width
        } else {
            DEFAULT_BORDER_WIDTH
        }
    } else {
        0.0
    };
    element
}

/// 同 key 重复提交时的字段合并策略（**core 唯一实现**，脚本层不再自己判断）。
///
/// 分三类处理：
/// 1. **覆盖**（新值说了算）：`kind` / `anchor` / `offset` / `size` / `color` / `image`
///    —— 每次提交都在描述「这一帧长什么样」。
/// 2. **沿用**（旧值说了算）：`layer` / `visible` / `uv` / `slice_margin` —— 样式与开关，
///    不该因为每帧重发几何就被重置；要改它们请走 `ui_set_layer` / `ui_set_visible` /
///    `ui_set_uv` / `ui_set_slice`。（九宫格边距跟 `uv` 同一档：都是「这张纹理怎么贴」，
///    而不是「这一帧画在哪、多大」。）
/// 3. **有则覆盖、无则沿用**：`border_width` —— 边框元素的入参 `> 0` 时取入参，
///    否则沿用旧线宽（旧值也没有就回落到 [`DEFAULT_BORDER_WIDTH`]）；非边框元素恒为 0。
pub fn merge_same_key(existing: &UiElement, incoming: UiElement) -> UiElement {
    // 先记下「提交方到底有没有给出线宽」，再走收敛（收敛会把 0 折成默认值）。
    let is_border = incoming.kind == UiKind::Border;
    let submitted_width = if incoming.border_width.is_finite() {
        incoming.border_width.max(0.0)
    } else {
        0.0
    };
    let incoming = normalize_border_width(incoming);
    let border_width = if !is_border {
        0.0
    } else if submitted_width > 0.0 {
        submitted_width
    } else if existing.border_width > 0.0 {
        existing.border_width
    } else {
        DEFAULT_BORDER_WIDTH
    };
    UiElement {
        layer: existing.layer,
        visible: existing.visible,
        uv: existing.uv,
        slice_margin: existing.slice_margin,
        border_width,
        ..incoming
    }
}

/// 按 key 覆盖式写入：同 key 整体替换原槽位（顺序不变），否则追加到末尾。
///
/// 这是**裸全量覆盖**：`layer` / `visible` / `uv` / `slice_margin` 也会被新值重置。脚本
/// 每帧提交的场景要用 [`upsert_styled`]（样式字段沿用旧值）。上限只拦新增、不拦覆盖——脚本每帧
/// 重发位置尺寸时必须能写进去，否则会被静默丢弃。非法值收敛由调用方先跑
/// [`UiElement::sanitized`]。
pub fn upsert(elements: &mut Vec<UiElement>, element: UiElement) -> UiUpsert {
    if let Some(slot) = elements.iter_mut().find(|e| e.key == element.key) {
        *slot = element;
        return UiUpsert::Replaced;
    }
    if elements.len() >= MAX_UI_ELEMENTS {
        return UiUpsert::Rejected;
    }
    elements.push(element);
    UiUpsert::Inserted
}

/// 脚本入口 / 每帧提交的唯一写点：同 key 按 [`merge_same_key`] 合并，新 key 追加。
///
/// 与 [`upsert`] 的差别只有「同 key 时怎么合并」：这里保留样式字段、并把边框线宽与
/// 非法值一起收敛（插入前跑 [`UiElement::sanitized`]）。数量上限语义与 [`upsert`]
/// 完全一致：只拦新增、不拦覆盖，满载时同 key 更新照样生效。
pub fn upsert_styled(elements: &mut Vec<UiElement>, element: UiElement) -> UiUpsert {
    if let Some(slot) = elements.iter_mut().find(|e| e.key == element.key) {
        let merged = merge_same_key(slot, element.sanitized());
        *slot = merged;
        return UiUpsert::Replaced;
    }
    if elements.len() >= MAX_UI_ELEMENTS {
        return UiUpsert::Rejected;
    }
    elements.push(normalize_border_width(element.sanitized()));
    UiUpsert::Inserted
}

/// 按 key 移除元素；返回是否真的删掉了。
pub fn remove(elements: &mut Vec<UiElement>, key: &str) -> bool {
    match elements.iter().position(|e| e.key == key) {
        Some(idx) => {
            elements.remove(idx);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(kind: UiKind, anchor: Anchor) -> UiElement {
        UiElement::new("k", kind, anchor)
    }

    /// 九个锚点在同一屏幕上的落点必须各自正确（含尺寸参与的自适应）。
    #[test]
    fn anchors_place_rect_correctly() {
        let screen = (1000.0, 600.0);
        let mut e = el(UiKind::Rect, Anchor::TopLeft);
        e.size = [UiValue::px(200.0), UiValue::px(100.0)];

        let cases = [
            (Anchor::TopLeft, 0.0, 0.0),
            (Anchor::TopCenter, 400.0, 0.0),
            (Anchor::TopRight, 800.0, 0.0),
            (Anchor::MiddleLeft, 0.0, 250.0),
            (Anchor::Center, 400.0, 250.0),
            (Anchor::MiddleRight, 800.0, 250.0),
            (Anchor::BottomLeft, 0.0, 500.0),
            (Anchor::BottomCenter, 400.0, 500.0),
            (Anchor::BottomRight, 800.0, 500.0),
        ];
        for (anchor, x, y) in cases {
            e.anchor = anchor;
            let r = resolve_rect(&e, screen).expect("应能解析");
            assert_eq!((r.x, r.y), (x, y), "锚点 {} 落点不对", anchor.name());
        }
    }

    /// 偏移叠加在锚点落点之上，像素偏移不随窗口变化。
    #[test]
    fn pixel_offset_is_added_on_top_of_anchor() {
        let mut e = el(UiKind::Rect, Anchor::TopLeft);
        e.size = [UiValue::px(100.0), UiValue::px(50.0)];
        e.offset = [UiValue::px(16.0), UiValue::px(12.0)];
        let r = resolve_rect(&e, (1280.0, 720.0)).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (16.0, 12.0, 100.0, 50.0));
    }

    /// 百分比尺寸随屏幕变化重算（resize 后不得错位）。
    #[test]
    fn percent_size_recomputes_on_resize() {
        let mut e = el(UiKind::Rect, Anchor::Center);
        e.size = [UiValue::pct(50.0), UiValue::pct(25.0)];

        let small = resolve_rect(&e, (800.0, 600.0)).unwrap();
        assert_eq!((small.width, small.height), (400.0, 150.0));
        assert_eq!((small.x, small.y), (200.0, 225.0));

        let big = resolve_rect(&e, (1600.0, 900.0)).unwrap();
        assert_eq!((big.width, big.height), (800.0, 225.0));
        assert_eq!((big.x, big.y), (400.0, 337.5));
    }

    /// 百分比偏移以屏幕尺寸为基准（不是以剩余空间为基准），语义要固定下来别漂移。
    #[test]
    fn percent_offset_uses_screen_extent() {
        let mut e = el(UiKind::Rect, Anchor::BottomCenter);
        e.size = [UiValue::px(200.0), UiValue::px(40.0)];
        e.offset = [UiValue::pct(0.0), UiValue::pct(-10.0)];
        let r = resolve_rect(&e, (1000.0, 500.0)).unwrap();
        assert_eq!(r.x, 400.0);
        assert_eq!(r.y, 460.0 - 50.0);
    }

    /// 空 / 超界 / 非有限尺寸一律返回 None，绝不 panic。
    #[test]
    fn degenerate_sizes_return_none() {
        let screen = (800.0, 600.0);
        for size in [
            [UiValue::px(0.0), UiValue::px(100.0)],
            [UiValue::px(-5.0), UiValue::px(100.0)],
            [UiValue::px(f32::NAN), UiValue::px(100.0)],
            [UiValue::px(f32::INFINITY), UiValue::px(100.0)],
            [UiValue::px(100.0), UiValue::px(0.0)],
        ] {
            let mut e = el(UiKind::Rect, Anchor::Center);
            e.size = size;
            assert!(
                resolve_rect(&e, screen).is_none(),
                "尺寸 {size:?} 不该被画出来"
            );
        }
        let e = el(UiKind::Rect, Anchor::Center);
        assert!(resolve_rect(&e, (0.0, 0.0)).is_none(), "零尺寸窗口不画");
        assert!(
            resolve_rect(&e, (f32::NAN, 600.0)).is_none(),
            "NaN 窗口不画"
        );
    }

    /// 层级升序绘制，同层保持加入顺序（稳定）。
    #[test]
    fn layer_order_is_stable() {
        let mk = |key: &str, layer: i32| {
            let mut e = el(UiKind::Rect, Anchor::TopLeft);
            e.key = key.to_string();
            e.layer = layer;
            e
        };
        let elements = vec![mk("a", 5), mk("b", 1), mk("c", 5), mk("d", -2), mk("e", 1)];
        let order: Vec<&str> = sorted_for_draw(&elements)
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        assert_eq!(order, vec!["d", "b", "e", "a", "c"]);
    }

    /// 同 key 覆盖式写入：不累积、槽位顺序不变、字段整体换成新值。
    #[test]
    fn same_key_overwrites_in_place() {
        let mk = |key: &str, w: f32| {
            let mut e = el(UiKind::Rect, Anchor::TopLeft);
            e.key = key.to_string();
            e.size = [UiValue::px(w), UiValue::px(10.0)];
            e
        };
        let mut elements = Vec::new();
        assert_eq!(upsert(&mut elements, mk("a", 100.0)), UiUpsert::Inserted);
        assert_eq!(upsert(&mut elements, mk("b", 100.0)), UiUpsert::Inserted);
        assert_eq!(upsert(&mut elements, mk("a", 250.0)), UiUpsert::Replaced);
        assert_eq!(elements.len(), 2, "同 key 不该累积出新元素");
        assert_eq!(elements[0].key, "a");
        assert_eq!(
            elements[0].size[0],
            UiValue::px(250.0),
            "覆盖要写成新值而不是保留旧值"
        );
        assert_eq!(elements[1].key, "b", "覆盖不该打乱槽位顺序");

        assert_eq!(find(&elements, "b").map(|e| e.key.as_str()), Some("b"));
        assert!(find(&elements, "nope").is_none());
        assert!(remove(&mut elements, "a"), "存在的 key 应删得掉");
        assert!(!remove(&mut elements, "a"), "重复删同一个 key 返回 false");
        assert_eq!(elements.len(), 1);

        // 每帧重发 100 次也只留一个槽位（脚本主循环的常见形态）。
        for _ in 0..100 {
            upsert(&mut elements, mk("b", 60.0));
        }
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].size[0], UiValue::px(60.0));
    }

    /// 数量上限只拦新增，同 key 覆盖在满载时仍必须写得进去。
    #[test]
    fn upsert_respects_capacity_but_still_overwrites() {
        let mut elements: Vec<UiElement> = (0..MAX_UI_ELEMENTS)
            .map(|i| {
                let mut e = el(UiKind::Rect, Anchor::TopLeft);
                e.key = format!("k{i}");
                e
            })
            .collect();

        let mut extra = el(UiKind::Rect, Anchor::TopLeft);
        extra.key = "overflow".into();
        assert_eq!(upsert(&mut elements, extra), UiUpsert::Rejected);
        assert_eq!(elements.len(), MAX_UI_ELEMENTS, "超限不该写进去");

        let mut again = el(UiKind::Rect, Anchor::TopLeft);
        again.key = "k0".into();
        again.layer = 7;
        assert_eq!(upsert(&mut elements, again), UiUpsert::Replaced);
        assert_eq!(elements.len(), MAX_UI_ELEMENTS);
        assert_eq!(elements[0].layer, 7, "满载时覆盖仍要生效");
    }

    /// 同 key 合并策略：几何 / 内容取新值，样式字段（layer / visible / uv）沿用旧值。
    #[test]
    fn merge_same_key_keeps_style_fields() {
        let mut old = el(UiKind::Rect, Anchor::TopLeft);
        old.key = "hp".into();
        old.size = [UiValue::px(10.0), UiValue::px(10.0)];
        old.layer = 7;
        old.visible = false;
        old.uv = [0.0, 0.0, 0.5, 1.0];

        let mut incoming = el(UiKind::Rect, Anchor::BottomRight);
        incoming.key = "hp".into();
        incoming.offset = [UiValue::px(4.0), UiValue::px(8.0)];
        incoming.size = [UiValue::pct(50.0), UiValue::px(12.0)];
        incoming.color = [0.1, 0.2, 0.3, 0.4];

        let merged = merge_same_key(&old, incoming);
        assert_eq!(merged.key, "hp");
        assert_eq!(merged.anchor, Anchor::BottomRight, "锚点属于几何，取新值");
        assert_eq!(merged.offset, [UiValue::px(4.0), UiValue::px(8.0)]);
        assert_eq!(merged.size, [UiValue::pct(50.0), UiValue::px(12.0)]);
        assert_eq!(merged.color, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(merged.layer, 7, "层级属于样式，沿用旧值");
        assert!(!merged.visible, "可见性属于样式，沿用旧值");
        assert_eq!(merged.uv, [0.0, 0.0, 0.5, 1.0], "uv 属于样式，沿用旧值");
    }

    /// 边框线宽的三条分支：给出就覆盖、没给就沿用、非边框恒为 0。
    #[test]
    fn merge_same_key_border_width_policy() {
        let mut old = el(UiKind::Border, Anchor::TopLeft);
        old.key = "f".into();
        old.border_width = 6.0;

        let mut explicit = el(UiKind::Border, Anchor::TopLeft);
        explicit.key = "f".into();
        explicit.border_width = 3.0;
        assert_eq!(merge_same_key(&old, explicit).border_width, 3.0);

        let mut unspecified = el(UiKind::Border, Anchor::TopLeft);
        unspecified.key = "f".into();
        unspecified.border_width = 0.0;
        assert_eq!(
            merge_same_key(&old, unspecified).border_width,
            6.0,
            "没给线宽时沿用旧值，不能被重置成默认"
        );

        let fresh = el(UiKind::Border, Anchor::TopLeft);
        assert_eq!(
            merge_same_key(&fresh, el(UiKind::Border, Anchor::TopLeft)).border_width,
            DEFAULT_BORDER_WIDTH,
            "旧值也没给时回落到默认"
        );

        let mut as_rect = el(UiKind::Rect, Anchor::TopLeft);
        as_rect.key = "f".into();
        assert_eq!(
            merge_same_key(&old, as_rect).border_width,
            0.0,
            "换成非边框元素后线宽归零（不参与绘制）"
        );
    }

    /// `upsert_styled`：同 key 覆盖不累积、样式保留；满载时只拒新增、不拒覆盖。
    #[test]
    fn upsert_styled_merges_and_respects_capacity() {
        let mut elements = Vec::new();
        let mut first = el(UiKind::Rect, Anchor::TopLeft);
        first.key = "a".into();
        first.size = [UiValue::px(100.0), UiValue::px(10.0)];
        assert_eq!(upsert_styled(&mut elements, first), UiUpsert::Inserted);
        assert!(find_mut(&mut elements, "a").is_some());
        find_mut(&mut elements, "a").unwrap().layer = 5;

        let mut again = el(UiKind::Rect, Anchor::TopLeft);
        again.key = "a".into();
        again.size = [UiValue::px(250.0), UiValue::px(10.0)];
        assert_eq!(upsert_styled(&mut elements, again), UiUpsert::Replaced);
        assert_eq!(elements.len(), 1, "同 key 不新增槽位");
        assert_eq!(elements[0].size[0], UiValue::px(250.0));
        assert_eq!(elements[0].layer, 5, "样式字段应被保留");

        let mut capped: Vec<UiElement> = (0..MAX_UI_ELEMENTS)
            .map(|i| {
                let mut e = el(UiKind::Rect, Anchor::TopLeft);
                e.key = format!("k{i}");
                e
            })
            .collect();
        let mut overflow = el(UiKind::Rect, Anchor::TopLeft);
        overflow.key = "overflow".into();
        assert_eq!(upsert_styled(&mut capped, overflow), UiUpsert::Rejected);
        assert_eq!(capped.len(), MAX_UI_ELEMENTS, "超限不该写进去");

        let mut update = el(UiKind::Rect, Anchor::TopLeft);
        update.key = "k0".into();
        update.size = [UiValue::px(7.0), UiValue::px(7.0)];
        assert_eq!(upsert_styled(&mut capped, update), UiUpsert::Replaced);
        assert_eq!(capped.len(), MAX_UI_ELEMENTS);
        assert_eq!(capped[0].size[0], UiValue::px(7.0), "满载仍可覆盖");
    }

    /// 插入路径要收敛非法值：NaN 颜色折成 0，边框线宽非法时回落默认。
    #[test]
    fn upsert_styled_sanitizes_incoming() {
        let mut elements = Vec::new();
        let mut e = el(UiKind::Border, Anchor::Center);
        e.key = "b".into();
        e.color = [f32::NAN, 2.0, -1.0, 1.0];
        e.border_width = f32::NAN;
        assert_eq!(upsert_styled(&mut elements, e), UiUpsert::Inserted);
        assert_eq!(elements[0].color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(elements[0].border_width, DEFAULT_BORDER_WIDTH);
    }

    /// 超出屏幕的极端偏移只影响落点，不得 panic、不得产生 NaN。
    #[test]
    fn offscreen_extremes_stay_finite() {
        let mut e = el(UiKind::Rect, Anchor::BottomRight);
        e.size = [UiValue::pct(200.0), UiValue::pct(300.0)];
        e.offset = [UiValue::px(-1.0e6), UiValue::pct(50.0)];
        let r = resolve_rect(&e, (800.0, 600.0)).expect("尺寸合法就该给出矩形");
        assert_eq!((r.width, r.height), (1600.0, 1800.0));
        assert_eq!((r.x, r.y), (-1_000_800.0, -900.0), "超界照算，不夹到屏幕内");
        assert!(r.x.is_finite() && r.y.is_finite());

        // NaN 偏移折成 0（resolve 已收敛），结果依旧有限。
        e.offset = [UiValue::px(f32::NAN), UiValue::px(f32::INFINITY)];
        let r = resolve_rect(&e, (800.0, 600.0)).unwrap();
        assert_eq!((r.x, r.y), (-800.0, -1200.0));
        assert!(!r.x.is_nan() && !r.y.is_nan());
    }

    /// 不可见元素不参与绘制。
    #[test]
    fn hidden_elements_are_filtered() {
        let mut a = el(UiKind::Rect, Anchor::TopLeft);
        a.key = "a".into();
        let mut b = el(UiKind::Rect, Anchor::TopLeft);
        b.key = "b".into();
        b.visible = false;
        let elements = vec![a, b];
        assert_eq!(sorted_for_draw(&elements).len(), 1);
    }

    /// 锚点名解析：大小写 / 别名都要认，错名字返回 None。
    #[test]
    fn anchor_parse_aliases() {
        assert_eq!(Anchor::parse("TopCenter"), Some(Anchor::TopCenter));
        assert_eq!(Anchor::parse(" top-center "), Some(Anchor::TopCenter));
        assert_eq!(Anchor::parse("C"), Some(Anchor::Center));
        assert_eq!(Anchor::parse("右下"), Some(Anchor::BottomRight));
        assert_eq!(Anchor::parse("middle-left"), Some(Anchor::MiddleLeft));
        assert_eq!(Anchor::parse("nowhere"), None);
    }

    /// 单位解析与解析成像素。
    #[test]
    fn unit_parse_and_resolve() {
        assert_eq!(UiUnit::parse("PCT"), Some(UiUnit::Percent));
        assert_eq!(UiUnit::parse("%"), Some(UiUnit::Percent));
        assert_eq!(UiUnit::parse("px"), Some(UiUnit::Px));
        assert_eq!(UiUnit::parse("em"), None);
        assert_eq!(UiValue::pct(50.0).resolve(800.0), 400.0);
        assert_eq!(UiValue::px(50.0).resolve(800.0), 50.0);
        assert_eq!(UiValue::pct(50.0).resolve(0.0), 0.0);
    }

    /// sanitized：颜色 / uv 夹紧，粗细与尺寸收敛到有限值。
    #[test]
    fn sanitize_clamps_and_reorders_uv() {
        let mut e = el(UiKind::Image, Anchor::Center);
        e.color = [2.0, -1.0, f32::NAN, 1.0];
        e.border_width = -3.0;
        e.size = [UiValue::px(f32::NAN), UiValue::px(10.0)];
        e.uv = [0.9, 0.8, 0.1, -0.2];
        let s = e.sanitized();
        assert_eq!(s.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.border_width, 0.0);
        assert_eq!(s.size[0], UiValue::px(0.0));
        assert_eq!(
            s.uv,
            [0.1, 0.0, 0.9, 0.8],
            "uv 应重排成 min/max 并夹到 0..1"
        );
    }

    /// 默认元素可直接序列化 / 反序列化（场景保存需要）。
    #[test]
    fn element_roundtrips_through_json() {
        let mut e = UiElement::new("hp_fill", UiKind::Rect, Anchor::MiddleLeft);
        e.size = [UiValue::pct(40.0), UiValue::px(18.0)];
        e.layer = 3;
        let json = serde_json::to_string(&e).expect("序列化应成功");
        let back: UiElement = serde_json::from_str(&json).expect("反序列化应成功");
        assert_eq!(back, e);
    }

    /// 九宫格边距：像素直接用，百分比以**元素自身**宽 / 高为基准。
    #[test]
    fn slice_margins_use_element_size_for_percent() {
        let r = UiRect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 100.0,
        };
        let px = slice_margins(
            [
                UiValue::px(8.0),
                UiValue::px(4.0),
                UiValue::px(12.0),
                UiValue::px(6.0),
            ],
            &r,
        );
        assert_eq!(px, [8.0, 4.0, 12.0, 6.0]);

        // 左右按宽 200、上下按高 100 换算
        let pct = slice_margins(
            [
                UiValue::pct(10.0),
                UiValue::pct(10.0),
                UiValue::pct(5.0),
                UiValue::pct(20.0),
            ],
            &r,
        );
        assert_eq!(pct, [20.0, 10.0, 10.0, 20.0]);
    }

    /// 边距之和超过元素尺寸时**等比**夹取到恰好撑满，中心块宽高收敛为 0。
    #[test]
    fn slice_margins_shrink_when_oversized() {
        let r = UiRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        // 左右 80 + 40 = 120 > 100 → 按 100/120 等比缩放；上下 30 + 30 = 60 > 40
        let m = slice_margins(
            [
                UiValue::px(80.0),
                UiValue::px(30.0),
                UiValue::px(40.0),
                UiValue::px(30.0),
            ],
            &r,
        );
        assert!((m[0] - 100.0 * 80.0 / 120.0).abs() < 1e-5, "{:?}", m);
        assert!((m[2] - 100.0 * 40.0 / 120.0).abs() < 1e-5, "{:?}", m);
        assert!((m[0] + m[2] - r.width).abs() < 1e-5, "左右之和应恰为宽");
        assert!((m[1] + m[3] - r.height).abs() < 1e-5, "上下之和应恰为高");
        assert!((m[1] - 20.0).abs() < 1e-5 && (m[3] - 20.0).abs() < 1e-5);

        // 等比缩放保持两边比例（80:40 仍是 2:1）
        assert!((m[0] / m[2] - 2.0).abs() < 1e-4);
    }

    /// 非法边距（NaN / 负 / 无穷）与零尺寸元素一律收敛为 0，不产出负数。
    #[test]
    fn slice_margins_reject_invalid_values() {
        let r = UiRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let m = slice_margins(
            [
                UiValue::px(f32::NAN),
                UiValue::px(-12.0),
                UiValue::px(f32::INFINITY),
                UiValue::pct(-5.0),
            ],
            &r,
        );
        assert_eq!(m, [0.0, 0.0, 0.0, 0.0]);

        // 零尺寸元素：百分比基数为 0，边距也只能是 0
        let flat = UiRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        assert_eq!(
            slice_margins([UiValue::pct(10.0); 4], &flat),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    /// sanitized 把负 / 非有限的边距折成 0，并保留合法单位。
    #[test]
    fn sanitize_rejects_bad_slice_margins() {
        let mut e = el(UiKind::Image, Anchor::Center);
        e.slice_margin = [
            UiValue::px(-4.0),
            UiValue::pct(f32::NAN),
            UiValue::pct(12.0),
            UiValue::px(f32::INFINITY),
        ];
        let s = e.sanitized();
        assert_eq!(
            s.slice_margin,
            [
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::pct(12.0),
                UiValue::px(0.0)
            ]
        );
    }

    /// 九宫格边距属于样式字段：同 key 每帧重发几何时沿用旧值（与 uv / layer 同档）。
    #[test]
    fn merge_keeps_slice_margins_from_existing() {
        let mut old = el(UiKind::Image, Anchor::TopLeft);
        old.size = [UiValue::px(64.0), UiValue::px(64.0)];
        old.uv = [0.25, 0.25, 0.5, 0.5];
        old.layer = 7;
        old.slice_margin = [
            UiValue::px(8.0),
            UiValue::px(8.0),
            UiValue::px(8.0),
            UiValue::px(8.0),
        ];

        // 新提交只描述几何，样式字段留给旧值
        let mut incoming = el(UiKind::Image, Anchor::TopLeft);
        incoming.size = [UiValue::px(200.0), UiValue::px(120.0)];
        incoming.offset = [UiValue::px(5.0), UiValue::px(6.0)];

        let merged = merge_same_key(&old, incoming);
        assert_eq!(merged.size, [UiValue::px(200.0), UiValue::px(120.0)]);
        assert_eq!(merged.offset, [UiValue::px(5.0), UiValue::px(6.0)]);
        assert_eq!(merged.slice_margin, old.slice_margin, "边距应沿用旧值");
        assert_eq!(merged.uv, old.uv, "uv 也应沿用");
        assert_eq!(merged.layer, 7);
    }

    /// 只有图片元素才认九宫格；矩形 / 边框即使写了边距也不切分。
    #[test]
    fn has_slice_only_for_images() {
        let r = UiRect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let mut img = el(UiKind::Image, Anchor::Center);
        img.slice_margin = [UiValue::px(4.0); 4];
        assert!(img.has_slice(&r));

        // 边距全 0 → 关闭
        img.slice_margin = [UiValue::px(0.0); 4];
        assert!(!img.has_slice(&r));

        img.slice_margin = [UiValue::px(4.0); 4];
        for kind in [UiKind::Rect, UiKind::Border] {
            let mut other = el(kind, Anchor::Center);
            other.slice_margin = [UiValue::px(4.0); 4];
            assert!(!other.has_slice(&r), "{:?} 不应有九宫格", kind);
        }
    }
}
