---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_648bb9a1e03311f18c5752540024e231
    ReservedCode1: elvE1S7qgdtjlThbPQglF5X0/bDrGE069gC+q3c3ReW3q4G1ZpmR6G3ZnZm9k5DBB4SYOTR+f4tFJDefYAK4ey5mKkr+2BhZYvnG4+M4lCG/AeJUir6Sk5BQ5A33a35EBjc3IYd//dZuyW8K/jk6l3Kb7xSurYLmiNPUCBqYree+AG2BViuwsxqSKaQ=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_648bb9a1e03311f18c5752540024e231
    ReservedCode2: elvE1S7qgdtjlThbPQglF5X0/bDrGE069gC+q3c3ReW3q4G1ZpmR6G3ZnZm9k5DBB4SYOTR+f4tFJDefYAK4ey5mKkr+2BhZYvnG4+M4lCG/AeJUir6Sk5BQ5A33a35EBjc3IYd//dZuyW8K/jk6l3Kb7xSurYLmiNPUCBqYree+AG2BViuwsxqSKaQ=
---



# rust3d-engine：以 AI 为核心的解耦开发路线图（路线 + 执行，合一）

- 制定日期：2026-11-14（判断 / 架构 / M0–M4 路线）
- 合并日期：2026-11-16（原 `AI_ROADMAP_EXECUTION.md` 全文并入本文，旧文件已删除，见附录 E）
- **本文档是路线与执行的唯一真源**：既回答"为什么这么走"（§一–§六），也回答"这周做什么、怎么算做完、做不完怎么办"（§三 Sprint 卡、§七–§九）。
- 使用方式：开工时只读 §零（当前位置）+ §三对应 Sprint 卡 + §八看板；每周五更新看板数字，**不改历史判断**。

---

## 零、当前所处位置（2026-11-16 实测，非文档口径）

**一句话**：M0 七个 Sprint **全部完成**（S0–S5，2026-11-16 收口），M0-1 ~ M0-9 九项交付物全部达成、**Gate 0 已过**；下一步是 M1（AI 写入闭环）。

### 0.1 Sprint 状态

| Sprint | 主题 | 状态 | 关键产出（实测） |
|---|---|---|---|
| M0-S0 | 收口 + 立尺 | ✅ 2026-11-14 | 双份代码消失、`r3e-app` 薄壳 30 行、`xtask` 五子命令、`size_debt.json` / `tests_baseline.json` |
| M0-S1 | `renderer.rs` 拆分 | ✅ 2026-11-15 | `renderer/` 17 文件，最大 716 行；`r3e-render` 307 测不减；像素基线未动 |
| M0-S2 | `api.rs` 双份收口 | ✅ 2026-11-15 | script 17 文件最大 428 / editor-host 13 文件最大 433；`core::cmd_spec` 命令表 **369 条**成单一真源 |
| M0-S3 | `r3e-doc` 立唯一真值 | ✅ 2026-11-15 | `Document` 只读 `Deref`、拒绝 `DerefMut`；写点收口 **129 处**；`doc-gate` 六规则；Validator 17 条负向用例 |
| M0-S4 | 事务 / Undo / 确定性 | ✅ 2026-11-15 | `Transaction` 198 行 / `StateHash` 604 行（FNV-1a64）/ `UndoStack` 168 行；新建 `r3e-replay` 546 行 11 测，600 帧两次同哈希 |
| M0-S5 | 协议剥离 + 能力清单 + 门禁转硬 | ✅ 2026-11-16 | `r3e-protocol` 8 文件 2,326 行仅 serde / `r3e-editor` 缩至 505 行传输层；`capabilities.json` 4,744 行 369 命令；`dep-check --strict` 硬门 0 违规 + 4 条债记账；`invariants` 三源 24/24/24；`verify` 20.4s 九道门；`AI_WORKFLOW.md` 落地 |

### 0.2 门禁现状（`cargo xtask verify`，2026-11-16 实测）

| 指标 | 现值（M0-S5 收口） | M0 目标 | 采集 |
|---|---|---|---|
| 最大文件行数 | 4057（`r3e-core/src/scene.rs`） | ≤800 | `cargo xtask lint-size` |
| `>800` 行文件数 | 23 | ≤12 | 同上（白名单 23 项，全部未上涨；协议层 8 文件最大 486 行） |
| `#[test]` 总数 | 1476 | 只增不减 | `cargo xtask check-tests` 快照比对 |
| 依赖违规数 | **0** | 0 | `cargo xtask dep-check --strict`（**已硬门**） |
| 依赖债务条数 | 4（`plugin→core` / `plugin→physics` / `script→physics` / `script→audio`，`limit=4` 只减不增） | 记账可查 | `xtask/dep_debt.json` |
| 能力清单漂移 | 0（`capabilities.json` 369 命令 / 39 RPC / 24 不变量，逐字节可复现） | 无漂移 | `cargo xtask gen-capabilities --check` |
| 不变量三源 | 24 = 24 = 24 | 三源一致 | `cargo xtask invariants` |
| `verify` 时长 | 20.4s（热缓存，九道门全硬；同机实测区间 16.5–25.4s，视缓存态） | <120s | 实测 |
| 回放确定性 | **已达成** `final=59d48dedf2b0677e` | 600 帧两次同哈希 | `cargo xtask replay --seed 42` |
| `doc-gate` | 256 个 `.rs` / 14 个 crate / 6 条规则 / 白名单 10 项 / 0 违规 | 0 违规 | `cargo xtask doc-gate` |

### 0.3 仓库结构现状

- crate 目录 **14 个**：`core / render / physics / audio / asset / script / doc / replay / runtime / editor-host / editor / protocol / plugin / app`，另有 `xtask`（`doc-gate` 计 14 个，不含 xtask）。
- `r3e-protocol` 已落地（8 文件 / 2,326 行 / 34 测，依赖仅 serde + serde_json），`r3e-editor` 只剩传输层（505 行 / 72 测），旧 `use r3e_editor::` 调用方零改动。
- 依赖违规 **0 条**；债务 4 条登记在 `xtask/dep_debt.json`（`plugin → core` / `plugin → physics` / `script → physics` / `script → audio`），M1 偿还（见 §3.1.7 收尾实测）。

---

## 一、先立四个判断（路线图的全部依据）

### 判断 1：当前缺的不是功能，是「AI 参与权」

引擎能力已全面协议化（369 条命令表、24 个 RPC 方法）——**但历史上这些是只读或半只读的**：`r3e-plugin` 能力声明只有 `fs.read`，AI 能看不能改。同时场景没有单一权威状态、没有事务、没有 undo。

结论：**先补参与权，再补功能**。继续堆渲染特性只会拉大"AI 能看懂但改不动"的落差。
（M0-S3/S4 已完成的部分：唯一真值 + 事务 + undo 已就位，参与权的"底座"到位，剩余"通道"归 M1。）

### 判断 2：AI 为核心的真正瓶颈是「验证成本」，不是「生成能力」

AI 写代码快，但没人能低成本判断它写对没有。旧验证路径是：改代码 → 编译 → 起 GPU → 肉眼看画面。这条路对 AI 是死路。

反推设计目标：**任何改动都必须有一条不需要 GPU、不需要人眼、可以自动判定对错的路径。**

这一条决定了：可验证性基建必须前置到 M0，不能留到"以后再补测试"。（已兑现：`verify` 一条命令覆盖 fmt / dep-check / clippy / test / doc-gate / lint-size / 600 帧回放，全程不开 GPU。）

### 判断 3：解耦对 AI 的价值是「可增量」，不是「好看」

