//! `tools` 的行为测试（M2-A05 自 `tools.rs` 内嵌测试块切出，逐条搬移、零删减）。

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

struct NoHost;

impl HostBridge for NoHost {
    fn workspace_info(&mut self) -> Result<Value, HostError> {
        Ok(Value::Null)
    }
}

/// 假引擎：监听回环端口，按顺序回 `replies`，返回 (端口, 收到的请求帧)。
///
/// 只读工具现在全是薄壳，「方法名 + 参数形状」就是插件与引擎之间唯一的契约；
/// 光看返回值验不出来——返回值可以来自本地，也可以来自引擎。
fn fake_engine(replies: Vec<Value>) -> (u16, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let port = listener.local_addr().expect("本地地址").port();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        if let Ok((stream, _)) = listener.accept() {
            let mut writer = stream.try_clone().expect("复制连接");
            let mut reader = BufReader::new(stream);
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("读请求") == 0 {
                    break;
                }
                let request: Value =
                    serde_json::from_str(line.trim_end()).expect("请求是合法 JSON");
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                seen.push(request);
                let frame = json!({ "jsonrpc": "2.0", "id": id, "result": reply }).to_string();
                writer.write_all(frame.as_bytes()).expect("写响应");
                writer.write_all(b"\n").expect("换行");
                writer.flush().expect("刷新");
            }
        }
        seen
    });
    (port, handle)
}

/// 假引擎：只回一帧错误，用来验「引擎错误码原样透传」。
fn fake_engine_error(code: i64, message: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let port = listener.local_addr().expect("本地地址").port();
    let message = message.to_string();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut writer = stream.try_clone().expect("复制连接");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            if reader.read_line(&mut line).expect("读请求") > 0 {
                let request: Value =
                    serde_json::from_str(line.trim_end()).expect("请求是合法 JSON");
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                let frame = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code, "message": message },
                })
                .to_string();
                writer.write_all(frame.as_bytes()).expect("写响应");
                writer.write_all(b"\n").expect("换行");
                writer.flush().expect("刷新");
            }
        }
    });
    port
}

fn with_engine(port: u16, mut args: Value) -> Value {
    args["endpoint"] = json!({ "addr": format!("127.0.0.1:{port}") });
    args
}

#[test]
fn engine_info_lists_backends_and_tools_without_faking_availability() {
    let info = invoke("engine_info", &json!({})).expect("engine_info");
    assert_eq!(info["engine"]["name"], json!("rust3d-engine"));
    assert_eq!(info["plugin"]["id"], json!(crate::spec::PLUGIN_ID));
    assert_eq!(info["plugin"]["compute"], json!("engine-rpc"));
    let backends = info["physics"]["backends"].as_array().expect("backends");
    assert!(backends.iter().any(|item| item["name"] == json!("builtin")));
    assert!(backends.iter().any(|item| item["name"] == json!("rapier")));
    // 后端可用性只有引擎知道：插件自述里不允许出现凭空断言的 available。
    for backend in backends {
        let available = backend.get("available");
        assert!(
            available.is_none() || available.map(Value::is_null).unwrap_or(false),
            "插件不再链物理 crate，不得凭空断言可用性：{backend}"
        );
    }
    assert_eq!(info["meshes"].as_array().expect("meshes").len(), 4);
}

#[test]
fn scene_inspect_is_a_shell_over_scene_snapshot() {
    let (port, handle) = fake_engine(vec![json!({ "nodeCount": 2, "listedCount": 2 })]);
    let args = with_engine(
        port,
        json!({
            "scene": { "name": "demo", "objects": [ { "name": "floor", "mesh": "builtin:plane" } ] },
            "maxNodes": 1
        }),
    );
    let report = invoke("scene_inspect", &args).expect("scene_inspect");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["method"], json!("scene.snapshot"));
    assert_eq!(seen[0]["params"]["maxNodes"], json!(1));
    // 内联对象必须序列化成**文本**再下行：引擎侧只实现一条解析路径。
    let scene = seen[0]["params"]["scene"]
        .as_str()
        .expect("scene 应以文本形态下行");
    let parsed: Value = serde_json::from_str(scene).expect("序列化后仍是合法 JSON");
    assert_eq!(parsed["name"], json!("demo"));
    // 引擎回什么就回什么，插件不再二次加工。
    assert_eq!(report["nodeCount"], json!(2));
}

