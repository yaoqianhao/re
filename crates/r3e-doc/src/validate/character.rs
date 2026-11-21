//! 角色配置与踩地判据（M2-B04）。
//!
//! 两条判据，都只吃**场景里显式写下的数据**：
//!
//! 1. `character.config.invalid` —— `Node::character` 的五个字段越界 / 非有限 / 互相矛盾
//!    （台阶高于身高、吸附长于身高）。写入路径（`r3e-doc` 的 `SetCharacter`）**不做校验**，
//!    合法性契约写在 [`r3e_core::CharacterSpec::validate`] 上，所以校验器要把关：
//!    非法配置留到步进时才炸，排查只能靠猜。
//! 2. `character.ground.penetration` —— 角色脚底（约定在节点世界位置处）低于支撑面
//!    超过 `snap_distance`。吸附的作用是把「走下缓坡时的毫米级悬空」拉回地面，
//!    超出吸附距离的埋深拉不回来，运行态会表现为「卡在地里、跳不出去」。
//!
//! **为什么这两条进 [`super::validate`] 而不进几何初筛**：它们不是「空间关系可疑」的
//! AABB 近似，而是拿角色自己的配置 + 场景显式位置/地形高度对账，结论确定。
//! 反过来说，本模块**不判**「角色走起来会不会掉下去」「斜坡能不能上去」——
//! 那是运行态问题，归 `r3e-character` 自己的判据测试（`tests/step_spec.rs` /
//! `tests/ramp_accuracy.rs`），校验器管不了每帧轨迹。
//!
//! 超量克制：同一份场景里角色数量天然有限（人工声明），不做上报上限。

use r3e_core::{Aabb, Node, Scene};

use super::{geom, Issue};

/// 把「物件顶面」认作支撑面的高度容差（米）。
///
/// 只认**脚底以下**的顶面：正好齐平的顶面（差 ≤ 1mm）也算踩住，避免浮点误差把
/// 「脚底 1e-7 悬空」判成穿地。高于脚底的顶面不是支撑面，是障碍物，不参与本判据。
const SUPPORT_CONTACT_TOL: f32 = 1e-3;

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    let bodies: Vec<(&Node, Aabb)> = scene
        .iter()
        .filter(|n| !geom::is_dynamic(n))
        .filter_map(|n| geom::node_aabb(n).map(|b| (n, b)))
        .filter(|(_, b)| !b.is_empty())
        .collect();

    for node in scene.iter() {
        let Some(spec) = node.character else {
            continue;
        };
        let p = node.world_position();
        let foot = [p.x, p.y, p.z];

        if let Err(err) = spec.validate() {
            out.push(
                Issue::error(
                    "character.config.invalid",
                    format!("nodes[{}].character", node.id),
                    format!("角色「{}」的配置非法：{err}", node.name),
                )
                .with_target(node.name.clone())
                .at(foot)
                .with_hint("越界值不做夹取：改半径 / 身高 / 坡度 / 台阶 / 吸附其中一个字段"),
            );
            // 配置非法时几何本身不可信，后续踩地判据没有意义
            continue;
        }

        let support = support_under(scene, node, &bodies, p.x, p.z, p.y);
        let depth = support - p.y;
        if depth > spec.snap_distance {
            out.push(
                Issue::error(
                    "character.ground.penetration",
                    format!("nodes[{}]", node.id),
                    format!(
                        "角色「{}」脚底 y={:.3} 埋进支撑面 y={:.3} 共 {:.3} 米，超出吸附距离 {:.3} 米（吸附拉不回）",
                        node.name, p.y, support, depth, spec.snap_distance
                    ),
                )
                .with_target(node.name.clone())
                .at([p.x, p.y, p.z])
                .with_hint("把节点抬回支撑面上；这里若是坑，就不该把物件声明成角色"),
            );
        }
    }
}

/// 角色脚下的支撑面高度（世界 y）。
///
/// 取值口径与静态层悬空初筛一致：地形高度（无地形时退到 [`geom::FLOOR_Y`]）与
/// **脚下**的其他物件顶面取最高者。父子关系不特殊处理——子物件踩在父物件上是正常用法。
fn support_under(
    scene: &Scene,
    node: &Node,
    bodies: &[(&Node, Aabb)],
    x: f32,
    z: f32,
    foot_y: f32,
) -> f32 {
    let mut h = geom::terrain_height(scene, x, z).unwrap_or(geom::FLOOR_Y);
    for (other, b) in bodies {
        if std::ptr::eq(*other, node) {
            continue;
        }
        if xz_contains(b, x, z) && b.max.y <= foot_y + SUPPORT_CONTACT_TOL {
            h = h.max(b.max.y);
        }
    }
    h
}

