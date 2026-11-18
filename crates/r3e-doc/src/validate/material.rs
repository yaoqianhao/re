//! 场景材质资源引用体系的自洽校验（M1-S4-A）。
//!
//! 这一层只管「文档自己说的通不通」，不碰渲染、不解码贴图：
//!
//! - `material.dup_name`：`materials[].name` 重复（`normalize_all` 本该去重，
//!   但热重载、脚本直写 `Vec`、外部反序列化都可能绕过那层保护，校验器兜住）；
//! - `material.ref_missing`：节点 `materialRef` 指向不存在的材质声明；
//! - `material.ref_conflict`：节点同时写了 `materialRef` 与内联 `material`，
//!   两条渲染路径会打架（内联优先还是引用优先没有共识，直接判死）；
//! - `material.texture_missing`：声明的贴图路径在场景目录下找不到文件
//!   （**只判存在性，不解码**；需要场景目录，见 [`check_in_dir`]）；
//! - `material.params_out_of_range`：标量越界或非有限（写作层已收敛，这里是兜底网）；
//! - `material.unreferenced`：材质声明没有任何节点引用（可疑但可用，`Warn`）。
//!
//! 存在性判定需要「场景文件所在目录」，而 [`super::validate`] 的契约是纯 `&Scene`，
//! 所以拆成两个入口：无目录的 [`check`] 跳过贴图存在性，带目录的 [`check_in_dir`]
//! 才做文件系统判定。这样纯函数语义（不 IO、可在任意上下文跑）不被破坏。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use r3e_core::{Scene, MAX_EMISSIVE, MIN_ROUGHNESS};

use super::Issue;

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    check_texture_or_not(scene, None, out);
}

/// 带场景目录的校验：额外判定贴图路径所指文件是否真的存在。
pub(crate) fn check_in_dir(scene: &Scene, dir: &Path, out: &mut Vec<Issue>) {
    check_texture_or_not(scene, Some(dir), out);
}

fn check_texture_or_not(scene: &Scene, dir: Option<&Path>, out: &mut Vec<Issue>) {
    check_dup_name(scene, out);
    check_params(scene, out);
    if let Some(dir) = dir {
        check_textures(scene, dir, out);
    }
    check_refs(scene, out);
    check_unreferenced(scene, out);
}

fn check_dup_name(scene: &Scene, out: &mut Vec<Issue>) {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (i, decl) in scene.materials.iter().enumerate() {
        if !seen.insert(decl.name.as_str()) {
            out.push(Issue::error(
                "material.dup_name",
                format!("materials[{i}].name"),
                format!("材质名「{}」重复，`materialRef` 无法唯一定位", decl.name),
            ));
        }
    }
}

/// 标量越界 / 非有限。口径与 `SurfaceMaterial::sanitized` 完全一致，只是这里**不改值**、
/// 只报告：写入层负责收敛，校验层负责告诉用户「这份场景文件不干净」。
fn check_params(scene: &Scene, out: &mut Vec<Issue>) {
    for (i, decl) in scene.materials.iter().enumerate() {
        if !matches!(decl.metallic, m if m.is_finite() && (0.0..=1.0).contains(&m)) {
            out.push(Issue::error(
                "material.params_out_of_range",
                format!("materials[{i}].metallic"),
                format!(
                    "材质「{}」金属度 {} 越界（应在 0..=1 且有限）",
                    decl.name, decl.metallic
                ),
            ));
        }
        if !matches!(decl.roughness, r if r.is_finite() && (MIN_ROUGHNESS..=1.0).contains(&r)) {
            out.push(Issue::error(
                "material.params_out_of_range",
                format!("materials[{i}].roughness"),
                format!(
                    "材质「{}」粗糙度 {} 越界（应在 {}..=1 且有限）",
                    decl.name, decl.roughness, MIN_ROUGHNESS
                ),
            ));
        }
        for (ch, v) in decl.emissive.iter().enumerate() {
            if !matches!(v, e if e.is_finite() && (0.0..=MAX_EMISSIVE).contains(e)) {
                out.push(Issue::error(
                    "material.params_out_of_range",
                    format!("materials[{i}].emissive[{ch}]"),
                    format!(
                        "材质「{}」自发光第 {ch} 通道 {} 越界（应在 0..={} 且有限）",
                        decl.name, v, MAX_EMISSIVE
                    ),
                ));
            }
        }
        for (axis, v) in decl.uv_scale.iter().enumerate() {
            if !matches!(
                v,
                s if s.is_finite() && (r3e_core::scene_material::UV_SCALE_MIN..=r3e_core::scene_material::UV_SCALE_MAX).contains(s)
            ) {
                out.push(Issue::error(
                    "material.params_out_of_range",
                    format!("materials[{i}].uvScale[{axis}]"),
                    format!(
                        "材质「{}」uv 缩放第 {axis} 轴 {} 越界（应在 {}..={} 且有限）",
                        decl.name,
                        v,
                        r3e_core::scene_material::UV_SCALE_MIN,
                        r3e_core::scene_material::UV_SCALE_MAX
                    ),
                ));
            }
        }
    }
}

