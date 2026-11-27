//! Prefab：模板定义 / 实例化 / 覆盖写入与读取（M2-E04）。
//!
//! 六条命令**全部归 [`Domain::Scene`]**（`M2_E_PLAN` §3.6，`P4` = A）：prefab 改的是
//! 场景装配结果（实例节点、覆盖表），不是新的资源域，因此不新增 [`Domain`] 变体，
//! `domains` 计数保持 18 不变。
//!
//! **只有两条带对外 RPC**（`P5` = B 的口径）：`prefab.spawn` 是跨进程**写**入口
//! （唯一实例化入口，决定场景内容），`prefab.info` 是跨进程**读**入口（编辑器 UI 取
//! 模板 ID 与覆盖字段清单，不必先拉整份 `scene.json` 自己反推）。
//! **`prefab_define` 故意无 rpc**：模板建库只走脚本（`r3e-script/src/api/prefab.rs`）
//! 或宿主本地调用（`r3e-editor-host/src/api/prefab.rs`），不登记进 `RPC_METHODS`。
//! 代价是跨进程复现必须先本地建好模板（模板文件随仓库分发到 `assets/prefab/<id>.json`
//! 后，`rpc --method prefab.spawn` 即可跑通），该缺口的补口触发条件登记在
//! `M2_E_PLAN` §13 `R7`。
//!
//! **`script_only` 因此 +4**（`spawn_many` / `set_override` / `clear_override` /
//! `define`）：这 4 条靠 `r3e-script/src/api/prefab.rs` 里真实的 `register_fn` 注册点
//! 才签得出脚本名，不是占位死条目。与 C 阶段（`terrain.*` 八条全带 rpc，为守住
//! 「`script_only` 不增」）是**有意不同**，代价与配平见 `M2_E_PLAN` §11.2 / §13 `R1`。
//!
//! 代价口径：`define` 含一次模板文件读（`Linear`，非重负载）；`spawn` / `spawn_many`
//! 走 `Scene::spawn` 逐实例建节点（`Linear`，`count ≤ 256`）；`set_override` /
//! `clear_override` 只碰一个字段（`Cheap`）；`info` 要汇总模板与覆盖字段清单
//! （`Linear`，遍历该实例的覆盖表）。

use super::{CommandSpec, Cost, Domain};

/// 本域全部命令。
pub(super) static ALL: &[CommandSpec] = &[
    // 建库：从文件载入/更新模板入表。未知或非法路径 → `PrefabOpError::Invalid`。
    // 可撤销：模板条目有前像（`old`），撤销即写回。
    CommandSpec {
        name: "prefab_define",
        rpc: None,
        domain: Domain::Scene,
        params: &["&str", "&str"],
        returns: "bool",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-E",
    },
    // 唯一跨进程写入口。模板未注册 → `PrefabOpError::Conflict`（不是 `Invalid`：
    // 参数形状没错，是引用了一个不存在的模板）。
    CommandSpec {
        name: "prefab_spawn",
        rpc: Some("prefab.spawn"),
        domain: Domain::Scene,
        params: &["&str", "&str", "&str"],
        returns: "&str",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-E",
    },
    // 批量实例化：`prefix` 加序号命名，`count` 上限 256（对齐 `SPAWN_PREFAB_MAX_NODES`），
    // 返回实际生成数。超出上限属参数非法（`Invalid`），不静默截断。
    CommandSpec {
        name: "prefab_spawn_many",
        rpc: None,
        domain: Domain::Scene,
        params: &["&str", "&str", "i64"],
        returns: "i64",
        reversible: true,
        cost: Cost::Linear,
        since: "M2-E",
    },
    // 只写差异字段：`value` 是唯一的 `dynamic` 参数（不扩散到其它命令），
    // `field` 不在 13 字段覆盖集内 → `Invalid`。
    CommandSpec {
        name: "prefab_set_override",
        rpc: None,
        domain: Domain::Scene,
        params: &["&str", "&str", "dynamic"],
        returns: "bool",
        reversible: true,
        cost: Cost::Cheap,
        since: "M2-E",
    },
    // 回退为继承模板：与 `set_override` 成对，撤销即重新写回旧值（前像）。
    CommandSpec {
        name: "prefab_clear_override",
        rpc: None,
        domain: Domain::Scene,
        params: &["&str", "&str"],
        returns: "bool",
        reversible: true,
        cost: Cost::Cheap,
        since: "M2-E",
    },
    // 只读：返回模板 ID 与覆盖字段清单（`Map`）。
    CommandSpec {
        name: "prefab_info",
        rpc: Some("prefab.info"),
        domain: Domain::Scene,
        params: &["&str"],
        returns: "Map",
        reversible: false,
        cost: Cost::Linear,
        since: "M2-E",
    },
];
