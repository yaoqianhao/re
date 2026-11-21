//! `character.*` 六条 RPC 的宿主层验收（M2-B01）。
//!
//! 这一层要守住的是**边界**，不是求解精度（精度归 `r3e-character` 的 solver_spec）：
//!
//! * 配置进场景（可撤销、进 `state_hash`），运行态**一格都不进**；
//! * `step` / `teleport` 不回写 `Transform`，回包用 `sceneSynced: false` 说明白；
//! * 没接控制器（headless）与「控制器不存在」是两种码，不能混成一句「查不到」。

use super::*;
use r3e_character::CharacterRuntime;
use r3e_core::CharacterSpec;
use r3e_editor::character::CharacterBodyParams;
use r3e_editor::{CHARACTER_NOT_READY, CHARACTER_SPEC_INVALID, INVALID_PARAMS};

/// 一个真正接了角色控制器的宿主装置。
///
/// 物理用「内置后端」而不是 `Physics::new()`：`character.step` 要问
/// `PhysicsQueryPort` 要地面，空世界会让每一次 `step` 都只是自由落体，
/// 验不出「查询确实被调到了」。
struct Rig {
    scene: Rc<RefCell<Document>>,
    physics: Rc<RefCell<Physics>>,
    character: Rc<dyn CharacterHost>,
    scripts: ScriptHost,
}

impl Rig {
    fn new() -> Self {
        let scene = scene_doc();
        let physics = Rc::new(RefCell::new(physics_on()));
        let scripts = ScriptHost::new(
            scene.clone(),
            PhysicsFacade::wrap(physics.clone()),
            Rc::new(RefCell::new(InputState::default())),
        );
        let character = CharacterRuntime::wrap(physics.clone());
        Self {
            scene,
            physics,
            character,
            scripts,
        }
    }

    fn api(&mut self) -> LocalEngineApi<'_> {
        let Self {
            scene,
            physics,
            character,
            scripts,
        } = self;
        api_with_character(scene, physics, scripts, character)
    }

    /// 场景里某个节点的角色配置（`None` = 还不是角色）。
    fn spec_of(&self, name: &str) -> Option<CharacterSpec> {
        self.scene.borrow().find(name).and_then(|n| n.character)
    }
}

fn bind(node: &str) -> CharacterBindQuery {
    CharacterBindQuery {
        node: node.to_string(),
        body: CharacterBodyParams::default(),
    }
}

fn bind_radius(node: &str, radius: f32) -> CharacterBindQuery {
    CharacterBindQuery {
        node: node.to_string(),
        body: CharacterBodyParams {
            radius: Some(radius),
            ..Default::default()
        },
    }
}

/// `create` 的双向落地：配置确实进了场景（可查、可撤销的前提），控制器也确实建了。
#[test]
fn create_writes_scene_and_binds() {
    let mut rig = Rig::new();
    let value = rig
        .api()
        .character_create(CharacterBindQuery {
            node: "cube".to_string(),
            body: CharacterBodyParams {
                height: Some(1.8),
                ..Default::default()
            },
        })
        .expect("节点存在，create 不该失败");

    assert_eq!(value["created"], json!(true), "首次 create 应报 created");
    assert_eq!(value["updated"], json!(false));
    let spec = rig.spec_of("cube").expect("配置必须落进 Node::character");
    assert_eq!(spec.height, 1.8);
    assert_eq!(
        spec.radius,
        CharacterSpec::default().radius,
        "没给的字段该取缺省值"
    );

    // 控制器建起来了：state 能读到，且起步位置是节点的世界坐标（原点）。
    let st = rig
        .api()
        .character_state(CharacterNameQuery {
            node: "cube".to_string(),
        })
        .expect("刚 create 过，state 不该失败");
    assert_eq!(st["position"], json!([0.0, 0.0, 0.0]));
    assert_eq!(st["sceneSynced"], json!(false), "回包要写明没写回节点");
}

/// 非法参数必须在写场景**之前**被拒：不能留下一个「半径为 0 的角色」。
#[test]
fn create_rejects_invalid_spec_without_writing() {
    let mut rig = Rig::new();
    let before = rig.scene.borrow().state_hash();
    let failure = rig
        .api()
        .character_create(bind_radius("cube", 0.0))
        .expect_err("半径 0 应被拒");
    assert_eq!(failure.code, CHARACTER_SPEC_INVALID, "{failure:?}");
    assert!(rig.spec_of("cube").is_none(), "被拒的配置不许落进场景");
    assert_eq!(
        rig.scene.borrow().state_hash(),
        before,
        "校验失败不许动场景哈希"
    );
}

/// 没有接入角色控制器（headless 宿主）时：六条一律 `CHARACTER_NOT_READY`。
#[test]
fn commands_report_not_ready_without_host() {
    let (scene, physics, mut scripts) = fixture();
    let mut api = api(&scene, &physics, &mut scripts);
    let failure = api.character_create(bind("cube")).expect_err("没接控制器");
    assert_eq!(failure.code, CHARACTER_NOT_READY, "{failure:?}");

    let failure = api
        .character_state(CharacterNameQuery {
            node: "cube".to_string(),
        })
        .expect_err("没接控制器");
    assert_eq!(failure.code, CHARACTER_NOT_READY, "{failure:?}");
    assert!(scene.borrow().find("cube").unwrap().character.is_none());
}

