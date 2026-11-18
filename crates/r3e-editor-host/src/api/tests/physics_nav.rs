//! M1-S4-D 宿主层：`physics.*` / `nav.*` / `scene.material.*` 八条。
//!
//! 三组各管一件事：
//!
//! * 物理探测——**「没有」和「不会」必须分开**：物理关着回 [`PHYSICS_DISABLED`]，
//!   后端不申报该位回 [`BACKEND_UNSUPPORTED`]，两条都不许退化成空结果。
//! * 导航——烘焙产物住宿主槽位、不进 `Document`，所以 `bake` 前后场景哈希必须一致；
//!   没烘过一律 [`NAV_NOT_BAKED`]，不拿空路径假装「场景走不通」。
//! * 材质表——`upsert` 是**整行声明**而不是打补丁，`list` 的 `users` 反查出引用者。

use super::*;
use r3e_editor::{
    CastShapeSpec, BACKEND_UNSUPPORTED, INVALID_PARAMS, NAV_NOT_BAKED, PHYSICS_DISABLED,
};
use r3e_nav::Nav;
use r3e_physics::NullBackend;

// ---- physics.* ----

/// 物理关闭（默认态）：三条都必须明确报「关着」，不能返回空结果。
#[test]
fn physics三条探测在物理关闭时都明确报错() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let failures = [
        api.physics_raycast(RayQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
        })
        .expect_err("物理关着，raycast 不该成功"),
        api.physics_overlap(OverlapQuery {
            center: [0.0, 0.0, 0.0],
            radius: 1.0,
            limit: None,
        })
        .expect_err("物理关着，overlap 不该成功"),
        api.physics_cast(CastQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
            shape: CastShapeSpec::Sphere { radius: 0.3 },
        })
        .expect_err("物理关着，cast 不该成功"),
    ];

    for failure in failures {
        assert_eq!(failure.code, PHYSICS_DISABLED, "错误码不对：{failure:?}");
        assert!(
            failure.message.contains("物理已关闭"),
            "消息要说清是「关着」而不是「查不到」：{}",
            failure.message
        );
    }
}

/// 后端不申报能力位：开机的 null 后端 ≠ 有能力，必须回 `BACKEND_UNSUPPORTED`。
#[test]
fn physics后端不申报能力位时报不支持() {
    let (scene, _physics, mut scripts) = fixture();
    // `with_backend` 只设「启用」，能力位仍由后端自己申报——null 后端申报的是 `NONE`。
    let physics = Rc::new(RefCell::new(Physics::with_backend(Box::new(
        NullBackend::new(),
    ))));
    let mut api = api(&scene, &physics, &mut scripts);

    let failure = api
        .physics_raycast(RayQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
        })
        .expect_err("null 后端不该给出查询结果");
    assert_eq!(failure.code, BACKEND_UNSUPPORTED, "错误码不对：{failure:?}");
    assert!(
        failure.message.contains("raycast"),
        "消息要点名缺哪个能力位：{}",
        failure.message
    );
}

