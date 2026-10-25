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
//! 都已在求解器里接好），但 `PhysicsBackend` 契约里还没有打开 CCD 的开关，也没有
//! 建关节的方法，脚本更没有任何入口——所以 `joints` / `ccd` 两位一律报 `false`。
//! 等门面补齐接口后再把 `capabilities()` 提升到 `CONTACT` 之外的档位。
//!
//! 语义对齐点（与内置后端保持一致，避免换后端后脚本行为跳变）：
//! * `BodyKind::Static` 完全不动，`Kinematic` 每帧从场景节点推位置，`Dynamic` 由物理积分
//! * 脚本直接改动态刚体节点的位置 → `pull_from_scene` 识别为「瞬移」并清零速度（不保留旧动量）
//! * 节点被移除 → `sync_to_scene` 回收对应刚体，无句柄泄漏

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::{Quat, Vec3};
use rapier3d::na::Unit;
use rapier3d::parry::query::{
    cast_shapes, intersection_test, Ray as ParryRay, ShapeCastOptions, ShapeCastStatus,
};
use rapier3d::parry::shape::{Ball, Cuboid};
use rapier3d::prelude::*;
use r3e_core::{NodeId, Scene};

use crate::{
    cast_candidate_better, BackendCaps, BodyDesc, BodyHandle, BodyKind, CastShape, ColliderShape,
    OverlapHit, PhysicsBackend, PhysicsEvent, PhysicsEventKind, Ray, RayHit, ShapeCastHit,
};

/// 地面半空间的 `user_data` 标记：它不是场景节点，事件里应报 `None`。
const GROUND_USER_DATA: u128 = u128::MAX;

/// 判定「脚本挪了节点」的位置阈值（米）。低于它认为节点没被动过。
const TELEPORT_EPS: f32 = 1e-4;

/// 单帧最多切几个子步，与内置后端保持同一节奏（避免大 dt 一帧穿透）。
const MAX_SUBSTEPS: usize = 6;

fn to_rapier(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn to_glam(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn quat_to_rapier(q: Quat) -> Rotation {
    Rotation::from_xyzw(q.x, q.y, q.z, q.w)
}

fn quat_to_glam(q: &Rotation) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}

/// 把 `ColliderShape` + 半尺寸 + 材质翻译成 Rapier 碰撞体。
fn collider_for(desc: &BodyDesc) -> ColliderBuilder {
    let h = desc.half_extents.abs().max(Vec3::splat(1e-3));
    let builder = match desc.shape {
        ColliderShape::Box => ColliderBuilder::cuboid(h.x, h.y, h.z),
        ColliderShape::Sphere => ColliderBuilder::ball(h.x),
        // 胶囊：内置后端把 h.y 当「半径 + 圆柱半高」，换算成 rapier 的 (圆柱半高, 半径)
        ColliderShape::Capsule => ColliderBuilder::capsule_y((h.y - h.x).max(0.0), h.x),
        ColliderShape::Cylinder => ColliderBuilder::cylinder(h.y, h.x),
    };
    let builder = builder
        .friction(desc.friction.clamp(0.0, 2.0))
        .restitution(desc.restitution.clamp(0.0, 1.0))
        .active_events(ActiveEvents::COLLISION_EVENTS);
    if matches!(desc.kind, BodyKind::Dynamic) && desc.mass > 1e-4 {
        builder.mass(desc.mass)
    } else {
        builder
    }
}

/// Rapier 事件回调：把 `ColliderHandle` 对翻译回 `NodeId` 对。
struct EventCollector {
    sink: Arc<Mutex<Vec<PhysicsEvent>>>,
}

/// 碰撞体 → 所属节点（地面标记为 `None`）。
fn node_of(bodies: &RigidBodySet, colliders: &ColliderSet, handle: ColliderHandle) -> Option<NodeId> {
    let parent = colliders.get(handle)?.parent()?;
    let data = bodies.get(parent)?.user_data;
    if data == GROUND_USER_DATA {
        None
    } else {
        Some(data as NodeId)
    }
}

impl EventHandler for EventCollector {
    fn handle_collision_event(
        &self,
        bodies: &RigidBodySet,
        colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        let (kind, a, b) = match event {
            CollisionEvent::Started(a, b, _) => (PhysicsEventKind::Started, a, b),
            CollisionEvent::Stopped(a, b, _) => (PhysicsEventKind::Stopped, a, b),
        };
        let ev = PhysicsEvent {
            kind,
            a: node_of(bodies, colliders, a),
            b: node_of(bodies, colliders, b),
        };
        if let Ok(mut sink) = self.sink.lock() {
            sink.push(ev);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: Real,
    ) {
        // 力事件目前不暴露给脚本；需要时在这里转成 PhysicsEvent 的新 kind。
    }

    fn handle_soft_body_tear_event(&self, _bodies: &SoftBodySet, _event: &SoftBodyTearEvent) {}
}

/// 一条刚体记录：句柄 ↔ Rapier 实体 ↔ 场景节点。
struct Record {
    body: RigidBodyHandle,
    /// 挂在刚体上的碰撞体；查询（raycast / overlap）按它做窄相检测。
    collider: ColliderHandle,
    node: NodeId,
    kind: BodyKind,
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
        // 只报门面已暴露的三位；joints / ccd 见文件头「能力申报口径」。
        BackendCaps::CONTACT
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

        let rb = self.bodies.insert(builder);
        if let Some(body) = self.bodies.get_mut(rb) {
            body.set_rotation(quat_to_rapier(desc.rotation), false);
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
            let started_inside = matches!(hit.status, ShapeCastStatus::PenetratingOrWithinTargetDist);
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
}
