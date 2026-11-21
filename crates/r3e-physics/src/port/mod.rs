//! 物理查询端口（M2 · A03 / D1=B）：上层与具体物理引擎之间的唯一查询边界。
//!
//! ## 为什么单独成模块
//!
//! 门面（[`Physics`](crate::Physics)）已经把「后端可换」收在 crate 内：`Builtin` / `Rapier` /
//! `Null` 三套实现并存，换后端不动上层。但门面 API 的类型（`BodyDesc` / `Ray` / `CastShape` /
//! `BackendCaps`）都是**具体实现态**的口径，调用方一旦持有门面，就把后端、生命周期、
//! 内部可变性一起绑死。
//!
//! 本模块把「上层真正需要的三件事」抽成 trait：射线打一下、胶囊站不站得下、这块地面多高。
//! 上层（角色控制器 / 脚本 / 运行时）只认这三个 trait 与五个 DTO，不认后端是谁，
//! 也不认门面怎么持有。
//!
//! ```text
//!   r3e-character / r3e-runtime（B 系列）
//!              │  &dyn PhysicsQueryPort（本模块）
//!              ▼
//!      r3e-physics::port  ◄── 只依赖 r3e-core（NodeId）+ glam
//!              ▲
//!              │  impl RaycastPort / CapsulePort / GroundPort（port_impl.rs）
//!           Physics 门面 → Builtin / Rapier / Null
//! ```
//!
//! ## 落点说明（2026-11-20 修订）
//!
//! A03 原计划把本模块放进 `r3e-core`，理由是 D2=B 的 `r3e-character` 只许依赖 core。
//! 按项目负责人指令改放本 crate 后：端口与实现同库、少一层跨 crate 往返；代价是消费端
//! 多一条 `r3e-physics` 依赖边（`xtask/dep_rules.json` 的白名单已在 B01 同步登记）。
//! `r3e-core` 因此保持「零引擎依赖」不变 —— 依赖方向仍是 `physics → core`，没有回边。
//!
//! ## 口径约定（三组查询共用）
//!
//! * 方向向量一律先归一化；返回的 `distance` 是**世界距离**，不是某个 t 参数。
//! * 起点已重叠时 `distance == 0` 且 `started_inside == true`，不静默丢命中。
//! * 查询只覆盖「物理世界里的刚体」，后端自带的地面平面只在 [`GroundPort`] 里体现；
//!   这与 crate 内 `raycast` / `overlap_sphere` 既有的口径保持一致。
//! * 申报与行为必须一致：`port_caps()` 报 `false` 的能力，对应方法必须返回空结果
//!   （`None` / 空 `Vec`），不得「能查却报不支持」或「报支持却查不到」。
//!
//! ## 胶囊为什么是竖直的
//!
//! 角色控制器（M2 · B 系列）站姿 / 蹲姿都沿 +Y 轴，本模块只提供**轴对齐**的
//! [`Capsule`]：中心点 + 半高 + 半径。倾斜胶囊等到真有需求时再加变体，
//! 现在多一个自由度只会让三套后端各写一遍四元数转换。

use glam::Vec3;
use r3e_core::NodeId;

/// 射线查询输入。
///
/// `dir` 允许非单位向量，实现方须先归一化；`max_distance` 是归一化之后的世界距离上限。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastRay {
    /// 起点（世界坐标）。
    pub origin: Vec3,
    /// 方向，非零即可，不必归一化。
    pub dir: Vec3,
    /// 最大查询距离（世界单位）。
    pub max_distance: f32,
}

impl CastRay {
    pub fn new(origin: Vec3, dir: Vec3, max_distance: f32) -> Self {
        Self {
            origin,
            dir,
            max_distance,
        }
    }

    /// 归一化方向；方向退化时返回 `Vec3::ZERO`。
    pub fn normalized_dir(&self) -> Vec3 {
        self.dir.normalize_or_zero()
    }

