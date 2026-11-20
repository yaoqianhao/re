//! 内置物理后端（`BuiltinBackend`）与空后端（`NullBackend`）。
//!
//! 两者都**零 rapier 依赖**，因此 `--no-default-features` 下仍可编译、可跑测试，
//! 这是「双后端」能成立的前提：默认构建走 `rapier_backend.rs`，关掉 feature 走本文件。
//!
//! 为什么从 `lib.rs` 搬出来：`lib.rs` 原本同时装着契约、门面、两套后端与 30 多条单测，
//! 一条文件里混了三种变化原因（契约改 / 门面改 / 后端改）。拆开后 `lib.rs` 只留
//! 「契约 + 门面」，后端各自一个文件，改后端不必再在两千行里翻找。
//!
//! 语义边界（与 `rapier_backend.rs` 必须一致，否则换后端脚本行为会跳变）：
//! * `NullBackend` 只记账不模拟：`step` 是空操作，物件完全由脚本驱动
//! * `BuiltinBackend` 是解析近似：AABB / 球 / 地面平面，无刚体间碰撞、无旋转动力学
//! * 两者共用 `cast_candidate_better` 的择优规则，「谁更近」的判定换后端不改
//!
//! 能力申报口径：内置后端只报它真能做到的位（`raycast` / `overlap` / `shape_cast`），
//! CCD 与关节一律报 `false`——宁可少报，也不让上层写出依赖不存在能力的代码。

use glam::Vec3;
use r3e_core::{NodeId, Scene};

use crate::port::{Capsule, GroundSample, PortHit};
use crate::{
    cast_candidate_better, BackendCaps, BodyDesc, BodyHandle, BodyKind, CastShape, ColliderShape,
    OverlapHit, PhysicsBackend, Ray, RayHit, ShapeCastHit,
};

// ---------------------------------------------------------------------------
// Null 后端：不做任何模拟，只记录句柄，保证脚本仍完全掌控物件。
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct NullBackend {
    next: u64,
    bodies: Vec<BodyHandle>,
}

impl NullBackend {
    pub fn new() -> Self {
        Self {
            next: 1,
            bodies: Vec::new(),
        }
    }
}

impl PhysicsBackend for NullBackend {
    fn name(&self) -> &'static str {
        "null"
    }

    fn create_body(&mut self, _node: NodeId, _desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;
        self.bodies.push(handle);
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        self.bodies.retain(|h| *h != handle);
    }

    /// 空后端不模拟，但它**确实**发出过句柄，句柄表是准的——
    /// 复用判定要如实回答，否则每次装配都重建一遍，报告里的数字就没意义了。
    fn has_body(&self, handle: BodyHandle) -> bool {
        self.bodies.contains(&handle)
    }

    fn apply_impulse(&mut self, _handle: BodyHandle, _impulse: Vec3) {}

    fn set_gravity(&mut self, _gravity: Vec3) {}

    fn step(&mut self, _dt: f32) {}

    fn sync_to_scene(&mut self, _scene: &mut Scene) {}

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.bodies.clone())
    }
}

// ---------------------------------------------------------------------------
// 内置最简物理后端
// ---------------------------------------------------------------------------

/// 单个刚体的运行期状态。位置以自身为准，每帧与场景节点双向对齐。
#[derive(Debug, Clone)]
struct RigidBody {
    handle: BodyHandle,
    node: NodeId,
    kind: BodyKind,
    shape: ColliderShape,
    half: Vec3,
    inv_mass: f32,
    velocity: Vec3,
    restitution: f32,
    friction: f32,
    position: Vec3,
    sleep_timer: f32,
    sleeping: bool,
}

impl RigidBody {
    /// 支撑半高：刚体中心到底面的距离。
    fn support(&self) -> f32 {
        let v = match self.shape {
            ColliderShape::Sphere => self.half.x,
            ColliderShape::Box | ColliderShape::Capsule | ColliderShape::Cylinder => self.half.y,
        };
        v.abs().max(0.001)
    }

    fn is_dynamic(&self) -> bool {
        matches!(self.kind, BodyKind::Dynamic)
    }
}

