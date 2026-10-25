//! r3e-physics —— 物理抽象层 + 可插拔后端（内置最简 / Rapier / 空转）。
//!
//! 设计分两层：
//!
//! * **抽象层**：`Physics` 门面持有 `Box<dyn PhysicsBackend>`，场景与脚本只认这套接口。
//!   后端名（`null` / `builtin` / `rapier`）只是实现细节，**能力由 [`BackendCaps`] 申报**，
//!   上层按能力分支而不是按后端名分支，因此换后端不牵动上层逻辑。
//! * **后端实现**：
//!   - `BuiltinBackend`：纯 Rust 自实现的最简规则（零外部依赖），只做「刚体 vs 地面平面」：
//!     重力积分、弹性反弹、摩擦、静止休眠。物件之间不碰撞、不做旋转动力学、无关节与堆叠。
//!   - `RapierBackend`（`feature = "rapier"`）：Rapier3D 真实物理，互撞 / 堆叠 / 旋转动力学 /
//!     碰撞事件。默认不编译，`--features rapier` 构建才可用。
//!   - `NullBackend`：空转，只登记句柄。
//!
//! 物理默认**关闭**（`Physics::new()` 即 Null 后端 + enabled=false），
//! 通过三层任一入口开启：命令行 `--physics on`、场景 JSON 的 `physics.enabled`、
//! 脚本 `physics_set_enabled(true)`。关闭时不进入积分、`attach_body` 返回 false、
//! `body_count` 恒为 0，物件完全由脚本驱动（与旧行为一致）。
//!
//! **可切换性是硬约束**：`rapier3d` / `parry` 的类型只允许出现在 `rapier_backend.rs`，
//! 门面、`app`、脚本层只接触 `glam` 向量与本文件的契约类型。


use glam::Vec3;
use r3e_core::{Aabb, MeshData, Node, NodeId, PhysicsSceneDesc, Scene};

#[cfg(feature = "rapier")]
mod rapier_backend;

/// 刚体句柄，由后端分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyHandle(pub u64);

/// 后端能力位：回答「这个后端能做什么」，而不是「它叫什么」。
///
/// 上层（`app` / 脚本 / 编辑器）应当按能力分支：例如只有在
/// `collision_events` 为真时才有意义去消费碰撞事件，
/// 只有 `body_body_contacts` 为真时才指望球撞球。
/// 新增后端（Jolt / PhysX / 刚体树）只需如实申报一份位表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendCaps {
    /// 刚体之间互相碰撞（不止与地面相交）。
    pub body_body_contacts: bool,
    /// 旋转动力学（力矩、翻滚、角速度）。
    pub rotational_dynamics: bool,
    /// 关节与约束。
    pub joints: bool,
    /// 连续碰撞检测（高速物体不穿透）。
    pub ccd: bool,
    /// 碰撞事件（开始 / 结束）。
    pub collision_events: bool,
    /// 射线查询（`raycast`）：从一点朝一个方向打射线，拿到最近命中体。
    pub raycast: bool,
    /// 形状重叠查询（`overlap_sphere`）：给定球体覆盖范围内的刚体。
    pub overlap: bool,
    /// 扫掠查询（`shape_cast`）：把球 / 盒沿方向推过去，拿到最近的一次接触。
    pub shape_cast: bool,
}

impl BackendCaps {
    /// 什么都不做（`null` 后端）。
    pub const NONE: Self = Self {
        body_body_contacts: false,
        rotational_dynamics: false,
        joints: false,
        ccd: false,
        collision_events: false,
        raycast: false,
        overlap: false,
        shape_cast: false,
    };

    /// 内置后端：只有「刚体 vs 地面」这一条接触规则，无旋转、无事件、无 CCD，
    /// 但查询（射线 / 重叠）用解析法真实可查。
    pub const BASIC: Self = Self {
        body_body_contacts: false,
        rotational_dynamics: false,
        joints: false,
        ccd: false,
        collision_events: false,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 有真实约束求解器的档位：刚体互撞 + 旋转动力学 + 碰撞事件（Rapier 当前落在这一档）。
    pub const CONTACT: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: false,
        ccd: false,
        collision_events: true,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 门面（`PhysicsBackend` trait + 脚本 `physics_*`）当前真正暴露给上层的上限。
    ///
    /// 关节与 CCD 还没有 trait 方法与脚本入口，所以任何后端都**不得**申报它们，
    /// 否则脚本通过 `physics_has_capability` 拿到的是一个无法使用的承诺。
    pub const EXPOSED: Self = Self::CONTACT;

    /// 完整能力，仅作「等门面补齐关节与 CCD 接口后」的目标档位预留。
    ///
    /// 当前没有任何后端可以返回它：申报能力必须以门面已暴露的接口为准，
    /// 详见 [`BackendCaps::EXPOSED`]。
    pub const FULL: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: true,
        ccd: true,
        collision_events: true,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 申报是否落在门面已暴露的能力范围内（越界即虚报）。
    pub fn is_within_exposed(&self) -> bool {
        !self.joints && !self.ccd
    }

    /// 能力键名（稳定标识，供日志与脚本消费）。
    pub fn enabled_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.body_body_contacts {
            keys.push("body_body");
        }
        if self.rotational_dynamics {
            keys.push("rotation");
        }
        if self.joints {
            keys.push("joints");
        }
        if self.ccd {
            keys.push("ccd");
        }
        if self.collision_events {
            keys.push("events");
        }
        if self.raycast {
            keys.push("raycast");
        }
        if self.overlap {
            keys.push("overlap");
        }
        if self.shape_cast {
            keys.push("shape_cast");
        }
        keys
    }

    /// 逗号分隔的能力摘要；全关时返回 `none`。
    pub fn summary(&self) -> String {
        let keys = self.enabled_keys();
        if keys.is_empty() {
            "none".to_string()
        } else {
            keys.join(",")
        }
    }

    /// 按能力名查询，未知键返回 false。
    pub fn has(&self, key: &str) -> bool {
        self.enabled_keys().contains(&key)
    }
}

/// 碰撞事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsEventKind {
    /// 两个碰撞体开始接触。
    Started,
    /// 两个碰撞体分离。
    Stopped,
}

/// 碰撞事件。一侧为 `None` 表示它不是场景节点（例如后端自带的地面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsEvent {
    pub kind: PhysicsEventKind,
    pub a: Option<NodeId>,
    pub b: Option<NodeId>,
}

impl PhysicsEvent {
    /// 压成一行文本：`started:7-none`，便于日志与脚本筛选。
    pub fn to_text(&self) -> String {
        let side = |n: Option<NodeId>| match n {
            Some(id) => id.to_string(),
            None => "none".to_string(),
        };
        let tag = match self.kind {
            PhysicsEventKind::Started => "started",
            PhysicsEventKind::Stopped => "stopped",
        };
        format!("{}:{}-{}", tag, side(self.a), side(self.b))
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Static,
    Dynamic,
    Kinematic,
}

impl BodyKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "static" | "fixed" => Self::Static,
            "kinematic" => Self::Kinematic,
            _ => Self::Dynamic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColliderShape {
    Box,
    Sphere,
    Capsule,
    Cylinder,
}

impl ColliderShape {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "sphere" | "ball" => Self::Sphere,
            "capsule" => Self::Capsule,
            "cylinder" => Self::Cylinder,
            _ => Self::Box,
        }
    }
}

