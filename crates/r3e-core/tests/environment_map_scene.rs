//! HDR 立方体反射探针（扩展二十三）场景段的公开 API 检查。
//!
//! 覆盖三件事：缺段时场景里就是 `None`（退回解析 IBL）、有段时按 `sanitized()` 收敛、
//! `load -> save -> load` 严格对称。模块内的单测已覆盖纯函数细节，这里只走场景出口。

use r3e_core::{Scene, CUBE_FACES, DEFAULT_CUBE_FACE_SIZE, DEFAULT_SPECULAR_MIPS};

/// 没写 `environment_map` 段的老场景：场景里是 `None`，保存回去也不该长出这一段。
#[test]
fn missing_section_stays_none() {
    let json = r#"{
        "name": "legacy",
        "objects": [{ "name": "cube", "mesh": "cube" }]
    }"#;
    let scene = Scene::from_json_str(json).expect("老场景可加载");
    assert!(scene.environment_map.is_none());
    assert!(!scene.environment_map_active());

    let saved = scene.to_json_string().unwrap();
    assert!(
        !saved.contains("environment_map"),
        "缺段场景保存后不该出现 environment_map：{saved}"
    );
}

/// 写了等距柱状路径：收敛保留、激活、往返对称。
#[test]
fn equirect_section_roundtrips() {
    let json = r#"{
        "name": "hdr",
        "environment_map": {
            "equirect": "  assets/sky.hdr  ",
            "face_size": 3000,
            "mip_levels": 0,
            "intensity": 12.0
        },
        "objects": [{ "name": "ball", "mesh": "sphere" }]
    }"#;
    let scene = Scene::from_json_str(json).expect("探针场景可加载");
    let probe = scene.environment_map.as_ref().expect("配置应保留");
    // 路径去空白；面长收敛到上限的 2 的幂；层数至少 1；强度夹到 IBL 上限
    assert_eq!(probe.equirect.as_deref(), Some("assets/sky.hdr"));
    assert!(probe.face_size <= r3e_core::MAX_CUBE_FACE_SIZE);
    assert!(probe.face_size.is_power_of_two());
    assert!(probe.mip_levels >= 1);
    assert!(probe.intensity <= r3e_core::MAX_IBL_INTENSITY);
    assert!(scene.environment_map_active());

    // 往返：再过一次序列化 / 反序列化，配置完全一致
    let again = Scene::from_json_str(&scene.to_json_string().unwrap()).unwrap();
    assert_eq!(again.environment_map, scene.environment_map);
}

/// 六面图：恰好 6 条才生效，少一条整段作废（退回解析 IBL），且不影响其他段。
#[test]
fn faces_need_all_six_sides() {
    let faces = (0..CUBE_FACES)
        .map(|i| format!("faces/px{i}.hdr"))
        .collect::<Vec<_>>();
    let json = format!(
        r#"{{ "name": "faces", "environment_map": {{ "faces": [{}] }} }}"#,
        faces
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let scene = Scene::from_json_str(&json).unwrap();
    let probe = scene.environment_map.clone().unwrap();
    assert_eq!(probe.faces.as_ref().unwrap(), &faces);
    assert_eq!(probe.source_label(), "faces");
    assert_eq!(probe.face_size, DEFAULT_CUBE_FACE_SIZE);
    assert_eq!(probe.mip_levels, DEFAULT_SPECULAR_MIPS);

    // 五面：整段作废，配置保留但未激活
    let broken = format!(
        r#"{{ "environment_map": {{ "faces": [{}] }} }}"#,
        faces[..CUBE_FACES - 1]
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let scene = Scene::from_json_str(&broken).unwrap();
    assert!(scene.environment_map.as_ref().unwrap().faces.is_none());
    assert!(!scene.environment_map_active());
}

/// 脚本入口：写入 / 关开关 / 清除，三条路径都不会污染其他环境设置。
#[test]
fn script_setters_toggle_probe() {
    let mut scene = Scene::new();
    assert!(!scene.environment_map_active());

    let written =
        scene.set_environment_map(r3e_core::EnvironmentMapSettings::with_equirect("sky.hdr"));
    assert_eq!(written.equirect.as_deref(), Some("sky.hdr"));
    assert!(scene.environment_map_active());

    assert!(!scene.set_environment_map_enabled(false));
    assert!(!scene.environment_map_active());
    // 路径保留，再打开即恢复
    assert_eq!(
        scene.environment_map.as_ref().unwrap().equirect.as_deref(),
        Some("sky.hdr")
    );
    scene.set_environment_map_enabled(true);
    assert!(scene.environment_map_active());

    assert!(scene.clear_environment_map());
    assert!(scene.environment_map.is_none());
    assert!(!scene.clear_environment_map());
}
