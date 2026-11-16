//! 能力清单（`docs/capabilities.json`）的数据结构、排序规则与序列化入口（M0-S5 / S5-02）。
//!
//! 清单本身**不在这里手写**：它由 `cargo xtask gen-capabilities` 从三源交叉生成
//! （`r3e_core::cmd_spec::all()` + `r3e_doc::ISSUE_CODES` + [`RPC_METHODS`]），本模块只负责
//! 「长什么样」与「怎么稳定地序列化」。这样能力声明永远跟着代码走，`--check` 能拦住漂移。
//!
//! # 稳定性约定（对外契约，改动需 `schema++`）
//!
//! 1. `commands` 按 `name` 字典序、`invariants` 按 `code`、`unsupported` 按 `item`；
//!    `domains` 保持声明序（即 `Domain` 枚举序），**不排序**；
//! 2. 字段顺序固定（结构体声明序），`rpc: null` 表示「仅脚本可用」，不省略键；
//! 3. 序列化统一 2 空格缩进 + 末尾换行，保证 `git diff` 逐行可读、`--check` 可逐字节比对。

use serde::Serialize;

use crate::RPC_METHODS;

/// 清单 schema 版本。字段增删 / 语义变更即 +1，消费方（ai_ide）需同步。
pub const SCHEMA_VERSION: u32 = 1;
/// 生成方式（写进清单，便于人工确认文件来源）。
pub const GENERATED_BY: &str = "cargo xtask gen-capabilities";
/// 引擎名。
pub const ENGINE_NAME: &str = "r3e";

/// 各类条目的计数汇总。
///
/// 冗余但有意的设计：它是「一眼可核」的指纹，CI 与人工评审都拿它对数，
/// 出现 369/39/330/17/24 之外的数字就说明命令表或校验器被改过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// 命令总数（脚本函数 + 仅 RPC）。
    pub commands: usize,
    /// 其中对外暴露 RPC 的条数。
    pub rpc: usize,
    /// 其中只有脚本可用（`rpc = null`）的条数。
    pub script_only: usize,
    /// 能力域个数。
    pub domains: usize,
    /// 不变量（issue code）条数。
    pub invariants: usize,
}

/// 一条命令的对外能力描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandCap {
    /// 脚本函数名；`meta` 域条目为点分 RPC 名。
    pub name: String,
    /// 所属能力域（`Domain` 的 snake_case 名）。
    pub domain: String,
    /// 对外 RPC 方法名；`None` = 仅脚本可用。
    pub rpc: Option<String>,
    /// 参数类型序列（重载以 `|` 分隔各签名）。
    pub params: Vec<String>,
    /// 返回类型（重载以 `/` 连接）。
    pub returns: String,
    /// 是否无持久副作用或可撤销。
    pub reversible: bool,
    /// 代价量级：`cheap` / `linear` / `heavy`。
    pub cost: String,
    /// 进入命令表的里程碑。
    pub since: String,
}

/// 一条不变量的对外描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvariantCap {
    /// 机器可读 code。
    pub code: String,
    /// 上报该 code 的校验层（`bounds` / `refs`）。
    pub layer: String,
    /// 严重度（`error` / `warn`）。
    pub severity: String,
}

/// 明确「不支持 / 有条件支持」的条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsupported {
    /// 条目名（点分）。
    pub item: String,
    /// 为什么不支持 / 什么条件下支持。
    pub reason: String,
    /// 预计处理的里程碑。
    pub since: String,
}

/// 整份能力清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub schema: u32,
    pub generated_by: String,
    pub engine: String,
    pub protocol_version: u32,
    pub counts: Counts,
    pub domains: Vec<String>,
    pub commands: Vec<CommandCap>,
    pub invariants: Vec<InvariantCap>,
    pub unsupported: Vec<Unsupported>,
}

impl Capabilities {
    /// 组装清单：内部完成排序与计数，调用方只喂原始数据。
    ///
    /// 计数一律**现算**而非让调用方传：清单里的 `counts` 与明细不可能对不上。
    pub fn build(
        protocol_version: u32,
        domains: Vec<String>,
        mut commands: Vec<CommandCap>,
        mut invariants: Vec<InvariantCap>,
        mut unsupported: Vec<Unsupported>,
    ) -> Self {
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        invariants.sort_by(|a, b| a.code.cmp(&b.code));
        unsupported.sort_by(|a, b| a.item.cmp(&b.item));

        let rpc = commands.iter().filter(|c| c.rpc.is_some()).count();
        let counts = Counts {
            commands: commands.len(),
            rpc,
            script_only: commands.len() - rpc,
            domains: domains.len(),
            invariants: invariants.len(),
        };

        Self {
            schema: SCHEMA_VERSION,
            generated_by: GENERATED_BY.to_string(),
            engine: ENGINE_NAME.to_string(),
            protocol_version,
            counts,
            domains,
            commands,
            invariants,
            unsupported,
        }
    }

