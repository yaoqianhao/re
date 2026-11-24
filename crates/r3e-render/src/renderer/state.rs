use std::collections::HashMap;
use std::sync::Arc;

use r3e_core::mesh::MeshProvider;
use r3e_core::{Aabb, CasterTree, EnvironmentSettings, MeshRef, Sh9};

use crate::mesh::InstanceRaw;
use crate::occlusion::OcclusionCuller;
use crate::postfx::PostFxPass;
use crate::ssao::SsaoPass;
use crate::text::{TextPass, TextStats};
use crate::ui::{UiImageProvider, UiPass, UiStats};

use super::*;

pub struct Renderer {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// 扩展四十四：设备能力摘要（纯数据）。**上层唯一可见的设备身份** ——
    /// 它不含任何图形库类型，换 API 时这个字段与它的访问器都不用动。
    /// 原来散在这里的 `AdapterInfo` / `Features` / `Backends` 三个 wgpu 类型
    /// 已随设备获取逻辑一起下沉到 `gfx::backend`。
    pub(crate) gfx_info: crate::gfx::GfxInfo,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) size: (u32, u32),
    pub(crate) pipeline: wgpu::RenderPipeline,
    /// 半透明管线（扩展十九）：与 `pipeline` 同着色器、同 alpha 混合，
    /// 唯一差别是 `depth_write_enabled = false` —— 半透明物件要参与深度测试
    /// （近处仍能挡住远处），但不能把深度写进去，否则会挡掉后画的同类物件。
    pub(crate) transparent_pipeline: wgpu::RenderPipeline,
    /// 深度预通道（扩展十九）：单采样、无颜色目标，只给遮蔽通道供深度。
    pub(crate) depth_prepass_pipeline: wgpu::RenderPipeline,
    pub(crate) camera_buffer: wgpu::Buffer,
    pub(crate) camera_bind_group: wgpu::BindGroup,
    pub(crate) depth_view: wgpu::TextureView,
    /// 单采样深度纹理本体（`depth_view` 的来源）。扩展二十二要把它整张 copy
    /// 到回读缓冲，所以除了 view 还得握着 texture。
    pub(crate) depth_texture: wgpu::Texture,
    /// 遮挡剔除的深度读回与金字塔缓存（扩展二十二）。开关关闭时它一个 GPU
    /// 命令都不产生。
    pub(crate) occlusion: OcclusionCuller,
    /// MSAA 多重采样颜色附件（`MSAA_SAMPLE_COUNT` 样本，`HDR_FORMAT`）。
    /// 主通道写它，pass 结束时 resolve 到 `postfx.hdr_view()` 的单采样目标。
    pub(crate) _msaa_color: wgpu::Texture,
    pub(crate) msaa_color_view: wgpu::TextureView,
    /// MSAA 深度附件。**不 resolve**：深度不能跨样本解析，需要单采样深度的
    /// 遮蔽通道改吃深度预通道写出的 `depth_view`。
    pub(crate) _msaa_depth: wgpu::Texture,
    pub(crate) msaa_depth_view: wgpu::TextureView,
    pub(crate) mesh_cache: MeshCache,
    pub(crate) material_cache: MaterialCache,
    pub(crate) instance_buffer: wgpu::Buffer,
    pub(crate) instance_capacity: u64,
    /// 扩展三十一：**上一帧真正传上 GPU 的实例记录**的影子副本。
    /// 脏段判定拿它和本帧的 `all` 逐字节比，所以它必须与 `instance_buffer` 的
    /// 内容严格同步 —— 缓冲换代（`instance_capacity` 扩容）时必须一起清空，
    /// 否则会把「新缓冲里的未初始化字节」当成已上传内容。
    pub(crate) instance_shadow: Vec<InstanceRaw>,
    /// 扩展三十一：本帧的上传读数，`render` 收尾时抄进 `RenderStats`。
    pub(crate) instance_upload: InstanceUploadStats,
    /// 扩展三十一：累计换代次数（缓冲重建次数），放渲染器里才不会被每帧
    /// 重建的 `RenderStats` 清零。
    pub(crate) instance_upload_resets: u64,
    /// P4：全体蒙皮实例共用的关节矩阵 storage buffer（容量固定为
    /// `MAX_JOINT_MATRICES` 个 mat4，约 256 KiB）。
    pub(crate) joint_buffer: wgpu::Buffer,
    pub(crate) joint_bind_group: wgpu::BindGroup,
    /// 扩展三十六：全体网格共用的形变增量表（与关节同组，binding 1）。
    /// 容量固定（[`MAX_MORPH_TABLE_FLOATS`]），内容随 `MeshCache::morph_version` 变化重写。
    pub(crate) morph_buffer: wgpu::Buffer,
    /// 已传上 GPU 的表版本。与缓存版本不等才重写 —— 静态场景下这里一帧零开销。
    pub(crate) morph_uploaded_version: u64,
    /// 重写中转：先拷进这里再 `write_buffer`，避免每次借用冲突与临时分配。
    pub(crate) morph_scratch: Vec<f32>,
    /// 形变表累计上传次数与当前规模，进 `RenderStats`。
    pub(crate) morph_uploads: u64,
    pub(crate) morph_floats: u64,
    /// 每帧收集的关节矩阵，扁平存 16 个 f32 / 矩阵，直接 memcpy 上传。
    pub(crate) joint_scratch: Vec<[f32; 16]>,
    /// 扩展三十二：**上一帧真正传上 GPU 的关节矩阵**的影子副本。
    /// 与 `joint_buffer` 内容严格同步；无蒙皮帧（`joint_scratch` 为空）时一起清空，
    /// 下一帧若又有蒙皮就自动退化为全量。
    pub(crate) joint_shadow: Vec<[f32; 16]>,
    /// 扩展三十二：关节矩阵的脏段上传读数。关节缓冲容量固定
    /// （`MAX_JOINT_MATRICES`）不换代，所以这里的 `resets` 恒为 0。
    pub(crate) joint_upload: InstanceUploadStats,
    /// 扩展三十五：暂存环（实例 + 关节共用）。脏段数据先在这里攒成一帧一次写入，
    /// 再由编码器按段块拷贝到目标缓冲；多帧飞行保证 GPU 在读的槽不被覆盖。
    pub(crate) staging: UploadStaging,
    /// 阶段 D / 扩展十六：方向光阴影贴图数组（`MAX_CASCADES` 层）+ 比较采样器。
    pub(crate) shadow_size: u32,
    pub(crate) _shadow_texture: wgpu::Texture,
    /// 整张纹理数组的视图：主通道按级下标采样。
    pub(crate) shadow_view: wgpu::TextureView,
    /// 每层的单独视图：第 i 层作为第 i 级深度 pass 的渲染目标。
    pub(crate) shadow_layer_views: Vec<wgpu::TextureView>,
    pub(crate) shadow_sampler: wgpu::Sampler,
    pub(crate) shadow_layout: wgpu::BindGroupLayout,
    pub(crate) shadow_bind_group: wgpu::BindGroup,
    pub(crate) shadow_uniform: wgpu::Buffer,
    /// 扩展十五：局部光源表 uniform（1 个计数 + 16 盏灯）——随 `shadow_bind_group`
    /// 一起以 group(3) binding(4) 绑定，不单开绑定组。
    pub(crate) lights_buffer: wgpu::Buffer,
    /// 阴影通道专用：每级一份「只含光源正交矩阵」的 uniform 与绑定组
    /// （见 `ShadowPassUniform`）。每级一次 pass，矩阵各自独立。
    pub(crate) shadow_pass_bind_groups: Vec<wgpu::BindGroup>,
    pub(crate) shadow_pass_uniforms: Vec<wgpu::Buffer>,
    /// 扩展十八 C：局部光源阴影图（`MAX_LOCAL_SHADOW_LAYERS` 层）+ 每层视图。
    /// 尺寸固定为 `DEFAULT_LOCAL_SHADOW_SIZE`，不跟方向光的 `shadow.size` 联动。
    pub(crate) local_shadow_size: u32,
    pub(crate) _local_shadow_texture: wgpu::Texture,
    pub(crate) local_shadow_view: wgpu::TextureView,
    pub(crate) local_shadow_layer_views: Vec<wgpu::TextureView>,
    pub(crate) local_shadow_uniform: wgpu::Buffer,
    /// 局部阴影每层一份的「单矩阵」uniform 与绑定组（深度 pass 用）。
    pub(crate) local_shadow_pass_uniforms: Vec<wgpu::Buffer>,
    pub(crate) local_shadow_pass_bind_groups: Vec<wgpu::BindGroup>,
    pub(crate) shadow_pipeline: wgpu::RenderPipeline,
    /// M1 文字渲染：屏幕空间 2D 文本通道，与 3D 主通道完全独立。
    pub(crate) text: TextPass,
    /// M1 最小 UI：屏幕空间 2D UI 通道（矩形 / 边框 / 图片），与文本通道同构但独立。
    pub(crate) ui: UiPass,
    /// 离屏 HDR 目标 + 泛光链 + composite（扩展十三）。常驻：即使场景没有
    /// `postfx` 段，主通道也要先写进 HDR 目标再由它直通到 surface。
    pub(crate) postfx: PostFxPass,
    /// 屏幕空间环境光遮蔽（扩展十七）：吃主通道的深度，产出 AO 图给 composite。
    /// 常驻但只在场景开了 `postfx.ssao` 时提交命令。
    pub(crate) ssao: SsaoPass,
    /// 环境探针（扩展十二）的球谐缓存。`Sh9::project` 要做 2048 次天空采样，
    /// 每帧重算是纯浪费——只在场景 `environment` 段真的变了才重投影。
    pub(crate) env_cache: Option<EnvironmentSettings>,
    pub(crate) env_sh: Sh9,
    /// 累计做过几次球谐投影（只增不减）。正常连续帧里应保持不变——
    /// 若它逐帧增长，说明缓存键比较失效、正在每帧重算。
    pub(crate) env_builds: u64,
    /// 扩展二十三：当前生效的 HDR 反射探针。
    ///
    /// `None` = 没有探针，着色器 `env_probe.x = 0`，镜面走解析天空（画面与扩展十二一致）。
    /// `Some` 里握着 GPU 立方体的所有权（view 已交给材质缓存）与本帧统计。
    pub(crate) env_probe: Option<EnvProbeGpu>,
    /// 扩展二十五：遮挡体的层级树缓存。
    ///
    /// 拓扑（父子关系）随场景结构走，只在 `Scene::layout_version` 前进时重建；
    /// 每帧只刷新各节点的实例包围盒并重聚合子树盒。零 GPU 依赖，纯 CPU 判定。
    pub(crate) caster_tree: Option<CasterTree>,
    /// 节点 id → 树节点下标（与 `caster_tree` 同寿命）。
    pub(crate) caster_slots: HashMap<r3e_core::NodeId, usize>,
    /// `caster_slots` / `caster_tree` 对应的场景结构版本。
    pub(crate) caster_layout: u64,
    /// 本帧各节点自身的实例包围盒（长度 = 树节点数，逐帧覆写）。
    pub(crate) caster_node_bounds: Vec<Aabb>,
    /// 本帧各节点自身的实例数（长度 = 树节点数，逐帧覆写）。
    pub(crate) caster_node_casts: Vec<u32>,
    /// 本帧实例 → 树节点下标的映射（与实例缓冲同序；映射不到的填 `usize::MAX`，
    /// 由 `expand_masks` 的 fail-open 兜成全保留）。
    pub(crate) caster_instance_nodes: Vec<usize>,
    /// 累计重建层级树的次数（只增不减，观测缓存是否真的生效）。
    pub(crate) caster_tree_builds: u64,
    /// 扩展二十九：主通道视锥剔除的层级树缓存。
    ///
    /// 与 `caster_tree` **分开一棵**：那棵只装「进桶的遮挡体」（被视锥剔掉的实例根本
    /// 不在里面），这棵要装「场景里所有可见节点」—— 整枝预筛必须看得见会被剔的实例，
    /// 否则子树盒会缩水成「只剩可见实例」，反而把本该保留的枝剪掉。两者语义不同，
    /// 共用一棵会互相踩。拓扑同样只在 `Scene::layout_version` 前进时重建。
    pub(crate) cull_tree: Option<CasterTree>,
    /// 节点 id → 视锥树节点下标（与 `cull_tree` 同寿命）。
    pub(crate) cull_slots: HashMap<r3e_core::NodeId, usize>,
    /// `cull_tree` / `cull_slots` 对应的场景结构版本。
    pub(crate) cull_layout: u64,
    /// 本帧各节点自身的实例包围盒（长度 = 树节点数，逐帧覆写）。
    pub(crate) cull_node_bounds: Vec<Aabb>,
    /// 本帧各节点自身的实例数（长度 = 树节点数，逐帧覆写）。
    pub(crate) cull_node_casts: Vec<u32>,
    /// 累计重建主通道视锥树的次数。
    pub(crate) cull_tree_builds: u64,
    /// M2-C03b 地形通道：场景有地形时由运行时把「本帧该画什么」递进来，这里只负责
    /// 上传缓冲与往主通道提交。`None` = 场景没有地形，此时**连管线都不建** ——
    /// 地形是可选内容，不该让不用它的场景多付一份常驻成本。
    pub(crate) terrain: Option<crate::terrain::TerrainPass>,
    /// 待上传的地形几何。`Some` = 有脏数据（首帧 / LOD 变化 / 换场景），
    /// `None` = 缓冲已是最新。逐帧比对上万顶点去判等是白费，判重交给装配层。
    pub(crate) terrain_dirty: Option<crate::terrain::TerrainDrawItem>,
    /// 地形材质层参数（4 层基色 + shaping）。每帧随相机一起重传，代价是一次 96 字节写。
    pub(crate) terrain_layers: crate::terrain::TerrainLayersUniform,
    pub(crate) stats: RenderStats,
}

