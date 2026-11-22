//! 端口 trait 的适配实现（M2 · A03 / D1=B）。
//!
//! 本文件只做一件事：把 [`PhysicsQueryPort`](crate::port::PhysicsQueryPort) 的三组
//! 查询接到 [`Physics`](crate::Physics) 门面上，并在两种 DTO 之间搬运字段。
//!
//! ## 为什么不把 impl 直接写在 lib.rs
//!
//! 门面 `lib.rs` 已经 1400+ 行（size_debt 上限 800，靠白名单兜着），端口适配是
//! **纯机械转发**，塞回门面只会继续推高债务。独立成文件后：端口契约在 `port.rs`、
//! 适配在 `port_impl.rs`、门面在 `lib.rs`，三块各自可读。
//!
//! ## 适配只搬字段，不做决策
//!
//! 是否降级、能力怎么申报、命中如何排序——全部由后端与 `port.rs` 的契约决定。
//! 这里唯一的「翻译」是 [`RayHit`] → [`PortHit`]：射线结果没有 `started_inside`
//! 字段，用 `distance == 0` 反推（起点与目标重叠时后端返回的正是 0）。
//! 这条规则必须在三套后端上一致，否则上层拿到的 `started_inside` 会随后端漂移。

use std::cell::RefCell;

use glam::Vec3;

use crate::port::{
    Capsule, CapsulePort, CastRay, GroundPort, GroundSample, PhysicsQueryPort, PortCaps, PortHit,
    RaycastPort,
};
use crate::{port_terrain, Physics};

impl RaycastPort for Physics {
    /// 射线查询：物理关闭时门面已返回 `None`，这里不再重复判断。
    ///
    /// 刚体 / 地形的合成在 [`port_terrain::cast_ray`]：这里只转发。
    fn cast_ray(&self, ray: &CastRay) -> Option<PortHit> {
        port_terrain::cast_ray(self, ray)
    }
}

impl CapsulePort for Physics {
    fn capsule_overlap(&self, capsule: &Capsule) -> Vec<PortHit> {
        port_terrain::capsule_overlap(self, capsule)
    }

    fn capsule_sweep(&self, capsule: &Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        port_terrain::capsule_sweep(self, capsule, dir, max_distance)
    }
}

impl GroundPort for Physics {
    /// 地面采样：地形 / 地面平面的择优（取更高者）已经在门面
    /// [`Physics::sample_ground`] 里做完，端口只做转发。
    ///
    /// 这里故意不重复合成：一旦端口自己再合一次，`&Physics` 与 `&dyn GroundPort`
    /// 两条调用路径就会各自算一遍，日后改一边漏一边，症状是「角色直接调用比
    /// 端口调用低一截」这种极难定位的偏差。
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        Physics::sample_ground(self, x, z)
    }
}

impl PhysicsQueryPort for Physics {
    /// 门面的能力申报：三组查询都**接线了**，但这不代表「此刻能查」——
    /// 物理被关掉时门面一律返回空，能力位仍报可用（能力是接线事实，不是开关状态）。
    ///
    /// 这一点与 `BackendCaps` 一致：后者申报的是后端接线能力，不是运行时开关。
    fn port_caps(&self) -> PortCaps {
        PortCaps::FULL
    }
}

/// 共享槽位本身就是查询端口（M2-B01）。
///
/// 装配层手里是 `Rc<RefCell<Physics>>`（脚本 / 编辑器 / 角色系统共用一个后端），
/// 而 `r3e-character` 只许认端口 trait、不许认 [`Physics`] 具体类型（`doc-gate`
/// 规则 8 看守）。实现落在**内层** `RefCell<Physics>` 上，于是
/// `Rc<RefCell<Physics>>` 能直接强制转换成 `Rc<dyn PhysicsQueryPort>`，
/// 装配层不必包一层转接头。
///
/// 借用期只覆盖单次查询：不在端口里缓存 `Ref`，也不跨查询持有，避免
/// 「角色推进时物理正被脚本改」这类重入 panic —— 重入时 `borrow()` 会当场
/// 报错而不是给出半旧的结果，这是想要的失败方式。
impl RaycastPort for RefCell<Physics> {
    fn cast_ray(&self, ray: &CastRay) -> Option<PortHit> {
        self.borrow().cast_ray(ray)
    }
}

