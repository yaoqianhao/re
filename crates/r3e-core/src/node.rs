//! 场景节点：物件本体与网格来源（内置程序化网格 / 外部资产网格）。

use crate::transform::Transform;
use glam::{Mat4, Vec4};

/// 节点唯一标识。单调递增，永不复用，脚本句柄因此不会指向错误对象。
pub type NodeId = u64;

/// 引擎内置的程序化网格（无需外部模型文件即可运行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BuiltinMesh {
    #[default]
    Cube,
    Sphere,
    Plane,
    Cylinder,
}

impl BuiltinMesh {
    pub const ALL: [Self; 4] = [Self::Cube, Self::Sphere, Self::Plane, Self::Cylinder];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cube" | "box" => Some(Self::Cube),
            "sphere" | "ball" => Some(Self::Sphere),
            "plane" | "ground" | "quad" => Some(Self::Plane),
            "cylinder" | "tube" => Some(Self::Cylinder),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cube => "cube",
            Self::Sphere => "sphere",
            Self::Plane => "plane",
            Self::Cylinder => "cylinder",
        }
    }
}

/// 外部资产的稳定标识。
///
/// 由资产路径经 FNV-1a 32 位散列得到：同一路径在同一进程内始终映射到同一个值，
/// 因此场景文本里可以直接写 `asset:<路径>#<网格序号>` 而无需运行期分配表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u32);

impl AssetId {
    /// 归一化路径（小写、正斜杠、去首尾空白）后取散列。
    pub fn from_path(path: &str) -> Self {
        let normalized = normalize_path(path);
        let mut hash: u32 = 0x811c_9dc5;
        for b in normalized.as_bytes() {
            hash ^= *b as u32;
            hash = hash.wrapping_mul(0x0100_0193);
        }
        Self(hash)
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:08x}", self.0)
    }
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

/// 节点使用的网格：内置程序化网格，或某个资产里的第 N 个网格。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshRef {
    Builtin(BuiltinMesh),
    Asset { asset: AssetId, mesh: u32 },
}

impl Default for MeshRef {
    fn default() -> Self {
        Self::Builtin(BuiltinMesh::Cube)
    }
}

impl MeshRef {
    pub fn asset(asset: AssetId, mesh: u32) -> Self {
        Self::Asset { asset, mesh }
    }

    /// 内置网格取值；资产网格返回 `None`。
    pub fn builtin(&self) -> Option<BuiltinMesh> {
        match self {
            Self::Builtin(b) => Some(*b),
            Self::Asset { .. } => None,
        }
    }

    /// 解析场景文本里的网格描述：
    ///
    /// * `cube|box` / `sphere|ball` / `plane|ground|quad` / `cylinder|tube`
    /// * `asset:<路径>#<网格序号>`（序号为资产内网格列表下标）
    pub fn parse(s: &str) -> Option<Self> {
        let raw = s.trim();
        if let Some(rest) = raw
            .strip_prefix("asset:")
            .or_else(|| raw.strip_prefix("ASSET:"))
        {
            let (path, index) = rest.rsplit_once('#')?;
            if path.trim().is_empty() {
                return None;
            }
            let mesh: u32 = index.trim().parse().ok()?;
            return Some(Self::Asset {
                asset: AssetId::from_path(path),
                mesh,
            });
        }
        BuiltinMesh::parse(raw).map(Self::Builtin)
    }

    /// 反向序列化，供脚本 / 日志回写使用。
    pub fn as_string(&self) -> String {
        match self {
            Self::Builtin(b) => b.as_str().to_string(),
            Self::Asset { asset, mesh } => format!("asset:{asset}#{mesh}"),
        }
    }

    pub fn is_asset(&self) -> bool {
        matches!(self, Self::Asset { .. })
    }
}

impl std::fmt::Display for MeshRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

/// 单个物件最多登记的语义标签数（M1-S2-C）。
pub const MAX_NODE_TAGS: usize = 16;

/// 单个标签的最大长度，按 `char` 计（M1-S2-C）。
pub const NODE_TAG_MAX_LEN: usize = 32;

