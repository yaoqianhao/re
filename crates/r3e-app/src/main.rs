//! r3e —— 用 Rust 写的最小可用 3D 游戏引擎。
//!
//! 用法：
//! ```text
//! r3e [场景.json] [脚本.rhai] [选项]
//! ```
//! 缺省读取 `assets/scene.json` 与 `scripts/main.rhai`（相对项目根目录）。
//!
//! 物理选项（命令行优先级最高，覆盖场景文件里的 `physics` 段）：
//! * `--physics on|off`：开关物理模拟
//! * `--physics-backend builtin|null|rapier`：选择后端（`rapier` 需 `--features rapier` 构建）
//! * `--gravity x,y,z`：重力加速度，例如 `--gravity 0,-9.81,0`
//! * `--ground y`：地面平面高度，例如 `--ground 0`
//!
//! 资产选项（M1 静态网格）：
//! * `--asset <fbx>`：导入 FBX 并按资产内节点路径 spawn（可重复）
//! * `--dump-asset <fbx>`：只导入并打印资产报告，不创建窗口
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

use app::CliOptions;

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

/// 解析 `on/off/true/false/1/0/yes/no`。
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "enable" | "enabled" => Some(true),
        "off" | "false" | "0" | "no" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

fn parse_vec3(s: &str) -> Option<[f32; 3]> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return None;
    }
    let x = parts[0].trim().parse().ok()?;
    let y = parts[1].trim().parse().ok()?;
    let z = parts[2].trim().parse().ok()?;
    Some([x, y, z])
}

const USAGE: &str = "\
r3e —— Rust 3D Engine

用法:
  r3e [场景.json] [脚本.rhai] [选项]

物理选项（覆盖场景文件里的 physics 段）:
  --physics on|off          开关物理模拟
  --physics-backend NAME    物理后端: builtin | null | rapier（rapier 需 --features rapier 构建）
  --gravity x,y,z           重力加速度，默认 0,-9.81,0
  --ground Y                地面平面高度，默认 0
  --ccd on|off              逐刚体 CCD 缺省值，默认 on（需后端支持，见 --physics-backend）

资产选项（M1 静态网格）:
  --asset PATH              导入 FBX 并按资产内节点路径 spawn（可重复）
  --assets-root DIR         场景 assets 段相对路径的解析根（默认场景文件所在目录）
  --dump-asset PATH         导入 FBX 打印资产报告后退出（不建窗口）

动画选项（P3 节点动画）:
  --dump-anim PATH          导入 FBX 打印各动画片段采样表后退出（不建窗口）
  --anim-clip NAME          配合 --dump-anim 只看指定片段
  --show-bones              把骨架节点画成小方块，肉眼确认节点动画
  --run-seconds SEC         窗口运行指定秒数后自动退出（无人值守验证）
  -h, --help                显示本帮助
