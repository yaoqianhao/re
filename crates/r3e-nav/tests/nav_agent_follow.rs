//! `M2·D06` 可玩性回归 3：**headless 航点跟随**（判据 ① / ② / ⑤）。
//!
//! D03 证明的是「按一条**给定的**折线走不切角、不浮空」；这里换成玩家真实遇到的
//! 形态：**目标一直在动，代理每次只看得见目标此刻的位置**。两者只差一件事——
//! D03 的折线是事前算好的，D06 的折线每 0.1 s 重算一次，于是「重规划会不会把代理
//! 带进墙、会不会飘离地面、会不会在边界前原地打转」这三件事才第一次被检验。
//!
//! # 跟随策略（刻意做得和真实玩家一样笨）
//!
//! 1. **目标**沿预设航点匀速前进（[`TARGET_SPEED`]），不吃地形、不做避障——它只是
//!    一个「要去的地方」；
//! 2. **代理**每 [`REPLAN_EVERY`] 帧用 `nav.path` + `smooth_path` 重规划一条
//!    「当前位置 → 目标当前位置」的路，再按 [`AGENT_SPEED`] 沿它推进一帧；
//! 3. 代理比目标快一倍，稳态下**贴着目标走**，航点判据（≤ 1 m）因此成立：
//!    重规划间隔 0.1 s × 1.5 m/s = 0.15 m 的滞后远小于 1 m 容差。
//!
//! 这段策略在两个测试文件里各有一份（`r3e-nav` 侧做几何判据，`r3e-runtime` 侧做
//! 跨后端回归）。集成测试之间不能互相 import，故有意重复；策略一动两处必须同改，
//! 这本身就是「判据口径一致」的写法。
//!
//! # 为什么不是「预先算好整条路」
//!
//! 预计算会把「目标移动」从回归里抹掉：代理实际执行的仍是 D03 那条折线，`nav.path`
//! 的实时性与 `smooth_path` 的二次稳定性都不会被覆盖。重规划每帧都跑，等于顺带把
//! D03 的平滑在移动目标上又验了几百遍。
//!
//! # 回退条款（`M2_D_PLAN` §9 风险栏）
//!
//! 若墙钟过慢，步长放大到 `1/30 s` 并改用「等效位移」断言。本卡实测 debug 构建下
//! 六条用例合计 ≈ 2 s，**未触发**回退，故保留 `1/60 s`。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{polyline_length, smooth_path, Nav, NavAgent, NavConfig, NavProduct, PathOutcome};
use r3e_physics::{Physics, PhysicsKind};
use std::path::PathBuf;

/// 游戏内固定步长（秒）：1 s 走 60 帧。
const DT: f32 = 1.0 / 60.0;
/// 代理速度（米每秒）：默认身位 0.4 m 的人在平地上小跑。
const AGENT_SPEED: f32 = 3.0;
/// 目标速度（米每秒）：刻意做慢，让「代理贴得上」是策略性质而不是运气。
const TARGET_SPEED: f32 = 1.5;
/// 重规划间隔（帧）= 0.1 s。
const REPLAN_EVERY: usize = 6;
/// 判据 ① 的单段时限（游戏内秒）。
const LEG_BUDGET_S: f32 = 40.0;
/// 判据 ① 的到点容差（米）。
const ARRIVE_EPS: f32 = 1.0;
/// 判据 ② 的贴地容差（米）。
const GROUND_EPS: f32 = 1e-2;
/// 判据 ② 的身位容差（米）。
const BODY_EPS: f32 = 1e-3;
/// 判据 ⑤ 的断连边界容差（米）。
const DISCONNECT_EPS: f32 = 2.0;
/// 失控保护：单条用例最多推进的帧数（200 s 游戏时间）。
const MAX_FRAMES: usize = 12_000;

// ---------------------------------------------------------------- 夹具

