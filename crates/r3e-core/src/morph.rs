//! 顶点形变（morph target / blend shape，扩展三十六）。
//!
//! 与 P4 蒙皮的分工不同：蒙皮把顶点交给一组关节矩阵去「摆姿势」，morph 只给出
//! **每个顶点沿某个方向的位移量**，按标量权重线性叠加。两者不冲突，着色器里的
//! 顺序是 `morph 位移 → 蒙皮矩阵 → 实例矩阵`，所以「说话时眨眼」这类叠加是免费的。
//!
//! 这一层只放**与图形 API 无关**的数据与数学：
//!
//! * [`MorphTarget`] —— 单个形状（一个表情 / 一个口型），每顶点一组位置增量；
//! * [`MorphSet`] —— 一个网格上的全部形状，顺序即脚本里的权重下标；
//! * [`MorphInstance`] —— 场景侧的一条绑定：哪个网格节点、各形状当前权重。
//!
//! # 与着色器的同一份公式
//!
//! 着色器（`shader.wgsl` 的 `morph_offset`）逐字按下式累加：
//!
//! ```text
//! p' = p + Σ_t w_t · Δp_t
//! n' = n + Σ_t w_t · Δn_t          （随后统一 normalize）
//! T' = T + Σ_t w_t · ΔT_t          （xyz 累加、手性 w 不变，随后重新正交化）
//! ```
//!
//! 切线增量（扩展三十七）是**可选**通道：形状可以不提供，此时该形状对切线的贡献为 0，
//! 强形变下法线贴图仍按基础切线方向解算。提供时缓冲区每顶点固定留 3 个 f32 的槽位
//! （见 [`MORPH_VERTEX_FLOATS`]），缺省形状补 0 —— 布局只有一种，着色器不需要分支。
//!
//! [`MorphSet::apply`] 是同一公式的 CPU 版，只用于**对拍与单测**：GPU 侧一旦写错，
//! 这里能给出可断言的期望值，不需要读回像素。

use std::fmt;

use crate::mesh::{MeshData, Vertex};
use crate::NodeId;

/// 单个实例可同时启用的形状数上限（权重一帧一个 `vec4` 传进顶点着色器）。
///
/// 表情 / 口型实际用量通常 2~4 个；超过该值的形状只能挤在同一格里 ——
/// 需要更多就分批切换权重，而不是加宽缓冲（加宽会让每个实例的顶点属性涨 16 字节）。
pub const MAX_MORPH_TARGETS: usize = 4;

/// 形变表里**每顶点**占用的 `f32` 个数（扩展三十七把 6 抬到 9）。
///
/// 布局：`[0..3) = Δ位置 xyz`、`[3..6) = Δ法线 xyz`、`[6..9) = Δ切线 xyz`。
///
/// 三个通道**恒占槽位**，形状没给切线时那 3 个 f32 就是 0。这样着色器的下标算术
/// 只有一种（`base + vertex * 9 + k`），不需要按形状切换布局 —— 代价是表变大 50%，
/// 在 [`crate::MAX_MORPH_TARGETS`] 的前提下完全可接受（见 ARCHITECTURE §4.21 的容量表）。
pub const MORPH_VERTEX_FLOATS: usize = 9;

/// 位置增量在一顶点槽位内的起点。
pub const MORPH_POSITION_OFFSET: usize = 0;
/// 法线增量在一顶点槽位内的起点。
pub const MORPH_NORMAL_OFFSET: usize = 3;
/// 切线增量在一顶点槽位内的起点（扩展三十七）。
pub const MORPH_TANGENT_OFFSET: usize = 6;

/// 形变数据校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MorphIssue {
    /// 某个形状的增量数组长度与网格顶点数不一致（下标会越界）。
    VertexCountMismatch {
        target: usize,
        expected: usize,
        got: usize,
    },
    /// 法线增量长度既不是 0（未提供）也不是顶点数。
    NormalCountMismatch {
        target: usize,
        expected: usize,
        got: usize,
    },
    /// 切线增量长度既不是 0（未提供）也不是顶点数（扩展三十七）。
    TangentCountMismatch {
        target: usize,
        expected: usize,
        got: usize,
    },
    /// 增量里出现 NaN / 无穷 —— 一旦进缓冲就会让整块几何的包围盒与剔除失效。
    NonFinite { target: usize, vertex: usize },
}

