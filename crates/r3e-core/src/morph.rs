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

/// 每实例携带的 `vec4` 权重组数（扩展三十九）。
///
/// 权重按 `vec4` 一组进顶点属性：加一组，实例记录涨 16 字节，**可同时生效的形状数
/// 就多 4 个**。与「加宽单个 `vec4`」（WGSL 里不存在这种类型）或「同一帧内分批切
/// 权重」（帧内做不到）相比，这是唯一既线性又不损帧内表达能力的扩展方式。
///
/// 取 3 组的原因：表情（`mouthOpen` / `blinkL` / `blinkR` / 微笑）加口型（8 个视素）
/// 的实际用量会到 10~12 个，8 个槽位装不下时只能靠「换网格」绕开；12 个槽位
/// 一次装下，付的仍是每实例 16 字节。再加组（16 形状）的容量收益已低于表与属性开销，
/// 见 ARCHITECTURE §4.20 的容量表。
pub const MORPH_WEIGHT_GROUPS: usize = 3;

/// 单个实例可同时启用的形状数上限 = [`MORPH_WEIGHT_GROUPS`] 组 × 每组 4 个。
///
/// 扩展三十九把 4 抬成 8、扩展四十一再抬成 12，**形状数随组数线性扩展**：每加一组
/// 只多付每实例 16 字节顶点属性，着色器多 4 次乘加。着色器按下标 `g * 4 + 分量`
/// 取权重，所以这个数必须是 4 的倍数 —— 下面那条 `assert!` 把约束钉在编译期。
pub const MAX_MORPH_TARGETS: usize = MORPH_WEIGHT_GROUPS * 4;

const _: () = assert!(
    MAX_MORPH_TARGETS == MORPH_WEIGHT_GROUPS * 4,
    "每组 vec4 装 4 个形状，形状上限必须是组数的 4 倍"
);

/// 「不属于任何互斥组」的哨兵值（扩展三十八）。
///
/// `MorphInstance::exclusive` 里每个形状一格：等于该哨兵表示它独立叠加（与扩展
/// 三十七前的行为一致），否则**同号即互斥**。取 `u8::MAX` 而不是 0 是为了让
/// 「全 0 初始化的缓冲」不会把八个形状意外并进同一个组。
pub const NO_EXCLUSIVE_GROUP: u8 = u8::MAX;

/// 一个互斥组最多容纳的形状数 = 每实例的形状上限。
pub const MAX_EXCLUSIVE_GROUP_MEMBERS: usize = MAX_MORPH_TARGETS;

/// 「全部形状启用」的掩码（扩展四十）。
///
/// 第 `i` 位为 1 表示形状 `i` 在本实例启用；默认就是这个值，所以不碰掩码时
/// 行为与上一扩展逐位相同。形状上限 12（扩展四十一），掩码只用到低 12 位（0xFFF）。
pub const ALL_SHAPE_MASK: u32 = if MAX_MORPH_TARGETS >= 32 {
    u32::MAX
} else {
    (1u32 << MAX_MORPH_TARGETS) - 1
};

/// 把掩码裁剪到「这个网格真有这么多形状」：`count` 位以上一律清零（扩展四十）。
///
/// 渲染层与单测共用同一份裁剪口径 —— 场景侧绑定的 `target_count` 与网格登记的
/// 形状数可能不等（网格形状少于绑定数），直接用未裁剪的掩码会让「启用第 5 个
/// 形状」在只有 3 个形状的网格上仍然亮着，形变表却读不到那段数据。
///
/// 同时夹到 [`MAX_MORPH_TARGETS`]：调用方传进来的可能是网格登记数（不受
/// `target_count` 约束），不夹的话返回的掩码会比形状上限还宽 —— 自己写着
/// 「生效 12 个」，着色器却只有 12 个槽位可查。
pub const fn shape_mask_bits(count: usize) -> u32 {
    let count = if count >= MAX_MORPH_TARGETS {
        MAX_MORPH_TARGETS
    } else {
        count
    };
    if count >= 32 {
        u32::MAX
    } else {
        (1u32 << count) - 1
    }
}

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
    /// 扩展三十八：每个形状所属的互斥组编号（[`NO_EXCLUSIVE_GROUP`] = 独立）。
    ///
    /// 同号的形状在一组里「抢同一块形变预算」：一组内**正值之和超过 1** 时按比例
    /// 压回 1（见 [`MorphInstance::normalized_weights`]）。典型用法是把
    /// 「张嘴 / 闭嘴」「睁眼 / 闭眼」这类语义相反的形状放进同一组 —— 否则两个 1.0
    /// 叠起来会把顶点推过位移两倍处，脸会当场翻面。
    ///
    /// 组只存在于**实例**侧：同一份形状集，敌人和主角可以配不同的互斥关系。
    pub exclusive: [u8; MAX_MORPH_TARGETS],
    /// 扩展四十：逐实例形状掩码 —— 第 `i` 位为 1 表示形状 `i` 在本实例启用。
    ///
    /// `target_count` 是**网格级**的（这份网格有几个形状），掩码是**实例级**的
    /// （这个实例用哪几个）。同一份「人脸」网格上，主角可能只用「笑 / 眨眼」，
    /// 路人群演只用「说话」——扩三十九之前只能把不用的权重全置 0，顶点照样
    /// 进 `morph_offset` 走满 8 个分支；有掩码后渲染层能在**实例级**把形状集
    /// 直接收缩到 0 个，那个实例的 `morph_count` 写 0，顶点路径与不带形变的
    /// 网格逐位相同。
    ///
    /// 越界位在读写时按 `target_count` 与网格登记数裁剪（见 [`shape_mask_bits`]），
    /// 默认 [`ALL_SHAPE_MASK`] 等价于扩展三十九的行为。
    pub mask: u32,
}

