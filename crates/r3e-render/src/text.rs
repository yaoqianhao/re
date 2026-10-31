//! 屏幕空间 2D 文本通道。
//!
//! 设计要点（M1「文字渲染」）：
//!
//! - **与 3D 通道解耦**：自带 pipeline / bind group / 顶点布局，不碰相机 uniform、
//!   深度缓冲和阴影贴图；`TextPass::set_enabled` 可整体关掉，关掉后主通道行为与
//!   加文本前完全一致。
//! - **只用场景真源**：每帧按 `Scene::texts` 重新排版、重新取字形，脚本改文本后
//!   下一帧即生效，和物件走同一套「场景是唯一真源」的规则。
//! - **字体解析在 core**：这里只依赖 `r3e_core::GlyphSource` 抽象拿度量与位图，
//!   core 侧不会因为字形分析而引入 wgpu；本层只负责「图集文本上传 + 绘制」。
//! - **图集按需增长**：`GlyphAtlas` 货架式分配，页满自动开新页，页数上限
//!   [`MAX_ATLAS_PAGES`]。

use std::path::PathBuf;

use bytemuck::{Pod, Zeroable};
use r3e_core::glyph_atlas::{GlyphAtlas, GlyphKey, DEFAULT_PAGE_SIZE};
use r3e_core::text::{layout_text, sanitize_size};
use r3e_core::{GlyphSource, ScreenText};

/// 图集页数上限（每页 1024x1024 R8 ≈ 1 MiB，8 页 ≈ 8 MiB）。
pub const MAX_ATLAS_PAGES: u16 = 8;
/// 文本实例缓冲初始容量（一个实例 = 一个字形四边形）。
const INITIAL_TEXT_INSTANCES: u64 = 512;
/// 单帧字形实例上限，防脚本刷出天文数字文本把显存打满。
pub const MAX_TEXT_INSTANCES: u32 = 32_768;
/// 图集无填充时，uv 各向内收半个纹素，避免线性采样吃到邻格字形。
const UV_INSET: f32 = 0.5;

/// 一个字形四边形（实例数据），与 `text.wgsl` 的 `Instance` 一一对应。
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct TextInstance {
    /// 左上角（屏幕像素，原点在左上角）
    pub origin: [f32; 2],
    /// 宽高（像素）
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: [f32; 4],
}

impl TextInstance {
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

/// 一段待上传的字形位图（新分配到图集时才会产生）。
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphUpload {
    pub page: usize,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub data: Vec<u8>,
}

/// 纯 CPU 的建批结果：按图集页分组的实例 + 需要上传的字形。
#[derive(Clone, Debug, Default)]
pub struct TextBuild {
    /// 下标即图集页号，每页一组实例。
    pub pages: Vec<Vec<TextInstance>>,
    pub uploads: Vec<GlyphUpload>,
    /// 本帧实际产生的字形四边形数。
    pub glyphs: u32,
    /// 缺字（该字符所有字体面都没有）数量，只占位不绘制。
    pub missing: u32,
    /// 图集放不下而丢弃的字形数（页数或尺寸超限）。
    pub overflow: u32,
}

impl TextBuild {
    /// 本帧实例总数。
    pub fn instance_count(&self) -> u32 {
        self.pages.iter().map(|p| p.len() as u32).sum()
    }
}

/// 一帧文本的绘制统计。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextStats {
    /// 通道是否开启（关闭时其余字段恒为 0）。
    pub enabled: bool,
    /// 是否有字体可用。
    pub has_font: bool,
    pub glyphs: u32,
    pub draw_calls: u32,
    /// 本帧新上传的字形数。
    pub uploads: u32,
    pub missing: u32,
    pub overflow: u32,
    pub atlas_pages: u32,
    pub atlas_occupancy: f32,
}

