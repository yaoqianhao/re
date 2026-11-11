//! 扩展四十（逐实例形状掩码）的**像素级验收**：同一个网格上，掩码能挡住
//! 「权重明明写着 1.0」的形状。
//!
//! 为什么必须上真机：单测只能证明「掩码按形状数裁剪、全关时摘掉形变」，
//! 着色器仍可能照旧把 8 个形状全累加进去 —— 那时 `cargo test` 全绿，只有像素会
//! 露馅：掩码写着「形状 0 关闭」，画面却还按形状 0 变形。
//!
//! 三态对照，同一夹具、唯一变量是掩码：
//!
//! * `shape0`         —— 形状 0 权重 1.0、掩码全开，左边两顶点左拉（对照，必须动）；
//! * `mask_first_off` —— 与 `shape0` **同一份权重**，只多一行「关掉形状 0」，
//!   画面必须回到基线。这条是「掩码」与「权重置 0 假装」的分界线：
//!   权重在日志里仍是 1.0，被挡住的只是它参与形变的资格；
//! * `mask_all_off`   —— 掩码写 0，整段摘掉形变，`morph=` 实例数掉到 0。
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

/// 形状 0 对基线的差异实测 5.34%，下限定 2% 留足余量。
const MIN_DIFF_RATIO: f32 = 0.02;

/// 表长：4 顶点 × 9 f32（位置/法线/切线）× 8 个形状 = 288。
///
/// 掩码只影响「实例用不用」，不该让表变小 —— 表是网格级的，同网格的其它实例
/// 可能还开着全部形状。
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
        .join("morph_mask")
}

