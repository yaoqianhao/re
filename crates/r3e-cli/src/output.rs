//! 双通道输出：人类通道（对齐文本）与 `--json` 通道（单行 JSON）。
//!
//! # 纪律：`--json` 模式下 stdout 只有**一行**
//!
//! 因为下游是脚本与 AI，任何多余的行都会让它不得不先「猜哪行才是结果」。
//! 所以本模块把两类内容彻底分开：
//!
//! - **结论**（`Value`）→ stdout，一行 JSON；
//! - **提示 / 建议 / 诊断**（人读的补充说明）→ stderr，两种模式都不占 stdout。
//!
//! # 纪律：不反序列化成引擎结构体
//!
//! 人读通道只从 `Value` 里**按需取几个字段**（`id` / `name` 之类）做排版，
//! 绝不 `use r3e_doc::Issue` 之类——那会让 `r3e-cli` 从「只认协议」变成引擎内部
//! 消费者（`dep_rules.json` 里 `r3e-cli.allow` 只有 `r3e-protocol`）。

use serde_json::Value;

use crate::args::Global;

/// 输出器。
#[derive(Debug, Clone, Copy)]
pub struct Out {
    json: bool,
}

impl Out {
    pub fn new(global: &Global) -> Self {
        Self { json: global.json }
    }

    /// 结论：json 模式打单行 JSON，人类模式打 `human`。
    pub fn success(self, value: &Value, human: &str) {
        if self.json {
            match serde_json::to_string(value) {
                Ok(line) => println!("{line}"),
                // 走到这里说明 value 里有非 UTF-8 键之类：改成打错误 JSON 也比打半行强。
                Err(e) => println!("{{\"error\":\"结果无法序列化：{e}\"}}"),
            }
        } else {
            println!("{human}");
        }
    }

    /// 一律走 stderr 的提示（人类模式与 json 模式都不污染 stdout）。
    pub fn note(self, text: &str) {
        if self.json {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }

    /// 失败：json 模式给一行 `{"error":...}`，人类模式给人读的多行说明。
    pub fn failure(self, summary: &str, details: &[String]) {
        if self.json {
            let payload = serde_json::json!({
                "error": summary,
                "details": details,
            });
            println!(
                "{}",
                serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"error\":\"失败详情无法序列化\"}".to_string()
                })
            );
        } else {
            eprintln!("失败：{summary}");
            for line in details {
                eprintln!("  {line}");
            }
        }
    }
}

/// 键值块：把「字段名 + 值」排成左对齐两列。
pub fn kv(pairs: &[(String, String)]) -> String {
    let width = pairs
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(key);
        for _ in display_width(key)..width {
            out.push(' ');
        }
        out.push_str("  ");
        out.push_str(value);
    }
    out
}

/// 值的一行摘要：字符串去引号，数组/对象给紧凑 JSON（不换行，长了下文再截）。
pub fn one_line(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<无法序列化>".to_string()),
    }
}

/// 把任意 JSON 对象排成 `k=v` 列表（用于 issue 这类字段随版本增长的记录）。
///
/// 不做字段白名单：白名单会让「引擎多出一个字段，CLI 就当没看见」成为常态，
/// 而体检结论恰恰要靠这些新字段解释。
pub fn object_pairs(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}={}", one_line(v)))
            .collect(),
        other => vec![one_line(other)],
    }
}

/// 取字符串字段（取不到返回 `None`，不 panic、不猜）。
pub fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// 取整数字段。
pub fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// 取布尔字段。
pub fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

/// 取字符串数组字段。
pub fn str_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().map(one_line).collect())
        .unwrap_or_default()
}

