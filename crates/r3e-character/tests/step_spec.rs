//! B03 台阶判据：抬升能力之内上得去、之外挡住；下行不看抬升能力，只看吸附与自由落体。
//!
//! 这些判据都不是解析恒等式 —— 「抬起来再扫一遍、再向下探一次」是后端扫掠与重叠
//! 判定的组合结果，只用自己写的 AABB 验等于自己出题自己答，所以每条都在真后端
//! （`Rapier` 与 `Builtin`）上各跑一遍。
//!
//! 与方案稿的一处口径差异（见 `docs/m2/M2_B_PLAN.md` §6.4）：方案稿把「1.1 倍台阶」
//! 写成「上/下都 blocked」。但**下行本来就不该被抬升能力挡住**：抬升是上行需求，
//! 下行只受吸附距离约束。所以下行拆成两条独立判据 —— 落差在吸附距离内平滑落地、
//! 远大于吸附距离时自由落体后落地，两条都不许 `blocked`。

mod common;

use common::*;
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, FIXED_DT,
};
use r3e_physics::port::PhysicsQueryPort;
use r3e_physics::{Physics, PhysicsKind};

/// 竖直容差（米）：1 毫米。
const EPS_Y: f32 = 1e-3;

/// 起点离地高度（米）：先从空中落下站稳，判据才有干净起点。
const DROP_FROM: f32 = 0.5;

/// 台阶左立面所在 x。
const LEDGE_X: f32 = 1.0;

/// 每帧水平位移（米）＝默认 `move_speed 4.5 / 60`，判据里的余量都按它算。
const STEP_DX: f32 = 4.5 / 60.0;

/// 上台阶时走多少帧（≈2.25 米，停在顶面中部）。
///
/// 顶面从 `LEDGE_X` 向 +X 延伸 3 米，走满 60 帧（4.5 米）会越过右边缘掉回地面，
/// 「站上台阶」的断言就失去了观察对象 —— 所以帧数按「走到顶面中间」取。
const CLIMB_FRAMES: usize = 30;

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

/// 抬升能力之内 / 之外的台阶高度（米）。
fn within() -> f32 {
    config().step_height * 0.9
}

fn beyond() -> f32 {
    config().step_height * 1.1
}

/// 后端未编译进本构建时整条用例跳过（`--no-default-features` 下只剩 `Builtin`）。
fn usable(kind: PhysicsKind) -> bool {
    let ok = kind.is_available();
    if !ok {
        eprintln!("跳过：后端 {kind:?} 未编译进本构建");
    }
    ok
}

fn step(
    port: &dyn PhysicsQueryPort,
    state: &mut CharacterState,
    wish: Vec3,
    dt: f32,
) -> r3e_character::MoveOutcome {
    KinematicSolver.step(
        &config(),
        state,
        MoveIntent {
            wish_dir: wish,
            jump: false,
        },
        dt,
        port,
    )
}

/// 一次水平行走的汇总：位置、是否被挡过、最大单帧抬升、最后一帧是否踩实。
struct Walk {
    feet: Vec3,
    blocked: bool,
    step_up_max: f32,
    grounded_last: bool,
    step_up_frames: usize,
}

/// 从 `start` 走向 `wish` 共 `frames` 帧（起点先落下站稳）。
fn walk(
    port: &dyn PhysicsQueryPort,
    start: Vec3,
    wish: Vec3,
    frames: usize,
) -> (Walk, CharacterState) {
    let mut state = CharacterState::new(start);
    for _ in 0..60 {
        step(port, &mut state, Vec3::ZERO, FIXED_DT);
    }
    let mut blocked = false;
    let mut step_up_max = 0.0f32;
    let mut step_up_frames = 0usize;
    let mut grounded_last = state.grounded;
    for _ in 0..frames {
        let out = step(port, &mut state, wish, FIXED_DT);
        blocked |= out.blocked;
        if out.step_up > 0.0 {
            step_up_max = step_up_max.max(out.step_up);
            step_up_frames += 1;
        }
        grounded_last = out.grounded;
    }
    (
        Walk {
            feet: state.feet,
            blocked,
            step_up_max,
            grounded_last,
            step_up_frames,
        },
        state,
    )
}

