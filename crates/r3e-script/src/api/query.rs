//! 只读查询 / UI 指针交互。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「只读查询 / UI 指针交互」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---- 只读查询 ----

    let c = ctx.clone();
    engine.register_fn("local_light_count", move || -> i64 {
        c.doc.borrow().lights.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("local_light_keys", move || -> Array {
        c.doc
            .borrow()
            .lights
            .iter()
            .map(|l| Dynamic::from(l.key.clone()))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("local_light_exists", move |key: &str| -> bool {
        c.doc.borrow().local_light(key).is_some()
    });

    // 一盏灯的完整快照：
    // `#{ key, kind, pos, color, intensity, range, dir, inner, outer, cast_shadow, shadow_bias }`。
    // 键不存在返回空 map（`map.is_empty()` 即可判空，不必先 exists）。
    let c = ctx.clone();
    engine.register_fn("local_light", move |key: &str| -> Map {
        let scene = c.doc.borrow();
        let Some(l) = scene.local_light(key) else {
            return Map::new();
        };
        let mut m = Map::new();
        m.insert("key".into(), Dynamic::from(l.key.clone()));
        m.insert("kind".into(), Dynamic::from(l.kind.name().to_string()));
        m.insert(
            "pos".into(),
            Dynamic::from(
                l.pos
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert(
            "color".into(),
            Dynamic::from(
                l.color
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert("intensity".into(), Dynamic::from(l.intensity as f64));
        m.insert("range".into(), Dynamic::from(l.range as f64));
        m.insert(
            "dir".into(),
            Dynamic::from(
                l.dir
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert("inner".into(), Dynamic::from(l.inner_deg as f64));
        m.insert("outer".into(), Dynamic::from(l.outer_deg as f64));
        m.insert("cast_shadow".into(), Dynamic::from(l.cast_shadow));
        m.insert("shadow_bias".into(), Dynamic::from(l.shadow_bias as f64));
        m
    });

    // 单字段便捷读取（HUD 用）：不存在时返回 0 / 空串，也能当「没有这盏灯」的信号。
    let c = ctx.clone();
    engine.register_fn("local_light_intensity", move |key: &str| -> f64 {
        c.doc
            .borrow()
            .local_light(key)
            .map(|l| l.intensity as f64)
            .unwrap_or(0.0)
    });

    let c = ctx.clone();
    engine.register_fn("local_light_kind", move |key: &str| -> String {
        c.doc
            .borrow()
            .local_light(key)
            .map(|l| l.kind.name().to_string())
            .unwrap_or_default()
    });

    // 投影意愿（场景里 `cast_shadow` 的原值），与「本帧实际投影」是两件事。
    let c = ctx.clone();
    engine.register_fn("local_light_cast_shadow", move |key: &str| -> bool {
        c.doc
            .borrow()
            .local_light(key)
            .map(|l| l.cast_shadow)
            .unwrap_or(false)
    });

    // 本帧真正参与投影的灯数（= `plan_local_shadows` 选中的盏数）。
    // 关掉局部光源时不投影，直接 0——脚本做「阴影预算是否吃满」的 HUD 很省事。
    let c = ctx.clone();
    engine.register_fn("local_shadow_caster_count", move || -> i64 {
        let scene = c.doc.borrow();
        if !scene.lights_enabled {
            return 0;
        }
        plan_local_shadows(&scene.lights).len() as i64
    });

    // 某点受到的全部局部光照度（与 GPU 同式的 CPU 参考实现）：
    // 脚本可以用它做「站在灯下才回血」这类玩法，不必自己重写衰减公式。
    let c = ctx.clone();
    engine.register_fn(
        "local_light_irradiance",
        move |x: f64, y: f64, z: f64| -> f64 {
            let scene = c.doc.borrow();
            if !scene.lights_enabled {
                return 0.0;
            }
            let p = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            scene
                .lights
                .iter()
                // `irradiance` 返回线性 RGB，取最大分量当「亮度」标量即可
                .map(|l| l.irradiance(p).max_element().max(0.0) as f64)
                .sum()
        },
    );

    // ------------------------------------------------------------------
    // UI 指针交互（只读）。
    //
    // 事件由 `app` 每帧根据真实指针输入推进（`r3e_core::ui::UiInteraction`），
    // 脚本只负责读——这样命中判定只有 core 一份实现，渲染、脚本、编辑器不会各判各的。
    //
    // 事件语义：
    // * `hovered` / `unhovered`：指针进出元素；
    // * `pressed`：在元素上按下；
    // * `clicked`：在同一元素上按下并抬起（`released` 之前）；
    // * `cancelled`：按下了但没在同一元素上抬起（移出后释放 / 元素被删 / 丢事件）；
    // * `released`：有按下捕获时抬起**总会**收到一条。
    // ------------------------------------------------------------------

    // 本帧事件列表：`[#{ kind: "clicked", key: "btn" }, ...]`，顺序即发生顺序。
    let c = ctx.clone();
    engine.register_fn("ui_events", move || -> Array {
        c.ui_interaction
            .borrow()
            .events()
            .iter()
            .map(|ev| {
                let mut m = Map::new();
                m.insert("kind".into(), Dynamic::from(ev.kind.name().to_string()));
                m.insert("key".into(), Dynamic::from(ev.key.clone()));
                Dynamic::from(m)
            })
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("ui_event_count", move || -> i64 {
        c.ui_interaction.borrow().events().len() as i64
    });

    // 当前悬停 / 按住不放的元素 key（空串 = 无）。
    let c = ctx.clone();
    engine.register_fn("ui_hover", move || -> String {
        c.ui_interaction.borrow().hover().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("ui_active", move || -> String {
        c.ui_interaction.borrow().active().unwrap_or("").to_string()
    });

    // 便利判定：省掉脚本里遍历事件数组的样板代码。
    let c = ctx.clone();
    engine.register_fn("ui_clicked", move |key: &str| -> bool {
        c.ui_interaction.borrow().clicked(key)
    });

    let c = ctx.clone();
    engine.register_fn("ui_hovered", move |key: &str| -> bool {
        c.ui_interaction.borrow().hovered(key)
    });

    // 指针是否正压在 UI 上（脚本可据此屏蔽自己的「点空白处」逻辑）。
    let c = ctx.clone();
    engine.register_fn("ui_pointer_over_ui", move || -> bool {
        c.ui_interaction.borrow().pointer_over_ui()
    });

    // 屏幕坐标命中测试：返回最上层元素的 key（空串 = 没命中）。
    // 用的是最近一帧 `app` 送进来的屏幕尺寸，所以和实际渲染的矩形完全一致。
    let c = ctx.clone();
    engine.register_fn("ui_hit", move |x: f64, y: f64| -> String {
        let ui = c.ui_interaction.borrow();
        let scene = c.doc.borrow();
        hit_test_key(&scene.ui, ui.screen(), f32_of(x), f32_of(y))
            .map(str::to_string)
            .unwrap_or_default()
    });
}
