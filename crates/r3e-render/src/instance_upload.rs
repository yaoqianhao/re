//! 扩展三十一：主通道实例缓冲的脏段上传。
//!
//! 扩展二十七起实例缓冲已经**跨帧复用**（`instance_capacity` 按 2 倍扩容，只有容量
//! 不足时才 `create_buffer` 换代），但每帧仍然把整张 `all` 全量 `write_buffer`：
//! 一万实例 = 960 KiB/帧，而静态场景里这 960 KiB 有九成以上是上一帧逐字节相同的副本。
//!
//! 这一层只算「哪些记录变了」，把变化处按连续区间合并成少量脏段交回渲染器逐段上传。
//! **记录内容一个字节都不改**，所以画面与全量上传逐位一致；调用方只需保证影子缓冲
//! （上一帧真正传上去的字节）与 GPU 缓冲内容同步，换代时清空影子即可退化为全量。
//!
//! 两个启发式都在「段数」和「字节数」之间取舍：
//!
//! * `MERGE_GAP`：两段脏区之间的空洞不超过它就并成一段 —— 多用几百字节换掉一次
//!   `write_buffer`；
//! * `MAX_RANGES`：段数超了就整张全量 —— `write_buffer` 每次调用都要过队列校验，
//!   碎成上百段时按段提交比全量更贵。

use std::ops::Range;

/// 相邻两段脏区之间允许的「空洞」记录数上限，默认 8。
pub const MERGE_GAP: u32 = 8;

/// 单帧脏段数量上限，默认 24；超过即退化为全量上传。
pub const MAX_RANGES: u32 = 24;

/// 一帧的上传计划。`ranges` 里的区间是**记录下标**（不是字节偏移），
/// 渲染器按 `start * size_of::<InstanceRaw>()` 换算后逐段 `write_buffer`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UploadPlan {
    /// 需要写入的连续记录区间，按起点升序且互不重叠。
    pub ranges: Vec<Range<u32>>,
    /// 本次真正写入的记录数（含为合并而重传的空洞）。
    pub recorded: u32,
    /// 与上一帧逐字节相同、被跳过的记录数。`recorded + skipped` 恒等于记录总数。
    pub skipped: u32,
    /// 本次实际写入的字节数。
    pub bytes: u64,
    /// 是否退化成了「整张从 0 开始的单段」—— 换代、长度变化、段数超限都走这里。
    pub full: bool,
}

impl UploadPlan {
    /// 脏段数量（日志用）。
    pub fn range_count(&self) -> u32 {
        self.ranges.len() as u32
    }

