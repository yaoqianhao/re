//! GPU 顶点布局与实例布局。
//!
//! `Vertex` / `MeshData` 的类型定义在 `r3e-core::mesh`，内置图元的程序化生成在
//! `r3e-core::procedural`（S1 起从本 crate 迁出）。渲染层只剩「wgpu 布局」这一件事，
//! 不再持有任何几何生成逻辑，因此也不存在按来源分支的第二条数据通道。

use r3e_core::mesh::Vertex;

pub use r3e_core::mesh::{MeshData as CoreMeshData, Vertex as CoreVertex};

/// 顶点属性：0 = position(f32x3)、1 = normal(f32x3)、2 = uv(f32x2)、
/// 3 = joints(u16x4)、4 = weights(f32x4)（P4 蒙皮）、11 = tangent(f32x4)（扩展二十一）。
///
/// 切线占用 location 11 而不是 5：5..=10 已被 `InstanceRaw` 的实例属性占用，
/// 顶点与实例共用同一套 shader location 命名空间，重号会直接让管线校验失败。
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Uint16x4,
    4 => Float32x4,
    11 => Float32x4
];

/// 顶点缓冲布局（stride = 72 字节：pos12 + normal12 + uv8 + joints8 + weights16 + tangent16）。
pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    }
}

/// 单个实例的模型矩阵、颜色、蒙皮槽位与形变权重。作为第二个顶点缓冲
/// （step_mode = Instance）上传，总长 112 字节。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    /// 蒙皮关节矩阵在本帧 storage buffer 中的起始槽位；非蒙皮实例填 0
    /// （此时顶点权重全 0，着色器不会读该槽位，见 `shader.wgsl`）。
    pub joint_base: u32,
    /// 扩展三十六：本实例的形变数据在全局 morph table 中的起始下标（**浮点数**，不是顶点）。
    pub morph_base: u32,
    /// 每个形状占多少个浮点（= 顶点数 × 6：位置增量 xyz + 法线增量 xyz）。
    pub morph_stride: u32,
    /// 可用形状数，夹到 [`r3e_core::MAX_MORPH_TARGETS`]；0 表示不做形变。
    pub morph_count: u32,
    /// 各形状权重，与 `morph_count` 一起进 `morph_offset`。
    pub morph_weights: [f32; 4],
}

impl InstanceRaw {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Uint32x4,
        12 => Float32x4
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// 没有形变的实例（绝大多数）。
    pub fn plain(model: [[f32; 4]; 4], color: [f32; 4], joint_base: u32) -> Self {
        Self {
            model,
            color,
            joint_base,
            morph_base: 0,
            morph_stride: 0,
            morph_count: 0,
            morph_weights: [0.0; 4],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::BuiltinMesh;

    /// P4：蒙皮属性必须落在顶点布局里，且 stride 与 `Vertex` 实际大小一致。
    #[test]
    fn vertex_layout_carries_skinning() {
        let layout = vertex_layout();
        assert_eq!(layout.array_stride as usize, std::mem::size_of::<Vertex>());
        assert_eq!(
            layout.array_stride, 72,
            "pos12+normal12+uv8+joints8+weights16+tangent16"
        );

        let locs: Vec<u32> = layout
            .attributes
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(
            locs,
            vec![0, 1, 2, 3, 4, 11],
            "顶点属性位置不得与实例属性冲突"
        );

        assert_eq!(layout.attributes[3].format, wgpu::VertexFormat::Uint16x4);
        assert_eq!(layout.attributes[4].format, wgpu::VertexFormat::Float32x4);
        assert_eq!(layout.attributes[3].offset, 32);
        assert_eq!(layout.attributes[4].offset, 40);
    }

    /// 扩展二十一：切线在顶点布局末尾（offset 56），供法线贴图构造 TBN。
    #[test]
    fn vertex_layout_carries_tangent() {
        let layout = vertex_layout();
        let tangent = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == 11)
            .expect("切线属性缺失");
        assert_eq!(tangent.format, wgpu::VertexFormat::Float32x4);
        assert_eq!(
            tangent.offset, 56,
            "切线紧跟 weights（40 + 16）之后，正好是结构体最后一个字段"
        );
        assert_eq!(
            tangent.offset + 16,
            layout.array_stride,
            "切线必须填满 stride 末尾，不留悬空字节"
        );
    }

    /// 实例布局新增 `joint_base`（关节槽位起点），位置不能与顶点属性重号。
    #[test]
    fn instance_layout_adds_joint_base() {
        let layout = InstanceRaw::layout();
        assert_eq!(
            layout.array_stride as usize,
            std::mem::size_of::<InstanceRaw>()
        );
        assert_eq!(layout.array_stride, 112);
        assert_eq!(layout.attributes.len(), 7);
        assert_eq!(layout.attributes[5].shader_location, 10);
        assert_eq!(layout.attributes[5].format, wgpu::VertexFormat::Uint32x4);
        assert_eq!(layout.attributes[5].offset, 80);
    }

    /// 扩展三十六：形变权重跟在实例元数据之后（location 12，offset 96），
    /// 正好填满 stride，不留悬空字节；也不与顶点布局的 location 撞号。
    #[test]
    fn instance_layout_carries_morph_weights() {
        let layout = InstanceRaw::layout();
        let weights = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == 12)
            .expect("形变权重属性缺失");
        assert_eq!(weights.format, wgpu::VertexFormat::Float32x4);
        assert_eq!(weights.offset, 96, "紧跟 meta（80 + 16）");
        assert_eq!(weights.offset + 16, layout.array_stride);

        let vertex_locs: Vec<u32> = VERTEX_ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert!(
            !vertex_locs.contains(&12),
            "实例属性 location 12 不得被顶点属性占用"
        );
    }

    /// `plain` 构造出的是「无蒙皮偏移 + 无形状」的中性实例，字段逐位可断言。
    #[test]
    fn plain_instance_has_no_morph() {
        let raw = InstanceRaw::plain([[0.0; 4]; 4], [1.0; 4], 7);
        assert_eq!(raw.joint_base, 7);
        assert_eq!(
            (raw.morph_base, raw.morph_stride, raw.morph_count),
            (0, 0, 0)
        );
        assert_eq!(raw.morph_weights, [0.0; 4]);
    }

    /// 内置网格已迁到 core，但「不蒙皮」这一约定仍要在此守住：
    /// 权重全 0 时着色器才会跳过 storage 读取。
    #[test]
    fn builtin_mesh_is_not_skinned() {
        for kind in BuiltinMesh::ALL {
            let mesh = r3e_core::procedural::builtin_mesh(kind);
            assert!(mesh.vertices.iter().all(|v| !v.is_skinned()), "{kind:?}");
        }
    }
}
