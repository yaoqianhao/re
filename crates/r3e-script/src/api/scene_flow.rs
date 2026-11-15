//! 场景切换与存档。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「场景切换与存档」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------- 场景切换与存档（扩展十八 B） ----------
    //
    // 这一组全是"申请"而不是"执行"：宿主在帧首统一处理命令，脚本再怎么调也
    // 不会在 `on_start` / `on_update` 中途把世界换掉。
    let c = ctx.clone();
    engine.register_fn("scene_switch", move |path: &str| -> bool {
        match non_empty_path(path) {
            Some(p) => queue_scene_cmd(&c, SceneCommand::switch(p)),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_back", move || -> bool {
        queue_scene_cmd(&c, SceneCommand::back())
    });

    let c = ctx.clone();
    engine.register_fn("scene_save_archive", move |path: &str| -> bool {
        // 空串 = 交给宿主挑默认位置（与 RPC `scene.archive.save` 省略 path 同义）。
        queue_scene_cmd(&c, SceneCommand::save_archive(non_empty_path(path)))
    });

    let c = ctx.clone();
    engine.register_fn("scene_load_archive", move |path: &str| -> bool {
        match non_empty_path(path) {
            Some(p) => queue_scene_cmd(&c, SceneCommand::load_archive(p)),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_command_pending", move || -> bool {
        c.scene_cmd.borrow().is_some()
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_names", move || -> Array {
        c.scene_stack
            .borrow()
            .names()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_depth", move || -> i64 {
        c.scene_stack.borrow().depth() as i64
    });

    // 游标位置：空栈返回 -1（而不是 0），"没有当前层"与"第 0 层"必须能区分。
    let c = ctx.clone();
    engine.register_fn("scene_stack_index", move || -> i64 {
        match c.scene_stack.borrow().current_index() {
            Some(i) => i as i64,
            None => -1,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_current", move || -> String {
        c.scene_stack
            .borrow()
            .current()
            .map(|e| e.name.clone())
            .unwrap_or_default()
    });
}
