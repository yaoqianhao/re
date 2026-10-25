//! 插件的**唯一真源**：清单元数据、工具/命令声明表。
//!
//! 这里刻意把「清单里的 tools/commands」和「握手回包里的 tools/commands」都从
//! 同一张表生成：ai_ide 的 `host.rs::verify()` 要求**握手声明的工具集与清单完全
//! 一致（不能多也不能少）**，两处各抄一份迟早漂移，一漂移就是 6106 装载失败。
//! 因此 `plugin.json` 也不手写，由二进制 `--emit-manifest` 现算（见 `main.rs`）。

use serde_json::{json, Value};

/// 插件 id：`^[a-z0-9][a-z0-9-]{1,63}$`。
pub const PLUGIN_ID: &str = "rust3d-engine";
/// 展示名。
pub const PLUGIN_NAME: &str = "rust3d-engine";
/// 遵循的插件 API 版本：宿主 `HOST_PLUGIN_API_MAJOR=1`，minor 不得超过宿主。
pub const PLUGIN_API: &str = "1.0";
/// 包内入口（相对包根，`/` 分隔）。打包脚本把二进制放到 `bin/` 下。
pub const MAIN_ENTRY: &str = "bin/r3e-plugin.exe";
/// 单次工具调用超时（ms）：宿主允许 1000..=120000，物理模拟留足余量。
pub const TIMEOUT_MS: u64 = 60_000;
/// 申请的能力：只读。所有工具都是纯计算 + 读场景文件，不需要写盘/联网/执行。
pub const CAPABILITIES: &[&str] = &["fs.read"];

/// 一个工具的声明。
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// 风险档位：`read` / `write`（`exec` 宿主不给，写了会被拒装）。
    pub risk: &'static str,
    /// 入参 JSON Schema 的构造函数（`json!` 不是 const，所以存函数指针）。
    pub parameters: fn() -> Value,
}

/// 一条命令（命令面板入口，不进模型工具表）。
pub struct CommandSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub risk: &'static str,
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn scene_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scene": {
                "type": ["object", "string"],
                "description": "场景：直接给 r3e 场景对象，或它的 JSON 文本（与场景文件同格式）"
            },
            "path": {
                "type": "string",
                "description": "工作区相对路径（如 assets/demo.r3e.json）。经 host/fs.read 读取，需 fs.read 授权；单文件上限 64 KB，超过请用 scene 直接传"
            }
        },
        "additionalProperties": false
    })
}

fn mesh_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "shape": {
                "type": "string",
                "enum": ["cube", "sphere", "plane", "cylinder"],
                "description": "内置图元，缺省 cube"
            },
            "segments": {
                "type": "integer",
                "minimum": 3,
                "maximum": 256,
                "description": "球/圆柱的经向分段，缺省 32"
            },
            "rings": {
                "type": "integer",
                "minimum": 2,
                "maximum": 256,
                "description": "球的纬向分段，缺省 16"
            },
            "divisions": {
                "type": "integer",
                "minimum": 1,
                "maximum": 128,
                "description": "平面的细分格数，缺省 1"
            }
        },
        "additionalProperties": false
    })
}

fn simulate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scene": {
                "type": ["object", "string"],
                "description": "场景对象或 JSON 文本；缺省时用 path 指向的文件"
            },
            "path": {
                "type": "string",
                "description": "工作区相对的场景文件路径（经 host/fs.read，需 fs.read 授权）"
            },
            "steps": {
                "type": "integer",
                "minimum": 1,
                "maximum": 2000,
                "description": "步进次数，缺省 120"
            },
            "dt": {
                "type": "number",
                "minimum": 0.001,
                "maximum": 0.1,
                "description": "单步时长（秒），缺省 1/60"
            },
            "backend": {
                "type": "string",
                "enum": ["builtin", "null", "rapier"],
                "description": "物理后端；缺省沿用场景声明，再缺省 builtin。后端未编进二进制时报错而不是悄悄降级"
            },
            "gravity": {
                "type": "array",
                "items": { "type": "number" },
                "minItems": 3,
                "maxItems": 3,
                "description": "重力向量，缺省沿用场景声明"
            },
            "ground": {
                "type": "number",
                "description": "地面平面高度，缺省沿用场景声明"
            },
            "maxBodies": {
                "type": "integer",
                "minimum": 1,
                "maximum": 64,
                "description": "回包最多列举多少个刚体，缺省 20"
            }
        },
        "additionalProperties": false
    })
}

