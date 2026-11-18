//! PBR 材质的写入收敛与场景往返（metallic / roughness / emissive）。
//!
//! 关心的不是 BRDF 数值（见 `r3e-core` 内的 `material::tests`），而是
//! 「非法值不进场景」与「写进去能原样读回来」这两条链路：
//! 场景文件 spawn → 场景 API → JSON 保存 → 重新加载。

use r3e_core::{
    ObjectDesc, Scene, SceneMaterialDecl, SurfaceMaterial, DEFAULT_ROUGHNESS, MAX_EMISSIVE,
};

fn desc(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        mesh: Some("cube".to_string()),
        ..Default::default()
    }
}

/// 场景文件里写的非法材质在 spawn 那一刻就被收敛，场景内不残留 NaN / 超界值。
#[test]
fn spawn_sanitizes_material_from_scene() {
    let mut scene = Scene::new();
    let mut obj = desc("bad");
    obj.material = Some(SurfaceMaterial {
        metallic: 3.5,
        roughness: f32::NAN,
        emissive: [-1.0, 1e9, 0.5],
    });
    scene.spawn(obj).expect("spawn");

    let m = scene.material_of("bad").expect("物件存在");
    assert_eq!(m.metallic, 1.0, "3.5 应收到上界 1");
    assert_eq!(m.roughness, DEFAULT_ROUGHNESS, "NaN 应退回缺省粗糙度");
    assert_eq!(
        m.emissive,
        [0.0, MAX_EMISSIVE, 0.5],
        "负值归零、超界收到上限"
    );
}

/// 场景 API 写入的材质可原样读回，且只改一个字段时另一个字段不受影响。
#[test]
fn scene_api_write_and_read_back() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");

    // 未声明材质时读回缺省值（渲染侧可见结果一致）
    assert_eq!(scene.material_of("ball"), Some(SurfaceMaterial::default()));

    assert!(scene.set_material_params("ball", 0.9, 0.15));
    let m = scene.material_of("ball").unwrap();
    assert_eq!((m.metallic, m.roughness), (0.9, 0.15));
    assert_eq!(m.emissive, [0.0, 0.0, 0.0]);

    // 只改 emissive，metallic / roughness 保留
    assert!(scene.set_emissive("ball", [0.2, 0.4, 0.6]));
    let m = scene.material_of("ball").unwrap();
    assert_eq!((m.metallic, m.roughness), (0.9, 0.15));
    assert_eq!(m.emissive, [0.2, 0.4, 0.6]);

    // 整体写入同样收敛
    assert!(scene.set_material(
        "ball",
        SurfaceMaterial {
            metallic: f32::INFINITY,
            roughness: -2.0,
            emissive: [f32::NAN, 0.0, 0.0],
        }
    ));
    let m = scene.material_of("ball").unwrap();
    assert_eq!(m.metallic, 1.0);
    assert_eq!(m.roughness, 0.0);
    assert_eq!(m.emissive, [0.0, 0.0, 0.0]);

    // 撤销覆盖后回到缺省
    assert!(scene.clear_material("ball"));
    assert_eq!(scene.material_of("ball"), Some(SurfaceMaterial::default()));
}

/// 物件不存在时读写 API 明确失败，不静默吞掉。
#[test]
fn material_api_reports_missing_node() {
    let mut scene = Scene::new();
    assert_eq!(scene.material_of("ghost"), None);
    assert!(!scene.set_material_params("ghost", 0.5, 0.5));
    assert!(!scene.set_emissive("ghost", [1.0, 1.0, 1.0]));
    assert!(!scene.set_material("ghost", SurfaceMaterial::default()));
    assert!(!scene.clear_material("ghost"));
}

/// 保存 → 重新加载：材质字段（含自发光）逐位一致。
#[test]
fn material_survives_json_roundtrip() {
    let mut scene = Scene::new();
    scene.spawn(desc("metal")).expect("spawn");
    scene.spawn(desc("lamp")).expect("spawn");
    scene.spawn(desc("plain")).expect("spawn");

    scene.set_material_params("metal", 1.0, 0.08);
    assert!(scene.set_emissive("lamp", [1.5, 0.75, 0.25]));

    let json = scene.to_json_pretty().expect("序列化");
    // 缺省材质不写进文件，保持场景文件干净
    let plain_obj = json
        .split("\"name\": \"plain\"")
        .nth(1)
        .expect("plain 物件");
    assert!(
        !plain_obj.contains("\"material\""),
        "缺省材质不应写入 JSON: {plain_obj}"
    );
    assert!(json.contains("\"metallic\""), "材质字段应被序列化");

    let reloaded = Scene::from_json_str(&json).expect("反序列化");
    assert_eq!(
        reloaded.material_of("metal"),
        Some(SurfaceMaterial {
            metallic: 1.0,
            roughness: 0.08,
            emissive: [0.0, 0.0, 0.0],
        })
    );
    assert_eq!(
        reloaded.material_of("lamp"),
        Some(SurfaceMaterial {
            metallic: 0.0,
            roughness: DEFAULT_ROUGHNESS,
            emissive: [1.5, 0.75, 0.25],
        })
    );
    assert_eq!(
        reloaded.material_of("plain"),
        Some(SurfaceMaterial::default())
    );
}

