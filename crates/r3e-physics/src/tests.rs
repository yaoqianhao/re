//! `Physics` 门面的行为测试（M2-A05 自 `lib.rs` 内嵌测试块切出，逐条搬移、零删减）。

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
fn enabling_without_backend_upgrades_to_preferred_default() {
    let mut physics = Physics::new();
    assert_eq!(physics.kind(), PhysicsKind::Null);
    physics
        .apply_config(PhysicsConfig::default().with_enabled(true))
        .unwrap();
    // 升到「当前构建默认」：带 rapier 的构建升 rapier，无 rapier 构建升内置
    assert_eq!(physics.kind(), PhysicsKind::preferred_default());
    assert!(physics.enabled());
    // 申报的后端必须真的可用，不能升到一个没编译进来的 kind
    assert!(physics.kind().is_available());
    assert_eq!(physics.backend_name(), physics.kind().name());
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

/// 造一个与网格来源无关的「长方体」AIR 网格，仅由 8 个角点构成。
fn air_box(min: Vec3, max: Vec3) -> MeshData {
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let vertices: Vec<_> = corners
        .iter()
        .map(|p| r3e_core::Vertex::new([p.x, p.y, p.z], [0.0, 1.0, 0.0], [0.0, 0.0]))
        .collect();
    let indices = vec![
        0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6, 0, 4, 5, 0, 5, 1, 1, 5, 6, 1, 6, 2, 2, 6, 7, 2, 7, 3,
        3, 7, 4, 3, 4, 0,
    ];
    MeshData::new(vertices, indices)
}

#[test]
fn shape_inference_reads_geometry_not_source() {
    // 同一套判定：内置图元的几何 → 对应形状；换成外部资产导出的等价几何，结论一致。
    let sphere = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Sphere);
    let cylinder = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Cylinder);
    let cube = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Cube);

    assert_eq!(infer_shape(&sphere), ColliderShape::Sphere);
    assert_eq!(infer_shape(&cylinder), ColliderShape::Cylinder);
    assert_eq!(infer_shape(&cube), ColliderShape::Box);

    // 顶点位置整体平移后（等价于带偏移的导入模型）判定不变。
    let mut shifted = sphere.clone();
    for vertex in &mut shifted.vertices {
        vertex.position[0] += 12.5;
        vertex.position[2] -= 3.0;
    }
    assert_eq!(infer_shape(&shifted), ColliderShape::Sphere);
}

#[test]
fn infer_reads_mesh_shape() {
    let (scene, id) = scene_with_ball();
    let sphere = r3e_core::procedural::builtin_mesh(r3e_core::BuiltinMesh::Sphere);
    let desc = BodyDesc::infer_with(scene.get(id).unwrap(), Some(&sphere));
    assert_eq!(desc.shape, ColliderShape::Sphere);
    assert!((desc.half_extents.x - 0.5).abs() < 1e-3);
}

#[test]
fn infer_with_uses_mesh_aabb_instead_of_unit_cube() {
    // 资产网格真实尺寸远大于 1：碰撞体应跟着 AABB 走，而不是退化成 scale*0.5
    let (mut scene, id) = scene_with_ball();
    scene.get_mut(id).unwrap().transform.scale = Vec3::new(2.0, 1.0, 0.5);
    let mesh = air_box(Vec3::new(-1.0, 0.0, -0.5), Vec3::new(1.0, 2.0, 0.5));
    let desc = BodyDesc::infer_with(scene.get(id).unwrap(), Some(&mesh));
    assert_eq!(desc.shape, ColliderShape::Box);
    // AABB 半尺寸 (1,1,0.5) × scale (2,1,0.5) = (2,1,0.25)
    assert!(
        (desc.half_extents.x - 2.0).abs() < 1e-6,
        "x={}",
        desc.half_extents.x
    );
    assert!(
        (desc.half_extents.y - 1.0).abs() < 1e-6,
        "y={}",
        desc.half_extents.y
    );
    assert!(
        (desc.half_extents.z - 0.25).abs() < 1e-6,
        "z={}",
        desc.half_extents.z
    );
}

#[test]
fn infer_without_mesh_falls_back_to_scale_half() {
    let (scene, id) = scene_with_ball();
    let desc = BodyDesc::infer_with(scene.get(id).unwrap(), None);
    assert_eq!(desc.shape, ColliderShape::Box);
    assert!((desc.half_extents.x - 0.5).abs() < 1e-6);
}

#[test]
fn disabled_by_default_and_ignores_bodies() {
    let mut physics = Physics::new();
    assert!(!physics.enabled());
    let (mut scene, id) = scene_with_ball();
    assert!(physics
        .try_create_body(id, ball_desc(Vec3::Y * 5.0))
        .is_none());
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
            ccd: None,
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

#[test]
fn exposed_caps_allow_ccd_but_still_forbid_joints() {
    // 门面已暴露 CCD：这一档必须能申报，且关节仍是禁区（编译期就钉死）
    const _: () = assert!(BackendCaps::EXPOSED.ccd);
    const _: () = assert!(!BackendCaps::EXPOSED.joints);
    assert!(BackendCaps::CONTACT_CCD.is_within_exposed());
    assert!(BackendCaps::CONTACT.is_within_exposed());
    // 关节没入口，谁申报谁虚报
    assert!(!BackendCaps::FULL.is_within_exposed());
    // 档位是逐位叠加：CONTACT_CCD 比 CONTACT 只多 `ccd` 一位
    assert!(BackendCaps::CONTACT_CCD.has("ccd"));
    assert!(!BackendCaps::CONTACT.has("ccd"));
    assert!(BackendCaps::CONTACT_CCD.has("body_body"));
}

#[test]
fn builtin_backend_honestly_reports_no_ccd() {
    let mut physics = Physics::new();
    // 显式点内置后端：默认后端已是 rapier（D1=B），这条测试只对内置口径负责
    physics
        .apply_config(
            PhysicsConfig::default()
                .with_enabled(true)
                .with_kind(PhysicsKind::Builtin),
        )
        .unwrap();
    assert_eq!(physics.kind(), PhysicsKind::Builtin);
    assert!(!physics.capabilities().ccd);
    // 不支持就得如实回 false，别让脚本以为设置成功
    assert!(!physics.set_ccd_default(true));
    assert!(!physics.ccd_default() || !physics.capabilities().ccd);
    assert_eq!(physics.set_ccd_all(true), 0);
}

#[test]
fn scene_ccd_flows_into_body_desc_and_config() {
    use r3e_core::scene::{PhysicsDesc, PhysicsSceneDesc};

    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "fast".into(),
            mesh: Some("sphere".into()),
            position: Some([0.0, 1.0, 0.0]),
            physics: Some(PhysicsDesc {
                body: "dynamic".into(),
                mass: 1.0,
                restitution: None,
                friction: None,
                collider: None,
                half_extents: None,
                ccd: Some(false),
            }),
            ..Default::default()
        })
        .expect("spawn");
    scene.update_world_transforms();
    // 逐物件显式 false 必须原样传下去（不能和「没写」混为一谈）
    assert_eq!(
        BodyDesc::from_node(scene.get(id).unwrap())
            .unwrap()
            .continuous,
        Some(false)
    );

    // 场景级开关进 PhysicsConfig，再落到后端缺省值
    let cfg = PhysicsConfig::from_scene(&PhysicsSceneDesc {
        ccd: Some(false),
        ..Default::default()
    });
    assert_eq!(cfg.ccd, Some(false));
    let merged = PhysicsConfig::default().merge_over(&cfg);
    assert_eq!(merged.ccd, Some(false));
}