/// 创建一个刚体所需的全部信息。
#[derive(Debug, Clone)]
pub struct BodyDesc {
    pub kind: BodyKind,
    pub shape: ColliderShape,
    /// 半尺寸（Box / 碰撞体缩放）。
    pub half_extents: Vec3,
    pub mass: f32,
    pub restitution: f32,
    pub friction: f32,
    /// 初始世界位置 / 旋转 / 缩放，来自节点当下状态。
    pub position: Vec3,
    pub rotation: glam::Quat,
    pub scale: Vec3,
}

impl Default for BodyDesc {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            shape: ColliderShape::Box,
            half_extents: Vec3::splat(0.5),
            mass: 1.0,
            restitution: 0.2,
            friction: 0.6,
            position: Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl BodyDesc {
    /// 仅凭网格与缩放推断一个刚体描述（不读 `physics` 段）。
    /// 用于脚本运行时创建、且没有物理描述的物件。
    pub fn infer(node: &Node) -> Self {
        Self::infer_with(node, None)
    }

    /// 同 [`BodyDesc::infer`]，但用网格真实几何推出更贴合的碰撞体。
    ///
    /// `mesh` 是 AIR 网格数据（局部空间）；形状由 [`infer_shape`] 按顶点分布判定，
    /// 半尺寸按 `AABB 半尺寸 × scale` 推导。缺失则退回 `Box` + `scale * 0.5`。
    pub fn infer_with(node: &Node, mesh: Option<&MeshData>) -> Self {
        let shape = mesh.map(infer_shape).unwrap_or(ColliderShape::Box);
        let scale = node.transform.scale.abs();
        Self {
            kind: BodyKind::Dynamic,
            shape,
            half_extents: half_extents_from(mesh.map(|m| m.aabb_box()), scale),
            position: node.world_position(),
            rotation: node.transform.rotation,
            scale,
            ..Default::default()
        }
    }

    /// 从场景节点推导刚体描述：节点需带 `physics` 段，否则返回 `None`。
    ///
    /// 缺省规则与场景文件约定一致：碰撞形状由 `collider` 指定、否则由 mesh 推导；
    /// 半尺寸由 `half_extents` 指定、否则取 scale * 0.5；质量缺省 1.0。
    pub fn from_node(node: &Node) -> Option<Self> {
        Self::from_node_with(node, None)
    }

    /// 同 [`BodyDesc::from_node`]，但可用网格真实包围盒推导碰撞体尺寸。
    ///
    /// 外部导入网格的真实尺寸往往远不是 1×1×1，退化成
    /// `scale * 0.5` 会让碰撞体明显偏小；这里让 AABB 参与推导。
    /// 显式写了 `half_extents` 的场景仍然以场景为准（人工调过的值优先）。
    pub fn from_node_with(node: &Node, mesh: Option<&MeshData>) -> Option<Self> {
        let p = node.physics.as_ref()?;
        let shape = p
            .collider
            .as_deref()
            .map(ColliderShape::parse)
            .unwrap_or_else(|| mesh.map(infer_shape).unwrap_or(ColliderShape::Box));
        let scale = node.transform.scale.abs();
        let half_extents = p
            .half_extents
            .map(|h| Vec3::from(h).abs())
            .unwrap_or_else(|| half_extents_from(mesh.map(|m| m.aabb_box()), scale));
        Some(Self {
            kind: BodyKind::parse(&p.body),
            shape,
            half_extents,
            mass: if p.mass > 1e-4 { p.mass } else { 1.0 },
            restitution: p.restitution.unwrap_or(0.2),
            friction: p.friction.unwrap_or(0.6),
            position: node.world_position(),
            rotation: node.transform.rotation,
            scale,
        })
    }
}

/// 碰撞体半尺寸：有真实 AABB 就用它乘缩放，否则退回 `scale * 0.5`。
///
/// 下限 1e-3 避免零厚度网格生成退化碰撞体。
fn half_extents_from(bounds: Option<Aabb>, scale: Vec3) -> Vec3 {
    match bounds.filter(|b| !b.is_empty()) {
        Some(aabb) => (aabb.half_extents() * scale).max(Vec3::splat(0.001)),
        None => (scale * 0.5).max(Vec3::splat(0.001)),
    }
}

/// 从 AIR 网格几何推断碰撞体形状——**不看 `MeshRef` 来源**。
///
/// 内置图元与外部资产走完全相同的判定，因此 FBX / glb 里导出的球、圆柱也能得到
/// 与内置图元一致的碰撞体：
/// * 顶点到 AABB 中心等距且三轴半径相等 → [`ColliderShape::Sphere`]；
/// * XZ 半径只有「侧壁半径」和「轴心 0」两类（无中间带）且 X/Z 半径相等 →
///   [`ColliderShape::Cylinder`]（轴向 Y，与引擎图元一致）；
/// * 其余一律 [`ColliderShape::Box`]，半尺寸由 AABB 给出。
pub fn infer_shape(data: &MeshData) -> ColliderShape {
    let points = &data.vertices;
    if points.len() < 4 {
        return ColliderShape::Box;
    }

    let aabb = data.aabb_box();
    if aabb.is_empty() {
        return ColliderShape::Box;
    }
    let center = aabb.center();
    let half = aabb.half_extents();
    let eps = 1e-4f32;
    if half.min_element() <= eps {
        return ColliderShape::Box;
    }
    let same = |a: f32, b: f32| (a - b).abs() <= 0.05 * a.max(b);

    // ---- 球 ----
    let radius = half.max_element();
    if same(half.x, half.y) && same(half.y, half.z) {
        let tolerance = 0.02 * radius;
        let is_sphere = points.iter().all(|vertex| {
            ((Vec3::from(vertex.position) - center).length() - radius).abs() <= tolerance
        });
        if is_sphere {
            return ColliderShape::Sphere;
        }
    }

    // ---- 圆柱（轴向 Y）----
    if same(half.x, half.z) {
        let radius = half.x.max(half.z);
        let mut wall = 0usize;
        let mut middle = 0usize;
        for vertex in points {
            let v = Vec3::from(vertex.position);
            let r = (v.x - center.x).hypot(v.z - center.z);
            if (r - radius).abs() <= 0.02 * radius {
                wall += 1;
            } else if r > 0.05 * radius && r < 0.95 * radius {
                middle += 1;
            }
        }
        // 侧壁顶点占多数，且没有落在中间半径上的顶点（否则更像球面/圆锥面）。
        if middle == 0 && wall * 2 >= points.len() {
            return ColliderShape::Cylinder;
        }
    }

    ColliderShape::Box
}

// ---------------------------------------------------------------------------
// 后端选择与配置
// ---------------------------------------------------------------------------

/// 物理后端种类。`Rapier` 由 `feature = "rapier"` 决定是否真的可用：
/// 未编译进来时选择它会返回错误而不是静默降级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsKind {
    /// 空转：只登记句柄，不做任何模拟（等价于「物理关闭」）。
    Null,
    /// 内置最简物理（本 crate 自实现）。
    Builtin,
    /// Rapier 后端（`feature = "rapier"`）。
    Rapier,
}