/// 内置后端申报了查询能力：成功回包自带 `backend` + `capabilities`，不必先探一次。
#[test]
fn physics探测回包自带能力位() {
    let (scene, physics, mut scripts) = fixture();
    physics.borrow_mut().set_enabled(true);
    let mut api = api(&scene, &physics, &mut scripts);

    let ray = api
        .physics_raycast(RayQuery {
            origin: [0.0, 50.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
        })
        .expect("内置后端应能打射线");
    assert!(
        ray["capabilities"]
            .as_array()
            .expect("capabilities 应为数组")
            .iter()
            .any(|k| k == "raycast"),
        "回包要带 raycast 能力位：{ray}"
    );
    assert!(
        ray["backend"].as_str().is_some_and(|s| !s.is_empty()),
        "回包要带后端名：{ray}"
    );
    // 打空了也要带能力位——「没打到」与「不会打」是两件事。
    assert_eq!(ray["hit"], false, "空场景上方朝下打，射程外应无命中：{ray}");

    let overlap = api
        .physics_overlap(OverlapQuery {
            center: [0.0, 0.0, 0.0],
            radius: 1.0,
            limit: None,
        })
        .expect("内置后端应能查重叠");
    assert_eq!(overlap["count"], 0, "空场景没有刚体：{overlap}");
    assert!(
        overlap["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|k| k == "overlap")),
        "空表也要带能力位解释为什么空：{overlap}"
    );

    let cast = api
        .physics_cast(CastQuery {
            origin: [0.0, 50.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
            shape: CastShapeSpec::Box {
                half: [0.2, 0.2, 0.2],
            },
        })
        .expect("内置后端应能扫掠");
    assert!(
        cast["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|k| k == "shape_cast")),
        "回包要带 shape_cast 能力位：{cast}"
    );
}

/// 非法入参：零方向、负射程、零半径都在宿主侧判参数错误，不悄悄换成默认值。
#[test]
fn physics探测拒绝非法入参() {
    let (scene, physics, mut scripts) = fixture();
    physics.borrow_mut().set_enabled(true);
    let mut api = api(&scene, &physics, &mut scripts);

    let zero_dir = api
        .physics_raycast(RayQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, 0.0, 0.0],
            max_distance: None,
        })
        .expect_err("零向量方向应报错");
    assert_eq!(zero_dir.code, INVALID_PARAMS, "{zero_dir:?}");

    let negative = api
        .physics_raycast(RayQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: Some(-5.0),
        })
        .expect_err("负射程应报错，不能解释成「不限」");
    assert_eq!(negative.code, INVALID_PARAMS, "{negative:?}");

    let zero_radius = api
        .physics_overlap(OverlapQuery {
            center: [0.0, 0.0, 0.0],
            radius: 0.0,
            limit: None,
        })
        .expect_err("零半径球应报错");
    assert_eq!(zero_radius.code, INVALID_PARAMS, "{zero_radius:?}");

    let degenerate = api
        .physics_cast(CastQuery {
            origin: [0.0, 1.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
            shape: CastShapeSpec::Sphere { radius: 0.0 },
        })
        .expect_err("退化形状应报错");
    assert_eq!(degenerate.code, INVALID_PARAMS, "{degenerate:?}");
}

/// 三条探测都是只读：不写场景、不推版本。
#[test]
fn physics三条探测都不改场景哈希() {
    let (scene, physics, mut scripts) = fixture();
    physics.borrow_mut().set_enabled(true);
    let before = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };

    {
        let mut api = api(&scene, &physics, &mut scripts);
        api.physics_raycast(RayQuery {
            origin: [0.0, 50.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
        })
        .expect("raycast 应成功");
        api.physics_overlap(OverlapQuery {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
            limit: None,
        })
        .expect("overlap 应成功");
        api.physics_cast(CastQuery {
            origin: [0.0, 50.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
            shape: CastShapeSpec::Sphere { radius: 0.3 },
        })
        .expect("cast 应成功");
    }

    let after = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };
    assert_eq!(before, after, "探测不该改动场景");
}

// ---- scene.material.* ----

fn material_decl(name: &str) -> MaterialUpsert {
    MaterialUpsert {
        name: name.to_string(),
        ..Default::default()
    }
}

/// `upsert` 新建 → 覆盖，`count` 不涨、`created` 翻转，且确实写进了场景。
#[test]
fn material_upsert新建后覆盖同一行() {
    let (scene, physics, mut scripts) = fixture();
    let before = scene.borrow().state_hash();

    let mut api = api(&scene, &physics, &mut scripts);
    let created = api
        .material_upsert(MaterialUpsert {
            metallic: Some(0.9),
            emissive: Some([1.0, 0.0, 0.0]),
            ..material_decl("brass")
        })
        .expect("新建材质应成功");
    assert_eq!(created["created"], true, "{created}");
    assert_eq!(created["count"], 1, "{created}");
    assert_eq!(created["material"]["name"], "brass", "{created}");

    let updated = api
        .material_upsert(MaterialUpsert {
            metallic: Some(0.1),
            ..material_decl("brass")
        })
        .expect("覆盖同名材质应成功");
    assert_eq!(updated["created"], false, "同名覆盖不该算新建：{updated}");
    assert_eq!(updated["count"], 1, "覆盖不该让表变长：{updated}");
    let metallic = updated["material"]["metallic"]
        .as_f64()
        .expect("metallic 应为数字");
    assert!(
        (metallic - 0.1).abs() < 1e-6,
        "覆盖后金属度应为 0.1：{updated}"
    );
    drop(api);

    assert_ne!(
        before,
        scene.borrow().state_hash(),
        "材质表是场景数据，写入应推哈希"
    );
    assert_eq!(scene.borrow().write_count(), 2, "两次 upsert 应各写一次");
}

/// 整行声明语义（与 `*Patch` 的补丁语义不同）：没给贴图就是「这一行没有贴图」。
#[test]
fn material_upsert是整行声明不是打补丁() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    api.material_upsert(MaterialUpsert {
        albedo: Some("assets/wood.png".to_string()),
        ..material_decl("wood")
    })
    .expect("带贴图的 upsert 应成功");

    let overwritten = api
        .material_upsert(MaterialUpsert {
            roughness: Some(0.4),
            ..material_decl("wood")
        })
        .expect("整行覆盖应成功");
    assert!(
        overwritten["material"]["textures"]
            .as_object()
            .is_some_and(|m| m.is_empty()),
        "没给贴图 = 这一行没有贴图（整行声明，不是「别动贴图」）：{overwritten}"
    );
}

/// `list` 的 `users` 按 `materialRef` 反查引用者——「改了会牵连谁」一眼可见。
#[test]
fn material_list按引用反查users() {
    let (scene, physics, mut scripts) = fixture();
    {
        let mut doc = scene.borrow_mut();
        doc.write(|scene_ref| {
            let cube = scene_ref.find_id("cube").expect("cube 应存在");
            let node = scene_ref.get_mut(cube).expect("cube 应可取");
            node.material_ref = Some("wood".to_string());
        });
    }

    let mut api = api(&scene, &physics, &mut scripts);
    api.material_upsert(MaterialUpsert {
        metallic: Some(0.2),
        ..material_decl("wood")
    })
    .expect("upsert 应成功");
    let listed = api.material_list().expect("list 应成功");

    assert_eq!(listed["count"], 1, "{listed}");
    let users = listed["materials"][0]["users"]
        .as_array()
        .expect("users 应为数组");
    assert_eq!(users.len(), 1, "只有一个节点引用：{listed}");
    assert_eq!(users[0], "cube", "{listed}");
}

/// 名字收敛后为空 = 参数错误，且不写场景。
#[test]
fn material_upsert拒绝空名() {
    let (scene, physics, mut scripts) = fixture();
    let before = scene.borrow().write_count();
    let mut api = api(&scene, &physics, &mut scripts);

    let failure = api
        .material_upsert(material_decl("   "))
        .expect_err("空名应报错");
    assert_eq!(failure.code, INVALID_PARAMS, "{failure:?}");
    assert_eq!(scene.borrow().write_count(), before, "报错不该留下写操作");
}

// ---- nav.* ----

/// 宿主没接槽位（headless）= 「没有网格」，必须回 `NAV_NOT_BAKED`。
#[test]
fn nav查询在无槽位时报未烘焙() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);

    let failure = api
        .nav_path(NavPathQuery {
            from: [0.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0],
        })
        .expect_err("没烘过不该给出路径");
    assert_eq!(failure.code, NAV_NOT_BAKED, "{failure:?}");
    assert!(
        failure.message.contains("nav.bake"),
        "消息要指出下一步：{}",
        failure.message
    );
}

