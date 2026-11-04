//! 应用层：窗口、事件循环、主循环、终端指令与脚本热重载。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Instant, SystemTime};

use gilrs::{Axis, Button};
use glam::{Mat4, Vec3};
use r3e_asset::AssetLibrary;
use r3e_audio::Audio;
use r3e_core::anim::{AnimBinding, AnimInstance};
use r3e_core::input::{
    key_from_char, InputState, KeyCode, MOUSE_BACK, MOUSE_FORWARD, MOUSE_LEFT, MOUSE_MIDDLE,
    MOUSE_RIGHT, PAD_AXIS_COUNT, PAD_BACK, PAD_BUTTON_COUNT, PAD_LEFT_X, PAD_LEFT_Y, PAD_LT,
    PAD_RIGHT_X, PAD_RIGHT_Y, PAD_RT, PAD_START,
};
use r3e_core::mesh::TextureImage;
use r3e_core::{
    AnimDecl, MeshData, MeshProvider, MeshRef, NodeId, ObjectDesc, Scene, SceneArchive,
    SceneCommand, SceneStack, SkinInstance, StackEntry, UiInteraction,
};
use r3e_editor::RpcServer;
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};
use r3e_render::ui::UiImageProvider;
use r3e_render::Renderer;
use r3e_script::ScriptHost;

use crate::editor::LocalEngineApi;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// UI 图片加载器：把「按路径取图」接到 r3e-asset 的解码链路（png / jpg / tga / bmp）。
///
/// 渲染层不碰文件 IO，所有解码都发生在 CPU 侧，失败只记一次（见 `UiPass`）。
struct FsUiImageProvider;

impl UiImageProvider for FsUiImageProvider {
    fn load_ui_image(&self, path: &str) -> Result<TextureImage, String> {
        // 相对路径按工作目录解析；找不到再退到可执行文件同目录（便于发布包分发）。
        let candidate = Path::new(path);
        let resolved = if candidate.is_absolute() || candidate.exists() {
            candidate.to_path_buf()
        } else {
            match std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join(path)))
                .filter(|p| p.exists())
            {
                Some(p) => p,
                None => candidate.to_path_buf(),
            }
        };
        r3e_asset::load_texture_image(&resolved)
            .ok_or_else(|| format!("无法解码 UI 图片 `{}`", resolved.display()))
    }
}

/// 场景文件缺失时的兜底场景，保证引擎任何情况下都能启动。
const FALLBACK_SCENE: &str = include_str!("../../../assets/scene.json");

const MOVE_SPEED: f32 = 6.0;
/// 鼠标拖拽转视角的灵敏度（度 / 像素）。
const MOUSE_ORBIT_SENS: f32 = 0.28;
/// 鼠标右键拖拽平移的灵敏度系数（随相机距离缩放）。
const MOUSE_PAN_SENS: f32 = 0.0016;
/// 一帧内与相机移动相关的按键快照。
///
/// 键盘状态存在 `InputState::keyboard` 里（脚本与主循环共享同一份真源），
/// 这里只在一帧开头取一次快照，避免相机计算过程中反复 borrow。
#[derive(Debug, Clone, Copy, Default)]
struct MoveKeys {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

impl MoveKeys {
    fn is_empty(self) -> bool {
        !(self.forward || self.back || self.left || self.right || self.up || self.down)
    }
}

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
    /// `--ccd on|off`：逐刚体 CCD 的缺省值（场景里逐物件的 `physics.ccd` 仍优先）。
    pub ccd: Option<bool>,
    /// `--asset <fbx>`（可重复）：启动时导入并 spawn 的静态资产。
    pub assets: Vec<PathBuf>,
    /// `--assets-root <dir>`：场景 `assets` 段内相对路径的解析根目录。
    ///
    /// 不指定时按场景文件所在目录解析（历史行为）；指定后资产可放在与场景
    /// 文件无关的目录里，便于场景文件随手换位置、公共资产集中管理。
    pub assets_root: Option<PathBuf>,
    /// `--dump-asset <fbx>`：只做导入并打印报告，不创建窗口。
    pub dump_asset: Option<PathBuf>,
    /// `--dump-anim <fbx>`：打印动画栈的采样表后退出（P3 的数值验收手段）。
    pub dump_anim: Option<PathBuf>,
    /// `--save-scene <json>`：读入场景后按引擎写法保存到该路径并退出（无窗口）。
    pub save_scene: Option<PathBuf>,
    /// `--anim-clip <name>`：配合 `--dump-anim` 只看某个片段。
    pub anim_clip: Option<String>,
    /// `--show-bones`：把骨架节点画成小方块，用于肉眼确认 P3 的节点动画。
    pub show_bones: bool,
    /// `--run-seconds <s>`：窗口运行指定秒数后自动退出（无人值守验证用）。
    pub run_seconds: Option<f32>,
    /// `--font <path>`（可重复）：覆盖文本通道的字体集，第一个文件优先匹配字形。
    ///
    /// 不指定时渲染层自动按平台常见路径找中文字体（`msyh.ttc` 等），
    /// 所以默认就能画中文；这里用于明确指定字形更好看的字体或图标字体。
    pub fonts: Vec<PathBuf>,
    /// `--rpc-port <port>`：编辑器桥监听端口（0 = 由系统分配）；`None` = 不启动桥。
    pub rpc_port: Option<u16>,
    /// `--rpc-token <token>`：握手令牌，客户端必须原样带回（缺省不校验）。
    pub rpc_token: Option<String>,
    /// `--rpc-max-per-frame <n>`：每帧最多消化的编辑器请求数（默认 8）。
    pub rpc_max_per_frame: Option<usize>,
}

