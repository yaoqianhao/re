//! `capture.frame` 的像素级回归用例（**需要 GPU，默认跳过**）。
//!
//! 分工：
//!
//! * 比对逻辑（容差 / 差异统计 / 差异图）是纯 CPU 的，已在
//!   `r3e_asset::visual_regression` 里单测覆盖，跑在任何机器上都不需要显卡；
//! * 本文件只负责「产出一张新截图」这唯一需要 GPU 的一步——起一个 `r3e` 进程，
//!   用固定的窗口尺寸跑到约定帧后截图，再拿它和基准图比。
//!
//! 三道闸门（缺一不可，避免在没有显卡的 CI 上假失败）：
//!
//! 1. `R3E_VISUAL_REGRESSION=1` 才真正跑，否则直接跳过并打印原因；
//! 2. 进程退出码 2 = 渲染器 / 截图链路不可用（无 GPU、窗口创建失败），按跳过处理；
//! 3. 基准图缺失时不是「通过」也不是「失败」，而是明确报错提示怎么生成。
//!
//! 跳过只代表「这台机器没有显卡」。为了让 CI 里的跳过不等于放任，本文件另有一条
//! **纯 CPU** 用例 `baseline_png_is_present_with_capture_size`：它校验基准图存在、
//! 可解码、尺寸与截图口径一致，跑在任何 runner 上都不需要 GPU，红绿是真结论。
//!
//! 环境变量：
//!
//! * `R3E_VISUAL_REGRESSION=1`：打开本用例；
//! * `R3E_UPDATE_BASELINE=1`：把本次截图写/覆盖成基准图（首次建档或有意更新画面时用）；
//! * `R3E_VISUAL_CHANNEL_DELTA`：覆盖单通道容差（默认取库里的常量）；
//! * `R3E_VISUAL_BAD_RATIO`：覆盖坏点比例上限（默认取库里的常量）。

use std::path::{Path, PathBuf};
use std::process::Command;

use r3e_asset::visual_regression::{
    assert_png_matches, RgbaFrame, Tolerance, DEFAULT_BAD_PIXEL_RATIO, DEFAULT_CHANNEL_DELTA,
};

/// 基准图与截图统一的**物理像素**尺寸。
///
/// 刻意不用 1280x720：回归图要进仓库，640x360 已经能看出排序 / MSAA / 后处理的
/// 变化，而 PNG 体积只有四分之一。
const CAPTURE_WIDTH: u32 = 640;
const CAPTURE_HEIGHT: u32 = 360;

/// 截图取第几帧之后。首帧的后处理目标与字形图集还没热，拍下来不稳定。
const CAPTURE_AFTER_FRAME: u32 = 3;

/// 仓库根目录（`CARGO_MANIFEST_DIR` 是 `crates/r3e-app`）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/r3e-app 的上两级就是仓库根")
        .to_path_buf()
}

/// 基准图：跟着测试代码一起进版本库。
fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("baselines")
        .join(format!(
            "capture_frame_{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}.png"
        ))
}

/// 回归专用夹具：静止场景 + 空脚本。
///
/// 不复用 `assets/scene.json` + `scripts/main.rhai`：演示脚本会转动物件、每帧刷新
/// 帧率文本，同一帧号下的画面随运行时长相变，那种输入做基线会让用例「代码没改也红」。
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("visual_regression")
}

/// 实际图与差异产物的落盘目录：`target/` 下，天然不进版本库。
fn artifacts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("visual-regression")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
}

/// 容差：默认走库里的常量，允许环境变量按需放宽（比如换了显卡驱动）。
fn tolerance() -> Tolerance {
    let channel_delta = std::env::var("R3E_VISUAL_CHANNEL_DELTA")
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .unwrap_or(DEFAULT_CHANNEL_DELTA);
    let bad_pixel_ratio = std::env::var("R3E_VISUAL_BAD_RATIO")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .unwrap_or(DEFAULT_BAD_PIXEL_RATIO);
    Tolerance::new(channel_delta, bad_pixel_ratio).sanitized()
}

/// 起 `r3e` 进程拍一张图，返回 `Ok(路径)` / `Err(跳过原因)`。
///
/// 用真实的二进制而不是把渲染器塞进测试进程：截图链路的入口就是命令行，
/// 顺手把 `--window-size` / `--capture-frame` / `--capture-after` 三个参数也覆盖到。
fn capture_actual_frame() -> Result<PathBuf, String> {
    let out_dir = artifacts_dir();
    std::fs::create_dir_all(&out_dir).map_err(|err| format!("建产物目录失败: {err}"))?;
    let actual = out_dir.join(format!(
        "capture_frame_{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}.png"
    ));
    // 先删旧图：否则进程若中途挂了，上一轮的图会冒充本轮结果骗过比对。
    let _ = std::fs::remove_file(&actual);

    let root = repo_root();
    let fixture = fixture_dir();
    let status = Command::new(env!("CARGO_BIN_EXE_r3e"))
        .current_dir(&root)
        .arg(fixture.join("scene.json"))
        .arg(fixture.join("static.rhai"))
        .arg("--window-size")
        .arg(format!("{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}"))
        .arg("--capture-frame")
        .arg(&actual)
        .arg("--capture-after")
        .arg(CAPTURE_AFTER_FRAME.to_string())
        .status()
        .map_err(|err| format!("启动 r3e 失败: {err}"))?;

    match status.code() {
        Some(0) => {}
        // 2 = 渲染器 / 截图链路不可用（无 GPU、窗口建不出来），按跳过处理
        Some(2) => return Err("渲染器不可用（无 GPU 或窗口创建失败），跳过".to_string()),
        other => return Err(format!("r3e 异常退出: {other:?}")),
    }
    if !actual.is_file() {
        return Err(format!("进程退出 0 但没有落盘截图: {}", actual.display()));
    }
    Ok(actual)
}

