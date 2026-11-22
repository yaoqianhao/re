//! 分组绘制的**实例区间契约**（真窗口回归，不需要 GPU）。
//!
//! 缺陷形态：`draw_indexed(索引区间, 0, 0..1)` 把「一个批次 N 个实例」退化成
//! 「只画批首 1 个」。这条路径最阴的地方是**统计口径看不出来** ——
//! `RenderStats` 的实例数按 `range` 长度累加，探针照样报「绘制实例 8」、
//! 上传记账照样报「跳过 8 条」，只有像素在少东西：
//!
//! * 运行时 `scene.spawn` 出来的新节点永远排在批尾 → 新加物件不显示，
//!   角色可视体（运行时 spawn 的 `hero_visual`）看不见；而既有节点执行
//!   `scene.settransform` 立刻生效（它是批首），于是一个渲染缺陷会被读成
//!   「物理没绑上」「相机框外」「场景装配漏了」；
//! * 阴影通道同一条规则，漏改则影子只投每批第一个实例。
//!
//! 正确性语义由 `src/renderer/tests.rs::draw_instance_count_tests` 兜底
//! （区间长度、空批、反向区间、批首偏移），这里只钉**接线**：
//!
//! 1. 主通道传给 `draw_indexed` 的实例范围来自 `instance_count`，不是字面量；
//! 2. 阴影通道同样；
//! 3. 两处共用同一个真源函数（各写一遍 `range.end - range.start` 迟早会漂）；
//! 4. 该真源对反向区间钳到 0，不让 wgpu 在 `slice()` 处 panic。

use std::path::{Path, PathBuf};

fn renderer_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("renderer")
}

fn read_renderer_file(name: &str) -> String {
    let path = renderer_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {} 失败: {e}", path.display()))
}

/// 主通道必须按整段实例区间绘制：批长取自 `instance_count`，绘制范围形如 `0..batch`。
#[test]
fn main_pass_draws_whole_batch() {
    let src = read_renderer_file("pass.rs");
    assert!(
        src.contains("let batch = instance_count(range);"),
        "主通道的批长必须来自 instance_count(range)，不许自己算或写死"
    );
    assert!(
        src.contains("0..batch,"),
        "draw_indexed 的实例范围必须是 0..batch（整段实例）"
    );
    assert!(
        !src.contains("0..1,"),
        "`draw_indexed(.., 0..1)` 是本次真窗口回归的缺陷形态：每批只画第一个实例"
    );
}

/// 阴影通道同一条不变量。
#[test]
fn shadow_pass_draws_whole_batch() {
    let src = read_renderer_file("shadow.rs");
    assert!(
        src.contains("let batch = instance_count(range);"),
        "阴影通道的批长同样必须来自 instance_count(range)"
    );
    assert!(
        src.contains("0..batch,"),
        "阴影通道 draw_indexed 的实例范围必须是整段"
    );
    assert!(
        !src.contains("0..1,"),
        "阴影通道漏改的症状是「主通道画全了，影子只投批首」"
    );
}

/// 单一真源：批长只能由 `types::instance_count` 给出，两个 pass 不许各算一遍。
#[test]
fn both_passes_share_one_source_of_truth() {
    let types = read_renderer_file("types.rs");
    assert!(
        types.contains("pub(crate) fn instance_count(range: &std::ops::Range<u32>) -> u32"),
        "instance_count 必须留在 types.rs 作为唯一真源"
    );

    let mod_src = read_renderer_file("mod.rs");
    assert!(
        mod_src.contains("instance_count,"),
        "instance_count 必须由 renderer/mod.rs 导出给两个 pass 共用"
    );

    for (name, src) in [
        ("pass.rs", read_renderer_file("pass.rs")),
        ("shadow.rs", read_renderer_file("shadow.rs")),
    ] {
        assert!(
            !src.contains("range.end - range.start"),
            "{name} 里又出现内联的区间长度计算：两处各算一遍必然漂，统一走 instance_count"
        );
    }
}

/// 反向区间钳到 0：宁可空绘制，也不让 wgpu 在 `slice()` 处 panic（定位成本差一个量级）。
#[test]
fn instance_count_clamps_reversed_range() {
    let types = read_renderer_file("types.rs");
    assert!(
        types.contains("saturating_sub(range.start)"),
        "反向区间必须用 saturating_sub 钳到 0"
    );
}
