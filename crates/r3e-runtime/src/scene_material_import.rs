//! 场景资源进库：网格资产与材质贴图的导入通道（M1-S4-A）。
//!
//! 为什么单独成文件：`app.rs` 是「窗口 / 事件循环 / 主循环」那一摊，已经挂在体量
//! 门禁的债务白名单上（不得再长）；而这两个导入函数只关心「文件 → `AssetLibrary`」，
//! 与主循环没有交集，挪出来两边都更好读。

use std::path::{Path, PathBuf};

use r3e_asset::AssetLibrary;
use r3e_core::Scene;

use crate::app::AssetSpec;

/// 材质贴图的基准目录：`--assets-root` 优先，否则退回场景文件所在目录。
///
/// 两者共用一份：贴图与网格资产指向同一套资源树。若贴图按场景目录算、网格按
/// `--assets-root` 算，同一份场景在两种启动方式下会算出两套 `AssetId`。
pub(crate) fn texture_base_dir(scene_path: &Path, assets_root: Option<&Path>) -> PathBuf {
    assets_root.map(Path::to_path_buf).unwrap_or_else(|| {
        scene_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    })
}

/// 按顺序导入资产；导入失败只记日志，不影响其余资产与启动流程。
///
/// 同一路径可能被命令行与多个场景声明同时引用（例如只想换 transform、或 `spawn: false`
/// 仅供脚本引用），这里按 `AssetId` 去重，避免重复解析同一个 FBX。
pub(crate) fn load_assets(specs: &[AssetSpec]) -> AssetLibrary {
    let mut library = AssetLibrary::new();
    for spec in specs {
        let key = spec.path.display().to_string();
        let id = r3e_core::AssetId::from_path(&key);
        if library.get(id).is_some() {
            log::debug!("资产已导入过，跳过重复加载: {key}");
            continue;
        }
        match library.load(&spec.path) {
            Ok(id) => {
                let (meshes, tris) = library
                    .get(id)
                    .map(|e| (e.meshes.len(), e.triangle_total()))
                    .unwrap_or((0, 0));
                log::info!(
                    "资产已导入: {} (id {}) — {} 个网格 / {} 三角形",
                    spec.path.display(),
                    id,
                    meshes,
                    tris
                );
            }
            Err(e) => log::error!("资产导入失败 {}: {e}", spec.path.display()),
        }
    }
    library
}

/// S4-A：把场景材质表里出现的贴图逐个导入资产库。
///
/// 为什么必须在启动时做一次：材质引用走的是「路径 → `AssetId` 散列」这条无表协议，
/// 渲染层只负责算出 id 并去库里取像素。库里没有这条记录时贴图会静默变成白色
/// （`MaterialCache` 找不到就绑白图），肉眼只能看出「材质没生效」，很难定位到
/// 「忘了导入」。所以载入场景时统一补一遍。
///
/// 与网格资产同样的去重口径：同一条路径被多个材质引用时只解码一次
/// （`import_texture` 内部按 `AssetId` 去重）。
pub(crate) fn load_scene_material_textures(
    library: &mut AssetLibrary,
    scene: &Scene,
    base_dir: &Path,
) {
    if scene.materials.is_empty() {
        return;
    }
    // 收集去重后的路径：同一张图被 albedo 与 emissive 同时引用很常见，
    // 按解析后的散列路径去重，与 `AssetId` 的口径完全一致。
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut imported = 0u32;
    for decl in &scene.materials {
        for rel in decl.texture_paths().into_iter().flatten() {
            let resolved = r3e_core::resolve_texture_path(Some(base_dir), rel);
            if !seen.insert(resolved.clone()) {
                continue;
            }
            match r3e_asset::import_texture(library, Path::new(&resolved)) {
                Ok(texture) => {
                    log::info!(
                        "材质贴图已导入: {resolved} (id {}) — 槽位 {:?}{}",
                        texture.asset,
                        texture.slot,
                        if texture.reused { "（复用）" } else { "" }
                    );
                    imported += 1;
                }
                Err(e) => log::warn!("材质贴图导入失败 {resolved}: {e}"),
            }
        }
    }
    if imported > 0 {
        log::info!("材质贴图共导入 {imported} 张");
    }
}
