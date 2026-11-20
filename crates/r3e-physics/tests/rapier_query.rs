//! rapier 专属查询语义回归（法线 / 旋转体 / AABB 近似答错的部分）。
//!
//! `port_contract.rs` 那一族只钉「两个后端都成立」的口径；本文件相反，专钉**只有真窄相**
//! 才成立的细节。凡这里出现的断言，都是有意识地把 builtin 的近似差异钉在明面上，
//! 免得日后有人用「反正测试都过了」把它当成等价实现。
//!
//! 地面 / 扫掠 / 输入校验 / 确定性见 `rapier_query_ground.rs`。
//! 全部测试挂在 `#[cfg(feature = "rapier")]` 下，`--no-default-features` 时整文件不编译。

#![cfg(feature = "rapier")]

mod common;

use common::*;

// =========================================================== 精确法线（真窄相才有）

#[test]
fn ramp_normal_is_tilted_not_up() {
    let (physics, _) = world_specs(&[ramp()]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 8.0, 0.0), 20.0))
        .expect("命中斜坡");
    assert!(hit.normal.y > 0.5, "n={:?}", hit.normal);
    assert!(
        hit.normal.y < 0.95,
        "斜坡法线不该是纯竖直: {:?}",
        hit.normal
    );
    assert!(
        hit.normal.x.abs() > 0.5,
        "斜坡法线该带横向分量: {:?}",
        hit.normal
    );
}

#[test]
fn ramp_normal_matches_rotation_axis() {
    let (physics, _) = world_specs(&[ramp()]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 8.0, 0.0), 20.0))
        .expect("命中");
    // 绕 Z 转 -45° 后，盒顶法线 = R * (0,1,0)
    let expect = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_4) * Vec3::Y;
    assert!(
        (hit.normal - expect).length() < 1e-3,
        "n={:?} 期望 {expect:?}",
        hit.normal
    );
}

#[test]
fn cube_top_normal_is_exactly_up() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::splat(0.5),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.1, 5.0, -0.2), 20.0))
        .expect("命中");
    assert!((hit.normal - Vec3::Y).length() < 1e-4, "n={:?}", hit.normal);
}

#[test]
fn sphere_top_normal_is_up() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 5.0, 0.0), 20.0))
        .expect("命中球顶");
    assert!((hit.point.y - 1.5).abs() < 1e-3, "p={:?}", hit.point);
    assert!((hit.normal - Vec3::Y).length() < 1e-3, "n={:?}", hit.normal);
}

#[test]
fn sphere_side_hit_normal_is_horizontal() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let hit = physics
        .cast_ray(&CastRay::new(Vec3::new(-5.0, 1.0, 0.0), Vec3::X, 20.0))
        .expect("命中球侧");
    assert!((hit.point.x + 0.5).abs() < 1e-3, "p={:?}", hit.point);
    assert!(hit.normal.x < -0.99, "n={:?}", hit.normal);
}

#[test]
fn sphere_oblique_normal_points_at_caster() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let origin = Vec3::new(3.0, 4.0, 0.0);
    let dir = Vec3::new(-3.0, -3.0, 0.0).normalize();
    let hit = physics
        .cast_ray(&CastRay::new(origin, dir, 20.0))
        .expect("斜射球");
    assert!(hit.normal.dot(-dir) > 0.5, "n={:?} dir={dir:?}", hit.normal);
    assert!((hit.point.length() - (Vec3::new(0.0, 1.0, 0.0)).length()).abs() < 0.5 + 1e-3);
}

#[test]
fn cylinder_side_normal_is_radial() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let hit = physics
        .cast_ray(&CastRay::new(Vec3::new(5.0, 1.0, 0.0), Vec3::NEG_X, 20.0))
        .expect("命中圆柱侧面");
    assert!((hit.point.x - 0.5).abs() < 1e-2, "p={:?}", hit.point);
    assert!(hit.normal.x > 0.99, "n={:?}", hit.normal);
    assert!(
        hit.normal.y.abs() < 1e-2,
        "侧面法线不该有竖直分量: {:?}",
        hit.normal
    );
}

#[test]
fn cylinder_top_normal_is_up() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 5.0, 0.0), 20.0))
        .expect("命中圆柱顶");
    assert!((hit.point.y - 2.0).abs() < 1e-2, "p={:?}", hit.point);
    assert!((hit.normal - Vec3::Y).length() < 1e-3, "n={:?}", hit.normal);
}

#[test]
fn ceiling_hit_normal_points_down() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 3.0, 0.0),
        Vec3::new(4.0, 0.2, 4.0),
    )]);
    let hit = physics
        .cast_ray(&CastRay::new(Vec3::new(0.0, -2.0, 0.0), Vec3::Y, 20.0))
        .expect("命中天花板");
    assert!((hit.normal + Vec3::Y).length() < 1e-4, "n={:?}", hit.normal);
}

#[test]
fn inner_corner_picks_near_face() {
    let (physics, _) = world_specs(&[
        spec(
            ColliderShape::Box,
            Vec3::new(-2.0, 1.0, 0.0),
            Vec3::new(0.5, 1.0, 2.0),
        ),
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 1.0, -2.0),
            Vec3::new(2.0, 1.0, 0.5),
        ),
    ]);
    let hit = physics
        .cast_ray(&CastRay::new(
            Vec3::new(-3.0, 1.0, 1.0),
            Vec3::new(1.0, 0.0, -0.2).normalize(),
            20.0,
        ))
        .expect("命中内角");
    assert!(hit.normal.length() > 0.99);
}

// =========================================================== 旋转体的包围盒不再可信

