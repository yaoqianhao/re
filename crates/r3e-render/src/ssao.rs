//! SSAO（扩展十七）的 GPU 侧接线：深度 → 遮蔽图 → 4×4 散焦 → composite。
//!
//! 位置在渲染流程里非常固定，必须夹在**主通道之后、composite 之前**：
//!
//! ```text
//! 主通道（写 HDR 颜色 + 深度）
//!    │
//!    ├─▶ [本模块] 深度 → 遮蔽图（全尺寸）→ 4×4 散焦
//!    ▼
//! composite（HDR × AO → 曝光 → 色调映射 → FXAA）──▶ surface
//! ```
//!
//! 为什么不能更早：遮蔽要用「已经画完的」深度缓冲反推位置，主通道没跑完
//! 深度里只有清屏值；为什么不能更晚：composite 是唯一写 surface 的通道，
//! 一旦错过它，AO 就没地方乘回颜色。
//!
//! 认证口径与 `postfx` 一致：数值公式的权威实现是 `r3e_core::ssao`，
//! `ssao.wgsl` 只是搬运，二者由 `tests/ssao_shader.rs` 锁住。

use bytemuck::{Pod, Zeroable};
use r3e_core::ssao::{kernel_uniform, SsaoSettings, MAX_SSAO_SAMPLES};
use r3e_core::Camera;

/// 遮蔽图的像素格式：单通道 8 位无符号归一化。
///
/// 遮蔽系数只有 `[0,1]` 一维、且经过 4×4 散焦，8 位精度足够；
/// 用 `Rgba16Float` 会让这张全尺寸纹理的带宽翻 8 倍，肉眼上却没有差别。
pub const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// 每帧提交的 pass 数：生成 1 + 散焦 1。
pub const AO_PASSES_PER_FRAME: u32 = 2;

/// 一个 pass 的参数块。字段顺序 / 大小与 `ssao.wgsl` 的 `struct SsaoParams`
/// 严格一致（416 字节 = 2 个 mat4 + 16 个 vec4 + 2 个 vec4）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SsaoParams {
    proj: [f32; 16],
    inv_proj: [f32; 16],
    kernel: [[f32; 4]; MAX_SSAO_SAMPLES as usize],
    /// xy = 1/尺寸，z = 半径，w = 偏置
    screen: [f32; 4],
    /// x = 强度，y = 样本数
    misc: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<SsaoParams>() == 416);
const _: () = assert!(std::mem::size_of::<SsaoParams>() % 16 == 0);

/// 本帧实际提交的 SSAO 统计，供 `--run-seconds` 探针打印。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SsaoStats {
    /// 是否真的跑了（场景开了遮蔽）。
    pub enabled: bool,
    /// 提交的 pass 数（生成 + 散焦）。0 = 整条跳过。
    pub passes: u32,
    /// 实际生效的半球核样本数。
    pub samples: u32,
    /// 遮蔽图分辨率（应与 HDR 目标一致）。
    pub size: (u32, u32),
}

pub struct SsaoPass {
    size: (u32, u32),
    params_buffer: wgpu::Buffer,
    params_bind_group: wgpu::BindGroup,
    /// 生成 pass 的输入：主通道深度缓冲。
    depth_layout: wgpu::BindGroupLayout,
    depth_bind_group: wgpu::BindGroup,
    /// 散焦 pass 的输入：刚生成的遮蔽图。
    ao_layout: wgpu::BindGroupLayout,
    blur_bind_group: wgpu::BindGroup,
    ssao_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    _ao_raw_texture: wgpu::Texture,
    ao_raw_view: wgpu::TextureView,
    _ao_final_texture: wgpu::Texture,
    ao_final_view: wgpu::TextureView,
    stats: SsaoStats,
    applied: SsaoSettings,
    /// 遮蔽图世代号：new 时为 1，每次按新尺寸重建 +1。postfx 用它判等，
    /// 避免每帧重建 composite 绑定组。
    generation: u64,
}

