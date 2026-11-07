//! `scene.set_environment_map` 的传输层契约测试（扩展二十三）。
//!
//! 只验证「JSON → `EnvironmentMapPatch`」这一段，与 `environment_rpc.rs` 各管一条线：
//! 这里不外泄 `r3e_core` 的任何类型，路径也**不在这里解析成绝对路径**（宿主才知道
//! 场景文件在哪）。三条容易回退的行为用断言钉死：
//!
//! * 字段**可选**：只给 `intensity` 时其余保持 `None`，宿主据此「保留原值」；
//! * 空补丁拒收：一个字段都不给是无意义调用，必须回到 `-32602`；
//! * 未实现的宿主明确报 `-32601`，而不是假装成功——前端靠这个区分「后端不支持」。

use r3e_editor::{
    dispatch, EngineApi, EngineInfo, EnvironmentMapPatch, PhysicsInfo, Request, Response,
    INVALID_PARAMS, METHOD_NOT_FOUND,
};
use serde_json::{json, Value};

/// 把补丁记下来、原样回显的假宿主。
#[derive(Default)]
struct Mock {
    seen: Option<EnvironmentMapPatch>,
}

impl Mock {
    fn info_impl() -> EngineInfo {
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
}

impl EngineApi for Mock {
    fn info(&mut self) -> EngineInfo {
        Self::info_impl()
    }

    fn scene_set_environment_map(
        &mut self,
        patch: EnvironmentMapPatch,
    ) -> Result<Value, r3e_editor::RpcFailure> {
        self.seen = Some(patch);
        Ok(json!({ "enabled": true, "source": "equirect", "environment_map": {} }))
    }
}

/// 只实现 `info` 的宿主：其余方法走 trait 默认实现（unsupported）。
#[derive(Default)]
struct Bare;

impl EngineApi for Bare {
    fn info(&mut self) -> EngineInfo {
        Mock::info_impl()
    }
}

fn call<A: EngineApi>(api: &mut A, method: &str, params: Value) -> Response {
    let req = Request {
        jsonrpc: r3e_editor::JSONRPC.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    };
    dispatch(api, &req)
}

#[test]
fn equirect_patch_is_parsed_field_by_field() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_environment_map",
        json!({
            "enabled": true,
            "equirect": "env/studio.hdr",
            "face_size": 256,
            "mip_levels": 6,
            "intensity": 1.8
        }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.enabled, Some(true));
    assert_eq!(patch.equirect.as_deref(), Some("env/studio.hdr"));
    assert_eq!(patch.face_size, Some(256));
    assert_eq!(patch.mip_levels, Some(6));
    assert_eq!(patch.intensity, Some(1.8));
    // 没给 faces 就必须是 None：宿主据此保留原有六面图而不是清掉
    assert_eq!(patch.faces, None);
    assert!(!patch.is_empty());
}

#[test]
fn faces_keep_declared_order() {
    let mut api = Mock::default();
    let faces = ["px.hdr", "nx.hdr", "py.hdr", "ny.hdr", "pz.hdr", "nz.hdr"];
    let resp = call(
        &mut api,
        "scene.set_environment_map",
        json!({ "faces": faces }),
    );
    assert!(resp.error.is_none(), "不应报错: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    // 顺序即 +X/-X/+Y/-Y/+Z/-Z，传输层不得排序或去重——错位会直接换来一张拧起来的反射图
    assert_eq!(
        patch.faces.expect("faces 应原样保留"),
        faces.iter().map(|s| s.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(patch.equirect, None);
}

#[test]
fn method_name_is_normalized() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.setEnvironmentMap",
        json!({ "intensity": 0.5 }),
    );
    assert!(resp.error.is_none(), "归一化方法名应命中: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.intensity, Some(0.5));
    assert_eq!(patch.enabled, None, "未给的开关必须保持 None");
}

#[test]
fn disable_switch_is_carried_through() {
    let mut api = Mock::default();
    let resp = call(
        &mut api,
        "scene.set_environment_map",
        json!({ "enabled": false }),
    );
    assert!(resp.error.is_none(), "enabled=false 是合法调用: {resp:?}");
    let patch = api.seen.expect("补丁应送达宿主");
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(patch.equirect, None, "翻开关不得顺手清路径");
}

#[test]
fn empty_patch_is_rejected_before_reaching_host() {
    let mut api = Mock::default();
    for params in [json!({}), json!(null)] {
        let resp = call(&mut api, "scene.set_environment_map", params);
        let err = resp.error.expect("空补丁应被拒");
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(api.seen.is_none(), "空补丁不该送达宿主");
    }
}

#[test]
fn wrong_types_are_reported_as_invalid_params() {
    for (key, bad) in [
        ("enabled", json!("true")),
        ("equirect", json!(3)),
        ("faces", json!("px.hdr")),
        ("faces", json!([1, 2, 3])),
        ("face_size", json!("256")),
        ("mip_levels", json!(6.5)),
        ("intensity", json!("bright")),
    ] {
        let mut api = Mock::default();
        let resp = call(&mut api, "scene.set_environment_map", json!({ key: bad }));
        let err = resp.error.expect("类型错误应被拒");
        assert_eq!(
            err.code, INVALID_PARAMS,
            "`{key}` 的类型错误应报 invalid params"
        );
        assert!(api.seen.is_none());
    }
}

#[test]
fn unimplemented_host_reports_method_not_found() {
    let mut api = Bare;
    let resp = call(
        &mut api,
        "scene.set_environment_map",
        json!({ "enabled": true }),
    );
    let err = resp.error.expect("默认实现应报未实现");
    assert_eq!(
        err.code, METHOD_NOT_FOUND,
        "宿主没实现时要明确说「不支持」，不能静默成功"
    );
}
