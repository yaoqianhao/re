//! 程序化生成的行为测试（C05 · 9 条）。
//!
//! 这一批测的都是**可判定的硬性质**：逐位可复现、接缝连续、覆盖无洞、非法输入不
//! 静默降级。生成算法长什么样（几层正弦、噪声怎么算）不在断言范围内 —— 那种断言
//! 一改参数就得跟着改，锁不住任何东西。
//!
//! 第 9 条用整张地形的指纹（高度位模式上的 FNV-1a）代替 `r3e-doc::state_hash`：
//! `r3e-terrain` 不依赖 `r3e-doc`（派生层只许向下依赖 core），「生成 → 写场景 →
//! 重放同序列得到同一 `state_hash`」由 C06 的 `terrain.gen` 回包测试在宿主侧锁。

use r3e_terrain::{
    generate, BlockGrid, DEFAULT_BLOCK_SIZE, GENERATE_MAX_HEIGHT, GENERATE_MIN_HEIGHT,
    GENERATE_RESOLUTION, MAX_BLOCKS, MAX_GENERATE_SIZE,
};

/// 200 × 200 米：4 × 4 = 16 块，与 D10 同一口径。
fn sample_terrain(seed: u64) -> Vec<r3e_core::TerrainPatch> {
    generate(200.0, 200.0, seed).expect("200×200 必须在允许范围内")
}

#[test]
fn same_seed_generates_bit_identical_patches() {
    let a = sample_terrain(42);
    let b = sample_terrain(42);
    assert_eq!(a.len(), b.len(), "同 seed 块数必须一致");
    for (pa, pb) in a.iter().zip(&b) {
        assert_eq!(pa.name, pb.name);
        assert_eq!(pa.origin, pb.origin);
        assert_eq!(pa.size, pb.size);
        assert_eq!(pa.resolution, pb.resolution);
        // 逐位比较：`==` 对 f32 而言 NaN 不等于自身，这里如果混进 NaN 会立刻暴露。
        assert_eq!(pa.heights.len(), pb.heights.len());
        for (ha, hb) in pa.heights.iter().zip(&pb.heights) {
            assert_eq!(
                ha.to_bits(),
                hb.to_bits(),
                "同 seed 同索引的高度必须逐位相同（{} 处 {ha} vs {hb}）",
                pa.name
            );
        }
    }
}

#[test]
fn different_seed_changes_samples() {
    let a = sample_terrain(42);
    let b = sample_terrain(43);
    let mut changed = 0usize;
    for (pa, pb) in a.iter().zip(&b) {
        changed += pa
            .heights
            .iter()
            .zip(&pb.heights)
            .filter(|(x, y)| x.to_bits() != y.to_bits())
            .count();
    }
    assert!(
        changed > 0,
        "换 seed 必须至少改一个采样点，否则种子没接进高度"
    );
    assert!(
        fingerprint(&a) != fingerprint(&b),
        "换 seed 后整张地形指纹必须不同"
    );
}

#[test]
fn block_count_resolution_and_capacity_are_within_limits() {
    let patches = sample_terrain(7);
    let grid = BlockGrid::new([0.0, 0.0, 200.0, 200.0], DEFAULT_BLOCK_SIZE).unwrap();
    assert_eq!(grid.dims(), [4, 4]);
    assert_eq!(patches.len(), 16, "200×200 / 50 米必须正好 16 块");
    assert!(
        patches.len() as u32 <= MAX_BLOCKS,
        "块数 {} 超过 MAX_BLOCKS = {MAX_BLOCKS}",
        patches.len()
    );
    assert!(
        patches.len() <= r3e_core::TERRAIN_MAX_PATCHES,
        "块数超过场景能容纳的 TERRAIN_MAX_PATCHES"
    );
    for p in &patches {
        assert_eq!(p.resolution, [GENERATE_RESOLUTION, GENERATE_RESOLUTION]);
        assert!(
            p.resolution[0] <= 128 && p.resolution[1] <= 128,
            "{} 的 resolution 超出 TERRAIN_MAX_RESOLUTION",
            p.name
        );
        assert_eq!(
            p.heights.len(),
            (p.resolution[0] * p.resolution[1]) as usize,
            "{} 的样本数与 resolution 不自洽",
            p.name
        );
    }
}

