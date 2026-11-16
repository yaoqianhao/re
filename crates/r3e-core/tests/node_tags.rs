//! `tags` / `interact_radius` 的落地契约（M1-S2-C）。
//!
//! 计划里的 R3 风险是「字段新增漏改 `node_desc` → 静默丢字段、查询结果对不上」，
//! 所以这里不测「结构体里有这个字段」，而是逐环测**数据能不能活着走完全程**：
//!
//! 1. `spawn` 收敛：空白、空串、重复、超长、超量；
//! 2. 半径语义：非正值与非有限值等于「不声明」；
//! 3. 场景文件 roundtrip：写了什么读回来还是什么，没写就不该凭空长出来；
//! 4. `node_desc` 输出：`to_scene_file()` 里能看见（R3 的直接守卫）。

use r3e_core::{
    normalize_interact_radius, normalize_tags, ObjectDesc, Scene, MAX_NODE_TAGS, NODE_TAG_MAX_LEN,
};

fn scene_with(desc: ObjectDesc) -> Scene {
    let mut scene = Scene::new();
    scene.spawn(desc).unwrap();
    scene
}

fn tagged(name: &str, tags: &[&str], radius: Option<f32>) -> Scene {
    scene_with(ObjectDesc {
        name: name.to_string(),
        tags: Some(tags.iter().map(|t| (*t).to_string()).collect()),
        interact_radius: radius,
        ..ObjectDesc::default()
    })
}

// ---------------------------------------------------------------------------
// 1. 收敛
// ---------------------------------------------------------------------------

#[test]
fn spawn_normalizes_tags() {
    let scene = tagged("door", &[" door ", "", "door", "enemy", "  "], None);
    let node = scene.get(scene.find_id("door").unwrap()).unwrap();
    assert_eq!(
        vec!["door".to_string(), "enemy".to_string()],
        node.tags,
        "空白与空串应丢弃、重复应去重且保持首次出现顺序"
    );
}

#[test]
fn spawn_caps_tag_count_and_length() {
    let many: Vec<String> = (0..40).map(|i| format!("tag{i}")).collect();
    let scene = scene_with(ObjectDesc {
        name: "many".to_string(),
        tags: Some(many),
        ..ObjectDesc::default()
    });
    let node = scene.get(scene.find_id("many").unwrap()).unwrap();
    assert_eq!(MAX_NODE_TAGS, node.tags.len(), "标签条数应截到上限");

    let long = "x".repeat(NODE_TAG_MAX_LEN + 10);
    let scene = scene_with(ObjectDesc {
        name: "long".to_string(),
        tags: Some(vec![long]),
        ..ObjectDesc::default()
    });
    let node = scene.get(scene.find_id("long").unwrap()).unwrap();
    assert_eq!(NODE_TAG_MAX_LEN, node.tags[0].chars().count());
}

#[test]
fn normalize_tags_keeps_case_sensitive_distinct_tags() {
    // 大小写不同是不同标签：查询面按字面匹配，不做隐式大小写折叠
    assert_eq!(
        vec!["Door".to_string(), "door".to_string()],
        normalize_tags(["Door", "door"])
    );
    assert!(normalize_tags(Vec::<String>::new()).is_empty());
}

#[test]
fn spawn_drops_non_positive_or_non_finite_interact_radius() {
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let scene = tagged("bad", &[], Some(bad));
        let node = scene.get(scene.find_id("bad").unwrap()).unwrap();
        assert_eq!(None, node.interact_radius, "半径 {bad} 应视为不可交互");
        assert_eq!(None, normalize_interact_radius(bad));
    }

    let scene = tagged("ok", &[], Some(1.5));
    let node = scene.get(scene.find_id("ok").unwrap()).unwrap();
    assert_eq!(Some(1.5), node.interact_radius);
}

// ---------------------------------------------------------------------------
// 2. node_desc（R3 守卫）
// ---------------------------------------------------------------------------

#[test]
fn node_desc_carries_tags_and_radius() {
    let scene = tagged("chest", &["pickup", "loot"], Some(0.75));
    let file = scene.to_scene_file();
    let desc = &file.objects[0];
    assert_eq!(
        Some(vec!["pickup".to_string(), "loot".to_string()]),
        desc.tags,
        "node_desc 漏了 tags 会静默丢字段"
    );
    assert_eq!(Some(0.75), desc.interact_radius);
}

#[test]
fn node_desc_omits_empty_tags_and_absent_radius() {
    let scene = scene_with(ObjectDesc {
        name: "plain".to_string(),
        ..ObjectDesc::default()
    });
    let file = scene.to_scene_file();
    let desc = &file.objects[0];
    assert_eq!(None, desc.tags, "无标签不该输出空数组");
    assert_eq!(None, desc.interact_radius);
}

// ---------------------------------------------------------------------------
// 3. 场景文件 roundtrip
// ---------------------------------------------------------------------------

#[test]
fn tags_and_radius_survive_scene_roundtrip() {
    let scene = tagged("npc", &["enemy", "boss"], Some(3.0));
    let json = scene.to_json_pretty().unwrap();
    assert!(json.contains("\"tags\""), "序列化应写出 tags：{json}");
    assert!(json.contains("\"interact_radius\""));

    let back = Scene::from_json_str(&json).unwrap();
    let node = back.get(back.find_id("npc").unwrap()).unwrap();
    assert_eq!(vec!["enemy".to_string(), "boss".to_string()], node.tags);
    assert_eq!(Some(3.0), node.interact_radius);
}

#[test]
fn scene_without_new_fields_roundtrips_byte_identical() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "box".to_string(),
            ..ObjectDesc::default()
        })
        .unwrap();
    let first = scene.to_json_pretty().unwrap();
    assert!(
        !first.contains("tags"),
        "旧场景 roundtrip 不该凭空长出 tags"
    );
    assert!(!first.contains("interact_radius"));

    let back = Scene::from_json_str(&first).unwrap();
    assert_eq!(
        first,
        back.to_json_pretty().unwrap(),
        "roundtrip 应逐字节稳定"
    );
}

#[test]
fn scene_file_can_declare_tags_inline() {
    // 生成式使用方（AI / 外部工具）只写 JSON，这条走的是纯 serde 路径
    let json = r#"{
        "objects": [
            {"name": "lever", "tags": [" interactable ", "switch"], "interact_radius": 2.0}
        ]
    }"#;
    let scene = Scene::from_json_str(json).unwrap();
    let node = scene.get(scene.find_id("lever").unwrap()).unwrap();
    assert_eq!(
        vec!["interactable".to_string(), "switch".to_string()],
        node.tags
    );
    assert_eq!(Some(2.0), node.interact_radius);
}
