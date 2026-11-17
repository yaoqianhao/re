//! `query.*` 的参数承载（M1-S2-B）。
//!
//! 三条查询方法共享同一份过滤条件，所以 DTO 只做**形状收敛**，不做语义裁决：
//!
//! - 类别名与组件位名都按**字符串原样透传**——枚举定义在 `r3e-core`（`NodeKind` /
//!   `Component`），协议层若也持一份就会变成第二个真源，加一个类别要改两处。
//!   拼错的名字由宿主在翻译时报参数错误，AI 拿到的是明确失败而不是「空结果」。
//! - `limit` 的缺省值协议层**也不填**：`None` 一路传到宿主，由宿主取
//!   `r3e_core::query::DEFAULT_LIMIT`。这样「默认条数」只有一个出处，
//!   改常量不会漏改协议层。
//!
//! 字段名与 `r3e_core::Query` 保持一一对应（`kind` / `all_tags` / `any_tags` /
//! `region` / `require` / `exclude` / `limit`），JSON 写法用驼峰
//! （`allTags` / `anyTags`），与其余 patch DTO 口径一致。

/// `query.find` 的过滤条件（`query.stats` 不用，`query.describe` 只吃节点句柄）。
///
/// 全部字段可缺省：一个 `{}` 进来就是「按默认上限列前 N 个节点」。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuerySpec {
    /// 类别过滤（`any` / `prop` / `interactable` / `animated` / `skinned` / `morphing` /
    /// `character` / `physical`）。`None` 与空串等价于 `any`。
    pub kind: Option<String>,
    /// 标签**全含**过滤（与的关系）。
    pub all_tags: Vec<String>,
    /// 标签**含任一**过滤（或的关系）；与 [`Self::all_tags`] 同时给出时两者都要满足。
    pub any_tags: Vec<String>,
    /// 世界空间区域 `[minX, minY, minZ, maxX, maxY, maxZ]`，边界含；对角点顺序不限
    /// （宿主侧 `Aabb::new` 会归一化）。
    pub region: Option<[f32; 6]>,
    /// 必须有这些组件位（全部满足）。
    pub require: Vec<String>,
    /// 不能有这些组件位（任一命中即排除）。
    pub exclude: Vec<String>,
    /// 条数上限；`None` = 宿主取默认值，`0` = 不限（AI 明确要求全量时用）。
    pub limit: Option<usize>,
}

impl QuerySpec {
    /// 是否一条过滤条件都没给（只有 `limit` 不算过滤条件）。
    ///
    /// 供宿主区分「无过滤」与「过滤后无命中」——两者的空结果含义完全不同，
    /// 前者通常是 AI 还没想清楚要问什么。
    pub fn is_unfiltered(&self) -> bool {
        self.kind.is_none()
            && self.all_tags.is_empty()
            && self.any_tags.is_empty()
            && self.region.is_none()
            && self.require.is_empty()
            && self.exclude.is_empty()
    }
}
