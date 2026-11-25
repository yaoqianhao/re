//! 导航产物（M2 · D01）：一份可哈希、可序列化、逐位比较的烘焙结果。
//!
//! ## 为什么要有这一层
//!
//! D01 的验收判据是「**双后端各烘一次，产物逐位相同**」。这条判据不能靠
//! 「两套烘焙代码分别写一遍，然后祈祷它们算出一样的数」——
//! 那是在赌两段浮点运算的次序恰好一致。所以口径是**构造性一致**：
//!
//! 1. 两个后端都只做一件事：把同一份 [`HeightField`](r3e_physics::HeightField)
//!    装上（`set_heightfield`）再原样取回；
//! 2. 真正决定网格长什么样的代码只有一份 [`bake_with_field`](crate::Nav::bake_with_field)，
//!    它只读那份高度场，**不再自己插值地形**；
//! 3. 于是「后端不同」这件事在烘焙里根本不可观测，产物自然逐位相同；
//! 4. [`NavProduct::product_hash`] 把整张网格折进一个 `u64`，
//!    [`NavProduct::to_bytes`] 把它摆成稳定的字节流，两者都是**回归锁**：
//!    将来任何人往烘焙里偷偷加一条后端相关的分支，哈希立刻变。
//!
//! ## 哈希不承诺什么
//!
//! 它**不是**密码学校验和（SplitMix64 不是抗碰撞的），用途只有两个：
//! 「同一次构建内两次烘焙是否一致」与「产物在版本间是否漂移」。
//! 不要拿它做安全判定。
//!
//! ## 输入摘要
//!
//! * `cfg_digest`：收敛后的 [`NavConfig`]，**不含**派生量（范围/格数由高度场决定）；
//! * `input_digest`：地形块（名字/原点/尺寸/分辨率/样本）+ 障碍盒 + 未解析资产名。
//!
//! 两者分开是为了排障：哈希对不上时，先看 `cfg_digest` 还是 `input_digest` 变了，
//! 就知道该怀疑「参数被谁动了」还是「场景内容被谁动了」。

use r3e_core::{Aabb, Scene};

use crate::{NavConfig, NavError};

/// 二进制产物的魔数（`R3NV`）。
const NAV_MAGIC: [u8; 4] = *b"R3NV";

/// 二进制产物的格式版本。改字段顺序/语义必须加一，否则老产物会被误读。
pub const NAV_FORMAT: u16 = 1;

/// 单格的分类结果：**只描述这一格能不能站、以及最陡的那条边是什么性质**。
///
/// 与 [`NavGrid::is_walkable_cell`](crate::NavGrid::is_walkable_cell) 的区别：
/// 可走位图是「静态阻挡」的结论（地形覆盖 + 障碍投影），本枚举在此之上
/// 用与 [`NavGrid::can_step`](crate::NavGrid::can_step) **同一个判定函数**
/// 量一遍最陡的相邻边。两边共用 `step_verdict`，所以不存在
/// 「分类说能走、寻路说迈不过去」的分裂。
///
/// 另有用途：坡度/台阶的分布是关卡体检指标（「这张图 30% 的格子是陡坡」），
/// 位图给不出这个信息，位图只有 0/1。
///
/// **口径（D01 锁死）**：`Blocked ⟺ !is_walkable_cell`。一条边陡到迈不过去
/// **不会**把这一格记成 `Blocked`——它站得住，只是那条边不通，那是
/// `can_step` 与连通块要表达的事。若两边都记，「站得住」就有两种互斥说法，
/// 直方图也从体检指标退化成噪声。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CellClass {
    /// 平坦可站：所有可通行正邻边的高差都小于 1 mm（平地容差）。
    Flat = 0,
    /// 可站但有连续斜面：至少一条边靠「坡度」判据才过，且高差超过平地容差。
    Slope = 1,
    /// 可站但是台阶：至少一条边靠「步高」判据才过（离散坎）。
    Step = 2,
    /// 不可站：地形不覆盖或被障碍投影封死（与可走位图逐格一致）。
    Blocked = 3,
}

