//! 多分辨率 LOD 采样（M2-C02）。
//!
//! 与 [`crate::sample_height_at`] 的区别只有一处：块**内部**按 LOD 步长跳采顶点再插值，
//! 块**边界带**强制回到步长 1（全采样）。这个「边界强制细档」是接缝规则的落地形式，
//! 也是本卡唯一容易写错的地方。
//!
//! ## 为什么边界必须回到 L0
//!
//! 两块相邻时共享同一条边界线。若两侧各自按自己的 LOD 跳采，边界上的样本会落在
//! 不同的世界位置 —— 网格一侧有顶点、另一侧只在内侧有顶点，三角化跨过边界时就出现
//! 裂缝（地形「漏光」）。把边界带一律按步长 1 采样后，两条边上的样本位置完全一致，
//! 于是「任意两个不同 LOD 的相邻块，接缝处采样差恒为 0」。
//!
//! 代价是边界多采一些点，量级是 `4 × 边长 / 步长`，与整块采样次数相比可忽略。
//!
//! ## 不是「降分辨率量化」
//!
//! 粗档取的是**跳采顶点 + 双线性**，不是「就近取一个粗顶点」。量化会在档位切换处
//! 产生台阶式跳变，接缝断言与「误差 ≤ 0.5 米」断言都会立刻红。

use r3e_core::TerrainPatch;

use crate::{BlockGrid, Lod};

/// 在一个 patch 的**跳采网格**上插值取高度；点不在该 patch 内返回 `None`。
///
/// `step` 为顶点步长，取值见 [`crate::LOD_STEPS`]；`step == 1` 时直接复用
/// [`TerrainPatch::sample_height`]，保证细档与全采样逐位一致（不另写一套插值）。
fn sample_patch_step(patch: &TerrainPatch, x: f32, z: f32, step: u32) -> Option<f32> {
    if step <= 1 {
        return patch.sample_height(x, z);
    }
    let (w, d) = (patch.size[0], patch.size[1]);
    if !(w > 0.0 && d > 0.0) || !x.is_finite() || !z.is_finite() {
        return None;
    }
    let u = (x - patch.origin[0]) / w;
    let v = (z - patch.origin[1]) / d;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let [rx, ry] = patch.resolution;
    if patch.heights.is_empty() || rx < 2 || ry < 2 {
        return Some(0.0);
    }
    if patch.heights.len() != (rx * ry) as usize {
        return Some(0.0);
    }
    let fx = u * (rx - 1) as f32;
    let fz = v * (ry - 1) as f32;
    let (x0, x1, tx) = lattice(fx, rx, step);
    let (z0, z1, tz) = lattice(fz, ry, step);
    let h00 = patch.height_at(x0, z0);
    let h10 = patch.height_at(x1, z0);
    let h01 = patch.height_at(x0, z1);
    let h11 = patch.height_at(x1, z1);
    let a = h00 + (h10 - h00) * tx;
    let b = h01 + (h11 - h01) * tx;
    Some(a + (b - a) * tz)
}

/// 落在跳采网格的哪个小区间：`(左顶点, 右顶点, 插值权重)`。
///
/// 末点单独处理：`f` 恰好等于 `n - 1` 时左右顶点重合，权重取 0（避免除零，
/// 也避免末点被外插到网格外）。
fn lattice(f: f32, n: u32, step: u32) -> (u32, u32, f32) {
    let last = n - 1;
    let lo = (((f / step as f32).floor().max(0.0)) as u32 * step).min(last);
    let hi = (lo + step).min(last);
    let t = if hi > lo {
        (f - lo as f32) / (hi - lo) as f32
    } else {
        0.0
    };
    (lo, hi, t.clamp(0.0, 1.0))
}

/// 点是否落在块的边界带内（块边界线两侧各一个粗档跨度）。
///
/// 带宽取「粗档顶点间距」而不是固定米数：这样两侧块在边界附近至少各有两排细档样本，
/// 无论 `block_size` 与 patch 采样步长是否整除，接缝两侧的样本位置都能对上。
fn in_edge_band(patch: &TerrainPatch, rect: [f32; 4], x: f32, z: f32, lod: Lod) -> bool {
    let step = lod.step();
    if step <= 1 {
        return true;
    }
    let [rx, ry] = patch.resolution;
    let cell_x = if rx >= 2 {
        patch.size[0] / (rx - 1) as f32
    } else {
        0.0
    };
    let cell_z = if ry >= 2 {
        patch.size[1] / (ry - 1) as f32
    } else {
        0.0
    };
    let band = cell_x.max(cell_z) * step as f32;
    (x - rect[0]).abs() <= band
        || (rect[2] - x).abs() <= band
        || (z - rect[1]).abs() <= band
        || (rect[3] - z).abs() <= band
}

/// 按 LOD 采样某个世界点的高度：块内跳采 + 边界带强制全采样，多块重叠仍取最高层。
///
/// 点不落在 `grid` 范围内、或没有任何 patch 覆盖该点时返回 `None`。
/// `lod` 只影响**精度**，不影响命中哪一层（同一 `(x, z)` 换档不会改变「取最高层」的裁决）。
pub fn sample_height_lod(
    patches: &[TerrainPatch],
    grid: &BlockGrid,
    x: f32,
    z: f32,
    lod: Lod,
) -> Option<f32> {
    let idx = grid.block_at(x, z)?;
    let rect = grid.block_rect(idx[0], idx[1])?;
    let mut highest: Option<f32> = None;
    for patch in patches {
        let step = if in_edge_band(patch, rect, x, z, lod) {
            1
        } else {
            lod.step()
        };
        if let Some(y) = sample_patch_step(patch, x, z, step) {
            highest = Some(highest.map_or(y, |cur: f32| cur.max(y)));
        }
    }
    highest
}
