//! 引擎 RPC 客户端（M1-S5）：JSON-RPC 2.0 over TCP / NDJSON 的**唯一**客户端实现。
//!
//! # 为什么住在协议层
//!
//! 传输格式由本 crate 定义（[`crate::msg`]、[`RPC_METHODS`]、错误码常量），
//! 「怎么连、怎么读写一行」是同一份协议的另一半。此前这段代码只存在于
//! `r3e-plugin/src/rpc.rs`（250 行），M1-S5 的 `r3e-cli` 需要**完全相同**的
//! 能力（长连接、令牌注入、单行上限、错误码透传）——再抄一份就是同一逻辑两份
//! 实现（`AI_ROADMAP` §九 R5 高危）。因此客户端上收到协议层，成为两个消费者
//! （CLI、插件）的共同底座。
//!
//! **零引擎依赖不变**：本模块只用 `std::net` + `std::io` + `serde_json`，
//! `cargo tree -p r3e-protocol` 仍只有 `serde` / `serde_json`。
//!
//! # 与传输层（`r3e-editor::server`）的约定
//!
//! - 一问一答：请求行 + `\n`，响应行 + `\n`；本模块每次 [`RpcClient::call`]
//!   写完就等到同 `id` 的响应，**不匹配即报错**（错序会把 A 的结果当 B 的）。
//! - 令牌走 `params.token`（引擎 `--rpc-token` 时必填），其余字段原样透传。
//! - 单行上限 1 MiB；超过即判错，避免被非本引擎的服务拖死。
//! - 连接信息按「显式参数 > 环境变量 > 内置默认」回退，环境变量名与插件一致。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{canonical_method, RPC_METHODS};

/// 引擎侧默认监听地址（文档示例同值；真实端口以引擎 `--rpc-port` 为准）。
pub const DEFAULT_ADDR: &str = "127.0.0.1:8787";
/// 覆盖默认地址的环境变量（与 `r3e-plugin` 同名同义）。
pub const ENV_ADDR: &str = "R3E_RPC_ADDR";
/// 覆盖令牌的环境变量（与 `r3e-plugin` 同名同义）。
pub const ENV_TOKEN: &str = "R3E_RPC_TOKEN";

/// 建立 TCP 连接的超时。
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 单次读写超时（引擎一帧处理有限请求数，给足余量）。
pub const IO_TIMEOUT: Duration = Duration::from_secs(20);
/// 单行（响应）字节上限。
pub const MAX_LINE_BYTES: u64 = 1 << 20;

/// 不声明在 [`RPC_METHODS`] 里但引擎确实接受的方法（传输层心跳与两条历史别名）。
///
/// 客户端把它们算作「已知方法」，否则 `r3e-cli` 的本地前置校验会把 `ping`
/// 当成拼错的方法名拒掉。
pub const EXTRA_METHODS: &[&str] = &["ping", "enginestats", "captureframe"];

/// 一次连接的全部参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `host:port`。
    pub addr: String,
    /// 握手令牌；引擎未设 `--rpc-token` 时为 `None`。
    pub token: Option<String>,
    /// 读写超时。
    pub io_timeout: Duration,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            addr: DEFAULT_ADDR.to_string(),
            token: None,
            io_timeout: IO_TIMEOUT,
        }
    }
}

