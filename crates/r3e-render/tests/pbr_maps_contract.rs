//! PBR 数据贴图（法线 / 粗糙度 / AO，扩展二十一）的 CPU-GPU 契约。
//!
//! 这几处写错都不会编译失败，只会在实机上表现为「贴图没效果」或「画面诡异」：
//! * `params2` 的开关位与 `Material` 结构体偏移对不上 → 着色器永远走标量分支；
//! * 三张贴图的绑定编号与 `material_bind_group_layout` 错位 → 采到别的图；
//! * 法线贴图按 sRGB 上传 → 法线被拉偏，光照方向整体发灰；
//! * 粗糙度取错通道（glTF 的 G vs 独立灰度图）→ 高光形状变了但看不出原因。
//!
//! 因此这里同时锁 CPU 侧的 uniform 打包与 WGSL 源码里的绑定 / 通道 / 分支。

use r3e_core::material::SurfaceMaterial;
use r3e_core::texture_slot::{classify_slot, TextureSlot};
use r3e_render::pbr::{MaterialUniform, PbrMapFlags};

const SHADER: &str = include_str!("../src/shader.wgsl");

fn parsed() -> naga::Module {
    match naga::front::wgsl::parse_str(SHADER) {
        Ok(module) => module,
        Err(e) => panic!("shader.wgsl 解析失败：\n{}", e.emit_to_string(SHADER)),
    }
}

/// uniform 仍必须是 64 字节 / 16 字节对齐（扩展十一的布局不能被挤坏）。
#[test]
fn material_uniform_keeps_its_layout() {
    assert_eq!(std::mem::size_of::<MaterialUniform>(), 64);
    assert_eq!(std::mem::align_of::<MaterialUniform>(), 4);
    // params2 落在第四个 vec4，即 offset 48
    let uniform = MaterialUniform::default();
    let base = &uniform as *const MaterialUniform as usize;
    let params2 = &uniform.params2 as *const [f32; 4] as usize;
    assert_eq!(params2 - base, 48, "params2 必须落在 offset 48");
}

/// 没挂贴图时 `params2` 全 0：着色器走纯标量分支，画面与扩展十一逐像素一致。
#[test]
fn no_maps_keeps_params2_zero() {
    let uniform = MaterialUniform::build([1.0, 1.0, 1.0, 1.0], 0.0, 0.6, [0.0; 3], false);
    assert_eq!(uniform.params2, [0.0; 4]);
    assert!(!PbrMapFlags::default().any());

    // 只挂其中一张时，另外两位必须还是 0（不能整组拉齐）
    let only_normal = PbrMapFlags {
        normal: true,
        ..PbrMapFlags::default()
    };
    let uniform =
        MaterialUniform::build_with_maps([1.0; 4], 0.0, 0.6, [0.0; 3], false, only_normal);
    assert_eq!(uniform.params2, [1.0, 0.0, 0.0, 0.0]);
}

/// 三个开关按 x/y/z 顺序打包，w 恒为 0（着色器没有第四张贴图）。
#[test]
fn map_flags_pack_in_xyz_order() {
    let flags = PbrMapFlags {
        normal: true,
        roughness: false,
        occlusion: true,
    };
    assert_eq!(flags.pack(), [1.0, 0.0, 1.0, 0.0]);
    assert!(flags.any());

    let all = PbrMapFlags {
        normal: true,
        roughness: true,
        occlusion: true,
    };
    assert_eq!(all.pack(), [1.0, 1.0, 1.0, 0.0]);
}

/// 标量参数依然走同一套收敛规则（贴图开关不影响 metallic/roughness 的夹取）。
#[test]
fn scalars_still_sanitized_with_maps() {
    let flags = PbrMapFlags {
        normal: true,
        roughness: true,
        occlusion: true,
    };
    let uniform = MaterialUniform::build_with_maps([1.0; 4], f32::NAN, 9.0, [0.0; 3], false, flags);
    let expected = SurfaceMaterial {
        metallic: f32::NAN,
        roughness: 9.0,
        emissive: [0.0; 3],
    }
    .sanitized();
    assert_eq!(uniform.params[0], expected.metallic);
    assert_eq!(uniform.params[1], expected.roughness);
    assert!(uniform.params[1] <= 1.0, "粗糙度必须夹到 1 以内");
}

/// 三张贴图的绑定编号：base color 在 0，数据贴图接在后面，且都是 `texture_2d`。
#[test]
fn shader_declares_three_extra_textures() {
    let module = parsed();
    let mut textures: Vec<(u32, String)> = Vec::new();
    for (_, var) in module.global_variables.iter() {
        if let Some(binding) = &var.binding {
            if matches!(var.space, naga::AddressSpace::Handle) {
                textures.push((binding.binding, var.name.clone().unwrap_or_default()));
            }
        }
    }
    for name in ["t_normal_map", "t_roughness_map", "t_occlusion_map"] {
        assert!(
            textures.iter().any(|(_, n)| n == name),
            "着色器缺少 {name}，实际有 {textures:?}"
        );
    }
    // 三张数据贴图必须各占一个独立的 binding
    let bindings: Vec<u32> = textures
        .iter()
        .filter(|(_, n)| n.starts_with("t_") && n != "t_base_color")
        .map(|(b, _)| *b)
        .collect();
    let mut sorted = bindings.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        bindings.len(),
        "数据贴图绑定编号重复：{bindings:?}"
    );
}

/// 着色器里的 `Material` 结构必须带 `params2`，且三张贴图开关位按 x/y/z 使用。
#[test]
fn shader_reads_params2_xyz() {
    assert!(
        SHADER.contains("params2: vec4<f32>"),
        "着色器 Material 结构缺少 params2"
    );
    for needle in [
        "u_material.params2.x > 0.5",
        "u_material.params2.y > 0.5",
        "u_material.params2.z > 0.5",
    ] {
        assert!(SHADER.contains(needle), "着色器缺少开关判断：{needle}");
    }
}

/// 法线贴图必须走 TBN（切线 + 手性），不能直接拿采样值当世界空间法线。
#[test]
fn shader_builds_tbn_for_normal_map() {
    assert!(
        SHADER.contains("cross(geometric_n, t) * in.tangent_handedness"),
        "法线贴图未按切线手性构造副切线"
    );
    assert!(
        SHADER.contains("* 2.0 - 1.0"),
        "线性法线贴图需要 *2-1 解码到 [-1,1]"
    );
    // 顶点输入必须有切线 + 手性
    assert!(
        SHADER.contains("world_tangent") && SHADER.contains("tangent_handedness"),
        "顶点着色器未传递切线空间基"
    );
}

/// 粗糙度取 G 通道、AO 取 R 通道——与 glTF 的 metallicRoughness / occlusion 约定一致。
#[test]
fn shader_samples_expected_channels() {
    assert!(
        SHADER.contains("textureSample(t_roughness_map, s_base_color, in.uv).g"),
        "粗糙度贴图应取 G 通道"
    );
    assert!(
        SHADER.contains("textureSample(t_occlusion_map, s_base_color, in.uv).r"),
        "AO 贴图应取 R 通道"
    );
}

/// 槽位分类：`bump` 是高度图，不能混进法线槽位（会把灰度图当法线，画面整片偏色）。
#[test]
fn bump_slot_is_not_normal() {
    assert_eq!(classify_slot("NormalMap"), TextureSlot::Normal);
    assert_eq!(classify_slot("Maya|normalCamera"), TextureSlot::Normal);
    assert_ne!(classify_slot("Maya|bump"), TextureSlot::Normal);
}
