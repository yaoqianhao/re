//! 会话式事务：跨多次 RPC 攒命令、一次提交（M1-S1）。
//!
//! [`crate::Transaction`] 要求 `&mut Document` 全程在手，适合「一次调用内跑完 n 条」；
//! 但 AI 侧的事务是**跨请求**的：`tx.begin` → 若干 `tx.apply` → `tx.preview` →
//! `tx.commit` 是四条独立 RPC，中间 `Document` 的 `RefCell` 会被释放。
//!
//! 本模块把「未决事务」降级成纯数据挂在 [`Document`] 上，只保留三样东西：`id`、
//! 基哈希 `base`、以及一条**影子场景** `shadow`。命令在影子上构造 + 执行，
//! 主场景在 `commit` 之前**一个字节都不动**。由此拿到三条性质：
//!
//! 1. **隔离**：事务期间主场景、版本号、撤销栈全都不变，其他 RPC 读到的永远是已提交态。
//! 2. **回滚零风险**：`rollback` 只是丢影子，不存在「某条 `undo` 不精确 → 回滚不完整」
//!    这种失败面（[`crate::Transaction`] 的 `RollbackFailed` 在本模块里只可能出现在
//!    `commit` 内部，且是防御性分支）。
//! 3. **冲突显式化**：`commit` 前比对 `state_hash()` 与 `base`。事务期间被别的路径改过
//!    文档时直接报 [`DocError::Conflict`]，而不是拿旧前像去覆盖别人的改动。
//!
//! 同一时刻**只允许一笔**未决事务（[`DocError::Busy`]）。多笔并行会让各自的影子各推一份
//! 未来，提交顺序不同得到不同结果——那是「先解决语义再谈并发」的问题，不是实现细节。
//!
//! 代价是 `begin` 时要 clone 一份 `Scene`（后续 `apply` 都是增量）。编辑器一次事务通常
//! 几条命令、秒级结束，这个代价按「一次 clone 换三段不变量」算得过。

use r3e_core::Scene;

use crate::{scene_hash, CommandKind, DocError, Document, StateHash};

/// 一笔未决事务。
///
/// 由 [`Document::tx_begin`] 创建，`commit` / `rollback` 后从文档上摘掉。
/// 未决期间若文档被换了场景（[`Document::replace_scene`]），本事务一并作废——
/// 前像指向的是另一个场景，留着只会在提交时制造一个内容正确的错误结果。
#[derive(Debug)]
pub struct TxSession {
    id: String,
    base: StateHash,
    base_version: u64,
    /// 主场景在 `begin` 时刻的副本；`apply` 在这上面推进。
    shadow: Scene,
    /// 已成功暂存的命令（按执行顺序）。`commit` 时整组入撤销栈。
    staged: Vec<CommandKind>,
    /// 与 `staged` 一一对应的意图名（`scene.settransform` 之类），供回包与诊断用。
    ops: Vec<String>,
}

impl TxSession {
    /// 事务 id（宿主的 `txId`）。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 事务开始时的内容哈希。`commit` 用它做冲突判定。
    pub fn base(&self) -> StateHash {
        self.base
    }

    /// 事务开始时的版本号（`commit` 后应恰好 +1，除非事务为空）。
    pub fn base_version(&self) -> u64 {
        self.base_version
    }

    /// 已暂存命令条数。
    pub fn len(&self) -> usize {
        self.staged.len()
    }

    /// 是否还没暂存任何命令。
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// 已暂存的意图名（按执行顺序）。
    pub fn ops(&self) -> &[String] {
        &self.ops
    }

    /// 影子场景的只读视图：= 「事务提交后的样子」。
    ///
    /// 会话式事务的意义就在这条视图：AI 可以拿它做「先看一眼再决定」，
    /// 而不必真的落一次盘再撤销。
    pub fn shadow(&self) -> &Scene {
        &self.shadow
    }

    /// 影子场景的内容哈希（= 提交后将得到的哈希）。
    pub fn shadow_hash(&self) -> StateHash {
        scene_hash(&self.shadow)
    }
}

/// `tx_begin` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxBegin {
    /// 事务 id。
    pub id: String,
    /// 基哈希（提交时用于冲突判定）。
    pub base: StateHash,
    /// 基版本。
    pub version: u64,
}

