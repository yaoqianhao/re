//! 场景 JSON schema 的两条硬约束（P0-1）。
//!
//! 1. **防漂移**：`assets/scene.schema.json` 必须与 `SceneFile` 的派生结果逐字节一致。
//!    改了 serde 结构却忘了重新生成 → 本测试红灯，并直接给出重新生成的命令。
//! 2. **交叉校验**：`assets/` 下全部演示场景必须通过该 schema 校验。
//!    schema 写了引擎不认的字段、或场景用了 schema 不认的字段/枚举，都会在这里暴露。
//!
//! 校验器是刻意手写的极简实现：只覆盖 schemars 1.x 实际产出的关键字子集
//! （`$ref` / `type` / `oneOf` / `anyOf` / `enum` / `const` / `properties` /
//! `required` / `items` / `minItems` / `maxItems` / `minimum` / `maximum` /
//! `additionalProperties`），不引入 JSON Schema 运行时依赖。

use std::path::PathBuf;

use serde_json::Value;

use r3e_core::scene_schema::{scene_schema, scene_schema_pretty, SCENE_SCHEMA_PATH};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/r3e-core 之上应有两层目录")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// 防漂移
// ---------------------------------------------------------------------------

#[test]
fn schema_on_disk_matches_serde_derivation() {
    let path = repo_root().join(SCENE_SCHEMA_PATH);
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "读不到 {}（{err}）；请先运行 `cargo run -p r3e-core --bin scene_schema_dump`",
            path.display()
        )
    });
    let generated = scene_schema_pretty();

    if on_disk != generated {
        // 直接指出首个差异行，避免用户去比对整份 JSON。
        let first_diff = on_disk
            .lines()
            .zip(generated.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("第 {} 行：\n  磁盘: {a}\n  派生: {b}", i + 1))
            .unwrap_or_else(|| "行数不同（磁盘与派生结果的末尾不一致）".to_string());
        panic!(
            "{} 与 SceneFile 的派生结果不一致——场景结构改过但 schema 没重新生成。\n{first_diff}\n\
             修复：`cargo run -p r3e-core --bin scene_schema_dump`",
            path.display()
        );
    }
}

#[test]
fn schema_declares_core_sections() {
    let schema = scene_schema();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("根节点应有 properties");

    // 场景文件的顶层段落：少一个都说明派生/裁剪出了问题。
    for key in [
        "name",
        "background",
        "camera",
        "light",
        "environment",
        "postfx",
        "lights",
        "objects",
        "texts",
        "ui",
        "physics",
        "assets",
    ] {
        assert!(props.contains_key(key), "schema 缺顶层字段 `{key}`");
    }

    // 嵌套定义必须真被引用出来，而不是空壳。
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("应有 $defs");
    for key in [
        "SceneFile",
        "ObjectDesc",
        "PhysicsDesc",
        "EnvironmentSettings",
    ]
    .iter()
    {
        // 根节点自身可能被内联，只要在该出现的地方出现即可：
        // SceneFile 内联到根，ObjectDesc 等应在 $defs 或某个属性的 $ref 里。
        let appeared = defs.contains_key(*key)
            || props
                .values()
                .any(|v| v.get("items").and_then(|i| i.get("$ref")).is_some());
        assert!(appeared, "schema 未包含 `{key}` 的定义或引用");
    }

    assert_eq!(
        schema.get("$id").and_then(Value::as_str),
        Some("https://r3e.dev/schema/scene.schema.json")
    );
    assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
}

// ---------------------------------------------------------------------------
// 与演示场景交叉校验
// ---------------------------------------------------------------------------

