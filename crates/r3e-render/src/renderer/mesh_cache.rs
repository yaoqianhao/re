use std::collections::HashMap;
use std::sync::Arc;

use r3e_core::mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage};
use r3e_core::{Aabb, AssetId, MeshRef, ProceduralProvider};

use super::*;

pub struct GpuMesh {
    pub(crate) vertex_buffer: wgpu::Buffer,
    pub(crate) index_buffer: wgpu::Buffer,
    /// 材质分区（P2 起）。至少一段；无材质信息时是整网格一段的 `Default`。
    /// 各段 `index_count` 之和即本网格的三角形索引总数。
    pub(crate) parts: Vec<GpuPart>,
    /// 扩展三十六：该网格在全局形变表里的位置（无形状为 `None`）。
    pub(crate) morph: Option<MeshMorph>,
}

/// 一个网格在全局形变表（`MeshCache::morph_table`）里的登记信息。
///
/// 表是**所有网格共用**的一块扁平 `f32` 数组，每形状内按顶点紧密排列、
/// 每顶点 [`MORPH_VERTEX_FLOATS`] 个 f32（位置 / 法线 / 切线三组增量，扩展三十七）。
/// 实例缓冲只带这三个数，着色器就能自己算下标：
///
/// ```text
/// 顶点 v 的形状 t 位置增量 = table[base + t * stride + v * 9 .. +3]
/// 顶点 v 的形状 t 法线增量 = 同上 +3 .. +3
/// 顶点 v 的形状 t 切线增量 = 同上 +6 .. +3
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MeshMorph {
    /// 表内起点（浮点下标）。
    pub base: u32,
    /// 每个形状占多少浮点（= 顶点数 × [`MORPH_VERTEX_FLOATS`]）。
    pub stride: u32,
    /// 可用形状数（≤ `MAX_MORPH_TARGETS`）。
    pub count: u32,
    /// 权重推到 1.0 时任意顶点的**最大位移长度**（详见 [`MeshMorph::cull_inflate`]）。
    ///
    /// 视锥 / 遮挡剔除用的是静止姿态 AABB，而形变会把顶点推到盒外——和蒙皮同一个坑。
    /// 蒙皮可以按比例外扩（[`SKIN_CULL_MARGIN`]），形变不行：位移量是资产说了算的绝对值，
    /// 与网格尺寸无关（一个 0.1 m 的眼球可以有 0.05 m 的表情位移）。
    pub max_displacement: f32,
    /// 扩展三十八：**逐形状**的峰值位移长度（下标即形状下标，未用的槽位为 0）。
    ///
    /// `max_displacement` 是这份数组的最大值（网格级粗界），剔除时用它更省；
    /// 细算用逐形状表 —— 一个脸只有「眨眼」位移 0.02、「张嘴」位移 0.15 时，
    /// 拿粗界去乘眨眼权重会把包围盒白白撑大七倍，遮挡剔除随之失效。
    pub max_displacements: [f32; r3e_core::MAX_MORPH_TARGETS],
}

impl MeshMorph {
    /// 剔除前要给世界包围盒外扩多少（世界单位）。
    ///
    /// `peak_weight` 取当前各形状权重的绝对值最大值 —— 权重**不做 clamp**（夸张表情
    /// 可以写 2.0，反向表情可以写负值），所以外扩量必须按当前值算，不能写死 1.0。
    /// `scale` 取实例世界矩阵三条轴的缩放模长最大值（非等比缩放下要按最大那条算）。
    ///
    /// 这是**网格级粗界**（按最长的那个形状算）：不知道实例权重分布时用它。
    /// 知道权重时优先用 [`MeshMorph::cull_inflate_weights`]。
    pub fn cull_inflate(&self, peak_weight: f32, scale: f32) -> f32 {
        // 取绝对值：位移方向可能与 +Z 相反，但盒要往两边撑一样多。
        let peak_weight = peak_weight.abs();
        if !self.max_displacement.is_finite()
            || !scale.is_finite()
            || self.max_displacement <= 0.0
            || peak_weight <= 0.0
            || scale <= 0.0
        {
            return 0.0;
        }
        self.max_displacement * peak_weight * scale
    }

