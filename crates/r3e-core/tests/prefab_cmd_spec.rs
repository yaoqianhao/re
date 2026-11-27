//! prefab 命令面的四条「数量与形状」契约（M2-E04 判据 ①～④）。
//!
//! `cmd_spec` 是脚本命令与 RPC 元数据的**单一真源**，因此命令面的判据要在这里锁死；
//! 而「六条命令是不是真的有脚本注册点」只能去 `r3e-script` 的绑定文件里看——本 crate
//! 不允许依赖 `r3e-script`（`xtask/dep_rules.json`），所以判据 ④ 用 `include_str!`
//! 把那个源文件当**文本**读进来核对，而不是调它的 API。这不是取巧：要防的是「命令表
//! 里登记了、脚本侧忘了注册」的死条目，死条目在类型层面看不出来，只能看源码文本。
//!
//! 四条判据的口径（`M2_E_PLAN` §7.3）：
//!
//! 1. `commands == 419`（E03 的 413 + prefab 六条），且六条 `since` 均为 `M2-E`。
//! 2. 六条**全部归 `Domain::Scene`**：prefab 改的是场景装配结果，不新增域，
//!    `domains` 计数保持 18。
//! 3. `rpc` 恰为 `prefab.spawn` + `prefab.info` 两条（`P5` = B），`prefab.define`
//!    的 `rpc` 必须为空——它只走脚本与宿主本地调用。
//! 4. 六条名字都能在脚本绑定里签出 `register_fn`（无死条目）。

use std::collections::HashSet;

use r3e_core::cmd_spec::{self, CommandSpec, Domain};

/// `M2_E_PLAN` §3.6 的六条命令（顺序即 `cmd_spec/prefab.rs` 的声明顺序）。
const PREFAB_COMMANDS: [&str; 6] = [
    "prefab_define",
    "prefab_spawn",
    "prefab_spawn_many",
    "prefab_set_override",
    "prefab_clear_override",
    "prefab_info",
];

/// 脚本绑定的源文件（文本读入，见文件头注释）。
const SCRIPT_BINDINGS: &str = include_str!("../../r3e-script/src/api/prefab.rs");

fn prefab_specs() -> Vec<&'static CommandSpec> {
    cmd_spec::all()
        .filter(|spec| spec.name.starts_with("prefab_"))
        .collect()
}

/// 判据 ①：总量配平 + 六条齐全 + 归属里程碑正确。
#[test]
fn commands_are_419_and_prefab_has_six_entries() {
    assert_eq!(
        cmd_spec::all().count(),
        419,
        "commands 应为 419（E03 的 413 + prefab 六条）；改本数等于改门禁口径，须同步 \
         M2_E_PLAN §11.2 与 xtask 的 EXPECTED_COMMANDS"
    );

    let specs = prefab_specs();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    assert_eq!(
        names, PREFAB_COMMANDS,
        "六条 prefab 命令的名字与顺序应与 §3.6 一致；顺序漂移会让脚本与宿主两侧的\
         `by_name` 解析对不上"
    );
    for spec in &specs {
        assert_eq!(spec.since, "M2-E", "`{}` 的 since 应为 M2-E", spec.name);
    }
}

/// 判据 ②：六条全归 `Domain::Scene`，且全域计数仍为 18。
#[test]
fn prefab_commands_live_in_scene_domain_without_new_domain() {
    for spec in prefab_specs() {
        assert_eq!(
            spec.domain,
            Domain::Scene,
            "`{}` 应归 Domain::Scene（prefab 改的是场景装配结果，不是新资源域）",
            spec.name
        );
    }

    let domains: HashSet<Domain> = cmd_spec::all().map(|spec| spec.domain).collect();
    assert_eq!(
        domains.len(),
        18,
        "domains 应保持 18：prefab 六条复用 Scene，不新增 Domain 变体"
    );
}

/// 判据 ③：RPC 面恰为两条，`prefab.define` 无 rpc。
#[test]
fn only_spawn_and_info_carry_rpc() {
    let mut rpcs: Vec<&str> = prefab_specs().iter().filter_map(|spec| spec.rpc).collect();
    rpcs.sort_unstable();
    assert_eq!(
        rpcs,
        ["prefab.info", "prefab.spawn"],
        "P5 = B：只有 spawn（跨进程写）与 info（跨进程读）带 rpc"
    );

    let define = cmd_spec::by_name("prefab_define").expect("prefab_define 应在命令表里");
    assert!(
        define.rpc.is_none(),
        "prefab_define 故意无 rpc：模板建库只走脚本或宿主本地调用（代价见 §13 R7）"
    );

    // 点分小写、无下划线（`dispatch::canonical_method` 会把下划线直接删掉，
    // 带下划线的名字永远匹配不上请求）。
    for rpc in rpcs {
        assert!(
            rpc.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'),
            "rpc 名只允许小写字母、数字和点：{rpc}"
        );
        assert_eq!(
            cmd_spec::by_rpc(rpc).map(|spec| spec.name),
            Some(if rpc == "prefab.spawn" {
                "prefab_spawn"
            } else {
                "prefab_info"
            }),
            "by_rpc({rpc}) 应反查回对应脚本名"
        );
    }
}

/// 判据 ④：六条名字在脚本侧都有真实 `register_fn` 注册点（无死条目）。
///
/// `script_only +4` 的合法性全靠这条：`spawn_many` / `set_override` /
/// `clear_override` / `define` 没有 rpc，若连脚本注册点也没有，就是既跨不了进程
/// 也调不到的占位死条目（`M2_E_PLAN` §11.2 的 `R1` 新口径明确禁止）。
#[test]
fn every_prefab_command_has_a_script_register_point() {
    for name in PREFAB_COMMANDS {
        let needle = format!("\"{name}\"");
        let registered = SCRIPT_BINDINGS.match_indices(&needle).any(|(idx, _)| {
            // 注册点写法是 `register_fn("name", ...)` 或
            // `register_fn(\n "name", ...)`（多行形态），所以只看名字前面
            // 一小段窗口里有没有 `register_fn`。窗口按**字符**截取，
            // 避免在多字节中文注释的中间切断 `&str`。
            let mut head: Vec<char> = SCRIPT_BINDINGS[..idx].chars().collect();
            let keep = head.len().saturating_sub(64);
            head.drain(..keep);
            head.into_iter().collect::<String>().contains("register_fn")
        });
        assert!(
            registered,
            "`{name}` 在 r3e-script/src/api/prefab.rs 里没有 register_fn 注册点——\
             命令表登记了却调不到，属死条目"
        );
    }

    assert_eq!(
        SCRIPT_BINDINGS.matches("register_fn").count(),
        6,
        "prefab 绑定应恰好 6 个 register_fn，多一个少一个都说明表与实现失配"
    );
}
