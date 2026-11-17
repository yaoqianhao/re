//! `tx.*` 事务臂（M1-S1）。
//!
//! 协议层只做**参数整形**：`txId` / `op` / `args` 三个字段的校验与缺省规则在这里裁决。
//! 事务身份（`tx_id → 暂存命令队列`）与「意图 → 命令」的翻译属于宿主
//! （`r3e-editor-host`），本层不认识 `Document`，也不认识 `CommandKind`。
//!
//! # 为什么 `tx.apply` 传「方法名 + 参数」而不是序列化的 `CommandKind`
//!
//! `CommandKind` 每一条都携带**前像**（`old: Transform` 之类），而前像只能在执行当下从
//! 场景里抓（见 `r3e-doc/src/command/mod.rs` 的「前像在构造时从场景抓取」）。若让远端
//! 序列化一整条命令过来，等于把「伪造前像」的权力交给 AI——撤销会把一个编造的旧值写回
//! 场景，比不可撤销更坏。因此协议只传**意图**：`op` 是白名单内的 RPC 方法名，`args` 是
//! 它的参数；前像由宿主在 `&mut Document` 作用域内就地抓取。
//!
//! # 五个方法
//!
//! | 方法 | 语义 | 副作用 |
//! |---|---|---|
//! | `tx.begin` | 开一笔事务，冻结基哈希 | 只读一次哈希，不推进版本 |
//! | `tx.apply` | 把一条意图**暂存**进事务（不落场景） | 无（宿主可在暂存期做 dry-run） |
//! | `tx.preview` | 影子预演，返回预演后哈希与受影响对象 | 无（结构上不可能写） |
//! | `tx.commit` | n 条命令一次性入撤销栈，版本只 +1 | 写场景 |
//! | `tx.rollback` | 丢弃暂存，场景回到 `begin` 时的基哈希 | 无 |
//!
//! 命中不到本组时交给 [`super::scene::dispatch_scene`]，不在本文件回错误。

use super::*;

pub(super) fn dispatch_tx(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "tx.begin" => match api.tx_begin() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "tx.apply" => {
            let tx = match require_tx_id(params) {
                Ok(t) => t,
                Err(f) => return Response::fail(id, f),
            };
            let op = match require_op(params) {
                Ok(o) => o,
                Err(f) => return Response::fail(id, f),
            };
            let args = match opt_args(params) {
                Ok(a) => a,
                Err(f) => return Response::fail(id, f),
            };
            match api.tx_apply(tx, op, &args) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "tx.intent" => {
            let tx = match require_tx_id(params) {
                Ok(t) => t,
                Err(f) => return Response::fail(id, f),
            };
            let intent = match require_intent(params) {
                Ok(i) => i,
                Err(f) => return Response::fail(id, f),
            };
            let args = match opt_args(params) {
                Ok(a) => a,
                Err(f) => return Response::fail(id, f),
            };
            match api.tx_intent(tx, intent, &args) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "tx.preview" => match require_tx_id(params).and_then(|tx| api.tx_preview(tx)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "tx.commit" => match require_tx_id(params).and_then(|tx| api.tx_commit(tx)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "tx.rollback" => match require_tx_id(params).and_then(|tx| api.tx_rollback(tx)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        _ => scene::dispatch_scene(api, id, params, method, raw),
    }
}

/// 取 `txId`：必填、非空。空串一律拒绝——`tx.apply` 的空串会落到「哪个事务都不是」，
/// 交给宿主去猜比当场报错难排查得多。
fn require_tx_id(params: &Value) -> Result<&str, RpcFailure> {
    match require_str(params, "txId") {
        Ok(tx) if !tx.trim().is_empty() => Ok(tx),
        Ok(_) => Err(RpcFailure::invalid_params("参数 `txId` 不能为空")),
        Err(f) => Err(f),
    }
}

/// 取 `op`：必填、非空；且必须是**点分方法名**形态（`scene.settransform` 这种）。
///
/// 这里只做形态校验，**白名单裁决归宿主**（`tx.apply` 只接受 AI 可写域内的意图）。
/// 协议层不持有白名单，是因为白名单随 M1-S3 的意图命令扩张，把它写进协议会让
/// 「协议版本」跟着业务能力一起跳。
fn require_op(params: &Value) -> Result<&str, RpcFailure> {
    require_dotted(params, "op")
}

/// 取 `intent`：与 `op` 同一套形态校验（必填、非空、点分名）。
///
/// 分成两个字段名而不是复用 `op`，是为了让 `tx.intent` 的**意图**与
/// `tx.apply` 的**裸命令**在日志与回包里一眼可辨：前者是「一次命中多条命令」的
/// 高层动作（M1-S3），后者是 M1-S1 的原子写。
fn require_intent(params: &Value) -> Result<&str, RpcFailure> {
    require_dotted(params, "intent")
}

/// 点分方法名的形态校验（`op` / `intent` 共用）。
fn require_dotted<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcFailure> {
    let value = match require_str(params, key) {
        Ok(value) => value,
        Err(_) => {
            return Err(RpcFailure::invalid_params(format!(
                "缺少字符串参数 `{key}`"
            )))
        }
    };
    if value.trim().is_empty() {
        return Err(RpcFailure::invalid_params(format!("参数 `{key}` 不能为空")));
    }
    if !value.contains('.') {
        return Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为点分方法名（如 `scene.settransform`），收到 `{value}`"
        )));
    }
    Ok(value)
}

/// 取 `args`：缺省 / `null` 得到 `null`；给了必须是对象。
///
/// 数组在这里是**非法**的：JSON-RPC 的定位参数数组会让「这个对象参数到底对应哪个字段」
/// 变成位置约定，而方法参数一加一减就静默错位。统一要求具名对象。
fn opt_args(params: &Value) -> Result<Value, RpcFailure> {
    match param(params, "args") {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(v @ Value::Object(_)) => Ok(v.clone()),
        Some(other) => Err(RpcFailure::invalid_params(format!(
            "参数 `args` 应为对象（可省略），收到 {other}"
        ))),
    }
}