/// 「0.5 米空降 + 站稳」这条前提本身也得成立，否则后面的判断都没有意义。
fn stand_on_floor(kind: PhysicsKind) -> Physics {
    let physics = floor_world(kind);
    let mut state = CharacterState::new(Vec3::new(-1.0, DROP_FROM, 0.0));
    for _ in 0..60 {
        step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    }
    assert!(state.grounded, "{kind:?}：平地站稳失败，判据前提不成立");
    physics
}

// ---------------------------------------------------------------- ① 能力之内上得去

fn climbs_within_step(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let top = within();
    let physics = with_step(stand_on_floor(kind), LEDGE_X, top);
    let (w, state) = walk(
        &physics,
        Vec3::new(0.0, DROP_FROM, 0.0),
        Vec3::X,
        CLIMB_FRAMES,
    );

    assert!(
        state.feet.x > LEDGE_X,
        "{kind:?}：{} 米台阶在 {} 米抬升能力内，应能迈上去，实际停在 x={}",
        top,
        config().step_height,
        state.feet.x
    );
    assert!(
        state.feet.x < LEDGE_X + 3.0 - config().radius,
        "{kind:?}：判据观察点应在顶面之内，实际 x={}",
        state.feet.x
    );
    assert!(
        (state.feet.y - top).abs() < EPS_Y * 2.0,
        "{kind:?}：站上台阶后脚底应为 {top}，实际 {}",
        state.feet.y
    );
    assert!(!w.blocked, "{kind:?}：迈得上却被判 blocked");
    assert!(
        (w.step_up_max - top).abs() < EPS_Y * 2.0,
        "{kind:?}：抬升量应如实上报 {top}，实际 {}",
        w.step_up_max
    );
    assert!(w.grounded_last, "{kind:?}：站上台阶顶必须报踩实");
}

#[test]
fn climbs_within_step_rapier() {
    climbs_within_step(PhysicsKind::Rapier);
}