    /// 扩展三十八：按**逐实例权重**算外扩量（世界单位）。
    ///
    /// `Σ |w_t| × displacement_t × scale`：小权重的高位移形状不再连累整个包围盒，
    /// 「只眨一下眼」的实例只外扩眨眼那点量。权重与位移都是绝对值参与 —— 反向
    /// 形变同样会把顶点推出静止盒，方向不影响「推多远」。
    ///
    /// 非有限权重按 0 处理（与着色器「非法权重当 0」的口径一致），累计超界时也
    /// 只返回有限值：剔除余量宁可偏大，绝不允许 NaN 把整个视锥判定污染成「全剔除」。
    pub fn cull_inflate_weights(
        &self,
        weights: &[f32; r3e_core::MAX_MORPH_TARGETS],
        scale: f32,
    ) -> f32 {
        if !scale.is_finite() || scale <= 0.0 {
            return 0.0;
        }
        let mut total = 0.0f32;
        for (index, weight) in weights
            .iter()
            .enumerate()
            .take(self.max_displacements.len())
        {
            if index as u32 >= self.count {
                break;
            }
            let displacement = self.max_displacements[index];
            if !weight.is_finite() || !displacement.is_finite() || displacement <= 0.0 {
                continue;
            }
            total += weight.abs() * displacement;
        }
        let inflate = total * scale;
        if inflate.is_finite() {
            inflate.max(0.0)
        } else {
            // 权重夸张到把 f32 顶炸：一律外扩到无穷（fail-open，宁多画不少画）。
            // 绝不能退成 0 —— 那会让「本该被外扩的实例」按静止盒判定而被误剔除。
            f32::MAX
        }
    }
}

/// 全局形变表的容量上限（浮点数）。1 MiB 的 storage buffer。
///
/// 每顶点占 [`MORPH_VERTEX_FLOATS`] = 9 个 f32，所以这张表够容纳
/// **四个形状全部用满的 7,281 个顶点**（`1 << 18 / (4 × 9)`）。真实表情 / 口型资产
/// 的形变顶点通常只有几百到几千个，够用；超了按「无形变」降级并告警（见
/// [`MeshCache::register_morph`]）。
/// 固定容量（不随内容增长重建）换取「绑定组只需建一次」。
pub const MAX_MORPH_TABLE_FLOATS: usize = 1 << 18;

/// GPU 侧的一个材质分区。
#[derive(Debug, Clone, Copy)]
pub(crate) struct GpuPart {
    pub(crate) first_index: u32,
    pub(crate) index_count: u32,
    pub(crate) material: MaterialKey,
}

/// 分层提供者：**资产优先、程序化兜底**。
///
/// 这是「分级分层」在输出侧的唯一实现点——不是两条并行取数路径，而是同一接口上的
/// 有序回退：上层（导入资产）命中就用，未命中落到下层（内置图元）。
pub struct LayeredProvider {
    pub(crate) primary: Arc<dyn MeshProvider>,
    pub(crate) fallback: Arc<dyn MeshProvider>,
}

impl LayeredProvider {
    pub fn new(primary: Arc<dyn MeshProvider>, fallback: Arc<dyn MeshProvider>) -> Self {
        Self { primary, fallback }
    }
}

