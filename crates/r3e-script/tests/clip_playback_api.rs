//! 脚本层「片段自带播放速率与循环标志」绑定的行为测试（扩展四十八）。
//!
//! `r3e-core` 的单测已经验证了「速率缩放 + 取模」「非循环钳末帧」这两条算术，这里补的是
//! 从脚本侧真正拨动之后骨骼怎么动：
//! ① 设了速率的只有那一路跑得快，同层其余路仍按层时间轴走（老口径逐位不变）；
//! ② 非循环到末帧停住，倒放往回走，速率 0 定格首帧；
//! ③ 清除后逐位回到老口径；
//! ④ 片段名写错 / 层不存在一律拒，回显给中性值，不 panic；
//! ⑤ 开了相位锁的层不看片段速率（要变速请调层 speed）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::{AnimClip, AnimKey, AnimTrack, ObjectDesc, Scene, Vec3};
use r3e_physics::Physics;
use r3e_script::ScriptHost;
use rhai::Dynamic;

const INST: &str = "demo/walk";

struct Fixture {
    scene: Rc<RefCell<Scene>>,
    host: ScriptHost,
}

fn line_track(index: u32, path: &str, end: Vec3, duration: f32) -> AnimTrack {
    AnimTrack {
        node_path: path.to_string(),
        node_index: index,
        keys: vec![
            AnimKey {
                time: 0.0,
                position: Vec3::ZERO,
                rotation: r3e_core::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimKey {
                time: duration,
                position: end,
                rotation: r3e_core::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ],
    }
}

/// arm：慢路 walk 1.0 s 推到 100、快路 run 0.5 s 推到 100。
fn fixture() -> Fixture {
    let mut scene = Scene::new();
    let arm = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();

    let walk = Arc::new(AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 1.0)],
    });
    let run = Arc::new(AnimClip {
        name: "run".to_string(),
        duration: 0.5,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 0.5)],
    });

    scene.register_anim_clip("demo/walk", walk.clone());
    scene.register_anim_clip("demo/run", run);

    let mut inst = AnimInstance::new(INST, walk);
    inst.bindings = vec![AnimBinding {
        node: arm,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);

    let scene = Rc::new(RefCell::new(scene));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(r3e_core::InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

impl Fixture {
    fn eval(&mut self, src: &str) -> Dynamic {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
    }

    fn bool_of(&mut self, src: &str) -> bool {
        self.eval(src)
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    fn f64_of(&mut self, src: &str) -> f64 {
        self.eval(src)
            .try_cast::<f64>()
            .unwrap_or_else(|| panic!("`{src}` 应返回数字"))
    }

    fn string_of(&mut self, src: &str) -> String {
        self.eval(src)
            .into_string()
            .unwrap_or_else(|_| panic!("`{src}` 应返回字符串"))
    }

    fn advance(&self, dt: f32) {
        self.scene.borrow_mut().update_animations(dt);
    }

    fn pos_x(&self, node: &str) -> f32 {
        let scene = self.scene.borrow();
        let id = scene.find_id(node).expect("节点应存在");
        scene.get(id).unwrap().transform.position.x
    }
}

/// 层 `move`：walk（1.0 s）+ run（0.5 s）等权混合，遮罩 arm，不锁相位。
fn setup() -> Fixture {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "walk", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "run", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_mask("demo/walk", "move", "arm")"#));
    f
}

#[test]
fn script_clip_playback_accelerates_only_that_path() {
    let mut f = setup();
    f.advance(0.25);
    // 老口径：两路都吃 0.25 s 层时间 → walk 25 / run 50，平均 37.5
    let x = f.pos_x("arm");
    assert!((x - 37.5).abs() < 1e-3, "老口径应 37.5，实际 {x}");
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_speed("demo/walk", "move", "run")"#),
        -1.0
    );
    assert!(f.bool_of(r#"anim_layer_clip_looping("demo/walk", "move", "run")"#));

    // run 改 2 倍速循环：0.25 s 层时间 → 采到 0.5 s，取模回 0（末帧接首帧），平均 12.5
    assert!(f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "run", 2.0, true)"#));
    assert!((f.f64_of(r#"anim_layer_clip_speed("demo/walk", "move", "run")"#) - 2.0).abs() < 1e-6);
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 12.5).abs() < 1e-3, "2 倍速应 12.5，实际 {x}");

    // 采到的时间：run 被折算，walk 一动不动
    assert!((f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#) - 0.0).abs() < 1e-6);
    assert!((f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "walk")"#) - 0.25).abs() < 1e-6);

    // 快照行带 pb= 段，探针也一样
    let report = f.string_of(r#"anim_layer_info("demo/walk", "move")"#);
    assert!(report.contains("pb=run=2.00"), "快照应带播放参数: {report}");
    let probe = f.string_of(r#"anim_layer_sync_probe("demo/walk", "move")"#);
    assert!(probe.contains("pb=run=2.00"), "探针应带播放参数: {probe}");
    assert!(
        probe.contains("drift=0.000"),
        "自带速率不该被报成滑步: {probe}"
    );

    // 清除后逐位回到 37.5
    assert!(f.bool_of(r#"clear_anim_layer_clip_playback("demo/walk", "move", "run")"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 37.5).abs() < 1e-3, "清除后应回 37.5，实际 {x}");
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_speed("demo/walk", "move", "run")"#),
        -1.0
    );
}

#[test]
fn script_clip_playback_handles_once_reverse_and_freeze() {
    let mut f = setup();
    // run 不循环：层时间 0.75 时本应采到 0.75（超时长），钳到末帧 0.5
    assert!(f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "run", 1.0, false)"#));
    assert!(!f.bool_of(r#"anim_layer_clip_looping("demo/walk", "move", "run")"#));
    f.advance(0.75);
    let t = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!((t - 0.5).abs() < 1e-6, "非循环应停在末帧 0.5，实际 {t}");

    // 倒放：速率 -1 循环，0.25 s 层时间折到 0.25/0.5 的倒数位置 → rem_euclid(-0.25, 0.5) = 0.25
    let mut f = setup();
    assert!(f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "run", -1.0, true)"#));
    f.advance(0.25);
    let t = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!((t - 0.25).abs() < 1e-6, "倒放应折到 0.25，实际 {t}");

    // 速率 0：定格首帧，层时间再走也不动
    let mut f = setup();
    assert!(f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "run", 0.0, true)"#));
    f.advance(0.6);
    let t = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!((t - 0.0).abs() < 1e-6, "速率 0 应定格首帧，实际 {t}");
}

#[test]
fn script_clip_playback_rejects_unknown_target() {
    let mut f = setup();
    f.advance(0.25);
    // 片段名写错 / 层不存在 / 实例不存在：返回 false，既有落点不变
    assert!(!f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "ghost", 2.0, true)"#));
    assert!(!f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "nope", "run", 2.0, true)"#));
    assert!(!f.bool_of(r#"set_anim_layer_clip_playback("nobody", "move", "run", 2.0, true)"#));
    assert!(!f.bool_of(r#"clear_anim_layer_clip_playback("demo/walk", "move", "ghost")"#));
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_speed("demo/walk", "move", "ghost")"#),
        -1.0
    );
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 37.5).abs() < 1e-3, "被拒后落点不该变，实际 {x}");
}

#[test]
fn script_clip_playback_ignored_under_phase_lock() {
    // 相位锁层的每路时间由相位决定，片段速率不参与
    let mut f = setup();
    assert!(f.bool_of(r#"set_anim_layer_sync("demo/walk", "move", true)"#));
    f.advance(0.25);
    let before = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!(
        (before - 0.125).abs() < 1e-6,
        "锁相位时应 0.125，实际 {before}"
    );

    assert!(f.bool_of(r#"set_anim_layer_clip_playback("demo/walk", "move", "run", 2.0, true)"#));
    f.advance(0.0);
    let after = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!(
        (after - before).abs() < 1e-6,
        "锁相位下片段速率不该生效，{before} -> {after}"
    );
}
