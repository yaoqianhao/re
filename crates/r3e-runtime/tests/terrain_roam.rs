//! M2-C07 可玩性回归 2：在**跨块地形**上走一遍。
//!
//! B05 走的是「盒子拼出来的路线」，判据落在台阶与斜坡；这里走的是「地形高度场本身」，
//! 判据换成了三件只有分块地形才会暴露的事：
//!
//! 1. **16 个派生块拼成一张连续可走的场** —— 块划分、LOD 全都只服务渲染，
//!    角色脚下踩的必须是同一张双线性高度面，接缝两边逐点相等；
//! 2. **坡度裁决跟着地形走** —— 15° 爬得上去、50° 滑下来，限值 45°
//!    （[`CharacterConfig`]）在两个方向上都被真地形验证一次；
//! 3. **物理侧拿到的场与渲染侧看到的面是同一张** —— 终点高度直接跟
//!    [`r3e_terrain::sample_height_at`] 对账，差超过 1 厘米就说明两套采样漂了。
//!
//! # 夹具为什么放 `labs` 不放 `verify`
//!
//! `assets/verify/` 下的场景全部在 `cargo xtask scene-verify` 的登记表里，而
//! `scene-verify` 的判据是「静态层体检不该报 `character.*`」。这块夹具是**地形**
//! 案例（斜坡上本就该报陡、本就没有静态盒），塞进 verify 会让那些断言变成噪声，
//! 也会牵动回放金标。所以它落在 `assets/labs/terrain_roam/`，与 B05 的
//! `character_stairs` 同一待遇：自包含、只被本文件 include。
//!
//! # 双后端
//!
//! 高度场通道两个后端都实现了（`C04`），所以这里的判据**不分后端**跑两遍，
//! 只在最后比一次终点差。这与 B05「斜坡只有 Rapier 能跑」的处境不同：
//! 那时差的是旋转体，这时高度场是后端无关的解析层。

use glam::Vec3;
use r3e_character::{
    CharacterConfig, CharacterSolver, CharacterState, KinematicSolver, MoveIntent, FIXED_DT,
};
use r3e_core::Scene;
use r3e_physics::{Physics, PhysicsConfig, PhysicsKind};
use r3e_runtime::terrain_field::heightfield_of;
use r3e_terrain::{sample_height_at, BlockGrid, DEFAULT_BLOCK_SIZE};

/// 地形夹具（`assets/labs/terrain_roam/`）。
const ROAM_JSON: &str = include_str!("../../../assets/labs/terrain_roam/terrain_roam.r3e.json");

/// 200 × 200 米的地形按 50 米切块 → 4 × 4 = 16 块。
const BLOCKS: [u32; 2] = [4, 4];
/// 平地高度（米）。
const FLAT_Y: f32 = 0.0;
/// 15° 缓坡中段（x=-50）的高度：20 米 × tan15° 的一半。
const SLOPE15_MID_Y: f32 = 2.679;
/// 坡顶平台高度（米）。
const TOP_Y: f32 = 29.193;
/// 竖直容差（米）：终点对账用 1 厘米。
const EPS_Y: f32 = 1e-2;

fn config() -> CharacterConfig {
    CharacterConfig::default()
}

/// 后端未编译进本构建时整条用例跳过（`Builtin` 永远可用）。
fn usable(kind: PhysicsKind) -> bool {
    if !kind.is_available() {
        eprintln!("跳过：后端 {kind:?} 未编译进本构建");
        return false;
    }
    true
}

/// 夹具 → 真后端：地形不铺代理盒，直接把 patch 铺成高度场挂到后端上。
fn world(kind: PhysicsKind) -> (Scene, Physics) {
    // doc_gate 规则 4：上层（`crates/r3e-runtime/`，含 tests）不得直接 `Scene::from_json_str`，
    // 只走 `r3e_doc::parse_scene`。此处与上层其它调用方同口径。
    let scene = r3e_doc::parse_scene(ROAM_JSON).expect("地形夹具必须合法");
    let mut physics = Physics::new();
    physics
        .apply_config(PhysicsConfig::default().with_enabled(true).with_kind(kind))
        .expect("后端应可选中");
    let field = heightfield_of(&scene.terrain).expect("地形必须能铺成高度场");
    physics.set_heightfield(Some(field));
    (scene, physics)
}

