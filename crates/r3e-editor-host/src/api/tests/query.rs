//! `query.*`（M1-S2-B）宿主层：过滤口径、截断回报、错误码，以及「查询不动 StateHash」。
//!
//! 这里守的是**协议到引擎的翻译**：`QuerySpec` 的字符串名字要被裁决成 `NodeKind` /
//! `Component`，`limit` 的缺省要落在 `DEFAULT_LIMIT`，`truncated` 要如实反映「还有更多」。
//! 过滤判据本身由 `r3e-core` 的 `tests/world_query.rs` 守着，这里不重复。

use super::*;
use r3e_core::DEFAULT_LIMIT;
use r3e_editor::{ENGINE_ERROR, INVALID_PARAMS};

/// 三个物件的语义场景：
///
/// | 名字 | 位置 x | 标签 | 可交互 |
/// |---|---|---|---|
/// | `door` | 0 | `door` `wood` | 是（1.5） |
/// | `crate` | 10 | `wood` | 否 |
/// | `lamp` | 20 | 无 | 否 |
///
/// 位置沿 x 轴拉开，区域过滤才有区分度；世界变换在 fixture 里先算一遍——
/// `Scene::query` 判的是**世界原点**，没更新过的变换全是单位阵，区域断言会假通过。
fn query_fixture() -> Fixture {
    let mut scene = Scene::new();
    scene.name = "query-test".to_string();
    for (name, x, tags, radius) in [
        (
            "door",
            0.0_f32,
            Some(vec!["door".to_string(), "wood".to_string()]),
            Some(1.5_f32),
        ),
        ("crate", 10.0, Some(vec!["wood".to_string()]), None),
        ("lamp", 20.0, None, None),
    ] {
        scene
            .spawn(ObjectDesc {
                name: name.to_string(),
                mesh: Some("cube".to_string()),
                position: Some([x, 0.0, 0.0]),
                tags,
                interact_radius: radius,
                ..Default::default()
            })
            .expect("测试场景 spawn 不应失败");
    }
    // 世界变换在**入 `Document` 之前**算一遍：`Scene::query` 判的是世界原点，
    // 没更新过的变换全是单位阵，区域断言会假通过。放这儿也省掉一次宿主写通道调用。
    scene.update_world_transforms();
    let scene = Rc::new(RefCell::new(Document::new(scene)));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    (scene, physics, scripts)
}

/// 名字表 → `Vec<String>`，省得每条用例都写一屏 `.to_string()`。
fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn query_find_缺省参数列出全部节点() {
    let (scene, physics, mut scripts) = query_fixture();
    let value = api(&scene, &physics, &mut scripts)
        .query_find(QuerySpec::default())
        .expect("空过滤条件应成功");

    assert_eq!(value["count"], 3, "三个物件都该命中：{value}");
    assert_eq!(value["limit"], DEFAULT_LIMIT, "缺省上限应取 core 常量");
    assert_eq!(value["truncated"], false);
    assert_eq!(value["unfiltered"], true, "一个过滤条件都没给");

    let nodes = value["nodes"].as_array().expect("nodes 应是数组");
    assert_eq!(nodes.len(), 3);
    // 插入顺序即返回顺序（core 的执行器不改序）。
    assert_eq!(nodes[0]["name"], "door");
    assert_eq!(nodes[1]["name"], "crate");
    assert_eq!(nodes[2]["name"], "lamp");
    assert_eq!(nodes[0]["interactRadius"], 1.5);
    assert_eq!(nodes[2]["interactRadius"], Value::Null, "不可交互该是 null");
    assert_eq!(nodes[0]["tags"], json!(["door", "wood"]));
}

#[test]
fn query_find_按标签与组件位过滤() {
    let (scene, physics, mut scripts) = query_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let wood = api
        .query_find(QuerySpec {
            all_tags: names(&["wood"]),
            ..Default::default()
        })
        .expect("标签过滤应成功");
    assert_eq!(wood["count"], 2, "door 与 crate 都带 wood：{wood}");

    // 「有标签但不可交互」：只剩 crate。
    let tagged_static = api
        .query_find(QuerySpec {
            require: names(&["tagged"]),
            exclude: names(&["interact"]),
            ..Default::default()
        })
        .expect("组件位过滤应成功");
    assert_eq!(tagged_static["count"], 1, "{tagged_static}");
    assert_eq!(tagged_static["nodes"][0]["name"], "crate");
}

