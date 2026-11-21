//! 物理查询端口（M2 · A03 / D1=B）：上层与具体物理引擎之间的唯一查询边界。
//!
//! ## 为什么单独成模块
//!
//! 门面（[`Physics`](crate::Physics)）已经把「后端可换」收在 crate 内：`Builtin` / `Rapier` /
//! `Null` 三套实现并存，换后端不动上层。但门面 API 的类型（`BodyDesc` / `Ray` / `CastShape` /
//! `BackendCaps`）都是**具体实现态**的口径，调用方一旦持有门面，就把后端、生命周期、
//! 内部可变性一起绑死。
//!
//! 本模块把「上层真正需要的三件事」抽成 trait：射线打一下、胶囊站不站得下、这块地面多高。
//! 上层（角色控制器 / 脚本 / 运行时）只认这三个 trait 与五个 DTO，不认后端是谁，
//! 也不认门面怎么持有。
//!
//! ```text
//!   r3e-character / r3e-runtime（B 系列）
//!              │  &dyn PhysicsQueryPort（本模块）
//!              ▼
//!      r3e-physics::port  ◄── 只依赖 r3e-core（NodeId）+ glam
//!              ▲
//!              │  impl RaycastPort / CapsulePort / GroundPort（port_impl.rs）
//!           Physics 门面 → Builtin / Rapier / Null
//! ```
//!
//! ## 落点说明（2026-11-20 修订）
//!
//! A03 原计划把本模块放进 `r3e-core`，理由是 D2=B 的 `r3e-character` 只许依赖 core。
//! 按项目负责人指令改放本 crate 后：端口与实现同库、少一层跨 crate 往返；代价是消费端
//! 多一条 `r3e-physics` 依赖边（`xtask/dep_rules.json` 的白名单已在 B01 同步登记）。
//! `r3e-core` 因此保持「零引擎依赖」不变 —— 依赖方向仍是 `physics → core`，没有回边。
//!
//! ## 口径约定（三组查询共用）
//!
//! * 方向向量一律先归一化；返回的 `distance` 是**世界距离**，不是某个 t 参数。
//! * 起点已重叠时 `distance == 0` 且 `started_inside == true`，不静默丢命中。
//! * 查询只覆盖「物理世界里的刚体」，后端自带的地面平面只在 [`GroundPort`] 里体现；
//!   这与 crate 内 `raycast` / `overlap_sphere` 既有的口径保持一致。
//! * 申报与行为必须一致：`port_caps()` 报 `false` 的能力，对应方法必须返回空结果
//!   （`None` / 空 `Vec`），不得「能查却报不支持」或「报支持却查不到」。
//!
//! ## 胶囊为什么是竖直的
//!
//! 角色控制器（M2 · B 系列）站姿 / 蹲姿都沿 +Y 轴，本模块只提供**轴对齐**的
//! [`Capsule`]：中心点 + 半高 + 半径。倾斜胶囊等到真有需求时再加变体，
//! 现在多一个自由度只会让三套后端各写一遍四元数转换。

use glam::Vec3;
use r3e_core::NodeId;

/// 射线查询输入。
///
/// `dir` 允许非单位向量，实现方须先归一化；`max_distance` 是归一化之后的世界距离上限。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastRay {
    /// 起点（世界坐标）。
    pub origin: Vec3,
    /// 方向，非零即可，不必归一化。
    pub dir: Vec3,
    /// 最大查询距离（世界单位）。
    pub max_distance: f32,
}

impl CastRay {
    pub fn new(origin: Vec3, dir: Vec3, max_distance: f32) -> Self {
        Self {
            origin,
            dir,
            max_distance,
        }
    }

    /// 归一化方向；方向退化时返回 `Vec3::ZERO`。
    pub fn normalized_dir(&self) -> Vec3 {
        self.dir.normalize_or_zero()
    }

    /// 沿归一化方向推进 `t` 后的世界点。
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.normalized_dir() * t
    }

    /// 方向退化或距离非正 → 这条射线没有意义，实现方应直接返回 `None`。
    pub fn is_degenerate(&self) -> bool {
        self.normalized_dir() == Vec3::ZERO
            || self.max_distance.is_nan()
            || self.max_distance <= 0.0
    }
}