/// 槽位接上了但还没烘：与「没有槽位」同一件事，同一错误码。
#[test]
fn nav查询在未烘焙时报未烘焙() {
    let (scene, physics, mut scripts) = fixture();
    let slot: RefCell<Option<Nav>> = RefCell::new(None);
    let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));

    let path = api
        .nav_path(NavPathQuery {
            from: [0.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0],
        })
        .expect_err("空槽位不该给出路径");
    assert_eq!(path.code, NAV_NOT_BAKED, "{path:?}");

    let reach = api
        .nav_reachability(NavReachQuery {
            from: [0.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0],
        })
        .expect_err("空槽位不该给出可达性");
    assert_eq!(reach.code, NAV_NOT_BAKED, "{reach:?}");
}

/// 有地面的场景：`bake` 只写槽位不写场景，`bake` 完就能查。
#[test]
fn nav烘焙只写槽位且烘焙后可查询() {
    let (scene, physics, mut scripts) = fixture();
    {
        let mut doc = scene.borrow_mut();
        doc.write(|scene_ref| {
            scene_ref
                .spawn(ObjectDesc {
                    name: "ground".to_string(),
                    mesh: Some("cube".to_string()),
                    position: Some([0.0, -0.5, 0.0]),
                    scale: Some([20.0, 1.0, 20.0]),
                    ..Default::default()
                })
                .expect("地面 spawn 不应失败");
        });
    }
    let before = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };

    let slot: RefCell<Option<Nav>> = RefCell::new(None);
    let report = {
        let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
        let report = api
            .nav_bake(NavBakeOptions::default())
            .expect("带地面的场景应能烘出网格");
        assert_eq!(report["baked"], true, "{report}");
        assert!(
            report["cells"].as_u64().is_some_and(|c| c > 0),
            "烘焙应产出格子：{report}"
        );
        assert!(report["dims"].is_array(), "回包应带网格尺寸：{report}");
        report
    };
    assert!(slot.borrow().is_some(), "烘好的网格应落进宿主槽位");

    let after = {
        let doc = scene.borrow();
        (doc.state_hash(), doc.write_count())
    };
    assert_eq!(before, after, "烘焙产物不进场景，哈希与写次数都不该变");

    let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
    let path = api
        .nav_path(NavPathQuery {
            from: [0.0, 0.0, 0.0],
            to: [4.0, 0.0, 0.0],
        })
        .expect("烘完应能求路径");
    assert!(
        path["outcome"].as_str().is_some(),
        "四种结局必须如实区分：{path}"
    );
    assert!(path["found"].is_boolean(), "{path}");
    assert!(path["points"].is_array(), "{path}");
    assert!(
        path["reason"].as_str().is_some_and(|s| !s.is_empty()),
        "每种结局都要给出人能读的理由：{path}"
    );

    let reach = api
        .nav_reachability(NavReachQuery {
            from: [0.0, 0.0, 0.0],
            to: [4.0, 0.0, 0.0],
        })
        .expect("烘完应能判可达性");
    assert!(reach["reachable"].is_boolean(), "{reach}");
    assert!(
        reach["fromRegion"].is_number(),
        "应带起点所在连通块：{reach}"
    );
    assert!(reach["toRegion"].is_number(), "应带终点所在连通块：{reach}");

    assert!(
        report["walkable"].as_u64().is_some(),
        "回包应带可行走格子数：{report}"
    );
}

