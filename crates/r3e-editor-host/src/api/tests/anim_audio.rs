//! 动画层补丁、骨骼掩码与 3D 空间化音频的 RPC 测试。
//!
//! 由 M0-S2 从 `api.rs` 内 `mod tests` 原样切出，断言未改动。

use super::*;

#[test]
fn anim_layers_lists_instances_and_layers() {
    let (scene, physics, mut scripts) = anim_fixture();
    // 层只能由脚本建（层必须绑定片段），编辑器只能读和调参。
    let _ = scripts
        .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0)"#)
        .expect("建层应成功");
    let mut api = api(&scene, &physics, &mut scripts);

    let all = api.scene_anim_layers(None, None).expect("列层应成功");
    assert_eq!(all["count"], json!(1));
    assert_eq!(all["instances"], json!(1), "应报告扫了几个实例");
    assert_eq!(all["limit"], json!(MAX_ANIM_LAYERS));
    let layer = &all["layers"][0];
    assert_eq!(layer["instance"], json!("hero/walk"));
    assert_eq!(layer["layer"], json!("upper"));
    assert_eq!(layer["mode"], json!("override"));
    assert_eq!(layer["mask"], json!("*"), "默认整副骨架");
    assert_eq!(layer["clips"][0]["name"], json!("atk"));
    assert_eq!(layer["playing"], json!(true));

    let one = api
        .scene_anim_layers(Some("hero/walk"), None)
        .expect("点名实例应成功");
    assert_eq!(one["count"], json!(1));

    let missing = api.scene_anim_layers(Some("nobody"), None);
    assert!(missing.is_err(), "点名的实例不存在必须报错");
}

#[test]
fn anim_layer_set_applies_phase_group_fields() {
    let (scene, physics, mut scripts) = anim_fixture();
    for layer in ["upper", "lower"] {
        let _ = scripts
            .eval(&format!(
                r#"add_anim_layer("hero/walk", "{layer}", "atk", 1.0)"#
            ))
            .expect("建层应成功");
    }
    let mut api = api(&scene, &physics, &mut scripts);

    // 只给片段 + 偏移：落到第一路（或指定那一路）的 phase_offset
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            clip: Some("atk".to_string()),
            clip_offset: Some(0.25),
            ..Default::default()
        })
        .expect("补丁应成功");
    let layer = &out["layer"];
    let offset = layer["clips"][0]["phase_offset"]
        .as_f64()
        .expect("每路应带 phase_offset");
    assert!((offset - 0.25).abs() < 1e-6, "偏移应为 0.25，实得 {offset}");

    // 跨层源：让 upper 跟随 lower
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            sync_source: Some("lower".to_string()),
            ..Default::default()
        })
        .expect("跟随应成功");
    assert_eq!(out["layer"]["sync_source"], json!("lower"));

    // 源层不存在 / 自引用：核心层拒，宿主翻成参数错误
    for source in ["ghost", "upper"] {
        let err = api
            .scene_anim_layer_set(AnimLayerPatch {
                instance: "hero/walk".to_string(),
                layer: "upper".to_string(),
                sync_source: Some(source.to_string()),
                ..Default::default()
            })
            .expect_err("应被拒");
        assert_eq!(err.code, r3e_editor::INVALID_PARAMS);
    }
    // 空串 = 清除，跟回自己
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            sync_source: Some(String::new()),
            ..Default::default()
        })
        .expect("清除应成功");
    assert_eq!(out["layer"]["sync_source"], json!(""));
}

