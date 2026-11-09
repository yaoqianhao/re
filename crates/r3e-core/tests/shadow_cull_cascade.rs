//! 扩展二十四：级联遮挡体按级裁剪在**真实级联矩阵**下的行为。
//!
//! 前面的 `shadow_cull::tests` 只验证判定逻辑本身（用一个手搭的正交矩阵）。
//! 这一层关心的是「接到真实 CSM 拟合上之后还成不成立」：
//!
//! 1. **保守性**：落在第 k 级切片里的遮挡体，在第 k 级必须被保留——少画一个
//!    的后果是影子凭空缺一块，比多画严重得多；
//! 2. **真的省**：近级覆盖的世界范围小，保留的遮挡体必须明显少于远级；
//! 3. **不裁不掉的**：拿不到 AABB 的实例（`fail_open_bounds`）在任何级都不许被裁；
//! 4. **区间精确**：`split_runs` 切出来的区间并集必须恰好等于掩码里的保留集合，
//!    多一个会画错实例，少一个会丢影子。
//!
//! 全程不开窗口、不建 GPU：只吃矩阵与 AABB。

use glam::{Mat4, Vec3};
use r3e_core::light::{
    cascade_splits, fit_cascade, fit_cascade_extended, frustum_slice_corners, CascadeSettings,
    Light,
};
use r3e_core::{fail_open_bounds, plan_casters, split_runs, Aabb, Camera};

fn camera() -> Camera {
    Camera {
        position: Vec3::new(0.0, 2.0, 8.0),
        target: Vec3::new(0.0, 0.5, 0.0),
        znear: 0.1,
        zfar: 100.0,
        aspect: 16.0 / 9.0,
        ..Camera::default()
    }
}

/// 按渲染层的真实接入方式拟合各级矩阵：先紧贴拟合拿到 `radius`，
/// 再把近平面朝光源推一个 `radius`（= 该级半宽的一倍，文档给的经验值）。
fn level_matrices(count: u32) -> (Camera, Vec<Mat4>) {
    let camera = camera();
    let settings = CascadeSettings {
        count,
        lambda: 0.6,
        blend: 0.1,
        ..CascadeSettings::default()
    };
    let dir = Light::default().direction;
    let splits = cascade_splits(&camera, settings);
    let mut matrices = Vec::new();
    for w in splits.windows(2) {
        let corners = frustum_slice_corners(&camera, w[0], w[1]);
        let base = fit_cascade(dir, &corners, 2048);
        matrices.push(fit_cascade_extended(dir, &corners, 2048, base.radius).view_proj);
    }
    (camera, matrices)
}

/// 第 k 级切片中心处放一个遮挡体：它属于第 k 级的接收区，不能被第 k 级裁掉。
#[test]
fn caster_inside_its_own_level_is_kept() {
    let (camera, matrices) = level_matrices(4);
    let splits = cascade_splits(
        &camera,
        CascadeSettings {
            count: 4,
            lambda: 0.6,
            blend: 0.1,
            ..CascadeSettings::default()
        },
    );
    let mut boxes = Vec::new();
    for w in splits.windows(2) {
        let corners = frustum_slice_corners(&camera, w[0], w[1]);
        let center = corners.iter().fold(Vec3::ZERO, |acc, c| acc + *c) / 8.0;
        // 半径取该级跨度的四分之一：既像真实物件，又保证不会一路穿到别的级
        let half = ((w[1] - w[0]) * 0.25).max(0.05);
        boxes.push(Aabb::from_center_half(center, Vec3::splat(half)));
    }
    let masks = plan_casters(&matrices, &boxes);
    assert_eq!(masks.len(), 4);
    for (level, mask) in masks.iter().enumerate() {
        assert!(
            mask[level],
            "第 {level} 级切片中心的遮挡体在该级被裁掉了——这会直接让影子缺一块"
        );
    }
}

/// 近级只覆盖相机前方很短一段，保留的遮挡体必须明显少于远级；总数也要真的降下来。
///
/// 实测（4 级 / 32 个沿视线均匀分布的遮挡体）：逐级保留 `[9, 19, 32, 32]`，
/// 合计 92，相比全量的 128 省掉 28%。这已经是**最不利于裁剪**的分布——物件全在
/// 视线上、横向不散开；真实场景物件铺开在 x/y 上，近级会裁得更狠。
#[test]
fn near_level_keeps_fewer_casters_than_far_level() {
    let (camera, matrices) = level_matrices(4);
    // 沿相机视线方向均匀铺 32 个物件，距离 0.5..90
    let forward = (camera.target - camera.position).normalize();
    let boxes: Vec<Aabb> = (0..32)
        .map(|i| {
            let distance = 0.5 + 90.0 * (i as f32 / 31.0);
            let center = camera.position + forward * distance;
            Aabb::from_center_half(center, Vec3::splat(0.5))
        })
        .collect();

    let masks = plan_casters(&matrices, &boxes);
    let counts: Vec<usize> = masks
        .iter()
        .map(|m| m.iter().filter(|k| **k).count())
        .collect();
    assert!(
        counts[0] < counts[3],
        "近级保留数 {} 应当明显少于远级 {}（否则等于没裁）",
        counts[0],
        counts[3]
    );
    let kept: usize = counts.iter().sum();
    assert!(
        kept < 4 * boxes.len(),
        "按级裁剪要真的省：总保留 {kept} 应小于全量 {}",
        4 * boxes.len()
    );
    assert!(kept > 0, "不该把所有遮挡体都裁掉");
}

/// 拿不到 AABB 的实例在任何级都不许被裁（fail-open 是这套裁剪的底线）。
#[test]
fn fail_open_instances_survive_every_level() {
    let (_, matrices) = level_matrices(4);
    let boxes = vec![fail_open_bounds(); 3];
    for mask in plan_casters(&matrices, &boxes) {
        assert!(mask.iter().all(|keep| *keep), "兜底盒被裁掉了");
    }
}

/// 掩码 → 区间必须精确：并集等于保留集合，段数不超过保留个数。
#[test]
fn split_runs_matches_mask_exactly() {
    let (camera, matrices) = level_matrices(3);
    let forward = (camera.target - camera.position).normalize();
    let boxes: Vec<Aabb> = (0..24)
        .map(|i| {
            let distance = 0.5 + 60.0 * (i as f32 / 23.0);
            Aabb::from_center_half(camera.position + forward * distance, Vec3::splat(0.6))
        })
        .collect();
    let offset = 7u32; // 假装这一批实例从缓冲第 7 个开始
    for mask in plan_casters(&matrices, &boxes) {
        let runs = split_runs(offset, &mask);
        let mut covered = Vec::new();
        for run in &runs {
            for index in run.clone() {
                covered.push(index - offset);
            }
        }
        let mut want: Vec<u32> = mask
            .iter()
            .enumerate()
            .filter(|(_, keep)| **keep)
            .map(|(i, _)| i as u32)
            .collect();
        let mut got = covered;
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want, "区间切分与掩码不一致");
        assert!(runs.len() <= want.len().max(1), "段数不该超过保留个数");
    }
}
