//! 刚体变换：位置 / 旋转 / 缩放三元组。

use glam::{Mat4, Quat, Vec3};

/// 单个节点的局部变换。
///
/// 组合顺序固定为 `T * R * S`，与主流引擎一致。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn new(position: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            position,
            rotation,
            scale,
        }
    }

    pub fn from_position(position: Vec3) -> Self {
        Self {
            position,
            ..Self::IDENTITY
        }
    }

    /// 平移（世界方向的局部增量）。
    pub fn translate(&mut self, delta: Vec3) {
        self.position += delta;
    }

    /// 按欧拉角（角度制，XYZ 顺序）设置旋转。
    pub fn set_euler_deg(&mut self, deg: Vec3) {
        self.rotation = Quat::from_euler(
            glam::EulerRot::XYZ,
            deg.x.to_radians(),
            deg.y.to_radians(),
            deg.z.to_radians(),
        );
    }

    /// 返回欧拉角（角度制）。
    pub fn euler_deg(&self) -> Vec3 {
        let (x, y, z) = self.rotation.to_euler(glam::EulerRot::XYZ);
        Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
    }

    /// 绕世界 Y 轴追加旋转（角度制）。
    pub fn rotate_y_deg(&mut self, deg: f32) {
        self.rotation = Quat::from_rotation_y(deg.to_radians()) * self.rotation;
    }

    /// 绕任意轴追加旋转（弧度制）。
    pub fn rotate_axis(&mut self, axis: Vec3, angle_rad: f32) {
        let axis = axis.normalize_or_zero();
        if axis != Vec3::ZERO {
            self.rotation = Quat::from_axis_angle(axis, angle_rad) * self.rotation;
        }
    }

    /// 朝向某个目标点（Y 轴向上）。
    pub fn look_at(&mut self, target: Vec3, up: Vec3) {
        let dir = target - self.position;
        if dir.length_squared() > 1e-8 {
            // look_at_quat 给出的是相机朝向（-Z 前向），物件需取反
            self.rotation = glam::camera::rh::view::look_at_quat(self.position, target, up).inverse();
        }
    }

    /// 局部变换矩阵。
    pub fn local_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    /// 法线矩阵（逆转置），用于非均匀缩放下的正确光照。
    pub fn normal_matrix(&self) -> glam::Mat3 {
        glam::Mat3::from_quat(self.rotation)
            * glam::Mat3::from_diagonal(Vec3::new(
                1.0 / self.scale.x.max(1e-6),
                1.0 / self.scale.y.max(1e-6),
                1.0 / self.scale.z.max(1e-6),
            ))
    }
}
