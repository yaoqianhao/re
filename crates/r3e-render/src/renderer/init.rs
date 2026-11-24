use std::collections::HashMap;
use std::sync::Arc;

use r3e_core::{Sh9, MAX_CASCADES};
use winit::window::Window;

use crate::envprobe::{self, EnvProbeSource, EnvProbeStats};
use crate::gfx;
use crate::mesh::{vertex_layout, InstanceRaw};
use crate::occlusion::OcclusionCuller;
use crate::postfx::PostFxPass;
use crate::ssao::SsaoPass;
use crate::text::TextPass;
use crate::ui::UiPass;

use super::*;

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // 扩展四十四：设备获取下沉到 `gfx` 抽象层。这里只表达「按选择打开上下文」，
        // 不再直接碰 Instance / Adapter / Device —— 选哪套图形 API 由 `R3E_GFX_API`
        // 与平台偏好表决定（见 `crate::gfx`），今天只有 wgpu 一个实现。
        let selection = gfx::selection()?;
        log::info!("图形 API 选择: {}", selection.describe());
        let context = gfx::create_context(&selection, Arc::clone(&window))?;
        let gfx::GfxContext {
            info: gfx_info,
            device,
            queue,
            surface,
            config,
            ..
        } = context;

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-camera-ubo"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-camera-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-camera-bg"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader.wgsl").into()),
        });

        let material_cache = MaterialCache::new(&device, &queue);

        // P4：group(2) —— 关节矩阵 storage buffer，所有蒙皮实例共用一段连续槽位。
        // 扩展三十六：同一组里追加 binding(1) 的形变增量表，两条都是只读 storage。
        let joint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-joint-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let joint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-joint-buffer"),
            size: (r3e_core::MAX_JOINT_MATRICES * 16 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 扩展三十六：形变增量表。容量固定 → 绑定组只建一次，内容变了就整表重写。
        let morph_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-morph-buffer"),
            size: (MAX_MORPH_TABLE_FLOATS * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let joint_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-joint-bg"),
            layout: &joint_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: joint_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: morph_buffer.as_entire_binding(),
                },
            ],
        });

        // 阶段 D：group(3) —— 方向光阴影贴图。布局与纹理在管线之前建好，
        // 尺寸按场景配置在 render() 里按需重建（改 `shadow.size` 不必重建整机）。
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-shadow-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        // 扩展十六：阴影图是 `MAX_CASCADES` 层的纹理数组，主通道按级取层
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 扩展十五：局部光源表（点光 / 聚光）。只被片元着色器读取。
                // 塞进 group(3) 而不是另开 group(4)：WebGPU core 规范里
                // `max_bind_groups` 就是 4，第 5 个组（索引 4）直接过不了校验。
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 扩展十八 C：局部光源阴影图（点光立方 6 面 / 聚光单面共用一张分层数组）。
                // 与 binding(0) 的方向光阴影图分开绑定：两者采样语义不同（一个按级联选层，
                // 一个按灯的槽位选层），合并在同一张图上会让层号空间互相踩。
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // 光源表 uniform：尺寸固定（1 个计数 vec4 + 16 × 64B），改灯数不需要重建。
        let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-lights-ubo"),
            size: std::mem::size_of::<LightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 深度 pass 的独立 group(3)：只有 binding(3) 一个矩阵 uniform，
        // 与主通道采样的 binding(0..=2) 完全分开，避免「写的同时采样」。
        let shadow_pass_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("r3e-shadow-pass-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        // 每级一份矩阵 uniform / 绑定组：深度 pass 逐级复用同一份管线布局
        let shadow_pass_uniforms: Vec<wgpu::Buffer> = (0..MAX_CASCADES)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("r3e-shadow-pass-ubo"),
                    size: std::mem::size_of::<ShadowPassUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let shadow_pass_bind_groups: Vec<wgpu::BindGroup> = shadow_pass_uniforms
            .iter()
            .map(|uniform| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("r3e-shadow-pass-bg"),
                    layout: &shadow_pass_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    }],
                })
            })
            .collect();

        let shadow_size = r3e_core::ShadowSettings::default().size;
        let (shadow_texture, shadow_view, shadow_layer_views) =
            create_shadow_target(&device, shadow_size);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-shadow-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // 比较采样：一次采样即完成硬件 PCF 的「小于等于」深度测试
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-shadow-ubo"),
            size: std::mem::size_of::<ShadowUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 扩展十八 C：局部光源阴影。尺寸固定（局部阴影不跟方向光的 `shadow.size`
        // 联动：一盏点光要占 6 层，跟着 4096 走会直接把显存打爆），
        // 每层一张 512×512 深度图，共 24 层（4 盏灯 × 6 面）。
        let local_shadow_size = r3e_core::DEFAULT_LOCAL_SHADOW_SIZE;
        let (local_shadow_texture, local_shadow_view, local_shadow_layer_views) =
            create_local_shadow_target(&device, local_shadow_size);
        let local_shadow_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-local-shadow-ubo"),
            size: LOCAL_SHADOW_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // 每层一份「只含该层投影矩阵」的 uniform / 绑定组：与方向光的深度 pass
        // 共用同一个 `shadow_pass_layout`，因此也能共用同一份深度管线。
        let local_shadow_pass_uniforms: Vec<wgpu::Buffer> = (0..r3e_core::MAX_LOCAL_SHADOW_LAYERS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("r3e-local-shadow-pass-ubo"),
                    size: std::mem::size_of::<ShadowPassUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let local_shadow_pass_bind_groups: Vec<wgpu::BindGroup> = local_shadow_pass_uniforms
            .iter()
            .map(|uniform| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("r3e-local-shadow-pass-bg"),
                    layout: &shadow_pass_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    }],
                })
            })
            .collect();

        let shadow_bind_group = create_shadow_bind_group(
            &device,
            &shadow_layout,
            ShadowBindInputs {
                view: &shadow_view,
                sampler: &shadow_sampler,
                uniform: &shadow_uniform_buffer,
                lights: &lights_buffer,
                local_view: &local_shadow_view,
                local_uniform: &local_shadow_uniform,
            },
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-pipeline-layout"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&material_cache.layout),
                Some(&joint_layout),
                Some(&shadow_layout),
            ],
            immediate_size: 0,
        });

        // 阴影通道专用布局：group(3) 换成 `shadow_pass_layout`（只有矩阵 uniform），
        // 不含「阴影贴图 + 比较采样器」——深度 pass 不能一边写一边采样同一张图。
        // 注意 wgpu 要求布局里每个组都被 set，所以 0/1/2 仍要在 pass 里逐个绑定。
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("r3e-shadow-pipeline-layout"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&material_cache.layout),
                    Some(&joint_layout),
                    Some(&shadow_pass_layout),
                ],
                immediate_size: 0,
            });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                // 扩展十九：主通道开通 MSAA。光栅化在 4× 附件上进行，
                // pass 结束时 resolve 到单采样 HDR 目标（见 `render_to_view`）。
                count: MSAA_SAMPLE_COUNT,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    // 扩展十三：主通道不再直接写 surface，目标换成后处理的
                    // 线性 HDR 离屏纹理（见 `PostFxPass::hdr_view`）。输出仍是线性值，
                    // sRGB 编码交给 composite 在写 surface 时一次性完成。
                    format: crate::postfx::HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let (depth_texture, depth_view) = create_depth_target(&device, width, height);
        // 扩展十九：MSAA 附件。颜色附件由硬件 resolve 进 HDR 目标，
        // 深度附件不做 resolve（深度跨样本没有物理意义），单采样深度改由预通道提供。
        let (msaa_color, msaa_color_view, msaa_depth, msaa_depth_view) =
            create_msaa_targets(&device, width, height);

        // 扩展十九：半透明管线。除「不写深度」外与主管线逐字段一致——
        // 同一份着色器、同一套混合、同一个 HDR 目标格式，
        // 只是半透明物件不能把深度写下去，否则会挡掉后面同样半透明的物件。
        let transparent_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-transparent-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // 半透明面片多半是单层薄面（玻璃 / 树叶 / 水面），
                // 一旦背面被剔除，从另一侧看过去整块物件就凭空消失
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLE_COUNT,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::postfx::HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // 扩展十九：深度预通道。主通道的深度挂在 4× 附件上，无法给遮蔽通道
        // 当输入（多重采样纹理不能直接采样），所以在主通道之前先用单采样
        // `depth_view` 铺一遍不透明批次的深度。只写深度、无颜色目标，fragment 为 None。
        let depth_prepass_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("r3e-depth-prepass-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                // 单样本：目标就是 `depth_view`，与主通道的多重采样附件互不相干
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

        // 阶段 D：阴影通道管线 —— 无颜色目标，只写深度。
        // 顶点布局与主通道完全一致（同一份实例缓冲直接复用），
        // 因此阴影通道不需要任何额外 CPU 侧顶点处理。
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-shadow-pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout()), Some(InstanceRaw::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // 斜率相关偏移：斜射面最容易出 acne，硬件斜率偏移比纯常数偏移稳
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-instance-buffer"),
            size: INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<InstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let text = TextPass::new(&device, config.format);
        let ui = UiPass::new(&device, config.format);
        // 后处理链与窗口同级：主通道从此写离屏 HDR 目标，不再直接写 surface
        let postfx = PostFxPass::new(&device, &queue, config.format, (width, height));
        // 遮蔽吃主通道深度、产出与 HDR 同尺寸的 AO 图，所以与窗口同级创建
        let ssao = SsaoPass::new(&device, (width, height), &depth_view);

        // 扩展三十五：暂存环要在 `device` 被 move 进结构体之前建好
        let staging = UploadStaging::new(&device, staging_enabled_from_env());

        Ok(Self {
            device,
            queue,
            gfx_info,
            surface,
            config,
            size: (width, height),
            pipeline,
            transparent_pipeline,
            depth_prepass_pipeline,
            camera_buffer,
            camera_bind_group,
            depth_view,
            depth_texture,
            occlusion: OcclusionCuller::new(),
            _msaa_color: msaa_color,
            msaa_color_view,
            _msaa_depth: msaa_depth,
            msaa_depth_view,
            mesh_cache: MeshCache::new(),
            material_cache,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            // 首帧没有影子：第一次上传自然走全量，之后就只传脏段
            instance_shadow: Vec::new(),
            instance_upload: InstanceUploadStats::default(),
            instance_upload_resets: 0,
            joint_buffer,
            joint_bind_group,
            morph_buffer,
            // 0 = 与空缓存同版本：首帧没有形变表可传，等第一个带 morph 的网格出现再传
            morph_uploaded_version: 0,
            morph_scratch: Vec::new(),
            morph_uploads: 0,
            morph_floats: 0,
            joint_scratch: Vec::new(),
            // 首帧没有关节影子：第一次有蒙皮时自然走全量
            joint_shadow: Vec::new(),
            joint_upload: InstanceUploadStats::default(),
            staging,
            shadow_size,
            _shadow_texture: shadow_texture,
            shadow_view,
            shadow_layer_views,
            shadow_sampler,
            shadow_layout,
            shadow_bind_group,
            shadow_uniform: shadow_uniform_buffer,
            lights_buffer,
            shadow_pass_bind_groups,
            shadow_pass_uniforms,
            local_shadow_size,
            _local_shadow_texture: local_shadow_texture,
            local_shadow_view,
            local_shadow_layer_views,
            local_shadow_uniform,
            local_shadow_pass_uniforms,
            local_shadow_pass_bind_groups,
            shadow_pipeline,
            text,
            ui,
            postfx,
            ssao,
            env_cache: None,
            env_sh: Sh9::default(),
            env_builds: 0,
            env_probe: None,
            caster_tree: None,
            caster_slots: HashMap::new(),
            caster_layout: 0,
            caster_node_bounds: Vec::new(),
            caster_node_casts: Vec::new(),
            caster_instance_nodes: Vec::new(),
            caster_tree_builds: 0,
            cull_tree: None,
            cull_slots: HashMap::new(),
            cull_layout: 0,
            cull_node_bounds: Vec::new(),
            cull_node_casts: Vec::new(),
            cull_tree_builds: 0,
            terrain: None,
            terrain_dirty: None,
            terrain_layers: crate::terrain::TerrainLayersUniform::default(),
            stats: RenderStats::default(),
        })
    }

    /// 扩展二十三：装载 HDR 反射探针。
    ///
    /// 传 `None` 等价于 [`Self::clear_env_probe`]（退回解析 IBL）。上传整条 mip 链是
    /// 一次性开销：每层每面一块 `write_texture`，因此只在场景加载 / 换探针时调用，
    /// 不要放进每帧循环。
    pub fn set_env_probe(&mut self, source: Option<&EnvProbeSource>) {
        let Some(source) = source else {
            self.clear_env_probe();
            return;
        };
        let prefiltered = source.prefiltered();
        let face_size = prefiltered.base().face_size() as u32;
        let mip_levels = prefiltered.mip_levels().max(1);
        if face_size == 0 {
            log::warn!("反射探针面边长为 0，忽略并退回解析 IBL");
            self.clear_env_probe();
            return;
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r3e-env-probe-cube"),
            size: wgpu::Extent3d {
                width: face_size,
                height: face_size,
                depth_or_array_layers: envprobe::CUBE_FACES as u32,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // 线性 HDR：辐亮度可以远大于 1，8 位会被截断成白斑
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for upload in envprobe::encode_faces(prefiltered) {
            // 预滤波层数可能多于纹理层数（配置被收敛过），多出来的层直接丢弃
            if upload.mip >= mip_levels {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: upload.mip,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: upload.face,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.bytes_per_row()),
                    rows_per_image: Some(upload.size),
                },
                wgpu::Extent3d {
                    width: upload.size,
                    height: upload.size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("r3e-env-probe-cube-view"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            mip_level_count: Some(mip_levels),
            ..Default::default()
        });

        // 换图必须重建全部材质 bind group（视图写在组里）
        self.material_cache.set_env_cube(view);
        self.env_probe = Some(EnvProbeGpu {
            _texture: texture,
            stats: source.stats(),
            // 漫反射用最清晰那层的辐照度投影：模糊层已经被粗糙度卷积过，再积分会双重衰减
            irradiance: prefiltered.base().project_irradiance(),
        });
    }

    /// 扩展二十三：卸载探针，退回扩展十二的解析 IBL（画面逐像素不变）。
    pub fn clear_env_probe(&mut self) {
        if self.env_probe.take().is_none() {
            return;
        }
        self.material_cache.reset_env_cube();
    }

    /// 当前生效的探针统计；没配探针时 `active = false`。
    pub fn env_probe_stats(&self) -> EnvProbeStats {
        self.env_probe.as_ref().map(|p| p.stats).unwrap_or_default()
    }
}
