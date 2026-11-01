//! PBR 直接光照的渲染侧数据准备：材质 uniform 布局 + 物件级材质键。
//!
//! 与 `r3e_core::material` 的分工：
//!
//! * `r3e-core` 负责 BGDF 数学与参数收敛（纯 CPU、可单测、不依赖 wgpu）；
//! * 本模块负责把收敛后的材质打包成 GPU uniform，并给出「物件级材质覆盖」
//!   的可哈希键 —— 同一个网格用不同 metallic / roughness 的物件必须能分成
//!   不同的绘制批次，否则后画的物体会覆盖前面物件的材质参数。
//!
//! 这里的函数都是纯函数，不碰 wgpu 句柄，可直接单测。

use bytemuck::{Pod, Zeroable};
use r3e_core::mesh::MaterialData;
use r3e_core::SurfaceMaterial;

/// 材质 uniform：base color + PBR 标量 + 自发光。48 字节，16 字节对齐。
///
/// 布局必须与 `shader.wgsl` 的 `Material` 结构逐字段一致：
///
/// ```text
/// struct Material {
///     base_color: vec4<f32>,   // offset 0
///     params:     vec4<f32>,   // offset 16  x = metallic, y = roughness, z = has_tex
///     emissive:   vec4<f32>,   // offset 32  rgb = 自发光（线性 HDR），a 保留
/// };
/// ```
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct MaterialUniform {
    pub base_color: [f32; 4],
    /// x = metallic，y = roughness，z = 是否有贴图（0/1），w 保留。
    pub params: [f32; 4],
    /// rgb = 自发光，a 保留（固定 0）。
    pub emissive: [f32; 4],
}

impl Default for MaterialUniform {
    fn default() -> Self {
        Self::build([1.0, 1.0, 1.0, 1.0], 0.0, 0.6, [0.0; 3], false)
    }
}

impl MaterialUniform {
    /// 按收敛规则打包一份材质 uniform。
    ///
    /// `metallic` / `roughness` / `emissive` 统一过 [`SurfaceMaterial::sanitized`]：
    /// NaN 退回缺省、超界夹到合法区间，因此 GPU 侧拿到的一定是有限值
    /// （着色器仍会再夹一次粗糙度下限，用于防除零，属于第二道防线）。
    pub fn build(
        base_color: [f32; 4],
        metallic: f32,
        roughness: f32,
        emissive: [f32; 3],
        has_texture: bool,
    ) -> Self {
        let surface = SurfaceMaterial {
            metallic,
            roughness,
            emissive,
        }
        .sanitized();
        Self {
            base_color,
            params: [
                surface.metallic,
                surface.roughness,
                if has_texture { 1.0 } else { 0.0 },
                0.0,
            ],
            emissive: [
                surface.emissive[0],
                surface.emissive[1],
                surface.emissive[2],
                0.0,
            ],
        }
    }
}

/// 物件级材质的可哈希键。
///
/// 用 `f32::to_bits` 做键而不是近似比较：材质参数在写入场景前已被收敛，
/// 同一份材质在任何路径下都得到逐位相同的数值，因此按位比较既精确又便宜。
/// `-0.0` 会在收敛时被归一成 `0.0`，不会出现「两个等价键」。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SurfaceKey {
    bits: [u32; 5],
}

impl SurfaceKey {
    /// 由收敛后的材质构造键（内部会再收敛一次，幂等）。
    pub fn new(material: SurfaceMaterial) -> Self {
        let m = material.sanitized();
        Self {
            bits: [
                m.metallic.to_bits(),
                m.roughness.to_bits(),
                m.emissive[0].to_bits(),
                m.emissive[1].to_bits(),
                m.emissive[2].to_bits(),
            ],
        }
    }

    /// 还原成材质参数。
    pub fn material(self) -> SurfaceMaterial {
        SurfaceMaterial {
            metallic: f32::from_bits(self.bits[0]),
            roughness: f32::from_bits(self.bits[1]),
            emissive: [
                f32::from_bits(self.bits[2]),
                f32::from_bits(self.bits[3]),
                f32::from_bits(self.bits[4]),
            ],
        }
    }

    /// 自发光分量（资产材质没有自发光字段，自发光只来自物件级覆盖）。
    pub fn emissive(self) -> [f32; 3] {
        self.material().emissive
    }

