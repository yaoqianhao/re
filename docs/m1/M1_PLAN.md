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
| **S1-C** | 命令化边界：定义 **AI 可写域白名单**（scene/light/env/postfx/text/ui/anim 参数），协议层对白名单外域不暴露 `tx.apply`；新增 `.write(` 写点台账与门禁（见 §5） | `docs/m1/M1_PLAN.md` §5（写点台账本体 + 白名单标注 tx 覆盖状态）+ `xtask/src/doc_gate.rs`（新增第 7 条规则：写点只增于白名单或显式登记） | 台账生成可复现；`doc_gate` 第 7 条对新增未登记写点报错 |
| **S1-D** | `r3e-plugin` 重写为只依赖 `protocol`；四条只读 RPC 上协议面承接原离线能力 | `crates/r3e-plugin/Cargo.toml` + `src/**`；`r3e-protocol/src/{inspect.rs,dispatch/inspect.rs}`；`r3e-editor-host/src/{inspect/,api/inspect.rs}`；`dep_debt.json` 偿还 `plugin → core` / `plugin → physics` 两条 | `dep-check --strict` 违规从 4 条降至 2 条 ✅ **已达成（2026-11-19，方案 A）** |

**S1 执行结果（2026-11-16 收口）**：S1-A ✅ 协议面五方法已上线（RPC 39 → 44、命令表 369 → 374、`dispatch/tx.rs` 102 行）；S1-B ✅ 事务态挂在 `Document` 上（`r3e-doc/src/tx.rs` 277 + `r3e-editor-host/src/api/tx.rs` 269），跨 RPC 存活、`tx.preview` 三哈希、commit 前主场景零字节变动；S1-C ✅ `docs/m1/M1_PLAN.md` §5 写点台账 + `doc_gate` 第 7 条（5 crate / 226 处）；**S1-D ⏸ 顺延**——R2 前置（替代清单）已按决策补齐为 `docs/m1/M1_S1D_SUBSTITUTION.md`，但四项离线能力的协议承接方（`query.*` / 计算类 RPC）尚未就位，本轮不动 `Cargo.toml` 与 `CAPABILITIES`，`dep-check` 仍 4 条债（`limit=4`）。单测 1,476 → 1,504，`cargo xtask verify` 十项全绿。

**S1-D 结清（2026-11-19，方案 A）**：`scene.snapshot` / `mesh.stats` / `scene.validate` / `physics.simulate` 四条只读 RPC 四层同改上线（协议 58 → 62、命令表 388 → 392、不变量 30 → 38）；计算落 `r3e-editor-host/src/inspect/`（`locate.rs` 179 行由插件整体迁入）；`r3e-plugin` 的 `Cargo.toml` 只剩 `serde` / `serde_json`，`rapier` feature 随依赖删除；四项工具改薄壳，**代价是引擎必须在线**（连不上回 -32011）。`dep_debt.json` 4 → 2 条（`limit` 4 → 2），`cargo xtask verify` 十项全绿，单测 1,817（`r3e-plugin` 47 持平：删掉本地计算用例后补 6 个薄壳用例）。详见 `docs/m1/M1_S1D_SUBSTITUTION.md`。
**能力名裁决（2026-11-19，同日闭合 S1-D 最后一项）**：`scene.write` / `doc.transaction` 从 r3e 侧撤销——ai_ide v1.0 能力表 7 项均绑定真实 `host/*` 方法，无 scene / doc 槽位，硬加只会得到勾了不管用的死能力名；写侧改由 `risk=write` + `path_field` 承载。裁决与实证见 `docs/m1/M1_S1D_SUBSTITUTION.md` §8，给 ai_ide 的扩展提案见 `docs/m2/M2_PROPOSAL_CAPABILITY_SLOTS.md`。

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

