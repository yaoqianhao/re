//! 契约：`dispatch` 的分支键与 `r3e_core::cmd_spec` 的 RPC 条目必须一一对应。
//!
//! 前三处契约管「脚本注册 ↔ 表」「宿主实现 ↔ 表」，这里管第二边：「`dispatch` 认哪些
//! 方法名 ↔ 表里登记了哪些 RPC」。少一条分支，宿主实现得再好也是 `method not found`。
//!
//! 分支键不手抄：`include_str!` 读被测源码后扫 `match canonical(&req.method)` 的分支头。
//! `ping` 刻意不进表——它是链路健康检查，不是命令。

use std::collections::BTreeSet;

use r3e_core::cmd_spec;

/// `dispatch` 的实现自 M0-S5 起在 `r3e-protocol` 的三个文件里：`mod.rs` 元命令、
/// `scene.rs` / `media.rs` 分组臂。扫描的是**源码文本**，与 crate 依赖方向无关，
/// 所以契约测试仍留在本 crate（`r3e-core::cmd_spec` 是本 crate 的 dev-dependency，
/// 而协议 crate 一个引擎依赖都不许有）。
const META_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/mod.rs");
const SCENE_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/scene.rs");
const MEDIA_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/media.rs");
/// 事务五条（M1-S1）自成一臂，与上面三处并列。
const TX_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/tx.rs");
/// 语义查询三条（M1-S2-B）自成一臂。
const QUERY_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/query.rs");
/// 物理探测三条（M1-S4-D）自成一臂。
const PHYSICS_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/physics.rs");
/// 导航三条（M1-S4-D）：`physics.rs` 命中不到时转过来的第二段。
const NAV_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/nav.rs");
/// 检视四条（M1-S1-D）：`scene.snapshot` / `mesh.stats` / `scene.validate` /
/// `physics.simulate`，兜底链的最后一环。
const INSPECT_SRC: &str = include_str!("../../r3e-protocol/src/dispatch/inspect.rs");

/// 扫某个 `dispatch*` 函数里 `match` 的分支键（只认恰好 8 空格缩进的分支头，
/// 嵌套 `match` 里的分支缩进更深，不会被误收）。
fn scan_branches(src: &'static str, marker: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in src.lines() {
        // 用 `contains`：子文件里是 `pub(super) fn dispatch_scene(`，带可见性前缀。
        if line.contains(marker) {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with("fn ")
            || line.starts_with("pub fn ")
            || line.starts_with("pub(super) fn ")
            || line.starts_with('}')
        {
            break;
        }
        if !line.starts_with("        \"") {
            continue;
        }
        let Some((head, _)) = line.split_once("=>") else {
            continue;
        };
        // 一个分支头可能挂多个别名：`"engine.stats" | "enginestats" =>`。
        out.extend(quoted_parts(head));
    }
    out
}

/// 三处 `dispatch*` 的分支键并起来，才是完整方法表。
fn dispatch_keys() -> Vec<&'static str> {
    let mut out = scan_branches(META_SRC, "pub fn dispatch(");
    out.extend(scan_branches(SCENE_SRC, "fn dispatch_scene("));
    out.extend(scan_branches(MEDIA_SRC, "fn dispatch_media("));
    out.extend(scan_branches(TX_SRC, "fn dispatch_tx("));
    out.extend(scan_branches(QUERY_SRC, "fn dispatch_query("));
    out.extend(scan_branches(PHYSICS_SRC, "fn dispatch_physics("));
    out.extend(scan_branches(NAV_SRC, "fn dispatch_nav("));
    out.extend(scan_branches(INSPECT_SRC, "fn dispatch_inspect("));
    out
}

/// 取出 `s` 里所有双引号包裹的片段。
fn quoted_parts(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        match after.find('"') {
            Some(close) => {
                out.push(&after[..close]);
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

#[test]
fn dispatch的每个点分方法都在命令表里() {
    let keys = dispatch_keys();
    assert!(
        keys.len() >= 62,
        "只扫到 {} 个分支，扫描逻辑可能失效（期望 62 个即：39 基础 + 5 事务 + 3 查询 + 1 意图 \
         + 8 物理导航材质 + 1 体检 + 1 装配 + 4 检视）",
        keys.len()
    );

    let declared: BTreeSet<&str> = cmd_spec::rpc_names().collect();
    let missing: Vec<String> = keys
        .iter()
        .filter(|k| k.contains('.'))
        .filter(|k| !declared.contains(**k))
        .map(|k| format!("`dispatch` 能分派 `{k}`，命令表没登记"))
        .collect();
    assert!(
        missing.is_empty(),
        "{missing:#?}\n加 RPC 分支时请同步在 `crates/r3e-core/src/cmd_spec/` 补条目"
    );
}

#[test]
fn 命令表里的rpc方法dispatch都认() {
    let keys = dispatch_keys();
    let declared: BTreeSet<&str> = cmd_spec::rpc_names().collect();
    let phantom: Vec<&&str> = declared.iter().filter(|rpc| !keys.contains(*rpc)).collect();
    assert!(
        phantom.is_empty(),
        "命令表登记了、`dispatch` 根本不认（表在骗人）：{phantom:?}"
    );
}

#[test]
fn ping是健康检查不进命令表() {
    let keys = dispatch_keys();
    assert!(keys.contains(&"ping"), "`dispatch` 少了 ping 分支");
    assert!(cmd_spec::by_rpc("ping").is_none(), "ping 不该被登记成命令");
}

#[test]
fn 无点别名不参与对齐且只有一个规范名() {
    let keys = dispatch_keys();
    let aliases: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|k| !k.contains('.') && *k != "ping")
        .collect();
    assert_eq!(
        aliases.len(),
        2,
        "无点别名应只有 captureframe / enginestats，实际：{aliases:?}"
    );
    for alias in aliases {
        assert!(
            cmd_spec::by_rpc(alias).is_none(),
            "别名 `{alias}` 不该进命令表，规范名才进"
        );
    }
}
