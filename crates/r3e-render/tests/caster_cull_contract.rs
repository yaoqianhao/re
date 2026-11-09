//! 扩展二十四：级联遮挡体按级裁剪的 **渲染层接入契约**（不开窗口、不建 GPU）。
//!
//! core 侧的判定逻辑由 `shadow_cull` 的单测与 `tests/shadow_cull_cascade.rs` 钉住，
//! 这一层只钉「接得对不对」——历史教训是判定对了、接线错了，症状同样只是影子消失：
//!
//! * 外扩量必须真的进拟合（`fit_cascade_extended` + `cascade.caster_extend`）；
//! * 掩码必须**逐级**从 `plan.masks` 取，取错（比如永远取第 0 级）会让远级用近级的
//!   保留集合，影子成片缺角；
//! * 开关关闭时 `masks` 必须为空、走 `mask = None` 的全量路径，行为退回扩展二十三；
//! * 统计口径：`shadow_casters = 级数 × 遮挡体数`，`shadow_culled` 由掩码里的 false 求和。

use r3e_core::CascadeSettings;
use r3e_render::renderer::RenderStats;

fn renderer_src() -> &'static str {
    include_str!("../src/renderer.rs")
}

/// 默认值是「裁剪开、外扩 0」：裁剪保守所以画面逐像素不变，外扩 0 让矩阵与
/// 扩展二十三逐位相同。统计字段在没跑起来之前必须是 0。
#[test]
fn defaults_keep_extension_twenty_three_behavior() {
    let default = CascadeSettings::default();
    assert!(
        default.caster_cull,
        "按级裁剪默认开启（保守裁剪，不改变画面）"
    );
    assert_eq!(
        default.caster_extend, 0.0,
        "外扩默认必须为 0，矩阵与扩展二十三一致"
    );
    assert_eq!(default.count, 1, "默认单级：老场景口径不变");

    let stats = RenderStats::default();
    assert_eq!(stats.shadow_casters, 0, "默认没有判过任何遮挡体");
    assert_eq!(stats.shadow_culled, 0, "默认没有裁掉任何遮挡体");
}

/// 掩码必须按级取，不能用同一份掩码跑完所有级。
#[test]
fn masks_are_taken_per_level() {
    let src = renderer_src();
    assert!(
        src.contains("shadow_runs(&groups, shadow_plan.masks.get(level)"),
        "每级必须取自己的掩码（masks.get(level)），否则远级会套用近级的保留集合"
    );
    assert!(
        src.contains("shadow_runs(&groups, None)"),
        "不裁剪的通道（局部光源阴影）必须走 mask = None 的全量路径"
    );
}

/// 开关、遮挡体、矩阵三者都在同一条路径上，缺一个都只能退回全量。
#[test]
fn cull_path_is_gated_by_switch() {
    let src = renderer_src();
    assert!(
        src.contains("cascade.caster_cull && !caster_bounds.is_empty()"),
        "裁剪必须同时要求「开关打开」与「有遮挡体包围盒」"
    );
    assert!(
        src.contains("r3e_core::plan_casters(&level_mats, caster_bounds)"),
        "掩码必须由 core 的 plan_casters 统一判定，渲染层不许自己算一遍"
    );
    // 只有生效级数的矩阵才参与判定：多出来的级不画，判了也是浪费
    assert!(
        src.contains("(0..levels)") && src.contains("Mat4::from_cols_array_2d(&matrices[level])"),
        "参与判定的矩阵必须只取生效级数"
    );
}

/// 统计口径要与字段注释一致，否则「省了多少」无法从日志里读出来。
#[test]
fn stats_follow_the_documented_formula() {
    let src = renderer_src();
    assert!(
        src.contains("plan.tested = (levels as u32).saturating_mul(caster_bounds.len() as u32)"),
        "shadow_casters 必须是 级数 × 遮挡体数"
    );
    assert!(
        src.contains("shadow_casters: shadow_plan.tested")
            && src.contains("shadow_culled: shadow_plan.culled"),
        "两个统计必须真的写进 RenderStats"
    );
    assert!(
        src.contains(".filter(|keep| !**keep).count() as u32"),
        "shadow_culled 必须由掩码里的 false 求和"
    );
}