impl CliOptions {
    /// 转成物理配置。后端名非法时只告警、不阻断启动。
    pub fn to_config(&self) -> PhysicsConfig {
        let kind = self.backend.as_deref().and_then(|name| {
            let parsed = PhysicsKind::parse(name);
            if parsed.is_none() {
                log::warn!(
                    "未知物理后端 `{name}`，当前构建可用：{}",
                    PhysicsKind::available_names().join(" / ")
                );
            }
            parsed
        });
        PhysicsConfig {
            enabled: self.physics,
            kind,
            gravity: self.gravity.map(Vec3::from),
            ground: self.ground,
            ccd: self.ccd,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.physics.is_none()
            && self.backend.is_none()
            && self.gravity.is_none()
            && self.ground.is_none()
            && self.ccd.is_none()
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
    /// 音频门面（非空间化）。无设备时内部已是静默后端，这里是同一份句柄的 app 侧引用，
    /// 主循环每帧驱动 `update()` 回收播完的实例。
    audio: Rc<RefCell<Audio>>,
    /// 输入快照：app 层写入，脚本层读取。
    input: Rc<RefCell<InputState>>,
    /// 手柄子系统；初始化失败时为 None（引擎其余功能不受影响）。
    gilrs: Option<gilrs::Gilrs>,
    /// 光标是否已有有效位置（首个 CursorMoved 之前不算位移）。
    cursor_valid: bool,
    /// UI 指针交互状态（与脚本宿主共享同一份 `Rc`）。
    ///
    /// 主循环每帧推进它，脚本侧 `ui_events()` / `ui_hover()` / `ui_hit()` 读的就是这份；
    /// 相机拖动也靠它判断「这一下点的是 UI 还是场景」。
    ui_interaction: Rc<RefCell<UiInteraction>>,
    /// 场景栈（扩展十八 B）：与脚本宿主共享同一份 `Rc`。
    ///
    /// 归属很明确——**宿主独占写**：压栈 / 回退 / 读档都由 [`EngineApp::apply_scene_command`]
    /// 在帧首做，脚本与 RPC 只能往命令槽里放一条意图。栈里每层存"离开时的样子"
    /// （源文本 + 运行时快照），所以切走再切回来不必重跑整局。
    scene_stack: Rc<RefCell<SceneStack>>,

    last_frame: Instant,
    last_watch: Instant,
    time: f64,
    frame_index: u64,
    paused: bool,
    last_dt_ms: f32,
    console: mpsc::Receiver<String>,
    scene_mtime: Option<SystemTime>,
    /// 上次完成「场景声明刚体」绑定的物理世代号。
    physics_gen: u64,
    /// 已导入的外部资产（渲染层通过 `Renderer::set_asset_library` 取得只读引用）。
    ///
    /// 用 `Rc<RefCell<_>>` 而非裸 `Arc`：场景热重载可能改 `assets` 段，需要就地换库，
    /// 同时让场景重建回调看到的是**换库之后**的同一份资产。
    assets: Rc<RefCell<Arc<AssetLibrary>>>,
    /// 当前生效的资产声明（命令行 + 场景 `assets` 段），热重载时按需重算。
    asset_specs: Rc<RefCell<Vec<AssetSpec>>>,
    /// 命令行 `--asset`（热重载重算资产声明时仍需带上）。
    cli_assets: Vec<PathBuf>,
    /// 命令行 `--font`（M1 文字渲染）：显式指定的文本字体，空表示用系统字体。
    cli_fonts: Vec<PathBuf>,
    /// `--assets-root`：场景内相对资产路径的解析根；`None` 时用场景文件所在目录。
    assets_root: Option<PathBuf>,
    /// 从资产 spawn 出来的节点（共享给场景重建回调，热重载时同步刷新）。
    asset_nodes: Rc<RefCell<Vec<NodeId>>>,
    /// `--run-seconds`：自动退出的秒数；`None` 表示一直运行。
    run_seconds: Option<f32>,
    /// 已运行的真实时长（秒），用于 `--run-seconds` 判定。
    run_elapsed: f32,
    /// 是否已打印过起始帧的动画/骨骼探针。
    probe_logged: bool,
    /// 起始帧的骨骼节点世界位置快照，用于在收尾时给出「动了多少」的数值。
    probe_snapshot: Vec<(String, Vec3)>,
    /// 蒙皮质心底账（名称 → 质心），与 `probe_snapshot` 同理用于算位移。
    probe_skin_snapshot: Vec<(String, Vec3)>,
    /// 请求在本次事件处理结束后退出事件循环。
    exit_pending: bool,
    /// 编辑器 RPC 服务端；`--rpc-port` 未指定（或绑定失败）时为 `None`。
    rpc: Option<RpcServer>,
    /// 每帧最多消化的编辑器请求数，避免编辑器洪泛把一帧拖成几百毫秒。
    rpc_max_per_frame: usize,
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
        // 场景里的相对路径按 `--assets-root` 解析，未指定时退回场景文件所在目录；
        // 导入失败不阻断启动。
        let asset_specs =
            collect_asset_specs(&scene, &scene_path, cli.assets_root.as_deref(), &cli.assets);
        let asset_specs = Rc::new(RefCell::new(asset_specs));
        let assets: Rc<RefCell<Arc<AssetLibrary>>> =
            Rc::new(RefCell::new(Arc::new(load_assets(&asset_specs.borrow()))));

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
            "物理: {} / 后端 {} / 能力 [{}] / CCD 默认 {} / 重力 {:?} / 地面 {}",
            if physics.enabled() {
                "启用"
            } else {
                "关闭"
            },
            physics.kind().name(),
            physics.capabilities().summary(),
            if physics.capabilities().ccd {
                if physics.ccd_default() {
                    "on"
                } else {
                    "off"
                }
            } else {
                "不支持"
            },
            physics.gravity.to_array(),
            physics.ground()
        );

        let scene = Rc::new(RefCell::new(scene));
        let physics = Rc::new(RefCell::new(physics));
        let input = Rc::new(RefCell::new(InputState::default()));

        // 音频：只有 app 层碰设备。拿不到设备就静默降级（Audio 内部已兜底），
        // 引擎不会因为没声卡 / 驱动异常起不来。
        let audio = Rc::new(RefCell::new(Audio::open_default()));
        {
            let mut a = audio.borrow_mut();
            log::info!("音频: {}", a.status_line());
        }

        let mut scripts = ScriptHost::new_with_audio(
            scene.clone(),
            physics.clone(),
            input.clone(),
            audio.clone(),
        );

        // UI 指针交互与脚本宿主共用同一份状态：app 推进、脚本读。
        let ui_interaction = scripts.ui_interaction();
        scripts.set_base_scene(scene_src.clone());

        // 场景栈与脚本宿主共用同一份状态：脚本只读层名，写只发生在宿主侧。
        let scene_stack = scripts.scene_stack_handle();
        {
            // 起始场景就是第 0 层：这样 `scene_back()` 在"还没切过"时有确定的语义
            // （没有更早的层 → 拒绝），而不是"栈为空"这种要和 0 层区分开的特例。
            let mut stack = scene_stack.borrow_mut();
            let entry = StackEntry::capture(
                scene.borrow().name.clone(),
                scene_path.display().to_string(),
                &scene.borrow(),
            );
            stack.push(entry);
        }

        // 资产节点：按资产内节点路径 spawn，重名自动加 `#N` 后缀
        let asset_nodes: Rc<RefCell<Vec<NodeId>>> = Rc::new(RefCell::new(Vec::new()));
        if !assets.borrow().is_empty() {
            let mut scene_mut = scene.borrow_mut();
            let spawned = spawn_asset_nodes(
                &mut scene_mut,
                &assets.borrow(),
                &asset_specs.borrow(),
                &mut asset_nodes.borrow_mut(),
                cli.show_bones,
            );
            log::info!("资产节点已 spawn: {spawned} 个");
            scene_mut.update_world_transforms();
        }

        // 场景热重载 / F5 会把场景重放成纯净 JSON，资产节点与动画实例需要补挂回去
        {
            let hook_nodes = asset_nodes.clone();
            let hook_assets = assets.clone();
            let hook_specs = asset_specs.clone();
            let show_bones = cli.show_bones;
            scripts.set_rebuild_hook(move |scene| {
                // 每次都取当前生效的库与声明：热重载换过库之后，这里必须看到新的那份
                let library = hook_assets.borrow().clone();
                let specs = hook_specs.borrow().clone();
                let mut sink = hook_nodes.borrow_mut();
                sink.clear();
                let spawned = spawn_asset_nodes(scene, &library, &specs, &mut sink, show_bones);
                if spawned > 0 {
                    log::info!("资产节点已随场景重建补挂: {spawned} 个");
                }
            });
        }

        // 脚本必须在资产（含动画实例）就绪之后再跑：`on_start` 里常要枚举 `anims()`
        if script_path.exists() {
            if let Err(e) = scripts.load_file(&script_path) {
                log::error!("脚本载入失败: {e}");
            }
        } else {
            log::warn!(
                "脚本 {} 不存在 (script not found)，跳过",
                script_path.display()
            );
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

        // 编辑器桥：只在显式给了 `--rpc-port` 时启动。绑定失败不阻断启动——
        // 端口被占是常见的开发期状况，引擎本身不应该因此起不来。
        let rpc = match cli.rpc_port {
            Some(port) => match RpcServer::bind(port, cli.rpc_token.clone()) {
                Ok(server) => {
                    log::info!(
                        "编辑器桥已监听 {}（令牌：{}）",
                        server.local_addr(),
                        if cli.rpc_token.is_some() {
                            "已启用"
                        } else {
                            "未设置"
                        }
                    );
                    Some(server)
                }
                Err(e) => {
                    log::error!("编辑器桥监听 127.0.0.1:{port} 失败（{e}），本次运行不提供 RPC");
                    None
                }
            },
            None => None,
        };

        Self {
            scene_path,
            script_path,
            window: None,
            renderer: None,
            scene,
            physics,
            scripts,
            audio,
            input,
            gilrs,
            cursor_valid: false,
            ui_interaction,
            scene_stack,
            last_frame: now,
            last_watch: now,
            time: 0.0,
            frame_index: 0,
            paused: false,
            last_dt_ms: 0.0,
            console: spawn_console(),
            scene_mtime,
            physics_gen: 0,
            assets,
            asset_specs,
            cli_assets: cli.assets.clone(),
            cli_fonts: cli.fonts.clone(),
            assets_root: cli.assets_root.clone(),
            asset_nodes,
            run_seconds: cli.run_seconds,
            run_elapsed: 0.0,
            probe_logged: false,
            probe_snapshot: Vec::new(),
            probe_skin_snapshot: Vec::new(),
            exit_pending: false,
            rpc,
            rpc_max_per_frame: cli.rpc_max_per_frame.unwrap_or(8).max(1),
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

    /// 消化本帧待处理的编辑器请求：一条请求 = 一次 `dispatch`，结果由连接线程回写。
    ///
    /// 每帧最多 `rpc_max_per_frame` 条，避免编辑器的高频请求把一帧拖成几百毫秒；
    /// 剩下的留在服务端有界队列里，溢出时 RPC 线程会明确回 `BUSY` 而不是无限堆内存。
    fn drain_editor_jobs(&mut self) {
        let Some(server) = self.rpc.as_ref() else {
            return;
        };
        let fps = if self.last_dt_ms > 0.0 {
            1000.0 / self.last_dt_ms
        } else {
            0.0
        };
        for _ in 0..self.rpc_max_per_frame {
            let Some(job) = server.try_recv() else { break };
            let mut api = LocalEngineApi {
                scene: &self.scene,
                physics: &self.physics,
                scripts: &mut self.scripts,
                scene_label: self.scene_path.display().to_string(),
                script_label: self.script_path.display().to_string(),
                frames: self.frame_index,
                fps,
                uptime_ms: (self.time * 1000.0).round() as u64,
                paused: self.paused,
            };
            job.run(&mut api);
        }
    }

    fn step_and_render(&mut self) {
        self.pump_console();
        self.drain_editor_jobs();
        // 编辑器 / 脚本排队的场景命令在这里落地：本帧脚本 tick 之前，
        // 保证 `on_update` 看到的是切完之后的世界，而不是切一半的。
        self.apply_pending_scene_command();
        self.poll_gamepad();

        let now = Instant::now();
        let mut dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        dt = dt.clamp(0.0, 0.1);
        self.last_dt_ms = dt * 1000.0;

        // 0) UI 指针交互：命中测试 / hover / 按下捕获在本帧输入清掉之前算完，
        //    这样脚本 on_update 读到的就是本帧事件，相机也据此决定要不要吃这次拖动。
        //    放在暂停判断之外：暂停时点 UI（按钮切换状态）仍然要有反馈。
        self.update_ui_interaction();

        if !self.paused {
            self.time += dt as f64;
            self.frame_index += 1;

            // 1) 脚本：可能生成 / 销毁 / 修改物件
            let fps = if self.last_dt_ms > 0.0 {
                1000.0 / self.last_dt_ms as f64
            } else {
                0.0
            };
            self.scripts
                .tick(dt as f64, self.time, self.frame_index, fps);

            // 2) 键盘驱动的相机移动
            self.apply_camera_input(dt);

            {
                let mut scene = self.scene.borrow_mut();
                scene.time = self.time as f32;
                scene.frame = self.frame_index;
            }

            // 2.5) 动画：先推进播放器并把局部 TRS 写回节点，再进行物理与世界矩阵
            // （顺序与 docs/ASSETS.md 4.4 一致：脚本之后、物理之前）
            self.scene.borrow_mut().update_animations(dt);

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

        // 音频回收：播完的实例在这里清账。放在暂停分支之外——暂停时也要回收，
        // 否则停掉的声音会一直占着「在播」名额。
        {
            let reaped = self.audio.borrow_mut().update();
            if !reaped.is_empty() {
                log::debug!("音频实例播完回收: {:?}", reaped);
            }
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
            let audio_tag = {
                let a = self.audio.borrow();
                if a.device_available() {
                    format!("音频 {} 实例", a.active_count())
                } else {
                    "音频静默".to_string()
                }
            };
            window.set_title(&format!(
                "r3e — Rust 3D Engine  |  {} 物件 / {} draw / {:.1} ms  |  {}  |  {}  |  {}  |  {}",
                stats.instances,
                stats.draw_calls,
                self.last_dt_ms,
                if self.paused {
                    "已暂停"
                } else {
                    "运行中"
                },
                pad_tag,
                phys_tag,
                audio_tag
            ));
        }

        // 无人值守验证：跑到第 30 帧打印一次探针，到达 --run-seconds 后打印收尾探针并退出
        if !self.probe_logged && self.frame_index >= 30 {
            self.probe_logged = true;
            self.log_anim_probe("起始");
            self.log_cull_probe("起始");
        }
        if let Some(secs) = self.run_seconds {
            self.run_elapsed += self.last_dt_ms / 1000.0;
            if self.run_elapsed >= secs {
                log::info!(
                    "已运行 {:.2}s（--run-seconds {:.2}），打印收尾探针后退出",
                    self.run_elapsed,
                    secs
                );
                self.log_anim_probe("收尾");
                self.log_cull_probe("收尾");
                self.exit_pending = true;
            }
        }

        // 帧末清掉边沿与增量：下一帧的 pressed / released 只反映新事件
        self.input.borrow_mut().end_frame();
    }

    /// 剔除探针：一帧里有多少物件参与渲染、多少被视锥剔掉。
    ///
    /// `缺包围盒` 持续非零说明资产层没喂 AABB（剔除会 fail-open 全画），是排查剔除失效的第一入口。
    fn log_cull_probe(&self, tag: &str) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let stats = renderer.stats();
        let visible = self.scene.borrow().iter().filter(|n| n.visible).count();
        log::info!(
            "[{tag}] 视锥剔除：可见物件 {visible} / 绘制实例 {} / 剔除 {} / 缺包围盒 {}",
            stats.instances,
            stats.culled,
            stats.cull_unknown
        );
        // 环境探针：enabled 为 false 表示退回 light.ambient 常数项；
        // builds 是球谐重投影次数（缓存命中时应远小于帧数），用于确认没有每帧重算。
        let (env_enabled, env_intensity, env_sun) = match self.scene.borrow().environment {
            Some(env) => (true, env.intensity, env.sun_intensity),
            None => (false, 0.0, 0.0),
        };
        log::info!(
            "[{tag}] 环境探针：启用 {env_enabled} / 总强度 {env_intensity:.2} / 太阳强度 {env_sun:.2} / 球谐重投影 {} 次",
            renderer.environment_builds()
        );
        // 后处理探针（扩展十三）：泛光 pass 数 > 0 说明整条链真的提交了；
        // 层级分辨率应随窗口尺寸变化；重建次数只该在 resize 时增长——
        // 若它逐帧增长，说明尺寸判定失效、每帧都在重建离屏目标。
        let (postfx_present, exposure, tonemap, bloom_on, fxaa_on) =
            match self.scene.borrow().postfx {
                Some(fx) => (
                    true,
                    fx.exposure,
                    fx.tonemap.name(),
                    fx.bloom.enabled,
                    fx.fxaa,
                ),
                None => (false, 0.0, "none(缺省)", false, false),
            };
        {
            let pf = renderer.postfx();
            let pf_stats = pf.stats();
            log::info!(
                "[{tag}] 后处理：postfx 段 {postfx_present} / 曝光 {exposure:+.2}EV / 色调映射 {tonemap} / 泛光 {bloom_on} / FXAA {fxaa_on} / HDR 目标 {:?}×{:?} / 泛光层级 {:?} / pass 泛光 {} + composite {} / 重建 {} 次",
                pf.size().0,
                pf.size().1,
                pf.bloom_sizes(),
                pf_stats.bloom_passes,
                pf_stats.composite_passes,
                pf.rebuilds()
            );
        }
        // 阴影探针：draw call 为 0 = 本帧没跑阴影通道（场景未开启 / 无可见物件），
        // 否则是深度通道里的 draw call 数（按网格合批 × 级数）。
        // `shadow_cascades` 是实际生效的级联数：1 = 单级（等价阶段 D），0 = 关闭。
        log::info!(
            "[{tag}] 阴影通道：级联 {} / draw call {} / 主通道实例 {}",
            stats.shadow_cascades,
            stats.shadow_draws,
            stats.instances
        );
        // 遮蔽探针（扩展十七）：passes 恒为 2（生成 + 散焦），为 0 说明本帧没提交——
        // 段未开、强度为 0，或设置收敛成了中性值。`samples` 打印「场景要的 / 实际生效的」，
        // 两者不等就说明被夹进了 [4, 16]（或场景压根没写这一段）。
        // 世代号只在遮蔽图句柄变化时增长：逐帧增长说明 AO 图在每帧重建，设置判定失效。
        let ssao_setting = match self.scene.borrow().postfx {
            Some(fx) => fx.ssao.sanitized(),
            None => r3e_core::ssao::SsaoSettings::default(),
        };
        log::info!(
            "[{tag}] 遮蔽探针：段内启用 {} / GPU 启用 {} / 样本 {}/{} / pass {} / AO 图 {}×{} / 世代 {} / 半径 {:.2} / 偏置 {:.3} / 强度 {:.2}",
            ssao_setting.enabled,
            stats.ssao.enabled,
            ssao_setting.samples,
            stats.ssao.samples,
            stats.ssao.passes,
            stats.ssao.size.0,
            stats.ssao.size.1,
            renderer.ssao().generation(),
            ssao_setting.radius,
            ssao_setting.bias,
            ssao_setting.intensity
        );
        // 文本探针：glyphs > 0 说明字形真的排版并提交了；missing > 0 说明有字符
        // 当前字体集合里找不到（中文变方框的主因），据此可判断字体是否加载成功。
        let text_nodes = self.scene.borrow().texts.len();
        log::info!(
            "[{tag}] 文本通道：节点 {text_nodes} / 字形 {} / draw call {} / 缺字 {} / 图集 {} 页 占用 {:.1}% / 开关 {}",
            stats.text_glyphs,
            stats.text_draws,
            stats.text_missing,
            renderer.text().atlas_pages(),
            renderer.text().atlas_occupancy() * 100.0,
            self.scene.borrow().text_enabled
        );
        // UI 探针：quads 是真正提交的四边形数（矩形 1 / 边框 4 / 图片 1），
        // draws 是纹理分组后的 draw call 数；ui 非空但 quads 为 0 说明元素全被判为退化（尺寸 <= 0）。
        let (ui_nodes, ui_enabled, ui_above_text) = {
            let scene = self.scene.borrow();
            (scene.ui.len(), scene.ui_enabled, scene.ui_above_text)
        };
        let ui = renderer.ui_stats();
        // 交互探针：hover / active 是本帧指针压住 / 按住的 key，事件数是本帧产出的事件条数。
        // 无鼠标输入的无人值守运行里这三项应为空 / 空 / 0，但 `update_ui_interaction` 确实跑过。
        let (ui_hover, ui_active, ui_events, ui_pointer) = {
            let it = self.ui_interaction.borrow();
            (
                it.hover().unwrap_or("").to_string(),
                it.active().unwrap_or("").to_string(),
                it.events().len(),
                it.pointer_over_ui(),
            )
        };
        log::info!(
            "[{tag}] UI 通道：节点 {ui_nodes} / 绘制元素 {} / 四边形 {} / draw call {} / 贴图 {} / 缺图回退 {} / 超限跳过 {} / 开关 {ui_enabled} / UI 在文本之上 {ui_above_text} / 交互 hover [{ui_hover}] active [{ui_active}] 事件 {ui_events} 指针压 UI {ui_pointer}",
            ui.elements,
            ui.quads,
            ui.draw_calls,
            ui.textures,
            ui.missing_images,
            ui.skipped
        );
        // 局部光源探针（扩展十五）：场景盏数 / 真正上传到 UBO 的盏数 / 总开关。
        // 三项关系能一眼定位问题：场景有灯但上传 0 → 总开关关了；上传数小于场景数
        // → 超过 MAX_LOCAL_LIGHTS 的尾巴被截（core 写入时已拒绝，正常不会发生）。
        let (light_nodes, lights_enabled) = {
            let scene = self.scene.borrow();
            (scene.lights.len(), scene.lights_enabled)
        };
        log::info!(
            "[{tag}] 局部光源：场景 {light_nodes} 盏 / 上传 {} 盏 / 开关 {lights_enabled}",
            stats.local_lights
        );
        // 音频探针：设备不可用 = 静默降级（引擎照常跑，只是不出声）；「在播」数来自门面账本。
        log::info!("[{tag}] 音频：{}", self.audio.borrow_mut().status_line());
    }

    /// 打印动画实例状态与骨架节点的世界位置。
    ///
    /// 这是「窗口内动画真的在推进」的数值证据：`--run-seconds` 会在起止各打一次，
    /// 起始帧记录所有骨骼节点的世界位置作为基准，收尾帧给出与基准偏差最大的节点与位移量，
    /// 位移明显非零即说明关键帧确实驱动了参与渲染的节点。
    fn log_anim_probe(&mut self, tag: &str) {
        let mut bones: Vec<(String, Vec3)> = Vec::new();
        {
            let scene = self.scene.borrow();
            if scene.animations.is_empty() {
                log::info!("[{tag}] 无动画实例");
            } else {
                for inst in &scene.animations {
                    log::info!(
                        "[{tag}] 动画 `{}` clip={} t={:.3}/{:.3}s speed={:.2} playing={} 绑定={} fade={}",
                        inst.name,
                        inst.clip.name,
                        inst.sample_time(),
                        inst.clip.duration,
                        inst.speed,
                        inst.playing,
                        inst.bindings.len(),
                        match inst.fade.as_ref() {
                            Some(f) => format!("{}→{} {:.0}%", f.clip.name, inst.clip.name, f.weight() * 100.0),
                            None => "无".to_string(),
                        }
                    );
                }
            }
            for node in scene.iter() {
                if node.name.contains('/') {
                    bones.push((node.name.clone(), node.world_position()));
                }
            }
        }

        for (name, p) in bones.iter().take(4) {
            log::info!(
                "[{tag}] 骨骼节点 `{}` 世界位置 ({:.4}, {:.4}, {:.4})",
                name,
                p.x,
                p.y,
                p.z
            );
        }

        if self.probe_snapshot.is_empty() {
            self.probe_snapshot = bones;
            log::info!(
                "[{tag}] 已记录 {} 个骨骼节点位置作为基准",
                self.probe_snapshot.len()
            );
        } else {
            let mut max_delta = 0.0f32;
            let mut max_name = String::new();
            for (name, p) in &bones {
                let Some((_, base)) = self.probe_snapshot.iter().find(|(n, _)| n == name) else {
                    continue;
                };
                let d = (*p - *base).length();
                if d > max_delta {
                    max_delta = d;
                    max_name = name.clone();
                }
            }
            log::info!(
                "[{tag}] 骨骼位移最大：`{}` 移动 {:.4}（对比基准帧）",
                max_name,
                max_delta
            );
        }

        self.log_skin_probe(tag);
    }

    /// 蒙皮探针：用关节矩阵在 CPU 上重算一遍顶点质心。
    ///
    /// 这是「网格真的在形变」的数值证据 —— 骨骼位移非零只说明节点在动，
    /// 质心跟着动才说明 joints/weights 与 skinning 矩阵链路通了。
    fn log_skin_probe(&mut self, tag: &str) {
        let mut centroids: Vec<(String, Vec3)> = Vec::new();
        {
            let scene = self.scene.borrow();
            if scene.skins.is_empty() {
                log::info!("[{tag}] 无蒙皮实例");
                return;
            }
            for skin in &scene.skins {
                let Some(node) = scene.get(skin.mesh_node) else {
                    continue;
                };
                let model = node.world;
                let library = self.assets.borrow();
                let Some(data) = library.mesh_data(node.mesh) else {
                    continue;
                };
                let mut sum = Vec3::ZERO;
                let mut count = 0usize;
                for v in &data.vertices {
                    if !v.is_skinned() {
                        continue;
                    }
                    let mut blended = Mat4::ZERO;
                    for k in 0..r3e_core::MAX_JOINTS_PER_VERTEX {
                        let w = v.weights[k];
                        let j = v.joints[k] as usize;
                        if w <= 0.0 || j >= skin.joint_matrices.len() {
                            continue;
                        }
                        blended += skin.joint_matrices[j] * w;
                    }
                    sum += model.transform_point3(blended.transform_point3(v.position()));
                    count += 1;
                }
                if count == 0 {
                    continue;
                }
                let centroid = sum / count as f32;
                log::info!(
                    "[{tag}] 蒙皮 `{}`：{} 关节 / {count} 顶点参与，质心 ({:.4}, {:.4}, {:.4})，最大偏差 {:.4}",
                    skin.name,
                    skin.joint_count(),
                    centroid.x,
                    centroid.y,
                    centroid.z,
                    skin.max_deviation()
                );
                centroids.push((skin.name.clone(), centroid));
            }
        }

        if self.probe_skin_snapshot.is_empty() {
            self.probe_skin_snapshot = centroids;
        } else {
            for (name, now) in &centroids {
                let Some((_, base)) = self.probe_skin_snapshot.iter().find(|(n, _)| n == name)
                else {
                    continue;
                };
                log::info!(
                    "[{tag}] 蒙皮质心位移 `{}`：{:.4}（对比基准帧）",
                    name,
                    (*now - *base).length()
                );
            }
        }
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
        let assets = self.assets.borrow().clone();
        for id in ids {
            // 物理与渲染同源：碰撞体形状/尺寸都从 AIR 网格几何推出，
            // 内置图元与外部资产走同一条路（S4 去来源分支）。
            let node_mesh = scene
                .get(id)
                .and_then(|node| node_mesh_data(&assets, node.mesh));
            let Some(desc) = scene
                .get(id)
                .and_then(|node| BodyDesc::from_node_with(node, node_mesh.as_ref()))
            else {
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
        // 手柄摇杆 / 扳机 + 键盘方向键快照（死区在输入层生效）
        let (pad_connected, lx, ly, rx, ry, zoom, keys) = {
            let i = self.input.borrow();
            let kb = &i.keyboard;
            (
                i.pad.connected,
                i.pad.axis_deadzone(PAD_LEFT_X),
                i.pad.axis_deadzone(PAD_LEFT_Y),
                i.pad.axis_deadzone(PAD_RIGHT_X),
                i.pad.axis_deadzone(PAD_RIGHT_Y),
                i.pad.axis(PAD_RT) - i.pad.axis(PAD_LT),
                MoveKeys {
                    forward: kb.down(KeyCode::W) || kb.down(KeyCode::ArrowUp),
                    back: kb.down(KeyCode::S) || kb.down(KeyCode::ArrowDown),
                    left: kb.down(KeyCode::A) || kb.down(KeyCode::ArrowLeft),
                    right: kb.down(KeyCode::D) || kb.down(KeyCode::ArrowRight),
                    up: kb.down(KeyCode::E),
                    down: kb.down(KeyCode::Q),
                },
            )
        };
        let pad_move = pad_connected && (lx != 0.0 || ly != 0.0);
        if keys.is_empty() && !pad_move && zoom == 0.0 && rx == 0.0 && ry == 0.0 {
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

        if keys.forward {
            delta += forward;
        }
        if keys.back {
            delta -= forward;
        }
        if keys.right {
            delta += right;
        }
        if keys.left {
            delta -= right;
        }
        if keys.up {
            delta += Vec3::Y;
        }
        if keys.down {
            delta -= Vec3::Y;
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
                    for (slot, down) in st.pad.buttons.iter_mut().zip(buttons.iter()) {
                        slot.set(*down);
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
                if self.paused {
                    "已暂停"
                } else {
                    "继续运行"
                }
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

    /// 当前窗口的像素尺寸（未创建窗口时退回 0，命中测试会因此一律不命中）。
    fn screen_size(&self) -> (f32, f32) {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width as f32, s.height as f32)
            })
            .unwrap_or((0.0, 0.0))
    }

    /// 本帧 UI 交互推进：把光标位置与左键边沿交给 core 的状态机。
    ///
    /// UI 通道被 `set_ui_enabled(false)` 关掉时不做命中——看不见的东西不该挡住点击。
    fn update_ui_interaction(&mut self) {
        let screen = self.screen_size();
        let cursor = {
            let i = self.input.borrow();
            if self.cursor_valid {
                Some((i.mouse_x, i.mouse_y))
            } else {
                None
            }
        };
        let (pressed, down, released) = {
            let i = self.input.borrow();
            (
                i.mouse_pressed(MOUSE_LEFT),
                i.mouse_down(MOUSE_LEFT),
                i.mouse_released(MOUSE_LEFT),
            )
        };
        let enabled = self.scene.borrow().ui_enabled;
        let scene = self.scene.borrow();
        let mut ui = self.ui_interaction.borrow_mut();
        ui.reset();
        if !enabled {
            ui.update(&[], screen, cursor, pressed, down, released);
            return;
        }
        ui.update(&scene.ui, screen, cursor, pressed, down, released);
    }

    /// 该屏幕坐标是否压在一个可见 UI 元素上（用于让 UI 吃掉相机拖动）。
    fn cursor_over_ui(&self, x: f32, y: f32) -> bool {
        if !self.scene.borrow().ui_enabled {
            return false;
        }
        let mut ui = self.ui_interaction.borrow_mut();
        ui.set_screen(self.screen_size());
        let scene = self.scene.borrow();
        ui.hit(&scene.ui, x, y).is_some()
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
            let dy = if self.cursor_valid {
                y as f32 - i.mouse_y
            } else {
                0.0
            };
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

        // 光标压在 UI 上时，这一拖属于 UI（按钮 / 滑块），相机不动。
        // 判断的是「当前位置」而不是「按下时位置」：从 UI 上开始拖、拖出到场景里，
        // 相机也不会突然接手——避免一下拖拽中途切换操纵对象。
        if self.cursor_over_ui(x as f32, y as f32) || self.ui_interaction.borrow().pointer_over_ui()
        {
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

    /// 场景文件热重载时同步 `assets` 段：声明变了就重新导入并换库。
    ///
    /// 只比较「声明」而不是文件内容：声明不变时连磁盘都不碰，避免热重载把
    /// 已经在播放的资产实例整体重建（会丢动画进度、打断物理体）。
    /// 换库后渲染层的 `set_asset_library` 会丢弃旧的 GPU 网格与材质缓存，
    /// 下一帧按新句柄重建，不会引用失效数据。
    fn refresh_assets_from_json(&mut self, json: &str) {
        let scenes = match Scene::from_json_str(json) {
            Ok(s) => s,
            // 解析失败时重放会另有报错，这里不重复刷日志
            Err(_) => return,
        };
        let fresh = collect_asset_specs(
            &scenes,
            &self.scene_path,
            self.assets_root.as_deref(),
            &self.cli_assets,
        );
        let unchanged = {
            let current = self.asset_specs.borrow();
            current.len() == fresh.len()
                && current
                    .iter()
                    .zip(fresh.iter())
                    .all(|(a, b)| asset_spec_eq(a, b))
        };
        if unchanged {
            return;
        }

        let library = Arc::new(load_assets(&fresh));
        log::info!(
            "场景 assets 段已变更，重新导入资产库: {} 个资产",
            library.len()
        );
        *self.asset_specs.borrow_mut() = fresh;
        *self.assets.borrow_mut() = library.clone();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_asset_library(library);
        }
    }

    /// 把内存里的场景写回磁盘（`None` = 写回本场景文件，即编辑器里「保存」的语义）。
    ///
    /// 保存的同时刷新 `scene_mtime`：写回被监视的场景文件时，`watch_files` 会把它
    /// 当成外部改动去做一次热重载（场景回放到刚写的那份 JSON、脚本重跑、动画重置）。
    /// 内容确实等价，但副作用没有必要，所以这里直接把 mtime 记成新值。
    ///
    /// 返回值：实际写入的绝对路径（规范化失败时退回原路径）。
    pub fn save_scene(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let target = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.scene_path.clone());
        {
            let scene = self.scene.borrow();
            scene.save_to_path(&target)?;
        }
        self.scene_mtime = std::fs::metadata(&self.scene_path)
            .ok()
            .and_then(|m| m.modified().ok());
        Ok(std::fs::canonicalize(&target).unwrap_or(target))
    }

    /// 帧首执行本帧排队的场景命令（每帧至多一条）。
    ///
    /// 命令一律在这里落地，而不是在脚本 / RPC 触发的那一刻落地：脚本可能在
    /// `on_start` 中途申请切场景，边重建边被自己打断会把世界撕成两半。
    fn apply_pending_scene_command(&mut self) {
        let Some(cmd) = self.scripts.take_scene_command() else {
            return;
        };
        match self.apply_scene_command(&cmd) {
            Ok(note) => log::info!("场景命令 `{}` 完成：{note}", cmd.label()),
            Err(e) => log::error!("场景命令 `{}` 失败：{e}", cmd.label()),
        }
    }

    /// 执行一条场景命令；返回给日志看的简述。
    fn apply_scene_command(&mut self, cmd: &SceneCommand) -> Result<String, String> {
        match cmd {
            SceneCommand::Switch { path } => {
                let target = self.resolve_scene_path(path);
                let src = std::fs::read_to_string(&target)
                    .map_err(|e| format!("读取场景 `{}` 失败: {e}", target.display()))?;
                self.push_current_layer();
                let restored = self.enter_scene_source(&src, Some(target.clone()))?;
                self.push_live_layer(&target);
                Ok(format!(
                    "已切到 `{}`（{} 个物件，恢复动画实例 {restored} 个）",
                    target.display(),
                    self.scene.borrow().len()
                ))
            }
            SceneCommand::Back => {
                let entry = {
                    let mut stack = self.scene_stack.borrow_mut();
                    stack.capture_current(&self.scene.borrow());
                    stack.back().cloned()
                };
                let Some(entry) = entry else {
                    return Err("已在最早一层，无法回退".to_string());
                };
                self.enter_stack_entry(&entry)?;
                Ok(format!("已回到 `{}`", entry.name))
            }
            SceneCommand::SaveArchive { path } => {
                let written = self.save_scene_archive(path.as_deref())?;
                Ok(format!("存档已写入 `{}`", written.display()))
            }
            SceneCommand::LoadArchive { path } => {
                let target = self.resolve_scene_path(path);
                let src = std::fs::read_to_string(&target)
                    .map_err(|e| format!("读取存档 `{}` 失败: {e}", target.display()))?;
                self.push_current_layer();
                // 读档不接管 `scene_path`：热重载仍盯着原来那个场景文件，
                // 存档只是一次性输入，不是新的"当前场景来源"。
                let restored = self.enter_scene_source(&src, None)?;
                Ok(format!(
                    "已读档 `{}`（恢复动画实例 {restored} 个）",
                    target.display()
                ))
            }
        }
    }

    /// 相对路径按进程工作目录解析；非绝对路径且不存在时，退回场景文件所在目录。
    fn resolve_scene_path(&self, path: &str) -> PathBuf {
        let candidate = PathBuf::from(path);
        if candidate.is_absolute() || candidate.exists() {
            return candidate;
        }
        let beside_scene = self
            .scene_path
            .parent()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|| candidate.clone());
        if beside_scene.exists() {
            beside_scene
        } else {
            candidate
        }
    }

    /// 把内存里的场景现状写回当前层（离开这一层之前必须做一次）。
    fn push_current_layer(&mut self) {
        let scene = self.scene.borrow();
        self.scene_stack.borrow_mut().capture_current(&scene);
    }

    /// 以内存现状压入一层新层（切完场景后调用，让新层成为当前层）。
    fn push_live_layer(&mut self, path: &Path) {
        let entry = {
            let scene = self.scene.borrow();
            let base = if scene.name.trim().is_empty() {
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "scene".to_string())
            } else {
                scene.name.clone()
            };
            let name = self.scene_stack.borrow().unique_name(&base);
            StackEntry::capture(name, path.display().to_string(), &scene)
        };
        let idx = self.scene_stack.borrow_mut().push(entry);
        log::info!(
            "场景栈：压入第 {idx} 层（共 {} 层）",
            self.scene_stack.borrow().depth()
        );
    }

    /// 装载一份场景源文本（裸场景或存档都可），返回恢复的动画实例数。
    ///
    /// 顺序是硬要求：**先换资产库**（重建回调要按新库补挂节点）→ 再就地换场景
    /// （同一个 `Rc`，渲染层与脚本手里那份不会变成野指针）→ 最后叠运行时快照
    /// （时间 / 帧号 / 通道开关 / 动画播放进度）。
    fn enter_scene_source(&mut self, src: &str, follow: Option<PathBuf>) -> Result<usize, String> {
        let archive = SceneArchive::from_json_str(src)?;
        archive.validate()?;
        let (scene_decl, _) = archive.build_scene()?;
        let json = scene_decl
            .to_json_pretty()
            .map_err(|e| format!("场景声明序列化失败: {e}"))?;

        self.refresh_assets_from_json(&json);
        self.scripts.rebuild_scene(&json)?;

        let restored = match &archive.runtime {
            Some(runtime) if runtime.is_compatible() => runtime.apply(&mut self.scene.borrow_mut()),
            _ => 0,
        };

        // 场景文件跟随：`watch_files` 要去盯新文件，否则在切过去的场景上改文件不会热重载。
        if let Some(path) = follow {
            self.scene_mtime = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok());
            self.scene_path = path;
        }
        Ok(restored)
    }

    /// 从栈层恢复：层里存的是源文本 + 运行时快照，不重跑整局。
    fn enter_stack_entry(&mut self, entry: &StackEntry) -> Result<(), String> {
        let mut scene = Scene::from_json_str(&entry.source)
            .map_err(|e| format!("层 `{}` 的场景解析失败: {e}", entry.name))?;
        if entry.runtime.is_compatible() {
            entry.runtime.apply(&mut scene);
        }
        let json = scene
            .to_json_pretty()
            .map_err(|e| format!("层 `{}` 场景序列化失败: {e}", entry.name))?;

        self.refresh_assets_from_json(&json);
        self.scripts.rebuild_scene(&json)?;
        if !entry.path.is_empty() {
            let path = PathBuf::from(&entry.path);
            if path.exists() {
                self.scene_mtime = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                self.scene_path = path;
            }
        }
        Ok(())
    }

    /// 存一份带运行时快照的存档（`None` = 与场景同目录、同名 `.archive.json`）。
    pub fn save_scene_archive(&mut self, path: Option<&str>) -> Result<PathBuf, String> {
        let target = match path {
            Some(p) => PathBuf::from(p),
            None => default_archive_path(&self.scene_path),
        };
        if let Some(dir) = target.parent() {
            if !dir.as_os_str().is_empty() && !dir.exists() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("创建存档目录 `{}` 失败: {e}", dir.display()))?;
            }
        }
        let archive = SceneArchive::from_scene(&self.scene.borrow());
        archive.save_to_path(&target)?;
        Ok(std::fs::canonicalize(&target).unwrap_or(target))
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
                Ok(json) => {
                    // 先看 `assets` 段有没有变：变了就换库，再重建场景（回调会用新库补挂节点）
                    self.refresh_assets_from_json(&json);
                    match self.scripts.rebuild_scene(&json) {
                        Ok(()) => log::info!(
                            "场景文件已热重载 (scene hot-reloaded): {}",
                            self.scene_path.display()
                        ),
                        Err(e) => log::error!("场景重载失败: {e}"),
                    }
                }
                Err(e) => log::error!("读取场景失败: {e}"),
            }
            return;
        }

