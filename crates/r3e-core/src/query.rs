//! 语义查询执行器（M1-S2-A）：把「场景里有哪些可交互物」这类问题收敛成一次结构化查询。
//!
//! 给 AI 的查询面只回答**语义问题**，不暴露内存布局：一次 [`Query`] 进来，一组
//! [`NodeView`] 出去。四类过滤同真才命中——`kind`（粗粒度类别）、`tags`（语义标签）、
//! `region`（世界空间区域）、`require` / `exclude`（组件位）。
//!
//! # 判据从哪来
//!
//! 全部落在场景已有数据上，本模块**不新增任何存储**，因此查询结果不可能与真值层漂移：
//!
//! | 类别 | 判据 |
//! |---|---|
//! | [`NodeKind::Interactable`] | `Node::interact_radius.is_some()`（写入侧已收敛） |
//! | [`NodeKind::Animated`] | 某条 `AnimInstance` 的 `bindings` 引用了该节点 |
//! | [`NodeKind::Skinned`] | `Scene::skin_of` 命中 |
//! | [`NodeKind::Morphing`] | `Scene::morph_of` 命中 |
//! | [`NodeKind::Character`] | `Skinned` 或 `Morphing`（角色 = 会动的网格） |
//! | [`NodeKind::Physical`] | `node.physics.is_some()` 或已有后端刚体句柄 `node.body` |
//! | [`NodeKind::Prop`] | 以上五类全不命中，且 `visible` |
//!
//! # 确定性
//!
//! 结果按场景插入顺序返回（[`Scene::iter`] 的顺序），同一份场景同一份查询永远同一份结果，
//! 便于回放比对；`limit` 只做尾部截断，不做任何抽样。

use std::collections::HashSet;

use glam::Vec3;

use crate::aabb::Aabb;
use crate::node::{Node, NodeId};
use crate::scene::Scene;

/// 查询结果的默认条数上限（`Query::limit = 0` 表示不限）。
///
/// 协议层（M1-S2-B）用它做缺省值，避免 AI 一句「列出所有物件」把整场景原样拉走。
pub const DEFAULT_LIMIT: usize = 64;

/// 物件的粗粒度类别。
///
/// 判据见模块文档；[`NodeKind::parse`] 解析失败返回 `None`，不做「猜一个最像的」——
/// AI 拼错类别名应当收到错误，而不是一份看起来正常的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum NodeKind {
    /// 不做类别过滤。
    #[default]
    Any,
    /// 普通布景物件：可见，且不是可交互物、不是角色、没有物理。
    Prop,
    /// 可交互物（声明了 `interact_radius`）。
    Interactable,
    /// 被动画实例驱动的节点。
    Animated,
    /// 带蒙皮的网格节点。
    Skinned,
    /// 带顶点形变的网格节点。
    Morphing,
    /// 角色类：带蒙皮或形变。
    Character,
    /// 有物理描述或已挂刚体句柄的节点。
    Physical,
}

impl NodeKind {
    /// 全部类别（改枚举时手工同步，`tests/world_query.rs` 有一条用例守着这个常量）。
    pub const ALL: [Self; 8] = [
        Self::Any,
        Self::Prop,
        Self::Interactable,
        Self::Animated,
        Self::Skinned,
        Self::Morphing,
        Self::Character,
        Self::Physical,
    ];

    /// 协议 / 脚本里的写法：`any` / `prop` / `interactable` / `animated` / `skinned` /
    /// `morphing` / `character` / `physical`（大小写不敏感，另收 `interact` / `morph` 等别名）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "any" | "" => Some(Self::Any),
            "prop" | "static" => Some(Self::Prop),
            "interactable" | "interact" => Some(Self::Interactable),
            "animated" | "anim" => Some(Self::Animated),
            "skinned" | "skin" => Some(Self::Skinned),
            "morphing" | "morph" => Some(Self::Morphing),
            "character" | "actor" => Some(Self::Character),
            "physical" | "physics" => Some(Self::Physical),
            _ => None,
        }
    }

    /// 反向序列化，供回包 / 日志回显。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Prop => "prop",
            Self::Interactable => "interactable",
            Self::Animated => "animated",
            Self::Skinned => "skinned",
            Self::Morphing => "morphing",
            Self::Character => "character",
            Self::Physical => "physical",
        }
    }
}

/// 组件位：比 [`NodeKind`] 更细的「这个物件有没有某样东西」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// 覆写了 PBR 表面材质（`node.material.is_some()`）。
    Material,
    /// 有物理描述或刚体句柄。
    Physics,
    /// 声明了可交互半径。
    Interact,
    /// 被动画实例驱动。
    Animated,
    /// 带蒙皮。
    Skinned,
    /// 带顶点形变。
    Morphing,
    /// 有父节点。
    Parent,
    /// 有子节点。
    Child,
    /// 至少一个语义标签。
    Tagged,
}

