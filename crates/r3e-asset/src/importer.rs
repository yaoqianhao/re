//! 导入器注册表（S2）：**按扩展名分派，所有导入器只产出 `AssetEntry`**。
//!
//! 设计要点：
//!
//! * 导入器接口只有三件事——名字、支持的扩展名、把文件读成 `AssetEntry`；
//! * 不允许导入器在 `AssetEntry` 之外携带「格式专属」的数据结构，否则渲染 / 物理 /
//!   脚本又会被迫认识来源；
//! * 每次导入结束后统一跑一遍 AIR 审计（[`audit_entry`]）。S2 阶段**只告警不拦截**，
//!   等 glb 接入（S3）稳定后再由 `strict` 开关升级为强制闸门。

use std::path::Path;

use r3e_core::air::{validate_parts, validate_with, AirIssue};
use r3e_core::mesh::MeshPartData;

use crate::gltf_import::GltfImporter;
use crate::{import_fbx, AssetEntry, AssetError};

/// 一个格式的导入器。
pub trait AssetImporter: Send + Sync {
    /// 格式名（日志与 `--list-importers` 用）。
    fn name(&self) -> &'static str;

    /// 支持的扩展名（小写、不含点）。
    fn extensions(&self) -> &'static [&'static str];

    /// 把文件导入成 AIR 资产条目。
    fn import(&self, path: &Path) -> Result<AssetEntry, AssetError>;
}

/// FBX 导入器：复用既有的 `import_fbx`，不新增任何格式分支。
#[derive(Debug, Default, Clone, Copy)]
pub struct FbxImporter;

impl AssetImporter for FbxImporter {
    fn name(&self) -> &'static str {
        "fbx"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["fbx"]
    }

    fn import(&self, path: &Path) -> Result<AssetEntry, AssetError> {
        import_fbx(path)
    }
}

/// 导入器注册表：扩展名 → 导入器，未知扩展名显式报错（不静默失败）。
#[derive(Default)]
pub struct ImporterRegistry {
    importers: Vec<Box<dyn AssetImporter>>,
    /// 审计出违规时是否拒绝导入。S2 默认 `false`（只告警）。
    strict: bool,
}

impl std::fmt::Debug for ImporterRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImporterRegistry")
            .field(
                "importers",
                &self
                    .importers
                    .iter()
                    .map(|i| (i.name(), i.extensions()))
                    .collect::<Vec<_>>(),
            )
            .field("strict", &self.strict)
            .finish()
    }
}

impl ImporterRegistry {
    /// 空注册表（测试或完全自定义的场景）。
    pub fn empty() -> Self {
        Self {
            importers: Vec::new(),
            strict: false,
        }
    }

    /// 引擎默认注册表：FBX + glTF/GLB。
    ///
    /// 两个导入器都只产出 `AssetEntry`，下游（渲染 / 物理 / 脚本）无法分辨来源。
    pub fn with_builtin() -> Self {
        let mut registry = Self::empty();
        registry.register(Box::new(FbxImporter));
        registry.register(Box::new(GltfImporter));
        registry
    }

    pub fn register(&mut self, importer: Box<dyn AssetImporter>) -> &mut Self {
        self.importers.push(importer);
        self
    }

    /// 开启后，AIR 审计不通过即拒绝导入（S3 之后的默认口径）。
    pub fn set_strict(&mut self, strict: bool) {
        self.strict = strict;
    }

    pub fn is_strict(&self) -> bool {
        self.strict
    }

    pub fn importers(&self) -> &[Box<dyn AssetImporter>] {
        &self.importers
    }

    /// 已注册的全部扩展名（去重、升序），供 CLI / 文档生成。
    pub fn supported_extensions(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .importers
            .iter()
            .flat_map(|i| i.extensions().iter().map(|e| e.to_string()))
            .collect();
        all.sort();
        all.dedup();
        all
    }

    pub fn find(&self, extension: &str) -> Option<&dyn AssetImporter> {
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        self.importers
            .iter()
            .find(|i| i.extensions().iter().any(|e| *e == ext))
            .map(|i| i.as_ref())
    }

    /// 按扩展名分派导入。未知扩展名 → [`AssetError::UnsupportedFormat`]。
    pub fn import(&self, path: &Path) -> Result<AssetEntry, AssetError> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let Some(importer) = self.find(&ext) else {
            return Err(AssetError::UnsupportedFormat {
                path: path.display().to_string(),
                extension: ext,
                supported: self.supported_extensions(),
            });
        };
        let entry = importer.import(path)?;
        let issues = audit_entry(&entry);
        if !issues.is_empty() {
            for issue in &issues {
                log::warn!("AIR 审计：资产 {} {}", entry.id, issue);
            }
            if self.strict {
                return Err(AssetError::AirViolation {
                    path: path.display().to_string(),
                    count: issues.len(),
                    first: issues
                        .first()
                        .map(|i| i.to_string())
                        .unwrap_or_default(),
                });
            }
        }
        Ok(entry)
    }
}

