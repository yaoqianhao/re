//! `anim` 测试跨前后两半共用的构造 helper：轨道、图层场景、攻击片段。

use super::*;

use crate::anim::{AnimClip, AnimKey, AnimTrack};

/// 一条 0 → `end` 的直线轨道，用于混合树断言。
pub(super) fn line_track(index: u32, path: &str, end: Vec3) -> AnimTrack {
    AnimTrack {
        node_path: path.to_string(),
        node_index: index,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::ZERO,
                rotation: glam::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: 1.0,
                position: end,
                rotation: glam::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ],
    }
}

/// arm（下标 1）+ leg（下标 2）；主片段 walk 把两者都推到 x=10。
pub(super) fn layer_scene() -> (Scene, NodeId, NodeId) {
    let mut scene = Scene::new();
    let arm = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let leg = scene
        .spawn(ObjectDesc {
            name: "leg".to_string(),
            ..Default::default()
        })
        .unwrap();

    let walk = AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![
            line_track(1, "root/arm", Vec3::new(10.0, 0.0, 0.0)),
            line_track(2, "root/leg", Vec3::new(10.0, 0.0, 0.0)),
        ],
    };

    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
    inst.bindings = vec![
        crate::anim::AnimBinding {
            node: arm,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        },
        crate::anim::AnimBinding {
            node: leg,
            node_index: 2,
            path: "root/leg".to_string(),
            track: 1,
        },
    ];
    scene.add_anim_instance(inst);
    (scene, arm, leg)
}

pub(super) fn atk_clip() -> AnimClip {
    AnimClip {
        name: "atk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    }
}
