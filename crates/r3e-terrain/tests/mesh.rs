//! M2-C03a：分块网格 + 4 层材质权重的判据（14 条原判据 + H-01B 修复补的 5 条）。
//!
//! 网格是纯数据，所有判据在 headless 下直接算，不需要 GPU 与截图。
//!
//! ## H-01B 补的 5 条（现象②坡面穿透 / 现象④横缝错位）
//!
//! 原 14 条判据全绿仍有缺陷：它们只断言「顶点高度等于采样」「公共顶点逐位相等」，
//! 而缺陷恰恰出在**顶点之间**——顶点等分落位让三角面横跨采样格点的折线，
//! 面中间抬起来的几米没有任何一条判据看得见。补的 5 条全部盯着「面内」与「档位交界」：
//!
//! | 判据 | 抓的是什么 |
//! |---|---|
//! | 15 顶点落在采样格点或块边界上 | 等分落位的根因（非整除夹具 20 / 50） |
//! | 16 三角面与真实地形面贴合 | 面内（重心 + 三边中点）偏差上界，坡面穿透 |
//! | 17 异档接缝在边中间采样也闭合 | L2-vs-L1 的缝，且要求裙边深度盖得住 |
//! | 18 裙边顶点在边界下方且朝块外 | 裙边是否真的挂上了、绕序是否朝外 |
//! | 19 相邻档位边界顶点互相包含 | 细档顶点必须在粗档里出现（L2⊆L1⊆L0） |

use r3e_core::TerrainPatch;
use r3e_terrain::{
    block_mesh, sample_height_at, sample_height_lod, BlockGrid, Lod, TerrainLayerConfig,
    TerrainMesh, LOD_STEPS,
};

const SIZE: f32 = 200.0;
const BLOCK: f32 = 50.0;

/// 200 × 200 米、65 × 65 个高度样本（步长 3.125 米）的起伏地面。
/// 50 / 3.125 = 16 整除，是「旧判据最容易漏检」的那种漂亮夹具。
fn hilly() -> TerrainPatch {
    let n = 65u32;
    let mut heights = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            let x = i as f32 / (n - 1) as f32 * SIZE;
            let z = j as f32 / (n - 1) as f32 * SIZE;
            heights.push((x * 0.02).sin() * 3.0 + (z * 0.03).cos() * 2.0 + x * 0.01);
        }
    }
    TerrainPatch {
        name: "hilly".into(),
        origin: [0.0, 0.0],
        size: [SIZE, SIZE],
        resolution: [n, n],
        heights,
        ..TerrainPatch::default()
    }
}

/// 20 米一折的三角波：每个 20 米处必有一个折点，格内是直线。
fn ridge(t: f32, amp: f32) -> f32 {
    let k = (t / 20.0).floor();
    let f = t - k * 20.0;
    if (k as i64).rem_euclid(2) == 0 {
        amp * (f / 20.0)
    } else {
        amp * (1.0 - f / 20.0)
    }
}

/// **非整除夹具**：格距 20 米、块边长 50 米（50 / 20 = 2.5 不整除）。
///
/// 折点全在 x、z 的 20 米格点上。旧口径按 50 / 3 = 16.67 米等分落点，三角面必然
/// 跨过 x = 20 / 40 的折线，面中间因此高出真实地形数米 —— 坡面穿透的充要夹具。
fn folded() -> TerrainPatch {
    let n = 11u32; // 200 / 10 = 20 米
    let mut heights = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            let x = i as f32 / (n - 1) as f32 * SIZE;
            let z = j as f32 / (n - 1) as f32 * SIZE;
            heights.push(ridge(x, 6.0) + ridge(z, 5.0));
        }
    }
    TerrainPatch {
        name: "folded".into(),
        origin: [0.0, 0.0],
        size: [SIZE, SIZE],
        resolution: [n, n],
        heights,
        ..TerrainPatch::default()
    }
}

fn flat() -> TerrainPatch {
    TerrainPatch::flat("flat", [0.0, 0.0], [SIZE, SIZE])
}

