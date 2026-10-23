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
