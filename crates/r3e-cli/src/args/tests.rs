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
fn missing_subcommand_names_all_four() {
    let msg = expect_err("");
    assert!(msg.contains("缺少子命令"), "{msg}");
    for name in ["tx", "query", "verify", "replay"] {
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
