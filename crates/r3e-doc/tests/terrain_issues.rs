//! 地形数据自洽与地表覆盖的校验契约（M2-C06）。
//!
//! 与 `tests/validate_material.rs` 同一立场：每条负例从「合法地形场景」出发只破坏
//! 一个约束，断言必须出现指定 code；正向用例确认不误报。
//!
//! 两条 code 的反例都**绕过命令层直写 `scene.terrain`**（与 `dup_material_name_is_error`
//! 同理）：`terrain.create` 会替调用方把样本补齐、把分辨率夹住，走命令层就永远
//! 造不出这两种坏数据，而热重载 / 手改 `.r3e.json` 能造出来——校验要拦的正是那条路。
//!
//! `assert_error` / `assert_warn` 的首个字符串实参恒为 code，`xtask invariants`
//! 按此扫源码，换写法会让能力清单漂移。

use r3e_core::terrain::TerrainPatch;
use r3e_core::Scene;
use r3e_doc::{Document, Severity};

/// 一块 20×20 米、4×4 采样的地形（样本数 16，与分辨率一致）。
fn patch(name: &str, origin: [f32; 2]) -> TerrainPatch {
    TerrainPatch {
        name: name.to_string(),
        origin,
        size: [20.0, 20.0],
        resolution: [4, 4],
        heights: vec![0.0; 16],
        ..TerrainPatch::default()
    }
}

fn codes(doc: &Document) -> Vec<&'static str> {
    doc.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.code)
        .collect()
}

fn warn_codes(doc: &Document) -> Vec<&'static str> {
    doc.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Warn)
        .map(|i| i.code)
        .collect()
}

fn assert_error(doc: &Document, code: &str) {
    let found = codes(doc);
    assert!(found.contains(&code), "期望 {code}，实得 {found:?}");
}

fn assert_warn(doc: &Document, code: &str) {
    let found = warn_codes(doc);
    assert!(found.contains(&code), "期望 warn {code}，实得 {found:?}");
}

// ---------- 负向 ----------

#[test]
fn sample_count_mismatch_is_error() {
    let mut scene = Scene::new();
    let mut p = patch("ground", [0.0, 0.0]);
    // 12 个样本配 4×4 分辨率：坡度权重归一的前提（样本齐）不成立。
    p.heights.truncate(12);
    scene.terrain = vec![p];
    let doc = Document::new(scene);
    assert_error(&doc, "terrain.weight_not_normalized");
}

#[test]
fn non_finite_height_is_error() {
    let mut scene = Scene::new();
    let mut p = patch("ground", [0.0, 0.0]);
    p.heights[5] = f32::NAN;
    scene.terrain = vec![p];
    let doc = Document::new(scene);
    assert_error(&doc, "terrain.weight_not_normalized");
}

#[test]
fn hole_between_patches_is_warn() {
    let mut scene = Scene::new();
    // 两块 20×20 之间留 20 米空档：中间列四周都有地，属于「被包起来的空洞」。
    scene.terrain = vec![patch("west", [0.0, 0.0]), patch("east", [40.0, 0.0])];
    let doc = Document::new(scene);
    assert_warn(&doc, "terrain.block_gap");
}

// ---------- 正向 ----------

#[test]
fn clean_terrain_is_silent() {
    let mut scene = Scene::new();
    scene.terrain = vec![patch("ground", [0.0, 0.0])];
    let doc = Document::new(scene);
    assert!(
        !codes(&doc).contains(&"terrain.weight_not_normalized"),
        "样本数正确的块不该判权重问题"
    );
    // 单块地形边界外全是空白，谈不上内部空洞。
    assert!(
        !warn_codes(&doc).contains(&"terrain.block_gap"),
        "单块地形不该判 block_gap"
    );
}

#[test]
fn empty_terrain_is_silent() {
    let doc = Document::new(Scene::new());
    assert!(!codes(&doc).contains(&"terrain.weight_not_normalized"));
    assert!(!warn_codes(&doc).contains(&"terrain.block_gap"));
}