#[test]
fn query_find_按区域过滤且对角点顺序不限() {
    let (scene, physics, mut scripts) = query_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    // 包围 [-1,-1,-1]..[1,1,1]：只有 door（x=0）落在里面。
    let inside = api
        .query_find(QuerySpec {
            region: Some([-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]),
            ..Default::default()
        })
        .expect("区域过滤应成功");
    assert_eq!(inside["count"], 1, "{inside}");
    assert_eq!(inside["nodes"][0]["name"], "door");

    // 对角点写反（max 在前）也要命中同一个：`Aabb::new` 内部按分量归一化。
    let reversed = api
        .query_find(QuerySpec {
            region: Some([1.0, 1.0, 1.0, -1.0, -1.0, -1.0]),
            ..Default::default()
        })
        .expect("反序对角点应被归一化");
    assert_eq!(reversed["count"], 1, "{reversed}");
}

#[test]
fn query_find_截断如实回报() {
    let (scene, physics, mut scripts) = query_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let short = api
        .query_find(QuerySpec {
            limit: Some(2),
            ..Default::default()
        })
        .expect("截断查询应成功");
    assert_eq!(short["count"], 2);
    assert_eq!(short["limit"], 2);
    assert_eq!(short["truncated"], true, "还有第 3 个没返回：{short}");

    // `limit: 0` 是「不限」，此时不可能截断。
    let all = api
        .query_find(QuerySpec {
            limit: Some(0),
            ..Default::default()
        })
        .expect("不限条数应成功");
    assert_eq!(all["count"], 3);
    assert_eq!(all["limit"], 0);
    assert_eq!(all["truncated"], false);
}

#[test]
fn query_find_拼错名字回参数错误而不是空结果() {
    let (scene, physics, mut scripts) = query_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let bad_kind = api
        .query_find(QuerySpec {
            kind: Some("dorr".to_string()),
            ..Default::default()
        })
        .expect_err("未知类别必须报错");
    assert_eq!(bad_kind.code, INVALID_PARAMS, "{}", bad_kind.message);
    assert!(
        bad_kind.message.contains("dorr") && bad_kind.message.contains("interactable"),
        "错误信息要点名拼错的值并列出合法取值：{}",
        bad_kind.message
    );

    let bad_component = api
        .query_find(QuerySpec {
            require: names(&["matrial"]),
            ..Default::default()
        })
        .expect_err("未知组件位必须报错");
    assert_eq!(bad_component.code, INVALID_PARAMS);
    assert!(
        bad_component.message.contains("matrial") && bad_component.message.contains("material"),
        "{}",
        bad_component.message
    );
}

#[test]
fn query_describe_给出投影与命中的组件位() {
    let (scene, physics, mut scripts) = query_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let value = api.query_describe("door").expect("door 应存在");
    assert_eq!(value["node"]["name"], "door");
    assert_eq!(value["node"]["interactRadius"], 1.5);

    let components = value["components"].as_array().expect("components 应是数组");
    assert!(components.contains(&json!("interact")), "{value}");
    assert!(components.contains(&json!("tagged")), "{value}");
    assert!(!components.contains(&json!("skinned")), "door 没有蒙皮");

    let kinds = value["kinds"].as_array().expect("kinds 应是数组");
    assert!(kinds.contains(&json!("interactable")), "{value}");
    assert!(
        !kinds.contains(&json!("any")),
        "恒真的 any 不该出现在详情里"
    );
}

#[test]
fn query_describe_未知物件回不存在而不是空对象() {
    let (scene, physics, mut scripts) = query_fixture();
    let failure = api(&scene, &physics, &mut scripts)
        .query_describe("ghost")
        .expect_err("ghost 不存在");

    assert_eq!(failure.code, ENGINE_ERROR);
    assert!(failure.message.contains("ghost"), "{}", failure.message);
}

#[test]
fn query_stats_给出全貌() {
    let (scene, physics, mut scripts) = query_fixture();
    let value = api(&scene, &physics, &mut scripts)
        .query_stats()
        .expect("普查应成功");

    assert_eq!(value["nodes"], 3);
    assert_eq!(value["visible"], 3);
    assert_eq!(value["interactable"], 1);
    assert_eq!(value["tagged"], 2, "door 与 crate 带标签");
    assert_eq!(value["tags"]["wood"], 2);
    assert_eq!(value["tags"]["door"], 1);
    assert_eq!(value["byKind"]["interactable"], 1);
    assert!(
        value["byKind"].get("skinned").is_none(),
        "命中数为 0 的类别不该出现：{value}"
    );
}

#[test]
fn query三条方法都不改状态哈希() {
    let (scene, physics, mut scripts) = query_fixture();

    let before = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };

    {
        let mut api = api(&scene, &physics, &mut scripts);
        api.query_find(QuerySpec {
            all_tags: names(&["wood"]),
            ..Default::default()
        })
        .expect("find 应成功");
        api.query_describe("door").expect("describe 应成功");
        api.query_stats().expect("stats 应成功");
    }

    let after = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };

    assert_eq!(before, after, "查询是只读的：哈希与写次数都不许动");
}
