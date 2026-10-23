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

use r3e_core::Scene;
use r3e_physics::Physics;
use rhai::{Array, Dynamic, Engine, Scope, AST};

pub use api::Clock;

/// 脚本宿主：持有 Rhai 引擎、编译后的 AST 与持久作用域。
pub struct ScriptHost {
    engine: Engine,
    clock: Rc<RefCell<Clock>>,
    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    ast: Option<AST>,
    scope: Scope<'static>,
    script_path: Option<PathBuf>,
    script_mtime: Option<SystemTime>,
    base_scene_json: Option<String>,
    /// 脚本运行期错误，供 HUD / 控制台观察。
    pub last_error: Option<String>,
}

impl ScriptHost {
    pub fn new(scene: Rc<RefCell<Scene>>, physics: Rc<RefCell<Physics>>) -> Self {
        let clock = Rc::new(RefCell::new(Clock::default()));
        let ctx = api::ScriptContext {
            scene: scene.clone(),
            physics: physics.clone(),
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
            ast: None,
            scope: Scope::new(),
            script_path: None,
            script_mtime: None,
            base_scene_json: None,
            last_error: None,
        }
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
            if let Err(e) = self.engine.call_fn::<Dynamic>(&mut self.scope, &ast, "on_start", ()) {
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
        if let Err(e) = self.engine.call_fn::<Dynamic>(&mut self.scope, &ast, "on_update", (dt,)) {
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
        let new_scene =
            Scene::from_json_str(json).map_err(|e| format!("场景解析失败: {e}"))?;
        *self.scene.borrow_mut() = new_scene;
        self.physics.borrow_mut().clear_bodies();
        self.base_scene_json = Some(json.to_string());

        if let Some(ast) = self.ast.clone() {
            self.scope.clear();
            self.engine
                .run_ast_with_scope(&mut self.scope, &ast)
                .map_err(|e| format!("脚本重放失败: {e}"))?;
            if has_fn(&ast, "on_start", 0) {
                if let Err(e) = self.engine.call_fn::<Dynamic>(&mut self.scope, &ast, "on_start", ()) {
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
        if let Some(json) = &self.base_scene_json {
            match Scene::from_json_str(json) {
                Ok(new_scene) => *self.scene.borrow_mut() = new_scene,
                Err(e) => println!("[script] 基础场景重放失败: {e}"),
            }
        } else {
            self.scene.borrow_mut().clear_nodes();
        }
        self.physics.borrow_mut().clear_bodies();

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
        .any(|f| f.name.to_string() == name && f.params.len() == arity)
}

/// 便捷：把脚本里 `names()` 返回的数组转成字符串列表。
pub fn array_to_strings(arr: Array) -> Vec<String> {
    arr.into_iter()
        .filter_map(|d| d.into_string().ok())
        .collect()
}
