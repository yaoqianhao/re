//! 「路径长度 ≤ 直线 × 阈值」的判据脚本（`M2·D05`）。
//!
//! ## 为什么要把判据做成代码
//!
//! 「走出来的路别绕得太离谱」这条要求，写在文档里等于没写：它既没有量纲也没有
//! 边界，换个夹具就换一套说法。本模块把它钉成三件事：
//!
//! 1. **一组可复现的起终点**——由 `seed` 从可走格集合里抽出来，同 seed 同结果，
//!    抽到的点集有一个哈希（[`ScaleReport::points_digest`]），便于在测试输出里
//!    对账「这次判的到底是不是同一批点」；
//! 2. **一个比值**——`平滑后路径长度 / 起终点三维直线距离`，除以直线距离是为了
//!    让判据与场景尺寸无关（16 m 的场地和 200 m 的场地能共用一条阈值）；
//! 3. **一个开关式的阈值**——[`PathScale::limit`] 只参与比较，不参与测量。
//!    收紧阈值只会让更多用例「超限」，不会让测量值本身变一点点偏差都没有
//!    （[`measure`] 的确定性判据就钉在这里）。
//!
//! ## 不可达怎么算
//!
//! 抽样时**只收可达的对**：`PathOutcome::Unreachable` / `InvalidEndpoint` /
//! `BudgetExceeded` 一律计入 [`ScaleReport::skipped`] 后重抽，不写进
//! [`ScaleReport::samples`]。理由是不可达的路径没有长度，硬塞一个 `0.0` 或
//! `INFINITY` 进比值都会被下游读成「这条路很好」或「这条路很烂」，两种都是假结论。
//! 不可达本身该由可达性判据（[`crate::Nav::path`] 的 `Unreachable` +
//! `disconnect_point`）负责，不该混进长度判据。
//!
//! ## 抽样的间距下限
//!
//! 起终点必须至少隔 [`SCALE_MIN_STRAIGHT_CELLS`] 格。距离太近的两个点，绕一个
//! 障碍就会让比值冲到 2 以上——那不是路径质量差，是分母太小。这条下限是判据
//! 的一部分而不是夹具的运气：它保证了 1.6 这个数字对「同城两点的路」有意义。
//!
//! ## 非法配置
//!
//! `cases == 0`（没有用例的判据等于没判）、`limit <= 0`（比值恒正，阈值非正必然
//! 全红）、`limit` 非有限（`NaN` 的比较恒 `false`，会让判据**静默恒绿**，最危险）
//! 三种一律拒绝，错误类型与文案在 [`ScaleError`] 里**冻结**（同 `D02` 的
//! `InvalidReason::describe` 口径）。

use glam::Vec3;

use crate::path::PathOutcome;
use crate::smooth::smooth_path;
use crate::Nav;

/// 默认判据组数（`M2_D_PLAN` §8：20 组）。
pub const DEFAULT_SCALE_CASES: usize = 20;
/// 默认随机种子（`M2_D_PLAN` §8：seed 42）。
pub const DEFAULT_SCALE_SEED: u64 = 42;
/// 默认阈值比例（`M2_D_PLAN` §8：1.6×）。
pub const DEFAULT_SCALE_LIMIT: f32 = 1.6;
/// 抽样时起终点之间的最小直线距离，单位是**格**（乘 `cell_size` 才是米）。
pub const SCALE_MIN_STRAIGHT_CELLS: f32 = 16.0;

/// 判据配置：组数 / 种子 / 阈值比例，三者都可覆盖。
///
/// 字段是公开的（夹具里改一个数就能得到一组新判据），但**改了要重新
/// [`PathScale::validate`]**——`Default` 与 [`PathScale::new`] 出来的实例一定合法，
/// 手改字段出来的不一定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathScale {
    /// 组数（起终点对的数量），必须 ≥ 1。
    pub cases: usize,
    /// 随机种子：决定抽到哪几对点。
    pub seed: u64,
    /// 阈值比例：`路径长度 ≤ 直线距离 × limit` 才算通过。
    pub limit: f32,
}

impl Default for PathScale {
    fn default() -> Self {
        Self {
            cases: DEFAULT_SCALE_CASES,
            seed: DEFAULT_SCALE_SEED,
            limit: DEFAULT_SCALE_LIMIT,
        }
    }
}

