//! 契约：脚本侧 `register_fn` 与 `r3e_core::cmd_spec` 命令表必须一一对应。
//!
//! 命令表是「登记簿」，脚本侧 `bind_all` 仍是手写真实现。两边任何一侧单方面改名，
//! 这里就该红：
//! - 注册了表里没有的命令 → 新命令没登记，宿主 / 文档 / 事务都看不见它；
//! - 表里有但没注册 → 表在骗人，脚本调不到。
//!
//! 为什么扫源码而不是问 rhai：`Engine::gen_fn_signatures` 要 `metadata` feature，
//! 本工程只开了 `serde`；开 feature 会拉长编译并影响下游，性价比低于编译期包含源码。
//! `include_str!` 把源码钉进测试二进制，等价于「与被测代码同版本」，不是运行时读盘。

use std::collections::{BTreeMap, BTreeSet};

use r3e_core::cmd_spec;

/// 域文件 → 源码（顺序与 `crates/r3e-script/src/api/` 一致，新增域文件必须加进来）。
const SOURCES: &[(&str, &str)] = &[
    ("anim_blend", include_str!("../src/api/anim_blend.rs")),
    ("anim_morph", include_str!("../src/api/anim_morph.rs")),
    ("anim_sync", include_str!("../src/api/anim_sync.rs")),
    ("audio", include_str!("../src/api/audio.rs")),
    ("basic", include_str!("../src/api/basic.rs")),
    ("env", include_str!("../src/api/env.rs")),
    ("input", include_str!("../src/api/input.rs")),
    ("light", include_str!("../src/api/light.rs")),
    ("physics", include_str!("../src/api/physics.rs")),
    ("pick", include_str!("../src/api/pick.rs")),
    ("postfx", include_str!("../src/api/postfx.rs")),
    ("query", include_str!("../src/api/query.rs")),
    ("scene_flow", include_str!("../src/api/scene_flow.rs")),
    ("shadow", include_str!("../src/api/shadow.rs")),
    ("text", include_str!("../src/api/text.rs")),
    ("ui", include_str!("../src/api/ui.rs")),
];

/// 取出源码里 `register_fn("名字", ...)` 的全部名字（按出现顺序，不去重）。
fn registered_names(src: &str) -> Vec<&str> {
    const NEEDLE: &str = "register_fn(";
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(hit) = rest.find(NEEDLE) {
        let after = &rest[hit + NEEDLE.len()..];
        let trimmed = after.trim_start();
        let trimmed = match trimmed.strip_prefix("move") {
            Some(t) => t.trim_start(),
            None => trimmed,
        };
        if let Some(quoted) = trimmed.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                out.push(&quoted[..end]);
            }
        }
        rest = after;
    }
    out
}

fn source_registrations() -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_domain, src) in SOURCES {
        for name in registered_names(src) {
            *counts.entry(name).or_default() += 1;
        }
    }
    counts
}

#[test]
fn 每个注册的命令都在表里登记了() {
    let registered: BTreeSet<&str> = source_registrations().keys().copied().collect();
    let declared: BTreeSet<&str> = cmd_spec::script_names().collect();

    let missing: Vec<&&str> = registered.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "这些命令在脚本侧注册了、命令表里却没有：{missing:?}\n\
         新增命令时请同步在 `crates/r3e-core/src/cmd_spec/` 对应域文件里补一条 CommandSpec"
    );
}

#[test]
fn 表里的每条脚本命令都真的注册了() {
    let registered: BTreeSet<&str> = source_registrations().keys().copied().collect();
    let declared: BTreeSet<&str> = cmd_spec::script_names().collect();

    let phantom: Vec<&&str> = declared.difference(&registered).collect();
    assert!(
        phantom.is_empty(),
        "命令表里登记了、脚本侧根本没注册（表在骗人）：{phantom:?}"
    );
}

#[test]
fn 统计与测绘口径一致() {
    let counts = source_registrations();
    let total: usize = counts.values().sum();
    // 测绘结论：16 个域文件共 364 次注册、353 个唯一名字（11 处重载）。
    // 数字变了说明有人加了命令却没更新 `docs/M0S2_API_SPLIT.md` 的口径表。
    assert_eq!(counts.len(), 353, "唯一命令数应为 353（M0 测绘口径）");
    assert_eq!(total, 364, "注册次数应为 364（含 11 处重载）");
    assert_eq!(
        cmd_spec::script_names().count(),
        353,
        "命令表脚本条目数应与注册唯一名一致"
    );
}

#[test]
fn 域文件切分与域映射能对上() {
    // 每个域文件都必须有命令登记在自己的 Domain 上，别把整域错挂到 Scene。
    for (domain, src) in SOURCES {
        let names: BTreeSet<&str> = registered_names(src).into_iter().collect();
        assert!(
            !names.is_empty(),
            "域文件 `{domain}` 里没扫到任何 register_fn"
        );
        for name in names {
            let spec = cmd_spec::by_name(name).unwrap_or_else(|| {
                panic!("`{name}`（域文件 {domain}）在表里查不到");
            });
            assert!(
                spec.domain != r3e_core::cmd_spec::Domain::Meta,
                "`{name}`（域文件 {domain}）在表里被挂到了 Meta 域，脚本命令不该进 Meta"
            );
        }
    }
}
