//! 编辑器宿主：把 `r3e-editor` 的 JSON-RPC 服务端接进运行时的帧循环。
//!
//! 分界线很清楚：
//! * 协议（方法名、参数校验、错误码）在 `r3e-editor`，它不认识引擎；
//! * 帧循环在 `r3e-runtime`，它不认识协议；
//! * **本 crate 是两者唯一的黏合层**——把 [`r3e_runtime::EngineAccess`] 里的
//!   借用打包成 [`api::LocalEngineApi`]，喂给协议层取出来的 `Job`。
//!
//! 这样「引擎要跑」不必链接协议，「协议要测」也不必起窗口。

pub mod api;
pub mod host;
pub mod verify;

pub use api::LocalEngineApi;
pub use host::RpcHost;
pub use verify::{Layer, PhysicsProbe, Unchecked, VerifyReport};

/// 按命令行给的端口 / 令牌装配一个编辑器宿主。
///
/// 返回 `Box<dyn EditorService>` 而不是具体类型：主循环只该知道「有个东西每帧
/// 能给我处理请求」，不该知道它是 TCP JSON-RPC 还是别的什么传输。
pub fn bind(
    port: u16,
    token: Option<String>,
) -> std::io::Result<Box<dyn r3e_runtime::EditorService>> {
    Ok(Box::new(RpcHost::bind(port, token)?))
}
