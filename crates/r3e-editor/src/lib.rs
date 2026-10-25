//! r3e-editor —— 编辑器桥（phase 1）。
//!
//! 职责只有两件事：
//!
//! 1. **协议**：JSON-RPC 2.0 的请求 / 响应 / 错误码 / 方法表，以及 `EngineApi` 抽象；
//! 2. **传输**：`127.0.0.1:<port>` 上的 TCP + NDJSON，读写各在自己的线程里，
//!    **引擎状态只由主线程碰**（见 [`server::Job`]）。
//!
//! # 为什么本 crate 不依赖 `r3e-core`
//!
//! 协议层只认 JSON 值，不认 `NodeId` / `Vec3`。这样编辑器桥与引擎内核之间是
//! 单向依赖（宿主 `r3e-app` 实现 [`EngineApi`]），换内部类型不必改协议，
//! 也不会出现 `core -> editor` 的反向耦合。
//!
//! # 线程模型
//!
//! ```text
//! 编辑器(客户端)  <-- TCP -->  连接线程（读行 / 写行）
//!                                  |  Job{request, reply}
//!                                  v
//!                            mpsc 队列（只在主线程 drain）
//!                                  |
//!                            主线程 dispatch(EngineApi)  --> reply
//! ```
//!
//! 主线程每帧只取有限条（默认 8），保证一帧的 RPC 开销有上界；RPC 线程从不
//! 触碰 `Scene` / `Physics` / 脚本引擎。

pub mod server;

pub use server::{Job, RpcServer};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// JSON-RPC 版本字符串。
pub const JSONRPC: &str = "2.0";
/// 桥协议版本（方法表或字段语义不兼容地变更时 +1）。
pub const PROTOCOL_VERSION: u32 = 1;

// JSON-RPC 2.0 标准错误码
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// 引擎侧业务错误（例如「找不到物件」）。
pub const ENGINE_ERROR: i32 = -32000;
/// 鉴权失败（配置了 `--rpc-token` 但请求没带或带错）。
pub const UNAUTHORIZED: i32 = -32001;
/// 桥过载：待处理队列已满，或并发连接数达到上限。客户端应稍后重试。
pub const SERVER_BUSY: i32 = -32002;
/// 主线程在超时时间内没有回包（引擎卡顿或主循环已停）。
pub const RPC_TIMEOUT: i32 = -32003;

/// 请求：`{"jsonrpc":"2.0","id":1,"method":"scene.list","params":{...}}`
///
/// `params` 缺省为空对象；`id` 缺省表示通知（服务器不回包）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "default_version")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

fn default_version() -> String {
    JSONRPC.to_string()
}

impl Request {
    /// 从 JSON 值构造并做最小校验（版本号 + 方法名非空）。
    pub fn from_value(value: Value) -> Result<Self, RpcFailure> {
        let req: Request = serde_json::from_value(value)
            .map_err(|e| RpcFailure::new(INVALID_REQUEST, format!("请求结构不合法: {e}")))?;
        if req.jsonrpc != JSONRPC {
            return Err(RpcFailure::new(
                INVALID_REQUEST,
                format!("jsonrpc 字段必须是 `{JSONRPC}`，收到 `{}`", req.jsonrpc),
            ));
        }
        if req.method.trim().is_empty() {
            return Err(RpcFailure::new(INVALID_REQUEST, "method 不能为空"));
        }
        Ok(req)
    }

    /// 是否通知（无 `id`，按 JSON-RPC 规范不回包）。
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 响应：成功给 `result`，失败给 `error`，二者互斥。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn fail(id: Option<Value>, failure: RpcFailure) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: None,
            error: Some(RpcError::from(failure)),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// 序列化成单行（NDJSON 不允许裸换行）。
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"{JSONRPC}","id":null,"error":{{"code":{INTERNAL_ERROR},"message":"响应序列化失败: {e}"}}}}"#
            )
        })
    }
}

/// 错误对象（线上表示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 引擎侧失败原因，`EngineApi` 用它报错，再被转成 [`RpcError`]。
#[derive(Debug, Clone, PartialEq)]
pub struct RpcFailure {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcFailure {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// 参数缺失 / 类型不对。
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    /// 物件 / 资源不存在。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ENGINE_ERROR, message)
    }

    /// 引擎执行失败（脚本报错、spawn 失败等）。
    pub fn engine(message: impl Into<String>) -> Self {
        Self::new(ENGINE_ERROR, message)
    }

    /// 本期未实现的方法。
    pub fn unsupported(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("方法 `{method}` 未实现"))
    }
}

impl From<RpcFailure> for RpcError {
    fn from(f: RpcFailure) -> Self {
        Self {
            code: f.code,
            message: f.message,
            data: f.data,
        }
    }
}

/// 引擎信息快照（`engine.info` 的 `result`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub scene: String,
    pub script: String,
    pub frames: u64,
    pub fps: f32,
    pub uptime_ms: u64,
    pub paused: bool,
    pub nodes: usize,
    pub physics: PhysicsInfo,
}

/// 物理子系统状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicsInfo {
    pub enabled: bool,
    pub backend: String,
    pub capabilities: Vec<String>,
    pub bodies: usize,
    pub ccd_default: bool,
}