/// 内置最简物理：重力 + 地面平面碰撞 + 弹性 + 摩擦 + 休眠。
///
/// 明确的能力边界（刻意不做，保证 ~400 行可读）：
/// * 物件之间不碰撞，只与一张无限大的地面平面相交
/// * 不做旋转动力学，物体不会翻滚
/// * 无关节、无堆叠求解、无连续碰撞检测（CCD）
pub struct BuiltinBackend {
    next: u64,
    gravity: Vec3,
    ground: f32,
    bodies: Vec<RigidBody>,
}

impl BuiltinBackend {
    pub fn new(gravity: Vec3, ground: f32) -> Self {
        Self {
            next: 1,
            gravity,
            ground,
            bodies: Vec::new(),
        }
    }

    fn find_mut(&mut self, handle: BodyHandle) -> Option<&mut RigidBody> {
        self.bodies.iter_mut().find(|b| b.handle == handle)
    }

    fn substep(&mut self, dt: f32) {
        let gravity = self.gravity;
        let ground = self.ground;
        for rb in self.bodies.iter_mut() {
            if !rb.is_dynamic() || rb.sleeping {
                continue;
            }

            // 1) 半隐式欧拉积分
            rb.velocity += gravity * dt;
            rb.position += rb.velocity * dt;

            // 2) 与地面平面求解
            let floor = ground + rb.support();
            if rb.position.y <= floor {
                rb.position.y = floor;
                let impact = -rb.velocity.y;
                if rb.velocity.y < 0.0 {
                    if impact > 0.6 && rb.restitution > 0.01 {
                        // 弹性：足够快的撞击才反弹，避免落地时无休止抖动
                        rb.velocity.y = impact * rb.restitution;
                    } else {
                        rb.velocity.y = 0.0;
                    }
                }

                // 3) 地面摩擦：指数衰减，与帧率无关
                let k = (1.0 - rb.friction * 8.0 * dt).clamp(0.0, 1.0);
                rb.velocity.x *= k;
                rb.velocity.z *= k;

                // 4) 静止阈值 -> 休眠（省算力，也彻底消除数值抖动）
                if rb.velocity.length_squared() < 0.0025 {
                    rb.sleep_timer += dt;
                    if rb.sleep_timer > 0.35 {
                        rb.sleeping = true;
                        rb.velocity = Vec3::ZERO;
                    }
                } else {
                    rb.sleep_timer = 0.0;
                }
            } else {
                rb.sleep_timer = 0.0;
                // 空气阻尼：防止长时间自由落体后数值过大
                rb.velocity *= (1.0 - 0.02 * dt).clamp(0.0, 1.0);
            }
        }
    }
}