impl CellClass {
    /// 全部取值（顺序即 `code()` 顺序），供直方图与测试遍历。
    pub const ALL: [CellClass; 4] = [
        CellClass::Flat,
        CellClass::Slope,
        CellClass::Step,
        CellClass::Blocked,
    ];

    /// 稳定的字节编码（进产物、进哈希）。**不要重排**——重排等于改产物格式。
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 由字节编码还原；未知编码返回 `None`（拒绝静默降级）。
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.code() == code)
    }

    /// 稳定名字（报告/日志/diff 用，不参与哈希）。
    pub fn name(self) -> &'static str {
        match self {
            CellClass::Flat => "flat",
            CellClass::Slope => "slope",
            CellClass::Step => "step",
            CellClass::Blocked => "blocked",
        }
    }

    /// 这一格是否站得住（`Blocked` 之外都站得住）。
    pub fn is_standable(self) -> bool {
        !matches!(self, CellClass::Blocked)
    }

    /// 严重度：合并多条边的分类时取最大值（陡坡压平坡、台阶压陡坡、阻挡压一切）。
    pub fn severity(self) -> u8 {
        self.code()
    }

    /// 取更「坏」的那个分类。
    pub(crate) fn worse(self, other: Self) -> Self {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }
}

/// SplitMix64 的一步混合。折哈希与 `product_hash` 共用。
pub(crate) fn mix64(z: u64) -> u64 {
    let mut x = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// 流式折叠：每喂一个值就混一次，最后 `finish()` 再混一次收尾。
///
/// 不是多项式滚动哈希（那样对尾部零比特敏感度会掉）。这里每次混合都经过
/// SplitMix64 的完整扰动，尾部零值也会改变状态。
#[derive(Debug, Clone, Copy)]
struct Fold(u64);

impl Fold {
    fn new() -> Self {
        // 初值取 π 的十六进制前缀：没有数学意义，只要不是 0/1 这种退化种子。
        Fold(0x243F_6A88_85A3_08D3)
    }

    fn u64(self, v: u64) -> Self {
        Fold(mix64(self.0 ^ mix64(v.wrapping_add(0x9E37_79B9_7F4A_7C15))))
    }

    fn u32(self, v: u32) -> Self {
        self.u64(u64::from(v))
    }

    fn f32(self, v: f32) -> Self {
        // 折比特而不是折值：`-0.0` 与 `0.0` 数值相等但比特不同，产物要求逐位一致，
        // 所以哈希也必须逐位敏感（否则「换了后端把 -0.0 变成 0.0」会被放过）。
        self.u32(v.to_bits())
    }

    fn bytes(self, b: &[u8]) -> Self {
        let mut acc = self;
        // 按 8 字节成块，尾部补齐块内有效长度，避免「ab」+「c」与「a」+「bc」撞一起。
        for chunk in b.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            acc = acc.u64(u64::from_le_bytes(word));
        }
        acc.u32(b.len() as u32)
    }

    fn finish(self) -> u64 {
        mix64(self.0 ^ 0xD6E8_FEB8_6659_FD93)
    }
}

/// 收敛后的烘焙参数摘要。与 [`NavConfig::sanitized`] 之后的字段一一对应。
pub(crate) fn cfg_digest(cfg: &NavConfig) -> u64 {
    let mut h = Fold::new();
    h = h.f32(cfg.cell_size);
    h = h.f32(cfg.agent_radius);
    h = h.f32(cfg.agent_height);
    h = h.f32(cfg.step_height);
    h = h.f32(cfg.max_slope_deg);
    match cfg.bounds {
        Some(b) => {
            h = h.u32(1);
            for v in b {
                h = h.f32(v);
            }
        }
        None => h = h.u32(0),
    }
    // 标签集合按字典序折，避免「声明顺序不同」被当成输入变化。
    let mut tags: Vec<&str> = cfg.ignore_tags.iter().map(String::as_str).collect();
    tags.sort_unstable();
    h = h.u32(tags.len() as u32);
    for t in tags {
        h = h.bytes(t.as_bytes());
    }
    h.finish()
}

