//! S4-A：场景材质声明的贴图路径 ↔ 独立图片资产 id 的绑定契约。
//!
//! 这条链路上没有共享映射表：文档层只写路径，运行期只散列路径。所以「两边算出同一个
//! `AssetId`」本身就是协议，一旦漂移，渲染层拿不到像素会静默绑定白图，肉眼只能看出
//! 「材质没生效」。这里的用例就是给这条隐式协议加锁。

use std::path::{Path, PathBuf};

use r3e_asset::{import_texture, AssetLibrary};
use r3e_core::{SceneMaterialDecl, TextureSlot};

fn fixture_dir(case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    std::fs::create_dir_all(&dir).expect("创建夹具目录失败");
    dir
}

/// 写一张 2x2 纯色 PNG，返回落盘路径。
fn write_png(path: &Path) {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 200, 30, 255]));
    image.save(path).expect("写 PNG 失败");
}

fn decl_with(paths: &[(&str, &str)]) -> SceneMaterialDecl {
    let mut decl = SceneMaterialDecl {
        name: "mat".to_string(),
        ..SceneMaterialDecl::default()
    };
    for (slot, path) in paths {
        let value = Some((*path).to_string());
        match *slot {
            "albedo" => decl.albedo = value,
            "normal" => decl.normal = value,
            "mr" => decl.metallic_roughness = value,
            "occlusion" => decl.occlusion = value,
            "emissive" => decl.emissive_map = value,
            other => panic!("未知槽位 {other}"),
        }
    }
    decl
}

/// 文档层算出的 id 必须等于运行期导入拿到的 id（正斜杠相对路径）。
#[test]
fn resolved_ids_match_imported_texture_ids() {
    let dir = fixture_dir("resolved_ids_match_imported_texture_ids");
    std::fs::create_dir_all(dir.join("tex")).expect("建 tex 目录");
    write_png(&dir.join("tex").join("a_albedo.png"));
    write_png(&dir.join("tex").join("a_normal.png"));

    let decl = decl_with(&[
        ("albedo", "tex/a_albedo.png"),
        ("normal", "tex/a_normal.png"),
    ]);

    let mut lib = AssetLibrary::default();
    let mut imported = Vec::new();
    for rel in decl.texture_paths().into_iter().flatten() {
        let resolved = r3e_core::resolve_texture_path(Some(&dir), rel);
        let tex = import_texture(&mut lib, Path::new(&resolved)).expect("导入贴图失败");
        imported.push(tex.asset);
    }

    let declared = decl.texture_asset_ids(Some(&dir));
    assert_eq!(declared[0], Some(imported[0]), "albedo 槽的 id 对不上");
    assert_eq!(declared[1], Some(imported[1]), "normal 槽的 id 对不上");
    assert_eq!(declared[2], None, "未声明的槽位不该有 id");
    assert_eq!(declared[3], None);
    assert_eq!(declared[4], None);
}

/// 调用方若把 Windows 原生反斜杠路径交给导入函数，算出的 id 仍要与文档层一致。
///
/// 这是真实风险点：`Path::to_string_lossy` 在 Windows 上保留反斜杠，而
/// `resolve_texture_path` 恒产出 `/` 分隔。不归一就会出现「同一张图两个 id」。
#[test]
fn native_backslash_path_still_matches() {
    let dir = fixture_dir("native_backslash_path_still_matches");
    std::fs::create_dir_all(dir.join("tex")).expect("建 tex 目录");
    write_png(&dir.join("tex").join("b_mr.png"));

    let decl = decl_with(&[("mr", "tex/b_mr.png")]);
    let resolved = r3e_core::resolve_texture_path(Some(&dir), "tex/b_mr.png");
    let native = resolved.replace('/', "\\");

    let mut lib = AssetLibrary::default();
    let tex = import_texture(&mut lib, PathBuf::from(&native)).expect("反斜杠路径导入失败");

    assert_eq!(
        decl.texture_asset_ids(Some(&dir))[2],
        Some(tex.asset),
        "反斜杠路径应归一到与文档层相同的 id"
    );
    assert_eq!(tex.source_path.replace('\\', "/"), resolved);
}

