//! [`PrefabApi`]：`prefab.*` 两条的协议面（M2-E04）。
//!
//! # 为什么只有两条
//!
//! `M2_E_PLAN` §3.6 / `P5` = B：六条 prefab 命令里**只有两条带 RPC**，额度按
//! 「跨进程读写各一」分配。
//!
//! * `prefab.spawn`：跨进程**写**入口，唯一决定场景内容的实例化口；
//! * `prefab.info`：跨进程**读**入口，编辑器 UI 取模板 ID 与覆盖字段清单，
//!   不必先拉整份 `scene.json` 自己反推。
//!
//! 其余四条（`define` / `spawn_many` / `set_override` / `clear_override`）**故意无 rpc**：
//! 它们只走脚本（`r3e-script/src/api/prefab.rs`）或宿主本地调用
//! （`r3e-editor-host/src/api/prefab.rs`）。这不是偷懒——那四条在脚本侧有真实的
//! `register_fn` 注册点，属「脚本可用、无 RPC」的真实能力面。
//!
//! # 为什么单独成文件
//!
//! 两条的契约注释（回包结构 / 模板未注册的口径 / 覆盖清单三态）加起来七八十行，
//! 挤进 [`crate::api::EngineApi`] 会把协议层单文件推向 `lint-size` 的 800 行上限。
//! 拆成子 trait 之后 `EngineApi: PrefabApi` 继承，调用方零改动。

use serde_json::Value;

use crate::msg::RpcFailure;

/// `prefab.spawn` 的入参。
///
/// `parent` 省略（或显式 `null`）= 挂场景根。**不用空串表达「挂根」**：
/// 空串在节点名域里是个合法但也有意义的写法，让两种意图共用一个编码，
/// 「挂到根」和「挂到一个名叫空串的节点」就分不开了。
#[derive(Debug, Clone, PartialEq)]
pub struct PrefabSpawnRequest {
    /// 模板 ID（`域.名` 点分小写，形状由 `r3e-core` 的 `validate_prefab_id` 判）。
    pub id: String,
    /// 实例名，进场景后就是节点名；重名由 `Scene::spawn` 拒。
    pub name: String,
    /// 父节点名；`None` = 挂场景根。
    pub parent: Option<String>,
}

/// `prefab.info` 的入参。
#[derive(Debug, Clone, PartialEq)]
pub struct PrefabInfoRequest {
    /// 实例（节点）名。
    pub name: String,
}

/// `prefab.*` 两条的协议面（M2-E04）。
///
/// 全部方法都在主线程调用；缺省实现回 `METHOD_NOT_FOUND`，宿主可以只点亮当期
/// 需要的那几条。
pub trait PrefabApi {
    /// 单实例化（M2-E04）：按模板 ID 造一个实例节点，返回实例名。
    ///
    /// 回包契约：`{ "name": String, "template": String, "parent": String|null,
    /// "nodeId": u64, "overrides": { ... } }`。
    ///
    /// `overrides` 是这条实例当前的覆盖表（刚实例化时为空对象）：回给调用方是为了
    /// 让「我刚造了一个干净实例」这件事**可验证**，而不是靠再发一条 `prefab.info`。
    ///
    /// 失败口径：模板未注册 → `CONFLICT`（参数形状没错，引用了一个不存在的模板）；
    /// ID / 名字形状非法 → `INVALID_PARAMS`。两者**不能合并**：前者是「先
    /// `prefab_define` 再来」，后者是「你把参数写错了」。
    fn prefab_spawn(&mut self, _req: PrefabSpawnRequest) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("prefab.spawn"))
    }

    /// 实例信息（M2-E04）：回模板 ID 与覆盖字段清单。
    ///
    /// 回包契约：`{ "name": String, "template": String, "parent": String|null,
    /// "node": String|null, "count": usize,
    /// "overrides": [{ "field": String, "state": "set"|"clear" }...],
    /// "fields": [String...] }`。
    ///
    /// `overrides` 给**逐字段的态**（`set` / `clear`），`fields` 只给字段名清单：
    /// 「把某字段显式清空」与「完全没碰过」在继承语义下是两个不同结果，只回一份
    /// 名字列表就把这个区分抹掉了。
    ///
    /// `node` 是实例对应的场景节点名（节点被删掉时为 `null`）：实例记录还在、
    /// 节点却不在，正是「模板灌好之前载入了场景」的中间态，如实带出去。
    fn prefab_info(&mut self, _req: PrefabInfoRequest) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("prefab.info"))
    }
}
