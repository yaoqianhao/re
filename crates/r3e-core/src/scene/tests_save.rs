//! 场景保存（save）的往返测试：`load -> save -> load` 必须语义等价。
//!
//! 覆盖仓库里随包发布的全部场景文件；用 `include_str!` 内联，测试不依赖工作目录。

use std::collections::BTreeSet;

use serde_json::Value;

use super::*;

/// 随仓库发布的场景：(文件名, 文本)。
const SHIPPED_SCENES: [(&str, &str); 6] = [
    ("scene.json", include_str!("../../../../assets/scene.json")),
    (
        "scene_physics.json",
        include_str!("../../../../assets/scene_physics.json"),
    ),
    (
        "scene_rapier.json",
        include_str!("../../../../assets/scene_rapier.json"),
    ),
    (
        "scene_ccd.json",
        include_str!("../../../../assets/scene_ccd.json"),
    ),
    (
        "scene_shadow.json",
        include_str!("../../../../assets/scene_shadow.json"),
    ),
    (
        "scene_anim.json",
        include_str!("../../../../assets/scene_anim.json"),
    ),
];

/// 浮点比较的相对容差：`quat -> euler -> quat` 之类的换算会带来末位噪声。
const EPS: f64 = 1e-5;

/// 递归比较两个 JSON 值：数字按容差比较，其它类型严格比较，键集合必须一致。
fn assert_json_close(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().expect("数字必须能转 f64"),
                y.as_f64().expect("数字必须能转 f64"),
            );
            assert!(
                (x - y).abs() <= EPS * x.abs().max(1.0),
                "`{path}` 数值不一致: {x} vs {y}"
            );
        }
        (Value::Array(xs), Value::Array(ys)) => {
            assert_eq!(xs.len(), ys.len(), "`{path}` 数组长度不一致");
            for (i, (x, y)) in xs.iter().zip(ys.iter()).enumerate() {
                assert_json_close(x, y, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(xs), Value::Object(ys)) => {
            let xk: BTreeSet<&String> = xs.keys().collect();
            let yk: BTreeSet<&String> = ys.keys().collect();
            assert_eq!(xk, yk, "`{path}` 字段集合不一致");
            for (k, x) in xs {
                assert_json_close(x, &ys[k], &format!("{path}.{k}"));
            }
        }
        _ => assert_eq!(a, b, "`{path}` 不一致"),
    }
}

/// 场景的可保存投影（`SceneFile` 序列化后的 JSON），用于整体比对。
fn scene_projection(scene: &Scene) -> Value {
    serde_json::to_value(scene.to_scene_file()).expect("SceneFile 必须可序列化")
}

/// 断言两个场景的源数据等价。
fn assert_scene_equivalent(a: &Scene, b: &Scene, label: &str) {
    assert_json_close(&scene_projection(a), &scene_projection(b), label);
    assert_eq!(a.len(), b.len(), "`{label}` 物件数量不一致");
    assert_eq!(
        a.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        b.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        "`{label}` 物件顺序不一致"
    );
}

#[test]
fn shipped_scenes_round_trip() {
    for (name, src) in SHIPPED_SCENES {
        let loaded = Scene::from_json_str(src).unwrap_or_else(|e| panic!("{name} 加载失败: {e}"));
        let saved = loaded
            .to_json_string()
            .unwrap_or_else(|e| panic!("{name} 序列化失败: {e}"));
        assert!(!saved.trim().is_empty(), "{name} 序列化结果为空");
        let reloaded = Scene::from_json_str(&saved)
            .unwrap_or_else(|e| panic!("{name} 回读失败: {e}\n{saved}"));

        assert_eq!(loaded.name, reloaded.name, "{name} 场景名丢失");
        assert_scene_equivalent(&loaded, &reloaded, name);
    }
}

#[test]
fn resave_after_reload_is_byte_stable() {
    // 第一次 save 会把文件规范成「引擎的写法」（欧拉角、省略缺省字段）；
    // 再 load -> save 应当逐字节一致，否则说明往返里还有未收敛的噪声。
    for (name, src) in SHIPPED_SCENES {
        let once = Scene::from_json_str(src)
            .unwrap_or_else(|e| panic!("{name} 加载失败: {e}"))
            .to_json_pretty()
            .expect("序列化");
        let twice = Scene::from_json_str(&once)
            .unwrap_or_else(|e| panic!("{name} 回读失败: {e}\n{once}"))
            .to_json_pretty()
            .expect("序列化");
        assert_eq!(once, twice, "{name} 规范化后应逐字节稳定");
    }
}

