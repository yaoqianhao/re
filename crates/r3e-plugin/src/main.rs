//! `r3e-plugin`：把 rust3d-engine 挂成 ai_ide 插件的**原生子进程入口**。
//!
//! ## 为什么是「原生 exe + NDJSON over stdio」
//!
//! ai_ide 的插件规范（`docs/14-插件系统设计.md`、`crates/plugin/src/loader.rs`）
//! 只认两种入口：`.mjs/.js/.cjs` 走 `node` 解释器，其它（`.exe`）直接 `spawn`。
//! 没有 dylib 的 ABI 契约、也没有 Wasm 宿主，所以这里选原生 exe——零运行时依赖，
//! 也不需要为 ABI 稳定性做 `extern "C"` 封装。
//!
//! ## 进程内的三段职责
//!
//! 1. [`protocol`]：一行一个 JSON 帧（上限 1 MiB）；
//! 2. [`host`]：反向调用 `host/fs.read` 等宿主能力（同步串行）；
//! 3. [`tools`]：真正的引擎能力（场景解析 / 网格统计 / AIR 校验 / 物理步进）。
//!
//! ## 子命令
//!
//! * 无参数或 `--serve`：进入服务模式（宿主就是这么启的），stdout **只准出协议帧**；
//! * `--emit-manifest`：把 `plugin.json` 打到 stdout，打包脚本用它生成清单，
//!   保证清单与二进制里握手声明的那张工具表永远是同一份；
//! * `--smoke-test`：拉起自己当宿主，跑一遍握手 + 全部工具 + 命令 + 关闭。
//! * `--help`：用法。
//!
//! 所有诊断信息一律走 stderr：stdout 被协议独占，混一行日志宿主就判协议违规。

mod host;
mod protocol;
mod smoke;
mod spec;
mod tools;

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

use host::{HostClient, HostError, Transport};
use tools::{HostBridge, ToolError};

/// 协议违规退出码：宿主侧看到非 0 退出会记一次崩溃并尝试重启（最多 3 次）。
const EXIT_PROTOCOL_VIOLATION: i32 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("--serve") => serve(),
        Some("--emit-manifest") => emit_manifest(),
        Some("--smoke-test") => smoke::run(),
        Some("--help") | Some("-h") => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("[r3e-plugin] 未知参数 `{other}`；用 --help 查看用法");
            EXIT_PROTOCOL_VIOLATION
        }
    };
    std::process::exit(code);
}

fn print_help() {
    println!("r3e-plugin {}（ai_ide 插件，pluginApi {}）", env!("CARGO_PKG_VERSION"), spec::PLUGIN_API);
    println!("用法：");
    println!("  r3e-plugin                 以服务模式运行（NDJSON over stdio，宿主调用方式）");
    println!("  r3e-plugin --emit-manifest 输出 plugin.json 到 stdout");
    println!("  r3e-plugin --smoke-test    自检：握手 + 全工具 + 命令 + 关闭");
    println!("  r3e-plugin --help          本帮助");
}

fn emit_manifest() -> i32 {
    match serde_json::to_string_pretty(&spec::manifest_json()) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("[r3e-plugin] 序列化清单失败：{err}");
            1
        }
    }
}

fn serve() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = Server {
        reader: BufReader::new(stdin.lock()),
        writer: stdout.lock(),
        client: HostClient::new(),
    };
    server.run()
}

/// 服务循环的走向。
enum Flow {
    Continue,
    Shutdown,
}

/// 一对管道：把「读 stdin / 写 stdout」包成 [`Transport`]，供反向调用复用。
struct Pipes<'a, R: BufRead, W: Write> {
    reader: &'a mut R,
    writer: &'a mut W,
}

impl<R: BufRead, W: Write> Transport for Pipes<'_, R, W> {
    fn send(&mut self, frame: &Value) -> Result<(), String> {
        protocol::write_frame(self.writer, frame).map_err(|err| err.to_string())
    }

    fn recv(&mut self) -> Result<Option<Value>, String> {
        protocol::read_frame(self.reader).map_err(|err| err.to_string())
    }
}

/// 把宿主能力接到 [`tools::HostBridge`] 上（工具层只看到 trait，不碰 IO 细节）。
struct Bridge<'a, R: BufRead, W: Write> {
    pipes: Pipes<'a, R, W>,
    client: &'a mut HostClient,
}

