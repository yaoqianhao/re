//! glTF 2.0 / `.glb` 导入器（S3）。
//!
//! 只做一件事：把 glTF 读成 `AssetEntry`（AIR），然后交给 `r3e-core::air::normalize`
//! 统一归一化。**不引入任何 glTF 专属的运行时结构**，因此渲染 / 物理 / 脚本依旧只认识
//! `MeshData` + `MaterialData` 一套数据。
//!
//! 坐标：glTF 右手 +Y up / -Z front → 引擎 +Y up / +Z front。
//! 换轴用 `Axes::GLTF`（行列式为负，`bake_transform` 会自动翻转绕序），因此**不要在
//! 这里手动交换索引**。
//!
//! 已知限制（`docs/ASSETS.md` 同频）：
//! * 不支持 Draco / meshopt 压缩、KTX2 / basis 贴图（需要额外解码器）；
//! * 不支持 morph target、多套 UV（只用 TEXCOORD_0）；
//! * `.gltf` 的 base64 data URI 缓冲区不支持，请用 `.glb` 或外部 `.bin`。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use r3e_core::air::normalize::{self, Axes};
use r3e_core::anim::{AnimClip, AnimKey, AnimTrack};
use r3e_core::mesh::{MeshData, TextureImage, Vertex};
use r3e_core::skin::{SkinData, SkinJoint};
use r3e_core::{AssetId, Mat4, Quat, Vec3};

use crate::importer::AssetImporter;
use crate::{
    AssetEntry, AssetError, MaterialInfo, MaterialPart, MeshAsset, NodeAsset, TextureInfo,
};

/// glTF 导入器。
#[derive(Debug, Default, Clone, Copy)]
pub struct GltfImporter;

impl AssetImporter for GltfImporter {
    fn name(&self) -> &'static str {
        "gltf"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["glb", "gltf"]
    }

    fn import(&self, path: &Path) -> Result<AssetEntry, AssetError> {
        import_gltf(path)
    }
}

fn load_error(path: &Path, message: impl Into<String>) -> AssetError {
    AssetError::Load {
        path: path.display().to_string(),
        message: message.into(),
    }
}

/// 与 `import_gltf` 等价，命名对称于 `import_fbx`，便于调用方按扩展名直呼。
pub fn import_glb(path: &Path) -> Result<AssetEntry, AssetError> {
    import_gltf(path)
}

/// 校验 GLB 容器头：magic / 版本 / 声明长度与实际字节数一致。
///
/// 真实踩坑：半途中断的下载会留下「头部声明 438044 字节、实际 31816 字节」的文件，
/// 直接喂给 gltf crate 只会得到一句 `failed to fill whole buffer`。
fn check_glb_integrity(path: &Path) -> Result<(), AssetError> {
    use std::io::Read;

    let mut file =
        std::fs::File::open(path).map_err(|e| load_error(path, format!("打不开文件: {e}")))?;
    let mut header = [0u8; 12];
    if let Err(e) = file.read_exact(&mut header) {
        return Err(load_error(
            path,
            format!(
                "文件不足 12 字节的 GLB 头（实际 {} 字节）: {e}",
                header_len(path)
            ),
        ));
    }
    if &header[0..4] != b"glTF" {
        return Err(load_error(
            path,
            "不是 GLB 容器（文件头缺少 `glTF` magic），若这是文本 glTF 请改用 .gltf 扩展名",
        ));
    }
    let version = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if version != 2 {
        return Err(load_error(
            path,
            format!("GLB 版本为 {version}，只支持 glTF 2.0"),
        ));
    }
    let declared = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as u64;
    let actual = header_len(path);
    if declared != actual {
        return Err(load_error(
            path,
            if declared > actual {
                format!(
                    "GLB 文件被截断：头部声明 {declared} 字节，实际只有 {actual} 字节（下载/复制可能未完成）"
                )
            } else {
                format!("GLB 头部声明 {declared} 字节，文件实际 {actual} 字节（头部损坏或存在多余尾随数据）")
            },
        ));
    }
    Ok(())
}