impl MeshProvider for LayeredProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData> {
        self.primary
            .mesh_data(key)
            .or_else(|| self.fallback.mesh_data(key))
    }

    fn mesh_aabb(&self, key: MeshRef) -> Option<Aabb> {
        self.primary
            .mesh_aabb(key)
            .or_else(|| self.fallback.mesh_aabb(key))
    }

    fn mesh_parts(&self, key: MeshRef) -> Vec<MeshPartData> {
        let parts = self.primary.mesh_parts(key);
        if parts.is_empty() {
            self.fallback.mesh_parts(key)
        } else {
            parts
        }
    }

    fn material(&self, asset: AssetId, index: u32) -> Option<MaterialData> {
        self.primary
            .material(asset, index)
            .or_else(|| self.fallback.material(asset, index))
    }

    fn texture(&self, asset: AssetId, index: u32) -> Option<TextureImage> {
        self.primary
            .texture(asset, index)
            .or_else(|| self.fallback.texture(asset, index))
    }
}

/// 网格缓存：按 `MeshRef` 惰性上传 GPU 网格。
///
/// **单一取数通道**：内置图元与导入资产都走 `provider.mesh_data(key)`，渲染层不再按
/// `MeshRef` 的来源分支。分层只体现在 [`LayeredProvider`] 的有序回退上：
///
/// * 应用层未注入资产库 —— 回退层只有 `ProceduralProvider`（内置图元）；
/// * 注入资产库后 —— 资产优先，未命中再落到内置图元。
///
/// 渲染层不认识文件 IO；`dyn MeshProvider` 定义在 `r3e-core`，因此本 crate 的依赖图里
/// 不含 `ufbx` / `gltf`。
pub struct MeshCache {
    pub(crate) meshes: HashMap<MeshRef, GpuMesh>,
    /// 网格局部包围盒缓存（视锥剔除与上传都要用）。`None` = 该网格确实取不到包围盒。
    ///
    /// 缓存的意义：包围盒是剔除热路径，只做一次哈希查找，避免每帧走 `dyn` 调用。
    pub(crate) bounds: HashMap<MeshRef, Option<Aabb>>,
    /// 唯一的数据来源；永不缺席（最底层是内置图元）。
    pub(crate) provider: Arc<dyn MeshProvider>,
    /// 已注入的资产层（`None` 表示尚未加载任何外部资产）。
    pub(crate) assets: Option<Arc<dyn MeshProvider>>,
    /// 扩展三十六：全局形变增量表，所有带 morph 的网格共用。
    ///
    /// 只在这里累积、不清段：`ensure` 每次上传新网格时把它的形状追加到尾部，
    /// 各网格的 `MeshMorph.base` 因此保持稳定；`set_library` 整表清空并 `bump` 版本，
    /// 渲染层看到版本变化就重传一次（表只有几百 KiB，重传远比维护脏段便宜）。
    pub(crate) morph_table: Vec<f32>,
    /// 表内容的版本号。渲染层拿它和「已传上 GPU 的版本」比，不等才重传。
    pub(crate) morph_version: u64,
}

impl Default for MeshCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshCache {
    /// 未注入资产库时的缓存：只有内置图元层。
    pub fn new() -> Self {
        Self {
            meshes: HashMap::new(),
            bounds: HashMap::new(),
            provider: Arc::new(ProceduralProvider::new()),
            assets: None,
            morph_table: Vec::new(),
            morph_version: 0,
        }
    }

    /// 以给定资产库构造（`LayeredProvider`：资产优先、内置兜底）。
    pub fn with_library(library: Arc<dyn MeshProvider>) -> Self {
        let mut cache = Self::new();
        cache.set_library(library);
        cache
    }