/// 端口命中结果：射线 / 胶囊重叠 / 胶囊扫掠共用一套字段。
///
/// 重叠查询没有「接触法线」的概念，此时 `normal` 为 [`Vec3::ZERO`]、
/// `distance` 为 `0`、`started_inside` 为 `true`，调用方按 `started_inside` 区分即可。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortHit {
    /// 后端刚体句柄（`r3e-physics` 的 `BodyHandle.0`，对上层只当作不透明 id）。
    pub body: u64,
    /// 该刚体绑定的场景节点。
    pub node: NodeId,
    /// 世界距离；重叠查询恒为 0。
    pub distance: f32,
    /// 命中点（世界坐标）；重叠查询取查询形状的中心。
    pub point: Vec3,
    /// 命中处指向查询体的法线；重叠查询为 `ZERO`。
    pub normal: Vec3,
    /// 查询起点已与目标重叠（此时 `distance == 0`）。
    pub started_inside: bool,
}

/// 竖直胶囊：轴向固定 +Y，中心点 + 半高 + 半径。
///
/// 圆柱段高度 = `2 * half_height`，两端各一个半径 `radius` 的半球，
/// 于是整体高度 = `2 * (half_height + radius)`、整体半径 = `radius`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    /// 胶囊几何中心（世界坐标）。
    pub center: Vec3,
    /// 圆柱段半高（不含两端半球）。
    pub half_height: f32,
    /// 半径（必须为正才有效）。
    pub radius: f32,
}

impl Capsule {
    pub fn new(center: Vec3, half_height: f32, radius: f32) -> Self {
        Self {
            center,
            half_height,
            radius,
        }
    }

    /// 整体高度（含两端半球）。
    pub fn height(&self) -> f32 {
        2.0 * (self.half_height.abs() + self.radius.abs())
    }

    /// 半径非正 / 数值非有限 → 这条查询没有意义。
    ///
    /// 半径要求严格 `> 0`：负半径属于调用方算错，不在这里用 `abs` 悄悄纠正，
    /// 否则错误会被静默吞掉，症状推迟到「角色卡在地里」才暴露。
    pub fn is_degenerate(&self) -> bool {
        self.radius.is_nan()
            || self.radius <= 0.0
            || !self.center.is_finite()
            || !self.half_height.is_finite()
    }

    /// 胶囊的轴对齐包围盒 `(min, max)`；内置后端用它做解析近似查询。
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let r = self.radius.abs();
        let h = Vec3::new(r, self.half_height.abs() + r, r);
        (self.center - h, self.center + h)
    }
}

/// 地表采样结果：给定世界 `(x, z)`，返回该处「可站立的最高表面」。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundSample {
    /// 表面高度（世界 Y）。
    pub height: f32,
    /// 表面法线；平面地面为 `+Y`。
    pub normal: Vec3,
    /// 提供该表面的刚体句柄；平地（后端自带地面）为 `None`。
    pub body: Option<u64>,
    /// 提供该表面的场景节点；平地（后端自带地面）为 `None`。
    pub node: Option<NodeId>,
}

impl GroundSample {
    /// 平地采样（后端自带地面平面）。
    pub fn flat(height: f32) -> Self {
        Self {
            height,
            normal: Vec3::Y,
            body: None,
            node: None,
        }
    }

    /// 该表面是否足够平（`normal.y >= cos(45°)`），落地判定常用。
    pub fn is_walkable(&self, min_up_dot: f32) -> bool {
        self.normal.normalize_or_zero().y >= min_up_dot
    }
}

/// 端口能力位表。
///
/// 与 `r3e-physics::BackendCaps` 分开申报：后者描述的是**物理模拟**能力
/// （互撞、旋转、CCD、事件），这里只描述**查询端口**是否可用。
/// 两者不共享字段是有意的——端口能力将来会随角色控制器扩展，
/// 而不应该反过来拉动脚本能看到的模拟能力面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PortCaps {
    /// 射线查询（[`RaycastPort::cast_ray`]）。
    pub raycast: bool,
    /// 胶囊查询（[`CapsulePort::capsule_overlap`] / [`CapsulePort::capsule_sweep`]）。
    pub capsule: bool,
    /// 地表采样（[`GroundPort::sample_ground`]）。
    pub ground: bool,
}

impl PortCaps {
    /// 什么都不支持（空后端 / 不支持查询的后端）。
    pub const NONE: Self = Self {
        raycast: false,
        capsule: false,
        ground: false,
    };

