//! 动画混合树 / 1D-2D-3D 混合空间。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「动画混合树 / 1D-2D-3D 混合空间」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 动画混合树（扩展十八 A） ----------------
    // 主片段是「当前动作」，层是同时进行的附加动作（上半身攻击、呼吸叠加）。
    // 层用名字寻址（`layer` 传空串 = 第一层）；遮罩用逗号串表达：
    // `"arm,hand"` 只作用于这两处骨骼，`"!root"` 表示除 root 外全部，
    // 空串或 `"*"` 表示整副骨架（默认）——骨骼按路径后缀匹配，`"arm"` 命中 `root/spine/arm`。
    let c = ctx.clone();
    engine.register_fn("anim_layers", move |inst: &str| -> Array {
        c.doc
            .borrow()
            .anim_layer_names(inst)
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("anim_layer_count", move |inst: &str| -> i64 {
        c.doc
            .borrow()
            .anim_layer_count(inst)
            .map(|n| n as i64)
            .unwrap_or(0)
    });

    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_info",
        move |inst: &str, layer: &str| -> String {
            c.doc
                .borrow()
                .anim_layer_report(inst, layer)
                .unwrap_or_else(|| format!("实例 `{inst}` 上没有名为 `{layer}` 的层"))
        },
    );

    // 有效权重（含进行中的过渡），0 = 层此刻完全不参与合成。
    let c = ctx.clone();
    engine.register_fn("anim_layer_weight", move |inst: &str, layer: &str| -> f64 {
        c.doc
            .borrow()
            .anim_layer_effective_weight(inst, layer)
            .map(|w| w as f64)
            .unwrap_or(0.0)
    });

    // 挂一层覆盖型（override）：命中遮罩的骨骼直接取层姿态。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.write(|s| {
                s.add_anim_layer_by_clip(inst, layer, clip, f32_of(weight), LayerMode::Override)
            })
        },
    );

    // 挂一层加法型（additive）：层姿态相对首帧的增量叠到主姿态上，适合呼吸 / 抖动。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_additive",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.write(|s| {
                s.add_anim_layer_by_clip(inst, layer, clip, f32_of(weight), LayerMode::Additive)
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_weight",
        move |inst: &str, layer: &str, weight: f64| -> bool {
            c.write(|s| s.set_anim_layer_weight(inst, layer, f32_of(weight)))
        },
    );

    // 带过渡地改层权重：`dur` 秒内线性+平滑过渡，游戏里淡入淡出层都用这个。
    let c = ctx.clone();
    engine.register_fn(
        "fade_anim_layer",
        move |inst: &str, layer: &str, weight: f64, dur: f64| -> bool {
            c.write(|s| s.fade_anim_layer(inst, layer, f32_of(weight), dur as f32))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_mode",
        move |inst: &str, layer: &str, mode: &str| -> bool {
            let Some(mode) = LayerMode::parse(mode) else {
                println!("[script] 未知叠加模式 `{mode}`（可用：override / additive）");
                return false;
            };
            c.write(|s| s.set_anim_layer_mode(inst, layer, mode))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_mask",
        move |inst: &str, layer: &str, spec: &str| -> bool {
            let mask = parse_bone_mask(spec);
            c.write(|s| s.set_anim_layer_mask(inst, layer, mask))
        },
    );

    // 遮罩回显：`"*"` = 全部，其余按 `include,!exclude` 还原，便于脚本自检与日志验收。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_mask",
        move |inst: &str, layer: &str| -> String {
            let s = c.doc.borrow();
            match s.anim_layer(inst, layer) {
                Some(l) if l.mask.is_pass_all() => "*".to_string(),
                Some(l) => {
                    let mut parts = l.mask.include.clone();
                    parts.extend(l.mask.exclude.iter().map(|e| format!("!{e}")));
                    parts.join(",")
                }
                None => String::new(),
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_speed",
        move |inst: &str, layer: &str, speed: f64| -> bool {
            c.write(|s| s.set_anim_layer_speed(inst, layer, f32_of(speed)))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clips",
        move |inst: &str, layer: &str| -> Array {
            c.doc
                .borrow()
                .anim_layer(inst, layer)
                .map(|l| l.clip_names().into_iter().map(Dynamic::from).collect())
                .unwrap_or_default()
        },
    );

    // 层内多片段加权混合：走路层里叠加「走 / 跑」两路片段按权重过渡。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.write(|s| s.add_anim_layer_clip(inst, layer, clip, f32_of(weight)))
        },
    );

    // 改层内某一路片段的权重（混合空间摘掉之后，手写权重从这里接管）。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_weight",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.write(|s| s.set_anim_layer_clip_weight(inst, layer, clip, f32_of(weight)))
        },
    );

    // 把层的片段整体换成指定片段（层内收敛为单片段，权重保留）。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.write(|s| s.set_anim_layer_clip(inst, layer, clip))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "remove_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.write(|s| s.remove_anim_layer_clip(inst, layer, clip))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "remove_anim_layer",
        move |inst: &str, layer: &str| -> bool { c.write(|s| s.remove_anim_layer(inst, layer)) },
    );

    let c = ctx.clone();
    engine.register_fn("clear_anim_layers", move |inst: &str| -> bool {
        c.write(|s| s.clear_anim_layers(inst))
    });

    // ---------------- 1D / 2D / 3D 混合空间（扩展四十五 / 四十九） ----------------
    //
    // 用法：给层挂一个混合空间 → 往里塞 (片段, 参数坐标) 样本 → 每帧推参数值。
    // 层内片段的权重就由「参数落在样本图上的位置」算出来，脚本不用再手算权重。
    // 三维空间（扩展四十九）把「八向移动 × 三档速度」这类原本要两层 2D 叠的表达
    // 收成一张四面体图：多了 `_3d` 三个入口，二维入口在三维空间上仍可用（前两维）。

    // 挂一维混合空间：`param` 只是参数名（日志/回显用），例如 "speed"。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_1d",
        move |inst: &str, layer: &str, param: &str| -> bool {
            c.write(|s| s.set_anim_layer_blend_space_1d(inst, layer, param))
        },
    );

    // 挂二维混合空间：`px` / `py` 两个参数名，例如 "move.x" / "move.y"。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_2d",
        move |inst: &str, layer: &str, px: &str, py: &str| -> bool {
            c.write(|s| s.set_anim_layer_blend_space_2d(inst, layer, px, py))
        },
    );

    // 挂三维混合空间（扩展四十九）：`px` / `py` / `pz` 三个参数名。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_3d",
        move |inst: &str, layer: &str, px: &str, py: &str, pz: &str| -> bool {
            c.write(|s| s.set_anim_layer_blend_space_3d(inst, layer, px, py, pz))
        },
    );

    // 摘掉空间，层回到手写权重（脚本里做「动态混合 / 硬编码权重」切换时用）。
    let c = ctx.clone();
    engine.register_fn(
        "clear_anim_layer_blend",
        move |inst: &str, layer: &str| -> bool {
            c.write(|s| s.clear_anim_layer_blend_space(inst, layer))
        },
    );

    // 加样本：`x` / `y` 是这块片段在参数空间里的坐标（一维只认 x）。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_blend_sample",
        move |inst: &str, layer: &str, clip: &str, x: f64, y: f64| -> bool {
            c.write(|s| {
                s.add_anim_layer_blend_sample(inst, layer, clip, [x as f32, y as f32])
                    .unwrap_or(false)
            })
        },
    );

    // 加三维样本（扩展四十九）：`x` / `y` / `z` 是片段在参数空间里的坐标。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_blend_sample_3d",
        move |inst: &str, layer: &str, clip: &str, x: f64, y: f64, z: f64| -> bool {
            c.write(|s| {
                s.add_anim_layer_blend_sample_3d(inst, layer, clip, [x as f32, y as f32, z as f32])
                    .unwrap_or(false)
            })
        },
    );

    // 每帧推参数值（一维只认 x）：权重随参数插值 / 外插钳制在内部完成。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_value",
        move |inst: &str, layer: &str, x: f64, y: f64| -> bool {
            c.write(|s| {
                s.set_anim_layer_blend_value(inst, layer, x as f32, y as f32)
                    .unwrap_or(false)
            })
        },
    );

    // 三维推值（扩展四十九）：二维入口在三维空间上等价于 z=0，要第三维就用这个。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_value_3d",
        move |inst: &str, layer: &str, x: f64, y: f64, z: f64| -> bool {
            c.write(|s| {
                s.set_anim_layer_blend_value_3d(inst, layer, x as f32, y as f32, z as f32)
                    .unwrap_or(false)
            })
        },
    );

    // 回显：`3d(move.x,move.y,speed) n=24 tet=28 drop=0 v=(0.00,1.00,1.00) w=...`，没挂空间返回空串。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend",
        move |inst: &str, layer: &str| -> String {
            let s = c.doc.borrow();
            let Some(brief) = s.anim_layer_blend_brief(inst, layer) else {
                return String::new();
            };
            let line = s
                .anim_layer_blend_weights(inst, layer)
                .unwrap_or_default()
                .iter()
                .map(|(name, w)| format!("{name}={w:.2}"))
                .collect::<Vec<_>>()
                .join(",");
            format!("{brief} w={line}")
        },
    );

    // 剖分挂起标记（扩展五十一）：批量 `add_anim_layer_blend_sample*` 之后为 `true`，
    // 下一帧 advance（或显式 `build_anim_layer_blend`）之后回到 `false`。
    // 没挂空间返回 `false`——探针里不用先判空间存不存在。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend_stale",
        move |inst: &str, layer: &str| -> bool {
            c.doc
                .borrow()
                .anim_layer_blend_stale(inst, layer)
                .unwrap_or(false)
        },
    );

    // 剖分提交账本（扩展五十三）：回显 `"inc=3 full=1"`（增量续算次数 / 整表重剖次数）。
    // 没挂空间返回空串——探针里不用先判空间存不存在，空串天然不等于任何期望值。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend_build_stats",
        move |inst: &str, layer: &str| -> String {
            match c.doc.borrow().anim_layer_blend_build_stats(inst, layer) {
                Some((inc, full)) => format!("inc={inc} full={full}"),
                None => String::new(),
            }
        },
    );

    // 立刻提交挂起的剖分（扩展五十一）：`true` = 这次真的重建了，`false` = 本来就最新
    // （编辑器装配完马上要画线框 / 探针要读剖分表时用，省得等一帧）。
    let c = ctx.clone();
    engine.register_fn(
        "build_anim_layer_blend",
        move |inst: &str, layer: &str| -> bool {
            c.write(|s| {
                s.build_anim_layer_blend_pending(inst, layer)
                    .unwrap_or(false)
            })
        },
    );

    // 按样本设速率（扩展五十二）：`clip` 在该层空间里有样本才改得动（`true`）。
    // 改了之后层时间推进 = 层 speed × 「各样本速率按当前权重的加权平均」，
    // 于是走→跑的步频跟着参数连续变化，而不是两段硬切。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_sample_speed",
        move |inst: &str, layer: &str, clip: &str, speed: f64| -> bool {
            c.write(|s| {
                s.set_anim_layer_blend_sample_speed(inst, layer, clip, speed as f32)
                    .unwrap_or(false)
            })
        },
    );

    // 当前按样本插值出的层速率倍率（扩展五十二）。没挂空间返回 1.0（C7 探针不用先判存在）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend_speed",
        move |inst: &str, layer: &str| -> f64 {
            c.doc
                .borrow()
                .anim_layer_blend_speed(inst, layer)
                .unwrap_or(1.0) as f64
        },
    );
}
