//! 与图形 API 无关的 CPU 侧网格数据。
//!
//! 这一层只描述「顶点长什么样」与「索引怎么排」，既不用 wgpu 类型，
//! 也不认识任何模型文件格式，供渲染层与资产层（`r3e-asset`）共同复用。

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

use crate::{aabb::Aabb, morph::MorphSet, AssetId, MeshRef};

/// 缺省切线：`+X` 方向、手性 +1（表示「尚未按 UV 计算过切线」）。
pub const DEFAULT_TANGENT: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// 统一顶点格式：位置 + 法线 + 第一套 UV + 蒙皮（P4）+ 切线（扩展二十一）。
///
/// 使用 `#[repr(C)]` + `Pod` 是为了让渲染层能直接把切片当作字节上传，
/// 但类型本身不依赖任何 wgpu API。
///
/// 蒙皮字段在 P4 加入：`joints` 是关节槽位下标（对应 `SkinInstance::joint_matrices`），
/// `weights` 是对应权重。**非蒙皮顶点两者全为 0**，着色器据此走「不做蒙皮」分支，
/// 因此同一份顶点布局能同时服务内置网格与蒙皮资产网格。
///
/// `tangent` 在扩展二十一加入（`xyz` = 切线，`w` = 手性 ±1），供法线贴图构造 TBN。
/// 缺省值是 `(1, 0, 0, 1)`，即「未计算切线」；真正需要的网格由
/// [`MeshData::recompute_tangents`] 从位置与 UV 现算（见 `crate::tangent`）。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// 每顶点最多 4 个关节下标（P4）。
    pub joints: [u16; 4],
    /// 每顶点最多 4 个权重，和为 1；非蒙皮顶点为 0（P4）。
    pub weights: [f32; 4],
    /// 切线空间基：`xyz` = 切线方向，`w` = 手性（`bitangent = cross(N, T) * w`）。
    pub tangent: [f32; 4],
}

impl Default for Vertex {
    fn default() -> Self {
        Self::new([0.0; 3], [0.0; 3], [0.0; 2])
    }
}

impl Vertex {
    /// 默认切线的顶点（切线待 [`MeshData::recompute_tangents`] 补齐）。
    pub const fn new(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Self {
        Self {
            position,
            normal,
            uv,
            joints: [0; 4],
            weights: [0.0; 4],
            tangent: DEFAULT_TANGENT,
        }
    }

    /// 带蒙皮权重的顶点（P4）。
    pub const fn skinned(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        joints: [u16; 4],
        weights: [f32; 4],
    ) -> Self {
        Self {
            position,
            normal,
            uv,
            joints,
            weights,
            tangent: DEFAULT_TANGENT,
        }
    }

    /// 显式给出切线的顶点。
    pub const fn with_tangent(mut self, tangent: [f32; 4]) -> Self {
        self.tangent = tangent;
        self
    }

    /// 切线（`Vec3`）；手性见 [`Vertex::tangent_handedness`]。
    pub fn tangent(&self) -> Vec3 {
        Vec3::new(self.tangent[0], self.tangent[1], self.tangent[2])
    }

    /// 切线手性 ±1；非有限值按 +1 处理。
    pub fn tangent_handedness(&self) -> f32 {
        if self.tangent[3] < 0.0 {
            -1.0
        } else {
            1.0
        }
    }

    /// 是否带蒙皮权重。
    pub fn is_skinned(&self) -> bool {
        self.weights.iter().any(|w| *w > 0.0)
    }

    pub fn position(&self) -> Vec3 {
        Vec3::from(self.position)
    }

    pub fn normal(&self) -> Vec3 {
        Vec3::from(self.normal)
    }

    pub fn uv(&self) -> Vec2 {
        Vec2::from(self.uv)
    }
}

/// CPU 侧三角网格：顶点数组 + 32 位索引。
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// 扩展三十六：顶点形变形状（表情 / 口型）。空表示该网格没有 morph 能力。
    ///
    /// 与 `vertices` 共享顶点下标空间，长度必须等于 `vertices.len()` ——
    /// 渲染层上传前会按这个数校验，不通过就当没有形状（见 [`MorphSet::validate_against`]）。
    pub morph: MorphSet,
}

