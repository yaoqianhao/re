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
/// 申请的能力：只读宿主文件系统。
///
/// P0-3 的写侧工具**不申请 `fs.write`**：`scene_patch` / `node_spawn` /
/// `script_write` / `scene_save` 全部把调用转给运行中的引擎进程，落盘由引擎执行
/// （受 `r3e --scene <path>` 与 `scene.save` 的路径约束）。宿主授权层面它们靠
/// `risk: "write"` 触发一次人工确认，而不是靠文件系统能力扩权——
/// 「插件能改磁盘」和「插件能让引擎改场景」是两件不同的事，不能混为一谈。
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

/// 写侧工具共用的引擎连接描述（P0-3）。
///
/// 写工具不碰宿主磁盘，只把调用转给**运行中的引擎**（JSON-RPC over TCP），
/// 所以连接信息走工具参数而不是插件能力声明：插件的 `fs.read` 只用于读场景文件。
fn endpoint_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "addr": {
                "type": "string",
                "description": "引擎 RPC 地址（host:port）；缺省读环境变量 R3E_RPC_ADDR，再缺省 127.0.0.1:8787"
            },
            "port": {
                "type": "integer",
                "minimum": 1,
                "maximum": 65535,
                "description": "只给端口时按 127.0.0.1:<port> 拼地址"
            },
            "token": {
                "type": "string",
                "description": "引擎以 --rpc-token 启动时的握手令牌；缺省读环境变量 R3E_RPC_TOKEN"
            }
        },
        "additionalProperties": false
    })
}

fn vec_n_schema(description: &str, n: usize) -> Value {
    json!({
        "type": "array",
        "items": { "type": "number" },
        "minItems": n,
        "maxItems": n,
        "description": description,
    })
}

fn patch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "node": {
                "type": "string",
                "description": "目标物件名（`scene_inspect` 回包里的 name；同名物件在加载阶段已被拒绝）"
            },
            "position": vec_n_schema("世界坐标位置 [x, y, z]", 3),
            "rotation": vec_n_schema("四元数 [x, y, z, w]（会由引擎归一化）", 4),
            "scale": vec_n_schema("缩放 [x, y, z]", 3),
            "material": {
                "type": "object",
                "properties": {
                    "metallic": { "type": "number", "minimum": 0, "maximum": 1, "description": "金属度 0..1" },
                    "roughness": { "type": "number", "minimum": 0, "maximum": 1, "description": "粗糙度 0..1" },
                    "emissive": {
                        "type": "array",
                        "items": { "type": "number", "minimum": 0 },
                        "minItems": 3,
                        "maxItems": 3,
                        "description": "自发光线性 RGB [r, g, b]"
                    }
                },
                "additionalProperties": false,
                "description": "只改给了的字段；未知字段直接报错，不静默忽略"
            },
            "endpoint": endpoint_schema()
        },
        "required": ["node"],
        "additionalProperties": false
    })
}

fn spawn_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "object": {
                "type": "object",
                "description": "物件描述，至少含 name；可选 mesh（cube/sphere/plane/cylinder 或 {asset, mesh}）、position、rotation、scale、color、material、physics",
                "properties": {
                    "name": { "type": "string", "description": "物件名，后续所有写工具按它定位" }
                },
                "required": ["name"]
            },
            "endpoint": endpoint_schema()
        },
        "required": ["object"],
        "additionalProperties": false
    })
}

fn script_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source": {
                "type": "string",
                "description": "rhai 脚本源码，交给引擎 script.eval 立即执行；脚本对场景的写入即时生效，可用 scene_patch / scene_save 复核"
            },
            "endpoint": endpoint_schema()
        },
        "required": ["source"],
        "additionalProperties": false
    })
}

fn save_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "存盘路径（工作区相对或绝对）；省略则由引擎存回当前场景文件"
            },
            "endpoint": endpoint_schema()
        },
        "additionalProperties": false
    })
}

/// P0-4 只读反馈：`engine_stats` 不需要任何业务参数，只可能给连接信息。
///
/// 刻意不提供「取样帧数」这类旋钮：帧时间窗口由引擎主循环维护（固定容量环形
/// 缓冲），插件再传一个帧数只会让调用方以为能改采样密度，实际上做不到。
fn stats_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "endpoint": endpoint_schema()
        },
        "additionalProperties": false
    })
}

