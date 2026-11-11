//! 形变（morph target，扩展三十六）的**像素级验收**：不比对基准图，只比对「同一份
//! 资产、唯一变量是权重」的两帧。
//!
//! 为什么不用基准图：形变的验收问题是「权重有没有真的走完 Scene → 实例缓冲 → 顶点
//! 着色器」，而不是「画面是否与某张固定图一致」。两帧自比对这个口径与显卡、驱动、
//! 排序、抗锯齿都无关，暴露出问题时也不用更新基准图，永远不会因为「换了个驱动」变红。
//!
//! 三道断言，分别堵三种真实踩过的坑：
//!
//! 1. **两帧都有物体**（非背景像素够多）—— 夹具写错时（绕序反、缺材质）画面会是纯
//!    背景色，两帧「完全一致」会让 0% 差异看起来像「形变没生效」，实际是根本没画出来；
//! 2. **差异比例落在区间内**（太小 = 形变没进 GPU，太大 = 渲染错乱）；
//! 3. **形变方向对**（顶点的 x 位移让包围盒左边界外扩）—— 只断言「有差异」的话，
//!    一个把权重错接到别的形状上的实现也能过。
//!
//! 闸门与 `visual_regression.rs` 一致：`R3E_VISUAL_REGRESSION=1` 才跑，进程退出码 2
//! 视为「没有显卡」跳过；`R3E_VISUAL_STRICT=1` 时禁止跳过。

use std::path::{Path, PathBuf};
use std::process::Command;

use r3e_asset::visual_regression::RgbaFrame;

/// 与 `visual_regression.rs` 保持同一口径，截图尺寸不另立标准。
const CAPTURE_WIDTH: u32 = 640;
const CAPTURE_HEIGHT: u32 = 360;

/// 首帧后处理目标 / 字形图集还没热，拍到第 4 帧为止。
const CAPTURE_AFTER_FRAME: u32 = 4;

/// 夹具背景色（`scene.json` 的 `background` 经 sRGB 编码后的实际像素值）。
const BACKGROUND: [u8; 3] = [39, 39, 48];

/// 单像素与背景色的通道差超过它才算「画了东西」。
const OBJECT_DELTA: u8 = 6;

/// 物体至少占多少像素才认为「画出来了」。四边形在 640x360 下占 3 万像素左右，
/// 阈值取 2000 只为挡住「几乎全黑 / 只画了个角」这类夹具事故。
const MIN_OBJECT_PIXELS: usize = 2000;

/// 权重 0 与权重 1 的差异比例下限：实测 13.8%，留足余量到 5%。
const MIN_DIFF_RATIO: f32 = 0.05;

/// 差异比例上限：形变只是把四边形一侧拉开，不该让半张图以上都变。
const MAX_DIFF_RATIO: f32 = 0.6;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/r3e-app 的上两级就是仓库根")
        .to_path_buf()
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("morph_deform")
}

fn out_dir() -> PathBuf {
    repo_root().join("target").join("morph_deform")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

/// 跑一次 `r3e`，截一张图。返回错误字符串表示「跳过」，而不是失败。
fn capture(script: &str, out: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out_dir()).map_err(|err| format!("建输出目录失败: {err}"))?;
    let _ = std::fs::remove_file(out);

    let status = Command::new(env!("CARGO_BIN_EXE_r3e"))
        .current_dir(repo_root())
        .arg(fixture_dir().join("scene.json"))
        .arg(fixture_dir().join(script))
        .arg("--window-size")
        .arg(format!("{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}"))
        .arg("--capture-frame")
        .arg(out)
        .arg("--capture-after")
        .arg(CAPTURE_AFTER_FRAME.to_string())
        .status()
        .map_err(|err| format!("启动 r3e 失败: {err}"))?;

    match status.code() {
        Some(0) => {}
        // 2 = 渲染器 / 截图链路不可用（无 GPU、窗口建不出来）
        Some(2) => return Err("渲染器不可用（无 GPU 或窗口创建失败）".to_string()),
        other => return Err(format!("r3e 异常退出: {other:?}")),
    }
    if !out.is_file() {
        return Err(format!("进程退出 0 但没有落盘截图: {}", out.display()));
    }
    Ok(out.to_path_buf())
}

