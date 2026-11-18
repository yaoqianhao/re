//! `tx.*`：一笔事务的六步（begin / apply / intent / preview / commit / rollback）。
//!
//! # 这里绝不做的事
//!
//! 不判断「这条命令该不该被允许」。可写域白名单在引擎侧（`tx.apply` 只接受 AI 可写域
//! 内的意图），CLI 手里只有**协议方法表**，拿它去裁决业务意图会两头错：
//! 误杀合法命令（命令表 388 条，协议方法表 58 条），或放过非法命令（本地表恰好有，
//! 但当前文档状态不允许）。
//!
//! 所以本地只拦**形态**错误（空、非点分名）——这类错误连引擎都不必打扰；
//! 名称对不对交给引擎回 `METHOD_NOT_FOUND` / `TX_REJECTED`，那时 `errmap` 会给出
//! 最近邻方法与下一步。

use serde_json::{json, Value};

use r3e_protocol::canonical_method;
use r3e_protocol::client::{is_known_method, nearest_methods};

use crate::args::{Global, TxAction};
use crate::cmd::{call, connect};
use crate::exit;
use crate::output::{kv, one_line, str_field, Out};
use crate::state;

/// 入口。
pub fn run(global: &Global, action: TxAction, explicit_tx: Option<String>, args: Value) -> i32 {
    match action {
        TxAction::Begin => begin(global),
        other => step(global, other, explicit_tx, args),
    }
}

/// `tx begin`：开一笔事务并把 txId 落盘。
fn begin(global: &Global) -> i32 {
    let out = Out::new(global);
    let addr = global.resolved_addr();

    let mut client = match connect(global) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let reply = match call(global, &mut client, "tx.begin", json!({})) {
        Ok(reply) => reply,
        Err(code) => return code,
    };

    let tx_id = str_field(&reply, "txId").unwrap_or("").trim().to_string();
    if tx_id.is_empty() {
        out.failure(
            "引擎回了 tx.begin 却没有 txId",
            &["协议异常：检查 editor-host 的 tx_begin 回包是否带 txId".to_string()],
        );
        return exit::NEGATIVE;
    }

    let mut tail = Vec::new();
    tail.push(format!("引擎       {}（{}）", addr, global.addr_source()));
    match &global.state_file {
        Some(path) => match state::save(path, &tx_id, &addr) {
            Ok(()) => tail.push(format!("会话态     {}", path.display())),
            Err(error) => {
                // 事务已经开了：这里必须提示而不是报错退出，否则调用方会以为没开、
                // 再开一笔撞 TX_BUSY，把真实问题（磁盘不可写）藏起来。
                tail.push(format!(
                    "会话态未落盘（{error}）：后续命令请显式 `--tx {tx_id}`"
                ));
            }
        },
        None => tail.push("会话态已关闭（`--state-file \"\"`）：后续命令请显式 `--tx`".to_string()),
    }

    let human = render(&reply, "事务已开启", "", &tail);
    out.success(&reply, &human);
    exit::OK
}

/// 其余五步：都要先解析出 txId。
fn step(global: &Global, action: TxAction, explicit_tx: Option<String>, args: Value) -> i32 {
    let out = Out::new(global);
    let addr = global.resolved_addr();

    let op = match &action {
        TxAction::Apply { op } => match normalize_op(global, op) {
            Ok(op) => Some(op),
            Err(code) => return code,
        },
        _ => None,
    };

    let tx_id = match resolve_tx_id(global, explicit_tx.as_deref(), &addr, out) {
        Ok(tx_id) => tx_id,
        Err(code) => return code,
    };

    // 本地方法表没收录时只提示、不拦：见文件头注释里的权衡。
    if let Some(op) = &op {
        if !is_known_method(op) {
            let near = nearest_methods(op, 3);
            let hint = if near.is_empty() {
                "方法名是否拼错".to_string()
            } else {
                format!("最近邻：{}", near.join(" / "))
            };
            out.note(&format!(
                "提示：`{op}` 不在本机协议方法表内（{hint}）；仍会发给引擎裁决"
            ));
        }
    }

    let params = build_params(&action, &tx_id, op.as_deref(), &args);
    let method = action.method();

    let mut client = match connect(global) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let reply = match call(global, &mut client, method, params) {
        Ok(reply) => reply,
        Err(code) => return code,
    };

    let mut tail = Vec::new();
    if action.is_terminal() {
        // 终结才清会话态：commit 失败（TX_CONFLICT 等）时事务仍可能未决，
        // 提前清掉会让调用方再也找不到它，只能靠重启引擎。
        if let Some(path) = &global.state_file {
            state::clear(path);
            tail.push(format!("会话态已清空  {}", path.display()));
        }
    }

    let human = render(&reply, title_for(&action), &tx_id, &tail);
    out.success(&reply, &human);
    exit::OK
}

