//! `cargo xtask gen-capabilities`：从三源交叉生成 `docs/capabilities.json`（M0-S5 / S5-02）。
//!
//! 三源与各自职责：
//!
//! | 源 | 提供 |
//! |---|---|
//! | `r3e_core::cmd_spec::all()` | `commands[]`（name / domain / rpc / params / returns / reversible / cost / since） |
//! | `r3e_doc::ISSUE_CODES` | `invariants[]`（code / layer / severity） |
//! | `r3e_protocol::RPC_METHODS` | 对外 RPC 方法集的权威列表，用于与命令表 `rpc` 字段互校 |
//!
//! 生成时**立即校验**（任一不成立即非 0 退出，绝不写出半成品清单）：
//!
//! - 命令总数 == [`EXPECTED_COMMANDS`]、RPC 数 == [`r3e_protocol::RPC_METHODS`]`.len()`
//! - 命令表 `rpc` 集合与 `RPC_METHODS` 完全相等（多一条 / 少一条都红）
//! - 能力域集合与声明序一致（17 域，顺序即 `Domain` 枚举序）
//! - 不变量条数 == `ISSUE_CODES.len()`
//!
//! `--check` 模式：重新在内存里生成，与仓库内文件逐字节比对；不一致打印首个差异位置并非 0。
//! 用于 CI 拦「改了命令表却没重生成清单」这类静默漂移。

use std::fs;
use std::path::Path;

use r3e_core::cmd_spec::{self, Cost, Domain};
use r3e_doc::ISSUE_CODES;
use r3e_protocol::caps::{Capabilities, CommandCap, InvariantCap, Unsupported};
use r3e_protocol::{PROTOCOL_VERSION, RPC_METHODS};

/// 命令表应有条数（369 = 39 仅 RPC + 330 脚本独有）。
///
/// 故意硬编码：加命令是**有意识**的动作，改完必须同时更新本常量与
/// `docs/M0S5_PROTOCOL.md` §3 的 counts，避免「悄悄多一条没人知道」。
const EXPECTED_COMMANDS: usize = 369;

/// 能力域声明序（与 [`Domain`] 枚举序一致，也是清单 `domains` 数组的顺序）。
const DOMAIN_ORDER: &[&str] = &[
    "scene",
    "scene_flow",
    "anim",
    "anim_blend",
    "anim_sync",
    "env",
    "render",
    "shadow",
    "ui",
    "text",
    "light",
    "audio",
    "physics",
    "input",
    "query",
    "pick",
    "meta",
];

/// 明确「有条件支持」的条目（人工维护，M1 起逐条收口）。
const UNSUPPORTED: &[(&str, &str, &str)] = &[(
    "physics.backend.rapier",
    "feature 未开启时退回内置积分器，纸面参数与刚体休眠行为不同",
    "M1",
)];

/// 子命令入口。返回进程退出码（0 = 成功）。
pub fn run(root: &Path, check: bool) -> i32 {
    let caps = match build() {
        Ok(caps) => caps,
        Err(problems) => {
            eprintln!(
                "[gen-capabilities] 生成前置校验失败（{0} 项）：",
                problems.len()
            );
            for p in problems {
                eprintln!("  - {p}");
            }
            return 1;
        }
    };

    let text = match caps.to_json() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[gen-capabilities] 序列化失败：{e}");
            return 1;
        }
    };

    let path = root.join("docs/capabilities.json");
    let c = caps.counts;
    println!(
        "[gen-capabilities] commands={} rpc={} script_only={} domains={} invariants={}",
        c.commands, c.rpc, c.script_only, c.domains, c.invariants
    );

    if check {
        let old = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[gen-capabilities] 读不到 {}：{e}", path.display());
                return 1;
            }
        };
        if old == text {
            println!("[gen-capabilities] 清单与代码一致（{}）", path.display());
            return 0;
        }
        eprintln!(
            "[gen-capabilities] 清单漂移：{} 与代码不一致，请运行 `cargo xtask gen-capabilities`",
            path.display()
        );
        print_first_diff(&old, &text);
        return 1;
    }

    if let Err(e) = fs::write(&path, &text) {
        eprintln!("[gen-capabilities] 写入 {} 失败：{e}", path.display());
        return 1;
    }
    println!(
        "[gen-capabilities] 已写入 {}（{} 字节）",
        path.display(),
        text.len()
    );
    0
}