impl Endpoint {
    /// 只给地址，其余走默认。
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: normalize_addr(&addr.into()),
            ..Self::default()
        }
    }

    /// 设置令牌（空串视为未设）。
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = non_empty(token.into());
        self
    }

    /// 设置读写超时（毫秒）。
    pub fn with_io_timeout_ms(mut self, ms: u64) -> Self {
        if ms > 0 {
            self.io_timeout = Duration::from_millis(ms);
        }
        self
    }

    /// 按「显式 > 环境变量 > 默认」解析。
    ///
    /// 显式值给空串等同于没给——命令行上 `--token ""` 常被脚本当成「不传」，
    /// 在这里静默当成空令牌会让引擎回 `UNAUTHORIZED`，不如按下一条来源回退。
    pub fn resolve(addr: Option<&str>, token: Option<&str>) -> Self {
        let addr = addr
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(normalize_addr)
            .or_else(|| env_non_empty(ENV_ADDR).map(|a| normalize_addr(&a)))
            .unwrap_or_else(|| DEFAULT_ADDR.to_string());
        let token = token
            .map(str::to_string)
            .and_then(non_empty)
            .or_else(|| env_non_empty(ENV_TOKEN));
        Self {
            addr,
            token,
            io_timeout: IO_TIMEOUT,
        }
    }

    /// 只读环境变量（一条命令若不做任何显式指定，就用这个）。
    pub fn from_env() -> Self {
        Self::resolve(None, None)
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

/// `:8787` / `8787` 这类写法补回环地址，免得 `to_socket_addrs` 报奇怪的错。
fn normalize_addr(addr: &str) -> String {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix(':') {
        return format!("127.0.0.1:{rest}");
    }
    if addr.chars().all(|c| c.is_ascii_digit()) && !addr.is_empty() {
        return format!("127.0.0.1:{addr}");
    }
    addr.to_string()
}

/// 客户端失败形态。
///
/// 与 [`crate::msg::RpcFailure`] 的区别：那个是**服务端**要回的错，这个是**调用方**
/// 拿到的错——它多一类「根本没连上」的 [`ClientError::Transport`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// 连接、写入或读行失败（引擎没启动、端口不对、被对方关掉、超时…）。
    Transport(String),
    /// 引擎返回了 JSON-RPC `error`：`code` / `message` 原样带回。
    Remote { code: i64, message: String },
}

impl ClientError {
    /// 错误码（仅远端错误有）。
    pub fn code(&self) -> Option<i64> {
        match self {
            Self::Remote { code, .. } => Some(*code),
            Self::Transport(_) => None,
        }
    }