impl fmt::Display for MorphIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MorphIssue::VertexCountMismatch {
                target,
                expected,
                got,
            } => write!(
                f,
                "morph target #{target} 的位置增量有 {got} 个顶点，网格有 {expected} 个（必须等长）"
            ),
            MorphIssue::NormalCountMismatch {
                target,
                expected,
                got,
            } => write!(
                f,
                "morph target #{target} 的法线增量有 {got} 个顶点，网格有 {expected} 个"
            ),
            MorphIssue::TangentCountMismatch {
                target,
                expected,
                got,
            } => write!(
                f,
                "morph target #{target} 的切线增量有 {got} 个顶点，网格有 {expected} 个"
            ),
            MorphIssue::NonFinite { target, vertex } => write!(
                f,
                "morph target #{target} 第 {vertex} 个顶点的增量不是有限数"
            ),
        }
    }
}

impl std::error::Error for MorphIssue {}

/// 单个形变形状：每个顶点一组位置增量，可选一组法线增量与一组切线增量。
///
/// 增量是**相对基础网格的差**，不是绝对坐标 —— 所以它和顶点一一对应、长度必须等于
/// 顶点数，但坐标空间与顶点相同（导入时随顶点一起过轴变换）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MorphTarget {
    /// 形状名（glTF 的 `extras.targetNames` 或脚本自定的名字），只用于日志与脚本定位。
    pub name: String,
    /// 每顶点位置增量，长度必须等于网格顶点数。
    pub positions: Vec<[f32; 3]>,
    /// 每顶点法线增量；空表示该形状只动位置。
    pub normals: Vec<[f32; 3]>,
    /// 每顶点切线增量（扩展三十七）；空表示该形状不动切线。
    ///
    /// 只存 xyz：地形变后切线的**手性 w 保持不变** —— 形变是局部拉伸，不翻面，
    /// 手性只跟 UV 镜像有关，跟顶点位置无关。
    pub tangents: Vec<[f32; 3]>,
}

impl MorphTarget {
    /// 只带位置增量的形状。
    pub fn new(name: impl Into<String>, positions: Vec<[f32; 3]>) -> Self {
        Self {
            name: name.into(),
            positions,
            normals: Vec::new(),
            tangents: Vec::new(),
        }
    }

    /// 补上法线增量（链式构造）。
    pub fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
        self.normals = normals;
        self
    }

    /// 补上切线增量（链式构造，扩展三十七）。
    pub fn with_tangents(mut self, tangents: Vec<[f32; 3]>) -> Self {
        self.tangents = tangents;
        self
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn has_normals(&self) -> bool {
        !self.normals.is_empty()
    }

    /// 该形状是否带切线增量（扩展三十七）。
    pub fn has_tangents(&self) -> bool {
        !self.tangents.is_empty()
    }

    /// 该形状是否全为 0（导入空形状时可以直接丢掉，省一段缓冲）。
    pub fn is_zero(&self) -> bool {
        self.positions
            .iter()
            .all(|d| d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0)
            && self
                .normals
                .iter()
                .all(|d| d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0)
            && self
                .tangents
                .iter()
                .all(|d| d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0)
    }

    /// 按顶点下标取位置增量；越界返回 0（调用方应先 `validate`）。
    pub fn position_at(&self, vertex: usize) -> [f32; 3] {
        self.positions.get(vertex).copied().unwrap_or([0.0; 3])
    }

    /// 按顶点下标取法线增量；未提供时返回 0。
    pub fn normal_at(&self, vertex: usize) -> [f32; 3] {
        self.normals.get(vertex).copied().unwrap_or([0.0; 3])
    }

    /// 按顶点下标取切线增量；未提供时返回 0（扩展三十七）。
    pub fn tangent_at(&self, vertex: usize) -> [f32; 3] {
        self.tangents.get(vertex).copied().unwrap_or([0.0; 3])
    }

    /// 线性缩放一份增量（导入时烘焙轴变换 / 镜像用：线性部分作用在增量上）。
    pub fn scale(&mut self, factor: f32) {
        for d in &mut self.positions {
            for c in d.iter_mut() {
                *c *= factor;
            }
        }
        for d in &mut self.normals {
            for c in d.iter_mut() {
                *c *= factor;
            }
        }
        for d in &mut self.tangents {
            for c in d.iter_mut() {
                *c *= factor;
            }
        }
    }
}

