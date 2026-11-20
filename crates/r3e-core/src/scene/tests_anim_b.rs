//! 图层淡入、混合空间、同步与容量（原 `anim_tests` 后半，12 条）。

use super::tests_anim_support::*;
use super::*;
use crate::anim::AnimClip;

#[test]
fn layer_fade_in_scales_contribution() {
    let (mut scene, arm, _leg) = layer_scene();
    let atk = atk_clip();
    scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
    let layer =
        AnimLayer::new("upper", std::sync::Arc::new(atk), 0.0).with_mask(BoneMask::only(["arm"]));
    assert!(scene.add_anim_layer("demo/walk", layer));

    scene.update_animations(0.5);
    let base_x = scene.get(arm).unwrap().transform.position.x;
    assert!(
        (base_x - 5.0).abs() < 1e-4,
        "权重 0 时层不生效，实际 {base_x}"
    );

    assert!(scene.fade_anim_layer("demo/walk", "upper", 1.0, 0.4));
    scene.update_animations(0.2);
    let mid = scene.get(arm).unwrap().transform.position.x;
    assert!(
        mid > 5.0 && mid < 70.0,
        "淡入半程应落在主姿态与层姿态之间，实际 {mid}"
    );

    scene.update_animations(0.2);
    // 时间累积 0.5 + 0.2 + 0.2 = 0.9s，层姿态 90 完全取代主片段
    let full = scene.get(arm).unwrap().transform.position.x;
    assert!(
        (full - 90.0).abs() < 1e-3,
        "淡入结束应完全取层姿态，实际 {full}"
    );
    assert_eq!(
        scene.anim_layer("demo/walk", "upper").unwrap().fade,
        None,
        "过渡完成后应清空"
    );
}

#[test]
fn layer_management_api_roundtrip() {
    let (mut scene, _arm, _leg) = layer_scene();
    let atk = atk_clip();
    let breathe = AnimClip {
        name: "breathe".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 1.0))],
    };
    scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
    scene.register_anim_clip("demo/breathe", std::sync::Arc::new(breathe));

    let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 0.5);
    assert!(scene.add_anim_layer("demo/walk", layer));
    assert_eq!(
        scene.anim_layer_names("demo/walk"),
        vec!["upper".to_string()]
    );
    assert_eq!(scene.anim_layer_count("demo/walk"), Some(1));

    assert!(scene.set_anim_layer_weight("demo/walk", "upper", 1.0));
    assert_eq!(
        scene.anim_layer_effective_weight("demo/walk", "upper"),
        Some(1.0)
    );
    assert!(scene.set_anim_layer_mode("demo/walk", "upper", LayerMode::Additive));
    assert_eq!(
        scene.anim_layer("demo/walk", "upper").unwrap().mode,
        LayerMode::Additive
    );
    assert!(scene.set_anim_layer_mask("demo/walk", "upper", BoneMask::only(["arm"])));
    assert_eq!(
        scene.anim_layer("demo/walk", "upper").unwrap().mask.include,
        vec!["arm".to_string()]
    );
    assert!(scene.set_anim_layer_speed("demo/walk", "upper", 2.0));

    assert!(scene.add_anim_layer_clip("demo/walk", "upper", "breathe", 0.5));
    assert_eq!(
        scene.anim_layer("demo/walk", "upper").unwrap().clips.len(),
        2
    );
    assert!(scene.set_anim_layer_clip_weight("demo/walk", "upper", "breathe", 2.0));
    assert!(scene.remove_anim_layer_clip("demo/walk", "upper", "breathe"));

    assert!(scene.set_anim_layer_clip("demo/walk", "upper", "atk"));
    assert_eq!(
        scene.anim_layer("demo/walk", "upper").unwrap().clips.len(),
        1
    );
    assert!(scene
        .anim_layer_report("demo/walk", "upper")
        .unwrap()
        .contains("upper"));

    assert!(scene.remove_anim_layer("demo/walk", "upper"));
    assert_eq!(scene.anim_layer_count("demo/walk"), Some(0));
    assert!(!scene.remove_anim_layer("demo/walk", "upper"));
    assert!(scene.anim_layer("demo/walk", "upper").is_none());
    assert!(scene.anim_layer_names("nope").is_empty());
}

