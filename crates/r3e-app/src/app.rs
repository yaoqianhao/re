//! 应用层：窗口、事件循环、主循环、终端指令与脚本热重载。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Instant, SystemTime};

use glam::Vec3;
use gilrs::{Axis, Button};
use r3e_core::input::{
    InputState, MOUSE_BACK, MOUSE_FORWARD, MOUSE_LEFT, MOUSE_MIDDLE, MOUSE_RIGHT, PAD_AXIS_COUNT,
    PAD_BACK, PAD_BUTTON_COUNT, PAD_LEFT_X, PAD_LEFT_Y, PAD_LT, PAD_RIGHT_X, PAD_RIGHT_Y, PAD_RT,
    PAD_START,
};
use r3e_asset::AssetLibrary;
use r3e_core::{NodeId, ObjectDesc, Scene};
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};
use r3e_render::Renderer;
use r3e_script::ScriptHost;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// 场景文件缺失时的兜底场景，保证引擎任何情况下都能启动。
const FALLBACK_SCENE: &str = include_str!("../../../assets/scene.json");

const MOVE_SPEED: f32 = 6.0;
/// 鼠标拖拽转视角的灵敏度（度 / 像素）。
const MOUSE_ORBIT_SENS: f32 = 0.28;
/// 鼠标右键拖拽平移的灵敏度系数（随相机距离缩放）。
const MOUSE_PAN_SENS: f32 = 0.0016;
/// 手柄右摇杆转视角（度 / 秒）。
const PAD_ORBIT_SPEED: f32 = 150.0;
/// 手柄扳机推拉速度（每秒缩放比例）。
const PAD_ZOOM_SPEED: f32 = 1.6;

/// 手柄按键索引 → gilrs 按键（与 r3e-core 的索引约定一一对应）。
const PAD_BUTTON_MAP: [Option<Button>; PAD_BUTTON_COUNT] = [
    Some(Button::South),
    Some(Button::East),
    Some(Button::West),
    Some(Button::North),
    Some(Button::LeftTrigger),
    Some(Button::RightTrigger),
    Some(Button::Select),
    Some(Button::Start),
    Some(Button::Mode),
    Some(Button::LeftThumb),
    Some(Button::RightThumb),
    Some(Button::DPadUp),
    Some(Button::DPadDown),
    Some(Button::DPadLeft),
    Some(Button::DPadRight),
    None,
];

/// 手柄轴索引 → gilrs 轴。
const PAD_AXIS_MAP: [Option<Axis>; PAD_AXIS_COUNT] = [
    Some(Axis::LeftStickX),
    Some(Axis::LeftStickY),
    Some(Axis::RightStickX),
    Some(Axis::RightStickY),
    Some(Axis::LeftZ),
    Some(Axis::RightZ),
    None,
    None,
];

/// 命令行物理选项。`None` 表示「该层不发言」，交由场景文件与默认值决定。
#[derive(Debug, Clone, Default)]
pub struct CliOptions {
    pub physics: Option<bool>,
    pub backend: Option<String>,
    pub gravity: Option<[f32; 3]>,
    pub ground: Option<f32>,
    /// `--asset <fbx>`（可重复）：启动时导入并 spawn 的静态资产。
    pub assets: Vec<PathBuf>,
    /// `--dump-asset <fbx>`：只做导入并打印报告，不创建窗口。
    pub dump_asset: Option<PathBuf>,
}

impl CliOptions {
    /// 转成物理配置。后端名非法时只告警、不阻断启动。
    pub fn to_config(&self) -> PhysicsConfig {
        let kind = self.backend.as_deref().and_then(|name| {
            let parsed = PhysicsKind::parse(name);
            if parsed.is_none() {
                log::warn!("未知物理后端 `{name}`，可用：builtin / null / rapier(预留)");
            }
            parsed
        });
        PhysicsConfig {
            enabled: self.physics,
            kind,
            gravity: self.gravity.map(Vec3::from),
            ground: self.ground,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.physics.is_none()
            && self.backend.is_none()
            && self.gravity.is_none()
            && self.ground.is_none()
    }

}

pub struct EngineApp {
    scene_path: PathBuf,
    script_path: PathBuf,

    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,

    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    scripts: ScriptHost,
    /// 输入快照：app 层写入，脚本层读取。
    input: Rc<RefCell<InputState>>,
    /// 手柄子系统；初始化失败时为 None（引擎其余功能不受影响）。
    gilrs: Option<gilrs::Gilrs>,
    /// 光标是否已有有效位置（首个 CursorMoved 之前不算位移）。
    cursor_valid: bool,

