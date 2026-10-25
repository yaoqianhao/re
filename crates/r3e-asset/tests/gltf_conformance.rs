//! glTF / `.glb` 一致性测试（S3/S4 验收）。
//!
//! 用例里的 glb 由测试自己按规范拼装，不依赖任何外部样本：
//! 一个单三角形 + 一个材质 + 一段旋转动画，用来锁死三件事——
//! 1. 换轴：glTF 的 `-Z front` 变成引擎的 `+Z front`（位置与法线的 z 都取反）；
//! 2. 绕序：镜像导致的行列式为负，`bake_transform` 必须自动翻转索引；
//! 3. 归一化：导入结果必须通过 `audit_entry` 的 AIR 不变量审计。

use std::path::{Path, PathBuf};

use r3e_asset::{import_glb, import_gltf, importer::audit_entry};

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

/// 拼一个最小 `.glb`：单三角形（法线 +Z）、深蓝色材质、一段 1 秒旋转动画。
fn write_fixture(dir: &Path) -> PathBuf {
    // ---- 二进制块：POSITION / NORMAL / INDICES / TIME / ROTATION ----
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];
    let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
    let indices: [u16; 3] = [0, 1, 2];
    let times: [f32; 2] = [0.0, 1.0];
    let rotations: [[f32; 4]; 2] = [[0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0]];

    let mut bin: Vec<u8> = Vec::new();

    let pos: Vec<f32> = positions.iter().flatten().copied().collect();
    let nor: Vec<f32> = normals.iter().flatten().copied().collect();
    let rot: Vec<f32> = rotations.iter().flatten().copied().collect();

    let (pos_off, pos_len) = push_f32(&mut bin, &pos);
    let (nor_off, nor_len) = push_f32(&mut bin, &nor);
    let idx_bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (idx_off, idx_len) = push_bytes(&mut bin, &idx_bytes);
    let (time_off, time_len) = push_f32(&mut bin, &times);
    let (rot_off, rot_len) = push_f32(&mut bin, &rot);
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_len = bin.len();

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
          "attributes": {{ "POSITION": 0, "NORMAL": 1 }},
          "indices": 2,
          "material": 0
        }}
      ]
    }}
  ],
  "materials": [
    {{
      "name": "Mat",
      "pbrMetallicRoughness": {{
        "baseColorFactor": [0.1, 0.2, 0.9, 1.0],
        "metallicFactor": 0.0,
        "roughnessFactor": 0.8
      }}
    }}
  ],
  "animations": [
    {{
      "name": "Spin",
      "channels": [{{ "sampler": 0, "target": {{ "node": 0, "path": "rotation" }} }}],
      "samplers": [{{ "input": 3, "output": 4, "interpolation": "LINEAR" }}]
    }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
       "min": [0.0, 0.0, 1.0], "max": [1.0, 1.0, 1.0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR" }},
    {{ "bufferView": 3, "componentType": 5126, "count": 2, "type": "SCALAR",
       "min": [0.0], "max": [1.0] }},
    {{ "bufferView": 4, "componentType": 5126, "count": 2, "type": "VEC4" }}
  ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": {pos_off}, "byteLength": {pos_len} }},
    {{ "buffer": 0, "byteOffset": {nor_off}, "byteLength": {nor_len} }},
    {{ "buffer": 0, "byteOffset": {idx_off}, "byteLength": {idx_len} }},
    {{ "buffer": 0, "byteOffset": {time_off}, "byteLength": {time_len} }},
    {{ "buffer": 0, "byteOffset": {rot_off}, "byteLength": {rot_len} }}
  ],
  "buffers": [{{ "byteLength": {bin_len} }}]
}}"#
    );

    // ---- 拼 glb：header + JSON chunk + BIN chunk ----
    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }

    let total = 12 + 8 + json_bytes.len() + 8 + bin_len;
    let mut glb: Vec<u8> = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin_len as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);

    let path = dir.join("air_conformance.glb");
    std::fs::write(&path, &glb).expect("写 glb 失败");
    path
}

#[test]
fn import_mirrors_z_and_flips_winding() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = write_fixture(dir);
    let entry = import_glb(&path).expect("导入 glb 失败");
    let mesh = entry.meshes.first().expect("应有 1 个网格");

    // 换轴：glTF +Z 前 → 引擎 +Z 前，z 取反（这里原始 z=1 → -1）
    for p in &mesh.positions {
        assert!((p[2] + 1.0).abs() < 1e-6, "z 未镜像: {p:?}");
    }
    for n in &mesh.normals {
        assert!((n[2] + 1.0).abs() < 1e-6, "法线 z 未镜像: {n:?}");
    }

    // 镜像使行列式为负，索引必须自动翻转（[0,1,2] → [2,1,0]）
    assert_eq!(mesh.indices, vec![2, 1, 0]);

    // 材质 / 动画也要落到 AIR
    assert_eq!(entry.materials.len(), 1);
    assert_eq!(entry.animations.len(), 1);
}

#[test]
fn import_is_deterministic_and_audited() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let path = write_fixture(dir);
    let a = import_glb(&path).expect("首次导入失败");
    let b = import_glb(&path).expect("二次导入失败");

    assert!(audit_entry(&a).is_empty(), "AIR 审计不应有违规");
    assert_eq!(a.meshes.len(), b.meshes.len());
    for (x, y) in a.meshes.iter().zip(&b.meshes) {
        assert_eq!(x.positions, y.positions);
        assert_eq!(x.indices, y.indices);
        assert_eq!(x.uvs, y.uvs);
    }
}

#[test]
fn gltf_and_glb_yield_the_same_air() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let glb_path = write_fixture(dir);
    let glb_entry = import_glb(&glb_path).expect("导入 glb 失败");

    // 把同一个 glb 拆成 `.gltf` + 外部 `.bin`，模拟「同一模型的两种容器」。
    let bytes = std::fs::read(&glb_path).expect("读 glb 失败");
    let json_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let json = String::from_utf8_lossy(&bytes[20..20 + json_len]).to_string();
    let bin_head = 20 + json_len;
    let bin_len = u32::from_le_bytes([
        bytes[bin_head],
        bytes[bin_head + 1],
        bytes[bin_head + 2],
        bytes[bin_head + 3],
    ]) as usize;
    let bin = &bytes[bin_head + 8..bin_head + 8 + bin_len];

    // 给 buffer 补上外部 `.bin` 引用（glb 里是内嵌块，gltf 必须显式给 uri）。
    let json = json.replace(
        "\"buffers\": [{",
        "\"buffers\": [{\"uri\": \"air_conformance.bin\", ",
    );
    assert!(json.contains("air_conformance.bin"), "gltf 未写入 buffer uri");
    let gltf_path = dir.join("air_conformance.gltf");
    std::fs::write(&gltf_path, json.as_bytes()).expect("写 gltf 失败");
    std::fs::write(dir.join("air_conformance.bin"), bin).expect("写 bin 失败");

    let gltf_entry = import_gltf(&gltf_path).expect("导入 gltf 失败");

    // 容器不同，AIR 必须逐字段一致——这是「存储统一格式」的硬约束。
    assert_eq!(gltf_entry.meshes.len(), glb_entry.meshes.len());
    for (a, b) in gltf_entry.meshes.iter().zip(&glb_entry.meshes) {
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.normals, b.normals);
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.material_parts.len(), b.material_parts.len());
    }
    assert_eq!(gltf_entry.materials.len(), glb_entry.materials.len());
    assert_eq!(gltf_entry.animations.len(), glb_entry.animations.len());
    assert!(audit_entry(&gltf_entry).is_empty());
}