/// 形态校验 + 归一化（去下划线、统一小写）。
fn normalize_op(global: &Global, raw: &str) -> Result<String, i32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Out::new(global).failure(
            "`tx apply` 的 op 不能为空",
            &["正确用法：`r3e-cli tx apply scene.settransform --args '{...}'`".to_string()],
        );
        return Err(exit::USAGE);
    }
    if !trimmed.contains('.') {
        Out::new(global).failure(
            &format!("op 应为点分方法名，收到 `{trimmed}`"),
            &[
                "点分命名铁律（`layer.verb`）是协议层的校验中心，CLI 提前拦下来省一次往返"
                    .to_string(),
            ],
        );
        return Err(exit::USAGE);
    }
    let canonical = canonical_method(trimmed);
    if canonical.is_empty() {
        Out::new(global).failure(
            &format!("op `{trimmed}` 归一化后为空"),
            &["检查是否只有下划线/分隔符".to_string()],
        );
        return Err(exit::USAGE);
    }
    Ok(canonical)
}

/// 解析 txId：`--tx` > state-file。
fn resolve_tx_id(
    global: &Global,
    explicit: Option<&str>,
    addr: &str,
    out: Out,
) -> Result<String, i32> {
    if let Some(id) = explicit.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(id.to_string());
    }

    let Some(path) = &global.state_file else {
        out.failure(
            "没有可用的 txId",
            &[
                "`--state-file \"\"` 关闭了会话态，此时必须用 `--tx <id>` 显式给出".to_string(),
                "或先跑 `r3e-cli tx begin` 拿一笔".to_string(),
            ],
        );
        return Err(exit::USAGE);
    };

    match state::load(path) {
        Some(session) => {
            if state::addr_mismatch(&session, addr) {
                // 不据此拒绝：同一端口换进程是常态，让引擎回 TX_NOT_FOUND 比客户端猜更准。
                out.note(&format!(
                    "提示：会话态记录的引擎是 `{}`，当前连的是 `{addr}`；引擎若已重启，旧 txId 会回 TX_NOT_FOUND",
                    session.addr.unwrap_or_default()
                ));
            }
            Ok(session.tx_id)
        }
        None => {
            out.failure(
                "没有可用的 txId",
                &[
                    format!("会话态 {} 里没有未决事务", path.display()),
                    "先 `r3e-cli tx begin` 开一笔，或用 `--tx <id>` 显式指定".to_string(),
                ],
            );
            Err(exit::USAGE)
        }
    }
}

/// 组参数（字段名严格对齐 `r3e-protocol` 的 `dispatch/tx.rs`）。
fn build_params(action: &TxAction, tx_id: &str, op: Option<&str>, args: &Value) -> Value {
    match action {
        TxAction::Apply { .. } => json!({
            "txId": tx_id,
            "op": op.unwrap_or_default(),
            "args": args,
        }),
        TxAction::Intent { name } => json!({
            "txId": tx_id,
            "intent": name,
            "args": args,
        }),
        _ => json!({ "txId": tx_id }),
    }
}

fn title_for(action: &TxAction) -> &'static str {
    match action {
        TxAction::Begin => "事务已开启",
        TxAction::Apply { .. } => "已暂存一条命令",
        TxAction::Intent { .. } => "已暂存一条意图",
        TxAction::Preview => "预演完成（场景未改动）",
        TxAction::Commit => "已提交，事务终结",
        TxAction::Rollback => "已回滚，事务终结",
    }
}

/// 通用回包渲染：标题 + 「回包字段」+ 补充行。
///
/// **不硬编码字段名**：宿主回包里 `staged` / `committed` / `dropped` / `base` 随动作
/// 变化，M1-S6 还会加字段。写死一套就意味着一有新增字段 CLI 就当没看见，
/// 而「AI 只需要看懂回包」的前提恰恰是回包完整可见。
fn render(reply: &Value, title: &str, tx_id: &str, tail: &[String]) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Value::Object(map) = reply {
        for (key, value) in map {
            pairs.push((key.clone(), short(value)));
        }
    }
    if !tx_id.is_empty() && !pairs.iter().any(|(k, _)| k == "txId") {
        pairs.insert(0, ("txId".to_string(), tx_id.to_string()));
    }

    let mut text = title.to_string();
    if !pairs.is_empty() {
        text.push('\n');
        text.push_str(&kv(&pairs));
    }
    for line in tail {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// 单值一行化：数组给紧凑 JSON（含条数与内容），长内容截断。
fn short(value: &Value) -> String {
    let text = one_line(value);
    truncate(&text, 160)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…")
}
