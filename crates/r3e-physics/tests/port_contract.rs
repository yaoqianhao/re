//! 端口 trait 契约回归（射线组 / 接线组）：同一批断言跑遍「当前构建可用的每个后端」。
//!
//! 这里只写**两套后端都成立**的口径（明显相交 / 明显分离 / 明显距离），
//! 精确窄相与 AABB 近似必然不同的地方（擦角、斜面法线）留在 `rapier_query*.rs`，
//! 由 `#[cfg(feature = "rapier")]` 单独钉。
//!
//! 胶囊重叠与扫掠见 `port_capsule_contract.rs`，地面采样见 `port_ground_contract.rs`；
//! 共用夹具在 `common/mod.rs`。

mod common;

use common::*;
// =========================================================== 射线端口（契约）

#[test]
fn ray_hits_flat_box_top_on_every_backend() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics.cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0));
        assert!(hit.is_some(), "{kind:?}");
    }
}

#[test]
fn ray_distance_is_world_units() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        assert!(
            (hit.distance - 5.0).abs() < 1e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn ray_point_lies_on_scene_surface() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        assert!(
            (hit.point.y - 1.0).abs() < 1e-2,
            "{kind:?} p={:?}",
            hit.point
        );
        assert!(hit.point.x.abs() < 1e-3 && hit.point.z.abs() < 1e-3);
    }
}

#[test]
fn ray_normal_faces_the_caster() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        assert!(hit.normal.y > 0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn ray_misses_when_pointing_away() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 6.0, 0.0), Vec3::Y, 20.0))
            .is_none());
    }
}

#[test]
fn ray_misses_outside_footprint() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .cast_ray(&down(Vec3::new(4.0, 6.0, 0.0), 20.0))
            .is_none());
    }
}

#[test]
fn ray_respects_max_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(
            physics
                .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 4.0))
                .is_none(),
            "{kind:?}"
        );
        assert!(
            physics
                .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 6.0))
                .is_some(),
            "{kind:?}"
        );
    }
}

#[test]
fn ray_hits_exactly_at_max_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        // 距离正好 5.0，上限给 5.0 时必须算命中（闭区间）
        assert!(
            physics
                .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 5.0))
                .is_some(),
            "{kind:?}"
        );
    }
}

#[test]
fn ray_accepts_unnormalized_direction() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&CastRay::new(
                Vec3::new(0.0, 6.0, 0.0),
                Vec3::new(0.0, -4.0, 0.0),
                20.0,
            ))
            .expect("命中");
        assert!(
            (hit.distance - 5.0).abs() < 1e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn ray_rejects_zero_direction() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 6.0, 0.0), Vec3::ZERO, 20.0))
            .is_none());
    }
}

#[test]
fn ray_rejects_zero_max_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 0.0))
            .is_none());
    }
}

#[test]
fn ray_stops_at_first_of_two_stacked_plates() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(3.0, 0.1, 3.0),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, -1.0, 0.0),
                    Vec3::new(3.0, 0.1, 3.0),
                ),
            ],
        );
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        // 上板顶 1.1，射线从 6.0 起算 → 4.9
        assert!(
            (hit.distance - 4.9).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn ray_picks_nearest_of_two_boxes() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 5.0, 0.0),
                    Vec3::splat(0.5),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::splat(0.5),
                ),
            ],
        );
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
            .expect("命中");
        assert!(
            (hit.point.y - 5.5).abs() < 1e-2,
            "{kind:?} p={:?}",
            hit.point
        );
    }
}

#[test]
fn ray_reports_node_of_hit_body() {
    for kind in selectable() {
        let (physics, ids) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(8.0, 0.5, 0.0),
                    Vec3::splat(0.5),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::splat(0.5),
                ),
            ],
        );
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        assert_eq!(hit.node, ids[1], "{kind:?}");
    }
}

#[test]
fn ray_kerb_side_normal_points_back() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(-3.0, 0.5, 0.0),
                Vec3::new(0.2, 0.5, 1.0),
            )],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 0.6, 0.0), Vec3::NEG_X, 10.0))
            .expect("命中墙侧");
        assert!(hit.normal.x > 0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn ray_diagonal_still_hits_box_top() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&CastRay::new(
                Vec3::new(3.0, 4.0, 0.0),
                Vec3::new(-1.0, -1.0, 0.0).normalize(),
                20.0,
            ))
            .expect("斜射命中");
        assert!(hit.distance > 0.0 && hit.distance < 20.0, "{kind:?}");
    }
}

