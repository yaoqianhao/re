//! 脚本层 HDR 立方体反射探针绑定的行为测试（扩展二十三）。
//!
//! `scripts_compile` 只保证演示脚本能编译；这里用 `ScriptHost::eval` 真跑 Rhai，
//! 检查四件事：
//! ① `set_env_map` / `set_env_map_faces` 会实体化 `environment_map` 段并激活；
//! ② 非法输入（空路径 / 不足六面 / NaN 强度）被 core 收敛，且不误激活；
//! ③ 关开关只停探针、保留路径；`clear_env_map` 才彻底移除；
//! ④ 只读函数在未配置时给 `none` / 1.0，不会因为探测而凭空建段。

use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::{InputState, Scene, CUBE_FACES, MAX_IBL_INTENSITY};
use r3e_physics::Physics;
use r3e_script::ScriptHost;

struct Fixture {
    scene: Rc<RefCell<Scene>>,
    host: ScriptHost,
}

fn fixture() -> Fixture {
    let scene = Rc::new(RefCell::new(Scene::new()));
    let physics = Rc::new(RefCell::new(Physics::new()));
    let input = Rc::new(RefCell::new(InputState::default()));
    let host = ScriptHost::new(scene.clone(), physics, input);
    Fixture { scene, host }
}

impl Fixture {
    fn run(&mut self, src: &str) {
        let _ = self
            .host
            .eval(src)
            .unwrap_or_else(|e| panic!("脚本执行失败: {e}\n源码: {src}"));
    }
}

#[test]
fn set_env_map_materializes_and_activates() {
    let mut f = fixture();
    assert!(f.scene.borrow().environment_map.is_none());

    f.run("set_env_map(\"assets/sky.hdr\")");
    assert!(f.scene.borrow().environment_map_active());
    assert_eq!(f.scene.borrow().env_map_source(), "equirect");
    assert_eq!(
        f.scene
            .borrow()
            .environment_map
            .as_ref()
            .unwrap()
            .equirect
            .as_deref(),
        Some("assets/sky.hdr")
    );

    // 空路径：配置留在场景里但未激活，渲染层退回解析 IBL
    f.run("set_env_map(\"   \")");
    assert!(!f.scene.borrow().environment_map_active());
    assert_eq!(f.scene.borrow().env_map_source(), "none");
}

#[test]
fn set_env_map_faces_requires_six_sides() {
    let mut f = fixture();
    let all = (0..CUBE_FACES)
        .map(|i| format!("\"f{i}.hdr\""))
        .collect::<Vec<_>>()
        .join(", ");
    f.run(&format!("set_env_map_faces([{all}])"));
    assert!(f.scene.borrow().environment_map_active());
    assert_eq!(f.scene.borrow().env_map_source(), "faces");

    // 五面：整段作废，不激活
    f.run("set_env_map_faces([\"a.hdr\", \"b.hdr\", \"c.hdr\", \"d.hdr\", \"e.hdr\"])");
    assert!(!f.scene.borrow().environment_map_active());
    assert!(f
        .scene
        .borrow()
        .environment_map
        .as_ref()
        .unwrap()
        .faces
        .is_none());
}

#[test]
fn intensity_is_clamped_and_toggle_keeps_paths() {
    let mut f = fixture();
    f.run("set_env_map(\"sky.hdr\"); set_env_map_intensity(1e9);");
    let intensity = f.scene.borrow().environment_map.as_ref().unwrap().intensity;
    assert_eq!(intensity, MAX_IBL_INTENSITY);

    // NaN 强度回落 1.0，不会污染 uniform
    f.run("set_env_map_intensity(0.0 / 0.0);");
    assert_eq!(
        f.scene.borrow().environment_map.as_ref().unwrap().intensity,
        1.0
    );

    // 关开关：路径还在，只是不生效
    f.run("set_env_map_enabled(false);");
    assert!(!f.scene.borrow().environment_map_active());
    assert_eq!(f.scene.borrow().env_map_source(), "disabled");
    assert_eq!(
        f.scene
            .borrow()
            .environment_map
            .as_ref()
            .unwrap()
            .equirect
            .as_deref(),
        Some("sky.hdr")
    );
    f.run("set_env_map_enabled(true);");
    assert!(f.scene.borrow().environment_map_active());

    // 彻底移除
    f.run("clear_env_map();");
    assert!(f.scene.borrow().environment_map.is_none());
    assert_eq!(f.scene.borrow().env_map_source(), "none");
}

#[test]
fn read_only_queries_do_not_create_section() {
    let mut f = fixture();
    // 未配置时查询：不该因为读一眼就长出 environment_map 段
    f.run("has_env_map(); env_map_source(); env_map_intensity();");
    assert!(f.scene.borrow().environment_map.is_none());

    // 返回值也是缺省口径
    let active = f.host.eval("has_env_map()").unwrap();
    assert!(!active.as_bool().unwrap());
    let source = f.host.eval("env_map_source()").unwrap();
    assert_eq!(source.into_string().unwrap(), "none");
    let intensity = f.host.eval("env_map_intensity()").unwrap();
    assert!((intensity.as_float().unwrap() - 1.0).abs() < 1e-6);
}