impl Component {
    /// 全部组件位。
    pub const ALL: [Self; 9] = [
        Self::Material,
        Self::Physics,
        Self::Interact,
        Self::Animated,
        Self::Skinned,
        Self::Morphing,
        Self::Parent,
        Self::Child,
        Self::Tagged,
    ];

    /// 协议 / 脚本里的写法（大小写不敏感）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "material" => Some(Self::Material),
            "physics" | "body" => Some(Self::Physics),
            "interact" | "interactable" => Some(Self::Interact),
            "anim" | "animated" => Some(Self::Animated),
            "skin" | "skinned" => Some(Self::Skinned),
            "morph" | "morphing" => Some(Self::Morphing),
            "parent" => Some(Self::Parent),
            "child" | "children" => Some(Self::Child),
            "tagged" | "tag" => Some(Self::Tagged),
            _ => None,
        }
    }

    /// 反向序列化。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Physics => "physics",
            Self::Interact => "interact",
            Self::Animated => "animated",
            Self::Skinned => "skinned",
            Self::Morphing => "morphing",
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Tagged => "tagged",
        }
    }
}

/// 一次语义查询：字段之间是 AND，默认值等价于「全场景、不限条数」。
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// 粗粒度类别，默认 [`NodeKind::Any`]。
    pub kind: NodeKind,
    /// 必须**全部**命中的标签。
    ///
    /// 比较时只忽略首尾空白，**大小写不折叠**（写入侧 `normalize_tags` 已明确「`Door` 与
    /// `door` 是两个标签」，查询侧再折一次就会给出与场景文件不符的结果）。
    pub all_tags: Vec<String>,
    /// **至少**命中一个即可的标签（空表示不约束）。比较口径同 [`Query::all_tags`]。
    pub any_tags: Vec<String>,
    /// 世界空间区域：物件**世界原点**落在盒内才命中（含边界）。
    ///
    /// 用原点是刻意的——它稳定、可预期、不依赖网格几何，AI 更容易解释「为什么这个物件在区域里」。
    pub region: Option<Aabb>,
    /// 必须全部具备的组件位。
    pub require: Vec<Component>,
    /// 只要具备任意一个就不返回的组件位。
    pub exclude: Vec<Component>,
    /// 条数上限，`0` 表示不限（协议层缺省用 [`DEFAULT_LIMIT`]）。
    pub limit: usize,
}

impl Query {
    /// 指定类别。
    pub fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// 追加一个必须命中的标签。
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.all_tags.push(tag.into());
        self
    }

    /// 追加一个「命中其一即可」的标签。
    pub fn with_any_tag(mut self, tag: impl Into<String>) -> Self {
        self.any_tags.push(tag.into());
        self
    }

    /// 限定世界空间区域。
    pub fn in_region(mut self, region: Aabb) -> Self {
        self.region = Some(region);
        self
    }

    /// 追加一个必须存在的组件位。
    pub fn requiring(mut self, component: Component) -> Self {
        self.require.push(component);
        self
    }

    /// 追加一个「有就排除」的组件位。
    pub fn excluding(mut self, component: Component) -> Self {
        self.exclude.push(component);
        self
    }

    /// 设置条数上限（`0` = 不限）。
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// 查询回包里的单个物件：只给语义字段，不含世界矩阵 / 网格句柄这类内存布局。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    /// 节点 id。
    pub id: NodeId,
    /// 物件名（场景内唯一，`find_id` 用的是同一个键）。
    pub name: String,
    /// 语义标签，写入侧收敛后的原样。
    pub tags: Vec<String>,
    /// 世界空间位置（最近一次 `update_world_transforms` 的结果）。
    pub position: [f32; 3],
    /// 可交互半径；`None` = 不可交互。
    pub interact_radius: Option<f32>,
}

impl NodeView {
    /// 从节点投影。
    pub fn from_node(node: &Node) -> Self {
        let p: Vec3 = node.world_position();
        Self {
            id: node.id,
            name: node.name.clone(),
            tags: node.tags.clone(),
            position: [p.x, p.y, p.z],
            interact_radius: node.interact_radius,
        }
    }
}

/// 一次查询内复用的索引集合：动画 / 蒙皮 / 形变都是场景级列表，逐节点线性扫会很浪费。
struct Facts {
    animated: HashSet<NodeId>,
    skinned: HashSet<NodeId>,
    morphing: HashSet<NodeId>,
}

impl Facts {
    fn collect(scene: &Scene) -> Self {
        let animated = scene
            .animations
            .iter()
            .flat_map(|a| a.bindings.iter().map(|b| b.node))
            .collect();
        let skinned = scene.skins.iter().map(|s| s.mesh_node).collect();
        let morphing = scene.morphs.iter().map(|m| m.mesh_node).collect();
        Self {
            animated,
            skinned,
            morphing,
        }
    }

    fn is_physical(node: &Node) -> bool {
        node.physics.is_some() || node.body.is_some()
    }

