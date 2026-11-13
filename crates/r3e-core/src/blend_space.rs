//! 1D / 2D / 3D 混合空间（扩展四十五 / 四十九）：把「参数」变成「一组片段权重」的纯函数。
//!
//! 分层混合树（扩展十八 A，`anim_blend`）解决的是「同时播多段动画」，
//! 但层内那几路片段的权重是**手写常量**——走 / 跑 / 冲刺该按速度分配多少，
//! 是靠脚本每帧改 `set_anim_layer_clip_weight` 凑出来的。
//! 成熟 RPG 的移动、转向、瞄准、攀爬、表情都要求「给一个参数，权重自己连续变」，
//! 这一层就是混合空间：
//!
//! ```text
//! 参数（速度 / 朝向 / 俯仰）── 落进样本点围成的几何体 ──> 各片段权重（和为 1）
//! ```
//!
//! # 为什么放在 core
//!
//! 混合空间的核心是一条**坐标 → 权重**的插值公式，不碰 GPU、不碰场景、不碰时间。
//! 放进 `r3e-render` 就只能靠看图判断对错；放在 core 可以用单测把端点精确性、
//! 钳制行为、归一性全部钉住（见本文件尾部 30 项测试）。
//! 与 `ssao` / `occlusion` / `pick` 同一层级：纯 CPU 契约，渲染层与脚本层各自复用。
//!
//! # 四个关键取舍
//!
//! 1. **2D 用三角剖分，不用反距离权重（IDW）或径向基（RBF）**。
//!    IDW 在样本点上并不精确等于 1（只是趋近），且有一个「幂次」参数要调；
//!    RBF 要解线性方程组，n 个样本 O(n³)，还得处理矩阵奇异。
//!    三角剖分（Bowyer–Watson）在三角形内是**线性重心插值**：样本点上权重精确为 1/0，
//!    内部连续、无参数可调、结果确定。代价有两个：样本点外的区域要靠兜底（见第 2 条），
//!    四边形中心这类**恰落在对角线上的点只由对角两个样本支撑**（不是双线性的四向平均）。
//!    后者与 Unity Freeform 2D 的行为一致；需要「四向均匀」时，在中心补一个静止样本即可。
//! 2. **外插一律禁止，样本外钳制到最近边（或最近点）**。
//!    动画参数超出录制范围时，外插等于「猜一个没人摆过的姿势」，几乎总是错的：
//!    速度 3.0 而样本只录到 2.0 时，正确表现是「保持冲刺」，不是继续放大步幅。
//! 3. **1D / 2D / 3D 是同一个类型**，低维只是「高位分量恒为 0 的高维」（1D 是 y=z=0，
//!    2D 是 z=0）。
//!    分成三个类型会让 `AnimLayer` 多一层枚举、脚本多三套 API、编辑器多三种快照，
//!    而低维的求值路径（排序 + 相邻两样本插值 / 三角形重心）本身就比高维简单，
//!    没必要共用体剖分。
//! 4. **3D 用四面体剖分（Bowyer–Watson 的三维版），不做「两层 2D 叠」**。
//!    八向移动 × 三档速度真正需要的是 `(move.x, move.y, speed)` 三个参数一起连续插值：
//!    用两层 2D（低速一张图 + 高速一张图，中间手动插）在速度交界处会出现权重不连续
//!    （两层各自归一化后相加，姿态会在交界跳一下），而且每加一档速度就要多手写一层。
//!    体剖分把这件事一次解决：24 个样本插进同一个 Delaunay 四面体网格，
//!    任意 `(x, y, v)` 都落在某个四面体里做四顶点重心插值，样本点上权重量精确为 1/0。
//!    代价是样本数上限抬到 32、剖分 O(n²) 与 4 个顶点的重心计算（每帧每层可忽略）。
//!
//! # 明确不做
//!
//! - 不做「轴归一化 / 每轴不同量程」：调用方把参数换算成同量纲的坐标再喂进来。
//! - 不做时间维度（那是片段内的时间轴，不是混合空间）。
//! - 不做样本增删以外的编辑（移动样本 = 删 + 加）。
//! - 不做 4D 及以上：RPG 里没有第四个连续参数需要与移动共用一张图。
//! - 3D 不做「样本共面时的降维剖分」：全共面的样本没有体剖分，求值退化为最近样本
//!   （权重 1），`brief()` 里 `tet=0` 一眼可见——要精确插值就给至少四个不共面的样本。

use crate::anim_blend::normalize_weights;
use std::collections::HashMap;

/// 单个混合空间最多容纳的样本数。
///
/// 32 个用来装下「八向移动 × 三档速度」（24 个）+ 待机 + 若干特例（倒退、侧移偏置），
/// 再往上收益就只剩「同一参数网格画得更密」，而体剖分是 O(样本数²) 的：
/// 上限处也只有 496 次四面体重心判定，每帧每层可忽略。
pub const MAX_BLEND_SAMPLES: usize = 32;

/// 坐标去重与「同位置」判定的容差（世界单位，不是像素）。
const EPS: f32 = 1e-6;

/// 超级三角形 / 超级四面体相对包围盒的放大倍数（Bowyer–Watson 要求它包住全部样本点）。
const SUPER_SCALE: f32 = 16.0;

/// 退化三角形（面积过小）的判定阈值。
const AREA_EPS: f32 = 1e-9;

/// 退化四面体（体积过小）的判定阈值：共面的四个样本不构成有效的体单元。
const VOLUME_EPS: f64 = 1e-12;

/// 重心坐标落在三角形外的容差：允许极小负值，避免「样本恰在边上」时被判为在外。
const BARY_TOLERANCE: f32 = 1e-5;

/// 混合维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendDim {
    /// 一维：一个参数（速度 / 转向角 / 生命值）。
    Linear1D,
    /// 二维：两个参数（移动方向 x / y、瞄准俯仰与偏航）。
    Planar2D,
    /// 三维：三个参数（移动方向 x / y + 速度档，扩展四十九）。
    Volumetric3D,
}

impl BlendDim {
    /// 规范名（日志、RPC、脚本共用同一口径）。
    pub const fn as_str(&self) -> &'static str {
        match self {
            BlendDim::Linear1D => "1d",
            BlendDim::Planar2D => "2d",
            BlendDim::Volumetric3D => "3d",
        }
    }

    /// 解析规范名（大小写不敏感，接受几个常见别名）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1d" | "linear" | "linear1d" | "linear_1d" => Some(BlendDim::Linear1D),
            "2d" | "planar" | "planar2d" | "planar_2d" => Some(BlendDim::Planar2D),
            "3d" | "volume" | "volumetric" | "volumetric3d" | "volume_3d" => {
                Some(BlendDim::Volumetric3D)
            }
            _ => None,
        }
    }

    /// 参数个数：1D 一个、2D 两个、3D 三个。
    pub const fn axes(&self) -> usize {
        match self {
            BlendDim::Linear1D => 1,
            BlendDim::Planar2D => 2,
            BlendDim::Volumetric3D => 3,
        }
    }
}

/// 一个样本：某组参数坐标上播放哪段片段。
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSample {
    /// 参数坐标。低维只用前面的分量，其余分量恒为 0（添加时就被归零）：
    /// 1D 只用 `at[0]`、2D 用 `at[0..2]`、3D 三个都用。
    pub at: [f32; 3],
    /// 该坐标上的片段名（与 `AnimClip::name` 同名，大小写不敏感匹配）。
    pub clip: String,
}

/// 1D / 2D / 3D 混合空间。
///
/// 字段全公开：它是**数据**而不是句柄，编辑器快照与存档要能直接读写。
/// 但增删样本请走 `add_sample` / `clear_samples`——它们负责去重与重建剖分，
/// 直接改 `samples` 会让 `triangles` / `tetra` 与样本失配（求值时会跳越界单元，
/// 退回最近边 / 最近面，结果仍然可用，只是不再是精确插值）。
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSpace {
    /// 维度。
    pub dim: BlendDim,
    /// 参数名（1D 只用第一个，2D 用前两个，3D 三个都用；空位留空串）。
    pub params: [String; 3],
    /// 样本表（添加顺序，1D 求值时按坐标排序，不改动本表）。
    pub samples: Vec<BlendSample>,
    /// 当前参数值（求值入口）。
    pub value: [f32; 3],
    /// 2D 的三角剖分结果（1D / 3D 恒为空）。
    triangles: Vec<[u16; 3]>,
    /// 3D 的四面体剖分结果（1D / 2D 恒为空）。
    tetra: Vec<[u16; 4]>,
    /// 3D 剖分的凸包面（只用于「样本外钳制到最近面」，与 `tetra` 同步重建）。
    hull_faces: Vec<[u16; 3]>,
    /// 被拒绝的样本数（非法坐标 / 重复位置），只用于日志，语义是「累计」。
    rejected: usize,
}

