//! AIR（Asset Intermediate Representation）：引擎内**唯一**的 3D 资产中间表示。
//!
//! 所有来源——内置程序化图元、`.fbx`、`.glb`、将来的 `.obj`——导入后都必须产出
//! [`MeshData`] / `MeshPartData` / `MaterialData` / `TextureImage` / `SkinData` 这几类
//! 统一结构，不许在导入器内部另造旁路类型。渲染、剔除、物理、脚本随后只认这一份数据，
//! 因而「分级分层」只发生在**输出侧**（见 `MeshProvider` 的各个分层面），
//! 存储侧不存在第二套表示。
//!
//! # AIR 不变量
//!
//! 1. 坐标：+X 右、+Y 上、+Z 前、右手系，单位米。
//! 2. 顶点空间：顶点已烘焙到「实例变换之前」的资产空间。
//! 3. 绕序：CCW 为正面。
//! 4. 法线：单位长度（容差 [`NORMAL_UNIT_EPS`]）。
//! 5. 索引：`u32`、三角形列表、`len % 3 == 0`、每个索引 `< vertex_count`。
//! 6. UV：uv0 必须存在（缺省 `[0, 0]`），允许越界但必须有限。
//! 7. 蒙皮：每顶点 ≤ 4 权重、权重和为 1（容差 [`WEIGHT_SUM_EPS`]）或全 0。
//! 8. 材质：`base_color` 为线性空间；贴图 RGBA8；索引在范围内。
//! 9. 分区：parts 首尾相接、无重叠、并集覆盖 `[0, indices.len())`，且对齐到 3 的倍数。
//! 10. AABB：非空、`min <= max`，并与顶点重算值一致。
//! 11. 命名：节点名唯一（由导入器负责追加后缀）。
//! 12. 确定性：同一文件两次导入，AIR 输出逐字节一致（见 [`mesh_fingerprint`]）。
//!
//! 本模块同时提供**唯一一份**归一化工具箱（[`normalize`]）：FBX 与 glb 导入器不得各自
//! 实现一套轴转换 / 绕序 / 蒙皮压缩逻辑，否则两套实现会各自演进、数值漂移。
//!
//! 现阶段（S0）校验只做**告警**，不拦截导入；待 S2 完成导入器标准化后升级为强制闸门。

use glam::{Mat3, Mat4, Vec3};

use crate::mesh::{MeshData, MeshPartData, Vertex};

/// 法线单位长度容差。
pub const NORMAL_UNIT_EPS: f32 = 1e-3;
/// 蒙皮权重和容差（也用于「是否为蒙皮顶点」的判定）。
pub const WEIGHT_SUM_EPS: f32 = 1e-3;
/// AABB 一致性容差。
pub const AABB_EPS: f32 = 1e-4;

/// 违规类别，便于测试逐条断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirIssueKind {
    EmptyMesh,
    IndexNotTriangleList,
    IndexOutOfRange,
    NonFinitePosition,
    NonFiniteNormal,
    NormalNotUnit,
    NonFiniteUv,
    SkinWeightNegative,
    SkinWeightSum,
    SkinJointOutOfRange,
    PartsCoverage,
    DegenerateAabb,
}

impl AirIssueKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmptyMesh => "empty-mesh",
            Self::IndexNotTriangleList => "index-not-triangle-list",
            Self::IndexOutOfRange => "index-out-of-range",
            Self::NonFinitePosition => "non-finite-position",
            Self::NonFiniteNormal => "non-finite-normal",
            Self::NormalNotUnit => "normal-not-unit",
            Self::NonFiniteUv => "non-finite-uv",
            Self::SkinWeightNegative => "skin-weight-negative",
            Self::SkinWeightSum => "skin-weight-sum",
            Self::SkinJointOutOfRange => "skin-joint-out-of-range",
            Self::PartsCoverage => "parts-coverage",
            Self::DegenerateAabb => "degenerate-aabb",
        }
    }
}

