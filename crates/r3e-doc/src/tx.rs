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
    /// 已成功暂存的动作（按执行顺序）。每一步是一条意图或一条裸命令。
    staged: Vec<TxStep>,
}

/// 事务里的一步：一条意图（展开成 N 条命令）或一条裸命令（M1-S1 的 `tx.apply`）。
///
/// 分组的唯一理由是**记账粒度**：`commit` 时每一步各自入撤销栈一格，于是
/// 「一条意图 = 一次 undo 全退」这条语义在文档层就成立，不必让 `UndoStack`
/// 去猜哪些命令属于同一意图。裸命令每步只有一条，退化成 S1 的既有行为。
#[derive(Debug)]
pub struct TxStep {
    name: String,
    intent: bool,
    cmds: Vec<CommandKind>,
}

impl TxStep {
    /// 意图名 / 方法名（宿主回包与诊断用）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 是不是意图（`tx.intent` 攒的）而非裸命令（`tx.apply`）。
    pub fn is_intent(&self) -> bool {
        self.intent
    }

    /// 本步展开出的命令条数。
    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    /// 本步是否一条命令都没有。
    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }
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

    /// 已暂存**命令**条数（意图展开后的总数，不是步数）。
    pub fn len(&self) -> usize {
        self.staged.iter().map(TxStep::len).sum()
    }

    /// 是否还没暂存任何命令。
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// 已暂存的步数（一条意图算一步，一条裸命令也算一步）。
    pub fn steps(&self) -> usize {
        self.staged.len()
    }

    /// 已暂存的动作名（按执行顺序，意图与裸命令混排）。
    pub fn ops(&self) -> Vec<&str> {
        self.staged.iter().map(TxStep::name).collect()
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
    /// 本步之后的暂存步数（`tx.apply` 每步 +1）。
    pub intents: usize,
}

