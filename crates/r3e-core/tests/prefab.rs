//! E01 判据：Prefab 模板 / 实例 / 覆盖三件套的**外形冻结**。
//!
//! 本文件 18 条用例与 `docs/m2/M2_E_PLAN.md` §4.2 的 18 条判据一一对应
//! （往返 4 / 三态外形 5 / `PrefabId` 形状 3 / `ObjectOverride` 机械化 3 / schema 3）。
//! 组名与判据号都写在每个 `#[test]` 的文档注释里，改动时请同步改计划文档。
//!
//! 为什么大量断言是「序列化后逐字节比」而不是比结构体：`PhysicsDesc` /
//! `SurfaceMaterial` 没有 `PartialEq`，而本卡真正要冻结的正是**落盘外形**
//! （哪些键在、哪些键是 `null`、键序如何）——比结构体反而比不到这些。

use std::path::PathBuf;

use serde_json::{json, Value};

use r3e_core::prefab::{
    prefab_rel_path, validate_prefab_id, FieldOverride, ObjectOverride, OverrideState,
    PrefabIdError, PrefabInstance, PrefabTemplate, PREFAB_SCHEMA_VERSION,
};
use r3e_core::prefab_schema::{
    prefab_root_properties, prefab_schema, prefab_schema_pretty, PREFAB_SCHEMA_PATH,
};
use r3e_core::ObjectDesc;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 造一个最小可用模板：ID + 原型名，其余字段走 `ObjectDesc` 默认值。
fn template(id: &str, proto_name: &str) -> PrefabTemplate {
    PrefabTemplate::new(
        id,
        ObjectDesc {
            name: proto_name.to_string(),
            ..ObjectDesc::default()
        },
    )
}

/// 仓库根目录（`crates/r3e-core/tests/xx.rs` 往上三层）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("crates/<crate>/tests 往上应有仓库根")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// 往返（判据 ①②③④）
// ---------------------------------------------------------------------------

/// 判据①：`PrefabTemplate` ↔ JSON 往返逐字段等值。
#[test]
fn template_roundtrip_keeps_every_field() {
    let mut origin = template("prop.crate", "crate");
    origin.tags = vec!["prop".to_string(), "breakable".to_string()];

    let text = serde_json::to_string(&origin).expect("模板应可序列化");
    let back: PrefabTemplate = serde_json::from_str(&text).expect("模板应可反序列化");

    assert_eq!(back.id, "prop.crate", "ID 往返不变");
    assert_eq!(back.schema, PREFAB_SCHEMA_VERSION, "版本号往返不变");
    assert_eq!(back.proto.name, "crate", "原型名往返不变");
    assert_eq!(back.tags, vec!["prop", "breakable"], "标签顺序与内容不变");
    // 再序列化一次必须逐字节相同：字段序与省略规则都稳定。
    assert_eq!(
        serde_json::to_string(&back).expect("二次序列化"),
        text,
        "模板往返后再落盘必须逐字节一致"
    );
}

/// 判据②：`PrefabInstance` 往返（含父节点与覆盖）。
#[test]
fn instance_roundtrip_keeps_parent_and_overrides() {
    let instance = PrefabInstance::new("crate_2", "prop.crate")
        .with_parent("room_a")
        .with_override(ObjectOverride {
            visible: Some(FieldOverride::set(false)),
            ..ObjectOverride::default()
        });

    let text = serde_json::to_string(&instance).expect("实例应可序列化");
    let back: PrefabInstance = serde_json::from_str(&text).expect("实例应可反序列化");

    assert_eq!(back.name, "crate_2");
    assert_eq!(back.template, "prop.crate");
    assert_eq!(back.parent.as_deref(), Some("room_a"));
    assert_eq!(
        back.overrides.field_state("visible"),
        Some(OverrideState::Set),
        "覆盖态往返后仍是 Set"
    );
    assert_eq!(serde_json::to_string(&back).expect("二次序列化"), text);
}

