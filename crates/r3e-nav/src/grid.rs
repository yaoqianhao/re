//! 导航网格的数据结构：体素高度 + 可走位图 + 连通块标注。
//!
//! 为什么是「2.5D 体素」而不是三维体素：M1 的角色只在地面上走，判定一个问题
//! （「从出生点能不能走到宝箱」）只需要「每个 XZ 格上是否站得住」这一个 bit，
//! 加一份地面高度。三维体素要 10 倍内存换来的垂直分层能力，M1 用不上。
//!
//! 内存：1 km² / 0.5 m 格 = 2000×2000 = 400 万格；可走位图按 bit 存 = 500 KB，
//! 高度 `Vec<f32>` = 16 MB，连通块 `Vec<u32>` = 16 MB。这是「烘焙一次、查询多次」的
//! 典型权衡——烘焙慢一点没关系，查询要 O(1)。
//!
//! 坐标约定：
//!
//! * 格 `(i, j)` 对应世界矩形 `[min.x + i*cell, min.x + (i+1)*cell) × [min.z + j*cell, ...)`；
//! * 格中心是 `min + (i + 0.5) * cell`，**所有世界坐标 → 格的换算都用 floor**，
//!   与「格覆盖半开区间」保持一致，避免边界上两个格都认领同一个点；
//! * 数组行主序（`j` 为行、`i` 为列），索引 = `j * dims[0] + i`。

use glam::Vec3;

use crate::NavConfig;

/// 单轴格数上限：防止误传一个巨大的世界范围把内存吃光。
pub const NAV_MAX_SIDE: u32 = 2048;

/// 总格数上限（400 万 ≈ 1 km² / 0.5 m）。
pub const NAV_MAX_CELLS: u64 = 4_000_000;

/// 一份烘焙好的导航网格。
///
/// 所有字段只读访问，唯一可写入口是 [`NavGrid::bake_heights`] 与
/// [`NavGrid::flood_regions`]（crate 内部），保证「格高 / 可走 / 连通块」三者
/// 始终由同一份烘焙逻辑写出来。
#[derive(Debug, Clone)]
pub struct NavGrid {
    cfg: NavConfig,
    dims: [u32; 2],
    min: [f32; 2],
    /// 每格地面高度（米，世界 Y）。
    heights: Vec<f32>,
    /// 可走位图：bit = 1 表示这一格站得住（不含坡度/步高约束，那是边属性）。
    walkable: Vec<u64>,
    /// 连通块 id，`0` 表示不可走，其余从 1 起。
    region: Vec<u32>,
    /// 每个连通块的格数，`region_cells[id - 1]`。
    region_cells: Vec<u32>,
    /// 烘焙时无法从几何判定的网格资产名（记进报告，不猜）。
    unresolved: Vec<String>,
}

