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

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::Vec3;

use crate::grid::{NavGrid, NEIGHBORS8};

/// 单次 A* 的节点扩展上限。超出即返回 `BudgetExceeded`，不静默给半截结论。
pub const NAV_PATH_MAX_EXPANSIONS: usize = 400_000;

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

/// 寻路结果。
#[derive(Debug, Clone, PartialEq)]
pub enum PathOutcome {
    /// 找到路径（世界坐标，首尾为调用方给的端点，中间为格中心）。
    Found(Vec<Vec3>),
    /// 起终点分属不同连通块：几何上一定走不到。
    Unreachable { from_region: u32, to_region: u32 },
    /// 端点吸附不到任何可走格。
    InvalidEndpoint { which: Endpoint },
    /// 扩展次数用尽，结论未知。
    BudgetExceeded { expanded: usize },
}

impl PathOutcome {
    pub fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    /// 人话描述，供日志与场景校验报告直接引用。
    pub fn describe(&self) -> String {
        match self {
            Self::Found(p) => format!("找到路径（{} 个路点）", p.len()),
            Self::Unreachable {
                from_region,
                to_region,
            } => format!("不可达：起点在连通块 {from_region}，终点在 {to_region}"),
            Self::InvalidEndpoint { which } => {
                format!("不可达：{} 端点不在任何可走格上", which.as_str())
            }
            Self::BudgetExceeded { expanded } => {
                format!("未算完：扩展 {expanded} 个节点后达到上限，结论未知")
            }
        }
    }
}

/// 从 `from` 走到 `to`。
///
/// 端点先做一次「最多一格」的吸附——实际使用中起点常见是角色当前站的位置，
/// 略高于地面或压在两格交界上，硬判会莫名其妙地报 InvalidEndpoint。
pub(crate) fn find_path(grid: &NavGrid, from: Vec3, to: Vec3) -> PathOutcome {
    let snap = grid.config().cell_size;
    let start = match grid
        .cell_of(from)
        .filter(|&c| grid.is_walkable_cell(c[0], c[1]))
    {
        Some(c) => c,
        None => match grid.nearest_walkable(from, snap, None) {
            Some(p) => grid.cell_of(p).expect("nearest_walkable 必落在网格内"),
            None => {
                return PathOutcome::InvalidEndpoint {
                    which: Endpoint::From,
                }
            }
        },
    };
    let goal = match grid
        .cell_of(to)
        .filter(|&c| grid.is_walkable_cell(c[0], c[1]))
    {
        Some(c) => c,
        None => match grid.nearest_walkable(to, snap, None) {
            Some(p) => grid.cell_of(p).expect("nearest_walkable 必落在网格内"),
            None => {
                return PathOutcome::InvalidEndpoint {
                    which: Endpoint::To,
                }
            }
        },
    };
    astar(grid, start, goal, from, to)
}

/// 格坐标之间的 A*；`from_world` / `to_world` 只用于把首尾点原样带回。
pub(crate) fn astar(
    grid: &NavGrid,
    start: [u32; 2],
    goal: [u32; 2],
    from_world: Vec3,
    to_world: Vec3,
) -> PathOutcome {
    let from_region = grid.region_at(start[0], start[1]);
    let to_region = grid.region_at(goal[0], goal[1]);
    if from_region == 0 {
        return PathOutcome::InvalidEndpoint {
            which: Endpoint::From,
        };
    }
    if to_region == 0 {
        return PathOutcome::InvalidEndpoint {
            which: Endpoint::To,
        };
    }
    if from_region != to_region {
        return PathOutcome::Unreachable {
            from_region,
            to_region,
        };
    }

    let dims = grid.dims();
    let total = grid.cell_count();
    let si = grid.index(start[0], start[1]);
    let gi = grid.index(goal[0], goal[1]);
    if si == gi {
        return PathOutcome::Found(vec![from_world, to_world]);
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
            return PathOutcome::BudgetExceeded { expanded };
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
            let step = if di != 0 && dj != 0 {
                grid.config().cell_size * std::f32::consts::SQRT_2
            } else {
                grid.config().cell_size
            };
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
        return PathOutcome::Unreachable {
            from_region,
            to_region,
        };
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
            return PathOutcome::Unreachable {
                from_region,
                to_region,
            };
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
    PathOutcome::Found(points)
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
