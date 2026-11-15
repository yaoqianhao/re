---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_21981446e0f411f18c5752540024e231
    ReservedCode1: wHH2rGRW9ZA+mmc9xdJMNAAks3p6ibykNGy30y4/FWedQFxfu5wOgEiVah/bgxFzmqrIcmaLYAPddDKl5IWjGRLu73VfgJuMY+M+XQBNks39m6yLIPj50X9av934IAmTjx4TEMKNgGEXO6T6SafreJ9NRfs8+somCzqZbq0K8SD3X4eyi9zWEbji2uY=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_21981446e0f411f18c5752540024e231
    ReservedCode2: wHH2rGRW9ZA+mmc9xdJMNAAks3p6ibykNGy30y4/FWedQFxfu5wOgEiVah/bgxFzmqrIcmaLYAPddDKl5IWjGRLu73VfgJuMY+M+XQBNks39m6yLIPj50X9av934IAmTjx4TEMKNgGEXO6T6SafreJ9NRfs8+somCzqZbq0K8SD3X4eyi9zWEbji2uY=
---

# M0-S4：Transaction / Undo / StateHash + headless 600 帧确定性（方案）

> 状态：**方案期，未落地**。基线 `727872e`（M0-S3 全包已闭），工作区干净。
> 本文所有行数、测试数、门禁项均于 2026-11-15 方案期实测（命令见 §1.5），
> 执行期若与代码不符，以代码为准并在此处留「落地偏差」章节，**禁止**执行期顺手改数字。

---

## 0. 摘要（一屏）

S3 把「谁是场景的唯一真值、谁能写」锁死了；S4 锁「**写操作能不能整体回退、能不能被机器验证**」。

四件事，按依赖排序：

1. **`StateHash`**（S4-03）：`r3e-doc/src/state_hash.rs`，FNV-1a 64 手写规范化遍历，**全仓唯一一份**；顺带把 `r3e-runtime` 里那份 FNV 收编，`document.version` 作为哈希缓存节拍器。
2. **`Transaction`**（S4-01/02）：`begin / preview / apply / commit / rollback`；`preview` 走 `Scene` 真快照（克隆体上预演，主文档零接触），原子性靠「失败逆序 undo + 哈希断言」。
3. **`UndoStack`**（S4-04）：把现有 `Vec<CommandKind>` 升级为带事务标记的栈，支持 `rollback_to(mark)`；性质测试 N=100 随机序列全量回退哈希一致。
4. **headless 回放**（S4-05/06）：新增 `crates/r3e-replay`（只依赖 core + doc，零 GPU 零音频），`--seed 42 --frames 600 --hash-every 60`；`cargo xtask replay` 跑两次逐行比对，接进 `verify`。

退出演示：`cargo xtask verify` 中出现 `[replay] 600 帧 / 11 个采样点 / 两次运行哈希完全一致`，全程不开 GPU、不建音频设备。

**红线**：`StateHash` 若出现第二份实现（当前 FNV 已实测散在 3 处，见 §1.3），视为 S4 失败。

---

## 1. 实测基线（先纠正路线文档口径）

### 1.1 路线文档的说法 vs 代码现实

