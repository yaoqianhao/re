//! `shader.wgsl` 里局部光源（扩展十五）的静态校验：不开窗口、不建 GPU。
//!
//! 三条最易单边漂移的约定各一条断言：
//!
//! * `struct LocalLight` 的**字段顺序**与 `LocalLightRaw` 逐字段对齐（错位不报错，只画错）；
//! * 数组长度 `array<LocalLight, 16>` 与 `MAX_LOCAL_LIGHTS` 一致；
//! * 绑定位置 `@group(4) @binding(0)` 与 renderer 的 pipeline layout 一致。

use r3e_core::{ATTENUATION_EPSILON, ATTENUATION_WINDOW_POWER, MAX_LOCAL_LIGHTS};

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

#[test]
fn entry_points_unaffected() {
    let module = parsed_module();
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for expected in ["vs_main", "fs_main", "vs_shadow"] {
        assert!(
            names.contains(&expected),
            "缺少入口点 {expected}，实际 {names:?}"
        );
    }
}

/// 四个 vec4 的顺序：`pos_range / color_power / dir_inner / outer_kind`。
/// 顺序错了不会有编译错误，只会让「半径」被当成「强度」用。
#[test]
fn local_light_struct_field_order_matches_rust() {
    let fields = ["pos_range", "color_power", "dir_inner", "outer_kind"];
    let start = SHADER
        .find("struct LocalLight {")
        .expect("wgsl 缺少 struct LocalLight");
    let end = start + SHADER[start..].find('}').expect("struct LocalLight 未闭合");
    let body = &SHADER[start..end];
    let mut cursor = 0usize;
    for field in fields {
        let found = body[cursor..]
            .find(field)
            .unwrap_or_else(|| panic!("struct LocalLight 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
    // Rust 侧必须同名同序
    let mut cursor = 0usize;
    for field in fields {
        let found = RENDERER[cursor..]
            .find(&format!("pub {field}:"))
            .unwrap_or_else(|| panic!("LocalLightRaw 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
}

#[test]
fn lights_array_length_matches_core_const() {
    let needle = format!("array<LocalLight, {MAX_LOCAL_LIGHTS}>");
    assert!(
        SHADER.contains(&needle),
        "wgsl 的数组长度必须等于 MAX_LOCAL_LIGHTS（{MAX_LOCAL_LIGHTS}），否则超出部分被静默丢弃"
    );
    let count_const = format!("const MAX_LOCAL_LIGHTS: u32 = {MAX_LOCAL_LIGHTS}u;");
    assert!(
        SHADER.contains(&count_const),
        "wgsl 缺少与之同步的 MAX_LOCAL_LIGHTS 常量"
    );
}

/// 光源表挂在 group(3) 的 binding(4) 上，**不能**另开 group(4)。
///
/// WebGPU core 规范里 `max_bind_groups` 就是 4（索引 0..=3），第 5 个组会直接
/// 让 `create_shader_module` 抛校验错误——窗口一开就 panic。这条测试把这个
/// 布局约束钉死，免得后续「顺手加个 group」再把启动打挂。
#[test]
fn lights_binding_is_group3_binding4() {
    assert!(
        SHADER.contains("@group(3) @binding(4)"),
        "局部光源表必须占 @group(3) @binding(4)（group(4) 超出 max_bind_groups）"
    );
    assert!(
        SHADER.contains("var<uniform> u_lights: Lights;"),
        "局部光源表必须是 uniform 变量 u_lights"
    );
    // shader 里不该再出现任何 group(4)：一旦出现就是又一次启动即 panic
    assert!(
        !SHADER.contains("@group(4)"),
        "wgsl 中不允许出现 @group(4)，WebGPU core 上限是 4 个组（0..=3）"
    );
    // renderer 侧：光源 buffer 由 shadow 布局的 binding 4 承载，且不单开绑定组
    assert!(
        RENDERER.contains("r3e-lights-ubo"),
        "renderer 缺少局部光源 uniform buffer"
    );
    assert!(
        !RENDERER.contains("r3e-lights-bgl"),
        "不应再单独创建光源绑定组布局（已并入 shadow_layout 的 binding 4）"
    );
    assert!(
        !RENDERER.contains("lights_bind_group"),
        "不应再有独立的 lights_bind_group"
    );
    // shadow_layout 的 entries 里必须声明 binding 4
    let idx = RENDERER
        .find("r3e-shadow-bgl")
        .expect("renderer 缺少 r3e-shadow-bgl");
    let tail = &RENDERER[idx..idx + 4000];
    assert!(
        tail.contains("binding: 4"),
        "shadow_layout 必须声明 binding 4（局部光源表）"
    );
}

/// CPU 与 wgsl 的衰减常数必须同源：`ATTENUATION_EPSILON` / `ATTENUATION_WINDOW_POWER`。
/// 只改一边会让「近处亮度」整体偏移，肉眼极难察觉。
#[test]
fn attenuation_constants_match_core() {
    // 按数值比较而非按文本比较：wgsl 里可以写 `4.0` 也可以写 `4`，两者等价
    fn wgsl_const(name: &str) -> f32 {
        let head = format!("const {name}: f32 = ");
        let start = SHADER
            .find(&head)
            .unwrap_or_else(|| panic!("wgsl 缺少常量 {name}"))
            + head.len();
        let end = start + SHADER[start..].find(';').expect("常量声明未以分号结尾");
        SHADER[start..end]
            .trim()
            .parse::<f32>()
            .unwrap_or_else(|e| panic!("常量 {name} 的字面量无法解析：{e}"))
    }
    assert_eq!(wgsl_const("ATTENUATION_EPSILON"), ATTENUATION_EPSILON);
    // core 侧是 i32（幂次只取整数），wgsl 里写成 f32 常数，按数值比较即可
    assert_eq!(
        wgsl_const("ATTENUATION_WINDOW_POWER"),
        ATTENUATION_WINDOW_POWER as f32
    );
}

/// 三个纯函数必须齐备，且主通道确实调用了 `local_lighting`。
#[test]
fn lighting_helpers_exist_and_are_used() {
    for f in [
        "fn local_attenuation(",
        "fn local_spot_factor(",
        "fn local_lighting(",
    ] {
        assert!(SHADER.contains(f), "wgsl 缺少 {f}");
    }
    let fs_main = SHADER.find("fn fs_main(").expect("wgsl 缺少 fs_main");
    let tail = &SHADER[fs_main..];
    let next_at = tail[1..].find("\n@").map(|i| i + 1).unwrap_or(tail.len());
    assert!(
        tail[..next_at].contains("local_lighting("),
        "fs_main 必须把局部光源项累加进出射光"
    );
}

/// 类型码判定阈值：wgsl 用 `outer_kind.y > 0.5` 区分聚光，必须与 `gpu_code()` 的 0/1 配套。
#[test]
fn spot_kind_threshold_matches_gpu_code() {
    assert!(
        SHADER.contains("light.outer_kind.y > 0.5"),
        "聚光判定阈值被改了，必须与 LocalLightKind::gpu_code 的 0/1 同步"
    );
    assert_eq!(r3e_core::LocalLightKind::Point.gpu_code(), 0.0);
    assert_eq!(r3e_core::LocalLightKind::Spot.gpu_code(), 1.0);
}
