//! 环境探针与 IBL 补丁（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

/// 环境探针补丁：在现有环境上叠加（未启用时从缺省起步），再交给 `EnvironmentSettings`
/// 统一收敛。`enabled = false` 清空环境、退回 `light.ambient` 常数项。
pub(super) fn scene_set_environment(
    api: &mut LocalEngineApi<'_>,
    patch: EnvironmentPatch,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            if patch.enabled == Some(false) {
                scene.environment = None;
                return Ok(json!({ "environment": Value::Null, "enabled": false }));
            }
            let mut env = scene.environment.unwrap_or_default();
            if let Some(v) = patch.sky_zenith {
                env.sky_zenith = v;
            }
            if let Some(v) = patch.sky_horizon {
                env.sky_horizon = v;
            }
            if let Some(v) = patch.ground {
                env.ground = v;
            }
            if let Some(v) = patch.sun_dir {
                env.sun_dir = v;
            }
            if let Some(v) = patch.sun_color {
                env.sun_color = v;
            }
            if let Some(v) = patch.sun_intensity {
                env.sun_intensity = v;
            }
            if let Some(v) = patch.intensity {
                env.intensity = v;
            }
            let applied = env.sanitized();
            scene.environment = Some(applied);
            Ok(json!({
                "enabled": true,
                "environment": environment_value(applied),
            }))
        })
}

/// 反射探针补丁（扩展二十三）：在现有 `environment_map` 段上叠加（段缺失时从缺省起步），
/// 再交给 `EnvironmentMapSettings::sanitized` 统一收敛。
///
/// 三条与「解析 IBL」不同的语义，都在这里定死：
/// * `enabled = false` 只翻开关，路径保留——A/B 对比时不必重新填路径；
/// * **显式给了来源却收敛成空**（`equirect: ""` / `faces: []` / 六面不足 6 条）时整段丢弃，
///   等价于「没有探针」，比翻开关更彻底（面板上的清空按钮走这条）；
/// * `face_size` / `mip_levels` 用有符号整数收，负数在这里夹到区间下限，不静默改 0。
pub(super) fn scene_set_environment_map(
    api: &mut LocalEngineApi<'_>,
    patch: EnvironmentMapPatch,
) -> Result<Value, RpcFailure> {
    api.scene
        .borrow_mut()
        .write(|scene| -> Result<Value, RpcFailure> {
            let source_given = patch.equirect.is_some() || patch.faces.is_some();
            let mut map = scene.environment_map.clone().unwrap_or_default();
            if let Some(v) = patch.enabled {
                map.enabled = v;
            }
            if let Some(v) = patch.equirect {
                map.equirect = Some(v);
            }
            if let Some(v) = patch.faces {
                map.faces = Some(v);
            }
            if let Some(v) = patch.face_size {
                // 负数 / 0 一律落到 4 像素下限，再由 sanitized 向上取到 2 的幂
                map.face_size = v.max(4) as usize;
            }
            if let Some(v) = patch.mip_levels {
                map.mip_levels = v.clamp(1, r3e_core::envmap::MAX_SPECULAR_MIPS as i32) as u32;
            }
            if let Some(v) = patch.intensity {
                map.intensity = v;
            }
            let applied = map.sanitized();
            if source_given && applied.equirect.is_none() && applied.faces.is_none() {
                // 来源被清空：整段丢掉，渲染层立刻退回解析 IBL
                scene.environment_map = None;
                return Ok(json!({
                    "enabled": false,
                    "source": "none",
                    "environment_map": Value::Null,
                }));
            }
            let snapshot = environment_map_value(applied.clone());
            scene.environment_map = Some(applied.clone());
            Ok(json!({
                "enabled": applied.is_active(),
                "source": applied.source_label(),
                "environment_map": snapshot,
            }))
        })
}
