//! 场景保存 / 栈 / 存档（编辑器 RPC 宿主层）。
//!
//! 由 M0-S2 从单体 `api.rs` 搬入：`self` → `api`，其余语句未改写。

use super::support::*;
use super::*;

pub(super) fn scene_save(
    api: &mut LocalEngineApi<'_>,
    path: Option<&str>,
) -> Result<Value, RpcFailure> {
    // 目标路径：显式给了就用它，否则写回本场景的原始文件（编辑器「Ctrl+S」语义）。
    let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            if api.scene_label.trim().is_empty() {
                return Err(RpcFailure::invalid_params(
                    "当前没有场景文件路径，请显式给出 `path`",
                ));
            }
            api.scene_label.clone()
        }
    };
    let scene = api.scene.borrow();
    scene.save_to_path(&target).map_err(RpcFailure::engine)?;
    // 相对路径按进程工作目录解析：回给调用方的是绝对路径与实际字节数
    let abs = std::fs::canonicalize(&target).unwrap_or_else(|_| std::path::PathBuf::from(&target));
    let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "path": plain_abs_path(&abs),
        "objects": scene.len(),
        "bytes": bytes,
    }))
}

/// 列出场景栈（扩展十八 B）。
///
/// 每层只给**声明态**（层号 / 名字 / 来源路径 / 源文本字节数）：面板刷新每帧都在发，
/// 真把十几层的场景 JSON 全塞进一个响应，网络和 Rust 侧的序列化都会变成瓶颈。
pub(super) fn scene_stack(
    api: &mut LocalEngineApi<'_>,
    limit: Option<usize>,
) -> Result<Value, RpcFailure> {
    let handle = api.scripts.scene_stack_handle();
    let stack = handle.borrow();
    let total = stack.depth();
    let take = limit.unwrap_or(total).min(total);
    let current = stack.current_index();
    let entries: Vec<Value> = stack
        .entries()
        .iter()
        .enumerate()
        .take(take)
        .map(|(index, entry)| {
            json!({
                "index": index,
                "name": entry.name,
                "path": entry.path,
                "source_bytes": entry.source.len(),
                "frame": entry.runtime.frame,
                "time": entry.runtime.time,
                "current": current == Some(index),
            })
        })
        .collect();
    Ok(json!({
        "count": total,
        "current": current,
        "max": MAX_SCENE_STACK,
        "entries": entries,
    }))
}

/// 请求切到某个场景文件：只做「路径存在性校验 + 排队」，真正换世界由宿主帧首执行。
pub(super) fn scene_switch(api: &mut LocalEngineApi<'_>, path: &str) -> Result<Value, RpcFailure> {
    let target = resolve_request_path(api.scene_label.as_str(), path);
    if !target.is_file() {
        return Err(RpcFailure::invalid_params(format!(
            "场景文件不存在：{}",
            target.display()
        )));
    }
    queue_scene_command(
        api.scripts,
        SceneCommand::switch(target.display().to_string()),
    )
}

/// 请求回到栈里的上一层（不删层）。
pub(super) fn scene_back(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    // 先看一眼栈：已经在最早一层时立刻报错，比"排队成功但下一帧静默失败"好得多
    let blocked = {
        let handle = api.scripts.scene_stack_handle();
        let stack = handle.borrow();
        match stack.current_index() {
            Some(0) | None => true,
            Some(_) => false,
        }
    };
    if blocked {
        return Err(RpcFailure::engine(
            "已在最早一层，无法回退（可用 `scene.stack` 查看栈）",
        ));
    }
    queue_scene_command(api.scripts, SceneCommand::back())
}

/// 存一份带运行时快照的存档。
///
/// 存档只写盘、不动世界，所以**当场写完**再返回真实路径：调用方拿到的是
/// 「文件已存在」这一确定结果，而不是一句"排队成功"。
pub(super) fn scene_archive_save(
    api: &mut LocalEngineApi<'_>,
    path: Option<&str>,
) -> Result<Value, RpcFailure> {
    let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            if api.scene_label.trim().is_empty() {
                return Err(RpcFailure::invalid_params(
                    "当前没有场景文件路径，请显式给出 `path`",
                ));
            }
            r3e_runtime::app::default_archive_path(std::path::Path::new(&api.scene_label))
        }
    };
    if let Some(dir) = target.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            std::fs::create_dir_all(dir)
                .map_err(|e| RpcFailure::engine(format!("创建存档目录失败: {e}")))?;
        }
    }
    let archive = SceneArchive::from_scene(&api.scene.borrow());
    archive.save_to_path(&target).map_err(RpcFailure::engine)?;
    let abs = std::fs::canonicalize(&target).unwrap_or_else(|_| std::path::PathBuf::from(&target));
    let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "path": plain_abs_path(&abs),
        "format": ARCHIVE_FORMAT,
        "version": ARCHIVE_VERSION,
        "bytes": bytes,
        "objects": archive.scene.objects.len(),
        "anims": archive.runtime.as_ref().map(|r| r.anims.len()).unwrap_or(0),
        "queued": false,
    }))
}

/// 请求读档：与 `scene_switch` 同理，只校验 + 排队。
pub(super) fn scene_archive_load(
    api: &mut LocalEngineApi<'_>,
    path: &str,
) -> Result<Value, RpcFailure> {
    let target = resolve_request_path(api.scene_label.as_str(), path);
    if !target.is_file() {
        return Err(RpcFailure::invalid_params(format!(
            "存档文件不存在：{}",
            target.display()
        )));
    }
    queue_scene_command(
        api.scripts,
        SceneCommand::load_archive(target.display().to_string()),
    )
}
