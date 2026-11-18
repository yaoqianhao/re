use std::collections::HashMap;
use std::sync::Arc;

use r3e_core::mesh::{MaterialData, MeshProvider};
use r3e_core::AssetId;

use crate::pbr::{MaterialUniform, PbrMapFlags};

use super::*;

/// 一个材质在 GPU 侧的状态。
pub(crate) struct GpuMaterial {
    pub(crate) bind_group: wgpu::BindGroup,
    pub(crate) _uniform: wgpu::Buffer,
}

/// 材质缓存：材质键 → bind group；贴图按 `(资产, 贴图下标)` 单独缓存，避免重复上传。
///
/// 任何一步取不到数据（材质缺失 / 贴图解码失败 / 超出纹理尺寸限制）都不会 panic，
/// 而是回退到「默认白色 + 纯色」的兜底材质，保证画面继续跑。
pub(crate) struct MaterialCache {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) sampler: wgpu::Sampler,
    /// 扩展 S4-A：平铺用的 Repeat 采样器。`sampler` 是 ClampToEdge（存量画面靠它
    /// 保证 uv 越界时不出现接缝），平铺场景必须要 Repeat，所以两者并存、按 uv_scale 分支。
    pub(crate) repeat_sampler: wgpu::Sampler,
    pub(crate) white: wgpu::TextureView,
    /// 未挂法线贴图时绑定的中性法线（0.5, 0.5, 1.0）。着色器按开关跳过采样，
    /// 这里只是让 bind group 永远填满 —— 万一开关与绑定不同步，中性法线也不会改外观。
    pub(crate) flat_normal: wgpu::TextureView,
    /// 扩展二十三：HDR 反射探针的立方体视图。没有探针时是 1×1 全黑兜底图；
    /// 换探针时整个材质缓存被清空重建，因此这里存的是「当前这一份」。
    pub(crate) env_cube: wgpu::TextureView,
    /// 兜底黑立方体（卸载探针时用它填回 binding 6）。
    pub(crate) fallback_env_cube: wgpu::TextureView,
    pub(crate) materials: HashMap<MaterialKey, GpuMaterial>,
    pub(crate) textures: HashMap<(AssetId, u32, TextureColorSpace), wgpu::TextureView>,
    pub(crate) max_texture_dim: u32,
}

