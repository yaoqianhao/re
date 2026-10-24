//! 后端契约回归测试：同一批断言跑遍「当前构建可用的每个后端」。
//!
//! 加新后端（Jolt / PhysX …）时只需把它塞进 [`selectable`]，契约不满足立刻红灯。
//! Rapier 只在 `--features rapier` 时参与；默认构建下本文件验证内置后端的契约。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene};
use r3e_physics::{BackendCaps, BodyDesc, ColliderShape, Physics, PhysicsConfig, PhysicsKind};

/// 当前构建真正能选中的后端（`rapier` 未编译时会被 `is_available` 挡掉）。
fn selectable() -> Vec<PhysicsKind> {
    PhysicsKind::ALL
        .iter()
        .copied()
        .filter(|k| *k != PhysicsKind::Null && k.is_available())
        .collect()
}

fn physics_of(kind: PhysicsKind) -> Physics {
    let mut physics = Physics::new();
    physics
        .apply_config(
            PhysicsConfig::default()
                .with_enabled(true)
                .with_kind(kind),
        )
        .expect("后端应可选中");
    physics
}

fn scene_with_ball() -> (Scene, r3e_core::NodeId) {
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

fn ball_desc(y: f32) -> BodyDesc {
    BodyDesc {
        shape: ColliderShape::Sphere,
        half_extents: Vec3::splat(0.5),
        position: Vec3::new(0.0, y, 0.0),
        restitution: 0.4,
        friction: 0.6,
        ..Default::default()
    }
}

#[test]
fn rapier_slot_matches_build_feature() {
    // 未开 feature 时必须诚实报不可用，而不是「编译得过但跑起来静默降级」
    assert_eq!(PhysicsKind::Rapier.is_available(), cfg!(feature = "rapier"));
    let names = PhysicsKind::available_names();
    assert!(names.contains(&"builtin"));
    if cfg!(feature = "rapier") {
        assert!(names.contains(&"rapier"));
    } else {
        assert!(!names.contains(&"rapier"));
    }
    // 未编译时选择 rapier 必须报错；编译了则必须成功
    let mut physics = Physics::new();
    let switched = physics.set_kind(PhysicsKind::Rapier);
    assert_eq!(switched.is_ok(), cfg!(feature = "rapier"), "{switched:?}");
}

#[test]
fn every_backend_drops_and_rests_on_ground() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        assert!(physics.enabled(), "{kind:?} 应处于开启状态");
        let (mut scene, id) = scene_with_ball();
        physics
            .try_create_body(id, ball_desc(5.0))
            .expect("物理开启时应能建刚体");
        assert_eq!(physics.body_count(), 1, "{kind:?}");

        for _ in 0..600 {
            physics.step(1.0 / 60.0, &mut scene);
        }
        let y = scene.get(id).expect("node").transform.position.y;
        // 球半径 0.5、地面 0 → 静止在 0.5 附近（rapier 允许极小穿透）
        assert!(
            (y - 0.5).abs() < 0.05,
            "{kind:?}: 期望 y≈0.5，实际 {y}"
        );
    }
}

#[test]
fn every_backend_destroys_bodies_and_clears_on_switch() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        let (mut scene, id) = scene_with_ball();
        let h = physics.try_create_body(id, ball_desc(5.0)).expect("body");
        physics.step(1.0 / 60.0, &mut scene);
        physics.destroy_body(h);
        assert_eq!(physics.body_count(), 0, "{kind:?} 销毁后应为 0");
        // 重复销毁必须幂等（后端契约要求）
        physics.destroy_body(h);
        assert_eq!(physics.body_count(), 0, "{kind:?} 重复销毁不应崩");

        // 切到别的后端会清空刚体并 bump generation（app 层据此重绑）
        let before = physics.generation();
        let other = if kind == PhysicsKind::Builtin {
            PhysicsKind::Null
        } else {
            PhysicsKind::Builtin
        };
        let _ = physics.try_create_body(id, ball_desc(3.0));
        physics.set_kind(other).expect("switch");
        assert_eq!(physics.body_count(), 0, "{kind:?} -> {other:?} 应清空");
        assert!(physics.generation() > before, "{kind:?} 应 bump generation");
    }
}

