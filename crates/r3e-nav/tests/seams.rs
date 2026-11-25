//! D01 遗留收口：**12 块拼接接缝**（原计划里「块边界 4 条」那一组）。
//!
//! ## 这条夹具要锁死什么
//!
//! 地形是**一块一块**拼出来的：每块各有一份自己的高度样本网格，块与块之间只在
//! 世界坐标上紧贴，`TerrainPatch::sample_height` 逐块插值。于是接缝处天然存在一个
//! 风险：**两块在接缝线上的高度对不上**（作者手工摆位、样本数不同、边缘样本写错），
//! 烘焙后接缝就会变成一道「假台阶」甚至「假悬空」，表现为整条接缝的格子被判成
//! `Blocked`（走不过去）或 `Step`（明明是同坡面却像路沿）。
//!
//! 这里用 **12 块（4 列 × 3 行）拼成一块连续地形**，三种接缝各造一遍：
//!
//! | 场景 | 接缝两侧高差 | 期望 |
//! |---|---|---|
//! | 连续平地 | 0 | 接缝两侧都 `Flat`，`Blocked = 0`，整个场地 1 个连通块 |
//! | 连续缓坡 | 与块内逐格步进相同（0.125 m） | 接缝两侧都 `Slope`，**不得**退化成 `Step` |
//! | 台阶（每块抬 0.3 m） | 0.3 m（超坡度上限、在步高内） | 低侧 `Step`、高侧 `Flat`，**双向可跨** |
//! | 超高台阶（每块抬 1.5 m） | 1.5 m（步高也不够） | 两侧仍 `Flat`（站得住），只是跨不过去，4 个连通块 |
//!
//! ## 为什么断言写成「逐格复算」而不是「钉格数」
//!
//! `classify` 的判据是「最陡的那条正邻边」（只看 +X / +Z，见 `grid.rs` 注释），
//! 复算一遍就是拿**同一份 `step_verdict` 口径**（坡度上限 `dist * tan(max_slope)`、
//! 步高、平坦容差 1e-3、比较余量 1e-5）把每条边重新判一次。容差一旦被人改动，
//! 逐格复算立刻红——这正是「分类与迈步判定同源」要盯的东西。
//!
//! ## 边界
//!
//! * **不动 `grid.rs`**（688 行，距 800 行债务线只剩 112 行），本夹具全在 `tests/`；
//! * 只用公开面：`Nav::bake_with` / `NavGrid::{dims, cell_center, ground, class_of,
//!   is_walkable_cell, can_step, region_at, region_count}` / `CellClass`；
//! * 确定性：同一份 `Scene` 烘两次，`product_hash` 必须逐位相同。

use r3e_core::{Scene, TerrainPatch};
use r3e_nav::{CellClass, Nav, NavConfig, NavGrid};

// ---------------------------------------------------------------------------
// 布局常量：12 块 = 4 列 × 3 行，每块 4 m × 4 m，格长 0.5 m（= 每块 8 格）
// ---------------------------------------------------------------------------

/// 单块边长（米）。
const BLOCK: f32 = 4.0;
/// 块列数。
const BX: u32 = 4;
/// 块行数。
const BY: u32 = 3;
/// 格长（米）。取默认值，不为了好算而改。
const CELL: f32 = 0.5;
/// 每块的格数（单轴）。
const PER: u32 = (BLOCK / CELL) as u32;
/// 每块单轴样本数：0.5 m 一个样本，含两端。
const RES: u32 = PER + 1;

/// 坡度口径里的比较余量（与 `grid.rs::step_verdict` 同源）。
const VERDICT_EPS: f32 = 1e-5;
/// 平地容差（与 `grid.rs` 的 `FLAT_EPS` 同源）。
const FLAT_EPS: f32 = 1e-3;

// ---------------------------------------------------------------------------
// 场景构造：12 块紧贴拼接
// ---------------------------------------------------------------------------

/// 按「块局部坐标」给出高度：`(块列, 块行, 块内 x, 块内 z)`。
///
/// 用块索引而不是全局坐标取整，是为了让「整块常数」类地形（台阶）在块边界上
/// 不留第二种解释：每块自己的样本只按自己的索引算。
type HeightFn = fn(u32, u32, f32, f32) -> f32;

/// 连续平地：全场高度 0。
fn flat_all(_c: u32, _r: u32, _lx: f32, _lz: f32) -> f32 {
    0.0
}