/// P0-4 只读反馈：`capture_frame` 只多一个落盘路径。
fn capture_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "minLength": 1,
                "description": "PNG 落盘路径（绝对路径，或相对引擎进程当前目录）；省略则写到场景旁的 captures/capture_<毫秒时间戳>.png。后缀必须是 .png：引擎写的是 PNG 字节，换后缀只会得到名不副实的文件"
            },
            "endpoint": endpoint_schema()
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
        description: "对场景做几何与结构校验：逐物件跑 AIR 网格校验（退化三角形、法线、权重、AABB 边界等）、检查重名、统计无法解析的外部资产。每条问题都带结构化定位（node / objectIndex / field 字段路径 / assetPath 资源路径 / mesh.anchors 顶点·索引·分区锚点），可直接拿去定位到具体物件与网格位置再改。",
        risk: "read",
        parameters: scene_schema,
    },
    ToolSpec {
        name: "physics_simulate",
        description: "用引擎物理后端对场景做纯头less 步进（不建窗口、不渲染），返回每个刚体的起止位置、位移、物理事件与后端能力。适合验证碰撞/重力配置。",
        risk: "read",
        parameters: simulate_schema,
    },
    // —— 写侧工具（P0-3）：risk = write，宿主会走授权流程；通道复用引擎已有写 RPC ——
    ToolSpec {
        name: "scene_patch",
        description: "【写】补丁式修改运行中场景的一个已有物件：位置/旋转/缩放与材质（metallic/roughness/emissive）只改给了的字段，改完回读一次快照。需要引擎以 --rpc-port 启动。",
        risk: "write",
        parameters: patch_schema,
    },
    ToolSpec {
        name: "node_spawn",
        description: "【写】往运行中场景里新建一个物件（复用引擎 scene.spawn）：至少给 object.name，可选网格、变换、颜色、材质、物理。返回引擎给的物件摘要。",
        risk: "write",
        parameters: spawn_schema,
    },
    ToolSpec {
        name: "script_write",
        description: "【写】把一段 rhai 脚本交给运行中引擎立即执行（引擎方法 script.eval），脚本对场景的改动即时生效；用于批量改动或调用 scene_patch 覆盖不到的脚本 API。",
        risk: "write",
        parameters: script_schema,
    },
    ToolSpec {
        name: "scene_save",
        description: "【写】把运行中的场景存回磁盘（复用引擎 scene.save）：省略 path 时存回当前场景文件，给了 path 则另存。存盘落点由引擎进程决定，插件不直接写宿主文件。",
        risk: "write",
        parameters: save_schema,
    },
    // —— 反馈工具（P0-4）：risk = read，只读引擎的渲染统计与离屏截图 ——
    ToolSpec {
        name: "engine_stats",
        description: "读取运行中引擎的「数字画像」：帧时间分位（samples/last/p50/p95/p99/max）、场景规模（物件数/可见数/网格种类/材质覆盖/物理刚体）、渲染统计（draw_calls、instances、culled、shadow_draws、shadow_cascades、ssao 开关与 passes、后处理、局部光源）与画布尺寸。改完场景用它核对 draw_calls / instances 是否按预期变化，比肉眼看画面更可靠。渲染器缺席时 render 为 null（renderer:false），不会拿 0 冒充正常。需要引擎以 --rpc-port 启动。",
        risk: "read",
        parameters: stats_schema,
    },
    ToolSpec {
        name: "capture_frame",
        description: "让运行中的引擎把当前一帧渲染到离屏纹理并落成 PNG，返回图片绝对路径、宽高与字节数，可直接拿去给人看「现在长什么样」。省略 path 时写到场景旁的 captures/capture_<时间戳>.png。路径由引擎进程写盘，插件本身不碰宿主磁盘。需要引擎以 --rpc-port 启动且窗口 / GPU 已就绪。",
        risk: "read",
        parameters: capture_schema,
    },
];

/// 命令表：只给用户从命令面板触发，模型看不到。
pub const COMMANDS: &[CommandSpec] = &[CommandSpec {
    name: "engine_diagnostics",
    title: "rust3d-engine：引擎诊断",
    description:
        "输出引擎信息 + 当前工作区读取能力是否可用，用于排查插件挂载后「读不到场景文件」这类问题。",
    risk: "read",
}];

/// 工具名列表（握手与清单一致性校验、自检脚本共用）。
pub fn tool_names() -> Vec<String> {
    TOOLS.iter().map(|tool| tool.name.to_string()).collect()
}

/// 命令名列表。
pub fn command_names() -> Vec<String> {
    COMMANDS
        .iter()
        .map(|command| command.name.to_string())
        .collect()
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
