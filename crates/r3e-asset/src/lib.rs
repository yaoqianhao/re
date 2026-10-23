//! `r3e-asset` —— 外部资产（当前为 FBX）导入层。
//!
//! 职责边界：
//!
//! * 只依赖 `r3e-core` 的数据类型与 `ufbx`，**不依赖 wgpu**，也不认识场景图算法；
//! * 只做静态网格与材质/贴图元数据，不涉及动画、蒙皮求值结果之外的任何运行时；
//! * 输出 `AssetEntry`（一个文件）与 `MeshAsset`（文件内一个网格），渲染层通过
//!   `AssetLibrary::mesh_data` 取 CPU 顶点数据自行上传 GPU。
//!
//! 坐标与单位约定（与 `docs/ASSETS.md` 一致）：
//! 目标坐标系 +X right / +Y up / +Z front，单位米（`target_unit_meters = 1.0`）。
//! 顶点位置在导入时已经过 `node.geometry_to_world` 变换（因此 `positions` 即世界空间），
//! `aabb_min / aabb_max` 直接由这些世界空间顶点求得；`node_local_matrix` 仅作为元数据保留。

use std::collections::HashMap;
use std::path::Path;

use r3e_core::mesh::{MeshData, Vertex};
use r3e_core::{AssetId, Mat4, MeshRef, Vec3, Vec4};
use thiserror::Error;

/// 目标单位：1.0 = 米。
pub const TARGET_UNIT_METERS: f64 = 1.0;

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("加载 FBX 失败 `{path}`: {message}")]
    Load { path: String, message: String },
    #[error("资产 {0} 不存在")]
    Unknown(AssetId),
    #[error("资产 {asset} 不包含第 {mesh} 个网格")]
    MeshNotFound { asset: AssetId, mesh: u32 },
}

/// 一张贴图的元数据（M1 只收集，不绑定 GPU）。
#[derive(Debug, Clone, Default)]
pub struct TextureInfo {
    /// FBX 材质槽名，例如 `DiffuseColor` / `BaseColor` / `NormalMap`。
    pub slot: String,
    /// 材质内声明的原始文件名。
    pub filename: String,
    /// 相对 FBX 的路径（ufbx 解析结果）。
    pub relative_filename: String,
    /// 解析后的绝对路径。
    pub absolute_filename: String,
    /// 是否内嵌在 FBX 二进制里。
    pub embedded: bool,
    /// 内嵌数据字节数（外部文件为 0）。
    pub content_bytes: usize,
    /// ufbx 是否认为该贴图有可用文件。
    pub has_file: bool,
    /// 该贴图使用的 UV 集名称。
    pub uv_set: String,
}

/// 材质元数据。
#[derive(Debug, Clone, Default)]
pub struct MaterialInfo {
    /// 资产内材质序号（= ufbx 场景材质表下标）。
    pub index: u32,
    pub name: String,
    /// 线性 base color。
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub textures: Vec<TextureInfo>,
}

/// 网格内的一个材质分区（连续三角形段）。
#[derive(Debug, Clone, Default)]
pub struct MaterialPart {
    /// 指向 `AssetEntry::materials` 的下标；`None` 表示 ufbx 未给出材质。
    pub material_index: Option<u32>,
    pub material_name: String,
    /// 该分区首个三角形在 `MeshAsset::indices` 中的序号（三角形为单位）。
    pub triangle_offset: u32,
    pub triangle_count: u32,
}

/// 资产内的单个网格（世界空间）。
#[derive(Debug, Clone)]
pub struct MeshAsset {
    /// 网格名（ufbx 网格元素名）。
    pub name: String,
    /// 网格所属节点在资产内的路径，例如 `RootNode/Arm/Hand`。
    pub node_path: String,
    /// 顶点位置，**已应用 `geometry_to_world`**。
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// UV，已做 `v' = 1 - v` 翻转（引擎 UV 原点在左上）。
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material_parts: Vec<MaterialPart>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// `node.geometry_to_world` 的 4x4 列主序拷贝（元数据）。
    pub node_local_matrix: [[f32; 4]; 4],
    pub source_path: String,
    /// ufbx 侧的原始统计，便于和导入结果交叉验证。
    pub source_vertex_count: usize,
    pub source_face_count: usize,
    pub source_triangle_count: usize,
}

impl MeshAsset {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// UV 是否有效（至少一个非零 UV）。
    pub fn uv_valid(&self) -> bool {
        self.uvs
            .iter()
            .any(|uv| uv[0].abs() > 1e-6 || uv[1].abs() > 1e-6)
    }

