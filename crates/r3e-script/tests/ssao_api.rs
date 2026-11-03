//! 脚本层 SSAO 绑定的行为测试（扩展十七）。
//!
//! `scripts_compile` 只保证演示脚本能编译，管不到「API 调用之后场景里到底变成什么」。
//! 这里用 `ScriptHost::eval` 真跑一遍 Rhai，检查四件事：
//! ① `set_ssao` 会把 `postfx` 段实体化并写入全部字段；
//! ② 越界值被 core 的 `sanitized` 收敛（半径 / 样本数夹紧，NaN 回落缺省）；
//! ③ `clear_ssao` 只掐遮蔽支线，不连累曝光 / 泛光；
//! ④ 只读函数在未开启时给缺省值，`has_ssao` 认「强度 0 = 没在算」。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::ssao::{
    DEFAULT_SSAO_BIAS, DEFAULT_SSAO_INTENSITY, DEFAULT_SSAO_RADIUS, DEFAULT_SSAO_SAMPLES,
    MAX_SSAO_INTENSITY, MAX_SSAO_RADIUS, MAX_SSAO_SAMPLES, MIN_SSAO_SAMPLES,
};
use r3e_core::{InputState, Scene};
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
        let _ = self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"));
    }

    fn ssao(&self) -> r3e_core::ssao::SsaoSettings {
        self.scene
            .borrow()
            .postfx
            .expect("postfx 段应已实体化")
            .ssao
    }
}

#[test]
fn set_ssao_materializes_postfx_segment() {
    let mut f = fixture();
    assert!(
        f.scene.borrow().postfx.is_none(),
        "空场景一开始不该有 postfx 段"
    );
    f.run("set_ssao(true, 0.45, 0.03, 1.5, 12)");
    let s = f.ssao();
    assert!(s.enabled);
    assert!((s.radius - 0.45).abs() < 1e-6);
    assert!((s.bias - 0.03).abs() < 1e-6);
    assert!((s.intensity - 1.5).abs() < 1e-6);
    assert_eq!(s.samples, 12);
}

#[test]
fn out_of_range_values_are_clamped_by_core() {
    let mut f = fixture();
    // 半径远超上限、强度也超、样本数远超上限：全部交给 sanitized 收敛。
    f.run("set_ssao(true, 1.0e9, 9.0, 99.0, 999)");
    let s = f.ssao();
    assert!(
        (s.radius - MAX_SSAO_RADIUS).abs() < 1e-6,
        "半径应夹到上限: {s:?}"
    );
    assert!(
        (s.intensity - MAX_SSAO_INTENSITY).abs() < 1e-6,
        "强度应夹到上限: {s:?}"
    );
    assert_eq!(s.samples, MAX_SSAO_SAMPLES);
    // 负数样本数在脚本层就被夹到下限再交给 core——`as u8` 是回绕不是饱和。
    f.run("set_ssao_samples(-9)");
    assert_eq!(f.ssao().samples, MIN_SSAO_SAMPLES);
}

#[test]
fn nan_input_falls_back_to_defaults() {
    let mut f = fixture();
    f.run("set_ssao(true, 0.0/0.0, 0.0/0.0, 0.0/0.0, 8)");
    let s = f.ssao();
    assert!(
        (s.radius - DEFAULT_SSAO_RADIUS).abs() < 1e-6,
        "NaN 半径应回落缺省: {s:?}"
    );
    assert!(
        (s.bias - DEFAULT_SSAO_BIAS).abs() < 1e-6,
        "NaN 偏置应回落缺省: {s:?}"
    );
    assert!(
        (s.intensity - DEFAULT_SSAO_INTENSITY).abs() < 1e-6,
        "NaN 强度应回落缺省: {s:?}"
    );
}

#[test]
fn clear_ssao_keeps_the_rest_of_the_chain() {
    let mut f = fixture();
    f.run("set_exposure(2.0); set_ssao(true, 0.7, 0.02, 1.0, 8); clear_ssao()");
    let fx = f.scene.borrow().postfx.expect("段不该被清掉");
    assert!(!fx.ssao.enabled, "遮蔽应已关闭");
    assert!((fx.exposure - 2.0).abs() < 1e-6, "曝光必须保留: {fx:?}");
    // 对比 `clear_postfx`：那才是把整段打回直通。
    f.run("clear_postfx()");
    assert!(f.scene.borrow().postfx.is_none());
}

#[test]
fn has_ssao_treats_zero_intensity_as_off() {
    let mut f = fixture();
    f.run("set_ssao(true, 0.6, 0.02, 0.0, 8)");
    let on = f.host.eval("has_ssao()").expect("eval 应成功");
    assert_eq!(on.as_bool().ok(), Some(false), "强度 0 时不该算在算遮蔽");
    f.run("set_ssao_intensity(1.0)");
    let on = f.host.eval("has_ssao()").expect("eval 应成功");
    assert_eq!(on.as_bool().ok(), Some(true));
}

#[test]
fn readers_report_defaults_before_anything_is_set() {
    let mut f = fixture();
    assert_eq!(
        f.host.eval("ssao_enabled()").expect("eval").as_bool().ok(),
        Some(false)
    );
    let radius = f.host.eval("ssao_radius()").expect("eval");
    assert_eq!(
        radius.as_float().ok().map(|v| v as f32),
        Some(DEFAULT_SSAO_RADIUS)
    );
    let samples = f.host.eval("ssao_samples()").expect("eval");
    assert_eq!(
        samples.as_int().ok().map(|v| v as u8),
        Some(DEFAULT_SSAO_SAMPLES)
    );
}

#[test]
fn enabling_alone_preserves_tuned_parameters() {
    let mut f = fixture();
    // 调好参数后关掉再开：半径 / 强度不该被重置回缺省。
    f.run("set_ssao(true, 1.2, 0.05, 2.0, 14); set_ssao_enabled(false); set_ssao_enabled(true)");
    let s = f.ssao();
    assert!(s.enabled);
    assert!((s.radius - 1.2).abs() < 1e-6);
    assert!((s.intensity - 2.0).abs() < 1e-6);
    assert_eq!(s.samples, 14);
}
