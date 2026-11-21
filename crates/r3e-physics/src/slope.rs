//! 斜坡贴行裁决（M2 · B04）。
//!
//! 这里只回答一个问题：**脚前缘踩到的面是斜坡时，本帧该走到哪、抬多少**。
//! 与台阶的分工写在 [`slope_verdict`] 的文档里；本模块不做速度、重力、坡度上限
//! 这类业务判断，上限由调用方（角色控制器）通过 `min_up_dot` 传进来。
//!
//! 独立成模块是为了守住「单文件行数」这条门禁：`port.rs` 已经贴着上限，
//! 坡度裁决自带一套阈值常量与枚举，留在里面会把整个端口层顶穿。

use glam::Vec3;

use crate::port::{Capsule, PhysicsQueryPort, SKIN};

/// 视为平地的法线阈值：`normal.y` 高于它就不走坡度分支。
///
/// 差一丁点不值得为「1° 的坡」启动贴行逻辑 —— 那点抬升量比 `SKIN` 还小。
const SLOPE_FLAT: f32 = 1.0 - 1e-3;

/// 视为竖直面的法线阈值：`normal.y` 低于它交给扫掠逻辑判「撞墙」。
///
/// 不这么分，垂直墙面会被当成 90° 陡坡而走进「站着不动」分支，
/// B02 的撞墙判据会跟着红。
const SLOPE_VERTICAL: f32 = 0.1;

/// 贴面上行落点相对坡面**主动让出的间隙**（米）。
///
/// 不能取 0（精确贴面），也不能只取 [`SKIN`] 的 1 毫米：
/// * 取 0 时落点正好压在接触面上，下一次竖直结算向下扫掠的起点会落进后端
///   判重叠的预测距离（parry 缺省 2 毫米）→ 拿到 `started_inside` → 「脚下有
///   地面」被判成悬空，重力逐帧累积，二十帧后角色沉进坡体并穿过地板
///   （B04 实测，40° 坡）。
/// * 取 [`SKIN`] 量级仍在预测距离之内，坡长一换（BVH 结构变了）结论就翻：
///   同一帧、同一几何，`half_len = 3` 过、`half_len = 4` 崩 —— 判据会变成掷骰子。
///
/// 所以按端口层脱开探测的老口径给 1 厘米：足够让扫掠起点干净，竖直结算的吸附
/// （默认 0.3 米）又会把它落回真实接触面，误差不跨帧累积。
const SLOPE_LIFT: f32 = 1e-2;

/// 坡面裁决结果（[`slope_verdict`] 的返回值）。
pub(crate) enum SlopeVerdict {
    /// 脚下前方不是坡（平地 / 墙 / 下坡）：交给扫掠逻辑。
    Flat,
    /// 可走斜坡：本帧贴面上行到该位置，并如实上报抬升量。
    Climb(Vec3, f32),
    /// 站不住的陡坡：丢掉上行意图，原地不动 —— 不报 `blocked`（挡它的是坡度，不是墙）。
    TooSteep,
}

/// 坡面贴行裁决：脚前缘踩到的面是斜坡时，本帧该走到哪、抬多少。
///
/// 与台阶的分工：台阶是「刀刃」，抬升量由**采样到的高度差**决定；斜坡是连续面，
/// 抬升量由**坡度 × 本帧水平位移**决定。用高度差算斜坡会把脚前缘那段采样距离
/// （≈ 半径）乘进坡度里，40° 坡算出 29 厘米的「台阶」——超过抬升能力，
/// 于是角色在自家能走的坡上被自己的台阶判据挡住（B03 实测：走 60 帧 y 不动）。
pub(crate) fn slope_verdict<P: PhysicsQueryPort + ?Sized>(
    port: &P,
    capsule: &Capsule,
    dir: Vec3,
    distance: f32,
    max_step: f32,
    min_up_dot: f32,
) -> SlopeVerdict {
    let center = capsule.center;
    // 采样点取脚前缘：中心还在坡前平地时，只看中心永远采到平地，坡就上不去。
    let toe = center + dir * capsule.radius.abs().max(SKIN);
    let Some(ground) = port.sample_ground(toe.x, toe.z) else {
        return SlopeVerdict::Flat;
    };
    let n = ground.normal.normalize_or_zero();
    if n.length_squared() == 0.0 || n.y <= SLOPE_VERTICAL || n.y >= SLOPE_FLAT {
        return SlopeVerdict::Flat;
    }
    if n.y < min_up_dot {
        return SlopeVerdict::TooSteep;
    }
    // 沿行进方向的爬升率：法线的水平分量指向上坡的反方向，所以取负号。
    let climb_rate = -(n.dot(dir)) / n.y;
    if climb_rate <= 0.0 {
        // 横向走或下坡：不抬也不挡，竖直方向交给 `settle_vertical`。
        return SlopeVerdict::Flat;
    }
    let rise = distance * climb_rate;
    if rise > max_step + SKIN {
        // 单帧要抬的量已经超过抬升能力：不是坡该有的形状，退回台阶/撞墙逻辑。
        return SlopeVerdict::Flat;
    }
    // 落点高度 = max(坡度增量外推, 前缘坡面高度)。
    //
    // 只用坡度增量是不够的：坡脚第一帧的抬升相对真实坡面本来就是跳变的，
    // 之后每帧都差同一截，永远追不回来 —— 落点一帧比一帧埋得深，最后被下面的
    // 重叠闸整体拒掉，角色表现为「上坡三步后失速、重力接管、穿地」（B04 实测）。
    // 前缘高度直接对齐坡面，误差不跨帧累积。
    let foot = capsule.half_height.abs() + capsule.radius.abs();
    let radius = capsule.radius.abs();
    // 竖直胶囊贴斜面时，圆心到斜面的距离是 `radius`，投影到竖直方向就多出
    // `radius / n.y`；脚底因此比「平面接触」高 `radius * (1 / n.y - 1)`。
    let round_lift = radius * (1.0 / n.y - 1.0);
    let toe_aligned = ground.height + round_lift + foot;
    let ceiling = center.y + max_step + SKIN;
    let aligned = toe_aligned.max(center.y + rise).min(ceiling);
    // 落点主动让出 `SLOPE_LIFT` 再交给竖直结算（理由见常量文档）。
    let target = Vec3::new(
        center.x + dir.x * distance,
        aligned + SLOPE_LIFT,
        center.z + dir.z * distance,
    );
    // 让出间隙后仍与几何相交，说明不是贴面误差，而是真卡进去了：不走坡度分支。
    let probe = Capsule::new(target, capsule.half_height, capsule.radius);
    if !port.capsule_overlap(&probe).is_empty() {
        return SlopeVerdict::Flat;
    }
    SlopeVerdict::Climb(target, rise)
}
