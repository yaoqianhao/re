//! 音频（非空间化 / 3D 空间化）。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「音频（非空间化 / 3D 空间化）」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ------------------------------------------------------------------
    // 音频（非空间化）：只做常规音效 / 音乐，没有 3D 衰减与多普勒。
    //
    // 统一约定：
    // * 参数 key 是调用方自己起的**声音名字**（不是文件路径），同名 key 重播 = 停掉旧实例从头播，
    //   不会叠加第二份；想同时放两份同一文件，就起两个 key。
    // * 所有函数都不 panic：无设备 / 文件打不开 / 解码失败 一律返回 false，并把原因留在
    //   `audio_last_error()`，主循环继续跑。
    // ------------------------------------------------------------------

    let c = ctx.clone();
    engine.register_fn("audio_play", move |key: &str, path: &str| -> bool {
        c.audio.borrow_mut().play_default(key, path).is_ok()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_play_ex",
        move |key: &str, path: &str, volume: f64, looping: bool| -> bool {
            c.audio
                .borrow_mut()
                .play(key, path, f32_of(volume), looping)
                .is_ok()
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_stop", move |key: &str| -> bool {
        c.audio.borrow_mut().stop(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_stop_all", move || {
        c.audio.borrow_mut().stop_all();
    });

    let c = ctx.clone();
    engine.register_fn("audio_pause", move |key: &str| -> bool {
        c.audio.borrow_mut().pause(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_resume", move |key: &str| -> bool {
        c.audio.borrow_mut().resume(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_set_volume", move |key: &str, volume: f64| -> bool {
        c.audio.borrow_mut().set_volume(key, f32_of(volume))
    });

    let c = ctx.clone();
    engine.register_fn("audio_set_loop", move |key: &str, looping: bool| -> bool {
        c.audio.borrow_mut().set_loop(key, looping)
    });

    let c = ctx.clone();
    engine.register_fn("audio_is_playing", move |key: &str| -> bool {
        c.audio.borrow_mut().is_playing(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_is_paused", move |key: &str| -> bool {
        c.audio.borrow().is_paused(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_count", move || -> i64 {
        c.audio.borrow().active_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_keys", move || -> Array {
        c.audio
            .borrow()
            .keys()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    // 总音量：与单条音量相乘后下发给混音器，0 也合法（等于静音）。
    let c = ctx.clone();
    engine.register_fn("audio_set_master_volume", move |volume: f64| {
        c.audio.borrow_mut().set_master_volume(f32_of(volume));
    });

    let c = ctx.clone();
    engine.register_fn("audio_master_volume", move || -> f64 {
        c.audio.borrow().master_volume() as f64
    });

    // 总开关：关掉后新建的播放请求直接拒绝（返回 false），已有声音全部停掉。
    let c = ctx.clone();
    engine.register_fn("audio_set_enabled", move |on: bool| {
        c.audio.borrow_mut().set_enabled(on);
    });

    let c = ctx.clone();
    engine.register_fn("audio_enabled", move || -> bool {
        c.audio.borrow().enabled()
    });

    // 探针：有真实设备 = true；静默降级 = false。脚本可据此不显示音量控件。
    let c = ctx.clone();
    engine.register_fn("audio_available", move || -> bool {
        c.audio.borrow().device_available()
    });

    let c = ctx.clone();
    engine.register_fn("audio_backend", move || -> String {
        c.audio.borrow().backend_name().to_string()
    });

    let c = ctx.clone();
    engine.register_fn("audio_last_error", move || -> String {
        c.audio.borrow().last_error().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("audio_error_count", move || -> i64 {
        c.audio.borrow().error_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_status", move || -> String {
        c.audio.borrow_mut().status_line()
    });

    // ------------------------------------------------------------------
    // 音频（3D 空间化，§12 第十项）
    //
    // 与上面 21 个非空间化 API 的关系是**纯叠加**：不调 `audio_spatial_attach*`
    // 的声音就是平面声，音量与历史版本逐位一致；调了之后覆盖层才给它乘距离衰减、
    // 把多普勒算出来下发成播放倍速。
    //
    // 约定：
    // * key 先 `audio_spatial_attach*` 再 `audio_play*`（注册前那一帧还是平面声）；
    // * 位置可由 `audio_spatial_set_position` 直接给，也可 `audio_spatial_attach_node`
    //   绑到场景节点，由 `app` 每帧解析节点世界坐标后灌进来；
    // * 听者默认在原点朝 -Z；`app` 每帧按**活动相机**覆盖，脚本一般不用手动设；
    // * 全程不 panic：参数非法一律收敛（NaN 坐标退原点、min>=max 自动拉开），
    //   无设备时多普勒只算不下发。
    // ------------------------------------------------------------------

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach",
        move |key: &str, min_distance: f64, max_distance: f64| -> bool {
            c.audio.borrow_mut().attach_spatial(
                key,
                SpatialSettings::new(f32_of(min_distance), f32_of(max_distance)),
            )
        },
    );

    // 带模型与多普勒强度的完整版。model 取 inverse / linear / exponential
    //（大小写不敏感，也接受 inv / lin / exp），认不出来就回退 inverse。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach_ex",
        move |key: &str,
              min_distance: f64,
              max_distance: f64,
              model: &str,
              doppler_factor: f64|
              -> bool {
            let mut settings = SpatialSettings::new(f32_of(min_distance), f32_of(max_distance));
            settings.model = DistanceModel::parse(model).unwrap_or_default();
            settings.doppler_factor = f32_of(doppler_factor);
            c.audio.borrow_mut().attach_spatial(key, settings)
        },
    );

    // 注册并绑到场景节点：位置由 app 每帧从节点世界变换取，脚本只管调参。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach_node",
        move |key: &str, node: &str, min_distance: f64, max_distance: f64| -> bool {
            c.audio.borrow_mut().attach_spatial_node(
                key,
                node,
                SpatialSettings::new(f32_of(min_distance), f32_of(max_distance)),
            )
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_detach", move |key: &str| -> bool {
        c.audio.borrow_mut().detach_spatial(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_clear", move || {
        c.audio.borrow_mut().clear_spatial();
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_count", move || -> i64 {
        c.audio.borrow().spatial_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_keys", move || -> Array {
        c.audio
            .borrow()
            .spatial_keys()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_position",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.audio
                .borrow_mut()
                .set_source_position(key, [f32_of(x), f32_of(y), f32_of(z)])
        },
    );

    // 声源世界坐标 [x, y, z]；未注册返回空数组（脚本可据此判存在）。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_position", move |key: &str| -> Array {
        match c.audio.borrow().source_position(key) {
            Some(p) => p.iter().map(|v| Dynamic::from(*v as f64)).collect(),
            None => Array::new(),
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_model",
        move |key: &str, model: &str| -> bool {
            let Some(model) = DistanceModel::parse(model) else {
                return false;
            };
            let audio = c.audio.borrow();
            let Some(mut settings) = audio.spatial_settings(key) else {
                return false;
            };
            drop(audio);
            settings.model = model;
            c.audio.borrow_mut().set_spatial_settings(key, settings)
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_model", move |key: &str| -> String {
        c.audio
            .borrow()
            .spatial_settings(key)
            .map(|s| s.model.name().to_string())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_doppler",
        move |key: &str, factor: f64| -> bool {
            let audio = c.audio.borrow();
            let Some(mut settings) = audio.spatial_settings(key) else {
                return false;
            };
            drop(audio);
            settings.doppler_factor = f32_of(factor);
            c.audio.borrow_mut().set_spatial_settings(key, settings)
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_doppler", move |key: &str| -> f64 {
        c.audio
            .borrow()
            .spatial_settings(key)
            .map(|s| s.doppler_factor as f64)
            .unwrap_or(1.0)
    });

    // 探针三件套：当前增益 / 倍速 / 距离（未注册时增益倍速为 1.0、距离为 -1）。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_gain", move |key: &str| -> f64 {
        c.audio.borrow().spatial_gain(key) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_rate", move |key: &str| -> f64 {
        c.audio.borrow().spatial_rate(key) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_distance", move |key: &str| -> f64 {
        c.audio.borrow().spatial_distance(key).unwrap_or(-1.0) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_node", move |key: &str| -> String {
        c.audio
            .borrow()
            .spatial_node(key)
            .map(str::to_string)
            .unwrap_or_default()
    });

    // 手动喂听者位姿：position(3) + forward(3)。forward 传零向量会被退化成正朝 -Z。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_listener",
        move |x: f64, y: f64, z: f64, fx: f64, fy: f64, fz: f64| {
            let pose = r3e_audio::ListenerPose::new(
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(fx), f32_of(fy), f32_of(fz)],
            );
            c.audio.borrow_mut().set_listener(pose);
        },
    );

    // 听者位姿 [px, py, pz, fx, fy, fz]。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_listener", move || -> Array {
        let pose = c.audio.borrow().listener();
        pose.position
            .iter()
            .chain(pose.forward.iter())
            .map(|v| Dynamic::from(*v as f64))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_set_enabled", move |on: bool| {
        c.audio.borrow_mut().set_spatial_enabled(on);
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_enabled", move || -> bool {
        c.audio.borrow().spatial_enabled()
    });

    // 手动推进一帧空间化（返回推进的声源数）。app 每帧自动调，脚本一般不需要；
    // 留着是为了无窗口测试与"脚本自己控制时间流速"的玩法。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_tick", move |dt: f64| -> i64 {
        c.audio.borrow_mut().tick_spatial(f32_of(dt)) as i64
    });

    // 探针：后端能否实时变速（多普勒落地前提）。静默降级时 false。
    let c = ctx.clone();
    engine.register_fn("audio_supports_speed", move || -> bool {
        c.audio.borrow().supports_speed()
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_status", move || -> String {
        c.audio.borrow().spatial_status_line()
    });
}