#[test]
fn scene_source_defaults_to_current_scene() {
    let (port, handle) = fake_engine(vec![json!({})]);
    let args = with_engine(port, json!({}));
    invoke("scene_validate", &args).expect("scene_validate");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen[0]["method"], json!("scene.validate"));
    // 一条来源都不给时不要凭空塞 `scene: null`：宿主会把它当成「给了空文本」。
    let params = seen[0]["params"].as_object().expect("params 是对象");
    assert!(!params.contains_key("scene"));
    assert!(!params.contains_key("path"));
}

#[test]
fn path_is_forwarded_as_is() {
    let (port, handle) = fake_engine(vec![json!({ "ok": true })]);
    let args = with_engine(port, json!({ "path": "scenes/demo.json" }));
    invoke("scene_validate", &args).expect("scene_validate");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen[0]["params"]["path"], json!("scenes/demo.json"));
}

#[test]
fn mesh_stats_forwards_tessellation_to_engine() {
    let (port, handle) = fake_engine(vec![json!({ "shape": "sphere", "vertices": 42 })]);
    let args = with_engine(
        port,
        json!({ "shape": "sphere", "segments": 8, "rings": 4 }),
    );
    let report = invoke("mesh_stats", &args).expect("mesh_stats");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen[0]["method"], json!("mesh.stats"));
    assert_eq!(seen[0]["params"]["shape"], json!("sphere"));
    assert_eq!(seen[0]["params"]["segments"], json!(8));
    assert_eq!(seen[0]["params"]["rings"], json!(4));
    assert_eq!(report["vertices"], json!(42));
}

#[test]
fn mesh_stats_rejects_unknown_shape_before_connecting() {
    // 端口 1 上不会有引擎：能拿到参数错就证明拒绝发生在连接之前。
    let error = invoke(
        "mesh_stats",
        &json!({ "shape": "teapot", "endpoint": { "port": 1 } }),
    )
    .expect_err("未知图元必须报错");
    assert_eq!(error.code, -32602);
    assert!(
        error.message.contains("cube"),
        "报错要列出可选形状：{}",
        error.message
    );
}

#[test]
fn physics_simulate_forwards_simulation_params() {
    let (port, handle) = fake_engine(vec![json!({ "ok": true, "bodyCount": 2 })]);
    let args = with_engine(
        port,
        json!({
            "scene": { "name": "demo", "objects": [] },
            "steps": 12,
            "dt": 0.02,
            "maxBodies": 3,
            "backend": "builtin",
            "gravity": [0.0, -9.81, 0.0],
            "ground": -0.5
        }),
    );
    let report = invoke("physics_simulate", &args).expect("physics_simulate");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen[0]["method"], json!("physics.simulate"));
    let params = &seen[0]["params"];
    assert_eq!(params["steps"], json!(12));
    assert_eq!(params["maxBodies"], json!(3));
    assert_eq!(params["backend"], json!("builtin"));
    assert_eq!(params["gravity"], json!([0.0, -9.81, 0.0]));
    let dt = params["dt"].as_f64().expect("dt");
    assert!((dt - 0.02).abs() < 1e-6, "dt 应为 0.02，实际 {dt}");
    let ground = params["ground"].as_f64().expect("ground");
    assert!(
        (ground + 0.5).abs() < 1e-6,
        "ground 应为 -0.5，实际 {ground}"
    );
    assert!(params["scene"].is_string());
    assert_eq!(report["bodyCount"], json!(2));
}

