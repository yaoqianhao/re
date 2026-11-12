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
//! 扩展四十二（形状槽列表）把这套约定整体换底，六条：
//!
//! * `const MORPH_SLOTS` 等于 `MORPH_SLOT_CAPACITY`（= 组数 × 4），而**不再**等于
//!   `MAX_MORPH_TARGETS` —— 形状总数已经解除 32 上限，槽只是「同时启用」的容量；
//! * 三组序号字 `morph_s0/s1/s2` 在两个顶点入口都是 `u32`，location 由
//!   `MORPH_SLOT_LOCATION_BASE` 推出；
//! * 两处 `morph_offset` 调用都要按 `s0, s1, s2, vi` 的顺序传（顺序错位类型都合法，
//!   只会静默算出别的形状）；
//! * 形状序号必须**逐字节**从字里取（`(word >> (lane * 8u)) & 0xFFu`），
//!   写成 `word & 0xFFu` 会让 12 个槽全落到同一个形状上；
//! * 序号字必须紧跟权重区，偏移由 `MORPH_WEIGHT_OFFSET + 16 × 组数` 推出；
//! * 掩码乘子（`morph_mask_vec`）必须彻底消失：留着它就等于「序号定了、又被掩码清零」，
//!   两套开关同时存在，谁说话算数说不清。

use r3e_core::morph::MORPH_VERTEX_FLOATS;
use r3e_core::{MAX_MORPH_TARGETS, MORPH_SLOT_CAPACITY, MORPH_WEIGHT_GROUPS};
use r3e_render::mesh::{
    MORPH_SLOT_ATTR_WORDS, MORPH_SLOT_LOCATION_BASE, MORPH_SLOT_OFFSET, MORPH_SLOT_WORDS,
};
use r3e_render::mesh::{MORPH_WEIGHT_BASE_LOCATION, MORPH_WEIGHT_OFFSET};

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

/// 扩展四十二：着色器的槽容量必须等于 core 的 `MORPH_SLOT_CAPACITY`（= 组数 × 4）。
///
/// 这条最危险：着色器写 12、core 写 8 时一切照常编译，只是第 9..12 个槽
/// 静默不生效 —— 脚本设了权重、顶点却不动，没有任何报错。
///
/// 与扩展四十一的差别：这里比的是**槽容量**，不是网格形状数上限。形状总数
/// 现在只受 `u8` 序号（256）与本机形变表大小限制，`MAX_MORPH_TARGETS` 反而
/// 不该再和这个常量相等 —— 相等就说明「上限还在 32」的旧假设没被拆掉。
#[test]
fn morph_slots_matches_core_const() {
    let needle = format!("const MORPH_SLOTS: u32 = {MORPH_SLOT_CAPACITY}u;");
    assert!(
        SHADER.contains(&needle),
        "期望 `{needle}`（要同步改 r3e_core::MORPH_SLOT_CAPACITY 与本行）"
    );
    assert!(
        !SHADER.contains("const MORPH_SHAPES"),
        "旧常量 MORPH_SHAPES 必须删掉：它的语义是「形状数上限」，已被槽容量取代"
    );
    assert_eq!(
        MORPH_SLOT_CAPACITY,
        MORPH_WEIGHT_GROUPS * 4,
        "槽容量必须恰好填满每组 4 个分量的权重属性"
    );
    assert!(
        MAX_MORPH_TARGETS <= u8::MAX as usize,
        "形状序号按 u8 打包进序号字，网格形状总数不能超过 256"
    );
}