/// 一条 AIR 违规记录：类别 + 定位信息（网格名 / 顶点号 / 三角形号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirIssue {
    pub kind: AirIssueKind,
    pub detail: String,
}

impl AirIssue {
    pub fn new(kind: AirIssueKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for AirIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.detail)
    }
}

/// 校验一份网格是否满足 AIR 不变量；返回空表示通过。
///
/// `joint_count` 为 `Some(n)` 时额外校验蒙皮关节下标 `< n`（没有骨骼表时传 `None`）。
pub fn validate_with(mesh: &MeshData, joint_count: Option<usize>) -> Vec<AirIssue> {
    let mut issues = Vec::new();

    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        issues.push(AirIssue::new(
            AirIssueKind::EmptyMesh,
            format!("顶点 {} / 索引 {}", mesh.vertices.len(), mesh.indices.len()),
        ));
        return issues;
    }

    // 不变量 5：三角列表
    if mesh.indices.len() % 3 != 0 {
        issues.push(AirIssue::new(
            AirIssueKind::IndexNotTriangleList,
            format!("索引数 {} 不是 3 的倍数", mesh.indices.len()),
        ));
    }

    let vertex_count = mesh.vertices.len() as u32;
    if let Some(bad) = mesh
        .indices
        .iter()
        .enumerate()
        .find(|(_, i)| **i >= vertex_count)
    {
        issues.push(AirIssue::new(
            AirIssueKind::IndexOutOfRange,
            format!("索引 #{} = {}，顶点数 {}", bad.0, bad.1, vertex_count),
        ));
    }

    for (i, v) in mesh.vertices.iter().enumerate() {
        validate_vertex(v, i, joint_count, &mut issues);
    }

    // 不变量 10：AABB 非空且 min <= max
    let (min, max) = mesh.aabb();
    if !min.is_finite() || !max.is_finite() || (max - min).min_element() < -AABB_EPS {
        issues.push(AirIssue::new(
            AirIssueKind::DegenerateAabb,
            format!("min = {min:?}, max = {max:?}"),
        ));
    }

    issues
}

/// 同 [`validate_with`]，不做关节下标校验。
pub fn validate(mesh: &MeshData) -> Vec<AirIssue> {
    validate_with(mesh, None)
}

fn validate_vertex(v: &Vertex, i: usize, joint_count: Option<usize>, issues: &mut Vec<AirIssue>) {
    if !v.position.iter().all(|c| c.is_finite()) {
        issues.push(AirIssue::new(
            AirIssueKind::NonFinitePosition,
            format!("顶点 #{i} position = {:?}", v.position),
        ));
    }
    if !v.normal.iter().all(|c| c.is_finite()) {
        issues.push(AirIssue::new(
            AirIssueKind::NonFiniteNormal,
            format!("顶点 #{i} normal = {:?}", v.normal),
        ));
    } else {
        let len = Vec3::from(v.normal).length();
        if (len - 1.0).abs() > NORMAL_UNIT_EPS {
            issues.push(AirIssue::new(
                AirIssueKind::NormalNotUnit,
                format!("顶点 #{i} |normal| = {len}"),
            ));
        }
    }
    if !v.uv.iter().all(|c| c.is_finite()) {
        issues.push(AirIssue::new(
            AirIssueKind::NonFiniteUv,
            format!("顶点 #{i} uv = {:?}", v.uv),
        ));
    }

    // 不变量 7：蒙皮
    if v.weights
        .iter()
        .any(|w| !w.is_finite() || *w < -WEIGHT_SUM_EPS)
    {
        issues.push(AirIssue::new(
            AirIssueKind::SkinWeightNegative,
            format!("顶点 #{i} weights = {:?}", v.weights),
        ));
        return;
    }
    let sum: f32 = v.weights.iter().sum();
    if sum > WEIGHT_SUM_EPS && (sum - 1.0).abs() > WEIGHT_SUM_EPS {
        issues.push(AirIssue::new(
            AirIssueKind::SkinWeightSum,
            format!("顶点 #{i} 权重和 = {sum}"),
        ));
    }
    if sum > WEIGHT_SUM_EPS {
        if let Some(count) = joint_count {
            if let Some(bad) = v
                .weights
                .iter()
                .zip(v.joints.iter())
                .find(|(w, j)| **w > 0.0 && **j as usize >= count)
                .map(|(_, j)| *j)
            {
                issues.push(AirIssue::new(
                    AirIssueKind::SkinJointOutOfRange,
                    format!("顶点 #{i} joint = {bad}，关节数 {count}"),
                ));
            }
        }
    }
}

