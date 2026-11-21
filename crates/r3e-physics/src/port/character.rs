//! 角色移动端口（M2 · B01）：把「走一步」拆成水平意图 + 竖直结算两次查询。
//!
//! 从 [`port`](super) 拆出来的理由只有一条：`port.rs` 已经越过 800 行的超大文件阈值，
//! 而「查询边界」与「移动结算」本来就是两件事 —— 前者是后端如实报事实，后者是本 crate
//! 内唯一带业务判断的代码（阻挡、抬台阶、贴坡、吸附）。分文件后两边都是一条直线。

use glam::Vec3;

use super::{Capsule, PhysicsQueryPort};
use crate::slope::{slope_verdict, SlopeVerdict};

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
pub(crate) const SKIN: f32 = 1e-3;

/// 起点脱开探测的抬升量（米）。
///
/// 与 [`SKIN`] 分开是 B03 踩出来的：1 毫米的 `SKIN` 挪不动 `Rapier` 的起点重叠
/// 判定（parry 的接触预测距离缺省 2 mm，零间隙贴合一律报 penetrating），
/// 但把 `SKIN` 本身放大又会毒化所有「减去 SKIN」的结算换算（落回高度、贴墙距离
/// 全跟着漂）。所以脱开归脱开：单独给一个够大的抬升量，探完立刻把高度差减回去。
const DETACH: f32 = 1e-2;

/// 脱开探测的抬升上限（米）：探到这个高度还重叠，说明是真卡在几何里，不是贴面。
const DETACH_MAX: f32 = 5e-2;

/// 台阶落面的可站立判据：法线与竖直的夹角不超过 60°（与 `GroundSample::is_walkable`
/// 的常用口径一致）。斜面太陡不算台阶，省得角色顺着陡坡往上爬。
const WALKABLE_UP: f32 = 0.5;

/// 胶囊当前是否压在一个**朝上的面**上（贴面接触时用来判「脚下有地面」）。
///
/// 返回交集里最朝上的那个法线；没有朝上的面（嵌在墙里、悬在空中）返回 `None`。
/// 存在的理由：触面接触时向下扫掠一律返回 `started_inside`，没有距离可算，
/// 而「算不出距离」不等于「脚下没东西」。
fn support_normal<P: PhysicsQueryPort + ?Sized>(port: &P, capsule: &Capsule) -> Option<Vec3> {
    let mut best: Option<Vec3> = None;
    for hit in port.capsule_overlap(capsule) {
        let n = hit.normal.normalize_or_zero();
        if n.y >= WALKABLE_UP && best.map_or(true, |b| n.y > b.y) {
            best = Some(n);
        }
    }
    best
}

/// 贴面接触时判「脚下有没有地面」的完整口径：先问交集，交集说不出来再问地表。
///
/// 两步是必要的。`Rapier` 在 penetrating 状态下给出的接触法线并不可靠 —— B05 的
/// 三级台阶段实测：站在平台顶面、脚前缘贴着台阶立面时，交集返回的两条法线都是
/// 零向量，`support_normal` 只能答「没有朝上的面」。而那里明明是平地。所以交集的
/// 法线拿不到时，退一步用 [`GroundPort::sample_ground`] 采一次脚下：采样高度与脚底
/// 相距在 [`DETACH_MAX`] 以内、且采样法线可站，就按踩实处理。
fn support_under<P: PhysicsQueryPort + ?Sized>(port: &P, capsule: &Capsule) -> Option<Vec3> {
    if let Some(normal) = support_normal(port, capsule) {
        return Some(normal);
    }
    let foot = capsule.half_height.abs() + capsule.radius.abs();
    let feet = capsule.center.y - foot;
    let sample = port.sample_ground(capsule.center.x, capsule.center.z)?;
    let normal = sample.normal.normalize_or_zero();
    if normal.y >= WALKABLE_UP && (sample.height - feet).abs() <= DETACH_MAX {
        return Some(normal);
    }
    None
}

