//! 端口 trait 契约回归（胶囊重叠 / 扫掠）：两套后端都必须成立的口径。
//!
//! 射线与接线见 `port_contract.rs`，地面采样见 `port_ground_contract.rs`。

mod common;

use common::*;
// =========================================================== 胶囊重叠（契约）

#[test]
fn overlap_hits_enclosing_box() {
    for kind in selectable() {
        let (physics, ids) = world_shapes(kind, &[cube()]);
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3));
        assert_eq!(hits.len(), 1, "{kind:?}");
        assert_eq!(hits[0].node, ids[0], "{kind:?}");
    }
}

#[test]
fn overlap_empty_when_far_away() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(30.0, 0.9, 0.0), 0.5, 0.3))
            .is_empty());
    }
}

#[test]
fn overlap_distance_is_zero() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))[0];
        assert_eq!(hit.distance, 0.0, "{kind:?}");
    }
}

#[test]
fn overlap_point_is_query_center() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let center = Vec3::new(0.1, 0.9, -0.2);
        let hit = physics.capsule_overlap(&capsule(center, 0.5, 0.3))[0];
        assert!((hit.point - center).length() < 1e-6, "{kind:?}");
    }
}

#[test]
fn overlap_normal_is_zero() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))[0];
        assert_eq!(hit.normal, Vec3::ZERO, "{kind:?}");
    }
}

#[test]
fn overlap_marks_started_inside() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))[0];
        assert!(hit.started_inside, "{kind:?}");
    }
}

#[test]
fn overlap_sorted_by_node_ascending() {
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
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(2.0, 0.5, 2.0),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(1.0, 0.5, 1.0),
                ),
            ],
        );
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.2, 0.0), 0.1, 0.1));
        let mut expect = ids.clone();
        expect.sort();
        let got: Vec<NodeId> = hits.iter().map(|h| h.node).collect();
        assert_eq!(got, expect, "{kind:?}");
    }
}

#[test]
fn overlap_reports_each_body_once() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(2.0, 0.5, 2.0),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 0.5, 0.0),
                    Vec3::new(1.0, 0.5, 1.0),
                ),
            ],
        );
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.3, 0.0), 0.2, 0.2));
        assert_eq!(hits.len(), 2, "{kind:?}");
        let distinct: std::collections::BTreeSet<u64> = hits.iter().map(|h| h.body).collect();
        assert_eq!(distinct.len(), hits.len(), "{kind:?} 不应重复报同一刚体");
    }
}

#[test]
fn overlap_rejects_zero_radius() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 0.5, 0.0))
            .is_empty());
    }
}

#[test]
fn overlap_rejects_negative_radius() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 0.5, -0.3))
            .is_empty());
    }
}

#[test]
fn overlap_rejects_nan_center() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let nan = capsule(Vec3::new(f32::NAN, 0.5, 0.0), 0.5, 0.3);
        assert!(physics.capsule_overlap(&nan).is_empty(), "{kind:?}");
    }
}

#[test]
fn overlap_allows_zero_half_height() {
    // 半球（两圆头贴着）= 球：不是退化输入，必须能查
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 0.0, 0.4));
        assert_eq!(hits.len(), 1, "{kind:?}");
    }
}

#[test]
fn overlap_on_empty_world_is_empty() {
    for kind in selectable() {
        assert!(empty(kind)
            .capsule_overlap(&capsule(Vec3::ZERO, 0.5, 0.3))
            .is_empty());
    }
}

#[test]
fn overlap_reports_side_contact_clear_case() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        // 胶囊悬在盒子正侧方 0.1（明显相交）
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.55, 0.5, 0.0), 0.3, 0.3));
        assert_eq!(hits.len(), 1, "{kind:?}");
    }
}

#[test]
fn overlap_touching_top_clear_case() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        // 胶囊底略微压进盒顶 0.01
        let center = Vec3::new(0.0, 1.0 + 0.4 + 0.3 - 0.01, 0.0);
        assert_eq!(
            physics.capsule_overlap(&capsule(center, 0.4, 0.3)).len(),
            1,
            "{kind:?}"
        );
    }
}

#[test]
fn overlap_clearly_above_is_empty() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let center = Vec3::new(0.0, 1.0 + 0.4 + 0.3 + 0.5, 0.0);
        assert!(
            physics
                .capsule_overlap(&capsule(center, 0.4, 0.3))
                .is_empty(),
            "{kind:?}"
        );
    }
}

#[test]
fn overlap_disabled_physics_is_empty() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[cube()]);
        physics.set_enabled(false);
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))
            .is_empty());
    }
}

#[test]
fn overlap_null_backend_is_empty() {
    let physics = Physics::with_backend(Box::new(NullBackend::new()));
    assert!(physics
        .capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 0.5, 0.3))
        .is_empty());
}

