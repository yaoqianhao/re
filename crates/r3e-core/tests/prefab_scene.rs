//! E02 判据：Prefab 实例化与 `Scene` 集成。
//!
//! 本文件 14 条用例与 `docs/m2/M2_E_PLAN.md` §5.2 的 14 条判据一一对应
//! （内存共享 3 / 命名父级 3 / 序列化与往返 4 / 模板解析 2 / 装配复用 2）。
//! 组名与判据号写在每个测试函数的文档注释里，改判据时请同步改计划文档。
//!
//! 本文件的断言刻意分成两类：
//! * **可执行口径**（`Arc::ptr_eq` / `strong_count` / `layout_version`）—— 这些话
//!   只有代码能说清，文档说服不了调用方；
//! * **落盘口径**（`to_scene_file` 的段与键）—— 场景文件是对外契约，比结构体更重要。

use std::path::PathBuf;
use std::sync::Arc;

use r3e_core::prefab::{apply, FieldOverride, ObjectOverride, PrefabInstance, PrefabTemplate};
use r3e_core::scene_schema::{scene_root_properties, scene_schema_pretty, SCENE_SCHEMA_PATH};
use r3e_core::{ObjectDesc, Scene};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("crates/<crate>/tests 往上应有仓库根")
        .to_path_buf()
}

/// 最小模板：ID + 原型名 + 一个网格，其余走 `ObjectDesc` 默认值。
fn template(id: &str, proto_name: &str) -> PrefabTemplate {
    PrefabTemplate::new(
        id,
        ObjectDesc {
            name: proto_name.to_string(),
            mesh: Some("crate.glb".to_string()),
            tags: Some(vec!["prop".to_string(), "breakable".to_string()]),
            ..ObjectDesc::default()
        },
    )
}

/// 一个已登记 `prop.crate` 模板的空场景。
fn scene_with_template() -> Scene {
    let mut scene = Scene::new();
    scene
        .define_prefab(template("prop.crate", "crate"))
        .expect("登记模板");
    scene
}

/// 一个普通物件（用来验证「实例节点与手写节点共享同一套约束」）。
fn plain_object(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        ..ObjectDesc::default()
    }
}

// ---------------------------------------------------------------------------
// 内存共享（判据 ①②③）
// ---------------------------------------------------------------------------

/// 判据①：100 个同模板实例共享同一份模板——`Arc::ptr_eq` 全等、`strong_count == 实例数 + 1`。
///
/// 这条是「模板只有一个实体」的可执行口径：如果哪天实现改成每个实例克隆一份模板，
/// 指针就不等了，`strong_count` 也会退回 1。
#[test]
fn hundred_instances_share_one_template_arc() {
    let mut scene = scene_with_template();
    for index in 0..100 {
        scene
            .spawn_instance(PrefabInstance::new(format!("crate_{index}"), "prop.crate"))
            .unwrap_or_else(|err| panic!("第 {index} 个实例应能实例化：{err}"));
    }

    let first = scene.instance_template("crate_0").expect("实例应持有句柄");
    for index in 1..100 {
        let handle = scene
            .instance_template(&format!("crate_{index}"))
            .expect("每个实例都应持有句柄");
        assert!(
            Arc::ptr_eq(first, handle),
            "第 {index} 个实例与第一个实例必须指向同一份模板"
        );
    }

    // 表里那份 + 100 个实例 = 101；`prefabs.get` 借的是表里的 `Arc`，不会额外 +1。
    let table_handle = scene
        .prefabs
        .get("prop.crate")
        .expect("模板表里应有 prop.crate");
    assert_eq!(
        Arc::strong_count(table_handle),
        101,
        "100 个实例 + 模板表自身 = 101 个强引用"
    );
    assert_eq!(scene.instances.len(), 100, "实例记录数应等于实例数");
}

/// 判据②：不同模板的实例**不共享**——反例守卫（否则 ptr_eq 断言可能因为全空而假绿）。
#[test]
fn different_templates_do_not_share_arc() {
    let mut scene = Scene::new();
    scene
        .define_prefab(template("prop.crate", "crate"))
        .expect("登记 crate");
    scene
        .define_prefab(template("prop.barrel", "barrel"))
        .expect("登记 barrel");

    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("实例化 crate");
    scene
        .spawn_instance(PrefabInstance::new("barrel_1", "prop.barrel"))
        .expect("实例化 barrel");

    let crate_handle = scene.instance_template("crate_1").expect("crate 句柄");
    let barrel_handle = scene.instance_template("barrel_1").expect("barrel 句柄");
    assert!(
        !Arc::ptr_eq(crate_handle, barrel_handle),
        "不同模板的实例不该共享同一份模板"
    );
    assert_eq!(Arc::strong_count(crate_handle), 2, "1 个实例 + 模板表 = 2");
    assert_eq!(Arc::strong_count(barrel_handle), 2, "1 个实例 + 模板表 = 2");
}

