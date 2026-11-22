//! Rapier 后端的胶水层：类型翻译、碰撞体构造、碰撞事件回填。
//!
//! 从 `rapier_backend.rs` 拆出（该文件已超 800 行门禁）：这里只有「与 rapier 打交道
//! 的纯函数与回调」，不持有任何后端状态。**`rapier3d` / `parry` / `nalgebra` 的类型
//! 只允许出现在本文件与 `rapier_backend.rs`**，门面、`app`、脚本层仍只碰
//! `glam::Vec3` / `Quat`。
//!
//! 拆开的另一层好处：`collider_for` 与事件回调的语义（尤其「地面不是场景节点，
//! 事件里必须报 `None`」这条）可以脱离一整个 `RapierBackend` 单独读。

use std::sync::{Arc, Mutex};

use glam::{Quat, Vec3};
use r3e_core::NodeId;
use rapier3d::prelude::*;

use crate::{BodyDesc, BodyKind, ColliderShape, PhysicsEvent, PhysicsEventKind};

/// 地面半空间的 `user_data` 标记：它不是场景节点，事件里应报 `None`。
pub(crate) const GROUND_USER_DATA: u128 = u128::MAX;

pub(crate) fn to_rapier(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

pub(crate) fn to_glam(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

pub(crate) fn quat_to_rapier(q: Quat) -> Rotation {
    Rotation::from_xyzw(q.x, q.y, q.z, q.w)
}

pub(crate) fn quat_to_glam(q: &Rotation) -> Quat {
    Quat::from_xyzw(q.x, q.y, q.z, q.w)
}

/// 把 `ColliderShape` + 半尺寸 + 材质翻译成 Rapier 碰撞体。
pub(crate) fn collider_for(desc: &BodyDesc) -> ColliderBuilder {
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
pub(crate) struct EventCollector {
    pub(crate) sink: Arc<Mutex<Vec<PhysicsEvent>>>,
}

/// 碰撞体 → 所属节点（地面标记为 `None`）。
fn node_of(
    bodies: &RigidBodySet,
    colliders: &ColliderSet,
    handle: ColliderHandle,
) -> Option<NodeId> {
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
