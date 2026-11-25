//! 产物回读测试（`M2·D04`）：`NavProduct` → 字节 → `NavProduct` → `Nav`。
//!
//! D01 已经把「烘焙 → 产物 → 字节」这一段钉住了（`tests/product.rs`），本文件补的是
//! 最后一段：**字节还能不能变回一张能查的网格**。判据只有一条，但要求逐位相等：
//! 恢复出来的 `product_hash` 必须和写盘前一模一样。哈希里折了格高、可走位图、
//! 逐格分类、连通块标注与两级摘要，任何一格被「重判」过都会立刻变号——所以它同时
//! 盯住了「别在 restore 里偷偷重算」这件事。
//!
//! 为什么单独开文件：`tests/product.rs` 已经贴着 800 行硬线，`grid.rs` 同理，
//! 新判据按 §3.9 落新文件，不动既有文件的行数。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{Nav, NavError, NavProduct};

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
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 判据 5（`.navmesh` 格式自洽）：导出 → 导入后产物哈希不变，且网格逐格一致。
///
/// 断言分三层，故意都在同一条用例里：
///
/// 1. **哈希不变**——盖住「所有进哈希的字段都没被改动」；
/// 2. **可走格数 / 连通块数 / 分类直方图不变**——万一哈希算法本身漏折了某个字段，
///    这三项是独立算出来的第二道证据；
/// 3. **查询口径不变**——恢复出来的网格能直接回答「这点可走吗 / 高度多少 / 在哪个块」，
///    且答案与内存里那份一致。只比字节不比查询，等于没验证它「还能用」。
#[test]
fn imported_product_restores_same_grid() {
    let scene = scene_with_wall();
    let baked = Nav::bake_scene(&scene).expect("平地 + 一面墙必须能烘出来");
    let before = baked.product();
    let hash_before = baked.product_hash();

    // 走一遍真实载体：内存 → 字节 → 内存。
    let bytes = before.to_bytes();
    let parsed = NavProduct::from_bytes(&bytes).expect("自己写出来的字节必须能读回来");
    let restored = Nav::from_product(parsed).expect("合法产物必须能恢复成网格");

    // 第 1 层：逐位相同。
    assert_eq!(
        restored.product_hash(),
        hash_before,
        "导入后产物哈希必须与导出前逐位相同"
    );
    // 第 2 层：独立算出来的统计量。
    assert_eq!(
        restored.report().walkable,
        baked.report().walkable,
        "可走格数必须一致"
    );
    assert_eq!(
        restored.report().regions,
        baked.report().regions,
        "连通块数必须一致"
    );
    assert_eq!(
        restored.grid().class_histogram(),
        baked.grid().class_histogram(),
        "分类直方图必须一致"
    );
    // 第 3 层：查询口径。
    let (grid_before, grid_after) = (baked.grid(), restored.grid());
    for (i, j) in [(0u32, 0u32), (10, 10), (23, 23)] {
        assert_eq!(
            grid_after.region_at(i, j),
            grid_before.region_at(i, j),
            "({i}, {j}) 的连通块标注必须一致"
        );
        let center = grid_before.cell_center(i, j);
        let ground = grid_before.ground_at(center).expect("格中心必须落在网格内");
        let probe = Vec3::new(center.x, ground, center.z);
        assert_eq!(
            grid_after.is_walkable(probe),
            grid_before.is_walkable(probe),
            "({i}, {j}) 的可走判定必须一致"
        );
    }
    // 报告里刻意留空的字段：出生点是场景信息，不在产物里，不能凭空补。
    assert!(
        restored.report().spawns.is_empty(),
        "导入后出生点表必须为空（产物不含场景侧信息）"
    );
}

/// 反向用例：尺寸非法的产物必须被拒，而不是恢复出一张半张网格。
///
/// 这条盯的是 `from_product` 的守卫顺序——先看 `dims` 再看格数，否则
/// `dims[0] * dims[1]` 溢出或 0 会先被 `NavGrid::restore` 的 `debug_assert` 吃掉
/// （release 下则直接越界），错误类型就变成 panic 而不是 `NavError`。
#[test]
fn degenerate_product_is_rejected() {
    let scene = scene_with_wall();
    let baked = Nav::bake_scene(&scene).expect("烘焙必须成功");
    let mut bytes = baked.product().to_bytes();
    // 头布局见 `product.rs::to_bytes`：魔数(4B) + 版本(2B) + dims_x(4B) + dims_y(4B) + …
    // 把 dims_y 写成 0（偏移 10..14），制造一个「格数 = 0」的产物。
    bytes[10..14].copy_from_slice(&0u32.to_le_bytes());
    match NavProduct::from_bytes(&bytes) {
        // 解析层自己就拒了（长度校验不过），也算达成目的：绝不返回一个能用的产物。
        Err(NavError::ProductMismatch { .. }) => {}
        // 若解析层放行（例如它只校验总长度），`from_product` 必须接着拒。
        Ok(product) => {
            assert!(
                matches!(
                    Nav::from_product(product),
                    Err(NavError::ProductMismatch { .. })
                ),
                "尺寸非法的产物必须被 from_product 拒绝"
            );
        }
        Err(other) => panic!("错误类型必须是 ProductMismatch，实际：{other:?}"),
    }
}