impl BlendSpace {
    /// 建一维混合空间（`param` 是参数名，例如 `"speed"`）。
    pub fn new_1d(param: impl Into<String>) -> Self {
        Self {
            dim: BlendDim::Linear1D,
            params: [param.into(), String::new(), String::new()],
            samples: Vec::new(),
            value: [0.0, 0.0, 0.0],
            triangles: Vec::new(),
            tetra: Vec::new(),
            hull_faces: Vec::new(),
            rejected: 0,
        }
    }

    /// 建二维混合空间（`x` / `y` 是两个参数名）。
    pub fn new_2d(x: impl Into<String>, y: impl Into<String>) -> Self {
        Self {
            dim: BlendDim::Planar2D,
            params: [x.into(), y.into(), String::new()],
            samples: Vec::new(),
            value: [0.0, 0.0, 0.0],
            triangles: Vec::new(),
            tetra: Vec::new(),
            hull_faces: Vec::new(),
            rejected: 0,
        }
    }

    /// 建三维混合空间（`x` / `y` / `z` 是三个参数名，扩展四十九）。
    pub fn new_3d(x: impl Into<String>, y: impl Into<String>, z: impl Into<String>) -> Self {
        Self {
            dim: BlendDim::Volumetric3D,
            params: [x.into(), y.into(), z.into()],
            samples: Vec::new(),
            value: [0.0, 0.0, 0.0],
            triangles: Vec::new(),
            tetra: Vec::new(),
            hull_faces: Vec::new(),
            rejected: 0,
        }
    }

    /// 维度。
    pub const fn dim(&self) -> BlendDim {
        self.dim
    }

    /// 参数名（按维度返回 1 / 2 / 3 个）。
    pub fn param_names(&self) -> Vec<String> {
        (0..self.dim.axes())
            .map(|i| self.params[i].clone())
            .collect()
    }

    /// 样本数。
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 是否还没有任何样本（此时求值返回空权重，调用方应回落到手写权重）。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 三角剖分出的三角形个数（1D / 3D 恒为 0）。
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// 体剖分出的四面体个数（1D / 2D 恒为 0；扩展四十九）。
    pub fn tetra_count(&self) -> usize {
        self.tetra.len()
    }

    /// 被拒绝的样本累计数。
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// 剖分结果（只读，编辑器画线框用）。
    pub fn triangles(&self) -> &[[u16; 3]] {
        &self.triangles
    }

    /// 体剖分结果（只读，调试 / 编辑器画四面体用；扩展四十九）。
    pub fn tetra(&self) -> &[[u16; 4]] {
        &self.tetra
    }

    /// 把坐标按维度归一：1D 强制 `y = z = 0`、2D 强制 `z = 0`，
    /// 这样「重复位置判定」在各维度下同一口径。
    fn point_of(&self, at: [f32; 3]) -> [f32; 3] {
        match self.dim {
            BlendDim::Linear1D => [at[0], 0.0, 0.0],
            BlendDim::Planar2D => [at[0], at[1], 0.0],
            BlendDim::Volumetric3D => at,
        }
    }

    /// 加一个样本（1D / 2D 入口：`z` 由维度归一吃掉）。成功返回 `true`。
    ///
    /// 拒绝的三种情况（`false`）：
    /// - 片段名为空（写错了名字，静默接受只会变成「层里多一路播不动的片段」）；
    /// - 坐标含 NaN / ∞（没法参与任何几何判定）；
    /// - 与已有样本同位置（同位置两份权重之和恒为 1，等于给这组参数一个「平均姿态」，
    ///   而调用方以为是「叠加」——这是最难查的一类错，直接拒）。
    ///
    /// 片段名重复**不拒**：同一个动作在参数轴上有两段录制是正常需求，
    /// 求值时按名字合并权重（见 `weights_at`）。
    pub fn add_sample(&mut self, clip: impl Into<String>, at: [f32; 2]) -> bool {
        self.add_sample_3d(clip, [at[0], at[1], 0.0])
    }

    /// 加一个三维样本（扩展四十九）。1D / 2D 空间调用时高位分量被维度归一吃掉。
    pub fn add_sample_3d(&mut self, clip: impl Into<String>, at: [f32; 3]) -> bool {
        let clip = clip.into();
        if clip.trim().is_empty() {
            self.rejected += 1;
            return false;
        }
        if !at[0].is_finite() || !at[1].is_finite() || !at[2].is_finite() {
            self.rejected += 1;
            return false;
        }
        if self.samples.len() >= MAX_BLEND_SAMPLES {
            return false;
        }
        let at = self.point_of(at);
        if self
            .samples
            .iter()
            .any(|s| distance3(s.at, at) <= EPS * EPS)
        {
            self.rejected += 1;
            return false;
        }
        self.samples.push(BlendSample { at, clip });
        self.rebuild();
        true
    }

    /// 记一次「外部判定」的拒绝（如片段名在实例里解析不到），只影响 `drop=` 计数。
    ///
    /// 这类失败发生在几何之外，但同样是「调用方以为加进去了、其实没加」，
    /// 算进 `drop=` 才能让探针一眼看出样本表为什么是空的。
    pub fn note_rejected(&mut self) {
        self.rejected = self.rejected.saturating_add(1);
    }

    /// 清空样本（剖分一并清掉，`rejected` 保留——它是「这个空间的病史」）。
    pub fn clear_samples(&mut self) {
        self.samples.clear();
        self.triangles.clear();
        self.tetra.clear();
        self.hull_faces.clear();
    }

    /// 设定当前参数（1D / 2D 入口，`z` 由维度归一吃掉）。非有限值整体拒绝（保持原值）。
    pub fn set_value(&mut self, x: f32, y: f32) -> bool {
        self.set_value_3d(x, y, 0.0)
    }

    /// 设定当前参数（三维入口，扩展四十九）。非有限值整体拒绝，返回是否写入。
    pub fn set_value_3d(&mut self, x: f32, y: f32, z: f32) -> bool {
        let next = self.point_of([x, y, z]);
        if !next[0].is_finite() || !next[1].is_finite() || !next[2].is_finite() {
            return false;
        }
        self.value = next;
        true
    }

    /// 一维设定参数的便捷入口（与 `set_value(x, 0.0)` 等价）。
    pub fn set_value_1d(&mut self, x: f32) -> bool {
        self.set_value(x, 0.0)
    }

    /// 当前参数（高位分量按维度恒为 0）。
    pub fn value(&self) -> [f32; 3] {
        self.value
    }

    /// 重建剖分（2D 走三角、3D 走四面体）。
    /// `add_sample` / `clear_samples` 已自动调用，只在直接改 `samples` 后需要。
    pub fn rebuild(&mut self) {
        self.triangles.clear();
        self.tetra.clear();
        self.hull_faces.clear();
        match self.dim {
            BlendDim::Linear1D => {}
            BlendDim::Planar2D => {
                if self.samples.len() < 3 {
                    return;
                }
                let points: Vec<[f32; 2]> =
                    self.samples.iter().map(|s| [s.at[0], s.at[1]]).collect();
                self.triangles = triangulate(&points);
            }
            BlendDim::Volumetric3D => {
                if self.samples.len() < 4 {
                    return;
                }
                let points: Vec<[f32; 3]> = self.samples.iter().map(|s| s.at).collect();
                let (tetra, faces) = tetrahedralize(&points);
                self.tetra = tetra;
                self.hull_faces = faces;
            }
        }
    }

    /// 当前参数下的权重（按名字合并、降序无关、和为 1）。
    ///
    /// 权重为 0 的样本**不出现在结果里**：调用方拿到的就是「这一帧真正要混合的几路」，
    /// 不必自己过滤。无样本时返回空数组。
    pub fn weights(&self) -> Vec<(String, f32)> {
        self.weights_at_3d(self.value)
    }

    /// 指定坐标下的权重（1D / 2D 入口，纯查询，不改 `value`）。
    ///
    /// 坐标含非有限值按 0 处理（与 `set_value` 拒绝写入不同：查询不该有副作用，
    /// 也不该返回 NaN 权重把整层姿态污染成 NaN）。
    pub fn weights_at(&self, at: [f32; 2]) -> Vec<(String, f32)> {
        self.weights_at_3d([at[0], at[1], 0.0])
    }

    /// 指定三维坐标下的权重（扩展四十九）。
    pub fn weights_at_3d(&self, at: [f32; 3]) -> Vec<(String, f32)> {
        if self.samples.is_empty() {
            return Vec::new();
        }
        let p = self.point_of([
            finite_or_zero(at[0]),
            finite_or_zero(at[1]),
            finite_or_zero(at[2]),
        ]);
        match self.dim {
            BlendDim::Linear1D => self.weights_1d(p[0]),
            BlendDim::Planar2D => self.weights_2d([p[0], p[1]]),
            BlendDim::Volumetric3D => self.weights_3d(p),
        }
    }

