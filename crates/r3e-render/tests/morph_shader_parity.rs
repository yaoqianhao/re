//! `shader.wgsl` 里形变（morph target）路径的静态校验：不开窗口、不建 GPU。
//!
//! 扩展三十七把**切线增量**接进形变表，扩展三十九把**权重从 1 组 `vec4` 扩到多组**
//! （形状数 4 → 8），四处最易单边漂移的约定各一条断言：
//!
//! * `const MORPH_VERTEX_FLOATS` 必须等于 `r3e_core::morph::MORPH_VERTEX_FLOATS`
//!   —— 两边不等时不会报错，只会让每顶点跨段串读（位置读到法线、切线读到下一个顶点）；
//! * `struct MorphDelta` 的字段顺序（position / normal / tangent）与表内
//!   `+0 / +3 / +6` 的偏移必须对应；
//! * 蒙皮段必须用**累积后的** `local_tangent`，退回 `in.tangent.xyz` 会静默吞掉切线增量；
//! * 形变表仍是 `@group(2) @binding(1)` 的只读 storage，实例元数据没换 group。
//!
//! 扩展三十九再加三条：`MORPH_SHAPES` 与 `MAX_MORPH_TARGETS` 一致、第 2 组权重接到
//! 形状 4..8、主通道与阴影通道都声明了两组权重属性。
//!
//! 扩展四十（逐实例形状掩码）再加三条：`@location(14)` 在两个顶点入口都必须是
//! `morph_mask: u32`、两处 `morph_offset` 调用都要把掩码传进去（漏传的通道形变
//! 不受掩码约束，就是「影子还在笑、脸已经不笑了」）、掩码乘子必须是 0/1 取值
//! （写错成 0..4 会直接把权重乘成倍率）。

use r3e_core::morph::{MAX_MORPH_TARGETS, MORPH_VERTEX_FLOATS};
use r3e_core::MORPH_WEIGHT_GROUPS;
use r3e_render::mesh::{MORPH_MASK_LOCATION, MORPH_WEIGHT_BASE_LOCATION, MORPH_WEIGHT_OFFSET};

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
            let weight_read = format!("w * vec3<f32>({read})");
            assert!(
                SHADER.contains(&weight_read),
                "{field} 增量必须按权重累加，找不到 `{weight_read}`"
            );
        }
    }
}

/// 扩展三十九：着色器侧的形状上限必须等于 core 的 `MAX_MORPH_TARGETS`（= 组数 × 4）。
///
/// 这条最危险：着色器写 4、core 写 8 时一切照常编译，只是第 5..8 个形状
/// 静默不生效 —— 脚本设了权重、顶点却不动，没有任何报错。
#[test]
fn morph_shapes_matches_core_const() {
    let needle = format!("const MORPH_SHAPES: u32 = {MAX_MORPH_TARGETS}u;");
    assert!(
        SHADER.contains(&needle),
        "期望 `{needle}`（要同步改 r3e_core::MAX_MORPH_TARGETS 与本行）"
    );
    assert_eq!(
        MAX_MORPH_TARGETS,
        MORPH_WEIGHT_GROUPS * 4,
        "形状数必须恰好填满每组 4 个分量的权重属性"
    );
}

/// 第二组权重必须真的接到第 5..8 个形状上：`w1` 的第 i 个分量配 `stride * (4+i)`。
///
/// 少接一条会在「形状 5 的权重被读进形状 1」这类错位上静默出错 —— 形变看着像在动，
/// 只是动的是另一张脸。
#[test]
fn second_weight_group_feeds_high_shapes() {
    // 扩展四十把签名拆成多行（多了 `mask` 参数），按单参数匹配而不是整行拼接
    let signature = fn_body("morph_offset");
    for group in ["w0", "w1"] {
        let expected = format!("{group}: vec4<f32>");
        assert!(
            signature.contains(&expected),
            "morph_offset 必须接收第 {group} 组权重（参数 `{expected}`）"
        );
    }
    // 扩展四十起权重先过掩码乘子（`on1` = w1 × 掩码高 4 位），分量对应关系不变
    for (lane, shape) in [("x", 4), ("y", 5), ("z", 6), ("w", 7)] {
        let expected = format!("morph_accumulate(&delta, head + stride * {shape}u, on1.{lane})");
        assert!(
            SHADER.contains(&expected),
            "形状 {shape} 应由 w1.{lane} 驱动，找不到 `{expected}`"
        );
    }
}

