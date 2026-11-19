//! 参数解析：`argv` → [`Invocation`]。
//!
//! # 为什么手写而不引 clap
//!
//! `r3e-cli` 的依赖面被 `xtask/dep_rules.json` 钉在 `allow: ["r3e-protocol"]`
//! （外加 workspace 已登记的 `serde_json`）。为四个子命令拉一整套参数框架，
//! 换来的是「多一个要跟版本的第三方树」；而 `r3e-replay/src/main.rs` 本来就是
//! 手写解析，两个命令行工具保持同一种风格，读代码的人少切换一次脑子。
//!
//! # 解析规则（三条硬约定）
//!
//! 1. **取值型参数必须带值**：`--addr` 后面紧跟的那一项就是值，哪怕它长得像子命令。
//!    宁可报「缺值」也不做「看起来像就跳过」的猜测——猜错会让命令静默跑到别的地址上。
//! 2. **布尔参数不吞下一项**：`--json verify` 里 `verify` 是子命令，不是 `--json` 的值。
//! 3. **未知参数一律报错**：拼错的 `--targes` 若被静默忽略，用户会以为自己在筛目标点，
//!    实际跑的是「全场景体检」——这类事故在只读命令上不致命，在写命令上是灾难。

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use r3e_protocol::client::{DEFAULT_ADDR, IO_TIMEOUT};

/// `--state-file` 的缺省路径（相对当前工作目录）。
pub const DEFAULT_STATE_FILE: &str = ".r3e-cli/session.json";

/// `replay` 的缺省参数，与 `r3e-replay` 的缺省值逐一对齐（写两套缺省就会两套漂移）。
pub const DEFAULT_REPLAY_SEED: u64 = 42;
pub const DEFAULT_REPLAY_FRAMES: u64 = 600;
pub const DEFAULT_REPLAY_HASH_EVERY: u64 = 60;

/// 需要取值的参数（白名单：未知即报错）。
const VALUE_FLAGS: &[&str] = &[
    "addr",
    "token",
    "state-file",
    "timeout",
    "tx",
    "args",
    "targets",
    "limit",
    "seed",
    "frames",
    "hash-every",
    "expect",
    "replay-bin",
];

/// 布尔参数。
const BOOL_FLAGS: &[&str] = &["json", "help"];

/// 任何子命令都接受的口径（连接参数与输出口径）。
const GLOBAL_FLAGS: &[&str] = &["addr", "token", "json", "state-file", "timeout", "help"];

/// 解析失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 面向用户的一句话（已含「怎么改」）。
    pub message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// 一次调用要连到哪里、怎么输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    /// 显式地址；`None` 表示走环境变量与缺省。
    pub addr: Option<String>,
    /// 显式令牌；`None` 表示走环境变量。
    pub token: Option<String>,
    /// 机器可读通道。
    pub json: bool,
    /// `None` = 关闭 state-file（`--state-file ""`）。
    pub state_file: Option<PathBuf>,
    /// 单次读写超时（毫秒）。
    pub timeout_ms: u64,
}

impl Default for Global {
    fn default() -> Self {
        Self {
            addr: None,
            token: None,
            json: false,
            state_file: Some(PathBuf::from(DEFAULT_STATE_FILE)),
            timeout_ms: IO_TIMEOUT.as_millis() as u64,
        }
    }
}

impl Global {
    /// 解析后的地址（显式 > `$R3E_RPC_ADDR` > 内置缺省）。
    pub fn resolved_addr(&self) -> String {
        r3e_protocol::client::Endpoint::resolve(self.addr.as_deref(), None).addr
    }

    /// 实际生效的地址来源，用于人类输出里的「连的是谁」。
    pub fn addr_source(&self) -> &'static str {
        if self.addr.as_deref().is_some_and(|a| !a.trim().is_empty()) {
            "--addr"
        } else if std::env::var_os(r3e_protocol::client::ENV_ADDR).is_some() {
            r3e_protocol::client::ENV_ADDR
        } else {
            "内置缺省"
        }
    }
}

/// `tx` 的六个动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxAction {
    Begin,
    Apply { op: String },
    Intent { name: String },
    Preview,
    Commit,
    Rollback,
}