/// 第二组权重必须真的接到第 5..8 个槽上：`w1` 的第 i 个分量配 `s1` 的第 i 个字节。
///
/// 少接一条会在「槽 5 的权重被读进槽 1」这类错位上静默出错 —— 形变看着像在动，
/// 只是动的是另一张脸。扩展四十二之后 断言对象从「形状常量」变成「(字, 字节) 对」：
/// 权重仍静态展开，序号是动态的，所以这条断言钉的是**配对**而不是偏移值。
#[test]
fn weight_groups_pair_with_slot_words() {
    let signature = fn_body("morph_offset");
    for group in ["w0", "w1", "w2"] {
        let expected = format!("{group}: vec4<f32>");
        assert!(
            signature.contains(&expected),
            "morph_offset 必须接收第 {group} 组权重（参数 `{expected}`）"
        );
    }
    assert!(
        signature.contains("slots: vec4<u32>"),
        "morph_offset 必须接收序号 vec4（一个字属性装 4 个字：x/y/z 有效、w 占位）"
    );
    // 每组 4 个槽：第 g 组权重 × 序号 vec4 的第 g 个分量，字节下标 0..4
    for (group, word) in [("w0", "slots.x"), ("w1", "slots.y"), ("w2", "slots.z")] {
        for (lane, index) in [("x", 0), ("y", 1), ("z", 2), ("w", 3)] {
            let expected = format!(
                "morph_slot_accumulate(&delta, head, stride, {word}, {index}u, {group}.{lane})"
            );
            assert!(
                SHADER.contains(&expected),
                "{group}.{lane} 应驱动 {word} 的第 {index} 槽，找不到 `{expected}`"
            );
        }
    }
    assert!(
        !SHADER.contains("slots.w"),
        "第 4 个字是占位（MORPH_SLOT_ATTR_WORDS = 4 > 3 个真实字），不该被读"
    );
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

/// 扩展四十二（契约一）：三组序号字必须同时出现在两个顶点入口，且类型是 `u32`。
///
/// location 由 `MORPH_SLOT_LOCATION_BASE` 推出（= 权重区之后），与顶点缓冲布局
/// 共用同一个数 —— 手抄 location 在两边，改权重组数时会先红在这里。
#[test]
fn both_vertex_stages_declare_morph_slots() {
    assert_eq!(
        MORPH_SLOT_LOCATION_BASE,
        MORPH_WEIGHT_BASE_LOCATION + MORPH_WEIGHT_GROUPS as u32,
        "序号字必须紧跟在最后一组权重之后，中间不留空位"
    );
    assert_eq!(MORPH_SLOT_WORDS, 3, "12 槽 = 3 个字，加组时这里先红");
    const {
        assert!(
            MORPH_SLOT_WORDS <= MORPH_SLOT_ATTR_WORDS,
            "真实字数装不进属性 vec4"
        )
    };
    assert_eq!(
        MORPH_SLOT_ATTR_WORDS, 4,
        "属性固定 4 字：location 只剩一格，摊成多个 Uint32 会越过 16 格上限"
    );
    for stage in ["VsIn", "ShadowVsIn"] {
        let body = struct_body(stage);
        let expected = format!("@location({MORPH_SLOT_LOCATION_BASE}) morph_slots: vec4<u32>");
        assert!(
            body.contains(&expected),
            "{stage} 缺少序号属性 `{expected}`"
        );
        // 越界的 location 会在 create_render_pipeline 时被拒（真机踩过）
        for word in 1..MORPH_SLOT_WORDS {
            let forbidden = format!("@location({})", MORPH_SLOT_LOCATION_BASE + word as u32);
            assert!(
                !body.contains(&forbidden),
                "{stage} 不该再有 `{forbidden}`：3 个字压进一个 vec4，location 只有 16 格"
            );
        }
    }
    const {
        assert!(
            MORPH_SLOT_LOCATION_BASE < 16,
            "序号属性必须落在 webgpu 的 0..=15 之内"
        )
    };
    // 顶点属性区没有被误占：0..=4 与 11 是顶点属性
    const {
        assert!(
            MORPH_SLOT_LOCATION_BASE > 11,
            "序号 location 与顶点切线（11）撞号"
        )
    };
}

/// 扩展四十二（契约二）：两处 `morph_offset` 调用都要按 `slots, vi` 顺序传。
///
/// 阴影通道漏传时编译就会失败（少参数），但**传错位**不会 —— 序号 vec4 与
/// `vertex_index` 的类型不同（`vec4<u32>` vs `u32`），所以这里只要钉住「都传了、
/// 顺序对」，剩下的交给 naga 校验。
#[test]
fn both_stages_pass_slot_words_into_morph_offset() {
    for stage in ["vs_main", "vs_shadow"] {
        let body = fn_body(stage);
        let mut cursor = 0usize;
        for token in ["in.morph_slots", "in.vertex_index"] {
            let found = body[cursor..]
                .find(token)
                .unwrap_or_else(|| panic!("{stage} 没按顺序传 {token}"));
            cursor += found + token.len();
        }
    }
}

/// 扩展四十二（契约三）：形状序号必须**逐字节**从序号字里取。
///
/// 写成 `word & 0xFFu`（少乘 `lane * 8`）时 12 个槽会全落到同一个形状上 ——
/// 屏幕上是「只有一个形变在动」，不报错、不越界，最难查。
#[test]
fn slot_index_is_extracted_per_byte() {
    let body = fn_body("morph_slot_shape");
    assert!(
        body.contains("(word >> (lane * 8u)) & 0xFFu"),
        "序号按 u8 小端逐字节取，找不到移位与掩码组合"
    );
    assert!(
        body.contains("-> u32"),
        "序号必须是 u32 才能参与 `stride * shape`"
    );

    let slot = fn_body("morph_slot_accumulate");
    assert!(
        slot.contains("head + stride * shape"),
        "表偏移必须是 `stride × 运行时序号`，静态偏移说明又退回逐个形状展开了"
    );
    assert!(
        slot.contains("morph_slot_shape(word, lane)"),
        "槽内必须按 (字, 字节) 取序号，而不是外部算好传进来"
    );
    let offset = fn_body("morph_offset");
    assert!(
        !offset.contains("morph_mask"),
        "morph_offset 里不能再出现掩码：序号已经是唯一开关"
    );
}

/// 扩展四十二（契约四）：掩码乘子必须彻底消失，别留成「第二套开关」。
///
/// 序号字与掩码同时存在时，语义会分裂成「序号说读形状 5、掩码说形状 5 停用」——
/// 两套数据谁说话算数没有唯一答案，脚本侧 `shape_mask()` 与上传侧槽列表也可能
/// 各说各话。掩码能力已由「不出现在槽列表里」等价替代。
#[test]
fn mask_multiplier_is_gone() {
    assert!(
        !SHADER.contains("morph_mask_vec"),
        "morph_mask_vec 必须删除，掩码已由槽列表压缩等价替代"
    );
    assert!(
        !SHADER.contains("morph_mask"),
        "shader.wgsl 里不应再有 morph_mask 相关标识符"
    );
}

/// 扩展四十二（契约五）：序号字区域紧跟权重区，偏移由权重偏移推出。
///
/// 序号字占**权重区之后**的 `4 × 字数` 字节，不是塞进权重区里 —— 权重偏移一变，
/// `@location(12)` 就读到别的字段上，整个形变会按脏权重跑。
#[test]
fn slot_offset_sits_after_the_weight_block() {
    let mesh_src = include_str!("../src/mesh.rs");
    assert!(
        mesh_src.contains(
            "MORPH_SLOT_OFFSET: u32 = MORPH_WEIGHT_OFFSET + 16 * MORPH_WEIGHT_GROUPS as u32"
        ),
        "序号字偏移必须由权重偏移 + 组数推出，不能手抄绝对值"
    );
    assert_eq!(
        MORPH_WEIGHT_OFFSET, 96,
        "权重区起点 = model(64) + color(16) + 元数据(16)"
    );
    assert_eq!(MORPH_SLOT_OFFSET, 144, "96 + 48（3 组权重）");
}
