//! 场景文件 / 脚本 `spawn` 的结构化描述类型（`ObjectDesc` 及其子结构）。
//!
//! 从 `scene.rs` 拆出：`scene.rs` 已越过 4000 行，而这一组类型是「场景文件长什么样」的
//! 唯一声明处，改字段时只需要看这一个文件。对外路径不变——`scene.rs` 原样重导出，
//! `crate::scene::ObjectDesc` 与 `crate::ObjectDesc` 都能用。

use serde::{Deserialize, Serialize};

use crate::SurfaceMaterial;

/// 单个物件在场景文件 / 脚本 `spawn` 中的描述。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ObjectDesc {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// 物理描述：只做数据承载，真正生效取决于启用的 `PhysicsBackend`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<PhysicsDesc>,
    /// PBR 表面材质（metallic / roughness / emissive）；省略表示不覆盖网格资产材质。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<SurfaceMaterial>,
    /// 场景材质表引用（M1-S4-A）：指向 `materials[].name`，与内联 `material` 互斥。
    ///
    /// 省略表示不引用（用网格资产材质或内联材质）。经
    /// [`crate::scene_material::normalize_material_ref`] 收敛：去空白、空串即不声明。
    #[serde(
        default,
        rename = "materialRef",
        skip_serializing_if = "Option::is_none"
    )]
    pub material_ref: Option<String>,
    /// 语义标签（如 `door` / `pickup` / `enemy`）；省略或空数组表示无标签。
    ///
    /// 写入前经 [`crate::normalize_tags`] 收敛：去首尾空白、丢空串、去重（保序）、
    /// 单标签截到 [`crate::NODE_TAG_MAX_LEN`] 个字符、总量截到 [`crate::MAX_NODE_TAGS`] 条。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// 可交互半径（世界单位）；省略表示该物件不可交互。
    ///
    /// 非有限值与 `<= 0` 一律按「不声明」处理（经 [`crate::normalize_interact_radius`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact_radius: Option<f32>,
}

/// 物理刚体描述（预留）。当前 `NullBackend` 会忽略，`rapier` 后端会消费。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PhysicsDesc {
    /// "dynamic" | "static" | "kinematic"
    #[serde(
        default = "default_body_kind",
        skip_serializing_if = "is_default_body_kind"
    )]
    pub body: String,
    #[serde(default = "default_mass", skip_serializing_if = "is_default_mass")]
    pub mass: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restitution: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<f32>,
    /// "box" | "sphere" | "cylinder"，缺省由 mesh 推导
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collider: Option<String>,
    /// 碰撞体半尺寸，缺省由 scale 推导
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub half_extents: Option<[f32; 3]>,
    /// 连续碰撞检测（CCD）：高速物体不穿过薄墙。
    ///
    /// 省略表示「跟场景级 `physics.ccd`」（再缺省为开）；显式写 `false` 可让
    /// 单个物件退出 CCD（例如大量慢速道具，省掉 sweep 开销）。
    /// 后端不申报 `ccd` 能力时该字段无效。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccd: Option<bool>,
}

/// 场景级物理配置：对应场景 JSON 顶层 `physics` 字段。
///
/// 所有字段都是可选的，缺省值由 `r3e_physics::PhysicsConfig` 决定：
/// 默认 enabled=false（物理关闭）、backend="builtin"、重力 -9.81、地面 y=0。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PhysicsSceneDesc {
    /// 是否启用物理模拟。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// "builtin"（内置最简物理）| "null"（空转，只登记刚体）| "rapier"（需构建时启用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// 重力加速度向量，缺省 [0.0, -9.81, 0.0]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<[f32; 3]>,
    /// 地面平面高度，缺省 0.0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<f32>,
    /// 所有刚体的 CCD 缺省开关（逐物件 `physics.ccd` 优先）。
    ///
    /// 缺省 `true`：后端申报 `ccd` 能力时默认开启，避免「高速物件穿墙」
    /// 这种一看就是缺陷的表现。脚本可用 `physics_set_ccd(false)` 全局关。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccd: Option<bool>,
}

fn default_body_kind() -> String {
    "dynamic".to_string()
}
fn default_mass() -> f32 {
    1.0
}

/// `PhysicsDesc::body` 的缺省值是 `"dynamic"`：等于缺省值时不写进 JSON。
fn is_default_body_kind(body: &str) -> bool {
    body == "dynamic"
}

/// `PhysicsDesc::mass` 的缺省值是 `1.0`：等于缺省值时不写进 JSON。
fn is_default_mass(mass: &f32) -> bool {
    *mass == 1.0
}
