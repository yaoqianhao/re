use crate::mesh::InstanceRaw;
use crate::upload_staging::{
    self, slot_fit, SlotFit, SlotRing, StagedCopy, STAGING_MIN_BYTES, STAGING_SLOTS,
};

use super::*;

/// 扩展三十五：块拷贝的目标缓冲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StagingTarget {
    /// 主通道实例缓冲（`instance_buffer`）。
    Instances,
    /// 蒙皮关节矩阵缓冲（`joint_buffer`）。
    Joints,
}

/// 一条待录进命令编码器的块拷贝。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingCopy {
    pub(crate) target: StagingTarget,
    pub(crate) copy: StagedCopy,
}

/// 扩展三十五：暂存环本体（GPU 侧）。
///
/// 每帧的脏段数据先 memcpy 进 `frame_bytes`，帧内**只调一次**
/// `Queue::write_buffer` 把它整体塞进当前槽，再由命令编码器把槽里的各段
/// `copy_buffer_to_buffer` 到目标缓冲。槽数 = 多帧飞行深度，只有本帧拷贝
/// 全部录制完成才轮转 —— 保证 GPU 还在读的槽不会被下一帧覆盖。
pub(crate) struct UploadStaging {
    /// 每槽一块缓冲：`write_buffer` 写入（COPY_DST），编码器读出（COPY_SRC）。
    pub(crate) slots: Vec<wgpu::Buffer>,
    pub(crate) ring: SlotRing,
    /// 单槽字节容量。
    pub(crate) capacity: u64,
    /// 本帧待写进槽的字节（实例与关节合并成一次写入）。
    pub(crate) frame_bytes: Vec<u8>,
    /// 本帧待录制的块拷贝。
    pub(crate) pending: Vec<PendingCopy>,
    /// 总开关（`R3E_UPLOAD_STAGING=0` 关闭，回到逐段 `write_buffer`）。
    pub(crate) enabled: bool,
    /// 累计读数。
    pub(crate) stats: StagingStats,
    /// 本帧已发生的 `write_buffer` 次数（每帧开头清零）。
    pub(crate) frame_writes: u64,
    /// 本帧走暂存路径的脏段总数（用来算省下的调用次数，每帧开头清零）。
    pub(crate) frame_ranges: u64,
    /// 走暂存路径时，本帧实例记录的快照 —— 要等命令**提交之后**才写进影子。
    ///
    /// 暂存路径的 GPU 写入是 `copy_buffer_to_buffer`，只有 submit 之后才真正
    /// 生效；若像直传路径那样当场更新影子，一旦本帧命令没有提交，影子会记着
    /// 「传过了」而缓冲里还是旧数据，造成永久漏传（扩展三十一的踩坑）。
    pub(crate) shadow_instances: Option<Vec<InstanceRaw>>,
    /// 同上，关节矩阵侧（`mat4`，定长、不换代）。
    pub(crate) shadow_joints: Option<Vec<[f32; 16]>>,
}

impl UploadStaging {
    /// 建环：`STAGING_SLOTS` 块最小容量的缓冲。
    pub(crate) fn new(device: &wgpu::Device, enabled: bool) -> Self {
        let mut staging = Self {
            slots: Vec::new(),
            ring: SlotRing::new(STAGING_SLOTS),
            capacity: STAGING_MIN_BYTES,
            frame_bytes: Vec::new(),
            pending: Vec::new(),
            enabled,
            stats: StagingStats {
                enabled,
                slots: STAGING_SLOTS as u32,
                capacity: STAGING_MIN_BYTES,
                ..Default::default()
            },
            frame_writes: 0,
            frame_ranges: 0,
            shadow_instances: None,
            shadow_joints: None,
        };
        staging.allocate(device);
        staging
    }

    /// 按当前 `capacity` 重建全部槽（扩容 / 首次分配共用）。
    pub(crate) fn allocate(&mut self, device: &wgpu::Device) {
        self.slots.clear();
        for index in 0..self.ring.slots() {
            self.slots
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("r3e-upload-staging-{index}")),
                    size: self.capacity,
                    usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        // 重建后所有旧槽内容作废，本帧已排的拷贝也一并丢弃（宁可多传一遍）
        self.frame_bytes.clear();
        self.pending.clear();
        self.ring.reset();
        self.stats.capacity = self.capacity;
    }

    /// 每帧开头：清零本帧写入计数。
    pub(crate) fn begin_frame(&mut self) {
        self.frame_writes = 0;
        self.frame_ranges = 0;
        // 上一帧若没走到提交，这里把「已暂存但没生效」的影子丢掉：
        // 影子保持原值，下一帧自然还会把同样的脏段再传一遍。
        self.shadow_instances = None;
        self.shadow_joints = None;
    }