    /// 沿归一化方向推进 `t` 后的世界点。
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.normalized_dir() * t
    }

    /// 方向退化或距离非正 → 这条射线没有意义，实现方应直接返回 `None`。
    pub fn is_degenerate(&self) -> bool {
        self.normalized_dir() == Vec3::ZERO
            || self.max_distance.is_nan()
            || self.max_distance <= 0.0
    }
}

/// 端口命中结果：射线 / 胶囊重叠 / 胶囊扫掠共用一套字段。
///
/// 重叠查询没有「接触法线」的概念，此时 `normal` 为 [`Vec3::ZERO`]、
/// `distance` 为 `0`、`started_inside` 为 `true`，调用方按 `started_inside` 区分即可。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortHit {
    /// 后端刚体句柄（`r3e-physics` 的 `BodyHandle.0`，对上层只当作不透明 id）。
    pub body: u64,
    /// 该刚体绑定的场景节点。
    pub node: NodeId,
    /// 世界距离；重叠查询恒为 0。
    pub distance: f32,
    /// 命中点（世界坐标）；重叠查询取查询形状的中心。
    pub point: Vec3,
    /// 命中处指向查询体的法线；重叠查询为 `ZERO`。
    pub normal: Vec3,
    /// 查询起点已与目标重叠（此时 `distance == 0`）。
    pub started_inside: bool,
}

/// 竖直胶囊：轴向固定 +Y，中心点 + 半高 + 半径。
///
/// 圆柱段高度 = `2 * half_height`，两端各一个半径 `radius` 的半球，
/// 于是整体高度 = `2 * (half_height + radius)`、整体半径 = `radius`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    /// 胶囊几何中心（世界坐标）。
    pub center: Vec3,
    /// 圆柱段半高（不含两端半球）。
    pub half_height: f32,
    /// 半径（必须为正才有效）。
    pub radius: f32,
}

impl Capsule {
    pub fn new(center: Vec3, half_height: f32, radius: f32) -> Self {
        Self {
            center,
            half_height,
            radius,
        }
    }

    /// 整体高度（含两端半球）。
    pub fn height(&self) -> f32 {
        2.0 * (self.half_height.abs() + self.radius.abs())
    }

    /// 半径非正 / 数值非有限 → 这条查询没有意义。
    ///
    /// 半径要求严格 `> 0`：负半径属于调用方算错，不在这里用 `abs` 悄悄纠正，
    /// 否则错误会被静默吞掉，症状推迟到「角色卡在地里」才暴露。
    pub fn is_degenerate(&self) -> bool {
        self.radius.is_nan()
            || self.radius <= 0.0
            || !self.center.is_finite()
            || !self.half_height.is_finite()
    }

    /// 胶囊的轴对齐包围盒 `(min, max)`；内置后端用它做解析近似查询。
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let r = self.radius.abs();
        let h = Vec3::new(r, self.half_height.abs() + r, r);
        (self.center - h, self.center + h)
    }
}

/// 地表采样结果：给定世界 `(x, z)`，返回该处「可站立的最高表面」。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundSample {
    /// 表面高度（世界 Y）。
    pub height: f32,
    /// 表面法线；平面地面为 `+Y`。
    pub normal: Vec3,
    /// 提供该表面的刚体句柄；平地（后端自带地面）为 `None`。
    pub body: Option<u64>,
    /// 提供该表面的场景节点；平地（后端自带地面）为 `None`。
    pub node: Option<NodeId>,
}

impl GroundSample {
    /// 平地采样（后端自带地面平面）。
    pub fn flat(height: f32) -> Self {
        Self {
            height,
            normal: Vec3::Y,
            body: None,
            node: None,
        }
    }

    /// 该表面是否足够平（`normal.y >= cos(45°)`），落地判定常用。
    pub fn is_walkable(&self, min_up_dot: f32) -> bool {
        self.normal.normalize_or_zero().y >= min_up_dot
    }
}