        // 2) 脚本文件变化 -> 重放场景并重跑脚本
        if self.scripts.reload_if_changed() {
            log::info!("脚本已热重载 (script hot-reloaded)，场景已重新生成");
        }
    }

    /// winit 物理键 → 引擎键位（修饰键左右、方向键、功能键、小键盘都靠它区分）。
    ///
    /// 字符键不走这里：逻辑键（`Key::Character`）才是当前键盘布局下的真实字符。
    fn physical_key_code(code: WinitKeyCode) -> Option<KeyCode> {
        Some(match code {
            WinitKeyCode::Backquote => KeyCode::Backquote,
            WinitKeyCode::Backslash => KeyCode::Backslash,
            WinitKeyCode::BracketLeft => KeyCode::BracketLeft,
            WinitKeyCode::BracketRight => KeyCode::BracketRight,
            WinitKeyCode::Comma => KeyCode::Comma,
            WinitKeyCode::Digit0 => KeyCode::Digit0,
            WinitKeyCode::Digit1 => KeyCode::Digit1,
            WinitKeyCode::Digit2 => KeyCode::Digit2,
            WinitKeyCode::Digit3 => KeyCode::Digit3,
            WinitKeyCode::Digit4 => KeyCode::Digit4,
            WinitKeyCode::Digit5 => KeyCode::Digit5,
            WinitKeyCode::Digit6 => KeyCode::Digit6,
            WinitKeyCode::Digit7 => KeyCode::Digit7,
            WinitKeyCode::Digit8 => KeyCode::Digit8,
            WinitKeyCode::Digit9 => KeyCode::Digit9,
            WinitKeyCode::Equal => KeyCode::Equal,
            WinitKeyCode::KeyA => KeyCode::A,
            WinitKeyCode::KeyB => KeyCode::B,
            WinitKeyCode::KeyC => KeyCode::C,
            WinitKeyCode::KeyD => KeyCode::D,
            WinitKeyCode::KeyE => KeyCode::E,
            WinitKeyCode::KeyF => KeyCode::F,
            WinitKeyCode::KeyG => KeyCode::G,
            WinitKeyCode::KeyH => KeyCode::H,
            WinitKeyCode::KeyI => KeyCode::I,
            WinitKeyCode::KeyJ => KeyCode::J,
            WinitKeyCode::KeyK => KeyCode::K,
            WinitKeyCode::KeyL => KeyCode::L,
            WinitKeyCode::KeyM => KeyCode::M,
            WinitKeyCode::KeyN => KeyCode::N,
            WinitKeyCode::KeyO => KeyCode::O,
            WinitKeyCode::KeyP => KeyCode::P,
            WinitKeyCode::KeyQ => KeyCode::Q,
            WinitKeyCode::KeyR => KeyCode::R,
            WinitKeyCode::KeyS => KeyCode::S,
            WinitKeyCode::KeyT => KeyCode::T,
            WinitKeyCode::KeyU => KeyCode::U,
            WinitKeyCode::KeyV => KeyCode::V,
            WinitKeyCode::KeyW => KeyCode::W,
            WinitKeyCode::KeyX => KeyCode::X,
            WinitKeyCode::KeyY => KeyCode::Y,
            WinitKeyCode::KeyZ => KeyCode::Z,
            WinitKeyCode::Minus => KeyCode::Minus,
            WinitKeyCode::Period => KeyCode::Period,
            WinitKeyCode::Quote => KeyCode::Quote,
            WinitKeyCode::Semicolon => KeyCode::Semicolon,
            WinitKeyCode::Slash => KeyCode::Slash,
            WinitKeyCode::AltLeft => KeyCode::AltLeft,
            WinitKeyCode::AltRight => KeyCode::AltRight,
            WinitKeyCode::Backspace => KeyCode::Backspace,
            WinitKeyCode::CapsLock => KeyCode::CapsLock,
            WinitKeyCode::ControlLeft => KeyCode::ControlLeft,
            WinitKeyCode::ControlRight => KeyCode::ControlRight,
            WinitKeyCode::Enter => KeyCode::Enter,
            WinitKeyCode::SuperLeft => KeyCode::SuperLeft,
            WinitKeyCode::SuperRight => KeyCode::SuperRight,
            WinitKeyCode::ShiftLeft => KeyCode::ShiftLeft,
            WinitKeyCode::ShiftRight => KeyCode::ShiftRight,
            WinitKeyCode::Space => KeyCode::Space,
            WinitKeyCode::Tab => KeyCode::Tab,
            WinitKeyCode::Delete => KeyCode::Delete,
            WinitKeyCode::End => KeyCode::End,
            WinitKeyCode::Home => KeyCode::Home,
            WinitKeyCode::Insert => KeyCode::Insert,
            WinitKeyCode::PageDown => KeyCode::PageDown,
            WinitKeyCode::PageUp => KeyCode::PageUp,
            WinitKeyCode::ArrowDown => KeyCode::ArrowDown,
            WinitKeyCode::ArrowLeft => KeyCode::ArrowLeft,
            WinitKeyCode::ArrowRight => KeyCode::ArrowRight,
            WinitKeyCode::ArrowUp => KeyCode::ArrowUp,
            WinitKeyCode::Escape => KeyCode::Escape,
            WinitKeyCode::F1 => KeyCode::F1,
            WinitKeyCode::F2 => KeyCode::F2,
            WinitKeyCode::F3 => KeyCode::F3,
            WinitKeyCode::F4 => KeyCode::F4,
            WinitKeyCode::F5 => KeyCode::F5,
            WinitKeyCode::F6 => KeyCode::F6,
            WinitKeyCode::F7 => KeyCode::F7,
            WinitKeyCode::F8 => KeyCode::F8,
            WinitKeyCode::F9 => KeyCode::F9,
            WinitKeyCode::F10 => KeyCode::F10,
            WinitKeyCode::F11 => KeyCode::F11,
            WinitKeyCode::F12 => KeyCode::F12,
            WinitKeyCode::Numpad0 => KeyCode::Numpad0,
            WinitKeyCode::Numpad1 => KeyCode::Numpad1,
            WinitKeyCode::Numpad2 => KeyCode::Numpad2,
            WinitKeyCode::Numpad3 => KeyCode::Numpad3,
            WinitKeyCode::Numpad4 => KeyCode::Numpad4,
            WinitKeyCode::Numpad5 => KeyCode::Numpad5,
            WinitKeyCode::Numpad6 => KeyCode::Numpad6,
            WinitKeyCode::Numpad7 => KeyCode::Numpad7,
            WinitKeyCode::Numpad8 => KeyCode::Numpad8,
            WinitKeyCode::Numpad9 => KeyCode::Numpad9,
            WinitKeyCode::NumpadAdd => KeyCode::NumpadAdd,
            WinitKeyCode::NumpadSubtract => KeyCode::NumpadSubtract,
            WinitKeyCode::NumpadMultiply => KeyCode::NumpadMultiply,
            WinitKeyCode::NumpadDivide => KeyCode::NumpadDivide,
            WinitKeyCode::NumpadDecimal => KeyCode::NumpadDecimal,
            _ => return None,
        })
    }

    /// winit 逻辑键 → 引擎键位（字符键、别名导航键）。
    fn logical_key_code(key: &Key) -> Option<KeyCode> {
        match key {
            Key::Character(text) => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => key_from_char(c),
                    _ => None,
                }
            }
            Key::Named(named) => Some(match named {
                NamedKey::Space => KeyCode::Space,
                NamedKey::Enter => KeyCode::Enter,
                NamedKey::Escape => KeyCode::Escape,
                NamedKey::Tab => KeyCode::Tab,
                NamedKey::Backspace => KeyCode::Backspace,
                NamedKey::Delete => KeyCode::Delete,
                NamedKey::Insert => KeyCode::Insert,
                NamedKey::Home => KeyCode::Home,
                NamedKey::End => KeyCode::End,
                NamedKey::PageUp => KeyCode::PageUp,
                NamedKey::PageDown => KeyCode::PageDown,
                NamedKey::ArrowUp => KeyCode::ArrowUp,
                NamedKey::ArrowDown => KeyCode::ArrowDown,
                NamedKey::ArrowLeft => KeyCode::ArrowLeft,
                NamedKey::ArrowRight => KeyCode::ArrowRight,
                NamedKey::Shift => KeyCode::ShiftLeft,
                NamedKey::Control => KeyCode::ControlLeft,
                NamedKey::Alt => KeyCode::AltLeft,
                NamedKey::Super | NamedKey::Meta => KeyCode::SuperLeft,
                NamedKey::CapsLock => KeyCode::CapsLock,
                NamedKey::F1 => KeyCode::F1,
                NamedKey::F2 => KeyCode::F2,
                NamedKey::F3 => KeyCode::F3,
                NamedKey::F4 => KeyCode::F4,
                NamedKey::F5 => KeyCode::F5,
                NamedKey::F6 => KeyCode::F6,
                NamedKey::F7 => KeyCode::F7,
                NamedKey::F8 => KeyCode::F8,
                NamedKey::F9 => KeyCode::F9,
                NamedKey::F10 => KeyCode::F10,
                NamedKey::F11 => KeyCode::F11,
                NamedKey::F12 => KeyCode::F12,
                _ => return None,
            }),
            _ => None,
        }
    }

    /// 一次按键事件 → 引擎键位。
    ///
    /// 字符键以逻辑键为准（布局相关：AZERTY 上按 "A" 就是 `A`），但小键盘要保留物理身份；
    /// 非字符键以物理键为准（能区分左右 Shift / Ctrl，逻辑键只能给出通用的 `Shift`）。
    fn key_code_of(logical: &Key, physical: PhysicalKey) -> Option<KeyCode> {
        let physical_code = match physical {
            PhysicalKey::Code(code) => Self::physical_key_code(code),
            _ => None,
        };
        match logical {
            Key::Character(_) => match physical_code {
                Some(code) if code.is_numpad() => Some(code),
                _ => Self::logical_key_code(logical).or(physical_code),
            },
            _ => physical_code.or_else(|| Self::logical_key_code(logical)),
        }
    }

    /// 处理键盘事件：先把状态写进输入层（脚本与本帧相机共享），再处理引擎级快捷键。
    ///
    /// `repeat`（系统自动重复）只更新持续状态，不重复触发快捷键，
    /// 因此按住空格不会来回切暂停。
    fn handle_key(
        &mut self,
        event_loop: &ActiveEventLoop,
        logical: Key,
        physical: PhysicalKey,
        state: ElementState,
        repeat: bool,
    ) {
        let pressed = state == ElementState::Pressed;
        if let Some(code) = Self::key_code_of(&logical, physical) {
            self.input.borrow_mut().keyboard.set(code, pressed);
        }

        if !pressed || repeat {
            return;
        }

        match &logical {
            Key::Named(NamedKey::Escape) => event_loop.exit(),
            Key::Named(NamedKey::Space) => {
                self.paused = !self.paused;
                log::info!(
                    "{}",
                    if self.paused {
                        "已暂停"
                    } else {
                        "继续运行"
                    }
                );
            }
            Key::Named(NamedKey::F2) => match self.save_scene(None) {
                Ok(path) => log::info!("场景已保存 (scene saved): {}", path.display()),
                Err(e) => log::error!("场景保存失败: {e}"),
            },
            Key::Named(NamedKey::F5) => {
                let path = self.script_path.clone();
                if let Err(e) = self.scripts.load_file(&path) {
                    log::error!("重载失败: {e}");
                }
            }
            _ => {}
        }
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
                let library = self.assets.borrow().clone();
                if !library.is_empty() {
                    renderer.set_asset_library(library.clone());
                    log::info!(
                        "渲染层已接入资产库: {} 个资产 / {} 个资产节点",
                        library.len(),
                        self.asset_nodes.borrow().len()
                    );
                }
                // UI 图片：应用层提供解码器，渲染层只消费 TextureImage
                renderer.set_ui_image_provider(Arc::new(FsUiImageProvider));
                // --font：显式指定字体（不指定时 TextPass 已自动加载系统中文字体）
                if !self.cli_fonts.is_empty() {
                    match renderer.load_text_fonts(&self.cli_fonts) {
                        Ok(()) => {
                            log::info!("文本字体已替换: {:?}", renderer.text().font_sources())
                        }
                        Err(e) => log::warn!("--font 加载失败，继续用系统字体: {e}"),
                    }
                }
                log::info!(
                    "文本通道就绪（字体 {:?}，图集 {} 页）",
                    renderer.text().font_sources(),
                    renderer.text().atlas_pages()
                );
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
                self.handle_key(
                    event_loop,
                    event.logical_key,
                    event.physical_key,
                    event.state,
                    event.repeat,
                );
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x, position.y);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(button, state);
            }
            WindowEvent::Focused(false) => {
                // 失焦后收不到松开事件，主动清掉鼠标 / 键盘 / 手柄的持续按下状态，
                // 避免按住键切窗口后卡键（键盘的边沿也一并清掉，不会补发一次"抬起"）
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
                if self.exit_pending {
                    event_loop.exit();
                }
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
#[derive(Clone)]
struct AssetSpec {
    path: PathBuf,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
    spawn: bool,
    /// 场景 JSON `assets[].anim` 段（P3）：声明要播放的片段与参数。
    anim: Option<AnimDecl>,
}

/// 合并命令行 `--asset` 与场景文件 `assets` 段。
///
/// 命令行排在最前，便于临时替换 / 追加；场景内相对路径按 `assets_root` 解析，
/// 未指定 `--assets-root` 时退回场景文件所在目录（保持旧行为）。
fn collect_asset_specs(
    scene: &Scene,
    scene_path: &Path,
    assets_root: Option<&Path>,
    cli: &[PathBuf],
) -> Vec<AssetSpec> {
    let base_dir =
        assets_root.unwrap_or_else(|| scene_path.parent().unwrap_or_else(|| Path::new(".")));
    let mut specs: Vec<AssetSpec> = cli
        .iter()
        .map(|path| AssetSpec {
            path: path.clone(),
            position: None,
            rotation: None,
            scale: None,
            spawn: true,
            anim: None,
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
            anim: decl.anim.clone(),
        });
    }
    specs
}

