//! B05 可玩性回归 1：整条路线走一遍 —— 平地 8 米 → 15° 斜坡 6 米 → 三级 0.3 米台阶
//! → 收尾平台 → 1 米超限台阶（挡停）。
//!
//! 与 B02/B03 的差别是**判据的单位**：前面是「一堵墙 / 一级台阶」的瞬时结论，
//! 这里是「走完一段路之后你还在不在路上」—— 落脚高度、贴地帧的占比、被挡的位置。
//! 这条路线上任何一处退化（吸附少补一拍、抬升试探漏一层、坡度裁决误判），
//! 都会以「末点高度对不上」或「贴地帧掉到 9 成以下」的形式暴露出来。
//!
//! ## 几何从哪来
//!
//! 路线写进 `assets/verify/character_ramp.r3e.json`，同一份夹具既喂 `cargo xtask
//! scene-verify`（静态层体检：不该报 `character.*`），也在这里被读进真后端。夹具只有
//! 一份，就不会出现「测试里那条路」和「验收时那条路」两条路各说各话。
//!
//! ## 为什么斜坡只有 Rapier 跑全程
//!
//! `BuiltinBackend` 的碰撞模型是轴对齐 AABB，**忽略旋转**（B03 实测）。夹具里的斜坡
//! 必须带旋转，否则它就是一块水平凸台。所以：
//!
//! * 全程路线（含斜坡）只在 `Rapier` 上跑；
//! * `Builtin` 上跑同一份夹具的**台阶段**（从坡顶平台起，三级台阶到收尾平台），
//!   这段全是轴对齐盒子，两个后端口径一致；
//! * 斜坡上的贴地判据在 `Builtin` 侧走解析世界 [`World::slope`]（它直接给出倾斜法线），
//!   与 B03 的既定口径一致，不因为后端表达不了就把坡地判据整段放弃。
//!
//! ## 留证
//!
//! [`route_log_is_parseable`] 把路点写成 `assets/verify/evidence/character_walk.log`：
//! 出问题时不用重跑，直接看哪一帧开始掉高。

mod common;

use std::fs;
use std::path::PathBuf;

use common::*;
use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, FIXED_DT,
};
use r3e_core::{ObjectDesc, Scene};
use r3e_physics::port::PhysicsQueryPort;
use r3e_physics::{BodyDesc, ColliderShape, Physics, PhysicsKind};

/// 路线夹具（与 `scene-verify` 共用同一份）。
const ROUTE_JSON: &str = include_str!("../../../assets/verify/character_ramp.r3e.json");

/// 坡顶平台顶面高度（米）＝ 2 × 3 米 × tan(15°)。
const RAMP_TOP_Y: f32 = 1.5529;
/// 三级 0.3 米台阶之后的收尾平台顶面高度。
const STEP_TOP_Y: f32 = 2.4529;
/// 一级台阶的抬升量（米）。
const STEP_RISE: f32 = 0.3;
/// 1 米超限台阶的左立面 x（米）。
const OVER_LIMIT_X: f32 = 14.9956;
/// 竖直容差（米）：1 厘米。全程判据比单步判据宽一档 —— 累积了 300 多帧的浮点。
const EPS_Y: f32 = 1e-2;

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

/// 后端未编译进本构建时整条用例跳过。
fn usable(kind: PhysicsKind) -> bool {
    let ok = kind.is_available();
    if !ok {
        eprintln!("跳过：后端 {kind:?} 未编译进本构建");
    }
    ok
}

// ---------------------------------------------------------------- 夹具 → 真后端

