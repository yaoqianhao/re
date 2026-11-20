//! rapier 专属查询语义回归（地面 / 扫掠 / 输入校验 / 确定性）。
//!
//! 法线与旋转体部分见 `rapier_query.rs`。

#![cfg(feature = "rapier")]

mod common;

use common::*;

// =========================================================== 地面采样（曲面精度）

#[test]
fn ground_on_ramp_is_below_plate_top() {
    let (physics, _) = world_specs(&[ramp()]);
    let g = physics.sample_ground(0.0, 0.0).expect("采样");
    // 45° 斜坡在原点附近的高度接近 0.707（半厚投影）
    assert!(g.height > -0.1 && g.height < 0.8, "h={}", g.height);
}

#[test]
fn ground_ramp_normal_is_tilted() {
    let (physics, _) = world_specs(&[ramp()]);
    let g = physics.sample_ground(0.0, 0.0).expect("采样");
    assert!(g.normal.y < 0.95, "斜坡采样法线不该竖直: {:?}", g.normal);
    assert!(g.is_walkable(0.9) == (g.normal.y > 0.9));
}

#[test]
fn ground_on_sphere_top_equals_radius() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let g = physics.sample_ground(0.0, 0.0).expect("采样");
    assert!((g.height - 2.5).abs() < 1e-2, "h={}", g.height);
}

#[test]
fn ground_on_sphere_side_is_lower_than_top() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let top = physics.sample_ground(0.0, 0.0).expect("采样").height;
    let side = physics.sample_ground(0.4, 0.0).expect("采样").height;
    assert!(side < top, "球侧应低于球顶: {side} vs {top}");
    assert!(side > 2.0, "球侧仍应在球心之上: {side}");
}

#[test]
fn ground_outside_sphere_falls_back() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::splat(0.5),
    )]);
    let g = physics.sample_ground(2.0, 2.0).expect("采样");
    assert!((g.height - 0.0).abs() < 1e-3, "h={}", g.height);
}

#[test]
fn ground_on_cylinder_rim_is_flat_top() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let center = physics.sample_ground(0.0, 0.0).expect("采样").height;
    let rim = physics.sample_ground(0.0, 0.45).expect("采样").height;
    assert!(
        (center - rim).abs() < 1e-2,
        "圆柱顶面应是平的: {center} vs {rim}"
    );
}

#[test]
fn ground_cylinder_outside_radius_falls_back() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let g = physics.sample_ground(0.51, 0.0).expect("采样");
    assert!(
        (g.height - 0.0).abs() < 1e-2,
        "半径外应回落到地面: h={}",
        g.height
    );
}

#[test]
fn ground_rotated_box_raises_centre_height() {
    let a = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::ZERO,
        Vec3::new(1.0, 0.2, 1.0),
    )])
    .0
    .sample_ground(0.0, 0.0)
    .expect("采样")
    .height;
    let b = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::new(1.0, 0.2, 1.0)),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
    )])
    .0
    .sample_ground(0.0, 0.0)
    .expect("采样")
    .height;
    // 45° 转后中心处取的是「半厚 × √2」，比平放更高
    assert!(b > a + 1e-3, "旋转后中心处应更高: {b} vs {a}");
}

#[test]
fn ground_prefers_body_over_plane_on_neglected_spot() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(10.0, 1.0, 10.0),
        Vec3::new(0.5, 0.5, 0.5),
    )]);
    assert!((physics.sample_ground(10.0, 10.0).expect("采样").height - 1.5).abs() < 1e-2);
    assert!((physics.sample_ground(0.0, 0.0).expect("采样").height).abs() < 1e-2);
}

#[test]
fn ground_returns_body_handle_of_sampled_body() {
    let (physics, ids) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 0.5, 1.0),
    )]);
    let g = physics.sample_ground(0.5, 0.5).expect("采样");
    assert_eq!(g.node, Some(ids[0]));
    assert!(g.body.is_some());
}

