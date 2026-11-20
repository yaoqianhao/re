//! M1-S4-H：`physics.bind` 编辑态显式装配口。
//!
//! 这条口子只解决一件事：编辑态暂停时运行时不再 `sync_physics_bindings`，主世界是空的，
//! 于是 `physics.raycast` 的 `hit: false` 无法区分「真没打中」和「世界没装」。
//! 因此本文件的断言全部围绕**可验证性**：
//!
//! * 装配**有效**：装完真能查到东西（不是回个 `bound: 0` 交差）；
//! * 装配**幂等**：连调两次第二次 `created = 0`、`reused = bound()`；
//! * 装配**不碰场景**：`Node::body` 仍为空、`stateHash` 前后一致、`wroteBackScene` 为 `false`；
//! * 缺条件时**报专用错误码**，绝不回空表让人以为场景里本来就没刚体。

use super::*;
use r3e_core::ObjectDesc;
use r3e_editor::{MESH_SOURCE_UNAVAILABLE, PHYSICS_DISABLED};

/// 一块 10×1×10 的静态地面（顶面在 y = 0），外加一个悬在半空的动态方块。
///
/// 不用 `scene_doc()` 的 fixture：那三个 cube 都不带 `physics` 段，`bind` 只会老实回
/// `candidates: 0`，测不出「装上了」和「没装上」的区别。
fn scene_with_bodies() -> Rc<RefCell<Document>> {
    let mut scene = Scene::new();
    for desc in [
        json!({
            "name": "ground",
            "mesh": "cube",
            "position": [0.0, -0.5, 0.0],
            "scale": [10.0, 1.0, 10.0],
            "physics": { "body": "static", "collider": "box", "half_extents": [5.0, 0.5, 5.0] }
        }),
        json!({
            "name": "crate",
            "mesh": "cube",
            "position": [0.0, 3.0, 0.0],
            "physics": { "body": "dynamic", "collider": "box", "half_extents": [0.5, 0.5, 0.5] }
        }),
    ] {
        let desc: ObjectDesc = serde_json::from_value(desc).expect("测试场景描述应能解析");
        scene.spawn(desc).expect("测试节点应能落进场景");
    }
    Rc::new(RefCell::new(Document::new(scene)))
}

/// 带网格来源的宿主：`ProceduralProvider` 提供 `cube` 图元，装配才有形状可推。
fn api_with_scene<'a>(
    scene: &'a Rc<RefCell<Document>>,
    physics: &'a Rc<RefCell<Physics>>,
    scripts: &'a mut ScriptHost,
) -> LocalEngineApi<'a> {
    api_with_meshes(
        scene,
        physics,
        scripts,
        Arc::new(r3e_core::ProceduralProvider),
    )
}

/// 装配**真的把刚体放进了主世界**，并且**一个字节都没写回场景**。
#[test]
fn physics_bind_装配主世界且不写回场景() {
    let scene = scene_with_bodies();
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    let mut api = api_with_scene(&scene, &physics, &mut scripts);

    let before = api.scene.borrow().state_hash();
    let report = api
        .physics_bind()
        .expect("物理开着且有网格来源，装配应成功");

    assert_eq!(
        report["candidates"],
        json!(2),
        "两个节点都声明了 physics 段：{report}"
    );
    assert_eq!(report["bound"], json!(2), "两个节点都该绑上：{report}");
    assert_eq!(report["created"], json!(2), "第一次装配全是新建：{report}");
    assert_eq!(
        report["reused"],
        json!(0),
        "第一次装配没有可复用的句柄：{report}"
    );
    assert_eq!(
        report["bodyCount"],
        json!(2),
        "世界里的刚体总数应与绑定数一致：{report}"
    );
    assert_eq!(
        report["wroteBackScene"],
        json!(false),
        "必须显式声明没写场景：{report}"
    );
    assert!(
        report["simulated"] == json!(true),
        "内置后端申报了查询能力位，`simulated` 不该是 false：{report}"
    );
    // 句柄要能翻回物件名，AI 拿一串数字认不出绑的是谁。
    let bindings = report["bindings"].as_array().expect("bindings 应为数组");
    let names: Vec<&str> = bindings.iter().filter_map(|b| b["node"].as_str()).collect();
    assert!(
        names.contains(&"ground") && names.contains(&"crate"),
        "绑定表要点名节点：{report}"
    );

    // 场景侧零写入：哈希不变、`Node::body` 仍然一个都没有。
    assert_eq!(
        report["stateHash"],
        json!(super::super::tx::hex(before)),
        "回包哈希应等于调用前的场景哈希：{report}"
    );
    {
        let document = api.scene.borrow();
        let scene: &Scene = &document;
        let bound_nodes: Vec<&str> = scene
            .iter()
            .filter(|n| n.body.is_some())
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            bound_nodes.is_empty(),
            "`physics.bind` 不许写 `Node::body`，这些节点被写了：{bound_nodes:?}"
        );
    }

    // 装配有效性的实证：地面在 y = 0，从 y = 2 往下打必然命中——这正是编辑态
    // 之前拿不到的可信结论。
    let hit = api
        .physics_raycast(RayQuery {
            origin: [0.0, 2.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            max_distance: None,
        })
        .expect("装配完的射线查询应成功");
    assert_eq!(
        hit["hit"],
        json!(true),
        "地面已装进世界，向下的射线必须命中：{hit}"
    );
}