| 单体现状（起点 → 现在） | 对 AI 的具体伤害 |
|---|---|
| `renderer.rs` 5582 → **已拆 `renderer/` 17 模块，最大 716 行** | AI 读一次吃满上下文窗口，读不全就改错 |
| `api.rs` 4070 → **已拆 `api/` 17 模块最大 428 行；editor-host 侧 3552 → 13 文件最大 433 行** | 加一个命令要扫全文件，容易破坏已有 369 条命令 |
| `editor.rs` 3235 → **随 S0 crate 拆分消解** | 改 RPC 和改主循环挤在一起，语义边界模糊 |
| `r3e-app` 混三件事 → **已薄壳化（main.rs 30 行）** | 想跑 headless 测试就必须把窗口也拖进来 |
| `scene.rs` **4057 行（当前最大单点）** | 下一个必须处理的"读不全"障碍，M1 `M1-DOC-FIELDS` 收口 |

解耦的真实收益是三条：**可增量读、可并行改、可局部验**。这三个是 AI 协作的前提，不是工程洁癖。

### 判断 4：不要为 AI 另造一套接口

AI 和未来的编辑器 UI 必须走**同一条命令通道**。否则两套语义必然漂移，届时修 bug 要修两遍，且 AI 那套永远落后于 UI 那套。

推论：**权威模型在引擎内，AI 在引擎外。** AI 只是一个"客户端"，和 UI 平级。

---

## 二、目标架构：五层 + 三条正交链

```
┌─ L4 表达层 ─────────────────────────────────────────────┐
│  编辑器 UI(未来)   CLI/AI Agent   测试脚本   CI 校验器   │
└──────────────────────┬──────────────────────────────────┘
                       │ 只允许走协议，禁止直接 use 引擎类型
┌─ L3 协议层 r3e-protocol ────────────────────────────────┐
│  JSON-RPC 消息  机器可读能力清单  事务/意图  错误语义    │
│  依赖：仅 serde。禁止依赖 core/physics/任何引擎类型      │
└──────────────────────┬──────────────────────────────────┘
┌─ L2 编辑权威层 r3e-doc ─────────────────────────────────┐
│  Document(唯一真值) → Command → Transaction → UndoStack │
│  Validator(不变量校验)  StateHash  Diff/Patch  WorldQuery│
│  依赖：仅 core 的数据结构。禁止依赖 render/physics/audio │
└──────────────────────┬──────────────────────────────────┘
┌─ L1 领域层（端口-适配器）───────────────────────────────┐
│ core | render | physics | audio | asset | anim | nav     │
│ terrain | char | flow  —— 各自出 trait 端口，互相不直连 │
└──────────────────────┬──────────────────────────────────┘
┌─ L0 平台层（可替换适配器）──────────────────────────────┐
│ wgpu | winit | rapier | rodio | ufbx/gltf               │
└─────────────────────────────────────────────────────────┘

三条正交链（贯穿每一层，不是某一层的功能）：
  ① 可观测：结构化日志 / 性能计数器 / 状态快照
  ② 可验证：CPU 不变量校验 / headless 确定性回放 / 像素回归
  ③ 可回滚：事务边界 / undo 栈 / diff-patch
```

### 2.1 crate 目标布局（9 → 13，现状已达成 13）

| crate | 状态 | 职责 | 关键约束 |
|---|---|---|---|
| `r3e-core` | 保持，内部拆分 | 纯数据与算法：场景图、动画、材质、UI 数据 | 零图形/物理依赖（已达成，保持） |
| `r3e-render` | 保持，`renderer/` 17 模块 | wgpu 后端 + gfx 抽象 | 唯一持有 GPU 句柄的层 |
| `r3e-physics` | 保持 | 物理端口 + rapier 适配器 | 端口 trait 化，`rapier` 默认 feature 待 M2 决策 |
| `r3e-audio` | 保持 | 音频端口 + rodio 适配器 | rodio 接入待 M2 决策 |
| `r3e-asset` | 保持 | 导入管线 | 不依赖 render（现状正确） |
| `r3e-script` | **改造中** | rhai 绑定 | 只能经 `CapabilityHost` trait 注入；现仍直连 `physics`/`audio`（S5-04 还债） |
| `r3e-doc` | **已建（M0-S3/S4）** | Document / Command(14) / Transaction / UndoStack / StateHash / Validator | 只依赖 `core`；写只走 `doc.write` |
| `r3e-replay` | **已建（M0-S4）** | headless 确定性回放（固定步长 + LCG 种子 + 状态哈希） | 不依赖渲染 / 音频 / 物理后端 |
| `r3e-protocol` | **待建（M0-S5）** | 协议 + 能力清单 + 事务消息 | **仅 serde**，零引擎类型 |
| `r3e-runtime` | **已建（M0-S0）** | 窗口、主循环、时间步、headless 模式 | 不含任何 RPC |
| `r3e-editor-host` | **已建（M0-S0）** | RPC 服务端，桥接 protocol ↔ doc | 不含主循环；现依赖过宽（S5-04 收敛） |
| `r3e-game` | M2 新建 | 游戏层框架：玩家、交互、关卡流程 | 示例游戏宿主，验证"引擎真的能做游戏" |
| `r3e-nav` / `r3e-terrain` | M2 新建 | NavMesh + A* / 高度图地形 | 独立 crate，不依赖渲染 |
| `r3e-plugin` | **重写（M1）** | AI 外部工具进程 | 只依赖 `protocol`，零 core/physics 依赖 |
| `r3e-editor` | 兼容别名/废弃 | 旧协议壳 | 5 个外部项迁移后删除 |

### 2.2 依赖规则表（CI 强制，违反即红）

| 来源 | 允许依赖 | 明确禁止 |
|---|---|---|
| `core` | glam, serde, bytemuck, fontdue | 任何 GPU/物理/脚本/窗口库 |
| `doc` | `core` | `render`, `physics`, `audio`, `rhai` |
| `protocol` | serde | **一切引擎 crate**（含 `core`） |
| `render/physics/audio/asset` | `core` + 各自平台库 | 互相依赖 |
| `replay` | `doc`, `core` | `render`, `physics`, `audio`, `winit` |
| `script` | `doc`, `protocol` + trait 端口 | 直接依赖具体子系统的类型 |
| `runtime` | 下层全部 + `doc` | `protocol`（运行时不需要懂 RPC） |
| `editor-host` | `protocol`, `doc`, `runtime` | `render` 内部实现细节 |
| `plugin` | `protocol` | 引擎所有其他 crate |
| `game` | 下层全部 + `doc` | `protocol`, `editor-host` |

落地方式：`cargo xtask dep-check` 解析依赖图对照上表（规则数据在 `xtask/dep_rules.json`，白名单未列入即违规）。**M0-S5（2026-11-16）已转硬门禁**：默认即严格模式，四判定任一命中即 `exit=1`，`verify` 与 CI 都按硬门跑。

| 判定 | 触发条件 | 现状 |
|---|---|---|
| ① 白名单外依赖 | 出现 `dep_rules.json` 未列明的依赖边 | 0 |
| ② 债务残留 | `dep_debt.json` 登记的 4 条边**被保留**（债还完未销账也算违规） | 4 / limit 4 |
| ③ 超限 | 债务条数 > `limit`（只减不增，新增必须改 json 并说明） | 4 = 4 |
| ④ 未登记 | 出现白名单外新边且未进 `dep_debt.json` | 0 |

债务记账（`xtask/dep_debt.json`）每条含 `from` / `to` / `why` / `plan`，`plan` 必须写清"哪个 Sprint 还、还成什么形态"，否则不算登记。

---

## 三、路线图：M0 → M4

总体原则：**每个里程碑都必须产出一个"AI 能验证的产物"**，否则不算完成。Sprint 卡是唯一的任务粒度真源；交付物编号只用于宏观对账。

### 3.0 M0 交付物 ↔ Sprint 映射（9 项）