fn grid() -> BlockGrid {
    BlockGrid::new([0.0, 0.0, SIZE, SIZE], BLOCK).unwrap()
}

fn mesh(patches: &[TerrainPatch], index: [u32; 2], lod: Lod) -> TerrainMesh {
    block_mesh(patches, &grid(), index, lod, &TerrainLayerConfig::default()).expect("该块应有网格")
}

fn all_lods() -> [Lod; 3] {
    [Lod::L0, Lod::L1, Lod::L2]
}

/// 本体的顶点（裙边下沿不算「地形面」，高度不是采样值）。
fn body(m: &TerrainMesh) -> &[[f32; 3]] {
    &m.positions[..m.body_vertices]
}

/// 本体顶点在某个轴上的坐标集合（升序去重）。
fn unique_axis(m: &TerrainMesh, axis: usize) -> Vec<f32> {
    let mut v: Vec<f32> = body(m).iter().map(|p| p[axis]).collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    v
}

/// 三角形所在平面在 `(x, z)` 处的高度（点在三角形外时是外插，仅作工具函数用）。
fn plane_y(a: [f32; 3], b: [f32; 3], c: [f32; 3], x: f32, z: f32) -> f32 {
    let (dx1, dz1, dy1) = (b[0] - a[0], b[2] - a[2], b[1] - a[1]);
    let (dx2, dz2, dy2) = (c[0] - a[0], c[2] - a[2], c[1] - a[1]);
    let det = dx1 * dz2 - dx2 * dz1;
    if det.abs() < 1e-12 {
        return a[1];
    }
    let (px, pz) = (x - a[0], z - a[2]);
    let s = (px * dz2 - dx2 * pz) / det;
    let t = (dx1 * pz - dz1 * px) / det;
    a[1] + s * dy1 + t * dy2
}

/// 一条竖直线（`x = 常数`）上的边界顶点折线：`(z, y)` 按 z 升序。
fn boundary_line(m: &TerrainMesh, x: f32) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = body(m)
        .iter()
        .filter(|p| (p[0] - x).abs() < 1e-4)
        .map(|p| (p[2], p[1]))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// 折线在 `z` 处的线性插值高度。
fn y_at(poly: &[(f32, f32)], z: f32) -> Option<f32> {
    for w in poly.windows(2) {
        if z >= w[0].0 - 1e-6 && z <= w[1].0 + 1e-6 {
            let span = w[1].0 - w[0].0;
            let t = if span.abs() > 1e-9 {
                (z - w[0].0) / span
            } else {
                0.0
            };
            return Some(w[0].1 + (w[1].1 - w[0].1) * t);
        }
    }
    None
}

/// 1. 本体顶点数 = (列 + 1) × (行 + 1)；顶点总数 = 本体 + 四条边的裙边下沿。
#[test]
fn 顶点数等于列加一行加一的乘积() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let [cols, rows] = m.segments;
        let (nx, nz) = (cols as usize + 1, rows as usize + 1);
        assert_eq!(m.body_vertices, nx * nz, "{lod:?} 本体顶点数与段数不自洽");
        // 四条边各铺一条裙边带（相邻边共享的角点各算一次，绕序各自成立）
        assert_eq!(
            m.positions.len(),
            nx * nz + 2 * nx + 2 * nz,
            "{lod:?} 顶点总数与裙边带不自洽"
        );
        assert_eq!(m.uvs.len(), m.positions.len());
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.weights.len(), m.positions.len());
        assert_eq!(m.lod, lod);
        assert!(m.skirt_depth > 0.0, "{lod:?} 裙边深度必须为正");
    }
}