/// 工具表：顺序即清单顺序，也即握手顺序。
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "engine_info",
        description: "返回 rust3d-engine 的版本、crate 组成、物理后端可用性、内置图元与插件协议参数。无副作用，可作为连通性自检。",
        risk: "read",
        parameters: empty_schema,
    },
    ToolSpec {
        name: "scene_inspect",
        description: "解析 r3e 场景（相机/光源/物件/物理配置），返回节点清单、网格引用、世界坐标与物理开关。用于在不启动渲染器的前提下读懂一个场景文件。",
        risk: "read",
        parameters: scene_schema,
    },
    ToolSpec {
        name: "mesh_stats",
        description: "生成内置图元（cube/sphere/plane/cylinder）网格并返回顶点数、三角形数、AABB、几何指纹与 AIR 网格校验结果。",
        risk: "read",
        parameters: mesh_schema,
    },
    ToolSpec {
        name: "scene_validate",
        description: "对场景做几何与结构校验：逐物件跑 AIR 网格校验（退化三角形、法线、权重、AABB 边界等）、检查重名、统计无法解析的外部资产。",
        risk: "read",
        parameters: scene_schema,
    },
    ToolSpec {
        name: "physics_simulate",
        description: "用引擎物理后端对场景做纯头less 步进（不建窗口、不渲染），返回每个刚体的起止位置、位移、物理事件与后端能力。适合验证碰撞/重力配置。",
        risk: "read",
        parameters: simulate_schema,
    },
];

/// 命令表：只给用户从命令面板触发，模型看不到。
pub const COMMANDS: &[CommandSpec] = &[CommandSpec {
    name: "engine_diagnostics",
    title: "rust3d-engine：引擎诊断",
    description: "输出引擎信息 + 当前工作区读取能力是否可用，用于排查插件挂载后「读不到场景文件」这类问题。",
    risk: "read",
}];

/// 工具名列表（握手与清单一致性校验、自检脚本共用）。
pub fn tool_names() -> Vec<String> {
    TOOLS.iter().map(|tool| tool.name.to_string()).collect()
}

/// 命令名列表。
pub fn command_names() -> Vec<String> {
    COMMANDS.iter().map(|command| command.name.to_string()).collect()
}

/// 生成工具的 JSON 声明（清单与握手共用同一份结构）。
pub fn tools_json() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "risk": tool.risk,
                    "parameters": (tool.parameters)(),
                })
            })
            .collect(),
    )
}

/// 生成命令的 JSON 声明。
pub fn commands_json() -> Value {
    Value::Array(
        COMMANDS
            .iter()
            .map(|command| {
                json!({
                    "name": command.name,
                    "title": command.title,
                    "description": command.description,
                    "risk": command.risk,
                })
            })
            .collect(),
    )
}

/// 生成完整的 `plugin.json`。
pub fn manifest_json() -> Value {
    json!({
        "id": PLUGIN_ID,
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "把 rust3d-engine 的纯逻辑层（场景、网格、AIR 校验、物理）挂给 ai_ide：原生子进程 + NDJSON over stdio，无需图形栈。",
        "pluginApi": PLUGIN_API,
        "main": MAIN_ENTRY,
        "capabilities": CAPABILITIES,
        "tools": tools_json(),
        "commands": commands_json(),
        "subscriptions": [],
        "timeoutMs": TIMEOUT_MS,
    })
}

/// 握手回包：`pluginApi` / `name` / `version` / `tools` / `commands` / `subscriptions`。
///
/// 宿主 `verify()` 只比对**工具名集合**与 `pluginApi` 主版本，但这里把
/// `risk` / `parameters` 一并带上：装载后工具表以握手为准，缺省才回落清单。
pub fn handshake_json() -> Value {
    json!({
        "pluginApi": { "major": 1, "minor": 0 },
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "tools": tools_json(),
        "commands": commands_json(),
        "subscriptions": [],
        "capabilities": CAPABILITIES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_name_pattern(name: &str) {
        let mut chars = name.chars();
        let first = chars.next().expect("name not empty");
        assert!(first.is_ascii_lowercase(), "{name} must start lowercase");
        assert!((2..=41).contains(&name.len()), "{name} length out of range");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name} has illegal chars"
        );
    }

    #[test]
    fn tool_and_command_names_follow_manifest_rules() {
        for tool in TOOLS {
            assert_name_pattern(tool.name);
            assert!(matches!(tool.risk, "read" | "write"));
            assert!(tool.description.chars().count() > 8, "{} 缺描述", tool.name);
            let schema = (tool.parameters)();
            assert_eq!(schema["type"], json!("object"));
        }
        for command in COMMANDS {
            assert_name_pattern(command.name);
            assert!(matches!(command.risk, "read" | "write"));
            assert!(!command.title.is_empty());
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let names = tool_names();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "工具名重复");
        // 命令与工具不能同名（宿主按名字分别路由，同名会让人误判）
        for command in command_names() {
            assert!(!names.contains(&command), "命令与工具重名：{command}");
        }
    }

    #[test]
    fn manifest_and_handshake_declare_same_tools() {
        let manifest = manifest_json();
        let handshake = handshake_json();
        assert_eq!(manifest["tools"], handshake["tools"]);
        assert_eq!(manifest["commands"], handshake["commands"]);
        assert_eq!(manifest["pluginApi"], json!(PLUGIN_API));
        assert_eq!(handshake["pluginApi"]["major"], json!(1));
        assert_eq!(manifest["main"], json!(MAIN_ENTRY));
        assert_eq!(manifest["capabilities"], json!(CAPABILITIES));
        // shell.exec 是宿主明令不提供的能力，出现在清单里等于自废安装
        assert!(!manifest["capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .any(|cap| cap == "shell.exec"));
    }

    #[test]
    fn timeout_is_within_host_range() {
        assert!((1000..=120_000).contains(&TIMEOUT_MS));
    }
}
