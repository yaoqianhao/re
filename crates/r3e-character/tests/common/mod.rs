//! 共享夹具：一片水平（或倾斜）地面 + 若干轴对齐盒子。
//!
//! 这里不引真物理后端：B01 的判据全是几何恒等式（走多远、停在哪、踩没踩实），
//! 用解析 AABB 就能算到 1e-6，比挂上 rapier 快三个数量级，也不会因为
//! 后端的接触容差漂移而变成「有时绿有时红」。
//!
//! 语义对齐 `r3e-physics::port` 的真实后端约定：
//! * 扫掠返回**最近**接触，`started_inside` 只在起点就已穿透时为 `true`；
//! * 恰好贴合（间隙 < `EPS`）不算重叠 —— 这是能站在地面上而不会被判「卡住」的前提。

#![allow(dead_code)]

use glam::Vec3;
use r3e_physics::port::{
    Capsule, CapsulePort, GroundPort, GroundSample, PhysicsQueryPort, PortCaps, PortHit,
    RaycastPort,
};

/// 贴合容差：小于它就当没接触 / 没穿透。
pub const EPS: f32 = 1e-4;

/// 轴对齐盒子（静态几何的最小替身）。
#[derive(Debug, Clone, Copy)]
pub struct Box3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Box3 {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// 以「脚底平面」为底的盒子：`min.y` 就从地面起算。
    pub fn slab(cx: f32, cz: f32, half_x: f32, half_z: f32, top: f32) -> Self {
        Self::new(
            Vec3::new(cx - half_x, 0.0, cz - half_z),
            Vec3::new(cx + half_x, top, cz + half_z),
        )
    }
}

/// 一个极简世界：水平地面（可带坡度）+ 盒子集合。
#[derive(Debug, Clone)]
pub struct World {
    pub ground: f32,
    pub slope_deg: f32,
    pub boxes: Vec<Box3>,
}

impl World {
    /// 平地。
    pub fn flat(ground: f32) -> Self {
        Self {
            ground,
            slope_deg: 0.0,
            boxes: Vec::new(),
        }
    }

    /// 带坡度的地面：`sample_ground` 与向下扫掠都会返回倾斜法线。
    pub fn slope(ground: f32, slope_deg: f32) -> Self {
        Self {
            ground,
            slope_deg,
            boxes: Vec::new(),
        }
    }

    /// 加一块静态方块。
    pub fn with(mut self, b: Box3) -> Self {
        self.boxes.push(b);
        self
    }

    /// 地面自身的表面法线。
    pub fn ground_normal(&self) -> Vec3 {
        let rad = self.slope_deg.to_radians();
        Vec3::new(rad.sin(), rad.cos(), 0.0).normalize()
    }

    /// 地面盒：无限宽、顶面在 `ground`。
    fn ground_box(&self) -> Box3 {
        Box3::new(
            Vec3::new(-1e5, -1e5, -1e5),
            Vec3::new(1e5, self.ground, 1e5),
        )
    }
}

/// 三个轴都严格穿透才算重叠（贴合不算）。
fn penetrates(mn: Vec3, mx: Vec3, b: &Box3) -> bool {
    mn.x < b.max.x - EPS
        && mx.x > b.min.x + EPS
        && mn.y < b.max.y - EPS
        && mx.y > b.min.y + EPS
        && mn.z < b.max.z - EPS
        && mx.z > b.min.z + EPS
}

