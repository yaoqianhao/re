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
//! 共享同一条时间轴（走跑混合要求相位同步），不同层各有独立时间轴。
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

/// 层内的一路片段及其权重。
#[derive(Debug, Clone)]
pub struct LayerClip {
    pub clip: Arc<AnimClip>,
    /// 混合权重（负值按 0 处理；层内按非零和归一化后使用）。
    pub weight: f32,
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
        }
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
pub fn sample_layer_pose(
    clips: &[LayerClip],
    time: f32,
    node_index: u32,
    node_path: &str,
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
        let Some(pose) = track.sample(time) else {
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
    match &layer.blend_space {
        Some(space) => format!("{base} blend={}", space.brief()),
        None => base,
    }
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
}
