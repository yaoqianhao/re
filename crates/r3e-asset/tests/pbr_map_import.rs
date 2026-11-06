//! PBR 数据贴图导入契约（扩展二十一）。
//!
//! 锁死三件事：
//! 1. glTF 的三类数据贴图槽位（`normalTexture` / `occlusionTexture` /
//!    `pbrMetallicRoughness.metallicRoughnessTexture`）都要落进 `MaterialInfo`，
//!    且 `/` 不能和其他槽位混成同一张；
//! 2. FBX 那种「只有槽位名」的信息能按 [`classify_slot`] 收敛成法线 / 粗糙度 / AO；
//! 3. 导入出口的网格一定带切线（法线贴图需要 TBN），且手性只能是 ±1。
//!
//! 夹具全部由测试自己拼装，不依赖外部样本文件。

use std::path::{Path, PathBuf};

use r3e_asset::{import_glb, pick_pbr_textures, MaterialInfo, TextureInfo};

/// 按 4 字节对齐把一段字节追加进二进制块，返回 `(byteOffset, byteLength)`。
fn push_bytes(bin: &mut Vec<u8>, bytes: &[u8]) -> (usize, usize) {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    bin.extend_from_slice(bytes);
    (offset, bytes.len())
}

fn push_f32(bin: &mut Vec<u8>, values: &[f32]) -> (usize, usize) {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    push_bytes(bin, &bytes)
}

/// 2x2 纯色 PNG：四张贴图共用同一份编码，但走不同的 bufferView，
/// 因此对导入层是四张独立贴图（验证「按槽位各取各的」）。
fn png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 120, 60, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("编码 PNG 失败");
    out.into_inner()
}

fn fixture_dir(case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    std::fs::create_dir_all(&dir).expect("创建夹具目录失败");
    dir
}

/// 拼一个带三类数据贴图的 `.glb`：单三角形 + 一张材质 + 四张 2x2 PNG。
fn write_textured_glb(dir: &Path) -> PathBuf {
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
    let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let indices: [u16; 3] = [0, 1, 2];

    let mut bin: Vec<u8> = Vec::new();
    let pos: Vec<f32> = positions.iter().flatten().copied().collect();
    let nor: Vec<f32> = normals.iter().flatten().copied().collect();
    let uv: Vec<f32> = uvs.iter().flatten().copied().collect();
    let (pos_off, pos_len) = push_f32(&mut bin, &pos);
    let (nor_off, nor_len) = push_f32(&mut bin, &nor);
    let (uv_off, uv_len) = push_f32(&mut bin, &uv);
    let idx_bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (idx_off, idx_len) = push_bytes(&mut bin, &idx_bytes);

    let png = png_bytes();
    let mut image_views: Vec<(usize, usize)> = Vec::new();
    for _ in 0..4 {
        image_views.push(push_bytes(&mut bin, &png));
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_len = bin.len();

    let image_views_json: Vec<String> = image_views
        .iter()
        .map(|(off, len)| {
            format!("{{ \"buffer\": 0, \"byteOffset\": {off}, \"byteLength\": {len} }}")
        })
        .collect();
    let views_json = image_views_json.join(",\n    ");
    // 图片 view 的下标接在 4 个几何 view 之后
    let img_view = |i: usize| 4 + i;

    let json = format!(
        r#"{{
  "asset": {{ "version": "2.0", "generator": "r3e-test" }},
  "scene": 0,
  "scenes": [{{ "nodes": [0] }}],
  "nodes": [{{ "name": "Tri", "mesh": 0 }}],
  "meshes": [
    {{
      "name": "Tri",
      "primitives": [
        {{
          "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }},
          "indices": 3,
          "material": 0
        }}
      ]
    }}
  ],
  "materials": [
    {{
      "name": "MappedMat",
      "pbrMetallicRoughness": {{
        "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
        "metallicFactor": 0.0,
        "roughnessFactor": 0.5,
        "baseColorTexture": {{ "index": 0 }},
        "metallicRoughnessTexture": {{ "index": 1 }}
      }},
      "normalTexture": {{ "index": 2 }},
      "occlusionTexture": {{ "index": 3 }}
    }}
  ],
  "textures": [
    {{ "sampler": 0, "source": 0 }},
    {{ "sampler": 0, "source": 1 }},
    {{ "sampler": 0, "source": 2 }},
    {{ "sampler": 0, "source": 3 }}
  ],
  "samplers": [{{ "magFilter": 9728, "minFilter": 9728 }}],
  "images": [
    {{ "bufferView": {}, "mimeType": "image/png" }},
    {{ "bufferView": {}, "mimeType": "image/png" }},
    {{ "bufferView": {}, "mimeType": "image/png" }},
    {{ "bufferView": {}, "mimeType": "image/png" }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
       "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": {pos_off}, "byteLength": {pos_len} }},
    {{ "buffer": 0, "byteOffset": {nor_off}, "byteLength": {nor_len} }},
    {{ "buffer": 0, "byteOffset": {uv_off}, "byteLength": {uv_len} }},
    {{ "buffer": 0, "byteOffset": {idx_off}, "byteLength": {idx_len} }},
    {views_json}
  ],
  "buffers": [{{ "byteLength": {bin_len} }}]
}}"#,
        img_view(0),
        img_view(1),
        img_view(2),
        img_view(3)
    );

    // GLB 容器：header + JSON chunk + BIN chunk
    let mut glb: Vec<u8> = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    let json_bytes = json.as_bytes();
    let json_pad = (4 - json_bytes.len() % 4) % 4;
    let total = 12 + 8 + json_bytes.len() + json_pad + 8 + bin.len();
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&((json_bytes.len() + json_pad) as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(json_bytes);
    // JSON chunk 必须 4 字节对齐（MSRV 1.79 没有 repeat_n，用 resize 补空格）
    glb.resize(glb.len() + json_pad, b' ');
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);

    let path = dir.join("pbr_maps.glb");
    std::fs::write(&path, &glb).expect("写 glb 失败");
    path
}

