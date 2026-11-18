//! 命令层 v0 的端到端测试：每条命令都验证「apply 生效 → undo 精确还原」。
//!
//! 还原判据统一用场景 JSON 全量比对（`Scene::to_json_string`）：比逐个字段断言更严，
//! 能同时抓出「撤销写错字段」和「撤销多留了一条残留」。

use r3e_core::{
    Camera, EnvironmentSettings, LocalLight, ObjectDesc, PostFxSettings, Scene, SceneMaterialDecl,
    ScreenText, SurfaceMaterial, TerrainPatch, UiElement, Vec4, SCENE_MATERIAL_MAX,
    TERRAIN_MAX_PATCHES,
};
use r3e_doc::{Command, CommandKind, DocError, Document};

fn base_doc() -> Document {
    let mut doc = Document::default();
    doc.write(|s| {
        s.spawn(ObjectDesc {
            name: "cube".to_string(),
            ..Default::default()
        })
        .expect("cube 应当建成功");
        s.spawn(ObjectDesc {
            name: "sphere".to_string(),
            position: Some([1.0, 2.0, 3.0]),
            ..Default::default()
        })
        .expect("sphere 应当建成功");
    });
    doc
}

fn snap(doc: &Document) -> String {
    doc.as_scene().to_json_string().expect("场景应当可序列化")
}

fn id_of(doc: &Document, name: &str) -> r3e_core::NodeId {
    doc.find(name).expect("节点应当存在").id
}

/// 造一盏灯（用结构体更新语法，避开构造函数的参数细节）。
fn lamp(key: &str, intensity: f32) -> LocalLight {
    LocalLight {
        key: key.to_string(),
        pos: [0.0, 3.0, 0.0],
        intensity,
        ..Default::default()
    }
}

/// 造一块地形：`flat` 给形状，这里只改想断言的字段（粗糙度）。
fn patch(name: &str, roughness: f32) -> TerrainPatch {
    let mut p = TerrainPatch::flat(name, [0.0, 0.0], [10.0, 10.0]);
    p.roughness = roughness;
    p
}

fn hud(key: &str) -> UiElement {
    UiElement {
        key: key.to_string(),
        ..Default::default()
    }
}

fn hud_text(key: &str, content: &str) -> ScreenText {
    ScreenText {
        key: key.to_string(),
        content: content.to_string(),
        ..Default::default()
    }
}

fn apply_then_undo(doc: &mut Document, kind: CommandKind) {
    let before = snap(doc);
    doc.apply(&kind).expect("apply 应当成功");
    let after = snap(doc);
    assert_ne!(before, after, "apply 必须真的改了场景");
    doc.undo().expect("undo 应当成功");
    assert_eq!(snap(doc), before, "undo 必须精确还原");
}

// ---------- 节点域 ----------

#[test]
fn spawn_node_applies_and_undo_removes() {
    let mut doc = Document::default();
    let cmd = CommandKind::spawn_node(ObjectDesc {
        name: "cone".to_string(),
        ..Default::default()
    });
    apply_then_undo(&mut doc, cmd);
    assert!(doc.find("cone").is_none());
}

#[test]
fn spawn_duplicate_name_is_rejected_by_core() {
    let mut doc = base_doc();
    let before_version = doc.version();
    let err = doc
        .apply(&CommandKind::spawn_node(ObjectDesc {
            name: "cube".to_string(),
            ..Default::default()
        }))
        .expect_err("重名必须失败");
    assert!(matches!(err, DocError::Scene(_)), "重名由 core 报错");
    assert_eq!(doc.version(), before_version, "失败的 apply 不得推进版本");
    assert_eq!(doc.undo_depth(), 0, "失败的 apply 不得入撤销栈");
}

#[test]
fn set_transform_roundtrip() {
    let doc0 = base_doc();
    let id = id_of(&doc0, "cube");
    let mut next = doc0.get(id).unwrap().transform;
    next.position = r3e_core::Vec3::new(9.0, 9.0, 9.0);
    let mut doc = doc0;
    let cmd = CommandKind::set_transform(&doc, id, next).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
}

#[test]
fn set_transform_on_missing_node_errors() {
    let doc = base_doc();
    let err = CommandKind::set_transform(&doc, 9999, Default::default())
        .expect_err("不存在的节点必须报错");
    assert!(matches!(err, DocError::NotFound(_)));
}