/// 判据③：空 `overrides` 不落盘（`skip_serializing_if`）。
#[test]
fn empty_overrides_key_is_absent() {
    let instance = PrefabInstance::new("crate_1", "prop.crate");
    let value = serde_json::to_value(&instance).expect("实例应可序列化");
    let keys: Vec<&str> = value
        .as_object()
        .expect("实例是 JSON 对象")
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(keys, vec!["name", "template"], "无覆盖时整段键都不该出现");
    assert!(
        !instance.has_overrides(),
        "无覆盖时 has_overrides 必须为假（E02 靠它判断要不要写覆盖段）"
    );
}

/// 判据④：未知字段名 → 反序列化**报错**，不静默丢弃。
#[test]
fn unknown_fields_are_rejected() {
    let bad_template = json!({
        "id": "prop.crate",
        "proto": { "name": "crate" },
        "colour": "#fff"
    });
    assert!(
        serde_json::from_value::<PrefabTemplate>(bad_template).is_err(),
        "模板根上拼错键名必须报错（模板是长期资产，静默丢字段等于把错误延后）"
    );

    let bad_override = json!({ "name": "alias" });
    assert!(
        serde_json::from_value::<ObjectOverride>(bad_override).is_err(),
        "覆盖里写 name 必须报错：实例名由实例自己给，不是可覆盖字段"
    );

    let bad_instance = json!({ "name": "crate_1", "template": "prop.crate", "proto": {} });
    assert!(
        serde_json::from_value::<PrefabInstance>(bad_instance).is_err(),
        "实例根不接受 proto：模板本体不进场景文件"
    );
}

// ---------------------------------------------------------------------------
// 三态外形（判据 ⑤⑥⑦⑧⑨）
// ---------------------------------------------------------------------------

/// 判据⑤：「未覆盖」序列化后**键缺失**。
#[test]
fn unset_override_serializes_as_missing_key() {
    let overrides = ObjectOverride {
        visible: None,
        scale: None,
        ..ObjectOverride::default()
    };
    let value = serde_json::to_value(&overrides).expect("覆盖应可序列化");

    assert_eq!(value, json!({}), "三态里的「继承」在 JSON 上就是键缺失");
    assert_eq!(overrides.count(), 0);
    assert_eq!(
        overrides.field_state("visible"),
        Some(OverrideState::Inherit)
    );
}

/// 判据⑥：`Set(v)` 落 `v`。
#[test]
fn set_override_serializes_as_value() {
    let overrides = ObjectOverride {
        visible: Some(FieldOverride::set(false)),
        scale: Some(FieldOverride::set([2.0, 2.0, 2.0])),
        tags: Some(FieldOverride::set(vec!["loot".to_string()])),
        ..ObjectOverride::default()
    };
    let value = serde_json::to_value(&overrides).expect("覆盖应可序列化");

    assert_eq!(
        value,
        json!({"visible": false, "scale": [2.0, 2.0, 2.0], "tags": ["loot"]}),
        "Set 直接落值，不套一层壳"
    );
    assert_eq!(overrides.count(), 3, "三个字段都有覆盖");
}

/// 判据⑦：`Clear` 落 `null`。
#[test]
fn clear_override_serializes_as_null() {
    let overrides = ObjectOverride {
        material: Some(FieldOverride::clear()),
        visible: Some(FieldOverride::clear()),
        ..ObjectOverride::default()
    };
    let value = serde_json::to_value(&overrides).expect("覆盖应可序列化");

    assert!(
        value.get("material").is_some_and(Value::is_null),
        "Clear 必须落成 null，不能整键消失——否则「清空」动作往返后就蒸发了"
    );
    assert!(value.get("visible").is_some_and(Value::is_null));
    assert_eq!(overrides.count(), 2, "Clear 也算一次覆盖");
}

