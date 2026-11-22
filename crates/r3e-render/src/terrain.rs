//! 地形渲染通道（M2-C03b）：独立顶点布局 + 4 层材质混合着色。
//!
//! # 为什么是独立管线，而不是给 PBR 顶点布局再加一个属性
//!
//! `crate::mesh` 的顶点业已占 `0..=4` 与 `11`，实例侧占 `5..=10` 与 `12..=15`——
//! webgpu 的 attribute location 只有 `0..=15` 这 16 格，**已经全满**。地形要 4 层权重
//! （一个 `Float32x4`）连塞进去的格子都没有。更根本的差异是语义：地形不做蒙皮、不做
//! 实例变换矩阵，但有逐顶点混合权重；硬并进 PBR 布局意味着改写着色器的每一个顶点入口，
//! 换来的是「省一条管线」，不划算。
//!
//! 于是这里自带 `VertexBufferLayout` / `wgsl` / bind group。**PBR 管线一行不改**，
//! 两条管线的 location 编号在各自管线内独立编号，互不影响。
//!
//! # 与 `r3e-terrain` 的关系（依赖方向红线）
//!
//! `r3e-render` 的 `allow` 表只有 `r3e-core`，本 crate 看不见 `r3e-terrain`。所以这一层
//! 收的是**裸切片**（`&[[f32; 3]]` / `&[[f32; 4]]` / `&[u32]`），不认 `TerrainMesh`——
//! 由装配层（`r3e_runtime::terrain_scene`）把 `TerrainMesh` 拆成这几个切片再递进来。
//! 好处是渲染这一侧对「地形数据从哪来」一无所知，测试用两条手写三角形就能覆盖。

use bytemuck::{Pod, Zeroable};

/// 材质层数。与 `terrain.wgsl` 的 `array<vec4<f32>, 4>` 和逐顶点 `weights` 的 4 分量一一对应。
pub const TERRAIN_LAYER_COUNT: usize = 4;

/// 单顶点字节数：`pos(12) + normal(12) + uv(8) + weights(16)`。
///
/// 写成常量而不是 `size_of::<TerrainVertex>()`，是为了让测试能断言「结构体没有 padding 漂移」——
/// 用 `size_of` 断言等于自己证明自己。
pub const TERRAIN_VERTEX_STRIDE: u32 = 48;

/// 相机 uniform 的字节数：`view_proj(64) + eye(16) + light(16) + sun_dir(16)`。
///
/// 与 `terrain.wgsl` 的 `Camera` 同构；写成常量而非 `size_of` 是为了让「shader 侧
/// struct 涨了而 buffer 没涨」这种漂移在代码评审里看得见。
pub const CAMERA_UNIFORM_BYTES: u32 = 112;

/// 地形顶点：`#[repr(C)]` 且全 `f32` 数组，故与 WGSL 的 4 属性布局逐字节一致。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub weights: [f32; 4],
}

/// 顶点属性表：0 = position、1 = normal、2 = uv、3 = 4 层权重。
///
/// 与 PBR 顶点属性的区别（测试 1 钉住的就是这几条）：本表**只有 4 条**、不含 joints /
/// 切线 / 蒙皮槽，且地形管线只挂一个 `Vertex` 步进缓冲，没有实例缓冲。
pub const TERRAIN_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Float32x4
];

/// 地形顶点缓冲布局。`array_stride` 取 [`TERRAIN_VERTEX_STRIDE`]。
pub fn terrain_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: TERRAIN_VERTEX_STRIDE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &TERRAIN_ATTRIBUTES,
    }
}

impl TerrainVertex {
    /// GPU 侧布局。存在的意义是让管线创建处不必重复写一遍 stride 与属性表。
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        terrain_vertex_layout()
    }
}

