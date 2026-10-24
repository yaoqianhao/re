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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use r3e_core::anim::{AnimClip, AnimTrack};
use r3e_core::mesh::{
    MaterialData, MeshData, MeshPartData, TextureImage, Vertex,
};
use r3e_core::skin::{SkinData, SkinJoint};
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

/// 一张贴图的元数据与解码结果（P2 起带像素）。
#[derive(Debug, Clone, Default)]
pub struct TextureInfo {
    /// 在 `AssetEntry::textures` 扁平表中的下标（材质引用贴图时用它）。
    pub index: u32,
    /// ufbx 贴图元素 id，用于跨材质去重。
    pub element_id: u32,
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
    /// 解码后的 RGBA8 像素；解码失败或文件缺失时为 `None`（渲染层回退到纯色）。
    pub image: Option<Arc<TextureImage>>,
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
    /// 当作 base color 使用的贴图在 `AssetEntry::textures` 中的下标。
    pub base_color_texture: Option<u32>,
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
    /// 每顶点关节槽位（P4）：与 `positions` 等长；全 0 表示该网格不参与蒙皮。
    pub joints: Vec<[u16; 4]>,
    /// 每顶点关节权重（P4）：与 `positions` 等长，已归一化。
    pub weights: Vec<[f32; 4]>,
    /// 该网格所用骨骼表在 `AssetEntry::skins` 中的下标（P4）；`None` 为静态网格。
    pub skin: Option<u32>,
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

    /// 转成渲染层可直接上传的 CPU 网格（蒙皮网格会自动带上 joint/weight）。
    pub fn to_mesh_data(&self) -> MeshData {
        if self.joints.len() == self.positions.len() && !self.joints.is_empty() {
            MeshData::from_arrays_skinned(
                &self.positions,
                &self.normals,
                Some(&self.uvs),
                Some(&self.joints),
                Some(&self.weights),
                self.indices.clone(),
            )
        } else {
            MeshData::from_arrays(
                &self.positions,
                &self.normals,
                Some(&self.uvs),
                self.indices.clone(),
            )
        }
    }

    /// 是否为蒙皮网格。
    pub fn is_skinned(&self) -> bool {
        self.skin.is_some() && self.joints.len() == self.positions.len()
    }
}

/// 资产内的一个节点（P3 起）。
///
/// 与 FBX 节点树一一对应，`index` 就是动画轨道里的 `node_index`。`path` 是
/// `Root/Arm/Hand` 形式的层级路径，用于日志与脚本按名字定位。
#[derive(Debug, Clone, Default)]
pub struct NodeAsset {
    pub index: u32,
    pub name: String,
    pub path: String,
    pub parent: Option<u32>,
    /// 绑定姿态的局部变换（节点未被动画驱动时的值）。
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// 该节点自身挂的网格在 `AssetEntry::meshes` 里的下标。
    pub mesh: Option<u32>,
}

/// 一个资产文件（FBX）的导入结果。
#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub id: AssetId,
    pub source_path: String,
    pub meshes: Vec<MeshAsset>,
    pub materials: Vec<MaterialInfo>,
    /// 去重后的贴图表：同一 ufbx 贴图元素只出现一次，`MaterialInfo::textures[].index` 指到这里。
    pub textures: Vec<TextureInfo>,
    /// 节点树（P3 起）：层级、绑定姿态与网格挂载点。
    pub nodes: Vec<NodeAsset>,
    /// 烘焙后的动画片段（P3 起）；为空表示该资产没有可播放的节点动画。
    pub animations: Vec<AnimClip>,
    /// 骨骼表（P4 起）：`MeshAsset::skin` 指向这里，关节记录 FBX 节点下标与逆绑定矩阵。
    pub skins: Vec<SkinData>,
}

impl AssetEntry {
    pub fn mesh(&self, index: u32) -> Option<&MeshAsset> {
        self.meshes.get(index as usize)
    }