| # | `AI_ROADMAP_EXECUTION.md` 的说法 | 方案期实测 | 处理 |
|---|---|---|---|
| 1 | 「S4 只需在 `write` 外层套前像栈」 | `CommandKind` 14 条**本身就是前像载体**（`old`/`new` 成对）；`Document.undo_stack: Vec<CommandKind>` 与 `undo()` 已存在（S3-P3 落地） | S4 不是「套壳」，是补**事务边界、原子回退、快照预演、状态哈希**四件 S3 未做的事 |
| 2 | 「对 `scene` 做稳定序列化后哈希（FNV/blake3 择一）」 | `Scene.nodes` 是 `HashMap<NodeId, Node>`，序列化顺序靠并行数组 `order: Vec<NodeId>`；但 `to_json_string()` 走 `to_scene_file()`，**丢 `next_id` / `time` / `frame` / `anim_clips` / `animations` / `skins` / `morphs`** | 不能把 JSON 串直接当状态哈希：`next_id` 不同会导致后续 `spawn` 的 id 分化，回放会假绿。改为手写遍历（§2 决定 1） |
| 3 | 「`StateHash` 只留一份」 | 实测 FNV 实现**散在 3 处**：`r3e-core/src/air.rs:606`、`r3e-plugin/src/tools.rs:1055,1073`、`r3e-runtime/src/app.rs:172,175` | `runtime` 那份**收编**进 `r3e-doc::state_hash`；`air`/`plugin` 属域内摘要（资产 digest / 工具表哈希），划清边界不动（§10） |
| 4 | 「`cargo xtask replay` 接进 `verify`」 | `xtask` 现有 **5** 个子命令（`verify`/`lint-size`/`check-tests`/`dep-check`/`doc-gate`）；`run_cargo` 用 `Stdio::inherit()`，**不捕获子进程输出** | 新增捕获式执行路径：先 `cargo build`，再直接跑 exe 并 `output()` 收两次 stdout 做逐行 diff（避免嵌套 cargo 抢 target 锁，§9 R6） |
| 5 | 「全程不开 GPU」 | `r3e-runtime` 的 `run_headless` 确实不建窗口/GPU，但会构造 `Audio::open_default()`、`ScriptHost`、物理场景、场景栈 | 回放不复用 runtime，新建只依赖 core+doc 的 `r3e-replay`：更快、无设备依赖、CI 上不会因无声卡抖动 |
| 6 | S4 卡「`Command` v0 先 10 条高频」 | 实落 **14 条**（S3 卡片已同步实况） | 本方案按 14 条计覆盖面 |

### 1.2 现有文档层结构（S3 交付物，实测行数）

| 文件 | 行数 | 职责 |
|---|---:|---|
| `crates/r3e-doc/src/lib.rs` | 285 | `Document` 定义、`DocError`、`Deref`、`parse_scene` |
| `crates/r3e-doc/src/write.rs` | 80 | `version`/`dirty`/`writes` 记账、`write()`、`replace_scene()`、`touch()` |
| `crates/r3e-doc/src/command/mod.rs` | 313 | `Command` trait、`CommandKind` 14 条 apply/undo、`Document::apply/undo`/`undo_stack` |
| `crates/r3e-doc/src/command/env.rs` | 92 | 摄像机/环境/后处理/局部光的写入原语 |
| `crates/r3e-doc/src/command/scene.rs` | 115 | 变换/可见/颜色/材质写入原语 |
| `crates/r3e-doc/src/command/ui.rs` | 83 | UI / 文本写入原语 |
| `crates/r3e-doc/src/validate/{mod,bounds,refs}.rs` | 113 / 218 / 131 | Validator v0 三类检查 |
| **合计** | **1430** | 依赖仅 `r3e-core`（口径：`splitlines()` 行数，与 `read_text` 的 offset 口径差 1 行，勿混用） |

`Document` 现有字段：`scene: Scene`、`version: u64`、`dirty: bool`、`writes: u64`、`undo_stack: Vec<CommandKind>`。
现有公开 API：`new/from_json_str/validate/as_scene/version/is_dirty/mark_clean/write_count/write/replace_scene/touch(pub(crate))/apply/undo/undo_depth/clear_undo/undo_label`。

### 1.3 `Scene` 结构（决定哈希覆盖面与克隆成本）

`Scene` 共 25 个字段，关键点三条：

- **存储**：`nodes: HashMap<NodeId, Node>` + `order: Vec<NodeId>` + `name_index: HashMap<String, NodeId>` + `next_id: NodeId`。**遍历顺序由 `order` 给定，稳定**，哈希无需排序，也不碰 HashMap 迭代顺序。
- **克隆**：`Scene` 当前 `#[derive(Debug)]`，`Node` 是 `#[derive(Debug, Clone)]`。方案期做了一次探针：`Scene` 改 `#[derive(Debug, Clone)]` 后 `cargo check -p r3e-core` → `Finished dev profile … in 17.59s`，零错误，随后 `git checkout` 还原（工作区已确认干净）。**结论：`Scene` 可克隆，preview 走真快照可行。**
- **序列化**：`to_json_string()` 覆盖 name/background/camera/light/environment/postfx/lights/occlusion/environment_map/objects（按 `order`）/physics/assets/texts/ui；不覆盖 `next_id`/`time`/`frame`/`anim_clips`/`animations`/`skins`/`morphs`。

### 1.4 门禁与测试现状

