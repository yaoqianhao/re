//! 音频能力面：45 条请求的封闭集合、`AudioHost` 契约与两个空实现。
//!
//! 拆自 `host.rs`（单文件曾超 800 行阈值），语义不变。

use std::any::Any;

// ======================================================================
// 音频
// ======================================================================

/// 一次音频请求：45 条脚本绑定的封闭集合。
pub enum AudioReq<'a> {
    PlayDefault {
        key: &'a str,
        path: &'a str,
    },
    Play {
        key: &'a str,
        path: &'a str,
        volume: f64,
        looping: bool,
    },
    Stop {
        key: &'a str,
    },
    StopAll,
    Pause {
        key: &'a str,
    },
    Resume {
        key: &'a str,
    },
    SetVolume {
        key: &'a str,
        volume: f64,
    },
    SetLoop {
        key: &'a str,
        looping: bool,
    },
    IsPlaying {
        key: &'a str,
    },
    IsPaused {
        key: &'a str,
    },
    ActiveCount,
    Keys,
    SetMasterVolume {
        volume: f64,
    },
    MasterVolume,
    SetEnabled {
        on: bool,
    },
    Enabled,
    DeviceAvailable,
    BackendName,
    LastError,
    ErrorCount,
    StatusLine,
    SpatialAttach {
        key: &'a str,
        min: f64,
        max: f64,
    },
    SpatialAttachEx {
        key: &'a str,
        min: f64,
        max: f64,
        model: &'a str,
        doppler: f64,
    },
    SpatialAttachNode {
        key: &'a str,
        node: &'a str,
        min: f64,
        max: f64,
    },
    SpatialDetach {
        key: &'a str,
    },
    SpatialClear,
    SpatialCount,
    SpatialKeys,
    SpatialSetPosition {
        key: &'a str,
        pos: [f64; 3],
    },
    SpatialPosition {
        key: &'a str,
    },
    SpatialSetModel {
        key: &'a str,
        model: &'a str,
    },
    SpatialModel {
        key: &'a str,
    },
    SpatialSetDoppler {
        key: &'a str,
        factor: f64,
    },
    SpatialDoppler {
        key: &'a str,
    },
    SpatialGain {
        key: &'a str,
    },
    SpatialRate {
        key: &'a str,
    },
    SpatialDistance {
        key: &'a str,
    },
    SpatialNode {
        key: &'a str,
    },
    SpatialSetListener {
        pos: [f64; 3],
        forward: [f64; 3],
    },
    SpatialListener,
    SpatialSetEnabled {
        on: bool,
    },
    SpatialEnabled,
    SpatialTick {
        dt: f64,
    },
    SupportsSpeed,
    SpatialStatusLine,
}

/// 音频应答。
pub enum AudioRsp {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Texts(Vec<String>),
    Floats(Vec<f64>),
    Pose {
        position: [f64; 3],
        forward: [f64; 3],
    },
    Unit,
}