fn header_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 读入 glTF（`.glb` 内嵌二进制或 `.gltf` + 外部 `.bin`）。
pub fn import_gltf(path: &Path) -> Result<AssetEntry, AssetError> {
    // `.glb` 先查容器完整性：截断的文件交给 gltf crate 只会得到
    // `failed to fill whole buffer` 这种无法定位的报错。
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
    {
        check_glb_integrity(path)?;
    }
    let gltf =
        gltf::Gltf::open(path).map_err(|e| load_error(path, format!("glTF 解析失败: {e}")))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let display = path.display().to_string();
    let id = AssetId::from_path(&display);

    // ---- 缓冲区 ----
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in gltf.buffers() {
        let bytes = match buffer.source() {
            gltf::buffer::Source::Bin => gltf.blob.clone().unwrap_or_default(),
            gltf::buffer::Source::Uri(uri) if uri.starts_with("data:") => {
                log::warn!("glTF: base64 data URI 缓冲区不支持，按空缓冲处理");
                Vec::new()
            }
            gltf::buffer::Source::Uri(uri) => {
                let file = base_dir.join(uri);
                match std::fs::read(&file) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log::warn!("glTF: 读不到外部缓冲 `{}`: {e}", file.display());
                        Vec::new()
                    }
                }
            }
        };
        buffers.push(bytes);
    }

    let document = gltf.document;

    // ---- 节点树与世界变换 ----
    let node_count = document.nodes().len();
    let mut parents: Vec<Option<usize>> = vec![None; node_count];
    for node in document.nodes() {
        for child in node.children() {
            parents[child.index()] = Some(node.index());
        }
    }
    let worlds = world_matrices(&document, &parents);
    let nodes = build_nodes(&document, &parents);

    let mut path_of: HashMap<u32, String> = HashMap::new();
    for node in &nodes {
        path_of.insert(node.index, node.path.clone());
    }

    // ---- 材质与贴图 ----
    let (materials, textures) = import_materials(&document, &buffers);

    // ---- 骨骼表 ----
    let skins = import_skins(&document, &worlds, &path_of);

    // ---- 网格 ----
    let mut meshes: Vec<MeshAsset> = Vec::new();
    let mut node_mesh: HashMap<u32, u32> = HashMap::new();
    for node in document.nodes() {
        let Some(mesh) = node.mesh() else {
            continue;
        };
        let world = worlds[node.index()];
        let asset = import_mesh(
            path,
            &display,
            &mesh,
            node.index(),
            world,
            node.skin().map(|s| s.index() as u32),
            &path_of,
            &buffers,
        )?;
        node_mesh.insert(node.index() as u32, meshes.len() as u32);
        meshes.push(asset);
    }

    if meshes.is_empty() {
        log::warn!("glTF: `{display}` 不含任何网格");
    }

    // ---- 节点树补 mesh 下标 ----
    let mut nodes = nodes;
    for node in &mut nodes {
        node.mesh = node_mesh.get(&node.index).copied();
    }

    // ---- 动画 ----
    let animations = import_animations(&document, &nodes, &buffers, &path_of);

    Ok(AssetEntry {
        id,
        source_path: display,
        meshes,
        materials,
        textures,
        nodes,
        animations,
        skins,
    })
}

/// 每个节点的世界矩阵（glTF 空间）。
fn world_matrices(document: &gltf::Document, parents: &[Option<usize>]) -> Vec<Mat4> {
    let locals: Vec<Mat4> = document
        .nodes()
        .map(|n| Mat4::from_cols_array_2d(&n.transform().matrix()))
        .collect();
    let mut worlds: Vec<Option<Mat4>> = vec![None; locals.len()];
    for i in 0..locals.len() {
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = Some(i);
        while let Some(idx) = cur {
            if worlds[idx].is_some() {
                break;
            }
            chain.push(idx);
            cur = parents[idx];
            if chain.len() > 512 {
                break;
            }
        }
        for idx in chain.into_iter().rev() {
            let local = locals[idx];
            let world = match parents[idx] {
                Some(p) => worlds[p].unwrap_or(Mat4::IDENTITY) * local,
                None => local,
            };
            worlds[idx] = Some(world);
        }
    }
    worlds
        .into_iter()
        .map(|w| w.unwrap_or(Mat4::IDENTITY))
        .collect()
}

