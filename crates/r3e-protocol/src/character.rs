//! `character.*` 六条命令的参数承载（M2-B01）。
//!
//! 协议层**零引擎依赖**，所以这里不出现 `CharacterSpec` / `Node`：五个参数各自
//! `Option<f32>`，由宿主决定「没给 = 缺省值」还是「保持现值」——
//! `character.create` 与 `character.config` 用的是同一个结构，**语义差别只在宿主**：
//!
//! * `create`：控制器不存在就按「给了的字段 + 缺省值」建，已存在则等价于 `config`。
//! * `config`：**只改给了的字段**，没给的保持现值（脚本调参不必回读再回写全量）。
//!
//! 参数名一律 camelCase（`slopeLimitDeg` / `stepHeight` / `snapDistance`），与
//! `physics.*` 的 `maxDistance` 同一口径；Rust 侧 snake_case。两边不做自动映射，
//! 由 dispatch 层显式取名——协议字段名是要写进文档给人看的，隐式映射只会让
//! 「为什么我的参数没生效」多一种解释不清的可能。

/// `character.create` / `character.config` 的入参。
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterBindQuery {
    /// 节点名（配置的真源是 `Node::character`）。
    pub node: String,
    /// 五个几何 / 手感参数；`None` = 按字段语义走缺省或保持现值。
    pub body: CharacterBodyParams,
}

/// 角色的五个可配参数（全 `Option`：缺省与否由宿主裁决）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CharacterBodyParams {
    /// 胶囊半径（米）。
    pub radius: Option<f32>,
    /// 站高（米，脚底到头顶）。
    pub height: Option<f32>,
    /// 可行走坡度上限（度）。
    pub slope_limit_deg: Option<f32>,
    /// 可直接抬腿跨上的台阶高度（米）。
    pub step_height: Option<f32>,
    /// 下坡时贴地吸附的最大距离（米）。
    pub snap_distance: Option<f32>,
}

/// `character.step` 的入参：一步的意图与步长。
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterStepQuery {
    /// 节点名。
    pub node: String,
    /// 水平意图方向（`[x, z]`，长度 0 = 原地）；缺省 = 原地。
    pub wish: [f32; 2],
    /// 是否请求起跳（只在着地时生效）。
    pub jump: bool,
    /// 步长（秒）；`None` = 宿主按固定帧长（1/60）走。
    pub dt: Option<f32>,
}

/// `character.state` 的入参：只有一个名字。
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterNameQuery {
    /// 节点名。
    pub node: String,
}

/// `character.teleport` 的入参：落到一个脚底位置。
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterTeleportQuery {
    /// 节点名。
    pub node: String,
    /// 目标**脚底**世界坐标（不是胶囊中心；换算由宿主按配置做）。
    pub position: [f32; 3],
}
