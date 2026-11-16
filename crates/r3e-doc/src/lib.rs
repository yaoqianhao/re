//! r3e-doc —— 文档层：场景的唯一真值持有者与唯一写入口。
//!
//! 分层（M0-S3）：
//!
//! ```text
//! script / editor-host / runtime  ──►  r3e-doc  ──►  r3e-core
//! ```
//!
//! 收口前后只差一条：改造前 `Rc<RefCell<Scene>>` 被 5 处共享持有，任一持有者都能直接
//! 可变借用（实测 129 个写点，改一半照样能跑）；改造后 [`Document`] 独占 `Scene`，
//! 只读经 [`Deref`] 透明转发（214 处只读点语法不变），写必须走 [`Document::write`]。
//! 编译器成为唯一裁判，漏改即编译失败，不存在灰区。
//!
//! 本轮（S3）只锁「可变借用的来源」：`Scene` 的公开字段保持不动，字段私有化推到
//! M1-DOC-FIELDS。原因见 `docs/M0S3_DOC_LAYER.md` §2 决定 2。

mod command;
mod state_hash;
mod transaction;
mod undo;
mod validate;
mod write;

use std::cell::Cell;
use std::fmt;
use std::ops::Deref;

use r3e_core::Scene;

pub use command::{Command, CommandKind};
pub use state_hash::{mix_f32, mix_str, mix_u64, scene_hash, world_hash, StateHash, FNV_OFFSET};
pub use transaction::Transaction;
pub use undo::{Mark, UndoGroup, UndoStack};
pub use validate::{
    errors as validate_errors, issue_code_by_code, validate, Issue, IssueCode, Severity,
    ISSUE_CODES,
};

/// 文档层错误。
///
/// 分四类：core 给出的错误、命令目标缺失、写入被拒、撤销栈为空。上层用 [`DocError::detail`]
/// 取正文展示，用变体决定要不要重试 / 提示用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    /// core 返回的错误字符串（场景解析失败、场景自身约束不满足）。
    Scene(String),
    /// 命令的目标不存在（节点 / 光源 / UI / 文本的 id 或 key 找不到）。
    NotFound(String),
    /// 目标存在但写入被拒（UI 元素表已满、局部光源已达上限）。
    Rejected(String),
    /// 撤销栈为空。
    NothingToUndo,
    /// 事务回滚不完整：某条命令的 `undo` 没能把状态精确还原。
    ///
    /// `undone` 是成功撤销的条数，`total` 是事务里已暂存的总条数。出现即代表文档状态
    /// 已不可信（事务内部会同时把文档标脏），上层应当提示用户重开或存盘。
    RollbackFailed {
        /// 成功撤销的条数。
        undone: usize,
        /// 事务中已暂存的条数。
        total: usize,
    },
}

impl DocError {
    /// 错误正文（不含变体前缀），便于上层直接展示。
    pub fn detail(&self) -> &str {
        match self {
            Self::Scene(e) | Self::NotFound(e) | Self::Rejected(e) => e,
            Self::NothingToUndo => "没有可撤销的操作",
            Self::RollbackFailed { .. } => "事务回滚不完整，文档状态已不可信",
        }
    }
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene(e) => write!(f, "场景错误：{e}"),
            Self::NotFound(e) => write!(f, "目标不存在：{e}"),
            Self::Rejected(e) => write!(f, "写入被拒：{e}"),
            Self::NothingToUndo => write!(f, "没有可撤销的操作"),
            Self::RollbackFailed { undone, total } => {
                write!(
                    f,
                    "事务回滚不完整：{undone}/{total} 条撤销成功，文档状态已不可信"
                )
            }
        }
    }
}

impl std::error::Error for DocError {}

/// 场景的唯一真值。
///
/// 只读：`document.camera` / `document.len()` 这类写法直接经 [`Deref`] 落到 `Scene`。
/// 写：只有 [`Document::write`] 一条路，每次写推进 [`Document::version`]。
///
/// 刻意不提供可变 Deref 转发：多一条可变通道，收口就白做（`cargo xtask doc-gate`
/// 规则 3 连这个关键词都不允许出现在本 crate 里）。
#[derive(Debug)]
pub struct Document {
    scene: Scene,
    version: u64,
    dirty: bool,
    writes: u64,
    undo: UndoStack,
    /// `state_hash()` 的缓存：`(当时的 version, 哈希)`。
    ///
    /// 用 `Cell` 是为了让 [`Document::state_hash`] 保持 `&self`（纯读语义）仍能填空缓存；
    /// 代价是 `Document` 不再 `Sync`——文档层本来就单线程持有，这个代价是明码的。
    hash_cache: Cell<Option<(u64, StateHash)>>,
}

impl Document {
    /// 用一份场景建档。初始 `version = 1`（0 保留给「没有文档」的语义）。
    pub fn new(scene: Scene) -> Self {
        Self {
            scene,
            version: 1,
            dirty: false,
            writes: 0,
            undo: UndoStack::new(),
            hash_cache: Cell::new(None),
        }
    }

    /// 从场景 JSON 建档。场景解析统一走这里（`doc-gate` 规则 4：上层不得直接调
    /// `Scene::from_json_str`）。
    pub fn from_json_str(src: &str) -> Result<Self, DocError> {
        Ok(Self::new(parse_scene(src).map_err(DocError::Scene)?))
    }

    /// 校验整份场景，返回全部问题（不修改文档、不标脏）。
    pub fn validate(&self) -> Vec<Issue> {
        validate::validate(&self.scene)
    }

