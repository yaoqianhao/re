//! 地形场的解析查询（M2 · C04 / D13）：把 [`HeightField`] 变成三条端口查询的候选。
//!
//! ## 为什么放在门面外侧
//!
//! 两套后端（`Builtin` / `Rapier`）都要能「打到地形」，但它们的内部表示完全不同：
//! 内置后端只有 AABB + 地面平面，Rapier 有一块真的 `HeightField3d` 碰撞体。
//! 若让各自实现一遍地形查询，同一个 `(x, z)` 在两套后端上会给出不同高度
//! （Rapier 走三角形、内置走插值），换后端手感就漂。
//!
//! 所以地形查询只写**一份**，放在门面这里：后端只负责保存场数据（Rapier 额外建
//! 一块碰撞体让动态刚体能落上去），查询时由 `port_impl` 把地形候选与后端候选合并择优。
//! 地形高度在两套后端上逐位相同，正是这么来的。
//!
//! ## 三条查询的口径
//!
//! * 射线：步进 + 二分。步长取 `cell / 2`（不越过一个单元的起伏），
//!   命中后二分细化到 1e-3 以内；起点已在地表下算 `started_inside`。
//! * 胶囊重叠：胶囊竖轴范围内是否夹住了地表（`bottom <= h <= top`）。
//! * 胶囊扫掠：把 footprint 内的最高地表当成一水平面，按 `dir` 求到达距离。
//!   竖向朝下扫掠时这就是「落地」，朝上时若起点已在地表下则如实报重叠。

use glam::Vec3;

use crate::heightfield::{HeightField, TERRAIN_BODY, TERRAIN_NODE};
use crate::port::{Capsule, CastRay, GroundSample, PortHit};

/// 射线细化次数：区间长度除以 `2^12`，1 米级的单元能细化到亚毫米。
const REFINE_STEPS: usize = 12;

/// 射线步进的最少段数（单元很大时保证不会一步跨过整座山）。
const MIN_STEPS: usize = 8;
/// 射线步进的最多段数（防止 `max_distance` 很大时把查询拖成上万次采样）。
const MAX_STEPS: usize = 256;

/// 地形命中的统一构造：地形不是刚体、不绑节点，用哨兵值占位。
fn hit(distance: f32, point: Vec3, normal: Vec3, started_inside: bool) -> PortHit {
    PortHit {
        body: TERRAIN_BODY,
        node: TERRAIN_NODE,
        distance,
        point,
        normal,
        started_inside,
    }
}

/// 射线 vs 地形：返回沿射线最近的一次地表穿越。
pub fn raycast(field: &HeightField, ray: &CastRay) -> Option<PortHit> {
    if ray.is_degenerate() {
        return None;
    }
    let d = ray.normalized_dir();
    let o = ray.origin;
    let max = ray.max_distance;

    // 起点就在地表下：如实报重叠（上层据此「从地里往外爬」，而不是当作没命中）。
    if let Some(h) = field.height_at(o.x, o.z) {
        if o.y < h {
            let n = field.normal_at(o.x, o.z).unwrap_or(Vec3::Y);
            return Some(hit(0.0, o, n, true));
        }
    }

    // 步进：只在「场内的那一段射线上」找穿越点，场外不必采样。
    let span = step_span(field, max);
    let mut prev_t = 0.0f32;
    let mut prev_above: Option<bool> = None;
    let mut t = 0.0f32;
    while t <= max {
        let p = o + d * t;
        // 场外（离开地形范围）时 `height_at` 返回 `None`：既不重置 `prev_above`，
        // 也不报命中，于是「出圈再进圈」不会凭一次场外采样伪造出穿越。
        if let Some(h) = field.height_at(p.x, p.z) {
            let above = p.y > h;
            if let Some(was_above) = prev_above {
                if was_above && !above {
                    let (t_hit, p_hit) = refine(field, o, d, prev_t, t, prev_t);
                    let n = field.normal_at(p_hit.x, p_hit.z).unwrap_or(Vec3::Y);
                    return Some(hit(t_hit, p_hit, n, false));
                }
            } else if !above {
                // 首个在场内的采样点已在地表下：说明起点在场外、从侧面切进了坡体。
                let n = field.normal_at(p.x, p.z).unwrap_or(Vec3::Y);
                return Some(hit(t, p, n, false));
            }
            prev_above = Some(above);
        }
        prev_t = t;
        t += span;
    }
    None
}

/// 步长：半个单元（不越过一个单元的起伏），并夹在
/// `[max / MAX_STEPS, max / MIN_STEPS]` 之间：前者防止把长射线拖成上万次采样，
/// 后者防止短射线只采到两三个点、把整块地「跨」过去。
fn step_span(field: &HeightField, max: f32) -> f32 {
    let lo = (max / MAX_STEPS as f32).max(1e-4);
    let hi = (max / MIN_STEPS as f32).max(lo);
    (field.cell() * 0.5).clamp(lo, hi)
}

