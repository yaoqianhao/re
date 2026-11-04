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

use r3e_audio::Audio;
use r3e_core::{InputState, Scene, SceneCommand, SceneStack, UiInteraction};
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
    /// 音频门面。脚本层**不**自己开设备：句柄由 app 注入，或退化成静默后端（编辑器 / 单测）。
    audio: Rc<RefCell<Audio>>,
    /// UI 指针交互状态。`app` 每帧 `update`，脚本只读——同一个 `Rc`，不存在两份真源。
    ui_interaction: Rc<RefCell<UiInteraction>>,
    /// 场景栈（扩展十八 B）。与 `app` 共用同一份：脚本只读，宿主负责压栈。
    scene_stack: Rc<RefCell<SceneStack>>,
    /// 场景命令槽（扩展十八 B）：脚本申请，宿主每帧取走执行。
    scene_cmd: Rc<RefCell<Option<SceneCommand>>>,
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
        // 默认静默后端：单测 / 编辑器不需要真设备，也不该因为没声卡就报错。
        Self::new_with_audio(
            scene,
            physics,
            input,
            Rc::new(RefCell::new(Audio::silent("脚本宿主未接入音频设备"))),
        )
    }

    /// 带音频门面的构造：app 层把持有真实设备的 `Audio` 句柄交进来，脚本与 app 共用同一混音器。
    pub fn new_with_audio(
        scene: Rc<RefCell<Scene>>,
        physics: Rc<RefCell<Physics>>,
        input: Rc<RefCell<InputState>>,
        audio: Rc<RefCell<Audio>>,
    ) -> Self {
        let clock = Rc::new(RefCell::new(Clock::default()));
        let ui_interaction = Rc::new(RefCell::new(UiInteraction::new()));
        // 场景栈与命令槽在构造时就建好：它们会被 `bind_all` 的闭包捕获，
        // 之后再换 `Rc` 不会影响已注册的绑定（与 `ui_interaction` 同一约束）。
        let scene_stack = Rc::new(RefCell::new(SceneStack::new()));
        let scene_cmd: Rc<RefCell<Option<SceneCommand>>> = Rc::new(RefCell::new(None));
        let ctx = api::ScriptContext {
            scene: scene.clone(),
            physics: physics.clone(),
            input: input.clone(),
            audio: audio.clone(),
            clock: clock.clone(),
            rng: Rc::new(std::cell::Cell::new(0x2545F4914F6CDD1D)),
            ui_log: Rc::new(RefCell::new(api::UiWriteLog::default())),
            ui_interaction: ui_interaction.clone(),
            scene_stack: scene_stack.clone(),
            scene_cmd: scene_cmd.clone(),
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
            audio,
            ui_interaction,
            scene_stack,
            scene_cmd,
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

    /// 音频门面句柄，供 app 层每帧驱动 `Audio::update` 并读取错误。
    pub fn audio_handle(&self) -> Rc<RefCell<Audio>> {
        self.audio.clone()
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
                // 与 on_update 一致：首次出错就打到控制台。on_start 原先只记不打印，
                // 脚本里写错参数个数会表现成「开场什么都没发生」，排查极其难受。
                let msg = format!("on_start 出错: {e}");
                if self.last_error.as_deref() != Some(msg.as_str()) {
                    println!("[script] {msg}");
                }
                self.last_error = Some(msg);
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

    /// UI 指针交互状态的共享句柄。
    ///
    /// `app` 拿到它每帧推进（送光标位置与左键边沿），脚本侧的 `ui_events` /
    /// `ui_hover` / `ui_active` / `ui_hit` 读的就是同一份状态。
    pub fn ui_interaction(&self) -> Rc<RefCell<UiInteraction>> {
        self.ui_interaction.clone()
    }

    /// 场景栈句柄（扩展十八 B）。宿主拿它压栈 / 回退，脚本侧只读同一个栈。
    pub fn scene_stack_handle(&self) -> Rc<RefCell<SceneStack>> {
        self.scene_stack.clone()
    }

    /// 取走脚本申请的场景命令（每帧一次；没有则返回 `None`）。
    ///
    /// "取走"而不是"读取"是刻意的：命令是一次性意图，留在槽里会被下一帧重复执行。
    pub fn take_scene_command(&self) -> Option<SceneCommand> {
        self.scene_cmd.borrow_mut().take()
    }

    /// 脚本侧是否还压着一条没被执行的场景命令。
    pub fn scene_command_pending(&self) -> bool {
        self.scene_cmd.borrow().is_some()
    }

    /// 由宿主（而非脚本）登记一条场景命令；槽已满时返回 `false` 且不覆盖。
    ///
    /// 与 `take_scene_command` 共用一个槽：无论命令来自脚本还是宿主别的入口，
    /// 都走同一条"帧首执行"的路径，不会出现两套切换逻辑。
    pub fn request_scene_command(&self, cmd: SceneCommand) -> bool {
        let mut slot = self.scene_cmd.borrow_mut();
        if slot.is_some() {
            return false;
        }
        *slot = Some(cmd);
        true
    }

    /// 每帧调用脚本的 `on_update(dt)`。
    pub fn tick(&mut self, dt: f64, time: f64, frame: u64, fps: f64) {
        {
            let mut c = self.clock.borrow_mut();
            c.dt = dt;
            c.time = time;
            c.frame = frame;
            c.fps = fps;
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
                    let msg = format!("on_start 出错: {e}");
                    if self.last_error.as_deref() != Some(msg.as_str()) {
                        println!("[script] {msg}");
                    }
                    self.last_error = Some(msg);
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
mod save_script_tests {
    use super::*;
    use r3e_core::ObjectDesc;

    /// 空场景 + 物理/输入句柄的脚本宿主。
    fn host() -> (ScriptHost, Rc<RefCell<Scene>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene.clone(), physics, input), scene)
    }

    #[test]
    fn script_exports_scene_json() {
        let (mut host, scene) = host();
        scene
            .borrow_mut()
            .spawn(ObjectDesc {
                name: "probe".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");

        let json = host
            .eval("scene_json()")
            .expect("scene_json 应可调用")
            .into_string()
            .expect("应返回字符串");
        assert!(json.contains("probe"), "导出的 JSON 应含物件名: {json}");
        assert!(!json.contains("world_normal"), "不应写运行时矩阵缓存");
    }

    #[test]
    fn script_saves_scene_to_disk() {
        let (mut host, scene) = host();
        scene
            .borrow_mut()
            .spawn(ObjectDesc {
                name: "probe".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");

        let dir = std::env::temp_dir().join(format!("r3e-script-save-{}", std::process::id()));
        let path = dir.join("scene_out.json");
        // Rhai 字符串里用正斜杠，避开 Windows 路径反斜杠的转义问题
        let literal = path.display().to_string().replace('\\', "/");
        let ok = host
            .eval(&format!(r#"save_scene("{literal}")"#))
            .expect("save_scene 应可调用")
            .as_bool()
            .expect("应返回 bool");
        assert!(ok, "save_scene 应返回 true");

        let text = std::fs::read_to_string(&path).expect("文件应已落盘");
        assert!(text.contains("probe"), "落盘内容应含物件名: {text}");
        let back = Scene::from_json_str(&text).expect("落盘内容必须能回读");
        assert_eq!(back.len(), 1, "回读后物件数应一致");

        std::fs::remove_file(&path).expect("清理临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }
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

#[cfg(test)]
mod text_script_tests {
    use super::*;

    /// 空场景宿主 + 场景句柄（文本要直接查 `Scene::texts` 验证）。
    fn host() -> (ScriptHost, Rc<RefCell<Scene>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene.clone(), physics, input), scene)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    /// 每帧提交的语义：同一 key 反复 `text_add` 只覆盖，节点数恒为 1（不跨帧累积）。
    #[test]
    fn per_frame_text_add_does_not_accumulate() {
        let (mut host, scene) = host();
        for frame in 0..10 {
            let src = format!(r#"text_add("hud", "帧 {frame}", 24.0, 20.0, 16.0, 1.0, 1.0, 1.0)"#);
            let created = eval_bool(&mut host, &src);
            assert_eq!(created, frame == 0, "只有首帧是新建，之后都是覆盖");
            assert_eq!(
                scene.borrow().texts.len(),
                1,
                "第 {frame} 帧后节点数应仍为 1"
            );
        }
        assert_eq!(scene.borrow().texts[0].content, "帧 9", "内容应是最新一帧");
    }

    /// `text_set*` 对不存在的 key 返回 false，脚本据此决定要不要补 `text_add`。
    #[test]
    fn text_set_on_missing_key_reports_false() {
        let (mut host, scene) = host();
        assert!(!eval_bool(&mut host, r#"text_set("nope", "x")"#));
        assert!(!eval_bool(&mut host, r#"text_set_align("nope", "center")"#));
        assert!(
            !eval_bool(&mut host, r#"text_set_align("nope", "middle")"#),
            "未知对齐值也应返回 false"
        );
        assert!(scene.borrow().texts.is_empty(), "失败的写入不应造出节点");
    }

    /// `text_clear` 清空全部；通道开关只翻 `text_enabled`，两者互不干扰。
    #[test]
    fn text_clear_and_channel_switch() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"text_add("a", "甲", 0.0, 0.0, 16.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"text_add("b", "乙", 0.0, 20.0, 16.0, 1.0, 1.0, 1.0)"#
        ));
        assert_eq!(scene.borrow().texts.len(), 2);

        let _ = host
            .eval("set_text_enabled(false)")
            .expect("set_text_enabled 应可调用");
        assert!(!scene.borrow().text_enabled, "开关应已关闭");
        assert_eq!(scene.borrow().texts.len(), 2, "关通道不清数据");

        let _ = host.eval("text_clear()").expect("text_clear 应可调用");
        assert!(scene.borrow().texts.is_empty(), "清空后应为 0 条");
        assert!(!scene.borrow().text_enabled, "清空不应顺带打开通道开关");
    }

    // ---------------- M1 最小 UI 脚本绑定 ----------------

    /// 每帧重复提交同一 key：节点数恒为 1，且尺寸单位按最新一次解析。
    #[test]
    fn per_frame_ui_rect_does_not_accumulate() {
        let (mut host, scene) = host();
        for frame in 0..10 {
            let src = format!(
                r#"ui_rect("hp", "topleft", "24px", "{}w", "50%", "12px", 0.2, 0.6, 0.2, 0.9)"#,
                0
            );
            let created = eval_bool(&mut host, &src);
            assert_eq!(created, frame == 0, "只有首帧是新建，之后都是覆盖");
            assert_eq!(scene.borrow().ui.len(), 1, "第 {frame} 帧后节点数应仍为 1");
        }
        let el = scene.borrow().ui[0].clone();
        assert_eq!(el.kind, r3e_core::ui::UiKind::Rect);
        assert_eq!(el.anchor, r3e_core::ui::Anchor::TopLeft);
        assert_eq!(el.size[0], r3e_core::ui::UiValue::pct(50.0));
        assert_eq!(el.size[1], r3e_core::ui::UiValue::px(12.0));
        assert_eq!(el.color, [0.2, 0.6, 0.2, 0.9]);
    }

    /// 三类元素 + 关键字型 setter 都能改到场景数据。
    #[test]
    fn ui_border_and_image_and_setters() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_border("frame", "bottomcenter", "0px", "-32px", "70%", "140px", 2.0, 1.0, 1.0, 1.0, 0.8)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"ui_image("icon", "topright", "-24px", "24px", "48px", "48px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0)"#
        ));
        {
            let scene = scene.borrow();
            assert_eq!(scene.ui.len(), 2);
            assert_eq!(scene.ui[0].border_width, 2.0);
            assert_eq!(scene.ui[1].image, "assets/ui/icon.png");
            assert_eq!(scene.ui[1].offset[0], r3e_core::ui::UiValue::px(-24.0));
        }

        // 覆盖式重提不会丢掉 layer / visible（样式保留、位置尺寸刷新）
        assert!(eval_bool(&mut host, r#"ui_set_layer("icon", 5)"#));
        assert!(!eval_bool(
            &mut host,
            r#"ui_rect("icon", "bottomright", "0px", "0px", "8px", "8px", 1.0, 0.0, 0.0, 1.0)"#
        ));
        {
            let scene = scene.borrow();
            let icon = scene.ui.iter().find(|e| e.key == "icon").unwrap();
            assert_eq!(icon.layer, 5, "层级属于样式，重提后应保留");
            assert_eq!(icon.anchor, r3e_core::ui::Anchor::BottomRight);
            assert_eq!(icon.kind, r3e_core::ui::UiKind::Rect);
        }

        assert!(eval_bool(&mut host, r#"ui_set_alpha("icon", 0.25)"#));
        assert_eq!(scene.borrow().ui[1].color[3], 0.25);
        assert!(eval_bool(&mut host, r#"ui_set_visible("icon", false)"#));
        assert!(!scene.borrow().ui[1].visible);
        assert!(eval_bool(
            &mut host,
            r#"ui_set_uv("icon", 0.0, 0.0, 0.5, 1.0)"#
        ));
        assert_eq!(scene.borrow().ui[1].uv, [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(host.eval("ui_count()").unwrap().as_int().unwrap(), 2);
    }

    /// 九宫格边距：像素 / 百分比都收，无参调用等于关闭（清零）。
    #[test]
    fn ui_set_slice_sets_and_clears_margins() {
        use r3e_core::ui::{UiRect, UiValue};
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_image("panel", "center", "0px", "0px", "200px", "100px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0)"#
        ));
        // 默认关闭
        assert_eq!(
            scene.borrow().ui[0].slice_margin,
            [
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0)
            ]
        );

        // 像素：左 / 上 / 右 / 下
        assert!(eval_bool(
            &mut host,
            r#"ui_set_slice("panel", "16px", "8px", "16px", "8px")"#
        ));
        assert_eq!(
            scene.borrow().ui[0].slice_margin,
            [
                UiValue::px(16.0),
                UiValue::px(8.0),
                UiValue::px(16.0),
                UiValue::px(8.0)
            ]
        );

        // 百分比（按元素自身宽 / 高换算）
        assert!(eval_bool(
            &mut host,
            r#"ui_set_slice("panel", "10%", "20%", "10%", "20%")"#
        ));
        let el = scene.borrow().ui[0].clone();
        assert_eq!(el.slice_margin[0], UiValue::pct(10.0));
        assert_eq!(
            el.slice_margins_px(&UiRect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 100.0
            }),
            [20.0, 20.0, 20.0, 20.0]
        );

        // 无参 = 关闭
        assert!(eval_bool(&mut host, r#"ui_set_slice("panel")"#));
        assert_eq!(
            scene.borrow().ui[0].slice_margin,
            [UiValue::px(0.0); 4],
            "无参调用应清零"
        );

        // 不存在的 key 返回 false
        assert!(!eval_bool(
            &mut host,
            r#"ui_set_slice("nope", "4px", "4px", "4px", "4px")"#
        ));
        assert_eq!(scene.borrow().ui.len(), 1, "失败的写入不应造出元素");
    }

    /// 九宫格边距是样式字段：每帧重发几何不会把它冲掉。
    #[test]
    fn ui_slice_margin_survives_per_frame_geometry_resubmit() {
        use r3e_core::ui::UiValue;
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_image("panel", "center", "0px", "0px", "200px", "100px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"ui_set_slice("panel", "12px", "12px", "12px", "12px")"#
        ));
        // 下一帧只重发几何（脚本常规写法）
        assert!(!eval_bool(
            &mut host,
            r#"ui_image("panel", "center", "10px", "0px", "300px", "150px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0)"#
        ));
        let el = scene.borrow().ui[0].clone();
        assert_eq!(el.size[0], UiValue::px(300.0), "几何应更新");
        assert_eq!(
            el.slice_margin,
            [UiValue::px(12.0); 4],
            "九宫格边距应沿用旧值"
        );
    }

    /// 不存在的 key 一律返回 false，且不会凭空造出元素。
    #[test]
    fn ui_set_on_missing_key_reports_false() {
        let (mut host, scene) = host();
        assert!(!eval_bool(&mut host, r#"ui_set_pos("nope", "1px", "2px")"#));
        assert!(!eval_bool(
            &mut host,
            r#"ui_set_size("nope", "1px", "2px")"#
        ));
        assert!(!eval_bool(&mut host, r#"ui_set_anchor("nope", "center")"#));
        assert!(!eval_bool(&mut host, r#"ui_set_anchor("nope", "middle")"#));
        assert!(!eval_bool(&mut host, r#"ui_set_layer("nope", 1)"#));
        assert!(!eval_bool(&mut host, r#"ui_remove("nope")"#));
        assert!(scene.borrow().ui.is_empty(), "失败的写入不应造出元素");
    }

    /// 清空与通道开关 / 层次开关互不干扰。
    #[test]
    fn ui_clear_and_channel_switches() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_rect("a", "center", "0px", "0px", "10px", "10px", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"ui_rect("b", "center", "0px", "0px", "10px", "10px", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert_eq!(scene.borrow().ui.len(), 2);

        let _ = host.eval("set_ui_enabled(false)").unwrap();
        assert!(!scene.borrow().ui_enabled);
        assert_eq!(scene.borrow().ui.len(), 2, "关通道不清数据");
        assert!(!scene.borrow().ui_above_text, "默认文本在上");

        let _ = host.eval("set_ui_above_text(true)").unwrap();
        assert!(eval_bool(&mut host, "ui_above_text()"));
        assert!(!eval_bool(&mut host, "ui_enabled()"));

        let _ = host.eval("ui_clear()").unwrap();
        assert!(scene.borrow().ui.is_empty());
        assert!(!scene.borrow().ui_enabled, "清空不应顺带打开通道开关");
    }

    /// 容量上限对脚本路径生效：满载后新建被拒、返回 false、计数不增长，上限值可读。
    #[test]
    fn ui_capacity_is_enforced_on_script_path() {
        let (mut host, scene) = host();
        assert_eq!(
            host.eval("ui_capacity()").unwrap().as_int().unwrap(),
            r3e_core::ui::MAX_UI_ELEMENTS as i64,
            "脚本可读到 core 定的上限"
        );

        // 用 core 直接铺满，验证的是「脚本这一侧是否还会绕过上限」。
        {
            let mut s = scene.borrow_mut();
            for i in 0..r3e_core::ui::MAX_UI_ELEMENTS {
                let mut el = r3e_core::ui::UiElement::new(
                    format!("fill{i}"),
                    r3e_core::ui::UiKind::Rect,
                    r3e_core::ui::Anchor::TopLeft,
                );
                el.size = [
                    r3e_core::ui::UiValue::px(1.0),
                    r3e_core::ui::UiValue::px(1.0),
                ];
                s.ui.push(el);
            }
        }
        assert_eq!(
            host.eval("ui_count()").unwrap().as_int().unwrap(),
            r3e_core::ui::MAX_UI_ELEMENTS as i64
        );

        let before = eval_bool(
            &mut host,
            r#"ui_rect("overflow", "center", "0px", "0px", "8px", "8px", 1.0, 1.0, 1.0, 1.0)"#,
        );
        assert!(!before, "超限必须返回 false，不得静默成功");
        assert_eq!(
            scene.borrow().ui.len(),
            r3e_core::ui::MAX_UI_ELEMENTS,
            "被拒的元素不得写进场景"
        );
        assert!(
            !scene.borrow().ui.iter().any(|e| e.key == "overflow"),
            "被拒的 key 不应存在"
        );
        assert_eq!(
            host.eval("ui_error_count()").unwrap().as_int().unwrap(),
            1,
            "被拒一次应记账一次"
        );
        let msg = host.eval("ui_last_error()").unwrap().into_string().unwrap();
        assert!(msg.contains("上限"), "错误信息应说明是容量上限: {msg}");
        assert!(msg.contains("overflow"), "错误信息应带上被拒的 key: {msg}");

        // 边框 / 图片入口同样受上限约束（三条入口共用唯一写点）。
        for src in [
            r#"ui_border("ob", "center", "0px", "0px", "8px", "8px", 2.0, 1.0, 1.0, 1.0, 1.0)"#,
            r#"ui_image("oi", "center", "0px", "0px", "8px", "8px", "a.png", 1.0, 1.0, 1.0, 1.0)"#,
        ] {
            assert!(
                !eval_bool(&mut host, src),
                "超限入口应一律返回 false: {src}"
            );
        }
        assert_eq!(scene.borrow().ui.len(), r3e_core::ui::MAX_UI_ELEMENTS);
        assert_eq!(
            host.eval("ui_error_count()").unwrap().as_int().unwrap(),
            3,
            "三种入口的被拒都应记账"
        );
    }

    /// 满载时覆盖已有 key 仍然成功：上限只拦新增，不拦更新。
    #[test]
    fn ui_capacity_still_allows_update_when_full() {
        let (mut host, scene) = host();
        {
            let mut s = scene.borrow_mut();
            for i in 0..r3e_core::ui::MAX_UI_ELEMENTS {
                let mut el = r3e_core::ui::UiElement::new(
                    format!("fill{i}"),
                    r3e_core::ui::UiKind::Rect,
                    r3e_core::ui::Anchor::TopLeft,
                );
                el.size = [
                    r3e_core::ui::UiValue::px(1.0),
                    r3e_core::ui::UiValue::px(1.0),
                ];
                s.ui.push(el);
            }
        }
        assert!(!eval_bool(
            &mut host,
            r#"ui_rect("fill0", "center", "0px", "0px", "9px", "9px", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert_eq!(
            scene.borrow().ui.len(),
            r3e_core::ui::MAX_UI_ELEMENTS,
            "覆盖不增长"
        );
        assert_eq!(
            scene.borrow().ui[0].size[0],
            r3e_core::ui::UiValue::px(9.0),
            "满载时覆盖仍要改到数据"
        );
        assert_eq!(
            host.eval("ui_error_count()").unwrap().as_int().unwrap(),
            0,
            "覆盖不算错误"
        );
    }

    /// 空 key 一律拒绝并记账，不得造出无 key 元素。
    #[test]
    fn ui_empty_key_is_rejected() {
        let (mut host, scene) = host();
        assert!(!eval_bool(
            &mut host,
            r#"ui_rect("", "center", "0px", "0px", "8px", "8px", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(!eval_bool(
            &mut host,
            r#"ui_rect("   ", "center", "0px", "0px", "8px", "8px", 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(scene.borrow().ui.is_empty(), "空 key 不应写入");
        assert_eq!(host.eval("ui_error_count()").unwrap().as_int().unwrap(), 2);
        let msg = host.eval("ui_last_error()").unwrap().into_string().unwrap();
        assert!(msg.contains("key"), "错误信息应指明 key 问题: {msg}");
    }

    /// 同 key 跨类型重设：类型 / 几何走新值，样式（layer / visible / uv）沿用旧值。
    #[test]
    fn ui_same_key_across_kinds_keeps_style() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_border("frame", "topleft", "0px", "0px", "10px", "10px", 3.0, 1.0, 1.0, 1.0, 1.0)"#
        ));
        assert!(eval_bool(&mut host, r#"ui_set_layer("frame", 4)"#));
        assert!(eval_bool(&mut host, r#"ui_set_visible("frame", false)"#));
        assert!(eval_bool(
            &mut host,
            r#"ui_set_uv("frame", 0.0, 0.0, 0.25, 0.5)"#
        ));

        // 换成图片元素：不该新增，样式仍在
        assert!(!eval_bool(
            &mut host,
            r#"ui_image("frame", "bottomright", "1px", "2px", "20px", "30px", "b.png", 0.5, 0.5, 0.5, 1.0)"#
        ));
        {
            let s = scene.borrow();
            assert_eq!(s.ui.len(), 1, "同 key 不增长");
            let el = &s.ui[0];
            assert_eq!(el.kind, r3e_core::ui::UiKind::Image);
            assert_eq!(el.image, "b.png");
            assert_eq!(el.anchor, r3e_core::ui::Anchor::BottomRight);
            assert_eq!(el.layer, 4, "层级沿用");
            assert!(!el.visible, "可见性沿用");
            assert_eq!(el.uv, [0.0, 0.0, 0.25, 0.5], "uv 沿用");
            assert_eq!(el.border_width, 0.0, "换成非边框后线宽归零");
        }

        // 换回边框：线宽走新值
        assert!(!eval_bool(
            &mut host,
            r#"ui_border("frame", "center", "0px", "0px", "5px", "5px", 6.0, 1.0, 1.0, 1.0, 1.0)"#
        ));
        {
            let s = scene.borrow();
            assert_eq!(s.ui.len(), 1);
            assert_eq!(s.ui[0].kind, r3e_core::ui::UiKind::Border);
            assert_eq!(s.ui[0].border_width, 6.0, "显式线宽应覆盖");
            assert_eq!(s.ui[0].layer, 4, "样式仍沿用");
        }
        assert_eq!(
            host.eval("ui_error_count()").unwrap().as_int().unwrap(),
            0,
            "覆盖不算错误"
        );
    }

    /// 非法参数不 panic：坏锚点回退左上并记账，颜色越界由 core 收敛。
    #[test]
    fn ui_invalid_params_are_sanitized() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"ui_rect("bad", "nowhere", "0px", "0px", "8px", "8px", 5.0, -2.0, 0.5, 1.0)"#
        ));
        {
            let s = scene.borrow();
            assert_eq!(s.ui.len(), 1, "坏锚点不应阻断写入");
            assert_eq!(s.ui[0].anchor, r3e_core::ui::Anchor::TopLeft, "回退左上");
            assert_eq!(s.ui[0].color, [1.0, 0.0, 0.5, 1.0], "颜色由 core 收敛");
        }
        assert_eq!(
            host.eval("ui_error_count()").unwrap().as_int().unwrap(),
            1,
            "坏锚点应记账"
        );
        let msg = host.eval("ui_last_error()").unwrap().into_string().unwrap();
        assert!(msg.contains("锚点"), "错误信息应指明锚点: {msg}");
    }
}

#[cfg(test)]
mod audio_script_tests {
    use super::*;

    /// 静默音频后端 + 空场景：等价于「这台机器没有声卡」的运行环境。
    fn host_silent() -> ScriptHost {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        // ScriptHost::new 内部就是 Audio::silent —— 单测不碰真设备。
        ScriptHost::new(scene, physics, input)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_bool()
            .expect("应返回 bool")
    }

    fn eval_int(host: &mut ScriptHost, src: &str) -> i64 {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_int()
            .expect("应返回整数")
    }

    fn eval_float(host: &mut ScriptHost, src: &str) -> f64 {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_float()
            .expect("应返回浮点")
    }

    /// 仓库自带素材的绝对路径；Rhai 字符串里统一用正斜杠。
    fn asset_wav() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/audio/blip.wav")
            .to_string_lossy()
            .replace('\\', "/")
    }

    #[test]
    fn audio_api_degrades_silently_without_device() {
        let mut host = host_silent();
        assert!(
            !eval_bool(&mut host, "audio_available()"),
            "静默后端应报告设备不可用"
        );
        assert_eq!(eval_int(&mut host, "audio_count()"), 0, "初始没有在播实例");
        let backend = host.eval("audio_backend()").unwrap().into_string().unwrap();
        assert_eq!(backend, "silent", "无设备时后端名应回落为 silent");

        // 解码能过就返回 true，但不会登记成「在播」，也不会 panic
        let wav = asset_wav();
        assert!(
            eval_bool(
                &mut host,
                &format!(r#"audio_play_ex("hit", "{wav}", 0.8, true)"#)
            ),
            "素材存在时播放请求应返回 true"
        );
        assert_eq!(
            eval_int(&mut host, "audio_count()"),
            0,
            "静默后端不占实例名额"
        );
        assert!(!eval_bool(&mut host, r#"audio_is_playing("hit")"#));
        assert!(!eval_bool(&mut host, r#"audio_is_paused("hit")"#));

        let status = host.eval("audio_status()").unwrap().into_string().unwrap();
        assert!(status.contains("silent"), "状态行应带后端名: {status}");
    }

    #[test]
    fn audio_load_failure_returns_false_and_records_error() {
        let mut host = host_silent();
        let before = eval_int(&mut host, "audio_error_count()");
        assert!(
            !eval_bool(
                &mut host,
                r#"audio_play("ghost", "assets/audio/no_such_file.wav")"#
            ),
            "缺文件必须返回 false 而不是 panic"
        );
        assert!(
            eval_int(&mut host, "audio_error_count()") > before,
            "失败必须记账（error_count 递增）"
        );
        let err = host
            .eval("audio_last_error()")
            .unwrap()
            .into_string()
            .unwrap();
        assert!(err.contains("no_such_file.wav"), "错误应带上路径: {err}");
    }

    #[test]
    fn audio_volume_bounds_are_clamped() {
        let mut host = host_silent();
        let _ = host.eval("audio_set_master_volume(5.0)").unwrap();
        assert!(
            (eval_float(&mut host, "audio_master_volume()") - 1.0).abs() < 1e-6,
            "上界应夹到 1"
        );
        let _ = host.eval("audio_set_master_volume(-2.0)").unwrap();
        assert_eq!(
            eval_float(&mut host, "audio_master_volume()"),
            0.0,
            "下界应夹到 0"
        );
        let _ = host.eval("audio_set_master_volume(0.25)").unwrap();
        assert!((eval_float(&mut host, "audio_master_volume()") - 0.25).abs() < 1e-6);

        // 未知 key 改音量 / 改循环：返回 false，不 panic
        assert!(!eval_bool(&mut host, r#"audio_set_volume("nobody", 0.5)"#));
        assert!(!eval_bool(&mut host, r#"audio_set_loop("nobody", true)"#));
    }

    #[test]
    fn audio_duplicate_key_does_not_stack_or_log_errors() {
        let mut host = host_silent();
        let wav = asset_wav();
        let before = eval_int(&mut host, "audio_error_count()");
        assert!(eval_bool(
            &mut host,
            &format!(r#"audio_play("sfx", "{wav}")"#)
        ));
        // 同名 key 再播 = 重播，不是错误、也不是第二层
        assert!(eval_bool(
            &mut host,
            &format!(r#"audio_play("sfx", "{wav}")"#)
        ));
        assert_eq!(
            eval_int(&mut host, "audio_error_count()"),
            before,
            "重播不该被记成错误"
        );
        assert_eq!(
            eval_int(&mut host, "audio_count()"),
            0,
            "同名 key 不会堆成两个实例"
        );
        // 空 key 视为非法参数：返回 false 并记账，不 panic
        assert!(!eval_bool(
            &mut host,
            &format!(r#"audio_play("  ", "{wav}")"#)
        ));
        assert!(eval_int(&mut host, "audio_error_count()") > before);
    }

    #[test]
    fn audio_enable_switch_blocks_play() {
        let mut host = host_silent();
        let wav = asset_wav();
        let _ = host.eval("audio_set_enabled(false)").unwrap();
        assert!(!eval_bool(&mut host, "audio_enabled()"));
        assert!(
            !eval_bool(&mut host, &format!(r#"audio_play("sfx", "{wav}")"#)),
            "通道关闭后播放请求应被拒绝"
        );
        let _ = host.eval("audio_set_enabled(true)").unwrap();
        assert!(eval_bool(&mut host, "audio_enabled()"));
        assert!(eval_bool(
            &mut host,
            &format!(r#"audio_play("sfx", "{wav}")"#)
        ));
        // stop_all / keys 在静默后端下也必须可用且不 panic
        let _ = host.eval("audio_stop_all()").unwrap();
        assert_eq!(eval_int(&mut host, "audio_count()"), 0);
        assert_eq!(eval_int(&mut host, "audio_keys().len()"), 0);
    }
}

/// 后处理脚本 API（扩展十三）：`set_postfx` 一族写到 `Scene::postfx`，
/// 并且一律过 `PostFxSettings::sanitized`——脚本里算出来的 NaN / 超界值
/// 不能一路灌进 uniform。
#[cfg(test)]
mod postfx_script_tests {
    use super::*;

    fn host() -> (ScriptHost, Rc<RefCell<Scene>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene.clone(), physics, input), scene)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    fn eval_float(host: &mut ScriptHost, src: &str) -> f64 {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_float()
            .unwrap_or_else(|_| panic!("`{src}` 应返回浮点"))
    }

    /// 一击式全量设置：一次调用把五个字段都落到场景上。
    #[test]
    fn set_postfx_writes_all_fields() {
        let (mut host, scene) = host();
        // `set_postfx` 与 `set_sky` 一族同风格：写接口不返回 bool，只看场景结果。
        let _ = host
            .eval(r#"set_postfx(0.35, "aces", true, 0.25, true)"#)
            .unwrap();
        let fx = scene.borrow().postfx.expect("postfx 段应实体化");
        assert!((fx.exposure - 0.35).abs() < 1e-6);
        assert_eq!(fx.tonemap, r3e_core::postfx::TonemapMode::Aces);
        assert!(fx.bloom.enabled);
        assert!((fx.bloom.intensity - 0.25).abs() < 1e-6);
        assert!(fx.fxaa);
        // 没动的字段保持缺省，不因这次调用被清零。
        assert!((fx.bloom.threshold - r3e_core::postfx::DEFAULT_BLOOM_THRESHOLD).abs() < 1e-6);
    }

    /// 分项设置：曝光 / 曲线 / 泛光 / FXAA 各自可改，且互不覆盖。
    #[test]
    fn granular_setters_compose() {
        let (mut host, scene) = host();
        let _ = host.eval("set_exposure(-2.0)").unwrap();
        let _ = host.eval(r#"set_tonemap("reinhard")"#).unwrap();
        let _ = host.eval("set_bloom(true, 2.0, 0.25, 0.5)").unwrap();
        let _ = host.eval("set_fxaa(true)").unwrap();
        let fx = scene.borrow().postfx.expect("postfx 段应实体化");
        assert!((fx.exposure + 2.0).abs() < 1e-6, "曝光应保持 -2 EV");
        assert_eq!(fx.tonemap, r3e_core::postfx::TonemapMode::Reinhard);
        assert!(fx.bloom.enabled);
        assert!((fx.bloom.threshold - 2.0).abs() < 1e-6);
        assert!((fx.bloom.knee - 0.25).abs() < 1e-6);
        assert!((fx.bloom.intensity - 0.5).abs() < 1e-6);
        assert!(fx.fxaa);
        // 回读接口与场景一致。
        assert!((eval_float(&mut host, "postfx_exposure()") + 2.0).abs() < 1e-6);
        assert_eq!(
            host.eval("postfx_tonemap()")
                .unwrap()
                .into_string()
                .unwrap(),
            "reinhard"
        );
    }

    /// 越界值被收敛：曝光夹进 [-8, 16]，泛光强度夹进 [0, 4]。
    #[test]
    fn out_of_range_values_are_clamped() {
        let (mut host, scene) = host();
        let _ = host.eval("set_exposure(999.0)").unwrap();
        let fx = scene.borrow().postfx.expect("postfx 段应实体化");
        assert!(
            (fx.exposure - r3e_core::postfx::MAX_EXPOSURE_EV).abs() < 1e-6,
            "曝光应被夹到上限，实际 {}",
            fx.exposure
        );
        let _ = host.eval("set_bloom(true, -5.0, -1.0, -3.0)").unwrap();
        let fx = scene.borrow().postfx.expect("postfx 段应实体化");
        assert!(fx.bloom.threshold >= 0.0, "阈值下限为 0");
        assert!(fx.bloom.knee >= 0.0, "软膝下限为 0");
        assert!(fx.bloom.intensity >= 0.0, "负强度会让泛光变成减光");
        // 曝光归零后与缺省一致，泛光关掉即可回到「直通」。
        let _ = host.eval("set_exposure(0.0)").unwrap();
        let _ = host.eval("set_bloom(false, 1.0, 0.5, 0.18)").unwrap();
        let fx = scene.borrow().postfx.unwrap_or_default();
        assert!(fx.is_neutral(), "全部关掉后应判定为直通");
    }

    /// 未知曲线名不 panic：回落 none 并保留其余字段。
    #[test]
    fn unknown_tonemap_falls_back_to_none() {
        let (mut host, scene) = host();
        let _ = host
            .eval(r#"set_postfx(1.0, "aces", false, 0.0, false)"#)
            .unwrap();
        let _ = host.eval(r#"set_tonemap("cinematic")"#).unwrap();
        let fx = scene.borrow().postfx.expect("postfx 段应实体化");
        assert_eq!(fx.tonemap, r3e_core::postfx::TonemapMode::None);
        assert!((fx.exposure - 1.0).abs() < 1e-6, "其余字段不该被牵连");
    }

    /// 清空开关：`clear_postfx` 后回到「离屏 HDR + composite 直通」。
    #[test]
    fn clear_and_has_postfx_toggle() {
        let (mut host, scene) = host();
        assert!(!eval_bool(&mut host, "has_postfx()"));
        let _ = host.eval("set_fxaa(true)").unwrap();
        assert!(eval_bool(&mut host, "has_postfx()"));
        let _ = host.eval("clear_postfx()").unwrap();
        assert!(!eval_bool(&mut host, "has_postfx()"));
        assert!(scene.borrow().postfx.is_none(), "清空后应为 None");
        // 未开启时回读给出缺省值，不 panic。
        assert_eq!(eval_float(&mut host, "postfx_exposure()"), 0.0);
    }
}

#[cfg(test)]
mod ui_interaction_script_tests {
    use super::*;
    use r3e_core::ui::{Anchor, UiElement, UiValue};

    const SCR: (f32, f32) = (800.0, 600.0);

    fn host() -> (ScriptHost, Rc<RefCell<Scene>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene.clone(), physics, input), scene)
    }

    fn button(scene: &Rc<RefCell<Scene>>, key: &str, x: f32, y: f32) {
        let mut el = UiElement::new(key, r3e_core::ui::UiKind::Rect, Anchor::TopLeft);
        el.offset = [UiValue::px(x), UiValue::px(y)];
        el.size = [UiValue::px(120.0), UiValue::px(40.0)];
        scene.borrow_mut().ui.push(el);
    }

    fn eval_str(host: &mut ScriptHost, src: &str) -> String {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .into_string()
            .unwrap_or_else(|_| panic!("`{src}` 应返回字符串"))
    }

    /// 事件数组：长度 / 字段名 / 顺序都要按 core 的语义落进脚本。
    #[test]
    fn ui_events_exposes_kind_and_key() {
        let (mut host, scene) = host();
        button(&scene, "btn", 10.0, 10.0);
        let ui = host.ui_interaction();
        let list = scene.borrow().ui.clone();

        // 进入 → 按下 → 抬起：hovered / pressed / clicked / released
        ui.borrow_mut()
            .update(&list, SCR, Some((20.0, 20.0)), false, false, false);
        ui.borrow_mut()
            .update(&list, SCR, Some((20.0, 20.0)), true, true, false);
        ui.borrow_mut()
            .update(&list, SCR, Some((20.0, 20.0)), false, false, true);

        assert_eq!(host.eval("ui_event_count()").unwrap().as_int().unwrap(), 4);
        assert_eq!(eval_str(&mut host, "ui_event_kind_name(0)"), "hovered");
        assert_eq!(eval_str(&mut host, "ui_event_kind_name(2)"), "clicked");
        assert_eq!(eval_str(&mut host, "ui_event_key(2)"), "btn");
        assert!(host
            .eval(r#"ui_events()[2]["kind"] == "clicked""#)
            .unwrap()
            .as_bool()
            .unwrap());
        assert!(host
            .eval(r#"ui_events()[2]["key"] == "btn""#)
            .unwrap()
            .as_bool()
            .unwrap());
    }

    /// hover / active / pointer_over_ui 三个状态读口子。
    #[test]
    fn hover_and_active_are_readable() {
        let (mut host, scene) = host();
        button(&scene, "btn", 10.0, 10.0);
        let ui = host.ui_interaction();
        let list = scene.borrow().ui.clone();

        assert_eq!(eval_str(&mut host, "ui_hover()"), "");
        assert!(!host
            .eval("ui_pointer_over_ui()")
            .unwrap()
            .as_bool()
            .unwrap());

        ui.borrow_mut()
            .update(&list, SCR, Some((20.0, 20.0)), true, true, false);
        assert_eq!(eval_str(&mut host, "ui_hover()"), "btn");
        assert_eq!(eval_str(&mut host, "ui_active()"), "btn");
        assert!(host
            .eval("ui_pointer_over_ui()")
            .unwrap()
            .as_bool()
            .unwrap());
        assert!(host
            .eval(r#"ui_hovered("btn")"#)
            .unwrap()
            .as_bool()
            .unwrap());
        assert!(!host
            .eval(r#"ui_clicked("btn")"#)
            .unwrap()
            .as_bool()
            .unwrap());

        ui.borrow_mut()
            .update(&list, SCR, Some((20.0, 20.0)), false, false, true);
        assert!(host
            .eval(r#"ui_clicked("btn")"#)
            .unwrap()
            .as_bool()
            .unwrap());
        assert_eq!(eval_str(&mut host, "ui_active()"), "");
    }

    /// `ui_hit` 用最近一帧的屏幕尺寸换算矩形——和渲染出的位置一致。
    #[test]
    fn ui_hit_uses_last_screen_size() {
        let (mut host, scene) = host();
        button(&scene, "btn", 10.0, 10.0);
        let ui = host.ui_interaction();
        {
            let list = scene.borrow().ui.clone();
            ui.borrow_mut()
                .update(&list, SCR, None, false, false, false);
        }
        assert_eq!(eval_str(&mut host, "ui_hit(20.0, 20.0)"), "btn");
        assert_eq!(eval_str(&mut host, "ui_hit(500.0, 400.0)"), "");
        // 未送屏幕尺寸时（screen = 0）一律不命中，不 panic。
        ui.borrow_mut().clear();
        ui.borrow_mut().set_screen((0.0, 0.0));
        assert_eq!(eval_str(&mut host, "ui_hit(20.0, 20.0)"), "");
    }

    /// 越界索引读事件：返回空串而不是抛错。
    #[test]
    fn out_of_range_event_reads_are_empty() {
        let (mut host, _scene) = host();
        assert_eq!(eval_str(&mut host, "ui_event_kind_name(5)"), "");
        assert_eq!(eval_str(&mut host, "ui_event_key(5)"), "");
        assert_eq!(eval_str(&mut host, "ui_event_kind_name(-1)"), "");
    }

    /// 事件集合在脚本里可被 for 循环消费（这是脚本用起来最基本的形态）。
    #[test]
    fn events_can_be_looped_in_script() {
        let (mut host, scene) = host();
        button(&scene, "a", 0.0, 0.0);
        button(&scene, "b", 200.0, 0.0);
        let ui = host.ui_interaction();
        let list = scene.borrow().ui.clone();
        ui.borrow_mut()
            .update(&list, SCR, Some((10.0, 10.0)), false, false, false);
        ui.borrow_mut()
            .update(&list, SCR, Some((210.0, 10.0)), false, false, false);

        let src = r#"
            let n = 0;
            for e in ui_events() {
                if e["kind"] == "hovered" && e["key"] != "" { n += 1; }
            }
            n
        "#;
        assert_eq!(host.eval(src).unwrap().as_int().unwrap(), 2);
    }
}

#[cfg(test)]
mod local_light_script_tests {
    use super::*;

    /// 空场景 + 物理/输入句柄的脚本宿主。
    fn host() -> (ScriptHost, Rc<RefCell<Scene>>) {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        (ScriptHost::new(scene.clone(), physics, input), scene)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("`{src}` 应返回 bool"))
    }

    fn eval_i64(host: &mut ScriptHost, src: &str) -> i64 {
        host.eval(src)
            .unwrap_or_else(|e| panic!("`{src}` 执行失败: {e}"))
            .as_int()
            .unwrap_or_else(|_| panic!("`{src}` 应返回整数"))
    }

    /// 点光写入后，场景里的值必须与脚本参数逐字段一致（含 range）。
    #[test]
    fn script_adds_point_light() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"light_add_point("lamp", 1.0, 2.0, 3.0, 1.0, 0.5, 0.25, 8.0, 12.0)"#
        ));
        let sc = scene.borrow();
        let l = sc.local_light("lamp").expect("灯应已写入场景");
        assert_eq!(l.pos, [1.0, 2.0, 3.0]);
        assert_eq!(l.color, [1.0, 0.5, 0.25]);
        assert_eq!(l.intensity, 8.0);
        assert_eq!(l.range, 12.0);
        assert_eq!(l.kind, r3e_core::LocalLightKind::Point);
    }

    /// 聚光写入：朝向、内外角都要落到场景里，且已通过 sanitized 收敛。
    #[test]
    fn script_adds_spot_light() {
        let (mut host, scene) = host();
        assert!(eval_bool(
            &mut host,
            r#"light_add_spot("beam", 0.0, 5.0, 0.0, 0.0, -1.0, 0.0, 1.0, 1.0, 1.0, 20.0, 30.0, 15.0, 25.0)"#
        ));
        let sc = scene.borrow();
        let l = sc.local_light("beam").expect("聚光应已写入场景");
        assert_eq!(l.kind, r3e_core::LocalLightKind::Spot);
        assert_eq!(l.dir, [0.0, -1.0, 0.0]);
        assert_eq!(l.inner_deg, 15.0);
        assert_eq!(l.outer_deg, 25.0);
    }

    /// 同 key 重复写入是「覆盖」而不是「追加」，顺带验证就地修改生效。
    #[test]
    fn script_upsert_same_key_overwrites() {
        let (mut host, scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("lamp", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 10.0)"#,
        );
        eval_bool(
            &mut host,
            r#"light_add_point("lamp", 4.0, 4.0, 4.0, 1.0, 1.0, 1.0, 2.0, 10.0)"#,
        );
        assert_eq!(
            eval_i64(&mut host, "local_light_count()"),
            1,
            "同 key 不应重复"
        );
        assert_eq!(
            scene.borrow().local_light("lamp").unwrap().pos,
            [4.0, 4.0, 4.0]
        );
    }

    /// 单字段改写：位置 / 颜色 / 强度 / 半径 / 朝向 / 锥角各自独立生效。
    #[test]
    fn script_field_setters_touch_only_target() {
        let (mut host, scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("lamp", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 3.0, 10.0)"#,
        );
        assert!(eval_bool(
            &mut host,
            r#"light_set_pos("lamp", 2.0, 3.0, 4.0)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"light_set_color("lamp", 0.2, 0.4, 0.6)"#
        ));
        assert!(eval_bool(&mut host, r#"light_set_intensity("lamp", 9.0)"#));
        assert!(eval_bool(&mut host, r#"light_set_range("lamp", 25.0)"#));
        assert!(eval_bool(
            &mut host,
            r#"light_set_dir("lamp", 0.0, 1.0, 0.0)"#
        ));
        assert!(eval_bool(
            &mut host,
            r#"light_set_cone("lamp", 10.0, 20.0)"#
        ));

        let sc = scene.borrow();
        let l = sc.local_light("lamp").unwrap();
        assert_eq!(l.pos, [2.0, 3.0, 4.0]);
        assert_eq!(l.color, [0.2, 0.4, 0.6]);
        assert_eq!(l.intensity, 9.0);
        assert_eq!(l.range, 25.0);
        assert_eq!(l.dir, [0.0, 1.0, 0.0]);
        assert_eq!(l.inner_deg, 10.0);
        assert_eq!(l.outer_deg, 20.0);
    }

    /// 改不存在的 key 必须返回 false 且**不新建**灯：拼错名字不该悄悄多一盏。
    #[test]
    fn script_setter_on_missing_key_is_rejected() {
        let (mut host, scene) = host();
        assert!(
            !eval_bool(&mut host, r#"light_set_intensity("ghost", 5.0)"#),
            "不存在的 key 应返回 false"
        );
        assert_eq!(
            eval_i64(&mut host, "local_light_count()"),
            0,
            "不应凭空新建灯"
        );
        assert_eq!(scene.borrow().lights.len(), 0);
    }

    /// 删除 / 清空 / 存在性查询的三态。
    #[test]
    fn script_remove_clear_and_exists() {
        let (mut host, scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("a", 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#,
        );
        eval_bool(
            &mut host,
            r#"light_add_point("b", 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#,
        );
        assert!(eval_bool(&mut host, r#"local_light_exists("a")"#));
        assert!(eval_bool(&mut host, r#"light_remove("a")"#));
        assert!(
            !eval_bool(&mut host, r#"light_remove("a")"#),
            "重复删除返回 false"
        );
        assert!(!eval_bool(&mut host, r#"local_light_exists("a")"#));
        assert_eq!(
            eval_i64(&mut host, "light_clear()"),
            1,
            "清空应返回删掉的盏数"
        );
        assert_eq!(scene.borrow().lights.len(), 0);
    }

    /// 上限行为：满载后新增被拒，但同 key 覆盖仍然生效（每帧更新位置不会被吞）。
    #[test]
    fn script_capacity_rejects_new_but_allows_overwrite() {
        let (mut host, scene) = host();
        for i in 0..r3e_core::MAX_LOCAL_LIGHTS {
            let src = format!(r#"light_add_point("l{i}", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#);
            assert!(eval_bool(&mut host, &src), "第 {i} 盏应写入成功");
        }
        assert!(
            !eval_bool(
                &mut host,
                r#"light_add_point("extra", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#
            ),
            "满载后新 key 必须被拒"
        );
        assert!(
            eval_bool(
                &mut host,
                r#"light_add_point("l0", 9.0, 9.0, 9.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#
            ),
            "满载时同 key 覆盖必须仍生效"
        );
        assert_eq!(scene.borrow().lights.len(), r3e_core::MAX_LOCAL_LIGHTS);
        assert_eq!(scene.borrow().local_light("l0").unwrap().pos, [9.0; 3]);
    }

    /// 总开关：关闭时辐照度查询返回 0，但灯数据保留；重新打开即恢复。
    #[test]
    fn script_enable_switch_keeps_data() {
        let (mut host, scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("lamp", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 10.0, 20.0)"#,
        );
        assert!(eval_bool(&mut host, "lights_enabled()"));
        let on = host
            .eval("local_light_irradiance(0.0, 1.0, 0.0)")
            .expect("辐照度查询应可调用")
            .as_float()
            .expect("应返回浮点");
        assert!(on > 0.0, "开灯时该点应收到光照，实际 {on}");

        let _ = host.eval("set_lights_enabled(false)").expect("应可关闭");
        assert!(!eval_bool(&mut host, "lights_enabled()"));
        let off = host
            .eval("local_light_irradiance(0.0, 1.0, 0.0)")
            .unwrap()
            .as_float()
            .unwrap();
        assert_eq!(off, 0.0, "关灯后辐照度必须为 0");
        assert_eq!(scene.borrow().lights.len(), 1, "关闭不应删数据");
    }

    /// 快照 map：字段齐全；查不到时返回空 map（脚本用 is_empty 判空）。
    #[test]
    fn script_light_snapshot_map() {
        let (mut host, _scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_spot("beam", 1.0, 2.0, 3.0, 0.0, -1.0, 0.0, 1.0, 1.0, 1.0, 7.0, 40.0, 12.0, 22.0)"#,
        );
        let map = host
            .eval(r#"local_light("beam")"#)
            .expect("应可调用")
            .cast::<rhai::Map>();
        assert_eq!(
            map.get("key").unwrap().clone().into_string().unwrap(),
            "beam"
        );
        assert_eq!(
            map.get("kind").unwrap().clone().into_string().unwrap(),
            "spot"
        );
        assert_eq!(
            map.get("intensity").unwrap().clone().as_float().unwrap(),
            7.0
        );
        assert_eq!(map.len(), 11, "快照应含 11 个字段（含扩展十八 C 的两项）");
        assert!(
            !map.get("cast_shadow").unwrap().clone().as_bool().unwrap(),
            "新灯默认不投影"
        );

        let empty = host
            .eval(r#"local_light("ghost")"#)
            .expect("应可调用")
            .cast::<rhai::Map>();
        assert!(empty.is_empty(), "查不到时应返回空 map");
    }

    /// keys 列表顺序 = 写入顺序（渲染侧按同一顺序上传，脚本靠下标对齐也不会错位）。
    #[test]
    fn script_keys_follow_insertion_order() {
        let (mut host, _scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("a", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#,
        );
        eval_bool(
            &mut host,
            r#"light_add_point("b", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 5.0)"#,
        );
        let keys = host
            .eval("local_light_keys()")
            .expect("应可调用")
            .cast::<rhai::Array>();
        let names: Vec<String> = keys
            .iter()
            .map(|d| d.clone().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    /// 坏值必须被收敛，不能被脚本塞进场景：NaN / 负强度 / 零半径 / 角度倒挂。
    #[test]
    fn script_bad_values_are_sanitized() {
        let (mut host, scene) = host();
        eval_bool(
            &mut host,
            r#"light_add_point("bad", 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, -5.0, 0.0)"#,
        );
        let sc = scene.borrow();
        let l = sc.local_light("bad").unwrap();
        assert!(
            l.intensity >= 0.0,
            "负强度必须夹到非负，实际 {}",
            l.intensity
        );
        assert!(
            l.range >= r3e_core::MIN_LIGHT_RANGE,
            "零半径必须抬到下限，实际 {}",
            l.range
        );

        drop(sc);
        eval_bool(
            &mut host,
            r#"light_add_spot("cone", 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 30.0, 40.0, 10.0)"#,
        );
        let sc = scene.borrow();
        let l = sc.local_light("cone").unwrap();
        assert!(
            l.outer_deg > l.inner_deg,
            "内外角倒挂必须被修正：inner={} outer={}",
            l.inner_deg,
            l.outer_deg
        );
    }
}

#[cfg(test)]
mod scene_binding_tests {
    use super::*;
    use r3e_core::{Scene, StackEntry};

    fn host() -> ScriptHost {
        let scene = Rc::new(RefCell::new(Scene::new()));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let input = Rc::new(RefCell::new(InputState::default()));
        ScriptHost::new(scene, physics, input)
    }

    fn eval_bool(host: &mut ScriptHost, src: &str) -> bool {
        host.eval(src)
            .unwrap_or_else(|e| panic!("脚本 `{src}` 执行失败: {e}"))
            .as_bool()
            .unwrap_or_else(|_| panic!("脚本 `{src}` 应返回 bool"))
    }

    fn stack_with_two(host: &ScriptHost) {
        let handle = host.scene_stack_handle();
        let mut stack = handle.borrow_mut();
        stack.push(StackEntry::capture("menu", "menu.json", &Scene::new()));
        stack.push(StackEntry::capture("lv1", "lv1.json", &Scene::new()));
    }

    #[test]
    fn script_queues_switch_command() {
        let mut h = host();
        assert!(eval_bool(&mut h, r#"scene_switch("levels/lv2.json")"#));
        assert!(h.scene_command_pending());
        match h.take_scene_command() {
            Some(SceneCommand::Switch { path }) => assert_eq!(path, "levels/lv2.json"),
            other => panic!("应排入 Switch 命令，实际: {other:?}"),
        }
        assert!(!h.scene_command_pending(), "取走后槽必须清空");
        assert!(h.take_scene_command().is_none(), "命令是一次性意图");
    }

    #[test]
    fn path_is_trimmed_and_blank_rejected() {
        let mut h = host();
        assert!(eval_bool(
            &mut h,
            r#"scene_load_archive("  slots/a.json  ")"#
        ));
        match h.take_scene_command() {
            Some(SceneCommand::LoadArchive { path }) => assert_eq!(path, "slots/a.json"),
            other => panic!("应排入 LoadArchive 命令，实际: {other:?}"),
        }
        // 空路径在脚本边界上就说不，不留给宿主去猜
        assert!(!eval_bool(&mut h, r#"scene_switch("   ")"#));
        assert!(!eval_bool(&mut h, r#"scene_load_archive("")"#));
        assert!(!h.scene_command_pending(), "被拒绝的调用不该占槽");
    }

    #[test]
    fn single_slot_rejects_second_command_without_overwrite() {
        let mut h = host();
        assert!(eval_bool(&mut h, "scene_back()"));
        assert!(
            !eval_bool(&mut h, r#"scene_switch("lv2.json")"#),
            "槽已满时应拒绝，而不是覆盖上一条"
        );
        assert_eq!(h.take_scene_command(), Some(SceneCommand::Back));
    }

    #[test]
    fn save_archive_blank_path_means_host_default() {
        let mut h = host();
        assert!(eval_bool(&mut h, r#"scene_save_archive("")"#));
        assert_eq!(
            h.take_scene_command(),
            Some(SceneCommand::SaveArchive { path: None })
        );
        assert!(eval_bool(
            &mut h,
            r#"scene_save_archive("  slots/1.json ")"#
        ));
        assert_eq!(
            h.take_scene_command(),
            Some(SceneCommand::SaveArchive {
                path: Some("slots/1.json".to_string())
            })
        );
    }

    #[test]
    fn stack_readers_follow_shared_handle() {
        let mut h = host();
        assert_eq!(h.eval("scene_stack_depth()").unwrap().as_int(), Ok(0));
        assert_eq!(h.eval("scene_stack_index()").unwrap().as_int(), Ok(-1));
        assert_eq!(
            h.eval("scene_stack_current()")
                .unwrap()
                .into_string()
                .unwrap(),
            ""
        );

        stack_with_two(&h);
        assert_eq!(h.eval("scene_stack_depth()").unwrap().as_int(), Ok(2));
        assert_eq!(h.eval("scene_stack_index()").unwrap().as_int(), Ok(1));
        assert_eq!(
            h.eval("scene_stack_current()")
                .unwrap()
                .into_string()
                .unwrap(),
            "lv1"
        );
        let names = h.eval("scene_stack_names()").unwrap();
        let arr = names.into_array().expect("应返回数组");
        let names: Vec<String> = arr
            .into_iter()
            .map(|d| d.into_string().expect("层名应为字符串"))
            .collect();
        assert_eq!(names, vec!["menu".to_string(), "lv1".to_string()]);
    }

    #[test]
    fn queued_command_survives_scene_rebuild() {
        let mut h = host();
        assert!(eval_bool(&mut h, r#"scene_switch("lv2.json")"#));
        let json = h
            .scene_handle()
            .borrow()
            .to_json_pretty()
            .expect("场景应可序列化");
        h.rebuild_scene(&json).expect("空场景重建应成功");
        // 重建会清脚本作用域，但命令槽在 Rc 里，属于宿主状态，不能被脚本重放抹掉
        assert!(
            matches!(h.take_scene_command(), Some(SceneCommand::Switch { .. })),
            "重建不应吞掉已排队的命令"
        );
    }

    #[test]
    fn host_request_uses_the_same_slot() {
        let h = host();
        assert!(h.request_scene_command(SceneCommand::back()));
        assert!(
            !h.request_scene_command(SceneCommand::back()),
            "满槽时拒绝覆盖"
        );
        assert_eq!(h.take_scene_command(), Some(SceneCommand::Back));
    }
}
