//! 寻路与可达性诊断：8 邻 A* + 视线拉直 + 「断在哪」定位。
//!
//! A* 的邻居枚举必须与烘焙时的连通块标注用**同一套**边判定（[`NavGrid::can_step`]
//! 加对角不切角），否则会出现「连通块说通、A* 说不通」这种自相矛盾的结论。
//!
//! 三种失败必须区分开，因为它们对使用者的意义完全不同：
//!
//! * [`PathOutcome::Unreachable`] —— 几何上真的走不过去（调关卡）；
//! * [`PathOutcome::InvalidEndpoint`] —— 起点/终点本身站不住（摆件悬空或埋进墙里）；
//! * [`PathOutcome::BudgetExceeded`] —— 图太大没算完，**结论未知**，不是「走不到」。
//!
//! 把后两者混进「走不到」是最容易骗到自己的地方：那不是关卡问题，是工具没算完。
//!
//! ## M2·D02 的三处增量（算法本体不动）
//!
//! 1. **分类代价**：边的代价不再是纯几何距离，而是接 [`CellClass`] 加权——斜坡按
//!    「这条边自身的高差角 / 45°」上浮，台阶加固定惩罚。于是「绕平路 3 m」与
//!    「爬 15° 斜 2 m」之间，A\* 会自己选前者，不需要关卡作者手工打点。
//! 2. **失败码细分**：`InvalidEndpoint` 拆出 [`InvalidReason`] 三子类。原来的单一
//!    失败码把「调用方传了 NaN」「问的地方不在导航范围」「地方在范围内但站不住」
//!    混成一件事，前端只能笼统说「端点非法」。
//! 3. **可观测**：[`PathStats`] 交出本次的展开数与路径代价，`BudgetExceeded`
//!    的结论也有了可核对的量；`(f, 格索引)` 双键定序保持不变（确定性是既有契约）。
//!
//! 代价加权**不破坏可采纳性**：所有分类系数都 ≥ 1，边权恒 ≥ 几何长度，octile
//! 启发式仍是下界，A\* 的最优性与一致性照旧。

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::Vec3;

use crate::grid::{NavGrid, NEIGHBORS8};
use crate::product::CellClass;

/// 单次 A* 的节点扩展上限。超出即返回 `BudgetExceeded`，不静默给半截结论。
pub const NAV_PATH_MAX_EXPANSIONS: usize = 400_000;

/// 斜坡代价的参考角（度）：`slope / 45°` 归一化到 `[0, 1]`（`M2_D_PLAN` §3.2 口径）。
pub const SLOPE_COST_REF_DEG: f32 = 45.0;

/// 斜坡代价的上浮系数：`cost × (1 + (slope/45°) · 0.5)`，即 45° 满坡贵一倍半。
pub const SLOPE_COST_GAIN: f32 = 0.5;

/// 跨一格台阶的固定惩罚（米）：让「有平路可绕」时不去无谓跨坎，但跨坎仍然划算。
pub const STEP_COST_PENALTY: f32 = 0.15;

/// 端点在哪一侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    From,
    To,
}

impl Endpoint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::From => "from",
            Self::To => "to",
        }
    }
}

/// `InvalidEndpoint` 的可诊断子类（`M2·D02`）。
///
/// 三者对使用者的下一步动作完全不同，所以必须分开：
///
/// * [`InvalidReason::NonFinite`] —— 调用方 bug（传了 `NaN` / `Inf`），重试无用；
/// * [`InvalidReason::OutOfBounds`] —— 问的地方压根不在导航范围里（换夹具或扩 `bounds`）；
/// * [`InvalidReason::OffMesh`] —— 地方在范围内但站不住（摆件悬空 / 埋进几何）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    /// 坐标含 `NaN` / `Inf`：连大小都不可比。
    NonFinite,
    /// 落在网格包围盒外：没有格子可吸附。
    OutOfBounds,
    /// 在网格内但不在任何可走格上（吸附半径内也没有可走点）。
    OffMesh,
}

impl InvalidReason {
    /// 稳定标识（协议面 / 日志用）。**不要改字符串**——改了等于改协议。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NonFinite => "nonFinite",
            Self::OutOfBounds => "outOfBounds",
            Self::OffMesh => "offMesh",
        }
    }

    /// 人话后半句，供 [`PathOutcome::describe`] 拼接（文案是契约）。
    pub fn describe(self) -> &'static str {
        match self {
            Self::NonFinite => "坐标含 NaN/Inf",
            Self::OutOfBounds => "落在网格范围外",
            Self::OffMesh => "不在任何可走格上（吸附半径内无可走点）",
        }
    }
}

