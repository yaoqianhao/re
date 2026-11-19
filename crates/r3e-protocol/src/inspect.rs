//! 只读检视类 RPC 的参数 DTO（M1-S1-D）：`scene.snapshot` / `mesh.stats` /
//! `scene.validate` / `physics.simulate`。
//!
//! # 这四条为什么存在
//!
//! 插件（`r3e-plugin`）原先在**插件进程内**直连 `r3e-core` / `r3e-physics` 做这四件事：
//! 遍历场景出快照、程序化网格计数 + AIR 校验、逐节点网格体检、跑一段物理模拟。
//! 代价是插件二进制把 core 与整个物理后端（含 `rapier` feature）一起编进去，
//! 于是 `dep_rules.json` 里那两条债（plugin→core、plugin→physics）永远还不掉。
//!
//! 把计算搬到引擎侧、插件只做薄壳，债才消得掉。这里只收敛**参数形状**：
//! 缺省值、夹取范围、语义裁决（比如 `dt` 的合法区间、`backend` 是否编进了引擎）
//! 全在宿主与 core 里判——协议层抄一份就会变成两份真相。
//!
//! # 场景来源（`scene` / `path`）为什么两条并列
//!
//! 四条里有三条要「一份场景」才能算。插件原来的语义是：给了 `path` 就走宿主
//! `host/fs.read` 读文件，给了 `scene` 就吃内联 JSON（对象或文本）。搬过来之后
//! 读文件与解析都在宿主侧，参数形状保持不变：
//!
//! * `path` —— 宿主按「绝对路径 / 当前工作目录 / 当前场景目录」三档解析（与
//!   `scene.switch` 同一套口径）；
//! * `scene` —— 场景 JSON **文本**。内联对象在 `dispatch` 里先序列化成文本再下行，
//!   这样宿主只认一种形态，不必同时实现「值」与「文本」两条解析路径。
//!
//! 两条都缺省时取**当前编辑场景**：宿主手里本来就有一份，逼调用方回传一遍
//! 既浪费又容易过期（回传的是调用方手里的旧副本，算出来的结论对不上眼前这份）。
//! 「只给空白字符串算不算给了」属于值域判断，协议层不拦，交宿主判。

/// `scene.snapshot`：场景结构快照（节点表 + 相机 / 灯光 / 后处理 / 物理摘要）。
#[derive(Debug, Clone, Default)]
pub struct SnapshotQuery {
    /// 场景 JSON 文本（内联对象已由 `dispatch` 序列化）。
    pub scene: Option<String>,
    /// 场景文件路径（宿主侧解析，相对路径按工作目录 / 场景目录两档找）。
    pub path: Option<String>,
    /// 最多列多少个节点（缺省与上限由宿主夹取；`nodeCount` 始终是真实总数）。
    pub max_nodes: Option<u32>,
}

/// `mesh.stats`：程序化网格的统计与 AIR 校验。
///
/// 只认内置网格（`cube` / `sphere` / `plane` / `cylinder`）：外部资产网格要走资产
/// 管线，插件原来就明确跳过，这条口子不为它开口子。
#[derive(Debug, Clone, Default)]
pub struct MeshStatsQuery {
    /// 网格形状名；缺省 `cube`。
    pub shape: Option<String>,
    /// 球 / 圆柱的经向分段；缺省与夹取在宿主。
    pub segments: Option<u32>,
    /// 球的纬向环数。
    pub rings: Option<u32>,
    /// 平面的细分。
    pub divisions: Option<u32>,
}

/// `scene.validate`：逐节点网格体检（AIR），带结构化定位。
#[derive(Debug, Clone, Default)]
pub struct ValidateQuery {
    pub scene: Option<String>,
    pub path: Option<String>,
}

/// `physics.simulate`：把场景丢进物理世界跑一段，回报位移与事件。
///
/// 这条是四条里唯一**会建物理世界**的，但仍然只读场景：模拟在宿主侧的临时世界
/// 里跑，算完把每个刚体的起止位置回出去，场景文件一个字节都不改。
///
/// `backend` 只在引擎编进了对应后端时才认（`rapier` 属可选 feature）；
/// 点一个没编进去的后端必须报参数错误，而不是静默退回内置后端——后者会让
/// 「我要用 rapier 复现」变成「你其实用的是内置后端」，结论全错。
#[derive(Debug, Clone, Default)]
pub struct SimulateQuery {
    pub scene: Option<String>,
    pub path: Option<String>,
    /// 步数；缺省与夹取在宿主。
    pub steps: Option<u32>,
    /// 单步时长（秒）；区间校验在宿主。
    pub dt: Option<f32>,
    /// 最多列多少个刚体的位移。
    pub max_bodies: Option<u32>,
    /// 后端名（`null` / `builtin` / `rapier`）；缺省按场景 → 引擎默认的优先级取。
    pub backend: Option<String>,
    /// 重力覆盖。
    pub gravity: Option<[f32; 3]>,
    /// 地面高度覆盖。
    pub ground: Option<f32>,
}
