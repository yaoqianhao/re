//! Validator v0 的负向铁律 + 正向不误报。
//!
//! 负向用例的立场：**校验器漏报比误报危险得多**。所以每条负向都从「合法场景」出发
//! 只破坏一个约束，断言必须出现指定 code 的 `Error`——不写「有 Error 就算过」的宽松断言，
//! 否则某天规则挂掉了测试照样绿。
//!
//! 正向用例跑仓库里真实在用的场景文件：校验器不能是「什么都报错」的假阳性机器。

use std::sync::Arc;

use r3e_core::{
    anim::{AnimBinding, AnimClip, AnimInstance},
    morph::MorphInstance,
    skin::SkinInstance,
    LocalLight, NodeId, ObjectDesc, PhysicsSceneDesc, ScreenText, SurfaceMaterial, UiElement, Vec4,
};
use r3e_doc::{Document, Severity};

fn doc_with_node() -> Document {
    let mut doc = Document::default();
    doc.write(|s| {
        s.spawn(ObjectDesc {
            name: "cube".to_string(),
            ..Default::default()
        })
        .expect("cube 应当建成功")
    });
    doc
}

fn id_of(doc: &Document, name: &str) -> NodeId {
    doc.find(name).expect("节点应当存在").id
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

/// 同一套「只破坏一个约束」的立场，只是断言 `Warn`——`Error` 会掩盖 `Warn` 的缺失，
/// 所以单独筛一遍。首个字符串实参固定为 code（`xtask invariants` 按此约定扫源码）。
fn assert_warn(doc: &Document, code: &str) {
    let found = warn_codes(doc);
    assert!(found.contains(&code), "期望 warn {code}，实得 {found:?}");
}

// ---------- 负向 ----------

#[test]
fn empty_node_name_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| s.get_mut(id).unwrap().name = "  ".to_string());
    assert_error(&doc, "node.name.empty");
}

#[test]
fn duplicate_node_name_is_error() {
    let mut doc = Document::default();
    doc.write(|s| {
        s.spawn(ObjectDesc {
            name: "a".to_string(),
            ..Default::default()
        })
        .unwrap();
        s.spawn(ObjectDesc {
            name: "b".to_string(),
            ..Default::default()
        })
        .unwrap();
    });
    let id = id_of(&doc, "b");
    doc.write(|s| s.get_mut(id).unwrap().name = "a".to_string());
    assert_error(&doc, "node.name.duplicate");
}

#[test]
fn duplicate_ui_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.ui.push(UiElement {
            key: "hp".to_string(),
            ..Default::default()
        });
        s.ui.push(UiElement {
            key: "hp".to_string(),
            ..Default::default()
        });
    });
    assert_error(&doc, "ui.key.duplicate");
}

#[test]
fn empty_text_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.texts.push(ScreenText::default()));
    assert_error(&doc, "text.key.empty");
}

#[test]
fn dangling_skin_mesh_node_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.add_skin(SkinInstance::new("skin", 9999)));
    assert_error(&doc, "ref.node_missing");
}

#[test]
fn dangling_skin_joint_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        let mut skin = SkinInstance::new("skin", id);
        skin.bindings.push(r3e_core::skin::SkinBinding {
            node: 4242,
            node_index: 0,
            path: "root/joint".to_string(),
            rest_world: r3e_core::Mat4::IDENTITY,
            inverse_bind: r3e_core::Mat4::IDENTITY,
        });
        s.add_skin(skin);
    });
    assert_error(&doc, "ref.node_missing");
}

#[test]
fn skin_joint_matrix_len_mismatch_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        let mut skin = SkinInstance::new("skin", id);
        skin.bindings.push(r3e_core::skin::SkinBinding {
            node: id,
            node_index: 0,
            path: "root/joint".to_string(),
            rest_world: r3e_core::Mat4::IDENTITY,
            inverse_bind: r3e_core::Mat4::IDENTITY,
        });
        // 两个关节矩阵对一个关节：渲染层会整具骨架错位。
        skin.joint_matrices.push(r3e_core::Mat4::IDENTITY);
        skin.joint_matrices.push(r3e_core::Mat4::IDENTITY);
        s.add_skin(skin);
    });
    assert_error(&doc, "skin.joint_matrices.len");
}

#[test]
fn dangling_anim_binding_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        let mut anim = AnimInstance::new("walk", Arc::new(AnimClip::default()));
        anim.bindings.push(AnimBinding {
            node: 7777,
            node_index: 0,
            path: "root".to_string(),
            track: 0,
        });
        s.animations.push(anim);
    });
    assert_error(&doc, "ref.node_missing");
}