| 项 | 实测值 |
|---|---|
| 测试基线 `xtask/tests_baseline.json` | total **1427**（core 576 / render 307 / script 132 / editor 101 / **doc 63** / asset 61 / editor-host 58 / plugin 47 / audio 29 / physics 28 / runtime 14 / app 11） |
| `xtask/size_debt.json` | `limit = 800`，登记 24 个文件（`r3e-runtime/src/app.rs` 3066、`r3e-core/src/scene.rs` 4052 …） |
| `xtask/doc_gate.json` | `rule4_scope` 3 项、`rule6_scene_keys` 5 项、`rule2_exceptions` 10 项 |
| `cargo xtask verify` 组成 | `fmt --all -- --check` → `clippy --workspace --all-targets -- -D warnings` → `test --workspace` → `lint-size` → `check-tests` → `doc-gate`（**无 replay**） |
| workspace members | 12 个 crate + `xtask`（无 `r3e-replay`） |
| `dep_rules.json` | `forbid_engine_all` 12 项；`crates` 登记 12 项；未登记即失败（规则 5） |
| 随机源 | 全仓无 `rand` 依赖，无 `seed` 运行时概念（`seed_2d/seed_3d/seed_corners` 只是测试辅助函数名） |

### 1.5 本次实测所用命令（可复跑）

```powershell
git log --oneline -3                       # HEAD = 727872e
git status --porcelain                     # 空
Select-String docs\AI_ROADMAP_EXECUTION.md -Pattern "S4"   # 取 S4 卡片原文（272–289 行）
# 行数：python 遍历 crates/r3e-doc/src/**/*.rs 计 \n
# 测试数：python 正则 ^\s*#\[(?:tokio::)?test\] 全仓计数 = 1427
Select-String crates\r3e-runtime\Cargo.toml -Pattern 'r3e-'        # runtime 已依赖 r3e-doc
Select-String xtask\src\main.rs -Pattern 'fn run_cargo' -Context 0,6  # Stdio::inherit()
# FNV 散落：grep 'FNV_OFFSET|fnv_mix|fnv1a|0xcbf2_9ce4' → air.rs / plugin/tools.rs / runtime/app.rs
# 克隆探针：scene.rs derive(Debug → Debug, Clone) → cargo check -p r3e-core → Finished 17.59s → git checkout
```

---

## 2. 核心设计：四个决定

### 决定 1：`StateHash` 落 `r3e-doc`，FNV-1a 64，手写规范化遍历

**归属**：`crates/r3e-doc/src/state_hash.rs`（新文件）。理由：`r3e-doc` 独占 `Scene`，哈希必须贴着真值；上层（runtime / replay / M1 CLI）只能调 API，不能自己算。

**算法**：FNV-1a 64。
- 不引 blake3：`r3e-doc` 目前依赖只有 `r3e-core`，加密码学依赖换不来收益——哈希在这里只做「同 / 不同」的二元判定（600 帧约 11 个采样点），不是防碰撞场景。
- 与既有实现同源，便于读者对照；但**全仓只允许这一份**：`state_hash::mix_u64` 是唯一混合原语，`r3e-runtime` 的 `FNV_OFFSET`/`fnv_mix` 删除。

**规范化遍历**（顺序即契约）：

```
Scene::order（Vec<NodeId>，插入序，稳定）
  └── 每个 node: id → name → transform(9×f32) → visible → color → material(有无 + 名)
      → parent / children（children 本身是 Vec，按序）→ mesh 存在性
再按固定顺序吃场景级字段（见下表）
```

Hash 输入一律走位表示（`f32::to_bits`、字符串按字节），避免浮点格式化差异。

**覆盖 / 不覆盖表**（写进代码注释，评审对照）：