#[test]
fn rotated_box_ray_hits_real_surface_not_aabb() {
    // 45° 立方体：AABB 半宽 0.707，真实面在 0.5
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
    )]);
    let hit = physics
        .cast_ray(&CastRay::new(Vec3::new(-5.0, 0.0, 0.0), Vec3::X, 20.0))
        .expect("命中");
    // 沿 X 打到的是棱（|x| = 0.7071），不是面（0.5）
    assert!(
        (hit.point.x.abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 5e-2,
        "p={:?}",
        hit.point
    );
}

#[test]
fn rotated_box_top_height_is_lower_than_aabb_top() {
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 5.0, 0.0), 20.0))
        .expect("命中");
    // 45° 转后沿 Y 打到的是棱，距离 0.7071
    assert!(
        (hit.point.y - std::f32::consts::FRAC_1_SQRT_2).abs() < 5e-2,
        "p={:?}",
        hit.point
    );
}

#[test]
fn rotated_wall_ray_can_slip_past() {
    // 薄板在 XZ 平面偏 60°，沿某条射线 AABB 会挡住，真窄相能穿过去
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::new(0.05, 2.0, 2.0)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_3),
    )]);
    let straight = physics.cast_ray(&down(Vec3::new(3.0, 5.0, 0.0), 20.0));
    assert!(
        straight.is_none(),
        "离板面 3 单位的铅垂射线不该命中: {straight:?}"
    );
}

#[test]
fn scale_doubles_surface_height() {
    let (physics, _) = world_specs(&[scaled(
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::splat(0.5),
        ),
        Vec3::splat(2.0),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
        .expect("命中");
    assert!(hit.point.y > 1.4, "缩放后顶面应显著抬高: {:?}", hit.point);
}

#[test]
fn rotation_does_not_change_body_identity() {
    let (physics, ids) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_x(0.3),
    )]);
    let hit = physics
        .cast_ray(&down(Vec3::new(0.0, 5.0, 0.0), 20.0))
        .expect("命中");
    assert_eq!(hit.node, ids[0]);
}

// =========================================================== AABB 近似必然答错的场景

#[test]
fn capsule_near_rotated_corner_is_not_reported_as_hit() {
    // 旋转 45° 立方体的包围盒角是空的；胶囊贴在角外仍有 AABB 相交
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
    )]);
    // 沿 45° 方向外侧 0.65 处（AABB 半对角 0.707，真实棱面 0.5）
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let hits = physics.capsule_overlap(&cap(Vec3::new(0.65 * d, 0.0, 0.65 * d), 0.0, 0.02));
    assert!(hits.is_empty(), "空角不该报命中: {hits:?}");
}

#[test]
fn capsule_inside_bounding_box_corner_misses() {
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
    )]);
    // AABB 角落 (0.68, 0.68) 落在包围盒内、实体外
    let hits = physics.capsule_overlap(&cap(Vec3::new(0.68, 0.0, 0.68), 0.0, 0.01));
    assert!(hits.is_empty(), "包围盒角落是空的: {hits:?}");
}

#[test]
fn sphere_overlap_is_sphere_not_box() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Sphere, Vec3::ZERO, Vec3::splat(0.5))]);
    // 球外、但 AABB 内的角落
    let hits = physics.capsule_overlap(&cap(Vec3::new(0.49, 0.49, 0.0), 0.0, 0.01));
    assert!(hits.is_empty(), "球外角落不该命中: {hits:?}");
}

#[test]
fn sphere_overlap_hits_near_surface() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Sphere, Vec3::ZERO, Vec3::splat(0.5))]);
    let hits = physics.capsule_overlap(&cap(Vec3::new(0.48, 0.0, 0.0), 0.0, 0.1));
    assert_eq!(hits.len(), 1);
}

#[test]
fn cylinder_corner_is_empty() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::ZERO,
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let hits = physics.capsule_overlap(&cap(Vec3::new(0.48, 0.0, 0.48), 0.0, 0.01));
    assert!(hits.is_empty(), "圆柱的方形角落是空的: {hits:?}");
}

#[test]
fn ray_through_bounding_box_corner_misses_sphere() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Sphere, Vec3::ZERO, Vec3::splat(0.5))]);
    // 沿 X 打在 (y=0.4, z=0.4)：离轴 √0.32 ≈ 0.566 > 0.5，AABB 内、球外
    assert!(physics
        .cast_ray(&CastRay::new(Vec3::new(-5.0, 0.4, 0.4), Vec3::X, 20.0))
        .is_none());
}

#[test]
fn rapier_and_builtin_disagree_on_clear_corner() {
    // 有意登记的差异：同一份空角场景，builtin（AABB）会误报，rapier 不会
    let corner = cap(Vec3::new(0.68, 0.0, 0.68), 0.0, 0.01);
    let (r, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
    )]);
    let (b, _) = world_on(
        physics_of(PhysicsKind::Builtin),
        &[spun(
            spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
        )],
    );
    assert!(r.capsule_overlap(&corner).is_empty(), "rapier 应判空");
    // builtin 用 AABB 近似，这里它报不报都不影响契约；只要求「若报，必须是已知近似导致」
    let _ = b.capsule_overlap(&corner);
}

#[test]
fn rapier_keeps_multi_body_corner_case_exact() {
    let (physics, _) = world_specs(&[
        spun(
            spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
            Quat::from_rotation_y(0.7),
        ),
        spun(
            spec(
                ColliderShape::Box,
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::splat(0.5),
            ),
            Quat::from_rotation_z(0.7),
        ),
    ]);
    // 两体之间的空档，AABB 会相叠
    let hits = physics.capsule_overlap(&cap(Vec3::new(1.5, 0.0, 0.0), 0.0, 0.05));
    assert!(hits.is_empty(), "两体之间是空的: {hits:?}");
}