    /// 是否属于「环境没准备好」这一类（CLI 用它决定退出码 3）。
    pub fn is_environment(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Remote { code, .. } => {
                *code == i64::from(crate::UNAUTHORIZED)
                    || *code == i64::from(crate::SERVER_BUSY)
                    || *code == i64::from(crate::RPC_TIMEOUT)
            }
        }
    }

    /// 面向人 / AI 的一句话：带「怎么修」，而不只是「失败了」。
    pub fn advice(&self) -> String {
        match self {
            Self::Transport(detail) => format!(
                "连不上引擎（{detail}）；请用 `--rpc-port <PORT>` 启动引擎，\
                 并用 `--addr` 或环境变量 {ENV_ADDR} 指定同一地址"
            ),
            Self::Remote { code, message } => {
                format!("引擎拒绝（{code}）：{message}")
            }
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(detail) => write!(f, "传输错误：{detail}"),
            Self::Remote { code, message } => write!(f, "引擎错误 {code}：{message}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// 一条长连接的 NDJSON 客户端。
///
/// 一次命令只建一次连接、发多次请求：引擎每帧处理有限的请求数，复用连接比
/// 「一方法一连接」少一次三次握手，也让同一批改动落在同一帧。
pub struct RpcClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    endpoint: Endpoint,
}

impl RpcClient {
    /// 建立连接（DNS 多地址逐个试，全失败才报错）。
    pub fn connect(endpoint: &Endpoint) -> Result<Self, ClientError> {
        let mut last_err = None;
        for addr in endpoint.addr.to_socket_addrs().map_err(|e| {
            ClientError::Transport(format!("地址 `{}` 无法解析: {e}", endpoint.addr))
        })? {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(endpoint.io_timeout));
                    let _ = stream.set_write_timeout(Some(endpoint.io_timeout));
                    let reader =
                        BufReader::new(stream.try_clone().map_err(|e| {
                            ClientError::Transport(format!("复制连接句柄失败: {e}"))
                        })?);
                    return Ok(Self {
                        reader,
                        writer: stream,
                        next_id: 1,
                        endpoint: endpoint.clone(),
                    });
                }
                Err(e) => last_err = Some(format!("连接 {addr} 失败: {e}")),
            }
        }
        Err(ClientError::Transport(last_err.unwrap_or_else(|| {
            format!("地址 `{}` 没有可用结果", endpoint.addr)
        })))
    }

    /// 本次连接使用的端点。
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// 发一条请求并等到同 `id` 的响应。
    ///
    /// 方法名会先过 [`canonical_method`]：`scene.setTransform` 与
    /// `scene.set_transform` 都能落到引擎认得的那条臂上，客户端不必让调用方背归一化规则。
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let params = with_token(params, self.endpoint.token.as_deref());
        let request = json!({
            "jsonrpc": crate::JSONRPC,
            "id": id,
            "method": canonical_method(method),
            "params": params,
        });
        let mut line = serde_json::to_string(&request)
            .map_err(|e| ClientError::Transport(format!("请求序列化失败: {e}")))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|e| ClientError::Transport(format!("发送请求失败: {e}")))?;

        let text = self.read_line()?;
        let response: Value = serde_json::from_str(&text).map_err(|e| {
            ClientError::Transport(format!("响应不是合法 JSON: {e}（原文 {text}）"))
        })?;

        let got = response.get("id").and_then(Value::as_u64);
        if got != Some(id) {
            return Err(ClientError::Transport(format!(
                "响应 id 与请求不符（请求 {id}，收到 {}）——连接上有并发或对端不是本引擎",
                response
                    .get("id")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "null".to_string())
            )));
        }

        if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32603);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("引擎未给出错误说明")
                .to_string();
            return Err(ClientError::Remote { code, message });
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 读一行响应（去尾换行），带空行跳过与上限检查。
    fn read_line(&mut self) -> Result<String, ClientError> {
        loop {
            let mut buf = Vec::new();
            let read = self
                .reader
                .read_until(b'\n', &mut buf)
                .map_err(|e| ClientError::Transport(read_failure(&e)))?;
            if read == 0 {
                return Err(ClientError::Transport(
                    "引擎在响应前关闭了连接（是否在启动时给了 `--rpc-port`？）".to_string(),
                ));
            }
            if read as u64 > MAX_LINE_BYTES {
                return Err(ClientError::Transport(format!(
                    "响应超过 {MAX_LINE_BYTES} 字节上限（疑似非本引擎的服务）"
                )));
            }
            let text = String::from_utf8(buf)
                .map_err(|e| ClientError::Transport(format!("响应不是合法 UTF-8: {e}")))?;
            let text = text.trim_end().to_string();
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }
}

/// 读失败时的文案。
///
/// 对端中止 / 复位这类「连接没了」的情况，`std::io::Error` 的原文是
/// 「你的主机中的软件中止了一个已建立的连接」（Windows · 10053）之类——它指向
/// 本机网络栈，真实原因九成是**引擎进程已经退出**。让调用方照着 OS 文案去查防火墙，
/// 是这条链路里最容易浪费半小时的地方，所以这里统一换成能指向 `--rpc-port` 的说法。
fn read_failure(err: &std::io::Error) -> String {
    use std::io::ErrorKind;

    let peer_gone = matches!(
        err.kind(),
        ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
    ) || matches!(err.raw_os_error(), Some(10053) | Some(10054) | Some(10058));

    if peer_gone {
        format!("引擎在响应前关闭了连接（是否在启动时给了 `--rpc-port`？）：{err}")
    } else {
        format!("读取响应失败: {err}")
    }
}

/// 引擎从 `params.token` 读握手令牌，其余字段原样透传。
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

/// 方法名是否是引擎认得的（[`RPC_METHODS`] 或 [`EXTRA_METHODS`]）。
///
/// 用于客户端**发请求前**的本地校验：拼错的方法名烧一次往返只会得到
/// `METHOD_NOT_FOUND`，连「你是不是想写 X」都没有。
pub fn is_known_method(method: &str) -> bool {
    let name = canonical_method(method);
    RPC_METHODS.contains(&name.as_str()) || EXTRA_METHODS.contains(&name.as_str())
}

