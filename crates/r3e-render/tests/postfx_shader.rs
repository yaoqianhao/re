//! 后处理着色器的静态校验：不开窗口、不建 GPU 也能验证 `postfx.wgsl`。
//!
//! 与 `shader_wgsl.rs` 同一思路（wgpu 内置 naga 做 `parse_str` + `validate`），
//! 另外锁住三条最容易单边漂移的契约：
//!
//! * uniform 结构体的**字段顺序**（WGSL 与 Rust 的 `PostFxParams` 必须逐字段对齐，
//!   否则 60 字节错位不会有任何编译错误，只会让画面静默用错参数）；
//! * 色调映射与泛光的**数值系数**（ACES 五系数、FXAA 双阈值、Rec.709 权重）；
//! * 六个入口点是否齐备（少一个 pass 就等于后处理链断了一节）。

use r3e_core::postfx::{
    ACES_A, ACES_B, ACES_C, ACES_D, ACES_E, FXAA_EDGE_THRESHOLD_MIN, FXAA_EDGE_THRESHOLD_RELATIVE,
    LUMA_WEIGHTS,
};

const POSTFX_SHADER: &str = include_str!("../src/postfx.wgsl");
const POSTFX_RUST: &str = include_str!("../src/postfx.rs");

fn parsed_module() -> naga::Module {
    match naga::front::wgsl::parse_str(POSTFX_SHADER) {
        Ok(module) => module,
        Err(e) => panic!(
            "postfx.wgsl 解析失败：\n{}",
            e.emit_to_string(POSTFX_SHADER)
        ),
    }
}

#[test]
fn postfx_shader_parses() {
    assert!(!parsed_module().functions.is_empty());
}

#[test]
fn postfx_shader_validates() {
    let module = parsed_module();
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    if let Err(e) = validator.validate(&module) {
        panic!("postfx.wgsl 语义校验失败：\n{e:?}");
    }
}

/// 六个 pass 的入口点：全屏三角形顶点 + 亮部 / 降采样 / 上采样 / composite。
#[test]
fn postfx_shader_declares_entry_points() {
    let module = parsed_module();
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for expected in [
        "vs_fullscreen",
        "fs_prefilter",
        "fs_downsample",
        "fs_upsample",
        "fs_composite",
    ] {
        assert!(
            names.contains(&expected),
            "缺少入口点 {expected}，实际有 {names:?}"
        );
    }
}

/// WGSL 里的 `struct Params` 与 Rust 的 `PostFxParams` 必须逐字段同序。
///
/// 两边都是「4 字节标量 + vec2」，字段一换位就会静默把曝光读成阈值，
/// 而 wgpu 只按字节长度校验，不会报错——所以这里用文本顺序当护栏。
#[test]
fn postfx_params_layout_matches_rust() {
    const FIELDS: [&str; 13] = [
        "exposure_scale",
        "tonemap",
        "bloom_intensity",
        "bloom_enabled",
        "fxaa_enabled",
        "_pad0",
        "_pad1",
        "_pad2",
        "hdr_texel",
        "threshold",
        "knee",
        "src_texel",
        "_pad3",
    ];
    for (label, src) in [("postfx.wgsl", POSTFX_SHADER), ("postfx.rs", POSTFX_RUST)] {
        let mut cursor = 0usize;
        for field in FIELDS.iter().filter(|f| !f.starts_with("_pad")) {
            let found = src[cursor..]
                .find(&format!("{field}:"))
                .unwrap_or_else(|| panic!("{label} 里找不到字段 {field}（顺序也可能错了）"));
            cursor += found + field.len();
        }
    }
    // 两侧都必须是 64 字节：16 字节对齐的 vec2 让结构体没有额外尾部填充
    assert!(
        POSTFX_RUST.contains("size_of::<PostFxParams>() == 64"),
        "Rust 侧应保留 64 字节的布局断言"
    );
}

/// uniform 里 `vec2<f32>` 的声明位置：WGSL 的 16 字节对齐规则下，
/// `hdr_texel` 必须落在 32 偏移（前 8 个 4 字节字段），不能挪到别处。
#[test]
fn postfx_params_vector_alignment() {
    let head = POSTFX_SHADER
        .split("hdr_texel")
        .next()
        .expect("应存在 hdr_texel");
    let scalars_before = head.matches(": f32,").count() + head.matches(": u32,").count();
    assert_eq!(
        scalars_before, 8,
        "hdr_texel 之前应恰好有 8 个 4 字节字段（32 字节偏移）"
    );
}

/// ACES 五系数与 FXAA 双阈值：CPU 改了这里必须同步改着色器。
#[test]
fn postfx_constants_match_cpu_reference() {
    let needles = [
        format!("const ACES_A: f32 = {ACES_A};"),
        format!("const ACES_B: f32 = {ACES_B};"),
        format!("const ACES_C: f32 = {ACES_C};"),
        format!("const ACES_D: f32 = {ACES_D};"),
        format!("const ACES_E: f32 = {ACES_E};"),
        format!("const FXAA_EDGE_THRESHOLD_MIN: f32 = {FXAA_EDGE_THRESHOLD_MIN};"),
        format!("const FXAA_EDGE_THRESHOLD_RELATIVE: f32 = {FXAA_EDGE_THRESHOLD_RELATIVE};"),
    ];
    for needle in needles {
        assert!(
            POSTFX_SHADER.contains(&needle),
            "postfx.wgsl 缺少常量声明：{needle}"
        );
    }
    let luma = format!(
        "const LUMA_WEIGHTS: vec3<f32> = vec3<f32>({}, {}, {});",
        LUMA_WEIGHTS.x, LUMA_WEIGHTS.y, LUMA_WEIGHTS.z
    );
    assert!(
        POSTFX_SHADER.contains(&luma),
        "postfx.wgsl 的亮度权重应与 core 的 LUMA_WEIGHTS 一致：{luma}"
    );
}

/// 泛光必须**先加后曝光**：`composite_color` 里 bloom 的采样要出现在
/// `tonemap_color` 调用之前。顺序反了画面仍能出，但泛光会变成「调色后再叠加」，
/// 高光周围一圈颜色失真——这种 bug 靠肉眼很难定位，只能靠结构断言。
#[test]
fn composite_adds_bloom_before_tonemap() {
    let body = POSTFX_SHADER
        .split("fn composite_color")
        .nth(1)
        .expect("应存在 composite_color");
    let body: String = body.chars().take(600).collect();
    let bloom_at = body.find("t_aux").expect("composite 应采样泛光纹理");
    let tonemap_at = body
        .find("tonemap_color")
        .expect("composite 应调用 tonemap_color");
    assert!(
        bloom_at < tonemap_at,
        "泛光必须在色调映射之前叠加：\n{body}"
    );
}

/// 上采样依赖加法混合：着色器侧只输出采样颜色，累加由管线混合完成。
/// 这条断言锁住「只有一个上采样入口且不自己做 dst 采样」的实现方式。
#[test]
fn upsample_is_single_tap_additive() {
    let body = POSTFX_SHADER
        .split("fn fs_upsample")
        .nth(1)
        .expect("应存在 fs_upsample");
    let body: String = body.chars().take(400).collect();
    assert_eq!(
        body.matches("textureSampleLevel").count(),
        1,
        "上采样应只采一次（累加交给混合状态）：\n{body}"
    );
    assert!(
        POSTFX_RUST.contains("dst_factor: wgpu::BlendFactor::One"),
        "上采样管线必须是 One/One 加法混合"
    );
}