/// 校验材质分区是否无缝覆盖索引缓冲（不变量 9）。
pub fn validate_parts(mesh: &MeshData, parts: &[MeshPartData]) -> Vec<AirIssue> {
    if parts.is_empty() {
        return Vec::new(); // 空表示「整网格一段默认材质」
    }
    let mut issues = Vec::new();
    let total = mesh.indices.len() as u32;
    let mut cursor = 0u32;
    for (i, part) in parts.iter().enumerate() {
        if part.index_count % 3 != 0 {
            issues.push(AirIssue::new(
                AirIssueKind::PartsCoverage,
                format!("part #{i} index_count {} 不是 3 的倍数", part.index_count),
            ));
        }
        if part.first_index != cursor {
            issues.push(AirIssue::new(
                AirIssueKind::PartsCoverage,
                format!(
                    "part #{i} 起点 {} 与上一段终点 {cursor} 不接续",
                    part.first_index
                ),
            ));
        }
        cursor = part.first_index.saturating_add(part.index_count);
        if cursor > total {
            issues.push(AirIssue::new(
                AirIssueKind::PartsCoverage,
                format!("part #{i} 越过索引缓冲末尾（{cursor} > {total}）"),
            ));
        }
    }
    if cursor != total {
        issues.push(AirIssue::new(
            AirIssueKind::PartsCoverage,
            format!("parts 并集止于 {cursor}，索引缓冲长度 {total}"),
        ));
    }
    issues
}

/// 归一化工具箱：所有导入器**必须**复用这里的实现。
pub mod normalize {
    use super::*;

    /// 一套源坐标系（右 / 上 / 前三个基向量，用引擎坐标表达）。
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Axes {
        pub right: Vec3,
        pub up: Vec3,
        pub front: Vec3,
    }

    impl Axes {
        /// 引擎目标坐标系：+X 右、+Y 上、+Z 前。
        pub const ENGINE: Self = Self {
            right: Vec3::X,
            up: Vec3::Y,
            front: Vec3::Z,
        };

        /// glTF / glb：+X 右、+Y 上、**-Z 前**（右手，与引擎差一次 Z 镜像）。
        pub const GLTF: Self = Self {
            right: Vec3::X,
            up: Vec3::Y,
            front: Vec3::NEG_Z,
        };

        /// 把源坐标下的点换算到引擎坐标的矩阵（列向量 = 源基向量在引擎坐标中的表达）。
        pub fn to_engine_matrix(&self) -> Mat4 {
            Mat4::from_cols(
                self.right.extend(0.0),
                self.up.extend(0.0),
                self.front.extend(0.0),
                glam::Vec4::W,
            )
        }

        /// 行列式为负 ⇒ 手性翻转，导入后必须 [`super::normalize::flip_winding`]。
        pub fn is_mirrored(&self) -> bool {
            self.to_engine_matrix().determinant() < 0.0
        }
    }

    /// 按源坐标系把网格搬到引擎坐标系；镜像时自动翻转绕序。
    pub fn to_engine_axes(mesh: &mut MeshData, axes: Axes) {
        let m = axes.to_engine_matrix();
        if m == Mat4::IDENTITY {
            return;
        }
        bake_transform(mesh, m);
    }

