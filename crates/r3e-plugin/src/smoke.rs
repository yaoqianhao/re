//! 打包自检：拉起**自己**当宿主，完整走一遍真实协议。
//!
//! 为什么不用单测代替：单测只能验证函数，验证不了「宿主 spawn 一个 exe、按行写帧、
//! 按行读回包」这条链路。打包脚本在组装完包之后会跑一次 `--smoke-test`，
//! 任何一步不符合规范就以非 0 退出，**产物不会被打上「可分发」的标签**。
//!
//! 自检覆盖：
//!
//! 1. 握手：`pluginApi.major == 1`，且工具集与清单完全一致（宿主 `verify()` 的硬条件）；
//! 2. 五个工具各跑一次，检查返回结构的关键字段；
//! 3. 命令面板入口跑一次；
//! 4. `plugin/shutdown` 后子进程必须以 0 退出（非 0 会被宿主记成崩溃）。
//!
//! 自检进程同时扮演**最小宿主**：插件反向发来的 `host/workspace.info`、
//! `host/fs.read` 由 [`Harness`] 就地应答。不这样做的话，`engine_diagnostics`
//! 会在等 `host/workspace.info` 回包时永久卡住——这正是第一版自检真实的翻车方式。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol;
use crate::spec;

/// 单条请求的回包等待上限：卡死要变成「失败」，不能变成「一直等」。
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// 自检用的最小但完整的场景：静态地面 + 两个动态刚体。
const SMOKE_SCENE: &str = r#"{
    "name": "smoke",
    "camera": { "position": [0, 2, 6], "target": [0, 1, 0], "fov": 60 },
    "light": { "direction": [-0.4, -1, -0.3], "intensity": 1.2 },
    "physics": { "enabled": true, "backend": "builtin", "gravity": [0, -9.81, 0], "ground": 0 },
    "objects": [
        { "name": "floor", "mesh": "builtin:plane", "scale": [8, 1, 8], "physics": { "body": "static" } },
        { "name": "crate", "mesh": "builtin:cube", "position": [0, 3, 0], "physics": { "body": "dynamic", "mass": 1 } },
        { "name": "ball", "mesh": "builtin:sphere", "position": [0.8, 4, 0], "physics": { "body": "dynamic", "mass": 0.5 } }
    ]
}"#;

/// 自检失败：直接带原因返回，由 `run` 打 stderr 并给非 0 退出码。
type Check = Result<(), String>;

pub fn run() -> i32 {
    match drive() {
        Ok(()) => {
            eprintln!("[smoke] 全部通过");
            0
        }
        Err(err) => {
            eprintln!("[smoke] 失败：{err}");
            1
        }
    }
}

fn drive() -> Check {
    let exe = std::env::current_exe().map_err(|err| format!("取自身路径失败：{err}"))?;
    eprintln!("[smoke] 拉起子进程 {}", exe.display());

    let mut child = Command::new(&exe)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("无法启动子进程：{err}"))?;

    let stdin = child.stdin.take().ok_or("拿不到子进程 stdin")?;
    let stdout = child.stdout.take().ok_or("拿不到子进程 stdout")?;
    // stderr 必须排空：管道写满后子进程会阻塞在写日志上，表现成「插件不响应」
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("  [plugin] {line}");
            }
        });
    }

    // stdout 交给读线程：主线程只从 channel 取帧，才能给「等回包」加上超时
    let (tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match protocol::read_frame(&mut reader) {
                Ok(Some(frame)) => {
                    if tx.send(frame).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(err) => {
                    eprintln!("  [smoke] 读帧出错，停止读取：{err}");
                    return;
                }
            }
        }
    });

    let mut harness = Harness {
        writer: stdin,
        frames,
        child,
        next_id: 0,
    };

    check_handshake(&mut harness)?;
    check_tools(&mut harness)?;
    check_command(&mut harness)?;

    // 优雅关闭：宿主发 plugin/shutdown，插件回包后自行退出
    harness.request("plugin/shutdown", json!({}))?;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match harness.child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    eprintln!("[smoke] 子进程退出码 0");
                    return Ok(());
                }
                return Err(format!("子进程退出码非 0：{status}"));
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = harness.child.kill();
                    return Err("plugin/shutdown 之后 10 秒仍未退出".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(format!("等待子进程失败：{err}")),
        }
    }
}

