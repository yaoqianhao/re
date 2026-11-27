//! prefab 宿主面的两条判据（M2-E04 判据 ⑦⑧）。
//!
//! 这里测的是**宿主层**，不是协议层：`r3e-protocol/tests/prefab_rpc.rs` 已经验过
//! 「请求被路由到 `PrefabApi`」，但路由到了不等于宿主侧语义正确。本文件补两件事：
//!
//! * ⑦ `prefab_define` 走**本地路径调用**（它没有 rpc，`P5` = B）：合法模板文件
//!   登记成功、重复登记报 `replaced`、非法路径回 `INVALID_PARAMS`。
//! * ⑧ `prefab_spawn` 引用**未登记**模板时回 `PREFAB_REFERENCE_MISSING`（不是
//!   `INVALID_PARAMS`，也不许 `panic`）；先 define 再 spawn 才通，且回包里的覆盖表
//!   是空对象——「刚造的实例是干净的」要能自证。
//!
//! 用真宿主 `LocalEngineApi` 而不是给 `PrefabApi` 写个假实现：宿主层最容易出错的
//! 地方是 `Document::write` 的借用与「写进 state_hash」的口径，假实现把这些全绕过了，
//! 测了个寂寞。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_audio::Audio;
use r3e_core::{InputState, Scene};
use r3e_doc::Document;
use r3e_editor::{PrefabApi, PrefabSpawnRequest, INVALID_PARAMS, PREFAB_REFERENCE_MISSING};
use r3e_editor_host::api::prefab_define;
use r3e_editor_host::LocalEngineApi;
use r3e_physics::{Physics, PhysicsFacade};
use r3e_runtime::time::FrameTimeStats;
use r3e_script::ScriptHost;

/// 仓库里真实存在的模板样例（`M2_E_PLAN` §8.2 判据 ③ 用的同一份）。
const CRATE_TEMPLATE: &str = "../../assets/prefab/prop/crate.json";

fn asset(rel: &str) -> String {
    // `cargo test` 的 CWD 是 crate 根，用 `CARGO_MANIFEST_DIR` 拼绝对路径，
    // 免得换工作目录跑就找不到资产。
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

/// 宿主三件套 + 脚本宿主（headless：无渲染器 / 无导航槽位 / 无角色）。
struct Harness {
    scene: Rc<RefCell<Document>>,
    physics: Rc<RefCell<Physics>>,
    audio: Rc<RefCell<Audio>>,
    scripts: ScriptHost,
}

impl Harness {
    fn new() -> Self {
        let scene = Rc::new(RefCell::new(Document::new(Scene::new())));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let scripts = ScriptHost::new(
            scene.clone(),
            PhysicsFacade::wrap(physics.clone()),
            Rc::new(RefCell::new(InputState::default())),
        );
        Self {
            scene,
            physics,
            audio: Rc::new(RefCell::new(Audio::silent("prefab 宿主单测不接音频设备"))),
            scripts,
        }
    }

    fn api(&mut self) -> LocalEngineApi<'_> {
        LocalEngineApi {
            scene: &self.scene,
            physics: &self.physics,
            audio: &self.audio,
            nav: None,
            character: None,
            scripts: &mut self.scripts,
            meshes: None,
            renderer: None,
            frame_ms: FrameTimeStats {
                samples: 2,
                last_ms: 16.0,
                p50_ms: 16.0,
                p95_ms: 18.0,
                p99_ms: 20.0,
                max_ms: 20.0,
            },
            capture_dir: std::path::PathBuf::from("assets/captures"),
            scene_label: "assets/scene.json".to_string(),
            script_label: "scripts/main.rhai".to_string(),
            frames: 0,
            fps: 60.0,
            uptime_ms: 0,
            paused: false,
        }
    }

    fn node_count(&self) -> usize {
        self.scene.borrow().len()
    }

    /// 实例表里有没有这条实例（判「失败不留残留」用）。
    fn has_instance(&self, name: &str) -> bool {
        self.scene.borrow().instance(name).is_some()
    }
}

