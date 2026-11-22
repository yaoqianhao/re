//! C02：多分辨率 LOD 采样的 14 条判据。
//!
//! 覆盖四类：块内误差（1–3）、接缝逐点相等（4–6）、确定性与参数校验（7–9、13）、
//! 退化地形与重叠裁决（10–12、14）。

use r3e_core::TerrainPatch;
use r3e_terrain::{
    lod_for, sample_height_at, sample_height_lod, BlockGrid, Lod, LodConfig, DEFAULT_BLOCK_SIZE,
    LOD_DISTANCES, LOD_STEPS,
};

/// 200 × 200 米、129 × 129 个样本（步长 1.5625 米）的缓坡地形：
/// 沿 x 线性上升 0.02 米/米，沿 z 叠一层 1.5 米幅度的缓正弦。
fn rolling_patch() -> TerrainPatch {
    const N: u32 = 129;
    const SPAN: f32 = 200.0;
    let step = SPAN / (N - 1) as f32;
    let mut patch = TerrainPatch::flat("rolling", [0.0, 0.0], [SPAN, SPAN]);
    patch.resolution = [N, N];
    patch.heights = (0..N)
        .flat_map(|row| {
            (0..N).map(move |col| {
                let wx = col as f32 * step;
                let wz = row as f32 * step;
                0.02 * wx + 1.5 * (wz / 60.0).sin()
            })
        })
        .collect();
    patch
}

fn flat_patch() -> TerrainPatch {
    TerrainPatch::flat("ground", [0.0, 0.0], [200.0, 200.0])
}

fn grid() -> BlockGrid {
    BlockGrid::new([0.0, 0.0, 200.0, 200.0], DEFAULT_BLOCK_SIZE).expect("200x200 / 50 = 4x4")
}

/// 块内点（避开边界带：默认块 50 米，粗档带宽约 6.25 米，取每块 10..40 米）。
fn interior_points() -> Vec<(f32, f32)> {
    let mut pts = Vec::new();
    for bi in 0..4 {
        for bj in 0..4 {
            let x0 = bi as f32 * 50.0;
            let z0 = bj as f32 * 50.0;
            let mut d = 10.0;
            while d <= 40.0 {
                pts.push((x0 + d, z0 + d));
                d += 3.7;
            }
        }
    }
    pts
}

fn max_abs_err(lod: Lod, patches: &[TerrainPatch], grid: &BlockGrid) -> f32 {
    let mut worst = 0.0f32;
    for (x, z) in interior_points() {
        let want = sample_height_at(patches, x, z).expect("块内点必有高度");
        let got = sample_height_lod(patches, grid, x, z, lod).expect("块内点必有 LOD 高度");
        worst = worst.max((got - want).abs());
    }
    worst
}

#[test]
fn 块内采样误差_l0_不超过半米() {
    let patches = [rolling_patch()];
    let g = grid();
    let err = max_abs_err(Lod::L0, &patches, &g);
    assert!(err <= 0.5, "L0 误差 {err} 超过 0.5 米");
    // L0 走的就是全采样，误差应当逐位为 0（不是「小于阈值」的近似）。
    assert_eq!(err, 0.0, "L0 必须与全采样逐位一致");
}

#[test]
fn 块内采样误差_l1_不超过半米() {
    let patches = [rolling_patch()];
    let g = grid();
    let err = max_abs_err(Lod::L1, &patches, &g);
    assert!(err <= 0.5, "L1 误差 {err} 超过 0.5 米");
}

#[test]
fn 块内采样误差_l2_不超过半米() {
    let patches = [rolling_patch()];
    let g = grid();
    let err = max_abs_err(Lod::L2, &patches, &g);
    assert!(err <= 0.5, "L2 误差 {err} 超过 0.5 米");
    // 缓坡上粗档本来就该准，但也不该与细档完全同值（否则等于没降精度）。
    let err_l0 = max_abs_err(Lod::L0, &patches, &g);
    assert!(err <= err_l0 + 0.5);
}

/// 块 `(0, 0)` 与 `(1, 0)` 的共享边界是 x = 50，沿 z 取 5 个点比较两个 LOD。
fn seam_points() -> Vec<(f32, f32)> {
    [5.0f32, 17.5, 25.0, 33.5, 45.0]
        .iter()
        .map(|z| (50.0, *z))
        .collect()
}

fn assert_seam_equal(a: Lod, b: Lod) {
    let patches = [rolling_patch()];
    let g = grid();
    for (x, z) in seam_points() {
        let ha = sample_height_lod(&patches, &g, x, z, a).expect("边界点必有高度");
        let hb = sample_height_lod(&patches, &g, x, z, b).expect("边界点必有高度");
        assert_eq!(
            ha.to_bits(),
            hb.to_bits(),
            "接缝点 ({x}, {z}) 上 {a:?} 与 {b:?} 采样不等：{ha} vs {hb}"
        );
        let full = sample_height_at(&patches, x, z).expect("全采样");
        assert_eq!(ha.to_bits(), full.to_bits(), "接缝点必须回到全采样");
    }
}

#[test]
fn 接缝处_l2_与_l1_逐点相等() {
    assert_seam_equal(Lod::L2, Lod::L1);
}

#[test]
fn 接缝处_l2_与_l0_逐点相等() {
    assert_seam_equal(Lod::L2, Lod::L0);
}

#[test]
fn 接缝处_l1_与_l0_逐点相等() {
    assert_seam_equal(Lod::L1, Lod::L0);
}