**S2-B 执行结果（2026-11-17）**：查询协议面三条方法落地，链路自下而上四层。① `r3e-core/src/query.rs` 增 `Scene::describe(id) -> Option<NodeDetail>` 与 `NodeDetail {view, components, kinds}`，与 `query` / `query_stats` **共用同一份 `Facts`**，因此「`find` 查得到的组件位」与「`describe` 报出来的组件位」不会打架；`kinds` 跳过恒真的 `any`（列进详情没有信息量）。② `r3e-protocol`：新增 `src/query.rs` 只放 DTO `QuerySpec`（字段名与 `r3e_core::Query` 一一对应，JSON 驼峰），**名字裁决与 `limit` 缺省都不在协议层做**——类别 / 组件位字符串原样透传，`limit: None` 一路传到宿主才取 `DEFAULT_LIMIT`，避免「加一个类别要改两处」「默认条数出现第二个出处」；`src/dispatch/query.rs` 承载三个臂，`lib.rs` 的 `RPC_METHODS` 加三条，`api.rs` 的 `EngineApi` 加三个默认 `unsupported` 方法。③ `r3e-editor-host/src/api/query.rs`：`query_find` 向 `Scene::query` 要 `limit + 1` 条只为如实回报 `truncated`（多的那条不返回），回包含 `count / limit / truncated / unfiltered / nodes`，`unfiltered` 用来区分「没给条件」与「过滤后无命中」；`query_describe` 名字不存在回 `NOT_FOUND` 而非空对象；`query_stats` 透传普查并把 `by_kind` 的枚举键转字符串。拼错的类别名 / 组件位名一律 `INVALID_PARAMS` 且**在错误信息里列出合法取值**——不能让 AI 把「名字打错了」误读成「场景里没有」。④ `cmd_spec/meta.rs` 登记三条 `Domain::Meta`（`reversible: true`，`find` / `stats` 为 `Cost::Linear`、`describe` 为 `Cost::Cheap`，`since: "M1-S2-B"`），四处同改齐（`RPC_METHODS` + dispatch 臂 + cmd_spec + `capabilities.json` 重生成，`commands 377 / rpc 47 / script_only 330`）。配套三处契约测试跟上：`r3e-editor` / `r3e-editor-host` 的 `cmd_spec_contract` 下限 44 → 47 并纳入 `query.rs` 扫描，`r3e-protocol/tests/rpc_contract.rs` 的 `SOURCES` 补 `dispatch/query.rs`（S1 踩坑「加 RPC 不补 dispatch / 不重生成清单」本次先补后跑）。新增 `r3e-editor-host/src/api/tests/query.rs` 9 条：缺省全量（`limit` 取 `DEFAULT_LIMIT`、`unfiltered` 为真）、标签与组件位组合、区域过滤（含**反序对角点**归一化）、截断回报、拼错名字回参数错误、`describe` 投影与组件位、`describe` 不存在、`stats` 全貌（不命中的类别不出现）、**三条方法前后 `StateHash` 与写次数逐位不变**。单测 1,537 → 1,546，`cargo xtask verify` 十项全绿。

