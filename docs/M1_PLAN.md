---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_758fa652e1a111f19919525400638852
    ReservedCode1: Wo3z8VGMt7uLUeXoPjDk/PX08NpVOAV3tTZdMyDSN5zsc0iLe+6mxCTqMuB/vqf2EDeeXJ4vyB6ahFdAjASUWiA7D6HwJpnJn20wbZ5cDLXZnLqtl7eRBWaMgTeEvlr8WI60QvZnfI8cCfWAmAAyja68HZaOxWq/bu9Y7O06bhzWJ3UPWgLeNXZVvNY=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_758fa652e1a111f19919525400638852
    ReservedCode2: Wo3z8VGMt7uLUeXoPjDk/PX08NpVOAV3tTZdMyDSN5zsc0iLe+6mxCTqMuB/vqf2EDeeXJ4vyB6ahFdAjASUWiA7D6HwJpnJn20wbZ5cDLXZnLqtl7eRBWaMgTeEvlr8WI60QvZnfI8cCfWAmAAyja68HZaOxWq/bu9Y7O06bhzWJ3UPWgLeNXZVvNY=
---

# M1 执行方案（AI 写入闭环）

- 基线：M0 收口（S0–S5 全绿，Gate 0 已过），2026-11-16
- 口径来源：`docs/AI_ROADMAP.md` §3.2（M1-S1 ~ M1-S5 + `M1-DOC-FIELDS` 顺延项）
- 落地节奏：**先出方案 → 再执行 → 后对齐文档**；每张卡独立 commit，本地 ahead，不推送
- 本文性质：可执行蓝图，不是路线图复述。每张卡写明改哪些文件、验收怎么判、文档改哪几节

---

## 1. 三条硬约束（决定方案形状，不是建议）

以下三条来自本轮仓库实证勘察，M1 的方案必须绕开它们，否则会在中途推倒重来。

### C1 事务态不能跨 RPC 持有

`doc` 层的 `Transaction` 借用 `&mut Document`，且 `commit` / `rollback` 消费 `self`。
RPC 是**无状态请求-响应**模型，无法在一次调用之间保持一个借用了 `&mut Document` 的事务对象。

→ **结论**：事务身份必须"实体化"到宿主侧（`tx_id` + 命令队列 + 基快照哈希），`doc::Transaction` 只在 `commit` 的单个 `&mut Document` 作用域内被瞬时构造并消费。

### C2 命令化实际覆盖率约 1/4，不是"收口未完成"而是"设计上不该全收"

实测 `CommandKind` 映射可行性：

| 写入面 | 写点数 | 可映射 `CommandKind` | 说明 |
|---|---|---|---|
| `r3e-editor-host` RPC 面 | 16 | 12 | 场景/光照/环境/后处理参数，天然是命令 |
| `r3e-runtime` | 11 | **0** | 引擎推进类：动画采样、物理步进、相机跟随，不是"编辑意图"，本就不该进 undo 栈 |
| `r3e-script` | ~130 | 约 28–31 | 经 `ScriptContext::write` 唯一出口，但域覆盖极不均（`anim_blend`/`env`/`light` 有命令，`anim/shadow/physics` 域整体无命令） |
| `doc` 内部 / 测试 | 其余 | — | 真值层自身写入，不属于外部写点 |

→ **结论**：M1-S1 不能承诺"全量命令化"。只能承诺 **"AI 可写域（可编辑意图）必须走命令"**，并把引擎推进域**显式排除**在协议面之外。这是本期唯一的正确目标。

### C3 写点总量 224，隐性写点必须被记账而非被消灭

全仓 `.write(` 实测 **224** 处（`r3e-script` 占 ~130，`r3e-runtime/src/app.rs` 11，`editor-host` 若干，`doc` 内部与测试若干）。
一次性把它们全改造成命令，必然破坏"零行为变化"纪律，且 M0-S5 已因同类操作降级记账（`dep_debt.json` 4 条）。

→ **结论**：采用**登记式门禁**代替**禁止式门禁**。M1 只消灭 AI 路径上的绕行，其余写点进入台账（见 S1-C）。

---

## 2. 范围：做什么 / 不做什么