impl TxAction {
    /// 对应 RPC 方法名。
    pub fn method(&self) -> &'static str {
        match self {
            Self::Begin => "tx.begin",
            Self::Apply { .. } => "tx.apply",
            Self::Intent { .. } => "tx.intent",
            Self::Preview => "tx.preview",
            Self::Commit => "tx.commit",
            Self::Rollback => "tx.rollback",
        }
    }

    /// 是否终结一笔事务（成功后 state-file 必须清空）。
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Commit | Self::Rollback)
    }
}

/// `query` 的三条只读方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryMethod {
    Find,
    Describe,
    Stats,
}

impl QueryMethod {
    /// 对应 RPC 方法名。
    pub fn method(self) -> &'static str {
        match self {
            Self::Find => "query.find",
            Self::Describe => "query.describe",
            Self::Stats => "query.stats",
        }
    }
}

/// `replay` 的参数（转发给 `r3e-replay` 子进程）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayArgs {
    pub seed: u64,
    pub frames: u64,
    pub hash_every: u64,
    /// 金标哈希；给了就做比对，不一致回退出码 1。
    pub expect: Option<String>,
    /// 显式二进制路径（优先于 `$R3E_REPLAY_BIN` 与自动探测）。
    pub bin: Option<PathBuf>,
}

impl Default for ReplayArgs {
    fn default() -> Self {
        Self {
            seed: DEFAULT_REPLAY_SEED,
            frames: DEFAULT_REPLAY_FRAMES,
            hash_every: DEFAULT_REPLAY_HASH_EVERY,
            expect: None,
            bin: None,
        }
    }
}

/// 子命令。
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Tx {
        action: TxAction,
        /// `--tx` 显式指定的事务 id（优先于 state-file）。
        tx: Option<String>,
        /// `tx.apply` / `tx.intent` 的操作参数。
        args: Value,
    },
    Query {
        method: QueryMethod,
        /// 已按方法整形好的入参（`query.describe` 的 `node` 在这里已合并）。
        params: Value,
    },
    Verify {
        /// 已解析的目标点数组。
        targets: Value,
        limit: Option<u64>,
    },
    /// 直调任意协议方法（工具通道，不进事务、不进撤销栈）。
    Rpc {
        method: String,
        args: Value,
    },
    Replay(ReplayArgs),
    Help,
}

/// 只该走事务的方法（`tx` 侧的写场景白名单）。
///
/// `rpc` 一律拒绝它们：绕过事务直接改场景，改动既不进撤销栈也不进版本记账，
/// 编辑器里看到的是一个「撤销不回去」的世界。要改场景就走 `tx intent`。
///
/// 表里混着两类名字，来源各不相同，都在引擎侧有据可查：
///
/// - **`tx apply` 的 op**：`r3e-editor-host/src/api/tx.rs` 的 `Intent::parse`
///   （`scene.spawn` / `scene.settransform` / `scene.setvisible` /
///   `light.remove` / `ui.remove` / `scene.material.upsert`）；
/// - **`tx intent` 的高层意图**：同文件的 `IntentPlan::parse` 与 `INTENTS`
///   （`scene.spawn_prefab` / `scene.place_light` / `scene.paint_terrain`）。
///   这三个名字**不在** `RPC_METHODS` 里——它们只被 `tx intent` 接受，
///   所以「不在方法表」不是写错名字，别拿方法表统一校验（`tx_only_methods_exist`
///   单测因此把两类来源分开核对）。
///
/// 另有一条只有 `tx apply` 认、但本身也是协议方法的：`scene.remove`
/// （协议里可 rpc 直调，事务侧没有对应 op，`rpc` 放行等于给一条
/// 「删了就撤不回来」的旁路，比改属性更狠）。
const TX_ONLY_METHODS: &[&str] = &[
    "scene.spawn",
    "scene.settransform",
    "scene.setvisible",
    "scene.material.upsert",
    "scene.remove",
    "light.remove",
    "ui.remove",
];

/// 只被 `tx intent` 接受的高层意图名（不在 [`RPC_METHODS`] 里）。
///
/// `rpc scene.spawn_prefab` 若不拦，会先撞「未知方法 -32601」——那个提示会把人
/// 引去查拼写，而正确答案是「这批名字只能走 `tx intent`」。所以单独一张表配
/// 单独的提示语。
const TX_ONLY_INTENTS: &[&str] = &[
    "scene.spawn_prefab",
    "scene.place_light",
    "scene.paint_terrain",
];

