//! 音频后端：`rodio`（真设备）与 `SilentBackend`（无设备 / 静默降级）。
//!
//! 分层约定：
//! * 后端**只**负责「把已经打开的音频文件变成声音」，不知道总音量、不喜欢 key 的语义；
//! * 音量合成（单条音量 × 总音量）、同名 key 重播、暂停状态这些策略都在 [`crate::Audio`] 门面里；
//! * 因此后端可以被测试替身（MockBackend）整体替换——测试不需要真声卡。

use std::fs::File;

use rodio::source::Source;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::Player;

use crate::AudioError;

/// 后端契约。所有方法都以 `key` 定位一条正在播放的声音。
pub trait AudioBackend {
    /// 后端名，进日志 / HUD / `audio_backend()`。
    fn name(&self) -> &'static str;
    /// 播放；`volume` 是**已经算好**的有效音量（单条 × 总音量），后端不再做策略。
    fn play(&mut self, key: &str, file: File, volume: f32, looping: bool)
        -> Result<(), AudioError>;
    /// 停止并丢弃一条声音；没有这个 key 返回 false。
    fn stop(&mut self, key: &str) -> bool;
    /// 停掉全部声音。
    fn stop_all(&mut self);
    /// 暂停（保留播放位置）。
    fn pause(&mut self, key: &str) -> bool;
    /// 从暂停处继续。
    fn resume(&mut self, key: &str) -> bool;
    /// 改单条有效音量。
    fn set_volume(&mut self, key: &str, volume: f32) -> bool;
    /// 改播放倍速（多普勒变调）。
    ///
    /// 默认实现返回 `false` = **该后端不支持实时变速**：门面照样会把倍速算出来并记账
    /// （`audio_spatial_rate()` 能读到），只是不下发。这条默认实现是为了让旧后端 /
    /// 测试替身不必为本项改动而全部实现一遍。
    fn set_speed(&mut self, _key: &str, _speed: f32) -> bool {
        false
    }
    /// 是否支持实时变速（探针 / 文档用；`SilentBackend` 恒 `false`）。
    fn supports_speed(&self) -> bool {
        false
    }
    /// 是否正在出声（暂停不算在播）。
    fn is_playing(&mut self, key: &str) -> bool;
    /// 是否处于暂停态。
    fn is_paused(&mut self, key: &str) -> bool;
    /// 当前后端里还活着的 key。
    fn active_keys(&mut self) -> Vec<String>;
    /// 回收已播完的实例，返回被回收的 key（每帧调一次）。
    fn update(&mut self) -> Vec<String>;
}

/// 真设备后端：rodio（cpal 播放 + symphonia 解码）。
pub struct RodioBackend {
    /// 设备流必须活到进程结束：drop 了声音就没了。
    sink: MixerDeviceSink,
    /// 一个 key 一条 `Player`，各自独立音量 —— 这就是「简单混音」的落点。
    players: Vec<(String, Player)>,
}

impl RodioBackend {
    /// 打开系统默认输出设备；失败（无声卡 / 独占占用）返回错误，由门面降级成静默。
    pub fn open() -> Result<Self, AudioError> {
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .map_err(|e| AudioError::new(format!("打开默认音频设备失败（{e}）")))?;
        // 正常退出时设备流必然被 drop，rodio 默认会打一行"声音将停止"的提示；
        // 引擎退出本来就要停声，这行只会污染收尾日志，关掉。
        sink.log_on_drop(false);
        log::info!("音频后端 rodio 已打开默认输出设备");
        Ok(Self {
            sink,
            players: Vec::new(),
        })
    }

    fn find(&mut self, key: &str) -> Option<&mut Player> {
        self.players
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, p)| p)
    }
}

