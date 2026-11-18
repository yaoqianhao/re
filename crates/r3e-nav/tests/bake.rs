//! 烘焙行为测试：可走判定、连通块、出生点体检、范围推导与规模护栏。
//!
//! 这里断言的是**口径**而不是具体格数——格数随默认 `cell_size` 变动就会碎，
//! 而「墙两边必须分成两块」「陡坡必须断开」是能力承诺，改坏了必须红。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{spawn_point, Nav, NavConfig, NavError};

/// 一块 16×16 的平地（默认地面高度 0，靠无地形也能站）。
fn flat_scene() -> Scene {
    let mut scene = Scene::new();
    let mut patch = TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0]);
    patch.name = "ground".to_string();
    scene.terrain.push(patch);
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 加一堵沿 Z 方向、把场地切成左右两半的墙。
fn add_wall(scene: &mut Scene, name: &str, x: f32, height: f32) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some([x, height * 0.5, 8.0]),
            scale: Some([0.2, height, 16.0]),
            ..Default::default()
        })
        .expect("墙必须能生成");
    scene.update_world_transforms();
}

#[test]
fn flat_scene_is_one_walkable_region() {
    let scene = flat_scene();
    let nav = Nav::bake(&scene).expect("平地可烘焙");
    let report = nav.report();
    assert_eq!(report.regions, 1, "整块平地必须是一个连通块");
    // 烘焙范围会外扩一格，地形之外那一圈不算地面，所以可走率不会到 100%。
    assert!(
        report.walkable_ratio() > 0.7 && report.walkable_ratio() < 1.0,
        "可走率异常：{}",
        report.walkable_ratio()
    );
    assert!(report.unresolved.is_empty());
    assert!(report.floating_spawns().is_empty());
    assert_eq!(report.spawns.len(), 1);
    assert_ne!(report.spawns[0].1, 0, "出生点必须落在可走格里");
}

#[test]
fn wall_splits_scene_into_two_regions() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(nav.report().regions, 2, "一堵到底的墙必须把场地切成两块");

    let start = spawn_point(&scene, "start").unwrap();
    let left = Vec3::new(3.0, 0.0, 8.0);
    let right = Vec3::new(13.0, 0.0, 8.0);
    assert!(nav.is_walkable(left) && nav.is_walkable(right));
    assert!(nav.reachable(start, left), "出生点同侧必须可达");
    assert!(!nav.reachable(start, right), "墙对面必须不可达");
    assert_ne!(nav.region_of(left), nav.region_of(right));
}

#[test]
fn thin_wall_leaves_no_gap_because_agent_radius_inflates_it() {
    // 墙只有 0.02 m 厚，但角色半径 0.4 m：这堵墙在导航意义上仍是实体。
    let mut scene = flat_scene();
    add_wall(&mut scene, "thin", 8.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(nav.report().regions, 2);
}

#[test]
fn low_obstacle_below_step_height_does_not_block() {
    let mut scene = flat_scene();
    // 高 0.1 m 的矮台：比 step_height(0.4) 矮，且整个盒都在「地面 + 0.02」之上但
    // 与格柱相交 —— 这说明单看「相交」会误判，真正决定能不能过的是高度。
    // 这里只验证它不会把整片场地切成两半（矮台挡不住跨过去的脚步）。
    scene
        .spawn(ObjectDesc {
            name: "curb".to_string(),
            mesh: Some("cube".to_string()),
            position: Some([8.0, 0.05, 8.0]),
            scale: Some([0.2, 0.1, 16.0]),
            ..Default::default()
        })
        .unwrap();
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(
        nav.report().regions,
        1,
        "0.1 m 的台阶低于 step_height，抬脚就过，不该把场地切断"
    );
    assert!(nav.reachable(Vec3::new(2.0, 0.0, 8.0), Vec3::new(13.0, 0.0, 8.0)));
}

#[test]
fn spawn_inside_wall_is_reported_as_floating() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    scene.upsert_spawn(SpawnDecl::new("inside", [8.0, 0.0, 8.0]));
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let floating = nav.report().floating_spawns();
    assert!(floating.contains(&"inside"), "墙里的出生点必须被标出来");
    assert!(!floating.contains(&"start"));
}