#[test]
fn set_visible_roundtrip() {
    let mut doc = base_doc();
    let id = id_of(&doc, "cube");
    let cmd = CommandKind::set_visible(&doc, id, false).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert!(doc.get(id).unwrap().visible);
}

#[test]
fn set_node_color_roundtrip() {
    let mut doc = base_doc();
    let id = id_of(&doc, "sphere");
    let cmd =
        CommandKind::set_node_color(&doc, id, Vec4::new(1.0, 0.0, 0.0, 1.0)).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
}

#[test]
fn set_node_material_roundtrip() {
    let mut doc = base_doc();
    let material = SurfaceMaterial {
        metallic: 0.9,
        roughness: 0.1,
        ..Default::default()
    };
    let cmd = CommandKind::set_node_material(&doc, "cube", Some(material)).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert!(
        doc.find("cube").unwrap().material.is_none(),
        "撤销后不应留下材质覆盖"
    );
}

#[test]
fn clear_node_material_undo_restores_override() {
    let mut doc = base_doc();
    doc.write(|s| {
        s.set_material(
            "cube",
            SurfaceMaterial {
                metallic: 0.5,
                ..Default::default()
            },
        )
    });
    let cmd = CommandKind::set_node_material(&doc, "cube", None).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert!(
        doc.find("cube").unwrap().material.is_some(),
        "撤销必须把覆盖恢复回来"
    );
}

#[test]
fn set_node_material_on_missing_node_errors() {
    let doc = base_doc();
    let err = CommandKind::set_node_material(&doc, "ghost", None).expect_err("缺失节点必须报错");
    assert!(matches!(err, DocError::NotFound(_)));
}

// ---------- 环境域 ----------

#[test]
fn set_camera_roundtrip() {
    let mut doc = base_doc();
    let mut camera = doc.camera;
    camera.fov_deg = 35.0;
    let cmd = CommandKind::set_camera(&doc, camera);
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.camera.fov_deg, Camera::default().fov_deg);
}

#[test]
fn set_environment_roundtrip() {
    let mut doc = base_doc();
    assert!(doc.environment.is_none(), "默认没有环境探针");
    let cmd = CommandKind::set_environment(&doc, Some(EnvironmentSettings::default()));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.environment.is_none(), "撤销必须回到 None");
}

#[test]
fn set_postfx_roundtrip() {
    let mut doc = base_doc();
    let fx = PostFxSettings {
        exposure: 2.0,
        ..Default::default()
    };
    let cmd = CommandKind::set_postfx(&doc, Some(fx));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.postfx.is_none());
}

// ---------- 灯光域 ----------

#[test]
fn new_local_light_undo_deletes_it() {
    let mut doc = base_doc();
    let cmd = CommandKind::upsert_local_light(&doc, lamp("lamp", 6.0));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.lights.is_empty(), "此前不存在的灯，撤销应删除");
}

#[test]
fn overwritten_local_light_undo_restores_old() {
    let mut doc = base_doc();
    doc.write(|s| s.upsert_local_light(lamp("lamp", 1.0)));
    let cmd = CommandKind::upsert_local_light(&doc, lamp("lamp", 8.0));
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.lights.len(), 1);
    assert!(
        (doc.lights[0].intensity - 1.0).abs() < 1e-6,
        "撤销应恢复旧强度"
    );
}

#[test]
fn remove_local_light_roundtrip() {
    let mut doc = base_doc();
    doc.write(|s| s.upsert_local_light(lamp("lamp", 3.0)));
    let cmd = CommandKind::remove_local_light(&doc, "lamp").expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.lights.len(), 1);
}

#[test]
fn remove_missing_local_light_errors() {
    let doc = base_doc();
    let err = CommandKind::remove_local_light(&doc, "nope").expect_err("缺失光源必须报错");
    assert!(matches!(err, DocError::NotFound(_)));
}

// ---------- UI / 文本域 ----------

#[test]
fn upsert_ui_insert_then_undo_removes() {
    let mut doc = base_doc();
    let cmd = CommandKind::upsert_ui(&doc, hud("hp"));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.ui.is_empty());
}

