//! 脚本层「3D 空间化音频」绑定的行为测试（§12 第十项）。
//!
//! 锁五件事：
//! ① 不注册 = 平面声：`audio_spatial_*` 探针对未注册 key 一律给中性值，既有 21 个
//!    `audio_*` API 的语义没被动过；
//! ② 注册 → 摆位 → 推进，距离衰减真的落到了增益上（越远越小，出界归零）；
//! ③ 模型 / 多普勒参数能写能读，认不出的模型名返回 `false` 且不改原值；
//! ④ 节点绑定只是记名字（`app` 负责解析），解除后立刻回落到平面值；
//! ⑤ 无音频设备时全程降级：不 panic、不算脚本错误、倍速探针恒 false。

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use r3e_core::{InputState, Scene};
use r3e_physics::Physics;
use r3e_script::ScriptHost;

struct Fixture {
    host: ScriptHost,
}

fn fixture() -> Fixture {
    let scene = Rc::new(RefCell::new(Scene::new()));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    Fixture {
        host: ScriptHost::new(scene, physics, input),
    }
}

/// 测试用 wav：与 lib 内音频单测共用同一份资源。
fn blip_wav() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/audio/blip.wav")
        .to_string_lossy()
        .replace('\\', "/")
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

    fn eval_float(&mut self, src: &str) -> f64 {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .as_float()
            .unwrap_or_else(|_| panic!("{src} 应返回浮点"))
    }

    fn eval_string(&mut self, src: &str) -> String {
        self.host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"))
            .into_string()
            .unwrap_or_else(|_| panic!("{src} 应返回字符串"))
    }
}

