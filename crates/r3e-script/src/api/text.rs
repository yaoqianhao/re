//! 2D 屏幕空间文本。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「2D 屏幕空间文本」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 2D 屏幕空间文本（M1 文字渲染）----------------
    // 场景里的 `texts` 是唯一真源：这些绑定只改数据，渲染层下一帧自动重排重画。
    // 都用 `key` 定位，同名即覆盖 —— 脚本每帧调用也不会重复堆积。
    let c = ctx.clone();
    engine.register_fn(
        "text_add",
        move |key: &str,
              content: &str,
              x: f64,
              y: f64,
              size: f64,
              r: f64,
              g: f64,
              b: f64|
              -> bool {
            let text = ScreenText {
                key: key.to_string(),
                content: content.to_string(),
                x: f32_of(x),
                y: f32_of(y),
                size: f32_of(size),
                color: [f32_of(r), f32_of(g), f32_of(b), 1.0],
                ..Default::default()
            }
            .sanitized();
            c.write(|scene| {
                match scene.texts.iter_mut().find(|t| t.key == key) {
                    Some(slot) => {
                        // 颜色沿用新给的，其余样式（对齐 / 换行）保持不变
                        let align = slot.align;
                        let wrap = slot.wrap;
                        let line_height = slot.line_height;
                        *slot = text;
                        slot.align = align;
                        slot.wrap = wrap;
                        slot.line_height = line_height;
                        false
                    }
                    None => {
                        scene.texts.push(text);
                        true
                    }
                }
            })
        },
    );

    // 改内容：最常用的一条，单独给最简签名。
    let c = ctx.clone();
    engine.register_fn("text_set", move |key: &str, content: &str| -> bool {
        with_text(&c, key, |t| {
            if t.content != content {
                t.content = content.to_string();
            }
        })
    });

    let c = ctx.clone();
    engine.register_fn("text_set_pos", move |key: &str, x: f64, y: f64| -> bool {
        with_text(&c, key, |t| {
            t.x = f32_of(x);
            t.y = f32_of(y);
        })
    });

    let c = ctx.clone();
    engine.register_fn("text_set_size", move |key: &str, size: f64| -> bool {
        with_text(&c, key, |t| t.size = sanitize_size(f32_of(size)))
    });

    let c = ctx.clone();
    engine.register_fn(
        "text_set_color",
        move |key: &str, r: f64, g: f64, b: f64| -> bool {
            with_text(&c, key, |t| {
                t.color = [
                    f32_of(r).clamp(0.0, 1.0),
                    f32_of(g).clamp(0.0, 1.0),
                    f32_of(b).clamp(0.0, 1.0),
                    t.color[3],
                ];
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("text_set_alpha", move |key: &str, a: f64| -> bool {
        with_text(&c, key, |t| t.color[3] = f32_of(a).clamp(0.0, 1.0))
    });

    // 对齐：left / center / right（也接受 l / c / r 等别名）
    let c = ctx.clone();
    engine.register_fn("text_set_align", move |key: &str, align: &str| -> bool {
        match TextAlign::parse(align) {
            Some(a) => with_text(&c, key, |t| t.align = a),
            None => {
                println!("[script] 未知对齐方式 `{align}`（可选 left / center / right）");
                false
            }
        }
    });

    // 自动换行宽度（像素）；0 = 只按 \n 硬换行
    let c = ctx.clone();
    engine.register_fn("text_set_wrap", move |key: &str, width: f64| -> bool {
        with_text(&c, key, |t| t.wrap = f32_of(width).max(0.0))
    });

    // 行高倍数（相对字号）；0 = 用字体自然行高
    let c = ctx.clone();
    engine.register_fn(
        "text_set_line_height",
        move |key: &str, factor: f64| -> bool {
            with_text(&c, key, |t| t.line_height = f32_of(factor).max(0.0))
        },
    );

    let c = ctx.clone();
    engine.register_fn("text_set_visible", move |key: &str, on: bool| -> bool {
        with_text(&c, key, |t| t.visible = on)
    });

    let c = ctx.clone();
    engine.register_fn("text_remove", move |key: &str| -> bool {
        c.write(|scene| {
            let before = scene.texts.len();
            scene.texts.retain(|t| t.key != key);
            scene.texts.len() != before
        })
    });

    let c = ctx.clone();
    engine.register_fn("text_clear", move || {
        c.write(|s| s.texts.clear());
    });

    let c = ctx.clone();
    engine.register_fn("text_exists", move |key: &str| -> bool {
        c.doc.borrow().texts.iter().any(|t| t.key == key)
    });

    let c = ctx.clone();
    engine.register_fn("text_content", move |key: &str| -> String {
        c.doc
            .borrow()
            .texts
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.content.clone())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn("text_count", move || -> i64 {
        c.doc.borrow().texts.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("text_keys", move || -> Array {
        c.doc
            .borrow()
            .texts
            .iter()
            .map(|t| Dynamic::from(t.key.clone()))
            .collect()
    });

    // 文本通道总开关：关掉后渲染层整帧跳过文本 pass（3D 主通道行为不变）。
    let c = ctx.clone();
    engine.register_fn("set_text_enabled", move |on: bool| {
        c.write(|s| s.text_enabled = on);
    });

    let c = ctx.clone();
    engine.register_fn("text_enabled", move || -> bool {
        c.doc.borrow().text_enabled
    });

    // 宽度 / 高度估算：不加载字体文件的近似值，脚本排版预算用（不是最终像素宽度）。
    engine.register_fn("text_width", |content: &str, size: f64| -> f64 {
        estimate_text_width(content, sanitize_size(f32_of(size))) as f64
    });

    engine.register_fn("text_height", |content: &str, size: f64| -> f64 {
        estimate_text_height(content, sanitize_size(f32_of(size))) as f64
    });
}
