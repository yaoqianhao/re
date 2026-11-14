//! 动画混合树（扩展十八 A）：分层、骨骼遮罩与加法叠加。
//!
//! 现有 [`crate::anim::AnimInstance`] 一个实例只承载一条「主片段」，外加切片段时的
//! 交叉淡入。这足以表达 idle → run → atk 这类互斥动作，但表达不了「上半身攻击 +
//! 下半身走路」——后者要求**同时**播放多段动画并逐骨骼加权，也就是混合树。
//!
//! 本模块把它拆成纯数据 + 纯函数，不依赖场景与渲染：
//!
//! * [`BoneMask`]：按资产内节点路径（大小写不敏感、支持后缀片段）决定哪些骨骼参与本层；
//! * [`AnimLayer`]：一层 = 一组按权重混合的片段 + 层权重（可淡入淡出）+ 叠加模式；
//! * [`blend_override`] / [`apply_additive`]：把层姿态落到主姿态上的两种算子。
//!
//! 求值口径与 [`crate::anim`] 完全一致：位置/缩放线性插值、旋转球面插值。层内多片段
//! 默认共享同一条时间轴，不同层各有独立时间轴；需要「走跑同相位」时给层开
//! [`LayerSync::Phase`]（扩展四十六），各片段按归一化相位缩放采样。
//!
//! 双层叠加公式（`base` 为主姿态）：
//!
//! * override：`lerp(base, pose, w)`，w 是「层权重 × 遮罩权重」；
//! * additive：`base + (pose - reference) * w`，reference 取片段首帧，因此叠加的是
//!   「相对参考姿态的增量」而不是绝对姿态——呼吸、受击抖动这类动作都靠它。

use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::anim::AnimClip;
use crate::blend_space::BlendSpace;
use crate::Transform;

/// 单个实例最多挂载的层数。
pub const MAX_ANIM_LAYERS: usize = 8;

/// 单层内最多参与混合的片段数（走 / 跑 / 冲刺按速度加权已足够）。
pub const MAX_LAYER_CLIPS: usize = 8;

/// 空片段时长兜底，避免除零。
const MIN_LAYER_DURATION: f32 = 1e-4;

/// 骨骼遮罩：按节点路径做包含 / 排除过滤。
///
/// `include` 为空表示「默认全通过」，再由 `exclude` 剔除；两者都为空即全通过。
/// 规则只写路径的尾段即可（`"Arm/Hand"` 能命中 `RootNode/Arm/Hand`），与
/// [`AnimClip::track_of`] 的路径口径一致，导入 FBX 时不必抄全路径。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoneMask {
    /// 只允许这些路径（后缀匹配）；空 = 不限制。
    pub include: Vec<String>,
    /// 排除这些路径（优先于 `include`）。
    pub exclude: Vec<String>,
}

impl BoneMask {
    /// 全通过（等价于不设遮罩）。
    pub fn pass_all() -> Self {
        Self::default()
    }

    /// 只允许给定路径参与。
    pub fn only(paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            include: paths.into_iter().map(Into::into).collect(),
            exclude: Vec::new(),
        }
    }

    /// 排除给定路径。
    pub fn except(paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            include: Vec::new(),
            exclude: paths.into_iter().map(Into::into).collect(),
        }
    }

    /// 是否等于「不设遮罩」。
    pub fn is_pass_all(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// 该节点路径是否参与本层。
    pub fn contains(&self, node_path: &str) -> bool {
        if self
            .exclude
            .iter()
            .any(|rule| path_matches(rule, node_path))
        {
            return false;
        }
        if self.include.is_empty() {
            return true;
        }
        self.include
            .iter()
            .any(|rule| path_matches(rule, node_path))
    }

    /// 该节点路径的遮罩权重：通过为 1，否则 0（保留浮点口径便于上层直接相乘）。
    pub fn weight_for(&self, node_path: &str) -> f32 {
        if self.contains(node_path) {
            1.0
        } else {
            0.0
        }
    }

    /// 规则条数（日志用）。
    pub fn rule_count(&self) -> usize {
        self.include.len() + self.exclude.len()
    }
}

/// 规则是否命中节点路径：整串相等，或规则恰是路径的末尾若干段。
///
/// 只用 `/` 作为分隔，且要求末尾段**逐段**相等——避免 `"Arm"` 误命中 `"Armature"`。
pub fn path_matches(rule: &str, node_path: &str) -> bool {
    let rule = rule.trim_matches('/');
    if rule.is_empty() {
        return false;
    }
    let path = node_path.trim_matches('/');
    if path.eq_ignore_ascii_case(rule) {
        return true;
    }
    let mut r = rule.rsplit('/');
    let mut p = path.rsplit('/');
    loop {
        match r.next() {
            None => return true,
            Some(rs) => match p.next() {
                Some(ps) if ps.eq_ignore_ascii_case(rs) => continue,
                _ => return false,
            },
        }
    }
}

/// 层的叠加模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerMode {
    /// 覆盖：层姿态按权重插值替换主姿态（默认，适合上半身遮罩层）。
    #[default]
    Override,
    /// 加法：把「相对参考姿态的增量」叠加到主姿态上（呼吸、受击抖动）。
    Additive,
}

impl LayerMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Additive => "additive",
        }
    }

    /// 解析脚本 / RPC 传来的模式名（大小写不敏感）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            x if x.eq_ignore_ascii_case("override") => Some(Self::Override),
            x if x.eq_ignore_ascii_case("additive") => Some(Self::Additive),
            _ => None,
        }
    }
}

/// 层内采样时间策略（扩展四十六）。
///
/// 走（1.0 s）与跑（0.5 s）同时参与混合时，若两路共用一个播放时间，跑片段会在 0.5 s 处
/// 到达末帧并停住，而走片段还在推进——同一时刻两段动作的**步态相位**不一致，脚掌落地节奏
/// 错开，画面上就是「滑步」。相位锁把层时间归一化成一个 0..1 的相位，每路片段再按自己的
/// 时长把相位还原成片段内时间，于是「同一相位 = 同一步态阶段」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerSync {
    /// 不锁相位（默认）：整层共用一个片段内时间，短片段到末帧后由轨道端点钳制停住。
    #[default]
    Off,
    /// 相位锁：按层时间归一化相位缩放各片段，步频差异保留、相位差恒为 0。
    Phase,
}

impl LayerSync {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Phase => "phase",
        }
    }

    /// 解析脚本 / RPC 传来的策略名（大小写不敏感）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            x if x.eq_ignore_ascii_case("off") || x.eq_ignore_ascii_case("none") => Some(Self::Off),
            x if x.eq_ignore_ascii_case("phase") || x.eq_ignore_ascii_case("sync") => {
                Some(Self::Phase)
            }
            _ => None,
        }
    }
}

/// 单路片段的播放参数（扩展四十八）：片段自带速率与循环标志。
///
/// 层级的 [`AnimLayer::speed`] / [`AnimLayer::looping`] 是「整层共用一条时间轴」的口径，
/// 一个层内几路片段长短不一时只能折中。这里把速率与循环下放到**每一路片段**上：
/// 呼吸（慢）与摇头（快）同处一层时，各自按自己的速率走，互不牵制。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipPlayback {
    /// 播放速率倍数：`1.0` 原速、`2.0` 快一倍、负值倒放、`0.0` 定格首帧。
    pub speed: f32,
    /// 到末帧后是否绕回开头（`false` = 停在末帧）。
    pub looping: bool,
}

impl ClipPlayback {
    /// 构造播放参数；非有限速率按 1.0（原速）处理，避免把 NaN 传进时间轴。
    pub fn new(speed: f32, looping: bool) -> Self {
        Self {
            speed: sanitize_speed(speed),
            looping,
        }
    }

    /// 本路在层时间 `time` 处应采样的片段内时间。
    ///
    /// 采样口径与 [`crate::anim::wrap_time`] 完全一致（循环取模 / 非循环钳到末帧），
    /// 只是先把层时间按 `speed` 缩放——因此负速率能自然倒放（`rem_euclid` 保证落在 `[0, d)`）。
    pub fn time_at(&self, time: f32, duration: f32) -> f32 {
        // 定格：速率 0 恒采首帧（等价于把时间轴钉在 0，而不是「按 0 缩放得 0」的巧合）
        let scaled = if self.speed == 0.0 {
            0.0
        } else {
            time * self.speed
        };
        if !scaled.is_finite() {
            return 0.0;
        }
        crate::anim::wrap_time(scaled, self.looping, duration)
    }

    /// 日志 / 探针用的紧凑写法：`1.50` 或 `2.00:once`（循环是常态，不额外标注）。
    pub fn brief(&self) -> String {
        if self.looping {
            format!("{:.2}", self.speed)
        } else {
            format!("{:.2}:once", self.speed)
        }
    }
}

/// 速率收敛：非有限值按 1.0（原速），其余原样保留（负值与 0 都有语义）。
fn sanitize_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed
    } else {
        1.0
    }
}

/// 层内的一路片段及其权重。
#[derive(Debug, Clone)]
pub struct LayerClip {
    pub clip: Arc<AnimClip>,
    /// 混合权重（负值按 0 处理；层内按非零和归一化后使用）。
    pub weight: f32,
    /// 相位偏移（扩展四十七）：相位锁下这一路的采样相位 = `wrap(层相位 + phase_offset)`。
    ///
    /// 0.0 时与扩展四十七之前**逐位一致**；0.5 即「同一时刻反着走半步」，用来做左右脚错步。
    pub phase_offset: f32,
    /// 本路自己的播放速率与循环标志（扩展四十八）。
    ///
    /// 为 `None`（默认）时这一路**原样使用层时间**，与扩展四十八之前**逐位一致**——
    /// 这是本扩展「零变化」验收的支点。只在 [`LayerSync::Off`] 下生效：相位锁层的
    /// 每路时间由相位决定（要变速请调层 [`AnimLayer::speed`]），否则相位对齐会被速率破坏。
    pub playback: Option<ClipPlayback>,
}