/// glTF TRS → 引擎（镜像 Z + 反向转）。
fn mirror_translation(t: [f32; 3]) -> [f32; 3] {
    [t[0], t[1], -t[2]]
}

fn mirror_rotation(r: [f32; 4]) -> [f32; 4] {
    // 绕 +Z 的镜像：轴分量 x、y 取反，角度取反 ⇒ (x,y,z,w) → (-x,-y,z,w)。
    [-r[0], -r[1], r[2], r[3]]
}

fn build_nodes(document: &gltf::Document, parents: &[Option<usize>]) -> Vec<NodeAsset> {
    let mut nodes: Vec<NodeAsset> = Vec::with_capacity(parents.len());
    for node in document.nodes() {
        let (t, r, s) = node.transform().decomposed();
        nodes.push(NodeAsset {
            index: node.index() as u32,
            name: node
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("node{}", node.index())),
            path: String::new(),
            parent: parents[node.index()].map(|p| p as u32),
            position: mirror_translation(t),
            rotation: mirror_rotation(r),
            scale: s,
            mesh: None,
        });
    }

    let mut paths: Vec<Option<String>> = vec![None; nodes.len()];
    for i in 0..nodes.len() {
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = Some(i);
        while let Some(idx) = cur {
            chain.push(idx);
            if paths[idx].is_some() {
                break;
            }
            cur = nodes[idx].parent.map(|p| p as usize);
            if chain.len() > 512 {
                break;
            }
        }
        let mut acc = chain
            .last()
            .and_then(|idx| paths[*idx].clone())
            .unwrap_or_default();
        for idx in chain.iter().rev() {
            if paths[*idx].is_some() {
                continue;
            }
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(&nodes[*idx].name);
            paths[*idx] = Some(acc.clone());
        }
    }
    for (i, p) in paths.into_iter().enumerate() {
        nodes[i].path = p.unwrap_or_else(|| nodes[i].name.clone());
    }
    nodes
}

/// 材质 + 贴图。贴图下标以 `AssetEntry::textures` 为准，同一 image 只解码一次。
fn import_materials(
    document: &gltf::Document,
    buffers: &[Vec<u8>],
) -> (Vec<MaterialInfo>, Vec<TextureInfo>) {
    let mut textures: Vec<TextureInfo> = Vec::new();
    let mut decoded: HashMap<usize, u32> = HashMap::new();
    let mut materials: Vec<MaterialInfo> = Vec::new();

    for material in document.materials() {
        let pbr = material.pbr_metallic_roughness();
        let mut info = MaterialInfo {
            index: material.index().unwrap_or(materials.len()) as u32,
            name: material
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("material{}", materials.len())),
            base_color: pbr.base_color_factor(),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            textures: Vec::new(),
            base_color_texture: None,
        };

        if let Some(tex_info) = pbr.base_color_texture() {
            let texture = tex_info.texture();
            let image_index = texture.source().index();
            let slot = match decoded.get(&image_index) {
                Some(index) => Some(*index),
                None => match decode_image(&texture.source(), buffers) {
                    Some(image) => {
                        let index = textures.len() as u32;
                        textures.push(TextureInfo {
                            index,
                            element_id: image_index as u32,
                            slot: "BaseColor".to_string(),
                            filename: match texture.source().source() {
                                gltf::image::Source::Uri { uri, .. } => uri.to_string(),
                                gltf::image::Source::View { .. } => {
                                    format!("embedded#{image_index}")
                                }
                            },
                            relative_filename: String::new(),
                            absolute_filename: String::new(),
                            embedded: matches!(
                                texture.source().source(),
                                gltf::image::Source::View { .. }
                            ),
                            content_bytes: 0,
                            has_file: true,
                            uv_set: "UV0".to_string(),
                            image: Some(image),
                        });
                        decoded.insert(image_index, index);
                        Some(index)
                    }
                    None => None,
                },
            };
            if let Some(index) = slot {
                info.base_color_texture = Some(index);
                info.textures.push(textures[index as usize].clone());
            }
        }
        materials.push(info);
    }
    (materials, textures)
}