#[test]
fn dangling_morph_mesh_node_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.add_morph(MorphInstance::new(8888, 2)));
    assert_error(&doc, "ref.node_missing");
}

#[test]
fn morph_target_count_over_max_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        let mut morph = MorphInstance::new(id, 0);
        morph.target_count = r3e_core::MAX_MORPH_TARGETS + 1;
        s.add_morph(morph);
    });
    assert_error(&doc, "morph.target_count.over_max");
}

#[test]
fn local_light_over_limit_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        for i in 0..=r3e_core::MAX_LOCAL_LIGHTS {
            s.lights.push(r3e_core::LocalLight {
                key: format!("l{i}"),
                ..Default::default()
            });
        }
    });
    assert_error(&doc, "lights.over_limit");
}

#[test]
fn empty_light_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.lights.push(r3e_core::LocalLight::default()));
    assert_error(&doc, "light.key.empty");
}

#[test]
fn non_finite_physics_gravity_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.physics = PhysicsSceneDesc {
            enabled: Some(true),
            gravity: Some([0.0, f32::NAN, 0.0]),
            ..Default::default()
        }
    });
    assert_error(&doc, "physics.gravity.non_finite");
}

#[test]
fn non_finite_camera_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.camera.fov_deg = f32::INFINITY);
    assert_error(&doc, "camera.non_finite");
}

#[test]
fn non_finite_node_transform_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| s.get_mut(id).unwrap().transform.position.x = f32::NAN);
    assert_error(&doc, "node.transform.non_finite");
}

#[test]
fn non_finite_node_color_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| s.get_mut(id).unwrap().color = Vec4::new(1.0, f32::NAN, 0.0, 1.0));
    assert_error(&doc, "node.color.non_finite");
}

#[test]
fn undeclared_asset_ref_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        s.assets.push(r3e_core::AssetDecl {
            path: "assets/cube.gltf".to_string(),
            ..Default::default()
        });
        s.get_mut(id).unwrap().mesh = r3e_core::MeshRef::Asset {
            asset: r3e_core::AssetId::from_path("assets/other.gltf"),
            mesh: 0,
        };
    });
    assert_error(&doc, "ref.asset_missing");
}

#[test]
fn non_finite_material_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        s.get_mut(id).unwrap().material = Some(SurfaceMaterial {
            metallic: f32::NAN,
            ..SurfaceMaterial::default()
        })
    });
    assert_error(&doc, "node.material.non_finite");
}

#[test]
fn duplicate_light_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.lights
            .push(LocalLight::point("lamp", [0.0, 2.0, 0.0], [1.0; 3], 1.0));
        s.lights
            .push(LocalLight::point("lamp", [1.0, 2.0, 0.0], [1.0; 3], 1.0));
    });
    assert_error(&doc, "light.key.duplicate");
}

#[test]
fn non_finite_light_value_is_error() {
    let mut doc = doc_with_node();
    // 绕开 `sanitized()` 直接塞 Vec：这正对应「反序列化 / 脚本手写 Vec」这条真实入口。
    doc.write(|s| {
        let mut light = LocalLight::point("lamp", [0.0, 2.0, 0.0], [1.0; 3], 1.0);
        light.intensity = f32::INFINITY;
        s.lights.push(light);
    });
    assert_error(&doc, "light.value.non_finite");
}

#[test]
fn empty_ui_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.ui.push(UiElement::default()));
    assert_error(&doc, "ui.key.empty");
}

#[test]
fn duplicate_text_key_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.texts.push(ScreenText {
            key: "score".to_string(),
            ..ScreenText::default()
        });
        s.texts.push(ScreenText {
            key: "score".to_string(),
            ..ScreenText::default()
        });
    });
    assert_error(&doc, "text.key.duplicate");
}

#[test]
fn non_finite_morph_weight_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        let mut morph = MorphInstance::new(id, 2);
        morph.weights[0] = f32::NAN;
        s.add_morph(morph);
    });
    assert_error(&doc, "morph.weight.non_finite");
}

#[test]
fn non_finite_background_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| s.background = Vec4::new(f32::NAN, 0.0, 0.0, 1.0));
    assert_error(&doc, "scene.background.non_finite");
}

#[test]
fn empty_anim_name_is_error() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.animations
            .push(AnimInstance::new("  ", Arc::new(AnimClip::default())));
    });
    assert_error(&doc, "anim.name.empty");
}

#[test]
fn empty_skin_name_is_error() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| s.add_skin(SkinInstance::new(" ", id)));
    assert_error(&doc, "skin.name.empty");
}

