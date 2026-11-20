//! `AudioHost` 的**实现**（M2-A03b）：把真实 `Audio` 混音器适配成脚本侧能力面。
//!
//! 能力 trait 住在 `r3e-core::host`，本文件是唯一实现点；`r3e-script` 因此不必
//! 认识 `Audio` / `SpatialSettings` / `ListenerPose` 等本 crate 类型。
//!
//! 转换逻辑与旧 `api/audio.rs` 的闭包体逐字一致（M2 的零行为变化约束）。

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use r3e_core::host::{AudioHost, AudioReq, AudioRsp};

use crate::{Audio, DistanceModel, ListenerPose, SpatialSettings};

/// 音频能力面门面：与真混音器共享同一个 `Audio`。
pub struct AudioFacade {
    inner: Rc<RefCell<Audio>>,
}

impl AudioFacade {
    /// 包成能力面。返回 `Rc<dyn AudioHost>`。
    pub fn wrap(inner: Rc<RefCell<Audio>>) -> Rc<dyn AudioHost> {
        Rc::new(Self { inner })
    }

    /// 取回底层混音器。
    pub fn inner(&self) -> &Rc<RefCell<Audio>> {
        &self.inner
    }
}

impl AudioHost for AudioFacade {
    fn execute(&self, req: AudioReq<'_>) -> AudioRsp {
        match req {
            AudioReq::PlayDefault { key, path } => {
                AudioRsp::Bool(self.inner.borrow_mut().play_default(key, path).is_ok())
            }
            AudioReq::Play {
                key,
                path,
                volume,
                looping,
            } => AudioRsp::Bool(
                self.inner
                    .borrow_mut()
                    .play(key, path, volume as f32, looping)
                    .is_ok(),
            ),
            AudioReq::Stop { key } => AudioRsp::Bool(self.inner.borrow_mut().stop(key)),
            AudioReq::StopAll => {
                self.inner.borrow_mut().stop_all();
                AudioRsp::Unit
            }
            AudioReq::Pause { key } => AudioRsp::Bool(self.inner.borrow_mut().pause(key)),
            AudioReq::Resume { key } => AudioRsp::Bool(self.inner.borrow_mut().resume(key)),
            AudioReq::SetVolume { key, volume } => {
                AudioRsp::Bool(self.inner.borrow_mut().set_volume(key, volume as f32))
            }
            AudioReq::SetLoop { key, looping } => {
                AudioRsp::Bool(self.inner.borrow_mut().set_loop(key, looping))
            }
            AudioReq::IsPlaying { key } => AudioRsp::Bool(self.inner.borrow_mut().is_playing(key)),
            AudioReq::IsPaused { key } => AudioRsp::Bool(self.inner.borrow().is_paused(key)),
            AudioReq::ActiveCount => AudioRsp::Int(self.inner.borrow().active_count() as i64),
            AudioReq::Keys => AudioRsp::Texts(self.inner.borrow().keys()),
            AudioReq::SetMasterVolume { volume } => {
                self.inner.borrow_mut().set_master_volume(volume as f32);
                AudioRsp::Unit
            }
            AudioReq::MasterVolume => AudioRsp::Float(self.inner.borrow().master_volume() as f64),
            AudioReq::SetEnabled { on } => {
                self.inner.borrow_mut().set_enabled(on);
                AudioRsp::Unit
            }
            AudioReq::Enabled => AudioRsp::Bool(self.inner.borrow().enabled()),
            AudioReq::DeviceAvailable => AudioRsp::Bool(self.inner.borrow().device_available()),
            AudioReq::BackendName => AudioRsp::Text(self.inner.borrow().backend_name().to_string()),
            AudioReq::LastError => {
                AudioRsp::Text(self.inner.borrow().last_error().unwrap_or("").to_string())
            }
            AudioReq::ErrorCount => AudioRsp::Int(self.inner.borrow().error_count() as i64),
            AudioReq::StatusLine => AudioRsp::Text(self.inner.borrow_mut().status_line()),
            AudioReq::SpatialAttach { key, min, max } => AudioRsp::Bool(
                self.inner
                    .borrow_mut()
                    .attach_spatial(key, SpatialSettings::new(min as f32, max as f32)),
            ),
            AudioReq::SpatialAttachEx {
                key,
                min,
                max,
                model,
                doppler,
            } => {
                let mut settings = SpatialSettings::new(min as f32, max as f32);
                settings.model = DistanceModel::parse(model).unwrap_or_default();
                settings.doppler_factor = doppler as f32;
                AudioRsp::Bool(self.inner.borrow_mut().attach_spatial(key, settings))
            }
            AudioReq::SpatialAttachNode {
                key,
                node,
                min,
                max,
            } => AudioRsp::Bool(self.inner.borrow_mut().attach_spatial_node(
                key,
                node,
                SpatialSettings::new(min as f32, max as f32),
            )),
            AudioReq::SpatialDetach { key } => {
                AudioRsp::Bool(self.inner.borrow_mut().detach_spatial(key))
            }
            AudioReq::SpatialClear => {
                self.inner.borrow_mut().clear_spatial();
                AudioRsp::Unit
            }
            AudioReq::SpatialCount => AudioRsp::Int(self.inner.borrow().spatial_count() as i64),
            AudioReq::SpatialKeys => AudioRsp::Texts(self.inner.borrow().spatial_keys()),
            AudioReq::SpatialSetPosition { key, pos } => AudioRsp::Bool(
                self.inner
                    .borrow_mut()
                    .set_source_position(key, [pos[0] as f32, pos[1] as f32, pos[2] as f32]),
            ),
            AudioReq::SpatialPosition { key } => match self.inner.borrow().source_position(key) {
                Some(p) => AudioRsp::Floats(vec![p[0] as f64, p[1] as f64, p[2] as f64]),
                None => AudioRsp::Unit,
            },
            AudioReq::SpatialSetModel { key, model } => {
                let Some(model) = DistanceModel::parse(model) else {
                    return AudioRsp::Bool(false);
                };
                let audio = self.inner.borrow();
                let Some(mut settings) = audio.spatial_settings(key) else {
                    return AudioRsp::Bool(false);
                };
                drop(audio);
                settings.model = model;
                AudioRsp::Bool(self.inner.borrow_mut().set_spatial_settings(key, settings))
            }
            AudioReq::SpatialModel { key } => AudioRsp::Text(
                self.inner
                    .borrow()
                    .spatial_settings(key)
                    .map(|s| s.model.name().to_string())
                    .unwrap_or_default(),
            ),
            AudioReq::SpatialSetDoppler { key, factor } => {
                let audio = self.inner.borrow();
                let Some(mut settings) = audio.spatial_settings(key) else {
                    return AudioRsp::Bool(false);
                };
                drop(audio);
                settings.doppler_factor = factor as f32;
                AudioRsp::Bool(self.inner.borrow_mut().set_spatial_settings(key, settings))
            }
            AudioReq::SpatialDoppler { key } => AudioRsp::Float(
                self.inner
                    .borrow()
                    .spatial_settings(key)
                    .map(|s| s.doppler_factor as f64)
                    .unwrap_or(1.0),
            ),
            AudioReq::SpatialGain { key } => {
                AudioRsp::Float(self.inner.borrow().spatial_gain(key) as f64)
            }
            AudioReq::SpatialRate { key } => {
                AudioRsp::Float(self.inner.borrow().spatial_rate(key) as f64)
            }
            AudioReq::SpatialDistance { key } => {
                AudioRsp::Float(self.inner.borrow().spatial_distance(key).unwrap_or(-1.0) as f64)
            }
            AudioReq::SpatialNode { key } => AudioRsp::Text(
                self.inner
                    .borrow()
                    .spatial_node(key)
                    .map(str::to_string)
                    .unwrap_or_default(),
            ),
            AudioReq::SpatialSetListener { pos, forward } => {
                let pose = ListenerPose::new(
                    [pos[0] as f32, pos[1] as f32, pos[2] as f32],
                    [forward[0] as f32, forward[1] as f32, forward[2] as f32],
                );
                self.inner.borrow_mut().set_listener(pose);
                AudioRsp::Unit
            }
            AudioReq::SpatialListener => {
                let pose = self.inner.borrow().listener();
                AudioRsp::Pose {
                    position: [
                        pose.position[0] as f64,
                        pose.position[1] as f64,
                        pose.position[2] as f64,
                    ],
                    forward: [
                        pose.forward[0] as f64,
                        pose.forward[1] as f64,
                        pose.forward[2] as f64,
                    ],
                }
            }
            AudioReq::SpatialSetEnabled { on } => {
                self.inner.borrow_mut().set_spatial_enabled(on);
                AudioRsp::Unit
            }
            AudioReq::SpatialEnabled => AudioRsp::Bool(self.inner.borrow().spatial_enabled()),
            AudioReq::SpatialTick { dt } => {
                AudioRsp::Int(self.inner.borrow_mut().tick_spatial(dt as f32) as i64)
            }
            AudioReq::SupportsSpeed => AudioRsp::Bool(self.inner.borrow().supports_speed()),
            AudioReq::SpatialStatusLine => {
                AudioRsp::Text(self.inner.borrow().spatial_status_line())
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
