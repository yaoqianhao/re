# M1-S1-D 替代清单与结清记录（插件去 core/physics 依赖）

- 立项：2026-11-16（M1-S1 收尾，当时判定「本轮不还」）
- 结清：2026-11-19（采用**方案 A**：插件改薄壳 + 4 条只读 RPC 上协议面）
- 对应卡：`docs/m1/M1_PLAN.md` §3 M1-S1-D
- 结论一句话：**`plugin → core` / `plugin → physics` 两条债已偿还**，`xtask/dep_debt.json` 由 4 条降为 **2 条**（`limit` 4 → 2）。原判「4 项离线能力无协议承接方」不再成立——不是找到了替代清单，而是**把承接方造了出来**：四条只读 RPC 进协议面，计算落到 `r3e-editor-host`。

## 1. 原判为什么当时是对的，现在为什么不成立

立项时的实证结论是：`scene_inspect` / `mesh_stats` / `scene_validate` / `physics_simulate` 四项在现有协议面上**都没有等价承接方**——`query.find` / `query.describe` 只回 `{id,name,tags,pos,interact_radius}` 五个字段（`scene.snapshot` 要 12 个）、`RPC_METHODS` 里没有 `mesh.*`、`scene.verify` 的语义是导航可达性体检而非 AIR 校验、物理那边连 `simulate` 都没有。

当时列过两条路：**A** = 插件薄壳 + 新开 RPC；**B** = 给插件配 sidecar（保离线，但债一条没消）。S1-D 立项时选的是「暂缓」，等 M1-S4 的内容验证器落地再说。

2026-11-19 复评后选 A，理由：`r3e-editor-host` 本来就依赖 `r3e-physics` 与 `r3e-core`，四项计算搬过去**不新增任何依赖边**；而 B 只是把「插件直连」换成「sidecar 直连」，债务仍然挂账，且多一个进程要管。

## 2. 落地的四条 RPC

| 方法 | 参数 DTO | 回包 | 宿主动词 | 语义要点 |
|---|---|---|---|---|
| `scene.snapshot` | `SnapshotQuery{scene, path, max_nodes}` | 扁平快照（节点表 + 相机 / 灯光 / 后处理 / 物理摘要） | `scene_snapshot` | `nodeCount` 恒为真实总数，`maxNodes` 只截列表 |
| `mesh.stats` | `MeshStatsQuery{shape, segments, rings, divisions}` | 顶点 / 三角面统计 + AIR 校验结果 | `mesh_stats` | 只认内置网格（`cube` / `sphere` / `plane` / `cylinder`） |
| `scene.validate` | `ValidateQuery{scene, path}` | 逐节点问题 + 结构化定位（坐标 / 节点 / 网格） | `scene_validate` | AIR 校验，缺口进 `unvalidated` |
| `physics.simulate` | `SimulateQuery{scene, path, steps, dt, max_bodies, backend, gravity, ground}` | 刚体起止位移 + 事件 | `physics_simulate` | 在**场景副本**上跑；未编入的后端报参数错而非静默退回 |

场景来源（`scene` / `path`）三条并列语义：`path` 由宿主按「绝对路径 / 进程工作目录 / 当前场景目录」三档解析（与 `scene.switch` 同一套 `resolve_request_path`）；`scene` 吃 JSON **文本**（内联对象由 `dispatch` 先序列化，宿主只实现一条解析路径）；两者皆缺省时取**当前编辑场景**。

错误码分工：形状 / 区间错归 `invalid_params`（-32602，改参数），数据解析不了归 `engine`（-32001，改场景）。

## 3. 四层同改落点

| 层 | 文件 | 内容 |
|---|---|---|
| core 能力 | `crates/r3e-editor-host/src/inspect/{mod.rs,locate.rs}` | 四项计算实现；`locate.rs` 由 `r3e-plugin/src/locate.rs` 整体迁入（179 行，插件侧已删除） |
| protocol | `crates/r3e-protocol/src/inspect.rs`、`dispatch/inspect.rs`、`lib.rs::RPC_METHODS`、`api.rs` 默认 `unsupported` | DTO 只收敛形状；缺省 / 夹取 / 语义裁决留在宿主 |
| host 实现 | `crates/r3e-editor-host/src/api/inspect.rs`、`api/mod.rs`（`impl EngineApi`） | 翻译层：场景来源解析 + 错误码映射，不含业务裁决 |
| cmd_spec | `crates/r3e-core/src/cmd_spec/{mod.rs,meta.rs}` | 四条纯 RPC 条目（`rpc` 有值、`name` 即点分名） |
| 契约测试 | `r3e-protocol/tests/rpc_contract.rs`、`r3e-editor/tests/cmd_spec_contract.rs`、`r3e-editor-host/tests/cmd_spec_contract.rs` | RPC 表 SOURCES、dispatch 扫描下限 58 → 62、宿主方法数 58 → 62、`mesh_` 前缀映射补进 `rpc_of` |
| 能力清单 | `docs/capabilities.json` + `xtask/src/gen_capabilities.rs` | 命令表 388 → **392**、RPC 58 → **62**，`EXPECTED_COMMANDS` 常量同步 |