    /// 一维求值：排序后找区间，线性插值；区间外钳制到端点。
    fn weights_1d(&self, x: f32) -> Vec<(String, f32)> {
        let mut pts: Vec<(f32, &str)> = self
            .samples
            .iter()
            .map(|s| (s.at[0], s.clip.as_str()))
            .collect();
        // 排序副本保证「样本表的添加顺序」不影响结果——热重载换个写法不该换出另一张画。
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let last = pts.len() - 1;
        if last == 0 || x <= pts[0].0 {
            return merge(&[(pts[0].1, 1.0)]);
        }
        if x >= pts[last].0 {
            return merge(&[(pts[last].1, 1.0)]);
        }
        let mut lo = 0;
        for i in 0..last {
            if pts[i].0 <= x && x <= pts[i + 1].0 {
                lo = i;
                break;
            }
        }
        let (x0, x1) = (pts[lo].0, pts[lo + 1].0);
        let span = x1 - x0;
        // 同位置样本在 add_sample 处已被拒；这里只剩「浮点几乎相等」的极端情形，
        // 平均分配比除出 ∞ 更像人想要的结果。
        let (w0, w1) = if span <= EPS {
            (0.5, 0.5)
        } else {
            let t = ((x - x0) / span).clamp(0.0, 1.0);
            (1.0 - t, t)
        };
        merge(&[(pts[lo].1, w0), (pts[lo + 1].1, w1)])
    }

    /// 二维求值：先试三角形重心插值（精确），落在全部三角形外时退回最近线段。
    ///
    /// 一个点通常只属于一个三角形；但规则网格的**中心点恰落在对角线上**，
    /// 此时两侧三角形都包含它。这里把两侧结果一起交给 `merge` 归一（等价于取平均），
    /// 于是四向样本的中心得到「各 0.25」，而不是「先被遍历到的那一对各 0.5」——
    /// 后者会让玩家在中心附近微动时姿态在对角之间跳变。
    fn weights_2d(&self, p: [f32; 2]) -> Vec<(String, f32)> {
        let n = self.samples.len();
        if n == 1 {
            return merge(&[(self.samples[0].clip.as_str(), 1.0)]);
        }
        let mut hits: Vec<(&str, f32)> = Vec::new();
        for tri in &self.triangles {
            let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if idx.iter().any(|i| *i >= n) {
                continue;
            }
            let a = xy(self.samples[idx[0]].at);
            let b = xy(self.samples[idx[1]].at);
            let c = xy(self.samples[idx[2]].at);
            if let Some(w) = barycentric(a, b, c, p) {
                hits.push((self.samples[idx[0]].clip.as_str(), w[0]));
                hits.push((self.samples[idx[1]].clip.as_str(), w[1]));
                hits.push((self.samples[idx[2]].clip.as_str(), w[2]));
            }
        }
        if !hits.is_empty() {
            return merge(&hits);
        }
        self.nearest_edge_weights(p)
    }

    /// 兜底：最近线段上的投影权重（端点样本处精确为 1/0）。
    ///
    /// 遍历**全部点对**而不是只遍历三角形边：点数上限 32，496 次线段距离可忽略，
    /// 换来的是「剖分缺失 / 三角形有洞」时结果依旧正确。
    fn nearest_edge_weights(&self, p: [f32; 2]) -> Vec<(String, f32)> {
        let mut best: Option<(f32, usize, usize, f32)> = None;
        for i in 0..self.samples.len() {
            for j in (i + 1)..self.samples.len() {
                let ai = self.samples[i].at;
                let aj = self.samples[j].at;
                let (dist2, t) = closest_on_segment([ai[0], ai[1]], [aj[0], aj[1]], p);
                if best.map(|b| dist2 < b.0).unwrap_or(true) {
                    best = Some((dist2, i, j, t));
                }
            }
        }
        match best {
            Some((_, i, j, t)) => merge(&[
                (self.samples[i].clip.as_str(), 1.0 - t),
                (self.samples[j].clip.as_str(), t),
            ]),
            None => merge(&[(self.samples[0].clip.as_str(), 1.0)]),
        }
    }

    /// 三维求值：先试四面体重心插值（精确），再钳制到最近凸包面，最后退回最近样本。
    ///
    /// 与 2D 同一思路：点可能同时属于多个四面体（正好落在网格面上），两侧结果一并
    /// 交给 `merge` 归一，避免玩家在两面之间微动时姿态跳变。
    fn weights_3d(&self, p: [f32; 3]) -> Vec<(String, f32)> {
        let n = self.samples.len();
        if n == 1 {
            return merge(&[(self.samples[0].clip.as_str(), 1.0)]);
        }
        let mut hits: Vec<(&str, f32)> = Vec::new();
        for t in &self.tetra {
            let idx = [t[0] as usize, t[1] as usize, t[2] as usize, t[3] as usize];
            if idx.iter().any(|i| *i >= n) {
                continue;
            }
            let a = self.samples[idx[0]].at;
            let b = self.samples[idx[1]].at;
            let c = self.samples[idx[2]].at;
            let d = self.samples[idx[3]].at;
            if let Some(w) = barycentric3(a, b, c, d, p) {
                for k in 0..4 {
                    hits.push((self.samples[idx[k]].clip.as_str(), w[k]));
                }
            }
        }
        if !hits.is_empty() {
            return merge(&hits);
        }
        if let Some(w) = self.nearest_face_weights(p) {
            return w;
        }
        self.nearest_sample_weights(p)
    }

    /// 兜底一：投影到最近凸包面上的重心权重（样本处在凸包外时的钳制，不做外插）。
    ///
    /// 只遍历凸包面（重建时算好，量级 O(样本数)），不遍历内部面：
    /// 凸包外的点最近的支撑一定落在凸包边界上。
    fn nearest_face_weights(&self, p: [f32; 3]) -> Option<Vec<(String, f32)>> {
        let n = self.samples.len();
        let mut best: Option<(f32, [f32; 3], [u16; 3])> = None;
        for f in &self.hull_faces {
            let idx = [f[0] as usize, f[1] as usize, f[2] as usize];
            if idx.iter().any(|i| *i >= n) {
                continue;
            }
            let a = self.samples[idx[0]].at;
            let b = self.samples[idx[1]].at;
            let c = self.samples[idx[2]].at;
            let (dist2, w) = closest_on_triangle(a, b, c, p);
            if best.map(|b| dist2 < b.0).unwrap_or(true) {
                best = Some((dist2, w, *f));
            }
        }
        best.map(|(_, w, f)| {
            merge(&[
                (self.samples[f[0] as usize].clip.as_str(), w[0]),
                (self.samples[f[1] as usize].clip.as_str(), w[1]),
                (self.samples[f[2] as usize].clip.as_str(), w[2]),
            ])
        })
    }

    /// 兜底二：最近样本（权重 1）。只在没有体剖分时用到——样本不足 4 个，
    /// 或全部共面 / 共线。此时精确插值无从谈起，退回「像谁就取谁」。
    fn nearest_sample_weights(&self, p: [f32; 3]) -> Vec<(String, f32)> {
        let mut best: Option<(f32, usize)> = None;
        for (i, s) in self.samples.iter().enumerate() {
            let d = distance3(s.at, p);
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, i));
            }
        }
        match best {
            Some((_, i)) => merge(&[(self.samples[i].clip.as_str(), 1.0)]),
            None => Vec::new(),
        }
    }

    /// 单行摘要（`layer_report` 与日志用）。
    ///
    /// 形如 `1d(speed) n=3 v=1.50` / `2d(move.x,move.y) n=4 tri=2 v=(0.50,-0.50)` /
    /// `3d(move.x,move.y,speed) n=24 tet=71 drop=0 v=(0.00,1.00,2.00)`，
    /// 与 `layer_report` 的 `k=v` 风格一致，便于 grep。
    pub fn brief(&self) -> String {
        match self.dim {
            BlendDim::Linear1D => format!(
                "1d({}) n={} drop={} v={:.2}",
                self.params[0],
                self.samples.len(),
                self.rejected,
                self.value[0]
            ),
            BlendDim::Planar2D => format!(
                "2d({},{}) n={} tri={} drop={} v=({:.2},{:.2})",
                self.params[0],
                self.params[1],
                self.samples.len(),
                self.triangles.len(),
                self.rejected,
                self.value[0],
                self.value[1]
            ),
            BlendDim::Volumetric3D => format!(
                "3d({},{},{}) n={} tet={} drop={} v=({:.2},{:.2},{:.2})",
                self.params[0],
                self.params[1],
                self.params[2],
                self.samples.len(),
                self.tetra.len(),
                self.rejected,
                self.value[0],
                self.value[1],
                self.value[2]
            ),
        }
    }

    /// 权重单行：`idle=0.50,walk=0.50`（脚本返回值 / 探针日志用）。
    pub fn weights_line(&self) -> String {
        self.weights()
            .iter()
            .map(|(name, w)| format!("{name}={w:.2}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// 非有限值归零（查询侧口径）。
fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 距离平方。
fn distance2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    dx * dx + dy * dy
}

/// 按片段名合并权重，再归一化；零权重条目直接丢弃。
fn merge(entries: &[(&str, f32)]) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    for (name, w) in entries {
        if !w.is_finite() || *w <= 0.0 {
            continue;
        }
        match out.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 += *w,
            None => out.push(((*name).to_string(), *w)),
        }
    }
    let raw: Vec<f32> = out.iter().map(|(_, w)| *w).collect();
    // 与层内权重复用同一个归一函数：全零返回空数组，调用方据此跳过本层。
    let norm = normalize_weights(&raw);
    out.iter()
        .zip(norm)
        .map(|((name, _), w)| (name.clone(), w))
        .collect()
}

