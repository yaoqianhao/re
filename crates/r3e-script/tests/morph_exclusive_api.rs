//! 脚本层表情互斥绑定的行为测试（扩展三十八）。
//!
//! 核心单测已经覆盖归一化算法本身，这里只验「脚本能不能真的把组配上去」：
//! ① `morph_exclusive` 写组号、`-1` 取出；
//! ② `morph_group` 读回的是场景里真实生效的组；
//! ③ `morph_norm` 给的是**进实例缓冲的那一份权重**（被等比压回 1）；
//! ④ 非法入参（负下标、越界组号、未知物件）一律返回 false / -1，不污染场景。

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
    let mut scene_ref = scene.borrow_mut();
    scene_ref
        .spawn(ObjectDesc {
            name: "face".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    drop(scene_ref);
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { host }
}

impl Fixture {
    fn eval(&mut self, src: &str) -> rhai::Dynamic {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
    }

    /// 只跑语句、不关心返回值（`eval` 返回的 `Dynamic` 必须显式丢弃）。
    fn run(&mut self, src: &str) {
        let _ = self.eval(src);
    }

    fn group(&mut self, index: usize) -> i64 {
        self.eval(&format!("morph_group(\"face\", {index})"))
            .try_cast::<i64>()
            .expect("morph_group 应返回整数")
    }

    fn norm(&mut self, index: usize) -> f32 {
        self.eval(&format!("morph_norm(\"face\", {index})"))
            .try_cast::<f64>()
            .expect("morph_norm 应返回浮点") as f32
    }

    fn truthy(&mut self, src: &str) -> bool {
        self.eval(src)
            .try_cast::<bool>()
            .unwrap_or_else(|| panic!("表达式应返回 bool: {src}"))
    }
}

#[test]
fn script_can_group_shapes_and_read_normalized_weights() {
    let mut f = fixture();
    // 两个形状各 1.0：不配组时互不影响，配进同一组后各被压到 0.5
    f.run(
        r#"
        morph_bind("face", 2);
        morph_set("face", 0, 1.0);
        morph_set("face", 1, 1.0);
        "#,
    );
    assert!(
        (f.norm(0) - 1.0).abs() < 1e-6,
        "没配组时应保持原权重，实际 {}",
        f.norm(0)
    );

    assert!(f.truthy("morph_exclusive(\"face\", 0, 0)"));
    assert!(f.truthy("morph_exclusive(\"face\", 1, 0)"));
    assert_eq!(f.group(0), 0);
    assert_eq!(f.group(1), 0);
    assert!((f.norm(0) - 0.5).abs() < 1e-6, "实际 {}", f.norm(0));
    assert!((f.norm(1) - 0.5).abs() < 1e-6, "实际 {}", f.norm(1));
    // 场景原权重不受影响：脚本读自己写进去的仍是 1.0
    assert_eq!(
        f.eval("morph_weight(\"face\", 0)").try_cast::<f64>(),
        Some(1.0)
    );

    // -1 = 取出该形状
    assert!(f.truthy("morph_exclusive(\"face\", 1, -1)"));
    assert_eq!(f.group(1), -1);
    assert!((f.norm(1) - 1.0).abs() < 1e-6, "实际 {}", f.norm(1));
}

#[test]
fn script_rejects_bad_arguments_without_touching_scene() {
    let mut f = fixture();
    f.run(
        r#"
        morph_bind("face", 2);
        morph_set("face", 0, 1.0);
        "#,
    );
    assert!(
        !f.truthy("morph_exclusive(\"face\", -1, 0)"),
        "负下标必须拒绝"
    );
    assert!(
        !f.truthy("morph_exclusive(\"face\", 9, 0)"),
        "超出形状数必须拒绝"
    );
    assert!(
        !f.truthy("morph_exclusive(\"face\", 0, 300)"),
        "组号超出 u8"
    );
    assert!(
        !f.truthy("morph_exclusive(\"nobody\", 0, 0)"),
        "未知物件必须拒绝"
    );
    assert_eq!(f.group(0), -1, "失败不应留下组号");
    assert!((f.norm(0) - 1.0).abs() < 1e-6);
}

#[test]
fn morph_norm_is_minus_one_without_binding() {
    let mut f = fixture();
    assert_eq!(
        f.eval("morph_norm(\"nobody\", 0)").try_cast::<f64>(),
        Some(-1.0)
    );
    assert_eq!(
        f.eval("morph_group(\"nobody\", 0)").try_cast::<i64>(),
        Some(-1)
    );
    f.run("morph_bind(\"face\", 2)");
    assert_eq!(
        f.eval("morph_norm(\"face\", 7)").try_cast::<f64>(),
        Some(-1.0)
    );
}
