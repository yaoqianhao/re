//! 动画实例 / 交叉淡入 / 图层基础（原 `anim_tests` 前半，11 条）。

use super::tests_anim_support::*;
use super::*;
use crate::anim::{AnimClip, AnimInstance, AnimKey, AnimTrack};
use glam::Quat;

fn clip_with_move() -> AnimClip {
    let track = AnimTrack {
        node_path: "root/arm".to_string(),
        node_index: 1,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::new(0.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: 1.0,
                position: Vec3::new(10.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ],
    };
    AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![track],
    }
}

#[test]
fn instance_advances_and_writes_node_transform() {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);

    scene.update_animations(0.25);
    let p = scene.get(id).unwrap().transform.position;
    assert!((p.x - 2.5).abs() < 1e-4, "0.25s 应插值到 x=2.5，实际 {p:?}");

    // 循环：1.0s 后回到 0，再走 0.5s → x=5
    scene.update_animations(0.75);
    scene.update_animations(0.5);
    let p = scene.get(id).unwrap().transform.position;
    assert!((p.x - 5.0).abs() < 1e-4, "循环后 x 应为 5，实际 {p:?}");
}

#[test]
fn stop_keeps_pose_and_speed_scales_time() {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    inst.looping = false;
    scene.add_anim_instance(inst);

    assert!(scene.set_anim_speed("demo/walk", 2.0));
    scene.update_animations(0.25); // 0.5s 处
    assert!((scene.anim_time("demo/walk").unwrap() - 0.5).abs() < 1e-4);

    assert!(scene.stop_anim("demo/walk"));
    scene.update_animations(0.5);
    let p = scene.get(id).unwrap().transform.position;
    assert!((p.x - 5.0).abs() < 1e-4, "暂停后姿态不应变化，实际 {p:?}");
}

#[test]
fn non_looping_stops_at_end() {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    inst.looping = false;
    scene.add_anim_instance(inst);

    scene.update_animations(3.0);
    let node = scene.get(id).unwrap();
    assert!((node.transform.position.x - 10.0).abs() < 1e-4);
    assert!(!scene.anim_instance("demo/walk").unwrap().playing);
}

#[test]
fn switching_clip_rebinds_tracks() {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();

    // 两个片段共用同一节点（下标 1 / 路径 root/arm），但终点位移不同
    let mut run = clip_with_move();
    run.name = "run".to_string();
    run.tracks[0].keys[1].position = Vec3::new(20.0, 0.0, 0.0);

    let walk = clip_with_move();
    scene.register_anim_clip("demo/walk", std::sync::Arc::new(walk.clone()));
    scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);

    assert_eq!(scene.anim_clip_names("demo"), vec!["run", "walk"]);
    assert!(scene.set_anim_clip("demo/walk", "run"), "应能切到 run");
    let inst = scene.anim_instance("demo/walk").unwrap();
    assert_eq!(inst.clip.name, "run");
    assert_eq!(inst.bindings.len(), 1, "切片段后应按节点下标重新绑定轨道");

    // 从 0 播到 0.5s（片段 1s 循环，直接播 1s 会绕回 0），x 应在 run 的中点 10
    scene.update_animations(0.5);
    let p = scene.get(id).unwrap().transform.position;
    assert!(
        (p.x - 10.0).abs() < 1e-4,
        "切片段后应采样新片段，实际 {p:?}"
    );
}

/// walk（x: 0→10）与 run（x: 0→20）共用同一节点，用于交叉淡入断言。
fn fade_scene() -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();

    let walk = clip_with_move();
    let mut run = clip_with_move();
    run.name = "run".to_string();
    run.tracks[0].keys[1].position = Vec3::new(20.0, 0.0, 0.0);

    scene.register_anim_clip("demo/walk", std::sync::Arc::new(walk.clone()));
    scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);
    (scene, id)
}

#[test]
fn cross_fade_blends_two_clips_then_settles() {
    let (mut scene, id) = fade_scene();
    scene.update_animations(0.25);
    assert!((scene.get(id).unwrap().transform.position.x - 2.5).abs() < 1e-4);

    // 过渡 0.4s：半程时新旧姿态各占一半权重
    assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.4));
    assert_eq!(scene.anim_fade_source("demo/walk"), "walk");
    assert!((scene.anim_fade_weight("demo/walk").unwrap() - 0.0).abs() < 1e-4);

    scene.update_animations(0.2);
    // 新 clip run 时间 0.2 → x=4；旧 clip walk 继续走到 0.45 → x=4.5；权重 smoothstep(0.5)=0.5
    let x = scene.get(id).unwrap().transform.position.x;
    assert!((x - 4.25).abs() < 1e-3, "半程应混合到 x=4.25，实际 {x}");

    scene.update_animations(0.3);
    // 过渡走完：只剩 run，时间 0.5 → x=10
    let x = scene.get(id).unwrap().transform.position.x;
    assert!((x - 10.0).abs() < 1e-3, "过渡结束后应等于新片段，实际 {x}");
    assert_eq!(scene.anim_fade_source("demo/walk"), "");
    assert!((scene.anim_fade_weight("demo/walk").unwrap() - 1.0).abs() < 1e-4);
}

#[test]
fn cross_fade_zero_duration_is_hard_cut() {
    let (mut scene, id) = fade_scene();
    scene.update_animations(0.25);

    assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.0));
    assert_eq!(
        scene.anim_fade_source("demo/walk"),
        "",
        "0 时长不应留下过渡"
    );
    scene.update_animations(0.2);
    let x = scene.get(id).unwrap().transform.position.x;
    assert!((x - 4.0).abs() < 1e-4, "硬切后直接采样 run，实际 {x}");
}