/// 脚本对音频的全部诉求。实现方只需给出 `execute`。
pub trait AudioHost {
    fn execute(&self, req: AudioReq<'_>) -> AudioRsp;

    fn as_any(&self) -> &dyn Any;

    // ---- 非空间化 ----

    fn play_default(&self, key: &str, path: &str) -> bool {
        as_bool_a(self.execute(AudioReq::PlayDefault { key, path }))
    }

    fn play(&self, key: &str, path: &str, volume: f64, looping: bool) -> bool {
        as_bool_a(self.execute(AudioReq::Play {
            key,
            path,
            volume,
            looping,
        }))
    }

    fn stop(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::Stop { key }))
    }

    fn stop_all(&self) {
        self.execute(AudioReq::StopAll);
    }

    fn pause(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::Pause { key }))
    }

    fn resume(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::Resume { key }))
    }

    fn set_volume(&self, key: &str, volume: f64) -> bool {
        as_bool_a(self.execute(AudioReq::SetVolume { key, volume }))
    }

    fn set_loop(&self, key: &str, looping: bool) -> bool {
        as_bool_a(self.execute(AudioReq::SetLoop { key, looping }))
    }

    fn is_playing(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::IsPlaying { key }))
    }

    fn is_paused(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::IsPaused { key }))
    }

    fn active_count(&self) -> i64 {
        as_int_a(self.execute(AudioReq::ActiveCount))
    }

    fn keys(&self) -> Vec<String> {
        as_texts_a(self.execute(AudioReq::Keys))
    }

    fn set_master_volume(&self, volume: f64) {
        self.execute(AudioReq::SetMasterVolume { volume });
    }

    fn master_volume(&self) -> f64 {
        as_float_a(self.execute(AudioReq::MasterVolume))
    }

    fn set_enabled(&self, on: bool) {
        self.execute(AudioReq::SetEnabled { on });
    }

    fn enabled(&self) -> bool {
        as_bool_a(self.execute(AudioReq::Enabled))
    }

    fn device_available(&self) -> bool {
        as_bool_a(self.execute(AudioReq::DeviceAvailable))
    }

    fn backend_name(&self) -> String {
        as_text_a(self.execute(AudioReq::BackendName))
    }

    fn last_error(&self) -> String {
        as_text_a(self.execute(AudioReq::LastError))
    }

    fn error_count(&self) -> i64 {
        as_int_a(self.execute(AudioReq::ErrorCount))
    }

    fn status_line(&self) -> String {
        as_text_a(self.execute(AudioReq::StatusLine))
    }

    // ---- 3D 空间化 ----

    fn spatial_attach(&self, key: &str, min: f64, max: f64) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialAttach { key, min, max }))
    }

    fn spatial_attach_ex(&self, key: &str, min: f64, max: f64, model: &str, doppler: f64) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialAttachEx {
            key,
            min,
            max,
            model,
            doppler,
        }))
    }

    fn spatial_attach_node(&self, key: &str, node: &str, min: f64, max: f64) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialAttachNode {
            key,
            node,
            min,
            max,
        }))
    }

    fn spatial_detach(&self, key: &str) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialDetach { key }))
    }

    fn spatial_clear(&self) {
        self.execute(AudioReq::SpatialClear);
    }

    fn spatial_count(&self) -> i64 {
        as_int_a(self.execute(AudioReq::SpatialCount))
    }

    fn spatial_keys(&self) -> Vec<String> {
        as_texts_a(self.execute(AudioReq::SpatialKeys))
    }

    fn spatial_set_position(&self, key: &str, pos: [f64; 3]) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialSetPosition { key, pos }))
    }

    /// 未注册返回 `None`（脚本侧映射成空数组）。
    fn spatial_position(&self, key: &str) -> Option<[f64; 3]> {
        match self.execute(AudioReq::SpatialPosition { key }) {
            AudioRsp::Floats(v) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        }
    }

    fn spatial_set_model(&self, key: &str, model: &str) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialSetModel { key, model }))
    }

    fn spatial_model(&self, key: &str) -> String {
        as_text_a(self.execute(AudioReq::SpatialModel { key }))
    }

    fn spatial_set_doppler(&self, key: &str, factor: f64) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialSetDoppler { key, factor }))
    }

    fn spatial_doppler(&self, key: &str) -> f64 {
        as_float_a(self.execute(AudioReq::SpatialDoppler { key }))
    }

    fn spatial_gain(&self, key: &str) -> f64 {
        as_float_a(self.execute(AudioReq::SpatialGain { key }))
    }

    fn spatial_rate(&self, key: &str) -> f64 {
        as_float_a(self.execute(AudioReq::SpatialRate { key }))
    }

    /// 未注册返回 `-1.0`。
    fn spatial_distance(&self, key: &str) -> f64 {
        as_float_a(self.execute(AudioReq::SpatialDistance { key }))
    }

    fn spatial_node(&self, key: &str) -> String {
        as_text_a(self.execute(AudioReq::SpatialNode { key }))
    }

    fn spatial_set_listener(&self, pos: [f64; 3], forward: [f64; 3]) {
        self.execute(AudioReq::SpatialSetListener { pos, forward });
    }

    /// 听者位姿：`[px, py, pz, fx, fy, fz]`。
    fn spatial_listener(&self) -> [f64; 6] {
        match self.execute(AudioReq::SpatialListener) {
            AudioRsp::Pose { position, forward } => [
                position[0],
                position[1],
                position[2],
                forward[0],
                forward[1],
                forward[2],
            ],
            _ => [0.0; 6],
        }
    }

    fn spatial_set_enabled(&self, on: bool) {
        self.execute(AudioReq::SpatialSetEnabled { on });
    }

    fn spatial_enabled(&self) -> bool {
        as_bool_a(self.execute(AudioReq::SpatialEnabled))
    }

    fn spatial_tick(&self, dt: f64) -> i64 {
        as_int_a(self.execute(AudioReq::SpatialTick { dt }))
    }

    fn supports_speed(&self) -> bool {
        as_bool_a(self.execute(AudioReq::SupportsSpeed))
    }

    fn spatial_status_line(&self) -> String {
        as_text_a(self.execute(AudioReq::SpatialStatusLine))
    }
}