fn check_handshake(harness: &mut Harness) -> Check {
    let reply = harness.request("plugin/handshake", json!({}))?;

    let major = reply
        .get("pluginApi")
        .and_then(|api| api.get("major"))
        .and_then(Value::as_u64)
        .ok_or("握手回包缺 pluginApi.major（宿主会判协议不符）")?;
    if major != 1 {
        return Err(format!("pluginApi.major={major}，宿主只接受 1"));
    }

    let mut reported: Vec<String> = reply
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("握手回包缺 tools 数组")?
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    reported.sort();
    let mut expected = spec::tool_names();
    expected.sort();
    if reported != expected {
        return Err(format!(
            "握手工具集与清单不一致（宿主 verify 会拒装）：声明 {expected:?}，实际 {reported:?}"
        ));
    }

    // 命令集也一并核对：宿主装载后命令面板直接用这份
    let commands = reply
        .get("commands")
        .and_then(Value::as_array)
        .ok_or("握手回包缺 commands 数组（没有命令也应当是空数组）")?
        .iter()
        .filter_map(|command| command.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    for expected_command in spec::command_names() {
        if !commands.contains(&expected_command.as_str()) {
            return Err(format!("握手回包缺命令 `{expected_command}`"));
        }
    }

    eprintln!(
        "[smoke] 握手通过：{}，工具 {} 个，命令 {} 个",
        reply
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("?"),
        reported.len(),
        commands.len()
    );
    Ok(())
}

fn check_tools(harness: &mut Harness) -> Check {
    let scene = json!({ "scene": SMOKE_SCENE });

    let info = harness.invoke("engine_info", json!({}))?;
    if info["engine"]["name"] != json!("rust3d-engine") {
        return Err("engine_info 缺 engine.name".to_string());
    }
    if info["physics"]["backends"].as_array().map(Vec::len) != Some(3) {
        return Err("engine_info 应当列出 null / builtin / rapier 三个后端".to_string());
    }
    eprintln!("[smoke] engine_info 通过");

    let inspect = harness.invoke("scene_inspect", scene.clone())?;
    if inspect["nodeCount"] != json!(3) {
        return Err(format!(
            "scene_inspect 应看到 3 个节点，实际 {}",
            inspect["nodeCount"]
        ));
    }
    eprintln!("[smoke] scene_inspect 通过（3 个节点）");

    let mesh = harness.invoke("mesh_stats", json!({ "shape": "sphere", "segments": 12, "rings": 6 }))?;
    if mesh["vertices"].as_u64().unwrap_or(0) == 0 {
        return Err("mesh_stats 生成了空网格".to_string());
    }
    if mesh["air"]["ok"] != json!(true) {
        return Err(format!("内置球体不该有 AIR 问题：{}", mesh["air"]));
    }
    eprintln!(
        "[smoke] mesh_stats 通过（{} 顶点 / {} 三角）",
        mesh["vertices"], mesh["triangles"]
    );

    let validation = harness.invoke("scene_validate", scene.clone())?;
    if validation["ok"] != json!(true) {
        return Err(format!("示例场景应当校验通过：{validation}"));
    }
    eprintln!("[smoke] scene_validate 通过");

    // 走 path 分支：验证「插件 → host/fs.read → 宿主给内容」这条反向调用链
    let from_file = harness.invoke("scene_inspect", json!({ "path": "assets/smoke.r3e.json" }))?;
    if from_file["nodeCount"] != json!(3) {
        return Err(format!(
            "经 host/fs.read 读到的场景应有 3 个节点，实际 {}",
            from_file["nodeCount"]
        ));
    }
    eprintln!("[smoke] host/fs.read 通路通过");

    let simulation = harness.invoke("physics_simulate", scene)?;
    if simulation["ok"] != json!(true) {
        return Err("physics_simulate 未返回 ok".to_string());
    }
    if simulation["bodyCount"] != json!(3) {
        return Err(format!(
            "physics_simulate 应当登记 3 个刚体，实际 {}",
            simulation["bodyCount"]
        ));
    }
    let dropped = simulation["maxDisplacement"].as_f64().unwrap_or(0.0) > 0.0;
    if !dropped {
        return Err("动态刚体没有位移，重力/步进可能没生效".to_string());
    }
    eprintln!(
        "[smoke] physics_simulate 通过（后端 {}，最大位移 {}）",
        simulation["backend"], simulation["maxDisplacement"]
    );
    Ok(())
}

fn check_command(harness: &mut Harness) -> Check {
    let reply = harness.request(
        "plugin/command-invoke",
        json!({ "command": "engine_diagnostics", "arguments": {} }),
    )?;
    if reply["engine"]["engine"]["name"] != json!("rust3d-engine") {
        return Err("engine_diagnostics 未返回引擎信息".to_string());
    }
    // 自检环境没有宿主同一进程的工作区，workspace.available 允许为 false
    if reply["workspace"]["available"].is_null() {
        return Err("engine_diagnostics 缺 workspace.available".to_string());
    }
    eprintln!(
        "[smoke] engine_diagnostics 通过（工作区可用：{}）",
        reply["workspace"]["available"]
    );
    Ok(())
}

struct Harness {
    writer: std::process::ChildStdin,
    /// 读线程把帧送进来：主线程用 `recv_timeout` 等，卡死即失败而不是永远阻塞。
    frames: Receiver<Value>,
    child: Child,
    next_id: u64,
}

impl Harness {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = json!(self.next_id);
        protocol::write_frame(&mut self.writer, &protocol::request_frame(id.clone(), method, params))
            .map_err(|err| format!("写请求失败：{err}"))?;

        loop {
            let frame = match self.frames.recv_timeout(REPLY_TIMEOUT) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "`{method}` 等回包超过 {} 秒（插件可能在等宿主回话）",
                        REPLY_TIMEOUT.as_secs()
                    ))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("插件在回应 `{method}` 之前就关闭了 stdout"))
                }
            };
            // 反向调用：插件在问宿主能力，就地应答（真实宿主也在这条线上回话）
            if let Some(host_method) = protocol::method_of(&frame) {
                self.answer_host(host_method, &frame)?;
                continue;
            }
            if frame.get("id") != Some(&id) {
                // 迟到的回包，忽略
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Err(format!("`{method}` 返回错误：{error}"));
            }
            return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 最小宿主能力面：只实现本插件真正会用的两个方法。
    fn answer_host(&mut self, method: &str, frame: &Value) -> Result<(), String> {
        let Some(id) = protocol::id_of(frame) else {
            return Ok(()); // 通知无需回包
        };
        let reply = match method {
            // 真实宿主在没打开工作区时回 null，这里照抄这个口径
            "host/workspace.info" => protocol::result_frame(id, Value::Null),
            // fs.read 直接吐自检场景，用来验证「path 走宿主读文件」这条支路
            "host/fs.read" => protocol::result_frame(
                id,
                json!({
                    "content": SMOKE_SCENE,
                    "bytes": SMOKE_SCENE.len(),
                    "truncated": false,
                }),
            ),
            other => protocol::error_frame(id, -32601, format!("自检宿主不提供 {other}")),
        };
        protocol::write_frame(&mut self.writer, &reply)
            .map_err(|err| format!("回应宿主能力 {method} 失败：{err}"))
    }

    fn invoke(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "plugin/tool-invoke",
            json!({ "tool": tool, "arguments": arguments }),
        )
    }
}