#[test]
fn upsert_ui_replace_then_undo_restores() {
    let mut doc = base_doc();
    doc.write(|s| {
        s.ui.push(UiElement {
            key: "hp".to_string(),
            layer: 1,
            ..Default::default()
        })
    });
    let cmd = CommandKind::upsert_ui(
        &doc,
        UiElement {
            key: "hp".to_string(),
            layer: 5,
            ..Default::default()
        },
    );
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.ui[0].layer, 1, "撤销应恢复旧图层");
}

#[test]
fn remove_ui_roundtrip() {
    let mut doc = base_doc();
    doc.write(|s| s.ui.push(hud("hp")));
    let cmd = CommandKind::remove_ui(&doc, "hp").expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.ui.len(), 1);
}

#[test]
fn upsert_text_insert_then_undo_removes() {
    let mut doc = base_doc();
    let cmd = CommandKind::upsert_text(&doc, hud_text("fps", "60"));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.texts.is_empty());
}

#[test]
fn upsert_text_replace_then_undo_restores() {
    let mut doc = base_doc();
    doc.write(|s| s.texts.push(hud_text("fps", "30")));
    let cmd = CommandKind::upsert_text(&doc, hud_text("fps", "120"));
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.texts[0].content, "30");
}

#[test]
fn remove_text_roundtrip() {
    let mut doc = base_doc();
    doc.write(|s| s.texts.push(hud_text("fps", "60")));
    let cmd = CommandKind::remove_text(&doc, "fps").expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.texts.len(), 1);
}

// ---------- 撤销栈语义 ----------

#[test]
fn undo_on_empty_stack_reports_nothing_to_undo() {
    let mut doc = base_doc();
    let err = doc.undo().expect_err("空栈必须报错");
    assert_eq!(err, DocError::NothingToUndo);
    assert_eq!(doc.undo_depth(), 0);
    assert_eq!(err.to_string(), "没有可撤销的操作");
}

#[test]
fn undo_stack_tracks_depth_and_label() {
    let mut doc = base_doc();
    let id = id_of(&doc, "cube");
    doc.apply(&CommandKind::set_visible(&doc, id, false).unwrap())
        .expect("apply 应当成功");
    doc.apply(&CommandKind::set_node_color(&doc, id, Vec4::ZERO).unwrap())
        .expect("apply 应当成功");
    assert_eq!(doc.undo_depth(), 2);
    assert_eq!(doc.undo_label(), Some("改颜色"));
    doc.clear_undo();
    assert_eq!(doc.undo_depth(), 0);
    assert_eq!(doc.undo_label(), None);
}

#[test]
fn replace_scene_clears_undo_stack() {
    let mut doc = base_doc();
    let id = id_of(&doc, "cube");
    doc.apply(&CommandKind::set_visible(&doc, id, false).unwrap())
        .expect("apply 应当成功");
    assert_eq!(doc.undo_depth(), 1);
    doc.replace_scene(Scene::new());
    assert_eq!(doc.undo_depth(), 0, "换场景后旧前像必须作废");
}

#[test]
fn undo_failure_keeps_command_on_stack() {
    let mut doc = base_doc();
    let id = id_of(&doc, "cube");
    doc.apply(&CommandKind::set_visible(&doc, id, false).unwrap())
        .expect("apply 应当成功");
    // 绕过命令层直接把节点删掉，让撤销目标消失。
    doc.write(|s| s.despawn("cube"));
    let err = doc.undo().expect_err("目标没了，撤销必须失败");
    assert!(matches!(err, DocError::NotFound(_)));
    assert_eq!(doc.undo_depth(), 1, "失败的撤销不得丢历史");
}

#[test]
fn dyn_command_goes_through_document() {
    let mut doc = base_doc();
    let id = id_of(&doc, "sphere");
    let kind = CommandKind::set_visible(&doc, id, false).unwrap();
    let before = snap(&doc);
    let cmd: &dyn Command = &kind;
    doc.apply(cmd).expect("apply 应当成功");
    assert!(!doc.get(id).unwrap().visible);
    doc.undo().expect("undo 应当成功");
    assert!(doc.get(id).unwrap().visible);
    assert_eq!(snap(&doc), before, "经 trait 对象往返后仍应精确还原");
}