| # | 交付物 | 验收标准（可自动判定） | 承接 Sprint | 状态 |
|---|---|---|---|---|
| M0-1 | 拆 `r3e-app` → `r3e-runtime` + `r3e-editor-host` + `main` 薄壳 | 三 crate 各自可独立测试；`runtime` 无窗口 headless 跑 600 帧 | S0-01~04 | ✅ |
| M0-2 | 三个单体文件拆到 ≤800 行/文件 | 单文件行数 CI 门禁；拆分后测试数不减 | S1-01~07、S2-04~05 | ✅（`editor.rs` 由 S0 消解） |
| M0-3 | `r3e-doc`：Document 接管场景唯一真值 | `core::scene` 不再被直接可变借用，只能经 `doc` | S3-01~06 | ✅ |
| M0-4 | Command + Transaction + UndoStack | 任意命令序列 apply 后 undo 全量回退，哈希与初始一致 | S3-03、S4-01~04 | ✅ |
| M0-5 | `r3e-protocol` 剥离，零引擎依赖 | `cargo tree -p r3e-protocol` 只有 serde | S5-01 | ✅ |
| M0-6 | 能力清单自动生成 | 清单与实现不一致 → CI 失败 | S5-02~03 | ✅ |
| M0-7 | Headless 确定性回放框架 | 同输入两次运行，帧状态哈希完全相同 | S4-05~06 | ✅ |
| M0-8 | CPU 不变量校验器 v0 | 故意注入非法场景 → 必须报错，有负向测试守护 | S3-04 | ✅ |
| M0-9 | `dep-check` 依赖门禁进 CI | 故意加一条违规依赖 → CI 红 | S0-08~09、S5-04~05 | ✅ S5-04~05 已转硬门禁（严格四判定 + 故障注入演练，见 `M0S5_PROTOCOL.md` §13.7） |

**M0 退出条件（Gate 0）**：AI 改完代码后只跑 `cargo xtask verify` 就能判断是否改对，全程不起 GPU。**这一条不达成，不进 M1。**

---

### 3.1 M0 Sprint 卡（7 个 Sprint）

#### 3.1.1 依赖顺序（本节最重要的信息）

```
S0 收口+立尺 ──► S1 renderer 拆分 ───────────┐
     │                                       ├──► S5 门禁收尾 ◄── 当前在 S5 起点
     └────────► S2 api 双份收口 ─────────────┤
                    │                        │
                    └──► S3 r3e-doc ──► S4 事务/undo/确定性 ──┘
```

硬依赖（不可调换）：S0 先于一切（没有 `lint-size` 无法验收拆分，没有基线快照发现不了测试流失）；S2 先于 S3（doc 要接管的是命令，而命令集合曾有两份 `api.rs`）；S3 先于 S4（事务必须作用在唯一真值上）。可并行：S1 与 S2 不同 crate。

#### 3.1.2 Sprint M0-S0 · 收口 + 立尺（1 周）✅ 2026-11-14

**目标**：把"两份代码并存 + 没有验收尺"变成"单一真源 + 尺子能跑"。本周不写任何新功能。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S0-01 ✅ | 决策：`runtime`/`editor-host` 是真拆分还是探索分支 | 决策记录进 `ARCHITECTURE.md` | 判**真拆分**：runtime 持帧循环、editor-host 持协议黏合 |
| S0-02 ✅ | 注册进 `members` + `workspace.dependencies` | `cargo metadata` 成功 | workspace 成员 12（11 引擎 crate + xtask） |
| S0-03 ✅ | 删除双份（`app.rs` / `editor.rs` / `api.rs`），`main.rs` 缩薄壳 | `cargo tree -p r3e-app` 不含 wgpu/winit/gilrs | `main.rs` 30 行，依赖已净 |
| S0-04 ✅ | 确认新 crate 覆盖旧实现全部能力 | 逐条能力对照 | **覆盖成立**；判据由"≥76 测"改为逐 crate 快照（旧 app 76 测随代码搬家，看单体必误判为删除） |
| S0-05 ✅ | 建 `xtask`：verify / lint-size / dep-check / gen-capabilities | `cargo xtask verify` 能跑通 | 实落 5 子命令（多 `check-tests`；后补 `doc-gate`、`replay`） |
| S0-06 ✅ | `lint-size` + 债务白名单 | 白名单外超标 → 非 0；白名单内上涨 → 非 0 | `size_debt.json` 27 项（旧口径 25，差异来自两个新 crate 文件入账；现为 24 项） |
| S0-07 ✅ | 测试数基线快照 | 任一项低于快照 → 红 | 合计 1,343；`r3e-app` 76 → 11 属位置变化 |
| S0-08 ✅ | `dep-check` v0 只报告不阻塞 | 输出违规清单 | 实落 3 条（计划 4 条：app 两条随薄壳消失；修掉 dev-deps 假阳性） |
| S0-09 ✅ | CI 接入结构门 | PR 可见 gate job | `gate` job（三道结构门，`continue-on-error` 报告态） |
| S0-10 ✅ | 文档同频 | 文档 crate 数 = `cargo metadata` | README 目录结构 + §11 门禁节、ARCHITECTURE §3/§3.1/§3.2/§3.3 |

**退出演示实测**：`verify` exit=0（fmt 0 diff / clippy 0 warning / 1,339 passed / lint-size ok / check-tests ok / dep-check 3 条报告）。
**明确留给后续**：`plugin` 协议剥离（S5-01）、`dep-check` 转 `--strict`（S5-05）、`renderer.rs` 未拆（S1）。
**熔断**：若 S0-04 覆盖不足则放弃合并、直接删两目录退回干净拆分——宁可慢一周，不可留两份。

#### 3.1.3 Sprint M0-S1 · `renderer.rs` 4487 → ≤800（1–2 周）✅ 2026-11-15

**目标**：消除"AI 读不全"的最大单点，行为零变化。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S1-01~02 ✅ | 接缝测绘 + 按域定边界（pipeline / resource / pass / state / facade） | 边界表评审并写进 ARCHITECTURE | 按域落地 |
| S1-03~06 ✅ | 逐域提取 + facade 收敛 | 每文件 ≤800；`r3e-render` 测试数不减 | `renderer/` **17 文件，最大 716 行**；外部 `use` 路径经 re-export 不变 |
| S1-07 ✅ | 像素回归验证 | 基准图**不变** | visual baseline 未更新亦通过 |

**熔断（当时执行）**：若某域必须改逻辑才能分开，留 `// TODO(split)` 并跳过，禁止顺手重构。

#### 3.1.4 Sprint M0-S2 · `api.rs` 双份收口（1 周）✅ 2026-11-15

**目标**：把"两份命令实现"变成"一份命令表 + 两个薄适配"。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S2-01~02 ✅ | 差异测绘 + 真源判定 | 三张差集清单；决策落纸 | 真源 = 命令表（`CommandSpec`），两处由它生成/校验 |
| S2-03 ✅ | 建 `CommandSpec` 表 v0 | 覆盖全部脚本函数 + RPC 方法 | `core::cmd_spec` **369 条**，`since: "M0-backfill"` 回填 |
| S2-04 ✅ | `r3e-script/src/api.rs` 按域拆 | 每文件 ≤800；script 测试数不减 | 17 文件，**最大 428 行** |
| S2-05 ✅ | `r3e-editor-host/src/api.rs` 同上 | 每文件 ≤800；host 测试数不减 | 13 文件，**最大 433 行** |
| S2-06 ✅ | 命令表 ↔ 实现 ↔ 两边注册一致性测试 | 缺一条 → 测试红 | 一致性测试上线，成为 `capabilities.json` 地基 |

**价值点**：S2-03 的这张表是 M1 `capabilities.json` / `unsupported` 探测 / `impact` 报告的唯一地基，没有它三者全要重来。

#### 3.1.5 Sprint M0-S3 · `r3e-doc` 立项：Document 接管唯一真值（1–2 周）✅ 2026-11-15

