//! 寻路测试：可达 / 不可达 / 端点非法 / 视线拉直 / 断点定位。
//!
//! 关键是把三种失败分开断言——把它们混成一个 `false`，关卡作者就永远不知道
//! 「是几何堵了」还是「工具没算完」。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{
    CellClass, Endpoint, InvalidReason, Nav, NavConfig, PathOutcome, PathStats,
    NAV_PATH_MAX_EXPANSIONS, STEP_COST_PENALTY,
};
use r3e_physics::HeightField;

fn flat_scene(size: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [size, size]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 在 x 方向加一堵墙，`z_center` 为中心、`z_len` 为沿 Z 的长度。
fn add_wall(scene: &mut Scene, name: &str, x: f32, z_center: f32, z_len: f32) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some([x, 1.5, z_center]),
            scale: Some([0.2, 3.0, z_len]),
            ..Default::default()
        })
        .expect("墙可生成");
    scene.update_world_transforms();
}

/// 中间留一条 4 m 缺口的墙。
fn wall_with_gap(scene: &mut Scene, x: f32) {
    add_wall(scene, "wall_a", x, 3.0, 6.0);
    add_wall(scene, "wall_b", x, 13.0, 6.0);
}

#[test]
fn straight_path_has_no_inner_points() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 13.0);
    let outcome = nav.path(a, b);
    let points = match outcome {
        PathOutcome::Found(p) => p,
        other => panic!("无障碍直线必须可达：{other:?}"),
    };
    assert_eq!(points.len(), 2, "没有障碍时拉直后只剩首尾两点");
    assert!((points[0] - a).length() < 1e-4);
    assert!((points[1] - b).length() < 1e-4);
}

#[test]
fn path_goes_around_wall_through_gap() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 2.0);
    let points = match nav.path(a, b) {
        PathOutcome::Found(p) => p,
        other => panic!("有缺口就必须能绕过去：{other:?}"),
    };
    assert!(points.len() > 2, "绕过缺口必须有拐点");
    for p in &points {
        assert!(nav.is_walkable(*p), "路径点 {p:?} 落在不可走格上");
    }
    // 每个路点之间必须真的看得通（拉直不许制造穿墙段）。
    for w in points.windows(2) {
        let (from, to) = (w[0], w[1]);
        let dist = from.distance(to);
        let steps = (dist / (nav.report().cell_size * 0.5)).ceil() as usize + 1;
        for k in 0..=steps {
            let t = k as f32 / steps as f32;
            let p = from.lerp(to, t);
            assert!(nav.is_walkable(p), "拉直段经过不可走点 {p:?}");
        }
    }
}

#[test]
fn full_wall_yields_unreachable_with_regions() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 8.0);
    let b = Vec3::new(14.0, 0.0, 8.0);
    match nav.path(a, b) {
        PathOutcome::Unreachable {
            from_region,
            to_region,
        } => {
            assert_ne!(from_region, to_region);
            assert_ne!(from_region, 0);
            assert_ne!(to_region, 0);
        }
        other => panic!("整堵墙隔断必须报 Unreachable：{other:?}"),
    }
    assert!(!nav.reachable(a, b));
    let (ok, msg) = nav.check_reach(a, b);
    assert!(!ok);
    assert!(msg.contains("不可达"), "诊断文案要能直接读：{msg}");
    assert!(msg.contains("最多走到"), "诊断要给出卡住的位置：{msg}");
}

#[test]
fn frontier_points_to_the_wall() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 8.0);
    let b = Vec3::new(14.0, 0.0, 8.0);
    let p = nav
        .frontier(a, b)
        .expect("隔断场景必须能给出可走到的极限位置");
    assert!(p.x < 8.0, "极限位置应该停在墙的这一侧，实际 {p:?}");
    assert!(nav.is_walkable(p));
}

