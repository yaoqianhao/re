//! `nav_ops`（`M2·D04`）：四条「要算一段 / 要读写字节」的导航命令。
//!
//! 这里不重复 `r3e-nav` 的算法测试（光滑、哈希、产物序列化各有自己的文件），只钉两件
//! 只有在 runtime 这一层才暴露出来的事：
//!
//! 1. **字节真的走了磁盘**——`export` 写出的是文件而不是内存缓冲，`import` 读回来的
//!    产物哈希必须和写之前一样。判据 5 的一半在 `r3e-nav/tests/import.rs`（字节 ↔ 网格），
//!    另一半在这里（文件 ↔ 字节），两段合起来才闭环。
//! 2. **`stats` 的数字是派生量**——`cells` 必须等于 `dims` 相乘，`walkableRatio` 必须等于
//!    `walkable / cells`，分类直方图求和也必须等于 `cells`。这三条互相印证，任何一项
//!    被「补齐」或「四舍五入」都会露出来。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, TerrainPatch};
use r3e_runtime::nav_ops;

fn scene_with_wall() -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [24.0, 24.0]));
    scene
        .spawn(ObjectDesc {
            name: "wall".to_string(),
            mesh: Some("cube".to_string()),
            position: Some([12.0, 1.5, 12.0]),
            scale: Some([4.0, 3.0, 0.6]),
            ..Default::default()
        })
        .expect("墙可生成");
    scene.update_world_transforms();
    scene
}

/// 判据 5（文件层）：`nav.export` → `.navmesh` → `nav.import` 往返后哈希与统计不变。
///
/// 用真实临时文件而不是内存字节：这一层的意义就是「路径解析 + 写盘 + 读回」全跑通，
/// 绕开文件等于把这层唯一的职责跳过。文件名带进程号，避免并行跑测试时互相覆盖。
#[test]
fn export_then_import_roundtrips_through_file() {
    let scene = scene_with_wall();
    let baked = r3e_nav::Nav::bake_scene(&scene).expect("平地 + 一面墙必须能烘出来");
    let stats_before = nav_ops::stats(&baked);

    let file = std::env::temp_dir().join(format!("r3e-d04-{}.navmesh", std::process::id()));
    let path = file.display().to_string();
    let written = nav_ops::export(&baked, &path).expect("导出必须成功");
    assert!(written.bytes > 0, "写出的字节数必须为正");
    assert_eq!(
        written.product_hash, stats_before.product_hash,
        "导出回包的哈希必须等于当前网格的产物哈希"
    );
    assert!(file.is_file(), "导出之后文件必须真的存在");

    let (restored, imported) = nav_ops::import(&path).expect("刚写出的文件必须能读回");
    std::fs::remove_file(&file).ok();

    assert_eq!(
        imported.product_hash, written.product_hash,
        "往返之后产物哈希必须一字不差"
    );
    assert_eq!(
        restored.product_hash(),
        baked.product_hash(),
        "恢复出来的网格必须与内存里那份是同一张"
    );
    let stats_after = nav_ops::stats(&restored);
    assert_eq!(stats_after.cells, stats_before.cells);
    assert_eq!(stats_after.walkable, stats_before.walkable);
    assert_eq!(stats_after.regions, stats_before.regions);
    assert_eq!(stats_after.unresolved, stats_before.unresolved);
}

/// `nav.stats` 三项互相印证 + `nav.smooth` 端点与长度。
///
/// `stats` 的数字全部是派生量，所以它们必须自洽；`smooth` 只做几何剪枝，所以
/// 端点与「直线段的长度」都不许变——两点直线平滑完还是两点、长度等于欧氏距离。
#[test]
fn stats_is_self_consistent_and_smooth_keeps_endpoints() {
    let scene = scene_with_wall();
    let nav = r3e_nav::Nav::bake_scene(&scene).expect("平地 + 一面墙必须能烘出来");
    let stats = nav_ops::stats(&nav);

    let cells = stats.dims[0] as usize * stats.dims[1] as usize;
    assert_eq!(stats.cells, cells, "cells 必须等于 dims 相乘");
    assert!(stats.walkable <= stats.cells, "可走格数不可能超过格数");
    assert!(
        (stats.walkable_ratio - stats.walkable as f32 / stats.cells as f32).abs() < 1e-6,
        "walkableRatio 必须等于 walkable / cells"
    );
    assert_eq!(
        stats.class_histogram.iter().sum::<usize>(),
        stats.cells,
        "分类直方图求和必须等于格数（每一格只能属于一类）"
    );
    assert_eq!(
        stats
            .region_sizes
            .iter()
            .map(|&n| n as usize)
            .sum::<usize>(),
        stats.walkable,
        "连通块大小求和必须等于可走格数"
    );

    let from = Vec3::new(1.0, 0.0, 1.0);
    let to = Vec3::new(20.0, 0.0, 20.0);
    let smoothed = nav_ops::smooth(&nav, &[from, to]).expect("两点可直行必须能平滑");
    assert_eq!(smoothed.input_count, 2);
    assert_eq!(smoothed.points.len(), 2, "两点直线剪不出第三个点");
    assert_eq!(smoothed.points[0], from.to_array(), "起点不许动");
    assert_eq!(smoothed.points[1], to.to_array(), "终点不许动");
    let span = (to - from).length();
    assert!(
        (smoothed.length - span).abs() < 1e-3,
        "两点折线的长度必须等于两点距离：{} vs {span}",
        smoothed.length
    );

    // 单点属于参数错，不是「返回空表」——否则前端拿到空折线也无从判断哪出错了。
    assert!(matches!(
        nav_ops::smooth(&nav, &[from]),
        Err(nav_ops::NavOpError::Invalid(_))
    ));
    // 空路径属于参数错：不许猜一个缺省落盘位置。
    assert!(matches!(
        nav_ops::export(&nav, "  "),
        Err(nav_ops::NavOpError::Invalid(_))
    ));
}

/// `nav.export` 的路径纪律：不猜落盘位置，也不替调用方建目录树。
///
/// 这条独立成测，是因为它钉的不是「写不写得进」，而是「**不许**在没有路径时
/// 悄悄写进当前目录」——后者会让一次查询级调用产生磁盘副作用，判据 6 的
/// 可重复性就从这里坏掉。父目录不存在同样判参数错：造不造目录树是调用方的决定。
#[test]
fn export_refuses_implicit_or_unreachable_path() {
    let scene = scene_with_wall();
    let baked = r3e_nav::Nav::bake_scene(&scene).expect("平地 + 一面墙必须能烘出来");

    for bad in ["", "   ", "\t"] {
        let err = nav_ops::export(&baked, bad).expect_err("空路径必须判参数错");
        assert!(
            matches!(err, nav_ops::NavOpError::Invalid(_)),
            "空路径属参数错而不是导航数据问题：{err}"
        );
        assert!(
            err.to_string().contains("path"),
            "文案要点名缺的是 `path`：{err}"
        );
    }

    // 父目录不存在：不许自动 mkdir，也不许落到别处。
    let nowhere = std::env::temp_dir()
        .join(format!("r3e-d04-missing-{}", std::process::id()))
        .join("deep")
        .join("x.navmesh");
    let err = nav_ops::export(&baked, &nowhere.display().to_string())
        .expect_err("父目录不存在必须判参数错");
    assert!(
        matches!(err, nav_ops::NavOpError::Invalid(_)),
        "父目录缺失属参数错：{err}"
    );
    assert!(!nowhere.exists(), "报错之后不许留下半个文件");
}
