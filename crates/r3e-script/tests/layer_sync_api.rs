//! 脚本层相位同步绑定的行为测试（扩展四十六）。
//!
//! `r3e-core` 的单测验证了「相位缩放后各片段同相位」这条算术，这里补的是从脚本侧
//! 真正拨动开关之后骨骼怎么动：
//! ① 默认不锁相位，长短两路同一时刻相位不同（老口径零变化）；
//! ② 锁相位后同一时刻两路同相位，姿态落点按层相位重算；
//! ③ 名字设策略（大小写不敏感 / 别名）与非法名拒绝；
//! ④ 回显与探针（`anim_layer_sync` / `anim_layer_phase` / `anim_layer_sync_probe`）；
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

/// arm：慢路 1.0 s 推到 100、快路 0.5 s 推到 100；leg 只挂在主实例上。
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

    let walk = AnimClip {
        name: "walk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 1.0)],
    };
    let run = AnimClip {
        name: "run".to_string(),
        duration: 0.5,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0), 0.5)],
    };

    let walk = Arc::new(walk);
    scene.register_anim_clip("demo/walk", walk.clone());
    scene.register_anim_clip("demo/run", Arc::new(run));

    let mut inst = AnimInstance::new(INST, walk.clone());
    inst.bindings = vec![AnimBinding {
        node: arm,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);
    // 主实例的 leg 节点留在场景里但不参与绑定，只为让骨架名看起来完整。
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

/// 建一个「走 + 跑」双层片段的层，返回 fixture。两路等权，遮罩只开 arm。
fn setup() -> Fixture {
    let mut f = fixture();
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "move", "walk", 1.0)"#));
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "run", 1.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_mask("demo/walk", "move", "arm")"#));
    f
}

#[test]
fn script_sync_off_keeps_phase_drift() {
    let mut f = setup();
    // 0.25 s：慢路 25%、快路 50%，等权 → 37.5
    f.advance(0.25);
    let x = f.pos_x("arm");
    assert!((x - 37.5).abs() < 1e-3, "默认应留相位差，实际 {x}");
    assert!(!f.bool_of(r#"anim_layer_sync("demo/walk", "move")"#));

    // 探针里能直接看到漂移量：|0.25 - 0.50| = 0.25
    let probe = f.string_of(r#"anim_layer_sync_probe("demo/walk", "move")"#);
    assert!(probe.contains("sync=off"), "探针策略段不对: {probe}");
    assert!(probe.contains("drift=0.250"), "探针漂移量不对: {probe}");
}

#[test]
fn script_sync_phase_removes_phase_drift() {
    let mut f = setup();
    f.advance(0.25);
    assert!((f.pos_x("arm") - 37.5).abs() < 1e-3);

    // 开锁：同相位 → 两路都 25%
    assert!(f.bool_of(r#"set_anim_layer_sync("demo/walk", "move", true)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 25.0).abs() < 1e-3, "锁相位应 25，实际 {x}");
    let phase = f.f64_of(r#"anim_layer_phase("demo/walk", "move")"#);
    assert!((phase - 0.25).abs() < 1e-4, "相位应 0.25，实际 {phase}");

    let probe = f.string_of(r#"anim_layer_sync_probe("demo/walk", "move")"#);
    assert!(probe.contains("sync=phase"), "探针策略段不对: {probe}");
    assert!(probe.contains("drift=0.000"), "锁相位后漂移应归零: {probe}");
    assert!(
        probe.contains("run=0.125/0.500"),
        "快路应缩到 0.125/0.5: {probe}"
    );

    // 推到层时间 0.6：锁相位下两路都 60%
    f.advance(0.35);
    let x = f.pos_x("arm");
    assert!((x - 60.0).abs() < 1e-3, "锁相位应 60，实际 {x}");

    // 关锁：快路停在末帧 → 回到 (60+100)/2 = 80
    assert!(f.bool_of(r#"set_anim_layer_sync("demo/walk", "move", false)"#));
    f.advance(0.0);
    let x = f.pos_x("arm");
    assert!((x - 80.0).abs() < 1e-3, "关锁应 80，实际 {x}");
}

#[test]
fn script_sync_mode_by_name_and_rejects_unknown() {
    let mut f = setup();
    f.advance(0.4);

    // 规范名 + 大小写无关
    assert!(f.bool_of(r#"set_anim_layer_sync_mode("demo/walk", "move", "PHASE")"#));
    assert!(f.bool_of(r#"anim_layer_sync("demo/walk", "move")"#));
    // 别名 sync / none
    assert!(f.bool_of(r#"set_anim_layer_sync_mode("demo/walk", "move", "sync")"#));
    assert!(f.bool_of(r#"set_anim_layer_sync_mode("demo/walk", "move", "none")"#));
    assert!(!f.bool_of(r#"anim_layer_sync("demo/walk", "move")"#));

    // 非法名：返回 false，且策略保持原样（此时是 off）
    assert!(!f.bool_of(r#"set_anim_layer_sync_mode("demo/walk", "move", "warp")"#));
    assert!(!f.bool_of(r#"anim_layer_sync("demo/walk", "move")"#));
}

#[test]
fn script_sync_api_returns_neutral_values_for_missing_target() {
    let mut f = setup();
    // 层不存在 / 实例不存在：一律中性值，不 panic
    assert!(!f.bool_of(r#"set_anim_layer_sync("demo/walk", "nope", true)"#));
    assert!(!f.bool_of(r#"set_anim_layer_sync_mode("nobody", "move", "phase")"#));
    assert!(!f.bool_of(r#"anim_layer_sync("demo/walk", "nope")"#));
    assert_eq!(f.f64_of(r#"anim_layer_phase("demo/walk", "nope")"#), 0.0);
    assert_eq!(
        f.string_of(r#"anim_layer_sync_probe("nobody", "move")"#),
        ""
    );
}
