//! `StateHash` 的覆盖守卫（S4-03）。
//!
//! 放在集成测试而非 `src/state_hash.rs` 内联：① 单文件 800 行门禁；② 只用公开 API，
//! 顺带证明「哈希能力对下游可用」。

use r3e_core::{
    AssetDecl, EnvironmentMapSettings, EnvironmentSettings, ObjectDesc, PostFxSettings, Scene,
    SceneMaterialDecl, SpawnDecl, Vec4,
};
use r3e_doc::Document;

/// 一组「扰动」：标签 + 对场景做一处改动，用于验证该字段确实进了哈希。
type Mutation = (&'static str, Box<dyn Fn(&mut Scene)>);

fn seeded() -> Document {
    let mut doc = Document::default();
    doc.write(|s| {
        s.name = "hash-base".to_string();
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let desc = ObjectDesc {
                name: (*name).to_string(),
                position: Some([i as f32, 0.0, 0.0]),
                ..ObjectDesc::default()
            };
            s.spawn(desc).unwrap();
        }
        s.upsert_spawn(SpawnDecl::new("start", [0.0, 0.0, 0.0]));
    });
    doc
}

/// S4-03：同状态两次哈希相同。
#[test]
fn state_hash_of_same_scene_is_stable() {
    let doc = seeded();
    let first = doc.state_hash();
    let second = doc.state_hash();
    assert_eq!(first, second);

    // 两次独立构造的同内容文档也必须同哈希。
    let other = seeded();
    assert_eq!(first, other.state_hash());
    assert_eq!(16, first.to_hex().len());
}