/// `scene.list` 的单条结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeBrief {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    pub position: [f32; 3],
    pub visible: bool,
}

/// `scene.set_transform` 的局部改动：只给要改的字段，其余保持原值。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransformPatch {
    pub position: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
}

impl TransformPatch {
    pub fn is_empty(&self) -> bool {
        self.position.is_none() && self.rotation.is_none() && self.scale.is_none()
    }
}

/// 宿主（`r3e-app`）要实现的引擎操作集合。
///
/// 全部方法都在**主线程**调用；实现方可以直接借用 `Scene` / `Physics` / 脚本宿主。
/// 缺省实现返回「未实现」，这样宿主可以只点亮本期需要的方法。
pub trait EngineApi {
    fn info(&mut self) -> EngineInfo;

    fn scene_list(
        &mut self,
        _filter: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<NodeBrief>, RpcFailure> {
        Err(RpcFailure::unsupported("scene.list"))
    }

    fn scene_get(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.get"))
    }

    fn scene_set_transform(
        &mut self,
        _node: &str,
        _patch: TransformPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_transform"))
    }

    fn scene_spawn(&mut self, _object: Value) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.spawn"))
    }

    fn scene_remove(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.remove"))
    }

    fn script_eval(&mut self, _source: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.eval"))
    }

    fn script_reload(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.reload"))
    }
}

/// 方法名归一化：忽略大小写与下划线，`scene.setTransform` 与 `scene.set_transform` 等价。
fn canonical(method: &str) -> String {
    method
        .chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn param<'a>(params: &'a Value, key: &str) -> Option<&'a Value> {
    params.get(key)
}

fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcFailure> {
    param(params, key)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcFailure::invalid_params(format!("缺少字符串参数 `{key}`")))
}

fn opt_f32_array<const N: usize>(
    params: &Value,
    key: &str,
) -> Result<Option<[f32; N]>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    let arr = value.as_array().ok_or_else(|| {
        RpcFailure::invalid_params(format!("参数 `{key}` 应为 {N} 个数字组成的数组"))
    })?;
    if arr.len() != N {
        return Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 需要 {N} 个数字，收到 {} 个",
            arr.len()
        )));
    }
    let mut out = [0.0f32; N];
    for (slot, item) in out.iter_mut().zip(arr) {
        *slot = item
            .as_f64()
            .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 含非数字元素")))?
            as f32;
    }
    Ok(Some(out))
}

/// 把一条请求派发到 [`EngineApi`]，永不 panic（未知方法走 `METHOD_NOT_FOUND`）。
pub fn dispatch(api: &mut dyn EngineApi, req: &Request) -> Response {
    let id = req.id.clone();
    match canonical(&req.method).as_str() {
        "ping" => Response::ok(
            id,
            json!({ "pong": true, "protocol": PROTOCOL_VERSION, "version": env!("CARGO_PKG_VERSION") }),
        ),
        "engine.info" => match serde_json::to_value(api.info()) {
            Ok(value) => Response::ok(id, value),
            Err(e) => Response::fail(id, RpcFailure::new(INTERNAL_ERROR, e.to_string())),
        },
        "scene.list" => {
            let filter = param(&req.params, "filter").and_then(Value::as_str);
            let limit = match param(&req.params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.scene_list(filter, limit) {
                Ok(nodes) => Response::ok(id, json!({ "count": nodes.len(), "nodes": nodes })),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.get" => match with_node(&req.params, |node| api.scene_get(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.settransform" => {
            let patch = match (
                opt_f32_array::<3>(&req.params, "position"),
                opt_f32_array::<4>(&req.params, "rotation"),
                opt_f32_array::<3>(&req.params, "scale"),
            ) {
                (Ok(p), Ok(r), Ok(s)) => TransformPatch {
                    position: p,
                    rotation: r,
                    scale: s,
                },
                (Err(f), _, _) | (_, Err(f), _) | (_, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "position / rotation / scale 至少要给一个（否则这次调用没有意义）",
                    ),
                );
            }
            match with_node(&req.params, |node| api.scene_set_transform(node, patch)) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.spawn" => {
            let object = param(&req.params, "object")
                .or_else(|| param(&req.params, "desc"))
                .cloned()
                .unwrap_or(Value::Null);
            if !object.is_object() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params("缺少对象参数 `object`（物件描述）"),
                );
            }
            match api.scene_spawn(object) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.remove" => match with_node(&req.params, |node| api.scene_remove(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "script.eval" => {
            let source = match require_str(&req.params, "source")
                .or_else(|_| require_str(&req.params, "code"))
            {
                Ok(s) => s,
                Err(f) => return Response::fail(id, f),
            };
            match api.script_eval(source) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "script.reload" => match api.script_reload() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        other => Response::fail(
            id,
            RpcFailure::new(METHOD_NOT_FOUND, format!("未知方法 `{}`", req.method))
                .with_data(json!({ "normalized": other })),
        ),
    }
}

/// 取出 `node` 参数并交给闭包，统一参数缺失的错误。
fn with_node<T>(
    params: &Value,
    f: impl FnOnce(&str) -> Result<T, RpcFailure>,
) -> Result<T, RpcFailure> {
    let node = require_str(params, "node")?;
    if node.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `node` 不能为空"));
    }
    f(node)
}
