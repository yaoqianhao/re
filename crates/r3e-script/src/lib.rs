//! r3e-script —— 用 Rhai 脚本实时操控场景。
//!
//! 设计要点：
//! * 场景是唯一真源，脚本每次修改都直接写进 `Scene`，渲染下一帧即可见。
//! * 脚本支持热重载：文件 mtime 变化即重新载入并重放基础场景。
//! * 物理通过 `r3e_physics::Physics` 门面暴露，当前为 `null` 后端（空实现）。

pub mod api;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use r3e_core::{InputState, Scene};
use r3e_physics::{Physics, PhysicsConfig};
use rhai::{Array, Dynamic, Engine, Scope, AST};

pub use api::Clock;

/// 脚本宿主：持有 Rhai 引擎、编译后的 AST 与持久作用域。
/// 场景重建后的补挂回调（P3）：宿主用它把资产节点与动画实例重新 spawn 回场景。
type RebuildHook = Box<dyn FnMut(&mut Scene)>;

pub struct ScriptHost {
    engine: Engine,
    clock: Rc<RefCell<Clock>>,
    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    input: Rc<RefCell<InputState>>,
    ast: Option<AST>,
    scope: Scope<'static>,
    script_path: Option<PathBuf>,
    script_mtime: Option<SystemTime>,
    base_scene_json: Option<String>,
    /// 场景重建后的补挂回调（P3）：宿主用它把资产节点与动画实例重新 spawn 回场景。
    rebuild_hook: Option<RebuildHook>,
    /// 脚本运行期错误，供 HUD / 控制台观察。
    pub last_error: Option<String>,
}

impl ScriptHost {
    pub fn new(
        scene: Rc<RefCell<Scene>>,
        physics: Rc<RefCell<Physics>>,
        input: Rc<RefCell<InputState>>,
    ) -> Self {
        let clock = Rc::new(RefCell::new(Clock::default()));
        let ctx = api::ScriptContext {
            scene: scene.clone(),
            physics: physics.clone(),
            input: input.clone(),
            clock: clock.clone(),
            rng: Rc::new(std::cell::Cell::new(0x2545F4914F6CDD1D)),
        };

        let mut engine = Engine::new();
        engine.on_print(|s| println!("[script] {s}"));
        api::bind_all(&mut engine, &ctx);

        Self {
            engine,
            clock,
            scene,
            physics,
            input,
            ast: None,
            scope: Scope::new(),
            script_path: None,
            script_mtime: None,
            base_scene_json: None,
            rebuild_hook: None,
            last_error: None,
        }
    }

    /// 注册场景重建回调：每次场景被重放（热重载 / F5）都会在重跑脚本前调用，
    /// 宿主借此把资产节点与动画实例补挂回去，避免热重载后资产凭空消失。
    pub fn set_rebuild_hook(&mut self, hook: impl FnMut(&mut Scene) + 'static) {
        self.rebuild_hook = Some(Box::new(hook));
    }

    /// 跑一遍重建回调并刷新世界变换；无回调时只刷新变换。
    fn run_rebuild_hook(&mut self) {
        let mut scene = self.scene.borrow_mut();
        if let Some(hook) = self.rebuild_hook.as_mut() {
            hook(&mut scene);
        }
        scene.update_world_transforms();
    }

    /// 输入状态句柄，供 app 层每帧填充后脚本读取。
    pub fn input_handle(&self) -> Rc<RefCell<InputState>> {
        self.input.clone()
    }

    pub fn scene_handle(&self) -> Rc<RefCell<Scene>> {
        self.scene.clone()
    }

    /// 记录基础场景 JSON，供热重载时重放。
    pub fn set_base_scene(&mut self, json: impl Into<String>) {
        self.base_scene_json = Some(json.into());
    }

    /// 编译并执行一段脚本源码。
    pub fn load_str(&mut self, src: &str, name: &str) -> Result<(), String> {
        // Windows 编辑器常给 UTF-8 加 BOM，Rhai 会把它当成非法首字符（Unexpected '\u{feff}'），
        // 这里统一剥掉，避免用户复制粘贴脚本后莫名其妙编译失败。
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let ast = self
            .engine
            .compile(src)
            .map_err(|e| format!("脚本 `{name}` 编译失败: {e}"))?;

        self.scope.clear();
        self.engine
            .run_ast_with_scope(&mut self.scope, &ast)
            .map_err(|e| format!("脚本 `{name}` 执行失败: {e}"))?;

        // 约定：逻辑一律写在 on_start / on_update 里。
        // Rhai 的 call_fn 会连同顶层语句一起执行，顶层语句因此会随每次 on_update 重复运行。
        self.ast = Some(ast.clone());
        if has_fn(&ast, "on_start", 0) {
            if let Err(e) = self
                .engine
                .call_fn::<Dynamic>(&mut self.scope, &ast, "on_start", ())
            {
                self.last_error = Some(format!("on_start 出错: {e}"));
            }
        }
        self.last_error = None;
        Ok(())
    }