/// 烘焙输入（场景内容）摘要：地形样本 + 障碍盒 + 未解析资产。
///
/// 障碍盒按 6 个坐标的比特排序后折入，所以「场景里节点的遍历顺序变了」不算输入变化，
/// 但「任何一个盒子的位置变了」算。
pub(crate) fn input_digest(scene: &Scene, obstacles: &[Aabb], unresolved: &[String]) -> u64 {
    let mut h = Fold::new();

    h = h.u32(scene.terrain.len() as u32);
    for patch in &scene.terrain {
        h = h.bytes(patch.name.as_bytes());
        h = h.f32(patch.origin[0]).f32(patch.origin[1]);
        h = h.f32(patch.size[0]).f32(patch.size[1]);
        h = h.u32(patch.resolution[0]).u32(patch.resolution[1]);
        h = h.u32(patch.heights.len() as u32);
        for &v in &patch.heights {
            h = h.f32(v);
        }
    }

    let mut boxes: Vec<[u32; 6]> = obstacles
        .iter()
        .map(|a| {
            [
                a.min.x.to_bits(),
                a.min.y.to_bits(),
                a.min.z.to_bits(),
                a.max.x.to_bits(),
                a.max.y.to_bits(),
                a.max.z.to_bits(),
            ]
        })
        .collect();
    boxes.sort_unstable();
    h = h.u32(boxes.len() as u32);
    for b in boxes {
        for v in b {
            h = h.u32(v);
        }
    }

    let mut names: Vec<&str> = unresolved.iter().map(String::as_str).collect();
    names.sort_unstable();
    h = h.u32(names.len() as u32);
    for n in names {
        h = h.bytes(n.as_bytes());
    }

    h.finish()
}

/// 一份烘焙产物：网格数据的**完整快照** + 两级输入摘要。
///
/// 所有字段都是已经定型的平面数组，不再持有场景引用——
/// 两后端产物逐位比较时比的就是这些数组。
#[derive(Debug, Clone, PartialEq)]
pub struct NavProduct {
    dims: [u32; 2],
    min: [f32; 2],
    cell_size: f32,
    heights: Vec<f32>,
    /// 可走位图，`cells.div_ceil(64)` 个 `u64` 字，与 [`NavGrid`](crate::NavGrid) 同构。
    walkable: Vec<u64>,
    /// 逐格分类（[`CellClass::code`]）。
    class: Vec<u8>,
    /// 逐格连通块 id（`0` = 不可走）。
    region: Vec<u32>,
    cfg_digest: u64,
    input_digest: u64,
}

