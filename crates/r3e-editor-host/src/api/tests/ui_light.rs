//! 屏幕 UI、局部光源与阴影 / SSAO 的 RPC 测试。
//!
//! 由 M0-S2 从 `api.rs` 内 `mod tests` 原样切出，断言未改动。

use super::*;

#[test]
fn ui_set_inserts_with_parsed_units() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let out = api
        .ui_set(UiPatch {
            key: "btn_start".to_string(),
            kind: Some("image".to_string()),
            anchor: Some("bottom_center".to_string()),
            x: Some("0".to_string()),
            y: Some("-48".to_string()),
            width: Some("50%".to_string()),
            height: Some("64".to_string()),
            layer: Some(7),
            ..Default::default()
        })
        .expect("应成功");

    assert_eq!(out["inserted"], true);
    assert_eq!(out["count"], 1);
    assert_eq!(out["element"]["kind"], "image");
    assert_eq!(out["element"]["anchor"], "bottom_center");
    // 百分比与像素各回读成自己的单位串。
    assert_eq!(out["element"]["width"], "50%");
    assert_eq!(out["element"]["height"], "64");

    let scene = scene.borrow();
    assert_eq!(scene.ui.len(), 1);
    assert_eq!(scene.ui[0].size[0], UiValue::pct(50.0));
    assert_eq!(scene.ui[0].size[1], UiValue::px(64.0));
    assert_eq!(scene.ui[0].offset[1], UiValue::px(-48.0));
    assert_eq!(scene.ui[0].layer, 7);
}

#[test]
fn ui_set_patches_existing_element_in_place() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.ui_set(UiPatch {
        key: "btn".to_string(),
        kind: Some("image".to_string()),
        image: Some("ui/btn.png".to_string()),
        width: Some("120".to_string()),
        layer: Some(3),
        ..Default::default()
    })
    .expect("首次写入应成功");

    // 只改 layer：其余字段必须保持原值（局部更新而不是整体覆盖）。
    let out = api
        .ui_set(UiPatch {
            key: "btn".to_string(),
            layer: Some(9),
            ..Default::default()
        })
        .expect("局部更新应成功");
    assert_eq!(out["inserted"], false);
    assert_eq!(out["replaced"], true);
    assert_eq!(out["count"], 1, "同 key 不应累积出新元素");

    let scene = scene.borrow();
    assert_eq!(scene.ui.len(), 1);
    assert_eq!(scene.ui[0].layer, 9);
    assert_eq!(scene.ui[0].kind, UiKind::Image);
    assert_eq!(scene.ui[0].image, "ui/btn.png");
    assert_eq!(scene.ui[0].size[0], UiValue::px(120.0));
}

#[test]
fn ui_set_rejects_unknown_names_without_side_effect() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let bad_anchor = api
        .ui_set(UiPatch {
            key: "panel".to_string(),
            anchor: Some("斜着".to_string()),
            ..Default::default()
        })
        .expect_err("未知锚点应报错");
    assert!(format!("{bad_anchor:?}").contains("斜着"));

    let bad_kind = api
        .ui_set(UiPatch {
            key: "panel".to_string(),
            kind: Some("圆角矩形".to_string()),
            ..Default::default()
        })
        .expect_err("未知类型应报错");
    assert!(format!("{bad_kind:?}").contains("圆角矩形"));

    assert!(scene.borrow().ui.is_empty(), "报错时不应留下半条元素");
}

#[test]
fn ui_list_and_remove_round_trip() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    for key in ["a", "b"] {
        api.ui_set(UiPatch {
            key: key.to_string(),
            ..Default::default()
        })
        .expect("应成功");
    }
    let listed = api.ui_list(None).expect("应成功");
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["elements"].as_array().expect("数组").len(), 2);

    let removed = api.ui_remove("a").expect("应成功");
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["count"], 1);

    let again = api.ui_remove("a").expect("重复删除应成功");
    assert_eq!(again["removed"], false, "已不存在时如实回报 false");
}

#[test]
fn ui_state_reports_pointer_interaction() {
    let (scene, physics, mut scripts) = fixture();
    scene.borrow_mut().write(|s| {
        s.ui.push(UiElement {
            key: "btn".to_string(),
            ..Default::default()
        })
    });
    // 直接把光标按在元素上（默认 100x100 从左上角起算），模拟一帧按下。
    {
        let interaction = scripts.ui_interaction();
        let mut state = interaction.borrow_mut();
        state.reset();
        state.update(
            &scene.borrow().ui,
            (800.0, 600.0),
            Some((10.0, 10.0)),
            true,
            true,
            false,
        );
    }
    let mut api = api(&scene, &physics, &mut scripts);
    let state = api.ui_state().expect("应成功");
    assert_eq!(state["hover"], "btn");
    assert_eq!(state["active"], "btn");
    assert_eq!(state["pointer_over_ui"], true);
    assert_eq!(state["events"].as_array().expect("数组").len(), 2);
}

