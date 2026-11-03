//! 脚本层动画混合树绑定的行为测试（扩展十八 A）。
//!
//! `scripts_compile` 只保证演示脚本能编译，管不到「脚本调用之后骨骼到底怎么动」。
//! 这里用 `ScriptHost::eval` 真跑一遍 Rhai，覆盖五件事：
//! ① 脚本挂的层真的参与合成（遮罩内取层姿态、遮罩外保留主片段）；
//! ② 遮罩字符串（`"arm,hand"` / `"!root"` / `""` / `"*"`）解析与回显闭环；
//! ③ 模式字符串经 `LayerMode::parse` 校验，写错不静默改成别的模式；
//! ④ 权重过渡（`fade_anim_layer`）从当前值起步并随时间推进；
//! ⑤ 层数上限、删除 / 清空、以及实例不存在时的中性返回值。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::{AnimClip, AnimKey, AnimTrack, InputState, ObjectDesc, Scene, Vec3};
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

/// arm（下标 1）+ leg（下标 2）：walk 把两者推到 x=10，atk 把 arm 推到 x=100。
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
        tracks: vec![
            line_track(1, "root/arm", Vec3::new(10.0, 0.0, 0.0)),
            line_track(2, "root/leg", Vec3::new(10.0, 0.0, 0.0)),
        ],
    };
    let atk = AnimClip {
        name: "atk".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
    };

    let mut inst = AnimInstance::new(INST, Arc::new(walk));
    inst.bindings = vec![
        AnimBinding {
            node: arm,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        },
        AnimBinding {
            node: leg,
            node_index: 2,
            path: "root/leg".to_string(),
            track: 1,
        },
    ];
    scene.add_anim_instance(inst);
    // 片段注册名带实例前缀，脚本侧只写短名也能解析到。
    scene.register_anim_clip("demo/atk", Arc::new(atk));
    scene.register_anim_clip("demo/idle", Arc::new(line_track_only()));

    let scene = Rc::new(RefCell::new(scene));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

fn line_track_only() -> AnimClip {
    AnimClip {
        name: "idle".to_string(),
        duration: 1.0,
        tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
    }
}

impl Fixture {
    fn run(&mut self, src: &str) {
        let _ = self
            .host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"));
    }

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