#[test]
fn overlap_sphere_body_is_reported() {
    for kind in selectable() {
        let (physics, ids) = world_shapes(
            kind,
            &[(
                ColliderShape::Sphere,
                Vec3::new(0.0, 0.5, 0.0),
                Vec3::splat(0.5),
            )],
        );
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 0.2, 0.2));
        assert_eq!(hits.len(), 1, "{kind:?}");
        assert_eq!(hits[0].node, ids[0]);
    }
}

#[test]
fn overlap_after_destroy_is_empty() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[cube()]);
        let handle = physics.try_create_body(
            {
                let mut scene = Scene::new();
                scene
                    .spawn(ObjectDesc {
                        name: "tmp".into(),
                        ..Default::default()
                    })
                    .expect("spawn")
            },
            BodyDesc {
                shape: ColliderShape::Box,
                half_extents: Vec3::splat(0.5),
                position: Vec3::new(5.0, 0.5, 0.0),
                ..Default::default()
            },
        );
        if let Some(h) = handle {
            physics.destroy_body(h);
        }
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(5.0, 0.5, 0.0), 0.2, 0.2))
            .is_empty());
    }
}

#[test]
fn overlap_agrees_between_backends_on_clear_case() {
    let mut seen: Vec<usize> = Vec::new();
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        seen.push(
            physics
                .capsule_overlap(&capsule(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))
                .len(),
        );
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "两后端口径必须一致: {seen:?}"
    );
}

#[test]
fn overlap_large_capsule_covers_all_bodies() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                (
                    ColliderShape::Box,
                    Vec3::new(1.0, 0.5, 0.0),
                    Vec3::splat(0.5),
                ),
                (
                    ColliderShape::Box,
                    Vec3::new(-1.0, 0.5, 0.0),
                    Vec3::splat(0.5),
                ),
            ],
        );
        let hits = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.5, 0.0), 1.0, 1.5));
        assert_eq!(hits.len(), 2, "{kind:?}");
    }
}

// =========================================================== 胶囊扫掠（契约）

#[test]
fn sweep_down_hits_floor() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("应撞到地板");
        assert!(
            hit.distance > 0.0 && hit.distance < 10.0,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn sweep_distance_matches_gap() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        // 地板顶 0.0；胶囊底 = 5.0-1.0-0.5 = 3.5 → 落 3.5
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("命中");
        assert!(
            (hit.distance - 3.5).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn sweep_normal_faces_up_on_floor() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("命中");
        assert!(hit.normal.y > 0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn sweep_started_inside_reports_zero_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 0.6, 0.0), 0.4, 0.3),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("起步即重叠");
        assert_eq!(hit.distance, 0.0, "{kind:?}");
        assert!(hit.started_inside, "{kind:?}");
    }
}

#[test]
fn sweep_started_inside_normal_is_unit() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 0.6, 0.0), 0.4, 0.3),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("起步即重叠");
        assert!(hit.started_inside, "{kind:?}");
        assert!(
            (hit.normal.length() - 1.0).abs() < 1e-2,
            "{kind:?} n={:?}",
            hit.normal
        );
    }
}

/// 起点就与支撑面重叠（贴地站立、零间隙）时，法线必须仍是**接触面法线**。
///
/// 曾经约定重叠态一律返回 `-dir`，B03 在真后端上直接把这条约定推翻了：站着贴地
/// 时 parry 的预测距离会把「踩在地上」报成 penetrating，法线若被伪造成 `-dir`，
/// 脚下这块支撑面就会被上层当成墙 —— 角色一落地就迈不开腿。所以约定改成
/// 「有几何就用真实法线，拿不到才退回 `-dir`」，这里把它钉死。
///
/// 方向斜向下（而不是正下方）是为了区分实现：只做 `-dir` 的实现会给出斜上方，
/// 真读几何的实现给出 `+Y`，两者不会混淆。
#[test]
fn sweep_started_inside_normal_follows_contact_face() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        // 胶囊底陷进表面 1 厘米（零间隙在真后端上会被判成「刚好相切」而非重叠），
        // 再斜向下推进：起步即重叠，且最小平移方向只能是 +Y。
        let cap = capsule(Vec3::new(0.0, 1.49, 0.0), 1.0, 0.5);
        let hit = physics
            .sweep_capsule(cap, Vec3::new(1.0, -1.0, 0.0), 3.0)
            .expect("起步即重叠");
        assert!(hit.started_inside, "{kind:?}");
        // 内置后端的 AABB 模型没有接触面，按契约退回 `-dir`，不参与这条断言。
        if kind == PhysicsKind::Rapier {
            assert!(
                hit.normal.y > 0.9,
                "{kind:?}：重叠态应给支撑面法线 +Y，实际 n={:?}",
                hit.normal
            );
        }
    }
}