/// 一个网格上的全部形变形状。顺序即脚本权重下标，长度受 [`MAX_MORPH_TARGETS`] 建议约束。
#[derive(Debug, Clone, Default)]
pub struct MorphSet {
    pub targets: Vec<MorphTarget>,
}

impl MorphSet {
    pub fn new(targets: Vec<MorphTarget>) -> Self {
        Self { targets }
    }

    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// 顶点数（取第一个形状的长度，空集返回 0）。
    pub fn vertex_count(&self) -> usize {
        self.targets.first().map_or(0, |t| t.vertex_count())
    }

    pub fn get(&self, index: usize) -> Option<&MorphTarget> {
        self.targets.get(index)
    }

    pub fn name_of(&self, index: usize) -> Option<&str> {
        self.targets.get(index).map(|t| t.name.as_str())
    }

    /// 校验内部一致性（各形状彼此等长）与有限性。
    pub fn validate(&self) -> Result<(), MorphIssue> {
        let expected = self.vertex_count();
        self.validate_against(expected)
    }

    /// 校验「能贴到这个顶点数的网格上」。渲染层在上传前用网格的真实顶点数调用；
    /// 不通过时整组形状按「没有形变」处理 —— 绝不让一组错长度的增量进缓冲
    /// （越界读在 GPU 上是未定义行为，不是 panic）。
    pub fn validate_against(&self, expected: usize) -> Result<(), MorphIssue> {
        for (index, target) in self.targets.iter().enumerate() {
            if target.vertex_count() != expected {
                return Err(MorphIssue::VertexCountMismatch {
                    target: index,
                    expected,
                    got: target.vertex_count(),
                });
            }
            if target.has_normals() && target.normals.len() != expected {
                return Err(MorphIssue::NormalCountMismatch {
                    target: index,
                    expected,
                    got: target.normals.len(),
                });
            }
            if target.has_tangents() && target.tangents.len() != expected {
                return Err(MorphIssue::TangentCountMismatch {
                    target: index,
                    expected,
                    got: target.tangents.len(),
                });
            }
            for (vertex, d) in target.positions.iter().enumerate() {
                if !d.iter().all(|c| c.is_finite()) {
                    return Err(MorphIssue::NonFinite {
                        target: index,
                        vertex,
                    });
                }
            }
            // 法线 / 切线增量同样要过有限性检查：NaN 会顺着 normalize 的输入一路传到
            // 光照结果，让整块几何出现黑斑或熄灭（位置有限不代表通道有限）。
            for (vertex, d) in target.normals.iter().enumerate() {
                if !d.iter().all(|c| c.is_finite()) {
                    return Err(MorphIssue::NonFinite {
                        target: index,
                        vertex,
                    });
                }
            }
            for (vertex, d) in target.tangents.iter().enumerate() {
                if !d.iter().all(|c| c.is_finite()) {
                    return Err(MorphIssue::NonFinite {
                        target: index,
                        vertex,
                    });
                }
            }
        }
        Ok(())
    }

    /// 扁平化成 GPU 表布局：每个形状内按顶点排列，每顶点 [`MORPH_VERTEX_FLOATS`] 个 f32
    /// （位置 + 法线 + 切线三组增量 xyz，见 [`MORPH_POSITION_OFFSET`] 等偏移常量）。
    /// 返回 `(数据, 每形状浮点数)`。
    ///
    /// 三个通道**交错**存是为了少几个 binding：着色器一次算出下标就能取齐三组，
    /// 局部性也好（一条 cache line 覆盖同一顶点的全部增量）。
    pub fn flatten(&self) -> (Vec<f32>, usize) {
        self.flatten_limited(self.targets.len())
    }

