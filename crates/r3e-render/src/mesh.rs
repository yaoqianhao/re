//! GPU 顶点布局与实例布局。
//!
//! `Vertex` / `MeshData` 的类型定义在 `r3e-core::mesh`，内置图元的程序化生成在
//! `r3e-core::procedural`（S1 起从本 crate 迁出）。渲染层只剩「wgpu 布局」这一件事，
//! 不再持有任何几何生成逻辑，因此也不存在按来源分支的第二条数据通道。

use r3e_core::mesh::Vertex;

pub use r3e_core::mesh::{MeshData as CoreMeshData, Vertex as CoreVertex};

/// 顶点属性：0 = position(f32x3)、1 = normal(f32x3)、2 = uv(f32x2)、
/// 3 = joints(u16x4)、4 = weights(f32x4)（P4 蒙皮）。
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Uint16x4,
    4 => Float32x4
];

/// 顶点缓冲布局（stride = 56 字节：pos12 + normal12 + uv8 + joints8 + weights16）。
pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    }
}

/// 单个实例的模型矩阵与颜色。作为第二个顶点缓冲（step_mode = Instance）上传。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    /// 蒙皮关节矩阵在本帧 storage buffer 中的起始槽位；非蒙皮实例填 0
    /// （此时顶点权重全 0，着色器不会读该槽位，见 `shader.wgsl`）。
    pub joint_base: u32,
    /// 填充，保持 16 字节对齐（InstanceRaw 总长 96 字节）。
    pub _pad: [u32; 3],
}

impl InstanceRaw {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Uint32
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
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
        assert_eq!(layout.array_stride, 56, "pos12+normal12+uv8+joints8+weights16");

        let locs: Vec<u32> = layout.attributes.iter().map(|a| a.shader_location).collect();
        assert_eq!(locs, vec![0, 1, 2, 3, 4], "顶点属性位置不得与实例属性冲突");

        assert_eq!(layout.attributes[3].format, wgpu::VertexFormat::Uint16x4);
        assert_eq!(layout.attributes[4].format, wgpu::VertexFormat::Float32x4);
        assert_eq!(layout.attributes[3].offset, 32);
        assert_eq!(layout.attributes[4].offset, 40);
    }

    /// 实例布局新增 `joint_base`（关节槽位起点），位置不能与顶点属性重号。
    #[test]
    fn instance_layout_adds_joint_base() {
        let layout = InstanceRaw::layout();
        assert_eq!(layout.array_stride as usize, std::mem::size_of::<InstanceRaw>());
        assert_eq!(layout.array_stride, 96);
        assert_eq!(layout.attributes.len(), 6);
        assert_eq!(layout.attributes[5].shader_location, 10);
        assert_eq!(layout.attributes[5].format, wgpu::VertexFormat::Uint32);
        assert_eq!(layout.attributes[5].offset, 80);
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