impl PhysicsBackend for BuiltinBackend {
    fn name(&self) -> &'static str {
        "builtin"
    }

    /// 只有「刚体 vs 地面」一条接触规则：不互撞、不旋转、无事件、无 CCD。
    fn capabilities(&self) -> BackendCaps {
        BackendCaps::BASIC
    }

    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;
        let inv_mass = match desc.kind {
            BodyKind::Dynamic => {
                if desc.mass > 1e-4 {
                    1.0 / desc.mass
                } else {
                    1.0
                }
            }
            _ => 0.0,
        };
        let half = body_half_extents(&desc);
        self.bodies.push(RigidBody {
            handle,
            node,
            kind: desc.kind,
            shape: desc.shape,
            half,
            inv_mass,
            velocity: Vec3::ZERO,
            restitution: desc.restitution.clamp(0.0, 1.0),
            friction: desc.friction.clamp(0.0, 2.0),
            position: desc.position,
            sleep_timer: 0.0,
            sleeping: false,
        });
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        self.bodies.retain(|b| b.handle != handle);
    }

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        if let Some(rb) = self.find_mut(handle) {
            if rb.inv_mass > 0.0 {
                rb.velocity += impulse * rb.inv_mass;
                rb.sleeping = false;
                rb.sleep_timer = 0.0;
            }
        }
    }

    fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
    }

    fn set_ground(&mut self, ground: f32) {
        self.ground = ground;
    }

    fn step(&mut self, dt: f32) {
        // 固定子步上限：掉帧时切成多步，保证碰撞不会穿透地板
        let total = dt.clamp(0.0, 0.1);
        if total <= 0.0 {
            return;
        }
        let steps = ((total / (1.0 / 60.0)).ceil() as usize).clamp(1, 6);
        let h = total / steps as f32;
        for _ in 0..steps {
            self.substep(h);
        }
    }

    fn pull_from_scene(&mut self, scene: &Scene) {
        for rb in self.bodies.iter_mut() {
            let Some(node) = scene.get(rb.node) else {
                continue;
            };
            // 脚本用 set_pos 挪过节点：以节点为准并唤醒刚体
            if (node.transform.position - rb.position).length_squared() > 1e-8 {
                rb.position = node.transform.position;
                rb.velocity = Vec3::ZERO;
                rb.sleeping = false;
                rb.sleep_timer = 0.0;
            }
        }
    }

    fn sync_to_scene(&mut self, scene: &mut Scene) {
        let mut dead: Vec<usize> = Vec::new();
        for (i, rb) in self.bodies.iter().enumerate() {
            match scene.get_mut(rb.node) {
                Some(node) => {
                    node.transform.position = rb.position;
                    node.body = Some(rb.handle.0);
                }
                None => dead.push(i),
            }
        }
        // 节点被 despawn 的刚体自动回收，避免句柄泄漏
        for i in dead.into_iter().rev() {
            self.bodies.remove(i);
        }
    }

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn has_body(&self, handle: BodyHandle) -> bool {
        self.bodies.iter().any(|b| b.handle == handle)
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.bodies.iter().map(|b| b.handle).collect())
    }

    /// 解析法射线查询：把每个刚体化成世界 AABB 做 slab 测试，取最近命中。
    ///
    /// 内置后端不旋转、形状本身就轴对齐，AABB 即精确包围体；
    /// 球用半径立方体近似（脚本拿来做拾取足够，需要精确球面就走 Rapier）。
    fn raycast(&self, ray: Ray, max_distance: f32) -> Option<RayHit> {
        if max_distance <= 0.0 {
            return None;
        }
        let mut best: Option<RayHit> = None;
        for rb in self.bodies.iter() {
            let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
            let Some((t, normal)) = ray_aabb(&ray, min, max, max_distance) else {
                continue;
            };
            if best.map_or(true, |b| t < b.distance) {
                best = Some(RayHit {
                    handle: rb.handle,
                    node: rb.node,
                    distance: t,
                    point: ray.point_at(t),
                    normal,
                });
            }
        }
        best
    }

    /// 解析法球体重叠：球心到 AABB 的最近距离 <= 半径即算命中。
    fn overlap_sphere(&self, center: Vec3, radius: f32) -> Vec<OverlapHit> {
        if radius <= 0.0 {
            return Vec::new();
        }
        let mut hits: Vec<OverlapHit> = self
            .bodies
            .iter()
            .filter(|rb| {
                let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
                aabb_distance_sq(center, min, max) <= radius * radius
            })
            .map(|rb| OverlapHit {
                handle: rb.handle,
                node: rb.node,
            })
            .collect();
        hits.sort_by_key(|h| h.node);
        hits
    }

    /// 解析法扫掠：把目标 AABB 按查询形状的半尺寸做 **Minkowski 展开**，再用射线 slab 求交。
    ///
    /// 盒 vs 盒是精确的（两个 AABB 的和仍是 AABB）；球用「半径立方体」展开，
    /// 因此在盒子棱角处偏保守（把圆角当方角），需要精确球面接触就走 Rapier。
    /// 这与 `raycast` 的近似口径保持一致，不额外引入第三套几何假设。
    fn shape_cast(
        &self,
        shape: CastShape,
        origin: Vec3,
        dir: Vec3,
        max_distance: f32,
    ) -> Option<ShapeCastHit> {
        if max_distance <= 0.0 || shape.is_degenerate() {
            return None;
        }
        let ray = Ray::new(origin, dir);
        let inflate = shape.half_extents();
        let mut best: Option<ShapeCastHit> = None;
        for rb in self.bodies.iter() {
            let (min, max) = body_aabb(rb.shape, rb.position, rb.half);
            let (min, max) = (min - inflate, max + inflate);
            // 起点已在膨胀盒内 → 初始重叠：距离 0，法线朝来向（与 raycast 内部起点口径一致）
            if point_in_aabb(ray.origin, min, max) {
                let cand = ShapeCastHit {
                    handle: rb.handle,
                    node: rb.node,
                    distance: 0.0,
                    point: ray.origin,
                    normal: -ray.dir,
                    started_inside: true,
                };
                if cast_candidate_better(&cand, best.as_ref()) {
                    best = Some(cand);
                }
                continue;
            }
            let Some((t, normal)) = ray_aabb(&ray, min, max, max_distance) else {
                continue;
            };
            let cand = ShapeCastHit {
                handle: rb.handle,
                node: rb.node,
                distance: t,
                point: ray.point_at(t),
                normal,
                started_inside: false,
            };
            if cast_candidate_better(&cand, best.as_ref()) {
                best = Some(cand);
            }
        }
        best
    }

    /// 胶囊重叠：包围盒相交即命中（解析近似，口径同 `raycast`）。
    ///
    /// 内置后端没有胶囊-盒距离场，这里用「胶囊 AABB 与刚体 AABB 相交」判定：
    /// 可能把擦角误判成命中，但**不会漏判**——角色控制器要的是「有没有卡住」，
    /// 宁可多报一次让调用方再确认，也不要少报导致穿地。要精确结果走 Rapier。
    fn overlap_capsule(&self, capsule: Capsule) -> Vec<PortHit> {
        if capsule.is_degenerate() {
            return Vec::new();
        }
        let (cmin, cmax) = capsule.aabb();
        let mut hits: Vec<PortHit> = Vec::new();
        for rb in self.bodies.iter() {
            let (bmin, bmax) = body_aabb(rb.shape, rb.position, rb.half);
            if !aabb_overlaps(cmin, cmax, bmin, bmax) {
                continue;
            }
            hits.push(PortHit {
                body: rb.handle.0,
                node: rb.node,
                distance: 0.0,
                point: capsule.center,
                normal: Vec3::ZERO,
                started_inside: true,
            });
        }
        hits.sort_by_key(|h| h.node);
        hits
    }

    /// 胶囊扫掠：胶囊 AABB 做 Minkowski 膨胀后与射线 slab 求交，同 `shape_cast` 的近似。
    fn sweep_capsule(&self, capsule: Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        if capsule.is_degenerate() || max_distance.is_nan() || max_distance <= 0.0 {
            return None;
        }
        let unit = dir.normalize_or_zero();
        if unit == Vec3::ZERO {
            return None;
        }
        let (cmin, cmax) = capsule.aabb();
        let inflate = Vec3::new(
            capsule.radius.abs(),
            capsule.half_height.abs() + capsule.radius.abs(),
            capsule.radius.abs(),
        );
        let ray = Ray::new(capsule.center, unit);
        let mut best: Option<PortHit> = None;
        for rb in self.bodies.iter() {
            let (bmin, bmax) = body_aabb(rb.shape, rb.position, rb.half);
            // 起步即重叠：距离 0，法线朝来向
            let cand = if aabb_overlaps(cmin, cmax, bmin, bmax) {
                PortHit {
                    body: rb.handle.0,
                    node: rb.node,
                    distance: 0.0,
                    point: capsule.center,
                    normal: -unit,
                    started_inside: true,
                }
            } else {
                let Some((t, normal)) =
                    ray_aabb(&ray, bmin - inflate, bmax + inflate, max_distance)
                else {
                    continue;
                };
                PortHit {
                    body: rb.handle.0,
                    node: rb.node,
                    distance: t,
                    point: ray.point_at(t),
                    normal,
                    started_inside: false,
                }
            };
            if port_hit_better(&cand, best.as_ref()) {
                best = Some(cand);
            }
        }
        best
    }

    /// 地表采样：地面平面与各刚体顶面里取最高的一个。
    ///
    /// 内置后端没有三角形地面，能提供的「地表」就是「地面平面 + 刚体 AABB 顶面」，
    /// 这与它的碰撞模型完全一致（物件只与地面平面相交，不互相支撑）。
    /// 因此站在两个叠放的箱子上时，只会采到最高箱子的顶面——
    /// 引擎里本来也没让它们互相支撑，采样不越权预报不存在的支撑。
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        let mut best = GroundSample::flat(self.ground);
        for rb in self.bodies.iter() {
            let (bmin, bmax) = body_aabb(rb.shape, rb.position, rb.half);
            if x < bmin.x || x > bmax.x || z < bmin.z || z > bmax.z {
                continue;
            }
            if bmax.y > best.height {
                best = GroundSample {
                    height: bmax.y,
                    normal: Vec3::Y,
                    body: Some(rb.handle.0),
                    node: Some(rb.node),
                };
            }
        }
        Some(best)
    }
}

