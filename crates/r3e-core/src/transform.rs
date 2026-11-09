//! 刚体变换：位置 / 旋转 / 缩放三元组。

use glam::{Mat3, Mat4, Quat, Vec3};

/// 线性部分被判为不可逆的行列式阈值（`|det|` 小于它即视为某轴被压到 0）。
///
/// 取 `1e-12`：在 `f32` 下 1e-6 量级的缩放三次相乘正好落在它附近，
/// 再小就会被 `inverse()` 放大成 1e12 以上的噪声（虽然仍是有限值，但已经完全没意义）。
pub const SINGULAR_DET_EPS: f32 = 1e-12;

/// 参与法线求解的最小缩放绝对值。
///
/// 低于它的分量按它算，避免 `1 / 0 = inf`；符号保留，所以负缩放（镜像）方向不会被抹平。
pub const MIN_LINEAR_SCALE: f32 = 1e-6;

/// 把单个缩放分量收进安全区间：非有限值退化为 [`MIN_LINEAR_SCALE`]，
/// 过小值按原来的符号夹到 ±[`MIN_LINEAR_SCALE`]，其余原样返回。
pub fn safe_scale_component(s: f32) -> f32 {
    if !s.is_finite() {
        MIN_LINEAR_SCALE
    } else if s.abs() < MIN_LINEAR_SCALE {
        if s.is_sign_negative() {
            -MIN_LINEAR_SCALE
        } else {
            MIN_LINEAR_SCALE
        }
    } else {
        s
    }
}

/// 世界法线矩阵（逆转置），**对退化输入也保证结果是有限值**。
///
/// 正常情形与直接写 `Mat3::from_mat4(world).inverse().transpose()` 逐位相同；
/// 只有当线性部分不可逆（`|det| < SINGULAR_DET_EPS`，例如某轴 `scale = 0`）
/// 或求逆结果出现 `NaN` / `inf` 时，才退到「逐列归一化重建旋转 × 逐轴最小缩放」
/// 的组合式算法——精度不如真逆，但不会把 `NaN` 传进光照与快照。
pub fn world_normal_matrix(world: Mat4) -> Mat3 {
    let linear = Mat3::from_mat4(world);
    let det = linear.determinant();
    if det.is_finite() && det.abs() >= SINGULAR_DET_EPS {
        let normal = linear.inverse().transpose();
        if normal.is_finite() {
            return normal;
        }
    }
    degenerate_normal_matrix(linear)
}

/// 退化情形下的法线矩阵：把每个轴拆成「方向（单位化）」和「缩放（夹进安全区间）」。
///
/// 只对 TRS 复合精确；父链带 shear 时是近似，但这个分支本身只在真不可逆时走到。
fn degenerate_normal_matrix(linear: Mat3) -> Mat3 {
    let (dx, ix) = axis_and_inverse_scale(linear.x_axis, Vec3::X);
    let (dy, iy) = axis_and_inverse_scale(linear.y_axis, Vec3::Y);
    let (dz, iz) = axis_and_inverse_scale(linear.z_axis, Vec3::Z);
    Mat3::from_cols(dx, dy, dz) * Mat3::from_diagonal(Vec3::new(ix, iy, iz))
}

/// 单列的「方向 + 缩放倒数」；列退化（长度 0 / 非有限）时方向退回 `fallback` 单位轴。
fn axis_and_inverse_scale(col: Vec3, fallback: Vec3) -> (Vec3, f32) {
    let len = col.length();
    if len.is_finite() && len >= MIN_LINEAR_SCALE {
        (col / len, 1.0 / len)
    } else {
        (fallback, 1.0 / MIN_LINEAR_SCALE)
    }
}

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
            self.rotation =
                glam::camera::rh::view::look_at_quat(self.position, target, up).inverse();
        }
    }

    /// 局部变换矩阵。
    pub fn local_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    /// 法线矩阵（逆转置），用于非均匀缩放下的正确光照。
    ///
    /// 这里只覆盖**本节点自己**的缩放；父链缩放要乘以 [`world_normal_matrix`] 的结果。
    /// 缩放分量过小时按 [`MIN_LINEAR_SCALE`] 夹紧，**负缩放保留符号**（镜像物件的法线要跟着翻）。
    pub fn normal_matrix(&self) -> glam::Mat3 {
        glam::Mat3::from_quat(self.rotation)
            * glam::Mat3::from_diagonal(Vec3::new(
                1.0 / safe_scale_component(self.scale.x),
                1.0 / safe_scale_component(self.scale.y),
                1.0 / safe_scale_component(self.scale.z),
            ))
    }
}
