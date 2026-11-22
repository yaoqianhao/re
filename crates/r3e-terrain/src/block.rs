//! 块划分：把一张地形矩形切成等边方格，供 LOD 选择与网格生成按块独立处理。

use crate::{DEFAULT_BLOCK_SIZE, MAX_BLOCKS, MAX_BLOCK_SIZE};

/// 一块地形在网格里的位置与它的世界矩形。
///
/// `rect` 已被裁剪到整张地形的范围内 —— 所以**块矩形的并集恰好等于整张矩形**，
/// 不重叠、不留缝（最后一行/列不会被 `block_size` 撑出去）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    /// 块索引 `[列, 行]`，从 `0` 起。
    pub index: [u32; 2],
    /// 世界坐标 `[min_x, min_z, max_x, max_z]`。
    pub rect: [f32; 4],
}

/// 等边方格切分方案。**构造即校验**：非法参数返回 `None`，不返回一个半合法的网格。
#[derive(Debug, Clone, PartialEq)]
pub struct BlockGrid {
    rect: [f32; 4],
    block_size: f32,
    dims: [u32; 2],
}

impl BlockGrid {
    /// 按 `block_size`（米）切分世界矩形 `[min_x, min_z, max_x, max_z]`。
    ///
    /// 以下任一情况返回 `None`（判定顺序即文档顺序，无隐含回退）：
    ///
    /// 1. 矩形任一分量非有限值，或 `max <= min`（零面积 / 反向）；
    /// 2. `block_size` 非有限值、`<= 0` 或 `> `[`MAX_BLOCK_SIZE`]；
    /// 3. 切出来的块数超过 [`MAX_BLOCKS`] —— **拒绝**而不是截断：
    ///    截断会让「剩下的地形」静默消失，宁可在命令层报 `false`。
    pub fn new(rect: [f32; 4], block_size: f32) -> Option<Self> {
        if !rect.iter().all(|v| v.is_finite()) {
            return None;
        }
        let (min_x, min_z, max_x, max_z) = (rect[0], rect[1], rect[2], rect[3]);
        if max_x <= min_x || max_z <= min_z {
            return None;
        }
        if !block_size.is_finite() || block_size <= 0.0 || block_size > MAX_BLOCK_SIZE {
            return None;
        }
        // 用 f64 算块数：block_size 很小时 f32 的除法会先溢出成 inf。
        let nx = ((max_x - min_x) as f64 / block_size as f64).ceil();
        let ny = ((max_z - min_z) as f64 / block_size as f64).ceil();
        // NaN 也走拒绝分支：`nx < 1.0` 对 NaN 为 false，单靠它挡不住。
        if !nx.is_finite() || !ny.is_finite() || nx < 1.0 || ny < 1.0 {
            return None;
        }
        if nx * ny > MAX_BLOCKS as f64 {
            return None;
        }
        Some(Self {
            rect,
            block_size,
            dims: [nx as u32, ny as u32],
        })
    }

    /// 按默认块边长（[`DEFAULT_BLOCK_SIZE`]）切分。
    pub fn with_default_block(rect: [f32; 4]) -> Option<Self> {
        Self::new(rect, DEFAULT_BLOCK_SIZE)
    }

    /// 整张地形的世界矩形 `[min_x, min_z, max_x, max_z]`。
    pub fn rect(&self) -> [f32; 4] {
        self.rect
    }

    /// 块边长（米）。
    pub fn block_size(&self) -> f32 {
        self.block_size
    }

    /// 块数 `[列数, 行数]`。
    pub fn dims(&self) -> [u32; 2] {
        self.dims
    }

    /// 总块数。
    pub fn block_count(&self) -> u32 {
        self.dims[0] * self.dims[1]
    }

    /// 世界坐标落在哪一块。矩形外（含 `NaN`）返回 `None`；矩形边线上的点归入内侧那块，
    /// 因此四个角点各自只命中一块，不会吃双份。
    pub fn block_at(&self, x: f32, z: f32) -> Option<[u32; 2]> {
        if !x.is_finite() || !z.is_finite() {
            return None;
        }
        if x < self.rect[0] || x > self.rect[2] || z < self.rect[1] || z > self.rect[3] {
            return None;
        }
        let i = ((x - self.rect[0]) / self.block_size).floor().max(0.0) as u32;
        let j = ((z - self.rect[1]) / self.block_size).floor().max(0.0) as u32;
        Some([i.min(self.dims[0] - 1), j.min(self.dims[1] - 1)])
    }

    /// 第 `(i, j)` 块的世界矩形（已裁到整张地形内）。索引越界返回 `None`。
    pub fn block_rect(&self, i: u32, j: u32) -> Option<[f32; 4]> {
        if i >= self.dims[0] || j >= self.dims[1] {
            return None;
        }
        let x0 = self.rect[0] + i as f32 * self.block_size;
        let z0 = self.rect[1] + j as f32 * self.block_size;
        let x1 = (x0 + self.block_size).min(self.rect[2]);
        let z1 = (z0 + self.block_size).min(self.rect[3]);
        Some([x0, z0, x1, z1])
    }

    /// 逐块枚举（行主序：先跑完一行再下一行）。顺序固定，可直接进哈希。
    pub fn blocks(&self) -> impl Iterator<Item = Block> + '_ {
        (0..self.dims[1]).flat_map(move |j| {
            (0..self.dims[0]).map(move |i| Block {
                index: [i, j],
                rect: self.block_rect(i, j).expect("枚举范围内索引必有效"),
            })
        })
    }

    /// 块矩形的并集是否**无缝且无重叠**地铺满整张地形（面积和 == 矩形面积）。
    ///
    /// 这是一条自检判据：`block_rect` 的裁剪逻辑一旦写错（少裁一块就多出面积、
    /// 多裁一块就少面积），这里立刻为 `false`。
    pub fn covers(&self) -> bool {
        let want = (self.rect[2] - self.rect[0]) as f64 * (self.rect[3] - self.rect[1]) as f64;
        let got: f64 = self
            .blocks()
            .map(|b| (b.rect[2] - b.rect[0]) as f64 * (b.rect[3] - b.rect[1]) as f64)
            .sum();
        if !want.is_finite() || want <= 0.0 {
            return false;
        }
        (got - want).abs() <= want * 1e-4
    }
}
