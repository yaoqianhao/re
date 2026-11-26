//! Prefab 模板 JSON 的机器可读契约（M2-E01）。
//!
//! 结构与 [`crate::prefab::PrefabTemplate`] 的 serde 定义一一对应，由
//! `schemars::JsonSchema` 派生，落盘为仓库根目录下的 `assets/prefab.schema.json`。
//! 配两条测试（见 `tests/prefab.rs`）：
//!
//! 1. **防漂移**：磁盘文件必须与 [`prefab_schema_pretty`] 的输出逐字节一致——
//!    改了模板结构却没重新生成 schema 即红灯；
//! 2. **样例过校验**：`assets/prefab/**/*.json` 必须能按本 schema 校验通过。
//!
//! 重新生成的方式：
//!
//! ```text
//! cargo run -p r3e-core --bin prefab_schema_dump
//! ```
//!
//! # 与 `scene.schema.json` 的分工
//!
//! `scene.schema.json` 描述**场景**（运行期实体的账本），本文件描述**模板**
//! （可复用原型的素材）。两者都内嵌 [`crate::ObjectDesc`] 的字段清单，所以
//! 改 `ObjectDesc` 时两份 schema 会一起漂——这正是防漂移测试要拦住的东西：
//! 每一份都得重新生成，不能只改一份。

use serde_json::{json, Value};

use crate::prefab::PrefabTemplate;

/// schema 在仓库内的相对路径（相对仓库根目录）。
pub const PREFAB_SCHEMA_PATH: &str = "assets/prefab.schema.json";

/// schema 的 `$id`：契约的稳定标识，供工具按名字引用。
pub const PREFAB_SCHEMA_ID: &str = "https://r3e.dev/schema/prefab.schema.json";

/// 从 serde 结构派生 prefab 模板 JSON Schema（未做展示层润色的 [`Value`]）。
///
/// 根节点额外补三项元信息：`$id`（稳定标识）、`title` / `description`（用途说明）。
/// 其余内容全部来自 [`PrefabTemplate`] 的派生结果，不手写、不复制文档。
pub fn prefab_schema() -> Value {
    let mut root =
        serde_json::to_value(schemars::schema_for!(PrefabTemplate)).expect("Schema 序列化不应失败");

    let obj = root.as_object_mut().expect("Schema 根节点必须是 JSON 对象");
    obj.insert("$id".to_string(), json!(PREFAB_SCHEMA_ID));
    obj.insert(
        "title".to_string(),
        json!("r3e Prefab 模板（assets/prefab/**.json）"),
    );
    obj.insert(
        "description".to_string(),
        json!(
            "r3e 引擎 Prefab 模板文件的完整契约，由 crates/r3e-core/src/prefab 的 serde 结构派生。\
             `id` 与 `proto` 必填，`schema` 省略即视为当前版本，`tags` 省略即无标签；\
             模板文件本身拒绝未知字段（拼错键名会报错）。`proto` 内层沿用场景 ObjectDesc 的\
             宽松口径（未知字段静默忽略、缺省即取引擎默认值）。"
        ),
    );
    root
}

/// 与 [`prefab_schema`] 相同，但以「稳定格式的 JSON 文本」返回，用于落盘与防漂移比对。
///
/// 末尾统一补一个换行，保证文件在各类编辑器 / git 下无「最后一行无换行」告警。
pub fn prefab_schema_pretty() -> String {
    let mut text = serde_json::to_string_pretty(&prefab_schema()).expect("Schema 序列化不应失败");
    text.push('\n');
    text
}

/// schema 里根级 `properties` 的键集合（供测试与工具做「字段是否存在」的快速断言）。
pub fn prefab_root_properties() -> Vec<String> {
    let schema = prefab_schema();
    let mut keys: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}