/// 判据③：实例重名被拒（`instances` 表口径），失败不留半截记录。
#[test]
fn duplicate_instance_name_is_rejected() {
    let mut scene = scene_with_template();
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("首次实例化应成功");

    let error = scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect_err("同名实例应被拒");
    assert!(error.contains("已存在"), "报错要说清是重名，实际：{error}");
    assert_eq!(scene.instances.len(), 1, "被拒的实例不该落进实例表");
    assert!(
        scene.instance_template("crate_1").is_some(),
        "首个实例的句柄应保留"
    );
}

// ---------------------------------------------------------------------------
// 命名与父级（判据 ④⑤⑥）
// ---------------------------------------------------------------------------

/// 判据④：实例名与已有手写节点重名 → 走 `Scene::spawn` 的重名拦截，且不留实例记录。
///
/// 这条同时证明「实例化没有绕开 spawn 的重名检查」。
#[test]
fn instance_name_colliding_with_plain_node_is_rejected() {
    let mut scene = scene_with_template();
    scene
        .spawn(plain_object("crate_1"))
        .expect("手写节点应能生成");

    let error = scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect_err("与手写节点重名应被拒");
    assert!(
        error.contains("已存在"),
        "报错应来自 spawn 的重名拦截，实际：{error}"
    );
    assert!(
        !scene.is_instance_node("crate_1"),
        "失败后不该留下实例记录（否则 to_scene_file 会写出一条幽灵实例）"
    );
    assert!(
        scene.instance_template("crate_1").is_none(),
        "失败后不该留下模板句柄"
    );
}

/// 判据⑤：`parent` 按名解析成 `NodeId` 并挂进父的 `children`；父不存在则报错。
#[test]
fn parent_is_resolved_by_name() {
    let mut scene = scene_with_template();
    let room = scene.spawn(plain_object("room")).expect("生成父节点");

    let child = scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate").with_parent("room"))
        .expect("父名可解析时应能实例化");
    assert_eq!(
        scene.get(child).expect("实例节点应在场景里").parent,
        Some(room),
        "父级应解析成 room 的 NodeId"
    );
    assert_eq!(
        scene.find("room").expect("父节点应在").children,
        vec![child],
        "父节点的 children 应登记这个实例"
    );

    let error = scene
        .spawn_instance(PrefabInstance::new("crate_2", "prop.crate").with_parent("ghost"))
        .expect_err("父名不存在应报错");
    assert!(
        error.contains("父物件"),
        "报错要说清是父物件缺失，实际：{error}"
    );
    assert!(scene.find("crate_2").is_none(), "失败的实例不该留下节点");
}

/// 判据⑥：实例化会推进 `layout_version`（拓扑变了，渲染层缓存必须失效）。
#[test]
fn instantiate_touches_layout() {
    let mut scene = scene_with_template();
    let before = scene.layout_version();
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("实例化应成功");
    assert!(
        scene.layout_version() > before,
        "实例化新增了节点，layout_version 必须自增（{} -> {}）",
        before,
        scene.layout_version()
    );
}

// ---------------------------------------------------------------------------
// 序列化与往返（判据 ⑦⑧⑨⑩）
// ---------------------------------------------------------------------------

/// 判据⑦：`to_scene_file` / `from_file` 往返一致——实例表逐字段保留，二次落盘逐字节相同。
#[test]
fn instances_roundtrip_through_scene_file() {
    let mut scene = scene_with_template();
    scene.spawn(plain_object("floor")).expect("生成普通节点");
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("实例化 1");
    let overrides = ObjectOverride {
        position: Some(FieldOverride::set([2.0, 0.5, -1.0])),
        ..ObjectOverride::default()
    };
    scene
        .spawn_instance(
            PrefabInstance::new("crate_2", "prop.crate")
                .with_parent("floor")
                .with_override(overrides),
        )
        .expect("实例化 2");

    let text = scene.to_json_string().expect("场景应可序列化");
    let back = Scene::from_json_str(&text).expect("场景应可反序列化");

    assert_eq!(back.instances.len(), 2, "实例记录数往返不变");
    assert_eq!(back.instances[0].name, "crate_1", "实例名往返不变");
    assert_eq!(back.instances[1].template, "prop.crate", "模板 ID 往返不变");
    assert_eq!(
        back.instances[1].parent.as_deref(),
        Some("floor"),
        "父名往返不变"
    );
    assert_eq!(
        back.instances[1].overrides.count(),
        1,
        "覆盖数往返不变（且只落差异）"
    );
    assert_eq!(
        back.to_json_string().expect("二次序列化"),
        text,
        "往返后再落盘必须逐字节一致"
    );
    assert_eq!(
        serde_json::to_string(&back.instances).expect("序列化"),
        serde_json::to_string(&scene.instances).expect("序列化"),
        "实例表结构往返相等（含覆盖三态）"
    );
}

