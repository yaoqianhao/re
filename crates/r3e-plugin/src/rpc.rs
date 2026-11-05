//! 引擎写通道（P0-3）：插件通过 `r3e-editor` 的 JSON-RPC 2.0 over TCP / NDJSON
//! 调用**已有的**写方法（`scene.settransform` / `scene.spawn` / `script.eval` /
//! `scene.save` …）。
//!
//! 刻意不新造协议：那 22 条写方法已经在编辑器侧有参数校验、错误码与测试，
//! 插件再发明一套"插件专用写协议"只会让同一件事有两份真相。插件的价值只在
//! 「把模型容易写错的东西收敛成少量高杠杆工具」，通道复用即可。
//!
//! 连接信息来自工具参数 `endpoint`（或环境变量），与主机授权无关：
//! - 地址：`endpoint.addr`（`host:port`）> 环境变量 `R3E_RPC_ADDR` > `127.0.0.1:8787`
//! - 令牌：`endpoint.token` > 环境变量 `R3E_RPC_TOKEN` > 无（引擎未设 `--rpc-token` 时）
//!
//! 引擎必须带 `--rpc-port` 启动，否则写工具只会拿到一条明确的连接失败。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{json, Value};

/// 引擎侧默认监听端口（与文档示例一致；真实端口以 `--rpc-port` 为准）。
pub const DEFAULT_ADDR: &str = "127.0.0.1:8787";
pub const ENV_ADDR: &str = "R3E_RPC_ADDR";
pub const ENV_TOKEN: &str = "R3E_RPC_TOKEN";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(20);
/// 单行响应上限：正常响应只有几 KB，留 1 MiB 余量后直接判错，避免被坏数据拖死。
const MAX_LINE_BYTES: u64 = 1 << 20;

/// 引擎连接参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub addr: String,
    pub token: Option<String>,
}

impl Endpoint {
    /// 从工具参数里取连接信息，缺省按「参数 > 环境变量 > 内置默认」的顺序回退。
    pub fn from_args(args: &Value) -> Self {
        let endpoint = args.get("endpoint");
        let addr = endpoint
            .and_then(|e| e.get("addr"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                endpoint
                    .and_then(|e| e.get("port"))
                    .and_then(Value::as_u64)
                    .map(|port| format!("127.0.0.1:{port}"))
            })
            .or_else(|| env_non_empty(ENV_ADDR))
            .unwrap_or_else(|| DEFAULT_ADDR.to_string());
        // `host:port` 里没写 host 时补回环地址，免得 `to_socket_addrs` 报奇怪的错。
        let addr = if addr.starts_with(':') {
            format!("127.0.0.1{addr}")
        } else {
            addr
        };
        let token = endpoint
            .and_then(|e| e.get("token"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or_else(|| env_non_empty(ENV_TOKEN));
        Self { addr, token }
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// RPC 调用的失败形态：连不上 / 引擎回了错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    /// 连接、写入或读行失败（引擎没启动、端口不对、被对方关掉…）。
    Transport(String),
    /// 引擎返回了 JSON-RPC `error`：`code` / `message` 原样带回。
    Remote { code: i64, message: String },
}

impl RpcError {
    /// 面向模型的一句话：包含「怎么修」，而不只是「失败了」。
    pub fn advice(&self) -> String {
        match self {
            Self::Transport(detail) => format!(
                "连不上引擎（{detail}）；请用 `--rpc-port <PORT>` 启动 r3e，\
                 并通过工具参数 `endpoint.addr` 或环境变量 {ENV_ADDR} 指定同一地址"
            ),
            Self::Remote { code, message } => format!("引擎拒绝（{code}）：{message}"),
        }
    }
}

/// 一条长连接的 NDJSON 客户端。
///
/// 一次工具调用只建一次连接、发多次请求：引擎每帧处理有限的请求数，
/// 复用连接比「一方法一连接」少一次三次握手，也让同一批改动落在同一帧。
pub struct RpcClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    token: Option<String>,
}

impl RpcClient {
    pub fn connect(endpoint: &Endpoint) -> Result<Self, RpcError> {
        let mut last_err = None;
        for addr in endpoint
            .addr
            .to_socket_addrs()
            .map_err(|e| RpcError::Transport(format!("地址 `{}` 无法解析: {e}", endpoint.addr)))?
        {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    let reader = BufReader::new(
                        stream
                            .try_clone()
                            .map_err(|e| RpcError::Transport(format!("复制连接句柄失败: {e}")))?,
                    );
                    return Ok(Self {
                        reader,
                        writer: stream,
                        next_id: 1,
                        token: endpoint.token.clone(),
                    });
                }
                Err(e) => last_err = Some(format!("连接 {addr} 失败: {e}")),
            }
        }
        Err(RpcError::Transport(last_err.unwrap_or_else(|| {
            format!("地址 `{}` 没有可用结果", endpoint.addr)
        })))
    }

