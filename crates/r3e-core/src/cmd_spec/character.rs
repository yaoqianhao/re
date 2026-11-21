//! 角色控制器：建控制器 / 改配置 / 逐帧推进 / 读运行态 / 落位 / 统计（M2-B01）。
//!
//! 六条全部带对外 RPC（`M2_B_PLAN` §9 BD-1 档 A）：脚本侧能驱动，能力清单能枚举，
//! B05 的可玩性回归才有稳定的命令行入口。挂 [`Domain::Physics`]——角色控制器吃的
//! 是物理查询（扫掠 / 地面采样），**不新增域**（`domains` 保持 17）。
//!
//! 参数口径的两条约定：
//!
//! 1. 尺寸 / 坡度 / 台阶 / 吸附五个数**一次给全**，不做「只改一项」的重载：
//!    半套配置写进场景比写错更麻烦，缺省值由 [`CharacterSpec::default`] 在
//!    `character_create` 里兜底（脚本侧可只传名字）。
//! 2. `character_step` 的 `dt` 由调用方给：固定 1/60 是运行时的口径，回归测试
//!    需要更细的步长观察台阶边缘，协议层不替调用方做主。
//!
//! [`CharacterSpec::default`]: crate::CharacterSpec::default

use super::{CommandSpec, Cost, Domain};

/// 本域全部命令。
pub(super) static ALL: &[CommandSpec] = &[
    // 写场景（挂 `Node::character`），可撤销走事务。
    CommandSpec {
        name: "character_create",
        rpc: Some("character.create"),
        domain: Domain::Physics,
        params: &["&str", "f64", "f64", "f64", "f64", "f64"],
        returns: "bool",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
    CommandSpec {
        name: "character_config",
        rpc: Some("character.config"),
        domain: Domain::Physics,
        params: &["&str", "f64", "f64", "f64", "f64", "f64"],
        returns: "bool",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
    // 推进一帧：改的是宿主侧运行态，不写场景，但**不是**只读动作（可推理性差、
    // 顺序敏感），所以 `reversible: false`、代价按常数级算（一次扫掠，不遍历场景）。
    CommandSpec {
        name: "character_step",
        rpc: Some("character.step"),
        domain: Domain::Physics,
        params: &["&str", "f64", "f64", "f64"],
        returns: "()",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
    CommandSpec {
        name: "character_state",
        rpc: Some("character.state"),
        domain: Domain::Physics,
        params: &["&str"],
        returns: "Map",
        reversible: true,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
    CommandSpec {
        name: "character_teleport",
        rpc: Some("character.teleport"),
        domain: Domain::Physics,
        params: &["&str", "f64", "f64", "f64"],
        returns: "bool",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
    CommandSpec {
        name: "character_stats",
        rpc: Some("character.stats"),
        domain: Domain::Physics,
        params: &[],
        returns: "Map",
        reversible: true,
        cost: Cost::Cheap,
        since: "M2-B01",
    },
];
