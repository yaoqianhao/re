//! 屏幕空间遮蔽的预算与统计契约（不需要 GPU）。
//!
//! `planned_passes` 是探针「预期 pass 数」的唯一来源：它一旦与 `render` 里
//! 实际提交的数量对不上，探针就会一直在报警与不报警之间摆动。

use r3e_core::ssao::SsaoSettings;
use r3e_render::ssao::{SsaoPass, SsaoStats, AO_PASSES_PER_FRAME};

/// 缺省设置是「关」：老场景一帧都不多跑，画面与扩展十六一致。
#[test]
fn neutral_settings_run_nothing() {
    let neutral = SsaoSettings::default();
    assert!(neutral.is_neutral());
    assert_eq!(SsaoPass::planned_passes(&neutral), 0);
}

/// 打开就是两个 pass（生成 + 散焦），与 `AO_PASSES_PER_FRAME` 同源。
#[test]
fn enabled_settings_run_two_passes() {
    let on = SsaoSettings {
        enabled: true,
        ..Default::default()
    };
    assert!(!on.is_neutral());
    assert_eq!(SsaoPass::planned_passes(&on), AO_PASSES_PER_FRAME);
    assert_eq!(SsaoPass::planned_passes(&on), 2);
}

/// 强度为 0 时等价于关闭：乘 0 会把整屏压黑，这里按「中性」处理。
#[test]
fn zero_intensity_is_neutral() {
    let off = SsaoSettings {
        enabled: true,
        intensity: 0.0,
        ..Default::default()
    };
    assert!(off.sanitized().is_neutral());
    assert_eq!(SsaoPass::planned_passes(&off), 0);
}

/// 统计默认值：没渲染过时 pass 数为 0，探针据此区分「没跑」与「跑了但统计没更新」。
#[test]
fn stats_default_is_idle() {
    let stats = SsaoStats::default();
    assert!(!stats.enabled);
    assert_eq!(stats.passes, 0);
    assert_eq!(stats.samples, 0);
    assert_eq!(stats.size, (0, 0));
}
