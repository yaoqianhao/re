//! `prefab.spawn` 的传输层契约（M2-E05）。
//!
//! E04 落地时只验到「门禁数字配平」（`cmd_spec_contract` 的宿主方法数 85、
//! `script_only` 放行），**没有一条用例真的发过一次 `prefab.spawn` 请求**。本文件补上：
//! 协议层对这两条 RPC 的职责只有三件事，逐条锁死。
//!
//! 1. **参数形状**：`id` / `name` 非空，`parent` 三态（省略 / `null` = 挂根，空串一律拒）。
//!    空串在节点名域里是合法写法，让它兼职表达「挂根」，两种意图就分不开了——
//!    所以 `parent: ""` 必须在协议层就挡住，不能透传给宿主自己猜。
//! 2. **透传**：形状合法就把三个字段原样交给宿主，协议层不替 core 判模板 ID 形状
//!    （同一套规则两份实现必然静默失配）。
//! 3. **失败码不合并**：模板没登记是 `PREFAB_REFERENCE_MISSING`（下一步是「先
//!    `prefab.define`」），参数写错是 `INVALID_PARAMS`（下一步是「改参数」）。合并成
//!    一个码等于把调用方的下一步藏起来。
//!
//! 方法名大小写不敏感（`Prefab.Spawn` ≡ `prefab.spawn`）由 `dispatch` 统一归一，
//! 这里顺手作为第一条用例的入口写法。

use r3e_protocol::*;
use serde_json::{json, Value};

/// 只关心 `prefab.spawn` 的桩：记录收到的请求，可选地按指定错误码失败。
#[derive(Default)]
struct PrefabStub {
    seen: Option<PrefabSpawnRequest>,
    calls: usize,
    seen_info: Option<PrefabInfoRequest>,
    infos: usize,
    fail_with: Option<RpcFailure>,
}

impl EngineApi for PrefabStub {
    fn info(&mut self) -> EngineInfo {
        unimplemented!("本测试不查 info")
    }
}

impl NavApi for PrefabStub {}

impl PrefabApi for PrefabStub {
    fn prefab_spawn(&mut self, req: PrefabSpawnRequest) -> Result<Value, RpcFailure> {
        self.calls += 1;
        let reply = json!({
            "name": req.name,
            "template": req.id,
            "parent": req.parent,
            "nodeId": 7,
            "overrides": {},
        });
        self.seen = Some(req);
        match self.fail_with.clone() {
            Some(failure) => Err(failure),
            None => Ok(reply),
        }
    }

    fn prefab_info(&mut self, req: PrefabInfoRequest) -> Result<Value, RpcFailure> {
        self.infos += 1;
        let reply = json!({
            "name": req.name,
            "template": "prop.crate",
            "parent": Value::Null,
            "node": req.name.clone(),
            "count": 0,
            "overrides": [],
            "fields": [],
        });
        self.seen_info = Some(req);
        Ok(reply)
    }
}

fn request(method: &str, params: Value) -> Request {
    Request {
        jsonrpc: JSONRPC.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    }
}

/// `prefab.spawn`：参数透传、`parent` 三态、失败码原样透传。
#[test]
fn prefab_spawn_forwards_params_and_keeps_reference_error() {
    // ① 三个字段原样透传，`parent` 有值 → `Some(..)`。
    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "Prefab.Spawn",
            json!({ "id": "prop.crate", "name": "crate_1", "parent": "ground" }),
        ),
    );
    assert!(res.error.is_none(), "不应报错: {:?}", res.error);
    assert_eq!(
        api.seen,
        Some(PrefabSpawnRequest {
            id: "prop.crate".to_string(),
            name: "crate_1".to_string(),
            parent: Some("ground".to_string()),
        }),
        "三个字段应原样交给宿主"
    );
    assert_eq!(
        res.result.unwrap()["template"],
        json!("prop.crate"),
        "宿主回包应原样带回"
    );

    // ② 省略 `parent` 与显式 `null` 都表示「挂场景根」。
    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "prefab.spawn",
            json!({ "id": "prop.crate", "name": "crate_2" }),
        ),
    );
    assert!(res.error.is_none(), "省略 parent 不该报错: {:?}", res.error);
    assert_eq!(
        api.seen.as_ref().and_then(|req| req.parent.clone()),
        None,
        "省略 parent = 挂场景根"
    );

    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "prefab.spawn",
            json!({ "id": "prop.crate", "name": "crate_3", "parent": null }),
        ),
    );
    assert!(res.error.is_none(), "显式 null 不该报错: {:?}", res.error);
    assert_eq!(
        api.seen.as_ref().and_then(|req| req.parent.clone()),
        None,
        "null 与省略同义"
    );

    // ③ 空串 `parent` 在协议层就拒，且不惊动宿主（不占一次调用）。
    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "prefab.spawn",
            json!({ "id": "prop.crate", "name": "crate_4", "parent": "" }),
        ),
    );
    let err = res.error.expect("空串 parent 应报错");
    assert_eq!(err.code, INVALID_PARAMS);
    assert_eq!(api.calls, 0, "参数非法时不该调用宿主");

    // ④ 模板没登记：宿主报 `PREFAB_REFERENCE_MISSING`，传输层不得改码或吞消息。
    let mut api = PrefabStub {
        fail_with: Some(RpcFailure::new(
            PREFAB_REFERENCE_MISSING,
            "模板 `prop.missing` 没登记",
        )),
        ..PrefabStub::default()
    };
    let res = dispatch(
        &mut api,
        &request(
            "prefab.spawn",
            json!({ "id": "prop.missing", "name": "crate_5" }),
        ),
    );
    let err = res.error.expect("宿主失败应转成错误回包");
    assert_eq!(
        err.code, PREFAB_REFERENCE_MISSING,
        "模板缺失必须是专用码，不能塌成 INVALID_PARAMS"
    );
    assert!(
        err.message.contains("prop.missing"),
        "宿主的消息应原样带出，实际：{}",
        err.message
    );
    assert_eq!(api.calls, 1, "参数合法，宿主应被调用恰好一次");
}