/// 连续缓坡：沿 +X 每米升 0.25 m（14°，45° 上限内），**跨块连续**（全局线性）。
fn ramp_x(c: u32, _r: u32, lx: f32, _lz: f32) -> f32 {
    0.25 * (c as f32 * BLOCK + lx)
}

/// 台阶：整块抬 0.3 m，接缝处是一级矮坎（超坡度上限、在步高内 —— 需配 `step_height = 0.5`）。
fn step_x(c: u32, _r: u32, _lx: f32, _lz: f32) -> f32 {
    0.3 * c as f32
}

/// 超高台阶：整块抬 1.5 m，接缝处跨不过去（但两侧都站得住）。
fn cliff_x(c: u32, _r: u32, _lx: f32, _lz: f32) -> f32 {
    1.5 * c as f32
}

/// 12 块拼接场景。块之间**零间隙**（后一块的左边界 = 前一块的右边界）。
fn twelve_patch_scene(h: HeightFn) -> Scene {
    let mut scene = Scene::new();
    for r in 0..BY {
        for c in 0..BX {
            let origin = [c as f32 * BLOCK, r as f32 * BLOCK];
            let mut patch = TerrainPatch::flat(format!("blk_{c}_{r}"), origin, [BLOCK, BLOCK]);
            patch.resolution = [RES, RES];
            let step = BLOCK / (RES - 1) as f32;
            patch.heights = (0..(RES * RES) as usize)
                .map(|idx| {
                    let i = (idx % RES as usize) as f32;
                    let j = (idx / RES as usize) as f32;
                    h(c, r, i * step, j * step)
                })
                .collect();
            scene.terrain.push(patch);
        }
    }
    scene.update_world_transforms();
    scene
}

/// 接缝两侧都站得住的默认参数：格长 0.5 m、45° 坡度上限、0.4 m 步高。
fn flat_cfg() -> NavConfig {
    NavConfig {
        cell_size: CELL,
        ..Default::default()
    }
}