// ---------- 扩展十五：局部光源 RPC ----------

/// 新建点光：缺省字段取 `LocalLight::default`，给了的字段逐个落地。
#[test]
fn light_set_inserts_point_light() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let out = api
        .light_set(LightPatch {
            key: "lamp".to_string(),
            pos: Some([1.0, 2.0, 3.0]),
            intensity: Some(8.0),
            range: Some(12.0),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(out["inserted"], true);
    assert_eq!(out["count"], 1);
    assert_eq!(out["light"]["kind"], "point");
    assert_eq!(out["light"]["pos"], json!([1.0, 2.0, 3.0]));

    let scene = scene.borrow();
    let light = scene.local_light("lamp").expect("场景里应有这盏灯");
    assert_eq!(light.intensity, 8.0);
    assert_eq!(light.range, 12.0);
    // 没给的字段保持默认，而不是被清零
    assert_eq!(light.color, LocalLight::default().color);
}

/// 同 key 再写一次是叠加而非替换：没给的字段保留上一次的值。
#[test]
fn light_set_merges_into_existing() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.light_set(LightPatch {
        key: "lamp".to_string(),
        kind: Some("spot".to_string()),
        pos: Some([0.0, 5.0, 0.0]),
        dir: Some([0.0, -1.0, 0.0]),
        inner_deg: Some(15.0),
        outer_deg: Some(25.0),
        ..Default::default()
    })
    .expect("应成功");
    let out = api
        .light_set(LightPatch {
            key: "lamp".to_string(),
            intensity: Some(20.0),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(out["inserted"], false, "第二次应是更新");
    assert_eq!(out["count"], 1, "不应重复新增");
    assert_eq!(out["light"]["kind"], "spot", "未给的 kind 保留");
    assert_eq!(out["light"]["intensity"], 20.0);
    assert_eq!(out["light"]["inner_deg"], 15.0, "未给的锥角保留");
}

/// 未知 kind 必须报参数错误，且不能留下半盏灯。
#[test]
fn light_set_rejects_unknown_kind_without_side_effect() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let err = api
        .light_set(LightPatch {
            key: "bad".to_string(),
            kind: Some("laser".to_string()),
            ..Default::default()
        })
        .expect_err("应报参数错误");
    assert!(err.message.contains("未知光源类型"), "{}", err.message);
    assert_eq!(scene.borrow().lights.len(), 0, "坏 kind 不应落地");
}

/// 坏值收敛：NaN / 负强度 / 零半径 / 内外角倒挂都必须在进场景前被夹住。
#[test]
fn light_set_sanitizes_bad_values() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.light_set(LightPatch {
        key: "bad".to_string(),
        pos: Some([f32::NAN, 0.0, 0.0]),
        intensity: Some(-3.0),
        range: Some(0.0),
        inner_deg: Some(40.0),
        outer_deg: Some(10.0),
        ..Default::default()
    })
    .expect("应成功（收敛后）");

    let scene = scene.borrow();
    let light = scene.local_light("bad").expect("应已落地");
    assert!(light.pos[0].is_finite(), "NaN 位置必须被抹平");
    assert_eq!(light.intensity, 0.0, "负强度夹到 0");
    assert_eq!(light.range, MIN_LIGHT_RANGE, "零半径抬到下限");
    assert!(light.outer_deg > light.inner_deg, "内外角必须修正");
}

