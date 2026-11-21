//! `character.*` 六条的宿主实现（M2-B01）。
//!
//! 「配置」与「运行态」在这六条里泾渭分明，是本文件最要紧的一条线：
//!
//! * **配置**住在场景里（`Node::character`，[`CharacterSpec`]），写入只经命令层
//!   （[`CommandKind::SetNodeCharacter`] → 撤销栈 / 脏标记 / `state_hash` 全部照常）。
//! * **运行态**住在 [`CharacterHost`] 背后的控制器里（位置 / 竖直速度 / 是否着地），
//!   **一个字节都不进场景**：进了场景，同一关卡「角色站哪儿」就会改变 `state_hash`，
//!   回放金标与撤销栈跟着一起漂。
//!
//! 由此带出两个「看起来少做了一步」的地方，都是刻意的：
//!
//! 1. `character.step` / `character.teleport` **不写回节点 `Transform`**。步进只回新的
//!    脚底位置，写回是主循环（`r3e-runtime`）每帧同步的活。宿主这里顺手写场景，
//!    会让「一帧一次 RPC 步进」变成「一帧一次撤销栈条目」。
//!    回包里 `sceneSynced: false` 把这件事写在明面上，别让调用方以为节点已经动了。
//! 2. `character.state` 在控制器不存在时报 [`CHARACTER_NOT_READY`]，**不回全零位置**——
//!    全零位置会被读成「角色在世界原点站着」，然后去调地形（与 `NAV_NOT_BAKED` 同一教训）。
//!
//! 事务：角色配置写入暂不支持在 `tx` 内暂存（`CommandKind` 已覆盖，但 `tx.apply`
//! 的 op 白名单还没扩）。事务未决时明确回 [`TX_REJECTED`] 并说明先 `commit`/`rollback`，
//! 而不是绕开影子直写主场景——后者正是 S4-D 记过的那个洞。

use super::support::check_finite;
use super::*;
use r3e_core::host::{CharacterHost, CharacterStatsDto};
use r3e_core::CharacterSpec;
use r3e_doc::CommandKind;
use r3e_editor::{CHARACTER_NOT_READY, CHARACTER_SPEC_INVALID, TX_REJECTED};

use super::tx::map_doc;
use r3e_editor::character::CharacterBodyParams;

/// 运行时的固定步长口径（与主循环一致）：`character.step` 不传 `dt` 时用它。
const DEFAULT_DT: f32 = 1.0 / 60.0;

/// 取角色能力面；宿主没接（headless 测试）与「控制器不存在」是两件事，码不同。
fn host<'a>(api: &'a LocalEngineApi<'a>) -> Result<&'a Rc<dyn CharacterHost>, RpcFailure> {
    api.character.ok_or_else(|| {
        RpcFailure::new(
            CHARACTER_NOT_READY,
            "该宿主没有接入角色控制器（headless 运行常见）：角色命令需要运行时装配 `CharacterRuntime`",
        )
    })
}

fn not_ready(node: &str) -> RpcFailure {
    RpcFailure::new(
        CHARACTER_NOT_READY,
        format!("节点 `{node}` 还没有角色控制器——先调 `character.create`（它会把配置写进节点并建立控制器）"),
    )
}

/// [`CharacterSpec`] → 回包（字段名与入参 camelCase 对齐）。
fn spec_value(spec: &CharacterSpec) -> Value {
    json!({
        "radius": spec.radius,
        "height": spec.height,
        "slopeLimitDeg": spec.slope_limit_deg,
        "stepHeight": spec.step_height,
        "snapDistance": spec.snap_distance,
    })
}

fn state_value(node: &str, st: r3e_core::host::CharacterStateDto) -> Value {
    json!({
        "node": node,
        "position": st.position,
        "verticalSpeed": st.vertical_speed,
        "grounded": st.grounded,
        "blocked": st.blocked,
        // 位置只是控制器的运行态：节点 `Transform` 由主循环同步，这里没动场景。
        "sceneSynced": false,
    })
}