/// 按夹具装配一个物理世界（地形矩形装成顶面 y=0 的盒，物件按 position/rotation/scale 装盒）。
///
/// 这里刻意**不复用** `scene-verify` 的装配：那边用 `BuiltinBackend` 的解析查询，
/// 差的是「谁算接触」；几何来自同一份 JSON，装配代码各写一份反而更容易看出夹具写错。
fn route_physics(kind: PhysicsKind) -> (Scene, Physics) {
    let mut scene = Scene::from_json_str(ROUTE_JSON).expect("路线夹具必须合法");
    let mut physics = physics_of(kind);

    let patches: Vec<(String, [f32; 2], [f32; 2])> = scene
        .terrain
        .iter()
        .map(|t| (t.name.clone(), t.origin, t.size))
        .collect();
    for (name, origin, size) in patches {
        let half = Vec3::new(size[0] * 0.5, 0.5, size[1] * 0.5);
        let center = Vec3::new(origin[0] + half.x, -half.y, origin[1] + half.z);
        let id = scene
            .spawn(ObjectDesc {
                name: format!("terrain_{name}"),
                position: Some(center.to_array()),
                ..Default::default()
            })
            .expect("spawn 地形代理盒");
        physics.create_body(
            id,
            BodyDesc {
                shape: ColliderShape::Box,
                half_extents: half,
                position: center,
                ..Default::default()
            },
        );
    }

    let boxes: Vec<_> = scene
        .iter()
        .filter(|n| {
            n.physics
                .as_ref()
                .map(|p| p.body == "static")
                .unwrap_or(false)
        })
        .map(|n| {
            (
                n.id,
                n.name.clone(),
                n.transform.position,
                n.transform.scale,
                n.transform.rotation,
            )
        })
        .collect();
    assert!(!boxes.is_empty(), "夹具里必须声明静态盒");
    for (id, _name, position, scale, rotation) in boxes {
        physics.create_body(
            id,
            BodyDesc {
                shape: ColliderShape::Box,
                half_extents: scale * 0.5,
                position,
                rotation,
                ..Default::default()
            },
        );
    }

    (scene, physics)
}

/// 夹具里的出生点（角色起点）。
fn start_of(scene: &Scene) -> Vec3 {
    let s = scene.first_spawn().expect("夹具必须声明出生点");
    Vec3::from(s.position)
}

// ---------------------------------------------------------------- 走

/// 一帧的路点。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Point {
    x: f32,
    y: f32,
    grounded: bool,
    step_up: f32,
    blocked: bool,
}

fn step_once(
    port: &dyn PhysicsQueryPort,
    state: &mut CharacterState,
    wish: Vec3,
) -> r3e_character::MoveOutcome {
    KinematicSolver.step(
        &config(),
        state,
        MoveIntent {
            wish_dir: wish,
            jump: false,
        },
        FIXED_DT,
        port,
    )
}

/// 从 `start` 向 +X 走 `frames` 帧（先原地落 60 帧站稳，站稳的帧不记路点）。
fn walk(port: &dyn PhysicsQueryPort, start: Vec3, frames: usize) -> Vec<Point> {
    let mut state = CharacterState::new(start);
    for _ in 0..60 {
        step_once(port, &mut state, Vec3::ZERO);
    }
    let mut points = Vec::with_capacity(frames);
    for _ in 0..frames {
        let out = step_once(port, &mut state, Vec3::X);
        points.push(Point {
            x: state.feet.x,
            y: state.feet.y,
            grounded: out.grounded,
            step_up: out.step_up,
            blocked: out.blocked,
        });
    }
    points
}

/// 贴地帧占比。
fn grounded_ratio(points: &[Point]) -> f32 {
    let n = points.iter().filter(|p| p.grounded).count();
    n as f32 / points.len() as f32
}

fn last(points: &[Point]) -> Point {
    *points.last().expect("路点不能为空")
}

// ---------------------------------------------------------------- ① 夹具有效性