#[test]
fn anim_layer_set_applies_clip_playback() {
    let (scene, physics, mut scripts) = anim_fixture();
    for layer in ["upper", "lower"] {
        let _ = scripts
            .eval(&format!(
                r#"add_anim_layer("hero/walk", "{layer}", "atk", 1.0)"#
            ))
            .expect("建层应成功");
    }
    let mut api = api(&scene, &physics, &mut scripts);

    // 速率 + 循环一起给：落到该路，并在快照里回显
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            clip: Some("atk".to_string()),
            clip_speed: Some(2.0),
            clip_looping: Some(false),
            ..Default::default()
        })
        .expect("补丁应成功");
    let clips = &out["layer"]["clips"];
    assert_eq!(clips[0]["clip_speed"], json!(2.0));
    assert_eq!(clips[0]["clip_looping"], json!(false));

    // 只给循环标志：速率沿用当前值，不该被顺手改成 1.0
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            clip_looping: Some(true),
            ..Default::default()
        })
        .expect("补丁应成功");
    assert_eq!(out["layer"]["clips"][0]["clip_speed"], json!(2.0));
    assert_eq!(out["layer"]["clips"][0]["clip_looping"], json!(true));

    // 只给速率（另一层没设过播放参数）：循环沿用默认 true
    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "lower".to_string(),
            clip_speed: Some(0.5),
            ..Default::default()
        })
        .expect("补丁应成功");
    assert_eq!(out["layer"]["clips"][0]["clip_speed"], json!(0.5));
    assert_eq!(out["layer"]["clips"][0]["clip_looping"], json!(true));

    // 真落到了 core 上
    let stored = scene
        .borrow()
        .anim_layer("hero/walk", "upper")
        .cloned()
        .expect("层应存在");
    let pb = stored.clips[0].playback.expect("应带上播放参数");
    assert!((pb.speed - 2.0).abs() < 1e-6);
    assert!(pb.looping, "第二次补丁把循环改回 true");

    // 片段名写错：参数错误，既有状态不变
    let err = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            clip: Some("ghost".to_string()),
            clip_speed: Some(3.0),
            ..Default::default()
        })
        .expect_err("应被拒");
    assert_eq!(err.code, r3e_editor::INVALID_PARAMS);
    let stored = scene
        .borrow()
        .anim_layer("hero/walk", "upper")
        .cloned()
        .expect("层应存在");
    assert!(
        (stored.clips[0].playback.expect("仍在").speed - 2.0).abs() < 1e-6,
        "被拒后速率不该变"
    );
}

#[test]
fn anim_layer_set_patches_and_returns_layer() {
    let (scene, physics, mut scripts) = anim_fixture();
    let _ = scripts
        .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0)"#)
        .expect("建层应成功");
    let mut api = api(&scene, &physics, &mut scripts);

    let out = api
        .scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            weight: Some(0.4),
            mode: Some("additive".to_string()),
            mask: Some("cube,!root".to_string()),
            speed: Some(2.0),
            clip: None,
            sync: Some("phase".to_string()),
            sync_source: None,
            clip_offset: None,
            clip_speed: None,
            clip_looping: None,
        })
        .expect("补丁应成功");
    let layer = &out["layer"];
    // f32→JSON 会有二进制尾差，按近似比。
    let weight = layer["weight"].as_f64().expect("weight 应是数字");
    assert!((weight - 0.4).abs() < 1e-6, "权重应为 0.4，实得 {weight}");
    assert_eq!(layer["mode"], json!("additive"));
    assert_eq!(layer["speed"], json!(2.0));
    assert_eq!(layer["mask_include"], json!(["cube"]));
    assert_eq!(layer["mask_exclude"], json!(["root"]));
    assert_eq!(layer["mask"], json!("cube,!root"), "遮罩应能原样回写成串");
    // 扩展四十六：采样策略随补丁落到 core，并可回读
    assert_eq!(layer["sync"], json!("phase"), "策略应原样回显");
    assert!(layer["phase"].is_number(), "相位应始终可读");

    // 落到了 core 上，而不是只回了个好看的 JSON。
    let stored = scene
        .borrow()
        .anim_layer("hero/walk", "upper")
        .cloned()
        .expect("层应存在");
    assert!((stored.weight - 0.4).abs() < 1e-6);
    assert_eq!(stored.mode, LayerMode::Additive);
    assert!((stored.speed - 2.0).abs() < 1e-6);
    assert_eq!(stored.sync, LayerSync::Phase);
    assert!(stored.mask.contains("cube") && !stored.mask.contains("root"));
}

