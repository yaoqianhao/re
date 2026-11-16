//! xtask：仓库级工程门禁。
//!
//! 这些检查原本散在 CI 的 yaml 与人的记忆里，现在收敛成一条命令：
//!
//! - `cargo xtask verify`：fmt + clippy + test + 下面三项自查，任一失败即非 0 退出
//! - `cargo xtask lint-size`：单文件行数不许超线（白名单只许减不许加）
//! - `cargo xtask check-tests`：每个 crate 的单测数不许低于基线快照
//! - `cargo xtask dep-check`：crate 间依赖方向门禁（默认严格；存量债见 `xtask/dep_debt.json`，只减不增）
//! - `cargo xtask doc-gate`：文档层收口六条规则（M0-S3 的 doc.write 唯一写入口不许回退）
//!
//! 设计约束：不引第三方 CLI 框架、不做花哨输出。门禁本身越简单越不容易坏。

mod doc_gate;
mod gen_capabilities;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USAGE: &str = "\
用法: cargo xtask <子命令>

子命令:
  verify             fmt --check + clippy -D warnings + test + lint-size + check-tests + doc-gate + dep-check
  lint-size          单文件行数门禁（白名单只许减不许加）
  check-tests        单测数不得低于 xtask/tests_baseline.json
  dep-check          crate 依赖方向门禁（默认严格；--report 只报告。债务见 xtask/dep_debt.json）
  doc-gate           文档层收口六条规则（RefCell<Scene> / &mut Scene / DerefMut / 解析入口 / 依赖登记 / 闭包重入）
  gen-capabilities   生成 docs/capabilities.json（三源交叉；--check 只校验不写盘）
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_else(|| "help".to_string());
    // dep-check 自 M0-S5 起默认严格；`--report` 显式降级为只报告（`--strict` 保留为同义写法）。
    let strict = !args.iter().any(|a| a == "--report");
    let check = args.iter().any(|a| a == "--check");
    let code = match cmd.as_str() {
        "verify" => verify(),
        "lint-size" => lint_size(),
        "check-tests" => check_tests(),
        "dep-check" => dep_check(strict),
        "replay" => replay(),
        "doc-gate" => doc_gate::run(&root()),
        "gen-capabilities" => gen_capabilities::run(&root(), check),
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
    if replay() != 0 {
        failed.push("replay".into());
    }
    if lint_size() != 0 {
        failed.push("lint-size".into());
    }
    if check_tests() != 0 {
        failed.push("check-tests".into());
    }
    if doc_gate::run(&root) != 0 {
        failed.push("doc-gate".into());
    }
    // dep-check 自 M0-S5 起是硬门禁：白名单外违规、债务新增、债务残留都红。
    if dep_check(true) != 0 {
        failed.push("dep-check".into());
    }

    if failed.is_empty() {
        println!("\n[verify] 全部通过");
        0
    } else {
        println!("\n[verify] 失败项：{}", failed.join(", "));
        1
    }
}

// ---------------------------------------------------------------- replay

/// 回放参数（与 `crates/r3e-replay` 的默认值保持一致）。
const REPLAY_ARGS: [&str; 7] = ["--seed", "42", "--frames", "600", "--hash-every", "60", ""];

/// 确定性回放门禁（S4-06）：同一组参数跑**两次**，逐行比对 stdout。
///
/// 为什么不用 `--frames 60` 之类的小值：600 帧覆盖 60 笔事务、60 个节点、
/// 540 条随机命令，短回放抓不到「累积到某帧才开始分叉」的问题。
///
/// 为什么捕获而不是直接继承输出：两次运行的输出要能逐行对比，且失败时要能
/// 打印「第一个不一致的行 + 两边的原文」，继承输出只能让人自己肉眼比对。
fn replay() -> i32 {
    let root = root();
    println!("[replay] cargo build -p r3e-replay --bin r3e-replay");
    if !run_cargo(&root, &["build", "-p", "r3e-replay", "--bin", "r3e-replay"]) {
        println!("[replay] 构建失败");
        return 1;
    }

    let args: Vec<&str> = REPLAY_ARGS
        .iter()
        .copied()
        .filter(|a| !a.is_empty())
        .collect();
    let run_once = || {
        Command::new("cargo")
            .args(["run", "-q", "-p", "r3e-replay", "--bin", "r3e-replay", "--"])
            .args(&args)
            .current_dir(&root)
            .output()
    };

    let first = match run_once() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            println!(
                "[replay] 第 1 次运行失败：{}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return 1;
        }
        Err(e) => {
            println!("[replay] 无法启动回放器：{e}");
            return 1;
        }
    };
    let second = match run_once() {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            println!(
                "[replay] 第 2 次运行失败：{}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return 1;
        }
        Err(e) => {
            println!("[replay] 无法启动回放器：{e}");
            return 1;
        }
    };

    let a = String::from_utf8_lossy(&first.stdout);
    let b = String::from_utf8_lossy(&second.stdout);
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();

    for (i, (x, y)) in la.iter().zip(lb.iter()).enumerate() {
        if x != y {
            println!("[replay] 第 {} 行不一致", i + 1);
            println!("  第 1 次: {x}");
            println!("  第 2 次: {y}");
            return 1;
        }
    }
    if la.len() != lb.len() {
        println!("[replay] 行数不一致：{} vs {}", la.len(), lb.len());
        return 1;
    }

    let samples = la.iter().filter(|l| l.starts_with("frame ")).count();
    let final_line = la.last().copied().unwrap_or("<空>");
    let final_hash = final_line
        .rsplit_once("final=")
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| "<缺失>".to_string());
    println!("[replay] 600 帧 / {samples} 个采样点 / 两次运行哈希完全一致（final={final_hash}）");
    0
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

