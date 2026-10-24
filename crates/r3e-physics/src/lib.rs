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
use r3e_core::{Aabb, BuiltinMesh, Node, NodeId, PhysicsSceneDesc, Scene};

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
}

impl BackendCaps {
    /// 什么都不做（`null` 后端）。
    pub const NONE: Self = Self {
        body_body_contacts: false,
        rotational_dynamics: false,
        joints: false,
        ccd: false,
        collision_events: false,
    };

    /// 内置后端：只有「刚体 vs 地面」这一条接触规则，无旋转、无事件、无 CCD。
    pub const BASIC: Self = Self::NONE;

    /// 完整能力（Rapier 这一档）。
    pub const FULL: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: true,
        ccd: true,
        collision_events: true,
    };

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

    /// 同 [`BodyDesc::infer`]，但可用网格真实包围盒推出更贴合的碰撞体尺寸。
    ///
    /// `bounds` 是网格**局部**空间的 AABB（来自 `MeshProvider::mesh_aabb`）；给了就按
    /// `AABB 半尺寸 × scale` 推导，缺失则退回 `scale * 0.5`。
    pub fn infer_with(node: &Node, bounds: Option<Aabb>) -> Self {
        let shape = match node.mesh {
            r3e_core::MeshRef::Builtin(BuiltinMesh::Sphere) => ColliderShape::Sphere,
            r3e_core::MeshRef::Builtin(BuiltinMesh::Cylinder) => ColliderShape::Cylinder,
            _ => ColliderShape::Box,
        };
        let scale = node.transform.scale.abs();
        Self {
            kind: BodyKind::Dynamic,
            shape,
            half_extents: half_extents_from(bounds, scale),
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
    /// 外部资产网格（`MeshRef::Asset`）的真实尺寸往往远不是 1×1×1，退化成
    /// `scale * 0.5` 会让碰撞体明显偏小；这里让 AABB 参与推导。
    /// 显式写了 `half_extents` 的场景仍然以场景为准（人工调过的值优先）。
    pub fn from_node_with(node: &Node, bounds: Option<Aabb>) -> Option<Self> {
        let p = node.physics.as_ref()?;
        let shape = p
            .collider
            .as_deref()
            .map(ColliderShape::parse)
            .unwrap_or(match node.mesh {
                r3e_core::MeshRef::Builtin(BuiltinMesh::Sphere) => ColliderShape::Sphere,
                r3e_core::MeshRef::Builtin(BuiltinMesh::Cylinder) => ColliderShape::Cylinder,
                _ => ColliderShape::Box,
            });
        let scale = node.transform.scale.abs();
        let half_extents = p
            .half_extents
            .map(|h| Vec3::from(h).abs())
            .unwrap_or_else(|| half_extents_from(bounds, scale));
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

    #[test]
    fn infer_reads_mesh_shape() {
        let (scene, id) = scene_with_ball();
        let desc = BodyDesc::infer(scene.get(id).unwrap());
        assert_eq!(desc.shape, ColliderShape::Sphere);
        assert!((desc.half_extents.x - 0.5).abs() < 1e-6);
    }

    #[test]
    fn infer_with_uses_mesh_aabb_instead_of_unit_cube() {
        // 资产网格真实尺寸远大于 1：碰撞体应跟着 AABB 走，而不是退化成 scale*0.5
        let (mut scene, id) = scene_with_ball();
        scene.get_mut(id).unwrap().transform.scale = Vec3::new(2.0, 1.0, 0.5);
        let bounds = Aabb::new(Vec3::new(-1.0, 0.0, -0.5), Vec3::new(1.0, 2.0, 0.5));
        let desc = BodyDesc::infer_with(scene.get(id).unwrap(), Some(bounds));
        // AABB 半尺寸 (1,1,0.5) × scale (2,1,0.5) = (2,1,0.25)
        assert!((desc.half_extents.x - 2.0).abs() < 1e-6, "x={}", desc.half_extents.x);
        assert!((desc.half_extents.y - 1.0).abs() < 1e-6, "y={}", desc.half_extents.y);
        assert!((desc.half_extents.z - 0.25).abs() < 1e-6, "z={}", desc.half_extents.z);
    }

    #[test]
    fn infer_without_bounds_falls_back_to_scale_half() {
        let (scene, id) = scene_with_ball();
        let desc = BodyDesc::infer_with(scene.get(id).unwrap(), None);
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
