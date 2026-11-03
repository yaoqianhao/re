//! 扩展十五：局部光源（点光 / 聚光）的场景层契约。
//!
//! 锁三件事：
//! 1. **JSON 往返对称**：`"lights": [...]` 读进来再写出去不能变形、不能丢字段；
//! 2. **收敛在入口**：场景文件里的坏值（NaN / 负 range / 内外角倒置）在 `from_file`
//!    之后必须已经合法，渲染层不需要再判；
//! 3. **按 key 去重 + 上限截断**：同名后写覆盖前写，空 key 跳过，超出上限的尾巴丢掉。

use r3e_core::{LocalLight, LocalLightKind, Scene, SceneFile, MAX_LOCAL_LIGHTS};

const POINT_JSON: &str = r#"{
    "lights": [
        {
            "key": "lamp_desk",
            "kind": "point",
            "pos": [1.0, 1.5, -2.0],
            "color": [1.0, 0.8, 0.6],
            "intensity": 6.0,
            "range": 8.0
        },
        {
            "key": "torch_hand",
            "kind": "spot",
            "pos": [0.0, 1.0, 0.0],
            "dir": [0.0, 0.0, -1.0],
            "color": [0.9, 0.95, 1.0],
            "intensity": 12.0,
            "range": 14.0,
            "inner_deg": 18.0,
            "outer_deg": 32.0
        }
    ]
}"#;

/// 两种灯都按 JSON 原样落进 `Scene`，类型 / 数值 / 方向都不走样。
#[test]
fn scene_file_loads_point_and_spot() {
    let file: SceneFile = serde_json::from_str(POINT_JSON).expect("场景 JSON 应可解析");
    let scene = Scene::from_file(file).expect("场景应可构建");
    assert_eq!(scene.lights.len(), 2);
    assert!(scene.lights_enabled, "默认应开启局部光源");

    let lamp = scene.local_light("lamp_desk").expect("点光应按 key 可查");
    assert_eq!(lamp.kind, LocalLightKind::Point);
    assert_eq!(lamp.pos, [1.0, 1.5, -2.0]);
    assert!((lamp.intensity - 6.0).abs() < 1e-6);
    assert!((lamp.range - 8.0).abs() < 1e-6);

    let torch = scene.local_light("torch_hand").expect("聚光应按 key 可查");
    assert_eq!(torch.kind, LocalLightKind::Spot);
    assert_eq!(torch.dir, [0.0, 0.0, -1.0]);
    assert!((torch.inner_deg - 18.0).abs() < 1e-6);
    assert!((torch.outer_deg - 32.0).abs() < 1e-6);
    // 余弦缓存与角度一致：内角余弦必须大于外角余弦
    assert!(torch.cos_inner() > torch.cos_outer());
}

/// 读进来再写出去，`lights` 段必须逐字段保持（含 kind 的小写串）。
#[test]
fn lights_survive_json_round_trip() {
    let file: SceneFile = serde_json::from_str(POINT_JSON).unwrap();
    let scene = Scene::from_file(file).unwrap();
    let out = scene.to_json_pretty().expect("应可序列化");
    assert!(out.contains("\"lights\""), "导出必须包含 lights 段");
    assert!(out.contains("\"spot\""), "kind 应序列化成小写串");

    let round: SceneFile = serde_json::from_str(&out).unwrap();
    let scene2 = Scene::from_file(round).unwrap();
    assert_eq!(scene.lights.len(), scene2.lights.len());
    for (a, b) in scene.lights.iter().zip(scene2.lights.iter()) {
        assert_eq!(a.key, b.key);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.pos, b.pos);
        assert_eq!(a.color, b.color);
        assert_eq!(a.dir, b.dir);
        assert!((a.intensity - b.intensity).abs() < 1e-6);
        assert!((a.range - b.range).abs() < 1e-6);
        assert!((a.inner_deg - b.inner_deg).abs() < 1e-6);
        assert!((a.outer_deg - b.outer_deg).abs() < 1e-6);
    }
}

/// 场景文件里的坏值在入口就被收敛，不留到 GPU。
#[test]
fn scene_file_sanitizes_bad_light_values() {
    let json = r#"{
        "lights": [
            {
                "key": "bad",
                "kind": "spot",
                "pos": [1.0, 2.0, 3.0],
                "color": [2.0, -1.0, 0.5],
                "intensity": -8.0,
                "range": 0.0,
                "dir": [0.0, 0.0, 0.0],
                "inner_deg": 50.0,
                "outer_deg": 10.0
            }
        ]
    }"#;
    let scene = Scene::from_file(serde_json::from_str(json).unwrap()).unwrap();
    let l = scene.local_light("bad").expect("坏值光源仍应存在（只是被夹住）");
    assert_eq!(l.intensity, 0.0, "负强度夹到 0");
    assert!(l.range > 0.0, "range=0 必须被抬到下限");
    assert_eq!(l.color[1], 0.0, "负颜色通道夹到 0");
    assert_eq!(l.dir, [0.0, -1.0, 0.0], "零方向退回朝下");
    assert!(
        l.outer_deg > l.inner_deg,
        "外角必须大于内角，实际 {} vs {}",
        l.outer_deg,
        l.inner_deg
    );
}

