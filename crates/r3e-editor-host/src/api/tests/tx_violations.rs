//! 物理 / 导航层违规的事务内测试（M1-S4-G）：这两层要证明的不是「能跑」，
//! 而是「跑过的层登记在 `layers`、没跑成的层连原因一起记账」——否则差集回包
//! 读起来就是「没报 = 没问题」。

use super::*;

/// 物理开着、但宿主没拿到网格来源：这一层算**跑过**（静态可判的穿模照跑），
/// 精确悬空跳过，且必须把「为什么跳过」写进 `unchecked`——既不假绿也不装瞎。
#[test]
fn tx_intent_impact_violations_物理开着时报物理层并交代跳过项() {
    let scene = fixture_physics_on();
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    let mut api = api(&scene, &physics, &mut scripts);
    let tx = begin(&mut api);

    let value = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "sky", "position": [0.0, 60.0, 0.0] }])),
        )
        .expect("意图应被接收");

    let violations = &value["impact"]["violations"];
    let layers = violations["layers"].as_array().expect("layers 应为数组");
    assert!(
        layers.iter().any(|l| l == "physics"),
        "物理开着就跑过了这一层，必须登记：{violations}"
    );
    let not_checked = violations["notChecked"]
        .as_array()
        .expect("notChecked 应为数组");
    assert!(
        !not_checked.iter().any(|l| l == "physics"),
        "跑过的层不许再挂在 notChecked 里：{violations}"
    );
    let unchecked = violations["unchecked"]
        .as_array()
        .expect("unchecked 应为数组");
    assert!(
        unchecked.iter().any(|u| u["layer"] == "physics"
            && u["reason"].as_str().is_some_and(|r| r.contains("网格来源"))),
        "没有网格来源就装配不出影子世界，必须如实交代：{violations}"
    );
}

/// 主场景烘过导航后，影子那份当场重烘再比：这一层同样要登记为「跑过」，
/// 而 `tx.intent` 不带目标点这件事必须在 `unchecked` 里点名。
#[test]
fn tx_intent_impact_violations_导航已烘焙时报导航层() {
    let mut scene = Scene::new();
    scene.terrain.push(r3e_core::TerrainPatch::flat(
        "ground",
        [0.0, 0.0],
        [20.0, 20.0],
    ));
    scene
        .spawns
        .push(r3e_core::SpawnDecl::new("start", [2.5, 0.0, 2.5]));
    let scene = Rc::new(RefCell::new(Document::new(scene)));
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    let slot: RefCell<Option<Nav>> = RefCell::new(None);
    let mut api = api_with_nav(&scene, &physics, &mut scripts, Some(&slot));
    api.nav_bake(NavBakeOptions::default())
        .expect("主场景应可烘焙");

    let tx = begin(&mut api);
    let value = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{ "name": "far", "position": [5.0, 0.5, 5.0] }])),
        )
        .expect("意图应被接收");

    let violations = &value["impact"]["violations"];
    let layers = violations["layers"].as_array().expect("layers 应为数组");
    assert!(
        layers.iter().any(|l| l == "nav"),
        "主场景烘过导航，影子当场重烘后这一层就该算跑过：{violations}"
    );
    let not_checked = violations["notChecked"]
        .as_array()
        .expect("notChecked 应为数组");
    assert!(
        !not_checked.iter().any(|l| l == "nav"),
        "跑过的层不许再挂在 notChecked 里：{violations}"
    );
    let unchecked = violations["unchecked"]
        .as_array()
        .expect("unchecked 应为数组");
    assert!(
        unchecked
            .iter()
            .any(|u| u["layer"] == "nav"
                && u["reason"].as_str().is_some_and(|r| r.contains("目标点"))),
        "接口不带目标点 → nav.no_path 没查，必须点名：{violations}"
    );
}

/// 拿到网格来源时物理层走**装配**：影子世界当场重建，悬空是精确结论
/// （`approximate = false`），而不是「解析近似 + 一句装配不了的说明」。
#[test]
fn tx_intent_impact_violations_有网格来源时物理层出精确悬空结论() {
    let scene = Rc::new(RefCell::new(Document::new(Scene::new())));
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    let mut api = api_with_meshes(
        &scene,
        &physics,
        &mut scripts,
        Arc::new(r3e_core::ProceduralProvider),
    );
    let tx = begin(&mut api);

    // 半米见方的静态体悬在 5 米高：离线也没有地板，必然判悬空。
    let value = api
        .tx_intent(
            &tx,
            "scene.spawn_prefab",
            &nodes(json!([{
                "name": "ledge",
                "mesh": "cube",
                "position": [0.0, 5.0, 0.0],
                "physics": { "body": "static", "collider": "box", "half_extents": [0.5, 0.5, 0.5] }
            }])),
        )
        .expect("意图应被接收");

    let violations = &value["impact"]["violations"];
    let unchecked = violations["unchecked"]
        .as_array()
        .expect("unchecked 应为数组");
    assert!(
        !unchecked
            .iter()
            .any(|u| u["reason"].as_str().is_some_and(|r| r.contains("网格来源"))),
        "有网格来源就不该再拿它当借口：{violations}"
    );
    let issues = violations["issues"].as_array().expect("issues 应为数组");
    assert!(
        issues.iter().any(|i| i["code"] == "support.floating"
            && i["target"] == "ledge"
            && i["approximate"] == json!(false)),
        "镜头里没有地板、没有别的碰撞体：精确结论必须是「悬空」，且不许标近似：{violations}"
    );
}