/// `tx_apply_intent` 的回包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxIntentStaged {
    /// 意图名（`spawn_prefab` / `place_light` / `paint_terrain` 之一）。
    pub intent: String,
    /// 本条意图展开出的命令条数。
    pub commands: usize,
    /// 本步之后的暂存命令总数。
    pub staged: usize,
    /// 本步之后的暂存步数。
    pub intents: usize,
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
    /// 已暂存的动作清单：`(名字, 展开的命令数)`，按执行顺序。
    ///
    /// 这是「变更半径」的第一层：AI 提交前据此确认自己到底攒了哪几条意图，
    /// 每条意图各有多大。
    pub intents: Vec<(String, usize)>,
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
                open.len()
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
        session.staged.push(TxStep {
            name: op.clone(),
            intent: false,
            cmds: vec![cmd],
        });
        Ok(TxStaged {
            op,
            staged: session.len(),
            intents: session.steps(),
        })
    }

    /// 在影子场景上暂存**一条意图**（M1-S3-A）：意图展开成 N 条原子命令，一起入栈。
    ///
    /// 与 [`Document::tx_apply`] 的两点差别，都是为了让 AI 能放心发大事：
    ///
    /// 1. **记账粒度**：N 条命令作为一个 [`TxStep`] 入事务（`tx_preview().intents` 按意图
    ///    聚合成一行），AI 看到的是「做了一件事」，而不是「撒了 N 条命令」。撤销粒度仍
    ///    按整笔事务算——一经 `commit`，一次 `undo` 全退，不会留下「地块画了一半」。
    /// 2. **失败原子性止于意图**：意图内某条命令执行失败时，本意图已应用的命令**逆序
    ///    回退**，影子回到意图之前的状态，**事务本身保留**（AI 改参数可以接着攒）。
    ///    这与 `tx_apply` 的「一步失败即整笔作废」并不矛盾：那边一步只有一条命令，
    ///    半截命令 = 不可解释状态，只能整笔作废；这边半截**意图**才是需要清掉的东西。
    ///
    /// 唯一例外：回退不精确（某条 `undo` 没能还原影子）时影子已不可信，整笔事务作废并
    /// 在错误里说明——**主场景始终一个字节都没动过**，作废只是让 AI 重开一笔。
    ///
    /// 空意图（展开出 0 条命令）直接拒：它要么是参数写错了，要么是刷了个空区域，
    /// 攒进事务只会让「提交了几条」和「AI 以为改了什么」对不上。
    pub fn tx_apply_intent(
        &mut self,
        id: &str,
        intent: impl Into<String>,
        build: impl FnOnce(&Scene) -> Result<Vec<CommandKind>, DocError>,
    ) -> Result<TxIntentStaged, DocError> {
        let session = self
            .tx
            .as_mut()
            .ok_or_else(|| DocError::TxNotFound(format!("没有未决事务，`{id}` 无从应用意图")))?;
        if session.id != id {
            return Err(DocError::TxNotFound(format!(
                "未决事务是 `{}`，不是 `{id}`",
                session.id
            )));
        }
        let intent = intent.into();
        // 构造失败（比如目标节点不存在）：影子还没被碰过，事务照旧。
        let cmds = build(&session.shadow)?;
        if cmds.is_empty() {
            return Err(DocError::Rejected(format!(
                "意图 `{intent}` 没有展开出任何命令（参数写错，或作用范围里没有可改的东西）"
            )));
        }

        let mut applied = 0;
        let mut failure = None;
        for cmd in &cmds {
            match cmd.apply(&mut session.shadow) {
                Ok(()) => applied += 1,
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }
        if let Some(err) = failure {
            // 逆序回退本意图已应用的部分。
            //
            // **不能拿哈希判「回退干净」**：`SpawnNode` 的 undo 只按名删节点，`next_id`
            // 不回退（撤销不等于时空倒流，`state_hash` 契约明确要覆盖 `next_id`），
            // 所以含 spawn 的意图回退后哈希必然与之前不同。这里只认 `undo` 自己的返回。
            let mut undone = 0;
            for cmd in cmds[..applied].iter().rev() {
                if cmd.undo(&mut session.shadow).is_ok() {
                    undone += 1;
                }
            }
            if undone != applied {
                // 回退不精确：影子已不可信，整笔作废（主场景未受影响）。
                self.tx = None;
                return Err(DocError::Rejected(format!(
                    "意图 `{intent}` 执行中断且回退不精确（已应用 {applied} 条，仅 {undone} 条\
                     还原成功），事务已作废；主场景未受影响，请重开一笔"
                )));
            }
            return Err(err);
        }

        let commands = cmds.len();
        session.staged.push(TxStep {
            name: intent.clone(),
            intent: true,
            cmds,
        });
        Ok(TxIntentStaged {
            intent,
            commands,
            staged: session.len(),
            intents: session.steps(),
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
            staged: session.len(),
            base: session.base,
            current: self.state_hash(),
            shadow: session.shadow_hash(),
            intents: session
                .staged
                .iter()
                .map(|step| (step.name().to_string(), step.len()))
                .collect(),
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
                open.len()
            )));
        }

        let session = self.tx.take().expect("上面刚确认过存在");
        if session.is_empty() {
            return Ok(TxCommit {
                committed: 0,
                hash: current,
                version: self.version,
            });
        }
        // 摊平成命令序列重放，但**保留每一步的边界**：入撤销栈时按步分组，
        // 因此「一条意图 = 一格撤销」在文档层兑现（见 [`TxStep`]）。
        let steps: Vec<Vec<CommandKind>> = session.staged.into_iter().map(|s| s.cmds).collect();
        let total: usize = steps.iter().map(Vec::len).sum();

        let mut applied = 0;
        let mut failure: Option<DocError> = None;
        'replay: for cmds in &steps {
            for cmd in cmds {
                match cmd.apply(&mut self.scene) {
                    Ok(()) => applied += 1,
                    Err(err) => {
                        failure = Some(err);
                        break 'replay;
                    }
                }
            }
        }
        if let Some(err) = failure {
            // 逆序回退：从最后一步的最后一条命令往回走，只撤 `applied` 条。
            let mut undone = 0;
            let mut remaining = applied;
            'undo: for cmds in steps.iter().rev() {
                for cmd in cmds.iter().rev() {
                    if remaining == 0 {
                        break 'undo;
                    }
                    if cmd.undo(&mut self.scene).is_ok() {
                        undone += 1;
                    }
                    remaining -= 1;
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

        // 整笔事务 = 撤销栈一格（S1 契约）：摊平成一条命令序列再入栈。
        //
        // 步边界只服务于「记账可读」（`TxStep` / `tx_preview` 的 `intents` 清单），
        // **不插手撤销粒度**——一笔事务就是一次 `undo` 全退，否则「提交了 3 条命令
        // 却要按 3 次」既反直觉，也让 S3 的意图（本来就是整笔的一部分）白拿一层格。
        let commands: Vec<CommandKind> = steps.into_iter().flatten().collect();
        self.undo.push_group(commands);
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
            dropped: session.len(),
            hash: self.state_hash(),
            version: self.version,
        })
    }
}