    /// 载入脚本文件并记录 mtime。
    pub fn load_file(&mut self, path: &Path) -> Result<(), String> {
        let src = std::fs::read_to_string(path)
            .map_err(|e| format!("读取脚本 {} 失败: {e}", path.display()))?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "script".to_string());
        self.script_mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        self.script_path = Some(path.to_path_buf());
        println!("[script] 载入 {name}（{} 字节）", src.len());
        self.load_str(&src, &name)
    }

    pub fn script_path(&self) -> Option<&Path> {
        self.script_path.as_deref()
    }

    /// 每帧调用脚本的 `on_update(dt)`。
    pub fn tick(&mut self, dt: f64, time: f64, frame: u64) {
        {
            let mut c = self.clock.borrow_mut();
            c.dt = dt;
            c.time = time;
            c.frame = frame;
        }

        let Some(ast) = self.ast.clone() else { return };
        if !has_fn(&ast, "on_update", 1) {
            return;
        }
        if let Err(e) = self
            .engine
            .call_fn::<Dynamic>(&mut self.scope, &ast, "on_update", (dt,))
        {
            let msg = format!("on_update 出错: {e}");
            if self.last_error.as_deref() != Some(msg.as_str()) {
                println!("[script] {msg}");
            }
            self.last_error = Some(msg);
        }
    }

    /// 触发一次立即执行的脚本片段（控制台 / 调试用）。
    pub fn eval(&mut self, src: &str) -> Result<Dynamic, String> {
        self.engine
            .eval_with_scope::<Dynamic>(&mut self.scope, src)
            .map_err(|e| e.to_string())
    }

    /// 用新的场景 JSON 重建世界，并重跑当前脚本（IDE 里改 scene.json 也会生效）。
    pub fn rebuild_scene(&mut self, json: &str) -> Result<(), String> {
        let new_scene = Scene::from_json_str(json).map_err(|e| format!("场景解析失败: {e}"))?;
        let phys_cfg = PhysicsConfig::from_scene(&new_scene.physics);
        *self.scene.borrow_mut() = new_scene;
        {
            let mut physics = self.physics.borrow_mut();
            physics.clear_bodies();
            if let Err(e) = physics.apply_config(phys_cfg) {
                println!("[script] 物理配置应用失败: {e}");
            }
        }
        self.base_scene_json = Some(json.to_string());
        self.run_rebuild_hook();

        if let Some(ast) = self.ast.clone() {
            self.scope.clear();
            self.engine
                .run_ast_with_scope(&mut self.scope, &ast)
                .map_err(|e| format!("脚本重放失败: {e}"))?;
            if has_fn(&ast, "on_start", 0) {
                if let Err(e) =
                    self.engine
                        .call_fn::<Dynamic>(&mut self.scope, &ast, "on_start", ())
                {
                    self.last_error = Some(format!("on_start 出错: {e}"));
                }
            }
        }
        Ok(())
    }

    /// 检查脚本文件是否变化；返回 true 表示已重载（场景已被重放）。
    pub fn reload_if_changed(&mut self) -> bool {
        let Some(path) = self.script_path.clone() else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return false;
        };
        let Ok(mtime) = meta.modified() else {
            return false;
        };
        if Some(mtime) == self.script_mtime {
            return false;
        }

        // 重放基础场景，保证脚本重新执行时状态干净
        let mut phys_cfg = None;
        if let Some(json) = &self.base_scene_json {
            match Scene::from_json_str(json) {
                Ok(new_scene) => {
                    phys_cfg = Some(PhysicsConfig::from_scene(&new_scene.physics));
                    *self.scene.borrow_mut() = new_scene;
                }
                Err(e) => println!("[script] 基础场景重放失败: {e}"),
            }
        } else {
            self.scene.borrow_mut().clear_nodes();
        }
        {
            let mut physics = self.physics.borrow_mut();
            physics.clear_bodies();
            // 回到场景文件声明的物理配置（脚本里的运行时改动随之重置）
            if let Some(cfg) = phys_cfg {
                if let Err(e) = physics.apply_config(cfg) {
                    println!("[script] 物理配置应用失败: {e}");
                }
            }
        }

        // 资产节点/动画实例必须重新挂回重放后的场景，且要在脚本 on_start 之前
        // （否则热重载后模型消失、on_start 里 anims() 为空）
        self.run_rebuild_hook();

        if let Err(e) = self.load_file(&path) {
            println!("[script] 热重载失败: {e}");
            return false;
        }
        println!("[script] 热重载完成");
        true
    }
}

