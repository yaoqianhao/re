//! 脚本求值与热重载（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn script_eval(api: &mut LocalEngineApi<'_>, source: &str) -> Result<Value, RpcFailure> {
    let value = api.scripts.eval(source).map_err(RpcFailure::engine)?;
    Ok(dynamic_to_json(&value))
}

pub(super) fn script_reload(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let path = api
        .scripts
        .script_path()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| RpcFailure::engine("当前宿主没有载入脚本文件，无法重载"))?;
    api.scripts.load_file(&path).map_err(RpcFailure::engine)?;
    Ok(json!({ "reloaded": path.display().to_string() }))
}