**做**
1. `tx.*` 事务协议上链，AI 侧唯一写入口具备 dry-run 与原子回滚
2. `WorldQuery` 语义查询面，AI 不读裸 JSON 就能回答场景问题
3. 意图级命令（一次意图 = N 条原子命令，undo 一次全退）+ `impact` 变更半径
4. 内容验证器（可通行性 / 悬空 / 穿模 / 材质缺失 / 引用断裂）
5. `r3e-cli`（`tx` / `query` / `verify` / `replay`）+ 三个可复现范式
6. `M1-DOC-FIELDS`：`r3e-core/src/scene.rs`（4,057 行）按域拆分

**不做（明确划出，防止范围蔓延）**
- `r3e-runtime` 引擎推进域命令化（C2）
- 外部进程的 `uasset` 直读 / 资产管线正式化（属 M3）
- `paint_terrain` 的真实地形网格生成与 LOD（M1 只做**数据壳 + undo + impact**，几何求值留 M2）
- 资产库落盘缓存、依赖图、热重载（M1-ASSET 系列，M2 起）

---

## 3. Sprint 拆解

顺序依据（沿用 Roadmap §3.2）：协议先于插件、查询先于意图、验证器在意图之后、CLI 最后。
每张卡一行 commit，格式 `feat(m1-sN): <卡名>`。

### M1-S1 事务协议上链路（拆 4 卡，地基）

| 卡 | 内容 | 主要改动面 | 验收 |
|---|---|---|---|
| **S1-A** | 协议面新增 `tx.begin / tx.apply / tx.preview / tx.commit / tx.rollback` | `r3e-protocol/src/lib.rs:53`（`RPC_METHODS` 39 条 → 44 条）+ dispatch 三层 match 补臂 + `cmd_spec` + `gen_capabilities` 重生成 | 五个方法可被分发；`cargo xtask gen-capabilities` 后 `capabilities.json` 增量且 `EXPECTED_COMMANDS`（`gen_capabilities.rs:33`，369）不变 |
| **S1-B** | 宿主事务态 `TxRegistry`：`tx_id → {base_state_hash, staged: Vec<Command>, created_at}`；`tx.apply` 只入队并返回结构化 diff；`tx.commit` 在单个 `&mut Document` 作用域内构造 `doc::Transaction`、逐条 apply、`commit` | `r3e-editor-host`（新增 `tx` 模块 + 宿主 API）+ 复用 `doc` 六入口（`begin/apply/stage/preview/commit/rollback` 均已实现，无需改） | dry-run 返回结构化 diff；`commit` 前 `StateHash` 不变（与基线逐字节相等） |
| **S1-C** | 命令化边界：定义 **AI 可写域白名单**（scene/light/env/postfx/text/ui/anim 参数），协议层对白名单外域不暴露 `tx.apply`；新增 `.write(` 写点台账与门禁（见 §5） | `docs/WRITE_SITES.md`（新增，写点基线 + 白名单标注 tx 覆盖状态）+ `xtask/src/doc_gate.rs`（新增第 7 条规则：写点只增于白名单或显式登记） | 台账生成可复现；`doc_gate` 第 7 条对新增未登记写点报错 |
| **S1-D** | `r3e-plugin` 重写为只依赖 `protocol`；能力声明加 `scene.write` / `doc.transaction` | `crates/r3e-plugin/Cargo.toml` + `src/**`；`dep_debt.json` 偿还 `plugin → core` / `plugin → physics` 两条 | `dep-check --strict` 违规从 4 条降至 2 条；**替代清单先行**（见 §7 风险 R2） |

**S1 执行结果（2026-11-16 收口）**：S1-A ✅ 协议面五方法已上线（RPC 39 → 44、命令表 369 → 374、`dispatch/tx.rs` 102 行）；S1-B ✅ 事务态挂在 `Document` 上（`r3e-doc/src/tx.rs` 277 + `r3e-editor-host/src/api/tx.rs` 269），跨 RPC 存活、`tx.preview` 三哈希、commit 前主场景零字节变动；S1-C ✅ `docs/WRITE_SITES.md` + `doc_gate` 第 7 条写点台账（5 crate / 226 处）；**S1-D ⏸ 顺延**——R2 前置（替代清单）已按决策补齐为 `docs/M1_S1D_SUBSTITUTION.md`，但四项离线能力的协议承接方（`query.*` / 计算类 RPC）尚未就位，本轮不动 `Cargo.toml` 与 `CAPABILITIES`，`dep-check` 仍 4 条债（`limit=4`）。单测 1,476 → 1,504，`cargo xtask verify` 十项全绿。