    /// 发一条请求并等到同 `id` 的响应。
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        let params = with_token(params, self.token.as_deref());
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&request)
            .map_err(|e| RpcError::Transport(format!("请求序列化失败: {e}")))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|e| RpcError::Transport(format!("发送请求失败: {e}")))?;

        let mut buf = Vec::new();
        let read = self
            .reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| RpcError::Transport(format!("读取响应失败: {e}")))?;
        if read == 0 {
            return Err(RpcError::Transport(
                "引擎在响应前关闭了连接（是否在启动时给了 `--rpc-port`？）".to_string(),
            ));
        }
        if read as u64 > MAX_LINE_BYTES {
            return Err(RpcError::Transport(format!(
                "响应超过 {MAX_LINE_BYTES} 字节上限（疑似非本引擎的服务）"
            )));
        }
        let text = String::from_utf8(buf)
            .map_err(|e| RpcError::Transport(format!("响应不是合法 UTF-8: {e}")))?;
        let response: Value = serde_json::from_str(text.trim_end()).map_err(|e| {
            RpcError::Transport(format!(
                "响应不是合法 JSON: {e}（原文 {}）",
                text.trim_end()
            ))
        })?;

        if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32603);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("引擎未给出错误说明")
                .to_string();
            return Err(RpcError::Remote { code, message });
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// 引擎从 `params.token` 读握手令牌（`--rpc-token` 时必填），其余字段原样透传。
fn with_token(params: Value, token: Option<&str>) -> Value {
    let mut params = match params {
        Value::Object(map) => Value::Object(map),
        Value::Null => json!({}),
        other => json!({ "value": other }),
    };
    if let Some(token) = token {
        params["token"] = json!(token);
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_defaults_to_loopback_without_env() {
        let endpoint = Endpoint::from_args(&json!({}));
        assert_eq!(endpoint.addr, DEFAULT_ADDR);
    }

    #[test]
    fn endpoint_reads_addr_and_token_from_args() {
        let endpoint = Endpoint::from_args(&json!({
            "endpoint": { "addr": "127.0.0.1:9999", "token": "dev" }
        }));
        assert_eq!(
            endpoint,
            Endpoint {
                addr: "127.0.0.1:9999".to_string(),
                token: Some("dev".to_string()),
            }
        );
    }

    #[test]
    fn endpoint_accepts_bare_port_and_empty_host() {
        assert_eq!(
            Endpoint::from_args(&json!({ "endpoint": { "port": 9123 } })).addr,
            "127.0.0.1:9123"
        );
    }

    #[test]
    fn token_is_injected_into_params_only_when_set() {
        assert_eq!(
            with_token(json!({ "node": "hero" }), Some("t0")),
            json!({ "node": "hero", "token": "t0" })
        );
        assert_eq!(
            with_token(json!({ "node": "hero" }), None),
            json!({ "node": "hero" })
        );
        // 方法没有参数时也得能塞令牌。
        assert_eq!(
            with_token(Value::Null, Some("t0")),
            json!({ "token": "t0" })
        );
    }

    #[test]
    fn transport_error_advice_mentions_rpc_port() {
        let advice = RpcError::Transport("refused".to_string()).advice();
        assert!(advice.contains("--rpc-port"), "{advice}");
    }
}