/// 一次寻路的观测值（`M2·D02`）：给 `nav.*` 回包与判据测试用的量。
///
/// `expanded` 只数**真正扩展过**的节点（堆里的陈旧副本不算），所以它能直接和
/// [`NAV_PATH_MAX_EXPANSIONS`] 比；`cost` 是分类代价加权的路径长度，失败时记 `0.0`
/// ——**不要**把失败时的 `0.0` 读成「路径长度为 0」。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PathStats {
    /// 扩展过的节点数。
    pub expanded: usize,
    /// 分类代价加权的路径长度（米）；失败时为 `0.0`。
    pub cost: f32,
}

/// 寻路结果。
#[derive(Debug, Clone, PartialEq)]
pub enum PathOutcome {
    /// 找到路径（世界坐标，首尾为调用方给的端点，中间为格中心）。
    Found(Vec<Vec3>),
    /// 起终点分属不同连通块：几何上一定走不到。
    Unreachable { from_region: u32, to_region: u32 },
    /// 端点站不住：`which` 说是哪一侧，`reason` 说为什么。
    InvalidEndpoint {
        which: Endpoint,
        reason: InvalidReason,
    },
    /// 扩展次数用尽，结论未知。
    BudgetExceeded { expanded: usize },
}

impl PathOutcome {
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    /// 失败原因子类；不是 `InvalidEndpoint` 时为 `None`。
    pub fn invalid_reason(&self) -> Option<InvalidReason> {
        match self {
            Self::InvalidEndpoint { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    /// 人话描述，供日志与场景校验报告直接引用。
    ///
    /// 「不可达：…」`InvalidEndpoint` 三种子类的文案在此**冻结**（`M2_D_PLAN` §3.5）：
    /// 前缀恒为 `不可达：<from|to> 端点`，后半句由 [`InvalidReason::describe`] 给。
    pub fn describe(&self) -> String {
        match self {
            Self::Found(p) => format!("找到路径（{} 个路点）", p.len()),
            Self::Unreachable {
                from_region,
                to_region,
            } => format!("不可达：起点在连通块 {from_region}，终点在 {to_region}"),
            Self::InvalidEndpoint { which, reason } => {
                format!("不可达：{} 端点{}", which.as_str(), reason.describe())
            }
            Self::BudgetExceeded { expanded } => {
                format!("未算完：扩展 {expanded} 个节点后达到上限，结论未知")
            }
        }
    }
}

/// 从 `from` 走到 `to`，连同本次的观测值一起返回。
///
/// 端点先做一次「最多一格」的吸附——实际使用中起点常见是角色当前站的位置，
/// 略高于地面或压在两格交界上，硬判会莫名其妙地报 InvalidEndpoint。吸附也失败时，
/// 由 [`endpoint_reason`] 判明是「坐标坏」「范围外」还是「站不住」。
pub(crate) fn find_path(grid: &NavGrid, from: Vec3, to: Vec3) -> (PathOutcome, PathStats) {
    let snap = grid.config().cell_size;
    let start = match snap_endpoint(grid, from, snap) {
        Ok(c) => c,
        Err(reason) => {
            return failed(PathOutcome::InvalidEndpoint {
                which: Endpoint::From,
                reason,
            })
        }
    };
    let goal = match snap_endpoint(grid, to, snap) {
        Ok(c) => c,
        Err(reason) => {
            return failed(PathOutcome::InvalidEndpoint {
                which: Endpoint::To,
                reason,
            })
        }
    };
    astar(grid, start, goal, from, to)
}

/// 端点 → 可走格：先原位判，再一格内吸附。
fn snap_endpoint(grid: &NavGrid, p: Vec3, snap: f32) -> Result<[u32; 2], InvalidReason> {
    if let Some(c) = grid
        .cell_of(p)
        .filter(|&c| grid.is_walkable_cell(c[0], c[1]))
    {
        return Ok(c);
    }
    match grid.nearest_walkable(p, snap, None) {
        Some(q) => Ok(grid.cell_of(q).expect("nearest_walkable 必落在网格内")),
        None => Err(endpoint_reason(grid, p)),
    }
}

/// 吸附失败后判子类。**顺序有意义**：坏坐标先于范围判定，否则 `NaN` 会被
/// 说成「范围外」，把调用方 bug 伪装成夹具问题。
fn endpoint_reason(grid: &NavGrid, p: Vec3) -> InvalidReason {
    if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
        return InvalidReason::NonFinite;
    }
    match grid.cell_of(p) {
        Some(_) => InvalidReason::OffMesh,
        None => InvalidReason::OutOfBounds,
    }
}

/// 失败时的一对返回值：`PathStats` 归零（不要读成「长度为 0 的路径」）。
fn failed(outcome: PathOutcome) -> (PathOutcome, PathStats) {
    (outcome, PathStats::default())
}

/// 格坐标之间的 A*；`from_world` / `to_world` 只用于把首尾点原样带回。
///
/// 边权来自 [`step_cost`]（分类代价），启发式仍是几何 octile —— 因为所有分类
/// 系数都 ≥ 1，边权恒 ≥ 几何长度，启发式因此仍然是**可采纳且一致**的下界。
/// 开放表按 `(f, 格索引)` 双键定序，f 相等时的 pop 顺序与格枚举顺序一致，
/// 同一份数据重跑逐点相同（`M2·D02` 的确定性判据就钉在这里）。
pub(crate) fn astar(
    grid: &NavGrid,
    start: [u32; 2],
    goal: [u32; 2],
    from_world: Vec3,
    to_world: Vec3,
) -> (PathOutcome, PathStats) {
    let from_region = grid.region_at(start[0], start[1]);
    let to_region = grid.region_at(goal[0], goal[1]);
    if from_region == 0 {
        // 走到这里说明调用方给的格不可走（`find_path` 已滤过一遍）：
        // 物理上「在范围内但站不住」，所以是 `OffMesh` 而不是范围外。
        return failed(PathOutcome::InvalidEndpoint {
            which: Endpoint::From,
            reason: InvalidReason::OffMesh,
        });
    }
    if to_region == 0 {
        return failed(PathOutcome::InvalidEndpoint {
            which: Endpoint::To,
            reason: InvalidReason::OffMesh,
        });
    }
    if from_region != to_region {
        return failed(PathOutcome::Unreachable {
            from_region,
            to_region,
        });
    }

    let dims = grid.dims();
    let total = grid.cell_count();
    let si = grid.index(start[0], start[1]);
    let gi = grid.index(goal[0], goal[1]);
    if si == gi {
        // 同格：0 次扩展、0 代价，路径就是「原地到原地」的两个端点。
        return (
            PathOutcome::Found(vec![from_world, to_world]),
            PathStats::default(),
        );
    }

    let mut g = vec![f32::INFINITY; total];
    let mut came = vec![u32::MAX; total];
    let mut open: BinaryHeap<Frontier> = BinaryHeap::new();
    g[si] = 0.0;
    open.push(Frontier {
        f: heur(grid, start, goal),
        idx: si as u32,
    });
    let mut expanded = 0usize;
    let mut found = false;

    while let Some(node) = open.pop() {
        let cur = node.idx as usize;
        if cur == gi {
            found = true;
            break;
        }
        if node.f > g[cur] + heur_only(grid, cur, gi) + 1e-3 {
            // 堆里的陈旧项（同一格被更优的 g 重新推入过），跳过。
            continue;
        }
        expanded += 1;
        if expanded > NAV_PATH_MAX_EXPANSIONS {
            return (
                PathOutcome::BudgetExceeded { expanded },
                PathStats {
                    expanded,
                    cost: 0.0,
                },
            );
        }
        let ci = (cur % dims[0] as usize) as i32;
        let cj = (cur / dims[0] as usize) as i32;
        for (di, dj) in NEIGHBORS8 {
            let ni = ci + di;
            let nj = cj + dj;
            if ni < 0 || nj < 0 || ni >= dims[0] as i32 || nj >= dims[1] as i32 {
                continue;
            }
            let next = [ni as u32, nj as u32];
            // 对角不许切角：两侧直邻都得可走，与烘焙时的连通块标注口径一致。
            if di != 0
                && dj != 0
                && (!grid.is_walkable_cell(ni as u32, cj as u32)
                    || !grid.is_walkable_cell(ci as u32, nj as u32))
            {
                continue;
            }
            if !grid.can_step([ci as u32, cj as u32], next) {
                continue;
            }
            // 分类代价：不再是无脑「几何长度」，斜坡/台阶在这里变贵。
            let step = step_cost(grid, [ci as u32, cj as u32], next, di != 0 && dj != 0);
            let ng = g[cur] + step;
            let nidx = grid.index(next[0], next[1]);
            if ng + 1e-4 < g[nidx] {
                g[nidx] = ng;
                came[nidx] = cur as u32;
                open.push(Frontier {
                    f: ng + heur(grid, next, goal),
                    idx: nidx as u32,
                });
            }
        }
    }

    if !found {
        // 同一连通块内却搜不到：正常几何下不该发生（连通块标注与 A* 同源），
        // 真出现了就按「不可达」如实上报，同时把展开数交出去便于定位。
        return (
            PathOutcome::Unreachable {
                from_region,
                to_region,
            },
            PathStats {
                expanded,
                cost: 0.0,
            },
        );
    }

    // 反走 came 还原格序列（si → gi）。
    let mut cells: Vec<[u32; 2]> = Vec::new();
    let mut cur = gi;
    loop {
        cells.push([
            (cur % dims[0] as usize) as u32,
            (cur / dims[0] as usize) as u32,
        ]);
        if cur == si {
            break;
        }
        let prev = came[cur];
        if prev == u32::MAX {
            return (
                PathOutcome::Unreachable {
                    from_region,
                    to_region,
                },
                PathStats {
                    expanded,
                    cost: 0.0,
                },
            );
        }
        cur = prev as usize;
    }
    cells.reverse();

    let cells = pull_string(grid, cells);
    let mut points: Vec<Vec3> = Vec::with_capacity(cells.len() + 2);
    points.push(from_world);
    for (k, c) in cells.iter().enumerate() {
        // 首尾格由调用方的精确端点代替，避免路径里出现「先退到格中心」的抖动。
        if k == 0 || k + 1 == cells.len() {
            continue;
        }
        points.push(grid.cell_center(c[0], c[1]));
    }
    points.push(to_world);
    (
        PathOutcome::Found(points),
        PathStats {
            expanded,
            cost: g[gi],
        },
    )
}

/// 单步代价：几何长度 × 分类系数（`M2·D02`，口径见 `M2_D_PLAN` §3.2）。
///
/// * `Flat` / `Blocked` —— 原样（`Blocked` 已被 [`NavGrid::can_step`] 挡住，这里只求穷尽分支）；
/// * `Slope` —— `1 + (这条边自身的高差角 / 45°) × 0.5`。**用边自身的高差**而不是整格
///   最坏边，否则「平地里嵌一格坡」会把整片平路都算贵，A\* 反而去穿坡；
/// * `Step` —— 加 [`STEP_COST_PENALTY`] 固定惩罚（米），避免「有平路却偏要跨坎」。
fn step_cost(grid: &NavGrid, from: [u32; 2], to: [u32; 2], diagonal: bool) -> f32 {
    let cell = grid.config().cell_size;
    let base = if diagonal {
        cell * std::f32::consts::SQRT_2
    } else {
        cell
    };
    match grid.class_of(to[0], to[1]) {
        Some(CellClass::Slope) => {
            let rise = (grid.ground(to[0], to[1]) - grid.ground(from[0], from[1])).abs();
            let deg = (rise / base).atan().to_degrees();
            base * (1.0 + (deg / SLOPE_COST_REF_DEG).clamp(0.0, 1.0) * SLOPE_COST_GAIN)
        }
        Some(CellClass::Step) => base + STEP_COST_PENALTY,
        _ => base,
    }
}

/// 视线拉直：能一步看通的中间格全丢掉，只留拐点。
///
/// 每段用**世界坐标逐点采样**判定，而不是格子索引空间的直线遍历：路径端点是
/// 格中心之外的精确点（角色实际位置），按「格中心到格中心」推出来的格序列会和
/// 真实线段经过的格差一两格，于是拉直会切过墙角。二分找最远可见点，避免 O(n²)。
fn pull_string(grid: &NavGrid, cells: Vec<[u32; 2]>) -> Vec<[u32; 2]> {
    if cells.len() <= 2 {
        return cells;
    }
    let centers: Vec<Vec3> = cells.iter().map(|c| grid.cell_center(c[0], c[1])).collect();
    let last = cells.len() - 1;
    let mut out: Vec<[u32; 2]> = vec![cells[0]];
    let mut i = 0usize;
    while i < last {
        let far = if segment_ok(grid, centers[i], centers[last]) {
            last
        } else {
            let (mut lo, mut hi) = (i + 1, last);
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if segment_ok(grid, centers[i], centers[mid]) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            lo.max(i + 1)
        };
        out.push(cells[far]);
        i = far;
    }
    out
}

/// 世界空间线段是否整段可走：按半格步长采样，逐点判可走、逐段判可迈。
fn segment_ok(grid: &NavGrid, a: Vec3, b: Vec3) -> bool {
    let cell = grid.config().cell_size;
    let dist = a.distance(b);
    let steps = (dist / (cell * 0.5)).ceil().max(1.0) as usize;
    let mut prev: Option<[u32; 2]> = None;
    for k in 0..=steps {
        let t = k as f32 / steps as f32;
        let p = a.lerp(b, t);
        let Some(c) = grid.cell_of(p) else {
            return false;
        };
        if !grid.is_walkable_cell(c[0], c[1]) {
            return false;
        }
        if let Some(prev) = prev {
            if prev != c && !grid.can_step(prev, c) {
                return false;
            }
        }
        prev = Some(c);
    }
    true
}

/// 诊断用：从 `from` 侧能走到、且离 `to` 最近的那个点。
///
/// 「走不到」只说明结论，不说明卡在哪；关卡作者需要的是「最多能走到这儿」，
/// 一眼看出是门没开还是桥断了。
pub(crate) fn frontier_nearest(grid: &NavGrid, from: Vec3, to: Vec3) -> Option<Vec3> {
    let start = grid
        .cell_of(from)
        .filter(|&c| grid.is_walkable_cell(c[0], c[1]))?;
    let target = grid.cell_of(to)?;
    let dims = grid.dims();
    let total = grid.cell_count();
    let mut seen = vec![false; total];
    let mut queue: Vec<usize> = Vec::new();
    let si = grid.index(start[0], start[1]);
    seen[si] = true;
    queue.push(si);
    let (ti, tj) = (target[0] as i32, target[1] as i32);
    let mut best: Option<(f32, [u32; 2])> = None;
    let mut head = 0;
    while head < queue.len() {
        let cur = queue[head];
        head += 1;
        let ci = (cur % dims[0] as usize) as u32;
        let cj = (cur / dims[0] as usize) as u32;
        let d = (((ci as i32 - ti).pow(2) + (cj as i32 - tj).pow(2)) as f32).sqrt();
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, [ci, cj]));
        }
        for (di, dj) in NEIGHBORS8 {
            let ni = ci as i32 + di;
            let nj = cj as i32 + dj;
            if ni < 0 || nj < 0 || ni >= dims[0] as i32 || nj >= dims[1] as i32 {
                continue;
            }
            if di != 0
                && dj != 0
                && (!grid.is_walkable_cell(ni as u32, cj) || !grid.is_walkable_cell(ci, nj as u32))
            {
                continue;
            }
            let next = [ni as u32, nj as u32];
            if !grid.can_step([ci, cj], next) {
                continue;
            }
            let nidx = grid.index(next[0], next[1]);
            if seen[nidx] {
                continue;
            }
            seen[nidx] = true;
            queue.push(nidx);
        }
    }
    best.map(|(_, c)| grid.cell_center(c[0], c[1]))
}

