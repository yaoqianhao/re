//! r3e-protocol —— 编辑器桥的**协议层**（M0-S5 从 `r3e-editor` 剥离）。
//!
//! 职责是「说什么」：JSON-RPC 2.0 的请求 / 响应 / 错误码 / 方法表、14 个 `*Patch`
//! 局部改动体、[`EngineApi`] 抽象，以及把它们串起来的 [`dispatch`]。
//!
//! **不负责**「怎么传」：TCP + NDJSON、连接线程、主线程队列都在
//! `r3e-editor::server`（`r3e-editor` 只做传输，并用 `pub use r3e_protocol::*`
//! 保持对外 API 与剥离前一致，旧调用方一行不改）。
//!
//! # 为什么本 crate 零引擎依赖
//!
//! 协议层只认 JSON 值，不认 `NodeId` / `Vec3`。`cargo tree -p r3e-protocol
//! --edges normal,dev` 只应出现 `serde` / `serde_json` —— 连 dev-dependency 也
//! 不许有引擎 crate，否则「协议层不认识引擎类型」这条不变量就是假的。宿主
//! （`r3e-app`）实现 [`EngineApi`]，换内部类型不必改协议。
//!
//! # 分层
//!
//! | 文件 | 内容 |
//! |---|---|
//! | `msg.rs` | `Request` / `Response` / `RpcError` / `RpcFailure` / 元信息 DTO |
//! | `patch.rs` | 14 个 `*Patch`（只给要改的字段，其余保持原值） |
//! | `api.rs` | `trait EngineApi`（52 方法，全部在主线程调用） |
//! | `dispatch/mod.rs` | 派发骨架 + 参数解码 + 元命令（`ping` / `engine.*` / `script.*` / `capture.*`） |
//! | `dispatch/tx.rs` | `tx.*` 事务臂（M1-S1：begin / apply / preview / commit / rollback） |
//! | `dispatch/scene.rs` | `scene.*` / `anim.*` 臂 |
//! | `dispatch/physics.rs` | `physics.*` 查询臂（M1-S4-D：raycast / overlap / cast） |
//! | `dispatch/nav.rs` | `nav.*` 臂（M1-S4-D：bake / path / reachability） |
//! | `dispatch/media.rs` | `ui.*` / `light.*` / `audio.*` 臂 + 未知方法 |
//! | `probe.rs` | `physics.*` 参数 DTO（形状收敛，不含引擎类型） |
//! | `nav.rs` | `nav.*` 参数 DTO（`NavConfig` 字段平铺成 `Option`） |
//! | `material.rs` | `scene.material.upsert` 参数 DTO（整行声明，不是补丁） |

pub mod api;
pub mod caps;
pub mod dispatch;
pub mod material;
pub mod msg;
pub mod nav;
pub mod patch;
pub mod probe;
pub mod query;
pub mod verify;

pub use self::dispatch::dispatch;
pub use api::EngineApi;
pub use material::*;
pub use msg::*;
pub use nav::*;
pub use patch::*;
pub use probe::*;
pub use query::*;
pub use verify::*;

pub const JSONRPC: &str = "2.0";
/// 桥协议版本（方法表或字段语义不兼容地变更时 +1）。
pub const PROTOCOL_VERSION: u32 = 1;