/// 同一张图被多个槽位引用时只解码一次，第二次复用同一条资产。
#[test]
fn shared_texture_is_decoded_once() {
    let dir = fixture_dir("shared_texture_is_decoded_once");
    write_png(&dir.join("shared.png"));

    let decl = decl_with(&[("albedo", "shared.png"), ("emissive", "shared.png")]);
    let resolved = r3e_core::resolve_texture_path(Some(&dir), "shared.png");

    let mut lib = AssetLibrary::default();
    let first = import_texture(&mut lib, Path::new(&resolved)).expect("首次导入失败");
    assert!(!first.reused, "首次导入不应标记为复用");
    let second = import_texture(&mut lib, Path::new(&resolved)).expect("二次导入失败");
    assert!(second.reused, "同路径二次导入必须复用");
    assert_eq!(first.asset, second.asset);

    let ids = decl.texture_asset_ids(Some(&dir));
    assert_eq!(ids[0], ids[4], "两个槽指向同一张图时应是同一个 id");
}

/// 文件名定槽位：`_normal` / `_mr` / `_ao` / `_emissive` 各自归位，否则渲染器
/// 会把法线图当基础色用（画面看不出错，但整体发灰）。
#[test]
fn slot_is_inferred_from_filename() {
    let dir = fixture_dir("slot_is_inferred_from_filename");
    std::fs::create_dir_all(dir.join("tex")).expect("建 tex 目录");
    for name in [
        "c_albedo.png",
        "c_normal.png",
        "c_mr.png",
        "c_ao.png",
        "c_emissive.png",
    ] {
        write_png(&dir.join("tex").join(name));
    }

    let mut lib = AssetLibrary::default();
    let mut slots = Vec::new();
    for name in [
        "c_albedo.png",
        "c_normal.png",
        "c_mr.png",
        "c_ao.png",
        "c_emissive.png",
    ] {
        let resolved = r3e_core::resolve_texture_path(Some(&dir), &format!("tex/{name}"));
        let tex = import_texture(&mut lib, Path::new(&resolved)).expect("导入失败");
        slots.push(tex.slot);
    }

    assert_eq!(
        slots,
        vec![
            TextureSlot::BaseColor,
            TextureSlot::Normal,
            TextureSlot::MetallicRoughness,
            TextureSlot::Occlusion,
            TextureSlot::Emissive,
        ]
    );
}

/// 文件缺失走 `AssetError::Load`：与「路径写错」区分开，供上层报 `texture_missing`。
#[test]
fn missing_file_is_a_load_error() {
    let dir = fixture_dir("missing_file_is_a_load_error");
    let resolved = r3e_core::resolve_texture_path(Some(&dir), "nope.png");
    let mut lib = AssetLibrary::default();
    assert!(
        import_texture(&mut lib, Path::new(&resolved)).is_err(),
        "不存在的文件必须报错，不能静默生成空贴图"
    );
}

/// 解析成渲染层直吃的 `MaterialData` 时，五槽各自带 `(id, 0)`，未声明的给 `None`。
#[test]
fn material_data_gets_one_index_per_slot() {
    let dir = fixture_dir("material_data_gets_one_index_per_slot");
    write_png(&dir.join("d_albedo.png"));

    let decl = decl_with(&[("albedo", "d_albedo.png")]);
    let data = decl.to_material_data(Some(&dir));

    let albedo = data.base_color_texture.expect("albedo 槽丢失");
    assert_eq!(albedo.1, 0, "独立图片资产里贴图下标恒为 0");
    assert_eq!(data.normal_texture, None);
    assert_eq!(data.roughness_texture, None);
    assert_eq!(data.occlusion_texture, None);
    assert_eq!(data.emissive_texture, None);
    assert_eq!(data.uv_scale, [1.0, 1.0]);
    assert_eq!(data.name, "mat");
}
