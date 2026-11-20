//! 物理查询测试的共用夹具与再导出。
//!
//! `tests/` 下一级 `.rs` 各自是独立测试 crate，夹具放这里给它们共享；
//! 这里再导出（`pub use`）常用类型与 trait，各测试文件只需 `use common::*` 即可，
//! 免得每个文件重复写一长串 import。

#![allow(dead_code, unused_imports)]

pub use glam::{Quat, Vec3};
pub use r3e_core::{NodeId, ObjectDesc, Scene};
pub use r3e_physics::port::{
    Capsule, CapsulePort, CastRay, GroundPort, PhysicsQueryPort, PortCaps, RaycastPort,
};
pub use r3e_physics::{BodyDesc, ColliderShape, NullBackend, Physics, PhysicsConfig, PhysicsKind};

/// 当前构建真正能选中的后端（`rapier` 未编译时只剩 builtin）。
pub fn selectable() -> Vec<PhysicsKind> {
    PhysicsKind::ALL
        .iter()
        .copied()
        .filter(|k| *k != PhysicsKind::Null && k.is_available())
        .collect()
}

/// 期望的默认后端（S4 之后 = `preferred_default`）。
pub fn default_backend() -> PhysicsKind {
    PhysicsKind::preferred_default()
}

pub fn physics_of(kind: PhysicsKind) -> Physics {
    let mut physics = Physics::new();
    physics
        .apply_config(PhysicsConfig::default().with_enabled(true).with_kind(kind))
        .expect("后端应可选中");
    physics
}

/// 建一个装了 `specs`（形状、中心、半尺寸）的门面，返回刚体所属节点。
pub fn world_shapes(
    kind: PhysicsKind,
    specs: &[(ColliderShape, Vec3, Vec3)],
) -> (Physics, Vec<NodeId>) {
    let mut scene = Scene::new();
    let mut physics = physics_of(kind);
    let mut ids = Vec::new();
    for (i, (shape, center, half)) in specs.iter().enumerate() {
        let id = scene
            .spawn(ObjectDesc {
                name: format!("n{i}"),
                ..Default::default()
            })
            .expect("spawn");
        physics.create_body(
            id,
            BodyDesc {
                shape: *shape,
                half_extents: *half,
                position: *center,
                ..Default::default()
            },
        );
        ids.push(id);
    }
    (physics, ids)
}

pub fn empty(kind: PhysicsKind) -> Physics {
    world_shapes(kind, &[]).0
}

pub fn capsule(center: Vec3, half_height: f32, radius: f32) -> Capsule {
    Capsule::new(center, half_height, radius)
}

/// 地板：中心 y=-0.5、半高 0.5 → **顶面 y=0.0**，占地 20x20。
pub fn floor() -> (ColliderShape, Vec3, Vec3) {
    (
        ColliderShape::Box,
        Vec3::new(0.0, -0.5, 0.0),
        Vec3::new(10.0, 0.5, 10.0),
    )
}

/// 立方箱：中心 y=0.5、半尺寸 0.5 → 顶面 y=1.0。
pub fn cube() -> (ColliderShape, Vec3, Vec3) {
    (
        ColliderShape::Box,
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::splat(0.5),
    )
}

pub fn down(origin: Vec3, max: f32) -> CastRay {
    CastRay::new(origin, Vec3::NEG_Y, max)
}

// ---------------------------------------------------------------- 带旋转/缩放的夹具

pub struct Spec {
    pub shape: ColliderShape,
    pub center: Vec3,
    pub half: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

pub fn spec(shape: ColliderShape, center: Vec3, half: Vec3) -> Spec {
    Spec {
        shape,
        center,
        half,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

pub fn spun(mut s: Spec, rotation: Quat) -> Spec {
    s.rotation = rotation;
    s
}

pub fn scaled(mut s: Spec, scale: Vec3) -> Spec {
    s.scale = scale;
    s
}

pub fn world_on(mut physics: Physics, specs: &[Spec]) -> (Physics, Vec<NodeId>) {
    let mut scene = Scene::new();
    let mut ids = Vec::new();
    for (i, s) in specs.iter().enumerate() {
        let id = scene
            .spawn(ObjectDesc {
                name: format!("n{i}"),
                ..Default::default()
            })
            .expect("spawn");
        physics.create_body(
            id,
            BodyDesc {
                shape: s.shape,
                half_extents: s.half,
                position: s.center,
                rotation: s.rotation,
                scale: s.scale,
                ..Default::default()
            },
        );
        ids.push(id);
    }
    (physics, ids)
}

/// 默认（rapier）后端的世界。
pub fn world_specs(specs: &[Spec]) -> (Physics, Vec<NodeId>) {
    world_on(physics_of(PhysicsKind::Rapier), specs)
}

pub fn cap(center: Vec3, half_height: f32, radius: f32) -> Capsule {
    Capsule::new(center, half_height, radius)
}

/// 45° 斜坡：绕 Z 轴转 -45°，把盒顶转成斜面。
pub fn ramp() -> Spec {
    spun(
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 0.5, 3.0),
        ),
        Quat::from_rotation_z(-std::f32::consts::FRAC_PI_4),
    )
}
