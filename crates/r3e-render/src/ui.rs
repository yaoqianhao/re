//! 屏幕空间 2D UI 通道（M1「最小 UI」）。
//!
//! 设计要点：
//!
//! - **一个通道画完全部 UI**：矩形、边框、图片都被规划成「四边形实例」，同一条
//!   pipeline、同一次 render pass 画完。没有给边框单独开通道，也没有给图片单独开
//!   通道——通道数量与元素类型无关，只与用到的纹理数量有关（每个纹理一次 draw）。
//! - **实例化**：与文本通道一样走 instance buffer，一个实例 48 字节，一帧只上传一次；
//!   顶点位置由 `vertex_index` 推出来，不需要顶点缓冲。
//! - **不改动 3D 通道**：不写深度、不测深度，`LoadOp::Load` 叠加，影子 / 蒙皮 /
//!   实例化相关的 pipeline 与 bind group 完全不受影响。
//! - **纯 CPU 规划可单测**：[`plan_ui`] 只吃 `UiElement` 与屏幕尺寸，产出实例列表，
//!   不碰 wgpu；锚点、百分比、层级、边框退化、超界裁剪、实例上限都在这一层验证。
//! - **纹理按路径缓存**：图片走 png（复用 `r3e_asset` 的 `image` 依赖解码），加载失败
//!   只记一次警告，之后用元素自身颜色（默认白）走回退路径，不会每帧重复尝试。
//!
//! 与文本的层次关系由 `Scene::ui_above_text` 决定，渲染器按该标志安排两个通道的
//! 先后顺序；默认先 UI 后文本，即文本在 UI 之上。

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use r3e_core::mesh::TextureImage;
use r3e_core::ui::{resolve_rect, sorted_for_draw, UiElement, UiKind};

/// UI 图片贴图来源。由应用层实现（复用 asset 层的 `image` 依赖解码 png 等格式），
/// 渲染层只负责上传与绘制，保持「render 不做文件 IO」的现有分层。
pub trait UiImageProvider: Send + Sync {
    /// 按路径加载一张图片。失败返回错误描述，渲染层会回退成纯色块且不再重试。
    fn load_ui_image(&self, path: &str) -> Result<TextureImage, String>;
}

/// 本帧需要向 provider 请求的图片：既不在缓存里、也不在失败名单里的那些。
///
/// 纯逻辑，便于验证「失败不重复尝试、缓存命中不重复加载」。
pub fn images_to_load(needed: &[String], loaded: &[String], failed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in needed {
        if loaded.iter().any(|p| p == path) || failed.iter().any(|p| p == path) {
            continue;
        }
        if !out.iter().any(|p| p == path) {
            out.push(path.clone());
        }
    }
    out
}

/// 单帧四边形上限（一帧最多 4096 个实例 ≈ 196 KiB 实例数据）。
pub const MAX_UI_QUADS: usize = 4096;
/// 实例缓冲初始容量。
const INITIAL_UI_QUADS: u64 = 256;
/// 单个 UI 纹理的边长上限，超过视为加载失败（避免脚本塞超大图把显存吃光）。
pub const MAX_UI_TEXTURE_DIM: u32 = 8192;

/// 一个 UI 四边形实例，与 `ui.wgsl` 的 `Instance` 一一对应（48 字节）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct UiInstance {
    /// 左上角（屏幕像素，原点在左上角）
    pub origin: [f32; 2],
    /// 像素宽高
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// 已乘过元素 alpha 的颜色
    pub color: [f32; 4],
}

impl UiInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
            2 => Float32x2,
            3 => Float32x2,
            4 => Float32x4,
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// 一段用同一个纹理绘制的实例（一次 draw）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiBatch {
    /// `None` 表示纯色（用 1x1 白纹理，靠实例颜色出效果）
    pub texture: Option<String>,
    pub instances: Vec<UiInstance>,
}

