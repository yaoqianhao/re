//! r3e 运行时：窗口、主循环、时间步、headless 模式。
//!
//! 这个 crate 是「引擎自己怎么活」的唯一答案：怎么开窗、怎么计时、一帧里按什么
//! 顺序推进物理/脚本/渲染、没有窗口时怎么把同样的逻辑跑完。
//!
//! 它**不认识任何编辑器协议**。编辑器宿主（`r3e-editor-host`）反过来依赖它，
//! 通过 [`port::EditorService`] 把协议请求接进主循环。这样「跑一次像素回归」
//! 与「开一个编辑器会话」共用同一条帧路径，谁也不用为了对方多链接一堆东西。

pub mod app;
mod asset_scene;
pub mod capture;
pub mod cli;
mod physics_bind;
mod physics_sync;
pub mod port;
mod scene_material_import;
pub mod terrain_host;
pub mod terrain_ops;
pub mod terrain_scene;
pub mod time;

pub use app::{CliOptions, EngineApp, HeadlessReport, HEADLESS_FIXED_DT};
pub use capture::{absolute_path, capture_to_file, default_capture_name, CaptureInfo};
pub use cli::{find_project_root, parse, print_usage, run_offscreen_tools, Parsed};
pub use port::{EditorService, EngineAccess};
pub use terrain_host::TerrainFacade;
// 脚本宿主与它的装配入口（M2-A03b）：`r3e-script` 已不认识 physics/audio，
// 需要「拿真实门面构造宿主」的调用方从这里拿，不要直接写 `ScriptHost::new`。
pub use r3e_script::ScriptHost;
pub use time::{FrameTimeStats, FrameTimeWindow, FRAME_TIME_WINDOW};

/// 启动引擎：headless 或窗口，二选一。
///
/// `editor` 是可选挂件：给了就每帧从中取编辑器请求，没给就是纯引擎进程。
/// headless 分支**不会**进事件循环——「无窗口」的意义就是没有事件循环，
/// 若在 headless 路径里仍创建 `EventLoop`，那在没有显示设备的 CI 容器里会直接失败。
pub fn run(parsed: cli::Parsed, editor: Option<Box<dyn EditorService>>) -> anyhow::Result<()> {
    log::info!("项目根目录: {}", parsed.root.display());
    log::info!("场景: {}", parsed.scene_path.display());
    log::info!("脚本: {}", parsed.script_path.display());

    let headless_frames = parsed.cli.headless_frames;
    let mut app = EngineApp::new(parsed.scene_path, parsed.script_path, parsed.cli, editor);

    if let Some(frames) = headless_frames {
        // 无头模式不接编辑器：桥的存在意义是「有人在看」，无头进程没有。
        if app.has_editor() {
            log::warn!("--headless-frames 与编辑器桥同时给出，headless 路径将忽略桥");
        }
        let report = app.run_headless(frames, HEADLESS_FIXED_DT);
        println!("{report}");
        return Ok(());
    }

    let event_loop = winit::event_loop::EventLoop::new()?;
    // Poll 而不是 Wait：引擎按帧推进，等事件会让「没有输入就没有帧」，
    // 脚本与物理就停在那儿了。
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    event_loop.run_app(&mut app)?;
    Ok(())
}