fn flat_scene(size: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [size, size]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 「先升后降」的山脊：与 `tests/agent.rs` 同一块夹具（每格高度都不同）。
fn ridge_scene(size: f32, rise: f32) -> Scene {
    let mut scene = Scene::new();
    let samples = size as u32 + 1;
    let half = samples / 2;
    let mut row: Vec<f32> = (0..=half).map(|k| rise * k as f32 / half as f32).collect();
    row.extend((1..=half).map(|k| rise * (half - k) as f32 / half as f32));
    let mut heights = row.clone();
    heights.extend_from_slice(&row);
    let mut patch = TerrainPatch::flat("ridge", [0.0, 0.0], [size, 4.0]);
    patch.resolution = [samples, 2];
    patch.heights = heights;
    scene.terrain.push(patch);
    scene.update_world_transforms();
    scene
}

/// XZ 包围盒：`([min_x, min_z], [max_x, max_z])`。
type Bbox = ([f32; 2], [f32; 2]);

/// 一块立方体障碍，返回它的 XZ 包围盒（与 `tests/agent.rs` 同口径）。
fn add_box(scene: &mut Scene, name: &str, center: [f32; 2], scale: [f32; 2]) -> Bbox {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some([center[0], 1.5, center[1]]),
            scale: Some([scale[0], 3.0, scale[1]]),
            ..Default::default()
        })
        .expect("障碍可生成");
    scene.update_world_transforms();
    let half = [scale[0] * 0.5, scale[1] * 0.5];
    (
        [center[0] - half[0], center[1] - half[1]],
        [center[0] + half[0], center[1] + half[1]],
    )
}

/// 中间留 4 m 缺口的墙（`z ∈ [6, 10]` 可过），返回两段墙的包围盒。
///
/// 缺口开得比 `2 × agent_radius` 宽得多，是为了让判据 ② 的「身位」在**穿缺口**
/// 这个动作上真的被压一次：格子膨胀后剩下的可走宽度只有 4 − 2×0.4 = 3.2 m。
fn wall_with_gap(scene: &mut Scene, x: f32) -> Vec<Bbox> {
    vec![
        add_box(scene, "wall_a", [x, 3.0], [0.2, 6.0]),
        add_box(scene, "wall_b", [x, 13.0], [0.2, 6.0]),
    ]
}

/// 四壁封闭的小房间：内部是一块「地理上可走、但外部到不了」的区域。
fn sealed_room(scene: &mut Scene, center: [f32; 2], half: f32) -> Vec<Bbox> {
    let [cx, cz] = center;
    let span = half * 2.0 + 0.4;
    vec![
        add_box(scene, "room_w", [cx - half, cz], [0.2, span]),
        add_box(scene, "room_e", [cx + half, cz], [0.2, span]),
        add_box(scene, "room_n", [cx, cz - half], [span, 0.2]),
        add_box(scene, "room_s", [cx, cz + half], [span, 0.2]),
    ]
}

// ---------------------------------------------------------------- 几何小工具

fn flat(a: Vec3, b: Vec3) -> f32 {
    ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt()
}

