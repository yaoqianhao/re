//! 场景自洽校验（S3-P3 Validator v0）。
//!
//! **口径**：只查「场景自己说得通吗」，不碰外部资产库。渲染侧的网格 / 材质 / 骨骼解析
//! 要走资产导入（`MeshAabbCache` / `MeshData`），那部分留到 M1 的 Validator v1；
//! 本轮把能在场景结构内部判死的问题全部抓出来，让 `doc-gate` 之外多一道运行时兜底。
//!
//! 三条铁律：
//! 1. **不修复、不 panic、不短路**——一次扫描报全量问题，交给上层决定怎么处理；
//! 2. **只读**——只吃 `&Scene`，不碰写入口，不会因为校验把文档标脏；
//! 3. **顺序稳定**——结果按 `path` 排序，同 path 按 `code`，测试可逐条断言。

use std::fmt;

use r3e_core::Scene;

mod bounds;
mod invariants;
mod material;
mod refs;

pub use invariants::{by_code as issue_code_by_code, IssueCode, ISSUE_CODES};

/// 问题严重度。
///
/// `Error` = 场景自相矛盾，跑下去必然出错（引用悬空、key 重复、数值非有限）；
/// `Warn` = 可疑但可用（例如蒙皮尚未求解过关节矩阵）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warn,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// 单条问题。
///
/// `code` 是稳定的机器可读标识（测试与上层筛选都按它来），不要为了文案好看去改；
/// 文案走 `msg`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    /// 出问题的位置，形如 `nodes[3]`、`animations[0].bindings[2].node`、`ui[4].key`。
    pub path: String,
    pub msg: String,
}

impl Issue {
    pub(crate) fn error(
        code: &'static str,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            code,
            path: path.into(),
            msg: msg.into(),
        }
    }

    pub(crate) fn warn(
        code: &'static str,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warn,
            code,
            path: path.into(),
            msg: msg.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} @ {}：{}",
            self.severity, self.code, self.path, self.msg
        )
    }
}

/// 校验整份场景。返回全部问题（可能为空），顺序稳定。
pub fn validate(scene: &Scene) -> Vec<Issue> {
    let mut out = Vec::new();
    refs::check(scene, &mut out);
    bounds::check(scene, &mut out);
    material::check(scene, &mut out);
    out.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.msg.cmp(&b.msg))
    });
    out
}

/// 带场景目录的完整校验：在 [`validate`] 之上额外判定 `materials[].*` 贴图文件是否存在。
///
/// `dir` 是场景文件所在目录（相对贴图路径的解析基准）。只读文件元数据，不解码、不改场景。
pub fn validate_in_dir(scene: &Scene, dir: &std::path::Path) -> Vec<Issue> {
    let mut out = Vec::new();
    refs::check(scene, &mut out);
    bounds::check(scene, &mut out);
    material::check_in_dir(scene, dir, &mut out);
    out.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.code.cmp(b.code))
            .then_with(|| a.msg.cmp(&b.msg))
    });
    out
}

/// 只要 `Error`，过滤掉 `Warn`。上层「能不能继续跑」的判据用这个。
pub fn errors(scene: &Scene) -> Vec<Issue> {
    validate(scene)
        .into_iter()
        .filter(Issue::is_error)
        .collect()
}