#[test]
fn labels_and_targets_are_populated() {
    let doc = base_doc();
    let id = id_of(&doc, "cube");
    let samples = [
        CommandKind::spawn_node(ObjectDesc {
            name: "x".to_string(),
            ..Default::default()
        }),
        CommandKind::set_visible(&doc, id, false).unwrap(),
        CommandKind::set_camera(&doc, Camera::default()),
        CommandKind::upsert_ui(&doc, hud("k")),
        CommandKind::set_node_material(&doc, "cube", None).unwrap(),
    ];
    for kind in samples {
        assert!(!kind.label().is_empty(), "命令必须有展示名");
    }
    let by_name = CommandKind::set_node_material(&doc, "cube", None).unwrap();
    assert_eq!(by_name.target_name(), Some("cube"));
    let by_id = CommandKind::set_visible(&doc, id, false).unwrap();
    assert_eq!(by_id.target_name(), None, "按 id 寻址的命令没有名字目标");
}

// ---------- 地形域（M1-S3-C） ----------

#[test]
fn new_terrain_patch_undo_deletes_it() {
    let mut doc = base_doc();
    let cmd = CommandKind::upsert_terrain_patch(&doc, patch("ground", 0.9));
    apply_then_undo(&mut doc, cmd);
    assert!(doc.terrain.is_empty(), "此前不存在的地块，撤销应删除");
}

#[test]
fn overwritten_terrain_patch_undo_restores_old() {
    let mut doc = base_doc();
    doc.write(|s| s.upsert_terrain_patch(patch("ground", 0.2)));
    // paint 语义就是覆盖：同名再刷一次必须生效，撤销时前像要把旧粗糙度带回。
    let cmd = CommandKind::upsert_terrain_patch(&doc, patch("ground", 0.95));
    apply_then_undo(&mut doc, cmd);
    assert_eq!(doc.terrain.len(), 1, "覆盖不得多出一块");
    assert!(
        (doc.terrain[0].roughness - 0.2).abs() < 1e-6,
        "撤销应恢复旧粗糙度"
    );
}

#[test]
fn terrain_patch_label_and_target() {
    let doc = base_doc();
    let cmd = CommandKind::upsert_terrain_patch(&doc, patch("ground", 0.5));
    assert_eq!(cmd.label(), "写地形");
    assert_eq!(cmd.target_name(), Some("ground"));
}

#[test]
fn upsert_terrain_patch_over_capacity_is_rejected() {
    let mut doc = Document::default();
    doc.write(|s| {
        for i in 0..TERRAIN_MAX_PATCHES {
            assert!(
                s.upsert_terrain_patch(patch(&format!("p{i}"), 0.5)),
                "空表逐块写入应当成功"
            );
        }
    });
    let version = doc.version();
    let depth = doc.undo_depth();
    let err = doc
        .apply(&CommandKind::upsert_terrain_patch(
            &doc,
            patch("extra", 0.5),
        ))
        .expect_err("表满后新名字必须被拒");
    assert!(matches!(err, DocError::Rejected(_)), "容量错是 Rejected");
    assert_eq!(doc.version(), version, "失败的 apply 不得推进版本");
    assert_eq!(doc.undo_depth(), depth, "失败的 apply 不得入撤销栈");

    // 表满时**覆盖**仍然要放行——否则「p0 想改个粗糙度」都得先腾位子。
    doc.apply(&CommandKind::upsert_terrain_patch(&doc, patch("p0", 0.8)))
        .expect("覆盖不受容量限制");
    assert_eq!(doc.terrain.len(), TERRAIN_MAX_PATCHES);
}

// ---------- 材质域（M1-S4-A） ----------

fn brick(roughness: f32) -> SceneMaterialDecl {
    SceneMaterialDecl {
        name: "brick".to_string(),
        roughness,
        albedo: Some("tex/brick_albedo.png".to_string()),
        normal: Some("tex/brick_normal.png".to_string()),
        ..SceneMaterialDecl::default()
    }
}

#[test]
fn upsert_material_applies_and_undo_removes() {
    let mut doc = base_doc();
    let cmd = CommandKind::upsert_scene_material(&doc, brick(0.8));
    apply_then_undo(&mut doc, cmd);
    assert!(
        doc.as_scene().scene_material("brick").is_none(),
        "撤销后不留材质表项"
    );
}