    /// 三组查询全开（内置后端与 Rapier 后端当前都落在这一档）。
    pub const FULL: Self = Self {
        raycast: true,
        capsule: true,
        ground: true,
    };

    /// 是否一个能力都没申报。
    pub fn is_empty(&self) -> bool {
        !self.raycast && !self.capsule && !self.ground
    }

    /// 能力键名（稳定标识，供日志与测试消费）。
    pub fn enabled_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.raycast {
            keys.push("raycast");
        }
        if self.capsule {
            keys.push("capsule");
        }
        if self.ground {
            keys.push("ground");
        }
        keys
    }

    /// 逗号分隔摘要；全关时返回 `none`。
    pub fn summary(&self) -> String {
        let keys = self.enabled_keys();
        if keys.is_empty() {
            "none".to_string()
        } else {
            keys.join(",")
        }
    }

    /// 按能力名查询，未知键返回 false。
    pub fn has(&self, key: &str) -> bool {
        self.enabled_keys().contains(&key)
    }
}

/// 射线查询端口。
pub trait RaycastPort {
    /// 从 `ray.origin` 沿 `ray.dir` 打一条射线，返回最近命中的刚体。
    ///
    /// 默认实现返回 `None`：不支持就别假装，调用方按 `port_caps().raycast` 判断。
    fn cast_ray(&self, ray: &CastRay) -> Option<PortHit> {
        let _ = ray;
        None
    }
}

/// 胶囊查询端口（角色控制器的站姿/蹲姿判定就靠这两条）。
pub trait CapsulePort {
    /// 列出与 `capsule` 相交的所有刚体（按 `node` 升序，便于复现）。
    fn capsule_overlap(&self, capsule: &Capsule) -> Vec<PortHit> {
        let _ = capsule;
        Vec::new()
    }

    /// 把 `capsule` 沿 `dir` 推 `max_distance`，返回最近的一次接触。
    ///
    /// 起点已重叠时返回 `distance == 0` 且 `started_inside == true` 的命中，
    /// 由调用方决定是「贴脸滑开」还是「原地不动」。
    fn capsule_sweep(&self, capsule: &Capsule, dir: Vec3, max_distance: f32) -> Option<PortHit> {
        let _ = (capsule, dir, max_distance);
        None
    }
}

/// 地表采样端口。
pub trait GroundPort {
    /// 查询世界 `(x, z)` 处可站立的最高表面。
    ///
    /// 输入非有限值时返回 `None`；后端自带的地面平面恒存在，
    /// 所以正常情况下**总能**拿到一个样本（没有刚体就返回平地）。
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        let _ = (x, z);
        None
    }
}

/// 只读查询端口：三组 trait 的合集 + 能力申报。
///
/// 上层拿 `&dyn PhysicsQueryPort` 就够了，不需要知道背后是 Builtin、Rapier 还是别的引擎；
/// 也不需要 `use r3e_physics::...`（这正是 A03 要买的东西）。
pub trait PhysicsQueryPort: RaycastPort + CapsulePort + GroundPort {
    /// 如实申报本端口支持哪几组查询。
    fn port_caps(&self) -> PortCaps;
}

// ======================================================================
// 角色移动端口（M2 · B01）
// ======================================================================

/// 正面阻挡判据：命中法线与运动方向的对齐度超过它才算「撞上了」。
///
/// 贴在切向上的接触（站在地上横着走、沿着斜坡滑）法线垂直于运动方向，
/// 这类命中若当成阻挡，角色每一帧都会「被地面卡住」。
const BLOCK_DOT: f32 = 0.1;

/// 贴面余量：贴墙停下 / 抬升落回时故意留出的极小间隙。
///
/// 取值同时要满足两个约束：大到能让下一次查询不再判定「起点重叠」，
/// 小到肉眼与断言都当它不存在。改这个常量会直接移动相关单测的期望值。
const SKIN: f32 = 1e-3;

/// 一次角色移动意图。
///
/// 只带方向与跳：速度、重力、坡度的裁决全在角色控制器（`r3e-character`）里，
/// 端口层不做任何「这一步该不该走」的业务判断。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoveIntent {
    /// 期望移动方向：只取水平分量（`y` 一律忽略），零长度表示原地。
    pub wish_dir: Vec3,
    /// 本帧是否请求起跳（是否真能起跳由控制器按 `grounded` 裁决）。
    pub jump: bool,
}