    /// 烘焙一个变换到顶点上；法线走逆转置，负行列式自动翻转绕序。
    pub fn bake_transform(mesh: &mut MeshData, m: Mat4) {
        let normal_matrix = normal_matrix(m);
        for v in &mut mesh.vertices {
            let p = m.transform_point3(Vec3::from(v.position));
            v.position = p.to_array();
            let n = normal_matrix * Vec3::from(v.normal);
            let len = n.length();
            v.normal = if len > 1e-8 {
                (n / len).to_array()
            } else {
                n.to_array()
            };
        }
        if m.determinant() < 0.0 {
            flip_winding(&mut mesh.indices);
        }
    }

    fn normal_matrix(m: Mat4) -> Mat3 {
        let linear = Mat3::from_mat4(m);
        if linear.determinant().abs() < 1e-12 {
            return Mat3::IDENTITY;
        }
        linear.inverse().transpose()
    }

    /// 翻转三角形绕序（每三角形交换第 0 与第 2 个索引）。
    pub fn flip_winding(indices: &mut [u32]) {
        let triangles = indices.len() / 3;
        for t in 0..triangles {
            indices.swap(t * 3, t * 3 + 2);
        }
    }

    /// 按三角形绕序重算法线（累加后归一，退化三角形保留原法线）。
    pub fn recalc_normals(mesh: &mut MeshData) {
        let mut acc = vec![Vec3::ZERO; mesh.vertices.len()];
        for tri in mesh.indices.chunks_exact(3) {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if a >= acc.len() || b >= acc.len() || c >= acc.len() {
                continue;
            }
            let pa = Vec3::from(mesh.vertices[a].position);
            let pb = Vec3::from(mesh.vertices[b].position);
            let pc = Vec3::from(mesh.vertices[c].position);
            let n = (pb - pa).cross(pc - pa);
            acc[a] += n;
            acc[b] += n;
            acc[c] += n;
        }
        for (v, n) in mesh.vertices.iter_mut().zip(acc) {
            let len = n.length();
            if len > 1e-8 {
                v.normal = (n / len).to_array();
            }
        }
    }

    /// 非索引网格补成三角形列表索引（`0..vertices.len()`）。
    pub fn ensure_indices(mesh: &mut MeshData) {
        if mesh.indices.is_empty() && !mesh.vertices.is_empty() {
            mesh.indices = (0..mesh.vertices.len() as u32).collect();
            if mesh.indices.len() % 3 != 0 {
                mesh.indices.truncate(mesh.indices.len() / 3 * 3);
            }
        }
    }

    /// `TRIANGLE_STRIP` → 三角形列表。
    pub fn triangles_from_strip(strip: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(strip.len().saturating_sub(2) * 3);
        for i in 2..strip.len() {
            if i % 2 == 0 {
                out.extend_from_slice(&[strip[i - 2], strip[i - 1], strip[i]]);
            } else {
                out.extend_from_slice(&[strip[i - 1], strip[i - 2], strip[i]]);
            }
        }
        out
    }

    /// `TRIANGLE_FAN` → 三角形列表。
    pub fn triangles_from_fan(fan: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(fan.len().saturating_sub(2) * 3);
        for i in 1..fan.len().saturating_sub(1) {
            out.extend_from_slice(&[fan[0], fan[i], fan[i + 1]]);
        }
        out
    }

    /// 蒙皮压缩：取权重最大的 `max`（≤4）个关节并重归一。
    ///
    /// 返回是否改动过该顶点。权重和为 0 的顶点视为非蒙皮，原样保留。
    pub fn compact_skinning_vertex(v: &mut Vertex, max: usize) -> bool {
        let max = max.clamp(1, 4);
        let sum: f32 = v.weights.iter().sum();
        if sum <= WEIGHT_SUM_EPS {
            return false;
        }
        let mut pairs: Vec<(u16, f32)> = v
            .joints
            .iter()
            .copied()
            .zip(v.weights.iter().copied())
            .filter(|(_, w)| *w > 0.0)
            .collect();
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let changed = pairs.len() > max || (sum - 1.0).abs() > WEIGHT_SUM_EPS;
        pairs.truncate(max);
        let kept: f32 = pairs.iter().map(|(_, w)| *w).sum();
        if kept <= WEIGHT_SUM_EPS {
            return false;
        }
        let mut joints = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for (i, (j, w)) in pairs.iter().enumerate() {
            joints[i] = *j;
            weights[i] = w / kept;
        }
        v.joints = joints;
        v.weights = weights;
        changed
    }