impl Renderer {
    /// HDR 后处理链（扩展十三）：离屏目标尺寸、泛光层级、最近一帧生效的设置。
    pub fn postfx(&self) -> &PostFxPass {
        &self.postfx
    }

    /// 屏幕空间遮蔽通道（扩展十七）：遮蔽图尺寸、本帧统计、最近一帧生效的设置。
    pub fn ssao(&self) -> &SsaoPass {
        &self.ssao
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 扩展四十四：设备能力摘要（纯数据）。**上层唯一允许看到的设备身份**。
    ///
    /// 这里刻意不返回 `wgpu::AdapterInfo`/`Features`/`Backends`：一旦返回，
    /// 「引擎不绑定单一图形 API」就只剩口号 —— 宿主会被迫认识 wgpu 的类型，
    /// 而下一个后端得伪造一个假 `AdapterInfo` 才能满足签名。
    pub fn gfx_info(&self) -> &crate::gfx::GfxInfo {
        &self.gfx_info
    }

    /// 扩展四十四：当前使用的图形 API。今天是 [`crate::gfx::GfxApi::Wgpu`]；
    /// 将来接入新后端时这里会如实返回新的身份，宿主分支无需改签名。
    pub fn gfx_api(&self) -> crate::gfx::GfxApi {
        self.gfx_info.api
    }

    /// 一行能力摘要，供启动日志与真机验收抓取。
    ///
    /// 形如 `api=wgpu backend=dx12 device=NVIDIA GeForce RTX 5060 Laptop GPU (discrete)
    /// 特性 timestamp-query|polygon-mode-line / 缺 timestamp-in-encoders 要求 dx12`。
    /// **缺特性不视为失败**：这些是可选能力，引擎在空特性集下也应有像素一致的结果。
    pub fn caps_line(&self) -> String {
        self.gfx_info.caps_line()
    }

    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }

