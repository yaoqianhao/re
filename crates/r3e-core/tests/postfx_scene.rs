//! 后处理链在场景层的链路：文件 → 场景 → JSON 保存 → 重新加载。
//!
//! 与 `environment_scene.rs` 同构，关心三件事：
//! 1. 不写 `postfx` 段时场景保持 `None`——老场景画面不能被后处理悄悄改掉；
//! 2. 场景文件里的非法数值在载入那一刻就被 `sanitized()` 收敛，不残留 NaN；
//! 3. 写进去的后处理参数能原样保存回 JSON，再载入逐字段一致（编辑器改一处不该丢其余字段）。

use r3e_core::postfx::{BloomSettings, PostFxSettings, TonemapMode};
use r3e_core::{ObjectDesc, Scene};

fn desc(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        mesh: Some("cube".to_string()),
        ..Default::default()
    }
}

fn scene_json_with_postfx(postfx: &str) -> String {
    format!(
        r#"{{
  "camera": {{ "position": [0.0, 1.0, 5.0], "target": [0.0, 0.0, 0.0] }},
  "objects": [{{ "name": "ball", "mesh": "cube" }}],
  "postfx": {postfx}
}}"#
    )
}

/// 没写 `postfx` 段时场景保持 `None`，渲染层应走直通 composite。
#[test]
fn missing_postfx_keeps_none() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    assert!(scene.postfx.is_none());
    assert!(
        scene.to_scene_file().postfx.is_none(),
        "不该凭空写出 postfx 段"
    );
}

/// 只写一个字段时，其余字段必须回落到缺省值（serde default 生效），
/// 而不是被整段归零——否则编辑器只改曝光会把泛光参数一起抹掉。
#[test]
fn partial_postfx_uses_field_defaults() {
    let scene =
        Scene::from_json_str(&scene_json_with_postfx(r#"{ "exposure": 1.5 }"#)).expect("载入场景");
    let fx = scene.postfx.expect("写了 postfx 段就该是 Some");
    let d = PostFxSettings::default();
    assert_eq!(fx.exposure, 1.5);
    assert_eq!(fx.tonemap, TonemapMode::None);
    assert_eq!(fx.bloom, d.bloom);
    assert!(!fx.fxaa);
}

/// 非法值在载入时被收敛：NaN 回缺省、越界夹到上下限、负的泛光强度归零。
#[test]
fn scene_load_sanitizes_postfx() {
    let scene = Scene::from_json_str(&scene_json_with_postfx(
        r#"{ "exposure": 1e9,
             "tonemap": "aces",
             "bloom": { "enabled": true, "threshold": -5.0, "knee": 100.0, "intensity": -2.0 },
             "fxaa": true }"#,
    ))
    .expect("载入场景");
    let fx = scene.postfx.expect("Some");
    assert_eq!(
        fx.exposure,
        r3e_core::postfx::MAX_EXPOSURE_EV,
        "曝光夹到上界"
    );
    assert_eq!(fx.tonemap, TonemapMode::Aces);
    assert_eq!(
        fx.bloom.knee,
        r3e_core::postfx::MAX_BLOOM_KNEE,
        "软膝夹到上界"
    );
    assert_eq!(fx.bloom.intensity, 0.0, "负强度归零");
    assert_eq!(fx.bloom.threshold, 0.0, "负阈值归零");
    assert!(fx.fxaa);
}

/// 未知的 tonemap 名字必须直接报错，而不是静默降级——
/// 否则拼错的 "acees" 会变成直通画面，排查时毫无线索。
#[test]
fn unknown_tonemap_name_is_rejected() {
    let err = Scene::from_json_str(&scene_json_with_postfx(r#"{ "tonemap": "acees" }"#))
        .expect_err("未知 tonemap 名应当报错");
    assert!(
        err.contains("tonemap") || err.contains("unknown variant"),
        "错误信息应指出出错字段，实际：{err}"
    );
}

/// 保存 → 重新载入逐字段一致，且 JSON 里出现 postfx 段。
#[test]
fn postfx_survives_json_roundtrip() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    scene.postfx = Some(
        PostFxSettings {
            exposure: 0.75,
            tonemap: TonemapMode::Aces,
            bloom: BloomSettings {
                enabled: true,
                threshold: 1.25,
                knee: 0.35,
                intensity: 0.22,
            },
            fxaa: true,
            ssao: Default::default(),
        }
        .sanitized(),
    );

    let text = scene.to_json_string().expect("导出场景 JSON");
    assert!(text.contains("\"postfx\""), "应写出 postfx 段：{text}");
    assert!(
        text.contains("\"aces\""),
        "音调映射名应写成规范小写：{text}"
    );

    let reloaded = Scene::from_json_str(&text).expect("重新载入");
    assert_eq!(
        scene.postfx.expect("Some"),
        reloaded.postfx.expect("Some"),
        "后处理参数往返应逐字段一致"
    );
}

/// `None` 与 `Some(default())` 在渲染层等价：两者的 CPU 参考结果必须完全相同，
/// 这就是「老场景画面不变」的数值保证。
#[test]
fn absent_postfx_equals_default_settings() {
    let neutral = PostFxSettings::default();
    let hdr = r3e_core::Vec3::new(2.5, 0.4, 0.1);
    let a = r3e_core::postfx::resolve(hdr, r3e_core::Vec3::ZERO, &neutral);
    let expected = hdr.min(r3e_core::Vec3::ONE);
    assert_eq!(a, expected, "直通 composite 应等价于 clamp");
}