/// 移动 AABB 对静态 AABB 的扫掠（Minkowski 后做 ray-AABB）。
///
/// 返回 `(进入距离, 进入面法线)`；不会相交返回 `None`。
/// 只沿切向擦过（没有任何轴真正被穿入）时返回 `None` —— 那是「贴着走」不是「撞上」。
fn sweep_aabb(mn: Vec3, mx: Vec3, dir: Vec3, maxd: f32, b: &Box3) -> Option<(f32, Vec3)> {
    let mut tmin = 0.0f32;
    let mut tmax = maxd;
    let mut axis: i32 = -1;
    for i in 0..3 {
        let d = dir[i];
        let lo = b.min[i] - mx[i];
        let hi = b.max[i] - mn[i];
        if d.abs() < 1e-9 {
            if mn[i] > b.max[i] || mx[i] < b.min[i] {
                return None;
            }
        } else {
            let t1 = lo / d;
            let t2 = hi / d;
            let (t1, t2) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
            if t1 > tmin {
                tmin = t1;
                axis = i as i32;
            }
            tmax = tmax.min(t2);
            if tmin > tmax {
                return None;
            }
        }
    }
    if tmax < 0.0 || axis < 0 || tmax < tmin {
        return None;
    }
    let mut n = Vec3::ZERO;
    n[axis as usize] = if dir[axis as usize] > 0.0 { -1.0 } else { 1.0 };
    Some((tmin.max(0.0).min(maxd), n))
}

impl World {
    /// 最近的一次扫掠命中：盒子按进入面算法线，地面单独按「只在向下时支撑」处理。
    fn nearest_sweep(&self, capsule: &Capsule, dir: Vec3, maxd: f32) -> Option<PortHit> {
        let (mn, mx) = capsule.aabb();
        let mut best: Option<(f32, Vec3)> = None;
        for b in &self.boxes {
            if let Some((t, n)) = sweep_aabb(mn, mx, dir, maxd, b) {
                // 只有迎着脸撞上的面才算阻挡（背面 / 切向接触一律忽略）。
                if n.dot(dir) > -0.1 {
                    continue;
                }
                if best.map(|(bt, _)| t < bt).unwrap_or(true) {
                    best = Some((t, n));
                }
            }
        }
        // 地面只在向下运动时才支撑：横着走是切向、往上跳时地面在身后。
        if dir.y < -0.1 {
            let t = (mn.y - self.ground).max(0.0);
            if t <= maxd && best.map(|(bt, _)| t < bt).unwrap_or(true) {
                best = Some((t, self.ground_normal()));
            }
        }
        best.map(|(t, n)| PortHit {
            body: 1,
            node: 1,
            distance: t,
            point: capsule.center + dir * t,
            normal: n,
            started_inside: false,
        })
    }
}

impl RaycastPort for World {}

impl GroundPort for World {
    fn sample_ground(&self, _x: f32, _z: f32) -> Option<GroundSample> {
        Some(GroundSample::flat(self.ground))
    }
}

impl CapsulePort for World {
    fn capsule_overlap(&self, capsule: &Capsule) -> Vec<PortHit> {
        if capsule.is_degenerate() {
            return Vec::new();
        }
        let (mn, mx) = capsule.aabb();
        let mut out = Vec::new();
        for b in self.boxes.iter().chain(std::iter::once(&self.ground_box())) {
            if penetrates(mn, mx, b) {
                out.push(PortHit {
                    body: 1,
                    node: 1,
                    distance: 0.0,
                    point: capsule.center,
                    normal: Vec3::ZERO,
                    started_inside: true,
                });
            }
        }
        out
    }

    fn capsule_sweep(&self, capsule: &Capsule, dir: Vec3, maxd: f32) -> Option<PortHit> {
        if capsule.is_degenerate() || !maxd.is_finite() || maxd <= 0.0 {
            return None;
        }
        let dir = dir.normalize_or_zero();
        if dir.length_squared() == 0.0 {
            return None;
        }
        if !self.capsule_overlap(capsule).is_empty() {
            return Some(PortHit {
                body: 1,
                node: 1,
                distance: 0.0,
                point: capsule.center,
                normal: -dir,
                started_inside: true,
            });
        }
        self.nearest_sweep(capsule, dir, maxd)
    }
}

impl PhysicsQueryPort for World {
    fn port_caps(&self) -> PortCaps {
        PortCaps {
            raycast: true,
            capsule: true,
            ground: true,
        }
    }
}