/// 端口命中的择优规则：与 `cast_candidate_better` 同一套口径，只是字段不同。
fn port_hit_better(cand: &PortHit, best: Option<&PortHit>) -> bool {
    let Some(best) = best else {
        return true;
    };
    if cand.started_inside != best.started_inside {
        return cand.started_inside;
    }
    if (cand.distance - best.distance).abs() > 1e-4 {
        return cand.distance < best.distance;
    }
    cand.node < best.node
}

/// 两个 AABB 是否相交（含贴面接触）。
fn aabb_overlaps(cmin: Vec3, cmax: Vec3, bmin: Vec3, bmax: Vec3) -> bool {
    (0..3).all(|axis| cmin[axis] <= bmax[axis] && cmax[axis] >= bmin[axis])
}

/// 刚体在世界空间的轴对齐包围盒（内置后端不旋转，AABB 精确）。
fn body_aabb(shape: ColliderShape, center: Vec3, half: Vec3) -> (Vec3, Vec3) {
    let h = match shape {
        ColliderShape::Sphere => Vec3::splat(half.x.abs()),
        ColliderShape::Box => half.abs(),
        // 胶囊 / 圆柱在竖直方向更高，水平方向按半径处理
        ColliderShape::Capsule | ColliderShape::Cylinder => {
            Vec3::new(half.x.abs(), half.y.abs(), half.x.abs())
        }
    };
    (center - h, center + h)
}

