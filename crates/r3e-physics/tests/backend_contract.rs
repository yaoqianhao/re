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
    assert!(
        caps.raycast && caps.overlap && caps.shape_cast,
        "内置后端用解析法查 AABB，必须申报 raycast/overlap/shape_cast"
    );

    #[cfg(feature = "rapier")]
    {
        let rapier = physics_of(PhysicsKind::Rapier);
        let caps = rapier.capabilities();
        assert_eq!(caps, BackendCaps::CONTACT);
        assert!(caps.body_body_contacts && caps.rotational_dynamics && caps.collision_events);
        assert!(
            caps.raycast && caps.overlap && caps.shape_cast,
            "Rapier 走 parry 查询管线"
        );
        assert!(caps.has("events") && !caps.has("ccd") && !caps.has("joints"));
        assert_eq!(rapier.kind(), PhysicsKind::Rapier);
        assert_eq!(rapier.backend_name(), "rapier");
    }
}

/// 查询层契约：每个可用后端都必须能「打中最近的盒子、打不中空处、球体能圈到物体」。
///
/// 两个静态盒子沿 Z 轴排在 z=0 与 z=-4 处，射线从 z=-10 朝 +Z 打过去，
/// 最近的应该是近处的那个（z=-4 盒子最近的面在 z=-5）。
#[test]
fn every_backend_raycast_and_overlap_find_nearest_body() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        let mut scene = Scene::new();
        let mut spawn = |name: &str, z: f32| {
            let id = scene
                .spawn(ObjectDesc {
                    name: name.into(),
                    mesh: Some("box".into()),
                    position: Some([0.0, 2.0, z]),
                    ..Default::default()
                })
                .expect("spawn");
            physics
                .try_create_body(
                    id,
                    BodyDesc {
                        kind: r3e_physics::BodyKind::Static,
                        shape: ColliderShape::Box,
                        half_extents: Vec3::splat(1.0),
                        position: Vec3::new(0.0, 2.0, z),
                        ..Default::default()
                    },
                )
                .expect("static body");
            id
        };
        spawn("near", -4.0);
        let far = spawn("far", 0.0);
        scene.update_world_transforms();

        let ray = r3e_physics::Ray::new(Vec3::new(0.0, 2.0, -10.0), Vec3::Z);
        let hit = physics
            .raycast(ray, 100.0)
            .unwrap_or_else(|| panic!("{kind:?}: 射线应命中盒子"));
        assert!(
            (hit.distance - 5.0).abs() < 0.05,
            "{kind:?}: 应命中 z=-5 的面，距离 5，实际 {}",
            hit.distance
        );
        assert!(
            (hit.point.z + 5.0).abs() < 0.05,
            "{kind:?}: 命中点应在 z≈-5，实际 {}",
            hit.point.z
        );
        // 法线朝向来向：-Z 方向
        assert!(
            hit.normal.z < -0.9,
            "{kind:?}: 迎面法线应为 -Z，实际 {:?}",
            hit.normal
        );
        assert_ne!(hit.node, far, "{kind:?}: 不该跳过近处盒子");

        // 背向打空：朝 -Z 只有空气
        let miss = r3e_physics::Ray::new(Vec3::new(0.0, 2.0, -10.0), -Vec3::Z);
        assert!(physics.raycast(miss, 100.0).is_none(), "{kind:?}: 背向应打空");
        // 距离不够也打不中
        assert!(
            physics.raycast(ray, 2.0).is_none(),
            "{kind:?}: max_distance 应生效"
        );

        // 球体圈中近处盒子，圈不到远处
        let near_hits = physics.overlap_sphere(Vec3::new(0.0, 2.0, -4.0), 1.2);
        assert!(
            !near_hits.is_empty(),
            "{kind:?}: 球心在盒内，重叠查询不应为空"
        );
        assert!(
            physics.overlap_sphere(Vec3::new(0.0, 2.0, -8.0), 0.5).is_empty(),
            "{kind:?}: 两盒之间的空隙不该有重叠"
        );

        // 关掉物理后查询立即失效，不残留旧结果
        physics.set_enabled(false);
        assert!(physics.raycast(ray, 100.0).is_none(), "{kind:?}: 关闭后不查");
        assert!(
            physics.overlap_sphere(Vec3::new(0.0, 2.0, -4.0), 1.2).is_empty(),
            "{kind:?}: 关闭后不查"
        );
    }
}