impl MorphInstance {
    pub fn new(mesh_node: NodeId, target_count: usize) -> Self {
        Self {
            mesh_node,
            weights: [0.0; MAX_MORPH_TARGETS],
            target_count: target_count.min(MAX_MORPH_TARGETS),
            // 默认全部独立：不调用 `set_exclusive_group` 时行为与扩展三十七逐位相同
            exclusive: [NO_EXCLUSIVE_GROUP; MAX_MORPH_TARGETS],
            // 默认全部启用：不碰掩码时行为与扩展三十九逐位相同
            mask: ALL_SHAPE_MASK,
        }
    }

    /// 本实例**生效**的形状掩码：已按 `target_count` 裁剪（扩展四十）。
    ///
    /// 越界位不会报错，只是不生效 —— 场景侧先绑 `target_count`、脚本后写掩码，
    /// 顺序反过来时也不该让整个实例静默失效。
    pub fn shape_mask(&self) -> u32 {
        self.mask & shape_mask_bits(self.target_count.min(MAX_MORPH_TARGETS))
    }

    /// 整块替换掩码，返回裁剪后的生效值（扩展四十）。越界位被静默丢掉。
    pub fn set_shape_mask(&mut self, mask: u32) -> u32 {
        self.mask = mask;
        self.shape_mask()
    }

    /// 开 / 关单个形状。越界下标拒绝并返回 `false`（不改动任何状态）。
    pub fn set_shape_enabled(&mut self, index: usize, enabled: bool) -> bool {
        if index >= self.target_count.min(MAX_MORPH_TARGETS) {
            return false;
        }
        let bit = 1u32 << index;
        if enabled {
            self.mask |= bit;
        } else {
            self.mask &= !bit;
        }
        true
    }

    /// 某个形状在本实例是否启用（越界返回 `None`）。
    pub fn shape_enabled(&self, index: usize) -> Option<bool> {
        if index >= self.target_count.min(MAX_MORPH_TARGETS) {
            return None;
        }
        Some(self.shape_mask() & (1u32 << index) != 0)
    }

    /// 全部形状重新启用（掩码回到 [`ALL_SHAPE_MASK`]，权重不动）。
    pub fn enable_all_shapes(&mut self) {
        self.mask = ALL_SHAPE_MASK;
    }

    /// 全部形状停用（掩码归 0）：渲染层会把该实例的形变整段摘掉。
    pub fn clear_shape_mask(&mut self) {
        self.mask = 0;
    }

    /// 生效掩码里启用了几个形状（`popcount`）。
    pub fn enabled_shape_count(&self) -> u32 {
        self.shape_mask().count_ones()
    }

    /// 被掩码挡在外面、但权重仍非零的形状数（扩展四十的探针读数）。
    ///
    /// 这个数就是「本实例因为掩码省下的形变乘加组数」—— 权重非零却用不到的
    /// 形状，扩三十九之前要进满分支，现在连槽都不占。
    pub fn masked_nonzero_count(&self) -> u32 {
        let on = self.shape_mask();
        let count = self.target_count.min(MAX_MORPH_TARGETS);
        let mut masked = 0;
        for (index, weight) in self.weights.iter().enumerate().take(count) {
            if on & (1u32 << index) == 0 && *weight != 0.0 && weight.is_finite() {
                masked += 1;
            }
        }
        masked
    }

