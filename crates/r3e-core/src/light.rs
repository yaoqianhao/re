//! 单方向光 + 环境光（Blinn-Phong 光照模型）。

use glam::Vec3;

#[derive(Debug, Clone, Copy)]
pub struct Light {
    /// 平行光方向（从光源射出的方向）。
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
    pub ambient: Vec3,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.45, -0.8, -0.4).normalize(),
            color: Vec3::new(1.0, 0.97, 0.92),
            intensity: 1.0,
            ambient: Vec3::new(0.18, 0.19, 0.23),
        }
    }
}
