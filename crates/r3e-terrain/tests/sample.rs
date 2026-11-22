//! 采样与法线行为测试（C01 · 7 条）。
//!
//! 第 12 条是**交叉断言**：`core::TerrainPatch::sample_height` 与 `r3e-nav` 烘焙期
//! 实际算出来的地面高度必须一致 —— 这条锁住 CD-3 的「上移不改行为」，
//! D 阶段删掉 nav 的私有实现时它不会变红。

use r3e_core::{Scene, TerrainPatch};
use r3e_nav::Nav;
use r3e_terrain::{sample_height_at, sample_normal};

/// 常数高度块：2×2 样本全为 `h`，覆盖 `[0,0] ~ [10,10]`。
fn constant_patch(h: f32) -> TerrainPatch {
    let mut p = TerrainPatch::flat("const", [0.0, 0.0], [10.0, 10.0]);
    p.resolution = [2, 2];
    p.heights = vec![h; 4];
    p
}

/// 沿 +x 的线性斜坡：3×3 样本，列值 `0/1/2`，因此解析解 `h(x) = 2·x / size_x`。
fn ramp_patch(size: f32) -> TerrainPatch {
    let mut p = TerrainPatch::flat("ramp", [0.0, 0.0], [size, size]);
    p.resolution = [3, 3];
    let mut heights = Vec::with_capacity(9);
    for _row in 0..3 {
        for col in 0..3 {
            heights.push(col as f32);
        }
    }
    p.heights = heights;
    p
}

#[test]
fn constant_height_samples_equal_everywhere() {
    let patch = constant_patch(5.0);
    for (x, z) in [(0.0f32, 0.0f32), (9.99, 0.0), (3.3, 7.7), (7.5, 7.5)] {
        assert_eq!(
            sample_height_at(std::slice::from_ref(&patch), x, z),
            Some(5.0),
            "常数高度块在 ({x}, {z}) 必须仍是 5.0"
        );
    }
}

#[test]
fn outside_patch_has_no_height() {
    let patch = constant_patch(5.0);
    assert_eq!(
        sample_height_at(std::slice::from_ref(&patch), 10.01, 5.0),
        None,
        "+x 出界"
    );
    assert_eq!(sample_height_at(&[patch], 5.0, -0.01), None, "-z 出界");
    // 边界本身含在内（闭区间口径）
    assert_eq!(
        sample_height_at(&[constant_patch(5.0)], 10.0, 10.0),
        Some(5.0)
    );
}

#[test]
fn linear_ramp_matches_analytic_solution() {
    let patch = ramp_patch(10.0);
    for x in [1.0f32, 2.5, 3.7, 6.4, 9.0] {
        let got = sample_height_at(std::slice::from_ref(&patch), x, 5.0).expect("块内必须采得到");
        let want = 2.0 * x / 10.0;
        assert!(
            (got - want).abs() < 1e-4,
            "x = {x}: 双线性采样 {got} 与解析解 {want} 偏差过大"
        );
    }
}

#[test]
fn nan_and_inf_coords_return_none_without_panic() {
    let patch = ramp_patch(10.0);
    assert_eq!(
        sample_height_at(std::slice::from_ref(&patch), f32::NAN, 5.0),
        None
    );
    assert_eq!(
        sample_height_at(std::slice::from_ref(&patch), 5.0, f32::NAN),
        None
    );
    assert_eq!(sample_height_at(&[patch], f32::INFINITY, 5.0), None);
    assert!(sample_normal(&[ramp_patch(10.0)], f32::NAN, 5.0, 0.5).is_none());
}

#[test]
fn overlapping_patches_take_the_highest_and_are_deterministic() {
    let ground = constant_patch(1.0);
    // 上层平台：2×2 米的小块，高 3 米，压在 ground 的 [2,2]~[6,6]
    let mut platform = TerrainPatch::flat("platform", [2.0, 2.0], [4.0, 4.0]);
    platform.resolution = [2, 2];
    platform.heights = vec![3.0; 4];
    let patches = vec![ground, platform];

    let first = sample_height_at(&patches, 3.0, 3.0).expect("重叠处必有两层");
    let second = sample_height_at(&patches, 3.0, 3.0).expect("同点第二次");
    assert_eq!(first, 3.0, "重叠处取最高的那一层");
    assert_eq!(first, second, "同点两次调用必须相等（确定性）");

    // 只有下层覆盖的点仍是 1.0：上层没盖到的地方不能被抬高
    assert_eq!(sample_height_at(&patches, 8.0, 8.0), Some(1.0));
}

#[test]
fn core_sample_matches_nav_bake_on_twenty_points() {
    let patch = ramp_patch(16.0);
    let mut scene = Scene::new();
    scene.terrain.push(patch.clone());
    scene.update_world_transforms();
    let nav = Nav::bake(&scene).expect("16×16 斜坡必须可烘焙");
    let grid = nav.grid();
    let [nx, ny] = grid.dims();

    let mut checked = 0;
    for k in 0..20u32 {
        // 避开最外圈：格中心要稳稳落在地形内
        let i = 2 + (k * 7) % nx.saturating_sub(4).max(1);
        let j = 2 + (k * 3) % ny.saturating_sub(4).max(1);
        let center = grid.cell_center(i, j);
        let nav_height = grid.ground(i, j);
        let core_height = sample_height_at(std::slice::from_ref(&patch), center.x, center.z)
            .expect("格中心必在地形范围内");
        assert!(
            (core_height - nav_height).abs() <= 1e-6,
            "格 ({i},{j}) 中心 ({}, {}): nav = {nav_height}, core = {core_height}（必须一致）",
            center.x,
            center.z
        );
        checked += 1;
    }
    assert_eq!(checked, 20, "交叉断言必须真的跑满 20 个点");
}

#[test]
fn normal_is_up_on_flat_and_tilts_toward_downhill() {
    let flat = constant_patch(0.0);
    let n = sample_normal(&[flat], 5.0, 5.0, 0.5).expect("块中心必须算得出法线");
    assert!(
        n[0].abs() < 1e-6 && n[2].abs() < 1e-6,
        "平地法线只能朝上：{n:?}"
    );
    assert!((n[1] - 1.0).abs() < 1e-5, "平地法线必须是单位 +Y：{n:?}");

    let ramp = ramp_patch(10.0);
    let n = sample_normal(&[ramp], 5.0, 5.0, 0.5).expect("斜坡中心必须算得出法线");
    assert!(n[0] < 0.0, "高度沿 +x 上升，法线 x 分量必须为负：{n:?}");
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    assert!((len - 1.0).abs() < 1e-5, "法线必须单位化：{n:?}");
    // 块边缘也要出法线（单侧越界用中心高度顶替，不允许 None 穿透）
    assert!(sample_normal(&[ramp_patch(10.0)], 0.1, 5.0, 0.5).is_some());
}
