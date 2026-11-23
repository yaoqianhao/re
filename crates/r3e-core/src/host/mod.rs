//! 脚本侧的**能力面**：只声明脚本能要求什么，不认任何具体实现 crate。
//!
//! 依赖倒置点（M2-A03b）：能力 trait 住在**公共契约层**（本 crate），
//! `r3e-script` 只消费 trait、不认识 `r3e-physics` / `r3e-audio`；
//! 实现分别在 `r3e-physics/src/host_adapter.rs` 与 `r3e-audio/src/host_adapter.rs`
//! （放在 core 而不是 script，是因为 script 自己的单元测试要能构造宿主 ——
//! dev-depend 到 script 的 crate 与之互不兼容，见 A03b 实施记录）。
//!
//! 设计要点：
//! * 每个域只有 **一个必须实现的方法** `execute`，其余能力全是默认方法，
//!   所以「新增一条脚本 API」的代价是「加一个枚举变体 + 一行默认方法」，不用改实现方。
//! * 参数与返回值全部是脚本友好类型（`&str` / `f64` / `bool` / 扁平 DTO），
//!   实现方在自己的 `match` 里做类型转换——转换点集中在一处，便于逐条断言。
//! * `impl<F: Fn(Req) -> Rsp> Host for F` 让「函数式适配器」成为字面意义：
//!   测试里写一个闭包就是一套后端。

mod audio;
mod character;
mod physics;
mod terrain;

