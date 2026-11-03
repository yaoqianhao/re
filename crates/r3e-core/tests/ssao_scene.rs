//! SSAO（扩展十七）在场景层的链路：文件 → 场景 → JSON 保存 → 重新加载。
//!
//! 与 `postfx_scene.rs` 同构，关心四件事：
//! 1. 不写 `postfx` / `postfx.ssao` 段时场景保持 `None` / 缺省——老场景画面不能被遮蔽悄悄改掉；
//! 2. 只给部分字段时其余回落到缺省（serde default），编辑器改半径不该抹掉样本数；
//! 3. 非法数值在载入那一刻就被 `sanitized()` 收敛，不残留 NaN / 越界样本数；
//! 4. 写进去的参数能原样保存回 JSON，再载入逐字段一致。

use r3e_core::postfx::PostFxSettings;
use r3e_core::ssao::{
    SsaoSettings, DEFAULT_SSAO_BIAS, DEFAULT_SSAO_INTENSITY, DEFAULT_SSAO_RADIUS,
    DEFAULT_SSAO_SAMPLES, MAX_SSAO_INTENSITY, MAX_SSAO_RADIUS, MAX_SSAO_SAMPLES, MIN_SSAO_BIAS,
};
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

/// 完全不写 postfx：场景保持 `None`，渲染层走「composite 直通 + 白色占位 AO」。
#[test]
fn missing_postfx_keeps_ssao_off() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    assert!(scene.postfx.is_none());
    let fx = scene.postfx.unwrap_or_default();
    assert!(fx.ssao.is_neutral(), "缺省不该打开遮蔽");
    assert!(fx.is_neutral());
}

/// 写了 `postfx` 但没写 `ssao`：整段回落到缺省值，而不是「开着但全零」。
#[test]
fn postfx_without_ssao_uses_defaults() {
    let scene =
        Scene::from_json_str(&scene_json_with_postfx(r#"{ "exposure": 0.5 }"#)).expect("载入场景");
    let fx = scene.postfx.expect("Some");
    let d = SsaoSettings::default();
    assert_eq!(fx.ssao, d);
    assert!(!fx.ssao.enabled);
    assert_eq!(fx.ssao.radius, DEFAULT_SSAO_RADIUS);
    assert_eq!(fx.ssao.bias, DEFAULT_SSAO_BIAS);
    assert_eq!(fx.ssao.intensity, DEFAULT_SSAO_INTENSITY);
    assert_eq!(fx.ssao.samples, DEFAULT_SSAO_SAMPLES);
}

/// 只给半径：其余字段仍是缺省值（serde 的逐字段 default，不是整段归零）。
#[test]
fn partial_ssao_keeps_other_fields() {
    let scene = Scene::from_json_str(&scene_json_with_postfx(
        r#"{ "ssao": { "enabled": true, "radius": 1.25 } }"#,
    ))
    .expect("载入场景");
    let s = scene.postfx.expect("Some").ssao;
    assert!(s.enabled);
    assert_eq!(s.radius, 1.25);
    assert_eq!(s.bias, DEFAULT_SSAO_BIAS);
    assert_eq!(s.intensity, DEFAULT_SSAO_INTENSITY);
    assert_eq!(s.samples, DEFAULT_SSAO_SAMPLES);
}

/// 载入时收敛：半径夹上界、偏置夹下界、强度夹上界、样本数夹到 16。
#[test]
fn scene_load_sanitizes_ssao() {
    let scene = Scene::from_json_str(&scene_json_with_postfx(
        r#"{ "ssao": { "enabled": true, "radius": 1e9, "bias": -3.0,
                       "intensity": 99.0, "samples": 255 } }"#,
    ))
    .expect("载入场景");
    let s = scene.postfx.expect("Some").ssao;
    assert_eq!(s.radius, MAX_SSAO_RADIUS);
    assert_eq!(s.bias, MIN_SSAO_BIAS);
    assert_eq!(s.intensity, MAX_SSAO_INTENSITY);
    assert_eq!(s.samples, MAX_SSAO_SAMPLES);
    assert_eq!(s.effective_samples(), MAX_SSAO_SAMPLES as usize);
}

/// 保存 → 重新载入逐字段一致。
#[test]
fn ssao_survives_json_roundtrip() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    scene.postfx = Some(PostFxSettings {
        exposure: 0.25,
        ssao: SsaoSettings {
            enabled: true,
            radius: 1.5,
            bias: 0.05,
            intensity: 2.0,
            samples: 12,
        },
        ..Default::default()
    });
    let file = scene.to_scene_file();
    let text = serde_json::to_string(&file).expect("序列化");
    assert!(text.contains("\"ssao\""), "JSON 里应出现 ssao 段: {text}");
    let back = Scene::from_json_str(&text).expect("重新载入");
    let s = back.postfx.expect("Some").ssao;
    assert!(s.enabled);
    assert_eq!(s.radius, 1.5);
    assert_eq!(s.bias, 0.05);
    assert_eq!(s.intensity, 2.0);
    assert_eq!(s.samples, 12);
}

/// `is_neutral` 只在遮蔽关闭时才为真：开了遮蔽就不再是「直通画面」。
#[test]
fn enabled_ssao_breaks_neutrality() {
    let mut fx = PostFxSettings::default();
    assert!(fx.is_neutral());
    fx.ssao.enabled = true;
    assert!(!fx.is_neutral());
}

/// 随包发布的演示场景必须真的能载入，且遮蔽参数一个不差地落到场景上。
///
/// 这条是给「演示资产」兜底的：脚本能编译不代表场景合法，字段拼错会在运行时
/// 静默回落缺省（serde default），画面上看不出差别，只有这里会红。
#[test]
fn shipped_ssao_demo_scene_loads() {
    let text = include_str!("../../../assets/scene_ssao.json");
    let scene = Scene::from_json_str(text).expect("assets/scene_ssao.json 必须合法");
    assert_eq!(scene.name, "r3e-ssao-demo");
    let fx = scene.postfx.expect("演示场景必须带 postfx 段");
    let s = fx.ssao;
    assert!(s.enabled, "演示场景应默认开着遮蔽");
    assert_eq!(s.samples, MAX_SSAO_SAMPLES, "演示场景用满 16 个样本");
    assert_eq!(s, s.sanitized(), "写进去的值本身就该是收敛后的值");
    // 半径 / 强度落在「能看见接触阴影」的区间里：太小只剩缝隙，太大整面墙发灰。
    assert!(s.radius > 0.5 && s.radius < 1.2, "半径超出演示意图: {s:?}");
    assert!(s.intensity > 1.0, "强度太低看不见遮蔽: {s:?}");
    // 场景里得有足够多的夹角（方块贴墙、台阶、缝隙），否则遮蔽无从体现。
    assert!(scene.len() >= 10, "演示场景物件太少: {}", scene.len());
}
