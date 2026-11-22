//! LOD 选择与空地形行为测试（C01 · 3 条）。

use r3e_core::TerrainPatch;
use r3e_terrain::{
    lod_for, sample_height_at, sample_normal, Lod, LodConfig, LOD_DISTANCES, LOD_STEPS,
};

#[test]
fn lod_never_gets_finer_as_distance_grows() {
    let cfg = LodConfig::default();
    let mut prev = Lod::L0;
    let mut d = 0.0f32;
    while d <= 400.0 {
        let cur = lod_for(d, &cfg);
        assert!(
            cur >= prev && cur.step() >= prev.step(),
            "距离 {d} 米时档位回退到更细（{prev:?} → {cur:?}）"
        );
        prev = cur;
        d += 0.5;
    }
    // 阈值边界含在细档：d == near 仍是 L0
    assert_eq!(lod_for(LOD_DISTANCES[0], &cfg), Lod::L0);
    assert_eq!(
        lod_for(LOD_DISTANCES[0] + 0.01, &cfg),
        Lod::L1,
        "刚过 near 就降一档"
    );
    assert_eq!(lod_for(LOD_DISTANCES[1], &cfg), Lod::L1);
    assert_eq!(
        lod_for(LOD_DISTANCES[1] + 0.01, &cfg),
        Lod::L2,
        "刚过 far 就降到最粗"
    );
    // 算不出距离时给最省的几何，而不是全分辨率
    assert_eq!(lod_for(f32::NAN, &cfg), Lod::L2);
    assert_eq!(lod_for(f32::INFINITY, &cfg), Lod::L2);
}

#[test]
fn lod_steps_are_one_two_four_and_roundtrip() {
    assert_eq!(
        [Lod::L0.step(), Lod::L1.step(), Lod::L2.step()],
        LOD_STEPS,
        "档位步长必须与 LOD_STEPS 一致"
    );
    assert_eq!(LOD_STEPS, [1, 2, 4]);
    for (index, step) in LOD_STEPS.iter().enumerate() {
        let lod = Lod::from_step(*step).expect("合法步长必须能反查档位");
        assert_eq!(lod.index(), index);
        assert_eq!(Lod::from_index(index as u32), Some(lod));
    }
    assert!(Lod::from_step(3).is_none(), "非 2 的幂不能猜档位");
    assert!(Lod::from_index(3).is_none());
    // 阈值配置：反序 / 非正 / 非有限一律拒绝
    assert!(LodConfig::new(200.0, 64.0).is_none());
    assert!(LodConfig::new(0.0, 64.0).is_none());
    assert!(LodConfig::new(64.0, f32::NAN).is_none());
    assert_eq!(
        LodConfig::default(),
        LodConfig::new(LOD_DISTANCES[0], LOD_DISTANCES[1]).unwrap()
    );
}

#[test]
fn empty_terrain_samples_nothing() {
    let empty: Vec<TerrainPatch> = Vec::new();
    assert_eq!(sample_height_at(&empty, 0.0, 0.0), None, "没有块就没有高度");
    assert_eq!(sample_height_at(&empty, -5.0, 12.0), None);
    assert!(sample_normal(&empty, 0.0, 0.0, 0.5).is_none());
}