    last_frame: Instant,
    last_watch: Instant,
    time: f64,
    frame_index: u64,
    paused: bool,
    keys: HashSet<Key>,
    last_dt_ms: f32,
    console: mpsc::Receiver<String>,
    scene_mtime: Option<SystemTime>,
    /// 上次完成「场景声明刚体」绑定的物理世代号。
    physics_gen: u64,
    /// 已导入的外部资产（渲染层通过 `Renderer::set_asset_library` 取得只读引用）。
    assets: Arc<AssetLibrary>,
    /// 从资产 spawn 出来的节点名（便于日志与后续热重载替换）。
    asset_nodes: Vec<NodeId>,
}

impl EngineApp {
    pub fn new(scene_path: PathBuf, script_path: PathBuf, cli: CliOptions) -> Self {
        let scene_src = std::fs::read_to_string(&scene_path).unwrap_or_else(|e| {
            log::warn!(
                "读取场景 {} 失败（{e}），使用内置兜底场景",
                scene_path.display()
            );
            FALLBACK_SCENE.to_string()
        });

        let scene = match Scene::from_json_str(&scene_src) {
            Ok(s) => s,
            Err(e) => {
                log::error!("场景解析失败: {e}，使用内置兜底场景");
                Scene::from_json_str(FALLBACK_SCENE).expect("兜底场景必须合法")
            }
        };
        log::info!("场景 `{}` 载入完成：{} 个物件", scene.name, scene.len());

        // 资产导入：命令行 `--asset` 与场景文件 `assets` 段合并（命令行排在前面）。
        // 场景里的相对路径按场景文件所在目录解析，导入失败不阻断启动。
        let asset_specs = collect_asset_specs(&scene, &scene_path, &cli.assets);
        let assets = Arc::new(load_assets(&asset_specs));

        // 物理：默认关闭；场景文件 physics 段 -> 命令行覆盖（后者优先且会粘住热重载）
        let mut physics = Physics::new();
        if let Err(e) = physics.apply_config(PhysicsConfig::from_scene(&scene.physics)) {
            log::warn!("场景物理配置未应用: {e}");
        }
        if !cli.is_empty() {
            if let Err(e) = physics.apply_override(cli.to_config()) {
                log::warn!("命令行物理选项未应用: {e}");
            }
        }
        log::info!(
            "物理: {} / 后端 {} / 重力 {:?} / 地面 {}",
            if physics.enabled() { "启用" } else { "关闭" },
            physics.kind().name(),
            physics.gravity.to_array(),
            physics.ground()
        );

        let scene = Rc::new(RefCell::new(scene));
        let physics = Rc::new(RefCell::new(physics));
        let input = Rc::new(RefCell::new(InputState::default()));
        let mut scripts = ScriptHost::new(scene.clone(), physics.clone(), input.clone());
        scripts.set_base_scene(scene_src);

        if script_path.exists() {
            if let Err(e) = scripts.load_file(&script_path) {
                log::error!("脚本载入失败: {e}");
            }
        } else {
            log::warn!("脚本 {} 不存在 (script not found)，跳过", script_path.display());
        }

        // 资产节点：按资产内节点路径 spawn，重名自动加 `#N` 后缀
        let mut asset_nodes: Vec<NodeId> = Vec::new();
        if !assets.is_empty() {
            let mut scene_mut = scene.borrow_mut();
            let spawned = spawn_asset_nodes(&mut scene_mut, &assets, &asset_specs, &mut asset_nodes);
            log::info!("资产节点已 spawn: {spawned} 个");
            scene_mut.update_world_transforms();
        }

        let scene_mtime = std::fs::metadata(&scene_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let gilrs = match gilrs::Gilrs::new() {
            Ok(g) => {
                log::info!("手柄子系统就绪 (gamepad ready)");
                Some(g)
            }
            Err(e) => {
                log::warn!("手柄子系统初始化失败（{e}），手柄输入不可用");
                None
            }
        };
        let now = Instant::now();
        Self {
            scene_path,
            script_path,
            window: None,
            renderer: None,
            scene,
            physics,
            scripts,
            input,
            gilrs,
            cursor_valid: false,
            last_frame: now,
            last_watch: now,
            time: 0.0,
            frame_index: 0,
            paused: false,
            keys: HashSet::new(),
            last_dt_ms: 0.0,
            console: spawn_console(),
            scene_mtime,
            physics_gen: 0,
            assets,
            asset_nodes,
        }
    }

    /// 处理终端输入：一行 Rhai 代码，立即作用于当前场景。
    fn pump_console(&mut self) {
        while let Ok(line) = self.console.try_recv() {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if !self.paused {
                self.paused = true;
                log::info!("收到控制台指令，已暂停脚本驱动（空格恢复）");
            }
            match self.scripts.eval(&line) {
                Ok(value) => {
                    if value.is_unit() {
                        println!("[repl] ok");
                    } else {
                        println!("[repl] {value}");
                    }
                }
                Err(e) => println!("[repl] 执行失败: {e}"),
            }
        }
    }

    fn step_and_render(&mut self) {
        self.pump_console();
        self.poll_gamepad();

        let now = Instant::now();
        let mut dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        dt = dt.clamp(0.0, 0.1);
        self.last_dt_ms = dt * 1000.0;

        if !self.paused {
            self.time += dt as f64;
            self.frame_index += 1;

            // 1) 脚本：可能生成 / 销毁 / 修改物件
            self.scripts.tick(dt as f64, self.time, self.frame_index);

            // 2) 键盘驱动的相机移动
            self.apply_camera_input(dt);

            {
                let mut scene = self.scene.borrow_mut();
                scene.time = self.time as f32;
                scene.frame = self.frame_index;
            }

            // 3) 物理：先把场景里声明了 physics 的物件绑成刚体，再推进模拟
            self.sync_physics_bindings();
            {
                let mut physics = self.physics.borrow_mut();
                let mut scene = self.scene.borrow_mut();
                physics.step(dt, &mut scene);
            }

            // 4) 重算世界矩阵
            self.scene.borrow_mut().update_world_transforms();
        }

        if let (Some(renderer), Some(window)) = (self.renderer.as_mut(), self.window.as_ref()) {
            renderer.render(&self.scene.borrow());
            let stats = renderer.stats();
            let pad_tag = {
                let i = self.input.borrow();
                if i.pad.connected {
                    format!("手柄 {}", i.pad.name)
                } else {
                    "手柄未连接".to_string()
                }
            };
            let phys_tag = {
                let p = self.physics.borrow();
                if p.enabled() {
                    format!("物理 {} / {} 刚体", p.kind().name(), p.body_count())
                } else {
                    "物理关".to_string()
                }
            };
            window.set_title(&format!(
                "r3e — Rust 3D Engine  |  {} 物件 / {} draw / {:.1} ms  |  {}  |  {}  |  {}",
                stats.instances,
                stats.draw_calls,
                self.last_dt_ms,
                if self.paused { "已暂停" } else { "运行中" },
                pad_tag,
                phys_tag
            ));
        }

        // 帧末清掉边沿与增量：下一帧的 pressed / released 只反映新事件
        self.input.borrow_mut().end_frame();
    }

    /// 把场景里声明了 `physics` 段的物件绑成刚体。
    ///
    /// 物理开关 / 后端切换 / 刚体表清空都会让世代号自增，这里只在世代变化时重绑一次，
    /// 因此脚本用 `attach_body` 手动绑的刚体不会被覆盖。
    fn sync_physics_bindings(&mut self) {
        let (gen, enabled) = {
            let p = self.physics.borrow();
            (p.generation(), p.enabled())
        };
        if !enabled || gen == self.physics_gen {
            return;
        }
        self.physics_gen = gen;

        let mut scene = self.scene.borrow_mut();
        let ids: Vec<NodeId> = scene
            .iter()
            .filter(|n| n.physics.is_some())
            .map(|n| n.id)
            .collect();
        if ids.is_empty() {
            return;
        }

        let mut physics = self.physics.borrow_mut();
        let mut bound = 0usize;
        for id in ids {
            let Some(desc) = scene.get(id).and_then(BodyDesc::from_node) else {
                continue;
            };
            if let Some(handle) = physics.try_create_body(id, desc) {
                if let Some(node) = scene.get_mut(id) {
                    node.body = Some(handle.0);
                }
                bound += 1;
            }
        }
        if bound > 0 {
            log::info!("物理已绑定 {bound} 个场景刚体");
        }
    }

    fn apply_camera_input(&mut self, dt: f32) {
        // 手柄摇杆 / 扳机快照（死区在此生效）
        let (pad_connected, lx, ly, rx, ry, zoom) = {
            let i = self.input.borrow();
            (
                i.pad.connected,
                i.pad.axis_deadzone(PAD_LEFT_X),
                i.pad.axis_deadzone(PAD_LEFT_Y),
                i.pad.axis_deadzone(PAD_RIGHT_X),
                i.pad.axis_deadzone(PAD_RIGHT_Y),
                i.pad.axis(PAD_RT) - i.pad.axis(PAD_LT),
            )
        };
        let pad_move = pad_connected && (lx != 0.0 || ly != 0.0);
        if self.keys.is_empty() && !pad_move && zoom == 0.0 && rx == 0.0 && ry == 0.0 {
            return;
        }

        let mut scene = self.scene.borrow_mut();
        let camera = &mut scene.camera;

        // 手柄右摇杆：环绕转视角
        if rx != 0.0 || ry != 0.0 {
            camera.orbit(-rx * PAD_ORBIT_SPEED * dt, ry * PAD_ORBIT_SPEED * dt);
        }

        // 手柄扳机：RT 拉近 / LT 推远
        if zoom != 0.0 {
            let offset = camera.position - camera.target;
            let len = (offset.length() * (1.0 - zoom * PAD_ZOOM_SPEED * dt)).clamp(0.8, 80.0);
            camera.position = camera.target + offset.normalize_or_zero() * len;
        }

        let forward = (camera.target - camera.position).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let mut delta = Vec3::ZERO;

        for key in &self.keys {
            match key {
                Key::Character(c) => match c.to_lowercase().as_str() {
                    "w" => delta += forward,
                    "s" => delta -= forward,
                    "d" => delta += right,
                    "a" => delta -= right,
                    "e" => delta += Vec3::Y,
                    "q" => delta -= Vec3::Y,
                    _ => {}
                },
                Key::Named(NamedKey::ArrowLeft) => delta -= right,
                Key::Named(NamedKey::ArrowRight) => delta += right,
                Key::Named(NamedKey::ArrowUp) => delta += forward,
                Key::Named(NamedKey::ArrowDown) => delta -= forward,
                _ => {}
            }
        }

        // 左摇杆：模拟量直接叠加（不归一化，保留推杆幅度）
        let move_vec = delta.normalize_or_zero() + right * lx + forward * ly;
        if move_vec.length_squared() > 1e-6 {
            let mag = move_vec.length().min(1.0);
            let step = move_vec.normalize() * MOVE_SPEED * mag * dt;
            camera.position += step;
            camera.target += step;
        }
    }

    /// 轮询手柄：先排空事件队列，再抓取当前状态快照写入输入层。
    fn poll_gamepad(&mut self) {
        let mut snapshot: Option<(String, [bool; PAD_BUTTON_COUNT], [f32; PAD_AXIS_COUNT])> = None;

        if let Some(gilrs) = self.gilrs.as_mut() {
            // 消费事件以刷新 gilrs 内部状态
            while gilrs.next_event().is_some() {}
            for (_id, pad) in gilrs.gamepads() {
                if !pad.is_connected() {
                    continue;
                }
                let mut buttons = [false; PAD_BUTTON_COUNT];
                for (i, mapped) in PAD_BUTTON_MAP.iter().enumerate() {
                    if let Some(b) = mapped {
                        buttons[i] = pad.is_pressed(*b);
                    }
                }
                let mut axes = [0.0f32; PAD_AXIS_COUNT];
                for (i, mapped) in PAD_AXIS_MAP.iter().enumerate() {
                    if let Some(a) = mapped {
                        axes[i] = pad.value(*a);
                    }
                }
                snapshot = Some((pad.name().to_string(), buttons, axes));
                break;
            }
        }

        let input = self.input.clone();
        let mut toggle_pause = false;
        let mut reload_script = false;

        {
            let mut st = input.borrow_mut();
            match snapshot {
                Some((name, buttons, axes)) => {
                    let first = !st.pad.connected;
                    st.pad.connected = true;
                    if st.pad.name != name {
                        st.pad.name = name.clone();
                    }
                    for i in 0..PAD_BUTTON_COUNT {
                        st.pad.buttons[i].set(buttons[i]);
                    }
                    st.pad.axes = axes;
                    toggle_pause = st.pad.buttons[PAD_START].pressed;
                    reload_script = st.pad.buttons[PAD_BACK].pressed;
                    drop(st);
                    if first {
                        log::info!("手柄已连接: {name}");
                    }
                }
                None => {
                    if st.pad.connected {
                        st.pad.connected = false;
                        st.pad.name.clear();
                        st.pad.axes = [0.0; PAD_AXIS_COUNT];
                        for b in &mut st.pad.buttons {
                            b.set(false);
                        }
                        log::info!("手柄已断开");
                    }
                }
            }
        }

        if toggle_pause {
            self.paused = !self.paused;
            log::info!(
                "{}（手柄 Start）",
                if self.paused { "已暂停" } else { "继续运行" }
            );
        }
        if reload_script {
            let path = self.script_path.clone();
            match self.scripts.load_file(&path) {
                Ok(()) => log::info!("脚本已重载（手柄 Back）"),
                Err(e) => log::error!("重载失败: {e}"),
            }
        }
    }

    /// 鼠标移动：更新位置 / 位移快照，并直接驱动相机（左键转视角、右键平移）。
    fn handle_cursor_moved(&mut self, x: f64, y: f64) {
        let (dx, dy, left, right) = {
            let mut i = self.input.borrow_mut();
            let dx = if self.cursor_valid {
                x as f32 - i.mouse_x
            } else {
                0.0
            };
            let dy = if self.cursor_valid { y as f32 - i.mouse_y } else { 0.0 };
            i.mouse_x = x as f32;
            i.mouse_y = y as f32;
            i.mouse_dx += dx;
            i.mouse_dy += dy;
            (dx, dy, i.mouse_down(MOUSE_LEFT), i.mouse_down(MOUSE_RIGHT))
        };
        self.cursor_valid = true;

        if !left && !right {
            return;
        }

        let mut scene = self.scene.borrow_mut();
        let cam = &mut scene.camera;
        if left {
            // 向右拖 -> 视角向右转
            cam.orbit(-dx * MOUSE_ORBIT_SENS, dy * MOUSE_ORBIT_SENS);
        }
        if right {
            // 抓取式平移：画面跟随光标
            let forward = (cam.target - cam.position).normalize_or_zero();
            let right_v = forward.cross(Vec3::Y).normalize_or_zero();
            let up_v = right_v.cross(forward).normalize_or_zero();
            let k = (cam.position - cam.target).length().max(0.5) * MOUSE_PAN_SENS;
            let offset = -right_v * dx * k + up_v * dy * k;
            cam.position += offset;
            cam.target += offset;
        }
    }

    fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let idx = match button {
            MouseButton::Left => MOUSE_LEFT,
            MouseButton::Right => MOUSE_RIGHT,
            MouseButton::Middle => MOUSE_MIDDLE,
            MouseButton::Back => MOUSE_BACK,
            MouseButton::Forward => MOUSE_FORWARD,
            MouseButton::Other(_) => return,
        };
        self.input
            .borrow_mut()
            .set_mouse_button(idx, state == ElementState::Pressed);
        log::trace!("鼠标 {button:?} {state:?}");
    }