    /// 序列化为落盘文本（2 空格缩进 + 末尾换行）。
    ///
    /// 走 `to_string_pretty` 而不是自己拼字符串：格式化规则交给 serde_json，
    /// 换版本时也不会出现缩进风格漂移。
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut out = serde_json::to_string_pretty(self)?;
        out.push('\n');
        Ok(out)
    }

    /// 校验清单与三源的一致性；返回全部问题（空 = 一致）。
    ///
    /// 生成器在写盘前调用它，`--check` 模式则用于兜住「源改了但没重生成」。
    pub fn check_self(&self) -> Vec<String> {
        let mut problems = Vec::new();

        let mut sorted = self.commands.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        if sorted != self.commands {
            problems.push("commands 未按 name 字典序排列".to_string());
        }

        let mut inv = self.invariants.clone();
        inv.sort_by(|a, b| a.code.cmp(&b.code));
        if inv != self.invariants {
            problems.push("invariants 未按 code 字典序排列".to_string());
        }

        if self.counts.commands != self.commands.len()
            || self.counts.invariants != self.invariants.len()
            || self.counts.domains != self.domains.len()
        {
            problems.push("counts 与明细分组不一致".to_string());
        }

        let rpc: Vec<&str> = self
            .commands
            .iter()
            .filter_map(|c| c.rpc.as_deref())
            .collect();
        for m in RPC_METHODS {
            if !rpc.contains(m) {
                problems.push(format!("命令表缺少 RPC 方法：{m}"));
            }
        }
        for m in &rpc {
            if !RPC_METHODS.contains(m) {
                problems.push(format!("RPC 方法未登记在 RPC_METHODS：{m}"));
            }
        }

        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, rpc: Option<&str>, domain: &str) -> CommandCap {
        CommandCap {
            name: name.to_string(),
            domain: domain.to_string(),
            rpc: rpc.map(str::to_string),
            params: Vec::new(),
            returns: "()".to_string(),
            reversible: true,
            cost: "cheap".to_string(),
            since: "M0-backfill".to_string(),
        }
    }

    #[test]
    fn build按字典序排命令并现算计数() {
        let caps = Capabilities::build(
            crate::PROTOCOL_VERSION,
            vec!["scene".to_string()],
            vec![
                cmd("b.cmd", None, "scene"),
                cmd("a.cmd", Some("a.cmd"), "scene"),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(caps.commands[0].name, "a.cmd");
        assert_eq!(caps.commands[1].name, "b.cmd");
        assert_eq!(caps.counts.commands, 2);
        assert_eq!(caps.counts.rpc, 1);
        assert_eq!(caps.counts.script_only, 1);
        assert_eq!(caps.counts.domains, 1);
    }

    #[test]
    fn 序列化两空格缩进且以换行结尾() {
        let caps = Capabilities::build(
            crate::PROTOCOL_VERSION,
            vec!["scene".to_string()],
            vec![cmd("a.cmd", Some("a.cmd"), "scene")],
            Vec::new(),
            Vec::new(),
        );
        let text = caps.to_json().expect("序列化必须成功");
        assert!(text.ends_with('\n'));
        assert!(text.contains("\n  \"schema\": 1,"));
        assert!(text.contains("\"rpc\": \"a.cmd\""));
    }

    #[test]
    fn 仅脚本命令的rpc字段序列化为null() {
        let caps = Capabilities::build(
            crate::PROTOCOL_VERSION,
            Vec::new(),
            vec![cmd("a.cmd", None, "scene")],
            Vec::new(),
            Vec::new(),
        );
        let text = caps.to_json().expect("序列化必须成功");
        assert!(text.contains("\"rpc\": null"), "rpc 键不许省略：{text}");
    }

    #[test]
    fn check_self能发现未登记的rpc() {
        let caps = Capabilities::build(
            crate::PROTOCOL_VERSION,
            Vec::new(),
            vec![cmd("ghost.method", Some("ghost.method"), "meta")],
            Vec::new(),
            Vec::new(),
        );
        let problems = caps.check_self();
        assert!(
            problems.iter().any(|p| p.contains("RPC_METHODS")),
            "未登记的 rpc 必须被报出来：{problems:?}"
        );
    }
}
