//! 打包自检：拉起**自己**当宿主，完整走一遍真实协议。
//!
//! 为什么不用单测代替：单测只能验证函数，验证不了「宿主 spawn 一个 exe、按行写帧、
//! 按行读回包」这条链路。打包脚本在组装完包之后会跑一次 `--smoke-test`，
//! 任何一步不符合规范就以非 0 退出，**产物不会被打上「可分发」的标签**。
//!
//! 自检覆盖：
//!
//! 1. 握手：`pluginApi.major == 1`，且工具集与清单完全一致（宿主 `verify()` 的硬条件）；
//! 2. 自述：`engine_info` 的后端表齐全，且 `compute == "engine-rpc"`；
//! 3. 薄壳（M1-S1-D）：四个只读计算工具在**没有引擎**时必须报 -32011，
//!    而不是在插件进程里算出一份结果——插件已经不链 `r3e-core` / `r3e-physics`；
//! 4. 参数前置：未知图元 / 未知物理后端要在**连引擎之前**就报 -32602；
//! 5. 写侧工具（P0-3）：无引擎时必须给出 -32602 / -32011 与补救提示；
//! 6. 命令面板入口跑一次；
//! 7. `plugin/shutdown` 后子进程必须以 0 退出（非 0 会被宿主记成崩溃）。
//!
//! 自检进程同时扮演**最小宿主**：插件反向发来的 `host/workspace.info` 由
//! [`Harness`] 就地应答。不这样做的话，`engine_diagnostics` 会在等
//! `host/workspace.info` 回包时永久卡住——这正是第一版自检真实的翻车方式。
//! 插件已不再反调 `host/fs.read`（读盘交给引擎），自检顺手断言这一点：
//! 一次自检跑完，宿主侧不该收到任何 `host/fs.read` 请求。

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
        host_methods: Vec::new(),
    };

    check_handshake(&mut harness)?;
    check_tools(&mut harness)?;
    check_write_tools(&mut harness)?;
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
        reply.get("version").and_then(Value::as_str).unwrap_or("?"),
        reported.len(),
        commands.len()
    );
    Ok(())
}

fn check_tools(harness: &mut Harness) -> Check {
    let info = harness.invoke("engine_info", json!({}))?;
    if info["engine"]["name"] != json!("rust3d-engine") {
        return Err("engine_info 缺 engine.name".to_string());
    }
    if info["physics"]["backends"].as_array().map(Vec::len) != Some(3) {
        return Err("engine_info 应当列出 null / builtin / rapier 三个后端".to_string());
    }
    // 插件不链物理 crate，后端**可用性**只有引擎知道：自述里不许出现断言。
    for backend in info["physics"]["backends"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        if backend
            .get("available")
            .map(|value| !value.is_null())
            .unwrap_or(false)
        {
            return Err(format!("插件不该断言后端可用性：{backend}"));
        }
    }
    if info["plugin"]["compute"] != json!("engine-rpc") {
        return Err(format!(
            "只读计算必须全部在引擎侧做，自述却是 {}",
            info["plugin"]["compute"]
        ));
    }
    eprintln!(
        "[smoke] engine_info 通过（3 个后端名，compute={}）",
        info["plugin"]["compute"]
    );

    // 四个只读工具是**薄壳**：引擎不在时必须报 -32011。若哪天它们又能离线算出结果，
    // 说明 core / physics 的依赖偷偷回来了，这条断言就是那道闸。
    for tool in [
        "scene_inspect",
        "mesh_stats",
        "scene_validate",
        "physics_simulate",
    ] {
        let offline = harness.invoke_err(
            tool,
            json!({ "scene": SMOKE_SCENE, "endpoint": { "port": 1 } }),
        )?;
        if offline["code"] != json!(-32011) {
            return Err(format!(
                "{tool} 在没有引擎时应当是 -32011（引擎侧计算），实际 {offline}"
            ));
        }
        let message = offline["message"].as_str().unwrap_or_default();
        if !message.contains("--rpc-port") {
            return Err(format!("{tool} 连不上引擎时没告诉用户怎么修：{message}"));
        }
    }
    eprintln!("[smoke] 四个只读工具都是引擎薄壳（无引擎即 -32011）");

    // `path` 原样下发，插件自己不读盘：给 path 却连不上引擎时，同样只能报 -32011。
    let by_path = harness.invoke_err(
        "scene_inspect",
        json!({ "path": "assets/smoke.r3e.json", "endpoint": { "port": 1 } }),
    )?;
    if by_path["code"] != json!(-32011) {
        return Err(format!("path 应当原样交给引擎解析，实际 {by_path}"));
    }
    if harness
        .host_methods
        .iter()
        .any(|method| method == "host/fs.read")
    {
        return Err(format!(
            "插件已不持有读盘逻辑，却仍在反调 host/fs.read：{:?}",
            harness.host_methods
        ));
    }
    eprintln!("[smoke] path 直通引擎，宿主未收到 host/fs.read");

    // 参数前置仍然在插件侧：形状名 / 后端名写错时不必连引擎就能报错。
    let bad_shape = harness.invoke_err(
        "mesh_stats",
        json!({ "shape": "teapot", "endpoint": { "port": 1 } }),
    )?;
    if bad_shape["code"] != json!(-32602) {
        return Err(format!(
            "未知图元必须报 -32602 参数错（连接之前拦下），实际 {bad_shape}"
        ));
    }
    let bad_backend = harness.invoke_err(
        "physics_simulate",
        json!({ "scene": SMOKE_SCENE, "backend": "bullet", "endpoint": { "port": 1 } }),
    )?;
    if bad_backend["code"] != json!(-32602) {
        return Err(format!(
            "未知物理后端必须报 -32602（连接之前拦下），实际 {bad_backend}"
        ));
    }
    eprintln!("[smoke] 参数前置通过（未知图元 / 未知后端 → -32602）");
    Ok(())
}