fn stats_value(stats: CharacterStatsDto, backend: &str) -> Value {
    json!({
        "controllers": stats.controllers,
        "steps": stats.steps,
        "blocked": stats.blocked,
        "stepUps": stats.step_ups,
        "backend": backend,
    })
}

/// 场景现值 + 本次入参 → 目标配置。
///
/// `create` 与 `config` 共用这一份：基线取「节点当前配置」，节点还不是角色时取
/// [`CharacterSpec::default`]。于是 `create` 没给的字段得到缺省值、`config` 没给的
/// 字段保持现值——同一段代码，两种语义，靠基线自然分开。
fn resolve_spec(base: Option<CharacterSpec>, body: CharacterBodyParams) -> CharacterSpec {
    let mut spec = base.unwrap_or_default();
    if let Some(v) = body.radius {
        spec.radius = v;
    }
    if let Some(v) = body.height {
        spec.height = v;
    }
    if let Some(v) = body.slope_limit_deg {
        spec.slope_limit_deg = v;
    }
    if let Some(v) = body.step_height {
        spec.step_height = v;
    }
    if let Some(v) = body.snap_distance {
        spec.snap_distance = v;
    }
    spec
}

/// 把校验过的配置写进节点（经命令层，可撤销）。
///
/// 返回「写之前节点是否已经是角色」，供 `created` 字段用。
fn write_spec(
    api: &mut LocalEngineApi<'_>,
    node: &str,
    spec: CharacterSpec,
) -> Result<bool, RpcFailure> {
    let mut document = api.scene.borrow_mut();
    if document.tx_session().is_some() {
        return Err(RpcFailure::new(
            TX_REJECTED,
            "角色配置写入暂不进事务影子：先 `tx.commit` 或 `tx.rollback`，再改 `character.*`",
        ));
    }
    let existed = document
        .find(node)
        .ok_or_else(|| RpcFailure::engine(format!("找不到节点 `{node}`")))?
        .character
        .is_some();
    let cmd = CommandKind::set_node_character(&document, node, Some(spec)).map_err(map_doc)?;
    document.apply(&cmd).map_err(map_doc)?;
    Ok(existed)
}

/// 按场景现值建立/刷新控制器。配置非法时**不建**（宁可不生效也不产生 NaN 角色）。
fn bind(api: &LocalEngineApi<'_>, node: &str) -> Result<(), RpcFailure> {
    let character = host(api)?;
    let document = api.scene.borrow();
    let target = document
        .find(node)
        .ok_or_else(|| RpcFailure::engine(format!("找不到节点 `{node}`")))?;
    if character.bind(target) {
        Ok(())
    } else {
        let detail = target
            .character
            .and_then(|spec| spec.validate().err().map(|e| e.to_string()))
            .unwrap_or_else(|| "节点上没有角色配置".to_string());
        Err(RpcFailure::new(
            CHARACTER_SPEC_INVALID,
            format!("节点 `{node}` 的角色配置建不起控制器：{detail}"),
        ))
    }
}

/// `character.create` / `character.config` 的共同实现。
///
/// 两者只差 `created` 的判定与文档语义：`create` 也接受「本来就已经是角色」的情况
/// （等价于改配置），**不报错**——脚本重复初始化是常态，报错只会逼出「先 state 再 create」
/// 这种啰嗦写法。
fn upsert(
    api: &mut LocalEngineApi<'_>,
    q: CharacterBindQuery,
    is_create: bool,
) -> Result<Value, RpcFailure> {
    // 先确认宿主接了控制器，再动场景：headless 下「配置写进节点、控制器却建不起来」
    // 会留下一个看似是角色却永远不动的节点，比在第一步就报错难查得多。
    host(api)?;
    let base = {
        let document = api.scene.borrow();
        document
            .find(&q.node)
            .ok_or_else(|| RpcFailure::engine(format!("找不到节点 `{}`", q.node)))?
            .character
    };
    let spec = resolve_spec(base, q.body);
    spec.validate()
        .map_err(|err| RpcFailure::new(CHARACTER_SPEC_INVALID, format!("角色参数不合法：{err}")))?;

    let existed = write_spec(api, &q.node, spec)?;
    bind(api, &q.node)?;
    Ok(json!({
        "node": q.node,
        "created": is_create && !existed,
        "updated": existed,
        "character": spec_value(&spec),
    }))
}