/// 2. 本体索引数 = cols × rows × 6（裙边另算），且所有索引都指向合法顶点。
#[test]
fn 索引数等于格数乘六且不越界() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let [cols, rows] = m.segments;
        assert_eq!(m.body_indices, (cols * rows * 6) as usize);
        assert_eq!(m.indices.len(), m.body_indices + m.skirt_triangles() * 3);
        // 四条边共 2 × (cols + rows) 个四边形，每片两个三角
        assert_eq!(m.skirt_triangles(), 4 * ((cols + rows) as usize));
        let limit = m.positions.len() as u32;
        for idx in &m.indices {
            assert!(*idx < limit, "{lod:?} 索引 {idx} 越界（顶点 {limit}）");
        }
    }
}

/// 3. 本体三角数 = 索引数 / 3；L2 的三角数正好是 L0 的 1/16（±4 容差留给尾块取整）。
#[test]
fn 三角数随档位按十六倍衰减() {
    let patches = [hilly()];
    let tri = |lod: Lod| mesh(&patches, [0, 0], lod).body_triangles();
    let (t0, t1, t2) = (tri(Lod::L0), tri(Lod::L1), tri(Lod::L2));
    assert!(t0 > 0 && t1 > 0 && t2 > 0);
    for (coarse, coarse_tri) in [(1u32, t1), (2, t2)] {
        let expected = t0 / (LOD_STEPS[coarse as usize].pow(2) as usize);
        let diff = coarse_tri.abs_diff(expected);
        assert!(
            diff <= 4,
            "LOD 步长 {} 时三角数 {coarse_tri} 与 {expected} 差 {diff}，超过 4",
            LOD_STEPS[coarse as usize]
        );
    }
}

/// 4. 每个顶点的 4 层权重和为 1。
#[test]
fn 每个顶点权重和为归一() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        for (i, w) in m.weights.iter().enumerate() {
            let sum: f32 = w.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-6,
                "{lod:?} 顶点 {i} 权重和 {sum} 不为 1（{w:?}）"
            );
            assert!(w.iter().all(|v| *v >= 0.0 && *v <= 1.0), "权重越界 {w:?}");
        }
    }
}

/// 5. 配置把四层全压掉时退化为层 0 = 1，绝不输出零和权重。
#[test]
fn 四层全失格时退化为纯草层() {
    let cfg = TerrainLayerConfig {
        grass_slope_max: -1.0e6,
        grass_height_max: -1.0e6,
        rock_slope_min: 1.0e6,
        sand_height_max: -1.0e6,
        snow_height_min: 1.0e6,
        fade: 6.0,
    };
    for (h, slope) in [(0.0f32, 0.0f32), (12.0, 40.0), (-30.0, 89.0)] {
        assert_eq!(
            cfg.weights(h, slope),
            [1.0, 0.0, 0.0, 0.0],
            "h={h} slope={slope}"
        );
    }
    // 默认配置下同一批样本不该退化成单层（否则上面的退化断言就没有判别力）
    let normal = TerrainLayerConfig::default();
    let w = normal.weights(0.0, 0.0);
    assert!(w[1] == 0.0 && w[3] == 0.0, "平地不该掺岩/雪：{w:?}");
    assert!(w[0] > 0.0 && w[2] > 0.0, "低地应是草沙混合：{w:?}");
}

/// 6. 法线全部单位长；平地法线恒为 (0, 1, 0)。
#[test]
fn 法线单位长且平地朝上() {
    let patches = [hilly(), flat()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        for (i, n) in m.normals.iter().enumerate() {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "顶点 {i} 法线长度 {len}");
            assert!(n[1] > 0.0, "顶点 {i} 法线朝下：{n:?}");
        }
    }
    let flat_only = [flat()];
    let m = mesh(&flat_only, [0, 0], Lod::L0);
    for n in &m.normals {
        assert!(
            (n[0]).abs() < 1e-6 && (n[1] - 1.0).abs() < 1e-6 && (n[2]).abs() < 1e-6,
            "{n:?}"
        );
    }
}