    /// 只扁平平前 `limit` 个形状。渲染层按 [`MAX_MORPH_TARGETS`] 调用 ——
    /// 每实例权重只有一个 `vec4`，多出来的形状传上去也没人读，白白占缓冲。
    pub fn flatten_limited(&self, limit: usize) -> (Vec<f32>, usize) {
        let vertices = self.vertex_count();
        let stride = vertices * MORPH_VERTEX_FLOATS;
        let count = self.targets.len().min(limit);
        let mut out = Vec::with_capacity(count * stride);
        for target in self.targets.iter().take(count) {
            for v in 0..vertices {
                let p = target.position_at(v);
                let n = target.normal_at(v);
                let t = target.tangent_at(v);
                out.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2], t[0], t[1], t[2]]);
            }
        }
        (out, stride)
    }

    /// CPU 侧求解（与 `shader.wgsl` 的 `morph_offset` 同一公式），只用于对拍与单测。
    ///
    /// 权重按 `MAX_MORPH_TARGETS` 截断，非有限权重视为 0；法线 / 切线累加后**不**归一化
    /// （着色器是在乘完模型伴随矩阵之后统一 normalize 的，这里保持中间值才可比）。
    pub fn apply(&self, mesh: &MeshData, weights: &[f32; MAX_MORPH_TARGETS]) -> MeshData {
        let mut out = mesh.clone();
        let count = self.targets.len().min(MAX_MORPH_TARGETS);
        for (index, target) in self.targets.iter().take(count).enumerate() {
            let weight = weights[index];
            if !weight.is_finite() || weight == 0.0 {
                continue;
            }
            for (vertex_index, vertex) in out.vertices.iter_mut().enumerate() {
                let dp = target.position_at(vertex_index);
                let dn = target.normal_at(vertex_index);
                let dt = target.tangent_at(vertex_index);
                for k in 0..3 {
                    vertex.position[k] += weight * dp[k];
                    vertex.normal[k] += weight * dn[k];
                    // 切线只动 xyz，手性 w 保持不变（形变不改变 UV 镜像关系）。
                    vertex.tangent[k] += weight * dt[k];
                }
            }
        }
        out
    }

    /// 按权重求解**单个顶点**（批量对拍时比整网格更省）。
    pub fn vertex_at(
        &self,
        base: &Vertex,
        vertex_index: usize,
        weights: &[f32; MAX_MORPH_TARGETS],
    ) -> Vertex {
        let mut v = *base;
        let count = self.targets.len().min(MAX_MORPH_TARGETS);
        for (index, target) in self.targets.iter().take(count).enumerate() {
            let weight = weights[index];
            if !weight.is_finite() || weight == 0.0 {
                continue;
            }
            let dp = target.position_at(vertex_index);
            let dn = target.normal_at(vertex_index);
            let dt = target.tangent_at(vertex_index);
            for k in 0..3 {
                v.position[k] += weight * dp[k];
                v.normal[k] += weight * dn[k];
                v.tangent[k] += weight * dt[k];
            }
        }
        v
    }
}

impl From<Vec<MorphTarget>> for MorphSet {
    fn from(targets: Vec<MorphTarget>) -> Self {
        Self { targets }
    }
}

/// 场景侧的一条形变绑定：网格节点 + 各形状当前权重。
///
/// 与 `SkinInstance` 不同，这里没有「每帧求解」这一步 —— 权重直接被渲染层搬进
/// 实例缓冲，形变发生在顶点着色器里。所以这条记录的全部状态就是四个标量。
#[derive(Debug, Clone, PartialEq)]
pub struct MorphInstance {
    /// 承载形变的网格节点（与 `Scene::skin_of` 用同一套节点 id）。
    pub mesh_node: NodeId,
    /// 各形状权重，下标即 `MorphSet::targets` 的顺序。
    pub weights: [f32; MAX_MORPH_TARGETS],
    /// 该网格实际存在的形状数；权重只需前 `target_count` 个。
    pub target_count: usize,
}