/// 解码一张 glTF 贴图（bufferView 内嵌或外部文件）。
fn decode_image(image_source: &gltf::Image<'_>, buffers: &[Vec<u8>]) -> Option<Arc<TextureImage>> {
    let bytes: Vec<u8> = match image_source.source() {
        gltf::image::Source::View { view, mime_type } => {
            let buffer = buffers.get(view.buffer().index())?;
            let start = view.offset();
            let end = start + view.length();
            if end > buffer.len() {
                log::warn!(
                    "glTF: 贴图 bufferView 越界（{start}..{end} / {}）",
                    buffer.len()
                );
                return None;
            }
            log::debug!("glTF: 解码内嵌贴图 {mime_type} {} 字节", view.length());
            buffer[start..end].to_vec()
        }
        gltf::image::Source::Uri { uri, .. } => {
            if uri.starts_with("data:") {
                log::warn!("glTF: base64 data URI 贴图不支持，已跳过");
                return None;
            }
            match std::fs::read(uri) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!("glTF: 读不到外部贴图 `{uri}`: {e}");
                    return None;
                }
            }
        }
    };
    let image = image::load_from_memory(&bytes).ok()?;
    let rgba = image.to_rgba8();
    Some(Arc::new(TextureImage::new(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
    )))
}

/// 骨骼表：关节顺序严格等于 `skin.joints()` 顺序（顶点里的 JOINT_0 就是这个下标）。
fn import_skins(
    document: &gltf::Document,
    worlds: &[Mat4],
    path_of: &HashMap<u32, String>,
) -> Vec<SkinData> {
    let mut skins: Vec<SkinData> = Vec::new();
    for skin in document.skins() {
        let joints: Vec<SkinJoint> = skin
            .joints()
            .map(|node| SkinJoint {
                node_index: node.index() as u32,
                path: path_of
                    .get(&(node.index() as u32))
                    .cloned()
                    .unwrap_or_else(|| format!("node{}", node.index())),
                // 静止姿态世界矩阵（镜像 Z）——引擎侧只用它与当前姿态求逆，
                // glTF 的 inverseBindMatrices 实测与「静姿态求逆」等价，故忽略。
                bind_world: mirror_matrix() * worlds[node.index()],
            })
            .collect();
        skins.push(SkinData {
            name: skin
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("skin{}", skin.index())),
            joints,
        });
    }
    skins
}