/// 申报口径守卫：任何可用后端都不得申报门面还没暴露的能力位。
///
/// 关节与 CCD 在 `PhysicsBackend` 契约里既没有方法、脚本也没有入口，
/// 一旦有后端报 `joints`/`ccd` = true，脚本 `physics_has_capability` 就会拿到
/// 一个用不了的承诺（这是本次修复的原始 bug）。
#[test]
fn no_backend_overclaims_capabilities() {
    for kind in PhysicsKind::ALL.iter().copied().filter(|k| k.is_available()) {
        let caps = physics_of(kind).capabilities();
        assert!(
            caps.is_within_exposed(),
            "{kind:?}: 申报了门面未暴露的能力位（joints/ccd），属于虚报"
        );
        assert!(
            !caps.has("joints") && !caps.has("ccd"),
            "{kind:?}: 门面没有关节/CCD 接口前不得申报这两项"
        );
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

/// 查询层契约（shape-cast）：扫掠查询必须与 raycast 同口径——
/// 撞到最近的表面、起点重叠单独上报、背向 / 距离不够打空、关掉物理立即失效。
///
/// 场景与 raycast 用例同构：两个静态盒子在 z=0 与 z=-4（半尺寸 1），
/// 球心从 z=-10 沿 +Z 推过去。近处盒子的正面在 z=-5，
/// 所以半径 0.5 的球推 4.5 就碰上，命中瞬间球心在 z≈-5.5。
#[test]
fn every_backend_shape_cast_reports_nearest_surface() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        let mut scene = Scene::new();
        let mut spawn = |name: &str, z: f32| {
            let id = scene
                .spawn(ObjectDesc {
                    name: name.into(),
                    mesh: Some("box".into()),
                    position: Some([0.0, 2.0, z]),
                    ..Default::default()
                })
                .expect("spawn");
            physics
                .try_create_body(
                    id,
                    BodyDesc {
                        kind: r3e_physics::BodyKind::Static,
                        shape: ColliderShape::Box,
                        half_extents: Vec3::splat(1.0),
                        position: Vec3::new(0.0, 2.0, z),
                        ..Default::default()
                    },
                )
                .expect("static body");
            id
        };
        spawn("near", -4.0);
        let far = spawn("far", 0.0);
        scene.update_world_transforms();

        let start = Vec3::new(0.0, 2.0, -10.0);

        // 球扫掠：半径 0.5，推 4.5 碰到 z=-5 的面
        let hit = physics
            .shape_cast_sphere(start, 0.5, Vec3::Z, 100.0)
            .unwrap_or_else(|| panic!("{kind:?}: 球扫掠应命中盒子"));
        assert!(
            (hit.distance - 4.5).abs() < 0.05,
            "{kind:?}: 球扫掠距离应为 4.5，实际 {}",
            hit.distance
        );
        assert!(
            (hit.point.z + 5.5).abs() < 0.05,
            "{kind:?}: 命中时球心应在 z≈-5.5，实际 {}",
            hit.point.z
        );
        assert!(
            hit.normal.z < -0.9,
            "{kind:?}: 迎面法线应为 -Z，实际 {:?}",
            hit.normal
        );
        assert!(!hit.started_inside, "{kind:?}: 起点在空气里，不该报初始重叠");
        assert_ne!(hit.node, far, "{kind:?}: 不该跳过近处盒子");

        // 盒扫掠：半尺寸 0.5 与球半径同义，距离也应是 4.5
        let box_hit = physics
            .shape_cast_box(start, Vec3::splat(0.5), Vec3::Z, 100.0)
            .unwrap_or_else(|| panic!("{kind:?}: 盒扫掠应命中盒子"));
        assert!(
            (box_hit.distance - 4.5).abs() < 0.05,
            "{kind:?}: 盒扫掠距离应为 4.5，实际 {}",
            box_hit.distance
        );
        assert!(
            box_hit.normal.z < -0.9,
            "{kind:?}: 盒扫掠法线应为 -Z，实际 {:?}",
            box_hit.normal
        );

        // 起点就在目标体内：distance=0 + started_inside + 法线朝来向
        let inside = physics
            .shape_cast_sphere(Vec3::new(0.0, 2.0, -4.0), 0.5, Vec3::Z, 100.0)
            .unwrap_or_else(|| panic!("{kind:?}: 起点重叠应上报而不是打空"));
        assert_eq!(inside.distance, 0.0, "{kind:?}: 初始重叠距离应为 0");
        assert!(inside.started_inside, "{kind:?}: 应标记初始重叠");
        assert!(
            inside.normal.z < -0.9,
            "{kind:?}: 初始重叠法线应朝来向 -Z，实际 {:?}",
            inside.normal
        );

        // 背向只有空气
        assert!(
            physics
                .shape_cast_sphere(start, 0.5, -Vec3::Z, 100.0)
                .is_none(),
            "{kind:?}: 背向应打空"
        );
        // 距离不够推不到
        assert!(
            physics.shape_cast_sphere(start, 0.5, Vec3::Z, 2.0).is_none(),
            "{kind:?}: max_distance 应生效"
        );
        // 退化形状查不出有意义结果
        assert!(
            physics.shape_cast_sphere(start, 0.0, Vec3::Z, 100.0).is_none(),
            "{kind:?}: 零半径应短路"
        );

        // 关掉物理后查询立即失效
        physics.set_enabled(false);
        assert!(
            physics.shape_cast_sphere(start, 0.5, Vec3::Z, 100.0).is_none(),
            "{kind:?}: 关闭后不查"
        );
    }
}
