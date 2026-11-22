//! 跨块高度/法线采样。
//!
//! 单块的双线性插值在 [`r3e_core::TerrainPatch::sample_height`]（CD-3 从 nav 上移），
//! 这里只做**跨块裁决**：一个 XZ 点可能同时落在多块地表里（上层平台压住下层地面），
//! 按「取最高的那一层」裁决 —— 与 `r3e-nav` 烘焙期同一口径。

use glam::Vec3;
use r3e_core::TerrainPatch;

/// 点 `(x, z)` 处的地面高度：所有覆盖该点的块里取**最高**那层。
///
/// 点不落在任何块内（或地形表为空）返回 `None` —— 与「高度为 0 的平地」区分开，
/// 否则「这里没地形」和「这里是零点」会被混成同一件事。
pub fn sample_height_at(patches: &[TerrainPatch], x: f32, z: f32) -> Option<f32> {
    let mut highest: Option<f32> = None;
    for patch in patches {
        if let Some(y) = patch.sample_height(x, z) {
            highest = Some(highest.map_or(y, |cur: f32| cur.max(y)));
        }
    }
    highest
}

/// 中心差分法线（世界空间，单位化）。
///
/// 四个采样点里**只要中心点落在块内**就出结果：单侧越界时用中心高度顶替那一侧，
/// 这样块边缘不会因为差分越界而拿不到法线。中心点本身不在任何块内 → `None`。
///
/// `eps` 为差分步长（米），非法值（非有限、`<= 0`）返回 `None`。
pub fn sample_normal(patches: &[TerrainPatch], x: f32, z: f32, eps: f32) -> Option<[f32; 3]> {
    if !eps.is_finite() || eps <= 0.0 {
        return None;
    }
    let center = sample_height_at(patches, x, z)?;
    let east = sample_height_at(patches, x + eps, z).unwrap_or(center);
    let west = sample_height_at(patches, x - eps, z).unwrap_or(center);
    let south = sample_height_at(patches, x, z + eps).unwrap_or(center);
    let north = sample_height_at(patches, x, z - eps).unwrap_or(center);
    let dx = (east - west) / (2.0 * eps);
    let dz = (south - north) / (2.0 * eps);
    // 高度沿 +x 上升 → dx > 0 → 法线 x 分量为负（指向低处的外法线）。
    let n = Vec3::new(-dx, 1.0, -dz).normalize();
    Some([n.x, n.y, n.z])
}
