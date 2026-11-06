//! 切线空间（TBN）构造：法线贴图的采样坐标系。
//!
//! 法线贴图存的是**切线空间**下的扰动法线（+Z 沿几何法线），要把它映射回世界空间，
//! 每个顶点至少需要一条切线，副切线由 `cross(normal, tangent) * w` 得到，`w` 是手性符号。
//! FBX / glTF 并不总带切线（glTF 的 `TANGENT` 是可选属性，FBX 干脆没有），
//! 所以这里按 Lengyel 的「按 UV 差分累加 + Gram-Schmidt 正交化」从位置与 UV 现算。
//!
//! 两条硬口径：
//!
//! * **退化不给 NaN**：UV 全等（三角形在 UV 空间退化成一点）时该三角形跳过；
//!   若整块网格都退化，按「与法线最不平行的轴」叉出一条任意正交切线，仍是有限单位向量。
//! * **手性来自副切线方向**：镜像 UV（u 反向）必须得到 `w = -1`，否则法线贴图会左右反掉。
//!
//! 本模块不依赖 wgpu / 文件格式，纯 CPU 纯函数，可直接单测。

use glam::Vec3;

use crate::mesh::{MeshData, Vertex};

/// UV 面积低于该阈值视为退化三角形（跳过，不参与累加）。
const UV_AREA_EPSILON: f32 = 1e-12;

/// 累加长度低于该阈值视为「没有可用切线」，退回任意正交基。
const TANGENT_EPSILON: f32 = 1e-8;

/// 由三角形的位置 / UV 差分累加出每顶点切线，返回 `[x, y, z, w]`（`w` = 手性）。
///
/// 返回长度与 `vertices` 一致；`indices` 不完整（`len % 3 != 0`）或越界时，
/// 越界的三角形直接跳过，不 panic。
pub fn compute_tangents(vertices: &[Vertex], indices: &[u32]) -> Vec<[f32; 4]> {
    let mut tangent_sum = vec![Vec3::ZERO; vertices.len()];
    let mut bitangent_sum = vec![Vec3::ZERO; vertices.len()];

    for tri in indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if i0 >= vertices.len() || i1 >= vertices.len() || i2 >= vertices.len() {
            continue;
        }
        let (v0, v1, v2) = (&vertices[i0], &vertices[i1], &vertices[i2]);

        let p0 = v0.position();
        let edge1 = v1.position() - p0;
        let edge2 = v2.position() - p0;
        let duv1 = v1.uv() - v0.uv();
        let duv2 = v2.uv() - v0.uv();

        // UV 空间的行列式：接近 0 说明三角形在 UV 上退化，差分解不出方向
        let det = duv1.x * duv2.y - duv1.y * duv2.x;
        if !det.is_finite() || det.abs() < UV_AREA_EPSILON {
            continue;
        }
        let inv = 1.0 / det;
        let tangent = (edge1 * duv2.y - edge2 * duv1.y) * inv;
        let bitangent = (edge2 * duv1.x - edge1 * duv2.x) * inv;
        if !tangent.is_finite() || !bitangent.is_finite() {
            continue;
        }

        for index in [i0, i1, i2] {
            tangent_sum[index] += tangent;
            bitangent_sum[index] += bitangent;
        }
    }

    vertices
        .iter()
        .enumerate()
        .map(|(i, vertex)| {
            let normal = normalized_or(vertex.normal(), Vec3::Y);
            // Gram-Schmidt：把累加切线投影到法线切平面上，保证 T ⟂ N
            let raw = tangent_sum[i] - normal * normal.dot(tangent_sum[i]);
            let tangent = if raw.length() > TANGENT_EPSILON && raw.is_finite() {
                raw.normalize()
            } else {
                any_orthogonal(normal)
            };
            let bitangent = bitangent_sum[i];
            // 手性：副切线方向与 cross(N, T) 反向说明 UV 被镜像过
            let w = if normal.cross(tangent).dot(bitangent) < 0.0 {
                -1.0
            } else {
                1.0
            };
            [tangent.x, tangent.y, tangent.z, w]
        })
        .collect()
}

/// 就地重算整块网格的切线（索引无效或网格为空时保持原样）。
pub fn recompute_tangents(mesh: &mut MeshData) {
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return;
    }
    let tangents = compute_tangents(&mesh.vertices, &mesh.indices);
    for (vertex, tangent) in mesh.vertices.iter_mut().zip(tangents) {
        vertex.tangent = tangent;
    }
}

