//! `shader.wgsl` 里形变（morph target）路径的静态校验：不开窗口、不建 GPU。
//!
//! 扩展三十七把**切线增量**接进形变表，四处最易单边漂移的约定各一条断言：
//!
//! * `const MORPH_VERTEX_FLOATS` 必须等于 `r3e_core::morph::MORPH_VERTEX_FLOATS`
//!   —— 两边不等时不会报错，只会让每顶点跨段串读（位置读到法线、切线读到下一个顶点）；
//! * `struct MorphDelta` 的字段顺序（position / normal / tangent）与表内
//!   `+0 / +3 / +6` 的偏移必须对应；
//! * 蒙皮段必须用**累积后的** `local_tangent`，退回 `in.tangent.xyz` 会静默吞掉切线增量；
//! * 形变表仍是 `@group(2) @binding(1)` 的只读 storage，实例元数据没换 group。

use r3e_core::morph::MORPH_VERTEX_FLOATS;

const SHADER: &str = include_str!("../src/shader.wgsl");

fn parsed_module() -> naga::Module {
    match naga::front::wgsl::parse_str(SHADER) {
        Ok(module) => module,
        Err(e) => panic!("shader.wgsl 解析失败：\n{}", e.emit_to_string(SHADER)),
    }
}

/// 截取一个函数体的文本切片（从 `fn <name>(` 到下一个顶层 `\n}`）。
fn fn_body(name: &str) -> &str {
    let start = SHADER
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("wgsl 缺少 fn {name}"));
    let rest = &SHADER[start..];
    let end = rest.find("\n}\n").expect("函数体未闭合");
    &rest[..end]
}

/// 截取一个 struct 的字段区。
fn struct_body(name: &str) -> &str {
    let start = SHADER
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("wgsl 缺少 struct {name}"));
    let rest = &SHADER[start..];
    let end = rest.find('}').expect("struct 未闭合");
    &rest[..end]
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

/// 每顶点浮点数必须与 Rust 常量一字不差：错了不会编译失败，只会读错段。
#[test]
fn per_vertex_floats_matches_core_const() {
    let needle = format!("const MORPH_VERTEX_FLOATS: u32 = {MORPH_VERTEX_FLOATS}u;");
    assert!(
        SHADER.contains(&needle),
        "期望 `{needle}`（要同步改 r3e_core::morph::MORPH_VERTEX_FLOATS 与本行）"
    );
}

/// 三组增量的字段顺序 = 表内偏移顺序：position(+0) / normal(+3) / tangent(+6)。
#[test]
fn morph_delta_field_order_matches_table_offsets() {
    let body = struct_body("MorphDelta");
    let mut cursor = 0usize;
    for (field, offset) in [("position", 0), ("normal", 3), ("tangent", 6)] {
        let found = body[cursor..]
            .find(field)
            .unwrap_or_else(|| panic!("struct MorphDelta 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
        // 取值处的偏移必须跟着字段走：第 1 个形状（weights.x）即代表全部形状。
        // offset 0 写成裸 `at`（不加 `+ 0u`），其余带显式偏移。
        let read = if offset == 0 {
            "morph_table[at], morph_table[at + 1u], morph_table[at + 2u]".to_string()
        } else {
            format!(
                "morph_table[at + {offset}u], morph_table[at + {}u], morph_table[at + {}u]",
                offset + 1,
                offset + 2
            )
        };
        assert!(
            SHADER.contains(&read),
            "形状 0 的 {field} 增量应按 offset {offset} 读，找不到 `{read}`"
        );
        if offset > 0 {
            let weight_read = format!("weights.x * vec3<f32>({read})");
            assert!(
                SHADER.contains(&weight_read),
                "{field} 增量必须按权重累加，找不到 `{weight_read}`"
            );
        }
    }
}

/// 形变给出的切线增量必须活到蒙皮之后：蒙皮用的是累积后的 `local_tangent`。
#[test]
fn tangent_delta_survives_skinning() {
    let vs = fn_body("vs_main");
    assert!(
        vs.contains("local_tangent += shape.tangent"),
        "顶点着色器必须把切线增量加进 local_tangent"
    );
    assert!(
        vs.contains("local_tangent = skin_adjugate * local_tangent"),
        "蒙皮必须作用在累积后的 local_tangent 上（退回 in.tangent.xyz 会吞掉形变）"
    );
    assert!(
        !vs.contains("skin_adjugate * in.tangent.xyz"),
        "蒙皮段禁止直接用未累积的 in.tangent.xyz"
    );
}

/// 阴影通道的顶点变换同样要吃形变：只改主通道会让影子留在静止姿态。
#[test]
fn shadow_vertex_stage_also_applies_morph() {
    let vs = fn_body("vs_shadow");
    assert!(
        vs.contains("morph_offset("),
        "vs_shadow 必须调用 morph_offset，否则影子不跟着形变"
    );
}

/// 形变表仍是 group(2) 的 binding(1) 只读 storage：换组会让绑定组布局对不上。
#[test]
fn morph_table_binding_is_group2_binding1() {
    let decl = SHADER
        .find("var<storage, read> morph_table")
        .expect("wgsl 缺少 morph_table 声明");
    // 按行取声明前面的属性行，避免在多字节字符中间切串
    let before: String = SHADER[..decl]
        .lines()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .join("\n");
    let before = before.as_str();
    assert!(
        before.contains("@group(2) @binding(1)"),
        "morph_table 的绑定位置变了，实际前置文本：{before}"
    );
}
