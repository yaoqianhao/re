//! 节点级动画（P3）：烘焙后的 TRS 关键帧、采样与场景内动画实例。
//!
//! 数据来源是资产层（`r3e-asset`）从 FBX 动画栈烘焙出来的关键帧表，本模块只做
//! 纯数据承载与求值，不认识 ufbx、也不认识渲染。
//!
//! 时间轴约定：
//!
//! * 所有关键帧时间都已重基到 0（`time_begin` 被减掉），因此 `clip.duration`
//!   就是可直接循环的长度；
//! * `AnimInstance::time` 是播放器自己的本地时间，取模/截断只发生在采样前。

use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::{NodeId, Transform};

/// 一条烘焙后的 TRS 关键帧。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimKey {
    /// 相对片段起点的时间（秒）。
    pub time: f32,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl AnimKey {
    pub fn transform(&self) -> Transform {
        Transform::new(self.position, self.rotation, self.scale)
    }
}

/// 单个节点的动画轨道：按时间升序的关键帧序列。
#[derive(Debug, Clone, Default)]
pub struct AnimTrack {
    /// 资产内节点路径（如 `RootNode/Arm/Hand`），用于日志与脚本定位。
    pub node_path: String,
    /// 资产内节点下标（`NodeAsset::index`）。
    pub node_index: u32,
    pub keys: Vec<AnimKey>,
}

impl AnimTrack {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// 轨道自身的首末时间。
    pub fn time_range(&self) -> (f32, f32) {
        match (self.keys.first(), self.keys.last()) {
            (Some(a), Some(b)) => (a.time, b.time),
            _ => (0.0, 0.0),
        }
    }

