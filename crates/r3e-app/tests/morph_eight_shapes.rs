//! 扩展三十九（多组 `vec4` 权重）的**像素级验收**：8 个形状里，第 5 个（下标 4）
//! 必须由第 2 组权重 `w1.x` 驱动，而不是被折回第 1 组。
//!
//! 为什么非得上真机：单测只能证明「扁平权重按序切成了两组」，着色器仍可能把
//! `w1.x` 读到形状 0 的槽位上 —— 那种错位在类型系统里完全合法，`cargo test`
//! 也全绿，只有像素会露馅：设形状 4 的权重，画面却按形状 0 在变形。
//!
//! 四态对照，同一夹具、唯一变量是脚本设了哪个形状的权重：
//!
//! * `raw`    —— 一个权重都不设，活物基线（`morph=0/288f`）；
//! * `shape0` —— 第 1 组首分量，左边两顶点左拉；
//! * `shape4` —— 第 2 组首分量，上边两顶点上顶，**必须与 `shape0` 明显不同**；
//! * `shape7` —— 第 2 组末分量、位移全 0，**差 0 像素**才算「权重进了对的槽位但没有位移」。
//!
//! 门槛与 `visual_regression.rs` 一致：`R3E_VISUAL_REGRESSION=1` 才跑，退出码 2
//! 视为「没有显卡」跳过；`R3E_VISUAL_STRICT=1` 时禁止跳过。

use std::path::{Path, PathBuf};
use std::process::Command;

use r3e_asset::visual_regression::{compare, RgbaFrame, Tolerance};

/// 与 `visual_regression.rs` 同一口径，截图尺寸不另立标准。
const CAPTURE_WIDTH: u32 = 640;
const CAPTURE_HEIGHT: u32 = 360;

/// 首帧后处理目标准 / 字形图集还没热，拍到第 4 帧为止。
const CAPTURE_AFTER_FRAME: u32 = 4;

/// 夹具背景色（`scene.json` 的 `background` 经 sRGB 编码后的实际像素值）。
const BACKGROUND: [u8; 3] = [39, 39, 48];

/// 单像素与背景色的通道差超过它才算「画了东西」。
const OBJECT_DELTA: u8 = 6;

/// 四边形在 640x360 下占 1.5 万像素左右，阈值取 2000 只为止住「几乎全黑」这类夹具事故。
const MIN_OBJECT_PIXELS: usize = 2000;

/// 形状 0 与形状 4 的差异实测 5.34% / 6.42% / 两者互差 10.77%，下限定 2% 留足余量。
const MIN_DIFF_RATIO: f32 = 0.02;

/// 形变只是把四边形的一边拉出去，不该让半张图以上都变。
const MAX_DIFF_RATIO: f32 = 0.6;

/// 表长：4 顶点 × 9 f32（位置/法线/切线）× 8 个形状 = 288。
///
/// 这个数同时钉住两件事：8 个形状全部进了显存表（没被容量截断）、每顶点步长没退化成
/// 4 形状时的 18。
const EXPECTED_TABLE_FLOATS: usize = 288;

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
        .join("morph_eight")
}

fn out_dir() -> PathBuf {
    repo_root().join("target").join("morph_eight_shapes")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

/// 一次真机截图的结果：像素 + 子进程日志（日志里有 `morph=` 探针行）。
struct Shot {
    frame: RgbaFrame,
    log: String,
}

/// 跑一次 `r3e`，截一张图。返回 `Err` 表示「跳过」（无 GPU），不是失败。
fn capture(script: &str, out: &Path) -> Result<Shot, String> {
    std::fs::create_dir_all(out_dir()).map_err(|err| format!("建输出目录失败: {err}"))?;

    let output = Command::new(env!("CARGO_BIN_EXE_r3e"))
        .current_dir(repo_root())
        .arg(fixture_dir().join("scene.json"))
        .arg(fixture_dir().join(script))
        .arg("--window-size")
        .arg(format!("{CAPTURE_WIDTH}x{CAPTURE_HEIGHT}"))
        .arg("--capture-frame")
        .arg(out)
        .arg("--capture-after")
        .arg(CAPTURE_AFTER_FRAME.to_string())
        .output()
        .map_err(|err| format!("启动 r3e 失败: {err}"))?;

    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    match output.status.code() {
        Some(0) => {}
        // 2 = 渲染器 / 截图链路不可用（无 GPU、窗口建不出来）
        Some(2) => return Err("渲染器不可用（无 GPU 或窗口创建失败）".to_string()),
        other => return Err(format!("r3e 异常退出: {other:?}\n{log}")),
    }
    if !out.is_file() {
        return Err(format!("进程退出 0 但没落盘截图: {}", out.display()));
    }
    let frame = RgbaFrame::load_png(out).map_err(|err| format!("读截图失败: {err:?}"))?;
    Ok(Shot { frame, log })
}

/// 「画了东西」的像素数。
fn object_pixels(frame: &RgbaFrame) -> usize {
    let mut count = 0usize;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let Some(px) = frame.pixel(x, y) else {
                continue;
            };
            let delta = (0..3)
                .map(|i| px[i].abs_diff(BACKGROUND[i]))
                .max()
                .unwrap_or(0);
            if delta > OBJECT_DELTA {
                count += 1;
            }
        }
    }
    count
}

/// 两帧的差异比例（容差 1 个通道，几乎逐位比较）。
fn diff_ratio(left: &RgbaFrame, right: &RgbaFrame) -> f32 {
    let report = compare(left, right, Tolerance::new(1, 0.0)).expect("同尺寸截图比较不该失败");
    report.bad_ratio
}

