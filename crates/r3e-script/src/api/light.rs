//! 局部光源 / 遮挡剔除。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「局部光源 / 遮挡剔除」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ------------------------------------------------------------------
    // 扩展十五：局部光源（点光 / 聚光）。
    //
    // 约定与 UI 那套一致：**写入走 `light_*`，读取走 `local_light_*`**，
    // 命中判定与数值收敛全部在 `r3e_core::light` 里，脚本层只做参数搬运，
    // 因此编辑器 RPC / 场景 JSON / 脚本三条入口的语义完全同源。
    //
    // 上限 `MAX_LOCAL_LIGHTS`：满载时只拒绝「新增」，同 key 改写照常生效
    // （脚本每帧更新一盏灯的位置不会被静默丢掉）。
    // ------------------------------------------------------------------

    // 添加 / 覆盖一盏点光：位置 + 颜色 + 强度 + 半径。
    let c = ctx.clone();
    engine.register_fn(
        "light_add_point",
        move |key: &str,
              x: f64,
              y: f64,
              z: f64,
              r: f64,
              g: f64,
              b: f64,
              intensity: f64,
              range: f64|
              -> bool {
            let mut light = LocalLight::point(
                key,
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(r), f32_of(g), f32_of(b)],
                f32_of(intensity),
            );
            light.range = f32_of(range);
            c.write(|s| s.upsert_local_light(light))
        },
    );

    // 添加 / 覆盖一盏聚光：位置 + 朝向 + 颜色 + 强度 + 半径 + 内外角（度）。
    let c = ctx.clone();
    engine.register_fn(
        "light_add_spot",
        move |key: &str,
              x: f64,
              y: f64,
              z: f64,
              dx: f64,
              dy: f64,
              dz: f64,
              r: f64,
              g: f64,
              b: f64,
              intensity: f64,
              range: f64,
              inner_deg: f64,
              outer_deg: f64|
              -> bool {
            let light = LocalLight::spot(
                key,
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(dx), f32_of(dy), f32_of(dz)],
                [f32_of(r), f32_of(g), f32_of(b)],
                f32_of(intensity),
                f32_of(range),
                f32_of(inner_deg),
                f32_of(outer_deg),
            );
            c.write(|s| s.upsert_local_light(light))
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_remove", move |key: &str| -> bool {
        c.write(|s| s.remove_local_light(key))
    });

    // 清空全部灯，返回删掉的盏数。
    let c = ctx.clone();
    engine.register_fn("light_clear", move || -> i64 {
        c.write(|s| s.clear_local_lights() as i64)
    });

    // 以下「改单个字段」全部走 `with_local_light`：key 不存在返回 false，
    // 不会悄悄新建一盏灯（拼错名字是最常见的脚本错误）。
    let c = ctx.clone();
    engine.register_fn(
        "light_set_pos",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.write(|s| {
                s.with_local_light(key, |l| {
                    l.pos = [f32_of(x), f32_of(y), f32_of(z)];
                })
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "light_set_color",
        move |key: &str, r: f64, g: f64, b: f64| -> bool {
            c.write(|s| {
                s.with_local_light(key, |l| {
                    l.color = [f32_of(r), f32_of(g), f32_of(b)];
                })
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_set_intensity", move |key: &str, v: f64| -> bool {
        c.write(|s| {
            s.with_local_light(key, |l| {
                l.intensity = f32_of(v);
            })
        })
    });

    let c = ctx.clone();
    engine.register_fn("light_set_range", move |key: &str, v: f64| -> bool {
        c.write(|s| {
            s.with_local_light(key, |l| {
                l.range = f32_of(v);
            })
        })
    });

    let c = ctx.clone();
    engine.register_fn(
        "light_set_dir",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.write(|s| {
                s.with_local_light(key, |l| {
                    l.dir = [f32_of(x), f32_of(y), f32_of(z)];
                })
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "light_set_cone",
        move |key: &str, inner_deg: f64, outer_deg: f64| -> bool {
            c.write(|s| {
                s.with_local_light(key, |l| {
                    l.inner_deg = f32_of(inner_deg);
                    l.outer_deg = f32_of(outer_deg);
                })
            })
        },
    );

    // 扩展十八 C：投影开关与深度偏移。
    //
    // 「会不会真的投影」由渲染层的 `plan_local_shadows` 决定（全世界最多
    // `MAX_LOCAL_SHADOW_CASTERS` 盏、按亮度排序），这里的 `on` 只是**允许**它参与
    // 竞争——所以 `local_light_cast_shadow` 读回来是「意愿」，不是「本帧实际在投影」；
    // 想查本帧有几盏真的在投影，用只读的 `local_shadow_caster_count`。
    let c = ctx.clone();
    engine.register_fn(
        "light_set_cast_shadow",
        move |key: &str, on: bool| -> bool {
            c.write(|s| {
                s.with_local_light(key, |l| {
                    l.cast_shadow = on;
                })
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_set_shadow_bias", move |key: &str, v: f64| -> bool {
        c.write(|s| {
            s.with_local_light(key, |l| {
                l.shadow_bias = f32_of(v);
            })
        })
    });

    // 总开关：关掉只是不上传（场景数据保留），打开即整体恢复。
    let c = ctx.clone();
    engine.register_fn("set_lights_enabled", move |on: bool| {
        c.write(|s| s.lights_enabled = on);
    });

    let c = ctx.clone();
    engine.register_fn("lights_enabled", move || -> bool {
        c.doc.borrow().lights_enabled
    });

    // ---- 遮挡剔除（扩展二十二） ----
    // 总开关只决定「这一帧要不要做遮挡判定」：关掉后渲染层不读回深度、不建金字塔，
    // 提交序列退化成只有视锥剔除；参数与场景数据都保留，再打开即按原参数恢复。
    let c = ctx.clone();
    engine.register_fn("set_occlusion_enabled", move |on: bool| {
        c.write(|s| s.set_occlusion_enabled(on));
    });

    let c = ctx.clone();
    engine.register_fn("occlusion_enabled", move || -> bool {
        c.doc.borrow().occlusion_enabled()
    });

    // 深度偏置（NDC 单位）：偏置越大越保守（越不剔），写入前统一收敛到合法区间。
    let c = ctx.clone();
    engine.register_fn("set_occlusion_bias", move |v: f64| -> f64 {
        c.write(|scene| {
            let mut s = scene.occlusion;
            s.depth_bias = f32_of(v);
            scene.set_occlusion(s).depth_bias as f64
        })
    });

    let c = ctx.clone();
    engine.register_fn("occlusion_bias", move || -> f64 {
        c.doc.borrow().occlusion.depth_bias as f64
    });
}
