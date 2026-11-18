//! 宿主层（`LocalEngineApi`）RPC 测试。
//!
//! 拆分说明：原 `api.rs` 内单个 `mod tests`（1696 行）按主题切成 `core` /
//! `ui_light` / `anim_audio` 三个子模块，公共 fixture 与构造辅助函数留在本文件。

use super::support::*;

use super::*;
use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::light::{MAX_LOCAL_SHADOW_BIAS, MIN_LIGHT_RANGE};
use r3e_core::{AnimClip, AnimKey, AnimTrack, InputState, MeshProvider};
use r3e_doc::Document;
use r3e_physics::{BuiltinBackend, Physics};
use r3e_runtime::time::FrameTimeWindow;
use std::sync::Arc;

type Fixture = (Rc<RefCell<Document>>, Rc<RefCell<Physics>>, ScriptHost);

/// 开一笔事务并取回 txId（`tx.*` 各主题的测试都要用，故提到公共层）。
fn begin(api: &mut LocalEngineApi<'_>) -> String {
    let value = api.tx_begin().expect("开事务不该失败");
    value["txId"].as_str().expect("回包应带 txId").to_string()
}

/// `scene.spawn_prefab` 的 `args`：`{ "nodes": [...] }`。
fn nodes(items: Value) -> Value {
    json!({ "nodes": items })
}

/// 一份「物理开着、后端申报 shape_cast / overlap」的世界（与 `tests/verify_layers.rs`
/// 同一口径：内置后端零外部依赖，查询是解析法真实可查）。
fn physics_on() -> Physics {
    Physics::with_backend(Box::new(BuiltinBackend::new(
        glam::Vec3::new(0.0, -9.81, 0.0),
        0.0,
    )))
}

/// `fixture` 的那份场景，但不带脚本宿主：物理层复核要自己决定世界开不开。
fn fixture_physics_on() -> Rc<RefCell<Document>> {
    scene_doc()
}

fn fixture() -> Fixture {
    let scene = scene_doc();
    let physics = Rc::new(RefCell::new(Physics::new()));
    let scripts = ScriptHost::new(
        scene.clone(),
        physics.clone(),
        Rc::new(RefCell::new(InputState::default())),
    );
    (scene, physics, scripts)
}

/// 三个重叠在原点的 cube：静态层基线本来就不干净，正好用来验「只报新增」。
fn scene_doc() -> Rc<RefCell<Document>> {
    let mut scene = Scene::new();
    scene.name = "editor-test".to_string();
    for name in ["cube", "sphere", "cube_2"] {
        scene
            .spawn(ObjectDesc {
                name: name.to_string(),
                mesh: Some("cube".to_string()),
                ..Default::default()
            })
            .expect("测试场景 spawn 不应失败");
    }
    Rc::new(RefCell::new(Document::new(scene)))
}

fn api<'a>(
    scene: &'a Rc<RefCell<Document>>,
    physics: &'a Rc<RefCell<Physics>>,
    scripts: &'a mut ScriptHost,
) -> LocalEngineApi<'a> {
    api_with_nav(scene, physics, scripts, None)
}

/// 带导航槽位的构造（M1-S4-D）：`nav.bake` 要往槽位里写，槽位必须由测试持有，
/// 才能验「烘完就能查、且场景哈希没变」。
fn api_with_nav<'a>(
    scene: &'a Rc<RefCell<Document>>,
    physics: &'a Rc<RefCell<Physics>>,
    scripts: &'a mut ScriptHost,
    nav: Option<&'a RefCell<Option<Nav>>>,
) -> LocalEngineApi<'a> {
    LocalEngineApi {
        scene,
        physics,
        nav,
        scripts,
        // 单元测试没有资产库：物理复核会因此退回「只出解析结论 + 记 unchecked」，
        // 这正是 headless 下该有的诚实结论（M1-S4-G）。
        meshes: None,
        // 单元测试没有窗口 / GPU：渲染器缺席正是「headless 也得给出可解释结果」的场景。
        renderer: None,
        frame_ms: FrameTimeStats {
            samples: 4,
            last_ms: 16.0,
            p50_ms: 16.0,
            p95_ms: 18.5,
            p99_ms: 20.0,
            max_ms: 20.0,
        },
        capture_dir: PathBuf::from("assets/captures"),
        scene_label: "assets/scene.json".to_string(),
        script_label: "scripts/main.rhai".to_string(),
        frames: 42,
        fps: 60.0,
        uptime_ms: 700,
        paused: false,
    }
}

/// 带网格来源的构造（M1-S4-G）：生产侧由 `EngineAccess::meshes` 递进运行时那份
/// 「资产优先、图元兜底」的 provider，这里用内置图元 provider 顶上同一接口。
fn api_with_meshes<'a>(
    scene: &'a Rc<RefCell<Document>>,
    physics: &'a Rc<RefCell<Physics>>,
    scripts: &'a mut ScriptHost,
    meshes: Arc<dyn MeshProvider>,
) -> LocalEngineApi<'a> {
    let mut api = api(scene, physics, scripts);
    api.meshes = Some(meshes);
    api
}

/// 造一个带混合树的场景：`hero/walk` 播 walk，`hero/atk` 供层叠加。
fn anim_fixture() -> Fixture {
    let (scene, physics, scripts) = fixture();
    {
        let atk = Arc::new(anim_clip("atk", 100.0));
        scene.borrow_mut().write(|scene_ref| {
            let cube = scene_ref.find_id("cube").expect("cube 应存在");
            let walk = Arc::new(anim_clip("walk", 10.0));
            let mut inst = AnimInstance::new("hero/walk", walk);
            inst.bindings = vec![AnimBinding {
                node: cube,
                node_index: 0,
                path: "cube".to_string(),
                track: 0,
            }];
            scene_ref.add_anim_instance(inst);
            scene_ref.register_anim_clip("hero/atk", atk);
        });
    }
    (scene, physics, scripts)
}

fn anim_clip(name: &str, end_x: f32) -> AnimClip {
    AnimClip {
        name: name.to_string(),
        duration: 1.0,
        tracks: vec![AnimTrack {
            node_path: "cube".to_string(),
            node_index: 0,
            keys: vec![anim_key(0.0, 0.0), anim_key(1.0, end_x)],
        }],
    }
}

fn anim_key(time: f32, x: f32) -> AnimKey {
    AnimKey {
        time,
        position: Vec3::new(x, 0.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    }
}

// ---- 3D 空间化音频（§12 第十项）宿主层 ----

fn spatial_patch(key: &str) -> AudioSpatialPatch {
    AudioSpatialPatch {
        key: key.to_string(),
        ..Default::default()
    }
}

fn env_map_patch(equirect: Option<&str>, faces: Option<Vec<String>>) -> EnvironmentMapPatch {
    EnvironmentMapPatch {
        equirect: equirect.map(str::to_string),
        faces,
        ..Default::default()
    }
}

mod anim_audio;
mod core;
mod physics_bind;
mod physics_nav;
mod query;
mod route;
mod tx;
mod tx_violations;
mod ui_light;
mod verify;
