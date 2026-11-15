//! 后处理与 SSAO 补丁（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

/// 后处理补丁（扩展十三）：在现有设置上叠加（未开启时从缺省起步），
/// 再交给 `PostFxSettings::sanitized` 统一收敛。
/// `enabled = false` 清空整段、回到 composite 直通。
pub(super) fn scene_set_postfx(
    api: &mut LocalEngineApi<'_>,
    patch: PostFxPatch,
) -> Result<Value, RpcFailure> {
    let mut scene = api.scene.borrow_mut();
    if patch.enabled == Some(false) {
        scene.postfx = None;
        return Ok(json!({ "postfx": Value::Null, "enabled": false }));
    }
    let mut fx = scene.postfx.unwrap_or_default();
    if let Some(v) = patch.exposure {
        fx.exposure = v;
    }
    // 名字的合法性在这里判定：认不出来就报参数错误，不做静默回落。
    if let Some(name) = patch.tonemap.as_deref() {
        fx.tonemap = TonemapMode::from_name(name).ok_or_else(|| {
            RpcFailure::invalid_params(format!(
                "未知色调映射 `{name}`（可选：none / reinhard / aces）"
            ))
        })?;
    }
    if let Some(v) = patch.bloom_enabled {
        fx.bloom.enabled = v;
    }
    if let Some(v) = patch.bloom_threshold {
        fx.bloom.threshold = v;
    }
    if let Some(v) = patch.bloom_knee {
        fx.bloom.knee = v;
    }
    if let Some(v) = patch.bloom_intensity {
        fx.bloom.intensity = v;
    }
    if let Some(v) = patch.fxaa {
        fx.fxaa = v;
    }
    let applied = fx.sanitized();
    scene.postfx = Some(applied);
    Ok(json!({
        "enabled": true,
        "postfx": postfx_value(applied),
    }))
}

/// 遮蔽补丁（扩展十七）：在现有后处理段的 `ssao` 上叠加（段缺失时从缺省起步），
/// 再交给 `SsaoSettings::sanitized` 统一收敛。
/// 与 `scene_set_postfx` 的区别：`enabled = false` 只关遮蔽这一条支线，
/// 曝光 / 泛光 / FXAA 全部保留——遮挡本来就该能单独开关。
pub(super) fn scene_set_ssao(
    api: &mut LocalEngineApi<'_>,
    patch: SsaoPatch,
) -> Result<Value, RpcFailure> {
    let mut scene = api.scene.borrow_mut();
    let mut fx = scene.postfx.unwrap_or_default();
    if let Some(v) = patch.enabled {
        fx.ssao.enabled = v;
    }
    if let Some(v) = patch.radius {
        fx.ssao.radius = v;
    }
    if let Some(v) = patch.bias {
        fx.ssao.bias = v;
    }
    if let Some(v) = patch.intensity {
        fx.ssao.intensity = v;
    }
    if let Some(v) = patch.samples {
        // 越界直接夹：样本数区间是引擎的硬约束，编辑器拖滑块越界是常态，
        // 报参数错误只会让前端反复弹错。
        fx.ssao.samples = v.clamp(i32::from(MIN_SSAO_SAMPLES), i32::from(MAX_SSAO_SAMPLES)) as u8;
    }
    let applied = fx.sanitized();
    scene.postfx = Some(applied);
    Ok(json!({
        "enabled": applied.ssao.enabled,
        "neutral": applied.ssao.is_neutral(),
        "ssao": ssao_value(applied.ssao),
        "postfx": postfx_value(applied),
    }))
}