/// 点 `p` 到线段 `a-b` 的（距离平方, 归一化位置）。
fn closest_on_segment(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> (f32, f32) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 <= EPS * EPS {
        return (distance2(a, p), 0.0);
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    (distance2(q, p), t)
}

/// 重心坐标：`Some([w_a, w_b, w_c])` 表示点落在三角形内（含边界）。
///
/// 返回前已把负的极小值夹到 0 并重新归一，因此调用方拿到的权重可直接用于合成。
fn barycentric(a: [f32; 2], b: [f32; 2], c: [f32; 2], p: [f32; 2]) -> Option<[f32; 3]> {
    let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if det.abs() <= AREA_EPS {
        return None;
    }
    let w1 = ((p[0] - a[0]) * (c[1] - a[1]) - (p[1] - a[1]) * (c[0] - a[0])) / det;
    let w2 = ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / det;
    let w0 = 1.0 - w1 - w2;
    if w0 < -BARY_TOLERANCE || w1 < -BARY_TOLERANCE || w2 < -BARY_TOLERANCE {
        return None;
    }
    let w = [w0.max(0.0), w1.max(0.0), w2.max(0.0)];
    let sum = w[0] + w[1] + w[2];
    if sum <= 0.0 {
        return None;
    }
    Some([w[0] / sum, w[1] / sum, w[2] / sum])
}

/// 三角形面积的两倍（判退化用）。
fn double_area(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
}

/// 全部样本是否共线（共线时不存在有效剖分，求值直接走「最近线段」）。
fn all_collinear(points: &[[f32; 2]]) -> bool {
    let a = points[0];
    let mut dir = None;
    for p in &points[1..] {
        if distance2(a, *p) > EPS * EPS {
            dir = Some([p[0] - a[0], p[1] - a[1]]);
            break;
        }
    }
    let Some(dir) = dir else {
        // 全部点重合：_add_sample_ 已在这一步之前拒掉重复位置，这里只是防守。
        return true;
    };
    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
    for p in &points[1..] {
        let cross = dir[0] * (p[1] - a[1]) - dir[1] * (p[0] - a[0]);
        if cross.abs() > EPS * len {
            return false;
        }
    }
    true
}

/// 点是否落在三角形外接圆内（Bowyer–Watson 的核心判定）。
///
/// 退化三角形一律判「在内」：否则剖分会卡在同一个点上反复插入而无法推进。
fn in_circumcircle(verts: &[[f32; 2]], tri: [usize; 3], p: [f32; 2]) -> bool {
    let a = verts[tri[0]];
    let b = verts[tri[1]];
    let c = verts[tri[2]];
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() <= AREA_EPS {
        return true;
    }
    let (a2, b2, c2) = (
        a[0] * a[0] + a[1] * a[1],
        b[0] * b[0] + b[1] * b[1],
        c[0] * c[0] + c[1] * c[1],
    );
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let r2 = (a[0] - ux) * (a[0] - ux) + (a[1] - uy) * (a[1] - uy);
    let px = p[0] - ux;
    let py = p[1] - uy;
    px * px + py * py <= r2 * (1.0 + 1e-6) + EPS
}

/// Bowyer–Watson 三角剖分：返回顶点下标三元组（`u16`，样本上限 16 足够）。
fn triangulate(points: &[[f32; 2]]) -> Vec<[u16; 3]> {
    let n = points.len();
    if n < 3 || all_collinear(points) {
        return Vec::new();
    }
    let mut verts: Vec<[f32; 2]> = points.to_vec();
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for p in points {
        min_x = min_x.min(p[0]);
        min_y = min_y.min(p[1]);
        max_x = max_x.max(p[0]);
        max_y = max_y.max(p[1]);
    }
    let span = (max_x - min_x).max(max_y - min_y).max(1.0) * SUPER_SCALE;
    let (cx, cy) = ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
    let super_base = verts.len();
    verts.push([cx - 2.0 * span, cy - span]);
    verts.push([cx, cy + 2.0 * span]);
    verts.push([cx + 2.0 * span, cy - span]);

    let mut tris: Vec<[usize; 3]> = vec![[super_base, super_base + 1, super_base + 2]];
    for (i, point) in verts.iter().enumerate().take(n) {
        let p = *point;
        let mut kept: Vec<[usize; 3]> = Vec::new();
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for tri in tris.drain(..) {
            if in_circumcircle(&verts, tri, p) {
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    // 内部边会在两个「坏三角形」里各出现一次（方向相反），双双消掉。
                    match edges.iter().position(|e| e.0 == b && e.1 == a) {
                        Some(pos) => {
                            edges.remove(pos);
                        }
                        None => edges.push((a, b)),
                    }
                }
            } else {
                kept.push(tri);
            }
        }
        for (a, b) in edges {
            kept.push([a, b, i]);
        }
        tris = kept;
    }

    let mut out: Vec<[u16; 3]> = Vec::new();
    for tri in tris {
        if tri.iter().any(|v| *v >= n) {
            continue;
        }
        if double_area(verts[tri[0]], verts[tri[1]], verts[tri[2]]) <= AREA_EPS {
            continue;
        }
        out.push([tri[0] as u16, tri[1] as u16, tri[2] as u16]);
    }
    // 输出顺序稳定化：剖分的插入顺序依赖样本顺序，排序后同一组样本恒得同一份三角形表。
    out.sort();
    out
}

// ---------------- 三维：四面体剖分（扩展四十九） ----------------

/// 取三维坐标的前两维（2D 求值用；维度归一保证 `z` 恒为 0）。
fn xy(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[1]]
}

/// 距离平方（三维）。
fn distance3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = (b[0] - a[0]) as f64;
    let dy = (b[1] - a[1]) as f64;
    let dz = (b[2] - a[2]) as f64;
    (dx * dx + dy * dy + dz * dz) as f32
}

/// 四面体有向体积的 6 倍（`d` 落在 `abc` 正面时为正）。
///
/// 全程用 f64：坐标量级 10 时 f32 只剩 7 位有效数字，判「共面 / 退化」的阈值
/// （1e-12）在 f32 下会被舍入噪声淹掉。
fn signed_volume6(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> f64 {
    let (ax, ay, az) = (
        (a[0] - d[0]) as f64,
        (a[1] - d[1]) as f64,
        (a[2] - d[2]) as f64,
    );
    let (bx, by, bz) = (
        (b[0] - d[0]) as f64,
        (b[1] - d[1]) as f64,
        (b[2] - d[2]) as f64,
    );
    let (cx, cy, cz) = (
        (c[0] - d[0]) as f64,
        (c[1] - d[1]) as f64,
        (c[2] - d[2]) as f64,
    );
    ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)
}

/// 4×4 行列式（按第一行余子式展开）。
fn det4(m: [[f64; 4]; 4]) -> f64 {
    let mut det = 0.0;
    for col in 0..4 {
        let mut minor = [[0.0_f64; 3]; 3];
        for (r, row) in m.iter().enumerate().skip(1) {
            let rest = row.iter().enumerate().filter(|(c, _)| *c != col);
            for (k, (_, v)) in rest.enumerate() {
                minor[r - 1][k] = *v;
            }
        }
        let sign = if col % 2 == 0 { 1.0 } else { -1.0 };
        det += sign * m[0][col] * det3(&minor);
    }
    det
}

