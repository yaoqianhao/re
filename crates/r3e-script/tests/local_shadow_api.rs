//! 脚本层「局部光源阴影」绑定的行为测试（扩展十八 C）。
//!
//! 锁四件事：
//! ① `light_set_cast_shadow` / `light_set_shadow_bias` 写进场景，key 不存在返回 false；
//! ② `local_light(key)` 快照里带 `cast_shadow` / `shadow_bias` 两项；
//! ③ `local_light_cast_shadow` 读的是「意愿」，`local_shadow_caster_count` 读的是
//!    本帧真正入选的盏数（超过预算时只报预算）；
//! ④ 关掉局部光源后 `local_shadow_caster_count` 归零。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::{InputState, LocalLight, Scene, MAX_LOCAL_SHADOW_CASTERS};
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

    fn eval_i64(&mut self, src: &str) -> i64 {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_int()
            .unwrap_or_else(|_| panic!("{src} 应返回整数"))
    }

    fn eval_bool(&mut self, src: &str) -> bool {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("{src} 应返回布尔"))
    }

    fn add_point(&mut self, key: &str, pos: [f32; 3], intensity: f32) {
        self.scene
            .borrow_mut()
            .upsert_local_light(LocalLight::point(key, pos, [1.0; 3], intensity));
    }
}

/// 开关默认关：老脚本不动场景就没有阴影；显式打开后场景里就是 true。
#[test]
fn cast_shadow_opt_in_via_script() {
    let mut f = fixture();
    f.add_point("lamp", [0.0, 3.0, 0.0], 4.0);
    assert!(
        !f.scene.borrow().local_light("lamp").unwrap().cast_shadow,
        "缺省不投影"
    );

    f.run("light_set_cast_shadow(\"lamp\", true)");
    assert!(f.scene.borrow().local_light("lamp").unwrap().cast_shadow);
    assert_eq!(f.eval_i64("local_shadow_caster_count()"), 1);

    f.run("light_set_cast_shadow(\"lamp\", false)");
    assert_eq!(
        f.eval_i64("local_shadow_caster_count()"),
        0,
        "关掉之后不再入选"
    );
}

/// 拼错 key 返回 false 且不新建灯（与 `light_set_intensity` 同一条约定）。
#[test]
fn cast_shadow_rejects_unknown_key() {
    let mut f = fixture();
    assert!(
        !f.eval_bool(r#"light_set_cast_shadow("nope", true)"#),
        "不存在的 key 必须返回 false"
    );
    assert_eq!(f.eval_i64("local_light_count()"), 0, "不得凭空新建灯");
    assert!(
        !f.eval_bool(r#"light_set_shadow_bias("nope", 0.01)"#),
        "偏移写入同样拒绝未知 key"
    );
}

/// 偏移量落地并被 core 收敛：负值夹到 0、天文数字夹到上限。
#[test]
fn shadow_bias_is_sanitized() {
    let mut f = fixture();
    f.add_point("a", [0.0; 3], 1.0);
    f.run("light_set_shadow_bias(\"a\", 0.01)");
    let bias = f.scene.borrow().local_light("a").unwrap().shadow_bias;
    assert!((bias - 0.01).abs() < 1e-6, "bias = {bias}");

    f.run("light_set_shadow_bias(\"a\", -5.0)");
    assert_eq!(
        f.scene.borrow().local_light("a").unwrap().shadow_bias,
        0.0,
        "负偏移夹到 0"
    );
    f.run("light_set_shadow_bias(\"a\", 999.0)");
    let big = f.scene.borrow().local_light("a").unwrap().shadow_bias;
    assert!(big.is_finite() && big > 0.0 && big <= r3e_core::MAX_LOCAL_SHADOW_BIAS);
}

/// 快照 map 字段齐全：脚本不必再拼多次单字段查询。
#[test]
fn local_light_snapshot_exposes_shadow_fields() {
    let mut f = fixture();
    f.add_point("lamp", [0.0, 2.0, 0.0], 3.0);
    f.run("light_set_cast_shadow(\"lamp\", true); light_set_shadow_bias(\"lamp\", 0.02)");
    let map = f.host.eval("local_light(\"lamp\")").unwrap();
    let map = map.cast::<rhai::Map>();
    assert!(map["cast_shadow"].clone().cast::<bool>());
    assert!((map["shadow_bias"].clone().cast::<f64>() - 0.02).abs() < 1e-6);
    // 老字段不能被这次改动挤掉
    for key in [
        "key",
        "kind",
        "pos",
        "color",
        "intensity",
        "range",
        "dir",
        "inner",
        "outer",
    ] {
        assert!(map.contains_key(key), "快照缺字段 {key}");
    }
    assert!(f.eval_bool(r#"local_light_cast_shadow("lamp")"#));
    assert!(
        !f.eval_bool(r#"local_light_cast_shadow("ghost")"#),
        "不存在的灯读回 false 而不是报错"
    );
}

/// 预算：超量的灯都开了投影，但入选盏数封顶在 `MAX_LOCAL_SHADOW_CASTERS`。
#[test]
fn caster_count_respects_budget() {
    let mut f = fixture();
    for i in 0..(MAX_LOCAL_SHADOW_CASTERS + 3) {
        f.add_point(&format!("l{i}"), [i as f32, 3.0, 0.0], (i + 1) as f32);
    }
    for i in 0..(MAX_LOCAL_SHADOW_CASTERS + 3) {
        f.run(&format!("light_set_cast_shadow(\"l{i}\", true)"));
    }
    assert_eq!(
        f.eval_i64("local_shadow_caster_count()"),
        MAX_LOCAL_SHADOW_CASTERS as i64,
        "入选盏数不得超过预算"
    );
}

/// 总开关优先：局部光源整体关闭时，一盏都不投影。
#[test]
fn disabled_lights_cast_no_shadows() {
    let mut f = fixture();
    f.add_point("lamp", [0.0, 3.0, 0.0], 5.0);
    f.run("light_set_cast_shadow(\"lamp\", true)");
    assert_eq!(f.eval_i64("local_shadow_caster_count()"), 1);
    f.run("set_lights_enabled(false)");
    assert_eq!(
        f.eval_i64("local_shadow_caster_count()"),
        0,
        "关掉局部光源后不投影"
    );
    f.run("set_lights_enabled(true)");
    assert_eq!(f.eval_i64("local_shadow_caster_count()"), 1);
}
