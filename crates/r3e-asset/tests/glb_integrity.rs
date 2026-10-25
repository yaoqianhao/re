//! GLB 容器完整性校验：截断 / 错 magic / 错版本都要给出能定位的报错。
//!
//! 起因是真实样本 `CesiumMan.glb` 只下载了一半（头声明 438044 字节，实际 31816），
//! 报错却只有 `failed to fill whole buffer`，完全看不出问题在哪。

use std::io::Write;
use std::path::PathBuf;

fn temp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("r3e_glb_integrity");
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir.join(name)
}

fn write(path: &PathBuf, bytes: &[u8]) {
    let mut f = std::fs::File::create(path).expect("建文件");
    f.write_all(bytes).expect("写文件");
}

fn glb_header(declared: u32, magic: &[u8; 4], version: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(magic);
    v.extend_from_slice(&version.to_le_bytes());
    v.extend_from_slice(&declared.to_le_bytes());
    v
}

#[test]
fn truncated_glb_reports_declared_vs_actual_size() {
    let path = temp_path("truncated.glb");
    // 头声明 4096 字节，实际只写 64 字节。
    let mut bytes = glb_header(4096, b"glTF", 2);
    bytes.resize(64, 0);
    write(&path, &bytes);

    let err = r3e_asset::import_glb(&path).expect_err("截断文件必须失败");
    let msg = err.to_string();
    assert!(msg.contains("被截断"), "报错应点明截断：{msg}");
    assert!(msg.contains("4096") && msg.contains("64"), "报错应含声明/实际字节数：{msg}");
}

#[test]
fn wrong_magic_reports_helpful_hint() {
    let path = temp_path("wrong_magic.glb");
    write(&path, &glb_header(12, b"NOPE", 2));

    let err = r3e_asset::import_glb(&path).expect_err("错 magic 必须失败");
    let msg = err.to_string();
    assert!(msg.contains("GLB") && msg.contains("magic"), "报错应提示 magic：{msg}");
}

#[test]
fn unsupported_glb_version_is_reported() {
    let path = temp_path("v1.glb");
    write(&path, &glb_header(12, b"glTF", 1));

    let err = r3e_asset::import_glb(&path).expect_err("glTF 1.0 必须失败");
    let msg = err.to_string();
    assert!(msg.contains("2.0"), "报错应说明只支持 2.0：{msg}");
}

#[test]
fn intact_header_passes_container_check_and_fails_later() {
    // 头部自洽但内容为空：应进入真正的解析阶段再失败（说明校验没误杀）。
    let path = temp_path("empty_but_consistent.glb");
    write(&path, &glb_header(12, b"glTF", 2));

    let err = r3e_asset::import_glb(&path).expect_err("空容器必须失败");
    let msg = err.to_string();
    assert!(!msg.contains("被截断"), "不应误判为截断：{msg}");
    assert!(!msg.contains("magic"), "不应误判为 magic 错误：{msg}");
}