/// 一条记录的人类标题：优先 `name` → `id` → `node` → `code`，都没有就用下标。
pub fn row_title(value: &Value, index: usize) -> String {
    for key in ["name", "id", "node", "code", "target"] {
        if let Some(text) = str_field(value, key) {
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    format!("#{}", index + 1)
}

/// 字符显示宽度（中文按 2 列算，保证对齐不乱）。
///
/// 只区分「东亚宽字符」与其余：帮助与结论里出现的宽字符就是中文标点与汉字，
/// 不需要引入 `unicode-width` 那种完整实现。
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            let code = c as u32;
            let wide = (0x1100..=0x115F).contains(&code)
                || (0x2E80..=0xA4CF).contains(&code)
                || (0xAC00..=0xD7A3).contains(&code)
                || (0xF900..=0xFAFF).contains(&code)
                || (0xFE30..=0xFE4F).contains(&code)
                || (0xFF00..=0xFF60).contains(&code)
                || (0xFFE0..=0xFFE6).contains(&code);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 值列在第几「显示列」开始（中文占 2 列，`find` 的字节下标不能跨行比较）。
    fn value_column(line: &str, value: &str) -> usize {
        let index = line.find(value).expect("行里应有该值");
        display_width(&line[..index])
    }

    #[test]
    fn kv_pads_by_display_width_not_char_count() {
        // 「中文按 2 列」不是洁癖：按 char 数补齐会让含中文的键把后面的值推歪。
        let text = kv(&[
            ("结论".to_string(), "通过".to_string()),
            ("ok".to_string(), "true".to_string()),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "结论  通过");
        assert_eq!(lines[1], "ok    true");
        assert_eq!(
            value_column(lines[0], "通过"),
            value_column(lines[1], "true"),
            "两行的值必须从同一列开始"
        );
    }

    #[test]
    fn kv_single_pair_has_no_trailing_newline() {
        assert_eq!(kv(&[("a".to_string(), "1".to_string())]), "a  1");
    }

    #[test]
    fn one_line_unquotes_strings_and_compacts_containers() {
        assert_eq!(one_line(&json!("hero")), "hero");
        assert_eq!(one_line(&json!(null)), "null");
        assert_eq!(one_line(&json!(true)), "true");
        assert_eq!(one_line(&json!([1, 2])), "[1,2]");
        assert_eq!(one_line(&json!({"a":1})), r#"{"a":1}"#);
    }

    #[test]
    fn object_pairs_keeps_every_field() {
        // 不做字段白名单：引擎多出的字段必须能被看见，否则体检结论无法解释。
        let pairs = object_pairs(&json!({"code":"x","at":[1,2]}));
        assert!(pairs.contains(&"code=x".to_string()));
        assert!(pairs.contains(&"at=[1,2]".to_string()));
        assert_eq!(object_pairs(&json!("plain")), vec!["plain".to_string()]);
    }

    #[test]
    fn row_title_falls_back_in_documented_order() {
        assert_eq!(row_title(&json!({"name":"hero","id":"n1"}), 0), "hero");
        assert_eq!(row_title(&json!({"id":"n1"}), 0), "n1");
        assert_eq!(row_title(&json!({"node":"chest"}), 0), "chest");
        assert_eq!(
            row_title(&json!({"code":"support.floating"}), 0),
            "support.floating"
        );
        assert_eq!(row_title(&json!({}), 0), "#1");
        // 空字符串不算名字，继续往下找。
        assert_eq!(row_title(&json!({"name":"","id":"n2"}), 0), "n2");
    }

    #[test]
    fn typed_field_readers_never_panic_on_wrong_shape() {
        let value = json!({"s":"x","i":3,"b":true,"arr":["a","b"],"obj":{}});
        assert_eq!(str_field(&value, "s"), Some("x"));
        assert_eq!(str_field(&value, "i"), None);
        assert_eq!(int_field(&value, "i"), Some(3));
        assert_eq!(int_field(&value, "s"), None);
        assert_eq!(bool_field(&value, "b"), Some(true));
        assert_eq!(bool_field(&value, "s"), None);
        assert_eq!(
            str_array(&value, "arr"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(str_array(&value, "s").is_empty());
    }
}
