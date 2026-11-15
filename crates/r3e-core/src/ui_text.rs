//! UI 数值的单位化文本 ↔ [`UiValue`] 互转（M0-S2 收口）。
//!
//! 脚本层与编辑器宿主原先各写一份逐字相同的解析实现，宿主还独有一份格式化实现
//! （能力不对称）。这里成为**唯一真源**：`"24px"` / `"50%"` / `"12"`（默认像素）
//! ↔ [`UiValue`]，两侧只保留薄包装转发，调用点一行不动。
//!
//! 为什么独立成模块而不是并进 [`crate::ui`]：`ui.rs` 已登记在
//! `xtask/size_debt.json` 白名单（只许下降不许上涨），往里加行会让 `lint-size` 直接红。

use crate::ui::{UiUnit, UiValue};

/// 解析带单位的 UI 数值：`"24px"` / `"50%"` / `"12"`（默认像素）。
///
/// 解析失败一律回落 `0.0`：脚本与 RPC 两侧历史上都这么做，脏值不该让调用方 panic。
pub fn parse_ui_value(text: &str) -> UiValue {
    let raw = text.trim();
    if let Some(num) = raw.strip_suffix('%') {
        return UiValue::pct(num.trim().parse::<f32>().unwrap_or(0.0));
    }
    let num = raw
        .strip_suffix("px")
        .unwrap_or(raw)
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    UiValue::px(num)
}

/// [`UiValue`] → 单位串（`"12"` / `"50%"`），供 RPC 回读。
pub fn format_ui_value(value: UiValue) -> String {
    match value.unit {
        UiUnit::Px => format!("{}", value.value),
        UiUnit::Percent => format!("{}%", value.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_px_percent_and_bare_number() {
        assert_eq!(parse_ui_value("24px"), UiValue::px(24.0));
        assert_eq!(parse_ui_value(" 50% "), UiValue::pct(50.0));
        assert_eq!(parse_ui_value("12"), UiValue::px(12.0));
    }

    #[test]
    fn bad_input_falls_back_to_zero() {
        assert_eq!(parse_ui_value("abc"), UiValue::px(0.0));
        assert_eq!(parse_ui_value("%"), UiValue::pct(0.0));
    }

    #[test]
    fn formats_both_units() {
        assert_eq!(format_ui_value(UiValue::px(12.0)), "12");
        assert_eq!(format_ui_value(UiValue::pct(50.0)), "50%");
    }
}