/// AST 中是否存在指定名字与参数个数的函数。
fn has_fn(ast: &AST, name: &str, arity: usize) -> bool {
    ast.iter_functions()
        .any(|f| f.name == name && f.params.len() == arity)
}

/// 便捷：把脚本里 `names()` 返回的数组转成字符串列表。
pub fn array_to_strings(arr: Array) -> Vec<String> {
    arr.into_iter()
        .filter_map(|d| d.into_string().ok())
        .collect()
}
#[cfg(test)]
mod key_script_tests {
    use super::*;
    use r3e_core::KeyCode;

    /// 空场景 + 可外部驱动的输入句柄。
    fn host_with_input() -> (ScriptHost, Rc<RefCell<InputState>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene, physics, input.clone()), input)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    /// 脚本侧三态：按下 → 按住（边沿消失）→ 抬起。
    #[test]
    fn script_keyboard_three_states() {
        let (mut host, input) = host_with_input();
        assert!(!eval_bool(&mut host, "key_down(\"w\")"));

        input.borrow_mut().keyboard.set(KeyCode::W, true);
        assert!(eval_bool(&mut host, "key_down(\"w\")"), "按下后应持续按住");
        assert!(eval_bool(&mut host, "key_down(\"W\")"), "键名不区分大小写");
        assert!(eval_bool(&mut host, "key_pressed(\"w\")"));
        assert!(!eval_bool(&mut host, "key_released(\"w\")"));

        // 帧末清边沿：按住仍在，边沿不再触发
        input.borrow_mut().end_frame();
        assert!(eval_bool(&mut host, "key_down(\"w\")"));
        assert!(
            !eval_bool(&mut host, "key_pressed(\"w\")"),
            "边沿只在本帧为真"
        );

        input.borrow_mut().keyboard.set(KeyCode::W, false);
        assert!(!eval_bool(&mut host, "key_down(\"w\")"));
        assert!(eval_bool(&mut host, "key_released(\"w\")"));
    }

    /// 任意键与按住列表：脚本做「按任意键开始」和 HUD 时用。
    #[test]
    fn script_lists_held_keys() {
        let (mut host, input) = host_with_input();
        assert!(!eval_bool(&mut host, "key_any()"));
        assert!(!eval_bool(&mut host, "key_any_pressed()"));

        {
            let mut i = input.borrow_mut();
            i.keyboard.set(KeyCode::A, true);
            i.keyboard.set(KeyCode::Space, true);
        }
        assert!(eval_bool(&mut host, "key_any()"));
        assert!(eval_bool(&mut host, "key_any_pressed()"));

        let count = host
            .eval("key_held_count()")
            .expect("key_held_count 应可调用")
            .as_int()
            .expect("应返回整数");
        assert_eq!(count, 2);

        let held = host
            .eval("keys_held()")
            .expect("keys_held 应可调用")
            .into_array()
            .expect("应返回数组");
        assert_eq!(held.len(), 2);
        assert_eq!(
            held[0].clone().into_string().expect("元素应为字符串"),
            "a",
            "按住列表按枚举顺序返回"
        );
    }

    /// 未知键名不 panic，返回 false；索引形式可用。
    #[test]
    fn script_unknown_key_and_index_form() {
        let (mut host, input) = host_with_input();
        assert!(!eval_bool(&mut host, "key_down(\"不存在的键\")"));
        assert!(!eval_bool(&mut host, "key_pressed(\"\")"));

        input.borrow_mut().keyboard.set(KeyCode::F5, true);
        let idx = KeyCode::F5.index() as i64;
        assert!(eval_bool(&mut host, &format!("key_down({idx})")));
        assert!(
            !eval_bool(&mut host, "key_down(9999)"),
            "越界索引应为 false"
        );
    }

    /// 失焦清空后，脚本读到的键盘状态必须全为 false（含边沿）。
    #[test]
    fn script_sees_no_keys_after_focus_loss() {
        let (mut host, input) = host_with_input();
        input.borrow_mut().keyboard.set(KeyCode::W, true);
        assert!(eval_bool(&mut host, "key_down(\"w\")"));

        input.borrow_mut().reset_held();
        assert!(!eval_bool(&mut host, "key_down(\"w\")"));
        assert!(!eval_bool(&mut host, "key_pressed(\"w\")"));
        assert!(
            !eval_bool(&mut host, "key_released(\"w\")"),
            "失焦不应伪造抬起边沿"
        );
        assert!(!eval_bool(&mut host, "key_any()"));
    }
}