#[test]
fn captured_frame_matches_baseline() {
    let strict = env_flag("R3E_VISUAL_STRICT");

    if !env_flag("R3E_VISUAL_REGRESSION") {
        let reason = "设置 R3E_VISUAL_REGRESSION=1 后重跑（需要 GPU）。\
                      纯逻辑比对语义由 r3e_asset::visual_regression 的单测覆盖。";
        if strict {
            panic!(
                "严格模式（R3E_VISUAL_STRICT=1）下闸门必须开着，但 R3E_VISUAL_REGRESSION 没设。\
                 这条用例跑在带 GPU 的 runner 上，跳过就等于没跑：{reason}"
            );
        }
        eprintln!("跳过 capture.frame 像素回归：{reason}");
        return;
    }

    let actual = match capture_actual_frame() {
        Ok(path) => path,
        Err(reason) => {
            if strict {
                // 严格模式 = 「我保证这台机器有 GPU，请把结果当真」。这时拍不到图不是环境限制，
                // 而是显卡 / 驱动 / 窗口创建坏了，必须红——否则这道门又是"跳过即通过"。
                panic!(
                    "严格模式（R3E_VISUAL_STRICT=1）下禁止跳过：{reason}\n\
                     该模式只应在带 GPU 的 runner 上开启（见 docs/CI_GPU_RUNNER.md）。"
                );
            }
            eprintln!("跳过 capture.frame 像素回归：{reason}");
            return;
        }
    };

    let baseline = baseline_path();
    let update = env_flag("R3E_UPDATE_BASELINE");
    if !baseline.is_file() {
        assert!(
            update,
            "基准图缺失: {}\n首次建档请用 R3E_VISUAL_REGRESSION=1 R3E_UPDATE_BASELINE=1 重跑。",
            baseline.display()
        );
        std::fs::create_dir_all(baseline.parent().expect("基准图必有父目录")).expect("建基准目录");
        std::fs::copy(&actual, &baseline).expect("写入基准图");
        eprintln!("已生成基准图: {}", baseline.display());
        return;
    }

    let report = assert_png_matches(&baseline, &actual, tolerance(), artifacts_dir())
        .expect("基准图与实际图都应可读、且尺寸一致");
    if !report.passed() && update {
        std::fs::copy(&actual, &baseline).expect("覆盖基准图");
        eprintln!("基准图已更新: {}", baseline.display());
        return;
    }
    assert!(
        report.passed(),
        "capture.frame 像素回归失败（基准 {} / 实际 {}）\n{}\
         如确认本次画面变化是预期内的，用 R3E_UPDATE_BASELINE=1 重跑更新基准图。",
        baseline.display(),
        actual.display(),
        report.summary()
    );
}

/// 基准图自身的体检：**纯 CPU，不建窗口、不需要显卡**。
///
/// 上面那条用例在没有 GPU 的机器（含 GitHub 托管的 `windows-latest`）上必然跳过，
/// 于是 CI 里「基准图被误删 / 尺寸漂移 / 文件被截断」这类退化就没人管了。
/// 这条用例把那一半补上：只要基准 PNG 不见了或者尺寸不是 `CAPTURE_WIDTH × CAPTURE_HEIGHT`，
/// 任何 runner 都会直接红——它保证「跳过」只意味着没有显卡，而不是没有基线。
#[test]
fn baseline_png_is_present_with_capture_size() {
    let baseline = baseline_path();
    assert!(
        baseline.is_file(),
        "基准图缺失: {}\n首次建档请用 R3E_VISUAL_REGRESSION=1 R3E_UPDATE_BASELINE=1 在本机重跑。",
        baseline.display()
    );
    let frame = RgbaFrame::load_png(&baseline)
        .unwrap_or_else(|err| panic!("基准图应可解码（{}）: {err}", baseline.display()));
    assert_eq!(
        (frame.width, frame.height),
        (CAPTURE_WIDTH, CAPTURE_HEIGHT),
        "基准图尺寸必须与截图口径一致，否则真跑时必然「尺寸不一致」报错"
    );
    assert_eq!(
        frame.pixel_count(),
        (CAPTURE_WIDTH as usize) * (CAPTURE_HEIGHT as usize),
        "基准图像素数应与声明尺寸吻合"
    );
}
