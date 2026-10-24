//! 单方向光 + 环境光（Blinn-Phong 光照模型），含方向光阴影配置（阶段 D）。

use glam::{Mat4, Vec3};

/// 方向光阴影贴图配置（阶段 D）。
///
/// 阴影相机是一台沿 `Light::direction` 观察场景包围球的正交相机：
/// `ortho_size == 0` 时半宽按包围球每帧自适应，大于 0 时固定为该半宽
/// （适合只看某个兴趣区域，或减小贴图拉伸）。
#[derive(Debug, Clone, Copy)]
pub struct ShadowSettings {
    /// 是否启用阴影。
    pub enabled: bool,
    /// 阴影贴图分辨率（正方形，像素）。规范化后夹到 [64, 8192]。
    pub size: u32,
    /// 正交相机半宽（世界单位）。0 = 按场景包围球自适应。
    pub ortho_size: f32,
    /// 深度比较偏移，抑制自阴影条纹（acne）。
    pub bias: f32,
    /// 法线方向偏移（单位：纹素），斜射面抗 acne 且比纯深度偏移更少漂浮。
    pub normal_bias: f32,
    /// 光源相机后退距离 = 包围球半径 × distance + 1。
    pub distance: f32,
    /// 阴影强度 0..1：1 = 完全遮挡，0 = 无阴影。
    pub strength: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 2048,
            ortho_size: 0.0,
            bias: 0.0015,
            normal_bias: 1.5,
            distance: 2.0,
            strength: 1.0,
        }
    }
}

impl ShadowSettings {
    /// 规范化：把分辨率与各标量夹到可用区间，避免脚本/场景传入坏值。
    pub fn sanitized(mut self) -> Self {
        self.size = self.size.clamp(64, 8192);
        if !self.ortho_size.is_finite() || self.ortho_size < 0.0 {
            self.ortho_size = 0.0;
        }
        self.bias = if self.bias.is_finite() {
            self.bias.clamp(0.0, 0.05)
        } else {
            0.0
        };
        self.normal_bias = if self.normal_bias.is_finite() {
            self.normal_bias.clamp(0.0, 8.0)
        } else {
            0.0
        };
        self.distance = if self.distance.is_finite() {
            self.distance.clamp(0.05, 16.0)
        } else {
            2.0
        };
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Light {
    /// 平行光方向（从光源射出的方向）。
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
    pub ambient: Vec3,
    /// 方向光阴影配置（阶段 D）。
    pub shadow: ShadowSettings,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.45, -0.8, -0.4).normalize(),
            color: Vec3::new(1.0, 0.97, 0.92),
            intensity: 1.0,
            ambient: Vec3::new(0.18, 0.19, 0.23),
            shadow: ShadowSettings::default(),
        }
    }
}

impl Light {
    /// 阴影相机的 view-projection（正交、右乘、NDC 深度 0..1，与 wgpu 一致）。
    ///
    /// `center` / `radius` 为场景包围球；`radius <= 0` 时退回 1.0。
    /// 光源放在球心沿 `-direction` 方向 `radius × distance + 1` 处，
    /// 远平面留足球体前后余量，因此光源换向时不需要重算包围盒。
    pub fn shadow_view_projection(&self, center: Vec3, radius: f32) -> Mat4 {
        let s = self.shadow.sanitized();
        let radius = if radius.is_finite() && radius > 1e-4 {
            radius
        } else {
            1.0
        };
        let dir = if self.direction.length_squared() > 1e-8 {
            self.direction.normalize()
        } else {
            Vec3::NEG_Y
        };
        let half = if s.ortho_size > 0.0 {
            s.ortho_size
        } else {
            radius * 1.05
        };
        let back = radius * s.distance + 1.0;
        let eye = center - dir * back;
        // 光源接近竖直时换 Z 轴当 up，避免 look_at 退化
        let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        // glam 0.34 起相机辅助函数迁到 glam::camera::rh 命名空间：
        // 与 Camera 保持同一套右手系 + directx(NDC 深度 0..1) 约定，才和 wgpu 对齐
        let view = glam::camera::rh::view::look_at_mat4(eye, center, up);
        let far = back + radius * 2.0 + 1.0;
        let proj = glam::camera::rh::proj::directx::orthographic(
            -half, half, -half, half, 0.05, far,
        );
        proj * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 场景中心必须落在阴影相机 NDC 的深度容差内、x/y 约为 0。
    #[test]
    fn shadow_frames_scene_center() {
        let light = Light::default();
        let center = Vec3::new(2.0, 1.0, -3.0);
        let vp = light.shadow_view_projection(center, 5.0);
        let clip = vp * center.extend(1.0);
        let ndc = clip / clip.w;
        assert!(ndc.x.abs() < 1e-4, "ndc.x = {}", ndc.x);
        assert!(ndc.y.abs() < 1e-4, "ndc.y = {}", ndc.y);
        assert!(ndc.z >= 0.0 && ndc.z <= 1.0, "ndc.z = {}", ndc.z);
    }

    /// 包围球内的点必须落在正交视锥内；球外偏移点必须落出去。
    #[test]
    fn shadow_extents_follow_radius() {
        let light = Light::default();
        let center = Vec3::ZERO;
        let radius = 4.0;
        let vp = light.shadow_view_projection(center, radius);
        let inside = center + Vec3::new(radius * 0.9, 0.0, 0.0);
        let clip = vp * inside.extend(1.0);
        assert!(clip.x.abs() <= clip.w, "inside 应落在视锥内");
        let outside = center + Vec3::new(radius * 3.0, 0.0, 0.0);
        let clip = vp * outside.extend(1.0);
        assert!(clip.x.abs() > clip.w, "outside 应落在视锥外");
    }

    /// 固定 ortho_size 时半宽不再随半径变化。
    #[test]
    fn fixed_ortho_size_overrides_radius() {
        let mut light = Light::default();
        light.shadow.ortho_size = 1.0;
        let vp = light.shadow_view_projection(Vec3::ZERO, 10.0);
        let p = Vec3::new(2.0, 0.0, 0.0);
        let clip = vp * p.extend(1.0);
        assert!(clip.x.abs() > clip.w, "半径 2 的点应超出半宽 1 的视锥");
    }

    /// 坏参数被 sanitize 兜住，不产生 NaN。
    #[test]
    fn sanitize_handles_garbage() {
        let s = ShadowSettings {
            enabled: true,
            size: 0,
            ortho_size: -3.0,
            bias: f32::NAN,
            normal_bias: f32::INFINITY,
            distance: 0.0,
            strength: 4.0,
        }
        .sanitized();
        assert_eq!(s.size, 64);
        assert_eq!(s.ortho_size, 0.0);
        assert_eq!(s.bias, 0.0);
        assert!(s.normal_bias.is_finite());
        assert!(s.distance >= 0.05);
        assert_eq!(s.strength, 1.0);
        let vp = Light::default().shadow_view_projection(Vec3::ZERO, 2.0);
        assert!(vp.to_cols_array().iter().all(|v| v.is_finite()));
    }
}
