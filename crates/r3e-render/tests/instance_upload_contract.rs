//! 扩展三十一：实例缓冲脏段上传的 **渲染层接入契约**（不开窗口、不建 GPU）。
//!
//! 纯算法由 `src/instance_upload.rs` 的单测钉住，这一层钉「接得对不对」。
//! 历史上这类改动的翻车方式都很隐蔽：
//!
//! * 影子没跟着缓冲换代一起清 → 新缓冲里的未初始化字节被当成「已上传」，
//!   那一帧的实例会带着垃圾矩阵跳一帧（静态图看不出来，动起来才闪）；
//! * 影子更新早于 `write_buffer` → 上传若被打断，影子已经记着「传过了」，
//!   下一帧会跳掉真正的脏段，画面从此停在旧数据上；
//! * 旧的整张全量写没删干净 → 脏段白算，`bytes` 读数看着降了、实际流量没降。

use std::mem::size_of;

use r3e_render::instance_upload::{plan_upload, UploadPlan, MAX_RANGES, MERGE_GAP};
use r3e_render::renderer::{InstanceUploadStats, RenderStats};
use r3e_render::{InstanceRaw, MORPH_WEIGHT_GROUPS};

fn renderer_src() -> &'static str {
    include_str!("../src/renderer.rs")
}

/// 默认读数全零：没跑起来之前不能有任何「传过字节」的迹象。
#[test]
fn defaults_report_nothing_uploaded() {
    let stats = InstanceUploadStats::default();
    assert_eq!(stats.ranges, 0);
    assert_eq!(stats.recorded, 0);
    assert_eq!(stats.skipped, 0);
    assert_eq!(stats.bytes, 0);
    assert_eq!(stats.full_bytes, 0);
    assert!(!stats.full, "默认不该报告退化");
    assert_eq!(stats.resets, 0);

    let plan = UploadPlan::default();
    assert!(plan.is_empty());
    assert_eq!(plan.range_count(), 0);

    let stats = RenderStats::default();
    assert_eq!(stats.instance_upload.bytes, 0);
    assert_eq!(stats.instance_upload.skipped, 0);
}

/// 记录布局必须与 shader 的实例步长一致：`InstanceRaw` 160 字节、且能整除
/// 4 字节对齐要求。脏段换算用的是同一个 stride，这里把它钉住。
///
/// 扩展三十六加了形变（morph）三项 + 一组 `vec4` 权重，把记录从 96 撑到 112；
/// 扩展三十九把权重从 1 组扩到 `MORPH_WEIGHT_GROUPS` 组（2 组 → 128 字节）；
/// 扩展四十把掩码换成 `u32` 位图；扩展四十一把组数抬到 3 组（+16 字节）→ 148；
/// 扩展四十二把位图拆成 3 个 `u8` 序号字，并压进一个 `vec4<u32>` 属性 → **160**：
/// `model` 64 + `color` 16 + `joint_base`/`morph_base`/`morph_stride`/`morph_count`
/// 打包成 `Uint32x4`（16）+ 每组权重 `vec4`（16 × `MORPH_WEIGHT_GROUPS`）+ 序号 `vec4`（16）。
/// 序号属性固定吃 16 字节（3 个真实字 + 1 个占位字）—— 这是换 location 的票价：
/// webgpu 顶点 location 只有 16 格，摊成 3 个 `Uint32` 会顶到 16/17 被拒建管线。
/// **步长同时是平移量的语义**：任何一次字段增删都要
/// 同步改这里与 `shader.wgsl` 的 `@location(10)`/`@location(12..14)`/`@location(15)`。
#[test]
fn instance_record_stride_matches_weight_groups() {
    let expected = 96 + 16 * MORPH_WEIGHT_GROUPS + 16;
    assert_eq!(size_of::<InstanceRaw>(), expected);
    assert_eq!(
        size_of::<InstanceRaw>(),
        160,
        "扩展四十二：3 组权重 + 序号 vec4（4 × 4 字节）"
    );
    assert_eq!(size_of::<InstanceRaw>() % 4, 0);
}