impl PhysicsKind {
    /// 全部后端槽位，顺序即日志/帮助里的展示顺序。
    pub const ALL: [PhysicsKind; 3] = [Self::Null, Self::Builtin, Self::Rapier];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "off" | "null" => Some(Self::Null),
            "builtin" | "internal" | "basic" | "simple" => Some(Self::Builtin),
            "rapier" | "rapier3d" => Some(Self::Rapier),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Builtin => "builtin",
            Self::Rapier => "rapier",
        }
    }

    /// 当前构建是否真的能提供该后端。
    pub fn is_available(&self) -> bool {
        match self {
            Self::Null | Self::Builtin => true,
            Self::Rapier => cfg!(feature = "rapier"),
        }
    }

    /// 当前构建真正可用的后端名（错误提示与帮助文本用，避免写死）。
    pub fn available_names() -> Vec<&'static str> {
        Self::ALL
            .iter()
            .filter(|k| k.is_available())
            .map(|k| k.name())
            .collect()
    }
}

/// 物理配置。每个字段都是 `Option`：
/// `None` 表示「这一层没有意见」，留给更低的优先级决定（命令行 > 场景文件 > 内置默认）。
#[derive(Debug, Clone, Default)]
pub struct PhysicsConfig {
    pub enabled: Option<bool>,
    pub kind: Option<PhysicsKind>,
    pub gravity: Option<Vec3>,
    pub ground: Option<f32>,
}

impl PhysicsConfig {
    pub fn from_scene(desc: &PhysicsSceneDesc) -> Self {
        Self {
            enabled: desc.enabled,
            kind: desc.backend.as_deref().and_then(PhysicsKind::parse),
            gravity: desc.gravity.map(Vec3::from),
            ground: desc.ground,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }

    pub fn with_kind(mut self, kind: PhysicsKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// `other` 中已明确指定的字段覆盖 `self`。
    pub fn merge_over(&self, other: &PhysicsConfig) -> PhysicsConfig {
        PhysicsConfig {
            enabled: other.enabled.or(self.enabled),
            kind: other.kind.or(self.kind),
            gravity: other.gravity.or(self.gravity),
            ground: other.ground.or(self.ground),
        }
    }
}

// ---------------------------------------------------------------------------
// 后端契约
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 查询（与后端无关的输入 / 输出类型）
// ---------------------------------------------------------------------------

/// 一条射线。`dir` 在构造时归一化，`max_distance` 由调用方按世界单位给。
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    /// 构造射线；零向量方向退化为 +Y，避免后端拿到 NaN 方向。
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        let len = dir.length();
        let dir = if len > 1e-6 { dir / len } else { Vec3::Y };
        Self { origin, dir }
    }

    /// 射线上的点：`origin + dir * t`。
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// 射线命中的最近刚体。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub handle: BodyHandle,
    pub node: NodeId,
    /// 沿射线的命中距离（世界单位，`>= 0`）。
    pub distance: f32,
    /// 命中点世界坐标。
    pub point: Vec3,
    /// 命中面法线（世界空间、已归一化；射线从物体内部出发时取 `-dir`）。
    pub normal: Vec3,
}

/// 重叠查询命中的一个刚体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlapHit {
    pub handle: BodyHandle,
    pub node: NodeId,
}

/// 扫掠（shape-cast）用的查询形状。
///
/// 与 `ColliderShape` 刻意分开：查询形状是「临时探针」，不需要建刚体、不需要写回场景，
/// 也不受 `BodyDesc` 那一堆质量 / 摩擦字段牵连。内置后端不旋转，因此这里没有朝向字段。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastShape {
    /// 球体：`radius` 取绝对值，`<= 0` 视为查不到。
    Sphere { radius: f32 },
    /// 轴对齐盒：`half` 为半尺寸（与 `BodyDesc::half_extents` 同口径）。
    Box { half: Vec3 },
}

impl CastShape {
    /// 球体构造（负半径取绝对值）。
    pub fn sphere(radius: f32) -> Self {
        Self::Sphere {
            radius: radius.abs(),
        }
    }

    /// 盒体构造（半尺寸取绝对值）。
    pub fn boxed(half: Vec3) -> Self {
        Self::Box { half: half.abs() }
    }

    /// 该形状的半尺寸：解析法用它把目标 AABB 做 Minkowski 展开。
    pub fn half_extents(&self) -> Vec3 {
        match *self {
            Self::Sphere { radius } => Vec3::splat(radius.abs()),
            Self::Box { half } => half.abs(),
        }
    }

    /// 尺寸退化到 ~0 时任何后端都查不出有意义的结果，门面直接短路。
    pub fn is_degenerate(&self) -> bool {
        self.half_extents().max_element() <= 1e-6
    }
}

/// 扫掠命中：把 `shape` 从 `origin` 沿 `dir` 推多远会撞上某个刚体。
///
/// 语义与 `RayHit` 对齐（`distance` 同为「沿方向的世界距离」），区别只在
/// 「接触的是形状表面而非一个点」——所以 `point` 给的是命中瞬间**查询形状质心**的位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeCastHit {
    pub handle: BodyHandle,
    pub node: NodeId,
    /// 沿 `dir` 需要推进的距离（世界单位，`>= 0`）；`0` 表示起点已经重叠。
    pub distance: f32,
    /// 命中瞬间查询形状质心的世界坐标（接触点 = `point + normal * 形状半尺寸`）。
    pub point: Vec3,
    /// 接触法线（世界空间、已归一化，指向查询形状一侧）；初始重叠时取 `-dir`。
    pub normal: Vec3,
    /// 起点即与目标重叠：`distance == 0`、`normal == -dir`。
    ///
    /// 单独给一位而不是靠 `distance == 0` 猜，是因为「贴面」与「已在体内」对
    /// 脚本的后续动作（推出 / 放弃）完全不同。
    pub started_inside: bool,
}

/// 物理后端契约。实现者负责积分、碰撞与把结果写回场景。
pub trait PhysicsBackend {
    fn name(&self) -> &'static str;

    /// 为节点创建刚体。返回句柄，写入 `Node::body`。
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;