/// P0-3：写侧工具在**没有引擎**时的行为必须可诊断，而不是含糊失败。
///
/// 三条断言分别覆盖三种失败姿势：参数缺字段（-32602）、字段名拼错（-32602，
/// 不静默忽略）、引擎连不上（-32011 + 「用 --rpc-port 启动」的补救提示）。
fn check_write_tools(harness: &mut Harness) -> Check {
    let missing = harness.invoke_err("scene_patch", json!({ "position": [0, 1, 0] }))?;
    if missing["code"] != json!(-32602) {
        return Err(format!(
            "scene_patch 缺 node 应当是 -32602 参数错，实际 {missing}"
        ));
    }

    let typo = harness.invoke_err(
        "scene_patch",
        json!({ "node": "crate", "material": { "metalness": 0.5 } }),
    )?;
    let typo_msg = typo["message"].as_str().unwrap_or_default();
    if typo["code"] != json!(-32602) || !typo_msg.contains("metalness") {
        return Err(format!(
            "材质字段拼错必须报错并指出字段名（否则模型以为改成功了）：{typo}"
        ));
    }

    // 127.0.0.1:1 上不会有引擎；连不上时必须给「怎么起引擎」的提示
    let offline = harness.invoke_err(
        "scene_patch",
        json!({
            "node": "crate",
            "position": [0, 2, 0],
            "endpoint": { "addr": "127.0.0.1:1" }
        }),
    )?;
    if offline["code"] != json!(-32011) {
        return Err(format!(
            "引擎连不上应当是 -32011（去启动引擎），实际 {offline}"
        ));
    }
    let offline_msg = offline["message"].as_str().unwrap_or_default();
    if !offline_msg.contains("--rpc-port") {
        return Err(format!("连不上引擎时没告诉用户怎么修：{offline_msg}"));
    }

    let blank = harness.invoke_err("script_write", json!({ "source": "   " }))?;
    if blank["code"] != json!(-32602) {
        return Err(format!("空白脚本必须被拒，实际 {blank}"));
    }
    eprintln!("[smoke] 写侧工具失败路径通过（-32602 / -32011 + 补救提示）");
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
    /// 插件反向调过来的宿主方法名，用来断言「插件不再读盘」。
    host_methods: Vec<String>,
}

impl Harness {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.exchange(method, params)?
            .map_err(|err| format!("`{method}` 返回错误：{err}"))
    }

    /// 走一遍完整往返，但把「协议错误帧」当成**正常结果**返回。
    ///
    /// 写侧工具在没有引擎时必须报错，自检要断言的是错误内容（错误码 + 补救提示），
    /// 所以不能直接调用 [`Self::request`]——那会把错误变成 `Err` 丢掉细节。
    fn invoke_err(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        let params = json!({ "tool": tool, "arguments": arguments });
        match self.exchange("plugin/tool-invoke", params)? {
            Ok(value) => Err(format!("`{tool}` 本应报错却成功返回：{value}")),
            Err(err) => Ok(err),
        }
    }

    fn exchange(&mut self, method: &str, params: Value) -> Result<Result<Value, Value>, String> {
        self.next_id += 1;
        let id = json!(self.next_id);
        protocol::write_frame(
            &mut self.writer,
            &protocol::request_frame(id.clone(), method, params),
        )
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
                return Ok(Err(error.clone()));
            }
            return Ok(Ok(frame.get("result").cloned().unwrap_or(Value::Null)));
        }
    }

    /// 最小宿主能力面：只实现本插件真正会用的那个方法。
    fn answer_host(&mut self, method: &str, frame: &Value) -> Result<(), String> {
        self.host_methods.push(method.to_string());
        let Some(id) = protocol::id_of(frame) else {
            return Ok(()); // 通知无需回包
        };
        let reply = match method {
            // 真实宿主在没打开工作区时回 null，这里照抄这个口径
            "host/workspace.info" => protocol::result_frame(id, Value::Null),
            // 读盘已交给引擎：`host/fs.read` 不该再出现，真来了就按「不提供」回，
            // 让自检里那条 host_methods 断言有机会抓到它。
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