**目标**：场景有了唯一权威容器，写操作有了收口点。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S3-01 ✅ | 新建 `r3e-doc`（依赖仅 `r3e-core`） | `cargo tree -p r3e-doc` 只有 core 树 | dep-check 零违规，`allow` 仅 `r3e-core` |
| S3-02 ✅ | `Document` v0：持有 `Scene`（move 语义），提供只读 view | `Scene` 不再被 runtime/editor-host 直接可变借用 | `Deref` 只读、**拒绝 `DerefMut`**；写只走 `doc.write(\|s\| …)` |
| S3-03 ✅ | `Command` trait + `CommandKind` v0（先 10 条） | 每条有 apply/undo 对，单测 ≥10 | 实落 **14 条**，每命令 apply + undo 各 1 测 |
| S3-04 ✅ | `Validator` v0 | **故意注入非法场景的测试必须报错**（负向铁律） | **17 条**负向用例 + 21 个 issue code；正向 11 份 scene json + 4 份 morph fixture 零 Error |
| S3-05 ✅ | `runtime` 改为从 `Document` 取场景 | runtime 测试绿，帧行为不变 | 写点收口 **129 处**（script 91+9 / host 18 / runtime 11）；全量 1,427 测绿 |
| S3-06 ✅ | （新增）`cargo xtask doc-gate` 六规则并入 `verify` | 结构约束靠机器而非自觉 | 六规则逐条负向验证被拦截（注入即 rc=1，还原即 ok） |

**熔断结论：未触发。** 实测显式 `&mut Scene` 15 处（全在 doc / core 内部 / physics 接收侧），129 写点 100% 经 `borrow_mut()` 单一形态，故 S3 走**完整收口**；只有"`Scene` 40 个 pub 字段私有化"按计划推 M1（`M1-DOC-FIELDS`）。

#### 3.1.6 Sprint M0-S4 · Transaction / Undo / 确定性（1–2 周）✅ 2026-11-15

**目标**：任意命令序列可整体回退，且回退可被机器验证。

**前置（S3 已交付）**：`doc.write(|s| …)` 已是场景唯一写入口（129 处收口）、`Document::version` 逐写自增、`Command` 的 apply/undo 成对——S4 只需在 `write` 外层套前像栈、对 `scene` 做稳定序列化后哈希，**无需新增真值容器**。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S4-01 ✅ | `Transaction`：`begin/preview/apply/commit/rollback` | `preview` 前零副作用（状态哈希对比证明） | `transaction.rs` **198 行**，六入口齐；`preview` 走 `Scene: Clone` 影子场景只回哈希；`preview_is_side_effect_free` + `preview_matches_real_execution` 两条测试锁住 |
| S4-02 ✅ | 原子性：N 条命令第 k 条失败 → 前 k-1 条自动回退 | 故障注入测试：中途失败，文档状态 == 初始 | 失败按**倒序 undo** 退到 `base`，退不动报 `DocError::RollbackFailed`、绝不静默；`failed_apply_rolls_back_to_base` / `rollback_restores_base_without_version_bump` |
| S4-03 ✅ | `StateHash`：稳定序列化 → 哈希（择一，**只留一份实现**） | 同状态两次相同；不同状态必不相同 | 择 **FNV-1a64**，`state_hash.rs` **604 行**（`mix_u64/mix_f32/mix_str` 一份实现）；13 组字段逐组守卫；`r3e-runtime` 帧指纹复用同一套 mix（输出格式不变），余 3 处 FNV 在 `air`/`plugin::tools` 属资产与插件域，按方案保留 |
| S4-04 ✅ | `UndoStack` + 全量回退测试 | 任意序列 apply 后 undo 到空，哈希与初始一致（随机序列 N=100） | `undo.rs` **168 行**：**一笔事务只占一格**、`rollback_to(mark)` 退到水位；性质测试 `random_sequence_undo_to_empty_restores_hash`（N=100）；另有两条锁"失败不截断历史" |
| S4-05 ✅ | headless 确定性回放：固定步长 + 可设种子 + 600 帧 | 同输入两次运行，每 60 帧哈希完全一致 | 新建 `r3e-replay`（**546 src / 11 测**）：LCG 种子 + `dt=1/60`、每帧 9 条命令、每 10 帧一笔事务；600 帧 10 采样点两次逐行一致，`final=59d48dedf2b0677e`、`nodes=60`、`depth=60`；不依赖渲染/音频/物理后端 |
| S4-06 ✅ | `cargo xtask replay` 接进 `verify` | `verify` 输出状态哈希行 | 先 build 再跑 exe **两次**、逐行 diff（不只比末行）；`verify` 末尾追加该步；实测 rc=0 |

**收尾实测（2026-11-15）**：`r3e-doc` 单测 63 → **82**（+19：哈希 5 / 事务 9 / 撤销 6，含 1 条 N=100 性质测试），新增 `r3e-replay` 11 测；全仓 **1,427 → 1,457 单测 / 0 failed**（`--all-features` 同值）；`verify` 的 fmt / dep-check / clippy（`-D warnings`）/ test / doc-gate / lint-size / replay 全绿。`Scene: Clone` 在方案期只做探针即还原，S4 真实引入（`preview` 需真快照）。

**红线**：`StateHash` 若出现两份实现（历史：几何校验/顶点装配/FNV 各有过两份），视为失败——重复实现会漂移出真 bug。

#### 3.1.7 Sprint M0-S5 · 协议剥离 + 能力清单 + 门禁转硬（1 周）✅ 2026-11-16

> 编号保留 S5（S0–S4 已占位），是 M0 的最后一个 Sprint。

| 卡号 | 任务 | 验收 | 执行结果 |
|---|---|---|---|
| S5-01 ✅ | `r3e-protocol` 从 `r3e-editor` 剥离（仅 `serde`） | `cargo tree -p r3e-protocol` 只含 serde | 8 文件 / 2,326 行 / 34 测；依赖只有 `serde` + `serde_json`；`r3e-editor` 留 505 行传输层 + `pub use` 再导出，外部调用方零改动 |
| S5-02 ✅ | `cargo xtask gen-capabilities` 产出 `capabilities.json` | 由代码导出，格式稳定可 `git diff` 审 | 4,744 行，`commands=369` / `rpc=39` / `script_only=330` / `domains=17` / `invariants=24`；真源为 ISSUE_CODES 常量表 + RPC_METHODS + `core::cmd_spec` 交叉 |
| S5-03 ✅ | 清单与实现不一致 → CI 红 | 删掉一个命令实现但留表项 → 红 | `gen-capabilities --check` 逐字节比对进 `verify`；四项注入演练（改 cmd_spec / 改 rpc 字段 / 改 dispatch 臂名 / 手改清单）全红后还原 |
| S5-04 ⚠️ 降级 | 修 3 条依赖违规：`script`→trait 端口、`plugin`→只 protocol、`editor-host`→收敛 | `dep-check` 后 0 违规 | `editor-host` 已收敛为叶子适配器（0 违规）；`script`→`physics`/`audio` 实测 **53 方法 / 704 行 / 14 测试**直连，端口化突破「零行为变化」纪律 → **降级记账**入 `dep_debt.json` 并留 M1 契约清单（详见 `M0S5_PROTOCOL.md` §13.5 偏差表） |
| S5-05 ✅ | `dep-check` 转硬门禁（CI fail）+ 故意加违规依赖验证 | PR 加一条违规依赖 → CI 红 | 默认即严格四判定（白名单外 / 债务残留 / 超限 / 未登记），`--report` 降级为显式开关；`ci.yml` 撤 `continue-on-error`；四项故障注入演练全红 |
| S5-06 ✅ | `verify` 收敛（fmt + dep-check + clippy + test + doc-gate + lint-size + replay + 新两门） | 单条命令 <120 秒 | 九道门全硬，实测 **20.4s**；新增 `invariants` 三源比对门（24 = 24 = 24）与 `gen-capabilities --check` |
| S5-07 ✅ | `docs/AI_WORKFLOW.md`：M0 之后的标准工作流（改码 → verify → 判定） | 3 个可复制命令块 | 落地，含九道门清单 / 改码闭环 / 债务与基线记账规则 / 提交纪律 |