impl LayerClip {
    pub fn new(clip: Arc<AnimClip>, weight: f32) -> Self {
        Self {
            clip,
            weight: if weight.is_finite() {
                weight.max(0.0)
            } else {
                0.0
            },
            phase_offset: 0.0,
            playback: None,
        }
    }

    pub fn with_phase_offset(mut self, offset: f32) -> Self {
        self.phase_offset = wrap_phase(offset);
        self
    }

    /// 设本路自己的播放速率与循环标志（扩展四十八）。链式构造用。
    pub fn with_playback(mut self, speed: f32, looping: bool) -> Self {
        self.playback = Some(ClipPlayback::new(speed, looping));
        self
    }

    /// 设本路播放参数（非有限速率按原速）。
    pub fn set_playback(&mut self, speed: f32, looping: bool) {
        self.playback = Some(ClipPlayback::new(speed, looping));
    }

    /// 清除本路播放参数：这一路回到「跟着层时间轴走」的老口径。
    pub fn clear_playback(&mut self) {
        self.playback = None;
    }

    /// 本路在层时间 `time` 处应采样的片段内时间（扩展四十八）。
    ///
    /// **没设播放参数时原样返回 `time`**（不乘 1.0、不取模），这就是「不设 = 老行为」的
    /// 算术支点：未调新 API 的场景逐位不变。
    pub fn playback_time(&self, time: f32) -> f32 {
        match self.playback {
            Some(pb) => pb.time_at(time, self.clip.duration.max(0.0)),
            None => time,
        }
    }

    /// 设本路相位偏移：非有限值按 0，其余折进 `[0, 1)`（-0.25 等价 0.75）。
    pub fn set_phase_offset(&mut self, offset: f32) {
        self.phase_offset = wrap_phase(offset);
    }

    /// 本路在给定层相位下应采样的片段内时间（含相位偏移）。
    pub fn synced_time(&self, phase: f32) -> f32 {
        clip_phase(phase, self.phase_offset) * self.clip.duration.max(0.0)
    }
}

/// 层权重的过渡状态（淡入 / 淡出）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerFade {
    pub from: f32,
    pub to: f32,
    pub elapsed: f32,
    pub duration: f32,
}

impl LayerFade {
    /// 构造过渡；`duration <= 0` 返回 `None`（调用方直接落值）。
    pub fn new(from: f32, to: f32, duration: f32) -> Option<Self> {
        if duration <= 0.0 {
            return None;
        }
        Some(Self {
            from: from.clamp(0.0, 1.0),
            to: to.clamp(0.0, 1.0),
            elapsed: 0.0,
            duration,
        })
    }

    /// 当前有效权重（smoothstep 缓入缓出，与交叉淡入口径一致）。
    pub fn weight(&self) -> f32 {
        let t = (self.elapsed / self.duration.max(1e-6)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        (self.from + (self.to - self.from) * s).clamp(0.0, 1.0)
    }

    /// 推进过渡；返回是否已走完。
    pub fn advance(&mut self, dt: f32) -> bool {
        self.elapsed += dt.max(0.0);
        self.elapsed >= self.duration
    }
}

/// 混合树中的一层。
///
/// 层之间按挂载顺序依次作用：先来的层先落到主姿态上，后来的层在其结果上再叠加，
/// 因此「下半身 override + 上半身 override」不会互相覆盖（遮罩已把骨骼切开了）。
#[derive(Debug, Clone)]
pub struct AnimLayer {
    /// 层名，脚本按它定位。
    pub name: String,
    /// 参与混合的片段（权重归一后加权平均）。
    pub clips: Vec<LayerClip>,
    /// 层权重（0 = 完全不生效）。
    pub weight: f32,
    pub mode: LayerMode,
    pub mask: BoneMask,
    /// 层自己的播放时间（秒，未取模）。
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    pub playing: bool,
    /// 权重过渡（`None` = 静止在当前权重）。
    pub fade: Option<LayerFade>,
    /// 参数驱动的权重（扩展四十五）：有空间时层内片段权重由它算出，手写权重被覆盖。
    ///
    /// 为 `None` 时行为与扩展四十五之前**逐位一致**——这是本扩展「零变化」验收的支点。
    pub blend_space: Option<BlendSpace>,
    /// 层内采样时间策略（扩展四十六）。
    ///
    /// 为 [`LayerSync::Off`] 时层内片段共用一个时间，与扩展四十六之前**逐位一致**。
    pub sync: LayerSync,
    /// 相位同步的源层名（扩展四十七）：`Some` 时本层相位改为跟随该层的相位。
    ///
    /// 只在 [`LayerSync::Phase`] 下生效；为 `None`（默认）时相位取自本层时间，
    /// 与扩展四十七之前**逐位一致**。名字解析失败的层按「没有源层」处理。
    pub sync_source: Option<String>,
}

impl AnimLayer {
    /// 以单片段建层（最常见的入口）。
    pub fn new(name: impl Into<String>, clip: Arc<AnimClip>, weight: f32) -> Self {
        Self {
            name: name.into(),
            clips: vec![LayerClip::new(clip, 1.0)],
            weight: if weight.is_finite() {
                weight.clamp(0.0, 1.0)
            } else {
                0.0
            },
            mode: LayerMode::Override,
            mask: BoneMask::pass_all(),
            time: 0.0,
            speed: 1.0,
            looping: true,
            playing: true,
            fade: None,
            blend_space: None,
            sync: LayerSync::Off,
            sync_source: None,
        }
    }

