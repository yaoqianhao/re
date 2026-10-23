//! 程序化网格生成：内置物件不依赖任何外部模型文件。
//!
//! 所有网格都输出「位置 + 法线」的交错顶点，程序化生成保证离线可用。

use bytemuck::{Pod, Zeroable};
use r3e_core::MeshKind;

/// GPU 顶点格式。stride = 24 字节（3 + 3 个 f32）。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// 单个实例的模型矩阵与颜色。作为第二个顶点缓冲（step_mode = Instance）上传。
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
}

impl InstanceRaw {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4,
        6 => Float32x4
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// CPU 侧网格数据。
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn for_kind(kind: MeshKind) -> Self {
        match kind {
            MeshKind::Cube => cube(),
            MeshKind::Sphere => sphere(28, 18),
            MeshKind::Plane => plane(10),
            MeshKind::Cylinder => cylinder(28),
        }
    }
}

/// 单位立方体（边长 1，中心在原点）。24 顶点实现硬边法线。
pub fn cube() -> MeshData {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),  // +Z
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // -Z
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),  // +X
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),  // -X
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),  // +Y
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),  // -Y
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (n, u, v) in faces {
        let base = vertices.len() as u32;
        // 半边长 0.5
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let pos = [
                n[0] * 0.5 + u[0] * su * 0.5 + v[0] * sv * 0.5,
                n[1] * 0.5 + u[1] * su * 0.5 + v[1] * sv * 0.5,
                n[2] * 0.5 + u[2] * su * 0.5 + v[2] * sv * 0.5,
            ];
            vertices.push(Vertex {
                position: pos,
                normal: n,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshData { vertices, indices }
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
            vertices.push(Vertex {
                position: [n[0] * 0.5, n[1] * 0.5, n[2] * 0.5],
                normal: n,
            });
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
    MeshData { vertices, indices }
}

/// XZ 平面（1x1，法线朝 +Y），细分避免大平面光照/精度问题。
pub fn plane(divisions: u32) -> MeshData {
    let n = divisions.max(1);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for z in 0..=n {
        for x in 0..=n {
            let fx = x as f32 / n as f32 - 0.5;
            let fz = z as f32 / n as f32 - 0.5;
            vertices.push(Vertex {
                position: [fx, 0.0, fz],
                normal: [0.0, 1.0, 0.0],
            });
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
    MeshData { vertices, indices }
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
        vertices.push(Vertex {
            position: [n[0] * 0.5, -0.5, n[2] * 0.5],
            normal: n,
        });
        vertices.push(Vertex {
            position: [n[0] * 0.5, 0.5, n[2] * 0.5],
            normal: n,
        });
    }
    for s in 0..segments {
        let i = s * 2;
        indices.extend_from_slice(&[i, i + 1, i + 2, i + 2, i + 1, i + 3]);
    }

    // 顶盖
    let top_center = vertices.len() as u32;
    vertices.push(Vertex {
        position: [0.0, 0.5, 0.0],
        normal: [0.0, 1.0, 0.0],
    });
    let top_start = vertices.len() as u32;
    for s in 0..segments {
        let theta = s as f32 / segments as f32 * std::f32::consts::TAU;
        let (st, ct) = theta.sin_cos();
        vertices.push(Vertex {
            position: [ct * 0.5, 0.5, st * 0.5],
            normal: [0.0, 1.0, 0.0],
        });
    }
    for s in 0..segments {
        let a = top_start + s;
        let b = top_start + (s + 1) % segments;
        indices.extend_from_slice(&[top_center, b, a]);
    }

    // 底盖
    let bot_center = vertices.len() as u32;
    vertices.push(Vertex {
        position: [0.0, -0.5, 0.0],
        normal: [0.0, -1.0, 0.0],
    });
    let bot_start = vertices.len() as u32;
    for s in 0..segments {
        let theta = s as f32 / segments as f32 * std::f32::consts::TAU;
        let (st, ct) = theta.sin_cos();
        vertices.push(Vertex {
            position: [ct * 0.5, -0.5, st * 0.5],
            normal: [0.0, -1.0, 0.0],
        });
    }
    for s in 0..segments {
        let a = bot_start + s;
        let b = bot_start + (s + 1) % segments;
        indices.extend_from_slice(&[bot_center, a, b]);
    }

    MeshData { vertices, indices }
}