impl PathScale {
    /// 构造并立即校验；非法配置在构造期就被拒，不留「先造出来再判」的空窗。
    pub fn new(cases: usize, seed: u64, limit: f32) -> Result<Self, ScaleError> {
        let this = Self { cases, seed, limit };
        this.validate()?;
        Ok(this)
    }

    /// 校验。顺序**有意义**：组数先于阈值，有限性先于正负号——
    /// 否则 `NaN <= 0.0` 为 `false`，会被误判成「合法」。
    pub fn validate(&self) -> Result<(), ScaleError> {
        if self.cases == 0 {
            return Err(ScaleError::ZeroCases);
        }
        if !self.limit.is_finite() {
            return Err(ScaleError::NonFiniteLimit);
        }
        if self.limit <= 0.0 {
            return Err(ScaleError::NonPositiveLimit);
        }
        Ok(())
    }

    /// 该比例是否在阈值内。`NaN` 判为**不通过**——判据宁可假红也不假绿。
    pub fn allows(&self, ratio: f32) -> bool {
        ratio.is_finite() && ratio <= self.limit
    }
}

/// 非法判据配置。文案冻结（`M2_D_PLAN` §8 判据 3），断言直接比字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleError {
    /// `cases == 0`。
    ZeroCases,
    /// `limit <= 0`。
    NonPositiveLimit,
    /// `limit` 是 `NaN` / `±∞`。
    NonFiniteLimit,
}

impl ScaleError {
    /// 稳定的机器可读标识（回包 / 日志用，不随文案改动）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZeroCases => "zeroCases",
            Self::NonPositiveLimit => "nonPositiveLimit",
            Self::NonFiniteLimit => "nonFiniteLimit",
        }
    }

    /// 人话说明。**文案冻结**：改字必须同时改判据测试。
    pub fn describe(self) -> &'static str {
        match self {
            Self::ZeroCases => "判据组数必须 ≥ 1（0 组没有判据可言）",
            Self::NonPositiveLimit => "阈值比例必须 > 0（路径长度不小于直线距离）",
            Self::NonFiniteLimit => "阈值比例必须是有限数（NaN / 无穷大不接受）",
        }
    }
}

impl std::fmt::Display for ScaleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.describe())
    }
}

impl std::error::Error for ScaleError {}

/// 一组起终点（格中心，`y` 取该格地面高度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleCase {
    /// 起点。
    pub from: Vec3,
    /// 终点。
    pub to: Vec3,
}

impl ScaleCase {
    /// 起终点三维直线距离（比值的分母）。
    pub fn straight(&self) -> f32 {
        self.from.distance(self.to)
    }
}

/// 一组的测量结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleSample {
    /// 组序号（从 0 起，等于注入顺序）。
    pub index: usize,
    /// 起点。
    pub from: Vec3,
    /// 终点。
    pub to: Vec3,
    /// 三维直线距离（分母）。
    pub straight: f32,
    /// 平滑后路径长度（分子）。
    pub length: f32,
    /// `length / straight`。
    pub ratio: f32,
}

/// 一次判据运行的完整结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleReport {
    /// 本次使用的配置（含覆盖后的值，便于回填测试日志）。
    pub scale: PathScale,
    /// 每组的测量值，长度即实际评到的组数。
    pub samples: Vec<ScaleSample>,
    /// 抽样过程中丢弃的次数（太近 / 不可达 / 端点非法 / 算不完）。
    pub skipped: usize,
    /// 点集哈希：同 `seed` + 同场景必相同，换 seed 或换组数极大概率不同。
    pub points_digest: u64,
}

impl ScaleReport {
    /// 实际评到的组数。
    pub fn cases(&self) -> usize {
        self.samples.len()
    }

    /// 是否凑满了配置要求的组数（抽样有尝试上限，极端场景可能凑不满）。
    pub fn complete(&self) -> bool {
        self.samples.len() == self.scale.cases
    }

    /// 最大比值；没有样本时为 `0.0`（**不是**「完美通过」，看 [`ScaleReport::complete`]）。
    pub fn max_ratio(&self) -> f32 {
        self.samples.iter().map(|s| s.ratio).fold(0.0f32, f32::max)
    }

    /// 比值最大的那一组。
    pub fn worst(&self) -> Option<&ScaleSample> {
        self.samples
            .iter()
            .max_by(|a, b| a.ratio.total_cmp(&b.ratio))
    }