## 4. 插件侧现状

| 项 | 处置 |
|---|---|
| `Cargo.toml` | 只剩 `serde` / `serde_json`；`r3e-core` / `r3e-physics` 两行已删，`rapier` feature 随依赖一并移除 |
| `tools.rs` | 四个入口改为薄壳：参数前置校验（枚举 / 定长数组 / 非负整数）后发 RPC，回包原样透传 |
| `src/locate.rs` | **已删除**，迁入 `r3e-editor-host/src/inspect/locate.rs` |
| `spec.rs` / `host.rs` / `smoke.rs` | 去 core 化：能力描述改「计算在引擎侧」，`compute: "engine-rpc"` |
| 离线能力 | **不再离线**：这四项现在要求引擎在线（连不上回 -32011 并给「去启动引擎」的提示）。这是方案 A 的已知代价，登记在 `spec.rs` 与 `engine_info` 的 `compute` 字段 |

## 5. 门禁基线（2026-11-19）

- `cargo xtask verify` 十项全绿：fmt / clippy / test / replay / lint-size / check-tests / doc-gate / dep-check / gen-capabilities / invariants / scene-verify
- 命令表 **392** / RPC **62** / 仅脚本 330 / 域 17 / 不变量 **38**（三源一致）
- 单测 **1,817**（`r3e-plugin` 47 持平｜新增 6 个薄壳用例抵掉删掉的本地计算用例）
- `dep-check`：白名单外 0 条，债务 **2** 条（上限 2），全部已登记

## 6. 剩下的两条债

| crate | dep | 归口 |
|---|---|---|
| `r3e-script` | `r3e-physics` | `docs/m0/M0S5_PROTOCOL.md` §13.6 端口化（53 条绑定直连具体实现） |
| `r3e-script` | `r3e-audio` | 同上，与物理同批偿还 |

这两条与本卡无关：它们是脚本 API 直连门面，端口化要动 704 行绑定。

## 7. 判据留档

删掉 `Cargo.toml` 里 `r3e-core` / `r3e-physics` 两行后 `cargo build -p r3e-plugin` 与 `cargo test -p r3e-plugin` 全绿，`cargo xtask dep-check --strict` 违规 0、债务 2——「能力没丢」由 四条 RPC 的宿主用例（`r3e-editor-host/src/inspect/` 内 AIR / 快照 / 模拟用例）与插件 6 个薄壳用例共同兜住。

## 8. 能力名裁决（2026-11-19，闭合 S1-D 最后一项）

**裁决对象**：`scene.write` / `doc.transaction` 两个能力名与 ai_ide 宿主的对齐。

**结论**：两个名字从 r3e 侧**撤销**。不是"对方还没认领"，而是 ai_ide v1.0 的能力模型里没有这两个槽位。

实证（2026-11-19 直接读 `D:\ai_ide` 源码）：

| 事实 | 证据 |
|---|---|
| 能力表封闭 7 项：`fs.read` / `fs.write` / `search` / `workspace.info` / `progress` / `net.fetch` / `llm.chat` | `crates/plugin/src/manifest.rs:63-71` `ALLOWED_CAPABILITIES` |
| 表外能力**直接拒载** | `manifest.rs:549`「未知能力 {cap}；host 只提供 {}」 |
| 能力名是闸门键，运行期由 `capability_of(method)` 唯一映射，无方法即无落点 | `crates/core/src/plugin_host.rs:122`；`host_api.rs` 无任何 scene / doc 方法 |
| 立表意图：未知能力无法被闸门覆盖，比拒载更危险 | `manifest.rs:105` 注释 |

推论：即便把两个名字硬加进白名单，也没有对应方法可挂闸门——授权页会多出两个勾了不管用的死能力名，正好违背立表初衷。

**替代承载**：写侧授权按 ai_ide 既有口径走 `risk: "write"` + `path_field`（逐文件审批，`scope_key` = 规范化路径）+ 能力 `fs.write`。`r3e-plugin` 当前 5 个工具全 `risk=read`、能力 `["fs.read"]`，写侧工具属 M2，届时只加 `fs.write`，不加业务语义名。

**迁移口**：若 ai_ide 后续提供通用批量事务原语（提案中的 `host/tx.run` + 能力 `tx.run`），本仓只需把写侧能力声明换成 `tx.run`，插件代码不动——引擎侧的 `tx.*` 本就是协议面方法，与宿主能力无关。

**提案留档**：`docs/m2/M2_PROPOSAL_CAPABILITY_SLOTS.md`（三种形态 + 契约草案 + 验收判据 + 触发条件）。
