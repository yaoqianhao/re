//! Rapier3D 物理后端（`feature = "rapier"`）。
//!
//! **本文件是整条物理链路里唯一允许出现 `rapier3d` / `parry` / `nalgebra` 类型的地方。**
//! 门面（`lib.rs`）、`app`、脚本层只接触 `glam::Vec3` / `Quat` 与 `PhysicsBackend` 契约。
//! 因此换后端 = 换掉本文件，其它代码零改动。
//!
//! 与内置后端的能力差异（属能力升级，不是接口破坏）：
//! * 刚体之间互相碰撞、可堆叠
//! * 旋转动力学（翻滚、角速度、力矩）
//! * 碰撞事件（`collision_events` 能力位）
//!
//! 能力申报口径（见 `BackendCaps::EXPOSED`）：本后端**只申报门面真能用的能力**。
//! Rapier 底层本来就有 CCD 与关节（`ccd_solver` / `impulse_joints` / `multibody_joints`
//! 都已在求解器里接好）。CCD 一侧的门面接口（`set_ccd` / `ccd_of` / `set_ccd_default`）
//! 已补齐，所以本后端申报 `CONTACT_CCD`；关节仍没有建关节的方法、脚本也没有入口，
//! `joints` 继续报 `false`。
//!
//! CCD 口径提醒：rapier 0.36 对**固定**碰撞体本来就做自动扫掠（`max_ccd_substeps != 0` 时），
//! `enable_ccd(true)` 额外把扫掠范围扩到**运动学 / 动态**刚体。所以「子弹穿静态墙」
//! 在关掉 CCD 时未必会穿——做穿透对照必须让靶体是运动学或动态体。
//!
//! 语义对齐点（与内置后端保持一致，避免换后端后脚本行为跳变）：
//! * `BodyKind::Static` 完全不动，`Kinematic` 每帧从场景节点推位置，`Dynamic` 由物理积分
//! * 脚本直接改动态刚体节点的位置 → `pull_from_scene` 识别为「瞬移」并清零速度（不保留旧动量）
//! * 节点被移除 → `sync_to_scene` 回收对应刚体，无句柄泄漏

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use r3e_core::{NodeId, Scene};
use rapier3d::na::Unit;
use rapier3d::parry::query::{
    cast_shapes, intersection_test, Ray as ParryRay, ShapeCastOptions, ShapeCastStatus,
};
use rapier3d::parry::shape::{Ball, Capsule as ParryCapsule, Cuboid};
use rapier3d::prelude::*;

use crate::port::{Capsule, GroundSample, PortHit};
use crate::rapier_glue::{
    collider_for, quat_to_glam, quat_to_rapier, to_glam, to_rapier, EventCollector,
    GROUND_USER_DATA,
};
use crate::{
    cast_candidate_better, BackendCaps, BodyDesc, BodyHandle, BodyKind, CastShape, OverlapHit,
    PhysicsBackend, PhysicsEvent, Ray, RayHit, ShapeCastHit,
};

/// 判定「脚本挪了节点」的位置阈值（米）。低于它认为节点没被动过。
const TELEPORT_EPS: f32 = 1e-4;

/// 单帧最多切几个子步，与内置后端保持同一节奏（避免大 dt 一帧穿透）。
const MAX_SUBSTEPS: usize = 6;

/// 地表采样时「这个面算不算地上」的法线下限（`normal.y`）。
///
/// 取 0.5（约 60°）：比角色 45° 的爬坡上限宽松，只用来排除**明显是障碍侧面**的
/// 命中（斜置箱体的端面、台阶/高台的立面）。门槛卡在 45° 会把角色能走的陡坡
/// 也当成障碍；卡得太低（如 0.2）又会放过 75° 的楔形棱面 —— 那正是 B05 接缝
/// 卡死的元凶。
const GROUND_STEEP_MIN: f32 = 0.5;

/// 地表采样遇到陡面时，最多再往下探几段。
///
/// 正常场地一两段就够（棱面 → 顶面）；给个上限只是防止病态几何把查询拖成死循环。
const GROUND_PROBE_STEPS: usize = 4;