/// 矮坎（0.3 m）要落在「超坡度、在步高内」的窗口里：把坡度上限压到 10°、步高提到 0.5 m。
///
/// 默认参数下 0.5 m 格长的坡度上限正好是 0.5 m，比 0.4 m 的步高还宽，
/// `Step` 分支永远轮不到——所以台阶场景必须显式给一组参数，这不是为了过测试而放宽，
/// 而是「矮坎」这个地形只有在坡度上限窄于步高时才与「缓坡」有区别。
fn step_cfg() -> NavConfig {
    NavConfig {
        cell_size: CELL,
        step_height: 0.5,
        max_slope_deg: 10.0,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// 接缝枚举与判据复算
// ---------------------------------------------------------------------------

/// 一条跨块接缝邻边：`a` / `b` 是相邻两格，`axis` 为 0（沿 +X）或 1（沿 +Z）。
#[derive(Debug, Clone, Copy)]
struct Seam {
    a: [u32; 2],
    b: [u32; 2],
    axis: u8,
}

/// 格中心落在第几块（列, 行）。块外（网格外扩的那一圈）会给负数或越界值。
fn block_of(grid: &NavGrid, cell: [u32; 2]) -> (i32, i32) {
    let c = grid.cell_center(cell[0], cell[1]);
    ((c.x / BLOCK).floor() as i32, (c.z / BLOCK).floor() as i32)
}

/// 枚举**两块地形之间**的接缝邻边。
///
/// 只认「两侧格中心落在相邻的两块、且两块都在 4×3 布局内」的边：
/// 地形最外沿那一圈（`auto_bounds` 外扩出来的 padding 格）不算接缝，
/// 否则「接缝」会混进「地形之外」的去重统计。
fn seam_edges(grid: &NavGrid) -> Vec<Seam> {
    let [cols, rows] = grid.dims();
    let inside = |b: (i32, i32)| b.0 >= 0 && b.1 >= 0 && b.0 < BX as i32 && b.1 < BY as i32;
    let adjacent = |p: (i32, i32), q: (i32, i32)| {
        inside(p) && inside(q) && ((p.0 - q.0).abs() + (p.1 - q.1).abs()) == 1
    };
    let mut out = Vec::new();
    for j in 0..rows {
        for i in 0..cols {
            let here = block_of(grid, [i, j]);
            if i + 1 < cols && adjacent(here, block_of(grid, [i + 1, j])) {
                out.push(Seam {
                    a: [i, j],
                    b: [i + 1, j],
                    axis: 0,
                });
            }
            if j + 1 < rows && adjacent(here, block_of(grid, [i, j + 1])) {
                out.push(Seam {
                    a: [i, j],
                    b: [i, j + 1],
                    axis: 1,
                });
            }
        }
    }
    out
}

/// 复算一次迈步判定（与 `grid.rs::step_verdict` 同一个口径，代码各自独立）。
fn verdict(cfg: &NavConfig, dh: f32, diagonal: bool) -> (bool, Option<CellClass>) {
    let dist = if diagonal {
        cfg.cell_size * std::f32::consts::SQRT_2
    } else {
        cfg.cell_size
    };
    let slope_limit = dist * cfg.max_slope_deg.to_radians().tan();
    if dh <= slope_limit + VERDICT_EPS {
        let class = if dh <= FLAT_EPS {
            CellClass::Flat
        } else {
            CellClass::Slope
        };
        (true, Some(class))
    } else if dh <= cfg.step_height + VERDICT_EPS {
        (true, Some(CellClass::Step))
    } else {
        (false, None)
    }
}

/// 按「最陡的正邻边」复算一格应有的分类，用于逐格对账。
fn expected_class(grid: &NavGrid, cfg: &NavConfig, i: u32, j: u32) -> CellClass {
    if !grid.is_walkable_cell(i, j) {
        return CellClass::Blocked;
    }
    let [cols, rows] = grid.dims();
    let mut worst = CellClass::Flat;
    let mut consider = |next: [u32; 2]| {
        let dh = (grid.ground(i, j) - grid.ground(next[0], next[1])).abs();
        if let (_, Some(class)) = verdict(cfg, dh, false) {
            if class.severity() > worst.severity() {
                worst = class;
            }
        }
    };
    if i + 1 < cols {
        consider([i + 1, j]);
    }
    if j + 1 < rows {
        consider([i, j + 1]);
    }
    worst
}

/// 接缝统计：跨接缝边数、被误判成 `Blocked` 的接缝格（去重）、`can_step` 双向皆真的边数。
struct SeamStats {
    edges: usize,
    blocked_cells: Vec<[u32; 2]>,
    crossable: usize,
}

/// 只要「两侧格的正邻边都还在地形里」的接缝边。
///
/// `classify` 只看 +X / +Z 两条正邻边，且**不看邻格是否可走**——地形最外沿那一格
/// 的 +X / +Z 邻落在地形外的空白区（高度 0），于是平坦或不高的地形边缘会被记成
/// `Step`。那是「地形边界 vs 虚空」的断面，**不是块拼接**，本夹具要盯的是块与块
/// 之间的接缝，所以逐格 `class` 的强断言只在内部接缝段上做（外沿段仍照常断言
/// `Blocked = 0` 与可跨性）。这条边界效应已作为观察项记在收口说明里，未改判据。
fn is_inner_seam(grid: &NavGrid, seam: &Seam) -> bool {
    let [cols, rows] = grid.dims();
    let inside = |c: [u32; 2]| {
        c[0] + 1 < cols
            && c[1] + 1 < rows
            && grid.is_walkable_cell(c[0] + 1, c[1])
            && grid.is_walkable_cell(c[0], c[1] + 1)
    };
    inside(seam.a) && inside(seam.b)
}

fn seam_stats(nav: &Nav) -> (Vec<Seam>, SeamStats) {
    let grid = nav.grid();
    let seams = seam_edges(grid);
    let mut blocked_cells: Vec<[u32; 2]> = Vec::new();
    let mut crossable = 0usize;
    for seam in &seams {
        for cell in [seam.a, seam.b] {
            if grid.class_of(cell[0], cell[1]) == Some(CellClass::Blocked)
                && !blocked_cells.contains(&cell)
            {
                blocked_cells.push(cell);
            }
        }
        if grid.can_step(seam.a, seam.b) && grid.can_step(seam.b, seam.a) {
            crossable += 1;
        }
    }
    let edges = seams.len();
    (
        seams,
        SeamStats {
            edges,
            blocked_cells,
            crossable,
        },
    )
}

// ---------------------------------------------------------------------------
// 一、连续平地：接缝不得有 Blocked
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_flat_seams_have_no_blocked_cell() {
    let scene = twelve_patch_scene(flat_all);
    let nav = Nav::bake_with(&scene, flat_cfg()).expect("12 块平地必须能烘焙");
    let grid = nav.grid();
    let (seams, stats) = seam_stats(&nav);

    // 可核对的计数：每块 8 格 → 地形覆盖 32 列 × 24 行（16 m × 12 m）；
    // 竖接缝 (4-1) 条 × 24 行 + 横接缝 (3-1) 条 × 32 列 = 72 + 64 = 136 条跨块邻边。
    // 栅格外扩出来的 padding 圈（`auto_bounds` 的 pad）不计入，因为它不属于任何块。
    let covered_cols = (BX * PER) as usize;
    let covered_rows = (BY * PER) as usize;
    let expected = (BX as usize - 1) * covered_rows + (BY as usize - 1) * covered_cols;
    assert_eq!(
        stats.edges, expected,
        "跨块接缝边数必须是 (竖接缝 × 覆盖行) + (横接缝 × 覆盖列) = {expected}，实得 {}",
        stats.edges
    );
    assert_eq!(
        expected, 136,
        "12 块 4×3 布局（每块 8 格）的接缝边数应为 136 条（动态算得 {expected}）"
    );

    assert!(
        stats.blocked_cells.is_empty(),
        "平地接缝不得有 Blocked 格，实得 {} 格：{:?}",
        stats.blocked_cells.len(),
        stats.blocked_cells
    );
    assert_eq!(stats.crossable, seams.len(), "平地接缝的每条边都应双向可跨");
    assert_eq!(
        grid.class_histogram()[CellClass::Flat.code() as usize],
        grid.walkable_count(),
        "平地场景的可走格必须全是 Flat"
    );
    assert_eq!(nav.report().regions, 1, "平地接缝不得把场地切碎");
}

// ---------------------------------------------------------------------------
// 二、连续缓坡：接缝与块内同性质，不得退化成台阶
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_gentle_ramp_seams_classify_slope() {
    let scene = twelve_patch_scene(ramp_x);
    let nav = Nav::bake_with(&scene, flat_cfg()).expect("12 块缓坡必须能烘焙");
    let grid = nav.grid();
    let cfg = flat_cfg();
    let (seams, stats) = seam_stats(&nav);

    assert!(stats.edges > 0, "缓坡场景同样要枚举到接缝");
    assert!(
        stats.blocked_cells.is_empty(),
        "缓坡接缝不得有 Blocked 格：{:?}",
        stats.blocked_cells
    );
    assert_eq!(stats.crossable, seams.len(), "缓坡接缝每条边都应双向可跨");

    let step = 0.25 * CELL; // 每格沿 +X 升 0.125 m（14°）
    for seam in &seams {
        let (a, b) = (seam.a, seam.b);
        let dh = (grid.ground(a[0], a[1]) - grid.ground(b[0], b[1])).abs();
        if seam.axis == 0 {
            // 沿坡向的接缝：两侧高差必须与块内逐格步进一致（连续斜面，不是坎）。
            assert!(
                (dh - step).abs() <= 1e-4,
                "沿坡接缝 {a:?}->{b:?} 的高差 {dh} 应等于块内步进 {step}，否则接缝处多了一道假台阶"
            );
        } else {
            // 垂直坡向的接缝：同坡等高，高差必须为 0。
            assert!(
                dh <= 1e-4,
                "横坡接缝 {a:?}->{b:?} 高差 {dh} 应为 0（等高处拼接）"
            );
        }
        // 两侧分类：缓坡里任何接缝格都不许退化成 Step 或 Blocked；
        // 具体值按「最陡正邻边」的坡度/步高口径逐格复算，不许凭空钉。
        for cell in [a, b] {
            let got = grid.class_of(cell[0], cell[1]).expect("每格都有分类");
            assert_ne!(
                got,
                CellClass::Blocked,
                "缓坡接缝格 {cell:?} 不得被判 Blocked"
            );
            assert_ne!(
                got,
                CellClass::Step,
                "缓坡接缝格 {cell:?} 被错判成台阶（块内步进 {step} 与接缝高差同为坡度范围内）"
            );
            assert_eq!(
                got,
                expected_class(grid, &cfg, cell[0], cell[1]),
                "缓坡接缝格 {cell:?} 的分类与坡度/步高复算不一致"
            );
        }
        // 接缝边的判定与复算口径一致：沿坡接缝是 Slope，横坡接缝同高是 Flat。
        let (ok, class) = verdict(&cfg, dh, false);
        assert_eq!(
            ok,
            grid.can_step(a, b),
            "接缝 {a:?}->{b:?} 的可跨性必须与坡度/步高口径一致"
        );
        let want = if seam.axis == 0 {
            CellClass::Slope
        } else {
            CellClass::Flat
        };
        assert_eq!(
            class,
            Some(want),
            "接缝 {a:?}->{b:?} 的高差 {dh} 应判为 {want:?} 而非 Step"
        );
    }
    assert_eq!(nav.report().regions, 1, "连续缓坡不该把场地切成多块");
}

// ---------------------------------------------------------------------------
// 三、台阶：接缝是矮坎，低侧 Step / 高侧 Flat，双向可跨
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_step_seams_classify_step_and_stay_crossable() {
    let scene = twelve_patch_scene(step_x);
    let cfg = step_cfg();
    let nav = Nav::bake_with(&scene, cfg.clone()).expect("12 块台阶必须能烘焙");
    let grid = nav.grid();
    let (seams, stats) = seam_stats(&nav);

    let axis0: Vec<&Seam> = seams.iter().filter(|s| s.axis == 0).collect();
    assert!(!axis0.is_empty(), "台阶场景必须有沿 +X 的接缝边");
    assert!(
        stats.blocked_cells.is_empty(),
        "台阶接缝不得有 Blocked 格（站得住，只是有坎）：{:?}",
        stats.blocked_cells
    );
    assert_eq!(
        stats.crossable,
        seams.len(),
        "0.3 m 的坎在 0.5 m 步高下，每条接缝边都必须双向可跨"
    );

    let mut step_sides = 0usize;
    let mut skipped_rim = 0usize;
    for seam in &axis0 {
        let (a, b) = (seam.a, seam.b);
        let dh = (grid.ground(a[0], a[1]) - grid.ground(b[0], b[1])).abs();
        assert!(
            (dh - 0.3).abs() <= 1e-4,
            "台阶接缝 {a:?}->{b:?} 的高差应恰好是 0.3 m，实得 {dh}"
        );
        let (ok, class) = verdict(&cfg, dh, false);
        assert!(ok, "0.3 m 必须能跨");
        assert_eq!(
            class,
            Some(CellClass::Step),
            "0.3 m 在 10° 坡度上限下必须判 Step"
        );

        if !is_inner_seam(grid, seam) {
            skipped_rim += 1;
            continue;
        }
        // 沿 +X 方向：低侧（更矮的一端）classification 记 Step，高侧块内是平的 → Flat。
        let (low, high) = if grid.ground(a[0], a[1]) < grid.ground(b[0], b[1]) {
            (a, b)
        } else {
            (b, a)
        };
        assert_eq!(
            grid.class_of(low[0], low[1]),
            Some(CellClass::Step),
            "坎的低侧 {low:?} 必须记 Step（它的 +X 邻边就是这道坎）"
        );
        assert_eq!(
            grid.class_of(high[0], high[1]),
            Some(CellClass::Flat),
            "坎的高侧 {high:?} 块内是平的，必须记 Flat"
        );
        step_sides += 1;
    }
    // 逐条核对的可核对计数：3 条竖接缝 × (24 行覆盖 - 最外 1 行) = 69 条内侧边。
    assert_eq!(
        step_sides, 69,
        "沿 +X 的内侧接缝边应逐条核对过（3 × 23 = 69），实得 {step_sides}"
    );
    assert_eq!(
        skipped_rim,
        axis0.len() - 69,
        "只允许地形最外沿那一行落在内侧断言之外"
    );
    assert_eq!(
        nav.report().regions,
        1,
        "能跨的台阶不该把场地切成多个连通块"
    );
}

// ---------------------------------------------------------------------------
// 四、超高台阶：站得住但跨不过去（口径：TooSteep 不进 Blocked）
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_tall_seams_stay_walkable_but_split_regions() {
    let scene = twelve_patch_scene(cliff_x);
    let nav = Nav::bake_with(&scene, flat_cfg()).expect("12 块断崖必须能烘焙");
    let grid = nav.grid();
    let (seams, stats) = seam_stats(&nav);

    assert!(
        stats.blocked_cells.is_empty(),
        "1.5 m 的断崖不该把接缝格标成 Blocked（D01 口径：站得住就不是 Blocked）：{:?}",
        stats.blocked_cells
    );
    let mut axis0 = 0usize;
    for seam in &seams {
        let (a, b) = (seam.a, seam.b);
        let dh = (grid.ground(a[0], a[1]) - grid.ground(b[0], b[1])).abs();
        if seam.axis == 0 {
            axis0 += 1;
            assert!(
                !grid.can_step(a, b) && !grid.can_step(b, a),
                "1.5 m 的断崖必须双向都跨不过去：{a:?}->{b:?}"
            );
            assert_eq!(
                verdict(&flat_cfg(), dh, false),
                (false, None),
                "1.5 m 必须判 TooSteep"
            );
        } else {
            assert!(dh <= 1e-4, "同一块列内等高，横接缝 {a:?}->{b:?} 高差应为 0");
        }
        for cell in [a, b] {
            assert!(
                grid.class_of(cell[0], cell[1])
                    .is_some_and(|c| c.is_standable()),
                "断崖两侧的格仍必须站得住：{cell:?}"
            );
        }
    }
    assert!(axis0 > 0, "断崖场景必须有沿 +X 的接缝边");
    assert_eq!(
        nav.report().regions,
        BX,
        "1.5 m 的断崖按列切开，正好 {BX} 个连通块（行方向仍连通）"
    );
}

// ---------------------------------------------------------------------------
// 五、逐格对账：分类必须与坡度/步高口径逐格一致（三场景全跑）
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_seam_class_matches_step_verdict_on_every_cell() {
    let cases: [(&str, HeightFn, NavConfig); 4] = [
        ("连续平地", flat_all, flat_cfg()),
        ("连续缓坡", ramp_x, flat_cfg()),
        ("台阶", step_x, step_cfg()),
        ("超高台阶", cliff_x, flat_cfg()),
    ];
    for (name, h, cfg) in cases {
        let scene = twelve_patch_scene(h);
        let nav = Nav::bake_with(&scene, cfg.clone()).expect("12 块场景必须能烘焙");
        let grid = nav.grid();
        let [cols, rows] = grid.dims();
        let mut checked = 0usize;
        for j in 0..rows {
            for i in 0..cols {
                let got = grid.class_of(i, j).expect("每格都有分类");
                let want = expected_class(grid, &cfg, i, j);
                assert_eq!(
                    got, want,
                    "{name}：格 ({i}, {j}) 的分类与坡度/步高口径不一致（got={got:?}, want={want:?}）"
                );
                checked += 1;
            }
        }
        assert_eq!(
            checked,
            (cols * rows) as usize,
            "{name}：必须逐格核对完整张网格，不许抽样"
        );
    }
}

// ---------------------------------------------------------------------------
// 六、确定性：同一份 12 块场景烘两次，产物逐位相同
// ---------------------------------------------------------------------------

#[test]
fn twelve_patch_seam_bake_is_deterministic() {
    let cases: [(&str, HeightFn, NavConfig); 3] = [
        ("连续平地", flat_all, flat_cfg()),
        ("连续缓坡", ramp_x, flat_cfg()),
        ("台阶", step_x, step_cfg()),
    ];
    for (name, h, cfg) in cases {
        let scene = twelve_patch_scene(h);
        let first = Nav::bake_with(&scene, cfg.clone()).expect("第一次烘焙");
        let second = Nav::bake_with(&scene, cfg).expect("第二次烘焙");
        assert_eq!(
            first.product_hash(),
            second.product_hash(),
            "{name}：同一份 12 块场景两次烘焙产物必须逐位相同"
        );
        let (seams_a, stats_a) = seam_stats(&first);
        let (seams_b, stats_b) = seam_stats(&second);
        assert_eq!(seams_a.len(), seams_b.len(), "{name}：接缝边数两次必须一致");
        assert_eq!(
            stats_a.crossable, stats_b.crossable,
            "{name}：可跨接缝边数两次必须一致"
        );
        assert_eq!(
            stats_a.blocked_cells.len(),
            stats_b.blocked_cells.len(),
            "{name}：接缝 Blocked 格数两次必须一致"
        );
    }
}
