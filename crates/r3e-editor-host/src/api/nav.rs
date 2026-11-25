//! `nav.*` 十条的宿主实现（M1-S4-D 落三条，M2-D04 补齐七条）。
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
use r3e_runtime::nav_ops::{self as ops, NavOpError};

/// 运行时导航操作的错误 → 协议失败。
///
/// 两类必须分开：`Invalid` 是调用方参数问题（`INVALID_PARAMS`），`Nav` 是
/// 产物 / 数据层面不可用（`ENGINE`）。合成一个码之后，脚本会把「你给错了」
/// 和「这份网格坏了」当成同一件事，修复方向直接跑偏。
fn map_op(err: NavOpError) -> RpcFailure {
    match err {
        NavOpError::Invalid(detail) => RpcFailure::invalid_params(detail),
        NavOpError::Nav(detail) => RpcFailure::engine(detail),
    }
}

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
    let mut nav = Nav::bake_scene_with(scene, config)
        .map_err(|err| RpcFailure::invalid_params(format!("导航烘焙失败：{err}")))?;
    // 记下当次物理后端名（M2·D04）：校验层的 `nav.backend.mismatch` 拿它和
    // 「当前物理后端」对账，看这份网格是不是另一个后端烘出来的。贴名不改网格、
    // 不改产物哈希——D01 的「换后端不改哈希」判据不受影响。
    nav.attach_backend(api.physics.borrow().backend_name());
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

/// `nav.smooth`：折线几何剪枝（M2-D04）。
///
/// 只读槽位、不碰场景：平滑不产生新的导航数据，也不改路径的起点终点。
pub(super) fn nav_smooth(
    api: &mut LocalEngineApi<'_>,
    q: NavSmoothQuery,
) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let points: Vec<Vec3> = q.points.iter().map(|p| Vec3::from_array(*p)).collect();
    let out = ops::smooth(nav, &points).map_err(map_op)?;
    Ok(json!({
        "inputCount": out.input_count,
        "count": out.points.len(),
        "removed": out.input_count.saturating_sub(out.points.len()),
        "length": out.length,
        "points": out.points,
    }))
}

/// `nav.probe`：单点站不站得住（M2-D04）。
///
/// 不吃寻路：寻路会先做起点吸附，正好把「这个点根本不在地上」掩盖掉。
pub(super) fn nav_probe(
    api: &mut LocalEngineApi<'_>,
    q: NavPointQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.point, "point")?;
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let out = ops::probe(nav, Vec3::from_array(q.point)).map_err(map_op)?;
    Ok(json!({
        "point": out.point,
        "walkable": out.walkable,
        "region": out.region,
        "groundHeight": out.ground_height,
        "heightDelta": out.height_delta,
        "cellSize": out.cell_size,
    }))
}

/// `nav.nearest`：把任意点吸到最近的可走面上（M2-D04）。
///
/// 没找到时回 `found: false` + `point: null`，绝不回原点——原点是个合法坐标，
/// 用它表示「没找到」会埋一个假锚点。
pub(super) fn nav_nearest(
    api: &mut LocalEngineApi<'_>,
    q: NavNearestQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.point, "point")?;
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let out = ops::nearest(nav, Vec3::from_array(q.point), q.radius, q.region).map_err(map_op)?;
    Ok(json!({
        "found": out.found,
        "point": out.point,
        "distance": out.distance,
        "region": out.region,
        "radius": out.radius,
    }))
}

/// `nav.stats`：体检数字 + 产物指纹 + 逐格分类直方图（M2-D04）。
///
/// 直方图的四个下标与 `CellClass` 的字节编码同序：`flat` / `slope` / `step` /
/// `blocked`。按键给出而不是给数组，是因为调用方十有八九只关心其中一项，
/// 数组下标一旦和枚举顺序脱钩就会静默错位。
pub(super) fn nav_stats(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let out = ops::stats(nav);
    let histogram = out.class_histogram;
    Ok(json!({
        "dims": out.dims,
        "cellSize": out.cell_size,
        "cells": out.cells,
        "walkable": out.walkable,
        "walkableRatio": out.walkable_ratio,
        "regions": out.regions,
        "regionSizes": out.region_sizes,
        "classHistogram": {
            "flat": histogram[0],
            "slope": histogram[1],
            "step": histogram[2],
            "blocked": histogram[3],
        },
        "unresolved": out.unresolved,
        "productHash": out.product_hash,
        "cfgDigest": out.cfg_digest,
        "inputDigest": out.input_digest,
    }))
}

/// `nav.hash`：只回三级指纹 + 尺寸（M2-D04）。
///
/// 缓存命中判定走这条：便宜、且与 `nav.export` 回包的 `productHash` 同源。
pub(super) fn nav_hash(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let out = ops::hash(nav);
    Ok(json!({
        "productHash": out.product_hash,
        "cfgDigest": out.cfg_digest,
        "inputDigest": out.input_digest,
        "dims": out.dims,
        "cellSize": out.cell_size,
        "cells": out.cells,
    }))
}

/// `nav.export`：把当前网格写成 `.navmesh`（M2-D04）。
///
/// 路径没给就是参数错，没有缺省落盘位置。写盘不碰场景、不碰槽位，`state_hash`
/// 前后一致——但它**确实**产生磁盘副作用，所以只允许显式路径。
pub(super) fn nav_export(
    api: &mut LocalEngineApi<'_>,
    target: NavFileTarget,
) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let guard = slot.borrow();
    let nav = guard.as_ref().ok_or_else(|| not_baked("槽位里没有网格"))?;
    let out = ops::export(nav, &target.path).map_err(map_op)?;
    Ok(json!({
        "path": out.path,
        "bytes": out.bytes,
        "productHash": out.product_hash,
        "exported": true,
    }))
}

/// `nav.import`：读 `.navmesh` 换掉槽位里的网格（M2-D04）。
///
/// 三条纪律：
///
/// 1. **只读不烘**：用 `Nav::from_product` 恢复，不重跑烘焙、不重判可走性；
/// 2. **失败不换槽位**：先读到局部变量，解析成功才写，坏文件不会把上一次的
///    网格冲成「没烘过」；
/// 3. **不贴后端名**：导入的网格不来自任何后端，`nav.backend.mismatch`
///    因此不会对它误报（见 [`Nav::backend`]）。
pub(super) fn nav_import(
    api: &mut LocalEngineApi<'_>,
    target: NavFileTarget,
) -> Result<Value, RpcFailure> {
    let slot = nav_slot(api)?;
    let (nav, out) = ops::import(&target.path).map_err(map_op)?;
    let walkable = nav.report().walkable;
    *slot.borrow_mut() = Some(nav);
    Ok(json!({
        "path": out.path,
        "bytes": out.bytes,
        "productHash": out.product_hash,
        "dims": out.dims,
        "walkable": walkable,
        "baked": false,
    }))
}