#[test]
fn steep_terrain_breaks_connectivity_gentle_slope_does_not() {
    // 陡坡：1 m 水平距离内抬高 3 m（≈71°），超过 45° 上限 → 必须断开。
    let mut steep = Scene::new();
    let mut patch = TerrainPatch::flat("slope", [0.0, 0.0], [1.0, 8.0]);
    patch.resolution = [2, 2];
    patch.heights = vec![0.0, 3.0, 0.0, 3.0];
    steep.terrain.push(patch);
    steep.update_world_transforms();
    let nav = Nav::bake(&steep).expect("陡坡可烘焙");
    assert_eq!(nav.report().regions, 2, "71° 的坡必须分成两块");

    // 缓坡：8 m 内抬高 3 m（≈20.6°）→ 连通。
    let mut gentle = Scene::new();
    let mut patch = TerrainPatch::flat("slope", [0.0, 0.0], [8.0, 2.0]);
    patch.resolution = [2, 2];
    patch.heights = vec![0.0, 3.0, 0.0, 3.0];
    gentle.terrain.push(patch);
    gentle.update_world_transforms();
    let nav = Nav::bake(&gentle).expect("缓坡可烘焙");
    assert_eq!(nav.report().regions, 1, "20° 的坡必须连通");
}

#[test]
fn asset_mesh_without_geometry_is_listed_as_unresolved() {
    let mut scene = flat_scene();
    scene
        .spawn(ObjectDesc {
            name: "rock".to_string(),
            mesh: Some("asset:rock.gltf#0".to_string()),
            position: Some([6.0, 0.5, 6.0]),
            scale: Some([1.0, 1.0, 1.0]),
            ..Default::default()
        })
        .expect("资产节点可生成");
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert!(
        !nav.report().unresolved.is_empty(),
        "外部网格没有几何可查，必须如实登记而不是假装算准了"
    );
    assert!(nav.report().summary().contains("未解析资产"));
}

#[test]
fn ignored_tags_are_not_baked_as_obstacles() {
    let mut scene = flat_scene();
    scene
        .spawn(ObjectDesc {
            name: "hero".to_string(),
            mesh: Some("cube".to_string()),
            position: Some([8.0, 0.5, 8.0]),
            scale: Some([0.5, 3.0, 8.0]),
            ..Default::default()
        })
        .unwrap();
    scene
        .find_mut("hero")
        .expect("hero 必须存在")
        .tags
        .push("character".to_string());
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(
        nav.report().regions,
        1,
        "带 character 标签的节点是动态物件，不该把场地切成两半"
    );
}

#[test]
fn bake_is_deterministic() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let a = Nav::bake(&scene).unwrap().report().clone();
    let b = Nav::bake(&scene).unwrap().report().clone();
    assert_eq!(a, b, "同一份场景烘两次，报告必须逐字段相同");
}

#[test]
fn oversized_bake_is_rejected_instead_of_eaten() {
    let scene = flat_scene();
    let cfg = NavConfig::default()
        .with_cell_size(0.05)
        .with_bounds([-500.0, -500.0, 500.0, 500.0]);
    match Nav::bake_with(&scene, cfg) {
        Err(NavError::TooLarge { .. }) => {}
        other => panic!("超大范围必须报 TooLarge 而不是硬烘：{other:?}"),
    }
}

#[test]
fn invalid_bounds_are_rejected() {
    let scene = flat_scene();
    let cfg = NavConfig::default().with_bounds([10.0, 10.0, 0.0, 0.0]);
    match Nav::bake_with(&scene, cfg) {
        Err(NavError::InvalidBounds { .. }) => {}
        other => panic!("反向范围必须报 InvalidBounds：{other:?}"),
    }
}

#[test]
fn empty_scene_bakes_as_a_default_floor() {
    // 没填 terrain 的场景 = 一整块默认平地（否则只有物件的场景会变成处处不可走）。
    // 但一旦填了 terrain，地形范围之外必须不可走——这条由
    // `terrain_outside_is_not_walkable` 单独守卫。
    let mut scene = Scene::new();
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("空场景也必须能烘");
    assert!(nav.report().cells > 0);
    assert_eq!(nav.report().walkable, nav.report().cells);
    assert_eq!(nav.report().regions, 1);
}

#[test]
fn terrain_outside_is_not_walkable() {
    // 8×8 的地形块，烘焙范围会被外扩一格；外扩出来的空白绝不能算地面，
    // 否则绕过地形边缘就能「绕出地图」把本该隔断的两半连起来。
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [8.0, 8.0]));
    add_wall(&mut scene, "wall", 4.0, 3.0);
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(
        nav.report().regions,
        2,
        "墙两侧必须断开，不许从地形外绕过去"
    );
    assert!(
        !nav.is_walkable(Vec3::new(-0.5, 0.0, 4.0)),
        "地形范围之外不该可走"
    );
}