/// 标签收敛：去首尾空白、丢空串、去重（保序）、逐条截断、整体截到 [`MAX_NODE_TAGS`]。
///
/// `Scene::spawn`（场景文件装入与脚本/协议生成共用）走这一个入口，保证
/// 「场景文件里写了什么」与「查询面看到什么」是同一份口径。
pub fn normalize_tags<I, S>(raw: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = Vec::new();
    for tag in raw {
        let tag = tag.as_ref().trim();
        if tag.is_empty() {
            continue;
        }
        let tag: String = tag.chars().take(NODE_TAG_MAX_LEN).collect();
        if out.iter().any(|t| t == &tag) {
            continue;
        }
        out.push(tag);
        if out.len() >= MAX_NODE_TAGS {
            break;
        }
    }
    out
}

/// 可交互半径收敛：非有限值（NaN / inf）与非正值一律视为「不声明」（`None`）。
///
/// 半径是「从物件原点算起多近算可交互」，0 与负数没有语义，直接当作不可交互，
/// 免得查询面拿到一个永远命中不了、又非 `None` 的半径。
pub fn normalize_interact_radius(r: f32) -> Option<f32> {
    (r.is_finite() && r > 0.0).then_some(r)
}

/// 场景中的一个物件。
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub transform: Transform,
    /// RGBA，线性空间。
    pub color: Vec4,
    pub mesh: MeshRef,
    pub visible: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// 物理后端的刚体句柄（预留）。
    pub body: Option<u64>,
    /// 场景文件里为该物件声明的物理描述（未声明为 None）。
    pub physics: Option<crate::scene::PhysicsDesc>,
    /// 物件级 PBR 表面材质（metallic / roughness / emissive）。
    ///
    /// `None` 表示「不覆盖」：走网格资产自带的材质，再退到默认材质。
    /// 写入前必须经过 [`crate::SurfaceMaterial::sanitized`] 收敛。
    pub material: Option<crate::SurfaceMaterial>,
    /// 场景材质表的引用（M1-S4-A）：指向 `Scene::materials[]` 里的 `name`。
    ///
    /// 与内联 [`Self::material`] **互斥**：两者同时声明由校验器报
    /// `material.ref_conflict`（保留内联是为了兼容存量场景，不删字段）。
    /// 写入前必须经过 [`crate::scene_material::normalize_material_ref`] 收敛。
    pub material_ref: Option<String>,
    /// 语义标签：给 AI 查询面按语义挑物件用（如 `door` / `pickup` / `enemy`）。
    ///
    /// 写入前必须经过 [`normalize_tags`] 收敛：非空串、去重、保序、条数与长度有上限。
    pub tags: Vec<String>,
    /// 可交互半径（世界单位）。`Some(r)` = 可交互，`None` = 不可交互。
    ///
    /// 写入前必须经过 [`normalize_interact_radius`] 收敛（非正值与非有限值视为不声明）。
    pub interact_radius: Option<f32>,
    /// 角色控制器配置（M2-B01）。`Some` = 这个物件由角色控制器驱动。
    ///
    /// 只放**静态**配置（半径 / 身高 / 坡度 / 台阶 / 吸附），每帧的位置与速度等
    /// 运行态**不在这里**——运行态归 `r3e-character` 的控制器，不进场景真值，
    /// 否则回放金标会随物理步进漂移（见 `docs/m2/M2_B_PLAN.md` §3.3）。
    ///
    /// 写入前必须经过 [`crate::CharacterSpec::validate`]；改它走 `r3e-doc` 的
    /// `SetCharacter` 命令（可撤销、进 `StateHash`）。
    pub character: Option<crate::CharacterSpec>,
    /// 每帧由 `Scene::update_world_transforms` 计算的缓存。
    pub world: Mat4,
    /// 世界法线矩阵缓存。
    pub world_normal: glam::Mat3,
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            transform: Transform::IDENTITY,
            color: Vec4::new(0.8, 0.8, 0.85, 1.0),
            mesh: MeshRef::default(),
            visible: true,
            parent: None,
            children: Vec::new(),
            body: None,
            physics: None,
            material: None,
            material_ref: None,
            tags: Vec::new(),
            interact_radius: None,
            character: None,
            world: Mat4::IDENTITY,
            world_normal: glam::Mat3::IDENTITY,
        }
    }

    /// 世界空间位置。
    pub fn world_position(&self) -> glam::Vec3 {
        self.world.w_axis.truncate()
    }
}