fn start_of(scene: &Scene) -> Vec3 {
    Vec3::from(scene.first_spawn().expect("夹具必须声明出生点").position)
}

/// 从 `start` 沿 +X 走 `frames` 帧；落地站稳的 60 帧不记路点。
fn walk(physics: &Physics, start: Vec3, frames: usize) -> Vec<Vec3> {
    let mut state = CharacterState::new(start);
    for _ in 0..60 {
        step(physics, &mut state, Vec3::ZERO);
    }
    let mut path = Vec::with_capacity(frames);
    for _ in 0..frames {
        step(physics, &mut state, Vec3::X);
        path.push(state.feet);
    }
    path
}

fn step(physics: &Physics, state: &mut CharacterState, wish: Vec3) -> r3e_character::MoveOutcome {
    KinematicSolver.step(
        &config(),
        state,
        MoveIntent {
            wish_dir: wish,
            jump: false,
        },
        FIXED_DT,
        physics,
    )
}

/// 走 `frames` 帧并同时记下每帧是否贴地。
fn walk_tracking(physics: &Physics, start: Vec3, frames: usize) -> (Vec<Vec3>, Vec<bool>) {
    let mut state = CharacterState::new(start);
    for _ in 0..60 {
        step(physics, &mut state, Vec3::ZERO);
    }
    let mut path = Vec::with_capacity(frames);
    let mut grounded = Vec::with_capacity(frames);
    for _ in 0..frames {
        let out = step(physics, &mut state, Vec3::X);
        path.push(state.feet);
        grounded.push(out.grounded);
    }
    (path, grounded)
}

/// 走完 `distance` 米需要的帧数（按控制器自己的步长算，不写死 4.5 m/s）。
fn frames_for(distance: f32) -> usize {
    let per_frame = config().move_speed * FIXED_DT;
    (distance / per_frame).round().max(1.0) as usize
}

// ---------------------------------------------------------------- ① 夹具 → 16 块

#[test]
fn fixture_yields_sixteen_blocks_and_lands_on_ground() {
    let (scene, physics) = world(PhysicsKind::Builtin);
    assert_eq!(scene.terrain.len(), 1, "夹具应只有一块地表");

    let patch = &scene.terrain[0];
    let grid = BlockGrid::new(
        [
            patch.origin[0],
            patch.origin[1],
            patch.origin[0] + patch.size[0],
            patch.origin[1] + patch.size[1],
        ],
        DEFAULT_BLOCK_SIZE,
    )
    .expect("200×200 的地形必须能切块");
    assert_eq!(
        grid.dims(),
        BLOCKS,
        "200 米按 {} 米切，应是 4×4=16 块",
        DEFAULT_BLOCK_SIZE
    );

    let field = physics.heightfield().expect("后端上必须挂着高度场");
    assert!(
        field.covers(-100.0, -100.0) && field.covers(99.0, 99.0),
        "高度场必须覆盖整块地形"
    );

    let (path, grounded) = walk_tracking(&physics, start_of(&scene), 30);
    assert!(
        grounded[0],
        "出生点落稳第一帧就该 grounded=true，实际 {:?}",
        path.first()
    );
    assert!(
        (path[0].y - FLAT_Y).abs() < EPS_Y,
        "出生点脚底应在平地 {FLAT_Y}，实际 {}",
        path[0].y
    );
}

// ---------------------------------------------------------------- ② 走得到路点 1