    pub fn with_mode(mut self, mode: LayerMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_mask(mut self, mask: BoneMask) -> Self {
        self.mask = mask;
        self
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_blend_space(mut self, space: BlendSpace) -> Self {
        self.blend_space = Some(space);
        self
    }

    pub fn with_sync(mut self, sync: LayerSync) -> Self {
        self.sync = sync;
        self
    }

    /// 让本层相位跟随 `source` 层（扩展四十七，仅 `Phase` 下生效；`None` = 跟随自己）。
    pub fn with_sync_source(mut self, source: Option<impl Into<String>>) -> Self {
        self.sync_source = source.map(Into::into);
        self
    }

    /// 设 / 清相位源层；空串等价清空。不做存在性校验（那需要全层视角，见 [`phase_source`]）。
    pub fn set_sync_source(&mut self, source: Option<&str>) {
        self.sync_source = source
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }

    /// 按片段名设相位偏移（扩展四十七）；`clip` 为空取第一路，名字大小写不敏感。
    ///
    /// 片段不存在返回 `false`，层状态不变。
    pub fn set_clip_phase_offset(&mut self, clip: &str, offset: f32) -> bool {
        let index = if clip.is_empty() {
            if self.clips.is_empty() {
                return false;
            }
            0
        } else {
            match self
                .clips
                .iter()
                .position(|c| c.clip.name == clip || c.clip.name.eq_ignore_ascii_case(clip))
            {
                Some(i) => i,
                None => return false,
            }
        };
        self.clips[index].set_phase_offset(offset);
        true
    }

    /// 按片段名设本路播放参数（扩展四十八）；`clip` 为空取第一路，名字大小写不敏感。
    ///
    /// 片段不存在返回 `false`，层状态不变。
    pub fn set_clip_playback(&mut self, clip: &str, speed: f32, looping: bool) -> bool {
        match self.clip_index_by_name(clip) {
            Some(i) => {
                self.clips[i].set_playback(speed, looping);
                true
            }
            None => false,
        }
    }

    /// 按片段名清除本路播放参数（回到跟着层时间轴走）。
    pub fn clear_clip_playback(&mut self, clip: &str) -> bool {
        match self.clip_index_by_name(clip) {
            Some(i) => {
                self.clips[i].clear_playback();
                true
            }
            None => false,
        }
    }

    /// 层内片段下标：空名取第一路，名字大小写不敏感，找不到返回 `None`。
    fn clip_index_by_name(&self, clip: &str) -> Option<usize> {
        if clip.is_empty() {
            if self.clips.is_empty() {
                return None;
            }
            return Some(0);
        }
        self.clips
            .iter()
            .position(|c| c.clip.name == clip || c.clip.name.eq_ignore_ascii_case(clip))
    }

    /// 把混合空间当前参数算出来的权重写进层内片段（扩展四十五）。
    ///
    /// 返回是否真的改过权重：
    /// - 没有混合空间 → `false`（层保持手写权重）；
    /// - 空间没有样本 → `false`（刚建空间还没加样本，此时不该把层清成静音）；
    /// - 有空间且有权重 → 先清零再按名字写，返回 `true`。
    ///
    /// 「先清零」是有意为之：参数从 1.0 走到 2.0 时，原来 1.0 那一档的片段必须真的掉到 0，
    /// 否则权重会残留成「越走越多路」。空间里出现、层里没有的片段名直接忽略
    /// （它的权重份额会被 `normalize_weights` 重新分配，不会让整层整体变暗）。
    pub fn apply_blend_space(&mut self) -> bool {
        let Some(space) = self.blend_space.as_ref() else {
            return false;
        };
        let weights = space.weights();
        if weights.is_empty() {
            return false;
        }
        for layer_clip in &mut self.clips {
            layer_clip.weight = 0.0;
        }
        for (name, weight) in weights {
            if let Some(layer_clip) = self
                .clips
                .iter_mut()
                .find(|c| c.clip.name.eq_ignore_ascii_case(&name))
            {
                layer_clip.weight = weight;
            }
        }
        true
    }

    /// 层内片段名（日志用）。
    pub fn clip_names(&self) -> Vec<String> {
        self.clips.iter().map(|c| c.clip.name.clone()).collect()
    }

    /// 层时长：层内片段时长的最大值（层时间按它取模）。
    pub fn duration(&self) -> f32 {
        self.clips
            .iter()
            .map(|c| c.clip.duration)
            .fold(0.0_f32, f32::max)
    }

    /// 当前有效权重（含过渡）。
    pub fn effective_weight(&self) -> f32 {
        match self.fade {
            Some(f) => f.weight(),
            None => self.weight.clamp(0.0, 1.0),
        }
    }

    /// 直接设定权重并取消进行中的过渡。
    pub fn set_weight(&mut self, weight: f32) {
        self.weight = if weight.is_finite() {
            weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.fade = None;
    }

    /// 从当前有效权重过渡到 `weight`（`duration <= 0` 等价 [`Self::set_weight`]）。
    pub fn fade_to(&mut self, weight: f32, duration: f32) {
        let to = if weight.is_finite() {
            weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match LayerFade::new(self.effective_weight(), to, duration) {
            Some(fade) => {
                self.fade = Some(fade);
                self.weight = to;
            }
            None => self.set_weight(to),
        }
    }

    /// 加入一路片段（同名替换权重）；超出 [`MAX_LAYER_CLIPS`] 返回 `false`。
    pub fn add_clip(&mut self, clip: Arc<AnimClip>, weight: f32) -> bool {
        if let Some(existing) = self
            .clips
            .iter_mut()
            .find(|c| c.clip.name.eq_ignore_ascii_case(&clip.name))
        {
            existing.weight = if weight.is_finite() {
                weight.max(0.0)
            } else {
                0.0
            };
            existing.clip = clip;
            return true;
        }
        if self.clips.len() >= MAX_LAYER_CLIPS {
            return false;
        }
        self.clips.push(LayerClip::new(clip, weight));
        true
    }

    /// 按片段名（大小写不敏感）移除一路；至少保留一路，移除最后一路返回 `false`。
    pub fn remove_clip(&mut self, clip_name: &str) -> bool {
        if self.clips.len() <= 1 {
            return false;
        }
        let before = self.clips.len();
        self.clips
            .retain(|c| !c.clip.name.eq_ignore_ascii_case(clip_name));
        self.clips.len() != before
    }

    /// 调整某一路片段权重。
    pub fn set_clip_weight(&mut self, clip_name: &str, weight: f32) -> bool {
        match self
            .clips
            .iter_mut()
            .find(|c| c.clip.name.eq_ignore_ascii_case(clip_name))
        {
            Some(c) => {
                c.weight = if weight.is_finite() {
                    weight.max(0.0)
                } else {
                    0.0
                };
                true
            }
            None => false,
        }
    }

    /// 层内权重归一化后的数组（顺序与 `clips` 一致，全零时为空）。
    pub fn normalized_weights(&self) -> Vec<f32> {
        normalize_weights(&self.clips.iter().map(|c| c.weight).collect::<Vec<_>>())
    }

    /// 推进层时间与权重过渡。
    pub fn advance(&mut self, dt: f32) {
        if self.playing && dt != 0.0 {
            self.time += dt * self.speed;
            if !self.looping {
                let duration = self.duration().max(MIN_LAYER_DURATION);
                if self.time >= duration {
                    self.time = duration;
                } else if self.time <= 0.0 && self.speed < 0.0 {
                    self.time = 0.0;
                }
            }
        }

        let finished = match self.fade.as_mut() {
            Some(fade) => {
                let done = fade.advance(dt);
                done.then_some(fade.to)
            }
            None => None,
        };
        if let Some(to) = finished {
            self.weight = to;
            self.fade = None;
        }
    }

    /// 当前应采样的片段内时间。
    pub fn sample_time(&self) -> f32 {
        crate::anim::wrap_time(self.time, self.looping, self.duration())
    }

    /// 层时间归一化后的相位（0..1，扩展四十六）。
    ///
    /// 归一化基准是层时长（层内最长片段），与 [`Self::sample_time`] 同一口径，因此
    /// `phase() * duration() == sample_time()` 恒成立；这也意味着相位**不随层内权重变化**
    /// 而跳变——走跑权重来回摆时不会突然错拍。
    pub fn phase(&self) -> f32 {
        let t = self.sample_time();
        if !t.is_finite() {
            return 0.0;
        }
        let duration = self.duration().max(MIN_LAYER_DURATION);
        (t / duration).clamp(0.0, 1.0)
    }

    /// 第 `index` 路片段在相位锁下应采样到的片段内时间（含该路的相位偏移）。
    ///
    /// 层里没有这一路返回 `None`；片段时长为 0 时返回 0（该路本来就采不出姿态）。
    /// 偏移为 0 时与扩展四十六逐位一致（层相位是 1.0 就采末帧，不折回首帧）。
    pub fn synced_clip_time(&self, index: usize, phase: f32) -> Option<f32> {
        self.clips.get(index).map(|clip| clip.synced_time(phase))
    }

    /// 每路片段当前的采样时间（按 `sync` 策略算，探针 / 回显用）。
    ///
    /// `Off` 时返回的就是老口径：整层同一个时间。
    pub fn clip_times(&self) -> Vec<f32> {
        self.clip_times_at(self.phase())
    }

    /// 在给定层相位下各路片段的采样时间（扩展四十七：允许外部传有效相位）。
    ///
    /// 扩展四十八：`Off` 口径下每路可按自己的速率/循环折算时；没设播放参数的那一路
    /// 拿到的仍是同一个层时间，逐位不变。
    pub fn clip_times_at(&self, phase: f32) -> Vec<f32> {
        match self.sync {
            LayerSync::Off => {
                let time = self.sample_time();
                self.clips.iter().map(|c| c.playback_time(time)).collect()
            }
            LayerSync::Phase => self.clips.iter().map(|c| c.synced_time(phase)).collect(),
        }
    }
}

/// 相位收敛：非有限值按 0，其余钳到 `[0, 1]`。
fn normalized_phase(phase: f32) -> f32 {
    if phase.is_finite() {
        phase.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 相位折进 `[0, 1)`：非有限值按 0，负偏移按循环等价（-0.25 → 0.75，1.0 → 0.0）。
///
/// 只作用于**叠加偏移之后**的相位；层相位本身仍走 [`normalized_phase`]，
/// 这样 1.0 这个「末帧端点」不会因为取模退回首帧，偏移为 0 的场景逐位不变。
pub fn wrap_phase(phase: f32) -> f32 {
    if !phase.is_finite() {
        return 0.0;
    }
    let wrapped = phase - phase.floor();
    if wrapped >= 1.0 {
        0.0
    } else {
        wrapped
    }
}

/// 某一路片段的采样相位：层相位钳到 `[0, 1]` 后叠加本路偏移再折进 `[0, 1)`。
///
/// 偏移为 0 时**原样返回**钳制结果（含 1.0 端点），这是「不设偏移 = 老行为」的算术支点。
fn clip_phase(phase: f32, offset: f32) -> f32 {
    let base = normalized_phase(phase);
    if offset == 0.0 {
        base
    } else {
        wrap_phase(base + offset)
    }
}

/// 找一层的相位源层下标（扩展四十七）：沿 `sync_source` 链走，最多
/// [`MAX_ANIM_LAYERS`] 跳。
///
/// 返回 `None` 表示「没有可跟随的源层」（没设、名字解析不到、指向自己、成环）；
/// 此时相位仍取本层时间，行为与扩展四十六一致。链式跟随是有意支持的
/// （上半身跟下半身、下半身再跟根层），但**环**必须在这里断掉，否则采样期会自引用死循环。
pub fn phase_source(layers: &[AnimLayer], index: usize) -> Option<usize> {
    let mut current = index;
    for _ in 0..MAX_ANIM_LAYERS {
        let name = layers.get(current)?.sync_source.as_deref()?;
        let next = layers
            .iter()
            .position(|l| l.name == name || l.name.eq_ignore_ascii_case(name))?;
        if next == index {
            // 直接或间接回到起点：当成没有源层
            return None;
        }
        if layers[next].sync_source.is_none() {
            return Some(next);
        }
        current = next;
    }
    None
}

/// 层的有效相位（扩展四十七）：`Phase` 下若挂了可用源层，取源层相位，否则取本层相位。
///
/// `Off` 的层直接返回本层相位（`Off` 采样不看相位，取源层只会在探针里造成误读）。
pub fn effective_phase(layers: &[AnimLayer], index: usize) -> f32 {
    let Some(layer) = layers.get(index) else {
        return 0.0;
    };
    if layer.sync == LayerSync::Off {
        return layer.phase();
    }
    match phase_source(layers, index).and_then(|i| layers.get(i)) {
        Some(source) => source.phase(),
        None => layer.phase(),
    }
}

/// 权重归一化：负值 / NaN 当 0，全零时返回空数组（调用方据此跳过整层）。
pub fn normalize_weights(raw: &[f32]) -> Vec<f32> {
    let cleaned: Vec<f32> = raw
        .iter()
        .map(|w| if w.is_finite() { w.max(0.0) } else { 0.0 })
        .collect();
    let sum: f32 = cleaned.iter().sum();
    if sum <= 1e-6 {
        return Vec::new();
    }
    cleaned.iter().map(|w| w / sum).collect()
}

/// 一层的采样结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSample {
    /// 本帧姿态。
    pub pose: Transform,
    /// 加法叠加用的参考姿态（片段首帧按同权重混合）。
    pub reference: Transform,
}

/// 采样一层在 `time` 处、某个节点上的姿态。
///
/// 层内任一片段没有该节点轨道时跳过那一路（剩余权重重新归一），全部缺失返回 `None`，
/// 调用方保留主姿态不动。
///
/// 扩展四十八：`time` 是**层时间**，每路片段再按自己的播放参数折算（没设参数的
/// 那一路原样使用 `time`，与扩展四十八之前逐位一致）。
pub fn sample_layer_pose(
    clips: &[LayerClip],
    time: f32,
    node_index: u32,
    node_path: &str,
) -> Option<LayerSample> {
    sample_layer_pose_impl(clips, node_index, node_path, |lc| lc.playback_time(time))
}

/// 相位锁采样（扩展四十六）：把归一化相位按各片段自己的时长还原成片段内时间。
///
/// 与 [`sample_layer_pose`] 的唯一差别是「用哪个时间采样每路片段」：本函数下
/// 片段内时间 = `phase × 片段时长`，因此走（1.0 s）与跑（0.5 s）在同一相位永远采到
/// 同一段步态（脚掌同时落地），而步频差异照旧保留。
pub fn sample_layer_pose_synced(
    clips: &[LayerClip],
    phase: f32,
    node_index: u32,
    node_path: &str,
) -> Option<LayerSample> {
    sample_layer_pose_impl(clips, node_index, node_path, |lc| lc.synced_time(phase))
}

/// 两种采样口径共用的一份实现：`time_of` 决定每路片段用哪个时间采样。
fn sample_layer_pose_impl(
    clips: &[LayerClip],
    node_index: u32,
    node_path: &str,
    mut time_of: impl FnMut(&LayerClip) -> f32,
) -> Option<LayerSample> {
    let mut acc: Option<(Transform, Transform, f32)> = None;
    for lc in clips {
        let weight = if lc.weight.is_finite() {
            lc.weight.max(0.0)
        } else {
            0.0
        };
        if weight <= 0.0 {
            continue;
        }
        let Some(track) = lc
            .clip
            .track_of(node_index, node_path)
            .and_then(|idx| lc.clip.tracks.get(idx as usize))
        else {
            continue;
        };
        let Some(pose) = track.sample(time_of(lc)) else {
            continue;
        };
        // 参考姿态取首帧：加法层叠加的是「相对参考的增量」
        let reference = track.keys.first().map(|k| k.transform()).unwrap_or(pose);
        acc = Some(match acc {
            None => (pose, reference, weight),
            Some((pose_acc, ref_acc, weight_acc)) => {
                let total = weight_acc + weight;
                let t = weight / total;
                (
                    blend_override(&pose_acc, &pose, t),
                    blend_override(&ref_acc, &reference, t),
                    total,
                )
            }
        });
    }
    acc.map(|(pose, reference, _)| LayerSample { pose, reference })
}

/// 加法层的采样（扩展五十）：把层内份额当「逐路增量系数」，而不是「姿态混合权重」。
///
/// 覆盖层多路混合是加权平均——份额 0.6 / 0.4 就该得到"六成 A 四成 B 的那个姿态"。
/// 加法层叠加的是**增量**（`片段姿态 - 该片段首帧`），多路份额是"各增量各占几成"，
/// 所以 `0.5·左倾 + 0.5·右倾` 的正确结果是**两次增量相互抵消**，而不是"一半幅度的左倾"。
/// 旧路径 [`sample_layer_pose`] 把两条姿态各自按份额平均后再求差：位置项因线性恰好等价，
/// 旋转项（四元数平均）与缩放项（比值平均）都会被平均钝化，故加法层单独走这份实现。
///
/// 合成口径：以**第一路有效片段的参考姿态**为零增量基准，把每路的增量按归一化份额
/// 作用到基准上（位置线性累加、旋转用 `slerp(identity, delta_i, k_i)` 连乘、缩放用比值增量连乘），
/// 返回「合成后的姿态 + 基准参考」，调用方照旧 [`apply_additive`] 叠一次层权重。
///
/// 单路有效片段时**原样返回那一路**（不做任何算术），与扩展四十九之前逐位一致。
pub fn sample_layer_pose_additive(
    clips: &[LayerClip],
    time: f32,
    node_index: u32,
    node_path: &str,
) -> Option<LayerSample> {
    sample_layer_pose_additive_impl(clips, node_index, node_path, |lc| lc.playback_time(time))
}

/// 相位锁版本，见 [`sample_layer_pose_additive`] / [`sample_layer_pose_synced`]。
pub fn sample_layer_pose_additive_synced(
    clips: &[LayerClip],
    phase: f32,
    node_index: u32,
    node_path: &str,
) -> Option<LayerSample> {
    sample_layer_pose_additive_impl(clips, node_index, node_path, |lc| lc.synced_time(phase))
}

/// 加法层采样共用实现：`time_of` 决定每路片段用哪个时间采样。
fn sample_layer_pose_additive_impl(
    clips: &[LayerClip],
    node_index: u32,
    node_path: &str,
    mut time_of: impl FnMut(&LayerClip) -> f32,
) -> Option<LayerSample> {
    let mut parts: Vec<(Transform, Transform, f32)> = Vec::new();
    let mut total = 0.0f32;
    for lc in clips {
        let weight = if lc.weight.is_finite() {
            lc.weight.max(0.0)
        } else {
            0.0
        };
        if weight <= 0.0 {
            continue;
        }
        let Some(track) = lc
            .clip
            .track_of(node_index, node_path)
            .and_then(|idx| lc.clip.tracks.get(idx as usize))
        else {
            continue;
        };
        let Some(pose) = track.sample(time_of(lc)) else {
            continue;
        };
        // 参考姿态取首帧，与覆盖路径同口径：加法层叠加的是「相对首帧的增量」
        let reference = track.keys.first().map(|k| k.transform()).unwrap_or(pose);
        parts.push((pose, reference, weight));
        total += weight;
    }

    match parts.len() {
        0 => None,
        // 单路：原样返回，零算术 —— 单路口加法层与扩展四十九之前逐位一致
        1 => Some(LayerSample {
            pose: parts[0].0,
            reference: parts[0].1,
        }),
        _ => {
            let reference = parts[0].1;
            // 从「零增量」姿态起步（= 第一路的参考），再逐路把增量按份额叠上去
            let mut pose = reference;
            for (part_pose, part_ref, weight) in &parts {
                let k = weight / total;
                pose.position += (part_pose.position - part_ref.position) * k;

                let delta = part_pose.rotation * part_ref.rotation.inverse();
                pose.rotation = (pose.rotation * Quat::slerp(Quat::IDENTITY, delta, k)).normalize();

                let ratio = Vec3::new(
                    scale_ratio(part_pose.scale.x, part_ref.scale.x),
                    scale_ratio(part_pose.scale.y, part_ref.scale.y),
                    scale_ratio(part_pose.scale.z, part_ref.scale.z),
                );
                pose.scale *= Vec3::ONE.lerp(ratio, k);
            }
            Some(LayerSample { pose, reference })
        }
    }
}

/// 覆盖式混合：`w = 0` 全取 `base`，`w = 1` 全取 `top`。
///
/// 与 [`crate::anim::blend_transform`] 同口径（旋转走 slerp，避免 180° 附近塌缩）。
pub fn blend_override(base: &Transform, top: &Transform, w: f32) -> Transform {
    crate::anim::blend_transform(base, top, w)
}

/// 加法式混合：把 `pose` 相对 `reference` 的增量按 `w` 叠加到 `base`。
///
/// 旋转用四元数增量（`pose * reference⁻¹`）再以 slerp 缩放，因此叠加结果始终是合法的
/// 单位四元数；缩放用比值增量，参考缩放接近 0 的分量按 1 处理，避免除零放大。
pub fn apply_additive(
    base: &Transform,
    reference: &Transform,
    pose: &Transform,
    w: f32,
) -> Transform {
    let w = if w.is_finite() {
        w.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if w <= 0.0 {
        return *base;
    }

    let position = base.position + (pose.position - reference.position) * w;

    let delta = pose.rotation * reference.rotation.inverse();
    let rotation = (base.rotation * Quat::slerp(Quat::IDENTITY, delta, w)).normalize();

    let ratio = Vec3::new(
        scale_ratio(pose.scale.x, reference.scale.x),
        scale_ratio(pose.scale.y, reference.scale.y),
        scale_ratio(pose.scale.z, reference.scale.z),
    );
    let scale = base.scale * Vec3::ONE.lerp(ratio, w);

    Transform::new(position, rotation, scale)
}

/// 缩放增量比值；参考分量过小或非有限时视为 1（无增量）。
fn scale_ratio(value: f32, reference: f32) -> f32 {
    if !value.is_finite() || reference.abs() <= 1e-6 {
        return 1.0;
    }
    value / reference
}

/// 层信息摘要（脚本 `anim_layer_info` / RPC 回显用）。
pub fn layer_report(layer: &AnimLayer) -> String {
    let base = format!(
        "{} mode={} w={:.2} clips={:?} mask={}",
        layer.name,
        layer.mode.as_str(),
        layer.effective_weight(),
        layer.clip_names(),
        layer.mask.rule_count()
    );
    // 混合空间段只在挂了空间时追加，既有日志行（前缀）保持不变。
    let base = match &layer.blend_space {
        Some(space) => format!("{base} blend={}", space.brief()),
        None => base,
    };
    // 相位锁同理：只在开启时追加，`Off` 的日志行与扩展四十六之前逐字一致。
    // 扩展四十七：挂了源层的层再追加 `src=`，没挂则整行与四十六逐字相同。
    let base = match layer.sync {
        LayerSync::Off => base,
        LayerSync::Phase => match layer.sync_source.as_deref() {
            Some(source) => format!("{base} sync=phase src={source}"),
            None => format!("{base} sync=phase"),
        },
    };
    // 扩展四十八：自带速率/循环的片段再追加 `pb=`（只有设过的才出现），
    // 没设播放参数的层整行与扩展四十七逐字相同。
    let playback = playback_list(layer);
    if playback.is_empty() {
        base
    } else {
        format!("{base} pb={playback}")
    }
}

/// 层内设了播放参数的片段清单：`run=2.00:once,walk=0.50`；一个都没设返回空串。
fn playback_list(layer: &AnimLayer) -> String {
    layer
        .clips
        .iter()
        .filter_map(|lc| lc.playback.as_ref().map(|pb| (lc, pb)))
        .map(|(lc, pb)| format!("{}={}", lc.clip.name, pb.brief()))
        .collect::<Vec<_>>()
        .join(",")
}

/// 相位同步探针（扩展四十六）：一行说清「每路片段在采哪一刻、相位还差多少」。
///
/// `drift` 是各路片段相位与**期望相位**的最大偏差：锁相位时恒为 0（含设了偏移的层——
/// 期望相位本身带偏移）；不锁相位时短片段会在末帧停住，drift 一路上涨——这就是滑步的量化证据。
pub fn sync_probe(layer: &AnimLayer) -> String {
    sync_probe_with(layer, layer.phase(), None)
}

/// 带源层的探针（扩展四十七）：`phase` 传**有效相位**，`source` 传源层名。
///
/// `src=` 段只在挂了可用源层时追加，`off=` 段只在有片段设了非零偏移时追加——
/// 两个默认窗口都不改，既有探针行逐字不变。
pub fn sync_probe_with(layer: &AnimLayer, phase: f32, source: Option<&str>) -> String {
    let phase = normalized_phase(phase);
    let duration = layer.duration().max(MIN_LAYER_DURATION);
    let times = layer.clip_times_at(phase);
    let mut drift = 0.0_f32;
    let mut parts = Vec::with_capacity(times.len());
    let mut offsets = Vec::new();
    let mut playbacks = Vec::new();
    for (lc, t) in layer.clips.iter().zip(times) {
        let clip_duration = lc.clip.duration.max(MIN_LAYER_DURATION);
        let p = t / clip_duration;
        // 锁相位时该值恒为 0；不锁相位时它一路上涨，就是「这一路已经停住」的量。
        // 扩展四十八：自带速率/循环的片段（仅 `Off` 生效）按自己的时间轴走，
        // 期望值就是它自己走到的相位——用户主动设的「不同速」不该被报成滑步。
        let expected = match (layer.sync, lc.playback) {
            (LayerSync::Off, Some(_)) => p,
            _ => clip_phase(phase, lc.phase_offset),
        };
        if p.is_finite() {
            drift = drift.max((p - expected).abs());
        }
        if lc.phase_offset != 0.0 {
            offsets.push(format!("{}={:.3}", lc.clip.name, lc.phase_offset));
        }
        if let Some(pb) = lc.playback.as_ref() {
            playbacks.push(format!("{}={}", lc.clip.name, pb.brief()));
        }
        parts.push(format!("{}={:.3}/{:.3}", lc.clip.name, t, clip_duration));
    }
    let mut line = format!(
        "sync={} phase={:.3} dur={:.3} drift={:.3}",
        layer.sync.as_str(),
        phase,
        duration,
        drift
    );
    if let Some(source) = source {
        line.push_str(&format!(" src={source}"));
    }
    if !offsets.is_empty() {
        line.push_str(&format!(" off={}", offsets.join(",")));
    }
    // 扩展四十八：只有设了播放参数的层才有这一段
    if !playbacks.is_empty() {
        line.push_str(&format!(" pb={}", playbacks.join(",")));
    }
    format!("{line} {}", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{AnimKey, AnimTrack};
    use glam::Vec3;

    #[test]
    fn blend_space_drives_clip_weights() {
        type Space = crate::blend_space::BlendSpace;
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        assert!(layer.add_clip(clip("run", vec![]), 1.0));
        let mut space = Space::new_1d("speed");
        assert!(space.add_sample("idle", [0.0, 0.0]));
        assert!(space.add_sample("run", [2.0, 0.0]));
        space.set_value_1d(0.5);
        layer.blend_space = Some(space);

        assert!(layer.apply_blend_space());
        let w = layer.normalized_weights();
        assert!((w[0] - 0.75).abs() < 1e-3, "idle 应 0.75，实为 {}", w[0]);
        assert!((w[1] - 0.25).abs() < 1e-3, "run 应 0.25，实为 {}", w[1]);

        // 参数推到底：run 全额、idle 必须真的归零（没有残留）
        layer
            .blend_space
            .as_mut()
            .expect("刚设过")
            .set_value_1d(2.0);
        assert!(layer.apply_blend_space());
        let w = layer.normalized_weights();
        assert_eq!(w[0], 0.0);
        assert_eq!(w[1], 1.0);
    }

    #[test]
    fn blend_space_without_samples_keeps_manual_weights() {
        type Space = crate::blend_space::BlendSpace;
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        assert!(layer.add_clip(clip("run", vec![]), 0.25));
        // 空间存在但一个样本都没有：层不能因此变成静音，手写权重照旧。
        layer.blend_space = Some(Space::new_1d("speed"));
        assert!(!layer.apply_blend_space());
        let w = layer.normalized_weights();
        assert!((w[0] - 0.8).abs() < 1e-3);
        assert!((w[1] - 0.2).abs() < 1e-3);
        // 去掉空间也一样：完全没有空间的层永远走手写权重
        layer.blend_space = None;
        assert!(!layer.apply_blend_space());
    }

    #[test]
    fn blend_space_ignores_clip_not_in_layer() {
        type Space = crate::blend_space::BlendSpace;
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        assert!(layer.add_clip(clip("run", vec![]), 0.5));
        let mut space = Space::new_1d("speed");
        space.add_sample("idle", [0.0, 0.0]);
        space.add_sample("run", [1.0, 0.0]);
        space.add_sample("ghost", [2.0, 0.0]);
        space.set_value_1d(2.0);
        layer.blend_space = Some(space);
        // 参数停在只有 ghost 有权重的位置：层内两路都被清零，
        // 归一化后为全零（层静音），但绝不能把权重错位到别的片段上。
        assert!(layer.apply_blend_space());
        assert!(layer.normalized_weights().is_empty() || layer.normalized_weights()[1] == 0.0);
        assert_eq!(layer.clips[1].weight, 0.0);
    }

    #[test]
    fn layer_report_appends_blend_segment() {
        type Space = crate::blend_space::BlendSpace;
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        let before = layer_report(&layer);
        assert!(!before.contains("blend="));

        let mut space = Space::new_1d("speed");
        assert!(space.add_sample("idle", [0.0, 0.0]));
        space.set_value_1d(0.7);
        layer.blend_space = Some(space);
        let after = layer_report(&layer);
        assert!(after.starts_with(&before), "原有前缀不应改动：{after}");
        assert!(after.contains("blend=1d(speed) n=1"), "{after}");
        assert!(after.contains("v=0.70"), "{after}");
    }

    #[test]
    fn blend_space_3d_drives_clip_weights() {
        type Space = crate::blend_space::BlendSpace;
        // 扩展四十九：层侧不改一行代码，三维空间靠 apply_blend_space 的通用路径生效。
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        assert!(layer.add_clip(clip("walk", vec![]), 1.0));
        assert!(layer.add_clip(clip("run", vec![]), 1.0));
        let mut space = Space::new_3d("speed", "turn", "slope");
        assert!(space.add_sample_3d("idle", [0.0, 0.0, 0.0]));
        assert!(space.add_sample_3d("walk", [1.0, 0.0, 0.0]));
        assert!(space.add_sample_3d("run", [2.0, 0.0, 0.0]));
        assert!(space.add_sample_3d("far", [2.0, 1.0, 1.0]));
        space.set_value_3d(1.0, 0.0, 0.0);
        layer.blend_space = Some(space);

        assert!(layer.apply_blend_space());
        let w = layer.normalized_weights();
        assert_eq!(w, vec![0.0, 1.0, 0.0], "参数停在 walk：{w:?}");

        // 斜着走到 idle/walk/run 中间：三档都得到份额，且层内没有的 far 被忽略
        layer
            .blend_space
            .as_mut()
            .expect("刚设过")
            .set_value_3d(1.0, 0.5, 0.5);
        assert!(layer.apply_blend_space());
        let w = layer.normalized_weights();
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{w:?}");
        assert!(w[1] > 0.0, "walk 应有份额：{w:?}");
        assert!(layer.clips[0].weight >= 0.0 && layer.clips[2].weight >= 0.0);
    }

    #[test]
    fn layer_report_appends_3d_blend_segment() {
        type Space = crate::blend_space::BlendSpace;
        let mut layer = AnimLayer::new("move", clip("idle", vec![]), 1.0);
        let mut space = Space::new_3d("speed", "turn", "slope");
        assert!(space.add_sample_3d("idle", [0.0, 0.0, 0.0]));
        assert!(space.add_sample_3d("walk", [1.0, 0.0, 0.0]));
        space.set_value_3d(0.5, 0.0, 0.0);
        layer.blend_space = Some(space);
        let info = layer_report(&layer);
        assert!(info.contains("blend=3d(speed,turn,slope)"), "{info}");
        assert!(info.contains("tet=0"), "{info}");
        assert!(info.contains("v=(0.50,0.00,0.00)"), "{info}");
    }

    fn track_at(node_index: u32, path: &str, end: Vec3) -> AnimTrack {
        AnimTrack {
            node_path: path.to_string(),
            node_index,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                AnimKey {
                    time: 1.0,
                    position: end,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            ],
        }
    }

    fn clip(name: &str, tracks: Vec<AnimTrack>) -> Arc<AnimClip> {
        Arc::new(AnimClip {
            name: name.to_string(),
            duration: 1.0,
            tracks,
        })
    }

    /// 同 `clip`，但时长可指定（相位同步测试要长短片段并存）。
    fn clip_of(name: &str, duration: f32, tracks: Vec<AnimTrack>) -> Arc<AnimClip> {
        Arc::new(AnimClip {
            name: name.to_string(),
            duration,
            tracks,
        })
    }

    /// 关键帧铺满 `duration` 的线性轨道（0 → `end`）。
    fn track_span(index: u32, path: &str, end: Vec3, duration: f32) -> AnimTrack {
        let mut t = track_at(index, path, end);
        t.keys[1].time = duration;
        t
    }

    #[test]
    fn mask_matches_suffix_segments_only() {
        let mask = BoneMask::only(["Arm/Hand"]);
        assert!(mask.contains("RootNode/Arm/Hand"));
        assert!(mask.contains("Arm/Hand"), "整串相等也算命中");
        assert!(!mask.contains("RootNode/Arm/Hands"), "逐段比较，不吞尾巴");
        assert!(!mask.contains("RootNode/Leg/Foot"));
        assert!(BoneMask::pass_all().contains("任意/节点"));
        assert!(!BoneMask::except(["Arm"]).contains("Root/Arm"));
        assert!(BoneMask::except(["Arm"]).contains("Root/Armature"));
    }

    #[test]
    fn normalize_weights_drops_zero_sum() {
        assert_eq!(normalize_weights(&[0.0, 0.0]), Vec::<f32>::new());
        assert_eq!(normalize_weights(&[-1.0, f32::NAN]), Vec::<f32>::new());
        let w = normalize_weights(&[1.0, 3.0]);
        assert!((w[0] - 0.25).abs() < 1e-6 && (w[1] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn blend_override_hits_endpoints() {
        let a = Transform::from_position(Vec3::ZERO);
        let b = Transform::from_position(Vec3::new(10.0, 0.0, 0.0));
        assert!((blend_override(&a, &b, 0.0).position.x).abs() < 1e-6);
        assert!((blend_override(&a, &b, 1.0).position.x - 10.0).abs() < 1e-6);
        assert!((blend_override(&a, &b, 0.5).position.x - 5.0).abs() < 1e-6);
    }

    #[test]
    fn additive_adds_relative_delta_on_top_of_base() {
        let base = Transform::from_position(Vec3::new(0.0, 1.0, 0.0));
        let reference = Transform::from_position(Vec3::ZERO);
        let pose = Transform::from_position(Vec3::new(0.0, 0.0, 2.0));

        let half = apply_additive(&base, &reference, &pose, 0.5);
        assert!((half.position.y - 1.0).abs() < 1e-6);
        assert!((half.position.z - 1.0).abs() < 1e-6, "增量应减半");

        let full = apply_additive(&base, &reference, &pose, 1.0);
        assert!((full.position.z - 2.0).abs() < 1e-6);
    }

    #[test]
    fn additive_rotation_wraps_around_base() {
        let base = Transform::new(
            Vec3::ZERO,
            Quat::from_rotation_y(45f32.to_radians()),
            Vec3::ONE,
        );
        let reference = Transform::new(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        let pose = Transform::new(
            Vec3::ZERO,
            Quat::from_rotation_y(90f32.to_radians()),
            Vec3::ONE,
        );

        let out = apply_additive(&base, &reference, &pose, 1.0);
        // 用旋转后的基向量比对：欧拉分解会把 135° 折成等价的 (180, -45, 180)
        let expected = (base.rotation * Quat::from_rotation_y(90f32.to_radians())) * Vec3::X;
        let got = out.rotation * Vec3::X;
        assert!(
            (got - expected).length() < 1e-4,
            "45° 基础上叠加 90° 增量，实际 {got:?} 应为 {expected:?}"
        );

        // 半权重时增量角度减半（45° + 45°）
        let half = apply_additive(&base, &reference, &pose, 0.5);
        let expected_half = (base.rotation * Quat::from_rotation_y(45f32.to_radians())) * Vec3::X;
        assert!((half.rotation * Vec3::X - expected_half).length() < 1e-4);
    }

    #[test]
    fn sample_layer_pose_blends_weighted_clips() {
        let walk = clip(
            "walk",
            vec![track_at(1, "root/arm", Vec3::new(10.0, 0.0, 0.0))],
        );
        let run = clip(
            "run",
            vec![track_at(1, "root/arm", Vec3::new(20.0, 0.0, 0.0))],
        );
        let clips = vec![LayerClip::new(walk, 0.5), LayerClip::new(run, 0.5)];

        let s = sample_layer_pose(&clips, 0.5, 1, "root/arm").expect("应采到姿态");
        // walk 在 0.5s 是 5，run 是 10，等权 → 7.5
        assert!(
            (s.pose.position.x - 7.5).abs() < 1e-4,
            "{:?}",
            s.pose.position
        );
        assert!((s.reference.position.x).abs() < 1e-4, "参考姿态取首帧");
    }

    /// 一段双关键帧轨道：位置 / 旋转 / 缩放各自从 `from` 走到 `to`。
    fn ramp_track(
        index: u32,
        path: &str,
        from: (Vec3, Quat, Vec3),
        to: (Vec3, Quat, Vec3),
    ) -> AnimTrack {
        AnimTrack {
            node_path: path.to_string(),
            node_index: index,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: from.0,
                    rotation: from.1,
                    scale: from.2,
                },
                AnimKey {
                    time: 1.0,
                    position: to.0,
                    rotation: to.1,
                    scale: to.2,
                },
            ],
        }
    }

    #[test]
    fn additive_single_clip_is_bitwise_unchanged() {
        // 单路口加法层必须与扩展四十九之前逐位一致：新路径在这条上「零算术」
        let only = clip(
            "bob",
            vec![ramp_track(
                1,
                "root/arm",
                (
                    Vec3::new(1.0, 2.0, 3.0),
                    Quat::IDENTITY,
                    Vec3::new(1.0, 1.0, 1.0),
                ),
                (
                    Vec3::new(4.0, 5.0, 6.0),
                    Quat::from_rotation_z(0.6),
                    Vec3::new(2.0, 2.0, 2.0),
                ),
            )],
        );
        let clips = vec![LayerClip::new(only, 1.0)];
        let old = sample_layer_pose(&clips, 0.5, 1, "root/arm").expect("覆盖路径应采到");
        let new = sample_layer_pose_additive(&clips, 0.5, 1, "root/arm").expect("加法路径应采到");
        assert_eq!(old.pose.position, new.pose.position);
        assert_eq!(old.pose.rotation, new.pose.rotation);
        assert_eq!(old.pose.scale, new.pose.scale);
        assert_eq!(old.reference.position, new.reference.position);
        assert_eq!(old.reference.rotation, new.reference.rotation);
        assert_eq!(old.reference.scale, new.reference.scale);
    }

    #[test]
    fn additive_sums_rotation_deltas_not_pose_average() {
        // A：绕 X 从 0 转到 90°（增量 90°）；B：参考与姿态都是绕 Y 80°（增量 0）。
        // 加法语义下 B 不该稀释 A —— 合成增量应是「半个 A 的 90°」= 45° 绕 X。
        // 覆盖语义会把两条姿态先平均（45° 的混轴旋转）再减平均参考，得到的是别的姿态。
        let a = clip(
            "a",
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
                (
                    Vec3::ZERO,
                    Quat::from_rotation_x(90f32.to_radians()),
                    Vec3::ONE,
                ),
            )],
        );
        let hold = Quat::from_rotation_y(80f32.to_radians());
        let b = clip(
            "b",
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, hold, Vec3::ONE),
                (Vec3::ZERO, hold, Vec3::ONE),
            )],
        );
        let clips = vec![LayerClip::new(a, 0.5), LayerClip::new(b, 0.5)];

        let add = sample_layer_pose_additive(&clips, 1.0, 1, "root/arm").expect("应采到姿态");
        let want = Quat::from_rotation_x(45f32.to_radians()) * Vec3::Z;
        let got = add.pose.rotation * Vec3::Z;
        assert!(
            (got - want).length() < 1e-4,
            "合成增量应为 45° 绕 X，实际 {got:?} 应为 {want:?}"
        );
        assert!(
            (add.reference.rotation * Vec3::Z - Vec3::Z).length() < 1e-4,
            "参考取第一路的首帧（identity）"
        );

        let old = sample_layer_pose(&clips, 1.0, 1, "root/arm").expect("覆盖路径应采到");
        assert!(
            (old.pose.rotation * Vec3::Z - got).length() > 0.05,
            "覆盖路径把增量钝化了，结果本应与加法路径明显不同"
        );
    }

    #[test]
    fn additive_sums_scale_ratios_not_pose_average() {
        // A：1 → 4（×4）；B：参考与姿态都是 2（增量 ×1）。
        // 加法：参考缩放 1 × lerp(1,4,.5) × lerp(1,1,.5) = 2.5
        let a = clip(
            "a",
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
                (Vec3::ZERO, Quat::IDENTITY, Vec3::splat(4.0)),
            )],
        );
        let two = Vec3::splat(2.0);
        let b = clip(
            "b",
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, Quat::IDENTITY, two),
                (Vec3::ZERO, Quat::IDENTITY, two),
            )],
        );
        let clips = vec![LayerClip::new(a, 0.5), LayerClip::new(b, 0.5)];
        let add = sample_layer_pose_additive(&clips, 1.0, 1, "root/arm").expect("应采到姿态");
        assert!(
            (add.pose.scale.x - 2.5).abs() < 1e-4,
            "比值增量应是 1 × 2 × 1 = 2.5，实际 {:?}",
            add.pose.scale
        );
    }