/// 点是否落在盒子的 XZ 投影内（含边界：踩在边缘上一角也算踩住）。
fn xz_contains(b: &Aabb, x: f32, z: f32) -> bool {
    b.min.x <= x && x <= b.max.x && b.min.z <= z && z <= b.max.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::{CharacterSpec, ObjectDesc};

    fn scene_with_character(name: &str, pos: [f32; 3], spec: CharacterSpec) -> Scene {
        let mut s = Scene::new();
        let id = s
            .spawn(ObjectDesc {
                name: name.to_string(),
                position: Some(pos),
                ..Default::default()
            })
            .expect("spawn");
        s.get_mut(id).expect("node").character = Some(spec);
        s.update_world_transforms();
        s
    }

    fn run(scene: &Scene) -> Vec<Issue> {
        let mut out = Vec::new();
        check(scene, &mut out);
        out
    }

    #[test]
    fn grounded_character_is_quiet() {
        let s = scene_with_character("hero", [0.0, 0.0, 0.0], CharacterSpec::new());
        assert!(run(&s).is_empty(), "脚底正好在支撑面上不该报错");
    }

    #[test]
    fn invalid_spec_reports_coordinate_and_target() {
        let mut spec = CharacterSpec::new();
        spec.radius = -1.0;
        let s = scene_with_character("hero", [1.0, 0.0, 2.0], spec);
        let issues = run(&s);
        assert_eq!(issues.len(), 1, "非法配置只报配置，不连带报穿地");
        let i = &issues[0];
        assert_eq!(i.code, "character.config.invalid");
        assert_eq!(i.target.as_deref(), Some("hero"));
        let loc = i.location.expect("必须给坐标");
        assert!((loc[0] - 1.0).abs() < 1e-3 && (loc[2] - 2.0).abs() < 1e-3);
    }

    #[test]
    fn buried_character_reports_penetration() {
        let s = scene_with_character("hero", [0.0, -0.5, 0.0], CharacterSpec::new());
        let issues = run(&s);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "character.ground.penetration");
        assert!(!issues[0].approximate, "配置 + 显式位置对账，结论确定");
    }

    #[test]
    fn shallow_sink_is_absorbed_by_snap() {
        let s = scene_with_character("hero", [0.0, -0.05, 0.0], CharacterSpec::new());
        assert!(run(&s).is_empty(), "吸附距离内的下沉属于正常踩地抖动");
    }

    #[test]
    fn platform_top_counts_as_support() {
        // 平台：单位网格 scale=2 半高 1，中心 y = 0 → 顶面 y = 1
        let mut s = Scene::new();
        s.spawn(ObjectDesc {
            name: "platform".to_string(),
            position: Some([0.0, 0.0, 0.0]),
            scale: Some([2.0, 2.0, 2.0]),
            ..Default::default()
        })
        .expect("platform");
        let id = s
            .spawn(ObjectDesc {
                name: "hero".to_string(),
                position: Some([0.0, 1.0, 0.0]),
                ..Default::default()
            })
            .expect("hero");
        s.get_mut(id).expect("node").character = Some(CharacterSpec::new());
        s.update_world_transforms();
        assert!(
            run(&s).is_empty(),
            "站在平台顶面上不该报穿地（地形在 y=0，平台顶面 y=1）"
        );
    }

    #[test]
    fn terrain_above_foot_is_penetration() {
        // 地形抬到 y = 0.5，角色脚底还在 y = 0 → 埋深 0.5 超出吸附距离
        let mut s = Scene::new();
        s.terrain.push(r3e_core::TerrainPatch {
            name: "ground".to_string(),
            origin: [-16.0, -16.0],
            size: [32.0, 32.0],
            resolution: [2, 2],
            heights: vec![0.5; 4],
            ..Default::default()
        });
        let id = s
            .spawn(ObjectDesc {
                name: "hero".to_string(),
                position: Some([0.0, 0.0, 0.0]),
                ..Default::default()
            })
            .expect("hero");
        s.get_mut(id).expect("node").character = Some(CharacterSpec::new());
        s.update_world_transforms();
        let codes: Vec<_> = run(&s).into_iter().map(|i| i.code).collect();
        assert_eq!(codes, vec!["character.ground.penetration"]);
    }
}