/// 解析结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub global: Global,
    pub cmd: Cmd,
}

/// 解析 `argv`（不含程序名）。
pub fn parse(argv: Vec<String>) -> Result<Invocation, ParseError> {
    let (positionals, flags) = split_flags(&argv)?;
    let global = global_from(&flags)?;

    if flags.contains_key("help") {
        return Ok(Invocation {
            global,
            cmd: Cmd::Help,
        });
    }

    let head = positionals.first().map(String::as_str).ok_or_else(|| {
        ParseError::new(
            "缺少子命令：`r3e-cli --help` 看全部用法（tx / query / verify / rpc / replay）",
        )
    })?;

    let cmd = match head {
        "tx" => parse_tx(&positionals, &flags)?,
        "query" => parse_query(&positionals, &flags)?,
        "verify" => parse_verify(&positionals, &flags)?,
        "rpc" => parse_rpc(&positionals, &flags)?,
        "replay" => parse_replay(&positionals, &flags)?,
        "help" => Cmd::Help,
        other => {
            return Err(ParseError::new(format!(
            "未知子命令 `{other}`；可用：tx / query / verify / rpc / replay（`r3e-cli --help` 看用法）"
        )))
        }
    };

    Ok(Invocation { global, cmd })
}

/// 拆解结果：位置参数 + 命名参数（`None` 表示开关）。
type Parsed = (Vec<String>, BTreeMap<String, Option<String>>);

/// 把 `argv` 拆成「位置参数」与「命名参数」。
fn split_flags(argv: &[String]) -> Result<Parsed, ParseError> {
    let mut positionals = Vec::new();
    let mut flags: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut i = 0;

    while i < argv.len() {
        let raw = &argv[i];
        i += 1;

        if raw == "-h" {
            insert_flag(&mut flags, "help", None)?;
            continue;
        }

        let body = match raw.strip_prefix("--") {
            Some(body) if !body.is_empty() => body,
            _ => {
                positionals.push(raw.clone());
                continue;
            }
        };

        let (key, inline) = match body.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (body.to_string(), None),
        };

        if BOOL_FLAGS.contains(&key.as_str()) {
            if inline.is_some() {
                return Err(ParseError::new(format!(
                    "参数 `--{key}` 是开关，不接受取值（收到 `--{key}={}`）",
                    inline.unwrap_or_default()
                )));
            }
            insert_flag(&mut flags, &key, None)?;
            continue;
        }

        if !VALUE_FLAGS.contains(&key.as_str()) {
            return Err(ParseError::new(format!(
                "未知参数 `--{key}`；`r3e-cli --help` 看全部参数"
            )));
        }

        let value = match inline {
            Some(v) => v,
            None => {
                let next = argv.get(i).ok_or_else(|| {
                    ParseError::new(format!("参数 `--{key}` 缺少取值（写法：`--{key} <值>`）"))
                })?;
                i += 1;
                next.clone()
            }
        };
        insert_flag(&mut flags, &key, Some(value))?;
    }

    Ok((positionals, flags))
}

fn insert_flag(
    flags: &mut BTreeMap<String, Option<String>>,
    key: &str,
    value: Option<String>,
) -> Result<(), ParseError> {
    if flags.contains_key(key) {
        return Err(ParseError::new(format!(
            "参数 `--{key}` 重复给出；后者会悄悄覆盖前者，所以这里直接拒绝"
        )));
    }
    flags.insert(key.to_string(), value);
    Ok(())
}

fn value_of<'a>(flags: &'a BTreeMap<String, Option<String>>, key: &str) -> Option<&'a str> {
    flags.get(key).and_then(|v| v.as_deref())
}

fn global_from(flags: &BTreeMap<String, Option<String>>) -> Result<Global, ParseError> {
    let mut global = Global::default();

    if let Some(addr) = value_of(flags, "addr") {
        global.addr = Some(addr.to_string());
    }
    if let Some(token) = value_of(flags, "token") {
        global.token = Some(token.to_string());
    }
    global.json = flags.contains_key("json");

    if let Some(path) = value_of(flags, "state-file") {
        // 空串 = 显式关闭（与 `--token ""` 的「当没给」不同：这里空路径确实无法落地）。
        global.state_file = if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        };
    }

    if let Some(ms) = value_of(flags, "timeout") {
        let ms = parse_u64(ms, "timeout")?;
        if ms == 0 {
            return Err(ParseError::new("`--timeout` 必须大于 0 毫秒"));
        }
        global.timeout_ms = ms;
    }

    Ok(global)
}