/// 起点脱开量：抬到「与任何几何都不再重叠」为止，抬不动就返回 0。
///
/// 只用于查询前把胶囊从贴面接触里挪出来，调用方拿到返回值后必须把这段高度
/// 从结果里**减回去**（它是查询技巧，不是位移）。
fn detach_lift<P: PhysicsQueryPort + ?Sized>(port: &P, capsule: &Capsule) -> f32 {
    let mut lift = DETACH;
    while lift <= DETACH_MAX {
        let probe = Capsule::new(
            capsule.center + Vec3::Y * lift,
            capsule.half_height,
            capsule.radius,
        );
        if port.capsule_overlap(&probe).is_empty() {
            return lift;
        }
        lift *= 2.0;
    }
    0.0
}

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
    /// 水平移动：先看脚下前方是不是坡，再直扫，被挡住再试一次台阶抬升，
    /// 都过不去就贴着障碍停下。
    ///
    /// 返回 `(新中心, 是否被挡, 台阶抬升量)`。
    /// * `dir` 的 `y` 分量被忽略；零长度方向直接返回原位置。
    /// * `distance <= 0` / 非有限：返回原位置（不是错误，是「这一帧没打算动」）。
    /// * `capsule` 退化（半径非正 / 坐标非有限）：返回原位置并标记被挡。
    /// * `min_up_dot`＝`cos(坡度上限)`：低于它的斜坡不许上行（丢掉上行意图，不报 `blocked`）。
    fn move_horizontal(
        &self,
        capsule: &Capsule,
        dir: Vec3,
        distance: f32,
        max_step: f32,
        min_up_dot: f32,
    ) -> (Vec3, bool, f32) {
        let here = capsule.center;
        if capsule.is_degenerate() {
            return (here, true, 0.0);
        }
        let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        if dir.length_squared() == 0.0 || !distance.is_finite() || distance <= 0.0 {
            return (here, false, 0.0);
        }
        let step = if max_step.is_finite() {
            max_step.max(0.0)
        } else {
            0.0
        };
        let min_up_dot = if min_up_dot.is_finite() {
            min_up_dot.clamp(0.0, 1.0)
        } else {
            1.0
        };

        // 坡面贴行：先于扫掠判定。斜坡上「贴着坡往前」会被扫掠报成撞墙，
        // 走得动走不动全看这里。
        match slope_verdict(self, capsule, dir, distance, step, min_up_dot) {
            SlopeVerdict::Climb(pos, rise) => return (pos, false, rise),
            SlopeVerdict::TooSteep => return (here, false, 0.0),
            SlopeVerdict::Flat => {}
        }

        // 起点脱开：站稳的瞬间胶囊底面与地面零间隙，扫掠会把「贴着地面站着」
        // 判成 `started_inside`，若照「起点重叠 ⇒ 一步都不走」处理，角色一落地
        // 就再也迈不开腿 —— 这是 B02 在真后端上抓到的缺陷（Rapier / Builtin
        // 都表现为「站稳以后所有水平移动全被 blocked」）。所以先上抬 `SKIN`
        // 让接触让开：抬了仍与几何重叠，才说明真的卡在墙里，那时才拒动。
        let lift = detach_lift(self, capsule);
        let lifted = Capsule::new(here + Vec3::Y * lift, capsule.half_height, capsule.radius);
        // `lift` 只是查询技巧，不该让角色每帧真的抬高那几毫米：凡是走
        // 「直接在原高度上结算」的分支，位置一律从 `here` 起算，不从 `from` 起算。
        let from = &lifted;

        if let Some(hit) = self.capsule_sweep(from, dir, distance) {
            // 切向接触不是墙：贴着障碍侧面滑行时照旧走满这一帧。
            //
            // 「起点就重叠」也按同一把尺子量：只有法线**顶着运动方向**的重叠
            // 才算卡住（人嵌在墙里、确实该原地不动）；法线朝上的支撑面同样
            // 放过。贴地站立时胶囊底面与地面零间隙，后端很容易把它报成
            // `started_inside`，若照「重叠 ⇒ 一步不走」处理，角色一落地
            // 就再也迈不开腿 —— B03 在 `Rapier` 上实测到的正是这一条。
            // 「法线朝下」的接触不是切向滑行，是**顶到了东西**：胶囊底部是半圆，
            // 撞上某个上棱边（台阶顶角、坡末端的平台前缘）时，头一个接触点的法线
            // 朝外且朝下，与水平运动方向的夹角可能小于 90°。只看水平分量就会把它
            // 当成「贴着侧面滑过去」，于是本帧满格前进却一毫米都不抬 —— 脚随即沉进
            // 脚下的面，竖直结算判悬空，角色从坡上直接穿下去（M2 · B05 可玩性回归
            // 在「斜坡 → 平台」接缝上实测到的坠落）。朝下的接触一律交给后面的台阶
            // 抬升分支：那才是「前面有个小坎」的正确出口。
            let n = hit.normal.normalize_or_zero();
            let tangential = n.dot(dir) > -BLOCK_DOT && n.y >= -BLOCK_DOT;
            if hit.started_inside {
                if tangential {
                    return (here + dir * distance, false, 0.0);
                }
                return (here, true, 0.0);
            }
            if tangential {
                return (here + dir * distance, false, 0.0);
            }
            if step > SKIN {
                let lifted = Capsule::new(
                    from.center + Vec3::Y * step,
                    capsule.half_height,
                    capsule.radius,
                );
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
                    // 抬升后必须**真的站上更高的面**才算跨过台阶：原地没抬起来
                    // （`rise == 0`）说明前面是墙而不是台阶 —— 墙不能被「抬一下
                    // 再落回原处」蒙混过去。
                    //
                    // 落面用「地表采样」而不是从抬升点向下扫掠：胶囊底部是半圆，
                    // 从高处往下扫时先碰到的是立面的**上棱边**（`Rapier` 实测法线
                    // (-0.93, 0.37)），不是顶面 —— 于是脚会停在顶面下方半米处
                    // 悬空，`grounded` 假、角色永远上不去台阶（B03 实测缺陷）。
                    // 采样返回的就是该 `(x, z)` 处「能站住的最高表面」，正是语义。
                    if advance > SKIN {
                        let ahead = lifted.center + dir * advance;
                        // 采样点探到**脚前缘**：胶囊中心走到台阶跟前时，中心还在
                        // 台阶外，只看中心会一直采到地面（`rise == 0`），台阶永远
                        // 迈不上去；脚前缘才是「能不能踩上去」的那个点。
                        let toe = ahead + dir * capsule.radius.abs();
                        if let Some(ground) = self.sample_ground(toe.x, toe.z) {
                            let foot = capsule.half_height.abs() + capsule.radius.abs();
                            let rise = ground.height - (here.y - foot);
                            if ground.is_walkable(WALKABLE_UP) && rise > SKIN && rise <= step {
                                // 位置落到**脚前缘**那一点上：台阶边缘是「脚踩上去、
                                // 重心还在阶外」的过渡姿态，只抬高度不往前挪，下一帧
                                // 脚下就是空的 —— 竖直结算判 `grounded = false`，
                                // 重力把人拽回去，台阶又变成迈不过的墙（B03 实测）。
                                // 所以这一帧按「迈一步」处理：水平直接进到顶面覆盖区内。
                                let pos = Vec3::new(toe.x, ground.height + foot, toe.z);
                                return (pos, false, rise);
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
            // 起跳：站位贴地时脚下那块地面会把「向上扫掠」的起点判成重叠，
            // 直接扫会得到「头顶立刻撞死」的假结论 —— 先脱开再扫，扫完把脱开量减回去。
            let lift = detach_lift(self, capsule);
            let probe = Capsule::new(here + Vec3::Y * lift, capsule.half_height, capsule.radius);
            let head = match self.capsule_sweep(&probe, Vec3::Y, rise + lift) {
                Some(hit) if !hit.started_inside => (hit.distance - lift).max(0.0).min(rise),
                Some(_) => 0.0,
                None => (rise + lift - lift).max(0.0).min(rise),
            };
            return (here + Vec3::Y * head, false, Vec3::ZERO);
        }

        let fall = if velocity_y.is_finite() {
            (-velocity_y * dt).max(0.0)
        } else {
            0.0
        };
        // 向下探测先把起点脱开：站在地上时「从接触位置直接下扫」踩在后端的预测
        // 距离边界上，Rapier 会给出不可靠结论（实测同一点位有时报 0.6 mm、有时
        // 直接「无命中」，后者让角色按自由落体往下掉，60 帧沉 1.8 厘米）。
        // 抬 `lift` 后扫 `reach + lift`，再把结果减掉 `lift` —— 与不抬等价，但不踩边界。
        let lift = detach_lift(self, capsule);
        let probe_at = |center: Vec3| {
            Capsule::new(center + Vec3::Y * lift, capsule.half_height, capsule.radius)
        };
        if fall > 0.0 {
            if let Some(hit) = self.capsule_sweep(&probe_at(here), -Vec3::Y, fall + lift) {
                if !hit.started_inside {
                    let down = (hit.distance - lift).max(0.0).min(fall);
                    // 净下落为 0：这一遍扫掠是「抬起 `lift` 之后才扫到的」，命中的是
                    // 侧面（法线水平甚至朝下），并不是脚下那层地。照它的法线往外报，
                    // 控制器会拿 `n.y < min_up_dot` 判悬空，`max_step` 当场归零 ——
                    // 站在台阶前 0.35 米处就再也迈不动。这种时候回退到真实姿态，
                    // 问一次「脚下有没有朝上的面」。
                    if down <= 0.0 {
                        if let Some(normal) = support_under(self, capsule) {
                            return (Vec3::new(here.x, here.y, here.z), true, normal);
                        }
                    }
                    return (Vec3::new(here.x, here.y - down, here.z), true, hit.normal);
                }
                // 起点就重叠：胶囊已经贴在（或略微嵌进）支撑面上。向下扫掠问不出
                // 距离，但这个交集**恰恰说明脚下有东西**。若照「没命中」放行，
                // 角色会被判悬空、重力逐帧累积，二十帧后沉进坡体并穿过地板 ——
                // B04 在 40° 坡上贴面行走时实测到的正是这一条。只认朝上的面：
                // 嵌在墙里不算踩实，那种情况照旧按悬空处理。
                // 探测用**未抬起**的真实姿态：抬 `lift` 是为了让扫掠起点干净，
                // 但它同时把胶囊从支撑面上摘了下去 —— 拿抬起后的姿态问「脚下有没有
                // 朝上的面」，站地贴面时会一律答「没有」。
                if let Some(normal) = support_under(self, capsule) {
                    return (here, true, normal);
                }
            }
        }
        let dropped = Vec3::new(here.x, here.y - fall, here.z);
        if snap > 0.0 {
            if let Some(hit) = self.capsule_sweep(&probe_at(dropped), -Vec3::Y, snap + lift) {
                if !hit.started_inside {
                    let down = (hit.distance - lift).max(0.0).min(snap);
                    // 与 `fall` 分支同理：净下落 0 说明扫到的是侧面而不是地。
                    if down <= 0.0 {
                        if let Some(normal) = support_under(self, capsule) {
                            return (Vec3::new(dropped.x, dropped.y, dropped.z), true, normal);
                        }
                    }
                    return (
                        Vec3::new(dropped.x, dropped.y - down, dropped.z),
                        true,
                        hit.normal,
                    );
                }
                // 与上面 `fall` 分支同一把尺子：起点重叠就是脚下有东西。这里必须
                // 单独补一遍，因为「站稳（`fall` = 0）+ 脚前缘顶到台阶立面」会整个
                // 跳过 `fall` 分支，只剩吸附这一遍扫掠；缺了兜底，角色在台阶前
                // 丢掉 `grounded`，抬升能力随之归零（`max_step` 直接取 `grounded`），
                // 从此一步也迈不动 —— M2 · B05 可玩性回归实测：三级台阶段卡在
                // 第一级立面外 0.35 米处，`blocked` 恒真、`x` 再也不动。
                // 同 `fall` 分支：用未抬起的姿态问支撑面。
                if let Some(normal) = support_under(
                    self,
                    &Capsule::new(dropped, capsule.half_height, capsule.radius),
                ) {
                    return (dropped, true, normal);
                }
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
    use crate::port::{CapsulePort, CastRay, GroundPort, GroundSample, PortCaps, RaycastPort};

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
