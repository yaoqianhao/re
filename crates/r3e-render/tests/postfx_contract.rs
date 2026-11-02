//! 后处理链的预算与格式契约（不需要 GPU）：pass 数、HDR 格式、槽位容量。
//!
//! 这些数字错了不会编译报错，只会在实机上表现为「某个 pass 用了别人的参数」
//! 或「泛光链少叠一层」——所以单独锁一遍。

use r3e_core::postfx::{BloomSettings, PostFxSettings};
use r3e_render::postfx::{PostFxPass, BLOOM_LEVELS, HDR_FORMAT, MAX_PASSES_PER_FRAME};

/// 离屏目标必须是 16 位浮点：8 位目标会把 >1.0 的高光当场切平，
/// 泛光根本没东西可提取。
#[test]
fn hdr_target_is_half_float() {
    assert_eq!(HDR_FORMAT, wgpu::TextureFormat::Rgba16Float);
}

/// 缺省设置下只跑 composite 一个 pass —— 老场景的像素路径与扩展十二逐像素一致。
#[test]
fn neutral_settings_only_run_composite() {
    let neutral = PostFxSettings::default();
    assert!(neutral.is_neutral());
    assert_eq!(PostFxPass::planned_passes(&neutral), 1);

    // `None` 与 `default()` 等价：都是直通
    assert_eq!(PostFxPass::planned_passes(&PostFxSettings::default()), 1);
}

/// 泛光开启：亮部 1 + 降采样 2 + 上采样 2 + composite 1 = 6。
#[test]
fn bloom_settings_fill_the_budget() {
    let fx = PostFxSettings {
        bloom: BloomSettings {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(!fx.is_neutral());
    let planned = PostFxPass::planned_passes(&fx);
    assert_eq!(planned, 1 + (BLOOM_LEVELS as u32 - 1) * 2 + 1);
    assert_eq!(planned, MAX_PASSES_PER_FRAME);
}

/// 槽位总数就是最大 pass 数：多一个浪费、少一个会索引越界 panic。
#[test]
fn pass_budget_matches_bloom_levels() {
    assert_eq!(BLOOM_LEVELS, 3, "泛光层级变了要同步改文档与探针");
    assert_eq!(MAX_PASSES_PER_FRAME, 6);
}
