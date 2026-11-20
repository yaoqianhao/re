//! `app` 形态绑定测试（M2-A05 自 `app.rs` 内嵌测试块切出，逐条搬移、零删减）。

use super::*;

fn scene_of(objects: &str) -> Scene {
    let json = format!(
        r#"{{
              "name": "morph-bind-test",
              "camera": {{ "position": [0.0, 0.0, 6.0], "target": [0.0, 0.0, 0.0], "fov": 55.0 }},
              "light": {{ "direction": [-0.3, -0.6, -0.7], "color": [1.0, 1.0, 1.0],
                         "intensity": 1.0, "ambient": [0.5, 0.5, 0.5] }},
              "objects": {objects}
            }}"#
    );
    r3e_doc::parse_scene(&json).expect("最小场景应能解析")
}

/// 只有「引用了带形状资产」的节点才绑；无形状资产与内置网格一律跳过。
#[test]
fn attaches_morph_only_to_meshes_with_targets() {
    let mut scene = scene_of(
        r#"[
              { "name": "Face", "mesh": "asset:face.glb#0" },
              { "name": "Body", "mesh": "asset:body.glb#0" },
              { "name": "Block" },
              { "name": "FaceAgain", "mesh": "asset:face.glb#0" }
            ]"#,
    );
    let face = scene.find_id("Face").expect("Face 存在");
    let body = scene.find_id("Body").expect("Body 存在");
    let block = scene.find_id("Block").expect("Block 存在");
    // 只让 Face 引用的那份资产带形状，Body 的资产是「无形状网格」
    let face_asset = scene
        .iter()
        .find(|node| node.id == face)
        .and_then(|node| match node.mesh {
            MeshRef::Asset { asset, .. } => Some(asset),
            MeshRef::Builtin(_) => None,
        })
        .expect("Face 引用资产网格");

    let bound = attach_morph_bindings_with(&mut scene, |mesh| match mesh {
        MeshRef::Asset { asset, .. } if asset == face_asset => Some(3),
        _ => None,
    });

    assert_eq!(bound, 2, "两个引用 face.glb#0 的节点各绑一次");
    assert_eq!(scene.morph_of(face).map(|m| m.target_count), Some(3));
    assert!(scene.morph_of(body).is_none(), "无形状的资产不绑");
    assert!(scene.morph_of(block).is_none(), "内置网格不绑");
}

/// 重复调用（场景热重载）不能把脚本已经调过的权重冲回 0。
#[test]
fn rebinding_keeps_existing_weights() {
    let mut scene = scene_of(r#"[ { "name": "Face", "mesh": "asset:face.glb#0" } ]"#);
    let face = scene.find_id("Face").expect("Face 存在");
    let count = |mesh| match mesh {
        MeshRef::Asset { .. } => Some(2),
        _ => None,
    };

    assert_eq!(attach_morph_bindings_with(&mut scene, count), 1);
    assert!(scene.set_morph_weight(face, 1, 0.75), "已绑节点可写权重");

    // 热重载后再走一遍：已绑定的节点跳过，权重保持
    assert_eq!(attach_morph_bindings_with(&mut scene, count), 0);
    let morph = scene.morph_of(face).expect("绑定仍在");
    assert_eq!(morph.weights[1], 0.75);
}

/// 形状数超过引擎上限时按上限截断，不能让 `MorphInstance` 记出越界数量。
#[test]
fn target_count_is_clamped_to_engine_limit() {
    let mut scene = scene_of(r#"[ { "name": "Face", "mesh": "asset:face.glb#0" } ]"#);
    let face = scene.find_id("Face").expect("Face 存在");
    attach_morph_bindings_with(&mut scene, |_| Some(64));
    assert_eq!(
        scene.morph_of(face).map(|m| m.target_count),
        Some(r3e_core::MAX_MORPH_TARGETS)
    );
}