impl MeshData {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self {
            vertices,
            indices,
            morph: MorphSet::default(),
        }
    }

    /// 挂上一组形变形状（链式）。
    pub fn with_morph(mut self, morph: MorphSet) -> Self {
        self.morph = morph;
        self
    }

    /// 是否带形变形状。
    pub fn has_morph(&self) -> bool {
        !self.morph.is_empty()
    }

    /// 由「位置 / 法线 / UV（可选）」三组平行数组拼装顶点。
    ///
    /// 数组长度不一致时按最短的长度截断，避免越界；UV 缺省填 0。
    pub fn from_arrays(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: Option<&[[f32; 2]]>,
        indices: Vec<u32>,
    ) -> Self {
        let count = positions.len().min(normals.len());
        let mut vertices = Vec::with_capacity(count);
        for (i, (pos, nrm)) in positions.iter().zip(normals.iter()).enumerate() {
            let uv = uvs.and_then(|u| u.get(i).copied()).unwrap_or([0.0, 0.0]);
            vertices.push(Vertex::new(*pos, *nrm, uv));
        }
        Self {
            vertices,
            indices,
            morph: MorphSet::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }

    /// 由「位置 / 法线 / UV / 蒙皮权重」平行数组拼装顶点（P4）。
    ///
    /// `joints` / `weights` 为 `None` 时蒙皮字段填 0，等价于非蒙皮网格；
    /// 位置数组长度决定顶点数，其余数组短了按缺省值补齐。
    pub fn from_arrays_skinned(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: Option<&[[f32; 2]]>,
        joints: Option<&[[u16; 4]]>,
        weights: Option<&[[f32; 4]]>,
        indices: Vec<u32>,
    ) -> Self {
        let count = positions.len();
        let mut vertices = Vec::with_capacity(count);
        for (i, pos) in positions.iter().enumerate() {
            let uv = uvs.and_then(|u| u.get(i).copied()).unwrap_or([0.0, 0.0]);
            let j = joints.and_then(|j| j.get(i).copied()).unwrap_or([0; 4]);
            let w = weights.and_then(|w| w.get(i).copied()).unwrap_or([0.0; 4]);
            let normal = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            vertices.push(Vertex::skinned(*pos, normal, uv, j, w));
        }
        Self {
            vertices,
            indices,
            morph: MorphSet::default(),
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// 从位置与 UV 现算每顶点切线并写回（扩展二十一）。
    ///
    /// 法线贴图需要 TBN，而 FBX 没有切线、glTF 的 `TANGENT` 也是可选属性，
    /// 因此统一在此补算，算法见 [`crate::tangent::compute_tangents`]。
    /// 空网格或索引为空时不动顶点（切线保持 [`DEFAULT_TANGENT`]）。
    pub fn recompute_tangents(&mut self) {
        crate::tangent::recompute_tangents(self);
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// 顶点位置（几何空间）的轴对齐包围盒。空网格返回全 0。
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for v in &self.vertices {
            let p = v.position();
            min = min.min(p);
            max = max.max(p);
        }
        if !min.is_finite() || !max.is_finite() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        (min, max)
    }

    /// 同 [`MeshData::aabb`]，但返回 [`Aabb`] 类型，便于做变换与视锥测试。
    pub fn aabb_box(&self) -> Aabb {
        let (min, max) = self.aabb();
        Aabb::new(min, max)
    }
}

/// 网格数据来源：渲染层只依赖该抽象，不认识具体资产存储与文件 IO。
///
/// `r3e-asset::AssetLibrary` 在资产层实现它，`r3e-app` 负责把实现注入渲染层，
/// 这样 `r3e-render` 的依赖图里不会出现 `ufbx`。
///
/// P2 起这一层除了顶点数据，还对外提供「材质分区」与「材质 / 贴图」——
/// 渲染层据此决定每个绘制段的 bind group，但仍不需要知道数据来自 FBX 还是别的格式。
pub trait MeshProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData>;

    /// 网格的局部包围盒（几何空间），用于视锥剔除与物理碰撞体推导。
    ///
    /// `None` 表示「本提供者不提供包围盒」，调用方应退回未剔除路径（fail-open），
    /// 而不是把物件当空盒剔掉。资产层的实现直接读导入时已算好的 AABB，零拷贝。
    fn mesh_aabb(&self, _key: MeshRef) -> Option<Aabb> {
        None
    }

    /// 网格的材质分区（按索引区间切分）。返回空表示「整网格一段、用默认材质」。
    fn mesh_parts(&self, _key: MeshRef) -> Vec<MeshPartData> {
        Vec::new()
    }

    /// 取某个资产内第 `index` 个材质。
    fn material(&self, _asset: AssetId, _index: u32) -> Option<MaterialData> {
        None
    }

    /// 取某个资产内第 `index` 张贴图（已解码为 RGBA8）。
    fn texture(&self, _asset: AssetId, _index: u32) -> Option<TextureImage> {
        None
    }
}

/// 一张已解码的贴图：RGBA8、行优先、每像素 4 字节。
#[derive(Debug, Clone, Default)]
pub struct TextureImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl TextureImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
        }
    }

    /// 尺寸与字节数是否自洽（渲染层上传前必须能通过这道闸）。
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.rgba.len() >= self.width as usize * self.height as usize * 4
    }
}

