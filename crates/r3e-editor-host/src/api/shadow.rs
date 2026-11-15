//! 方向光阴影设置（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn scene_shadow(
    api: &mut LocalEngineApi<'_>,
    patch: Option<ShadowPatch>,
) -> Result<Value, RpcFailure> {
    let mut scene = api.scene.borrow_mut();
    let Some(patch) = patch else {
        // 只读：直接回当前生效值，不做任何收敛（它本来就已经是收敛过的）
        return Ok(shadow_value(&scene.light.shadow));
    };
    let mut shadow = scene.light.shadow;
    if let Some(v) = patch.enabled {
        shadow.enabled = v;
    }
    if let Some(v) = patch.size {
        shadow.size = v;
    }
    if let Some(v) = patch.strength {
        shadow.strength = v;
    }
    if let Some(v) = patch.ortho_size {
        shadow.ortho_size = v;
    }
    if let Some(v) = patch.bias {
        shadow.bias = v;
    }
    if let Some(v) = patch.cascades {
        shadow.cascade.count = v;
    }
    if let Some(v) = patch.blend {
        shadow.cascade.blend = v;
    }
    if let Some(v) = patch.distance {
        shadow.cascade.max_distance = v;
    }
    // 一次性收敛：级数越界 / 混合比越界 / 尺寸非 2 的幂都在这里归位，
    // 免得「编辑器改完看起来生效了，渲染器又按另一套规则夹一遍」。
    shadow = shadow.sanitized();
    scene.light.shadow = shadow;
    Ok(shadow_value(&shadow))
}