    /// 按名字找动画片段（大小写不敏感）。
    pub fn animation(&self, name: &str) -> Option<&AnimClip> {
        self.animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// 默认片段：第一个非空动画。
    pub fn default_animation(&self) -> Option<&AnimClip> {
        self.animations.first()
    }


    pub fn triangle_total(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }

    pub fn vertex_total(&self) -> usize {
        self.meshes.iter().map(|m| m.vertex_count()).sum()
    }

    /// 按骨骼表下标取蒙皮描述（P4）。
    pub fn skin(&self, index: u32) -> Option<&SkinData> {
        self.skins.get(index as usize)
    }

    /// 蒙皮统计：`(蒙皮网格数, 关节总数)`。
    pub fn skin_stats(&self) -> (usize, usize) {
        (
            self.meshes.iter().filter(|m| m.is_skinned()).count(),
            self.skins.iter().map(|s| s.joint_count()).sum(),
        )
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

    fn mesh_parts(&self, key: MeshRef) -> Vec<MeshPartData> {
        let MeshRef::Asset { asset, mesh } = key else {
            return Vec::new();
        };
        let Some(entry) = self.entries.get(&asset) else {
            return Vec::new();
        };
        let Some(mesh_asset) = entry.mesh(mesh) else {
            return Vec::new();
        };
        mesh_asset
            .material_parts
            .iter()
            .map(|part| MeshPartData {
                first_index: part.triangle_offset * 3,
                index_count: part.triangle_count * 3,
                material: part.material_index.map(|m| (asset, m)),
            })
            .collect()
    }

    fn material(&self, asset: AssetId, index: u32) -> Option<MaterialData> {
        let info = self.entries.get(&asset)?.materials.get(index as usize)?;
        Some(MaterialData {
            name: info.name.clone(),
            base_color: info.base_color,
            metallic: info.metallic,
            roughness: info.roughness,
            base_color_texture: info.base_color_texture.map(|t| (asset, t)),
        })
    }

    fn texture(&self, asset: AssetId, index: u32) -> Option<TextureImage> {
        let info = self.entries.get(&asset)?.textures.get(index as usize)?;
        info.image.as_deref().cloned()
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
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();

    // ---- 1) 全局材质表 ----
    let mut materials: Vec<MaterialInfo> = Vec::with_capacity(scene.materials.len());
    let mut material_id_to_index: HashMap<u32, u32> = HashMap::new();
    for i in 0..scene.materials.len() {
        let mat: &ufbx::Material = &scene.materials[i];
        let info = extract_material(mat, i as u32, &base_dir);
        material_id_to_index.insert(mat.element.element_id, i as u32);
        materials.push(info);
    }

    // ---- 1b) 贴图去重 + 指定 base color 贴图 ----
    //
    // 同一张贴图常被多个材质引用，因此按 ufbx 贴图元素 id 去重成一张扁平表；
    // 材质里只保留「槽位元数据 + 扁平表下标」，渲染层据此建 bind group。
    let mut textures: Vec<TextureInfo> = Vec::new();
    let mut tex_index: HashMap<u32, u32> = HashMap::new();
    for mat in &mut materials {
        for tex in &mut mat.textures {
            let index = match tex_index.get(&tex.element_id) {
                Some(&existing) => existing,
                None => {
                    let index = textures.len() as u32;
                    tex_index.insert(tex.element_id, index);
                    textures.push(tex.clone_with_index(index));
                    index
                }
            };
            tex.index = index;
        }
        mat.base_color_texture = pick_base_color_texture(&mat.textures);
        // FBX SDK 把 0.588 当「未设置」的缺省 diffuse 因子；带贴图时若仍是该值，
        // 归一成白色，避免 0.588²·² 的二次压暗（真实外观由贴图决定）。
        if mat.base_color_texture.is_some() && is_default_fbx_factor(mat.base_color) {
            mat.base_color = [1.0, 1.0, 1.0, mat.base_color[3].max(1.0)];
        }
    }
    let decoded = textures.iter().filter(|t| t.image.is_some()).count();
    if !textures.is_empty() {
        log::debug!(
            "asset: `{display}` 贴图 {} 张（成功解码 {decoded} 张）",
            textures.len()
        );
    }

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

    // 节点 element_id → scene.nodes 下标（等于 `NodeAsset::index`），
    // 蒙皮关节要用它把 FBX 骨骼节点绑到动画轨道同一套下标上（P4）
    let mut node_index_of: HashMap<u32, u32> = HashMap::with_capacity(scene.nodes.len());
    for i in 0..scene.nodes.len() {
        node_index_of.insert(scene.nodes[i].element.element_id, i as u32);
    }

    // ---- 3) 逐节点抽取网格 ----
    let mut meshes: Vec<MeshAsset> = Vec::new();
    // 骨骼表（P4）：多个网格共用同一 deformation 时按 deformer 名复用
    let mut skins: Vec<SkinData> = Vec::new();
    // 节点 element_id -> 网格下标，供节点树记录「哪个节点挂着哪个网格」
    let mut node_mesh: HashMap<u32, u32> = HashMap::new();
    for i in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[i];
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh: &ufbx::Mesh = mesh_ref;
        let (mut asset, skin_data) = extract_mesh(
            mesh,
            node,
            &display,
            &materials,
            &material_id_to_index,
            &parent_of,
            &name_of,
            &node_index_of,
        )?;
        if let Some(skin) = skin_data {
            let slot = match skins
                .iter()
                .position(|s| s.name == skin.name && s.joint_count() == skin.joint_count())
            {
                Some(existing) => existing as u32,
                None => {
                    skins.push(skin);
                    (skins.len() - 1) as u32
                }
            };
            asset.skin = Some(slot);
            log::debug!(
                "asset: {} -> 蒙皮 {} 关节",
                asset.node_path,
                skins[slot as usize].joint_count()
            );
        } else {
            asset.skin = None;
        }
        log::debug!(
            "asset: {} -> {} ({} 顶点 / {} 三角)",
            asset.node_path,
            asset.name,
            asset.vertex_count(),
            asset.triangle_count()
        );
        node_mesh.insert(node.element.element_id, meshes.len() as u32);
        meshes.push(asset);
    }

    if meshes.is_empty() {
        log::warn!("asset: `{display}` 不含任何网格节点");
    }

    // ---- 3b) 节点树（P3 起） ----
    let nodes = extract_nodes(&scene, &node_mesh);

    // ---- 3c) 动画烘焙（P3 起） ----
    let animations = extract_animations(&scene, &nodes);
    if !animations.is_empty() {
        let keys: usize = animations.iter().map(|a| a.tracks.iter().map(|t| t.keys.len()).sum::<usize>()).sum();
        log::debug!(
            "asset: `{display}` 动画 {} 段 / 共 {keys} 个关键帧",
            animations.len()
        );
    }

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

/// 由 ufbx 节点表构建节点树：父子下标、层级路径与绑定姿态局部变换。
fn extract_nodes(scene: &ufbx::Scene, node_mesh: &HashMap<u32, u32>) -> Vec<NodeAsset> {
    let mut index_of: HashMap<u32, u32> = HashMap::with_capacity(scene.nodes.len());
    for i in 0..scene.nodes.len() {
        index_of.insert(scene.nodes[i].element.element_id, i as u32);
    }

    let mut nodes: Vec<NodeAsset> = Vec::with_capacity(scene.nodes.len());
    for i in 0..scene.nodes.len() {
        let node: &ufbx::Node = &scene.nodes[i];
        let lt = &node.local_transform;
        nodes.push(NodeAsset {
            index: i as u32,
            name: node.element.name.to_string(),
            path: String::new(),
            parent: node
                .parent
                .as_ref()
                .and_then(|p| index_of.get(&p.element.element_id).copied()),
            position: [lt.translation.x as f32, lt.translation.y as f32, lt.translation.z as f32],
            rotation: [
                lt.rotation.x as f32,
                lt.rotation.y as f32,
                lt.rotation.z as f32,
                lt.rotation.w as f32,
            ],
            scale: [lt.scale.x as f32, lt.scale.y as f32, lt.scale.z as f32],
            mesh: node_mesh.get(&node.element.element_id).copied(),
        });
    }

    // 路径自顶向下拼：先沿父链收集未知段，再回填，避免递归借用。
    let mut paths: Vec<Option<String>> = vec![None; nodes.len()];
    for i in 0..nodes.len() {
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = Some(i);
        while let Some(idx) = cur {
            if let Some(known) = &paths[idx] {
                chain.push(idx);
                let _ = known;
                break;
            }
            chain.push(idx);
            cur = nodes[idx].parent.map(|p| p as usize);
            if chain.len() > 128 {
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

/// 烘焙动画栈：把每个动画栈的节点 TRS 曲线抽成 `AnimClip`。
///
/// 用 ufbx 的 bake 而不是直接读曲线：一是不必在引擎里重实现贝塞尔/阶跃插值，
/// 二是烘焙结果已经把多个 layer 与 blend 结果合到了一起。采样率 30fps，
/// 并开启关键帧精简，避免常量通道产生冗长数据。
fn extract_animations(scene: &ufbx::Scene, nodes: &[NodeAsset]) -> Vec<AnimClip> {
    let mut out: Vec<AnimClip> = Vec::new();
    if scene.anim_stacks.len() == 0 {
        return out;
    }

    // ufbx 节点元素 id -> `scene.nodes` 下标（即 `NodeAsset::index`）
    let mut elem_to_index: HashMap<u32, u32> = HashMap::with_capacity(scene.nodes.len());
    for i in 0..scene.nodes.len() {
        elem_to_index.insert(scene.nodes[i].element.element_id, i as u32);
    }

    for si in 0..scene.anim_stacks.len() {
        let stack: &ufbx::AnimStack = &scene.anim_stacks[si];
        let name = stack.element.name.to_string();
        if stack.layers.len() == 0 {
            // 空栈（只有名字没有 layer）没有可求值内容，按规划直接降级跳过
            log::debug!("动画栈 `{name}` 无 layer，跳过");
            continue;
        }

        let opts = ufbx::BakeOpts {
            resample_rate: 30.0,
            trim_start_time: true,
            key_reduction_enabled: true,
            key_reduction_threshold: 1e-4,
            ..Default::default()
        };
        let baked = match ufbx::bake_anim(scene, &stack.anim, opts) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("动画栈 `{name}` 烘焙失败: {:?} {}", e.type_, e.description);
                continue;
            }
        };

        let mut tracks: Vec<AnimTrack> = Vec::new();
        for bn in baked.nodes.iter() {
            // `BakedNode::element_id` 对应 ufbx 节点元素 id
            let Some(node_index) = elem_to_index.get(&bn.element_id).copied() else {
                continue;
            };
            let bind = &nodes[node_index as usize];
            let keys = bake_node_keys(bn, bind);
            if keys.len() <= 1 {
                // 常量通道（整段不动）无需参与播放，省下每帧写回
                continue;
            }
            tracks.push(AnimTrack {
                node_path: bind.path.clone(),
                node_index,
                keys,
            });
        }
        if tracks.is_empty() {
            log::debug!("动画栈 `{name}` 没有变化的节点通道，跳过");
            continue;
        }

        let mut clip = AnimClip {
            name,
            duration: 0.0,
            tracks,
        };
        for track in &mut clip.tracks {
            r3e_core::anim::rebase_track(track);
        }
        clip.duration = r3e_core::anim::clip_duration(&clip.tracks);
        log::debug!(
            "动画栈 `{}`：{} 条轨道 / {:.2}s",
            clip.name,
            clip.tracks.len(),
            clip.duration
        );
        out.push(clip);
    }
    out
}

/// 把一个烘焙节点的三通道关键帧合并到统一时间轴，输出引擎侧 `AnimKey`。
///
/// 位移/缩放线性插值，旋转球面插值。某通道缺失时用绑定姿态填充。
fn bake_node_keys(bn: &ufbx::BakedNode, bind: &NodeAsset) -> Vec<r3e_core::AnimKey> {
    use r3e_core::AnimKey;

    let mut times: Vec<f64> = Vec::new();
    times.extend(bn.translation_keys.iter().map(|k| k.time));
    times.extend(bn.rotation_keys.iter().map(|k| k.time));
    times.extend(bn.scale_keys.iter().map(|k| k.time));
    if times.is_empty() {
        return Vec::new();
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);

    let bind_pos = Vec3::new(bind.position[0], bind.position[1], bind.position[2]);
    let bind_rot = r3e_core::Quat::from_xyzw(
        bind.rotation[0],
        bind.rotation[1],
        bind.rotation[2],
        bind.rotation[3],
    );
    let bind_scale = Vec3::new(bind.scale[0], bind.scale[1], bind.scale[2]);

    let mut out: Vec<AnimKey> = Vec::with_capacity(times.len());
    for t in times {
        let position = sample_vec3_keys(&bn.translation_keys, t).unwrap_or(bind_pos);
        let rotation = sample_quat_keys(&bn.rotation_keys, t).unwrap_or(bind_rot);
        let scale = sample_vec3_keys(&bn.scale_keys, t).unwrap_or(bind_scale);
        out.push(AnimKey {
            time: t as f32,
            position,
            rotation,
            scale,
        });
    }
    out
}

fn sample_vec3_keys(keys: &[ufbx::BakedVec3], time: f64) -> Option<Vec3> {
    let first = keys.first()?;
    if keys.len() == 1 || time <= first.time {
        return Some(Vec3::new(
            first.value.x as f32,
            first.value.y as f32,
            first.value.z as f32,
        ));
    }
    let last = keys.last()?;
    if time >= last.time {
        return Some(Vec3::new(
            last.value.x as f32,
            last.value.y as f32,
            last.value.z as f32,
        ));
    }
    let mut lo = 0usize;
    let mut hi = keys.len() - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if keys[mid].time <= time {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = &keys[lo];
    let b = &keys[hi];
    let span = b.time - a.time;
    let t = if span > 1e-9 {
        ((time - a.time) / span) as f32
    } else {
        0.0
    };
    let av = Vec3::new(a.value.x as f32, a.value.y as f32, a.value.z as f32);
    let bv = Vec3::new(b.value.x as f32, b.value.y as f32, b.value.z as f32);
    Some(av.lerp(bv, t))
}

fn sample_quat_keys(keys: &[ufbx::BakedQuat], time: f64) -> Option<r3e_core::Quat> {
    let first = keys.first()?;
    let to_quat = |q: &ufbx::Quat| {
        r3e_core::Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32).normalize()
    };
    if keys.len() == 1 || time <= first.time {
        return Some(to_quat(&first.value));
    }
    let last = keys.last()?;
    if time >= last.time {
        return Some(to_quat(&last.value));
    }
    let mut lo = 0usize;
    let mut hi = keys.len() - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if keys[mid].time <= time {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = &keys[lo];
    let b = &keys[hi];
    let span = b.time - a.time;
    let t = if span > 1e-9 {
        ((time - a.time) / span) as f32
    } else {
        0.0
    };
    Some(to_quat(&a.value).slerp(to_quat(&b.value), t))
}

fn extract_material(mat: &ufbx::Material, index: u32, base_dir: &Path) -> MaterialInfo {
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
        metallic: map_scalar(&pbr.metalness).clamp(0.0, 1.0),
        roughness: map_scalar(&pbr.roughness).clamp(0.0, 1.0),
        textures: extract_textures(mat, base_dir),
        base_color_texture: None,
    }
}

/// FBX SDK 缺省 diffuse 因子 0.588（sRGB 近似），用来判断「材质没设颜色」。
fn is_default_fbx_factor(color: [f32; 4]) -> bool {
    const DEFAULT: f32 = 0.588;
    (color[0] - DEFAULT).abs() < 0.01
        && (color[1] - DEFAULT).abs() < 0.01
        && (color[2] - DEFAULT).abs() < 0.01
}

fn map_scalar(map: &ufbx::MaterialMap) -> f32 {
    if map.has_value {
        map.value_vec4.x as f32
    } else {
        0.0
    }
}

fn extract_textures(mat: &ufbx::Material, base_dir: &Path) -> Vec<TextureInfo> {
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
            index: 0,
            element_id: tex.element.element_id,
            slot: slot_name,
            filename: tex.filename.to_string(),
            relative_filename: tex.relative_filename.to_string(),
            absolute_filename: tex.absolute_filename.to_string(),
            embedded: !tex.content.is_empty(),
            content_bytes: tex.content.size,
            has_file: tex.has_file,
            uv_set: tex.uv_set.to_string(),
            image: decode_texture(tex, base_dir),
        });
    }
    out
}

impl TextureInfo {
    /// 复制一份并把扁平表下标写进去（像素是 `Arc`，不会重复占内存）。
    fn clone_with_index(&self, index: u32) -> Self {
        let mut out = self.clone();
        out.index = index;
        out
    }
}

/// 贴图解码：内嵌内容优先 → ufbx 解析出的绝对路径 → 相对 FBX 目录的路径。
///
/// 三级回退全部失败时返回 `None`，由渲染层退化为「纯色 + 默认白图」，绝不 panic。
fn decode_texture(tex: &ufbx::Texture, base_dir: &Path) -> Option<Arc<TextureImage>> {
    // 1) 内嵌在 FBX 里的二进制
    if !tex.content.is_empty() {
        let bytes: &[u8] = &tex.content;
        if let Some(image) = decode_image_bytes(bytes, Some(&tex.filename.to_string())) {
            return Some(image);
        }
        log::warn!(
            "asset: 贴图 `{}` 内嵌数据（{} 字节）解码失败",
            tex.filename,
            tex.content.size
        );
    }

    // 2) 外部文件：绝对路径 → 相对 FBX 目录
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !tex.absolute_filename.is_empty() {
        candidates.push(PathBuf::from(tex.absolute_filename.to_string()));
    }
    for rel in [&tex.relative_filename, &tex.filename] {
        if !rel.is_empty() {
            // FBX 里常见 Windows 反斜杠分隔，先原样拼，再按 `/` 归一化拼一次。
            let raw = rel.to_string();
            candidates.push(base_dir.join(&raw));
            candidates.push(base_dir.join(raw.replace('\\', "/")));
        }
    }

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        match std::fs::read(&path) {
            Ok(bytes) => match decode_image_bytes(&bytes, path.to_str()) {
                Some(image) => return Some(image),
                None => log::warn!("asset: 贴图 `{}` 不是可识别的图片格式", path.display()),
            },
            Err(err) => log::warn!("asset: 读取贴图 `{}` 失败：{err}", path.display()),
        }
    }
    None
}

/// 解码一张贴图。先按文件头嗅探；TGA / BMP 这类没有可靠 magic 的格式靠扩展名兜底。
fn decode_image_bytes(bytes: &[u8], name_hint: Option<&str>) -> Option<Arc<TextureImage>> {
    if bytes.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(bytes).or_else(|err| {
        let ext = name_hint
            .and_then(|name| std::path::Path::new(name).extension())
            .and_then(|ext| ext.to_str())
            .and_then(image::ImageFormat::from_extension);
        match ext {
            Some(format) => image::load_from_memory_with_format(bytes, format).map_err(|inner| {
                log::debug!("按扩展名 {format:?} 解码仍失败: {inner}（嗅探错误: {err}）");
                inner
            }),
            None => Err(err),
        }
    });
    let image = match decoded {
        Ok(image) => image,
        Err(err) => {
            log::debug!("贴图解码失败: {err}");
            return None;
        }
    }
    .to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(Arc::new(TextureImage::new(
        width,
        height,
        image.into_raw(),
    )))
}

/// 从材质槽位里挑一个当 base color：按常见命名优先级取第一个命中的。
fn pick_base_color_texture(textures: &[TextureInfo]) -> Option<u32> {
    const PREFERRED: [&str; 5] = [
        "diffusecolor",
        "basecolor",
        "base_color",
        "albedo",
        "diffuse",
    ];
    for want in PREFERRED {
        if let Some(tex) = textures
            .iter()
            .find(|t| t.slot.to_ascii_lowercase().contains(want))
        {
            return Some(tex.index);
        }
    }
    None
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
    node_index_of: &HashMap<u32, u32>,
) -> Result<(MeshAsset, Option<SkinData>), AssetError> {
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
    // 输出顶点 → ufbx 顶点（控制点）号：蒙皮权重按 ufbx 顶点号索引，必须留映射（P4）
    let mut vertex_pos_index: Vec<u32> = Vec::new();
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
        vertex_pos_index.push(pos_ix);
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

    // ---- 蒙皮（P4）：关节表 + 每顶点 joint/weight ----
    let (skin_data, joints, weights) =
        extract_skinning(mesh, &vertex_pos_index, node_index_of, parent_of, name_of);

    Ok((MeshAsset {
        name: mesh.element.name.to_string(),
        node_path: node_path_of(node.element.element_id, parent_of, name_of),
        positions,
        normals,
        uvs,
        indices,
        material_parts,
        aabb_min: [aabb_min.x, aabb_min.y, aabb_min.z],
        aabb_max: [aabb_max.x, aabb_max.y, aabb_max.z],
        joints,
        weights,
        // 真正下标由 import_fbx 在收集骨骼表时回填
        skin: skin_data.as_ref().map(|_| 0),
        node_local_matrix: world.to_cols_array_2d(),
        source_path: source_path.to_string(),
        source_vertex_count: mesh.num_vertices,
        source_face_count: mesh.num_faces,
        source_triangle_count: mesh.num_triangles,
    }, skin_data))
}

/// 抽取一个网格的蒙皮（P4）。
///
/// ufbx 的权重是**按 ufbx 顶点号（控制点）聚集**的：
/// `deformer.vertices[v]` 给出该顶点在 `deformer.weights` 里的区间
/// `[weight_begin, weight_begin + num_weights)`，区间内每项是
/// `(cluster_index, weight)`。而引擎顶点是按角点拆分的，所以要用
/// `vertex_pos_index` 把每个输出顶点映射回 ufbx 顶点号。
///
/// 返回 `(骨骼表, 每顶点关节槽位, 每顶点权重)`；无蒙皮时返回 `(None, vec![], vec![])`。
fn extract_skinning(
    mesh: &ufbx::Mesh,
    vertex_pos_index: &[u32],
    node_index_of: &HashMap<u32, u32>,
    parent_of: &HashMap<u32, u32>,
    name_of: &HashMap<u32, String>,
) -> (Option<SkinData>, Vec<[u16; 4]>, Vec<[f32; 4]>) {
    let Some(deformer) = mesh.skin_deformers.first() else {
        return (None, Vec::new(), Vec::new());
    };
    if mesh.skin_deformers.len() > 1 {
        log::warn!(
            "网格 `{}` 有 {} 个 skin deformer，只使用第一个",
            mesh.element.name,
            mesh.skin_deformers.len()
        );
    }

    // cluster 下标 → 关节槽位（骨骼表下标）；拿不到骨骼节点的 cluster 直接丢弃
    let mut slot_of_cluster: Vec<Option<u16>> = vec![None; deformer.clusters.len()];
    let mut joints: Vec<SkinJoint> = Vec::new();
    for (ci, cluster) in deformer.clusters.iter().enumerate() {
        let Some(bone) = cluster.bone_node.as_ref() else {
            continue;
        };
        let bone_id = bone.element.element_id;
        let Some(&node_index) = node_index_of.get(&bone_id) else {
            log::warn!(
                "蒙皮 cluster `{}` 的骨骼节点不在节点树里，已跳过",
                cluster.element.name
            );
            continue;
        };
        if joints.len() >= r3e_core::MAX_JOINT_MATRICES {
            log::warn!("关节数超过上限 {}，后续关节被丢弃", r3e_core::MAX_JOINT_MATRICES);
            break;
        }
        slot_of_cluster[ci] = Some(joints.len() as u16);
        joints.push(SkinJoint {
            node_index,
            path: node_path_of(bone_id, parent_of, name_of),
            bind_world: mat4_from(&cluster.bind_to_world),
        });
    }

    if joints.is_empty() {
        return (None, Vec::new(), Vec::new());
    }

    let mut joints_out: Vec<[u16; 4]> = Vec::with_capacity(vertex_pos_index.len());
    let mut weights_out: Vec<[f32; 4]> = Vec::with_capacity(vertex_pos_index.len());
    let mut overflow = 0usize;
    let mut orphan = 0usize;

    for &pos_ix in vertex_pos_index {
        let mut acc: Vec<(u16, f32)> = Vec::new();
        if let Some(sv) = deformer.vertices.get(pos_ix as usize) {
            let begin = sv.weight_begin as usize;
            let end = begin + sv.num_weights as usize;
            if sv.num_weights as usize > 4 {
                overflow += 1;
            }
            for k in begin..end.min(deformer.weights.len()) {
                let w = deformer.weights[k];
                let Some(Some(slot)) = slot_of_cluster.get(w.cluster_index as usize) else {
                    continue;
                };
                let weight = w.weight as f32;
                if weight > 0.0 {
                    acc.push((*slot, weight));
                }
            }
        }
        if acc.is_empty() {
            orphan += 1;
        }
        // 权重大的优先保留前 4 个，再归一化（保证着色器里 sum(weight)=1）
        acc.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        acc.truncate(4);
        let sum: f32 = acc.iter().map(|(_, w)| *w).sum();
        let mut j = [0u16; 4];
        let mut wt = [0.0f32; 4];
        if sum > 1e-6 {
            for (i, (slot, w)) in acc.iter().enumerate() {
                j[i] = *slot;
                wt[i] = w / sum;
            }
        }
        joints_out.push(j);
        weights_out.push(wt);
    }

    if overflow > 0 {
        log::warn!("网格 `{}` 有 {overflow} 个顶点权重数超过 4，已截断", mesh.element.name);
    }
    if orphan > 0 {
        log::debug!("网格 `{}` 有 {orphan} 个顶点没有有效权重（按静止位置绘制）", mesh.element.name);
    }

    let data = SkinData {
        name: if deformer.element.name.is_empty() {
            mesh.element.name.to_string()
        } else {
            deformer.element.name.to_string()
        },
        joints,
    };
    (Some(data), joints_out, weights_out)
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
                let decoded = match tex.image.as_deref() {
                    Some(img) => format!("已解码 {}x{}", img.width, img.height),
                    None => "未解码（渲染回退纯色）".to_string(),
                };
                let _ = writeln!(
                    out,
                    "       - [{}] {} ({}, {}) 原始引用: {}",
                    tex.slot,
                    if tex.filename.is_empty() {
                        "<未命名>"
                    } else {
                        tex.filename.as_str()
                    },
                    state,
                    decoded,
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

#[cfg(test)]
mod skin_tests {
    use super::*;

    /// 真实样本：老虎 FBX 的蒙皮提取结果自洽性检查。
    /// 样本不存在时直接跳过（该文件来自本机游戏 Mod，不入库）。
    const SAMPLE: &str =
        "D:/steam/steamapps/common/AmazingCultivationSimulator/Mods/Example/Resources/Mod/Npc/Monster/tirger/tirger.fbx";

    #[test]
    fn extract_skinning_from_sample() {
        if !std::path::Path::new(SAMPLE).exists() {
            eprintln!("跳过：样本 {SAMPLE} 不存在");
            return;
        }
        let entry = import_fbx(std::path::Path::new(SAMPLE)).expect("导入样本失败");
        let mesh = &entry.meshes[0];
        let skin_index = mesh.skin.expect("网格应带蒙皮槽位");
        let skin = entry.skin(skin_index).expect("蒙皮数据应存在");

        assert!(!skin.is_empty(), "关节表不应为空");
        assert_eq!(mesh.joints.len(), mesh.positions.len(), "每顶点一组关节下标");
        assert_eq!(mesh.weights.len(), mesh.positions.len(), "每顶点一组权重");

        let joint_count = skin.joint_count() as u16;
        let mut weighted = 0usize;
        for (vi, w) in mesh.weights.iter().enumerate() {
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                weighted += 1;
                assert!((sum - 1.0).abs() < 1e-3, "顶点 {vi} 权重未归一化: {sum}");
            }
            for (k, j) in mesh.joints[vi].iter().enumerate() {
                if mesh.weights[vi][k] > 0.0 {
                    assert!(*j < joint_count, "顶点 {vi} 关节下标越界: {j}");
                }
            }
        }
        assert!(weighted > 0, "至少应有顶点带权重");
        // 角点拆分后每个引擎顶点都应从 ufbx 顶点继承权重
        assert!(weighted <= mesh.joints.len());
    }
}
