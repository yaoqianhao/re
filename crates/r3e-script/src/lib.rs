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
    /// 音频门面。脚本层**不**自己开设备：句柄由 app 注入，或退化成静默后端（编辑器 / 单测）。
    audio: Rc<RefCell<Audio>>,
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
        let ctx = api::ScriptContext {
            scene: scene.clone(),
            physics: physics.clone(),
            input: input.clone(),
            audio: audio.clone(),
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
            audio,
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
