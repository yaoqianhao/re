//! 核心链路：引擎信息 / 场景读写 / 保存 / 截图 / 后处理 / 环境探针。
//!
//! 由 M0-S2 从 `api.rs` 内 `mod tests` 原样切出，断言未改动。

use super::*;

#[test]
fn info_reports_scene_and_physics() {
    let (scene, physics, mut scripts) = fixture();
    let info = api(&scene, &physics, &mut scripts).info();
    assert_eq!(info.name, "r3e");
    assert_eq!(info.protocol, PROTOCOL_VERSION);
    assert_eq!(info.nodes, 3);
    assert_eq!(info.frames, 42);
    assert_eq!(info.scene, "assets/scene.json");
    assert!(!info.physics.backend.is_empty());
}

#[test]
fn scene_list_filters_and_limits() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let all = api.scene_list(None, None).expect("应成功");
    assert_eq!(all.len(), 3);

    let cubes = api.scene_list(Some("cube"), None).expect("应成功");
    assert_eq!(cubes.len(), 2);
    assert!(cubes.iter().all(|n| n.name.contains("cube")));

    // 大小写不敏感
    let upper = api.scene_list(Some("CUBE"), None).expect("应成功");
    assert_eq!(upper.len(), 2);

    let limited = api.scene_list(None, Some(2)).expect("应成功");
    assert_eq!(limited.len(), 2);
}

#[test]
fn set_transform_writes_local_transform() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let patched = api
        .scene_set_transform(
            "cube",
            TransformPatch {
                position: Some([1.0, 2.0, 3.0]),
                rotation: Some([0.0, 0.0, 0.0, 1.0]),
                scale: Some([2.0, 2.0, 2.0]),
            },
        )
        .expect("应成功");
    assert_eq!(patched["position"], json!([1.0, 2.0, 3.0]));

    let node = scene.borrow().find("cube").expect("物件应存在").clone();
    assert_eq!(node.transform.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(node.transform.scale, Vec3::splat(2.0));
    // 世界矩阵已按新局部变换刷新
    assert!((node.world.w_axis.x - 1.0).abs() < 1e-6);
}

#[test]
fn set_transform_rejects_bad_input() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let zero_quat = api.scene_set_transform(
        "cube",
        TransformPatch {
            rotation: Some([0.0, 0.0, 0.0, 0.0]),
            ..Default::default()
        },
    );
    assert!(zero_quat.is_err(), "全零四元数必须被拒绝");

    let missing = api.scene_set_transform(
        "not-there",
        TransformPatch {
            position: Some([0.0, 0.0, 0.0]),
            ..Default::default()
        },
    );
    assert!(missing.is_err(), "不存在的物件必须报错");
}

#[test]
fn scene_save_writes_requested_path() {
    let (scene, physics, mut scripts) = fixture();
    let expected = scene.borrow().len();
    let dir = std::env::temp_dir().join(format!("r3e-editor-save-{}", std::process::id()));
    let path = dir.join("nested/out.json");
    let literal = path.display().to_string().replace('\\', "/");

    let mut api = api(&scene, &physics, &mut scripts);
    let saved = api.scene_save(Some(&literal)).expect("显式路径保存应成功");
    assert_eq!(saved["objects"], json!(expected), "物件数应与场景一致");
    assert!(saved["bytes"].as_u64().unwrap_or(0) > 0, "应报告写入字节数");
    let reported = saved["path"].as_str().unwrap_or_default();
    assert!(
        !reported.contains(r"\\?\"),
        "回报路径不应带 Windows 逐字前缀: {reported}"
    );
    assert!(
        reported.ends_with("out.json"),
        "回报路径应是绝对路径: {reported}"
    );

    let text = std::fs::read_to_string(&path).expect("文件应已落盘");
    assert!(text.contains("cube"), "落盘内容应含物件名");
    assert!(!text.contains("world"), "不应写入运行时派生状态");
    let back = r3e_doc::parse_scene(&text).expect("落盘内容必须能回读");
    assert_eq!(back.len(), expected, "回读后物件数应一致");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scene_save_without_path_needs_label() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.scene_label = String::new();
    let err = api
        .scene_save(None)
        .expect_err("无路径且无场景文件时应报错");
    assert!(
        format!("{err:?}").contains("path"),
        "错误信息应提示需要 path: {err:?}"
    );

    // 空白路径等同未给
    assert!(api.scene_save(Some("   ")).is_err(), "空白 path 应被拒绝");
}

#[test]
fn spawn_and_remove_roundtrip() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let spawned = api
        .scene_spawn(json!({ "name": "probe", "mesh": "sphere", "position": [0.0, 1.0, 0.0] }))
        .expect("应成功");
    assert_eq!(spawned["name"], "probe");
    assert_eq!(spawned["nodes"], 4);

    let snapshot = api.scene_get("probe").expect("应成功");
    assert_eq!(snapshot["mesh"], "sphere");
    assert_eq!(snapshot["position"], json!([0.0, 1.0, 0.0]));

    // 重名必须报错，不能悄悄覆盖
    assert!(api
        .scene_spawn(json!({ "name": "probe", "mesh": "cube" }))
        .is_err());

    let removed = api.scene_remove("probe").expect("应成功");
    assert_eq!(removed["removed"], "probe");
    assert_eq!(scene.borrow().len(), 3);
}