impl NavGrid {
    /// crate 内部构造：调用方须保证 `heights.len() == dims[0] * dims[1]`。
    pub(crate) fn new(cfg: NavConfig, dims: [u32; 2], min: [f32; 2], heights: Vec<f32>) -> Self {
        let cells = dims[0] as usize * dims[1] as usize;
        debug_assert_eq!(heights.len(), cells, "高度样本数必须等于格数");
        Self {
            cfg,
            dims,
            min,
            heights,
            walkable: vec![0u64; cells.div_ceil(64)],
            region: vec![0u32; cells],
            region_cells: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    pub fn config(&self) -> &NavConfig {
        &self.cfg
    }

    pub fn dims(&self) -> [u32; 2] {
        self.dims
    }

    /// 世界矩形 `[min_x, min_z, max_x, max_z]`。
    pub fn bounds(&self) -> [f32; 4] {
        let cell = self.cfg.cell_size;
        [
            self.min[0],
            self.min[1],
            self.min[0] + self.dims[0] as f32 * cell,
            self.min[1] + self.dims[1] as f32 * cell,
        ]
    }

    pub fn cell_count(&self) -> usize {
        self.dims[0] as usize * self.dims[1] as usize
    }

    /// crate 内部：烘焙时登记「没有几何可判」的网格资产（外部 glTF 等）。
    pub(crate) fn note_unresolved(&mut self, name: String) {
        if !self.unresolved.contains(&name) && self.unresolved.len() < 64 {
            self.unresolved.push(name);
        }
    }

    pub(crate) fn unresolved(&self) -> &[String] {
        &self.unresolved
    }

    // ---------- 索引与坐标换算 ----------

    pub fn index(&self, i: u32, j: u32) -> usize {
        j as usize * self.dims[0] as usize + i as usize
    }

    /// 世界坐标 → 格。落在矩形外（或含 NaN）一律 `None`。
    pub fn cell_of(&self, p: Vec3) -> Option<[u32; 2]> {
        let cell = self.cfg.cell_size;
        let fx = (p.x - self.min[0]) / cell;
        let fz = (p.z - self.min[1]) / cell;
        if !fx.is_finite() || !fz.is_finite() || fx < 0.0 || fz < 0.0 {
            return None;
        }
        let i = fx as u64;
        let j = fz as u64;
        if i >= u64::from(self.dims[0]) || j >= u64::from(self.dims[1]) {
            return None;
        }
        Some([i as u32, j as u32])
    }

    /// 格 → 格中心的世界坐标（Y 取该格地面高度）。
    pub fn cell_center(&self, i: u32, j: u32) -> Vec3 {
        let cell = self.cfg.cell_size;
        Vec3::new(
            self.min[0] + (i as f32 + 0.5) * cell,
            self.heights[self.index(i, j)],
            self.min[1] + (j as f32 + 0.5) * cell,
        )
    }

    /// 格的地面高度。
    pub fn ground(&self, i: u32, j: u32) -> f32 {
        self.heights[self.index(i, j)]
    }

    /// 世界点所在格的地面高度（越界 `None`）。
    pub fn ground_at(&self, p: Vec3) -> Option<f32> {
        self.cell_of(p).map(|[i, j]| self.ground(i, j))
    }

    // ---------- 可走位 ----------

    pub fn is_walkable_cell(&self, i: u32, j: u32) -> bool {
        if i >= self.dims[0] || j >= self.dims[1] {
            return false;
        }
        let idx = self.index(i, j);
        self.walkable[idx / 64] & (1u64 << (idx % 64)) != 0
    }

    /// 世界点是否落在可走格上。
    pub fn is_walkable(&self, p: Vec3) -> bool {
        match self.cell_of(p) {
            Some([i, j]) => self.is_walkable_cell(i, j),
            None => false,
        }
    }

    pub(crate) fn set_walkable(&mut self, i: u32, j: u32, on: bool) {
        let idx = self.index(i, j);
        let mask = 1u64 << (idx % 64);
        if on {
            self.walkable[idx / 64] |= mask;
        } else {
            self.walkable[idx / 64] &= !mask;
        }
    }

    /// 可走格数量。
    pub fn walkable_count(&self) -> usize {
        self.walkable.iter().map(|w| w.count_ones() as usize).sum()
    }

    // ---------- 连通块 ----------

    /// 格所属连通块 id（`0` = 不可走或不连通）。
    pub fn region_at(&self, i: u32, j: u32) -> u32 {
        if i >= self.dims[0] || j >= self.dims[1] {
            return 0;
        }
        self.region[self.index(i, j)]
    }

    /// 世界点所属连通块 id。
    pub fn region_of(&self, p: Vec3) -> u32 {
        match self.cell_of(p) {
            Some([i, j]) => self.region_at(i, j),
            None => 0,
        }
    }

    pub fn region_count(&self) -> u32 {
        self.region_cells.len() as u32
    }

    /// 每个连通块的格数（下标 = id - 1），按格数降序排列的「主体」是 `[0]`。
    pub fn region_sizes(&self) -> &[u32] {
        &self.region_cells
    }

    /// 标注连通块：把「可走 + 步高/坡度合法」的相邻格并成一块。
    ///
    /// 8 邻接，但**对角穿越必须两侧直邻都可走**——否则角色会从两个障碍的夹角里
    /// 「穿」过去，而实际身体过不去。这条约束与 [`Self::can_step`] 保持一致，
    /// 因此「同块 ⇔ A* 一定能连通」是成立的。
    pub fn flood_regions(&mut self) {
        let dims = self.dims;
        let total = dims[0] as usize * dims[1] as usize;
        self.region.iter_mut().for_each(|r| *r = 0);
        self.region_cells.clear();
        let mut queue: Vec<usize> = Vec::new();

        let Self {
            cfg,
            heights,
            walkable,
            region,
            region_cells,
            ..
        } = self;

        let walk = |idx: usize| walkable[idx / 64] & (1u64 << (idx % 64)) != 0;

        let mut next_id = 1u32;
        for start in 0..total {
            if region[start] != 0 || !walk(start) {
                continue;
            }
            region[start] = next_id;
            queue.clear();
            queue.push(start);
            let mut cells = 0u32;
            let mut head = 0;
            while head < queue.len() {
                let cur = queue[head];
                head += 1;
                cells += 1;
                let ci = (cur % dims[0] as usize) as i32;
                let cj = (cur / dims[0] as usize) as i32;
                for (di, dj) in NEIGHBORS8 {
                    let ni = ci + di;
                    let nj = cj + dj;
                    if ni < 0 || nj < 0 || ni >= dims[0] as i32 || nj >= dims[1] as i32 {
                        continue;
                    }
                    let nidx = nj as usize * dims[0] as usize + ni as usize;
                    if region[nidx] != 0 || !walk(nidx) {
                        continue;
                    }
                    // 对角不切角：两侧直邻也必须可走。
                    if di != 0 && dj != 0 {
                        let side_a = cj as usize * dims[0] as usize + ni as usize;
                        let side_b = nj as usize * dims[0] as usize + ci as usize;
                        if !walk(side_a) || !walk(side_b) {
                            continue;
                        }
                    }
                    if !step_ok(cfg, heights, cur, nidx, di != 0 && dj != 0) {
                        continue;
                    }
                    region[nidx] = next_id;
                    queue.push(nidx);
                }
            }
            region_cells.push(cells);
            next_id += 1;
        }
    }

    /// 从 `from` 走到相邻格 `to` 是否合法（两格都可走 + 步高/坡度在限内）。
    ///
    /// 这里**不检查对角切角**：那一步是对角专属的邻居裁剪，只在 [`Self::flood_regions`]
    /// 与 A* 的邻居枚举里做，本方法只判「高差是否允许」。
    pub fn can_step(&self, from: [u32; 2], to: [u32; 2]) -> bool {
        if !self.is_walkable_cell(to[0], to[1]) || !self.is_walkable_cell(from[0], from[1]) {
            return false;
        }
        let diag = from[0] != to[0] && from[1] != to[1];
        let a = self.index(from[0], from[1]);
        let b = self.index(to[0], to[1]);
        step_ok(&self.cfg, &self.heights, a, b, diag)
    }

    /// 直线视线：`a` 与 `b` 之间每一格都可走、且逐格可迈（拉直路径用）。
    pub fn line_walkable(&self, a: [u32; 2], b: [u32; 2]) -> bool {
        for cell in supercover(a, b) {
            if !self.is_walkable_cell(cell[0], cell[1]) {
                return false;
            }
        }
        // 高度连续性单独走一遍：逐格检查迈步是否合法。
        let cells: Vec<[u32; 2]> = supercover(a, b).collect();
        cells.windows(2).all(|w| self.can_step(w[0], w[1]))
    }

    /// 距 `p` 最近的可走格中心（搜索半径上限 `max_radius` 米）。
    ///
    /// `region` 给定时只接受该连通块内的格——用于「把掉落点拉回同一块地面上」，
    /// 否则可能把人拉到隔墙的另一侧，看着近、其实过不去。
    pub fn nearest_walkable(&self, p: Vec3, max_radius: f32, region: Option<u32>) -> Option<Vec3> {
        let cell = self.cfg.cell_size;
        let start = self.cell_of(p)?;
        let limit = (max_radius / cell).ceil() as i32;
        if limit <= 0 {
            return None;
        }
        let accept = |i: u32, j: u32| {
            self.is_walkable_cell(i, j) && region.map_or(true, |r| self.region_at(i, j) == r)
        };
        if accept(start[0], start[1]) {
            return Some(self.cell_center(start[0], start[1]));
        }
        let (si, sj) = (start[0] as i32, start[1] as i32);
        let mut best: Option<(f32, [u32; 2])> = None;
        for r in 1..=limit {
            // 只扫环上的格：内圈已经扫过，重复扫等于 O(r²) 变 O(r³)。
            for di in -r..=r {
                for dj in [-r, r] {
                    for (i, j) in [(si + di, sj + dj), (si + dj, sj + di)] {
                        if i < 0 || j < 0 || i >= self.dims[0] as i32 || j >= self.dims[1] as i32 {
                            continue;
                        }
                        let (i, j) = (i as u32, j as u32);
                        if !accept(i, j) {
                            continue;
                        }
                        if di.abs() != r && dj.abs() != r {
                            continue;
                        }
                        let c = self.cell_center(i, j);
                        let d = Vec3::new(c.x - p.x, 0.0, c.z - p.z).length();
                        if d <= max_radius && best.map_or(true, |(bd, _)| d < bd) {
                            best = Some((d, [i, j]));
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(_, [i, j])| self.cell_center(i, j))
    }
}

/// 8 邻偏移（先 4 邻后 4 对角，保证 BFS 先扩直邻）。
pub(crate) const NEIGHBORS8: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// 迈步是否合法：高差 ≤ `step_height`，且不超过坡度上限（对角要按 1.414 倍格距算）。
pub(crate) fn step_ok(
    cfg: &NavConfig,
    heights: &[f32],
    a: usize,
    b: usize,
    diagonal: bool,
) -> bool {
    let dh = (heights[a] - heights[b]).abs();
    if !dh.is_finite() {
        return false;
    }
    let dist = if diagonal {
        cfg.cell_size * std::f32::consts::SQRT_2
    } else {
        cfg.cell_size
    };
    let slope_limit = dist * cfg.max_slope_deg.to_radians().tan();
    // 两条判据取**或**，不是且：
    // * 连续斜面靠坡度判（45° 斜面每格升 0.5 m，早就超了 0.4 m 的步高，
    //   若再要求「也必须小于 step_height」，任何像样的斜坡都会被判成悬崖）；
    // * 离散台阶靠步高判（陡但只有 0.3 m 的坎，抬脚就能上）。
    let slope_ok = dh <= slope_limit + 1e-5;
    let step_ok = dh <= cfg.step_height + 1e-5;
    slope_ok || step_ok
}

/// 两格之间的超覆盖（supercover）直线：走到的每一格都返回一次，含首尾。
///
/// 用 Amanatides–Woo 体素遍历，不采点——采点会漏掉只被擦过一角的格，
/// 让「能看见」的判定比实际宽。
fn supercover(a: [u32; 2], b: [u32; 2]) -> impl Iterator<Item = [u32; 2]> {
    let (mut i, mut j) = (a[0] as i32, a[1] as i32);
    let (ti, tj) = (b[0] as i32, b[1] as i32);
    let di = (ti - i).signum();
    let dj = (tj - j).signum();
    let (dx, dy) = ((ti - i).abs(), (tj - j).abs());
    let mut out: Vec<[u32; 2]> = Vec::with_capacity((dx + dy + 1) as usize);
    out.push([i as u32, j as u32]);
    if dx == 0 && dy == 0 {
        return out.into_iter();
    }
    // 参数化：t 从 0 到 1，i 变化发生在 t = k/dx，j 变化在 t = k/dy。
    let (mut si, mut sj) = (0i32, 0i32);
    let (step_i, step_j) = (1.0 / dx as f32, 1.0 / dy as f32);
    let mut next_i = step_i;
    let mut next_j = step_j;
    while si < dx || sj < dy {
        // 平手时两个方向都走（对角穿越），避免出现一条「贴边」的伪直线。
        let take_i = sj >= dy || (si < dx && next_i <= next_j);
        let take_j = si >= dx || (sj < dy && next_j <= next_i);
        if take_i {
            i += di;
            si += 1;
            next_i += step_i;
        }
        if take_j {
            j += dj;
            sj += 1;
            next_j += step_j;
        }
        out.push([i as u32, j as u32]);
    }
    out.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_grid(dims: [u32; 2]) -> NavGrid {
        let cfg = NavConfig {
            cell_size: 1.0,
            ..NavConfig::default()
        };
        let cells = dims[0] as usize * dims[1] as usize;
        NavGrid::new(cfg, dims, [0.0, 0.0], vec![0.0; cells])
    }

    #[test]
    fn cell_of_uses_floor_and_rejects_out_of_bounds() {
        let g = flat_grid([4, 4]);
        assert_eq!(g.cell_of(Vec3::new(0.1, 0.0, 0.1)), Some([0, 0]));
        assert_eq!(g.cell_of(Vec3::new(0.99, 0.0, 0.99)), Some([0, 0]));
        assert_eq!(g.cell_of(Vec3::new(1.0, 0.0, 0.0)), Some([1, 0]));
        assert_eq!(g.cell_of(Vec3::new(-0.01, 0.0, 0.0)), None);
        assert_eq!(g.cell_of(Vec3::new(4.0, 0.0, 0.0)), None);
        assert_eq!(g.cell_of(Vec3::new(f32::NAN, 0.0, 0.0)), None);
    }

    #[test]
    fn cell_center_is_half_cell_offset() {
        let g = flat_grid([4, 4]);
        assert_eq!(g.cell_center(0, 0), Vec3::new(0.5, 0.0, 0.5));
        assert_eq!(g.cell_center(3, 2), Vec3::new(3.5, 0.0, 2.5));
    }

    #[test]
    fn walkable_bitset_roundtrip() {
        let mut g = flat_grid([70, 3]);
        g.set_walkable(69, 2, true);
        assert!(g.is_walkable_cell(69, 2));
        assert!(!g.is_walkable_cell(68, 2));
        assert_eq!(g.walkable_count(), 1);
        g.set_walkable(69, 2, false);
        assert!(!g.is_walkable_cell(69, 2));
        assert!(!g.is_walkable_cell(70, 0), "越界必须是不可走");
    }

    #[test]
    fn flood_regions_splits_disconnected_islands() {
        let mut g = flat_grid([5, 1]);
        // 中间一格挖空 → 左右两块互不连通。
        for i in [0, 1, 3, 4] {
            g.set_walkable(i, 0, true);
        }
        g.flood_regions();
        assert_eq!(g.region_count(), 2);
        assert_ne!(g.region_at(0, 0), g.region_at(4, 0));
        assert_eq!(g.region_at(2, 0), 0);
        assert_eq!(g.region_sizes(), &[2, 2]);
    }

    #[test]
    fn diagonal_crossing_forbidden_when_both_sides_blocked() {
        let mut g = flat_grid([3, 3]);
        g.set_walkable(0, 0, true);
        g.set_walkable(1, 1, true);
        // 两个直邻都不可走 → 对角不许连
        g.flood_regions();
        assert_ne!(g.region_at(0, 0), g.region_at(1, 1));
    }

    #[test]
    fn step_height_limits_connectivity() {
        // 步高管**离散坎**：1 m 格子里 2 m 落差 = 63°，既超 45° 坡限也超 0.3 步高。
        let cfg = NavConfig {
            cell_size: 1.0,
            step_height: 0.3,
            max_slope_deg: 45.0,
            ..NavConfig::default()
        };
        let mut g = NavGrid::new(cfg.clone(), [2, 1], [0.0, 0.0], vec![0.0, 2.0]);
        g.set_walkable(0, 0, true);
        g.set_walkable(1, 0, true);
        g.flood_regions();
        assert_ne!(
            g.region_at(0, 0),
            g.region_at(1, 0),
            "2 米落差既超 45° 坡限也超 0.3 步高，必须断开"
        );

        // 同落差把坡限放宽到 89°（≈ 2 m/1 m 是 63°，在限内）后必须连通：
        // 坡度与步高是「或」关系，坡限内的斜面不该被 step_height 判死。
        let cfg2 = NavConfig {
            max_slope_deg: 89.0,
            ..cfg
        };
        let mut g2 = NavGrid::new(cfg2, [2, 1], [0.0, 0.0], vec![0.0, 2.0]);
        g2.set_walkable(0, 0, true);
        g2.set_walkable(1, 0, true);
        g2.flood_regions();
        assert_eq!(
            g2.region_at(0, 0),
            g2.region_at(1, 0),
            "坡限内的斜面必须连通，不能被 step_height 单独否掉"
        );
    }

    #[test]
    fn supercover_visits_every_touched_cell() {
        let cells: Vec<[u32; 2]> = supercover([0, 0], [3, 1]).collect();
        assert_eq!(cells.first(), Some(&[0, 0]));
        assert_eq!(cells.last(), Some(&[3, 1]));
        assert!(cells.len() >= 4);
    }

    #[test]
    fn nearest_walkable_finds_closest_cell() {
        let mut g = flat_grid([5, 5]);
        g.set_walkable(3, 3, true);
        let p = Vec3::new(0.5, 0.0, 0.5);
        let got = g.nearest_walkable(p, 10.0, None).unwrap();
        assert_eq!(got, Vec3::new(3.5, 0.0, 3.5));
        assert!(g.nearest_walkable(p, 1.0, None).is_none());
    }
}