    /// 对整个网格做 §7 的蒙皮压缩（每顶点 ≤4、权重和 = 1）。
    pub fn compact_skinning(mesh: &mut MeshData) -> usize {
        let mut changed = 0usize;
        for v in mesh.vertices.iter_mut() {
            if compact_skinning_vertex(v, 4) {
                changed += 1;
            }
        }
        changed
    }

    /// 顶点焊接：位置 / 法线 / UV / 蒙皮按 `quantum` 量化后相同的顶点合并。
    ///
    /// 返回合并后的顶点数变化（正数表示减少）。保持顶点首次出现顺序，结果确定。
    pub fn weld_vertices(mesh: &mut MeshData, quantum: f32) -> usize {
        let q = if quantum > 0.0 { quantum } else { 1e-5 };
        let before = mesh.vertices.len();
        let mut seen: std::collections::HashMap<Vec<i64>, u32> = std::collections::HashMap::new();
        let mut remap = vec![0u32; before];
        let mut vertices: Vec<Vertex> = Vec::with_capacity(before);
        for (i, v) in mesh.vertices.iter().enumerate() {
            let mut key: Vec<i64> = Vec::with_capacity(14);
            for c in v.position.iter().chain(v.normal.iter()).chain(v.uv.iter()) {
                key.push((*c as f64 / q as f64).round() as i64);
            }
            for (w, j) in v.weights.iter().zip(v.joints.iter()) {
                key.push((*w as f64 / q as f64).round() as i64);
                key.push(*j as i64);
            }
            let next = vertices.len() as u32;
            let index = *seen.entry(key).or_insert_with(|| {
                vertices.push(*v);
                next
            });
            remap[i] = index;
        }
        if vertices.len() == before {
            return 0;
        }
        for i in mesh.indices.iter_mut() {
            if let Some(mapped) = remap.get(*i as usize) {
                *i = *mapped;
            }
        }
        mesh.vertices = vertices;
        before - mesh.vertices.len()
    }