/// 一次角色移动的结果。
///
/// `position` 是**胶囊中心**的新位置，不是脚底；脚底 = `position.y - (half_height + radius)`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveOutcome {
    /// 胶囊中心的新世界坐标。
    pub position: Vec3,
    /// 本帧结束后是否踩实。
    pub grounded: bool,
    /// 踩实时的表面法线；悬空为 `Vec3::ZERO`。
    pub ground_normal: Vec3,
    /// 水平移动是否被挡住（含「起点就重叠」的拒动）。
    pub blocked: bool,
    /// 本帧靠台阶抬升吃掉的高度差；没抬过为 `0.0`。
    pub step_up: f32,
}

impl MoveOutcome {
    /// 原地不动：被拒绝 / 无输入时的统一返回值。
    pub fn idle(position: Vec3) -> Self {
        Self {
            position,
            grounded: false,
            ground_normal: Vec3::ZERO,
            blocked: false,
            step_up: 0.0,
        }
    }
}

/// 角色移动端口：在 [`PhysicsQueryPort`] 之上补两条**移动**语义。
///
/// 与查询端口的分工：查询端口回答「这里能不能站」，本 trait 回答「这一步该走到哪」。
/// 两条都是带默认实现的纯函数，后端**零改动**即可用——这正是 B01 选「端口并列」
/// 而不是「塞进 [`PhysicsQueryPort`] 超 trait」的原因：超 trait 一改，
/// 三套后端与全部测试 mock 都要跟着实现，代价与收益不成比例。
///
/// 默认实现只依赖 [`CapsulePort::capsule_sweep`] / [`CapsulePort::capsule_overlap`]，
/// 因此 `Null` 后端天然「全都走不动」：既不 panic，也不假装成功。
pub trait CharacterPort: PhysicsQueryPort {
    /// 水平移动：先直扫，被挡住再试一次台阶抬升，都过不去就贴着障碍停下。
    ///
    /// 返回 `(新中心, 是否被挡, 台阶抬升量)`。
    /// * `dir` 的 `y` 分量被忽略；零长度方向直接返回原位置。
    /// * `distance <= 0` / 非有限：返回原位置（不是错误，是「这一帧没打算动」）。
    /// * `capsule` 退化（半径非正 / 坐标非有限）：返回原位置并标记被挡。
    fn move_horizontal(
        &self,
        capsule: &Capsule,
        dir: Vec3,
        distance: f32,
        max_step: f32,
    ) -> (Vec3, bool, f32) {
        let here = capsule.center;
        if capsule.is_degenerate() {
            return (here, true, 0.0);
        }
        let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        if dir.length_squared() == 0.0 || !distance.is_finite() || distance <= 0.0 {
            return (here, false, 0.0);
        }

        if let Some(hit) = self.capsule_sweep(capsule, dir, distance) {
            if hit.started_inside {
                // 起点就重叠：一步都不走，交给上层决定「贴脸滑开」还是「原地不动」。
                return (here, true, 0.0);
            }
            // 切向接触不是墙：贴着障碍侧面滑行时照旧走满这一帧。
            if hit.normal.normalize_or_zero().dot(dir) > -BLOCK_DOT {
                return (here + dir * distance, false, 0.0);
            }
            let step = if max_step.is_finite() {
                max_step.max(0.0)
            } else {
                0.0
            };
            if step > SKIN {
                let lifted =
                    Capsule::new(here + Vec3::Y * step, capsule.half_height, capsule.radius);
                if self.capsule_overlap(&lifted).is_empty() {
                    let advance = self
                        .capsule_sweep(&lifted, dir, distance)
                        .map(|h| {
                            if h.started_inside {
                                0.0
                            } else {
                                h.distance.min(distance)
                            }
                        })
                        .unwrap_or(distance);
                    // 抬升后必须**真的站上更高的面**才算跨过台阶：
                    // 落回时若整段高度又掉回去了（`raised == step`），说明前面是墙
                    // 而不是台阶 —— 墙不能被「抬一下再落回原处」蒙混过去。
                    if advance > SKIN {
                        let ahead = lifted.center + dir * advance;
                        let probe = Capsule::new(ahead, capsule.half_height, capsule.radius);
                        if let Some(drop) = self.capsule_sweep(&probe, -Vec3::Y, step + SKIN) {
                            if !drop.started_inside {
                                let raised = drop.distance.min(step);
                                if step - raised > SKIN {
                                    let pos = Vec3::new(ahead.x, ahead.y - raised, ahead.z);
                                    return (pos, false, step - raised);
                                }
                            }
                        }
                    }
                }
            }
            let advanced = (hit.distance - SKIN).max(0.0).min(distance);
            return (here + dir * advanced, true, 0.0);
        }

        (here + dir * distance, false, 0.0)
    }

