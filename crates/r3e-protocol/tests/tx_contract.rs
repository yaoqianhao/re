//! `tx.*` 协议面契约测试（M1-S1）。
//!
//! 只测**协议层该负责的部分**：参数整形、缺省规则、方法名归一、缺省实现的拒答口径。
//! 事务语义（暂存、影子预演、原子提交、回滚到基哈希）在宿主侧，由
//! `r3e-editor-host` 的 `tx_contract` 看守——本文件出现任何「场景有没有变」的断言，
//! 都说明协议层越界了。

use r3e_protocol::*;
use serde_json::{json, Value};

/// 记录调用轨迹的桩：`tx.*` 六条全部接住，把入参原样回显成回包。
#[derive(Default)]
struct TxStub {
    /// 已收到的调用序列（`begin` / `apply` / `intent` / `preview` / `commit` / `rollback`）。
    calls: Vec<&'static str>,
    /// 最近一次 `tx.apply` 的入参（`txId` / `op` / `args`）。
    last_apply: Option<(String, String, Value)>,
    /// 最近一次 `tx.intent` 的入参（`txId` / `intent` / `args`）。
    last_intent: Option<(String, String, Value)>,
}

impl EngineApi for TxStub {
    fn info(&mut self) -> EngineInfo {
        unimplemented!("本测试不查 info")
    }

    fn tx_begin(&mut self) -> Result<Value, RpcFailure> {
        self.calls.push("begin");
        Ok(json!({ "txId": "tx-1", "baseHash": 7u64, "version": 3u64, "staged": 0 }))
    }

    fn tx_apply(&mut self, tx_id: &str, op: &str, args: &Value) -> Result<Value, RpcFailure> {
        self.calls.push("apply");
        self.last_apply = Some((tx_id.to_string(), op.to_string(), args.clone()));
        Ok(json!({ "txId": tx_id, "staged": 1, "op": op }))
    }

    fn tx_intent(&mut self, tx_id: &str, intent: &str, args: &Value) -> Result<Value, RpcFailure> {
        self.calls.push("intent");
        self.last_intent = Some((tx_id.to_string(), intent.to_string(), args.clone()));
        Ok(json!({
            "txId": tx_id,
            "intent": intent,
            "commands": 2,
            "staged": 2,
            "intents": 1,
        }))
    }

    fn tx_preview(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        self.calls.push("preview");
        Ok(json!({ "staged": 1 }))
    }

    fn tx_commit(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        self.calls.push("commit");
        Ok(json!({ "committed": 1 }))
    }

    fn tx_rollback(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        self.calls.push("rollback");
        Ok(json!({ "undone": 0 }))
    }
}

impl NavApi for TxStub {}
impl PrefabApi for TxStub {}

/// 什么都没实现的宿主：验证 trait 缺省实现拒答 `tx.*`。
struct BareStub;

impl EngineApi for BareStub {
    fn info(&mut self) -> EngineInfo {
        unimplemented!("本测试不查 info")
    }
}

impl NavApi for BareStub {}
impl PrefabApi for BareStub {}

fn request(method: &str, params: Value) -> Request {
    Request {
        jsonrpc: JSONRPC.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    }
}

// ---------------------------------------------------------------- 方法表

#[test]
fn tx_methods_are_declared() {
    for m in [
        "tx.begin",
        "tx.apply",
        "tx.intent",
        "tx.preview",
        "tx.commit",
        "tx.rollback",
    ] {
        assert!(RPC_METHODS.contains(&m), "`{m}` 应登记在 RPC_METHODS");
    }
}

#[test]
fn rpc_methods_stay_sorted_and_unique() {
    // 字典序是能力清单 `git diff` 可读的前提；重复登记会让「44 条」这条硬断言说谎。
    let mut sorted: Vec<&str> = RPC_METHODS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), RPC_METHODS.len(), "RPC_METHODS 有重复条目");
    assert_eq!(sorted, RPC_METHODS.to_vec(), "RPC_METHODS 应保持字典序");
}

// ---------------------------------------------------------------- 缺省实现

#[test]
fn tx_methods_unsupported_by_default() {
    let mut api = BareStub;
    // tx.begin 无参数；其余四个缺 txId 也应在参数层就被拦下，因此这里只断言
    // 「不会 panic 且给出错误」，具体码由参数校验决定。
    let res = dispatch(&mut api, &request("tx.begin", json!({})));
    let err = res.error.expect("缺省实现应报未实现");
    assert_eq!(err.code, METHOD_NOT_FOUND);
    assert!(
        err.message.contains("tx.begin"),
        "错误消息应点名方法: {}",
        err.message
    );

    let res = dispatch(&mut api, &request("tx.commit", json!({ "txId": "tx-1" })));
    let err = res.error.expect("缺省实现应报未实现");
    assert_eq!(err.code, METHOD_NOT_FOUND);
    assert!(err.message.contains("tx.commit"));
}

