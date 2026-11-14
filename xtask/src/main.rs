//! xtask：仓库级工程门禁。
//!
//! 这些检查原本散在 CI 的 yaml 与人的记忆里，现在收敛成一条命令：
//!
//! - `cargo xtask verify`：fmt + clippy + test + 下面三项自查，任一失败即非 0 退出
//! - `cargo xtask lint-size`：单文件行数不许超线（白名单只许减不许加）
//! - `cargo xtask check-tests`：每个 crate 的单测数不许低于基线快照
//! - `cargo xtask dep-check`：crate 间依赖方向对照规则表（v0 只报告，`--strict` 才失败）
//!
//! 设计约束：不引第三方 CLI 框架、不做花哨输出。门禁本身越简单越不容易坏。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USAGE: &str = "\
用法: cargo xtask <子命令>

子命令:
  verify             fmt --check + clippy -D warnings + test + lint-size + check-tests + dep-check
  lint-size          单文件行数门禁（白名单只许减不许加）
  check-tests        单测数不得低于 xtask/tests_baseline.json
  dep-check          crate 依赖方向对照 xtask/dep_rules.json（v0 只报告，--strict 失败）
  gen-capabilities   能力清单骨架（扫描脚本 API 注册点）
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_else(|| "help".to_string());
    let strict = args.iter().any(|a| a == "--strict");
    let code = match cmd.as_str() {
        "verify" => verify(),
        "lint-size" => lint_size(),
        "check-tests" => check_tests(),
        "dep-check" => dep_check(strict),
        "gen-capabilities" => gen_capabilities(),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            0
        }
        other => {
            eprintln!("未知子命令：{other}\n");
            print!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------- 基础设施

/// 仓库根目录：xtask 的 manifest 目录的父目录。
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 必须在仓库根下的 xtask/ 目录")
        .to_path_buf()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read_json(path: &Path) -> serde_json::Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", path.display()))
}

fn line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

/// 收集 `crates/` 下所有 .rs 文件（不含 target）。
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n == "target").unwrap_or(false) {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn crate_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("crates")) else {
        return v;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("Cargo.toml").exists() {
            v.push(p);
        }
    }
    v.sort();
    v
}

fn run_cargo(root: &Path, args: &[&str]) -> bool {
    println!("[verify] cargo {}", args.join(" "));
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    matches!(status, Ok(s) if s.success())
}

// ---------------------------------------------------------------- verify

fn verify() -> i32 {
    let root = root();
    let mut failed: Vec<String> = Vec::new();

    if !run_cargo(&root, &["fmt", "--all", "--", "--check"]) {
        failed.push("cargo fmt --check".into());
    }
    if !run_cargo(
        &root,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    ) {
        failed.push("cargo clippy".into());
    }
    if !run_cargo(&root, &["test", "--workspace"]) {
        failed.push("cargo test".into());
    }
    if lint_size() != 0 {
        failed.push("lint-size".into());
    }
    if check_tests() != 0 {
        failed.push("check-tests".into());
    }
    // dep-check v0 只报告：债务还没有还完，阻塞会让人习惯性绕过门禁。
    dep_check(false);

    if failed.is_empty() {
        println!("\n[verify] 全部通过");
        0
    } else {
        println!("\n[verify] 失败项：{}", failed.join(", "));
        1
    }
}

// ---------------------------------------------------------------- lint-size

