//! `rpc`：直调任意协议方法（**工具通道**，不进事务、不进撤销栈）。
//!
//! # 为什么需要它
//!
//! `tx` / `query` / `verify` 是三条**有形状**的路：事务、只读、体检。可 M1 的
//! 闭环里还有一类方法既不是场景写入、也不属于上面三条——`physics.bind`（把主场景
//! 装配进主物理世界）、`nav.bake`（烘焙导航）、`assets.load`（换资产）。它们没有
//! 专用子命令，而缺了它们 `verify` 的三层永远跑不满（物理层「世界与场景不同步」
//! 与导航层「宿主未接导航槽位」都会落到 `unchecked`，退出码恒为 1）。
//!
//! 没有这个入口的结果很具体：**体检门禁永远无法通过，不是场景有问题，是没人能喊动
//! 那两步装配**。
//!
//! # 护栏：改场景的那批方法一律拒绝
//!
//! 方法名在 [`crate::args::TX_ONLY_METHODS`]（`scene.spawn` / `scene.settransform` /
//! `scene.material.upsert` 等）时直接报错并指回 `tx intent`。理由是撤销栈：绕过事务
//! 直接改场景，编辑器里会留下一个「撤销不回去」的世界，而这正是 M1-S1 建事务通道时
//! 要消灭的东西。
//!
//! # 输出
//!
//! 人类通道只做「方法名 + 回包字段」的平铺，字段不做白名单——白名单会让
//! 「引擎多一个字段，CLI 就当没看见」成为常态，而新字段恰恰是排查时最需要的。

use serde_json::Value;

use crate::args::Global;
use crate::cmd::{call, connect};
use crate::exit;
use crate::output::{object_pairs, Out};

/// 入口。
pub fn run(global: &Global, method: &str, args: Value) -> i32 {
    let out = Out::new(global);

    let mut client = match connect(global) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let reply = match call(global, &mut client, method, args) {
        Ok(reply) => reply,
        Err(code) => return code,
    };

    out.success(&reply, &human(method, &reply));
    exit::OK
}

/// 人类通道渲染：首行点名方法，其后平铺回包字段。
fn human(method: &str, reply: &Value) -> String {
    let mut lines = vec![format!("{method} 成功（{}）", target_note(method))];
    let pairs = object_pairs(reply);
    if pairs.is_empty() {
        // 空对象是合法回包（例如 `physics.bind` 这类只报状态的方法），
        // 不硬凑一行「无字段」，但要让人知道「没东西」不是「没跑成」。
        lines.push("回包  {}".to_string());
        return lines.join("\n");
    }
    lines.extend(pairs);
    lines.join("\n")
}

/// 方法属于哪个域（只用于首行提示，不影响语义）。
fn target_note(method: &str) -> &'static str {
    match method.split('.').next().unwrap_or("") {
        "tx" => "事务通道",
        "query" => "只读查询",
        "scene" => "场景",
        "physics" => "物理",
        "nav" => "导航",
        "assets" | "asset" => "资产",
        "capture" => "截图",
        "media" => "多媒体",
        "ui" => "界面",
        _ => "协议直调",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn human_names_method_and_lists_fields() {
        let reply = json!({"bound": true, "bodies": 4});
        let text = human("physics.bind", &reply);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("physics.bind"), "{}", lines[0]);
        assert!(lines[0].contains("物理"), "首行应点出域：{}", lines[0]);
        assert!(
            lines.iter().any(|l| l.contains("bound=true")),
            "回包字段要平铺出来：{text}"
        );
    }

    #[test]
    fn empty_reply_says_empty_not_failure() {
        // 空回包不能渲染成空白——用户会当成「没跑成」。
        let text = human("nav.bake", &json!({}));
        assert!(text.contains("nav.bake"));
        assert!(text.contains("回包"), "{text}");
    }

    #[test]
    fn unknown_domain_still_renders() {
        let text = human("weird.thing", &json!({"x": 1}));
        assert!(text.contains("协议直调"), "{text}");
        assert!(text.contains("x=1"), "{text}");
    }
}
