//! 模板表：会话内的模板注册表。
//!
//! 范式照 `spawn.rs` 的表：**容量上限 + 收敛 + upsert**
//! （[`PrefabTable::define`] 同 ID 覆盖、[`PrefabTable::normalize_all`] 批量收敛）。
//!
//! 表里存 `Arc<PrefabTemplate>`，实例只克隆 `Arc`：E02 判据①（100 个实例的模板句柄
//! `Arc::ptr_eq` 全等）靠的就是「同一个 ID 拿到的永远是同一个 `Arc`」。
//!
//! 表**不参与序列化**：模板是素材，场景文件只记实例（E02 判据⑫）。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::prefab::{validate_prefab_id, PrefabIdError, PrefabTemplate, PREFAB_MAX};

/// 注册结果（新增还是覆盖了同名模板）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefineOutcome {
    /// 表里原本没有这个 ID。
    Inserted,
    /// 表里已有同 ID，已被本次替换。
    Replaced,
}

/// 注册失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefabTableError {
    /// 模板 ID 形状非法。
    Id(PrefabIdError),
    /// `proto.name` 为空（实例化后无法入场景）。
    ProtoNameEmpty,
    /// 表已满且本次不是替换。
    Full { limit: usize },
}

impl std::fmt::Display for PrefabTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(err) => write!(f, "模板 ID 非法：{err}"),
            Self::ProtoNameEmpty => write!(f, "模板的 proto.name 不能为空"),
            Self::Full { limit } => write!(f, "模板表已满（上限 {limit}）"),
        }
    }
}

impl std::error::Error for PrefabTableError {}

/// 会话内的模板注册表（按 ID 排序，`ids()` 输出稳定）。
#[derive(Debug, Clone, Default)]
pub struct PrefabTable {
    entries: BTreeMap<String, Arc<PrefabTemplate>>,
}

impl PrefabTable {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已注册条数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空表（`len() == 0`）。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 已注册的全部 ID（字典序，便于快照比对与工具枚举）。
    pub fn ids(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// 是否已注册该 ID。
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id.trim())
    }

    /// 借用取句柄（同一 ID 永远返回同一个 `Arc`，实例共享原型）。
    pub fn get(&self, id: &str) -> Option<&Arc<PrefabTemplate>> {
        self.entries.get(id.trim())
    }

    /// 取句柄（克隆 `Arc`，E02 的实例就用它持有模板）。
    pub fn handle(&self, id: &str) -> Option<Arc<PrefabTemplate>> {
        self.get(id).map(Arc::clone)
    }

    /// 注册：先收敛再校验，同 ID 覆盖（返回 [`DefineOutcome::Replaced`]）。
    ///
    /// 满表只在**新增**时挡住：替换已有 ID 不占新位，否则表满后连改模板都做不到。
    pub fn define(&mut self, template: PrefabTemplate) -> Result<DefineOutcome, PrefabTableError> {
        let template = template.sanitized();
        validate_prefab_id(&template.id).map_err(PrefabTableError::Id)?;
        if template.proto.name.trim().is_empty() {
            return Err(PrefabTableError::ProtoNameEmpty);
        }
        let id = template.id.clone();
        let replacing = self.entries.contains_key(&id);
        if !replacing && self.entries.len() >= PREFAB_MAX {
            return Err(PrefabTableError::Full { limit: PREFAB_MAX });
        }
        self.entries.insert(id, Arc::new(template));
        Ok(if replacing {
            DefineOutcome::Replaced
        } else {
            DefineOutcome::Inserted
        })
    }

    /// 注销；返回是否真的删掉了。
    pub fn remove(&mut self, id: &str) -> bool {
        self.entries.remove(id.trim()).is_some()
    }

    /// 批量收敛：非法 ID / 空原型名丢弃，同 ID 后者覆盖前者（保留首次出现的位置），
    /// 总量截到 [`PREFAB_MAX`]。
    ///
    /// 装载一批模板文件时走这一个入口——单条 [`Self::define`] 报错的场景在这里是
    /// 「丢掉这一条、其余照常」，因为一个坏文件不该拖垮整批素材。
    pub fn normalize_all<I>(templates: I) -> Vec<PrefabTemplate>
    where
        I: IntoIterator<Item = PrefabTemplate>,
    {
        let mut table = Self::new();
        let mut order: Vec<String> = Vec::new();
        for template in templates {
            let template = template.sanitized();
            let id = template.id.clone();
            if table.define(template).is_ok() && !order.iter().any(|seen| seen == &id) {
                order.push(id);
            }
        }
        order
            .into_iter()
            .filter_map(|id| table.handle(&id).map(|template| (*template).clone()))
            .collect()
    }
}
