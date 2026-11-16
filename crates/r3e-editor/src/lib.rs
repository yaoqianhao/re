//! r3e-editor —— 编辑器桥的**传输层**。
//!
//! 协议层（JSON-RPC 2.0 的请求 / 响应 / 错误码 / 方法表、`*Patch`、`EngineApi`、
//! `dispatch`）自 M0-S5 起搬到了独立 crate `r3e-protocol`，本 crate 只剩一件事：
//! **传输** —— `127.0.0.1:<port>` 上的 TCP + NDJSON，读写各在自己的线程里，
//! **引擎状态只由主线程碰**（见 [`server::Job`]）。
//!
//! # 为什么保留 `pub use r3e_protocol::*`
//!
//! 剥离采用「纯搬迁 + 整体再导出」：`r3e-editor-host` 与本 crate 的 9 个集成测试
//! 都还写着 `use r3e_editor::{EngineApi, TransformPatch, dispatch, …}`，再导出让它们
//! 一行不改就继续编译（M0-S5 的验收要求）。M1 会把这些引用迁到 `r3e_protocol::`，
//! 然后删掉这层壳（见 `docs/M0S5_PROTOCOL.md` §12）。
//!
//! 依赖方向上：`r3e-editor -> r3e-protocol`，协议 crate 不认识任何引擎类型，
//! 所以既不依赖 `r3e-core`，也不会出现 `core -> editor` 的反向耦合。
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

pub use r3e_protocol::*;
pub use server::{Job, RpcServer};