impl AudioBackend for RodioBackend {
    fn name(&self) -> &'static str {
        "rodio"
    }

    fn play(
        &mut self,
        key: &str,
        file: File,
        volume: f32,
        looping: bool,
    ) -> Result<(), AudioError> {
        // 先解码再停旧实例：文件坏了就别把正在响的那条也弄没
        let decoder = rodio::Decoder::try_from(file)
            .map_err(|e| AudioError::new(format!("解码音频失败（{key}）：{e}")))?;
        let player = Player::connect_new(self.sink.mixer());
        player.set_volume(volume);
        if looping {
            player.append(decoder.repeat_infinite());
        } else {
            player.append(decoder);
        }
        self.stop(key);
        self.players.push((key.to_string(), player));
        Ok(())
    }

    fn stop(&mut self, key: &str) -> bool {
        let Some(idx) = self.players.iter().position(|(k, _)| k == key) else {
            return false;
        };
        let (_, player) = self.players.remove(idx);
        player.stop();
        true
    }

    fn stop_all(&mut self) {
        for (_, player) in self.players.drain(..) {
            player.stop();
        }
    }

    fn pause(&mut self, key: &str) -> bool {
        match self.find(key) {
            Some(p) => {
                p.pause();
                true
            }
            None => false,
        }
    }

    fn resume(&mut self, key: &str) -> bool {
        match self.find(key) {
            Some(p) => {
                p.play();
                true
            }
            None => false,
        }
    }

    fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        match self.find(key) {
            Some(p) => {
                p.set_volume(volume);
                true
            }
            None => false,
        }
    }

    fn is_playing(&mut self, key: &str) -> bool {
        match self.find(key) {
            // `empty()` = 队列里还有没有样本；暂停时算「没在播」但实例还在
            Some(p) => !p.empty() && !p.is_paused(),
            None => false,
        }
    }

    /// 多普勒落点：rodio 的 `Player::set_speed` 直接改采样步进，**音高随倍速等比变化**，
    /// 正是多普勒需要的行为（速度变快 = 音高变高），而且是实时可控、不打断播放。
    ///
    /// 夹紧在 `0.1 ..= 8.0`：rodio 内部按倍数推进采样，`0` 会让播放器空转，
    /// 负数没有意义。门面已经夹过一次（0.25..=4），这里是第二道防线——
    /// 后端不该信任任何上游传入的浮点数。
    fn set_speed(&mut self, key: &str, speed: f32) -> bool {
        let speed = if speed.is_finite() {
            speed.clamp(0.1, 8.0)
        } else {
            1.0
        };
        match self.find(key) {
            Some(p) => {
                p.set_speed(speed);
                true
            }
            None => false,
        }
    }

    fn supports_speed(&self) -> bool {
        true
    }

    fn is_paused(&mut self, key: &str) -> bool {
        match self.find(key) {
            Some(p) => p.is_paused(),
            None => false,
        }
    }

    fn active_keys(&mut self) -> Vec<String> {
        self.players.iter().map(|(k, _)| k.clone()).collect()
    }

    fn update(&mut self) -> Vec<String> {
        let mut finished = Vec::new();
        self.players.retain(|(key, player)| {
            if player.empty() {
                finished.push(key.clone());
                false
            } else {
                true
            }
        });
        finished
    }
}

/// 静默后端：没有声卡时占位。
///
/// 它仍然会**解码一次**文件——这样「文件不存在 / 格式不支持」和「没声卡」是两种可区分的
/// 错误，前者如实报错，后者只降级不报错（见 `docs/ARCHITECTURE.md` §15.4）。
pub struct SilentBackend {
    reason: String,
}

impl SilentBackend {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// 降级原因，进 `audio_last_error()`。
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl AudioBackend for SilentBackend {
    fn name(&self) -> &'static str {
        "silent"
    }

    fn play(
        &mut self,
        key: &str,
        file: File,
        _volume: f32,
        _looping: bool,
    ) -> Result<(), AudioError> {
        rodio::Decoder::try_from(file)
            .map(|_| ())
            .map_err(|e| AudioError::new(format!("解码音频失败（{key}）：{e}")))
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
        false
    }

    fn is_paused(&mut self, _key: &str) -> bool {
        false
    }

    fn active_keys(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn update(&mut self) -> Vec<String> {
        Vec::new()
    }
}
