//! 端口层的地形合成（M2 · C04 / D13）：**转发到门面**。
//!
//! 地形是**独立通道**：不进刚体表、不占 `ColliderShape`、不占能力位。所以
//! 「射线打到哪」「胶囊跟谁重叠」「脚下方块地多高」这些问题，后端只能答一半，
//! 另一半由地形解析查询回答，两边各出一份候选后择优。
//!
//! ## 合成为什么在门面而不在这里
//!
//! 早先择优写在 `port_terrain`，结果是同一个问题两种答案：拿 `&Physics` 直接调
//! `raycast` 打不到地面，拿端口调却打得到。`terrain_roam` 的手动跑图脚本
//! `roam.rhai` 正是踩在这上面 —— 脚本贴地的射线永远读不到地形，角色浮在原地。
//! 现在 `Physics::raycast` / `overlap_capsule` / `sweep_capsule` 三条门面查询
//! 自己合成（规则见 `lib.rs` 的 `nearer_hit` / `nearer_port`），端口只搬形状：
//! `CastRay` → `Ray`、`RayHit` → `PortHit`。
//!
//! ## 一套数值，两套后端
//!
//! 比较规则与 `terrain_query` 的解析算法都与后端无关：换后端只换热体表那一路，
//! 地形这一路仍走同一份代码。`tests/terrain_field.rs` 对可选后端循环断言，就是在守这条。

use glam::Vec3;

use crate::port::{Capsule, CastRay, PortHit};
use crate::{Physics, Ray};

/// 门面射线结果 → 端口命中（地形用的也是这一套字段口径）。
///
/// `RayHit` 没有 `started_inside` 字段，用 `distance <= 0` 反推：起点与目标
/// 重叠时返回的正是 0。这条规则必须与地形那一路一致，否则上层拿到的
/// `started_inside` 会随「撞的是刚体还是地」漂移。
fn port_hit(ray: &CastRay, hit: crate::RayHit) -> PortHit {
    PortHit {
        body: hit.handle.0,
        node: hit.node,
        distance: hit.distance,
        point: ray.point_at(hit.distance),
        normal: hit.normal,
        started_inside: hit.distance <= 0.0,
    }
}

/// 射线：门面已经做完「刚体 vs 地形」的择优，这里只换类型。
pub(crate) fn cast_ray(physics: &Physics, ray: &CastRay) -> Option<PortHit> {
    if ray.is_degenerate() || !physics.enabled() {
        return None;
    }
    physics
        .raycast(Ray::new(ray.origin, ray.dir), ray.max_distance)
        .map(|hit| port_hit(ray, hit))
}

/// 胶囊重叠：门面结果里刚体在前、地形在后（顺序由门面保证）。
pub(crate) fn capsule_overlap(physics: &Physics, capsule: &Capsule) -> Vec<PortHit> {
    physics.overlap_capsule(*capsule)
}

/// 胶囊扫掠：门面已择优，这里只转发。
pub(crate) fn capsule_sweep(
    physics: &Physics,
    capsule: &Capsule,
    dir: Vec3,
    max_distance: f32,
) -> Option<PortHit> {
    physics.sweep_capsule(*capsule, dir, max_distance)
}
