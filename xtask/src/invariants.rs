//! `cargo xtask invariants`：不变量三源比对（M0-S5 / S5-05）。
//!
//! 三源与各自职责：
//!
//! | 源 | 提供 | 位置 |
//! |---|---|---|
//! | A | 引擎声明自己会报哪些 code（真源） | `r3e_doc::ISSUE_CODES` |
//! | B | 有反例测试兜住的 code | `crates/r3e-doc/tests/*.rs` 与 `crates/r3e-editor-host/tests/*.rs` 的断言 helper 字面量 |
//! | C | 对外清单声明的 code | `docs/capabilities.json` 的 `invariants[].code` |
//!
//! 判据是**三集合相等**，任一差集非空即非 0 退出，并分别打印：
//!
//! - `missing_in_tests`：登记了却没有反例测试——新增不变量最常见的漏项
//! - `stale_in_tests`：测试断言了没登记的 code——改 code 文本没同步测试
//! - `missing_in_caps`：登记了但清单里没有——忘了 `gen-capabilities`
//! - `stale_in_caps`：清单里有但登记表已删——删规则没重生成清单
//!
//! 这是**元测试**：回答「三个集合是否对齐」，不回答「规则实现是否正确」
//! （后者由 `tests/validate.rs` 的场景测试负责）。因为只读源码不跑引擎，
//! 一次几十毫秒，才有资格进 `verify` 每轮都跑。
//!
//! 扫字面量而不解析 Rust AST：断言 helper 调用的首个字符串实参恒为 code，
//! 约定由测试文件头部文档固定；为几十行的门禁引 `syn` 得不偿失。
//!
//! **B 源为什么要跨两个 crate（M1-S4-E）**：静态层判出来的码（`support.*` / `overlap.*`）
//! 反例在 `r3e-doc`，物理层 / 导航空层的码（`support.floating` / `overlap.penetration` /
//! `nav.*`）只能在 `r3e-editor-host` 里构造——它们需要物理后端与烘焙好的 navmesh。
//! 只扫 `r3e-doc/tests` 会让这 6 条码永远「登记了却没有反例测试」，门禁要么红、
//! 要么被逼着去写没有真实断言的空测试。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use r3e_doc::ISSUE_CODES;

/// 反例测试的断言入口（测试文件里这些 helper 固定「首个字符串实参 = code」）。
///
/// - `assert_error` / `assert_warn`：`r3e-doc/tests` 的静态层断言；
/// - `assert_geometry_warn`：`r3e-doc/tests` 的静态层几何初筛（`validate_geometry`）断言；
/// - `has_code`：`r3e-editor-host/tests` 的三层验证报告断言（物理层 / 导航层结论）。
const ASSERT_HELPERS: [&str; 4] = [
    "assert_error(",
    "assert_warn(",
    "assert_geometry_warn(",
    "has_code(",
];

/// B 源的扫描目录（相对仓库根）。
///
/// 分成「静态层」与「三层验证」两处，是因为后者依赖物理 / 导航后端，代码不可能写在
/// `r3e-doc` 里；但它们的结论码与静态层同属 [`ISSUE_CODES`]，必须有反例兜住。
const TEST_DIRS: [&str; 2] = ["crates/r3e-doc/tests", "crates/r3e-editor-host/tests"];

/// 子命令入口。返回进程退出码（0 = 三源一致）。
pub fn run(root: &Path) -> i32 {
    let a: BTreeSet<String> = ISSUE_CODES.iter().map(|ic| ic.code.to_string()).collect();

    let b = match asserted_codes(root) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[invariants] {e}");
            return 1;
        }
    };

    let c = match listed_codes(root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[invariants] {e}");
            return 1;
        }
    };

    println!(
        "[invariants] A(登记表)={} B(反例测试)={} C(能力清单)={}",
        a.len(),
        b.len(),
        c.len()
    );

    let pairs: [(&str, &BTreeSet<String>, &BTreeSet<String>); 4] = [
        ("missing_in_tests", &a, &b),
        ("stale_in_tests", &b, &a),
        ("missing_in_caps", &a, &c),
        ("stale_in_caps", &c, &a),
    ];

    let mut failed = false;
    for (label, left, right) in pairs {
        let diff: Vec<&String> = left.difference(right).collect();
        if diff.is_empty() {
            continue;
        }
        failed = true;
        eprintln!("[invariants] {label}（{} 条）：", diff.len());
        for code in diff {
            eprintln!("  - {code}");
        }
    }

    if failed {
        eprintln!(
            "[invariants] 三源不一致：补测试改 r3e-doc/tests/ 或 r3e-editor-host/tests/、改清单跑 \
             `cargo xtask gen-capabilities`、改登记表看 docs/m0/M0S5_PROTOCOL.md 的 code 契约"
        );
        return 1;
    }

    println!("[invariants] 三源一致");
    0
}

