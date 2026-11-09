//! 级联阴影的**遮挡体按级裁剪**（扩展二十四）。
//!
//! 背景：扩展十六把方向光阴影做成 N 级级联之后，每级各自拟合一台光源正交相机，
//! 但深度 pass 仍然**整批**提交——每级都把全部实例重画一遍。一个 4 级级联的
//! 大场景，阴影通道的开销就是 4 倍全场景几何，而绝大多数物件在近级的光源
//! 视锥外、在远级的 xy 范围外，纯属白画。
//!
//! 这一层只做一件事：给每一级算出「哪些实例真的有可能把影子投进该级」，
//! 让渲染层按掩码切区间提交。判定必须是**保守**的——宁可多画一个，不能少画，
//! 因为少画的表现是「影子凭空缺一块」，比多画的性能损失严重得多。
//!
//! 保守的具体含义：
//!
//! * `x` / `y`：与 NDC `[-1, 1]` 的区间**相交**就保留，任一侧在范围外只裁掉整段都在外的；
//! * `z`：只裁**远侧**（`z_min > 1`，即整块都在该级最远平面之外，影子落不到接收区）。
//!   靠近光源的一侧（`z < 0`）一律保留：那块区域里的物件正处在「接收体与光源之间」，
//!   它们的影子恰恰会投进该级，裁掉就是丢影子。`CascadeSettings::caster_extend`
//!   把光源相机的深度范围朝光源方向再拉开一段，正是为了让这段区域的遮挡体进得来。
//!
//! 零 wgpu / winit 依赖：本模块只吃矩阵与包围盒，纯 CPU 可测。

use crate::aabb::Aabb;
use glam::{Mat4, Vec4};
use std::ops::Range;

/// 判定用的浮点余量：贴边物件按「相交」处理，避免因为一次浮点舍入丢掉边缘像素的影子。
const EDGE_EPSILON: f32 = 1.0e-4;

/// 「拿不到包围盒」时用的兜底盒半宽（世界单位，1e7 ≈ 一万公里）。
///
/// 渲染层的约定是 fail-open：没有 AABB 的实例不参与任何裁剪。用一个必然与所有
/// 级联视锥相交的巨盒表达这个语义，代价只是该实例多画几次。
pub const FAIL_OPEN_HALF_EXTENT: f32 = 1.0e7;

/// 兜底盒本体：任意级联判定都会返回 `true`。
pub fn fail_open_bounds() -> Aabb {
    Aabb::from_center_half(glam::Vec3::ZERO, glam::Vec3::splat(FAIL_OPEN_HALF_EXTENT))
}

