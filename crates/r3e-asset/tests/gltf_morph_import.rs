//! glTF 形变形状（morph target / blend shape）导入测试（扩展三十六）。
//!
//! 夹具同样自己按规范拼 `.glb`，不依赖外部样本。一个单三角形网格挂两个形状：
//! * 形状 0 只动位置；
//! * 形状 1 同时带位置与法线增量。
//!
//! 同一个文件的第二个网格**不带** `targets`，用来锁死「没有形状的网格不会被凭空
//! 造出一组零增量」。
//!
//! 锁死三件事：
//! 1. `targets` 的顺序就是脚本权重下标（`morph_set(name, t, w)` 的 `t`）；
//! 2. 增量必须跟着网格一起过换轴（glTF 的 `-Z front` → 引擎 `+Z front`，z 取反）；
//! 3. `MeshAsset::to_mesh_data` 必须把形状带到运行时 `MeshData::morph` 上，
//!    且长度自洽到能通过 `MorphSet::validate_against`。

use std::path::{Path, PathBuf};

use r3e_asset::import_glb;

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

/// 每个用例独占一个夹具目录（并行执行时避免互相覆盖同名文件）。
fn fixture_dir(case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    std::fs::create_dir_all(&dir).expect("创建夹具目录失败");
    dir
}