/// 镜像 Z 的 4x4 矩阵：`diag(1, 1, -1, 1)`。
fn mirror_matrix() -> Mat4 {
    Mat4::from_cols_array_2d(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

#[allow(clippy::too_many_arguments)]
fn import_mesh<'a>(
    path: &Path,
    display: &str,
    mesh: &gltf::Mesh<'a>,
    node_index: usize,
    world: Mat4,
    skin: Option<u32>,
    path_of: &HashMap<u32, String>,
    buffers: &'a [Vec<u8>],
) -> Result<MeshAsset, AssetError> {
    let mut data = MeshData::default();
    let mut parts: Vec<MaterialPart> = Vec::new();
    let mut skinned = false;
    let mut missing_normals = false;
    let mut missing_uv = false;

    for (prim_index, primitive) in mesh.primitives().enumerate() {
        let reader =
            primitive.reader(|buffer| buffers.get(buffer.index()).map(|bytes| bytes.as_slice()));
        let Some(positions) = reader.read_positions().map(|p| p.collect::<Vec<_>>()) else {
            log::warn!(
                "glTF: 网格 `{}` 第 {prim_index} 个 primitive 没有 POSITION，已跳过",
                mesh.name().unwrap_or("<unnamed>")
            );
            continue;
        };
        if positions.is_empty() {
            continue;
        }
        let normals = reader.read_normals().map(|n| n.collect::<Vec<_>>());
        let uvs = reader
            .read_tex_coords(0)
            .map(|t| t.into_f32().collect::<Vec<_>>());
        let joints = reader
            .read_joints(0)
            .map(|j| j.into_u16().collect::<Vec<_>>());
        let weights = reader
            .read_weights(0)
            .map(|w| w.into_f32().collect::<Vec<_>>());
        let indices: Vec<u32> = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..positions.len() as u32).collect(),
        };
        let triangles: Vec<u32> = match primitive.mode() {
            gltf::mesh::Mode::Triangles => indices,
            gltf::mesh::Mode::TriangleStrip => normalize::triangles_from_strip(&indices),
            gltf::mesh::Mode::TriangleFan => normalize::triangles_from_fan(&indices),
            other => {
                log::warn!("glTF: primitive 模式 {other:?} 不是三角形，已跳过（只支持三角网格）");
                continue;
            }
        };

        if normals.is_none() {
            missing_normals = true;
        }
        if uvs.is_none() {
            missing_uv = true;
        }

        let base = data.vertices.len() as u32;
        let has_skin = joints.is_some() && weights.is_some();
        skinned |= has_skin;
        for (i, pos) in positions.iter().enumerate() {
            let normal = normals
                .as_ref()
                .and_then(|n| n.get(i).copied())
                .unwrap_or([0.0, 0.0, 1.0]);
            let uv = uvs
                .as_ref()
                .and_then(|u| u.get(i).copied())
                .unwrap_or([0.0, 0.0]);
            let mut vertex = Vertex::new(*pos, normal, uv);
            if let (Some(j), Some(w)) = (&joints, &weights) {
                if let (Some(j), Some(w)) = (j.get(i), w.get(i)) {
                    vertex.joints = *j;
                    vertex.weights = *w;
                }
            }
            data.vertices.push(vertex);
        }
        // 每个 primitive 一段连续三角形区间，对应一个材质分区。
        let first_triangle = (data.indices.len() / 3) as u32;
        data.indices.extend(triangles.into_iter().map(|i| base + i));
        let triangle_count = (data.indices.len() / 3) as u32 - first_triangle;
        let material = primitive.material();
        parts.push(MaterialPart {
            material_index: material.index().map(|m| m as u32),
            material_name: material.name().unwrap_or_default().to_string(),
            triangle_offset: first_triangle,
            triangle_count,
        });
    }

    if data.vertices.is_empty() || data.indices.is_empty() {
        return Err(load_error(
            path,
            format!(
                "网格 `{}` 没有任何可用的三角形",
                mesh.name().unwrap_or("<unnamed>")
            ),
        ));
    }

    // 蒙皮网格的顶点按 glTF 规范处于「骨架空间」，忽略节点自身的变换；
    // 静态网格则把世界变换烘进顶点（与 FBX 导入一致：`positions` 即世界空间）。
    if !skinned {
        normalize::bake_transform(&mut data, world);
    } else if world != Mat4::IDENTITY {
        log::debug!(
            "glTF: 蒙皮网格 `{}` 的节点世界变换按规范忽略",
            mesh.name().unwrap_or("<unnamed>")
        );
    }
    normalize::to_engine_axes(&mut data, Axes::GLTF);
    if missing_normals {
        normalize::recalc_normals(&mut data);
    }
    if missing_uv {
        log::debug!(
            "glTF: 网格 `{}` 缺少 TEXCOORD_0，UV 填 0",
            mesh.name().unwrap_or("<unnamed>")
        );
    }
    normalize::ensure_indices(&mut data);
    if skinned {
        normalize::compact_skinning(&mut data);
    }

    let mut positions = Vec::with_capacity(data.vertices.len());
    let mut normals = Vec::with_capacity(data.vertices.len());
    let mut uv_out = Vec::with_capacity(data.vertices.len());
    let mut joints_out = Vec::with_capacity(data.vertices.len());
    let mut weights_out = Vec::with_capacity(data.vertices.len());
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &data.vertices {
        positions.push(v.position);
        normals.push(v.normal);
        uv_out.push(v.uv);
        joints_out.push(v.joints);
        weights_out.push(v.weights);
        for k in 0..3 {
            min[k] = min[k].min(v.position[k]);
            max[k] = max[k].max(v.position[k]);
        }
    }
    if !min.iter().chain(max.iter()).all(|v| v.is_finite()) {
        min = [0.0; 3];
        max = [0.0; 3];
    }

    Ok(MeshAsset {
        name: mesh
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("mesh{}", mesh.index())),
        node_path: path_of
            .get(&(node_index as u32))
            .cloned()
            .unwrap_or_else(|| format!("node{node_index}")),
        positions,
        normals,
        uvs: uv_out,
        indices: data.indices.clone(),
        material_parts: parts,
        aabb_min: min,
        aabb_max: max,
        node_local_matrix: (mirror_matrix() * world).to_cols_array_2d(),
        source_path: display.to_string(),
        joints: joints_out,
        weights: weights_out,
        skin: if skinned { skin } else { None },
        source_vertex_count: data.vertices.len(),
        source_face_count: data.indices.len() / 3,
        source_triangle_count: data.indices.len() / 3,
    })
}