    /// 若本帧什么都不用传（与上一帧逐字节一致），返回 true。
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

/// 比较上一帧影子与本次记录数组，算出需要上传的脏段。
///
/// * `prev` / `next`：同一记录布局的原始字节视图，长度不等即视为没有可比基线；
/// * `record`：单条记录的字节数（`std::mem::size_of::<InstanceRaw>()`，112）；
/// * 记录之间用整段切片比较（`memcmp` 语义），因此 NaN / `-0.0` 这类
///   位模式差异不会被误判成「没变」—— 位不同就算脏，宁多传不漏传。
pub fn plan_upload(
    prev: &[u8],
    next: &[u8],
    record: usize,
    merge_gap: u32,
    max_ranges: u32,
) -> UploadPlan {
    let stride = record.max(1);
    let total = (next.len() / stride) as u32;
    if total == 0 {
        return UploadPlan::default();
    }
    let all_bytes = total as u64 * stride as u64;

    // 没有可比基线（首帧 / 扩容换代 / 实例数变了）：只能全量
    if prev.len() != next.len() || prev.len() / stride != total as usize {
        return UploadPlan {
            ranges: std::iter::once(0..total).collect(),
            recorded: total,
            skipped: 0,
            bytes: all_bytes,
            full: true,
        };
    }

    let mut runs: Vec<Range<u32>> = Vec::new();
    for index in 0..total {
        let start = index as usize * stride;
        if prev[start..start + stride] == next[start..start + stride] {
            continue;
        }
        match runs.last_mut() {
            // 空洞不超过 merge_gap 就并进上一段
            Some(last) if index <= last.end.saturating_add(merge_gap) => last.end = index + 1,
            _ => runs.push(index..index + 1),
        }
    }

    if runs.is_empty() {
        // 整帧逐字节相同：一个字节都不用传，全部记录都算「跳过」
        return UploadPlan {
            skipped: total,
            ..Default::default()
        };
    }
    if runs.len() as u32 > max_ranges {
        return UploadPlan {
            ranges: std::iter::once(0..total).collect(),
            recorded: total,
            skipped: 0,
            bytes: all_bytes,
            full: true,
        };
    }

    let recorded: u32 = runs.iter().map(|run| run.end - run.start).sum();
    UploadPlan {
        ranges: runs,
        recorded,
        skipped: total - recorded,
        bytes: recorded as u64 * stride as u64,
        full: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录大小 4 字节的假实例，方便手算区间。
    fn records(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn values_of(plan: &UploadPlan) -> Vec<Range<u32>> {
        plan.ranges.clone()
    }

    #[test]
    fn identical_frames_upload_nothing() {
        let prev = records(&[1, 2, 3, 4, 5, 6]);
        let next = records(&[1, 2, 3, 4, 5, 6]);
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert!(plan.is_empty(), "逐字节相同不该产生上传");
        assert_eq!(plan.recorded, 0);
        assert_eq!(plan.skipped, 6);
        assert_eq!(plan.bytes, 0);
        assert!(!plan.full);
    }

    #[test]
    fn empty_next_frame_uploads_nothing() {
        let plan = plan_upload(&[], &[], 96, MERGE_GAP, MAX_RANGES);
        assert!(plan.is_empty());
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn first_frame_without_shadow_uploads_everything() {
        let next = records(&[7, 8, 9]);
        let plan = plan_upload(&[], &next, 4, MERGE_GAP, MAX_RANGES);
        assert!(plan.full, "没有基线必须退化为全量");
        assert_eq!(values_of(&plan), vec![0..3]);
        assert_eq!(plan.recorded, 3);
        assert_eq!(plan.bytes, 12);
    }

    #[test]
    fn record_count_change_rebuilds_whole_buffer() {
        let prev = records(&[1, 2, 3]);
        let next = records(&[1, 2, 3, 4]);
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert!(plan.full, "实例数变化时区间语义变了，只能全量");
        assert_eq!(values_of(&plan), vec![0..4]);
        assert_eq!(plan.skipped, 0);
    }

    #[test]
    fn tail_change_becomes_single_tail_range() {
        let prev = records(&[1, 2, 3, 4]);
        let next = records(&[1, 2, 9, 9]);
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![2..4]);
        assert_eq!(plan.recorded, 2);
        assert_eq!(plan.skipped, 2);
        assert_eq!(plan.bytes, 8);
        assert!(!plan.full);
    }

    #[test]
    fn small_gap_is_merged_single_call_wins() {
        // 脏点在第 1、第 4 条：间隔 2 条 <= merge_gap，并成一段
        let prev = records(&[0, 0, 0, 0, 0, 0]);
        let next = records(&[0, 1, 0, 0, 1, 0]);
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![1..5]);
        assert_eq!(plan.recorded, 4, "空洞一起重传");
        assert_eq!(plan.skipped, 2);
    }

    #[test]
    fn large_gap_stays_two_ranges() {
        let prev = records(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let mut next_values = [0u32; 16];
        next_values[0] = 1;
        next_values[15] = 1;
        let next = records(&next_values);
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![0..1, 15..16]);
        assert_eq!(plan.recorded, 2);
        assert_eq!(plan.skipped, 14);
    }

    #[test]
    fn merge_gap_boundary_is_inclusive() {
        // 与第 0 条相距 8 条：落在 index 8 <= 1 + 8 的连通边界上 → 合并
        let mut wide = vec![0u32; 16];
        wide[0] = 1;
        wide[9] = 1;
        let plan = plan_upload(
            &records(&[0u32; 16]),
            &records(&wide),
            4,
            MERGE_GAP,
            MAX_RANGES,
        );
        assert_eq!(values_of(&plan), vec![0..10], "间隔恰为 merge_gap 时连通");
        // 间隔 9 > 8 时断开
        let mut split = vec![0u32; 16];
        split[0] = 1;
        split[10] = 1;
        let plan = plan_upload(
            &records(&[0u32; 16]),
            &records(&split),
            4,
            MERGE_GAP,
            MAX_RANGES,
        );
        assert_eq!(values_of(&plan), vec![0..1, 10..11]);
    }

    #[test]
    fn too_many_ranges_falls_back_to_full_upload() {
        // 32 条记录、隔一条脏一条 → 16 段；把上限压到 4 强制退化
        let prev = records(&[0u32; 32]);
        let next_values: Vec<u32> = (0..32).map(|i| if i % 2 == 0 { 1 } else { 0 }).collect();
        let plan = plan_upload(&prev, &records(&next_values), 4, 0, 4);
        assert!(plan.full, "段数超过上限时按全量提交");
        assert_eq!(values_of(&plan), vec![0..32]);
        assert_eq!(plan.bytes, 128);
    }

    #[test]
    fn bit_level_change_counts_as_dirty() {
        // +0.0 / -0.0 数值相等但位模式不同：必须当成脏，否则 GPU 侧会留着旧符号位
        let prev = 0.0f32.to_le_bytes();
        let next = (-0.0f32).to_le_bytes();
        let plan = plan_upload(&prev, &next, 4, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![0..1]);
        assert_eq!(plan.recorded, 1);
    }

    #[test]
    fn ninety_six_byte_records_align_correctly() {
        // 真实 InstanceRaw 是 112 字节：只有第 1 条的第 90 字节变了，也必须定位到第 1 条
        let mut prev = vec![0u8; 96 * 3];
        let mut next = prev.clone();
        prev[96 + 90] = 0;
        next[96 + 90] = 1;
        let plan = plan_upload(&prev, &next, 96, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![1..2]);
        assert_eq!(plan.bytes, 96);
        assert_eq!(plan.skipped, 2);
    }

    #[test]
    fn joint_matrices_use_sixty_four_byte_records() {
        // 扩展三十二：关节记录是 mat4（64 B）。改了第 1 个矩阵的平移列
        // （列主序 index 12 = 字节 48）也只能脏那一条，不能牵连同帧其它角色。
        let stride = 64usize;
        let mut prev = vec![0u8; stride * 4];
        let mut next = prev.clone();
        prev[stride + 48] = 0;
        next[stride + 48] = 1;
        let plan = plan_upload(&prev, &next, stride, MERGE_GAP, MAX_RANGES);
        assert_eq!(values_of(&plan), vec![1..2]);
        assert_eq!(plan.bytes, 64);
        assert_eq!(plan.skipped, 3);
    }

    #[test]
    fn static_pose_uploads_nothing() {
        // 没有动画的蒙皮物件每帧矩阵逐字节相同：零字节上传，全部计入跳过
        let stride = 64usize;
        let joints = vec![7u8; stride * 96];
        let plan = plan_upload(&joints, &joints, stride, MERGE_GAP, MAX_RANGES);
        assert!(plan.is_empty());
        assert_eq!(plan.bytes, 0);
        assert_eq!(plan.recorded, 0);
        assert_eq!(plan.skipped, 96);
    }
}