/// 两条资产声明是否等价（用于热重载时判断要不要重新导入）。
///
/// 不实现 `PartialEq` 而是手写：`AnimDecl` 在 core 层没有派生，且语义上
/// 只需要「路径 + 变换 + spawn + 动画参数」这几项参与比较。
fn asset_spec_eq(a: &AssetSpec, b: &AssetSpec) -> bool {
    a.path == b.path
        && a.spawn == b.spawn
        && a.position == b.position
        && a.rotation == b.rotation
        && a.scale == b.scale
        && match (&a.anim, &b.anim) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                x.clip == y.clip
                    && x.speed == y.speed
                    && x.looping == y.looping
                    && x.time == y.time
                    && x.play == y.play
            }
            _ => false,
        }
}

/// 取节点的 AIR 网格数据（资产优先、内置图元兜底），供物理推断碰撞体。
///
/// 与渲染层同一个 `MeshProvider` 语义：谁提供数据不重要，物理只认 `MeshData`。
fn node_mesh_data(assets: &AssetLibrary, mesh: MeshRef) -> Option<MeshData> {
    <AssetLibrary as MeshProvider>::mesh_data(assets, mesh)
        .or_else(|| r3e_core::procedural::ProceduralProvider::new().mesh_data(mesh))
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
        match library.load(&spec.path) {
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

/// 存档默认落点：与场景文件同目录、同名 + `.archive.json`。
///
/// 单独成一个函数是为了让 RPC 侧（`scene.archive.save` 省略 path 时）与主循环
/// 走同一条路径规则——两处各写一份是最容易长歪的地方。
pub fn default_archive_path(scene_path: &Path) -> PathBuf {
    scene_path.with_extension("archive.json")
}

/// 无窗口的「加载 → 规范化保存」：把场景读进内存再按引擎写法写回 `out`。
///
/// 给 CI / 脚本批量把手写的场景 JSON 规范成统一格式用（`--save-scene <out>`）。
/// 不应用命令行物理覆盖——保存的是**文件里声明的源数据**，不是进程运行期状态。
///
/// 返回 `(物件数, 写入字节数)`。
pub fn save_scene_file(scene_path: &Path, out: &Path) -> Result<(usize, u64), String> {
    let src = std::fs::read_to_string(scene_path)
        .map_err(|e| format!("读取场景 {} 失败: {e}", scene_path.display()))?;
    let scene = Scene::from_json_str(&src).map_err(|e| format!("场景解析失败: {e}"))?;
    let nodes = scene.len();
    scene.save_to_path(out)?;
    let bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok((nodes, bytes))
}

/// P3 数值验收：打印每个片段的轨道统计，并在若干时刻采样「位移最大的轨道」。
///
/// 骨骼动画在 P4 之前不会改变网格形状（顶点是 P2 烘焙好的世界坐标），
/// 所以这里用采样表证明「轨道 → 节点 TRS」这条链路真的在动，而不是靠肉眼看窗口。
pub fn dump_anim(path: &Path, want_clip: Option<&str>) -> Result<String, String> {
    let mut library = AssetLibrary::new();
    let id = library
        .load(path)
        .map_err(|e| format!("资产导入失败: {e}"))?;
    let entry = library
        .get(id)
        .ok_or_else(|| format!("资产 {} 未在库中", path.display()))?;

    let mut out = String::new();
    out.push_str(&format!("动画报告 {}\n", path.display()));
    out.push_str(&format!(
        "节点 {} / 网格 {} / 动画栈 {}\n",
        entry.nodes.len(),
        entry.meshes.len(),
        entry.animations.len()
    ));
    if entry.animations.is_empty() {
        out.push_str("该资产没有动画栈（静态模型）\n");
        return Ok(out);
    }

    for (i, clip) in entry.animations.iter().enumerate() {
        if let Some(want) = want_clip.map(str::trim).filter(|s| !s.is_empty()) {
            if !clip.name.eq_ignore_ascii_case(want) {
                continue;
            }
        }
        let keys: usize = clip.tracks.iter().map(|t| t.keys.len()).sum();
        out.push_str(&format!(
            "\n[{i}] {:<12} 时长 {:>6.2}s  轨道 {:>3}  关键帧 {:>5}\n",
            clip.name,
            clip.duration,
            clip.tracks.len(),
            keys
        ));

        // 找出位置变化幅度最大的轨道，用它证明采样不是常量
        let mut best: Option<(&r3e_core::AnimTrack, f32)> = None;
        for track in &clip.tracks {
            let (Some(first), Some(last)) = (track.keys.first(), track.keys.last()) else {
                continue;
            };
            let span = (last.position - first.position).length()
                + (first.position - track.keys[track.keys.len() / 2].position).length();
            if best.map(|(_, s)| span > s).unwrap_or(true) {
                best = Some((track, span));
            }
        }
        let Some((track, span)) = best else {
            out.push_str("  （所有轨道都没有关键帧）\n");
            continue;
        };
        out.push_str(&format!(
            "  位移最大轨道: {}（幅度 {:.3}）\n",
            if track.node_path.is_empty() {
                format!("#{}", track.node_index)
            } else {
                track.node_path.clone()
            },
            span
        ));
        let duration = clip.duration.max(1e-4);
        for step in 0..5 {
            let t = duration * step as f32 / 4.0;
            if let Some(tr) = track.sample(t) {
                let e = tr.rotation.to_euler(glam::EulerRot::XYZ);
                out.push_str(&format!(
                    "    t={:.2}s  pos({:>8.2}, {:>8.2}, {:>8.2})  euler_deg({:>7.1}, {:>7.1}, {:>7.1})\n",
                    t,
                    tr.position.x,
                    tr.position.y,
                    tr.position.z,
                    e.0.to_degrees(),
                    e.1.to_degrees(),
                    e.2.to_degrees()
                ));
            }
        }
    }
    Ok(out)
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
    show_bones: bool,
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
        // 网格节点按资产内 mesh 下标对齐（P4 蒙皮要拿它当被驱动对象）
        let mut mesh_nodes: Vec<NodeId> = Vec::new();
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
                material: None,
            };
            match scene.spawn(desc) {
                Ok(node_id) => {
                    out.push(node_id);
                    mesh_nodes.push(node_id);
                    spawned += 1;
                }
                Err(e) => log::warn!("资产节点 `{name}` spawn 失败: {e}"),
            }
        }

        // 有节点树时建骨架（动画片段顺带登记）；纯静态资产不额外造节点
        if !entry.nodes.is_empty() && (!entry.animations.is_empty() || !entry.skins.is_empty()) {
            let skel = spawn_asset_anim(scene, entry, spec, &path_str, show_bones);

            // P4：骨架就位后立刻以「静止姿态」绑定蒙皮 —— 逆绑定矩阵必须取这一刻的
            // 世界矩阵，任何动画推进之后再绑都会把当前姿态误当绑定姿态。
            if let Some(skel) = skel {
                scene.update_world_transforms();
                bind_asset_skins(scene, entry, &path_str, &skel, &mesh_nodes);
            }
        }
    }
    spawned
}

