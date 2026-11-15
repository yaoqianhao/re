//! 3D 空间化音频宿主层（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

/// 空间化音频列表（§12 第十项）：声明态 + 上一帧结算结果一起给。
///
/// 这里**借用 `ScriptHost` 的 Audio 句柄**而不是另存一份：脚本、编辑器、帧驱动
/// 三方必须看到同一张声源表，否则面板上调完的参数会被脚本下一次 attach 覆盖掉。
pub(super) fn audio_spatial_list(
    api: &mut LocalEngineApi<'_>,
    limit: Option<usize>,
) -> Result<Value, RpcFailure> {
    let audio = api.scripts.audio_handle();
    let a = audio.borrow();
    let sources: Vec<Value> = a
        .spatial_keys()
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|key| spatial_source_value(&a, key))
        .collect();
    let listener = a.listener();
    Ok(json!({
        "enabled": a.spatial_enabled(),
        "count": a.spatial_count(),
        "limit": MAX_SPATIAL_SOURCES,
        "listener": {
            "position": listener.position,
            "forward": listener.forward,
        },
        "sources": sources,
    }))
}

/// 注册 / 局部更新一个声源：模型名与距离先解析完再落地，写错的名字不留半改状态。
pub(super) fn audio_spatial_set(
    api: &mut LocalEngineApi<'_>,
    patch: AudioSpatialPatch,
) -> Result<Value, RpcFailure> {
    if patch.key.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
    }
    let model = match patch.model.as_deref() {
        Some(name) => Some(DistanceModel::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!(
                "未知距离模型 `{name}`（可用：{}）",
                DistanceModel::all()
                    .iter()
                    .map(|m| m.name())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ))
        })?),
        None => None,
    };

    let audio = api.scripts.audio_handle();
    let mut a = audio.borrow_mut();
    let existed = a.spatial_settings(&patch.key);
    let inserted = existed.is_none();
    let mut settings = existed.unwrap_or_default();
    if let Some(v) = patch.min_distance {
        settings.min_distance = v;
    }
    if let Some(v) = patch.max_distance {
        settings.max_distance = v;
    }
    if let Some(v) = model {
        settings.model = v;
    }
    if let Some(v) = patch.doppler {
        settings.doppler_factor = v;
    }
    // `attach_spatial` 内部会走 `sanitized()`，并把已有声源就地更新（不清位置）。
    // 满载时它只拒绝「新增」，同 key 覆盖照常生效——因此失败一定是容量问题。
    if !a.attach_spatial(&patch.key, settings) {
        return Err(RpcFailure::invalid_params(format!(
            "空间化声源已达上限 {MAX_SPATIAL_SOURCES}，无法新增；请先删除或复用已有 key"
        )));
    }
    if let Some(pos) = patch.position {
        a.set_source_position(&patch.key, pos);
    }
    if let Some(node) = patch.node.as_deref() {
        // 空串 = 解绑（回到「只按手动 position 算」），与脚本侧 `audio_spatial_attach_node`
        // 的语义一致：传空节点名就是清绑定。
        let settings = a.spatial_settings(&patch.key).unwrap_or_default();
        if !a.attach_spatial_node(&patch.key, node, settings) {
            return Err(RpcFailure::invalid_params(format!(
                "声源 `{}` 注册失败，节点 `{node}` 未绑定",
                patch.key
            )));
        }
    }
    Ok(json!({
        "key": patch.key,
        "inserted": inserted,
        "count": a.spatial_count(),
        "source": spatial_source_value(&a, &patch.key),
    }))
}

pub(super) fn audio_spatial_remove(
    api: &mut LocalEngineApi<'_>,
    key: &str,
) -> Result<Value, RpcFailure> {
    let audio = api.scripts.audio_handle();
    let mut a = audio.borrow_mut();
    let removed = a.detach_spatial(key);
    Ok(json!({ "key": key, "removed": removed, "count": a.spatial_count() }))
}

pub(super) fn audio_spatial_clear(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let audio = api.scripts.audio_handle();
    let mut a = audio.borrow_mut();
    let removed = a.spatial_count();
    a.clear_spatial();
    Ok(json!({ "removed": removed, "count": 0 }))
}

pub(super) fn audio_spatial_listener(
    api: &mut LocalEngineApi<'_>,
    patch: Option<ListenerPatch>,
) -> Result<Value, RpcFailure> {
    let audio = api.scripts.audio_handle();
    let mut a = audio.borrow_mut();
    if let Some(patch) = patch {
        let mut pose = a.listener();
        if let Some(pos) = patch.position {
            pose.position = pos;
        }
        if let Some(forward) = patch.forward {
            // 零向量是无效朝向（归一化会炸成 NaN），当作没给
            if !forward.iter().all(|v| *v == 0.0) {
                pose.forward = forward;
            }
        }
        a.set_listener(pose);
        // 立刻结算一次：编辑器改完听者想马上看到增益变化，不该等下一帧
        a.tick_spatial(0.0);
    }
    let listener = a.listener();
    Ok(json!({
        "position": listener.position,
        "forward": listener.forward,
        "count": a.spatial_count(),
    }))
}

pub(super) fn audio_spatial_set_enabled(
    api: &mut LocalEngineApi<'_>,
    enabled: Option<bool>,
) -> Result<Value, RpcFailure> {
    let audio = api.scripts.audio_handle();
    let mut a = audio.borrow_mut();
    if let Some(on) = enabled {
        a.set_spatial_enabled(on);
        a.tick_spatial(0.0);
    }
    Ok(json!({
        "enabled": a.spatial_enabled(),
        "count": a.spatial_count(),
    }))
}