/// 两个顶点入口都要带上两组权重属性（location 12 / 13）：漏一个的通道会拿到全 0
/// 权重，该通道里的形变整体失效（阴影通道漏了就是「影子不动」）。
#[test]
fn both_vertex_stages_declare_two_weight_groups() {
    for stage in ["VsIn", "ShadowVsIn"] {
        let body = struct_body(stage);
        for group in 0..MORPH_WEIGHT_GROUPS {
            let location = 12 + group as u32;
            let expected = format!("@location({location})");
            assert!(
                body.contains(&expected),
                "{stage} 缺少第 {group} 组权重属性 `{expected}`"
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

/// 扩展四十（契约一）：掩码属性必须同时出现在两个顶点入口，且类型是 `u32`。
///
/// location 取 `r3e_render::mesh::MORPH_MASK_LOCATION`（= 权重区之后），
/// 与顶点缓冲布局共用同一个数 —— 手抄一个 14 在两边，改权重组数时会先红在这里。
#[test]
fn both_vertex_stages_declare_morph_mask() {
    let location = MORPH_MASK_LOCATION;
    assert_eq!(
        location,
        MORPH_WEIGHT_BASE_LOCATION + MORPH_WEIGHT_GROUPS as u32,
        "掩码必须紧跟在最后一组权重之后，中间不留空位"
    );
    let expected = format!("@location({location}) morph_mask: u32");
    for stage in ["VsIn", "ShadowVsIn"] {
        let body = struct_body(stage);
        assert!(
            body.contains(&expected),
            "{stage} 缺少掩码属性 `{expected}`"
        );
    }
    // 顶点属性区没有被误占：0..=4 与 11 是顶点属性
    assert!(location > 11, "掩码 location 与顶点切线（11）撞号");
}

/// 扩展四十（契约二）：两处 `morph_offset` 调用都要传掩码。
///
/// 阴影通道漏传时编译就会失败（少一个参数），但**传错位**不会 —— 比如把
/// `in.morph_mask` 塞到 `vi` 的位置、把 `in.vertex_index` 塞到 mask 的位置，
/// 类型恰好都是 `u32`，于是影子的形变按「顶点下标作掩码」跑，静默错位。
#[test]
fn both_stages_pass_mask_into_morph_offset() {
    for stage in ["vs_main", "vs_shadow"] {
        let body = fn_body(stage);
        assert!(
            body.contains("in.morph_mask"),
            "{stage} 必须把实例掩码传给 morph_offset"
        );
        // 掩码紧跟两组权重之后、顶点下标之前
        let mask_at = body.find("in.morph_mask").expect("上面已断言");
        let vi_at = body
            .find("in.vertex_index")
            .unwrap_or_else(|| panic!("{stage} 没传 vertex_index"));
        assert!(
            mask_at < vi_at,
            "{stage} 里掩码必须排在 vertex_index 之前（签名顺序 mask, vi）"
        );
    }
}

/// 扩展四十（契约三）：掩码乘子只产出 0 / 1，且两组权重要分别右移 0 / 4 位。
///
/// `select(0.0, 1.0, ...)` 逐位展开是「无分支」的写法：掩码是实例级数据，
/// 同批次内每实例可能不同，用 `if` 会让 warp 内部发散。乘子若写成别的值
/// （把权重当倍率放大），掩码就从「开关」变成了「增益」，与场景侧
/// `MorphInstance::shape_mask()` 的语义（位 = 启用/停用）彻底对不上。
#[test]
fn mask_multiplier_is_binary_and_splits_groups() {
    let body = fn_body("morph_mask_vec");
    for bit in ["1u", "2u", "4u", "8u"] {
        let expected = format!("(m & {bit}) != 0u");
        assert!(
            body.contains(&expected),
            "morph_mask_vec 缺少位判断 `{expected}`"
        );
    }
    assert_eq!(
        body.matches("select(0.0, 1.0,").count(),
        4,
        "4 个分量各一个 0/1 选择，多一个少一个都会错位"
    );
    assert!(
        !body.contains("f32(m)"),
        "掩码不能直接当数值参与乘加，必须经 select 归一成 0/1"
    );

    let offset = fn_body("morph_offset");
    assert!(
        offset.contains("morph_mask_vec(mask, 0u)"),
        "第 1 组（形状 0..4）按掩码低 4 位筛"
    );
    assert!(
        offset.contains("morph_mask_vec(mask, 4u)"),
        "第 2 组（形状 4..8）按掩码高 4 位筛"
    );
}

/// 扩展四十（契约四）：掩码挪进实例缓冲后，权重区偏移不能跟着漂。
///
/// 掩码占**权重区之后**的 4 字节，不是塞进权重区里 —— 权重偏移一变，
/// `@location(12)` 就读到别的字段上，整个形变会按脏权重跑。
#[test]
fn mask_offset_sits_after_the_weight_block() {
    let mesh_src = include_str!("../src/mesh.rs");
    assert!(
        mesh_src.contains(
            "MORPH_MASK_OFFSET: u32 = MORPH_WEIGHT_OFFSET + 16 * MORPH_WEIGHT_GROUPS as u32"
        ),
        "掩码偏移必须由权重偏移 + 组数推出，不能手抄绝对值"
    );
    assert_eq!(
        MORPH_WEIGHT_OFFSET, 96,
        "权重区起点 = model(64) + color(16) + 元数据(16)"
    );
}
