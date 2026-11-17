//! 事务 RPC 宿主实现（M1-S1）：
//! `tx.begin` / `tx.apply` / `tx.preview` / `tx.commit` / `tx.rollback`。
//!
//! 事务状态挂在 [`r3e_doc::Document`] 上（[`r3e_doc::Document::tx_begin`] 家族），
//! 不在 [`LocalEngineApi`] 里——每次 RPC 都新建一个 `LocalEngineApi`，
//! 放这里的字段活不过一次请求。
//!
//! 本文件只负责两件「宿主该管」的事：
//!
//! 1. **参数整形**：把 RPC 的 `args` 解析成纯数据（`scene.settransform` 的
//!    `position` / `rotation` / `scale` 之类），在这里就做有限性与非零四元数校验——
//!    校验失败属于调用方的错（`INVALID_PARAMS`），不该算进事务内。
//! 2. **意图 → 命令**：在影子场景上「看着当前状态」补前像，构造 [`CommandKind`]。
//!    只有这一步需要 `&Scene`，因此它被放进 `tx_apply` 的闭包里。
//!
//! 白名单（M1-S1 首版，见 `docs/WRITE_SITES.md`）：
//!
//! | `op` | 命令 |
//! | --- | --- |
//! | `scene.spawn` | [`CommandKind::spawn_node`] |
//! | `scene.settransform` | [`CommandKind::set_transform`] |
//! | `scene.setvisible` | [`CommandKind::set_visible`] |
//! | `light.remove` | [`CommandKind::remove_local_light`] |
//! | `ui.remove` | [`CommandKind::remove_ui`] |
//!
//! 其余写点（`scene.remove` / `scene.setmaterial` / `light.set` / `ui.set` / 环境类）
//! 目前**没有**对应命令（`CommandKind` 尚未覆盖，或 patch → 对象的合并逻辑仍在
//! RPC 实现里直写场景），调用即回 `TX_REJECTED`。这个表只许增加命令后同步扩白名单，
//! 不允许为了「凑齐」在事务里绕过命令直接改场景。

use super::support::*;
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

use r3e_doc::{CommandKind, DocError, StateHash};
use r3e_editor::{TX_APPLY_FAILED, TX_BUSY, TX_CONFLICT, TX_NOT_FOUND, TX_REJECTED};

/// 进程内事务 id 序列（`tx-1`、`tx-2`……）。
///
/// `txId` 只在本次进程的会话里有意义：重启后未决事务本就随文档一起没了，
/// 所以不做持久化唯一性。
static TX_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_tx_id() -> String {
    format!("tx-{}", TX_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 哈希的出网形态：`0x` + 16 位十六进制。
///
/// 不直接给 `u64` 数值——JSON 数字进 JS 会被 `Number` 截到 53 位有效位，
/// 事务冲突判定恰好依赖这个值。
fn hex(hash: StateHash) -> String {
    format!("0x{:016x}", hash.raw())
}

/// [`DocError`] → RPC 失败，按变体给码。
fn map_doc(err: DocError) -> RpcFailure {
    let message = err.detail().to_string();
    let code = match &err {
        DocError::Busy(_) => TX_BUSY,
        DocError::TxNotFound(_) => TX_NOT_FOUND,
        DocError::Conflict(_) => TX_CONFLICT,
        DocError::Rejected(_) => TX_REJECTED,
        _ => TX_APPLY_FAILED,
    };
    RpcFailure::new(code, message)
}

fn required_str(args: &Value, key: &str) -> Result<String, RpcFailure> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) | None => {
            Err(RpcFailure::invalid_params(format!("参数 `{key}` 不能为空")))
        }
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 需要字符串"
        ))),
    }
}

/// 一条已解析、尚未落到场景上的写入意图。
///
/// 与 RPC 参数一一对应，但**不含前像**——前像要等进到影子场景里才知道。
enum Intent {
    Spawn(Box<ObjectDesc>),
    Transform {
        node: String,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 4]>,
        scale: Option<[f32; 3]>,
    },
    Visible {
        node: String,
        visible: bool,
    },
    RemoveLight(String),
    RemoveUi(String),
}

