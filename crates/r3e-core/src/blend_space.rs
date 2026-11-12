//! 1D / 2D 混合空间（扩展四十五）：把「参数」变成「一组片段权重」的纯函数。
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
//! 钳制行为、归一性全部钉住（见本文件尾部 16 项测试）。
//! 与 `ssao` / `occlusion` / `pick` 同一层级：纯 CPU 契约，渲染层与脚本层各自复用。
//!
//! # 三个关键取舍
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
//! 3. **1D 与 2D 是同一个类型**，1D 只是「y 分量恒为 0 的 2D」。
//!    分成两个类型会让 `AnimLayer` 多一层枚举、脚本多一套 API、编辑器多一种快照，
//!    而 1D 的求值路径（排序 + 相邻两样本插值）本身就比 2D 简单，没必要共用三角形。
//!
//! # 明确不做
//!
//! - 不做「轴归一化 / 每轴不同量程」：调用方把参数换算成同量纲的坐标再喂进来。
//! - 不做时间维度（那是片段内的时间轴，不是混合空间）。
//! - 不做样本增删以外的编辑（移动样本 = 删 + 加）。
//! - 不做 3D 及以上：RPG 里没有第四个连续参数需要与移动共用一张图，加上去只会让
//!   「剖分」变成「体剖分」，复杂度翻倍而收益为零。

use crate::anim_blend::normalize_weights;

/// 单个混合空间最多容纳的样本数。
///
/// 16 个足够表达「待机 / 走 / 跑 / 冲刺」（1D）与「八向移动 + 三个速度档」（2D），
/// 而求值是 O(样本数) 到 O(样本数²)：上限处也只有 120 次线段距离，每帧每层可忽略。
pub const MAX_BLEND_SAMPLES: usize = 16;

/// 坐标去重与「同位置」判定的容差（世界单位，不是像素）。
const EPS: f32 = 1e-6;

/// 超级三角形相对包围盒的放大倍数（Bowyer–Watson 要求它包住全部样本点）。
const SUPER_SCALE: f32 = 16.0;

/// 退化三角形（面积过小）的判定阈值。
const AREA_EPS: f32 = 1e-9;

/// 重心坐标落在三角形外的容差：允许极小负值，避免「样本恰在边上」时被判为在外。
const BARY_TOLERANCE: f32 = 1e-5;

/// 混合维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendDim {
    /// 一维：一个参数（速度 / 转向角 / 生命值）。
    Linear1D,
    /// 二维：两个参数（移动方向 x / y、瞄准俯仰与偏航）。
    Planar2D,
}

impl BlendDim {
    /// 规范名（日志、RPC、脚本共用同一口径）。
    pub const fn as_str(&self) -> &'static str {
        match self {
            BlendDim::Linear1D => "1d",
            BlendDim::Planar2D => "2d",
        }
    }

    /// 解析规范名（大小写不敏感，接受几个常见别名）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1d" | "linear" | "linear1d" | "linear_1d" => Some(BlendDim::Linear1D),
            "2d" | "planar" | "planar2d" | "planar_2d" => Some(BlendDim::Planar2D),
            _ => None,
        }
    }

    /// 参数个数：1D 一个、2D 两个。
    pub const fn axes(&self) -> usize {
        match self {
            BlendDim::Linear1D => 1,
            BlendDim::Planar2D => 2,
        }
    }
}

/// 一个样本：某组参数坐标上播放哪段片段。
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSample {
    /// 参数坐标。1D 只用 `at[0]`，`at[1]` 恒为 0（添加时就被归零）。
    pub at: [f32; 2],
    /// 该坐标上的片段名（与 `AnimClip::name` 同名，大小写不敏感匹配）。
    pub clip: String,
}

/// 1D / 2D 混合空间。
///
/// 字段全公开：它是**数据**而不是句柄，编辑器快照与存档要能直接读写。
/// 但增删样本请走 `add_sample` / `clear_samples`——它们负责去重与重建剖分，
/// 直接改 `samples` 会让 `triangles` 与样本失配（求值时会跳越界三角形，退回最近边，
/// 结果仍然可用，只是不再是精确插值）。
#[derive(Debug, Clone, PartialEq)]
pub struct BlendSpace {
    /// 维度。
    pub dim: BlendDim,
    /// 参数名（1D 只用第一个，第二个留空串）。
    pub params: [String; 2],
    /// 样本表（添加顺序，1D 求值时按坐标排序，不改动本表）。
    pub samples: Vec<BlendSample>,
    /// 当前参数值（求值入口）。
    pub value: [f32; 2],
    /// 2D 的三角剖分结果（1D 恒为空）。
    triangles: Vec<[u16; 3]>,
    /// 被拒绝的样本数（非法坐标 / 重复位置），只用于日志，语义是「累计」。
    rejected: usize,
}

impl BlendSpace {
    /// 建一维混合空间（`param` 是参数名，例如 `"speed"`）。
    pub fn new_1d(param: impl Into<String>) -> Self {
        Self {
            dim: BlendDim::Linear1D,
            params: [param.into(), String::new()],
            samples: Vec::new(),
            value: [0.0, 0.0],
            triangles: Vec::new(),
            rejected: 0,
        }
    }

    /// 建二维混合空间（`x` / `y` 是两个参数名）。
    pub fn new_2d(x: impl Into<String>, y: impl Into<String>) -> Self {
        Self {
            dim: BlendDim::Planar2D,
            params: [x.into(), y.into()],
            samples: Vec::new(),
            value: [0.0, 0.0],
            triangles: Vec::new(),
            rejected: 0,
        }
    }

