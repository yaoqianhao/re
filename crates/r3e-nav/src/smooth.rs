//! 路径平滑：视线剪枝（string-pulling）+ 拐角内缩（`M2·D03`）。
//!
//! ## 为什么不是「严格漏斗」
//!
//! 教科书漏斗算法（simple stupid funnel）要求输入是**三角形通道**，且要按左右
//! 链同时推进、在 `apex` 与两侧链之间反复收缩。本仓库的 `DD-1 = A` 档只保证
//! 「格 + 可走位图」这一层，没有三角剖分可依；硬造一份 CPU 三角网属于另一张卡
//! 的工程量。所以这里用**格级视线检验**（[`NavGrid::line_walkable`] 加半格采样）
//! 等价实现「能直着走就别拐弯」，再对保留下来的拐点做**内缩**，把顶点从障碍
//! 凸角旁推开一个身位。
//!
//! 差别只在「路径最优性」：漏斗给出的是通道内的最短折线，本实现给出的是一条
//! 合法、单调更短、且离障碍更远的折线。判据（`M2_D_PLAN` §6）要的正是后三条。
//!
//! ## 三道防线
//!
//! 1. **剪枝只做减法**：被剪掉的点必须「两头都看得通」，每段的格级视线与
//!    世界采样**同时**为真（只看格级会被端点偏离格中心骗过去，只看采样会漏掉
//!    对角切角）；
//! 2. **内缩可失败**：退不进去（不可走、或退完两段反而看不见）就保留原顶点，
//!    宁钝不穿；
//! 3. **总长不增**：内缩后的总长若比输入更长，整体回退到纯剪枝结果——平滑
//!    永远不该让路变长，这是调用方敢直接替换路径的依据。

use glam::Vec3;

use crate::grid::NavGrid;
use crate::path::segment_ok;

/// 拐角内缩的额外余量（米）：`agent_radius + 这个值` 才是退让距离。
///
/// 余量存在的原因：`agent_radius` 是**烘焙时**把障碍外扩的量，格离散化之后
/// 边界上还留着最多半格的对角空隙，退让必须再往外让一点才敢说「不贴墙」。
pub const CORNER_INSET_MARGIN: f32 = 0.05;

/// 剪枝/内缩之后的一条折线。
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedPath {
    /// 平滑后的路点；首尾与输入**逐位相同**（调用方的精确端点不许被挪动）。
    pub points: Vec<Vec3>,
    /// 折线总长（含高差的三维长度，米）。
    pub length: f32,
    /// 相对输入被剪掉的顶点数（`输入点数 − 输出点数`，内缩不算剪枝）。
    pub pruned: usize,
}

impl SmoothedPath {
    /// 路点数（`points.len()` 的简写，回包 `count` 用）。
    pub fn count(&self) -> usize {
        self.points.len()
    }
}

/// 折线的三维总长。
pub fn polyline_length(points: &[Vec3]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()
}

/// 把一条折线平滑成「可直走的直线段 + 内缩拐点」。
///
/// 输入不要求来自 A\*：任何一串首尾相接的点（格中心序列、脚本给定的航点）都行。
/// 输入本身被视为**已知可行**，因此极端输入（0 / 1 点）原样返回，不报错——
/// 调用方已经拿到一份能走的路，平滑器没有资格把它变成错误。
pub fn smooth_path(grid: &NavGrid, points: &[Vec3]) -> SmoothedPath {
    let input: Vec<Vec3> = dedup(points);
    let raw_len = polyline_length(&input);
    if input.len() <= 2 {
        return SmoothedPath {
            length: raw_len,
            pruned: points.len().saturating_sub(input.len()),
            points: input,
        };
    }

    let pruned = pull(grid, &input);
    let insets = inset_candidates(grid, &pruned);

    // 第三道防线：内缩是「花长度换身位」的动作，必须花在预算里。
    // 预算 = 剪枝已经省下来的长度（`raw_len` 与剪枝结果之差），逐点申请；
    // 超了就从最后一个生效的点开始回退——全部退完就退化成纯剪枝结果，
    // 而纯剪枝只会更短，所以循环一定收敛。
    let mut applied: Vec<bool> = insets.iter().map(|c| c.is_some()).collect();
    let (final_points, length) = loop {
        let pts = compose(&pruned, &insets, &applied);
        let len = polyline_length(&pts);
        if len <= raw_len + 1e-4 {
            break (pts, len);
        }
        match (0..applied.len()).rev().find(|k| applied[*k]) {
            Some(k) => applied[k] = false,
            None => break (pts, len),
        }
    };

    SmoothedPath {
        pruned: points.len().saturating_sub(final_points.len()),
        points: final_points,
        length,
    }
}

/// 去掉相邻重复点（距离小于 1 mm）：重复点会让夹角变成 `NaN`。
fn dedup(points: &[Vec3]) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::with_capacity(points.len());
    for p in points {
        match out.last() {
            Some(last) if last.distance(*p) < 1e-3 => {}
            _ => out.push(*p),
        }
    }
    out
}