#[test]
fn fixture_declares_full_route() {
    let (scene, physics) = route_physics(PhysicsKind::Builtin);
    let names: Vec<&str> = scene.iter().map(|n| n.name.as_str()).collect();
    for want in ["ramp", "step1", "step2", "step3", "platform_end"] {
        assert!(names.contains(&want), "夹具缺 {want}：{names:?}");
    }
    assert!(
        names.contains(&"step_over_limit"),
        "夹具必须留一段超限台阶（否则「挡停」没有观察对象）：{names:?}"
    );
    assert!(!scene.terrain.is_empty(), "夹具缺地形，导航烘焙会失败");

    let ramp = scene.iter().find(|n| n.name == "ramp").expect("夹具缺斜坡");
    assert!(
        ramp.transform
            .rotation
            .angle_between(glam::Quat::IDENTITY)
            .abs()
            > 1e-3,
        "斜坡必须带旋转，否则它就是一块水平凸台（15° 的判据全靠这个旋转）"
    );

    // 装配出来的刚体数 = 地形代理盒 + 所有静态盒。
    let want = scene.terrain.len() + scene.iter().filter(|n| n.physics.is_some()).count();
    assert_eq!(
        physics.body_count(),
        want,
        "夹具声明的盒子数与装进后端的刚体数对不上"
    );

    let start = start_of(&scene);
    assert!(
        (start.x + 8.0).abs() < 1e-3 && start.y.abs() < 1e-3,
        "起点应在平地 x=-8 处，实际 {start:?}"
    );
}

// ---------------------------------------------------------------- ② 全程（Rapier）

/// 走够到被超限台阶挡停的帧数：27 米名义位移 / (4.5 m/s ÷ 60) ≈ 360 帧。
const ROUTE_FRAMES: usize = 360;

fn full_route() -> Option<Vec<Point>> {
    if !usable(PhysicsKind::Rapier) {
        return None;
    }
    let (scene, physics) = route_physics(PhysicsKind::Rapier);
    let start = start_of(&scene);
    Some(walk(&physics, start, ROUTE_FRAMES))
}

#[test]
fn route_ends_on_end_platform_rapier() {
    let Some(points) = full_route() else {
        return;
    };
    let end = last(&points);
    assert!(
        end.x > OVER_LIMIT_X - config().radius - EPS_Y,
        "角色应一路走到超限台阶立面（x≈{}）才停，实际 x={}",
        OVER_LIMIT_X - config().radius,
        end.x
    );
    assert!(
        (end.y - STEP_TOP_Y).abs() < EPS_Y,
        "走完全程脚底应在收尾平台 {}，实际 {}",
        STEP_TOP_Y,
        end.y
    );
    // 坡顶：翻过斜坡那一刻必须真的上到 1.55 米，而不是贴着一块水平凸台蹭过去。
    let on_top = points
        .iter()
        .filter(|p| p.x > 6.5 && p.x < 8.0)
        .map(|p| p.y)
        .fold(f32::MIN, f32::max);
    assert!(
        (on_top - RAMP_TOP_Y).abs() < 0.05,
        "坡顶平台上脚底应在 {RAMP_TOP_Y} 附近，实际 {on_top}"
    );
}

#[test]
fn route_stays_grounded_rapier() {
    let Some(points) = full_route() else {
        return;
    };
    let all = grounded_ratio(&points);
    assert!(
        all >= 0.95,
        "整条路线的贴地帧占比 {}，低于 0.95 —— 有段落靠自由落体硬撑",
        all
    );
    // 分段看：斜坡段是最容易「一路弹跳下去」的地方。
    let slope: Vec<Point> = points
        .iter()
        .copied()
        .filter(|p| p.x > -0.5 && p.x < 5.8)
        .collect();
    assert!(slope.len() > 30, "斜坡段的采样帧太少：{}", slope.len());
    assert!(
        grounded_ratio(&slope) >= 0.95,
        "斜坡段贴地帧占比 {}，低于 0.95",
        grounded_ratio(&slope)
    );
}

#[test]
fn route_climbs_three_steps_rapier() {
    let Some(points) = full_route() else {
        return;
    };
    // 斜坡每帧只抬 2 厘米（4.5/60 × tan15°），只有台阶才会一次性抬 0.3 米。
    let big: Vec<f32> = points
        .iter()
        .map(|p| p.step_up)
        .filter(|v| *v > STEP_RISE * 0.8)
        .collect();
    assert!(
        big.len() >= 3,
        "应至少报出 3 次 0.3 米级抬升，实际 {} 次：{big:?}",
        big.len()
    );
    let max = big.iter().copied().fold(0.0f32, f32::max);
    assert!(
        (max - STEP_RISE).abs() < 0.05,
        "单次抬升上限应约 {STEP_RISE}，实际 {max}"
    );
    assert!(
        max <= config().step_height + 1e-3,
        "抬升量 {} 超过抬升能力 {}：控制器不该越权",
        max,
        config().step_height
    );
}