/// 扩展十八 C：阴影两项走同一补丁通道，缺省不动原值，非法偏移被夹进合法区间。
#[test]
fn light_set_merges_shadow_fields() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.light_set(LightPatch {
        key: "lamp".to_string(),
        intensity: Some(2.0),
        ..Default::default()
    })
    .expect("新建应成功");
    assert!(
        !scene.borrow().local_light("lamp").unwrap().cast_shadow,
        "新灯默认不投影"
    );

    // 只给 cast_shadow：shadow_bias 必须沿用原值
    let before_bias = scene.borrow().local_light("lamp").unwrap().shadow_bias;
    let resp = api
        .light_set(LightPatch {
            key: "lamp".to_string(),
            cast_shadow: Some(true),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(resp["inserted"], json!(false), "同 key 是更新而非新增");
    assert_eq!(resp["light"]["cast_shadow"], json!(true), "回包带新值");
    assert_eq!(resp["light"]["shadow_bias"], json!(before_bias));
    assert!(scene.borrow().local_light("lamp").unwrap().cast_shadow);

    // 超上限的偏移被夹住，不会原样落进场景
    api.light_set(LightPatch {
        key: "lamp".to_string(),
        shadow_bias: Some(99.0),
        ..Default::default()
    })
    .expect("应成功（收敛后）");
    let bias = scene.borrow().local_light("lamp").unwrap().shadow_bias;
    assert!(bias <= MAX_LOCAL_SHADOW_BIAS, "偏移必须被夹到上限内");

    // 关掉投影同样走这条通道
    api.light_set(LightPatch {
        key: "lamp".to_string(),
        cast_shadow: Some(false),
        ..Default::default()
    })
    .expect("应成功");
    assert!(!scene.borrow().local_light("lamp").unwrap().cast_shadow);
}

/// 满载：新增被拒并报上限，同 key 覆盖仍生效。
#[test]
fn light_set_capacity_limit() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    for i in 0..MAX_LOCAL_LIGHTS {
        api.light_set(LightPatch {
            key: format!("l{i}"),
            intensity: Some(1.0),
            ..Default::default()
        })
        .expect("前 MAX_LOCAL_LIGHTS 盏应成功");
    }
    let err = api
        .light_set(LightPatch {
            key: "extra".to_string(),
            intensity: Some(1.0),
            ..Default::default()
        })
        .expect_err("满载后新增应报错");
    assert!(
        err.message.contains(&MAX_LOCAL_LIGHTS.to_string()),
        "{}",
        err.message
    );

    api.light_set(LightPatch {
        key: "l0".to_string(),
        intensity: Some(9.0),
        ..Default::default()
    })
    .expect("满载时同 key 覆盖必须仍生效");
    assert_eq!(scene.borrow().local_light("l0").unwrap().intensity, 9.0);
}

/// list / remove / clear / enabled 四件套。
#[test]
fn light_list_remove_clear_and_switch() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    for key in ["a", "b"] {
        api.light_set(LightPatch {
            key: key.to_string(),
            pos: Some([0.0, 1.0, 0.0]),
            intensity: Some(1.0),
            ..Default::default()
        })
        .expect("应成功");
    }
    let listed = api.light_list(None).expect("应成功");
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["enabled"], true);
    let arr = listed["lights"].as_array().expect("数组");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["key"], "a");
    assert_eq!(arr[0]["kind"], "point");
    // 九个字段都要在，编辑器靠这个面板直接渲染
    for field in [
        "key",
        "kind",
        "pos",
        "dir",
        "color",
        "intensity",
        "range",
        "inner_deg",
        "outer_deg",
    ] {
        assert!(arr[0].get(field).is_some(), "快照缺少字段 {field}");
    }

    let limited = api.light_list(Some(1)).expect("应成功");
    assert_eq!(limited["count"], 2, "count 是全量");
    assert_eq!(
        limited["lights"].as_array().unwrap().len(),
        1,
        "limit 只截结果"
    );

    let removed = api.light_remove("a").expect("应成功");
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["count"], 1);
    assert_eq!(
        api.light_remove("a").expect("应成功")["removed"],
        false,
        "重复删除如实回报"
    );

    let switched = api.light_set_enabled(Some(false)).expect("应成功");
    assert_eq!(switched["enabled"], false);
    assert_eq!(switched["count"], 1, "关灯不删数据");
    assert_eq!(
        api.light_set_enabled(None).expect("应成功")["enabled"],
        false,
        "不给参数是只读"
    );

    let cleared = api.light_clear().expect("应成功");
    assert_eq!(cleared["removed"], 1);
    assert_eq!(scene.borrow().lights.len(), 0);
}