/// 场景文件只写部分字段时，其余字段取缺省（不因缺字段而丢弃整份材质）。
#[test]
fn partial_material_in_json_uses_defaults() {
    let json = r#"{
        "objects": [
            { "name": "satin", "mesh": "cube", "material": { "roughness": 0.35 } },
            { "name": "gold", "mesh": "cube", "material": { "metallic": 1.0 } }
        ]
    }"#;
    let scene = Scene::from_json_str(json).expect("反序列化");
    let satin = scene.material_of("satin").unwrap();
    assert_eq!(satin.metallic, 0.0);
    assert_eq!(satin.roughness, 0.35);
    assert_eq!(satin.emissive, [0.0, 0.0, 0.0]);
    let gold = scene.material_of("gold").unwrap();
    assert_eq!(gold.metallic, 1.0);
    assert_eq!(gold.roughness, DEFAULT_ROUGHNESS);
}

// ===== S4-A：场景材质表（materials[] + 节点 materialRef）=====

/// 带五张贴图的材质经「场景 API → JSON → 重新加载」后逐字段一致。
///
/// 这里不碰磁盘：`to_json_pretty` / `from_json_str` 覆盖的是序列化契约，
/// 贴图存在性属于 doc 层校验（`material.texture_missing`），不在这一层断言。
#[test]
fn scene_material_table_survives_json_roundtrip() {
    let mut scene = Scene::new();
    scene.spawn(desc("wall")).expect("spawn");

    assert!(scene.upsert_scene_material(SceneMaterialDecl {
        name: "brick".to_string(),
        metallic: 0.0,
        roughness: 0.8,
        emissive: [0.1, 0.0, 0.0],
        albedo: Some("tex/brick_albedo.png".to_string()),
        normal: Some("tex/brick_normal.png".to_string()),
        metallic_roughness: Some("tex/brick_mr.png".to_string()),
        occlusion: Some("tex/brick_ao.png".to_string()),
        emissive_map: Some("tex/brick_emissive.png".to_string()),
        uv_scale: [2.0, 3.0],
    }));
    assert!(scene.set_node_material_ref("wall", Some("brick")));

    let json = scene.to_json_pretty().expect("序列化");
    let reloaded = Scene::from_json_str(&json).expect("反序列化");

    let decl = reloaded.scene_material("brick").expect("材质表随场景往返");
    assert_eq!(decl.metallic, 0.0);
    assert_eq!(decl.roughness, 0.8);
    assert_eq!(decl.emissive, [0.1, 0.0, 0.0]);
    assert_eq!(decl.albedo.as_deref(), Some("tex/brick_albedo.png"));
    assert_eq!(decl.normal.as_deref(), Some("tex/brick_normal.png"));
    assert_eq!(decl.metallic_roughness.as_deref(), Some("tex/brick_mr.png"));
    assert_eq!(decl.occlusion.as_deref(), Some("tex/brick_ao.png"));
    assert_eq!(decl.emissive_map.as_deref(), Some("tex/brick_emissive.png"));
    assert_eq!(decl.uv_scale, [2.0, 3.0]);

    // 节点引用也要原样回来，否则渲染侧取不到材质（画面退化成纯标量）
    assert_eq!(reloaded.material_ref_of("wall"), Some("brick"));
    assert_eq!(
        reloaded
            .node_scene_material(reloaded.find_id("wall").unwrap())
            .map(|d| d.name.as_str()),
        Some("brick")
    );
}

/// 空材质表不写进场景文件（保持存量场景文件的字节形态不变）。
#[test]
fn empty_material_table_is_not_serialized() {
    let mut scene = Scene::new();
    scene.spawn(desc("cube")).expect("spawn");
    let json = scene.to_json_pretty().expect("序列化");
    assert!(
        !json.contains("\"materials\""),
        "空材质表不应写入 JSON: {json}"
    );
    assert!(
        !json.contains("\"materialRef\""),
        "没有引用时不应写出 materialRef: {json}"
    );
}

/// 材质参数在写进场景表那一刻就被收敛：NaN / 越界值不残留。
#[test]
fn scene_material_params_are_sanitized_on_upsert() {
    let mut scene = Scene::new();
    scene.upsert_scene_material(SceneMaterialDecl {
        name: "bad".to_string(),
        metallic: 9.0,
        roughness: f32::NAN,
        emissive: [-1.0, 1e9, 0.25],
        uv_scale: [0.0, -3.0],
        ..SceneMaterialDecl::default()
    });
    let decl = scene.scene_material("bad").expect("材质已写入");
    assert_eq!(decl.metallic, 1.0, "9.0 应收到上界 1");
    assert_eq!(decl.roughness, DEFAULT_ROUGHNESS, "NaN 应退回缺省粗糙度");
    assert_eq!(decl.emissive, [0.0, MAX_EMISSIVE, 0.25]);
    // uv 倍率为 0 会让整张贴图压成一个像素，必须夹到正数下界
    assert!(
        decl.uv_scale[0] > 0.0 && decl.uv_scale[1] > 0.0,
        "uv 倍率必须被夹到正数: {:?}",
        decl.uv_scale
    );
}

/// 同名材质是「覆盖」而不是「追加」：材质表上限只拦新增。
#[test]
fn upsert_scene_material_overwrites_by_name() {
    let mut scene = Scene::new();
    assert!(scene.upsert_scene_material(SceneMaterialDecl {
        name: "wood".to_string(),
        roughness: 0.9,
        ..SceneMaterialDecl::default()
    }));
    assert!(scene.upsert_scene_material(SceneMaterialDecl {
        name: "wood".to_string(),
        roughness: 0.3,
        ..SceneMaterialDecl::default()
    }));
    assert_eq!(scene.materials.len(), 1, "同名覆盖不应追加新条目");
    assert_eq!(scene.scene_material("wood").unwrap().roughness, 0.3);
}
