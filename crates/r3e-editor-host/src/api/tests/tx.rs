//! `tx.intent`（M1-S3-A）宿主层：白名单裁决、参数整形错误码、展开记账，
//! 以及「提交后一次 undo 全退」。
//!
//! 事务语义本身（影子隔离、冲突、回滚复位）由 `r3e-doc` 的 `tests/tx.rs` 看守；
//! 这里守的是**意图 → 命令**这一层：什么名字进得来、`nodes` 怎么整形、
//! `staged` / `intents` 两个计数分别算什么。

use super::*;
use r3e_editor::{INVALID_PARAMS, TX_NOT_FOUND, TX_REJECTED};

/// 开一笔事务并取回 txId。
fn begin(api: &mut LocalEngineApi<'_>) -> String {
    let value = api.tx_begin().expect("开事务不该失败");
    value["txId"].as_str().expect("回包应带 txId").to_string()
}

/// `scene.spawn_prefab` 的 `args`：`{ "nodes": [...] }`。
fn nodes(items: Value) -> Value {
    json!({ "nodes": items })
}

/// `scene.place_light`（M1-S3-B）的 `args`：`{ "lights": [...] }`。
fn lights(items: Value) -> Value {
    json!({ "lights": items })
}

#[test]
fn tx_intent_按命令数记staged_按意图数记步数() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let value = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([
                { "name": "desk" },
                { "name": "lamp", "parent": "desk" },
            ])),
        )
        .expect("白名单意图应被接收");

    assert_eq!(value["txId"], json!(tx));
    assert_eq!(value["intent"], json!("scene.spawn_prefab"));
    assert_eq!(value["commands"], json!(2), "一条意图展开成 2 条命令");
    assert_eq!(value["staged"], json!(2), "staged 记的是命令条数");
    assert_eq!(value["intents"], json!(1), "intents 记的是步数（意图数）");

    // 只攒暂存：主场景一个节点都不该多出来。
    assert!(
        scene.borrow().find_id("desk").is_none(),
        "暂存期不许碰主场景"
    );
    assert!(scene.borrow().find_id("lamp").is_none());
}

#[test]
fn tx_intent_提交后落地_一次undo全退() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    api.tx_intent(
        &tx,
        "scene.spawn_prefab",
        &nodes(json!([
            { "name": "desk" },
            { "name": "lamp", "parent": "desk" },
            { "name": "chair" },
        ])),
    )
    .expect("意图应被接收");

    let committed = api.tx_commit(&tx).expect("提交不该失败");
    assert_eq!(committed["committed"], json!(3), "提交的是命令条数");
    assert!(scene.borrow().find_id("desk").is_some(), "提交后应落地");
    assert!(scene.borrow().find_id("chair").is_some());

    // 整笔事务一格：一条意图展开 3 条命令，撤销栈只该涨 1。
    assert_eq!(scene.borrow().undo_depth(), 1, "整笔事务只占一格撤销");

    scene.borrow_mut().undo().expect("撤销不该失败");
    assert!(scene.borrow().find_id("desk").is_none(), "一次 undo 全退");
    assert!(scene.borrow().find_id("lamp").is_none());
    assert!(scene.borrow().find_id("chair").is_none());
    assert_eq!(scene.borrow().undo_depth(), 0);
}

#[test]
fn tx_intent_未纳入白名单的意图报拒绝且事务保留() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let err = api
        .tx_intent(&tx, "scene.teleport", &json!({}))
        .expect_err("未白名单的意图必须拒绝");
    assert_eq!(err.code, TX_REJECTED, "{}", err.message);
    assert!(
        err.message.contains("scene.teleport") && err.message.contains("scene.spawn_prefab"),
        "错误信息要点名收到的意图与当前白名单：{}",
        err.message
    );

    // 事务没被作废：还能继续攒、也能查预览。
    let preview = api.tx_preview(&tx).expect("事务应保留");
    assert_eq!(preview["staged"], json!(0), "被拒的意图不该留下暂存");
    api.tx_rollback(&tx).expect("回滚不该失败");
}

#[test]
fn tx_intent_缺nodes或空数组都报参数错误() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let missing = api
        .tx_intent(&tx, "scene.spawn_prefab", &json!({}))
        .expect_err("缺 nodes 应报错");
    assert_eq!(missing.code, INVALID_PARAMS, "{}", missing.message);
    assert!(
        missing.message.contains("nodes"),
        "错误信息应点名 nodes：{}",
        missing.message
    );

    let empty = api
        .tx_intent(&tx, "scene.spawn_prefab", &nodes(json!([])))
        .expect_err("空数组应报错");
    assert_eq!(empty.code, INVALID_PARAMS, "{}", empty.message);
    assert!(
        empty.message.contains("不能为空"),
        "空意图的拒绝理由要写清：{}",
        empty.message
    );

    let wrong_type = api
        .tx_intent(&tx, "scene.spawn_prefab", &nodes(json!({ "name": "desk" })))
        .expect_err("nodes 不是数组应报错");
    assert_eq!(wrong_type.code, INVALID_PARAMS, "{}", wrong_type.message);
}