/// 7. uv 全落在 [0, 1]，且四个角点各自被顶到。
#[test]
fn uv_归一到零一且四角对齐() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        for uv in &m.uvs {
            assert!(
                uv[0] >= 0.0 && uv[0] <= 1.0 && uv[1] >= 0.0 && uv[1] <= 1.0,
                "uv 越界 {uv:?}"
            );
        }
        let has = |u: f32, v: f32| {
            m.uvs
                .iter()
                .any(|uv| (uv[0] - u).abs() < 1e-6 && (uv[1] - v).abs() < 1e-6)
        };
        assert!(has(0.0, 0.0), "{lod:?} 缺 (0,0) 角");
        assert!(has(1.0, 0.0), "{lod:?} 缺 (1,0) 角");
        assert!(has(0.0, 1.0), "{lod:?} 缺 (0,1) 角");
        assert!(has(1.0, 1.0), "{lod:?} 缺 (1,1) 角");
    }
}

/// 8. 索引绕序朝上：任取前 100 个本体三角，叉积的 y 分量与顶点法线同向。
#[test]
fn 三角绕序使面朝上() {
    let patches = [hilly()];
    let m = mesh(&patches, [0, 0], Lod::L0);
    let tri_count = m.body_triangles();
    assert!(tri_count >= 100);
    for t in 0..100 {
        let a = m.positions[m.indices[t * 3] as usize];
        let b = m.positions[m.indices[t * 3 + 1] as usize];
        let c = m.positions[m.indices[t * 3 + 2] as usize];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        assert!(cross[1] > 0.0, "第 {t} 个三角朝下：{cross:?}");
        let n = m.normals[m.indices[t * 3] as usize];
        let dot = cross[0] * n[0] + cross[1] * n[1] + cross[2] * n[2];
        assert!(dot > 0.0, "第 {t} 个三角与顶点法线反向（dot={dot}）");
    }
}

/// 9. 本体顶点高度就是 `sample_height_lod` 的读数，网格不另算一套插值。
#[test]
fn 顶点高度取自同一份采样() {
    let patches = [hilly()];
    let g = grid();
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        for (i, p) in body(&m).iter().enumerate() {
            let expect = sample_height_lod(&patches, &g, p[0], p[2], lod)
                .unwrap_or_else(|| panic!("顶点 {i} 落在覆盖之外"));
            assert!(
                (p[1] - expect).abs() < 1e-5,
                "{lod:?} 顶点 {i} 高度 {} ≠ 采样 {expect}",
                p[1]
            );
        }
    }
}

/// 10. 块没有任何 patch 覆盖 → None，不拿空网格冒充成功。
#[test]
fn 无覆盖的块返回空() {
    let small = TerrainPatch::flat("partial", [0.0, 0.0], [80.0, 80.0]);
    let g = grid();
    let cfg = TerrainLayerConfig::default();
    let only = std::slice::from_ref(&small);
    assert!(block_mesh(only, &g, [3, 3], Lod::L0, &cfg).is_none());
    assert!(block_mesh(only, &g, [0, 0], Lod::L0, &cfg).is_some());
    // 越界块同样给 None，不 panic
    assert!(block_mesh(only, &g, [9, 9], Lod::L0, &cfg).is_none());
}

/// 11. 块边长不整除（40.4 米）也能生成，段数取整规则不 panic。
#[test]
fn 非整除块边长仍能生成网格() {
    let patches = [hilly()];
    // 40.4 米：5 × 5 = 25 块（不超 32 块上限），且 40.4 / 3.125 不整除
    let g = BlockGrid::new([0.0, 0.0, SIZE, SIZE], 40.4).unwrap();
    let cfg = TerrainLayerConfig::default();
    let m = block_mesh(&patches, &g, [0, 0], Lod::L0, &cfg).expect("应能生成");
    let [cols, rows] = m.segments;
    assert!(cols >= 1 && rows >= 1);
    assert_eq!(m.body_vertices, (cols as usize + 1) * (rows as usize + 1));
    assert_eq!(m.body_indices, (cols * rows * 6) as usize);
    // 顶点必须铺满整块（首末顶点贴在块边上）
    let xs = unique_axis(&m, 0);
    assert!(
        xs.first().unwrap().abs() < 1e-4 && (xs.last().unwrap() - 40.4).abs() < 1e-4,
        "{:?}",
        (xs.first(), xs.last())
    );
}

