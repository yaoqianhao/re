use r3e_core::mesh::TextureImage;
use r3e_core::MAX_CASCADES;

use super::*;

impl Renderer {
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let (depth_texture, depth_view) = create_depth_target(&self.device, width, height);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        // 扩展二十二：金字塔绑的是旧尺寸的深度，换尺寸后必须作废，
        // 否则会拿旧画面的 tile 去剔新画面的物件（误剔）
        self.occlusion.reset();
        // 扩展十九：MSAA 附件与窗口同尺寸，必须一起重建，否则 resolve 目标尺寸对不上
        let (msaa_color, msaa_color_view, msaa_depth, msaa_depth_view) =
            create_msaa_targets(&self.device, width, height);
        self._msaa_color = msaa_color;
        self.msaa_color_view = msaa_color_view;
        self._msaa_depth = msaa_depth;
        self.msaa_depth_view = msaa_depth_view;
        // 后处理的离屏目标必须一起重建：纹理不能原地 resize，
        // 不重建就会继续按旧尺寸采样，画面被拉成旧分辨率的内容。
        self.postfx.resize(&self.device, width, height);
        // 遮蔽图与深度图同尺寸，必须跟着一起换
        self.ssao
            .resize(&self.device, width, height, &self.depth_view);
        log::debug!("窗口尺寸变化: {width}x{height}");
    }
}

/// 创建 1x1 中性法线贴图：(0.5, 0.5, 1.0) 对应切线空间里的「正上方」。
/// 用线性格式上传，避免 0.5 被 sRGB 解码成 0.21 而把法线整体压弯。
pub(crate) fn create_flat_normal_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-flat-normal-1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[128u8, 128, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_white_texture(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-white-1x1"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// 扩展二十三：1×1 全黑立方体贴图，没有探针时占住 binding 6。
///
/// 用 `Rgba16Float` 与真探针同格式，避免切场景时 bind group 的采样类型变化；
/// 六个面都填 0，配合 `env_probe.x = 0` 双保险 —— 即使开关误开也只会得到黑色镜面。
pub(crate) fn create_black_cube_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 6,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-env-cube-fallback"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6u32 {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8; 8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("r3e-env-cube-fallback-view"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// 贴图的色彩空间（扩展二十一）。
///
/// 决定上传成哪种 GPU 格式：颜色类贴图走 `Rgba8UnormSrgb`（采样时自动线性化），
/// 数据类贴图（法线 / 粗糙度 / AO）走 `Rgba8Unorm`（原值直读）。
/// 缓存键带上它，同一张图被当颜色和当数据各用一次时不会串。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TextureColorSpace {
    Srgb,
    Linear,
}

/// 把 RGBA8 像素上传成贴图。`space` 决定是否做 sRGB 自动线性化。
pub(crate) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &TextureImage,
    max_dim: u32,
    space: TextureColorSpace,
) -> Option<wgpu::TextureView> {
    if !image.is_valid() {
        log::warn!(
            "贴图像素数据不完整（{}x{}），跳过",
            image.width,
            image.height
        );
        return None;
    }
    if image.width > max_dim || image.height > max_dim {
        log::warn!(
            "贴图 {}x{} 超过设备上限 {max_dim}，跳过",
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
    let format = match space {
        TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-asset-texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        size,
    );
    Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// 阶段 D / 扩展十六：创建正方形阴影深度**数组**（`MAX_CASCADES` 层，可作渲染目标，
/// 也可被比较采样）。
///
/// 返回「整张数组的视图」与「每层各自的视图」：主通道要 D2Array 视图才能按级取层，
/// 深度 pass 要 D2 单层视图才能把某一层当渲染目标。
pub(crate) fn create_shadow_target(
    device: &wgpu::Device,
    size: u32,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    create_shadow_array(
        device,
        size,
        MAX_CASCADES as u32,
        "r3e-shadow-map",
        "r3e-shadow-array-view",
        "r3e-shadow-layer-view",
    )
}

/// 扩展十八 C：创建局部光源阴影深度数组（`MAX_LOCAL_SHADOW_LAYERS` 层）。
///
/// 与方向光那张图结构相同，但层数由「灯数 × 面数」决定：点光 6 面、聚光 1 面，
/// 每盏灯固定占 6 层的槽位（聚光的其余 5 层留空），这样层号可以纯算术得到。
pub(crate) fn create_local_shadow_target(
    device: &wgpu::Device,
    size: u32,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    create_shadow_array(
        device,
        size,
        r3e_core::MAX_LOCAL_SHADOW_LAYERS as u32,
        "r3e-local-shadow-map",
        "r3e-local-shadow-array-view",
        "r3e-local-shadow-layer-view",
    )
}

/// 创建可写可采样的分层深度数组（方向光级联与局部阴影共用）。
pub(crate) fn create_shadow_array(
    device: &wgpu::Device,
    size: u32,
    layers: u32,
    label: &str,
    array_label: &str,
    layer_label: &str,
) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
    let size = size.max(1);
    let layers = layers.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(array_label),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let layer_views = (0..layers)
        .map(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(layer_label),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    (texture, array_view, layer_views)
}

/// group(3) 的全部资源（方向光阴影 + 局部光源表 + 局部光源阴影）。
///
/// 抽成结构体是因为这张绑定组要在两处创建（启动、`resize_shadow`），
/// 参数列表一长就容易在某一处漏绑某个 binding —— 漏一次就是整帧画面的静默错误。
pub(crate) struct ShadowBindInputs<'a> {
    pub(crate) view: &'a wgpu::TextureView,
    pub(crate) sampler: &'a wgpu::Sampler,
    pub(crate) uniform: &'a wgpu::Buffer,
    pub(crate) lights: &'a wgpu::Buffer,
    pub(crate) local_view: &'a wgpu::TextureView,
    pub(crate) local_uniform: &'a wgpu::Buffer,
}

pub(crate) fn create_shadow_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    inputs: ShadowBindInputs<'_>,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("r3e-shadow-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(inputs.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(inputs.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: inputs.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: inputs.lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(inputs.local_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(inputs.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: inputs.local_uniform.as_entire_binding(),
            },
        ],
    })
}