| 覆盖 | 理由 | 不覆盖 | 理由 |
|---|---|---|---|
| `order` + 每节点 id/name/transform/visible/color/material/parent/children/mesh | 命令 14 条全部落在这几项上 | `time` / `frame` | 属「世界节拍」，另给 `world_hash()`；`state_hash()` 保持纯内容语义 |
| `name` / `background` / `camera` / `light` | 命令 `SetCamera` 直接改 | `anim_clips`（`HashMap<String, Arc<AnimClip>>`） | 运行期资产引用，非文档语义；若纳入，哈希会随资产导入顺序漂移（且是 HashMap） |
| `environment` / `postfx` / `lights` / `texts` / `ui` / `text_enabled` / `ui_enabled` / `ui_above_text` | 命令 `SetEnvironment`/`SetPostFx`/`Upsert*`/`Remove*` 直接改 | `animations` 播放进度 / `skins` 关节求解结果 | 每帧求值产物，进哈希等于给渲染层开门 |
| `next_id` | **必须覆盖**：不回退它，回放里同一脚本会分配出不同 NodeId，哈希假绿 | `physics` 的求解缓存 | 同上 |
| `occlusion` / `environment_map` / `physics`（配置）/ `assets` / `morphs` 权重 | 文档语义的一部分，成本低 | | |

**API**：

```rust
pub struct StateHash(u64);              // Display 输出 16 位小写十六进制
pub fn mix_u64(h: &mut u64, v: u64);
pub fn mix_f32(h: &mut u64, v: f32);    // 内部走 to_bits
pub fn mix_str(h: &mut u64, s: &str);
pub fn scene_hash(scene: &Scene) -> StateHash;
impl Document {
    pub fn state_hash(&self) -> StateHash;   // 纯内容，不触 version
    pub fn world_hash(&self) -> StateHash;   // 内容 + time + frame
}
```

**缓存**：`Document` 增 `hash_cache: Option<(u64 /*version*/, StateHash)>`。`version` 是节拍器（S3 已按此设计要求「宁可多跳一格，不可漏记」），版本没变直接返回缓存。600 帧 × 11 次采样下这层缓存不是必需，但它让「每帧算哈希」在 M1 也敢用。

### 决定 2：`preview` 用真快照（`Scene: Clone`），不用「apply 后立即 undo」

S4-01 的验收是「`preview` 前零副作用（用状态哈希对比证明）」。两条路：

| 路线 | 做法 | 问题 |
|---|---|---|
| A 可逆试算 | 在主文档上真 apply，再立即逆序 undo，比较哈希 | 中途状态对任何观察者可见（脚本回调 / 编辑器桥），且正确性依赖「每条 undo 都精确」，preview 反而成了 undo 的测试用例 |
| **B 真快照（采纳）** | `Scene` 克隆一份影子，命令在影子上跑，算哈希后丢弃 | 需要 `Scene: Clone`——**方案期已实测可行**（§1.3） |

采用 B：零副作用由**结构**保证（主文档根本没被碰），哈希断言退化为兜底。

配套两个 `pub(crate)` 通道，仅在 `r3e-doc` 内部使用，不扩大可变借用面（`doc-gate` 规则 2/3 不受影响，因为文档层对本 crate 豁免且不出现 `DerefMut`）：

```rust
impl Document {
    pub(crate) fn scene_snapshot(&self) -> Scene;     // 克隆真值
    pub(crate) fn scene_mut_raw(&mut self) -> &mut Scene;  // 事务内部通道，不记账
}
```

`scene_mut_raw` 是关键取舍：`write()` 会 `touch()`（推进版本、置脏），而**事务的 rollback 与 preview 都不该推进版本**——preview 在主文档上完全不写；rollback 属于「撤销这次失败」，推进版本会让缓存与「版本=状态」的直觉脱钩。这个通道只在 `transaction.rs` / `undo.rs` 里出现，`doc-gate` 规则 6 管的是「`write` 闭包体内再借场景载体」，不冲突。

### 决定 3：事务与既有单命令路径并存，`apply` 语义不变

保留 `Document::apply(&dyn Command)` 原语义（成功即 `touch()`，版本 +1）。理由：14 条命令的 28 个既有测试、runtime / editor-host 的调用点都压在这条路径上，S4 没有理由顺手改语义。1427 的基线要能原样通过。

新增显式事务：

```rust
pub struct Transaction<'a> { /* doc: &'a mut Document, base: StateHash, staged: Vec<CommandKind> */ }

impl Document {
    pub fn begin(&mut self) -> Transaction<'_>;        // 冻结 base = state_hash()
}
impl<'a> Transaction<'a> {
    pub fn apply(&mut self, cmd: CommandKind) -> Result<(), DocError>;   // 逐条执行，失败即整体回滚
    pub fn preview(&self, cmds: &[CommandKind]) -> Result<StateHash, DocError>;  // 影子预演，返回执行后哈希
    pub fn staged(&self) -> &[CommandKind];
    pub fn commit(self) -> Result<StateHash, DocError>;  // 版本 +1（整笔一次），undo_stack 追加 n 条并打 tx 标记
    pub fn rollback(self) -> Result<StateHash, DocError>; // 逆序 undo，版本不推进，返回回退后哈希
}
```