**S3-A 执行结果（2026-11-17）**：意图命令框架四层落地，「一次意图 = N 条原子命令、整笔事务一格撤销」。① **命名裁决（本轮踩坑，写在最前）**：方法名定为 `tx.intent`，**不能叫 `tx.apply_intent`**——`dispatch::canonical()` 归一化会删掉下划线，请求端的 `tx.apply_intent` 被归一成 `tx.applyintent`，dispatch 臂永远匹配不上、只能落到 `METHOD_NOT_FOUND`；`cmd_spec` 的 `rpc名唯一且是点分小写` 也会直接红。故 `RPC_METHODS` 里的名字必须是「归一后形态」（仅小写字母、数字、点），Rust 侧方法名仍是 `tx_intent`，`tx_contract` 的 `rpc_of` 按 `tx_` 前缀折成点分名同样成立。铁律已写进 `src/lib.rs` 的 `RPC_METHODS` 文档注释。② `r3e-protocol`：`RPC_METHODS` 加一条（47 → 48，字典序插在 `tx.commit` 与 `tx.preview` 之间）、`dispatch/tx.rs` 增臂（`intent` 必填、必须点分非空，`args` 缺省补 `null`）、`api.rs` 的 `EngineApi` 加默认 `unsupported` 方法。③ `r3e-doc/src/tx.rs` 的 `TxSession::tx_apply_intent`：意图展开出的 N 条命令进同一条影子事务，`staged` 涨 N 而 `intents` 涨 1；提交/撤销以**事务**为单位，一次 `undo` 全退（验收「意图内一条失败 → 整体不落地」由文档层按命令逆序回退 + 事务可回滚兜住）。④ `r3e-editor-host/src/api/tx.rs`：`INTENTS = ["scene.spawn_prefab"]` 白名单（未纳入即 `TX_REJECTED` 且事务保留、可继续用）、`IntentPlan::parse` 做参数整形（`nodes` 缺失 / 空数组 / 类型错 → `INVALID_PARAMS`，上限 256）、`build` 做**影子之外的预检**（意图内重名、与场景已有物件重名、父物件须在本意图前面或已在场景），这些错若留到 apply 才炸就是「前 k 条已改影子再回退」，白跑一次回退。⑤ `cmd_spec/meta.rs` 登记 `Domain::Meta`（`reversible: true`、`Cost::Linear`、`since: "M1-S3-A"`），四处同改齐（`RPC_METHODS` + dispatch 臂 + cmd_spec + `capabilities.json` 重生成，`commands 378 / rpc 48 / script_only 330`）。契约同步：`rpc_contract.rs` 的 `SOURCES` 无需增文件（臂在既有 `dispatch/tx.rs`），`cmd_spec_contract` 的宿主方法数断言 47 → 48（含 `r3e-editor` 与 `r3e-editor-host` 两处，`tx_intent` 按 `tx_` 前缀折成 `tx.intent` 命中表内条目）。新增测试 18 条：`r3e-protocol/tests/tx_contract.rs` 5 条（缺 `intent`、空白/非点分 `intent`、`intent`+`args` 透传、`args` 缺省为 `null`、缺 `txId`）、`r3e-doc/tests/tx.rs` 5 条（意图记账口径、提交后落地且一次 undo 全退、意图复用既有事务、事务内多意图累加）、`r3e-editor-host/src/api/tests/tx.rs` 8 条（`staged` 按命令数 / `intents` 按意图数、未白名单报 `TX_REJECTED` 且事务保留、`nodes` 缺失/空/类型错、超 256 上限、意图内重名与场景已有重名、父子引用规则、`txId` 不匹配报 `TX_NOT_FOUND`）。单测 1,546 → 1,564（`xtask/tests_baseline.json` 同步），`cargo xtask verify` 十项全绿。

**S3-B 执行结果（2026-11-17）**：第二条意图 `scene.place_light` 落地，与 S3-A 的 `scene.spawn_prefab` 一起把「一批东西 = 一条意图 = 一格撤销」从节点扩到灯光。① **白名单是宿主常量，加意图零协议改动**——S3-A 把意图名挡在协议层之外的裁决在这里兑现：`RPC_METHODS` 仍是 48 条、`capabilities.json` 仍是 `commands 378 / rpc 48 / script_only 330`、协议层零行改动，只在 `r3e-editor-host/src/api/tx.rs` 的 `INTENTS` 追加一项、`IntentPlan` 加 `PlaceLight(Vec<LocalLight>)`。② **参数整形**（`IntentPlan::parse`）：`lights` 缺失 / 非数组 / 空数组 → `INVALID_PARAMS`；上限 `PLACE_LIGHT_MAX_LIGHTS = MAX_LOCAL_LIGHTS`（直接借光源表容量 16，不另立数字，避免「意图上限」与「场景容量」两处出处）；每项按 `LocalLight` 反序列化（字段名与场景 JSON 严格同名：`key` / `kind` / `pos` / `color` / `intensity` / `range` / `dir` / `inner_deg` / `outer_deg` / `cast_shadow` / `shadow_bias`），`key` 空则报 `INVALID_PARAMS`——撤销要按 key 找回这盏灯，没有 key 就没有可逆性。③ **影子之外的三重预检**（`IntentPlan::build`，全部 `TX_REJECTED` 且事务保留）：同批 `key` 重复、与场景已有灯光重 key（`place_light` 只新建不覆盖，要改参数先删再放）、剩余空位不够（`MAX_LOCAL_LIGHTS - lights.len()`）。这三类若留到 apply 才炸，就是「前 k 盏已写影子再逆序回退」，白跑一次回退。④ **展开与撤销**：每盏灯一条 `CommandKind::upsert_local_light(&scene, light)`（预检保证 `old = None`），提交后 N 条命令只占 **1 格**撤销；`new_local_light_undo_deletes_it` 已守「`old` 为 `None` 时 undo 删灯」这一撤销语义底座，故一次 `undo` 把整批灯全退且 `lights` 回到意图前，`StateHash` 随之复位。⑤ 配套 3 条宿主测试：按灯数记账（`commands` 2 / `staged` 2 / `intents` 1）且暂存期不碰主场景、提交后 2 盏落地只占 1 格撤销、一次 `undo` 全退；参数整形五类错误码；重 key / 场景已有 key / 表满三重拒绝后 `staged` 仍为 0、`lights` 与 `undo_depth` 不变。单测 1,564 → 1,567（`xtask/tests_baseline.json` 同步：`r3e-editor-host 75 → 78`），`cargo xtask verify` 十项全绿。

