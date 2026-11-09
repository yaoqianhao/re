//! 级联阴影（扩展十六）在场景层的链路：文件 → 场景 → JSON 保存 → 重新加载。
//!
//! 关心四件事：
//! 1. 不写级联字段时保持 `count = 1`——老场景的画面与统计口径不能被 CSM 悄悄改掉；
//! 2. 场景文件里的越界值在载入那一刻就被收敛到 1..=MAX_CASCADES、0..=0.5；
//! 3. 显式开了多级就必须写回 JSON，否则重开场景会静默退回单级；
//! 4. `1 级 + 缺省混合比` 属于等价形态，不写成噪声字段。

use r3e_core::light::{CascadeSettings, MAX_CASCADES};
use r3e_core::Scene;

fn scene_json_with_shadow(shadow: &str) -> String {
    format!(
        r#"{{
  "camera": {{ "position": [0.0, 1.0, 5.0], "target": [0.0, 0.0, 0.0] }},
  "objects": [{{ "name": "ball", "mesh": "cube" }}],
  "light": {{ "shadow": {shadow} }}
}}"#
    )
}

/// 老场景（shadow 段里没有级联字段）必须仍然是单级，行为与阶段 D 完全一致。
#[test]
fn missing_cascade_fields_keep_single_level() {
    let scene = Scene::from_json_str(&scene_json_with_shadow(
        r#"{ "enabled": true, "size": 2048 }"#,
    ))
    .expect("载入场景");
    let cascade = scene.light.shadow.cascade;
    assert_eq!(cascade.count, 1);
    assert_eq!(cascade, CascadeSettings::default());

    // 存回去时也不该凭空多出 cascades / blend 字段
    let file = scene.to_scene_file();
    let shadow = file
        .light
        .expect("应写出 light 段")
        .shadow
        .expect("应写出 shadow 段");
    assert_eq!(shadow.cascades, None, "单级是缺省，不该写进文件");
    assert_eq!(shadow.blend, None, "缺省混合比不该写进文件");
}

/// 写进去的级联配置逐字段生效，并能原样保存再载入。
#[test]
fn cascade_fields_round_trip() {
    let scene = Scene::from_json_str(&scene_json_with_shadow(
        r#"{ "enabled": true, "cascades": 3, "blend": 0.25, "cascadeDistance": 60.0 }"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.light.shadow.cascade.count, 3);
    assert_eq!(scene.light.shadow.cascade.blend, 0.25);
    assert_eq!(scene.light.shadow.cascade.max_distance, 60.0);

    let json = scene.to_json_pretty().expect("序列化");
    let again = Scene::from_json_str(&json).expect("重新载入");
    assert_eq!(again.light.shadow.cascade, scene.light.shadow.cascade);
    assert_eq!(again.light.shadow.size, scene.light.shadow.size);
}

/// 越界与 NaN 在载入时收敛：级数夹到 1..=4、混合比夹到 0..=0.5。
#[test]
fn scene_load_sanitizes_cascade() {
    let scene = Scene::from_json_str(&scene_json_with_shadow(
        r#"{ "cascades": 99, "blend": 8.0 }"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.light.shadow.cascade.count, MAX_CASCADES as u32);
    assert_eq!(scene.light.shadow.cascade.blend, 0.5);

    let scene =
        Scene::from_json_str(&scene_json_with_shadow(r#"{ "cascades": 0 }"#)).expect("载入场景");
    assert_eq!(
        scene.light.shadow.cascade.count, 1,
        "0 级抬到 1，绝不能真关掉阴影"
    );
}

/// `cascade` 单数别名也认（手写场景容易写成单数）。
#[test]
fn singular_alias_is_accepted() {
    let scene =
        Scene::from_json_str(&scene_json_with_shadow(r#"{ "cascade": 2 }"#)).expect("载入场景");
    assert_eq!(scene.light.shadow.cascade.count, 2);
}

// ---------------- 扩展二十四：遮挡体按级裁剪 ----------------

/// 老场景不写这两个字段时必须保持「不扩张 + 开启裁剪」，且存回文件时不产生噪声字段。
#[test]
fn missing_caster_fields_keep_defaults() {
    let scene =
        Scene::from_json_str(&scene_json_with_shadow(r#"{ "enabled": true }"#)).expect("载入场景");
    let cascade = scene.light.shadow.cascade;
    assert_eq!(
        cascade.caster_extend, 0.0,
        "缺省不扩张，画面与扩展二十三一致"
    );
    assert!(cascade.caster_cull, "按级裁剪是纯优化，缺省应开着");

    let file = scene.to_scene_file();
    let shadow = file
        .light
        .expect("应写出 light 段")
        .shadow
        .expect("应写出 shadow 段");
    assert_eq!(shadow.caster_extend, None, "0 是缺省，不该写进文件");
    assert_eq!(shadow.caster_cull, None, "开启是缺省，不该写进文件");
}

/// 外扩距离与裁剪开关逐字段生效，并能原样往返。
#[test]
fn caster_fields_round_trip() {
    let scene = Scene::from_json_str(&scene_json_with_shadow(
        r#"{ "cascades": 4, "casterExtend": 12.5, "casterCull": false }"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.light.shadow.cascade.caster_extend, 12.5);
    assert!(!scene.light.shadow.cascade.caster_cull);

    let json = scene.to_json_pretty().expect("序列化");
    let again = Scene::from_json_str(&json).expect("重新载入");
    assert_eq!(again.light.shadow.cascade, scene.light.shadow.cascade);
    // 写回文件时：非零外扩与「显式关掉裁剪」都必须保留，否则重开场景会静默恢复
    let shadow = again
        .to_scene_file()
        .light
        .expect("应写出 light 段")
        .shadow
        .expect("应写出 shadow 段");
    assert_eq!(shadow.caster_extend, Some(12.5));
    assert_eq!(shadow.caster_cull, Some(false));
}

/// 蛇形别名也认；越界 / 负数 / 超大外扩在载入那一刻收敛到 0..=MAX_CASTER_EXTEND。
#[test]
fn caster_fields_accept_alias_and_sanitize() {
    use r3e_core::light::MAX_CASTER_EXTEND;
    let scene = Scene::from_json_str(&scene_json_with_shadow(
        r#"{ "caster_extend": 3.0, "caster_cull": true }"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.light.shadow.cascade.caster_extend, 3.0);

    for (raw, want) in [("-9.0", 0.0), ("0", 0.0), ("1e9", MAX_CASTER_EXTEND)] {
        let scene = Scene::from_json_str(&scene_json_with_shadow(&format!(
            r#"{{ "casterExtend": {raw} }}"#
        )))
        .expect("载入场景");
        assert_eq!(
            scene.light.shadow.cascade.caster_extend, want,
            "casterExtend = {raw} 应收敛到 {want}"
        );
    }
}