/// 贴图存在性：相对路径按场景目录拼，绝对路径原样判。只看文件在不在，不解码。
fn check_textures(scene: &Scene, dir: &Path, out: &mut Vec<Issue>) {
    for (i, decl) in scene.materials.iter().enumerate() {
        for (slot, rel) in decl.textures() {
            let path = resolve(dir, rel);
            if !path.is_file() {
                out.push(Issue::error(
                    "material.texture_missing",
                    format!("materials[{i}].{slot}"),
                    format!(
                        "材质「{}」的 {slot} 贴图不存在：{}",
                        decl.name,
                        path.display()
                    ),
                ));
            }
        }
    }
}

fn resolve(dir: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

fn check_refs(scene: &Scene, out: &mut Vec<Issue>) {
    let declared: BTreeSet<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
    for node in scene.iter() {
        let Some(name) = node.material_ref.as_deref() else {
            continue;
        };
        if !declared.contains(name) {
            out.push(Issue::error(
                "material.ref_missing",
                format!("nodes[{}].materialRef", node.id),
                format!(
                    "节点「{}」引用的材质「{}」未在 materials 段声明",
                    node.name, name
                ),
            ));
        }
        if node.material.is_some() {
            out.push(Issue::error(
                "material.ref_conflict",
                format!("nodes[{}].material", node.id),
                format!(
                    "节点「{}」同时写了 materialRef 与内联 material，两条路径互斥",
                    node.name
                ),
            ));
        }
    }
}

fn check_unreferenced(scene: &Scene, out: &mut Vec<Issue>) {
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for node in scene.iter() {
        if let Some(name) = node.material_ref.as_deref() {
            used.insert(name);
        }
    }
    for (i, decl) in scene.materials.iter().enumerate() {
        if !used.contains(decl.name.as_str()) {
            out.push(Issue::warn(
                "material.unreferenced",
                format!("materials[{i}].name"),
                format!("材质「{}」没有任何节点引用", decl.name),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::{ObjectDesc, SceneMaterialDecl};

    fn scene_with(materials: Vec<SceneMaterialDecl>) -> Scene {
        let mut scene = Scene::new();
        for m in materials {
            let _ = r3e_core::scene_material::upsert_material(&mut scene.materials, m);
        }
        scene
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    fn decl(name: &str) -> SceneMaterialDecl {
        SceneMaterialDecl {
            name: name.to_string(),
            ..SceneMaterialDecl::default()
        }
    }

    #[test]
    fn 未被引用的材质报warn() {
        let scene = scene_with(vec![decl("brick")]);
        let mut out = Vec::new();
        check(&scene, &mut out);
        assert_eq!(codes(&out), vec!["material.unreferenced"]);
        assert!(!out[0].is_error(), "未引用只是可疑，不该判 Error");
    }

    #[test]
    fn 重复名与越界标量各有反例() {
        let mut raw = Scene::new();
        // 绕过 upsert 的收敛/去重，直接塞脏数据——模拟热重载直写 Vec。
        raw.materials = vec![decl("brick"), decl("brick")];
        raw.materials[0].metallic = 2.0;
        raw.materials[1].roughness = f32::NAN;
        let mut out = Vec::new();
        check(&raw, &mut out);
        let got = codes(&out);
        assert_eq!(got.iter().filter(|c| **c == "material.dup_name").count(), 1);
        assert_eq!(
            got.iter()
                .filter(|c| **c == "material.params_out_of_range")
                .count(),
            2
        );
    }

    #[test]
    fn 悬空引用与内联冲突都判error() {
        let mut scene = scene_with(vec![decl("brick")]);
        let id = scene
            .spawn(ObjectDesc {
                name: "wall".to_string(),
                material_ref: Some("brick".to_string()),
                ..ObjectDesc::default()
            })
            .expect("spawn wall");
        let bad = scene
            .spawn(ObjectDesc {
                name: "floor".to_string(),
                material_ref: Some("nope".to_string()),
                material: Some(r3e_core::SurfaceMaterial::default()),
                ..ObjectDesc::default()
            })
            .expect("spawn floor");
        let mut out = Vec::new();
        check(&scene, &mut out);
        let got = codes(&out);
        assert!(got.contains(&"material.ref_missing"), "got: {got:?}");
        assert!(got.contains(&"material.ref_conflict"), "got: {got:?}");
        assert!(
            !out.iter()
                .any(|i| i.path == format!("nodes[{id}].materialRef")),
            "合法引用不该报错"
        );
        assert!(out
            .iter()
            .any(|i| i.path == format!("nodes[{bad}].material")));
    }

    #[test]
    fn 贴图存在性只在带目录时判定() {
        let dir = std::env::temp_dir().join("r3e_s4a_validate_tex");
        let _ = std::fs::create_dir_all(&dir);
        let have = dir.join("ok.png");
        std::fs::write(&have, b"png").unwrap();

        let mut scene = scene_with(vec![]);
        let mut m = decl("brick");
        m.albedo = Some("ok.png".to_string());
        m.normal = Some("gone.png".to_string());
        let _ = r3e_core::scene_material::upsert_material(&mut scene.materials, m);

        let mut plain = Vec::new();
        check(&scene, &mut plain);
        assert!(
            !codes(&plain).contains(&"material.texture_missing"),
            "无目录时不该判存在性"
        );

        let mut with_dir = Vec::new();
        check_in_dir(&scene, &dir, &mut with_dir);
        let missing: Vec<&Issue> = with_dir
            .iter()
            .filter(|i| i.code == "material.texture_missing")
            .collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].path, "materials[0].normal");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