**收尾实测（2026-11-16）**：全仓 **1,476 单测 / 0 failed**（默认特性 111 target 含 12 doctest；`rapier` 特性 1,480）；`verify` 20.4s 九道门全绿；`dep-check --strict` 0 违规 / 4 条债务；`invariants` 24/24/24；`capabilities.json` 无漂移；回放 `final=59d48dedf2b0677e`。

**Gate 0 判据（复核结果）**：
```bash
cargo xtask verify                  # ✅ 绿，20.4s，含回放哈希
cargo xtask dep-check --strict      # ✅ 0 违规（4 条债务已登记，limit=4）
cargo xtask lint-size               # ✅ 白名单外 0 文件
cargo xtask gen-capabilities --check # ✅ 无漂移
cargo xtask invariants              # ✅ 24 = 24 = 24
```

**遗留项（Gate 0 允许带账过，M1 处理）**：
1. `>800` 行文件数 **23**，未收敛到 12 以内——`r3e-core/src/scene.rs` 4,057 行归 M1 `M1-DOC-FIELDS`，其余随 M1/M2 按域拆分；`size_debt.json` 23 项只减不增，`verify` 每次盯住不涨。
2. 依赖债 4 条（`plugin → core/physics`、`script → physics/audio`）留在 `dep_debt.json`，M1 开端口契约时偿还；偿还前 `dep-check --strict` 会拒绝第 5 条新增。

#### 3.1.8 M0 周历（单人业余口径，含缓冲）

| 周 | Sprint | 主任务 | 里程碑检查点 | 实际 |
|---|---|---|---|---|
| W1 | S0 | 收口 + xtask + 快照 | 双份代码消失，`verify` 能跑 | ✅ 2026-11-14 |
| W2 | S1 | renderer 拆分前半 | `renderer.rs` 首刀 | ✅ |
| W3 | S1 + S2 | renderer 收尾 / api 差异测绘 | visual baseline 不变 | ✅ |
| W4 | S2 | 命令表 + api 按域拆 | 一致性测试上线 | ✅ |
| W5 | S3 | doc crate + Document | 场景唯一真值成立 | ✅ 2026-11-15 |
| W6 | S4 | 事务 + undo + 哈希 | 全量回退通过 | ✅ 2026-11-15 |
| W7 | S4 + S5 | 回放 + 协议剥离起步 | 回放哈希稳定 | ✅ 回放完成 |
| W8 | S5 | 门禁转硬 + 文档 | **Gate 0** | ✅ 2026-11-16（带 2 项遗留账） |

即 7–8 周（路线图估 4–6 周全职，此处按业余 15–20h/周折算，符合"2–3 倍折算"下限）。

---

### 3.2 M1：AI 写入闭环（核心期，约 4–6 周，5 个 Sprint）

**目标**：让 AI 从"看"变成"改"，并且改得可回滚、可解释、可验证。

| Sprint | 主题 | 关键卡 | 退出演示 | 对应交付物 |
|---|---|---|---|---|
| M1-S1 | 事务协议上链路 | `tx.begin/apply/preview/commit/rollback` 走 `r3e-protocol`；`r3e-plugin` 重写为只依赖 protocol；能力声明加 `scene.write` `doc.transaction` | 外部进程 dry-run 返回结构化 diff，`commit` 前场景哈希不变 | M1-1、M1-2 |
| M1-S2 | 语义查询 | `WorldQuery`：按 kind/tag/bbox/component 过滤，结果含 `{id,name,tags,pos,interact_radius}` | AI 不读裸 JSON 就能回答"场景里有哪些可交互物"（3 个问答用例） | M1-3 |
| M1-S3 | 意图命令 + 变更半径 | `spawn_prefab` / `paint_terrain` / `place_light`；一次意图 = N 条原子命令，undo 一次全退；输出含 `impact` | 一条意图命令的 `impact` 能列出受影响对象/材质 | M1-4、M1-7 |
| M1-S4 | 内容验证器 | 可通行性、悬空、穿模、材质缺失、引用断裂 | 故意生成不可达关卡 → 报出具体失败点位坐标 | M1-5 |
| M1-S5 | CLI + 三范式 | `r3e-cli tx/query/verify/replay`；三个从零可复现的 AI 任务范式 | **Gate 1** | M1-6、M1-8 |

**待办顺延**：`M1-DOC-FIELDS`（`Scene` 40 个 pub 字段私有化 + `scene.rs` 4057 行按域拆），随 M1-S1 一并处理。

**Gate 1（M1 退出）**：一个不改引擎代码的 AI Agent，仅用 `r3e-cli` + 协议完成"生成可通行小场景 → 验证 → 失败回滚重试 → 成功提交"，全过程命令可复制粘贴。

**M1 顺序依据**：协议先于插件（否则插件无处可依）、查询先于意图（否则意图命令设计没有反馈源）、验证器在意图之后（先有内容才谈验证）、CLI 最后（它是前四者的壳）。

---

### 3.3 M2：游戏可用最小集（结构期，约 6–8 周）

**目标**：补齐 RPG/沙盒的结构性缺口。**严格串行，不可并行**（后项依赖前项）。

| 序 | 内容 | 前置 | 判据（可自动判定） |
|---|---|---|---|
| 1 | 角色控制器（胶囊/台阶/斜坡/地面吸附） | 物理端口 trait 化完成 | headless 里角色沿斜坡上行高度误差 <2cm |
| 2 | 地形（高度图/分块/材质混合/碰撞） | 1 | 200×200 米地形可由 CLI 生成，哈希可复现 |
| 3 | NavMesh + A\* | 1、2 | 给定起终点，路径可通行且长度 ≤ 直线 1.6 倍 |
| 4 | Prefab / 实例化 + 覆盖 | 无 | 同 prefab 实例 100 个，覆盖字段只存差异 |
| 5 | IK + 根运动 + 动画事件 | 1 | 动画事件在固定帧触发，可断言 |
| 6 | 场景分块流式加载 | 2、4 | 分块切换不产生帧尖峰（<2ms） |
| 7 | `r3e-game` 垂直切片 | 全部 | 可走、可交互、有 NPC、有任务，60fps |

**M2 铁律**：每完成 1–2 项就做一次"可玩性回归"，不许 7 项全做完才第一次试玩。
**M2 待决策**（从 M0 顺延的两项）: `rapier` 是否转默认 feature、`r3e-audio` 是否接 rodio —— M0 不动以避免与拆分风险叠加。

**M2 退出条件**：`r3e-game` 里一个 200×200 米地形场景，玩家可自由行走、NPC 能寻路追击、目标硬件稳定 60fps，且**全部可由 CLI 生成与验证**。

---

### 3.4 M3：内容管线 AI 化（产能期，约 6–8 周）

**目标**：让 AI 能生产内容，而不只是操作场景。按下列顺序推进（不排周）：

资产管线正式化（导入 → 校验 → 引用完整性 → 增量重建）→ `r3e-flow` 数据驱动（对话树/任务/状态机）→ 数据 Schema 化 + 从 schema 反向生成 AI 提示 → 关卡生成器接口（程序化 + AI 混合，带种子可复现）→ 平衡性/可玩性模拟器（headless 跑 N 次）→ `r3e-cli content` 子命令族 → 资产内容寻址 + prefab 引用替换。

**Gate 3**：AI 仅凭自然语言需求 + schema，产出"一段对话 + 一个任务 + 一个关卡块"，自动通过校验与可玩性模拟，人工只做审美判断。

---

### 3.5 M4：工业化（打磨期，持续）

**不做排期，只做准入**：每一项必须由"做游戏时实际卡住"的实例驱动，且必须先写一句"卡在哪一步、卡了多久"。无实例不立项。