#[test]
fn anim_layer_set_rejects_unknown_sync_without_side_effect() {
    let (scene, physics, mut scripts) = anim_fixture();
    let _ = scripts
        .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 0.8)"#)
        .expect("建层应成功");
    let mut api = api(&scene, &physics, &mut scripts);

    // 策略名写错：整张补丁都不该落地，权重保持 0.8、策略保持 off。
    let bad = api.scene_anim_layer_set(AnimLayerPatch {
        instance: "hero/walk".to_string(),
        layer: "upper".to_string(),
        weight: Some(0.2),
        sync: Some("warp".to_string()),
        ..Default::default()
    });
    assert!(bad.is_err(), "未知采样策略必须报错");

    let stored = scene
        .borrow()
        .anim_layer("hero/walk", "upper")
        .cloned()
        .expect("层应存在");
    assert!((stored.weight - 0.8).abs() < 1e-6, "权重不该被改");
    assert_eq!(stored.sync, LayerSync::Off, "策略不该被改");
}

#[test]
fn anim_layer_set_rejects_unknown_mode_without_side_effect() {
    let (scene, physics, mut scripts) = anim_fixture();
    let _ = scripts
        .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 0.8)"#)
        .expect("建层应成功");
    let mut api = api(&scene, &physics, &mut scripts);

    let bad = api.scene_anim_layer_set(AnimLayerPatch {
        instance: "hero/walk".to_string(),
        layer: "upper".to_string(),
        weight: Some(0.1),
        mode: Some("multiply".to_string()),
        ..Default::default()
    });
    assert!(bad.is_err(), "未知模式必须报错");

    // 关键：模式写错时，同一次补丁里的权重也不该落地——不留半个改过的层。
    let stored = scene
        .borrow()
        .anim_layer("hero/walk", "upper")
        .cloned()
        .expect("层应存在");
    assert!((stored.weight - 0.8).abs() < 1e-6, "权重不该被改");
    assert_eq!(stored.mode, LayerMode::Override, "模式不该被改");
}

#[test]
fn anim_layer_set_rejects_missing_instance_or_layer() {
    let (scene, physics, mut scripts) = anim_fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let no_instance = api.scene_anim_layer_set(AnimLayerPatch {
        instance: "nobody".to_string(),
        weight: Some(0.5),
        ..Default::default()
    });
    assert!(no_instance.is_err(), "实例不存在必须报错");

    // 实例在、层不在：同样报错，不凭空建层。
    let no_layer = api.scene_anim_layer_set(AnimLayerPatch {
        instance: "hero/walk".to_string(),
        layer: "legs".to_string(),
        weight: Some(0.5),
        ..Default::default()
    });
    assert!(no_layer.is_err(), "层不存在必须报错");
    assert!(
        scene.borrow().anim_layer_names("hero/walk").is_empty(),
        "失败调用不该建出任何层"
    );

    // 片段名解析不到也报错。
    let no_clip = api.scene_anim_layer_set(AnimLayerPatch {
        instance: "hero/walk".to_string(),
        clip: Some("nosuchclip".to_string()),
        ..Default::default()
    });
    assert!(no_clip.is_err(), "片段解析不到必须报错");
}

#[test]
fn anim_layers_limit_truncates_but_count_is_total() {
    let (scene, physics, mut scripts) = anim_fixture();
    let _ = scripts
        .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0); add_anim_layer("hero/walk", "lower", "atk", 0.5)"#)
        .expect("建层应成功");
    let mut api = api(&scene, &physics, &mut scripts);

    let list = api.scene_anim_layers(None, Some(1)).expect("列层应成功");
    assert_eq!(list["count"], json!(2), "count 报总数");
    assert_eq!(
        list["layers"].as_array().map(|v| v.len()),
        Some(1),
        "limit 只截断明细"
    );
}