/// 堆里的一项：f 值 + 格索引。
#[derive(Debug, Clone, Copy)]
struct Frontier {
    f: f32,
    idx: u32,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f && self.idx == other.idx
    }
}

impl Eq for Frontier {}

impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // 小顶堆：f 小的优先；f 相等时按 idx 定序，保证 pop 顺序稳定可复现。
        other
            .f
            .partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.idx.cmp(&self.idx))
    }
}

impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 启发式：八向 octile 距离（可采纳、且不低估对角）。
fn heur(grid: &NavGrid, a: [u32; 2], b: [u32; 2]) -> f32 {
    let dx = (a[0] as i32 - b[0] as i32).unsigned_abs();
    let dz = (a[1] as i32 - b[1] as i32).unsigned_abs();
    let (lo, hi) = (dx.min(dz) as f32, dx.max(dz) as f32);
    (lo * std::f32::consts::SQRT_2 + (hi - lo)) * grid.config().cell_size
}

/// 从格索引算启发式（A* 里需要按索引取格坐标）。
fn heur_only(grid: &NavGrid, idx: usize, goal: usize) -> f32 {
    let dims = grid.dims();
    heur(
        grid,
        [
            (idx % dims[0] as usize) as u32,
            (idx / dims[0] as usize) as u32,
        ],
        [
            (goal % dims[0] as usize) as u32,
            (goal / dims[0] as usize) as u32,
        ],
    )
}