| 优先 | 内容 | 理由 |
|---|---|---|
| 高 | TAA + 运动模糊 | 画面完成度分水岭 |
| 高 | 音频总线 + 混音 + 动态音乐 | 现在只能"播空间音"，做不出游戏音频设计 |
| 高 | 编辑器 UI（`egui` 或 Web 前端，走现有协议） | 人类介入 AI 产出的必要界面；协议已就位 |
| 中 | 第二图形后端（建议只做 Vulkan） | 验证 gfx 抽象是否真不泄漏 |
| 中 | 性能体系：多线程渲染、动态分辨率、分层预设 | 沙盒场景必然撞性能墙 |
| 中 | OIT 半透明 / 金属度贴图 / 多 UV | 美术产能的实际约束 |
| 低 | Ragdoll / 软体 / 流体 | 加分项，非门槛 |
| 低 | 网络同步 | 单机 RPG 不需要；要做联机再单独立项 |

**M4 原则**：先做出一个能玩的东西，再让它变好。

---

## 四、"AI 为核心"的六个具体机制

### 机制 1：能力自描述（机器可读，不手写）

```
cargo xtask gen-capabilities  →  capabilities.json
{
  "version": "1.0",
  "commands": [{ "name": "scene.spawn", "params_schema": {...},
                 "reversible": true, "cost": "low", "since": "M0" }],
  "invariants": [...],
  "unsupported": [...]   // 明确列出"不支持"，供 AI 探测降级
}
```

关键：**由代码导出，不是文档手写**。清单与实现不一致，CI 直接红。`unsupported` 继承现有 `-32601` 的诚实文化——让 AI 能可靠区分"没这个能力"和"执行失败"。（地基已就位：`core::cmd_spec` 369 条命令表 + 一致性测试；导出动作在 S5-02。）

### 机制 2：事务优先，AI 永不直接改状态

```
AI 意图 → tx.preview(dry-run) → 得到 diff + impact → 自检 → tx.commit | tx.rollback
```

AI 不允许直接 mutate 场景。所有写入必须经 `r3e-doc` 的 Transaction，天然带 undo 边界。这一条同时解决三个问题：可回滚、可审查、可并行（不同事务冲突可检测）。
（已在 M0-S4 落地：`preview` 走影子场景只回哈希、零副作用；N 条命令第 k 条失败按倒序 undo 回 base；一笔事务在 `UndoStack` 只占一格。）

### 机制 3：语义查询代替原始数据读取

不要给 AI 369 个命令 + 一堆 JSON。给它：

```rust
world.query(Query { kind: Interactable, in_bbox: bbox, has_component: "trigger" })
  → [{ id, name, tag: ["rpg","chest"], pos, interact_radius }]
```

粗粒度、意图化、稳定。AI 处理语义对象，不处理内存布局。

### 机制 4：三层验证，逐层加价

| 层 | 手段 | 成本 | AI 可自主跑 |
|---|---|---|---|
| V1 | 编译 + 单元/契约测试 | 秒级 | ✅ |
| V2 | headless 确定性回放 + 不变量校验 + 内容验证器 | 十秒级 | ✅ |
| V3 | 像素回归 / 人工审美 | 分钟级 + GPU | ❌（只做最终确认） |

**设计红线**：任何新功能，若无法在 V1/V2 层判定其对错，则该功能设计不合格，打回重设计。这是"AI 为核心"能否成立的唯一硬保障。

### 机制 5：确定性是一等公民

固定时间步、可设种子、可控帧率、状态可哈希。缺了这条，AI 无法重现 bug、无法回归、无法自检。
（已落地：`StateHash` 为 FNV-1a64 **唯一一份实现**；`r3e-replay` 固定 `dt=1/60`、LCG 种子、600 帧 10 采样点两次逐行一致。）

### 机制 6：AI 开发引擎自身的工作流

| 环节 | 机制 |
|---|---|
| 上下文控制 | 单文件 ≤800 行硬门禁，保证 AI 一次读全 |
| 改动边界 | 依赖门禁 + 每 crate 独立可测，AI 改动影响面可预测 |
| 并行开发 | crate 级隔离，允许多个 AI Agent 同时改不同 crate |
| 反馈闭环 | `cargo xtask verify` 一条命令覆盖全部 V1/V2（20.4s 热缓存，九道门） |
| 契约守护 | 每个 crate 出端口 trait，改实现不破契约；契约测试由 AI 补写 |

---

## 五、必须避免的五个反模式

| 反模式 | 为什么是坑 | 正确做法 |
|---|---|---|
| 在 `r3e-app` / `r3e-runtime` 里直接塞 AI 逻辑 | 引擎被 AI 供应商绑定，换模型要重写，且 AI 错误会污染主循环 | AI 永远在进程外，只经 protocol |
| 给 AI 一堆细粒度 RPC 让它自由编排 | 编排逻辑散落在提示词里，不可复现、不可回滚、每次结果不同 | 高层意图命令 + 引擎内展开为原子命令 |
| 先用自然语言提示词驱动，再补验证 | 验证缺失时 AI 的产出无法判断，会积压成技术债 | 验证先于生成（M0 先于 M1） |
| 为 AI 单开一套"简化接口" | 两套语义必然漂移，AI 那套永远落后 | AI 与 UI 共用同一条命令通道 |
| 功能清单驱动 M4 | 做了一堆没人用的特性，游戏还是不好玩 | 由"做游戏时实际卡住"驱动 |

### 每周五 5 分钟自检（把上表变成动作）

| # | 问题 | 若"是" |
|---|---|---|
| 1 | 是否把 AI 逻辑写进 `r3e-app` / `r3e-runtime`？ | 立即移到进程外，只经 protocol |
| 2 | 是否让 AI 直接编排细粒度 RPC？ | 收敛为意图命令 |
| 3 | 是否先加功能、后补验证？ | 补验证优先于下一个功能 |
| 4 | 是否给 AI 另开了一套"简化接口"？ | 合并回同一命令通道 |
| 5 | 本周新增的文件/功能，能否在 V1/V2 层（无 GPU）判定对错？ | 不能 → 判为设计不合格，打回 |

---

## 六、优先级总表（一张纸看完）

| 阶段 | 主题 | 周期 | 关键交付 | AI 能力跃迁 |
|---|---|---|---|---|
| **M0** | 解耦 + 可验证地基 | 4–6 周（业余 7–8） | doc 模型、事务、headless 回放、依赖门禁 | 从「只能读」到「改得动、验得了」 |
| **M1** | AI 写入闭环 | 4–6 周 | 事务协议、语义查询、内容验证器、CLI | 从「改得动」到「自主闭环」 |
| **M2** | 游戏可用最小集 | 6–8 周 | 角色/地形/NavMesh/prefab/垂直切片 | 从「操作场景」到「做出游戏」 |
| **M3** | 内容管线 AI 化 | 6–8 周 | 数据驱动 flow、schema、可玩性模拟 | 从「做游戏」到「AI 产内容」 |
| **M4** | 工业化 | 持续 | TAA、音频总线、编辑器 UI、性能体系 | 从「能玩」到「卖得动」 |

**总周期估计**：M0–M3 约 5–7 个月（单人全职口径）；业余按 2–3 倍折算。

---

## 七、执行节奏与治理（先定规则，再排任务）

### 7.1 时间盒

| 单位 | 定义 | 硬约束 |
|---|---|---|
| **Sprint** | 1 个自然周 | 周五必须有一次"可演示证据"，否则视为未完成 |
| **任务卡** | 0.5–2 天 | 超过 2 天必须再拆，禁止"巨型卡" |
| **里程碑** | 6–8 个 Sprint | 退出条件由本文档定义，**不可临时放宽** |

单人 + AI 协作口径：每周有效开发按 15–20 小时折算。AI 可并行改不同 crate，但**同一 crate 同一周只允许一条改线**。

### 7.2 每周固定动作（不占用决策时间）