/// 动画：把 glTF 通道按节点合并成统一时间轴的 `AnimKey`。
fn import_animations(
    document: &gltf::Document,
    nodes: &[NodeAsset],
    buffers: &[Vec<u8>],
    path_of: &HashMap<u32, String>,
) -> Vec<AnimClip> {
    let mut clips: Vec<AnimClip> = Vec::new();
    for animation in document.animations() {
        // node → 三通道采样表
        #[derive(Default)]
        struct Channels {
            times: Vec<f32>,
            translation: Option<(Vec<f32>, Vec<[f32; 3]>)>,
            rotation: Option<(Vec<f32>, Vec<[f32; 4]>)>,
            scale: Option<(Vec<f32>, Vec<[f32; 3]>)>,
        }
        let mut per_node: HashMap<usize, Channels> = HashMap::new();

        for channel in animation.channels() {
            let target = channel.target();
            let node = target.node();
            let reader =
                channel.reader(|buffer| buffers.get(buffer.index()).map(|bytes| bytes.as_slice()));
            let Some(times) = reader.read_inputs().map(|i| i.collect::<Vec<f32>>()) else {
                continue;
            };
            if times.is_empty() {
                continue;
            }
            let Some(outputs) = reader.read_outputs() else {
                continue;
            };
            let entry = per_node.entry(node.index()).or_default();
            entry.times.extend(times.iter().copied());
            match (target.property(), outputs) {
                (
                    gltf::animation::Property::Translation,
                    gltf::animation::util::ReadOutputs::Translations(t),
                ) => {
                    entry.translation = Some((times, t.collect()));
                }
                (
                    gltf::animation::Property::Rotation,
                    gltf::animation::util::ReadOutputs::Rotations(r),
                ) => {
                    // glTF 输出是 (x, y, z, w)
                    entry.rotation = Some((times, r.into_f32().collect()));
                }
                (
                    gltf::animation::Property::Scale,
                    gltf::animation::util::ReadOutputs::Scales(s),
                ) => {
                    entry.scale = Some((times, s.collect()));
                }
                (property, _) => {
                    log::debug!("glTF: 动画通道 {property:?} 不支持，已跳过");
                }
            }
        }

        let mut tracks: Vec<AnimTrack> = Vec::new();
        for (node_index, channels) in per_node {
            let Some(bind) = nodes.get(node_index) else {
                continue;
            };
            let mut times = channels.times.clone();
            times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            if times.len() < 2 {
                continue; // 常量通道不参与播放
            }
            let mut keys: Vec<AnimKey> = Vec::with_capacity(times.len());
            for t in &times {
                let position = channels
                    .translation
                    .as_ref()
                    .and_then(|(kt, kv)| sample_vec3(kt, kv, *t))
                    .map(mirror_translation)
                    .unwrap_or(bind.position);
                let rotation = channels
                    .rotation
                    .as_ref()
                    .and_then(|(kt, kv)| sample_quat(kt, kv, *t))
                    .map(|q| mirror_rotation([q[0], q[1], q[2], q[3]]))
                    .unwrap_or(bind.rotation);
                let scale = channels
                    .scale
                    .as_ref()
                    .and_then(|(kt, kv)| sample_vec3(kt, kv, *t))
                    .unwrap_or(bind.scale);
                let (rx, ry, rz, rw) = (rotation[0], rotation[1], rotation[2], rotation[3]);
                keys.push(AnimKey {
                    time: *t,
                    position: Vec3::from(position),
                    rotation: Quat::from_xyzw(rx, ry, rz, rw).normalize(),
                    scale: Vec3::from(scale),
                });
            }
            tracks.push(AnimTrack {
                node_path: path_of
                    .get(&(node_index as u32))
                    .cloned()
                    .unwrap_or_else(|| bind.path.clone()),
                node_index: node_index as u32,
                keys,
            });
        }

        if tracks.is_empty() {
            log::debug!(
                "glTF: 动画 `{}` 没有变化的通道，跳过",
                animation.name().unwrap_or("<unnamed>")
            );
            continue;
        }
        let mut clip = AnimClip {
            name: animation
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("anim{}", animation.index())),
            duration: 0.0,
            tracks,
        };
        for track in &mut clip.tracks {
            r3e_core::anim::rebase_track(track);
        }
        clip.duration = r3e_core::anim::clip_duration(&clip.tracks);
        clips.push(clip);
    }
    clips
}