#[test]
fn physics_simulate_rejects_unknown_backend_before_connecting() {
    let error = invoke(
        "physics_simulate",
        &json!({ "scene": "{}", "backend": "bullet", "endpoint": { "port": 1 } }),
    )
    .expect_err("未知后端必须报错");
    assert_eq!(error.code, -32602);
    assert!(error.message.contains("builtin"), "{}", error.message);
}

#[test]
fn read_only_tools_report_missing_engine_as_actionable_error() {
    for tool in [
        "scene_inspect",
        "mesh_stats",
        "scene_validate",
        "physics_simulate",
    ] {
        let error =
            invoke(tool, &json!({ "endpoint": { "port": 1 } })).expect_err("端口 1 上不该有引擎");
        assert_eq!(error.code, -32011, "{tool}");
        assert!(
            error.message.contains("引擎"),
            "{tool} 的报错要点明引擎：{}",
            error.message
        );
    }
}

#[test]
fn engine_rejection_keeps_its_code_and_names_the_method() {
    let port = fake_engine_error(-32004, "场景里没有节点 `hero`");
    let args = with_engine(port, json!({ "scene": "{}" }));
    let error = invoke("scene_inspect", &args).expect_err("引擎拒绝");
    assert_eq!(error.code, -32004, "引擎错误码必须原样透传");
    assert!(
        error.message.contains("scene.snapshot"),
        "报错要点明是哪条方法：{}",
        error.message
    );
}

#[test]
fn wrong_argument_type_is_rejected() {
    let error =
        invoke("scene_inspect", &json!({ "scene": {}, "maxNodes": "many" })).expect_err("类型错");
    assert_eq!(error.code, -32602);
}

#[test]
fn scene_must_be_object_or_text() {
    let error = invoke("scene_validate", &json!({ "scene": 42 })).expect_err("scene 类型不对");
    assert_eq!(error.code, -32602);
    assert!(error.message.contains("scene"), "{}", error.message);
}

#[test]
fn unknown_tool_is_rejected() {
    let error = invoke("nope", &json!({})).expect_err("未知工具");
    assert_eq!(error.code, -32602);
}

#[test]
fn diagnostics_command_tolerates_missing_workspace() {
    let report = invoke_command("engine_diagnostics", &mut NoHost).expect("diagnostics");
    assert_eq!(report["engine"]["engine"]["name"], json!("rust3d-engine"));
    assert_eq!(report["workspace"]["available"], json!(false));
}

// —— P0-4 反馈工具：参数校验必须在「连引擎之前」就拦下 ——

#[test]
fn capture_frame_rejects_non_png_suffix() {
    // 引擎写的是 PNG 字节；后缀给 .jpg 只会产出一个名不副实的文件，
    // 这种错要在插件侧直接报参数错，而不是等用户打开图片才发现。
    for bad in ["a.jpg", "b.png.jpg", "c.jpeg", "d"] {
        let error =
            invoke("capture_frame", &json!({ "path": bad })).expect_err("非 PNG 后缀应被拒绝");
        assert_eq!(error.code, -32602, "path={bad}");
        assert!(
            error.message.contains(".png"),
            "报错应点明要 .png，实际：{}",
            error.message
        );
    }
}

#[test]
fn capture_frame_rejects_blank_and_directory_paths() {
    for bad in ["", "   ", "captures/", "captures\\"] {
        let error =
            invoke("capture_frame", &json!({ "path": bad })).expect_err("空白或目录路径应被拒绝");
        assert_eq!(error.code, -32602, "path={bad:?}");
    }
    // 合法路径不该被参数校验拦下：应当越过校验、死在「连不上引擎」这一步，
    // 以此证明拒绝发生在连接之前而不是「反正连不上都报错」。
    let error = invoke(
        "capture_frame",
        &json!({ "path": "D:/tmp/shot.PNG", "endpoint": { "port": 1 } }),
    )
    .expect_err("端口 1 上不该有引擎");
    assert_ne!(error.code, -32602, "合法路径不应报参数错：{error:?}");
}

