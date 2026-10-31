//! r3e-audio —— 非空间化音频播放（M1 最后一项）。
//!
//! 设计要点：
//! * **只做非空间化**：一次播放一条立体声（或单声道）声音，没有 3D 声源衰减 / 多普勒 /
//!   听者朝向。空间化留给后续里程碑，接口刻意不预留半成品参数。
//! * **后端可替换**：真设备走 `rodio`（cpal + symphonia），无声卡走 `SilentBackend`；
//!   门面 [`Audio`] 对上层暴露同一套 API，引擎永远不会因为音频问题 panic。
//! * **单条音量 × 总音量**：门面存单条用户音量，每次下发给后端的是有效音量，
//!   改总音量时逐条重算，不依赖后端支持 master。
//! * **同名 key 重播**：再次 `play` 同一个 key 会停掉旧实例并从头播（不是拒绝、也不是叠加），
//!   这条语义写进 README / ARCHITECTURE，脚本可以直接拿它当「按键连打只响一份」。
//!
//! 范围外：3D 声源、混响、多声道定位、音频流式加载（整文件解码进内存，够演示用）。

pub mod backend;

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

pub use backend::{AudioBackend, RodioBackend, SilentBackend};

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
            self.backend.set_volume(&key, volume * master);
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

        let effective = volume * self.master;
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
        let Some(meta) = self.sounds.get_mut(key) else {
            return false;
        };
        meta.volume = volume;
        self.backend.set_volume(key, volume * master)
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
        let effective = meta.volume * self.master;
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
            "后端 {} / 设备 {} / 在播 {playing}（实例 {}） / 总音量 {:.2} / 开关 {} / 错误 {} / 最近 {}",
            self.backend.name(),
            if self.device_ok { "可用" } else { "不可用（静默降级）" },
            self.sounds.len(),
            self.master,
            if self.enabled { "开" } else { "关" },
            self.error_count,
            self.last_error.as_deref().unwrap_or("无")
        )
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
        stops: u32,
        plays: u32,
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
}
