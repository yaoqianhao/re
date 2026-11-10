//! 遮挡剔除（扩展二十二）的渲染侧：深度读回与金字塔缓存。
//!
//! CPU 判定本身在 [`r3e_core::occlusion`]（纯算术、可单测）；这里只解决「金字塔
//! 从哪来」这一件事：**低频**把深度预通道写出的单采样 `Depth32Float` 读回 CPU，
//! 压缩成 tile 金字塔，供**后续帧**做遮挡判定。
//!
//! 三条设计约束：
//!
//! * **可开关即零成本**：`enabled = false` 时 [`OcclusionCuller::begin_frame`] 返回
//!   `false`，本帧不 poll、不编码 copy、不读回，`prepare_instances` 也不做判定——
//!   GPU 命令与实例数都与加这个功能之前逐字节一致。
//! * **低频 + 非阻塞**：读回是「发起 -> 若干帧后取回」的两段式。`request` 只把
//!   copy 编进本帧命令缓冲；结果在之后某帧的开头用 `device.poll(PollType::Poll)`
//!   非阻塞地取。没取到就一直沿用上一份金字塔，绝不原地等待 GPU（会卡帧）。
//! * **保守 fail-open**：金字塔未就绪（首帧、刚 resize、读回失败）、尺寸与当前
//!   画面对不上时一律判「可见」。开关关闭时还会主动丢弃已有金字塔，避免下次打开
//!   时拿着一份过期画面去剔东西。
//!
//! 读回的数据量：`width x height x 4` 字节按 256 字节行对齐（1080p ≈ 8.3 MiB），
//! 每 [`DEFAULT_READBACK_INTERVAL`] 帧一次。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use r3e_core::occlusion::{
    plan_occlusion, test_aabb, DepthPyramid, OcclusionCullPlan, OcclusionSettings, OcclusionVerdict,
};
use r3e_core::{Aabb, CasterTree};

/// 默认读回间隔（帧）。4 帧约等于 60fps 下 15Hz，物件移动的滞后肉眼基本看不出来。
pub const DEFAULT_READBACK_INTERVAL: u64 = 4;

/// 读回间隔下限：每帧读回 8 MiB 纯属浪费，且金字塔本来就允许滞后。
pub const MIN_READBACK_INTERVAL: u64 = 1;

/// 读回间隔上限：再稀就会在快速移动的镜头下明显滞后。
pub const MAX_READBACK_INTERVAL: u64 = 60;

/// 一帧的遮挡剔除统计（进 [`crate::RenderStats::occlusion`]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OcclusionStats {
    /// 本帧是否启用（相机 `enabled` 且金字塔就绪才算真正在判）。
    pub enabled: bool,
    /// 本帧做过遮挡判定的实例数（已通过视锥剔除的那部分）。
    ///
    /// 扩展三十起这不再是「全部不透明实例」：被整枝跳过的那部分不计入，见 `pruned`。
    pub tested: u32,
    /// 本帧因遮挡被剔除的实例数。0 表示没剔到或开关关闭。
    pub occluded: u32,
    /// 本帧是否取到了新的深度金字塔（低频读回，多数帧为 false）。
    pub readback: bool,
    /// 当前金字塔层数；0 表示还没有可用金字塔。
    pub levels: u32,
    /// 扩展三十：本帧是否走了「按子树预筛」的层级路径。
    /// false = 金字塔没就绪 / 开关关闭，此时退回逐实例判定。
    pub hierarchy: bool,
    /// 扩展三十：本帧做的层级盒判定数（被下探的树节点 + 混合节点自身盒）。
    /// 远小于 `tested` 说明整枝跳过真的在起作用。
    pub box_tests: u32,
    /// 扩展三十：没做实例级判定就被判遮挡的实例数（整枝跳过 + 叶子直接被裁）。
    /// `tested + pruned` 恒等于「过了视锥的不透明实例数」。
    pub pruned: u32,
    /// 扩展三十：`pruned` 里来自多实例子树的部分 —— 层级预筛的净收益。
    /// 叶子被裁在逐实例判定里也是一次 AABB，省不下来。
    pub branch_pruned: u32,
}

/// 一次读回的 buffer 元信息。buffer 本体留在 [`OcclusionCuller`] 里复用。
#[derive(Debug, Clone, Copy)]
struct ReadbackLayout {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
}

impl ReadbackLayout {
    fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let unpadded = width.checked_mul(4)?;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align).checked_mul(align)?;
        Some(Self {
            width,
            height,
            padded_bytes_per_row: padded,
        })
    }

    fn byte_len(&self) -> u64 {
        self.padded_bytes_per_row as u64 * self.height as u64
    }
}

