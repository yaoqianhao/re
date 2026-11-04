//! 扩展十八 C：局部光源阴影的 **CPU 侧打包契约**（不开窗口、不建 GPU）。
//!
//! 这一层最容易「改一处、静默错一片」的地方有三个，各用断言钉死：
//!
//! * uniform 的字段顺序 / 总字节数必须与 wgsl 的 `struct LocalShadow` 一致；
//! * 每盏投影灯占 6 层（聚光只用 1 层），槽位 `layer_base = slot * POINT_SHADOW_FACES`，
//!   非投影灯必须是 -1（着色器靠它早退）；
//! * 上传的矩阵必须**逐位等于** `LocalLight::shadow_face_view_projection`——深度 pass 与
//!   主通道共用同一份，一旦这里另算一遍就会出现半像素级的读写错位（表现为阴影边缘发虚）。

use r3e_core::{
    point_shadow_face, LocalLight, LocalLightKind, MAX_LOCAL_LIGHTS, MAX_LOCAL_SHADOW_CASTERS,
    MAX_LOCAL_SHADOW_LAYERS, POINT_SHADOW_FACES,
};
use r3e_render::renderer::LocalShadowUniform;
use r3e_render::{LOCAL_SHADOW_NORMAL_BIAS, LOCAL_SHADOW_UNIFORM_SIZE};

fn point_at(key: &str, pos: [f32; 3], intensity: f32, range: f32) -> LocalLight {
    LocalLight {
        intensity,
        range,
        cast_shadow: true,
        ..LocalLight::point(key, pos, [1.0, 1.0, 1.0], intensity)
    }
}

/// 2 个 vec4 + 24 个 mat4 + 16 个 vec4，全部按 wgsl std140 的 16 字节网格排布。
#[test]
fn uniform_size_matches_wgsl_layout() {
    let expected = 2 * 16 + (MAX_LOCAL_SHADOW_LAYERS as u64) * 64 + (MAX_LOCAL_LIGHTS as u64) * 16;
    assert_eq!(LOCAL_SHADOW_UNIFORM_SIZE, expected);
    assert_eq!(
        MAX_LOCAL_SHADOW_LAYERS, 24,
        "4 盏 × 6 面，改这里必须改 wgsl"
    );
    assert_eq!(MAX_LOCAL_LIGHTS, 16);
    assert_eq!(POINT_SHADOW_FACES, 6);
}

/// 默认（关灯 / 未投影）必须是「零开销 + 零影响」：灯数 0、层数 0、所有槽位 -1。
#[test]
fn disabled_uniform_requests_no_pass() {
    let lights = vec![point_at("a", [0.0, 2.0, 0.0], 5.0, 10.0)];
    for uniform in [
        LocalShadowUniform::default(),
        LocalShadowUniform::from_lights(&lights, false),
    ] {
        assert_eq!(uniform.params[0], 0.0, "关闭时投影灯数必须是 0");
        assert_eq!(uniform.params[2], 0.0, "关闭时启用标记必须是 0");
        assert!(
            uniform.slots.iter().all(|s| s[0] < 0.0),
            "关闭时槽位必须全 -1"
        );
        assert!(uniform.layers().is_empty(), "关闭时不应提交任何深度 pass");
    }
    // 开灯但一盏都没标 cast_shadow：同样零 pass（老场景的默认路径）
    let plain = vec![LocalLight::point("p", [0.0, 2.0, 0.0], [1.0; 3], 3.0)];
    let uniform = LocalShadowUniform::from_lights(&plain, true);
    assert_eq!(uniform.params[0], 0.0);
    assert!(uniform.layers().is_empty());
}

/// 点光占满 6 层，聚光只占 1 层；槽位按 `light_index` 直接索引（着色器据此回查光源）。
#[test]
fn point_takes_six_layers_spot_takes_one() {
    let mut spot = LocalLight::spot(
        "s",
        [1.0, 3.0, 0.0],
        [0.0, -1.0, 0.0],
        [1.0; 3],
        4.0,
        12.0,
        20.0,
        35.0,
    );
    spot.cast_shadow = true;
    let lights = vec![point_at("p", [0.0, 2.0, 0.0], 8.0, 10.0), spot];

    let uniform = LocalShadowUniform::from_lights(&lights, true);
    assert_eq!(uniform.params[0], 2.0, "两盏灯都投影");
    assert_eq!(uniform.slots[0][1], 6.0, "点光 6 面");
    assert_eq!(uniform.slots[1][1], 1.0, "聚光 1 面");
    // 排序按「辐亮度 × range²」，8.0 那盏更亮 → slot 0，聚光排到 slot 1
    let point_base = uniform.slots[0][0];
    let spot_base = uniform.slots[1][0];
    assert_eq!(point_base, 0.0);
    assert_eq!(spot_base, 6.0, "第二盏灯接在点光占用的 6 层之后");
    assert_eq!(
        uniform.layers(),
        vec![0, 1, 2, 3, 4, 5, 6],
        "点光 6 层 + 聚光 1 层，且按升序提交"
    );
    assert_eq!(uniform.params2[1], 7.0, "本帧占用 7 层");
    assert_eq!(uniform.params2[0], 1.0, "阴影强度默认 1（可直接调暗）");
    // 该灯的深度偏移与法线偏移逐灯存放
    assert!((uniform.slots[0][2] - lights[0].clone().sanitized().shadow_bias).abs() < 1e-6);
    assert!(
        uniform.slots[0][3] > 0.0,
        "法线偏移必须是正的（沿法线推离表面）"
    );
}

