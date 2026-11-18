//! 宿主层（`LocalEngineApi`）RPC 测试。
//!
//! 拆分说明：原 `api.rs` 内单个 `mod tests`（1696 行）按主题切成 `core` /
//! `ui_light` / `anim_audio` 三个子模块，公共 fixture 与构造辅助函数留在本文件。

use super::support::*;

use super::*;
use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::light::{MAX_LOCAL_SHADOW_BIAS, MIN_LIGHT_RANGE};
use r3e_core::{AnimClip, AnimKey, AnimTrack, InputState};
use r3e_doc::Document;
use r3e_runtime::time::FrameTimeWindow;
use std::sync::Arc;

type Fixture = (Rc<RefCell<Document>>, Rc<RefCell<Physics>>, ScriptHost);

fn fixture() -> Fixture {
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
    let scene = Rc::new(RefCell::new(Document::new(scene)));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let scripts = ScriptHost::new(
        scene.clone(),
        physics.clone(),
        Rc::new(RefCell::new(InputState::default())),
    );
    (scene, physics, scripts)
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
mod physics_nav;
mod query;
mod route;
mod tx;
mod ui_light;
mod verify;
