//! 地形：建块 / 删块 / 采样 / 块划分 / 重采样 / 生成 / 统计（M2-C06）。
//!
//! 八条命令**全部带对外 RPC**（`M2_C_PLAN` §12 CD-1 档 A）：`script_only` 不得增加，
//! 所以新命令一律配 RPC；代价是 `rpc` 预算从 +3 放宽到 +8（68 → 76）。
//!
//! 一条挂在 [`Domain::Meta`]：`terrain.mesh`。理由是依赖边——网格生成在
//! `r3e-terrain`，而 `r3e-script` 的 allow 只有 `[r3e-core, r3e-doc, r3e-protocol]`，
//! 脚本侧碰不到它，只能做成「只 RPC、无脚本函数」。其余七条只吃
//! [`r3e_core::TerrainPatch`] 的采样与块索引（C01 已把单块采样下沉到 core），
//! 脚本侧零新依赖。
//!
//! 代价口径：`create` / `remove` 只改一块（常数级）；`sample` 是单点采样（常数级）；
//! `blocks` / `stats` 遍历全部块（线性）；`resample` 重算一块的样本网格（线性）；
//! `gen` 生成整张地形并落盘度量（重，事务要拆帧）。

use super::{CommandSpec, Cost, Domain};

/// 本域全部命令。
pub(super) static ALL: &[CommandSpec] = &[
    // 写场景（进 `Node` 之外的地形表），可撤销走事务。
    CommandSpec {
        name: "terrain_create",
        rpc: Some("terrain.create"),
        domain: Domain::Terrain,
        params: &["&str", "f64", "f64", "f64", "f64", "i64"],
        returns: "bool",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-C06",
    },
    CommandSpec {
        name: "terrain_remove",
        rpc: Some("terrain.remove"),
        domain: Domain::Terrain,
        params: &["&str"],
        returns: "bool",
        reversible: false,
        cost: Cost::Cheap,
        since: "M2-C06",
    },
    // 只读采样：返回 JSON 字符串（高度 / 法线 / 权重）。块名可空——空名表示
    // 「在该坐标命中的所有块里选第一块」，这是查询最常用的形态。
    CommandSpec {
        name: "terrain_sample",
        rpc: Some("terrain.sample"),
        domain: Domain::Terrain,
        params: &["&str", "f64", "f64"],
        returns: "&str",
        reversible: true,
        cost: Cost::Cheap,
        since: "M2-C06",
    },
    CommandSpec {
        name: "terrain_blocks",
        rpc: Some("terrain.blocks"),
        domain: Domain::Terrain,
        params: &[],
        returns: "&str",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-C06",
    },
    CommandSpec {
        name: "terrain_resample",
        rpc: Some("terrain.resample"),
        domain: Domain::Terrain,
        params: &["&str", "f64", "i64"],
        returns: "bool",
        reversible: false,
        cost: Cost::Linear,
        since: "M2-C06",
    },
    CommandSpec {
        name: "terrain_gen",
        rpc: Some("terrain.gen"),
        domain: Domain::Terrain,
        params: &["i64", "i64", "i64"],
        returns: "&str",
        reversible: false,
        cost: Cost::Heavy,
        since: "M2-C06",
    },
    CommandSpec {
        name: "terrain_stats",
        rpc: Some("terrain.stats"),
        domain: Domain::Terrain,
        params: &[],
        returns: "&str",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-C06",
    },
    // 只 RPC：网格生成要 `r3e-terrain`，脚本侧依赖边不允许。名字用点分 RPC 名，
    // 与其它 `Meta` 条目一致（`Domain::Meta` 的判据见 `script_names`）。
    CommandSpec {
        name: "terrain.mesh",
        rpc: Some("terrain.mesh"),
        domain: Domain::Meta,
        params: &[],
        returns: "&str",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-C06",
    },
];