impl Intent {
    /// 解析 `args`。所有会让调用方「补参数就能过」的校验都在这里做，
    /// 留下的失败才是场景本身的失败（物件不存在等）。
    fn parse(op: &str, args: &Value) -> Result<Self, RpcFailure> {
        match op {
            "scene.spawn" => {
                let desc: ObjectDesc = serde_json::from_value(args.clone())
                    .map_err(|e| RpcFailure::invalid_params(format!("物件描述不合法: {e}")))?;
                Ok(Self::Spawn(Box::new(desc)))
            }
            "scene.settransform" => {
                let node = required_str(args, "node")?;
                let position = optional_array::<3>(args, "position")?;
                let rotation = optional_array::<4>(args, "rotation")?;
                let scale = optional_array::<3>(args, "scale")?;
                if position.is_none() && rotation.is_none() && scale.is_none() {
                    return Err(RpcFailure::invalid_params("变换补丁为空"));
                }
                if let Some(v) = &position {
                    check_finite(v, "position")?;
                }
                if let Some(v) = &scale {
                    check_finite(v, "scale")?;
                }
                if let Some(v) = &rotation {
                    check_finite(v, "rotation")?;
                    let quat = Quat::from_xyzw(v[0], v[1], v[2], v[3]);
                    if quat.length_squared() < 1e-12 {
                        return Err(RpcFailure::invalid_params(
                            "rotation 需要非零四元数 [x,y,z,w]（不能全为 0）",
                        ));
                    }
                }
                Ok(Self::Transform {
                    node,
                    position,
                    rotation,
                    scale,
                })
            }
            "scene.setvisible" => {
                let node = required_str(args, "node")?;
                let visible = match args.get("visible") {
                    Some(Value::Bool(v)) => *v,
                    Some(_) => return Err(RpcFailure::invalid_params("参数 `visible` 需要布尔值")),
                    None => return Err(RpcFailure::invalid_params("缺少参数 `visible`")),
                };
                Ok(Self::Visible { node, visible })
            }
            "light.remove" => Ok(Self::RemoveLight(required_str(args, "key")?)),
            "ui.remove" => Ok(Self::RemoveUi(required_str(args, "key")?)),
            other => Err(RpcFailure::new(
                TX_REJECTED,
                format!(
                    "`{other}` 未纳入事务白名单（M1-S1 首版仅 scene.spawn / \
                     scene.settransform / scene.setvisible / light.remove / ui.remove）"
                ),
            )),
        }
    }

    /// 在影子场景上补前像、造命令。
    fn build(self, scene: &Scene) -> Result<CommandKind, DocError> {
        match self {
            Self::Spawn(desc) => Ok(CommandKind::spawn_node(*desc)),
            Self::Transform {
                node,
                position,
                rotation,
                scale,
            } => {
                let id = find_node_doc(scene, &node)?;
                let mut next = scene
                    .get(id)
                    .ok_or_else(|| DocError::NotFound(format!("物件 `{node}` 不存在")))?
                    .transform;
                if let Some(v) = position {
                    next.position = Vec3::from_array(v);
                }
                if let Some(v) = rotation {
                    next.rotation = Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize();
                }
                if let Some(v) = scale {
                    next.scale = Vec3::from_array(v);
                }
                CommandKind::set_transform(scene, id, next)
            }
            Self::Visible { node, visible } => {
                let id = find_node_doc(scene, &node)?;
                CommandKind::set_visible(scene, id, visible)
            }
            Self::RemoveLight(key) => CommandKind::remove_local_light(scene, &key),
            Self::RemoveUi(key) => CommandKind::remove_ui(scene, &key),
        }
    }
}

/// 按名找 `NodeId`，失败回 `DocError`（`find_node_id` 回的是 `RpcFailure`，
/// 这里已经在事务闭包里，错误得走文档层的口径）。
fn find_node_doc(scene: &Scene, key: &str) -> Result<r3e_core::NodeId, DocError> {
    scene
        .find_id(key)
        .ok_or_else(|| DocError::NotFound(format!("物件 `{key}` 不存在")))
}