// ---------- 1D / 2D 混合空间（扩展四十五） ----------

#[test]
fn blend_space_drives_layer_clip_weights() {
    let (mut scene, arm, _leg) = layer_scene();
    // still 恒 0；fast 是 0→100 的直线，0.5s 处为 50
    let still = std::sync::Arc::new(AnimClip {
        name: "still".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
    });
    let fast = std::sync::Arc::new(AnimClip {
        name: "fast".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    });
    scene.register_anim_clip("demo/still", still.clone());
    scene.register_anim_clip("demo/fast", fast.clone());

    let mut layer = AnimLayer::new("move", still.clone(), 1.0);
    assert!(layer.add_clip(fast.clone(), 0.0));
    assert!(scene.add_anim_layer("demo/walk", layer));

    // 没挂空间：照旧走手写权重（fast 权重 0 → arm 停在 0），零变化
    scene.update_animations(0.5);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!(x.abs() < 1e-4, "未挂空间时应走手写权重，实际 {x}");

    assert!(scene.set_anim_layer_blend_space_1d("demo/walk", "move", "speed"));
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "still", [0.0, 0.0]),
        Some(true)
    );
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "fast", [1.0, 0.0]),
        Some(true)
    );

    // 参数 0.5：两路各半 → 0*0.5 + 50*0.5 = 25
    assert_eq!(
        scene.set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0),
        Some(true)
    );
    scene.update_animations(0.0);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!((x - 25.0).abs() < 1e-3, "参数 0.5 应混合到 25，实际 {x}");

    // 参数推到底：fast 全额 → 50
    assert_eq!(
        scene.set_anim_layer_blend_value("demo/walk", "move", 1.0, 0.0),
        Some(true)
    );
    scene.update_animations(0.0);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!((x - 50.0).abs() < 1e-3, "参数 1.0 应取 fast，实际 {x}");
}

#[test]
fn blend_space_3d_drives_layer_clip_weights() {
    let (mut scene, arm, _leg) = layer_scene();
    let still = std::sync::Arc::new(AnimClip {
        name: "still".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
    });
    let fast = std::sync::Arc::new(AnimClip {
        name: "fast".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    });
    // 只为凑出四面体：这两个片段不进层，权重份额应被忽略。
    let up = std::sync::Arc::new(AnimClip {
        name: "up".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 0.0))],
    });
    let far = std::sync::Arc::new(AnimClip {
        name: "far".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 0.0))],
    });
    scene.register_anim_clip("demo/still", still.clone());
    scene.register_anim_clip("demo/fast", fast.clone());
    scene.register_anim_clip("demo/up", up);
    scene.register_anim_clip("demo/far", far);

    let mut layer = AnimLayer::new("move", still.clone(), 1.0);
    assert!(layer.add_clip(fast.clone(), 0.0));
    assert!(scene.add_anim_layer("demo/walk", layer));

    assert!(scene.set_anim_layer_blend_space_3d("demo/walk", "move", "speed", "turn", "slope"));
    assert_eq!(
        scene.anim_layer_blend_dim("demo/walk", "move"),
        Some("3d"),
        "挂的应是三维空间"
    );
    for (name, at) in [
        ("still", [0.0, 0.0, 0.0]),
        ("fast", [1.0, 0.0, 0.0]),
        ("up", [0.0, 1.0, 0.0]),
        ("far", [0.0, 0.0, 1.0]),
    ] {
        assert_eq!(
            scene.add_anim_layer_blend_sample_3d("demo/walk", "move", name, at),
            Some(true),
            "{name}"
        );
    }
    let brief = scene
        .anim_layer_blend_brief("demo/walk", "move")
        .expect("刚挂过");
    assert!(brief.contains("3d(speed,turn,slope)"), "{brief}");
    assert!(
        brief.contains("tet=1"),
        "四个不共面样本应建出一个四面体：{brief}"
    );

    // 棱中点 still-fast：各半 → 0*0.5 + 50*0.5 = 25
    assert_eq!(
        scene.set_anim_layer_blend_value_3d("demo/walk", "move", 0.5, 0.0, 0.0),
        Some(true)
    );
    scene.update_animations(0.5);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!((x - 25.0).abs() < 1e-3, "三维插值应到 25，实际 {x}");

    // 顶点：fast 全额 → 50（层时间停在 0.5s，避免跨过 1s 循环点）
    assert_eq!(
        scene.set_anim_layer_blend_value_3d("demo/walk", "move", 1.0, 0.0, 0.0),
        Some(true)
    );
    scene.update_animations(0.0);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!((x - 50.0).abs() < 1e-3, "顶点应取 fast，实际 {x}");

    // 二维入口在三维空间上仍可用（前两维），不该 panic 或丢权重
    assert_eq!(
        scene.set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0),
        Some(true)
    );
    scene.update_animations(0.0);
    let x = scene.get(arm).unwrap().transform.position.x;
    assert!((x - 25.0).abs() < 1e-3, "二维入口后应回到 25，实际 {x}");
}

