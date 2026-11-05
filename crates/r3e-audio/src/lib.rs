//! r3e-audio —— 音频播放：非空间化基线 + 3D 空间化扩展。
//!
//! 设计要点：
//! * **非空间化基线不变**：不注册空间化时，[`Audio`] 的行为与历史版本逐位一致
//!   （有效音量 = 单条 × 总音量，倍速恒 1.0）。21 个 `audio_*` API 语义未改。
//! * **空间化是叠加层**（`§12` 第十项）：`spatial` 模块提供纯数学（距离衰减 / 多普勒 /
//!   听者位姿 / 声源推进），[`Audio`] 用「声源 key → [`SpatialSource`]」的旁挂表把它
//!   叠到同一条实例上；声源挂场景节点只是记个名字，解析坐标由 `app` 每帧做。
//! * **听者由活动相机推导**：`app` 每帧用 camera 位置 + 朝向喂 [`Audio::set_listener`]，
//!   本 crate 不依赖 `r3e-core`。
//! * **后端可替换**：真设备走 `rodio`（cpal + symphonia），无声卡走 `SilentBackend`；
//!   门面 [`Audio`] 对上层暴露同一套 API，引擎永远不会因为音频问题 panic。
//! * **单条音量 × 总音量**：门面存单条用户音量，每次下发给后端的是有效音量，
//!   改总音量时逐条重算，不依赖后端支持 master。
//! * **同名 key 重播**：再次 `play` 同一个 key 会停掉旧实例并从头播（不是拒绝、也不是叠加），
//!   这条语义写进 README / ARCHITECTURE，脚本可以直接拿它当「按键连打只响一份」。
//!
//! 范围外：HRTF / 双耳定位、混响、多声道、音频流式加载（整文件解码进内存，够演示用）。

pub mod backend;
pub mod spatial;

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

pub use backend::{AudioBackend, RodioBackend, SilentBackend};
pub use spatial::{
    DistanceModel, ListenerPose, SpatialSettings, SpatialSource, MAX_DOPPLER_SHIFT,
    MAX_SPATIAL_SOURCES,
};

/// 单条 / 总音量上限。大于 1 会把后端推爆，统一夹到 1.0。
pub const MAX_VOLUME: f32 = 1.0;
/// `audio_play` 省略音量时的默认值。
pub const DEFAULT_VOLUME: f32 = 0.8;

/// 音频层错误：文案即给脚本 / 日志看的内容（不暴露 rodio 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioError {
    message: String,
}

impl AudioError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AudioError {}

/// 音量清洗：NaN → 0（静音），负数 → 0，超界 → [`MAX_VOLUME`]。
///
/// 单独公开是为了让脚本侧边界测试和门面用同一套规则，不出现两份 clamp。
pub fn sanitize_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        return 0.0;
    }
    volume.clamp(0.0, MAX_VOLUME)
}

/// 门面里记的一条声音：策略状态都在这里，后端只管出声。
#[derive(Debug, Clone)]
struct SoundMeta {
    /// 单条用户音量（不含总音量）；`set_loop` 重启时要按它重算有效音量。
    volume: f32,
    looping: bool,
    paused: bool,
    /// 重启（`set_loop`）需要重新打开文件，所以路径要留着。
    path: PathBuf,
}

/// 音频门面：引擎里唯一持有音频设备的地方。
pub struct Audio {
    backend: Box<dyn AudioBackend>,
    sounds: HashMap<String, SoundMeta>,
    master: f32,
    enabled: bool,
    device_ok: bool,
    last_error: Option<String>,
    error_count: u64,
    /// 空间化声源（旁挂表）：key 与 `sounds` 同名即生效，缺了就是平面声。
    spatial: HashMap<String, SpatialSource>,
    /// 听者位姿（`app` 每帧按活动相机覆盖）。
    listener: ListenerPose,
    /// 空间化总开关；关闭时所有衰减 / 多普勒退回平面语义。
    spatial_enabled: bool,
}

impl Audio {
    /// 尝试打开默认设备；失败则静默降级（记一条 warn + `last_error`），引擎照常跑。
    pub fn open_default() -> Self {
        match RodioBackend::open() {
            Ok(backend) => Self::from_backend(Box::new(backend)),
            Err(e) => {
                log::warn!("音频设备不可用（{e}），本次运行音频降级为静默模式");
                Self::silent(e.message())
            }
        }
    }

    /// 直接给定后端（测试替身 / 未来换后端都走这里）。
    pub fn from_backend(backend: Box<dyn AudioBackend>) -> Self {
        Self {
            backend,
            sounds: HashMap::new(),
            master: 1.0,
            enabled: true,
            device_ok: true,
            last_error: None,
            error_count: 0,
            spatial: HashMap::new(),
            listener: ListenerPose::default(),
            spatial_enabled: true,
        }
    }