/// 深度读回 + 金字塔缓存。常驻在 [`crate::Renderer`] 里，不启用时几乎不占资源
/// （不建 buffer、不 poll）。
pub struct OcclusionCuller {
    /// 上一份**已解析完成**的金字塔。判定只读它，因此永远比画面滞后若干帧。
    pyramid: Option<DepthPyramid>,
    buffer: Option<wgpu::Buffer>,
    layout: Option<ReadbackLayout>,
    /// 已发起 map_async、等待回调的读回；`Some` 期间不再发起新的 copy。
    inflight: bool,
    mapped_ok: Arc<AtomicBool>,
    /// 上一次发起读回的帧号（用于限流）。
    last_request: Option<u64>,
    interval: u64,
    stats: OcclusionStats,
}

impl Default for OcclusionCuller {
    fn default() -> Self {
        Self::new()
    }
}

impl OcclusionCuller {
    pub fn new() -> Self {
        Self {
            pyramid: None,
            buffer: None,
            layout: None,
            inflight: false,
            mapped_ok: Arc::new(AtomicBool::new(false)),
            last_request: None,
            interval: DEFAULT_READBACK_INTERVAL,
            stats: OcclusionStats::default(),
        }
    }

    /// 已就绪的金字塔（判定入口）。`None` = 还没有可用深度，调用方必须 fail-open。
    pub fn pyramid(&self) -> Option<&DepthPyramid> {
        self.pyramid.as_ref()
    }

    pub fn stats(&self) -> OcclusionStats {
        self.stats
    }

    pub fn readback_interval(&self) -> u64 {
        self.interval
    }

    /// 设置读回间隔，收敛到 [`MIN_READBACK_INTERVAL`..=[`MAX_READBACK_INTERVAL`]]。
    pub fn set_readback_interval(&mut self, frames: u64) -> u64 {
        self.interval = frames.clamp(MIN_READBACK_INTERVAL, MAX_READBACK_INTERVAL);
        self.interval
    }

    /// 丢掉已有金字塔与读回状态。resize / 关闭开关时调用：
    /// 尺寸变了金字塔就与当前画面对不上，留着只会误剔。
    pub fn reset(&mut self) {
        self.pyramid = None;
        self.last_request = None;
        self.stats = OcclusionStats::default();
    }

    /// 本帧开头调用：先把上一轮读回取回来（如果有），再决定本帧要不要判。
    ///
    /// 返回 `true` 表示「本帧有可用金字塔，可以做遮挡判定」。关闭时顺手 `reset`，
    /// 保证重新打开后不会拿着关闭期间留下的陈年深度去剔东西。
    pub fn begin_frame(&mut self, device: &wgpu::Device, settings: OcclusionSettings) -> bool {
        // 每帧重置逐帧计数，`levels` 是金字塔的固有属性、跨帧保留
        self.stats.tested = 0;
        self.stats.occluded = 0;
        self.stats.readback = false;
        // 扩展三十：层级预筛的逐帧计数同样每帧清零，由 `record_hierarchy` 累加
        self.stats.hierarchy = false;
        self.stats.box_tests = 0;
        self.stats.pruned = 0;
        self.stats.branch_pruned = 0;
        if !settings.enabled {
            if self.pyramid.is_some() {
                self.reset();
            }
            self.stats = OcclusionStats::default();
            return false;
        }
        self.poll(device, settings.tile_size);
        self.stats.levels = self.pyramid.as_ref().map_or(0, |p| p.level_count() as u32);
        self.stats.enabled = self.pyramid.is_some();
        self.stats.enabled
    }