/// 纯 CPU 的建批结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiBuild {
    pub batches: Vec<UiBatch>,
    /// 本帧参与绘制的元素数（可见且矩形有效）
    pub elements: u32,
    /// 被跳过的可见元素数（尺寸非法 / 屏幕为 0 / 超出实例上限）
    pub skipped: u32,
    /// 用到的图片路径（去重，保持首次出现顺序）
    pub images: Vec<String>,
}

impl UiBuild {
    /// 本帧实例（四边形）总数。
    pub fn quad_count(&self) -> u32 {
        self.batches.iter().map(|b| b.instances.len() as u32).sum()
    }

    fn push(&mut self, texture: Option<&str>, instances: &[UiInstance]) {
        if instances.is_empty() {
            return;
        }
        if let Some(last) = self.batches.last_mut() {
            if last.texture.as_deref() == texture {
                last.instances.extend_from_slice(instances);
                return;
            }
        }
        self.batches.push(UiBatch {
            texture: texture.map(str::to_string),
            instances: instances.to_vec(),
        });
    }
}

/// 把一个可见元素规划成若干四边形实例。
///
/// 返回 `None` 表示这个元素本帧不该画（尺寸为 0 / 非有限 / 屏幕为 0）。
fn plan_element(el: &UiElement, screen: (f32, f32), out: &mut Vec<UiInstance>) -> Option<()> {
    let rect = resolve_rect(el, screen)?;
    out.clear();
    match el.kind {
        UiKind::Rect => out.push(UiInstance {
            origin: [rect.x, rect.y],
            size: [rect.width, rect.height],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: el.color,
        }),
        UiKind::Image => out.push(UiInstance {
            origin: [rect.x, rect.y],
            size: [rect.width, rect.height],
            uv_min: [el.uv[0], el.uv[1]],
            uv_max: [el.uv[2], el.uv[3]],
            color: el.color,
        }),
        UiKind::Border => {
            // 四条边切成四个不重叠的矩形：粗细先夹到短边的一半，避免细矩形画成正片
            let t = el.border_width.min(rect.width * 0.5).min(rect.height * 0.5);
            if t <= 0.0 {
                return None;
            }
            let quad = |x: f32, y: f32, w: f32, h: f32| UiInstance {
                origin: [x, y],
                size: [w, h],
                uv_min: [0.0, 0.0],
                uv_max: [1.0, 1.0],
                color: el.color,
            };
            // 上 / 下整条，左 / 右夹在中间，保证四角只画一次
            out.push(quad(rect.x, rect.y, rect.width, t));
            out.push(quad(rect.x, rect.bottom() - t, rect.width, t));
            let mid_h = rect.height - t * 2.0;
            if mid_h > 0.0 {
                out.push(quad(rect.x, rect.y + t, t, mid_h));
                out.push(quad(rect.right() - t, rect.y + t, t, mid_h));
            }
        }
    }
    Some(())
}

/// 把 UI 元素列表规划成实例批次（纯 CPU，可单测）。
pub fn plan_ui(elements: &[UiElement], screen: (f32, f32)) -> UiBuild {
    plan_ui_limited(elements, screen, MAX_UI_QUADS)
}

/// 同 [`plan_ui`]，但可指定四边形上限（测试用）。
pub fn plan_ui_limited(elements: &[UiElement], screen: (f32, f32), max_quads: usize) -> UiBuild {
    let mut build = UiBuild::default();
    let mut quads = 0usize;
    let mut buf: Vec<UiInstance> = Vec::with_capacity(4);

    for el in sorted_for_draw(elements) {
        // 先按剩余额度预判，避免画出「半个边框」
        if quads >= max_quads {
            build.skipped += 1;
            continue;
        }
        if plan_element(el, screen, &mut buf).is_none() {
            build.skipped += 1;
            continue;
        }
        let allowed = max_quads.saturating_sub(quads);
        let take = buf.len().min(allowed);
        if take < buf.len() {
            build.skipped += 1;
        }
        let texture = match el.kind {
            UiKind::Image if !el.image.trim().is_empty() => Some(el.image.trim()),
            _ => None,
        };
        if let Some(path) = texture {
            if !build.images.iter().any(|p| p == path) {
                build.images.push(path.to_string());
            }
        }
        build.push(texture, &buf[..take]);
        quads += take;
        build.elements += 1;
    }

    build
}

