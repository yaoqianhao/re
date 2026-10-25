//! 插件 → 宿主的反向调用（`host/*`）。
//!
//! 协议约束（来自 `crates/plugin/src/host_api.rs` 与 `crates/plugin/src/host.rs`）：
//!
//! * 只有 `host/*` 命名空间可用，其它前缀宿主直接拒；
//! * 调用必须发生在**一次工具调用内部**，宿主按「单次工具调用 200 次」计配额；
//! * 同步阻塞：插件发请求后一边读 stdin 等回包（宿主不会在我们等的时候再发
//!   请求，但**通知**可能有），等到 `id` 匹配的响应才返回。
//!
//! 这里只封装本插件真正用到的两个方法：`host/fs.read` 与 `host/workspace.info`。

use serde_json::{json, Value};

use crate::protocol;

/// `host/fs.read`。
pub const FS_READ_METHOD: &str = "host/fs.read";
/// `host/workspace.info`（归 `fs.read` 能力）。
pub const WORKSPACE_INFO_METHOD: &str = "host/workspace.info";

/// 宿主允许的单次工具调用内宿主调用次数（docs/14 §8.4）。
pub const QUOTA_PER_CALL: usize = 200;

/// 帧收发抽象：让 `HostClient` 不依赖具体管道类型，方便单测注入假宿主。
pub trait Transport {
    fn send(&mut self, frame: &Value) -> Result<(), String>;
    /// `Ok(None)` 表示宿主侧管道已关闭。
    fn recv(&mut self) -> Result<Option<Value>, String>;
}

/// 调用宿主失败的原因。
#[derive(Debug)]
pub enum HostError {
    /// 宿主回了错误帧（`6101` 未授权 / `1002` 越界 / `-32601` 方法不存在等）。
    Remote { code: i64, message: String },
    /// 管道断了或帧坏了：插件已经没法继续和宿主对话。
    Transport(String),
    /// 本次工具调用内的宿主调用次数超配额。
    Quota,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote { code, message } => write!(f, "宿主拒绝（{code}）：{message}"),
            Self::Transport(message) => write!(f, "宿主通道不可用：{message}"),
            Self::Quota => write!(f, "本次工具调用内的宿主调用次数已达上限 {QUOTA_PER_CALL}"),
        }
    }
}

impl HostError {
    /// 供工具层区分「宿主没授权 / 越界」与「管道炸了」：前者可以降级返回提示，
    /// 后者应当让整次工具调用失败（继续算也没有意义）。
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }
}

/// 反向调用客户端：维护自增请求 id，串行发一收一。
///
/// 插件是单线程模型（一个工具调用跑完才读下一帧），所以不需要真并发；
/// 自增 id 只是让「回包对应哪个请求」这件事有据可查。
pub struct HostClient {
    next_id: u64,
    /// 本次工具调用内已发起的宿主调用次数（每次工具调用由 `begin_call` 归零）。
    calls_in_flight: usize,
}

impl Default for HostClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HostClient {
    pub fn new() -> Self {
        Self {
            next_id: 0,
            calls_in_flight: 0,
        }
    }

    /// 每次工具调用入口调一次：宿主配额是「按次」计，不是按进程计。
    pub fn begin_call(&mut self) {
        self.calls_in_flight = 0;
    }