    /// 超限的组（`ratio > limit`）。空表即通过。
    pub fn exceeded(&self) -> Vec<&ScaleSample> {
        self.samples
            .iter()
            .filter(|s| !self.scale.allows(s.ratio))
            .collect()
    }

    /// 判据结论：凑满组数且无一超限。
    pub fn within(&self) -> bool {
        self.complete() && self.exceeded().is_empty()
    }

    /// 点集哈希的定宽十六进制（日志 / 报告用）。
    pub fn digest_hex(&self) -> String {
        format!("{:016x}", self.points_digest)
    }

    /// 一句话结论，形如 `20 组 / seed 42 / 1.6×：最大 1.12（第 7 组）`。
    pub fn summary(&self) -> String {
        let head = format!(
            "{} 组 / seed {} / {:.2}×",
            self.scale.cases, self.scale.seed, self.scale.limit
        );
        match self.worst() {
            Some(w) => format!(
                "{head}：最大 {:.2}（第 {} 组），实评 {} 组，跳过 {} 次，点集 {}",
                w.ratio,
                w.index,
                self.samples.len(),
                self.skipped,
                self.digest_hex()
            ),
            None => format!("{head}：一组都没评到（跳过 {} 次）", self.skipped),
        }
    }
}

/// 按配置跑一次判据。
///
/// 非法配置在**跑之前**就被拒（返回 [`ScaleError`]），不会消耗任何寻路。
pub fn measure(nav: &Nav, scale: &PathScale) -> Result<ScaleReport, ScaleError> {
    scale.validate()?;
    let cells = walkable_cells(nav);
    let min_straight = nav.report().cell_size * SCALE_MIN_STRAIGHT_CELLS;
    let mut rng = SplitMix64::new(scale.seed);
    let mut samples: Vec<ScaleSample> = Vec::with_capacity(scale.cases);
    let mut skipped = 0usize;
    let mut attempts = 0usize;
    // 尝试上限：可达格足够时一次就能凑满，卡在「几乎全不可达」的场景上不该死循环。
    let cap = scale.cases.saturating_mul(64).max(64);

    while samples.len() < scale.cases && attempts < cap && cells.len() >= 2 {
        attempts += 1;
        let a = cells[rng.below(cells.len())];
        let b = cells[rng.below(cells.len())];
        let from = nav.grid().cell_center(a[0], a[1]);
        let to = nav.grid().cell_center(b[0], b[1]);
        let straight = from.distance(to);
        if !(straight.is_finite() && straight >= min_straight) {
            skipped += 1;
            continue;
        }
        match nav.path(from, to) {
            PathOutcome::Found(points) => {
                let length = smooth_path(nav.grid(), &points).length;
                if !(length.is_finite() && length > 0.0) {
                    skipped += 1;
                    continue;
                }
                samples.push(ScaleSample {
                    index: samples.len(),
                    from,
                    to,
                    straight,
                    length,
                    ratio: length / straight,
                });
            }
            // 不可达 / 端点非法 / 算不完：都不产生「路径长度」，只记跳过。
            _ => skipped += 1,
        }
    }

    let cases: Vec<ScaleCase> = samples
        .iter()
        .map(|s| ScaleCase {
            from: s.from,
            to: s.to,
        })
        .collect();
    Ok(ScaleReport {
        scale: *scale,
        samples,
        skipped,
        points_digest: digest_cases(&cases),
    })
}

/// 可走格的行主序清单（抽样域）。顺序稳定，是确定性的第一环。
fn walkable_cells(nav: &Nav) -> Vec<[u32; 2]> {
    let [w, h] = nav.grid().dims();
    let mut out = Vec::with_capacity(nav.grid().walkable_count());
    for j in 0..h {
        for i in 0..w {
            if nav.grid().is_walkable_cell(i, j) {
                out.push([i, j]);
            }
        }
    }
    out
}

/// 点集哈希（FNV-1a 64，按 `f32` 的位模式喂）。只求「换 seed 就变」，不求密码学强度。
fn digest_cases(cases: &[ScaleCase]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for c in cases {
        for v in [c.from.x, c.from.y, c.from.z, c.to.x, c.to.y, c.to.z] {
            for b in v.to_bits().to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    h
}

/// splitmix64：状态只有 64 位、跨平台逐位相同，够用且不引依赖。
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}
