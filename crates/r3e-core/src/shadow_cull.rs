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

// ---------------------------------------------------------------------------
// 扩展二十五：层级剔除
// ---------------------------------------------------------------------------
//
// 扩展二十四的判定是**逐实例**的：每级对每个遮挡体做一次 8 角点投影，成本
// `O(级 × 实例)`。物件上万的场景里这笔 CPU 开销是实打实的，而其中绝大多数
// 实例和它们的兄弟坐在同一棵子树上——父节点的包围盒一旦整块落在该级覆盖
// 范围之外，整枝实例都不必再判。
//
// 这一层把场景树的**父子关系**接进判定：先自底向上聚出每棵子树的并集包围盒，
// 再自顶向下判——子树盒被裁就整枝跳过（省掉枝内全部实例判定），子树盒保留才
// 下探，到叶子才逐实例判。
//
// 保守性与扩展二十四**完全一致**：正交投影下并集的投影范围恰好等于各成员
// 投影范围的并集，因此「子树盒整块在级外」等价于「枝内每个实例都整块在级外」。
// 层级剔除只少判、不改判：掩码与逐实例判定逐位相同（差分测试钉住）。

/// 层级树里的一根枝：指向父节点、自身（直属实例）的世界包围盒与实例数。
///
/// `casts == 0` 表示纯变换节点（自身不画东西，只有子节点）；
/// 一个节点承载多个实例时 `casts > 1`，它们共享同一份包围盒判定结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CasterNode {
    /// 父节点下标；`None` = 根。越界 / 自指 / 成环一律按根处理（见 [`CasterTree::new`]）。
    pub parent: Option<usize>,
    /// 本节点自身实例的世界包围盒；没有实例时填 [`Aabb::EMPTY`]。
    pub bounds: Aabb,
    /// 本节点自身的遮挡体实例数。
    pub casts: u32,
}

impl CasterNode {
    /// 叶节点：自带一个遮挡体实例。
    pub fn leaf(bounds: Aabb) -> Self {
        Self {
            parent: None,
            bounds,
            casts: 1,
        }
    }

    /// 纯分组节点：自身不画东西，只把子节点拢在一起。
    pub fn group(parent: Option<usize>) -> Self {
        Self {
            parent,
            bounds: Aabb::EMPTY,
            casts: 0,
        }
    }

    /// 挂到父节点下。
    pub fn under(mut self, parent: usize) -> Self {
        self.parent = Some(parent);
        self
    }
}

/// 一次层级判定的计数：判定成本与省下的量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelCounts {
    /// 做过的盒判定次数（子树盒 + 混合节点自身盒）。
    pub box_tests: u32,
    /// 实际判过的实例次数（= 级 × 实例 - `pruned`）。
    pub instance_tests: u32,
    /// 被整枝跳过、省下的实例判定次数。
    pub pruned: u32,
}

/// 层级判定的结果：每级一份「按节点」的保留掩码 + 合计计数。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CasterHierarchyPlan {
    /// 外层下标是级，内层下标是 [`CasterTree`] 的节点下标。
    pub masks: Vec<Vec<bool>>,
    pub box_tests: u32,
    pub instance_tests: u32,
    pub pruned: u32,
}

/// 遮挡体的层级树（扁平存储）。
///
/// 构造后子树包围盒与子树实例数就已经聚好，`plan` 只做自顶向下的判定。
/// 零 wgpu 依赖，纯 CPU 可测。
#[derive(Debug, Clone, Default)]
pub struct CasterTree {
    nodes: Vec<CasterNode>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    subtree: Vec<Aabb>,
    sizes: Vec<u32>,
}

impl CasterTree {
    /// 由扁平节点表建树，并自底向上聚出子树包围盒。
    ///
    /// 父链上的越界、自指与成环一律降级成根：层级数据坏了宁可少裁，
    /// 也不能让下探原地打转。
    pub fn new(mut nodes: Vec<CasterNode>) -> Self {
        let count = nodes.len();
        for index in 0..count {
            let mut cursor = nodes[index].parent;
            let mut steps = 0usize;
            while let Some(parent) = cursor {
                if parent >= count || parent == index || steps > count {
                    nodes[index].parent = None;
                    break;
                }
                steps += 1;
                cursor = nodes[parent].parent;
            }
        }

        let mut children = vec![Vec::new(); count];
        let mut roots = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            match node.parent {
                Some(parent) => children[parent].push(index),
                None => roots.push(index),
            }
        }