fn dist_to_box_xz(p: Vec3, b: Bbox) -> f32 {
    let dx = (b.0[0] - p.x).max(p.x - b.1[0]).max(0.0);
    let dz = (b.0[1] - p.z).max(p.z - b.1[1]).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

fn min_dist_to_boxes(p: Vec3, boxes: &[Bbox]) -> f32 {
    boxes
        .iter()
        .map(|b| dist_to_box_xz(p, *b))
        .fold(f32::INFINITY, f32::min)
}

/// 折线各顶点的累计长度（`cum[0] == 0`）。
fn cumulative(route: &[Vec3]) -> Vec<f32> {
    let mut cum = Vec::with_capacity(route.len());
    cum.push(0.0);
    for w in route.windows(2) {
        let last = *cum.last().expect("至少一个点");
        cum.push(last + flat(w[0], w[1]));
    }
    cum
}

/// 折线上行进 `dist` 米后的位置。
fn point_at(route: &[Vec3], cum: &[f32], dist: f32) -> Vec3 {
    for k in 0..cum.len() - 1 {
        if dist <= cum[k + 1] {
            let span = cum[k + 1] - cum[k];
            let t = if span > 1e-6 {
                ((dist - cum[k]) / span).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let (a, b) = (route[k], route[k + 1]);
            return Vec3::new(
                a.x + (b.x - a.x) * t,
                a.y + (b.y - a.y) * t,
                a.z + (b.z - a.z) * t,
            );
        }
    }
    *route.last().expect("非空航线")
}

// ---------------------------------------------------------------- 跟随策略

/// 从 `from` 到 `to` 规划一条平滑折线，并把代理放上去（起点可不在首顶点）。
///
/// 不可达一律 panic：本文件的夹具里目标**始终**在可走区，`Found` 之外的任何结局
/// 都说明夹具或烘焙出了问题，不该被静默吞掉。
fn plan(nav: &Nav, from: Vec3, to: Vec3) -> NavAgent {
    let points = match nav.path(from, to) {
        PathOutcome::Found(points) => smooth_path(nav.grid(), &points).points,
        other => panic!("跟随目标必须可达（{from:?} → {to:?}）：{other:?}"),
    };
    NavAgent::at(&points, from)
}

/// 一段航点的观测结果。
struct Leg {
    /// 从「目标离开上一航点」到「代理进入 1 m 容差」的游戏内秒数。
    seconds: f32,
    /// 本段内代理离该航点最近的一次距离（米）。
    error: f32,
    /// 是否在时限内进入过容差。
    reached: bool,
}

/// 一次跟随的全程观测。
struct FollowLog {
    legs: Vec<Leg>,
    /// 逐帧代理位置（首帧为 `start`）。
    trail: Vec<Vec3>,
    /// 逐帧目标位置。
    target_trail: Vec<Vec3>,
    /// 全程 `|y − 地面|` 的最大值。
    max_y_err: f32,
    /// 全程离障碍的最小距离。
    min_clearance: f32,
    /// 落在不可走格上的帧数。
    unwalkable: usize,
    /// 被判偏离（`AgentStep::off_path`）的帧数。
    off_path: usize,
    /// 实际推进的游戏内秒数。
    seconds: f32,
}

/// 让代理跟随目标走完 `route`，返回全程观测。
fn follow(nav: &Nav, start: Vec3, route: &[Vec3], boxes: &[Bbox]) -> FollowLog {
    let cum = cumulative(route);
    let total = *cum.last().expect("非空航线");
    let mut legs: Vec<Leg> = (1..route.len())
        .map(|_| Leg {
            seconds: 0.0,
            error: f32::INFINITY,
            reached: false,
        })
        .collect();

    let mut pos = start;
    let mut target_dist = 0.0f32;
    let mut t = 0.0f32;
    let mut leg = 0usize;
    let mut leg_start_t = 0.0f32;
    let mut agent: Option<NavAgent> = None;
    let mut log = FollowLog {
        legs: Vec::new(),
        trail: vec![start],
        target_trail: vec![point_at(route, &cum, 0.0)],
        max_y_err: 0.0,
        min_clearance: f32::INFINITY,
        unwalkable: 0,
        off_path: 0,
        seconds: 0.0,
    };

    let mut frames = 0usize;
    loop {
        if frames >= MAX_FRAMES || legs.iter().all(|l| l.reached) {
            break;
        }
        // 失控保护：单段耗时超过两倍时限就不再空转（由断言报错，而不是死循环）。
        if t - leg_start_t > LEG_BUDGET_S * 2.0 {
            break;
        }

        // ① 目标沿预设航点匀速推进；到终点后停在原地等代理追上。
        target_dist = (target_dist + TARGET_SPEED * DT).min(total);
        let target = point_at(route, &cum, target_dist);

        // ② 代理每 `REPLAN_EVERY` 帧按「当前位置 → 目标当前位置」重规划。
        if frames % REPLAN_EVERY == 0 || agent.is_none() {
            agent = Some(plan(nav, pos, target));
        }
        let step = agent
            .as_mut()
            .expect("上一行刚规划过")
            .advance(nav.grid(), DT, AGENT_SPEED);
        if step.off_path {
            log.off_path += 1;
        }
        pos = agent.as_ref().expect("代理已装配").pos();
        t += DT;
        frames += 1;
        log.trail.push(pos);
        log.target_trail.push(target);

        // ③ 全程约束逐帧记账（判据 ②）。
        let ground = nav.grid().ground_at(pos).expect("代理不许走网格外");
        log.max_y_err = log.max_y_err.max((pos.y - ground).abs());
        if !nav.is_walkable(pos) {
            log.unwalkable += 1;
        }
        log.min_clearance = log.min_clearance.min(min_dist_to_boxes(pos, boxes));

        // ④ 航点判定：代理离「当前目标航点」≤ 1 m 即记一次抵达。
        let wp = route[leg + 1];
        let d = flat(pos, wp);
        legs[leg].error = legs[leg].error.min(d);
        if !legs[leg].reached && d <= ARRIVE_EPS {
            legs[leg].reached = true;
            legs[leg].seconds = t - leg_start_t;
        }

        // ⑤ 目标越过后进入下一段（最后一段没有下一段可进）。
        if leg + 1 < legs.len() && target_dist + 1e-4 >= cum[leg + 1] {
            leg += 1;
            leg_start_t = t;
        }
    }

    log.legs = legs;
    log.seconds = t;
    log
}

/// 一条 4 段航线：起点 → 墙前 → 缺口北侧 → 缺口南侧 → 墙后深处。
///
/// 第 3 段（`(13, 8) → (26, 8)`）必须从 `z ∈ [6, 10]` 的缺口穿过去；代理从上一段
/// 赶来时直线方向会被 `wall_a` 挡住，于是 `nav.path` 必须自己绕——判据 ⑤ 的
/// 「停在能走到的边界」在同一块地形上有了正向对照。
fn gap_route() -> Vec<Vec3> {
    vec![
        Vec3::new(4.0, 0.0, 3.0),
        Vec3::new(13.0, 0.0, 3.0),
        Vec3::new(13.0, 0.0, 8.0),
        Vec3::new(26.0, 0.0, 8.0),
        Vec3::new(26.0, 0.0, 22.0),
    ]
}

// ---------------------------------------------------------------- 判据 ① 航点抵达（2 条）

/// 判据 ①-a：目标沿 4 个预设航点走，代理 40 s 内抵达每一个，误差 ≤ 1 m。
#[test]
fn agent_reaches_every_moving_waypoint_within_budget() {
    let mut scene = flat_scene(32.0);
    let boxes = wall_with_gap(&mut scene, 18.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let route = gap_route();
    let start = Vec3::new(4.0, 0.0, 3.0);

    let log = follow(&nav, start, &route, &boxes);

    assert_eq!(log.legs.len(), 4, "4 段航点必须逐段判定");
    for (k, leg) in log.legs.iter().enumerate() {
        assert!(
            leg.reached,
            "判据1：第 {} 个航点没在 {} s 内进去（最近一次 {} m）",
            k + 1,
            LEG_BUDGET_S,
            leg.error
        );
        assert!(
            leg.seconds <= LEG_BUDGET_S,
            "判据1：第 {} 个航点用了 {} s，超 {} s 时限",
            k + 1,
            leg.seconds,
            LEG_BUDGET_S
        );
        assert!(
            leg.error <= ARRIVE_EPS,
            "判据1：第 {} 个航点误差 {} m，超 1 m",
            k + 1,
            leg.error
        );
    }
    // 「累计误差 ≤ 1 m」的另一半含义：误差不跨段累加——每段独立从 1 m 内进点，
    // 全程最大误差就是单段最大误差，这里再钉一次上界。
    let worst = log.legs.iter().map(|l| l.error).fold(0.0f32, f32::max);
    assert!(worst <= ARRIVE_EPS, "累计误差 {worst} m 超 1 m");

    // 目标真的走完了全程（循环在「最后一个航点被抵达」处停下，此刻目标可能还差
    // 一两帧的路，所以量的是轨迹长度而不是逐位终点），代理轨迹长度与航线同量级
    // （没抄近路、没原地画圈）。
    let target_walked = polyline_length(&log.target_trail);
    assert!(
        target_walked >= polyline_length(&route) * 0.9,
        "目标轨迹 {target_walked} m 明显短于航线 {} m，说明航点没走完",
        polyline_length(&route)
    );
    let walked = polyline_length(&log.trail);
    assert!(
        walked >= polyline_length(&route) * 0.9,
        "代理轨迹 {walked} m 明显短于航线 {} m，说明没真的跟着走",
        polyline_length(&route)
    );
    assert_eq!(log.off_path, 0, "目标始终在可走区，代理不该被判偏离");
}

/// 判据 ①-b：目标**中途换道**（切到另一组预设航点），代理从当前位置续行仍逐点抵达。
///
/// 这一条盯的是重规划本身：换道瞬间代理正处在上一段的任意位置，`NavAgent::at` 要把
/// 当前段挪到最近一段、`remaining` 要从当前位置重算——续行逻辑一退化，代理就会朝
/// 「已经走过的航点」回头，误差与耗时同时爆掉。
#[test]
fn agent_resumes_follow_when_target_switches_route() {
    let mut scene = flat_scene(32.0);
    let boxes = wall_with_gap(&mut scene, 18.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");

    // 第一组：从西侧沿墙往北走，代理会跟到墙前 x ≈ 13 一带。
    let first = vec![
        Vec3::new(4.0, 0.0, 3.0),
        Vec3::new(13.0, 0.0, 3.0),
        Vec3::new(13.0, 0.0, 8.0),
    ];
    let head = follow(&nav, Vec3::new(4.0, 0.0, 3.0), &first, &boxes);
    assert!(head.legs[0].reached, "第一组第一段本该走完");
    let handover = *head.trail.last().expect("有轨迹");
    assert!(
        flat(handover, Vec3::new(4.0, 0.0, 3.0)) > 3.0,
        "换道点必须在半路上（{handover:?}），否则这一步什么都没考"
    );

    // 第二组：从换道点继续，目标改走缺口 → 墙后 → 东北角。
    let second = vec![
        handover,
        Vec3::new(13.0, 0.0, 8.0),
        Vec3::new(26.0, 0.0, 8.0),
        Vec3::new(28.0, 0.0, 20.0),
    ];
    let tail = follow(&nav, handover, &second, &boxes);

    for (k, leg) in tail.legs.iter().enumerate() {
        assert!(
            leg.reached,
            "判据1：换道后第 {} 个航点没在时限内抵达",
            k + 2
        );
        assert!(
            leg.seconds <= LEG_BUDGET_S,
            "判据1：换道后第 {} 个航点用了 {} s",
            k + 2,
            leg.seconds
        );
        assert!(
            leg.error <= ARRIVE_EPS,
            "判据1：换道后第 {} 个航点误差 {} m",
            k + 2,
            leg.error
        );
    }
    // 换道后不许回头：x 单调不减（目标一直往东走，代理不该「先回起点」）。
    let mut prev = handover.x;
    for p in &tail.trail[1..] {
        assert!(
            p.x >= prev - 1.0,
            "判据1：换道后代理反向走回 x={} → {}（续行把段号挪错了）",
            prev,
            p.x
        );
        prev = p.x;
    }
    assert_eq!(tail.off_path, 0, "换道后也不该被判偏离");
}

// ---------------------------------------------------------------- 判据 ② 全程约束（2 条）

/// 判据 ②-a：全程落在可走格，且离障碍 ≥ `agent_radius − 1e-3`（含穿缺口那一段）。
#[test]
fn follow_keeps_body_width_and_walkable_cells_every_frame() {
    let mut scene = flat_scene(32.0);
    let boxes = wall_with_gap(&mut scene, 18.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let radius = nav.grid().config().agent_radius;
    let route = gap_route();

    let log = follow(&nav, Vec3::new(4.0, 0.0, 3.0), &route, &boxes);

    assert_eq!(
        log.unwalkable, 0,
        "判据2：{} 帧落在不可走格",
        log.unwalkable
    );
    assert!(
        log.min_clearance >= radius - BODY_EPS,
        "判据2：全程最近只到障碍 {} m，要求 ≥ {} m",
        log.min_clearance,
        radius - BODY_EPS
    );
    // 轨迹必须真的横穿缺口，否则「离墙够远」可能只是没靠近过墙。
    let west = log.trail.iter().filter(|p| p.x < 18.0).count();
    let east = log.trail.iter().filter(|p| p.x > 18.0).count();
    assert!(
        west > 0 && east > 0,
        "轨迹没有横穿缺口（西 {west} 帧 / 东 {east} 帧）"
    );
    // 缺口处必须两头都离墙够远：墙线上的那一帧必然落在 z ∈ (6, 10) 内。
    let crossing = log
        .trail
        .iter()
        .find(|p| (p.x - 18.0).abs() < radius)
        .expect("穿缺口时必然有帧跨在墙线上");
    assert!(
        crossing.z > 6.0 && crossing.z < 10.0,
        "判据2：穿墙点 z={} 不在缺口 [6, 10] 内，属于穿墙",
        crossing.z
    );
}

/// 判据 ②-b：山脊上全程 `|y − 地面| ≤ 1 cm`（贴地），且 y 真的爬升过。
#[test]
fn follow_tracks_ground_within_one_centimeter_over_ridge() {
    let scene = ridge_scene(32.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let route = vec![
        Vec3::new(3.0, 0.0, 2.0),
        Vec3::new(12.0, 0.0, 2.0),
        Vec3::new(20.0, 0.0, 2.0),
        Vec3::new(29.0, 0.0, 2.0),
    ];

    let log = follow(&nav, Vec3::new(3.0, 0.0, 2.0), &route, &[]);

    assert!(
        log.max_y_err <= GROUND_EPS,
        "判据2：全程 |y − 地面| 最大 {} m，超 1 cm",
        log.max_y_err
    );
    assert_eq!(log.unwalkable, 0, "判据2：山脊上不该有帧落在不可走格");
    // 夹具必须真的让 y 爬升过，否则「贴地」在平地上是白给的。
    let ys: Vec<f32> = log.trail.iter().map(|p| p.y).collect();
    let span = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max)
        - ys.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(span > 1.0, "山脊夹具必须让 y 真的爬升过（跨度 {span} m）");
    for leg in &log.legs {
        assert!(leg.reached, "判据1：山脊上的航点也得逐点抵达");
    }
}

// ---------------------------------------------------------------- 判据 ③ 双后端 + 判据 ④ CLI 复现（1 条）

/// 造一个指定后端的物理实例（与 `tests/product.rs` 同口径：**先开总开关、再选后端**）。
fn physics_with(kind: PhysicsKind) -> Physics {
    let mut physics = Physics::new();
    physics.set_enabled(true);
    physics
        .set_kind(kind)
        .unwrap_or_else(|e| panic!("后端 {} 应当可用：{e}", kind.name()));
    physics
}

/// 当前构建里编译进来的**真实**后端（`Null` 装不上地形场，不算）。
fn real_backends() -> Vec<PhysicsKind> {
    PhysicsKind::ALL
        .into_iter()
        .filter(|k| *k != PhysicsKind::Null && k.is_available())
        .collect()
}

/// 从固定出生点跑完整段跟随后的终点。
fn follow_end(nav: &Nav, route: &[Vec3], boxes: &[Bbox]) -> Vec3 {
    *follow(nav, Vec3::new(4.0, 0.0, 3.0), route, boxes)
        .trail
        .last()
        .expect("有轨迹")
}

/// `nav.bake --path` 指向的那个文件。
fn nav_product_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("r3e_d06_nav_follow_{}.navmesh", std::process::id()));
    p
}

/// 判据 ③ + ④：同一段跟随在「双后端烘焙」与「产物文件往返」两条通路上给出同一个终点。
///
/// 把两条判据合在一条用例里，是因为它们问的是**同一个问题**：给定同一份场景，换一条
/// 通路（换物理后端 / 中间过一遍 `.navmesh` 文件）之后，`nav.path` 与逐帧跟随还认不
/// 认得出是同一件事。拆成两条只会得到两段几乎一样的断言，故障隔离也没有收益。
///
/// # 判据 ④「CLI 复现」在这条用例里的边界
///
/// `r3e-cli` 只依赖协议层（`xtask/dep_rules.json` 钉死），是个纯 RPC 客户端；仓库里
/// 没有常驻宿主进程，集成测试也不该为了跑一条用例去起服务。所以这里复现的是 CLI 两跳
/// 命令 `nav.bake --path <p>` → `nav.path --from .. --to ..` 的**数据通路**：产物字节
/// 落到 `--path` 指向的文件、再从该文件读回、再在装回来的网格上走同一段跟随、`path`
/// 逐点对账。这正是两跳之间唯一的载体；宿主层（`nav_ops`）的导出/导入往返已在 D04 的
/// `tests/nav_ops.rs` 里锁过，本用例只补「同一条命令换条通路，答案不变」这一层。
///
/// # `state_hash` 的口径
///
/// 判据 ③ 原文要 `state_hash` 相同（与 C07 一致）。`state_hash` 是 `r3e-doc` 层的
/// 口径，`r3e-nav` 不依赖 `r3e-doc`；为了取一个哈希硬拉一条依赖不划算。这里改用它
/// 的上游口径**更强的版本**：`product_hex`（逐格位相等）+ `input_digest`（场景几何
/// 的构建指纹）+ `cfg_digest` 三项全等——逐格位相等蕴含 `state_hash` 相等，反向不
/// 成立，所以这是收紧而不是放松。`state_hash` 本体的跨后端对账在 C07
/// `tests/terrain_roam.rs::both_backends_agree_on_end_point` 里已经做过一次。
#[test]
fn follow_end_point_matches_across_backends_and_product_round_trip() {
    let mut scene = flat_scene(32.0);
    let boxes = wall_with_gap(&mut scene, 18.0);
    let cfg = NavConfig::default();
    let route = gap_route();

    // ① 基准：本地烘焙（不经后端）。
    let base = Nav::bake_with(&scene, cfg.clone()).expect("本地烘焙成功");
    let base_end = follow_end(&base, &route, &boxes);
    assert!(
        polyline_length(&route) > 20.0,
        "夹具航线必须够长，否则终点对账没有分辨力（{} m）",
        polyline_length(&route)
    );

    // ② 判据 ③：每个真实后端各烘一遍，产物与本地逐位相等，终点差 < 0.5 m。
    let kinds = real_backends();
    assert!(
        kinds.len() >= 2,
        "判据3：本构建只编译了 {} 个真实后端（{kinds:?}），双后端回归无从谈起",
        kinds.len()
    );
    let mut ends: Vec<(String, Vec3)> = vec![("local".to_string(), base_end)];
    for kind in &kinds {
        let mut physics = physics_with(*kind);
        let nav = Nav::bake_with_backend(&scene, cfg.clone(), &mut physics).expect("后端烘焙成功");
        assert_eq!(nav.backend(), Some(kind.name()), "判据3：后端名要贴上");
        assert_eq!(
            nav.product_hex(),
            base.product_hex(),
            "判据3：{} 烘出来的产物与本地不一致（逐格位相等被破坏）",
            kind.name()
        );
        assert_eq!(
            nav.input_digest(),
            base.input_digest(),
            "判据3：场景指纹不许随后端变"
        );
        assert_eq!(nav.cfg_digest(), base.cfg_digest(), "判据3：配置指纹不许变");
        ends.push((kind.name().to_string(), follow_end(&nav, &route, &boxes)));
    }
    for (name, end) in &ends {
        let d = flat(*end, base_end);
        assert!(d < 0.5, "判据3：{name} 的终点与基准差 {d} m，超 0.5 m");
        assert!(
            d < 1e-3,
            "判据3：{name} 的终点与基准差 {d} m——同一张网格上不该有任何偏差"
        );
    }
    // 夹具得真的让代理动过，否则「终点相同」是白给的。
    assert!(
        flat(base_end, Vec3::new(4.0, 0.0, 3.0)) > 15.0,
        "代理必须真的走到了墙后（终点 {base_end:?}）"
    );

    // ③ 判据 ④：产物字节 → 文件（`nav.bake --path`）→ 读回（`nav.path`）→ 同一段跟随。
    let path = nav_product_path();
    std::fs::write(&path, base.product().to_bytes()).expect("产物必须能落盘");
    let bytes = std::fs::read(&path).expect("产物必须能读回");
    let product = NavProduct::from_bytes(&bytes).expect("产物字节必须能解析");
    assert_eq!(
        product.product_hash(),
        base.product_hash(),
        "判据4：往返后产物指纹漂了"
    );
    let reimported = Nav::from_product(product).expect("产物必须能装回网格");
    let reimported_end = follow_end(&reimported, &route, &boxes);
    let drift = flat(reimported_end, base_end);
    assert!(
        drift < 0.5,
        "判据4：经产物文件往返后终点差 {drift} m，超 0.5 m"
    );
    assert!(
        drift < 1e-6,
        "判据4：同一份产物过一遍文件后终点偏了 {drift} m"
    );

    // 两跳命令的 from / to 在本文件里也真有对应物：同一对起终点在两块网格上必须
    // 给出**逐点相同**的路径，这才是「命令输出与测试内断言一致」。
    let from = Vec3::new(4.0, 0.0, 3.0);
    let to = route[3];
    let a = match base.path(from, to) {
        PathOutcome::Found(points) => points,
        other => panic!("判据4：基准网格上必须可达：{other:?}"),
    };
    let b = match reimported.path(from, to) {
        PathOutcome::Found(points) => points,
        other => panic!("判据4：装回来的网格上也必须可达：{other:?}"),
    };
    assert_eq!(a.len(), b.len(), "判据4：往返前后路径点数不一致");
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x, y, "判据4：往返前后路径点不一致：{x:?} vs {y:?}");
    }
    std::fs::remove_file(&path).expect("清理临时产物");
}

// ---------------------------------------------------------------- 判据 ⑤ 不可达停断（1 条）

/// 判据 ⑤：目标移进封闭房间（可走但外部到不了），代理停在断连边界而非抖动或穿墙。
#[test]
fn agent_stops_at_disconnect_boundary_when_target_unreachable() {
    let mut scene = flat_scene(32.0);
    let target = Vec3::new(22.0, 0.0, 22.0);
    let boxes = sealed_room(&mut scene, [target.x, target.z], 2.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let radius = nav.grid().config().agent_radius;
    let start = Vec3::new(4.0, 0.0, 4.0);

    // 前置：目标本身站得住（可走），但外部走不进去 —— 才是真正的 `Unreachable`。
    assert!(nav.is_walkable(target), "房间内部必须仍是一块可走区域");
    match nav.path(start, target) {
        PathOutcome::Unreachable { .. } => {}
        other => panic!("夹具应构造出「分属不同连通块」：{other:?}"),
    }

    // 断连边界：从起点能走到、且离目标最近的那一点。
    let disc = nav
        .disconnect_point(start, target)
        .expect("不可达必然存在断连边界");
    assert!(
        flat(disc, target) > DISCONNECT_EPS,
        "断连边界离目标 {} m，夹具没把目标关严",
        flat(disc, target)
    );

    // ① 代理沿「起点 → 断连边界」走过去，全程守可走与身位（不穿墙）。
    let mut agent = plan(&nav, start, disc);
    let mut trail = vec![agent.pos()];
    let mut steps = 0usize;
    while !agent.arrived() && steps < MAX_FRAMES {
        let step = agent.advance(nav.grid(), DT, AGENT_SPEED);
        assert!(!step.off_path, "通往断连边界的路本就该可走");
        trail.push(agent.pos());
        steps += 1;
    }
    assert!(agent.arrived(), "必须能走到断连边界");
    for p in &trail {
        assert!(nav.is_walkable(*p), "判据5：代理不许踏进不可走格：{p:?}");
        let d = min_dist_to_boxes(*p, &boxes);
        assert!(
            d >= radius - BODY_EPS,
            "判据5：代理在 {p:?} 处离墙只有 {d} m（穿墙了）"
        );
    }
    let stopped = agent.pos();
    assert!(
        flat(stopped, disc) <= DISCONNECT_EPS,
        "判据5：停点离断连边界 {} m，超 2 m",
        flat(stopped, disc)
    );

    // ② 停在边界后继续决策 120 帧：目标仍不可达、边界不再前移、代理位移收敛为 0。
    let mut pos = stopped;
    let mut net = 0.0f32;
    let mut still = 0usize;
    for _ in 0..120 {
        match nav.path(pos, target) {
            PathOutcome::Unreachable { .. } => {}
            other => panic!("目标必须始终不可达：{other:?}"),
        }
        let frontier = nav
            .disconnect_point(pos, target)
            .expect("不可达必然存在断连边界");
        assert!(
            flat(pos, frontier) <= DISCONNECT_EPS,
            "判据5：边界跑到了 {} m 外",
            flat(pos, frontier)
        );
        // 策略：往断连边界推进——已在边界上就该一动不动（这才是「停断」，
        // 而不是贴着墙左右蹭）。
        let mut probe = NavAgent::at(&[pos, frontier], pos);
        probe.advance(nav.grid(), DT, AGENT_SPEED);
        let moved = flat(pos, probe.pos());
        net += moved;
        still = if moved < 1e-4 { still + 1 } else { 0 };
        pos = probe.pos();
        assert!(nav.is_walkable(pos), "判据5：代理不许站进不可走格：{pos:?}");
    }
    assert!(
        still >= 60,
        "判据5：停断后最长连续静止只有 {still} 帧，属于原地抖动"
    );
    assert!(
        net <= 1.0,
        "判据5：停断后累计位移 {net} m，属于抖动而非停断"
    );
    assert!(
        flat(pos, target) > DISCONNECT_EPS,
        "判据5：代理钻进了房间（离目标 {} m）",
        flat(pos, target)
    );
}
