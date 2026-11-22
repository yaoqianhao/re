//! 程序化地形生成（C05）：`尺寸 + seed` → 一组 [`TerrainPatch`]。
//!
//! 本模块是**纯函数**：同样的入参算两次逐位相同，不读时钟、不读随机源、不碰全局
//! 状态、不看块遍历顺序。生成的是 `core::TerrainPatch`（场景唯一真值），**不是网格** ——
//! 网格仍由 [`crate::block_mesh`] 现算（CD-2 派生层口径），所以生成物可以直接进
//! 场景、进 `state_hash`、进撤销栈。
//!
//! ## 两个口径（都为了「跨块无缝」）
//!
//! 1. **基高度只看世界坐标**：三层正弦叠加，同一点必同值，与块划分无关；
//! 2. **微扰只看全局样本索引**：`SplitMix64(seed, gx, gz)`。不用世界坐标做噪声输入，
//!    是因为块边界两侧的 x 由两条不同的浮点表达式算出，可能差 1 ulp —— 一旦拿它喂
//!    噪声，接缝两侧就是两个不相干的高度，地面会裂开一条可走的缝。
//!
//! 采样点按**整张地形的统一网格**定位（`gx / 总段数`），块边界两侧的采样点因此
//! 落在同一个世界坐标上、吃同一个 `gx`，高度逐位相等。代价是当整张尺寸不是
//! `block_size` 的整数倍时，最后一块内的采样间距与「均匀分布」有极小偏差
//! （被裁剪掉的那一小段摊到 63 个间隔里），换绝对无缝，值这个价。
//!
//! ## 尺寸与预算
//!
//! 200 × 200 米 → 4 × 4 = 16 块 × 64 × 64 采样 = 65536 个高度样本；
//! 单轴上限 [`MAX_GENERATE_SIZE`]、块数上限由 [`BlockGrid`] 的 [`crate::MAX_BLOCKS`]
//! 把关 —— 超限一律返回 `None`，**不截断**（截断会让「剩下的地形」静默消失）。

use r3e_core::TerrainPatch;

use crate::{BlockGrid, DEFAULT_BLOCK_SIZE, MAX_BLOCKS};

/// 单轴尺寸上限（米）。再大块数就超 [`crate::MAX_BLOCKS`]，直接拒。
pub const MAX_GENERATE_SIZE: f32 = 1024.0;

/// 生成时每块每轴的采样点数。`64 ≤ 128`（`TERRAIN_MAX_RESOLUTION`），
/// 且接缝靠「共享端点样本」实现，段数 = 采样点数 - 1。
pub const GENERATE_RESOLUTION: u32 = 64;

/// 每块的采样点数（`GENERATE_RESOLUTION²`），供调用方估算量级。
pub const GENERATE_SAMPLES_PER_BLOCK: usize = (GENERATE_RESOLUTION * GENERATE_RESOLUTION) as usize;

/// 高度下界（米），超出的截断。
pub const GENERATE_MIN_HEIGHT: f32 = -20.0;

/// 高度上界（米），超出的截断。
pub const GENERATE_MAX_HEIGHT: f32 = 120.0;

/// 噪声微扰幅度（米）：取值为 `±GENERATE_JITTER`，峰峰值 `2 ×`。**≤ 0.05**，
/// 永远盖不过正弦起伏，只是让「同 seed 的地形」看起来不像数学题。
pub const GENERATE_JITTER: f64 = 0.05;