/// 拼一个带两个形变形状的 `.glb`：网格 0 有 2 个形状，网格 1 没有。
fn write_morph_glb(dir: &Path) -> PathBuf {
    // 基：单三角形，朝 +Z（glTF 空间）。
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];
    let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
    let indices: [u16; 3] = [0, 1, 2];

    // 形状 0：只抬第一个顶点（z 方向）。换轴后 z 必须变号。
    let shape0_pos: [[f32; 3]; 3] = [[0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
    // 形状 1：三个顶点都沿 x 张开，并给一点法线增量（验证法线增量不归一化）。
    let shape1_pos: [[f32; 3]; 3] = [[-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.25, 0.0]];
    let shape1_nrm: [[f32; 3]; 3] = [[0.0, 0.0, 0.25], [0.0, 0.0, 0.25], [0.0, 0.0, 0.25]];

    let mut bin: Vec<u8> = Vec::new();
    let (pos_off, pos_len) = push_f32(
        &mut bin,
        &positions.iter().flatten().copied().collect::<Vec<_>>(),
    );
    let (nor_off, nor_len) = push_f32(
        &mut bin,
        &normals.iter().flatten().copied().collect::<Vec<_>>(),
    );
    let idx_bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (idx_off, idx_len) = push_bytes(&mut bin, &idx_bytes);
    let (s0_off, s0_len) = push_f32(
        &mut bin,
        &shape0_pos.iter().flatten().copied().collect::<Vec<_>>(),
    );
    let (s1p_off, s1p_len) = push_f32(
        &mut bin,
        &shape1_pos.iter().flatten().copied().collect::<Vec<_>>(),
    );
    let (s1n_off, s1n_len) = push_f32(
        &mut bin,
        &shape1_nrm.iter().flatten().copied().collect::<Vec<_>>(),
    );
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_len = bin.len();

    let json = format!(
        r#"{{
  "asset": {{ "version": "2.0", "generator": "r3e-test" }},
  "scene": 0,
  "scenes": [{{ "nodes": [0, 1] }}],
  "nodes": [
    {{ "name": "Face", "mesh": 0 }},
    {{ "name": "Plain", "mesh": 1 }}
  ],
  "meshes": [
    {{
      "name": "Face",
      "primitives": [
        {{
          "attributes": {{ "POSITION": 0, "NORMAL": 1 }},
          "indices": 2,
          "targets": [
            {{ "POSITION": 3 }},
            {{ "POSITION": 4, "NORMAL": 5 }}
          ]
        }}
      ]
    }},
    {{
      "name": "Plain",
      "primitives": [
        {{ "attributes": {{ "POSITION": 0, "NORMAL": 1 }}, "indices": 2 }}
      ]
    }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
       "min": [0.0, 0.0, 1.0], "max": [1.0, 1.0, 1.0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR" }},
    {{ "bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 4, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 5, "componentType": 5126, "count": 3, "type": "VEC3" }}
  ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": {pos_off}, "byteLength": {pos_len} }},
    {{ "buffer": 0, "byteOffset": {nor_off}, "byteLength": {nor_len} }},
    {{ "buffer": 0, "byteOffset": {idx_off}, "byteLength": {idx_len} }},
    {{ "buffer": 0, "byteOffset": {s0_off}, "byteLength": {s0_len} }},
    {{ "buffer": 0, "byteOffset": {s1p_off}, "byteLength": {s1p_len} }},
    {{ "buffer": 0, "byteOffset": {s1n_off}, "byteLength": {s1n_len} }}
  ],
  "buffers": [{{ "byteLength": {bin_len} }}]
}}"#
    );

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

    let path = dir.join("morph_conformance.glb");
    std::fs::write(&path, &glb).expect("写 glb 失败");
    path
}

#[test]
fn import_reads_morph_targets_in_order() {
    let dir = fixture_dir("import_reads_morph_targets_in_order");
    let path = write_morph_glb(&dir);
    let entry = import_glb(&path).expect("导入 glb 失败");

    let face = entry
        .meshes
        .iter()
        .find(|mesh| mesh.name == "Face")
        .expect("应有 Face 网格");
    assert_eq!(face.morph.len(), 2, "两个形状必须都导入");
    assert_eq!(face.morph.name_of(0), Some("target0"));
    assert_eq!(face.morph.name_of(1), Some("target1"));

    // 换轴：glTF 的 +Z 台面 → 引擎 -Z，形状 0 的 z 增量必须跟着取反。
    let s0 = face.morph.get(0).expect("形状 0");
    assert!(
        (s0.positions[0][2] + 1.0).abs() < 1e-6,
        "z 增量未镜像: {:?}",
        s0.positions[0]
    );
    assert!(
        s0.positions[0][0].abs() < 1e-6,
        "x 增量不该被改: {:?}",
        s0.positions[0]
    );
    assert!(s0.normals.is_empty(), "形状 0 没有法线增量");

    // 形状 1：x 张开保持不变，法线增量只取反 z、**不归一化**（0.25 仍是 0.25）。
    let s1 = face.morph.get(1).expect("形状 1");
    assert!(
        (s1.positions[0][0] + 0.5).abs() < 1e-6,
        "{:?}",
        s1.positions[0]
    );
    assert!(
        (s1.positions[2][1] - 0.25).abs() < 1e-6,
        "{:?}",
        s1.positions[2]
    );
    assert_eq!(s1.normals.len(), 3, "形状 1 带法线增量");
    for n in &s1.normals {
        assert!(
            (n[2] + 0.25).abs() < 1e-6,
            "法线增量应按法线矩阵变换且不归一化: {n:?}"
        );
        assert!(n[0].abs() < 1e-6 && n[1].abs() < 1e-6, "{n:?}");
    }

    // 没有 targets 的网格不该凭空多出形状。
    let plain = entry
        .meshes
        .iter()
        .find(|mesh| mesh.name == "Plain")
        .expect("应有 Plain 网格");
    assert!(plain.morph.is_empty(), "无 targets 的网格必须保持空形状集");
}

#[test]
fn morph_survives_to_mesh_data_and_validates() {
    let dir = fixture_dir("morph_survives_to_mesh_data_and_validates");
    let path = write_morph_glb(&dir);
    let entry = import_glb(&path).expect("导入 glb 失败");
    let face = entry
        .meshes
        .iter()
        .find(|mesh| mesh.name == "Face")
        .expect("应有 Face 网格");

    // 出口：MeshAsset → 运行时 MeshData，形状不能在中途被丢掉。
    let data = face.to_mesh_data();
    assert!(data.has_morph(), "to_mesh_data 必须带上形变形状");
    assert_eq!(data.morph.len(), face.morph.len());
    assert_eq!(data.morph.len(), 2);

    // 顶点数自洽：渲染层上传前就是拿真实顶点数做这个校验。
    data.morph
        .validate_against(data.vertices.len())
        .expect("形状长度必须与顶点数一致");

    // 增量逐字一致（出口只做搬运，不做二次变换）。
    for (from, to) in face.morph.targets.iter().zip(&data.morph.targets) {
        assert_eq!(from.positions, to.positions);
        assert_eq!(from.normals, to.normals);
    }

    // 权重 0：CPU 版求值必须回到原始位置，证明「不动就是不动」。
    let zero = data.morph.apply(&data, &[0.0; r3e_core::MAX_MORPH_TARGETS]);
    for (a, b) in zero.vertices.iter().zip(&data.vertices) {
        for axis in 0..3 {
            assert!(
                (a.position[axis] - b.position[axis]).abs() < 1e-6,
                "权重 0 时位置应不变"
            );
        }
    }

    // 权重 1：形状 0 的位移必须整段落在第一个顶点上（z 为负，已换轴）。
    let one = data.morph.apply(&data, &[1.0, 0.0, 0.0, 0.0]);
    let base = data.vertices[0].position;
    assert!(
        (one.vertices[0].position[2] - (base[2] - 1.0)).abs() < 1e-5,
        "{:?}",
        one.vertices[0].position
    );
}