#[test]
fn ray_started_inside_box_is_flagged() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Y, 10.0))
            .expect("盒内起步");
        assert_eq!(hit.distance, 0.0, "{kind:?}");
        assert!(hit.started_inside, "{kind:?}");
    }
}

#[test]
fn ray_body_handle_is_stable_across_calls() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let a = physics
            .cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
            .expect("命中");
        let b = physics
            .cast_ray(&down(Vec3::new(0.2, 8.0, 0.2), 20.0))
            .expect("命中");
        assert_eq!(a.body, b.body, "{kind:?}");
    }
}

#[test]
fn ray_from_below_hits_underside() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, 20.0))
            .expect("命中盒底");
        assert!(
            (hit.distance - 5.0).abs() < 1e-2,
            "{kind:?} d={}",
            hit.distance
        );
        assert!(hit.normal.y < -0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn ray_on_empty_world_returns_none() {
    for kind in selectable() {
        let physics = empty(kind);
        assert!(
            physics
                .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 100.0))
                .is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn ray_long_range_still_hits() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 500.0, 0.0), 1000.0))
            .expect("命中");
        assert!(
            (hit.distance - 499.0).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn ray_points_through_null_backend_are_empty() {
    // 后端不实现 → 用 trait 默认实现（空），门面不替它猜
    let physics = Physics::with_backend(Box::new(NullBackend::new()));
    assert!(physics
        .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
        .is_none());
}

// =========================================================== 端口接线（契约）

#[test]
fn caps_full_on_default_backend() {
    let physics = physics_of(default_backend());
    assert_eq!(physics.port_caps(), PortCaps::FULL);
}

#[test]
fn caps_full_on_every_available_backend() {
    for kind in selectable() {
        assert_eq!(physics_of(kind).port_caps(), PortCaps::FULL, "{kind:?}");
    }
}

#[test]
fn port_stays_wired_across_backend_switch() {
    // 切后端会重建后端（刚体随之清空，这是既有语义），但端口接线不能丢：
    // 切完仍要能查，不能变成「已申报却不响应」
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics.sample_ground(0.0, 0.0).is_some());
        for other in selectable() {
            physics.set_kind(other).expect("可切");
        }
        physics.set_kind(kind).expect("可切回");
        assert_eq!(physics.port_caps(), PortCaps::FULL, "{kind:?}");
        assert!(physics.sample_ground(0.0, 0.0).is_some(), "{kind:?}");
        // 切回来后查询照样能问能答（切到同一后端时刚体仍在，故不断言必空）
        let _ = physics.cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0));
        assert!(!physics
            .capsule_overlap(&capsule(Vec3::new(999.0, 0.0, 999.0), 0.2, 0.2))
            .iter()
            .any(|h| h.point.x < 100.0));
    }
}

#[test]
fn dyn_port_dispatches_to_backend() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let port: &dyn PhysicsQueryPort = &physics;
        assert!(
            port.cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0))
                .is_some(),
            "{kind:?}"
        );
        assert!(port.sample_ground(0.0, 0.0).is_some(), "{kind:?}");
        // trait 版签名吃引用，门面版吃值 —— 两条路径都要能用
        let probe = capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3);
        assert_eq!(port.capsule_overlap(&probe).len(), 1, "{kind:?}");
        assert!(
            port.capsule_sweep(
                &capsule(Vec3::new(0.0, 4.0, 0.0), 0.4, 0.3),
                Vec3::NEG_Y,
                10.0
            )
            .is_some(),
            "{kind:?}"
        );
    }
}

#[test]
fn port_caps_keys_survive_backend_switch() {
    let (mut physics, _) = world_shapes(default_backend(), &[cube()]);
    let keys = physics.port_caps().enabled_keys();
    for other in selectable() {
        physics.set_kind(other).expect("可切");
        assert_eq!(physics.port_caps().enabled_keys(), keys, "{other:?}");
    }
}
