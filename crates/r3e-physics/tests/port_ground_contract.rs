//! 端口 trait 契约回归（地面采样）：两套后端都必须成立的口径。
//!
//! 射线与接线见 `port_contract.rs`，胶囊重叠与扫掠见 `port_capsule_contract.rs`。

mod common;

use common::*;
// =========================================================== 地面采样（契约）

#[test]
fn ground_flat_on_empty_world() {
    for kind in selectable() {
        let g = empty(kind).sample_ground(0.0, 0.0).expect("恒有地面");
        assert_eq!(g.height, 0.0, "{kind:?}");
        assert_eq!(g.normal, Vec3::Y, "{kind:?}");
        assert_eq!(g.body, None, "{kind:?}");
        assert_eq!(g.node, None, "{kind:?}");
    }
}

#[test]
fn ground_follows_facade_level() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_ground(-3.0);
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height + 3.0).abs() < 1e-3, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_box_top_is_sampled() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height - 1.0).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_outside_footprint_falls_back_to_plane() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let g = physics.sample_ground(20.0, 20.0).expect("采样");
        assert!((g.height - 0.0).abs() < 1e-3, "{kind:?} h={}", g.height);
        assert_eq!(g.node, None, "{kind:?}");
    }
}

#[test]
fn ground_picks_highest_surface() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(3.0, 0.5, 3.0),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 2.0, 0.0),
                    Vec3::new(1.0, 0.5, 1.0),
                ),
            ],
        );
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height - 2.5).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_node_matches_highest_body() {
    for kind in selectable() {
        let (physics, ids) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(3.0, 0.5, 3.0),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 2.0, 0.0),
                    Vec3::new(1.0, 0.5, 1.0),
                ),
            ],
        );
        assert_eq!(
            physics.sample_ground(0.0, 0.0).expect("采样").node,
            Some(ids[1]),
            "{kind:?}"
        );
    }
}

#[test]
fn ground_normal_is_up_on_flat_top() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(
            physics.sample_ground(0.0, 0.0).expect("采样").normal.y > 0.9,
            "{kind:?}"
        );
    }
}

#[test]
fn ground_flat_is_walkable() {
    for kind in selectable() {
        let g = empty(kind).sample_ground(0.0, 0.0).expect("采样");
        assert!(g.is_walkable(0.5), "{kind:?}");
    }
}

#[test]
fn ground_rejects_nan_x() {
    for kind in selectable() {
        assert!(
            empty(kind).sample_ground(f32::NAN, 0.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn ground_rejects_nan_z() {
    for kind in selectable() {
        assert!(
            empty(kind).sample_ground(0.0, f32::NAN).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn ground_rejects_infinite_coords() {
    for kind in selectable() {
        assert!(
            empty(kind).sample_ground(f32::INFINITY, 0.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn ground_disabled_physics_is_none() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[cube()]);
        physics.set_enabled(false);
        assert!(physics.sample_ground(0.0, 0.0).is_none(), "{kind:?}");
    }
}

#[test]
fn ground_null_backend_is_none() {
    let physics = Physics::with_backend(Box::new(NullBackend::new()));
    assert!(physics.sample_ground(0.0, 0.0).is_none());
}

#[test]
fn ground_cylinder_top_is_sampled() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Cylinder,
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.5, 1.0, 0.5),
            )],
        );
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height - 2.0).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_sphere_top_is_sampled() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Sphere,
                Vec3::new(0.0, 2.0, 0.0),
                Vec3::splat(0.5),
            )],
        );
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height - 2.5).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_below_zero_level() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(0.0, -2.5, 0.0),
                Vec3::splat(0.5),
            )],
        );
        // 地平面压到箱底下，采样才会取到「负高度的箱顶」
        physics.set_ground(-5.0);
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height + 2.0).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_after_clear_bodies_is_flat() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[cube()]);
        assert!((physics.sample_ground(0.0, 0.0).expect("采样").height - 1.0).abs() < 1e-2);
        physics.clear_bodies();
        let g = physics.sample_ground(0.0, 0.0).expect("采样");
        assert!((g.height - 0.0).abs() < 1e-3, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_agrees_between_backends() {
    let mut hs: Vec<f32> = Vec::new();
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        hs.push(physics.sample_ground(0.0, 0.0).expect("采样").height);
    }
    assert!(
        hs.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-2),
        "两后端高度应一致: {hs:?}"
    );
}

#[test]
fn ground_body_is_some_when_on_body() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(
            physics
                .sample_ground(0.0, 0.0)
                .expect("采样")
                .body
                .is_some(),
            "{kind:?}"
        );
    }
}

#[test]
fn ground_inside_box_footprint_near_edge() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        // 盒半宽 0.5，取 0.45（明显在内部，避开「边沿算不算」的口径差）
        let g = physics.sample_ground(0.45, 0.45).expect("采样");
        assert!((g.height - 1.0).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_large_plate_keeps_height() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(50.0, 1.0, 50.0),
            )],
        );
        let g = physics.sample_ground(30.0, -30.0).expect("采样");
        assert!((g.height - 0.0).abs() < 1e-2, "{kind:?} h={}", g.height);
    }
}

#[test]
fn ground_repeated_calls_are_stable() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let a = physics.sample_ground(0.0, 0.0).expect("采样");
        let b = physics.sample_ground(0.0, 0.0).expect("采样");
        assert_eq!(a.height, b.height, "{kind:?}");
        assert_eq!(a.node, b.node, "{kind:?}");
        assert_eq!(a.body, b.body, "{kind:?}");
    }
}

#[test]
fn ground_does_not_mutate_body_count() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let before = physics.body_count();
        let _ = physics.sample_ground(0.0, 0.0);
        let _ = physics.cast_ray(&down(Vec3::new(0.0, 6.0, 0.0), 20.0));
        let _ = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3));
        assert_eq!(physics.body_count(), before, "{kind:?}");
    }
}

#[test]
fn ground_on_walkable_slope_still_returns_normal() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let g = physics.sample_ground(1.0, 1.0).expect("采样");
        assert!(g.normal.length() > 0.9, "{kind:?} n={:?}", g.normal);
        assert!(g.is_walkable(0.5), "{kind:?}");
    }
}