/// 本帧 UI 通道统计（供 HUD / 调试输出）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiStats {
    pub enabled: bool,
    /// 参与绘制的元素数
    pub elements: u32,
    /// 四边形数
    pub quads: u32,
    pub draw_calls: u32,
    /// 已缓存的纹理数
    pub textures: u32,
    /// 本帧因图片缺失而回退的元素数
    pub missing_images: u32,
    /// 因上限被跳过的元素数
    pub skipped: u32,
}

/// 一个已上传的 UI 纹理。
struct UiTexture {
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
}

/// GPU 侧 UI 通道。
pub struct UiPass {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    /// 1x1 白纹理：纯色元素与「图片缺失」的回退都走它
    white: UiTexture,
    textures: HashMap<String, UiTexture>,
    failed: Vec<String>,
    provider: Option<Arc<dyn UiImageProvider>>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    enabled: bool,
    stats: UiStats,
}

impl UiPass {
    /// 建管线（一个 pipeline 覆盖矩形 / 边框 / 图片）。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-ui-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").into()),
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-ui-texture-bgl"),
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
            ],
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-ui-globals-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-ui-globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-ui-globals-bg"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-ui-pipeline-layout"),
            bind_group_layouts: &[Some(&texture_layout), Some(&globals_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-ui-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(UiInstance::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // 2D 四边形不需要背面概念
                cull_mode: None,
                ..Default::default()
            },
            // 与文本通道一致：不写深度、不测深度
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-ui-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let white = Self::create_texture(
            device,
            &texture_layout,
            &sampler,
            &TextureImage::new(1, 1, vec![255, 255, 255, 255]),
        )
        .expect("1x1 白纹理必定可创建");

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-ui-instances"),
            size: INITIAL_UI_QUADS * std::mem::size_of::<UiInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            texture_layout,
            sampler,
            globals_buffer,
            globals_bind_group,
            white,
            textures: HashMap::new(),
            failed: Vec::new(),
            provider: None,
            instance_buffer,
            instance_capacity: INITIAL_UI_QUADS,
            enabled: true,
            stats: UiStats::default(),
        }
    }

    /// 通道开关。关掉后每帧不产生任何 GPU 命令，场景数据保留。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn stats(&self) -> UiStats {
        self.stats
    }

    /// 已缓存的纹理数（不含内置白纹理）。
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// 丢弃所有图片纹理与「加载失败」记录，下一帧按需重新加载。
    pub fn release_images(&mut self) {
        self.textures.clear();
        self.failed.clear();
    }

    fn create_texture(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        image: &TextureImage,
    ) -> Option<UiTexture> {
        if !image.is_valid() || image.width == 0 || image.height == 0 {
            return None;
        }
        if image.width > MAX_UI_TEXTURE_DIM || image.height > MAX_UI_TEXTURE_DIM {
            log::warn!(
                "UI 图片 {}x{} 超过上限 {MAX_UI_TEXTURE_DIM}，按加载失败处理",
                image.width,
                image.height
            );
            return None;
        }
        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r3e-ui-texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-ui-texture-bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Some(UiTexture {
            bind_group,
            _texture: texture,
            _view: view,
        })
    }

    /// 设置图片提供者（应用层注入，通常复用 asset 层的解码能力）。
    pub fn set_image_provider(&mut self, provider: Arc<dyn UiImageProvider>) {
        self.provider = Some(provider);
        // 换提供者后旧缓存可能来自旧路径解析规则，清掉重来
        self.release_images();
    }

    /// 已加载失败的图片路径（供调试输出）。
    pub fn failed_images(&self) -> &[String] {
        &self.failed
    }

    fn cached_paths(&self) -> Vec<String> {
        self.textures.keys().cloned().collect()
    }

    /// 确保某个路径的图片已上传。失败只记录一次，返回 `false` 让调用方回退。
    fn ensure_image(&mut self, device: &wgpu::Device, path: &str) -> bool {
        if self.textures.contains_key(path) {
            return true;
        }
        if self.failed.iter().any(|p| p == path) {
            return false;
        }
        let Some(provider) = self.provider.clone() else {
            log::warn!("UI 图片 {path} 没有可用的贴图来源，回退为纯色块");
            self.failed.push(path.to_string());
            return false;
        };
        match provider.load_ui_image(path) {
            Ok(image) => {
                match Self::create_texture(device, &self.texture_layout, &self.sampler, &image) {
                    Some(tex) => {
                        self.textures.insert(path.to_string(), tex);
                        true
                    }
                    None => {
                        self.failed.push(path.to_string());
                        false
                    }
                }
            }
            Err(e) => {
                log::warn!("UI 图片加载失败（{path}）：{e}，回退为纯色块");
                self.failed.push(path.to_string());
                false
            }
        }
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u64) {
        if needed <= self.instance_capacity {
            return;
        }
        let mut capacity = self.instance_capacity.max(1);
        while capacity < needed {
            capacity = (capacity * 2).min(MAX_UI_QUADS as u64);
        }
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-ui-instances"),
            size: capacity * std::mem::size_of::<UiInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = capacity;
    }
}

