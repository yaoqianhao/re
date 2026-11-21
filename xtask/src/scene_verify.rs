//! `cargo xtask scene-verify`（M1-S4-F，F-5）：拿三层验证器给夹具做体检。
//!
//! 这是 Gate 1 的**可执行落点**：`assets/verify/expect.json` 写好「哪个场景必须报哪条码、
//! 哪个场景必须干净」，本命令逐条跑 [`VerifyReport`] 并比对。夹具与期望分离，是为了让
//! 「改一处几何就变成正向对照」这件事只看 JSON，不动门禁代码。
//!
//! 三条口径（与 `docs/m1/M1_S4F_PLAN.md` §5 一致）：
//!
//! 1. **`unchecked` 必须为空**：`xtask` 手上有完整装配条件（物理后端申报 `shape_cast`、
//!    导航逐场景烘焙），一条「没检查」都不该出现。出现了就说明夹具或后端退化了，
//!    这时「没报错」不能算过——`passed()` 只看 error，`unchecked` 得单独看。
//! 2. **必报码必须带坐标**：`location` 为空等于退回「布尔式体检」，验收要求的是能定位。
//! 3. **`passed` 与期望逐字对齐**：反例该红就红（否则夹具没抓住问题），该绿就绿
//!    （否则就是「永远报错」的假红）。注意 `must` 与 `passed` 是两件事——`support.floating`
//!    按 S4-E 的口径是 `warn` 级：必须报出、但不拦 `passed`，所以 `floating_bad` 的期望是
//!    「报出悬空 + `passed = true`」，而不是想当然地写成红。
//!
//! 物理后端用内置 [`BuiltinBackend`] 而不是 rapier：它的 `shape_cast` / `overlap` 是
//! 解析法真实可查，且零外部依赖——门禁跑在每次 `verify` 里，不能为此多等链接时间。

use std::fs;
use std::path::Path;

use r3e_core::Vec3;
use r3e_editor_host::{Layer, VerifyReport};
use r3e_nav::Nav;
use r3e_physics::{BuiltinBackend, Physics};

use crate::read_json;

/// 夹具目录（相对仓库根）。
const DIR: &str = "assets/verify";

pub fn run(root: &Path) -> i32 {
    let dir = root.join(DIR);
    let spec = read_json(&dir.join("expect.json"));
    let Some(cases) = spec["cases"].as_array() else {
        println!("[scene-verify] {} 缺 cases 数组", DIR_MSG);
        return 1;
    };

    println!(
        "[scene-verify] {} 个夹具（物理后端 builtin，导航逐场景烘焙）",
        cases.len()
    );

    let mut failures: Vec<String> = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let file = case["scene"].as_str().unwrap_or_default().to_string();
        let why = case["why"].as_str().unwrap_or("").to_string();
        let problems = check_case(&dir, case, &file);
        if problems.is_empty() {
            println!("  ✓ {file:<22} {why}");
        } else {
            println!("  × {file:<22} {why}");
            for p in &problems {
                println!("      {p}");
            }
            failures.push(format!("{}（{}）", file, i + 1));
        }
    }

    if failures.is_empty() {
        println!(
            "[scene-verify] ok：{} 个夹具全部符合期望（反例报得出坐标，正向对照干净）",
            cases.len()
        );
        0
    } else {
        println!(
            "[scene-verify] FAIL：{} 个夹具不符 —— {}",
            failures.len(),
            failures.join("、")
        );
        1
    }
}