/// `config` 只改给了的字段：这是与脚本侧「全量覆盖」刻意不同的口径。
#[test]
fn config_only_touches_given_fields() {
    let mut rig = Rig::new();
    rig.api()
        .character_create(CharacterBindQuery {
            node: "cube".to_string(),
            body: CharacterBodyParams {
                height: Some(1.8),
                radius: Some(0.35),
                ..Default::default()
            },
        })
        .expect("create 应成功");

    let value = rig
        .api()
        .character_config(CharacterBindQuery {
            node: "cube".to_string(),
            body: CharacterBodyParams {
                snap_distance: Some(0.2),
                ..Default::default()
            },
        })
        .expect("config 应成功");
    assert_eq!(value["created"], json!(false), "config 不报 created");
    assert_eq!(value["updated"], json!(true));

    let spec = rig.spec_of("cube").expect("配置还在");
    assert_eq!(spec.height, 1.8, "没给的字段必须保持现值");
    assert_eq!(spec.radius, 0.35);
    assert_eq!(spec.snap_distance, 0.2);
}

/// 节点没有角色配置就 `step`：要报「先 create」，不是静默算一帧。
#[test]
fn step_without_config_reports_not_ready() {
    let mut rig = Rig::new();
    let before = rig.scene.borrow().state_hash();
    let failure = rig
        .api()
        .character_step(CharacterStepQuery {
            node: "cube".to_string(),
            wish: [1.0, 0.0],
            jump: false,
            dt: None,
        })
        .expect_err("没有角色配置");
    assert_eq!(failure.code, CHARACTER_NOT_READY, "{failure:?}");
    assert_eq!(rig.scene.borrow().state_hash(), before, "step 不许改场景");
}

/// `dt` 非正数必须拒：`dt = 0` 会让求解器静默不动，最难查。
#[test]
fn step_rejects_non_positive_dt() {
    let mut rig = Rig::new();
    rig.api().character_create(bind("cube")).expect("create");
    for dt in [0.0_f32, -0.016] {
        let failure = rig
            .api()
            .character_step(CharacterStepQuery {
                node: "cube".to_string(),
                wish: [0.0, 0.0],
                jump: false,
                dt: Some(dt),
            })
            .expect_err("dt 非正应被拒");
        assert_eq!(failure.code, INVALID_PARAMS, "{failure:?}");
    }
}

/// `step` 推进运行态但**不碰场景**：位置会掉（空世界里自由落体），
/// 而节点的 `Transform` 与 `state_hash` 一动不动。
#[test]
fn step_moves_only_runtime_state() {
    let mut rig = Rig::new();
    rig.api()
        .character_create(CharacterBindQuery {
            node: "cube".to_string(),
            body: CharacterBodyParams {
                height: Some(1.8),
                ..Default::default()
            },
        })
        .expect("create");
    rig.api()
        .character_teleport(CharacterTeleportQuery {
            node: "cube".to_string(),
            position: [0.0, 5.0, 0.0],
        })
        .expect("teleport");
    let before_hash = rig.scene.borrow().state_hash();
    let before_pos = rig.scene.borrow().find("cube").unwrap().transform.position;

    let st = rig
        .api()
        .character_step(CharacterStepQuery {
            node: "cube".to_string(),
            wish: [0.0, 0.0],
            jump: false,
            dt: Some(1.0 / 60.0),
        })
        .expect("已 create，step 应成功");
    let y = st["position"][1].as_f64().expect("position 是数组");
    assert!(y < 5.0, "一帧重力必须让它下去（实际 {y}）");
    assert_eq!(st["sceneSynced"], json!(false));

    assert_eq!(
        rig.scene.borrow().state_hash(),
        before_hash,
        "运行态不许进场景哈希"
    );
    assert_eq!(
        rig.scene.borrow().find("cube").unwrap().transform.position,
        before_pos,
        "step 不许回写 Transform"
    );
}

/// `teleport` 清竖直速度：否则「传送到空中」会带着上一段的下坠速度立刻再落一截。
#[test]
fn teleport_resets_vertical_speed() {
    let mut rig = Rig::new();
    rig.api().character_create(bind("cube")).expect("create");
    rig.api()
        .character_step(CharacterStepQuery {
            node: "cube".to_string(),
            wish: [0.0, 0.0],
            jump: false,
            dt: Some(1.0 / 60.0),
        })
        .expect("step");
    let st = rig
        .api()
        .character_teleport(CharacterTeleportQuery {
            node: "cube".to_string(),
            position: [1.0, 4.0, -2.0],
        })
        .expect("teleport 应成功");
    assert_eq!(st["position"], json!([1.0, 4.0, -2.0]));
    assert_eq!(st["verticalSpeed"], json!(0.0));

    let failure = rig
        .api()
        .character_teleport(CharacterTeleportQuery {
            node: "ghost".to_string(),
            position: [0.0; 3],
        })
        .expect_err("节点不存在");
    assert_ne!(failure.code, CHARACTER_NOT_READY, "该报「找不到节点」");
}

/// `stats` 必须把后端名如实带出：全 0 时「没动过」与「没接」是两件事。
#[test]
fn stats_report_backend_and_counters() {
    let mut rig = Rig::new();
    rig.api().character_create(bind("cube")).expect("create");
    rig.api()
        .character_step(CharacterStepQuery {
            node: "cube".to_string(),
            wish: [0.0, 0.0],
            jump: false,
            dt: None,
        })
        .expect("step");

    let stats = rig.api().character_stats().expect("stats 应成功");
    assert_eq!(stats["backend"], json!("kinematic"), "后端名要如实");
    assert_eq!(stats["controllers"], json!(1));
    assert_eq!(stats["steps"], json!(1));
}