/// 把场景里的文本排成字形四边形。
///
/// 纯 CPU、无 GPU 依赖，单测直接跑这个函数就能验证「复用 / 分页 / 缺字」。
/// 关键点：
/// - 同一字符同一字号第二次出现时 `GlyphAtlas::insert` 命中缓存，不再产生上传；
/// - 缺字不 panic、不画东西，只累计 `missing`；
/// - 排版交给 `r3e_core::layout_text`（对齐、换行、UTF-8 边界都在那侧处理）。
pub fn build_text<S: GlyphSource + ?Sized>(
    src: &S,
    texts: &[ScreenText],
    atlas: &mut GlyphAtlas,
) -> TextBuild {
    build_text_limited(src, texts, atlas, MAX_TEXT_INSTANCES)
}

/// 同 [`build_text`]，但可指定单帧字形上限。
///
/// 抽出来只为单测能在几十个字形内触发「超限丢弃」分支——线上永远走
/// [`MAX_TEXT_INSTANCES`]，这条路径不该被测试专用代码污染。
pub fn build_text_limited<S: GlyphSource + ?Sized>(
    src: &S,
    texts: &[ScreenText],
    atlas: &mut GlyphAtlas,
    max_instances: u32,
) -> TextBuild {
    let mut build = TextBuild::default();
    let page_size = atlas.page_size().max(1) as f32;

    for text in texts {
        if !text.visible || text.content.is_empty() {
            continue;
        }
        let px = sanitize_size(text.size);
        let layout = layout_text(src, text);
        build.missing += layout.missing as u32;

        for (line_index, line) in layout.lines.iter().enumerate() {
            let baseline = text.y + layout.ascent + line_index as f32 * layout.line_height;
            for glyph in &line.glyphs {
                if build.glyphs >= max_instances {
                    build.overflow += 1;
                    continue;
                }
                let Some(bitmap) = src.glyph_bitmap(glyph.face, glyph.glyph, px) else {
                    // 度量有、位图取不到：算缺字，不画
                    build.missing += 1;
                    continue;
                };
                if bitmap.is_empty() {
                    // 空格之类：有前进宽度但无像素，正常跳过
                    continue;
                }
                let key = GlyphKey::new(glyph.face, glyph.glyph, px);
                let Some(insert) =
                    atlas.insert(key, bitmap.width, bitmap.height, bitmap.left, bitmap.top)
                else {
                    build.overflow += 1;
                    continue;
                };
                let slot = insert.slot();
                if insert.is_new() {
                    build.uploads.push(GlyphUpload {
                        page: slot.page as usize,
                        x: slot.x,
                        y: slot.y,
                        w: slot.w,
                        h: slot.h,
                        data: bitmap.data,
                    });
                }

                // 笔位置取整到像素，避免线性采样把字糊掉
                let left = (text.x + glyph.x).round() + slot.left as f32;
                let top = baseline + slot.top as f32;
                let instance = TextInstance {
                    origin: [left, top],
                    size: [slot.w as f32, slot.h as f32],
                    uv_min: [
                        (slot.x as f32 + UV_INSET) / page_size,
                        (slot.y as f32 + UV_INSET) / page_size,
                    ],
                    uv_max: [
                        (slot.x as f32 + slot.w as f32 - UV_INSET) / page_size,
                        (slot.y as f32 + slot.h as f32 - UV_INSET) / page_size,
                    ],
                    color: text.color,
                };

                let page = slot.page as usize;
                if build.pages.len() <= page {
                    build.pages.resize(page + 1, Vec::new());
                }
                build.pages[page].push(instance);
                build.glyphs += 1;
            }
        }
    }

    build
}

/// GPU 侧文本通道：图集纹理 + 实例缓冲 + 独立 pipeline。
pub struct TextPass {
    pipeline: wgpu::RenderPipeline,
    atlas_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    pages: Vec<wgpu::Texture>,
    page_bind_groups: Vec<wgpu::BindGroup>,
    atlas: GlyphAtlas,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    fonts: Option<r3e_core::FontSet>,
    enabled: bool,
    stats: TextStats,
}