/// 「画了东西」的像素数与包围盒（x0, y0, x1, y1）。
fn object_extent(frame: &RgbaFrame) -> (usize, Option<(u32, u32, u32, u32)>) {
    let mut count = 0usize;
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let Some(px) = frame.pixel(x, y) else {
                continue;
            };
            let is_object = (0..3).any(|c| px[c].abs_diff(BACKGROUND[c]) > OBJECT_DELTA);
            if !is_object {
                continue;
            }
            count += 1;
            bbox = Some(match bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    (count, bbox)
}

#[test]
fn morph_weight_moves_vertices() {
    let strict = env_flag("R3E_VISUAL_STRICT");
    if !env_flag("R3E_VISUAL_REGRESSION") {
        let reason = "设置 R3E_VISUAL_REGRESSION=1 后重跑（需要 GPU）。\
                      纯逻辑侧由 r3e-render 的 morph_table_tests 与 r3e-asset 的 \
                      gltf_morph_import 覆盖。";
        if strict {
            panic!("严格模式下闸门必须开着，但 R3E_VISUAL_REGRESSION 没设：{reason}");
        }
        eprintln!("跳过形变像素验收：{reason}");
        return;
    }

    let skip_or_panic = |reason: String| {
        if strict {
            panic!("严格模式（R3E_VISUAL_STRICT=1）下禁止跳过：{reason}");
        }
        eprintln!("跳过形变像素验收：{reason}");
    };

    let zero = match capture("weight_zero.rhai", &out_dir().join("weight_zero.png")) {
        Ok(path) => path,
        Err(reason) => return skip_or_panic(reason),
    };
    let one = match capture("weight_one.rhai", &out_dir().join("weight_one.png")) {
        Ok(path) => path,
        Err(reason) => return skip_or_panic(reason),
    };

    let zero_frame = RgbaFrame::load_png(&zero).expect("权重 0 的截图应可解码");
    let one_frame = RgbaFrame::load_png(&one).expect("权重 1 的截图应可解码");

    // 断言 1：两种权重下都要真的画出物体 —— 否则「0% 差异」会被误读成形变没生效
    let (zero_px, zero_box) = object_extent(&zero_frame);
    let (one_px, one_box) = object_extent(&one_frame);
    assert!(
        zero_px >= MIN_OBJECT_PIXELS,
        "权重 0 的画面只有 {zero_px} 个非背景像素（阈值 {MIN_OBJECT_PIXELS}）：\
         夹具根本没画出来（绕序反 / 缺材质 / 相机看不到），这时比像素差没有意义。\
         截图: {}",
        zero.display()
    );
    assert!(
        one_px >= MIN_OBJECT_PIXELS,
        "权重 1 的画面只有 {one_px} 个非背景像素（阈值 {MIN_OBJECT_PIXELS}），截图: {}",
        one.display()
    );

    // 断言 2：差异比例落在区间内
    let diff = r3e_asset::visual_regression::compare(
        &zero_frame,
        &one_frame,
        r3e_asset::visual_regression::Tolerance::exact(),
    )
    .expect("两张同尺寸截图应可比对");
    assert!(
        diff.bad_ratio >= MIN_DIFF_RATIO,
        "权重 0 → 1 只改动了 {:.2}% 的像素（下限 {:.2}%）：形变没有真正影响顶点。\n{}",
        diff.bad_ratio * 100.0,
        MIN_DIFF_RATIO * 100.0,
        diff.summary()
    );
    assert!(
        diff.bad_ratio <= MAX_DIFF_RATIO,
        "权重 0 → 1 改动了 {:.2}% 的像素（上限 {:.2}%）：形变幅度失控，像是读错了增量。\n{}",
        diff.bad_ratio * 100.0,
        MAX_DIFF_RATIO * 100.0,
        diff.summary()
    );

    // 断言 3：位移方向对。夹具里左侧两个顶点带 (-2, 1.8, 0) 的增量，
    // 权重 1 时包围盒左边界必须明显左移（屏幕 x 减小）。
    let (zx0, _, _, _) = zero_box.expect("权重 0 有物体必有包围盒");
    let (ox0, _, _, _) = one_box.expect("权重 1 有物体必有包围盒");
    assert!(
        ox0 + 20 < zx0,
        "形变方向不对：包围盒左边界从 x={zx0} 只变到 x={ox0}（应至少左移 20 像素）。\
         说明权重喂给了错误的形状，或增量的量纲 / 分量被读串。"
    );
}

/// 夹具自检：**纯 CPU**，不建窗口、不需要显卡。
///
/// 上面那条用例在无 GPU 的机器上必然跳过。若夹具被误删 / 脚本被改成不调
/// `morph_set`，跳过就会变成「永远绿」。这条用例把夹具本身钉住：文件在、
/// glb 里确实声明了 `targets`、两个脚本一个写 0 一个写 1。
#[test]
fn fixture_declares_one_morph_target() {
    let dir = fixture_dir();
    for name in [
        "scene.json",
        "morph_face.glb",
        "weight_zero.rhai",
        "weight_one.rhai",
    ] {
        let path = dir.join(name);
        assert!(path.is_file(), "夹具缺失: {}", path.display());
    }

    let glb = std::fs::read(dir.join("morph_face.glb")).expect("夹具 glb 应可读");
    // glTF 的 JSON chunk 是纯文本，直接按字节找关键字即可，不必为此引入解析依赖
    let text = String::from_utf8_lossy(&glb);
    assert!(
        text.contains("\"targets\""),
        "夹具 glb 没有声明 morph targets，形变用例会静默通过"
    );
    assert!(text.contains("POSITION"), "夹具 glb 缺 POSITION 访问器");

    let zero = std::fs::read_to_string(dir.join("weight_zero.rhai")).expect("脚本应可读");
    let one = std::fs::read_to_string(dir.join("weight_one.rhai")).expect("脚本应可读");
    assert!(
        zero.contains("morph_set(\"Face\", 0, 0.0)"),
        "对照组必须把权重写成 0"
    );
    assert!(
        one.contains("morph_set(\"Face\", 0, 1.0)"),
        "试验组必须把权重写成 1"
    );
}
