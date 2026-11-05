//! 半透明批次排序（纯 CPU、纯函数，不碰 wgpu）。
//!
//! 前向管线里没有逐像素的深度排序，半透明物件只能靠「先画远的、后画近的」，
//! 让后画的内容通过 alpha 混合盖在前面的结果上。顺序一旦反了，近处物体
//! 会被远处物体盖住，混合结果与真实遮挡完全相反。
//!
//! 本模块只负责两件可单测的事：
//!
//! 1. 判定一个批次该不该走半透明通道（[`is_transparent`]）；
//! 2. 给出「由远及近」的绘制顺序（[`back_to_front`]），并且顺序**稳定**——
//!    深度相同的实例保持场景遍历顺序，否则同分情况会在帧间抖动。
//!
//! 排序键用「世界位置到相机的距离平方」：它与距离同序，但省掉一次 `sqrt`。
//! 只要相机位置与实例世界位置来自同一份数据，单调性就与真实视图深度一致。

use glam::Vec3;

/// alpha 大于等于该值即视为不透明。
///
/// 不用 `== 1.0` 判断：节点颜色与材质 alpha 相乘的结果常是 `0.9999999`，
/// 按位比较会把它们误判成半透明，白白多一遍「不写深度」的绘制。
pub const ALPHA_OPAQUE_THRESHOLD: f32 = 0.999;

/// 该 alpha 是否要走半透明通道。
///
/// `NaN` / 无穷一律按**不透明**处理：半透明通道不写深度，一旦排序键里混进
/// `NaN`，「谁更远」就失去意义；宁可当不透明画（写深度、有遮挡），
/// 也不要为了一个坏值把整批顺序搅乱。
pub fn is_transparent(alpha: f32) -> bool {
    alpha.is_finite() && alpha < ALPHA_OPAQUE_THRESHOLD
}

/// 排序键：世界位置到相机的距离平方（越大越远）。
pub fn view_depth(world_position: Vec3, camera_position: Vec3) -> f32 {
    (world_position - camera_position).length_squared()
}

/// 「由远及近」的索引顺序，用于把已收集好的半透明实例原地重排。
///
/// - 深度相同的元素保持输入顺序（稳定排序）→ 同一帧同一场景可复现；
/// - `NaN` 当作最远处理，保证比较器是全序，不会触发 `sort_by` 的不一致 panic；
/// - 空输入返回空 `Vec`，调用方无需单独判空。
pub fn back_to_front(depths: &[f32]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..depths.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let (far_a, far_b) = (rank(depths[a as usize]), rank(depths[b as usize]));
        // 远的先画：键降序；同分退回原始下标升序，保证稳定可复现
        far_b.total_cmp(&far_a).then(a.cmp(&b))
    });
    order
}

/// 把不可比较的浮点折成可全序比较的值：`NaN` 视为最远。
fn rank(depth: f32) -> f32 {
    if depth.is_nan() {
        f32::INFINITY
    } else {
        depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_opaque_alpha_is_not_transparent() {
        assert!(!is_transparent(1.0));
        // 0.999 是阈值本身：差一点点也算不透明，避免浮点乘法把不透明面料送进半透明通道
        assert!(!is_transparent(ALPHA_OPAQUE_THRESHOLD));
        assert!(!is_transparent(1.5));
    }

    #[test]
    fn partial_alpha_is_transparent() {
        assert!(is_transparent(0.5));
        assert!(is_transparent(0.0));
        assert!(is_transparent(ALPHA_OPAQUE_THRESHOLD - 1e-4));
    }

    #[test]
    fn non_finite_alpha_falls_back_to_opaque() {
        assert!(!is_transparent(f32::NAN));
        assert!(!is_transparent(f32::INFINITY));
        assert!(!is_transparent(f32::NEG_INFINITY));
    }

    #[test]
    fn view_depth_grows_with_distance() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let near = view_depth(Vec3::new(0.0, 0.0, 4.0), eye);
        let far = view_depth(Vec3::new(0.0, 0.0, -10.0), eye);
        assert!(near < far, "近处实例的排序键必须更小");
        assert_eq!(view_depth(eye, eye), 0.0);
    }

    #[test]
    fn back_to_front_orders_far_first() {
        // 1.0(近) / 3.0(远) / 2.0(中) → 远、中、近
        assert_eq!(back_to_front(&[1.0, 3.0, 2.0]), vec![1, 2, 0]);
    }

    #[test]
    fn equal_depths_keep_input_order() {
        // 稳定排序：同深度不交换，避免帧间抖动
        assert_eq!(back_to_front(&[2.0, 2.0, 2.0]), vec![0, 1, 2]);
        assert_eq!(back_to_front(&[4.0, 1.0, 4.0]), vec![0, 2, 1]);
    }

    #[test]
    fn nan_sorts_to_the_far_end() {
        // NaN 当最远：排在最前，但不影响其余元素的相对顺序
        assert_eq!(back_to_front(&[1.0, f32::NAN, 2.0]), vec![1, 2, 0]);
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(back_to_front(&[]).is_empty());
        assert_eq!(back_to_front(&[7.0]), vec![0]);
    }

    #[test]
    fn order_covers_every_index_once() {
        let depths = [0.5, 9.0, 3.0, 3.0, 0.0];
        let mut order = back_to_front(&depths);
        order.sort_unstable();
        assert_eq!(order, vec![0, 1, 2, 3, 4]);
    }
}