#[test]
fn blend_space_api_reports_missing_layer_and_space() {
    let (mut scene, _arm, _leg) = layer_scene();
    let atk = atk_clip();
    scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));

    // 层不存在：整组 API 全是 None / false，不 panic
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "nope", "atk", [0.0, 0.0]),
        None
    );
    assert_eq!(
        scene.set_anim_layer_blend_value("demo/walk", "nope", 0.0, 0.0),
        None
    );
    assert!(scene.anim_layer_blend_brief("demo/walk", "nope").is_none());
    assert!(scene
        .anim_layer_blend_weights("demo/walk", "nope")
        .is_none());
    assert!(!scene.set_anim_layer_blend_space_1d("demo/walk", "nope", "speed"));
    assert!(!scene.clear_anim_layer_blend_space("demo/walk", "nope"));

    // 层存在但没挂空间：同样是 None
    let layer = AnimLayer::new("move", std::sync::Arc::new(atk), 1.0);
    assert!(scene.add_anim_layer("demo/walk", layer));
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
        None
    );
    assert_eq!(
        scene.set_anim_layer_blend_value("demo/walk", "move", 0.0, 0.0),
        None
    );
    assert!(scene.anim_layer_blend_brief("demo/walk", "move").is_none());
    assert!(!scene.clear_anim_layer_blend_space("demo/walk", "move"));

    // 挂上二维空间
    assert!(scene.set_anim_layer_blend_space_2d("demo/walk", "move", "mx", "my"));
    let brief = scene
        .anim_layer_blend_brief("demo/walk", "move")
        .expect("应已挂空间");
    assert!(brief.starts_with("2d(mx,my)"), "{brief}");
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
        Some(true)
    );
    // 重复位置被拒；片段名解析不到也被拒
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
        Some(false)
    );
    assert_eq!(
        scene.add_anim_layer_blend_sample("demo/walk", "move", "ghost", [1.0, 0.0]),
        Some(false)
    );
    assert_eq!(
        scene.anim_layer_blend_weights("demo/walk", "move"),
        Some(vec![("atk".to_string(), 1.0)])
    );
    // 摘掉空间后回到手写权重
    assert!(scene.clear_anim_layer_blend_space("demo/walk", "move"));
    assert!(scene.anim_layer_blend_brief("demo/walk", "move").is_none());
    scene.update_animations(0.5);
    assert!((scene.get(_arm).unwrap().transform.position.x - 50.0).abs() < 1e-3);
}

// ---------- 层内相位同步（扩展四十六） ----------

