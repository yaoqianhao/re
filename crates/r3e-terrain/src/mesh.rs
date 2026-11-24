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
//! ## 顶点落在哪（H-01B 现象②/④ 的修复口径）
//!
//! 顶点**不许**在块内等分落位，必须落在 **patch 采样格点上**（间隔 = patch 步长 × LOD 步长）：
//!
//! * 块边长除以格距常常不整除（`block_size = 50`、`cell = 20` 时 `50 / 20 = 2.5`），
//!   等分落位会把顶点放在 `16.67 / 33.33` 这种既不是格点、也不在折线上的位置；
//! * 三角形平面于是**横跨过一条格点折线**，把「格点两侧各是平面」的折面抹成一张
//!   斜平面 —— 渲染面在折点处抬高数米，人物按高度场贴地就穿进坡面（现象②）；
//! * 顶点落在格点上后，任一格内地形是双线性（格内无缝可跨），三角化与真实面对齐。
//!
//! 块两端仍强制落在块边界线上（相邻块共享边界、必须重合），两侧再各补一个最近的
//! 细档格点，使**边界带在任意档位下都是 1 格宽**——这既让接缝两侧的顶点互相包含，
//! 也让交界处的三角面只跨 1 格，不会把粗档的大跨度三角面贴到接缝上去。
//!
//! ## 裙边（接缝的最后一道保险）
//!
//! 两档 LOD 相邻时，边界线上的顶点虽已对齐，但细档在两个粗顶点之间仍可能高出/低于
//! 粗档的那条直线段 —— 这是**采样精度差**，不是数据错。它可能被看穿（漏出清屏色，
//! 现象④）。因此每块四周再加一圈**向下垂的裙边**：把边界顶点沿 `-Y` 复制一份并连成
//! 竖直带，缝隙由这条带子兜住。
//!
//! 裙边只做**显示兜底**：物理侧的 `HeightField` 由 patch 直接铺（见
//! `r3e-runtime::terrain_field`），不认识网格，也就不会把裙边当成可以踩的墙。
//! 裙边顶点排在 `positions[body_vertices..]`，`indices[body_indices..]` 才是裙边三角，
//! 统计与几何判据都要按这两个分界点切开看。
//!
//! ## 权重怎么定（不引入新数据源）
//!
//! 4 层（草 / 岩 / 沙 / 雪）由「高度 + 坡度」程序化判定，配置见
//! [`TerrainLayerConfig`]。每层先算一个 0..=1 的原始分，再按和归一化；
//! **和恰为 0**（配置把四层全压掉）时退化为「层 0 = 1」，绝不留出权重和不为 1 的顶点。

use r3e_core::TerrainPatch;

use crate::{sample_height_lod, BlockGrid, Lod};

/// 单轴顶点坐标上限。病态输入（格距 1e-6 米、块边长 1000 米）时退回均匀切分，
/// 绝不让顶点数在主循环里失控。
const MAX_AXIS_POINTS: usize = 4096;

/// 裙边下沉深度 = 粗档顶点间距 × 本系数，再夹到下面的上下限（米）。
const SKIRT_DEPTH_FACTOR: f32 = 1.5;
/// 裙边最浅多少米：太浅挡不住 L0 与 L1 之间几米的误差。
const SKIRT_DEPTH_MIN: f32 = 0.5;
/// 裙边最深多少米：再深就从「兜缝的带子」变成「外圈一道悬崖」。
const SKIRT_DEPTH_MAX: f32 = 8.0;

/// 一块地形的网格数据。顶点属性一一对应，索引指向 `positions`。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMesh {
    /// 顶点世界坐标（行主序：先跑完一行 `i` 再进下一行 `j`）。
    ///
    /// 前 `body_vertices` 个是地表本体，其后是四周裙边的下沿顶点。
    pub positions: Vec<[f32; 3]>,
    /// 顶点法线（单位化，朝向天空一侧）。裙边顶点沿用对应上沿顶点的法线。
    pub normals: Vec<[f32; 3]>,
    /// 顶点 uv，块内归一化到 `[0, 1]`。裙边沿用上沿顶点的 uv。
    pub uvs: Vec<[f32; 2]>,
    /// 4 层混合权重，逐顶点和为 1。
    pub weights: Vec<[f32; 4]>,
    /// 三角形索引（地表每格两个三角，其后是裙边带）。绕序一律朝外。
    pub indices: Vec<u32>,
    /// 生成时用的档位。
    pub lod: Lod,
    /// 网格段数 `[列数, 行数]`：本体顶点数是 `(列+1) × (行+1)`。
    pub segments: [u32; 2],
    /// 地表本体顶点数（`positions[..body_vertices]`）；其后全是裙边下沿顶点。
    pub body_vertices: usize,
    /// 地表本体索引数（`indices[..body_indices]`）；其后全是裙边三角。
    pub body_indices: usize,
    /// 裙边下沉深度（米）。`0` 表示这块没加裙边（顶点数退化，无可成带的边）。
    pub skirt_depth: f32,
}