// ---------------------------------------------------------------------------
// 判据 ⑤⑥（M2-E04）：方法表登记与 dispatch 路由。
//
// 上面那条用例验的是「形状与透传」，前提是 `dispatch` 已经把请求交给了
// `PrefabApi`；本条把那个前提本身锁死——方法名没进 `RPC_METHODS`，或者进了表
// 却没人路由，上面那条用例照样绿，可跨进程调用方一到手就是 `METHOD_NOT_FOUND`。
// ---------------------------------------------------------------------------

/// 判据 ⑤：`RPC_METHODS` 恰含两条 `prefab.*`，且不含 `prefab.define`。
#[test]
fn rpc_methods_register_exactly_spawn_and_info() {
    assert_eq!(
        RPC_METHODS.len(),
        85,
        "RPC 方法总数应为 85（E03 的 83 + prefab 两条）；改本数等于改门禁口径"
    );

    let mut prefab: Vec<&str> = RPC_METHODS
        .iter()
        .copied()
        .filter(|method| method.starts_with("prefab."))
        .collect();
    prefab.sort_unstable();
    assert_eq!(
        prefab,
        ["prefab.info", "prefab.spawn"],
        "只有 spawn（写）与 info（读）带 rpc；`prefab.define` 不入表"
    );

    // 同一份表里不能有重复项（重复会让 `rpc` 计数虚高、路由却只有一处）。
    let mut sorted: Vec<&str> = RPC_METHODS.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(before, sorted.len(), "RPC_METHODS 不应有重复方法名");

    // 命名铁律：点分小写、无下划线（`canonical_method` 会把下划线删掉）。
    for method in RPC_METHODS {
        assert!(
            method
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'),
            "方法名只允许小写字母、数字和点：{method}"
        );
    }
}

/// 判据 ⑥：dispatch 命中两条 prefab 方法，未登记的方法一律拒。
#[test]
fn dispatch_routes_prefab_methods_and_rejects_unknown() {
    // 两条都真的被路由到了宿主（而不是落到下一组兜底链或 unknown）。
    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request("prefab.info", json!({ "name": "crate_1" })),
    );
    assert!(res.error.is_none(), "prefab.info 应命中: {:?}", res.error);
    assert_eq!(api.infos, 1, "prefab.info 应被路由到宿主一次");
    assert_eq!(
        api.seen_info.as_ref().map(|req| req.name.as_str()),
        Some("crate_1"),
        "`name` 应原样交给宿主"
    );

    // 大小写不敏感：`PREFAB.SPAWN` ≡ `prefab.spawn`（由 `canonical_method` 归一）。
    let mut api = PrefabStub::default();
    let res = dispatch(
        &mut api,
        &request(
            "PREFAB.SPAWN",
            json!({ "id": "prop.crate", "name": "crate_1" }),
        ),
    );
    assert!(res.error.is_none(), "大小写不应影响路由: {:?}", res.error);
    assert_eq!(api.calls, 1, "归一后应命中 prefab.spawn");

    // 未登记的方法必须在协议层就拒，且不惊动任何宿主方法。
    let mut api = PrefabStub::default();
    let res = dispatch(&mut api, &request("prefab.nope", json!({})));
    let err = res.error.expect("未登记的方法应报错");
    assert_eq!(
        err.code, METHOD_NOT_FOUND,
        "未登记方法应是 METHOD_NOT_FOUND"
    );
    assert_eq!(
        (api.calls, api.infos),
        (0, 0),
        "拒答不该顺带调一次宿主：兜底链必须在这里就断掉"
    );
}