/// 空 key 是无效光源：读入时跳过，而不是塞一盏查不到的灯。
#[test]
fn scene_file_skips_lights_without_key() {
    let json = r#"{ "lights": [ { "intensity": 3.0 }, { "key": "  " }, { "key": "ok" } ] }"#;
    let scene = Scene::from_file(serde_json::from_str(json).unwrap()).unwrap();
    assert_eq!(scene.lights.len(), 1);
    assert!(scene.local_light("ok").is_some());
}

/// 同名后写覆盖前写：顺序不变、数量不涨。
#[test]
fn scene_file_dedupes_by_key() {
    let json = r#"{
        "lights": [
            { "key": "a", "intensity": 1.0 },
            { "key": "b", "intensity": 2.0 },
            { "key": "a", "intensity": 9.0, "pos": [5.0, 5.0, 5.0] }
        ]
    }"#;
    let scene = Scene::from_file(serde_json::from_str(json).unwrap()).unwrap();
    assert_eq!(scene.lights.len(), 2, "同 key 不应重复堆积");
    assert_eq!(scene.lights[0].key, "a");
    assert_eq!(scene.lights[1].key, "b");
    assert!((scene.lights[0].intensity - 9.0).abs() < 1e-6);
    assert_eq!(scene.lights[0].pos, [5.0, 5.0, 5.0]);
}

/// 超上限的光源被截断到 `MAX_LOCAL_LIGHTS`，且保留的是前面那些。
#[test]
fn scene_file_truncates_to_capacity() {
    let mut items = Vec::new();
    for i in 0..(MAX_LOCAL_LIGHTS + 5) {
        items.push(format!(r#"{{ "key": "l{i}", "intensity": 1.0 }}"#));
    }
    let json = format!("{{ \"lights\": [{}] }}", items.join(","));
    let scene = Scene::from_file(serde_json::from_str(&json).unwrap()).unwrap();
    assert_eq!(scene.lights.len(), MAX_LOCAL_LIGHTS);
    assert!(scene.local_light("l0").is_some());
    assert!(
        scene.local_light(&format!("l{MAX_LOCAL_LIGHTS}")).is_none(),
        "超出上限的尾巴应被丢掉"
    );
}

/// 场景级增删改：覆盖写入不涨数量、改不存在的 key 返回 false、清空返回条数。
#[test]
fn scene_light_table_supports_upsert_patch_and_clear() {
    let mut scene = Scene::new();
    assert!(!scene.upsert_local_light(LocalLight::default()), "空 key 必须拒绝");
    assert!(scene.upsert_local_light(LocalLight::point(
        "key1",
        [0.0, 3.0, 0.0],
        [1.0, 1.0, 1.0],
        2.0
    )));
    assert_eq!(scene.lights.len(), 1);

    // 覆盖同 key：内容变了、数量不变
    assert!(scene.upsert_local_light(LocalLight::point(
        "key1",
        [0.0, 4.0, 0.0],
        [1.0, 1.0, 1.0],
        5.0
    )));
    assert_eq!(scene.lights.len(), 1);
    assert!((scene.lights[0].intensity - 5.0).abs() < 1e-6);

    // 改不存在的 key 不新建
    assert!(!scene.with_local_light("nope", |l| l.intensity = 1.0));
    assert_eq!(scene.lights.len(), 1);
    // 改存在的 key 生效，且坏值被再次收敛
    assert!(scene.with_local_light("key1", |l| l.range = -1.0));
    assert!(scene.lights[0].range > 0.0);

    assert!(scene.remove_local_light("key1"));
    assert!(!scene.remove_local_light("key1"));
    assert!(scene.upsert_local_light(LocalLight::point(
        "a",
        [0.0; 3],
        [1.0; 3],
        1.0
    )));
    assert_eq!(scene.clear_local_lights(), 1);
    assert!(scene.lights.is_empty());
}

/// 没有 lights 段的旧场景照常加载（向后兼容）。
#[test]
fn legacy_scene_without_lights_still_loads() {
    let scene = Scene::from_file(serde_json::from_str("{}").unwrap()).unwrap();
    assert!(scene.lights.is_empty());
    assert!(scene.lights_enabled);
}
