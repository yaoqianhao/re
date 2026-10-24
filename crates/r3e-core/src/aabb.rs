//! 轴对齐包围盒（AABB）。
//!
//! 两个用途：
//!
//! * **视锥剔除**：把网格的局部包围盒经节点世界矩阵变换后与视锥平面做相交测试；
//! * **物理碰撞体推导**：外部资产的碰撞盒不再是「单位立方 × 缩放」，而是真实几何包围盒。
//!
//! 约定：`min` 与 `max` 按分量存储，不做自动排序；构造时请用 [`Aabb::new`]，
//! 它会保证 `min <= max`。空盒用 [`Aabb::EMPTY`]（`min = +inf`、`max = -inf`）表示，
//! 参与 [`Aabb::union`] 时会被自然忽略。

use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// 全空包围盒：任何 `union` 的起点。
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    /// 由两个对角点构造；自动按分量取 min/max，调用方不必关心点的先后。
    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    /// 顶点迭代器的最小/最大包围盒；空迭代返回 [`Aabb::EMPTY`]。
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut out = Self::EMPTY;
        for p in points {
            out = out.expand(p);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn center(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            (self.min + self.max) * 0.5
        }
    }

    /// 全尺寸（`max - min`）。
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            self.max - self.min
        }
    }

    /// 半尺寸（`size * 0.5`），物理碰撞体直接用它当 `half_extents`。
    pub fn half_extents(&self) -> Vec3 {
        self.size() * 0.5
    }

    pub fn expand(&self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// 各方向外扩固定距离；负值会收缩（可能变空）。
    pub fn inflate(&self, amount: f32) -> Self {
        if self.is_empty() {
            return *self;
        }
        Self {
            min: self.min - Vec3::splat(amount),
            max: self.max + Vec3::splat(amount),
        }
    }

    /// 按比例外扩，用于蒙皮网格：动画形变会超出静止姿态包围盒，留余量避免边缘被误剔除。
    pub fn inflate_relative(&self, ratio: f32) -> Self {
        if self.is_empty() {
            return *self;
        }
        let pad = self.size() * ratio.max(0.0);
        Self {
            min: self.min - pad,
            max: self.max + pad,
        }
    }

    /// 八个角点。
    pub fn corners(&self) -> [Vec3; 8] {
        let (min, max) = (self.min, self.max);
        [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ]
    }

    /// 用任意 4x4 矩阵变换：8 个角点各自变换后重新取包围盒。
    ///
    /// 旋转会让包围盒变大（这是 AABB 的固有保守性），换来的是测试只需 6 次点积。
    pub fn transformed(&self, m: &Mat4) -> Self {
        if self.is_empty() {
            return *self;
        }
        let mut out = Self::EMPTY;
        for c in self.corners() {
            out = out.expand(m.transform_point3(c));
        }
        out
    }

    pub fn to_tuple(self) -> (Vec3, Vec3) {
        (self.min, self.max)
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    #[test]
    fn new_normalizes_corner_order() {
        let a = Aabb::new(Vec3::new(1.0, 2.0, 3.0), Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(a.min, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(a.max, Vec3::new(1.0, 2.0, 3.0));
        assert!(!a.is_empty());
        assert_eq!(a.size(), Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(a.half_extents(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(a.center(), Vec3::ZERO);
    }

    #[test]
    fn empty_box_is_ignored_by_union() {
        let a = Aabb::new(Vec3::ZERO, Vec3::ONE);
        let u = Aabb::EMPTY.union(&a);
        assert_eq!(u, a);
        assert!(Aabb::EMPTY.is_empty());
        assert_eq!(Aabb::EMPTY.center(), Vec3::ZERO);
        assert_eq!(Aabb::EMPTY.size(), Vec3::ZERO);
    }

    #[test]
    fn from_points_tracks_extremes() {
        let a = Aabb::from_points([
            Vec3::new(1.0, 0.0, -2.0),
            Vec3::new(-3.0, 4.0, 0.0),
            Vec3::new(0.0, -1.0, 5.0),
        ]);
        assert_eq!(a.min, Vec3::new(-3.0, -1.0, -2.0));
        assert_eq!(a.max, Vec3::new(1.0, 4.0, 5.0));
    }

    #[test]
    fn transform_translates_and_rotates() {
        let a = Aabb::new(Vec3::new(-1.0, -1.0, -2.0), Vec3::new(1.0, 1.0, 2.0));

        let moved = a.transformed(&Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)));
        assert_eq!(moved.center(), Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(moved.size(), a.size());

        // 绕 Y 轴 90°：x/z 尺寸互换
        let rotated = a.transformed(&Mat4::from_quat(Quat::from_rotation_y(
            std::f32::consts::FRAC_PI_2,
        )));
        assert!((rotated.size().x - a.size().z).abs() < 1e-5);
        assert!((rotated.size().z - a.size().x).abs() < 1e-5);
        assert!((rotated.size().y - a.size().y).abs() < 1e-5);
    }

    #[test]
    fn inflate_variants_shrink_and_grow() {
        let a = Aabb::new(Vec3::ZERO, Vec3::splat(2.0));
        assert_eq!(a.inflate(0.5).size(), Vec3::splat(3.0));
        assert_eq!(a.inflate_relative(0.5).size(), Vec3::splat(4.0));
        // 相对外扩不改变中心
        assert_eq!(a.inflate_relative(0.25).center(), a.center());
    }
}