    /// 竖直结算：上升先顶上，下降先按本帧位移扫掠、再按 `snap` 吸附。
    ///
    /// 返回 `(新中心, 是否踩实, 表面法线)`。
    /// * `velocity_y > 0`：向上扫掠，撞到顶就贴住，并恒报「未踩实」。
    /// * 其余情况先按 `|velocity_y| * dt` 下落；没踩到地面再用 `snap` 向下补一次
    ///   （下楼梯 / 走下缓坡时不做这一步就会一格一格地弹）。
    fn settle_vertical(
        &self,
        capsule: &Capsule,
        velocity_y: f32,
        dt: f32,
        snap: f32,
    ) -> (Vec3, bool, Vec3) {
        let here = capsule.center;
        if capsule.is_degenerate() {
            return (here, false, Vec3::ZERO);
        }
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        let snap = if snap.is_finite() { snap.max(0.0) } else { 0.0 };

        if velocity_y > 0.0 && velocity_y.is_finite() {
            let rise = velocity_y * dt;
            let head = match self.capsule_sweep(capsule, Vec3::Y, rise) {
                Some(hit) if !hit.started_inside => (hit.distance - SKIN).max(0.0).min(rise),
                Some(_) => 0.0,
                None => rise,
            };
            return (here + Vec3::Y * head, false, Vec3::ZERO);
        }

        let fall = if velocity_y.is_finite() {
            (-velocity_y * dt).max(0.0)
        } else {
            0.0
        };
        if fall > 0.0 {
            if let Some(hit) = self.capsule_sweep(capsule, -Vec3::Y, fall) {
                let down = if hit.started_inside {
                    0.0
                } else {
                    hit.distance.min(fall)
                };
                return (Vec3::new(here.x, here.y - down, here.z), true, hit.normal);
            }
        }
        let dropped = Vec3::new(here.x, here.y - fall, here.z);
        if snap > 0.0 {
            let probe = Capsule::new(dropped, capsule.half_height, capsule.radius);
            if let Some(hit) = self.capsule_sweep(&probe, -Vec3::Y, snap) {
                let down = if hit.started_inside {
                    0.0
                } else {
                    hit.distance.min(snap)
                };
                return (
                    Vec3::new(dropped.x, dropped.y - down, dropped.z),
                    true,
                    hit.normal,
                );
            }
        }
        (dropped, false, Vec3::ZERO)
    }
}

/// 一切具备查询能力的端口都自动具备移动能力：后端无需为角色控制器多写一行。
///
/// `?Sized` 是有意加的：`&dyn PhysicsQueryPort` 也要能直接调这两条默认方法，
/// 否则控制器签名就得从 `&dyn` 改成泛型，`r3e-character` 会被单态化绑死。
impl<T: PhysicsQueryPort + ?Sized> CharacterPort for T {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个什么都没实现的端口：所有默认实现都必须返回空，不能凭空造结果。
    struct Bare;

    impl RaycastPort for Bare {}
    impl CapsulePort for Bare {}
    impl GroundPort for Bare {}

    impl PhysicsQueryPort for Bare {
        fn port_caps(&self) -> PortCaps {
            PortCaps::NONE
        }
    }