/// 对一个资产条目做 AIR 审计：逐网格校验不变量与材质分区。
///
/// 返回人类可读的违规描述；空表示全部通过。
pub fn audit_entry(entry: &AssetEntry) -> Vec<String> {
    let mut issues: Vec<String> = Vec::new();
    for (index, mesh) in entry.meshes.iter().enumerate() {
        let data = mesh.to_mesh_data();
        // 关节上限按「该网格自己引用的骨骼表」取，避免多骨架文件误报。
        let joints = mesh
            .skin
            .and_then(|slot| entry.skins.get(slot as usize))
            .or_else(|| entry.skins.first())
            .map(|s| s.joint_count());
        for issue in validate_with(&data, joints) {
            issues.push(format!("mesh #{index}({}) {issue}", mesh.name));
        }
        for issue in validate_parts(&data, &mesh_parts(entry.id, mesh)) {
            issues.push(format!("mesh #{index}({}) {issue}", mesh.name));
        }
    }
    issues
}

fn mesh_parts(asset: r3e_core::AssetId, mesh: &crate::MeshAsset) -> Vec<MeshPartData> {
    mesh.material_parts
        .iter()
        .map(|part| MeshPartData {
            first_index: part.triangle_offset * 3,
            index_count: part.triangle_count * 3,
            material: part.material_index.map(|m| (asset, m)),
        })
        .collect()
}

/// 供 `MeshAsset` 之外的调用方复用的审计入口（例如 `--dump-asset`）。
pub fn audit_issues(entry: &AssetEntry) -> Vec<AirIssue> {
    let mut out = Vec::new();
    for mesh in &entry.meshes {
        let data = mesh.to_mesh_data();
        out.extend(validate_with(&data, entry.skins.first().map(|s| s.joint_count())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn unknown_extension_is_rejected_explicitly() {
        let registry = ImporterRegistry::with_builtin();
        let err = registry
            .import(&PathBuf::from("model.obj"))
            .expect_err("未知扩展名必须显式报错");
        match err {
            AssetError::UnsupportedFormat { extension, supported, .. } => {
                assert_eq!(extension, "obj");
                assert!(supported.contains(&"fbx".to_string()));
            }
            other => panic!("意外的错误类型: {other:?}"),
        }
    }

    #[test]
    fn extension_lookup_is_case_insensitive() {
        let registry = ImporterRegistry::with_builtin();
        assert!(registry.find("FBX").is_some());
        assert!(registry.find(".Fbx").is_some());
        assert!(registry.find("glb").is_some());
        assert!(registry.find("GLTF").is_some());
        assert_eq!(
            registry.supported_extensions(),
            vec!["fbx".to_string(), "glb".to_string(), "gltf".to_string()]
        );
    }

    /// 构造一个只含单个三角形的最小 `AssetEntry`，用于审计测试。
    fn triangle_entry(normals: Vec<[f32; 3]>, indices: Vec<u32>) -> AssetEntry {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let mesh = crate::MeshAsset {
            name: "tri".to_string(),
            node_path: "tri".to_string(),
            positions: positions.clone(),
            normals,
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices,
            material_parts: vec![crate::MaterialPart {
                material_index: None,
                material_name: String::new(),
                triangle_offset: 0,
                triangle_count: 1,
            }],
            aabb_min: [0.0, 0.0, 0.0],
            aabb_max: [1.0, 1.0, 0.0],
            node_local_matrix: r3e_core::Mat4::IDENTITY.to_cols_array_2d(),
            source_path: "memory".to_string(),
            joints: vec![[0; 4]; 3],
            weights: vec![[0.0; 4]; 3],
            skin: None,
            source_vertex_count: 3,
            source_face_count: 1,
            source_triangle_count: 1,
        };
        AssetEntry {
            id: r3e_core::AssetId(1),
            source_path: "memory".to_string(),
            meshes: vec![mesh],
            materials: Vec::new(),
            textures: Vec::new(),
            nodes: Vec::new(),
            animations: Vec::new(),
            skins: Vec::new(),
        }
    }

    #[test]
    fn audit_accepts_a_well_formed_triangle() {
        let entry = triangle_entry(
            vec![[0.0, 0.0, 1.0]; 3],
            vec![0, 1, 2],
        );
        assert!(audit_entry(&entry).is_empty(), "{:?}", audit_entry(&entry));
    }

    #[test]
    fn audit_flags_bad_normal_and_bad_index() {
        let entry = triangle_entry(
            vec![[0.0, 0.0, 3.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
            vec![0, 1, 99],
        );
        let issues = audit_entry(&entry);
        assert!(!issues.is_empty(), "坏法线 + 越界索引必须被审计捕获");
    }

    #[test]
    fn registry_is_strict_off_by_default() {
        let registry = ImporterRegistry::with_builtin();
        assert!(!registry.is_strict());
    }
}
