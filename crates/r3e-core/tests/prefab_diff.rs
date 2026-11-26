//! M2-E03 判据：覆盖只落差异、三态语义、可逆性与体积组合判据（core 侧 10 条）。
//!
//! 与 `docs/m2/M2_E_PLAN.md` §6.2 的 12 条判据对应关系：只落差异 ①②③、三态语义 ④⑤、
//! 可逆性 ⑥⑦、体积组合判据 ⑨⑩⑪（共 10 条）；⑧⑫ 需要 `Document` 的 undo 栈与前像，
//! 落在 `crates/r3e-doc/tests/prefab_command.rs`。
//!
//! 为什么断言一律比「序列化后逐字节」而不是比结构体：`PhysicsDesc` / `SurfaceMaterial`
//! 没有实现 `PartialEq`，而本卡真正要冻结的正是**落盘外形**（哪些键在、哪些键是 `null`）。
//! 这也是判据⑥⑫ 的口径——字节相等即「写回场景文件后逐字节相同」。

use serde::Serialize;
use serde_json::{json, Value};

use r3e_core::prefab::{
    apply, diff, revert, ObjectOverride, OverrideState, PrefabInstance, PrefabTemplate,
};
use r3e_core::{ObjectDesc, PhysicsDesc, Scene, SurfaceMaterial};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 全 13 字段填满的原型夹具（判据⑨⑩⑪ 的模板侧）。
///
/// 字段值取自 `assets/prefab/prop/crate.json` 的 `proto`，并把 `parent` /
/// `materialRef` 补齐、资产路径按真实工程长度写长——`P2` 组合判据要求模板落盘
/// **≥ 480 B**（见判据⑩ 里的实测值），精简夹具会让体积比失去意义。
fn full_proto() -> ObjectDesc {
    ObjectDesc {
        name: "crate".to_string(),
        mesh: Some("props/props_library/crate_large_wooden_v2.glb".to_string()),
        position: Some([0.0, 0.4, 0.0]),
        rotation: Some([0.0, 0.0, 0.0]),
        scale: Some([1.0, 1.0, 1.0]),
        color: Some([0.72, 0.55, 0.34, 1.0]),
        parent: Some("shelf/shelf_row_1/slot_3".to_string()),
        visible: Some(true),
        physics: Some(PhysicsDesc {
            body: "dynamic".to_string(),
            mass: 12.0,
            restitution: Some(0.15),
            friction: Some(0.6),
            collider: Some("box".to_string()),
            half_extents: Some([0.4, 0.4, 0.4]),
            ccd: Some(false),
        }),
        material: Some(SurfaceMaterial {
            metallic: 0.0,
            roughness: 0.62,
            emissive: [0.0, 0.0, 0.0],
        }),
        material_ref: Some("wood/wood_crate_outer_v2".to_string()),
        tags: Some(vec![
            "prop".to_string(),
            "crate".to_string(),
            "breakable".to_string(),
            "physics".to_string(),
            "grabbable".to_string(),
        ]),
        interact_radius: Some(1.2),
    }
}

/// 只把 `visible` 翻成 `false` 的目标形态（判据⑨⑩⑪ 的目标侧）。
fn proto_visible_off(proto: &ObjectDesc) -> ObjectDesc {
    let mut target = proto.clone();
    target.visible = Some(false);
    target
}

/// 序列化成紧凑 JSON 字节（判据⑥⑩⑪ 一律用它比长度 / 比相等）。
fn json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("夹具必须可序列化")
}

/// 序列化成 JSON 值（用来取字段表）。
fn json_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("夹具必须可序列化")
}

/// 取 `ObjectDesc` 的字段表（键 = 落盘键）。
fn desc_fields(desc: &ObjectDesc) -> serde_json::Map<String, Value> {
    match json_value(desc) {
        Value::Object(fields) => fields,
        other => panic!("ObjectDesc 必须序列化成 JSON 对象，实际是 {other}"),
    }
}