    fn destroy_body(&mut self, handle: BodyHandle);

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3);

    fn set_gravity(&mut self, gravity: Vec3);

    /// 推进模拟。
    fn step(&mut self, dt: f32);

    /// 把物理解算结果同步回场景节点（位置 / 旋转）。
    fn sync_to_scene(&mut self, scene: &mut Scene);

    fn body_count(&self) -> usize;

    /// 可选：返回所有现存句柄，便于整体清空。默认返回 `None`（后端自行管理）。
    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        None
    }

    /// 可选：step 之前从场景回收节点位置。
    /// 内置后端用它识别「脚本直接挪了节点」并唤醒刚体；外部后端可忽略。
    fn pull_from_scene(&mut self, _scene: &Scene) {}

    /// 可选：设置地面平面高度（仅内置后端消费）。
    fn set_ground(&mut self, _ground: f32) {}

    /// 可选：如实申报本后端能做什么。默认「什么都不做」。
    ///
    /// 这是「可切换性」的关键位：上层按能力分支，不要按 `name()` 分支。
    fn capabilities(&self) -> BackendCaps {
        BackendCaps::NONE
    }

    /// 可选：取走自上次调用以来的碰撞事件（不清空刚体）。
    /// 默认后端不产生事件；支持事件的后端需在 `capabilities().collision_events` 里如实置位。
    fn drain_events(&mut self) -> Vec<PhysicsEvent> {
        Vec::new()
    }

    /// 可选：射线查询，返回 `max_distance` 内最近的命中体。
    ///
    /// 默认实现返回 `None`；能查的后端必须同时把 `capabilities().raycast` 置真，
    /// 否则脚本会拿到「申报能查、实际查不到」的假承诺。
    fn raycast(&self, _ray: Ray, _max_distance: f32) -> Option<RayHit> {
        None
    }

    /// 可选：球体重叠查询，返回球心 `center`、半径 `radius` 内相交的刚体。
    ///
    /// 结果按 `node` 升序，保证同一场景下多次查询顺序稳定。
    /// 默认实现返回空表；能查的后端需把 `capabilities().overlap` 置真。
    fn overlap_sphere(&self, _center: Vec3, _radius: f32) -> Vec<OverlapHit> {
        Vec::new()
    }

    /// 可选：扫掠查询（shape-cast），把 `shape` 从 `origin` 沿 `dir` 推 `max_distance`，
    /// 返回最近的一次接触。
    ///
    /// 三者（raycast / overlap / shape_cast）共用一套口径：`dir` 会归一化、
    /// `distance` 是世界距离、起点已重叠时距离为 0。默认实现返回 `None`；
    /// 能查的后端需把 `capabilities().shape_cast` 置真。
    fn shape_cast(
        &self,
        _shape: CastShape,
        _origin: Vec3,
        _dir: Vec3,
        _max_distance: f32,
    ) -> Option<ShapeCastHit> {
        None
    }
}

// ---------------------------------------------------------------------------
// Null 后端：不做任何模拟，只记录句柄，保证脚本仍完全掌控物件。
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct NullBackend {
    next: u64,
    bodies: Vec<BodyHandle>,
}

impl NullBackend {
    pub fn new() -> Self {
        Self {
            next: 1,
            bodies: Vec::new(),
        }
    }
}

impl PhysicsBackend for NullBackend {
    fn name(&self) -> &'static str {
        "null"
    }

    fn create_body(&mut self, _node: NodeId, _desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;
        self.bodies.push(handle);
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        self.bodies.retain(|h| *h != handle);
    }

    fn apply_impulse(&mut self, _handle: BodyHandle, _impulse: Vec3) {}

    fn set_gravity(&mut self, _gravity: Vec3) {}

    fn step(&mut self, _dt: f32) {}

    fn sync_to_scene(&mut self, _scene: &mut Scene) {}

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.bodies.clone())
    }
}

// ---------------------------------------------------------------------------
// 内置最简物理后端
// ---------------------------------------------------------------------------

/// 单个刚体的运行期状态。位置以自身为准，每帧与场景节点双向对齐。
#[derive(Debug, Clone)]
struct RigidBody {
    handle: BodyHandle,
    node: NodeId,
    kind: BodyKind,
    shape: ColliderShape,
    half: Vec3,
    inv_mass: f32,
    velocity: Vec3,
    restitution: f32,
    friction: f32,
    position: Vec3,
    sleep_timer: f32,
    sleeping: bool,
}

impl RigidBody {
    /// 支撑半高：刚体中心到底面的距离。
    fn support(&self) -> f32 {
        let v = match self.shape {
            ColliderShape::Sphere => self.half.x,
            ColliderShape::Box | ColliderShape::Capsule | ColliderShape::Cylinder => self.half.y,
        };
        v.abs().max(0.001)
    }

    fn is_dynamic(&self) -> bool {
        matches!(self.kind, BodyKind::Dynamic)
    }
}

/// 内置最简物理：重力 + 地面平面碰撞 + 弹性 + 摩擦 + 休眠。
///
/// 明确的能力边界（刻意不做，保证 ~400 行可读）：
/// * 物件之间不碰撞，只与一张无限大的地面平面相交
/// * 不做旋转动力学，物体不会翻滚
/// * 无关节、无堆叠求解、无连续碰撞检测（CCD）
pub struct BuiltinBackend {
    next: u64,
    gravity: Vec3,
    ground: f32,
    bodies: Vec<RigidBody>,
}

impl BuiltinBackend {
    pub fn new(gravity: Vec3, ground: f32) -> Self {
        Self {
            next: 1,
            gravity,
            ground,
            bodies: Vec::new(),
        }
    }

    fn find_mut(&mut self, handle: BodyHandle) -> Option<&mut RigidBody> {
        self.bodies.iter_mut().find(|b| b.handle == handle)
    }

    fn substep(&mut self, dt: f32) {
        let gravity = self.gravity;
        let ground = self.ground;
        for rb in self.bodies.iter_mut() {
            if !rb.is_dynamic() || rb.sleeping {
                continue;
            }

            // 1) 半隐式欧拉积分
            rb.velocity += gravity * dt;
            rb.position += rb.velocity * dt;

            // 2) 与地面平面求解
            let floor = ground + rb.support();
            if rb.position.y <= floor {
                rb.position.y = floor;
                let impact = -rb.velocity.y;
                if rb.velocity.y < 0.0 {
                    if impact > 0.6 && rb.restitution > 0.01 {
                        // 弹性：足够快的撞击才反弹，避免落地时无休止抖动
                        rb.velocity.y = impact * rb.restitution;
                    } else {
                        rb.velocity.y = 0.0;
                    }
                }

                // 3) 地面摩擦：指数衰减，与帧率无关
                let k = (1.0 - rb.friction * 8.0 * dt).clamp(0.0, 1.0);
                rb.velocity.x *= k;
                rb.velocity.z *= k;

                // 4) 静止阈值 -> 休眠（省算力，也彻底消除数值抖动）
                if rb.velocity.length_squared() < 0.0025 {
                    rb.sleep_timer += dt;
                    if rb.sleep_timer > 0.35 {
                        rb.sleeping = true;
                        rb.velocity = Vec3::ZERO;
                    }
                } else {
                    rb.sleep_timer = 0.0;
                }
            } else {
                rb.sleep_timer = 0.0;
                // 空气阻尼：防止长时间自由落体后数值过大
                rb.velocity *= (1.0 - 0.02 * dt).clamp(0.0, 1.0);
            }
        }
    }
}