    #[test]
    fn additive_normalizes_partial_weights() {
        // 权重 1 / 3 → 份额 0.25 / 0.75，增量按份额分
        let a = clip("a", vec![track_at(1, "root/arm", Vec3::new(4.0, 0.0, 0.0))]);
        let b = clip("b", vec![track_at(1, "root/arm", Vec3::ZERO)]);
        let clips = vec![LayerClip::new(a, 1.0), LayerClip::new(b, 3.0)];
        let s = sample_layer_pose_additive(&clips, 1.0, 1, "root/arm").expect("应采到姿态");
        assert!(
            (s.pose.position.x - 1.0).abs() < 1e-4,
            "0.25 × 4 = 1，实际 {:?}",
            s.pose.position
        );
        assert!((s.reference.position.length()).abs() < 1e-6);
    }

    #[test]
    fn additive_skips_zero_weight_and_missing_track() {
        let arm = clip("a", vec![track_at(1, "root/arm", Vec3::new(4.0, 0.0, 0.0))]);
        // 权重 0 的那路不参与（否则会被当成"零增量"稀释掉唯一有效路）
        let clips = vec![LayerClip::new(arm.clone(), 1.0), LayerClip::new(arm, 0.0)];
        let s = sample_layer_pose_additive(&clips, 1.0, 1, "root/arm").expect("应采到姿态");
        assert!(
            (s.pose.position.x - 4.0).abs() < 1e-4,
            "{:?}",
            s.pose.position
        );
        // 轨道缺失时返回 None，调用方保留主姿态
        assert!(sample_layer_pose_additive(&clips, 1.0, 2, "root/leg").is_none());
    }