    fn watch_files(&mut self) {
        if self.last_watch.elapsed().as_millis() < 400 {
            return;
        }
        self.last_watch = Instant::now();

        // 1) 场景文件变化 -> 重放场景并重跑脚本
        let scene_mtime = std::fs::metadata(&self.scene_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if scene_mtime.is_some() && scene_mtime != self.scene_mtime {
            self.scene_mtime = scene_mtime;
            match std::fs::read_to_string(&self.scene_path) {
                Ok(json) => match self.scripts.rebuild_scene(&json) {
                    Ok(()) => log::info!("场景文件已热重载 (scene hot-reloaded): {}", self.scene_path.display()),
                    Err(e) => log::error!("场景重载失败: {e}"),
                },
                Err(e) => log::error!("读取场景失败: {e}"),
            }
            return;
        }

        // 2) 脚本文件变化 -> 重放场景并重跑脚本
        if self.scripts.reload_if_changed() {
            log::info!("脚本已热重载 (script hot-reloaded)，场景已重新生成");
        }
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, key: Key, state: ElementState) {
        if state == ElementState::Released {
            self.keys.remove(&key);
            return;
        }

        match &key {
            Key::Named(NamedKey::Escape) => {
                event_loop.exit();
                return;
            }
            Key::Named(NamedKey::Space) => {
                self.paused = !self.paused;
                log::info!("{}", if self.paused { "已暂停" } else { "继续运行" });
                self.keys.remove(&key);
                return;
            }
            Key::Named(NamedKey::F5) => {
                let path = self.script_path.clone();
                if let Err(e) = self.scripts.load_file(&path) {
                    log::error!("重载失败: {e}");
                }
                return;
            }
            _ => {}
        }
        self.keys.insert(key);
    }
}

impl ApplicationHandler for EngineApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("r3e — Rust 3D Engine")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("创建窗口失败: {e}");
                event_loop.exit();
                return;
            }
        };

        match Renderer::new(window.clone()) {
            Ok(mut renderer) => {
                log::info!("渲染器就绪: {}x{}", renderer.size().0, renderer.size().1);
                if !self.assets.is_empty() {
                    renderer.set_asset_library(self.assets.clone());
                    log::info!(
                        "渲染层已接入资产库: {} 个资产 / {} 个资产节点",
                        self.assets.len(),
                        self.asset_nodes.len()
                    );
                }
                self.renderer = Some(renderer);
            }
            Err(e) => {
                log::error!("渲染器初始化失败: {e}");
                event_loop.exit();
                return;
            }
        }

        self.last_frame = Instant::now();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event_loop, event.logical_key, event.state);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::Focused(false) => {
                // 失焦后收不到松开事件，主动清掉持续按下状态，避免粘键
                self.keys.clear();
                self.input.borrow_mut().reset_held();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 60.0,
                };
                self.input.borrow_mut().scroll += amount;
                let mut scene = self.scene.borrow_mut();
                let cam = &mut scene.camera;
                let offset = cam.position - cam.target;
                let len = (offset.length() - amount * 0.6).clamp(0.8, 80.0);
                cam.position = cam.target + offset.normalize_or_zero() * len;
            }
            WindowEvent::RedrawRequested => {
                self.watch_files();
                self.step_and_render();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// 后台线程读取 stdin，把每一行送到主循环执行。
fn spawn_console() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("r3e-console".into())
        .spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .ok();
    rx
}

