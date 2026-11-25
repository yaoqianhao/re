//! 路径平滑测试（`M2·D03`）：剪枝、内缩、长度、端点、可达。
//!
//! 六条判据在这里逐条落地。夹具沿用 `tests/path.rs` 的平地 + 带缺口墙，因为
//! 「绕缺口」这一形状同时含有直角、凸角和一条身位级的窄道，是内缩最容易出错的
//! 地方；再加一条手工造得比 A\* 输出密得多的格中心序列，专门盯判据 1 的
//! 「点数严格减少」。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{polyline_length, smooth_path, Nav, SmoothedPath};

fn flat_scene(size: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [size, size]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
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
    // 墙的 XZ 包围盒（scale 是全长，所以半长各取一半）。
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

fn found(nav: &Nav, a: Vec3, b: Vec3) -> Vec<Vec3> {
    match nav.path(a, b) {
        r3e_nav::PathOutcome::Found(p) => p,
        other => panic!("必须可达：{other:?}"),
    }
}

/// 点到 XZ 包围盒的距离（点在盒内为 0）。
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

/// 用半格步长把折线采样一遍，返回采样点（判据 3/4 都靠它看「整段」而非「路点」）。
fn samples(nav: &Nav, points: &[Vec3]) -> Vec<Vec3> {
    let cell = nav.report().cell_size;
    let mut out = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let steps = ((a.distance(b) / (cell * 0.5)).ceil() as usize).max(1);
        for k in 0..=steps {
            out.push(a.lerp(b, k as f32 / steps as f32));
        }
    }
    out
}

/// 手工造的「格中心级」密集折线：先沿 +X 走、再沿 +Z 走，每 0.25 m 一个点。
///
/// 比 A\* 的格中心序列还密一档，专门盯判据 1：剪枝必须真的把点数压下去，
/// 而不是原样返回。
fn dense_l_corner(start: Vec3, arm: f32) -> Vec<Vec3> {
    let step = 0.25;
    let n = (arm / step) as usize;
    let mut out = Vec::with_capacity(n * 2 + 1);
    for k in 0..=n {
        out.push(Vec3::new(start.x + k as f32 * step, start.y, start.z));
    }
    for k in 1..=n {
        out.push(Vec3::new(start.x + arm, start.y, start.z + k as f32 * step));
    }
    out
}

// ---------- 判据 1：点数严格减少，端点不动 ----------

#[test]
fn dense_corner_collapses_to_two_straight_plus_one_corner() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let raw = dense_l_corner(Vec3::new(2.0, 0.0, 2.0), 8.0);
    assert!(raw.len() > 60, "输入得足够密才测得出剪枝");
    let smooth = smooth_path(nav.grid(), &raw);

    assert!(
        smooth.count() < raw.len(),
        "判据1：平滑后点数必须严格减少（{} → {}）",
        raw.len(),
        smooth.count()
    );
    assert_eq!(smooth.points[0], raw[0], "判据1：首点必须逐位不变");
    assert_eq!(
        *smooth.points.last().unwrap(),
        *raw.last().unwrap(),
        "判据1：末点必须逐位不变"
    );
    // 一条 90° 折线剪完只剩 3 个点（首、拐、尾），允许内缩把拐点挪开但点数不增。
    assert!(smooth.count() <= 3, "直线段上不该留下多余顶点");
}

#[test]
fn nav_path_endpoints_stay_exact_after_smoothing() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let a = Vec3::new(2.0, 0.0, 2.0);
    let b = Vec3::new(13.0, 0.0, 2.0);
    let raw = found(&nav, a, b);
    let smooth = smooth_path(nav.grid(), &raw);

    assert!(smooth.count() <= raw.len(), "平滑不该制造新顶点");
    assert_eq!(smooth.points[0], raw[0], "首点必须与 A* 输出的精确端点一致");
    assert_eq!(
        *smooth.points.last().unwrap(),
        *raw.last().unwrap(),
        "末点必须与 A* 输出的精确端点一致"
    );
    // 端点就是调用方给的坐标，被平滑器挪动过就会「差一点到不了」。
    assert!((smooth.points[0] - a).length() < 1e-4);
    assert!((*smooth.points.last().unwrap() - b).length() < 1e-4);
}