    /// 注入（或替换）资产库；替换时丢弃此前上传的**资产**网格与包围盒，避免引用失效。
    ///
    /// 内置图元的数据与资产库无关，可以保留缓存（避免重复上传）。
    // 渲染器整体在主线程上跑，`MeshProvider` 刻意不加 `Send + Sync`：
    // 内部持有 `Rc<RefCell<..>>` 共享场景/资源，跨线程共享本就不成立。
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn set_library(&mut self, library: Arc<dyn MeshProvider>) {
        self.assets = Some(library.clone());
        self.provider = Arc::new(LayeredProvider::new(
            library,
            Arc::new(ProceduralProvider::new()),
        ));
        // 单一 upload 路径下不再按来源判断能否复用：整表清空，由 `ensure` 重新上传。
        // 代价只是几个内置图元的顶点数据重传（微秒级），换来零来源分支。
        self.meshes.clear();
        self.bounds.clear();
        // 扩展三十六：形变表同样整表作废 —— 旧 `MeshMorph.base` 指向的增量
        // 属于被丢弃的资产，继续留着只会让新网格读到别人的形状。
        self.morph_table.clear();
        self.morph_version = self.morph_version.wrapping_add(1);
    }

    /// 全局形变增量表（扁平 `f32`，布局见 [`MeshMorph`]）。
    pub fn morph_table(&self) -> &[f32] {
        &self.morph_table
    }

    /// 表内容版本号：变化即代表 GPU 侧那份副本已过期。
    pub fn morph_version(&self) -> u64 {
        self.morph_version
    }

    /// 某网格的形变登记（无形状 / 校验未过时为 `None`）。
    pub fn morph_of(&self, key: MeshRef) -> Option<MeshMorph> {
        self.meshes.get(&key).and_then(|mesh| mesh.morph)
    }