/// 幂等：连调两次，第二次全是复用，世界里的刚体数不涨。
#[test]
fn physics_bind_连调两次第二次全是复用() {
    let scene = scene_with_bodies();
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    let mut api = api_with_scene(&scene, &physics, &mut scripts);

    let first = api.physics_bind().expect("第一次装配应成功");
    let second = api.physics_bind().expect("第二次装配应成功");

    assert_eq!(first["created"], json!(2), "第一次应新建两个：{first}");
    assert_eq!(second["created"], json!(0), "第二次不该再新建：{second}");
    assert_eq!(
        second["reused"], second["bound"],
        "第二次的绑定应全部来自复用：{second}"
    );
    assert_eq!(
        second["bodyCount"], first["bodyCount"],
        "幂等装配不该让世界里的刚体数翻倍：{first} / {second}"
    );
}

/// 物理关着：回 [`PHYSICS_DISABLED`]，不回「绑了 0 个」。
#[test]
fn physics_bind_物理关闭时报专用错误码() {
    // `fixture()` 的 `Physics::new()` 默认关闭。
    let (scene, physics, mut scripts) = fixture();
    let mut api = api_with_scene(&scene, &physics, &mut scripts);

    let failure = api.physics_bind().expect_err("物理关着，绑定不该成功");
    assert_eq!(failure.code, PHYSICS_DISABLED, "错误码不对：{failure:?}");
    assert!(
        failure.message.contains("物理已关闭"),
        "消息要说清是「关着」而不是「绑不上」：{}",
        failure.message
    );
    assert!(
        api.physics.borrow().body_count() == 0,
        "报错路径不许偷偷装配"
    );
}

/// 没有网格来源：回 [`MESH_SOURCE_UNAVAILABLE`]，与后端「不会」区分开。
#[test]
fn physics_bind_无网格来源时报专用错误码() {
    let scene = scene_with_bodies();
    let physics = Rc::new(RefCell::new(physics_on()));
    let mut scripts = ScriptHost::new(
        scene.clone(),
        PhysicsFacade::wrap(physics.clone()),
        Rc::new(RefCell::new(InputState::default())),
    );
    // 不给 `meshes`：装配需要的形状与尺寸全都拿不到。
    let mut api = api(&scene, &physics, &mut scripts);

    let failure = api.physics_bind().expect_err("没有网格来源，绑定不该成功");
    assert_eq!(
        failure.code, MESH_SOURCE_UNAVAILABLE,
        "错误码不对：{failure:?}"
    );
    assert!(
        failure.message.contains("网格来源"),
        "消息要点名缺的是网格来源：{}",
        failure.message
    );
    assert!(
        api.physics.borrow().body_count() == 0,
        "报错路径不许按 scale * 0.5 猜一个形状装进去"
    );
}