#[test]
fn save_omits_runtime_state() {
    let src = include_str!("../../../../assets/scene.json");
    let scene = Scene::from_json_str(src).expect("scene.json 必须合法");
    let value: Value = serde_json::from_str(&scene.to_json_string().expect("序列化"))
        .expect("序列化结果必须是合法 JSON");

    // 顶层不得出现运行时记账
    let top: BTreeSet<&String> = value.as_object().expect("场景必须是对象").keys().collect();
    for forbidden in [
        "time",
        "frame",
        "skins",
        "animations",
        "anim_clips",
        "next_id",
    ] {
        assert!(
            !top.contains(&forbidden.to_string()),
            "顶层不应写 `{forbidden}`"
        );
    }

    // 物件不得写世界矩阵缓存、刚体句柄、子节点索引
    let objects = value["objects"].as_array().expect("objects 必须是数组");
    assert!(!objects.is_empty(), "scene.json 应当有物件");
    for object in objects {
        let keys: BTreeSet<&String> = object.as_object().expect("物件必须是对象").keys().collect();
        for forbidden in ["id", "world", "world_normal", "body", "children"] {
            assert!(
                !keys.contains(&forbidden.to_string()),
                "物件不应写 `{forbidden}`：{object}"
            );
        }
    }
}

#[test]
fn save_reflects_runtime_edits() {
    let mut scene = Scene::from_json_str(include_str!("../../../../assets/scene.json"))
        .expect("scene.json 必须合法");

    // 运行时改动：新增物件、移动、隐藏、挂物理描述、改相机与光
    let id = scene
        .spawn(ObjectDesc {
            name: "saved_probe".to_string(),
            mesh: Some("sphere".to_string()),
            position: Some([1.5, 2.5, -3.5]),
            rotation: Some([0.0, 45.0, 0.0]),
            scale: Some([0.5, 0.5, 0.5]),
            visible: Some(false),
            physics: Some(PhysicsDesc {
                body: "dynamic".to_string(),
                mass: 2.5,
                restitution: None,
                friction: None,
                collider: None,
                half_extents: None,
                ccd: Some(true),
            }),
            ..Default::default()
        })
        .expect("spawn 应成功");
    scene.get_mut(id).expect("物件应存在").visible = false;
    scene.camera.position = Vec3::new(1.0, 2.0, 3.0);
    scene.light.intensity = 0.75;
    scene.name = "edited".to_string();
    scene.update_world_transforms();

    let text = scene.to_json_pretty().expect("序列化");
    let back = Scene::from_json_str(&text).expect("回读必须成功");
    assert_scene_equivalent(&scene, &back, "edited");

    let probe = back.find("saved_probe").expect("新增物件必须写回");
    assert_eq!(probe.mesh, MeshRef::Builtin(crate::BuiltinMesh::Sphere));
    assert!(!probe.visible, "隐藏状态必须写回");
    assert_eq!(probe.transform.position, Vec3::new(1.5, 2.5, -3.5));
    assert_eq!(probe.transform.scale, Vec3::splat(0.5));
    let physics = probe.physics.as_ref().expect("物理描述必须写回");
    assert_eq!(physics.mass, 2.5);
    assert_eq!(physics.ccd, Some(true));
}

#[test]
fn save_to_path_writes_readable_file() {
    let scene = Scene::from_json_str(include_str!("../../../../assets/scene_shadow.json"))
        .expect("scene_shadow.json 必须合法");
    let dir = std::env::temp_dir().join(format!("r3e-save-{}", std::process::id()));
    let path = dir.join("nested/scene_out.json");

    scene
        .save_to_path(&path)
        .unwrap_or_else(|e| panic!("保存失败: {e}"));
    let text = std::fs::read_to_string(&path).expect("写入后必须能读回");
    assert_eq!(
        text,
        scene.to_json_pretty().expect("序列化"),
        "落盘内容应与内存序列化一致"
    );
    assert!(text.ends_with("}\n"), "落盘文件应以换行结尾");
    let back = Scene::from_json_str(&text).expect("落盘内容必须能回读");
    assert_scene_equivalent(&scene, &back, "scene_shadow.json/disk");

    std::fs::remove_file(&path).expect("清理临时文件");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hierarchy_parent_survives_round_trip() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "root".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    scene
        .spawn(ObjectDesc {
            name: "child".to_string(),
            parent: Some("root".to_string()),
            position: Some([0.0, 1.0, 0.0]),
            ..Default::default()
        })
        .expect("spawn 应成功");
    scene.update_world_transforms();

    let back = Scene::from_json_str(&scene.to_json_string().expect("序列化")).expect("回读");
    let child = back.find("child").expect("子物件必须存在");
    let parent = back.find("root").expect("父物件必须存在");
    assert_eq!(child.parent, Some(parent.id), "父子关系必须写回");
    assert!(parent.children.contains(&child.id), "父节点应列出子节点");
    // 世界矩阵是算出来的，不是写出来的
    assert!((child.world.w_axis.y - 1.0).abs() < 1e-6);
}