#[test]
fn block_rects_cover_requested_size_without_gap() {
    let patches = sample_terrain(42);
    let grid = BlockGrid::new([0.0, 0.0, 200.0, 200.0], DEFAULT_BLOCK_SIZE).unwrap();
    assert!(grid.covers(), "块划分自身必须无缝无重叠");

    // 面积法：块的并集面积必须等于请求面积（无洞无重叠）。
    let area: f32 = patches.iter().map(|p| p.size[0] * p.size[1]).sum();
    assert!(
        (area - 200.0 * 200.0).abs() < 1e-3,
        "块面积之和 {area} 应等于 200×200 = 40000"
    );

    // 逐块对照 rect：生成顺序与 `BlockGrid::blocks()` 一致，矩形逐位相等。
    for (p, b) in patches.iter().zip(grid.blocks()) {
        assert_eq!(
            p.origin,
            [b.rect[0], b.rect[1]],
            "{} 的 origin 与块划分不符",
            p.name
        );
        assert_eq!(
            p.size,
            [b.rect[2] - b.rect[0], b.rect[3] - b.rect[1]],
            "{} 的 size 与块划分不符",
            p.name
        );
        assert!(
            p.size[0] > 0.0 && p.size[1] > 0.0,
            "{} 尺寸必须为正",
            p.name
        );
    }
}

#[test]
fn generated_patches_are_already_sanitized() {
    for p in sample_terrain(99) {
        let before = p.clone();
        let after = p.sanitized();
        assert_eq!(after.name, before.name, "名字不该被 sanitize 改动");
        assert_eq!(after.origin, before.origin);
        assert_eq!(after.size, before.size);
        assert_eq!(after.resolution, before.resolution);
        assert_eq!(after.heights.len(), before.heights.len());
        for (h0, h1) in before.heights.iter().zip(&after.heights) {
            assert_eq!(
                h0.to_bits(),
                h1.to_bits(),
                "生成物依赖 sanitized 兜数据：{} 的样本被改写了",
                before.name
            );
        }
    }
}

#[test]
fn seed_extremes_are_reproducible() {
    for seed in [0u64, u64::MAX, 1] {
        let a = sample_terrain(seed);
        let b = sample_terrain(seed);
        assert_eq!(
            fingerprint(&a),
            fingerprint(&b),
            "seed = {seed} 两次生成必须同指纹"
        );
        assert_eq!(a.len(), 16, "seed = {seed} 块数不该变");
    }
    assert_ne!(
        fingerprint(&sample_terrain(0)),
        fingerprint(&sample_terrain(u64::MAX)),
        "seed = 0 与 seed = u64::MAX 必须给出不同地形"
    );
}

#[test]
fn invalid_sizes_are_rejected_without_partial_output() {
    // 零、负、超上限、非有限值 —— 全部 `None`，不返回「剩下多少算多少」的半张地形。
    for (sx, sz) in [
        (0.0f32, 0.0f32),
        (0.0, 200.0),
        (200.0, 0.0),
        (-1.0, 200.0),
        (200.0, -50.0),
        (MAX_GENERATE_SIZE + 1.0, 200.0),
        (200.0, MAX_GENERATE_SIZE + 1.0),
        (f32::NAN, 200.0),
        (200.0, f32::INFINITY),
        (f32::NEG_INFINITY, 200.0),
    ] {
        assert!(
            generate(sx, sz, 42).is_none(),
            "尺寸 ({sx}, {sz}) 必须被拒，实际返回了地形"
        );
    }
    // 边界值本身合法（`<=` 而非 `<`）：上限尺寸块数也会超，但拒的理由应是块数而不是尺寸。
    assert!(
        generate(MAX_GENERATE_SIZE, MAX_GENERATE_SIZE, 42).is_none(),
        "1024×1024 切 50 米是 21×21 = 441 块，必超 MAX_BLOCKS"
    );
    assert!(
        generate(150.0, 150.0, 42).is_some(),
        "3×3 = 9 块的尺寸必须能生成"
    );
}