/// `tx_apply` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxStaged {
    /// 意图名。
    pub op: String,
    /// 本步之后的暂存条数。
    pub staged: usize,
}

/// `tx_preview` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxPreview {
    /// 暂存条数。
    pub staged: usize,
    /// 基哈希（= 开启事务时的 `state_hash()`）。
    pub base: StateHash,
    /// 主场景当前哈希（≠ `base` 说明已发生冲突，提交会失败）。
    pub current: StateHash,
    /// 影子哈希（= 提交后将得到的哈希）。
    pub shadow: StateHash,
}

/// `tx_commit` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxCommit {
    /// 实际提交的命令条数。
    pub committed: usize,
    /// 提交后的内容哈希。
    pub hash: StateHash,
    /// 提交后的版本号。
    pub version: u64,
}

/// `tx_rollback` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxRollback {
    /// 被丢弃的命令条数。
    pub dropped: usize,
    /// 回滚后的内容哈希（应当等于 `base`）。
    pub hash: StateHash,
    /// 回滚后的版本号（应当等于 `base_version`）。
    pub version: u64,
}

impl Document {
    /// 未决事务的只读视图（`None` 表示当前没有事务）。
    pub fn tx_session(&self) -> Option<&TxSession> {
        self.tx.as_ref()
    }

    /// 开一笔事务。
    ///
    /// 只读一次哈希、clone 一份场景，不推进版本、不置脏、不进撤销栈。
    /// 已有未决事务时报 [`DocError::Busy`]——同一时刻只允许一笔。
    pub fn tx_begin(&mut self, id: impl Into<String>) -> Result<TxBegin, DocError> {
        if let Some(open) = &self.tx {
            return Err(DocError::Busy(format!(
                "已有未决事务 `{}`（{} 条待提交）",
                open.id,
                open.staged.len()
            )));
        }
        let id = id.into();
        let base = self.state_hash();
        let base_version = self.version;
        self.tx = Some(TxSession {
            id: id.clone(),
            base,
            base_version,
            shadow: self.scene.clone(),
            staged: Vec::new(),
            ops: Vec::new(),
        });
        Ok(TxBegin {
            id,
            base,
            version: base_version,
        })
    }

    /// 在影子场景上「看着当前状态造一条命令，再执行」。
    ///
    /// `build` 拿到的是影子场景（已含前面暂存命令的效果），所以前像永远基于
    /// 「事务走到这一步的样子」——与逐条 RPC 顺序执行等价。
    ///
    /// 任一步失败即**整笔作废**（影子丢弃、`tx_id` 失效）：半途的事务没有意义，
    /// 留着只会让调用方误以为还能接着攒。这条与 [`crate::Transaction`] 的 poisoned
    /// 策略一致，只是粒度更粗（一次作废而不是一次拒收）。
    pub fn tx_apply(
        &mut self,
        id: &str,
        op: impl Into<String>,
        build: impl FnOnce(&Scene) -> Result<CommandKind, DocError>,
    ) -> Result<TxStaged, DocError> {
        let session = self
            .tx
            .as_mut()
            .ok_or_else(|| DocError::TxNotFound(format!("没有未决事务，`{id}` 无从应用")))?;
        if session.id != id {
            return Err(DocError::TxNotFound(format!(
                "未决事务是 `{}`，不是 `{id}`",
                session.id
            )));
        }
        let op = op.into();
        // 先构造、后执行：构造失败（意图非法）不该污染影子，更不该作废整笔事务。
        let cmd = build(&session.shadow)?;
        if let Err(err) = cmd.apply(&mut session.shadow) {
            self.tx = None;
            return Err(err);
        }
        session.staged.push(cmd);
        session.ops.push(op.clone());
        Ok(TxStaged {
            op,
            staged: session.staged.len(),
        })
    }