**S3-C 执行结果（2026-11-17）**：地形数据壳与 `impact` 清单落地，S3 三卡全部收口。① `r3e-core/src/terrain.rs`（238 行）：`TerrainPatch` 七字段（`name` / `origin` / `size` / `resolution` / `albedo` / `roughness` / `heights`），字段用 `[f32; N]` 数组不用 glam 类型、与 `LocalLight` / `UiElement` 同约定，场景 JSON 严格对称；`sanitized` 幂等收敛（名字截断 64、尺寸保正到 `TERRAIN_MIN_SIZE`、分辨率夹到 `1..=TERRAIN_MAX_RESOLUTION(128)`、`heights` 长度对齐 `resolution.0 * resolution.1` 且非有限值归零，空 `heights` 表达「平坦地面」）；表语义与光源表对齐但**容量只拦新增**——`upsert_patch` 同名整体覆盖、上限 `TERRAIN_MAX_PATCHES = 32`，脚本每帧重发同一块地必须写进去。`scene.rs` 的光源/地表两张表操作抽到 `scene_tables.rs`（70 行），`scene.rs` 3,972 → 3,947 行（`xtask/size_debt.json` 基线同步下调）。② `r3e-doc/src/state_hash.rs` 增 `mix_terrain`：混「块数 + 每块内容」，高度样本**逐点入哈希**——`paint_terrain` 只改一个顶点也必须让差分哈希看见，否则「改了地形但 `StateHash` 没变」会让 `tx` 的冲突检测失真。③ `r3e-doc/src/command/terrain.rs`（44 行）：`CommandKind::upsert_terrain_patch` 取同名旧块前像（无则 `None`），`write_patch` 把「名字为空」与「表满且是新名字」报成 `DocError::Rejected` 而**不是「命令显示成功但场景没变」**，`remove_patch` 按名找不到报 `NotFound`。④ 宿主侧 `scene.paint_terrain` 入白名单（`INTENTS` 追加一项、`IntentPlan::PaintTerrain`），`capabilities.json` 与 `RPC_METHODS` **零改动**（意图名挡在协议层之外的裁决第三次兑现：`commands 378 / rpc 48 / script_only 330`）。参数整形：`patches` 缺失 / 非数组 / 空数组 / 超 `TERRAIN_MAX_PATCHES` / 某项 `name` 空 → `INVALID_PARAMS`。影子之外的预检与 `place_light` **关键差别**：`paint` 的语义就是覆盖，**同名重刷是正事**（放行），只有「同批重名」与「新名字块数超过剩余空位」才 `TX_REJECTED`。⑤ `impact` 输出：`tx.intent` 回包新增 `impact { objects, lights, terrain, materials }` 四字段，**恒定存在**（空也输出空数组，结构稳定才好解析），不进 `StateHash`、不入撤销栈——AI 拿它判断「刚才到底改到哪儿」，省掉为确认再发一串 `query.*`；`materials` 按 `{ target, params }` 记：物体材质只列**与缺省不同**的参数（`metallic` / `roughness` / `emissive`，否则「没改」与「改成缺省值」分不清），地表材质恒列 `albedo` + `roughness`（这两项总是显式写入，不做缺省比较）。⑥ `assets/scene.schema.json` 重生成 43,796 → 46,898 字节（新增 `terrain` 段）。写点台账 226 → 228（`r3e-doc` 58 → 60，两处新增都在 `tests/command.rs` 的夹具）。⑦ 配套测试 7 条：`r3e-doc/tests/command.rs` 4 条（新建块 `undo` 删块、覆盖块 `undo` 复原旧块、命令标签与目标名、表满新增被拒）、`r3e-editor-host/src/api/tests/tx.rs` 3 条（按块数记账并回带 `impact`、参数整形错误码、同批重名与空位不足都拒绝而**同名覆盖放行**）。单测 1,567 → 1,574（`xtask/tests_baseline.json` 同步：`r3e-doc 114 → 118`、`r3e-editor-host 78 → 81`），`cargo xtask verify` 十项全绿。

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

