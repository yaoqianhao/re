//! 着色器静态校验：不创建窗口、不开 GPU 也能验证 `shader.wgsl` 的语法与语义。
//!
//! 之前着色器只在实机跑 `--run-seconds` 时才被 naga 编译，写错一处至少要起一次
//! 图形上下文才能发现（无显示器环境根本跑不到）。这里直接调 wgpu 内置的 naga 做
//! `parse_str` + `validate`，把反馈周期压到毫秒级。
//!
//! 同时锁住 CPU / GPU 之间的几条数值契约，防止日后改 CPU 参考实现时单边漂移：
//! 球谐项数、Lambertian 卷积系数、太阳盘指数。

use glam::Vec3;
use r3e_core::env::{self, EnvironmentSettings, Sh9};

const SHADER: &str = include_str!("../src/shader.wgsl");

fn parsed_module() -> naga::Module {
    match naga::front::wgsl::parse_str(SHADER) {
        Ok(module) => module,
        Err(e) => panic!("shader.wgsl 解析失败：\n{}", e.emit_to_string(SHADER)),
    }
}

#[test]
fn shader_parses() {
    let module = parsed_module();
    assert!(!module.functions.is_empty(), "解析出的模块里没有任何函数");
}

#[test]
fn shader_validates() {
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
fn shader_declares_entry_points() {
    let module = parsed_module();
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    // 主通道 + 阴影通道：任何一个丢了都说明着色器被改坏
    for expected in ["vs_main", "fs_main", "vs_shadow"] {
        assert!(
            names.contains(&expected),
            "缺少入口点 {expected}，实际有 {names:?}"
        );
    }
}

/// WGSL 里的 `env_sh: array<vec4<f32>, N>` 必须和 CPU 侧的球谐项数一致。
#[test]
fn shader_sh_array_matches_cpu() {
    let expected = format!("array<vec4<f32>, {}>", env::SH_COEFFICIENTS);
    assert!(
        SHADER.contains(&expected),
        "着色器里的球谐数组声明应与 CPU 侧的 SH_COEFFICIENTS = {} 一致",
        env::SH_COEFFICIENTS
    );
}

/// 太阳盘指数与 Lambertian 三档卷积系数：CPU 改了这里必须同步改着色器。
#[test]
fn shader_constants_match_cpu_reference() {
    let sun = format!(
        "const SUN_SHAPE_EXPONENT: f32 = {}.0;",
        env::SUN_SHAPE_EXPONENT
    );
    // A0 = π, A1 = 2π/3, A2 = π/4，与 r3e-core 的私有常量同值
    let lambert = [
        "const LAMBERT_A0: f32 = 3.141592653589793;",
        "const LAMBERT_A1: f32 = 2.094395102393195;",
        "const LAMBERT_A2: f32 = 0.785398163397448;",
    ];
    assert!(SHADER.contains(&sun), "着色器缺少常量声明：{sun}");
    for needle in lambert {
        assert!(SHADER.contains(needle), "着色器缺少常量声明：{needle}");
    }
}

/// 朝上的方向：球谐漫反射不该比直接采样天空暗一个数量级，
/// 否则说明投影 / 卷积系数写反了——这条断言是「用 CPU 参考实现校验 GPU 观感」的前提。
#[test]
fn cpu_reference_matches_sh_projection() {
    let settings = EnvironmentSettings::default();
    let sh = Sh9::project(&settings);
    let up = Vec3::Y;
    let direct = env::sky_radiance(up, &settings);
    let diffuse = sh.diffuse_radiance(up);
    assert!(diffuse.x >= 0.0 && diffuse.y >= 0.0 && diffuse.z >= 0.0);
    assert!(
        diffuse.length() > direct.length() * 0.1,
        "球谐漫反射 {diffuse:?} 相对朝上天空 {direct:?} 过暗，投影可能写反"
    );
}
