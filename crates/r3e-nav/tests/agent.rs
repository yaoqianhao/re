//! 代理沿路行走测试（`M2·D03`）：判据 5（贴地）与判据 6（到点 + 步数上界），
//! 外加「不切角穿墙」这条路本身的可走性与身位检查。
//!
//! 代理刻意做得很笨（只沿线段推进、不做避障试探），所以这里的断言分两层：
//! 一层盯**几何**（每一步的落点可走、离障碍够远），一层盯**节拍**
//! （`dt × speed` 的位移预算有没有被浮点误差吃掉，导致步数失控）。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{polyline_length, smooth_path, Nav, NavAgent};

fn flat_scene(size: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [size, size]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 「先升后降」的山脊：做法与 `tests/path.rs` 的夹具一致，这里复制一份是因为
/// 集成测试之间不能互相 import，而 D03 需要一块**每格高度都不同**的地面。
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

fn add_wall(
    scene: &mut Scene,
    name: &str,
    x: f32,
    z_center: f32,
    z_len: f32,
) -> ([f32; 2], [f32; 2]) {
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
    (
        [x - 0.1, z_center - z_len * 0.5],
        [x + 0.1, z_center + z_len * 0.5],
    )
}

/// 中间留一条 4 m 缺口的墙，返回两段墙的 XZ 包围盒。
fn wall_with_gap(scene: &mut Scene, x: f32) -> Vec<([f32; 2], [f32; 2])> {
    vec![
        add_wall(scene, "wall_a", x, 3.0, 6.0),
        add_wall(scene, "wall_b", x, 13.0, 6.0),
    ]
}

fn dist_to_box_xz(p: Vec3, b: ([f32; 2], [f32; 2])) -> f32 {
    let dx = (b.0[0] - p.x).max(p.x - b.1[0]).max(0.0);
    let dz = (b.0[1] - p.z).max(p.z - b.1[1]).max(0.0);
    (dx * dx + dz * dz).sqrt()
}

fn min_dist_to_boxes(p: Vec3, boxes: &[([f32; 2], [f32; 2])]) -> f32 {
    boxes
        .iter()
        .map(|b| dist_to_box_xz(p, *b))
        .fold(f32::INFINITY, f32::min)
}

fn smoothed(nav: &Nav, a: Vec3, b: Vec3) -> Vec<Vec3> {
    let raw = match nav.path(a, b) {
        r3e_nav::PathOutcome::Found(p) => p,
        other => panic!("必须可达：{other:?}"),
    };
    let smooth = smooth_path(nav.grid(), &raw);
    smooth.points
}

/// 推进直到到达或步数超限，返回步数与轨迹。
fn walk(nav: &Nav, path: &[Vec3], dt: f32, speed: f32, cap: usize) -> (usize, Vec<Vec3>) {
    let mut agent = NavAgent::new(path);
    let mut trail = vec![agent.pos()];
    let mut steps = 0;
    while !agent.arrived() && steps < cap {
        let step = agent.advance(nav.grid(), dt, speed);
        assert!(!step.off_path, "第 {steps} 步被挡回：路径本就该可走");
        trail.push(agent.pos());
        steps += 1;
    }
    (steps, trail)
}

// ---------- 沿线走完 + 精确停在终点 ----------

#[test]
fn agent_walks_path_and_stops_exactly_on_target() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let path = [
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 9.0),
    ];

    // 小预算：一步步走完，每一步都朝终点前进。
    let (_, trail) = walk(&nav, &path, 0.05, 2.0, 200);
    let mut last = trail[0];
    for p in &trail[1..] {
        assert!((*p - last).length() <= 0.1 + 1e-4, "单帧位移不得超过预算");
        last = *p;
    }

    // 大预算：一帧跨完整条多段路径，必须直接到点。
    let mut agent = NavAgent::new(&path);
    let step = agent.advance(nav.grid(), 1.0, 100.0);
    assert!(step.arrived, "预算足够时一帧就该到终点");
    assert!(
        (agent.pos() - path[2]).length() < 1e-4,
        "必须精确停在终点上"
    );
    assert_eq!(agent.remaining(), 0.0);

    // 到点之后继续推进不许再动。
    let step = agent.advance(nav.grid(), 0.1, 2.0);
    assert!(step.arrived && step.progressed == 0.0);
    assert!((agent.pos() - path[2]).length() < 1e-4);
}

// ---------- 判据 5：y 全程贴地 ----------

#[test]
fn agent_y_tracks_ground_all_the_way_over_ridge() {
    let scene = ridge_scene(16.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let path = smoothed(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(14.0, 0.0, 2.0));

    let (steps, trail) = walk(&nav, &path, 0.05, 2.0, 400);
    assert!(steps > 0, "山脊路径必须真的走过");
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    // 首点是调用方给的起点（`y` 由调用方负责），从第一帧起才是代理自己贴的地。
    for p in &trail[1..] {
        let ground = nav.grid().ground_at(*p).expect("轨迹点必须落在网格内");
        assert!(
            (p.y - ground).abs() <= 1e-3,
            "判据5：{p:?} 的 y 与地面 {ground} 差 {} m",
            (p.y - ground).abs()
        );
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    assert!(
        max_y - min_y > 1.0,
        "山脊夹具必须让 y 真的爬升过（{min_y} ～ {max_y}）"
    );
}

// ---------- 全程落在可走格 ----------

#[test]
fn agent_never_enters_unwalkable_cell_around_wall() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let path = smoothed(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));

    let (steps, trail) = walk(&nav, &path, 0.05, 2.0, 400);
    assert!(steps > 0);
    for p in &trail {
        assert!(nav.grid().is_walkable(*p), "代理踏进了不可走格：{p:?}");
        let cell = nav.grid().cell_of(*p).expect("轨迹点必须在网格内");
        assert!(nav.grid().is_walkable_cell(cell[0], cell[1]));
    }
}