**S1 附带（同一 Sprint 内，独立 commit）**：`M1-DOC-FIELDS` 的**文件拆分**部分——`scene.rs` 4,057 行按域拆到 `scene/` 子模块，行为零变化、无 API 变化。字段私有化见 §4 决策 D2。

### M1-S2 语义查询（3 卡）

| 卡 | 内容 | 验收 |
|---|---|---|
| S2-A | `WorldQuery` 执行器：`kind` / `tag` / `bbox` / `component` 四类过滤 + 投影字段 `{id,name,tags,pos,interact_radius}` | 单测覆盖四类过滤与组合；对现有测试场景可返回全量对象 |
| S2-B | 协议面 `query.*`（`query.find` / `query.describe` / `query.stats`），复用 S1 的事务只读语义（查询不产生写点） | RPC 可调用；查询后 `StateHash` 不变 |
| S2-C | `tags` / `interact_radius` 字段落地：`node.rs` / `scene.rs` / schema / `state_hash` **四处同改**，且 `node_desc` 必须同步（漏改会静默丢字段） | 新增字段参与哈希；`node_desc` 输出含新字段；三个问答用例（"场景里有哪些可交互物"等）全过 |

> **执行顺序调整**：S2 实际按 **C → A → B** 推进。理由：`tags` / `interact_radius` 是 S2-A `WorldQuery` 投影字段（`{id,name,tags,pos,interact_radius}`）与 S2-B `query.*` DTO 的**前置**，字段不先落地上层无物可投影；S1-D 的解绑顺序也依赖 `query.*`，字段越早稳定越好。

**S2-C 执行结果（2026-11-16）**：四处同改全部落地——① `r3e-core/src/node.rs` 的 `Node` 增 `tags: Vec<String>` / `interact_radius: Option<f32>`；② `r3e-core/src/scene.rs` 的 `ObjectDesc` 同名增参，`spawn` 入口经 `normalize_tags` / `normalize_interact_radius` 收敛（去空白、去重、截断 `MAX_NODE_TAGS` / `NODE_TAG_MAX_LEN`、半径非法值归 `None`），`node_desc`（`to_scene_file` 回写）仅在非空时输出，roundtrip 不增生字段；③ `assets/scene.schema.json` 重新生成（43,796 字节）；④ `r3e-doc/src/state_hash.rs` 的 `mix_node` 按「条数 + 逐条」混 `tags`、按「判别位 + 值」混 `interact_radius`，语义不同不碰撞。配套：`r3e-core/tests/node_tags.rs` 9 条（收敛/上限/半径语义/`node_desc` 携带/roundtrip 不增字段）、`r3e-doc/tests/state_hash.rs` 覆盖守卫增 2 条扰动，R3「静默丢字段」风险已用断言封住。附带把 `scene.rs` 的 `ObjectDesc` / `PhysicsDesc` / `PhysicsSceneDesc` 拆到 `scene_desc.rs`（`scene.rs` 4,057 → 3,972 行，`size_debt` 基线同步）。单测 1,504 → 1,513，`cargo xtask verify` 十项全绿。

