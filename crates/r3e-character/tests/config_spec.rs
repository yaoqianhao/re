//! 参数校验与尺寸换算（3 条）。

use glam::Vec3;
use r3e_character::{CharacterConfig, ConfigError};

#[test]
fn default_config_is_valid() {
    let cfg = CharacterConfig::default();
    assert_eq!(cfg.validate(), Ok(()));
    // 1.8 高、0.35 半径 → 圆柱段半高 = 0.9 - 0.35 = 0.55。
    assert!((cfg.half_height() - 0.55).abs() < 1e-6);
}

#[test]
fn rejects_impossible_parameters() {
    let cfg = CharacterConfig {
        radius: 1.0, // height 1.8 装不下两个 1.0 的半球
        ..CharacterConfig::default()
    };
    assert_eq!(
        cfg.validate(),
        Err(ConfigError::HeightTooSmall {
            height: 1.8,
            min: 2.0
        })
    );

    let cfg = CharacterConfig {
        gravity: 9.8, // 向上「重力」
        ..CharacterConfig::default()
    };
    assert!(matches!(
        cfg.validate(),
        Err(ConfigError::GravityNotDownward { .. })
    ));

    let cfg = CharacterConfig {
        max_slope_deg: 100.0,
        ..CharacterConfig::default()
    };
    assert!(matches!(
        cfg.validate(),
        Err(ConfigError::SlopeOutOfRange { .. })
    ));

    let cfg = CharacterConfig {
        snap_distance: f32::NAN,
        ..CharacterConfig::default()
    };
    assert!(matches!(
        cfg.validate(),
        Err(ConfigError::NonFinite {
            field: "snap_distance"
        })
    ));

    // 台阶与吸附允许为 0（= 关闭该行为），只拦负数。
    let cfg = CharacterConfig {
        step_height: 0.0,
        snap_distance: 0.0,
        ..CharacterConfig::default()
    };
    assert_eq!(cfg.validate(), Ok(()));
}

#[test]
fn center_feet_roundtrip_and_slope_threshold() {
    let cfg = CharacterConfig::default();
    let feet = Vec3::new(1.0, 2.0, 3.0);
    let center = cfg.center(feet);
    assert!((center.y - 2.9).abs() < 1e-6, "中心 = 脚底 + height/2");
    let back = cfg.feet(center);
    assert!((back - feet).length() < 1e-6);

    // cos(45°) ≈ 0.7071：恰好 45° 算站得住，更陡站不住。
    let limit = cfg.min_up_dot();
    assert!((limit - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!(cfg.is_walkable(Vec3::new(limit.sin(), limit, 0.0)));
    assert!(!cfg.is_walkable(Vec3::new(0.9, 0.3, 0.0)));
    // 零法线是「没查到地面」的哨兵值，不是垂直墙。
    assert!(!cfg.is_walkable(Vec3::ZERO));
}
