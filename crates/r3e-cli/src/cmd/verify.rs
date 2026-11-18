//! `verify`：三层体检 + 门禁退出码。
//!
//! # `passed` 与 `ok` 不是一回事（这是本子命令存在的理由）
//!
//! - `ok` 只说「没有 Error」；
//! - `passed` 才说「可以放行」：`error_count == 0` **且** 没有 `unchecked`。
//!
//! 所以退出码由 `passed` 决定，绝不能写成 `ok`。差别在于「物理层没跑成」：
//! 场景里确实没有错误，但也没人检查过——判成通过等于让门禁带着一个未经检验的
//! 场景过关（`docs/M1_S5_PLAN.md` §3.3 的 E6）。
//!
//! # 截断不改结论
//!
//! `--limit` 只截 `issues` 明细，不动计数与 `passed`；`truncated=true` 表示
//! 「还有更多明细」，看计数决定要不要带更大的 `--limit` 重问。

use serde_json::{json, Value};

use crate::args::Global;
use crate::cmd::{call, connect};
use crate::exit;
use crate::output::{bool_field, int_field, kv, one_line, str_array, str_field, Out};

/// 入口。
pub fn run(global: &Global, targets: Value, limit: Option<u64>) -> i32 {
    let out = Out::new(global);

    let mut params = json!({ "targets": targets });
    if let Some(limit) = limit {
        if let Some(map) = params.as_object_mut() {
            map.insert("limit".to_string(), json!(limit));
        }
    }

    let mut client = match connect(global) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let reply = match call(global, &mut client, "scene.verify", params) {
        Ok(reply) => reply,
        Err(code) => return code,
    };

    let passed = match bool_field(&reply, "passed") {
        Some(passed) => passed,
        None => {
            // 缺 `passed` 时不能默认通过：门禁最危险的失效方式就是「字段没了当没事」。
            out.failure(
                "回包里没有 `passed` 字段，无法判定门禁",
                &["协议异常：检查 editor-host 的 scene.verify 回包".to_string()],
            );
            return exit::NEGATIVE;
        }
    };

    out.success(&reply, &human(&reply, passed));
    if passed {
        exit::OK
    } else {
        exit::NEGATIVE
    }
}

/// 人类通道渲染。
fn human(reply: &Value, passed: bool) -> String {
    let error_count = reply
        .get("counts")
        .and_then(|c| int_field(c, "error"))
        .unwrap_or(0);
    let warn_count = reply
        .get("counts")
        .and_then(|c| int_field(c, "warn"))
        .unwrap_or(0);

    let mut pairs: Vec<(String, String)> = vec![
        (
            "结论".to_string(),
            if passed {
                "通过（可放行）".to_string()
            } else {
                "未通过（不可放行）".to_string()
            },
        ),
        (
            "计数".to_string(),
            format!("error {error_count} / warn {warn_count}"),
        ),
    ];

    if let Some(ok) = bool_field(reply, "ok") {
        pairs.push((
            "ok".to_string(),
            format!(
                "{ok}{}",
                if ok && !passed {
                    "（只表示无 Error；未通过的原因是存在未检查层）"
                } else {
                    ""
                }
            ),
        ));
    }
    if let Some(hash) = str_field(reply, "stateHash") {
        pairs.push(("stateHash".to_string(), hash.to_string()));
    }
    let layers = str_array(reply, "layers");
    if !layers.is_empty() {
        pairs.push(("已跑层".to_string(), layers.join(" / ")));
    }
    if let Some(layered) = reply.get("counts").and_then(|c| c.get("byLayer")) {
        let by_layer: Vec<String> = ["static", "physics", "nav"]
            .iter()
            .filter_map(|name| int_field(layered, name).map(|count| format!("{name} {count}")))
            .collect();
        if !by_layer.is_empty() {
            pairs.push(("分层次数".to_string(), by_layer.join(" / ")));
        }
    }

    let mut text = kv(&pairs);

    let issues = reply
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let truncated = bool_field(reply, "truncated").unwrap_or(false);
    text.push_str(&format!(
        "\n\n问题明细（{}{}）",
        issues.len(),
        if truncated {
            " 条，已截断：加 `--limit` 看全"
        } else {
            " 条"
        }
    ));
    if issues.is_empty() {
        text.push_str("\n  （无）");
    }
    for (index, issue) in issues.iter().enumerate() {
        text.push_str(&format!("\n  {}. {}", index + 1, issue_line(issue)));
    }

    let unchecked = reply
        .get("unchecked")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !unchecked.is_empty() {
        text.push_str(&format!("\n\n未检查（{} 层）", unchecked.len()));
        for (index, item) in unchecked.iter().enumerate() {
            let layer = str_field(item, "layer").unwrap_or("?");
            let reason = str_field(item, "reason").unwrap_or("");
            text.push_str(&format!("\n  {}. {layer}：{reason}", index + 1));
        }
    }

    text
}

