//! 显式事务：一笔事务 = N 条原子命令，版本只推一次（S4-02）。
//!
//! 与单命令路径 [`Document::apply`] 并存，语义不变：一条命令仍然「成功即 +1 版本」。
//! 事务只是把多条命令的记账合并成一次意图：
//!
//! ```ignore
//! let mut tx = doc.begin();
//! tx.apply(move_cmd)?;
//! tx.apply(tint_cmd)?;
//! tx.commit()?;          // 版本只 +1，撤销栈一次进 n 条（P3 起带同一 tx 标记）
//! ```
//!
//! 三条硬约束：
//!
//! 1. **原子性**：第 k 条失败 → 逆序撤销前 k-1 条，文档回到 `base`；回退本身再失败
//!    （说明某条命令的 `undo` 不精确）→ [`DocError::RollbackFailed`]，并把文档标脏——
//!    此时状态已不可信，禁止静默继续。
//! 2. **版本粒度**：`commit` 只推进一次版本，`rollback` 完全不推进。因此事务期间读取
//!    [`Document::state_hash`] 会看到「版本没变但内容变了」的中间态，本模块每次改动都
//!    主动清空哈希缓存（[`Document::invalidate_hash_cache`]），避免缓存把中间态当现值。
//! 3. **无嵌套**：`Transaction` 持有 `&mut Document`，类型系统直接挡住。

use crate::{CommandKind, DocError, Document, StateHash};

/// 一笔进行中的事务。
///
/// 由 [`Document::begin`] 创建。未 `commit` / `rollback` 就 drop 时**不回滚**——
/// 与 `Vec` 的「泄漏优于误清」同一条原则：静默撤销比留下半成品更难排查。
/// 需要「失败自动回滚」的调用方用 [`Transaction::apply`] 的返回值决定走 `rollback`。
pub struct Transaction<'a> {
    doc: &'a mut Document,
    /// 事务开始时的内容哈希（`rollback` 的收敛目标）。
    base: StateHash,
    /// 事务开始时的撤销栈深度（`rollback` 的截断目标）。
    base_depth: usize,
    /// 已成功执行的命令，按执行顺序排列。
    staged: Vec<CommandKind>,
    /// 首次失败后置位：此后任何写入都直接返回同一个错误。
    poisoned: Option<DocError>,
}

impl Document {
    /// 开一笔事务，冻结 `base` 哈希。
    ///
    /// 只读一次哈希，不推进版本、不置脏；事务本身不产生任何可观察副作用。
    pub fn begin(&mut self) -> Transaction<'_> {
        let base = self.state_hash();
        let base_depth = self.undo_stack.len();
        Transaction {
            doc: self,
            base,
            base_depth,
            staged: Vec::new(),
            poisoned: None,
        }
    }
}

impl<'a> Transaction<'a> {
    /// 事务开始时的内容哈希（`rollback` 的收敛目标）。
    pub fn base(&self) -> StateHash {
        self.base
    }

    /// 已成功执行的命令（按执行顺序）。
    pub fn staged(&self) -> &[CommandKind] {
        &self.staged
    }

    /// 已暂存的命令条数。
    pub fn len(&self) -> usize {
        self.staged.len()
    }

    /// 是否还没成功执行过任何命令。
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// 执行一条命令并暂存。
    ///
    /// 失败即整体回滚到 `base`（已成功的 k-1 条逆序撤销），并把事务置为 poisoned：
    /// 后续 `apply` 一律返回首错，不再继续往一个已作废的事务里塞命令。
    pub fn apply(&mut self, cmd: CommandKind) -> Result<(), DocError> {
        if let Some(err) = &self.poisoned {
            return Err(err.clone());
        }
        if let Err(err) = cmd.apply(&mut self.doc.scene) {
            let total = self.staged.len();
            let undone = self.undo_staged();
            self.doc.invalidate_hash_cache();
            if undone != total {
                // 回退不完整：文档状态已不可信，标脏交给上层决定（存盘 / 重开）。
                self.doc.dirty = true;
                let fatal = DocError::RollbackFailed { undone, total };
                self.poisoned = Some(fatal.clone());
                return Err(fatal);
            }
            self.poisoned = Some(err.clone());
            return Err(err);
        }
        self.staged.push(cmd);
        // 事务期间版本不动，缓存必须手动失效，否则 `state_hash()` 会把中间态当现值。
        self.doc.invalidate_hash_cache();
        Ok(())
    }