/// 一个资产实例建出来的骨架句柄：节点下标 → 场景节点，供蒙皮按 `node_index` 对齐。
struct SkeletonSpawn {
    prefix: String,
    ids: Vec<Option<NodeId>>,
}

/// 把资产的蒙皮数据绑到网格节点上（P4）。
///
/// 关节按 `node_index` 对齐场景节点（与动画轨道同一套对齐方式），路径只作日志；
/// 绑定姿态取当前世界矩阵，交给 `SkinInstance` 求逆绑定矩阵。
fn bind_asset_skins(
    scene: &mut Scene,
    entry: &r3e_asset::AssetEntry,
    path_str: &str,
    skel: &SkeletonSpawn,
    mesh_nodes: &[NodeId],
) {
    for (mi, mesh) in entry.meshes.iter().enumerate() {
        let Some(skin_index) = mesh.skin else {
            continue;
        };
        let Some(data) = entry.skin(skin_index) else {
            log::warn!("资产 `{path_str}` 的网格 #{mi} 引用了不存在的蒙皮 #{skin_index}");
            continue;
        };
        let Some(&mesh_node) = mesh_nodes.get(mi) else {
            continue;
        };
        let Some(mesh_world) = scene.get(mesh_node).map(|n| n.world) else {
            continue;
        };
        let mut inst = SkinInstance::new(format!("{}/skin#{mi}", skel.prefix), mesh_node);
        for joint in &data.joints {
            let Some(Some(node)) = skel.ids.get(joint.node_index as usize) else {
                continue;
            };
            let Some(n) = scene.get(*node) else {
                continue;
            };
            inst.add_joint(
                *node,
                joint.node_index,
                joint.path.clone(),
                n.world,
                mesh_world,
            );
        }
        if inst.is_empty() {
            log::warn!("蒙皮 `{}` 无有效关节，跳过", inst.name);
            continue;
        }
        log::info!(
            "蒙皮 `{}`：{} 个关节 / {} 顶点带权重，绑定到网格节点 `{}`",
            inst.name,
            inst.joint_count(),
            mesh.joints.len(),
            scene
                .get(mesh_node)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        );
        scene.add_skin(inst);
    }
}