/// 取一个长度为 `N` 的数字数组（缺省 / `null` → `None`）。
///
/// 不用 `serde_json::from_value::<[f32; N]>`：serde 只为 0..=32 的**具体**长度实现
/// 了 `Deserialize`，泛型 `N` 编不过。
fn optional_array<const N: usize>(args: &Value, key: &str) -> Result<Option<[f32; N]>, RpcFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            if items.len() != N {
                return Err(RpcFailure::invalid_params(format!(
                    "参数 `{key}` 需要长度 {N} 的数组"
                )));
            }
            let mut out = [0.0_f32; N];
            for (slot, item) in out.iter_mut().zip(items) {
                let value = item.as_f64().ok_or_else(|| {
                    RpcFailure::invalid_params(format!("参数 `{key}` 需要数字数组"))
                })?;
                *slot = value as f32;
            }
            Ok(Some(out))
        }
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 需要长度 {N} 的数组"
        ))),
    }
}

pub(super) fn tx_begin(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let id = next_tx_id();
    let begun = api.scene.borrow_mut().tx_begin(id).map_err(map_doc)?;
    Ok(json!({
        "txId": begun.id,
        "baseHash": hex(begun.base),
        "version": begun.version,
        "staged": 0,
    }))
}

pub(super) fn tx_apply(
    api: &mut LocalEngineApi<'_>,
    tx_id: &str,
    op: &str,
    args: &Value,
) -> Result<Value, RpcFailure> {
    let intent = Intent::parse(op, args)?;
    let staged = api
        .scene
        .borrow_mut()
        .tx_apply(tx_id, op, |scene| intent.build(scene))
        .map_err(map_doc)?;
    Ok(json!({
        "txId": tx_id,
        "op": staged.op,
        "staged": staged.staged,
    }))
}

pub(super) fn tx_preview(api: &mut LocalEngineApi<'_>, tx_id: &str) -> Result<Value, RpcFailure> {
    let preview = api.scene.borrow().tx_preview(tx_id).map_err(map_doc)?;
    Ok(json!({
        "txId": tx_id,
        "staged": preview.staged,
        "base": hex(preview.base),
        "current": hex(preview.current),
        "shadow": hex(preview.shadow),
    }))
}

pub(super) fn tx_commit(api: &mut LocalEngineApi<'_>, tx_id: &str) -> Result<Value, RpcFailure> {
    let committed = api.scene.borrow_mut().tx_commit(tx_id).map_err(map_doc)?;
    Ok(json!({
        "txId": tx_id,
        "committed": committed.committed,
        "hash": hex(committed.hash),
        "version": committed.version,
    }))
}

pub(super) fn tx_rollback(api: &mut LocalEngineApi<'_>, tx_id: &str) -> Result<Value, RpcFailure> {
    let rolled = api.scene.borrow_mut().tx_rollback(tx_id).map_err(map_doc)?;
    Ok(json!({
        "txId": tx_id,
        "dropped": rolled.dropped,
        "hash": hex(rolled.hash),
        "version": rolled.version,
    }))
}

/// 意图白名单（M1-S3-A）。每加一个意图，必须同时给出「展开成哪些命令」。
///
/// 白名单只活在宿主，不进协议：意图名是业务能力，跟引擎版本跑得不一样快。
const INTENTS: &[&str] = &["scene.spawn_prefab"];

/// `scene.spawn_prefab` 单次最多实例化多少个节点。
///
/// 一次意图展开成 N 条命令、占撤销栈一格，N 没有上限就等于给了 AI 一个
/// 「一笔改爆场景，一次 undo 全退」的开关。256 够搭一个常规道具组。
const SPAWN_PREFAB_MAX_NODES: usize = 256;

/// 一条已解析、尚未落到场景上的**意图**（M1-S3-A）。
///
/// 与 [`Intent`]（一条 `op` = 一条命令）的区别只在「展开粒度」：这里的每个变体
/// 展开成 **N 条** `CommandKind`，N 由参数决定。
enum IntentPlan {
    /// 一次性实例化一组节点：一组组件 = 一条意图 = 一格撤销。
    SpawnPrefab(Vec<ObjectDesc>),
}