#[test]
fn cross_fade_keeps_old_track_when_new_clip_lacks_it() {
    let (mut scene, id) = fade_scene();
    // 新片段只有另一条轨道（root/leg），因此 arm 在过渡期间只能由旧片段驱动
    let mut run = clip_with_move();
    run.name = "run".to_string();
    // 换一条轨道（下标与路径都不同），确保 arm 的绑定在新片段里找不到对应轨道
    run.tracks[0].node_index = 2;
    run.tracks[0].node_path = "root/leg".to_string();
    scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

    scene.update_animations(0.25);
    assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.4));
    scene.update_animations(0.2);
    let x = scene.get(id).unwrap().transform.position.x;
    assert!(
        (x - 4.5).abs() < 1e-3,
        "缺轨道时应由旧片段驱动到 walk(0.45)=4.5，实际 {x}"
    );

    // 过渡结束后不再有轨道驱动，节点停在旧片段最后写入的姿态
    scene.update_animations(0.3);
    let x = scene.get(id).unwrap().transform.position.x;
    assert!((x - 4.5).abs() < 1e-3, "过渡结束后应停在末姿态，实际 {x}");
}

#[test]
fn dead_node_bindings_are_pruned() {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
    inst.bindings = vec![crate::anim::AnimBinding {
        node: id,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);

    scene.despawn("arm");
    scene.prune_anim_bindings();
    assert_eq!(scene.anim_instance("demo/walk").unwrap().bindings.len(), 0);
    // 没有绑定也不该 panic
    scene.update_animations(0.1);
}

#[test]
fn override_layer_replaces_masked_bone_only() {
    let (mut scene, arm, leg) = layer_scene();
    let atk = atk_clip();
    scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));

    let layer =
        AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0).with_mask(BoneMask::only(["arm"]));
    assert!(scene.add_anim_layer("demo/walk", layer));

    scene.update_animations(0.5);
    let arm_x = scene.get(arm).unwrap().transform.position.x;
    let leg_x = scene.get(leg).unwrap().transform.position.x;
    assert!(
        (arm_x - 50.0).abs() < 1e-4,
        "遮罩内骨骼应取层姿态，实际 {arm_x}"
    );
    assert!(
        (leg_x - 5.0).abs() < 1e-4,
        "遮罩外骨骼保持主片段，实际 {leg_x}"
    );
}

#[test]
fn additive_layer_stacks_delta_on_base() {
    let (mut scene, arm, _leg) = layer_scene();
    let bob = AnimClip {
        name: "bob".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 2.0))],
    };
    let layer = AnimLayer::new("bob", std::sync::Arc::new(bob), 0.5).with_mode(LayerMode::Additive);
    assert!(scene.add_anim_layer("demo/walk", layer));

    scene.update_animations(0.5);
    let p = scene.get(arm).unwrap().transform.position;
    // 主片段 x=5；加法层在 0.5s 的相对增量是 (0,0,1)，按 0.5 权重叠成 z=0.5
    assert!((p.x - 5.0).abs() < 1e-4, "{p:?}");
    assert!((p.z - 0.5).abs() < 1e-4, "加法层应叠加相对增量，实际 {p:?}");
}

#[test]
fn additive_layer_multiclip_uses_delta_semantics() {
    // 扩展五十：加法层多路份额合的是「增量」，不是「姿态平均」。
    // A：arm 绕 X 0 → 90°（增量 90°）；B：参考与姿态都绕 Y 80°（增量 0）。
    // 采样点 0.5 s 时 A 自身增量已是 45°，再乘 0.5 份额 → 22.5° 绕 X；
    // B 增量恒为零，不该把 A 的增量稀释成混轴姿态（覆盖路径会）。
    let rot_track = |deg: f32| AnimTrack {
        node_path: "root/arm".to_string(),
        node_index: 1,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: 1.0,
                position: Vec3::ZERO,
                rotation: Quat::from_rotation_x(deg.to_radians()),
                scale: Vec3::ONE,
            },
        ],
    };
    let hold = Quat::from_rotation_y(80f32.to_radians());
    let hold_track = AnimTrack {
        node_path: "root/arm".to_string(),
        node_index: 1,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::ZERO,
                rotation: hold,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: 1.0,
                position: Vec3::ZERO,
                rotation: hold,
                scale: Vec3::ONE,
            },
        ],
    };

    let (mut scene, arm, _leg) = layer_scene();
    let a = AnimClip {
        name: "a".to_string(),
        duration: 1.0,
        tracks: vec![rot_track(90.0)],
    };
    let b = AnimClip {
        name: "b".to_string(),
        duration: 1.0,
        tracks: vec![hold_track],
    };
    let mut layer =
        AnimLayer::new("add", std::sync::Arc::new(a), 1.0).with_mode(LayerMode::Additive);
    assert!(layer.add_clip(std::sync::Arc::new(b), 1.0));
    // 用 1D 混合空间把两路份额拉到 0.5 / 0.5（不手写权重，走真实驱动链路）
    let mut space = crate::blend_space::BlendSpace::new_1d("w");
    assert!(space.add_sample("a", [0.0, 0.0]));
    assert!(space.add_sample("b", [1.0, 0.0]));
    space.set_value_1d(0.5);
    layer.blend_space = Some(space);
    assert!(scene.add_anim_layer("demo/walk", layer));

    scene.update_animations(0.5);
    let t = scene.get(arm).unwrap().transform;
    assert!(
        (t.position.x - 5.0).abs() < 1e-4,
        "主片段位置不受影响：{t:?}"
    );
    let want = Quat::from_rotation_x(22.5f32.to_radians()) * Vec3::Z;
    let got = t.rotation * Vec3::Z;
    assert!(
        (got - want).length() < 1e-3,
        "加法层多路应合成 22.5° 绕 X 的增量，实际 {got:?} 应为 {want:?}"
    );
}