#[test]
fn endpoint_outside_grid_is_invalid_not_unreachable() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let inside = Vec3::new(4.0, 0.0, 4.0);
    let outside = Vec3::new(500.0, 0.0, 500.0);
    match nav.path(inside, outside) {
        PathOutcome::InvalidEndpoint { which, .. } => assert_eq!(which, Endpoint::To),
        other => panic!("网格外的端点必须报 InvalidEndpoint：{other:?}"),
    }
    match nav.path(outside, inside) {
        PathOutcome::InvalidEndpoint { which, .. } => assert_eq!(which, Endpoint::From),
        other => panic!("网格外的起点必须报 InvalidEndpoint：{other:?}"),
    }
}

#[test]
fn endpoint_inside_obstacle_is_invalid() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    // 墙的中心：吸附半径只有 1.5 格，吸不出可走点。
    let outcome = nav.path(Vec3::new(2.0, 0.0, 8.0), Vec3::new(8.0, 1.0, 8.0));
    assert!(
        matches!(outcome, PathOutcome::InvalidEndpoint { .. }),
        "埋在墙里的目标必须报 InvalidEndpoint：{outcome:?}"
    );
    assert!(matches!(
        outcome,
        PathOutcome::InvalidEndpoint {
            which: Endpoint::To,
            ..
        }
    ));
}

#[test]
fn slightly_off_surface_start_is_snapped() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    // 起点抬高 0.3 m（角色脚底略微离地），不该因此报非法端点。
    let a = Vec3::new(2.0, 0.3, 2.0);
    let b = Vec3::new(12.0, 0.0, 12.0);
    assert!(matches!(nav.path(a, b), PathOutcome::Found(_)));
}

#[test]
fn same_cell_path_is_two_points() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(4.0, 0.0, 4.0);
    let b = Vec3::new(4.2, 0.0, 4.2);
    let points = match nav.path(a, b) {
        PathOutcome::Found(p) => p,
        other => panic!("同格必须可达：{other:?}"),
    };
    assert_eq!(points.len(), 2);
}

#[test]
fn path_is_deterministic() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 2.0);
    let first = nav.path(a, b);
    for _ in 0..3 {
        assert_eq!(nav.path(a, b), first, "同一份数据算出的路径必须逐点相同");
    }
}

#[test]
fn spawn_to_spawn_reachability_uses_spawn_points() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    scene.upsert_spawn(SpawnDecl::new("boss", [13.0, 0.0, 3.0]));
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let points = r3e_nav::spawn_points(&scene);
    assert_eq!(points.len(), 2);
    let start = r3e_nav::spawn_point(&scene, "start").expect("start 出生点");
    let boss = r3e_nav::spawn_point(&scene, "boss").expect("boss 出生点");
    assert!(nav.reachable(start, boss), "同一连通块的两个出生点必须互通");
    assert_eq!(nav.region_of(start), nav.region_of(boss));
}

// ---------- M2·D02：A* 契约增量（分类代价 / 失败码细分 / 确定性 / 上界） ----------

/// 造一个方块（尺寸单位 = 米），用于台阶 / 墙 / 蛇形长廊夹具。
fn add_box(scene: &mut Scene, name: &str, position: [f32; 3], scale: [f32; 3]) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some(position),
            scale: Some(scale),
            ..Default::default()
        })
        .expect("方块可生成");
    scene.update_world_transforms();
}

/// 水平投影距离：判「几何长度是否相当」用它，不掺高度。
fn horizontal(a: Vec3, b: Vec3) -> f32 {
    ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt()
}

fn class_count(nav: &Nav, class: CellClass) -> usize {
    nav.grid().class_histogram()[class.code() as usize]
}

/// 「先升后降」的山脊：每米一个高度样本，水平投影长度与同尺寸平地完全一致，
/// 只有中途那一段坡格变贵 —— 用来把「分类代价」和「几何长度」两个变量分开。
fn ridge_scene(size: f32, rise: f32) -> Scene {
    let mut scene = Scene::new();
    let samples = size as u32 + 1;
    let half = samples / 2;
    let mut row: Vec<f32> = (0..=half).map(|k| rise * k as f32 / half as f32).collect();
    row.extend((1..=half).map(|k| rise * (half - k) as f32 / half as f32));
    assert_eq!(row.len(), samples as usize, "高度样本数必须是 size + 1");
    let mut heights = row.clone();
    heights.extend_from_slice(&row);
    let mut patch = TerrainPatch::flat("ridge", [0.0, 0.0], [size, 4.0]);
    patch.resolution = [samples, 2];
    patch.heights = heights;
    scene.terrain.push(patch);
    scene.update_world_transforms();
    scene
}