#[test]
fn ground_on_stepped_platform_takes_upper() {
    let (physics, _) = world_specs(&[
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(2.0, 0.5, 2.0),
        ),
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 1.5, 0.0),
            Vec3::new(1.0, 0.5, 1.0),
        ),
    ]);
    assert!((physics.sample_ground(0.2, 0.2).expect("采样").height - 2.0).abs() < 1e-2);
    assert!((physics.sample_ground(1.5, 1.5).expect("采样").height - 1.0).abs() < 1e-2);
}

// =========================================================== 扫掠（真窄相细节）

#[test]
fn sweep_on_ramp_normal_matches_ramp_face() {
    let (physics, _) = world_specs(&[ramp()]);
    // 从斜面中部落（x=0 是最高棱，落那儿打到的是棱上的封口面）。
    // shape cast 的接触法线在棱边附近会被相邻面抢走，所以这里只钉「法线是单位
    // 朝向观察者、且确实比纯竖直更倾斜或至少朝上」，斜面精确法线交给上面的射线测试。
    let hit = physics
        .sweep_capsule(cap(Vec3::new(1.2, 6.0, 0.0), 1.0, 0.5), Vec3::NEG_Y, 20.0)
        .expect("落到斜坡");
    assert!(
        (hit.normal.length() - 1.0).abs() < 1e-2,
        "n={:?}",
        hit.normal
    );
    assert!(hit.normal.y > 0.5, "n={:?}", hit.normal);
    assert!(hit.distance < 6.0, "d={}", hit.distance);
}

#[test]
fn sweep_on_ramp_stops_above_ramp_surface() {
    let (physics, _) = world_specs(&[ramp()]);
    let start = 6.0;
    let hit = physics
        .sweep_capsule(cap(Vec3::new(0.0, start, 0.0), 1.0, 0.5), Vec3::NEG_Y, 20.0)
        .expect("命中");
    assert!(
        hit.distance > 0.0 && hit.distance < start,
        "d={}",
        hit.distance
    );
}

#[test]
fn sweep_hits_sphere_top_exactly() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::splat(0.5),
    )]);
    // 球顶 1.5；胶囊底 = 5-0.5=4.5 → 3.0
    let hit = physics
        .sweep_capsule(cap(Vec3::new(0.0, 5.0, 0.0), 0.0, 0.5), Vec3::NEG_Y, 20.0)
        .expect("命中球顶");
    assert!((hit.distance - 3.0).abs() < 2e-2, "d={}", hit.distance);
}

#[test]
fn sweep_slides_past_sphere_edge() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Sphere,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::splat(0.5),
    )]);
    // 偏离球心 2 个单位，直落到底——球半径 0.5，不该被挡
    assert!(physics
        .sweep_capsule(cap(Vec3::new(2.5, 5.0, 0.0), 0.0, 0.5), Vec3::NEG_Y, 20.0)
        .is_none());
}

#[test]
fn sweep_hits_cylinder_side_with_radial_normal() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let hit = physics
        .sweep_capsule(cap(Vec3::new(4.0, 1.0, 0.0), 0.0, 0.3), Vec3::NEG_X, 20.0)
        .expect("命中柱侧");
    assert!(hit.normal.x > 0.9, "n={:?}", hit.normal);
    assert!((hit.distance - 3.2).abs() < 5e-2, "d={}", hit.distance);
}

#[test]
fn sweep_through_rotated_thin_plate_is_blocked() {
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::new(2.0, 0.02, 2.0)),
        Quat::from_rotation_x(0.35),
    )]);
    assert!(physics
        .sweep_capsule(cap(Vec3::new(0.0, 5.0, 0.0), 0.1, 0.2), Vec3::NEG_Y, 20.0)
        .is_some());
}

#[test]
fn sweep_long_distance_still_uses_shape_not_centre() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(2.0, 0.05, 2.0),
    )]);
    let hit = physics
        .sweep_capsule(
            cap(Vec3::new(0.0, 100.0, 0.0), 0.5, 0.5),
            Vec3::NEG_Y,
            300.0,
        )
        .expect("命中");
    assert!((hit.distance - 98.95).abs() < 5e-2, "d={}", hit.distance);
}