/// 材质数据：渲染层只认这份描述。
///
/// 贴图字段都是 `(资产, 该资产贴图表下标)`，渲染层通过 `MeshProvider::texture`
/// 取到已解码的 [`TextureImage`] 再上传 —— 这一层与渲染层都不做文件 IO。
/// **全部缺省为 `None`**，此时渲染走纯标量参数（与扩展十一的行为完全一致）。
#[derive(Debug, Clone)]
pub struct MaterialData {
    pub name: String,
    /// 线性 base color（与贴图相乘的染色因子）。
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// base color 贴图：`(资产, 该资产贴图表下标)`。
    pub base_color_texture: Option<(AssetId, u32)>,
    /// 切线空间法线贴图（线性数据，需 TBN 采样，见 `crate::tangent`）。
    pub normal_texture: Option<(AssetId, u32)>,
    /// 粗糙度贴图（线性数据，取 G 通道；也承接 glTF 的 `metallicRoughness` 图）。
    pub roughness_texture: Option<(AssetId, u32)>,
    /// 环境光遮蔽贴图（线性数据，取 R 通道）。
    pub occlusion_texture: Option<(AssetId, u32)>,
}

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            base_color_texture: None,
            normal_texture: None,
            roughness_texture: None,
            occlusion_texture: None,
        }
    }
}

impl MaterialData {
    /// 是否带任何一张 PBR 数据贴图（法线 / 粗糙度 / AO）。
    pub fn has_pbr_maps(&self) -> bool {
        self.normal_texture.is_some()
            || self.roughness_texture.is_some()
            || self.occlusion_texture.is_some()
    }

    /// 数据贴图张数，用于导入统计与文档口径。
    pub fn pbr_map_count(&self) -> usize {
        [
            self.normal_texture,
            self.roughness_texture,
            self.occlusion_texture,
        ]
        .iter()
        .filter(|slot| slot.is_some())
        .count()
    }
}

/// 网格内的一个材质分区：索引区间 + 使用的材质。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshPartData {
    /// 起始索引（索引缓冲内下标，非三角形号）。
    pub first_index: u32,
    pub index_count: u32,
    /// `None` 表示该段使用默认材质（无贴图、白色）。
    pub material: Option<(AssetId, u32)>,
}
