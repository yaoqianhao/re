//! 扩展十六（CSM）的 GPU 侧契约守卫。
//!
//! 级联阴影横跨三处：`r3e_core` 的纯函数、`renderer.rs` 的 uniform / 纹理布局与
//! `shader.wgsl` 的采样逻辑。任何一处单独改动都会静默失效（矩阵错位、层数不匹配、
//! 采样器维度不对），这里用文本断言把三者钉在一起，改一处必须同步改三处。

use r3e_core::MAX_CASCADES;

const SHADER: &str = include_str!("../src/shader.wgsl");
const RENDERER: &str = include_str!("../src/renderer.rs");

/// 把连续空白压成单个空格，避免断言被缩进/换行踩到。
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 在 [`flat`] 之上再抹掉「方括号贴空格」的排版差异。
///
/// rustfmt 会把短数组压成一行（`[a, b]`）、长数组拆成多行（`[\n a,` 压完是 `[ a`），
/// 同一段代码换个行长就变形状。断言关心的是字段与顺序，不该被排版踩到。
fn tight(s: &str) -> String {
    flat(s).replace("[ ", "[").replace(" ]", "]")
}

#[test]
fn shadow_map_is_a_depth_array_with_matching_layers() {
    assert!(
        SHADER.contains("texture_depth_2d_array"),
        "阴影图必须声明为深度数组（每级一层）"
    );
    assert!(
        !SHADER.contains("var shadow_map: texture_depth_2d;"),
        "阴影图不应再是单层 2D 纹理"
    );
    assert!(
        RENDERER.contains("view_dimension: wgpu::TextureViewDimension::D2Array"),
        "绑定布局的 binding 0 必须是 D2Array"
    );
    assert!(
        RENDERER.contains("depth_or_array_layers: layers,"),
        "分层深度数组的层数必须来自 layers 参数"
    );
    // 扩展十八 C 把方向光级联与局部阴影的建纹理逻辑抽成 create_shadow_array，
    // 因此这里改成断言「调用处传的是 MAX_CASCADES」，语义与原来的字面量断言等价。
    let map_label = RENDERER
        .find("\"r3e-shadow-map\"")
        .expect("renderer 缺少方向光阴影纹理");
    let layers_arg = RENDERER
        .find("MAX_CASCADES as u32")
        .expect("方向光阴影纹理数组层数必须等于 MAX_CASCADES");
    assert!(
        layers_arg < map_label && map_label - layers_arg < 120,
        "方向光阴影纹理的层数参数必须是 MAX_CASCADES"
    );
}

#[test]
fn shader_matrix_array_length_matches_core_const() {
    let needle = format!("array<mat4x4<f32>, {MAX_CASCADES}>");
    assert!(
        SHADER.contains(&needle),
        "shader 里级联矩阵数组长度必须与 MAX_CASCADES 一致（期望 `{needle}`）"
    );
}

#[test]
fn shadow_uniform_field_order_matches_rust() {
    // WGSL 与 #[repr(C)] 的字段顺序必须严格一致，错位会静默读出垃圾矩阵
    let struct_start = SHADER
        .find("struct Shadow {")
        .expect("shader 缺少 Shadow 结构");
    let struct_end = SHADER[struct_start..]
        .find("};")
        .map(|i| struct_start + i)
        .expect("Shadow 结构未闭合");
    let body = &SHADER[struct_start..struct_end];

    let order = ["light_view_proj", "split_ndc", "params", "params2"];
    let mut last = 0;
    for field in order {
        let idx = body
            .find(field)
            .unwrap_or_else(|| panic!("Shadow 缺少字段 {field}"));
        assert!(idx >= last, "Shadow 字段顺序必须为 {order:?}");
        last = idx;
    }

    assert!(
        RENDERER.contains("light_view_proj: [[[f32; 4]; 4]; MAX_CASCADES],")
            && RENDERER.contains("split_ndc: [f32; 4],"),
        "renderer 侧 ShadowUniform 的字段顺序必须与 shader 一致"
    );
}

#[test]
fn per_level_matrix_and_split_table_are_uploaded() {
    assert!(
        RENDERER.contains("r3e_core::cascade_splits(camera, cascade)"),
        "renderer 必须用 core 的 cascade_splits 计算切分点（不要在 GPU 侧另推一套）"
    );
    // 扩展二十四把 fit 换成 fit_cascade_extended（把级联近端朝光源推 caster_extend）。
    // extend = 0 时它与 fit_cascade 逐位相同，老场景矩阵不变。
    assert!(
        RENDERER.contains("r3e_core::fit_cascade_extended(")
            && RENDERER.contains("cascade.caster_extend,"),
        "每级矩阵必须来自 core 的 fit_cascade_extended，且外扩量取自 cascade.caster_extend"
    );
    assert!(
        RENDERER.contains("shadow_pass_uniforms: Vec<wgpu::Buffer>"),
        "每级需要独立的光源矩阵 uniform（深度 pass 逐级重绑）"
    );
    // 扩展二十四起，循环上界取 `ShadowPlan::levels`（0 = 本帧阴影关闭，一次 pass 都不跑），
    // 每轮从 plan.masks 取该级的保留集合。
    assert!(
        RENDERER.contains("for level in 0..shadow_plan.levels as usize"),
        "深度 pass 必须逐级各跑一遍，且级数为 0 时整体跳过"
    );
}

#[test]
fn sampling_selects_level_by_camera_depth_and_blends() {
    assert!(
        SHADER.contains("fn shadow_sample_level("),
        "需要按级采样的辅助函数"
    );
    assert!(
        SHADER.contains("u_shadow.split_ndc[i]"),
        "选级必须比较各级远端的 NDC z"
    );
    // textureSampleCompare 的数组纹理重载必须带上层号，否则采到第 0 层就永远不变
    let shader = flat(SHADER);
    assert!(
        shader.contains("uv + offset, level, cmp_depth,"),
        "textureSampleCompare 必须传入级号"
    );
    assert!(
        SHADER.contains("params2.z") && SHADER.contains("mix(visible, next_visible, t)"),
        "级间必须按 blend 带宽做混合"
    );
    assert!(
        tight(RENDERER)
            .contains("params2: [shadow.strength, levels as f32, shadow.cascade.blend, 0.0],"),
        "params2 必须上传 强度 / 级数 / 混合带宽（尾位留给对齐）"
    );
}

#[test]
fn single_cascade_keeps_stage_d_behavior() {
    // 默认 count = 1：级数为 1 时 shader 只选第 0 层，等价阶段 D 的单张阴影图
    let default_count = r3e_core::CascadeSettings::default().count;
    assert_eq!(default_count, 1, "默认必须单级，老场景画面不变");
    assert!(
        flat(SHADER).contains("if levels <= 0 { return 1.0;"),
        "级数为 0 时必须整段跳过（关闭阴影零开销）"
    );
}
