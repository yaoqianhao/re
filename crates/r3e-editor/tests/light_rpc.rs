//! `light.*` 的传输层契约测试（扩展十五）。
//!
//! 只验证「JSON → `LightPatch` / key / limit」这一段：字段缺省与逐个字段映射、
//! 类型错误拒绝、空 key 拒绝、只给 key 视为无意义调用、归一化方法名命中同一分支、
//! 以及宿主未实现时的 `METHOD_NOT_FOUND` 口径。
//!
//! 数值收敛（NaN / 负强度 / 零半径 / 内外角倒挂）与容量上限属于宿主侧语义，
//! 由 `r3e-core` / `r3e-app` 的单测覆盖；本层不做、也不该做这些判断。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, LightPatch, PhysicsInfo, Request, Response, RpcFailure,
    INVALID_PARAMS, METHOD_NOT_FOUND,
};
use serde_json::{json, Value};

/// 只把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<LightPatch>,
    removed: Option<String>,
    limit: Option<Option<usize>>,
    cleared: bool,
    enabled: Option<Option<bool>>,
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

    fn light_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        self.limit = Some(limit);
        Ok(json!({ "enabled": true, "count": 0, "lights": [] }))
    }

    fn light_set(&mut self, patch: LightPatch) -> Result<Value, RpcFailure> {
        self.seen = Some(patch.clone());
        Ok(json!({ "key": patch.key }))
    }

    fn light_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        self.removed = Some(key.to_string());
        Ok(json!({ "key": key, "removed": true }))
    }

    fn light_clear(&mut self) -> Result<Value, RpcFailure> {
        self.cleared = true;
        Ok(json!({ "removed": 3 }))
    }

    fn light_set_enabled(&mut self, enabled: Option<bool>) -> Result<Value, RpcFailure> {
        self.enabled = Some(enabled);
        Ok(json!({ "enabled": enabled.unwrap_or(true) }))
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
fn light_set_patch_is_parsed_field_by_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "light.set",
        json!({
            "key": "lamp",
            "kind": "spot",
            "pos": [1.0, 2.0, 3.0],
            "dir": [0.0, -1.0, 0.0],
            "color": [1.0, 0.5, 0.25],
            "intensity": 8.0,
            "range": 12.0,
            "inner_deg": 15.0,
            "outer_deg": 25.0,
        }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert_eq!(seen.key, "lamp");
    assert_eq!(seen.kind.as_deref(), Some("spot"));
    assert_eq!(seen.pos, Some([1.0, 2.0, 3.0]));
    assert_eq!(seen.dir, Some([0.0, -1.0, 0.0]));
    assert_eq!(seen.color, Some([1.0, 0.5, 0.25]));
    assert_eq!(seen.intensity, Some(8.0));
    assert_eq!(seen.range, Some(12.0));
    assert_eq!(seen.inner_deg, Some(15.0));
    assert_eq!(seen.outer_deg, Some(25.0));
}

/// 扩展十八 C：阴影两项同样是可选补丁字段，缺省必须保持 `None`（沿用宿主原值）。
#[test]
fn light_set_patch_parses_shadow_fields() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "light.set",
        json!({ "key": "lamp", "cast_shadow": true, "shadow_bias": 0.004 }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert_eq!(seen.cast_shadow, Some(true));
    assert_eq!(seen.shadow_bias, Some(0.004));
    assert!(!seen.is_key_only(), "只写阴影也算有效补丁");

    // 只写 shadow_bias / 只写 cast_shadow 都不能被误判成「只有 key」
    let mut api = Mock::default();
    call(
        &mut api,
        "light.set",
        json!({ "key": "lamp", "cast_shadow": false }),
    );
    assert!(api
        .seen
        .is_some_and(|p| p.cast_shadow == Some(false) && p.shadow_bias.is_none()));

    // 类型不对要挡在传输层
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "light.set",
        json!({ "key": "lamp", "cast_shadow": 1 }),
    );
    assert!(resp.error.is_some(), "cast_shadow 应为布尔，1 必须被拒");
    assert!(api.seen.is_none(), "非法参数不得落到宿主");
}

/// 只给 key + 一个字段：其余必须是 `None`（补丁式语义，不是「补全默认值」）。
#[test]
fn light_set_partial_patch_leaves_rest_none() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "light.set",
        json!({ "key": "lamp", "intensity": 2.0 }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert_eq!(seen.intensity, Some(2.0));
    assert_eq!(seen.pos, None);
    assert_eq!(seen.kind, None);
    assert!(!seen.is_key_only());
}

/// 显式 `null` 与字段缺省等价（JSON 客户端常把没填的项写成 null）。
#[test]
fn light_set_null_means_absent() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "light.set",
        json!({ "key": "lamp", "pos": null, "kind": null, "range": null, "color": [1.0, 1.0, 1.0] }),
    );
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    let seen = api.seen.expect("宿主应收到补丁");
    assert_eq!(seen.pos, None);
    assert_eq!(seen.kind, None);
    assert_eq!(seen.range, None);
    assert_eq!(seen.color, Some([1.0, 1.0, 1.0]));
}

/// 只给 key：无意义调用，必须在传输层被挡掉，不能落到宿主。
#[test]
fn light_set_key_only_is_rejected() {
    let mut api = Mock::default();
    let resp = call(&mut api, "light.set", json!({ "key": "lamp" }));
    let err = resp.error.expect("应报参数错误");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.seen.is_none(), "宿主不应收到空补丁");
}