/// S4-03：字段逐组扰动，哈希必变（覆盖面守卫）。
#[test]
fn every_covered_field_changes_hash() {
    let base = seeded().state_hash();

    let cases: Vec<Mutation> = vec![
        (
            "node.transform",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("a").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.transform.position.x += 1.0;
                }
            }),
        ),
        (
            "node.visible",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("b").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.visible = false;
                }
            }),
        ),
        (
            "node.color",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("c").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.color = Vec4::new(1.0, 0.0, 0.0, 1.0);
                }
            }),
        ),
        (
            "node.name",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("a").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.name = "renamed".to_string();
                }
            }),
        ),
        (
            "node.tags",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("a").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.tags = vec!["door".to_string()];
                }
            }),
        ),
        (
            "node.interact_radius",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("b").unwrap();
                if let Some(n) = s.get_mut(id) {
                    n.interact_radius = Some(2.5);
                }
            }),
        ),
        (
            "scene.name",
            Box::new(|s: &mut Scene| s.name = "other".to_string()),
        ),
        (
            "background",
            Box::new(|s: &mut Scene| s.background = Vec4::ONE),
        ),
        ("camera", Box::new(|s: &mut Scene| s.camera.fov_deg += 1.0)),
        ("light", Box::new(|s: &mut Scene| s.light.intensity += 1.0)),
        (
            "environment",
            Box::new(|s: &mut Scene| {
                s.environment = Some(EnvironmentSettings::default());
            }),
        ),
        (
            "postfx",
            Box::new(|s: &mut Scene| {
                s.postfx = Some(PostFxSettings::default());
            }),
        ),
        (
            "lights_enabled",
            Box::new(|s: &mut Scene| s.lights_enabled = !s.lights_enabled),
        ),
        (
            "text_enabled",
            Box::new(|s: &mut Scene| s.text_enabled = !s.text_enabled),
        ),
        (
            "ui_enabled",
            Box::new(|s: &mut Scene| s.ui_enabled = !s.ui_enabled),
        ),
        (
            "ui_above_text",
            Box::new(|s: &mut Scene| s.ui_above_text = !s.ui_above_text),
        ),
        (
            "occlusion",
            Box::new(|s: &mut Scene| s.occlusion.enabled = !s.occlusion.enabled),
        ),
        (
            "environment_map",
            Box::new(|s: &mut Scene| {
                s.environment_map = Some(EnvironmentMapSettings::default());
            }),
        ),
        (
            "physics",
            Box::new(|s: &mut Scene| s.physics.ground = Some(1.5)),
        ),
        (
            "assets",
            Box::new(|s: &mut Scene| {
                s.assets.push(AssetDecl {
                    path: "models/x.glb".to_string(),
                    ..AssetDecl::default()
                });
            }),
        ),
        (
            "morphs",
            Box::new(|s: &mut Scene| {
                let id = s.find_id("a").unwrap();
                if s.bind_morph_by_name("a", 2) {
                    s.set_morph_weight(id, 0, 0.5);
                }
            }),
        ),
        // S4-A：场景材质表。三处都要各自进哈希 —— 只混「有这张表」而不混内容，
        // 会让「改了贴图路径但哈希不变」，回放时贴图换了却判定为同状态。
        (
            "materials（新增一条）",
            Box::new(|s: &mut Scene| {
                s.upsert_scene_material(SceneMaterialDecl {
                    name: "brick".to_string(),
                    ..SceneMaterialDecl::default()
                });
            }),
        ),
        (
            "materials.scalars",
            Box::new(|s: &mut Scene| {
                s.upsert_scene_material(SceneMaterialDecl {
                    name: "brick".to_string(),
                    roughness: 0.9,
                    ..SceneMaterialDecl::default()
                });
            }),
        ),
        (
            "materials.albedo",
            Box::new(|s: &mut Scene| {
                s.upsert_scene_material(SceneMaterialDecl {
                    name: "brick".to_string(),
                    albedo: Some("tex/a.png".to_string()),
                    ..SceneMaterialDecl::default()
                });
            }),
        ),
        (
            "materials.uv_scale",
            Box::new(|s: &mut Scene| {
                s.upsert_scene_material(SceneMaterialDecl {
                    name: "brick".to_string(),
                    uv_scale: [2.0, 2.0],
                    ..SceneMaterialDecl::default()
                });
            }),
        ),
        (
            "spawns.position",
            Box::new(|s: &mut Scene| {
                s.upsert_spawn(SpawnDecl::new("start", [3.0, 0.0, 1.0]));
            }),
        ),
        (
            "spawns.yaw",
            Box::new(|s: &mut Scene| {
                s.upsert_spawn(SpawnDecl {
                    yaw: 1.25,
                    ..SpawnDecl::new("start", [0.0, 0.0, 0.0])
                });
            }),
        ),
        (
            "spawns.name（新增一条）",
            Box::new(|s: &mut Scene| {
                s.upsert_spawn(SpawnDecl::new("respawn", [1.0, 0.0, 1.0]));
            }),
        ),
        (
            "node.material_ref",
            Box::new(|s: &mut Scene| {
                s.upsert_scene_material(SceneMaterialDecl {
                    name: "brick".to_string(),
                    ..SceneMaterialDecl::default()
                });
                assert!(s.set_node_material_ref("a", Some("brick")));
            }),
        ),
        (
            "next_id（不回退它会让回放假绿）",
            Box::new(|s: &mut Scene| {
                s.spawn(ObjectDesc {
                    name: "late".to_string(),
                    ..ObjectDesc::default()
                })
                .unwrap();
                let id = s.find_id("late").unwrap();
                s.despawn("late");
                // 节点数回到原样，只有 next_id 前进了。
                assert!(s.get(id).is_none());
            }),
        ),
    ];

    for (label, mutate) in cases {
        let mut doc = seeded();
        doc.write(mutate);
        assert_ne!(
            base,
            doc.state_hash(),
            "扰动 `{label}` 后哈希没变，说明该字段没进 StateHash"
        );
    }
}

/// S4-03：`time` / `frame` 不进内容哈希，只进世界哈希。
#[test]
fn world_hash_tracks_time_and_frame_but_state_hash_does_not() {
    let content = seeded();
    let before_content = content.state_hash();

    let mut ticked = seeded();
    ticked.write(|s| {
        s.time += 1.0 / 60.0;
        s.frame += 1;
    });

    assert_eq!(before_content, ticked.state_hash(), "节拍不该进内容哈希");
    assert_ne!(
        content.world_hash(),
        ticked.world_hash(),
        "世界哈希必须跟节拍走"
    );
}

/// 缓存以 `version` 为键：写一次就失效并给出新值。
#[test]
fn state_hash_cache_follows_version() {
    let mut doc = seeded();
    let before_version = doc.version();
    let first = doc.state_hash();
    assert_eq!(first, doc.state_hash());
    assert_eq!(before_version, doc.version(), "读哈希不该推进版本");

    doc.write(|s| s.camera.fov_deg += 2.0);
    assert!(doc.version() > before_version);
    assert_ne!(first, doc.state_hash());
}