/// 判据⑧：实例化出来的节点**不写进 `objects`**——真源只有 `instances` 一处。
#[test]
fn instance_nodes_are_not_duplicated_into_objects() {
    let mut scene = scene_with_template();
    scene.spawn(plain_object("floor")).expect("生成普通节点");
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("实例化");

    let file = scene.to_scene_file();
    assert_eq!(file.instances.len(), 1, "实例表应有一条");
    assert!(
        file.objects.iter().all(|object| object.name != "crate_1"),
        "实例节点不该同时出现在 objects 里（两份真源会漂移）"
    );
    assert!(
        file.objects.iter().any(|object| object.name == "floor"),
        "普通节点必须照旧写进 objects"
    );
}

/// 判据⑨：旧场景（没有 `instances` 段）仍能读，且空实例表不落盘。
#[test]
fn scene_without_instances_still_loads() {
    let path = repo_root().join("assets/scene.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读不到 {}（{err}）", path.display()));
    assert!(
        !text.contains("\"instances\""),
        "这条判据的前提是样例场景里没有 instances 段"
    );

    let scene = Scene::from_json_str(&text).expect("没有 instances 段的旧场景必须能读");
    assert!(scene.instances.is_empty(), "缺段 = 空实例表");
    assert!(scene.prefabs.is_empty(), "缺段不会凭空造出模板");
    assert!(
        !scene
            .to_json_string()
            .expect("序列化")
            .contains("\"instances\""),
        "空实例表不落盘（旧场景 roundtrip 不该凭空长出 instances 键）"
    );
}

/// 判据⑩：`assets/scene.schema.json` 已随结构重生成，且含 `instances` 段。
///
/// 防漂移的完整比对在 `tests/scene_schema.rs`，这里只钉 E02 引入的那一段。
#[test]
fn scene_schema_declares_instances() {
    let properties = scene_root_properties();
    assert!(
        properties.contains(&"instances".to_string()),
        "schema 根级 properties 必须有 instances，实际：{properties:?}"
    );

    let path = repo_root().join(SCENE_SCHEMA_PATH);
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读不到 {}（{err}）", path.display()));
    assert_eq!(
        on_disk,
        scene_schema_pretty(),
        "schema 磁盘文件与派生结果不一致——请运行 `cargo run -p r3e-core --bin scene_schema_dump`"
    );
}

// ---------------------------------------------------------------------------
// 模板解析（判据 ⑪⑫）
// ---------------------------------------------------------------------------

/// 判据⑪：未注册模板 → 明确报错（`Conflict` 口径，不 panic），且不留半截状态。
#[test]
fn unknown_template_is_rejected() {
    let mut scene = Scene::new();
    let error = scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect_err("模板没登记就该被拒");
    assert!(
        error.contains("prefab 模板未注册"),
        "报错应指向「模板未注册」，实际：{error}"
    );
    assert!(
        error.contains("Conflict"),
        "口径要能被 E04 映射成 PrefabOpError::Conflict，实际：{error}"
    );
    assert!(scene.instances.is_empty(), "失败的实例不该落进实例表");
    assert!(scene.find("crate_1").is_none(), "失败的实例不该留下节点");
}

/// 判据⑫：覆盖按三态生效（`Set` 替换 / `Clear` 清空 / `None` 继承），且模板本体不落盘。
#[test]
fn overrides_apply_and_template_body_is_not_serialized() {
    let mut scene = scene_with_template();
    let overrides = ObjectOverride {
        // 清空模板的 tags（Clear 是「显式清空」，不是「继承」）
        tags: Some(FieldOverride::clear()),
        position: Some(FieldOverride::set([5.0, 1.0, 0.0])),
        visible: Some(FieldOverride::set(false)),
        ..ObjectOverride::default()
    };
    let id = scene
        .spawn_instance(
            PrefabInstance::new("crate_1", "prop.crate").with_override(overrides.clone()),
        )
        .expect("实例化应成功");

    let node = scene.get(id).expect("实例节点应在");
    assert!(node.tags.is_empty(), "Clear 应清空模板给的两个标签");
    assert_eq!(node.transform.position.x, 5.0, "Set 应替换位置");
    assert!(!node.visible, "Set 应替换可见性");

    // 实例化结果必须等于 apply(proto, overrides, name) 的机械展开（判据⑬的同源口径）
    let proto = scene
        .prefab_handle("prop.crate")
        .expect("模板句柄")
        .proto
        .clone();
    let expected = serde_json::to_string(&apply(&proto, &overrides, "crate_1")).expect("序列化");
    let resolved = serde_json::to_string(
        &scene
            .resolve_overrides(scene.instance("crate_1").expect("实例记录"))
            .expect("应能算出实体"),
    )
    .expect("序列化");
    assert_eq!(resolved, expected, "实例化结果应等于 apply 的机械展开");

    // 模板本体不进场景文件：网格名只存在于内存里的 proto
    let text = scene.to_json_string().expect("序列化");
    assert!(
        !text.contains("crate.glb"),
        "模板本体不该出现在场景文件里（落盘只有引用 + 差异）"
    );
    assert_eq!(
        scene
            .instance("crate_1")
            .expect("实例记录")
            .overrides
            .count(),
        3,
        "落盘的覆盖应只有 3 个字段，不含继承字段"
    );
}