    /// 网格局部包围盒（几何空间），供宿主做屏幕射线拾取（扩展三十四）。
    ///
    /// 和视锥剔除共用 `MeshCache` 的那份缓存：剔除已经算过一次的盒子，
    /// 拾取不会重新扫顶点。`None` 表示提供者不带包围盒数据 —— 调用方应把这
    /// 个网格当作「不可拾取」，而不是退回原点或空盒（否则会点中不存在的体积）。
    pub fn mesh_aabb(&mut self, key: MeshRef) -> Option<Aabb> {
        self.mesh_cache.bounds(key)
    }

    /// 环境探针的累计球谐投影次数。连续帧之间不该增长（缓存命中），
    /// 供集成测试与遥测确认「没有每帧重算」。
    pub fn environment_builds(&self) -> u64 {
        self.env_builds
    }

    /// 当前生效的环境探针 `Some` = 用 IBL，`None` = 用 `light.ambient` 常数项。
    pub fn environment(&self) -> Option<EnvironmentSettings> {
        self.env_cache
    }

    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// 文本通道（只读）：查统计、查字体来源。
    pub fn text(&self) -> &TextPass {
        &self.text
    }

    /// 文本通道（可变）：开关、换字体。
    pub fn text_mut(&mut self) -> &mut TextPass {
        &mut self.text
    }