语义约定：

1. **原子性**：`apply` 第 k 条失败 → 逆序 undo 前 k-1 条 → `base` 对比；回退本身再失败（说明某条命令 undo 不精确）→ 返回新增变体 `DocError::RollbackFailed { undone: usize, total: usize }`，并把文档标脏（状态已不可信，禁止静默）。
2. **版本推进粒度**：一笔事务 `commit` 只 +1，不是每条命令 +1。这样「一次意图 = N 条原子命令」（M1-S3）在版本号上也是一件事。
3. **undo 粒度**：`commit` 后 `document.undo()` 一次退整笔（栈里 n 条共享同一 tx 标记）；`undo_depth()` 语义改为「可撤销的事务笔数」，既有单命令路径 = 每笔 1 条，行为不变。
4. **嵌套**：禁止（`Transaction` 持有 `&mut Document`，类型系统已挡）。

### 决定 4：回放独立成 `crates/r3e-replay`，不进 `r3e-runtime`

| 方案 | 优点 | 缺点 |
|---|---|---|
| 复用 `r3e-runtime --headless-frames 600` | 零新增 crate | 会构造 `Audio::open_default()`、`ScriptHost`、物理场景、场景栈；依赖 render/audio/physics 全套，构建慢、CI 与真机设备耦合；输出用 `log::info!`（受日志级别影响） |
| **新建 `r3e-replay`（采纳）** | 依赖只有 `r3e-core` + `r3e-doc`；600 帧 2 次 < 2 秒；输出直接 `println!`，格式即契约 | 多一个 crate 要登记 `dep_rules` / `workspace` / `tests_baseline` |

`r3e-replay` 形态：`lib`（回放引擎，可被 M1-S5 的 `r3e-cli replay` 直接复用）+ `bin`（`r3e-replay`，参数解析 + 打印）。

`r3e-runtime` 侧同步做两件事（不新增功能，只收编 FNV）：`HeadlessReport.fingerprint` 改调 `r3e_doc::state_hash::mix_u64`，混入内容不变（`frame_index` / `time` / `scene_nodes` / `camera` × 3），既有 runtime 14 个测试的行为与输出格式保持不变。

---

## 3. 确定性回放规格（S4-05）

**参数**：`--seed <u64>`（默认 42）、`--frames <u64>`（默认 600）、`--hash-every <u64>`（默认 60）、`--scene <path>`（可选，默认空场景）。

**步长**：`dt = 1/60`（与 `HEADLESS_FIXED_DT` 同值，回放侧独立常量并在文档互相引用，避免跨 crate 依赖）。

**每帧**：

1. `doc.write(|s| { s.time += dt; s.frame += 1; })`——世界节拍推进，走 S3 唯一写入口；
2. 按 LCG 派生动作：前 60 帧预热（每帧 `SpawnNode`，名字 `n{seq:03}`）；之后每帧从上一步到场的节点里选一个，随机命中 `SetTransform` / `SetVisible` / `SetNodeColor` 之一；
3. 每 10 帧把本批 5 条命令组一笔 `Transaction`（`begin → 5×apply → commit`），**事务路径也进回放覆盖面**；
4. 每 `hash-every` 帧打印一行。

**LCG**：`state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)`（PCG 常数，64 位），取 `state >> 33` 作选择位。不引 `rand`。

**输出契约**（稳定，供 `xtask` 逐行比对，也给 M1-S5 CLI 复用）：

```
[replay] seed=42 frames=600 hash_every=60 dt=0.016667
frame 0060 world=0f3a... state=7c11... nodes=61 depth=6
frame 0120 world=...       state=...    nodes=61 depth=6
...
[replay] done frames=600 commits=60 undo_depth=60 final=...
```

节点/深度字段进输出是有意的：只比哈希时，若两次运行「错得一样」也会绿；带上可读量，失败时一眼能定位到哪一帧开始分叉。