// ---------------------------------------------------------------------------
// 装配复用（判据 ⑬⑭）
// ---------------------------------------------------------------------------

/// 判据⑬：`spawn_instances` 批量实例化与逐条实例化等价（同一套 spawn 语义）。
#[test]
fn batch_instantiation_equals_individual_calls() {
    let mut batched = scene_with_template();
    let ids = batched
        .spawn_instances(vec![
            PrefabInstance::new("crate_1", "prop.crate"),
            PrefabInstance::new("crate_2", "prop.crate").with_parent("crate_1"),
            PrefabInstance::new("crate_3", "prop.crate"),
        ])
        .expect("批量实例化应成功");
    assert_eq!(ids.len(), 3, "三条实例都应返回 NodeId");
    assert_eq!(batched.instances.len(), 3);

    let mut single = scene_with_template();
    single
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("1");
    single
        .spawn_instance(PrefabInstance::new("crate_2", "prop.crate").with_parent("crate_1"))
        .expect("2");
    single
        .spawn_instance(PrefabInstance::new("crate_3", "prop.crate"))
        .expect("3");

    assert_eq!(
        serde_json::to_string(&batched.to_scene_file()).expect("序列化"),
        serde_json::to_string(&single.to_scene_file()).expect("序列化"),
        "批量与逐条实例化必须得到同一份场景（同一条装配路径）"
    );

    // 批量失败即停：错误里要能定位到第几条
    let mut partial = scene_with_template();
    let error = partial
        .spawn_instances(vec![
            PrefabInstance::new("crate_1", "prop.crate"),
            PrefabInstance::new("crate_1", "prop.crate"),
        ])
        .expect_err("第二条重名应让整批失败");
    assert!(
        error.contains("第 1 条") && error.contains("crate_1"),
        "批量报错要带上是第几条与实例名，实际：{error}"
    );
    assert_eq!(partial.instances.len(), 1, "第一条已经落地，不回滚");
}

/// 判据⑭：实例化只复用 `Scene::spawn`——节点被删时实例记录同步摘除，补建幂等。
///
/// 这条钉的是「不留第二条装配路径」的后果：既然节点由 spawn 建、由 despawn 删，
/// 实例记录就必须跟着同一套生命周期走，否则会留下指向空气的幽灵实例。
#[test]
fn instance_records_follow_node_lifecycle() {
    let mut scene = scene_with_template();
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("实例化");

    assert!(scene.despawn("crate_1"), "实例节点应能被普通 despawn 删除");
    assert!(
        scene.instance("crate_1").is_none(),
        "节点没了，实例记录也要一起走"
    );
    assert!(
        scene.instance_template("crate_1").is_none(),
        "节点没了，模板句柄也要一起走"
    );
    assert!(
        !scene
            .to_scene_file()
            .instances
            .iter()
            .any(|item| item.name == "crate_1"),
        "落盘不该留下幽灵实例"
    );

    // `instantiate_declared`：模板到位后把声明的实例补出来，重复调用幂等
    let mut loaded = Scene::from_json_str(&{
        let mut declared = scene_with_template();
        declared
            .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
            .expect("实例化");
        declared.to_json_string().expect("序列化")
    })
    .expect("场景应可读");
    assert_eq!(loaded.instances.len(), 1, "实例记录应读回来");
    assert!(loaded.find("crate_1").is_none(), "载入不展开节点");

    loaded
        .define_prefab(template("prop.crate", "crate"))
        .expect("模板到位");
    assert_eq!(
        loaded.instantiate_declared().expect("补建应成功").len(),
        1,
        "模板到位后应补出 1 个节点"
    );
    assert!(loaded.find("crate_1").is_some(), "节点应已存在");
    assert!(
        loaded.instantiate_declared().expect("再补一次").is_empty(),
        "已实例化的不该重复建（幂等）"
    );
}