    /// 在 `time` 秒处采样：位置 / 缩放线性插值，旋转球面插值。
    ///
    /// 超出首末关键帧时钳到端点（循环由调用方按 `clip.duration` 取模完成）。
    pub fn sample(&self, time: f32) -> Option<Transform> {
        let first = self.keys.first()?;
        if self.keys.len() == 1 {
            return Some(first.transform());
        }
        if time <= first.time {
            return Some(first.transform());
        }
        let last = self.keys.last()?;
        if time >= last.time {
            return Some(last.transform());
        }

        // 二分找到最后一个 `key.time <= time` 的下标
        let mut lo = 0usize;
        let mut hi = self.keys.len() - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.keys[mid].time <= time {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let a = &self.keys[lo];
        let b = &self.keys[hi];
        let span = b.time - a.time;
        let t = if span > 1e-6 {
            ((time - a.time) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };

        Some(Transform::new(
            a.position.lerp(b.position, t),
            // slerp 处理符号翻转，避免 180° 附近绕远路；归一化失败时退回前一个关键帧
            a.rotation.slerp(b.rotation, t),
            a.scale.lerp(b.scale, t),
        ))
    }
}

/// 一个动画片段：名称 + 时长 + 各节点轨道。
#[derive(Debug, Clone, Default)]
pub struct AnimClip {
    pub name: String,
    /// 片段时长（秒），由最早/最晚关键帧推导。
    pub duration: f32,
    pub tracks: Vec<AnimTrack>,
}

impl AnimClip {
    /// 采样整段片段，返回 `(轨道下标, 变换)`。未到时间的轨道按端点钳制。
    pub fn sample_all(&self, time: f32) -> Vec<(u32, Transform)> {
        let mut out = Vec::with_capacity(self.tracks.len());
        for (i, track) in self.tracks.iter().enumerate() {
            if let Some(t) = track.sample(time) {
                out.push((i as u32, t));
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// 按名字查找轨道（大小写不敏感，兼容 `Idle` / `idle` 两种写法）。
    pub fn track_index(&self, node_path: &str) -> Option<u32> {
        self.tracks
            .iter()
            .position(|t| t.node_path.eq_ignore_ascii_case(node_path))
            .map(|i| i as u32)
    }

    /// 按「资产内节点下标优先、路径兜底」定位轨道，用于切片段时重绑。
    pub fn track_of(&self, node_index: u32, node_path: &str) -> Option<u32> {
        self.tracks
            .iter()
            .position(|t| t.node_index == node_index)
            .or_else(|| {
                self.tracks
                    .iter()
                    .position(|t| t.node_path.eq_ignore_ascii_case(node_path))
            })
            .map(|i| i as u32)
    }
}

/// 一条「片段轨道 → 场景节点」的绑定。
///
/// 同时记录资产内节点下标与节点路径：切片段时优先按下标找轨道（最稳），
/// 下标缺失（例如另一份 FBX 的同一骨架）再退回路径匹配。
#[derive(Debug, Clone)]
pub struct AnimBinding {
    /// 被驱动的场景节点。
    pub node: NodeId,
    /// 资产内节点下标（`AnimTrack::node_index`）。
    pub node_index: u32,
    /// 资产内节点路径（`AnimTrack::node_path`）。
    pub path: String,
    /// 当前片段内的轨道下标。
    pub track: u32,
}

/// 「该片段没有这条轨道」的哨兵下标（用于交叉淡入期间保留旧片段独有轨道）。
pub const NO_TRACK: u32 = u32::MAX;

/// 把播放时间折算成片段内时间：循环取模，非循环钳到 `[0, duration]`。
pub fn wrap_time(time: f32, looping: bool, duration: f32) -> f32 {
    let duration = duration.max(1e-4);
    if looping {
        // rem_euclid 保证负速度（倒放）也落在 [0, duration)
        time.rem_euclid(duration)
    } else {
        time.clamp(0.0, duration)
    }
}

/// 两个位姿之间按权重混合（`w = 0` 全取 `from`，`w = 1` 全取 `to`）。
///
/// 位置 / 缩放线性插值，旋转球面插值——与关键帧插值口径一致，避免四元数直接线性
/// 插值在 180° 附近出现体积塌缩。
pub fn blend_transform(from: &Transform, to: &Transform, w: f32) -> Transform {
    let w = w.clamp(0.0, 1.0);
    Transform::new(
        from.position.lerp(to.position, w),
        from.rotation.slerp(to.rotation, w),
        from.scale.lerp(to.scale, w),
    )
}

/// 交叉淡入（cross-fade）状态：切片段之前那一片段的快照。
///
/// 过渡期间两套片段各自按自己的时间推进，逐轨道混合后写回节点，因此不需要复制
/// 骨架节点、也不需要在渲染层做双份蒙皮权重。
#[derive(Debug, Clone)]
pub struct AnimFade {
    /// 过渡的「旧片段」。
    pub clip: Arc<AnimClip>,
    /// 旧片段的本地时间（秒，未取模）。
    pub time: f32,
    /// 旧片段的播放速度（切片段那一刻冻结，不跟随 `set_anim_speed` 改动）。
    pub speed: f32,
    pub looping: bool,
    /// 与 `AnimInstance::bindings` 同序：旧片段里对应的轨道下标，`NO_TRACK` 表示没有。
    pub tracks: Vec<u32>,
    /// 已过渡时长（秒）。
    pub elapsed: f32,
    /// 过渡总时长（秒）。
    pub duration: f32,
}

impl AnimFade {
    /// 新片段当前应占的权重（0 → 1，smoothstep 缓入缓出）。
    pub fn weight(&self) -> f32 {
        let t = (self.elapsed / self.duration.max(1e-6)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// 旧片段当前应采样的片段内时间。
    pub fn sample_time(&self) -> f32 {
        wrap_time(self.time, self.looping, self.clip.duration)
    }
}

/// 场景里的一个动画实例：绑定一个片段 + 一组场景节点 + 播放状态。
///
/// 一个资产实例（一次 `assets[]` 声明）对应一个实例；轨道按「资产内节点路径 →
/// 场景节点」的映射绑定，因此同一片段可以被多个实例复用（`Arc` 共享）。
#[derive(Debug, Clone)]
pub struct AnimInstance {
    /// 实例名，用于脚本 `play_anim` 定位与日志。
    pub name: String,
    pub clip: Arc<AnimClip>,
    /// 绑定表；节点被销毁时该绑定自动失效（跳过写回）。
    pub bindings: Vec<AnimBinding>,
    /// 播放本地时间（秒，未取模）。
    pub time: f32,
    /// 播放速度（1.0 = 原始速度，可为负用于倒放）。
    pub speed: f32,
    pub looping: bool,
    pub playing: bool,
    /// 正在进行的交叉淡入（`None` = 单片段直出）。
    pub fade: Option<AnimFade>,
}

impl AnimInstance {
    pub fn new(name: impl Into<String>, clip: Arc<AnimClip>) -> Self {
        Self {
            name: name.into(),
            clip,
            bindings: Vec::new(),
            time: 0.0,
            speed: 1.0,
            looping: true,
            playing: true,
            fade: None,
        }
    }

    /// 当前应采样的片段内时间。
    pub fn sample_time(&self) -> f32 {
        wrap_time(self.time, self.looping, self.clip.duration)
    }

    /// 新片段当前权重：无过渡时为 1.0，过渡中按 smoothstep 从 0 涨到 1。
    pub fn fade_weight(&self) -> f32 {
        self.fade.as_ref().map(AnimFade::weight).unwrap_or(1.0)
    }

    /// 旧片段当前应采样的片段内时间；无过渡时为 `None`。
    pub fn fade_sample_time(&self) -> Option<f32> {
        self.fade.as_ref().map(AnimFade::sample_time)
    }

    /// 推进时间；非循环播放到末尾后自动停止。过渡随访同步推进，走完自动丢弃。
    pub fn advance(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        self.time += dt * self.speed;
        if !self.looping {
            let duration = self.clip.duration.max(1e-4);
            if self.time >= duration {
                self.time = duration;
                self.playing = false;
            } else if self.time <= 0.0 && self.speed < 0.0 {
                self.time = 0.0;
                self.playing = false;
            }
        }

        let mut finished = false;
        if let Some(fade) = self.fade.as_mut() {
            fade.elapsed += dt;
            // 旧片段按自己冻结的速度继续走，避免切片段瞬间姿态跳变
            fade.time += dt * fade.speed;
            finished = fade.elapsed >= fade.duration;
        }
        if finished {
            self.fade = None;
        }
    }

    /// 轨道数（日志用）。
    pub fn track_count(&self) -> usize {
        self.bindings.len()
    }

    /// 换一个片段继续播放（同一骨架的不同动作，如走→跑→攻击）。
    ///
    /// 绑定表按节点下标/路径重新对齐到新片段的轨道，因此不需要重新 spawn 节点树；
    /// 时间归零并恢复播放，符合「换动作从头播」的直觉。找不到轨道的绑定会被丢弃，
    /// 避免每帧做无效查找。
    pub fn set_clip(&mut self, clip: Arc<AnimClip>) {
        self.fade = None;
        self.bindings
            .retain_mut(|b| match clip.track_of(b.node_index, &b.path) {
                Some(track) => {
                    b.track = track;
                    true
                }
                None => false,
            });
        self.clip = clip;
        self.time = 0.0;
        self.playing = true;
    }

    /// 带交叉淡入地换片段：过渡期间旧片段继续推进并逐轨道与新片段加权混合。
    ///
    /// `duration <= 0`（或片段相同）等价于硬切，直接走 [`Self::set_clip`]。
    /// 与硬切的另一处差别是绑定表不裁剪：新片段缺的轨道保留为 [`NO_TRACK`]，
    /// 过渡期间由旧片段继续驱动，过渡结束后该节点停在旧片段的末姿态。
    pub fn cross_fade(&mut self, clip: Arc<AnimClip>, duration: f32) {
        if duration <= 0.0 || clip.name == self.clip.name {
            self.set_clip(clip);
            return;
        }
        let fade = AnimFade {
            clip: self.clip.clone(),
            time: self.time,
            speed: self.speed,
            looping: self.looping,
            tracks: self.bindings.iter().map(|b| b.track).collect(),
            elapsed: 0.0,
            duration,
        };
        for b in self.bindings.iter_mut() {
            b.track = clip.track_of(b.node_index, &b.path).unwrap_or(NO_TRACK);
        }
        self.clip = clip;
        self.fade = Some(fade);
        self.time = 0.0;
        self.playing = true;
    }
}

/// 把轨道的关键帧时间整体平移，使最早一帧落到 0。
///
/// FBX 动画栈常带 `time_begin != 0`（例如 1..25 帧），不重基会让片段开头出现
/// 一段「静止」或直接跳到中段，因此导入时统一归一。
pub fn rebase_track(track: &mut AnimTrack) {
    let Some(min) = track
        .keys
        .iter()
        .map(|k| k.time)
        .fold(None, |acc: Option<f32>, t| {
            Some(acc.map_or(t, |a| a.min(t)))
        })
    else {
        return;
    };
    if min.abs() < 1e-6 {
        return;
    }
    for key in &mut track.keys {
        key.time -= min;
    }
}

/// 片段时长：所有轨道末帧时间的最大值。
pub fn clip_duration(tracks: &[AnimTrack]) -> f32 {
    tracks
        .iter()
        .filter_map(|t| t.keys.last().map(|k| k.time))
        .fold(0.0_f32, f32::max)
}