/// 「慢 1.0 s + 快 0.5 s」的层场景：两路都是 0→100 的直线，遮罩只开 arm。
fn sync_scene() -> (Scene, NodeId) {
    let (mut scene, arm, _leg) = layer_scene();
    let slow = std::sync::Arc::new(AnimClip {
        name: "slow".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    });
    let mut fast_track = line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0));
    fast_track.keys[1].time = 0.5;
    let fast = std::sync::Arc::new(AnimClip {
        name: "fast".to_string(),
        duration: 0.5,
        tracks: vec![fast_track],
    });

    let mut layer = AnimLayer::new("move", slow, 1.0).with_mask(BoneMask::only(["arm"]));
    assert!(layer.add_clip(fast, 1.0));
    assert!(scene.add_anim_layer("demo/walk", layer));
    (scene, arm)
}

#[test]
fn anim_layer_sync_removes_phase_drift() {
    let (mut scene, arm) = sync_scene();
    // 0.25 s：慢路 25%、快路 50%，等权平均 37.5 —— 两路步态已经错开
    scene.update_animations(0.25);
    let off = scene.get(arm).unwrap().transform.position.x;
    assert!((off - 37.5).abs() < 1e-3, "不锁相位应 37.5，实际 {off}");
    assert_eq!(scene.anim_layer_sync("demo/walk", "move"), Some(false));

    // 锁相位：同一时刻两路都走 25%，等权平均仍是 25%
    assert!(scene.set_anim_layer_sync("demo/walk", "move", true));
    assert_eq!(scene.anim_layer_sync("demo/walk", "move"), Some(true));
    scene.update_animations(0.0);
    let on = scene.get(arm).unwrap().transform.position.x;
    assert!((on - 25.0).abs() < 1e-3, "锁相位应 25，实际 {on}");

    // 推到层时间 0.6：锁相位时两路仍是 60%
    scene.update_animations(0.35);
    let on = scene.get(arm).unwrap().transform.position.x;
    assert!((on - 60.0).abs() < 1e-3, "锁相位应 60，实际 {on}");
    // 关掉相位锁：快路停在末帧(100)，混合值回到 80 —— 漂移重新出现
    assert!(scene.set_anim_layer_sync("demo/walk", "move", false));
    scene.update_animations(0.0);
    let off = scene.get(arm).unwrap().transform.position.x;
    assert!((off - 80.0).abs() < 1e-3, "不锁相位应 80，实际 {off}");
}

#[test]
fn anim_layer_sync_probe_and_missing_layer() {
    let (mut scene, _arm) = sync_scene();
    scene.update_animations(0.3);
    let probe = scene
        .anim_layer_sync_probe("demo/walk", "move")
        .expect("层存在");
    assert!(probe.contains("sync=off"), "{probe}");
    assert!(probe.contains("drift=0."), "{probe}");

    assert!(scene.set_anim_layer_sync_strategy("demo/walk", "move", LayerSync::Phase));
    let probe = scene
        .anim_layer_sync_probe("demo/walk", "move")
        .expect("层存在");
    assert!(probe.contains("sync=phase"), "{probe}");
    assert!(probe.contains("drift=0.000"), "{probe}");
    let phase = scene.anim_layer_phase("demo/walk", "move").expect("层存在");
    assert!((phase - 0.3).abs() < 1e-4, "相位应 0.3，实际 {phase}");

    // 实例 / 层不存在：一律 None / false，不 panic
    assert_eq!(scene.anim_layer_sync("demo/walk", "nope"), None);
    assert_eq!(scene.anim_layer_phase("nope", "move"), None);
    assert!(scene.anim_layer_sync_probe("demo/walk", "nope").is_none());
    assert!(!scene.set_anim_layer_sync("demo/walk", "nope", true));
    assert!(!scene.set_anim_layer_sync_strategy("nope", "move", LayerSync::Phase));
}

// ---------- 相位偏移量与跨层对齐（扩展四十七） ----------

