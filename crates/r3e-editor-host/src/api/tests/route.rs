//! 场景切换 / 存档链路上不需要窗口的那半段测试。
//!
//! 由 M0-S2 从 `api.rs` 内 `mod scene_route_tests` 原样切出，断言未改动。

// 「存档写盘 → 读回」的往返。真正换世界的部分在 `EngineApp` 里，需要
// 图形设备，交给 `cargo run` 的手动回归与 r3e-core 的纯函数单测覆盖。

use super::super::support::resolve_request_path;
use r3e_core::scene_stack::SceneArchive;
use r3e_runtime::app::default_archive_path;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("r3e-scene-route-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

#[test]
fn default_archive_path_appends_archive_suffix() {
    // `with_extension` 是整体替换后缀，所以 `lv1.json` → `lv1.archive.json`，
    // 而不是 `lv1.json.archive.json`——这条是面板上"另存存档"看到的默认名。
    assert_eq!(
        default_archive_path(std::path::Path::new("D:/a/lv1.json")),
        std::path::PathBuf::from("D:/a/lv1.archive.json")
    );
    // 没有后缀的场景文件也要产出可读的名字，不能变成 `lv1.archive`
    assert_eq!(
        default_archive_path(std::path::Path::new("D:/a/lv1")),
        std::path::PathBuf::from("D:/a/lv1.archive.json")
    );
}

#[test]
fn resolve_request_path_keeps_absolute_and_existing() {
    let dir = temp_dir("abs");
    let file = dir.join("lv2.json");
    std::fs::write(&file, "{}").unwrap();
    assert_eq!(
        resolve_request_path("D:/nowhere/lv1.json", file.to_str().unwrap()),
        file
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_request_path_falls_back_to_scene_directory() {
    let dir = temp_dir("rel");
    let scene = dir.join("lv1.json");
    std::fs::write(&scene, "{}").unwrap();
    let target = dir.join("lv2.json");
    std::fs::write(&target, "{}").unwrap();

    // 面板里填的是相对名：用户心里的"当前目录"是场景所在目录
    assert_eq!(
        resolve_request_path(scene.to_str().unwrap(), "lv2.json"),
        target
    );
    // 两边都不存在时原样返回，让调用方按"文件不存在"报错，而不是瞎猜
    assert_eq!(
        resolve_request_path(scene.to_str().unwrap(), "missing.json"),
        std::path::PathBuf::from("missing.json")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn archive_written_to_disk_reads_back_with_runtime_state() {
    let dir = temp_dir("roundtrip");
    let mut scene =
        r3e_doc::parse_scene(r#"{"name":"lv1","objects":[{"name":"box","mesh":"cube"}]}"#)
            .expect("场景解析");
    scene.time = 12.5;
    scene.frame = 750;

    let target = dir.join("lv1.archive.json");
    let archive = SceneArchive::from_scene(&scene);
    archive.save_to_path(&target).expect("存档写盘");

    let back = SceneArchive::load_path(&target).expect("存档读回");
    back.validate().expect("存档自校验");
    let (restored_scene, restored_anims) = back.build_scene().expect("重建场景");
    assert_eq!(restored_scene.len(), 1);
    assert_eq!(restored_scene.name, "lv1");
    // 声明态走 `scene` 段，运行时态走 `runtime` 段：时间 / 帧号这两样
    // `to_scene_file` 刻意不写的字段，存档必须原样带回来。
    assert!((restored_scene.time - 12.5).abs() < 1e-6);
    assert_eq!(restored_scene.frame, 750);
    // 场景里没有动画实例，恢复计数为 0 才是对的
    assert_eq!(restored_anims, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