impl CapsulePort for RefCell<Physics> {
    fn capsule_overlap(&self, capsule: &Capsule) -> Vec<PortHit> {
        self.borrow().capsule_overlap(capsule)
    }

    fn capsule_sweep(&self, capsule: &Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        self.borrow().capsule_sweep(capsule, dir, max_distance)
    }
}

impl GroundPort for RefCell<Physics> {
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        self.borrow().sample_ground(x, z)
    }
}

impl PhysicsQueryPort for RefCell<Physics> {
    fn port_caps(&self) -> PortCaps {
        self.borrow().port_caps()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodyDesc, ColliderShape, PhysicsConfig, PhysicsKind};
    use r3e_core::{NodeId, ObjectDesc, Scene};

    /// 建一个开了物理的门面（后端由 `kind` 指定），并生成 `specs` 里的盒子刚体。
    fn physics_with_boxes(kind: PhysicsKind, specs: &[(Vec3, Vec3)]) -> (Physics, Vec<NodeId>) {
        let mut scene = Scene::new();
        let mut physics = Physics::new();
        physics
            .apply_config(PhysicsConfig::default().with_enabled(true).with_kind(kind))
            .expect("后端应可选");
        let mut ids = Vec::new();
        for (i, (center, half)) in specs.iter().enumerate() {
            let id = scene
                .spawn(ObjectDesc {
                    name: format!("box{i}"),
                    ..Default::default()
                })
                .expect("spawn");
            physics.create_body(
                id,
                BodyDesc {
                    shape: ColliderShape::Box,
                    half_extents: *half,
                    position: *center,
                    ..Default::default()
                },
            );
            ids.push(id);
        }
        (physics, ids)
    }

    fn empty_physics(kind: PhysicsKind) -> Physics {
        let (physics, _) = physics_with_boxes(kind, &[]);
        physics
    }

    fn capsule_at(center: Vec3, half_height: f32, radius: f32) -> Capsule {
        Capsule::new(center, half_height, radius)
    }

    // ---------------------------------------------------------------- 能力申报