/// 3×3 行列式。
fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// 三维重心坐标：`Some([w_a, w_b, w_c, w_d])` 表示点落在四面体内（含边界）。
///
/// 返回前已把负的极小值夹到 0 并重新归一，调用方拿到的权重可直接用于合成。
fn barycentric3(
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    p: [f32; 3],
) -> Option<[f32; 4]> {
    let vol = signed_volume6(a, b, c, d);
    if vol.abs() <= VOLUME_EPS {
        return None;
    }
    // 权重顺序必须与传入的顶点顺序一致：把 p 换掉哪个顶点，得到的子体积就对应那个顶点。
    let wa = signed_volume6(p, b, c, d) / vol;
    let wb = signed_volume6(a, p, c, d) / vol;
    let wc = signed_volume6(a, b, p, d) / vol;
    let wd = 1.0 - wa - wb - wc;
    let tol = BARY_TOLERANCE as f64;
    if wa < -tol || wb < -tol || wc < -tol || wd < -tol {
        return None;
    }
    let w = [wa.max(0.0), wb.max(0.0), wc.max(0.0), wd.max(0.0)];
    let sum: f64 = w.iter().sum();
    if sum <= 0.0 {
        return None;
    }
    Some([
        (w[0] / sum) as f32,
        (w[1] / sum) as f32,
        (w[2] / sum) as f32,
        (w[3] / sum) as f32,
    ])
}

/// 点是否落在四面体外接球内（Bowyer–Watson 的三维核心判定）。
///
/// 退化四面体一律判「在内」：否则剖分会卡在同一个点上反复插入而无法推进
/// （与 2D 的 `in_circumcircle` 同一条处理）。
fn in_sphere(verts: &[[f32; 3]], t: [usize; 4], p: [f32; 3]) -> bool {
    let a = verts[t[0]];
    let b = verts[t[1]];
    let c = verts[t[2]];
    let d = verts[t[3]];
    let orient = signed_volume6(a, b, c, d);
    if orient.abs() <= VOLUME_EPS {
        return true;
    }
    let rows = [a, b, c, d].map(|v| {
        let dx = (v[0] - p[0]) as f64;
        let dy = (v[1] - p[1]) as f64;
        let dz = (v[2] - p[2]) as f64;
        [dx, dy, dz, dx * dx + dy * dy + dz * dz]
    });
    let det = det4(rows);
    if orient > 0.0 {
        det > 0.0
    } else {
        det < 0.0
    }
}

/// 点是否在四面体内（含边界，容差按有向体积的正负号判）。
fn inside_tetra(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], p: [f32; 3]) -> bool {
    let vol = signed_volume6(a, b, c, d);
    if vol.abs() <= VOLUME_EPS {
        return false;
    }
    let sign = if vol > 0.0 { 1.0 } else { -1.0 };
    let parts = [
        signed_volume6(p, b, c, d),
        signed_volume6(a, p, c, d),
        signed_volume6(a, b, p, d),
        signed_volume6(a, b, c, p),
    ];
    parts.iter().all(|v| *v * sign >= -VOLUME_EPS)
}

/// 全部样本是否共面（共面时不存在体剖分，求值退化为最近样本）。
fn all_coplanar(points: &[[f32; 3]]) -> bool {
    let a = points[0];
    // 先找两个与 a 不共线的点，定出平面法向；找不到说明全部共线。
    let mut normal: Option<[f64; 3]> = None;
    'search: for i in 1..points.len() {
        let u = [
            (points[i][0] - a[0]) as f64,
            (points[i][1] - a[1]) as f64,
            (points[i][2] - a[2]) as f64,
        ];
        let ul = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
        if ul <= EPS as f64 {
            continue;
        }
        for q in points.iter().skip(i + 1) {
            let v = [
                (q[0] - a[0]) as f64,
                (q[1] - a[1]) as f64,
                (q[2] - a[2]) as f64,
            ];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if nl > EPS as f64 * ul {
                normal = Some(n);
                break 'search;
            }
        }
    }
    let Some(n) = normal else {
        return true;
    };
    let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    for p in &points[1..] {
        let v = [
            (p[0] - a[0]) as f64,
            (p[1] - a[1]) as f64,
            (p[2] - a[2]) as f64,
        ];
        let dist = (v[0] * n[0] + v[1] * n[1] + v[2] * n[2]).abs() / nl;
        if dist > EPS as f64 {
            return false;
        }
    }
    true
}

