//! 层内相位同步 / 相位偏移与跨层对齐。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「层内相位同步 / 相位偏移与跨层对齐」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 层内相位同步（扩展四十六） ----------------
    //
    // 用法：走 / 跑这类长短不一的片段同处一层时，开相位锁让两路按同一相位采样
    // （脚掌同时落地）；不开则沿用老口径——整层共用一个时间，短片段到末帧停住。

    // 开关相位锁：`true` = 各片段按归一化相位缩放采样。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync",
        move |inst: &str, layer: &str, enabled: bool| -> bool {
            c.write(|s| s.set_anim_layer_sync(inst, layer, enabled))
        },
    );

    // 按名字设策略：`"off"` / `"phase"`（大小写不敏感，`none` / `sync` 也认）。
    // 名字解析不了返回 false，层状态不动。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync_mode",
        move |inst: &str, layer: &str, mode: &str| -> bool {
            let Some(sync) = LayerSync::parse(mode) else {
                return false;
            };
            c.write(|s| s.set_anim_layer_sync_strategy(inst, layer, sync))
        },
    );

    // 回显：层是否开着相位锁（层不存在返回 false）。
    let c = ctx.clone();
    engine.register_fn("anim_layer_sync", move |inst: &str, layer: &str| -> bool {
        c.doc.borrow().anim_layer_sync(inst, layer).unwrap_or(false)
    });

    // 层相位（0..1，层不存在返回 0）。
    let c = ctx.clone();
    engine.register_fn("anim_layer_phase", move |inst: &str, layer: &str| -> f64 {
        c.doc
            .borrow()
            .anim_layer_phase(inst, layer)
            .map(f64::from)
            .unwrap_or(0.0)
    });

    // 单行探针：`sync=phase phase=0.300 dur=1.000 drift=0.000 slow=0.300/1.000,...`
    // `drift` 是各路片段相位与层相位的最大偏差，锁相位时恒为 0。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_probe",
        move |inst: &str, layer: &str| -> String {
            c.doc
                .borrow()
                .anim_layer_sync_probe(inst, layer)
                .unwrap_or_default()
        },
    );

    // ---------------- 相位偏移量与跨层对齐（扩展四十七） ----------------
    //
    // 用法：左右脚要「同样的步幅、错开半步」时给一路设 0.5 偏移；
    // 上半身要跟下半身同一相位起步时把上身的同步源指到下身的层名。

    // 设某一路片段的相位偏移：`offset` 折进 `[0,1)`（`-0.25` -> `0.75`，`1.25` -> `0.25`）。
    // `clip` 传空串表示层内第一路；片段/层不存在返回 false。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_phase_offset",
        move |inst: &str, layer: &str, clip: &str, offset: f64| -> bool {
            c.write(|s| s.set_anim_layer_clip_phase_offset(inst, layer, clip, f32_of(offset)))
        },
    );

    // 回显某一路的相位偏移（没设过是 0）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_phase_offset",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.doc
                .borrow()
                .anim_layer_clip_phase_offset(inst, layer, clip)
                .map(f64::from)
                .unwrap_or(0.0)
        },
    );

    // 设跨层相位源：`source` 层名（大小写不敏感，空串 = 清除、跟回自己）。
    // 源层不存在 / 指向自己 / 成环返回 false，层状态回滚。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync_source",
        move |inst: &str, layer: &str, source: &str| -> bool {
            let source = if source.trim().is_empty() {
                None
            } else {
                Some(source)
            };
            c.write(|s| s.set_anim_layer_sync_source(inst, layer, source))
        },
    );

    // 回显跨层相位源（没设过返回空串）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_source",
        move |inst: &str, layer: &str| -> String {
            c.doc
                .borrow()
                .anim_layer_sync_source(inst, layer)
                .unwrap_or_default()
        },
    );

    // 跨层对齐后的有效相位（0 表示层不存在）。没挂源层时等于 `anim_layer_phase`。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_phase",
        move |inst: &str, layer: &str| -> f64 {
            c.doc
                .borrow()
                .anim_layer_sync_phase(inst, layer)
                .map(f64::from)
                .unwrap_or(0.0)
        },
    );

    // 设某一路片段自带的播放速率与循环标志：速率为负倒放、0 定格首帧、非有限值按原速；
    // `looping = false` 时到末帧停住。只在层没开相位锁时影响采样。
    // `clip` 传空串表示层内第一路；片段/层不存在返回 false。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_playback",
        move |inst: &str, layer: &str, clip: &str, speed: f64, looping: bool| -> bool {
            c.write(|s| s.set_anim_layer_clip_playback(inst, layer, clip, f32_of(speed), looping))
        },
    );

    // 清除某一路的播放参数（回到跟着层时间轴走）。
    let c = ctx.clone();
    engine.register_fn(
        "clear_anim_layer_clip_playback",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.write(|s| s.clear_anim_layer_clip_playback(inst, layer, clip))
        },
    );

    // 回显某一路的播放速率（没设过返回 -1，因为 0 是合法的「定格首帧」）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_speed",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.doc
                .borrow()
                .anim_layer_clip_playback(inst, layer, clip)
                .map(|(speed, _)| f64::from(speed))
                .unwrap_or(-1.0)
        },
    );

    // 回显某一路的循环标志（没设过返回 true：跟层走的默认口径就是循环）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_looping",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.doc
                .borrow()
                .anim_layer_clip_playback(inst, layer, clip)
                .map(|(_, looping)| looping)
                .unwrap_or(true)
        },
    );

    // 某一路这一帧实际采到的时间（秒，含速率/循环折算）；层/片段不存在返回 -1。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_time",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.doc
                .borrow()
                .anim_layer_clip_time(inst, layer, clip)
                .map(f64::from)
                .unwrap_or(-1.0)
        },
    );
}
