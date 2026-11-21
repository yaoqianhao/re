//! `ScriptHost` 的构造：把装配层显式给出的能力面拼成宿主。
//!
//! 拆自 `lib.rs`（该文件已 2000+ 行、由 `size_debt` 白名单看管，只许变小）。
//! 构造函数与 `ScriptContext` 的字段填充耦合最紧，放在一起读比追 `lib.rs` 更清楚。

use super::*;

impl ScriptHost {
    /// 构造：`physics` 传能力面（`PhysicsFacade::wrap(rc)` 之类），音频走**默认静默后端**。
    ///
    /// 单测 / 编辑器不需要真声卡，也不该因为没设备就报错。
    pub fn new(
        doc: Rc<RefCell<Document>>,
        physics: Rc<dyn PhysicsHost>,
        input: Rc<RefCell<InputState>>,
    ) -> Self {
        Self::new_with_hosts(
            doc,
            physics,
            input,
            Rc::new(host::SilentAudioHost::new("脚本宿主未接入音频设备")),
        )
    }

    /// 构造：物理 / 音频都传能力面（app 用：脚本与引擎共用同一物理世界与混音器）。
    pub fn new_with_audio(
        doc: Rc<RefCell<Document>>,
        physics: Rc<dyn PhysicsHost>,
        input: Rc<RefCell<InputState>>,
        audio: Rc<dyn AudioHost>,
    ) -> Self {
        Self::new_with_hosts(doc, physics, input, audio)
    }

    /// 底层构造：两个能力面都显式给出（`new` / `new_with_audio` 都落到这里）。
    pub fn new_with_hosts(
        doc: Rc<RefCell<Document>>,
        physics: Rc<dyn PhysicsHost>,
        input: Rc<RefCell<InputState>>,
        audio: Rc<dyn AudioHost>,
    ) -> Self {
        Self::new_with_capabilities(
            doc,
            physics,
            input,
            audio,
            Rc::new(host::NullCharacterHost::new()),
        )
    }

    /// 底层构造：三个能力面都显式给出（M2-B01 起，角色控制器也走注入）。
    ///
    /// `character` 用 [`host::NullCharacterHost`](crate::host::NullCharacterHost) 兜底时，
    /// 脚本里 `character_*` 全部返回「没有控制器」，但**不 panic、不中断脚本**——
    /// 与音频的静默后端同一口径：没接能力不该让整段脚本挂掉。
    pub fn new_with_capabilities(
        doc: Rc<RefCell<Document>>,
        physics: Rc<dyn PhysicsHost>,
        input: Rc<RefCell<InputState>>,
        audio: Rc<dyn AudioHost>,
        character: Rc<dyn CharacterHost>,
    ) -> Self {
        let clock = Rc::new(RefCell::new(Clock::default()));
        let ui_interaction = Rc::new(RefCell::new(UiInteraction::new()));
        // 场景栈与命令槽在构造时就建好：它们会被 `bind_all` 的闭包捕获，
        // 之后再换 `Rc` 不会影响已注册的绑定（与 `ui_interaction` 同一约束）。
        let scene_stack = Rc::new(RefCell::new(SceneStack::new()));
        let scene_cmd: Rc<RefCell<Option<SceneCommand>>> = Rc::new(RefCell::new(None));
        // 包围盒表同样在这里建好：绑定闭包捕获的是这个 `Rc`，之后换表不会生效。
        let pick_bounds = Rc::new(RefCell::new(MeshAabbCache::new()));
        let ctx = api::ScriptContext {
            doc: doc.clone(),
            phys: physics.clone(),
            input: input.clone(),
            audio: audio.clone(),
            character: character.clone(),
            clock: clock.clone(),
            rng: Rc::new(std::cell::Cell::new(0x2545F4914F6CDD1D)),
            ui_log: Rc::new(RefCell::new(api::UiWriteLog::default())),
            ui_interaction: ui_interaction.clone(),
            scene_stack: scene_stack.clone(),
            scene_cmd: scene_cmd.clone(),
            pick_bounds: pick_bounds.clone(),
        };

        let mut engine = Engine::new();
        engine.on_print(|s| println!("[script] {s}"));
        api::bind_all(&mut engine, &ctx);

        Self {
            engine,
            clock,
            doc,
            physics,
            input,
            ui_interaction,
            scene_stack,
            scene_cmd,
            pick_bounds,
            ast: None,
            scope: Scope::new(),
            script_path: None,
            script_mtime: None,
            base_scene_json: None,
            rebuild_hook: None,
            last_error: None,
        }
    }
}
