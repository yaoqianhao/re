//! `args` 的解析契约测试。
//!
//! 单独成文件而不是塞在 `args.rs` 里：`lint-size` 的 800 行门槛对「实现」和
//! 「测试」一视同仁，而这两部分的阅读目的完全不同——改解析逻辑的人不该在一屏之外
//! 才看到实现。

use super::*;

/// 把一行命令拆成 argv（测试里一律按空格切，引号不参与）。
fn parse_str(line: &str) -> Result<Invocation, ParseError> {
    parse(line.split_whitespace().map(str::to_string).collect())
}

fn expect_err(line: &str) -> String {
    match parse_str(line) {
        Ok(inv) => panic!("`{line}` 不该解析成功，实得 {inv:?}"),
        Err(err) => err.message,
    }
}

#[test]
fn missing_subcommand_names_all_five() {
    let msg = expect_err("");
    assert!(msg.contains("缺少子命令"), "{msg}");
    for name in ["tx", "query", "verify", "rpc", "replay"] {
        assert!(msg.contains(name), "帮助里应点到 `{name}`：{msg}");
    }
}

#[test]
fn unknown_subcommand_is_rejected() {
    let msg = expect_err("querry stats");
    assert!(msg.contains("未知子命令"), "{msg}");
    assert!(msg.contains("querry"), "要把用户敲错的原样回显：{msg}");
}

#[test]
fn unknown_flag_is_rejected_not_ignored() {
    // `--targes` 拼错若被静默忽略，用户会以为自己在筛目标点，实际跑的是全场景体检。
    let msg = expect_err("verify --targes []");
    assert!(msg.contains("未知参数"), "{msg}");
    assert!(msg.contains("targes"), "{msg}");
}

#[test]
fn duplicate_flag_is_rejected() {
    let msg = expect_err("query stats --addr 127.0.0.1:1 --addr 127.0.0.1:2");
    assert!(msg.contains("重复"), "{msg}");
}

#[test]
fn value_flag_without_value_is_rejected() {
    let msg = expect_err("query stats --addr");
    assert!(msg.contains("缺少取值"), "{msg}");
}

#[test]
fn bool_flag_does_not_swallow_subcommand() {
    // CLI 文档第 2 条：`--json verify` 里 verify 是子命令，不是 --json 的值。
    let inv = parse_str("--json verify").expect("应解析成功");
    assert!(inv.global.json);
    assert!(matches!(inv.cmd, Cmd::Verify { .. }));

    let inv = parse_str("verify --json").expect("开关放后面等价");
    assert!(inv.global.json);
    assert!(matches!(inv.cmd, Cmd::Verify { .. }));
}

#[test]
fn bool_flag_rejects_inline_value() {
    let msg = expect_err("query stats --json=1");
    assert!(msg.contains("开关"), "{msg}");
}

#[test]
fn args_must_be_json_object() {
    let msg = expect_err("query find --args [1,2]");
    assert!(msg.contains("必须是 JSON 对象"), "{msg}");
    assert!(msg.contains("数组"), "要点出实际类型：{msg}");
}

#[test]
fn args_with_broken_json_reports_original_text() {
    let msg = expect_err("query find --args {oops}");
    assert!(msg.contains("不是合法 JSON"), "{msg}");
    assert!(msg.contains("{oops}"), "要把原文回显出来：{msg}");
}

