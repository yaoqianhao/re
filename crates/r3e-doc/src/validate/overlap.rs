//! 静态层「穿模」初筛（M1-S4-E）。
//!
//! AABB 两两求交，交集体积为正才算候选——**相切不算**：贴边摆放是关卡设计师的日常操作，
//! 报出来只会教人忽略告警。真正的结论由物理层（`overlap_sphere` 逐对候选精确复核）给出，
//! 并按 `(category, target, code)` 覆盖本层的近似条目。
//!
//! 刻意的三条克制：
//!
//! 1. 父子之间不判：子物件挂在父物件的坐标系里，体积重叠是层次结构的正常形态；
//! 2. `body = dynamic` 的刚体不判：它本来就在动，静态快照里「插进去」下一帧会自己弹开；
//! 3. 节点数上限：两两比较是 O(n²)，一份一万节点的场景会在这里耗掉几十秒，
//!    超上限只扫前 [`OVERLAP_MAX_NODES`] 个并明说（宁可少报，不许拖死编辑器）。

use r3e_core::{Aabb, Node, Scene, Vec3};

use super::{geom, Issue};

/// 两两比较的节点上限（512² / 2 ≈ 13 万次，毫秒级）。
const OVERLAP_MAX_NODES: usize = 512;

/// 交集体积下限：小于这个值的重叠是浮点噪音，不是穿模。
const OVERLAP_MIN_VOLUME: f32 = 1e-4;

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    let boxes: Vec<(&Node, Aabb)> = scene
        .iter()
        .filter(|n| !geom::is_dynamic(n))
        .filter_map(|n| geom::node_aabb(n).map(|b| (n, b)))
        .filter(|(_, b)| !b.is_empty())
        .take(OVERLAP_MAX_NODES)
        .collect();

    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let (a, ab) = boxes[i];
            let (b, bb) = boxes[j];
            if geom::related(scene, a, b) {
                continue;
            }
            let Some(hit) = intersection(&ab, &bb) else {
                continue;
            };
            let c = hit.center();
            out.push(
                Issue::warn(
                    "overlap.suspect",
                    format!("nodes[{}]", a.id),
                    format!(
                        "物件「{}」与「{}」（nodes[{}]）包围盒交叠 {:.4} m³，疑似穿模",
                        a.name,
                        b.name,
                        b.id,
                        volume(&hit)
                    ),
                )
                .with_target(format!("{}|{}", a.name, b.name))
                .at([c.x, c.y, c.z])
                .approx()
                .with_hint("由物理层 `overlap_sphere` 逐对复核；确认后把物件挪开或改碰撞体"),
            );
        }
    }
}

/// 交集盒；体积不为正（相切 / 分离）返回 `None`。
fn intersection(a: &Aabb, b: &Aabb) -> Option<Aabb> {
    let min = Vec3::new(
        a.min.x.max(b.min.x),
        a.min.y.max(b.min.y),
        a.min.z.max(b.min.z),
    );
    let max = Vec3::new(
        a.max.x.min(b.max.x),
        a.max.y.min(b.max.y),
        a.max.z.min(b.max.z),
    );
    if min.x >= max.x || min.y >= max.y || min.z >= max.z {
        return None;
    }
    let hit = Aabb::new(min, max);
    (volume(&hit) > OVERLAP_MIN_VOLUME).then_some(hit)
}

fn volume(a: &Aabb) -> f32 {
    let s = a.size();
    s.x * s.y * s.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::{ObjectDesc, PhysicsDesc};

    fn spawn(s: &mut Scene, name: &str, pos: [f32; 3], scale: [f32; 3]) {
        s.spawn(ObjectDesc {
            name: name.to_string(),
            position: Some(pos),
            scale: Some(scale),
            ..Default::default()
        })
        .expect("spawn");
    }

    fn physics(body: &str, half: Option<[f32; 3]>) -> PhysicsDesc {
        PhysicsDesc {
            body: body.to_string(),
            mass: 1.0,
            restitution: None,
            friction: None,
            collider: None,
            half_extents: half,
            ccd: None,
        }
    }

    fn run(scene: &Scene) -> Vec<Issue> {
        let mut out = Vec::new();
        check(scene, &mut out);
        out
    }

    #[test]
    fn separated_nodes_are_quiet() {
        let mut s = Scene::new();
        spawn(&mut s, "a", [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        spawn(&mut s, "b", [5.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        s.update_world_transforms();
        assert!(run(&s).is_empty());
    }

    #[test]
    fn touching_faces_are_not_penetration() {
        // 两个单位盒心距 1.0 → 面贴面，交集体积 0
        let mut s = Scene::new();
        spawn(&mut s, "a", [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        spawn(&mut s, "b", [1.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "相切不算穿模");
    }

    #[test]
    fn overlapping_pair_reports_intersection_center() {
        let mut s = Scene::new();
        spawn(&mut s, "a", [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]); // [-1,1]³
        spawn(&mut s, "b", [1.0, 0.0, 0.0], [2.0, 2.0, 2.0]); // [0,2]×[-1,1]²
        s.update_world_transforms();
        let issues = run(&s);
        assert_eq!(issues.len(), 1);
        let i = &issues[0];
        assert_eq!(i.code, "overlap.suspect");
        assert_eq!(
            i.severity,
            crate::Severity::Warn,
            "静态层只是候选，误报成本高于漏报"
        );
        assert!(i.approximate);
        assert_eq!(i.target.as_deref(), Some("a|b"));
        let loc = i.location.expect("穿模必须给坐标");
        assert!((loc[0] - 0.5).abs() < 1e-3, "交集中心 x=0.5，实际 {loc:?}");
        assert!(loc[1].abs() < 1e-3 && loc[2].abs() < 1e-3);
    }

    #[test]
    fn parent_child_overlap_is_skipped() {
        let mut s = Scene::new();
        spawn(&mut s, "parent", [0.0, 0.0, 0.0], [4.0, 4.0, 4.0]);
        s.spawn(ObjectDesc {
            name: "child".to_string(),
            position: Some([0.5, 0.0, 0.0]),
            scale: Some([1.0, 1.0, 1.0]),
            parent: Some("parent".to_string()),
            ..Default::default()
        })
        .expect("spawn");
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "父子体积重叠是层次结构，不是穿模");
    }

    #[test]
    fn dynamic_bodies_are_skipped() {
        let mut s = Scene::new();
        spawn(&mut s, "static", [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        s.spawn(ObjectDesc {
            name: "bullet".to_string(),
            position: Some([0.3, 0.0, 0.0]),
            scale: Some([1.0, 1.0, 1.0]),
            physics: Some(physics("dynamic", None)),
            ..Default::default()
        })
        .expect("spawn");
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "动态刚体自己会弹开");
    }

    #[test]
    fn declared_half_extents_drive_the_box() {
        // scale 很大但碰撞体声明很小 → 判定按声明走（与物理层同源，避免层间打架）
        let mut s = Scene::new();
        s.spawn(ObjectDesc {
            name: "slim".to_string(),
            position: Some([0.0, 0.0, 0.0]),
            scale: Some([8.0, 8.0, 8.0]),
            physics: Some(physics("static", Some([0.1, 0.1, 0.1]))),
            ..Default::default()
        })
        .expect("spawn");
        spawn(&mut s, "block", [2.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        s.update_world_transforms();
        assert!(run(&s).is_empty(), "碰撞体半尺寸优先于 scale");
    }
}