/// 从日志里抠 `morph=<实例数>/<表长>f` 这一行，返回 `(实例数, 表长)`。
fn morph_probe(log: &str) -> Option<(usize, usize)> {
    let line = log.lines().find(|line| line.contains("morph="))?;
    let tail = &line[line.find("morph=")? + "morph=".len()..];
    let (instances, rest) = tail.split_once('/')?;
    let table = rest.split('f').next()?;
    Some((instances.parse().ok()?, table.parse().ok()?))
}

fn shot_or_skip(script: &str) -> Option<Shot> {
    let out = out_dir().join(format!("{}.png", script.replace('.', "_")));
    match capture(script, &out) {
        Ok(shot) => Some(shot),
        Err(reason) => {
            assert!(
                !env_flag("R3E_VISUAL_STRICT"),
                "跳过截图（{script}）：{reason}（R3E_VISUAL_STRICT=1 时禁止跳过）"
            );
            eprintln!("跳过截图（{script}）：{reason}");
            None
        }
    }
}

/// 形状 4（第 2 组权重）必须真的驱动顶点：设它的权重，画面要变，且变得跟形状 0 不一样。
///
/// 这条是整个扩展的验收核心 —— 着色器若把 `w1.x` 错接到形状 0，这里两帧会完全重合。
#[test]
fn shape_five_is_driven_by_the_second_weight_group() {
    if !env_flag("R3E_VISUAL_REGRESSION") {
        eprintln!("跳过：需要 R3E_VISUAL_REGRESSION=1");
        return;
    }
    let Some(raw) = shot_or_skip("raw.rhai") else {
        return;
    };
    let Some(first) = shot_or_skip("shape0.rhai") else {
        return;
    };
    let Some(fifth) = shot_or_skip("shape4.rhai") else {
        return;
    };

    // 1. 两张图里都得有物件，否则「一样」可能只是「都没画出来」
    let raw_pixels = object_pixels(&raw.frame);
    assert!(
        raw_pixels >= MIN_OBJECT_PIXELS,
        "基线只画了 {raw_pixels} 个像素，夹具怀疑没被渲染"
    );

    // 2. 探针：8 个形状全在表里（288 = 4 顶点 × 9 f32 × 8），且两次都算「真的在形变」
    for (label, shot) in [("raw", &raw), ("shape0", &first), ("shape4", &fifth)] {
        let (instances, table) =
            morph_probe(&shot.log).unwrap_or_else(|| panic!("{label} 的日志里没有 morph= 探针行"));
        assert_eq!(
            table, EXPECTED_TABLE_FLOATS,
            "{label} 的形变表长度不对（8 个形状应全部进表）"
        );
        let want = if label == "raw" { 0 } else { 1 };
        assert_eq!(instances, want, "{label} 的 morph_instances 不对");
    }

    // 3. 两个形状各自都要让画面动（第一组、第二组都在工作）
    let first_ratio = diff_ratio(&raw.frame, &first.frame);
    let fifth_ratio = diff_ratio(&raw.frame, &fifth.frame);
    for (label, ratio) in [("形状 0", first_ratio), ("形状 4", fifth_ratio)] {
        assert!(
            (MIN_DIFF_RATIO..=MAX_DIFF_RATIO).contains(&ratio),
            "{label} 对基线差异 {:.2}%，应落在 {:.0}%~{:.0}%",
            ratio * 100.0,
            MIN_DIFF_RATIO * 100.0,
            MAX_DIFF_RATIO * 100.0
        );
    }

    // 4. 核心断言：形状 4 与形状 0 是两个不同形状。错接权重时这一条必挂。
    let cross = diff_ratio(&first.frame, &fifth.frame);
    assert!(
        cross >= MIN_DIFF_RATIO,
        "形状 4 与形状 0 只差 {:.2}% —— 两组权重很可能接到了同一批槽位",
        cross * 100.0
    );

    eprintln!(
        "形变多组权重探针：表 {}f | 形状0 对基线 {:.2}% | 形状4 对基线 {:.2}% | 两者互差 {:.2}%",
        EXPECTED_TABLE_FLOATS,
        first_ratio * 100.0,
        fifth_ratio * 100.0,
        cross * 100.0
    );
}

/// 权重进了对的槽位、但那个形状位移全 0 时，画面必须**一位都不动**。
///
/// 这条堵的是「让所有形状都读到同一份增量」这类错法：那种实现下
/// 设形状 7 的权重也会画出形状 0 的形变，这里就会多出像素。
#[test]
fn zero_displacement_shape_changes_nothing() {
    if !env_flag("R3E_VISUAL_REGRESSION") {
        eprintln!("跳过：需要 R3E_VISUAL_REGRESSION=1");
        return;
    }
    let Some(raw) = shot_or_skip("raw.rhai") else {
        return;
    };
    let Some(last) = shot_or_skip("shape7.rhai") else {
        return;
    };

    let (instances, table) =
        morph_probe(&last.log).unwrap_or_else(|| panic!("shape7 的日志里没有 morph= 探针行"));
    assert_eq!(table, EXPECTED_TABLE_FLOATS, "形变表长度不该变");
    assert_eq!(instances, 1, "设了权重就该算在形变实例里（哪怕位移为 0）");

    let ratio = diff_ratio(&raw.frame, &last.frame);
    assert_eq!(
        ratio,
        0.0,
        "形状 7 没有位移，画面却有 {:.4}% 像素变化 —— 增量大概率串到了别的形状上",
        ratio * 100.0
    );
}
