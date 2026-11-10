//! 扩展三十五：暂存环（staging ring）+ 多帧飞行。
//!
//! 扩展三十一 / 三十二把「传多少字节」压到了脏段级别，但**调用次数**没变：
//! 一个 24 段的上限意味着单帧最多 24 次 `Queue::write_buffer`，每次调用 wgpu 都要
//! 在内部临时缓冲上做一次映射、一次 `copy_buffer_to_buffer` 录制和一次队列校验。
//! 静态场景里这些段通常只有几百字节，真正贵的是调用开销而不是带宽。
//!
//! 这一层把「本帧所有脏段的数据」先 memcpy 进**一块** CPU 暂存区，一次性交给 GPU，
//! 再由命令编码器按段 `copy_buffer_to_buffer` 搬到目标缓冲：
//!
//! * 实例与关节两条路径（96 B / 64 B 记录）共用同一个暂存槽，所以一帧只有一次写入；
//! * 槽位轮转做**多帧飞行**（[`STAGING_SLOTS`] 帧），避免覆盖 GPU 还在读的槽；
//! * 槽容量按需 2 倍思路增长到 [`STAGING_MAX_BYTES`] 为止，超过即由调用方退化回
//!   逐段 `write_buffer`（绝不因为优化让大场景变慢）。
//!
//! 本模块是**纯 CPU 契约**，不碰 wgpu：打包、槽位规划、游标轮转都能脱离设备单测，
//! 渲染器只负责把 `copies` 变成 `copy_buffer_to_buffer`。
//!
//! 关于「持久映射」：[`wgpu`] 要求缓冲在作为拷贝源之前处于 unmapped 状态，
//! 长期保持映射的缓冲无法进入命令编码器。因此这里的实现是「一帧一次写入 + 环形
//! 槽位飞行」，它在效果上等价于持久映射的批量化收益，又不需要在帧间做 map/unmap
//! 状态机。

use std::ops::Range;

/// 暂存环的槽数 —— 即「多帧飞行」的深度。3 帧足够覆盖
/// 「本帧录制 + 上帧在飞 + 上上帧可能未完成」的窗口，再多只是白占显存。
pub const STAGING_SLOTS: usize = 3;

/// 单槽最小字节数。低于这个量级时扩容/重建的成本比省下的调用开销还大。
pub const STAGING_MIN_BYTES: u64 = 64 * 1024;

/// 单槽字节上限。超过即视为「不值得走暂存」——大场景直接逐段上传更快，
/// 也避免为了 8 MiB 以上的脏数据白占三倍显存。
pub const STAGING_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 槽容量对齐粒度。`copy_buffer_to_buffer` 的偏移要求 4 字节对齐，
/// 这里取 256 是为了让槽尺寸落在整页上，便于不同记录布局（96 / 64 B）复用。
pub const STAGING_ALIGN: u64 = 256;

/// 一条「从暂存槽到目标缓冲」的拷贝。
///
/// `src_offset` 是槽内偏移，`dst_offset` 是目标缓冲（实例或关节）内的字节偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedCopy {
    /// 目标缓冲内的字节偏移（`记录下标 * stride`）。
    pub dst_offset: u64,
    /// 暂存槽内的字节偏移（打包顺序累加）。
    pub src_offset: u64,
    /// 拷贝字节数，恒为 stride 的整数倍。
    pub size: u64,
}

/// 一帧的暂存打包结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StagedPack {
    /// 本帧要写进暂存槽的连续字节（`copies` 的源）。
    pub bytes: Vec<u8>,
    /// 逐段的拷贝指令，顺序与 `bytes` 的填充顺序一致。
    pub copies: Vec<StagedCopy>,
    /// 打包后总字节数超出槽容量：调用方必须退化回逐段 `write_buffer`，
    /// 此时 `bytes` / `copies` 都是空的（不做半截事）。
    pub over_budget: bool,
}

impl StagedPack {
    /// 本帧暂存字节数。
    pub fn total_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// 什么也没打包（本帧无脏段）。
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }
}

/// 槽位容量规划结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotFit {
    /// 现有槽装得下，直接用。
    Fits,
    /// 装不下但值得扩：按给定字节数重建整环。
    Grow(u64),
    /// 需求超过 [`STAGING_MAX_BYTES`]：别扩了，退化回直传。
    TooLarge,
}

/// 按对齐粒度与下限算出「装下 `need` 字节」该给的单槽容量。
pub fn slot_bytes(need: u64) -> u64 {
    let aligned = need.div_ceil(STAGING_ALIGN) * STAGING_ALIGN;
    aligned.max(STAGING_MIN_BYTES)
}

/// 判断现有槽容量是否够用；不够时给出新的单槽容量。
///
/// 先看「够不够」再看「大不大」：已经扩到 16 MiB 的槽在碰到
/// [`STAGING_MAX_BYTES`] 以上的需求时仍算 `TooLarge`，但 `need <= capacity`
/// 时一律 `Fits` —— 不让上限反过来卡住已经存在的槽。
pub fn slot_fit(need: u64, capacity: u64) -> SlotFit {
    if need <= capacity {
        return SlotFit::Fits;
    }
    if need > STAGING_MAX_BYTES {
        return SlotFit::TooLarge;
    }
    SlotFit::Grow(slot_bytes(need).min(STAGING_MAX_BYTES))
}

