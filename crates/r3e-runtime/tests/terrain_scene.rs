//! C03b 装配侧可判定面：零地形、空块跳过、LOD 重建计数、非法配置、顶点守恒。
//!
//! 装配层不碰 GPU，所以这里可以把「这帧到底上传什么」整段断言掉。

use r3e_core::TerrainPatch;
use r3e_runtime::terrain_scene::{TerrainScene, TerrainSceneConfig, TerrainSceneEvent};
use r3e_terrain::{Lod, LodConfig};

/// 造一块地形：`res × res` 的高度样本，按 `height` 闭包算高度。
fn patch(name: &str, origin: [f32; 2], size: [f32; 2], res: u32, height: f32) -> TerrainPatch {
    let n = (res * res) as usize;
    TerrainPatch {
        name: name.to_string(),
        origin,
        size,
        resolution: [res, res],
        heights: vec![height; n],
        ..Default::default()
    }
}

/// 测试 7：场景没有地形 → 空计划，调用方据此不创建管线。
#[test]
fn 零地形返回空计划() {
    let mut scene = TerrainScene::new(TerrainSceneConfig::default());
    let plan = scene.plan(&[], Some([0.0, 0.0]));
    assert!(plan.is_empty());
    assert_eq!(plan.vertex_count(), 0);
    assert_eq!(plan.index_count(), 0);
    assert_eq!(plan.rebuilds, 0);
    assert!(plan.events.is_empty(), "零地形不该产生任何事件");

    // 从有地形切回没地形：状态清干净，下一次有地形时重新全量重建
    let patches = [patch("p", [0.0, 0.0], [100.0, 100.0], 11, 1.0)];
    let first = scene.plan(&patches, None);
    assert!(!first.is_empty());
    assert!(scene.plan(&[], None).is_empty());
    let again = scene.plan(&patches, None);
    assert_eq!(
        again.rebuilds,
        first.blocks.len(),
        "清空后重新出现的地形必须整段重建"
    );
}

