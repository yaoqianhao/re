//! 视锥（6 个裁剪平面）与 AABB 的相交测试。
//!
//! 平面由 `view_projection` 矩阵按 Gribb–Hartmann 法提取：
//! 记 `r0..r3` 为矩阵的四行，则
//!
//! ```text
//! left   = r3 + r0        right = r3 - r0
//! bottom = r3 + r1        top   = r3 - r1
//! near   = r2             far   = r3 - r2     （NDC 深度 0..1，wgpu 约定）
//! ```
//!
//! 若用 `-1..1` 的 OpenGL 约定，近平面应为 `r3 + r2`；本引擎的
//! [`crate::camera::Camera::projection`] 走 directx 风格，所以这里是 `r2`。
//!
//! 测试方式：取包围盒在平面法线方向上的「最远点」（p-vertex），
//! 只要该点在平面外侧，整个盒子必定在外侧 —— 6 个平面各一次，足够保守且不误剔。

use crate::aabb::Aabb;
use glam::{Mat4, Vec3, Vec4};

#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    /// 六个平面，法线朝向视锥内部（等价于 `dot(n, p) + d >= 0` 表示在内侧）。
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_view_projection(vp: &Mat4) -> Self {
        let row = |i: usize| Vec4::new(vp.x_axis[i], vp.y_axis[i], vp.z_axis[i], vp.w_axis[i]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let planes = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            r2,      // near（NDC 0..1）
            r3 - r2, // far
        ];
        Self {
            planes: planes.map(normalize_plane),
        }
    }

    pub fn planes(&self) -> &[Vec4; 6] {
        &self.planes
    }

    /// 点是否在视锥内。
    pub fn contains_point(&self, p: Vec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.truncate().dot(p) + plane.w >= 0.0)
    }

    /// AABB 是否与视锥相交（视锥外的盒子返回 `false`）。
    ///
    /// 保守判定：可能保留少量视锥外的盒子（贴着边角的情况），但绝不会剔除视锥内的物体。
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        if aabb.is_empty() {
            return false;
        }
        for plane in &self.planes {
            let n = plane.truncate();
            // 沿法线方向的最远点：法线分量为正取 max，否则取 min
            let p = Vec3::new(
                if n.x >= 0.0 { aabb.max.x } else { aabb.min.x },
                if n.y >= 0.0 { aabb.max.y } else { aabb.min.y },
                if n.z >= 0.0 { aabb.max.z } else { aabb.min.z },
            );
            if n.dot(p) + plane.w < 0.0 {
                return false;
            }
        }
        true
    }
}

fn normalize_plane(p: Vec4) -> Vec4 {
    let len = p.truncate().length();
    if len > 1e-8 {
        p / len
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    fn camera_at(pos: Vec3) -> Camera {
        Camera {
            position: pos,
            target: Vec3::ZERO,
            ..Default::default()
        }
    }

    #[test]
    fn aabb_in_front_is_visible() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        // 原点附近的小盒子：正对相机
        let aabb = Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5));
        assert!(frustum.intersects_aabb(&aabb));
        assert!(frustum.contains_point(Vec3::ZERO));
    }

    #[test]
    fn aabb_behind_camera_is_culled() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        // 相机在 +z，视锥朝 -z（看向原点）；+z 更远处在背后
        let behind = Aabb::new(Vec3::new(-0.5, -0.5, 9.0), Vec3::new(0.5, 0.5, 11.0));
        assert!(!frustum.intersects_aabb(&behind));
        assert!(!frustum.contains_point(Vec3::new(0.0, 0.0, 10.0)));
    }

    #[test]
    fn aabb_beyond_far_plane_is_culled() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        let far = Aabb::new(
            Vec3::new(-1.0, -1.0, -cam.zfar - 50.0),
            Vec3::new(1.0, 1.0, -cam.zfar - 10.0),
        );
        assert!(!frustum.intersects_aabb(&far));
    }

    #[test]
    fn aabb_far_to_the_side_is_culled() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        // 距原点 100 m 的侧向盒子，60° FOV 下必然在视锥外
        let side = Aabb::new(Vec3::new(99.0, -1.0, -1.0), Vec3::new(101.0, 1.0, 1.0));
        assert!(!frustum.intersects_aabb(&side));
    }

    #[test]
    fn straddling_aabb_is_kept() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        // 横跨相机平面的巨大盒子：即使中心在背后也必须保留（保守性）
        let huge = Aabb::new(Vec3::splat(-20.0), Vec3::splat(20.0));
        assert!(frustum.intersects_aabb(&huge));
    }

    #[test]
    fn empty_aabb_is_never_visible() {
        let cam = camera_at(Vec3::new(0.0, 0.0, 5.0));
        let frustum = Frustum::from_view_projection(&cam.view_projection());
        assert!(!frustum.intersects_aabb(&Aabb::EMPTY));
    }
}
