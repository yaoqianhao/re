//! 脚本层「3D 屏幕射线拾取」绑定的行为测试（扩展三十四）。
//!
//! 锁五件事：
//! ① `pick_at` 用屏幕像素点中内置图元物件，返回 `name` / `distance` / `point`；
//! ② 未命中时返回**空模板**（`hit=false`、`node=-1`、`name=""`、`point=[]`），
//!    脚本写 `r.node` / `r.point[0]` 不会因缺键报错；
//! ③ `pick_all_at` 返回同一条射线上全部命中且按距离升序（穿透选择）；
//! ④ 遮挡语义：近处物件挡住远处，`pick_at` 只报近的那个；
//! ⑤ 屏幕尺寸来自 `UiInteraction`——同一像素坐标在两种分辨率下结论可以不同，
//!    说明绑定确实用了宿主送进来的尺寸，而不是某个写死的常量。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::{Camera, InputState, Scene, Vec3};
use r3e_physics::Physics;
use r3e_script::ScriptHost;

struct Fixture {
    scene: Rc<RefCell<Scene>>,
    host: ScriptHost,
}

fn fixture() -> Fixture {
    let scene = Rc::new(RefCell::new(Scene::new()));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

impl Fixture {
    fn run(&mut self, src: &str) {
        let _ = self
            .host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"));
    }

    fn eval_str(&mut self, src: &str) -> String {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .into_string()
            .unwrap_or_else(|_| panic!("{src} 应返回字符串"))
    }

    fn eval_i64(&mut self, src: &str) -> i64 {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_int()
            .unwrap_or_else(|_| panic!("{src} 应返回整数"))
    }

    fn eval_f64(&mut self, src: &str) -> f64 {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_float()
            .unwrap_or_else(|_| panic!("{src} 应返回浮点"))
    }

    fn eval_bool(&mut self, src: &str) -> bool {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("{src} 应返回布尔"))
    }

    /// 相机正对 +Z（看向原点），屏幕尺寸 800×600：屏幕中心就是 +Z 轴本身。
    fn setup(&mut self, screen: (f32, f32)) {
        {
            let mut scene = self.scene.borrow_mut();
            scene.camera = Camera::new(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
            scene.camera.aspect = screen.0 / screen.1.max(1.0);
            scene.update_world_transforms();
        }
        self.host.ui_interaction().borrow_mut().set_screen(screen);
    }
}

#[test]
fn pick_at_hits_builtin_cube_and_reports_fields() {
    let mut f = fixture();
    f.run(r#"create_node("target", "cube", 0.0, 0.0, 0.0);"#);
    f.setup((800.0, 600.0));

    assert!(
        f.eval_bool("pick_at(400.0, 300.0).hit"),
        "屏幕中心应命中 cube"
    );
    assert_eq!(f.eval_str("pick_at(400.0, 300.0).name"), "target");
    // 内置 cube 边长 1，入口面在 z = 0.5，相机在 z = 10，所以距离 9.5
    assert!(
        (f.eval_f64("pick_at(400.0, 300.0).distance") - 9.5).abs() < 1e-3,
        "distance = {}",
        f.eval_f64("pick_at(400.0, 300.0).distance")
    );
    // 命中点就是正对的那个面上的一点
    assert!((f.eval_f64("pick_at(400.0, 300.0).point[2]") - 0.5).abs() < 1e-3);
    assert!(
        f.eval_i64("pick_at(400.0, 300.0).node") >= 0,
        "命中时 node 应是有效 id"
    );
}

#[test]
fn miss_returns_full_template_so_scripts_never_hit_missing_keys() {
    let mut f = fixture();
    f.run(r#"create_node("target", "cube", 0.0, 0.0, 0.0);"#);
    f.setup((800.0, 600.0));

    // 左上角：射向远处，不该命中中心的 cube
    assert!(!f.eval_bool("pick_at(0.0, 0.0).hit"));
    assert_eq!(f.eval_i64("pick_at(0.0, 0.0).node"), -1);
    assert_eq!(f.eval_str("pick_at(0.0, 0.0).name"), "");
    assert_eq!(
        f.eval_i64("pick_at(0.0, 0.0).point.len()"),
        0,
        "空模板的 point 是空数组"
    );
    // 空模板里的 distance 是 -1，脚本可以直接比较而不用先判 hit
    assert!((f.eval_f64("pick_at(0.0, 0.0).distance") + 1.0).abs() < 1e-6);
}

#[test]
fn pick_all_at_returns_targets_in_near_to_far_order() {
    let mut f = fixture();
    // 三点共线（都在 +Z 轴上）：屏幕中心的那条射线上应该三个都命中
    f.run(
        r#"
        create_node("near", "cube", 0.0, 0.0, 2.0);
        create_node("mid", "cube", 0.0, 0.0, 0.0);
        create_node("far", "cube", 0.0, 0.0, -2.0);
        "#,
    );
    f.setup((800.0, 600.0));

    assert_eq!(f.eval_i64("pick_all_at(400.0, 300.0).len()"), 3);
    assert_eq!(
        f.eval_str("pick_all_at(400.0, 300.0)[0].name"),
        "near",
        "必须按距离升序"
    );
    assert_eq!(f.eval_str("pick_all_at(400.0, 300.0)[1].name"), "mid");
    assert_eq!(f.eval_str("pick_all_at(400.0, 300.0)[2].name"), "far");

    // 最近命中就等于列表第一项：两条 API 在同一套几何上给出一致结论
    assert_eq!(f.eval_str("pick_at(400.0, 300.0).name"), "near");
}

#[test]
fn nearest_wins_when_one_object_occludes_another() {
    let mut f = fixture();
    f.run(
        r#"
        create_node("front", "cube", 0.0, 0.0, 3.0);
        create_node("back", "cube", 0.0, 0.0, 0.0);
        "#,
    );
    f.setup((800.0, 600.0));
    assert_eq!(
        f.eval_str("pick_at(400.0, 300.0).name"),
        "front",
        "前面的物件优先"
    );
    // 但穿透列表里后面那个仍然在（"点不中但查得到"）
    assert_eq!(f.eval_i64("pick_all_at(400.0, 300.0).len()"), 2);
}

#[test]
fn screen_size_comes_from_ui_interaction_not_a_constant() {
    let mut f = fixture();
    f.run(r#"create_node("target", "cube", 0.0, 0.0, 0.0);"#);
    f.setup((800.0, 600.0));
    assert!(f.eval_bool("pick_at(400.0, 300.0).hit"));

    // 换一块更宽的屏：同样的像素坐标不再是中心，射线偏了就该点不到
    f.host
        .ui_interaction()
        .borrow_mut()
        .set_screen((1600.0, 600.0));
    assert!(
        !f.eval_bool("pick_at(400.0, 300.0).hit"),
        "屏幕尺寸变了以后 (400,300) 不再是中心，必须点不到"
    );
    // 新中心重新命中，证明是尺寸在起作用而不是"永远点不到"
    assert!(f.eval_bool("pick_at(800.0, 300.0).hit"));
}

#[test]
fn pick_under_mouse_reads_input_state() {
    let mut f = fixture();
    f.run(r#"create_node("target", "cube", 0.0, 0.0, 0.0);"#);
    f.setup((800.0, 600.0));

    // 脚本层没有写光标的 API，直接改 app 注入的那份 InputState（同一个 Rc）
    {
        let input = f.host.input_handle();
        let mut input = input.borrow_mut();
        input.mouse_x = 400.0;
        input.mouse_y = 300.0;
    }
    assert_eq!(f.eval_str("pick_under_mouse().name"), "target");
    {
        let input = f.host.input_handle();
        let mut input = input.borrow_mut();
        input.mouse_x = 5.0;
        input.mouse_y = 5.0;
    }
    assert!(
        !f.eval_bool("pick_under_mouse().hit"),
        "光标挪开后不该还命中"
    );
}

#[test]
fn screen_size_is_readable_from_scripts() {
    let mut f = fixture();
    f.setup((1280.0, 720.0));
    assert!((f.eval_f64("screen_width()") - 1280.0).abs() < 1e-6);
    assert!((f.eval_f64("screen_height()") - 720.0).abs() < 1e-6);
    // screen_size() 返回 [w, h]：脚本要走"屏幕中心"这类计算不必自己去问一次宽高
    assert_eq!(f.eval_i64("screen_size().len()"), 2);
    assert!((f.eval_f64("screen_size()[1]") - 720.0).abs() < 1e-6);
    // 与 pick_* 同源：按 screen_size() 算出的中心点必然命中正中的物件
    f.run(r#"create_node("center", "cube", 0.0, 0.0, 0.0);"#);
    f.scene.borrow_mut().update_world_transforms();
    assert_eq!(
        f.eval_str(r#"let s = screen_size(); pick_at(s[0] * 0.5, s[1] * 0.5).name"#),
        "center"
    );
}

#[test]
fn invisible_node_is_not_pickable() {
    let mut f = fixture();
    f.run(
        r#"
        create_node("hidden", "cube", 0.0, 0.0, 0.0);
        set_visible("hidden", false);
        "#,
    );
    f.setup((800.0, 600.0));
    assert!(
        !f.eval_bool("pick_at(400.0, 300.0).hit"),
        "看不见的物件不该被点到（渲染侧不画，拾取侧也不该报）"
    );
}
