//! 局部光源表（扩展十五）的**字节布局契约**。
//!
//! 不开窗口、不建 GPU，纯 CPU 侧把「Rust 结构体 → uniform 字节」这条链路钉死。
//! 这里故意比对着 wgsl 的 `struct LocalLight / struct Lights` 写断言：
//! 一旦有人改了打包顺序或忘了同步 `MAX_LOCAL_LIGHTS`，GPU 侧不会报错，
//! 只会把「强度」读成「半径」这类静默错误——所以尺寸与字段偏移必须由测试兜住。

use bytemuck::bytes_of;
use r3e_core::{LocalLight, LocalLightKind, MAX_LOCAL_LIGHTS};
use r3e_render::{LightsUniform, LocalLightRaw, LIGHTS_UNIFORM_SIZE, LOCAL_LIGHT_STRIDE};

#[test]
fn local_light_stride_is_four_vec4() {
    assert_eq!(
        std::mem::size_of::<LocalLightRaw>(),
        64,
        "单盏灯必须是 4 个 vec4 = 64 字节"
    );
    assert_eq!(LOCAL_LIGHT_STRIDE, 64);
}

#[test]
fn lights_uniform_is_count_plus_16_lights() {
    // 1 个计数 vec4（16B）+ 16 盏 × 64B = 1040B，结构体按 16 字节对齐无尾部填充
    assert_eq!(
        std::mem::size_of::<LightsUniform>(),
        16 + 64 * MAX_LOCAL_LIGHTS
    );
    assert_eq!(LIGHTS_UNIFORM_SIZE, 16 + 64 * 16);
    assert_eq!(MAX_LOCAL_LIGHTS, 16);
}

#[test]
fn lights_uniform_field_offsets() {
    // wgsl 里 `u_lights.items[i]` 的地址 = 16 + i*64，靠这个偏移算出来的；
    // 用真实指针相减验证，比手写 size_of 更贴 struct 的实际排布。
    let u = LightsUniform::default();
    let base = &u as *const _ as usize;
    let count_off = &u.count as *const _ as usize - base;
    let items_off = &u.items[0] as *const _ as usize - base;
    let item1_off = &u.items[1] as *const _ as usize - base;
    assert_eq!(count_off, 0, "count 必须在偏移 0");
    assert_eq!(items_off, 16, "items 必须紧跟 count，从偏移 16 开始");
    assert_eq!(item1_off, 80, "相邻两盏灯间隔 64 字节");
}

#[test]
fn disabled_writes_zero_count_and_keeps_data_out() {
    let lights = vec![LocalLight::point(
        "p",
        [1.0, 2.0, 3.0],
        [1.0, 1.0, 1.0],
        5.0,
    )];
    let off = LightsUniform::from_lights(&lights, false);
    assert_eq!(
        off.count[0], 0.0,
        "总开关关闭时计数必须为 0，GPU 循环整体跳过"
    );
    assert_eq!(
        off.items[0].pos_range, [0.0; 4],
        "关闭时不应把灯数据传上去（数据留在场景里即可）"
    );
}

#[test]
fn enabled_packs_position_range_color_intensity() {
    let mut light = LocalLight::point("p", [1.5, -2.0, 3.25], [0.5, 0.75, 1.0], 8.0);
    light.range = 12.0; // point() 的第 4 参数是强度，半径要单独给
    let u = LightsUniform::from_lights(&[light], true);
    assert_eq!(u.count[0], 1.0);
    assert_eq!(u.items[0].pos_range, [1.5, -2.0, 3.25, 12.0]);
    assert_eq!(u.items[0].color_power, [0.5, 0.75, 1.0, 8.0]);
}

#[test]
fn spot_packs_cosine_and_kind_code() {
    let mut light = LocalLight::point("s", [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], 4.0);
    light.kind = LocalLightKind::Spot;
    light.dir = [0.0, -1.0, 0.0];
    light.inner_deg = 20.0;
    light.outer_deg = 35.0;
    let s = light.clone().sanitized();
    let u = LightsUniform::from_lights(&[light], true);
    let raw = u.items[0];
    assert_eq!(
        raw.outer_kind[1], 1.0,
        "聚光类型码必须是 1（wgsl 用 >0.5 判定）"
    );
    assert!(
        (raw.dir_inner[3] - s.cos_inner()).abs() < 1e-6,
        "内角余弦必须在 CPU 侧算好塞进 w"
    );
    assert!(
        (raw.outer_kind[0] - s.cos_outer()).abs() < 1e-6,
        "外角余弦塞进 outer_kind.x"
    );
    assert_eq!(raw.dir_inner[..3], [0.0, -1.0, 0.0]);
}

#[test]
fn point_kind_code_is_zero() {
    let u = LightsUniform::from_lights(&[LocalLight::point("p", [0.0; 3], [1.0; 3], 1.0)], true);
    assert_eq!(
        u.items[0].outer_kind[1], 0.0,
        "点光类型码为 0，wgsl 不加锥因子"
    );
}

#[test]
fn from_lights_clamps_to_max_and_sanitizes() {
    // 17 盏 → 只取前 16；坏值（负强度 / 零半径）必须先被 sanitized 收敛
    let mut lights: Vec<LocalLight> = (0..17)
        .map(|i| LocalLight::point(format!("l{i}"), [0.0; 3], [1.0; 3], 1.0))
        .collect();
    lights[0].intensity = -5.0;
    lights[0].range = 0.0;
    let u = LightsUniform::from_lights(&lights, true);
    assert_eq!(u.count[0], 16.0, "上界钳到 MAX_LOCAL_LIGHTS");
    assert!(u.items[0].pos_range[3] > 0.0, "零半径必须被改写成正数");
    assert!(u.items[0].color_power[3] >= 0.0, "负强度必须被夹到非负");
    // 第 17 盏不应落进 items 之外的内存
    assert_eq!(u.items.len(), MAX_LOCAL_LIGHTS);
}

#[test]
fn upload_bytes_are_zeroed_for_unused_slots() {
    let u = LightsUniform::from_lights(&[], true);
    assert_eq!(u.count[0], 0.0);
    let bytes = bytes_of(&u);
    assert_eq!(bytes.len() as u64, LIGHTS_UNIFORM_SIZE);
    assert!(
        bytes.iter().all(|b| *b == 0),
        "没有灯时整块 uniform 必须全零，避免残留上一帧数据"
    );
}
