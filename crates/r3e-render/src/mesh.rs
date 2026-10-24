//! 程序化网格生成与 GPU 顶点布局。
//!
//! `Vertex` / `MeshData` 的类型定义已下沉到 `r3e-core::mesh`，渲染层只负责
//! 「wgpu 顶点布局」与「内置网格的程序化生成」。

use r3e_core::mesh::{MeshData, Vertex};
use r3e_core::BuiltinMesh;

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

/// 内置程序化网格的统一入口。
pub fn builtin_mesh(kind: BuiltinMesh) -> MeshData {
    match kind {
        BuiltinMesh::Cube => cube(),
        BuiltinMesh::Sphere => sphere(28, 18),
        BuiltinMesh::Plane => plane(10),
        BuiltinMesh::Cylinder => cylinder(28),
    }
}

fn vtx(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex::new(position, normal, uv)
}

/// 单位立方体（边长 1，中心在原点）。24 顶点实现硬边法线，每面独立 UV。
pub fn cube() -> MeshData {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // +Z
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // -Z
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]), // +X
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), // -X
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]), // +Y
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]), // -Y
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (n, u, v) in faces {
        let base = vertices.len() as u32;
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let pos = [
                n[0] * 0.5 + u[0] * su * 0.5 + v[0] * sv * 0.5,
                n[1] * 0.5 + u[1] * su * 0.5 + v[1] * sv * 0.5,
                n[2] * 0.5 + u[2] * su * 0.5 + v[2] * sv * 0.5,
            ];
            // UV 原点左上：u 向右增长，v 向下增长
            let uv = [su * 0.5 + 0.5, 0.5 - sv * 0.5];
            vertices.push(vtx(pos, n, uv));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshData::new(vertices, indices)
}

/// UV 球（半径 0.5）。
pub fn sphere(segments: u32, rings: u32) -> MeshData {
    let segments = segments.max(6);
    let rings = rings.max(3);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let phi = v * std::f32::consts::PI;
        let (sp, cp) = phi.sin_cos();
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (st, ct) = theta.sin_cos();
            let n = [sp * ct, cp, sp * st];
            vertices.push(vtx([n[0] * 0.5, n[1] * 0.5, n[2] * 0.5], n, [u, v]));
        }
    }

    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData::new(vertices, indices)
}

/// XZ 平面（1x1，法线朝 +Y），细分避免大平面光照/精度问题。
pub fn plane(divisions: u32) -> MeshData {
    let n = divisions.max(1);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for z in 0..=n {
        for x in 0..=n {
            let fx = x as f32 / n as f32;
            let fz = z as f32 / n as f32;
            vertices.push(vtx(
                [fx - 0.5, 0.0, fz - 0.5],
                [0.0, 1.0, 0.0],
                [fx, fz],
            ));
        }
    }
    let row = n + 1;
    for z in 0..n {
        for x in 0..n {
            let a = z * row + x;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData::new(vertices, indices)
}

/// 圆柱（半径 0.5，高 1，轴为 Y）。
pub fn cylinder(segments: u32) -> MeshData {
    let segments = segments.max(6);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // 侧面
    for s in 0..=segments {
        let t = s as f32 / segments as f32;
        let theta = t * std::f32::consts::TAU;
        let (st, ct) = theta.sin_cos();
        let n = [ct, 0.0, st];
        vertices.push(vtx([n[0] * 0.5, -0.5, n[2] * 0.5], n, [t, 1.0]));
        vertices.push(vtx([n[0] * 0.5, 0.5, n[2] * 0.5], n, [t, 0.0]));
    }
    for s in 0..segments {
        let i = s * 2;
        indices.extend_from_slice(&[i, i + 1, i + 2, i + 2, i + 1, i + 3]);
    }

    // 顶盖
    let top_center = vertices.len() as u32;
    vertices.push(vtx([0.0, 0.5, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5]));
    let top_start = vertices.len() as u32;
    for s in 0..segments {
        let theta = s as f32 / segments as f32 * std::f32::consts::TAU;
        let (st, ct) = theta.sin_cos();
        vertices.push(vtx(
            [ct * 0.5, 0.5, st * 0.5],
            [0.0, 1.0, 0.0],
            [ct * 0.5 + 0.5, 0.5 - st * 0.5],
        ));
    }
    for s in 0..segments {
        let a = top_start + s;
        let b = top_start + (s + 1) % segments;
        indices.extend_from_slice(&[top_center, b, a]);
    }

    // 底盖
    let bot_center = vertices.len() as u32;
    vertices.push(vtx([0.0, -0.5, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]));
    let bot_start = vertices.len() as u32;
    for s in 0..segments {
        let theta = s as f32 / segments as f32 * std::f32::consts::TAU;
        let (st, ct) = theta.sin_cos();
        vertices.push(vtx(
            [ct * 0.5, -0.5, st * 0.5],
            [0.0, -1.0, 0.0],
            [ct * 0.5 + 0.5, 0.5 + st * 0.5],
        ));
    }
    for s in 0..segments {
        let a = bot_start + s;
        let b = bot_start + (s + 1) % segments;
        indices.extend_from_slice(&[bot_center, a, b]);
    }

    MeshData::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 内置网格不蒙皮：权重必须为全 0，着色器据此跳过 storage 读取。
    #[test]
    fn builtin_mesh_is_not_skinned() {
        for kind in [
            BuiltinMesh::Cube,
            BuiltinMesh::Sphere,
            BuiltinMesh::Plane,
            BuiltinMesh::Cylinder,
        ] {
            let mesh = builtin_mesh(kind);
            assert!(mesh.vertices.iter().all(|v| !v.is_skinned()), "{kind:?}");
        }
    }
}