    /// 把一个形状归入某个互斥组（`group = None` 表示退出组、回到独立叠加）。
    ///
    /// 组号 0..=[`NO_EXCLUSIVE_GROUP`]-1 可用 —— 哨兵值本身不能当组号，
    /// 否则「独立」与「0 号组」就分不开了。越界下标 / 非法组号一律拒绝并
    /// 返回 `false`（不改动任何状态）。
    pub fn set_exclusive_group(&mut self, index: usize, group: Option<u8>) -> bool {
        if index >= self.target_count.min(MAX_MORPH_TARGETS) {
            return false;
        }
        match group {
            None => self.exclusive[index] = NO_EXCLUSIVE_GROUP,
            Some(group) if group != NO_EXCLUSIVE_GROUP => self.exclusive[index] = group,
            Some(_) => return false,
        }
        true
    }

    /// 某个形状当前所属的互斥组（独立时返回 `None`）。
    pub fn exclusive_group_of(&self, index: usize) -> Option<u8> {
        if index >= self.target_count.min(MAX_MORPH_TARGETS) {
            return None;
        }
        let group = self.exclusive[index];
        (group != NO_EXCLUSIVE_GROUP).then_some(group)
    }

    /// 清空全部互斥关系（回到「四个形状各自独立」的默认状态）。权重不动。
    pub fn clear_exclusive_groups(&mut self) {
        self.exclusive = [NO_EXCLUSIVE_GROUP; MAX_MORPH_TARGETS];
    }

    /// 该形状集里目前有多少个**互斥组**（`> 0` 说明归一化有事可做）。
    pub fn exclusive_group_count(&self) -> usize {
        let count = self.target_count.min(MAX_MORPH_TARGETS);
        let mut seen: Vec<u8> = Vec::new();
        for index in 0..count {
            let group = self.exclusive[index];
            if group != NO_EXCLUSIVE_GROUP && !seen.contains(&group) {
                seen.push(group);
            }
        }
        seen.len()
    }

