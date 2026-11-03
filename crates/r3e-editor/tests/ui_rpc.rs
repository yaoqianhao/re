//! `ui.*` 的传输层契约测试（扩展十四）。
//!
//! 只验证「JSON → `UiPatch` / key」这一段：字段缺省、类型错误、空 key 拒绝、
//! 归一化方法名命中同一分支、以及宿主未实现时的 `METHOD_NOT_FOUND` 口径。
//! 单位串的解析（`"50%"` → 百分比）与容量上限在宿主侧（`r3e-app`）由单测覆盖。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, PhysicsInfo, Request, Response, UiPatch, INVALID_PARAMS,
};
use serde_json::{json, Value};

/// 只把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<UiPatch>,
    removed: Option<String>,
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

    fn ui_set(&mut self, patch: UiPatch) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen = Some(patch.clone());
        Ok(json!({ "key": patch.key, "unit_echo": patch.width }))
    }

    fn ui_remove(&mut self, key: &str) -> Result<Value, r3e_editor::RpcFailure> {
        self.removed = Some(key.to_string());
        Ok(json!({ "removed": true }))
    }

    fn ui_state(&mut self) -> Result<Value, r3e_editor::RpcFailure> {
        Ok(json!({ "hover": "btn_start", "active": null, "events": [] }))
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
fn ui_set_patch_is_parsed_field_by_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "ui.set",
        json!({
            "key": "btn_start",
            "kind": "image",
            "anchor": "bottom_center",
            "x": "0",
            "y": "-48",
            "width": "50%",
            "height": "64",
            "color": [1.0, 0.5, 0.25, 0.9],
            "layer": 7,
            "visible": false,
            "border_width": 3.0,
            "image": "ui/btn.png",
        }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert_eq!(seen.key, "btn_start");
    assert_eq!(seen.kind.as_deref(), Some("image"));
    assert_eq!(seen.anchor.as_deref(), Some("bottom_center"));
    assert_eq!(seen.x.as_deref(), Some("0"));
    assert_eq!(seen.y.as_deref(), Some("-48"));
    assert_eq!(seen.width.as_deref(), Some("50%"));
    assert_eq!(seen.height.as_deref(), Some("64"));
    assert_eq!(seen.color, Some([1.0, 0.5, 0.25, 0.9]));
    assert_eq!(seen.layer, Some(7));
    assert_eq!(seen.visible, Some(false));
    assert_eq!(seen.border_width, Some(3.0));
    assert_eq!(seen.image.as_deref(), Some("ui/btn.png"));
}

#[test]
fn ui_set_with_only_key_is_allowed_and_marks_empty_patch() {
    // 只给 key：对已有元素是「无意义的空补丁」，但传输层不拦——
    // 是否需要拒绝由宿主按「元素是否存在」自己决定（新建时的默认值就来自这里）。
    let mut api = Mock::default();
    let resp = call(&mut api, "ui.set", json!({ "key": "panel" }));
    assert!(resp.error.is_none(), "空补丁应放行：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert!(seen.is_geometry_only());
}

#[test]
fn ui_set_rejects_empty_key() {
    for key in [json!(""), json!("   "), json!(null)] {
        let mut api = Mock::default();
        let resp = call(&mut api, "ui.set", json!({ "key": key.clone() }));
        let err = resp.error.expect("应报参数错误");
        assert_eq!(err.code, INVALID_PARAMS, "key={key}");
        assert!(api.seen.is_none(), "非法参数不应落到宿主");
    }
}

#[test]
fn ui_set_rejects_wrong_field_types() {
    let cases = [
        json!({ "key": "a", "layer": "顶层" }),
        json!({ "key": "a", "visible": 1 }),
        json!({ "key": "a", "border_width": "宽" }),
        json!({ "key": "a", "color": [1.0, 0.0, 0.0] }),
        json!({ "key": "a", "width": 120 }),
    ];
    for params in cases {
        let mut api = Mock::default();
        let resp = call(&mut api, "ui.set", params.clone());
        let err = resp.error.unwrap_or_else(|| panic!("应报错：{params}"));
        assert_eq!(err.code, INVALID_PARAMS, "params={params}");
    }
}

#[test]
fn method_name_is_normalized() {
    let mut api = Mock::default();
    // 归一化只吃下划线与大小写，点号是命名分隔符要保留：
    // `UI.Set` 命中 `ui.set`，而 `ui_set` 归一成 `uiset`——与 `scene.set_material` 同一口径。
    for method in ["ui.set", "UI.Set", "Ui.SeT"] {
        let resp = call(&mut api, method, json!({ "key": "x", "layer": 1 }));
        assert!(resp.error.is_none(), "{method} 应命中同一分支");
    }
}

#[test]
fn ui_remove_requires_non_empty_key() {
    let mut api = Mock::default();
    let resp = call(&mut api, "ui.remove", json!({ "key": "btn" }));
    assert!(resp.error.is_none());
    assert_eq!(api.removed.as_deref(), Some("btn"));

    let mut api = Mock::default();
    let resp = call(&mut api, "ui.remove", json!({ "key": "  " }));
    assert_eq!(resp.error.expect("应报错").code, INVALID_PARAMS);
    assert!(api.removed.is_none());
}

#[test]
fn ui_state_and_list_pass_through() {
    let mut api = Mock::default();
    let resp = call(&mut api, "ui.state", json!({}));
    assert!(resp.error.is_none());
    assert_eq!(resp.result.as_ref().unwrap()["hover"], "btn_start");

    // 宿主未实现 ui_list：走 unsupported → METHOD_NOT_FOUND，不 panic。
    let resp = call(&mut api, "ui.list", json!({}));
    assert_eq!(
        resp.error.expect("应报未实现").code,
        r3e_editor::METHOD_NOT_FOUND
    );
}

#[test]
fn ui_list_rejects_negative_limit() {
    let mut api = Mock::default();
    let resp = call(&mut api, "ui.list", json!({ "limit": -1 }));
    assert_eq!(resp.error.expect("应报参数错误").code, INVALID_PARAMS);
}