/// 解析 `--args`：缺省 `{}`，必须是 JSON **对象**。
///
/// 数组在这里被拒：`tx.apply` 的 args 是具名对象（协议层同样拒绝数组），
/// 到了 CLI 再"帮忙转一次"只会把位置约定的错误藏起来。
fn json_args(flags: &BTreeMap<String, Option<String>>) -> Result<Value, ParseError> {
    let Some(text) = value_of(flags, "args") else {
        return Ok(json!({}));
    };
    let value: Value = serde_json::from_str(text)
        .map_err(|e| ParseError::new(format!("`--args` 不是合法 JSON：{e}（收到 `{text}`）")))?;
    match value {
        Value::Object(map) => Ok(Value::Object(map)),
        other => Err(ParseError::new(format!(
            "`--args` 必须是 JSON 对象（如 `'{{\"node\":\"hero\"}}'`），收到 {}",
            json_kind(&other)
        ))),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔值",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

fn parse_u64(raw: &str, key: &str) -> Result<u64, ParseError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| ParseError::new(format!("`--{key}` 需要非负整数，收到 `{raw}`")))
}

fn ensure_len(positionals: &[String], want: usize, usage: &str) -> Result<(), ParseError> {
    if positionals.len() > want {
        return Err(ParseError::new(format!(
            "`{usage}` 之后有多余参数 `{}`；正确用法：`r3e-cli {usage}`",
            positionals[want]
        )));
    }
    Ok(())
}

fn reject_unrelated(
    flags: &BTreeMap<String, Option<String>>,
    allowed: &[&str],
) -> Result<(), ParseError> {
    for key in flags.keys() {
        if GLOBAL_FLAGS.contains(&key.as_str()) || allowed.contains(&key.as_str()) {
            continue;
        }
        return Err(ParseError::new(format!(
            "参数 `--{key}` 不属于当前子命令；`r3e-cli --help` 看归属"
        )));
    }
    Ok(())
}

fn parse_tx(
    positionals: &[String],
    flags: &BTreeMap<String, Option<String>>,
) -> Result<Cmd, ParseError> {
    reject_unrelated(flags, &["tx", "args"])?;

    let action = positionals.get(1).map(String::as_str).ok_or_else(|| {
        ParseError::new("`tx` 缺少动作：begin / apply / intent / preview / commit / rollback")
    })?;
    let tx = value_of(flags, "tx")
        .map(str::to_string)
        .filter(|id| !id.trim().is_empty());

    let (action, args) = match action {
        "begin" => {
            ensure_len(positionals, 2, "tx begin")?;
            (TxAction::Begin, json!({}))
        }
        "apply" => {
            let op = positionals
                .get(2)
                .ok_or_else(|| {
                    ParseError::new(
                        "`tx apply` 缺少 <op>；op 是协议方法名，如 `scene.settransform`",
                    )
                })?
                .clone();
            ensure_len(positionals, 3, "tx apply <op> [--args '<json>']")?;
            (TxAction::Apply { op }, json_args(flags)?)
        }
        "intent" => {
            let name = positionals
                .get(2)
                .ok_or_else(|| ParseError::new("`tx intent` 缺少 <name>；如 `scene.spawn_prefab`"))?
                .clone();
            ensure_len(positionals, 3, "tx intent <name> [--args '<json>']")?;
            (TxAction::Intent { name }, json_args(flags)?)
        }
        "preview" => {
            ensure_len(positionals, 2, "tx preview")?;
            (TxAction::Preview, json!({}))
        }
        "commit" => {
            ensure_len(positionals, 2, "tx commit")?;
            (TxAction::Commit, json!({}))
        }
        "rollback" => {
            ensure_len(positionals, 2, "tx rollback")?;
            (TxAction::Rollback, json!({}))
        }
        other => {
            return Err(ParseError::new(format!(
                "未知 tx 动作 `{other}`；可用：begin / apply / intent / preview / commit / rollback"
            )))
        }
    };

    Ok(Cmd::Tx { action, tx, args })
}

