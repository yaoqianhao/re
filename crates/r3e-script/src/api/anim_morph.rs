//! 动画播放 / 顶点形变 / 表情互斥 / 逐实例形状掩码。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「动画播放 / 顶点形变 / 表情互斥 / 逐实例形状掩码」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 动画（P3） ----------------
    // 一个「动画实例」= 一个资产实例 + 一个片段（导入时建好，名字为 `<资产名>/<片段名>`）。
    // 脚本侧只操作实例，不直接碰轨道；`anim_of(node)` 用于从骨骼节点反查它属于哪个实例。
    let c = ctx.clone();
    engine.register_fn("anims", move || -> Array {
        c.scene
            .borrow()
            .anim_names()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("anim_count", move || -> i64 {
        c.scene.borrow().animations.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("anim_of", move |node: &str| -> String {
        let s = c.scene.borrow();
        let Some(id) = s.find_id(node) else {
            return String::new();
        };
        for inst in &s.animations {
            if inst.bindings.iter().any(|b| b.node == id) {
                return inst.name.clone();
            }
        }
        String::new()
    });

    let c = ctx.clone();
    engine.register_fn("play_anim", move |name: &str| -> bool {
        c.scene.borrow_mut().play_anim(name)
    });

    let c = ctx.clone();
    engine.register_fn("stop_anim", move |name: &str| -> bool {
        c.scene.borrow_mut().stop_anim(name)
    });

    let c = ctx.clone();
    engine.register_fn("set_anim_speed", move |name: &str, speed: f64| -> bool {
        c.scene.borrow_mut().set_anim_speed(name, f32_of(speed))
    });

    let c = ctx.clone();
    engine.register_fn("set_anim_time", move |name: &str, t: f64| -> bool {
        c.scene.borrow_mut().set_anim_time(name, f32_of(t))
    });

    let c = ctx.clone();
    engine.register_fn("anim_time", move |name: &str| -> f64 {
        c.scene
            .borrow()
            .anim_time(name)
            .map(|t| t as f64)
            .unwrap_or(0.0)
    });

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_looping",
        move |name: &str, looping: bool| -> bool {
            match c.scene.borrow_mut().anim_instance_mut(name) {
                Some(inst) => {
                    inst.looping = looping;
                    true
                }
                None => false,
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn("anim_playing", move |name: &str| -> bool {
        c.scene
            .borrow()
            .anim_instance(name)
            .map(|a| a.playing)
            .unwrap_or(false)
    });

    // ---------------- 顶点形变（扩展三十六） ----------------
    //
    // 脚本侧只做「登记绑定 + 改权重」两件事：形变在顶点着色器里算，
    // 权重变了下一帧就生效，不需要重建网格或重传缓冲。

    let c = ctx.clone();
    engine.register_fn("morph_bind", move |name: &str, target_count: i64| -> bool {
        if target_count <= 0 {
            return false;
        }
        c.scene
            .borrow_mut()
            .bind_morph_by_name(name, target_count as usize)
    });

    let c = ctx.clone();
    engine.register_fn(
        "morph_set",
        move |name: &str, index: i64, weight: f64| -> bool {
            if index < 0 {
                return false;
            }
            c.scene
                .borrow_mut()
                .set_morph_weight_by_name(name, index as usize, f32_of(weight))
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_reset", move |name: &str| -> bool {
        c.scene.borrow_mut().reset_morph(name)
    });

    let c = ctx.clone();
    engine.register_fn("morph_weight", move |name: &str, index: i64| -> f64 {
        if index < 0 {
            return -1.0;
        }
        c.scene
            .borrow()
            .morph_weight_by_name(name, index as usize)
            .map(|w| w as f64)
            .unwrap_or(-1.0)
    });

    let c = ctx.clone();
    engine.register_fn("morph_count", move |name: &str| -> i64 {
        c.scene
            .borrow()
            .morph_target_count(name)
            .map(|n| n as i64)
            .unwrap_or(-1)
    });

    // 权重是否全部为 0（渲染层据此跳过整段形变路径）。
    let c = ctx.clone();
    engine.register_fn("morph_active", move |name: &str| -> bool {
        let s = c.scene.borrow();
        s.find_id(name)
            .and_then(|node| s.morph_of(node))
            .map(|m| m.is_active())
            .unwrap_or(false)
    });

    // ---------------- 表情互斥（扩展三十八） ----------------
    //
    // 把若干形状划进同一个组，组内权重和超过 1 时渲染层等比压回 1。
    // 脚本里 group 写 -1 即「取出该形状」（对应 `NO_EXCLUSIVE_GROUP` 哨兵）。

    let c = ctx.clone();
    engine.register_fn(
        "morph_exclusive",
        move |name: &str, index: i64, group: i64| -> bool {
            if index < 0 || group < -1 || group > u8::MAX as i64 {
                return false;
            }
            let slot = if group < 0 {
                NO_EXCLUSIVE_GROUP
            } else {
                group as u8
            };
            c.scene
                .borrow_mut()
                .set_morph_exclusive_by_name(name, index as usize, slot)
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_group", move |name: &str, index: i64| -> i64 {
        if index < 0 {
            return -1;
        }
        match c
            .scene
            .borrow()
            .morph_exclusive_by_name(name, index as usize)
        {
            Some(group) => group as i64,
            None => -1,
        }
    });

    // 归一化后的权重：与渲染层剔除吃的是同一份数，可直接断言「被压回 1」
    let c = ctx.clone();
    engine.register_fn("morph_norm", move |name: &str, index: i64| -> f64 {
        if index < 0 {
            return -1.0;
        }
        match c
            .scene
            .borrow()
            .morph_normalized_weight_by_name(name, index as usize)
        {
            Some(w) => w as f64,
            None => -1.0,
        }
    });

    // ---------------- 逐实例形状掩码（扩展四十） ----------------
    //
    // 同一个网格可以有十几个实例，各用一套形状子集：掩码第 i 位 = 形状 i 启用。
    // 掩码关空的实例整段摘掉形变（`morph_count` 写 0），半掩的实例权重按掩码归零。

    // 整块写掩码，返回裁剪到「网格形状数」之后的生效值；没有绑定返回 -1
    let c = ctx.clone();
    engine.register_fn("morph_mask_set", move |name: &str, mask: i64| -> i64 {
        if !(0..=u32::MAX as i64).contains(&mask) {
            return -1;
        }
        match c
            .scene
            .borrow_mut()
            .set_morph_mask_by_name(name, mask as u32)
        {
            Some(effective) => effective as i64,
            None => -1,
        }
    });

    let c = ctx.clone();
    engine.register_fn("morph_mask", move |name: &str| -> i64 {
        match c.scene.borrow().morph_mask_by_name(name) {
            Some(mask) => mask as i64,
            None => -1,
        }
    });

    // 单个形状开 / 关：脚本里写这个比算位运算直观
    let c = ctx.clone();
    engine.register_fn(
        "morph_show",
        move |name: &str, index: i64, enabled: bool| -> bool {
            if index < 0 {
                return false;
            }
            c.scene
                .borrow_mut()
                .set_morph_shape_enabled_by_name(name, index as usize, enabled)
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_visible", move |name: &str, index: i64| -> bool {
        if index < 0 {
            return false;
        }
        c.scene
            .borrow()
            .morph_shape_enabled_by_name(name, index as usize)
            .unwrap_or(false)
    });

    let c = ctx.clone();
    engine.register_fn("anim_info", move |name: &str| -> String {
        let s = c.scene.borrow();
        match s.anim_instance(name) {
            Some(a) => format!(
                "{} / 片段 {} / {} 轨道 / 时长 {:.2}s / 当前 {:.2}s / 速度 {} / {} / {}",
                a.name,
                a.clip.name,
                a.track_count(),
                a.clip.duration,
                a.sample_time(),
                a.speed,
                if a.looping { "循环" } else { "单次" },
                if a.playing { "播放中" } else { "已暂停" }
            ),
            None => format!("没有名为 `{name}` 的动画实例"),
        }
    });

    let c = ctx.clone();
    engine.register_fn("anim_clip", move |name: &str| -> String {
        c.scene
            .borrow()
            .anim_instance(name)
            .map(|a| a.clip.name.clone())
            .unwrap_or_default()
    });

    // 切片段：同一骨架的不同动作共用一个实例，避免重复 spawn 节点树。
    let c = ctx.clone();
    engine.register_fn("set_anim_clip", move |inst: &str, clip: &str| -> bool {
        c.scene.borrow_mut().set_anim_clip(inst, clip)
    });

    // 带交叉淡入地切片段：`dur` 秒内旧动作逐渐淡出，避免 idle→run 这类切换硬跳。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_clip_fade",
        move |inst: &str, clip: &str, dur: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_clip_fade(inst, clip, dur as f32)
        },
    );

    // 过渡进度（新动作权重，1.0 = 已完成/无过渡），供脚本做状态机与日志验收。
    let c = ctx.clone();
    engine.register_fn("anim_fade", move |inst: &str| -> f64 {
        c.scene
            .borrow()
            .anim_fade_weight(inst)
            .map(|w| w as f64)
            .unwrap_or(1.0)
    });

    // 正在淡出的旧动作名（无过渡为空串）。
    let c = ctx.clone();
    engine.register_fn("anim_fade_from", move |inst: &str| -> String {
        c.scene.borrow().anim_fade_source(inst)
    });

    let c = ctx.clone();
    engine.register_fn("anim_clips", move |prefix: &str| -> Array {
        c.scene
            .borrow()
            .anim_clip_names(prefix)
            .into_iter()
            .map(Into::into)
            .collect()
    });
}