    /// 维度。
    pub const fn dim(&self) -> BlendDim {
        self.dim
    }

    /// 参数名（1D 返一个、2D 返两个）。
    pub fn param_names(&self) -> Vec<String> {
        match self.dim {
            BlendDim::Linear1D => vec![self.params[0].clone()],
            BlendDim::Planar2D => vec![self.params[0].clone(), self.params[1].clone()],
        }
    }

    /// 样本数。
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 是否还没有任何样本（此时求值返回空权重，调用方应回落到手写权重）。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 三角剖分出的三角形个数（1D 恒为 0）。
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// 被拒绝的样本累计数。
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// 剖分结果（只读，编辑器画线框用）。
    pub fn triangles(&self) -> &[[u16; 3]] {
        &self.triangles
    }

    /// 把坐标按维度归一：1D 强制 `y = 0`，这样「重复位置判定」在两种维度下同一口径。
    fn point_of(&self, at: [f32; 2]) -> [f32; 2] {
        match self.dim {
            BlendDim::Linear1D => [at[0], 0.0],
            BlendDim::Planar2D => at,
        }
    }

    /// 加一个样本。成功返回 `true`。
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
        let clip = clip.into();
        if clip.trim().is_empty() {
            self.rejected += 1;
            return false;
        }
        if !at[0].is_finite() || !at[1].is_finite() {
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
            .any(|s| distance2(s.at, at) <= EPS * EPS)
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
    }

    /// 设定当前参数。非有限值整体拒绝（保持原值），返回是否写入。
    pub fn set_value(&mut self, x: f32, y: f32) -> bool {
        let next = self.point_of([x, y]);
        if !next[0].is_finite() || !next[1].is_finite() {
            return false;
        }
        self.value = next;
        true
    }

    /// 一维设定参数的便捷入口（与 `set_value(x, 0.0)` 等价）。
    pub fn set_value_1d(&mut self, x: f32) -> bool {
        self.set_value(x, 0.0)
    }

    /// 当前参数。
    pub fn value(&self) -> [f32; 2] {
        self.value
    }

    /// 重建 2D 剖分。`add_sample` / `clear_samples` 已自动调用，只在直接改 `samples` 后需要。
    pub fn rebuild(&mut self) {
        self.triangles.clear();
        if self.dim != BlendDim::Planar2D || self.samples.len() < 3 {
            return;
        }
        let points: Vec<[f32; 2]> = self.samples.iter().map(|s| s.at).collect();
        self.triangles = triangulate(&points);
    }

    /// 当前参数下的权重（按名字合并、降序无关、和为 1）。
    ///
    /// 权重为 0 的样本**不出现在结果里**：调用方拿到的就是「这一帧真正要混合的几路」，
    /// 不必自己过滤。无样本时返回空数组。
    pub fn weights(&self) -> Vec<(String, f32)> {
        self.weights_at(self.value)
    }

    /// 指定坐标下的权重（纯查询，不改 `value`）。
    ///
    /// 坐标含非有限值按 0 处理（与 `set_value` 拒绝写入不同：查询不该有副作用，
    /// 也不该返回 NaN 权重把整层姿态污染成 NaN）。
    pub fn weights_at(&self, at: [f32; 2]) -> Vec<(String, f32)> {
        if self.samples.is_empty() {
            return Vec::new();
        }
        let p = self.point_of([finite_or_zero(at[0]), finite_or_zero(at[1])]);
        match self.dim {
            BlendDim::Linear1D => self.weights_1d(p[0]),
            BlendDim::Planar2D => self.weights_2d(p),
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
            let a = self.samples[idx[0]].at;
            let b = self.samples[idx[1]].at;
            let c = self.samples[idx[2]].at;
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
    /// 遍历**全部点对**而不是只遍历三角形边：点数上限 16，120 次线段距离可忽略，
    /// 换来的是「剖分缺失 / 三角形有洞」时结果依旧正确。
    fn nearest_edge_weights(&self, p: [f32; 2]) -> Vec<(String, f32)> {
        let mut best: Option<(f32, usize, usize, f32)> = None;
        for i in 0..self.samples.len() {
            for j in (i + 1)..self.samples.len() {
                let (dist2, t) = closest_on_segment(self.samples[i].at, self.samples[j].at, p);
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

    /// 单行摘要（`layer_report` 与日志用）。
    ///
    /// 形如 `1d(speed) n=3 v=1.50` / `2d(move.x,move.y) n=4 tri=2 v=(0.50,-0.50)`，
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
        assert_eq!(s.samples[0].at, [0.0, 0.0]);
        assert_eq!(s.samples[1].at, [1.0, 0.0]);
        s.set_value(0.5, 999.0);
        assert_eq!(s.value(), [0.5, 0.0]);
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
        assert_eq!(s.value(), [0.25, 0.5]);
        // 查询侧非有限按 0 处理：得到 idle 而不是 NaN 权重
        let w = s.weights_at([f32::NAN, f32::NAN]);
        assert_eq!(weight_of(&w, "idle"), 1.0);

        // 一维只认 x：y 传 ∞ 也会被丢弃，不算非法。
        let mut s1 = space_1d();
        assert!(s1.set_value(1.0, f32::INFINITY));
        assert_eq!(s1.value(), [1.0, 0.0]);
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
}