/// 未标 `cast_shadow` 的灯必须保持 -1，且不占用任何层。
#[test]
fn non_cast_lights_keep_negative_slot() {
    let lights = vec![
        point_at("caster", [0.0, 2.0, 0.0], 9.0, 10.0),
        LocalLight::point("quiet", [4.0, 2.0, 0.0], [1.0; 3], 1.0),
    ];
    let uniform = LocalShadowUniform::from_lights(&lights, true);
    assert_eq!(uniform.slots[0][0], 0.0);
    assert!(uniform.slots[1][0] < 0.0, "不投影的灯槽位必须留在 -1");
    assert_eq!(uniform.layers(), vec![0, 1, 2, 3, 4, 5]);
}

/// 超出 `MAX_LOCAL_SHADOW_CASTERS` 的灯被截掉：层号绝不允许爬到数组外。
#[test]
fn caster_count_is_capped() {
    let lights: Vec<LocalLight> = (0..MAX_LOCAL_SHADOW_CASTERS + 2)
        .map(|i| {
            point_at(
                &format!("l{i}"),
                [i as f32, 2.0, 0.0],
                5.0 - i as f32 * 0.1,
                8.0,
            )
        })
        .collect();
    let uniform = LocalShadowUniform::from_lights(&lights, true);
    assert_eq!(uniform.params[0], MAX_LOCAL_SHADOW_CASTERS as f32);
    let layers = uniform.layers();
    assert_eq!(layers.len(), MAX_LOCAL_SHADOW_CASTERS * POINT_SHADOW_FACES);
    assert!(
        layers
            .iter()
            .all(|l| (*l as usize) < MAX_LOCAL_SHADOW_LAYERS),
        "层号越界会让 wgsl 读到垃圾矩阵"
    );
    assert_eq!(uniform.params2[1], MAX_LOCAL_SHADOW_LAYERS as f32);
}

/// 点光的六个面必须落到六个不同的矩阵（否则「立方体阴影」退化成单向阴影）。
#[test]
fn point_faces_are_distinct_and_match_core() {
    let lights = vec![point_at("p", [3.0, 4.0, -2.0], 6.0, 20.0)];
    let uniform = LocalShadowUniform::from_lights(&lights, true);
    let light = lights[0].clone().sanitized();
    for face in 0..POINT_SHADOW_FACES {
        let expected = light.shadow_face_view_projection(face).to_cols_array_2d();
        assert_eq!(
            uniform.view_proj[face], expected,
            "第 {face} 层矩阵必须与 core 算的完全一致（深度 pass 与主通道共用）"
        );
    }
    for a in 0..POINT_SHADOW_FACES {
        for b in (a + 1)..POINT_SHADOW_FACES {
            assert_ne!(
                uniform.view_proj[a], uniform.view_proj[b],
                "第 {a} 面与第 {b} 面矩阵相同，说明选面逻辑退化了"
            );
        }
    }
    // 未占用的层保持全 0（不是单位阵）——着色器只会读 slots 覆盖到的层
    assert_eq!(uniform.view_proj[POINT_SHADOW_FACES], [[0.0; 4]; 4]);
}

/// 法线偏移 = 系数 × 单纹素世界尺寸，因此 range 越大偏移越大；深度偏移夹在 [0, 0.05]。
#[test]
fn normal_bias_scales_with_light_range() {
    let near_lights = vec![point_at("small", [0.0, 2.0, 0.0], 5.0, 2.0)];
    let far_lights = vec![point_at("big", [0.0, 2.0, 0.0], 5.0, 60.0)];
    let near_u = LocalShadowUniform::from_lights(&near_lights, true);
    let far_u = LocalShadowUniform::from_lights(&far_lights, true);
    assert!(
        far_u.slots[0][3] > near_u.slots[0][3],
        "远灯的纹素更大，法线偏移也必须更大"
    );
    assert!(near_u.slots[0][3] > 0.0);

    let mut dirty = point_at("dirty", [0.0, 2.0, 0.0], 5.0, 10.0);
    dirty.shadow_bias = 9.0;
    let uniform = LocalShadowUniform::from_lights(&[dirty], true);
    assert!(
        (0.0..=r3e_core::MAX_LOCAL_SHADOW_BIAS).contains(&uniform.slots[0][2]),
        "深度偏移必须夹进 [0, MAX_LOCAL_SHADOW_BIAS]，否则整盏灯的阴影会消失"
    );
    // 系数是常量，这里只做「配置未被人为清零」的守门（clippy 建议写成 const 块）
    const { assert!(LOCAL_SHADOW_NORMAL_BIAS > 0.0) };
}

/// 着色器选面逻辑与 `r3e_core::point_shadow_face` 必须同一套（三处同源中的第一处）。
#[test]
fn face_selection_matches_core_order() {
    use glam::Vec3;
    // 主轴方向 → 期望层偏移，顺序 +X, -X, +Y, -Y, +Z, -Z
    let dirs = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    for (expected, dir) in dirs.iter().enumerate() {
        assert_eq!(point_shadow_face(*dir), expected);
    }
    // 立方体的 8 个角附近也必须落到「绝对值最大的那根轴」上
    let corner = Vec3::new(1.0, -0.5, 0.2);
    assert_eq!(point_shadow_face(corner), 0);
    assert_eq!(point_shadow_face(-corner), 1);
    // 聚光方向退化时不能 panic（dir 全 0 走 NEG_Y 兜底）
    let degenerate = LocalLight {
        kind: LocalLightKind::Spot,
        dir: [0.0, 0.0, 0.0],
        cast_shadow: true,
        ..LocalLight::default()
    };
    let _ = degenerate.shadow_face_view_projection(0);
}
