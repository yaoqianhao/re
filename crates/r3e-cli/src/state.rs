//! 会话态：把「当前这笔事务的 txId」落在本地文件里。
//!
//! # 为什么必须落盘
//!
//! `txId` 由宿主分配、**只在引擎进程内有效**（不是持久 id，更不是可推导的）。
//! 而 AI 调 CLI 是一次一条命令、一次一个进程——上一条 `tx begin` 拿到的 txId
//! 若不落盘，下一条 `tx apply` 就无从引用。所以「一次会话 = 一个文件」是 S5
//! 的硬需求，不是便利设施。
//!
//! # 三条约定
//!
//! 1. **缺省路径** `.r3e-cli/session.json`（相对工作目录）：与项目同目录便于隔仓隔离，
//!    又不会因为 cd 到子目录就换文件（相对路径以「用户敲命令时的工作目录」为准）。
//! 2. **`--state-file ""` 关闭**：多会话并行（多个 AI 同时改同一引擎）时共享一个
//!    文件会互相踩，此时必须显式关闭并改用 `--tx <id>`。
//! 3. **写坏不算错**：状态文件只是缓存，读失败就当「没有 txId」，让调用方重新
//!    `tx begin`。因为把「文件损坏」升级成致命错误只会挡住本该能继续的流程。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// 会话态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// 当前未决事务的 txId。
    pub tx_id: String,
    /// 记录时的引擎地址：换了地址再用旧 txId 一定是「查无此事务」，
    /// 留着只为在报错时能提示「你是不是连错引擎了」。
    pub addr: Option<String>,
}

/// 读会话态；文件不存在、内容不符、字段缺失一律返回 `None`。
pub fn load(path: &Path) -> Option<Session> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let tx_id = value.get("txId")?.as_str()?.trim().to_string();
    if tx_id.is_empty() {
        return None;
    }
    let addr = value
        .get("addr")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(Session { tx_id, addr })
}

/// 写会话态（父目录自动创建）。
pub fn save(path: &Path, tx_id: &str, addr: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("创建状态目录 {} 失败：{e}", dir.display()))?;
        }
    }
    let payload = json!({
        "txId": tx_id,
        "addr": addr,
        "savedAtUnix": now_unix(),
        // 写给「用编辑器打开这个文件的人」看的：它是什么、什么时候该删。
        "note": "r3e-cli 的当前事务；txId 只在引擎进程内有效，引擎重启后必须重新 tx begin",
    });
    let text =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("序列化会话态失败：{e}"))?;
    write_atomic(path, &text)
}

/// 清会话态（事务终结）。文件本来就不在也算成功。
pub fn clear(path: &Path) {
    let _ = fs::remove_file(path);
    // 顺手收掉空目录，别在工作目录里留一个 `.r3e-cli/` 空壳。
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// 会话态文件是否「明显属于另一个引擎」。
///
/// 只做提示用：不据此拒绝执行（同一个 port 换进程是常见操作，此时旧 txId 会由引擎
/// 回 `TX_NOT_FOUND` 精确地报出来，比客户端猜更准）。
pub fn addr_mismatch(session: &Session, addr: &str) -> bool {
    session
        .addr
        .as_deref()
        .is_some_and(|recorded| recorded != addr)
}

/// 先写临时文件再 rename：避免进程被杀在中间时留下半个 JSON，
/// 让下一次读直接当成「没有 txId」而白白丢掉一笔未提交事务。
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = sibling_temp(path);
    fs::write(&tmp, text).map_err(|e| format!("写临时文件 {} 失败：{e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("落盘 {} 失败：{e}", path.display())
    })
}

fn sibling_temp(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp{}", std::process::id()));
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独占一个目录，避免并行跑测试时互相踩状态文件。
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "r3e-cli-state-{}-{tag}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = scratch("round");
        let path = dir.join("nested").join("session.json");
        save(&path, "tx-42", "127.0.0.1:8787").expect("落盘应成功");
        let session = load(&path).expect("应能读回");
        assert_eq!(session.tx_id, "tx-42");
        assert_eq!(session.addr.as_deref(), Some("127.0.0.1:8787"));
        clear(&path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_not_an_error() {
        let dir = scratch("missing");
        assert_eq!(load(&dir.join("nope.json")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_json_counts_as_no_session() {
        // 状态文件只是缓存：读坏了就当没有，让调用方重新 tx begin。
        let dir = scratch("broken");
        fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join("session.json");
        fs::write(&path, "{not json").expect("写坏文件");
        assert_eq!(load(&path), None);
        fs::write(&path, r#"{"addr":"127.0.0.1:1"}"#).expect("缺 txId");
        assert_eq!(load(&path), None);
        fs::write(&path, r#"{"txId":"   "}"#).expect("空白 txId");
        assert_eq!(load(&path), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_addr_loads_as_none() {
        let dir = scratch("addr");
        fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join("session.json");
        fs::write(&path, r#"{"txId":"tx-1"}"#).expect("写文件");
        let session = load(&path).expect("应能读回");
        assert_eq!(session.tx_id, "tx-1");
        assert_eq!(session.addr, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_file_and_empty_dir() {
        let dir = scratch("clear");
        let path = dir.join("only").join("session.json");
        save(&path, "tx-9", "127.0.0.1:8787").expect("落盘应成功");
        clear(&path);
        assert!(!path.exists(), "状态文件应被删掉");
        assert!(
            !path.parent().expect("有父目录").exists(),
            "空的 `.r3e-cli/` 不该留在工作目录里"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_is_idempotent() {
        let dir = scratch("idem");
        clear(&dir.join("never-existed.json"));
        clear(&dir.join("never-existed.json"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn addr_mismatch_only_when_addr_was_recorded_and_differs() {
        let recorded = Session {
            tx_id: "tx-1".to_string(),
            addr: Some("127.0.0.1:8787".to_string()),
        };
        assert!(!addr_mismatch(&recorded, "127.0.0.1:8787"));
        assert!(addr_mismatch(&recorded, "127.0.0.1:9999"));
        let unknown = Session {
            tx_id: "tx-1".to_string(),
            addr: None,
        };
        assert!(
            !addr_mismatch(&unknown, "127.0.0.1:9999"),
            "没记录地址就不该报警"
        );
    }

    #[test]
    fn save_writes_a_human_readable_note() {
        // 状态文件可能被人用编辑器打开：里面必须写清「引擎重启后它就作废」。
        let dir = scratch("note");
        let path = dir.join("session.json");
        save(&path, "tx-3", "127.0.0.1:8787").expect("落盘应成功");
        let text = fs::read_to_string(&path).expect("读回");
        assert!(text.contains("note"), "{text}");
        assert!(text.contains("tx begin"), "note 里应指向 tx begin：{text}");
        let _ = fs::remove_dir_all(&dir);
    }
}