#[test]
fn bone_mask_spec_is_normalized() {
    // 空 / `*` / 只有分隔符 → 整副骨架（空遮罩会让整层都不出效果）。
    for spec in ["", "  ", "*", " , ", ",,"] {
        let mask = bone_mask_of(spec);
        assert!(mask.is_pass_all(), "`{spec}` 应归一成整副骨架");
        assert_eq!(mask.rule_count(), 0);
    }
    let include = bone_mask_of("cube, sphere");
    assert_eq!(
        include.include,
        vec!["cube".to_string(), "sphere".to_string()]
    );
    assert!(include.exclude.is_empty());

    let exclude = bone_mask_of("!root");
    assert!(exclude.include.is_empty());
    assert_eq!(exclude.exclude, vec!["root".to_string()]);

    let mixed = bone_mask_of("cube, !root");
    assert_eq!(mixed.include, vec!["cube".to_string()]);
    assert_eq!(mixed.exclude, vec!["root".to_string()]);
    assert!(!mixed.is_pass_all());

    // 只有 `!` 的规则没有名字，忽略而不是造一条空规则。
    assert!(bone_mask_of("!").is_pass_all());
}

#[test]
fn audio_spatial_set_upserts_and_lists() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let created = api
        .audio_spatial_set(AudioSpatialPatch {
            position: Some([1.0, 0.0, 0.0]),
            min_distance: Some(2.0),
            max_distance: Some(20.0),
            model: Some("inverse".to_string()),
            doppler: Some(0.5),
            ..spatial_patch("bell")
        })
        .expect("注册应成功");
    assert_eq!(created["inserted"], json!(true));
    assert_eq!(created["count"], json!(1));
    assert_eq!(created["source"]["model"], json!("inverse"));
    assert_eq!(created["source"]["min_distance"], json!(2.0));

    // 同 key 再打一次是覆盖，不是新增
    let updated = api
        .audio_spatial_set(AudioSpatialPatch {
            max_distance: Some(35.0),
            ..spatial_patch("bell")
        })
        .expect("覆盖应成功");
    assert_eq!(updated["inserted"], json!(false));
    assert_eq!(updated["count"], json!(1));
    assert_eq!(updated["source"]["max_distance"], json!(35.0));
    // 没给的字段保持原值，不被默认值冲掉
    assert_eq!(updated["source"]["min_distance"], json!(2.0));
    assert_eq!(updated["source"]["position"], json!([1.0, 0.0, 0.0]));

    let list = api.audio_spatial_list(None).expect("列表应成功");
    assert_eq!(list["count"], json!(1));
    assert_eq!(list["enabled"], json!(true));
    assert_eq!(list["sources"].as_array().map(|v| v.len()), Some(1));
    // 没注册的 key 不算声源
    assert_eq!(api.audio_spatial_list(Some(0)).unwrap()["count"], json!(1));
    assert_eq!(
        api.audio_spatial_list(Some(0)).unwrap()["sources"]
            .as_array()
            .map(|v| v.len()),
        Some(0),
        "limit 只截断明细"
    );
}

#[test]
fn audio_spatial_set_rejects_bad_input_without_side_effect() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    assert!(
        api.audio_spatial_set(spatial_patch("   ")).is_err(),
        "空 key 必须报错"
    );
    let bad_model = api.audio_spatial_set(AudioSpatialPatch {
        model: Some("quadratic".to_string()),
        ..spatial_patch("bell")
    });
    assert!(bad_model.is_err(), "未知距离模型必须报错");
    // 失败不留半改：模型名解析在校验前完成，声源表应当还是空的
    assert_eq!(
        api.audio_spatial_list(None).unwrap()["count"],
        json!(0),
        "报错的调用不该注册出任何声源"
    );
}

