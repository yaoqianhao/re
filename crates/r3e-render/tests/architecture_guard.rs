//! 架构守卫（S1/S4 验收）：输出侧不得再按「来源」分支。
//!
//! 统一 AIR 之后，渲染 / 物理 / 应用层只认 `MeshProvider` 给的 `MeshData`，
//! 不允许出现 `MeshRef::Builtin`、`BuiltinMesh::*` 之类的来源判断——那种分支一旦
//! 复活，内置图元与外部资产就会重新走两套逻辑。
//!
//! 本测试直接扫描源码文本（`#[cfg(test)]` 之后的内容忽略），属于最便宜的回归闸门。

use std::path::{Path, PathBuf};

fn crate_src(crate_dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 目录")
        .join(crate_dir)
        .join("src")
}

/// 读源码并截掉测试模块（测试里允许直接构造内置图元做对照）。
fn production_source(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path:?} 失败: {e}"));
    match text.find("#[cfg(test)]") {
        Some(idx) => text[..idx].to_string(),
        None => text,
    }
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("读目录 {dir:?} 失败: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}

fn assert_no_tokens(crate_dir: &str, tokens: &[&str]) {
    let dir = crate_src(crate_dir);
    for file in rs_files(&dir) {
        let src = production_source(&file);
        for token in tokens {
            assert!(
                !src.contains(token),
                "{} 的生产代码里出现了来源分支 `{token}`——统一流水线被破坏",
                file.display()
            );
        }
    }
}

#[test]
fn render_layer_has_no_source_branch() {
    assert_no_tokens("r3e-render", &["BuiltinMesh", "MeshRef::Builtin"]);
}

#[test]
fn physics_layer_has_no_source_branch() {
    // 物理只看几何：形状由 `infer_shape(&MeshData)` 判定，半尺寸由 AABB 推出。
    assert_no_tokens("r3e-physics", &["BuiltinMesh", "MeshRef::Builtin", "MeshRef::Asset"]);
}

#[test]
fn app_layer_has_no_source_branch() {
    assert_no_tokens("r3e-app", &["BuiltinMesh", "MeshRef::Builtin"]);
}

#[test]
fn upload_path_is_single() {
    // `MeshCache::ensure` 必须只经 `self.provider.mesh_data` 取数据，不允许旁路。
    let renderer = production_source(&crate_src("r3e-render").join("renderer.rs"));
    let ensure_body = renderer
        .split("pub fn ensure(")
        .nth(1)
        .expect("应存在 MeshCache::ensure");
    let head: String = ensure_body.chars().take(400).collect();
    assert!(
        head.contains("self.provider.mesh_data(key)"),
        "ensure 未走统一 provider：\n{head}"
    );
}