    /// 静默后端：没有设备，但文件照样会被解码校验。
    pub fn silent(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            backend: Box::new(SilentBackend::new(reason.clone())),
            sounds: HashMap::new(),
            master: 1.0,
            enabled: true,
            device_ok: false,
            last_error: Some(reason),
            error_count: 0,
            spatial: HashMap::new(),
            listener: ListenerPose::default(),
            spatial_enabled: true,
        }
    }

    /// 后端名（`rodio` / `silent`）。
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// 设备是否真的可用。false = 静默降级中，`play` 只做校验不出声。
    pub fn device_available(&self) -> bool {
        self.device_ok
    }

    /// 音频通道开关（脚本 `audio_set_enabled`）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 关闭时顺带停掉所有声音，避免「关了还在响」。
    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        if !on {
            self.backend.stop_all();
            self.sounds.clear();
        }
    }

    /// 总音量（0.0–1.0）。
    pub fn master_volume(&self) -> f32 {
        self.master
    }

    /// 改总音量：逐条重算有效音量下发，后端不需要支持 master。
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master = sanitize_volume(volume);
        let master = self.master;
        let entries: Vec<(String, f32)> = self
            .sounds
            .iter()
            .map(|(key, meta)| (key.clone(), meta.volume))
            .collect();
        for (key, volume) in entries {
            let effective = volume * master * self.spatial_gain_of(&key);
            self.backend.set_volume(&key, effective);
        }
    }

    /// 播放一个文件。
    ///
    /// 语义：
    /// * 同名 key 已在播 → **停掉旧实例、从头重播**（不叠加、不拒绝）；
    /// * `volume` 走 [`sanitize_volume`]；
    /// * 文件打不开 / 解码失败 → `Err`（如实记错误，不 panic）；
    /// * 无设备 → 仍会解码校验，校验过就 `Ok`，但不会登记成「在播」。
    pub fn play(
        &mut self,
        key: &str,
        path: impl AsRef<Path>,
        volume: f32,
        looping: bool,
    ) -> Result<(), AudioError> {
        if !self.enabled {
            let err = self.note_error("音频已禁用（audio_set_enabled(false)）");
            return Err(err);
        }
        if key.trim().is_empty() {
            let err = self.note_error("音频 key 不能为空");
            return Err(err);
        }

        let path = path.as_ref();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                let err = self.note_error(format!("打开音频文件失败（{}）：{e}", path.display()));
                return Err(err);
            }
        };

        let volume = sanitize_volume(volume);
        // 重播语义：先把旧实例停掉（后端内部也会再 stop 一次，幂等）
        self.backend.stop(key);
        self.sounds.remove(key);

        // 有效音量 = 单条 × 总音量 ×（空间化衰减；未注册声源时恒 1.0）
        let effective = volume * self.master * self.spatial_gain_of(key);
        let played = self.backend.play(key, file, effective, looping);
        if let Err(e) = played {
            return Err(self.note_error(e.message().to_string()));
        }

        // 静默后端不会真的出声（is_playing 恒 false）——此时不登记，避免「在播数」虚高
        if self.backend.is_playing(key) {
            self.sounds.insert(
                key.to_string(),
                SoundMeta {
                    volume,
                    looping,
                    paused: false,
                    path: path.to_path_buf(),
                },
            );
        }
        Ok(())
    }

    /// 用默认音量、不循环播放。
    pub fn play_default(&mut self, key: &str, path: impl AsRef<Path>) -> Result<(), AudioError> {
        self.play(key, path, DEFAULT_VOLUME, false)
    }

    /// 停止一条声音。
    pub fn stop(&mut self, key: &str) -> bool {
        let known = self.sounds.remove(key).is_some();
        let stopped = self.backend.stop(key);
        known || stopped
    }

    /// 停掉全部。
    pub fn stop_all(&mut self) {
        self.sounds.clear();
        self.backend.stop_all();
    }

    /// 暂停（保留位置）。静默模式下返回 false。
    pub fn pause(&mut self, key: &str) -> bool {
        let ok = self.backend.pause(key);
        if ok {
            if let Some(meta) = self.sounds.get_mut(key) {
                meta.paused = true;
            }
        }
        ok
    }

    /// 从暂停处继续。
    pub fn resume(&mut self, key: &str) -> bool {
        let ok = self.backend.resume(key);
        if ok {
            if let Some(meta) = self.sounds.get_mut(key) {
                meta.paused = false;
            }
        }
        ok
    }

    /// 改单条音量（夹到 0–1，再乘总音量下发）。
    pub fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        let volume = sanitize_volume(volume);
        let master = self.master;
        let gain = self.spatial_gain_of(key);
        let Some(meta) = self.sounds.get_mut(key) else {
            return false;
        };
        meta.volume = volume;
        self.backend.set_volume(key, volume * master * gain)
    }

    /// 改循环标志。
    ///
    /// **会从头重播**：rodio 的循环是在解码源构造时决定的，改标志只能重新 append 一次；
    /// 这与「同名 key 重播」是同一套语义，文档里合并说明。
    pub fn set_loop(&mut self, key: &str, looping: bool) -> bool {
        let Some(meta) = self.sounds.get(key).cloned() else {
            return false;
        };
        if meta.looping == looping {
            return true;
        }

        let file = match File::open(&meta.path) {
            Ok(f) => f,
            Err(e) => {
                self.note_error(format!("重播失败（{}）：{e}", meta.path.display()));
                self.sounds.remove(key);
                self.backend.stop(key);
                return false;
            }
        };
        let effective = meta.volume * self.master * self.spatial_gain_of(key);
        if let Some(m) = self.sounds.get_mut(key) {
            m.looping = looping;
            m.paused = false;
        }
        if let Err(e) = self.backend.play(key, file, effective, looping) {
            self.note_error(e.message().to_string());
            self.sounds.remove(key);
            return false;
        }
        true
    }

    /// 是否正在出声（暂停 / 播完都算 false）。
    pub fn is_playing(&mut self, key: &str) -> bool {
        match self.sounds.get(key) {
            Some(meta) if !meta.paused => self.backend.is_playing(key),
            _ => false,
        }
    }

    /// 是否暂停。
    pub fn is_paused(&self, key: &str) -> bool {
        self.sounds.get(key).map(|m| m.paused).unwrap_or(false)
    }

    /// 在播（含暂停）的实例数。
    pub fn active_count(&self) -> usize {
        self.sounds.len()
    }

    /// 当前实例 key（有序，方便日志对比）。
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.sounds.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 回收已经播完的实例，返回被回收的 key。主循环每帧调一次。
    pub fn update(&mut self) -> Vec<String> {
        let finished = self.backend.update();
        for key in &finished {
            self.sounds.remove(key);
        }
        finished
    }

    /// 最近一条错误（含降级原因）。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// 真正的失败次数（降级本身不计入）。
    pub fn error_count(&self) -> u64 {
        self.error_count
    }

    /// 一行状态摘要，进启动日志 / 收尾探针。
    pub fn status_line(&mut self) -> String {
        let playing = self
            .keys()
            .into_iter()
            .filter(|k| self.is_playing(k))
            .count();
        format!(
            "后端 {} / 设备 {} / 在播 {playing}（实例 {}） / 总音量 {:.2} / 开关 {} / 空间化 {} / 错误 {} / 最近 {}",
            self.backend.name(),
            if self.device_ok { "可用" } else { "不可用（静默降级）" },
            self.sounds.len(),
            self.master,
            if self.enabled { "开" } else { "关" },
            self.spatial_brief(),
            self.error_count,
            self.last_error.as_deref().unwrap_or("无")
        )
    }

    // ------------------------------------------------------------------
    // 3D 空间化音频（§12 第十项）
    //
    // 旁挂式设计：`spatial` 表里没有的 key 一律按平面声处理，因此既有 21 个
    // `audio_*` API 的语义与历史版本逐位一致（不注册就完全不受影响）。
    // ------------------------------------------------------------------

    /// 空间化总开关（关掉 = 所有已注册声源退回平面音量 / 倍速 1.0）。
    pub fn spatial_enabled(&self) -> bool {
        self.spatial_enabled
    }

    /// 开关空间化：立即把在播实例的音量 / 倍速拉回平面值（或重新按位姿下发）。
    pub fn set_spatial_enabled(&mut self, on: bool) {
        if self.spatial_enabled == on {
            return;
        }
        self.spatial_enabled = on;
        for key in self.spatial_keys() {
            self.push_spatial_to_backend(&key);
        }
    }

    /// 设置听者位姿。非法输入（NaN / 零长度朝向）会被收敛成默认值，不进距离公式。
    pub fn set_listener(&mut self, pose: ListenerPose) {
        self.listener = ListenerPose::new(pose.position, pose.forward);
    }

    /// 当前听者位姿。
    pub fn listener(&self) -> ListenerPose {
        self.listener
    }

    /// 把 key 注册成 3D 声源。
    ///
    /// * 位置先落原点，随后用 [`Audio::set_source_position`] 由脚本喂，或交给 `app`
    ///   每帧按绑定节点解析后喂；
    /// * 已注册则只更新设置，**保留位姿**（脚本调完参数不用重新摆位置）；
    /// * 超过 [`MAX_SPATIAL_SOURCES`] 拒绝并记错误（不挤掉已有声源）。
    pub fn attach_spatial(&mut self, key: &str, settings: SpatialSettings) -> bool {
        if key.trim().is_empty() {
            return false;
        }
        let settings = settings.sanitized();
        if let Some(src) = self.spatial.get_mut(key) {
            src.settings = settings;
            return true;
        }
        if self.spatial.len() >= MAX_SPATIAL_SOURCES {
            self.note_error(format!(
                "空间化声源已达上限（{MAX_SPATIAL_SOURCES}），`{key}` 未注册"
            ));
            return false;
        }
        self.spatial.insert(
            key.to_string(),
            SpatialSource::new([0.0, 0.0, 0.0], settings),
        );
        true
    }

    /// 注册成 3D 声源并记住绑在哪个场景节点上（`app` 每帧解析该节点世界坐标）。
    ///
    /// `node` 只做记录：本 crate 不依赖 `r3e-core`，把节点名解析成坐标是 `app` 的职责。
    pub fn attach_spatial_node(
        &mut self,
        key: &str,
        node: &str,
        settings: SpatialSettings,
    ) -> bool {
        if !self.attach_spatial(key, settings) {
            return false;
        }
        if let Some(src) = self.spatial.get_mut(key) {
            src.node = if node.trim().is_empty() {
                None
            } else {
                Some(node.to_string())
            };
        }
        true
    }

    /// 声源绑定的场景节点名（未绑定 / 未注册 → `None`）。
    pub fn spatial_node(&self, key: &str) -> Option<&str> {
        self.spatial.get(key).and_then(|s| s.node.as_deref())
    }

    /// 所有「声源 key → 场景节点」绑定（按 key 有序，`app` 每帧据此解析坐标）。
    pub fn spatial_bindings(&self) -> Vec<(String, String)> {
        let mut bindings: Vec<(String, String)> = self
            .spatial
            .iter()
            .filter_map(|(key, src)| src.node.clone().map(|node| (key.clone(), node)))
            .collect();
        bindings.sort();
        bindings
    }

    /// 解除注册（**不影响播放**：声音继续以平面音量播着，只是不再被衰减）。
    pub fn detach_spatial(&mut self, key: &str) -> bool {
        if self.spatial.remove(key).is_none() {
            return false;
        }
        self.push_spatial_to_backend(key);
        true
    }

    /// 清空全部空间化声源（同样不影响播放）。
    pub fn clear_spatial(&mut self) {
        let keys = self.spatial_keys();
        self.spatial.clear();
        for key in keys {
            self.push_spatial_to_backend(&key);
        }
    }

    /// 设置声源世界坐标（非有限输入收敛到原点）。下一帧 [`Audio::tick_spatial`] 生效。
    pub fn set_source_position(&mut self, key: &str, position: [f32; 3]) -> bool {
        let position = spatial::sanitize_position(position);
        match self.spatial.get_mut(key) {
            Some(src) => {
                src.position = position;
                true
            }
            None => false,
        }
    }

    /// 声源当前世界坐标。
    pub fn source_position(&self, key: &str) -> Option<[f32; 3]> {
        self.spatial.get(key).map(|s| s.position)
    }

    /// 改某条声源的调参（不重置位姿）。
    pub fn set_spatial_settings(&mut self, key: &str, settings: SpatialSettings) -> bool {
        let settings = settings.sanitized();
        match self.spatial.get_mut(key) {
            Some(src) => {
                src.settings = settings;
                true
            }
            None => false,
        }
    }

    /// 某条声源的调参（未注册 → `None`）。
    pub fn spatial_settings(&self, key: &str) -> Option<SpatialSettings> {
        self.spatial.get(key).map(|s| s.settings)
    }

    /// 已注册声源的 key（有序）。
    pub fn spatial_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.spatial.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 已注册声源数。
    pub fn spatial_count(&self) -> usize {
        self.spatial.len()
    }

    /// 某条声源最近一次算出的距离增益（未注册 / 关掉 → `1.0`）。
    pub fn spatial_gain(&self, key: &str) -> f32 {
        self.spatial_gain_of(key)
    }

    /// 某条声源最近一次算出的播放倍速（未注册 / 关掉 → `1.0`）。
    pub fn spatial_rate(&self, key: &str) -> f32 {
        self.spatial_rate_of(key)
    }

    /// 某条声源最近一次算出的到听者距离（米；未注册 → `None`）。
    pub fn spatial_distance(&self, key: &str) -> Option<f32> {
        self.spatial.get(key).map(|s| s.distance)
    }

    /// 推进一帧空间化：按听者位姿重算每条声源的衰减与多普勒，并下发到后端。
    ///
    /// 返回本次推进的声源数。`dt` 非有限或 <= 0 时只重算增益、不更新速度
    /// （暂停时相机还在动，距离衰减仍要跟着变），因此不会产生 NaN 或负倍速。
    pub fn tick_spatial(&mut self, dt: f32) -> usize {
        if self.spatial.is_empty() {
            return 0;
        }
        let dt = if dt.is_finite() && dt > 0.0 {
            dt.min(0.25)
        } else {
            0.0
        };
        let listener = self.listener;
        let mut keys = Vec::with_capacity(self.spatial.len());
        for (key, src) in self.spatial.iter_mut() {
            src.advance(listener, dt);
            keys.push(key.clone());
        }
        keys.sort();
        for key in &keys {
            self.push_spatial_to_backend(key);
        }
        keys.len()
    }

    /// 一行空间化摘要（进启动日志 / 收尾探针）。
    pub fn spatial_status_line(&self) -> String {
        let backend = if self.backend.supports_speed() {
            "倍速可用"
        } else {
            "倍速不支持（多普勒只算不下发）"
        };
        let nearest = self
            .spatial
            .iter()
            .min_by(|a, b| a.1.distance.total_cmp(&b.1.distance))
            .map(|(key, src)| {
                format!(
                    "{key} @ {:.1}m / gain {:.2} / rate {:.2}",
                    src.distance, src.gain, src.rate
                )
            })
            .unwrap_or_else(|| "无".to_string());
        format!(
            "空间化 {} / 听者 [{:.1}, {:.1}, {:.1}] / 声源 {}（上限 {}） / {} / 最近 {}",
            if self.spatial_enabled { "开" } else { "关" },
            self.listener.position[0],
            self.listener.position[1],
            self.listener.position[2],
            self.spatial.len(),
            MAX_SPATIAL_SOURCES,
            backend,
            nearest
        )
    }

    /// 后端是否支持实时变速（多普勒落地前提；静默后端返回 false）。
    pub fn supports_speed(&self) -> bool {
        self.backend.supports_speed()
    }

    /// 距离增益（关掉空间化 / 未注册 → 1.0，即平面语义）。
    fn spatial_gain_of(&self, key: &str) -> f32 {
        if !self.spatial_enabled {
            return 1.0;
        }
        match self.spatial.get(key) {
            Some(src) if src.gain.is_finite() => src.gain.clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    /// 播放倍速（夹在 `1/MAX_DOPPLER_SHIFT ..= MAX_DOPPLER_SHIFT`，绝不放出 0 或负数）。
    fn spatial_rate_of(&self, key: &str) -> f32 {
        if !self.spatial_enabled {
            return 1.0;
        }
        match self.spatial.get(key) {
            Some(src) if src.rate.is_finite() => {
                src.rate.clamp(1.0 / MAX_DOPPLER_SHIFT, MAX_DOPPLER_SHIFT)
            }
            _ => 1.0,
        }
    }

    /// 把某条声源当前的音量 / 倍速下发（未在播则啥也不做）。
    fn push_spatial_to_backend(&mut self, key: &str) {
        let gain = self.spatial_gain_of(key);
        let rate = self.spatial_rate_of(key);
        let master = self.master;
        let Some(volume) = self.sounds.get(key).map(|m| m.volume) else {
            return;
        };
        self.backend.set_volume(key, volume * master * gain);
        self.backend.set_speed(key, rate);
    }

    /// 「开（3 源）」这类短摘要，塞进 [`Audio::status_line`]。
    fn spatial_brief(&self) -> String {
        let state = if self.spatial_enabled { "开" } else { "关" };
        if self.spatial.is_empty() {
            format!("{state}（0 源）")
        } else {
            format!("{state}（{} 源）", self.spatial.len())
        }
    }

    fn note_error(&mut self, message: impl Into<String>) -> AudioError {
        let err = AudioError::new(message);
        self.error_count += 1;
        self.last_error = Some(err.message().to_string());
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    /// 测试 wav：16-bit 单声道 8kHz 正弦，20ms 就够解码器认格式。
    fn write_test_wav(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("r3e_audio_test_{}_{tag}.wav", std::process::id()));
        let rate: u32 = 8000;
        let samples: u32 = rate / 50; // 20ms
        let data_len = samples * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let t = i as f32 / rate as f32;
            let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2;
            bytes.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("写测试 wav");
        path
    }

    fn write_garbage(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("r3e_audio_test_{}_{tag}.bin", std::process::id()));
        let mut f = File::create(&path).expect("写垃圾文件");
        f.write_all(b"this is definitely not audio").expect("写入");
        path
    }

    #[derive(Default)]
    struct MockState {
        playing: Vec<(String, f32, bool)>,
        /// 最近下发的倍速（多普勒落点）。
        speeds: Vec<(String, f32)>,
        stops: u32,
        plays: u32,
    }

    impl MockState {
        fn volume_of(&self, key: &str) -> Option<f32> {
            self.playing
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, v, _)| *v)
        }

        fn speed_of(&self, key: &str) -> Option<f32> {
            self.speeds
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, s)| *s)
        }
    }

    struct MockBackend {
        state: Rc<RefCell<MockState>>,
    }

    impl MockBackend {
        fn new() -> (Self, Rc<RefCell<MockState>>) {
            let state = Rc::new(RefCell::new(MockState::default()));
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }
    }

    impl AudioBackend for MockBackend {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn play(
            &mut self,
            key: &str,
            _file: File,
            volume: f32,
            looping: bool,
        ) -> Result<(), AudioError> {
            let mut state = self.state.borrow_mut();
            state.plays += 1;
            state.playing.retain(|(k, _, _)| k != key);
            state.playing.push((key.to_string(), volume, looping));
            Ok(())
        }

        fn stop(&mut self, key: &str) -> bool {
            let mut state = self.state.borrow_mut();
            let before = state.playing.len();
            state.playing.retain(|(k, _, _)| k != key);
            let hit = state.playing.len() != before;
            if hit {
                state.stops += 1;
            }
            hit
        }

        fn stop_all(&mut self) {
            let mut state = self.state.borrow_mut();
            state.stops += state.playing.len() as u32;
            state.playing.clear();
        }

        fn pause(&mut self, _key: &str) -> bool {
            false
        }

        fn resume(&mut self, _key: &str) -> bool {
            false
        }

        fn set_volume(&mut self, key: &str, volume: f32) -> bool {
            let mut state = self.state.borrow_mut();
            match state.playing.iter_mut().find(|(k, _, _)| k == key) {
                Some(entry) => {
                    entry.1 = volume;
                    true
                }
                None => false,
            }
        }

        fn set_speed(&mut self, key: &str, speed: f32) -> bool {
            let mut state = self.state.borrow_mut();
            if !state.playing.iter().any(|(k, _, _)| k == key) {
                return false;
            }
            state.speeds.retain(|(k, _)| k != key);
            state.speeds.push((key.to_string(), speed));
            true
        }

        fn supports_speed(&self) -> bool {
            true
        }

        fn is_playing(&mut self, key: &str) -> bool {
            self.state.borrow().playing.iter().any(|(k, _, _)| k == key)
        }

        fn is_paused(&mut self, _key: &str) -> bool {
            false
        }

        fn active_keys(&mut self) -> Vec<String> {
            self.state
                .borrow()
                .playing
                .iter()
                .map(|(k, _, _)| k.clone())
                .collect()
        }

        fn update(&mut self) -> Vec<String> {
            Vec::new()
        }
    }

    fn mock_audio() -> (Audio, Rc<RefCell<MockState>>) {
        let (backend, state) = MockBackend::new();
        (Audio::from_backend(Box::new(backend)), state)
    }

    #[test]
    fn missing_file_returns_error_and_is_recorded() {
        let mut audio = Audio::silent("测试注入：无音频设备");
        let err = audio
            .play("boom", "Z:/no/such/dir/none.wav", 1.0, false)
            .expect_err("缺失文件必须报错");
        assert!(err.message().contains("打开音频文件失败"), "{err}");
        assert_eq!(audio.error_count(), 1);
        assert!(audio
            .last_error()
            .expect("必须有最近错误")
            .contains("打开音频文件失败"));
        assert_eq!(audio.active_count(), 0);
    }

    #[test]
    fn invalid_content_returns_error_not_panic() {
        let garbage = write_garbage("decode");
        let mut audio = Audio::silent("测试注入：无音频设备");
        let err = audio
            .play("bad", &garbage, 1.0, false)
            .expect_err("非音频内容必须报错");
        assert!(err.message().contains("解码音频失败"), "{err}");
        let _ = std::fs::remove_file(&garbage);
    }

    #[test]
    fn volume_is_clamped_at_boundaries() {
        let wav = write_test_wav("clamp");
        let (mut audio, state) = mock_audio();

        audio.set_master_volume(2.0);
        assert_eq!(audio.master_volume(), MAX_VOLUME);
        audio.set_master_volume(-0.5);
        assert_eq!(audio.master_volume(), 0.0);
        audio.set_master_volume(f32::NAN);
        assert_eq!(audio.master_volume(), 0.0);
        audio.set_master_volume(1.0);
        assert_eq!(audio.master_volume(), 1.0);

        audio.play("hit", &wav, 5.0, false).expect("播放应成功");
        // 有效音量 = clamp(5.0) * 1.0 = 1.0
        assert_eq!(state.borrow().playing[0].1, 1.0);

        // 单条音量同样夹到 0–1
        assert!(audio.set_volume("hit", 3.0));
        assert_eq!(state.borrow().playing[0].1, 1.0);
        assert!(audio.set_volume("hit", -2.0));
        assert_eq!(state.borrow().playing[0].1, 0.0);
        // 总音量 0.5 时下发值是 0.5 × 单条
        assert!(audio.set_volume("hit", 1.0));
        audio.set_master_volume(0.5);
        assert_eq!(state.borrow().playing[0].1, 0.5);

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn duplicate_key_restarts_instead_of_stacking() {
        let wav = write_test_wav("dupe");
        let (mut audio, state) = mock_audio();

        audio.play("hit", &wav, 1.0, false).expect("首次播放");
        audio.play("hit", &wav, 1.0, false).expect("重播");
        assert_eq!(audio.active_count(), 1, "同名 key 只能有一个实例");
        assert_eq!(state.borrow().plays, 2);
        assert_eq!(state.borrow().stops, 1, "旧实例必须被停掉");

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn loop_toggle_restarts_and_is_idempotent() {
        let wav = write_test_wav("loop");
        let (mut audio, state) = mock_audio();

        audio.play("pad", &wav, 0.5, false).expect("播放");
        assert!(audio.set_loop("pad", true));
        assert_eq!(state.borrow().plays, 2, "开循环需要重播一次");
        assert!(state.borrow().playing[0].2, "重播实例应带循环标志");
        // 同值重复设置不重播
        assert!(audio.set_loop("pad", true));
        assert_eq!(state.borrow().plays, 2);
        assert!(audio.set_loop("pad", false));
        assert_eq!(state.borrow().plays, 3);
        // 不存在的 key 返回 false
        assert!(!audio.set_loop("ghost", true));

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn silent_backend_accepts_valid_file_without_playing() {
        let wav = write_test_wav("silent");
        let mut audio = Audio::silent("测试注入：无音频设备");
        assert!(!audio.device_available());
        assert_eq!(audio.backend_name(), "silent");

        // 解码通过 → Ok（静默降级），但不登记成「在播」
        audio
            .play("beep", &wav, 0.8, false)
            .expect("静默模式应接受");
        assert!(!audio.is_playing("beep"));
        assert_eq!(audio.active_count(), 0);
        assert_eq!(audio.error_count(), 0, "降级不算错误");
        assert!(audio.last_error().expect("降级原因").contains("无音频设备"));

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn disabled_channel_rejects_play_and_stops_all() {
        let wav = write_test_wav("disable");
        let (mut audio, state) = mock_audio();

        audio.set_enabled(false);
        assert!(audio.play("hit", &wav, 1.0, false).is_err());
        assert_eq!(audio.error_count(), 1);

        audio.set_enabled(true);
        audio.play("hit", &wav, 1.0, false).expect("重新开启后能播");
        assert_eq!(audio.active_count(), 1);
        audio.set_enabled(false);
        assert_eq!(audio.active_count(), 0, "关闭必须停掉在播实例");
        assert_eq!(state.borrow().stops, 1);

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn empty_key_is_rejected() {
        let wav = write_test_wav("empty");
        let (mut audio, _state) = mock_audio();
        assert!(audio.play("  ", &wav, 1.0, false).is_err());
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn finished_sounds_are_reaped() {
        struct FinishingBackend {
            finished: bool,
        }
        impl AudioBackend for FinishingBackend {
            fn name(&self) -> &'static str {
                "finishing"
            }
            fn play(
                &mut self,
                _key: &str,
                _file: File,
                _volume: f32,
                _looping: bool,
            ) -> Result<(), AudioError> {
                self.finished = false;
                Ok(())
            }
            fn stop(&mut self, _key: &str) -> bool {
                false
            }
            fn stop_all(&mut self) {}
            fn pause(&mut self, _key: &str) -> bool {
                false
            }
            fn resume(&mut self, _key: &str) -> bool {
                false
            }
            fn set_volume(&mut self, _key: &str, _volume: f32) -> bool {
                false
            }
            fn is_playing(&mut self, _key: &str) -> bool {
                !self.finished
            }
            fn is_paused(&mut self, _key: &str) -> bool {
                false
            }
            fn active_keys(&mut self) -> Vec<String> {
                Vec::new()
            }
            fn update(&mut self) -> Vec<String> {
                if self.finished {
                    return Vec::new();
                }
                self.finished = true;
                vec!["once".to_string()]
            }
        }

        let wav = write_test_wav("reap");
        let mut audio = Audio::from_backend(Box::new(FinishingBackend { finished: true }));
        audio.play("once", &wav, 1.0, false).expect("播放");
        assert_eq!(audio.active_count(), 1);
        let reaped = audio.update();
        assert_eq!(reaped, vec!["once".to_string()]);
        assert_eq!(audio.active_count(), 0);

        let _ = std::fs::remove_file(&wav);
    }

    // ------------------------------------------------------------------
    // 空间化音频（§12 第十项）：叠加语义、下发、降级、边界
    // ------------------------------------------------------------------

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn spatial_未注册声源语义完全不变() {
        let wav = write_test_wav("spatial_plain");
        let (mut audio, state) = mock_audio();
        audio.set_master_volume(0.5);
        audio.play("hit", &wav, 1.0, false).expect("播放应成功");
        assert!(near(state.borrow().volume_of("hit").expect("在播"), 0.5));

        // 注册另一条声源 + 推进听者帧：老实例音量不该被碰
        assert!(audio.attach_spatial("far", SpatialSettings::new(1.0, 10.0)));
        audio.set_listener(ListenerPose::from_position([0.0, 0.0, 0.0]));
        assert_eq!(audio.tick_spatial(0.016), 1);
        assert!(near(state.borrow().volume_of("hit").expect("在播"), 0.5));
        assert_eq!(audio.spatial_gain("hit"), 1.0);
        assert_eq!(audio.spatial_rate("hit"), 1.0);
        assert_eq!(audio.spatial_distance("hit"), None);
        // 平面语义：倍速恒 1.0，从未下发过变速
        assert_eq!(state.borrow().speed_of("hit"), None);

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn spatial_距离越远下发音量越低() {
        let wav = write_test_wav("spatial_falloff");
        let (mut audio, state) = mock_audio();
        audio.set_listener(ListenerPose::from_position([0.0, 0.0, 0.0]));
        assert!(audio.attach_spatial("hit", SpatialSettings::new(1.0, 20.0)));
        audio.play("hit", &wav, 1.0, false).expect("播放应成功");

        audio.set_source_position("hit", [0.0, 0.0, 3.0]);
        audio.tick_spatial(0.5);
        let near_volume = state.borrow().volume_of("hit").expect("应已下发");

        audio.set_source_position("hit", [0.0, 0.0, 15.0]);
        audio.tick_spatial(0.5);
        let far_volume = state.borrow().volume_of("hit").expect("应已下发");

        assert!(
            near_volume > far_volume,
            "近处应比远处响：{near_volume} vs {far_volume}"
        );
        assert!((0.0..=1.0).contains(&near_volume));
        assert!((0.0..=1.0).contains(&far_volume));
        assert!(near(audio.spatial_distance("hit").expect("有距离"), 15.0));
        assert!(near(audio.spatial_gain("hit"), far_volume / 1.0));

        // 超出 max_distance → 完全听不见（增益 0，而不是"很小声"）
        audio.set_source_position("hit", [0.0, 0.0, 25.0]);
        audio.tick_spatial(0.5);
        assert_eq!(audio.spatial_gain("hit"), 0.0);
        assert_eq!(state.borrow().volume_of("hit"), Some(0.0));

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn spatial_关掉后回到平面音量() {
        let wav = write_test_wav("spatial_off");
        let (mut audio, state) = mock_audio();
        audio.set_listener(ListenerPose::from_position([0.0, 0.0, 0.0]));
        assert!(audio.attach_spatial("hit", SpatialSettings::new(1.0, 10.0)));
        audio.play("hit", &wav, 1.0, false).expect("播放应成功");

        audio.set_source_position("hit", [0.0, 0.0, 9.0]);
        audio.tick_spatial(0.5);
        let quiet = state.borrow().volume_of("hit").expect("应已下发");
        assert!(quiet < 1.0, "远处应先衰减：{quiet}");

        assert!(audio.spatial_enabled());
        audio.set_spatial_enabled(false);
        assert!(!audio.spatial_enabled());
        // 关掉即刻拉回平面值，不必等下一帧
        assert!(near(
            state.borrow().volume_of("hit").expect("应已下发"),
            1.0
        ));
        assert_eq!(audio.spatial_gain("hit"), 1.0);
        assert_eq!(audio.spatial_rate("hit"), 1.0);

        audio.tick_spatial(0.5);
        assert!(near(
            state.borrow().volume_of("hit").expect("应已下发"),
            1.0
        ));

        // 再打开：按当前位姿重新衰减
        audio.set_spatial_enabled(true);
        assert!(state.borrow().volume_of("hit").expect("应已下发") < 1.0);

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn spatial_解除注册恢复平面音量与倍速() {
        let wav = write_test_wav("spatial_detach");
        let (mut audio, state) = mock_audio();
        audio.set_listener(ListenerPose::from_position([0.0, 0.0, 0.0]));
        assert!(audio.attach_spatial("hit", SpatialSettings::new(1.0, 10.0)));
        audio.play("hit", &wav, 1.0, false).expect("播放应成功");
        audio.set_source_position("hit", [0.0, 0.0, 8.0]);
        audio.tick_spatial(0.5);
        assert!(state.borrow().volume_of("hit").expect("在播") < 1.0);

        assert!(audio.detach_spatial("hit"));
        assert_eq!(audio.spatial_count(), 0);
        assert!(near(state.borrow().volume_of("hit").expect("在播"), 1.0));
        assert_eq!(state.borrow().speed_of("hit"), Some(1.0));
        // 幂等：再解一次返回 false，不 panic
        assert!(!audio.detach_spatial("hit"));
        // 解除注册不等于停止播放
        assert_eq!(audio.active_count(), 1);

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn spatial_静默后端长时间推进不_panic() {
        let wav = write_test_wav("spatial_silent");
        let mut audio = Audio::silent("测试注入：无音频设备");
        assert!(!audio.supports_speed(), "静默后端不该声称支持变速");
        assert!(audio.attach_spatial("env", SpatialSettings::new(0.5, 30.0)));
        audio
            .play("env", &wav, 1.0, true)
            .expect("静默后端也应解码通过");

        audio.set_listener(ListenerPose::new([1.0, 2.0, 3.0], [0.0, 0.0, -1.0]));
        for i in 0..200 {
            audio.set_source_position("env", [i as f32 * 0.5, 0.0, 5.0]);
            assert_eq!(audio.tick_spatial(0.016), 1);
        }
        let gain = audio.spatial_gain("env");
        let rate = audio.spatial_rate("env");
        assert!(gain.is_finite() && (0.0..=1.0).contains(&gain), "{gain}");
        assert!(rate.is_finite() && rate > 0.0, "{rate}");
        let line = audio.spatial_status_line();
        assert!(line.contains("不支持"), "{line}");
        assert_eq!(audio.error_count(), 0, "没声卡不算脚本错误");
        assert_eq!(audio.active_count(), 0, "静默后端不登记在播实例");

        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn spatial_声源数量有上限且可覆盖更新() {
        let mut audio = Audio::silent("测试注入：无音频设备");
        for i in 0..MAX_SPATIAL_SOURCES {
            assert!(
                audio.attach_spatial(&format!("s{i}"), SpatialSettings::default()),
                "第 {i} 条应注册成功"
            );
        }
        assert_eq!(audio.spatial_count(), MAX_SPATIAL_SOURCES);

        // 超限：拒绝并记账，不 panic、不挤掉已有声源
        assert!(!audio.attach_spatial("overflow", SpatialSettings::default()));
        assert_eq!(audio.spatial_count(), MAX_SPATIAL_SOURCES);
        assert_eq!(audio.error_count(), 1);
        assert!(audio.last_error().expect("应有错误").contains("上限"));

        // 空 key 直接拒绝
        assert!(!audio.attach_spatial("   ", SpatialSettings::default()));

        // 已注册的可以改设置（不占新名额、保留位姿）
        audio.set_source_position("s0", [1.0, 2.0, 3.0]);
        assert!(audio.attach_spatial("s0", SpatialSettings::new(2.0, 8.0)));
        assert_eq!(audio.spatial_count(), MAX_SPATIAL_SOURCES);
        assert_eq!(audio.source_position("s0"), Some([1.0, 2.0, 3.0]));
        assert!(near(
            audio.spatial_settings("s0").expect("有设置").max_distance,
            8.0
        ));

        // 清空后能重新注册（配额释放）
        audio.clear_spatial();
        assert_eq!(audio.spatial_count(), 0);
        assert!(audio.attach_spatial("fresh", SpatialSettings::default()));
    }

    #[test]
    fn spatial_非法参数被收敛而不是产生_nan() {
        let mut audio = Audio::silent("测试注入：无音频设备");
        assert!(audio.attach_spatial(
            "bad",
            SpatialSettings {
                min_distance: f32::NAN,
                max_distance: -3.0,
                doppler_factor: f32::INFINITY,
                speed_of_sound: 0.0,
                ..SpatialSettings::default()
            }
        ));
        let settings = audio.spatial_settings("bad").expect("有设置");
        assert!(
            settings.max_distance > settings.min_distance,
            "{settings:?}"
        );
        assert!(settings.speed_of_sound > 1.0, "{settings:?}");

        // 非有限坐标 → 退回原点，不进距离公式
        audio.set_source_position("bad", [f32::NAN, 0.0, 0.0]);
        assert_eq!(audio.source_position("bad"), Some([0.0, 0.0, 0.0]));
        // 非法听者位姿 → 退回默认
        audio.set_listener(ListenerPose::new(
            [f32::INFINITY, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        ));
        assert_eq!(audio.listener().position, [0.0, 0.0, 0.0]);
        assert_eq!(audio.listener().forward, [0.0, 0.0, -1.0]);

        // 非法 dt 不产生 NaN 增益 / 负倍速
        audio.tick_spatial(f32::NAN);
        audio.tick_spatial(-1.0);
        audio.tick_spatial(0.0);
        let gain = audio.spatial_gain("bad");
        let rate = audio.spatial_rate("bad");
        assert!(gain.is_finite() && (0.0..=1.0).contains(&gain), "{gain}");
        assert!(rate.is_finite() && rate > 0.0, "{rate}");
        // 未注册的 key 一律按平面值
        assert_eq!(audio.spatial_gain("nope"), 1.0);
        assert_eq!(audio.spatial_rate("nope"), 1.0);
        assert_eq!(audio.spatial_distance("nope"), None);
        assert!(!audio.set_source_position("nope", [1.0, 0.0, 0.0]));
        assert!(!audio.set_spatial_settings("nope", SpatialSettings::default()));
    }

    #[test]
    fn spatial_节点绑定可查询且有序() {
        let mut audio = Audio::silent("测试注入：无音频设备");
        assert!(audio.attach_spatial_node("drum", "drum_node", SpatialSettings::default()));
        assert!(audio.attach_spatial_node("bell", "bell_node", SpatialSettings::default()));
        assert!(audio.attach_spatial("plain", SpatialSettings::default()));

        assert_eq!(audio.spatial_node("bell"), Some("bell_node"));
        assert_eq!(audio.spatial_node("plain"), None);
        assert_eq!(audio.spatial_node("missing"), None);
        assert_eq!(
            audio.spatial_keys(),
            vec!["bell".to_string(), "drum".to_string(), "plain".to_string()]
        );
        assert_eq!(
            audio.spatial_bindings(),
            vec![
                ("bell".to_string(), "bell_node".to_string()),
                ("drum".to_string(), "drum_node".to_string()),
            ]
        );

        // 空节点名 = 解绑（声源仍在，只是不再由节点驱动）
        assert!(audio.attach_spatial_node("bell", "  ", SpatialSettings::default()));
        assert_eq!(audio.spatial_node("bell"), None);
        assert_eq!(audio.spatial_bindings().len(), 1);

        let line = audio.spatial_status_line();
        assert!(line.contains("听者") && line.contains("声源 3"), "{line}");
    }

    #[test]
    fn spatial_多普勒倍速随靠近抬高并下发() {
        let wav = write_test_wav("spatial_doppler");
        let (mut audio, state) = mock_audio();
        assert!(audio.supports_speed());
        audio.set_listener(ListenerPose::from_position([0.0, 0.0, 0.0]));
        assert!(audio.attach_spatial("pass", SpatialSettings::new(1.0, 100.0)));
        audio.play("pass", &wav, 1.0, false).expect("播放应成功");

        // 以 10 m/s 沿 -Z 冲向听者（每帧 0.1s 走 1m）
        let mut z = 60.0_f32;
        for _ in 0..40 {
            z -= 1.0;
            audio.set_source_position("pass", [0.0, 0.0, z]);
            audio.tick_spatial(0.1);
        }
        let rate = state.borrow().speed_of("pass").expect("应已下发倍速");
        assert!(rate > 1.0, "冲过来音高应升：{rate}");
        assert!(rate <= MAX_DOPPLER_SHIFT + 1e-4, "{rate}");

        // 掉头远离 → 音高降回去
        for _ in 0..60 {
            z += 1.0;
            audio.set_source_position("pass", [0.0, 0.0, z]);
            audio.tick_spatial(0.1);
        }
        let rate = state.borrow().speed_of("pass").expect("应已下发倍速");
        assert!(rate < 1.0, "远离音高应降：{rate}");
        assert!(rate >= 1.0 / MAX_DOPPLER_SHIFT - 1e-4, "{rate}");

        let _ = std::fs::remove_file(&wav);
    }
}