**S2-A 执行结果（2026-11-17）**：`WorldQuery` 执行器落地在 `r3e-core/src/query.rs`（443 行，`lib.rs` 已 `pub use`）。① 四类过滤：`NodeKind`（`any` / `prop` / `interactable` / `animated` / `skinned` / `morphing` / `character` / `physical`，判据分别是「可见且非可交互/非角色/无物理」「`interact_radius.is_some()`」「被 `AnimInstance.bindings` 指向」「`SkinInstance.mesh_node` 指向」「`MorphInstance.mesh_node` 指向」「蒙皮或形变」「`physics.is_some() || body.is_some()`」）；标签 `all_tags`（AND）/ `any_tags`（OR，空不约束）；`region: Aabb` 按**世界原点**含边界判定；`Component` 九位（material / physics / interact / animated / skinned / morphing / parent / child / tagged）分 `require`（全需）与 `exclude`（有就排除，优先级高于 `require`）。② 投影 `NodeView {id, name, tags, position, interact_radius}`，`position` 取 `world_position()`（子节点是世界坐标）；`limit = 0` 为不限，`DEFAULT_LIMIT = 64` 供 S2-B 做协议缺省。③ 查询**只读**：不建事务、不产生写点、不动 `StateHash`，S2-B 可直接套只读事务语义。④ 配套 `Scene::query_stats()`（总数/可见数/可交互数/带标签数 + 各类别计数 + 标签出现次数），与 `query` 共用同一套判据，避免「stats 说有 3 个、find 只查到 2 个」；`Facts` 在单次查询内缓存动画/蒙皮/形变索引，不逐节点线性扫场景级列表。⑤ 标签匹配定为**字面比较、仅忽略首尾空白、大小写不折叠**，与 `normalize_tags`「`Door` 与 `door` 是两个标签」的契约一致（同一文件两处注释曾口径不一，已统一）。配套 `r3e-core/tests/world_query.rs` 24 条（八类判据、标签 AND/OR、区域含边界与组合、组件位、排除优先、插入顺序与 `limit` 截尾、五字段投影与子节点世界坐标、查询只读、`stats` 与 `query` 逐类一致、两个枚举的字符串面 roundtrip）。单测 1,513 → 1,537（`xtask/tests_baseline.json` 同步），`cargo xtask verify` 十项全绿。

### M1-S3 意图命令 + 变更半径（3 卡）

| 卡 | 内容 | 验收 |
|---|---|---|
| S3-A | 意图命令框架：一次意图 = N 条原子命令，包进单个 `Transaction` | 意图内一条失败 → 整体 rollback，`StateHash` 回到意图前 |
| S3-B | `spawn_prefab` / `place_light` | 两者 undo 一次全退 |
| S3-C | `paint_terrain`（数据壳）+ `impact` 输出（受影响对象/材质清单） | 一条意图的 `impact` 能列出受影响对象与材质；`paint_terrain` 不生成几何（M2） |

### M1-S4 内容验证器（2 卡）

| 卡 | 内容 | 验收 |
|---|---|---|
| S4-A | 五类检查：可通行性（导航连通性）、悬空（无支撑）、穿模（AABB/凸包侵入）、材质缺失、引用断裂 | 故意生成不可达关卡 → 报出**具体失败点位坐标**，不是布尔值 |
| S4-B | 验证器接入 `verify` 链与 `impact` 报告 | 失败项可定位到对象 id + 坐标 + 修复建议 |

### M1-S5 CLI + 三范式（3 卡）

| 卡 | 内容 | 验收 |
|---|---|---|
| S5-A | 新 crate `r3e-cli`（只依赖 `protocol`）：`tx` / `query` / `verify` / `replay` 四个子命令 | 纯 CLI 可完成一次完整事务 |
| S5-B | 三个从零可复现范式脚本（生成可通行小场景 / 验证 / 失败回滚重试） | 三条命令可复制粘贴跑通 |
| S5-C | **Gate 1 演练**：不改引擎代码，仅用 `r3e-cli` 完成"生成 → 验证 → 失败回滚 → 成功提交" | Gate 1 通过并写入 `AI_ROADMAP.md` §8 验收表 |

---

## 4. 需要拍板的三个决策

| 编号 | 决策 | 选项 | 推荐 |
|---|---|---|---|
| **D1** | S1 强制层放在哪 | A. 只收口 RPC 面（AI 入口），进程内 script/runtime 保留直写 + 台账<br>B. 全量收口到命令层 | **A**。B 会碰 224 写点、破坏零行为变化，等于把 M1 变成 M0 重做（C3） |
| **D2** | `Scene` 40 个 pub 字段私有化放哪 | A. M1 内做（配 getter/setter，可能破外部 API）<br>B. M1 只拆文件，私有化推 M2 | **B**。私有化会冲击全仓读点，与 S1/S2 并行风险过高；拆文件已解决"读不全"这个真实痛点 |
| **D3** | `paint_terrain` 做到什么深度 | A. 数据壳（高度图数据 + undo + impact）<br>B. 真实网格生成 + LOD | **A**。几何求值属渲染侧大工程，M1 的目标是闭环而非画质 |

---

## 5. 写点台账与门禁（S1-C 细则）

`docs/WRITE_SITES.md` 内容结构：

```
基线：224 处 .write(（2026-11-16）
按 crate 计数表（script 130 / runtime 11 / editor-host 16 / doc 内部 其余）
逐条标注：{路径:行, 所属域, 是否 AI 可写域, tx 覆盖状态: 已覆盖|排除|待办}
```

