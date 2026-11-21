//! B02 地面判据：站在平地上不下沉、不掉离地面、吸附距离内贴地、距离外不硬拉。
//!
//! 这些判据和 B01 的几何恒等式不同 —— 「贴不贴地」是**后端算出来的**：
//! 扫掠方向、接触法线、吸附补探都由 `r3e-physics` 的端口实现决定。只用解析夹具
//! 验等于自己出题自己答，所以每个判据都在真后端（`Rapier` 与 `Builtin`）上各跑一遍。
//!
//! 十条判据 × 两个后端 = 二十条测试函数（`check-tests` 按 crate 字面数测试标记，
//! 连注释里出现的也算，所以正文别把它当普通名词写，否则口径会虚高一条）。

mod common;

use common::*;
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, MoveOutcome,
    FIXED_DT,
};
use r3e_physics::{Physics, PhysicsKind};

/// 竖直容差（米）：1 毫米。底面是平的，超出这个量就是「真的沉了 / 真的浮了」。
const EPS_Y: f32 = 1e-3;

/// 起点离地高度（米）：够高才能让首帧走「下落 → 扫掠命中」，而不是一上来就贴脸重叠。
const DROP_FROM: f32 = 0.5;

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

/// 后端未编译进本构建时整条用例跳过（`--no-default-features` 下只剩 `Builtin`）。
fn usable(kind: PhysicsKind) -> bool {
    let ok = kind.is_available();
    if !ok {
        eprintln!("跳过：后端 {kind:?} 未编译进本构建");
    }
    ok
}

fn step(physics: &Physics, state: &mut CharacterState, wish: Vec3, dt: f32) -> MoveOutcome {
    KinematicSolver.step(
        &config(),
        state,
        MoveIntent {
            wish_dir: wish,
            jump: false,
        },
        dt,
        physics,
    )
}

/// 在平地世界（顶面 y=0）从 `DROP_FROM` 落下并站稳，返回世界与状态。
fn landed(kind: PhysicsKind) -> (Physics, CharacterState) {
    let physics = floor_world(kind);
    let mut state = CharacterState::new(Vec3::new(0.0, DROP_FROM, 0.0));
    for _ in 0..60 {
        step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    }
    assert!(state.grounded, "落地 60 帧后仍悬空：判据前提不成立");
    assert!(
        state.feet.y.abs() < EPS_Y,
        "落地后脚底应在 y≈0，实际 {}",
        state.feet.y
    );
    (physics, state)
}

// ---------------------------------------------------------------- ① 水平面行走不下沉

fn walk_does_not_sink(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let before = state.feet.y;
    for _ in 0..60 {
        step(&physics, &mut state, Vec3::X, FIXED_DT);
    }
    assert!(
        (state.feet.y - before).abs() < EPS_Y,
        "{kind:?}：走 60 帧后脚底漂了 {}（{before} → {}）",
        state.feet.y - before,
        state.feet.y
    );
}

#[test]
fn walk_does_not_sink_rapier() {
    walk_does_not_sink(PhysicsKind::Rapier);
}