    /// 把一条上传路径的脏段打进暂存区。
    ///
    /// 返回 `true` = 已进暂存（调用方不要再逐段 `write_buffer`）；
    /// 返回 `false` = 装不下 / 超上限 / 已关闭（调用方必须退化回逐段直传）。
    pub(crate) fn stage(
        &mut self,
        device: &wgpu::Device,
        target: StagingTarget,
        next_bytes: &[u8],
        stride: usize,
        ranges: &[std::ops::Range<u32>],
    ) -> bool {
        if !self.enabled || ranges.is_empty() {
            return false;
        }
        let stride = stride.max(1);
        let need: u64 = ranges
            .iter()
            .map(|run| run.end.saturating_sub(run.start) as u64 * stride as u64)
            .sum();
        // 槽容量按「本帧已打包字节 + 本次需求」规划：实例与关节共用一个槽
        let want = self.frame_bytes.len() as u64 + need;
        match slot_fit(want, self.capacity) {
            SlotFit::Fits => {}
            SlotFit::Grow(bytes) => {
                self.capacity = bytes;
                self.allocate(device);
                self.stats.rebuilds += 1;
            }
            SlotFit::TooLarge => {
                self.stats.fallbacks += 1;
                return false;
            }
        }

        let pack = upload_staging::pack_runs(next_bytes, stride, ranges, self.capacity);
        if pack.over_budget || pack.copies.is_empty() {
            self.stats.fallbacks += 1;
            return false;
        }
        let base = self.frame_bytes.len() as u64;
        self.frame_ranges += ranges.len() as u64;
        self.frame_bytes.extend_from_slice(&pack.bytes);
        for mut copy in pack.copies {
            copy.src_offset += base;
            self.pending.push(PendingCopy { target, copy });
        }
        true
    }

    /// 本帧一次写入：把 `frame_bytes` 整体塞进当前槽。
    pub(crate) fn flush(&mut self, queue: &wgpu::Queue) {
        if self.frame_bytes.is_empty() {
            return;
        }
        let slot = self.ring.cursor();
        queue.write_buffer(&self.slots[slot], 0, &self.frame_bytes);
        self.frame_writes += 1;
        self.stats.writes += 1;
        self.stats.bytes += self.frame_bytes.len() as u64;
        self.frame_bytes.clear();
        // 本帧本该有的逐段调用次数 - 实际调用次数 = 省下的队列往返
        self.stats.saves += upload_staging::saved_calls(self.frame_ranges, self.frame_writes);
    }

    /// 把排好的块拷贝录进编码器，并轮转槽位（多帧飞行）。
    ///
    /// 调用点必须在**创建编码器之后、提交之前**，且每帧只调一次；录完即认为
    /// 本槽的数据会被 GPU 消费，因此紧接着推进到下一槽。
    pub(crate) fn record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        instance_buffer: &wgpu::Buffer,
        joint_buffer: &wgpu::Buffer,
    ) {
        if self.pending.is_empty() {
            // 本帧没有走暂存的数据：仍要轮转，否则下一帧会写到 GPU 可能还在读的槽
            self.ring.advance(true);
            return;
        }
        let slot = self.ring.cursor();
        let source = &self.slots[slot];
        for item in &self.pending {
            let target = match item.target {
                StagingTarget::Instances => instance_buffer,
                StagingTarget::Joints => joint_buffer,
            };
            encoder.copy_buffer_to_buffer(
                source,
                item.copy.src_offset,
                target,
                item.copy.dst_offset,
                item.copy.size,
            );
        }
        self.stats.copies += self.pending.len() as u64;
        self.stats.frames += 1;
        self.pending.clear();
        self.ring.advance(true);
    }

    /// 丢掉本帧尚未提交的拷贝（目标缓冲换代时用）。
    pub(crate) fn drop_pending(&mut self) {
        self.pending.clear();
        self.frame_bytes.clear();
        self.shadow_instances = None;
        self.shadow_joints = None;
    }

    /// 走暂存路径时登记本帧的实例记录快照（提交后才应用）。
    pub(crate) fn set_instance_shadow(&mut self, all: &[InstanceRaw]) {
        self.shadow_instances = Some(all.to_vec());
    }

    /// 走暂存路径时登记本帧的关节记录快照（提交后才应用）。
    pub(crate) fn set_joint_shadow(&mut self, joints: &[[f32; 16]]) {
        self.shadow_joints = Some(joints.to_vec());
    }

    /// 命令提交之后调用：把暂存路径的影子一次性对齐到本帧内容。
    ///
    /// 此刻 `copy_buffer_to_buffer` 已入队，影子与 GPU 缓冲重新同步；
    /// 直传路径的影子在 `write_buffer` 当场就更新过，这里不会重复写。
    pub(crate) fn commit_shadows(
        &mut self,
        instance_shadow: &mut Vec<InstanceRaw>,
        joint_shadow: &mut Vec<[f32; 16]>,
    ) {
        if let Some(records) = self.shadow_instances.take() {
            instance_shadow.clear();
            instance_shadow.extend_from_slice(&records);
        }
        if let Some(records) = self.shadow_joints.take() {
            joint_shadow.clear();
            joint_shadow.extend_from_slice(&records);
        }
    }

    /// 对外读数快照（把与槽位状态相关的字段对齐到当前值）。
    pub(crate) fn snapshot(&self) -> StagingStats {
        StagingStats {
            enabled: self.enabled,
            slots: self.ring.slots() as u32,
            capacity: self.capacity,
            ..self.stats
        }
    }
}

/// 环境开关：`R3E_UPLOAD_STAGING=0` 时关闭暂存环，回到逐段 `write_buffer`。
pub(crate) fn staging_enabled_from_env() -> bool {
    !matches!(
        std::env::var("R3E_UPLOAD_STAGING").ok().as_deref(),
        Some("0")
    )
}