#[test]
fn route_stops_at_over_limit_step_rapier() {
    let Some(points) = full_route() else {
        return;
    };
    let end = last(&points);
    assert!(
        end.x < OVER_LIMIT_X + EPS_Y,
        "1 米台阶超过 {} 米抬升能力，不许穿过去，实际 x={}",
        config().step_height,
        end.x
    );
    assert!(
        points.iter().any(|p| p.blocked),
        "被超限台阶挡住时必须报 blocked，整条路线一次都没报"
    );
}

#[test]
fn route_never_sinks_rapier() {
    let Some(points) = full_route() else {
        return;
    };
    let min_y = points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    assert!(min_y > -EPS_Y, "全程脚底不该掉到地面以下，最低 {min_y}");
    // 高度只许「原地或上升」：整条路线是单调的上坡，任何一次向下多于 2 厘米
    // 都意味着某一帧被吸附/抬升逻辑抖下去了。
    let mut drops = 0usize;
    for w in points.windows(2) {
        if w[1].y < w[0].y - 0.02 {
            drops += 1;
        }
    }
    assert_eq!(drops, 0, "上坡路线不该出现向下跳变，实际 {drops} 次");
}

#[test]
fn route_is_deterministic_rapier() {
    if !usable(PhysicsKind::Rapier) {
        return;
    }
    let (scene_a, physics_a) = route_physics(PhysicsKind::Rapier);
    let a = walk(&physics_a, start_of(&scene_a), 120);
    let (scene_b, physics_b) = route_physics(PhysicsKind::Rapier);
    let b = walk(&physics_b, start_of(&scene_b), 120);

    assert_eq!(a.len(), b.len(), "两次跑的路点数应一致");
    for (i, (p, q)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            (p.x.to_bits(), p.y.to_bits(), p.grounded),
            (q.x.to_bits(), q.y.to_bits(), q.grounded),
            "第 {i} 帧两次跑的结果不同：{p:?} vs {q:?}"
        );
    }
}

// ---------------------------------------------------------------- ③ 台阶段（双后端）

/// 从坡顶平台起、走三级台阶到收尾平台：9 米路走 90 帧（6.75 米）。
const STEP_FRAMES: usize = 90;

fn walk_steps_only(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let (_scene, physics) = route_physics(kind);
    let points = walk(&physics, Vec3::new(6.5, RAMP_TOP_Y, 0.0), STEP_FRAMES);
    let end = last(&points);
    assert!(
        end.x > 12.6,
        "{kind:?}：应走上收尾平台（x>12.6），实际 x={}",
        end.x
    );
    assert!(
        (end.y - STEP_TOP_Y).abs() < EPS_Y,
        "{kind:?}：三级台阶后脚底应为 {STEP_TOP_Y}，实际 {}",
        end.y
    );
    assert!(
        grounded_ratio(&points) >= 0.95,
        "{kind:?}：台阶段的贴地帧占比 {}，低于 0.95",
        grounded_ratio(&points)
    );
}

#[test]
fn walks_three_steps_rapier() {
    walk_steps_only(PhysicsKind::Rapier);
}

#[test]
fn walks_three_steps_builtin() {
    walk_steps_only(PhysicsKind::Builtin);
}

// ---------------------------------------------------------------- ④ 斜坡贴地（双后端）