**判定**（`xtask replay`）：

1. `cargo build -p r3e-replay --bin r3e-replay` 一次；
2. 用同一组参数跑**两次**，`Command::output()` 捕获 stdout；
3. 逐行比对，第一个不一致行打印两次内容 + 帧号，返回非 0；
4. 成功打印 `[replay] 600 帧 / 11 个采样点 / 两次运行哈希完全一致（final=...）`。

---

## 4. 目标文件树与行数预算

```
crates/r3e-doc/src/
├── lib.rs               285 → ~300   （+ StateHash 缓存字段、+ begin()、+ 变体）
├── write.rs              80 → ~95    （+ scene_mut_raw、+ hash 缓存失效）
├── state_hash.rs         —  → ~280   ★新：唯一哈希实现（FNV + 规范化遍历 + 覆盖表注释）
├── transaction.rs        —  → ~260   ★新：Transaction / preview / 原子回退
├── undo.rs               —  → ~150   ★新：UndoStack（tx 标记 / rollback_to / depth）
├── command/mod.rs       313 → ~290   （undo_stack 迁出到 undo.rs，Document::apply/undo 改为委托）
└── command/{env,scene,ui}.rs / validate/*   不动

crates/r3e-replay/
├── Cargo.toml                     ★新（只依赖 r3e-core / r3e-doc）
└── src/
    ├── lib.rs            —  → ~220   ★新：ReplayConfig / Lcg / run() / ReplayReport
    └── main.rs           —  → ~90    ★新：参数解析 + 打印

xtask/src/main.rs         450 → ~520  （+ replay() 捕获式执行 + verify 接入）
xtask/dep_rules.json               ★改：+ r3e-replay 登记
xtask/tests_baseline.json          ★改：+ 新测数
Cargo.toml                         ★改：members + workspace.dependencies
docs/M0S4_TRANSACTION.md           ★本文
```

全部 ≤ 800，`lint-size` 不需要新增债务登记。

---

## 5. 门禁与依赖规则变更（必须显式记账）

| 文件 | 变更 | 原因 |
|---|---|---|
| `Cargo.toml` | `members += "crates/r3e-replay"`；`workspace.dependencies += r3e-replay` | 新 crate |
| `xtask/dep_rules.json` | `forbid_engine_all += "r3e-replay"`；`crates["r3e-replay"] = { allow: ["r3e-core","r3e-doc"], note: "回放器只允许依赖真值与哈希，不得引入 render/audio/physics" }` | `doc-gate` 规则 5：未登记即失败 |
| `xtask/tests_baseline.json` | `r3e-doc` 63 → 63+n，`r3e-replay` 0 → m，`total` 1427 → 1427+n+m（落地后按 `cargo test` 实测回填） | `check-tests` 只许增不许减 |
| `xtask/src/main.rs` | `"replay" => replay()`；`verify()` 在 `test` 之后、`lint-size` 之前插入 `if replay() != 0 { failed.push("replay".into()) }` | S4-06 |
| `xtask/doc_gate.json` | **不改** | 新代码全部落在 `r3e-doc`（规则 1/2/3 豁免区）与 `r3e-replay`（规则 4 scope 之外）；不新增 `&mut Scene` 公开签名 |

`verify` 耗时预算：`replay` 新增 ≤ 8 秒（build 一次 + 2×600 帧），`verify` 总时长仍 < 120 秒（S5-06 要求）。

---

## 6. 工作包与分批

| 包 | 内容 | 交付判定 | 预估 |
|---|---|---|---|
| **P0** | 本文 + 路线图 S4 卡片「前置条件」补「`Scene: Clone` 已探针可行」 | 文档入库 | 0.5 天 |
| **P1** | `state_hash.rs` + `Document::{state_hash,world_hash}` + 哈希缓存；`runtime` 侧 FNV 收编 | 同状态两次哈希相同；改 13 组字段任一处哈希必变；`cargo test -p r3e-runtime` 全绿（输出格式不变） | 1–1.5 天 |
| **P2** | `transaction.rs`：`begin/preview/apply/commit/rollback` + `DocError::RollbackFailed` | `preview` 前后 `state_hash()` 相同；故障注入（第 3 条失败）后哈希 == 初始；`commit` 只推 1 次版本 | 2 天 |
| **P3** | `undo.rs`：`UndoStack` 带 tx 标记 + `rollback_to(mark)`；性质测试 N=100 | 随机序列 apply→undo 到空，哈希 == 初始；一笔事务单次 undo 全退 | 1.5 天 |
| **P4** | `r3e-replay` crate + `xtask replay` + `verify` 接入 + 3 个 JSON 变更 | `cargo xtask replay --seed 42 --frames 600` 两次同哈希；`cargo xtask verify` 全绿 | 2 天 |

