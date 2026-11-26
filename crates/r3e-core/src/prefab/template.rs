//! 模板：一个可复用的「物件原型」+ 身份 + 版本 + 标签。
//!
//! 落盘位置由 ID 决定（见 [`crate::prefab::prefab_rel_path`]）：
//! `prop.crate` → `assets/prefab/prop/crate.json`。模板是**素材**，
//! 不进场景文件（E02 判据⑫：`scene.json` 里只有实例，没有模板表）。
//!
//! # 为什么 `proto` 直接用 [`crate::ObjectDesc`]
//!
//! 模板要描述的东西和场景里的一个物件是同一件事（网格、变换、物理、材质、标签……）。
//! 另造一套 `PrefabProto` 只会带来「两套字段名、两份默认值、两条校验路径」，
//! 而且实例化时还得逐字段翻译。复用 `ObjectDesc` 的代价是要接受它的 serde 习性
//! （`Option` + `skip_serializing_if`），收益是「模板里能写的」与「场景里能写的」
//! 天然一致——这正是 E02 判据⑬（实例化结果 == `apply(proto, overrides)`）成立的前提。
//!
//! # 未知字段
//!
//! 模板文件本身 `deny_unknown_fields`：拼错键名必须报错，不能静默丢掉——
//! 模板是长期资产，静默丢字段等于把错误延后到「美术发现模型不对」的那天。
//! 注意 `proto` 内层仍是 [`crate::ObjectDesc`] 的宽松口径（与 `scene.json` 一致），
//! 严格化那一层是另一张卡的事。

use serde::{Deserialize, Serialize};

use crate::prefab::{validate_prefab_id, PrefabIdError};
use crate::{normalize_tags, ObjectDesc, MAX_NODE_TAGS};

/// 模板文件格式版本，落在每个模板的 `schema` 字段上（判据⑱按它比对）。
///
/// 加字段不需要加版本（旧文件缺字段走默认值）；**改变已有字段的含义**才加。
pub const PREFAB_SCHEMA_VERSION: u32 = 1;

/// 单个会话里可注册的模板条数上限（判据⑬门禁口径，依据见 `M2_E_PLAN.md` §3）。
pub const PREFAB_MAX: usize = 256;

/// 模板标签条数上限：沿用节点标签的口径（[`MAX_NODE_TAGS`]），
/// 免得「模板标签」与「实例标签」两套上限互相打架。
pub const PREFAB_MAX_TAGS: usize = MAX_NODE_TAGS;

/// Prefab 模板。
///
/// 无 `PartialEq`：内层 [`crate::PhysicsDesc`] / [`crate::SurfaceMaterial`] 未实现
/// `PartialEq`，而判据只需要「序列化成 JSON 再比」——那也正好是本卡真正冻结的东西。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrefabTemplate {
    /// 模板 ID：`域.名` 点分小写，形状校验见 [`validate_prefab_id`]。
    pub id: String,
    /// 格式版本；省略即视为当前版本（[`PREFAB_SCHEMA_VERSION`]）。
    #[serde(default = "default_schema_version")]
    pub schema: u32,
    /// 原型：实例化时把这份 [`ObjectDesc`] 当底稿，再叠覆盖。
    pub proto: ObjectDesc,
    /// 检索用标签（`prop` / `enemy` / `breakable` …）；空数组不落盘。
    ///
    /// 经 [`normalize_tags`] 收敛：去空白、丢空串、去重保序、逐条截断、
    /// 总量截到 [`PREFAB_MAX_TAGS`]。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn default_schema_version() -> u32 {
    PREFAB_SCHEMA_VERSION
}

impl PrefabTemplate {
    /// 建一个当前版本的模板（`tags` 为空）。
    pub fn new(id: impl Into<String>, proto: ObjectDesc) -> Self {
        Self {
            id: id.into(),
            schema: PREFAB_SCHEMA_VERSION,
            proto,
            tags: Vec::new(),
        }
    }

    /// 收敛：ID 去首尾空白、标签归一。**不改** `proto`（原型由作者负责）。
    ///
    /// 写入路径（[`crate::prefab::PrefabTable::define`]）与 [`Self::normalized`] 都走这里，
    /// 保证「文件里写的」与「表里存的」是同一份口径。
    pub fn sanitized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.tags = normalize_tags(self.tags.iter().map(String::as_str));
        self.tags.truncate(PREFAB_MAX_TAGS);
        self
    }

    /// [`Self::sanitized`] 的借用版（不改自己，返回新值）。
    pub fn normalized(&self) -> Self {
        self.clone().sanitized()
    }

    /// 校验：ID 形状合法，且 `proto.name` 不为空（空名字的模板实例化后无法入场景）。
    pub fn validate(&self) -> Result<(), PrefabValidateError> {
        validate_prefab_id(&self.id).map_err(PrefabValidateError::Id)?;
        if self.proto.name.trim().is_empty() {
            return Err(PrefabValidateError::EmptyProtoName);
        }
        Ok(())
    }

    /// 版本是否为本进程认识的版本（未来版本的文件不静默按老规则解释）。
    pub fn version_is_current(&self) -> bool {
        self.schema == PREFAB_SCHEMA_VERSION
    }

    /// 原型名（实例化时若实例不给名字，就退回它——E02 决定，这里只提供读数）。
    pub fn proto_name(&self) -> &str {
        self.proto.name.trim()
    }

    /// 追加标签（先追加后归一，重复项由 [`normalize_tags`] 去掉）。
    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.tags
            .extend(tags.into_iter().map(|t| t.as_ref().to_string()));
        self.sanitized()
    }
}

/// 模板不符合契约的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefabValidateError {
    /// ID 形状非法。
    Id(PrefabIdError),
    /// `proto.name` 为空（或只有空白）。
    EmptyProtoName,
}

impl std::fmt::Display for PrefabValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(err) => write!(f, "模板 ID 非法：{err}"),
            Self::EmptyProtoName => write!(f, "模板的 proto.name 不能为空"),
        }
    }
}

impl std::error::Error for PrefabValidateError {}