/// 单个夹具的全部不符点（空 = 通过）。
fn check_case(dir: &Path, case: &serde_json::Value, file: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let path = dir.join(file);
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return vec![format!("读不到夹具：{e}")],
    };
    let mut scene = match r3e_doc::parse_scene(&src) {
        Ok(s) => s,
        Err(e) => return vec![format!("场景不合法：{e}")],
    };
    // 验证器的契约是「调用方负责刷世界矩阵」，夹具刚解析完必须先刷一遍。
    scene.update_world_transforms();

    // 导航层：夹具自带地形，烘焙失败说明夹具本身坏了，如实报错而不是降级成 unchecked。
    let nav = match Nav::bake_scene(&scene) {
        Ok(nav) => nav,
        Err(e) => return vec![format!("导航烘焙失败：{e}")],
    };

    let targets: Vec<(String, Vec3)> = case["targets"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|t| {
                    let at = t["at"].as_array().cloned().unwrap_or_default();
                    let get = |i: usize| at.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    (
                        t["name"].as_str().unwrap_or_default().to_string(),
                        Vec3::new(get(0), get(1), get(2)),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let report = VerifyReport::collect_with_targets(&scene, &physics(), Some(&nav), &targets);

    // 口径 1：门禁条件下不许有「没检查」——除非夹具显式声明该层「本来就检查不了」。
    //
    // `allow_unchecked` 只收层名，不收敛原因：夹具里刻意放了旋转几何（15° 斜坡）时，
    // `BuiltinBackend` 的轴对齐解析式确实给不出结论，这是**预期**而不是门禁退化。
    // 风险由两点钉住：① 必须写在该 case 的 `why` 里，评审看得见；② 只放行点名的层，
    // 其余层漏检照样红。不加这一条，唯一出路是把夹具改成「没有旋转」，那等于为了
    // 门禁好看而删掉要测的几何。
    let allowed: Vec<&str> = case["allow_unchecked"]
        .as_array()
        .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for u in report.unchecked() {
        let layer = u.layer().as_str();
        if allowed.contains(&layer) {
            println!("      · {layer} 层未检查（夹具已声明）：{}", u.reason());
            continue;
        }
        problems.push(format!(
            "{layer} 层没检查：{}（scene-verify 条件完备，不该出现）",
            u.reason()
        ));
    }

    // 口径 2：必报码逐条核对，且必须带坐标。
    for code in case["must"].as_array().into_iter().flatten() {
        let code = code.as_str().unwrap_or_default();
        match report.issues().iter().find(|i| i.code == code) {
            None => problems.push(format!(
                "必报码 {code} 没出现（实际：{:?}）",
                codes(&report)
            )),
            Some(issue) if issue.location.is_none() => {
                problems.push(format!("必报码 {code} 报出来了但没有坐标"))
            }
            Some(_) => {}
        }
    }

    // 口径 3：pass/fail 两个方向都要对得上。
    let want_pass = case["passed"].as_bool().unwrap_or(false);
    let got_pass = report.passed();
    if want_pass != got_pass {
        problems.push(format!(
            "期望 passed={want_pass}，实际 {got_pass}（error={}, warn={}, 码={:?}）",
            report.error_count(),
            report.warn_count(),
            codes(&report)
        ));
    }

    problems
}

/// 报告里的码（按层顺序，已去重）。
fn codes(report: &VerifyReport) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = report.issues().iter().map(|i| i.code).collect();
    out.dedup();
    out
}

/// 「物理开着、后端申报 `shape_cast`」的世界。
///
/// `ground = 0.0`：地面高度是场景级真值，与夹具里地形的高度一致。
fn physics() -> Physics {
    Physics::with_backend(Box::new(BuiltinBackend::new(
        Vec3::new(0.0, -9.81, 0.0),
        0.0,
    )))
}

/// 报错文案里指代夹具目录的名字（避免与 `DIR` 常量重复拼接）。
const DIR_MSG: &str = "assets/verify/expect.json";

/// 三层各自的条目数（调试用，`verify` 不打印；保留给将来 `--verbose`）。
#[allow(dead_code)]
fn layer_counts(report: &VerifyReport) -> (usize, usize, usize) {
    (
        report.count_by_layer(Layer::Static),
        report.count_by_layer(Layer::Physics),
        report.count_by_layer(Layer::Nav),
    )
}