/// B：收集测试里被断言过的 code。
fn asserted_codes(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in TEST_DIRS {
        let dir = root.join(dir);
        collect_rs(&dir, &mut files)?;
    }
    if files.is_empty() {
        return Err(format!("{:?} 下没有测试文件，无法比对 B 源", TEST_DIRS));
    }

    let mut out = BTreeSet::new();
    for file in files {
        let text =
            fs::read_to_string(&file).map_err(|e| format!("读不到 {}：{e}", file.display()))?;
        for helper in ASSERT_HELPERS {
            out.extend(first_string_args(&text, helper));
        }
    }
    Ok(out)
}

/// C：收集 `docs/capabilities.json` 里声明的 code。
fn listed_codes(root: &Path) -> Result<BTreeSet<String>, String> {
    let path = root.join("docs/capabilities.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("读不到 {}：{e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} 不是合法 JSON：{e}", path.display()))?;
    let arr = json
        .get("invariants")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("{} 缺 invariants 数组", path.display()))?;

    let mut out = BTreeSet::new();
    for item in arr {
        let code = item
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{} 的 invariants[] 有缺 code 的条目", path.display()))?;
        out.insert(code.to_string());
    }
    Ok(out)
}

/// 递归收集 `*.rs`（目录不存在视为空，由调用方判定）。
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("读不到目录 {}：{e}", dir.display())),
    };
    for entry in entries {
        let path = entry
            .map_err(|e| format!("遍历 {} 失败：{e}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// 每次**调用** `needle` 后，取紧跟的首个字符串字面量内容。
///
/// 两处刻意收紧：
/// - 跳过 helper 自身的定义行（`fn assert_error(...)`），否则会把函数体里
///   `assert!` 的提示文案当成 code 收进来；
/// - 只收点分小写形态（与 `ISSUE_CODES` 的 code 契约一致），文案 / 路径字面量天然被滤掉。
fn first_string_args(text: &str, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(needle) {
        let call_start = from + found + needle.len();
        if !text[..from + found].trim_end().ends_with("fn") {
            if let Some(q) = text[call_start..].find('"') {
                let body_start = call_start + q + 1;
                if let Some(end) = text[body_start..].find('"') {
                    let code = &text[body_start..body_start + end];
                    if is_code_shape(code) {
                        out.push(code.to_string());
                    }
                }
            }
        }
        from = call_start;
    }
    out
}

/// code 契约：点分小写，形如 `node.name.empty`。
fn is_code_shape(s: &str) -> bool {
    s.contains('.')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 取首个字符串实参() {
        let mut got =
            first_string_args(r#"assert_error(&doc, "node.name.empty");"#, "assert_error(");
        assert_eq!(got.pop().as_deref(), Some("node.name.empty"));
    }

    #[test]
    fn 跨行调用也能取到() {
        let src = "assert_warn(\n    &doc,\n    \"skin.joint_matrices.unsolved\",\n);";
        let mut got = first_string_args(src, "assert_warn(");
        assert_eq!(got.pop().as_deref(), Some("skin.joint_matrices.unsolved"));
    }

    #[test]
    fn 跳过helper定义只收调用() {
        let src = "fn assert_error(doc: &Document, code: &str) {\n    assert!(found.contains(&code), \"期望 {code}，实得 {found:?}\");\n}\n\n#[test]\nfn t() {\n    assert_error(&doc, \"ui.key.empty\");\n}";
        let got = first_string_args(src, "assert_error(");
        assert_eq!(got, vec!["ui.key.empty".to_string()]);
    }

    #[test]
    fn 多次调用全收() {
        let src = "assert_error(&a, \"a.b\");\nassert_error(&b, \"c.d\");";
        let got = first_string_args(src, "assert_error(");
        assert_eq!(got, vec!["a.b".to_string(), "c.d".to_string()]);
    }
}