#[test]
fn audio_spatial_listener_drives_distance_and_gain() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.audio_spatial_set(AudioSpatialPatch {
        position: Some([0.0, 0.0, 0.0]),
        min_distance: Some(1.0),
        max_distance: Some(20.0),
        ..spatial_patch("bell")
    })
    .expect("注册应成功");

    let near = api
        .audio_spatial_listener(Some(r3e_editor::ListenerPatch {
            position: Some([0.0, 0.0, 0.0]),
            forward: Some([0.0, 0.0, -1.0]),
        }))
        .expect("写听者应成功");
    assert_eq!(near["position"], json!([0.0, 0.0, 0.0]));

    let far = api
        .audio_spatial_listener(Some(r3e_editor::ListenerPatch {
            position: Some([15.0, 0.0, 0.0]),
            ..Default::default()
        }))
        .expect("写听者应成功");
    assert_eq!(far["position"], json!([15.0, 0.0, 0.0]));

    // 写完立刻结算：不用等下一帧就能在面板上看到距离与增益变化
    let list = api.audio_spatial_list(None).expect("列表应成功");
    let src = &list["sources"][0];
    let distance = src["distance"].as_f64().expect("应有距离");
    assert!(
        (distance - 15.0).abs() < 1e-3,
        "距离应约为 15，实际 {distance}"
    );
    let gain = src["gain"].as_f64().expect("应有增益");
    assert!(gain < 1.0 && gain > 0.0, "远处应衰减但仍可闻，实际 {gain}");
    assert_eq!(list["listener"]["position"], json!([15.0, 0.0, 0.0]));

    // 只读模式（省略参数）不该改掉刚写的听者
    let read = api.audio_spatial_listener(None).expect("只读应成功");
    assert_eq!(read["position"], json!([15.0, 0.0, 0.0]));

    // 零向量朝向被忽略，不能把朝向归一成 NaN
    api.audio_spatial_listener(Some(r3e_editor::ListenerPatch {
        forward: Some([0.0, 0.0, 0.0]),
        ..Default::default()
    }))
    .expect("零朝向应被静默忽略");
    let list = api.audio_spatial_list(None).expect("列表应成功");
    assert_eq!(list["listener"]["forward"], json!([0.0, 0.0, -1.0]));
}

#[test]
fn audio_spatial_node_binding_and_detach() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.audio_spatial_set(AudioSpatialPatch {
        node: Some("cube".to_string()),
        ..spatial_patch("bell")
    })
    .expect("带节点注册应成功");
    assert_eq!(
        api.audio_spatial_list(None).unwrap()["sources"][0]["node"],
        json!("cube")
    );

    // 空串 = 解绑（回到只按手动 position 算）
    api.audio_spatial_set(AudioSpatialPatch {
        node: Some("  ".to_string()),
        ..spatial_patch("bell")
    })
    .expect("解绑应成功");
    assert_eq!(
        api.audio_spatial_list(None).unwrap()["sources"][0]["node"],
        Value::Null,
        "解绑后不该残留节点名"
    );

    let removed = api.audio_spatial_remove("bell").expect("移除应成功");
    assert_eq!(removed["removed"], json!(true));
    assert_eq!(removed["count"], json!(0));
    // 重复移除不报错，只是 false
    assert_eq!(
        api.audio_spatial_remove("bell").unwrap()["removed"],
        json!(false)
    );
}

#[test]
fn audio_spatial_capacity_and_clear() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    for i in 0..MAX_SPATIAL_SOURCES {
        api.audio_spatial_set(spatial_patch(&format!("s{i}")))
            .unwrap_or_else(|e| panic!("第 {i} 个声源应注册成功: {e:?}"));
    }
    let over = api.audio_spatial_set(spatial_patch("overflow"));
    assert!(over.is_err(), "满载后新增必须报错而不是挤掉别人");
    assert_eq!(
        api.audio_spatial_list(None).unwrap()["count"],
        json!(MAX_SPATIAL_SOURCES)
    );

    let cleared = api.audio_spatial_clear().expect("清空应成功");
    assert_eq!(cleared["removed"], json!(MAX_SPATIAL_SOURCES));
    assert_eq!(cleared["count"], json!(0));
    // 清空后能重新注册（不会因为计数残留而卡死）
    assert!(api.audio_spatial_set(spatial_patch("fresh")).is_ok());
}
