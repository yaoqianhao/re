//! `scene.set_environment` 的传输层契约测试。
//!
//! 这里只验证「JSON → `EnvironmentPatch`」这一段：字段缺省、类型错误、空补丁拒绝、
//! 以及归一化后的方法名都能命中同一条分支。真正的收敛（NaN / 超界 / 方向归一化）
//! 在 `r3e-core` 的 `EnvironmentSettings::sanitized` 里，由 `env_probe` 用例覆盖。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, EnvironmentPatch, PhysicsInfo, Request, Response,
    INVALID_PARAMS,
};
use serde_json::{json, Value};

/// 只把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<EnvironmentPatch>,
}

impl EngineApi for Mock {
    fn info(&mut self) -> EngineInfo {
        EngineInfo {
            name: "mock".into(),
            version: "0.0.0".into(),
            protocol: r3e_editor::PROTOCOL_VERSION,
            scene: "mock".into(),
            script: String::new(),
            frames: 0,
            fps: 0.0,
            uptime_ms: 0,
            paused: false,
            nodes: 0,
            physics: PhysicsInfo {
                enabled: false,
                backend: "null".into(),
                capabilities: Vec::new(),
                bodies: 0,
                ccd_default: false,
            },
        }
    }

    fn scene_set_environment(
        &mut self,
        patch: EnvironmentPatch,
    ) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen = Some(patch);
        Ok(json!({ "enabled": true, "echo": patch.sun_intensity }))
    }
}

fn call(api: &mut Mock, method: &str, params: Value) -> Response {
    let req = Request {
        jsonrpc: r3e_editor::JSONRPC.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    };
    dispatch(api, &req)
}

#[test]
fn full_patch_is_parsed_field_by_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_environment",
        json!({
            "sky_zenith": [0.1, 0.2, 0.5],
            "sky_horizon": [0.6, 0.5, 0.45],
            "ground": [0.05, 0.05, 0.05],
            "sun_dir": [0.4, 0.8, 0.3],
            "sun_color": [1.0, 0.95, 0.9],
            "sun_intensity": 1.4,
            "intensity": 0.8
        }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.sky_zenith, Some([0.1, 0.2, 0.5]));
    assert_eq!(patch.sky_horizon, Some([0.6, 0.5, 0.45]));
    assert_eq!(patch.ground, Some([0.05, 0.05, 0.05]));
    assert_eq!(patch.sun_dir, Some([0.4, 0.8, 0.3]));
    assert_eq!(patch.sun_color, Some([1.0, 0.95, 0.9]));
    assert_eq!(patch.sun_intensity, Some(1.4));
    assert_eq!(patch.intensity, Some(0.8));
    // 没给的字段必须保持 None，宿主据此「保留原值」。
    assert_eq!(patch.enabled, None);
}

#[test]
fn partial_patch_leaves_other_fields_untouched() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.setEnvironment",
        json!({ "intensity": 2.5 }),
    );
    assert!(resp.error.is_none(), "归一化方法名应命中: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.intensity, Some(2.5));
    assert_eq!(patch.sun_intensity, None);
    assert_eq!(patch.sky_zenith, None);
}

#[test]
fn disable_switch_is_carried_through() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_environment",
        json!({ "enabled": false }),
    );
    assert!(
        resp.error.is_none(),
        "enabled=false 是合法清空请求: {resp:?}"
    );
    assert_eq!(api.seen.expect("补丁应送达宿主").enabled, Some(false));
}

#[test]
fn empty_patch_is_rejected_before_reaching_host() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_environment", json!({}));
    let err = resp.error.expect("空补丁应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.seen.is_none(), "空补丁不该送达宿主");
}

#[test]
fn wrong_types_are_reported_as_invalid_params() {
    for (key, bad) in [
        ("sky_zenith", json!([0.1, 0.2])),
        ("sun_color", json!("white")),
        ("sun_intensity", json!("bright")),
        ("enabled", json!("false")),
    ] {
        let mut api = Mock::default();
        let resp = call(&mut api, "scene.set_environment", json!({ key: bad }));
        let err = resp.error.expect("类型错误应被拒");
        assert_eq!(
            err.code, INVALID_PARAMS,
            "`{key}` 的类型错误应报 invalid params"
        );
        assert!(api.seen.is_none());
    }
}