/// 一条刚体记录：句柄 ↔ Rapier 实体 ↔ 场景节点。
struct Record {
    body: RigidBodyHandle,
    /// 挂在刚体上的碰撞体；查询（raycast / overlap）按它做窄相检测。
    collider: ColliderHandle,
    node: NodeId,
    kind: BodyKind,
    /// 该刚体建体时的 CCD 解析结果（`BodyDesc::continuous` 或后端默认）。
    continuous: bool,
}

/// Rapier 后端。所有 Rapier 状态集中在这里，不向上层泄漏。
pub struct RapierBackend {
    next: u64,
    gravity: Vec3,
    ground: f32,
    ground_body: RigidBodyHandle,
    pipeline: PhysicsPipeline,
    integration: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd_solver: CCDSolver,
    collector: EventCollector,
    events: Arc<Mutex<Vec<PhysicsEvent>>>,
    records: HashMap<BodyHandle, Record>,
    /// 刚体未显式指定 CCD 时的缺省值（`Physics::ccd_default` 推下来）。
    ccd_default: bool,
}

impl RapierBackend {
    pub fn new(gravity: Vec3, ground: f32) -> Self {
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();

        // 地面：无限大半空间，法线朝 +Y，位置随 `ground` 平移。
        // 与内置后端的「无限平面」语义一致，场景里那条 plane 只负责显示。
        let ground_body = bodies.insert(
            RigidBodyBuilder::fixed()
                .translation(Vector::new(0.0, ground, 0.0))
                .user_data(GROUND_USER_DATA),
        );
        colliders.insert_with_parent(
            ColliderBuilder::halfspace(Unit::new_unchecked(Vector::Y))
                .friction(0.6)
                .active_events(ActiveEvents::COLLISION_EVENTS),
            ground_body,
            &mut bodies,
        );

        let events: Arc<Mutex<Vec<PhysicsEvent>>> = Arc::default();
        let collector = EventCollector {
            sink: Arc::clone(&events),
        };

        Self {
            next: 1,
            gravity,
            ground,
            ground_body,
            pipeline: PhysicsPipeline::new(),
            integration: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies,
            colliders,
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            ccd_solver: CCDSolver::new(),
            collector,
            events,
            records: HashMap::new(),
            // 与 `Physics::ccd_default` 的缺省保持一致：开
            ccd_default: true,
        }
    }

    /// 推进一个子步。`&self.collector` 只做事件外传，不持有可变状态冲突。
    fn substep(&mut self, h: f32) {
        self.integration.dt = h;
        let gravity = to_rapier(self.gravity);
        self.pipeline.step(
            gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd_solver,
            &(),
            &self.collector,
        );
    }
}

