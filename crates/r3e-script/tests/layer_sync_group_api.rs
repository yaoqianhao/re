//! 脚本层相位同步组绑定的行为测试（扩展四十七）。
//!
//! `r3e-core` 的单测已经验证了「偏移折进 [0,1)」「跨层取源相位」这两条算术，这里补的是
//! 从脚本侧真正拨动之后骨骼怎么动：
//! ① 每路片段加相位偏移后只挪这一路的落点，其余路不受影响；
//! ② 偏移归零 / 非法片段名不改状态（老口径逐位不变）；
//! ③ 跨层对齐：上层跟随下层相位后采样落点跟着变，但本层相位（画标尺用）不动；
//! ④ 自引用 / 源层不存在 / 成环一律拒，空串清除后跟回自己；
//! ⑤ 实例或层不存在时返回中性值，不 panic。

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

/// arm：慢路 1.0 s 推到 100、快路 0.5 s 推到 100；leg 给「下半身」层用。
fn fixture() -> Fixture {
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

    let walk = Arc::new(AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 1.0)],
    });
    let run = Arc::new(AnimClip {
        name: "run".to_string(),
        duration: 0.5,
        tracks: vec![
            line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 0.5),
            line_track(2, "root/leg", Vec3::new(100.0, 0.0, 0.0), 0.5),
        ],
    });

    scene.register_anim_clip("demo/walk", walk.clone());
    scene.register_anim_clip("demo/run", run);

    let mut inst = AnimInstance::new(INST, walk.clone());
    inst.bindings = vec![AnimBinding {
        node: arm,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);
    let _ = leg;

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

/// 上半身层 `move`（走 1.0 s + 跑 0.5 s，遮罩 arm）+ 下半身层 `upper`（时间不推进，遮罩 leg）。
fn setup() -> Fixture {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "walk", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "run", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_mask("demo/walk", "move", "arm")"#));
    assert!(f.bool_of(r#"set_anim_layer_sync("demo/walk", "move", true)"#));

    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "upper", "run", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_mask("demo/walk", "upper", "leg")"#));
    assert!(f.bool_of(r#"set_anim_layer_speed("demo/walk", "upper", 0.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_sync("demo/walk", "upper", true)"#));
    f
}

#[test]
fn script_clip_phase_offset_shifts_only_that_path() {
    let mut f = setup();
    f.advance(0.25);
    // 锁相位下两路同为 25%
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "锁相位应 25，实际 {x}");

    // 给跑路加半步偏移：它走到 75%，平均被拉到 50
    assert!(f.bool_of(r#"set_anim_layer_clip_phase_offset("demo/walk", "move", "run", 0.5)"#));
    let read = f.f64_of(r#"anim_layer_clip_phase_offset("demo/walk", "move", "run")"#);
    assert!((read - 0.5).abs() < 1e-6, "回显偏移应 0.5，实际 {read}");
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 50.0).abs() < 1e-3, "错步应 50，实际 {x}");

    // 探针同时给出偏移与零漂移（期望相位跟着偏移走，不该被当成滑步）
    let probe = f.string_of(r#"anim_layer_sync_probe("demo/walk", "move")"#);
    assert!(probe.contains("off=run=0.500"), "探针偏移段不对: {probe}");
    assert!(probe.contains("drift=0.000"), "带偏移也不该有漂移: {probe}");
    assert!(
        probe.contains("run=0.375/0.500"),
        "快路应 0.375/0.5: {probe}"
    );

    // 归零后逐位回到 25
    assert!(f.bool_of(r#"set_anim_layer_clip_phase_offset("demo/walk", "move", "run", 0.0)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "归零应 25，实际 {x}");
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_phase_offset("demo/walk", "move", "run")"#),
        0.0
    );
}

#[test]
fn script_clip_phase_offset_rejects_unknown_target() {
    let mut f = setup();
    f.advance(0.25);
    // 片段名写错 / 层不存在：返回 false，且既有状态不变
    assert!(!f.bool_of(r#"set_anim_layer_clip_phase_offset("demo/walk", "move", "ghost", 0.5)"#));
    assert!(!f.bool_of(r#"set_anim_layer_clip_phase_offset("demo/walk", "nope", "run", 0.5)"#));
    assert!(!f.bool_of(r#"set_anim_layer_clip_phase_offset("nobody", "move", "run", 0.5)"#));
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_phase_offset("demo/walk", "move", "ghost")"#),
        0.0
    );
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "被拒后落点不该变，实际 {x}");
}

#[test]
fn script_sync_source_aligns_upper_and_lower() {
    let mut f = setup();
    f.advance(0.25);
    assert!((f.f64_of(r#"anim_layer_phase("demo/walk", "move")"#) - 0.25).abs() < 1e-4);

    // 上半身跟下半身（下半身时间不推进 → 相位 0）
    assert!(f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "move", "upper")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "move")"#),
        "upper"
    );
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!(x.abs() < 1e-3, "跨层对齐后应回到起点，实际 {x}");
    let synced = f.f64_of(r#"anim_layer_sync_phase("demo/walk", "move")"#);
    assert!(synced.abs() < 1e-4, "有效相位应 0，实际 {synced}");
    // 本层相位不动，画标尺仍读自己的
    let own = f.f64_of(r#"anim_layer_phase("demo/walk", "move")"#);
    assert!((own - 0.25).abs() < 1e-4, "本层相位应仍是 0.25，实际 {own}");

    let probe = f.string_of(r#"anim_layer_sync_probe("demo/walk", "move")"#);
    assert!(probe.contains("src=upper"), "探针源层段不对: {probe}");

    // 层名大小写不敏感
    assert!(f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "MOVE", "UPPER")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "move")"#),
        "UPPER"
    );

    // 空串清除 → 跟回自己
    assert!(f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "move", "")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "move")"#),
        ""
    );
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "清除后应回到 25，实际 {x}");
}

#[test]
fn script_sync_source_rejects_self_ghost_and_cycle() {
    let mut f = setup();
    f.advance(0.25);

    // 自引用 / 源层不存在 / 层不存在
    assert!(!f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "move", "move")"#));
    assert!(!f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "move", "nope")"#));
    assert!(!f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "nope", "upper")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "move")"#),
        ""
    );

    // 成环：move 已跟 upper 后，upper 反跟 move 必须拒
    assert!(f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "move", "upper")"#));
    assert!(!f.bool_of(r#"set_anim_layer_sync_source("demo/walk", "upper", "move")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "upper")"#),
        ""
    );
}

#[test]
fn script_sync_group_api_returns_neutral_values_for_missing_target() {
    let mut f = setup();
    // 实例 / 层不存在：中性值，不 panic
    assert!(!f.bool_of(r#"set_anim_layer_sync_source("nobody", "move", "upper")"#));
    assert_eq!(
        f.string_of(r#"anim_layer_sync_source("demo/walk", "nope")"#),
        ""
    );
    assert_eq!(
        f.f64_of(r#"anim_layer_sync_phase("demo/walk", "nope")"#),
        0.0
    );
    assert_eq!(
        f.f64_of(r#"anim_layer_clip_phase_offset("nobody", "move", "run")"#),
        0.0
    );
    // 没设过源层时，有效相位就是本层相位
    f.advance(0.4);
    let own = f.f64_of(r#"anim_layer_phase("demo/walk", "upper")"#);
    let synced = f.f64_of(r#"anim_layer_sync_phase("demo/walk", "upper")"#);
    assert!((own - synced).abs() < 1e-6, "{own} vs {synced}");
}