fn parse_query(
    positionals: &[String],
    flags: &BTreeMap<String, Option<String>>,
) -> Result<Cmd, ParseError> {
    reject_unrelated(flags, &["args"])?;

    let kind = positionals
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| ParseError::new("`query` 缺少方法：find / describe / stats"))?;
    let args = json_args(flags)?;

    let (method, params) = match kind {
        "find" => {
            ensure_len(positionals, 2, "query find [--args '<json>']")?;
            (QueryMethod::Find, args)
        }
        "describe" => {
            let node = positionals.get(2).ok_or_else(|| {
                ParseError::new("`query describe` 缺少 <node>；例如 `query describe hero`")
            })?;
            ensure_len(positionals, 3, "query describe <node> [--args '<json>']")?;
            let mut map: Map<String, Value> = match args {
                Value::Object(map) => map,
                _ => Map::new(),
            };
            // 位置参数优先级更高：`--args` 里写 node 是笔误，不该压过命令行上明写的那个。
            map.insert("node".to_string(), json!(node));
            (QueryMethod::Describe, Value::Object(map))
        }
        "stats" => {
            ensure_len(positionals, 2, "query stats")?;
            (QueryMethod::Stats, args)
        }
        other => {
            return Err(ParseError::new(format!(
                "未知 query 方法 `{other}`；可用：find / describe / stats"
            )))
        }
    };

    Ok(Cmd::Query { method, params })
}

fn parse_verify(
    positionals: &[String],
    flags: &BTreeMap<String, Option<String>>,
) -> Result<Cmd, ParseError> {
    reject_unrelated(flags, &["targets", "limit"])?;
    ensure_len(positionals, 1, "verify [--targets '<json>'] [--limit N]")?;

    let targets = match value_of(flags, "targets") {
        None => json!([]),
        Some(text) => {
            let value: Value = serde_json::from_str(text).map_err(|e| {
                ParseError::new(format!("`--targets` 不是合法 JSON：{e}（收到 `{text}`）"))
            })?;
            if !value.is_array() {
                return Err(ParseError::new(format!(
                    "`--targets` 必须是数组，如 `'[{{\"name\":\"chest\",\"at\":[0,0,0]}}]'`，收到 {}",
                    json_kind(&value)
                )));
            }
            value
        }
    };

    let limit = match value_of(flags, "limit") {
        None => None,
        Some(raw) => Some(parse_u64(raw, "limit")?),
    };

    Ok(Cmd::Verify { targets, limit })
}

/// 解析 `rpc <method> [--args '<json>']`。
///
/// 只做两件校验：方法名得像协议方法（含 `.`），且不能是「只该走事务」的那批。
/// 参数形状一律交给引擎判——CLI 抄一份校验就多一处会跟引擎漂移的假真相。
fn parse_rpc(
    positionals: &[String],
    flags: &BTreeMap<String, Option<String>>,
) -> Result<Cmd, ParseError> {
    reject_unrelated(flags, &["args"])?;

    let method = positionals
        .get(1)
        .ok_or_else(|| {
            ParseError::new(
                "`rpc` 缺少 <method>；方法名取自协议方法表，如 `physics.bind` / `nav.bake`",
            )
        })?
        .clone();
    ensure_len(positionals, 2, "rpc <method> [--args '<json>']")?;

    if !method.contains('.') {
        return Err(ParseError::new(format!(
            "`rpc` 的 <method> 需要带命名空间，如 `nav.bake`，收到 `{method}`"
        )));
    }
    if method.starts_with("tx.") {
        return Err(ParseError::new(format!(
            "`{method}` 属于事务通道，请用 `tx begin` / `tx apply` / `tx commit`（`rpc` 不记事务状态）"
        )));
    }
    if TX_ONLY_INTENTS.contains(&method.as_str()) {
        return Err(ParseError::new(format!(
            "`{method}` 是高层意图（一批命令 = 一格撤销），只被 `tx intent` 接受：`tx intent {method} --args ...`；`rpc` 直调连方法表都过不去（引擎回 -32601）"
        )));
    }
    if TX_ONLY_METHODS.contains(&method.as_str()) {
        return Err(ParseError::new(format!(
            "`{method}` 会改场景，只允许走事务：`tx apply {method} --args ...`（或 `tx intent` 包一层）才能进撤销栈；用 `rpc` 直调会留下撤销不回去的改动"
        )));
    }

    Ok(Cmd::Rpc {
        method,
        args: json_args(flags)?,
    })
}

