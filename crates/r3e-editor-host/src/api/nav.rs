//! `nav.*` 三条的宿主实现（M1-S4-D）。
//!
//! 导航网格是**烘焙产物**，不是场景数据，所以它住在宿主槽位
//! （[`LocalEngineApi::nav`]）而不是 `Document` 里：写进场景会让它参与
//! `state_hash`，「烘一次导航」看起来就像「改了场景」，撤销栈与脏标记全失真。
//!
//! 三条铁律的落地点：
//!
//! * **没烘过回 [`NAV_NOT_BAKED`]，不回空表**——空路径会被读成「场景里真的
//!   走不通」，从而去改本来就对的关卡。
//! * **只读**：`nav.path` / `nav.reachability` 只 `borrow` 槽位，
//!   `StateHash` 前后一致；`nav.bake` 只写槽位，不写节点。
//! * **如实带出坏消息**：`unresolved` / `floatingSpawns` 原样回传，
//!   它们是「这份网格不可信」的直接证据。

use super::support::check_finite;
use super::*;
use r3e_editor::NAV_NOT_BAKED;
use r3e_nav::{Nav, NavConfig, PathOutcome};

/// 统一的「没烘过」错误：说清是什么、该怎么办，且不暗示场景本身有问题。
fn not_baked(detail: &str) -> RpcFailure {
    RpcFailure::new(
        NAV_NOT_BAKED,
        format!("导航网格还没烘过：{detail}——先调 `nav.bake`，当前没有任何网格可查"),
    )
}

/// 取宿主导航槽位；宿主没接槽位与「槽位是空的」对调用方是同一件事。
fn nav_slot<'a>(api: &'a LocalEngineApi<'a>) -> Result<&'a RefCell<Option<Nav>>, RpcFailure> {
    api.nav
        .ok_or_else(|| not_baked("该宿主未接导航槽位（headless 运行常见）"))
}

/// 协议侧平铺选项 → `NavConfig`。
///
/// 缺省一律取 `NavConfig::default()`（默认值只有一个出处，不抄到协议层）；
/// 显式给的**非有限值**判参数错误而不是静默回默认——「给了 NaN 却没报错」
/// 才是最难查的那种不一致。区间夹取交给 `NavConfig::sanitized`，与引擎同口径。
fn nav_config(opts: NavBakeOptions) -> Result<NavConfig, RpcFailure> {
    let check = |value: Option<f32>, key: &str| -> Result<(), RpcFailure> {
        match value {
            Some(v) if !v.is_finite() => {
                Err(RpcFailure::invalid_params(format!("{key} 不是有限值")))
            }
            _ => Ok(()),
        }
    };
    check(opts.cell_size, "cellSize")?;
    check(opts.agent_radius, "agentRadius")?;
    check(opts.agent_height, "agentHeight")?;
    check(opts.step_height, "stepHeight")?;
    check(opts.max_slope_deg, "maxSlopeDeg")?;
    if let Some(bounds) = opts.bounds {
        check_finite(&bounds, "bounds")?;
        if bounds[0] > bounds[2] || bounds[1] > bounds[3] {
            return Err(RpcFailure::invalid_params(
                "bounds 必须是 [minX, minZ, maxX, maxZ] 且 min 不大于 max",
            ));
        }
    }

    let mut config = NavConfig::default();
    if let Some(v) = opts.cell_size {
        config.cell_size = v;
    }
    if let Some(v) = opts.agent_radius {
        config.agent_radius = v;
    }
    if let Some(v) = opts.agent_height {
        config.agent_height = v;
    }
    if let Some(v) = opts.step_height {
        config.step_height = v;
    }
    if let Some(v) = opts.max_slope_deg {
        config.max_slope_deg = v;
    }
    if let Some(v) = opts.bounds {
        config.bounds = Some(v);
    }
    // `ignoreTags` 是整体替换：给空表 = 谁都不忽略，给 `None` 才用缺省忽略表。
    if let Some(tags) = opts.ignore_tags {
        config.ignore_tags = tags;
    }
    Ok(config.sanitized())
}

/// 烘焙回包（`baked` 一定为 `true`：失败走错误码，不返回半真半假的结果）。
fn bake_report(nav: &Nav) -> Value {
    let report = nav.report();
    json!({
        "baked": true,
        "dims": report.dims,
        "cellSize": report.cell_size,
        "cells": report.cells,
        "walkable": report.walkable,
        "walkableRatio": report.walkable_ratio(),
        "regions": report.regions,
        "regionSizes": report.region_sizes,
        "spawns": report
            .spawns
            .iter()
            .map(|(name, region)| json!({ "name": name, "region": region }))
            .collect::<Vec<_>>(),
        "unresolved": report.unresolved,
        "floatingSpawns": report.floating_spawns(),
    })
}

/// `nav.bake`：按场景烘一份网格写进宿主槽位。
pub(super) fn nav_bake(
    api: &mut LocalEngineApi<'_>,
    opts: NavBakeOptions,
) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let config = nav_config(opts)?;
    let document = api.scene.borrow();
    let scene: &Scene = &document;
    // 先烘到局部变量：烘焙失败不许把上一次成功的网格冲成「没烘过」。
    let nav = Nav::bake_scene_with(scene, config)
        .map_err(|err| RpcFailure::invalid_params(format!("导航烘焙失败：{err}")))?;
    let report = bake_report(&nav);
    drop(document);
    *slot.borrow_mut() = Some(nav);
    Ok(report)
}

/// `nav.path`：A* 求一条可走路径，四种结局如实区分。
pub(super) fn nav_path(api: &mut LocalEngineApi<'_>, q: NavPathQuery) -> Result<Value, RpcFailure> {
    check_finite(&q.from, "from")?;
    check_finite(&q.to, "to")?;
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;

    let outcome = nav.path(Vec3::from_array(q.from), Vec3::from_array(q.to));
    let reason = outcome.describe();
    let (found, kind, points) = match &outcome {
        PathOutcome::Found(points) => (
            true,
            "found",
            points.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
        ),
        // 「走不到」和「算不完」对调用方是完全不同的下一步，不能合并成 found:false。
        PathOutcome::Unreachable { .. } => (false, "unreachable", Vec::new()),
        PathOutcome::InvalidEndpoint { .. } => (false, "invalidEndpoint", Vec::new()),
        PathOutcome::BudgetExceeded { .. } => (false, "budgetExceeded", Vec::new()),
    };
    let count = points.len();
    Ok(json!({
        "found": found,
        "outcome": kind,
        "reason": reason,
        "points": points,
        "count": count,
    }))
}

/// `nav.reachability`：同连通块即可达，断连时给「断在哪儿」。
pub(super) fn nav_reachability(
    api: &mut LocalEngineApi<'_>,
    q: NavReachQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.from, "from")?;
    check_finite(&q.to, "to")?;
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;

    let from = Vec3::from_array(q.from);
    let to = Vec3::from_array(q.to);
    let (reachable, reason) = nav.check_reach(from, to);
    // 断点只在不可达时给：可达却给个点，等于告诉调用方「这里有隐患」。
    let disconnect = if reachable {
        None
    } else {
        nav.disconnect_point(from, to).map(|p| p.to_array())
    };
    Ok(json!({
        "reachable": reachable,
        "reason": reason,
        "fromRegion": nav.region_of(from),
        "toRegion": nav.region_of(to),
        "disconnectPoint": disconnect,
    }))
}
