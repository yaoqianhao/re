//! 地形场装配（M2-C07）：`Scene.terrain` → `r3e_physics::HeightField`。
//!
//! # 这段胶水为什么不放 physics 也不放 terrain
//!
//! `r3e-physics` 与 `r3e-terrain` 之间**不允许**有依赖边（`dep_rules.json`：两边
//! `allow` 都只有 `r3e-core`）。物理侧只认 [`r3e_physics::HeightField`] 这一份
//! 「世界 Y 数组 + 间距」，地形侧只认 [`r3e_core::TerrainPatch`] 这一份「矩形 + 样本网格」，
//! 两种表达之间的翻译必须落在同时看得见两边的那一层 —— 运行时装配层。
//! 这与渲染侧「拿顶点数组去上传」是同一次装配的两条出口：同一份 patch，一边出三角网，
//! 一边出高度场，谁都不把对方的类型带进自己的 crate。
//!
//! # 数值口径
//!
//! 高度样本一律由 [`r3e_terrain::sample_height_at`] 取（双线性、同点同值），
//! 与渲染侧用的是**同一个函数**：脚底踩到的面和眼睛看到的面必须是同一张面。
//! 场外一点（域内但没有 patch 覆盖）按 `0.0` 兜底 —— 那是「还没做地形」的地面，
//! 与场景里 `ground_plane` 的约定一致。
//!
//! # 上限
//!
//! 高度场的采样点数是 `cols × rows`，直接按 200 米 / 2 米铺就是一万个 `f32`。
//! 超过 [`FIELD_MAX_SAMPLES`] 时按 1.5 倍逐级放大 `cell` 重铺，直到落进预算；
//! 放大到 64 倍仍超就返回 `None`（宁可没有地形碰撞，也不要在主循环里铺一张
//! 百兆的数组）。这套降级完全由输入决定，纯函数、可重现。

use r3e_core::TerrainPatch;
use r3e_physics::HeightField;
use r3e_terrain::sample_height_at;

/// 默认采样间距（米）。2 米一个格，走路的手感误差在厘米级，肉眼看不出台阶感。
pub const FIELD_CELL: f32 = 2.0;

/// 单张高度场的采样点上限（约 256 × 256）。
pub const FIELD_MAX_SAMPLES: usize = 65_536;

/// 按默认间距把场景地形铺成一张高度场；没有地形块时返回 `None`。
pub fn heightfield_of(patches: &[TerrainPatch]) -> Option<HeightField> {
    heightfield_with_cell(patches, FIELD_CELL)
}

/// 指定采样间距铺高度场。间距非法时回落到 [`FIELD_CELL`]。
///
/// 域取所有块的**并集包围盒**：`HeightField` 是单张矩形网格，多块地形必须并进
/// 同一张场里，否则块与块之间会出现一道物理上的「悬崖」。
pub fn heightfield_with_cell(patches: &[TerrainPatch], cell: f32) -> Option<HeightField> {
    let (min_x, min_z, max_x, max_z) = bounds(patches)?;
    let base = if cell.is_finite() && cell > 0.0 {
        cell
    } else {
        FIELD_CELL
    };

    let span_x = (max_x - min_x).max(0.0);
    let span_z = (max_z - min_z).max(0.0);

    let mut cell = base;
    // 最多放大 64 倍（1.5^10 ≈ 57.7，11 轮到位），超出预算就放弃。
    for _ in 0..12 {
        let cols = axis_samples(span_x, cell);
        let rows = axis_samples(span_z, cell);
        if cols * rows <= FIELD_MAX_SAMPLES {
            let heights = sample_grid(patches, min_x, min_z, cell, cols, rows);
            return HeightField::new(min_x, min_z, cell, cols, rows, heights);
        }
        cell *= 1.5;
    }
    None
}

/// 单轴采样点数：向上取整到覆盖整个跨度，至少 2 个（`HeightField` 的下限）。
fn axis_samples(span: f32, cell: f32) -> usize {
    let n = (span / cell).ceil() as usize + 1;
    n.max(2)
}

