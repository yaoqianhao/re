//! 材质引用体系的校验契约（M1-S4-A）。
//!
//! 立场与 `tests/validate.rs` 一致：每条负例从「合法材质场景」出发只破坏一个约束，
//! 断言必须出现指定 code；正向用例确认不误报。
//!
//! `assert_error` / `assert_warn` 的两个参数顺序是硬约定——`xtask invariants` 按
//! 「首个字符串实参 = code」扫源码，换写法会让能力清单漂移。

use std::path::PathBuf;

use r3e_core::{ObjectDesc, Scene, SceneMaterialDecl, SurfaceMaterial};
use r3e_doc::{Document, Severity};

fn decl(name: &str) -> SceneMaterialDecl {
    SceneMaterialDecl {
        name: name.to_string(),
        ..SceneMaterialDecl::default()
    }
}

/// 建一份「材质表 + 一个引用它的节点」的合法场景。
fn good_doc() -> Document {
    let mut scene = Scene::new();
    let _ = r3e_core::scene_material::upsert_material(&mut scene.materials, decl("brick"));
    scene
        .spawn(ObjectDesc {
            name: "wall".to_string(),
            material_ref: Some("brick".to_string()),
            ..ObjectDesc::default()
        })
        .expect("wall 应当建成功");
    Document::new(scene)
}

fn codes(doc: &Document) -> Vec<&'static str> {
    doc.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.code)
        .collect()
}

fn warn_codes(doc: &Document) -> Vec<&'static str> {
    doc.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Warn)
        .map(|i| i.code)
        .collect()
}

fn assert_error(doc: &Document, code: &str) {
    let found = codes(doc);
    assert!(found.contains(&code), "期望 {code}，实得 {found:?}");
}

fn assert_warn(doc: &Document, code: &str) {
    let found = warn_codes(doc);
    assert!(found.contains(&code), "期望 warn {code}，实得 {found:?}");
}

// ---------- 负向 ----------

#[test]
fn dup_material_name_is_error() {
    let mut scene = Scene::new();
    // 绕过 upsert 的去重，直接塞两条同名——模拟热重载直写 Vec。
    scene.materials = vec![decl("brick"), decl("brick")];
    let doc = Document::new(scene);
    assert_error(&doc, "material.dup_name");
}

#[test]
fn params_out_of_range_is_error() {
    let mut scene = Scene::new();
    scene.materials = vec![decl("brick")];
    scene.materials[0].metallic = 3.0;
    let doc = Document::new(scene);
    assert_error(&doc, "material.params_out_of_range");
}

#[test]
fn material_ref_missing_is_error() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "wall".to_string(),
            material_ref: Some("nope".to_string()),
            ..ObjectDesc::default()
        })
        .expect("wall 应当建成功");
    let doc = Document::new(scene);
    assert_error(&doc, "material.ref_missing");
}

#[test]
fn ref_and_inline_material_conflict_is_error() {
    let mut doc = good_doc();
    let id = doc.find("wall").expect("wall 应当存在").id;
    doc.write(|s| {
        s.get_mut(id).unwrap().material = Some(SurfaceMaterial::default());
    });
    assert_error(&doc, "material.ref_conflict");
}

#[test]
fn texture_missing_is_error_only_with_dir() {
    let dir = temp_dir("texture_missing");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ok.png"), b"png").unwrap();

    let mut scene = Scene::new();
    let mut m = decl("brick");
    m.albedo = Some("ok.png".to_string());
    m.normal = Some("gone.png".to_string());
    let _ = r3e_core::scene_material::upsert_material(&mut scene.materials, m);
    let mut doc = Document::new(scene);

    assert!(
        !codes(&doc).contains(&"material.texture_missing"),
        "未登记场景目录时不该判存在性"
    );

    doc.set_base_dir(&dir);
    assert_error(&doc, "material.texture_missing");
    assert_eq!(doc.base_dir(), Some(dir.as_path()));

    let _ = std::fs::remove_dir_all(&dir);
}

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("r3e_s4a_{tag}_{}", std::process::id()))
}

// ---------- 正向 ----------

#[test]
fn unreferenced_material_is_warn_not_error() {
    let mut scene = Scene::new();
    let _ = r3e_core::scene_material::upsert_material(&mut scene.materials, decl("brick"));
    let doc = Document::new(scene);
    assert_warn(&doc, "material.unreferenced");
    assert!(
        codes(&doc).is_empty(),
        "未引用只是可疑，不该有 Error：{:?}",
        codes(&doc)
    );
}

#[test]
fn good_material_scene_has_no_issues() {
    let doc = good_doc();
    assert!(
        doc.validate().is_empty(),
        "合法材质场景不该报问题：{:?}",
        doc.validate()
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
    );
}