    #[test]
    fn additive_synced_uses_own_clip_duration() {
        // 相位锁：A（1 s）与 B（4 s）在同一相位 0.5 各自采样到 0.5 s 与 2.0 s
        let a = clip_of(
            "a",
            1.0,
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
                (Vec3::splat(10.0), Quat::IDENTITY, Vec3::ONE),
            )],
        );
        let b = clip_of(
            "b",
            4.0,
            vec![ramp_track(
                1,
                "root/arm",
                (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
                (Vec3::splat(10.0), Quat::IDENTITY, Vec3::ONE),
            )],
        );
        let clips = vec![LayerClip::new(a, 1.0), LayerClip::new(b, 1.0)];
        let s = sample_layer_pose_additive_synced(&clips, 0.5, 1, "root/arm").expect("应采到姿态");
        // A 在 0.5 s 增量 5；B 在 2.0 s 越界钳到 10 → 0.5×5 + 0.5×10 = 7.5
        assert!(
            (s.pose.position.x - 7.5).abs() < 1e-4,
            "相位锁下每路按自己时长折算，实际 {:?}",
            s.pose.position
        );
    }

    #[test]
    fn sample_layer_pose_skips_missing_track() {
        let only_arm = clip(
            "walk",
            vec![track_at(1, "root/arm", Vec3::new(10.0, 0.0, 0.0))],
        );
        let clips = vec![LayerClip::new(only_arm, 1.0)];
        assert!(sample_layer_pose(&clips, 0.0, 2, "root/leg").is_none());
        // 权重全零时同样不产出
        let zero = vec![LayerClip::new(
            clip("walk", vec![track_at(1, "root/arm", Vec3::ZERO)]),
            0.0,
        )];
        assert!(sample_layer_pose(&zero, 0.0, 1, "root/arm").is_none());
    }

    #[test]
    fn layer_fade_ramps_weight_smoothly() {
        let mut layer = AnimLayer::new(
            "upper",
            clip("atk", vec![track_at(1, "root/arm", Vec3::ZERO)]),
            0.0,
        );
        layer.fade_to(1.0, 0.4);
        assert!(layer.effective_weight() < 1e-3, "过渡起点应为 0");

        layer.advance(0.2);
        let mid = layer.effective_weight();
        assert!((mid - 0.5).abs() < 1e-3, "半程应约 0.5，实际 {mid}");

        layer.advance(0.2);
        assert!((layer.effective_weight() - 1.0).abs() < 1e-6);
        assert!(layer.fade.is_none(), "过渡走完应清空");
        assert!((layer.weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn layer_advance_wraps_time_and_keeps_order() {
        let mut layer = AnimLayer::new(
            "good",
            clip("w", vec![track_at(1, "root/arm", Vec3::ZERO)]),
            1.0,
        );
        layer.advance(2.5);
        assert!((layer.sample_time() - 0.5).abs() < 1e-4);
        layer.looping = false;
        layer.advance(5.0);
        assert!((layer.sample_time() - 1.0).abs() < 1e-4, "非循环应钳到时长");
    }

    #[test]
    fn layer_clip_management_respects_bounds() {
        let mut layer = AnimLayer::new(
            "upper",
            clip("a", vec![track_at(1, "root/arm", Vec3::ZERO)]),
            1.0,
        );
        assert!(layer.add_clip(clip("b", vec![track_at(1, "root/arm", Vec3::ZERO)]), 0.5));
        assert_eq!(layer.clips.len(), 2);
        assert!(layer.set_clip_weight("b", 2.0));
        let w = layer.normalized_weights();
        assert!((w[1] - 2.0 / 3.0).abs() < 1e-5);
        assert!(layer.remove_clip("b"));
        assert!(!layer.remove_clip("a"), "不能移除最后一个片段");
    }

    #[test]
    fn layer_mode_parse_is_case_insensitive() {
        assert_eq!(LayerMode::parse("Additive"), Some(LayerMode::Additive));
        assert_eq!(LayerMode::parse("override"), Some(LayerMode::Override));
        assert_eq!(LayerMode::parse("mix"), None);
    }

    // ---------- 层内相位同步（扩展四十六） ----------

    #[test]
    fn layer_sync_parse_and_default_off() {
        assert_eq!(LayerSync::default(), LayerSync::Off);
        assert_eq!(LayerSync::parse("Phase"), Some(LayerSync::Phase));
        assert_eq!(LayerSync::parse("SYNC"), Some(LayerSync::Phase));
        assert_eq!(LayerSync::parse(" none "), Some(LayerSync::Off));
        assert_eq!(LayerSync::parse("locked"), None);

        // 新建层默认不锁相位：既有日志行里不会突然多出 sync= 段
        let layer = AnimLayer::new("l", clip("a", vec![]), 1.0);
        assert_eq!(layer.sync, LayerSync::Off);
        assert!(
            !layer_report(&layer).contains("sync="),
            "{}",
            layer_report(&layer)
        );
        // 开了才会追加
        let locked = layer.with_sync(LayerSync::Phase);
        assert!(layer_report(&locked).contains("sync=phase"));
    }

    #[test]
    fn sync_phase_scales_clip_times_by_clip_duration() {
        // 走 1.0 s + 跑 0.5 s：层时长取最长的那一路
        let mut layer = AnimLayer::new("move", clip("walk", vec![]), 1.0);
        assert!(layer.add_clip(clip_of("run", 0.5, vec![]), 1.0));
        layer.advance(0.25);
        assert!((layer.phase() - 0.25).abs() < 1e-5, "{}", layer.phase());

        // 不锁相位：两路共用一个时间（这就是扩展四十六之前的老口径）
        assert_eq!(layer.clip_times(), vec![0.25, 0.25]);

        // 锁相位：每路按自己时长缩放，相位相同而步频保留
        layer.sync = LayerSync::Phase;
        let times = layer.clip_times();
        assert!((times[0] - 0.25).abs() < 1e-5, "{times:?}");
        assert!((times[1] - 0.125).abs() < 1e-5, "{times:?}");
        // 越界相位被钳到 0..1，不会反向外插
        assert_eq!(layer.synced_clip_time(1, 3.0), Some(0.5));
        assert_eq!(layer.synced_clip_time(1, -1.0), Some(0.0));
        assert_eq!(layer.synced_clip_time(9, 0.5), None);
    }

    #[test]
    fn synced_sampling_aligns_equal_phase_across_durations() {
        // 两路都是 0→10 的直线：走用 1.0 s 走完、跑用 0.5 s 走完
        let walk = clip_of(
            "walk",
            1.0,
            vec![track_span(1, "root/arm", Vec3::new(10.0, 0.0, 0.0), 1.0)],
        );
        let run = clip_of(
            "run",
            0.5,
            vec![track_span(1, "root/arm", Vec3::new(10.0, 0.0, 0.0), 0.5)],
        );
        let both = vec![LayerClip::new(walk.clone(), 1.0), LayerClip::new(run, 1.0)];

        // 老口径：同一时刻 0.25 s → 走到 25%、跑到 50%，两路姿态已经错开（滑步的成因）
        let legacy = sample_layer_pose(&both, 0.25, 1, "root/arm").expect("两路都有该轨道");
        assert!(
            (legacy.pose.position.x - 3.75).abs() < 1e-4,
            "老口径应 3.75，实际 {}",
            legacy.pose.position.x
        );

        // 相位锁：同一相位 0.25 → 两路都走到 25%，姿态逐位重合
        let one = vec![LayerClip::new(walk, 1.0)];
        let two = vec![LayerClip::new(
            clip_of(
                "run",
                0.5,
                vec![track_span(1, "root/arm", Vec3::new(10.0, 0.0, 0.0), 0.5)],
            ),
            1.0,
        )];
        let a = sample_layer_pose_synced(&one, 0.25, 1, "root/arm").expect("走有轨道");
        let b = sample_layer_pose_synced(&two, 0.25, 1, "root/arm").expect("跑有轨道");
        assert!(
            (a.pose.position.x - 2.5).abs() < 1e-4,
            "{}",
            a.pose.position.x
        );
        assert!(
            (a.pose.position.x - b.pose.position.x).abs() < 1e-6,
            "同相位姿态应一致：{} vs {}",
            a.pose.position.x,
            b.pose.position.x
        );
    }

    #[test]
    fn sync_probe_quantifies_phase_drift() {
        let mut layer = AnimLayer::new("move", clip("walk", vec![]), 1.0);
        assert!(layer.add_clip(clip_of("run", 0.5, vec![]), 1.0));
        layer.advance(0.6);

        // 不锁相位：跑片段时间也被推到 0.6，可它的时长只有 0.5 → 相位 1.2，drift 0.6
        let probe = sync_probe(&layer);
        assert!(probe.contains("sync=off"), "{probe}");
        assert!(probe.contains("drift=0.600"), "{probe}");

        // 锁相位：两路相位同为 0.6，drift 归零
        layer.sync = LayerSync::Phase;
        let probe = sync_probe(&layer);
        assert!(probe.contains("sync=phase"), "{probe}");
        assert!(probe.contains("drift=0.000"), "{probe}");
        assert!(probe.contains("walk=0.600/1.000"), "{probe}");
        assert!(probe.contains("run=0.300/0.500"), "{probe}");
        assert!(probe.contains("phase=0.600"), "{probe}");
    }

    // ---------- 相位偏移量与跨层对齐（扩展四十七） ----------

    #[test]
    fn phase_offset_wraps_into_unit_range() {
        // 偏移折进 [0,1)：-0.25 等价 0.75，1.25 等价 0.25，0.5 就是错开半步
        let walk = clip_of("walk", 1.0, vec![]);
        let mut lc = LayerClip::new(walk.clone(), 1.0);
        assert_eq!(lc.phase_offset, 0.0);
        lc.set_phase_offset(-0.25);
        assert!((lc.phase_offset - 0.75).abs() < 1e-6, "{}", lc.phase_offset);
        lc.set_phase_offset(1.25);
        assert!((lc.phase_offset - 0.25).abs() < 1e-6, "{}", lc.phase_offset);
        lc.set_phase_offset(f32::NAN);
        assert_eq!(lc.phase_offset, 0.0);

        // 相位 0 + 偏移 0.5 → 采到片段半程
        let half = LayerClip::new(walk, 1.0).with_phase_offset(0.5);
        assert!((half.synced_time(0.0) - 0.5).abs() < 1e-6);
        // 相位 0.75 + 偏移 0.5 折回 0.25
        assert!((half.synced_time(0.75) - 0.25).abs() < 1e-6);
        // 折回后不会越界
        assert!((0.0..=1.0).contains(&wrap_phase(2.0)));
        assert_eq!(wrap_phase(-0.25), 0.75);
    }

    #[test]
    fn zero_offset_keeps_legacy_arithmetic() {
        // 偏移为 0 时必须与扩展四十六逐位一致：相位 1.0 采末帧，不折回首帧
        let mut layer = AnimLayer::new("move", clip("walk", vec![]), 1.0);
        assert!(layer.add_clip(clip_of("run", 0.5, vec![]), 1.0));
        layer.sync = LayerSync::Phase;
        assert_eq!(layer.synced_clip_time(0, 1.0), Some(1.0));
        assert_eq!(layer.synced_clip_time(1, 1.0), Some(0.5));
        // 探针行里也不该凭空多出 off= 段
        let probe = sync_probe(&layer);
        assert!(!probe.contains("off="), "{probe}");
        assert!(!probe.contains("src="), "{probe}");
    }

    #[test]
    fn sync_probe_keeps_drift_zero_with_offset() {
        // 设了偏移后 drift 仍应为 0：期望相位本身跟着偏移走，不该被误判成滑步
        let walk = clip_of("walk", 1.0, vec![]);
        let run = clip_of("run", 0.5, vec![]);
        let mut layer = AnimLayer::new("move", walk, 1.0);
        assert!(layer.add_clip(run, 1.0));
        layer.sync = LayerSync::Phase;
        assert!(layer.set_clip_phase_offset("run", 0.5));
        layer.advance(0.25);

        let probe = sync_probe(&layer);
        assert!(probe.contains("drift=0.000"), "{probe}");
        assert!(probe.contains("off=run=0.500"), "{probe}");
        // run 时长 0.5、相位 0.75 → 0.375
        assert!(probe.contains("run=0.375/0.500"), "{probe}");
    }

    /// 三层：`index 0` 根、`1` 跟随 `0`、`2` 再跟随 `1`（链式跟随应一路走到 0）。
    fn chain_layers() -> Vec<AnimLayer> {
        let mut root = AnimLayer::new("root", clip_of("walk", 1.0, vec![]), 1.0);
        root.sync = LayerSync::Phase;
        root.advance(0.4);
        let mut mid = AnimLayer::new("mid", clip_of("walk", 1.0, vec![]), 1.0)
            .with_sync(LayerSync::Phase)
            .with_sync_source(Some("root"));
        mid.speed = 0.0;
        let mut top = AnimLayer::new("top", clip_of("walk", 1.0, vec![]), 1.0)
            .with_sync(LayerSync::Phase)
            .with_sync_source(Some("mid"));
        top.speed = 0.0;
        vec![root, mid, top]
    }

    #[test]
    fn phase_source_follows_chain_and_breaks_cycles() {
        let layers = chain_layers();
        assert_eq!(phase_source(&layers, 1), Some(0));
        assert_eq!(phase_source(&layers, 2), Some(0));
        assert_eq!(phase_source(&layers, 0), None, "根层没有源");
        // 有效相位取链尾的相位，本层相位仍是自己的
        assert!((effective_phase(&layers, 2) - 0.4).abs() < 1e-5);
        assert!((layers[2].phase() - 0.0).abs() < 1e-5);

        // 成环：mid 反过来跟 top，链走不出来 → None，回落本层相位
        let mut cyclic = chain_layers();
        cyclic[1].set_sync_source(Some("top"));
        assert_eq!(phase_source(&cyclic, 1), None);
        assert!((effective_phase(&cyclic, 1) - 0.0).abs() < 1e-5);

        // 自引用与解析不到的名字同样当没有源层
        let mut selfish = chain_layers();
        selfish[1].set_sync_source(Some("mid"));
        assert_eq!(phase_source(&selfish, 1), None);
        let mut ghost = chain_layers();
        ghost[1].set_sync_source(Some("nope"));
        assert_eq!(phase_source(&ghost, 1), None);

        // Off 的层不看源层：相位就是自己的
        let mut off = chain_layers();
        off[2].sync = LayerSync::Off;
        assert!((effective_phase(&off, 2) - off[2].phase()).abs() < 1e-6);
    }

    // ---------- 片段自带速率与循环标志（扩展四十八） ----------

    #[test]
    fn clip_playback_scales_and_wraps_time() {
        let walk = clip_of("walk", 1.0, vec![]);

        // 没设播放参数：层时间原样透传（不乘 1.0、不取模），这是零变化的算术支点
        let plain = LayerClip::new(walk.clone(), 1.0);
        assert!(plain.playback.is_none());
        assert_eq!(plain.playback_time(0.0), 0.0);
        assert_eq!(plain.playback_time(0.37), 0.37);
        assert_eq!(
            plain.playback_time(3.0),
            3.0,
            "老口径不做取模，超时长交给 track 钳末帧"
        );

        // 2 倍速：0.25 秒的层时间采到片段 0.5 秒处，超过时长绕回
        let fast = LayerClip::new(walk.clone(), 1.0).with_playback(2.0, true);
        assert!((fast.playback_time(0.25) - 0.5).abs() < 1e-6);
        assert!(
            (fast.playback_time(0.75) - 0.5).abs() < 1e-6,
            "1.5 折回 0.5"
        );

        // 不循环：钳到末帧（时长 1.0）
        let once = LayerClip::new(walk.clone(), 1.0).with_playback(2.0, false);
        assert!((once.playback_time(0.75) - 1.0).abs() < 1e-6);

        // 负速率倒放：层时间 0.25、速率 -1 → rem_euclid 折到 0.75
        let back = LayerClip::new(walk.clone(), 1.0).with_playback(-1.0, true);
        assert!((back.playback_time(0.25) - 0.75).abs() < 1e-6);
        // 非循环的倒放：钳到首帧
        let back_once = LayerClip::new(walk.clone(), 1.0).with_playback(-1.0, false);
        assert_eq!(back_once.playback_time(0.25), 0.0);

        // 速率 0：定格首帧
        let frozen = LayerClip::new(walk.clone(), 1.0).with_playback(0.0, true);
        assert_eq!(frozen.playback_time(0.6), 0.0);

        // 非有限速率按原速（配 looping=true 时 0.4 原样落进 [0,1)）
        let nan = LayerClip::new(walk, 1.0).with_playback(f32::NAN, true);
        assert_eq!(nan.playback.as_ref().map(|p| p.speed), Some(1.0));
        assert!((nan.playback_time(0.4) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn clip_playback_changes_sampled_pose() {
        // 位置轨道 0 → (10,0,0)，时长 1 s；层时间 0.25 时老口径采到 x = 2.5
        let track = track_at(1, "root/arm", Vec3::new(10.0, 0.0, 0.0));
        let walk = clip_of("walk", 1.0, vec![track]);

        let plain = vec![LayerClip::new(walk.clone(), 1.0)];
        let legacy = sample_layer_pose(&plain, 0.25, 1, "root/arm").expect("有轨道");
        assert!((legacy.pose.position.x - 2.5).abs() < 1e-5);

        // 2 倍速：同一层时间采到 0.5 s 处，x = 5.0
        let fast = vec![LayerClip::new(walk.clone(), 1.0).with_playback(2.0, true)];
        let sampled = sample_layer_pose(&fast, 0.25, 1, "root/arm").expect("有轨道");
        assert!(
            (sampled.pose.position.x - 5.0).abs() < 1e-5,
            "2 倍速应采到半程，实际 {}",
            sampled.pose.position.x
        );

        // 清除播放参数后逐位回到老口径
        let mut cleared = LayerClip::new(walk, 1.0).with_playback(2.0, true);
        cleared.clear_playback();
        assert_eq!(cleared.playback_time(0.25), 0.25);
        let back = sample_layer_pose(&[cleared], 0.25, 1, "root/arm").expect("有轨道");
        assert_eq!(back.pose.position.x, legacy.pose.position.x, "逐位一致");
    }

    #[test]
    fn clip_playback_only_shows_up_when_set() {
        // 没设播放参数：快照行与探针行都不该凭空多出 pb= 段（零变化）
        let mut layer = AnimLayer::new("move", clip_of("walk", 1.0, vec![]), 1.0);
        assert!(layer.add_clip(clip_of("run", 0.5, vec![]), 1.0));
        assert!(
            !layer_report(&layer).contains("pb="),
            "{}",
            layer_report(&layer)
        );
        assert!(
            !sync_probe(&layer).contains("pb="),
            "{}",
            sync_probe(&layer)
        );

        // 设了一路：快照与探针都追加 pb=，循环是常态不标注，非循环标 :once
        assert!(layer.set_clip_playback("run", 2.0, false));
        let report = layer_report(&layer);
        assert!(report.contains("pb=run=2.00:once"), "{report}");
        let probe = sync_probe(&layer);
        assert!(probe.contains("pb=run=2.00:once"), "{probe}");

        // 自带速率的层 drift 归零：用户主动设的不同速不该被报成滑步
        assert!(probe.contains("drift=0.000"), "{probe}");
    }

    #[test]
    fn phase_lock_ignores_clip_playback() {
        // 相位锁层的每路时间由相位决定，播放速率不参与（要变速请调层 speed）
        let mut layer = AnimLayer::new("move", clip_of("walk", 1.0, vec![]), 1.0);
        assert!(layer.add_clip(clip_of("run", 0.5, vec![]), 1.0));
        layer.sync = LayerSync::Phase;
        assert!(layer.set_clip_playback("run", 2.0, true));
        let times = layer.clip_times_at(0.5);
        assert!((times[0] - 0.5).abs() < 1e-6, "{times:?}");
        assert!((times[1] - 0.25).abs() < 1e-6, "{times:?}");
    }
}
