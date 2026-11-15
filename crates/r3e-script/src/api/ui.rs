//! 2D 屏幕空间 UI。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「2D 屏幕空间 UI」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 2D 屏幕空间 UI（M1 最小 UI）----------------
    // 与 text_* 同构：场景里的 `ui` 是唯一真源，这些绑定只改数据。
    // 都用 `key` 定位，同名即整体覆盖 —— 脚本每帧调用也不会重复堆积。
    // 尺寸 / 偏移写成带单位字符串："24px"、"50%"（百分比按屏幕宽高算）。
    //
    // 三条画元素的入口（`ui_rect` / `ui_border` / `ui_image`）全部走 core 的
    // `r3e_core::ui::upsert_styled`：数量上限与「同 key 怎么合并」都由 core 定，
    // 脚本层只负责把参数搬过去。返回值：true = 新建成功，false = 没新增（同 key
    // 覆盖 或 被上限拒绝）；被拒原因留在 `ui_last_error()` / `ui_error_count()`。
    let c = ctx.clone();
    engine.register_fn(
        "ui_rect",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let el = ui_element(
                &c,
                "ui_rect",
                key,
                UiKind::Rect,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "ui_border",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              width: f64,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let mut el = ui_element(
                &c,
                "ui_border",
                key,
                UiKind::Border,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            el.border_width = f32_of(width);
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "ui_image",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              path: &str,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let mut el = ui_element(
                &c,
                "ui_image",
                key,
                UiKind::Image,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            el.image = path.to_string();
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_set_pos", move |key: &str, x: &str, y: &str| -> bool {
        with_ui(&c, key, |el| {
            el.offset = [parse_ui_value(x), parse_ui_value(y)]
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_size", move |key: &str, w: &str, h: &str| -> bool {
        with_ui(&c, key, |el| {
            el.size = [parse_ui_value(w), parse_ui_value(h)]
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_anchor", move |key: &str, anchor: &str| -> bool {
        match Anchor::parse(anchor) {
            Some(a) => with_ui(&c, key, |el| el.anchor = a),
            None => {
                println!(
                    "[script] 未知锚点 `{anchor}`（可选 topleft / topcenter / ... / bottomright）"
                );
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "ui_set_color",
        move |key: &str, r: f64, g: f64, b: f64, a: f64| -> bool {
            with_ui(&c, key, |el| {
                el.color = [
                    f32_of(r).clamp(0.0, 1.0),
                    f32_of(g).clamp(0.0, 1.0),
                    f32_of(b).clamp(0.0, 1.0),
                    f32_of(a).clamp(0.0, 1.0),
                ]
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_set_alpha", move |key: &str, a: f64| -> bool {
        with_ui(&c, key, |el| el.color[3] = f32_of(a).clamp(0.0, 1.0))
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_layer", move |key: &str, layer: i64| -> bool {
        with_ui(&c, key, |el| {
            el.layer = layer.clamp(i32::MIN as i64, i32::MAX as i64) as i32
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_visible", move |key: &str, on: bool| -> bool {
        with_ui(&c, key, |el| el.visible = on)
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_border", move |key: &str, width: f64| -> bool {
        with_ui(&c, key, |el| el.border_width = f32_of(width))
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_image", move |key: &str, path: &str| -> bool {
        with_ui(&c, key, |el| el.image = path.to_string())
    });

    // uv 子区间（图集裁切；九宫格切分时作为整体外边界）
    let c = ctx.clone();
    engine.register_fn(
        "ui_set_uv",
        move |key: &str, u0: f64, v0: f64, u1: f64, v1: f64| -> bool {
            with_ui(&c, key, |el| {
                el.uv = [f32_of(u0), f32_of(v0), f32_of(u1), f32_of(v1)]
            })
        },
    );

    // 九宫格边距（左 / 上 / 右 / 下）：像素 "24px" 或百分比 "10%"（按元素自身宽 / 高）
    // 无参调用 = 关闭九宫格（四条边距清零），与传全 0 等价
    let c = ctx.clone();
    engine.register_fn("ui_set_slice", move |key: &str| -> bool {
        with_ui(&c, key, |el| {
            el.slice_margin = [
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
            ]
        })
    });

    let c = ctx.clone();
    engine.register_fn(
        "ui_set_slice",
        move |key: &str, left: &str, top: &str, right: &str, bottom: &str| -> bool {
            with_ui(&c, key, |el| {
                el.slice_margin = [
                    parse_ui_value(left),
                    parse_ui_value(top),
                    parse_ui_value(right),
                    parse_ui_value(bottom),
                ]
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_remove", move |key: &str| -> bool {
        c.write(|scene| {
            let before = scene.ui.len();
            scene.ui.retain(|el| el.key != key);
            scene.ui.len() != before
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_clear", move || {
        c.write(|s| s.ui.clear());
    });

    let c = ctx.clone();
    engine.register_fn("ui_exists", move |key: &str| -> bool {
        c.doc.borrow().ui.iter().any(|el| el.key == key)
    });

    let c = ctx.clone();
    engine.register_fn("ui_count", move || -> i64 {
        c.doc.borrow().ui.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("ui_keys", move || -> Array {
        c.doc
            .borrow()
            .ui
            .iter()
            .map(|el| Dynamic::from(el.key.clone()))
            .collect()
    });

    // 容量上限（core 定值，脚本侧只读）：脚本可以据此提前决定要不要新建元素。
    engine.register_fn("ui_capacity", move || -> i64 { MAX_UI_ELEMENTS as i64 });

    // 被拒 / 被纠正的写入：原因 + 累计次数。与 `audio_last_error` 同构，不刷屏。
    let c = ctx.clone();
    engine.register_fn("ui_last_error", move || -> String {
        c.ui_log.borrow().last_error().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("ui_error_count", move || -> i64 {
        c.ui_log.borrow().error_count() as i64
    });

    // UI 通道总开关：关掉后渲染层整帧跳过 UI pass（3D / 文本行为不变）。
    let c = ctx.clone();
    engine.register_fn("set_ui_enabled", move |on: bool| {
        c.write(|s| s.ui_enabled = on);
    });

    let c = ctx.clone();
    engine.register_fn("ui_enabled", move || -> bool { c.doc.borrow().ui_enabled });

    // 文本与 UI 的层次：false（默认）= 文本在上；true = UI 盖住文本。
    let c = ctx.clone();
    engine.register_fn("set_ui_above_text", move |on: bool| {
        c.write(|s| s.ui_above_text = on);
    });

    let c = ctx.clone();
    engine.register_fn("ui_above_text", move || -> bool {
        c.doc.borrow().ui_above_text
    });
}