    fn strings_of(&mut self, src: &str) -> Vec<String> {
        self.eval(src)
            .into_array()
            .unwrap_or_else(|_| panic!("`{src}` 应返回数组"))
            .into_iter()
            .map(|d| d.into_string().unwrap_or_default())
            .collect()
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

#[test]
fn script_layer_overrides_masked_bone_only() {
    let mut f = fixture();
    assert!(f.bool_of(
        r#"add_anim_layer("demo/walk", "upper", "atk", 1.0); set_anim_layer_mask("demo/walk", "upper", "arm")"#
    ));
    f.advance(0.5);
    let arm = f.pos_x("arm");
    let leg = f.pos_x("leg");
    assert!(
        (arm - 50.0).abs() < 1e-4,
        "遮罩内骨骼应取层姿态，实际 {arm}"
    );
    assert!(
        (leg - 5.0).abs() < 1e-4,
        "遮罩外骨骼应保留主片段，实际 {leg}"
    );
}

#[test]
fn mask_string_roundtrip() {
    let mut f = fixture();
    f.run(r#"add_anim_layer("demo/walk", "upper", "atk", 1.0)"#);
    // 默认整副骨架。
    assert_eq!(
        f.eval(r#"anim_layer_mask("demo/walk", "upper")"#)
            .into_string()
            .unwrap(),
        "*"
    );

    f.run(r#"set_anim_layer_mask("demo/walk", "upper", "arm, hand")"#);
    assert_eq!(
        f.eval(r#"anim_layer_mask("demo/walk", "upper")"#)
            .into_string()
            .unwrap(),
        "arm,hand"
    );

    f.run(r#"set_anim_layer_mask("demo/walk", "upper", "!root")"#);
    assert_eq!(
        f.eval(r#"anim_layer_mask("demo/walk", "upper")"#)
            .into_string()
            .unwrap(),
        "!root"
    );

    // 空串 / `*` / 只有逗号，一律回到「全部骨骼」，不该留下一个空遮罩（那样一层都不动）。
    for spec in [r#""""#, r#""*""#, r#"" , ""#] {
        f.run(&format!(
            r#"set_anim_layer_mask("demo/walk", "upper", {spec})"#
        ));
        assert_eq!(
            f.eval(r#"anim_layer_mask("demo/walk", "upper")"#)
                .into_string()
                .unwrap(),
            "*",
            "遮罩 `{spec}` 应归一成全部"
        );
    }
}

#[test]
fn mode_string_is_validated() {
    let mut f = fixture();
    f.run(r#"add_anim_layer_additive("demo/walk", "breath", "idle", 0.5)"#);
    let info = f
        .eval(r#"anim_layer_info("demo/walk", "breath")"#)
        .into_string()
        .unwrap();
    assert!(
        info.contains("additive"),
        "加法层应标 additive，实际：{info}"
    );

    assert!(
        f.bool_of(r#"set_anim_layer_mode("demo/walk", "breath", "OVERRIDE")"#),
        "模式名大小写不敏感"
    );
    let info = f
        .eval(r#"anim_layer_info("demo/walk", "breath")"#)
        .into_string()
        .unwrap();
    assert!(info.contains("override"), "应切成 override，实际：{info}");

    // 认不出的模式名返回 false，且不偷偷改成别的模式。
    assert!(!f.bool_of(r#"set_anim_layer_mode("demo/walk", "breath", "multiply")"#));
    let info = f
        .eval(r#"anim_layer_info("demo/walk", "breath")"#)
        .into_string()
        .unwrap();
    assert!(
        info.contains("override"),
        "失败调用不该改模式，实际：{info}"
    );
}

#[test]
fn fade_starts_from_current_weight() {
    let mut f = fixture();
    f.run(r#"add_anim_layer("demo/walk", "upper", "atk", 1.0)"#);
    let w = f
        .eval(r#"anim_layer_weight("demo/walk", "upper")"#)
        .as_float()
        .unwrap();
    assert!((w - 1.0).abs() < 1e-5, "初始权重应为 1，实际 {w}");

    // 1 → 0 的淡出：刚起步仍在 1，走完一半到 0.5 附近，走完完全归零。
    f.run(r#"fade_anim_layer("demo/walk", "upper", 0.0, 1.0)"#);
    let w = f
        .eval(r#"anim_layer_weight("demo/walk", "upper")"#)
        .as_float()
        .unwrap();
    assert!((w - 1.0).abs() < 1e-5, "过渡起点应保持当前值，实际 {w}");

    f.advance(0.5);
    let w = f
        .eval(r#"anim_layer_weight("demo/walk", "upper")"#)
        .as_float()
        .unwrap();
    assert!((w - 0.5).abs() < 0.06, "半程应接近 0.5，实际 {w}");

    f.advance(0.6);
    let w = f
        .eval(r#"anim_layer_weight("demo/walk", "upper")"#)
        .as_float()
        .unwrap();
    assert!(w.abs() < 1e-4, "走完应归零，实际 {w}");
}

#[test]
fn layer_clip_list_can_be_edited_from_script() {
    let mut f = fixture();
    f.run(r#"add_anim_layer("demo/walk", "upper", "atk", 1.0)"#);
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk"]
    );

    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "upper", "idle", 0.5)"#));
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk", "idle"]
    );

    // 移除其中一路：剩 atk 一路。
    assert!(f.bool_of(r#"remove_anim_layer_clip("demo/walk", "upper", "idle")"#));
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk"]
    );

    // 换成单片段：多路时收敛成一路，权重回到 1.0。
    assert!(f.bool_of(r#"add_anim_layer_clip("demo/walk", "upper", "idle", 0.5)"#));
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk", "idle"]
    );
    assert!(f.bool_of(r#"set_anim_layer_clip("demo/walk", "upper", "atk")"#));
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk"]
    );

    // 最后一路不可移除：删空了层就没动作可播，返回 false 而不是留个空层。
    assert!(!f.bool_of(r#"remove_anim_layer_clip("demo/walk", "upper", "atk")"#));
    assert_eq!(
        f.strings_of(r#"anim_layer_clips("demo/walk", "upper")"#),
        vec!["atk"]
    );
}

#[test]
fn unknown_clip_name_does_not_create_a_layer() {
    let mut f = fixture();
    assert!(!f.bool_of(r#"add_anim_layer("demo/walk", "upper", "no_such_clip", 1.0)"#));
    assert!(f.strings_of(r#"anim_layers("demo/walk")"#).is_empty());
}

#[test]
fn layer_capacity_is_bounded_and_reported() {
    let mut f = fixture();
    for i in 0..8 {
        assert!(
            f.bool_of(&format!(
                r#"add_anim_layer("demo/walk", "l{i}", "atk", 1.0)"#
            )),
            "第 {i} 层应成功"
        );
    }
    assert!(
        !f.bool_of(r#"add_anim_layer("demo/walk", "l8", "atk", 1.0)"#),
        "第 9 层应被拒"
    );
    assert_eq!(
        f.eval(r#"anim_layer_count("demo/walk")"#).as_int().unwrap(),
        8
    );

    // 同名层是覆盖而不是追加。
    assert!(f.bool_of(r#"add_anim_layer("demo/walk", "l0", "idle", 0.2)"#));
    assert_eq!(
        f.eval(r#"anim_layer_count("demo/walk")"#).as_int().unwrap(),
        8
    );
}

#[test]
fn remove_and_clear_layers() {
    let mut f = fixture();
    f.run(r#"add_anim_layer("demo/walk", "a", "atk", 1.0); add_anim_layer("demo/walk", "b", "idle", 1.0)"#);
    assert!(f.bool_of(r#"remove_anim_layer("demo/walk", "a")"#));
    assert_eq!(f.strings_of(r#"anim_layers("demo/walk")"#), vec!["b"]);
    assert!(
        !f.bool_of(r#"remove_anim_layer("demo/walk", "a")"#),
        "重复删除应返回 false"
    );

    assert!(f.bool_of(r#"clear_anim_layers("demo/walk")"#));
    assert!(f.strings_of(r#"anim_layers("demo/walk")"#).is_empty());
}

#[test]
fn missing_instance_returns_neutral_values() {
    let mut f = fixture();
    assert!(f.strings_of(r#"anim_layers("nobody")"#).is_empty());
    assert_eq!(f.eval(r#"anim_layer_count("nobody")"#).as_int().unwrap(), 0);
    let w = f
        .eval(r#"anim_layer_weight("nobody", "x")"#)
        .as_float()
        .unwrap();
    assert!(w.abs() < 1e-6);
    assert!(!f.bool_of(r#"add_anim_layer("nobody", "x", "atk", 1.0)"#));
    // 提示串里点名实例与层，脚本打日志时能直接定位。
    let info = f
        .eval(r#"anim_layer_info("nobody", "x")"#)
        .into_string()
        .unwrap();
    assert!(
        info.contains("nobody") && info.contains('x'),
        "实际：{info}"
    );
}
