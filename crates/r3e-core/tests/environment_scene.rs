//! 环境探针在场景层的链路：文件 → 场景 → 脚本级 API → JSON 保存 → 重新加载。
//!
//! 关心三件事：
//! 1. 缺省（不写 `environment` 段）必须是 `None`——老场景画面不能被 IBL 悄悄改掉；
//! 2. 场景文件里的非法数值在载入那一刻就被 `sanitized()` 收敛，不残留 NaN；
//! 3. 写进去的环境参数能原样保存回 JSON，再载入结果一致（编辑器改一处不该丢其余字段）。

use r3e_core::env::{EnvironmentSettings, Sh9, MAX_IBL_INTENSITY};
use r3e_core::{ObjectDesc, Scene};

fn desc(name: &str) -> ObjectDesc {
    ObjectDesc {
        name: name.to_string(),
        mesh: Some("cube".to_string()),
        ..Default::default()
    }
}

fn scene_json_with_env(env: &str) -> String {
    format!(
        r#"{{
  "camera": {{ "position": [0.0, 1.0, 5.0], "target": [0.0, 0.0, 0.0] }},
  "objects": [{{ "name": "ball", "mesh": "cube" }}],
  "environment": {env}
}}"#
    )
}

/// 没写 `environment` 段时场景保持 `None`，渲染走 `light.ambient` 常数项。
#[test]
fn missing_environment_keeps_none() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    assert!(scene.environment.is_none());

    // 保存时也不该凭空写出 environment 字段
    assert!(scene.to_scene_file().environment.is_none());
}

/// 场景文件里的越界值在载入时被收敛；没有非法值时与缺省一致。
#[test]
fn scene_load_sanitizes_environment() {
    let json = scene_json_with_env(
        r#"{ "sky_zenith": [-1.0, 1e9, 0.5], "intensity": 1e9, "sun_dir": [0.0, 0.0, 0.0] }"#,
    );
    let scene = Scene::from_json_str(&json).expect("载入场景");
    let env = scene.environment.expect("写了 environment 段就该是 Some");
    assert_eq!(env.sky_zenith[0], 0.0, "负辐亮度归零");
    assert!(env.sky_zenith[1] > 1.0, "超界值收到辐亮度上限而不是归零");
    assert_eq!(env.intensity, MAX_IBL_INTENSITY, "总强度夹到上界");
    assert_eq!(env.sun_dir, [0.0, 1.0, 0.0], "零向量方向回退为正上方");

    // 部分字段缺省：其余字段必须保持缺省值，不能因为只写了 zenith 就整段归零
    let json = scene_json_with_env(r#"{ "sky_zenith": [0.2, 0.3, 0.4] }"#);
    let scene = Scene::from_json_str(&json).expect("载入场景");
    let env = scene.environment.expect("Some");
    let d = EnvironmentSettings::default();
    assert_eq!(env.sky_zenith, [0.2, 0.3, 0.4]);
    assert_eq!(env.sky_horizon, d.sky_horizon);
    assert_eq!(env.sun_intensity, d.sun_intensity);
}

/// 写进场景的环境参数保存成 JSON 后再载入，逐字段一致。
#[test]
fn environment_survives_json_roundtrip() {
    let mut scene = Scene::new();
    scene.spawn(desc("ball")).expect("spawn");
    scene.environment = Some(
        EnvironmentSettings {
            sky_zenith: [0.1, 0.2, 0.5],
            sky_horizon: [0.6, 0.55, 0.45],
            ground: [0.05, 0.04, 0.03],
            sun_dir: [0.0, 0.6, 0.8],
            sun_color: [1.0, 0.9, 0.7],
            sun_intensity: 2.5,
            intensity: 1.5,
        }
        .sanitized(),
    );

    let text = scene.to_json_string().expect("导出场景 JSON");
    assert!(
        text.contains("\"environment\""),
        "应写出 environment 段：{text}"
    );

    let reloaded = Scene::from_json_str(&text).expect("重新载入");
    let a = scene.environment.expect("Some");
    let b = reloaded.environment.expect("Some");
    assert_eq!(a, b, "环境参数往返应逐字段一致");
}

/// 环境探针的球谐投影是缓存的依据：同一份参数两次投影结果必须逐位相同，
/// 否则渲染侧的 `env_cache` 比较永远命中不了、会退化成每帧重算。
#[test]
fn sh_projection_is_deterministic() {
    let env = EnvironmentSettings::default();
    let a = Sh9::project(&env);
    let b = Sh9::project(&env);
    assert_eq!(a.coeffs, b.coeffs);
    assert!(a.is_finite());
}