    pub fn text_stats(&self) -> TextStats {
        self.text.stats()
    }

    /// 独立开关 2D 文本通道；关掉后主通道 / 阴影通道行为完全不变。
    pub fn set_text_enabled(&mut self, enabled: bool) {
        self.text.set_enabled(enabled);
    }

    /// 用指定字体文件（可多个）替换文本字体集，CJK 字体走这里。
    pub fn load_text_fonts(&mut self, paths: &[std::path::PathBuf]) -> Result<(), String> {
        self.text.load_fonts(paths)
    }

    /// UI 通道（只读）：查统计、查已失败的图片。
    pub fn ui(&self) -> &UiPass {
        &self.ui
    }

    /// UI 通道（可变）：开关、注入图片提供者、清图片缓存。
    pub fn ui_mut(&mut self) -> &mut UiPass {
        &mut self.ui
    }

    pub fn ui_stats(&self) -> UiStats {
        self.ui.stats()
    }

    /// 独立开关 2D UI 通道；关掉后主通道 / 阴影 / 文本行为完全不变。
    pub fn set_ui_enabled(&mut self, enabled: bool) {
        self.ui.set_enabled(enabled);
    }

    /// 注入 UI 图片解码器（应用层复用 asset 的 image 依赖实现）。
    pub fn set_ui_image_provider(&mut self, provider: std::sync::Arc<dyn UiImageProvider>) {
        self.ui.set_image_provider(provider);
    }

    /// 注入资产库（由应用层加载 FBX 后调用）。渲染层本身不认识文件 IO。
    pub fn set_asset_library(&mut self, library: Arc<dyn MeshProvider>) {
        self.mesh_cache.set_library(library);
        // 资产库换了，材质 / 贴图句柄可能失效，整批丢弃重来。
        self.material_cache.materials.clear();
        self.material_cache.textures.clear();
    }

    pub fn mesh_cache(&self) -> &MeshCache {
        &self.mesh_cache
    }
}
