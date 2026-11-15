//! 相机 / 光照 / 环境探针 / HDR 立方体反射。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「相机 / 光照 / 环境探针 / HDR 立方体反射」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 相机 / 光照 / 环境 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "camera",
        move |px: f64, py: f64, pz: f64, tx: f64, ty: f64, tz: f64| {
            c.write(|s| {
                s.camera.position = Vec3::new(f32_of(px), f32_of(py), f32_of(pz));
                s.camera.target = Vec3::new(f32_of(tx), f32_of(ty), f32_of(tz));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("orbit", move |yaw: f64, pitch: f64| {
        c.write(|s| s.camera.orbit(f32_of(yaw), f32_of(pitch)));
    });

    let c = ctx.clone();
    engine.register_fn("fov", move |deg: f64| {
        c.write(|s| s.camera.fov_deg = f32_of(deg));
    });

    let c = ctx.clone();
    engine.register_fn("set_light", move |dx: f64, dy: f64, dz: f64| {
        c.write(|s| {
            let d = Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
            if d.length_squared() > 1e-8 {
                s.light.direction = d.normalize();
            }
        })
    });

    let c = ctx.clone();
    engine.register_fn("set_light_color", move |r: f64, g: f64, b: f64, i: f64| {
        c.write(|s| {
            s.light.color = Vec3::new(f32_of(r), f32_of(g), f32_of(b));
            s.light.intensity = f32_of(i);
        })
    });

    let c = ctx.clone();
    engine.register_fn("set_ambient", move |r: f64, g: f64, b: f64| {
        c.write(|s| s.light.ambient = Vec3::new(f32_of(r), f32_of(g), f32_of(b)));
    });

    // ---------------- 环境探针（IBL，扩展十二） ----------------
    // 语义：`scene.environment` 为 `None` 时走 `set_ambient` 的常数项（老场景行为不变）；
    // 一旦调用 set_sky / set_env_sun / set_env_intensity，就切到环境探针：
    // 漫反射由 9 项球谐给出方向感，金属靠环境高光不再死黑。
    // 所有写入都经 `sanitized()`，脚本传 NaN / 天文数字不会污染渲染。

    let c = ctx.clone();
    engine.register_fn(
        "set_sky",
        move |zr: f64, zg: f64, zb: f64, hr: f64, hg: f64, hb: f64, gr: f64, gg: f64, gb: f64| {
            c.write(|s| {
                let mut env = s.environment.unwrap_or_default();
                env.sky_zenith = [f32_of(zr), f32_of(zg), f32_of(zb)];
                env.sky_horizon = [f32_of(hr), f32_of(hg), f32_of(hb)];
                env.ground = [f32_of(gr), f32_of(gg), f32_of(gb)];
                s.environment = Some(env.sanitized());
            })
        },
    );

    // 太阳项：颜色 + 强度 + 指向太阳的方向。方向为零向量时保留原值（与 set_light 同规矩）。
    let c = ctx.clone();
    engine.register_fn(
        "set_env_sun",
        move |r: f64, g: f64, b: f64, intensity: f64, dx: f64, dy: f64, dz: f64| {
            c.write(|s| {
                let mut env = s.environment.unwrap_or_default();
                env.sun_color = [f32_of(r), f32_of(g), f32_of(b)];
                env.sun_intensity = f32_of(intensity);
                let d = Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
                if d.length_squared() > 1e-8 {
                    let n = d.normalize();
                    env.sun_dir = [n.x, n.y, n.z];
                }
                s.environment = Some(env.sanitized());
            })
        },
    );

    // 环境总强度：整体乘在天空 / 地面 / 太阳项上（1.0 = 物理量原值）。
    let c = ctx.clone();
    engine.register_fn("set_env_intensity", move |v: f64| {
        c.write(|s| {
            let mut env = s.environment.unwrap_or_default();
            env.intensity = f32_of(v);
            s.environment = Some(env.sanitized());
        })
    });

    // 关掉环境探针，退回 `light.ambient` 常数项。
    let c = ctx.clone();
    engine.register_fn("clear_environment", move || {
        c.write(|s| s.environment = None);
    });

    let c = ctx.clone();
    engine.register_fn("has_environment", move || -> bool {
        c.doc.borrow().environment.is_some()
    });

    // ---------------- HDR 立方体反射探针（扩展二十三） ----------------
    // 语义：`scene.environment_map` 为 `None` / 未激活时，镜面 IBL 仍走上面那套解析球谐，
    // 画面逐像素不变；一旦写入来源（等距柱状单图或立方体六面），金属 / 光滑材质改用
    // 预滤波 mip 链采样。与 set_sky 一族同规矩：每次写入都过 `sanitized()`。

    // 等距柱状单图（.hdr）。空字符串视为「没写」，整段探针被清掉。
    let c = ctx.clone();
    engine.register_fn("set_env_map", move |path: &str| -> bool {
        let settings = EnvironmentMapSettings::with_equirect(path);
        c.write(|s| s.set_environment_map(settings).is_active())
    });

    // 立方体六面：按 +X -X +Y -Y +Z -Z 顺序给 6 条路径，少于 6 条整段作废（返回 false）。
    let c = ctx.clone();
    engine.register_fn("set_env_map_faces", move |paths: Array| -> bool {
        let faces: Vec<String> = paths
            .iter()
            .map(|d| d.clone().into_string().unwrap_or_default())
            .collect();
        let settings = EnvironmentMapSettings::with_faces(faces);
        c.write(|s| s.set_environment_map(settings).is_active())
    });

    let c = ctx.clone();
    engine.register_fn("set_env_map_intensity", move |v: f64| -> f64 {
        c.write(|s| {
            let mut settings = s.environment_map.clone().unwrap_or_default();
            settings.intensity = f32_of(v);
            s.set_environment_map(settings).intensity as f64
        })
    });

    let c = ctx.clone();
    engine.register_fn("set_env_map_enabled", move |on: bool| -> bool {
        c.write(|s| s.set_environment_map_enabled(on))
    });

    let c = ctx.clone();
    engine.register_fn("clear_env_map", move || -> bool {
        c.write(|s| s.clear_environment_map())
    });

    let c = ctx.clone();
    engine.register_fn("has_env_map", move || -> bool {
        c.doc.borrow().environment_map_active()
    });

    // 诊断出口：来源标签（none / disabled / equirect / faces），编辑器面板可直接显示。
    let c = ctx.clone();
    engine.register_fn("env_map_source", move || -> String {
        c.doc.borrow().env_map_source()
    });

    let c = ctx.clone();
    engine.register_fn("env_map_intensity", move || -> f64 {
        c.doc
            .borrow()
            .environment_map
            .as_ref()
            .map_or(1.0, |s| s.intensity as f64)
    });
}