/// 对外 RPC 能力方法表（57 条，字典序），与 `r3e_core::cmd_spec` 中 `rpc = Some` 的条目一一对应。
///
/// 为什么单独列出：`dispatch` 的 `match` 臂比它多 3 条——传输层心跳 `ping`（不计入能力），
/// 以及两条历史别名 `enginestats` / `captureframe`（无点旧写法，仍接受但不再声明）。
/// 能力清单（`docs/capabilities.json`）与 `cargo xtask gen-capabilities` 的硬断言都要
/// 「57 == 命令表 rpc 条目数 == 本表长度」，故必须有一条机器可读的权威列表，
/// 而不是靠人肉数 `match` 臂。
///
/// 增删方法的三步：改 `dispatch` 臂 → 改 `cmd_spec` → 改本表（`cargo xtask
/// gen-capabilities` 会立刻红）。
///
/// M1-S1 增 5 条 `tx.*`（39 → 44）：事务协议上链。M1-S2-B 增 3 条 `query.*`（44 → 47）：
/// 语义查询面。这两批都**不新增脚本函数**，故只动本表与 `cmd_spec` 的 `Meta` 域，
/// 脚本契约测试不受影响；S3/S4 的写入口则是 `tx.apply` 的 `op` 白名单，不再动本表。
///
/// M1-S4-D 增 8 条（48 → 56），第一次为「只读查询面」扩容：
/// `physics.raycast` / `physics.overlap` / `physics.cast`（物理探测）、
/// `nav.bake` / `nav.path` / `nav.reachability`（导航查询）、
/// `scene.material.list` / `scene.material.upsert`（场景材质表）。
/// 这八条同样不新增脚本函数（脚本侧直接拿 `&mut Scene` / `&Physics`），
/// 所以 `cmd_spec` 里落在 `Meta` 域、`rpc = Some` 与 `name` 同名，
/// 脚本契约的「域内函数数」断言不受影响。
///
/// M1-S4-F 增 1 条 `scene.verify`（56 → 57）：三层体检的**只读**出口。
/// 它把 S4-E 的三层验证器从「进程内可调」变成「协议面可问」——AI / 远端不必自己
/// 遍历场景，也不会因为「宿主没接物理或导航」而把未检查误读成通过；
/// 同样不新增脚本函数，登记在 `cmd_spec` 的 `Meta` 域。
///
/// M1-S3-A 增 1 条 `tx.intent`（47 → 48）：意图原子单元。**这是本表第一次为
/// S3 扩容**——S3 的意图需要一个独立的 RPC 面来携带「一次命中 N 条命令」的语义
/// （`tx.apply` 的 `op` 白名单只能表达一条命令）。具体意图名仍走 `intent` 参数的
/// 宿主白名单，加意图不动本表。
///
/// 命名铁律：本表里的名字必须是 `dispatch::canonical()` 归一后的形态——只含小写
/// 字母、数字和点，**不能带下划线**（归一化会把下划线直接删掉，`tx.apply_intent`
/// 会被请求端归一成 `tx.applyintent` 而永远匹配不上；`cmd_spec` 的
/// `rpc名唯一且是点分小写` 也会红）。所以意图那条叫 `tx.intent`，Rust 侧方法名
/// 仍是 `tx_intent`。
pub const RPC_METHODS: &[&str] = &[
    "audio.spatial.clear",
    "audio.spatial.list",
    "audio.spatial.listener",
    "audio.spatial.remove",
    "audio.spatial.set",
    "audio.spatial.setenabled",
    "capture.frame",
    "engine.info",
    "engine.stats",
    "light.clear",
    "light.list",
    "light.remove",
    "light.set",
    "light.setenabled",
    "nav.bake",
    "nav.path",
    "nav.reachability",
    "physics.cast",
    "physics.overlap",
    "physics.raycast",
    "query.describe",
    "query.find",
    "query.stats",
    "scene.animlayers",
    "scene.animlayerset",
    "scene.archive.load",
    "scene.archive.save",
    "scene.back",
    "scene.get",
    "scene.list",
    "scene.material.list",
    "scene.material.upsert",
    "scene.remove",
    "scene.save",
    "scene.setenvironment",
    "scene.setenvironmentmap",
    "scene.setmaterial",
    "scene.setpostfx",
    "scene.setssao",
    "scene.settransform",
    "scene.shadow",
    "scene.spawn",
    "scene.stack",
    "scene.switch",
    "scene.verify",
    "script.eval",
    "script.reload",
    "tx.apply",
    "tx.begin",
    "tx.commit",
    "tx.intent",
    "tx.preview",
    "tx.rollback",
    "ui.list",
    "ui.remove",
    "ui.set",
    "ui.state",
];

// JSON-RPC 2.0 标准错误码
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// 引擎侧业务错误（例如「找不到物件」）。
pub const ENGINE_ERROR: i32 = -32000;
/// 鉴权失败（配置了 `--rpc-token` 但请求没带或带错）。
pub const UNAUTHORIZED: i32 = -32001;
/// 桥过载：待处理队列已满，或并发连接数达到上限。客户端应稍后重试。
pub const SERVER_BUSY: i32 = -32002;
/// 主线程在超时时间内没有回包（引擎卡顿或主循环已停）。
pub const RPC_TIMEOUT: i32 = -32003;

// ---- 事务协议业务错误码（M1-S1，占用 -32010 起的独立段，与桥传输层错误区分）----

/// `txId` 不存在，或该事务已经 `commit` / `rollback` 终结。
pub const TX_NOT_FOUND: i32 = -32010;
/// 已有未决事务。同一文档同时只允许一笔——排队或隐式提交上一笔都会让「回滚到底回哪」失去定义。
pub const TX_BUSY: i32 = -32011;
/// 意图被拒：`op` 不在 AI 可写域白名单内，或其参数过不了命令层校验。
pub const TX_REJECTED: i32 = -32012;
/// 基哈希冲突：`commit` 时文档哈希与 `tx.begin` 的记录不一致，说明事务外有人改了文档。
pub const TX_CONFLICT: i32 = -32013;
/// 提交期某条命令执行失败，整笔不落（原子性），事务保持未决。
pub const TX_APPLY_FAILED: i32 = -32014;

// ---- 探测 / 导航查询业务错误码（M1-S4-D，占用 -32020 起的独立段）----
//
// 这三条的语义分界必须守住——它们回答的是不同问题，混用会让 AI 把「能力没有」
// 当成「查了没有」：
// * `PHYSICS_DISABLED`：物理总开关是关的，先把开关打开再问；
// * `BACKEND_UNSUPPORTED`：开关开着，但当前后端不申报这项查询能力；
// * `NAV_NOT_BAKED`：导航数据压根没烘过。

/// 物理开关是关的（`Physics::enabled() == false`）。**不是**「没命中」。
pub const PHYSICS_DISABLED: i32 = -32020;
/// 当前物理后端不申报这项查询能力（`BackendCaps::raycast` / `overlap` / `shape_cast` 为假）。
pub const BACKEND_UNSUPPORTED: i32 = -32021;
/// 导航数据还没烘焙（宿主侧 `Nav` 槽位为空），`nav.path` / `nav.reachability` 无从计算。
pub const NAV_NOT_BAKED: i32 = -32022;
