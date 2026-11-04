//! `shader.wgsl` 里局部光源阴影（扩展十八 C）的静态校验：不开窗口、不建 GPU。
//!
//! 局部阴影横跨三处：`r3e_core` 的选面/矩阵纯函数、`renderer.rs` 的 uniform 与纹理布局、
//! `shader.wgsl` 的采样逻辑。任何一处单独改动都会静默失效（层号错位、数组长度脱节、
//! 采样器维度不对），这里用文本 + naga 断言把三者钉在一起。

use r3e_core::{MAX_LOCAL_LIGHTS, MAX_LOCAL_SHADOW_LAYERS};

const SHADER: &str = include_str!("../src/shader.wgsl");
const RENDERER: &str = include_str!("../src/renderer.rs");

fn parsed_module() -> naga::Module {
    match naga::front::wgsl::parse_str(SHADER) {
        Ok(module) => module,
        Err(e) => panic!("shader.wgsl 解析失败：\n{}", e.emit_to_string(SHADER)),
    }
}

#[test]
fn shader_still_parses_and_validates() {
    let module = parsed_module();
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    if let Err(e) = validator.validate(&module) {
        panic!("shader.wgsl 语义校验失败：\n{e:?}");
    }
}

/// 四个字段的顺序必须与 `LocalShadowUniform` 逐字段对齐（错位不报错，只画错）。
#[test]
fn struct_field_order_matches_rust() {
    let fields = ["params", "params2", "view_proj", "slots"];
    let start = SHADER
        .find("struct LocalShadow {")
        .expect("wgsl 缺少 struct LocalShadow");
    let end = start
        + SHADER[start..]
            .find('}')
            .expect("struct LocalShadow 未闭合");
    let body = &SHADER[start..end];
    let mut cursor = 0usize;
    for field in fields {
        let found = body[cursor..]
            .find(&format!("{field}:"))
            .unwrap_or_else(|| panic!("struct LocalShadow 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
    // Rust 侧同名同序
    let struct_start = RENDERER
        .find("pub struct LocalShadowUniform {")
        .expect("renderer 缺少 LocalShadowUniform");
    let struct_end = struct_start + RENDERER[struct_start..].find('}').expect("结构未闭合");
    let rust_body = &RENDERER[struct_start..struct_end];
    let mut cursor = 0usize;
    for field in fields {
        let found = rust_body[cursor..]
            .find(&format!("pub {field}:"))
            .unwrap_or_else(|| panic!("LocalShadowUniform 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
}

/// 矩阵数组 24 层、槽位数组 16 项，必须与 core 常量同值。
#[test]
fn array_lengths_match_core_consts() {
    let needle = format!("array<mat4x4<f32>, {MAX_LOCAL_SHADOW_LAYERS}>");
    assert!(
        SHADER.contains(&needle),
        "wgsl 的矩阵数组长度必须等于 MAX_LOCAL_SHADOW_LAYERS（{MAX_LOCAL_SHADOW_LAYERS}）"
    );
    let needle = format!("array<vec4<f32>, {MAX_LOCAL_LIGHTS}>");
    assert!(
        SHADER.contains(&needle),
        "wgsl 的槽位数组长度必须等于 MAX_LOCAL_LIGHTS（{MAX_LOCAL_LIGHTS}）"
    );
    let const_decl = format!("const MAX_LOCAL_SHADOW_LAYERS: i32 = {MAX_LOCAL_SHADOW_LAYERS};");
    assert!(
        SHADER.contains(&const_decl),
        "wgsl 缺少与之同步的层数常量（下标夹取靠它，缺了会越界读矩阵）"
    );
    // 渲染层的纹理层数也必须来自同一个常量
    assert!(
        RENDERER.contains("r3e_core::MAX_LOCAL_SHADOW_LAYERS as u32"),
        "局部阴影纹理的层数必须来自 MAX_LOCAL_SHADOW_LAYERS"
    );
}

/// 绑定必须落在 group(3) 的 5/6/7 上：WebGPU core 的 `max_bind_groups` 就是 4，
/// 另开 group(4) 会让 `create_shader_module` 直接抛校验错误（窗口一开就 panic）。
#[test]
fn bindings_are_group3_binding_5_6_7() {
    for (binding, decl) in [
        (5, "var local_shadow_map: texture_depth_2d_array;"),
        (6, "var local_shadow_sampler: sampler_comparison;"),
        (7, "var<uniform> u_local_shadow: LocalShadow;"),
    ] {
        let head = format!("@group(3) @binding({binding})");
        let idx = SHADER
            .find(&head)
            .unwrap_or_else(|| panic!("wgsl 缺少 {head}"));
        let tail = &SHADER[idx..idx + 200];
        assert!(
            tail.contains(decl),
            "binding({binding}) 必须声明为 `{decl}`"
        );
    }
    assert!(
        !SHADER.contains("@group(4)"),
        "wgsl 中不允许出现 @group(4)，WebGPU core 上限是 4 个组（0..=3）"
    );
    // 深度比较必须是 Comparison 采样器，否则 textureSampleCompare 校验不过
    assert!(
        RENDERER.contains("wgpu::SamplerBindingType::Comparison"),
        "局部阴影必须用比较采样器"
    );
    assert!(
        RENDERER.contains("r3e-local-shadow-map"),
        "renderer 缺少局部阴影纹理"
    );
}

/// 采样路径必须齐备：选面函数、阴影因子函数，且被逐灯调用。
#[test]
fn sampling_path_is_wired_into_local_lighting() {
    for f in [
        "fn point_shadow_face(",
        "fn local_shadow_factor(",
        "textureSampleCompare(",
    ] {
        assert!(SHADER.contains(f), "wgsl 缺少 {f}");
    }
    let start = SHADER
        .find("fn local_lighting(")
        .expect("wgsl 缺少 local_lighting");
    let end = start + SHADER[start..].find("\n}").expect("local_lighting 未闭合");
    let body = &SHADER[start..end];
    assert!(
        body.contains("local_shadow_factor("),
        "local_lighting 必须逐灯乘上局部阴影因子"
    );
    assert!(
        body.contains("falloff * shadow"),
        "阴影因子必须乘进该灯的直接光项（不参与环境项）"
    );
}

/// 选面分支必须与 `point_shadow_face` 的 0..=5 一一对应（少一个分支就有一面永远读错层）。
#[test]
fn face_branches_cover_six_faces() {
    let start = SHADER
        .find("fn point_shadow_face(")
        .expect("wgsl 缺少 point_shadow_face");
    let end = start + SHADER[start..].find("\n}").expect("函数未闭合");
    let body = &SHADER[start..end];
    for face in 0..6 {
        assert!(
            body.contains(&format!("return {face};")),
            "选面函数缺少第 {face} 面的分支"
        );
    }
    // 面数判定阈值：1.5 用来区分「点光 6 面」与「聚光 1 面」
    assert!(
        SHADER.contains("slot.y > 1.5"),
        "点光/聚光的面数判定阈值被改了，必须与 shadow_faces() 的 6 / 1 同步"
    );
    // 未投影灯的早退条件：slots 默认 -1
    assert!(
        SHADER.contains("slot.x < 0.0"),
        "必须保留「槽位 -1 即不投影」的早退，否则老场景会被算进阴影"
    );
}