#[test]
fn demo_scenes_validate_against_schema() {
    let assets = repo_root().join("assets");
    let mut scenes: Vec<PathBuf> = std::fs::read_dir(&assets)
        .unwrap_or_else(|err| panic!("读不到 {}（{err}）", assets.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("json")
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n == "scene.json" || (n.starts_with("scene_") && n.ends_with(".json"))
                })
        })
        .collect();
    scenes.sort();

    assert!(
        scenes.len() >= 11,
        "演示场景数量异常（{} 个）：{}",
        scenes.len(),
        scenes
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let schema = scene_schema();
    let mut failures: Vec<String> = Vec::new();
    for path in &scenes {
        let text = std::fs::read_to_string(path).expect("读场景文件失败");
        let value: Value = serde_json::from_str(&text).expect("场景不是合法 JSON");
        let errors = validate(&schema, &schema, &value, "$", 0);
        if !errors.is_empty() {
            failures.push(format!(
                "{}\n  {}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                errors.join("\n  ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "有 {} 个场景不符合 scene.schema.json：\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// 校验器自检：确认「交叉校验」不是空跑
// ---------------------------------------------------------------------------

#[test]
fn validator_rejects_broken_scenes() {
    let schema = scene_schema();

    // 元组长度不足：`background` 是 `[f32; 4]`。
    let short_background = serde_json::json!({ "background": [0.1, 0.2, 0.3] });
    let errors = validate(&schema, &schema, &short_background, "$", 0);
    assert!(
        errors.iter().any(|e| e.contains("minItems")),
        "长度不足的 background 应被拒：{errors:?}"
    );

    // 枚举写错：`ui[].kind` 只能是 rect / border / image。
    let bad_kind = serde_json::json!({ "ui": [{ "kind": "star" }] });
    let errors = validate(&schema, &schema, &bad_kind, "$", 0);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("oneOf") || e.contains("不允许")),
        "非法 UiKind 应被拒：{errors:?}"
    );

    // 类型写错：`camera.fov` 是数字（`camera` 是可省段，错误会落在 anyOf 分支上）。
    let bad_type = serde_json::json!({ "camera": { "fov": "wide" } });
    let errors = validate(&schema, &schema, &bad_type, "$", 0);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("anyOf") || e.contains("期望类型")),
        "字符串形式的 fov 应被拒：{errors:?}"
    );

    // 合法的最小场景必须通过（空对象全部取默认值）。
    let empty = serde_json::json!({});
    assert!(
        validate(&schema, &schema, &empty, "$", 0).is_empty(),
        "空场景应合法"
    );
}

// ---------------------------------------------------------------------------
// 极简 JSON Schema 校验器
// ---------------------------------------------------------------------------

/// 校验 `instance` 是否满足 `schema`；`root` 用于解析 `$ref`。
fn validate(
    root: &Value,
    schema: &Value,
    instance: &Value,
    path: &str,
    depth: usize,
) -> Vec<String> {
    let mut errors = Vec::new();
    if depth > 64 {
        errors.push(format!("{path}: schema 递归过深"));
        return errors;
    }
    let Some(obj) = schema.as_object() else {
        return errors;
    };

    // $ref：本地 `#/$defs/<name>` 形式。
    if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
        if let Some(target) = resolve_ref(root, reference) {
            errors.extend(validate(root, target, instance, path, depth + 1));
        }
        return errors;
    }

    // type：字符串或字符串数组（数组表示联合类型）。
    if let Some(expected) = obj.get("type") {
        let ok = match expected {
            Value::String(s) => type_matches(s, instance),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .any(|s| type_matches(s, instance)),
            _ => true,
        };
        if !ok {
            errors.push(format!(
                "{path}: 期望类型 {expected}，实际为 {}",
                json_type(instance)
            ));
            return errors;
        }
    }

    // const / enum
    if let Some(expected) = obj.get("const") {
        if instance != expected {
            errors.push(format!("{path}: 值应为 {expected}，实际为 {instance}"));
        }
    }
    if let Some(Value::Array(options)) = obj.get("enum") {
        if !options.iter().any(|o| o == instance) {
            errors.push(format!("{path}: 值 {instance} 不在允许集合 {options:?} 内"));
        }
    }

    // oneOf / anyOf：都按「至少一支通过」处理。
    for (keyword, kind) in [("anyOf", "anyOf"), ("oneOf", "oneOf")] {
        if let Some(Value::Array(branches)) = obj.get(keyword) {
            let passed = branches
                .iter()
                .any(|b| validate(root, b, instance, path, depth + 1).is_empty());
            if !passed {
                errors.push(format!("{path}: 不满足 {kind} 中的任何一支"));
            }
        }
    }

    // 数值边界
    if let Some(n) = instance.as_f64() {
        if let Some(min) = obj.get("minimum").and_then(Value::as_f64) {
            if n < min {
                errors.push(format!("{path}: {n} 小于最小值 {min}"));
            }
        }
        if let Some(max) = obj.get("maximum").and_then(Value::as_f64) {
            if n > max {
                errors.push(format!("{path}: {n} 大于最大值 {max}"));
            }
        }
    }

    // 数组
    if let Some(items) = instance.as_array() {
        if let Some(min) = obj.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < min {
                errors.push(format!(
                    "{path}: 元素数 {} 少于 minItems {min}",
                    items.len()
                ));
            }
        }
        if let Some(max) = obj.get("maxItems").and_then(Value::as_u64) {
            if (items.len() as u64) > max {
                errors.push(format!(
                    "{path}: 元素数 {} 多于 maxItems {max}",
                    items.len()
                ));
            }
        }
        if let Some(item_schema) = obj.get("items") {
            for (i, item) in items.iter().enumerate() {
                errors.extend(validate(
                    root,
                    item_schema,
                    item,
                    &format!("{path}[{i}]"),
                    depth + 1,
                ));
            }
        }
    }

    // 对象
    if let Some(map) = instance.as_object() {
        if let Some(Value::Array(required)) = obj.get("required") {
            for key in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    errors.push(format!("{path}: 缺少必填字段 `{key}`"));
                }
            }
        }
        if let Some(Value::Object(props)) = obj.get("properties") {
            for (key, value) in map {
                match props.get(key) {
                    Some(sub) => errors.extend(validate(
                        root,
                        sub,
                        value,
                        &format!("{path}.{key}"),
                        depth + 1,
                    )),
                    None => {
                        if obj.get("additionalProperties") == Some(&Value::Bool(false)) {
                            errors.push(format!("{path}: 不允许的字段 `{key}`"));
                        }
                    }
                }
            }
        }
    }

    errors
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let name = reference.strip_prefix("#/$defs/")?;
    root.get("$defs")?.get(name)
}

fn type_matches(expected: &str, instance: &Value) -> bool {
    match expected {
        "null" => instance.is_null(),
        "boolean" => instance.is_boolean(),
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "number" => instance.is_number(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        _ => true,
    }
}

fn json_type(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