// ---------- 判据 4：全程保持一个身位 ----------

#[test]
fn agent_keeps_body_width_while_walking_smoothed_detour() {
    let mut scene = flat_scene(16.0);
    let boxes = wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let radius = 0.4; // `NavConfig::default().agent_radius`
    let path = smoothed(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));

    let (_, trail) = walk(&nav, &path, 0.05, 2.0, 400);
    for p in &trail {
        let d = min_dist_to_boxes(*p, &boxes);
        assert!(
            d >= radius - 1e-3,
            "判据4：代理在 {p:?} 处离墙只有 {d} m（要求 ≥ {radius}）"
        );
    }
}

// ---------- 判据 6：到点 + 步数上界 ----------

#[test]
fn agent_arrives_within_step_budget() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let (dt, speed) = (0.1f32, 2.0f32);
    let path = smoothed(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));
    let len = polyline_length(&path);
    let cap = (len / (speed * dt) * 1.5).ceil() as usize;

    let mut agent = NavAgent::new(&path);
    let mut steps = 0;
    while !agent.arrived() {
        let step = agent.advance(nav.grid(), dt, speed);
        assert!(!step.off_path, "平滑路径本该全程可走");
        steps += 1;
        assert!(
            steps <= cap,
            "判据6：走了 {steps} 步还没到，上界是 {cap}（路径 {len:.2} m）"
        );
    }
    assert!(agent.arrived());
    assert!(
        (agent.pos() - *path.last().unwrap()).length() < 1e-4,
        "到点必须是精确到点"
    );
    assert!(agent.remaining() <= 1e-4, "剩余长度必须归零");
}

// ---------- 退化输入：不动、不 panic、不写 NaN ----------

#[test]
fn agent_handles_degenerate_inputs_without_moving() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");

    // 空路径：一开始就算到达，位置在原点（`Vec3::ZERO`）。
    let mut empty = NavAgent::new(&[]);
    assert!(empty.arrived(), "空路径视为已到达");
    let step = empty.advance(nav.grid(), 0.1, 2.0);
    assert!(step.arrived && step.progressed == 0.0);
    assert_eq!(empty.pos(), Vec3::ZERO);

    // 单点路径同理。
    let mut single = NavAgent::new(&[Vec3::new(3.0, 0.0, 4.0)]);
    assert!(single.arrived());
    single.advance(nav.grid(), 0.1, 2.0);
    assert_eq!(single.pos(), Vec3::new(3.0, 0.0, 4.0));

    // 非法 dt / speed：位移 0，位置不动，更不许出现 NaN。
    let path = [Vec3::new(2.0, 0.0, 2.0), Vec3::new(10.0, 0.0, 2.0)];
    for (dt, speed) in [
        (0.0f32, 2.0f32),
        (-0.1, 2.0),
        (0.1, 0.0),
        (f32::NAN, 2.0),
        (0.1, f32::INFINITY),
    ] {
        let mut agent = NavAgent::new(&path);
        let step = agent.advance(nav.grid(), dt, speed);
        assert_eq!(step.progressed, 0.0, "dt={dt} speed={speed} 不该产生位移");
        assert_eq!(agent.pos(), path[0]);
        assert!(agent.pos().is_finite(), "位置必须保持有限值");
    }
}

// ---------- 被推离路径后续行 ----------

#[test]
fn agent_resumes_from_off_path_position() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let path = [
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 10.0),
    ];

    // 被推到第二段起始附近：`at` 必须把当前段挪到最近的那一段。
    let nudged = Vec3::new(6.0, 0.0, 5.0);
    let agent = NavAgent::at(&path, nudged);
    assert_eq!(agent.seg(), 1, "最近的一段是第二段");
    assert!(agent.remaining() < 5.5, "剩余长度必须从当前位置算起");

    // 从偏离位置继续推进也能走完，且不越出可走区。
    let (steps, trail) = walk(&nav, &path, 0.05, 2.0, 400);
    assert!(steps > 0 && trail.len() == steps + 1);

    let mut resumed = NavAgent::at(&path, nudged);
    let mut steps = 0;
    while !resumed.arrived() && steps < 400 {
        let step = resumed.advance(nav.grid(), 0.05, 2.0);
        assert!(!step.off_path);
        assert!(nav.grid().is_walkable(resumed.pos()), "续行轨迹必须可走");
        steps += 1;
    }
    assert!(resumed.arrived());
    assert!((resumed.pos() - path[2]).length() < 1e-4);
}