/// 判据⑧：`null` 反序列化为 `Set(Clear)` 而非 `None`（本卡关键分歧点）。
#[test]
fn null_deserializes_to_clear_not_inherit() {
    let overrides: ObjectOverride =
        serde_json::from_value(json!({"material": null})).expect("null 应能读成 Clear");

    assert_eq!(
        overrides.field_state("material"),
        Some(OverrideState::Clear),
        "键存在且为 null = 显式清空；被 serde 的 Option 读成 None 就丢语义了"
    );
    assert!(
        overrides.material.is_some(),
        "Clear 在结构体里必须是 Some(Clear)，不是 None"
    );
    assert_eq!(
        overrides.count(),
        1,
        "Clear 也是覆盖：不计数就会在 is_empty 上被当成「没有覆盖」而整段丢弃"
    );
    // 往返稳定：读进来再写出去仍是 null。
    assert_eq!(
        serde_json::to_value(&overrides).expect("再序列化"),
        json!({"material": null})
    );
}

/// 判据⑨：键缺失反序列化为 `None`（继承）。
#[test]
fn missing_key_deserializes_to_none() {
    let overrides: ObjectOverride =
        serde_json::from_value(json!({"visible": true})).expect("缺键应走默认");

    assert!(overrides.material.is_none(), "未出现的键是 None 而非 Clear");
    assert_eq!(
        overrides.field_state("material"),
        Some(OverrideState::Inherit),
        "缺键 = 继承模板值"
    );
    assert_eq!(overrides.field_state("visible"), Some(OverrideState::Set));
    assert_eq!(overrides.count(), 1);
}

// ---------------------------------------------------------------------------
// PrefabId 形状（判据 ⑩⑪⑫）
// ---------------------------------------------------------------------------

/// 判据⑩：合法 ID 通过（`prop.crate`，以及 3~4 段的合法形态）。
#[test]
fn legal_prefab_ids_are_accepted() {
    for id in ["prop.crate", "prop.crate.big", "env.tile.01", "a.b.c.d"] {
        assert!(
            validate_prefab_id(id).is_ok(),
            "合法 ID `{id}` 不该被拒：{:?}",
            validate_prefab_id(id)
        );
    }
    assert_eq!(
        prefab_rel_path("prop.crate").as_deref(),
        Some("prefab/prop/crate.json"),
        "ID 决定落盘路径"
    );
    assert!(prefab_rel_path("Prop.Crate").is_none(), "非法 ID 不给路径");
}

/// 判据⑪：含大写 / 下划线的 ID 被拒（`Prop_Crate`）。
#[test]
fn uppercase_or_underscore_ids_are_rejected() {
    for id in ["Prop.Crate", "prop.Crate", "prop_crate.x", "prop.c rate"] {
        match validate_prefab_id(id) {
            Err(PrefabIdError::BadChar { .. }) => {}
            other => panic!("`{id}` 应因非法字符被拒，实际 {other:?}"),
        }
    }
}

/// 判据⑫：空段被拒（`prop..crate`），首尾点与段数同样收口。
#[test]
fn empty_segment_ids_are_rejected() {
    assert!(matches!(
        validate_prefab_id("prop..crate"),
        Err(PrefabIdError::EmptySegment { .. })
    ));
    assert!(matches!(
        validate_prefab_id(".crate"),
        Err(PrefabIdError::EmptySegment { .. })
    ));
    assert!(matches!(
        validate_prefab_id("crate"),
        Err(PrefabIdError::SegmentsOutOfRange { .. })
    ));
    assert!(
        matches!(validate_prefab_id("  "), Err(PrefabIdError::Empty)),
        "纯空白按空处理"
    );
}

// ---------------------------------------------------------------------------
// ObjectOverride 机械化（判据 ⑬⑭⑮）
// ---------------------------------------------------------------------------

/// 判据⑬：全部可覆盖字段初始为「继承」，`is_empty()` 为真。
#[test]
fn every_field_starts_inherited() {
    let overrides = ObjectOverride::default();

    assert!(overrides.is_empty(), "新覆盖段必须是空的");
    assert_eq!(overrides.count(), 0);
    for field in ObjectOverride::FIELD_NAMES {
        assert_eq!(
            overrides.field_state(field),
            Some(OverrideState::Inherit),
            "字段 `{field}` 初始应为继承"
        );
    }
}