#[test]
fn sweep_rejects_zero_direction() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(cap, Vec3::ZERO, 10.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_rejects_zero_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(cap, Vec3::NEG_Y, 0.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_rejects_negative_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(cap, Vec3::NEG_Y, -3.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_misses_when_distance_short() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(cap, Vec3::NEG_Y, 1.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_picks_nearest_obstacle() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[
                floor(),
                (
                    ColliderShape::Box,
                    Vec3::new(0.0, 2.0, 0.0),
                    Vec3::new(5.0, 0.1, 5.0),
                ),
            ],
        );
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 6.0, 0.0), 1.0, 0.5),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("命中上层板");
        // 上层板顶 2.1，胶囊底 4.5 → 2.4
        assert!(
            (hit.distance - 2.4).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn sweep_horizontal_hits_wall() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(-3.0, 0.5, 0.0),
                Vec3::new(0.2, 0.5, 2.0),
            )],
        );
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 0.6, 0.0), 0.4, 0.3),
                Vec3::NEG_X,
                10.0,
            )
            .expect("命中墙");
        // 墙面 x=-2.8，胶囊半径 0.3 → 2.5
        assert!(
            (hit.distance - 2.5).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
        assert!(hit.normal.x > 0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn sweep_upward_hits_ceiling() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(2.0, 0.2, 2.0),
            )],
        );
        let hit = physics
            .sweep_capsule(capsule(Vec3::new(0.0, 0.5, 0.0), 0.4, 0.3), Vec3::Y, 10.0)
            .expect("命中天花板");
        // 天花板底 2.8，胶囊顶 1.2 → 1.6
        assert!(
            (hit.distance - 1.6).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
        assert!(hit.normal.y < -0.9, "{kind:?} n={:?}", hit.normal);
    }
}

#[test]
fn sweep_rejects_degenerate_capsule() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let bad = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.0);
        assert!(
            physics.sweep_capsule(bad, Vec3::NEG_Y, 10.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_rejects_non_finite_center() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let bad = capsule(Vec3::new(f32::INFINITY, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(bad, Vec3::NEG_Y, 10.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_disabled_physics_is_none() {
    for kind in selectable() {
        let (mut physics, _) = world_shapes(kind, &[floor()]);
        physics.set_enabled(false);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            physics.sweep_capsule(cap, Vec3::NEG_Y, 10.0).is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn sweep_null_backend_is_none() {
    let physics = Physics::with_backend(Box::new(NullBackend::new()));
    let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
    assert!(physics.sweep_capsule(cap, Vec3::NEG_Y, 10.0).is_none());
}

#[test]
fn sweep_hit_body_matches_overlap() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[cube()]);
        let cap = capsule(Vec3::new(0.0, 3.0, 0.0), 0.4, 0.3);
        let swept = physics.sweep_capsule(cap, Vec3::NEG_Y, 10.0).expect("命中");
        let overlapped = physics.capsule_overlap(&capsule(Vec3::new(0.0, 0.6, 0.0), 0.4, 0.3));
        assert_eq!(swept.body, overlapped[0].body, "{kind:?}");
        assert_eq!(swept.node, overlapped[0].node, "{kind:?}");
    }
}

#[test]
fn sweep_does_not_tunnel_through_thin_plate() {
    for kind in selectable() {
        let (physics, _) = world_shapes(
            kind,
            &[(
                ColliderShape::Box,
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(5.0, 0.05, 5.0),
            )],
        );
        // 薄板顶 0.05，胶囊底 9.5，一次扫 20 —— 必须报命中而不是穿过去
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 10.0, 0.0), 0.2, 0.3),
                Vec3::NEG_Y,
                20.0,
            )
            .expect("薄板也要挡住");
        assert!(
            (hit.distance - 9.45).abs() < 5e-2,
            "{kind:?} d={}",
            hit.distance
        );
    }
}

#[test]
fn sweep_unnormalized_dir_gives_same_distance() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        let a = physics.sweep_capsule(cap, Vec3::NEG_Y, 10.0).expect("命中");
        let b = physics
            .sweep_capsule(cap, Vec3::new(0.0, -9.0, 0.0), 10.0)
            .expect("命中");
        assert!((a.distance - b.distance).abs() < 1e-3, "{kind:?}");
    }
}

#[test]
fn sweep_agrees_between_backends_on_clear_case() {
    let mut ds: Vec<f32> = Vec::new();
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let hit = physics
            .sweep_capsule(
                capsule(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
                Vec3::NEG_Y,
                10.0,
            )
            .expect("命中");
        ds.push(hit.distance);
    }
    assert!(
        ds.windows(2).all(|w| (w[0] - w[1]).abs() < 5e-2),
        "两后端距离应一致: {ds:?}"
    );
}

#[test]
fn sweep_point_moves_along_direction() {
    for kind in selectable() {
        let (physics, _) = world_shapes(kind, &[floor()]);
        let center = Vec3::new(0.0, 5.0, 0.0);
        let hit = physics
            .sweep_capsule(capsule(center, 1.0, 0.5), Vec3::NEG_Y, 10.0)
            .expect("命中");
        let expect = center + Vec3::NEG_Y * hit.distance;
        assert!((hit.point - expect).length() < 1e-3, "{kind:?}");
    }
}