#[test]
fn tx_intent_超过节点上限报参数错误() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let items: Vec<Value> = (0..=256)
        .map(|i| json!({ "name": format!("n{i}") }))
        .collect();
    let err = api
        .tx_intent(&tx, "scene.spawn_prefab", &nodes(Value::Array(items)))
        .expect_err("超过上限应报错");
    assert_eq!(err.code, INVALID_PARAMS, "{}", err.message);
    assert!(
        err.message.contains("最多"),
        "错误信息应给出上限：{}",
        err.message
    );
}

#[test]
fn tx_intent_意图内重名或与场景已有物件重名都拒绝() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let dup = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "desk" }, { "name": "desk" }])),
        )
        .expect_err("同批重名应拒绝");
    assert_eq!(dup.code, TX_REJECTED, "{}", dup.message);
    assert!(
        dup.message.contains("重复"),
        "拒绝理由应说明重名：{}",
        dup.message
    );

    // fixture 里已有 `cube`：意图只新建不覆盖。
    let exists = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "cube" }])),
        )
        .expect_err("与场景已有物件重名应拒绝");
    assert_eq!(exists.code, TX_REJECTED, "{}", exists.message);
    assert!(
        exists.message.contains("已存在"),
        "拒绝理由应说明已存在：{}",
        exists.message
    );

    // 两次都被预检拦下：影子没被碰过，暂存仍为 0。
    let preview = api.tx_preview(&tx).expect("事务应保留");
    assert_eq!(preview["staged"], json!(0));
}

#[test]
fn tx_intent_父物件须在场景里或排在本意图前面() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let dangling = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "lamp", "parent": "ghost" }])),
        )
        .expect_err("父物件不存在应拒绝");
    assert_eq!(dangling.code, TX_REJECTED, "{}", dangling.message);
    assert!(
        dangling.message.contains("ghost"),
        "拒绝理由要点名缺失的父物件：{}",
        dangling.message
    );

    // 父物件排在意图内前序 → 允许。
    let ok = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([
                { "name": "desk" },
                { "name": "lamp", "parent": "desk" },
            ])),
        )
        .expect("父物件在本意图内应通过");
    assert_eq!(ok["commands"], json!(2));

    // 父物件已经在场景里（`cube`）→ 也允许。
    let existing_parent = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "hat", "parent": "cube" }])),
        )
        .expect("父物件在场景内应通过");
    assert_eq!(existing_parent["commands"], json!(1));
    assert_eq!(existing_parent["staged"], json!(3), "前一笔 2 条 + 这 1 条");
    assert_eq!(existing_parent["intents"], json!(2));

    api.tx_rollback(&tx).expect("回滚不该失败");
}

#[test]
fn tx_intent_事务id对不上报txnotfound() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    // 一个事务都没开：任何 txId 都是「查无此事务」。
    let err = api
        .tx_intent(
            "tx-nope",
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "desk" }])),
        )
        .expect_err("没有事务应报错");
    assert_eq!(err.code, TX_NOT_FOUND, "{}", err.message);

    // 开了事务但 id 不匹配，同样拒绝。
    begin(&mut api);
    let mismatch = api
        .tx_intent(
            "tx-nope",
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "desk" }])),
        )
        .expect_err("txId 不匹配应报错");
    assert_eq!(mismatch.code, TX_NOT_FOUND, "{}", mismatch.message);
    assert!(scene.borrow().find_id("desk").is_none(), "拒绝后场景不许变");
}

#[test]
fn tx_intent_place_light_按灯数记账并一次undo全退() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let value = api
        .tx_intent(
            &tx,
            "scene.place_light",
            &lights(json!([
                { "key": "fill", "kind": "point", "pos": [0.0, 3.0, 0.0], "intensity": 2.0 },
                { "key": "spot_a", "kind": "spot", "pos": [1.0, 4.0, 1.0], "outer_deg": 40.0 },
            ])),
        )
        .expect("白名单意图应被接收");

    assert_eq!(value["intent"], json!("scene.place_light"));
    assert_eq!(value["commands"], json!(2), "一条意图展开成 2 条命令");
    assert_eq!(value["staged"], json!(2));
    assert_eq!(value["intents"], json!(1), "两盏灯仍算一步");

    assert!(scene.borrow().lights.is_empty(), "暂存期不许碰主场景的灯");

    let committed = api.tx_commit(&tx).expect("提交不该失败");
    assert_eq!(committed["committed"], json!(2), "提交的是命令条数");
    assert_eq!(scene.borrow().lights.len(), 2, "两盏灯应落地");
    assert_eq!(scene.borrow().undo_depth(), 1, "两盏灯只占一格撤销");

    scene.borrow_mut().undo().expect("撤销不该失败");
    assert!(
        scene.borrow().lights.is_empty(),
        "一次 undo 把两盏灯全退干净"
    );
    assert_eq!(scene.borrow().undo_depth(), 0);
}