/// 与 `method` 最接近的已知方法名（编辑距离升序，最多 `limit` 条）。
///
/// 只用于把 `-32601` 变成可操作建议，不参与任何裁决。
pub fn nearest_methods(method: &str, limit: usize) -> Vec<&'static str> {
    let target = canonical_method(method);
    let mut scored: Vec<(usize, &'static str)> = RPC_METHODS
        .iter()
        .map(|m| (edit_distance(&target, m), *m))
        .chain(
            EXTRA_METHODS
                .iter()
                .map(|m| (edit_distance(&target, m), *m)),
        )
        .filter(|(d, _)| *d <= 6)
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.into_iter().take(limit).map(|(_, m)| m).collect()
}

/// 两个字符串的 Levenshtein 距离（长度都在 64 以内，O(n·m) 足够）。
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_falls_back_to_default_loopback() {
        // 环境变量可能被外部设置，这里只断言「没有显式值时不会是空」。
        let endpoint = Endpoint::resolve(Some(DEFAULT_ADDR), None);
        assert_eq!(endpoint.addr, DEFAULT_ADDR);
        assert_eq!(endpoint.io_timeout, IO_TIMEOUT);
    }

    #[test]
    fn endpoint_accepts_bare_port_and_bare_colon_port() {
        assert_eq!(Endpoint::new("9123").addr, "127.0.0.1:9123");
        assert_eq!(Endpoint::new(":9123").addr, "127.0.0.1:9123");
        assert_eq!(Endpoint::new("127.0.0.1:9123").addr, "127.0.0.1:9123");
        assert_eq!(Endpoint::new(" example.com:1 ").addr, "example.com:1");
    }

    #[test]
    fn empty_token_or_addr_falls_through() {
        let endpoint = Endpoint::resolve(Some("127.0.0.1:9000"), Some("   "));
        assert_eq!(endpoint.token, None);
        // 空地址回退到默认（环境变量未设时为内置默认）。
        let blank = Endpoint::resolve(Some("  "), None);
        assert!(!blank.addr.is_empty());
    }

    #[test]
    fn token_is_injected_only_when_set() {
        assert_eq!(
            with_token(json!({ "node": "hero" }), Some("t0")),
            json!({ "node": "hero", "token": "t0" })
        );
        assert_eq!(
            with_token(json!({ "node": "hero" }), None),
            json!({ "node": "hero" })
        );
        // 无参方法也得能塞令牌。
        assert_eq!(
            with_token(Value::Null, Some("t0")),
            json!({ "token": "t0" })
        );
    }

    #[test]
    fn known_method_accepts_canonical_and_alias_forms() {
        assert!(is_known_method("tx.begin"));
        assert!(is_known_method("scene.setTransform"));
        assert!(is_known_method("scene.set_transform"));
        assert!(is_known_method("ping"));
        assert!(is_known_method("engine_stats"));
        assert!(!is_known_method("tx.apply_intent"));
        assert!(!is_known_method("scene.setttransform"));
    }

    #[test]
    fn nearest_methods_suggests_the_typo_target() {
        let top = nearest_methods("scene.setttransform", 3);
        assert_eq!(top.first().copied(), Some("scene.settransform"), "{top:?}");
        // 归一化会把下划线删掉，`tx.apply_intent` 应指向 `tx.apply` 一族。
        let for_alias = nearest_methods("tx.apply_intent", 3);
        assert!(!for_alias.is_empty(), "应给出候选");
    }

    #[test]
    fn edit_distance_is_symmetric_and_bounded() {
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn transport_error_is_environment_and_has_advice() {
        let err = ClientError::Transport("refused".to_string());
        assert!(err.is_environment());
        assert_eq!(err.code(), None);
        assert!(err.advice().contains("--rpc-port"), "{}", err.advice());
    }

    #[test]
    fn remote_error_classifies_environment_codes() {
        let unauthorized = ClientError::Remote {
            code: i64::from(crate::UNAUTHORIZED),
            message: "bad token".to_string(),
        };
        assert!(unauthorized.is_environment());
        let rejected = ClientError::Remote {
            code: i64::from(crate::TX_REJECTED),
            message: "nope".to_string(),
        };
        assert!(!rejected.is_environment());
        assert_eq!(rejected.code(), Some(i64::from(crate::TX_REJECTED)));
    }
}