    /// 非阻塞地把已完成的读回解析成金字塔。
    fn poll(&mut self, device: &wgpu::Device, tile: u32) {
        if !self.inflight {
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        if !self.mapped_ok.load(Ordering::Acquire) {
            return;
        }
        self.inflight = false;
        self.mapped_ok.store(false, Ordering::Release);
        let (Some(buffer), Some(layout)) = (self.buffer.as_ref(), self.layout) else {
            return;
        };
        // 逐行剥掉 256 字节对齐的填充，得到紧凑的 width * height 深度数组。
        // 闭包包一层：`get_mapped_range` 失败 / 越界都返回 `None`，之后统一 unmap。
        let expected = (layout.width as usize) * (layout.height as usize);
        let depths = {
            let slice = buffer.slice(..);
            (|| -> Option<Vec<f32>> {
                let data = slice.get_mapped_range().ok()?;
                let row_bytes = layout.padded_bytes_per_row as usize;
                let row_floats = layout.width as usize;
                let mut depths: Vec<f32> = Vec::with_capacity(expected);
                for row in 0..layout.height as usize {
                    let start = row * row_bytes;
                    let end = start + row_floats * 4;
                    let chunk = data.get(start..end)?;
                    for px in 0..row_floats {
                        depths.push(f32::from_le_bytes([
                            chunk[px * 4],
                            chunk[px * 4 + 1],
                            chunk[px * 4 + 2],
                            chunk[px * 4 + 3],
                        ]));
                    }
                }
                Some(depths)
            })()
        };
        buffer.unmap();
        let Some(depths) = depths else {
            return;
        };
        if depths.len() != expected {
            return;
        }
        if let Some(p) = DepthPyramid::build(layout.width, layout.height, tile, &depths) {
            self.pyramid = Some(p);
            self.stats.readback = true;
            self.stats.levels = self.pyramid.as_ref().map_or(0, |p| p.level_count() as u32);
        }
    }

    /// 把本帧的深度 copy 编进命令缓冲。返回 `true` 表示真的编了：
    /// 调用方必须在 `queue.submit` **之后**调一次 [`Self::after_submit`]。
    ///
    /// 有读回在飞 / 未到间隔 / 尺寸非法时返回 `false`，本帧不产生任何额外命令。
    pub fn encode_readback(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        width: u32,
        height: u32,
        frame: u64,
    ) -> bool {
        if self.inflight {
            return false;
        }
        if let Some(last) = self.last_request {
            if frame.wrapping_sub(last) < self.interval {
                return false;
            }
        }
        let Some(layout) = ReadbackLayout::new(width, height) else {
            return false;
        };
        // 尺寸变了（resize）就重建 buffer —— 旧 buffer 装不下新画面
        if self.layout.map(|l| (l.width, l.height)) != Some((layout.width, layout.height)) {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-occlusion-depth-readback"),
                size: layout.byte_len(),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
            self.layout = Some(layout);
            // 尺寸变了，旧金字塔立刻作废
            self.pyramid = None;
            self.stats.levels = 0;
        }
        let Some(buffer) = self.buffer.as_ref() else {
            return false;
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(layout.padded_bytes_per_row),
                    rows_per_image: Some(layout.height),
                },
            },
            wgpu::Extent3d {
                width: layout.width,
                height: layout.height,
                depth_or_array_layers: 1,
            },
        );
        self.last_request = Some(frame);
        true
    }

    /// submit 之后启动 map_async（回调只置一个原子标志，不阻塞）。
    pub fn after_submit(&mut self) {
        if self.last_request.is_none() || self.inflight {
            return;
        }
        let Some(buffer) = self.buffer.as_ref() else {
            return;
        };
        let flag = Arc::clone(&self.mapped_ok);
        flag.store(false, Ordering::Release);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                // 失败也置位：poll 里会因为 unmapped 直接跳过本次解析（结果不变旧金字塔）
                if result.is_ok() {
                    flag.store(true, Ordering::Release);
                }
            });
        self.inflight = true;
    }

    /// 对一个世界 AABB 做两阶段遮挡判定，并累计统计。
    ///
    /// 金字塔没就绪 / 参数中性 / 投影失败时一律返回 `false`（可见）。
    /// 扩展三十起这是**退化路径**：金字塔就绪时调用方走 [`Self::plan`]，
    /// 只有没建成层级树（`None` 树 / 节点不在表里）才逐个问这里。
    pub fn test(
        &mut self,
        settings: &OcclusionSettings,
        aabb: &Aabb,
        view_projection: &glam::Mat4,
        width: u32,
        height: u32,
    ) -> bool {
        self.stats.tested += 1;
        let Some(pyramid) = self.pyramid.as_ref() else {
            return false;
        };
        match test_aabb(pyramid, aabb, view_projection, width, height, settings) {
            OcclusionVerdict::Occluded => {
                self.stats.occluded += 1;
                true
            }
            _ => false,
        }
    }

    /// 扩展三十：按层级树做遮挡预筛，返回与树节点一一对应的「照画」掩码。
    ///
    /// 金字塔没就绪（首帧 / 刚 resize / 开关关闭）时返回 `None`，调用方必须退回
    /// 逐实例 [`Self::test`]。判定口径见 [`r3e_core::occlusion::plan_occlusion`]：
    /// 子树盒被判遮挡 ⇒ 枝内实例一个都不做金字塔查询。
    ///
    /// 只读 `&self`：先拿 plan、再 [`Self::record_hierarchy`] 记账，调用处不必
    /// 在两个 `&mut self` 方法之间来回借。
    pub fn plan(
        &self,
        settings: &OcclusionSettings,
        tree: &CasterTree,
        view_projection: &glam::Mat4,
        width: u32,
        height: u32,
    ) -> Option<OcclusionCullPlan> {
        let pyramid = self.pyramid.as_ref()?;
        Some(plan_occlusion(
            tree,
            pyramid,
            view_projection,
            width,
            height,
            settings,
        ))
    }

    /// 把一次层级预筛的结果并进本帧统计。
    ///
    /// `tested` 记的是「真做了实例级判定的实例数」，因此加的是
    /// `instance_tests` 而不是过视锥的实例总数 —— 差的那部分体现在 `pruned`。
    pub fn record_hierarchy(&mut self, plan: &OcclusionCullPlan) {
        self.stats.hierarchy = true;
        self.stats.tested = self.stats.tested.saturating_add(plan.counts.instance_tests);
        self.stats.occluded = self.stats.occluded.saturating_add(plan.occluded);
        self.stats.box_tests = self.stats.box_tests.saturating_add(plan.counts.box_tests);
        self.stats.pruned = self.stats.pruned.saturating_add(plan.counts.pruned);
        self.stats.branch_pruned = self
            .stats
            .branch_pruned
            .saturating_add(plan.counts.branch_pruned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rejects_degenerate_size() {
        assert!(ReadbackLayout::new(0, 720).is_none());
        assert!(ReadbackLayout::new(1280, 0).is_none());
        assert!(ReadbackLayout::new(0, 0).is_none());
    }

    #[test]
    fn layout_pads_rows_to_copy_alignment() {
        // 1281 * 4 = 5124 字节，不整除 256，必须向上补齐
        let layout = ReadbackLayout::new(1281, 720).expect("layout");
        assert_eq!(
            layout.padded_bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
            0
        );
        assert!(layout.padded_bytes_per_row as usize >= 1281 * 4);
        assert_eq!(layout.byte_len(), layout.padded_bytes_per_row as u64 * 720);
    }

    #[test]
    fn layout_exact_multiple_is_not_padded_further() {
        // 64 * 4 = 256，正好一行一个对齐单位
        let layout = ReadbackLayout::new(64, 8).expect("layout");
        assert_eq!(layout.padded_bytes_per_row, 256);
        assert_eq!(layout.byte_len(), 2048);
    }

    #[test]
    fn interval_is_clamped_to_safe_range() {
        let mut culler = OcclusionCuller::new();
        assert_eq!(culler.readback_interval(), DEFAULT_READBACK_INTERVAL);
        assert_eq!(culler.set_readback_interval(0), MIN_READBACK_INTERVAL);
        assert_eq!(culler.set_readback_interval(1_000), MAX_READBACK_INTERVAL);
        assert_eq!(culler.set_readback_interval(7), 7);
    }

    #[test]
    fn fresh_culler_is_fail_open() {
        // 没有金字塔就绝不允许剔东西：调用方全靠这个返回值决定 fail-open
        let culler = OcclusionCuller::new();
        assert!(culler.pyramid().is_none());
        let stats = culler.stats();
        assert!(!stats.enabled);
        assert_eq!(stats.levels, 0);
        assert_eq!(stats.occluded, 0);
    }

    #[test]
    fn reset_drops_pyramid_and_stats() {
        let mut culler = OcclusionCuller::new();
        culler.stats.enabled = true;
        culler.stats.levels = 3;
        culler.stats.occluded = 9;
        culler.reset();
        assert!(culler.pyramid().is_none());
        let stats = culler.stats();
        assert!(!stats.enabled);
        assert_eq!(stats.levels, 0);
        assert_eq!(stats.occluded, 0);
    }

    #[test]
    fn plan_needs_a_pyramid() {
        // 金字塔没就绪 → `None`，调用方必须退回逐实例 `test`（fail-open）
        let culler = OcclusionCuller::new();
        let tree = CasterTree::default();
        let vp = glam::Mat4::IDENTITY;
        assert!(culler
            .plan(&OcclusionSettings::default(), &tree, &vp, 64, 64)
            .is_none());
        let stats = culler.stats();
        assert!(!stats.hierarchy);
        assert_eq!(stats.box_tests, 0);
        assert_eq!(stats.pruned, 0);
    }

    #[test]
    fn record_hierarchy_accumulates_counts() {
        // 记账口径：`tested` 只加真正做了实例判定的量，省下的落在 `pruned` /
        // `branch_pruned`；多次调用累加而不是覆盖。
        let mut culler = OcclusionCuller::new();
        let plan = OcclusionCullPlan {
            mask: vec![true, false, true],
            counts: r3e_core::LevelCounts {
                box_tests: 3,
                instance_tests: 2,
                pruned: 4,
                branch_pruned: 4,
            },
            occluded: 4,
        };
        culler.record_hierarchy(&plan);
        culler.record_hierarchy(&plan);
        let stats = culler.stats();
        assert!(stats.hierarchy);
        assert_eq!(stats.box_tests, 6);
        assert_eq!(stats.tested, 4);
        assert_eq!(stats.pruned, 8);
        assert_eq!(stats.branch_pruned, 8);
        assert_eq!(stats.occluded, 8);
    }
}