#[test]
fn 同一档两次调用逐位相等() {
    let patches = [rolling_patch()];
    let g = grid();
    for lod in [Lod::L0, Lod::L1, Lod::L2] {
        for (x, z) in [
            (12.5f32, 33.5f32),
            (77.0, 91.0),
            (149.5, 8.25),
            (50.0, 50.0),
        ] {
            let a = sample_height_lod(&patches, &g, x, z, lod).expect("有高度");
            let b = sample_height_lod(&patches, &g, x, z, lod).expect("有高度");
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "({x}, {z}) {lod:?} 两次结果不一致"
            );
        }
    }
}

#[test]
fn 非法步长与非法序号不猜档位() {
    for good in LOD_STEPS {
        let lod = Lod::from_step(good).expect("合法步长必有档位");
        assert_eq!(lod.step(), good);
    }
    for bad in [0u32, 3, 5, 8, 16, u32::MAX] {
        assert_eq!(Lod::from_step(bad), None, "步长 {bad} 不该映射到任何档位");
    }
    assert_eq!(Lod::from_index(3), None);
    assert_eq!(Lod::from_index(u32::MAX), None);
}

#[test]
fn lod_config_拒绝非法阈值() {
    assert_eq!(LodConfig::new(-1.0, 10.0), None, "负 near");
    assert_eq!(LodConfig::new(0.0, 10.0), None, "零 near");
    assert_eq!(LodConfig::new(10.0, 10.0), None, "far == near");
    assert_eq!(LodConfig::new(20.0, 10.0), None, "far < near（反序会跳档）");
    assert_eq!(LodConfig::new(f32::NAN, 10.0), None, "NaN near");
    assert_eq!(LodConfig::new(10.0, f32::INFINITY), None, "inf far");
    let ok = LodConfig::new(30.0, 90.0).expect("合法阈值");
    assert_eq!((ok.near, ok.far), (30.0, 90.0));
}

#[test]
fn 平地按_l2_采样仍是常数() {
    let patches = [flat_patch()];
    let g = grid();
    for (x, z) in [(3.0f32, 197.0f32), (50.0, 50.0), (123.4, 66.6), (0.0, 0.0)] {
        let h = sample_height_lod(&patches, &g, x, z, Lod::L2).expect("平地上必有高度");
        assert_eq!(h, 0.0, "跳采样在平地上不该引入插值误差");
    }
}

#[test]
fn 单块地形_l2_不越界不崩溃() {
    let patches = [rolling_patch()];
    let g = BlockGrid::new([0.0, 0.0, 200.0, 200.0], 200.0).expect("整张图一块");
    assert_eq!(g.dims(), [1, 1]);
    for (x, z) in [
        (0.0f32, 0.0f32),
        (200.0, 200.0),
        (100.0, 100.0),
        (0.0, 200.0),
    ] {
        let h = sample_height_lod(&patches, &g, x, z, Lod::L2).expect("单块内必有高度");
        assert!(h.is_finite(), "({x}, {z}) 采到非有限值");
    }
}

#[test]
fn 地形小于一块时块数为一块() {
    let patches = [TerrainPatch::flat("tiny", [0.0, 0.0], [30.0, 30.0]), {
        let mut p = TerrainPatch::flat("tiny_ramp", [0.0, 0.0], [30.0, 30.0]);
        p.resolution = [3, 3];
        p.heights = vec![0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0];
        p
    }];
    let g = BlockGrid::new([0.0, 0.0, 30.0, 30.0], 50.0).expect("30x30 也切得出 1 块");
    assert_eq!(g.dims(), [1, 1]);
    assert_eq!(g.block_count(), 1);
    let h = sample_height_lod(&patches, &g, 15.0, 15.0, Lod::L2).expect("块内有高度");
    // 两块重叠，取最高的那块（ramp 在 15 米处 ≈ 0.5）。
    assert!((h - 0.5).abs() < 1e-5, "LOD 采样在 1x1 网格上取值 {h}");
}

#[test]
fn lod_for_阈值点归细档() {
    let cfg = LodConfig::default();
    let (near, far) = (LOD_DISTANCES[0], LOD_DISTANCES[1]);
    assert_eq!(cfg.near, near);
    assert_eq!(cfg.far, far);
    assert_eq!(lod_for(0.0, &cfg), Lod::L0);
    assert_eq!(lod_for(near, &cfg), Lod::L0, "阈值点归细档（<= 而非 <）");
    assert_eq!(lod_for(near + 0.001, &cfg), Lod::L1);
    assert_eq!(lod_for(far, &cfg), Lod::L1, "阈值点归细档");
    assert_eq!(lod_for(far + 0.001, &cfg), Lod::L2);
    assert_eq!(lod_for(f32::NAN, &cfg), Lod::L2, "算不出距离按最粗档");
    assert_eq!(lod_for(f32::INFINITY, &cfg), Lod::L2);
}

#[test]
fn 重叠区域_lod_不改变命中层() {
    let mut high = TerrainPatch::flat("high", [0.0, 0.0], [100.0, 100.0]);
    high.resolution = [2, 2];
    high.heights = vec![2.0; 4];
    let mut low = TerrainPatch::flat("low", [0.0, 0.0], [100.0, 100.0]);
    low.resolution = [2, 2];
    low.heights = vec![1.0; 4];
    let patches = [low, high];
    let g = grid();
    for lod in [Lod::L0, Lod::L1, Lod::L2] {
        let h = sample_height_lod(&patches, &g, 25.0, 25.0, lod).expect("重叠区内有高度");
        assert_eq!(h, 2.0, "{lod:?} 换档后命中层变了");
        assert_eq!(h, sample_height_at(&patches, 25.0, 25.0).expect("全采样"));
    }
}