#[test]
fn every_backend_reclaims_body_when_node_removed() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        let (mut scene, id) = scene_with_ball();
        physics.try_create_body(id, ball_desc(5.0)).expect("body");
        physics.step(1.0 / 60.0, &mut scene);
        assert_eq!(physics.body_count(), 1);

        scene.despawn_id(id);
        physics.step(1.0 / 60.0, &mut scene);
        assert_eq!(
            physics.body_count(),
            0,
            "{kind:?}: 节点移除后应回收刚体，避免句柄泄漏"
        );
    }
}

#[test]
fn capability_matrix_is_honest() {
    let mut null = Physics::new();
    assert_eq!(null.capabilities(), BackendCaps::NONE);
    assert_eq!(null.capabilities().summary(), "none");
    assert!(null.drain_events().is_empty());

    let builtin = physics_of(PhysicsKind::Builtin);
    let caps = builtin.capabilities();
    assert!(!caps.body_body_contacts, "内置后端不互撞");
    assert!(!caps.rotational_dynamics, "内置后端不旋转");
    assert!(!caps.collision_events, "内置后端不产事件");

    #[cfg(feature = "rapier")]
    {
        let rapier = physics_of(PhysicsKind::Rapier);
        let caps = rapier.capabilities();
        assert!(caps.body_body_contacts && caps.rotational_dynamics && caps.collision_events);
        assert!(caps.has("events") && caps.has("ccd") && caps.has("joints"));
        assert_eq!(rapier.kind(), PhysicsKind::Rapier);
        assert_eq!(rapier.backend_name(), "rapier");
    }
}

/// Rapier 专属：物件互撞 + 碰撞事件通道真的通。
/// 内置后端只有「刚体 vs 地面」一条规则，这里让球落在刚体平台上验证的是
/// 「换后端带来的能力升级」，而不是再测一遍重力。
#[cfg(feature = "rapier")]
#[test]
fn rapier_stacks_bodies_and_reports_collision_events() {
    let mut physics = physics_of(PhysicsKind::Rapier);
    let mut scene = Scene::new();
    let platform = scene
        .spawn(ObjectDesc {
            name: "platform".into(),
            mesh: Some("cube".into()),
            position: Some([0.0, 1.0, 0.0]),
            scale: Some([4.0, 0.5, 4.0]),
            ..Default::default()
        })
        .expect("spawn platform");
    let ball = scene
        .spawn(ObjectDesc {
            name: "ball".into(),
            mesh: Some("sphere".into()),
            position: Some([0.0, 4.0, 0.0]),
            scale: Some([1.0, 1.0, 1.0]),
            ..Default::default()
        })
        .expect("spawn ball");
    scene.update_world_transforms();

    physics
        .try_create_body(
            platform,
            BodyDesc {
                kind: r3e_physics::BodyKind::Static,
                shape: ColliderShape::Box,
                half_extents: Vec3::new(2.0, 0.25, 2.0),
                position: Vec3::new(0.0, 1.0, 0.0),
                ..Default::default()
            },
        )
        .expect("platform body");
    physics
        .try_create_body(ball, ball_desc(4.0))
        .expect("ball body");

    let mut started = 0usize;
    for _ in 0..240 {
        physics.step(1.0 / 60.0, &mut scene);
        started += physics
            .drain_events()
            .iter()
            .filter(|e| e.kind == r3e_physics::PhysicsEventKind::Started)
            .count();
    }

    assert!(started > 0, "rapier 应上报 collision started 事件");
    let y = scene.get(ball).expect("ball node").transform.position.y;
    // 平台顶面 1.25 + 球半径 0.5 → 停在 ~1.75；内置后端会穿到 0.5，故这条断言专属于互撞能力
    assert!(
        (y - 1.75).abs() < 0.12,
        "球应停在平台顶面 y≈1.75（互撞生效），实际 {y}"
    );
}