#[test]
fn script_eval_returns_json() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    assert_eq!(api.script_eval("1 + 1").expect("应成功"), json!(2));
    assert_eq!(api.script_eval("\"r3e\"").expect("应成功"), json!("r3e"));
    assert_eq!(api.script_eval("()").expect("应成功"), Value::Null);
    assert!(api.script_eval("this is not rhai").is_err());
    // 脚本侧改场景后，scene.* 立即可见（同一个 Scene 实例）
    api.script_eval("create_node_at_origin(\"from_script\", \"cube\")")
        .expect("应成功");
    assert!(scene.borrow().find("from_script").is_some());
}

#[test]
fn set_postfx_merges_and_sanitizes() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    // 逐项给：未给的字段保持原值（首次从未开启的缺省起步）。
    let first = api
        .scene_set_postfx(PostFxPatch {
            exposure: Some(0.5),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(first["enabled"], json!(true));
    assert_eq!(first["postfx"]["exposure"], json!(0.5));
    assert_eq!(first["postfx"]["tonemap"], json!("none"));

    let second = api
        .scene_set_postfx(PostFxPatch {
            tonemap: Some("aces".into()),
            fxaa: Some(true),
            ..Default::default()
        })
        .expect("应成功");
    // 曝光保留上一次的值，不被这次补丁清零。
    assert_eq!(second["postfx"]["exposure"], json!(0.5));
    assert_eq!(second["postfx"]["tonemap"], json!("aces"));
    assert_eq!(second["postfx"]["fxaa"], json!(true));

    // 越界值收敛到上界，而不是写进 uniform。
    let clamped = api
        .scene_set_postfx(PostFxPatch {
            exposure: Some(9999.0),
            bloom_intensity: Some(-4.0),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(
        clamped["postfx"]["exposure"],
        json!(r3e_core::postfx::MAX_EXPOSURE_EV)
    );
    assert_eq!(clamped["postfx"]["bloom"]["intensity"], json!(0.0));

    // 认不出的曲线名报参数错误，且不改动现有设置。
    let bad = api
        .scene_set_postfx(PostFxPatch {
            tonemap: Some("cinematic".into()),
            ..Default::default()
        })
        .expect_err("未知曲线名应报错");
    assert!(
        format!("{bad:?}").contains("cinematic"),
        "错误信息应带上给错的名字: {bad:?}"
    );
    assert_eq!(
        scene.borrow().postfx.expect("postfx 仍在").tonemap,
        r3e_core::postfx::TonemapMode::Aces
    );

    // enabled=false 是唯一的清空入口。
    let cleared = api
        .scene_set_postfx(PostFxPatch {
            enabled: Some(false),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(cleared["postfx"], Value::Null);
    assert!(scene.borrow().postfx.is_none());
}

#[test]
fn script_reload_without_file_is_an_error() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    assert!(api.script_reload().is_err());
}

// ------------------------------------------------------------ P0-4 反馈工具

#[test]
fn engine_stats_without_renderer_reports_null_render() {
    let (scene, physics, mut scripts) = fixture();
    let stats = api(&scene, &physics, &mut scripts)
        .engine_stats()
        .expect("无渲染器时 engine_stats 仍然要给场景快照");

    // 关键契约：没有渲染器就说没有，而不是把 draw_calls 填 0 冒充「渲染了但没画」。
    assert_eq!(stats["renderer"], json!(false));
    assert!(stats["render"].is_null(), "render 应为 null: {stats}");
    assert_eq!(stats["size"]["width"], json!(0));
    assert_eq!(stats["size"]["height"], json!(0));
}

#[test]
fn engine_stats_summarizes_scene_and_frame_time() {
    let (scene, physics, mut scripts) = fixture();
    let stats = api(&scene, &physics, &mut scripts)
        .engine_stats()
        .expect("engine_stats");

    assert_eq!(stats["scene"]["name"], json!("editor-test"));
    assert_eq!(stats["scene"]["objects"], json!(3));
    assert_eq!(stats["scene"]["visible"], json!(3));
    // 三个物件都是 cube：去重后的网格种类是 1，不是 3
    assert_eq!(stats["scene"]["mesh_kinds"], json!(1));
    assert_eq!(stats["scene"]["material_overrides"], json!(0));
    assert_eq!(stats["frame"]["index"], json!(42));
    // fixture 里预置的帧时间分布要原样透传，不能被 fps 覆盖
    assert_eq!(stats["frame"]["frame_ms"]["samples"], json!(4));
    assert_eq!(stats["frame"]["frame_ms"]["p95"], json!(18.5));
}

#[test]
fn capture_frame_without_renderer_reports_unavailable() {
    let (scene, physics, mut scripts) = fixture();
    let err = api(&scene, &physics, &mut scripts)
        .capture_frame(Some("captures/x.png"))
        .expect_err("没有渲染器时截图必须失败");
    // 报错要指向「为什么不能截」，而不是让调用方以为路径写错了
    assert!(err.message.contains("渲染器"), "{}", err.message);
    assert_eq!(err.code, r3e_editor::ENGINE_ERROR);
}

#[test]
fn frame_time_percentiles_use_nearest_rank() {
    // 1..=100 ms：nearest-rank 下 p50=50、p95=95、p99=99、max=100，都是真实观测值
    let samples: Vec<f32> = (1..=100).map(|ms| ms as f32).collect();
    let stats = FrameTimeStats::from_samples(&samples);
    assert_eq!(stats.samples, 100);
    assert_eq!(stats.last_ms, 100.0);
    assert_eq!(stats.p50_ms, 50.0);
    assert_eq!(stats.p95_ms, 95.0);
    assert_eq!(stats.p99_ms, 99.0);
    assert_eq!(stats.max_ms, 100.0);

    // 空样本给全零，避免出现 NaN 分位
    assert_eq!(FrameTimeStats::from_samples(&[]), FrameTimeStats::default());
}

#[test]
fn frame_time_window_wraps_and_ignores_bad_samples() {
    let mut window = FrameTimeWindow::new(4);
    window.push(f32::NAN);
    window.push(-1.0);
    assert_eq!(window.snapshot().samples, 0, "非法样本不应进入缓冲");

    for ms in [10.0, 20.0, 30.0, 40.0] {
        window.push(ms);
    }
    assert_eq!(window.snapshot().samples, 4);
    assert_eq!(window.snapshot().max_ms, 40.0);

    // 满载后覆盖最旧的：10.0 被 5.0 顶掉（样本变 [5,20,30,40]），
    // 于是 p50 从 10.0 抬到 20.0 —— 这就是「旧样本真的被淘汰」的证据。
    window.push(5.0);
    let snapshot = window.snapshot();
    assert_eq!(snapshot.samples, 4);
    assert_eq!(snapshot.last_ms, 5.0, "last 必须是最后推入的样本");
    assert_eq!(snapshot.max_ms, 40.0);
    assert_eq!(snapshot.p50_ms, 20.0);
    assert_eq!(snapshot.p99_ms, 40.0);
}

#[test]
fn default_capture_name_is_png_and_unique() {
    let first = default_capture_name();
    assert!(first.ends_with(".png"), "{first}");
    assert!(first.starts_with("capture_"), "{first}");
    // 同一毫秒内可能相同，这里只验证格式：名字里必须是纯数字时间戳
    let stamp = first
        .trim_start_matches("capture_")
        .trim_end_matches(".png");
    assert!(
        stamp.chars().all(|c| c.is_ascii_digit()),
        "时间戳应为纯数字: {first}"
    );
}

/// 宿主合并语义：段缺失时从缺省起步，未给的字段保留原值，回包给的是**收敛后**的值。
#[test]
fn environment_map_patch_merges_and_reports_source() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let first = api
        .scene_set_environment_map(EnvironmentMapPatch {
            equirect: Some("env/studio.hdr".into()),
            face_size: Some(100),
            ..Default::default()
        })
        .expect("首次设置应成功");
    assert_eq!(first["enabled"], true, "有来源即生效: {first}");
    assert_eq!(first["source"], "equirect");
    // 100 不是 2 的幂，收敛后应向上取到 128（默认值恰好也是 128，这里验证的是「被写成收敛值」）
    assert_eq!(first["environment_map"]["face_size"], 128);

    // 第二次只调强度：路径 / 面边长必须原样保留，不能被默认值覆盖
    let second = api
        .scene_set_environment_map(EnvironmentMapPatch {
            intensity: Some(2.5),
            ..Default::default()
        })
        .expect("局部补丁应成功");
    assert_eq!(second["environment_map"]["equirect"], "env/studio.hdr");
    assert_eq!(second["environment_map"]["intensity"], 2.5);
    assert_eq!(second["environment_map"]["face_size"], 128);

    let map = scene.borrow().environment_map.clone().expect("段应落地");
    assert_eq!(map.face_size, 128);
    assert!(map.is_active());
}

/// 越界参数由宿主先夹到区间、再交给 `sanitized`：负数面边长不得变成 0 或 panic。
#[test]
fn environment_map_patch_clamps_sizes() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let resp = api
        .scene_set_environment_map(EnvironmentMapPatch {
            equirect: Some("env/a.hdr".into()),
            face_size: Some(-8),
            mip_levels: Some(999),
            ..Default::default()
        })
        .expect("越界值应被夹取而不是报错");
    assert_eq!(
        resp["environment_map"]["face_size"], 4,
        "负数落到 4 像素下限"
    );
    assert_eq!(
        resp["environment_map"]["mip_levels"],
        r3e_core::envmap::MAX_SPECULAR_MIPS,
        "层数上限必须夹到 MAX_SPECULAR_MIPS"
    );

    let low = api
        .scene_set_environment_map(EnvironmentMapPatch {
            mip_levels: Some(0),
            ..Default::default()
        })
        .expect("层数 0 应被夹到 1");
    assert_eq!(low["environment_map"]["mip_levels"], 1);
}

/// 六面来源：长度不是 6 时 `sanitized` 判为无效 → 来源清空 → 整段丢弃，
/// 渲染层立刻退回解析 IBL（这是「配错了不会静默用半套图」的关键路径）。
#[test]
fn environment_map_bad_faces_drop_section() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let resp = api
        .scene_set_environment_map(env_map_patch(
            None,
            Some(vec!["px.hdr".into(), "nx.hdr".into()]),
        ))
        .expect("无效来源应给出可解释回包");
    assert_eq!(resp["enabled"], false, "只有 2 面拼不出立方体: {resp}");
    assert_eq!(resp["source"], "none");
    assert_eq!(resp["environment_map"], Value::Null);
    assert!(
        scene.borrow().environment_map.is_none(),
        "来源被判无效时应整段丢弃，而不是留个半残配置"
    );
}

/// 关开关只翻 `enabled`，路径必须留着——这正是「A/B 切换不丢配置」的用法。
#[test]
fn environment_map_disable_keeps_paths() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    api.scene_set_environment_map(env_map_patch(Some("env/studio.hdr"), None))
        .expect("先设置");
    let off = api
        .scene_set_environment_map(EnvironmentMapPatch {
            enabled: Some(false),
            ..Default::default()
        })
        .expect("关开关应成功");
    assert_eq!(off["enabled"], false);
    assert_eq!(off["source"], "disabled");
    assert_eq!(off["environment_map"]["equirect"], "env/studio.hdr");

    let map = scene
        .borrow()
        .environment_map
        .clone()
        .expect("段仍在，只是没启用");
    assert!(!map.is_active());
}

/// 快照字段名必须与场景 JSON 的 `environment_map` 段逐字一致，前端才能直接回写。
#[test]
fn environment_map_snapshot_matches_scene_schema() {
    let settings = r3e_core::envmap::EnvironmentMapSettings::with_equirect("env/x.hdr");
    let value = environment_map_value(settings.sanitized());
    let keys: Vec<&str> = value
        .as_object()
        .expect("快照应为对象")
        .keys()
        .map(String::as_str)
        .collect();
    for key in [
        "enabled",
        "equirect",
        "faces",
        "face_size",
        "mip_levels",
        "intensity",
    ] {
        assert!(keys.contains(&key), "快照缺少字段 `{key}`");
    }
    assert_eq!(keys.len(), 6, "不多不少六个字段，避免回写时多出未知键");
}