    /// UV 取值范围，便于确认是否真的按 `v' = 1 - v` 翻转过。
    pub fn uv_bounds(&self) -> ([f32; 2], [f32; 2]) {
        let mut min = [f32::INFINITY, f32::INFINITY];
        let mut max = [f32::NEG_INFINITY, f32::NEG_INFINITY];
        for uv in &self.uvs {
            min[0] = min[0].min(uv[0]);
            min[1] = min[1].min(uv[1]);
            max[0] = max[0].max(uv[0]);
            max[1] = max[1].max(uv[1]);
        }
        if !min[0].is_finite() {
            return ([0.0, 0.0], [0.0, 0.0]);
        }
        (min, max)
    }

    pub fn size(&self) -> [f32; 3] {
        [
            self.aabb_max[0] - self.aabb_min[0],
            self.aabb_max[1] - self.aabb_min[1],
            self.aabb_max[2] - self.aabb_min[2],
        ]
    }

    /// 转成渲染层可直接上传的 CPU 网格。
    pub fn to_mesh_data(&self) -> MeshData {
        MeshData::from_arrays(&self.positions, &self.normals, Some(&self.uvs), self.indices.clone())
    }
}

/// 一个资产文件（FBX）的导入结果。
#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub id: AssetId,
    pub source_path: String,
    pub meshes: Vec<MeshAsset>,
    pub materials: Vec<MaterialInfo>,
    /// 全部材质贴图的扁平列表（去重前）。
    pub textures: Vec<TextureInfo>,
}

impl AssetEntry {
    pub fn mesh(&self, index: u32) -> Option<&MeshAsset> {
        self.meshes.get(index as usize)
    }

    pub fn triangle_total(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }

    pub fn vertex_total(&self) -> usize {
        self.meshes.iter().map(|m| m.vertex_count()).sum()
    }
}

/// 资产库：按 `AssetId` 索引已导入的资产文件。
#[derive(Debug, Default)]
pub struct AssetLibrary {
    entries: HashMap<AssetId, AssetEntry>,
    order: Vec<AssetId>,
}

impl AssetLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn ids(&self) -> &[AssetId] {
        &self.order
    }

    pub fn get(&self, id: AssetId) -> Option<&AssetEntry> {
        self.entries.get(&id)
    }

    pub fn entries(&self) -> impl Iterator<Item = &AssetEntry> {
        self.order.iter().filter_map(move |id| self.entries.get(id))
    }

    /// 按 `MeshRef` 取网格资产；内置网格返回 `None`。
    pub fn mesh(&self, mesh: MeshRef) -> Option<&MeshAsset> {
        match mesh {
            MeshRef::Builtin(_) => None,
            MeshRef::Asset { asset, mesh } => self.entries.get(&asset)?.mesh(mesh),
        }
    }

    /// 按 `MeshRef` 取 CPU 网格数据（渲染层上传入口）。
    pub fn mesh_data(&self, mesh: MeshRef) -> Option<MeshData> {
        self.mesh(mesh).map(|m| m.to_mesh_data())
    }

    /// 导入一个 FBX 文件并登记到资产库。
    pub fn load_fbx(&mut self, path: impl AsRef<Path>) -> Result<AssetId, AssetError> {
        let path = path.as_ref();
        let entry = import_fbx(path)?;
        let id = entry.id;
        if self.entries.insert(id, entry).is_none() {
            self.order.push(id);
        }
        Ok(id)
    }

    /// 生成一份人类可读的资产报告（供 `--dump-asset` 使用）。
    pub fn dump(&self, id: AssetId) -> Result<String, AssetError> {
        let entry = self.entries.get(&id).ok_or(AssetError::Unknown(id))?;
        Ok(render_dump(entry))
    }
}

impl r3e_core::mesh::MeshProvider for AssetLibrary {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData> {
        AssetLibrary::mesh_data(self, key)
    }
}

/// 一次性导入并输出报告（`--dump-asset` 的便捷入口）。
pub fn dump_fbx(path: impl AsRef<Path>) -> Result<String, AssetError> {
    let entry = import_fbx(path.as_ref())?;
    Ok(render_dump(&entry))
}