> 本节是 `cargo xtask doc-gate` **规则 7 的唯一真源**。原 `docs/WRITE_SITES.md` 已于
> M1-S4-E 并入本节（内容一字未丢，少一个「代码改了、台账文件没跟」的漂移点）。
> 规则 7 只解析下面那对 `write-sites` 注释锚点（begin / end）包住的表格，
> 机读字段只有前两列：`| crate 目录名 | 登记写点数 |`；锚点本身全文件只允许出现一次。

<!-- write-sites:begin -->

| crate | 登记写点数 | 说明 |
| --- | --- | --- |
| r3e-doc | 63 | **合法收口本体**：`Document` 自己执行命令、`#[cfg(test)]` 内的夹具构造。这里的数字跟着新测试涨，收口性质不变，登记数可随测试增长而更新（M1-S3-C 地形命令测试 +2：`tests/command.rs` 的 `paint` 与一次 undo 全退两处夹具；M1-S4-A 材质校验契约 +1：`tests/validate_material.rs` 的「引用与内联材质冲突」夹具绕过命令层直写 `Node.material`，故意造非法态；M2-B04 角色不变量 +1：`tests/character_issues.rs` 的 `doc_with_character` 夹具借 `doc.write` 直写 `Node::character`，故意绕开 `character.create` 命令层——要测的正是「已经写进场景的非法角色配置会不会被静态层抓住」，走命令层就先被参数校验挡住了） |
| r3e-script | 138 | 脚本宿主（`.rhai` 绑定）。绝大多数是「脚本直接改场景」——M1-S3 意图命令要收的对象；眼下是最大单笔债。M2-B01 +1：`api/character.rs` 的 `write_spec` 借 `ctx.write` 写 `Node::character`（同 `physics` 域口径——脚本侧配置直写场景，等意图命令收口时一并归并） |
| r3e-editor-host | 22 | 编辑器宿主写路径（新建节点、属性面板、播放态回写）。M1-S1 之后新写的必须走 `tx_*` 或命令层。M1-S4-D +3：`physics_nav.rs` 的 `materialRef` 反查夹具一处、nav 测试的两处地面 spawn 夹具两处，均属 `#[cfg(test)]` 夹具（故意绕过命令层直接造态）。原先 `src/api/material.rs` 的第四处写点（`scene.material.upsert` 直写材质表：推版本/置脏/计写次但不入撤销栈）已于 M1-S4-E 前置补债清除——该 RPC 两条路都改走命令层（无事务 `Document::apply`、有事务 `tx_apply`），于是这里 22 → 21。M1-S4-F +1：`src/api/tests/verify.rs` 的「物理世界与场景不同步」夹具借 `doc.write` 直造一个带 `physics` 段的节点（世界侧故意不装），属 `#[cfg(test)]` 夹具 |
| r3e-runtime | 11 | 运行时（物理回写 / 动画采样）。**结构性不可映射**：这些写点由每帧计算驱动，没有对应的用户意图，不上撤销栈是对的，登记在案即可 |
| r3e-replay | 2 | 回放装载。走 `replace_scene` 之外的少量补写，M1-S2 语义查询若覆盖可再降 |

合计 **236** 处（= 上表五列登记数之和，`doc-gate` 打印值一致；M1-S4-E 前登记 234 处，材质 `upsert` 改走命令层后 -1，M1-S4-F 夹具 +1 回到 234，M2-B01 脚本侧角色配置写点 +1 到 235，M2-B04 角色不变量夹具 +1 到 236）。

<!-- write-sites:end -->

### 5.1 这张表在管什么