/// 一条待导入的资产：路径 + 可选整体变换 + 是否 spawn 成物件。
///
/// 路径同时用作 `AssetId` 与 `asset:<路径>#<序号>` 引用键（`AssetId::from_path`
/// 内部会做小写与正斜杠归一化，因此两种写法指向同一资产）。
struct AssetSpec {
    path: PathBuf,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
    spawn: bool,
}

/// 合并命令行 `--asset` 与场景文件 `assets` 段。
///
/// 命令行排在最前，便于临时替换 / 追加；场景内相对路径按场景文件目录解析。
fn collect_asset_specs(scene: &Scene, scene_path: &Path, cli: &[PathBuf]) -> Vec<AssetSpec> {
    let base_dir = scene_path.parent().unwrap_or_else(|| Path::new("."));
    let mut specs: Vec<AssetSpec> = cli
        .iter()
        .map(|path| AssetSpec {
            path: path.clone(),
            position: None,
            rotation: None,
            scale: None,
            spawn: true,
        })
        .collect();

    for decl in &scene.assets {
        let raw = decl.path.trim();
        if raw.is_empty() {
            log::warn!("场景 assets 段存在空 path，已跳过");
            continue;
        }
        let mut path = PathBuf::from(raw);
        if path.is_relative() {
            path = base_dir.join(path);
        }
        specs.push(AssetSpec {
            path,
            position: decl.position,
            rotation: decl.rotation,
            scale: decl.scale,
            spawn: decl.spawn.unwrap_or(true),
        });
    }
    specs
}

