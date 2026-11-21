//! 运动学求解器 7 条判据：走多远、被什么挡住、能不能跨台阶、落不落地、跳不跳得起。

mod common;

use common::{Box3, World};
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, FIXED_DT,
};
use r3e_physics::port::PhysicsQueryPort;

/// 落地一帧：让状态从「悬空」变成「踩实」，后续判据才有干净的起点。
fn settle(world: &World, cfg: &CharacterConfig, st: &mut CharacterState) {
    KinematicSolver.step(cfg, st, MoveIntent::default(), FIXED_DT, world);
    assert!(st.grounded, "夹具前提：静止一帧就该踩实");
}

fn fast() -> CharacterConfig {
    CharacterConfig {
        move_speed: 10.0, // 单帧 0.1667 m，够跨过 0.15 m 的接触距离
        ..CharacterConfig::default()
    }
}

#[test]
fn free_walk_moves_full_step() {
    let cfg = CharacterConfig::default(); // move_speed 4.5
    let world = World::flat(0.0);
    let mut st = CharacterState::new(Vec3::ZERO);
    settle(&world, &cfg, &mut st);

    let out = KinematicSolver.step(
        &cfg,
        &mut st,
        MoveIntent {
            wish_dir: Vec3::X,
            jump: false,
        },
        FIXED_DT,
        &world,
    );

    let expect = cfg.move_speed * FIXED_DT; // 4.5 / 60 = 0.075
    assert!((st.feet.x - expect).abs() < 1e-5, "平地应走满一步");
    assert!((st.feet.y).abs() < 1e-5, "平地不该抬高或下陷");
    assert!(out.grounded && !out.blocked);
    assert_eq!(out.step_up, 0.0);
}

#[test]
fn wall_stops_horizontal() {
    let cfg = fast();
    let world = World::flat(0.0).with(Box3::new(
        Vec3::new(-5.0, 0.0, 0.5),
        Vec3::new(5.0, 2.0, 2.0),
    ));
    let mut st = CharacterState::new(Vec3::ZERO);
    settle(&world, &cfg, &mut st);

    let out = KinematicSolver.step(
        &cfg,
        &mut st,
        MoveIntent {
            wish_dir: Vec3::Z,
            jump: false,
        },
        FIXED_DT,
        &world,
    );

    assert!(out.blocked && st.blocked, "两米高的墙必须挡住");
    // 停在墙面前一个半径 + 一点余量处。
    let wall = 0.5 - cfg.radius;
    assert!(st.feet.z <= wall, "不许穿墙");
    assert!(
        (st.feet.z - wall).abs() < 5e-3,
        "应贴着墙面停下，实际 {}",
        st.feet.z
    );
    assert!((st.feet.y).abs() < 1e-4, "撞墙不该顺带爬高");
}

#[test]
fn low_ledge_steps_up() {
    let cfg = fast(); // step_height 0.35
    let world = World::flat(0.0).with(Box3::new(
        Vec3::new(-5.0, 0.0, 0.5),
        Vec3::new(5.0, 0.3, 3.0),
    ));
    let mut st = CharacterState::new(Vec3::ZERO);
    settle(&world, &cfg, &mut st);

    let out = KinematicSolver.step(
        &cfg,
        &mut st,
        MoveIntent {
            wish_dir: Vec3::Z,
            jump: false,
        },
        FIXED_DT,
        &world,
    );

    assert!(!out.blocked, "0.3 m 台阶在 0.35 m 抬升能力内");
    assert!((st.feet.y - 0.3).abs() < 5e-3, "应站上台阶顶");
    assert!((out.step_up - 0.3).abs() < 5e-3, "抬升量应如实上报");
    assert!(out.grounded, "站上台阶后必须报踩实");
}

#[test]
fn tall_ledge_blocks() {
    let cfg = fast(); // step_height 0.35 < 0.8
    let world = World::flat(0.0).with(Box3::new(
        Vec3::new(-5.0, 0.0, 0.5),
        Vec3::new(5.0, 0.8, 3.0),
    ));
    let mut st = CharacterState::new(Vec3::ZERO);
    settle(&world, &cfg, &mut st);

    let out = KinematicSolver.step(
        &cfg,
        &mut st,
        MoveIntent {
            wish_dir: Vec3::Z,
            jump: false,
        },
        FIXED_DT,
        &world,
    );

    assert!(out.blocked, "0.8 m 台阶超过抬升能力，必须挡住");
    assert!((st.feet.y).abs() < 1e-4, "挡住时不许自己长高");
    assert_eq!(out.step_up, 0.0);
}

#[test]
fn fall_lands_on_ground() {
    let cfg = CharacterConfig::default();
    let world = World::flat(0.0);
    let mut st = CharacterState::new(Vec3::new(0.0, 0.01, 0.0));
    st.velocity_y = -1.0;
    assert!(!st.grounded);

    let out = KinematicSolver.step(&cfg, &mut st, MoveIntent::default(), FIXED_DT, &world);

    assert!(out.grounded, "本帧下落量已越过地面，必须判落地");
    assert!(st.feet.y.abs() < 1e-4, "落地后脚底贴地");
    assert_eq!(st.velocity_y, 0.0, "落地即清竖直速度");
}

#[test]
fn jump_leaves_ground() {
    let cfg = CharacterConfig::default();
    let world = World::flat(0.0);
    let mut st = CharacterState::new(Vec3::ZERO);
    settle(&world, &cfg, &mut st);

    let out = KinematicSolver.step(
        &cfg,
        &mut st,
        MoveIntent {
            wish_dir: Vec3::ZERO,
            jump: true,
        },
        FIXED_DT,
        &world,
    );

    let rise = cfg.jump_speed * FIXED_DT; // 6 / 60 = 0.1
    assert!((st.feet.y - rise).abs() < 1e-4, "起跳当帧应上升 0.1 m");
    assert!(!out.grounded, "起跳后不是踩实状态");
    assert!(st.velocity_y > 0.0, "起跳后仍在上冲");
}

#[test]
fn steep_slope_is_not_grounded() {
    let cfg = CharacterConfig::default(); // max_slope 45°
    let world = World::slope(0.0, 60.0); // 60° 坡：几何上贴住了，但站不住
    let mut st = CharacterState::new(Vec3::ZERO);

    KinematicSolver.step(&cfg, &mut st, MoveIntent::default(), FIXED_DT, &world);

    assert!(!st.grounded, "超过坡度上限的面不算落地");
    assert_eq!(st.ground_normal, Vec3::ZERO, "站不住时不给地面法线");
    assert!(world.port_caps().capsule, "夹具应声明支持胶囊查询");
}