/// 一万个静态实例里只动了一个：脏段必须收缩到那一小块，而不是整张 960 KiB。
#[test]
fn one_moving_instance_uploads_one_range() {
    let stride = size_of::<InstanceRaw>();
    let total = 10_000usize;
    let prev = vec![0u8; stride * total];
    let mut next = prev.clone();
    // 只改第 6182 条记录里的一个字节（模拟某物件挪了位置）
    next[6182 * stride + stride / 2] = 7;

    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert_eq!(plan.range_count(), 1);
    assert_eq!(plan.ranges[0], 6182..6183);
    assert_eq!(plan.bytes, stride as u64);
    assert_eq!(plan.skipped as usize, total - 1);
    assert!(!plan.full);
    assert!(
        plan.bytes * 1000 < (stride * total) as u64,
        "脏段上传必须把流量压到全量的千分之一以下"
    );
}

/// 整帧不动：一个字节都不传，且 `skipped` 记满 —— 探针靠这个判断
/// 「静态场景真的没在上传」。
#[test]
fn static_frame_uploads_zero_bytes() {
    let stride = size_of::<InstanceRaw>();
    let prev = vec![3u8; stride * 2048];
    let plan = plan_upload(&prev, &prev, stride, MERGE_GAP, MAX_RANGES);
    assert!(plan.is_empty());
    assert_eq!(plan.bytes, 0);
    assert_eq!(plan.recorded, 0);
    assert_eq!(plan.skipped, 2048);
}

/// 实例数变化（增删物件）时区间语义变了，必须整张重传，否则后半段是旧数据。
#[test]
fn instance_count_change_forces_full_upload() {
    let stride = size_of::<InstanceRaw>();
    let prev = vec![1u8; stride * 8];
    let next = vec![1u8; stride * 9];
    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert!(plan.full);
    assert_eq!(plan.ranges, vec![0..9]);
    assert_eq!(plan.bytes, (stride * 9) as u64);
}

/// 接线：上传必须走脏段函数，旧的整张写必须已经删掉。
#[test]
fn renderer_uploads_through_dirty_runs() {
    let src = renderer_src();
    assert!(
        src.contains("self.upload_instance_runs(&all)"),
        "prepare_instances 必须把上传交给 upload_instance_runs"
    );
    assert!(
        src.contains("instance_upload::plan_upload("),
        "脏段必须由 plan_upload 算出来（纯函数，可单测）"
    );
    assert!(
        !src.contains("write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&all))"),
        "旧的整张全量写必须删掉，否则脏段白算"
    );
}

/// 接线：缓冲换代必须清影子并记一次 reset，否则那一帧会带着未初始化字节作画。
#[test]
fn buffer_rebuild_clears_shadow() {
    let src = renderer_src();
    assert!(
        src.contains("self.instance_shadow.clear()"),
        "换代 / 空场景都要清影子"
    );
    assert!(
        src.contains("self.instance_upload_resets += 1"),
        "换代要计数，resets 逐帧增长说明容量估算失效"
    );
    // 影子必须在 write_buffer 之后才更新
    let body_start = src
        .find("fn upload_instance_runs")
        .expect("上传函数必须存在");
    let body = &src[body_start..];
    let write_at = body.find("write_buffer(").expect("必须真的写 GPU");
    let shadow_at = body
        .find("self.instance_shadow.extend_from_slice(all)")
        .expect("必须刷新影子");
    assert!(
        write_at < shadow_at,
        "影子必须在写 GPU 之后再刷新，否则中断一次就永久漏传"
    );
}

/// 统计口径自洽：`recorded + skipped` 恒等于记录总数，`bytes` 恒等于
/// `recorded × stride`。探针与 editor JSON 都按这个口径读数。
#[test]
fn stats_are_self_consistent() {
    let stride = size_of::<InstanceRaw>();
    let total = 512usize;
    let prev = vec![0u8; stride * total];
    let mut next = prev.clone();
    for index in [3usize, 100, 101, 400] {
        next[index * stride + 1] = 9;
    }
    let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
    assert_eq!(plan.recorded + plan.skipped, total as u32);
    assert_eq!(plan.bytes, plan.recorded as u64 * stride as u64);
    assert!(!plan.full);
}
