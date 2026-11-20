//! 物理契约类型：能力位、刚体 / 碰撞描述、事件与查询结果（M2-A05 自 `lib.rs` 横切抽出）。
//!
//! 只放数据与纯函数，不含后端实现，也不含门面逻辑。

use super::*;

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
    /// 连续碰撞检测（CCD）。
    ///
    /// * `None` —— 跟随后端默认（`Physics::ccd_default`，缺省开）；
    /// * `Some(true/false)` —— 这个刚体显式开关，压过后端默认。
    ///
    /// 三态而非 `bool`，是为了让「场景没写」与「场景写 false」区分开：
    /// 前者应当听全局，后者必须能真正退出 CCD。
    pub continuous: Option<bool>,
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
            continuous: None,
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
}

/// 碰撞体半尺寸：有真实 AABB 就用它乘缩放，否则退回 `scale * 0.5`。
///
/// 下限 1e-3 避免零厚度网格生成退化碰撞体。
pub(crate) fn half_extents_from(bounds: Option<Aabb>, scale: Vec3) -> Vec3 {
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

    /// 当前构建的**默认后端**：编译了 rapier 就用 rapier，否则退到内置后端。
    ///
    /// D1=B 之后「默认物理」不再是内置近似后端：能上真引擎就上真引擎，
    /// 内置后端退居「无 rapier 构建时的降级实现」。
    /// 只在**没有显式指定后端**时使用 —— 场景/命令行显式给出的 kind 永远优先。
    pub fn preferred_default() -> Self {
        if cfg!(feature = "rapier") {
            Self::Rapier
        } else {
            Self::Builtin
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
    /// 逐刚体 CCD 的缺省值（`BodyDesc::continuous = None` 时生效），缺省视为 `true`。
    pub ccd: Option<bool>,
}

impl PhysicsConfig {
    pub fn from_scene(desc: &PhysicsSceneDesc) -> Self {
        Self {
            enabled: desc.enabled,
            kind: desc.backend.as_deref().and_then(PhysicsKind::parse),
            gravity: desc.gravity.map(Vec3::from),
            ground: desc.ground,
            ccd: desc.ccd,
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

    pub fn with_ccd(mut self, ccd: bool) -> Self {
        self.ccd = Some(ccd);
        self
    }

    /// `other` 中已明确指定的字段覆盖 `self`。
    pub fn merge_over(&self, other: &PhysicsConfig) -> PhysicsConfig {
        PhysicsConfig {
            enabled: other.enabled.or(self.enabled),
            kind: other.kind.or(self.kind),
            gravity: other.gravity.or(self.gravity),
            ground: other.ground.or(self.ground),
            ccd: other.ccd.or(self.ccd),
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