/// 判据⑭：宏生成的「字段名 ↔ 访问器」映射覆盖全部可覆盖字段且无重复。
#[test]
fn field_names_cover_object_desc_minus_name() {
    let names = ObjectOverride::FIELD_NAMES;
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "字段名不许重复");

    // 与 schema 里 ObjectDesc 的键对齐：去掉 name 后必须完全相等。
    let schema = prefab_schema();
    let proto_keys: std::collections::BTreeSet<String> = schema["$defs"]["ObjectDesc"]
        ["properties"]
        .as_object()
        .expect("schema 应内嵌 ObjectDesc")
        .keys()
        .cloned()
        .collect();
    assert!(proto_keys.contains("name"), "ObjectDesc 的键里有 name");
    let expected: std::collections::BTreeSet<String> = proto_keys
        .iter()
        .filter(|k| *k != "name")
        .cloned()
        .collect();
    let actual: std::collections::BTreeSet<String> = names.iter().map(|n| n.to_string()).collect();

    assert_eq!(
        actual, expected,
        "可覆盖字段 = ObjectDesc 字段去掉 name；两边不一致说明宏清单漏改"
    );
    assert_eq!(names.len(), 12, "13 个字段去掉 name 应为 12");
    assert!(
        names.contains(&"materialRef"),
        "驼峰键名必须原样保留（E02 直接把它写进场景文件）"
    );
}

/// 判据⑮：任一字段 `Set` 或 `Clear` 都让 `is_empty()` 变假。
#[test]
fn any_override_makes_is_empty_false() {
    for field in ObjectOverride::FIELD_NAMES {
        // Clear 走的路径：`null` 反序列化后必须被计为覆盖。
        let cleared: ObjectOverride = serde_json::from_value(json!({ *field: null }))
            .unwrap_or_else(|err| panic!("字段 `{field}` 的 null 读不进来：{err}"));
        assert!(
            !cleared.is_empty(),
            "字段 `{field}` 被 Clear 后 is_empty 仍为真——清空动作会丢"
        );
        assert_eq!(cleared.count(), 1, "字段 `{field}` 应只计一次覆盖");
    }

    let set_one = ObjectOverride {
        visible: Some(FieldOverride::set(true)),
        ..ObjectOverride::default()
    };
    assert!(!set_one.is_empty(), "Set 也必须让 is_empty 变假");
    assert_eq!(set_one.count(), 1);
}

// ---------------------------------------------------------------------------
// schema（判据 ⑯⑰⑱）
// ---------------------------------------------------------------------------

/// 判据⑯：`assets/prefab.schema.json` 逐字节防漂移（重生成必须零差异）。
#[test]
fn schema_file_matches_derivation() {
    let path = repo_root().join(PREFAB_SCHEMA_PATH);
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读不到 {}（{err}）", path.display()));

    assert_eq!(
        on_disk,
        prefab_schema_pretty(),
        "磁盘上的 schema 与派生结果不一致——改了模板结构请重跑 \
         `cargo run -p r3e-core --bin prefab_schema_dump`"
    );
    assert_eq!(
        prefab_root_properties(),
        vec![
            "id".to_string(),
            "proto".to_string(),
            "schema".to_string(),
            "tags".to_string()
        ],
        "根级字段集合是契约的一部分"
    );
    assert_eq!(
        prefab_schema()["required"],
        json!(["id", "proto"]),
        "只有 id 与 proto 必填"
    );
    assert_eq!(
        prefab_schema()["additionalProperties"],
        json!(false),
        "根级必须是封闭对象"
    );
}