/// 相对逐段直传，本帧省下的队列调用次数。
///
/// 走暂存路径时整帧只有一次 `write_buffer`，所以「脏段数 - 已发生的写入数」
/// 就是省下来的调用往返；同帧多次调用时会自动累进（`writes` 传累计值）。
pub fn saved_calls(ranges: u64, writes: u64) -> u64 {
    ranges.saturating_sub(writes)
}

/// 把 `ranges` 指定的记录段打进一块连续字节，并算出逐段拷贝指令。
///
/// * `next`：本帧记录的原始字节视图（与 GPU 缓冲同布局）；
/// * `stride`：单条记录字节数（`InstanceRaw` 96 / `mat4` 64）；
/// * `ranges`：**记录下标**区间，来自 `instance_upload::plan_upload`；
/// * `capacity`：当前槽容量，装不下就返回 `over_budget`，不做截断打包。
///
/// 段顺序保持 `ranges` 的原序，`src_offset` 累加，因此 `bytes` 与 `copies`
/// 一一对应；调用方按 `copies` 逐条录制即可。
pub fn pack_runs(next: &[u8], stride: usize, ranges: &[Range<u32>], capacity: u64) -> StagedPack {
    let stride = stride.max(1);
    let total: u64 = ranges
        .iter()
        .map(|run| run.end.saturating_sub(run.start) as u64 * stride as u64)
        .sum();
    if total == 0 {
        return StagedPack::default();
    }
    if total > capacity {
        return StagedPack {
            over_budget: true,
            ..Default::default()
        };
    }

    let mut bytes = Vec::with_capacity(total as usize);
    let mut copies = Vec::with_capacity(ranges.len());
    for run in ranges {
        let start = run.start as usize * stride;
        let end = (run.end as usize * stride).min(next.len());
        if end <= start {
            continue;
        }
        let src_offset = bytes.len() as u64;
        bytes.extend_from_slice(&next[start..end]);
        copies.push(StagedCopy {
            dst_offset: start as u64,
            src_offset,
            size: (end - start) as u64,
        });
    }
    StagedPack {
        bytes,
        copies,
        over_budget: false,
    }
}

/// 环形槽位游标（多帧飞行的状态机）。
///
/// 只在「本槽的拷贝都已录进命令编码器」时才能推进到下一槽，否则留在当前槽继续
/// 追加 —— 这是保护 GPU 仍在读取的槽不被覆盖的唯一机制，跳过它就会出现
/// 「画面素材偶发撕裂一帧」这种极难复现的问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRing {
    slots: usize,
    cursor: usize,
}

impl SlotRing {
    /// 建一个 `slots` 深度的环（至少 1，避免除零）。
    pub fn new(slots: usize) -> Self {
        Self {
            slots: slots.max(1),
            cursor: 0,
        }
    }

    /// 环的深度。
    pub fn slots(&self) -> usize {
        self.slots
    }

    /// 当前槽下标。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 尝试推进到下一槽。`committed` 为 false（本帧拷贝还没提交）时原地不动，
    /// 返回是否真的推进了。
    pub fn advance(&mut self, committed: bool) -> bool {
        if !committed {
            return false;
        }
        self.cursor = (self.cursor + 1) % self.slots;
        true
    }