/// 权重归一化：把任意 4 分量纠正成「非负、和为 1」。
///
/// `r3e_terrain::block_mesh` 产出的权重本来就已归一化，这个函数不是给它擦屁股，而是
/// **管线入口的守卫**：装配层以后若接了别的权重来源（手工摆的、脚本改的、插值出来的），
/// 归一化保证不会跑进一个「着色结果被 `weights` 和小于 1 悄悄压暗」的场面。
///
/// 退化处理：任一负分量先夹 0；四个全 0 或含 `NaN` / 无穷时退化为层 0 独占。
pub fn normalise_weights(weights: [f32; 4]) -> [f32; 4] {
    let mut clamped = [0.0f32; 4];
    let mut sum = 0.0f32;
    for (dst, src) in clamped.iter_mut().zip(weights) {
        // `NaN` 比较恒假，这里用「不是有限值就记 0」而不是 `!(src > 0.0)`
        *dst = if src.is_finite() && src > 0.0 {
            src
        } else {
            0.0
        };
        sum += *dst;
    }
    if !sum.is_finite() || sum <= 0.0 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    let inv = 1.0 / sum;
    [
        clamped[0] * inv,
        clamped[1] * inv,
        clamped[2] * inv,
        clamped[3] * inv,
    ]
}

/// 把并列的裸切片打成一串 [`TerrainVertex`]。
///
/// 长度以四个输入里的**最短者**为准（多出来的位置静默丢弃，不做 panic）——装配层已经把
/// 「四者等长」当作前置条件测过，这里再 panic 一次只会让上层多一种崩法。
///
/// 权重逐个过 [`normalise_weights`]：这是「着色器输入端已归一化」的落点。
pub fn pack_vertices(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    weights: &[[f32; 4]],
) -> Vec<TerrainVertex> {
    let count = positions
        .len()
        .min(normals.len())
        .min(uvs.len())
        .min(weights.len());
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push(TerrainVertex {
            position: positions[i],
            normal: normals[i],
            uv: uvs[i],
            weights: normalise_weights(weights[i]),
        });
    }
    out
}

/// 索引是否必须用 `u32`：单块顶点数超过 `u16::MAX + 1` 时为真。
///
/// 实测会超：200 米地块按 0.78 米步长排下来约 256 列 × 256 行 ≈ 66k 顶点 > 65536。
/// 所以 [`TerrainIndices`] 从第一天就是 `u32`，不留「以后再说」的口子。
pub fn needs_u32_indices(vertex_count: usize) -> bool {
    vertex_count > u16::MAX as usize + 1
}

/// 一条地形绘制项：顶点与索引都已是 GPU 可上传形态。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainDrawItem {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

impl TerrainDrawItem {
    pub fn from_parts(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        weights: &[[f32; 4]],
        indices: &[u32],
    ) -> Self {
        Self {
            vertices: pack_vertices(positions, normals, uvs, weights),
            indices: indices.to_vec(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }
}

/// 4 层材质参数（`terrain.wgsl` 的 `@group(0) @binding(1)`）。
///
/// `base` 是 4 层基色，`shaping` 是光照塑形系数（x = 环境、y = 直射）。缺省值即
/// 「常量色」兜底：草绿 / 岩灰 / 沙黄 / 雪白。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct TerrainLayersUniform {
    pub base: [[f32; 4]; TERRAIN_LAYER_COUNT],
    pub shaping: [f32; 4],
}

impl Default for TerrainLayersUniform {
    fn default() -> Self {
        Self {
            base: [
                [0.29, 0.42, 0.22, 1.0],
                [0.42, 0.41, 0.39, 1.0],
                [0.74, 0.66, 0.44, 1.0],
                [0.92, 0.93, 0.95, 1.0],
            ],
            shaping: [0.55, 0.75, 0.0, 0.0],
        }
    }
}

impl TerrainLayersUniform {
    /// 按层名取色：认 `grass` / `rock` / `sand` / `snow`，其余（含拼错的名字）一律回落
    /// 到该层的缺省常量色，不 panic、不报错。
    ///
    /// 这是测试 6 的落点：素材里材质名对不上是常态，地形不该因此整块消失。
    pub fn color_of(&self, name: &str) -> [f32; 4] {
        match layer_index(name) {
            Some(i) => self.base[i],
            None => Self::default().base[0],
        }
    }