#[test]
fn engine_stats_reports_missing_engine_as_actionable_error() {
    let error = invoke("engine_stats", &json!({ "endpoint": { "port": 1 } }))
        .expect_err("端口 1 上不该有引擎");
    // 错误必须告诉调用方「去找引擎」，而不是含糊的 IO 失败
    assert!(
        error.message.contains("引擎") || error.message.contains("RPC"),
        "报错应指出引擎连接问题，实际：{}",
        error.message
    );
}

#[test]
fn scene_validate_forwards_inline_text_without_rewriting_it() {
    // 已是文本就原样下行：插件不再 `parse` 一遍，否则「文本」与「对象」两条路
    // 会在引擎侧给出两套不同的错误文案。
    let text = r#"{"name":"lv1","objects":[]}"#;
    let (port, handle) = fake_engine(vec![json!({ "ok": true })]);
    let args = with_engine(port, json!({ "scene": text }));
    invoke("scene_validate", &args).expect("scene_validate");
    let seen = handle.join().expect("假引擎线程");
    assert_eq!(seen[0]["method"], json!("scene.validate"));
    assert_eq!(seen[0]["params"]["scene"], json!(text));
}

#[test]
fn scene_source_rejects_non_text_and_non_object_scene() {
    // 端口 1 上不会有引擎：拿到参数错就证明拒绝发生在连接之前。
    for bad in [json!(3), json!([1, 2]), json!(true)] {
        let error = invoke(
            "scene_inspect",
            &json!({ "scene": bad, "endpoint": { "port": 1 } }),
        )
        .expect_err("scene 只认场景对象或场景 JSON 文本");
        assert_eq!(error.code, -32602, "scene={bad}");
        assert!(
            error.message.contains("scene"),
            "报错要点名 scene，实际：{}",
            error.message
        );
    }
}

#[test]
fn scene_inspect_omits_max_nodes_when_absent() {
    // 没给就是没给：插件不替引擎补默认值，否则两边的缺省值迟早会分叉。
    let (port, handle) = fake_engine(vec![json!({})]);
    let args = with_engine(port, json!({ "scene": "{}" }));
    invoke("scene_inspect", &args).expect("scene_inspect");
    let seen = handle.join().expect("假引擎线程");
    let params = seen[0]["params"].as_object().expect("params 是对象");
    assert!(!params.contains_key("maxNodes"), "{params:?}");
}

#[test]
fn mesh_stats_omits_absent_tessellation_params() {
    let (port, handle) = fake_engine(vec![json!({})]);
    let args = with_engine(port, json!({ "shape": "sphere" }));
    invoke("mesh_stats", &args).expect("mesh_stats");
    let seen = handle.join().expect("假引擎线程");
    let params = seen[0]["params"].as_object().expect("params 是对象");
    for key in ["segments", "rings", "divisions"] {
        assert!(!params.contains_key(key), "不该补 `{key}`：{params:?}");
    }
}

#[test]
fn physics_simulate_keeps_engine_error_code_and_names_method() {
    let port = fake_engine_error(-32004, "场景里没有节点 `hero`");
    let args = with_engine(port, json!({ "scene": "{}" }));
    let error = invoke("physics_simulate", &args).expect_err("引擎拒绝");
    assert_eq!(error.code, -32004, "引擎错误码必须原样透传");
    assert!(
        error.message.contains("physics.simulate"),
        "报错要点名方法，实际：{}",
        error.message
    );
}

#[test]
fn gravity_must_be_exactly_three_numbers() {
    let error = invoke(
        "physics_simulate",
        &json!({ "scene": "{}", "gravity": [0.0, -9.81], "endpoint": { "port": 1 } }),
    )
    .expect_err("gravity 少一个分量必须报错");
    assert_eq!(error.code, -32602);
    assert!(
        error.message.contains("gravity") && error.message.contains('3'),
        "报错要点明需要 3 个分量，实际：{}",
        error.message
    );
}
