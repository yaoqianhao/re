//! HDR 后处理链的 GPU 侧（扩展十三）：离屏 HDR 目标 + 泛光 + composite。
//!
//! 渲染流程从「主通道直接写 surface」改成：
//!
//! ```text
//! 主通道 ──▶ Rgba16Float 离屏目标（线性 HDR，不再被 sRGB 目标切平）
//!            │
//!            ├─(bloom 开启)─▶ 亮部提取 → 1/2 → 1/4 → 1/8 三级降采样
//!            │                 └─▶ 1/8 → 1/4 → 1/2 逐级加法回叠
//!            ▼
//!          composite（hdr + bloom·强度 → 曝光 → 色调映射 → FXAA）──▶ sRGB surface
//!            │
//!            ├─▶ 文本通道（Load）  ─┐
//!            └─▶ UI 通道（Load）   ─┴─▶ 最终画面
//! ```
//!
//! 为什么文本 / UI 放在 composite **之后**：这两个通道的顶点色是 LDR 用户数据
//! （脚本里写的 `[1,0,0,1]` 就是想要的红色），不该被曝光与 ACES 二次调色；
//! 放在 composite 之前会让 HUD 随场景曝光一起漂移，这在编辑器里立刻能看出来。
//!
//! 认证口径（矩阵在哪）：所有数值公式的权威实现是 `r3e_core::postfx`，
//! `postfx.wgsl` 只是搬运，二者由 `tests/shader_wgsl.rs` 的常量比对锁住。

use bytemuck::{Pod, Zeroable};
use r3e_core::postfx::{
    exposure_scale, PostFxSettings, TonemapMode, DEFAULT_BLOOM_INTENSITY, DEFAULT_BLOOM_KNEE,
    DEFAULT_BLOOM_THRESHOLD,
};

/// 离屏 HDR 目标的像素格式：16 位浮点，线性空间，不做 sRGB 编解码。
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// 泛光的降采样级数：1/2、1/4、1/8。再细的层级在小窗口上会退化成一两个像素，
/// 只会把光晕抖成噪点。
pub const BLOOM_LEVELS: usize = 3;

/// 每个泛光层级相对 HDR 目标的分辨率除数（依次 2、4、8）。
const BLOOM_DIVISORS: [u32; BLOOM_LEVELS] = [2, 4, 8];

/// 一帧里最多提交几个全屏 pass（亮部 1 + 降采样 2 + 上采样 2 + composite 1）。
pub const MAX_PASSES_PER_FRAME: u32 = 1 + (BLOOM_LEVELS as u32 - 1) * 2 + 1;

/// 一个 pass 的参数块。字段顺序 / 大小与 `postfx.wgsl` 的 `struct Params` 严格一致
/// （64 字节，全部 4 字节对齐，`vec2<f32>` 落在 32 / 48 偏移上）。
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Pod, Zeroable)]
struct PostFxParams {
    exposure_scale: f32,
    tonemap: u32,
    bloom_intensity: f32,
    bloom_enabled: u32,
    fxaa_enabled: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    hdr_texel: [f32; 2],
    threshold: f32,
    knee: f32,
    src_texel: [f32; 2],
    _pad3: [f32; 2],
}

const _: () = assert!(std::mem::size_of::<PostFxParams>() == 64);

/// 本帧实际提交的 pass 统计，供 `--run-seconds` 探针打印。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostFxStats {
    /// 泛光链提交的 pass 数（亮部 + 降采样 + 上采样）。0 表示泛光关闭。
    pub bloom_passes: u32,
    /// composite 是否提交。恒为 1——它是把画面送到屏幕的唯一通路。
    pub composite_passes: u32,
    /// 本帧生效的泛光层级数。
    pub bloom_levels: u32,
}

/// 一组「纹理 + 采样器」的绑定组（group 1）。`aux` 只给 composite 用。
struct SourceBinding {
    bind_group: wgpu::BindGroup,
}

