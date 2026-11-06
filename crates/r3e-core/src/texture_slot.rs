//! 贴图槽位分类：把 FBX / glTF 里五花八门的槽位名收敛成「这张图是干什么用的」。
//!
//! 法线 / 粗糙度 / AO 三类贴图和 base color 的差别不只是名字：
//!
//! * **色彩空间不同**：base color 是 sRGB，采样时要线性化；法线 / 粗糙度 / AO 是
//!   线性数据（法线图是 `n * 0.5 + 0.5` 的编码，粗糙度、AO 本身就是 0..1 标量），
//!   若按 sRGB 采样会被 gamma 曲线掰弯，法线方向、粗糙度全部失真。
//! * **数值通道不同**：glTF 的 `metallicRoughness` 图把 roughness 放 G、metallic 放 B；
//!   AO 图放 R；法线图放 RGB。
//!
//! 所以导入层挑贴图时按本模块给槽位归类，渲染层再据此决定上传格式（见
//! `r3e-render::renderer` 的 `upload_texture`），两边共用同一套判据。

/// 贴图用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureSlot {
    /// 基础色（sRGB）。
    BaseColor,
    /// 切线空间法线图（线性，`n * 0.5 + 0.5`）。
    Normal,
    /// 独立粗糙度图（线性，取 R 通道）。
    Roughness,
    /// glTF 合并的金属度-粗糙度图（线性，G = roughness，B = metallic）。
    MetallicRoughness,
    /// 环境光遮蔽图（线性，取 R 通道）。
    Occlusion,
    /// 自发光图（sRGB）。
    Emissive,
    /// 认不出来的槽位（含 FBX 的 bump 高度图：用法线方式采样会得到错误方向，故不归类）。
    Unknown,
}

impl TextureSlot {
    /// 该用途的贴图是否应按**线性**（非 sRGB）格式上传。
    ///
    /// 法线 / 粗糙度 / AO / 合并图都是数据图，只有 base color 与自发光是颜色图。
    pub const fn is_linear_data(self) -> bool {
        matches!(
            self,
            Self::Normal | Self::Roughness | Self::MetallicRoughness | Self::Occlusion
        )
    }

    /// 该用途的贴图是否参与材质着色（`Unknown` 只出现在导入统计里）。
    pub const fn is_pbr_map(self) -> bool {
        matches!(
            self,
            Self::Normal | Self::Roughness | Self::MetallicRoughness | Self::Occlusion
        )
    }
}

/// 归一化槽位名：小写化并去掉常见分隔符，方便子串判断。
fn normalized(slot: &str) -> String {
    slot.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' ' | '.' | '|' | ':' | '/'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 按槽位名判定贴图用途。
///
/// 顺序有意义：`metallicroughness` 同时含 "metallic" 与 "rough"，
/// 必须先判合并图再判单通道粗糙度；`bump`（高度图）刻意不当作法线。
pub fn classify_slot(slot: &str) -> TextureSlot {
    let s = normalized(slot);
    if s.is_empty() {
        return TextureSlot::Unknown;
    }
    if s.contains("normal") && !s.contains("bump") {
        return TextureSlot::Normal;
    }
    if s.contains("metallic") && s.contains("rough") {
        return TextureSlot::MetallicRoughness;
    }
    if s.contains("rough") {
        return TextureSlot::Roughness;
    }
    if s.contains("occlusion") || s.contains("ambientocclusion") || ends_with_ao(&s) {
        return TextureSlot::Occlusion;
    }
    if s.contains("emissive") || s.contains("emission") {
        return TextureSlot::Emissive;
    }
    if s.contains("diffuse")
        || s.contains("basecolor")
        || s.contains("albedo")
        || s.contains("color")
        || s.contains("colour")
    {
        return TextureSlot::BaseColor;
    }
    TextureSlot::Unknown
}

/// 形如 `ao` / `texao` / `mapao` 的末尾缩写；`shadow` 之类的词不会命中。
fn ends_with_ao(s: &str) -> bool {
    s.ends_with("ao")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FBX 常见槽位名（含 Maya / Max 前缀）都要认对。
    #[test]
    fn classifies_fbx_slot_names() {
        let cases = [
            ("DiffuseColor", TextureSlot::BaseColor),
            ("Maya|baseColor", TextureSlot::BaseColor),
            ("albedo", TextureSlot::BaseColor),
            ("NormalMap", TextureSlot::Normal),
            ("Maya|normalCamera", TextureSlot::Normal),
            ("normal", TextureSlot::Normal),
            ("Maya|roughness", TextureSlot::Roughness),
            ("roughnessMap", TextureSlot::Roughness),
            ("Maya|metallicRoughness", TextureSlot::MetallicRoughness),
            ("Ambient_Occlusion", TextureSlot::Occlusion),
            ("Maya|ambientOcclusion", TextureSlot::Occlusion),
            ("AO", TextureSlot::Occlusion),
            ("tex_ao", TextureSlot::Occlusion),
        ];
        for (slot, expected) in cases {
            assert_eq!(classify_slot(slot), expected, "槽位 {slot}");
        }
    }

    /// bump 是高度图，不按法线处理；认不出的槽位也不能误判成 base color。
    #[test]
    fn rejects_ambiguous_slots() {
        for slot in ["Maya|bump", "bump", "height", "shadow", "specular", ""] {
            assert_eq!(classify_slot(slot), TextureSlot::Unknown, "槽位 {slot}");
        }
    }

    /// 色彩空间口径：只有颜色图走 sRGB，数据图必须线性。
    #[test]
    fn data_maps_are_linear() {
        assert!(!TextureSlot::BaseColor.is_linear_data());
        assert!(!TextureSlot::Emissive.is_linear_data());
        for slot in [
            TextureSlot::Normal,
            TextureSlot::Roughness,
            TextureSlot::MetallicRoughness,
            TextureSlot::Occlusion,
        ] {
            assert!(slot.is_linear_data(), "{slot:?} 必须是线性数据图");
            assert!(slot.is_pbr_map(), "{slot:?} 应参与 PBR 着色");
        }
        assert!(!TextureSlot::Unknown.is_pbr_map());
    }
}
