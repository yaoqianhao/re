//! `query.*`：只读三查（find / describe / stats）。
//!
//! # 只读命令的定位
//!
//! 这三条**不开事务、不推版本**，所以 AI 可以在任何阶段放心调用：定位目标
//! （find）→ 看清现状（describe）→ 摸清全局（stats）→ 再决定要不要写。
//! 少了这一步，AI 就只能盲写，靠 `TX_REJECTED` 反推场景长什么样。
//!
//! # 渲染策略：先把回包看全，再谈排版
//!
//! find / describe / stats 的回包形状由宿主决定（节点投影 / 普查计数），这里不硬编字段：
//! 标量字段排成键值块，数组字段逐条铺开。**新加字段自动出现在输出里**——
//! 对「让 AI 读懂」而言，漏字段比排版丑严重得多。

use serde_json::Value;

use crate::args::{Global, QueryMethod};
use crate::cmd::{call, connect};
use crate::exit;
use crate::output::{kv, object_pairs, one_line, row_title, Out};

/// 入口。
pub fn run(global: &Global, method: QueryMethod, params: Value) -> i32 {
    let out = Out::new(global);

    let mut client = match connect(global) {
        Ok(client) => client,
        Err(code) => return code,
    };
    let reply = match call(global, &mut client, method.method(), params) {
        Ok(reply) => reply,
        Err(code) => return code,
    };

    out.success(&reply, &human(method, &reply));
    exit::OK
}

/// 人类通道渲染。
fn human(method: QueryMethod, reply: &Value) -> String {
    match reply {
        Value::Array(items) => {
            let mut text = format!("{}（{} 条）", method.method(), items.len());
            text.push_str(&rows(items));
            text
        }
        Value::Object(map) => {
            let mut text = String::new();

            let mut scalars: Vec<(String, String)> = Vec::new();
            let mut arrays: Vec<(&String, &Vec<Value>)> = Vec::new();
            for (key, value) in map {
                match value {
                    Value::Array(items) => arrays.push((key, items)),
                    other => scalars.push((key.clone(), one_line(other))),
                }
            }

            if !scalars.is_empty() {
                text.push_str(&kv(&scalars));
            }
            for (key, items) in arrays {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("{key}（{} 条）", items.len()));
                text.push_str(&rows(items));
            }
            if text.is_empty() {
                text.push_str("（空回包）");
            }
            text
        }
        other => one_line(other),
    }
}

/// 数组逐条铺开：`1. 目标名  k=v k=v`。
///
/// 标题取 `name`/`id`/`node`/`code` 里第一个非空值，并把同名字段从键值里剔除，
/// 免得每条都出现两遍同样的名字（AI 读得累、人也读得累）。
fn rows(items: &[Value]) -> String {
    if items.is_empty() {
        return "\n  （无）".to_string();
    }
    let mut text = String::new();
    for (index, item) in items.iter().enumerate() {
        let title = row_title(item, index);
        let detail: Vec<String> = object_pairs(item)
            .into_iter()
            .filter(|pair| {
                pair.rsplit_once('=')
                    .map(|(_, value)| value != title)
                    .unwrap_or(true)
            })
            .collect();
        text.push('\n');
        if detail.is_empty() {
            text.push_str(&format!("{}. {title}", index + 1));
        } else {
            text.push_str(&format!("{}. {title}  {}", index + 1, detail.join("  ")));
        }
    }
    text
}
