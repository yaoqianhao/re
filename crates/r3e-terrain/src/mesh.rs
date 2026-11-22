//! 分块网格生成（M2-C03a）：一块地形 → 顶点 / 法线 / uv / 4 层权重 / 索引。
//!
//! 产出是**纯数据**（裸 `Vec`），不引用 `r3e-render` 任何类型 —— 转 `MeshData`、挂管线
//! 都是 C03b 在 `r3e-runtime` 装配层做的事。这样整个几何与权重口径可以在 headless
//! 下逐条判定，不依赖 GPU 与截图。
//!
//! ## 顶点从哪来
//!
//! 顶点高度一律取 [`crate::sample_height_lod`]，不另算一遍：网格是「同一份地形的
//! 另一种表达」，两边各写一套插值迟早漂移。于是 §5 的接缝规则自动延续到网格上——
//! 边界带强制细档，相邻块共享边界线时两侧顶点逐位相等。
//!
//! 顶点密度由 **patch 自己的采样步长 × LOD 步长** 决定（块边长除以它向上取整），
//! 所以块边长 50 米、patch 步长 1.5625 米时：`L0` 32 段、`L1` 16 段、`L2` 8 段，
//! 三角数正好差 16 倍。尾块窄一些就自然少几段，不做特例。
//!
//! ## 权重怎么定（不引入新数据源）
//!
//! 4 层（草 / 岩 / 沙 / 雪）由「高度 + 坡度」程序化判定，配置见
//! [`TerrainLayerConfig`]。每层先算一个 0..=1 的原始分，再按和归一化；
//! **和恰为 0**（配置把四层全压掉）时退化为「层 0 = 1」，绝不留出权重和不为 1 的顶点。

use r3e_core::TerrainPatch;

use crate::{sample_height_lod, BlockGrid, Lod};

/// 一块地形的网格数据。顶点属性一一对应，索引指向 `positions`。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMesh {
    /// 顶点世界坐标（行主序：先跑完一行 `i` 再进下一行 `j`）。
    pub positions: Vec<[f32; 3]>,
    /// 顶点法线（单位化，朝向天空一侧）。
    pub normals: Vec<[f32; 3]>,
    /// 顶点 uv，块内归一化到 `[0, 1]`。
    pub uvs: Vec<[f32; 2]>,
    /// 4 层混合权重，逐顶点和为 1。
    pub weights: Vec<[f32; 4]>,
    /// 三角形索引（每格两个三角，绕序使法线朝上）。
    pub indices: Vec<u32>,
    /// 生成时用的档位。
    pub lod: Lod,
    /// 网格段数 `[列数, 行数]`：顶点数是 `(列+1) × (行+1)`。
    pub segments: [u32; 2],
}

/// 4 层材质权重的判定配置。全部可配，默认值即 §6.2 的表格。
///
/// `fade` 是过渡带宽（坡度用「度」、高度用「米」，共用一个数值窗口）：
/// 阈值两侧各 `fade` 的范围内线性过渡，避免坡度/高度一跨阈值就从草直接跳到岩。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainLayerConfig {
    /// 层 0（草）能接受的最大坡度（度）。
    pub grass_slope_max: f32,
    /// 层 0（草）能接受的最大高度（米）。
    pub grass_height_max: f32,
    /// 层 1（岩）的起始坡度（度）。
    pub rock_slope_min: f32,
    /// 层 2（沙）的高度上限（米）。
    pub sand_height_max: f32,
    /// 层 3（雪）的高度下限（米）。
    pub snow_height_min: f32,
    /// 过渡带宽。
    pub fade: f32,
}

impl Default for TerrainLayerConfig {
    fn default() -> Self {
        Self {
            grass_slope_max: 25.0,
            grass_height_max: 80.0,
            rock_slope_min: 25.0,
            sand_height_max: 5.0,
            snow_height_min: 80.0,
            fade: 6.0,
        }
    }
}

impl TerrainLayerConfig {
    /// 过渡带宽；非有限或非正值一律按 1.0 处理，绝不让除零溜进归一化。
    fn fade(&self) -> f32 {
        if self.fade.is_finite() && self.fade > 0.0 {
            self.fade
        } else {
            1.0
        }
    }

    /// 由高度与坡度算 4 层权重（和为 1）。
    ///
    /// 四层原始分全为 0（配置互相压死）时退化为层 0 = 1 —— 网格里不存在
    /// 「权重和是 0」的顶点，着色器也就不必再兜这个底。
    pub fn weights(&self, height: f32, slope_deg: f32) -> [f32; 4] {
        let fade = self.fade();
        let ramp = |v: f32| (v / fade).clamp(0.0, 1.0);
        let grass = ramp(self.grass_slope_max - slope_deg) * ramp(self.grass_height_max - height);
        let rock = ramp(slope_deg - self.rock_slope_min);
        let sand = ramp(self.sand_height_max - height);
        let snow = ramp(height - self.snow_height_min);
        let sum = grass + rock + sand + snow;
        if !sum.is_finite() || sum <= 0.0 {
            return [1.0, 0.0, 0.0, 0.0];
        }
        [grass / sum, rock / sum, sand / sum, snow / sum]
    }
}