// ---------- 判据 2：每段 `line_walkable` ----------

#[test]
fn every_smoothed_segment_is_line_walkable() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let raw = found(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));
    let smooth = smooth_path(nav.grid(), &raw);

    let grid = nav.grid();
    for w in smooth.points.windows(2) {
        let (from, to) = (w[0], w[1]);
        let ca = grid.cell_of(from).expect("路点在网格内");
        let cb = grid.cell_of(to).expect("路点在网格内");
        assert!(
            grid.line_walkable(ca, cb),
            "判据2：段 {from:?} → {to:?} 的格级视线不通"
        );
        for p in samples(&nav, &[from, to]) {
            assert!(
                grid.is_walkable(p),
                "判据2：段 {from:?} → {to:?} 经过不可走点 {p:?}"
            );
        }
    }
}

// ---------- 判据 3：总长不增 ----------

#[test]
fn smoothed_length_never_exceeds_raw_length() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let cases = [
        (Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 13.0)),
        (Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0)),
        (Vec3::new(2.0, 0.0, 14.0), Vec3::new(14.0, 0.0, 14.0)),
        (Vec3::new(12.0, 0.0, 12.0), Vec3::new(3.0, 0.0, 3.0)),
    ];
    for (a, b) in cases {
        let raw = found(&nav, a, b);
        let smooth = smooth_path(nav.grid(), &raw);
        let raw_len = polyline_length(&raw);
        assert!(
            smooth.length <= raw_len + 1e-4,
            "判据3：{a:?} → {b:?} 平滑后变长了（{raw_len} → {}）",
            smooth.length
        );
        // 返回的 length 必须与点列自算的一致，否则回包字段就是假的。
        assert!((smooth.length - polyline_length(&smooth.points)).abs() < 1e-3);
        // 手造的密集折线同样适用。
        let dense = dense_l_corner(Vec3::new(2.0, 0.0, 2.0), 6.0);
        let d = smooth_path(nav.grid(), &dense);
        assert!(
            d.length <= polyline_length(&dense) + 1e-4,
            "密集折线也不许变长"
        );
    }
}

// ---------- 判据 4：拐角内缩后离墙更远 ----------

#[test]
fn smoothing_keeps_a_body_width_from_wall_corners() {
    let mut scene = flat_scene(16.0);
    let boxes = wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let radius = 0.4; // `NavConfig::default().agent_radius`
    let raw = found(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));
    let smooth = smooth_path(nav.grid(), &raw);

    for p in samples(&nav, &smooth.points) {
        let d = min_dist_to_boxes(p, &boxes);
        assert!(
            d >= radius - 1e-3,
            "判据4：平滑路径上的 {p:?} 离墙只有 {d} m（要求 ≥ {radius}）"
        );
    }
}

// ---------- 稳定性与退化输入 ----------