#[test]
fn unsolved_joint_matrices_is_warn() {
    let mut doc = doc_with_node();
    let id = id_of(&doc, "cube");
    doc.write(|s| {
        let mut skin = SkinInstance::new("skin", id);
        skin.bindings.push(r3e_core::skin::SkinBinding {
            node: id,
            node_index: 0,
            path: "root/joint".to_string(),
            rest_world: r3e_core::Mat4::IDENTITY,
            inverse_bind: r3e_core::Mat4::IDENTITY,
        });
        // 有骨骼但一张矩阵都没求：可用但可疑，只报 Warn。
        s.add_skin(skin);
    });
    assert_warn(&doc, "skin.joint_matrices.unsolved");
}

// ---------- 正向：仓库真实场景必须零 Error ----------

fn parse(src: &str, tag: &str) -> Document {
    Document::from_json_str(src).unwrap_or_else(|e| panic!("{tag} 应当解析成功：{e}"))
}

#[test]
fn shipped_scenes_have_no_errors() {
    let cases: [(&str, &str); 11] = [
        (
            "scene.json(FALLBACK)",
            include_str!("../../../assets/scene.json"),
        ),
        (
            "scene_anim.json",
            include_str!("../../../assets/scene_anim.json"),
        ),
        (
            "scene_ccd.json",
            include_str!("../../../assets/scene_ccd.json"),
        ),
        (
            "scene_hdr.json",
            include_str!("../../../assets/scene_hdr.json"),
        ),
        (
            "scene_ibl.json",
            include_str!("../../../assets/scene_ibl.json"),
        ),
        (
            "scene_local_shadow.json",
            include_str!("../../../assets/scene_local_shadow.json"),
        ),
        (
            "scene_physics.json",
            include_str!("../../../assets/scene_physics.json"),
        ),
        (
            "scene_rapier.json",
            include_str!("../../../assets/scene_rapier.json"),
        ),
        (
            "scene_shadow.json",
            include_str!("../../../assets/scene_shadow.json"),
        ),
        (
            "scene_spatial_audio.json",
            include_str!("../../../assets/scene_spatial_audio.json"),
        ),
        (
            "scene_ssao.json",
            include_str!("../../../assets/scene_ssao.json"),
        ),
    ];
    for (tag, src) in cases {
        let doc = parse(src, tag);
        let errs: Vec<String> = doc
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.to_string())
            .collect();
        assert!(errs.is_empty(), "{tag} 不该报 Error：{errs:#?}");
    }
}

#[test]
fn morph_fixture_scenes_have_no_errors() {
    let cases: [(&str, &str); 4] = [
        (
            "morph_deform",
            include_str!("../../r3e-app/tests/fixtures/morph_deform/scene.json"),
        ),
        (
            "morph_eight",
            include_str!("../../r3e-app/tests/fixtures/morph_eight/scene.json"),
        ),
        (
            "morph_mask",
            include_str!("../../r3e-app/tests/fixtures/morph_mask/scene.json"),
        ),
        (
            "morph_twelve",
            include_str!("../../r3e-app/tests/fixtures/morph_twelve/scene.json"),
        ),
    ];
    for (tag, src) in cases {
        let doc = parse(src, tag);
        let errs: Vec<String> = doc
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.to_string())
            .collect();
        assert!(errs.is_empty(), "{tag} 不该报 Error：{errs:#?}");
    }
}

#[test]
fn clean_document_has_no_issues_at_all() {
    let doc = doc_with_node();
    assert!(doc.validate().is_empty(), "新建文档不该有任何问题");
}

#[test]
fn issues_are_sorted_by_path() {
    let mut doc = doc_with_node();
    doc.write(|s| {
        s.lights.push(r3e_core::LocalLight::default());
        s.texts.push(ScreenText::default());
    });
    let issues = doc.validate();
    let mut paths: Vec<String> = issues.iter().map(|i| i.path.clone()).collect();
    let sorted = {
        let mut p = paths.clone();
        p.sort();
        p
    };
    assert_eq!(paths, sorted, "结果顺序必须稳定");
    paths.dedup();
    assert!(issues.len() >= 2);
}

#[test]
fn validate_does_not_write_or_dirty() {
    let doc = doc_with_node();
    let version = doc.version();
    let dirty = doc.is_dirty();
    let _ = doc.validate();
    assert_eq!(doc.version(), version, "校验不得经写入口");
    assert_eq!(doc.is_dirty(), dirty, "校验不得改变脏标记");
}