#[test]
fn reaches_first_waypoint_on_flat() {
    let (scene, physics) = world(PhysicsKind::Builtin);
    let start = start_of(&scene);
    let wp1 = waypoint(&scene, "wp1_flat");
    let path = walk(&physics, start, frames_for(wp1.x - start.x));
    let end = *path.last().expect("至少要有一帧");
    assert!(
        (end.x - wp1.x).abs() < 0.2,
        "应停在路点 1 的 x={} 附近（误差 <0.2），实际 {}",
        wp1.x,
        end.x
    );
    assert!(
        (end.y - FLAT_Y).abs() < EPS_Y,
        "平地上脚底应保持 {FLAT_Y}，实际 {}",
        end.y
    );
}

// ---------------------------------------------------------------- ③ 15° 缓坡全程贴地

#[test]
fn climbs_fifteen_degree_slope_staying_grounded() {
    let (_scene, physics) = world(PhysicsKind::Builtin);
    // 从坡脚前的平地起步（x∈[-100,-60] 是 0 高度的平地），一路走上 15° 段。
    let from = Vec3::new(-64.0, 0.0, 20.0);
    let (path, grounded) = walk_tracking(&physics, from, frames_for(16.0));
    let ratio = grounded.iter().filter(|g| **g).count() as f32 / grounded.len() as f32;
    assert!(
        ratio >= 0.95,
        "15° 缓坡贴地帧占比 {ratio}，低于 0.95 —— 角色在坡上弹跳而不是走"
    );
    let mid = *path.last().expect("至少有帧");
    assert!(
        mid.x > -52.0,
        "16 米应走过坡中段（x>-52），实际 x={}",
        mid.x
    );
    assert!(
        mid.y > SLOPE15_MID_Y * 0.8,
        "坡上脚底应随坡面抬高（>{}），实际 {}",
        SLOPE15_MID_Y * 0.8,
        mid.y
    );
}

// ---------------------------------------------------------------- ④ 50° 陡坡上不去

/// 45° 是「站得住」的限值，50° 段爬上去的判据不是「腾空」，而是**推进失败**：
/// 名义 6.75 米的 wish 距离里，实际水平位移要小到只剩零头。
#[test]
fn cannot_climb_fifty_degree_slope() {
    let (_scene, physics) = world(PhysicsKind::Builtin);
    let from = Vec3::new(-40.0, 5.358, 20.0);
    let frames = 90;
    let path = walk(&physics, from, frames);
    let end = *path.last().expect("至少有帧");
    let nominal = config().move_speed * FIXED_DT * frames as f32;
    assert!(
        end.x - from.x < nominal * 0.3,
        "50° 超过 {}° 坡度限值，90 帧名义 {nominal} 米，实际只该推进零头，实走 {} 米",
        config().max_slope_deg,
        end.x - from.x
    );
    assert!(
        end.y >= from.y - 0.1,
        "被陡坡挡住时不该掉到坡脚以下：起点 {} → {}",
        from.y,
        end.y
    );
}

// ---------------------------------------------------------------- ⑤ 跨块接缝不卡顿

#[test]
fn crosses_block_seam_without_stalling() {
    let (_scene, physics) = world(PhysicsKind::Builtin);
    // x=0 是块边界之一，两侧都是坡顶平台。
    let path = walk(&physics, Vec3::new(-5.0, TOP_Y, 20.0), 120);
    let mut stalls = 0usize;
    let mut jumps = 0usize;
    for w in path.windows(2) {
        let d = w[1] - w[0];
        if d.length() < 1e-4 {
            stalls += 1;
        }
        if d.y.abs() > 0.1 {
            jumps += 1;
        }
    }
    assert_eq!(
        stalls, 0,
        "跨块接缝时不该有任何一帧原地不动，实际 {stalls} 帧"
    );
    assert_eq!(jumps, 0, "接缝上不该出现垂直跳变，实际 {jumps} 次");
    let end = *path.last().expect("至少有帧");
    assert!(end.x > 0.0, "应确实跨过接缝（x>0），实际 {}", end.x);
}

// ---------------------------------------------------------------- ⑥ 终点高度对账