    /// 覆盖某一层基色；名字不认识时返回 `false`（调用方自己决定要不要记 warn）。
    pub fn set_layer(&mut self, name: &str, color: [f32; 4]) -> bool {
        match layer_index(name) {
            Some(i) => {
                self.base[i] = color;
                true
            }
            None => false,
        }
    }
}

/// 层名 → 层号。大小写不敏感，容忍前后空白。
pub fn layer_index(name: &str) -> Option<usize> {
    match name.trim().to_ascii_lowercase().as_str() {
        "grass" => Some(0),
        "rock" => Some(1),
        "sand" => Some(2),
        "snow" => Some(3),
        _ => None,
    }
}

/// 着色函数的 CPU 等价实现：按权重混合 4 层颜色。
///
/// 存在的唯一目的是让「混合口径」在不碰 GPU 的环境里可判（测试 12）。它必须与
/// `terrain.wgsl` 的 `mix_layers` 保持同一口径：**不做归一化**，权重和是多少就按多少乘。
pub fn mix_layers(weights: [f32; 4], base: &[[f32; 4]; TERRAIN_LAYER_COUNT]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (layer, w) in base.iter().zip(weights) {
        out[0] += layer[0] * w;
        out[1] += layer[1] * w;
        out[2] += layer[2] * w;
    }
    out
}

/// 地形 GPU 资源：顶点/索引缓冲 + 两个 uniform + 管线。
///
/// 缓冲按需重建：调用方把 [`TerrainDrawItem`] 递进来，`upload` 在容量够用时只写不重建，
/// 不够才重新分配——每帧无脑 `create_buffer` 是最贵的偷懒。
pub struct TerrainPass {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    layers_buffer: wgpu::Buffer,
    layers_bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    vertex_capacity: u64,
    index_capacity: u64,
    index_count: u32,
}

impl TerrainPass {
    /// 建管线。`color_format` / `depth_format` 由调用方给（主通道是 HDR + 深度附件）。
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-terrain-camera"),
            size: CAMERA_UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layers_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-terrain-layers"),
            size: std::mem::size_of::<TerrainLayersUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = uniform_layout(device, "r3e-terrain-camera-bgl");
        let layers_layout = uniform_layout(device, "r3e-terrain-layers-bgl");
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-terrain-camera-bg"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let layers_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-terrain-layers-bg"),
            layout: &layers_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: layers_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-terrain-pipeline-layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&layers_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-terrain-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: &[Some(terrain_vertex_layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_terrain"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            camera_buffer,
            camera_bind_group,
            layers_buffer,
            layers_bind_group,
            vertex_buffer: None,
            index_buffer: None,
            vertex_capacity: 0,
            index_capacity: 0,
            index_count: 0,
        }
    }

    /// 写入材质层参数与相机矩阵。
    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        sun_dir: [f32; 3],
        light: [f32; 2],
        layers: &TerrainLayersUniform,
    ) {
        let mut camera = [0.0f32; 32];
        for (row, values) in view_proj.iter().enumerate() {
            camera[row * 4..row * 4 + 4].copy_from_slice(values);
        }
        camera[16..19].copy_from_slice(&eye);
        camera[20] = light[0];
        camera[21] = light[1];
        camera[24..27].copy_from_slice(&sun_dir);
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&camera));
        queue.write_buffer(&self.layers_buffer, 0, bytemuck::bytes_of(layers));
    }

    /// 上传一块地形。返回 `true` 表示这次真的重建了缓冲（装配层拿它算 rebuild 次数）。
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        item: &TerrainDrawItem,
    ) -> bool {
        if item.is_empty() {
            self.index_count = 0;
            return false;
        }
        let vertex_bytes = (item.vertices.len() as u64) * (TERRAIN_VERTEX_STRIDE as u64);
        let index_bytes = (item.indices.len() as u64) * 4;
        let mut rebuilt = false;

        if vertex_bytes > self.vertex_capacity {
            self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-terrain-vertices"),
                size: vertex_bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.vertex_capacity = vertex_bytes.next_power_of_two();
            rebuilt = true;
        }
        if index_bytes > self.index_capacity {
            self.index_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-terrain-indices"),
                size: index_bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.index_capacity = index_bytes.next_power_of_two();
            rebuilt = true;
        }

        if let Some(buffer) = &self.vertex_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&item.vertices));
        }
        if let Some(buffer) = &self.index_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&item.indices));
        }
        self.index_count = item.indices.len() as u32;
        rebuilt
    }

    /// 当前待绘制的索引数（0 表示这帧没有地形）。
    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    /// 往主通道里画地形。没有缓冲时什么都不做，不 panic。
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(vertex_buffer), Some(index_buffer)) = (&self.vertex_buffer, &self.index_buffer)
        else {
            return;
        };
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(1, &self.layers_bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

fn uniform_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// 着色器源码：测试用它做 naga 解析，运行期由 `include_str!` 编进去。
pub const TERRAIN_WGSL: &str = include_str!("terrain.wgsl");