    fn matches_kind(&self, node: &Node, kind: NodeKind) -> bool {
        match kind {
            NodeKind::Any => true,
            NodeKind::Interactable => node.interact_radius.is_some(),
            NodeKind::Animated => self.animated.contains(&node.id),
            NodeKind::Skinned => self.skinned.contains(&node.id),
            NodeKind::Morphing => self.morphing.contains(&node.id),
            NodeKind::Character => {
                self.skinned.contains(&node.id) || self.morphing.contains(&node.id)
            }
            NodeKind::Physical => Self::is_physical(node),
            NodeKind::Prop => {
                node.visible
                    && node.interact_radius.is_none()
                    && !Self::is_physical(node)
                    && !self.animated.contains(&node.id)
                    && !self.skinned.contains(&node.id)
                    && !self.morphing.contains(&node.id)
            }
        }
    }

    fn has_component(&self, node: &Node, component: Component) -> bool {
        match component {
            Component::Material => node.material.is_some(),
            Component::Physics => Self::is_physical(node),
            Component::Interact => node.interact_radius.is_some(),
            Component::Animated => self.animated.contains(&node.id),
            Component::Skinned => self.skinned.contains(&node.id),
            Component::Morphing => self.morphing.contains(&node.id),
            Component::Parent => node.parent.is_some(),
            Component::Child => !node.children.is_empty(),
            Component::Tagged => !node.tags.is_empty(),
        }
    }
}

/// 标签命中判定：按字面比较，只忽略首尾空白。
///
/// 大小写**不折叠**——写入侧 `normalize_tags` 已明确「`Door` 与 `door` 是两个标签」，
/// 查询侧若做大小写折叠，就会出现「查 `door` 命中 `Door`」这种与场景文件不符的结果。
fn has_tag(node: &Node, want: &str) -> bool {
    let want = want.trim();
    !want.is_empty() && node.tags.iter().any(|t| t == want)
}

/// 点是否落在盒内（含边界）。
fn contains_point(region: &Aabb, p: Vec3) -> bool {
    p.x >= region.min.x
        && p.x <= region.max.x
        && p.y >= region.min.y
        && p.y <= region.max.y
        && p.z >= region.min.z
        && p.z <= region.max.z
}

impl Scene {
    /// 执行一次语义查询，返回按插入顺序排列的物件投影。
    ///
    /// 只读：不构造事务、不产生写点、不改 `StateHash`——协议层（`query.*`）因此可以
    /// 直接借用只读事务语义，查询前后哈希必然相等。
    pub fn query(&self, query: &Query) -> Vec<NodeView> {
        let facts = Facts::collect(self);
        let mut out: Vec<NodeView> = Vec::new();
        for node in self.iter() {
            if !facts.matches_kind(node, query.kind) {
                continue;
            }
            if !query.all_tags.iter().all(|t| has_tag(node, t)) {
                continue;
            }
            if !query.any_tags.is_empty() && !query.any_tags.iter().any(|t| has_tag(node, t)) {
                continue;
            }
            if let Some(region) = &query.region {
                if !contains_point(region, node.world_position()) {
                    continue;
                }
            }
            if !query.require.iter().all(|c| facts.has_component(node, *c)) {
                continue;
            }
            if query.exclude.iter().any(|c| facts.has_component(node, *c)) {
                continue;
            }
            out.push(NodeView::from_node(node));
            if query.limit > 0 && out.len() >= query.limit {
                break;
            }
        }
        out
    }

    /// 场景的语义普查：各类别计数 + 标签表 + 可交互物数量。
    ///
    /// 给 `query.stats`（M1-S2-B）与 AI 的「先摸清场景里有什么」用；与 [`Scene::query`]
    /// 共用同一套判据，不会出现「stats 说有 3 个、find 只能查到 2 个」。
    pub fn query_stats(&self) -> QueryStats {
        let facts = Facts::collect(self);
        let mut stats = QueryStats {
            nodes: self.len(),
            ..QueryStats::default()
        };
        for node in self.iter() {
            for kind in NodeKind::ALL {
                if facts.matches_kind(node, kind) {
                    *stats.by_kind.entry(kind).or_insert(0) += 1;
                }
            }
            if node.visible {
                stats.visible += 1;
            }
            if node.interact_radius.is_some() {
                stats.interactable += 1;
            }
            if !node.tags.is_empty() {
                stats.tagged += 1;
            }
            for tag in &node.tags {
                *stats.tags.entry(tag.clone()).or_insert(0) += 1;
            }
        }
        stats
    }
}

/// [`Scene::query_stats`] 的回包。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryStats {
    /// 场景节点总数。
    pub nodes: usize,
    /// 可见节点数。
    pub visible: usize,
    /// 可交互物数量。
    pub interactable: usize,
    /// 带至少一个标签的节点数。
    pub tagged: usize,
    /// 各类别命中数（`NodeKind::ALL` 里命中数为 0 的类别不出现，避免回包包一堆 0）。
    pub by_kind: std::collections::BTreeMap<NodeKind, usize>,
    /// 标签出现次数，供 AI 反查「这个场景习惯用什么标签」。
    pub tags: std::collections::BTreeMap<String, usize>,
}