impl MaterialCache {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("r3e-material-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // 扩展二十一：法线 / 粗糙度 / AO 三张数据贴图，复用 binding(2) 的采样器
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 扩展二十三：HDR 反射探针立方体贴图。`Rgba16Float` + 线性过滤，
                // 因此必须是 `Float { filterable: true }`；没有探针时绑 1×1 全黑兜底图，
                // 绑定组永远填满，着色器靠 `env_probe.x` 决定要不要采样。
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                // 扩展 S4-A：自发光贴图（sRGB 颜色图，复用 binding(2) 的采样器）。
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 扩展 S4-A：平铺采样器（Repeat）。base color 与数据贴图的正片叠底
                // 都靠它，uv_scale = 1 时着色器不用这个 binding，存量画面不动。
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-material-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // 扩展 S4-A：与上面同参数，只有寻址方式换成 Repeat —— 平铺时 uv 会超出 0..1，
        // ClampToEdge 会把边缘像素拉成条带，Repeat 才是正确的平铺语义。
        let repeat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("r3e-material-repeat-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let white = create_white_texture(device);
        let flat_normal = create_flat_normal_texture(device, queue);
        let env_cube = create_black_cube_texture(device, queue);
        let fallback_env_cube = env_cube.clone();

        let mut cache = Self {
            layout,
            sampler,
            repeat_sampler,
            white,
            flat_normal,
            env_cube,
            fallback_env_cube,
            materials: HashMap::new(),
            textures: HashMap::new(),
            max_texture_dim: 8192,
        };
        cache.max_texture_dim = device.limits().max_texture_dimension_2d;
        cache
    }

    pub(crate) fn get(&self, key: MaterialKey) -> Option<&GpuMaterial> {
        self.materials.get(&key)
    }

    /// 扩展二十三：换探针立方体。bind group 里的视图是写死的，所以换图必须
    /// 让所有材质重新建组 —— 探针只在场景加载时换一次，这个代价可以接受。
    pub(crate) fn set_env_cube(&mut self, view: wgpu::TextureView) {
        self.env_cube = view;
        self.materials.clear();
    }

    /// 扩展二十三：卸探针，回到兜底黑立方体。同样清空材质缓存。
    pub(crate) fn reset_env_cube(&mut self) {
        self.env_cube = self.fallback_env_cube.clone();
        self.materials.clear();
    }

    /// 确保材质已就绪；失败时退回默认材质（调用方永远能拿到一组 bind group）。
    pub(crate) fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        library: Option<&Arc<dyn MeshProvider>>,
        key: MaterialKey,
    ) {
        if self.materials.contains_key(&key) {
            return;
        }

        let (asset_material, surface, scene_material) = key.split();
        // S4-A：场景材质是**完整材质**（标量 + 五张贴图 + uv），不参与资产材质继承，
        // 也不吃物件级覆盖 —— 它取代整份数据。其余键仍走原来的「资产 → 物件覆盖」链。
        let data = match scene_material {
            Some(material) => material.to_material_data(),
            None => {
                let base = match (asset_material, library) {
                    (Some((asset, index)), Some(library)) => {
                        library.material(asset, index).unwrap_or_else(|| {
                            log::warn!("资产 {} 缺少材质 {}，回退默认材质", asset, index);
                            MaterialData::default()
                        })
                    }
                    _ => MaterialData::default(),
                };
                // 物件级 PBR 覆盖只改 metallic / roughness，资产的颜色与贴图保持不变
                match surface {
                    Some(surface) => surface.overlay(&base),
                    None => base,
                }
            }
        };

        let (view, has_texture) = match (data.base_color_texture, library) {
            (Some((asset, index)), Some(library)) => {
                match self.texture_view(
                    device,
                    queue,
                    asset,
                    index,
                    library,
                    TextureColorSpace::Srgb,
                ) {
                    Some(view) => (view, true),
                    None => (self.white.clone(), false),
                }
            }
            _ => (self.white.clone(), false),
        };

        // 扩展二十一：三张数据贴图各自解析。开关只认「真的绑上了图」的那张 ——
        // 贴图超限/像素不全会让 texture_view 返回 None，此时开关必须是 0，
        // 否则着色器会去采样我们垫的中性图，材质行为就悄悄变了。
        let (normal_view, normal_map) = self.resolve_pbr_map(
            device,
            queue,
            data.normal_texture,
            library,
            TextureColorSpace::Linear,
            self.flat_normal.clone(),
        );
        let (roughness_view, roughness_map) = self.resolve_pbr_map(
            device,
            queue,
            data.roughness_texture,
            library,
            TextureColorSpace::Linear,
            self.white.clone(),
        );
        let (occlusion_view, occlusion_map) = self.resolve_pbr_map(
            device,
            queue,
            data.occlusion_texture,
            library,
            TextureColorSpace::Linear,
            self.white.clone(),
        );
        // S4-A：自发光贴图是颜色图（sRGB），与上面三张数据图的色彩空间不同。
        let (emissive_view, emissive_map) = self.resolve_pbr_map(
            device,
            queue,
            data.emissive_texture,
            library,
            TextureColorSpace::Srgb,
            self.white.clone(),
        );
        let maps = PbrMapFlags {
            normal: normal_map,
            roughness: roughness_map,
            occlusion: occlusion_map,
            emissive: emissive_map,
        };

        // 自发光强度：场景材质自带一份，物件级覆盖只对旧路径生效
        let emissive = match scene_material {
            Some(material) => material.emissive(),
            None => surface.map(|s| s.emissive()).unwrap_or([0.0; 3]),
        };
        let uniform = MaterialUniform::build_full(
            data.base_color,
            data.metallic,
            data.roughness,
            emissive,
            has_texture,
            maps,
            data.uv_scale,
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-material-ubo"),
            size: std::mem::size_of::<MaterialUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&uniform));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("r3e-material-bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&roughness_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&occlusion_view),
                },
                // 扩展二十三：探针立方体。没有探针时是兜底黑图，不影响画面。
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.env_cube),
                },
                // 扩展 S4-A：自发光贴图 + 平铺采样器
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&emissive_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.repeat_sampler),
                },
            ],
        });

        self.materials.insert(
            key,
            GpuMaterial {
                bind_group,
                _uniform: buffer,
            },
        );
    }

    pub(crate) fn texture_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        asset: AssetId,
        index: u32,
        library: &Arc<dyn MeshProvider>,
        space: TextureColorSpace,
    ) -> Option<wgpu::TextureView> {
        if let Some(view) = self.textures.get(&(asset, index, space)) {
            return Some(view.clone());
        }
        let image = library.texture(asset, index)?;
        let view = upload_texture(device, queue, &image, self.max_texture_dim, space)?;
        log::info!(
            "贴图已上传 GPU: asset {} tex {} — {}x{} ({:?})",
            asset,
            index,
            image.width,
            image.height,
            space
        );
        self.textures.insert((asset, index, space), view.clone());
        Some(view)
    }

    /// 解析一张 PBR 数据贴图：成功返回 (视图, true)，否则返回 (兜底视图, false)。
    ///
    /// 兜底视图的作用只是把 bind group 填满 —— 开关为 false 时着色器根本不采样，
    /// 所以「没图」不等于「画面变了」。
    pub(crate) fn resolve_pbr_map(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: Option<(AssetId, u32)>,
        library: Option<&Arc<dyn MeshProvider>>,
        space: TextureColorSpace,
        placeholder: wgpu::TextureView,
    ) -> (wgpu::TextureView, bool) {
        let (Some((asset, index)), Some(library)) = (slot, library) else {
            return (placeholder, false);
        };
        match self.texture_view(device, queue, asset, index, library, space) {
            Some(view) => (view, true),
            None => (placeholder, false),
        }
    }
}