fn lint_size() -> i32 {
    let root = root();
    let debt = read_json(&root.join("xtask/size_debt.json"));
    let limit = debt["limit"].as_u64().unwrap_or(800) as usize;
    let registered: BTreeMap<String, usize> = debt["files"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n as usize)))
                .collect()
        })
        .unwrap_or_default();

    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    files.sort();

    let mut over_limit: Vec<(String, usize)> = Vec::new();
    let mut grew: Vec<(String, usize, usize)> = Vec::new();
    let mut shrank: Vec<(String, usize, usize)> = Vec::new();
    let mut seen = 0usize;

    for f in &files {
        let n = line_count(f);
        let key = rel(&root, f);
        match registered.get(&key) {
            Some(&base) => {
                seen += 1;
                if n > base {
                    grew.push((key, base, n));
                } else if n < base {
                    shrank.push((key, base, n));
                }
            }
            None => {
                if n > limit {
                    over_limit.push((key, n));
                }
            }
        }
    }

    println!(
        "[lint-size] 扫描 {} 个 .rs，阈值 {limit} 行，债务白名单 {} 项",
        files.len(),
        registered.len()
    );
    for (f, n) in &over_limit {
        println!("  超出阈值: {f} {n} 行（> {limit}）");
    }
    for (f, base, n) in &grew {
        println!("  债务上涨: {f} {base} -> {n} 行");
    }
    for (f, base, n) in &shrank {
        println!("  债务下降（可更新白名单）: {f} {base} -> {n} 行");
    }
    let missing: Vec<&String> = registered
        .keys()
        .filter(|k| !root.join(k.as_str()).exists())
        .collect();
    for f in missing {
        println!("  白名单条目已不存在（可删）: {f}");
    }

    if !over_limit.is_empty() || !grew.is_empty() {
        println!("[lint-size] FAIL：不得新增超大文件，也不得让已有大文件继续变大");
        return 1;
    }
    println!("[lint-size] ok（白名单命中 {seen} 项，全部未上涨）");
    0
}

// ---------------------------------------------------------------- check-tests

fn check_tests() -> i32 {
    let root = root();
    let baseline = read_json(&root.join("xtask/tests_baseline.json"));
    let expected: BTreeMap<String, usize> = baseline["crates"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n as usize)))
                .collect()
        })
        .unwrap_or_default();

    let mut actual: BTreeMap<String, usize> = BTreeMap::new();
    let mut files = Vec::new();
    for dir in crate_dirs(&root) {
        rust_files(&dir, &mut files);
    }
    files.sort();
    for f in &files {
        let Ok(text) = fs::read_to_string(f) else {
            continue;
        };
        let key = rel(&root, f);
        // crates/<crate>/...
        let crate_name = key.split('/').nth(1).unwrap_or("?").to_string();
        let n = text.matches("#[test]").count();
        *actual.entry(crate_name).or_insert(0) += n;
    }

    let mut bad = false;
    println!("[check-tests] crate 单测数（基线 -> 实际）");
    for (crate_name, base) in &expected {
        let now = actual.get(crate_name).copied().unwrap_or(0);
        let mark = if now < *base {
            bad = true;
            "  <-- 低于基线"
        } else if now > *base {
            "  (涨)"
        } else {
            ""
        };
        println!("  {crate_name:<16} {base:>4} -> {now:>4}{mark}");
    }
    for (crate_name, now) in &actual {
        if !expected.contains_key(crate_name) {
            println!("  {crate_name:<16}    0 -> {now:>4}  (新 crate，请更新基线)");
        }
    }
    let total_expected: usize = expected.values().sum();
    let total_actual: usize = actual.values().sum();
    println!("  {:<16} {total_expected:>4} -> {total_actual:>4}", "合计");

    if bad || total_actual < total_expected {
        println!("[check-tests] FAIL：单测数不得下降");
        return 1;
    }
    println!("[check-tests] ok");
    0
}

// ---------------------------------------------------------------- dep-check

