//! `scene.set_postfx` 的传输层契约测试（扩展十三）。
//!
//! 这里只验证「JSON → `PostFxPatch`」这一段：字段缺省、类型错误、空补丁拒绝、
//! 归一化后的方法名命中同一条分支，以及 `enabled=false` 的清空语义。
//! 真正的收敛（NaN / 超界）在 `r3e-core` 的 `PostFxSettings::sanitized` 里，
//! 由 `postfx_scene` 用例覆盖；色调映射名字的合法性由宿主的 `TonemapMode::from_name`
//! 判定——传输层不认识 `r3e_core` 的任何类型，所以这里不测名字。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, PhysicsInfo, PostFxPatch, Request, Response, INVALID_PARAMS,
};
use serde_json::{json, Value};

/// 只把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<PostFxPatch>,
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

    fn scene_set_postfx(
        &mut self,
        patch: PostFxPatch,
    ) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen = Some(patch.clone());
        Ok(json!({ "enabled": true, "echo": patch.exposure }))
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
        "scene.set_postfx",
        json!({
            "exposure": 0.35,
            "tonemap": "aces",
            "bloom_enabled": true,
            "bloom_threshold": 1.0,
            "bloom_knee": 0.5,
            "bloom_intensity": 0.25,
            "fxaa": true
        }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.exposure, Some(0.35));
    assert_eq!(patch.tonemap.as_deref(), Some("aces"));
    assert_eq!(patch.bloom_enabled, Some(true));
    assert_eq!(patch.bloom_threshold, Some(1.0));
    assert_eq!(patch.bloom_knee, Some(0.5));
    assert_eq!(patch.bloom_intensity, Some(0.25));
    assert_eq!(patch.fxaa, Some(true));
    // 没给的字段必须保持 None，宿主据此「保留原值」。
    assert_eq!(patch.enabled, None);
}

#[test]
fn partial_patch_leaves_other_fields_untouched() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.setPostFx", json!({ "fxaa": true }));
    assert!(resp.error.is_none(), "归一化方法名应命中: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.fxaa, Some(true));
    assert_eq!(patch.exposure, None);
    assert_eq!(patch.tonemap, None);
    assert_eq!(patch.bloom_enabled, None);
}

#[test]
fn disable_switch_is_carried_through() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_postfx", json!({ "enabled": false }));
    assert!(
        resp.error.is_none(),
        "enabled=false 是合法清空请求: {resp:?}"
    );
    assert_eq!(api.seen.expect("补丁应送达宿主").enabled, Some(false));
}

#[test]
fn null_tonemap_is_treated_as_absent() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_postfx",
        json!({ "tonemap": Value::Null, "exposure": 1.0 }),
    );
    assert!(resp.error.is_none(), "显式 null 应视为未给: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.tonemap, None);
    assert_eq!(patch.exposure, Some(1.0));
}

#[test]
fn empty_patch_is_rejected_before_reaching_host() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_postfx", json!({}));
    let err = resp.error.expect("空补丁应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.seen.is_none(), "空补丁不该送达宿主");
}

#[test]
fn wrong_types_are_reported_as_invalid_params() {
    for (key, bad) in [
        ("exposure", json!("bright")),
        ("tonemap", json!(2)),
        ("bloom_enabled", json!("yes")),
        ("bloom_threshold", json!([1.0])),
        ("fxaa", json!(1)),
        ("enabled", json!("false")),
    ] {
        let mut api = Mock::default();
        let resp = call(&mut api, "scene.set_postfx", json!({ key: bad }));
        let err = resp.error.expect("类型错误应被拒");
        assert_eq!(
            err.code, INVALID_PARAMS,
            "`{key}` 的类型错误应报 invalid params"
        );
        assert!(api.seen.is_none());
    }
}
