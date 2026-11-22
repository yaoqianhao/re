//! 端口层的地形合成（M2 · C04 / D13）。
//!
//! 地形是**独立通道**：不进刚体表、不占 `ColliderShape`、不占能力位。所以
//! 「射线打到哪」「胶囊跟谁重叠」「脚下方块地多高」这些问题，后端只能答一半，
//! 另一半由地形解析查询回答，两边各出一份候选后在这里择优。
//!
//! ## 为什么单独成文件而不塞进 `port_impl.rs`
//!
//! `port_impl.rs` 已经贴着 `lint-size` 的 800 行上限（这份文件本身也是因为它才分出来的）。
//! 合成逻辑要带比较规则和一份解释，塞回去只会把一条已经存在的债务线继续顶穿；
//! 分成「搬运」（`port_impl`）与「决策」（这里）两块之后，两边都还在阈值内，
//! 而且读端口契约的人不必同时读地形算法。
//!
//! ## 一套数值，两套后端
//!
//! 这里的比较规则与 `terrain_query` 的解析算法都与后端无关：换后端只换热体表那一路，
//! 地形这一路仍走同一份代码。`tests/terrain_field.rs` 对 `selectable()` 循环断言，
//! 就是在守这条。

use glam::Vec3;

use crate::port::{Capsule, CastRay, PortHit};
use crate::{terrain_query, Physics, Ray};

/// 地形候选与后端候选择优。
///
/// 顺序：**已重叠**（`started_inside`）优先于「远处才撞上」——起点就嵌在里面时
/// 距离恒为 0，但上层更关心「我卡住了」而不是「几米外有面墙」；
/// 其次比距离，1e-4 以内视为同一次接触；最后按 `node` 升序取小，
/// 让两次相同查询拿到稳定结果（否则合并顺序一变，上层看到的目标就跳）。
fn nearer(a: Option<PortHit>, b: Option<PortHit>) -> Option<PortHit> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if a.started_inside != b.started_inside {
                return Some(if a.started_inside { a } else { b });
            }
            if (a.distance - b.distance).abs() <= 1e-4 {
                return Some(if a.node <= b.node { a } else { b });
            }
            Some(if a.distance < b.distance { a } else { b })
        }
        (a, b) => a.or(b),
    }
}

/// 门面射线结果 → 端口命中（地形用的也是这一套字段口径）。
///
/// 后端射线结果没有 `started_inside` 字段，用 `distance <= 0` 反推：起点与目标
/// 重叠时后端返回的正是 0。这条规则必须与地形那一路一致，否则上层拿到的
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

/// 射线：刚体与地形各出一份候选，取更近的那个。
///
/// 地形不参与刚体表，但「射线打到哪」对上层只有一种答案，所以合成必须做在
/// 端口这一层，而不是让调用方自己再查一次地形。
pub(crate) fn cast_ray(physics: &Physics, ray: &CastRay) -> Option<PortHit> {
    if ray.is_degenerate() || !physics.enabled() {
        return None;
    }
    let solid = physics
        .raycast(Ray::new(ray.origin, ray.dir), ray.max_distance)
        .map(|hit| port_hit(ray, hit));
    let terrain = physics
        .heightfield()
        .and_then(|field| terrain_query::raycast(field, ray));
    nearer(solid, terrain)
}

/// 胶囊重叠：刚体命中在前，地形最多再贡献一条（地表从胶囊中间穿过）。
///
/// 端口契约不承诺顺序，但「刚体在前、地形在后」是稳定的：调用方按需要挑第一条
/// 时不会因为装了地形就把原来拿到的刚体换成地面。
pub(crate) fn capsule_overlap(physics: &Physics, capsule: &Capsule) -> Vec<PortHit> {
    let mut hits = physics.overlap_capsule(*capsule);
    if !physics.enabled() {
        return hits;
    }
    if let Some(field) = physics.heightfield() {
        hits.extend(terrain_query::capsule_overlap(field, capsule));
    }
    hits
}

/// 胶囊扫掠：刚体与地形各出一份候选，取更近的那个。
pub(crate) fn capsule_sweep(
    physics: &Physics,
    capsule: &Capsule,
    dir: Vec3,
    max_distance: f32,
) -> Option<PortHit> {
    if !physics.enabled() {
        return None;
    }
    let solid = physics.sweep_capsule(*capsule, dir, max_distance);
    let terrain = physics
        .heightfield()
        .and_then(|field| terrain_query::capsule_sweep(field, capsule, dir, max_distance));
    nearer(solid, terrain)
}