/// 依赖方向门禁（M0-S5 起为硬门禁，默认严格）。
///
/// 四种判定，任一命中即失败（`--report` 可降级为只报告，verify 与 CI 永远用严格模式）：
/// 1. 白名单外违规：依赖了 `dep_rules.json` 不允许的引擎 crate，且没登记在 `dep_debt.json`；
/// 2. 债务残留：`dep_debt.json` 里的边在代码里已经不存在（或已被规则放开）——债还了就得销账；
/// 3. 债务超限：条目数超过 `limit`，只许减不许增；
/// 4. 规则缺失：`crates/` 下有 crate 没登记规则，会被静默放行。
fn dep_check(strict: bool) -> i32 {
    let root = root();
    let rules = read_json(&root.join("xtask/dep_rules.json"));
    let debt = read_json(&root.join("xtask/dep_debt.json"));
    let debt_limit = debt["limit"].as_u64().unwrap_or(0) as usize;

    // 债务清单只需要键做判定，plan 用于把「为什么还欠着」直接打在输出里。
    let mut debt_edges: Vec<(String, String)> = Vec::new();
    let mut debt_plan: BTreeMap<String, String> = BTreeMap::new();
    if let Some(entries) = debt["entries"].as_array() {
        for e in entries {
            let c = e["crate"].as_str().unwrap_or("").trim().to_string();
            let d = e["dep"].as_str().unwrap_or("").trim().to_string();
            if c.is_empty() || d.is_empty() {
                println!("  dep_debt.json 有条目缺 crate/dep 字段：{e}");
                continue;
            }
            if debt_edges.iter().any(|(ec, ed)| *ec == c && *ed == d) {
                continue;
            }
            debt_plan.insert(
                format!("{c} -> {d}"),
                e["plan"].as_str().unwrap_or("").to_string(),
            );
            debt_edges.push((c, d));
        }
    }

    let engine: Vec<String> = crate_dirs(&root)
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();

    // 违规逐边记录（crate, dep），聚合只在输出层做，判定要精确到边。
    let mut violations: Vec<(String, String)> = Vec::new();
    let mut notes: BTreeMap<String, String> = BTreeMap::new();
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
        notes.insert(
            name.clone(),
            rule["note"].as_str().unwrap_or("").to_string(),
        );

        let (prod, _dev) = crate_deps(&dir.join("Cargo.toml"));
        for dep in prod {
            if !engine.contains(&dep) || dep == name {
                continue;
            }
            if !allow.contains(&dep) && forbid_all {
                violations.push((name.clone(), dep));
            }
        }
    }
    violations.sort();
    violations.dedup();

    let in_debt = |c: &str, d: &str| debt_edges.iter().any(|(ec, ed)| ec == c && ed == d);
    let uncovered: Vec<&(String, String)> =
        violations.iter().filter(|(c, d)| !in_debt(c, d)).collect();
    let covered: Vec<&(String, String)> =
        violations.iter().filter(|(c, d)| in_debt(c, d)).collect();
    let stale: Vec<&(String, String)> = debt_edges
        .iter()
        .filter(|(c, d)| !violations.iter().any(|(vc, vd)| vc == c && vd == d))
        .collect();

    println!(
        "[dep-check] 规则表 {} 个 crate，扫描 {} 个引擎 crate；债务白名单 {} 条（上限 {} 条）",
        rules["crates"].as_object().map(|m| m.len()).unwrap_or(0),
        engine.len(),
        debt_edges.len(),
        debt_limit
    );
    println!(
        "[dep-check] 违规 {} 条：白名单外 {} 条，债务内 {} 条",
        violations.len(),
        uncovered.len(),
        covered.len()
    );
    for (c, d) in &violations {
        if in_debt(c, d) {
            println!("  债务 {c} -> {d}");
            if let Some(p) = debt_plan.get(&format!("{c} -> {d}")) {
                if !p.is_empty() {
                    println!("      偿还计划：{p}");
                }
            }
        } else {
            println!(
                "  违规 {c} -> {d}（未登记；规则：{}）",
                notes.get(c).map(String::as_str).unwrap_or("")
            );
        }
    }
    for (c, d) in &stale {
        println!("  残留 {c} -> {d}：这条已在代码里消失（或规则已放开），请从 dep_debt.json 删掉");
    }

    let mut fail = false;
    if !uncovered.is_empty() {
        println!(
            "  × 判定 1 白名单外违规 {} 条：改依赖方向，或在 dep_debt.json 登记 why/plan（条数只减不增）",
            uncovered.len()
        );
        fail = true;
    }
    if !stale.is_empty() {
        println!(
            "  × 判定 2 债务残留 {} 条：还清了就得删条，否则免死金牌会留着发霉",
            stale.len()
        );
        fail = true;
    }
    if debt_edges.len() > debt_limit {
        println!(
            "  × 判定 3 债务超限 {} > {}：只许减不许增，真加了债请同步下调 limit 之外的理由",
            debt_edges.len(),
            debt_limit
        );
        fail = true;
    }
    if !missing_rule.is_empty() {
        println!("  × 判定 4 未登记规则的 crate：{}", missing_rule.join(", "));
        fail = true;
    }

    if fail {
        if strict {
            println!("[dep-check] FAIL（严格模式）");
            return 1;
        }
        println!("[dep-check] 发现问题；--report 模式不阻塞，但 verify / CI 会红");
        return 0;
    }
    println!(
        "[dep-check] ok（白名单外 0 条，债务 {} 条，全部已登记）",
        covered.len()
    );
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
