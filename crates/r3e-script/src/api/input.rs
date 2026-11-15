//! 时钟与随机 / 键盘 / 鼠标 / 手柄。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「时钟与随机 / 键盘 / 鼠标 / 手柄」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 时钟与随机 ----------------
    let c = ctx.clone();
    engine.register_fn("time", move || -> f64 { c.clock.borrow().time });

    let c = ctx.clone();
    engine.register_fn("frame", move || -> i64 { c.clock.borrow().frame as i64 });

    let c = ctx.clone();
    engine.register_fn("dt", move || -> f64 { c.clock.borrow().dt });

    // 瞬时帧率（1/dt）。HUD 文本常用，省得脚本自己算倒数还得防 0
    let c = ctx.clone();
    engine.register_fn("fps", move || -> f64 { c.clock.borrow().fps });

    let c = ctx.clone();
    engine.register_fn("rand", move |min: f64, max: f64| -> f64 {
        min + xorshift(&c.rng) * (max - min)
    });

    // ---------------- 输入：键盘 ----------------
    // 名字与鼠标 / 手柄保持同一套语义：down = 持续按住，pressed / released = 本帧边沿。
    let c = ctx.clone();
    engine.register_fn("key_down", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.down(k),
            None => {
                println!("[script] 未知按键 `{key}`（例：w / space / shift / f1 / up）");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_down", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.down(k),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_pressed", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.pressed(k),
            None => {
                println!("[script] 未知按键 `{key}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_pressed", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.pressed(k),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_released", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.released(k),
            None => {
                println!("[script] 未知按键 `{key}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_released", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.released(k),
            None => false,
        }
    });

    // 任意键：脚本做「按任意键开始」这类交互时不必逐个枚举。
    let c = ctx.clone();
    engine.register_fn("key_any", move || -> bool {
        c.input.borrow().keyboard.any_down()
    });

    let c = ctx.clone();
    engine.register_fn("key_any_pressed", move || -> bool {
        c.input.borrow().keyboard.any_pressed()
    });

    // 当前按住的键名列表（HUD / 调试面板）。
    let c = ctx.clone();
    engine.register_fn("keys_held", move || -> Array {
        c.input
            .borrow()
            .keyboard
            .held()
            .into_iter()
            .map(|k| Dynamic::from(k.name().to_string()))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("key_held_count", move || -> i64 {
        c.input.borrow().keyboard.held_count() as i64
    });

    // ---------------- 输入：鼠标 ----------------
    // 读数均为"本逻辑帧"的快照：pressed / released 只在该帧为 true。
    let c = ctx.clone();
    engine.register_fn("mouse_x", move || -> f64 {
        c.input.borrow().mouse_x as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_y", move || -> f64 {
        c.input.borrow().mouse_y as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pos", move || -> Array {
        let i = c.input.borrow();
        vec![
            Dynamic::from(i.mouse_x as f64),
            Dynamic::from(i.mouse_y as f64),
        ]
    });

    let c = ctx.clone();
    engine.register_fn("mouse_dx", move || -> f64 {
        c.input.borrow().mouse_dx as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_dy", move || -> f64 {
        c.input.borrow().mouse_dy as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_moved", move || -> bool {
        let i = c.input.borrow();
        i.mouse_dx != 0.0 || i.mouse_dy != 0.0
    });

    let c = ctx.clone();
    engine.register_fn("scroll", move || -> f64 { c.input.borrow().scroll as f64 });

    let c = ctx.clone();
    engine.register_fn("mouse_down", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_down(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`（可选 left/right/middle/back/forward）");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_down", move |idx: i64| -> bool {
        c.input.borrow().mouse_down(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pressed", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_pressed(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pressed", move |idx: i64| -> bool {
        c.input.borrow().mouse_pressed(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("mouse_released", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_released(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_released", move |idx: i64| -> bool {
        c.input.borrow().mouse_released(idx.max(0) as usize)
    });

    // ---------------- 输入：手柄 ----------------
    let c = ctx.clone();
    engine.register_fn("gamepad_connected", move || -> bool {
        c.input.borrow().pad.connected
    });

    let c = ctx.clone();
    engine.register_fn("gamepad_name", move || -> String {
        c.input.borrow().pad.name.clone()
    });

    let c = ctx.clone();
    engine.register_fn("pad_down", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_down(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_down", move |idx: i64| -> bool {
        c.input.borrow().pad_down(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_pressed", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_pressed(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_pressed", move |idx: i64| -> bool {
        c.input.borrow().pad_pressed(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_released", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_released(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_released", move |idx: i64| -> bool {
        c.input.borrow().pad_released(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_axis", move |axis: &str| -> f64 {
        match parse_pad_axis(axis) {
            // 摇杆做死区处理，扳机（lt/rt 为 6、7 号之后的索引）保留原始值
            Some(i) => {
                let pad = &c.input.borrow().pad;
                let v = if i <= r3e_core::input::PAD_RIGHT_Y {
                    pad.axis_deadzone(i)
                } else {
                    pad.axis(i)
                };
                v as f64
            }
            None => {
                println!("[script] 未知手柄轴 `{axis}`");
                0.0
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_axis", move |idx: i64| -> f64 {
        c.input.borrow().pad.axis(idx.max(0) as usize) as f64
    });
}