impl IntentPlan {
    /// 参数整形。失败属于调用方的错（`INVALID_PARAMS` / `TX_REJECTED`），
    /// 此时事务一个字节都没动。
    fn parse(intent: &str, args: &Value) -> Result<Self, RpcFailure> {
        match intent {
            "scene.spawn_prefab" => {
                let items =
                    match args.get("nodes") {
                        Some(Value::Array(items)) => items,
                        Some(_) => return Err(RpcFailure::invalid_params(
                            "参数 `nodes` 需要数组（每项一个物件描述，字段与 `scene.spawn` 相同）",
                        )),
                        None => return Err(RpcFailure::invalid_params("缺少参数 `nodes`")),
                    };
                if items.is_empty() {
                    return Err(RpcFailure::invalid_params(
                        "参数 `nodes` 不能为空数组（空意图不攒进事务）",
                    ));
                }
                if items.len() > SPAWN_PREFAB_MAX_NODES {
                    return Err(RpcFailure::invalid_params(format!(
                        "参数 `nodes` 最多 {SPAWN_PREFAB_MAX_NODES} 项，收到 {}",
                        items.len()
                    )));
                }
                let mut nodes = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    let desc: ObjectDesc = serde_json::from_value(item.clone()).map_err(|e| {
                        RpcFailure::invalid_params(format!("`nodes[{i}]` 物件描述不合法: {e}"))
                    })?;
                    nodes.push(desc);
                }
                Ok(Self::SpawnPrefab(nodes))
            }
            other => Err(RpcFailure::new(
                TX_REJECTED,
                format!(
                    "`{other}` 未纳入意图白名单（M1-S3-A 仅 {}）",
                    INTENTS.join(" / ")
                ),
            )),
        }
    }

    /// 在影子场景上把意图展开成命令序列。
    ///
    /// 这里做的是**影子之外的预检**（重名冲突、父子引用能不能落地）：这些错一旦
    /// 留到 apply 才炸，就是「前 k 条命令已经改了影子再回退」，白白多一次回退。
    /// 预检通过后仍可能因场景状态在命令间变化而失败——那种才交给文档层逆序回退。
    fn build(self, scene: &Scene) -> Result<Vec<CommandKind>, DocError> {
        match self {
            Self::SpawnPrefab(nodes) => {
                let mut seen: Vec<&str> = Vec::with_capacity(nodes.len());
                for desc in &nodes {
                    let name = desc.name.as_str();
                    if seen.contains(&name) {
                        return Err(DocError::Rejected(format!(
                            "意图内节点名 `{name}` 重复（同批建的两个节点不能同名）"
                        )));
                    }
                    if scene.find_id(name).is_some() {
                        return Err(DocError::Rejected(format!(
                            "物件 `{name}` 已存在；`scene.spawn_prefab` 只新建不覆盖，请换名或先删"
                        )));
                    }
                    if let Some(parent) = &desc.parent {
                        let in_batch = seen.contains(&parent.as_str());
                        if !in_batch && scene.find_id(parent).is_none() {
                            return Err(DocError::Rejected(format!(
                                "节点 `{name}` 的父物件 `{parent}` 既不在本意图内、也不在场景里\
                                 （父物件须排在同一意图的前面，或已存在于场景）"
                            )));
                        }
                    }
                    seen.push(name);
                }
                Ok(nodes.into_iter().map(CommandKind::spawn_node).collect())
            }
        }
    }
}

/// `tx.intent`：把一条意图展开成 N 条命令暂存进事务。
///
/// 回包与 [`tx_apply`] 的区别是多了 `commands`（本条意图展开出几条）与 `intents`
/// （事务内累计步数）——`staged` 仍是命令总数，因为 `tx.commit` 提交的是命令条数。
pub(super) fn tx_intent(
    api: &mut LocalEngineApi<'_>,
    tx_id: &str,
    intent: &str,
    args: &Value,
) -> Result<Value, RpcFailure> {
    let plan = IntentPlan::parse(intent, args)?;
    let staged = api
        .scene
        .borrow_mut()
        .tx_apply_intent(tx_id, intent, |scene| plan.build(scene))
        .map_err(map_doc)?;
    Ok(json!({
        "txId": tx_id,
        "intent": staged.intent,
        "commands": staged.commands,
        "staged": staged.staged,
        "intents": staged.intents,
    }))
}
