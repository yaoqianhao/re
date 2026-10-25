//! 内置程序化图元：与 GPU / 文件 IO 无关的几何生成，住在 core 层。
//!
//! 历史：这段代码原先在 `r3e-render`（`mesh.rs`），导致「内置图元」和「资产网格」
//! 在渲染层形成两条数据路径。S1 把它迁到 core，于是：
//!
//! * 渲染层只通过 [`MeshProvider`] 取数据，不再认识 `MeshRef::Builtin`；
//! * 资产层可以把内置图元的应答委托给这里的 [`ProceduralProvider`]；
//! * 物理、脚本拿到的是同一份 [`MeshData`]（含真实 AABB），不需要来源特判。
//!
//! 生成的几何同样必须满足 `crate::air` 的不变量（单测已覆盖）。

use crate::aabb::Aabb;
use crate::mesh::{MeshData, MeshPartData, MeshProvider, Vertex};
use crate::{BuiltinMesh, MeshRef};

/// 内置图元的统一入口：一种 `BuiltinMesh` → 一份 AIR 网格。
pub fn builtin_mesh(kind: BuiltinMesh) -> MeshData {
    match kind {
        BuiltinMesh::Cube => cube(),
        BuiltinMesh::Sphere => sphere(28, 18),
        BuiltinMesh::Plane => plane(10),
        BuiltinMesh::Cylinder => cylinder(28),
    }
}

/// 供 `MeshProvider` 使用的内置图元提供者：**只**应答 `MeshRef::Builtin`。
///
/// 渲染层的 `MeshCache` 在没有注入资产库时默认用它，因此 `MeshCache::new()`
/// 依旧开箱可用，但走的是与资产完全相同的 `mesh_data` / `mesh_aabb` 通道。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProceduralProvider;

impl ProceduralProvider {
    pub const fn new() -> Self {
        Self
    }
}

impl MeshProvider for ProceduralProvider {
    fn mesh_data(&self, key: MeshRef) -> Option<MeshData> {
        key.builtin().map(builtin_mesh)
    }

    fn mesh_aabb(&self, key: MeshRef) -> Option<Aabb> {
        let aabb = builtin_mesh(key.builtin()?).aabb_box();
        (!aabb.is_empty()).then_some(aabb)
    }

    /// 内置图元没有材质分区：整网格一段默认材质。
    fn mesh_parts(&self, _key: MeshRef) -> Vec<MeshPartData> {
        Vec::new()
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
            vertices.push(vtx([fx - 0.5, 0.0, fz - 0.5], [0.0, 1.0, 0.0], [fx, fz]));
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
    use crate::air::{validate, validate_parts};

    #[test]
    fn primitives_pass_air_and_are_not_skinned() {
        for kind in BuiltinMesh::ALL {
            let mesh = builtin_mesh(kind);
            assert_eq!(validate(&mesh), Vec::new(), "{kind:?}");
            assert!(mesh.vertices.iter().all(|v| !v.is_skinned()), "{kind:?}");
            assert_eq!(validate_parts(&mesh, &[]), Vec::new());
        }
    }

    #[test]
    fn provider_serves_builtin_only() {
        let provider = ProceduralProvider::new();
        let key = MeshRef::Builtin(BuiltinMesh::Cube);
        let mesh = provider.mesh_data(key).expect("cube");
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(provider.mesh_parts(key), Vec::new());

        let aabb = provider.mesh_aabb(key).expect("aabb");
        assert!(!aabb.is_empty());

        let asset = MeshRef::Asset {
            asset: crate::AssetId(0),
            mesh: 0,
        };
        assert!(provider.mesh_data(asset).is_none());
        assert!(provider.mesh_aabb(asset).is_none());
    }
}
