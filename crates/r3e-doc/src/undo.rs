//! 撤销栈：以「笔」为单位记录历史（S4-04）。
//!
//! S3 的栈是扁平的 `Vec<CommandKind>`，一次 `undo` 退一条命令。事务引入后这个粒度就不
//! 够了：一笔事务由 n 条命令组成、只推一次版本，撤销必须**一次退一整笔**，否则用户看到
//! 的是「按了 n 次撤销才回到上一笔之前」。
//!
//! 结构：
//!
//! ```text
//! groups: [ {mark:1, cmds:[a]}, {mark:2, cmds:[b,c,d]}, {mark:3, cmds:[e]} ]
//!                                 ^ 一笔事务（commit 时整组入栈）
//! ```
//!
//! - `mark` 单调递增，`Document::begin` 记下当前水位 `next_mark`，回滚时用它划出边界。
//! - 单命令路径仍是「一笔一条」，语义与 S3 完全一致（一次 undo 退一条）。
//! - 撤销失败时**不丢历史**：该笔里没撤掉的部分原样留在栈上，错误上抛。

use r3e_core::Scene;

use crate::{CommandKind, DocError};

/// 撤销栈水位标记。单调递增，`0` 表示「栈底」（空栈的水位）。
pub type Mark = u64;

/// 一笔历史（一组同生共死的命令）。
#[derive(Debug, Clone)]
pub struct UndoGroup {
    mark: Mark,
    cmds: Vec<CommandKind>,
}

impl UndoGroup {
    /// 该笔的水位标记（回滚边界的锚点）。
    pub fn mark(&self) -> Mark {
        self.mark
    }

    /// 该笔包含的命令条数。
    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    /// 该笔是否为空（正常入栈后恒为 `false`）。
    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// 该笔最后一条命令的展示名（撤销按钮上的文案）。
    pub fn label(&self) -> Option<&'static str> {
        self.cmds.last().map(CommandKind::label)
    }

    /// 该笔包含的命令（按执行顺序）。
    pub fn commands(&self) -> &[CommandKind] {
        &self.cmds
    }
}

/// 分组撤销栈。`Document` 独占持有一份。
#[derive(Debug)]
pub struct UndoStack {
    groups: Vec<UndoGroup>,
    next_mark: Mark,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    /// 空栈，第一笔的标记从 `1` 开始（`0` 保留为「栈底」水位）。
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            next_mark: 1,
        }
    }

    /// 笔数（= `Document::undo_depth` 的口径）。
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// 是否没有任何历史。
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// 命令总条数（笔内累加，性质测试与诊断用）。
    pub fn total_cmds(&self) -> usize {
        self.groups.iter().map(UndoGroup::len).sum()
    }

    /// 下一次入栈会拿到的标记（= 当前水位）。
    ///
    /// `Document::begin` 在事务开头抓一次，`rollback_to` 用它当边界。
    pub fn next_mark(&self) -> Mark {
        self.next_mark
    }

    /// 最近一笔。
    pub fn last(&self) -> Option<&UndoGroup> {
        self.groups.last()
    }

    /// 最近一笔的展示名。
    pub fn last_label(&self) -> Option<&'static str> {
        self.groups.last().and_then(UndoGroup::label)
    }

    /// 压入一笔「单命令」历史（S3 老路径）。
    pub fn push_single(&mut self, cmd: CommandKind) -> Mark {
        self.push_group(vec![cmd]).expect("单命令组非空")
    }

    /// 压入一笔多命令历史，返回该笔标记；空组不入栈，返回 `None`。
    pub fn push_group(&mut self, cmds: Vec<CommandKind>) -> Option<Mark> {
        if cmds.is_empty() {
            return None;
        }
        let mark = self.next_mark;
        self.next_mark += 1;
        self.groups.push(UndoGroup { mark, cmds });
        Some(mark)
    }

    /// 回滚到水位 `mark`：撤销标记 `>= mark` 的所有笔（逆序），返回撤销成功的命令条数。
    ///
    /// `rollback_to(next_mark())` 是空操作；`rollback_to(1)` 清空全部历史。
    ///
    /// 语义要点：**失败不丢历史**。某条命令撤销失败时循环停下，该笔里没撤掉的部分
    /// （含失败那条及更早的）原样留栈，错误上抛给调用方；已经撤掉的笔正常出栈。
    pub fn rollback_to(&mut self, mark: Mark, scene: &mut Scene) -> Result<usize, DocError> {
        let mut undone = 0;
        while self.groups.last().is_some_and(|g| g.mark >= mark) {
            let group = self.groups.pop().expect("循环条件已保证非空");
            let total = group.cmds.len();
            let mut keep = total;
            let mut failure: Option<DocError> = None;
            for i in (0..total).rev() {
                match group.cmds[i].undo(scene) {
                    Ok(()) => undone += 1,
                    Err(err) => {
                        failure = Some(err);
                        keep = i + 1;
                        break;
                    }
                }
            }
            if let Some(err) = failure {
                let mut partial = group;
                partial.cmds.truncate(keep);
                self.groups.push(partial);
                return Err(err);
            }
        }
        Ok(undone)
    }

    /// 清空全部历史（存盘 / 换场景后调用）。
    ///
    /// 水位不重置：`Mark` 只增不减，避免旧标记与新笔撞号（外部可能还攥着一个过期 mark）。
    pub fn clear(&mut self) {
        self.groups.clear();
    }
}