/// 测试 8：块没有 patch 覆盖时被跳过，并留下一条事件（不生成空网格去骗 draw call）。
#[test]
fn 无覆盖块被跳过() {
    // 两块 50×50 的地形，中间留 50 米空洞 → 并集矩形切出 3 块，第 2 块无覆盖
    let patches = [
        patch("left", [0.0, 0.0], [50.0, 50.0], 11, 2.0),
        patch("right", [100.0, 0.0], [50.0, 50.0], 11, 2.0),
    ];
    let mut scene = TerrainScene::new(TerrainSceneConfig {
        block_size: 50.0,
        ..Default::default()
    });
    let plan = scene.plan(&patches, None);

    assert_eq!(plan.blocks.len(), 2, "只有两块有覆盖");
    let indices: Vec<[u32; 2]> = plan.blocks.iter().map(|b| b.index).collect();
    assert_eq!(indices, vec![[0, 0], [2, 0]]);
    let skipped: Vec<[u32; 2]> = plan
        .events
        .iter()
        .filter_map(|e| match e {
            TerrainSceneEvent::BlockSkipped { index } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(skipped, vec![[1, 0]]);
    // 被跳过的块不占任何顶点预算
    assert!(plan.vertex_count() > 0);
    assert!(plan
        .blocks
        .iter()
        .all(|b| !b.vertices.is_empty() && !b.indices.is_empty()));
}

/// 测试 9：只有 LOD 变化的块才重建，相机不动时零重建。
#[test]
fn lod_变化才重建() {
    let patches = [patch("big", [0.0, 0.0], [200.0, 200.0], 21, 3.0)];
    let mut scene = TerrainScene::new(TerrainSceneConfig {
        block_size: 50.0,
        lod: LodConfig::default(),
        ..Default::default()
    });

    // 第一帧：所有块都从「没有」变成「有」→ 全量重建
    let first = scene.plan(&patches, Some([0.0, 0.0]));
    assert_eq!(first.blocks.len(), 16, "200 米按 50 米切 = 4×4");
    assert_eq!(first.rebuilds, 16);
    assert_eq!(first.total_rebuilds, 16);
    assert!(first
        .events
        .iter()
        .all(|e| matches!(e, TerrainSceneEvent::LodChanged { .. })));

    // 第二帧：相机不动 → 一块都不用重建
    let second = scene.plan(&patches, Some([0.0, 0.0]));
    assert_eq!(second.rebuilds, 0, "相机不动不该重建任何块");
    assert_eq!(second.total_rebuilds, 16);
    assert!(second.events.is_empty());
    assert_eq!(second.blocks, first.blocks);

    // 第三帧：相机挪到很远处 → 全部退到 L2，4 块各重建一次
    let far = scene.plan(&patches, Some([5000.0, 5000.0]));
    assert!(far.blocks.iter().all(|b| b.lod == Lod::L2));
    // 精确口径：只有「档位真的变了」的块才重建。第一帧里最远那几块本来就是 L2，
    // 相机拉到 5000 米外时它们的档位不变，不该被算成重建。
    let already_l2 = first.blocks.iter().filter(|b| b.lod == Lod::L2).count();
    assert_eq!(
        far.rebuilds,
        first.blocks.len() - already_l2,
        "重建块数应等于档位发生变化的块数（首帧已是 L2 的有 {already_l2} 块）"
    );
    assert_eq!(far.total_rebuilds, 16 + far.rebuilds as u64);
    // 远档顶点数必须比近档少（LOD 真的生效，而不是只换了标签）
    assert!(
        far.vertex_count() < first.vertex_count(),
        "L2 顶点数 {} 未少于 L0 的 {}",
        far.vertex_count(),
        first.vertex_count()
    );

    // 第四帧：同样远处 → 又不重建
    assert_eq!(scene.plan(&patches, Some([5000.0, 5000.0])).rebuilds, 0);

    // 无相机（headless）→ 一律 L0，且与「相机在原点」同档
    let mut headless = TerrainScene::new(TerrainSceneConfig {
        block_size: 50.0,
        ..Default::default()
    });
    let plan = headless.plan(&patches, None);
    assert!(plan.blocks.iter().all(|b| b.lod == Lod::L0));
}

/// 测试 10：非法配置 → 记事件 + 本帧不画地形，而不是悄悄用默认值兜底。
#[test]
fn 非法配置返回空计划并记事件() {
    let patches = [patch("p", [0.0, 0.0], [100.0, 100.0], 11, 1.0)];

    for bad in [-1.0f32, 0.0, f32::NAN, f32::INFINITY, 5000.0] {
        let mut scene = TerrainScene::new(TerrainSceneConfig {
            block_size: bad,
            ..Default::default()
        });
        let plan = scene.plan(&patches, None);
        assert!(plan.is_empty(), "块尺寸 {bad} 不该产出地形");
        assert!(
            matches!(
                plan.events.as_slice(),
                [TerrainSceneEvent::InvalidConfig { .. }]
            ),
            "块尺寸 {bad} 应记一条 InvalidConfig，实得 {:?}",
            plan.events
        );
    }

    // 退化矩形（尺寸为 0）同样拒绝
    let degenerate = [patch("flat", [0.0, 0.0], [0.0, 0.0], 1, 0.0)];
    let mut scene = TerrainScene::new(TerrainSceneConfig::default());
    let plan = scene.plan(&degenerate, None);
    assert!(plan.is_empty());
    assert!(matches!(
        plan.events.as_slice(),
        [TerrainSceneEvent::InvalidConfig { .. }]
    ));

    // 块尺寸改了 → 块表作废，下一帧重新全量重建
    let mut scene = TerrainScene::new(TerrainSceneConfig {
        block_size: 50.0,
        ..Default::default()
    });
    assert_eq!(scene.plan(&patches, None).rebuilds, 4);
    scene.set_block_size(100.0);
    assert_eq!(
        scene.plan(&patches, None).rebuilds,
        1,
        "改成 100 米块只剩 1 块"
    );
}

/// 装配守恒：顶点索引合法、权重归一、块矩形拼起来不漏不叠。
#[test]
fn 装配结果自洽() {
    let patches = [patch("slope", [0.0, 0.0], [120.0, 80.0], 13, 5.0)];
    let mut scene = TerrainScene::new(TerrainSceneConfig {
        block_size: 40.0,
        ..Default::default()
    });
    let plan = scene.plan(&patches, None);
    assert_eq!(plan.blocks.len(), 6, "120×80 按 40 米切 = 3×2");

    for block in &plan.blocks {
        assert!(!block.indices.is_empty());
        assert_eq!(block.indices.len() % 3, 0, "三角形索引必须是 3 的倍数");
        let vcount = block.vertices.len() as u32;
        assert!(
            block.indices.iter().all(|i| *i < vcount),
            "块 {:?} 出现越界索引",
            block.index
        );
        // 定性：所有顶点权重归一
        for v in &block.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "权重未归一：{sum}");
            assert!(v.weights.iter().all(|w| *w >= 0.0));
            assert!(v.position.iter().all(|c| c.is_finite()));
            assert!(v.normal.iter().all(|c| c.is_finite()));
            // 顶点必须落在块矩形内（含边界）
            assert!(v.position[0] >= block.rect[0] - 1e-3 && v.position[0] <= block.rect[2] + 1e-3);
            assert!(v.position[2] >= block.rect[1] - 1e-3 && v.position[2] <= block.rect[3] + 1e-3);
        }
    }

    // 块矩形面积之和 = 地形总面积（不漏不叠）
    let total: f32 = plan
        .blocks
        .iter()
        .map(|b| (b.rect[2] - b.rect[0]) * (b.rect[3] - b.rect[1]))
        .sum();
    assert!((total - 120.0 * 80.0).abs() < 1e-2, "块面积和 {total}");
}