/// 判据⑰：`assets/prefab/**/*.json` 样例全部通过 schema 校验。
#[test]
fn sample_templates_validate_against_schema() {
    let dir = repo_root().join("assets").join("prefab");
    let mut files = Vec::new();
    collect_json(&dir, &mut files);
    assert!(
        !files.is_empty(),
        "{} 下应有样例模板（至少 `prop/crate.json`）",
        dir.display()
    );

    let schema = prefab_schema();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("样例模板应可读");
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|err| panic!("{} 不是合法 JSON（{err}）", file.display()));
        if let Err(err) = validate(&value, &schema, &schema, "$") {
            panic!("{} 未通过 schema 校验：{err}", file.display());
        }
        // 光有 schema 不算过：还要能被本 crate 的类型读进来（两套口径必须同步）。
        let parsed: PrefabTemplate = serde_json::from_value(value)
            .unwrap_or_else(|err| panic!("{} 读不成 PrefabTemplate（{err}）", file.display()));
        assert!(
            parsed.validate().is_ok(),
            "{} 的 ID 形状不合法",
            file.display()
        );
    }
}

/// 判据⑱：`schema` 字段版本与文件头（本 crate 常量）一致。
#[test]
fn schema_version_matches_crate_constant() {
    assert_eq!(PREFAB_SCHEMA_VERSION, 1, "E01 冻结的模板格式版本是 1");
    assert_eq!(
        prefab_schema()["properties"]["schema"]["default"],
        json!(PREFAB_SCHEMA_VERSION),
        "schema 里登记的默认版本必须与常量同源"
    );

    let path = repo_root()
        .join("assets")
        .join("prefab")
        .join("prop")
        .join("crate.json");
    let text = std::fs::read_to_string(&path).expect("样例模板应可读");
    let sample: PrefabTemplate = serde_json::from_str(&text).expect("样例应能读成模板");
    assert_eq!(sample.schema, PREFAB_SCHEMA_VERSION);
    assert!(
        sample.version_is_current(),
        "样例必须是最新版本，否则等于用老格式冻结契约"
    );

    // 省略 `schema` 时按当前版本解释（旧文件不必补字段）。
    let legacy: PrefabTemplate =
        serde_json::from_value(json!({"id": "prop.crate", "proto": {"name": "crate"}}))
            .expect("缺 schema 应走默认值");
    assert_eq!(legacy.schema, PREFAB_SCHEMA_VERSION);
}

// ---------------------------------------------------------------------------
// 测试内的极简 schema 校验器（只覆盖 schemars 1.x 实际产出的关键字子集）
// ---------------------------------------------------------------------------

fn collect_json(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
}

fn validate(value: &Value, schema: &Value, root: &Value, at: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.trim_start_matches("#/$defs/");
        let target = root
            .get("$defs")
            .and_then(|defs| defs.get(name))
            .ok_or_else(|| format!("{at}: 解析不到 $ref {reference}"))?;
        return validate(value, target, root, at);
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        let ok = any_of
            .iter()
            .any(|branch| validate(value, branch, root, at).is_ok());
        if !ok {
            return Err(format!("{at}: 不满足 anyOf 的任何一支"));
        }
    }
    if let Some(expected) = schema.get("type") {
        let types: Vec<&str> = match expected {
            Value::String(s) => vec![s.as_str()],
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|t| matches_type(value, t)) {
            return Err(format!("{at}: 类型应为 {types:?}"));
        }
    }
    if let (Some(required), Some(object)) = (
        schema.get("required").and_then(Value::as_array),
        value.as_object(),
    ) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(format!("{at}: 缺少必填键 `{key}`"));
            }
        }
    }
    if let Some(object) = value.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        for (key, child) in object {
            match props.and_then(|p| p.get(key)) {
                Some(child_schema) => {
                    validate(child, child_schema, root, &format!("{at}.{key}"))?;
                }
                None => {
                    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                        return Err(format!("{at}: 不允许的键 `{key}`"));
                    }
                }
            }
        }
        if let Some(min) = schema.get("minProperties").and_then(Value::as_u64) {
            if (object.len() as u64) < min {
                return Err(format!("{at}: 键数少于 {min}"));
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
            if (array.len() as u64) < min {
                return Err(format!("{at}: 数组元素少于 {min}"));
            }
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
            if (array.len() as u64) > max {
                return Err(format!("{at}: 数组元素多于 {max}"));
            }
        }
        for (index, item) in array.iter().enumerate() {
            validate(item, items, root, &format!("{at}[{index}]"))?;
        }
    }
    Ok(())
}

fn matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}