/// 烘焙失败不许把上一次成功的网格冲掉：失败后原有网格仍可查。
#[test]
fn nav烘焙失败保留上一次网格() {
    let (scene, physics, mut scripts) = fixture();
    {
        let mut doc = scene.borrow_mut();
        doc.write(|scene_ref| {
            scene_ref
                .spawn(ObjectDesc {
                    name: "ground".to_string(),
                    mesh: Some("cube".to_string()),
                    position: Some([0.0, -0.5, 0.0]),
                    scale: Some([20.0, 1.0, 20.0]),
                    ..Default::default()
                })
                .expect("地面 spawn 不应失败");
        });
    }
    let slot: RefCell<Option<Nav>> = RefCell::new(None);

    {
        let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
        api.nav_bake(NavBakeOptions::default())
            .expect("首次烘焙应成功");
    }

    {
        let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
        let failure = api
            .nav_bake(NavBakeOptions {
                // min > max：宿主侧判参数错误，且不该动槽位。
                bounds: Some([5.0, 5.0, -5.0, -5.0]),
                ..Default::default()
            })
            .expect_err("非法 bounds 应报错");
        assert_eq!(failure.code, INVALID_PARAMS, "{failure:?}");
        assert!(slot.borrow().is_some(), "失败不该冲掉已有网格");
    }

    let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
    api.nav_path(NavPathQuery {
        from: [0.0, 0.0, 0.0],
        to: [2.0, 0.0, 0.0],
    })
    .expect("原有网格仍应可查");
}