    /// 扩展三十八：把权重按互斥组归一化后的副本，仍带一点「是否真的压过」的读数。
    ///
    /// 规则（逐组独立执行，组之间互不影响）：
    ///
    /// * 组内**正值之和 > 1** 时，把组内正值等比缩放到和恰好为 1；
    /// * 和 ≤ 1 时原样返回（「半张嘴」这类部分权重不该被抬到满值）；
    /// * 负权重不参与求和、也不被缩放 —— 形变允许反向（`w < 0`），把它算进预算
    ///   会让「闭眼 -0.5 + 睁眼 +1.0」互相抵消掉本该生效的位移；
    /// * 独立形状（[`NO_EXCLUSIVE_GROUP`]）与 `target_count` 之外的槽位原样保留；
    /// * 非有限权重原样保留（由调用方按「非法 → 当 0 处理」的既有口径收口，
    ///   这一层不替它做决定，否则 NaN 会被偷偷改成 0 而掩盖脚本错误）。
    ///
    /// 扩展四十：**掩码外的形状先清零再归一化** —— 掩码是「这个实例不看这个形状」，
    /// 它既不吃组预算、也不进实例缓冲。返回值就是渲染层要的那一份（剔除余量按它
    /// 求和，屏幕上看到的位移也按它算），所以两者不会分叉。
    ///
    /// 返回值第二项是「本次真的缩放过」：渲染层用它做零变化验收 —— 没有任何互斥组
    /// 超预算时，进实例缓冲的权重与场景里的原值逐位相同。
    ///
    /// 这是**纯函数**：`&self` 进、新数组出，场景侧权重不被改动，脚本读回的值仍是
    /// 自己写进去的那个（否则「写 1 读回 0.5」会让所有调参脚本当场懵）。
    pub fn normalized_weights(&self) -> ([f32; MAX_MORPH_TARGETS], bool) {
        let mut weights = self.weights;
        let count = self.target_count.min(MAX_MORPH_TARGETS);
        // 扩展四十：掩码外的槽位先归零，后面的互斥求和自然忽略它们
        let on = self.shape_mask();
        let mut handled = [false; MAX_MORPH_TARGETS];
        let mut scaled = false;
        for (index, weight) in weights.iter_mut().enumerate() {
            if on & (1u32 << index) == 0 {
                *weight = 0.0;
            }
        }
        for index in 0..count {
            let group = self.exclusive[index];
            if group == NO_EXCLUSIVE_GROUP || handled[index] {
                continue;
            }
            // 先把同组成员挑出来：`count ≤ 4`，线性扫比建表便宜
            let mut members = [0usize; MAX_EXCLUSIVE_GROUP_MEMBERS];
            let mut members_len = 0usize;
            let mut sum = 0.0f32;
            for (other, slot) in self.exclusive.iter().enumerate().take(count).skip(index) {
                if *slot != group {
                    continue;
                }
                members[members_len] = other;
                members_len += 1;
                handled[other] = true;
                let weight = weights[other];
                if weight.is_finite() && weight > 0.0 {
                    sum += weight;
                }
            }
            // 只有超预算才动：否则「其余形状各 0.3、和 0.9」会被无端抬到 1.0
            if sum > 1.0 {
                let inverse = 1.0 / sum;
                for member in members.iter().take(members_len) {
                    let weight = weights[*member];
                    if weight.is_finite() && weight > 0.0 {
                        weights[*member] = weight * inverse;
                    }
                }
                scaled = true;
            }
        }
        (weights, scaled)
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
    ///
    /// 扩展四十：被掩码挡住的形状不算 —— 「权重非零但形状停用」的实例在渲染层
    /// 是与「没有表情」同一个路径，这里也要给出同一个答案。
    pub fn is_active(&self) -> bool {
        let on = self.shape_mask();
        (0..self.target_count.min(MAX_MORPH_TARGETS)).any(|index| {
            on & (1u32 << index) != 0 && {
                let weight = self.weights[index];
                weight != 0.0 && weight.is_finite()
            }
        })
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

    /// 把「第 1 组的 4 个权重」铺成立长 `MAX_MORPH_TARGETS` 的数组。
    ///
    /// 扩展三十九之后权重是 8 格，用例里写全 8 个字面量既吵、又会在再加组时集体失效，
    /// 所以统一走这个 helper（第 2 组及以后默认 0）。
    fn weights4(first_group: [f32; 4]) -> [f32; MAX_MORPH_TARGETS] {
        let mut out = [0.0; MAX_MORPH_TARGETS];
        out[..4].copy_from_slice(&first_group);
        out
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

        let full = set.apply(&mesh, &weights4([1.0, 0.0, 0.0, 0.0]));
        assert_eq!(full.vertices[1].position, [2.0, 1.0, 0.0]);
        assert_eq!(full.vertices[2].position, [0.0, 1.0, 0.0]);

        let half = set.apply(&mesh, &weights4([0.5, 0.0, 0.0, 0.0]));
        assert_eq!(half.vertices[1].position, [1.5, 0.5, 0.0]);
    }

    /// 两个形状按各自权重叠加，且顺序无关（线性）。
    #[test]
    fn apply_sums_targets_linearly() {
        let mesh = triangle();
        let shift = MorphTarget::new("shift", vec![[0.0; 3], [0.0; 3], [0.0, 0.0, 2.0]]);
        let set = MorphSet::new(vec![smile(), shift]);
        let a = set.apply(&mesh, &weights4([1.0, 1.0, 0.0, 0.0]));
        let b = set.apply(&mesh, &weights4([1.0, 0.5, 0.0, 0.0]));
        assert_eq!(a.vertices[1].position, [2.0, 1.0, 0.0]);
        assert_eq!(a.vertices[2].position, [0.0, 1.0, 2.0]);
        assert_eq!(b.vertices[2].position, [0.0, 1.0, 1.0]);
    }

    /// 非有限权重按 0 处理，不会把网格拖成 NaN。
    #[test]
    fn non_finite_weight_is_ignored() {
        let mesh = triangle();
        let set = MorphSet::new(vec![smile()]);
        let out = set.apply(&mesh, &weights4([f32::INFINITY, 0.0, 0.0, 0.0]));
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
        let out = set.apply(&mesh, &weights4([0.5, 0.0, 0.0, 0.0]));
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

    /// 扩展三十八（契约一）：没设组时一切照旧 —— 归一化是纯拷贝、不缩放。
    ///
    /// 这条是「零变化验收」的 CPU 侧锚点：老场景不调 `set_exclusive_group`，
    /// 进实例缓冲的权重就必须与场景里的原值逐位相同。
    #[test]
    fn default_instance_has_no_exclusive_group() {
        let mut instance = MorphInstance::new(7, 3);
        assert_eq!(instance.exclusive, [NO_EXCLUSIVE_GROUP; MAX_MORPH_TARGETS]);
        assert_eq!(instance.exclusive_group_count(), 0);
        assert_eq!(instance.exclusive_group_of(0), None);
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 1.0);
        let (weights, scaled) = instance.normalized_weights();
        assert!(!scaled, "没有组就不该缩放过");
        assert_eq!(weights, instance.weights, "权重必须逐位相同");
    }

    /// 扩展三十八（契约二）：同组正值之和超 1 时按比例压回 1，组外形状不受影响。
    #[test]
    fn exclusive_group_caps_sum_to_one() {
        let mut instance = MorphInstance::new(7, 3);
        assert!(instance.set_exclusive_group(0, Some(0)));
        assert!(instance.set_exclusive_group(1, Some(0)));
        assert_eq!(instance.exclusive_group_count(), 1, "0 与 1 是同一组");
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 1.0);
        instance.set_weight(2, 1.0);
        let (weights, scaled) = instance.normalized_weights();
        assert!(scaled);
        assert!((weights[0] - 0.5).abs() < 1e-6, "组内各占一半: {weights:?}");
        assert!((weights[1] - 0.5).abs() < 1e-6, "{weights:?}");
        assert_eq!(weights[2], 1.0, "组外形状不该被牵连");
        // 组内和必须恰好是 1：这是「两个表情互相顶」的数学表述
        assert!((weights[0] + weights[1] - 1.0).abs() < 1e-6);
    }

    /// 扩展三十八（契约三）：组内和 ≤ 1 时原样保留 —— 部分权重不该被抬到满值。
    #[test]
    fn exclusive_group_leaves_partial_weights_alone() {
        let mut instance = MorphInstance::new(7, 2);
        assert!(instance.set_exclusive_group(0, Some(4)));
        assert!(instance.set_exclusive_group(1, Some(4)));
        instance.set_weight(0, 0.3);
        instance.set_weight(1, 0.3);
        let (weights, scaled) = instance.normalized_weights();
        assert!(!scaled, "0.6 ≤ 1，不该缩放");
        // 只断言前两格：组数扩了也不该改这条用例（扩展三十九）
        assert_eq!(&weights[..2], &[0.3, 0.3]);
        assert!(weights[2..].iter().all(|w| *w == 0.0), "其余槽位应保持 0");
    }

    /// 扩展三十八（契约四）：负权重不占预算、也不被缩放。
    ///
    /// 「闭眼 -0.5 + 睁眼 +1.0」是同组的常见写法：若把 -0.5 算进和，本该生效的
    /// +1.0 会被压成 +0.67，正向位移白丢三分之一。
    #[test]
    fn negative_weight_stays_out_of_budget() {
        let mut instance = MorphInstance::new(7, 2);
        assert!(instance.set_exclusive_group(0, Some(0)));
        assert!(instance.set_exclusive_group(1, Some(0)));
        instance.set_weight(0, 1.0);
        instance.set_weight(1, -0.5);
        let (weights, scaled) = instance.normalized_weights();
        assert!(!scaled, "正值和 1.0 不超预算，不该缩放");
        assert_eq!(weights[0], 1.0);
        assert_eq!(weights[1], -0.5, "反向系数必须原样保留");

        // 超预算时负值依然不动（只缩正值），和恒等于 1
        instance.set_weight(0, 3.0);
        let (weights, scaled) = instance.normalized_weights();
        assert!(scaled);
        assert!((weights[0] - 1.0).abs() < 1e-6);
        assert_eq!(weights[1], -0.5);
    }

    /// 扩展三十八（契约五）：`normalized_weights` 是纯函数，不改场景侧权重。
    #[test]
    fn normalized_weights_is_pure() {
        let mut instance = MorphInstance::new(7, 2);
        assert!(instance.set_exclusive_group(0, Some(1)));
        assert!(instance.set_exclusive_group(1, Some(1)));
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 1.0);
        let (weights, _) = instance.normalized_weights();
        assert_eq!(weights[0], 0.5);
        assert_eq!(weights[1], 0.5);
        assert_eq!(
            &instance.weights[..2],
            &[1.0, 1.0],
            "脚本读回的权重必须还是自己写进去的那个值"
        );
    }

