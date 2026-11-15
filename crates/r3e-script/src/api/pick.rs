//! 3D 屏幕射线拾取。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「3D 屏幕射线拾取」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------- 3D 屏幕射线拾取（扩展三十四） ----------
    //
    // 这三个入口是「脚本自己就能做交互」的关键：以前点选只能靠 `raycast`（要挂刚体）或
    // 手算投影矩阵，现在屏幕坐标直接换成物件。屏幕尺寸取 `app` 最近一帧送进
    // `UiInteraction` 的那份，所以和画面完全对齐，不是猜的窗口大小。

    // 脚本要自己做像素级交互（拾取 / 自绘 HUD / 判边界）就得知道画布多大。
    // 这三个读的与 `pick_*` 是同一份尺寸，运行中改窗口大小会跟着变。
    let c = ctx.clone();
    engine.register_fn("screen_width", move || -> f64 {
        c.ui_interaction.borrow().screen().0 as f64
    });
    let c = ctx.clone();
    engine.register_fn("screen_height", move || -> f64 {
        c.ui_interaction.borrow().screen().1 as f64
    });
    let c = ctx.clone();
    engine.register_fn("screen_size", move || -> Array {
        let (w, h) = c.ui_interaction.borrow().screen();
        vec![Dynamic::from(w as f64), Dynamic::from(h as f64)]
    });

    // 屏幕像素 → 最近命中的可见物件（渲染几何 AABB，不要求刚体）。
    let c = ctx.clone();
    engine.register_fn("pick_at", move |x: f64, y: f64| -> Map {
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hit = pick_from_screen(&scene, &provider, f32_of(x), f32_of(y), screen.0, screen.1);
        pick_map(&scene, hit)
    });

    // 鼠标当前位置的同一件事（少写两次 `mouse_x()`）。
    let c = ctx.clone();
    engine.register_fn("pick_under_mouse", move || -> Map {
        let (mx, my) = {
            let input = c.input.borrow();
            (input.mouse_x, input.mouse_y)
        };
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hit = pick_from_screen(&scene, &provider, mx, my, screen.0, screen.1);
        pick_map(&scene, hit)
    });

    // 同一条射线上**所有**命中的物件，按距离升序（穿透式点选 / 处理遮挡里的目标）。
    // 每项字段与 `pick_at` 完全一致，脚本用同一套取值代码即可。
    let c = ctx.clone();
    engine.register_fn("pick_all_at", move |x: f64, y: f64| -> Array {
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hits =
            pick_all_from_screen(&scene, &provider, f32_of(x), f32_of(y), screen.0, screen.1);
        hits.into_iter()
            .map(|hit| Dynamic::from(pick_map(&scene, Some(hit))))
            .collect()
    });

    // 本帧的 UI 层级顺序里，某个 key 是否存在（脚本常用来先判存在再读事件）。
    let c = ctx.clone();
    engine.register_fn("ui_event_kind_name", move |i: i64| -> String {
        let ui = c.ui_interaction.borrow();
        let idx = i.max(0) as usize;
        ui.events()
            .get(idx)
            .map(|e| e.kind.name().to_string())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn("ui_event_key", move |i: i64| -> String {
        let ui = c.ui_interaction.borrow();
        let idx = i.max(0) as usize;
        ui.events()
            .get(idx)
            .map(|e| e.key.clone())
            .unwrap_or_default()
    });

    // 事件名的稳定取值（脚本里常写 `if k == ui_kind_clicked()` 之类的判断）。
    engine.register_fn("ui_kind_clicked", || -> String {
        UiEventKind::Clicked.name().to_string()
    });
}