// ---------------------------------------------------------------- 方法名归一

#[test]
fn tx_methods_accept_case_and_underscore_variants() {
    let mut api = TxStub::default();
    // 归一化规则是「小写 + 删下划线」，所以 `tx._begin` 也落到 `tx.begin`。
    // 注意 `tx_begin`（无点）归一成 `txbegin`，**不是**别名——无点别名只对
    // `enginestats` / `captureframe` 两条历史方法保留，新方法一律要求点分形态。
    for method in ["TX.Begin", "Tx.BEGIN", "tx._begin"] {
        let res = dispatch(&mut api, &request(method, json!({})));
        assert!(res.error.is_none(), "{method} 应可用: {:?}", res.error);
    }
    assert_eq!(api.calls, vec!["begin", "begin", "begin"]);
}

// ---------------------------------------------------------------- 参数整形

#[test]
fn tx_begin_takes_no_params() {
    let mut api = TxStub::default();
    let res = dispatch(&mut api, &request("tx.begin", json!({})));
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    let result = res.result.expect("应有回包");
    assert_eq!(result["baseHash"], json!(7));
    assert_eq!(result["staged"], json!(0), "新事务暂存必为 0");
    assert_eq!(api.calls, vec!["begin"]);
}

#[test]
fn tx_apply_requires_tx_id() {
    let mut api = TxStub::default();
    let res = dispatch(
        &mut api,
        &request("tx.apply", json!({ "op": "scene.settransform" })),
    );
    let err = res.error.expect("缺 txId 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.calls.is_empty(), "参数非法时不应调用宿主");
}

#[test]
fn tx_apply_rejects_blank_tx_id() {
    let mut api = TxStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "tx.apply",
            json!({ "txId": "   ", "op": "scene.settransform" }),
        ),
    );
    let err = res.error.expect("空白 txId 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.calls.is_empty());
}

