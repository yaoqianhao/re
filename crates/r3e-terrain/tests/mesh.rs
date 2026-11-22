//! M2-C03a：分块网格 + 4 层材质权重的 14 条判据。
//!
//! 网格是纯数据，所有判据在 headless 下直接算，不需要 GPU 与截图。

use r3e_core::TerrainPatch;
use r3e_terrain::{
    block_mesh, sample_height_lod, BlockGrid, Lod, TerrainLayerConfig, TerrainMesh, LOD_STEPS,
};

const SIZE: f32 = 200.0;
const BLOCK: f32 = 50.0;

/// 200 × 200 米、65 × 65 个高度样本（步长 3.125 米）的起伏地面。
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

/// 1. 顶点数 = (列 + 1) × (行 + 1)，三级各验一次。
#[test]
fn 顶点数等于列加一行加一的乘积() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let [cols, rows] = m.segments;
        assert_eq!(
            m.positions.len(),
            (cols as usize + 1) * (rows as usize + 1),
            "{lod:?} 顶点数与段数不自洽"
        );
        assert_eq!(m.uvs.len(), m.positions.len());
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.weights.len(), m.positions.len());
        assert_eq!(m.lod, lod);
    }
}

/// 2. 索引数 = cols × rows × 6，且所有索引都指向合法顶点。
#[test]
fn 索引数等于格数乘六且不越界() {
    let patches = [hilly()];
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        let [cols, rows] = m.segments;
        assert_eq!(m.indices.len(), (cols * rows * 6) as usize);
        let limit = m.positions.len() as u32;
        for idx in &m.indices {
            assert!(*idx < limit, "{lod:?} 索引 {idx} 越界（顶点 {limit}）");
        }
    }
}

/// 3. 三角数 = 索引数 / 3；L2 的三角数正好是 L0 的 1/16（±4 容差留给尾块取整）。
#[test]
fn 三角数随档位按十六倍衰减() {
    let patches = [hilly()];
    let tri = |lod: Lod| mesh(&patches, [0, 0], lod).indices.len() / 3;
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

/// 8. 索引绕序朝上：任取 100 个三角，叉积的 y 分量与顶点法线同向。
#[test]
fn 三角绕序使面朝上() {
    let patches = [hilly()];
    let m = mesh(&patches, [0, 0], Lod::L0);
    let tri_count = m.indices.len() / 3;
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

/// 9. 顶点高度就是 `sample_height_lod` 的读数，网格不另算一套插值。
#[test]
fn 顶点高度取自同一份采样() {
    let patches = [hilly()];
    let g = grid();
    for lod in all_lods() {
        let m = mesh(&patches, [0, 0], lod);
        for (i, p) in m.positions.iter().enumerate() {
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

/// 11. 块边长不整除（33.3 米）也能生成，段数取整规则不 panic。
#[test]
fn 非整除块边长仍能生成网格() {
    let patches = [hilly()];
    // 40.4 米：5 × 5 = 25 块（不超 32 块上限），且 40.4 / 3.125 不整除
    let g = BlockGrid::new([0.0, 0.0, SIZE, SIZE], 40.4).unwrap();
    let cfg = TerrainLayerConfig::default();
    let m = block_mesh(&patches, &g, [0, 0], Lod::L0, &cfg).expect("应能生成");
    let [cols, rows] = m.segments;
    assert!(cols >= 1 && rows >= 1);
    assert_eq!(m.positions.len(), (cols as usize + 1) * (rows as usize + 1));
    assert_eq!(m.indices.len(), (cols * rows * 6) as usize);
    // 顶点必须铺满整块（首末顶点贴在块边上）
    let xs: Vec<f32> = m.positions.iter().map(|p| p[0]).collect();
    let min = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        (min - 0.0).abs() < 1e-4 && (max - 40.4).abs() < 1e-4,
        "{min} .. {max}"
    );
}

/// 12. L2 块与相邻 L0 块的共享边界线上，顶点逐个逐位相等。
#[test]
fn 跨块接缝的顶点逐位相等() {
    let patches = [hilly()];
    let coarse = mesh(&patches, [0, 0], Lod::L2);
    let fine = mesh(&patches, [1, 0], Lod::L0);
    let boundary = |m: &TerrainMesh| -> Vec<(u32, f32)> {
        m.positions
            .iter()
            .filter(|p| (p[0] - BLOCK).abs() < 1e-4)
            .map(|p| (p[2].to_bits(), p[1]))
            .collect()
    };
    let (ca, fa) = (boundary(&coarse), boundary(&fine));
    assert!(
        ca.len() >= 2 && fa.len() >= 2,
        "边界上顶点太少：{} / {}",
        ca.len(),
        fa.len()
    );
    for (z_bits, y) in &ca {
        let z = f32::from_bits(*z_bits);
        let hit = fa
            .iter()
            .find(|(fz, _)| (f32::from_bits(*fz) - z).abs() < 1e-4)
            .unwrap_or_else(|| panic!("L0 侧缺 z={z} 的边界顶点"));
        assert_eq!(
            y.to_bits(),
            hit.1.to_bits(),
            "z={z} 处接缝高度不等：L2={y} L0={}",
            hit.1
        );
    }
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
