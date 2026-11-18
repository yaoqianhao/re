//! 子命令实现与连接/调用的公共通道。
//!
//! 四个子命令的骨架完全一致：**建连 → 调一次 → 翻译结果/错误 → 退出码**。
//! 所以「建连」与「调用」的错误处理只在这里写一遍：任何一处漏翻译，
//! 就会出现「同一种失败在不同子命令下退出码不同」的怪事。

use serde_json::Value;

use r3e_protocol::client::{ClientError, Endpoint, RpcClient};

use crate::args::{Cmd, Global};
use crate::errmap;
use crate::exit;
use crate::output::Out;

pub mod query;
pub mod replay;
pub mod tx;
pub mod verify;

/// 分发。
pub fn run(global: &Global, cmd: Cmd) -> i32 {
    match cmd {
        Cmd::Tx { action, tx, args } => tx::run(global, action, tx, args),
        Cmd::Query { method, params } => query::run(global, method, params),
        Cmd::Verify { targets, limit } => verify::run(global, targets, limit),
        Cmd::Replay(replay_args) => replay::run(global, replay_args),
        Cmd::Help => {
            print!("{}", crate::args::help_text());
            exit::OK
        }
    }
}

/// 建连接。
///
/// 失败时把「引擎侧该检查什么」打到 stderr 并给出退出码——不把 `Transport` 原样
/// 扔给用户，因为 `Connection refused` 对「忘了给 `--rpc-port`」这个最常见原因是
/// 零信息量的。
pub fn connect(global: &Global) -> Result<RpcClient, i32> {
    let endpoint = Endpoint::resolve(global.addr.as_deref(), global.token.as_deref())
        .with_io_timeout_ms(global.timeout_ms);
    match RpcClient::connect(&endpoint) {
        Ok(client) => Ok(client),
        Err(error) => {
            report(global, &error);
            Err(errmap::exit_code(&error))
        }
    }
}

/// 调一次 RPC。
pub fn call(
    global: &Global,
    client: &mut RpcClient,
    method: &str,
    params: Value,
) -> Result<Value, i32> {
    match client.call(method, params) {
        Ok(reply) => Ok(reply),
        Err(error) => {
            report(global, &error);
            Err(errmap::exit_code(&error))
        }
    }
}

/// 统一打错误：一句话结论 + 「下一步」。
pub fn report(global: &Global, error: &ClientError) {
    let out = Out::new(global);
    let (summary, details) = errmap::explain(error);
    out.failure(&summary, &details);
}