/// 15° 坡上走 1 秒：全程贴地，且脚底跟着坡面走（每帧抬 ≈2 厘米）。
fn slope_walk(kind: PhysicsKind) {
    if !usable(kind) {
        return;
    }
    let physics = with_ramp(floor_world(kind), 15.0, 3.0);
    let points = walk(&physics, Vec3::new(-2.0, 0.0, 0.0), 60);
    let end = last(&points);
    let want = end.x * 15.0f32.to_radians().tan();
    assert!(
        grounded_ratio(&points) >= 0.95,
        "{kind:?}：斜坡上贴地帧占比 {}，低于 0.95",
        grounded_ratio(&points)
    );
    assert!(
        (end.y - want).abs() < 0.05,
        "{kind:?}：坡上 1 秒后脚底应约 {want}（x={} 处的坡面），实际 {}",
        end.x,
        end.y
    );
    assert!(end.x > 1.0, "{kind:?}：应确实走上坡（x>1），实际 {}", end.x);
}

#[test]
fn walks_ramp_rapier() {
    slope_walk(PhysicsKind::Rapier);
}

/// `Builtin` 表达不了旋转体，斜坡改走解析世界（槽面法线与真后端同口径）。
#[test]
fn walks_ramp_builtin_analytical() {
    let world = World::slope(0.0, 15.0);
    let points = walk(&world, Vec3::new(-2.0, 0.0, 0.0), 60);
    let end = last(&points);
    assert!(
        grounded_ratio(&points) >= 0.95,
        "解析坡上贴地帧占比 {}，低于 0.95",
        grounded_ratio(&points)
    );
    assert!(
        (end.y - world.surface_y(end.x)).abs() < EPS_Y,
        "解析坡上脚底应贴合坡面 {}，实际 {}",
        world.surface_y(end.x),
        end.y
    );
}

// ---------------------------------------------------------------- ⑤ 留证

/// 路点日志：一行一帧，字段固定，能被人直接读、也能被脚本切。
#[test]
fn route_log_is_parseable() {
    let Some(points) = full_route() else {
        return;
    };
    let mut text = String::from("# r3e-character 路线留证（M2-B05）\n");
    text.push_str(
        "route=flat 8m -> 15deg ramp 6m -> 3x0.3m steps -> platform -> 1m over-limit step\n",
    );
    text.push_str(&format!("frames={}\n", points.len()));
    for (i, p) in points.iter().enumerate() {
        text.push_str(&format!(
            "frame={i} x={:.4} y={:.4} grounded={} step_up={:.4} blocked={}\n",
            p.x,
            p.y,
            u8::from(p.grounded),
            p.step_up,
            u8::from(p.blocked)
        ));
    }
    let end = last(&points);
    text.push_str(&format!(
        "summary end_x={:.4} end_y={:.4} grounded_ratio={:.4} step_frames={}\n",
        end.x,
        end.y,
        grounded_ratio(&points),
        points
            .iter()
            .filter(|p| p.step_up > STEP_RISE * 0.8)
            .count()
    ));

    // 自解析一遍：日志说得出跟内存里一致的东西，才配叫「留证」。
    let data: Vec<&str> = text.lines().filter(|l| l.starts_with("frame=")).collect();
    assert_eq!(
        data.len(),
        points.len(),
        "日志帧数与路点数对不上，切片脚本会静默读少"
    );
    let first = data[0];
    for key in ["frame=", "x=", "y=", "grounded=", "step_up=", "blocked="] {
        assert!(first.contains(key), "日志缺字段 {key}：{first}");
    }
    let last_line = *data.last().expect("至少一帧");
    let logged_y: f32 = last_line
        .split(' ')
        .find_map(|kv| kv.strip_prefix("y="))
        .expect("末帧缺 y")
        .parse()
        .expect("y 必须是数");
    assert!(
        (logged_y - end.y).abs() < 1e-3,
        "日志末帧 y={logged_y} 与实测 {} 不符",
        end.y
    );

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/verify/evidence/character_walk.log");
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // 写不进去不算失败（CI 可能只读），但要在输出里说清楚，避免「以为留了证其实没有」。
    match fs::write(&path, &text) {
        Ok(()) => eprintln!("留证已写入 {}", path.display()),
        Err(e) => eprintln!("留证未写入（{}）：{e}", path.display()),
    }
}