依赖顺序：P1 → P2 → P3 → P4（P3 的 `rollback_to` 同时被 P2 内部复用，允许 P2/P3 并行起草、P3 先冻结 `UndoStack` 接口）。

---

## 7. 风险与熔断

| # | 风险 | 观察信号 | 熔断 / 处置 |
|---|---|---|---|
| R1 | `Scene::Clone` 深拷贝开销进了热路径 | 回放 600 帧耗时 > 5 秒 | 只允许 `preview` 与 `rollback` 用快照；若 preview 被上层高频调用，退化为「每笔事务一次快照 + 复用」并在 §10 记债 |
| R2 | 某条命令 `undo` 不精确（历史高危型：`SetNodeMaterial` 的 `old = None` 必须真删材质） | 性质测试出现哈希不等 | 逐命令补「apply → undo → 哈希一致」断言（14 条各 1 测）；修不了就把该命令降级为「不可进事务」并在文档点名 |
| R3 | `StateHash` 覆盖面漂移（新增 `Scene` 字段后忘了纳入） | 新字段改了哈希不变 | 一条守卫测试：对 13 组字段各造一次扰动，断言哈希全变；`state_hash.rs` 顶部维护覆盖表注释，评审对照 |
| R4 | `Scene` 加 `Clone` 与 `--all-features` 组合出问题（rapier 特性） | `cargo check --workspace --all-features` 报错 | P1 落地后立刻跑全量 check；探针只跑了 `-p r3e-core` 默认特性 |
| R5 | 回放里 `HashMap` 影响确定性 | 两次运行哈希不等 | `anim_clips` / `name_index` **不进哈希**（§2 决定 1 表）；若将来必须纳入，按 key 排序后混入 |
| R6 | `xtask replay` 在 `cargo` 里再跑 `cargo run` → target 锁竞争 | `Blocking waiting for file lock` | 先 `cargo build -p r3e-replay`，再直接执行 `target/<triple>/debug/r3e-replay`（可执行路径由 `cargo metadata` 或约定 triple 解析） |
| R7 | 浮点跨平台差异 | 换机器哈希不同 | S4 只保证「同机同二进制两次运行一致」；跨平台确定性推 M2 并在 §10 声明 |

---

## 8. 不做（明确排除）

1. **redo / 重做栈**：S4 卡片未要求，M1 协议也没提；现在加会顺带定义「重新提交的事务 id 语义」，等 `r3e-protocol` 定型。
2. **字段私有化**（`Scene` 字段可见性收口）：M1-DOC-FIELDS，S3 决定 2 已推后。
3. **`air.rs` / `plugin/tools.rs` 的 FNV 收编**：那是资产 digest 与工具表哈希，与 `StateHash` 不同域；强行统一会把 `r3e-doc` 变成通用哈希库。本方案只在文档点名三处并声明「`StateHash` 只有一份」。
4. **场景差分（diff / patch）**：S4-01 只说 `preview` 零副作用，M1-S1 才要结构化 diff。
5. **跨平台位级确定性**：见 R7。
6. **`verify` 并行化**：当前串行已够；replay 只加 8 秒。
7. **回放接入脚本 / 输入录放**：`r3e-script` 参与会引入资产与运行时状态，属 M1-S5 的 `r3e-cli replay`。

---

## 9. 验收清单（全部可自动判定）