| 时点 | 动作 | 产出 |
|---|---|---|
| 周一 30 min | 领本周 Sprint 卡，确认依赖前置已绿 | Sprint 卡片上墙 |
| 每日 | `cargo xtask verify` | 绿/红 |
| 每次提交前 | `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` | 无告警 |
| 周五 30 min | 更新 §八看板 6 个数字 + 过一遍 §五自检清单 | 看板追加一行 |
| 周五 15 min | 跑一遍本周"退出演示"命令并贴输出 | 演示记录 |

### 7.3 提交规范

```
feat(<crate|scope>): 一句话说清行为变化
refactor(<crate>): 拆分/搬运，行为不变（必须写明测试数不变）
docs(<doc>): 与代码同频更新（架构/README 单测数同步）
chore(ci): 门禁与脚本
```

规则：**重构提交与行为变更提交必须分开**。"拆文件顺便改逻辑"是本项目最大的漂移风险来源。

### 7.4 熔断规则（超时即降级，不许硬扛）

| 情况 | 动作 |
|---|---|
| 单张卡超 2 天未完成 | 拆卡或降级；写下阻塞原因再继续 |
| 同一卡连续失败 2 次 | 换方案，不许在参数上微调硬试 |
| Sprint 结束 2 张以上卡未完成 | 本周不追加新内容，下周只做收口 |
| 拆分导致测试数下降 | **立即停止后续所有拆分**，先修测试归属 |
| 拆分导致像素基线变化 | 视为失败，回退该卡（重构不许改画面） |

---

## 八、度量看板（每周五更新一行，只许变好）

| # | 指标 | 当前 (2026-11-16) | M0 目标 | 采集方式 |
|---|---|---|---|---|
| 1 | 最大文件行数 | 4057 | ≤800 | `cargo xtask lint-size` |
| 2 | `>800` 行文件数 | 23 | ≤12 | 同上 |
| 3 | `#[test]` 总数 | 1476 | 只增不减 | `cargo xtask check-tests` |
| 4 | 依赖违规数 | 0（债务 4 条已登记） | 0 | `cargo xtask dep-check --strict` |
| 5 | `verify` 时长 | 20.4s（热缓存，九道门） | <120s | 实测 |
| 6 | 回放确定性 | 已达成（`final=59d48dedf2b0677e`） | 600 帧两次同哈希 | `cargo xtask replay --seed 42` |

记录表（每周追加）：

| 周 | 最大行数 | >800 数 | 测试数 | 违规数 | verify(s) | 回放 |
|---|---|---|---|---|---|---|
| 2026-11-14 | 4487 | 25 | 1408 | 4 | — | — |
| 2026-11-15 | 4053 | 24 | 1427 | 3 | — | — |
| 2026-11-15（M0-S3 收口） | 4053 | 24 | 1427 | 3 | — | — |
| 2026-11-15（M0-S4 收口） | 4057 | 24 | 1457 | 3 | 19.6 | 600 帧两次一致 |
| 2026-11-16（文档合并复核） | 4057 | 24 | 1457 | 3 | 14.7 | 600 帧两次一致 |
| 2026-11-16（**M0-S5 收口 / Gate 0**） | 4057 | 23 | 1476 | **0** | 20.4 | 600 帧两次一致 |

---

## 九、风险登记册

| ID | 风险 | 触发信号 | 缓解 / 回退 |
|---|---|---|---|
| R1 | 拆分过程中"顺手改逻辑"导致行为漂移 | 像素基线变化 / 契约测试红 | 重构与行为提交分离；漂移即回退该卡 |
| R2 | 双份代码并存 | `git status` 出现重复 crate | S0 已强制收口；未注册 crate 直接删 |
| R3 | 测试归属迁移导致计数下降 | 快照比对红 | 冻结拆分，先补测试归属 |
| R4 | `doc` 接管 `scene` 引发大范围 `&mut` 冲突 | 编译错误数 >50 | 降级为"只读视图 + 写入口收口"，字段私有化推后（已按此执行） |
| R5 | 同一逻辑两份实现（历史：几何校验/顶点装配/FNV） | grep 出同名函数 2 份 | 列为高危，发现即合并；`StateHash` 明确唯一 |
| R6 | 时间不足，Sprint 堆积 | 连续 2 周未完成率 >30% | 只留门禁卡，非门禁卡进 Backlog；**M0 退出条件不可放宽** |
| R7 | 门禁一上线即全红，长期挂红失去意义 | dep-check 常态违规 | 先报告后阻塞（S0 v0），S5-05 才转硬 |
| R8 | 文档与代码漂移（两份路线文档各写一遍、数字互相引用） | 引用编号对不上 / 同一指标两处写不同值 | 本合并已把路线与执行收成一份，指标只在 §八维护；清单类数据（超标文件、测试基线）指向 `xtask/*.json` 单一真源，**禁止在文档里手抄第二份** |

---

## 十、下一步（当前 Sprint 的执行入口）

M0 收官（S0–S5 全部 ✅，Gate 0 已过）。**下一个入口是 M1（AI 写入闭环，§3.2）**：

1. **先还账**：`xtask/dep_debt.json` 的 4 条债随 M1 端口契约偿还；`>800` 行 23 项按 `M1-DOC-FIELDS`（`scene.rs` 4,057 行）优先拆；
2. **M1 主线**：语义查询层（`capabilities.json` 369 命令 → 收敛的查询面）+ 事务化写入 + 三层验证；
3. **纪律不变**：任何改动前跑 `cargo xtask verify`（应 exit=0），改动后同一命令必须仍绿——细则见 `docs/AI_WORKFLOW.md`；
4. **本地提交不推送**，按 D1…Dn 分段提交。

**开工前先跑一次** `cargo xtask verify`（应 exit=0），确认起点是绿。

---

## 附录 A：与现状的对应关系

| 现状资产 | 在路线图中的角色 |
|---|---|
| `r3e-core` 零图形依赖 + 大量纯 CPU 测试 | M0 headless 验证的地基，已直接复用 |
| 369 条命令表（`core::cmd_spec`）+ 一致性测试 | S5-02 `capabilities.json` 的原材料（已产出 4,744 行清单），M1 语义查询要**收敛**的对象 |
| `-32601` 诚实失败语义 | 机制 1 `unsupported` 字段的文化来源 |
| `Document` / `Transaction` / `UndoStack` / `StateHash` | 机制 2、机制 5 的已落地实现 |
| `r3e-replay`（546 行 / 11 测） | 机制 4 的 V2 层，Gate 0 的判据来源 |
| 1476 个测试 | V1 验证基线，任何拆分不得减数 |
| `gate` CI job（报告态） | S5-05 转硬门禁的接入点 |
| 动画混合空间（深水区） | 相对成熟，M2 只需补 IK/根运动/事件 |
| `r3e-protocol`（仅 serde，2,326 行 / 34 测） | S5-01 的产物：协议层独立 crate，`r3e-editor` 只剩传输层 |

## 附录 B：验证命令约定（`xtask` 唯一清单）

```bash
# .cargo/config.toml 已注册：[alias] xtask = "run --package xtask --"

cargo xtask verify            # 九道门一次跑完：fmt + dep-check(--strict) + clippy(-D warnings) + test
                              # + doc-gate + lint-size + check-tests + gen-capabilities --check + invariants + replay
cargo xtask dep-check         # 依赖规则表校验（默认严格四判定，任一命中 exit=1；--report 只报告）
cargo xtask lint-size         # 单文件 ≤800 + 债务白名单（size_debt.json）只减不增
cargo xtask check-tests       # #[test] 数与基线快照（tests_baseline.json）比对
cargo xtask doc-gate          # 写入口 / 唯一真值六条结构规则
cargo xtask gen-capabilities  # 导出 docs/capabilities.json；--check 逐字节比对（不进 verify 的产物会被判红）
cargo xtask invariants        # 不变量三源比对（ISSUE_CODES / RPC_METHODS / cmd_spec），当前 24 = 24 = 24
cargo xtask replay --seed 42 --frames 600   # 确定性回放，输出状态哈希（逐行比对两次运行）
```