/// 平地 + 一道离散坎的高度场：`x ≥ side/2` 一侧整体抬高 `rise`。
///
/// 为什么必须直接给高度场、而不是放个矮盒子：`stamp_obstacle` 对 `rise ≤
/// step_height` 的障碍是「不封路也不抬高度」（`bake.rs` 的路沿石口径），盒子根本
/// 进不了 `grid.heights`，于是永远烘不出 `Step` 格。坎必须来自地形这条真值链。
///
/// 为什么格长要压到 0.2 m：`Step` 的判据是「`dh > cell·tan(45°)` 且 `dh ≤
/// step_height`」。默认 0.5 m 格下这个区间是空集（0.5 > 0.4），无论怎么造都只会
/// 落到 `Slope` 或 `Blocked`。0.2 m 格下区间是 `(0.2, 0.4]`，`rise = 0.3` 正好落进去。
fn ledge_field(side: f32, cell: f32, rise: f32) -> HeightField {
    let cols = (side / cell).round() as usize;
    let rows = cols;
    let x0 = cell * 0.5;
    let z0 = cell * 0.5;
    let mut heights = Vec::with_capacity(cols * rows);
    for _ in 0..rows {
        for i in 0..cols {
            let x = x0 + i as f32 * cell;
            heights.push(if x >= side * 0.5 { rise } else { 0.0 });
        }
    }
    HeightField::new(x0, z0, cell, cols, rows, heights).expect("高度场构造成功")
}

/// 200 m 见方的蛇形长廊：8 道横墙交替在左右两翼留缺口，逼 A* 一路折返。
fn serpentine_scene(side: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [side, side]));
    let bands = 8;
    let gap = side * 0.1;
    for k in 0..bands {
        let z = side * (k as f32 + 1.0) / (bands as f32 + 1.0);
        // 偶数道缺口在右端、奇数道在左端。
        let (cx, len) = if k % 2 == 0 {
            ((side - gap) * 0.5, side - gap)
        } else {
            (gap + (side - gap) * 0.5, side - gap)
        };
        add_box(
            &mut scene,
            &format!("wall{k}"),
            [cx, 1.5, z],
            [len, 3.0, 0.4],
        );
    }
    scene.update_world_transforms();
    scene
}

/// 判据 1 的「每段邻接」：拉直后的路径段会跨很多格，所以按半格采样逐步验证
/// 「每一步都落在可走格上、相邻采样点之间迈得过去」。
fn assert_segments_stepwise_valid(nav: &Nav, points: &[Vec3]) {
    let grid = nav.grid();
    let cell = nav.report().cell_size;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let steps = ((a.distance(b) / (cell * 0.5)).ceil() as usize).max(1);
        let mut prev = grid.cell_of(a).expect("路点必须落在网格内");
        assert!(
            grid.is_walkable_cell(prev[0], prev[1]),
            "路点 {a:?} 所在的格不可走"
        );
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let p = a.lerp(b, t);
            let c = grid.cell_of(p).expect("路径采样点必须落在网格内");
            assert!(
                grid.is_walkable_cell(c[0], c[1]),
                "路径段 {a:?} → {b:?} 经过不可走格 {p:?}（{c:?}）"
            );
            if c != prev {
                assert!(grid.can_step(prev, c), "路径段 {prev:?} → {c:?} 迈不过去");
                prev = c;
            }
        }
    }
}

#[test]
fn found_path_segments_step_by_step_are_valid() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let points = match nav.path(Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0)) {
        PathOutcome::Found(p) => p,
        other => panic!("带缺口的墙两侧必须可达：{other:?}"),
    };
    assert!(points.len() > 2, "绕缺口必须有拐点，实际 {points:?}");
    assert_segments_stepwise_valid(&nav, &points);
}