`Document::write(|s| ...)` 是文档层留给上层的**借用收口**：它保证「写场景」这件事只有
一个入口，但它**不保证写进撤销栈**。绕过命令层直接借 `&mut Scene` 改状态，能改成功、
不报错，只是：

- 不进 undo（用户 Ctrl+Z 撤不掉）；
- 不进事务影子（M1-S1 之后，事务期间的这类写入会在 commit 时被判 `Conflict`）；
- 不进 `StateHash` 的"可解释变更"序列（外部看不到谁改的）。

所以它不是错误，是**债**。台账的作用是把债的规模钉住：谁再加一处，评审必须看见。

### 5.2 规则 7 的判定口径

`doc-gate` 规则 7 逐 crate 统计 `crates/<crate>/**/*.rs` 里 `.write(` 的字面出现次数
（只数方法调用，不区分是否在 `#[cfg(test)]` 块内——测试也要收口）：

| 情况 | 结果 |
| --- | --- |
| 实际 > 登记 | **失败**：新增写点必须先在 §5 表格里登记并说明理由 |
| 实际 = 登记 | 通过 |
| 实际 < 登记 | 通过 + 打印「可下调」提示（只许减，减了顺手把数字改小） |

新增 crate 出现 `.write(` 但表里没有该行 → 失败（同"实际 > 登记"，登记数为 0）。
锚点缺失或表格解析为空 → 同样**失败**（宁可红，也不让门禁静默退化成「没有台账」）。

- 统计全仓 `.write(` 数量与逐文件分布
- 与本节表格比对：**只允许减少，不允许新增未登记写点**
- 新增写点必须在表中显式登记并标注 tx 覆盖状态

这条规则的价值：把"绕过协议写入"从"不可见"变成"审计可见"，成本极低、无行为风险。

### 5.3 降债路线

| 阶段 | 动作 | 预期 |
| --- | --- | --- |
| M1-S1 | 事务五条 RPC 上线，宿主写路径优先走 `tx_*` | 本表先钉住，不加 |
| M1-S3 | 意图命令（`scene.spawn` 等）在 `r3e-script` 建立「意图 → `CommandKind`」映射 | `r3e-script` 明显下降 |
| M1-S4 | 三层验证器上线，未入账写点可被探针发现 | 结构性写点（runtime）单列「豁免但可见」 |
| M2+ | `r3e-runtime` 物理/动画回写走「隐式命令」通道 | 从台账转入正式通道 |

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
| **R2** | S1-D 插件只依赖 `protocol` 会砍掉 4 项离线能力（`scene_inspect` / `validate` / `mesh_stats` / `physics_simulate`） | 插件侧功能回退 | ✅ **已消解（2026-11-19）**：方案 A 自建承接方——四条只读 RPC 落协议面、计算搬 `r3e-editor-host`，四项工具改薄壳后行为等价；离线能力改为「引擎在线即可用」，见 `docs/m1/M1_S1D_SUBSTITUTION.md` |
| **R3** | `tags` / `interact_radius` 新增若漏改 `node_desc` | 字段静默丢失，查询结果对不上 | S2-C 四处同改 + 单测断言 `node_desc` 输出 |
| **R4** | 事务态复制到宿主侧后，与 `doc` 内 `StateHash` 出现双源 | `commit` 前哈希不变校验失真 | S1-B 的 `base_state_hash` 只在 `begin` 时取一次，`commit` 后强制重取并比对，不一致即报错 |
| **R5** | `scene.rs` 拆文件期间，其他卡的改动打进同一文件 | 冲突与返工 | S1 附带卡**独立 commit 且优先执行**，拆完再动 S2 |

---

## 8. 文档对齐清单（每卡第 3 步照此执行）

| 文档 | 何时改 | 改什么 |
|---|---|---|
| `docs/AI_ROADMAP.md` | 每 Sprint 收口 | §3.2 交付物表勾选 + §8 验收表 + 状态行 |
| `docs/ARCHITECTURE.md` | 协议面/真值层变更时 | 核心契约、协议方法表、探针字段、行数增量表 |
| `docs/m1/M1_PLAN.md` §5 | S1-C 起，每次写点变化 | 写点台账（表格本体 + 登记数 + 逐条覆盖状态，M1-S4-E 起不再单列文件） |
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
