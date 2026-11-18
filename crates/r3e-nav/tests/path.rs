//! 寻路测试：可达 / 不可达 / 端点非法 / 视线拉直 / 断点定位。
//!
//! 关键是把三种失败分开断言——把它们混成一个 `false`，关卡作者就永远不知道
//! 「是几何堵了」还是「工具没算完」。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{Endpoint, Nav, PathOutcome};

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
        PathOutcome::InvalidEndpoint { which } => assert_eq!(which, Endpoint::To),
        other => panic!("网格外的端点必须报 InvalidEndpoint：{other:?}"),
    }
    match nav.path(outside, inside) {
        PathOutcome::InvalidEndpoint { which } => assert_eq!(which, Endpoint::From),
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
            which: Endpoint::To
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