#[test]
fn path_turning_points_sit_on_walkable_cells() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let points = match nav.path(Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 13.0)) {
        PathOutcome::Found(p) => p,
        other => panic!("必须可达：{other:?}"),
    };
    let grid = nav.grid();
    for p in &points {
        let c = grid.cell_of(*p).expect("路点必须在网格内");
        assert!(
            grid.is_walkable_cell(c[0], c[1]),
            "拐点 {p:?} 落在 {c:?}，那一格站不住"
        );
    }
}

#[test]
fn invalid_endpoint_off_mesh_is_reported() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let outcome = nav.path(Vec3::new(2.0, 0.0, 8.0), Vec3::new(8.0, 0.0, 8.0));
    assert_eq!(
        outcome.invalid_reason(),
        Some(InvalidReason::OffMesh),
        "埋在墙里的目标必须细分为 OffMesh：{outcome:?}"
    );
    assert!(
        matches!(
            outcome,
            PathOutcome::InvalidEndpoint {
                which: Endpoint::To,
                ..
            }
        ),
        "报错侧必须是 to：{outcome:?}"
    );
}

#[test]
fn invalid_endpoint_out_of_bounds_is_reported() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let outcome = nav.path(Vec3::new(4.0, 0.0, 4.0), Vec3::new(500.0, 0.0, 500.0));
    assert_eq!(
        outcome.invalid_reason(),
        Some(InvalidReason::OutOfBounds),
        "网格外的点必须细分为 OutOfBounds：{outcome:?}"
    );
    assert!(
        matches!(
            outcome,
            PathOutcome::InvalidEndpoint {
                which: Endpoint::To,
                ..
            }
        ),
        "报错侧必须是 to：{outcome:?}"
    );
}

#[test]
fn invalid_endpoint_nonfinite_is_reported() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let outcome = nav.path(Vec3::new(4.0, 0.0, 4.0), Vec3::new(f32::NAN, 0.0, 4.0));
    assert_eq!(
        outcome.invalid_reason(),
        Some(InvalidReason::NonFinite),
        "NaN 必须细分为 NonFinite：{outcome:?}"
    );
    assert!(
        matches!(
            outcome,
            PathOutcome::InvalidEndpoint {
                which: Endpoint::To,
                ..
            }
        ),
        "报错侧必须是 to：{outcome:?}"
    );
}

#[test]
fn nonfinite_takes_priority_over_out_of_bounds() {
    // 判定顺序是契约：坐标坏到不可比时先报 NonFinite，不能因为「也在范围外」
    // 就说成 OutOfBounds —— 那会把调用方 bug 误导成「夹具范围不够」。
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let outcome = nav.path(Vec3::new(f32::NAN, 0.0, 500.0), Vec3::new(4.0, 0.0, 4.0));
    assert_eq!(
        outcome.invalid_reason(),
        Some(InvalidReason::NonFinite),
        "NaN 且越界时必须是 NonFinite：{outcome:?}"
    );
    assert!(
        matches!(
            outcome,
            PathOutcome::InvalidEndpoint {
                which: Endpoint::From,
                ..
            }
        ),
        "报错侧必须是 from：{outcome:?}"
    );
}

#[test]
fn invalid_reason_wire_names_are_frozen() {
    // 协议面（nav.* 回包）直接吃这三个串，改了等于改协议。
    assert_eq!(InvalidReason::NonFinite.as_str(), "nonFinite");
    assert_eq!(InvalidReason::OutOfBounds.as_str(), "outOfBounds");
    assert_eq!(InvalidReason::OffMesh.as_str(), "offMesh");
}

#[test]
fn invalid_reason_describe_text_is_frozen() {
    let cases = [
        (
            Endpoint::From,
            InvalidReason::NonFinite,
            "不可达：from 端点坐标含 NaN/Inf",
        ),
        (
            Endpoint::To,
            InvalidReason::OutOfBounds,
            "不可达：to 端点落在网格范围外",
        ),
        (
            Endpoint::To,
            InvalidReason::OffMesh,
            "不可达：to 端点不在任何可走格上（吸附半径内无可走点）",
        ),
    ];
    for (which, reason, expect) in cases {
        let outcome = PathOutcome::InvalidEndpoint { which, reason };
        assert_eq!(
            outcome.describe(),
            expect,
            "{which:?}/{reason:?} 的文案被改了"
        );
        assert_eq!(outcome.invalid_reason(), Some(reason));
    }
}