#[test]
fn walk_does_not_sink_builtin() {
    walk_does_not_sink(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ② 不掉离地面

fn never_leaves_ground(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    for frame in 0..60 {
        let out = step(&physics, &mut state, Vec3::X, FIXED_DT);
        assert!(
            out.grounded,
            "{kind:?}：第 {frame} 帧掉离地面（脚底 {}）",
            state.feet.y
        );
    }
}

#[test]
fn never_leaves_ground_rapier() {
    never_leaves_ground(PhysicsKind::Rapier);
}

#[test]
fn never_leaves_ground_builtin() {
    never_leaves_ground(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ③ 吸附距离内贴地

fn snaps_within_distance(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let lift = config().snap_distance * 0.9;
    state.teleport(state.feet + Vec3::Y * lift);
    let out = step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    assert!(
        out.grounded,
        "{kind:?}：抬高 {lift} 米（< 吸附距离）后应被吸附回来"
    );
    assert!(
        state.feet.y.abs() < EPS_Y,
        "{kind:?}：吸附后脚底应贴回 y≈0，实际 {}",
        state.feet.y
    );
}

#[test]
fn snaps_within_distance_rapier() {
    snaps_within_distance(PhysicsKind::Rapier);
}

#[test]
fn snaps_within_distance_builtin() {
    snaps_within_distance(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ④ 吸附距离外不硬拉

fn beyond_snap_keeps_falling(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let lift = config().snap_distance * 1.5;
    state.teleport(state.feet + Vec3::Y * lift);
    let first = step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    assert!(
        !first.grounded,
        "{kind:?}：抬高 {lift} 米（> 吸附距离）后不该被硬拉回地面"
    );
    let after_first = state.feet.y;
    let second = step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    assert!(
        !second.grounded && state.feet.y < after_first,
        "{kind:?}：应当继续下落，实际 {after_first} → {}",
        state.feet.y
    );
}

#[test]
fn beyond_snap_keeps_falling_rapier() {
    beyond_snap_keeps_falling(PhysicsKind::Rapier);
}

#[test]
fn beyond_snap_keeps_falling_builtin() {
    beyond_snap_keeps_falling(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑤ 无输入原地不动

fn idle_keeps_position(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let before = state.feet;
    for _ in 0..30 {
        step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    }
    assert!(
        (state.feet - before).length() < EPS_Y,
        "{kind:?}：零输入时角色漂了 {}（{before} → {}）",
        (state.feet - before).length(),
        state.feet
    );
    assert!(state.grounded, "{kind:?}：零输入时不该掉离地面");
}

#[test]
fn idle_keeps_position_rapier() {
    idle_keeps_position(PhysicsKind::Rapier);
}

#[test]
fn idle_keeps_position_builtin() {
    idle_keeps_position(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑥ 低帧率下仍贴地

fn low_fps_stays_grounded(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    // 15 Hz：单帧步长是额定的 4 倍，下落位移与吸附都要跟得上。
    let dt = 1.0 / 15.0;
    for frame in 0..15 {
        let out = step(&physics, &mut state, Vec3::X, dt);
        assert!(out.grounded, "{kind:?}：15 Hz 第 {frame} 帧掉离地面");
    }
    assert!(
        state.feet.y.abs() < EPS_Y,
        "{kind:?}：15 Hz 下脚底应在 y≈0，实际 {}",
        state.feet.y
    );
}

#[test]
fn low_fps_stays_grounded_rapier() {
    low_fps_stays_grounded(PhysicsKind::Rapier);
}

#[test]
fn low_fps_stays_grounded_builtin() {
    low_fps_stays_grounded(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑦ 斜向行走 Y 不变

fn diagonal_keeps_height(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let before = state.feet;
    // `y = 99` 故意给个离谱值：意图的竖直分量必须被丢弃，否则这一帧就飞了。
    for _ in 0..60 {
        step(&physics, &mut state, Vec3::new(1.0, 99.0, 1.0), FIXED_DT);
    }
    assert!(
        (state.feet.y - before.y).abs() < EPS_Y,
        "{kind:?}：斜向行走把脚底带到了 {}（原 {}）",
        state.feet.y,
        before.y
    );
    assert!(
        state.feet.x > before.x + 1.0 && state.feet.z > before.z + 1.0,
        "{kind:?}：斜向应当同时推进 x 与 z，实际 {}",
        state.feet
    );
}

#[test]
fn diagonal_keeps_height_rapier() {
    diagonal_keeps_height(PhysicsKind::Rapier);
}

#[test]
fn diagonal_keeps_height_builtin() {
    diagonal_keeps_height(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑧ dt = 0 幂等

fn zero_dt_is_idempotent(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    let before = state.feet;
    for _ in 0..3 {
        step(&physics, &mut state, Vec3::X, 0.0);
    }
    assert!(
        (state.feet - before).length() < 1e-6,
        "{kind:?}：dt=0 不该移动，实际 {before} → {}",
        state.feet
    );
    assert!(
        state.grounded,
        "{kind:?}：dt=0 应沿用上一帧的落地结论，不该改成悬空"
    );
}

#[test]
fn zero_dt_is_idempotent_rapier() {
    zero_dt_is_idempotent(PhysicsKind::Rapier);
}

#[test]
fn zero_dt_is_idempotent_builtin() {
    zero_dt_is_idempotent(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑨ 巨大 dt 不穿透

fn huge_dt_does_not_tunnel(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (physics, mut state) = landed(kind);
    // 卡顿帧：0.5 秒。控制器按 `MAX_DT` 裁剪，单帧竖直位移 2 m/s × 0.1 s = 0.2 m，
    // 扫掠应当命中地面，而不是「跨过地板继续往下」。
    let out = step(&physics, &mut state, Vec3::X, 0.5);
    assert!(out.grounded, "{kind:?}：巨大 dt 单帧后仍应踩实");
    assert!(
        state.feet.y.abs() < EPS_Y,
        "{kind:?}：脚底穿到地面以下（{}）",
        state.feet.y
    );
}

#[test]
fn huge_dt_does_not_tunnel_rapier() {
    huge_dt_does_not_tunnel(PhysicsKind::Rapier);
}

#[test]
fn huge_dt_does_not_tunnel_builtin() {
    huge_dt_does_not_tunnel(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ⑩ 被墙挡住时仍着地

fn blocked_still_grounded(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let physics = with_wall(floor_world(kind));
    let mut state = CharacterState::new(Vec3::new(0.0, DROP_FROM, 0.0));
    for _ in 0..60 {
        step(&physics, &mut state, Vec3::ZERO, FIXED_DT);
    }
    let mut blocked = false;
    for _ in 0..120 {
        let out = step(&physics, &mut state, Vec3::X, FIXED_DT);
        blocked |= out.blocked;
    }
    assert!(blocked, "{kind:?}：撞墙 120 帧却没报 blocked");
    assert!(
        state.grounded,
        "{kind:?}：贴着墙停下时仍应踩在地面上（脚底 {}）",
        state.feet.y
    );
    assert!(
        state.feet.x < 0.5,
        "{kind:?}：撞墙后不该穿到墙里，x = {}",
        state.feet.x
    );
}

#[test]
fn blocked_still_grounded_rapier() {
    blocked_still_grounded(PhysicsKind::Rapier);
}

#[test]
fn blocked_still_grounded_builtin() {
    blocked_still_grounded(PhysicsKind::Builtin);
}