    /// 把网格的形状追加进全局表，返回登记信息。
    ///
    /// 三道闸门，任何一道不过都按「没有形变」处理（绝不让错长度 / 非有限的增量
    /// 进缓冲 —— GPU 上越界读是未定义行为，不是 panic）：
    ///
    /// 1. `validate_against(网格顶点数)` —— 长度不等或含 NaN；
    /// 2. 只取前 `MAX_MORPH_TARGETS` 个形状（每实例权重是 `MORPH_WEIGHT_GROUPS` 组 `vec4`）；
    /// 3. 表容量上限（[`MAX_MORPH_TABLE_FLOATS`]）。
    pub(crate) fn register_morph(&mut self, data: &MeshData) -> Option<MeshMorph> {
        if data.morph.is_empty() {
            return None;
        }
        if let Err(issue) = data.morph.validate_against(data.vertices.len()) {
            log::warn!("网格形变数据不可用（{issue}），按无形变处理");
            return None;
        }
        let count = data.morph.len().min(r3e_core::MAX_MORPH_TARGETS);
        let (flat, stride) = data.morph.flatten_limited(count);
        if stride == 0 || flat.is_empty() {
            return None;
        }
        if self.morph_table.len() + flat.len() > MAX_MORPH_TABLE_FLOATS {
            log::warn!(
                "形变表已满（{} + {} > {MAX_MORPH_TABLE_FLOATS}），该网格按无形变处理",
                self.morph_table.len(),
                flat.len()
            );
            return None;
        }
        let base = self.morph_table.len() as u32;
        self.morph_table.extend_from_slice(&flat);
        self.morph_version = self.morph_version.wrapping_add(1);
        // 剔除余量要在登记期一次算定：每帧遍历全部顶点不划算，而增量是静态的。
        // 位置增量的模长最大值 × 权重峰值 × 世界缩放 = 顶点可能跑出静止 AABB 的距离。
        // 扩展三十八：**逐形状**各算一份 —— 权重按形状给，余量也该按形状给。
        let mut max_displacements = [0.0f32; r3e_core::MAX_MORPH_TARGETS];
        for (slot, target) in data.morph.targets.iter().take(count).enumerate() {
            max_displacements[slot] = target.positions.iter().fold(0.0f32, |acc, delta| {
                acc.max((delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt())
            });
        }
        // 网格级粗界 = 逐形状表的最大值（供不知道权重的调用方用，口径与扩展三十七一致）
        let max_displacement = max_displacements.iter().copied().fold(0.0f32, f32::max);
        Some(MeshMorph {
            base,
            stride: stride as u32,
            count: count as u32,
            max_displacement,
            max_displacements,
        })
    }

    /// 当前的数据来源（分层后的组合提供者）。材质 / 贴图也经它取。
    pub fn provider(&self) -> &Arc<dyn MeshProvider> {
        &self.provider
    }

    /// 已注入的资产层。
    pub fn assets(&self) -> Option<&Arc<dyn MeshProvider>> {
        self.assets.as_ref()
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    pub fn get(&self, key: MeshRef) -> Option<&GpuMesh> {
        self.meshes.get(&key)
    }

    /// 网格材质自带的不透明度（`base_color.a` 的最小值）。
    ///
    /// 一个网格可以有多段材质分区：只要其中任一段是半透明的，整网格就必须走
    /// 半透明通道 —— 否则该分区的混合会在不透明段里被深度写入锁死。取最小值
    /// 而非平均值，正是为了不让「一段透明一段不透明」被平均成不透明。
    ///
    /// 无材质分区（内置图元 / 无材质的资产段）按 1.0 计；NaN 也按 1.0 兜底，
    /// 免得一个坏材质把整批物件拽进半透明通道。取不到任何分区时返回 `None`。
    pub fn material_alpha(&self, key: MeshRef) -> Option<f32> {
        let parts = self.provider.mesh_parts(key);
        if parts.is_empty() {
            return None;
        }
        let mut alpha = 1.0_f32;
        for part in &parts {
            let Some((asset, index)) = part.material else {
                continue;
            };
            // 材质取不到时保持 1.0：宁可当成不透明，也不要凭空捏造一个透明度
            if let Some(material) = self.provider.material(asset, index) {
                let value = material.base_color[3];
                if value.is_finite() {
                    alpha = alpha.min(value.clamp(0.0, 1.0));
                }
            }
        }
        Some(alpha)
    }

    /// 网格局部包围盒（几何空间）。取不到时返回 `None`，调用方必须按「不剔除」处理。
    ///
    /// 顺序：缓存 → `mesh_aabb`（零拷贝）→ 兜底 `mesh_data` 现算（仅当提供者没有实现
    /// `mesh_aabb`，且只会发生一次）。
    pub fn bounds(&mut self, key: MeshRef) -> Option<Aabb> {
        if let Some(cached) = self.bounds.get(&key) {
            return *cached;
        }
        let provider = self.provider.clone();
        let computed = provider
            .mesh_aabb(key)
            .or_else(|| provider.mesh_data(key).map(|data| data.aabb_box()))
            .filter(|aabb| !aabb.is_empty());
        self.bounds.insert(key, computed);
        computed
    }

    /// 确保某个网格已在 GPU 上；返回是否可用。
    pub fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: MeshRef) -> bool {
        if self.meshes.contains_key(&key) {
            return true;
        }
        let Some(data) = self.provider.mesh_data(key) else {
            log::warn!("网格 {key:?} 数据不可用，跳过绘制");
            return false;
        };

        // 材质分区：由提供者给出三角形区间；空表示整网格一段默认材质。
        let parts: Vec<GpuPart> = self
            .provider
            .mesh_parts(key)
            .into_iter()
            .map(|part| GpuPart {
                first_index: part.first_index,
                index_count: part.index_count,
                material: match part.material {
                    Some((asset, index)) => MaterialKey::Asset { asset, index },
                    None => MaterialKey::Default,
                },
            })
            .collect();

        let gpu = {
            let mut gpu = upload_mesh(device, queue, &data, parts);
            // 扩展三十六：形状增量进全局表（校验不过 / 表满时返回 None）
            gpu.morph = self.register_morph(&data);
            gpu
        };
        log::debug!(
            "网格已上传 GPU: {key:?} — {} 顶点 / {} 三角",
            data.vertices.len(),
            data.indices.len() / 3
        );
        self.meshes.insert(key, gpu);
        true
    }
}