    /// 补齐缺省（UV / 法线）并跑完整校验；`Err` 携带全部违规项。
    ///
    /// S0 阶段调用方只记录告警，不因此拒绝导入。
    pub fn finalize(mesh: &mut MeshData) -> Result<(), Vec<AirIssue>> {
        for v in &mut mesh.vertices {
            if !v.uv.iter().all(|c| c.is_finite()) {
                v.uv = [0.0, 0.0];
            }
            if !v.normal.iter().all(|c| c.is_finite()) {
                v.normal = [0.0, 1.0, 0.0];
            }
        }
        let issues = validate(mesh);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

/// 网格指纹：用于「同文件两次导入结果逐字节一致」的确定性回归。
///
/// 按 f32 位模式累加 FNV-1a 64，避免浮点比较与 NaN 陷阱。
pub fn mesh_fingerprint(mesh: &MeshData) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for v in &mesh.vertices {
        for c in v.position.iter().chain(v.normal.iter()).chain(v.uv.iter()) {
            mix(&c.to_bits().to_le_bytes());
        }
        for j in &v.joints {
            mix(&j.to_le_bytes());
        }
        for w in &v.weights {
            mix(&w.to_bits().to_le_bytes());
        }
    }
    for i in &mesh.indices {
        mix(&i.to_le_bytes());
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> MeshData {
        // 两个三角形，CCW，单位正方形
        MeshData::new(
            vec![
                Vertex::new([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0]),
                Vertex::new([1.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 1.0]),
                Vertex::new([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 1.0]),
            ],
            vec![0, 2, 1, 0, 3, 2],
        )
    }

    fn kinds(issues: &[AirIssue]) -> Vec<AirIssueKind> {
        issues.iter().map(|i| i.kind).collect()
    }

    #[test]
    fn valid_mesh_passes() {
        assert_eq!(validate(&quad()), Vec::new());
    }

    #[test]
    fn rejects_index_not_triangle_list() {
        let mut mesh = quad();
        mesh.indices.pop();
        assert!(kinds(&validate(&mesh)).contains(&AirIssueKind::IndexNotTriangleList));
    }

    #[test]
    fn rejects_index_out_of_range() {
        let mut mesh = quad();
        mesh.indices[0] = 99;
        assert!(kinds(&validate(&mesh)).contains(&AirIssueKind::IndexOutOfRange));
    }

    #[test]
    fn rejects_non_finite_position_and_uv() {
        let mut mesh = quad();
        mesh.vertices[1].position[0] = f32::NAN;
        mesh.vertices[2].uv[1] = f32::INFINITY;
        let found = kinds(&validate(&mesh));
        assert!(found.contains(&AirIssueKind::NonFinitePosition));
        assert!(found.contains(&AirIssueKind::NonFiniteUv));
    }

    #[test]
    fn rejects_normal_not_unit() {
        let mut mesh = quad();
        mesh.vertices[0].normal = [0.0, 2.0, 0.0];
        assert!(kinds(&validate(&mesh)).contains(&AirIssueKind::NormalNotUnit));
    }

    #[test]
    fn rejects_skin_weight_sum_and_negative() {
        let mut mesh = quad();
        mesh.vertices[0].joints = [0, 1, 0, 0];
        mesh.vertices[0].weights = [0.5, 0.2, 0.0, 0.0];
        assert!(kinds(&validate(&mesh)).contains(&AirIssueKind::SkinWeightSum));

        mesh.vertices[0].weights = [-0.1, 1.0, 0.0, 0.0];
        assert!(kinds(&validate(&mesh)).contains(&AirIssueKind::SkinWeightNegative));
    }

    #[test]
    fn rejects_skin_joint_out_of_range() {
        let mut mesh = quad();
        mesh.vertices[0].joints = [7, 0, 0, 0];
        mesh.vertices[0].weights = [1.0, 0.0, 0.0, 0.0];
        assert_eq!(validate_with(&mesh, None), Vec::new());
        assert!(kinds(&validate_with(&mesh, Some(4))).contains(&AirIssueKind::SkinJointOutOfRange));
    }

    #[test]
    fn rejects_parts_gap_and_overrun() {
        let mesh = quad();
        let parts = vec![
            MeshPartData {
                first_index: 0,
                index_count: 3,
                material: None,
            },
            MeshPartData {
                first_index: 4,
                index_count: 3,
                material: None,
            },
        ];
        assert!(kinds(&validate_parts(&mesh, &parts)).contains(&AirIssueKind::PartsCoverage));

        let covered = vec![MeshPartData {
            first_index: 0,
            index_count: 6,
            material: None,
        }];
        assert_eq!(validate_parts(&mesh, &covered), Vec::new());
    }

    #[test]
    fn gltf_axes_mirror_flips_winding() {
        let mut mesh = quad();
        let before = mesh.indices.clone();
        assert!(normalize::Axes::GLTF.is_mirrored());
        normalize::to_engine_axes(&mut mesh, normalize::Axes::GLTF);
        // Z 镜像：位置翻号，绕序反转
        assert_eq!(mesh.vertices[1].position, [1.0, 0.0, -0.0]);
        assert_eq!(mesh.indices[0], before[2]);
        assert_eq!(mesh.indices[2], before[0]);
    }

    #[test]
    fn engine_axes_is_noop() {
        let mut mesh = quad();
        let fp = mesh_fingerprint(&mesh);
        normalize::to_engine_axes(&mut mesh, normalize::Axes::ENGINE);
        assert_eq!(mesh_fingerprint(&mesh), fp);
    }

    #[test]
    fn flip_winding_twice_restores() {
        let mut mesh = quad();
        let before = mesh.indices.clone();
        normalize::flip_winding(&mut mesh.indices);
        assert_ne!(mesh.indices, before);
        normalize::flip_winding(&mut mesh.indices);
        assert_eq!(mesh.indices, before);
    }

    #[test]
    fn recalc_normals_points_up_for_ccw_ground() {
        let mut mesh = quad();
        for v in &mut mesh.vertices {
            v.normal = [0.0, 0.0, 0.0];
        }
        normalize::recalc_normals(&mut mesh);
        for v in &mesh.vertices {
            assert!((v.normal[1] - 1.0).abs() < 1e-5, "{:?}", v.normal);
        }
    }

    #[test]
    fn ensure_indices_expands_non_indexed() {
        // 4 顶点非索引 → 补成 1 个三角形（尾部不完整的三元组被截断）
        let mut mesh = MeshData::new(quad().vertices, Vec::new());
        normalize::ensure_indices(&mut mesh);
        assert_eq!(mesh.indices.len(), 3);
        assert_eq!(validate(&mesh), Vec::new());

        // 6 顶点 → 完整两个三角形
        let mut verts = quad().vertices.clone();
        verts.push(Vertex::new([2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0]));
        verts.push(Vertex::new([2.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0]));
        let mut mesh = MeshData::new(verts, Vec::new());
        normalize::ensure_indices(&mut mesh);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(validate(&mesh), Vec::new());
    }

    #[test]
    fn strip_and_fan_become_triangle_lists() {
        let strip = normalize::triangles_from_strip(&[0, 1, 2, 3]);
        assert_eq!(strip, vec![0, 1, 2, 2, 1, 3]);
        let fan = normalize::triangles_from_fan(&[0, 1, 2, 3]);
        assert_eq!(fan, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn compact_skinning_keeps_4_and_renormalizes() {
        let mut v = Vertex::skinned(
            [0.0; 3],
            [0.0, 1.0, 0.0],
            [0.0; 2],
            [1, 2, 3, 4],
            [0.1, 0.2, 0.3, 0.4],
        );
        assert!(!normalize::compact_skinning_vertex(&mut v, 4));
        let mut v2 = Vertex::skinned(
            [0.0; 3],
            [0.0, 1.0, 0.0],
            [0.0; 2],
            [7, 8, 0, 0],
            [3.0, 1.0, 0.0, 0.0],
        );
        assert!(normalize::compact_skinning_vertex(&mut v2, 4));
        assert_eq!(v2.joints, [7, 8, 0, 0]);
        assert!((v2.weights[0] - 0.75).abs() < 1e-6);
        assert!((v2.weights[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn weld_merges_duplicate_vertices() {
        let mut mesh = quad();
        // 把一个三角形改成重复顶点构成的同位置几何，焊接后顶点数应减少
        mesh.vertices
            .push(Vertex::new([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0]));
        mesh.indices.extend_from_slice(&[0, 4, 2]);
        let removed = normalize::weld_vertices(&mut mesh, 1e-5);
        assert_eq!(removed, 1);
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices.last(), Some(&2));
    }

    #[test]
    fn finalize_repairs_defaults_and_reports_rest() {
        let mut mesh = quad();
        mesh.vertices[0].uv = [f32::NAN, 0.0];
        assert_eq!(normalize::finalize(&mut mesh), Ok(()));
        assert_eq!(mesh.vertices[0].uv, [0.0, 0.0]);

        let mut bad = quad();
        bad.indices[0] = 9;
        assert!(normalize::finalize(&mut bad).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let a = quad();
        let mut b = quad();
        assert_eq!(mesh_fingerprint(&a), mesh_fingerprint(&b));
        b.vertices[0].position[0] = 0.5;
        assert_ne!(mesh_fingerprint(&a), mesh_fingerprint(&b));
    }

    /// 内置图元必须满足同一份 AIR（S1 归位前先在此断言契约本身）。
    #[test]
    fn procedural_primitives_satisfy_air() {
        for kind in crate::BuiltinMesh::ALL {
            let mesh = crate::procedural::builtin_mesh(kind);
            assert_eq!(validate(&mesh), Vec::new(), "{kind:?}");
        }
    }
}
