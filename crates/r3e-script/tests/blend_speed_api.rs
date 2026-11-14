//! 脚本层「速率随样本插值」绑定的行为测试（扩展五十二）。
//!
//! core 单测证明了加权平均本身对，这里补三件只有真跑 Rhai 才能确认的事：
//! ① `set_anim_layer_blend_sample_speed` 的返回值语义（有样本才 `true`，否则不静默改）；
//! ② 变速真的进了层的时间推进——同样的 `dt` 走出两倍 `dt` 的时间；
//! ③ `anim_layer_blend` 回显只在**有变速样本**时追加 `spd=`，全原速时逐字符不变。

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

fn line_track(index: u32, path: &str, end: Vec3) -> AnimTrack {
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
                time: 1.0,
                position: end,
                rotation: r3e_core::Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ],
    }
}

fn fixture() -> Fixture {
    let mut scene = Scene::new();
    let arm = scene
        .spawn(ObjectDesc {
            name: "arm".to_string(),
            ..Default::default()
        })
        .unwrap();

    let walk = walk_clip("walk", 4.0);
    let mut inst = AnimInstance::new(INST, Arc::new(walk.clone()));
    inst.bindings = vec![AnimBinding {
        node: arm,
        node_index: 1,
        path: "root/arm".to_string(),
        track: 0,
    }];
    scene.add_anim_instance(inst);
    // 注册表按「实例前缀/片段名」存：demo/walk 是锚定片段，demo/run 是另外可切换的
    scene.register_anim_clip("demo/walk", Arc::new(walk));
    scene.register_anim_clip("demo/run", Arc::new(walk_clip("run", 4.0)));

    let scene = Rc::new(RefCell::new(scene));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(r3e_core::InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

fn walk_clip(name: &str, duration: f32) -> AnimClip {
    AnimClip {
        name: name.to_string(),
        duration,
        tracks: vec![line_track(1, "root/arm", Vec3::new(20.0, 0.0, 0.0))],
    }
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
            .as_float()
            .unwrap_or_else(|_| panic!("`{src}` 应返回浮点"))
    }

    fn string_of(&mut self, src: &str) -> String {
        self.eval(src)
            .into_string()
            .unwrap_or_else(|_| panic!("`{src}` 应返回字符串"))
    }

    fn advance(&self, dt: f32) {
        self.scene.borrow_mut().update_animations(dt);
    }

    /// 一维速度空间：walk@0 / run@1，两路片段都已在层里。
    fn with_1d_space(&mut self) {
        assert!(self.bool_of(r#"add_anim_layer("demo/walk", "move", "walk", 1.0)"#));
        assert!(self.bool_of(r#"add_anim_layer_clip("demo/walk", "move", "run", 0.0)"#));
        assert!(self.bool_of(r#"set_anim_layer_blend_1d("demo/walk", "move", "speed")"#));
        assert!(
            self.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "walk", 0.0, 0.0)"#)
        );
        assert!(
            self.bool_of(r#"add_anim_layer_blend_sample("demo/walk", "move", "run", 1.0, 0.0)"#)
        );
    }
}

#[test]
fn script_sample_speed_rejects_missing_sample() {
    let mut f = fixture();
    f.with_1d_space();

    // 空间里没有 idle 样本：改速必须落空（false），且倍率仍是最中性的 1.0
    assert!(!f.bool_of(r#"set_anim_layer_blend_sample_speed("demo/walk", "move", "idle", 2.0)"#));
    assert!((f.f64_of(r#"anim_layer_blend_speed("demo/walk", "move")"#) - 1.0).abs() < 1e-9);

    // 层 / 实例不存在时也走中性值，探针不用先判存在
    assert!((f.f64_of(r#"anim_layer_blend_speed("demo/none", "move")"#) - 1.0).abs() < 1e-9);
    assert!(!f.bool_of(r#"set_anim_layer_blend_sample_speed("demo/none", "move", "run", 2.0)"#));
}

#[test]
fn script_sample_speed_drives_layer_time() {
    let mut f = fixture();
    f.with_1d_space();

    // 参数推到 run 端点：这一层的推进完全由 run 的样本速率决定
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 1.0, 0.0)"#));

    // 未变速：0.1s 的 dt 走出 0.1s 的片段时间
    f.advance(0.1);
    let base = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!((base - 0.1).abs() < 1e-4, "未变速时应走 0.1，实际 {base}");

    // 打开 run 的 2 倍速：同样的 0.1s 走出 0.2s（累计 0.3）
    assert!(f.bool_of(r#"set_anim_layer_blend_sample_speed("demo/walk", "move", "run", 2.0)"#));
    assert!((f.f64_of(r#"anim_layer_blend_speed("demo/walk", "move")"#) - 2.0).abs() < 1e-6);
    f.advance(0.1);
    let fast = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!((fast - 0.3).abs() < 1e-4, "2 倍速应累计到 0.3，实际 {fast}");

    // 参数退回 walk 端点：run 权重归零，倍率回到 1.0，节奏立刻恢复正常
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 0.0, 0.0)"#));
    assert!((f.f64_of(r#"anim_layer_blend_speed("demo/walk", "move")"#) - 1.0).abs() < 1e-9);
    f.advance(0.1);
    let back = f.f64_of(r#"anim_layer_clip_time("demo/walk", "move", "run")"#);
    assert!(
        (back - 0.4).abs() < 1e-4,
        "回到 walk 后应只走 0.1，实际 {back}"
    );
}

#[test]
fn script_blend_line_reports_speed_only_when_varied() {
    let mut f = fixture();
    f.with_1d_space();

    // 全原速：回显逐字符与扩展五十一之前一致（不得凭空多出 spd=）
    let plain = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(!plain.contains("spd="), "未变速时不该有 spd=: {plain}");

    // 变速之后追加 spd=，数值是当前按权重插出来的倍率
    assert!(f.bool_of(r#"set_anim_layer_blend_sample_speed("demo/walk", "move", "run", 2.0)"#));
    assert!(f.bool_of(r#"set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0)"#));
    let line = f.string_of(r#"anim_layer_blend("demo/walk", "move")"#);
    assert!(line.contains("spd=1.50"), "半程应报 1.50 倍: {line}");
}