";

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let root = find_project_root();
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let mut positional: Vec<String> = Vec::new();
    let mut cli = CliOptions::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].clone();
        let take_value = |name: &str, i: &mut usize| -> Option<String> {
            if let Some(v) = arg.split_once('=').map(|(_, v)| v.to_string()) {
                return Some(v);
            }
            if *i + 1 < argv.len() {
                *i += 2;
                Some(argv[*i - 1].clone())
            } else {
                log::error!("参数 {name} 缺少取值");
                None
            }
        };
        match arg.split('=').next().unwrap_or("") {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--physics" => {
                if let Some(v) = take_value("--physics", &mut i) {
                    match parse_bool(&v) {
                        Some(b) => cli.physics = Some(b),
                        None => log::error!("--physics 取值应为 on/off，收到 `{v}`"),
                    }
                }
                continue;
            }
            "--physics-backend" | "--backend" => {
                if let Some(v) = take_value("--physics-backend", &mut i) {
                    cli.backend = Some(v);
                }
                continue;
            }
            "--gravity" => {
                if let Some(v) = take_value("--gravity", &mut i) {
                    match parse_vec3(&v) {
                        Some(g) => cli.gravity = Some(g),
                        None => log::error!("--gravity 取值应为 x,y,z，收到 `{v}`"),
                    }
                }
                continue;
            }
            "--ccd" => {
                if let Some(v) = take_value("--ccd", &mut i) {
                    match parse_bool(&v) {
                        Some(b) => cli.ccd = Some(b),
                        None => log::error!("--ccd 取值应为 on/off，收到 `{v}`"),
                    }
                }
                continue;
            }
            "--ground" => {
                if let Some(v) = take_value("--ground", &mut i) {
                    match v.trim().parse::<f32>() {
                        Ok(y) => cli.ground = Some(y),
                        Err(_) => log::error!("--ground 取值应为数字，收到 `{v}`"),
                    }
                }
                continue;
            }
            "--asset" => {
                if let Some(v) = take_value("--asset", &mut i) {
                    if v.trim().is_empty() {
                        log::error!("--asset 需要非空的 FBX 路径");
                    } else {
                        cli.assets.push(PathBuf::from(v));
                    }
                }
                continue;
            }
            "--assets-root" => {
                if let Some(v) = take_value("--assets-root", &mut i) {
                    if v.trim().is_empty() {
                        log::error!("--assets-root 需要非空目录");
                    } else {
                        cli.assets_root = Some(PathBuf::from(v));
                    }
                }
                continue;
            }
            "--dump-asset" => {
                if let Some(v) = take_value("--dump-asset", &mut i) {
                    cli.dump_asset = Some(PathBuf::from(v));
                }
                continue;
            }
            "--dump-anim" => {
                if let Some(v) = take_value("--dump-anim", &mut i) {
                    cli.dump_anim = Some(PathBuf::from(v));
                }
                continue;
            }
            "--anim-clip" | "--clip" => {
                if let Some(v) = take_value("--anim-clip", &mut i) {
                    cli.anim_clip = Some(v);
                }
                continue;
            }
            "--show-bones" | "--bones" => {
                cli.show_bones = true;
                i += 1;
                continue;
            }
            "--run-seconds" | "--seconds" => {
                if let Some(v) = take_value("--run-seconds", &mut i) {
                    match v.trim().parse::<f32>() {
                        Ok(secs) if secs > 0.0 => cli.run_seconds = Some(secs),
                        _ => log::error!("--run-seconds 取值应为正数秒，收到 `{v}`"),
                    }
                }
                continue;
            }
            other if other.starts_with("--") => {
                log::warn!("未知选项 `{arg}`，已忽略（--help 查看用法）");
                i += 1;
                continue;
            }
            _ => {}
        }
        positional.push(arg);
        i += 1;
    }

    let scene_path = positional
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("assets/scene.json"));
    let script_path = positional
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("scripts/main.rhai"));

    // --dump-asset：无窗口路径，导入后打印报告直接退出
    if let Some(path) = cli.dump_asset.clone() {
        log::info!("资产导入: {}", path.display());
        match r3e_asset::dump_asset(&path) {
            Ok(report) => {
                println!("{report}");
                return Ok(());
            }
            Err(e) => {
                eprintln!("资产导入失败: {e}");
                std::process::exit(1);
            }
        }
    }

    // --dump-anim：P3 数值验收，打印轨道采样表后退出
    if let Some(path) = cli.dump_anim.clone() {
        match app::dump_anim(&path, cli.anim_clip.as_deref()) {
            Ok(report) => {
                println!("{report}");
                return Ok(());
            }
            Err(e) => {
                eprintln!("动画导入失败: {e}");
                std::process::exit(1);
            }
        }
    }

    log::info!("项目根目录: {}", root.display());
    log::info!("场景: {}", scene_path.display());
    log::info!("脚本: {}", script_path.display());

    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = app::EngineApp::new(scene_path, script_path, cli);
    event_loop.run_app(&mut app)?;
    Ok(())
}
