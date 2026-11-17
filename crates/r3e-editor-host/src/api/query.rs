//! `query.*`（M1-S2-B）：语义查询面的宿主实现。
//!
//! 三条方法**都只读**：`api.scene.borrow()` 拿不可变视图，再调 `Scene::query` /
//! `Scene::query_stats` / `Scene::describe`。全程不碰 `Document::write`、不推版本，
//! 因此查询前后 `StateHash` 逐位相等（`api::tests::query` 有一遍守门）。
//!
//! 协议层的 [`QuerySpec`] 只做形状收敛，名字的裁决在这儿：类别名走
//! `NodeKind::parse`、组件位名走 `Component::parse`，拼错就回 `INVALID_PARAMS`
//! 并列出合法取值——不能让 AI 把「名字打错了」误读成「场景里没有」。

use super::support::*;
use super::*;
use r3e_core::{Component, NodeKind, NodeView, Query, DEFAULT_LIMIT};

/// `query.find`：按类别 / 标签 / 区域 / 组件位过滤，返回五字段投影。
///
/// 为了能如实回报 `truncated`，向 `Scene::query` 要的是 `limit + 1` 条：
/// 多出来的那一条不返回，只用来判断「后面还有」。
pub(super) fn query_find(
    api: &mut LocalEngineApi<'_>,
    spec: QuerySpec,
) -> Result<Value, RpcFailure> {
    let limit = spec.limit.unwrap_or(DEFAULT_LIMIT);
    let query = build_query(&spec, limit)?;

    let doc = api.scene.borrow();
    let scene: &Scene = &doc;
    let mut nodes = scene.query(&query);

    let truncated = limit > 0 && nodes.len() > limit;
    if truncated {
        nodes.truncate(limit);
    }

    Ok(json!({
        "count": nodes.len(),
        "limit": limit,
        "truncated": truncated,
        "unfiltered": spec.is_unfiltered(),
        "nodes": nodes.iter().map(node_view_value).collect::<Vec<Value>>(),
    }))
}

/// `query.describe`：单节点的投影 + 命中的组件位与类别。
///
/// 节点不存在回 `NOT_FOUND` 而不是空对象：对 AI 来说「没这个物件」和「有但什么都没有」
/// 是两条完全不同的下一步。
pub(super) fn query_describe(
    api: &mut LocalEngineApi<'_>,
    node: &str,
) -> Result<Value, RpcFailure> {
    let doc = api.scene.borrow();
    let scene: &Scene = &doc;
    let id = find_node_id(scene, node)?;
    let detail = scene
        .describe(id)
        .ok_or_else(|| RpcFailure::not_found(format!("物件 `{node}` 不存在")))?;

    Ok(json!({
        "node": node_view_value(&detail.view),
        "components": detail.components.iter().map(|c| c.as_str()).collect::<Vec<&str>>(),
        "kinds": detail.kinds.iter().map(|k| k.as_str()).collect::<Vec<&str>>(),
    }))
}

/// `query.stats`：全场景普查，用来「先看全貌再决定查什么」。
pub(super) fn query_stats(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let doc = api.scene.borrow();
    let scene: &Scene = &doc;
    let stats = scene.query_stats();

    // `by_kind` 的键是枚举，回包要字符串；不命中的类别 core 已剔除，这里原样透传。
    let by_kind: std::collections::BTreeMap<&str, usize> = stats
        .by_kind
        .iter()
        .map(|(kind, count)| (kind.as_str(), *count))
        .collect();

    Ok(json!({
        "nodes": stats.nodes,
        "visible": stats.visible,
        "interactable": stats.interactable,
        "tagged": stats.tagged,
        "byKind": by_kind,
        "tags": stats.tags,
    }))
}

/// 把协议 DTO 翻成 core 的 [`Query`]：名字裁决 + 区域归一化。
fn build_query(spec: &QuerySpec, limit: usize) -> Result<Query, RpcFailure> {
    let kind = match spec.kind.as_deref() {
        Some(raw) => NodeKind::parse(raw).ok_or_else(|| {
            RpcFailure::invalid_params(format!(
                "未知物件类别 `{raw}`（可用：any / prop / interactable / animated / \
                 skinned / morphing / character / physical）"
            ))
        })?,
        None => NodeKind::Any,
    };

    let mut query = Query::new(kind);
    query.all_tags = spec.all_tags.clone();
    query.any_tags = spec.any_tags.clone();
    query.region = spec.region.map(|r| {
        // 对角点顺序不限：`Aabb::new` 内部按分量取 min / max。
        Aabb::new(Vec3::new(r[0], r[1], r[2]), Vec3::new(r[3], r[4], r[5]))
    });
    query.require = components(&spec.require, "require")?;
    query.exclude = components(&spec.exclude, "exclude")?;
    // 多要一条用来判 `truncated`；`0` 仍是不限。
    query.limit = if limit == 0 { 0 } else { limit + 1 };
    Ok(query)
}

/// 组件位名 → 枚举；拼错回 `INVALID_PARAMS` 并列出合法取值。
fn components(names: &[String], field: &str) -> Result<Vec<Component>, RpcFailure> {
    names
        .iter()
        .map(|raw| {
            Component::parse(raw).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "`{field}` 里有未知组件位 `{raw}`（可用：material / physics / interact / \
                     animated / skinned / morphing / parent / child / tagged）"
                ))
            })
        })
        .collect()
}

/// 五字段投影的 JSON 形态（`query.find` 的数组元素与 `query.describe` 的 `node` 共用，
/// 保证两处同一物件长得一模一样）。
fn node_view_value(view: &NodeView) -> Value {
    json!({
        "id": view.id,
        "name": view.name,
        "tags": view.tags,
        "position": view.position,
        "interactRadius": view.interact_radius,
    })
}