/// 端口能力位表。
///
/// 与 `r3e-physics::BackendCaps` 分开申报：后者描述的是**物理模拟**能力
/// （互撞、旋转、CCD、事件），这里只描述**查询端口**是否可用。
/// 两者不共享字段是有意的——端口能力将来会随角色控制器扩展，
/// 而不应该反过来拉动脚本能看到的模拟能力面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PortCaps {
    /// 射线查询（[`RaycastPort::cast_ray`]）。
    pub raycast: bool,
    /// 胶囊查询（[`CapsulePort::capsule_overlap`] / [`CapsulePort::capsule_sweep`]）。
    pub capsule: bool,
    /// 地表采样（[`GroundPort::sample_ground`]）。
    pub ground: bool,
}

impl PortCaps {
    /// 什么都不支持（空后端 / 不支持查询的后端）。
    pub const NONE: Self = Self {
        raycast: false,
        capsule: false,
        ground: false,
    };

    /// 三组查询全开（内置后端与 Rapier 后端当前都落在这一档）。
    pub const FULL: Self = Self {
        raycast: true,
        capsule: true,
        ground: true,
    };

    /// 是否一个能力都没申报。
    pub fn is_empty(&self) -> bool {
        !self.raycast && !self.capsule && !self.ground
    }

    /// 能力键名（稳定标识，供日志与测试消费）。
    pub fn enabled_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.raycast {
            keys.push("raycast");
        }
        if self.capsule {
            keys.push("capsule");
        }
        if self.ground {
            keys.push("ground");
        }
        keys
    }

    /// 逗号分隔摘要；全关时返回 `none`。
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

/// 射线查询端口。
pub trait RaycastPort {
    /// 从 `ray.origin` 沿 `ray.dir` 打一条射线，返回最近命中的刚体。
    ///
    /// 默认实现返回 `None`：不支持就别假装，调用方按 `port_caps().raycast` 判断。
    fn cast_ray(&self, ray: &CastRay) -> Option<PortHit> {
        let _ = ray;
        None
    }
}

/// 胶囊查询端口（角色控制器的站姿/蹲姿判定就靠这两条）。
pub trait CapsulePort {
    /// 列出与 `capsule` 相交的所有刚体（按 `node` 升序，便于复现）。
    fn capsule_overlap(&self, capsule: &Capsule) -> Vec<PortHit> {
        let _ = capsule;
        Vec::new()
    }

    /// 把 `capsule` 沿 `dir` 推 `max_distance`，返回最近的一次接触。
    ///
    /// 起点已重叠时返回 `distance == 0` 且 `started_inside == true` 的命中，
    /// 由调用方决定是「贴脸滑开」还是「原地不动」。
    fn capsule_sweep(&self, capsule: &Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        let _ = (capsule, dir, max_distance);
        None
    }
}

/// 地表采样端口。
pub trait GroundPort {
    /// 查询世界 `(x, z)` 处可站立的最高表面。
    ///
    /// 输入非有限值时返回 `None`；后端自带的地面平面恒存在，
    /// 所以正常情况下**总能**拿到一个样本（没有刚体就返回平地）。
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        let _ = (x, z);
        None
    }
}

/// 只读查询端口：三组 trait 的合集 + 能力申报。
///
/// 上层拿 `&dyn PhysicsQueryPort` 就够了，不需要知道背后是 Builtin、Rapier 还是别的引擎；
/// 也不需要 `use r3e_physics::...`（这正是 A03 要买的东西）。
pub trait PhysicsQueryPort: RaycastPort + CapsulePort + GroundPort {
    /// 如实申报本端口支持哪几组查询。
    fn port_caps(&self) -> PortCaps;
}

/// 角色移动端口：水平意图 + 竖直结算（M2 · B01，见 [`character`]）。
mod character;
pub use character::*;