/// 覆盖里「有动作」的字段名（`Set` 与 `Clear` 都算，与 `ObjectOverride::count` 同口径）。
fn overridden_fields(overrides: &ObjectOverride) -> Vec<&'static str> {
    ObjectOverride::FIELD_NAMES
        .iter()
        .copied()
        .filter(|name| {
            overrides
                .field_state(name)
                .is_some_and(OverrideState::is_overridden)
        })
        .collect()
}

/// 场景落盘字节：`state_hash` 的输入就是这份 `SceneFile` 的序列化结果，
/// 故这里字节相等 ⇔ doc 层哈希相等（真哈希断言在 `prefab_command.rs`）。
fn scene_bytes(scene: &Scene) -> Vec<u8> {
    json_bytes(&scene.to_scene_file())
}

// ---------------------------------------------------------------------------
// 只落差异（判据 ①②③）
// ---------------------------------------------------------------------------

/// 判据①：覆盖 1 个字段 → 其余 11 个可覆盖字段全为「继承」（`None`）。
#[test]
fn diff_records_only_the_changed_field() {
    let proto = full_proto();
    let mut target = proto.clone();
    target.position = Some([1.5, 0.4, 2.25]);

    let overrides = diff(&proto, &target);

    assert_eq!(overrides.count(), 1, "只改了 position，只能落 1 条覆盖");
    assert_eq!(
        overrides.field_state("position"),
        Some(OverrideState::Set),
        "position 落成 Set"
    );
    for name in ObjectOverride::FIELD_NAMES {
        if *name == "position" {
            continue;
        }
        assert_eq!(
            overrides.field_state(name),
            Some(OverrideState::Inherit),
            "`{name}` 没改，必须是继承（键缺失）"
        );
    }
    // 落盘外形：覆盖里只有 position 一个键，不是整份原型。
    let fields = match json_value(&overrides) {
        Value::Object(fields) => fields,
        other => panic!("覆盖必须序列化成 JSON 对象，实际是 {other}"),
    };
    assert_eq!(fields.keys().collect::<Vec<_>>(), vec!["position"]);
}

