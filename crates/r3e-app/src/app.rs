//! 应用层：窗口、事件循环、主循环、终端指令与脚本热重载。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Instant, SystemTime};

use glam::Vec3;
use r3e_core::Scene;
use r3e_physics::Physics;
use r3e_render::Renderer;
use r3e_script::ScriptHost;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// 场景文件缺失时的兜底场景，保证引擎任何情况下都能启动。
const FALLBACK_SCENE: &str = include_str!("../../../assets/scene.json");

const MOVE_SPEED: f32 = 6.0;

pub struct EngineApp {
    scene_path: PathBuf,
    script_path: PathBuf,

    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,

    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    scripts: ScriptHost,

    last_frame: Instant,
    last_watch: Instant,
    time: f64,
    frame_index: u64,
    paused: bool,
    keys: HashSet<Key>,
    last_dt_ms: f32,
    console: mpsc::Receiver<String>,
    scene_mtime: Option<SystemTime>,
}

impl EngineApp {
    pub fn new(scene_path: PathBuf, script_path: PathBuf) -> Self {
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

        let scene = Rc::new(RefCell::new(scene));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let mut scripts = ScriptHost::new(scene.clone(), physics.clone());
        scripts.set_base_scene(scene_src);

        if script_path.exists() {
            if let Err(e) = scripts.load_file(&script_path) {
                log::error!("脚本载入失败: {e}");
            }
        } else {
            log::warn!("脚本 {} 不存在 (script not found)，跳过", script_path.display());
        }

        let scene_mtime = std::fs::metadata(&scene_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let now = Instant::now();
        Self {
            scene_path,
            script_path,
            window: None,
            renderer: None,
            scene,
            physics,
            scripts,
            last_frame: now,
            last_watch: now,
            time: 0.0,
            frame_index: 0,
            paused: false,
            keys: HashSet::new(),
            last_dt_ms: 0.0,
            console: spawn_console(),
            scene_mtime,
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

            // 3) 物理（当前 null 后端，空实现；换后端后自动生效）
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
            window.set_title(&format!(
                "r3e — Rust 3D Engine  |  {} 物件 / {} draw / {:.1} ms  |  {}",
                stats.instances,
                stats.draw_calls,
                self.last_dt_ms,
                if self.paused { "已暂停" } else { "运行中" }
            ));
        }
    }

    fn apply_camera_input(&mut self, dt: f32) {
        if self.keys.is_empty() {
            return;
        }
        let mut scene = self.scene.borrow_mut();
        let camera = &mut scene.camera;

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

        if delta.length_squared() > 1e-6 {
            let step = delta.normalize() * MOVE_SPEED * dt;
            camera.position += step;
            camera.target += step;
        }
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
            Ok(renderer) => {
                log::info!("渲染器就绪: {}x{}", renderer.size().0, renderer.size().1);
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
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 60.0,
                };
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