    /// 把物件级材质叠加到资产材质上：**只覆盖 metallic / roughness**。
    ///
    /// base color 与贴图仍归资产 / 物件颜色管，避免一个材质参数把贴图染掉。
    pub fn overlay(self, base: &MaterialData) -> MaterialData {
        let m = self.material();
        MaterialData {
            metallic: m.metallic,
            roughness: m.roughness,
            ..base.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::material::{DEFAULT_ROUGHNESS, MAX_EMISSIVE};

    fn sample_asset() -> MaterialData {
        MaterialData {
            name: "asset-mat".to_string(),
            base_color: [0.2, 0.4, 0.6, 1.0],
            metallic: 0.1,
            roughness: 0.9,
            base_color_texture: Some((r3e_core::AssetId(7), 3)),
        }
    }

    /// uniform 布局：三个 vec4 紧邻，48 字节，无隐藏填充。
    #[test]
    fn uniform_layout_matches_shader() {
        assert_eq!(std::mem::size_of::<MaterialUniform>(), 48);
        assert_eq!(std::mem::align_of::<MaterialUniform>(), 4);
    }

    /// 非法参数在打包前就收敛，GPU 侧不会收到 NaN / inf。
    #[test]
    fn uniform_sanitizes_before_upload() {
        let u = MaterialUniform::build(
            [1.0, 1.0, 1.0, 1.0],
            f32::NAN,
            f32::INFINITY,
            [f32::NEG_INFINITY, 1e9, f32::NAN],
            true,
        );
        assert_eq!(u.params[0], 0.0, "metallic NaN → 0");
        assert_eq!(u.params[1], 1.0, "roughness +inf → 1");
        assert_eq!(u.params[2], 1.0, "有贴图标记为 1");
        assert_eq!(u.emissive[0], 0.0, "负无穷自发光归零");
        assert_eq!(u.emissive[1], MAX_EMISSIVE, "超亮自发光收到上限");
        assert_eq!(u.emissive[2], 0.0, "NaN 自发光归零");
        assert!(u.params.iter().all(|v| v.is_finite()));
        assert!(u.emissive.iter().all(|v| v.is_finite()));
    }

    /// 缺省材质与着色的默认值一致。
    #[test]
    fn uniform_default_matches_surface_default() {
        let u = MaterialUniform::default();
        let s = SurfaceMaterial::default();
        assert_eq!(u.params[0], s.metallic);
        assert_eq!(u.params[1], s.roughness);
        assert_eq!(u.params[2], 0.0);
        assert_eq!(u.emissive, [0.0, 0.0, 0.0, 0.0]);
    }

    /// 键往返：同一份材质得到同一个键，不同参数得到不同的键。
    #[test]
    fn surface_key_roundtrips_and_distinguishes() {
        let m = SurfaceMaterial::new(0.75, 0.25).with_emissive([0.5, 0.0, 0.0]);
        let key = SurfaceKey::new(m);
        assert_eq!(key.material(), m);
        assert_eq!(key, SurfaceKey::new(m));

        assert_ne!(key, SurfaceKey::new(SurfaceMaterial::new(0.75, 0.26)));
        assert_ne!(key, SurfaceKey::new(SurfaceMaterial::new(0.76, 0.25)));
        assert_ne!(key, SurfaceKey::new(m.with_emissive([0.5, 0.0, 0.1])));
    }

    /// `-0.0` 与 `0.0`、NaN 与缺省都必须收敛成同一个键，避免多出冗余绘制批次。
    #[test]
    fn surface_key_collapses_equivalent_inputs() {
        let zero = SurfaceKey::new(SurfaceMaterial::new(0.0, 0.5));
        let neg_zero = SurfaceKey::new(SurfaceMaterial::new(-0.0, 0.5));
        assert_eq!(zero, neg_zero);

        let nan = SurfaceKey::new(SurfaceMaterial {
            metallic: f32::NAN,
            roughness: DEFAULT_ROUGHNESS,
            emissive: [0.0; 3],
        });
        assert_eq!(nan, SurfaceKey::new(SurfaceMaterial::default()));
    }

    /// 物件级材质只覆盖 metallic / roughness，资产的颜色与贴图保持不变。
    #[test]
    fn overlay_keeps_asset_color_and_texture() {
        let asset = sample_asset();
        let key = SurfaceKey::new(SurfaceMaterial::new(1.0, 0.1).with_emissive([2.0, 0.0, 0.0]));
        let merged = key.overlay(&asset);
        assert_eq!((merged.metallic, merged.roughness), (1.0, 0.1));
        assert_eq!(merged.base_color, asset.base_color);
        assert_eq!(merged.base_color_texture, asset.base_color_texture);
        assert_eq!(merged.name, asset.name);
        assert_eq!(key.emissive(), [2.0, 0.0, 0.0]);
    }

    /// 覆盖后的材质再打包成 uniform，数值与键上的完全一致。
    #[test]
    fn overlay_then_build_keeps_values() {
        let asset = MaterialData::default();
        let key = SurfaceKey::new(SurfaceMaterial::new(0.3, 0.7));
        let merged = key.overlay(&asset);
        let u = MaterialUniform::build(
            merged.base_color,
            merged.metallic,
            merged.roughness,
            key.emissive(),
            false,
        );
        assert_eq!(u.params, [0.3, 0.7, 0.0, 0.0]);
    }
}
