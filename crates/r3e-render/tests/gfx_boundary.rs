//! 图形 API 抽象层边界（扩展四十四验收）。
//!
//! 「引擎不绑定单一图形 API」这句话如果不能被测试钉住，半年后就会退化成一堆
//! 散落的 `Instance::new`。本测试扫源码文本（忽略 `#[cfg(test)]` 之后的内容），
//! 守住四条边界：
//!
//! 1. **设备只能在一个地方弄到手** —— Instance / Adapter / Device / Surface 的创建
//!    只许出现在 `gfx/backend.rs`；
//! 2. **渲染器不再把后端类型递给上层** —— 扩展四十三那三个泄漏点不许复活；
//! 3. **中性面真的中性** —— `gfx` 除 `backend.rs` 外不得出现 `wgpu::`；
//! 4. **平台偏好表与 API 枚举同步** —— 每行至少一个已知 API，且默认 API 在候选里。

use std::path::{Path, PathBuf};

fn crate_src(crate_dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates 目录")
        .join(crate_dir)
        .join("src")
}

/// 读源码并截掉测试模块。测试里允许直接提到后端 API 做对照断言。
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

/// 设备获取的唯一合法位置（相对 crate 的 `src`）。
const ACQUISITION_SITE: &str = "gfx/backend.rs";

/// 去掉注释行：边界关心的是**代码**里的类型引用，文档注释里点名
/// `wgpu::AdapterInfo` 恰恰是在解释「为什么不能暴露它」。
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn rel(crate_src_dir: &Path, file: &Path) -> String {
    file.strip_prefix(crate_src_dir)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn device_acquisition_lives_only_in_the_backend_impl() {
    // 这些调用一旦散出去，「换 API 只改一个文件」就不成立了
    let forbidden = [
        "wgpu::Instance",
        ".request_adapter(",
        ".request_device(",
        ".create_surface(",
    ];
    for crate_dir in ["r3e-render", "r3e-app", "r3e-editor"] {
        let dir = crate_src(crate_dir);
        if !dir.is_dir() {
            continue;
        }
        for file in rs_files(&dir) {
            let rel_path = rel(&dir, &file);
            if crate_dir == "r3e-render" && rel_path == ACQUISITION_SITE {
                continue;
            }
            let src = production_source(&file);
            for token in forbidden {
                assert!(
                    !src.contains(token),
                    "{crate_dir}/{rel_path} 的生产代码里出现了 `{token}` —— 设备获取必须收在 \
                     r3e-render/src/{ACQUISITION_SITE}，否则换图形 API 要改多处"
                );
            }
        }
    }
}

#[test]
fn renderer_no_longer_hands_out_backend_types() {
    // 扩展四十三曾在 `Renderer` 上暴露 AdapterInfo / Features / Backends，
    // 上层因此被迫认识 wgpu。这三个签名是那次泄漏的指纹，不许复活。
    let path = crate_src("r3e-render").join("renderer.rs");
    let src = production_source(&path);
    for leaked in [
        "pub fn adapter_info(",
        "pub fn device_features(",
        "pub fn requested_backends(",
        "adapter_info: wgpu::AdapterInfo",
    ] {
        assert!(
            !src.contains(leaked),
            "renderer.rs 又泄漏了后端类型 `{leaked}` —— 上层请改用 gfx::GfxInfo"
        );
    }
}

#[test]
fn the_neutral_layer_stays_neutral() {
    // 抽象层的「公开面」必须与具体图形库无关：api / info / selection / mod 里
    // 出现 `wgpu::` 就说明抽象正在被击穿（backend.rs 是实现，不在此列）
    let dir = crate_src("r3e-render").join("gfx");
    for file in rs_files(&dir) {
        let rel_path = rel(&dir, &file);
        if rel_path == "backend.rs" {
            continue;
        }
        let src = code_only(&production_source(&file));
        assert!(
            !src.contains("wgpu::"),
            "gfx/{rel_path} 里出现了 `wgpu::` —— 中性面被污染，换 API 时这里要跟着改"
        );
    }
}

#[test]
fn renderer_implements_the_cross_api_contract() {
    // 契约真的挂上了才叫抽象；否则 trait 只是摆设
    let path = crate_src("r3e-render").join("renderer.rs");
    let src = production_source(&path);
    assert!(
        src.contains("impl crate::gfx::RenderBackend for Renderer"),
        "Renderer 没有实现 RenderBackend —— 跨 API 协议形同虚设"
    );
}

#[test]
fn platform_preference_table_is_consistent() {
    use r3e_render::{default_api, preference, GfxApi, GfxPlatform, PLATFORM_PREFERENCE};

    for (platform, apis) in PLATFORM_PREFERENCE {
        assert!(
            !apis.is_empty(),
            "平台 {platform} 的偏好表为空——自动选择会无解"
        );
        for api in *apis {
            assert!(
                GfxApi::ALL.contains(api),
                "平台 {platform} 的偏好表里有未声明进 GfxApi::ALL 的项 {api}"
            );
        }
        // 默认 API 必须落在候选里（否则 `default_api` 与 `preference` 各说各话）
        assert!(
            preference(*platform).contains(&default_api(*platform)),
            "平台 {platform} 的默认 API 不在候选表里"
        );
    }

    // 每个已知平台都得有自己的一行：靠 `preference` 的兜底会把「漏配」藏起来，
    // 所以这里直接按平台名逐条查表，查不到就是漏配。
    for platform in [
        GfxPlatform::Windows,
        GfxPlatform::Linux,
        GfxPlatform::MacOs,
        GfxPlatform::Android,
        GfxPlatform::Web,
        GfxPlatform::Other,
    ] {
        assert!(
            PLATFORM_PREFERENCE
                .iter()
                .any(|(known, _)| *known == platform),
            "平台 {platform} 没有偏好表行——自动选择会静默退回 ALL"
        );
    }
}

#[test]
fn only_implemented_apis_are_offered_today() {
    use r3e_render::{default_api, GfxApi, PLATFORM_PREFERENCE};

    // 今天只有 wgpu 一个实现。新增后端时**必须**同时改这条断言 —— 逼作者承认
    // 「我加了一个 API 家族」，而不是悄悄塞进枚举里没人实现。
    assert_eq!(GfxApi::ALL, &[GfxApi::Wgpu], "新增图形 API 请更新本断言");
    for (platform, _) in PLATFORM_PREFERENCE {
        assert_eq!(default_api(*platform), GfxApi::Wgpu);
    }
    assert_eq!(GfxApi::Wgpu.name(), "wgpu");
    assert_eq!(GfxApi::from_name("WGPU"), Some(GfxApi::Wgpu));
    assert_eq!(GfxApi::from_name("vulkan"), None, "未实现的 API 不得被识别");
}
