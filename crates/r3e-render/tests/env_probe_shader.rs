//! `shader.wgsl` 里 HDR 立方体反射探针（扩展二十三）的静态校验：不开窗口、不建 GPU。
//!
//! 探针横跨四处：`r3e_core::envmap` 的解码 / 预滤波、`envprobe.rs` 的 f16 编码与 mip 映射、
//! `renderer.rs` 的纹理与绑定布局、`shader.wgsl` 的镜面采样分支。任何一处单边改动都会
//! 静默失效（层号错位、绑定维度不对、开关读错分量），这里用文本 + naga 断言把它们钉死：
//!
//! * 未配探针时 `env_probe.x = 0`，片元着色器一次都不采样 `t_env_cube` ——
//!   这是「画面与扩展十二逐像素一致」的前提，因此分支条件与兜底绑定都要断言。
//! * CPU 的 `mip_lod` 与着色器的 `lod` 必须是同一条式子，否则统计与实际采样各算各的。

use r3e_render::{
    encode_faces, mip_lod, EnvProbeSource, EnvProbeStats, DUMMY_CUBE_FACE_SIZE, ENV_CUBE_BINDING,
};

const SHADER: &str = include_str!("../src/shader.wgsl");
const RENDERER: &str = include_str!("../src/renderer.rs");

fn parsed_module() -> naga::Module {
    match naga::front::wgsl::parse_str(SHADER) {
        Ok(module) => module,
        Err(e) => panic!("shader.wgsl 解析失败：\n{}", e.emit_to_string(SHADER)),
    }
}

/// 按字符数从 `at` 截一段窗口：源文件全是中文注释，按字节切会踩多字节边界。
fn window(src: &str, at: usize, max_chars: usize) -> &str {
    let end = src[at..]
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| at + i)
        .unwrap_or(src.len());
    &src[at..end]
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

/// 探针立方体必须挂在 `group(1) @binding(6)`：WebGPU core 的 `max_bind_groups` 是 4，
/// `0..=3` 已被相机 / 材质 / 关节 / 阴影占满，另开一组会让 `create_shader_module` 直接报错。
#[test]
fn cube_binding_is_group1_binding6() {
    let head = format!("@group(1) @binding({ENV_CUBE_BINDING})");
    let idx = SHADER
        .find(&head)
        .unwrap_or_else(|| panic!("wgsl 缺少 {head}"));
    let tail = window(SHADER, idx, 240);
    assert!(
        tail.contains("var t_env_cube: texture_cube<f32>;"),
        "binding({ENV_CUBE_BINDING}) 必须声明为 texture_cube<f32>"
    );
    assert!(
        !SHADER.contains("@group(4)"),
        "wgsl 中不允许出现 @group(4)，WebGPU core 上限是 4 个组（0..=3）"
    );
    assert_eq!(
        ENV_CUBE_BINDING, 6,
        "binding 编号改动必须同步 ARCHITECTURE 的绑定表"
    );
}

/// 绑定组的布局项必须与 wgsl 声明一致：立方体 + 可过滤浮点，否则创建管线时就校验失败。
#[test]
fn layout_entry_matches_shader_declaration() {
    let idx = RENDERER
        .match_indices("binding: 6,")
        .map(|(i, _)| i)
        .find(|i| window(RENDERER, *i, 500).contains("TextureViewDimension::Cube"))
        .expect("renderer 缺少 binding 6（立方体）的布局项");
    let body = window(RENDERER, idx, 500);
    assert!(
        body.contains("wgpu::TextureViewDimension::Cube"),
        "binding 6 的视图维度必须是 Cube"
    );
    assert!(
        body.contains("wgpu::TextureSampleType::Float") && body.contains("filterable: true"),
        "Rgba16Float 探针必须声明为可过滤浮点采样类型"
    );
    // 采样器复用材质组里的 s_base_color（ClampToEdge + Linear），不额外加 binding
    assert!(
        SHADER.contains("textureSampleLevel(t_env_cube, s_base_color"),
        "探针采样必须复用 s_base_color 采样器，避免再加一个 sampler binding"
    );
}

