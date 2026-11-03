//! `scene.anim_layers` / `scene.anim_layer_set` 的传输层契约测试（扩展十八 A）。
//!
//! 只验证「JSON → `AnimLayerPatch`」这一段：字段缺省、显式 null、类型错误、
//! 空补丁拒绝、方法名归一化、`layer` 缺省语义。至于「权重 / 遮罩 / 模式到底怎么落到
//! 骨骼上」，属于 `r3e-core` 的合成逻辑与宿主的 `scene_anim_layer_set`，
//! 由 `anim_blend` 单测与 `r3e-app` 的宿主测试覆盖——传输层不认识 `r3e_core` 的
//! 任何类型，遮罩串与模式名在这里也只是字符串。

use r3e_editor::{
    dispatch, AnimLayerPatch, EngineApi, EngineInfo, PhysicsInfo, Request, Response, INVALID_PARAMS,
};
use serde_json::{json, Value};

/// 只把请求记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen_list: Option<(Option<String>, Option<usize>)>,
    seen_patch: Option<AnimLayerPatch>,
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

    fn scene_anim_layers(
        &mut self,
        instance: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen_list = Some((instance.map(|s| s.to_string()), limit));
        Ok(json!({ "count": 0, "instances": 0, "layers": [] }))
    }

    fn scene_anim_layer_set(
        &mut self,
        patch: AnimLayerPatch,
    ) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen_patch = Some(patch.clone());
        Ok(json!({ "layer": { "instance": patch.instance, "layer": patch.layer } }))
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
fn list_passes_filters_through() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layers",
        json!({ "instance": "hero/walk", "limit": 4 }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    assert_eq!(
        api.seen_list,
        Some((Some("hero/walk".to_string()), Some(4))),
        "实例名与上限应原样送达"
    );
}

#[test]
fn list_without_filters_asks_for_everything() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.anim_layers", json!({}));
    assert!(resp.error.is_none(), "无参应合法: {resp:?}");
    assert_eq!(
        api.seen_list,
        Some((None, None)),
        "缺省 = 全部实例、不限条数"
    );

    // 显式 null 与缺省等价：编辑器面板常常把「没选实例」序列化成 null。
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layers",
        json!({ "instance": Value::Null }),
    );
    assert!(resp.error.is_none(), "instance=null 应视为未给: {resp:?}");
    assert_eq!(api.seen_list, Some((None, None)));
}

#[test]
fn list_rejects_non_integer_limit() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.anim_layers", json!({ "limit": -2 }));
    let err = resp.error.expect("负数上限应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(
        err.message.contains("limit"),
        "错误信息应点名字段: {}",
        err.message
    );
    assert!(api.seen_list.is_none(), "非法请求不该送到宿主");
}

#[test]
fn set_parses_every_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({
            "instance": "hero/walk",
            "layer": "upper",
            "weight": 0.75,
            "mode": "additive",
            "mask": "arm,hand",
            "speed": 1.5,
            "clip": "atk"
        }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen_patch.expect("补丁应送达宿主");
    assert_eq!(patch.instance, "hero/walk");
    assert_eq!(patch.layer, "upper");
    assert_eq!(patch.weight, Some(0.75));
    assert_eq!(patch.mode.as_deref(), Some("additive"));
    assert_eq!(patch.mask.as_deref(), Some("arm,hand"));
    assert_eq!(patch.speed, Some(1.5));
    assert_eq!(patch.clip.as_deref(), Some("atk"));
}

#[test]
fn partial_patch_leaves_other_fields_untouched() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({ "instance": "hero/walk", "weight": 0.2 }),
    );
    assert!(resp.error.is_none(), "部分补丁应合法: {resp:?}");
    let patch = api.seen_patch.expect("补丁应送达宿主");
    assert_eq!(patch.weight, Some(0.2));
    // 没给的字段必须是 None，宿主据此「保留原值」。
    assert_eq!(patch.mode, None);
    assert_eq!(patch.mask, None);
    assert_eq!(patch.speed, None);
    assert_eq!(patch.clip, None);
}

#[test]
fn missing_layer_defaults_to_first_layer() {
    // `layer` 缺省 = 该实例第一层，与脚本侧「空名取第一层」保持一致，
    // 传输层不做补全，把空串原样交给宿主判定。
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({ "instance": "hero/walk", "speed": 2.0 }),
    );
    assert!(resp.error.is_none(), "缺 layer 应合法: {resp:?}");
    assert_eq!(api.seen_patch.expect("补丁应送达宿主").layer, "");
}

#[test]
fn method_name_is_normalized_both_ways() {
    for method in [
        "scene.animLayerSet",
        "scene.animlayer_set",
        "scene.ANIMLAYERSET",
    ] {
        let mut api = Mock::default();
        let resp = call(&mut api, method, json!({ "instance": "a", "weight": 1.0 }));
        assert!(
            resp.error.is_none(),
            "`{method}` 应命中同一条分支: {resp:?}"
        );
        assert!(api.seen_patch.is_some());
    }
    for method in ["scene.animLayers", "scene.ANIM_LAYERS"] {
        let mut api = Mock::default();
        let resp = call(&mut api, method, json!({}));
        assert!(
            resp.error.is_none(),
            "`{method}` 应命中同一条分支: {resp:?}"
        );
        assert!(api.seen_list.is_some());
    }
}

#[test]
fn empty_patch_is_rejected_before_reaching_host() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({ "instance": "hero/walk" }),
    );
    let err = resp.error.expect("空补丁应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.seen_patch.is_none(), "空补丁不该送到宿主");
}

#[test]
fn blank_or_missing_instance_is_rejected() {
    for params in [
        json!({ "weight": 1.0 }),
        json!({ "instance": "   ", "weight": 1.0 }),
    ] {
        let mut api = Mock::default();
        let resp = call(&mut api, "scene.anim_layer_set", params);
        let err = resp.error.expect("缺/空实例名应被拒");
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(
            err.message.contains("instance"),
            "错误信息应点名字段: {}",
            err.message
        );
        assert!(api.seen_patch.is_none());
    }
}

#[test]
fn wrong_type_is_reported_with_field_name() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({ "instance": "a", "speed": "fast" }),
    );
    let err = resp.error.expect("类型错误应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(
        err.message.contains("speed"),
        "错误信息应点名字段: {}",
        err.message
    );
    assert!(api.seen_patch.is_none());
}

#[test]
fn null_fields_are_treated_as_absent() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.anim_layer_set",
        json!({
            "instance": "a",
            "weight": Value::Null,
            "mask": Value::Null,
            "speed": 0.5
        }),
    );
    assert!(resp.error.is_none(), "显式 null 应视为未给: {resp:?}");
    let patch = api.seen_patch.expect("补丁应送达宿主");
    assert_eq!(patch.weight, None);
    assert_eq!(patch.mask, None);
    assert_eq!(patch.speed, Some(0.5));
}
