//! r3e —— 用 Rust 写的最小可用 3D 游戏引擎。
//!
//! 用法：
//! ```text
//! r3e [场景.json] [脚本.rhai]
//! ```
//! 缺省读取 `assets/scene.json` 与 `scripts/main.rhai`（相对项目根目录）。
//!
//! 操作：
//! * WASD / 方向键：水平移动相机
//! * Q / E：升降相机
//! * 鼠标滚轮：推拉相机
//! * 空格：暂停 / 继续
//! * F5：重新载入脚本（脚本文件改动后会自动热重载）
//! * Esc：退出

mod app;

use std::path::{Path, PathBuf};

fn find_project_root() -> PathBuf {
    // 1) 优先当前工作目录（用户在项目根目录执行 cargo run / r3e.exe）
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("assets").is_dir() {
            return cwd;
        }
    }
    // 2) 退化到可执行文件所在目录的上级
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..3 {
            if let Some(d) = dir.clone() {
                if d.join("assets").is_dir() {
                    return d;
                }
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }
    // 3) 编译期路径兜底
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let root = find_project_root();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene_path = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("assets/scene.json"));
    let script_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("scripts/main.rhai"));

    log::info!("项目根目录: {}", root.display());
    log::info!("场景: {}", scene_path.display());
    log::info!("脚本: {}", script_path.display());

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = app::EngineApp::new(scene_path, script_path);
    event_loop.run_app(&mut app)?;
    Ok(())
}