fn parse_replay(
    positionals: &[String],
    flags: &BTreeMap<String, Option<String>>,
) -> Result<Cmd, ParseError> {
    reject_unrelated(
        flags,
        &["seed", "frames", "hash-every", "expect", "replay-bin"],
    )?;
    ensure_len(
        positionals,
        1,
        "replay [--seed N] [--frames N] [--hash-every N] [--expect <hex>]",
    )?;

    let mut args = ReplayArgs::default();
    if let Some(raw) = value_of(flags, "seed") {
        args.seed = parse_u64(raw, "seed")?;
    }
    if let Some(raw) = value_of(flags, "frames") {
        args.frames = parse_u64(raw, "frames")?;
        if args.frames == 0 {
            return Err(ParseError::new(
                "`--frames` 必须大于 0（0 帧没有可比对的末哈希）",
            ));
        }
    }
    if let Some(raw) = value_of(flags, "hash-every") {
        args.hash_every = parse_u64(raw, "hash-every")?;
        if args.hash_every == 0 {
            return Err(ParseError::new(
                "`--hash-every` 必须大于 0（否则哈希采样无定义）",
            ));
        }
    }
    if let Some(raw) = value_of(flags, "expect") {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ParseError::new(
                "`--expect` 不能为空；要么给 16 位十六进制，要么别给",
            ));
        }
        args.expect = Some(raw.to_string());
    }
    if let Some(raw) = value_of(flags, "replay-bin") {
        if raw.trim().is_empty() {
            return Err(ParseError::new("`--replay-bin` 不能为空路径"));
        }
        args.bin = Some(PathBuf::from(raw));
    }

    Ok(Cmd::Replay(args))
}

/// 帮助文本（`--help` 与 `help` 子命令共用）。
pub fn help_text() -> String {
    format!(
        "\
r3e-cli — r3e 引擎的远程调用入口（JSON-RPC 2.0 over TCP / NDJSON）

用法:
  r3e-cli <子命令> [参数] [--addr host:port] [--token <s>] [--json]

事务（写场景，一次意图 = 一格撤销）:
  tx begin                            开一笔事务，打印并记住 txId
  tx apply <op> [--args '<json>']     把一条意图暂存进事务（op 是协议方法名）
  tx intent <name> [--args '<json>']  暂存一条高层意图（白名单在引擎侧）
  tx preview                          影子预演：回预演哈希与暂存条数
  tx commit                           提交：n 条命令一次入撤销栈，版本 +1
  tx rollback                         丢弃暂存，场景回基哈希

只读查询（不建事务、不推版本）:
  query find [--args '<json>']        按类别 / 标签 / 区域 / 组件位过滤
  query describe <node>               单节点投影
  query stats                         全场景普查

体检:
  verify [--targets '<json>'] [--limit N]
                                      三层体检；退出码由 passed 决定（不是 ok）

工具通道（不进事务、不进撤销栈）:
  rpc <method> [--args '<json>']      直调任意协议方法，如 `rpc nav.bake` /
                                      `rpc physics.bind`；改场景的那批方法一律拒绝

确定性回放（子进程，不连引擎）:
  replay [--seed N] [--frames N] [--hash-every N] [--expect <hex>] [--replay-bin <path>]

全局参数:
  --addr <host:port>   缺省 $R3E_RPC_ADDR → {DEFAULT_ADDR}
  --token <s>          缺省 $R3E_RPC_TOKEN；走 params.token
  --json               机器可读：stdout 只输出一行 JSON，人类提示走 stderr
  --state-file <path>  记住当前 txId，缺省 {DEFAULT_STATE_FILE}；传 \"\" 关闭
  --timeout <ms>       单次读写超时，缺省 {}
  --tx <id>            显式指定 txId（优先于 state-file）

退出码:
  0  成功 / verify 通过
  1  业务否定（verify 未通过、tx 被引擎拒绝）
  2  用法或参数错误（未知子命令、`--args` 非法 JSON、txId 缺失）
  3  环境错误（连不上引擎 / 鉴权失败 / 超时 / 引擎过载）
  4  依赖缺失（找不到 r3e-replay 二进制）
",
        IO_TIMEOUT.as_millis()
    )
}
#[cfg(test)]
mod tests;
