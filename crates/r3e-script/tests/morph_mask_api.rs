//! 脚本层逐实例形状掩码的行为测试（扩展四十）。
//!
//! 场景与渲染两侧的掩码口径已有单测，这里只验「脚本能不能真的把掩码配上去」：
//! ① `morph_mask` 默认全开、`morph_mask_set` 写回裁剪后的生效值；
//! ② `morph_show` 单形状开关与 `morph_visible` 读回一致；
//! ③ 掩码与归一化联动：关掉组里一个形状后，剩下的不再被平分；
//! ④ 非法入参（负下标、超 u32 的掩码、未知物件）一律 -1 / false，不污染场景。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::{InputState, ObjectDesc, Scene};
use r3e_physics::Physics;
use r3e_script::ScriptHost;

struct Fixture {
    host: ScriptHost,
}

fn fixture() -> Fixture {
    let scene = Rc::new(RefCell::new(Scene::new()));
    scene
        .borrow_mut()
        .spawn(ObjectDesc {
            name: "face".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let host = ScriptHost::new(scene, physics, input);
    Fixture { host }
}

impl Fixture {
    fn eval(&mut self, src: &str) -> rhai::Dynamic {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
    }

    fn run(&mut self, src: &str) {
        let _ = self.eval(src);
    }

    fn int(&mut self, src: &str) -> i64 {
        self.eval(src)
            .try_cast::<i64>()
            .unwrap_or_else(|| panic!("表达式应返回整数: {src}"))
    }

    fn truthy(&mut self, src: &str) -> bool {
        self.eval(src)
            .try_cast::<bool>()
            .unwrap_or_else(|| panic!("表达式应返回 bool: {src}"))
    }

    fn norm(&mut self, index: usize) -> f32 {
        self.eval(&format!("morph_norm(\"face\", {index})"))
            .try_cast::<f64>()
            .expect("morph_norm 应返回浮点") as f32
    }
}

#[test]
fn script_mask_defaults_to_all_shapes() {
    let mut f = fixture();
    f.run("morph_bind(\"face\", 3)");
    assert_eq!(f.int("morph_mask(\"face\")"), 0b111, "默认全启用");
    assert!(f.truthy("morph_visible(\"face\", 0)"));
    assert!(f.truthy("morph_visible(\"face\", 2)"));
    assert!(
        !f.truthy("morph_visible(\"face\", 3)"),
        "越界下标读不到「启用」"
    );
}

#[test]
fn script_mask_write_returns_clipped_value() {
    let mut f = fixture();
    f.run("morph_bind(\"face\", 3)");
    // 65535 只有低 3 位有效
    assert_eq!(f.int("morph_mask_set(\"face\", 65535)"), 0b111);
    assert_eq!(f.int("morph_mask(\"face\")"), 0b111);
    // 只开形状 1
    assert_eq!(f.int("morph_mask_set(\"face\", 0b010)"), 0b010);
    assert!(f.truthy("morph_visible(\"face\", 1)"));
    assert!(!f.truthy("morph_visible(\"face\", 0)"));
}

#[test]
fn script_shape_toggle_feeds_normalized_weights() {
    let mut f = fixture();
    // 两个形状同组各 1.0 → 各 0.5；关掉形状 1 后形状 0 独自 1.0
    f.run(
        r#"
        morph_bind("face", 2);
        morph_set("face", 0, 1.0);
        morph_set("face", 1, 1.0);
        morph_exclusive("face", 0, 0);
        morph_exclusive("face", 1, 0);
        "#,
    );
    assert!((f.norm(0) - 0.5).abs() < 1e-6, "实际 {}", f.norm(0));

    assert!(f.truthy("morph_show(\"face\", 1, false)"));
    assert!(!f.truthy("morph_visible(\"face\", 1)"));
    assert!(
        (f.norm(0) - 1.0).abs() < 1e-6,
        "被掩码挡住后形状 0 不再被平分，实际 {}",
        f.norm(0)
    );
    assert!((f.norm(1) - 0.0).abs() < 1e-6, "掩码外的形状权重读到 0");

    assert!(f.truthy("morph_show(\"face\", 1, true)"));
    assert!((f.norm(0) - 0.5).abs() < 1e-6, "实际 {}", f.norm(0));
}

#[test]
fn script_mask_rejects_bad_arguments_without_touching_scene() {
    let mut f = fixture();
    f.run("morph_bind(\"face\", 2)");
    assert!(
        !f.truthy("morph_show(\"face\", -1, false)"),
        "负下标必须拒绝"
    );
    assert!(
        !f.truthy("morph_show(\"face\", 9, false)"),
        "超出形状数必须拒绝"
    );
    assert!(
        !f.truthy("morph_show(\"nobody\", 0, false)"),
        "未知物件必须拒绝"
    );
    assert_eq!(f.int("morph_mask(\"face\")"), 0b11, "被拒的写不改变掩码");

    assert_eq!(
        f.int("morph_mask_set(\"face\", -1)"),
        -1,
        "负数掩码不许按位截断"
    );
    assert_eq!(
        f.int("morph_mask_set(\"face\", 4294967296)"),
        -1,
        "超出 u32 不许静默回绕"
    );
    assert_eq!(f.int("morph_mask_set(\"nobody\", 1)"), -1);
    assert_eq!(f.int("morph_mask(\"nobody\")"), -1);
}
