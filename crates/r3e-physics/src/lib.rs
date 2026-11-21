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
//! 物理默认**关闭**（`Physics::new()` 即 Null 后端 + enabled=false），三层任一入口可开：
//! 命令行 `--physics on` / 场景 `physics.enabled` / 脚本 `physics_set_enabled(true)`。
//! 关闭时不积分、`attach_body` 返 false、`body_count` 恒 0，物件全由脚本驱动（同旧行为）。
//!
//! **可切换性是硬约束**：`rapier3d` / `parry` 的类型只允许出现在 `rapier_backend.rs`，
//! 门面、`app`、脚本层只接触 `glam` 向量与本文件的契约类型。

use glam::Vec3;
use r3e_core::{Aabb, MeshData, Node, NodeId, PhysicsSceneDesc, Scene};

use crate::port::{Capsule, GroundSample, PortHit};
/// 契约类型（能力位、描述、事件、查询结果）。
mod types;
pub use types::*;

/// 后端契约 trait 与跨后端共用的择优工具。
mod backend;
pub(crate) use backend::cast_candidate_better;
pub use backend::*;

mod bind;
pub use bind::BindReport;

/// 内置 / 空后端（零 rapier 依赖，`--no-default-features` 下唯一可用的实现）。
mod builtin_backend;
pub use builtin_backend::{BuiltinBackend, NullBackend};

mod host_adapter;
/// 物理查询端口：上层与具体后端的唯一查询边界（M2 · A03），拿 `&dyn PhysicsQueryPort` 即可。
pub mod port;

/// 能力面门面（M2-A03b）：把真门面适配成 `r3e_core::host` 契约。
pub use host_adapter::PhysicsFacade;
mod port_impl;
/// 端口适配：把 `PhysicsQueryPort` 三组查询接到门面上（纯字段搬运，无决策逻辑）。
mod slope;

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
    /// 逐刚体 CCD 的缺省开关（`BodyDesc::continuous = None` 时生效），缺省开。
    ccd_default: bool,
    /// 配置世代号：开关 / 后端 / 刚体表的任何重置都会自增，
    /// 供上层（app）判断是否需要重新绑定场景里声明的刚体。
    generation: u64,
    /// 上一次 [`Physics::bind_scene`] 产出的「节点 → 句柄」表。
    ///
    /// 存在的唯一原因：让**只读**装配入口自己就能幂等。只读调用方（内容验证器）
    /// 不会把句柄写回场景，若幂等只能靠 `Node::body` 判断，它每调一次就会重建一批
    /// 刚体——同一个场景两次校验得到两套句柄，验证结果也就跟着漂。
    last_bindings: Vec<(NodeId, u64)>,
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
            ccd_default: true,
            generation: 1,
            last_bindings: Vec::new(),
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
            ccd_default: true,
            generation: 1,
            last_bindings: Vec::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        // 「开物理」但后端仍是 null（默认）时自动升级到**当前构建的默认后端**，
        // 否则用户开了开关却什么都不会动。显式选 null 后端请用 set_kind。
        if enabled && self.kind == PhysicsKind::Null {
            let _ = self.set_kind(PhysicsKind::preferred_default());
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

    /// 逐刚体 CCD 的缺省开关（场景 `physics.ccd` / 命令行 `--ccd` 落到这里）。
    ///
    /// 返回值是「当前后端是否支持 CCD」：`false` 表示这个开关在后端层面无效，
    /// 上层（脚本 / IDE）应如实告知用户，而不是假装生效。
    pub fn set_ccd_default(&mut self, on: bool) -> bool {
        self.ccd_default = on;
        self.backend.set_ccd_default(on);
        self.backend.capabilities().ccd
    }

    /// 当前 CCD 缺省开关。
    pub fn ccd_default(&self) -> bool {
        self.ccd_default
    }

    /// 给单个刚体开关 CCD；后端不支持时返回 `false`。
    pub fn set_ccd(&mut self, handle: BodyHandle, on: bool) -> bool {
        self.backend.set_ccd(handle, on)
    }

    /// 读回单个刚体当前 CCD 状态；后端不支持时返回 `None`。
    pub fn ccd_of(&self, handle: BodyHandle) -> Option<bool> {
        self.backend.ccd_of(handle)
    }

    /// 把 CCD 开关应用到**所有**现存刚体（脚本 `physics_set_ccd` 用）。
    ///
    /// 返回实际改到的刚体数：后端没有句柄表或不支持 CCD 时为 0。
    pub fn set_ccd_all(&mut self, on: bool) -> usize {
        let Some(handles) = self.backend.take_all_handles() else {
            return 0;
        };
        let mut n = 0;
        for h in handles {
            if self.backend.set_ccd(h, on) {
                n += 1;
            }
        }
        n
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
        if let Some(ccd) = merged.ccd {
            // 只改「缺省值」：场景里显式写了 `physics.ccd` 的刚体仍以逐物件为准
            self.set_ccd_default(ccd);
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
        self.backend.set_ccd_default(self.ccd_default);
        // 切后端后旧句柄在后端里全部失效，缓存必须跟着清，
        // 否则下一次装配会拿一批死句柄去「复用」。
        self.last_bindings.clear();
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

    /// 胶囊重叠查询（[`port::CapsulePort`] 的门面版）：物理关闭时返回空表。
    ///
    /// 走的是后端的 `overlap_capsule`，结果已按 `node` 升序；后端不支持时由
    /// 后端默认实现返回空表 —— 门面不额外造一套「不支持」的错误语义。
    pub fn overlap_capsule(&self, capsule: Capsule) -> Vec<PortHit> {
        if !self.enabled {
            return Vec::new();
        }
        self.backend.overlap_capsule(capsule)
    }

    /// 胶囊扫掠查询（[`port::CapsulePort`] 的门面版）：物理关闭时返回 `None`。
    pub fn sweep_capsule(&self, capsule: Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        if !self.enabled {
            return None;
        }
        self.backend.sweep_capsule(capsule, dir, max_distance)
    }

    /// 地表采样（[`port::GroundPort`] 的门面版）：物理关闭时返回 `None`。
    ///
    /// 与端口契约一致：开启时只要后端自带地面，正常情况下总能拿到样本。
    pub fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        if !self.enabled {
            return None;
        }
        self.backend.sample_ground(x, z)
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
mod tests;