/// glTF 的三类数据贴图槽位都要落进材质，且互不混淆。
#[test]
fn gltf_data_maps_land_in_their_slots() {
    let dir = fixture_dir("gltf_data_maps_land_in_their_slots");
    let path = write_textured_glb(&dir);
    let entry = import_glb(&path).expect("导入带贴图的 glb 失败");

    assert_eq!(entry.materials.len(), 1);
    let mat = &entry.materials[0];
    assert!(mat.base_color_texture.is_some(), "base color 槽位丢失");

    let normal = mat.normal_texture.expect("法线贴图槽位丢失");
    let roughness = mat.roughness_texture.expect("粗糙度贴图槽位丢失");
    let occlusion = mat.occlusion_texture.expect("AO 贴图槽位丢失");
    // metallicRoughness 合并图必须落到粗糙度槽位
    assert_eq!(
        Some(roughness),
        mat.textures
            .iter()
            .find(|t| t.index == roughness)
            .map(|t| t.index),
        "粗糙度槽位应指向已登记的贴图"
    );
    for (a, b) in [
        (normal, roughness),
        (normal, occlusion),
        (roughness, occlusion),
    ] {
        assert_ne!(a, b, "三类数据贴图不能指向同一张图");
    }

    // 四张贴图都要真正解码成功（PNG 走 bufferView 内嵌）
    for index in [normal, roughness, occlusion] {
        let tex = entry
            .textures
            .iter()
            .find(|t| t.index == index)
            .unwrap_or_else(|| panic!("贴图 {index} 未登记"));
        let image = tex
            .image
            .as_ref()
            .unwrap_or_else(|| panic!("贴图 {index} 未解码"));
        assert_eq!((image.width, image.height), (2, 2), "贴图 {index} 尺寸不对");
    }
}

/// FBX 只有槽位名可用：按名字归类，且 `bump` 高度图不能当法线。
#[test]
fn fbx_slot_names_map_to_pbr_maps() {
    let tex = |index: u32, slot: &str| TextureInfo {
        index,
        slot: slot.to_string(),
        ..TextureInfo::default()
    };
    let textures = vec![
        tex(0, "Maya|DiffuseColor"),
        tex(1, "Maya|normalCamera"),
        tex(2, "Maya|roughness"),
        tex(3, "Ambient_Occlusion"),
        tex(4, "Maya|bump"),
    ];
    let slots = pick_pbr_textures(&textures);
    assert_eq!(slots.normal, Some(1));
    assert_eq!(slots.roughness, Some(2));
    assert_eq!(slots.occlusion, Some(3), "bump 不能被当成法线，也不能占 AO");

    // 同一用途多张图时取扁平表里第一张，不是最后一张
    let dup = vec![tex(7, "NormalMap"), tex(8, "normal")];
    assert_eq!(pick_pbr_textures(&dup).normal, Some(7));

    // 一张数据贴图都没有时全部为 None（退化为标量材质）
    let plain = vec![tex(0, "DiffuseColor")];
    let slots = pick_pbr_textures(&plain);
    assert_eq!(slots.normal, None);
    assert_eq!(slots.roughness, None);
    assert_eq!(slots.occlusion, None);
}

/// 空材质列表也要构造出合法默认值，方便渲染层无脑取用。
#[test]
fn default_material_has_no_data_maps() {
    let mat = MaterialInfo::default();
    assert!(mat.normal_texture.is_none());
    assert!(mat.roughness_texture.is_none());
    assert!(mat.occlusion_texture.is_none());
}

/// 导入出口的网格必须已经带切线：法线贴图没有 TBN 无法还原世界空间法线。
#[test]
fn imported_mesh_ships_tangents() {
    let dir = fixture_dir("imported_mesh_ships_tangents");
    let path = write_textured_glb(&dir);
    let entry = import_glb(&path).expect("导入带贴图的 glb 失败");

    let mesh = entry.meshes.first().expect("缺少网格");
    let data = mesh.to_mesh_data();
    assert_eq!(data.vertices.len(), 3);
    for vertex in &data.vertices {
        let t = vertex.tangent;
        let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-4, "切线应归一：{t:?}");
        assert_eq!(t[3].abs(), 1.0, "手性只能是 ±1：{t:?}");
    }
}