impl PhysicsBackend for RapierBackend {
    fn name(&self) -> &'static str {
        "rapier"
    }

    fn capabilities(&self) -> BackendCaps {
        // 刚体互撞 + 旋转 + 事件 + CCD；关节仍无门面入口，故不报 `joints`。
        BackendCaps::CONTACT_CCD
    }

    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;

        let builder = match desc.kind {
            BodyKind::Static => RigidBodyBuilder::fixed(),
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
            BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
        }
        .translation(to_rapier(desc.position))
        .user_data(node as u128);

        let continuous = desc.continuous.unwrap_or(self.ccd_default);
        let rb = self.bodies.insert(builder);
        if let Some(body) = self.bodies.get_mut(rb) {
            body.set_rotation(quat_to_rapier(desc.rotation), false);
            // 静态 / 运动学刚体由脚本驱动，CCD 对它们没有意义；只在动态体上开
            if continuous && matches!(desc.kind, BodyKind::Dynamic) {
                body.enable_ccd(true);
            }
        }
        let collider = self
            .colliders
            .insert_with_parent(collider_for(&desc), rb, &mut self.bodies);

        self.records.insert(
            handle,
            Record {
                body: rb,
                collider,
                node,
                kind: desc.kind,
                continuous,
            },
        );
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        let Some(rec) = self.records.remove(&handle) else {
            return;
        };
        // 一并移除挂在刚体上的碰撞体，避免残留在 broad phase 里。
        self.bodies.remove(
            rec.body,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            true,
        );
    }

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        let Some(rec) = self.records.get(&handle) else {
            return;
        };
        if let Some(body) = self.bodies.get_mut(rec.body) {
            // `true` = 唤醒（休眠体收到冲量必须醒，否则冲量白给）
            body.apply_impulse(to_rapier(impulse), true);
        }
    }

    fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
        // Rapier 的重力在 step 时按值传入，这里只需唤醒所有刚体让它们重新响应
        for rec in self.records.values() {
            if let Some(body) = self.bodies.get_mut(rec.body) {
                body.wake_up(true);
            }
        }
    }

    fn set_ground(&mut self, ground: f32) {
        self.ground = ground;
        if let Some(body) = self.bodies.get_mut(self.ground_body) {
            body.set_translation(Vector::new(0.0, ground, 0.0), true);
        }
    }

    fn set_ccd(&mut self, handle: BodyHandle, on: bool) -> bool {
        let Some(rec) = self.records.get_mut(&handle) else {
            return false;
        };
        // 静态 / 运动学刚体由脚本驱动，不存在「自己高速飞过去」的情况，改它没意义
        if !matches!(rec.kind, BodyKind::Dynamic) {
            return false;
        }
        rec.continuous = on;
        if let Some(body) = self.bodies.get_mut(rec.body) {
            // `enable_ccd` 只升级「扫掠运动学 / 动态体」；
            // 对固定碰撞体的自动 CCD（rapier 自带）不受它控制，属预期行为
            body.enable_ccd(on);
        }
        true
    }

    fn ccd_of(&self, handle: BodyHandle) -> Option<bool> {
        let rec = self.records.get(&handle)?;
        if !matches!(rec.kind, BodyKind::Dynamic) {
            return Some(false);
        }
        Some(
            self.bodies
                .get(rec.body)
                .map(|b| b.is_ccd_enabled())
                .unwrap_or(false),
        )
    }

    fn set_ccd_default(&mut self, on: bool) {
        self.ccd_default = on;
    }

    fn ccd_default(&self) -> bool {
        self.ccd_default
    }

    fn step(&mut self, dt: f32) {
        let total = dt.clamp(0.0, 0.1);
        if total <= 0.0 {
            return;
        }
        // 与内置后端同一策略：按 1/60 切子步（最多 6 个），子步时长均分，不吃掉时间。
        let steps = ((total * 60.0).ceil() as usize).clamp(1, MAX_SUBSTEPS);
        let h = total / steps as f32;
        for _ in 0..steps {
            self.substep(h);
        }
    }

    fn pull_from_scene(&mut self, scene: &Scene) {
        for rec in self.records.values() {
            let Some(node) = scene.get(rec.node) else {
                continue;
            };
            let node_pos = node.transform.position;
            let node_rot = node.transform.rotation;
            let Some(body) = self.bodies.get_mut(rec.body) else {
                continue;
            };
            match rec.kind {
                // 静态体不跟随节点：脚本挪静态物件是「改摆放」，重建刚体才生效
                BodyKind::Static => {}
                // 运动学体由脚本驱动：每帧把节点位姿推给 Rapier，它负责带动碰撞
                BodyKind::Kinematic => {
                    body.set_translation(to_rapier(node_pos), true);
                    body.set_rotation(quat_to_rapier(node_rot), true);
                }
                // 动态体通常由物理主导；只有节点被脚本明显挪动过才视为瞬移
                BodyKind::Dynamic => {
                    if (to_glam(body.translation()) - node_pos).length() > TELEPORT_EPS {
                        body.set_translation(to_rapier(node_pos), true);
                        body.set_linvel(Vector::new(0.0, 0.0, 0.0), true);
                        body.set_angvel(Vector::new(0.0, 0.0, 0.0), true);
                    }
                }
            }
        }
    }

    fn sync_to_scene(&mut self, scene: &mut Scene) {
        let mut dead = Vec::new();
        for (handle, rec) in self.records.iter() {
            let (pos, rot) = match self.bodies.get(rec.body) {
                Some(body) => (to_glam(body.translation()), quat_to_glam(body.rotation())),
                None => {
                    dead.push(*handle);
                    continue;
                }
            };
            match scene.get_mut(rec.node) {
                Some(node) => {
                    node.transform.position = pos;
                    node.transform.rotation = rot;
                    node.body = Some(handle.0);
                }
                // 节点已被脚本 remove_node：顺手回收刚体，避免残留
                None => dead.push(*handle),
            }
        }
        for handle in dead {
            self.destroy_body(handle);
        }
    }

    fn body_count(&self) -> usize {
        self.records.len()
    }

    fn has_body(&self, handle: BodyHandle) -> bool {
        // 以 records 为准而不是 bodies：地面刚体在 RigidBodySet 里但没有 record，
        // 它不该被当成「某节点已绑定」的证据。
        self.records.contains_key(&handle)
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.records.keys().copied().collect())
    }

    fn drain_events(&mut self) -> Vec<PhysicsEvent> {
        match self.events.lock() {
            Ok(mut sink) => std::mem::take(&mut *sink),
            Err(_) => Vec::new(),
        }
    }

    /// 用 parry 的窄相做射线检测：逐碰撞体 cast，取最近命中。
    ///
    /// 地面半空间不属于任何场景节点（`records` 里没有它），因此查询只覆盖刚体；
    /// 这与内置后端「查刚体、不查地面」的口径一致。
    fn raycast(&self, ray: Ray, max_distance: f32) -> Option<RayHit> {
        if max_distance <= 0.0 {
            return None;
        }
        let parry_ray = ParryRay::new(to_rapier(ray.origin), to_rapier(ray.dir));
        let mut best: Option<RayHit> = None;
        for (handle, rec) in self.records.iter() {
            let Some(collider) = self.colliders.get(rec.collider) else {
                continue;
            };
            let Some(hit) = collider.shape().cast_ray_and_get_normal(
                collider.position(),
                &parry_ray,
                max_distance,
                true,
            ) else {
                continue;
            };
            let distance = hit.time_of_impact;
            if best.map_or(true, |b| distance < b.distance) {
                best = Some(RayHit {
                    handle: *handle,
                    node: rec.node,
                    distance,
                    point: ray.point_at(distance),
                    normal: to_glam(hit.normal).normalize_or_zero(),
                });
            }
        }
        best
    }

    /// 用 parry 的 `intersection_test` 做球 vs 碰撞体相交，结果按 node 升序。
    fn overlap_sphere(&self, center: Vec3, radius: f32) -> Vec<OverlapHit> {
        if radius <= 0.0 {
            return Vec::new();
        }
        let ball = Ball::new(radius);
        let pose = Pose3::translation(center.x, center.y, center.z);
        let mut hits: Vec<OverlapHit> = self
            .records
            .iter()
            .filter(|(_, rec)| {
                let Some(collider) = self.colliders.get(rec.collider) else {
                    return false;
                };
                intersection_test(&pose, &ball, collider.position(), collider.shape())
                    .map(|r| r.intersecting)
                    .unwrap_or(false)
            })
            .map(|(handle, rec)| OverlapHit {
                handle: *handle,
                node: rec.node,
            })
            .collect();
        hits.sort_by_key(|h| h.node);
        hits
    }

    /// 用 parry 的 `cast_shapes` 做真实窄相扫掠（球 / 盒 vs 任意碰撞体），取最近接触。
    ///
    /// TOI 归一化技巧：速度取 `dir * max_distance` 且 `max_time_of_impact = 1.0`，
    /// 于是 `time_of_impact ∈ [0,1]`，乘回 `max_distance` 就是世界距离，
    /// 与 `raycast` 的 `distance` 同口径（上层不需要知道这是 TOI）。
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
        let d = dir.normalize_or_zero();
        if d == Vec3::ZERO {
            return None;
        }

        // parry 的查询形状要活到循环结束，这里用局部变量托住所有权
        let ball;
        let cuboid;
        let probe: &dyn Shape = match shape {
            CastShape::Sphere { radius } => {
                ball = Ball::new(radius.abs());
                &ball
            }
            CastShape::Box { half } => {
                let h = half.abs();
                cuboid = Cuboid::new(to_rapier(h));
                &cuboid
            }
        };

        let pose = Pose3::translation(origin.x, origin.y, origin.z);
        let vel = to_rapier(d * max_distance);
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            // 允许 t=0 命中也上报：起始重叠对脚本是有效信息（`started_inside`）
            stop_at_penetration: true,
            // 穿透时也要给出可用法线，否则 over 内物体拿不到推出方向
            compute_impact_geometry_on_penetration: true,
        };

        let mut best: Option<ShapeCastHit> = None;
        for (handle, rec) in self.records.iter() {
            let Some(collider) = self.colliders.get(rec.collider) else {
                continue;
            };
            let Ok(Some(hit)) = cast_shapes(
                &pose,
                vel,
                probe,
                collider.position(),
                Vector::new(0.0, 0.0, 0.0),
                collider.shape(),
                options,
            ) else {
                continue;
            };
            let started_inside =
                matches!(hit.status, ShapeCastStatus::PenetratingOrWithinTargetDist);
            let distance = (hit.time_of_impact * max_distance).max(0.0);
            let normal = if started_inside {
                // 初始重叠：parry 给的是「最小平移方向」，与推进方向无关（球心在盒心时甚至可能朝上）。
                // 契约规定这种情况下法线统一取 `-dir`，与内置后端完全一致，脚本只认一条规则。
                d * -1.0
            } else {
                let g = to_glam(hit.normal2).normalize_or_zero();
                if g.length_squared() > 0.5 {
                    g
                } else {
                    // 数值退化（面重合 / 深度穿透）时退回「朝向来的方向」
                    d * -1.0
                }
            };
            let cand = ShapeCastHit {
                handle: *handle,
                node: rec.node,
                distance: if started_inside { 0.0 } else { distance },
                point: origin + d * if started_inside { 0.0 } else { distance },
                normal,
                started_inside,
            };
            if cast_candidate_better(&cand, best.as_ref()) {
                best = Some(cand);
            }
        }
        best
    }

    /// 胶囊重叠：parry 胶囊 vs 任意碰撞体的窄相相交测试（精确，不是 AABB 近似）。
    ///
    /// 与内置后端的差别记在这里：内置是包围盒近似（宁多报不漏报），
    /// 这里是真实窄相，贴边擦过不会误报。两者都按 `node` 升序返回。
    fn overlap_capsule(&self, capsule: Capsule) -> Vec<PortHit> {
        if capsule.is_degenerate() {
            return Vec::new();
        }
        let probe = ParryCapsule::new_y(capsule.half_height.abs(), capsule.radius.abs());
        let pose = Pose3::translation(capsule.center.x, capsule.center.y, capsule.center.z);
        let mut hits: Vec<PortHit> = self
            .records
            .iter()
            .filter(|(_, rec)| {
                let Some(collider) = self.colliders.get(rec.collider) else {
                    return false;
                };
                intersection_test(&pose, &probe, collider.position(), collider.shape())
                    .map(|r| r.intersecting)
                    .unwrap_or(false)
            })
            .map(|(handle, rec)| PortHit {
                body: handle.0,
                node: rec.node,
                distance: 0.0,
                point: capsule.center,
                normal: Vec3::ZERO,
                started_inside: true,
            })
            .collect();
        hits.sort_by_key(|h| h.node);
        hits
    }

    /// 胶囊扫掠：parry `cast_shapes` 真窄相，TOI 归一化技巧同 `shape_cast`。
    fn sweep_capsule(&self, capsule: Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        if capsule.is_degenerate() || max_distance <= 0.0 {
            return None;
        }
        let d = dir.normalize_or_zero();
        if d == Vec3::ZERO {
            return None;
        }
        let probe = ParryCapsule::new_y(capsule.half_height.abs(), capsule.radius.abs());
        let pose = Pose3::translation(capsule.center.x, capsule.center.y, capsule.center.z);
        let vel = to_rapier(d * max_distance);
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        };
        let mut best: Option<PortHit> = None;
        for (handle, rec) in self.records.iter() {
            let Some(collider) = self.colliders.get(rec.collider) else {
                continue;
            };
            let Ok(Some(hit)) = cast_shapes(
                &pose,
                vel,
                &probe,
                collider.position(),
                Vector::new(0.0, 0.0, 0.0),
                collider.shape(),
                options,
            ) else {
                continue;
            };
            let started_inside =
                matches!(hit.status, ShapeCastStatus::PenetratingOrWithinTargetDist);
            let distance = if started_inside {
                0.0
            } else {
                (hit.time_of_impact * max_distance).max(0.0)
            };
            // 起点就重叠时**也要如实给出接触法线**：贴地站立时胶囊底面与地面
            // 零间隙，parry 的预测距离会把「站在地上」报成 penetrating。若这里
            // 一律把法线伪造成 `-dir`，那脚下这块支撑面就会被上层当成墙 ——
            // 角色一落地就再也迈不开腿（B03 在 Rapier 上抓到的缺陷：抬升永远
            // 不发生、水平移动第 5 帧起全 blocked）。只有拿不到几何时才退回 -dir。
            let g = to_glam(hit.normal2).normalize_or_zero();
            let normal = if g.length_squared() > 0.5 {
                g
            } else {
                d * -1.0
            };
            let cand = PortHit {
                body: handle.0,
                node: rec.node,
                distance,
                point: capsule.center + d * distance,
                normal,
                started_inside,
            };
            if port_hit_better(&cand, best.as_ref()) {
                best = Some(cand);
            }
        }
        best
    }

    /// 地表采样：从 `(x, 高处, z)` 朝 -Y 打一条射线，取最近命中的顶面；
    /// 一个刚体都没命中就退回后端自带的地面平面。
    ///
    /// 地面半空间不参与（它不在 `records` 里），永远以 `ground` 字段兜底，
    /// 因此「站在空地上」返回的是 `body = None` 的平地样本，与内置后端一致。
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let mut top = self.ground + 1.0;
        for rec in self.records.values() {
            if let Some(collider) = self.colliders.get(rec.collider) {
                top = top.max(collider.compute_aabb().maxs.y);
            }
        }
        let origin = Vec3::new(x, top + 1.0, z);
        let mut best = GroundSample::flat(self.ground);
        for (handle, rec) in self.records.iter() {
            let Some(collider) = self.colliders.get(rec.collider) else {
                continue;
            };
            // 沿 -Y 逐段下探，**只认站得住的面**。
            //
            // 第一个命中的未必是地面：斜置箱体的端面、高台的侧壁都会先挡住射线。
            // M2-B05 在「斜坡 → 平台」接缝上实测到的就是坡体端面 —— 法线
            // `(0.966, 0.259, 0)`，`n.y` 只有 0.259，上层按坡度判 `TooSteep` 后
            // 整帧原地冻结、还不报 `blocked`（角色卡在 x=5.5 一动不动）。把障碍
            // 的侧面当地表，会把能走的路判成走不了。所以命中的面太陡就继续往下
            // 打，取下面第一个立得住的面；一路打不到就退回家自带的地面平面。
            let mut from_y = origin.y;
            for _ in 0..GROUND_PROBE_STEPS {
                let probe_origin = Vec3::new(x, from_y, z);
                let parry_ray = ParryRay::new(to_rapier(probe_origin), to_rapier(Vec3::NEG_Y));
                let max_distance = (probe_origin.y - self.ground) + 1.0;
                // `solid = false`：下探起点就贴着上一层表面，不能一出发就自撞。
                let Some(hit) = collider.shape().cast_ray_and_get_normal(
                    collider.position(),
                    &parry_ray,
                    max_distance,
                    false,
                ) else {
                    break;
                };
                let height = probe_origin.y - hit.time_of_impact;
                let normal = to_glam(hit.normal).normalize_or_zero();
                if normal.y >= GROUND_STEEP_MIN {
                    if height > best.height {
                        best = GroundSample {
                            height,
                            normal,
                            body: Some(handle.0),
                            node: Some(rec.node),
                        };
                    }
                    break;
                }
                // 陡面：从它下方一点点接着探，别被同一个面反复挡住。
                from_y = height - 1e-3;
            }
        }
        Some(best)
    }
}

/// 端口命中的择优规则：与 `cast_candidate_better` 同一套口径，只是字段不同。
fn port_hit_better(cand: &PortHit, best: Option<&PortHit>) -> bool {
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