/// 射线 vs AABB（slab 法）。返回 `(距离, 法线)`；起点在盒内时距离取 0、法线取 `-dir`。
fn ray_aabb(ray: &Ray, min: Vec3, max: Vec3, max_distance: f32) -> Option<(f32, Vec3)> {
    let mut t_enter = 0.0f32;
    let mut t_exit = max_distance;
    let mut normal = -ray.dir;
    for axis in 0..3 {
        let o = ray.origin[axis];
        let d = ray.dir[axis];
        let (lo, hi) = (min[axis], max[axis]);
        if d.abs() < 1e-6 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let mut t1 = (lo - o) * inv;
        let mut t2 = (hi - o) * inv;
        // 从哪一端进入：沿正方向走就是撞到 lo 面（法线朝 -轴），反之撞 hi 面。
        let mut n = Vec3::ZERO;
        n[axis] = if d > 0.0 { -1.0 } else { 1.0 };
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        if t1 > t_enter {
            t_enter = t1;
            normal = n;
        }
        t_exit = t_exit.min(t2);
        if t_enter > t_exit {
            return None;
        }
    }
    (t_enter <= max_distance).then_some((t_enter, normal))
}

/// 点到 AABB 的最近距离平方。
fn aabb_distance_sq(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    let mut d2 = 0.0;
    for axis in 0..3 {
        let v = p[axis];
        let e = if v < min[axis] {
            min[axis] - v
        } else if v > max[axis] {
            v - max[axis]
        } else {
            0.0
        };
        d2 += e * e;
    }
    d2
}

/// 点是否落在 AABB 内（含边界）。
fn point_in_aabb(p: Vec3, min: Vec3, max: Vec3) -> bool {
    (0..3).all(|axis| p[axis] >= min[axis] && p[axis] <= max[axis])
}

/// 统一推导碰撞体半尺寸：优先 BodyDesc.half_extents，退化到 scale * 0.5。
fn body_half_extents(desc: &BodyDesc) -> Vec3 {
    let h = desc.half_extents;
    if h.length_squared() > 1e-8 {
        h.abs()
    } else {
        (desc.scale.abs() * 0.5).max(Vec3::splat(0.001))
    }
}