`doc_gate` 第 7 条规则（**登记式**，替代原设想的"禁直改 Scene"静态扫描——那在现有纯文本扫描机制下不可实现）：

- 统计全仓 `.write(` 数量与逐文件分布
- 与 `WRITE_SITES.md` 基线比对：**只允许减少，不允许新增未登记写点**
- 新增写点必须在台账中显式登记并标注 tx 覆盖状态

这条规则的价值：把"绕过协议写入"从"不可见"变成"审计可见"，成本极低、无行为风险。

---

## 6. 每卡执行模板（四步，不可跳跃）

1. **代码 + 测试**：算法单测在 `r3e-doc`/`r3e-core`，布局/上传/着色器一致性合约在 `r3e-render`（若涉及），协议面变更配一致性测试
2. **质量门**：`cargo fmt --check` → `cargo clippy --workspace --all-features -- -D warnings` → `cargo test --workspace --all-features --features rapier` → `cargo xtask verify`（串行 10 项，含 replay seed42 frames600 同哈希）
3. **文档同频**（见 §8）
4. **commit**：`feat(m1-sN): <卡名>`，本地 ahead 不推送

---

## 7. 风险台账（执行前已识别）

| 编号 | 风险 | 影响 | 处置 |
|---|---|---|---|
| **R1** | `doc.write` 是**借用收口**而非**命令收口**：走 `write` 闭包可绕过命令层，不报错但不可 undo | AI 误用后场景不可撤销 | S1-C 白名单 + `doc_gate` 第 7 条台账；`tx.*` 协议面不暴露裸 `write` |
| **R2** | S1-D 插件只依赖 `protocol` 会砍掉 4 项离线能力（`scene_inspect` / `validate` / `mesh_stats` / `physics_simulate`） | 插件侧功能回退 | **替代清单前置**：先在 `r3e-cli`（S5-A 提前交付最小版）或 `query.*`（S2）里补齐，再削插件依赖 |
| **R3** | `tags` / `interact_radius` 新增若漏改 `node_desc` | 字段静默丢失，查询结果对不上 | S2-C 四处同改 + 单测断言 `node_desc` 输出 |
| **R4** | 事务态复制到宿主侧后，与 `doc` 内 `StateHash` 出现双源 | `commit` 前哈希不变校验失真 | S1-B 的 `base_state_hash` 只在 `begin` 时取一次，`commit` 后强制重取并比对，不一致即报错 |
| **R5** | `scene.rs` 拆文件期间，其他卡的改动打进同一文件 | 冲突与返工 | S1 附带卡**独立 commit 且优先执行**，拆完再动 S2 |

---

## 8. 文档对齐清单（每卡第 3 步照此执行）

| 文档 | 何时改 | 改什么 |
|---|---|---|
| `docs/AI_ROADMAP.md` | 每 Sprint 收口 | §3.2 交付物表勾选 + §8 验收表 + 状态行 |
| `docs/ARCHITECTURE.md` | 协议面/真值层变更时 | 核心契约、协议方法表、探针字段、行数增量表 |
| `docs/WRITE_SITES.md` | S1-C 起，每次写点变化 | 写点基线、逐条覆盖状态 |
| `docs/capabilities.json` | 协议/命令变化时 | `cargo xtask gen-capabilities` 自动生成，不手写 |
| `docs/M1S1_*.md` … `M1S5_*.md` | 每 Sprint 完成 | 新增，格式对齐 `M0S1`–`M0S5` 既有文档 |
| `README.md` / `docs/ASSETS.md` | 能力面变化时 | CLI 用法、能力清单 |

---

## 9. 执行顺序总览

```
S1-A 协议面 ──> S1-B 宿主事务态 ──> S1-C 白名单+台账 ──> S1-D 插件重写
                                                          │
        M1-DOC-FIELDS(拆文件, 独立) ──────────────────────┘
                                                          ↓
S2-A/B/C 查询面 ──> S3-A/B/C 意图命令 ──> S4-A/B 验证器 ──> S5-A/B/C CLI ──> Gate 1
```

第一个动作：**S1-A**（协议面五个方法），它是全部后续卡的前置，且不触碰任何已有写点，风险最低。
*（内容由AI生成，仅供参考）*