/// 函数式适配器（同物理：闭包即后端）。
impl<F> AudioHost for F
where
    F: for<'a> Fn(AudioReq<'a>) -> AudioRsp + 'static,
{
    fn execute(&self, req: AudioReq<'_>) -> AudioRsp {
        (self)(req)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 空实现：无声卡的宿主 / 单测的默认值，语义等同 `Audio::silent`。
pub struct NullAudioHost;

impl AudioHost for NullAudioHost {
    fn execute(&self, req: AudioReq<'_>) -> AudioRsp {
        match req {
            AudioReq::Keys | AudioReq::SpatialKeys => AudioRsp::Texts(Vec::new()),
            AudioReq::BackendName => AudioRsp::Text("silent".to_string()),
            AudioReq::LastError => AudioRsp::Text(String::new()),
            AudioReq::SpatialModel { .. } | AudioReq::SpatialNode { .. } => {
                AudioRsp::Text(String::new())
            }
            AudioReq::MasterVolume => AudioRsp::Float(1.0),
            AudioReq::SpatialDoppler { .. } => AudioRsp::Float(1.0),
            AudioReq::SpatialGain { .. } | AudioReq::SpatialRate { .. } => AudioRsp::Float(1.0),
            AudioReq::SpatialDistance { .. } => AudioRsp::Float(-1.0),
            AudioReq::SpatialPosition { .. } => AudioRsp::Floats(Vec::new()),
            AudioReq::SpatialListener => AudioRsp::Pose {
                position: [0.0; 3],
                forward: [0.0, 0.0, -1.0],
            },
            AudioReq::Enabled | AudioReq::SpatialEnabled => AudioRsp::Bool(true),
            AudioReq::ActiveCount
            | AudioReq::ErrorCount
            | AudioReq::SpatialCount
            | AudioReq::SpatialTick { .. } => AudioRsp::Int(0),
            _ => AudioRsp::Bool(false),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn as_bool_a(rsp: AudioRsp) -> bool {
    matches!(rsp, AudioRsp::Bool(true))
}

fn as_int_a(rsp: AudioRsp) -> i64 {
    match rsp {
        AudioRsp::Int(v) => v,
        _ => 0,
    }
}

fn as_float_a(rsp: AudioRsp) -> f64 {
    match rsp {
        AudioRsp::Float(v) => v,
        AudioRsp::Int(v) => v as f64,
        _ => 0.0,
    }
}

fn as_text_a(rsp: AudioRsp) -> String {
    match rsp {
        AudioRsp::Text(v) => v,
        _ => String::new(),
    }
}

fn as_texts_a(rsp: AudioRsp) -> Vec<String> {
    match rsp {
        AudioRsp::Texts(v) => v,
        _ => Vec::new(),
    }
}

// ======================================================================
// 装配辅助
// ======================================================================

/// 默认静默音频（`ScriptHost::new` 三参数版用它）：不碰设备、不出声，
/// 状态行与 `Audio::silent(reason)` 同构 —— 单测 / 编辑器不需要声卡。
pub struct SilentAudioHost {
    reason: &'static str,
}

impl SilentAudioHost {
    pub const fn new(reason: &'static str) -> Self {
        Self { reason }
    }
}

impl AudioHost for SilentAudioHost {
    fn execute(&self, req: AudioReq<'_>) -> AudioRsp {
        match req {
            AudioReq::Keys | AudioReq::SpatialKeys => AudioRsp::Texts(Vec::new()),
            AudioReq::BackendName => AudioRsp::Text("silent".to_string()),
            AudioReq::LastError => AudioRsp::Text(self.reason.to_string()),
            AudioReq::StatusLine => AudioRsp::Text(format!(
                "后端 silent / 设备 不可用（静默降级） / 在播 0（实例 0） / 总音量 1.00 / \
                 开关 开 / 空间化 0 条（开关 开） / 错误 0 / 最近 {}",
                self.reason
            )),
            AudioReq::SpatialStatusLine => {
                AudioRsp::Text(format!("空间化 0 条（开关 开） / 最近 {}", self.reason))
            }
            AudioReq::DeviceAvailable => AudioRsp::Bool(false),
            AudioReq::MasterVolume => AudioRsp::Float(1.0),
            AudioReq::Enabled | AudioReq::SpatialEnabled => AudioRsp::Bool(true),
            AudioReq::SpatialDoppler { .. } => AudioRsp::Float(1.0),
            AudioReq::SpatialGain { .. } | AudioReq::SpatialRate { .. } => AudioRsp::Float(1.0),
            AudioReq::SpatialDistance { .. } => AudioRsp::Float(-1.0),
            AudioReq::SpatialModel { .. } | AudioReq::SpatialNode { .. } => {
                AudioRsp::Text(String::new())
            }
            AudioReq::SpatialPosition { .. } => AudioRsp::Floats(Vec::new()),
            AudioReq::SpatialListener => AudioRsp::Pose {
                position: [0.0; 3],
                forward: [0.0, 0.0, -1.0],
            },
            AudioReq::ActiveCount
            | AudioReq::ErrorCount
            | AudioReq::SpatialCount
            | AudioReq::SpatialTick { .. } => AudioRsp::Int(0),
            _ => AudioRsp::Bool(false),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
