//! B04 判据：沿坡面直线行走 N 步，每个采样点的实测脚底高度与「坡面解析高度」之差 < 2cm。
//!
//! 为什么单独立一个文件而不并进 `step_spec`：这份判据的容差比规程里其它判据宽一个量级
//! （2cm 对 1mm），且判据本身是**误差包络**而非单点是否成立 —— 混在一起会让读者误以为
//! 台阶判据也放宽了。
//!
//! 四档坡度（5° / 15° / 30° / 40°）全部落在默认 45° 上限之内，所以「走不上去」不是
//! 解释项：走不上去就是红卡。两个后端各跑一遍（解析世界的 AABB 与 Rapier 的 CCD 是
//! 两套完全不同的数值路径）。
//!
//! 一处口径说明：`Rapier` 侧脚底不是坡面高度，而是坡面往上多出 `r * (secθ - 1)` 的
//! 胶囊半圆高度（40° 时约 7.6cm）。那是几何事实，不是控制器没吸附 —— 期望值里减掉它，
//! 剩下的 2cm 才是真正被断言的东西。

mod common;

use common::*;
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, MoveOutcome,
    FIXED_DT,
};
use r3e_physics::port::PhysicsQueryPort;
use r3e_physics::{Physics, PhysicsKind};

/// 高度误差容差（米）：2 厘米。放宽它就得先改 `docs/m2/M2_B_PLAN.md` §7.1。
const RAMP_HEIGHT_TOL: f32 = 0.02;

/// 踏板半长（米）：要够 120 帧走完（≈9 米），最短的 40° 档也得留足投影长度。
const RAMP_HALF_LEN: f32 = 12.0;

/// 直行帧数（2 秒）。
const WALK_FRAMES: usize = 120;

/// 采样间隔（帧）：每 10 帧记一次，共 12 个采样点。
const SAMPLE_EVERY: usize = 10;

/// 起点离地高度（米）：先从空中落下站稳，判据才有干净起点。
const DROP_FROM: f32 = 0.5;

/// 上坡起点：坡脚外一米。
const START_X: f32 = -1.0;

/// 四档判据坡度（度）。
const DEGREES: [f32; 4] = [5.0, 15.0, 30.0, 40.0];

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