impl<R: BufRead, W: Write> HostBridge for Bridge<'_, R, W> {
    fn fs_read(&mut self, path: &str) -> Result<String, HostError> {
        self.client.fs_read(&mut self.pipes, path, None)
    }

    fn workspace_info(&mut self) -> Result<Value, HostError> {
        self.client.workspace_info(&mut self.pipes)
    }
}

struct Server<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    client: HostClient,
}

impl<R: BufRead, W: Write> Server<R, W> {
    fn run(&mut self) -> i32 {
        loop {
            let frame = match protocol::read_frame(&mut self.reader) {
                Ok(Some(frame)) => frame,
                // stdin 关了 = 宿主退出，正常收摊
                Ok(None) => return 0,
                Err(err) => {
                    eprintln!("[r3e-plugin] 读帧失败：{err}");
                    return EXIT_PROTOCOL_VIOLATION;
                }
            };
            match self.handle(frame) {
                Ok(Flow::Continue) => {}
                Ok(Flow::Shutdown) => return 0,
                Err(err) => {
                    eprintln!("[r3e-plugin] {err}");
                    return EXIT_PROTOCOL_VIOLATION;
                }
            }
        }
    }

    fn handle(&mut self, frame: Value) -> Result<Flow, String> {
        let Some(method) = protocol::method_of(&frame).map(str::to_string) else {
            // 响应帧：本插件的反向调用是同步的，走到这里说明宿主多发了东西，忽略即可。
            return Ok(Flow::Continue);
        };
        let params = frame.get("params").cloned().unwrap_or(Value::Null);

        let Some(id) = protocol::id_of(&frame) else {
            // 通知：宿主只发插件订阅过的事件，本插件不订阅，忽略。
            eprintln!("[r3e-plugin] 忽略通知 {method}");
            return Ok(Flow::Continue);
        };

        let (outcome, flow) = match method.as_str() {
            // 宿主装载时会先 verify 握手，再放行工具调用
            "plugin/handshake" | "plugin/initialize" => (Ok(spec::handshake_json()), Flow::Continue),
            "plugin/tool-invoke" => (self.tool_invoke(&params), Flow::Continue),
            "plugin/command-invoke" => (self.command_invoke(&params), Flow::Continue),
            "plugin/event" => (Ok(json!({ "ok": true })), Flow::Continue),
            "plugin/shutdown" => (Ok(json!({ "ok": true })), Flow::Shutdown),
            other => (
                Err(ToolError::invalid_params(format!(
                    "未知方法 `{other}`；支持 plugin/handshake、plugin/tool-invoke、plugin/command-invoke、plugin/event、plugin/shutdown"
                ))),
                Flow::Continue,
            ),
        };

        let reply = match outcome {
            Ok(result) => protocol::result_frame(id, result),
            Err(err) => protocol::error_frame(id, err.code, err.message),
        };
        protocol::write_frame(&mut self.writer, &reply).map_err(|err| err.to_string())?;
        Ok(flow)
    }