    /// 事务期间的只读投影（读版本、撤销深度、当前哈希都走它）。
    ///
    /// 事务持有 `&mut Document`，外部拿不到 `&Document`；这条通道让上层不必为读一个
    /// 计数器而放弃事务形式。
    pub fn document(&self) -> &Document {
        self.doc
    }

    /// 事务期间场景的只读视图（= `self.document().as_scene()`）。
    pub fn scene(&self) -> &r3e_core::Scene {
        self.doc.as_scene()
    }

    /// 「看着当前状态造一条命令，再执行」。
    ///
    /// 事务独占 `&mut Document` 时调用方拿不到 `&Scene`，而命令的前像必须从**当前**
    /// （已含前面暂存命令的）状态抓——这条通道把「读-造-写」压进一次借用里。
    /// 构造失败（闭包返回 `Err`）不污染事务；执行失败与 [`Transaction::apply`] 同策略。
    pub fn stage(
        &mut self,
        build: impl FnOnce(&r3e_core::Scene) -> Result<CommandKind, DocError>,
    ) -> Result<(), DocError> {
        if let Some(err) = &self.poisoned {
            return Err(err.clone());
        }
        let cmd = build(self.doc.as_scene())?;
        self.apply(cmd)
    }

    /// 影子预演：在 `Scene` 的克隆上跑一串命令，返回执行后的**内容哈希**。
    ///
    /// 主文档完全不碰（零副作用由结构保证，哈希断言只是兜底）：`preview` 取 `&self`，
    /// 拿到的是不可变借用，即使想写也写不了。
    ///
    /// 预演的命令不带账：失败直接返回错误，影子丢弃，文档与撤销栈均无变化。
    pub fn preview(&self, cmds: &[CommandKind]) -> Result<StateHash, DocError> {
        let mut shadow = self.doc.scene.clone();
        for cmd in cmds {
            cmd.apply(&mut shadow)?;
        }
        Ok(crate::scene_hash(&shadow))
    }

    /// 提交：n 条命令一次性入撤销栈，版本只 +1。
    ///
    /// 空事务提交是安全的空操作：不推进版本，返回当前哈希。
    pub fn commit(mut self) -> Result<StateHash, DocError> {
        if let Some(err) = self.poisoned.take() {
            return Err(err);
        }
        if !self.staged.is_empty() {
            let staged = std::mem::take(&mut self.staged);
            self.doc.undo_stack.extend(staged);
            self.doc.touch();
        }
        Ok(self.doc.state_hash())
    }

    /// 回滚：逆序撤销已暂存的命令，文档回到 `base`，版本不推进。
    ///
    /// 返回回退后的哈希（应当等于 [`Transaction::base`]，调用方可以直接断言）。
    pub fn rollback(mut self) -> Result<StateHash, DocError> {
        let total = self.staged.len();
        let undone = self.undo_staged();
        // 事务期间撤销栈没动过，truncate 是幂等保险（防止将来有路径在事务里出栈）。
        self.doc.undo_stack.truncate(self.base_depth);
        self.doc.invalidate_hash_cache();
        if undone != total {
            self.doc.dirty = true;
            return Err(DocError::RollbackFailed { undone, total });
        }
        Ok(self.doc.state_hash())
    }

    /// 逆序撤销暂存命令，返回成功条数。
    ///
    /// 单条失败也继续往前撤：能救回多少算多少，最后由 `RollbackFailed{undone,total}` 交代。
    fn undo_staged(&mut self) -> usize {
        let mut undone = 0;
        while let Some(cmd) = self.staged.pop() {
            if cmd.undo(&mut self.doc.scene).is_ok() {
                undone += 1;
            }
        }
        undone
    }
}
