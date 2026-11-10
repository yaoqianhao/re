//! 透视相机。

use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// 垂直视场角（角度制）。
    pub fov_deg: f32,
    pub znear: f32,
    pub zfar: f32,
    pub aspect: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(4.0, 3.0, 6.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_deg: 60.0,
            znear: 0.05,
            zfar: 500.0,
            aspect: 16.0 / 9.0,
        }
    }
}

impl Camera {
    pub fn new(position: Vec3, target: Vec3) -> Self {
        Self {
            position,
            target,
            ..Default::default()
        }
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.position, self.target, self.up)
    }

    /// 注意：使用 directx 风格投影（NDC 深度 0..1、Y 向上），与 wgpu 一致。
    pub fn projection(&self) -> Mat4 {
        // wgpu 的 NDC 深度区间是 [0, 1]，对应 glam 的 directx 投影族
        glam::camera::rh::proj::directx::perspective(
            self.fov_deg.to_radians(),
            self.aspect.max(1e-4),
            self.znear,
            self.zfar,
        )
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection() * self.view()
    }

    /// 屏幕像素 → 世界射线（扩展三十四）。`x`/`y` 为窗口坐标：左上原点、y 向下。
    ///
    /// `width`/`height` 传窗口逻辑像素尺寸；非正数按 1 兜底，避免除零。
    /// 相机 `aspect` 与这里的宽高比不一致时以 `aspect` 为准（投影矩阵里已经用了它），
    /// 调用方应保证两者同源，否则拾取会与画面有偏移。
    pub fn screen_ray(&self, x: f32, y: f32, width: f32, height: f32) -> crate::pick::Ray {
        let w = if width > 0.0 { width } else { 1.0 };
        let h = if height > 0.0 { height } else { 1.0 };
        // 像素 → NDC：x 向右为 +1，屏幕 y 向下而 NDC y 向上，因此要取反
        self.ndc_ray((x / w) * 2.0 - 1.0, 1.0 - (y / h) * 2.0)
    }

    /// NDC → 世界射线。NDC 深度 0 为近平面、1 为远平面（与 [`Camera::projection`] 一致）。
    ///
    /// 做法是反投影近/远两点再连线：这样透视除法、非对称视锥（将来若加）都由矩阵自己处理，
    /// 不必在相机这层重写一遍投影公式。反投影退化（矩阵不可逆 / 齐次 w 为 0）时
    /// 退回「从相机位置沿视线前向」这条保守射线。
    pub fn ndc_ray(&self, nx: f32, ny: f32) -> crate::pick::Ray {
        let inverse = self.view_projection().inverse();
        let near = inverse * glam::Vec4::new(nx, ny, 0.0, 1.0);
        let far = inverse * glam::Vec4::new(nx, ny, 1.0, 1.0);
        let forward = (self.target - self.position).normalize_or_zero();
        let forward = if forward.length_squared() > 0.5 {
            forward
        } else {
            Vec3::NEG_Z
        };
        if near.w.abs() < 1e-9 || far.w.abs() < 1e-9 {
            return crate::pick::Ray::new(self.position, forward);
        }
        let near_pos = near.truncate() / near.w;
        let far_pos = far.truncate() / far.w;
        let dir = far_pos - near_pos;
        if !dir.is_finite() || dir.length_squared() < 1e-12 {
            return crate::pick::Ray::new(self.position, forward);
        }
        // 起点用相机位置而不是近平面点：两者在同一条射线上，但相机位置不受 znear 影响，
        // 「贴脸的物件」也能被点中。
        crate::pick::Ray::new(self.position, dir)
    }

    pub fn orbit(&mut self, yaw_deg: f32, pitch_deg: f32) {
        let offset = self.position - self.target;
        let radius = offset.length().max(1e-3);
        let mut yaw = offset.z.atan2(offset.x);
        let mut pitch = (offset.y / radius).asin();
        yaw += yaw_deg.to_radians();
        pitch = (pitch + pitch_deg.to_radians()).clamp(-1.5, 1.5);
        self.position = self.target
            + Vec3::new(
                radius * pitch.cos() * yaw.cos(),
                radius * pitch.sin(),
                radius * pitch.cos() * yaw.sin(),
            );
    }
}