impl NavProduct {
    /// crate 内部构造：长度由 [`NavGrid`](crate::NavGrid) 保证一致。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        dims: [u32; 2],
        min: [f32; 2],
        cell_size: f32,
        heights: Vec<f32>,
        walkable: Vec<u64>,
        class: Vec<u8>,
        region: Vec<u32>,
        cfg_digest: u64,
        input_digest: u64,
    ) -> Self {
        debug_assert_eq!(heights.len(), dims[0] as usize * dims[1] as usize);
        Self {
            dims,
            min,
            cell_size,
            heights,
            walkable,
            class,
            region,
            cfg_digest,
            input_digest,
        }
    }

    pub fn dims(&self) -> [u32; 2] {
        self.dims
    }

    pub fn min(&self) -> [f32; 2] {
        self.min
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// 每格地面高度（行主序 `j * dims[0] + i`）。
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// 逐格分类编码（[`CellClass::code`]）。
    pub fn classes(&self) -> &[u8] {
        &self.class
    }

    /// 逐格连通块 id。
    pub fn regions(&self) -> &[u32] {
        &self.region
    }

    /// 可走位图的原始字。
    pub fn walkable_words(&self) -> &[u64] {
        &self.walkable
    }

    /// 第 `idx` 格（行主序）的分类编码。
    pub fn class_code(&self, idx: usize) -> u8 {
        self.class[idx]
    }

    /// 每类格子的数量，下标 = [`CellClass::code`]。
    pub fn class_histogram(&self) -> [usize; 4] {
        let mut out = [0usize; 4];
        for &c in &self.class {
            out[c as usize] += 1;
        }
        out
    }

    /// 可走格数量（与位图 `count_ones` 口径一致）。
    pub fn walkable_cells(&self) -> usize {
        self.walkable.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn cfg_digest(&self) -> u64 {
        self.cfg_digest
    }

    pub fn input_digest(&self) -> u64 {
        self.input_digest
    }

    /// 整份产物的哈希：格式版本 + 两级输入摘要 + 全部网格数组的逐位折叠。
    pub fn product_hash(&self) -> u64 {
        let mut h = Fold::new();
        h = h.u32(u32::from(NAV_FORMAT));
        h = h.u64(self.cfg_digest).u64(self.input_digest);
        h = h.u32(self.dims[0]).u32(self.dims[1]);
        h = h.f32(self.min[0]).f32(self.min[1]).f32(self.cell_size);
        h = h.u32(self.heights.len() as u32);
        for &v in &self.heights {
            h = h.f32(v);
        }
        h = h.u32(self.walkable.len() as u32);
        for &w in &self.walkable {
            h = h.u64(w);
        }
        for &c in &self.class {
            h = h.u32(u32::from(c));
        }
        for &r in &self.region {
            h = h.u32(r);
        }
        h.finish()
    }

    /// 产物哈希的十六进制形式（16 位定长小写），报告与 diff 用。
    pub fn hash_hex(&self) -> String {
        format!("{:016x}", self.product_hash())
    }

    /// 稳定字节流。字段顺序即 [`NAV_FORMAT`] 的约定：
    /// 魔数 → 版本 → 维度/原点/格长 → 两级摘要 → 高度 → 分类 → 连通块 → 位图。
    pub fn to_bytes(&self) -> Vec<u8> {
        let cells = self.heights.len();
        let mut out = Vec::with_capacity(
            4 + 2 + 4 + 4 + 8 + 8 + 8 + 8 + cells * (4 + 1 + 4) + self.walkable.len() * 8,
        );
        out.extend_from_slice(&NAV_MAGIC);
        out.extend_from_slice(&NAV_FORMAT.to_le_bytes());
        put_u32(&mut out, self.dims[0]);
        put_u32(&mut out, self.dims[1]);
        for v in self.min {
            put_f32(&mut out, v);
        }
        put_f32(&mut out, self.cell_size);
        put_u64(&mut out, self.cfg_digest);
        put_u64(&mut out, self.input_digest);
        put_u32(&mut out, cells as u32);
        for &v in &self.heights {
            put_f32(&mut out, v);
        }
        for &c in &self.class {
            out.push(c);
        }
        for &r in &self.region {
            put_u32(&mut out, r);
        }
        put_u32(&mut out, self.walkable.len() as u32);
        for &w in &self.walkable {
            put_u64(&mut out, w);
        }
        out
    }

    /// 从 [`Self::to_bytes`] 的字节流还原。
    ///
    /// 任何一处对不上（魔数、版本、长度）都返回 `NavError::ProductMismatch` 而不是猜——
    /// 半懂不懂地读一半数据，比直接报错危险得多。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NavError> {
        let bad = |reason: &str| NavError::ProductMismatch {
            format: NAV_FORMAT,
            reason: reason.to_string(),
        };
        if bytes.len() < 4 + 2 {
            return Err(bad("字节流太短，连文件头都不完整"));
        }
        if bytes[0..4] != NAV_MAGIC {
            return Err(bad("魔数不匹配：这不是导航产物文件"));
        }
        let mut pos = 4;
        let version = get_u16(bytes, &mut pos).ok_or_else(|| bad("版本字段被截断"))?;
        if version != NAV_FORMAT {
            return Err(NavError::ProductMismatch {
                format: version,
                reason: format!("产物格式版本 {version} 与当前 {NAV_FORMAT} 不符"),
            });
        }
        let dims = [
            get_u32(bytes, &mut pos).ok_or_else(|| bad("维度字段被截断"))?,
            get_u32(bytes, &mut pos).ok_or_else(|| bad("维度字段被截断"))?,
        ];
        let min = [
            get_f32(bytes, &mut pos).ok_or_else(|| bad("原点字段被截断"))?,
            get_f32(bytes, &mut pos).ok_or_else(|| bad("原点字段被截断"))?,
        ];
        let cell_size = get_f32(bytes, &mut pos).ok_or_else(|| bad("格长字段被截断"))?;
        let cfg_digest = get_u64(bytes, &mut pos).ok_or_else(|| bad("参数摘要被截断"))?;
        let input_digest = get_u64(bytes, &mut pos).ok_or_else(|| bad("输入摘要被截断"))?;
        let cells = get_u32(bytes, &mut pos).ok_or_else(|| bad("格数字段被截断"))? as usize;
        if cells != dims[0] as usize * dims[1] as usize {
            return Err(bad("格数与维度不一致"));
        }
        let mut heights = Vec::with_capacity(cells);
        for _ in 0..cells {
            heights.push(get_f32(bytes, &mut pos).ok_or_else(|| bad("高度数组被截断"))?);
        }
        if pos + cells > bytes.len() {
            return Err(bad("分类数组被截断"));
        }
        let class = bytes[pos..pos + cells].to_vec();
        pos += cells;
        let mut region = Vec::with_capacity(cells);
        for _ in 0..cells {
            region.push(get_u32(bytes, &mut pos).ok_or_else(|| bad("连通块数组被截断"))?);
        }
        let words = get_u32(bytes, &mut pos).ok_or_else(|| bad("位图长度被截断"))? as usize;
        if words != cells.div_ceil(64) {
            return Err(bad("可走位图长度与格数不一致"));
        }
        let mut walkable = Vec::with_capacity(words);
        for _ in 0..words {
            walkable.push(get_u64(bytes, &mut pos).ok_or_else(|| bad("可走位图被截断"))?);
        }
        if pos != bytes.len() {
            return Err(bad("字节流尾部有多余数据"));
        }
        for &c in &class {
            if CellClass::from_code(c).is_none() {
                return Err(bad("出现未知的格子分类编码"));
            }
        }
        Ok(Self::new(
            dims,
            min,
            cell_size,
            heights,
            walkable,
            class,
            region,
            cfg_digest,
            input_digest,
        ))
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get_u16(b: &[u8], pos: &mut usize) -> Option<u16> {
    let end = pos.checked_add(2)?;
    let slice = b.get(*pos..end)?;
    *pos = end;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn get_u32(b: &[u8], pos: &mut usize) -> Option<u32> {
    let end = pos.checked_add(4)?;
    let slice = b.get(*pos..end)?;
    *pos = end;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn get_u64(b: &[u8], pos: &mut usize) -> Option<u64> {
    let end = pos.checked_add(8)?;
    let slice = b.get(*pos..end)?;
    *pos = end;
    let mut word = [0u8; 8];
    word.copy_from_slice(slice);
    Some(u64::from_le_bytes(word))
}

fn get_f32(b: &[u8], pos: &mut usize) -> Option<f32> {
    get_u32(b, pos).map(f32::from_bits)
}