    /// 扩展三十八（契约六）：组号校验 —— 哨兵值不能当组号，越界下标被拒。
    #[test]
    fn exclusive_group_rejects_sentinel_and_bad_index() {
        let mut instance = MorphInstance::new(7, 2);
        assert!(
            !instance.set_exclusive_group(0, Some(NO_EXCLUSIVE_GROUP)),
            "哨兵值是「独立」的编码，不能再当成 0xFF 号组"
        );
        assert!(
            !instance.set_exclusive_group(2, Some(0)),
            "超出 target_count"
        );
        assert!(instance.set_exclusive_group(0, Some(3)));
        assert_eq!(instance.exclusive_group_of(0), Some(3));
        assert!(instance.set_exclusive_group(0, None), "退组");
        assert_eq!(instance.exclusive_group_of(0), None);
        assert_eq!(instance.exclusive_group_of(1), None);
    }

    /// 扩展三十八（契约七）：归一化后的权重喂给 CPU 求解，位移正好折半。
    ///
    /// 这条把「互斥」从权重算术接到真实几何上：两个同组形状权重 1.0 + 1.0
    /// 若不归一化，顶点会被推到位移的两倍处；归一化后应等于只开一个。
    #[test]
    fn normalized_weights_halve_displacement_in_apply() {
        let mesh = triangle();
        // 两个同向形状：各自把顶点 1 沿 +x 推 1
        let set = MorphSet::new(vec![
            MorphTarget::new("a", vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0; 3]]),
            MorphTarget::new("b", vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0; 3]]),
        ]);
        let mut instance = MorphInstance::new(0, 2);
        assert!(instance.set_exclusive_group(0, Some(0)));
        assert!(instance.set_exclusive_group(1, Some(0)));
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 1.0);

        // 顶点 1 起点 x = 1.0：两个形状各推 1 → 3.0；归一化后只推 1 → 2.0
        let raw = set.apply(&mesh, &instance.weights);
        assert!(
            (raw.vertices[1].position[0] - 3.0).abs() < 1e-6,
            "未归一化时应叠加到两倍位移: {:?}",
            raw.vertices[1].position
        );
        let (weights, _) = instance.normalized_weights();
        let capped = set.apply(&mesh, &weights);
        assert!(
            (capped.vertices[1].position[0] - 2.0).abs() < 1e-6,
            "归一化后应等于只开一个形状: {:?}",
            capped.vertices[1].position
        );
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

    // ---- 扩展三十九：多组 vec4 权重（形状数随组数线性扩展） ----

    /// 扩展三十九（契约一）：形状上限 = 组数 × 4，且必须是 4 的倍数。
    ///
    /// 着色器按下标 `g * 4 + 分量` 取权重，组数一旦不是「每组 4 个」就会整体串位。
    #[test]
    fn target_limit_is_groups_times_four() {
        assert_eq!(MORPH_WEIGHT_GROUPS, 3, "扩展四十一取 3 组 = 12 个形状");
        assert_eq!(MAX_MORPH_TARGETS, MORPH_WEIGHT_GROUPS * 4);
        assert_eq!(MAX_MORPH_TARGETS % 4, 0, "每组固定 4 个分量");
        assert_eq!(MAX_EXCLUSIVE_GROUP_MEMBERS, MAX_MORPH_TARGETS);
    }

    /// 扩展三十九（契约二）：第 2 组（下标 ≥ 4）的形状与第 1 组一样参与求解。
    ///
    /// 扩组之前这些下标会被 `MAX_MORPH_TARGETS` 挡在门外；现在必须真的推动顶点，
    /// 否则「形状数 > 4」就只是文档里的一句话。
    #[test]
    fn weights_beyond_first_group_deform_vertices() {
        let mesh = triangle();
        let last = MAX_MORPH_TARGETS - 1;
        let mut targets: Vec<MorphTarget> = (0..MAX_MORPH_TARGETS)
            .map(|_| MorphTarget::new("zero", vec![[0.0; 3]; 3]))
            .collect();
        // 最后一格（第 2 组的 w 分量）把顶点 1 沿 +y 推 2
        targets[last] = MorphTarget::new("last", vec![[0.0; 3], [0.0, 2.0, 0.0], [0.0; 3]]);
        let set = MorphSet::new(targets);

        let mut weights = [0.0f32; MAX_MORPH_TARGETS];
        weights[last] = 1.0;
        let out = set.apply(&mesh, &weights);
        assert!(
            (out.vertices[1].position[1] - 2.0).abs() < 1e-6,
            "第 2 组最后一格必须生效，实际 {:?}",
            out.vertices[1].position
        );
        assert_eq!(out.vertices[0].position, [0.0, 0.0, 0.0], "零增量顶点不动");

        // 半权重就是半位移：组的位置不影响线性叠加
        weights[last] = 0.5;
        let half = set.apply(&mesh, &weights);
        assert!((half.vertices[1].position[1] - 1.0).abs() < 1e-6);
    }

    /// 扩展三十九（契约三）：互斥组能横跨权重组 —— 组号是场景概念，与 `vec4` 无关。
    ///
    /// 把「微笑（第 1 组）」和「撇嘴（第 2 组）」放进同一组后，两者各 1.0 仍要
    /// 折半，否则「加组」会顺手拆掉扩展三十八的归一化。
    #[test]
    fn exclusive_group_spans_weight_groups() {
        let last = MAX_MORPH_TARGETS - 1;
        let mut instance = MorphInstance::new(0, MAX_MORPH_TARGETS);
        assert!(instance.set_exclusive_group(0, Some(7)));
        assert!(instance.set_exclusive_group(last, Some(7)));
        assert_eq!(instance.exclusive_group_count(), 1, "跨组也是同一组");
        instance.set_weight(0, 1.0);
        instance.set_weight(last, 1.0);
        let (weights, scaled) = instance.normalized_weights();
        assert!(scaled);
        assert!((weights[0] - 0.5).abs() < 1e-6, "{weights:?}");
        assert!((weights[last] - 0.5).abs() < 1e-6, "{weights:?}");
    }

    /// 扩展三十九（契约四）：扁平化整表覆盖全部权重槽位 —— 第 8 个形状落在表尾。
    ///
    /// 表尾下标一旦算错，`morph_table` 读到的是下一顶点或越界（WGSL 不报错，
    /// 只会画花），所以这里按偏移逐项钉住。
    #[test]
    fn flatten_covers_all_weight_slots() {
        let per_shape = 3 * MORPH_VERTEX_FLOATS;
        let targets: Vec<MorphTarget> = (0..MAX_MORPH_TARGETS)
            .map(|i| {
                // 每形状唯一值，方便按偏移验证顺序
                MorphTarget::new(format!("t{i}"), vec![[i as f32; 3]; 3])
            })
            .collect();
        let set = MorphSet::new(targets);
        let (flat, stride) = set.flatten_limited(MAX_MORPH_TARGETS);
        assert_eq!(stride, per_shape, "stride 仍是单形状的浮点数");
        assert_eq!(flat.len(), MAX_MORPH_TARGETS * per_shape);
        // 最后一个形状的第一顶点、位置 x 分量
        let tail = (MAX_MORPH_TARGETS - 1) * per_shape;
        assert_eq!(flat[tail], (MAX_MORPH_TARGETS - 1) as f32);
        // 形状 4（第 2 组第 1 个）同样在表内，不是补零
        assert_eq!(flat[4 * per_shape], 4.0);
    }

    /// 扩展三十九（契约五）：`MorphInstance` 能登记到上限，且越界一位立刻被拒。
    #[test]
    fn instance_accepts_full_weight_slots() {
        let mut instance = MorphInstance::new(0, MAX_MORPH_TARGETS + 5);
        assert_eq!(instance.target_count, MAX_MORPH_TARGETS, "登记数夹到上限");
        let last = MAX_MORPH_TARGETS - 1;
        assert!(instance.set_weight(last, 0.75), "最后一格可写");
        assert_eq!(instance.weight_of(last), Some(0.75));
        assert!(!instance.set_weight(MAX_MORPH_TARGETS, 1.0), "越界一格");
        assert_eq!(instance.weight_of(MAX_MORPH_TARGETS), None);
    }

    /// 扩展四十（契约一）：默认掩码 = 全部启用，与扩展三十九逐位兼容。
    ///
    /// 不碰掩码的场景必须完全不受影响 —— 这是「加能力不改画面」的底线。
    #[test]
    fn mask_default_keeps_every_shape_enabled() {
        let instance = MorphInstance::new(0, 3);
        assert_eq!(instance.mask, ALL_SHAPE_MASK);
        assert_eq!(instance.enabled_shape_count(), 3, "只数到 target_count");
        assert_eq!(instance.shape_mask(), 0b111, "越界位被裁掉");
        assert_eq!(instance.masked_nonzero_count(), 0);
        for index in 0..3 {
            assert_eq!(instance.shape_enabled(index), Some(true));
        }
        assert_eq!(instance.shape_enabled(3), None, "越界不启用也不报错");
    }

    /// 扩展四十（契约二）：占比裁到位 —— 掩码写成 u32::MAX 也只生效前 `count` 位。
    ///
    /// `target_count` 与网格登记数可能不等（场景绑定数 > 网格形状数）。
    /// 不裁的话「启用第 5 个形状」在只有 3 个形状的网格上仍然亮着，
    /// 形变表却读不到那段数据 —— 静默读到下一个顶点的位移。
    #[test]
    fn mask_bits_are_clipped_to_target_count() {
        assert_eq!(shape_mask_bits(0), 0);
        assert_eq!(shape_mask_bits(3), 0b111);
        assert_eq!(shape_mask_bits(MAX_MORPH_TARGETS), ALL_SHAPE_MASK);
        assert_eq!(
            shape_mask_bits(MAX_MORPH_TARGETS + 5),
            ALL_SHAPE_MASK,
            "超过上限也不越界"
        );

        let mut instance = MorphInstance::new(0, 3);
        assert_eq!(instance.set_shape_mask(u32::MAX), 0b111);
        assert_eq!(instance.enabled_shape_count(), 3);
        assert!(!instance.set_shape_enabled(5, true), "越界下标直接拒");
    }

    /// 扩展四十（契约三）：掩码外的权重在归一化结果里先归零，且不参与互斥预算。
    ///
    /// 这是「掩码」与「权重置 0」在语义上的分界：置 0 是用户自己抹掉，
    /// 掩码是渲染层抹掉 —— 后者不该让组内另一形状被无端抬到满值。
    #[test]
    fn mask_zeroes_weights_before_exclusive_budget() {
        let mut instance = MorphInstance::new(0, 4);
        // 形状 0、1 同组：各 1.0 时本应折半
        assert!(instance.set_exclusive_group(0, Some(0)));
        assert!(instance.set_exclusive_group(1, Some(0)));
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 1.0);
        let (weights, scaled) = instance.normalized_weights();
        assert!(scaled);
        assert!((weights[0] - 0.5).abs() < 1e-6);

        // 关掉形状 1 后，组里只剩形状 0 独自 1.0 —— 不超预算，原样保留
        assert!(instance.set_shape_enabled(1, false));
        let (weights, scaled) = instance.normalized_weights();
        assert!(!scaled, "被掩码挡住的形状不进预算");
        assert!((weights[0] - 1.0).abs() < 1e-6, "{weights:?}");
        assert_eq!(weights[1], 0.0, "掩码外的槽位归零");
        assert_eq!(instance.weight_of(1), Some(1.0), "场景原值不动");
    }

    /// 扩展四十（契约四）：`is_active` 与 `masked_nonzero_count` 口径一致。
    ///
    /// 两者的答案必须相同 —— 渲染层用前者决定「整段摘掉形变」，
    /// 探针用后者报「省了几个乘加组」，一个说有事一个说没事就会自相矛盾。
    #[test]
    fn masked_shapes_are_inactive_and_counted() {
        let mut instance = MorphInstance::new(0, 4);
        instance.set_weight(0, 1.0);
        instance.set_weight(1, 0.5);
        assert!(instance.is_active());
        assert_eq!(instance.masked_nonzero_count(), 0, "全启用时没有被挡的");

        instance.clear_shape_mask();
        assert!(!instance.is_active(), "形状全关 = 没有形变在起作用");
        assert_eq!(instance.masked_nonzero_count(), 2, "两个非零权重被挡");

        instance.enable_all_shapes();
        assert!(instance.is_active(), "重新打开就恢复");
        assert_eq!(instance.masked_nonzero_count(), 0);
    }

    /// 扩展四十（契约五）：掩码全关时 `normalized_weights` 返回全 0 且不动原值。
    ///
    /// 渲染层据此把 `morph_count` 写 0，顶点路径与不带形变的网格逐位相同。
    #[test]
    fn cleared_mask_yields_zero_weights() {
        let mut instance = MorphInstance::new(0, MAX_MORPH_TARGETS);
        for index in 0..MAX_MORPH_TARGETS {
            instance.set_weight(index, 1.0);
        }
        instance.clear_shape_mask();
        let (weights, scaled) = instance.normalized_weights();
        assert!(!scaled);
        assert!(weights.iter().all(|w| *w == 0.0), "{weights:?}");
        assert!(
            instance.weights.iter().all(|w| *w == 1.0),
            "纯函数：场景原值一份不动"
        );
    }
}