/// 归一化；零向量 / 非有限值退回 `fallback`。
fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    if v.is_finite() && v.length() > TANGENT_EPSILON {
        v.normalize()
    } else {
        fallback
    }
}

/// 与 `n` 正交的任意单位向量（取与 n 最不平行的坐标轴叉乘）。
fn any_orthogonal(n: Vec3) -> Vec3 {
    let axis = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let t = axis.cross(n);
    if t.length() > TANGENT_EPSILON && t.is_finite() {
        t.normalize()
    } else {
        Vec3::X
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 法线朝 +Z、UV 与位置对齐的平面四边形（两个三角形）。
    fn quad_vertices(mirror_u: bool) -> (Vec<Vertex>, Vec<u32>) {
        let u = |x: f32| if mirror_u { 1.0 - x } else { x };
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [u(0.0), 0.0]),
            Vertex::new([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [u(1.0), 0.0]),
            Vertex::new([1.0, 1.0, 0.0], [0.0, 0.0, 1.0], [u(1.0), 1.0]),
            Vertex::new([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [u(0.0), 1.0]),
        ];
        (vertices, vec![0, 1, 2, 0, 2, 3])
    }

    /// 标准平面：切线沿 +X，手性 +1。
    #[test]
    fn planar_quad_gets_expected_tangent() {
        let (vertices, indices) = quad_vertices(false);
        let tangents = compute_tangents(&vertices, &indices);
        assert_eq!(tangents.len(), vertices.len());
        for t in &tangents {
            assert!((t[0] - 1.0).abs() < 1e-5, "切线应沿 +X：{t:?}");
            assert!(
                t[1].abs() < 1e-5 && t[2].abs() < 1e-5,
                "切线应无 Y/Z 分量：{t:?}"
            );
            assert_eq!(t[3], 1.0, "未镜像 UV 时手性应为 +1");
        }
    }

    /// 镜像 UV（u 反向）必须翻转手性，否则法线贴图左右反。
    #[test]
    fn mirrored_uv_flips_handedness() {
        let (vertices, indices) = quad_vertices(true);
        let tangents = compute_tangents(&vertices, &indices);
        assert!(tangents.iter().all(|t| t[3] == -1.0), "{tangents:?}");
    }

    /// UV 全等 → 差分为零，退化路径必须给有限、单位、与法线垂直的切线。
    #[test]
    fn degenerate_uv_falls_back_to_orthogonal_basis() {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.5, 0.5]),
            Vertex::new([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.5, 0.5]),
            Vertex::new([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.5, 0.5]),
        ];
        let tangents = compute_tangents(&vertices, &[0, 1, 2]);
        for t in &tangents {
            let v = Vec3::new(t[0], t[1], t[2]);
            assert!(v.is_finite(), "退化切线必须有限：{t:?}");
            assert!((v.length() - 1.0).abs() < 1e-5, "退化切线应归一：{t:?}");
            assert!(v.z.abs() < 1e-5, "退化切线应与 +Z 法线正交：{t:?}");
            assert_eq!(t[3], 1.0);
        }
    }

    /// 零法线 / 越界索引 / 不完整三角形：不 panic，输出仍是有限值。
    #[test]
    fn hostile_inputs_stay_finite() {
        let vertices = vec![
            Vertex::new([0.0; 3], [0.0; 3], [0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0], [0.0; 3], [1.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0], [0.0; 3], [0.0, 1.0]),
        ];
        for indices in [vec![0, 1, 2], vec![0, 1, 99], vec![0, 1], vec![]] {
            let tangents = compute_tangents(&vertices, &indices);
            assert_eq!(tangents.len(), vertices.len());
            for t in &tangents {
                assert!(t.iter().all(|c| c.is_finite()), "{indices:?} → {t:?}");
                let v = Vec3::new(t[0], t[1], t[2]);
                assert!((v.length() - 1.0).abs() < 1e-5, "{indices:?} → {t:?}");
            }
        }
    }

    /// `recompute_tangents` 把结果写回顶点；空网格保持空。
    #[test]
    fn recompute_writes_back_to_mesh() {
        let (vertices, indices) = quad_vertices(false);
        let mut mesh = MeshData::new(vertices, indices);
        recompute_tangents(&mut mesh);
        assert!(mesh.vertices.iter().all(|v| v.tangent[3] == 1.0));
        assert!(mesh
            .vertices
            .iter()
            .all(|v| (v.tangent[0] - 1.0).abs() < 1e-5));

        let mut empty = MeshData::default();
        recompute_tangents(&mut empty);
        assert!(empty.vertices.is_empty());
    }
}
