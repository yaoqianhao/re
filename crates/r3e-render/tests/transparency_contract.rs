//! 半透明排序与 MSAA 的对外契约（不需要 GPU）。
//!
//! 这里锁住三件容易被后续重构改坏的事：
//!
//! 1. `MSAA_SAMPLE_COUNT` 必须是 wgpu 三后端都支持的 4×，且不能再退回 1；
//! 2. `RenderStats` 默认值必须让「没有半透明工作」可被观测（0 而非哨兵值）；
//! 3. 排序 API 的组合语义：不透明批次不进半透明段，半透明段内由远及近。

use glam::Vec3;
use r3e_render::transparency::{back_to_front, is_transparent, view_depth, ALPHA_OPAQUE_THRESHOLD};
use r3e_render::{RenderStats, MSAA_SAMPLE_COUNT};

/// 主通道的采样数：4× 是 Vulkan / Metal / DX12 对 `Rgba16Float` 的共同下限，
/// 也是 resolve 到单采样 HDR 目标的前提。退回 1 等于关掉 MSAA。
#[test]
fn msaa_sample_count_is_four() {
    assert_eq!(MSAA_SAMPLE_COUNT, 4);
    // wgpu 只接受 2 的幂；非 2 的幂会在创建管线时被校验拒绝
    assert!(MSAA_SAMPLE_COUNT.is_power_of_two());
}

/// 统计默认值：没渲染过时半透明与深度预通道计数都是 0。
#[test]
fn stats_default_reports_no_transparent_work() {
    let stats = RenderStats::default();
    assert_eq!(stats.transparent_draws, 0);
    assert_eq!(stats.transparent_instances, 0);
    assert_eq!(stats.depth_prepass_draws, 0);
}

/// 不透明批次的排序键不该进半透明通道：alpha 乘出来略小于 1 也按不透明处理。
#[test]
fn opaque_after_alpha_product_stays_out_of_transparent_pass() {
    // 节点颜色 1.0 × 材质 0.9999999 —— 浮点乘法的典型结果
    let product = 1.0_f32 * 0.999_999_9_f32;
    assert!(!is_transparent(product));
    // 真正半透明的玻璃 / 水面材质
    assert!(is_transparent(0.35));
    // 阈值边界两侧
    assert!(!is_transparent(ALPHA_OPAQUE_THRESHOLD));
    assert!(is_transparent(ALPHA_OPAQUE_THRESHOLD - 1e-3));
}

/// 端到端语义：给定一组半透明实例的世界位置，`back_to_front` 必须给出
/// 「距离相机由远及近」的绘制下标，近处实例最后画才不会被远处覆盖。
#[test]
fn transparent_instances_are_drawn_far_to_near() {
    let eye = Vec3::ZERO;
    // 三个实例沿 +Z 排开：z=1(近) / z=8(最远) / z=4(中)
    let positions = [
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 0.0, 8.0),
        Vec3::new(0.0, 0.0, 4.0),
    ];
    let depths: Vec<f32> = positions
        .iter()
        .map(|position| view_depth(*position, eye))
        .collect();

    let order = back_to_front(&depths);
    // 远 → 中 → 近：对应下标 1、2、0
    assert_eq!(order, vec![1, 2, 0]);

    // 顺序里相邻两项的距离必须单调不增（严格递减时才能保证混合正确）
    let ordered: Vec<f32> = order.iter().map(|&i| depths[i as usize]).collect();
    assert!(
        ordered.windows(2).all(|pair| pair[0] >= pair[1]),
        "半透明绘制顺序必须由远及近，实际为 {ordered:?}"
    );
    // 近处实例（z=1）必须排在最后
    assert_eq!(*order.last().unwrap(), 0);
}

/// 相机移动后顺序必须跟着翻转：同一批实例，从反方向看时前后关系相反。
#[test]
fn order_flips_when_camera_moves_to_the_other_side() {
    let positions = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 6.0)];
    let front = back_to_front(
        &positions
            .iter()
            .map(|p| view_depth(*p, Vec3::ZERO))
            .collect::<Vec<_>>(),
    );
    let back = back_to_front(
        &positions
            .iter()
            .map(|p| view_depth(*p, Vec3::new(0.0, 0.0, 10.0)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(front, vec![1, 0]);
    assert_eq!(back, vec![0, 1]);
}

/// 排序结果必须是「每个实例恰好出现一次」的排列：漏项会让物件凭空消失，
/// 重复项则会让同一个物件被多画一遍、混合结果偏暗。
#[test]
fn order_is_a_permutation_of_every_instance() {
    let depths = [3.0, 0.5, 12.0, 7.5, 0.5, f32::NAN];
    let order = back_to_front(&depths);
    assert_eq!(order.len(), depths.len());
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (0..depths.len() as u32).collect::<Vec<_>>());
}
