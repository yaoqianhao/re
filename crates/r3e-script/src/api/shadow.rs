//! 场景保存 / 方向光阴影。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「场景保存 / 方向光阴影」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 场景保存（M1） ----------------
    // 只读导出：当前场景 → JSON 文本（相对路径无关，可赋值给变量做 diff / 打印）。
    // 失败返回空串并打印原因——与 create_node 一类写接口的返回风格保持一致。
    let c = ctx.clone();
    engine.register_fn("scene_json", move || -> String {
        match c.doc.borrow().to_json_string() {
            Ok(text) => text,
            Err(e) => {
                println!("[script] scene_json 失败: {e}");
                String::new()
            }
        }
    });

    // 落盘：把内存里的当前场景写回 path（相对路径按进程工作目录解析）。
    // 成功返回 true；失败返回 false 并打印原因。
    let c = ctx.clone();
    engine.register_fn("save_scene", move |path: &str| -> bool {
        match c.doc.borrow().save_to_path(path) {
            Ok(()) => true,
            Err(e) => {
                println!("[script] save_scene 失败: {e}");
                false
            }
        }
    });

    // ---------------- 阶段 D：方向光阴影 ----------------
    // 开关：set_shadow(true/false)。关闭后渲染器整帧跳过阴影通道，零开销。
    let c = ctx.clone();
    engine.register_fn("set_shadow", move |on: bool| {
        c.write(|s| s.light.shadow.enabled = on);
    });

    // 只读查询：当前阴影是否开启（脚本里做开关叙事 / 打印状态用）。
    let c = ctx.clone();
    engine.register_fn("shadow_enabled", move || -> bool {
        c.doc.borrow().light.shadow.enabled
    });

    // 阴影强度 0..1：1 = 完全遮挡，0 = 无阴影（保留光照）。用于淡入/调试。
    let c = ctx.clone();
    engine.register_fn("set_shadow_strength", move |strength: f64| {
        c.write(|s| s.light.shadow.strength = f32_of(strength));
    });

    // 阴影贴图分辨率（正方形），运行时改会触发贴图重建；非 2 的幂会被夹到 64..8192。
    let c = ctx.clone();
    engine.register_fn("set_shadow_size", move |size: i64| {
        c.write(|s| s.light.shadow.size = size.clamp(64, 8192) as u32);
    });

    // 正交相机半宽：0 = 按场景包围球自适应（默认），>0 = 固定覆盖范围（更清晰但更易裁切）。
    // 扩展十六起它是「各级覆盖半宽的上限」，不是逐级固定值。
    let c = ctx.clone();
    engine.register_fn("set_shadow_extent", move |half: f64| {
        c.write(|s| s.light.shadow.ortho_size = f32_of(half).max(0.0));
    });

    // 扩展十六：级联级数 1..=MAX_CASCADES（1 = 单级，等价阶段 D 的老行为）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_cascades", move |count: i64| {
        let max = r3e_core::MAX_CASCADES as i64;
        c.write(|s| s.light.shadow.cascade.count = count.clamp(1, max) as u32);
    });

    // 只读查询：当前级联级数。
    let c = ctx.clone();
    engine.register_fn("shadow_cascades", move || -> i64 {
        c.doc.borrow().light.shadow.cascade.count as i64
    });

    // 级间混合带宽：占该级跨度的比例 0..1。0 = 硬切（能看到级间接缝），0.1~0.3 较自然。
    let c = ctx.clone();
    engine.register_fn("set_shadow_blend", move |blend: f64| {
        c.write(|s| s.light.shadow.cascade.blend = f32_of(blend).clamp(0.0, 1.0));
    });

    // 阴影覆盖的最远视距：0 = 由场景包围球自动推断，>0 = 固定值（越近越清晰）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_distance", move |distance: f64| {
        c.write(|s| s.light.shadow.cascade.max_distance = f32_of(distance).max(0.0));
    });

    // 扩展二十四：级联遮挡体向光源方向的外扩距离（世界单位）。
    // 0 = 紧贴级边界（裁剪最狠，边缘处物件可能把影子投丢）；0.5~2 通常既安全又省 draw。
    let c = ctx.clone();
    engine.register_fn("set_shadow_caster_extend", move |extend: f64| {
        let max = r3e_core::MAX_CASTER_EXTEND as f64;
        c.write(|s| s.light.shadow.cascade.caster_extend = f32_of(extend).clamp(0.0, max as f32));
    });

    // 扩展二十四：按级裁剪开关。关掉即回到「每级都画全场景」的老行为（调试对照用）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_caster_cull", move |on: bool| {
        c.write(|s| s.light.shadow.cascade.caster_cull = on);
    });

    // 只读查询：当前是否按级裁剪遮挡体。
    let c = ctx.clone();
    engine.register_fn("shadow_caster_cull", move || -> bool {
        c.doc.borrow().light.shadow.cascade.caster_cull
    });

    let c = ctx.clone();
    engine.register_fn("set_background", move |r: f64, g: f64, b: f64| {
        c.write(|s| s.background = Vec4::new(f32_of(r), f32_of(g), f32_of(b), 1.0));
    });
}
