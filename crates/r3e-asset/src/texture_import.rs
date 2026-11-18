//! 独立贴图导入通道（M1-S4-A）。
//!
//! 场景材质（`materials[]`）里写的是**裸图片路径**，不是 FBX/glTF 资产。这条通道把
//! 一张独立图片变成「一份只含一条 `textures` 的资产条目」，于是渲染侧可以照旧用
//! [`r3e_core::mesh::MeshProvider::texture`] 取图，**取图路径一行不改**。
//!
//! 三条硬约定：
//!
//! - **id 由路径决定**（[`AssetId::from_path`]）：同一张图不管你导入几次都拿到同一个
//!   `AssetId`，天然去重；重复导入只做"命中即返回"，不重复解码。
//! - **解码复用既有栈**：走 [`crate::decode_image_bytes`]（`image::load_from_memory`
//!   + 扩展名兜底），不新引依赖、不另写一份格式嗅探。
//! - **槽位靠文件名归类**：交给 [`classify_slot`]，与 FBX 导入同一套判定。

use std::path::Path;

use r3e_core::mesh::MaterialData;
use r3e_core::texture_slot::{classify_slot, TextureSlot};
use r3e_core::AssetId;

use crate::{AssetEntry, AssetError, AssetLibrary, MaterialInfo, TextureInfo};

/// 一次贴图导入的结果。
#[derive(Debug, Clone)]
pub struct ImportedTexture {
    /// 该贴图所属资产（= 路径散列，跨进程稳定）。
    pub asset: AssetId,
    /// 贴图在该资产 `textures` 表里的下标。独立图片恒为 0。
    pub index: u32,
    /// 按文件名判定的槽位（albedo / normal / …）。
    pub slot: TextureSlot,
    /// 归一化后的源路径（写进 `AssetEntry::source_path`）。
    pub source_path: String,
    /// 上一帧就已经在库里时复用，不是这次解码出来的。
    pub reused: bool,
}

impl ImportedTexture {
    /// 直接当 `MaterialData` 的纹理槽值用。
    pub fn handle(&self) -> (AssetId, u32) {
        (self.asset, self.index)
    }
}

/// 把一张独立图片导入资产库，返回可直接塞进材质槽的 `(AssetId, u32)`。
///
/// 同一路径重复调用不会重复解码：命中库里已有条目直接返回 `reused = true`。
/// 文件不存在 / 不是可识别图片都报 [`AssetError::Load`]——上层据此报
/// `material.texture_missing` 之外的「解码失败」，两者不要混。
pub fn import_texture(
    lib: &mut AssetLibrary,
    path: impl AsRef<Path>,
) -> Result<ImportedTexture, AssetError> {
    let path = path.as_ref();
    // 反斜杠统一成正斜杠：`AssetId` 是「路径字符串的散列」，而
    // `r3e_core::resolve_texture_path` 产出的散列输入恒为 `/` 分隔。两处口径必须
    // 完全一致，否则同一张图会拿到两个 id —— 文档层算出一个、运行期导入另一个，
    // 渲染层取不到像素就静默变白图，是最难定位的一类「材质没生效」。
    let display = path.to_string_lossy().replace('\\', "/");
    let asset = AssetId::from_path(&display);
    let slot = slot_of(path);

    // 命中去重：同 id 已在库里就是同一张图，连解码都不用做。
    if let Some(entry) = lib.get(asset) {
        if let Some(first) = entry.textures.first() {
            return Ok(ImportedTexture {
                asset,
                index: first.index,
                slot,
                source_path: entry.source_path.clone(),
                reused: true,
            });
        }
    }

    let bytes = std::fs::read(path).map_err(|err| AssetError::Load {
        path: display.clone(),
        message: format!("读贴图失败：{err}"),
    })?;
    let image =
        crate::decode_image_bytes(&bytes, path.to_str()).ok_or_else(|| AssetError::Load {
            path: display.clone(),
            message: "不是可识别的图片格式（png / jpg / tga / bmp）".to_string(),
        })?;
    let (width, height) = (image.width, image.height);

    let info = TextureInfo {
        index: 0,
        element_id: 0,
        slot: slot_name(slot).to_string(),
        filename: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        relative_filename: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        absolute_filename: display.clone(),
        embedded: false,
        content_bytes: bytes.len(),
        has_file: true,
        uv_set: "UVMap".to_string(),
        image: Some(image),
    };

    let entry = AssetEntry {
        id: asset,
        source_path: display.clone(),
        meshes: Vec::new(),
        materials: Vec::new(),
        textures: vec![info],
        nodes: Vec::new(),
        animations: Vec::new(),
        skins: Vec::new(),
    };
    lib.insert(entry);

    log::debug!(
        "asset: 贴图导入 {display}（{width}x{height}，槽位 {:?}）",
        slot
    );

    Ok(ImportedTexture {
        asset,
        index: 0,
        slot,
        source_path: display,
        reused: false,
    })
}

