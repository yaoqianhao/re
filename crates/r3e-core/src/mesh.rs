//! 与图形 API 无关的 CPU 侧网格数据。
//!
//! 这一层只描述「顶点长什么样」与「索引怎么排」，既不用 wgpu 类型，
//! 也不认识任何模型文件格式，供渲染层与资产层（`r3e-asset`）共同复用。

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

use crate::MeshRef;

/// 统一顶点格式：位置 + 法线 + 第一套 UV。
///
/// 使用 `#[repr(C)]` + `Pod` 是为了让渲染层能直接把切片当作字节上传，
/// 但类型本身不依赖任何 wgpu API。
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    pub const fn new(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Self {
        Self {
            position,
            normal,
            uv,
        }
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
}

impl MeshData {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
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
        for i in 0..count {
            let uv = uvs
                .and_then(|u| u.get(i).copied())
                .unwrap_or([0.0, 0.0]);
            vertices.push(Vertex::new(positions[i], normals[i], uv));
        }
        Self { vertices, indices }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
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
}

/// 网格数据来源：渲染层只依赖该抽象，不认识具体资产存储与文件 IO。
///
/// `r3e-asset::AssetLibrary` 在资产层实现它，`r3e-app` 负责把实现注入渲染层，
/// 这样 `r3e-render` 的依赖图里不会出现 `ufbx`。
pub trait MeshProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData>;
}