impl MorphInstance {
    pub fn new(mesh_node: NodeId, target_count: usize) -> Self {
        Self {
            mesh_node,
            weights: [0.0; MAX_MORPH_TARGETS],
            target_count: target_count.min(MAX_MORPH_TARGETS),
        }
    }

    /// 设置一个形状的权重。越界 / 非有限值一律拒绝并返回 `false`（不改动任何状态）。
    pub fn set_weight(&mut self, index: usize, weight: f32) -> bool {
        if index >= self.target_count.min(MAX_MORPH_TARGETS) || !weight.is_finite() {
            return false;
        }
        self.weights[index] = weight;
        true
    }

    pub fn weight_of(&self, index: usize) -> Option<f32> {
        if index < self.target_count.min(MAX_MORPH_TARGETS) {
            Some(self.weights[index])
        } else {
            None
        }
    }

    /// 把全部权重清零（「恢复原样」）。
    pub fn reset(&mut self) {
        self.weights = [0.0; MAX_MORPH_TARGETS];
    }

    /// 是否有任何形状在起作用。全 0 时渲染层会跳过整条形变路径。
    pub fn is_active(&self) -> bool {
        self.weights
            .iter()
            .take(self.target_count.min(MAX_MORPH_TARGETS))
            .any(|w| *w != 0.0 && w.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> MeshData {
        MeshData::from_arrays(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[[0.0, 0.0, 1.0]; 3],
            None,
            vec![0, 1, 2],
        )
    }

    fn smile() -> MorphTarget {
        // 顶点 1 向右上抬 1，顶点 2 不动
        MorphTarget::new("smile", vec![[0.0; 3], [1.0, 1.0, 0.0], [0.0; 3]])
    }

    /// 契约一：形状长度必须与网格顶点数一致，否则拒绝上传。
    #[test]
    fn validate_rejects_length_mismatch() {
        let good = MorphSet::new(vec![smile()]);
        assert_eq!(good.validate_against(3), Ok(()));

        let bad = MorphSet::new(vec![MorphTarget::new("bad", vec![[0.0; 3]; 2])]);
        match bad.validate_against(3) {
            Err(MorphIssue::VertexCountMismatch {
                target,
                expected,
                got,
            }) => {
                assert_eq!((target, expected, got), (0, 3, 2), "期望取自网格顶点数");
            }
            other => panic!("应报顶点数不一致，实际 {other:?}"),
        }
        // 两组形状彼此不等长时，即便不传网格顶点数也能自查出来
        let ragged = MorphSet::new(vec![smile(), MorphTarget::new("short", vec![[0.0; 3]; 2])]);
        assert!(ragged.validate().is_err());
    }

    /// 契约二：法线增量要么不提供，要么与顶点数等长。
    #[test]
    fn validate_checks_normal_length() {
        let mut target = smile();
        target.normals = vec![[0.0; 3]; 2];
        let set = MorphSet::new(vec![target]);
        match set.validate() {
            Err(MorphIssue::NormalCountMismatch { expected, got, .. }) => {
                assert_eq!(expected, 3);
                assert_eq!(got, 2);
            }
            other => panic!("应报法线数不一致，实际 {other:?}"),
        }
    }

    /// 契约三：NaN 增量必须被挡在上传前（进缓冲会污染包围盒）。
    #[test]
    fn validate_rejects_non_finite() {
        let mut target = smile();
        target.positions[2] = [f32::NAN, 0.0, 0.0];
        let set = MorphSet::new(vec![target]);
        match set.validate() {
            Err(MorphIssue::NonFinite { target, vertex }) => {
                assert_eq!((target, vertex), (0, 2));
            }
            other => panic!("应报非有限数，实际 {other:?}"),
        }
    }

    /// 权重 0 与权重 1 的 CPU 求解：0 时逐位不变，1 时等于基础网格 + 增量。
    #[test]
    fn apply_matches_linear_blend() {
        let mesh = triangle();
        let set = MorphSet::new(vec![smile()]);

        let zero = set.apply(&mesh, &[0.0; MAX_MORPH_TARGETS]);
        for (a, b) in zero.vertices.iter().zip(mesh.vertices.iter()) {
            assert_eq!(a.position, b.position);
        }

        let full = set.apply(&mesh, &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(full.vertices[1].position, [2.0, 1.0, 0.0]);
        assert_eq!(full.vertices[2].position, [0.0, 1.0, 0.0]);

        let half = set.apply(&mesh, &[0.5, 0.0, 0.0, 0.0]);
        assert_eq!(half.vertices[1].position, [1.5, 0.5, 0.0]);
    }

    /// 两个形状按各自权重叠加，且顺序无关（线性）。
    #[test]
    fn apply_sums_targets_linearly() {
        let mesh = triangle();
        let shift = MorphTarget::new("shift", vec![[0.0; 3], [0.0; 3], [0.0, 0.0, 2.0]]);
        let set = MorphSet::new(vec![smile(), shift]);
        let a = set.apply(&mesh, &[1.0, 1.0, 0.0, 0.0]);
        let b = set.apply(&mesh, &[1.0, 0.5, 0.0, 0.0]);
        assert_eq!(a.vertices[1].position, [2.0, 1.0, 0.0]);
        assert_eq!(a.vertices[2].position, [0.0, 1.0, 2.0]);
        assert_eq!(b.vertices[2].position, [0.0, 1.0, 1.0]);
    }

    /// 非有限权重按 0 处理，不会把网格拖成 NaN。
    #[test]
    fn non_finite_weight_is_ignored() {
        let mesh = triangle();
        let set = MorphSet::new(vec![smile()]);
        let out = set.apply(&mesh, &[f32::INFINITY, 0.0, 0.0, 0.0]);
        assert!(out
            .vertices
            .iter()
            .all(|v| v.position.iter().all(|c| c.is_finite())));
        assert_eq!(out.vertices[1].position, mesh.vertices[1].position);
    }

    /// 扁平化布局：每形状 `顶点数 × MORPH_VERTEX_FLOATS` 个浮点，
    /// 每顶点依次是位置 / 法线 / 切线三组增量。
    #[test]
    fn flatten_interleaves_position_normal_and_tangent() {
        let per_vertex = MORPH_VERTEX_FLOATS;
        let set = MorphSet::new(vec![smile()]);
        let (data, stride) = set.flatten();
        assert_eq!(stride, 3 * per_vertex);
        assert_eq!(data.len(), 3 * per_vertex);
        // 顶点 1 的位置增量 (1,1,0)，法线增量全 0，切线增量全 0（smile 没给切线通道）
        assert_eq!(
            &data[per_vertex..per_vertex + 6],
            &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert!(data[(per_vertex + 6)..(2 * per_vertex)]
            .iter()
            .all(|v| *v == 0.0));
    }

    /// 切线增量进扁平表的位置：顶点 2 的切线段落在 `2 * per_vertex + 6`。
    #[test]
    fn flatten_places_tangent_delta_after_normal() {
        let mut target = MorphTarget::new("with_tan", vec![[0.0; 3]; 3]).with_tangents(vec![
            [0.0; 3],
            [0.0; 3],
            [0.4, -0.2, 0.9],
        ]);
        target.normals = vec![[0.0; 3]; 3];
        let set = MorphSet::new(vec![target]);
        let (data, stride) = set.flatten();
        let base = 2 * MORPH_VERTEX_FLOATS + 6;
        assert_eq!(&data[base..base + 3], &[0.4, -0.2, 0.9]);
        assert_eq!(stride, 3 * MORPH_VERTEX_FLOATS);
    }

    /// CPU 求解也要把切线增量乘权重累加进去，且不改动手性（第 4 分量）。
    #[test]
    fn apply_accumulates_tangent_like_position() {
        let mesh = triangle();
        let target = MorphTarget::new("tan", vec![[0.0; 3]; 3]).with_tangents(vec![
            [0.0; 3],
            [2.0, 0.0, 0.0],
            [0.0; 3],
        ]);
        let set = MorphSet::new(vec![target]);
        let out = set.apply(&mesh, &[0.5, 0.0, 0.0, 0.0]);
        let before = mesh.vertices[1].tangent;
        let after = out.vertices[1].tangent;
        assert!(
            (after[0] - (before[0] + 1.0)).abs() < 1e-6,
            "切线 x 应加 2.0 × 0.5：{before:?} -> {after:?}"
        );
        assert_eq!(after[1..3], before[1..3], "无切线增量的分量不动");
        assert_eq!(
            out.vertices[1].tangent[3], mesh.vertices[1].tangent[3],
            "手性不参与形变"
        );
        assert_eq!(
            out.vertices[0].tangent, mesh.vertices[0].tangent,
            "权重不影响零增量顶点"
        );
    }

    /// 切线通道长度不对同样要拒绝：GPU 上按 stride 硬读，短了会越界。
    #[test]
    fn validate_checks_tangent_length() {
        let target = MorphTarget::new("t", vec![[0.0; 3]; 3]).with_tangents(vec![[0.0; 3]; 2]);
        match MorphSet::new(vec![target]).validate_against(3) {
            Err(MorphIssue::TangentCountMismatch {
                target,
                expected,
                got,
            }) => assert_eq!((target, expected, got), (0, 3, 2)),
            other => panic!("应报切线数不一致，实际 {other:?}"),
        }
    }

    /// `flatten_limited` 只保留前 N 个形状，stride 仍是「单形状浮点数」。
    #[test]
    fn flatten_limited_truncates_targets() {
        let set = MorphSet::new(vec![
            smile(),
            MorphTarget::new("b", vec![[0.0; 3]; 3]),
            MorphTarget::new("c", vec![[0.0; 3]; 3]),
        ]);
        let per_shape = 3 * MORPH_VERTEX_FLOATS;
        assert_eq!(
            set.flatten_limited(MAX_MORPH_TARGETS).0.len(),
            3 * per_shape
        );
        let (data, stride) = set.flatten_limited(2);
        assert_eq!(stride, per_shape, "stride 是单形状的浮点数，不随 limit 变");
        assert_eq!(data.len(), 2 * per_shape);
        // 前两形状的内容与整体扁平化的前缀完全一致
        assert_eq!(data, set.flatten().0[..2 * per_shape]);
    }

    /// 场景绑定：越界 index 与非有限权重都被拒绝，`reset` 清零。
    #[test]
    fn morph_instance_rejects_bad_input() {
        let mut instance = MorphInstance::new(7, 2);
        assert!(instance.set_weight(0, 0.5));
        assert!(instance.set_weight(1, 1.0));
        assert!(!instance.set_weight(2, 1.0), "超出 target_count");
        assert!(!instance.set_weight(0, f32::NAN), "非有限权重");
        assert_eq!(instance.weight_of(0), Some(0.5));
        assert_eq!(instance.weight_of(2), None);
        assert!(instance.is_active());
        instance.reset();
        assert!(!instance.is_active());
        assert_eq!(instance.weights, [0.0; MAX_MORPH_TARGETS]);
    }

    /// `MAX_MORPH_TARGETS` 之外的多余形状不上传、不参与求解。
    #[test]
    fn apply_ignores_targets_beyond_limit() {
        let mesh = triangle();
        let mut targets: Vec<MorphTarget> = (0..MAX_MORPH_TARGETS)
            .map(|i| MorphTarget::new(format!("t{i}"), vec![[0.0; 3]; 3]))
            .collect();
        targets.push(MorphTarget::new("extra", vec![[9.0, 9.0, 9.0]; 3]));
        let set = MorphSet::new(targets);
        let out = set.apply(&mesh, &[0.0; MAX_MORPH_TARGETS]);
        assert_eq!(out.vertices[0].position, mesh.vertices[0].position);
    }
}