/// `Camera` 结构体里 `env_probe` 必须紧跟 `env_sh`：字段错位不报错，只会把开关读成强度。
#[test]
fn camera_struct_field_order_matches_rust() {
    let fields = [
        "view_proj",
        "camera_pos",
        "env_zenith",
        "env_sh",
        "env_probe",
    ];
    let start = SHADER
        .find("struct Camera {")
        .expect("wgsl 缺少 struct Camera");
    let end = start + SHADER[start..].find('}').expect("struct Camera 未闭合");
    let body = &SHADER[start..end];
    let mut cursor = 0usize;
    for field in fields {
        let found = body[cursor..]
            .find(&format!("{field}:"))
            .unwrap_or_else(|| panic!("struct Camera 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
    // Rust 侧同名同序（CameraUniform 的字段表）
    let struct_start = RENDERER
        .find("struct CameraUniform {")
        .expect("renderer 缺少 CameraUniform");
    let struct_end = struct_start + RENDERER[struct_start..].find("\n}").expect("结构未闭合");
    let rust_body = &RENDERER[struct_start..struct_end];
    let mut cursor = 0usize;
    for field in fields {
        let found = rust_body[cursor..]
            .find(&format!("\n    {field}:"))
            .unwrap_or_else(|| panic!("CameraUniform 里找不到 {field}（顺序可能错了）"));
        cursor += found + field.len();
    }
}

/// 未配探针必须**不采样**立方体：分支条件是 uniform 常量，关闭时镜面仍走解析天空。
#[test]
fn specular_branch_falls_back_to_analytic_sky() {
    for f in ["fn probe_specular(", "fn env_radiance("] {
        assert!(SHADER.contains(f), "wgsl 缺少 {f}");
    }
    let start = SHADER.find("fn fs_main(").expect("wgsl 缺少 fs_main");
    let end = start + SHADER[start..].find("\n}").expect("fs_main 未闭合");
    let body = &SHADER[start..end];
    assert!(
        body.contains("if u_camera.env_probe.x > 0.5 {"),
        "镜面分支必须由 env_probe.x 控制"
    );
    assert!(
        body.contains("env_radiance(dominant, roughness)")
            && body.contains("probe_specular(reflect_dir, roughness)"),
        "未激活时走 env_radiance、激活时走 probe_specular"
    );
    // 默认值：解析天空在前，探针赋值在后（赋值顺序反了等于探针永远不生效）
    let default_at = body.find("env_radiance(dominant, roughness)").unwrap();
    let branch_at = body.find("probe_specular(reflect_dir, roughness)").unwrap();
    assert!(
        default_at < branch_at,
        "env_radiance 必须是默认值，探针只能覆盖它"
    );
}

/// 着色器里的 lod 式子必须与 CPU 的 `mip_lod` 同源（粗糙度 × 层数-1，两端夹取）。
#[test]
fn shader_lod_matches_cpu_mapping() {
    let start = SHADER
        .find("fn probe_specular(")
        .expect("wgsl 缺少 probe_specular");
    let end = start + SHADER[start..].find("\n}").expect("函数未闭合");
    let body = &SHADER[start..end];
    assert!(
        body.contains("clamp(roughness, 0.0, 1.0) * (levels - 1.0)"),
        "lod 必须是 clamp(roughness,0,1) × (层数-1)，与 envprobe::mip_lod 逐位一致"
    );
    assert!(
        body.contains("u_camera.env_probe.y"),
        "层数必须取自 env_probe.y"
    );
    assert!(
        body.contains("u_camera.env_probe.z"),
        "强度必须取自 env_probe.z，且只乘在镜面项"
    );
    // CPU 侧走同一条映射：两端点必须严丝合缝
    assert_eq!(mip_lod(0.0, 5), 0.0);
    assert_eq!(mip_lod(1.0, 5), 4.0);
    assert_eq!(mip_lod(0.5, 5), 2.0);
    assert_eq!(mip_lod(f32::NAN, 5), 4.0, "NaN 粗糙度夹到最糊一层");
    assert_eq!(mip_lod(0.7, 1), 0.0, "只有一层时恒取 0");
}

/// 兜底纹理：没有探针时绑 1×1×6 全黑立方体，且与真探针同格式（否则切场景要改布局）。
#[test]
fn fallback_cube_is_one_by_one_hdr() {
    let idx = RENDERER
        .find("fn create_black_cube_texture(")
        .expect("renderer 缺少兜底立方体");
    let body = window(RENDERER, idx, 2000);
    assert!(
        body.contains("depth_or_array_layers: 6"),
        "兜底图必须补齐 6 个面"
    );
    assert!(
        body.contains("wgpu::TextureFormat::Rgba16Float"),
        "兜底图必须与真探针同格式 Rgba16Float"
    );
    assert!(
        body.contains("wgpu::TextureViewDimension::Cube"),
        "兜底图必须建成立方体视图"
    );
    assert_eq!(DUMMY_CUBE_FACE_SIZE, 1, "兜底面边长就是 1 像素");
    // 换图 / 卸图都要让材质缓存失效（bind group 里写的是视图本身）
    let set_idx = RENDERER
        .find("fn set_env_cube(")
        .expect("renderer 缺少 set_env_cube");
    assert!(
        window(RENDERER, set_idx, 400).contains("self.materials.clear()"),
        "换探针必须清空材质缓存，否则旧 bind group 还指着旧视图"
    );
    let reset_idx = RENDERER
        .find("fn reset_env_cube(")
        .expect("renderer 缺少 reset_env_cube");
    assert!(
        window(RENDERER, reset_idx, 400).contains("self.materials.clear()"),
        "卸探针同样要清空材质缓存"
    );
}

/// 统计与日志：`RenderStats::env_probe` 必须存在，且缺省即「没配探针」。
#[test]
fn stats_default_is_idle() {
    let stats = EnvProbeStats::default();
    assert!(!stats.active);
    assert!(stats.is_idle());
    assert_eq!(stats.source, "none");
    assert_eq!(stats.face_size, 0);
    assert_eq!(stats.mip_levels, 0);
    assert!(
        RENDERER.contains("pub env_probe: EnvProbeStats"),
        "RenderStats 必须带出探针状态"
    );
    assert!(
        RENDERER.contains("env_probe: self.env_probe_stats()"),
        "每帧统计必须从渲染器当前探针取"
    );
}

/// 上传清单与场景段配置的联动：来源标签、面边长、层数都要如实进统计。
#[test]
fn upload_plan_matches_settings() {
    let settings = r3e_core::EnvironmentMapSettings::with_equirect("sky.hdr").sanitized();
    let image =
        r3e_core::envmap::HdrImage::constant(64, 32, [0.25, 0.5, 1.0]).expect("纯色环境图应能构造");
    let prefiltered =
        r3e_core::envmap::build_probe(&image, settings.face_size, settings.mip_levels);
    let source = EnvProbeSource::from_settings(prefiltered.clone(), &settings);
    let stats = source.stats();
    assert!(stats.active);
    assert_eq!(stats.source, settings.source_label());
    assert_eq!(stats.face_size, prefiltered.base().face_size() as u32);
    assert_eq!(stats.mip_levels, prefiltered.mip_levels());

    let uploads = encode_faces(&prefiltered);
    assert_eq!(
        uploads.len(),
        prefiltered.mip_levels() as usize * r3e_render::CUBE_FACES,
        "每层每面都要有一条上传记录"
    );
    for upload in &uploads {
        assert_eq!(upload.bytes.len(), (upload.size * upload.size * 8) as usize);
        assert_eq!(
            upload.bytes_per_row(),
            upload.size * 8,
            "行距不得有 padding"
        );
    }
}
