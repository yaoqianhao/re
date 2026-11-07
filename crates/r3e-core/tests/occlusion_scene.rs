//! 遮挡剔除（扩展二十二）在场景层的链路：文件 → 场景 → JSON 保存 → 重新加载。
//!
//! 与 `ssao_scene.rs` 同构，关心四件事：
//! 1. 不写 `occlusion` 段时场景是缺省值（关闭）——老场景的 GPU 提交量不能被悄悄改掉；
//! 2. 只给部分字段时其余回落到缺省（serde default），改偏置不该顺手把 tile 抹成 0；
//! 3. 非法数值在载入那一刻就被 `sanitized()` 收敛，不残留 NaN / 越界 tile；
//! 4. 缺省值不写回 JSON（老场景 roundtrip 不该凭空长出 `occlusion` 字段），
//!    非缺省值则原样保存，再载入逐字段一致。

use r3e_core::occlusion::{
    OcclusionSettings, DEFAULT_DEPTH_BIAS, DEFAULT_MARGIN_PX, DEFAULT_MAX_TILES, DEFAULT_TILE_SIZE,
    MAX_DEPTH_BIAS, MAX_MARGIN_PX, MAX_TILE_SIZE, MIN_TILE_SIZE,
};
use r3e_core::{ObjectDesc, Scene};

fn desc(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        mesh: Some("cube".to_string()),
        ..Default::default()
    }
}

fn scene_json_with_occlusion(occlusion: &str) -> String {
    format!(
        r#"{{
  "camera": {{ "position": [0.0, 1.0, 5.0], "target": [0.0, 0.0, 0.0] }},
  "objects": [{{ "name": "ball", "mesh": "cube" }}],
  "occlusion": {occlusion}
}}"#
    )
}

/// 完全不写 `occlusion`：场景保持缺省（关闭），渲染层一颗都不剔。
#[test]
fn missing_occlusion_section_stays_off() {
    let scene = Scene::from_json_str(r#"{ "objects": [ { "name": "ball", "mesh": "cube" } ] }"#)
        .expect("载入场景");
    assert_eq!(scene.occlusion, OcclusionSettings::default());
    assert!(!scene.occlusion_enabled());
    assert!(scene.occlusion.is_neutral());
}

/// 写了段但不写 `enabled`：仍然是关闭——开不开只能由显式的 true 决定。
#[test]
fn occlusion_section_without_enabled_stays_off() {
    let scene = Scene::from_json_str(&scene_json_with_occlusion(r#"{ "tile_size": 4 }"#))
        .expect("载入场景");
    assert!(!scene.occlusion.enabled);
    assert_eq!(scene.occlusion.tile_size, 4, "写了的值要生效");
    assert!(scene.occlusion.is_neutral());
}

/// 只给一个字段：其余回落到缺省，不是整段归零。
#[test]
fn partial_occlusion_keeps_other_fields() {
    let scene = Scene::from_json_str(&scene_json_with_occlusion(
        r#"{ "enabled": true, "depth_bias": 0.01 }"#,
    ))
    .expect("载入场景");
    let o = scene.occlusion;
    assert!(o.enabled);
    assert!((o.depth_bias - 0.01).abs() < 1e-6);
    assert_eq!(o.tile_size, DEFAULT_TILE_SIZE);
    assert_eq!(o.margin_px, DEFAULT_MARGIN_PX);
    assert_eq!(o.max_tiles, DEFAULT_MAX_TILES);
}

/// 越界 / NaN 在载入那一刻收敛，不会带进渲染层。
#[test]
fn illegal_occlusion_values_are_sanitized_on_load() {
    let scene = Scene::from_json_str(&scene_json_with_occlusion(
        r#"{ "enabled": true, "depth_bias": 99.0, "tile_size": 0, "margin_px": 4096, "max_tiles": 1 }"#,
    ))
    .expect("载入场景");
    let o = scene.occlusion;
    assert_eq!(o.depth_bias, MAX_DEPTH_BIAS);
    assert_eq!(o.tile_size, MIN_TILE_SIZE);
    assert_eq!(o.margin_px, MAX_MARGIN_PX);
    assert_eq!(o.max_tiles, r3e_core::occlusion::MIN_MAX_TILES);

    let scene = Scene::from_json_str(&scene_json_with_occlusion(
        r#"{ "enabled": true, "tile_size": 9999 }"#,
    ))
    .expect("载入场景");
    assert_eq!(scene.occlusion.tile_size, MAX_TILE_SIZE);
}

/// NaN 偏置不能污染场景（JSON 里 NaN 用字符串形式进不来，构造体走 `set_occlusion`）。
#[test]
fn set_occlusion_sanitizes_nan_bias() {
    let mut scene = Scene::new();
    let applied = scene.set_occlusion(OcclusionSettings {
        enabled: true,
        depth_bias: f32::NAN,
        ..OcclusionSettings::default()
    });
    assert_eq!(applied.depth_bias, DEFAULT_DEPTH_BIAS);
    assert_eq!(scene.occlusion.depth_bias, DEFAULT_DEPTH_BIAS);
}

/// 缺省值不写回 JSON：老场景保存后不会凭空多出 `occlusion` 段。
#[test]
fn default_occlusion_is_not_serialized() {
    let scene = Scene::from_json_str(r#"{ "objects": [ { "name": "ball", "mesh": "cube" } ] }"#)
        .expect("载入场景");
    let text = scene.to_json_string().expect("序列化");
    assert!(
        !text.contains("occlusion"),
        "缺省值不该写进场景文件: {text}"
    );
}

/// 非缺省值原样保存，再载入逐字段一致（load → save → load 语义等价）。
#[test]
fn occlusion_roundtrips_through_json() {
    let original = OcclusionSettings {
        enabled: true,
        depth_bias: 0.0125,
        tile_size: 16,
        margin_px: 3,
        max_tiles: 128,
    };
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    scene.set_occlusion(original);

    let text = scene.to_json_string().expect("序列化");
    assert!(text.contains("\"occlusion\""), "非缺省值要写进场景文件");

    let reloaded = Scene::from_json_str(&text).expect("重新载入");
    assert_eq!(reloaded.occlusion, original);
    assert!(reloaded.occlusion_enabled());
}

/// 运行时开关：关掉只动开关位，参数与场景内容都不动；再打开即恢复。
#[test]
fn toggle_keeps_parameters_and_scene() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    scene.set_occlusion(OcclusionSettings {
        enabled: true,
        tile_size: 4,
        ..OcclusionSettings::default()
    });
    assert!(scene.occlusion_enabled());

    assert!(!scene.set_occlusion_enabled(false));
    assert!(!scene.occlusion_enabled());
    assert_eq!(scene.occlusion.tile_size, 4, "关掉不该清参数");
    assert_eq!(scene.len(), 1, "关掉不该动物件");

    assert!(scene.set_occlusion_enabled(true));
    assert!(scene.occlusion_enabled());
    assert_eq!(scene.occlusion.tile_size, 4);
}