impl PhysicsBackend for BuiltinBackend {
    fn name(&self) -> &'static str {
        "builtin"
    }

    /// 只有「刚体 vs 地面」一条接触规则：不互撞、不旋转、无事件、无 CCD。
    fn capabilities(&self) -> BackendCaps {
        BackendCaps::BASIC
    }

    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;
        let inv_mass = match desc.kind {
            BodyKind::Dynamic => {
                if desc.mass > 1e-4 {
                    1.0 / desc.mass
                } else {
                    1.0
                }
            }
            _ => 0.0,
        };
        let half = body_half_extents(&desc);
        self.bodies.push(RigidBody {
            handle,
            node,
            kind: desc.kind,
            shape: desc.shape,
            half,
            inv_mass,
            velocity: Vec3::ZERO,
            restitution: desc.restitution.clamp(0.0, 1.0),
            friction: desc.friction.clamp(0.0, 2.0),
            position: desc.position,
            sleep_timer: 0.0,
            sleeping: false,
        });
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        self.bodies.retain(|b| b.handle != handle);
    }

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        if let Some(rb) = self.find_mut(handle) {
            if rb.inv_mass > 0.0 {
                rb.velocity += impulse * rb.inv_mass;
                rb.sleeping = false;
                rb.sleep_timer = 0.0;
            }
        }
    }

    fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
    }

    fn set_ground(&mut self, ground: f32) {
        self.ground = ground;
    }

    fn step(&mut self, dt: f32) {
        // 固定子步上限：掉帧时切成多步，保证碰撞不会穿透地板
        let total = dt.clamp(0.0, 0.1);
        if total <= 0.0 {
            return;
        }
        let steps = ((total / (1.0 / 60.0)).ceil() as usize).clamp(1, 6);
        let h = total / steps as f32;
        for _ in 0..steps {
            self.substep(h);
        }
    }

    fn pull_from_scene(&mut self, scene: &Scene) {
        for rb in self.bodies.iter_mut() {
            let Some(node) = scene.get(rb.node) else {
                continue;
            };
            // 脚本用 set_pos 挪过节点：以节点为准并唤醒刚体
            if (node.transform.position - rb.position).length_squared() > 1e-8 {
                rb.position = node.transform.position;
                rb.velocity = Vec3::ZERO;
                rb.sleeping = false;
                rb.sleep_timer = 0.0;
            }
        }
    }

    fn sync_to_scene(&mut self, scene: &mut Scene) {
        let mut dead: Vec<usize> = Vec::new();
        for (i, rb) in self.bodies.iter().enumerate() {
            match scene.get_mut(rb.node) {
                Some(node) => {
                    node.transform.position = rb.position;
                    node.body = Some(rb.handle.0);
                }
                None => dead.push(i),
            }
        }
        // 节点被 despawn 的刚体自动回收，避免句柄泄漏
        for i in dead.into_iter().rev() {
            self.bodies.remove(i);
        }
    }

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.bodies.iter().map(|b| b.handle).collect())
    }

    /// 解析法射线查询：把每个刚体化成世界 AABB 做 slab 测试，取最近命中。
    ///
    /// 内置后端不旋转、形状本身就轴对齐，AABB 即精确包围体；
    /// 球用半径立方体近似（脚本拿来做拾取足够，需要精确球面就走 Rapier）。
    fn raycast(&self, ray: Ray, max_distance: f32) -> Option<RayHit> {
        if max_distance <= 0.0 {
            return None;
        }
        let mut best: Option<RayHit> = None;
        for rb in self.bodies.iter() {
            let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
            let Some((t, normal)) = ray_aabb(&ray, min, max, max_distance) else {
                continue;
            };
            if best.map_or(true, |b| t < b.distance) {
                best = Some(RayHit {
                    handle: rb.handle,
                    node: rb.node,
                    distance: t,
                    point: ray.point_at(t),
                    normal,
                });
            }
        }
        best
    }

    /// 解析法球体重叠：球心到 AABB 的最近距离 <= 半径即算命中。
    fn overlap_sphere(&self, center: Vec3, radius: f32) -> Vec<OverlapHit> {
        if radius <= 0.0 {
            return Vec::new();
        }
        let mut hits: Vec<OverlapHit> = self
            .bodies
            .iter()
            .filter(|rb| {
                let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
                aabb_distance_sq(center, min, max) <= radius * radius
            })
            .map(|rb| OverlapHit {
                handle: rb.handle,
                node: rb.node,
            })
            .collect();
        hits.sort_by_key(|h| h.node);
        hits
    }

    /// 解析法扫掠：把目标 AABB 按查询形状的半尺寸做 **Minkowski 展开**，再用射线 slab 求交。
    ///
    /// 盒 vs 盒是精确的（两个 AABB 的和仍是 AABB）；球用「半径立方体」展开，
    /// 因此在盒子棱角处偏保守（把圆角当方角），需要精确球面接触就走 Rapier。
    /// 这与 `raycast` 的近似口径保持一致，不额外引入第三套几何假设。
    fn shape_cast(
        &self,
        shape: CastShape,
        origin: Vec3,
        dir: Vec3,
        max_distance: f32,
    ) -> Option<ShapeCastHit> {
        if max_distance <= 0.0 || shape.is_degenerate() {
            return None;
        }
        let ray = Ray::new(origin, dir);
        let inflate = shape.half_extents();
        let mut best: Option<ShapeCastHit> = None;
        for rb in self.bodies.iter() {
            let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
            let (min, max) = (min - inflate, max + inflate);
            // 起点已在膨胀盒内 → 初始重叠：距离 0，法线朝来向（与 raycast 内部起点口径一致）
            if point_in_aabb(ray.origin, min, max) {
                let cand = ShapeCastHit {
                    handle: rb.handle,
                    node: rb.node,
                    distance: 0.0,
                    point: ray.origin,
                    normal: -ray.dir,
                    started_inside: true,
                };
                if cast_candidate_better(&cand, best.as_ref()) {
                    best = Some(cand);
                }
                continue;
            }
            let Some((t, normal)) = ray_aabb(&ray, min, max, max_distance) else {
                continue;
            };
            let cand = ShapeCastHit {
                handle: rb.handle,
                node: rb.node,
                distance: t,
                point: ray.point_at(t),
                normal,
                started_inside: false,
            };
            if cast_candidate_better(&cand, best.as_ref()) {
                best = Some(cand);
            }
        }
        best
    }
}

/// 刚体在世界空间的轴对齐包围盒（内置后端不旋转，AABB 精确）。
fn body_aabb(shape: ColliderShape, center: Vec3, half: Vec3) -> (Vec3, Vec3) {
    let h = match shape {
        ColliderShape::Sphere => Vec3::splat(half.x.abs()),
        ColliderShape::Box => half.abs(),
        // 胶囊 / 圆柱在竖直方向更高，水平方向按半径处理
        ColliderShape::Capsule | ColliderShape::Cylinder => {
            Vec3::new(half.x.abs(), half.y.abs(), half.x.abs())
        }
    };
    (center - h, center + h)
}