| # | 对应卡 | 验收项 | 判定方式 |
|---|---|---|---|
| 1 | S4-03 | 同状态两次哈希相同 | `state_hash_of_same_scene_is_stable` |
| 2 | S4-03 | 不同状态必不相同（13 组字段逐组扰动） | `every_covered_field_changes_hash` |
| 3 | S4-03 | 全仓 `StateHash` 实现唯一 | `cargo xtask doc-gate` + 人工 grep 三处 FNV 边界说明 |
| 4 | S4-01 | `preview` 前后主文档哈希不变 | `preview_leaves_document_untouched` |
| 5 | S4-01 | `preview` 返回值 == 真 apply 后的哈希 | `preview_matches_real_apply` |
| 6 | S4-02 | 故障注入：第 k 条失败 → 状态 == 初始 | `failed_transaction_rolls_back_to_base`（k=1,3,n 各一测） |
| 7 | S4-02 | 回退失败上报 `RollbackFailed`（不静默） | `rollback_failure_is_reported` |
| 8 | S4-04 | 随机序列 N=100 全量回退哈希一致 | `random_sequence_undo_to_empty_restores_hash` |
| 9 | S4-04 | 一笔事务单次 undo 全退 | `one_undo_reverts_whole_transaction` |
| 10 | S4-05 | 600 帧两次运行每 60 帧哈希一致 | `cargo xtask replay --seed 42 --frames 600` |
| 11 | S4-06 | `verify` 含回放且输出状态哈希行 | `cargo xtask verify`（日志出现 `[replay]`） |
| 12 | 全局 | 1427 基线不退化 + 新增测数入基线 | `cargo xtask check-tests` |
| 13 | 全局 | 质量门零告警 | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` |
| 14 | 全局 | 全特性通过（含 rapier） | `cargo test --workspace --all-features` |

---

## 10. 提交规划（本地提交，**不推送**）

| # | 提交 | 内容 |
|---|---|---|
| 1 | `feat(doc): StateHash 唯一实现 + 哈希缓存` | P1（含 runtime FNV 收编、13 组字段守卫测试） |
| 2 | `feat(doc): Transaction 快照预演与原子回退` | P2（含 `RollbackFailed` 变体） |
| 3 | `feat(doc): UndoStack 事务标记与全量回退` | P3（性质测试 N=100） |
| 4 | `feat(replay): headless 确定性回放 + xtask replay 接入 verify` | P4（新 crate、3 个 JSON、verify 顺序） |
| 5 | `docs: M0-S4 方案落地偏差与文档同频` | `ARCHITECTURE.md` / `README.md` / `AI_ROADMAP_EXECUTION.md` 同频，实测值回填 |

---

## 11. 文档同频清单（S4 收尾时一并刷新，以当时实测为准）

1. `docs/AI_ROADMAP_EXECUTION.md`：S4 卡片 S4-01…S4-06 打勾；看板测试数刷到落地实测值；「前置条件」补 S3 交接实况（`Scene: Clone`、`undo_stack` 已在）。
2. `docs/ARCHITECTURE.md`：§「文档层」补 `state_hash` / `transaction` / `undo` 三模块与职责边界；规模表加 `r3e-doc` 新行数与 `r3e-replay` 一行；「运行与验证」表加 `cargo xtask replay --seed 42 --frames 600`。
3. `README.md`：边界段补一句「文档层负责事务与状态哈希，回放不依赖渲染 / 音频」；新增一节说明 600 帧确定性回放怎么跑、输出怎么读。
4. `xtask/tests_baseline.json` / `size_debt.json`：落地实测回填（只增不减、只降不升）。
5. `docs/M0S3_DOC_LAYER.md` §14：把「S4 的接口已预留」改为「已由 M0S4_TRANSACTION.md 承接」并给链接。

---

## 12. S4 之后（为 S5 / M1 预留）

- `Transaction` 的 `preview` 返回值要在 M1-S1 升级成**结构化 diff**（受影响对象 / 材质清单），因此 `preview` 现在返回 `StateHash` 是刻意留的窄口子——M1 换成 `PreviewReport { hash, diff }` 时不破坏 `apply/commit/rollback`。
- `UndoStack` 的 tx 标记直接给 M1-S3「一次意图 = N 条原子命令，undo 一次全退」用，S4 已把粒度做对。
- `r3e-replay` 的 `ReplayConfig` / `run()` 直接给 M1-S5 的 `r3e-cli replay` 复用，S4 只做「同机两次一致」，M1 再加「跨版本对照」。
- `state_hash` 是 S5「invariants」与 M1「对外 dry-run 返回哈希」的唯一哈希入口，届时只允许调用，不允许再写第二份。
*（内容由AI生成，仅供参考）*
