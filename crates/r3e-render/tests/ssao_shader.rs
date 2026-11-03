//! 屏幕空间遮蔽（扩展十七）的着色器静态校验：不开 GPU 也能验 `ssao.wgsl`。
//!
//! 与 `postfx_shader.rs` 同路子：直接调 wgpu 内置的 naga 做 `parse_str` +
//! `validate`，把「着色器写错」的反馈周期从「起一次图形上下文」压到毫秒级。
//!
//! 另一半是 CPU / GPU 的交叉锁定：半球核槽位数、绑定位置、AO 与泛光的
//! 乘叠顺序。这几处写错都不会编译失败，只会在实机上表现为「采样拿到零」
//! 或「光晕被暗角吃掉」，靠肉眼极难定位。

use r3e_core::ssao::MAX_SSAO_SAMPLES;
use r3e_render::ssao::{AO_FORMAT, AO_PASSES_PER_FRAME};

const SSAO: &str = include_str!("../src/ssao.wgsl");
const POSTFX: &str = include_str!("../src/postfx.wgsl");

fn parsed(source: &str, file: &str) -> naga::Module {
    match naga::front::wgsl::parse_str(source) {
        Ok(module) => module,
        Err(e) => panic!("{file} 解析失败：\n{}", e.emit_to_string(source)),
    }
}

fn validated(source: &str, file: &str) -> naga::Module {
    let module = parsed(source, file);
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    if let Err(e) = validator.validate(&module) {
        panic!("{file} 语义校验失败：\n{e:?}");
    }
    module
}

#[test]
fn ssao_shader_parses_and_validates() {
    let module = validated(SSAO, "ssao.wgsl");
    assert!(!module.functions.is_empty(), "解析出的模块里没有任何函数");
}

/// 三个入口：全屏三角形顶点 + 生成 + 散焦。少一个就有一半链路断掉。
#[test]
fn ssao_shader_declares_entry_points() {
    let module = parsed(SSAO, "ssao.wgsl");
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for expected in ["vs_fullscreen", "fs_ssao", "fs_blur"] {
        assert!(
            names.contains(&expected),
            "缺少入口点 {expected}，实际有 {names:?}"
        );
    }
}

/// 半球核槽位数必须与 core 侧的 `MAX_SSAO_SAMPLES` 一致：
/// 少一个会读越界（wgpu 静默钳到 0），多一个白占 uniform 空间。
#[test]
fn kernel_array_matches_core() {
    let expected = format!("array<vec4<f32>, {MAX_SSAO_SAMPLES}>");
    assert!(
        SSAO.contains(&expected),
        "着色器的半球核声明应与 core 的 MAX_SSAO_SAMPLES = {MAX_SSAO_SAMPLES} 一致"
    );
}

/// 参数结构体字段顺序 = Rust 侧 `SsaoParams` 的顺序（proj、inv_proj、kernel、screen、misc）。
/// wgpu 只按字节长度校验 uniform，字段换位不会报错，只会静默把半径读成偏置。
#[test]
fn ssao_params_layout_matches_rust() {
    let order = [
        "proj: mat4x4<f32>",
        "inv_proj: mat4x4<f32>",
        "kernel: array<vec4<f32>, ",
    ];
    let mut cursor = 0usize;
    for field in order {
        let found = SSAO[cursor..]
            .find(field)
            .unwrap_or_else(|| panic!("ssao.wgsl 的 SsaoParams 缺少字段 {field}（或顺序不对）"));
        cursor += found + field.len();
    }
    for tail in ["screen: vec4<f32>", "misc: vec4<f32>"] {
        assert!(
            SSAO[cursor..].contains(tail),
            "ssao.wgsl 的 SsaoParams 应含 {tail}，且排在 kernel 之后"
        );
    }
}

/// 绑定位置：group(0) 是参数块，group(1) 是「每 pass 一张输入图」。
/// 生成 pass 读深度（texture_depth_2d），散焦 pass 读遮蔽图。
#[test]
fn ssao_bindings_match_pipeline() {
    assert!(
        SSAO.contains("@group(0) @binding(0) var<uniform> params"),
        "参数块应固定在 group(0) binding(0)"
    );
    assert!(
        SSAO.contains("@group(1) @binding(0) var depth_tex: texture_depth_2d"),
        "生成 pass 的深度输入应绑在 group(1) binding(0)，且类型必须是 texture_depth_2d"
    );
    assert!(
        SSAO.contains("@group(1) @binding(1) var src_ao: texture_2d<f32>"),
        "散焦 pass 的遮蔽图输入应绑在 group(1) binding(1)"
    );
}

/// 遮蔽图必须是单通道 8 位：它只有 `[0,1]` 一维、且经过 4×4 散焦，
/// 用 16 位浮点只会把全尺寸纹理的带宽翻 8 倍。
#[test]
fn ao_target_is_single_channel() {
    assert_eq!(AO_FORMAT, wgpu::TextureFormat::R8Unorm);
    assert_eq!(AO_PASSES_PER_FRAME, 2, "生成 + 散焦，多一个就是预算漏算");
}

/// 乘叠顺序：遮蔽压直接光，必须**在泛光叠加之前**乘进颜色。
/// 反过来等于让暗角把光晕也吃掉——画面仍能出，但高光周围会发灰。
#[test]
fn postfx_composite_applies_ao_before_bloom() {
    assert!(
        POSTFX.contains("@group(1) @binding(4) var t_ao: texture_2d<f32>"),
        "composite 需要一张绑定在 group(1) binding(4) 的遮蔽图"
    );
    let body_start = POSTFX
        .find("fn composite_color")
        .expect("postfx.wgsl 缺 composite_color");
    let body = &POSTFX[body_start..];
    let ao_at = body
        .find("t_ao")
        .expect("composite_color 没有采样遮蔽图，AO 不会生效");
    let bloom_at = body.find("t_aux").expect("composite_color 没有采样泛光图");
    assert!(
        ao_at < bloom_at,
        "遮蔽必须在泛光之前乘进颜色：先乘 AO，再叠散射光"
    );
    // 曝光 / 色调映射一定在两者之后
    let tonemap_at = body
        .find("tonemap_color")
        .expect("composite_color 没做色调映射");
    assert!(bloom_at < tonemap_at, "泛光必须在色调映射之前叠加");
}