/// 生成整张地形：`size_x × size_z` 米、以 `(0, 0)` 为左下角、种子 `seed`。
///
/// 返回按行主序排列的块（先跑完一行再下一行），块名 `terrain_<列>_<行>`。
///
/// 以下任一情况返回 `None`（判定顺序即文档顺序，无隐含回退）：
///
/// 1. 尺寸非有限值、`<= 0` 或 `> `[`MAX_GENERATE_SIZE`]；
/// 2. 按 [`DEFAULT_BLOCK_SIZE`] 切出来的块数超 [`MAX_BLOCKS`]（[`BlockGrid`] 判）。
///
/// 生成物**已经是收敛过的**：`heights` 长度 == `resolution.0 * resolution.1`、
/// 全部是有限值、都在 `[GENERATE_MIN_HEIGHT, GENERATE_MAX_HEIGHT]` 内，
/// 所以 `sanitized()` 走一遍必须原样返回（`tests/generate.rs` 锁这条）。
pub fn generate(size_x: f32, size_z: f32, seed: u64) -> Option<Vec<TerrainPatch>> {
    let grid = grid_for(size_x, size_z)?;
    let [nx, ny] = grid.dims();
    let rect = grid.rect();
    let res = GENERATE_RESOLUTION;
    // 段数 = 采样点数 - 1：相邻两块共享边界上的那一个采样点，接缝才没有缝。
    let seg = res - 1;
    let total_x = nx * seg;
    let total_z = ny * seg;

    let mut patches = Vec::with_capacity(grid.block_count() as usize);
    for block in grid.blocks() {
        let [bi, bj] = block.index;
        let mut heights = Vec::with_capacity(GENERATE_SAMPLES_PER_BLOCK);
        for row in 0..res {
            let gz = bj * seg + row;
            let z = axis_pos(rect[1], size_z, gz, total_z);
            for col in 0..res {
                let gx = bi * seg + col;
                let x = axis_pos(rect[0], size_x, gx, total_x);
                heights.push(height_at(x, z, seed, gx, gz));
            }
        }
        let mut patch = TerrainPatch::flat(
            format!("terrain_{bi}_{bj}"),
            [block.rect[0], block.rect[1]],
            [block.rect[2] - block.rect[0], block.rect[3] - block.rect[1]],
        );
        patch.resolution = [res, res];
        patch.heights = heights;
        patches.push(patch);
    }
    Some(patches)
}

/// 尺寸校验 + 切块方案。非法尺寸返回 `None`，不返回半个网格。
fn grid_for(size_x: f32, size_z: f32) -> Option<BlockGrid> {
    if !size_x.is_finite() || !size_z.is_finite() {
        return None;
    }
    if size_x <= 0.0 || size_z <= 0.0 {
        return None;
    }
    if size_x > MAX_GENERATE_SIZE || size_z > MAX_GENERATE_SIZE {
        return None;
    }
    let grid = BlockGrid::new([0.0, 0.0, size_x, size_z], DEFAULT_BLOCK_SIZE)?;
    // `BlockGrid::new` 已经拦了块数上限；这里再断言一次，是为了把「生成器依赖这个
    // 上限」写成代码而不是写成注释 —— 上限一旦松动，这里立刻是 `None` 而不是
    // 一张 2000 块的地形把哈希和快照撑爆。
    if grid.block_count() > MAX_BLOCKS {
        return None;
    }
    Some(grid)
}

/// 第 `g` 个采样点的世界坐标（`g` 取 `0..=total`，`total` 处正好落在右/上边界）。
///
/// 用 `f64` 算完再落 `f32`：`g / total * size` 在 `f32` 下会出现「相邻两个采样点
/// 算出同一个坐标」（大尺寸 + 大 `total` 时），那等于白白丢掉一个采样点。
fn axis_pos(origin: f32, size: f32, g: u32, total: u32) -> f32 {
    if total == 0 {
        return origin;
    }
    (origin as f64 + (g as f64 / total as f64) * size as f64) as f32
}

/// 采样点 `(gx, gz)` 处的高度：三层正弦叠加 + 按索引的微扰，再截到合法区间。
fn height_at(x: f32, z: f32, seed: u64, gx: u32, gz: u32) -> f32 {
    let (xf, zf) = (x as f64, z as f64);
    // 三层不同波长：大起伏（~200 米周期）/ 中坡（~90 米）/ 碎石（~33 米）。
    let base = 3.0 * (xf * 0.031).sin() * (zf * 0.027).cos()
        + 1.5 * ((xf + zf) * 0.071).sin()
        + 0.6 * (xf * 0.19).sin() * (zf * 0.17).sin();
    let jitter = (unit_noise(seed, gx, gz) - 0.5) * 2.0 * GENERATE_JITTER;
    // 先叠加再截断：反过来（先截断再加微扰）会漏出边界外 0.05 米。
    (base + jitter).clamp(GENERATE_MIN_HEIGHT as f64, GENERATE_MAX_HEIGHT as f64) as f32
}

/// `SplitMix64`：一次混合就够当噪声源，不需要周期长的伪随机序列。
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `[0, 1)` 的确定性噪声：输入只有 `seed` 与全局样本索引，**不含坐标浮点**。
///
/// 取 `SplitMix64` 的高 53 位再除 `2^53`，保证映射到 `f64` 时每个尾数位都用上。
fn unit_noise(seed: u64, gx: u32, gz: u32) -> f64 {
    let pair = ((gx as u64) << 32) | gz as u64;
    let h = splitmix64(seed ^ splitmix64(pair));
    (h >> 11) as f64 / (1u64 << 53) as f64
}