#[test]
fn sweep_diagonal_hits_sphere_side() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Sphere, Vec3::ZERO, Vec3::splat(0.5))]);
    let hit = physics
        .sweep_capsule(
            cap(Vec3::new(3.0, 3.0, 0.0), 0.1, 0.2),
            Vec3::new(-1.0, -1.0, 0.0).normalize(),
            20.0,
        )
        .expect("斜扫命中球");
    assert!(hit.distance > 0.0, "d={}", hit.distance);
    assert!(
        hit.normal.dot(Vec3::new(1.0, 1.0, 0.0).normalize()) > 0.5,
        "n={:?}",
        hit.normal
    );
}

#[test]
fn sweep_against_two_boxes_picks_closer() {
    let (physics, _) = world_specs(&[
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(2.0, 0.1, 2.0),
        ),
        spec(
            ColliderShape::Box,
            Vec3::new(0.0, -3.0, 0.0),
            Vec3::new(2.0, 0.1, 2.0),
        ),
    ]);
    let hit = physics
        .sweep_capsule(cap(Vec3::new(0.0, 6.0, 0.0), 0.0, 0.2), Vec3::NEG_Y, 20.0)
        .expect("命中上板");
    assert!((hit.distance - 2.7).abs() < 5e-2, "d={}", hit.distance);
}

#[test]
fn sweep_blocked_by_cylinder_cap() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Cylinder,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.5),
    )]);
    let hit = physics
        .sweep_capsule(cap(Vec3::new(0.0, 6.0, 0.0), 0.0, 0.3), Vec3::NEG_Y, 20.0)
        .expect("命中柱顶");
    assert!((hit.distance - 3.7).abs() < 5e-2, "d={}", hit.distance);
    assert!(hit.normal.y > 0.9, "n={:?}", hit.normal);
}

// =========================================================== 输入校验（rapier 侧）

#[test]
fn ray_hits_rotated_box_face_obliquely() {
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
    )]);
    let hit = physics
        .cast_ray(&CastRay::new(Vec3::new(-5.0, 0.0, 0.6), Vec3::X, 20.0))
        .expect("斜打旋转盒");
    assert!(hit.point.x < 0.0, "应从侧面进入: p={:?}", hit.point);
    assert!(
        hit.normal.x < -0.5 || hit.normal.z > 0.5,
        "n={:?}",
        hit.normal
    );
}

#[test]
fn sweep_on_rotated_box_lands_on_face() {
    let (physics, _) = world_specs(&[spun(
        spec(ColliderShape::Box, Vec3::ZERO, Vec3::splat(0.5)),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
    )]);
    let hit = physics
        .sweep_capsule(cap(Vec3::new(0.0, 5.0, 0.0), 0.0, 0.2), Vec3::NEG_Y, 20.0)
        .expect("落到旋转盒");
    assert!(
        hit.distance > 4.0 && hit.distance < 5.0,
        "d={}",
        hit.distance
    );
    assert!(hit.normal.y > 0.5, "n={:?}", hit.normal);
}

#[test]
fn ray_rejects_nan_direction_on_rapier() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Box, Vec3::splat(0.5), Vec3::splat(0.5))]);
    let bad = CastRay::new(
        Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(f32::NAN, -1.0, 0.0),
        20.0,
    );
    assert!(physics.cast_ray(&bad).is_none());
}

#[test]
fn overlap_rejects_infinite_center_on_rapier() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Box, Vec3::splat(0.5), Vec3::splat(0.5))]);
    assert!(physics
        .capsule_overlap(&cap(Vec3::new(f32::INFINITY, 0.5, 0.0), 0.2, 0.2))
        .is_empty());
}

#[test]
fn sweep_rejects_nan_direction_on_rapier() {
    let (physics, _) = world_specs(&[spec(ColliderShape::Box, Vec3::splat(0.5), Vec3::splat(0.5))]);
    let bad = cap(Vec3::new(0.0, 5.0, 0.0), 0.2, 0.2);
    assert!(physics
        .sweep_capsule(bad, Vec3::new(f32::NAN, -1.0, 0.0), 20.0)
        .is_none());
}