fn dep_check(strict: bool) -> i32 {
    let root = root();
    let rules = read_json(&root.join("xtask/dep_rules.json"));
    let engine: Vec<String> = crate_dirs(&root)
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();

    // 违规按 crate 聚合：一个 crate 违反方向约束算一条，便于和债务清单对齐。
    let mut violations: Vec<(String, Vec<String>, String)> = Vec::new();
    let mut missing_rule: Vec<String> = Vec::new();

    for dir in crate_dirs(&root) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(rule) = rules["crates"].get(&name) else {
            missing_rule.push(name);
            continue;
        };
        let allow: Vec<String> = rule["allow"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let forbid_all = rule["forbid_all_engine"].as_bool().unwrap_or(true);
        let note = rule["note"].as_str().unwrap_or("").to_string();

        let mut bad: Vec<String> = Vec::new();
        let (prod, _dev) = crate_deps(&dir.join("Cargo.toml"));
        for dep in prod {
            if !engine.contains(&dep) || dep == name {
                continue;
            }
            let allowed = allow.contains(&dep);
            if !allowed && forbid_all {
                bad.push(dep);
            }
        }
        if !bad.is_empty() {
            bad.sort();
            bad.dedup();
            violations.push((name, bad, note));
        }
    }

    println!(
        "[dep-check] 规则表 {} 个 crate",
        rules["crates"].as_object().map(|m| m.len()).unwrap_or(0)
    );
    if !missing_rule.is_empty() {
        println!("  未登记规则的 crate: {}", missing_rule.join(", "));
    }
    if violations.is_empty() {
        println!("[dep-check] 无违规");
    } else {
        let total: usize = violations.iter().map(|(_, d, _)| d.len()).sum();
        println!(
            "[dep-check] 违规 {} 条（涉及 {} 条依赖边）：",
            violations.len(),
            total
        );
        for (name, deps, note) in &violations {
            println!("  {name} -> {}", deps.join(", "));
            if !note.is_empty() {
                println!("      规则：{note}");
            }
        }
        println!("  说明：v0 只报告不阻塞（债务逐 Sprint 偿还），加 --strict 才返回非 0");
    }
    if strict && (!violations.is_empty() || !missing_rule.is_empty()) {
        return 1;
    }
    0
}

/// 读 Cargo.toml 的依赖键名，按用途分开：
/// 返回 (生产依赖, 仅测试依赖)。`[dev-dependencies]` 里的引擎 crate 不计入生产依赖图，
/// 否则薄壳会因为有测试就永远红。
fn crate_deps(manifest: &Path) -> (Vec<String>, Vec<String>) {
    let Ok(text) = fs::read_to_string(manifest) else {
        return (Vec::new(), Vec::new());
    };
    let mut section = String::new();
    let mut prod = Vec::new();
    let mut dev = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            continue;
        }
        let is_dep = matches!(
            section.as_str(),
            "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
        );
        if !is_dep || t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, _)) = t.split_once('=') {
            // 工作区内依赖写作 `r3e-core.workspace = true`，要把 `.workspace` 后缀剥掉，
            // 否则依赖名与 crate 名对不上，门禁会静默放行。
            let name = k.trim().trim_end_matches(".workspace").trim().to_string();
            if section == "[dev-dependencies]" {
                dev.push(name);
            } else {
                prod.push(name);
            }
        }
    }
    (prod, dev)
}

// ---------------------------------------------------------------- gen-capabilities

fn gen_capabilities() -> i32 {
    let root = root();
    let mut files = Vec::new();
    rust_files(&root.join("crates/r3e-script"), &mut files);
    files.sort();

    // 能力清单的真源是脚本注册点：`engine.register_fn("name", ...)` 一类调用。
    // v0 只做计数与清单骨架，等 protocol 落地后再升级为结构化 JSON 输出。
    let mut names: Vec<String> = Vec::new();
    for f in &files {
        let Ok(text) = fs::read_to_string(f) else {
            continue;
        };
        for line in text.lines() {
            let l = line.trim();
            if let Some(idx) = l.find(".register_fn(\"") {
                let rest = &l[idx + ".register_fn(\"".len()..];
                if let Some(end) = rest.find('"') {
                    names.push(rest[..end].to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();

    println!("[gen-capabilities] 扫描 {} 个脚本源文件", files.len());
    println!("  注册点数量: {}", names.len());
    for n in names.iter().take(20) {
        println!("    {n}");
    }
    if names.len() > 20 {
        println!("    ...（其余 {} 项）", names.len() - 20);
    }
    println!("  TODO(M1): 输出 machine-readable capabilities.json，含读写能力与参数签名");
    0
}
