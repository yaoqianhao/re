//! ai_ide 插件协议的帧收发：**NDJSON over stdio**。
//!
//! 规范来自 ai_ide `docs/14-插件系统设计.md` 与 `crates/plugin/src/host.rs`：
//!
//! * 每一帧是一行 JSON（`\n` 结尾），单帧上限 1 MiB（超限判 6106 协议违规）；
//! * 帧分三类：请求（有 `id` + `method`）、响应（有 `id`，有 `result` / `error`）、
//!   通知（无 `id`）；
//! * 宿主发来的方法只有 `plugin/handshake`（兼容 `plugin/initialize`）、
//!   `plugin/tool-invoke`、`plugin/command-invoke`、`plugin/event`、`plugin/shutdown`；
//! * 插件反向调用宿主只允许 `host/*` 命名空间，且必须在工具调用内部发起。
//!
//! 这一层只做「字节 ↔ `serde_json::Value`」，不理解任何业务语义。

use std::io::{BufRead, Write};

use serde_json::{json, Value};

/// 单帧上限，与宿主 `MAX_FRAME_BYTES` 对齐（1 MiB）。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 帧读写失败的原因。
#[derive(Debug)]
pub enum FrameError {
    /// 底层 IO 出错（管道断开等）。
    Io(String),
    /// 单帧超过 [`MAX_FRAME_BYTES`]：宿主会判协议违规，插件自己也停下来。
    TooLarge { bytes: usize },
    /// 不是合法 UTF-8，或不是合法 JSON。
    Json(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "stdio 读写失败：{err}"),
            Self::TooLarge { bytes } => write!(
                f,
                "单帧 {bytes} 字节，超过上限 {MAX_FRAME_BYTES}（协议违规，宿主判 6106）"
            ),
            Self::Json(err) => write!(f, "帧不是合法 JSON：{err}"),
        }
    }
}

/// 读一帧。`Ok(None)` 表示 stdin 已关闭（宿主走了，插件应当退出）。
///
/// 空行会被跳过：`trim()` 之后为空即继续读下一行，避免把心跳空行当坏帧。
pub fn read_frame<R: BufRead>(reader: &mut R) -> Result<Option<Value>, FrameError> {
    let mut buffer: Vec<u8> = Vec::with_capacity(512);
    loop {
        buffer.clear();
        // `take` 只限制**本帧**的读取量，超限时不会把后续帧一起吞掉；
        // 同时避免恶意/失控的宿主发一条几百 MB 的单行把内存撑爆。
        let read = std::io::Read::take(&mut *reader, (MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut buffer)
            .map_err(|err| FrameError::Io(err.to_string()))?;
        if read == 0 {
            return Ok(None);
        }
        if read > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge { bytes: read });
        }
        let text = std::str::from_utf8(&buffer).map_err(|err| FrameError::Json(err.to_string()))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed)
            .map(Some)
            .map_err(|err| FrameError::Json(err.to_string()));
    }
}

/// 写一帧：序列化成单行 JSON 并立刻 flush（宿主按行读，不 flush 就是死等）。
pub fn write_frame<W: Write>(writer: &mut W, value: &Value) -> Result<(), FrameError> {
    let mut text = serde_json::to_string(value).map_err(|err| FrameError::Json(err.to_string()))?;
    text.push('\n');
    writer
        .write_all(text.as_bytes())
        .map_err(|err| FrameError::Io(err.to_string()))?;
    writer
        .flush()
        .map_err(|err| FrameError::Io(err.to_string()))
}

/// 组装一个请求帧。
pub fn request_frame(id: Value, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// 组装一个成功响应帧。`id` 原样回填宿主给的类型（字符串或数字都合法）。
pub fn result_frame(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// 组装一个错误响应帧。
pub fn error_frame(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// 取帧里的 `method`：没有 `method` 说明是响应（或噪声），调用方自行处理。
pub fn method_of(frame: &Value) -> Option<&str> {
    frame.get("method").and_then(Value::as_str)
}

/// 取帧里的 `id`：`None` 表示这是通知（不需要回包）。
pub fn id_of(frame: &Value) -> Option<Value> {
    frame.get("id").cloned().filter(|id| !id.is_null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn blank_lines_are_skipped() {
        let mut cursor = Cursor::new(b"\n  \n{\"method\":\"plugin/handshake\"}\n".to_vec());
        let frame = read_frame(&mut cursor).expect("read frame").expect("frame");
        assert_eq!(method_of(&frame), Some("plugin/handshake"));
    }

    #[test]
    fn eof_returns_none() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_frame(&mut cursor).expect("read frame").is_none());
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut payload = vec![b'a'; MAX_FRAME_BYTES + 8];
        payload.push(b'\n');
        let mut cursor = Cursor::new(payload);
        match read_frame(&mut cursor) {
            Err(FrameError::TooLarge { .. }) => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_rejected() {
        let mut cursor = Cursor::new(b"{not json}\n".to_vec());
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Json(_))));
    }

    #[test]
    fn written_frame_is_single_line() {
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, &result_frame(json!("a1"), json!({ "ok": true }))).expect("写帧");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.ends_with('\n'));
        assert_eq!(text.matches('\n').count(), 1);
        let parsed: Value = serde_json::from_str(text.trim()).expect("合法 json");
        assert_eq!(parsed["id"], json!("a1"));
        assert_eq!(parsed["result"]["ok"], json!(true));
    }
}