/// 一个 pass 的参数缓冲 + 它的 group(0) 绑定组。
struct ParamSlot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// 单个泛光层级：分辨率 + 纹理 + 视图 + 作为「源」时的绑定组。
struct BloomLevel {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    source: SourceBinding,
}

pub struct PostFxPass {
    surface_format: wgpu::TextureFormat,
    size: (u32, u32),
    hdr_texture: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    /// HDR 目标作为「源」时的绑定组（亮部提取、composite 的主输入）。
    hdr_source: SourceBinding,
    /// composite 专用：主输入 = HDR，副输入 = 泛光一级，遮蔽段 = 当前 AO 图。
    ///
    /// 与 `hdr_source` 分开是因为 composite 要多吃一张遮蔽图，而泛光链不需要；
    /// 分成两个绑定组后，AO 换图只需重建这一个，泛光链的绑定组保持不动。
    composite_source: SourceBinding,
    /// 1×1 全白遮蔽图：AO 关闭时绑它，采样恒为 1（画面与扩展十六逐像素一致）。
    _ao_placeholder: wgpu::Texture,
    ao_placeholder_view: wgpu::TextureView,
    /// 当前绑在 composite 上的遮蔽图（`None` = 用占位图）。
    ao_bound: Option<wgpu::TextureView>,
    /// 与 `ao_bound` 配对的世代号，0 = 占位。判等用它，不去比较视图句柄。
    ao_key: u64,
    levels: Vec<BloomLevel>,
    sampler: wgpu::Sampler,
    source_layout: wgpu::BindGroupLayout,
    /// 顺序：亮部、降采样×2、上采样×2、composite。
    slots: Vec<ParamSlot>,
    prefilter_pipeline: wgpu::RenderPipeline,
    downsample_pipeline: wgpu::RenderPipeline,
    upsample_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    stats: PostFxStats,
    /// 最近一帧真正生效的设置（探针与测试读它，比从场景反推可靠）。
    applied: PostFxSettings,
    /// HDR 目标被重建过几次。只在窗口尺寸变化时增长。
    rebuilds: u64,
}

