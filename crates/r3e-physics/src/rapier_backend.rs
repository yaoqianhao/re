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
//! * 支持 CCD（目前默认关闭，留给需要时按物体打开）
//!
//! 语义对齐点（与内置后端保持一致，避免换后端后脚本行为跳变）：
//! * `BodyKind::Static` 完全不动，`Kinematic` 每帧从场景节点推位置，`Dynamic` 由物理积分
//! * 脚本直接改动态刚体节点的位置 → `pull_from_scene` 识别为「瞬移」并清零速度（不保留旧动量）
//! * 节点被移除 → `sync_to_scene` 回收对应刚体，无句柄泄漏

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::{Quat, Vec3};
use rapier3d::na::Unit;
use rapier3d::prelude::*;
use r3e_core::{NodeId, Scene};

use crate::{
    BackendCaps, BodyDesc, BodyHandle, BodyKind, ColliderShape, PhysicsBackend, PhysicsEvent,
    PhysicsEventKind,
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
        BackendCaps::FULL
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
        self.colliders
            .insert_with_parent(collider_for(&desc), rb, &mut self.bodies);

        self.records.insert(
            handle,
            Record {
                body: rb,
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
}