impl TerrainMesh {
    /// 本体的三角形数（不含裙边）。
    pub fn body_triangles(&self) -> usize {
        self.body_indices / 3
    }

    /// 裙边带的三角形数。
    pub fn skirt_triangles(&self) -> usize {
        self.indices.len().saturating_sub(self.body_indices) / 3
    }
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

/// 落在 patch 采样格点上的「不小于 `v` 的最小格点」。序号溢出时原样返回 `v`。
fn lattice_ceil(origin: f32, cell: f32, v: f32) -> f32 {
    let k = ((v - origin) / cell).ceil();
    if k.is_finite() {
        origin + k * cell
    } else {
        v
    }
}

/// 落在 patch 采样格点上的「不大于 `v` 的最大格点」。序号溢出时原样返回 `v`。
fn lattice_floor(origin: f32, cell: f32, v: f32) -> f32 {
    let k = ((v - origin) / cell).floor();
    if k.is_finite() {
        origin + k * cell
    } else {
        v
    }
}

/// 一个轴上的顶点坐标（严格升序、去重、两端必为块边界）。
///
/// 规则见模块文档：块内取 **patch 采样格点** 的整数倍（粗档间隔 = patch 步长 × LOD
/// 步长），两端各补一个最近的细档格点形成 1 格宽的边界带；块边界本身必取。
///
/// 格距或跨度非法时退化成「两端点」（单段网格）；坐标数会超过 [`MAX_AXIS_POINTS`]
/// 的病态输入退化成均匀切分 —— 宁可有缝，也不许在主循环里铺出百万顶点。
fn axis_coords(origin: f32, cell: f32, start: f32, end: f32, step: u32) -> Vec<f32> {
    let span = end - start;
    if !origin.is_finite()
        || !cell.is_finite()
        || cell <= 0.0
        || !start.is_finite()
        || !end.is_finite()
        || span.is_nan()
        || span <= 0.0
    {
        return vec![start, end];
    }
    let coarse = (cell * step.max(1) as f32).max(f32::MIN_POSITIVE);
    let eps = (coarse * 1e-4).max(1e-6);
    let mut inner: Vec<f32> = Vec::new();

    let lo_k = ((start - origin) / coarse).ceil() as i64 - 1;
    let hi_k = ((end - origin) / coarse).floor() as i64 + 1;
    if hi_k.saturating_sub(lo_k) <= MAX_AXIS_POINTS as i64 {
        let mut k = lo_k;
        while k <= hi_k {
            inner.push(origin + k as f32 * coarse);
            k += 1;
        }
        // 边界带：块边界内侧最近的细档格点，保证最外一圈只有 1 格宽。
        inner.push(lattice_ceil(origin, cell, start));
        inner.push(lattice_floor(origin, cell, end));
    } else {
        // 病态格距：不追求格点对齐，只保证坐标数有界。
        let n = ((span / coarse).ceil() as usize).clamp(1, MAX_AXIS_POINTS);
        for i in 1..n {
            inner.push(start + span * i as f32 / n as f32);
        }
    }

    inner.retain(|v| v.is_finite() && *v > start + eps && *v < end - eps);
    inner.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<f32> = Vec::with_capacity(inner.len() + 2);
    out.push(start);
    for v in inner {
        match out.last() {
            Some(prev) if (v - prev).abs() <= eps => {}
            _ => out.push(v),
        }
    }
    out.push(end);
    out
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
    let step = lod.step();
    // 粗档顶点间距 = patch 采样步长 × LOD 步长；顶点就落在这些格点上。
    let cell_x = patch_cell(patch, 0).max(f32::MIN_POSITIVE);
    let cell_z = patch_cell(patch, 1).max(f32::MIN_POSITIVE);
    let spacing_x = (cell_x * step.max(1) as f32).max(f32::MIN_POSITIVE);
    let spacing_z = (cell_z * step.max(1) as f32).max(f32::MIN_POSITIVE);

    let coords_x = axis_coords(patch.origin[0], cell_x, rect[0], rect[2], step);
    let coords_z = axis_coords(patch.origin[1], cell_z, rect[1], rect[3], step);
    let (nx, nz) = (coords_x.len(), coords_z.len());
    let cols = nx.saturating_sub(1);
    let rows = nz.saturating_sub(1);

    let mut buf = MeshBuf {
        positions: Vec::with_capacity(nx * nz),
        normals: Vec::new(),
        uvs: Vec::with_capacity(nx * nz),
        weights: Vec::new(),
        indices: Vec::new(),
    };
    for &z in &coords_z {
        let v = (z - rect[1]) / depth;
        for &x in &coords_x {
            let u = (x - rect[0]) / width;
            let y = sample_height_lod(patches, grid, x, z, lod).unwrap_or(0.0);
            buf.positions.push([x, y, z]);
            buf.uvs.push([u, v]);
        }
    }

    buf.normals = build_normals(&buf.positions, cols, rows);
    buf.weights = buf
        .positions
        .iter()
        .zip(buf.normals.iter())
        .map(|(p, n)| cfg.weights(p[1], slope_deg(*n)))
        .collect();

    buf.indices.reserve(cols * rows * 6);
    for j in 0..rows {
        for i in 0..cols {
            let v00 = (j * nx + i) as u32;
            let v01 = ((j + 1) * nx + i) as u32;
            let v10 = v00 + 1;
            let v11 = v01 + 1;
            buf.indices
                .extend_from_slice(&[v00, v01, v10, v10, v01, v11]);
        }
    }
    let body_vertices = buf.positions.len();
    let body_indices = buf.indices.len();

    // 裙边深度由粗档间距定：缝隙的量级就是「两个粗顶点之间的采样差」，
    // 一个粗档跨度足够兜住，再按上下限夹一道，免得外圈出现深不见底的悬崖。
    let skirt_depth =
        (spacing_x.max(spacing_z) * SKIRT_DEPTH_FACTOR).clamp(SKIRT_DEPTH_MIN, SKIRT_DEPTH_MAX);
    append_skirt(&mut buf, nx, nz, skirt_depth);

    Some(TerrainMesh {
        positions: buf.positions,
        normals: buf.normals,
        uvs: buf.uvs,
        weights: buf.weights,
        indices: buf.indices,
        lod,
        segments: [cols as u32, rows as u32],
        body_vertices,
        body_indices,
        skirt_depth,
    })
}

/// 顶点各通道的并行缓冲。
///
/// 裙边要在四条边上复制边界顶点，五个通道必须同步推进，所以捆在一起传，
/// 免得 `append_skirt` 摆出八个参数、也免得某天漏改一个通道。
struct MeshBuf {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    weights: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

/// 四周加一圈向下垂的裙边带。
///
/// 每块四条边各成一条带：上沿用**已有的边界顶点**，下沿是把上沿沿 `-Y` 复制一份
/// （法线 / uv / 权重一并沿用）。三角绕序按「面朝块外」定，四条边的上沿走向依次是
/// `+X` / `-X` / `-Z` / `+Z` —— 这是叉积 `(上沿走向) × (向下)` 指向块外的充要条件。
fn append_skirt(buf: &mut MeshBuf, nx: usize, nz: usize, depth: f32) {
    if !(depth.is_finite() && depth > 0.0) || nx < 2 || nz < 2 {
        return;
    }
    // 四条边的上沿顶点序号；首尾各自不封闭（相邻边共享的角点各带一次，绕序各自成立）。
    let edges: [Vec<u32>; 4] = [
        (0..nx).map(|i| i as u32).collect(),
        (0..nx).rev().map(|i| ((nz - 1) * nx + i) as u32).collect(),
        (0..nz).rev().map(|j| (j * nx) as u32).collect(),
        (0..nz).map(|j| (j * nx + nx - 1) as u32).collect(),
    ];
    for edge in edges {
        let base = buf.positions.len() as u32;
        for &top in &edge {
            let p = buf.positions[top as usize];
            let n = buf.normals[top as usize];
            let t = buf.uvs[top as usize];
            let w = buf.weights[top as usize];
            buf.positions.push([p[0], p[1] - depth, p[2]]);
            buf.normals.push(n);
            buf.uvs.push(t);
            buf.weights.push(w);
        }
        for k in 0..edge.len().saturating_sub(1) {
            let t0 = edge[k];
            let t1 = edge[k + 1];
            let b0 = base + k as u32;
            let b1 = b0 + 1;
            buf.indices.extend_from_slice(&[t0, t1, b1, t0, b1, b0]);
        }
    }
}

/// 法线由网格自身的高度差分算（中心差分，边缘单侧顶替），再单位化。
///
/// 不走 [`crate::sample_normal`]：网格法线要跟**这一块**的顶点密度一致，
/// 跨越块边界去多点采样会让 LOD 段边缘的法线糊掉。顶点间距不匀（格点对齐后两端
/// 更密）也没关系：差分的分母取的是真实坐标差，不是名义格距。
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