/// 不注册就是平面声：探针全中性，老 API 语义不受影响。
#[test]
fn unregistered_key_keeps_flat_semantics() {
    let mut f = fixture();
    assert_eq!(f.eval_i64("audio_spatial_count()"), 0);

    // 未注册：增益 / 倍速 1.0、距离 -1、位置空数组、模型空串
    assert!((f.eval_float(r#"audio_spatial_gain("ghost")"#) - 1.0).abs() < 1e-6);
    assert!((f.eval_float(r#"audio_spatial_rate("ghost")"#) - 1.0).abs() < 1e-6);
    assert!((f.eval_float(r#"audio_spatial_distance("ghost")"#) + 1.0).abs() < 1e-6);
    assert_eq!(f.eval_i64(r#"audio_spatial_position("ghost").len()"#), 0);
    assert_eq!(f.eval_string(r#"audio_spatial_model("ghost")"#), "");
    assert_eq!(f.eval_string(r#"audio_spatial_node("ghost")"#), "");

    // 未注册时改位置 / 改模型 / 解除 都返回 false，不 panic
    assert!(!f.eval_bool(r#"audio_spatial_set_position("ghost", 1.0, 2.0, 3.0)"#));
    assert!(!f.eval_bool(r#"audio_spatial_set_model("ghost", "linear")"#));
    assert!(!f.eval_bool(r#"audio_spatial_set_doppler("ghost", 0.5)"#));
    assert!(!f.eval_bool(r#"audio_spatial_detach("ghost")"#));
    assert_eq!(
        f.eval_i64("audio_spatial_count()"),
        0,
        "拒绝的调用不得凭空新建"
    );

    // 平面声照旧：播放请求照常受理（静默后端不登记在播实例，故用返回值判定），
    // 且没有被算出任何衰减
    let wav = blip_wav();
    assert!(f.eval_bool(&format!(r#"audio_play_ex("hit", "{wav}", 0.5, false)"#)));
    assert!((f.eval_float(r#"audio_spatial_gain("hit")"#) - 1.0).abs() < 1e-6);
    assert_eq!(f.eval_i64("audio_spatial_tick(0.016)"), 0, "没有声源可推进");
    f.run("audio_stop_all()");
}

/// 注册 → 摆位 → 推进：距离越大增益越小，超出 max 归零。
#[test]
fn distance_attenuation_reaches_probe() {
    let mut f = fixture();
    let wav = blip_wav();
    assert!(f.eval_bool(r#"audio_spatial_attach("bell", 1.0, 20.0)"#));
    assert_eq!(f.eval_i64("audio_spatial_count()"), 1);
    assert_eq!(f.eval_i64(r#"audio_spatial_keys().len()"#), 1);

    f.run(&format!(r#"audio_play_ex("bell", "{wav}", 1.0, true)"#));
    f.run("audio_spatial_set_listener(0.0, 0.0, 0.0, 0.0, 0.0, -1.0)");

    f.run(r#"audio_spatial_set_position("bell", 0.0, 0.0, 2.0)"#);
    assert_eq!(f.eval_i64("audio_spatial_tick(0.1)"), 1);
    let near_gain = f.eval_float(r#"audio_spatial_gain("bell")"#);
    assert!((f.eval_float(r#"audio_spatial_distance("bell")"#) - 2.0).abs() < 1e-4);

    f.run(r#"audio_spatial_set_position("bell", 0.0, 0.0, 15.0)"#);
    f.run("audio_spatial_tick(0.1)");
    let far_gain = f.eval_float(r#"audio_spatial_gain("bell")"#);
    assert!(
        near_gain > far_gain,
        "近处应比远处响：{near_gain} vs {far_gain}"
    );
    assert!((0.0..=1.0).contains(&near_gain) && (0.0..=1.0).contains(&far_gain));

    // 出界：增益 0（听不见），而不是"很小声"
    f.run(r#"audio_spatial_set_position("bell", 0.0, 0.0, 25.0)"#);
    f.run("audio_spatial_tick(0.1)");
    assert_eq!(f.eval_float(r#"audio_spatial_gain("bell")"#), 0.0);

    // 位置快照是 [x, y, z]
    assert_eq!(f.eval_i64(r#"audio_spatial_position("bell").len()"#), 3);
    assert!((f.eval_float(r#"audio_spatial_position("bell")[2]"#) - 25.0).abs() < 1e-4);
    f.run("audio_stop_all()");
}

/// 模型与多普勒参数可写可读；认不出的模型名拒绝且不改原值。
#[test]
fn model_and_doppler_round_trip() {
    let mut f = fixture();
    assert!(f.eval_bool(r#"audio_spatial_attach_ex("jet", 2.0, 80.0, "linear", 2.5)"#));
    assert_eq!(f.eval_string(r#"audio_spatial_model("jet")"#), "linear");
    assert!((f.eval_float(r#"audio_spatial_doppler("jet")"#) - 2.5).abs() < 1e-6);

    assert!(f.eval_bool(r#"audio_spatial_set_model("jet", "exp")"#));
    assert_eq!(
        f.eval_string(r#"audio_spatial_model("jet")"#),
        "exponential",
        "别名 exp 应解析成 exponential"
    );

    assert!(
        !f.eval_bool(r#"audio_spatial_set_model("jet", "管他什么曲线")"#),
        "未知模型必须返回 false"
    );
    assert_eq!(
        f.eval_string(r#"audio_spatial_model("jet")"#),
        "exponential",
        "拒绝时不得改掉原值"
    );

    // doppler 0 = 关掉多普勒（合法值，不是错误）
    assert!(f.eval_bool(r#"audio_spatial_set_doppler("jet", 0.0)"#));
    assert_eq!(f.eval_float(r#"audio_spatial_doppler("jet")"#), 0.0);
    f.run("audio_spatial_clear()");
    assert_eq!(f.eval_i64("audio_spatial_count()"), 0);
}

/// 节点绑定只记名字；解绑后回落平面值，且不影响正在播的声音。
#[test]
fn node_binding_is_recorded_not_resolved() {
    let mut f = fixture();
    let wav = blip_wav();
    assert!(f.eval_bool(r#"audio_spatial_attach_node("drum", "drum_mesh", 1.0, 30.0)"#));
    assert_eq!(f.eval_string(r#"audio_spatial_node("drum")"#), "drum_mesh");

    // 绑定只是记录：坐标仍需 app 每帧灌（这里没有 app，所以还在原点）
    assert_eq!(f.eval_i64("audio_spatial_tick(0.016)"), 1);
    f.run(&format!(r#"audio_play_ex("drum", "{wav}", 1.0, true)"#));
    f.run(r#"audio_spatial_set_position("drum", 0.0, 0.0, 12.0)"#);
    f.run("audio_spatial_tick(0.1)");
    assert!(f.eval_float(r#"audio_spatial_gain("drum")"#) < 1.0);

    // 解除注册：声音继续播（解除 ≠ 停止），增益立刻回 1.0
    assert!(f.eval_bool(r#"audio_spatial_detach("drum")"#));
    assert_eq!(f.eval_i64("audio_spatial_count()"), 0);
    assert_eq!(f.eval_float(r#"audio_spatial_gain("drum")"#), 1.0);
    assert_eq!(f.eval_string(r#"audio_spatial_node("drum")"#), "");
    f.run("audio_stop_all()");
}

/// 上限：注册满额后继续注册返回 false 并记错误，不影响已有声源。
#[test]
fn attach_respects_source_budget() {
    let mut f = fixture();
    let budget = r3e_audio::MAX_SPATIAL_SOURCES;
    for i in 0..budget {
        assert!(
            f.eval_bool(&format!(r#"audio_spatial_attach("s{i}", 1.0, 10.0)"#)),
            "第 {i} 条应注册成功"
        );
    }
    assert_eq!(f.eval_i64("audio_spatial_count()"), budget as i64);

    let errors_before = f.eval_i64("audio_error_count()");
    assert!(
        !f.eval_bool(r#"audio_spatial_attach("overflow", 1.0, 10.0)"#),
        "超限必须拒绝"
    );
    assert_eq!(f.eval_i64("audio_spatial_count()"), budget as i64);
    assert!(
        f.eval_i64("audio_error_count()") > errors_before,
        "超限应记一笔错误"
    );
}

/// 无设备降级：不 panic、倍速探针 false、状态行标明不支持变速、错误计数不因无声卡增长。
#[test]
fn silent_backend_degrades_without_panic() {
    let mut f = fixture();
    let wav = blip_wav();
    assert!(!f.eval_bool("audio_available()"), "测试环境没有声卡");
    assert!(
        !f.eval_bool("audio_supports_speed()"),
        "静默后端不该声称支持变速"
    );

    let errors_before = f.eval_i64("audio_error_count()");
    assert!(f.eval_bool(r#"audio_spatial_attach("env", 0.5, 30.0)"#));
    f.run(&format!(r#"audio_play_ex("env", "{wav}", 1.0, true)"#));

    // 200 帧横穿：增益 / 倍速必须是有限值，且倍速为正（绝不出现 0 或负数）
    for i in 0..200 {
        f.run(&format!(
            r#"audio_spatial_set_position("env", {:.1}, 0.0, 5.0); audio_spatial_tick(0.016)"#,
            i as f64 * 0.5
        ));
    }
    let gain = f.eval_float(r#"audio_spatial_gain("env")"#);
    let rate = f.eval_float(r#"audio_spatial_rate("env")"#);
    assert!(gain.is_finite() && (0.0..=1.0).contains(&gain), "{gain}");
    assert!(rate.is_finite() && rate > 0.0, "{rate}");

    // 非法 dt 同样不炸
    f.run("audio_spatial_tick(0.0); audio_spatial_tick(-1.0)");

    let status = f.eval_string("audio_spatial_status()");
    assert!(
        status.contains("space") || status.contains("空间化"),
        "{status}"
    );
    assert!(status.contains("不支持"), "{status}");

    assert_eq!(
        f.eval_i64("audio_error_count()"),
        errors_before,
        "没有声卡不算脚本错误"
    );
    f.run("audio_stop_all()");
}