    /// 影子预演：影子哈希 + 主场景当前哈希 + 暂存条数。
    ///
    /// 取 `&self`，结构上不可能写主场景。
    pub fn tx_preview(&self, id: &str) -> Result<TxPreview, DocError> {
        let session = self
            .tx
            .as_ref()
            .ok_or_else(|| DocError::TxNotFound(format!("没有未决事务，`{id}` 无从预览")))?;
        if session.id != id {
            return Err(DocError::TxNotFound(format!(
                "未决事务是 `{}`，不是 `{id}`",
                session.id
            )));
        }
        Ok(TxPreview {
            staged: session.staged.len(),
            base: session.base,
            current: self.state_hash(),
            shadow: session.shadow_hash(),
        })
    }

    /// 提交：把暂存命令重放到主场景，整组入撤销栈，**版本只 +1**。
    ///
    /// 三个阶段，任一段失败都不留半成品：
    ///
    /// 1. **冲突判定**：`state_hash() != base` → [`DocError::Conflict`]，事务作废。
    ///    事务期间被别的 RPC / 手动编辑改过文档时，宁可让调用方重来，也不拿旧前像覆盖。
    /// 2. **重放**：逐条 `apply` 到真场景。影子已经跑通过一次，所以这里是「不该失败」的
    ///    防御分支；万一失败，逆序 `undo` 已应用的命令把文档还回去。
    /// 3. **记账**：整组 `push_group` + 一次 `touch()`。这是「版本只 +1」的唯一落点。
    ///
    /// 空事务提交是安全空操作：不作废、不推进版本、返回当前哈希（与
    /// [`crate::Transaction::commit`] 同语义），但事务本身仍然被摘掉——空事务没有继续存在的理由。
    pub fn tx_commit(&mut self, id: &str) -> Result<TxCommit, DocError> {
        let session = self
            .tx
            .as_ref()
            .ok_or_else(|| DocError::TxNotFound(format!("没有未决事务，`{id}` 无从提交")))?;
        if session.id != id {
            return Err(DocError::TxNotFound(format!(
                "未决事务是 `{}`，不是 `{id}`",
                session.id
            )));
        }
        let base = session.base;
        let current = self.state_hash();
        if current != base {
            let open = self.tx.take().expect("上面刚确认过存在");
            return Err(DocError::Conflict(format!(
                "事务 `{}` 开启于哈希 {}，当前已是 {}（{} 条暂存已丢弃）",
                open.id,
                base.raw(),
                current.raw(),
                open.staged.len()
            )));
        }

        let session = self.tx.take().expect("上面刚确认过存在");
        let total = session.staged.len();
        if total == 0 {
            return Ok(TxCommit {
                committed: 0,
                hash: current,
                version: self.version,
            });
        }

        let mut applied = 0;
        let mut failure: Option<DocError> = None;
        for cmd in &session.staged {
            match cmd.apply(&mut self.scene) {
                Ok(()) => applied += 1,
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }
        if let Some(err) = failure {
            let mut undone = 0;
            for cmd in session.staged[..applied].iter().rev() {
                if cmd.undo(&mut self.scene).is_ok() {
                    undone += 1;
                }
            }
            self.invalidate_hash_cache();
            if undone != applied {
                // 回退不完整：状态已不可信，标脏交给上层（存盘 / 重开）。
                self.dirty = true;
                return Err(DocError::RollbackFailed {
                    undone,
                    total: applied,
                });
            }
            return Err(err);
        }

        self.undo.push_group(session.staged);
        self.touch();
        Ok(TxCommit {
            committed: total,
            hash: self.state_hash(),
            version: self.version,
        })
    }

    /// 回滚：丢掉影子，主场景一个字节都没动过。
    ///
    /// 返回的哈希与版本应当分别等于 `base` 与 `base_version`——调用方可以直接断言。
    pub fn tx_rollback(&mut self, id: &str) -> Result<TxRollback, DocError> {
        let session = self
            .tx
            .as_ref()
            .ok_or_else(|| DocError::TxNotFound(format!("没有未决事务，`{id}` 无从回滚")))?;
        if session.id != id {
            return Err(DocError::TxNotFound(format!(
                "未决事务是 `{}`，不是 `{id}`",
                session.id
            )));
        }
        let session = self.tx.take().expect("上面刚确认过存在");
        Ok(TxRollback {
            dropped: session.staged.len(),
            hash: self.state_hash(),
            version: self.version,
        })
    }
}