        // 自底向上聚合：按拓扑序（父在子前）反着走一遍即可
        let mut order = Vec::with_capacity(count);
        let mut queue: std::collections::VecDeque<usize> = roots.iter().copied().collect();
        while let Some(index) = queue.pop_front() {
            order.push(index);
            for &child in &children[index] {
                queue.push_back(child);
            }
        }
        let mut subtree: Vec<Aabb> = nodes.iter().map(|node| node.bounds).collect();
        let mut sizes: Vec<u32> = nodes.iter().map(|node| node.casts).collect();
        for &index in order.iter().rev() {
            if let Some(parent) = nodes[index].parent {
                subtree[parent] = subtree[parent].union(&subtree[index]);
                sizes[parent] = sizes[parent].saturating_add(sizes[index]);
            }
        }

        Self {
            nodes,
            children,
            roots,
            subtree,
            sizes,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn nodes(&self) -> &[CasterNode] {
        &self.nodes
    }

    /// 某节点子树的并集包围盒。
    pub fn subtree_bounds(&self, index: usize) -> Aabb {
        self.subtree.get(index).copied().unwrap_or(Aabb::EMPTY)
    }

    /// 某节点子树里的遮挡体实例数。
    pub fn subtree_casts(&self, index: usize) -> u32 {
        self.sizes.get(index).copied().unwrap_or(0)
    }

    /// 全树的遮挡体实例数（= 逐实例判定的基线分母）。
    pub fn total_casts(&self) -> u32 {
        // 只累根：`sizes` 存的是「子树»总和，逐节点求和会重复计数
        self.roots
            .iter()
            .map(|index| self.sizes[*index])
            .fold(0u32, u32::saturating_add)
    }

    /// 每级一份「按节点」的保留掩码，附带判定计数。
    pub fn plan(&self, levels: &[Mat4]) -> CasterHierarchyPlan {
        let mut plan = CasterHierarchyPlan {
            masks: Vec::with_capacity(levels.len()),
            ..CasterHierarchyPlan::default()
        };
        for view_proj in levels {
            let mut mask = vec![false; self.nodes.len()];
            let counts = self.level_mask(view_proj, &mut mask);
            plan.box_tests = plan.box_tests.saturating_add(counts.box_tests);
            plan.instance_tests = plan.instance_tests.saturating_add(counts.instance_tests);
            plan.pruned = plan.pruned.saturating_add(counts.pruned);
            plan.masks.push(mask);
        }
        plan
    }

    /// 单级的自顶向下判定：子树盒被裁就整枝跳过，否则下探，叶子逐实例判。
    fn level_mask(&self, view_proj: &Mat4, mask: &mut [bool]) -> LevelCounts {
        let mut counts = LevelCounts::default();
        let mut stack: Vec<usize> = self.roots.clone();
        while let Some(index) = stack.pop() {
            // 枝里没有遮挡体（纯变换节点、后代也空）：不判也不下探
            if self.sizes[index] == 0 {
                continue;
            }
            counts.box_tests = counts.box_tests.saturating_add(1);
            if !aabb_in_cascade(view_proj, &self.subtree[index]) {
                // 整枝都在级外：掩码保持 false，枝内实例一个都不再判
                counts.pruned = counts.pruned.saturating_add(self.sizes[index]);
                continue;
            }
            let node = &self.nodes[index];
            if node.casts > 0 {
                counts.instance_tests = counts.instance_tests.saturating_add(node.casts);
                mask[index] = if self.children[index].is_empty() {
                    // 叶节点的子树盒就是它自己的盒，上面已经判过 true
                    true
                } else {
                    // 混合节点：子树盒比自身盒大，自身实例再判一次更精确的
                    counts.box_tests = counts.box_tests.saturating_add(1);
                    aabb_in_cascade(view_proj, &node.bounds)
                };
            }
            stack.extend(self.children[index].iter().copied());
        }
        counts
    }
}

/// 把「按节点」的掩码摊成「按实例」的掩码（`instance_nodes[i]` 是第 i 个实例的节点下标）。
///
/// 下标失效时按**保留**处理：掩码错位的代价是「多画一个遮挡体」，
/// 而不是「影子凭空缺一块」。
pub fn expand_masks(masks: &[Vec<bool>], instance_nodes: &[usize]) -> Vec<Vec<bool>> {
    masks
        .iter()
        .map(|mask| {
            instance_nodes
                .iter()
                .map(|index| mask.get(*index).copied().unwrap_or(true))
                .collect()
        })
        .collect()
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