    /// 归零（容量重建后从第 0 槽重新开始）。
    pub fn reset(&mut self) {
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单个区间。写成 `&[a..b]` 会被 clippy 的 `single_range_in_vec_init` 拦下
    /// （与扩展三十一 `plan_upload` 首版踩的同一个坑），这里统一走一个具名构造。
    fn one(run: Range<u32>) -> [Range<u32>; 1] {
        [run]
    }

    /// 4 字节记录的假数据，方便手算偏移。
    fn records(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn single_run_copies_bytes_verbatim() {
        let next = records(&[1, 2, 3, 4]);
        let pack = pack_runs(&next, 4, &one(1..3), 1024);
        assert!(!pack.over_budget);
        assert_eq!(pack.bytes, records(&[2, 3]));
        assert_eq!(
            pack.copies,
            vec![StagedCopy {
                dst_offset: 4,
                src_offset: 0,
                size: 8
            }]
        );
        assert_eq!(pack.total_bytes(), 8);
    }

    #[test]
    fn two_runs_accumulate_src_offsets() {
        let next = records(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let pack = pack_runs(&next, 4, &[0..1, 5..8], 1024);
        assert_eq!(pack.bytes, records(&[1, 6, 7, 8]));
        assert_eq!(
            pack.copies,
            vec![
                StagedCopy {
                    dst_offset: 0,
                    src_offset: 0,
                    size: 4
                },
                StagedCopy {
                    dst_offset: 20,
                    src_offset: 4,
                    size: 12
                },
            ]
        );
        // 目标偏移保持绝对（能直接喂给 copy_buffer_to_buffer 的偏移），
        // 源偏移连续（暂存区没有空洞）
        assert_eq!(pack.copies[1].dst_offset, 5 * 4);
        assert_eq!(pack.copies[1].src_offset, 4);
    }

    #[test]
    fn empty_ranges_pack_nothing() {
        let next = records(&[1, 2, 3]);
        let pack = pack_runs(&next, 4, &[], 1024);
        assert!(pack.is_empty());
        assert_eq!(pack.total_bytes(), 0);
        assert!(!pack.over_budget);
    }

    #[test]
    fn over_capacity_reports_budget_instead_of_truncating() {
        let next = vec![7u8; 96 * 10];
        let pack = pack_runs(&next, 96, &one(0..10), 512);
        assert!(pack.over_budget, "装不下必须整包放弃");
        assert!(pack.bytes.is_empty(), "不能做半截打包");
        assert!(pack.copies.is_empty());
    }

    #[test]
    fn range_past_end_is_clamped_to_source() {
        // 记录数组比 ranges 声明的短（调用方算错时的兜底）：截断而不是 panic
        let next = vec![1u8; 8];
        let pack = pack_runs(&next, 4, &one(0..4), 1024);
        assert_eq!(pack.bytes.len(), 8);
        assert_eq!(pack.copies.len(), 1);
        assert_eq!(pack.copies[0].size, 8);
    }

    #[test]
    fn ninety_six_byte_records_map_to_exact_offsets() {
        // 真实 InstanceRaw：第 2 条记录 → 目标偏移 2 * 96 = 192
        let mut next = vec![0u8; 96 * 4];
        next[96 * 2 + 3] = 0xAB;
        let pack = pack_runs(&next, 96, &one(2..3), STAGING_MIN_BYTES);
        assert_eq!(
            pack.copies,
            vec![StagedCopy {
                dst_offset: 192,
                src_offset: 0,
                size: 96
            }]
        );
        assert_eq!(pack.bytes[3], 0xAB);
    }

    #[test]
    fn slot_bytes_respects_minimum_and_alignment() {
        assert_eq!(slot_bytes(0), STAGING_MIN_BYTES);
        assert_eq!(slot_bytes(1), STAGING_MIN_BYTES);
        // 超过下限后按 256 上取整
        assert_eq!(slot_bytes(STAGING_MIN_BYTES + 1), STAGING_MIN_BYTES + 256);
        assert_eq!(slot_bytes(STAGING_MIN_BYTES + 256), STAGING_MIN_BYTES + 256);
    }

    #[test]
    fn slot_fit_three_way() {
        assert_eq!(slot_fit(1024, STAGING_MIN_BYTES), SlotFit::Fits);
        assert_eq!(
            slot_fit(STAGING_MIN_BYTES + 1, STAGING_MIN_BYTES),
            SlotFit::Grow(STAGING_MIN_BYTES + 256)
        );
        assert_eq!(
            slot_fit(STAGING_MAX_BYTES + 1, STAGING_MIN_BYTES),
            SlotFit::TooLarge
        );
        // 已经扩到上限的槽，需求不超容量时仍算 Fits
        assert_eq!(
            slot_fit(STAGING_MAX_BYTES, STAGING_MAX_BYTES),
            SlotFit::Fits
        );
    }

    #[test]
    fn ring_does_not_advance_before_commit() {
        let mut ring = SlotRing::new(STAGING_SLOTS);
        assert_eq!(ring.cursor(), 0);
        assert!(!ring.advance(false), "未提交时必须留在原槽");
        assert_eq!(ring.cursor(), 0);
        assert!(ring.advance(true));
        assert_eq!(ring.cursor(), 1);
    }

    #[test]
    fn ring_wraps_after_three_frames() {
        let mut ring = SlotRing::new(STAGING_SLOTS);
        let seen: Vec<usize> = (0..7)
            .map(|_| {
                let at = ring.cursor();
                ring.advance(true);
                at
            })
            .collect();
        assert_eq!(seen, vec![0, 1, 2, 0, 1, 2, 0]);
        ring.reset();
        assert_eq!(ring.cursor(), 0);
    }

    #[test]
    fn saved_calls_counts_queue_round_trips() {
        // 24 段脏区 + 一次整帧写入 → 省 23 次
        assert_eq!(saved_calls(24, 1), 23);
        // 只有一段时与直传持平，不算「省」
        assert_eq!(saved_calls(1, 1), 0);
        // 关节路径把累计写入数带上后，两路合并只算一次开销
        assert_eq!(saved_calls(10 + 6, 1), 15);
        // 关闭暂存（逐段直传）时不会出现负数
        assert_eq!(saved_calls(0, 3), 0);
    }

    #[test]
    fn ring_with_zero_slots_is_clamped() {
        let mut ring = SlotRing::new(0);
        assert_eq!(ring.slots(), 1);
        assert!(ring.advance(true));
        assert_eq!(ring.cursor(), 0, "单槽环永远停在 0");
    }
}