/// 视线剪枝：从当前点向前找**最远**可见点，中间的点全部丢掉。
///
/// 先试终点（大多数几何一次就到底），试不通再二分——二分成立的前提是「可见性
/// 沿折线单调」：折线上的点是从近到远排的，一旦某个点看不见，更远的点也不会
/// 突然又看得见。这个前提在凸形障碍下成立，在凹形障碍下偶尔不成立（会少剪一个
/// 点），代价只是「路径多留一个拐点」，不会产生穿墙段。
fn pull(grid: &NavGrid, points: &[Vec3]) -> Vec<Vec3> {
    let last = points.len() - 1;
    let mut out: Vec<Vec3> = vec![points[0]];
    let mut i = 0usize;
    while i < last {
        let far = if sees(grid, points[i], points[last]) {
            last
        } else {
            let (mut lo, mut hi) = (i + 1, last);
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if sees(grid, points[i], points[mid]) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            lo.max(i + 1)
        };
        out.push(points[far]);
        i = far;
    }
    out
}

/// 拐角内缩候选：每个内部顶点沿**角平分线**退 `agent_radius + 0.05` 米。
///
/// 方向取 `−(a + b)`（`a`/`b` 是从拐点指向两个邻点的单位向量）：这个方向与
/// 「入射方向 + 出射方向」的角平分线一致，是折线在拐点处**凸出去的那一侧**。
/// 绕障碍时凸侧正对障碍凸角，所以朝这一侧退就是在给身位留量。
/// 退点必须同时满足「自己站得住」和「两侧仍然看得通」，否则放弃这个顶点。
///
/// 注意：这里只产候选，不判长度——退化让路径必然略微变长，长度预算由
/// `smooth_path` 统一把关（剪枝省下来的长度先花，超了就退回原顶点）。
fn inset_candidates(grid: &NavGrid, points: &[Vec3]) -> Vec<Option<Vec3>> {
    let dist = grid.config().agent_radius + CORNER_INSET_MARGIN;
    let mut out: Vec<Option<Vec3>> = vec![None; points.len()];
    if points.len() <= 2 {
        return out;
    }
    for k in 1..points.len() - 1 {
        let (prev, cur, next) = (points[k - 1], points[k], points[k + 1]);
        out[k] = inset_one(grid, prev, cur, next, dist)
            .filter(|p| sees(grid, prev, *p) && sees(grid, *p, next));
    }
    out
}

/// 按 `applied` 把候选点贴回折线（未生效的顶点保留剪枝后的原位置）。
fn compose(base: &[Vec3], insets: &[Option<Vec3>], applied: &[bool]) -> Vec<Vec3> {
    base.iter()
        .enumerate()
        .map(|(k, p)| match (&insets[k], applied[k]) {
            (Some(cand), true) => *cand,
            _ => *p,
        })
        .collect()
}

/// 单个顶点的内缩候选点；几何退化（共线 / 原路折返 / 越界）一律 `None`。
fn inset_one(grid: &NavGrid, prev: Vec3, cur: Vec3, next: Vec3, dist: f32) -> Option<Vec3> {
    let a = flat_dir(prev, cur)?;
    let b = flat_dir(next, cur)?;
    let sum = a + b;
    if sum.length() < 1e-3 {
        // 180° 原路折返：没有「内侧」可分，退让方向不确定。
        return None;
    }
    // 取负 = 入射方向与出射方向的角平分线，指向折线弯曲的内侧。
    let dir = -sum.normalize();
    let p = Vec3::new(cur.x + dir.x * dist, cur.y, cur.z + dir.z * dist);
    if !grid.is_walkable(p) {
        return None;
    }
    grid.ground_at(p).map(|y| Vec3::new(p.x, y, p.z))
}

/// 「从 `cur` 指向 `dir_to`」的单位水平向量（Y 不参与：拐角是水平面上的角）。
fn flat_dir(dir_to: Vec3, cur: Vec3) -> Option<Vec3> {
    let d = Vec3::new(dir_to.x - cur.x, 0.0, dir_to.z - cur.z);
    let len = d.length();
    if !len.is_finite() || len < 1e-4 {
        return None;
    }
    Some(d / len)
}

/// 两点之间是否「看得通」：格级 `supercover` 视线 **且** 世界半格采样都通过。
///
/// 两道一起用的理由：格级只认格中心到格中心的连线，端点偏离格中心时会切墙角；
/// 世界采样只看可走位，会漏掉「对角跨格时两侧都没墙、实际贴着角落擦过去」。
fn sees(grid: &NavGrid, a: Vec3, b: Vec3) -> bool {
    if a.distance(b) < 1e-4 {
        return true;
    }
    let (Some(ca), Some(cb)) = (grid.cell_of(a), grid.cell_of(b)) else {
        return false;
    };
    grid.line_walkable(ca, cb) && segment_ok(grid, a, b)
}
