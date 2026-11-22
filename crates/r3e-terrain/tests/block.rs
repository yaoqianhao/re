//! 块划分行为测试（C01 · 6 条）。
//!
//! 断言的是**口径**而不是实现细节：多少块、边界怎么算、非法参数是否拒绝、
//! 块矩形是否真的无缝铺满。改坏任何一条都是能力退化。

use r3e_terrain::{BlockGrid, DEFAULT_BLOCK_SIZE, MAX_BLOCKS};

fn rect200() -> [f32; 4] {
    [0.0, 0.0, 200.0, 200.0]
}

#[test]
fn two_hundred_meters_with_default_block_is_four_by_four_and_covers() {
    let grid = BlockGrid::new(rect200(), DEFAULT_BLOCK_SIZE).expect("200/50 必须合法");
    assert_eq!(grid.dims(), [4, 4], "200 米按 50 米切必须是 4×4");
    assert_eq!(grid.block_count(), 16, "D10 口径：4×4 = 16 块");
    assert_eq!(grid.block_size(), DEFAULT_BLOCK_SIZE);
    assert!(grid.covers(), "块矩形必须无缝铺满整张地形");
}

#[test]
fn illegal_block_size_is_rejected() {
    for bs in [0.0f32, -1.0, -0.5, f32::NAN, f32::INFINITY] {
        assert!(
            BlockGrid::new(rect200(), bs).is_none(),
            "block_size = {bs} 必须拒绝，不能静默回退默认值"
        );
    }
    // 矩形本身非法同样拒绝：零面积 / 反向 / 非有限
    assert!(BlockGrid::new([0.0, 0.0, 0.0, 10.0], 50.0).is_none());
    assert!(BlockGrid::new([10.0, 0.0, 0.0, 10.0], 50.0).is_none());
    assert!(BlockGrid::new([0.0, 0.0, f32::NAN, 10.0], 50.0).is_none());
}

#[test]
fn too_many_blocks_is_rejected_not_truncated() {
    // 200 / 0.4 = 500 ✕ 500：远超上限，必须整体拒绝
    assert!(BlockGrid::new(rect200(), 0.4).is_none());
    // 刚好 32 块是合法的（上限本身不吃掉）
    let limit = BlockGrid::new([0.0, 0.0, 80.0, 40.0], 10.0).expect("8×4 = 32 块合法");
    assert_eq!(limit.block_count(), MAX_BLOCKS);
    // 36 块就该拒绝，而不是截断成 32 块让边缘地形静默消失
    assert!(BlockGrid::new([0.0, 0.0, 90.0, 40.0], 10.0).is_none());
    // 块边长超过 MAX_BLOCK_SIZE 也拒绝
    assert!(BlockGrid::new(rect200(), 1000.1).is_none());
}

#[test]
fn point_outside_rect_has_no_block() {
    let grid = BlockGrid::new(rect200(), 50.0).unwrap();
    assert!(grid.block_at(-0.1, 10.0).is_none(), "左侧出界");
    assert!(grid.block_at(10.0, 200.1).is_none(), "上侧出界");
    assert!(grid.block_at(f32::NAN, 10.0).is_none(), "NaN 不得 panic");
    assert_eq!(grid.block_at(0.0, 0.0), Some([0, 0]));
    assert_eq!(
        grid.block_at(200.0, 200.0),
        Some([3, 3]),
        "上边界归入最后一格"
    );
    assert_eq!(grid.block_at(25.0, 25.0), Some([0, 0]));
    assert_eq!(
        grid.block_at(50.0, 50.0),
        Some([1, 1]),
        "块边界归入右上那一块"
    );
}

#[test]
fn block_rects_tile_the_whole_rect_area_exactly_once() {
    // 120×80 / 50 → 3 列 2 行，最后一行/列被裁到矩形内
    let grid = BlockGrid::new([0.0, 0.0, 120.0, 80.0], 50.0).unwrap();
    assert_eq!(grid.dims(), [3, 2]);

    let (w, d) = (120usize, 80usize);
    let mut count = vec![0u32; w * d];
    for b in grid.blocks() {
        for z in (b.rect[1] as i32)..(b.rect[3] as i32) {
            for x in (b.rect[0] as i32)..(b.rect[2] as i32) {
                count[z as usize * w + x as usize] += 1;
            }
        }
    }
    assert!(
        count.iter().all(|c| *c == 1),
        "每个 1×1 米格子必须恰好被一块覆盖（既不重叠也不留缝）"
    );

    let sum: f32 = grid
        .blocks()
        .map(|b| (b.rect[2] - b.rect[0]) * (b.rect[3] - b.rect[1]))
        .sum();
    assert!(
        (sum - 120.0 * 80.0).abs() < 1e-2,
        "块面积和必须等于矩形面积"
    );
    assert!(grid.covers());
}

#[test]
fn four_corner_points_each_hit_one_distinct_block() {
    let grid = BlockGrid::new(rect200(), 50.0).unwrap();
    let corners = [
        (0.0f32, 0.0f32, [0u32, 0u32]),
        (200.0, 0.0, [3, 0]),
        (0.0, 200.0, [0, 3]),
        (200.0, 200.0, [3, 3]),
    ];
    let mut seen = Vec::new();
    for (x, z, want) in corners {
        let got = grid.block_at(x, z).expect("角点在矩形内必须命中一块");
        assert_eq!(got, want, "角点 ({x}, {z}) 归块错误");
        assert_eq!(
            grid.block_at(x, z),
            Some(got),
            "同一点两次必须得到同一块（不吃双份、不随机）"
        );
        assert!(
            grid.block_rect(want[0], want[1]).is_some(),
            "命中的块必须有合法矩形"
        );
        seen.push(got);
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 4, "四个角点必须落在四个不同的块上");
}