impl PostFxPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-postfx-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // 线性过滤：上采样时相邻像素自动插值，光晕不受块状台阶影响
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-postfx-source-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // 遮蔽图（扩展十七）。只有 composite 会读它，泛光 pass 照样要绑
                // 一份（绑占位），否则布局不一致、管线创建直接失败。
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-postfx-params-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-postfx-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("postfx.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-postfx-pipeline-layout"),
            bind_group_layouts: &[Some(&params_layout), Some(&source_layout)],
            immediate_size: 0,
        });

        let prefilter_pipeline = fullscreen_pipeline(
            device,
            "r3e-postfx-prefilter",
            &pipeline_layout,
            &shader,
            "fs_prefilter",
            HDR_FORMAT,
            wgpu::BlendState::REPLACE,
        );
        let downsample_pipeline = fullscreen_pipeline(
            device,
            "r3e-postfx-downsample",
            &pipeline_layout,
            &shader,
            "fs_downsample",
            HDR_FORMAT,
            wgpu::BlendState::REPLACE,
        );
        let upsample_pipeline = fullscreen_pipeline(
            device,
            "r3e-postfx-upsample",
            &pipeline_layout,
            &shader,
            "fs_upsample",
            HDR_FORMAT,
            // 加法混合：把粗层光晕**累加**到细层已降采样的结果上
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::REPLACE,
            },
        );
        let composite_pipeline = fullscreen_pipeline(
            device,
            "r3e-postfx-composite",
            &pipeline_layout,
            &shader,
            "fs_composite",
            surface_format,
            wgpu::BlendState::REPLACE,
        );

        // 亮部 1 + 降采样 (级数-1) + 上采样 (级数-1) + composite 1
        let slot_count = MAX_PASSES_PER_FRAME as usize;
        let mut slots = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-postfx-params"),
                size: std::mem::size_of::<PostFxParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("r3e-postfx-params-bg"),
                layout: &params_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            slots.push(ParamSlot { buffer, bind_group });
        }

        let width = size.0.max(1);
        let height = size.1.max(1);
        let (hdr_texture, hdr_view) = create_hdr_target(device, width, height);
        let (ao_placeholder, ao_placeholder_view) = create_ao_placeholder(device, queue);
        let hdr_source = SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &source_layout,
                &hdr_view,
                &hdr_view,
                &ao_placeholder_view,
                &sampler,
            ),
        };

        // 先按「无遮蔽」克隆一份，`rebuild_bloom_chain` 会立刻用正版覆盖
        let composite_source = SourceBinding {
            bind_group: hdr_source.bind_group.clone(),
        };

        let mut this = Self {
            surface_format,
            size: (width, height),
            hdr_texture,
            hdr_view,
            hdr_source,
            // 真值在 `rebuild_bloom_chain` 里构建（要等泛光一级落地后才能绑副输入）
            composite_source,
            _ao_placeholder: ao_placeholder,
            ao_placeholder_view,
            ao_bound: None,
            ao_key: 0,
            levels: Vec::new(),
            sampler,
            source_layout,
            slots,
            prefilter_pipeline,
            downsample_pipeline,
            upsample_pipeline,
            composite_pipeline,
            stats: PostFxStats::default(),
            applied: PostFxSettings::default(),
            rebuilds: 0,
        };
        this.rebuild_bloom_chain(device);
        this
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    /// 主通道的颜色附件：线性 HDR 离屏目标。
    pub fn hdr_view(&self) -> &wgpu::TextureView {
        &self.hdr_view
    }

    pub fn stats(&self) -> PostFxStats {
        self.stats
    }

    /// 最近一帧真正生效的设置。
    /// composite 当前采样的遮蔽图世代号（0 = 占位 / 未开 AO）。
    pub fn ao_key(&self) -> u64 {
        self.ao_key
    }

    /// 指定 composite 要乘的遮蔽图。`key` 是调用方的世代号：同一个 key 重复
    /// 传不会重建绑定组，因此可以每帧调用（渲染循环里就是这么用的）。
    ///
    /// `key = 0`（或 `view = None`）表示「本次不乘遮蔽」——绑回 1×1 白图，
    /// 是与场景真的关掉 AO 等价的视觉结果，而不是「继续用上一帧的旧 AO」。
    pub fn bind_ao(&mut self, device: &wgpu::Device, key: u64, view: Option<&wgpu::TextureView>) {
        let key = if view.is_none() { 0 } else { key };
        if key == self.ao_key {
            return;
        }
        // 泛光链还没建好时无处可绑（只有 new 的中间态会走到这里）
        if self.levels.is_empty() {
            return;
        }
        self.ao_key = key;
        self.ao_bound = view.cloned();
        let ao = self.ao_bound.as_ref().unwrap_or(&self.ao_placeholder_view);
        self.composite_source = SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &self.source_layout,
                &self.hdr_view,
                &self.levels[0].view,
                ao,
                &self.sampler,
            ),
        };
    }

    pub fn applied(&self) -> PostFxSettings {
        self.applied
    }

    /// HDR 目标重建次数（窗口尺寸变化时才该增长）。
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    /// 各级泛光分辨率，供探针打印与测试断言。
    pub fn bloom_sizes(&self) -> Vec<(u32, u32)> {
        self.levels.iter().map(|l| l.size).collect()
    }

    /// 窗口尺寸变化后重建 HDR 目标与泛光链。句柄必须整体换新，
    /// 纹理不能原地 resize。
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        let (texture, view) = create_hdr_target(device, width, height);
        self.hdr_texture = texture;
        self.hdr_view = view;
        self.hdr_source = SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &self.source_layout,
                &self.hdr_view,
                &self.hdr_view,
                &self.ao_placeholder_view,
                &self.sampler,
            ),
        };
        // 遮蔽图由 SsaoPass 自己重建，这里只把世代号清回占位：
        // 下一帧 `bind_ao` 拿到新句柄会重建 composite 绑定组。
        self.ao_key = 0;
        self.ao_bound = None;
        self.rebuild_bloom_chain(device);
        self.rebuilds += 1;
    }

    fn rebuild_bloom_chain(&mut self, device: &wgpu::Device) {
        self.levels.clear();
        for divisor in BLOOM_DIVISORS {
            let w = (self.size.0 / divisor).max(1);
            let h = (self.size.1 / divisor).max(1);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("r3e-bloom-target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = make_source_bind_group(
                device,
                &self.source_layout,
                &view,
                &view,
                &self.ao_placeholder_view,
                &self.sampler,
            );
            self.levels.push(BloomLevel {
                size: (w, h),
                _texture: texture,
                view,
                source: SourceBinding { bind_group },
            });
        }
        // composite 的副输入固定是「最终累积」的泛光一级（1/2 分辨率）。
        // 泛光关闭时 shader 走不到 t_aux，绑什么都不影响结果，因此不必随开关重建。
        let (hdr_view, bloom_view) = (&self.hdr_view, &self.levels[0].view);
        let ao_view = self.ao_bound.as_ref().unwrap_or(&self.ao_placeholder_view);
        self.composite_source = SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &self.source_layout,
                hdr_view,
                bloom_view,
                ao_view,
                &self.sampler,
            ),
        };
    }

    /// 本帧要提交的 pass 数（不改任何状态，探针可先问再渲）。
    pub fn planned_passes(settings: &PostFxSettings) -> u32 {
        let bloom = if settings.bloom.enabled {
            // 亮部 1 + 降采样 (级数-1) + 上采样 (级数-1)
            1 + (BLOOM_LEVELS as u32 - 1) * 2
        } else {
            0
        };
        bloom + 1
    }

    /// 记录本帧的全部后处理命令。`target` 是 surface 视图（文本 / UI 稍后往它上面叠）。
    ///
    /// `settings` 会先过一遍 `sanitized()`：把「场景里写了什么」与「GPU 用的是什么」
    /// 收敛到同一处，避免 NaN 一路传进 uniform。
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        settings: &PostFxSettings,
    ) {
        let fx = settings.sanitized();
        self.applied = fx;

        let hdr_texel = [1.0 / self.size.0 as f32, 1.0 / self.size.1 as f32];
        let base = PostFxParams {
            exposure_scale: exposure_scale(fx.exposure),
            tonemap: fx.tonemap.code(),
            bloom_intensity: fx.bloom.intensity,
            bloom_enabled: u32::from(fx.bloom.enabled),
            fxaa_enabled: u32::from(fx.fxaa),
            hdr_texel,
            threshold: fx.bloom.threshold,
            knee: fx.bloom.knee,
            ..Default::default()
        };

        let mut pass_count = 0u32;
        if fx.bloom.enabled {
            // pass 1：HDR → 亮部（1/2）
            let params = PostFxParams {
                src_texel: hdr_texel,
                ..base
            };
            self.upload(queue, 0, params);
            self.fullscreen_pass(
                encoder,
                "r3e-postfx-prefilter",
                &self.prefilter_pipeline,
                0,
                &self.hdr_source.bind_group,
                &self.levels[0].view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                device,
            );
            pass_count += 1;

            // pass 2..：逐级降采样（1/2 → 1/4 → 1/8）
            for level in 1..BLOOM_LEVELS {
                let src = &self.levels[level - 1];
                let src_texel = [1.0 / src.size.0 as f32, 1.0 / src.size.1 as f32];
                let params = PostFxParams { src_texel, ..base };
                self.upload(queue, level, params);
                self.fullscreen_pass(
                    encoder,
                    "r3e-postfx-downsample",
                    &self.downsample_pipeline,
                    level,
                    &self.levels[level - 1].source.bind_group,
                    &self.levels[level].view,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    device,
                );
                pass_count += 1;
            }

            // pass 2..：上采样加法回叠（1/8 → 1/4 → 1/2）
            for step in 0..BLOOM_LEVELS - 1 {
                let src_level = BLOOM_LEVELS - 1 - step; // 2, 1
                let dst_level = src_level - 1; // 1, 0
                let src_texel = {
                    let l = &self.levels[src_level];
                    [1.0 / l.size.0 as f32, 1.0 / l.size.1 as f32]
                };
                let params = PostFxParams { src_texel, ..base };
                // 每个 pass 一个独立参数槽：`write_buffer` 在本帧 submit 前统一生效，
                // 复用同一个槽会让先写的参数被后写的覆盖，所有 pass 都读到同一份数值。
                let slot = 1 + (BLOOM_LEVELS - 1) + step;
                self.upload(queue, slot, params);
                self.fullscreen_pass(
                    encoder,
                    "r3e-postfx-upsample",
                    &self.upsample_pipeline,
                    slot,
                    &self.levels[src_level].source.bind_group,
                    &self.levels[dst_level].view,
                    // Load：保留下采样阶段写进去的内容，本 pass 是「叠加」而非覆盖
                    wgpu::LoadOp::Load,
                    device,
                );
                pass_count += 1;
            }
        }

        // 最后一个 pass：composite → surface。
        let composite_slot = self.slots.len() - 1;
        self.upload(queue, composite_slot, base);
        self.fullscreen_pass(
            encoder,
            "r3e-postfx-composite",
            &self.composite_pipeline,
            composite_slot,
            &self.composite_source.bind_group,
            target,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            device,
        );
        pass_count += 1;

        self.stats = PostFxStats {
            bloom_passes: pass_count - 1,
            composite_passes: 1,
            bloom_levels: if fx.bloom.enabled {
                BLOOM_LEVELS as u32
            } else {
                0
            },
        };
    }

    fn upload(&self, queue: &wgpu::Queue, slot: usize, params: PostFxParams) {
        queue.write_buffer(&self.slots[slot].buffer, 0, bytemuck::bytes_of(&params));
    }

    #[allow(clippy::too_many_arguments)]
    fn fullscreen_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        pipeline: &wgpu::RenderPipeline,
        slot: usize,
        source: &wgpu::BindGroup,
        target: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
        _device: &wgpu::Device,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.slots[slot].bind_group, &[]);
        pass.set_bind_group(1, source, &[]);
        // 单三角形覆盖全屏，无顶点/索引缓冲
        pass.draw(0..3, 0..1);
    }
}

