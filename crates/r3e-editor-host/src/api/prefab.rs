//! `prefab.*` 六条在宿主侧的落地（M2-E04）。
//!
//! 两条带 rpc（`prefab.spawn` / `prefab.info`）走 [`PrefabApi`] 的实现块，是跨进程
//! 读写入口。四条无 rpc（`prefab_define` / `prefab_spawn_many` / `prefab_set_override` /
//! `prefab_clear_override`）在宿主侧**只留本地调用口**、不进 `RPC_METHODS`
//! （`M2_E_PLAN` `P5` = B 的留痕口径）；脚本侧那一份在 `r3e-script/src/api/prefab.rs`
//! 独立实现——`r3e-script` 不依赖 `r3e-runtime`（后者反向依赖前者），两侧共用的是
//! `r3e-core` 的 `Scene` 接口，不是这里的函数。
//!
//! 这四条里 `prefab_define` 有真实进程内调用者（装配层与集成测试，见
//! `api/mod.rs` 的 `pub use prefab::prefab_define`），因此是 `pub`；另外三条
//! 眼下**没有进程内调用者**（`#[allow(dead_code)]` 逐条标注理由）：
//! 保留是因为 `P5` 明写它们「处理函数仍在」，一旦 §13 `R7` 的触发条件成立
//! （CLI 需远程建模板 / 部署目标无 `assets/prefab/`），补 rpc 只需在 `RPC_METHODS`
//! 与 dispatch 上加一行，不必从零再写一遍参数校验与错误分流。
//!
//! 三条纪律：
//!
//! * **写操作全部走 `Document::write`**：实例表进 `state_hash`（`P3` = A），
//!   绕过 `write` 就等于绕过撤销栈与脏标记，`prefab.spawn` 会看起来「没改场景」。
//! * **只读不写**：`prefab.info` 只 `borrow`，`StateHash` 前后一致。
//! * **两种错分开**：`Invalid` 是参数问题（`INVALID_PARAMS`），`Conflict` 是
//!   「引用了一个不存在的东西」（模板没登记 / 实例不在）——合成一个码之后，
//!   调用方分不清「该先 define」还是「参数写错了」。

use super::*;
use r3e_editor::PREFAB_REFERENCE_MISSING;
use r3e_runtime::prefab_ops::{self as ops, PrefabOpError};

/// 运行时 prefab 操作的错误 → 协议失败（分工同 `api::nav::map_op`）。
fn map_op(err: PrefabOpError) -> RpcFailure {
    match err {
        PrefabOpError::Invalid(detail) => RpcFailure::invalid_params(detail),
        // 码名取「引用缺失」而不是「模板缺失」：`Conflict` 也覆盖「实例名不在
        // 场景里」这类引用失败，码名写窄了就会有人把实例那半边硬塞进参数错。
        PrefabOpError::Conflict(detail) => RpcFailure::new(PREFAB_REFERENCE_MISSING, detail),
    }
}

/// `prefab.spawn`：按模板 ID 造一个实例节点。
pub(super) fn prefab_spawn(
    api: &mut LocalEngineApi<'_>,
    req: PrefabSpawnRequest,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let out =
                ops::spawn(scene, &req.id, &req.name, req.parent.as_deref()).map_err(map_op)?;
            // 实例节点是刚 `spawn` 出来的，世界矩阵要立刻算：否则同一个 tick 里
            // 紧接着的 `scene.get` 会读到未更新的 world。
            scene.update_world_transforms();
            Ok(json!({
                "name": out.name,
                "template": out.template,
                "parent": out.parent,
                "nodeId": out.node,
                // 刚实例化 = 覆盖表为空对象。回出来是为了让「干净实例」可验证，
                // 不必再发一条 `prefab.info` 才能确认。
                "overrides": {},
            }))
        })
}

/// `prefab.info`：模板 ID + 逐字段覆盖态（只读）。
pub(super) fn prefab_info(
    api: &mut LocalEngineApi<'_>,
    req: PrefabInfoRequest,
) -> Result<Value, RpcFailure> {
    let scene = api.scene.borrow();
    let out = ops::info(&scene, &req.name).map_err(map_op)?;
    let overrides: Vec<Value> = out
        .overrides
        .iter()
        .map(|(field, state)| json!({ "field": field, "state": state }))
        .collect();
    Ok(json!({
        "name": out.name,
        "template": out.template,
        "parent": out.parent,
        "node": out.node,
        "count": overrides.len(),
        "overrides": overrides,
        "fields": out.fields,
        "templateRegistered": out.template_registered,
    }))
}

/// `prefab_define`（无 rpc）：把一份模板文件登记进场景模板表。
///
/// `pub` 而非 `pub(super)`：它是本 crate 唯一「无 rpc 但有真实进程内调用者」的
/// prefab 方法（装配层载入 `assets/prefab/`、`tests/prefab_api.rs` 验判据），
/// 由 `api/mod.rs` 的 `pub use` 对外，见那里的注释。
pub fn prefab_define(
    api: &mut LocalEngineApi<'_>,
    id: &str,
    path: &str,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let out = ops::define(scene, id, path).map_err(map_op)?;
            Ok(json!({
                "id": out.id,
                "replaced": out.replaced,
                "path": out.path,
                "proto": out.proto_name,
                "tags": out.tags,
            }))
        })
}

/// `prefab_spawn_many`（无 rpc）：按 `<prefix><序号>` 批量实例化。
#[allow(dead_code)] // `prefab_spawn_many` 无 rpc（`P5` = B）：批量入口只在脚本侧注册
pub(super) fn prefab_spawn_many(
    api: &mut LocalEngineApi<'_>,
    id: &str,
    prefix: &str,
    count: i64,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let created = ops::spawn_many(scene, id, prefix, count).map_err(map_op)?;
            scene.update_world_transforms();
            Ok(json!({
                "count": created,
                "prefix": prefix.trim(),
                "nodes": scene.len(),
            }))
        })
}

/// `prefab_set_override`（无 rpc）：写一个覆盖字段并同步节点。
#[allow(dead_code)] // `prefab_set_override` 无 rpc（`P5` = B）：覆盖写入只在脚本侧注册
pub(super) fn prefab_set_override(
    api: &mut LocalEngineApi<'_>,
    name: &str,
    field: &str,
    value: &Value,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            ops::set_override(scene, name, field, value).map_err(map_op)?;
            scene.update_world_transforms();
            Ok(json!({ "name": name, "field": field, "state": "set" }))
        })
}

/// `prefab_clear_override`（无 rpc）：把一个字段退回继承模板。
#[allow(dead_code)] // `prefab_clear_override` 无 rpc（`P5` = B）：覆盖回退只在脚本侧注册
pub(super) fn prefab_clear_override(
    api: &mut LocalEngineApi<'_>,
    name: &str,
    field: &str,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let changed = ops::clear_override(scene, name, field).map_err(map_op)?;
            scene.update_world_transforms();
            Ok(json!({
                "name": name,
                "field": field,
                "state": "inherit",
                // `changed=false` = 本来就没覆盖过，属正常结果而非失败：
                // 幂等清理不该报错，但要让调用方看得见「这次什么都没动」。
                "changed": changed,
            }))
        })
}