#[test]
fn anim_layer_clip_phase_offset_shifts_single_path() {
    let (mut scene, arm) = sync_scene();
    scene.update_animations(0.25);
    assert!(scene.set_anim_layer_sync("demo/walk", "move", true));
    scene.update_animations(0.0);
    // 相位锁：两路同为 25%，平均值仍是 25
    assert!((scene.get(arm).unwrap().transform.position.x - 25.0).abs() < 1e-3);

    // 给快路加半步偏移：它走到 75%，两路平均被拉到 50 —— 步幅没变、只是错开半步
    assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "fast", 0.5));
    assert_eq!(
        scene.anim_layer_clip_phase_offset("demo/walk", "move", "fast"),
        Some(0.5)
    );
    scene.update_animations(0.0);
    let offset = scene.get(arm).unwrap().transform.position.x;
    assert!((offset - 50.0).abs() < 1e-3, "错步应 50，实际 {offset}");

    // 归零即回到逐位相同的 25
    assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "fast", 0.0));
    scene.update_animations(0.0);
    let back = scene.get(arm).unwrap().transform.position.x;
    assert!((back - 25.0).abs() < 1e-3, "归零应 25，实际 {back}");

    // 片段 / 层写错都不该改状态
    assert!(!scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "ghost", 0.5));
    assert!(!scene.set_anim_layer_clip_phase_offset("demo/walk", "nope", "fast", 0.5));
    assert_eq!(
        scene.anim_layer_clip_phase_offset("demo/walk", "move", "ghost"),
        None
    );
    // 空片段名作用于层内第一路；负偏移按循环等价折叠
    assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "", -0.25));
    let first = scene
        .anim_layer_clip_phase_offset("demo/walk", "move", "slow")
        .expect("第一路是 slow");
    assert!((first - 0.75).abs() < 1e-5, "{first}");
}