#[test]
fn tx_intent_place_light_参数整形错误码() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let missing = api
        .tx_intent(&tx, "scene.place_light", &json!({}))
        .expect_err("缺 lights 应报错");
    assert_eq!(missing.code, INVALID_PARAMS, "{}", missing.message);
    assert!(
        missing.message.contains("lights"),
        "错误信息应点名 lights：{}",
        missing.message
    );

    let empty = api
        .tx_intent(&tx, "scene.place_light", &lights(json!([])))
        .expect_err("空数组应报错");
    assert_eq!(empty.code, INVALID_PARAMS, "{}", empty.message);
    assert!(
        empty.message.contains("不能为空"),
        "空意图的拒绝理由要写清：{}",
        empty.message
    );

    let wrong_type = api
        .tx_intent(&tx, "scene.place_light", &lights(json!({ "key": "fill" })))
        .expect_err("lights 不是数组应报错");
    assert_eq!(wrong_type.code, INVALID_PARAMS, "{}", wrong_type.message);

    // `key` 是撤销找回这盏灯的唯一凭据，缺了就不许进事务。
    let no_key = api
        .tx_intent(
            &tx,
            "scene.place_light",
            &lights(json!([{ "intensity": 2.0 }])),
        )
        .expect_err("key 为空应报错");
    assert_eq!(no_key.code, INVALID_PARAMS, "{}", no_key.message);
    assert!(
        no_key.message.contains("key"),
        "错误信息应点名 key：{}",
        no_key.message
    );

    // 上限就是局部光源表容量，多一盏都不收。
    let too_many: Vec<Value> = (0..=16)
        .map(|i| json!({ "key": format!("l{i}") }))
        .collect();
    let over = api
        .tx_intent(&tx, "scene.place_light", &lights(Value::Array(too_many)))
        .expect_err("超过光源表容量应报错");
    assert_eq!(over.code, INVALID_PARAMS, "{}", over.message);
    assert!(
        over.message.contains("最多"),
        "错误信息应给出上限：{}",
        over.message
    );

    let preview = api.tx_preview(&tx).expect("事务应保留");
    assert_eq!(preview["staged"], json!(0), "被拒的意图不该留下暂存");
}

#[test]
fn tx_intent_place_light_重key或表满都拒绝且不碰场景() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let dup = api
        .tx_intent(
            &tx,
            "scene.place_light",
            &lights(json!([{ "key": "fill" }, { "key": "fill" }])),
        )
        .expect_err("同批重 key 应拒绝");
    assert_eq!(dup.code, TX_REJECTED, "{}", dup.message);
    assert!(
        dup.message.contains("重复"),
        "拒绝理由应说明重 key：{}",
        dup.message
    );

    // 再正常放一盏 `fill` 并提交：`place_light` 只新建不覆盖。
    api.tx_intent(
        &tx,
        "scene.place_light",
        &lights(json!([{ "key": "fill", "intensity": 2.0 }])),
    )
    .expect("同批重 key 被拒后，事务仍可继续攒");
    api.tx_commit(&tx).expect("提交不该失败");
    assert_eq!(scene.borrow().lights.len(), 1, "这盏该落地");

    let tx = begin(&mut api);

    let exists = api
        .tx_intent(
            &tx,
            "scene.place_light",
            &lights(json!([{ "key": "fill" }])),
        )
        .expect_err("与场景已有灯光重 key 应拒绝");
    assert_eq!(exists.code, TX_REJECTED, "{}", exists.message);
    assert!(
        exists.message.contains("已存在"),
        "拒绝理由应说明已存在：{}",
        exists.message
    );

    // 光源表只剩 15 个空位，一次要 16 盏 → 预检拦下。
    let too_many: Vec<Value> = (0..16).map(|i| json!({ "key": format!("n{i}") })).collect();
    let full = api
        .tx_intent(&tx, "scene.place_light", &lights(Value::Array(too_many)))
        .expect_err("空位不够应拒绝");
    assert_eq!(full.code, TX_REJECTED, "{}", full.message);
    assert!(
        full.message.contains("空位"),
        "拒绝理由应给出剩余空位：{}",
        full.message
    );

    let preview = api.tx_preview(&tx).expect("事务应保留");
    assert_eq!(preview["staged"], json!(0), "被预检拦下的两次没留下暂存");
    assert_eq!(scene.borrow().lights.len(), 1, "被拒的意图不许动场景");
    assert_eq!(
        scene.borrow().undo_depth(),
        1,
        "只有先提交的那一笔占一格撤销"
    );
}
