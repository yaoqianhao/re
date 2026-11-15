//! HDR 后处理链 / 屏幕空间环境光遮蔽。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「HDR 后处理链 / 屏幕空间环境光遮蔽」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- HDR 后处理链（扩展十三） ----------------
    // 语义：`scene.postfx` 为 `None` 时整条链退化成「离屏 HDR 目标 + composite 直通」，
    // 画面与不开启时一致；任何 set_postfx* 调用都会把这一段实体化。
    // 与 set_sky 一族同规矩：每次写入都过 `sanitized()`，
    // 脚本里算出来的 NaN / 超界值不会一路灌进 uniform。

    // 一击式全量设置。`tonemap` 取 "none" / "reinhard" / "aces"，未知名字按 none
    // 处理并打印一行说明——比静默忽略更容易在编辑器里定位脚本写错。
    let c = ctx.clone();
    engine.register_fn(
        "set_postfx",
        move |exposure: f64, tonemap: &str, bloom: bool, intensity: f64, fxaa: bool| {
            let mode = match TonemapMode::from_name(tonemap) {
                Some(m) => m,
                None => {
                    println!("[script] set_postfx: 未知色调映射 `{tonemap}`，按 none 处理");
                    TonemapMode::None
                }
            };
            c.write(|s| {
                let mut fx = s.postfx.unwrap_or_default();
                fx.exposure = f32_of(exposure);
                fx.tonemap = mode;
                fx.bloom.enabled = bloom;
                fx.bloom.intensity = f32_of(intensity);
                fx.fxaa = fxaa;
                s.postfx = Some(fx.sanitized());
            })
        },
    );

    // 曝光（EV）。正数提亮、负数压暗，范围由 core 夹在 [-8, 16]。
    let c = ctx.clone();
    engine.register_fn("set_exposure", move |ev: f64| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.exposure = f32_of(ev);
            s.postfx = Some(fx.sanitized());
        })
    });

    // 换色调映射曲线。
    let c = ctx.clone();
    engine.register_fn("set_tonemap", move |name: &str| {
        let mode = match TonemapMode::from_name(name) {
            Some(m) => m,
            None => {
                println!("[script] set_tonemap: 未知色调映射 `{name}`，按 none 处理");
                TonemapMode::None
            }
        };
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.tonemap = mode;
            s.postfx = Some(fx.sanitized());
        })
    });

    // 泛光：开关 + 阈值 + 软膝 + 强度。阈值小于 1.0 会让整片天空都发光，
    // 想要「只有高光溢出」就保持 ≥ 1.0。
    let c = ctx.clone();
    engine.register_fn(
        "set_bloom",
        move |enabled: bool, threshold: f64, knee: f64, intensity: f64| {
            c.write(|s| {
                let mut fx = s.postfx.unwrap_or_default();
                fx.bloom.enabled = enabled;
                fx.bloom.threshold = f32_of(threshold);
                fx.bloom.knee = f32_of(knee);
                fx.bloom.intensity = f32_of(intensity);
                s.postfx = Some(fx.sanitized());
            })
        },
    );

    // FXAA：在 composite 里做一次边缘内插，弥补没有 MSAA 的锯齿。
    let c = ctx.clone();
    engine.register_fn("set_fxaa", move |enabled: bool| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.fxaa = enabled;
            s.postfx = Some(fx.sanitized());
        })
    });

    // 关掉后处理段，退回直通。
    let c = ctx.clone();
    engine.register_fn("clear_postfx", move || {
        c.write(|s| s.postfx = None);
    });

    let c = ctx.clone();
    engine.register_fn("has_postfx", move || -> bool {
        c.doc.borrow().postfx.is_some()
    });

    // 当前生效值快照（脚本侧断言 / 编辑器回读用），未开启时给出缺省值。
    let c = ctx.clone();
    engine.register_fn("postfx_exposure", move || -> f64 {
        f64::from(c.doc.borrow().postfx.unwrap_or_default().exposure)
    });

    let c = ctx.clone();
    engine.register_fn("postfx_tonemap", move || -> String {
        c.doc
            .borrow()
            .postfx
            .unwrap_or_default()
            .tonemap
            .name()
            .to_string()
    });

    // ---------------- 屏幕空间环境光遮蔽（扩展十七） ----------------
    // 遮蔽图挂在后处理段下（`postfx.ssao`），所以这一族与 `set_postfx` 同规矩：
    // 段不存在就先实体化成缺省值再写——不让脚本为了开个遮蔽而先手动开一遍后处理。
    // 半径 / 偏置 / 强度在 core 里有硬区间，样本数夹进 [4, 16]；写入一律过
    // `sanitized()`，脚本里算出来的 NaN 不会一路灌进 uniform。

    // 一击式全量设置。`samples` 越大噪点越少、开销越高，8 是缺省平衡点。
    let c = ctx.clone();
    engine.register_fn(
        "set_ssao",
        move |enabled: bool, radius: f64, bias: f64, intensity: f64, samples: i64| {
            c.write(|s| {
                let mut fx = s.postfx.unwrap_or_default();
                fx.ssao.enabled = enabled;
                fx.ssao.radius = f32_of(radius);
                fx.ssao.bias = f32_of(bias);
                fx.ssao.intensity = f32_of(intensity);
                fx.ssao.samples = ssao_samples_of(samples);
                s.postfx = Some(fx.sanitized());
            })
        },
    );

    // 只切开关：已调好的半径 / 强度不动，脚本里对比开关效果时不必重复写参数。
    let c = ctx.clone();
    engine.register_fn("set_ssao_enabled", move |enabled: bool| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.ssao.enabled = enabled;
            s.postfx = Some(fx.sanitized());
        })
    });

    // 单独调半径（世界空间单位，典型 0.2 ~ 1.5）：太小只有缝隙变脏，太大糊成一片。
    let c = ctx.clone();
    engine.register_fn("set_ssao_radius", move |radius: f64| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.ssao.radius = f32_of(radius);
            s.postfx = Some(fx.sanitized());
        })
    });

    // 单独调强度：0 等价于关闭（AO 系数恒为 1），4 是上限。
    let c = ctx.clone();
    engine.register_fn("set_ssao_intensity", move |intensity: f64| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.ssao.intensity = f32_of(intensity);
            s.postfx = Some(fx.sanitized());
        })
    });

    // 单独调样本数：会被夹进 [4, 16]，取偶数更好对齐。
    let c = ctx.clone();
    engine.register_fn("set_ssao_samples", move |samples: i64| {
        c.write(|s| {
            let mut fx = s.postfx.unwrap_or_default();
            fx.ssao.samples = ssao_samples_of(samples);
            s.postfx = Some(fx.sanitized());
        })
    });

    // 关掉遮蔽但**保留**后处理段里的曝光 / 泛光 / FXAA：与 `clear_postfx` 的区别
    // 就在于只掐这一条支线，不把整段打回直通。
    let c = ctx.clone();
    engine.register_fn("clear_ssao", move || {
        c.write(|s| {
            if let Some(mut fx) = s.postfx {
                fx.ssao.enabled = false;
                s.postfx = Some(fx);
            }
        })
    });

    // 只有「段存在且开了且强度非零」才算真的在算遮蔽——强度 0 时 GPU 那边整帧跳过。
    let c = ctx.clone();
    engine.register_fn("has_ssao", move || -> bool {
        c.doc
            .borrow()
            .postfx
            .map(|fx| !fx.ssao.is_neutral())
            .unwrap_or(false)
    });

    // 当前生效值快照（脚本侧断言 / 编辑器回读用），未开启时给出缺省值。
    let c = ctx.clone();
    engine.register_fn("ssao_enabled", move || -> bool {
        c.doc
            .borrow()
            .postfx
            .map(|fx| fx.ssao.enabled)
            .unwrap_or(false)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_radius", move || -> f64 {
        f64::from(c.doc.borrow().postfx.unwrap_or_default().ssao.radius)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_bias", move || -> f64 {
        f64::from(c.doc.borrow().postfx.unwrap_or_default().ssao.bias)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_intensity", move || -> f64 {
        f64::from(c.doc.borrow().postfx.unwrap_or_default().ssao.intensity)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_samples", move || -> i64 {
        i64::from(c.doc.borrow().postfx.unwrap_or_default().ssao.samples)
    });
}
