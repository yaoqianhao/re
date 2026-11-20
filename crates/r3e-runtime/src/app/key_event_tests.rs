//! `app` 键位映射测试（M2-A05 自 `app.rs` 内嵌测试块切出，逐条搬移、零删减）。

use super::*;

/// 字符键走逻辑键：AZERTY 上按 "a" 报 `A`，与物理位置无关。
#[test]
fn character_keys_follow_logical_layout() {
    let key = Key::Character("a".into());
    // 物理位置是 Q 的键（AZERTY 的 A），逻辑键仍是 a
    assert_eq!(
        EngineApp::key_code_of(&key, PhysicalKey::Code(WinitKeyCode::KeyQ)),
        Some(KeyCode::A)
    );
    let upper = Key::Character("W".into());
    assert_eq!(
        EngineApp::key_code_of(&upper, PhysicalKey::Code(WinitKeyCode::KeyW)),
        Some(KeyCode::W),
        "大写字母应归到同一键位"
    );
}

/// 物理键补足逻辑键给不出的信息：左右修饰键分别映射。
#[test]
fn modifier_side_comes_from_physical_key() {
    let shift = Key::Named(NamedKey::Shift);
    assert_eq!(
        EngineApp::key_code_of(&shift, PhysicalKey::Code(WinitKeyCode::ShiftRight)),
        Some(KeyCode::ShiftRight)
    );
    assert_eq!(
        EngineApp::key_code_of(&shift, PhysicalKey::Code(WinitKeyCode::ShiftLeft)),
        Some(KeyCode::ShiftLeft)
    );
    // 逻辑键单独解析时退回左修饰键，与脚本别名 `shift` 一致
    assert_eq!(
        EngineApp::logical_key_code(&shift),
        Some(KeyCode::ShiftLeft)
    );
    assert_eq!(
        EngineApp::key_code_of(
            &Key::Named(NamedKey::Control),
            PhysicalKey::Code(WinitKeyCode::ControlRight)
        ),
        Some(KeyCode::ControlRight)
    );
}

/// 小键盘数字与主键区数字分开统计。
#[test]
fn numpad_digits_stay_distinct() {
    let one = Key::Character("1".into());
    assert_eq!(
        EngineApp::key_code_of(&one, PhysicalKey::Code(WinitKeyCode::Numpad1)),
        Some(KeyCode::Numpad1)
    );
    assert_eq!(
        EngineApp::key_code_of(&one, PhysicalKey::Code(WinitKeyCode::Digit1)),
        Some(KeyCode::Digit1)
    );
}

/// 功能键 / 方向键 / 未知键。
#[test]
fn named_and_unknown_keys() {
    assert_eq!(
        EngineApp::key_code_of(
            &Key::Named(NamedKey::F5),
            PhysicalKey::Code(WinitKeyCode::F5)
        ),
        Some(KeyCode::F5)
    );
    assert_eq!(
        EngineApp::key_code_of(
            &Key::Named(NamedKey::ArrowLeft),
            PhysicalKey::Code(WinitKeyCode::ArrowLeft)
        ),
        Some(KeyCode::ArrowLeft)
    );
    // 引擎不关心的键不写入输入层
    assert_eq!(
        EngineApp::key_code_of(
            &Key::Named(NamedKey::Help),
            PhysicalKey::Code(WinitKeyCode::Help)
        ),
        None
    );
}

/// 相机移动判据：任意一个方向键按下即应参与计算。
#[test]
fn move_keys_detect_any_direction() {
    assert!(MoveKeys::default().is_empty());
    assert!(!MoveKeys {
        up: true,
        ..Default::default()
    }
    .is_empty());
}