/// 按顺序导入资产；导入失败只记日志，不影响其余资产与启动流程。
///
/// 同一路径可能被命令行与多个场景声明同时引用（例如只想换 transform、或 `spawn: false`
/// 仅供脚本引用），这里按 `AssetId` 去重，避免重复解析同一个 FBX。
fn load_assets(specs: &[AssetSpec]) -> AssetLibrary {
    let mut library = AssetLibrary::new();
    for spec in specs {
        let key = spec.path.display().to_string();
        let id = r3e_core::AssetId::from_path(&key);
        if library.get(id).is_some() {
            log::debug!("资产已导入过，跳过重复加载: {key}");
            continue;
        }
        match library.load_fbx(&spec.path) {
            Ok(id) => {
                let (meshes, tris) = library
                    .get(id)
                    .map(|e| (e.meshes.len(), e.triangle_total()))
                    .unwrap_or((0, 0));
                log::info!(
                    "资产已导入: {} (id {}) — {} 个网格 / {} 三角形",
                    spec.path.display(),
                    id,
                    meshes,
                    tris
                );
            }
            Err(e) => log::error!("资产导入失败 {}: {e}", spec.path.display()),
        }
    }
    library
}

/// 按资产内节点路径把一个 FBX 的每个网格 spawn 成场景物件。
///
/// * 物件名默认取网格所属节点的路径（如 `RootNode/Arm/Hand`）；
/// * 与已有物件或同批重名时追加 `#1`、`#2` 后缀；
/// * `mesh` 字段写成 `asset:<路径>#<序号>`，渲染层据此从资产库取数据；
/// * 顶点已是 `geometry_to_world` 后的世界空间坐标，因此默认不加变换；
///   场景 `assets` 段给了 position/rotation/scale 时会作为整体变换施加。
fn spawn_asset_nodes(
    scene: &mut Scene,
    library: &AssetLibrary,
    specs: &[AssetSpec],
    out: &mut Vec<NodeId>,
) -> usize {
    let mut spawned = 0usize;
    for spec in specs {
        if !spec.spawn {
            continue;
        }
        let path_str = spec.path.display().to_string();
        let id = r3e_core::AssetId::from_path(&path_str);
        let Some(entry) = library.get(id) else {
            continue;
        };
        for (index, mesh) in entry.meshes.iter().enumerate() {
            let base = if mesh.node_path.trim().is_empty() {
                if mesh.name.trim().is_empty() {
                    format!("asset_{index}")
                } else {
                    mesh.name.clone()
                }
            } else {
                mesh.node_path.clone()
            };

            let mut name = base.clone();
            let mut suffix = 0u32;
            while scene.find_id(&name).is_some() {
                suffix += 1;
                name = format!("{base}#{suffix}");
            }

            // P2 起颜色由材质 bind group 负责（贴图 + 材质因子），节点色保持白色，
            // 否则会和材质 base_color 相乘造成二次压暗。
            let color = [1.0, 1.0, 1.0, 1.0];

            let desc = ObjectDesc {
                name: name.clone(),
                mesh: Some(format!("asset:{}#{}", path_str, index)),
                position: spec.position,
                rotation: spec.rotation,
                scale: spec.scale,
                color: Some(color),
                parent: None,
                visible: Some(true),
                physics: None,
            };
            match scene.spawn(desc) {
                Ok(node_id) => {
                    out.push(node_id);
                    spawned += 1;
                }
                Err(e) => log::warn!("资产节点 `{name}` spawn 失败: {e}"),
            }
        }
    }
    spawned
}