/// 加载参数：坐标系、单位与各项求值开关。
pub fn load_opts() -> ufbx::LoadOpts<'static> {
    ufbx::LoadOpts {
        // +X right / +Y up / +Z front
        target_axes: ufbx::CoordinateAxes {
            right: ufbx::CoordinateAxis::PositiveX,
            up: ufbx::CoordinateAxis::PositiveY,
            front: ufbx::CoordinateAxis::PositiveZ,
        },
        target_unit_meters: TARGET_UNIT_METERS,
        load_external_files: true,
        ignore_missing_external_files: true,
        evaluate_skinning: true,
        use_blender_pbr_material: true,
        // 顶点法线缺失时由 ufbx 生成，避免下游拿到零向量
        generate_missing_normals: true,
        ..Default::default()
    }
}

/// 导入 FBX：解析 → 逐节点抽取网格 → 世界空间变换 → 材质分区。
pub fn import_fbx(path: &Path) -> Result<AssetEntry, AssetError> {
    let display = path.display().to_string();
    let scene = ufbx::load_file(&display, load_opts()).map_err(|e| AssetError::Load {
        path: display.clone(),
        message: format!("{:?}: {} {}", e.type_, e.description, e.info()),
    })?;

    let id = AssetId::from_path(&display);

    // ---- 1) 全局材质表 ----
    let mut materials: Vec<MaterialInfo> = Vec::with_capacity(scene.materials.len());
    let mut material_id_to_index: HashMap<u32, u32> = HashMap::new();
    for i in 0..scene.materials.len() {
        let mat: &ufbx::Material = &scene.materials[i];
        let info = extract_material(mat, i as u32);
        material_id_to_index.insert(mat.element.element_id, i as u32);
        materials.push(info);
    }

    let textures: Vec<TextureInfo> = materials
        .iter()
        .flat_map(|m| m.textures.iter().cloned())
        .collect();

    // ---- 2) 节点父子索引（用于拼节点路径） ----
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    let mut name_of: HashMap<u32, String> = HashMap::new();
    for i in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[i];
        let nid = node.element.element_id;
        name_of.insert(nid, node.element.name.to_string());
        if let Some(parent) = node.parent.as_ref() {
            parent_of.insert(nid, parent.element.element_id);
        }
    }

    // ---- 3) 逐节点抽取网格 ----
    let mut meshes: Vec<MeshAsset> = Vec::new();
    for i in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[i];
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh: &ufbx::Mesh = mesh_ref;
        let asset = extract_mesh(
            mesh,
            node,
            &display,
            &materials,
            &material_id_to_index,
            &parent_of,
            &name_of,
        )?;
        log::debug!(
            "asset: {} -> {} ({} 顶点 / {} 三角)",
            asset.node_path,
            asset.name,
            asset.vertex_count(),
            asset.triangle_count()
        );
        meshes.push(asset);
    }

    if meshes.is_empty() {
        log::warn!("asset: `{display}` 不含任何网格节点");
    }

    Ok(AssetEntry {
        id,
        source_path: display,
        meshes,
        materials,
        textures,
    })
}

fn extract_material(mat: &ufbx::Material, index: u32) -> MaterialInfo {
    let pbr = &mat.pbr;
    // base_factor 在经典 Phong 材质下可能只携带 1 个分量（仅 x 有值），
    // 直接当 vec4 用会得到 (1,0,0) 这种假红色，因此仅在 3 分量时才采用。
    let mut base = if pbr.base_factor.has_value && pbr.base_factor.value_components >= 3 {
        pbr.base_factor.value_vec4
    } else if pbr.base_color.has_value {
        pbr.base_color.value_vec4
    } else {
        ufbx::Vec4 {
            x: 0.8,
            y: 0.8,
            z: 0.8,
            w: 1.0,
        }
    };
    if base.w <= 0.0 {
        base.w = 1.0;
    }

    MaterialInfo {
        index,
        name: mat.element.name.to_string(),
        base_color: [base.x as f32, base.y as f32, base.z as f32, base.w as f32],
        metallic: map_scalar(&pbr.metalness),
        roughness: map_scalar(&pbr.roughness),
        textures: extract_textures(mat),
    }
}

fn map_scalar(map: &ufbx::MaterialMap) -> f32 {
    if map.has_value {
        map.value_vec4.x as f32
    } else {
        0.0
    }
}