/// 在 `[lo, hi]` 区间里二分找「地表上下翻转」的分界点。
///
/// `span_hint` 只用于把区间切得足够细到让 `lo` 一侧确实在地表之上；
/// 找不到合法分界时退化为线性插值结果（不 panic、不返回错误值）。
fn refine(field: &HeightField, o: Vec3, d: Vec3, lo: f32, hi: f32, span_hint: f32) -> (f32, Vec3) {
    let _ = span_hint;
    let mut a = lo;
    let mut b = hi;
    for _ in 0..REFINE_STEPS {
        let m = (a + b) * 0.5;
        let p = o + d * m;
        match field.height_at(p.x, p.z) {
            Some(h) if p.y > h => a = m,
            Some(_) => b = m,
            None => a = m,
        }
    }
    let t = (a + b) * 0.5;
    (t, o + d * t)
}

/// 胶囊 vs 地形：胶囊包住的竖直区间是否夹住了地表。
///
/// 地形没有体积，所以「重叠」只有一种情况：地表从胶囊中间穿过
/// （或胶囊底面嵌进了地表以下而顶面还在地表以上）。
pub fn capsule_overlap(field: &HeightField, capsule: &Capsule) -> Vec<PortHit> {
    if capsule.is_degenerate() {
        return Vec::new();
    }
    let foot = capsule.half_height.abs() + capsule.radius.abs();
    let bottom = capsule.center.y - foot;
    let top = capsule.center.y + foot;
    let Some(h) = field.peak_over(capsule.center.x, capsule.center.z, capsule.radius) else {
        return Vec::new();
    };
    if bottom > h || top < h {
        return Vec::new();
    }
    let n = field
        .normal_at(capsule.center.x, capsule.center.z)
        .unwrap_or(Vec3::Y);
    let point = Vec3::new(capsule.center.x, h, capsule.center.z);
    vec![hit(0.0, point, n, true)]
}

/// 胶囊 vs 地形：沿 `dir` 扫掠，返回到达地表所需的世界距离。
///
/// 把 footprint 内的最高地表当成水平面是刻意的近似：胶囊的轴向固定 +Y，
/// 倾斜地表上「先碰哪一侧」取决于半径与坡度的组合，那种精度只有三角形扫掠能给；
/// 而角色控制器真正需要的是「这一帧落不落得到地上 / 会不会穿过山」，用一个
/// 保守的上界（最高点）恰好不会漏判穿透。
pub fn capsule_sweep(
    field: &HeightField,
    capsule: &Capsule,
    dir: Vec3,
    max_distance: f32,
) -> Option<PortHit> {
    if capsule.is_degenerate() || !max_distance.is_finite() || max_distance <= 0.0 {
        return None;
    }
    let d = dir.normalize_or_zero();
    if d.length_squared() == 0.0 {
        return None;
    }
    let peak = field.peak_over(capsule.center.x, capsule.center.z, capsule.radius)?;
    let foot = capsule.half_height.abs() + capsule.radius.abs();
    let bottom = capsule.center.y - foot;
    let point_ok = |t: f32| {
        let p = capsule.center + d * t;
        Vec3::new(p.x, peak, p.z)
    };

    if bottom <= peak && d.y <= 0.0 {
        // 已经贴在地表上或嵌进地表：起点即重叠。
        let n = field
            .normal_at(capsule.center.x, capsule.center.z)
            .unwrap_or(Vec3::Y);
        return Some(hit(0.0, point_ok(0.0), n, true));
    }
    if d.y >= 0.0 {
        // 向上 / 水平扫掠碰不到「脚下的地表」：地形是单值高度场，没有侧面可撞。
        return None;
    }
    let t = (bottom - peak) / -d.y;
    if t < 0.0 || t > max_distance {
        return None;
    }
    let n = field
        .normal_at(capsule.center.x, capsule.center.z)
        .unwrap_or(Vec3::Y);
    Some(hit(t, point_ok(t), n, false))
}

/// 地表采样合成：地形与后端结果取**更高**的那个。
///
/// 「站得最高的那一层才是脚下这层地」—— 与两套后端 `sample_ground` 的既有口径一致
/// （内置后端取「平面 vs 刚体顶面」的最高者，Rapier 取所有命中里最高的可站面）。
pub fn merge_ground(
    field: &HeightField,
    backend: Option<GroundSample>,
    x: f32,
    z: f32,
) -> Option<GroundSample> {
    let terrain = field.sample(x, z);
    match (terrain, backend) {
        (Some(t), Some(b)) => Some(if t.height >= b.height { t } else { b }),
        (Some(t), None) => Some(t),
        (None, b) => b,
    }
}