/// 判据②：覆盖值**等于**模板值 → 收敛为「未覆盖」，整段 `overrides` 不落盘。
#[test]
fn diff_converges_equal_value_to_inherit() {
    let proto = full_proto();
    // 逐个字段写成「与模板相同的值」，一条都不该记。
    let delta = diff(&proto, &proto.clone());

    assert!(delta.is_empty(), "与模板完全一致时不该有任何覆盖");
    assert_eq!(delta.count(), 0);
    assert_eq!(overridden_fields(&delta), Vec::<&'static str>::new());
    assert_eq!(json_bytes(&delta), b"{}", "空覆盖序列化成空对象");

    // 实例侧：空覆盖 → 整段 `overrides` 省略（判据③ 的落盘开关，E01 已冻结）。
    let instance = PrefabInstance::new("crate#1", "prop.crate").with_override(delta);
    let instance_json = json_value(&instance);
    assert!(
        instance_json.get("overrides").is_none(),
        "空覆盖不得落盘，否则每条实例都背一段 `{{}}`"
    );
}

/// 判据③：覆盖值不等 → 落为该字段的 `Set`；且 `set → clear → set` 幂等（同终态同字节）。
#[test]
fn set_clear_set_is_idempotent() {
    let proto = full_proto();
    let target = proto_visible_off(&proto);

    let mut instance = PrefabInstance::new("crate#1", "prop.crate");
    instance.overrides = diff(&proto, &target);
    let after_set = json_bytes(&instance);
    assert_eq!(
        instance.overrides.field_state("visible"),
        Some(OverrideState::Set)
    );

    // clear：显式清空（落 `null`）。它不是「回到继承」，是另一个动作。
    instance.overrides = serde_json::from_value(json!({ "visible": null })).expect("清空外形合法");
    let after_clear = json_bytes(&instance);
    assert_ne!(after_set, after_clear, "clear 是动作，字节必须与 set 不同");
    assert_eq!(
        instance.overrides.field_state("visible"),
        Some(OverrideState::Clear)
    );

    // 再 set 回同一取值 → 与第一次 set 的字节完全一致（幂等）。
    instance.overrides = diff(&proto, &target);
    assert_eq!(json_bytes(&instance), after_set);
}

// ---------------------------------------------------------------------------
// 三态语义（判据 ④⑤）
// ---------------------------------------------------------------------------

/// 判据④：`Clear` 让 `apply` 结果为 `None`（把模板里的值清掉，而不是继承）。
#[test]
fn clear_makes_apply_none() {
    let proto = full_proto();
    // physics / material 是复合字段：清空后必须是 None，而不是「模板里的那份」。
    let overrides: ObjectOverride =
        serde_json::from_value(json!({ "visible": null, "material": null }))
            .expect("清空两份字段的外形合法");

    let out = apply(&proto, &overrides, "crate#1");

    assert_eq!(out.visible, None, "Clear 后 visible 必须是 None");
    assert!(out.material.is_none(), "Clear 后 material 必须是 None");
    // 相邻字段不受影响（清空是逐字段的动作，不是整份重置）。
    assert_eq!(out.mesh, proto.mesh);
    assert_eq!(out.interact_radius, proto.interact_radius);
}

/// 判据⑤：`None`（键缺失）让 `apply` 结果为模板原值（继承），名字取实例名。
#[test]
fn inherit_keeps_proto_values() {
    let proto = full_proto();
    let overrides = ObjectOverride::default();

    let out = apply(&proto, &overrides, "crate#1");

    assert_eq!(out.visible, Some(true), "继承必须拿到模板值 true");
    assert_eq!(out.position, Some([0.0, 0.4, 0.0]));
    assert_eq!(out.interact_radius, Some(1.2));
    // 名字换成实例名，其余字节与原型一致（模板的 `proto.name` 只是原型名）。
    let mut expected = proto.clone();
    expected.name = "crate#1".to_string();
    assert_eq!(json_bytes(&out), json_bytes(&expected));
}

// ---------------------------------------------------------------------------
// 可逆性（判据 ⑥⑦）
// ---------------------------------------------------------------------------

/// 判据⑥：`set → revert` 后序列化字节 `A == B`（初末相等）。
#[test]
fn set_then_revert_restores_bytes() {
    let proto = full_proto();
    let mut target = proto.clone();
    target.position = Some([1.5, 0.4, 2.25]);

    let mut instance = PrefabInstance::new("crate#1", "prop.crate");
    let before = json_bytes(&instance);

    instance.overrides = diff(&proto, &target);
    let with_override = json_bytes(&instance);
    assert_ne!(before, with_override, "set 必须改到字节，否则判据恒真");

    instance.overrides = revert(&instance.overrides);
    let after = json_bytes(&instance);
    assert_eq!(before, after, "revert 后必须逐字节回到 A");
    // 覆盖不引入原型快照：多出来的字节只是 `position` 那一段。
    assert!(
        with_override.len() < before.len() + 64,
        "覆盖段膨胀到 {} B（A = {} B），疑似落了整份原型",
        with_override.len(),
        before.len()
    );
}

/// 判据⑦：场景级可逆——覆盖改动后 `revert`，场景落盘字节初末相等。
///
/// `Document::state_hash` 的输入就是 `Scene::to_scene_file()` 的序列化结果，故此处
/// 字节相等 ⇒ 哈希相等；doc 层的真哈希断言在 `r3e-doc/tests/prefab_command.rs`（判据⑫）。
#[test]
fn scene_bytes_restored_after_revert() {
    let proto = full_proto();
    let target = proto_visible_off(&proto);

    let mut scene = Scene::new();
    scene
        .define_prefab(PrefabTemplate::new("prop.crate", proto.clone()))
        .expect("模板 ID 形状合法、表未满");
    scene
        .spawn_instance(PrefabInstance::new("crate#1", "prop.crate"))
        .expect("模板已登记，实例名未占用");

    let before = scene_bytes(&scene);

    scene.instances[0].overrides = diff(&proto, &target);
    let changed = scene_bytes(&scene);
    assert_ne!(before, changed, "覆盖必须进场景落盘（`instances` 段）");
    assert!(
        changed.windows(5).any(|window| window == b"false"),
        "落盘的实例段里应能看到覆盖取值"
    );

    scene.instances[0].overrides = revert(&scene.instances[0].overrides);
    assert_eq!(before, scene_bytes(&scene), "revert 后场景字节初末相等");
}

// ---------------------------------------------------------------------------
// 体积（判据 ⑨⑩⑪，`P2` 组合判据）
// ---------------------------------------------------------------------------

/// 判据⑨（结构判据，主判）：`overrides` 里与模板同值的字段数 == 0，且字段数 ≤ 1。
///
/// 与体积无关，因此不受浮点文本化长度漂移影响——这是 `P2` 档 A 的「主判」。
#[test]
fn structural_no_equal_field_in_overrides() {
    let proto = full_proto();
    let mut target = proto.clone();
    target.visible = Some(false);

    let overrides = diff(&proto, &target);
    let proto_fields = desc_fields(&proto);
    let override_fields = match json_value(&overrides) {
        Value::Object(fields) => fields,
        other => panic!("覆盖必须序列化成 JSON 对象，实际是 {other}"),
    };

    let mut equal_count = 0;
    for (name, value) in &override_fields {
        if proto_fields.get(name) == Some(value) {
            equal_count += 1;
        }
    }
    assert_eq!(equal_count, 0, "与模板同值的字段不得出现在覆盖里");
    assert!(
        overridden_fields(&overrides).len() <= 1,
        "只改 1 个字段，覆盖字段数必须 ≤ 1"
    );
}

/// 判据⑩（体积判据，辅判）：`overrides` 段字节 / 全字段模板 JSON 字节 < 5%。
///
/// 夹具 = 全 13 字段填满的原型（判据要求 ≥ 480 B），实测值写在这里留痕：
/// 模板 `505 B`、覆盖 `{"visible":false}` = `17 B`、比值 `3.37%`（< 5%）。
/// 阈值与判据类别是 `P2` 拍定的，实测变化时**只准补夹具长度**，不许改阈值。
#[test]
fn volume_ratio_below_five_percent() {
    let proto = full_proto();
    let target = proto_visible_off(&proto);
    let instance =
        PrefabInstance::new("crate#1", "prop.crate").with_override(diff(&proto, &target));

    let template_len = json_bytes(&proto).len();
    let overrides_len = json_bytes(&instance.overrides).len();
    assert!(
        template_len >= 480,
        "夹具模板必须 ≥ 480 B（实际 {template_len} B），否则比值失去意义"
    );

    let ratio = overrides_len as f64 / template_len as f64;
    assert!(
        ratio < 0.05,
        "覆盖段 {overrides_len} B / 模板 {template_len} B = {:.2}%，超出 5%",
        ratio * 100.0
    );
}

/// 判据⑪（防退化，硬判据）：实例若退化成「13 字段全落盘」，体积比必然 ≥ 100%。
///
/// 反向自检：把覆盖填成整份快照，比值必须涨到 1.0 以上——这证明判据⑩ 的 5% 阈值
/// 不是「怎么写都能过」，一旦实现退化成快照式覆盖，⑩ 一定红。
#[test]
fn degenerate_full_snapshot_exceeds_template() {
    let proto = full_proto();
    let mut snapshot = desc_fields(&proto);
    snapshot.remove("name"); // `name` 不可覆盖（实例名由实例自己给）

    let overrides: ObjectOverride =
        serde_json::from_value(Value::Object(snapshot)).expect("整份原型就是一份合法覆盖");
    assert_eq!(overrides.count(), 12, "退化夹具必须是 12 个可覆盖字段全填");

    let instance = PrefabInstance::new("crate#1", "prop.crate").with_override(overrides);
    let template_len = json_bytes(&proto).len();
    let instance_len = json_bytes(&instance).len();

    let ratio = instance_len as f64 / template_len as f64;
    assert!(
        ratio >= 1.0,
        "退化实例 {instance_len} B / 模板 {template_len} B = {:.2}%，未达 100%——防退化断言失效",
        ratio * 100.0
    );
}