/// 世界 AABB 是否可能在该级联里投出可见阴影。
///
/// `light_view_proj` 是该级的光源正交相机矩阵（与深度 pass 用的那份完全一致）。
/// 空盒一律 `true`（fail-open）。
pub fn aabb_in_cascade(light_view_proj: &Mat4, aabb: &Aabb) -> bool {
    if aabb.is_empty() {
        return true;
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in aabb.corners() {
        let clip: Vec4 = *light_view_proj * corner.extend(1.0);
        if !clip.is_finite() || clip.w.abs() < 1.0e-9 {
            // 退化输入（矩阵含 NaN / 非正交投影）：不裁，交给上层画
            return true;
        }
        let inv = 1.0 / clip.w;
        let point = [clip.x * inv, clip.y * inv, clip.z * inv];
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }

    if max[0] < -1.0 - EDGE_EPSILON || min[0] > 1.0 + EDGE_EPSILON {
        return false;
    }
    if max[1] < -1.0 - EDGE_EPSILON || min[1] > 1.0 + EDGE_EPSILON {
        return false;
    }
    // 只裁远侧：`z > 1` 已经在接收区之后，影子不可能回到该级
    if min[2] > 1.0 + EDGE_EPSILON {
        return false;
    }
    true
}

/// 为每一级算出「保留哪些实例」的掩码：外层下标是级，内层与 `aabbs` 同序。
///
/// `levels` 长度即本帧生效的级数（1..=`MAX_CASCADES`），掩码顺序与实例缓冲一致，
/// 渲染层可以直接用 [`split_runs`] 把掩码切成连续区间。
pub fn plan_casters(levels: &[Mat4], aabbs: &[Aabb]) -> Vec<Vec<bool>> {
    levels
        .iter()
        .map(|vp| aabbs.iter().map(|aabb| aabb_in_cascade(vp, aabb)).collect())
        .collect()
}

/// 把所有实例都保留的掩码（局部光源阴影用：那些灯没有级联，一条 pass 画全量）。
pub fn keep_all(len: usize) -> Vec<bool> {
    vec![true; len]
}

/// 把一段连续的保留标记切成若干连续区间（`offset` 是该段在实例缓冲里的起点）。
///
/// 例：`offset = 10`、`flags = [true, true, false, true]` → `[10..12, 13..14]`。
/// 全为 `false` 时返回空；全为 `true` 时返回一个整段。
pub fn split_runs(offset: u32, flags: &[bool]) -> Vec<Range<u32>> {
    let mut out = Vec::new();
    let mut start: Option<u32> = None;
    for (index, keep) in flags.iter().enumerate() {
        let current = offset.saturating_add(index as u32);
        match (keep, start) {
            (true, None) => start = Some(current),
            (false, Some(begin)) => {
                out.push(begin..current);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        out.push(begin..offset.saturating_add(flags.len() as u32));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// 把世界坐标 `[-2, 2]` 线性映到 NDC `[-1, 1]` 的正交相机。
    ///
    /// 测试只关心「判定逻辑」，所以矩阵故意取得比真实级联简单：级联的
    /// view-proj 无论怎么拟合，用的都是同一套 NDC 约定。
    fn unit_ortho() -> Mat4 {
        Mat4::from_scale(Vec3::splat(0.5))
    }

    fn box_at(x: f32, y: f32, z: f32, half: f32) -> Aabb {
        Aabb::from_center_half(Vec3::new(x, y, z), Vec3::splat(half))
    }

    /// 级内的遮挡体必须保留，否则就会出现「该级没有影子」。
    #[test]
    fn caster_inside_level_is_kept() {
        let vp = unit_ortho();
        assert!(aabb_in_cascade(&vp, &box_at(0.0, 0.0, 0.0, 0.5)));
        assert!(aabb_in_cascade(&vp, &box_at(1.8, -1.8, 1.8, 0.1)));
    }

    /// 只裁**远侧**：`z_min > 1` 说明整块都在接收区之后，影子落不回该级。
    #[test]
    fn caster_behind_receiver_is_culled() {
        let vp = unit_ortho();
        assert!(!aabb_in_cascade(&vp, &box_at(0.0, 0.0, 40.0, 0.5)));
    }

    /// 靠近光源的一侧一律保留：那些物件正处在「接收体与光源之间」，
    /// 影子恰恰会投进该级。这条是保守性的核心，不能因为「不在视锥深度里」就裁。
    /// 注意光空间 xy 是接收区的横向范围，那儿超出去仍然裁——投影是正交的，
    /// 横向超出就真的投不进这一级。
    #[test]
    fn caster_toward_light_is_kept() {
        let vp = unit_ortho();
        assert!(aabb_in_cascade(&vp, &box_at(0.0, 0.0, -40.0, 0.5)));
        assert!(aabb_in_cascade(&vp, &box_at(1.5, -1.5, -40.0, 0.5)));
    }

    /// xy 整段都在范围外才裁；压着边界的一定要留。
    #[test]
    fn caster_xy_outside_is_culled_but_straddling_is_kept() {
        let vp = unit_ortho();
        assert!(!aabb_in_cascade(&vp, &box_at(5.0, 0.0, 0.0, 0.2)));
        assert!(!aabb_in_cascade(&vp, &box_at(0.0, -5.0, 0.0, 0.2)));
        // 中心在范围外、但本体跨过边界：必须保留，否则边界上的影子缺一条
        assert!(aabb_in_cascade(&vp, &box_at(2.05, 0.0, 0.0, 0.2)));
        assert!(aabb_in_cascade(&vp, &box_at(0.0, 2.05, 0.0, 0.2)));
    }

    /// 空盒与退化矩阵都走 fail-open：宁可多画，不能丢影子。
    #[test]
    fn empty_box_and_broken_matrix_fail_open() {
        let vp = unit_ortho();
        assert!(aabb_in_cascade(&vp, &Aabb::EMPTY));
        let nan = Mat4::from_scale(Vec3::splat(f32::NAN));
        assert!(aabb_in_cascade(&nan, &box_at(9.0, 9.0, 9.0, 0.5)));
    }

    /// 兜底盒在任何正常尺度的级联里都判为保留——它是「这个实例没有 AABB」的表达方式。
    #[test]
    fn fail_open_bounds_survive_any_level() {
        let open = fail_open_bounds();
        assert!(!open.is_empty());
        for scale in [0.001, 0.5, 1.0, 200.0] {
            let vp = Mat4::from_scale(Vec3::splat(scale));
            assert!(
                aabb_in_cascade(&vp, &open),
                "兜底盒在 scale={scale} 的级联里被裁掉了"
            );
        }
    }

    /// 掩码形状与 `aabbs` 对齐，且宽级保留的东西不少于窄级。
    #[test]
    fn plan_casters_masks_follow_level_width() {
        let narrow = Mat4::from_scale(Vec3::splat(0.5)); // 覆盖 ±2
        let wide = Mat4::from_scale(Vec3::splat(0.1)); // 覆盖 ±10
        let aabbs = [box_at(0.0, 0.0, 0.0, 0.5), box_at(6.0, 0.0, 0.0, 0.5)];
        let masks = plan_casters(&[narrow, wide], &aabbs);
        assert_eq!(masks.len(), 2);
        assert_eq!(masks[0].len(), aabbs.len());
        assert_eq!(masks[1].len(), aabbs.len());
        // 近级：只有中心那个；远级：两个都在
        assert_eq!(masks[0], vec![true, false]);
        assert_eq!(masks[1], vec![true, true]);
    }

    /// 空级列表 / 空实例列表都要给出形状正确的空结果，不能 panic。
    #[test]
    fn plan_casters_handles_empty_inputs() {
        assert!(plan_casters(&[], &[box_at(0.0, 0.0, 0.0, 1.0)]).is_empty());
        let masks = plan_casters(&[unit_ortho()], &[]);
        assert_eq!(masks.len(), 1);
        assert!(masks[0].is_empty());
    }

    /// 掩码到区间的转换：交替段要切准，全保留是整段，全裁是空。
    #[test]
    fn split_runs_cuts_contiguous_ranges() {
        assert_eq!(
            split_runs(10, &[true, true, false, true]),
            vec![10..12, 13..14]
        );
        assert_eq!(split_runs(0, &[true, true, true]), vec![0..3]);
        assert!(split_runs(7, &[false, false]).is_empty());
        assert!(split_runs(0, &[]).is_empty());
        // 尾部开着的段要收在末尾
        assert_eq!(split_runs(3, &[false, true, true]), vec![4..6]);
        // 单元素两种态势
        assert_eq!(split_runs(5, &[true]), vec![5..6]);
        assert!(split_runs(5, &[false]).is_empty());
    }

    /// 局部光源阴影用 keep_all：长度对、全 true。
    #[test]
    fn keep_all_keeps_everything() {
        assert!(keep_all(0).is_empty());
        let flags = keep_all(4);
        assert_eq!(flags.len(), 4);
        assert!(flags.iter().all(|keep| *keep));
        assert_eq!(split_runs(0, &flags), vec![0..4]);
    }
}