#[test]
fn path_deterministic_over_hundred_runs() {
    // 判据 3：同输入 100 次路径逐点一致（原先只跑 4 次）。
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 13.0);
    let first = nav.path(a, b);
    assert!(first.is_found(), "夹具必须可达：{first:?}");
    for round in 0..100 {
        assert_eq!(
            nav.path(a, b),
            first,
            "第 {round} 次的结果与首次不一致，tie-break 不稳定"
        );
    }
}

#[test]
fn path_stats_deterministic_over_hundred_runs() {
    // 观测值也必须稳定：展开数抖动意味着 tie-break 命中了堆的非确定顺序。
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 13.0);
    let (_, first) = nav.path_with_stats(a, b);
    for round in 0..100 {
        let (_, stats) = nav.path_with_stats(a, b);
        assert_eq!(
            stats.expanded, first.expanded,
            "第 {round} 次的展开数变了：{} ≠ {}",
            stats.expanded, first.expanded
        );
        assert_eq!(
            stats.cost.to_bits(),
            first.cost.to_bits(),
            "第 {round} 次的代价变了：{} ≠ {}",
            stats.cost,
            first.cost
        );
    }
}

#[test]
fn flat_path_cost_equals_geometric_length() {
    // 分类代价的基线：平地上所有系数都是 1，cost 必须等于几何长度。
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 2.0);
    let (outcome, stats) = nav.path_with_stats(a, b);
    assert!(outcome.is_found(), "平地必须可达：{outcome:?}");
    let geo = horizontal(a, b);
    assert!(
        (stats.cost - geo).abs() < 0.05,
        "平地 cost={} 应等于几何长度 {geo}",
        stats.cost
    );
    assert_eq!(
        class_count(&nav, CellClass::Slope) + class_count(&nav, CellClass::Step),
        0,
        "这块夹具不该出现坡或坎"
    );
}

#[test]
fn slope_path_costs_more_than_flat_of_same_length() {
    // 判据 4：几何长度相当（水平投影完全相同）时，走坡必须更贵。
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 2.0);
    let flat = Nav::bake(&flat_scene(16.0)).expect("烘焙成功");
    let ridge = Nav::bake(&ridge_scene(16.0, 3.0)).expect("烘焙成功");
    assert!(
        class_count(&ridge, CellClass::Slope) > 0,
        "山脊夹具必须真的烘出坡格"
    );
    assert_eq!(
        class_count(&ridge, CellClass::Step),
        0,
        "连续斜面不该被判成台阶"
    );
    let (fo, fs) = flat.path_with_stats(a, b);
    let (ro, rs) = ridge.path_with_stats(a, b);
    assert!(fo.is_found() && ro.is_found(), "两侧都必须可达");
    assert!((horizontal(a, b) - horizontal(a, b)).abs() < 1e-6);
    assert!(
        rs.cost > fs.cost * 1.05,
        "几何长度相当（{:.1} m）时坡路必须更贵：坡 {:.3} vs 平 {:.3}",
        horizontal(a, b),
        rs.cost,
        fs.cost
    );
}

