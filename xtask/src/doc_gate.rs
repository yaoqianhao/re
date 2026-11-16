//! `cargo xtask doc-gate`：文档层收口的六条静态门禁。
//!
//! 规则来自 `docs/M0S3_DOC_LAYER.md` §5。收口做完不算完——只要后续有人再写一个
//! `Rc<RefCell<Scene>>` 或直接 `Scene::from_json_str`，P4 那 129 个写点就白迁了。
//! 这里把六条约定变成 `verify` 会跑的检查。
//!
//! | # | 规则 | 例外 |
//! |---|---|---|
//! | 1 | `RefCell<Scene>` 只允许在 `crates/r3e-doc/`（测试块内放宽） | `#[cfg(test)]` 之后的代码 |
//! | 2 | 返回/接收 `&mut Scene` 的公开 API 只允许在 `crates/r3e-doc/` | `doc_gate.json` 逐条登记 |
//! | 3 | `DerefMut` 一词在 `crates/r3e-doc/` 出现即失败 | 无 |
//! | 4 | 上层不得出现 `Scene::from_json_str`，只走 `r3e_doc::parse_scene` / `Document::from_json_str` | 无 |
//! | 5 | 未登记 `xtask/dep_rules.json` 的 crate 一律失败 | 无 |
//! | 6 | `write(|s| ...)` 闭包体内不得再借用场景载体（RefCell 重入） | 无 |
//! | 7 | `.write(` 写点数不得超过 `docs/WRITE_SITES.md` 台账登记（M1-S1 起） | 无，台账即登记 |
//!
//! 白名单和 `lint-size` 一样**只许减**：条目对不上就报「可删」，多出来的直接失败。
//! 规则 7 同理——台账数字比实际大就提示「可下调」，比实际小就红。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::{read_json, rel, rust_files};

/// 文档层目录：规则 1、2 对这里完全豁免。
const DOC_CRATE: &str = "crates/r3e-doc/";

