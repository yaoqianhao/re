//! 静态层「悬空」初筛（M1-S4-E）。
//!
//! **这一层只做剔除**：拿 AABB 近似找「下方什么都没有」的物件，给物理层一个候选清单。
//! 真正的结论必须由物理层（`r3e-editor-host/src/verify/*`，向下 `shape_cast` 精确找支撑面）
//! 给出，并按 `(category, target, code)` 覆盖本层的近似条目——铁律见
//! `docs/M1_S4_PLAN.md` §4 S4-E。
//!
//! 三条刻意的克制：
//!
//! 1. `body = dynamic` 的刚体不判——它每帧都在动，静态快照里的「悬空」是下一帧的落地过程；
//! 2. 父子节点不互相充当支撑面？**要**充当——子物件本来就踩在父物件上（反之亦然），
//!    AABB 相交的父子关系只影响穿模判定，不影响支撑判定；
//! 3. 超量截断：一份场景报上千条悬空没有可读性，超上限后合并成一条汇总条目。

use r3e_core::{Aabb, Node, Scene};

use super::{geom, Issue};

/// 支撑面高差容差：底面与支撑面高差在此范围内视为「踩在上面」。
///
/// 5cm 是「视觉上已经贴地、但浮点/建模误差还没暴露」的经验值；再放宽就会把真正的
/// 悬空平台吞掉，再收紧则会让每一个 `y = 0.0001` 的手写场景都报错。
const SUPPORT_TOLERANCE: f32 = 0.05;

/// 单次上报上限：超出后只报一条汇总。
const SUPPORT_MAX_REPORTS: usize = 128;

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    let boxes: Vec<(&Node, Aabb)> = scene
        .iter()
        .filter(|n| !geom::is_dynamic(n))
        .filter_map(|n| geom::node_aabb(n).map(|b| (n, b)))
        .filter(|(_, b)| !b.is_empty())
        .collect();

    let mut reported = 0usize;
    let mut truncated = 0usize;

    for (node, b) in &boxes {
        if supported(scene, node, b, &boxes) {
            continue;
        }
        if reported >= SUPPORT_MAX_REPORTS {
            truncated += 1;
            continue;
        }
        reported += 1;
        let cx = (b.min.x + b.max.x) * 0.5;
        let cz = (b.min.z + b.max.z) * 0.5;
        out.push(
            Issue::warn(
                "support.suspect",
                format!("nodes[{}]", node.id),
                format!(
                    "物件「{}」底面 y={:.3} 向下 {} 内没有支撑面（地面 / 地形 / 其他物件顶面），疑似悬空",
                    node.name, b.min.y, SUPPORT_TOLERANCE
                ),
            )
            .with_target(node.name.clone())
            .at([cx, b.min.y, cz])
            .approx()
            .with_hint("由物理层向下 `shape_cast` 精确复核后再决定贴地还是补支撑面"),
        );
    }

    if truncated > 0 {
        out.push(
            Issue::warn(
                "support.suspect",
                "nodes",
                format!(
                    "另有 {truncated} 个物件疑似悬空已截断（本层单次上限 {SUPPORT_MAX_REPORTS} 条）"
                ),
            )
            .approx()
            .with_hint("先用 `query` / `find_all` 缩小范围再看"),
        );
    }
}

/// 是否有支撑：地面 / 地形高度，或任一其他物件的顶面，与自身底面的高差落在容差内。
fn supported(scene: &Scene, node: &Node, b: &Aabb, boxes: &[(&Node, Aabb)]) -> bool {
    let cx = (b.min.x + b.max.x) * 0.5;
    let cz = (b.min.z + b.max.z) * 0.5;

    let ground = geom::terrain_height(scene, cx, cz).unwrap_or(geom::FLOOR_Y);
    if near(b.min.y, ground) {
        return true;
    }

    boxes.iter().any(|(other, ob)| {
        !std::ptr::eq(*other, node) && xz_overlap(b, ob) && near(b.min.y, ob.max.y)
    })
}

fn near(bottom: f32, support: f32) -> bool {
    (bottom - support).abs() <= SUPPORT_TOLERANCE
}