    fn tool_invoke(&mut self, params: &Value) -> Result<Value, ToolError> {
        let name = params
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid_params("plugin/tool-invoke 缺少 tool 字段"))?
            .to_string();
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);

        // 宿主配额按「一次工具调用」计，每次调用前归零
        self.client.begin_call();
        let mut bridge = Bridge {
            pipes: Pipes {
                reader: &mut self.reader,
                writer: &mut self.writer,
            },
            client: &mut self.client,
        };
        tools::invoke(&name, &args, &mut bridge)
    }

    fn command_invoke(&mut self, params: &Value) -> Result<Value, ToolError> {
        let name = params
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid_params("plugin/command-invoke 缺少 command 字段"))?
            .to_string();

        self.client.begin_call();
        let mut bridge = Bridge {
            pipes: Pipes {
                reader: &mut self.reader,
                writer: &mut self.writer,
            },
            client: &mut self.client,
        };
        tools::invoke_command(&name, &mut bridge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 用内存管道跑一遍服务循环：输入若干帧，取回输出帧。
    fn run_frames(input: &[Value]) -> Vec<Value> {
        let mut text = String::new();
        for frame in input {
            text.push_str(&serde_json::to_string(frame).expect("序列化帧"));
            text.push('\n');
        }
        let mut server = Server {
            reader: Cursor::new(text.into_bytes()),
            writer: Vec::<u8>::new(),
            client: HostClient::new(),
        };
        let code = server.run();
        assert_eq!(code, 0, "正常关闭应当是 0");
        String::from_utf8(server.writer)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("合法 JSON 帧"))
            .collect()
    }

    #[test]
    fn handshake_reports_manifest_tools() {
        let replies = run_frames(&[json!({
            "jsonrpc": "2.0",
            "id": "h1",
            "method": "plugin/handshake",
            "params": {}
        })]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], json!("h1"));
        let names: Vec<String> = replies[0]["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(names, spec::tool_names());
        assert_eq!(replies[0]["result"]["pluginApi"]["major"], json!(1));
    }

    #[test]
    fn tool_invoke_routes_to_engine() {
        let replies = run_frames(&[json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "plugin/tool-invoke",
            "params": { "tool": "engine_info", "arguments": {} }
        })]);
        assert_eq!(replies[0]["id"], json!(7));
        assert_eq!(replies[0]["result"]["engine"]["name"], json!("rust3d-engine"));
    }

    #[test]
    fn unknown_tool_returns_jsonrpc_error() {
        let replies = run_frames(&[json!({
            "jsonrpc": "2.0",
            "id": "x",
            "method": "plugin/tool-invoke",
            "params": { "tool": "nope", "arguments": {} }
        })]);
        assert_eq!(replies[0]["error"]["code"], json!(-32602));
    }

    #[test]
    fn unknown_method_returns_jsonrpc_error() {
        let replies = run_frames(&[json!({
            "jsonrpc": "2.0",
            "id": "y",
            "method": "plugin/nope",
            "params": {}
        })]);
        assert_eq!(replies[0]["error"]["code"], json!(-32602));
    }

    #[test]
    fn shutdown_stops_the_loop() {
        let replies = run_frames(&[
            json!({ "jsonrpc": "2.0", "id": "s", "method": "plugin/shutdown", "params": {} }),
            // 这一帧不该被处理：shutdown 之后循环就退出了
            json!({ "jsonrpc": "2.0", "id": "after", "method": "plugin/handshake", "params": {} }),
        ]);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], json!("s"));
    }

    #[test]
    fn notification_is_ignored() {
        let replies = run_frames(&[json!({
            "jsonrpc": "2.0",
            "method": "plugin/event",
            "params": { "event": "whatever" }
        })]);
        assert!(replies.is_empty());
    }

    /// stdout 被协议独占：混进通知、未知方法、工具调用之后，输出的每一行仍必须是
    /// 合法 JSON-RPC 帧（`run_frames` 会逐行 `from_str`，有任何一行日志都会在这里炸）。
    #[test]
    fn stdout_only_carries_protocol_frames() {
        let replies = run_frames(&[
            json!({ "jsonrpc": "2.0", "id": "n", "method": "plugin/event", "params": { "event": "x" } }),
            // 通知没有 id：只允许走 stderr，不许往 stdout 写任何东西
            json!({ "jsonrpc": "2.0", "method": "plugin/event", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": "bad", "method": "plugin/nope", "params": {} }),
            json!({
                "jsonrpc": "2.0",
                "id": "t",
                "method": "plugin/tool-invoke",
                "params": { "tool": "mesh_stats", "arguments": {} }
            }),
            json!({ "jsonrpc": "2.0", "id": "bye", "method": "plugin/shutdown", "params": {} }),
        ]);
        assert_eq!(replies.len(), 4, "通知不该产生回包：{replies:?}");
        for frame in &replies {
            assert_eq!(frame["jsonrpc"], json!("2.0"), "缺 jsonrpc 版本：{frame}");
            assert!(frame.get("id").is_some(), "回包必须带 id：{frame}");
            assert!(
                frame.get("result").is_some() || frame.get("error").is_some(),
                "回包必须有 result 或 error：{frame}"
            );
        }
    }

    #[test]
    fn emit_manifest_matches_handshake() {
        let manifest = spec::manifest_json();
        let handshake = spec::handshake_json();
        assert_eq!(manifest["tools"], handshake["tools"]);
        assert_eq!(manifest["commands"], handshake["commands"]);
    }
}