#[test]
fn step_cell_adds_fixed_penalty() {
    // 判据 4：跨坎加固定惩罚。坎来自地形高度场（见 `ledge_field` 的两条理由），
    // 基线用同一份场但 `rise = 0`，于是两边的几何长度完全一致，差值只能是惩罚本身。
    let side = 16.0f32;
    let cell = 0.2f32;
    let cfg = NavConfig::default().with_cell_size(cell);
    let scene = flat_scene(side);
    let flat = Nav::bake_with_field(&scene, &ledge_field(side, cell, 0.0), cfg.clone())
        .expect("平地基线烘焙成功");
    let stepped =
        Nav::bake_with_field(&scene, &ledge_field(side, cell, 0.3), cfg).expect("矮坎烘焙成功");
    assert!(
        class_count(&stepped, CellClass::Step) > 0,
        "0.3 m 离散坎在 0.2 m 格下必须烘成 Step 格（实际分类直方图 {:?}）",
        stepped.grid().class_histogram()
    );
    assert_eq!(
        class_count(&stepped, CellClass::Slope),
        0,
        "离散坎不该被判成连续斜面"
    );
    let a = Vec3::new(3.0, 0.0, 8.0);
    let b = Vec3::new(13.0, 0.0, 8.0);
    let (fo, fs) = flat.path_with_stats(a, b);
    let (so, ss) = stepped.path_with_stats(a, b);
    assert!(fo.is_found(), "平地基线必须可达：{fo:?}");
    assert!(so.is_found(), "跨 0.3 m 矮坎必须可达：{so:?}");
    assert!(
        ss.cost >= fs.cost + STEP_COST_PENALTY * 0.9,
        "跨坎至少要多 {} 米代价：{} vs {}",
        STEP_COST_PENALTY,
        ss.cost,
        fs.cost
    );
}

#[test]
fn same_cell_path_reports_zero_cost_and_no_expansion() {
    // 同格是「退化成两点的路径」，不是在图上搜出来的，所以观测值必须是默认值。
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let (outcome, stats) = nav.path_with_stats(Vec3::new(4.0, 0.0, 4.0), Vec3::new(4.2, 0.0, 4.2));
    assert!(outcome.is_found(), "同格必须可达：{outcome:?}");
    assert_eq!(stats, PathStats::default(), "同格不该有展开数或代价");
}

#[test]
fn unreachable_reports_zero_cost_but_keeps_expanded() {
    // 失败时 cost 记 0.0 是「没有路径」。这里 `expanded` 也必须是 0：区域预检
    // （`region_at` 判两端连通块）先于搜索下结论，压根不该开始展点 —— 一旦这里
    // 出现非 0，说明有人在不可达场景上白烧了 40 万次扩展上限。
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let (outcome, stats) = nav.path_with_stats(Vec3::new(2.0, 0.0, 8.0), Vec3::new(13.0, 0.0, 8.0));
    assert!(
        matches!(outcome, PathOutcome::Unreachable { .. }),
        "整墙切断必须报 Unreachable：{outcome:?}"
    );
    assert_eq!(stats.cost, 0.0, "不可达时 cost 必须是 0.0");
    assert_eq!(
        stats.expanded, 0,
        "不可达应由连通块预检直接下结论，不该展开任何节点"
    );
}

#[test]
fn worst_case_200m_expansion_below_budget() {
    // 判据 5：200 m × 200 m（cell 0.5 m → 400×400 格）蛇形长廊的最坏展开数。
    let side = 200.0f32;
    let scene = serpentine_scene(side);
    let cfg = NavConfig::default().with_cell_size(0.5);
    let nav = Nav::bake_with(&scene, cfg).expect("烘焙成功");
    let a = Vec3::new(5.0, 0.0, 5.0);
    let b = Vec3::new(side - 5.0, 0.0, side - 5.0);
    let (outcome, stats) = nav.path_with_stats(a, b);
    assert!(outcome.is_found(), "蛇形长廊必须连通：{outcome:?}");
    println!(
        "D02 最坏场景实测：展开 {} 个节点（上限 {NAV_PATH_MAX_EXPANSIONS}），cost = {:.2} m",
        stats.expanded, stats.cost
    );
    assert!(
        stats.expanded < NAV_PATH_MAX_EXPANSIONS,
        "最坏场景展开 {} 已撞上限 {}",
        stats.expanded,
        NAV_PATH_MAX_EXPANSIONS
    );
    // 蛇形路线远长于直线：cost 必须显著大于两端直线距离（≈ 269 m）。
    assert!(
        stats.cost > 500.0,
        "蛇形路线的代价应远大于直线，实际 {:.2}",
        stats.cost
    );
}