impl UiPass {
    /// 在给定颜色目标上叠加 UI。`screen` 是视口像素尺寸；resize 后无需任何通知，
    /// 每帧用当帧尺寸重新解析所有元素的锚点与百分比。
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        elements: &[UiElement],
        screen: (u32, u32),
    ) {
        if !self.enabled || elements.is_empty() || screen.0 == 0 || screen.1 == 0 {
            self.stats = UiStats {
                enabled: self.enabled,
                textures: self.textures.len() as u32,
                ..Default::default()
            };
            return;
        }

        let screen_f = (screen.0 as f32, screen.1 as f32);
        let build = plan_ui(elements, screen_f);
        if build.batches.is_empty() {
            self.stats = UiStats {
                enabled: true,
                elements: build.elements,
                skipped: build.skipped,
                textures: self.textures.len() as u32,
                ..Default::default()
            };
            return;
        }

        // 图片按需上传（只问新出现的路径，失败过的不会再问）；缺图回退白纹理，
        // 元素自己的颜色仍然生效，所以「png 加载失败 → 纯色块」是可预期的降级行为
        let candidates = images_to_load(&build.images, &self.cached_paths(), &self.failed);
        for path in &candidates {
            self.ensure_image(device, path);
        }
        let missing_images = build
            .images
            .iter()
            .filter(|p| !self.textures.contains_key(*p))
            .count() as u32;

        let total = build.quad_count() as u64;
        self.ensure_instance_capacity(device, total);

        let globals = [screen_f.0, screen_f.1, 0.0, 0.0];
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::cast_slice(&globals));

        let mut ranges: Vec<(Option<&str>, std::ops::Range<u32>)> = Vec::new();
        let mut cursor = 0u32;
        for batch in &build.batches {
            let start = cursor;
            queue.write_buffer(
                &self.instance_buffer,
                start as u64 * std::mem::size_of::<UiInstance>() as u64,
                bytemuck::cast_slice(&batch.instances),
            );
            cursor += batch.instances.len() as u32;
            ranges.push((batch.texture.as_deref(), start..cursor));
        }

        let mut draw_calls = 0u32;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-ui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            for (texture, range) in &ranges {
                let bind_group = match texture {
                    Some(path) => self
                        .textures
                        .get(*path)
                        .map(|t| &t.bind_group)
                        .unwrap_or(&self.white.bind_group),
                    None => &self.white.bind_group,
                };
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..6, range.clone());
                draw_calls += 1;
            }
        }

        self.stats = UiStats {
            enabled: true,
            elements: build.elements,
            quads: total as u32,
            draw_calls,
            textures: self.textures.len() as u32,
            missing_images,
            skipped: build.skipped,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::ui::{Anchor, UiValue};

    fn rect(key: &str, anchor: Anchor, x: f32, y: f32, w: f32, h: f32) -> UiElement {
        let mut el = UiElement::new(key, UiKind::Rect, anchor);
        el.offset = [UiValue::px(x), UiValue::px(y)];
        el.size = [UiValue::px(w), UiValue::px(h)];
        el
    }

    /// 矩形 → 1 个实例，像素位置与尺寸就是解析后的矩形。
    #[test]
    fn rect_makes_one_quad() {
        let build = plan_ui(
            &[rect("a", Anchor::TopLeft, 10.0, 20.0, 100.0, 50.0)],
            (800.0, 600.0),
        );
        assert_eq!(build.quad_count(), 1);
        assert_eq!(build.elements, 1);
        assert!(build.images.is_empty());
        let inst = &build.batches[0].instances[0];
        assert_eq!(inst.origin, [10.0, 20.0]);
        assert_eq!(inst.size, [100.0, 50.0]);
        assert_eq!(inst.uv_min, [0.0, 0.0]);
        assert_eq!(inst.uv_max, [1.0, 1.0]);
    }

    /// 百分比尺寸的矩形在 resize 后自动重算（同一份场景数据，两个屏幕尺寸）。
    #[test]
    fn percent_rect_follows_screen_size() {
        let mut el = UiElement::new("hp", UiKind::Rect, Anchor::BottomCenter);
        el.size = [UiValue::pct(50.0), UiValue::px(20.0)];
        el.offset = [UiValue::px(0.0), UiValue::px(-10.0)];

        let small = plan_ui(std::slice::from_ref(&el), (800.0, 600.0));
        let i = &small.batches[0].instances[0];
        assert_eq!(i.size[0], 400.0);
        assert_eq!(i.origin, [200.0, 570.0]);

        let big = plan_ui(std::slice::from_ref(&el), (1600.0, 900.0));
        let i = &big.batches[0].instances[0];
        assert_eq!(i.size[0], 800.0);
        assert_eq!(i.origin, [400.0, 870.0]);
    }

    /// 边框 → 4 个实例（上下整条 + 左右中段），粗细超限时夹到短边一半。
    #[test]
    fn border_makes_four_quads_and_clamps_thickness() {
        let mut el = UiElement::new("bd", UiKind::Border, Anchor::TopLeft);
        el.offset = [UiValue::px(0.0), UiValue::px(0.0)];
        el.size = [UiValue::px(100.0), UiValue::px(50.0)];
        el.border_width = 4.0;
        let build = plan_ui(std::slice::from_ref(&el), (800.0, 600.0));
        assert_eq!(build.quad_count(), 4);

        // 细矩形：粗细 10 夹到 height/2 = 5，左右中段高度为 0 不再生成
        el.size = [UiValue::px(100.0), UiValue::px(10.0)];
        el.border_width = 10.0;
        let build = plan_ui(std::slice::from_ref(&el), (800.0, 600.0));
        assert_eq!(build.quad_count(), 2, "中段高度为 0 时应只剩上下两条");

        // 粗细 0 → 不画
        el.border_width = 0.0;
        let build = plan_ui(std::slice::from_ref(&el), (800.0, 600.0));
        assert_eq!(build.quad_count(), 0);
        assert_eq!(build.skipped, 1);
    }

    /// 层级：小的先画；同层保持加入顺序（稳定排序在 render 层也成立）。
    #[test]
    fn layers_are_drawn_in_order() {
        let mut a = rect("a", Anchor::TopLeft, 0.0, 0.0, 10.0, 10.0);
        a.layer = 10;
        let mut b = rect("b", Anchor::TopLeft, 0.0, 0.0, 10.0, 10.0);
        b.layer = -1;
        let mut c = rect("c", Anchor::TopLeft, 0.0, 0.0, 10.0, 10.0);
        c.layer = 10;
        a.color = [1.0, 0.0, 0.0, 1.0];
        b.color = [0.0, 1.0, 0.0, 1.0];
        c.color = [0.0, 0.0, 1.0, 1.0];

        let build = plan_ui(&[a, b, c], (800.0, 600.0));
        let colors: Vec<[f32; 4]> = build.batches[0].instances.iter().map(|i| i.color).collect();
        assert_eq!(
            colors,
            vec![
                [0.0, 1.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0]
            ]
        );
    }

    /// 图片元素：带 uv、按路径归批；相同路径的连续批次会合并成一次 draw。
    #[test]
    fn image_batches_group_by_texture() {
        let mut a = UiElement::new("icon", UiKind::Image, Anchor::TopRight);
        a.image = "assets/ui/icon.png".into();
        a.offset = [UiValue::px(-16.0), UiValue::px(16.0)];
        a.size = [UiValue::px(32.0), UiValue::px(32.0)];
        a.uv = [0.0, 0.0, 0.5, 0.5];

        let mut b = a.clone();
        b.key = "icon2".into();
        b.offset = [UiValue::px(-64.0), UiValue::px(16.0)];

        let plain = rect("solid", Anchor::TopLeft, 0.0, 0.0, 10.0, 10.0);

        let build = plan_ui(&[a, b, plain], (800.0, 600.0));
        assert_eq!(build.quad_count(), 3);
        assert_eq!(build.batches.len(), 2, "两张同图应合并为一个批次");
        assert_eq!(
            build.batches[0].texture.as_deref(),
            Some("assets/ui/icon.png")
        );
        assert_eq!(build.batches[0].instances.len(), 2);
        assert_eq!(build.batches[0].instances[0].uv_max, [0.5, 0.5]);
        assert_eq!(build.batches[1].texture, None);
        assert_eq!(build.images, vec!["assets/ui/icon.png".to_string()]);
    }

    /// 不可见元素、零尺寸元素都不产生实例，也不 panic。
    #[test]
    fn hidden_and_degenerate_elements_are_skipped() {
        let mut hidden = rect("h", Anchor::Center, 0.0, 0.0, 10.0, 10.0);
        hidden.visible = false;
        let zero = rect("z", Anchor::Center, 0.0, 0.0, 0.0, 10.0);
        let build = plan_ui(&[hidden, zero], (800.0, 600.0));
        assert_eq!(build.quad_count(), 0);
        assert_eq!(build.elements, 0);
        assert_eq!(build.skipped, 1, "只有尺寸非法的那个算被跳过");

        // 空列表 / 零屏幕尺寸不 panic
        assert_eq!(plan_ui(&[], (800.0, 600.0)).quad_count(), 0);
        assert_eq!(
            plan_ui(
                &[rect("a", Anchor::Center, 0.0, 0.0, 10.0, 10.0)],
                (0.0, 0.0)
            )
            .quad_count(),
            0
        );
    }

    /// 实例上限：超出上限的元素被跳过并计数，不会画出半个边框。
    #[test]
    fn quad_budget_drops_extras() {
        let mut el = UiElement::new("bd", UiKind::Border, Anchor::TopLeft);
        el.size = [UiValue::px(100.0), UiValue::px(100.0)];
        el.border_width = 2.0;
        let build = plan_ui_limited(&[el.clone(), el.clone()], (800.0, 600.0), 5);
        assert_eq!(build.quad_count(), 5);
        assert_eq!(build.skipped, 1);
        assert_eq!(build.elements, 2);
    }

    /// 完整 HUD 场景（血条两层 + 对话框底框 + 图标）不 panic 且批次数合理。
    #[test]
    fn hud_scene_plans_cleanly() {
        let mut bg = rect("hp_bg", Anchor::TopLeft, 24.0, 24.0, 240.0, 24.0);
        bg.color = [0.0, 0.0, 0.0, 0.6];
        let mut fill = rect("hp_fill", Anchor::TopLeft, 26.0, 26.0, 160.0, 20.0);
        fill.layer = 1;
        fill.color = [0.8, 0.15, 0.15, 0.9];
        let mut dlg = rect("dlg", Anchor::BottomCenter, 0.0, -32.0, 720.0, 140.0);
        dlg.size = [UiValue::pct(70.0), UiValue::px(140.0)];
        dlg.color = [0.05, 0.05, 0.08, 0.75];
        let mut frame = UiElement::new("dlg_frame", UiKind::Border, Anchor::BottomCenter);
        frame.offset = [UiValue::px(0.0), UiValue::px(-32.0)];
        frame.size = [UiValue::pct(70.0), UiValue::px(140.0)];
        frame.border_width = 2.0;
        frame.layer = 2;
        let mut icon = UiElement::new("icon", UiKind::Image, Anchor::TopRight);
        icon.offset = [UiValue::px(-24.0), UiValue::px(24.0)];
        icon.size = [UiValue::px(48.0), UiValue::px(48.0)];
        icon.image = "assets/ui/icon.png".into();
        icon.layer = 3;

        let build = plan_ui(&[bg, fill, dlg, frame, icon], (1280.0, 720.0));
        assert_eq!(build.elements, 5);
        assert_eq!(build.quad_count(), 1 + 1 + 1 + 4 + 1);
        assert_eq!(build.skipped, 0);
        // 纯色元素（血条两层 + 对话框 + 边框）相邻且纹理相同 → 合并为一批；
        // 图标层级最高且在最后，单独成批
        assert_eq!(build.batches.len(), 2);
        assert_eq!(build.batches[0].texture, None);
        assert_eq!(build.batches[0].instances.len(), 7);
        assert_eq!(
            build.batches[1].texture.as_deref(),
            Some("assets/ui/icon.png")
        );
    }

    /// 图片失败 / 缓存命中：不重复向 provider 请求，缺图元素走纯色回退。
    #[test]
    fn images_are_requested_once_per_path() {
        let needed = vec![
            "a.png".to_string(),
            "b.png".to_string(),
            "a.png".to_string(),
        ];
        // 首帧：两张都要请求（去重）
        assert_eq!(
            images_to_load(&needed, &[], &[]),
            vec!["a.png".to_string(), "b.png".to_string()]
        );
        // b 已失败、a 已缓存：都不再请求
        assert!(images_to_load(&needed, &["a.png".to_string()], &["b.png".to_string()]).is_empty());
        // 只有 a 缓存：只问 b
        assert_eq!(
            images_to_load(&needed, &["a.png".to_string()], &[]),
            vec!["b.png".to_string()]
        );
    }

    /// provider 返回错误时，回退路径必须可观测（未知路径不进缓存、只进失败名单）。
    #[test]
    fn failing_provider_is_recorded_once() {
        struct AlwaysFail;
        impl UiImageProvider for AlwaysFail {
            fn load_ui_image(&self, path: &str) -> Result<TextureImage, String> {
                Err(format!("{path} 不存在"))
            }
        }
        let provider = AlwaysFail;
        assert!(provider.load_ui_image("missing.png").is_err());
        // 失败名单里的路径不会再被请求（与 UiPass::ensure_image 的判定一致）
        let failed = vec!["missing.png".to_string()];
        assert!(images_to_load(&["missing.png".to_string()], &[], &failed).is_empty());
    }
}