/// 判据 ⑦：`prefab_define` 的本地调用语义（成功 / 重复登记 / 非法路径）。
#[test]
fn prefab_define_is_local_only_and_reports_semantics() {
    // 它没有 rpc：`RPC_METHODS` 里不该出现 `prefab.define`（`P5` = B）。
    assert!(
        !r3e_editor::RPC_METHODS.contains(&"prefab.define"),
        "prefab_define 无 rpc，不该登记进 RPC_METHODS"
    );

    let mut harness = Harness::new();
    // 借用 `api` 期间不能再借 `harness`，所以先拎出一份场景句柄供断言用。
    let scene = harness.scene.clone();
    let mut api = harness.api();

    // ① 合法模板文件：登记成功，回包自证 id / 路径 / proto。
    let value = prefab_define(&mut api, "prop.crate", &asset(CRATE_TEMPLATE))
        .expect("合法模板文件应登记成功");
    assert_eq!(value["id"], "prop.crate", "回包应回显模板 ID");
    assert_eq!(value["replaced"], false, "首次登记不该报 replaced");
    assert_eq!(
        value["path"].as_str().map(|p| p.ends_with("crate.json")),
        Some(true),
        "回包应带实际读到的模板路径: {value}"
    );
    assert!(
        value["proto"].as_str().is_some_and(|p| !p.is_empty()),
        "回包应带模板原型名（proto）: {value}"
    );

    // ② 同一 ID 再登记 = 覆盖：replaced 翻真，模板表不新增条目。
    let again = prefab_define(&mut api, "prop.crate", &asset(CRATE_TEMPLATE))
        .expect("重复登记应成功（覆盖语义）");
    assert_eq!(again["replaced"], true, "第二次登记应报 replaced = true");
    assert!(
        scene.borrow().prefab_handle("prop.crate").is_some(),
        "覆盖登记后模板应仍在表里"
    );

    // ③ 非法路径（文件不存在）→ `INVALID_PARAMS`：这是参数错，不是「引用缺失」，
    //    调用方该去修路径，而不是先去建模板。
    let err = prefab_define(
        &mut api,
        "prop.missing",
        &asset("../../assets/prefab/prop/nope.json"),
    )
    .expect_err("不存在的模板文件应报错");
    assert_eq!(
        err.code, INVALID_PARAMS,
        "非法路径应是 INVALID_PARAMS，收到 {} / {}",
        err.code, err.message
    );
    assert!(
        scene.borrow().prefab_handle("prop.missing").is_none(),
        "失败的登记不该留下半条模板"
    );
}

/// 判据 ⑧：`prefab_spawn` 引用未登记模板 → `PREFAB_REFERENCE_MISSING`，不 panic。
#[test]
fn prefab_spawn_surfaces_missing_reference_without_panicking() {
    let mut harness = Harness::new();
    let before_nodes = harness.node_count();

    {
        // 引用一个没登记过的模板：必须是可捕获的 Err，而不是 panic / 空场景假装成功。
        let mut api = harness.api();
        let err = api
            .prefab_spawn(PrefabSpawnRequest {
                id: "prop.crate".to_string(),
                name: "crate_1".to_string(),
                parent: None,
            })
            .expect_err("未登记模板应报错");
        assert_eq!(
            err.code, PREFAB_REFERENCE_MISSING,
            "未登记模板应是 PREFAB_REFERENCE_MISSING（不是 INVALID_PARAMS）：{} / {}",
            err.code, err.message
        );
    }

    // 失败路径不留残留：节点表与实例表都没动。
    assert_eq!(harness.node_count(), before_nodes, "失败不该留下节点");
    assert!(!harness.has_instance("crate_1"), "失败不该留下半条实例记录");

    // 先登记再来一次就通：回包里的 `overrides` 是空对象（干净实例可自证）。
    {
        let mut api = harness.api();
        prefab_define(&mut api, "prop.crate", &asset(CRATE_TEMPLATE)).expect("模板登记应成功");
        let spawned = api
            .prefab_spawn(PrefabSpawnRequest {
                id: "prop.crate".to_string(),
                name: "crate_1".to_string(),
                parent: None,
            })
            .expect("登记后实例化应成功");
        assert_eq!(spawned["name"], "crate_1", "回包应回实例名: {spawned}");
        assert_eq!(spawned["template"], "prop.crate", "回包应回模板 ID");
        assert_eq!(
            spawned["overrides"],
            serde_json::json!({}),
            "刚实例化的覆盖表应为空对象: {spawned}"
        );
        assert!(
            spawned["nodeId"].as_u64().is_some(),
            "回包应带 nodeId: {spawned}"
        );
    }

    assert!(harness.has_instance("crate_1"), "应留下一条实例记录");
    assert_eq!(
        harness.node_count(),
        before_nodes + 1,
        "应恰好新增一个实例节点"
    );
}