/// 为一个资产实例建立「骨架节点树 + 播放器」（P3）。
///
/// 场景节点命名规则 `<资产名>/<FBX 节点路径>`，例如
/// `tirger/RootNode/bip01/Spine`。网格顶点在导入时已烘焙到世界空间（P2 约定），
/// 因此骨架节点只承载变换、不挂网格（`visible: false` 纯粹是让渲染层跳过），
/// 真正决定形状的蒙皮要等 P4；P3 验证的是「轨道采样 → 节点局部 TRS」这条链路。
///
/// 返回骨架句柄（根节点创建失败时为 `None`）；无可用片段时只建骨架不挂播放器。
fn spawn_asset_anim(
    scene: &mut Scene,
    entry: &r3e_asset::AssetEntry,
    spec: &AssetSpec,
    path_str: &str,
    show_bones: bool,
) -> Option<SkeletonSpawn> {
    let stem = Path::new(path_str)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset")
        .to_string();
    // 同一 FBX 可能被声明多次（不同位移），骨架名字要能区分
    let prefix = {
        let mut p = stem.clone();
        let mut n = 0u32;
        while !scene.find_all(&format!("{p}/")).is_empty() && n < 100 {
            n += 1;
            p = format!("{stem}#{n}");
        }
        p
    };

    // 资产实例根：承载 `assets[].position/rotation/scale`，骨架各根节点挂在它下面
    let root_name = prefix.clone();
    if let Err(e) = scene.spawn(ObjectDesc {
        name: root_name.clone(),
        mesh: None,
        position: spec.position,
        rotation: spec.rotation,
        scale: spec.scale,
        color: None,
        parent: None,
        visible: Some(false),
        physics: None,
        material: None,
    }) {
        log::warn!("资产实例根 `{root_name}` 创建失败: {e}");
        return None;
    }

    // 建节点：父节点先建，父下标缺失时挂到实例根
    let mut ids: Vec<Option<NodeId>> = vec![None; entry.nodes.len()];
    let mut pending: Vec<usize> = (0..entry.nodes.len()).collect();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|&i| {
            let node = &entry.nodes[i];
            let parent_name = match node.parent {
                Some(pi) if (pi as usize) < ids.len() => match ids[pi as usize] {
                    Some(_) => {
                        let p = &entry.nodes[pi as usize];
                        format!(
                            "{prefix}/{}",
                            if p.path.is_empty() {
                                p.name.as_str()
                            } else {
                                p.path.as_str()
                            }
                        )
                    }
                    None => return true, // 父还没建，留到下一轮
                },
                _ => root_name.clone(),
            };
            let name = format!(
                "{prefix}/{}",
                if node.path.is_empty() {
                    node.name.as_str()
                } else {
                    node.path.as_str()
                }
            );
            let desc = ObjectDesc {
                name: name.clone(),
                mesh: None,
                position: Some(node.position),
                rotation: None,
                scale: Some(node.scale),
                color: None,
                parent: Some(parent_name),
                visible: Some(false),
                physics: None,
                material: None,
            };
            match scene.spawn(desc) {
                Ok(id) => {
                    // 四元数直接用绑定姿态，避免 euler 往返丢精度
                    if let Some(n) = scene.get_mut(id) {
                        n.transform.rotation = glam::Quat::from_array(node.rotation);
                    }
                    ids[i] = Some(id);
                    false
                }
                Err(e) => {
                    log::warn!("骨架节点 `{name}` 创建失败: {e}");
                    false
                }
            }
        });
        if pending.len() == before {
            // 父链有环或父下标越界：剩下的全挂实例根，避免死循环
            for i in std::mem::take(&mut pending) {
                let node = &entry.nodes[i];
                let name = format!("{prefix}/{}", node.path);
                if let Ok(id) = scene.spawn(ObjectDesc {
                    name,
                    mesh: None,
                    position: Some(node.position),
                    rotation: None,
                    scale: Some(node.scale),
                    color: None,
                    parent: Some(root_name.clone()),
                    visible: Some(false),
                    physics: None,
                    material: None,
                }) {
                    if let Some(n) = scene.get_mut(id) {
                        n.transform.rotation = glam::Quat::from_array(node.rotation);
                    }
                    ids[i] = Some(id);
                }
            }
            break;
        }
    }
    debug_assert!(!ids.is_empty() || entry.nodes.is_empty());

    // `--show-bones`：每个骨架节点挂一个小方块，动画推进时方块跟着动。
    // P4 蒙皮上线前，这是唯一能用肉眼确认「节点 TRS 采样生效」的手段。
    if show_bones {
        let root_scale = spec
            .scale
            .map(|s| s[0].abs())
            .filter(|v| *v > 1e-6)
            .unwrap_or(1.0);
        let local = 0.25 / root_scale;
        let mut markers = 0usize;
        for (i, slot) in ids.iter().enumerate() {
            if slot.is_none() {
                continue;
            }
            let node = &entry.nodes[i];
            let node_name = format!(
                "{prefix}/{}",
                if node.path.is_empty() {
                    node.name.as_str()
                } else {
                    node.path.as_str()
                }
            );
            match scene.spawn(ObjectDesc {
                name: format!("{prefix}/bone#{i}"),
                mesh: Some("cube".to_string()),
                position: Some([0.0, 0.0, 0.0]),
                rotation: None,
                scale: Some([local, local, local]),
                color: Some([1.0, 0.42, 0.12, 1.0]),
                parent: Some(node_name),
                visible: Some(true),
                physics: None,
                material: None,
            }) {
                Ok(_) => markers += 1,
                Err(e) => log::warn!("骨架标记 `{prefix}/bone#{i}` 创建失败: {e}"),
            }
        }
        log::info!("骨架可视化: `{prefix}` 共 {markers} 个标记方块");
    }

    // 把该资产的全部片段登记到场景，供脚本 `set_anim_clip` 切换（idle → run → atk）
    for anim in &entry.animations {
        if !anim.is_empty() {
            scene.register_anim_clip(format!("{prefix}/{}", anim.name), Arc::new(anim.clone()));
        }
    }

    // 选片段：声明里的 clip（大小写不敏感）优先，否则第一个
    let decl = spec.anim.clone().unwrap_or_default();
    let clip = match decl
        .clip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(want) => entry
            .animations
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(want))
            .or_else(|| entry.default_animation()),
        None => entry.default_animation(),
    };
    let Some(clip) = clip else {
        // 只有蒙皮没有动画的资产：骨架已建好，回去绑蒙皮即可
        log::info!(
            "资产 `{path_str}` 无动画片段，仅建立骨架（{} 个节点）",
            ids.len()
        );
        return Some(SkeletonSpawn {
            prefix: prefix.clone(),
            ids,
        });
    };

    // 轨道 → 场景节点（按资产内节点下标对齐，比路径匹配更稳；路径留作切片段时的兜底）
    let mut bindings = Vec::new();
    for (ti, track) in clip.tracks.iter().enumerate() {
        if let Some(Some(id)) = ids.get(track.node_index as usize) {
            bindings.push(AnimBinding {
                node: *id,
                node_index: track.node_index,
                path: track.node_path.clone(),
                track: ti as u32,
            });
        }
    }

    let mut inst_name = format!("{prefix}/{}", clip.name);
    if scene.anim_instance(&inst_name).is_some() {
        inst_name = format!("{prefix}#{}", clip.name);
    }
    let mut inst = AnimInstance::new(inst_name.clone(), Arc::new(clip.clone()));
    inst.bindings = bindings;
    inst.speed = decl.speed.unwrap_or(1.0);
    inst.looping = decl.looping.unwrap_or(true);
    inst.time = decl.time.unwrap_or(0.0);
    inst.playing = decl.play.unwrap_or(true);
    let tracks = inst.track_count();
    let duration = inst.clip.duration;
    scene.add_anim_instance(inst);
    log::info!(
        "动画实例 `{inst_name}`：{} 条轨道 / {:.2}s / {}",
        tracks,
        duration,
        if scene
            .anim_instance(&inst_name)
            .map(|a| a.playing)
            .unwrap_or(false)
        {
            "播放中"
        } else {
            "已暂停"
        }
    );
    let _ = inst_name;
    Some(SkeletonSpawn { prefix, ids })
}