fn create_hdr_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-hdr-target"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn make_source_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    src: &wgpu::TextureView,
    aux: &wgpu::TextureView,
    ao: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("r3e-postfx-source-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(aux),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(ao),
            },
        ],
    })
}

/// 1×1 全白遮蔽图（R8Unorm 的 255 = 1.0）：AO 关闭时 composite 绑它，
/// 采样恒为 1，乘上去等于没乘。
fn create_ao_placeholder(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-postfx-ao-placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // 必须真的写进去：纹理创建后内容是未定义的，多数后端给 0。
    // 少了这一笔，AO 关闭时 composite 把整帧乘成黑色 —— 背景清除色也一起消失。
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[255u8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: None,
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            // 顶点坐标由 vertex_index 现算，不需要任何顶点缓冲
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            // 全屏三角形按惯例不做背面剔除：uv 翻转后绕序可能反过来，
            // 一旦剔除就整屏消失，这种 bug 只在实机上才现形。
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// 缺省泛光参数（与 `core::postfx` 同值，供探针打印与文档对照）。
pub fn default_bloom_triplet() -> (f32, f32, f32) {
    (
        DEFAULT_BLOOM_THRESHOLD,
        DEFAULT_BLOOM_KNEE,
        DEFAULT_BLOOM_INTENSITY,
    )
}

/// 色调映射模式在 GPU uniform 里的编码（与着色器的 `mode` 分支对应）。
pub fn tonemap_code(mode: TonemapMode) -> u32 {
    mode.code()
}