`verify` 单次实测 20.4s（热缓存；同机区间 16.5–25.4s），是新代码进入仓库前唯一必跑的命令；细则见 `docs/AI_WORKFLOW.md`。

## 附录 C：任务卡模板

```
卡号：M0-S5-04
标题：修 3 条依赖违规（script/plugin/editor-host）
输入：dep-check 现状 3 条违规（9 条依赖边）
动作：script → trait 端口注入；plugin → 只留 protocol；editor-host → 收敛到 protocol+doc+runtime
产出：dep-check 0 违规
验收命令：cargo xtask dep-check --strict   （应 exit=0）
预算：1 天
回退：git checkout -- crates/（依赖改动集中在 Cargo.toml 与 lib.rs 导出）
风险：中（依赖方向改动会影响编译面，靠 verify 兜底）
```

## 附录 D：基线校准（2026-11-14 历史快照，仅作起点参照）

> 口径提醒：下表是 M0 开工时的实测，**不要当现值用**。现值只看 §八看板；清单类数据（超标文件、测试基线）以 `xtask/size_debt.json`、`xtask/tests_baseline.json` 为单一真源，本文不再手抄。

| 项目 | 当时路线图口径 | 当时实测 | 判定 |
|---|---|---|---|
| crates 数 | 9 | workspace `members` = 9，磁盘上 11 个目录 | ⚠️ 偏差（即双份代码问题，S0 已解决） |
| 源文件数 | 158 | 162 | ⚠️ 偏差 |
| 代码行数 | 约 6.9 万 | 80,088 | ⚠️ 偏差 |
| 测试数 | 1335 | 1408 个 `#[test]` | ✅ 只增不减 |
| 最大文件 | `renderer.rs` 4487 | 4487（一致） | ✅ |
| `>800` 行文件数 | 未统计 | 25 个 | 需登记白名单（S0-06 落地） |
| `cargo xtask` | 假定已存在 | **不存在**，无 alias | ❗ 全部验证命令当时不可用（S0-05 已建） |
| CI job | 5 个 | 5 个 | ✅ |
| `rapier` 默认开启 | 要求默认开启 | `default = []` | ⚠️ 顺延 M2 决策 |
| `r3e-audio` 接 rodio | 要求端口 + rodio | 依赖仅 `log` | ⚠️ 顺延 M2 决策 |

**当时的关键发现**：`r3e-runtime` / `r3e-editor-host` 已经有完整代码却**不在 workspace `members`**、且 `r3e-app` 旧 `app.rs`(3235) / `editor.rs`(3442) 同时还在——同一套主循环与 RPC 存在两份副本，其中一份从未被编译过，编译器不会报警。这一条直接催生了 Sprint M0-S0（先收口、再立尺）。

## 附录 E：文档合并说明（2026-11-16）

### E.1 为什么合并

原 `AI_ROADMAP.md`（357 行）与 `AI_ROADMAP_EXECUTION.md`（472 行）说的是同一件事的两个切面：前者是判断 + 架构 + 里程碑定义，后者是同一批里程碑的 Sprint 细化。实际后果是：M0 的交付物写了两遍（ROADMAP §三 M0-1~M0-9 vs EXECUTION 的 S0–S5 卡片）、验证命令写了两遍（ROADMAP 附录 B vs EXECUTION §9.2）、同一组指标两处各维护一份数字，正是 §九 R8 描述的风险。合并后：**判断与机制只留一份骨架，Sprint 卡是任务粒度的唯一真源，指标只在 §八维护。**

### E.2 章节映射

| 原文档章节 | 合并后位置 | 处理 |
|---|---|---|
| ROADMAP §一 四个判断 | §一 | 原文保留，补 M0 已兑现事实 |
| ROADMAP §二 目标架构 / 2.1 / 2.2 | §二 / §2.1 / §2.2 | 保留编号（`xtask/dep_rules.json` 外部引用 §2.2），状态列按现状更新，新增 `replay` 行 |
| ROADMAP §三 M0–M4 | §三（3.0 映射 + 3.1~3.5） | M0 交付物表压缩为"Sprint 映射表"，细节以 Sprint 卡为准；M2/M3/M4 与 EXECUTION §5 门禁表合并为"前置 + 判据"一张表 |
| ROADMAP §四 六个机制 | §四 | 原文保留，标注已落地部分 |
| ROADMAP §五 五个反模式 | §五 | 与 EXECUTION §8 每周自检合并为"反模式表 + 自检清单" |
| ROADMAP §六 优先级总表 | §六 | 保留 |
| ROADMAP §七 立即可执行的第一步 | §十 | 改写为"当前 Sprint 执行入口"（原内容已完成） |
| ROADMAP 附录 A / B | 附录 A / 附录 B | 附录 B 与 EXECUTION §9.2 合并为唯一命令清单 |
| EXECUTION §0 / §1 基线校准 | 附录 D | 压缩为历史快照 + 口径提醒 |
| EXECUTION §2 节奏约定 | §七（7.1~7.4） | 保留全部四条规则 |
| EXECUTION §3 M0 任务地图（含 S0 执行记录、周历） | §3.1（3.1.1~3.1.8） | 保留卡号与执行记录（含偏差），S1/S2 补收口结果，S5 标注当前 |
| EXECUTION §4 M1 细化 | §3.2 | 与 ROADMAP M1 交付物表合并为一表 + 顺序依据 |
| EXECUTION §5 M2/M3/M4 门禁 | §3.3~§3.5 | 同上 |
| EXECUTION §6 度量看板 | §八 | 唯一指标维护点，补 2026-11-16 行 |
| EXECUTION §7 风险登记册 | §九 | 补 R8 文档漂移风险 |
| EXECUTION §8 反模式自检 | §五 末表 | 合并 |
| EXECUTION §9.1 任务卡模板 | 附录 C | 换成当前 Sprint 的示例 |
| EXECUTION §9.2 xtask 命令 | 附录 B | 与 ROADMAP 附录 B 合并去重 |
| EXECUTION §9.3 与上游路线图的差异登记 | E.3 | 保留（历史自洽性） |

### E.3 原"与上游路线图的差异登记"（保留原文，说明当时的调整依据）

相对原路线图口径的调整（均由实测基线偏差触发，方向未变）：

1. 新增 **M0-S0 收口 Sprint**（原路线图未预见"已存在半成品重复 crate"）；
2. 将"验证基建"从 M0 任务之一提升为 **S0 前置条件**（先有尺再动刀）；
3. 明确 **S2 命令表（`CommandSpec`）先于 doc 模型**，因为存在两份 `api.rs`；
4. `rapier` 默认 feature 与 `r3e-audio` 接 rodio 两项偏差**推迟到 M2 决策**；
5. M0 周期由 4–6 周全职口径细化为 **7–8 周业余口径**。

### E.4 外部引用待同频

以下文件在原 `AI_ROADMAP_EXECUTION.md` 被删除后，若仍在引用旧文件名，需指向本文档对应章节（历史方案文档 `M0S1_RENDERER_SPLIT.md` / `M0S2_API_SPLIT.md` / `M0S3_DOC_LAYER.md` / `M0S4_TRANSACTION.md` 属已完成的一次性方案记录，保留原文不改写历史）：

| 文件 | 旧引用 | 新指向 |
|---|---|---|
| `.github/workflows/ci.yml` | `docs/AI_ROADMAP_EXECUTION.md §3.1` | `docs/AI_ROADMAP.md §3.1` |
| `xtask/dep_rules.json` | `docs/AI_ROADMAP.md §2.2` | 不变（§2.2 编号保留） |

*（本文档由 AI 辅助生成，判断与架构来自 2026-11-14 原始路线图，Sprint 执行记录与验收判据均来自仓库实测）*
*（内容由AI生成，仅供参考）*