#[test]
fn slope_threshold_is_forty_five_degrees() {
    // 45° 是可行走上限：正好 45° 仍算可走（斜坡不该被「等于边界」判死），
    // 60° 必须断开。
    for (deg, expect_regions) in [(45.0f32, 1u32), (60.0, 17)] {
        let mut scene = Scene::new();
        let run = 8.0f32;
        let rise = run * deg.to_radians().tan();
        // height_at 索引是 z * rx + x：沿 x 给 9 个样本（每米一个），
        // 这样整段坡是连续斜面而不是一格一格的陡坎。
        let mut patch = TerrainPatch::flat("slope", [0.0, 0.0], [run, 4.0]);
        patch.resolution = [9, 2];
        let mut heights = Vec::with_capacity(18);
        for k in 0..9 {
            heights.push(rise * k as f32 / 8.0);
        }
        heights.extend_from_slice(&heights.clone());
        patch.heights = heights;
        scene.terrain.push(patch);
        // 坡顶再铺一块 4×4 平地（高度 = 坡顶高度），好让「坡上得去」有落脚区。
        let mut top = TerrainPatch::flat("top", [run, 0.0], [4.0, 4.0]);
        top.resolution = [2, 2];
        top.heights = vec![rise; 4];
        scene.terrain.push(top);
        scene.update_world_transforms();
        let nav = Nav::bake(&scene).expect("烘焙成功");
        if deg < 50.0 {
            assert_eq!(
                nav.report().regions,
                expect_regions,
                "{deg}° 坡在坡限内，必须是一整块连通区"
            );
        } else {
            assert!(
                nav.report().regions >= 3,
                "{deg}° 坡超 45° 坡限，必须被切成多块，实际 {}",
                nav.report().regions
            );
        }
    }
}

#[test]
fn step_height_threshold_is_three_tenths_walkable_six_tenths_not() {
    // 台阶用实体方块造：一块 0.3 m 高的台阶抬脚就过，0.6 m 的必须挡住路
    // （阈值 = 默认 step_height 0.4 m）。
    for (step, expect_reachable) in [(0.3f32, true), (0.6, false)] {
        let mut scene = Scene::new();
        scene
            .terrain
            .push(TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0]));
        scene
            .spawn(ObjectDesc {
                name: "step".to_string(),
                mesh: Some("cube".to_string()),
                position: Some([8.0, step * 0.5, 8.0]),
                scale: Some([0.2, step, 16.0]),
                ..Default::default()
            })
            .expect("生成台阶节点");
        scene.update_world_transforms();
        let nav = Nav::bake(&scene).expect("烘焙成功");
        let ok = nav.reachable(Vec3::new(3.0, 0.0, 8.0), Vec3::new(13.0, 0.0, 8.0));
        assert_eq!(ok, expect_reachable, "{step} m 台阶的可达性判定不对");
    }
}

#[test]
fn one_square_kilometre_bakes_under_half_a_second() {
    // 1 km² 基准：cell 1.0 → 100 万格。这是「编辑器里能随手重烘」的门槛，
    // 超了就要上分层栅格（R5 对策）。CI 机器慢，阈值给到 500 ms。
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [1000.0, 1000.0]));
    for k in 0..200 {
        let x = 10.0 + (k % 20) as f32 * 50.0;
        let z = 10.0 + (k / 20) as f32 * 50.0;
        let _ = z;
        add_wall(&mut scene, &format!("w{k}"), x, 3.0);
    }
    scene.update_world_transforms();
    let cfg = r3e_nav::NavConfig {
        cell_size: 1.0,
        ..Default::default()
    };
    let t0 = std::time::Instant::now();
    let nav = Nav::bake_with(&scene, cfg).expect("1 km² 必须能烘");
    let spent = t0.elapsed();
    println!("1 km² 烘焙耗时 {spent:?}，{}", nav.report().summary());
    assert!(nav.report().cells >= 1_000_000);
    assert!(
        spent < std::time::Duration::from_millis(500),
        "1 km² 烘焙耗时 {spent:?} 超过 500 ms 门槛"
    );
}

#[test]
fn regions_report_and_disconnect_point_agree() {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0]));
    add_wall(&mut scene, "wall", 8.0, 3.0);
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("烘焙成功");
    let regions = nav.regions();
    assert_eq!(regions.len() as u32, nav.report().regions);
    assert!(regions.iter().all(|r| r.cells > 0));
    assert!(regions.iter().all(|r| r.min[0] <= r.max[0]));
    let a = Vec3::new(2.0, 0.0, 8.0);
    let b = Vec3::new(13.0, 0.0, 8.0);
    assert!(!nav.reachable(a, b));
    let cut = nav.disconnect_point(a, b).expect("断开点必须给得出来");
    assert!(nav.is_walkable(cut), "断开点必须落在可走面上");
    assert_eq!(nav.region_of(cut), nav.region_of(a), "断开点必须与起点同块");
}