fn out_dir() -> PathBuf {
    repo_root().join("target").join("morph_mask")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

/// 一次真机截图的结果：像素 + 子进程日志（日志里有 `morph=` / `mask=` 探针行）。
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

/// 从日志里抠 `morph=<实例数>/<表长>f` 这一行。
fn morph_probe(log: &str) -> Option<(usize, usize)> {
    let line = log.lines().find(|line| line.contains("morph="))?;
    let tail = &line[line.find("morph=")? + "morph=".len()..];
    let (instances, rest) = tail.split_once('/')?;
    let table = rest.split('f').next()?;
    Some((instances.parse().ok()?, table.parse().ok()?))
}

/// 从日志里抠 `mask=<被整段摘掉的实例数>/<被挡住的形状槽位数>`（注意探针里没有空格，
/// 脚本自己的 `log("mask = ...")` 带空格，不会误匹配）。
fn mask_probe(log: &str) -> Option<(usize, usize)> {
    let line = log.lines().find(|line| line.contains("mask="))?;
    let tail = &line[line.find("mask=")? + "mask=".len()..];
    let (masked, shapes) = tail.split_once('/')?;
    let shapes = shapes.split_whitespace().next()?;
    Some((masked.parse().ok()?, shapes.parse().ok()?))
}

/// 脚本自己打的日志行（`[script] mask = ...`），用来钉住「权重确实还是 1.0」。
fn script_line(log: &str) -> String {
    log.lines()
        .find(|line| line.starts_with("[script] mask = "))
        .unwrap_or("")
        .to_string()
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

/// 掩码挡住一个**权重仍在**的形状时，画面必须回到基线。
///
/// 这条是整个扩展的验收核心：把「关掉」做成「权重置 0」的实现也能让画面不动，
/// 但它做不到「权重还是 1.0」——日志里 `w0 = 1.0` 与 `visible0 = false` 同时成立，
/// 加上像素逐位回到基线，才证明是掩码在起作用。
#[test]
fn mask_hides_a_shape_that_still_carries_weight() {
    if !env_flag("R3E_VISUAL_REGRESSION") {
        eprintln!("跳过：需要 R3E_VISUAL_REGRESSION=1");
        return;
    }
    let Some(raw) = shot_or_skip("raw.rhai") else {
        return;
    };
    let Some(moving) = shot_or_skip("shape0.rhai") else {
        return;
    };
    let Some(masked) = shot_or_skip("mask_first_off.rhai") else {
        return;
    };

    let raw_pixels = object_pixels(&raw.frame);
    assert!(
        raw_pixels >= MIN_OBJECT_PIXELS,
        "基线只画了 {raw_pixels} 个像素，夹具怀疑没被渲染"
    );

    // 1. 对照组：形状 0 权重 1.0 且掩码全开，画面必须动
    let moving_ratio = diff_ratio(&raw.frame, &moving.frame);
    assert!(
        moving_ratio >= MIN_DIFF_RATIO,
        "形状 0 开着时对基线只差 {:.2}% —— 对照没成立，后面的「不动」就没有意义",
        moving_ratio * 100.0
    );

    // 2. 探针：掩码只清掉第 0 位（255 → 254），权重仍是 1.0
    let line = script_line(&masked.log);
    assert!(
        line.contains("mask = 254"),
        "掩码应是 254（只关形状 0），实际日志行：{line}"
    );
    assert!(
        line.contains("w0 = 1.0"),
        "形状 0 的权重必须还是 1.0 —— 掩码不是把权重清零，实际：{line}"
    );
    assert!(
        line.contains("visible0 = false"),
        "脚本应读到形状 0 已关闭，实际：{line}"
    );

    // 3. 渲染统计：半掩的实例不走「整段摘掉」，但挡住的槽位要记进读数
    let (instances, table) = morph_probe(&masked.log)
        .unwrap_or_else(|| panic!("mask_first_off 的日志里没有 morph= 探针行"));
    assert_eq!(
        table, EXPECTED_TABLE_FLOATS,
        "形变表是网格级的，掩码不该缩表"
    );
    assert_eq!(
        instances, 0,
        "形状 0 被挡住后这个实例没有可用权重，不应算在形变实例里"
    );
    let (dropped, hidden) = mask_probe(&masked.log)
        .unwrap_or_else(|| panic!("mask_first_off 的日志里没有 mask= 探针行"));
    assert_eq!(dropped, 0, "半掩不摘实例，只清权重");
    assert_eq!(hidden, 1, "挡住 1 个「权重非 0 却被掩码关掉」的槽位");

    // 4. 核心断言：画面逐位回到基线
    let ratio = diff_ratio(&raw.frame, &masked.frame);
    assert_eq!(
        ratio,
        0.0,
        "掩码关掉形状 0 后画面仍有 {:.4}% 像素变化 —— 着色器仍在累加被掩码挡住的形状",
        ratio * 100.0
    );

    eprintln!(
        "掩码探针：形状0 对基线 {:.2}% | 掩码挡住后 {:.4}% | 挡住槽位 {} 个",
        moving_ratio * 100.0,
        ratio * 100.0,
        hidden
    );
}

/// 掩码整块写 0 时，该实例彻底不参与形变：`morph=` 实例数掉到 0、画面回到基线。
///
/// 这是扩展四十相对「权重置 0」唯一能省下真金白银的路径 —— 着色器在
/// `imeta.w == 0` 处提前返回，8 次权重比较全不做。
#[test]
fn fully_closed_mask_drops_the_morph_path() {
    if !env_flag("R3E_VISUAL_REGRESSION") {
        eprintln!("跳过：需要 R3E_VISUAL_REGRESSION=1");
        return;
    }
    let Some(raw) = shot_or_skip("raw.rhai") else {
        return;
    };
    let Some(closed) = shot_or_skip("mask_all_off.rhai") else {
        return;
    };

    let line = script_line(&closed.log);
    assert!(
        line.contains("mask = 0") && line.contains("effective = 0"),
        "掩码应整块归零，实际：{line}"
    );
    assert!(
        line.contains("w4 = 1.0"),
        "被摘掉的是形变资格，权重本身要留着（脚本可随时把掩码开回来），实际：{line}"
    );

    let (instances, table) = morph_probe(&closed.log)
        .unwrap_or_else(|| panic!("mask_all_off 的日志里没有 morph= 探针行"));
    assert_eq!(table, EXPECTED_TABLE_FLOATS, "表不该因为一个实例关掉而缩水");
    assert_eq!(instances, 0, "整块关闭后不该再有形变实例");

    let (dropped, hidden) =
        mask_probe(&closed.log).unwrap_or_else(|| panic!("mask_all_off 的日志里没有 mask= 探针行"));
    assert_eq!(dropped, 1, "应恰好摘掉 1 个实例");
    assert_eq!(hidden, 1, "挡住的槽位数 = 形状 4 那一个非零权重");

    let ratio = diff_ratio(&raw.frame, &closed.frame);
    assert_eq!(ratio, 0.0, "形变被整段摘掉后画面仍差 {:.4}%", ratio * 100.0);
}

/// 越界掩码按网格形状数裁剪：脚本写 65535，读回来必须是 255。
///
/// 不裁的话「第 12 个形状启用中」会被记进统计，形变表那一段却根本不存在。
#[test]
fn mask_is_clipped_to_mesh_shape_count_on_device() {
    if !env_flag("R3E_VISUAL_REGRESSION") {
        eprintln!("跳过：需要 R3E_VISUAL_REGRESSION=1");
        return;
    }
    let Some(clipped) = shot_or_skip("mask_clip.rhai") else {
        return;
    };

    let line = script_line(&clipped.log);
    assert!(
        line.contains("mask = 255"),
        "8 个形状的网格上，65535 必须被裁成 255，实际：{line}"
    );
    assert!(
        line.contains("effective = 255"),
        "`morph_mask_set` 的返回值也应是裁剪后的生效值，实际：{line}"
    );
    assert!(
        line.contains("count = 8"),
        "夹具应仍有 8 个形状（表长 288 才成立），实际：{line}"
    );

    let (_, table) =
        morph_probe(&clipped.log).unwrap_or_else(|| panic!("mask_clip 的日志里没有 morph= 探针行"));
    assert_eq!(table, EXPECTED_TABLE_FLOATS, "8 个形状应全部进表");
}
