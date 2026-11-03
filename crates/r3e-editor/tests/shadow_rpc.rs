//! `scene.shadow` 的传输层契约测试（扩展十六）。
//!
//! 只验证「JSON → `Option<ShadowPatch>`」这一段：字段逐个映射、全缺省视为只读、
//! 显式 `null` 视为没给、类型错误在该层被拦、方法名归一化、宿主未实现时的口径。
//!
//! 数值收敛（级数夹到 1..=MAX_CASCADES、混合比夹到 0..1、尺寸夹到 64..8192）与
//! 「合并后回写」属于宿主语义，由 `r3e-app` 的单测覆盖。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, PhysicsInfo, Request, Response, RpcFailure, ShadowPatch,
    INVALID_PARAMS, METHOD_NOT_FOUND,
};
use serde_json::{json, Value};

/// 只把补丁记下来的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<Option<ShadowPatch>>,
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

    fn scene_shadow(&mut self, patch: Option<ShadowPatch>) -> Result<Value, RpcFailure> {
        self.seen = Some(patch);
        Ok(json!({ "cascades": 4 }))
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

/// 全字段一次性映射，逐项核对——任何字段接错线都会在这里炸。
#[test]
fn patch_is_parsed_field_by_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.shadow",
        json!({
            "enabled": true,
            "size": 2048,
            "strength": 0.8,
            "orthoSize": 12.0,
            "bias": 0.002,
            "cascades": 3,
            "blend": 0.15,
            "distance": 60.0,
        }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let patch = api.seen.flatten().expect("宿主应收到补丁");
    assert_eq!(patch.enabled, Some(true));
    assert_eq!(patch.size, Some(2048));
    assert_eq!(patch.strength, Some(0.8));
    assert_eq!(patch.ortho_size, Some(12.0));
    assert_eq!(patch.bias, Some(0.002));
    assert_eq!(patch.cascades, Some(3));
    assert_eq!(patch.blend, Some(0.15));
    assert_eq!(patch.distance, Some(60.0));
}

/// 只给一个字段：其余保持 `None`（补丁式语义）。
#[test]
fn partial_patch_leaves_rest_none() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.shadow", json!({ "cascades": 4 }));
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let patch = api.seen.flatten().expect("宿主应收到补丁");
    assert_eq!(patch.cascades, Some(4));
    assert_eq!(patch.size, None);
    assert_eq!(patch.blend, None);
    assert!(!patch.is_read_only());
}

/// 空参数 = 纯只读：宿主收到 `None`，不会误触发回写。
#[test]
fn empty_params_mean_read_only() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.shadow", json!({}));
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    assert_eq!(api.seen, Some(None), "空参数必须是只读");

    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.shadow",
        json!({ "enabled": null, "size": null, "cascades": null }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    assert_eq!(api.seen, Some(None), "全 null 也等价于只读");
}

/// `enabled: false` 必须与「没给」区分开——这是最容易写错的一处（falsy 陷阱）。
#[test]
fn false_and_zero_are_not_treated_as_absent() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.shadow",
        json!({ "enabled": false, "strength": 0.0, "blend": 0.0, "distance": 0.0 }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let patch = api.seen.flatten().expect("宿主应收到补丁");
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(patch.strength, Some(0.0));
    assert_eq!(patch.blend, Some(0.0));
    assert_eq!(patch.distance, Some(0.0));
}

/// 类型错误在传输层拦住，不落到宿主。
#[test]
fn wrong_types_are_rejected() {
    let cases = [
        json!({ "enabled": 1 }),
        json!({ "size": -1 }),
        json!({ "size": 2048.5 }),
        json!({ "cascades": "4" }),
        json!({ "blend": true }),
        json!({ "distance": [1.0] }),
    ];
    for params in cases {
        let mut api = Mock::default();
        let resp = call(&mut api, "scene.shadow", params.clone());
        let err = resp.error.expect("应报参数错误");
        assert_eq!(err.code, INVALID_PARAMS, "params={params}");
        assert!(api.seen.is_none(), "params={params} 不应落地");
    }
}

/// 方法名归一化：大小写不敏感。
#[test]
fn method_name_is_normalized() {
    for method in ["scene.shadow", "Scene.Shadow", "SCENE.SHADOW"] {
        let mut api = Mock::default();
        let resp = call(&mut api, method, json!({ "cascades": 2 }));
        assert!(resp.error.is_none(), "{method} 应命中分支：{:?}", resp.error);
        assert!(api.seen.is_some(), "{method} 应到达宿主");
    }
}

/// 宿主未实现时报 `METHOD_NOT_FOUND`，而不是 panic 或静默成功。
#[test]
fn unimplemented_reports_method_not_found() {
    struct Bare;
    impl EngineApi for Bare {
        fn info(&mut self) -> EngineInfo {
            Mock::default().info()
        }
    }
    let mut api = Bare;
    let req = Request {
        jsonrpc: r3e_editor::JSONRPC.to_string(),
        id: Some(json!(1)),
        method: "scene.shadow".to_string(),
        params: json!({ "cascades": 2 }),
    };
    let resp = dispatch(&mut api, &req);
    assert_eq!(resp.error.expect("应报错").code, METHOD_NOT_FOUND);
}