/// 两块底面在 XZ 平面的轴对齐矩形是否相交（**含相切**：面积 0 的接触也算踩住）。
fn xz_overlap(a: &Aabb, b: &Aabb) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.z <= b.max.z && b.min.z <= a.max.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::ObjectDesc;

    fn scene_with(nodes: &[(&str, [f32; 3], [f32; 3])]) -> Scene {
        let mut s = Scene::new();
        for (name, pos, scale) in nodes {
            s.spawn(ObjectDesc {
                name: (*name).to_string(),
                position: Some(*pos),
                scale: Some(*scale),
                ..Default::default()
            })
            .expect("spawn");
        }
        s.update_world_transforms();
        s
    }

    fn run(scene: &Scene) -> Vec<Issue> {
        let mut out = Vec::new();
        check(scene, &mut out);
        out
    }

    #[test]
    fn grounded_node_is_quiet() {
        // 单位网格 scale=2 → 半高 1，position.y = 1 → 底面正好在 y=0
        let s = scene_with(&[("crate", [0.0, 1.0, 0.0], [2.0, 2.0, 2.0])]);
        assert!(run(&s).is_empty(), "贴地物件不该报悬空");
    }

    #[test]
    fn floating_node_reports_coordinate() {
        let s = scene_with(&[("island", [3.0, 5.0, -2.0], [2.0, 2.0, 2.0])]);
        let issues = run(&s);
        assert_eq!(issues.len(), 1);
        let i = &issues[0];
        assert_eq!(i.code, "support.suspect");
        assert_eq!(i.target.as_deref(), Some("island"));
        assert!(i.approximate, "静态层结论必须标近似");
        let loc = i.location.expect("悬空必须给坐标");
        assert!((loc[0] - 3.0).abs() < 1e-3);
        assert!((loc[1] - 4.0).abs() < 1e-3, "坐标取底面高度");
        assert!((loc[2] + 2.0).abs() < 1e-3);
    }

    #[test]
    fn stacked_node_rests_on_top_face() {
        // 底块顶面 y=2，上块底面 y=2 → 有支撑
        let s = scene_with(&[
            ("base", [0.0, 1.0, 0.0], [2.0, 2.0, 2.0]),
            ("top", [0.0, 3.0, 0.0], [2.0, 2.0, 2.0]),
        ]);
        assert!(run(&s).is_empty(), "摞在上面的物件由顶面支撑");
    }

    #[test]
    fn gap_over_tolerance_is_reported() {
        // 底块顶面 y=2，上块底面 y=2.4 → 差 0.4 > 0.05
        let s = scene_with(&[
            ("base", [0.0, 1.0, 0.0], [2.0, 2.0, 2.0]),
            ("float", [0.0, 3.4, 0.0], [2.0, 2.0, 2.0]),
        ]);
        let issues = run(&s);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].target.as_deref(), Some("float"));
    }

    #[test]
    fn dynamic_body_is_not_judged() {
        let mut s = Scene::new();
        s.spawn(ObjectDesc {
            name: "falling".to_string(),
            position: Some([0.0, 9.0, 0.0]),
            scale: Some([1.0, 1.0, 1.0]),
            physics: Some(r3e_core::PhysicsDesc {
                body: "dynamic".to_string(),
                mass: 1.0,
                restitution: None,
                friction: None,
                collider: None,
                half_extents: None,
                ccd: None,
            }),
            ..Default::default()
        })
        .expect("spawn");
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "动态刚体的悬空是落地过程，静态层不判");
    }

    #[test]
    fn terrain_ground_supports_node() {
        let mut s = Scene::new();
        s.terrain.push(r3e_core::TerrainPatch {
            name: "ground".to_string(),
            origin: [-16.0, -16.0],
            size: [32.0, 32.0],
            resolution: [2, 2],
            heights: Vec::new(),
            ..Default::default()
        });
        s.spawn(ObjectDesc {
            name: "on_terrain".to_string(),
            position: Some([0.0, 1.0, 0.0]),
            scale: Some([2.0, 2.0, 2.0]),
            ..Default::default()
        })
        .expect("spawn");
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "地形高度 0 与地面同口径");
    }

    #[test]
    fn truncation_is_summarized() {
        let mut s = Scene::new();
        for i in 0..(SUPPORT_MAX_REPORTS + 3) {
            s.spawn(ObjectDesc {
                name: format!("island{i}"),
                position: Some([i as f32 * 4.0, 20.0, 0.0]),
                scale: Some([1.0, 1.0, 1.0]),
                ..Default::default()
            })
            .expect("spawn");
        }
        s.update_world_transforms();
        let issues = run(&s);
        assert_eq!(issues.len(), SUPPORT_MAX_REPORTS + 1);
        let last = issues.last().expect("汇总条目");
        assert_eq!(last.path, "nodes");
        assert!(last.msg.contains("已截断"));
    }
}