/// 组装并校验清单。返回 `Err(problems)` 表示三源自相矛盾，不得落盘。
fn build() -> Result<Capabilities, Vec<String>> {
    let mut problems = Vec::new();

    let commands: Vec<CommandCap> = cmd_spec::all()
        .map(|spec| CommandCap {
            name: spec.name.to_string(),
            domain: domain_name(spec.domain).to_string(),
            rpc: spec.rpc.map(str::to_string),
            params: spec.params.iter().map(|p| (*p).to_string()).collect(),
            returns: spec.returns.to_string(),
            reversible: spec.reversible,
            cost: cost_name(spec.cost).to_string(),
            since: spec.since.to_string(),
        })
        .collect();

    let invariants: Vec<InvariantCap> = ISSUE_CODES
        .iter()
        .map(|ic| InvariantCap {
            code: ic.code.to_string(),
            layer: ic.layer.to_string(),
            severity: ic.severity.to_string(),
        })
        .collect();

    let unsupported: Vec<Unsupported> = UNSUPPORTED
        .iter()
        .map(|(item, reason, since)| Unsupported {
            item: (*item).to_string(),
            reason: (*reason).to_string(),
            since: (*since).to_string(),
        })
        .collect();

    let caps = Capabilities::build(
        PROTOCOL_VERSION,
        DOMAIN_ORDER.iter().map(|d| (*d).to_string()).collect(),
        commands,
        invariants,
        unsupported,
    );

    // ---- 硬断言：三源必须互相对得上 ----
    if caps.counts.commands != EXPECTED_COMMANDS {
        problems.push(format!(
            "命令表 {} 条，期望 {EXPECTED_COMMANDS} 条（改命令表须同步本常量与 docs/M0S5_PROTOCOL.md §3）",
            caps.counts.commands
        ));
    }
    if caps.counts.rpc != RPC_METHODS.len() {
        problems.push(format!(
            "命令表 rpc 条目 {} 条，RPC_METHODS {} 条",
            caps.counts.rpc,
            RPC_METHODS.len()
        ));
    }

    let mut table_rpc: Vec<&str> = caps
        .commands
        .iter()
        .filter_map(|c| c.rpc.as_deref())
        .collect();
    table_rpc.sort_unstable();
    let mut methods: Vec<&str> = RPC_METHODS.to_vec();
    methods.sort_unstable();
    if table_rpc != methods {
        let missing: Vec<&str> = methods
            .iter()
            .copied()
            .filter(|m| !table_rpc.contains(m))
            .collect();
        let extra: Vec<&str> = table_rpc
            .iter()
            .copied()
            .filter(|m| !methods.contains(m))
            .collect();
        problems.push(format!(
            "命令表 rpc 与 RPC_METHODS 不一致：命令表缺 {missing:?}，RPC_METHODS 缺 {extra:?}"
        ));
    }

    let used_domains: Vec<String> = DOMAIN_ORDER
        .iter()
        .filter(|d| caps.commands.iter().any(|c| &c.domain == *d))
        .map(|d| (*d).to_string())
        .collect();
    if used_domains.len() != DOMAIN_ORDER.len() {
        let unused: Vec<&str> = DOMAIN_ORDER
            .iter()
            .copied()
            .filter(|d| !used_domains.iter().any(|u| u == d))
            .collect();
        problems.push(format!(
            "以下能力域没有任何命令（枚举多余或漏登记）：{unused:?}"
        ));
    }

    if caps.counts.invariants != ISSUE_CODES.len() {
        problems.push(format!(
            "不变量 {} 条，ISSUE_CODES {} 条",
            caps.counts.invariants,
            ISSUE_CODES.len()
        ));
    }

    problems.extend(caps.check_self());

    if problems.is_empty() {
        Ok(caps)
    } else {
        Err(problems)
    }
}

/// 能力域的 snake_case 名（清单对外形态）。
fn domain_name(domain: Domain) -> &'static str {
    match domain {
        Domain::Scene => "scene",
        Domain::SceneFlow => "scene_flow",
        Domain::Anim => "anim",
        Domain::AnimBlend => "anim_blend",
        Domain::AnimSync => "anim_sync",
        Domain::Env => "env",
        Domain::Render => "render",
        Domain::Shadow => "shadow",
        Domain::Ui => "ui",
        Domain::Text => "text",
        Domain::Light => "light",
        Domain::Audio => "audio",
        Domain::Physics => "physics",
        Domain::Input => "input",
        Domain::Query => "query",
        Domain::Pick => "pick",
        Domain::Meta => "meta",
    }
}

/// 代价量级的小写名（清单对外形态）。
fn cost_name(cost: Cost) -> &'static str {
    match cost {
        Cost::Cheap => "cheap",
        Cost::Linear => "linear",
        Cost::Heavy => "heavy",
    }
}

/// 打印首个差异（行号 + 两侧文本），让 `--check` 红灯可直接定位。
fn print_first_diff(old: &str, new: &str) {
    for (i, (a, b)) in old.lines().zip(new.lines()).enumerate() {
        if a != b {
            eprintln!("  首个差异在第 {} 行：", i + 1);
            eprintln!("    仓库: {a}");
            eprintln!("    生成: {b}");
            return;
        }
    }
    eprintln!(
        "  行内容一致但长度不同：仓库 {} 行 / 生成 {} 行",
        old.lines().count(),
        new.lines().count()
    );
}