    #[test]
    fn port_caps_is_full_for_open_physics() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        assert_eq!(physics.port_caps(), PortCaps::FULL);
        assert_eq!(physics.port_caps().summary(), "raycast,capsule,ground");
    }

    #[test]
    fn port_caps_reports_three_groups() {
        let caps = empty_physics(PhysicsKind::preferred_default()).port_caps();
        assert!(caps.raycast);
        assert!(caps.capsule);
        assert!(caps.ground);
        assert!(!caps.is_empty());
    }

    #[test]
    fn port_caps_keys_are_ordered_and_stable() {
        let keys = empty_physics(PhysicsKind::preferred_default())
            .port_caps()
            .enabled_keys();
        assert_eq!(keys, vec!["raycast", "capsule", "ground"]);
    }

    #[test]
    fn port_caps_has_answers_each_key() {
        let caps = empty_physics(PhysicsKind::preferred_default()).port_caps();
        assert!(caps.has("raycast"));
        assert!(caps.has("capsule"));
        assert!(caps.has("ground"));
        assert!(!caps.has("joints"), "端口能力面不含关节");
        assert!(!caps.has(""), "空串不是能力键");
    }

    #[test]
    fn port_caps_survives_backend_switch_between_available() {
        // 换后端不改端口接线：三组查询在 builtin / rapier 上都接了线
        for kind in PhysicsKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != PhysicsKind::Null && k.is_available())
        {
            let physics = empty_physics(kind);
            assert_eq!(physics.port_caps(), PortCaps::FULL, "{kind:?}");
        }
    }

    #[test]
    fn port_caps_ignores_null_backend_but_facade_still_wired() {
        // Null 后端不接线，门面仍如实申报「接线了哪些查询」；
        // 它此刻靠 kind==Null 保证不返回任何结果（下一个测试钉死这一点）
        let physics = empty_physics(PhysicsKind::Null);
        assert_eq!(physics.port_caps(), PortCaps::FULL);
    }

    // ---------------------------------------------------------------- 关闭态

    #[test]
    fn closed_physics_reports_caps_but_returns_nothing() {
        let mut physics = Physics::new();
        physics.set_enabled(false);
        assert_eq!(physics.port_caps(), PortCaps::FULL);
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 100.0))
            .is_none());
        let capsule = capsule_at(Vec3::ZERO, 0.5, 0.4);
        assert!(Physics::overlap_capsule(&physics, capsule).is_empty());
        assert!(Physics::sweep_capsule(&physics, capsule, Vec3::NEG_Y, 10.0).is_none());
        assert!(Physics::sample_ground(&physics, 0.0, 0.0).is_none());
    }

    #[test]
    fn closed_physics_hides_existing_bodies_from_ports() {
        let (mut physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::ZERO, Vec3::splat(1.0))],
        );
        // 有刚体时能查到
        assert!(!Physics::overlap_capsule(&physics, capsule_at(Vec3::ZERO, 0.5, 0.4)).is_empty());
        physics.set_enabled(false);
        // 关掉后一律空：端口不越过门面开关
        assert!(Physics::overlap_capsule(&physics, capsule_at(Vec3::ZERO, 0.5, 0.4)).is_empty());
        assert!(Physics::sweep_capsule(
            &physics,
            capsule_at(Vec3::ZERO, 0.5, 0.4),
            Vec3::NEG_Y,
            10.0
        )
        .is_none());
        assert!(Physics::sample_ground(&physics, 0.0, 0.0).is_none());
    }

    #[test]
    fn reenabling_restores_port_results() {
        let (mut physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        physics.set_enabled(false);
        assert!(Physics::sample_ground(&physics, 0.0, 0.0).is_none());
        physics.set_enabled(true);
        assert!(Physics::sample_ground(&physics, 0.0, 0.0).is_some());
    }

    #[test]
    fn degenerate_ray_is_rejected_by_port_not_by_backend() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::ZERO, Vec3::ZERO, 10.0))
            .is_none());
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::ZERO, Vec3::NEG_Y, 0.0))
            .is_none());
    }

    // ---------------------------------------------------------------- 射线端口

    #[test]
    fn raycast_port_hits_box_top() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中盒顶");
        // 盒顶 y=1.0，射线起点 y=10 → 世界距离 9.0
        assert!(
            (hit.distance - 9.0).abs() < 1e-2,
            "distance={}",
            hit.distance
        );
        assert!(!hit.started_inside);
    }

    #[test]
    fn raycast_port_returns_world_point_on_surface() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中");
        assert!((hit.point.y - 1.0).abs() < 1e-2, "point={:?}", hit.point);
        assert!((hit.point.x).abs() < 1e-3);
        assert!((hit.point.z).abs() < 1e-3);
    }

    #[test]
    fn raycast_port_normal_points_back_at_caster() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中");
        assert!(hit.normal.y > 0.9, "法线应朝上，实际 {:?}", hit.normal);
    }

    #[test]
    fn raycast_port_carries_body_and_node() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中");
        assert_eq!(hit.node, ids[0]);
        // 句柄口径由后端决定（rapier 不保证从 0 起），只钉「指向唯一那个刚体」
        assert_eq!(physics.body_count(), 1);
        let again = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中");
        assert_eq!(hit.body, again.body, "同一刚体两次查询必须同句柄");
    }

    #[test]
    fn raycast_port_respects_max_distance() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 5.0))
            .is_none());
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 15.0))
            .is_some());
    }

    #[test]
    fn raycast_port_misses_when_pointing_away() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::Y, 20.0))
            .is_none());
    }

    #[test]
    fn raycast_port_accepts_unnormalized_dir() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hit = physics
            .cast_ray(&CastRay::new(
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(0.0, -7.5, 0.0),
                20.0,
            ))
            .expect("方向量级不影响命中");
        // 距离是世界单位，不是 t 参数
        assert!(
            (hit.distance - 9.0).abs() < 1e-2,
            "distance={}",
            hit.distance
        );
    }

    #[test]
    fn raycast_port_picks_nearest_of_two_boxes() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[
                (Vec3::new(0.0, 4.0, 0.0), Vec3::splat(0.5)),
                (Vec3::new(0.0, 1.0, 0.0), Vec3::splat(0.5)),
            ],
        );
        let hit = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("应命中近的那个");
        assert!((hit.point.y - 4.5).abs() < 1e-2, "point={:?}", hit.point);
    }

    // ---------------------------------------------------------------- 胶囊重叠

    #[test]
    fn capsule_overlap_finds_enclosing_box() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let hits =
            Physics::overlap_capsule(&physics, capsule_at(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].node, ids[0]);
    }

    #[test]
    fn capsule_overlap_reports_zero_distance_contract() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let center = Vec3::new(0.0, 0.9, 0.0);
        let hit = Physics::overlap_capsule(&physics, capsule_at(center, 0.5, 0.3))[0];
        assert_eq!(hit.distance, 0.0);
        assert!(hit.started_inside);
        assert_eq!(hit.normal, Vec3::ZERO, "重叠查询无法线");
        assert!((hit.point - center).length() < 1e-6, "重叠点取查询中心");
    }

    #[test]
    fn capsule_overlap_sorts_by_node() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[
                (Vec3::new(0.0, 0.5, 0.0), Vec3::new(2.0, 0.5, 2.0)),
                (Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.5, 0.5, 1.5)),
                (Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0)),
            ],
        );
        let hits =
            Physics::overlap_capsule(&physics, capsule_at(Vec3::new(0.0, 0.6, 0.0), 0.4, 0.2));
        assert_eq!(hits.len(), 3);
        let mut expected = ids.clone();
        expected.sort();
        let got: Vec<NodeId> = hits.iter().map(|h| h.node).collect();
        assert_eq!(got, expected, "必须按 node 升序");
    }

    #[test]
    fn capsule_overlap_empty_when_far_away() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5))],
        );
        assert!(Physics::overlap_capsule(
            &physics,
            capsule_at(Vec3::new(50.0, 0.5, 0.0), 0.5, 0.3)
        )
        .is_empty());
    }

    #[test]
    fn capsule_overlap_rejects_degenerate_capsule() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::ZERO, Vec3::splat(1.0))],
        );
        // 零半径 / 非有限值：端口层不猜，直接空
        assert!(Physics::overlap_capsule(&physics, capsule_at(Vec3::ZERO, 0.5, 0.0)).is_empty());
        assert!(Physics::overlap_capsule(
            &physics,
            capsule_at(Vec3::new(f32::NAN, 0.0, 0.0), 0.5, 0.3)
        )
        .is_empty());
    }

    #[test]
    fn capsule_overlap_on_empty_world_is_empty() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        assert!(Physics::overlap_capsule(&physics, capsule_at(Vec3::ZERO, 0.5, 0.3)).is_empty());
    }

    // ---------------------------------------------------------------- 胶囊扫掠

    #[test]
    fn capsule_sweep_hits_floor_from_above() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.5, 5.0))],
        );
        // 胶囊底面 y=3.5，地板顶面 y=0.5 → 落 3.0
        let hit = Physics::sweep_capsule(
            &physics,
            capsule_at(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
            Vec3::NEG_Y,
            10.0,
        )
        .expect("应撞到地板");
        assert!(
            (hit.distance - 3.0).abs() < 5e-2,
            "distance={}",
            hit.distance
        );
        assert_eq!(hit.node, ids[0]);
        assert!(!hit.started_inside);
        assert!(hit.normal.y > 0.9, "地板法线朝上，实际 {:?}", hit.normal);
    }

    #[test]
    fn capsule_sweep_reports_started_inside() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let cap = capsule_at(Vec3::new(0.0, 0.6, 0.0), 0.6, 0.3);
        let hit = Physics::sweep_capsule(&physics, cap, Vec3::NEG_Y, 10.0).expect("起步即重叠");
        assert!(hit.started_inside);
        assert_eq!(hit.distance, 0.0);
        assert!((hit.normal - Vec3::Y).length() < 1e-3, "重叠时法线取 -dir");
    }

    #[test]
    fn capsule_sweep_misses_when_far_below() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.5, 5.0))],
        );
        assert!(Physics::sweep_capsule(
            &physics,
            capsule_at(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
            Vec3::NEG_Y,
            1.0,
        )
        .is_none());
    }

    #[test]
    fn capsule_sweep_rejects_degenerate_inputs() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.5, 5.0))],
        );
        let cap = capsule_at(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5);
        assert!(
            Physics::sweep_capsule(&physics, cap, Vec3::ZERO, 10.0).is_none(),
            "零方向"
        );
        assert!(
            Physics::sweep_capsule(&physics, cap, Vec3::NEG_Y, 0.0).is_none(),
            "零距离"
        );
        assert!(
            Physics::sweep_capsule(
                &physics,
                capsule_at(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.0),
                Vec3::NEG_Y,
                10.0
            )
            .is_none(),
            "零半径"
        );
    }

    #[test]
    fn capsule_sweep_accepts_unnormalized_dir() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.5, 5.0))],
        );
        let hit = Physics::sweep_capsule(
            &physics,
            capsule_at(Vec3::new(0.0, 5.0, 0.0), 1.0, 0.5),
            Vec3::new(0.0, -3.0, 0.0),
            10.0,
        )
        .expect("量级不影响命中");
        assert!(
            (hit.distance - 3.0).abs() < 5e-2,
            "distance={}",
            hit.distance
        );
    }

    #[test]
    fn capsule_sweep_picks_nearest_obstacle() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[
                (Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.5, 5.0)),
                (Vec3::new(0.0, 2.0, 0.0), Vec3::new(5.0, 0.1, 5.0)),
            ],
        );
        let hit = Physics::sweep_capsule(
            &physics,
            capsule_at(Vec3::new(0.0, 6.0, 0.0), 1.0, 0.5),
            Vec3::NEG_Y,
            10.0,
        )
        .expect("应命中近的上层板");
        // 上层板顶面 y=2.1，胶囊底 y=4.5 → 2.4
        assert!(
            (hit.distance - 2.4).abs() < 5e-2,
            "distance={}",
            hit.distance
        );
    }

    // ---------------------------------------------------------------- 地面采样

    #[test]
    fn sample_ground_returns_flat_when_no_bodies() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        let g = Physics::sample_ground(&physics, 0.0, 0.0).expect("自带地面恒在");
        assert_eq!(g.height, 0.0);
        assert_eq!(g.body, None);
        assert_eq!(g.node, None);
        assert_eq!(g.normal, Vec3::Y);
        assert!(g.is_walkable(0.5));
    }

    #[test]
    fn sample_ground_follows_facade_ground_level() {
        let mut physics = empty_physics(PhysicsKind::preferred_default());
        physics.set_ground(-3.0);
        let g = Physics::sample_ground(&physics, 1.0, -2.0).expect("应采样到地面");
        assert!((g.height + 3.0).abs() < 1e-3, "height={}", g.height);
    }

    #[test]
    fn sample_ground_finds_box_top() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let g = Physics::sample_ground(&physics, 0.0, 0.0).expect("应采样到盒顶");
        assert!((g.height - 1.0).abs() < 1e-2, "height={}", g.height);
        assert_eq!(g.node, Some(ids[0]));
        assert!(g.body.is_some());
    }

    #[test]
    fn sample_ground_falls_back_outside_box_footprint() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let g = Physics::sample_ground(&physics, 5.0, 5.0).expect("盒外回落到平地");
        assert!((g.height - 0.0).abs() < 1e-3);
        assert_eq!(g.node, None);
    }

    #[test]
    fn sample_ground_rejects_non_finite_input() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        assert!(Physics::sample_ground(&physics, f32::NAN, 0.0).is_none());
        assert!(Physics::sample_ground(&physics, 0.0, f32::INFINITY).is_none());
    }

    #[test]
    fn sample_ground_picks_highest_surface() {
        let (physics, ids) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[
                (Vec3::new(0.0, 0.5, 0.0), Vec3::new(3.0, 0.5, 3.0)),
                (Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 0.5, 1.0)),
            ],
        );
        let g = Physics::sample_ground(&physics, 0.0, 0.0).expect("应采样");
        assert!((g.height - 2.5).abs() < 1e-2, "height={}", g.height);
        assert_eq!(g.node, Some(ids[1]), "取最高的那个");
    }

    // ---------------------------------------------------------------- 对象安全

    #[test]
    fn port_is_object_safe_and_injectable() {
        let physics = empty_physics(PhysicsKind::preferred_default());
        let ports: Vec<&dyn PhysicsQueryPort> = vec![&physics];
        for port in ports {
            assert_eq!(port.port_caps(), PortCaps::FULL);
            assert!(port
                .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
                .is_none());
            assert!(port
                .capsule_overlap(&capsule_at(Vec3::ZERO, 0.5, 0.3))
                .is_empty());
            assert!(port
                .capsule_sweep(&capsule_at(Vec3::ZERO, 0.5, 0.3), Vec3::NEG_Y, 5.0)
                .is_none());
            assert!(port.sample_ground(0.0, 0.0).is_some());
        }
    }

    #[test]
    fn dyn_port_matches_facade_results() {
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let port: &dyn PhysicsQueryPort = &physics;
        let via_port = port
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .expect("端口命中");
        let via_facade =
            Physics::overlap_capsule(&physics, capsule_at(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3));
        assert_eq!(
            via_port.node, via_facade[0].node,
            "两条路径必须指向同一刚体"
        );
    }

    #[test]
    fn port_traits_are_separately_callable() {
        // 上层可以只取其中一个 trait 的引用，不需要整套端口
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let ground: &dyn GroundPort = &physics;
        assert!(ground.sample_ground(0.0, 0.0).is_some());
        let ray: &dyn RaycastPort = &physics;
        assert!(ray
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
            .is_some());
        let caps: &dyn CapsulePort = &physics;
        assert!(!caps
            .capsule_overlap(&capsule_at(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))
            .is_empty());
    }

    #[test]
    fn port_caps_and_behavior_agree() {
        // 申报与行为一致：申报了就必须查得到，查不到就必须没申报
        let (physics, _) = physics_with_boxes(
            PhysicsKind::preferred_default(),
            &[(Vec3::new(0.0, 0.5, 0.0), Vec3::new(1.0, 0.5, 1.0))],
        );
        let caps = physics.port_caps();
        assert_eq!(
            caps.raycast,
            physics
                .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
                .is_some()
        );
        assert_eq!(
            caps.capsule,
            !physics
                .capsule_overlap(&capsule_at(Vec3::new(0.0, 0.9, 0.0), 0.5, 0.3))
                .is_empty()
        );
        assert_eq!(caps.ground, physics.sample_ground(0.0, 0.0).is_some());
    }

    #[test]
    fn port_results_survive_backend_switch() {
        // 换后端不换契约：同一个场景，两套后端都必须答得出「脚下多高」
        let specs = [(Vec3::new(0.0, 0.5, 0.0), Vec3::new(2.0, 0.5, 2.0))];
        for kind in PhysicsKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != PhysicsKind::Null && k.is_available())
        {
            let (physics, _) = physics_with_boxes(kind, &specs);
            let g = physics.sample_ground(0.0, 0.0).expect("应有地面");
            assert!(
                (g.height - 1.0).abs() < 1e-2,
                "{kind:?} height={}",
                g.height
            );
            assert!(physics
                .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 20.0))
                .is_some());
        }
    }
}