#[test]
fn top_height_matches_terrain_sample() {
    let (scene, physics) = world(PhysicsKind::Builtin);
    let wp5 = waypoint(&scene, "wp5_top");
    let path = walk(&physics, Vec3::new(45.0, TOP_Y, 20.0), frames_for(8.0));
    let end = *path.last().expect("至少有帧");
    let want = sample_height_at(&scene.terrain, end.x, end.z).expect("终点必须在地形内");
    assert!(
        (end.y - want).abs() < EPS_Y,
        "终点脚底 {} 与地形采样 {want} 相差超过 {EPS_Y} —— 物理与渲染漂了两套采样",
        end.y
    );
    assert!(
        (end.y - wp5.y).abs() < 0.1,
        "路点 5 标在 {TOP_Y}，实测 {}",
        end.y
    );
}

// ---------------------------------------------------------------- ⑦ 无 panic / 无 NaN

#[test]
fn walk_never_produces_nan() {
    let (scene, physics) = world(PhysicsKind::Builtin);
    let path = walk(&physics, start_of(&scene), frames_for(30.0));
    assert!(!path.is_empty(), "必须走出一段路");
    for (i, p) in path.iter().enumerate() {
        assert!(
            p.is_finite(),
            "第 {i} 帧位置不是有限值：{p:?} —— 高度场里出现了 NaN 或 inf"
        );
    }
}

// ---------------------------------------------------------------- ⑧ 双后端终点差

#[test]
fn both_backends_agree_on_end_point() {
    let mut ends = Vec::new();
    for kind in [PhysicsKind::Builtin, PhysicsKind::Rapier] {
        if !usable(kind) {
            continue;
        }
        let (_scene, physics) = world(kind);
        let path = walk(&physics, Vec3::new(0.0, TOP_Y, 20.0), frames_for(50.0));
        let end = *path.last().expect("至少有帧");
        eprintln!("{kind:?} 终点 {end:?}");
        ends.push(end);
    }
    if ends.len() < 2 {
        eprintln!("只剩一个后端可用，跳过终点对比");
        return;
    }
    let d = (ends[1] - ends[0]).length();
    assert!(
        d < 0.5,
        "两个后端在同一片地形上走了 50 米后差了 {d} 米（{:?} vs {:?}）—— 超过 0.5 米就不是同一个手感",
        ends[0],
        ends[1]
    );
}

// ---------------------------------------------------------------- ⑨ 回放确定性

#[test]
fn same_seed_replays_identically() {
    let (scene_a, physics_a) = world(PhysicsKind::Builtin);
    let (scene_b, physics_b) = world(PhysicsKind::Builtin);
    let a = walk(&physics_a, start_of(&scene_a), frames_for(20.0));
    let b = walk(&physics_b, start_of(&scene_b), frames_for(20.0));
    assert_eq!(a.len(), b.len(), "两次跑的帧数应一致");
    for (i, (p, q)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()),
            (q.x.to_bits(), q.y.to_bits(), q.z.to_bits()),
            "第 {i} 帧两次跑的结果不同：{p:?} vs {q:?}"
        );
    }
    assert_eq!(
        r3e_doc::scene_hash(&scene_a),
        r3e_doc::scene_hash(&scene_b),
        "同一份夹具加载两次，场景指纹必须一致"
    );
}

// ---------------------------------------------------------------- ⑩ 金标不受影响

#[test]
fn fixture_does_not_touch_golden_scenes() {
    // 夹具落在 labs（自包含、不进 verify 登记表），金标场景集是 assets/verify 下那批。
    assert!(
        ROAM_JSON.contains("\"terrain_roam\""),
        "include 出来的应当是地形漫游夹具"
    );
    let expect = include_str!("../../../assets/verify/expect.json");
    assert!(
        !expect.contains("terrain_roam"),
        "地形夹具不该进 verify 登记表 —— 进去就会牵动回放金标 385482eb033d2964"
    );
}

// ---------------------------------------------------------------- 工具

fn waypoint(scene: &Scene, name: &str) -> Vec3 {
    scene
        .iter()
        .find(|n| n.name == name)
        .map(|n| n.transform.position)
        .unwrap_or_else(|| panic!("夹具缺路点 {name}"))
}
