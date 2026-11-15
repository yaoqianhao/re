//! 动画层列举与补丁（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

/// 列出混合树层（扩展十八 A）。`instance` 缺省列出全部动画实例的层。
///
/// 显式点名却找不到实例时报参数错误：编辑器把实例名拼错时，回空列表会让人以为
/// 「这个实例没有层」，比直接说「没有这个实例」难排查得多。
pub(super) fn scene_anim_layers(
    api: &mut LocalEngineApi<'_>,
    instance: Option<&str>,
    limit: Option<usize>,
) -> Result<Value, RpcFailure> {
    let scene = api.scene.borrow();
    let mut layers = Vec::new();
    let (mut total, mut instances) = (0usize, 0usize);
    match instance {
        Some(name) => {
            let inst = scene.anim_instance(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!("没有名为 `{name}` 的动画实例"))
            })?;
            instances = 1;
            total = inst.layers.len();
            for layer in inst.layers.iter().take(limit.unwrap_or(usize::MAX)) {
                layers.push(anim_layer_value(&inst.name, layer));
            }
        }
        None => {
            for inst in &scene.animations {
                instances += 1;
                total += inst.layers.len();
                for layer in &inst.layers {
                    layers.push(anim_layer_value(&inst.name, layer));
                }
            }
            layers.truncate(limit.unwrap_or(usize::MAX));
        }
    }
    Ok(json!({
        "count": total,
        "instances": instances,
        "limit": MAX_ANIM_LAYERS,
        "layers": layers,
    }))
}

/// 混合树层补丁（扩展十八 A）：与 `light_set` 不同，层**不能**凭空创建——
/// 层必须绑定片段，凭空建层等于建一个播不了任何动作的空层，因此层不存在一律报错。
/// 所有字符串先解析完再落地，写错的模式 / 片段不会留下半个被改过的层。
pub(super) fn scene_anim_layer_set(
    api: &mut LocalEngineApi<'_>,
    patch: AnimLayerPatch,
) -> Result<Value, RpcFailure> {
    let mode = match patch.mode.as_deref() {
        Some(name) => Some(LayerMode::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!(
                "未知叠加模式 `{name}`（可选：override / additive）"
            ))
        })?),
        None => None,
    };
    let mask = patch.mask.as_deref().map(bone_mask_of);
    let sync = match patch.sync.as_deref() {
        Some(name) => Some(LayerSync::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!("未知采样策略 `{name}`（可选：off / phase）"))
        })?),
        None => None,
    };
    let mut scene = api.scene.borrow_mut();
    if scene.anim_instance(&patch.instance).is_none() {
        return Err(RpcFailure::invalid_params(format!(
            "没有名为 `{}` 的动画实例",
            patch.instance
        )));
    }
    if let Some(clip) = patch.clip.as_deref() {
        if !scene.set_anim_layer_clip(&patch.instance, &patch.layer, clip) {
            return Err(RpcFailure::invalid_params(format!(
                "实例 `{}` 上没有层 `{}`，或片段 `{clip}` 解析不到",
                patch.instance, patch.layer
            )));
        }
    }
    if let Some(v) = patch.weight {
        if !scene.set_anim_layer_weight(&patch.instance, &patch.layer, v) {
            return Err(layer_missing(&patch.instance, &patch.layer));
        }
    }
    if let Some(v) = mode {
        if !scene.set_anim_layer_mode(&patch.instance, &patch.layer, v) {
            return Err(layer_missing(&patch.instance, &patch.layer));
        }
    }
    if let Some(v) = mask {
        if !scene.set_anim_layer_mask(&patch.instance, &patch.layer, v) {
            return Err(layer_missing(&patch.instance, &patch.layer));
        }
    }
    if let Some(v) = patch.speed {
        if !scene.set_anim_layer_speed(&patch.instance, &patch.layer, v) {
            return Err(layer_missing(&patch.instance, &patch.layer));
        }
    }
    if let Some(v) = sync {
        if !scene.set_anim_layer_sync_strategy(&patch.instance, &patch.layer, v) {
            return Err(layer_missing(&patch.instance, &patch.layer));
        }
    }
    // 扩展四十七：偏移要在 `clip` 之后应用——换片段会把层内收敛成单片段、偏移归零。
    if let Some(offset) = patch.clip_offset {
        let clip = patch.clip.as_deref().unwrap_or_default();
        if !scene.set_anim_layer_clip_phase_offset(&patch.instance, &patch.layer, clip, offset) {
            return Err(RpcFailure::invalid_params(format!(
                "实例 `{}` 的层 `{}` 上没有片段 `{clip}`（`clip_offset` 需配 `clip` 或作用于第一路）",
                patch.instance, patch.layer
            )));
        }
    }
    // 跨层相位源：核心层已在 setter 里挡掉源层不存在 / 自引用 / 成环，这里只透传。
    if let Some(source) = patch.sync_source.as_deref() {
        let source = if source.trim().is_empty() {
            None
        } else {
            Some(source)
        };
        if !scene.set_anim_layer_sync_source(&patch.instance, &patch.layer, source) {
            return Err(RpcFailure::invalid_params(format!(
                "实例 `{}` 的层 `{}` 无法跟随 `{}`（源层不存在、指向自己或成环）",
                patch.instance,
                patch.layer,
                source.unwrap_or("")
            )));
        }
    }
    // 扩展四十八：片段自带速率/循环，两种参数一起下发时只调一次 setter。
    if patch.clip_speed.is_some() || patch.clip_looping.is_some() {
        let clip = patch.clip.as_deref().unwrap_or_default();
        let current = scene.anim_layer_clip_playback(&patch.instance, &patch.layer, clip);
        // 只给循环标志时沿用当前速率；没设过就按原速 1.0，避免"只想停住"被顺手改成别的速度。
        let speed = patch.clip_speed.or(current.map(|(s, _)| s)).unwrap_or(1.0);
        // 只给速率时沿用当前循环标志；没设过默认循环。
        let looping = patch
            .clip_looping
            .or(current.map(|(_, l)| l))
            .unwrap_or(true);
        if !scene.set_anim_layer_clip_playback(&patch.instance, &patch.layer, clip, speed, looping)
        {
            return Err(RpcFailure::invalid_params(format!(
                "实例 `{}` 的层 `{}` 上没有片段 `{clip}`（`clip_speed` / `clip_looping` 需配 `clip` 或作用于第一路）",
                patch.instance, patch.layer
            )));
        }
    }
    let layer = scene
        .anim_layer(&patch.instance, &patch.layer)
        .ok_or_else(|| layer_missing(&patch.instance, &patch.layer))?;
    Ok(json!({
        "layer": anim_layer_value(&patch.instance, layer),
    }))
}
