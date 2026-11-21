//! 状态推进（2 条）：传送清速度、apply 以结果为准。

use glam::Vec3;
use r3e_character::{CharacterConfig, CharacterState};
use r3e_physics::port::MoveOutcome;

#[test]
fn teleport_resets_motion() {
    let cfg = CharacterConfig::default();
    let mut st = CharacterState::new(Vec3::ZERO);
    st.velocity_y = -12.0;
    st.grounded = true;
    st.ground_normal = Vec3::Y;

    st.teleport(Vec3::new(5.0, 1.0, 0.0));
    assert_eq!(st.feet, Vec3::new(5.0, 1.0, 0.0));
    assert_eq!(st.velocity_y, 0.0, "传送不该带着旧下坠速度");
    assert!(!st.grounded, "传送后必须重新踩一次地");
    assert_eq!(st.ground_normal, Vec3::ZERO);
    // 脚底 → 中心换算与配置一致。
    assert!((st.center(&cfg).y - 1.9).abs() < 1e-6);
}

#[test]
fn apply_tracks_feet_from_outcome() {
    let cfg = CharacterConfig::default();
    let mut st = CharacterState::new(Vec3::ZERO);
    // 结果报的是胶囊中心；apply 必须换算回脚底。
    let outcome = MoveOutcome {
        position: Vec3::new(0.0, 0.9, 0.0),
        grounded: true,
        ground_normal: Vec3::Y,
        blocked: true,
        step_up: 0.3,
    };
    st.apply(&cfg, &outcome);

    assert!((st.feet.y - 0.0).abs() < 1e-6, "中心 0.9 → 脚底 0.0");
    assert!(st.grounded && st.blocked);
    assert!((st.step_up - 0.3).abs() < 1e-6);
    // apply 不碰速度：那是求解器的职责。
    assert_eq!(st.velocity_y, 0.0);
}