/// 一条 issue 的一行：关键字段按固定顺序在前，其余字段补在后面（不漏新字段）。
fn issue_line(issue: &Value) -> String {
    const LEAD: [&str; 7] = ["severity", "code", "layer", "target", "path", "msg", "hint"];
    let mut parts: Vec<String> = Vec::new();

    for key in LEAD {
        if let Some(value) = issue.get(key) {
            let text = one_line(value);
            if text.is_empty() || text == "null" {
                continue;
            }
            parts.push(format!("{key}={text}"));
        }
    }
    if let Value::Object(map) = issue {
        for (key, value) in map {
            if LEAD.contains(&key.as_str()) {
                continue;
            }
            let text = one_line(value);
            if text.is_empty() || text == "null" {
                continue;
            }
            parts.push(format!("{key}={text}"));
        }
    }
    parts.join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_reply() -> Value {
        json!({
            "passed": true,
            "ok": true,
            "stateHash": "0x6ee98c0a9dbf6555",
            "layers": ["static", "physics", "nav"],
            "counts": {"error": 0, "warn": 1, "byLayer": {"static": 0, "physics": 1, "nav": 2}},
            "issues": [],
            "unchecked": [],
            "truncated": false
        })
    }

    #[test]
    fn passing_verify_reads_as_releasable() {
        let text = human(&passing_reply(), true);
        assert!(text.contains("通过（可放行）"), "{text}");
        assert!(text.contains("error 0 / warn 1"), "{text}");
        assert!(text.contains("0x6ee98c0a9dbf6555"), "{text}");
        assert!(text.contains("static / physics / nav"), "{text}");
        assert!(text.contains("static 0 / physics 1 / nav 2"), "{text}");
        assert!(text.contains("（无）"), "没有 issue 时要明说：{text}");
    }

    #[test]
    fn unchecked_layer_makes_ok_but_not_passed_explicit() {
        // 这正是 `verify` 存在的原因：没有 Error，但有人没检查过，就不许放行。
        let reply = json!({
            "passed": false,
            "ok": true,
            "counts": {"error": 0, "warn": 0},
            "issues": [],
            "unchecked": [
                {"layer": "nav", "reason": "宿主未接导航槽位：出生点 / 可达性类结论全部跳过（不是通过）"}
            ]
        });
        let text = human(&reply, false);
        assert!(text.contains("未通过（不可放行）"), "{text}");
        assert!(text.contains("只表示无 Error"), "{text}");
        assert!(text.contains("未检查（1 层）"), "{text}");
        assert!(text.contains("nav：宿主未接导航槽位"), "{text}");
    }

    #[test]
    fn truncated_details_say_how_to_see_more() {
        let reply = json!({
            "passed": false,
            "ok": false,
            "counts": {"error": 2, "warn": 0},
            "issues": [{"severity": "error", "code": "static.missing_ref", "target": "chest"}],
            "truncated": true
        });
        let text = human(&reply, false);
        assert!(text.contains("已截断"), "{text}");
        assert!(text.contains("--limit"), "要告诉用户加什么参数：{text}");
        assert!(text.contains("severity=error"), "{text}");
        assert!(text.contains("code=static.missing_ref"), "{text}");
    }

    #[test]
    fn issue_line_puts_lead_fields_first_and_keeps_extras() {
        let issue = json!({
            "extra": 7,
            "hint": "补 spawn",
            "target": "chest",
            "severity": "error",
            "msg": "缺少出生点",
            "unused": null
        });
        let line = issue_line(&issue);
        assert_eq!(
            line,
            "severity=error  target=chest  msg=缺少出生点  hint=补 spawn  extra=7"
        );
    }

    #[test]
    fn issue_line_skips_empty_values() {
        let issue = json!({"severity": "warn", "target": "", "path": null});
        assert_eq!(issue_line(&issue), "severity=warn");
    }
}