pub use audio::{AudioHost, AudioReq, AudioRsp, NullAudioHost, SilentAudioHost};
pub use character::{
    CharReq, CharRsp, CharacterHost, CharacterStateDto, CharacterStatsDto, NullCharacterHost,
};
pub use physics::{HitDto, NullPhysicsHost, PhysReq, PhysRsp, PhysicsHost};
pub use terrain::{
    NullTerrainHost, TerrainBlockDto, TerrainBlocksDto, TerrainHost, TerrainReq,
    TerrainResampleDto, TerrainRsp, TerrainSampleDto, TerrainStatsDto,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, PhysicsSceneDesc};

    fn node() -> Node {
        Node::new(1, "box")
    }

    /// 空物理后端：全拒语义（对齐 `null` 后端）。
    #[test]
    fn null_physics_rejects_every_write() {
        let h = NullPhysicsHost;
        assert!(!h.enabled());
        assert!(!h.set_enabled(true));
        assert!(!h.has_capability("ccd"));
        assert_eq!(h.body_count(), 0);
        assert_eq!(h.backend(), "null");
        assert!(h.attach(&node(), "dynamic", 1.0).is_none());
        assert!(h.raycast([0.0; 3], [0.0, 0.0, -1.0], 10.0).is_none());
        assert!(h.take_events().is_empty());
        assert!(!h.set_ccd(true));
    }

    /// 空音频后端：静默降级语义（对齐 `Audio::silent`）。
    #[test]
    fn null_audio_is_silent_but_alive() {
        let h = NullAudioHost;
        assert!(!h.play_default("k", "a.ogg"));
        assert!(!h.device_available());
        assert_eq!(h.backend_name(), "silent");
        assert_eq!(h.last_error(), "");
        assert_eq!(h.master_volume(), 1.0);
        assert!(h.keys().is_empty());
        assert_eq!(h.spatial_distance("k"), -1.0);
        assert_eq!(h.spatial_listener(), [0.0, 0.0, 0.0, 0.0, 0.0, -1.0]);
    }

    /// 闭包即适配器：物理。
    #[test]
    fn closure_is_a_physics_host() {
        let h = |req: PhysReq<'_>| match req {
            PhysReq::BodyCount => PhysRsp::Int(7),
            PhysReq::Raycast { max, .. } => PhysRsp::Hit(if max > 5.0 {
                Some(HitDto {
                    node: 3,
                    handle: 9,
                    distance: 2.0,
                    ..Default::default()
                })
            } else {
                None
            }),
            PhysReq::SetBackend(_) => PhysRsp::Bool(true),
            _ => PhysRsp::Bool(false),
        };
        assert_eq!(h.body_count(), 7);
        assert_eq!(h.raycast([0.0; 3], [0.0, 0.0, -1.0], 10.0).unwrap().node, 3);
        assert!(h.raycast([0.0; 3], [0.0, 0.0, -1.0], 1.0).is_none());
        assert!(h.set_backend("rapier"));
    }

    /// 闭包即适配器：音频。
    #[test]
    fn closure_is_an_audio_host() {
        let h = |req: AudioReq<'_>| match req {
            AudioReq::ActiveCount => AudioRsp::Int(2),
            AudioReq::Keys => AudioRsp::Texts(vec!["a".into(), "b".into()]),
            AudioReq::Play { volume, .. } => AudioRsp::Bool(volume > 0.5),
            _ => AudioRsp::Bool(false),
        };
        assert_eq!(h.active_count(), 2);
        assert_eq!(h.keys(), vec!["a".to_string(), "b".to_string()]);
        assert!(h.play("k", "a.ogg", 0.9, false));
        assert!(!h.play("k", "a.ogg", 0.1, false));
    }

    /// 应答变体解包：类型不符一律退化到安全值，不 panic。
    #[test]
    fn mismatched_response_degrades_safely() {
        let h = |_req: PhysReq<'_>| PhysRsp::Text("oops".into());
        assert!(!h.enabled());
        assert_eq!(h.body_count(), 0);
        assert_eq!(h.gravity(), 0.0);
        assert!(h.overlap([0.0; 3], 1.0).is_empty());
        assert_eq!(h.backend(), "oops");
    }

    /// 表驱动：24 条物理能力全部可达 `execute`（覆盖默认方法转发）。
    #[test]
    fn physics_default_methods_forward_to_execute() {
        let h = |req: PhysReq<'_>| match req {
            PhysReq::Detach { handle } => PhysRsp::Int(handle as i64),
            PhysReq::Impulse { handle, v } => PhysRsp::Float(handle as f64 + v[1]),
            PhysReq::SetGravity(g) => PhysRsp::Float(g[1]),
            PhysReq::SetGround(y) => PhysRsp::Float(y),
            PhysReq::BodySetCcd { handle, .. } => PhysRsp::Bool(handle == 1),
            PhysReq::ResetScene { .. } => PhysRsp::Err(Some("boom".into())),
            _ => PhysRsp::Unit,
        };
        h.detach(5);
        h.impulse(2, [0.0, 3.0, 0.0]);
        h.set_gravity([0.0, -9.8, 0.0]);
        h.set_ground(-1.5);
        assert!(h.body_set_ccd(1, true));
        assert!(!h.body_set_ccd(2, true));
        assert_eq!(
            h.reset_scene(&PhysicsSceneDesc::default()),
            Some("boom".to_string())
        );
    }

    /// 闭包即适配器：角色。
    #[test]
    fn closure_is_a_character_host() {
        let h = |req: CharReq<'_>| match req {
            CharReq::Bind { node } => CharRsp::Bool(node.character.is_some()),
            CharReq::Stats => CharRsp::Stats(CharacterStatsDto {
                controllers: 3,
                steps: 7,
                blocked: 1,
                step_ups: 2,
            }),
            CharReq::Backend => CharRsp::Text("closure".into()),
            _ => CharRsp::Bool(false),
        };
        let mut node = node();
        node.character = Some(crate::CharacterSpec::default());
        assert!(h.bind(&node));
        node.character = None;
        assert!(!h.bind(&node));
        assert_eq!(h.stats().controllers, 3);
        assert_eq!(h.backend(), "closure");
    }

    /// 空实现也实现了 `as_any`（装配层取回实现体的通道）。
    #[test]
    fn null_hosts_expose_any() {
        let p = NullPhysicsHost;
        assert!(p.as_any().downcast_ref::<NullPhysicsHost>().is_some());
        let a = NullAudioHost;
        assert!(a.as_any().downcast_ref::<NullAudioHost>().is_some());
        let c = NullCharacterHost::new();
        assert!(c.as_any().downcast_ref::<NullCharacterHost>().is_some());
    }

    /// `Node` 快照可跨 trait 边界：`attach` 只依赖 core 类型。
    #[test]
    fn attach_passes_core_node_snapshot() {
        let h = |req: PhysReq<'_>| match req {
            PhysReq::Attach { node, kind, mass } => {
                assert_eq!(node.name, "box");
                assert_eq!(kind, "kinematic");
                PhysRsp::Int(if mass > 0.0 { 11 } else { -1 })
            }
            _ => PhysRsp::Unit,
        };
        assert_eq!(h.attach(&node(), "kinematic", 2.0), Some(11));
        assert_eq!(h.attach(&node(), "kinematic", 0.0), None);
    }
}