/// 射线 vs AABB（slab 法）。返回 `(距离, 法线)`；起点在盒内时距离取 0、法线取 `-dir`。
fn ray_aabb(ray: &Ray, min: Vec3, max: Vec3, max_distance: f32) -> Option<(f32, Vec3)> {
    let mut t_enter = 0.0f32;
    let mut t_exit = max_distance;
    let mut normal = -ray.dir;
    for axis in 0..3 {
        let o = ray.origin[axis];
        let d = ray.dir[axis];
        let (lo, hi) = (min[axis], max[axis]);
        if d.abs() < 1e-6 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let mut t1 = (lo - o) * inv;
        let mut t2 = (hi - o) * inv;
        // 从哪一端进入：沿正方向走就是撞到 lo 面（法线朝 -轴），反之撞 hi 面。
        let mut n = Vec3::ZERO;
        n[axis] = if d > 0.0 { -1.0 } else { 1.0 };
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        if t1 > t_enter {
            t_enter = t1;
            normal = n;
        }
        t_exit = t_exit.min(t2);
        if t_enter > t_exit {
            return None;
        }
    }
    (t_enter <= max_distance).then_some((t_enter, normal))
}

/// 点到 AABB 的最近距离平方。
fn aabb_distance_sq(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    let mut d2 = 0.0;
    for axis in 0..3 {
        let v = p[axis];
        let e = if v < min[axis] {
            min[axis] - v
        } else if v > max[axis] {
            v - max[axis]
        } else {
            0.0
        };
        d2 += e * e;
    }
    d2
}

/// 点是否落在 AABB 内（含边界）。
fn point_in_aabb(p: Vec3, min: Vec3, max: Vec3) -> bool {
    (0..3).all(|axis| p[axis] >= min[axis] && p[axis] <= max[axis])
}

/// 扫掠候选的择优规则，两个后端共用同一套，保证换后端不换「谁更近」的判定：
/// 初始重叠优先 → 距离更近优先 → `node` 升序兜底（后端内部表顺序不保证稳定）。
pub(crate) fn cast_candidate_better(cand: &ShapeCastHit, best: Option<&ShapeCastHit>) -> bool {
    let Some(best) = best else {
        return true;
    };
    if cand.started_inside != best.started_inside {
        return cand.started_inside;
    }
    if (cand.distance - best.distance).abs() > 1e-4 {
        return cand.distance < best.distance;
    }
    cand.node < best.node
}

/// 统一推导碰撞体半尺寸：优先 BodyDesc.half_extents，退化到 scale * 0.5。
fn body_half_extents(desc: &BodyDesc) -> Vec3 {
    let h = desc.half_extents;
    if h.length_squared() > 1e-8 {
        h.abs()
    } else {
        (desc.scale.abs() * 0.5).max(Vec3::splat(0.001))
    }
}

// ---------------------------------------------------------------------------
// 门面
// ---------------------------------------------------------------------------

/// 物理世界门面：持有当前后端、开关与配置。
pub struct Physics {
    backend: Box<dyn PhysicsBackend>,
    pub gravity: Vec3,
    /// 总开关：关闭时 step 直接返回，也不接受新刚体。
    enabled: bool,
    kind: PhysicsKind,
    ground: f32,
    /// 高优先级强制项（命令行等）：每次 apply_config 都会覆盖到最终配置上。
    overrides: PhysicsConfig,
    /// 配置世代号：开关 / 后端 / 刚体表的任何重置都会自增，
    /// 供上层（app）判断是否需要重新绑定场景里声明的刚体。
    generation: u64,
}

impl std::fmt::Debug for Physics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Physics")
            .field("backend", &self.backend.name())
            .field("kind", &self.kind.name())
            .field("enabled", &self.enabled)
            .field("gravity", &self.gravity)
            .field("ground", &self.ground)
            .field("bodies", &self.backend.body_count())
            .finish()
    }
}

impl Default for Physics {
    fn default() -> Self {
        Self::new()
    }
}

impl Physics {
    /// 默认：关闭 + null 后端。保持与「物理未接入」时完全一致的行为。
    pub fn new() -> Self {
        Self {
            backend: Box::new(NullBackend::new()),
            gravity: Vec3::new(0.0, -9.81, 0.0),
            enabled: false,
            kind: PhysicsKind::Null,
            ground: 0.0,
            overrides: PhysicsConfig::default(),
            generation: 1,
        }
    }

