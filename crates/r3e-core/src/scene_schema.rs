//! 场景 JSON 的机器可读契约（P0-1）。
//!
//! 场景字段、取值范围、枚举选项、默认值此前只存在于 `docs/ARCHITECTURE.md` 的散文与
//! serde 结构里。生成式使用方（AI / 外部工具）拿不到契约就只能照抄 10 个示例场景，
//! 拼错字段名不报错、枚举写错静默回退、越界值到运行期才炸。
//!
//! 这里把 [`SceneFile`] 及其所有嵌套结构的 serde 定义直接派生成 JSON Schema
//! （`schemars::JsonSchema`），落盘为仓库根目录下的 `assets/scene.schema.json`，
//! 并配两条测试（见 `tests/scene_schema.rs`）：
//!
//! 1. **防漂移**：磁盘文件必须与 [`scene_schema_pretty`] 的输出逐字节一致，
//!    改了 serde 结构却没重新生成 schema 即红灯；
//! 2. **交叉校验**：`assets/scene*.json` 全部必须通过该 schema 校验。
//!
//! 重新生成的方式：
//!
//! ```text
//! cargo run -p r3e-core --bin scene_schema_dump
//! ```
//!
//! schema 与场景文件同目录，是因为「改场景的人先看到场景，再看到契约」；
//! 文件名固定，便于任何工具按相对路径取用。

use serde_json::{json, Map, Value};

use crate::scene::SceneFile;

/// schema 在仓库内的相对路径（相对仓库根目录）。
pub const SCENE_SCHEMA_PATH: &str = "assets/scene.schema.json";

/// schema 的 `$id`：契约的稳定标识，供工具按名字引用。
pub const SCENE_SCHEMA_ID: &str = "https://r3e.dev/schema/scene.schema.json";

/// 从 serde 结构派生场景 JSON Schema（未做展示层润色的 [`Value`]）。
///
/// 根节点额外补三项元信息：`$id`（稳定标识）、`title` / `description`（给人看的用途说明）。
/// 其余内容全部来自 `SceneFile` 的派生结果，不手写、不复制文档。
pub fn scene_schema() -> Value {
    let mut root =
        serde_json::to_value(schemars::schema_for!(SceneFile)).expect("Schema 序列化不应失败");

    let obj = root.as_object_mut().expect("Schema 根节点必须是 JSON 对象");
    obj.insert("$id".to_string(), json!(SCENE_SCHEMA_ID));
    obj.insert(
        "title".to_string(),
        json!("r3e 场景文件（assets/scene*.json）"),
    );
    obj.insert(
        "description".to_string(),
        json!(
            "r3e 引擎场景 JSON 的完整契约，由 crates/r3e-core 的 serde 结构派生。\
             所有字段均可省略（省略即取引擎默认值）；未声明的额外字段会被 serde 静默忽略，\
             因此拼错字段名不会报错——请以本 schema 的 properties 列表为准。"
        ),
    );
    root
}

/// 与 [`scene_schema`] 相同，但以「稳定格式的 JSON 文本」返回，用于落盘与防漂移比对。
///
/// 末尾统一补一个换行，保证文件在各类编辑器 / git 下无「最后一行无换行」告警。
pub fn scene_schema_pretty() -> String {
    let mut text = serde_json::to_string_pretty(&scene_schema()).expect("Schema 序列化不应失败");
    text.push('\n');
    text
}

/// schema 里根级 `properties` 的键集合（供测试与工具做「字段是否存在」的快速断言）。
pub fn scene_root_properties() -> Vec<String> {
    let schema = scene_schema();
    let mut keys: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// 把 schema 转成 `Map`，供需要直接改字段的调用方使用（例如工具层加 `examples`）。
pub fn scene_schema_object() -> Map<String, Value> {
    scene_schema()
        .as_object()
        .cloned()
        .expect("Schema 根节点必须是 JSON 对象")
}