/// 与块矩形**有正面积重叠**的第一个 patch（按切片顺序，结果因此确定）。
fn patch_of_block(patches: &[TerrainPatch], rect: [f32; 4]) -> Option<&TerrainPatch> {
    patches.iter().find(|p| {
        let (w, d) = (p.size[0], p.size[1]);
        if !w.is_finite() || !d.is_finite() || w <= 0.0 || d <= 0.0 {
            return false;
        }
        let ix = (rect[2].min(p.origin[0] + w) - rect[0].max(p.origin[0])).min(w);
        let iz = (rect[3].min(p.origin[1] + d) - rect[1].max(p.origin[1])).min(d);
        ix > 0.0 && iz > 0.0
    })
}

/// 一段网格的采样步长（米）：patch 在该轴上的采样间距。
fn patch_cell(patch: &TerrainPatch, axis: usize) -> f32 {
    let n = patch.resolution[axis];
    if n >= 2 {
        patch.size[axis] / (n - 1) as f32
    } else {
        patch.size[axis]
    }
}

/// 生成块 `index` 在档位 `lod` 下的网格。
///
/// 返回 `None` 的条件（都不生成「空网格」冒充成功）：
/// 块索引越界、该块没有任何 patch 覆盖、挂到块上的 patch 尺寸非法。
pub fn block_mesh(
    patches: &[TerrainPatch],
    grid: &BlockGrid,
    index: [u32; 2],
    lod: Lod,
    cfg: &TerrainLayerConfig,
) -> Option<TerrainMesh> {
    let rect = grid.block_rect(index[0], index[1])?;
    let patch = patch_of_block(patches, rect)?;
    let (width, depth) = (rect[2] - rect[0], rect[3] - rect[1]);
    if !width.is_finite() || !depth.is_finite() || width <= 0.0 || depth <= 0.0 {
        return None;
    }
    let step = lod.step() as f32;
    // 顶点间距 = patch 采样步长 × LOD 步长；向上取整保证块内不留半格。
    let spacing_x = (patch_cell(patch, 0) * step).max(f32::MIN_POSITIVE);
    let spacing_z = (patch_cell(patch, 1) * step).max(f32::MIN_POSITIVE);
    let cols = ((width / spacing_x).ceil() as u32).max(1);
    let rows = ((depth / spacing_z).ceil() as u32).max(1);
    let (cn, rn) = (cols as usize, rows as usize);
    let count = (cn + 1) * (rn + 1);

    let mut positions = Vec::with_capacity(count);
    let mut uvs = Vec::with_capacity(count);
    for j in 0..=rn {
        let v = j as f32 / rn as f32;
        let z = rect[1] + depth * v;
        for i in 0..=cn {
            let u = i as f32 / cn as f32;
            let x = rect[0] + width * u;
            let y = sample_height_lod(patches, grid, x, z, lod).unwrap_or(0.0);
            positions.push([x, y, z]);
            uvs.push([u, v]);
        }
    }

    let normals = build_normals(&positions, cn, rn);
    let weights = positions
        .iter()
        .zip(normals.iter())
        .map(|(p, n)| cfg.weights(p[1], slope_deg(*n)))
        .collect();

    let mut indices = Vec::with_capacity(cn * rn * 6);
    for j in 0..rn {
        for i in 0..cn {
            let v00 = (j * (cn + 1) + i) as u32;
            let v01 = ((j + 1) * (cn + 1) + i) as u32;
            let v10 = v00 + 1;
            let v11 = v01 + 1;
            indices.extend_from_slice(&[v00, v01, v10, v10, v01, v11]);
        }
    }

    Some(TerrainMesh {
        positions,
        normals,
        uvs,
        weights,
        indices,
        lod,
        segments: [cols, rows],
    })
}

/// 法线由网格自身的高度差分算（中心差分，边缘单侧顶替），再单位化。
///
/// 不走 [`crate::sample_normal`]：网格法线要跟**这一块**的顶点密度一致，
/// 跨越块边界去多点采样会让 LOD 段边缘的法线糊掉。
fn build_normals(positions: &[[f32; 3]], cols: usize, rows: usize) -> Vec<[f32; 3]> {
    let stride = cols + 1;
    let mut out = Vec::with_capacity(positions.len());
    for j in 0..=rows {
        for i in 0..=cols {
            let il = i.saturating_sub(1);
            let ir = (i + 1).min(cols);
            let jd = j.saturating_sub(1);
            let ju = (j + 1).min(rows);
            let dx = positions[j * stride + ir][0] - positions[j * stride + il][0];
            let dz = positions[ju * stride + i][2] - positions[jd * stride + i][2];
            let gx = if dx > 0.0 {
                (positions[j * stride + ir][1] - positions[j * stride + il][1]) / dx
            } else {
                0.0
            };
            let gz = if dz > 0.0 {
                (positions[ju * stride + i][1] - positions[jd * stride + i][1]) / dz
            } else {
                0.0
            };
            out.push(normalize([-gx, 1.0, -gz]));
        }
    }
    out
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !len.is_finite() || len <= 0.0 {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

/// 法线 → 坡度（度，`0` 为水平、`90` 为垂直面）。
fn slope_deg(n: [f32; 3]) -> f32 {
    let y = n[1].clamp(-1.0, 1.0);
    y.acos().to_degrees()
}
