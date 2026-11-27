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
//! | `client.rs` | 引擎 RPC 客户端（M1-S5：TCP + NDJSON 长连接、令牌注入、方法名校验） |
//! | `patch.rs` | 14 个 `*Patch`（只给要改的字段，其余保持原值） |
//! | `api.rs` | `trait EngineApi`（73 方法，全部在主线程调用；`nav.*` 十条在 `api/nav.rs`） |
//! | `api/nav.rs` | `trait NavApi`：`nav.*` 十条的协议面（M2-D04 拆出，`EngineApi` 继承） |
//! | `dispatch/mod.rs` | 派发骨架 + 参数解码 + 元命令（`ping` / `engine.*` / `script.*` / `capture.*`） |
//! | `dispatch/tx.rs` | `tx.*` 事务臂（M1-S1：begin / apply / preview / commit / rollback） |
//! | `dispatch/scene.rs` | `scene.*` / `anim.*` 臂 |
//! | `dispatch/physics.rs` | `physics.*` 查询臂（M1-S4-D：raycast / overlap / cast） |
//! | `dispatch/nav.rs` | `nav.*` 臂（M1-S4-D 三条；M2-D04 补到十条） |
//! | `dispatch/inspect.rs` | 只读检视臂 + 未知方法（M1-S1-D：snapshot / mesh.stats / validate / simulate） |
//! | `dispatch/media.rs` | `ui.*` / `light.*` / `audio.*` 臂 |
//! | `probe.rs` | `physics.*` 参数 DTO（形状收敛，不含引擎类型） |
//! | `nav.rs` | `nav.*` 参数 DTO（`NavConfig` 字段平铺成 `Option`） |
//! | `inspect.rs` | 只读检视参数 DTO（M1-S1-D：场景来源 / 网格形状 / 模拟参数） |
//! | `material.rs` | `scene.material.upsert` 参数 DTO（整行声明，不是补丁） |
//! | `terrain.rs` | `terrain.*` 参数 DTO（M2-C06：扁平数值，不认 `TerrainPatch`） |

pub mod api;
pub mod caps;
pub mod character;
pub mod client;
pub mod dispatch;
pub mod inspect;
pub mod material;
pub mod msg;
pub mod nav;
pub mod patch;
pub mod probe;
pub mod query;
pub mod terrain;
pub mod verify;

pub use self::character::*;
pub use self::client::*;
pub use self::dispatch::{canonical_method, dispatch};
pub use self::terrain::*;
pub use api::{EngineApi, NavApi, PrefabApi, PrefabInfoRequest, PrefabSpawnRequest};
pub use inspect::*;
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