fn extract_textures(mat: &ufbx::Material) -> Vec<TextureInfo> {
    let mut out = Vec::new();
    for i in 0..mat.textures.len() {
        let slot: &ufbx::MaterialTexture = &mat.textures[i];
        let tex: &ufbx::Texture = &slot.texture;
        let slot_name = if slot.material_prop.is_empty() {
            slot.shader_prop.to_string()
        } else {
            slot.material_prop.to_string()
        };
        out.push(TextureInfo {
            slot: slot_name,
            filename: tex.filename.to_string(),
            relative_filename: tex.relative_filename.to_string(),
            absolute_filename: tex.absolute_filename.to_string(),
            embedded: !tex.content.is_empty(),
            content_bytes: tex.content.size,
            has_file: tex.has_file,
            uv_set: tex.uv_set.to_string(),
        });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn extract_mesh(
    mesh: &ufbx::Mesh,
    node: &ufbx::Node,
    source_path: &str,
    materials: &[MaterialInfo],
    material_id_to_index: &HashMap<u32, u32>,
    parent_of: &HashMap<u32, u32>,
    name_of: &HashMap<u32, String>,
) -> Result<MeshAsset, AssetError> {
    let world = mat4_from(&node.geometry_to_world);
    let normal_matrix = world.inverse().transpose();
    let has_normal = mesh.vertex_normal.exists;
    let has_uv = mesh.vertex_uv.exists;

    // ---- 顶点：必须按「角点（corner）」拆分，而不是按 ufbx 顶点号 ----
    //
    // ufbx 的 position/normal/uv 是三套独立的取值空间：mesh.vertex_indices[c] 把角点
    // 映射到位号数组，vertex_normal.indices[c] / vertex_uv.indices[c] 映射到法线/UV 数组。
    // 同一个位号在不同角点上可能带不同 UV（UV 接缝）或不同法线（硬边），
    // 因此这里以 (position, normal, uv) 三元组去重生成最终顶点，
    // 并由角点号反查输出顶点号，保证 GPU 侧拿到的是自洽的顶点缓冲。
    let corner_count = mesh.num_indices;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut corner_to_vertex: Vec<u32> = vec![0; corner_count];
    let mut dedup: HashMap<(u32, u32, u32), u32> = HashMap::new();

    let mut aabb_min = Vec3::splat(f32::INFINITY);
    let mut aabb_max = Vec3::splat(f32::NEG_INFINITY);

    for c in 0..corner_count {
        let pos_ix = mesh.vertex_indices.get(c).copied().unwrap_or(0);
        let nrm_ix = if has_normal {
            mesh.vertex_normal.indices.get(c).copied().unwrap_or(0)
        } else {
            0
        };
        let uv_ix = if has_uv {
            mesh.vertex_uv.indices.get(c).copied().unwrap_or(0)
        } else {
            0
        };
        let key = (pos_ix, nrm_ix, uv_ix);
        if let Some(&out) = dedup.get(&key) {
            corner_to_vertex[c] = out;
            continue;
        }

        // 位置：geometry_to_world 承担单位换算与轴向转换（M0 探针结论）
        let p = mesh
            .vertex_position
            .values
            .get(pos_ix as usize)
            .copied()
            .unwrap_or(ufbx::Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
        let wp = ufbx::transform_position(&node.geometry_to_world, p);
        let wp = Vec3::new(wp.x as f32, wp.y as f32, wp.z as f32);
        aabb_min = aabb_min.min(wp);
        aabb_max = aabb_max.max(wp);

        // 法线：缺失时 ufbx 会生成 generated_normals，取不到则退化为 +Y
        let n_local = if has_normal {
            mesh.vertex_normal
                .values
                .get(nrm_ix as usize)
                .copied()
                .unwrap_or(ufbx::Vec3 {
                    x: 0.0,
                    y: 1.0,
                    z: 0.0,
                })
        } else {
            ufbx::Vec3 {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            }
        };
        let n = normal_matrix.transform_vector3(Vec3::new(
            n_local.x as f32,
            n_local.y as f32,
            n_local.z as f32,
        ));
        let n = if n.length_squared() > 1e-12 {
            n.normalize()
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };

        // UV：FBX 原点在左下，引擎在左上 → v' = 1 - v
        let (u, v) = if has_uv {
            let uv = mesh
                .vertex_uv
                .values
                .get(uv_ix as usize)
                .copied()
                .unwrap_or(ufbx::Vec2 { x: 0.0, y: 0.0 });
            (uv.x as f32, 1.0 - uv.y as f32)
        } else {
            (0.0, 0.0)
        };

        let out = positions.len() as u32;
        positions.push([wp.x, wp.y, wp.z]);
        normals.push([n.x, n.y, n.z]);
        uvs.push([u, v]);
        dedup.insert(key, out);
        corner_to_vertex[c] = out;
    }

    if !aabb_min.is_finite() || !aabb_max.is_finite() {
        aabb_min = Vec3::ZERO;
        aabb_max = Vec3::ZERO;
    }

    // ---- 索引：逐 face 三角化，禁止用 num_indices 直接拼 ----
    let mut indices: Vec<u32> = Vec::with_capacity(mesh.num_triangles * 3);
    let mut material_parts: Vec<MaterialPart> = Vec::new();
    let mut face_tris: Vec<u32> = Vec::new();

    let mut part_mat: Option<Option<u32>> = None; // Some(当前分区的材质下标)

    for fi in 0..mesh.num_faces {
        let face = mesh.faces[fi];
        ufbx::triangulate_face_vec(&mut face_tris, mesh, face);
        if face_tris.is_empty() {
            continue;
        }
        if mesh.reversed_winding {
            let tris = face_tris.len() / 3;
            for t in 0..tris {
                face_tris.swap(t * 3 + 1, t * 3 + 2);
            }
        }

        let raw_material = mesh.face_material.get(fi).copied().unwrap_or(0);
        let global_material = material_of(mesh, raw_material, material_id_to_index);
        let material_name = global_material
            .and_then(|idx| materials.get(idx as usize))
            .map(|m| m.name.clone())
            .unwrap_or_default();

        let tri_count = (face_tris.len() / 3) as u32;
        let tri_offset = (indices.len() / 3) as u32;
        // triangulate_face_vec 输出的是角点号，必须经 corner_to_vertex 转成顶点号
        indices.extend(face_tris.iter().map(|&c| {
            corner_to_vertex
                .get(c as usize)
                .copied()
                .unwrap_or(0)
        }));

        // 连续同材质 face 合并为一个分区
        let same_part = match part_mat {
            Some(prev) => prev == global_material,
            None => false,
        };
        if same_part {
            if let Some(last) = material_parts.last_mut() {
                last.triangle_count += tri_count;
            }
        } else {
            material_parts.push(MaterialPart {
                material_index: global_material,
                material_name: material_name.clone(),
                triangle_offset: tri_offset,
                triangle_count: tri_count,
            });
            part_mat = Some(global_material);
        }
    }

    Ok(MeshAsset {
        name: mesh.element.name.to_string(),
        node_path: node_path_of(node.element.element_id, parent_of, name_of),
        positions,
        normals,
        uvs,
        indices,
        material_parts,
        aabb_min: [aabb_min.x, aabb_min.y, aabb_min.z],
        aabb_max: [aabb_max.x, aabb_max.y, aabb_max.z],
        node_local_matrix: world.to_cols_array_2d(),
        source_path: source_path.to_string(),
        source_vertex_count: mesh.num_vertices,
        source_face_count: mesh.num_faces,
        source_triangle_count: mesh.num_triangles,
    })
}

fn material_of(
    mesh: &ufbx::Mesh,
    raw_material: u32,
    material_id_to_index: &HashMap<u32, u32>,
) -> Option<u32> {
    if mesh.materials.is_empty() {
        return None;
    }
    let idx = raw_material as usize;
    if idx >= mesh.materials.len() {
        return Some(0);
    }
    let mat: &ufbx::Material = &mesh.materials[idx];
    material_id_to_index
        .get(&mat.element.element_id)
        .copied()
        .or(Some(raw_material))
}

fn node_path_of(
    id: u32,
    parent_of: &HashMap<u32, u32>,
    name_of: &HashMap<u32, String>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Some(id);
    for _ in 0..64 {
        let Some(c) = cur else { break };
        if let Some(name) = name_of.get(&c) {
            if !name.is_empty() {
                parts.push(name.clone());
            }
        }
        cur = parent_of.get(&c).copied();
    }
    parts.reverse();
    parts.join("/")
}

/// ufbx 的 4x3 列主序矩阵 → glam 4x4。
pub fn mat4_from(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(m.m00 as f32, m.m10 as f32, m.m20 as f32, 0.0),
        Vec4::new(m.m01 as f32, m.m11 as f32, m.m21 as f32, 0.0),
        Vec4::new(m.m02 as f32, m.m12 as f32, m.m22 as f32, 0.0),
        Vec4::new(m.m03 as f32, m.m13 as f32, m.m23 as f32, 1.0),
    )
}

/// 把导入结果渲染成文本报告。
pub fn render_dump(entry: &AssetEntry) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "===== FBX 资产报告 =====");
    let _ = writeln!(out, "文件: {}", entry.source_path);
    let _ = writeln!(out, "AssetId: {}", entry.id);
    let _ = writeln!(
        out,
        "网格数: {}  顶点合计: {}  三角合计: {}  材质数: {}  贴图数: {}",
        entry.meshes.len(),
        entry.vertex_total(),
        entry.triangle_total(),
        entry.materials.len(),
        entry.textures.len()
    );

    for (i, m) in entry.meshes.iter().enumerate() {
        let (uv_min, uv_max) = m.uv_bounds();
        let size = m.size();
        let _ = writeln!(out, "\n-- [{}] 节点路径: {}", i, m.node_path);
        let _ = writeln!(out, "   网格名: {}", m.name);
        let _ = writeln!(
            out,
            "   顶点: {} (ufbx {} 顶点)  三角: {} (ufbx {} 三角)  索引: {}",
            m.vertex_count(),
            m.source_vertex_count,
            m.triangle_count(),
            m.source_triangle_count,
            m.indices.len()
        );
        let _ = writeln!(out, "   ufbx face 数: {}", m.source_face_count);
        let _ = writeln!(
            out,
            "   AABB min: ({:.4}, {:.4}, {:.4})",
            m.aabb_min[0], m.aabb_min[1], m.aabb_min[2]
        );
        let _ = writeln!(
            out,
            "   AABB max: ({:.4}, {:.4}, {:.4})  尺寸: ({:.4}, {:.4}, {:.4})",
            m.aabb_max[0], m.aabb_max[1], m.aabb_max[2], size[0], size[1], size[2]
        );
        let _ = writeln!(
            out,
            "   UV: {} (u {:.3}..{:.3}, v {:.3}..{:.3})",
            if m.uv_valid() { "有效" } else { "无效/全零" },
            uv_min[0],
            uv_max[0],
            uv_min[1],
            uv_max[1]
        );
        let _ = writeln!(out, "   材质分区: {} 个", m.material_parts.len());
        for (pi, part) in m.material_parts.iter().enumerate() {
            let _ = writeln!(
                out,
                "     part {}: 材质 #{} \"{}\"  三角偏移 {}  三角数 {}",
                pi,
                part.material_index
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                part.material_name,
                part.triangle_offset,
                part.triangle_count
            );
        }
        let _ = writeln!(out, "   贴图:");
        if m.material_parts.is_empty() {
            let _ = writeln!(out, "     (无材质分区)");
        }
        for (pi, part) in m.material_parts.iter().enumerate() {
            let _ = writeln!(
                out,
                "     part {} -> {}",
                pi,
                part.material_name
            );
            let Some(mi) = part.material_index else { continue };
            let Some(mat) = entry.materials.get(mi as usize) else {
                continue;
            };
            if mat.textures.is_empty() {
                let _ = writeln!(out, "       (该材质没有贴图)");
            }
            for tex in &mat.textures {
                let state = if tex.embedded {
                    format!("嵌入 {} 字节", tex.content_bytes)
                } else if tex.has_file {
                    "外部文件已解析".to_string()
                } else {
                    "外部文件缺失".to_string()
                };
                let _ = writeln!(
                    out,
                    "       - [{}] {} ({}) 原始引用: {}",
                    tex.slot,
                    if tex.filename.is_empty() {
                        "<未命名>"
                    } else {
                        tex.filename.as_str()
                    },
                    state,
                    if tex.absolute_filename.is_empty() {
                        "-"
                    } else {
                        tex.absolute_filename.as_str()
                    }
                );
            }
        }
    }

    if !entry.materials.is_empty() {
        let _ = writeln!(out, "\n材质表:");
        for m in &entry.materials {
            let _ = writeln!(
                out,
                "  #{} \"{}\"  base_color=({:.3}, {:.3}, {:.3}, {:.3})  metallic={:.3}  roughness={:.3}  贴图 {} 张",
                m.index,
                m.name,
                m.base_color[0],
                m.base_color[1],
                m.base_color[2],
                m.base_color[3],
                m.metallic,
                m.roughness,
                m.textures.len()
            );
        }
    }

    out
}

/// 为渲染层构造顶点（供外部直接使用，避免重复实现 `MeshData::from_arrays`）。
pub fn vertices_of(mesh: &MeshAsset) -> Vec<Vertex> {
    let count = mesh
        .positions
        .len()
        .min(mesh.normals.len())
        .min(mesh.uvs.len());
    (0..count)
        .map(|i| Vertex::new(mesh.positions[i], mesh.normals[i], mesh.uvs[i]))
        .collect()
}