    /// 发一个请求并等对应回包。
    pub fn call<T: Transport>(
        &mut self,
        transport: &mut T,
        method: &str,
        params: Value,
    ) -> Result<Value, HostError> {
        if self.calls_in_flight >= QUOTA_PER_CALL {
            return Err(HostError::Quota);
        }
        self.calls_in_flight += 1;
        self.next_id += 1;
        let id = json!(format!("r3e-{}", self.next_id));
        transport
            .send(&protocol::request_frame(id.clone(), method, params))
            .map_err(HostError::Transport)?;

        let want = id.as_str().unwrap_or_default().to_string();
        loop {
            let frame = match transport.recv().map_err(HostError::Transport)? {
                Some(frame) => frame,
                None => {
                    return Err(HostError::Transport(
                        "宿主关闭了 stdin，无法收齐回包".to_string(),
                    ))
                }
            };
            // 有 method：宿主发来的请求/通知。等待回包期间没法嵌套服务，直接拒绝
            // 有 id 的请求（通知无需回包，丢弃即可）。
            if protocol::method_of(&frame).is_some() {
                if let Some(request_id) = protocol::id_of(&frame) {
                    let method = protocol::method_of(&frame).unwrap_or_default().to_string();
                    let reply = protocol::error_frame(
                        request_id,
                        -32601,
                        format!("插件正在等待宿主回包，暂不处理嵌套请求：{method}"),
                    );
                    transport.send(&reply).map_err(HostError::Transport)?;
                }
                continue;
            }
            // 响应帧：只认我们自己那个 id，别人的说明状态已乱，丢弃继续等。
            let got = frame.get("id").and_then(Value::as_str).unwrap_or_default();
            if got != want {
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Err(HostError::Remote {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("宿主未给出错误信息")
                        .to_string(),
                });
            }
            return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 读一个工作区相对路径的文本文件。
    ///
    /// 宿主 `fs.read` 默认只给 64 KB，`truncated` 为真时**必须**报错：
    /// 截断的 JSON 交给 `serde_json` 只会得到一句莫名其妙的「EOF while parsing」。
    pub fn fs_read<T: Transport>(
        &mut self,
        transport: &mut T,
        path: &str,
        max_bytes: Option<u64>,
    ) -> Result<String, HostError> {
        let mut params = json!({ "path": path });
        if let Some(max_bytes) = max_bytes {
            params["maxBytes"] = json!(max_bytes);
        }
        let reply = self.call(transport, FS_READ_METHOD, params)?;
        if reply.get("truncated").and_then(Value::as_bool) == Some(true) {
            let bytes = reply.get("bytes").and_then(Value::as_u64).unwrap_or(0);
            return Err(HostError::Remote {
                code: 6107,
                message: format!(
                    "场景文件被截断（仅读到 {bytes} 字节，宿主默认上限 64 KB）。请改用 scene 参数直接传内容，或拆分场景"
                ),
            });
        }
        reply
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| HostError::Transport("host/fs.read 回包缺少 content 字段".to_string()))
    }

    /// 取当前工作区信息（未打开工作区时宿主回 `null`）。
    pub fn workspace_info<T: Transport>(&mut self, transport: &mut T) -> Result<Value, HostError> {
        self.call(transport, WORKSPACE_INFO_METHOD, json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 假宿主：按脚本回包，同时记录插件发出去的帧。
    struct FakeHost {
        script: VecDeque<Value>,
        sent: Vec<Value>,
    }

    impl FakeHost {
        fn new(script: Vec<Value>) -> Self {
            Self {
                script: script.into(),
                sent: Vec::new(),
            }
        }
    }

    impl Transport for FakeHost {
        fn send(&mut self, frame: &Value) -> Result<(), String> {
            self.sent.push(frame.clone());
            Ok(())
        }

        fn recv(&mut self) -> Result<Option<Value>, String> {
            Ok(self.script.pop_front())
        }
    }

    #[test]
    fn fs_read_returns_content() {
        let mut host = FakeHost::new(vec![json!({
            "jsonrpc": "2.0",
            "id": "r3e-1",
            "result": { "path": "a.json", "content": "{}", "bytes": 2, "truncated": false }
        })]);
        let mut client = HostClient::new();
        client.begin_call();
        let text = client.fs_read(&mut host, "a.json", None).expect("读到内容");
        assert_eq!(text, "{}");
        assert_eq!(host.sent[0]["method"], json!(FS_READ_METHOD));
        assert_eq!(host.sent[0]["params"]["path"], json!("a.json"));
    }

    #[test]
    fn truncated_read_is_an_error() {
        let mut host = FakeHost::new(vec![json!({
            "jsonrpc": "2.0",
            "id": "r3e-1",
            "result": { "path": "a.json", "content": "{", "bytes": 65536, "truncated": true }
        })]);
        let mut client = HostClient::new();
        client.begin_call();
        let error = client.fs_read(&mut host, "a.json", None).expect_err("必须报错");
        assert!(matches!(error, HostError::Remote { .. }));
    }

    #[test]
    fn remote_error_is_mapped() {
        let mut host = FakeHost::new(vec![json!({
            "jsonrpc": "2.0",
            "id": "r3e-1",
            "error": { "code": 6103, "message": "能力尚未授权：fs.read" }
        })]);
        let mut client = HostClient::new();
        client.begin_call();
        match client.workspace_info(&mut host) {
            Err(HostError::Remote { code, .. }) => assert_eq!(code, 6103),
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn quota_is_enforced_per_tool_call() {
        let mut host = FakeHost::new(Vec::new());
        let mut client = HostClient::new();
        client.begin_call();
        client.calls_in_flight = QUOTA_PER_CALL;
        assert!(matches!(
            client.workspace_info(&mut host),
            Err(HostError::Quota)
        ));
        // 下一次工具调用归零后又能发
        client.begin_call();
        assert_eq!(client.calls_in_flight, 0);
    }
}