/// 对外 RPC 能力方法表（83 条，字典序），与 `r3e_core::cmd_spec` 中 `rpc = Some` 的条目一一对应。
///
/// 为什么单独列出：`dispatch` 的 `match` 臂比它多 3 条——传输层心跳 `ping`（不计入能力），
/// 以及两条历史别名 `enginestats` / `captureframe`（无点旧写法，仍接受但不再声明）。
/// 能力清单（`docs/capabilities.json`）与 `cargo xtask gen-capabilities` 的硬断言都要
/// 「83 == 命令表 rpc 条目数 == 本表长度」，故必须有一条机器可读的权威列表，
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
/// M1-S4-H 增 1 条 `physics.bind`（57 → 58）：编辑态的**显式装配口**。
///
/// 运行时的装配发生在主循环里（`if !paused` 分支），编辑态一暂停主世界就永不装配，
/// 于是 `physics.*` 三条在编辑态拿到的是「世界是空的」这个既不可信、又不报错的答案
/// （`hit: false` 与「世界没装」用同一种回包），`scene.verify` 的物理层也永远被记
/// `unchecked`。这条口子把「装配」变成调用方能显式请求的动作：
/// 绑的是**主世界**、**不写回** `Node::body`（回包显式带 `wroteBackScene: false`），
/// 幂等靠后端句柄表（连调两次第二次全是 `reused`）。
/// 同样不新增脚本函数，登记在 `cmd_spec` 的 `Meta` 域。
///
/// M1-S1-D 增 4 条（58 → 62）：把插件的四项**离线只读工具**搬到引擎侧，
/// 好让 `r3e-plugin` 摘掉 core / physics 依赖（还 `dep_debt` 前两条）：
/// `scene.snapshot`（结构快照）、`mesh.stats`（程序化网格计数 + AIR）、
/// `scene.validate`（逐节点网格体检，带结构化定位）、
/// `physics.simulate`（临时世界跑一段，回报位移与事件）。
/// 四条全部**只读场景**——`physics.simulate` 在宿主侧另建世界跑，算完只回位移，
/// `Node` 一个字段都不改；所以同样不新增脚本函数，落在 `cmd_spec` 的 `Meta` 域。
///
/// M2-B01 增 6 条（62 → 68）：角色控制器命令面。`character.*` 六条与脚本函数同名
/// （`cmd_spec` 的 `character` 域），是 B 阶段唯一新增的对外入口——把「角色站在哪、
/// 能不能走」从编辑器内部能力变成协议面可驱动，B05 的可玩性回归才不必依赖 GUI。
/// 六条全部挂 `Domain::Physics`（吃的是物理查询），故本表增员而 `domains` 不变。
///
/// M2-C06 增 8 条（68 → 76）：地形命令面。`terrain.*` 覆盖「造 / 删 / 采样 / 分块 /
/// 重采样 / 生成 / 汇总 / 取网格」全链，**八条全带 RPC**（CD-1 档 A 的直接后果：
/// 卡表要 rpc+8，而守恒规则 4 禁 `script_only` 增，故全局 80 → 85）。
/// 七条挂 `Domain::Terrain`（新域，域数 17 → 18），`terrain.mesh` 挂 `Domain::Meta`
/// ——它只读不写、给的是派生几何统计，跟 `mesh.stats` / `scene.snapshot` 同一族，
/// 若另开一个「地形渲染」域，域表就会为一张只读卡多开一栏。
///
/// M2-D04 增 7 条（76 → 83）：导航运维面。`nav.smooth` / `nav.stats` / `nav.export` /
/// `nav.import` / `nav.probe` / `nav.nearest` / `nav.hash` **七条全带 RPC**（`DD-4` 档 A），
/// 全部挂 `Domain::Meta`——它们吃的是已烘好的 `Nav`，既不写场景也不新增域，与
/// `nav.bake` 三条同族但把「体检 / 存取 / 吸附」从只在编辑器内部可用变成协议面可驱动。
/// 同一批里 `nav.*` 的协议面搬到了 `api/nav.rs` 的子 trait，本表条目只是方法名清单，
/// 契约注释不在这里。
///
/// M2-E04 增 2 条（83 → 85）：prefab 实例化面。`prefab.spawn`（跨进程写入口）
/// 与 `prefab.info`（跨进程读入口）**两条带 RPC**（`M2_E_PLAN` §3.6，`P5` = B）。
/// `prefab_define` / `spawn_many` / `set_override` / `clear_override` 四条**故意不进本表**：
/// 模板建库与批量操作只走脚本或宿主本地调用，代价与补口触发条件见 `M2_E_PLAN` §13 `R7`。
///
/// 命名铁律：本表里的名字必须是 `dispatch::canonical_method()` 归一后的形态——只含小写
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
    "character.config",
    "character.create",
    "character.state",
    "character.stats",
    "character.step",
    "character.teleport",
    "engine.info",
    "engine.stats",
    "light.clear",
    "light.list",
    "light.remove",
    "light.set",
    "light.setenabled",
    "mesh.stats",
    "nav.bake",
    "nav.export",
    "nav.hash",
    "nav.import",
    "nav.nearest",
    "nav.path",
    "nav.probe",
    "nav.reachability",
    "nav.smooth",
    "nav.stats",
    "physics.bind",
    "physics.cast",
    "physics.overlap",
    "physics.raycast",
    "physics.simulate",
    "prefab.info",
    "prefab.spawn",
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
    "scene.snapshot",
    "scene.spawn",
    "scene.stack",
    "scene.switch",
    "scene.validate",
    "scene.verify",
    "script.eval",
    "script.reload",
    "terrain.blocks",
    "terrain.create",
    "terrain.gen",
    "terrain.mesh",
    "terrain.remove",
    "terrain.resample",
    "terrain.sample",
    "terrain.stats",
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
/// 宿主拿不到网格来源（`EngineAccess::meshes` 为空），`physics.bind` 无从装配。
///
/// 为什么不与 `BACKEND_UNSUPPORTED` 合用：那是「后端不会」，这是「宿主这份进程里
/// 根本没有几何来源」（headless / 资产库未就绪）。两者该做的事不同——前者换后端，
/// 后者补网格来源——合成一个码等于让调用方去猜。
pub const MESH_SOURCE_UNAVAILABLE: i32 = -32023;
/// 该节点还没有角色控制器（`character.step` / `state` / `teleport` 的前置条件没满足）。
///
/// 与 `NAV_NOT_BAKED` 同一教训：**绝不回全零运行态**。`position: [0,0,0]` 会被读成
/// 「角色在世界原点站着」，然后据此去调地形、判定掉落，越查越偏。
pub const CHARACTER_NOT_READY: i32 = -32030;
/// 地形块名不存在（`terrain.remove` / `terrain.sample` / `terrain.resample` 找不到同名块）。
///
/// 与 `CHARACTER_NOT_READY` 同一教训的另一种形态：**「没有这块地形」必须能跟
/// 「这块地形高度是 0」分开**。平坦地面合法地返回 `height: 0`，若「查不到」也回 0，
/// 调用方就会把「拼错名字」当成「这片地是平的」，然后据此把角色放到半空。
pub const TERRAIN_PATCH_MISSING: i32 = -32040;
/// 块索引越界，或该块没有任何地形覆盖（`terrain.mesh`）。
pub const TERRAIN_BLOCK_MISSING: i32 = -32041;
/// 节点上的角色配置非法（半径 <= 0、坡度越界、身高小于半径等），控制器不会建立。
///
/// 判据来自 `CharacterSpec::validate`，回包 `message` 直接带它的原文——不在宿主另写
/// 一份「参数不对」的话术，否则同一个上限两个真源。
pub const CHARACTER_SPEC_INVALID: i32 = -32031;
/// `prefab.*` 引用了一个不存在的东西：模板 ID 没登记（`prefab.spawn` 前没
/// `prefab_define`），或实例名不是 prefab 实例（`prefab.info` / `set_override`）。
///
/// 为什么不并到 `INVALID_PARAMS`：参数形状是对的，错的是引用目标的状态。
/// 两者的修复动作完全不同——前者「先把模板登记进来」，后者「把参数改对」；
/// 合成一个码之后调用方只能去读 message 文本猜，而 message 是给人看的。
/// 也**不叫** `PREFAB_TEMPLATE_MISSING`：同族错误里还有「实例名不在」这半边，
/// 码名写窄了，后来人就会把那半边硬塞回参数错去。
pub const PREFAB_REFERENCE_MISSING: i32 = -32050;