/// 按格逐点取高度。行主序（`j` 沿 +Z、`i` 沿 +X），与 `HeightField` 的约定一致。
fn sample_grid(
    patches: &[TerrainPatch],
    min_x: f32,
    min_z: f32,
    cell: f32,
    cols: usize,
    rows: usize,
) -> Vec<f32> {
    let mut heights = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        let z = min_z + j as f32 * cell;
        for i in 0..cols {
            let x = min_x + i as f32 * cell;
            heights.push(sample_height_at(patches, x, z).unwrap_or(0.0));
        }
    }
    heights
}

/// 所有块的并集包围盒 `(min_x, min_z, max_x, max_z)`；空表或含非有限值返回 `None`。
fn bounds(patches: &[TerrainPatch]) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for p in patches {
        let (x, z) = (p.origin[0], p.origin[1]);
        let (w, d) = (p.size[0], p.size[1]);
        if !x.is_finite() || !z.is_finite() || !w.is_finite() || !d.is_finite() {
            return None;
        }
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x + w.max(0.0));
        max_z = max_z.max(z + d.max(0.0));
    }
    if patches.is_empty() || max_x < min_x || max_z < min_z {
        return None;
    }
    Some((min_x, min_z, max_x, max_z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slab() -> TerrainPatch {
        TerrainPatch::flat("ground", [-100.0, -100.0], [200.0, 200.0])
    }

    #[test]
    fn empty_scene_has_no_field() {
        assert!(heightfield_of(&[]).is_none(), "没有地形就不该铺场");
    }

    #[test]
    fn flat_patch_covers_its_rect() {
        let field = heightfield_of(&[slab()]).expect("平地块必须能铺成场");
        assert_eq!((field.cols(), field.rows()), (101, 101));
        assert!(field.covers(-100.0, -100.0), "左下角必须在场里");
        assert!(field.covers(99.0, 99.0), "右下角必须在场里");
        assert!(!field.covers(120.0, 0.0), "场外不该被覆盖");
        assert_eq!(field.height_at(0.0, 0.0), Some(0.0));
    }

    #[test]
    fn two_patches_merge_into_one_domain() {
        let left = TerrainPatch::flat("left", [-100.0, -50.0], [100.0, 100.0]);
        let right = TerrainPatch::flat("right", [0.0, -50.0], [100.0, 100.0]);
        let field = heightfield_of(&[left, right]).expect("两块地形必须并成一张场");
        assert!(field.covers(-90.0, 0.0), "左块必须在场里");
        assert!(
            field.covers(90.0, 0.0),
            "右块必须在场里 —— 并集域覆盖整条缝"
        );
        assert!(field.normal_at(0.0, 0.0).is_some(), "接缝处必须给得出法线");
    }

    #[test]
    fn cell_shrinks_when_budget_blows_up() {
        let field = heightfield_of(&[slab()]).expect("必须铺得出场");
        assert!(
            field.cols() * field.rows() <= FIELD_MAX_SAMPLES,
            "采样点 {} 超预算 {}",
            field.cols() * field.rows(),
            FIELD_MAX_SAMPLES
        );
        let coarse = heightfield_with_cell(&[slab()], 50.0).expect("粗间距也能铺");
        assert!(coarse.cols() < field.cols(), "间距越大点数越少");
    }

    #[test]
    fn heights_follow_the_patch() {
        let mut slope = TerrainPatch::flat("slope", [0.0, 0.0], [20.0, 20.0]);
        slope.resolution = [3, 3];
        // 沿 +X 从 0 升到 10：中间列 5。
        slope.heights = vec![0.0, 5.0, 10.0, 0.0, 5.0, 10.0, 0.0, 5.0, 10.0];
        let field = heightfield_with_cell(&[slope], 1.0).expect("斜坡必须能铺");
        let mid = field.height_at(10.0, 10.0).expect("场中心必须可采样");
        assert!((mid - 5.0).abs() < 1e-3, "中点高度应为 5，实际 {mid}");
        let top = field.height_at(20.0, 10.0).expect("右边界必须可采样");
        assert!((top - 10.0).abs() < 1e-3, "右边界高度应为 10，实际 {top}");
    }
}
