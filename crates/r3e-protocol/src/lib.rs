//! r3e-protocol —— 编辑器桥的**协议层**（M0-S5 从 `r3e-editor` 剥离）。
//!
//! 职责是「说什么」：JSON-RPC 2.0 的请求 / 响应 / 错误码 / 方法表、14 个 `*Patch`
//! 局部改动体、[`EngineApi`] 抽象，以及把它们串起来的 [`dispatch`]。
//!
//! **不负责**「怎么传」：TCP + NDJSON、连接线程、主线程队列都在
//! `r3e-editor::server`（`r3e-editor` 只做传输，并用 `pub use r3e_protocol::*`
//! 保持对外 API 与剥离前一致，旧调用方一行不改）。
//!
//! # 为什么本 crate 零引擎依赖
//!
//! 协议层只认 JSON 值，不认 `NodeId` / `Vec3`。`cargo tree -p r3e-protocol
//! --edges normal,dev` 只应出现 `serde` / `serde_json` —— 连 dev-dependency 也
//! 不许有引擎 crate，否则「协议层不认识引擎类型」这条不变量就是假的。宿主
//! （`r3e-app`）实现 [`EngineApi`]，换内部类型不必改协议。
//!
//! # 分层
//!
//! | 文件 | 内容 |
//! |---|---|
//! | `msg.rs` | `Request` / `Response` / `RpcError` / `RpcFailure` / 元信息 DTO |
//! | `patch.rs` | 14 个 `*Patch`（只给要改的字段，其余保持原值） |
//! | `api.rs` | `trait EngineApi`（39 方法，全部在主线程调用） |
//! | `dispatch/mod.rs` | 派发骨架 + 参数解码 + 元命令（`ping` / `engine.*` / `script.*` / `capture.*`） |
//! | `dispatch/scene.rs` | `scene.*` / `anim.*` 臂 |
//! | `dispatch/media.rs` | `ui.*` / `light.*` / `audio.*` 臂 + 未知方法 |

pub mod api;
pub mod dispatch;
pub mod msg;
pub mod patch;

pub use self::dispatch::dispatch;
pub use api::EngineApi;
pub use msg::*;
pub use patch::*;

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