/// 扩展十九：创建 MSAA 附件组（4× 颜色 + 4× 深度）。
///
/// 返回 `(颜色纹理, 颜色视图, 深度纹理, 深度视图)`。颜色纹理由硬件 resolve 到
/// HDR 目标，因此只要 `RENDER_ATTACHMENT`；深度则完全不参与采样 —— 遮蔽通道要的
/// 单采样深度由深度预通道写进 `create_depth_view` 那张纹理，两者互不干扰。
///
/// 纹理句柄必须随视图一起返回：wgpu 的视图不持有纹理所有权，提前 drop 会让
/// 视图引用悬空，真机上表现为每帧校验失败。
pub(crate) fn create_msaa_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-msaa-color"),
        size,
        mip_level_count: 1,
        sample_count: MSAA_SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: crate::postfx::HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-msaa-depth"),
        size,
        mip_level_count: 1,
        sample_count: MSAA_SAMPLE_COUNT,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth, depth_view)
}

/// 建单采样深度目标，返回「纹理 + 视图」。
///
/// 两个都要：pass 用视图，遮挡剔除（扩展二十二）的整帧 readback 用纹理本体
/// （`copy_texture_to_buffer` 只能针对纹理，不能针对视图）。
pub(crate) fn create_depth_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r3e-depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        // 主通道写这个深度附件，SSAO（扩展十七）还要把它当纹理采样：
        // 少了 TEXTURE_BINDING，遮蔽通道的绑定组会在真机上直接校验失败。
        // 扩展二十二再加 COPY_SRC：整张深度要能被 copy 进回读缓冲。
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}
