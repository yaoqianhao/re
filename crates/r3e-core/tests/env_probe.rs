//! 环境探针（IBL）的公开 API 端到端检查。
//!
//! 模块内的单测覆盖数学细节，这里只验证「拿引擎对外暴露的类型和函数，
//! 能拼出一条完整的漫反射 + 高光环境光照链路，且结果落在物理合理区间」。
//! 不涉及任何图形 API，`cargo test` 即可全绿。

use r3e_core::{
    env_brdf_approx, sky_radiance, sky_radiance_blurred, specular_dominant_dir,
    EnvironmentSettings, Sh9, Vec3, DEFAULT_ROUGHNESS, MAX_ENV_RADIANCE,
};

fn constant_sky(level: f32) -> EnvironmentSettings {
    EnvironmentSettings {
        sky_zenith: [level; 3],
        sky_horizon: [level; 3],
        ground: [level; 3],
        sun_intensity: 0.0,
        ..EnvironmentSettings::default()
    }
}

/// 采样数从 512 提到 4096，投影结果应基本不变（Fibonacci 采样的均匀性够用）。
#[test]
fn sh_projection_is_sample_count_stable() {
    let s = EnvironmentSettings::default();
    let coarse = Sh9::project_with_samples(&s, 512);
    let fine = Sh9::project_with_samples(&s, 4096);
    for i in 0..9 {
        let delta = (coarse.coeffs[i] - fine.coeffs[i]).length();
        let scale = fine.coeffs[i].length().max(1e-3);
        assert!(delta / scale < 0.02, "第 {i} 项系数随采样数漂移 {delta}");
    }
}

/// 漫反射 IBL 的辐照度在太阳所在一侧明显更高——方向感来自 l=1 项，
/// 这是 SH 环境光相对常数 ambient 的核心收益。
#[test]
fn diffuse_ibl_has_directional_bias() {
    let s = EnvironmentSettings::default();
    let probe = Sh9::project(&s);
    let sun = Vec3::from(s.sun_dir);
    let toward = probe.diffuse_radiance(sun);
    let away = probe.diffuse_radiance(-sun);
    assert!(
        toward.x > away.x * 1.1,
        "朝太阳 {toward:?} 应明显亮于背太阳 {away:?}"
    );
    // 天顶（冷色）与地面（暖色）的色偏应可分辨
    let up = probe.diffuse_radiance(Vec3::Y);
    let down = probe.diffuse_radiance(Vec3::NEG_Y);
    assert!(up.z > up.x, "朝上应偏蓝，实际 {up:?}");
    assert!(down.z <= up.z, "朝下不应比天空更蓝，实际 {down:?}");
    assert!(
        down.length() < up.length() * 0.6,
        "地面辐照度应明显弱于天空：{down:?} vs {up:?}"
    );
    // 朝下的环境几乎中性（地面是灰的），不应出现明显色偏
    assert!(
        (down.z - down.x).abs() < 0.05 * down.x.max(1e-3),
        "地面方向应接近中性，实际 {down:?}"
    );
}

/// 金属的高光环境项必须显著高于绝缘体：这正是修复「金属暗部死黑」的判据。
#[test]
fn metal_env_specular_is_lifted() {
    let s = EnvironmentSettings::default();
    let probe_irr = Sh9::project(&s);
    let n = Vec3::Y;
    let v = Vec3::new(0.0, 0.7, 0.7).normalize();
    let r = v - 2.0 * v.dot(n) * n;
    let ndv = v.dot(n).clamp(1e-4, 1.0);
    let roughness = 0.15_f32;
    let dominant = specular_dominant_dir(r, n, roughness);
    let env = sky_radiance_blurred(dominant, roughness, &s);

    let evaluate = |f0: f32| {
        let (scale, bias) = env_brdf_approx(roughness, ndv);
        env * (f0 * scale + bias)
    };
    let metal = evaluate(0.9); // 金属用 base color 当 F0
    let dielectric = evaluate(0.04);
    assert!(metal.x > 0.1, "金属高光不应接近黑色，实际 {metal:?}");
    assert!(metal.x > dielectric.x * 3.0, "金属应远亮于绝缘体");
    // 模糊环境亮度不应超过天空三色与太阳的合理上界
    let cap = MAX_ENV_RADIANCE;
    assert!(env.x <= cap && env.y <= cap && env.z <= cap);
    // 漫反射项（绝缘体）不该被 SH 截断成 0
    let diffuse = probe_irr.diffuse_radiance(n) * 0.8;
    assert!(diffuse.x > 0.0 && diffuse.is_finite());
}

/// 任意方向、任意粗糙度下，环境亮度都是有限非负值——着色器里不许出现 NaN / 负光。
#[test]
fn env_radiance_is_finite_everywhere() {
    let s = EnvironmentSettings {
        sun_dir: [1.0, 0.0, 0.0],
        intensity: 2.0,
        ..EnvironmentSettings::default()
    };
    for i in 0..128 {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / 128.0;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let dir = Vec3::new(r, y, 0.0);
        for roughness in [0.0, 0.045, 0.5, 1.0] {
            let l = sky_radiance_blurred(dir, roughness, &s);
            assert!(l.is_finite() && l.x >= 0.0 && l.y >= 0.0 && l.z >= 0.0);
        }
        let sharp = sky_radiance(dir, &s);
        assert!(sharp.is_finite());
    }
    // 退化输入：NaN 粗糙度按缺省处理，不产生 NaN
    let nan = sky_radiance_blurred(Vec3::Y, f32::NAN, &s);
    assert!(nan.is_finite());
    assert!(
        (nan - sky_radiance_blurred(Vec3::Y, DEFAULT_ROUGHNESS, &s)).length() < 1e-6,
        "NaN 粗糙度应等价于缺省粗糙度"
    );
}

/// 环境设置的 JSON 往返：只写想改的字段，其余走缺省。
#[test]
fn environment_settings_json_roundtrip() {
    let partial = r#"{"sun_intensity": 3.5, "sky_zenith": [0.1, 0.2, 0.5]}"#;
    let parsed: EnvironmentSettings = serde_json::from_str(partial).expect("反序列化");
    let sanitized = parsed.sanitized();
    assert_eq!(sanitized.sun_intensity, 3.5);
    assert_eq!(sanitized.sky_zenith, [0.1, 0.2, 0.5]);
    // 未写的字段回退缺省，而不是 0
    let default = EnvironmentSettings::default();
    assert_eq!(sanitized.sky_horizon, default.sky_horizon);
    assert_eq!(sanitized.ground, default.ground);
    assert_eq!(sanitized.intensity, default.intensity);

    let json = serde_json::to_string(&sanitized).expect("序列化");
    let back: EnvironmentSettings = serde_json::from_str(&json).expect("再反序列化");
    assert_eq!(back, sanitized);
    assert!(!sanitized.is_default());
    assert!(EnvironmentSettings::default().is_default());
}

/// 常数天空下，漫反射 IBL 收敛到解析解 albedo × L（式子里所有 π 都对消掉）。
#[test]
fn diffuse_ibl_conserves_constant_sky() {
    let level = 0.4_f32;
    let probe = Sh9::project(&constant_sky(level));
    for normal in [
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::X,
        Vec3::new(1.0, 1.0, 1.0).normalize(),
    ] {
        let diffuse = probe.diffuse_radiance(normal);
        assert!(
            (diffuse.x - level).abs() < 0.02 * level,
            "常数天空下漫反射辐亮度应回到 L={level}，实际 {diffuse:?}"
        );
    }
}