impl TextPass {
    /// 建管线并尝试加载系统中文字体（失败不致命，只是画不出字）。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r3e-text-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text.wgsl").into()),
        });

        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-text-atlas-bgl"),
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
            label: Some("r3e-text-globals-bgl"),
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
            label: Some("r3e-text-globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-text-globals-bg"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("r3e-text-pipeline-layout"),
            bind_group_layouts: &[Some(&atlas_layout), Some(&globals_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r3e-text-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(TextInstance::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // 屏幕四边形是顺时针拼的，不剔除；文本也不需要背面概念
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            // 文本通道不写深度、不测深度：主通道的深度图与它无关
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
            label: Some("r3e-text-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-text-instances"),
            size: INITIAL_TEXT_INSTANCES * std::mem::size_of::<TextInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let fonts = match r3e_core::FontSet::load_system_default() {
            Ok(set) => {
                log::info!("文本：已加载系统字体 {:?}", set.face_sources());
                Some(set)
            }
            Err(e) => {
                log::warn!("文本：系统字体加载失败，本帧起只排版不绘制 —— {e}");
                None
            }
        };

        Self {
            pipeline,
            atlas_layout,
            sampler,
            globals_buffer,
            globals_bind_group,
            pages: Vec::new(),
            page_bind_groups: Vec::new(),
            atlas: GlyphAtlas::new(DEFAULT_PAGE_SIZE, MAX_ATLAS_PAGES),
            instance_buffer,
            instance_capacity: INITIAL_TEXT_INSTANCES,
            fonts,
            enabled: true,
            stats: TextStats::default(),
        }
    }

    /// 用指定字体文件替换当前字体集（可多次给，先给的优先匹配字形）。
    pub fn load_fonts(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let set = r3e_core::FontSet::load_files(paths)?;
        log::info!("文本：已加载字体 {:?}", set.face_sources());
        self.fonts = Some(set);
        // 换字体后旧图集的槽位与新字体的 glyph 索引不再同义，必须清空重来
        self.reset_atlas();
        Ok(())
    }

    /// 图集感知不到页纹理已经换掉，这里连同 CPU 记账一起重置。
    fn reset_atlas(&mut self) {
        self.atlas = GlyphAtlas::new(DEFAULT_PAGE_SIZE, MAX_ATLAS_PAGES);
        self.pages.clear();
        self.page_bind_groups.clear();
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn has_font(&self) -> bool {
        self.fonts.is_some()
    }

    /// 字体源描述（日志 / 报告用）。
    pub fn font_sources(&self) -> Vec<String> {
        self.fonts
            .as_ref()
            .map(|f| f.face_sources())
            .unwrap_or_default()
    }

    pub fn stats(&self) -> TextStats {
        self.stats
    }

    /// 每页的已用面积比例，调试 / 报告用。
    pub fn atlas_occupancy(&self) -> f32 {
        self.atlas.occupancy()
    }

    pub fn atlas_pages(&self) -> usize {
        self.atlas.page_count()
    }

    /// 让图集页纹理数量跟上 CPU 侧页数（页满后才会新增）。
    fn sync_pages(&mut self, device: &wgpu::Device) {
        let wanted = self.atlas.page_count();
        let size = self.atlas.page_size();
        while self.pages.len() < wanted {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("r3e-text-atlas"),
                size: wgpu::Extent3d {
                    width: size as u32,
                    height: size as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // 单通道覆盖度：R8 足够，显存占用是 RGBA8 的 1/4
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("r3e-text-atlas-bg"),
                layout: &self.atlas_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.pages.push(texture);
            self.page_bind_groups.push(bind_group);
        }
    }

    fn upload_glyphs(&mut self, queue: &wgpu::Queue, uploads: &[GlyphUpload]) {
        for upload in uploads {
            let Some(texture) = self.pages.get(upload.page) else {
                continue;
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: upload.x as u32,
                        y: upload.y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.w as u32),
                    rows_per_image: Some(upload.h as u32),
                },
                wgpu::Extent3d {
                    width: upload.w as u32,
                    height: upload.h as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u64) {
        if needed <= self.instance_capacity {
            return;
        }
        let mut capacity = self.instance_capacity.max(1);
        while capacity < needed {
            capacity = (capacity * 2).min(MAX_TEXT_INSTANCES as u64);
        }
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-text-instances"),
            size: capacity * std::mem::size_of::<TextInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = capacity;
    }

    /// 在给定颜色目标上叠加文本。`screen` 是视口像素尺寸。
    ///
    /// 返回本帧统计；通道关闭或无字体时直接返回默认值且不产生任何 GPU 命令。
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        texts: &[ScreenText],
        screen: (u32, u32),
    ) {
        let Some(fonts) = self.fonts.as_ref() else {
            self.stats = TextStats {
                enabled: self.enabled,
                has_font: false,
                atlas_pages: self.atlas.page_count() as u32,
                ..Default::default()
            };
            return;
        };
        if !self.enabled || texts.is_empty() {
            self.stats = TextStats {
                enabled: self.enabled,
                has_font: true,
                atlas_pages: self.atlas.page_count() as u32,
                atlas_occupancy: self.atlas.occupancy(),
                ..Default::default()
            };
            return;
        }

        let build = build_text(fonts, texts, &mut self.atlas);
        self.sync_pages(device);
        self.upload_glyphs(queue, &build.uploads);

        let total = build.instance_count() as u64;
        let stats_base = TextStats {
            enabled: true,
            has_font: true,
            glyphs: total as u32,
            missing: build.missing,
            overflow: build.overflow,
            uploads: build.uploads.len() as u32,
            atlas_pages: self.atlas.page_count() as u32,
            atlas_occupancy: self.atlas.occupancy(),
            ..Default::default()
        };

        if total == 0 {
            self.stats = stats_base;
            return;
        }

        self.ensure_instance_capacity(device, total);

        // 视口尺寸每帧都写一次（16 字节，代价可忽略），resize 无需额外通知
        let globals = [screen.0.max(1) as f32, screen.1.max(1) as f32, 0.0, 0.0];
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::cast_slice(&globals));

        // 按页摊平写入实例缓冲，记下每页的实例区间
        let mut ranges: Vec<(usize, std::ops::Range<u32>)> = Vec::new();
        let mut cursor = 0u32;
        for (page, instances) in build.pages.iter().enumerate() {
            if instances.is_empty() {
                continue;
            }
            let start = cursor;
            queue.write_buffer(
                &self.instance_buffer,
                start as u64 * std::mem::size_of::<TextInstance>() as u64,
                bytemuck::cast_slice(instances),
            );
            cursor += instances.len() as u32;
            ranges.push((page, start..cursor));
        }

        let mut draw_calls = 0u32;
        {
            // LoadOp::Load：在 3D 主通道已画好的画面上叠加，不清屏
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-text-pass"),
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
            for (page, range) in &ranges {
                let Some(bind_group) = self.page_bind_groups.get(*page) else {
                    continue;
                };
                pass.set_bind_group(0, bind_group, &[]);
                // 6 个顶点拼两个三角形，顶点位置由 vertex_index 推出来
                pass.draw(0..6, range.clone());
                draw_calls += 1;
            }
        }

        self.stats = TextStats {
            draw_calls,
            ..stats_base
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::{GlyphBitmap, GlyphMetric, LineMetrics, TextAlign};

    /// 纯算术假字体：不依赖机器上有没有装中文字体。
    struct FakeFont {
        cjk_width_ratio: f32,
        missing: Vec<char>,
    }

    impl FakeFont {
        fn new() -> Self {
            Self {
                // CJK 近似方块字：1em 宽
                cjk_width_ratio: 1.0,
                missing: vec!['\u{2603}'],
            }
        }
    }

    impl GlyphSource for FakeFont {
        fn glyph_metric(&self, ch: char, px: f32) -> Option<GlyphMetric> {
            if self.missing.contains(&ch) {
                return None;
            }
            let ratio = if (ch as u32) >= 0x2E80 {
                self.cjk_width_ratio
            } else {
                0.5
            };
            Some(GlyphMetric {
                face: 0,
                glyph: (ch as u32 & 0x3FFF) as u16 + 1,
                advance: px * ratio,
            })
        }

        fn glyph_bitmap(&self, _face: u16, _glyph: u16, px: f32) -> Option<GlyphBitmap> {
            let side = px.round().max(1.0) as usize;
            Some(GlyphBitmap {
                width: side as u16,
                height: side as u16,
                left: 0,
                top: -(side as i16),
                advance: px,
                data: vec![200; side * side],
            })
        }

        fn line_metrics(&self, px: f32) -> LineMetrics {
            LineMetrics {
                ascent: px * 0.8,
                descent: -px * 0.2,
                line_gap: 0.0,
            }
        }
    }

    fn text_at(content: &str, x: f32, y: f32, size: f32) -> ScreenText {
        ScreenText::new("t", content, x, y, size)
    }

    fn atlas() -> GlyphAtlas {
        GlyphAtlas::new(256, MAX_ATLAS_PAGES)
    }

    #[test]
    fn builds_one_quad_per_visible_glyph() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let build = build_text(&font, &[text_at("Hi", 10.0, 20.0, 16.0)], &mut atlas);
        assert_eq!(build.glyphs, 2);
        assert_eq!(build.instance_count(), 2);
        assert_eq!(build.uploads.len(), 2);
        // 首字形左上角 = 文本原点 + xmin/顶偏移
        let first = build.pages[0][0];
        assert_eq!(first.size, [16.0, 16.0]);
        assert!(first.origin[0] >= 10.0);
        assert!(first.uv_min[0] < first.uv_max[0]);
    }

    #[test]
    fn repeated_glyphs_reuse_atlas_slot() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let first = build_text(&font, &[text_at("中中中", 0.0, 0.0, 12.0)], &mut atlas);
        assert_eq!(first.glyphs, 3);
        // 同一个字只上传一次，其余复用同一槽位
        assert_eq!(first.uploads.len(), 1);
        assert_eq!(atlas.len(), 1);

        // 第二帧同一段文本：零上传，uv 完全一致
        let second = build_text(&font, &[text_at("中中中", 0.0, 0.0, 12.0)], &mut atlas);
        assert!(second.uploads.is_empty());
        assert_eq!(second.pages[0][0].uv_min, first.pages[0][0].uv_min);
        assert_eq!(atlas.len(), 1);
    }

    #[test]
    fn second_frame_overlapping_text_only_uploads_new_glyphs() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        build_text(&font, &[text_at("AB", 0.0, 0.0, 10.0)], &mut atlas);
        let build = build_text(&font, &[text_at("ABC", 0.0, 0.0, 10.0)], &mut atlas);
        assert_eq!(build.glyphs, 3);
        // A、B 命中缓存，只有 C 是新字形
        assert_eq!(build.uploads.len(), 1);
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn missing_glyph_is_counted_but_not_drawn() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let build = build_text(&font, &[text_at("A\u{2603}B", 0.0, 0.0, 10.0)], &mut atlas);
        assert_eq!(build.glyphs, 2);
        assert_eq!(build.missing, 1);
        assert!(build.uploads.len() <= 2);
    }

    #[test]
    fn utf8_multibyte_never_panics_and_keeps_quads() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let build = build_text(&font, &[text_at("中文😀é", 0.0, 0.0, 14.0)], &mut atlas);
        // 4 个 char（emoji 是单 char）各有位图
        assert_eq!(build.glyphs, 4);
        assert_eq!(atlas.len(), 4);
    }

    #[test]
    fn multiline_text_offsets_baselines() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let build = build_text(&font, &[text_at("A\nB", 0.0, 0.0, 10.0)], &mut atlas);
        assert_eq!(build.glyphs, 2);
        // 两行的 origin.y 相差一个行高
        let first = build.pages[0][0];
        let second = build.pages[0][1];
        assert!((second.origin[1] - first.origin[1] - 10.0).abs() < 0.01);
    }

    #[test]
    fn empty_or_invisible_text_produces_nothing() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let mut hidden = text_at("AB", 0.0, 0.0, 10.0);
        hidden.visible = false;
        let build = build_text(&font, &[hidden, text_at("", 0.0, 0.0, 10.0)], &mut atlas);
        assert_eq!(build.glyphs, 0);
        assert!(build.uploads.is_empty());
        assert_eq!(atlas.len(), 0);
    }

    #[test]
    fn full_atlas_spills_to_second_page() {
        let font = FakeFont::new();
        // 页取最小值 128x128（`GlyphAtlas` 会把更小的请求夹到下限），
        // 字号 60 → 一个字 60x60 + 1px 间隙，一页正好放 4 个（2 列 x 2 行），
        // 8 个字必然撑到第 2 页。
        let mut atlas = GlyphAtlas::new(128, MAX_ATLAS_PAGES);
        let build = build_text(&font, &[text_at("ABCDEFGH", 0.0, 0.0, 60.0)], &mut atlas);
        assert_eq!(build.glyphs, 8);
        assert!(atlas.page_count() >= 2, "页满应自动开新页");
        assert_eq!(build.pages.len(), atlas.page_count());
        assert!(!build.pages[1].is_empty());
    }

    #[test]
    fn text_colors_are_carried_into_instances() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let mut text = text_at("A", 0.0, 0.0, 10.0);
        text.color = [1.0, 0.5, 0.25, 0.75];
        let build = build_text(&font, &[text], &mut atlas);
        assert_eq!(build.pages[0][0].color, [1.0, 0.5, 0.25, 0.75]);
    }

    #[test]
    fn larger_font_size_gets_separate_atlas_entry() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        build_text(&font, &[text_at("A", 0.0, 0.0, 10.0)], &mut atlas);
        let build = build_text(&font, &[text_at("A", 0.0, 0.0, 20.0)], &mut atlas);
        // 字号变了必须重新栅格化，不能复用 10px 的位图
        assert_eq!(build.uploads.len(), 1);
        assert_eq!(atlas.len(), 2);
    }

    /// 对齐换算：同一段文字在同一 wrap 块里，left/center/right 的四边形 x 必须
    /// 依次右移 0 / (块宽-行宽)/2 / (块宽-行宽)，且行数、字形数不变。
    #[test]
    fn align_shifts_quads_within_wrap_block() {
        let font = FakeFont::new();
        let block = 120.0;
        // FakeFont 对 CJK 取 1em 宽：size=20 → 单字形前进 20px，行宽 = 20
        let line_w = 20.0;
        let mut origins = Vec::new();
        for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
            let mut atlas = atlas();
            let mut text = text_at("中", 0.0, 0.0, 20.0);
            text.align = align;
            text.wrap = block;
            let build = build_text(&font, &[text], &mut atlas);
            assert_eq!(build.glyphs, 1);
            origins.push(build.pages[0][0].origin[0]);
        }
        let (left, center, right) = (origins[0], origins[1], origins[2]);
        assert!((center - left - (block - line_w) * 0.5).abs() < 1.0);
        assert!((right - left - (block - line_w)).abs() < 1.0);
    }

    /// 超限：达到单帧字形上限后继续排字只累计 overflow，不 panic、不越界。
    #[test]
    fn glyph_cap_drops_extra_instances_without_panic() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let long: String = "中".repeat(40);
        let build = build_text_limited(&font, &[text_at(&long, 0.0, 0.0, 12.0)], &mut atlas, 4);
        assert_eq!(build.glyphs, 4);
        assert_eq!(build.instance_count(), 4);
        assert_eq!(build.overflow, 36);
        // 只上传实际画出来的那一个字形（同一个字复用同一槽位）
        assert_eq!(build.uploads.len(), 1);
    }

    /// 超长文本（远超屏幕宽度）：不 panic、不截断字符，字形数等于字符数。
    #[test]
    fn very_long_text_does_not_panic() {
        let font = FakeFont::new();
        let mut atlas = atlas();
        let long: String = "中文AB".repeat(800);
        let build = build_text(&font, &[text_at(&long, 0.0, 0.0, 14.0)], &mut atlas);
        assert_eq!(build.glyphs, 3200);
        assert_eq!(build.instance_count(), 3200);
        assert_eq!(build.overflow, 0, "远低于单帧上限，不应丢字");
        // 只有 4 个不同字形进图集，其余全部复用
        assert_eq!(atlas.len(), 4);
    }
}