pub fn run(root: &Path) -> i32 {
    let cfg = read_json(&root.join("xtask/doc_gate.json"));
    let scope4: Vec<String> = cfg["rule4_scope"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let scene_keys: Vec<String> = cfg["rule6_scene_keys"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                "scene".into(),
                "doc".into(),
                "document".into(),
                "self.scene".into(),
                "self.doc".into(),
            ]
        });

    // 规则 2 白名单：(路径, 签名片段) -> (登记条数, 备注)
    let mut allow: BTreeMap<(String, String), (usize, String)> = BTreeMap::new();
    if let Some(list) = cfg["rule2_exceptions"].as_array() {
        for e in list {
            let path = e["path"].as_str().unwrap_or_default().to_string();
            let needle = e["needle"].as_str().unwrap_or_default().to_string();
            let count = e["count"].as_u64().unwrap_or(1) as usize;
            let note = e["note"].as_str().unwrap_or("").to_string();
            allow.insert((path, needle), (count, note));
        }
    }
    let mut used: BTreeMap<(String, String), usize> = BTreeMap::new();

    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    files.sort();

    let mut bad: Vec<String> = Vec::new();
    let mut checked = 0usize;
    // 规则 7 用：crate 目录名 -> `.write(` 出现次数
    let mut write_sites: BTreeMap<String, usize> = BTreeMap::new();

    for f in &files {
        let Ok(text) = fs::read_to_string(f) else {
            continue;
        };
        checked += 1;
        let path = rel(root, f);
        let is_doc = path.starts_with(DOC_CRATE);
        let in_scope4 = scope4.iter().any(|p| path.starts_with(p.as_str()));
        let lines: Vec<&str> = text.lines().collect();

        // ---- 规则 7 计数：按 crate 累计 `.write(` 字面次数（注释与字符串也算，
        // 口径与 docs/WRITE_SITES.md 的人工统计保持一致，避免"看起来降了其实是注释")
        if let Some(crate_name) = path
            .strip_prefix("crates/")
            .and_then(|p| p.split('/').next())
        {
            let n = text.matches(".write(").count();
            if n > 0 {
                *write_sites.entry(crate_name.to_string()).or_insert(0) += n;
            }
        }

        // ---- 规则 1 / 3：逐行文本扫描（规则 1 需要知道是否已在测试块内）
        let mut in_test = false;
        for (i, raw) in lines.iter().enumerate() {
            let t = raw.trim();
            // 注释里提到这些词不算违规：只看 `//` 之前的代码部分。
            let code = raw.split("//").next().unwrap_or("");
            if t.starts_with("#[cfg(test)]") {
                in_test = true;
            }
            if is_doc {
                // 规则 3：文档层连这个词都不许出现。
                if raw.contains("DerefMut") {
                    bad.push(format!("规则3 {path}:{} 文档层出现可变 Deref", i + 1));
                }
            } else if code.contains("RefCell<Scene>") && !in_test {
                // 规则 1：唯一真值只能被 Document 独占，别处不许再套 RefCell。
                bad.push(format!(
                    "规则1 {path}:{} RefCell<Scene> 只允许出现在 r3e-doc",
                    i + 1
                ));
            }
        }

        // ---- 规则 4：上层不得直连 core 的场景解析
        if in_scope4 {
            for (i, raw) in lines.iter().enumerate() {
                let code = raw.split("//").next().unwrap_or("");
                if code.contains("Scene::from_json_str") {
                    bad.push(format!(
                        "规则4 {path}:{} 场景解析请走 r3e_doc::parse_scene / Document::from_json_str",
                        i + 1
                    ));
                }
            }
        }

        // ---- 规则 2：公开 API 不得泄漏 &mut Scene
        if !is_doc {
            let mut i = 0usize;
            while i < lines.len() {
                let t = lines[i].trim();
                let looks_sig = (t.starts_with("pub ")
                    || t.starts_with("pub(")
                    || t.starts_with("fn ")
                    || t.starts_with("unsafe fn"))
                    && !t.starts_with("//");
                if !looks_sig {
                    i += 1;
                    continue;
                }
                // 多行签名：一路拼到 `{` 或 `;`（最多 6 行，够覆盖本项目全部签名）
                let mut sig = t.to_string();
                let mut j = i;
                while !(sig.trim_end().ends_with('{') || sig.trim_end().ends_with(';'))
                    && j - i < 6
                    && j + 1 < lines.len()
                {
                    j += 1;
                    sig.push(' ');
                    sig.push_str(lines[j].trim());
                }
                i = j + 1;
                if !sig.contains("&mut Scene") {
                    continue;
                }
                // 只有公开面算违规：`pub` 前缀，或 trait 里的方法声明（以 `;` 结尾）。
                let public = t.starts_with("pub") || sig.trim_end().ends_with(';');
                if !public {
                    continue;
                }
                let key = (path.clone(), t.to_string());
                match allow.get(&key) {
                    Some((limit, _)) => {
                        let n = used.entry(key).or_insert(0);
                        *n += 1;
                        if *n > *limit {
                            bad.push(format!(
                                "规则2 {path}:{} 同一签名出现 {} 次，白名单只登记 {limit} 次",
                                i - 1,
                                n
                            ));
                        }
                    }
                    None => bad.push(format!(
                        "规则2 {}:{} 公开 API 泄漏可变场景借用：{}",
                        path,
                        i - 1,
                        t
                    )),
                }
            }
        }

        // ---- 规则 6：write 闭包体内不得再借用场景载体
        let clean = strip_noise(&text);
        let mut from = 0usize;
        while let Some(off) = clean[from..].find(".write(") {
            let start = from + off;
            from = start + 1;
            let Some(body) = paren_body(&clean, start + ".write(".len() - 1) else {
                continue;
            };
            if !body.trim_start().starts_with('|') {
                continue;
            }
            for key in &scene_keys {
                let hit = body.contains(&format!("{key}.borrow()"))
                    || body.contains(&format!("{key}.borrow_mut()"));
                if hit {
                    let line = clean[..start].lines().count();
                    bad.push(format!(
                        "规则6 {path}:{line} write 闭包体内又借用了 `{key}`（RefCell 重入）"
                    ));
                    break;
                }
            }
        }
    }

    // ---- 规则 5：每个 crate 都要在 dep_rules.json 里登记
    let rules = read_json(&root.join("xtask/dep_rules.json"));
    let mut crates = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("crates")) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("Cargo.toml").exists() {
                if let Some(n) = p.file_name() {
                    crates.push(n.to_string_lossy().to_string());
                }
            }
        }
    }
    crates.sort();
    for name in &crates {
        if rules["crates"].get(name).is_none() {
            bad.push(format!("规则5 crate `{name}` 未登记 xtask/dep_rules.json"));
        }
    }

    // ---- 白名单对不上就是债没还完：只许减
    for (key, (limit, note)) in &allow {
        let got = used.get(key).copied().unwrap_or(0);
        if got < *limit {
            let tail = if note.is_empty() {
                String::new()
            } else {
                format!("（{note}）")
            };
            println!(
                "  可下调: {} `{}` 登记 {limit} 次，实际 {got} 次{tail}",
                key.0, key.1
            );
        }
    }

    // ---- 规则 7：写点台账只许减（docs/WRITE_SITES.md，机读列：crate / 登记数）
    let ledger_text = fs::read_to_string(root.join("docs/WRITE_SITES.md")).unwrap_or_default();
    let mut ledger: BTreeMap<String, usize> = BTreeMap::new();
    for line in ledger_text.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = t.trim_matches('|').split('|').map(|c| c.trim()).collect();
        if cols.len() < 2 || !cols[0].starts_with("r3e-") {
            continue;
        }
        if let Ok(count) = cols[1].parse::<usize>() {
            ledger.insert(cols[0].to_string(), count);
        }
    }
    if ledger.is_empty() {
        bad.push(
            "规则7 docs/WRITE_SITES.md 台账解析为空（表头应为 `| crate | 登记写点数 | 说明 |`）"
                .into(),
        );
    }
    for (name, actual) in &write_sites {
        match ledger.get(name) {
            Some(&reg) if *actual > reg => bad.push(format!(
                "规则7 `{name}` 有 {actual} 处 .write(，台账只登记 {reg} 处：新增写点必须先在 docs/WRITE_SITES.md 登记并写明理由"
            )),
            Some(&reg) if *actual < reg => {
                println!("  可下调: 写点 {name} 登记 {reg} 处，实际 {actual} 处")
            }
            Some(_) => {}
            None => bad.push(format!(
                "规则7 `{name}` 有 {actual} 处 .write(，但台账没有该 crate 的登记行"
            )),
        }
    }
    for name in ledger.keys() {
        if !write_sites.contains_key(name) {
            println!("  可下调: 台账里的 `{name}` 已无写点，可整行删除");
        }
    }

    println!(
        "[doc-gate] 扫描 {checked} 个 .rs，{} 个 crate，规则 7 条，白名单 {} 项，写点台账 {} 个 crate / {} 处",
        crates.len(),
        allow.len(),
        ledger.len(),
        write_sites.values().sum::<usize>()
    );
    if bad.is_empty() {
        println!("[doc-gate] ok");
        0
    } else {
        for b in &bad {
            println!("  {b}");
        }
        println!("[doc-gate] FAIL：{} 条违规", bad.len());
        1
    }
}

/// 从 `(`（`open` 指向它的字节偏移）开始配平，返回括号内部文本。
fn paren_body(s: &str, open: usize) -> Option<String> {
    let start = s[..open.min(s.len())].chars().count();
    let mut depth = 0i32;
    let mut out = String::new();
    for (k, c) in s.chars().enumerate().skip(start) {
        if k == start {
            if c != '(' {
                return None;
            }
            depth = 1;
            continue;
        }
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                out.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(out);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    None
}

/// 去掉行注释与字符串字面量：注释里写 `.borrow()` 不该误报。
fn strip_noise(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if chars[i] == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            out.push_str("\"\"");
            i = j + 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}
