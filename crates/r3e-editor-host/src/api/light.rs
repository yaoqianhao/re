//! 局部光源 RPC（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn light_list(
    api: &mut LocalEngineApi<'_>,
    limit: Option<usize>,
) -> Result<Value, RpcFailure> {
    let scene = api.scene.borrow();
    let lights: Vec<Value> = scene
        .lights
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(local_light_value)
        .collect();
    Ok(json!({
        "count": scene.lights.len(),
        "limit": MAX_LOCAL_LIGHTS,
        "enabled": scene.lights_enabled,
        "lights": lights,
    }))
}

/// 局部光源补丁（扩展十五）：key 存在则叠加，不存在则从默认值起步新建，
/// 最后统一交给 `LocalLight::sanitized`——编辑器传 NaN / 负强度也污染不了场景。
pub(super) fn light_set(
    api: &mut LocalEngineApi<'_>,
    patch: LightPatch,
) -> Result<Value, RpcFailure> {
    if patch.key.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
    }
    // 名字先解析完再落地：写错的 kind 不该留下半盏被改的灯。
    let kind = match patch.kind.as_deref() {
        Some(name) => Some(LocalLightKind::parse(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!("未知光源类型 `{name}`（可用：point / spot）"))
        })?),
        None => None,
    };

    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let existing = scene.local_light(&patch.key).cloned();
            let inserted = existing.is_none();
            let mut light = existing.unwrap_or_else(|| LocalLight {
                key: patch.key.clone(),
                ..LocalLight::default()
            });
            if let Some(v) = kind {
                light.kind = v;
            }
            if let Some(v) = patch.pos {
                light.pos = v;
            }
            if let Some(v) = patch.dir {
                light.dir = v;
            }
            if let Some(v) = patch.color {
                light.color = v;
            }
            if let Some(v) = patch.intensity {
                light.intensity = v;
            }
            if let Some(v) = patch.range {
                light.range = v;
            }
            if let Some(v) = patch.inner_deg {
                light.inner_deg = v;
            }
            if let Some(v) = patch.outer_deg {
                light.outer_deg = v;
            }
            if let Some(v) = patch.cast_shadow {
                light.cast_shadow = v;
            }
            if let Some(v) = patch.shadow_bias {
                light.shadow_bias = v;
            }
            let applied = light.sanitized();
            // `upsert_local_light` 满载时只拒绝「新增」，同 key 覆盖照常生效——
            // 因此这里的失败一定是容量问题，报错文案直接点明上限。
            if !scene.upsert_local_light(applied) {
                return Err(RpcFailure::invalid_params(format!(
                    "局部光源已达上限 {MAX_LOCAL_LIGHTS}，无法新增；请先删除或复用已有 key"
                )));
            }
            let value = scene
                .local_light(&patch.key)
                .map(local_light_value)
                .unwrap_or(Value::Null);
            Ok(json!({
                "key": patch.key,
                "inserted": inserted,
                "count": scene.lights.len(),
                "light": value,
            }))
        })
}

pub(super) fn light_remove(api: &mut LocalEngineApi<'_>, key: &str) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let removed = scene.remove_local_light(key);
            Ok(json!({ "key": key, "removed": removed, "count": scene.lights.len() }))
        })
}

pub(super) fn light_clear(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let removed = scene.clear_local_lights();
            Ok(json!({ "removed": removed, "count": 0 }))
        })
}

pub(super) fn light_set_enabled(
    api: &mut LocalEngineApi<'_>,
    enabled: Option<bool>,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            if let Some(on) = enabled {
                scene.lights_enabled = on;
            }
            Ok(json!({
                "enabled": scene.lights_enabled,
                "count": scene.lights.len(),
            }))
        })
}