/// 按路径判槽位。文件名里没有可识别关键词时归 `BaseColor`？
///
/// 不——归 [`TextureSlot::Unknown`]，让调用方显式决定，避免"猜错了把 AO 当 albedo"。
///
/// 只喂**主干名**给 [`classify_slot`]：扩展名会让归一化结果变成 `c_ao.png` →
/// `caopng`，末尾缩写判定（`ends_with_ao`）与 `mr` 之类的短名全部失效。
fn slot_of(path: &Path) -> TextureSlot {
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let by_name = classify_slot(&stem);
    if by_name != TextureSlot::Unknown {
        return by_name;
    }
    // 缩写兜底：`mr` / `orm` / `ao` 这类短名不在关键词表里，但贴图工作流里出现率
    // 极高（Substance / Painter 导出默认就是 `xxx_mr`、`xxx_ao`）。从右往左取第一个
    // 命中，保证 `c_mr`、`rock_orm_v2` 都能归位；`macro` 之类的整词不会误命中。
    let lowered = stem.to_lowercase();
    let tokens: Vec<&str> = lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    for token in tokens.iter().rev() {
        match *token {
            "mr" | "orm" | "mrao" | "rm" => return TextureSlot::MetallicRoughness,
            "ao" => return TextureSlot::Occlusion,
            _ => {}
        }
    }
    TextureSlot::Unknown
}

fn slot_name(slot: TextureSlot) -> &'static str {
    match slot {
        TextureSlot::BaseColor => "BaseColor",
        TextureSlot::Normal => "NormalMap",
        TextureSlot::Roughness => "Roughness",
        TextureSlot::MetallicRoughness => "MetallicRoughness",
        TextureSlot::Occlusion => "Occlusion",
        TextureSlot::Emissive => "Emissive",
        TextureSlot::Unknown => "Unknown",
    }
}

/// 把一条资产材质记录装配成渲染层直吃的 [`MaterialData`]。
///
/// 独立图片这条通道本身不产出材质，但它是「材质 → 渲染数据」的唯一装配点的邻居：
/// `AssetLibrary::material` 只留一行委派，`lib.rs` 也不会为了四个 `map(|t| (asset, t))`
/// 继续变长（该文件挂在体量门禁的债务白名单上）。
pub(crate) fn material_data(asset: AssetId, info: &MaterialInfo) -> MaterialData {
    MaterialData {
        name: info.name.clone(),
        base_color: info.base_color,
        metallic: info.metallic,
        roughness: info.roughness,
        base_color_texture: info.base_color_texture.map(|t| (asset, t)),
        normal_texture: info.normal_texture.map(|t| (asset, t)),
        roughness_texture: info.roughness_texture.map(|t| (asset, t)),
        occlusion_texture: info.occlusion_texture.map(|t| (asset, t)),
        // 网格资产暂不解析自发光贴图（glTF / FBX 的该槽位还没进 `MaterialInfo`），
        // uv 也不带平铺因子 —— 这两项由场景材质的「路径 → AssetId」通道提供。
        emissive_texture: None,
        uv_scale: [1.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张 2x2 的合法 PNG（直接用 crate 自己的写盘工具，避免引测试夹具）。
    fn png(path: &Path) {
        let px = [255u8, 0, 0, 255].repeat(4);
        crate::write_png_rgba(path, 2, 2, &px).expect("写 PNG 应当成功");
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "r3e_s4a_tex_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn import_assigns_stable_id_and_decodes_pixels() {
        let dir = tmp("basic");
        let path = dir.join("brick_albedo.png");
        png(&path);

        let mut lib = AssetLibrary::new();
        let one = import_texture(&mut lib, &path).expect("导入应当成功");
        assert_eq!(one.index, 0);
        assert!(!one.reused);
        assert_eq!(
            one.slot,
            TextureSlot::BaseColor,
            "文件名含 albedo 应判 base color"
        );
        assert_eq!(one.asset, AssetId::from_path(&path.to_string_lossy()));

        let entry = lib.get(one.asset).expect("资产应当入库");
        assert_eq!(entry.textures.len(), 1);
        let image = entry.textures[0].image.as_ref().expect("像素应当解码出来");
        assert_eq!((image.width, image.height), (2, 2));
        assert!(entry.meshes.is_empty(), "贴图资产不该带网格");

        // 同路径再导一次：命中复用，不新增条目。
        let again = import_texture(&mut lib, &path).expect("重复导入应当成功");
        assert!(again.reused, "同路径必须复用");
        assert_eq!(again.asset, one.asset);
        assert_eq!(lib.len(), 1, "同一张图只能有一条资产");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_missing_file_and_garbage() {
        let dir = tmp("bad");
        let mut lib = AssetLibrary::new();

        let err = import_texture(&mut lib, dir.join("nope.png")).expect_err("缺文件必须失败");
        assert!(matches!(err, AssetError::Load { .. }), "缺文件是 Load");

        let junk = dir.join("junk_normal.png");
        std::fs::write(&junk, b"not an image").unwrap();
        let err = import_texture(&mut lib, &junk).expect_err("非图片必须失败");
        assert!(matches!(err, AssetError::Load { .. }), "解码失败是 Load");
        assert!(lib.is_empty(), "失败的导入不得入库");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