#[test]
fn tx_apply_requires_op() {
    let mut api = TxStub::default();
    let res = dispatch(&mut api, &request("tx.apply", json!({ "txId": "tx-1" })));
    let err = res.error.expect("缺 op 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(err.message.contains("op"), "错误消息应点名 op");
    assert!(api.calls.is_empty());
}

#[test]
fn tx_apply_rejects_non_dotted_op() {
    let mut api = TxStub::default();
    // 脚本函数名形态（`set_transform`）不许混进来：它没有点分名，无法与 RPC 方法表对齐，
    // 白名单也就无从索引。
    let res = dispatch(
        &mut api,
        &request("tx.apply", json!({ "txId": "tx-1", "op": "set_transform" })),
    );
    let err = res.error.expect("非点分 op 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.calls.is_empty());
}

#[test]
fn tx_apply_defaults_args_to_null() {
    let mut api = TxStub::default();
    let res = dispatch(
        &mut api,
        &request("tx.apply", json!({ "txId": "tx-1", "op": "scene.list" })),
    );
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    let (tx, op, args) = api.last_apply.clone().expect("应调用宿主");
    assert_eq!(tx, "tx-1");
    assert_eq!(op, "scene.list");
    assert_eq!(args, Value::Null, "省略 args 应交给宿主 null");
}

#[test]
fn tx_apply_passes_object_args_through() {
    let mut api = TxStub::default();
    let args = json!({ "name": "box", "visible": false });
    let res = dispatch(
        &mut api,
        &request(
            "tx.apply",
            json!({ "txId": "tx-2", "op": "scene.setvisible", "args": args }),
        ),
    );
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    let (tx, op, passed) = api.last_apply.clone().expect("应调用宿主");
    assert_eq!((tx.as_str(), op.as_str()), ("tx-2", "scene.setvisible"));
    assert_eq!(passed, args, "对象参数应原样透传，协议层不改写");
}

#[test]
fn tx_apply_rejects_array_args() {
    let mut api = TxStub::default();
    // 定位参数数组会让「哪个值对应哪个字段」变成位置约定，加一个字段就静默错位。
    let res = dispatch(
        &mut api,
        &request(
            "tx.apply",
            json!({ "txId": "tx-1", "op": "scene.settransform", "args": [1, 2, 3] }),
        ),
    );
    let err = res.error.expect("数组 args 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.calls.is_empty(), "参数非法时不应调用宿主");
}

#[test]
fn tx_apply_intent_requires_intent() {
    let mut api = TxStub::default();
    let res = dispatch(&mut api, &request("tx.intent", json!({ "txId": "tx-1" })));
    let err = res.error.expect("缺 intent 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(
        err.message.contains("intent"),
        "错误消息应点名 intent: {}",
        err.message
    );
    assert!(api.calls.is_empty(), "参数非法时不应调用宿主");
}

#[test]
fn tx_apply_intent_rejects_blank_and_non_dotted_intent() {
    // 空白串与非点分名（脚本函数形态 / 光秃秃的单词）都不收：
    // 意图名要和 RPC 方法一样可索引，否则宿主的白名单无从对齐。
    for intent in ["   ", "spawn_prefab", "spawnprefab"] {
        let mut api = TxStub::default();
        let res = dispatch(
            &mut api,
            &request("tx.intent", json!({ "txId": "tx-1", "intent": intent })),
        );
        let err = res.error.expect("非法 intent 应报错");
        assert_eq!(err.code, INVALID_PARAMS, "intent = {intent:?}");
        assert!(api.calls.is_empty(), "intent = {intent:?} 时不应调用宿主");
    }
}

#[test]
fn tx_apply_intent_passes_intent_and_args_through() {
    let mut api = TxStub::default();
    let args = json!({ "prefab": "desk", "position": [1.0, 0.0, 2.0] });
    let res = dispatch(
        &mut api,
        &request(
            "tx.intent",
            json!({ "txId": "tx-3", "intent": "scene.spawn_prefab", "args": args }),
        ),
    );
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    let result = res.result.expect("应有回包");
    assert_eq!(
        result["intent"],
        json!("scene.spawn_prefab"),
        "回包要回显意图名"
    );
    assert_eq!(result["commands"], json!(2), "回包要报展开出的命令条数");
    assert_eq!(result["intents"], json!(1), "回包要报步数（意图数）");

    let (tx, intent, passed) = api.last_intent.clone().expect("应调用宿主");
    assert_eq!(
        (tx.as_str(), intent.as_str()),
        ("tx-3", "scene.spawn_prefab")
    );
    assert_eq!(passed, args, "对象参数应原样透传，协议层不改写");
}

#[test]
fn tx_apply_intent_defaults_args_to_null() {
    let mut api = TxStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "tx.intent",
            json!({ "txId": "tx-1", "intent": "scene.spawn_prefab" }),
        ),
    );
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    let (_, _, args) = api.last_intent.clone().expect("应调用宿主");
    assert_eq!(args, Value::Null, "省略 args 应交给宿主 null");
}

#[test]
fn tx_apply_intent_requires_tx_id() {
    let mut api = TxStub::default();
    let res = dispatch(
        &mut api,
        &request("tx.intent", json!({ "intent": "scene.spawn_prefab" })),
    );
    let err = res.error.expect("缺 txId 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(api.calls.is_empty());
}

#[test]
fn tx_preview_commit_rollback_require_tx_id() {
    for method in ["tx.preview", "tx.commit", "tx.rollback"] {
        let mut api = TxStub::default();
        let res = dispatch(&mut api, &request(method, json!({})));
        let err = res.error.expect("缺 txId 应报错");
        assert_eq!(err.code, INVALID_PARAMS, "{method}");
        assert!(api.calls.is_empty(), "{method} 参数非法时不应调用宿主");
    }
}

#[test]
fn tx_preview_commit_rollback_dispatch_to_host() {
    let mut api = TxStub::default();
    for method in ["tx.preview", "tx.commit", "tx.rollback"] {
        let res = dispatch(&mut api, &request(method, json!({ "txId": "tx-9" })));
        assert!(res.error.is_none(), "{method} 不应报错: {:?}", res.error);
    }
    assert_eq!(api.calls, vec!["preview", "commit", "rollback"]);
}

// ---------------------------------------------------------------- 不越界

#[test]
fn unknown_method_still_not_found() {
    // tx 组挂在 scene/media 之前，兜底链必须仍然通到最后一组，否则所有 `ui.*` 会哑掉。
    let mut api = TxStub::default();
    let res = dispatch(&mut api, &request("tx.nope", json!({})));
    assert_eq!(
        res.error.expect("未知方法应 404").code,
        METHOD_NOT_FOUND,
        "tx 前缀的未知方法也应走 METHOD_NOT_FOUND"
    );
    assert!(api.calls.is_empty());
}