/// 点到三角形的最近点：返回（距离平方, 三个顶点的重心权重）。
///
/// 与 2D 的 `barycentric` 不同，这里不要求点落在三角形内——凸包外的参数值要投影到
/// 最近的面上（钳制），所以按「最近特征」分段求：内部 → 重心插值，边 → 边插值，
/// 顶点 → 该顶点权重 1。
fn closest_on_triangle(a: [f32; 3], b: [f32; 3], c: [f32; 3], p: [f32; 3]) -> (f32, [f32; 3]) {
    let sub = |x: [f32; 3], y: [f32; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let dot = |x: [f32; 3], y: [f32; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (distance3(a, p), [1.0, 0.0, 0.0]);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (distance3(b, p), [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let q = [a[0] + ab[0] * v, a[1] + ab[1] * v, a[2] + ab[2] * v];
        return (distance3(q, p), [1.0 - v, v, 0.0]);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (distance3(c, p), [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let q = [a[0] + ac[0] * w, a[1] + ac[1] * w, a[2] + ac[2] * w];
        return (distance3(q, p), [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let bc = sub(c, b);
        let q = [b[0] + bc[0] * w, b[1] + bc[1] * w, b[2] + bc[2] * w];
        return (distance3(q, p), [0.0, 1.0 - w, w]);
    }
    let denom = va + vb + vc;
    if denom.abs() <= f32::EPSILON {
        // 退化三角形：三个点共线或重合，退回最近顶点（重建时已过滤这类面，这里只防守）。
        let da = distance3(a, p);
        let db = distance3(b, p);
        let dc = distance3(c, p);
        return if da <= db && da <= dc {
            (da, [1.0, 0.0, 0.0])
        } else if db <= dc {
            (db, [0.0, 1.0, 0.0])
        } else {
            (dc, [0.0, 0.0, 1.0])
        };
    }
    let inv = 1.0 / denom;
    let v = vb * inv;
    let w = vc * inv;
    let q = [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ];
    (distance3(q, p), [1.0 - v - w, v, w])
}

/// 顶点下标三元组排序成稳定 key（面去重 / 对消用）。
fn face_key(f: [usize; 3]) -> [usize; 3] {
    let mut k = f;
    k.sort_unstable();
    k
}

/// 四面体剖分（Bowyer–Watson 三维）：返回（四面体顶点下标, 凸包面）。
///
/// 与 2D 版本同构，只是「坏三角形 → 坏四面体」、外接圆 → 外接球、
/// 边去重 → 面对消（内部面在两个坏四面体里各出现一次，双双消掉）。
/// 四面体统一成「有向体积为正」的顶点顺序，`in_sphere` 的正负号判定才成立。
fn tetrahedralize(points: &[[f32; 3]]) -> (Vec<[u16; 4]>, Vec<[u16; 3]>) {
    let n = points.len();
    if n < 4 || all_coplanar(points) {
        return (Vec::new(), Vec::new());
    }
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in points {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let extent = (max[0] - min[0])
        .max(max[1] - min[1])
        .max(max[2] - min[2])
        .max(1.0);

    // 超级四面体：正四面体四角，边长系数 L。L = 2·extent 时经典构造能包住整个包围盒
    // （最远的对角点满足 x+y+z = 1.5·extent < L），这里仍做一次包含性校验，不满足就翻倍。
    let mut verts: Vec<[f32; 3]> = points.to_vec();
    let base = verts.len();
    let mut scale = extent * SUPER_SCALE;
    let super_of = |c: [f32; 3], l: f32| {
        [
            [c[0] - l, c[1] - l, c[2] - l],
            [c[0] + l, c[1] + l, c[2] - l],
            [c[0] + l, c[1] - l, c[2] + l],
            [c[0] - l, c[1] + l, c[2] + l],
        ]
    };
    let mut quad = super_of(center, scale);
    for _ in 0..8 {
        let ok = points
            .iter()
            .all(|p| inside_tetra(quad[0], quad[1], quad[2], quad[3], *p));
        if ok {
            break;
        }
        scale *= 2.0;
        quad = super_of(center, scale);
    }
    for v in quad {
        verts.push(v);
    }

    let mut tets: Vec<[usize; 4]> = vec![[base, base + 1, base + 2, base + 3]];
    for i in 0..n {
        let p = verts[i];
        let mut kept: Vec<[usize; 4]> = Vec::new();
        let mut face_count: HashMap<[usize; 3], usize> = HashMap::new();
        for t in tets {
            if in_sphere(&verts, t, p) {
                // 坏四面体的 4 个面全部参与对消
                for omit in 0..4 {
                    let f = [t[(omit + 1) % 4], t[(omit + 2) % 4], t[(omit + 3) % 4]];
                    *face_count.entry(face_key(f)).or_insert(0) += 1;
                }
            } else {
                kept.push(t);
            }
        }
        for (f, count) in face_count {
            if count != 1 {
                continue;
            }
            let mut t = [f[0], f[1], f[2], i];
            if signed_volume6(verts[t[0]], verts[t[1]], verts[t[2]], verts[t[3]]) < 0.0 {
                t.swap(1, 2);
            }
            kept.push(t);
        }
        tets = kept;
    }

    let mut out: Vec<[u16; 4]> = Vec::new();
    let mut face_seen: HashMap<[usize; 3], usize> = HashMap::new();
    for t in &tets {
        if t.iter().any(|v| *v >= n) {
            continue;
        }
        if signed_volume6(verts[t[0]], verts[t[1]], verts[t[2]], verts[t[3]]).abs() <= VOLUME_EPS {
            continue;
        }
        out.push([t[0] as u16, t[1] as u16, t[2] as u16, t[3] as u16]);
        for omit in 0..4 {
            let f = [t[(omit + 1) % 4], t[(omit + 2) % 4], t[(omit + 3) % 4]];
            *face_seen.entry(face_key(f)).or_insert(0) += 1;
        }
    }
    // 凸包面 = 只被一个四面体用到的面（内部面在两个四面体里各出现一次）
    let mut hull: Vec<[u16; 3]> = face_seen
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(f, _)| [f[0] as u16, f[1] as u16, f[2] as u16])
        .collect();
    // 输出顺序稳定化：HashMap 迭代顺序不固定，排序后同一组样本恒得同一份剖分表。
    out.sort();
    hull.sort();
    (out, hull)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space_1d() -> BlendSpace {
        let mut s = BlendSpace::new_1d("speed");
        assert!(s.add_sample("idle", [0.0, 0.0]));
        assert!(s.add_sample("walk", [1.0, 0.0]));
        assert!(s.add_sample("run", [2.0, 0.0]));
        s
    }

    fn space_2d() -> BlendSpace {
        let mut s = BlendSpace::new_2d("move.x", "move.y");
        assert!(s.add_sample("idle", [0.0, 0.0]));
        assert!(s.add_sample("right", [1.0, 0.0]));
        assert!(s.add_sample("forward", [0.0, 1.0]));
        assert!(s.add_sample("quad", [1.0, 1.0]));
        s
    }

    fn weight_of(weights: &[(String, f32)], name: &str) -> f32 {
        weights
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, w)| *w)
            .unwrap_or(0.0)
    }

    fn sum_of(weights: &[(String, f32)]) -> f32 {
        weights.iter().map(|(_, w)| *w).sum()
    }

    #[test]
    fn linear_1d_interpolates_between_neighbours() {
        let mut s = space_1d();
        s.set_value_1d(0.5);
        let w = s.weights();
        assert!((weight_of(&w, "idle") - 0.5).abs() < 1e-4);
        assert!((weight_of(&w, "walk") - 0.5).abs() < 1e-4);
        assert_eq!(weight_of(&w, "run"), 0.0);

        s.set_value_1d(1.5);
        let w = s.weights();
        assert!((weight_of(&w, "walk") - 0.5).abs() < 1e-4);
        assert!((weight_of(&w, "run") - 0.5).abs() < 1e-4);
    }

    #[test]
    fn linear_1d_clamps_outside_range() {
        let mut s = space_1d();
        // 超出录制范围不外插：钳到端点，权重精确为 1。
        s.set_value_1d(-5.0);
        let w = s.weights();
        assert_eq!(w.len(), 1);
        assert_eq!(weight_of(&w, "idle"), 1.0);

        s.set_value_1d(9.0);
        let w = s.weights();
        assert_eq!(w.len(), 1);
        assert_eq!(weight_of(&w, "run"), 1.0);
    }

    #[test]
    fn linear_1d_single_sample_is_constant() {
        let mut s = BlendSpace::new_1d("speed");
        assert!(s.add_sample("only", [3.0, 0.0]));
        s.set_value_1d(-100.0);
        assert_eq!(s.weights(), vec![("only".to_string(), 1.0)]);
        s.set_value_1d(100.0);
        assert_eq!(s.weights(), vec![("only".to_string(), 1.0)]);
    }

    #[test]
    fn linear_1d_merges_same_clip() {
        // 同一个动作在参数轴两端各录一段：查询端点时不应出现两条同名条目。
        let mut s = BlendSpace::new_1d("speed");
        assert!(s.add_sample("walk", [0.0, 0.0]));
        assert!(s.add_sample("run", [1.0, 0.0]));
        assert!(s.add_sample("walk", [2.0, 0.0]));
        s.set_value_1d(1.5);
        let w = s.weights();
        assert_eq!(w.len(), 2);
        assert!((weight_of(&w, "walk") - 0.5).abs() < 1e-4);
        assert!((weight_of(&w, "run") - 0.5).abs() < 1e-4);
    }

    #[test]
    fn linear_1d_rejects_duplicate_position() {
        let mut s = space_1d();
        assert!(!s.add_sample("dup", [1.0, 0.0]));
        assert_eq!(s.sample_count(), 3);
        assert_eq!(s.rejected(), 1);
        // 拒绝后求值不受影响
        s.set_value_1d(1.0);
        assert_eq!(weight_of(&s.weights(), "walk"), 1.0);
    }

    #[test]
    fn add_sample_rejects_bad_input() {
        let mut s = BlendSpace::new_1d("speed");
        assert!(!s.add_sample("", [0.0, 0.0]));
        assert!(!s.add_sample("  ", [0.0, 0.0]));
        assert!(!s.add_sample("nan", [f32::NAN, 0.0]));
        assert!(!s.add_sample("inf", [0.0, f32::INFINITY]));
        assert_eq!(s.rejected(), 4);
        assert!(s.is_empty());
    }

    #[test]
    fn linear_1d_ignores_second_axis() {
        // 1D 只认 x：y 在入库时被归零，因此 y 不同不算重复位置。
        let mut s = BlendSpace::new_1d("speed");
        assert!(s.add_sample("a", [0.0, 5.0]));
        assert!(s.add_sample("b", [1.0, -7.0]));
        assert_eq!(s.samples[0].at, [0.0, 0.0, 0.0]);
        assert_eq!(s.samples[1].at, [1.0, 0.0, 0.0]);
        s.set_value(0.5, 999.0);
        assert_eq!(s.value(), [0.5, 0.0, 0.0]);
    }

    #[test]
    fn planar_2d_corner_is_exact() {
        let mut s = space_2d();
        assert_eq!(s.triangle_count(), 2);
        s.set_value(1.0, 0.0);
        let w = s.weights();
        assert_eq!(w.len(), 1);
        assert_eq!(weight_of(&w, "right"), 1.0);
    }

    #[test]
    fn planar_2d_barycentric_inside_triangle() {
        let mut s = BlendSpace::new_2d("x", "y");
        assert!(s.add_sample("a", [0.0, 0.0]));
        assert!(s.add_sample("b", [1.0, 0.0]));
        assert!(s.add_sample("c", [0.0, 1.0]));
        assert_eq!(s.triangle_count(), 1);
        s.set_value(1.0 / 3.0, 1.0 / 3.0);
        let w = s.weights();
        assert!((weight_of(&w, "a") - 1.0 / 3.0).abs() < 1e-3);
        assert!((weight_of(&w, "b") - 1.0 / 3.0).abs() < 1e-3);
        assert!((weight_of(&w, "c") - 1.0 / 3.0).abs() < 1e-3);
        assert!((sum_of(&w) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn planar_2d_quad_center_splits_across_diagonal() {
        // 四边形中心恰好落在对角线上：只由对角两个样本支撑，另外两个三角形顶点权重为 0。
        // 这是三角剖分的固有行为（Unity Freeform 2D 同此），不是 bug；
        // 需要「四向均匀」时应在中心放一个样本（见下一个测试）。
        let mut s = space_2d();
        s.set_value(0.5, 0.5);
        let w = s.weights();
        assert!((weight_of(&w, "idle") - 0.5).abs() < 1e-3);
        assert!((weight_of(&w, "quad") - 0.5).abs() < 1e-3);
        assert_eq!(weight_of(&w, "right"), 0.0);
        assert_eq!(weight_of(&w, "forward"), 0.0);
        assert!((sum_of(&w) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn planar_2d_center_sample_is_exact() {
        // 中心放一个 idle：中心精确归 idle，四角权重全 0 —— 这就是「八向移动 + 静止」的写法。
        let mut s = space_2d();
        assert!(s.add_sample("center", [0.5, 0.5]));
        assert_eq!(s.sample_count(), 5);
        s.set_value(0.5, 0.5);
        let w = s.weights();
        assert_eq!(w.len(), 1);
        assert_eq!(weight_of(&w, "center"), 1.0);
        // 中心与右中点的中点：center / right 各半
        s.set_value(0.75, 0.5);
        let w = s.weights();
        assert!((sum_of(&w) - 1.0).abs() < 1e-4);
        assert!(
            weight_of(&w, "center") > 0.3 && weight_of(&w, "right") > 0.05,
            "{w:?}"
        );
    }

    #[test]
    fn planar_2d_outside_clamps_to_nearest_edge() {
        let mut s = space_2d();
        // (2, 0) 在四边形外右侧：最近点应是 right(1,0)，权重 1。
        s.set_value(2.0, 0.0);
        let w = s.weights();
        assert_eq!(weight_of(&w, "right"), 1.0);

        // (1, 2) 在上方：最近点 forward-quad 之间，但 forward(0,1) 与 quad(1,1)
        // 都在 y=1 线上；(1,2) 的正下方是 quad(1,1)。
        s.set_value(1.0, 2.0);
        let w = s.weights();
        assert_eq!(weight_of(&w, "quad"), 1.0);

        // (2, 1) 右上方：最近线段是 right(1,0)-quad(1,1) 的顶端，仍是 quad。
        s.set_value(2.0, 1.0);
        assert_eq!(weight_of(&s.weights(), "quad"), 1.0);
    }

    #[test]
    fn planar_2d_collinear_falls_back_to_segment() {
        let mut s = BlendSpace::new_2d("x", "y");
        assert!(s.add_sample("a", [0.0, 0.0]));
        assert!(s.add_sample("b", [1.0, 1.0]));
        assert!(s.add_sample("c", [2.0, 2.0]));
        // 三点共线：没有有效三角形，但不能因此什么都不给。
        assert_eq!(s.triangle_count(), 0);
        s.set_value(0.5, 0.5);
        let w = s.weights();
        assert!((weight_of(&w, "a") - 0.5).abs() < 1e-3);
        assert!((weight_of(&w, "b") - 0.5).abs() < 1e-3);
        assert!((sum_of(&w) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn capacity_is_capped() {
        let mut s = space_1d();
        // 已有 3 个，再加到上限为止。
        for i in 3..MAX_BLEND_SAMPLES {
            assert!(s.add_sample(format!("c{i}"), [i as f32, 0.0]));
        }
        assert_eq!(s.sample_count(), MAX_BLEND_SAMPLES);
        assert!(!s.add_sample("overflow", [99.0, 0.0]));
        assert_eq!(s.sample_count(), MAX_BLEND_SAMPLES);
        // 上限拒绝不计入「非法样本」——它不是用户的错。
        assert_eq!(s.rejected(), 0);
    }

    #[test]
    fn weights_are_always_normalized() {
        let mut s1 = space_1d();
        for step in -4..=12 {
            let v = step as f32 * 0.5;
            s1.set_value_1d(v);
            let w = s1.weights();
            assert!(!w.is_empty());
            assert!(
                (sum_of(&w) - 1.0).abs() < 1e-4,
                "1D v={v} 权重和为 {}",
                sum_of(&w)
            );
            assert!(w.iter().all(|(_, x)| *x > 0.0));
        }

        let mut s2 = space_2d();
        for ix in -2..=6 {
            for iy in -2..=6 {
                let v = [ix as f32 * 0.5, iy as f32 * 0.5];
                s2.set_value(v[0], v[1]);
                let w = s2.weights();
                assert!(!w.is_empty());
                assert!(
                    (sum_of(&w) - 1.0).abs() < 1e-4,
                    "2D v={v:?} 权重和为 {}",
                    sum_of(&w)
                );
            }
        }
    }

    #[test]
    fn empty_space_returns_no_weights() {
        let s = BlendSpace::new_2d("x", "y");
        assert!(s.is_empty());
        assert!(s.weights().is_empty());
        assert_eq!(s.weights_line(), "");
        // 非有限坐标查询不该产出 NaN 权重
        assert!(s.weights_at([f32::NAN, f32::NAN]).is_empty());
    }

    #[test]
    fn set_value_rejects_non_finite_and_keeps_old() {
        let mut s = space_2d();
        s.set_value(0.25, 0.5);
        // 二维两个分量都参与几何判定，任一非有限就整条拒绝，保持旧值。
        assert!(!s.set_value(f32::NAN, 0.0));
        assert!(!s.set_value(0.0, f32::INFINITY));
        assert_eq!(s.value(), [0.25, 0.5, 0.0]);
        // 查询侧非有限按 0 处理：得到 idle 而不是 NaN 权重
        let w = s.weights_at([f32::NAN, f32::NAN]);
        assert_eq!(weight_of(&w, "idle"), 1.0);

        // 一维只认 x：y 传 ∞ 也会被丢弃，不算非法。
        let mut s1 = space_1d();
        assert!(s1.set_value(1.0, f32::INFINITY));
        assert_eq!(s1.value(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn report_and_weights_line_are_greppable() {
        let mut s = space_2d();
        s.set_value(0.5, 0.5);
        let brief = s.brief();
        assert!(brief.starts_with("2d(move.x,move.y) n=4 tri=2"), "{brief}");
        assert!(brief.contains("v=(0.50,0.50)"), "{brief}");

        let line = s.weights_line();
        // 中心点落在对角线上：只有对角两个样本有权重（见 quad_center 测试）。
        assert_eq!(line, "idle=0.50,quad=0.50");

        let s1 = BlendSpace::new_1d("speed");
        assert_eq!(s1.brief(), "1d(speed) n=0 drop=0 v=0.00");
    }

    #[test]
    fn sample_order_does_not_change_result() {
        let mut a = BlendSpace::new_1d("speed");
        a.add_sample("idle", [0.0, 0.0]);
        a.add_sample("walk", [1.0, 0.0]);
        a.add_sample("run", [2.0, 0.0]);
        let mut b = BlendSpace::new_1d("speed");
        b.add_sample("run", [2.0, 0.0]);
        b.add_sample("idle", [0.0, 0.0]);
        b.add_sample("walk", [1.0, 0.0]);
        for v in [0.0, 0.3, 1.0, 1.7, 2.0] {
            a.set_value_1d(v);
            b.set_value_1d(v);
            assert_eq!(a.weights(), b.weights(), "v={v} 应与样本顺序无关");
        }
    }

    // ---------------- 三维（扩展四十九） ----------------

    /// 一个标准四面体：a=原点、b/c/d 落在三条轴上，方便手算权重。
    fn space_tetra() -> BlendSpace {
        let mut s = BlendSpace::new_3d("x", "y", "z");
        assert!(s.add_sample_3d("a", [0.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("b", [1.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("c", [0.0, 1.0, 0.0]));
        assert!(s.add_sample_3d("d", [0.0, 0.0, 1.0]));
        s
    }

    #[test]
    fn volumetric_3d_single_tetra_has_exact_barycentric() {
        let mut s = space_tetra();
        assert_eq!(s.dim(), BlendDim::Volumetric3D);
        assert_eq!(s.dim().as_str(), "3d");
        assert_eq!(s.sample_count(), 4);
        assert_eq!(s.triangle_count(), 0, "3D 不建三角剖分");
        assert_eq!(s.tetra_count(), 1, "四个不共面样本恰好一个四面体");

        // 内部点 (0.25,0.25,0.25)：a 占 1-3×0.25=0.25，b/c/d 各 0.25。
        s.set_value_3d(0.25, 0.25, 0.25);
        let w = s.weights();
        assert_eq!(w.len(), 4, "{w:?}");
        for name in ["a", "b", "c", "d"] {
            assert!((weight_of(&w, name) - 0.25).abs() < 1e-3, "{name}: {w:?}");
        }
        assert!((sum_of(&w) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn volumetric_3d_vertex_and_edge_are_exact() {
        let mut s = space_tetra();
        // 顶点：权重 1，其余不出现
        s.set_value_3d(1.0, 0.0, 0.0);
        let w = s.weights();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(weight_of(&w, "b"), 1.0);

        // 棱中点 b-c：两侧各 0.5，a/d 权重为 0 不出现在结果里
        s.set_value_3d(0.5, 0.5, 0.0);
        let w = s.weights();
        assert!((weight_of(&w, "b") - 0.5).abs() < 1e-3, "{w:?}");
        assert!((weight_of(&w, "c") - 0.5).abs() < 1e-3, "{w:?}");
        assert_eq!(weight_of(&w, "a"), 0.0);
    }

    #[test]
    fn volumetric_3d_outside_clamps_to_hull_face() {
        let mut s = space_tetra();
        // (2,0,0) 在四面体外：最近支撑是 b(1,0,0) 这个顶点。
        s.set_value_3d(2.0, 0.0, 0.0);
        let w = s.weights();
        assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{w:?}");
        assert_eq!(weight_of(&w, "b"), 1.0, "{w:?}");

        // (0.5, 0.5, 0.5) 沿 b+c+d 方向出界：投影到斜面 bcd，三角均分。
        s.set_value_3d(0.5, 0.5, 0.5);
        let w = s.weights();
        assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{w:?}");
        assert!((weight_of(&w, "b") - 1.0 / 3.0).abs() < 1e-2, "{w:?}");
        assert!((weight_of(&w, "c") - 1.0 / 3.0).abs() < 1e-2, "{w:?}");
        assert!((weight_of(&w, "d") - 1.0 / 3.0).abs() < 1e-2, "{w:?}");
        assert_eq!(weight_of(&w, "a"), 0.0, "{w:?}");
    }

    #[test]
    fn volumetric_3d_cube_has_valid_tetrahedralization() {
        // 八向移动 × 三档速度的简化版：立方体 8 个角。
        let mut s = BlendSpace::new_3d("move.x", "move.y", "speed");
        let mut n = 0;
        for ix in 0..2 {
            for iy in 0..2 {
                for iz in 0..2 {
                    assert!(s.add_sample_3d(
                        format!("c{ix}{iy}{iz}"),
                        [ix as f32, iy as f32, iz as f32]
                    ));
                    n += 1;
                }
            }
        }
        assert_eq!(s.sample_count(), n);
        let tets = s.tetra_count();
        assert!(
            (5..=6).contains(&tets),
            "立方体应为 5 或 6 个四面体，实际 {tets}"
        );
        // 每个顶点都是精确支撑
        for ix in 0..2 {
            for iy in 0..2 {
                for iz in 0..2 {
                    s.set_value_3d(ix as f32, iy as f32, iz as f32);
                    let w = s.weights();
                    assert_eq!(w.len(), 1, "角点 ({ix},{iy},{iz}) 应只由自身支撑：{w:?}");
                    assert_eq!(weight_of(&w, &format!("c{ix}{iy}{iz}")), 1.0);
                }
            }
        }
        // 体积内部的点（非面上）：权重和恒为 1，且每个权重都是有限正数
        s.set_value_3d(0.5, 0.5, 0.25);
        let w = s.weights();
        assert!(!w.is_empty());
        assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{w:?}");
        assert!(w.iter().all(|(_, v)| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn volumetric_3d_coplanar_falls_back_to_nearest_sample() {
        // 四个共面样本：体剖分不存在，但不能什么都不给。
        let mut s = BlendSpace::new_3d("x", "y", "z");
        assert!(s.add_sample_3d("a", [0.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("b", [1.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("c", [0.0, 1.0, 0.0]));
        assert!(s.add_sample_3d("d", [1.0, 1.0, 0.0]));
        assert_eq!(s.tetra_count(), 0);

        s.set_value_3d(0.9, 0.9, 0.0);
        let w = s.weights();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(weight_of(&w, "d"), 1.0);
    }

    #[test]
    fn volumetric_3d_with_three_samples_still_normalizes() {
        // 不足四个样本：没有体剖分，退回最近样本，权重和仍为 1。
        let mut s = BlendSpace::new_3d("x", "y", "z");
        assert!(s.add_sample_3d("a", [0.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("b", [1.0, 1.0, 1.0]));
        assert!(s.add_sample_3d("c", [2.0, 0.0, 0.0]));
        assert_eq!(s.tetra_count(), 0);
        for v in [[0.0, 0.0, 0.0], [1.5, 0.5, 0.5], [-3.0, 9.0, 0.0]] {
            s.set_value_3d(v[0], v[1], v[2]);
            let w = s.weights();
            assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{v:?} → {w:?}");
        }
        // 单样本三维空间恒为该样本
        let mut one = BlendSpace::new_3d("x", "y", "z");
        assert!(one.add_sample_3d("only", [0.0, 0.0, 0.0]));
        one.set_value_3d(9.0, 9.0, 9.0);
        assert_eq!(weight_of(&one.weights(), "only"), 1.0);
    }

    #[test]
    fn volumetric_3d_ignores_non_finite_value() {
        let mut s = space_tetra();
        s.set_value_3d(0.25, 0.25, 0.25);
        assert!(!s.set_value_3d(f32::NAN, 0.0, 0.0));
        assert!(!s.set_value_3d(0.0, 0.0, f32::INFINITY));
        assert_eq!(s.value(), [0.25, 0.25, 0.25]);
        // 查询侧非有限按 0 处理，不产出 NaN 权重
        let w = s.weights_at_3d([f32::NAN, f32::NAN, f32::NAN]);
        assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{w:?}");
        assert!(w.iter().all(|(_, v)| v.is_finite()));
    }

    #[test]
    fn volumetric_3d_brief_and_names() {
        let s = BlendSpace::new_3d("move.x", "move.y", "speed");
        assert_eq!(s.param_names(), vec!["move.x", "move.y", "speed"]);
        assert_eq!(
            s.brief(),
            "3d(move.x,move.y,speed) n=0 tet=0 drop=0 v=(0.00,0.00,0.00)"
        );
        assert_eq!(BlendDim::parse("3d"), Some(BlendDim::Volumetric3D));
        assert_eq!(
            BlendDim::parse(" VOLUMETRIC "),
            Some(BlendDim::Volumetric3D)
        );
        assert_eq!(BlendDim::Volumetric3D.axes(), 3);
    }

    #[test]
    fn volumetric_3d_rejects_duplicate_position_on_all_axes() {
        // 1D / 2D 归一吃掉高位，3D 必须三个分量都参与去重。
        let mut s = BlendSpace::new_3d("x", "y", "z");
        assert!(s.add_sample_3d("a", [0.0, 0.0, 0.0]));
        // 3D 空间上走二维入口：z 由维度归一补 0，仍是重复位置
        assert!(!s.add_sample("b", [0.0, 0.0]));
        assert!(!s.add_sample_3d("c", [0.0, 0.0, 0.0]));
        assert!(s.add_sample_3d("d", [0.0, 0.0, 0.5]));
        assert_eq!(s.sample_count(), 2);
        assert_eq!(s.rejected(), 2);
    }

    #[test]
    fn volumetric_3d_stays_on_layer_plane_at_layer_boundary() {
        // 速度恰好落在某一档（z=0 或 z=1）时，权重只能来自该档的样本，
        // 不能把另一档的速度混进来——否则「贴着两层 2D 叠」的老手感会被改掉。
        let mut s = BlendSpace::new_3d("x", "y", "speed");
        for iz in 0..2 {
            for ix in 0..2 {
                for iy in 0..2 {
                    assert!(s.add_sample_3d(
                        format!("c{ix}{iy}{iz}"),
                        [ix as f32, iy as f32, iz as f32]
                    ));
                }
            }
        }
        for z in [0.0_f32, 1.0] {
            for p in [[0.25, 0.25], [0.5, 0.5], [0.75, 0.25], [0.0, 1.0]] {
                s.set_value_3d(p[0], p[1], z);
                let w = s.weights();
                assert!((sum_of(&w) - 1.0).abs() < 1e-4, "p={p:?} z={z}: {w:?}");
                let other = if z == 0.0 { "1" } else { "0" };
                for (name, value) in &w {
                    assert!(
                        !name.ends_with(other),
                        "z={z} 的权重混进了另一档样本 {name}：{w:?}"
                    );
                    assert_ne!(*value, 0.0);
                }
            }
        }
        // 两层之间（z=0.5）才允许同时吃到两档，且权重仍归一。
        s.set_value_3d(0.5, 0.5, 0.5);
        let w = s.weights();
        assert!((sum_of(&w) - 1.0).abs() < 1e-4, "{w:?}");
        assert!(w.iter().any(|(n, _)| n.ends_with('0')));
        assert!(w.iter().any(|(n, _)| n.ends_with('1')));
    }

    #[test]
    fn volumetric_3d_scales_to_24_samples() {
        // 八向 × 三档速 = 24 个样本，仍在 MAX_BLEND_SAMPLES 之内，且剖分可用。
        let mut s = BlendSpace::new_3d("move.x", "move.y", "speed");
        let dirs = [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (-1.0, 1.0),
            (-1.0, 0.0),
            (-1.0, -1.0),
            (0.0, -1.0),
        ];
        for (speed, tag) in [(0.0_f32, "idle"), (1.0, "walk"), (2.0, "run")] {
            for (i, (dx, dy)) in dirs.iter().enumerate() {
                assert!(s.add_sample_3d(format!("{tag}{i}"), [*dx, *dy, speed]));
            }
        }
        assert_eq!(s.sample_count(), 24);
        assert!(s.tetra_count() > 0, "24 个样本应能建出四面体剖分");
        // 中间档速度 + 斜向：权重和恒为 1
        for speed in [0.0, 0.5, 1.0, 1.5, 2.0] {
            for p in [[0.0, 0.0], [0.6, 0.6], [-0.3, 0.9]] {
                s.set_value_3d(p[0], p[1], speed);
                let w = s.weights();
                assert!(
                    (sum_of(&w) - 1.0).abs() < 1e-4,
                    "v=({},{},{speed}) → {w:?}",
                    p[0],
                    p[1]
                );
            }
        }
    }
}