    pub fn with_backend(mut backend: Box<dyn PhysicsBackend>) -> Self {
        backend.set_gravity(Vec3::new(0.0, -9.81, 0.0));
        Self {
            backend,
            gravity: Vec3::new(0.0, -9.81, 0.0),
            enabled: true,
            kind: PhysicsKind::Builtin,
            ground: 0.0,
            overrides: PhysicsConfig::default(),
            generation: 1,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        // 「开物理」但后端仍是 null（默认）时自动升级到内置后端，
        // 否则用户开了开关却什么都不会动。显式选 null 后端请用 set_kind。
        if enabled && self.kind == PhysicsKind::Null {
            let _ = self.set_kind(PhysicsKind::Builtin);
        }
        if self.enabled != enabled {
            self.enabled = enabled;
            self.generation += 1;
        }
    }

    pub fn kind(&self) -> PhysicsKind {
        self.kind
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub fn ground(&self) -> f32 {
        self.ground
    }

    pub fn set_ground(&mut self, ground: f32) {
        self.ground = ground;
        self.backend.set_ground(ground);
    }

    /// 运行时切换后端（会清空已有刚体、`generation` 自增，脚本需重新 attach）。
    pub fn set_kind(&mut self, kind: PhysicsKind) -> Result<(), String> {
        if !kind.is_available() {
            return Err(format!(
                "物理后端 `{}` 未编译进本构建（当前可用：{}）；带 Rapier 构建：cargo build --features rapier",
                kind.name(),
                PhysicsKind::available_names().join(" / ")
            ));
        }
        if kind == self.kind {
            return Ok(());
        }
        self.kind = kind;
        self.rebuild_backend();
        Ok(())
    }

    /// 应用一份配置（`None` 字段保持原值），并叠加高优先级强制项。
    pub fn apply_config(&mut self, cfg: PhysicsConfig) -> Result<(), String> {
        let merged = cfg.merge_over(&self.overrides);
        if let Some(kind) = merged.kind {
            self.set_kind(kind)?;
        }
        if let Some(g) = merged.gravity {
            self.set_gravity(g);
        }
        if let Some(gr) = merged.ground {
            self.set_ground(gr);
        }
        if let Some(e) = merged.enabled {
            self.set_enabled(e);
        }
        Ok(())
    }

    /// 登记高优先级强制项（命令行参数），并立即生效。
    pub fn apply_override(&mut self, cfg: PhysicsConfig) -> Result<(), String> {
        self.overrides = self.overrides.merge_over(&cfg);
        self.apply_config(PhysicsConfig::default())
    }

    fn rebuild_backend(&mut self) {
        self.backend = match self.kind {
            PhysicsKind::Builtin => Box::new(BuiltinBackend::new(self.gravity, self.ground)),
            PhysicsKind::Rapier => Self::external_backend(self.gravity, self.ground),
            PhysicsKind::Null => Box::new(NullBackend::new()),
        };
        self.backend.set_gravity(self.gravity);
        self.backend.set_ground(self.ground);
        self.generation += 1;
    }

    /// 外部后端工厂：唯一一处受 `feature` 影响的分支。
    /// 将来接 Jolt 等只需在这里加一个 `PhysicsKind` + 一句构造。
    #[cfg(feature = "rapier")]
    fn external_backend(gravity: Vec3, ground: f32) -> Box<dyn PhysicsBackend> {
        Box::new(rapier_backend::RapierBackend::new(gravity, ground))
    }

    /// 未编译外部后端时的兜底：`set_kind` 已经拦下，正常走不到这里。
    #[cfg(not(feature = "rapier"))]
    fn external_backend(_gravity: Vec3, _ground: f32) -> Box<dyn PhysicsBackend> {
        Box::new(NullBackend::new())
    }

    /// 当前后端的能力位。上层按能力分支，别按后端名分支。
    pub fn capabilities(&self) -> BackendCaps {
        self.backend.capabilities()
    }

    /// 取走碰撞事件（需要 `capabilities().collision_events`）。
    pub fn drain_events(&mut self) -> Vec<PhysicsEvent> {
        self.backend.drain_events()
    }

    /// 射线查询：物理关闭或后端不申报 `raycast` 时返回 `None`。
    ///
    /// `max_distance` 用世界单位；`handle` / `node` 可直接用来做拾取后的操作。
    pub fn raycast(&self, ray: Ray, max_distance: f32) -> Option<RayHit> {
        if !self.enabled || !self.backend.capabilities().raycast {
            return None;
        }
        self.backend.raycast(ray, max_distance)
    }

    /// 球体重叠查询：物理关闭或后端不申报 `overlap` 时返回空表。
    pub fn overlap_sphere(&self, center: Vec3, radius: f32) -> Vec<OverlapHit> {
        if !self.enabled || !self.backend.capabilities().overlap {
            return Vec::new();
        }
        self.backend.overlap_sphere(center, radius)
    }

    /// 扫掠查询：把 `shape` 从 `origin` 沿 `dir` 推 `max_distance`，返回最近的一次接触。
    ///
    /// 物理关闭或后端不申报 `shape_cast` 时返回 `None`。`dir` 自动归一化；
    /// 起点已与目标重叠时返回 `distance = 0` 且 `started_inside = true`。
    pub fn shape_cast(
        &self,
        shape: CastShape,
        origin: Vec3,
        dir: Vec3,
        max_distance: f32,
    ) -> Option<ShapeCastHit> {
        if !self.enabled || !self.backend.capabilities().shape_cast {
            return None;
        }
        self.backend.shape_cast(shape, origin, dir, max_distance)
    }

    /// `shape_cast` 的球体便捷版：脚本 / IDE 里最常用的一种（角色胶囊近似、推箱检测）。
    pub fn shape_cast_sphere(
        &self,
        center: Vec3,
        radius: f32,
        dir: Vec3,
        max_distance: f32,
    ) -> Option<ShapeCastHit> {
        self.shape_cast(CastShape::sphere(radius), center, dir, max_distance)
    }

    /// `shape_cast` 的盒体便捷版（AABB 平移检测）。
    pub fn shape_cast_box(
        &self,
        center: Vec3,
        half: Vec3,
        dir: Vec3,
        max_distance: f32,
    ) -> Option<ShapeCastHit> {
        self.shape_cast(CastShape::boxed(half), center, dir, max_distance)
    }

    /// 创建刚体；物理关闭时返回 `None`，由调用方决定如何提示。
    pub fn try_create_body(&mut self, node: NodeId, desc: BodyDesc) -> Option<BodyHandle> {
        if !self.enabled {
            return None;
        }
        Some(self.backend.create_body(node, desc))
    }

    pub fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle {
        self.backend.create_body(node, desc)
    }

    pub fn destroy_body(&mut self, handle: BodyHandle) {
        self.backend.destroy_body(handle)
    }

    pub fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        self.backend.apply_impulse(handle, impulse)
    }

    pub fn step(&mut self, dt: f32, scene: &mut Scene) {
        if !self.enabled {
            return;
        }
        self.backend.pull_from_scene(scene);
        self.backend.step(dt);
        self.backend.sync_to_scene(scene);
    }

    pub fn body_count(&self) -> usize {
        if !self.enabled {
            return 0;
        }
        self.backend.body_count()
    }

    /// 清空所有刚体（脚本热重载时用）。
    pub fn clear_bodies(&mut self) {
        // 后端契约只给了单个销毁，这里用句柄表兜底：后端自己保证幂等。
        if let Some(list) = self.backend.take_all_handles() {
            for h in list {
                self.backend.destroy_body(h);
            }
        }
        self.generation += 1;
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
        self.backend.set_gravity(gravity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::ObjectDesc;

    fn scene_with_ball() -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "ball".into(),
                mesh: Some("sphere".into()),
                position: Some([0.0, 5.0, 0.0]),
                scale: Some([1.0, 1.0, 1.0]),
                ..Default::default()
            })
            .expect("spawn");
        scene.update_world_transforms();
        (scene, id)
    }

    fn ball_desc(pos: Vec3) -> BodyDesc {
        BodyDesc {
            shape: ColliderShape::Sphere,
            half_extents: Vec3::splat(0.5),
            position: pos,
            restitution: 0.5,
            friction: 0.6,
            ..Default::default()
        }
    }