#[test]
fn smoothing_a_smoothed_path_is_stable() {
    let mut scene = flat_scene(16.0);
    wall_with_gap(&mut scene, 8.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let raw = found(&nav, Vec3::new(2.0, 0.0, 2.0), Vec3::new(13.0, 0.0, 2.0));
    let once = smooth_path(nav.grid(), &raw);
    let twice = smooth_path(nav.grid(), &once.points);

    assert_eq!(twice.count(), once.count(), "二次平滑不该再改点数");
    assert!(
        (twice.length - once.length).abs() < 1e-4,
        "二次平滑不该再改长度（{} vs {}）",
        once.length,
        twice.length
    );
    assert_eq!(twice.points, once.points, "平滑必须收敛到不动点");
}

#[test]
fn degenerate_inputs_pass_through_unchanged() {
    let scene = flat_scene(16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let grid = nav.grid();

    let empty = smooth_path(grid, &[]);
    assert_eq!(empty.count(), 0, "空路径原样返回");
    assert_eq!(empty.length, 0.0);

    let single = smooth_path(grid, &[Vec3::new(2.0, 0.0, 2.0)]);
    assert_eq!(single.count(), 1, "单点路径原样返回");
    assert_eq!(single.length, 0.0);

    let pair = [Vec3::new(2.0, 0.0, 2.0), Vec3::new(6.0, 0.0, 2.0)];
    let two = smooth_path(grid, &pair);
    assert_eq!(two.points.len(), 2);
    assert!((two.length - 4.0).abs() < 1e-4);

    // 相邻重复点（浮点抖动造出来的）不许让夹角变成 NaN。
    let dup = [
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 2.0),
        Vec3::new(6.0, 0.0, 2.0),
    ];
    let dedup = smooth_path(grid, &dup);
    assert_eq!(dedup.points.len(), 2, "重复点应被并掉");
    assert!(dedup.length.is_finite() && (dedup.length - 4.0).abs() < 1e-4);
}

#[test]
fn corner_kept_when_inset_would_break_the_segment() {
    // L 形窄廊：外侧墙紧贴拐角，内缩候选会落进墙里，此时必须保留原顶点。
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "outer", 8.0, 8.0, 16.0); // x = 8 的整面墙
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let grid = nav.grid();
    let raw = [
        Vec3::new(2.0, 0.0, 6.0),
        Vec3::new(7.0, 0.0, 6.0),
        Vec3::new(7.0, 0.0, 12.0),
    ];
    let smooth: SmoothedPath = smooth_path(grid, &raw);

    assert_eq!(smooth.points[0], raw[0]);
    assert_eq!(
        *smooth.points.last().unwrap(),
        raw[2],
        "拐点内缩不许挪动末点"
    );
    // 不管拐点挪没挪，段必须仍然走得通、长度不许增加。
    for w in smooth.points.windows(2) {
        for p in samples(&nav, &[w[0], w[1]]) {
            assert!(grid.is_walkable(p), "内缩到墙里的段经过不可走点 {p:?}");
        }
    }
    assert!(smooth.length <= polyline_length(&raw) + 1e-4);
}

// ---------- 内缩确实会生效：剪枝留了余量时把拐点推离墙角 ----------

#[test]
fn inset_pushes_corner_away_from_wall_when_budget_allows() {
    // 一段半墙：路径必须从 z = 6 以上绕过去，墙角落在 (8.0, 6.0)。
    let mut scene = flat_scene(16.0);
    let box_a = add_wall(&mut scene, "wall", 8.0, 3.0, 6.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    // 手工给一条带直角台阶的点列：剪枝会把 (7,2) 这类台阶点直接剪掉，
    // 省下来的长度正好够内缩把拐点各推 `agent_radius + 0.05` 米。
    let stepped = [
        Vec3::new(2.0, 0.0, 2.0),
        Vec3::new(7.0, 0.0, 2.0),
        Vec3::new(7.0, 0.0, 7.0),
        Vec3::new(12.0, 0.0, 7.0),
        Vec3::new(12.0, 0.0, 2.0),
    ];
    let stepped_len = polyline_length(&stepped);
    let smooth = smooth_path(nav.grid(), &stepped);

    assert!(smooth.count() < stepped.len(), "台阶点应被剪掉");
    assert!(
        smooth.length <= stepped_len + 1e-4,
        "判据3：不许超过输入长度"
    );
    assert_eq!(smooth.points[0], stepped[0]);
    assert_eq!(*smooth.points.last().unwrap(), stepped[4], "末点必须不动");
    // 内缩生效的硬证据：内部顶点不再落在格点上（原输入全是整数坐标）。
    let moved = smooth.points[1..smooth.count() - 1]
        .iter()
        .any(|p| p.x.fract().abs() > 1e-3 || p.z.fract().abs() > 1e-3);
    assert!(
        moved,
        "预算充足时拐角必须被内缩，实际点列 {:?}",
        smooth.points
    );
    // 退完之后离墙角更远，且全程仍有一身位。
    let corner = Vec3::new(8.0, 0.0, 6.0);
    let nearest = smooth.points[1..smooth.count() - 1]
        .iter()
        .map(|p| (*p - corner).length())
        .fold(f32::INFINITY, f32::min);
    assert!(nearest > 1.5, "内缩后拐点仍贴着墙角（{nearest} m）");
    for p in samples(&nav, &smooth.points) {
        assert!(nav.grid().is_walkable(p), "平滑路径经过不可走点 {p:?}");
        assert!(
            dist_to_box_xz(p, box_a) >= 0.4 - 1e-3,
            "判据4：{p:?} 离墙太近"
        );
    }
}
