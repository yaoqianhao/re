//! 消息层：请求 / 响应 / 错误 / 引擎元信息。
//!
//! 全部结构只由 `serde` 派生，字段与线上 JSON 一一对应；这一层不允许出现任何
//! 引擎类型，否则整个 crate 的「零依赖」前提就失效了。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ENGINE_ERROR, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JSONRPC, METHOD_NOT_FOUND,
};

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