/// 12. 异档相邻块共享边界线上的顶点逐个逐位相等（L2-vs-L0 与 L2-vs-L1 各验一次）。
#[test]
fn 跨块接缝的顶点逐位相等() {
    let patches = [hilly()];
    let check = |left: Lod, right: Lod| {
        let coarse = mesh(&patches, [0, 0], left);
        let fine = mesh(&patches, [1, 0], right);
        let (ca, fa) = (boundary_line(&coarse, BLOCK), boundary_line(&fine, BLOCK));
        assert!(
            ca.len() >= 2 && fa.len() >= 2,
            "{left:?}-vs-{right:?} 边界上顶点太少：{} / {}",
            ca.len(),
            fa.len()
        );
        for (z, y) in &ca {
            let hit = fa
                .iter()
                .find(|(fz, _)| (fz - z).abs() < 1e-4)
                .unwrap_or_else(|| panic!("{left:?}-vs-{right:?} 缺 z={z} 的边界顶点"));
            assert_eq!(
                y.to_bits(),
                hit.1.to_bits(),
                "{left:?}-vs-{right:?} 在 z={z} 处接缝高度不等：{y} vs {}",
                hit.1
            );
        }
    };
    check(Lod::L2, Lod::L0);
    check(Lod::L2, Lod::L1);
    check(Lod::L1, Lod::L0);
}

/// 13. 空地形 → None。
#[test]
fn 空地形不产网格() {
    let g = grid();
    let cfg = TerrainLayerConfig::default();
    assert!(block_mesh(&[], &g, [0, 0], Lod::L0, &cfg).is_none());
}

/// 14. 同输入两次生成，顶点 / 索引 / 权重逐位相同。
#[test]
fn 同输入两次生成逐位相同() {
    let patches = [hilly()];
    let a = mesh(&patches, [1, 1], Lod::L1);
    let b = mesh(&patches, [1, 1], Lod::L1);
    assert_eq!(a.position_bits(), b.position_bits());
    assert_eq!(a.indices, b.indices);
    assert_eq!(a.segments, b.segments);
    assert_eq!(a.lod, b.lod);
    assert_eq!(a.skirt_depth.to_bits(), b.skirt_depth.to_bits());
    for (wa, wb) in a.weights.iter().zip(b.weights.iter()) {
        for k in 0..4 {
            assert_eq!(
                wa[k].to_bits(),
                wb[k].to_bits(),
                "权重漂移 {wa:?} vs {wb:?}"
            );
        }
    }
}

/// 15. 本体顶点只能落在**采样格点**或**块边界**上（非整除夹具 20 / 50）。
///
/// 这条直接盯住现象②的根因：旧口径把 50 米等分成 3 段，顶点落在 16.67 / 33.33，
/// 既不是 20 米格点也不在块边上，三角面于是横跨 x = 20 的折线。
#[test]
fn 顶点落在采样格点或块边界上() {
    let patches = [folded()];
    let cell = 20.0f32;
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let step = LOD_STEPS[match lod {
            Lod::L0 => 0,
            Lod::L1 => 1,
            Lod::L2 => 2,
        }] as f32;
        let coarse = cell * step;
        for (name, axis) in [("x", 0usize), ("z", 2usize)] {
            let vals = unique_axis(&m, axis);
            // 两端必须顶在块边界上（相邻块共享边界线）
            assert!(
                vals.first().unwrap().abs() < 1e-4,
                "{lod:?} {name} 首点不在块边"
            );
            assert!(
                (vals.last().unwrap() - BLOCK).abs() < 1e-4,
                "{lod:?} {name} 末点不在块边"
            );
            for v in &vals {
                let on_edge = v.abs() < 1e-3 || (v - BLOCK).abs() < 1e-3;
                let k = v / cell;
                let on_lattice = (k - k.round()).abs() < 1e-3;
                assert!(
                    on_edge || on_lattice,
                    "{lod:?} {name} = {v} 既不在块边界、也不在 20 米采样格点上"
                );
            }
            // 相邻顶点间距不超过一个粗档跨度（对齐后不可能出现「跨两格」的大面）
            for w in vals.windows(2) {
                assert!(
                    w[1] - w[0] <= coarse + 1e-3,
                    "{lod:?} {name} 相邻顶点间距 {} 超过粗档 {coarse}",
                    w[1] - w[0]
                );
            }
        }
        // L0 是最贴人的一档：一格一面，面内不可能出现折点
        if lod == Lod::L0 {
            let vals = unique_axis(&m, 0);
            for w in vals.windows(2) {
                assert!(
                    w[1] - w[0] <= cell + 1e-3,
                    "L0 相邻顶点间距 {} 超过一格 {cell}",
                    w[1] - w[0]
                );
            }
        }
    }
}