#[test]
fn climbs_within_step_builtin() {
    climbs_within_step(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ② 能力之外挡住

fn blocks_beyond_step(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let top = beyond();
    let physics = with_step(stand_on_floor(kind), LEDGE_X, top);
    let (w, state) = walk(&physics, Vec3::new(0.0, DROP_FROM, 0.0), Vec3::X, 60);

    assert!(
        w.blocked,
        "{kind:?}：{} 米台阶超过 {} 米抬升能力，必须挡住",
        top,
        config().step_height
    );
    assert!(
        state.feet.y.abs() < EPS_Y,
        "{kind:?}：被挡住时不许自己长高，脚底 {}",
        state.feet.y
    );
    assert_eq!(w.step_up_max, 0.0, "{kind:?}：跨不过去就不许上报抬升");
    assert!(
        state.feet.x <= LEDGE_X + EPS_Y,
        "{kind:?}：应停在立面外，实际 x={}",
        state.feet.x
    );
    assert!(
        state.feet.x > LEDGE_X - config().radius - STEP_DX,
        "{kind:?}：停在立面外一帧位移之内，实际 x={}",
        state.feet.x
    );
}

#[test]
fn blocks_beyond_step_rapier() {
    blocks_beyond_step(PhysicsKind::Rapier);
}

#[test]
fn blocks_beyond_step_builtin() {
    blocks_beyond_step(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ③ 下行：吸附距离之内

fn descends_within_snap(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let top = within();
    let physics = with_step(stand_on_floor(kind), LEDGE_X, top);
    // 站在台阶顶上，往 -X 走下来。
    let (w, state) = walk(
        &physics,
        Vec3::new(LEDGE_X + 1.5, top + DROP_FROM, 0.0),
        -Vec3::X,
        60,
    );

    assert!(
        state.feet.x < LEDGE_X,
        "{kind:?}：应已走出台阶范围，实际 x={}",
        state.feet.x
    );
    assert!(
        state.feet.y.abs() < EPS_Y * 2.0,
        "{kind:?}：{} 米落差在 {} 米吸附距离内，应平滑落地到 y≈0，实际 {}",
        top,
        config().snap_distance,
        state.feet.y
    );
    assert!(!w.blocked, "{kind:?}：下台阶不该被判 blocked");
    assert!(w.grounded_last, "{kind:?}：走下来后应踩实地面");
}

#[test]
fn descends_within_snap_rapier() {
    descends_within_snap(PhysicsKind::Rapier);
}

#[test]
fn descends_within_snap_builtin() {
    descends_within_snap(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ④ 下行：超出吸附距离

fn free_falls_beyond_snap(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    // 三倍抬升能力 ≈ 1.05 米，远超 0.4 米吸附距离：走下来只能自由落体。
    let top = config().step_height * 3.0;
    let physics = with_step(stand_on_floor(kind), LEDGE_X, top);
    let (w, state) = walk(
        &physics,
        Vec3::new(LEDGE_X + 1.5, top + DROP_FROM, 0.0),
        -Vec3::X,
        90,
    );

    assert!(
        state.feet.x < LEDGE_X,
        "{kind:?}：应已走出台阶范围，实际 x={}",
        state.feet.x
    );
    assert!(
        state.feet.y.abs() < EPS_Y * 2.0,
        "{kind:?}：{} 米落差超过吸附距离，落完应贴地 y≈0，实际 {}",
        top,
        state.feet.y
    );
    assert!(
        !w.blocked,
        "{kind:?}：下行没有抬升需求，落差再大也不该 blocked"
    );
    assert!(w.grounded_last, "{kind:?}：落完应踩实地面");
}

#[test]
fn free_falls_beyond_snap_rapier() {
    free_falls_beyond_snap(PhysicsKind::Rapier);
}

#[test]
fn free_falls_beyond_snap_builtin() {
    free_falls_beyond_snap(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑤ 抬升能力为零

fn zero_step_height_blocks(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let cfg = CharacterConfig {
        step_height: 0.0,
        ..CharacterConfig::default()
    };
    // 0.1 米的小台阶：能力为零时也不许「小到看不出来就迈上去」。
    let top = 0.1;
    let physics = with_step(stand_on_floor(kind), LEDGE_X, top);
    let mut state = CharacterState::new(Vec3::new(0.0, DROP_FROM, 0.0));
    for _ in 0..60 {
        KinematicSolver.step(&cfg, &mut state, MoveIntent::default(), FIXED_DT, &physics);
    }
    let mut blocked = false;
    for _ in 0..60 {
        let out = KinematicSolver.step(
            &cfg,
            &mut state,
            MoveIntent {
                wish_dir: Vec3::X,
                jump: false,
            },
            FIXED_DT,
            &physics,
        );
        blocked |= out.blocked;
        assert_eq!(out.step_up, 0.0, "{kind:?}：能力为零时不许上报抬升");
    }

    assert!(blocked, "{kind:?}：step_height=0 时任何台阶都必须挡住");
    assert!(
        state.feet.y.abs() < EPS_Y,
        "{kind:?}：被挡住时脚底不该抬高，实际 {}",
        state.feet.y
    );
    assert!(
        state.feet.x <= LEDGE_X,
        "{kind:?}：应停在立面外，实际 x={}",
        state.feet.x
    );
}

#[test]
fn zero_step_height_blocks_rapier() {
    zero_step_height_blocks(PhysicsKind::Rapier);
}

#[test]
fn zero_step_height_blocks_builtin() {
    zero_step_height_blocks(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑥ 斜坡：站得住的坡上得去

/// 踏板半长（米）：坡沿 x ∈ [0, 3] 上升，够站几个人。
const RAMP_HALF_LEN: f32 = 3.0;

/// 上坡起点：坡脚外一米，先在地面站稳再往里走。
const RAMP_START_X: f32 = -1.0;

/// 上坡走多少帧（≈2.25 米，够爬完坡脚到坡腰）。
const RAMP_FRAMES: usize = 30;

/// DSL 视角下「可走」的坡：上限的 0.9 倍。
fn ramp_within() -> f32 {
    config().max_slope_deg * 0.9
}

/// 站不住的陡坡：上限的 1.1 倍。
fn ramp_beyond() -> f32 {
    config().max_slope_deg * 1.1
}

/// 斜坡夹具：真后端画旋转踏板，解析世界用倾斜法线 + 逐点高度。
///
/// `Builtin` 后端的 `body_aabb` 只看 `position` / `half_extents`，旋转被丢掉
/// （见 [`with_ramp`] 注释），画不出坡 —— 所以它这一路改走 [`World::slope`]。
/// 这条「降级」是夹具层的，不是被测代码层的：端口契约一样，走的是同一份
/// `move_horizontal`。
fn ramp_port(kind: PhysicsKind, deg: f32) -> Box<dyn PhysicsQueryPort> {
    match kind {
        PhysicsKind::Builtin => Box::new(World::slope(0.0, deg)),
        _ => Box::new(with_ramp(stand_on_floor(kind), deg, RAMP_HALF_LEN)),
    }
}

/// 坡面在 `x` 处的高度（与夹具几何同一口径：过原点沿 +X 上升）。
fn ramp_height(deg: f32, x: f32) -> f32 {
    (x * deg.to_radians().tan()).max(0.0)
}

/// 站在斜面上时，「脚底」（`center.y - (half_height + radius)`）比坡面高出的量。
///
/// 胶囊底面是半圆：球与斜面按**法线**相切，球心到斜面的垂距是 `r`，换算到竖直
/// 方向就是 `r / cosθ` —— 于是竖直意义上的脚底必然浮起 `r * (secθ - 1)`（40.5°
/// 时约 11 厘米）。这是几何事实，不是控制器漏了吸附：把「脚底 = 坡面高度」当断言，
/// 任何正确实现都过不了。
///
/// 解析世界的 AABB 模型没有这层（它按 `mn.y - surface_y` 判支撑），脚底直接贴坡面，
/// 所以两种夹具各按各的口径取期望值。
fn ramp_foot_lift(kind: PhysicsKind, deg: f32) -> f32 {
    match kind {
        PhysicsKind::Builtin => 0.0,
        _ => config().radius * (1.0 / deg.to_radians().cos() - 1.0),
    }
}

/// 坡上期望脚底高度：坡面高度 + 胶囊半圆带来的浮起量。
///
/// 走到坡脚以外的平地时浮起量为 0（平地上「脚底」就是地面）。
fn expect_feet_on_ramp(kind: PhysicsKind, deg: f32, x: f32) -> f32 {
    let h = ramp_height(deg, x);
    if h <= 0.0 {
        return 0.0;
    }
    h + ramp_foot_lift(kind, deg)
}

fn climbs_walkable_ramp(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let deg = ramp_within();
    let port = ramp_port(kind, deg);
    let (w, state) = walk(
        port.as_ref(),
        Vec3::new(RAMP_START_X, DROP_FROM, 0.0),
        Vec3::X,
        RAMP_FRAMES,
    );

    assert!(
        w.feet.x > 0.5,
        "{kind:?}：{deg}° 坡在 {}° 上限内，应能走上去，实际停在 x={}",
        config().max_slope_deg,
        w.feet.x
    );
    assert_eq!(w.feet, state.feet, "{kind:?}：汇总与状态应指向同一位置");
    let expect = expect_feet_on_ramp(kind, deg, state.feet.x);
    assert!(
        (state.feet.y - expect).abs() < EPS_Y * 20.0,
        "{kind:?}：脚底应贴着坡面 {expect}，实际 {}",
        state.feet.y
    );
    assert!(!w.blocked, "{kind:?}：可走的坡不该判 blocked");
    assert!(w.grounded_last, "{kind:?}：坡上走完必须报踩实");
}

#[test]
fn climbs_walkable_ramp_rapier() {
    climbs_walkable_ramp(PhysicsKind::Rapier);
}

#[test]
fn climbs_walkable_ramp_analytic() {
    climbs_walkable_ramp(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑦ 斜坡：站不住的陡坡上不去

fn blocks_steep_ramp(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let deg = ramp_beyond();
    let port = ramp_port(kind, deg);
    let (w, state) = walk(
        port.as_ref(),
        Vec3::new(RAMP_START_X, DROP_FROM, 0.0),
        Vec3::X,
        RAMP_FRAMES,
    );

    assert!(
        state.feet.x < 0.0,
        "{kind:?}：{deg}° 超过 {}° 上限，应停在坡脚外，实际 x={}",
        config().max_slope_deg,
        state.feet.x
    );
    assert!(
        state.feet.y.abs() < EPS_Y * 2.0,
        "{kind:?}：陡坡上不去就不许自己长高，脚底 {}",
        state.feet.y
    );
    assert_eq!(w.step_up_max, 0.0, "{kind:?}：陡坡不是台阶，不许上报抬升");
    assert!(
        !w.blocked,
        "{kind:?}：挡住陡坡的是坡度不是墙，不该报 blocked"
    );
}

#[test]
fn blocks_steep_ramp_rapier() {
    blocks_steep_ramp(PhysicsKind::Rapier);
}

#[test]
fn blocks_steep_ramp_analytic() {
    blocks_steep_ramp(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑧ 斜坡：下坡不受坡度限制

fn descends_walkable_ramp(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let deg = ramp_within();
    let port = ramp_port(kind, deg);
    let start_x = 1.5;
    let (w, state) = walk(
        port.as_ref(),
        Vec3::new(start_x, ramp_height(deg, start_x) + DROP_FROM, 0.0),
        -Vec3::X,
        RAMP_FRAMES,
    );

    assert!(
        state.feet.x < 0.5,
        "{kind:?}：应已走下坡，实际 x={}",
        state.feet.x
    );
    let expect = expect_feet_on_ramp(kind, deg, state.feet.x);
    assert!(
        (state.feet.y - expect).abs() < EPS_Y * 20.0,
        "{kind:?}：下行脚底应贴坡面 {expect}，实际 {}",
        state.feet.y
    );
    assert!(!w.blocked, "{kind:?}：下坡没有上行意图，不该 blocked");
    assert!(w.grounded_last, "{kind:?}：下坡走完应踩实");
}

#[test]
fn descends_walkable_ramp_rapier() {
    descends_walkable_ramp(PhysicsKind::Rapier);
}

#[test]
fn descends_walkable_ramp_analytic() {
    descends_walkable_ramp(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑨ 斜坡不是台阶

fn ramp_reports_no_step_up(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let deg = ramp_within();
    let port = ramp_port(kind, deg);
    let (w, _) = walk(
        port.as_ref(),
        Vec3::new(RAMP_START_X, DROP_FROM, 0.0),
        Vec3::X,
        RAMP_FRAMES,
    );

    // 一帧最多走 `STEP_DX`，坡再陡也只能抬 `STEP_DX * tan(deg)` —— 远小于台阶。
    let budget = STEP_DX * deg.to_radians().tan() * 1.5 + EPS_Y;
    assert!(
        w.step_up_max <= budget,
        "{kind:?}：坡上单帧抬升 {} 超过坡度预算 {budget}，被当成台阶了",
        w.step_up_max
    );
    // 起步那几帧还在坡脚外的平地上，抬升为 0 是应该的；但绝大多数帧必须有连续抬升，
    // 否则说明走的是「绝大多数帧不抬、偶尔一跳」的台阶语义。
    assert!(
        w.step_up_frames * 2 > RAMP_FRAMES,
        "{kind:?}：坡上只有 {} / {RAMP_FRAMES} 帧有抬升，不像连续贴面",
        w.step_up_frames
    );
}

#[test]
fn ramp_reports_no_step_up_rapier() {
    ramp_reports_no_step_up(PhysicsKind::Rapier);
}

#[test]
fn ramp_reports_no_step_up_analytic() {
    ramp_reports_no_step_up(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑩ 坡上站得住

fn stands_on_ramp(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let deg = ramp_within();
    let port = ramp_port(kind, deg);
    let start_x = 1.0;
    let (_, state) = walk(
        port.as_ref(),
        Vec3::new(start_x, ramp_height(deg, start_x) + DROP_FROM, 0.0),
        Vec3::ZERO,
        RAMP_FRAMES,
    );

    assert!(
        state.grounded,
        "{kind:?}：{}° 坡在 {}° 上限内，站在上面必须报踩实",
        deg,
        config().max_slope_deg
    );
    assert!(
        state.ground_normal.y >= config().min_up_dot() - EPS_Y,
        "{kind:?}：坡面法线应倾斜但站得住，实际 {}",
        state.ground_normal
    );
    let expect = expect_feet_on_ramp(kind, deg, state.feet.x);
    assert!(
        (state.feet.y - expect).abs() < EPS_Y * 20.0,
        "{kind:?}：静止时不该沿坡下滑，脚底 {} 应为 {expect}",
        state.feet.y
    );
}

#[test]
fn stands_on_ramp_rapier() {
    stands_on_ramp(PhysicsKind::Rapier);
}

#[test]
fn stands_on_ramp_analytic() {
    stands_on_ramp(PhysicsKind::Builtin);
}