/// `character.create`：节点没有配置就按缺省值 + 入参建，已有则等价于 `config`。
pub(super) fn character_create(
    api: &mut LocalEngineApi<'_>,
    q: CharacterBindQuery,
) -> Result<Value, RpcFailure> {
    upsert(api, q, true)
}

/// `character.config`：只改给了的字段，其余保持现值。
pub(super) fn character_config(
    api: &mut LocalEngineApi<'_>,
    q: CharacterBindQuery,
) -> Result<Value, RpcFailure> {
    upsert(api, q, false)
}

/// `character.step`：推进一帧，回新的脚底位置（**不写回节点**）。
pub(super) fn character_step(
    api: &mut LocalEngineApi<'_>,
    q: CharacterStepQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.wish, "wish")?;
    let dt = q.dt.unwrap_or(DEFAULT_DT);
    // NaN 走 `is_finite` 兜底：`NaN <= 0.0` 是 false，光比大小会放它过去。
    if dt <= 0.0 || !dt.is_finite() {
        return Err(RpcFailure::invalid_params(format!(
            "参数 `dt` 必须为正数，收到 {dt}"
        )));
    }
    let character = host(api)?;
    let document = api.scene.borrow();
    let target = document
        .find(&q.node)
        .ok_or_else(|| RpcFailure::engine(format!("找不到节点 `{}`", q.node)))?;

    let wish = [q.wish[0] as f64, q.wish[1] as f64];
    let dt = dt as f64;
    // 建过配置但没建控制器（换个进程跑脚本 / 场景刚读回来）时先补一次 bind：
    // 这比让调用方「自己保证先 create」可靠，也省掉一条 RPC。
    let outcome = character
        .step(target, wish, q.jump, dt)
        .or_else(|| {
            character
                .bind(target)
                .then(|| character.step(target, wish, q.jump, dt))
                .flatten()
        })
        .ok_or_else(|| not_ready(&q.node))?;
    Ok(state_value(&q.node, outcome))
}

/// `character.state`：只读运行态，不动场景、不动控制器。
pub(super) fn character_state(
    api: &mut LocalEngineApi<'_>,
    q: CharacterNameQuery,
) -> Result<Value, RpcFailure> {
    let character = host(api)?;
    let st = character.state(&q.node).ok_or_else(|| not_ready(&q.node))?;
    Ok(state_value(&q.node, st))
}

/// `character.teleport`：把脚底落到指定位置并清掉竖直速度。
pub(super) fn character_teleport(
    api: &mut LocalEngineApi<'_>,
    q: CharacterTeleportQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.position, "position")?;
    let character = host(api)?;
    // 节点存在性先校验：控制器可能还在，但节点刚被删——那时该报「找不到节点」。
    {
        let document = api.scene.borrow();
        if document.find(&q.node).is_none() {
            return Err(RpcFailure::engine(format!("找不到节点 `{}`", q.node)));
        }
    }
    let mut position = [0.0_f64; 3];
    for (dst, src) in position.iter_mut().zip(q.position) {
        *dst = src as f64;
    }
    if !character.teleport(&q.node, position) {
        return Err(not_ready(&q.node));
    }
    let st = character.state(&q.node).ok_or_else(|| not_ready(&q.node))?;
    Ok(state_value(&q.node, st))
}

/// `character.stats`：全局汇总 + 后端名（用来分辨「没动过」和「没接」）。
pub(super) fn character_stats(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let character = host(api)?;
    Ok(stats_value(character.stats(), &character.backend()))
}