#[test]
fn describe_positional_beats_args_node() {
    // 位置参数优先：`--args` 里写 node 是笔误，不该压过命令行上明写的那个。
    let inv = parse_str(r#"query describe hero --args {"node":"villain"}"#).expect("应解析成功");
    match inv.cmd {
        Cmd::Query { method, params } => {
            assert_eq!(method, QueryMethod::Describe);
            assert_eq!(params["node"], json!("hero"));
        }
        other => panic!("应为 query，实得 {other:?}"),
    }
}

#[test]
fn describe_without_node_is_rejected() {
    let msg = expect_err("query describe");
    assert!(msg.contains("缺少 <node>"), "{msg}");
}

#[test]
fn flag_from_other_subcommand_is_rejected() {
    let msg = expect_err("tx begin --limit 5");
    assert!(msg.contains("不属于当前子命令"), "{msg}");
}

#[test]
fn state_file_empty_string_disables_persistence() {
    // 空串只能靠真正的空 argv 项表示：`--state-file ""`（shell 去引号后就是一个空串）。
    let inv = parse(vec![
        "query".to_string(),
        "stats".to_string(),
        "--state-file".to_string(),
        String::new(),
    ])
    .expect("应解析成功");
    assert_eq!(inv.global.state_file, None);
}

#[test]
fn state_file_defaults_to_session_json() {
    let inv = parse_str("query stats").expect("应解析成功");
    assert_eq!(
        inv.global.state_file,
        Some(PathBuf::from(DEFAULT_STATE_FILE))
    );
}

#[test]
fn zero_timeout_is_rejected() {
    let msg = expect_err("query stats --timeout 0");
    assert!(msg.contains("大于 0"), "{msg}");
}

#[test]
fn timeout_must_be_number() {
    let msg = expect_err("query stats --timeout soon");
    assert!(msg.contains("非负整数"), "{msg}");
}

#[test]
fn verify_targets_must_be_array() {
    let msg = expect_err(r#"verify --targets {"name":"chest"}"#);
    assert!(msg.contains("必须是数组"), "{msg}");
}

#[test]
fn verify_targets_default_to_empty_array() {
    let inv = parse_str("verify").expect("应解析成功");
    match inv.cmd {
        Cmd::Verify { targets, limit } => {
            assert_eq!(targets, json!([]));
            assert_eq!(limit, None);
        }
        other => panic!("应为 verify，实得 {other:?}"),
    }
}

#[test]
fn verify_limit_is_parsed() {
    let inv = parse_str("verify --limit 3").expect("应解析成功");
    match inv.cmd {
        Cmd::Verify { limit, .. } => assert_eq!(limit, Some(3)),
        other => panic!("应为 verify，实得 {other:?}"),
    }
}

#[test]
fn tx_apply_requires_op() {
    let msg = expect_err("tx apply");
    assert!(msg.contains("缺少 <op>"), "{msg}");
}

#[test]
fn tx_unknown_action_lists_all_actions() {
    let msg = expect_err("tx publish");
    assert!(msg.contains("未知 tx 动作"), "{msg}");
    for action in ["begin", "apply", "intent", "preview", "commit", "rollback"] {
        assert!(msg.contains(action), "应列出 `{action}`：{msg}");
    }
}

#[test]
fn tx_begin_with_extra_positional_is_rejected() {
    let msg = expect_err("tx begin now");
    assert!(msg.contains("多余参数"), "{msg}");
}

#[test]
fn tx_methods_match_protocol_names() {
    assert_eq!(TxAction::Begin.method(), "tx.begin");
    assert_eq!(TxAction::Apply { op: "x".into() }.method(), "tx.apply");
    assert_eq!(TxAction::Intent { name: "x".into() }.method(), "tx.intent");
    assert_eq!(TxAction::Preview.method(), "tx.preview");
    assert_eq!(TxAction::Commit.method(), "tx.commit");
    assert_eq!(TxAction::Rollback.method(), "tx.rollback");
}

#[test]
fn only_commit_and_rollback_are_terminal() {
    // `is_terminal` 决定 state-file 何时清空：判错会让下一次 `tx apply` 拿着死 txId 打空气。
    assert!(!TxAction::Begin.is_terminal());
    assert!(!TxAction::Preview.is_terminal());
    assert!(TxAction::Commit.is_terminal());
    assert!(TxAction::Rollback.is_terminal());
}

#[test]
fn explicit_tx_id_is_kept_and_blank_is_dropped() {
    let inv = parse_str("tx commit --tx tx-7").expect("应解析成功");
    match inv.cmd {
        Cmd::Tx { tx, .. } => assert_eq!(tx.as_deref(), Some("tx-7")),
        other => panic!("应为 tx，实得 {other:?}"),
    }
    let inv = parse(vec![
        "tx".to_string(),
        "commit".to_string(),
        "--tx".to_string(),
        String::new(),
    ])
    .expect("应解析成功");
    match inv.cmd {
        Cmd::Tx { tx, .. } => assert_eq!(tx, None, "空 txId 应视为「没给」"),
        other => panic!("应为 tx，实得 {other:?}"),
    }
}

#[test]
fn replay_defaults_match_replay_crate() {
    let inv = parse_str("replay").expect("应解析成功");
    match inv.cmd {
        Cmd::Replay(args) => {
            assert_eq!(args.seed, DEFAULT_REPLAY_SEED);
            assert_eq!(args.frames, DEFAULT_REPLAY_FRAMES);
            assert_eq!(args.hash_every, DEFAULT_REPLAY_HASH_EVERY);
            assert_eq!(args.expect, None);
            assert_eq!(args.bin, None);
        }
        other => panic!("应为 replay，实得 {other:?}"),
    }
}

#[test]
fn replay_rejects_zero_frames_and_hash_every() {
    assert!(expect_err("replay --frames 0").contains("大于 0"));
    assert!(expect_err("replay --hash-every 0").contains("大于 0"));
}

#[test]
fn replay_rejects_blank_expect_and_blank_bin() {
    let blank = |flag: &str| {
        parse(vec![
            "replay".to_string(),
            flag.to_string(),
            "   ".to_string(),
        ])
        .expect_err("空白取值应被拒")
        .message
    };
    assert!(blank("--expect").contains("不能为空"));
    assert!(blank("--replay-bin").contains("不能为空路径"));
}

#[test]
fn replay_keeps_expect_and_bin() {
    let inv = parse_str("replay --expect 0123456789abcdef --replay-bin C:/bin/r3e-replay.exe")
        .expect("应解析成功");
    match inv.cmd {
        Cmd::Replay(args) => {
            assert_eq!(args.expect.as_deref(), Some("0123456789abcdef"));
            assert_eq!(args.bin, Some(PathBuf::from("C:/bin/r3e-replay.exe")));
        }
        other => panic!("应为 replay，实得 {other:?}"),
    }
}

#[test]
fn help_flag_wins_over_any_subcommand() {
    for line in ["--help", "help", "query stats --help"] {
        assert!(
            matches!(parse_str(line).expect("应解析成功").cmd, Cmd::Help),
            "`{line}` 应解析成 Help"
        );
    }
    // 缺 op 的 tx apply 是参数错误，不该被 help 掩盖——没给 --help 就不该进 Help。
    assert!(parse_str("tx apply").is_err());
}

#[test]
fn help_text_documents_every_exit_code() {
    let text = help_text();
    for code in ["0", "1", "2", "3", "4"] {
        assert!(text.contains(code), "帮助里缺少退出码 {code}");
    }
    assert!(text.contains(DEFAULT_STATE_FILE));
    assert!(text.contains(DEFAULT_ADDR));
}

#[test]
fn addr_source_reports_where_the_address_came_from() {
    // 显式给了就报 `--addr`（不碰环境变量，避免测试间互相污染）。
    let global = Global {
        addr: Some("127.0.0.1:9abc".to_string()),
        ..Global::default()
    };
    assert_eq!(global.addr_source(), "--addr");
    assert_eq!(global.resolved_addr(), "127.0.0.1:9abc");
}

#[test]
fn rpc_takes_method_and_object_args() {
    let inv = parse_str("rpc nav.bake").expect("应解析成功");
    assert_eq!(
        inv.cmd,
        Cmd::Rpc {
            method: "nav.bake".to_string(),
            args: json!({})
        }
    );
}

#[test]
fn rpc_requires_method_name() {
    let msg = expect_err("rpc");
    assert!(msg.contains("缺少 <method>"), "{msg}");
    assert!(msg.contains("nav.bake"), "要给出可用示例：{msg}");
}

#[test]
fn rpc_rejects_method_without_namespace() {
    let msg = expect_err("rpc bake");
    assert!(msg.contains("命名空间"), "{msg}");
}

#[test]
fn rpc_rejects_tx_methods() {
    // 事务状态在 state-file 里，`rpc` 不记账——放过去等于让事务静默半截。
    let msg = expect_err("rpc tx.commit");
    assert!(msg.contains("事务通道"), "{msg}");
}

#[test]
fn rpc_rejects_scene_writes() {
    // 绕过事务改场景 = 撤销栈里没有这一格，这是 M1-S1 要消灭的形态。
    for method in [
        "scene.spawn",
        "scene.settransform",
        "scene.setvisible",
        "scene.material.upsert",
        "scene.remove",
        "light.remove",
        "ui.remove",
    ] {
        let msg = expect_err(&format!("rpc {method}"));
        assert!(msg.contains("只允许走事务"), "{method}: {msg}");
        assert!(msg.contains("tx intent"), "要指回事务通道：{msg}");
    }
}

#[test]
fn rpc_rejects_intent_names() {
    // 这三个名字只在 `tx intent` 的表里，`rpc` 撞上去本该得 -32601「未知方法」，
    // 那句提示会把人引去查拼写。CLI 提前拦住，直接说清它们是高层意图。
    for intent in TX_ONLY_INTENTS {
        let msg = expect_err(&format!("rpc {intent}"));
        assert!(msg.contains("只被 `tx intent` 接受"), "{intent}: {msg}");
    }
}

#[test]
fn tx_only_tables_are_consistent() {
    // 两张表的口径不同，混起来就会给出误导提示：
    // - `TX_ONLY_METHODS` 是 `rpc` 拦下的名字，其中 `scene.setvisible` 只被
    //   `tx apply` 的 `Intent::parse` 认，协议方法表里没有它；
    // - `TX_ONLY_INTENTS` 是只被 `tx intent` 接受的高层意图名，**不该**出现在
    //   方法表里（一旦升格成真方法，提示语得跟着改）。
    // 另外：拼错一个字母（`scene.settransfrom`）不会让任何测试变红，
    // 只会让护栏静默漏风，所以名字要对着方法表核。
    use r3e_protocol::client::is_known_method;
    for name in TX_ONLY_INTENTS {
        assert!(
            !is_known_method(name),
            "`{name}` 已是协议方法，别再按「只被 tx intent 接受」解释"
        );
    }
    for name in TX_ONLY_METHODS {
        if *name == "scene.setvisible" {
            // 事务专属 op：`tx apply scene.setvisible` 认，`rpc` 只能拒。
            continue;
        }
        assert!(
            is_known_method(name),
            "`{name}` 不在协议方法表内：护栏列了一个不存在的口子，等于漏风"
        );
    }
    for name in TX_ONLY_METHODS {
        assert!(
            !TX_ONLY_INTENTS.contains(name),
            "`{name}` 同时在方法表和意图表里，提示语会二义"
        );
    }
}

#[test]
fn rpc_rejects_foreign_flags() {
    let msg = expect_err("rpc nav.bake --targets []");
    assert!(msg.contains("不属于当前子命令"), "{msg}");
}