fn step(
    port: &dyn PhysicsQueryPort,
    state: &mut CharacterState,
    wish: Vec3,
    dt: f32,
) -> MoveOutcome {
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

/// 坡面在 `x` 处的高度（与夹具几何同一口径：过原点沿 +X 上升，坡前平地兜零）。
fn ramp_height(deg: f32, x: f32) -> f32 {
    (x * deg.to_radians().tan()).max(0.0)
}

/// 胶囊半圆落在斜面上时脚底必然浮起的量（米）。
///
/// 解析世界按 `mn.y - surface_y` 判支撑，没有这层；`Rapier` 是真接触，浮起量不可避免。
fn foot_lift(kind: PhysicsKind, deg: f32) -> f32 {
    match kind {
        PhysicsKind::Builtin => 0.0,
        _ => config().radius * (1.0 / deg.to_radians().cos() - 1.0),
    }
}

/// 期望脚底高度：坡面高度 + 半圆浮起量（平地上浮起量为 0）。
fn expect_feet(kind: PhysicsKind, deg: f32, x: f32) -> f32 {
    let h = ramp_height(deg, x);
    if h <= 0.0 {
        0.0
    } else {
        h + foot_lift(kind, deg)
    }
}

/// 斜坡夹具：真后端画旋转踏板，解析世界用倾斜法线 + 逐点高度。
fn ramp_port(kind: PhysicsKind, deg: f32) -> Box<dyn PhysicsQueryPort> {
    match kind {
        PhysicsKind::Builtin => Box::new(World::slope(0.0, deg)),
        _ => {
            let physics: Physics = with_ramp(floor_world(kind), deg, RAMP_HALF_LEN);
            Box::new(physics)
        }
    }
}

/// 一次采样：帧号、脚底位置、当帧是否踩实。
struct Sample {
    frame: usize,
    feet: Vec3,
    grounded: bool,
}

fn walk_ramp(kind: PhysicsKind, deg: f32) -> Vec<Sample> {
    let port = ramp_port(kind, deg);
    let mut state = CharacterState::new(Vec3::new(START_X, DROP_FROM, 0.0));
    for _ in 0..60 {
        step(port.as_ref(), &mut state, Vec3::ZERO, FIXED_DT);
    }
    assert!(
        state.grounded,
        "{kind:?}：{deg}° 起步前没能站稳，判据前提不成立"
    );

    let mut samples = Vec::with_capacity(WALK_FRAMES / SAMPLE_EVERY);
    for frame in 1..=WALK_FRAMES {
        let out = step(port.as_ref(), &mut state, Vec3::X, FIXED_DT);
        if frame % SAMPLE_EVERY == 0 {
            samples.push(Sample {
                frame,
                feet: state.feet,
                grounded: out.grounded,
            });
        }
    }
    samples
}

/// 主判据：每个采样点的 |实测 - 期望| < `RAMP_HEIGHT_TOL`。
fn height_error_within_tol(kind: PhysicsKind, deg: f32) {
    if !kind.is_available() {
        eprintln!("跳过：后端 {kind:?} 未编译进本构建");
        return;
    }
    let samples = walk_ramp(kind, deg);

    // 走过一段坡才有资格谈误差：全程 12 个采样点必须都在坡面上（x > 5 米）。
    let last = samples.last().expect("采样点不应为空");
    assert!(
        last.feet.x > 5.0,
        "{kind:?}：{deg}° 坡在 {}° 上限内，120 帧应走上坡，实际停在 x={}",
        config().max_slope_deg,
        last.feet.x
    );

    let mut worst = 0.0f32;
    for s in &samples {
        let expect = expect_feet(kind, deg, s.feet.x);
        let diff = s.feet.y - expect;
        worst = worst.max(diff.abs());
        assert!(
            diff.abs() < RAMP_HEIGHT_TOL,
            "{kind:?}：{deg}° 坡第 {} 帧高度误差 {diff:+.4} 超过 {RAMP_HEIGHT_TOL}（实测 {:.4} / 期望 {:.4} / x={:.4}）",
            s.frame,
            s.feet.y,
            expect,
            s.feet.x
        );
        assert!(
            s.grounded,
            "{kind:?}：{deg}° 坡第 {} 帧应一直踩实，不能掉下坡（x={:.4}）",
            s.frame, s.feet.x
        );
    }
    eprintln!("{kind:?}：{deg}° 坡 12 个采样点最大误差 {worst:.4} 米");
}

#[test]
fn height_error_within_tol_5_deg_rapier() {
    height_error_within_tol(PhysicsKind::Rapier, DEGREES[0]);
}

#[test]
fn height_error_within_tol_5_deg_analytic() {
    height_error_within_tol(PhysicsKind::Builtin, DEGREES[0]);
}

#[test]
fn height_error_within_tol_15_deg_rapier() {
    height_error_within_tol(PhysicsKind::Rapier, DEGREES[1]);
}

#[test]
fn height_error_within_tol_15_deg_analytic() {
    height_error_within_tol(PhysicsKind::Builtin, DEGREES[1]);
}

#[test]
fn height_error_within_tol_30_deg_rapier() {
    height_error_within_tol(PhysicsKind::Rapier, DEGREES[2]);
}

#[test]
fn height_error_within_tol_30_deg_analytic() {
    height_error_within_tol(PhysicsKind::Builtin, DEGREES[2]);
}

#[test]
fn height_error_within_tol_40_deg_rapier() {
    height_error_within_tol(PhysicsKind::Rapier, DEGREES[3]);
}

#[test]
fn height_error_within_tol_40_deg_analytic() {
    height_error_within_tol(PhysicsKind::Builtin, DEGREES[3]);
}