#[cfg(test)]
mod key_event_tests {
    use super::*;

    /// 字符键走逻辑键：AZERTY 上按 "a" 报 `A`，与物理位置无关。
    #[test]
    fn character_keys_follow_logical_layout() {
        let key = Key::Character("a".into());
        // 物理位置是 Q 的键（AZERTY 的 A），逻辑键仍是 a
        assert_eq!(
            EngineApp::key_code_of(&key, PhysicalKey::Code(WinitKeyCode::KeyQ)),
            Some(KeyCode::A)
        );
        let upper = Key::Character("W".into());
        assert_eq!(
            EngineApp::key_code_of(&upper, PhysicalKey::Code(WinitKeyCode::KeyW)),
            Some(KeyCode::W),
            "大写字母应归到同一键位"
        );
    }

    /// 物理键补足逻辑键给不出的信息：左右修饰键分别映射。
    #[test]
    fn modifier_side_comes_from_physical_key() {
        let shift = Key::Named(NamedKey::Shift);
        assert_eq!(
            EngineApp::key_code_of(&shift, PhysicalKey::Code(WinitKeyCode::ShiftRight)),
            Some(KeyCode::ShiftRight)
        );
        assert_eq!(
            EngineApp::key_code_of(&shift, PhysicalKey::Code(WinitKeyCode::ShiftLeft)),
            Some(KeyCode::ShiftLeft)
        );
        // 逻辑键单独解析时退回左修饰键，与脚本别名 `shift` 一致
        assert_eq!(
            EngineApp::logical_key_code(&shift),
            Some(KeyCode::ShiftLeft)
        );
        assert_eq!(
            EngineApp::key_code_of(
                &Key::Named(NamedKey::Control),
                PhysicalKey::Code(WinitKeyCode::ControlRight)
            ),
            Some(KeyCode::ControlRight)
        );
    }