/// 16. 三角面与真实地形面贴合（面内采样，不是只看顶点）。
///
/// 旧判据只比顶点高度，而穿透发生在**面中间**。这里对每个本体三角取重心与三条边的
/// 中点，用三角形所在平面求高，与 `sample_height_at`（与物理高度场同一函数）比。
/// 非整除夹具上每个格内是平面，对齐后偏差应只剩浮点噪声。
#[test]
fn 三角面与真实地形面贴合() {
    let patches = [folded()];
    for index in [[0u32, 0u32], [1, 0], [0, 1], [3, 3]] {
        let m = mesh(&patches, index, Lod::L0);
        let mut worst = 0.0f32;
        for t in 0..m.body_triangles() {
            let a = m.positions[m.indices[t * 3] as usize];
            let b = m.positions[m.indices[t * 3 + 1] as usize];
            let c = m.positions[m.indices[t * 3 + 2] as usize];
            let probes = [
                ((a[0] + b[0] + c[0]) / 3.0, (a[2] + b[2] + c[2]) / 3.0),
                ((a[0] + b[0]) / 2.0, (a[2] + b[2]) / 2.0),
                ((b[0] + c[0]) / 2.0, (b[2] + c[2]) / 2.0),
                ((c[0] + a[0]) / 2.0, (c[2] + a[2]) / 2.0),
            ];
            for (x, z) in probes {
                let truth = sample_height_at(&patches, x, z)
                    .unwrap_or_else(|| panic!("块 {index:?} 探针 ({x}, {z}) 无覆盖"));
                let got = plane_y(a, b, c, x, z);
                worst = worst.max((got - truth).abs());
            }
        }
        assert!(
            worst <= 0.01,
            "块 {index:?} 三角面偏离真实地形 {worst} 米（> 1 厘米即坡面穿透）"
        );
    }
}

/// 17. 异档接缝在**边中间**也闭合（L2 块 vs L1 块），且裙边深度盖得住残余精度差。
///
/// 顶点对齐只保证「粗档有的顶点，细档都有」，两个粗顶点之间细档仍可能高出几个厘米。
/// 判据：沿边界线密采样 401 个点比对两侧折线高度，落差必须落在裙边深度以内
/// （否则会从缝里看见清屏色 = 现象④）。
#[test]
fn 异档接缝在边中间采样也闭合() {
    for patches in [[hilly()], [folded()]] {
        let coarse = mesh(&patches, [0, 0], Lod::L2);
        let fine = mesh(&patches, [1, 0], Lod::L1);
        let (left, right) = (boundary_line(&coarse, BLOCK), boundary_line(&fine, BLOCK));
        assert!(left.len() >= 2 && right.len() >= 2);
        let mut worst = 0.0f32;
        let mut worst_z = 0.0f32;
        for k in 0..=400 {
            let z = BLOCK * k as f32 / 400.0;
            let a = y_at(&left, z).expect("左侧边界折线应覆盖整条边");
            let b = y_at(&right, z).expect("右侧边界折线应覆盖整条边");
            if (a - b).abs() > worst {
                worst = (a - b).abs();
                worst_z = z;
            }
        }
        // 凸性/折面夹具上两侧顶点完全一致，缝应为 0；平滑夹具留 0.35 米给插值差。
        assert!(
            worst <= 0.35,
            "z={worst_z} 处接缝落差 {worst} 米，超过插值精度差的合理上界"
        );
        // 裙边必须比这条缝更深，否则缝隙会漏光
        let curtain = coarse.skirt_depth.min(fine.skirt_depth);
        assert!(
            curtain >= worst,
            "裙边 {curtain} 米盖不住 {worst} 米的接缝落差"
        );
        assert!(curtain > 0.0);
    }
}