/// 空 key / 全空白 key 一律拒绝。
#[test]
fn light_set_rejects_blank_key() {
    for key in ["", "   "] {
        let mut api = Mock::default();
        let resp = call(
            &mut api,
            "light.set",
            json!({ "key": key, "intensity": 1.0 }),
        );
        let err = resp.error.expect("应报参数错误");
        assert_eq!(err.code, INVALID_PARAMS, "key={key:?}");
    }
}

/// 类型错误必须在传输层拦住，且错误信息要点名出错的字段。
#[test]
fn light_set_rejects_wrong_types() {
    let cases = [
        json!({ "key": "lamp", "pos": [1.0, 2.0] }),
        json!({ "key": "lamp", "color": "red" }),
        json!({ "key": "lamp", "intensity": "8" }),
        json!({ "key": "lamp", "inner_deg": true }),
        json!({ "key": "lamp", "kind": 3 }),
    ];
    for params in cases {
        let mut api = Mock::default();
        let resp = call(&mut api, "light.set", params.clone());
        let err = resp.error.expect("应报参数错误");
        assert_eq!(err.code, INVALID_PARAMS, "params={params}");
        assert!(api.seen.is_none(), "params={params} 不应落到宿主");
    }
}

/// 方法名归一化：大小写与下划线不敏感（`light.set` / `Light.Set` / `LIGHT_SET`…）。
/// 注意分隔点本身保留，所以 `light_set` 不会被当成 `light.set`——与既有
/// `scene.set_transform` 的口径一致。
#[test]
fn method_name_is_normalized() {
    for method in ["light.set", "Light.Set", "LIGHT.SET"] {
        let mut api = Mock::default();
        let resp = call(&mut api, method, json!({ "key": "lamp", "intensity": 1.0 }));
        assert!(
            resp.error.is_none(),
            "{method} 应命中分支：{:?}",
            resp.error
        );
        assert!(api.seen.is_some(), "{method} 应到达宿主");
    }
}

/// `light.list` 的 limit：缺省 / null → None，给了数字 → Some。
#[test]
fn light_list_limit_parsing() {
    let mut api = Mock::default();
    assert!(call(&mut api, "light.list", json!({})).error.is_none());
    assert_eq!(api.limit, Some(None));

    let mut api = Mock::default();
    assert!(call(&mut api, "light.list", json!({ "limit": null }))
        .error
        .is_none());
    assert_eq!(api.limit, Some(None));

    let mut api = Mock::default();
    assert!(call(&mut api, "light.list", json!({ "limit": 5 }))
        .error
        .is_none());
    assert_eq!(api.limit, Some(Some(5)));

    let mut api = Mock::default();
    let resp = call(&mut api, "light.list", json!({ "limit": -1 }));
    assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    assert!(api.limit.is_none(), "非法 limit 不应到达宿主");
}

/// `light.remove`：key 原样下传；空 key 拒绝。
#[test]
fn light_remove_passes_key_and_rejects_blank() {
    let mut api = Mock::default();
    let resp = call(&mut api, "light.remove", json!({ "key": "lamp" }));
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    assert_eq!(api.removed.as_deref(), Some("lamp"));

    let mut api = Mock::default();
    let resp = call(&mut api, "light.remove", json!({ "key": "  " }));
    assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    assert!(api.removed.is_none());
}

/// `light.clear` 与 `light.set_enabled`：后者三态（给 true / 给 false / 不给 = 只读）。
#[test]
fn light_clear_and_enable_switch() {
    let mut api = Mock::default();
    let resp = call(&mut api, "light.clear", json!({}));
    assert!(resp.error.is_none(), "应成功：{:?}", resp.error);
    assert!(api.cleared);

    let mut api = Mock::default();
    assert!(
        call(&mut api, "light.set_enabled", json!({ "enabled": false }))
            .error
            .is_none()
    );
    assert_eq!(api.enabled, Some(Some(false)));

    let mut api = Mock::default();
    assert!(call(&mut api, "light.set_enabled", json!({}))
        .error
        .is_none());
    assert_eq!(api.enabled, Some(None), "不给参数应是只读");

    let mut api = Mock::default();
    let resp = call(&mut api, "light.set_enabled", json!({ "enabled": 1 }));
    assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    assert!(api.enabled.is_none());
}

/// 宿主全用缺省实现时，五个方法都必须报 `METHOD_NOT_FOUND` 而不是 panic / 静默成功。
#[test]
fn unimplemented_light_methods_report_method_not_found() {
    struct Bare;
    impl EngineApi for Bare {
        fn info(&mut self) -> EngineInfo {
            Mock::default().info()
        }
    }
    let mut api = Bare;
    let cases = [
        ("light.list", json!({})),
        ("light.set", json!({ "key": "lamp", "intensity": 1.0 })),
        ("light.remove", json!({ "key": "lamp" })),
        ("light.clear", json!({})),
        ("light.set_enabled", json!({ "enabled": true })),
    ];
    for (method, params) in cases {
        let req = Request {
            jsonrpc: r3e_editor::JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        };
        let resp = dispatch(&mut api, &req);
        let err = resp.error.expect("应报错");
        assert_eq!(err.code, METHOD_NOT_FOUND, "{method}");
    }
}