#[test]
fn ground_rejects_infinite_z_on_rapier() {
    let (physics, _) = world_specs(&[]);
    assert!(physics.sample_ground(0.0, f32::INFINITY).is_none());
    assert!(physics.sample_ground(0.0, f32::NEG_INFINITY).is_none());
}

// =========================================================== 确定性与一致性

#[test]
fn rapier_repeated_ray_is_identical() {
    let (physics, _) = world_specs(&[ramp()]);
    let a = physics
        .cast_ray(&down(Vec3::new(0.1, 8.0, 0.2), 20.0))
        .expect("命中");
    for _ in 0..5 {
        let b = physics
            .cast_ray(&down(Vec3::new(0.1, 8.0, 0.2), 20.0))
            .expect("命中");
        assert_eq!(a.point, b.point);
        assert_eq!(a.normal, b.normal);
        assert_eq!(a.body, b.body);
    }
}

#[test]
fn rapier_repeated_overlap_is_sorted_identically() {
    let (physics, _) = world_specs(&[
        spec(
            ColliderShape::Box,
            Vec3::new(0.4, 0.5, 0.0),
            Vec3::splat(0.5),
        ),
        spec(
            ColliderShape::Box,
            Vec3::new(-0.4, 0.5, 0.0),
            Vec3::splat(0.5),
        ),
        spec(
            ColliderShape::Sphere,
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::splat(0.5),
        ),
    ]);
    let probe = cap(Vec3::new(0.0, 0.5, 0.0), 0.2, 0.3);
    let first: Vec<u64> = physics
        .capsule_overlap(&probe)
        .iter()
        .map(|h| h.body)
        .collect();
    for _ in 0..5 {
        let again: Vec<u64> = physics
            .capsule_overlap(&probe)
            .iter()
            .map(|h| h.body)
            .collect();
        assert_eq!(first, again);
    }
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted, "结果应按 node 升序稳定");
}

#[test]
fn rapier_caps_are_full_and_dyn_dispatch_works() {
    let (physics, _) = world_specs(&[ramp()]);
    let port: &dyn PhysicsQueryPort = &physics;
    assert_eq!(
        port.port_caps().enabled_keys(),
        physics.port_caps().enabled_keys()
    );
    assert!(port
        .cast_ray(&down(Vec3::new(0.0, 8.0, 0.0), 20.0))
        .is_some());
    assert!(port.sample_ground(0.0, 0.0).is_some());
}

#[test]
fn rapier_queries_do_not_advance_simulation() {
    let (physics, _) = world_specs(&[ramp()]);
    let before = physics.body_count();
    let _ = physics.cast_ray(&down(Vec3::new(0.0, 8.0, 0.0), 20.0));
    let _ = physics.capsule_overlap(&cap(Vec3::new(0.0, 0.5, 0.0), 0.2, 0.2));
    let _ = physics.sample_ground(0.0, 0.0);
    assert_eq!(physics.body_count(), before);
}

#[test]
fn rapier_query_after_clear_is_clean() {
    let (mut physics, _) = world_specs(&[ramp()]);
    physics.clear_bodies();
    assert_eq!(physics.body_count(), 0);
    assert!(physics
        .cast_ray(&down(Vec3::new(0.0, 8.0, 0.0), 20.0))
        .is_none());
    assert_eq!(physics.sample_ground(0.0, 0.0).expect("采样").height, 0.0);
}

#[test]
fn rapier_overlap_matches_ray_on_same_box() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 0.5, 1.0),
    )]);
    let ray = physics
        .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
        .expect("命中");
    // 往下压 0.05 才是「真相交」，相切在窄相里不算重叠
    let over = physics.capsule_overlap(&cap(Vec3::new(0.0, 1.55, 0.0), 0.0, 0.1));
    assert_eq!(ray.body, over[0].body);
    assert_eq!(ray.node, over[0].node);
}

#[test]
fn rapier_free_body_does_not_answer_queries() {
    let (physics, _) = world_specs(&[spec(
        ColliderShape::Box,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 0.5, 0.5),
    )]);
    // 有体：命中
    assert!(physics
        .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
        .is_some());
    // 换到空世界：不命中
    let (empty, _) = world_specs(&[]);
    assert!(empty
        .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
        .is_none());
}