#[test]
fn heights_stay_in_range_and_seams_match() {
    let patches = sample_terrain(42);
    for p in &patches {
        for (i, h) in p.heights.iter().enumerate() {
            assert!(h.is_finite(), "{} 第 {i} 个样本不是有限值", p.name);
            assert!(
                (GENERATE_MIN_HEIGHT..=GENERATE_MAX_HEIGHT).contains(h),
                "{} 第 {i} 个样本 {h} 越界",
                p.name
            );
        }
    }

    // 接缝：左右相邻块在共享边界上取同一条样本列（前块最后一列 == 后块第一列），
    // 上下相邻块同理。这是「跨块行走无卡顿」的地基，破了下游 C07 会直接抖。
    let grid = BlockGrid::new([0.0, 0.0, 200.0, 200.0], DEFAULT_BLOCK_SIZE).unwrap();
    let res = GENERATE_RESOLUTION as usize;
    let by_index: std::collections::HashMap<[u32; 2], &r3e_core::TerrainPatch> = patches
        .iter()
        .zip(grid.blocks())
        .map(|(p, b)| (b.index, p))
        .collect();

    for b in grid.blocks() {
        let [i, j] = b.index;
        let cur = by_index[&[i, j]];
        if let Some(right) = by_index.get(&[i + 1, j]) {
            for row in 0..res {
                let left_edge = cur.heights[row * res + (res - 1)];
                let right_edge = right.heights[row * res];
                assert_eq!(
                    left_edge.to_bits(),
                    right_edge.to_bits(),
                    "块 ({i},{j}) 与右邻在 row {row} 的接缝高度不一致"
                );
            }
        }
        if let Some(up) = by_index.get(&[i, j + 1]) {
            for col in 0..res {
                let bottom_edge = cur.heights[(res - 1) * res + col];
                let top_edge = up.heights[col];
                assert_eq!(
                    bottom_edge.to_bits(),
                    top_edge.to_bits(),
                    "块 ({i},{j}) 与上邻在 col {col} 的接缝高度不一致"
                );
            }
        }
    }
}

#[test]
fn whole_terrain_fingerprint_is_stable() {
    // 指纹覆盖「块名 + origin + size + resolution + 每个样本的位模式」，
    // 任何一处漂移都会改数字。C06 接上 `state_hash` 之前，用它当回归锚点。
    let a = fingerprint(&sample_terrain(42));
    let b = fingerprint(&sample_terrain(42));
    assert_eq!(a, b, "同一份地形两次生成必须同指纹");

    let expected = fingerprint(&sample_terrain(42));
    assert_eq!(a, expected, "指纹必须可重复计算（无隐藏状态）");

    // 改一个采样维度（尺寸变化）必须改指纹，否则指纹覆盖不全。
    let other = fingerprint(&generate(150.0, 150.0, 42).unwrap());
    assert_ne!(a, other, "不同尺寸的地形不能撞指纹");
}

#[test]
fn non_divisible_size_still_tiles_without_gap() {
    // 175 = 3 × 50 + 25：最后一块只有 25 米宽，且整张不是 `block_size` 的整数倍。
    // 这条盯的是「不为整除尺寸开特例」—— 生成器只按 `BlockGrid` 的裁定走，
    // 覆盖面积仍必须严丝合缝，接缝仍必须逐位相等。
    let size = 175.0f32;
    let patches = generate(size, size, 42).expect("175×175 必须能生成");
    let grid = BlockGrid::new([0.0, 0.0, size, size], DEFAULT_BLOCK_SIZE).unwrap();
    assert_eq!(grid.dims(), [4, 4]);
    assert_eq!(patches.len(), 16);

    let area: f32 = patches.iter().map(|p| p.size[0] * p.size[1]).sum();
    assert!(
        (area - size * size).abs() < 1e-2,
        "块面积之和 {area} 应等于 175×175 = 30625"
    );

    let res = GENERATE_RESOLUTION as usize;
    let by_index: std::collections::HashMap<[u32; 2], &r3e_core::TerrainPatch> = patches
        .iter()
        .zip(grid.blocks())
        .map(|(p, b)| (b.index, p))
        .collect();
    for b in grid.blocks() {
        let [i, j] = b.index;
        let cur = by_index[&[i, j]];
        if let Some(right) = by_index.get(&[i + 1, j]) {
            for row in 0..res {
                assert_eq!(
                    cur.heights[row * res + (res - 1)].to_bits(),
                    right.heights[row * res].to_bits(),
                    "非整除尺寸下块 ({i},{j}) 与右邻接缝不一致"
                );
            }
        }
    }
}

/// 整张地形的 FNV-1a 指纹（`u64`）。
fn fingerprint(patches: &[r3e_core::TerrainPatch]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for p in patches {
        eat(p.name.as_bytes());
        for v in p.origin.iter().chain(p.size.iter()) {
            eat(&v.to_bits().to_le_bytes());
        }
        eat(&p.resolution[0].to_le_bytes());
        eat(&p.resolution[1].to_le_bytes());
        for h in &p.heights {
            eat(&h.to_bits().to_le_bytes());
        }
    }
    hash
}