    #[test]
    fn scene_config_then_cli_override_wins() {
        let mut physics = Physics::new();
        // 场景文件说关
        physics
            .apply_config(PhysicsConfig {
                enabled: Some(false),
                kind: Some(PhysicsKind::Builtin),
                ..Default::default()
            })
            .unwrap();
        assert!(!physics.enabled());
        // 命令行说开：优先级更高
        physics
            .apply_override(PhysicsConfig::default().with_enabled(true))
            .unwrap();
        assert!(physics.enabled());
        // 再应用一次场景配置（模拟热重载），命令行强制项依然生效
        physics
            .apply_config(PhysicsConfig {
                enabled: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert!(physics.enabled());
    }

    #[test]
    fn enabling_without_backend_upgrades_to_builtin() {
        let mut physics = Physics::new();
        assert_eq!(physics.kind(), PhysicsKind::Null);
        physics
            .apply_config(PhysicsConfig::default().with_enabled(true))
            .unwrap();
        assert_eq!(physics.kind(), PhysicsKind::Builtin);
        assert!(physics.enabled());
    }

    #[test]
    fn generation_bumps_on_toggle() {
        let mut physics = Physics::new();
        let g0 = physics.generation();
        physics.set_enabled(true);
        assert!(physics.generation() > g0);
        let g1 = physics.generation();
        physics.set_enabled(true); // 无变化不自增
        assert_eq!(physics.generation(), g1);
    }

    /// 造一个与网格来源无关的「长方体」AIR 网格，仅由 8 个角点构成。
    fn air_box(min: Vec3, max: Vec3) -> MeshData {
        let corners = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];
        let vertices: Vec<_> = corners
            .iter()
            .map(|p| r3e_core::Vertex::new([p.x, p.y, p.z], [0.0, 1.0, 0.0], [0.0, 0.0]))
            .collect();
        let indices = vec![
            0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6, 0, 4, 5, 0, 5, 1, 1, 5, 6, 1, 6, 2, 2, 6, 7, 2, 7,
            3, 3, 7, 4, 3, 4, 0,
        ];
        MeshData::new(vertices, indices)
    }

    #[test]
    fn shape_inference_reads_geometry_not_source() {
        // 同一套判定：内置图元的几何 → 对应形状；换成外部资产导出的等价几何，结论一致。
        let sphere = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Sphere);
        let cylinder = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Cylinder);
        let cube = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Cube);

        assert_eq!(infer_shape(&sphere), ColliderShape::Sphere);
        assert_eq!(infer_shape(&cylinder), ColliderShape::Cylinder);
        assert_eq!(infer_shape(&cube), ColliderShape::Box);

        // 顶点位置整体平移后（等价于带偏移的导入模型）判定不变。
        let mut shifted = sphere.clone();
        for vertex in &mut shifted.vertices {
            vertex.position[0] += 12.5;
            vertex.position[2] -= 3.0;
        }
        assert_eq!(infer_shape(&shifted), ColliderShape::Sphere);
    }

    #[test]
    fn infer_reads_mesh_shape() {
        let (scene, id) = scene_with_ball();
        let sphere = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Sphere);
        let desc = BodyDesc::infer_with(scene.get(id).unwrap(), Some(&sphere));
        assert_eq!(desc.shape, ColliderShape::Sphere);
        assert!((desc.half_extents.x - 0.5).abs() < 1e-3);
    }

    #[test]
    fn infer_with_uses_mesh_aabb_instead_of_unit_cube() {
        // 资产网格真实尺寸远大于 1：碰撞体应跟着 AABB 走，而不是退化成 scale*0.5
        let (mut scene, id) = scene_with_ball();
        scene.get_mut(id).unwrap().transform.scale = Vec3::new(2.0, 1.0, 0.5);
        let mesh = air_box(
            Vec3::new(-1.0, 0.0, -0.5),
            Vec3::new(1.0, 2.0, 0.5),
        );
        let desc = BodyDesc::infer_with(scene.get(id).unwrap(), Some(&mesh));
        assert_eq!(desc.shape, ColliderShape::Box);
        // AABB 半尺寸 (1,1,0.5) × scale (2,1,0.5) = (2,1,0.25)
        assert!((desc.half_extents.x - 2.0).abs() < 1e-6, "x={}", desc.half_extents.x);
        assert!((desc.half_extents.y - 1.0).abs() < 1e-6, "y={}", desc.half_extents.y);
        assert!((desc.half_extents.z - 0.25).abs() < 1e-6, "z={}", desc.half_extents.z);
    }

    #[test]
    fn infer_without_mesh_falls_back_to_scale_half() {
        let (scene, id) = scene_with_ball();
        let desc = BodyDesc::infer_with(scene.get(id).unwrap(), None);
        assert_eq!(desc.shape, ColliderShape::Box);
        assert!((desc.half_extents.x - 0.5).abs() < 1e-6);
    }

    #[test]
    fn disabled_by_default_and_ignores_bodies() {
        let mut physics = Physics::new();
        assert!(!physics.enabled());
        let (mut scene, id) = scene_with_ball();
        assert!(physics.try_create_body(id, ball_desc(Vec3::Y * 5.0)).is_none());
        physics.step(1.0 / 60.0, &mut scene);
        assert_eq!(physics.body_count(), 0);
        assert!((scene.get(id).unwrap().transform.position.y - 5.0).abs() < 1e-6);
    }

    #[test]
    fn builtin_enabled_falls_and_rests_on_ground() {
        let mut physics = Physics::new();
        physics
            .apply_config(
                PhysicsConfig::default()
                    .with_enabled(true)
                    .with_kind(PhysicsKind::Builtin),
            )
            .unwrap();
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(Vec3::Y * 5.0));
        assert_eq!(physics.body_count(), 1);

        for _ in 0..600 {
            physics.step(1.0 / 60.0, &mut scene);
        }
        let y = scene.get(id).unwrap().transform.position.y;
        // 半径 0.5 + 地面 0 => 静止在 0.5
        assert!((y - 0.5).abs() < 1e-3, "y = {y}");
    }

    #[test]
    fn restitution_bounces_before_settling() {
        let mut physics = Physics::new();
        physics
            .apply_config(
                PhysicsConfig::default()
                    .with_enabled(true)
                    .with_kind(PhysicsKind::Builtin),
            )
            .unwrap();
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(Vec3::Y * 5.0));

        let mut bounced_upward = false;
        let mut last_y = 5.0_f32;
        for _ in 0..300 {
            physics.step(1.0 / 60.0, &mut scene);
            let y = scene.get(id).unwrap().transform.position.y;
            if y > last_y + 1e-3 {
                bounced_upward = true;
            }
            last_y = y;
        }
        assert!(bounced_upward, "弹性 0.5 应当反弹");
    }

    #[test]
    fn script_move_wakes_sleeping_body() {
        let mut physics = Physics::new();
        physics
            .apply_config(
                PhysicsConfig::default()
                    .with_enabled(true)
                    .with_kind(PhysicsKind::Builtin),
            )
            .unwrap();
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(Vec3::Y * 1.0));
        for _ in 0..300 {
            physics.step(1.0 / 60.0, &mut scene);
        }
        // 脚本把球抬到空中，物理应重新接管并让它下落
        scene.get_mut(id).unwrap().transform.position = Vec3::Y * 4.0;
        for _ in 0..60 {
            physics.step(1.0 / 60.0, &mut scene);
        }
        let y = scene.get(id).unwrap().transform.position.y;
        assert!(y < 4.0, "唤醒后应当下落，y = {y}");
    }

    #[test]
    fn switching_backend_clears_bodies() {
        let mut physics = Physics::new();
        physics
            .apply_config(
                PhysicsConfig::default()
                    .with_enabled(true)
                    .with_kind(PhysicsKind::Builtin),
            )
            .unwrap();
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(Vec3::Y * 2.0));
        assert_eq!(physics.body_count(), 1);
        physics.set_kind(PhysicsKind::Null).unwrap();
        assert_eq!(physics.body_count(), 0);
        // rapier 槽位诚实地跟随构建特性：没编进去就报错，编进去了就能切
        let switched = physics.set_kind(PhysicsKind::Rapier);
        assert_eq!(switched.is_ok(), cfg!(feature = "rapier"), "{switched:?}");
        assert!(physics.set_kind(PhysicsKind::Null).is_ok());
        physics.step(1.0 / 60.0, &mut scene);
    }

    #[test]
    fn ground_height_is_configurable() {
        let mut physics = Physics::new();
        physics
            .apply_config(PhysicsConfig {
                enabled: Some(true),
                kind: Some(PhysicsKind::Builtin),
                gravity: None,
                ground: Some(2.0),
            })
            .unwrap();
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(Vec3::Y * 6.0));
        for _ in 0..600 {
            physics.step(1.0 / 60.0, &mut scene);
        }
        let y = scene.get(id).unwrap().transform.position.y;
        assert!((y - 2.5).abs() < 1e-3, "y = {y}");
    }
}