/// 18. 裙边：下沿严格在边界顶点正下方一个深度，且每个裙边三角都竖立、朝向块外。
#[test]
fn 裙边顶点在边界下方且朝块外() {
    let patches = [hilly()];
    let (cx, cz) = (BLOCK / 2.0, BLOCK / 2.0);
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let mut top = std::collections::HashMap::new();
        for p in body(&m) {
            top.insert((p[0].to_bits(), p[2].to_bits()), p[1]);
        }
        let depth = m.skirt_depth;
        assert!(depth > 0.0 && depth <= 8.0, "{lod:?} 裙边深度 {depth} 越界");
        for p in &m.positions[m.body_vertices..] {
            let upper = *top
                .get(&(p[0].to_bits(), p[2].to_bits()))
                .unwrap_or_else(|| panic!("{lod:?} 裙边下沿 ({}, {}) 不是边界顶点", p[0], p[2]));
            assert!(
                (p[1] - (upper - depth)).abs() < 1e-4,
                "{lod:?} 裙边下沿高度 {} 与上沿 {upper} 差不是深度 {depth}",
                p[1]
            );
        }
        for t in (m.body_indices / 3)..(m.indices.len() / 3) {
            let a = m.positions[m.indices[t * 3] as usize];
            let b = m.positions[m.indices[t * 3 + 1] as usize];
            let c = m.positions[m.indices[t * 3 + 2] as usize];
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let scale = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt()
                * (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!(
                cross[1].abs() <= 1e-4 * scale.max(1e-12),
                "{lod:?} 第 {t} 个裙边三角不竖立：{cross:?}"
            );
            let (mx, mz) = (
                (a[0] + b[0] + c[0]) / 3.0 - cx,
                (a[2] + b[2] + c[2]) / 3.0 - cz,
            );
            assert!(
                cross[0] * mx + cross[2] * mz > 0.0,
                "{lod:?} 第 {t} 个裙边三角法线朝块内：{cross:?} 中心偏移 ({mx}, {mz})"
            );
        }
    }
}

/// 19. 相邻档位的边界顶点互相包含：L1 ⊆ L0、L2 ⊆ L1（非整除夹具同样成立）。
#[test]
fn 相邻档位边界顶点互相包含() {
    for patches in [[hilly()], [folded()]] {
        let m0 = mesh(&patches, [0, 0], Lod::L0);
        let m1 = mesh(&patches, [0, 0], Lod::L1);
        let m2 = mesh(&patches, [0, 0], Lod::L2);
        for (name, fine, coarse) in [
            ("L1⊆L0", unique_axis(&m0, 0), unique_axis(&m1, 0)),
            ("L2⊆L1", unique_axis(&m1, 0), unique_axis(&m2, 0)),
        ] {
            for v in &coarse {
                assert!(
                    fine.iter().any(|f| (f - v).abs() < 1e-4),
                    "{name}：粗档顶点 {v} 在细档里不存在（接缝必然错位）"
                );
            }
        }
    }
}

/// 测试辅助：把顶点坐标拍平成位模式，便于逐位比较。
trait PositionBits {
    fn position_bits(&self) -> Vec<u32>;
}

impl PositionBits for TerrainMesh {
    fn position_bits(&self) -> Vec<u32> {
        self.positions
            .iter()
            .flat_map(|p| p.iter().map(|v| v.to_bits()))
            .collect()
    }
}