#[test]
fn upsert_material_over_existing_undo_restores_old_params() {
    let mut doc = base_doc();
    doc.apply(&CommandKind::upsert_scene_material(&doc, brick(0.8)))
        .expect("首写应当成功");
    // 同名覆盖：撤销必须回到「第一份」而不是「没有」，否则材质名字会被一起撤掉。
    let cmd = CommandKind::upsert_scene_material(&doc, brick(0.2));
    apply_then_undo(&mut doc, cmd);
    let kept = doc
        .as_scene()
        .scene_material("brick")
        .expect("旧材质应当还在");
    assert!((kept.roughness - 0.8).abs() < 1e-6, "撤销要还原旧粗糙度");
}

#[test]
fn remove_material_applies_and_undo_restores() {
    let mut doc = base_doc();
    doc.apply(&CommandKind::upsert_scene_material(&doc, brick(0.8)))
        .expect("写入应当成功");
    let cmd = CommandKind::remove_scene_material(&doc, "brick").expect("材质应当存在");
    apply_then_undo(&mut doc, cmd);
    assert!(
        doc.as_scene().scene_material("brick").is_some(),
        "撤销要把材质整份放回来"
    );
}

#[test]
fn remove_missing_material_errors() {
    let doc = base_doc();
    let err = CommandKind::remove_scene_material(&doc, "ghost").expect_err("缺材质必须报错");
    assert!(matches!(err, DocError::NotFound(_)), "缺材质是 NotFound");
}

#[test]
fn set_node_material_ref_applies_and_undo_clears() {
    let mut doc = base_doc();
    doc.apply(&CommandKind::upsert_scene_material(&doc, brick(0.8)))
        .expect("写入应当成功");
    let cmd = CommandKind::set_node_material_ref(&doc, "cube", Some("brick".to_string()))
        .expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert_eq!(
        doc.as_scene().material_ref_of("cube"),
        None,
        "撤销后不该留引用"
    );
}

#[test]
fn clear_material_ref_undo_restores_reference() {
    let mut doc = base_doc();
    doc.apply(&CommandKind::upsert_scene_material(&doc, brick(0.8)))
        .expect("写入应当成功");
    doc.apply(
        &CommandKind::set_node_material_ref(&doc, "cube", Some("brick".to_string()))
            .expect("构造应当成功"),
    )
    .expect("引用应当写成功");
    let cmd = CommandKind::set_node_material_ref(&doc, "cube", None).expect("构造应当成功");
    apply_then_undo(&mut doc, cmd);
    assert_eq!(
        doc.as_scene().material_ref_of("cube"),
        Some("brick"),
        "清除引用的撤销要写回原引用"
    );
}

#[test]
fn set_material_ref_on_missing_node_errors() {
    let doc = base_doc();
    let err = CommandKind::set_node_material_ref(&doc, "ghost", None).expect_err("缺节点必须报错");
    assert!(matches!(err, DocError::NotFound(_)), "缺节点是 NotFound");
}

#[test]
fn upsert_material_over_capacity_is_rejected_but_overwrite_passes() {
    let mut doc = Document::default();
    doc.write(|s| {
        for i in 0..SCENE_MATERIAL_MAX {
            assert!(
                s.upsert_scene_material(SceneMaterialDecl {
                    name: format!("m{i}"),
                    ..SceneMaterialDecl::default()
                }),
                "空表逐份写入应当成功"
            );
        }
    });
    let version = doc.version();
    let depth = doc.undo_depth();
    let err = doc
        .apply(&CommandKind::upsert_scene_material(
            &doc,
            SceneMaterialDecl {
                name: "extra".to_string(),
                ..SceneMaterialDecl::default()
            },
        ))
        .expect_err("表满后新名字必须被拒");
    assert!(matches!(err, DocError::Rejected(_)), "容量错是 Rejected");
    assert_eq!(doc.version(), version, "失败的 apply 不得推进版本");
    assert_eq!(doc.undo_depth(), depth, "失败的 apply 不得入撤销栈");

    doc.apply(&CommandKind::upsert_scene_material(
        &doc,
        SceneMaterialDecl {
            name: "m0".to_string(),
            roughness: 0.9,
            ..SceneMaterialDecl::default()
        },
    ))
    .expect("覆盖已有材质不受容量限制");
    assert_eq!(doc.materials.len(), SCENE_MATERIAL_MAX);
}