    /// 只读视图。`Deref` 已经覆盖绝大多数场景，这个方法留给需要显式拿到 `&Scene`
    /// 的泛型位置（例如 `fn f(s: &Scene)` 的实参）。
    pub fn as_scene(&self) -> &Scene {
        &self.scene
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new(Scene::new())
    }
}

/// 只把场景 JSON 解析成 [`Scene`]，不建档。
///
/// 给「需要 owned `Scene`」的位置用：初始场景构造、[`Document::replace_scene`]、场景栈回退。
/// 解析路径与 [`Document::from_json_str`] 同一条，所以 `doc-gate` 规则 4 只认这两个入口——
/// 上层（runtime / script / editor-host）出现 `Scene::from_json_str` 即失败。
///
/// 错误类型故意保持 `String`（与 `r3e-core` 同形）：这些调用点的 `map_err` 文案与
/// `?` 转换都是按字符串写的，这里包一层 `DocError` 只会让上层多一次拆包。
pub fn parse_scene(src: &str) -> Result<Scene, String> {
    Scene::from_json_str(src)
}

impl Deref for Document {
    type Target = Scene;

    fn deref(&self) -> &Scene {
        &self.scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::ObjectDesc;

    fn doc_with_one_node() -> Document {
        let mut doc = Document::default();
        doc.write(|s| {
            s.spawn(ObjectDesc {
                name: "cube".to_string(),
                ..Default::default()
            })
            .expect("spawn 应当成功")
        });
        doc
    }

    #[test]
    fn new_document_is_clean_and_versioned() {
        let doc = Document::new(Scene::new());
        assert_eq!(doc.version(), 1);
        assert!(!doc.is_dirty());
        assert_eq!(doc.write_count(), 0);
    }

    #[test]
    fn default_document_holds_empty_scene() {
        let doc = Document::default();
        assert!(doc.is_empty());
        assert_eq!(doc.len(), 0);
    }

    #[test]
    fn deref_exposes_scene_reads() {
        let doc = doc_with_one_node();
        // 经 Deref 直接读 Scene 的方法与字段，语法与改造前一致。
        assert_eq!(doc.len(), 1);
        assert!(doc.find("cube").is_some());
        assert_eq!(doc.name, "untitled");
    }

    #[test]
    fn as_scene_matches_deref() {
        let doc = doc_with_one_node();
        let s: &Scene = doc.as_scene();
        assert_eq!(s.len(), doc.len());
        assert_eq!(s.name, doc.name);
    }

    #[test]
    fn write_advances_version_and_dirty() {
        let mut doc = Document::new(Scene::new());
        let before = doc.version();
        doc.write(|s| s.name = "edited".to_string());
        assert_eq!(doc.version(), before + 1);
        assert!(doc.is_dirty());
        assert_eq!(doc.name, "edited");
    }

    #[test]
    fn write_returns_closure_value() {
        let mut doc = Document::default();
        let id = doc
            .write(|s| {
                s.spawn(ObjectDesc {
                    name: "a".to_string(),
                    ..Default::default()
                })
            })
            .expect("spawn 应当成功");
        assert!(doc.get(id).is_some());
    }

    #[test]
    fn write_counts_every_call() {
        let mut doc = Document::default();
        doc.write(|s| s.frame = 1);
        doc.write(|s| s.frame = 2);
        assert_eq!(doc.write_count(), 2);
        assert_eq!(doc.version(), 3);
    }

    #[test]
    fn read_only_write_still_bumps_version() {
        // write 的语义是「经过写入口」，即便闭包内只读也推进版本：
        // 版本号是 S4 StateHash 的节拍器，宁可多跳一格，不可漏记。
        let mut doc = Document::default();
        let n = doc.write(|s| s.len());
        assert_eq!(n, 0);
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn mark_clean_clears_dirty_but_keeps_version() {
        let mut doc = Document::default();
        doc.write(|s| s.frame = 7);
        let v = doc.version();
        doc.mark_clean();
        assert!(!doc.is_dirty());
        assert_eq!(doc.version(), v);
    }

    #[test]
    fn replace_scene_returns_old_and_bumps_version() {
        let mut doc = doc_with_one_node();
        let before = doc.version();
        let mut next = Scene::new();
        next.name = "second".to_string();
        let old = doc.replace_scene(next);
        assert_eq!(old.len(), 1, "旧场景应当原样返回");
        assert_eq!(doc.name, "second");
        assert_eq!(doc.len(), 0);
        assert_eq!(doc.version(), before + 1);
        assert!(doc.is_dirty());
    }

    #[test]
    fn from_json_str_builds_document() {
        let doc = Document::from_json_str(MINIMAL_SCENE).expect("最小场景应当解析成功");
        assert_eq!(doc.version(), 1);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn from_json_str_reports_scene_error() {
        let err = Document::from_json_str("{ not json }").expect_err("非法 JSON 必须失败");
        assert!(matches!(err, DocError::Scene(_)));
        assert!(!err.detail().is_empty());
        assert!(err.to_string().contains("场景错误"));
    }

    #[test]
    fn parse_scene_returns_owned_scene_without_document() {
        // 需要 owned Scene 的位置（replace_scene / 场景栈回退）只解析、不建档：
        // 解析路径与 from_json_str 同一条，错误也统一成 DocError。
        let scene = parse_scene(MINIMAL_SCENE).expect("最小场景应当解析成功");
        assert_eq!(scene.name, "min");
        assert!(parse_scene("{ not json }").is_err());
    }

    /// 最小可用场景 JSON：只给一个空节点表。
    pub(crate) const MINIMAL_SCENE: &str = r#"{
        "name": "min",
        "nodes": []
    }"#;
}
