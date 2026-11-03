//! `scene.set_ssao` 的传输层契约测试（扩展十七）。
//!
//! 只验证「JSON → `SsaoPatch`」这一段：字段缺省、类型错误、显式 null、空补丁拒绝、
//! 归一化后的方法名命中同一条分支。区间收敛（半径 / 偏置 / 强度 / 样本数夹紧）
//! 在 `r3e-core` 的 `SsaoSettings::sanitized` 里，由 `ssao_scene` 与宿主的
//! `scene_set_ssao` 测试覆盖——传输层不认识 `r3e_core` 的任何类型，所以这里不测区间。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, PhysicsInfo, Request, Response, SsaoPatch, INVALID_PARAMS,
};
use serde_json::{json, Value};

/// 只把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<SsaoPatch>,
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

    fn scene_set_ssao(&mut self, patch: SsaoPatch) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen = Some(patch.clone());
        Ok(json!({ "enabled": true, "echo": patch.radius }))
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
        "scene.set_ssao",
        json!({
            "enabled": true,
            "radius": 0.45,
            "bias": 0.03,
            "intensity": 1.5,
            "samples": 12
        }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.enabled, Some(true));
    assert_eq!(patch.radius, Some(0.45));
    assert_eq!(patch.bias, Some(0.03));
    assert_eq!(patch.intensity, Some(1.5));
    assert_eq!(patch.samples, Some(12));
}

#[test]
fn partial_patch_leaves_other_fields_untouched() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.setSSAO", json!({ "radius": 1.0 }));
    assert!(resp.error.is_none(), "归一化方法名应命中: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.radius, Some(1.0));
    // 没给的字段必须是 None，宿主据此「保留原值」。
    assert_eq!(patch.enabled, None);
    assert_eq!(patch.bias, None);
    assert_eq!(patch.intensity, None);
    assert_eq!(patch.samples, None);
}

#[test]
fn disable_only_touches_the_ssao_branch() {
    // 与 set_postfx 的关键差别：`enabled = false` 只是关掉遮蔽这一条支线，
    // 补丁里不会出现「清空整段」的语义位，所以宿主拿到的一定是 `Some(false)`。
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_ssao", json!({ "enabled": false }));
    assert!(resp.error.is_none(), "enabled=false 是合法请求: {resp:?}");
    assert_eq!(api.seen.expect("补丁应送达宿主").enabled, Some(false));
}

#[test]
fn null_field_is_treated_as_absent() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_ssao",
        json!({ "intensity": Value::Null, "samples": 6 }),
    );
    assert!(resp.error.is_none(), "显式 null 应视为未给: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.intensity, None);
    assert_eq!(patch.samples, Some(6));
}

#[test]
fn negative_samples_reach_host_untouched() {
    // 夹紧是宿主的活：传输层把 -3 原样送到，宿主统一夹到下限，
    // 好过这里静默改写成 0 让调用方查不出自己传错了。
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_ssao", json!({ "samples": -3 }));
    assert!(resp.error.is_none(), "负数样本数不该在传输层被拒: {resp:?}");
    assert_eq!(api.seen.expect("补丁应送达宿主").samples, Some(-3));
}

#[test]
fn empty_patch_is_rejected_before_reaching_host() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_ssao", json!({}));
    let err = resp.error.expect("空补丁应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.seen.is_none(), "空补丁不该送到宿主");
}

#[test]
fn wrong_type_is_reported_with_field_name() {
    let mut api = Mock::default();
    let resp = call(&mut api, "scene.set_ssao", json!({ "radius": "big" }));
    let err = resp.error.expect("类型错误应被拒");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(
        err.message.contains("radius"),
        "错误信息应点名字段: {}",
        err.message
    );
    assert!(api.seen.is_none(), "非法补丁不该送到宿主");
}