/// 扩展十六：`scene.shadow` 的读 / 写 / 收敛三段语义。
#[test]
fn shadow_rpc_reads_writes_and_clamps() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    // 只读：不该改动任何值
    let before = scene.borrow().light.shadow;
    let read = api.scene_shadow(None).expect("应成功");
    assert_eq!(read["enabled"], before.enabled);
    assert_eq!(read["cascades"], before.cascade.count);
    assert_eq!(
        read["maxCascades"], MAX_CASCADES,
        "上限必须回给编辑器，面板才能画对滑块范围"
    );
    for field in [
        "enabled",
        "size",
        "strength",
        "orthoSize",
        "bias",
        "normalBias",
        "cascades",
        "maxCascades",
        "blend",
        "distance",
        "lambda",
    ] {
        assert!(read.get(field).is_some(), "回包缺少字段 {field}");
    }

    // 写入：补丁合并后落地
    let written = api
        .scene_shadow(Some(ShadowPatch {
            cascades: Some(4),
            blend: Some(0.2),
            distance: Some(80.0),
            ..Default::default()
        }))
        .expect("应成功");
    assert_eq!(written["cascades"], 4);
    assert_eq!(written["blend"], 0.2_f64 as f32);
    assert_eq!(written["distance"], 80.0);
    let shadow = scene.borrow().light.shadow;
    assert_eq!(shadow.cascade.count, 4);
    assert_eq!(shadow.cascade.blend, 0.2);

    // 越界一次性收敛，且回包给的是收敛后的值（不能「回包好看、实际另一套」）
    let clamped = api
        .scene_shadow(Some(ShadowPatch {
            cascades: Some(99),
            blend: Some(4.0),
            size: Some(1000),
            ..Default::default()
        }))
        .expect("应成功");
    assert_eq!(clamped["cascades"], MAX_CASCADES);
    assert_eq!(
        clamped["blend"], 0.5,
        "混合比上限是 0.5（再大就跨级采到不存在的层）"
    );
    assert_eq!(
        clamped["size"], 1000,
        "只要在 64..8192 内就原样保留（不强行取整）"
    );
    let shadow = scene.borrow().light.shadow;
    assert_eq!(shadow.cascade.count, MAX_CASCADES as u32);
    assert_eq!(shadow.size, 1000);

    // 关掉阴影只关开关，级数等配置保留（编辑器取消勾选不该丢配置）
    let off = api
        .scene_shadow(Some(ShadowPatch {
            enabled: Some(false),
            ..Default::default()
        }))
        .expect("应成功");
    assert_eq!(off["enabled"], false);
    assert_eq!(off["cascades"], MAX_CASCADES, "关阴影不重置级联配置");
}

/// 空 key 一律拒绝（传输层已拦一次，宿主侧再兜一次，防止别的调用方绕过）。
#[test]
fn light_set_rejects_blank_key() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    assert!(api
        .light_set(LightPatch {
            key: "   ".to_string(),
            ..Default::default()
        })
        .is_err());
    assert_eq!(scene.borrow().lights.len(), 0);
}

/// 遮蔽补丁落在后处理段的 `ssao` 子段里：既有设置（曝光）必须原样留着，
/// 越界样本数在宿主侧夹进 [4, 16] 而不是报参数错误。
#[test]
fn set_ssao_merges_and_clamps() {
    let (scene, physics, mut scripts) = fixture();
    {
        let mut api = api(&scene, &physics, &mut scripts);
        api.scene_set_postfx(PostFxPatch {
            exposure: Some(1.5),
            ..Default::default()
        })
        .expect("postfx 补丁应成功");
        let v = api
            .scene_set_ssao(SsaoPatch {
                enabled: Some(true),
                radius: Some(0.9),
                samples: Some(99),
                ..Default::default()
            })
            .expect("ssao 补丁应成功");
        assert_eq!(v["ssao"]["samples"], json!(16), "样本数应夹到上限");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["neutral"], false);
    }
    let fx = scene.borrow().postfx.expect("后处理段应已实体化");
    assert!(fx.ssao.enabled);
    assert!((fx.ssao.radius - 0.9).abs() < 1e-6);
    assert!(
        (fx.exposure - 1.5).abs() < 1e-6,
        "遮蔽补丁不该动曝光: {fx:?}"
    );
}

/// `enabled = false` 只关遮蔽支线：曝光 / 泛光 / FXAA 保留，段本身不清空。
#[test]
fn disable_ssao_keeps_rest_of_postfx() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    api.scene_set_postfx(PostFxPatch {
        exposure: Some(-0.5),
        bloom_enabled: Some(true),
        fxaa: Some(true),
        ..Default::default()
    })
    .expect("postfx 补丁应成功");
    api.scene_set_ssao(SsaoPatch {
        enabled: Some(true),
        intensity: Some(2.0),
        ..Default::default()
    })
    .expect("ssao 补丁应成功");
    let off = api
        .scene_set_ssao(SsaoPatch {
            enabled: Some(false),
            ..Default::default()
        })
        .expect("应成功");
    assert_eq!(off["enabled"], false);
    assert_eq!(off["neutral"], true);
    let fx = scene.borrow().postfx.expect("段不该被清空");
    assert!(!fx.ssao.enabled);
    assert!((fx.ssao.intensity - 2.0).abs() < 1e-6, "强度应保留");
    assert!((fx.exposure + 0.5).abs() < 1e-6);
    assert!(fx.bloom.enabled);
    assert!(fx.fxaa);
}