/// 线性/阶梯采样 vec3 通道。
fn sample_vec3(times: &[f32], values: &[[f32; 3]], t: f32) -> Option<[f32; 3]> {
    if times.is_empty() || values.is_empty() {
        return None;
    }
    if t <= times[0] {
        return values.first().copied();
    }
    let last = times.len().min(values.len()) - 1;
    if t >= times[last] {
        return values.get(last).copied();
    }
    for i in 0..last {
        if t >= times[i] && t <= times[i + 1] {
            let span = times[i + 1] - times[i];
            let w = if span.abs() < 1e-9 {
                0.0
            } else {
                (t - times[i]) / span
            };
            let (a, b) = (values[i], values[i + 1]);
            return Some([
                a[0] + (b[0] - a[0]) * w,
                a[1] + (b[1] - a[1]) * w,
                a[2] + (b[2] - a[2]) * w,
            ]);
        }
    }
    values.get(last).copied()
}

/// 线性采样四元数通道（nlerp，关键帧已重采样到统一时间轴，误差可忽略）。
fn sample_quat(times: &[f32], values: &[[f32; 4]], t: f32) -> Option<[f32; 4]> {
    if times.is_empty() || values.is_empty() {
        return None;
    }
    if t <= times[0] {
        return values.first().copied();
    }
    let last = times.len().min(values.len()) - 1;
    if t >= times[last] {
        return values.get(last).copied();
    }
    for i in 0..last {
        if t >= times[i] && t <= times[i + 1] {
            let span = times[i + 1] - times[i];
            let w = if span.abs() < 1e-9 {
                0.0
            } else {
                (t - times[i]) / span
            };
            let a = Quat::from_xyzw(values[i][0], values[i][1], values[i][2], values[i][3]);
            let b = Quat::from_xyzw(
                values[i + 1][0],
                values[i + 1][1],
                values[i + 1][2],
                values[i + 1][3],
            );
            let q = a.slerp(b, w).normalize();
            return Some([q.x, q.y, q.z, q.w]);
        }
    }
    values.get(last).copied()
}