    #[test]
    fn cast_ray_normalizes_and_steps_along_dir() {
        let ray = CastRay::new(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 5.0), 10.0);
        // 非单位方向也要按世界距离推进
        assert!((ray.normalized_dir() - Vec3::Z).length() < 1e-6);
        assert!((ray.point_at(3.0) - Vec3::new(0.0, 1.0, 3.0)).length() < 1e-6);
    }

    #[test]
    fn cast_ray_flags_degenerate_inputs() {
        let zero_dir = CastRay::new(Vec3::ZERO, Vec3::ZERO, 5.0);
        assert!(zero_dir.is_degenerate(), "零方向应判退化");
        let zero_dist = CastRay::new(Vec3::ZERO, Vec3::Z, 0.0);
        assert!(zero_dist.is_degenerate(), "零距离应判退化");
        let negative = CastRay::new(Vec3::ZERO, Vec3::Z, -1.0);
        assert!(negative.is_degenerate(), "负距离应判退化");
        let nan = CastRay::new(Vec3::ZERO, Vec3::Z, f32::NAN);
        assert!(nan.is_degenerate(), "NaN 距离应判退化");
        let ok = CastRay::new(Vec3::ZERO, Vec3::Z, 1.0);
        assert!(!ok.is_degenerate());
    }

    #[test]
    fn capsule_reports_height_and_aabb() {
        let c = Capsule::new(Vec3::new(0.0, 2.0, 0.0), 0.6, 0.4);
        // 整体高度 = 2 * (半高 + 半径)
        assert!((c.height() - 2.0).abs() < 1e-6);
        let (min, max) = c.aabb();
        assert!((min - Vec3::new(-0.4, 1.0, -0.4)).length() < 1e-6);
        assert!((max - Vec3::new(0.4, 3.0, 0.4)).length() < 1e-6);
        assert!(!c.is_degenerate());
    }

    #[test]
    fn capsule_rejects_non_positive_radius_and_nan() {
        assert!(
            Capsule::new(Vec3::ZERO, 0.5, 0.0).is_degenerate(),
            "零半径无效"
        );
        assert!(
            Capsule::new(Vec3::ZERO, 0.5, -0.3).is_degenerate(),
            "负半径无效"
        );
        assert!(
            Capsule::new(Vec3::new(f32::NAN, 0.0, 0.0), 0.5, 0.4).is_degenerate(),
            "非有限中心无效"
        );
        assert!(
            Capsule::new(Vec3::ZERO, f32::INFINITY, 0.4).is_degenerate(),
            "非有限半高无效"
        );
    }

    #[test]
    fn ground_sample_flat_is_walkable_up_only() {
        let flat = GroundSample::flat(-2.5);
        assert!((flat.height + 2.5).abs() < 1e-6);
        assert_eq!(flat.normal, Vec3::Y);
        assert_eq!(flat.body, None);
        assert_eq!(flat.node, None);
        assert!(flat.is_walkable(0.5), "平地必须可站");
        let steep = GroundSample {
            height: 0.0,
            normal: Vec3::new(1.0, 0.0, 0.0),
            body: Some(7),
            node: None,
        };
        assert!(!steep.is_walkable(0.5), "垂直面不可站");
    }

    #[test]
    fn port_caps_keys_are_stable() {
        assert_eq!(PortCaps::NONE.summary(), "none");
        assert!(PortCaps::NONE.is_empty());
        assert_eq!(PortCaps::FULL.summary(), "raycast,capsule,ground");
        assert!(!PortCaps::FULL.is_empty());
        assert!(PortCaps::FULL.has("capsule"));
        assert!(PortCaps::FULL.has("ground"));
        assert!(!PortCaps::NONE.has("raycast"), "未知/未申报的键必须 false");

        let partial = PortCaps {
            raycast: true,
            capsule: false,
            ground: false,
        };
        assert_eq!(partial.enabled_keys(), vec!["raycast"]);
        assert_eq!(partial.summary(), "raycast");
    }

    #[test]
    fn default_impls_pretend_nothing() {
        let bare = Bare;
        assert!(bare
            .cast_ray(&CastRay::new(Vec3::ZERO, Vec3::Z, 10.0))
            .is_none());
        assert!(bare
            .capsule_overlap(&Capsule::new(Vec3::ZERO, 0.5, 0.4))
            .is_empty());
        assert!(bare
            .capsule_sweep(&Capsule::new(Vec3::ZERO, 0.5, 0.4), Vec3::Y, 10.0)
            .is_none());
        assert!(bare.sample_ground(0.0, 0.0).is_none());
        assert_eq!(bare.port_caps(), PortCaps::NONE);
    }

    #[test]
    fn port_is_object_safe_and_injectable() {
        // 上层只拿 &dyn：这正是角色控制器 / 脚本注入的形态
        let bare = Bare;
        let ports: Vec<&dyn PhysicsQueryPort> = vec![&bare];
        for port in ports {
            assert_eq!(port.port_caps().summary(), "none");
            assert!(port.sample_ground(1.0, 1.0).is_none());
        }
    }
}