#[test]
fn anim_layer_sync_source_aligns_across_layers() {
    let (mut scene, arm) = sync_scene();
    // 旁观层：只驱动 leg、时间不推进（相位恒 0），充当「下半身」相位源
    let idle = std::sync::Arc::new(AnimClip {
        name: "idle".to_string(),
        duration: 1.0,
        tracks: vec![line_track(2, "root/leg", Vec3::new(100.0, 0.0, 0.0))],
    });
    let upper = AnimLayer::new("upper", idle, 1.0).with_mask(BoneMask::only(["leg"]));
    assert!(scene.add_anim_layer("demo/walk", upper));
    assert!(scene.set_anim_layer_speed("demo/walk", "upper", 0.0));
    assert!(scene.set_anim_layer_sync("demo/walk", "move", true));

    scene.update_animations(0.25);
    assert!((scene.anim_layer_sync_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
    assert!((scene.get(arm).unwrap().transform.position.x - 25.0).abs() < 1e-3);

    // 跨层对齐：move 跟随 upper（相位 0）→ 采样相位变 0，arm 回到起点
    assert!(scene.set_anim_layer_sync_source("demo/walk", "move", Some("upper")));
    assert_eq!(
        scene.anim_layer_sync_source("demo/walk", "move").as_deref(),
        Some("upper")
    );
    scene.update_animations(0.0);
    assert!(
        scene
            .anim_layer_sync_phase("demo/walk", "move")
            .unwrap()
            .abs()
            < 1e-4
    );
    let aligned = scene.get(arm).unwrap().transform.position.x;
    assert!(aligned.abs() < 1e-3, "对齐后应 0，实际 {aligned}");
    // 本层相位不受影响（编辑器画标尺仍读自己的）
    assert!((scene.anim_layer_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
    // 探针带 src 段
    let probe = scene
        .anim_layer_sync_probe("demo/walk", "move")
        .expect("层存在");
    assert!(probe.contains("src=upper"), "{probe}");
    assert!(probe.contains("drift=0.000"), "{probe}");

    // 自引用 / 源层不存在：直接拒
    assert!(!scene.set_anim_layer_sync_source("demo/walk", "move", Some("move")));
    assert!(!scene.set_anim_layer_sync_source("demo/walk", "move", Some("nope")));
    assert!(!scene.set_anim_layer_sync_source("demo/walk", "nope", Some("upper")));
    // 先让 move 跟 upper 时就能看出环：upper 尚未跟人，这里成功
    assert!(scene.set_anim_layer_sync_source("demo/walk", "move", Some("upper")));
    // 再让 upper 反跟 move 就成环，必须拒
    assert!(!scene.set_anim_layer_sync_source("demo/walk", "upper", Some("move")));

    // 清除后相位跟回自己
    assert!(scene.set_anim_layer_sync_source("demo/walk", "move", None));
    assert_eq!(
        scene.anim_layer_sync_source("demo/walk", "move").as_deref(),
        Some("")
    );
    assert!((scene.anim_layer_sync_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
    // 不存在的层没有相位可读
    assert!(scene.anim_layer_sync_phase("demo/walk", "nope").is_none());
    assert!(scene.anim_layer_sync_source("demo/walk", "nope").is_none());
}

#[test]
fn layer_capacity_is_bounded() {
    let (mut scene, _arm, _leg) = layer_scene();
    let atk = std::sync::Arc::new(atk_clip());
    for i in 0..MAX_ANIM_LAYERS {
        assert!(
            scene.add_anim_layer(
                "demo/walk",
                AnimLayer::new(format!("l{i}"), atk.clone(), 1.0)
            ),
            "第 {i} 层应能挂载"
        );
    }
    assert!(
        !scene.add_anim_layer("demo/walk", AnimLayer::new("overflow", atk.clone(), 1.0)),
        "超出上限应拒绝"
    );
    // 同名覆盖不受上限限制
    assert!(scene.add_anim_layer("demo/walk", AnimLayer::new("l0", atk, 0.5)));
    assert_eq!(scene.anim_layer_count("demo/walk"), Some(MAX_ANIM_LAYERS));
}

#[test]
fn layers_survive_main_clip_switch() {
    let (mut scene, arm, _leg) = layer_scene();
    let atk = atk_clip();
    let run = AnimClip {
        name: "run".to_string(),
        duration: 1.0,
        tracks: vec![
            line_track(1, "root/arm", Vec3::new(20.0, 0.0, 0.0)),
            line_track(2, "root/leg", Vec3::new(20.0, 0.0, 0.0)),
        ],
    };
    scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
    scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

    let layer =
        AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0).with_mask(BoneMask::only(["arm"]));
    assert!(scene.add_anim_layer("demo/walk", layer));

    assert!(scene.set_anim_clip("demo/walk", "run"));
    assert_eq!(
        scene.anim_layer_count("demo/walk"),
        Some(1),
        "换主片段不应丢弃层"
    );

    scene.update_animations(0.5);
    let arm_x = scene.get(arm).unwrap().transform.position.x;
    assert!(
        (arm_x - 50.0).abs() < 1e-4,
        "换主片段后层仍应生效，实际 {arm_x}"
    );
}

#[test]
fn clearing_layers_restores_base_pose() {
    let (mut scene, arm, _leg) = layer_scene();
    let atk = atk_clip();
    let layer =
        AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0).with_mask(BoneMask::only(["arm"]));
    assert!(scene.add_anim_layer("demo/walk", layer));
    scene.update_animations(0.5);
    assert!((scene.get(arm).unwrap().transform.position.x - 50.0).abs() < 1e-4);

    assert!(scene.clear_anim_layers("demo/walk"));
    assert!(!scene.clear_anim_layers("demo/walk"), "已空应返回 false");
    // 只重算不推进时间：清层后应回到主片段在 0.5s 的姿态
    scene.update_animations(0.0);
    let base_x = scene.get(arm).unwrap().transform.position.x;
    assert!(
        (base_x - 5.0).abs() < 1e-4,
        "清层后应回到主片段姿态，实际 {base_x}"
    );
}