    /// 小键盘数字与主键区数字分开统计。
    #[test]
    fn numpad_digits_stay_distinct() {
        let one = Key::Character("1".into());
        assert_eq!(
            EngineApp::key_code_of(&one, PhysicalKey::Code(WinitKeyCode::Numpad1)),
            Some(KeyCode::Numpad1)
        );
        assert_eq!(
            EngineApp::key_code_of(&one, PhysicalKey::Code(WinitKeyCode::Digit1)),
            Some(KeyCode::Digit1)
        );
    }

    /// 功能键 / 方向键 / 未知键。
    #[test]
    fn named_and_unknown_keys() {
        assert_eq!(
            EngineApp::key_code_of(
                &Key::Named(NamedKey::F5),
                PhysicalKey::Code(WinitKeyCode::F5)
            ),
            Some(KeyCode::F5)
        );
        assert_eq!(
            EngineApp::key_code_of(
                &Key::Named(NamedKey::ArrowLeft),
                PhysicalKey::Code(WinitKeyCode::ArrowLeft)
            ),
            Some(KeyCode::ArrowLeft)
        );
        // 引擎不关心的键不写入输入层
        assert_eq!(
            EngineApp::key_code_of(
                &Key::Named(NamedKey::Help),
                PhysicalKey::Code(WinitKeyCode::Help)
            ),
            None
        );
    }

    /// 相机移动判据：任意一个方向键按下即应参与计算。
    #[test]
    fn move_keys_detect_any_direction() {
        assert!(MoveKeys::default().is_empty());
        assert!(!MoveKeys {
            up: true,
            ..Default::default()
        }
        .is_empty());
    }
}