impl SsaoPass {
    pub fn new(device: &wgpu::Device, size: (u32, u32), depth_view: &wgpu::TextureView) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-ssao-params"),
            size: std::mem::size_of::<SsaoParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-ssao-params-bgl"),
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
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-ssao-params-bg"),
            layout: &params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        // 深度只做 textureLoad：不需要采样器，也不需要「可过滤」类型标注
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-ssao-depth-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        // 遮蔽图同样只做 textureLoad；binding 号与 shader 的 `src_ao` 对齐（1）
        let ao_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-ssao-ao-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-ssao-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ssao.wgsl").into()),
        });

        let ssao_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-ssao-pipeline-layout"),
            bind_group_layouts: &[Some(&params_layout), Some(&depth_layout)],
            immediate_size: 0,
        });
        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-ssao-blur-pipeline-layout"),
            bind_group_layouts: &[Some(&params_layout), Some(&ao_layout)],
            immediate_size: 0,
        });

        let ssao_pipeline = ao_pipeline(device, "r3e-ssao", &ssao_layout, &shader, "fs_ssao");
        let blur_pipeline = ao_pipeline(device, "r3e-ssao-blur", &blur_layout, &shader, "fs_blur");

        let (ao_raw_texture, ao_raw_view) = create_ao_target(device, size, "r3e-ssao-raw");
        let (ao_final_texture, ao_final_view) = create_ao_target(device, size, "r3e-ssao-blur");
        let depth_bind_group = depth_bind(device, &depth_layout, depth_view);
        let blur_bind_group = ao_bind(device, &ao_layout, &ao_raw_view);

        Self {
            size,
            params_buffer,
            params_bind_group,
            depth_layout,
            depth_bind_group,
            ao_layout,
            blur_bind_group,
            ssao_pipeline,
            blur_pipeline,
            _ao_raw_texture: ao_raw_texture,
            ao_raw_view,
            _ao_final_texture: ao_final_texture,
            ao_final_view,
            stats: SsaoStats::default(),
            applied: SsaoSettings::default(),
            generation: 1,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 散焦后的遮蔽图：composite 采样它。
    pub fn ao_view(&self) -> &wgpu::TextureView {
        &self.ao_final_view
    }

    pub fn stats(&self) -> SsaoStats {
        self.stats
    }

    /// 最近一帧真正生效的设置。
    pub fn applied(&self) -> SsaoSettings {
        self.applied
    }

    /// 遮蔽图世代号。句柄没换就不该变，postfx 靠它决定要不要重建绑定组。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 窗口尺寸变化后重建两张遮蔽图与相关绑定组。
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        depth_view: &wgpu::TextureView,
    ) {
        let size = (width.max(1), height.max(1));
        let size_changed = size != self.size;
        self.size = size;
        // 深度视图在 resize 里换了新句柄，即使尺寸相同也得重绑
        self.depth_bind_group = depth_bind(device, &self.depth_layout, depth_view);
        if !size_changed {
            return;
        }
        let (raw_tex, raw_view) = create_ao_target(device, size, "r3e-ssao-raw");
        let (final_tex, final_view) = create_ao_target(device, size, "r3e-ssao-blur");
        self._ao_raw_texture = raw_tex;
        self.ao_raw_view = raw_view;
        self._ao_final_texture = final_tex;
        self.ao_final_view = final_view;
        self.blur_bind_group = ao_bind(device, &self.ao_layout, &self.ao_raw_view);
        self.generation += 1;
    }

    /// 本帧要提交的 pass 数（不改状态，探针可先问再渲）。
    pub fn planned_passes(settings: &SsaoSettings) -> u32 {
        if settings.sanitized().is_neutral() {
            0
        } else {
            AO_PASSES_PER_FRAME
        }
    }

    /// 记录本帧的遮蔽计算。必须在主通道之后、`postfx.render` 之前调用。
    ///
    /// 关闭时（`enabled = false`）一次 GPU 命令都不发、`stats.passes` 归零；
    /// 此时 composite 采样的是一张 1×1 白色占位纹理，画面与扩展十六逐像素一致。
    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        settings: &SsaoSettings,
    ) {
        let s = settings.sanitized();
        self.applied = s;
        let samples = s.effective_samples();

        if s.is_neutral() {
            self.stats = SsaoStats {
                enabled: false,
                passes: 0,
                samples: samples as u32,
                size: self.size,
            };
            return;
        }

        let proj = camera.projection();
        let mut params = SsaoParams {
            proj: proj.to_cols_array(),
            inv_proj: proj.inverse().to_cols_array(),
            kernel: [[0.0; 4]; MAX_SSAO_SAMPLES as usize],
            screen: [
                1.0 / self.size.0 as f32,
                1.0 / self.size.1 as f32,
                s.radius,
                s.bias,
            ],
            misc: [s.intensity, samples as f32, 0.0, 0.0],
        };
        for (slot, v) in params
            .kernel
            .iter_mut()
            .zip(kernel_uniform(samples, s.radius))
        {
            *slot = [v.x, v.y, v.z, 0.0];
        }
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        // pass 1：深度 → 遮蔽图
        self.fullscreen(
            encoder,
            "r3e-ssao-pass",
            &self.ssao_pipeline,
            &self.depth_bind_group,
            &self.ao_raw_view,
        );
        // pass 2：遮蔽图 → 4×4 散焦
        self.fullscreen(
            encoder,
            "r3e-ssao-blur",
            &self.blur_pipeline,
            &self.blur_bind_group,
            &self.ao_final_view,
        );

        self.stats = SsaoStats {
            enabled: true,
            passes: AO_PASSES_PER_FRAME,
            samples: samples as u32,
            size: self.size,
        };
    }

    fn fullscreen(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        pipeline: &wgpu::RenderPipeline,
        source: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 每个像素都会被全屏三角形覆盖，清屏值永远不会被读到
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.params_bind_group, &[]);
        pass.set_bind_group(1, source, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn create_ao_target(
    device: &wgpu::Device,
    size: (u32, u32),
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: AO_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn depth_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("r3e-ssao-depth-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

fn ao_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("r3e-ssao-ao-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

fn ao_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
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
            // 全屏三角形不做背面剔除：uv 翻转后绕序可能反过来，
            // 一旦剔除就整屏消失，这类 bug 只在实机上才现形。
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
                format: AO_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
