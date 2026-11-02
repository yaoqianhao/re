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
    exposure_scale, PostFxSettings, TonemapMode, DEFAULT_BLOOM_INTENSITY,
    DEFAULT_BLOOM_KNEE, DEFAULT_BLOOM_THRESHOLD,
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
    /// composite 专用：主输入 = HDR，副输入 = 泛光一级。
    hdr_bloom_source: Option<SourceBinding>,
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
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
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
        let hdr_source = SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &source_layout,
                &hdr_view,
                &hdr_view,
                &sampler,
            ),
        };

        let mut this = Self {
            surface_format,
            size: (width, height),
            hdr_texture,
            hdr_view,
            hdr_source,
            hdr_bloom_source: None,
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
                &self.sampler,
            ),
        };
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
            let bind_group =
                make_source_bind_group(device, &self.source_layout, &view, &view, &self.sampler);
            self.levels.push(BloomLevel {
                size: (w, h),
                _texture: texture,
                view,
                source: SourceBinding { bind_group },
            });
        }
        // composite 的副输入固定是「最终累积」的泛光一级（1/2 分辨率）
        let (hdr_view, bloom_view) = (&self.hdr_view, &self.levels[0].view);
        self.hdr_bloom_source = Some(SourceBinding {
            bind_group: make_source_bind_group(
                device,
                &self.source_layout,
                hdr_view,
                bloom_view,
                &self.sampler,
            ),
        });
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
                let params = PostFxParams {
                    src_texel,
                    ..base
                };
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
                let params = PostFxParams {
                    src_texel,
                    ..base
                };
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
        let source = if fx.bloom.enabled {
            self.hdr_bloom_source
                .as_ref()
                .map(|s| &s.bind_group)
                .unwrap_or(&self.hdr_source.bind_group)
        } else {
            &self.hdr_source.bind_group
        };
        self.fullscreen_pass(
            encoder,
            "r3e-postfx-composite",
            &self.composite_pipeline,
            composite_slot,
            source,
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
        ],
    })
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
