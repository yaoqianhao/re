---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_b457423be35711f18c5752540024e231
    ReservedCode1: gw9250OCqDz5Sod/8oJWpgDMCnhzcmg0t/5Bw5o4flR/K4LZq5psB1XqnEc3arBIB3dxYAWEVTYfBwSYR2Ece6WNK6MtWjfqsTopnJOoZpTYlqe0dQP3Bz4WdIbfo7JRnsZ+uQT/9a62q9/RPn/9/9gvJ+2qudMS49SXoGjLFDpKS5igLLZjYa2ihFU=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_b457423be35711f18c5752540024e231
    ReservedCode2: gw9250OCqDz5Sod/8oJWpgDMCnhzcmg0t/5Bw5o4flR/K4LZq5psB1XqnEc3arBIB3dxYAWEVTYfBwSYR2Ece6WNK6MtWjfqsTopnJOoZpTYlqe0dQP3Bz4WdIbfo7JRnsZ+uQT/9a62q9/RPn/9/9gvJ+2qudMS49SXoGjLFDpKS5igLLZjYa2ihFU=
---

# M1-S5 执行方案 V1：`r3e-cli` + 三范式 + Gate 1

> 状态：**待裁决**（D1–D5 见 §5；未开工）
> 日期：2026-11-18
> 前置：M1-S4 全部收口（HEAD `6aff76e`）；RPC 58、命令表 388、`ISSUE_CODES` 38、`#[test]` 1,725、`verify` 十项全绿；本地领先 origin/main 13 提交未 push
> 归属：`docs/M1_PLAN.md` §S5（S5-A / S5-B / S5-C）；父计划 `docs/AI_ROADMAP.md` §3.2 M1-S5

---

## 0. 一句话结论

S5 的难点不在「写个命令行」，而在三处**口径冲突**，不先裁决就会写出一个 Gate 1 过不了的 CLI：

1. **`replay` 无法在「只依赖 protocol」下实现**——确定性回放的真实实现住在 `r3e-replay`（依赖 core + doc）。若 CLI 内重写一遍回放循环，就是第 2 份实现（R5 高危）。裁决方向：**子进程复用既有 `r3e-replay` 二进制**，CLI 只做参数转发 + 输出契约校验 + 金标比对。
2. **事务是会话状态，CLI 是无状态命令**——`txId` 由宿主分配、进程内有效（`api/tx.rs:52`）。若靠 shell 变量接续，范式脚本就绑死 `bash + jq`，"命令可复制粘贴"直接破功。裁决方向：**txId 一律打印**（人可读/JSON 都有）+ **`--state-file` 兜底接续**（缺省 `.r3e-cli/session.json`）。
3. **引擎侧没有 headless 入口**——能起 RPC 的只有 `r3e-app`（要窗口 + GPU），且 `--rpc-port 0` 时真实端口只落在日志里，脚本无法发现。裁决方向：**固定端口 8787**（与 `r3e-plugin` 的 `DEFAULT_ADDR` 同值）+ 两行可复制命令；**不新增 `--headless`**（那是改引擎代码，且属 runtime 大改，违反 Gate 1 "不改引擎代码" 的演练前提）。

本 Sprint 不改任何引擎行为：协议层只**新增客户端模块**（`client.rs`）与把一个私有函数公开化，其余全在新建 crate 与文档/门禁台账里。

---

## 1. 实证快照（开工前只读核查）

| # | 事实 | 实证位置 |
|---|---|---|
| E1 | 协议层已具备 CLI 所需全部原语：`RPC_METHODS`（58 条，`pub const`）、错误码常量全 `pub`（含 `TX_*` / `PHYSICS_DISABLED` / `MESH_SOURCE_UNAVAILABLE`） | `crates/r3e-protocol/src/lib.rs:107 / 169-216` |
| E2 | 方法名归一化函数存在但**私有**：`fn canonical(method: &str) -> String`；CLI 若在发送前校验，只能自己抄一份（R5 风险） | `crates/r3e-protocol/src/dispatch/mod.rs:35` |
| E3 | `tx.*` 六方法已就绪（`begin/apply/intent/preview/commit/rollback`）；协议只做参数整形，**op 白名单在宿主** | `crates/r3e-protocol/src/dispatch/tx.rs:34-86` |
| E4 | `txId` 由宿主生成且**只在本次进程会话有效**；`next_tx_id()` 在宿主 | `crates/r3e-editor-host/src/api/tx.rs:52 / 56 / 222` |
| E5 | `tx.apply` 的 op 白名单 = 协议方法名（形态校验），`tx.intent` 的意图白名单 = 宿主 `INTENTS` 4 条：`scene.spawn_prefab` / `scene.place_light` / `scene.paint_terrain` / `scene.material.upsert` | `crates/r3e-editor-host/src/api/tx.rs:286` |
| E6 | `scene.verify` 回包含 `passed`，其定义为 `error_count == 0 && unchecked 为空`——**CLI 的退出码必须用 `passed`，不能用 `ok`** | `crates/r3e-editor-host/src/api/verify.rs:90-92 / 116` |
| E7 | 传输层：TCP + NDJSON，token 走 `params.token`；单行上限 1 MiB；每帧限请求数、超了回 `SERVER_BUSY`；鉴权失败 `UNAUTHORIZED` | `crates/r3e-editor/src/server.rs` |
| E8 | **同构客户端已存在**：`r3e-plugin/src/rpc.rs`（250 行，`Endpoint` + `RpcClient::call`，环境变量 `R3E_RPC_ADDR` / `R3E_RPC_TOKEN`，缺省 `127.0.0.1:8787`） | `crates/r3e-plugin/src/rpc.rs:22-27 / 105-200` |
| E9 | `r3e-replay` 已有独立二进制，`main.rs` 注释明写「M1-S5 的 `r3e-cli replay` 直接复用」；参数 `--seed` / `--frames` / `--hash-every`，缺省 42 / 600 / 60 | `crates/r3e-replay/src/main.rs` |
| E10 | `r3e-replay` 生产依赖只有 `r3e-core` + `r3e-doc`（无 protocol、无网络）→ CLI 无法直接 `use` 其逻辑而不破依赖方向 | `crates/r3e-replay/Cargo.toml` |
| E11 | workspace `members` 是**手写清单**（非 glob），内部依赖也逐个登记在 `[workspace.dependencies]` | `Cargo.toml:3-19 / 32-45` |
| E12 | `dep_rules.json` 无 `r3e-cli` 条目；扫描时**缺表 crate 会进 `missing_rule` 直接判红**（不是按空 allow 放行） | `xtask/src/main.rs:485-489` |
| E13 | `tests_baseline.json` 逐 crate 计数 + `total`，只许增不许减；新增 crate 不登记则该 crate 测试数无从比对 | `xtask/tests_baseline.json` |
| E14 | `lint-size` 有 800 行/文件硬上限（S4-F 曾因此把测试拆到 `tests/`） | `xtask/src/main.rs`（lint-size 门） |
| E15 | 引擎侧启动只有 `r3e-app`（要窗口）；`--rpc-port` / `--rpc-token` / `--rpc-max-per-frame` 由 `r3e-runtime/src/cli.rs` 解析；`--rpc-port 0` 的真实端口不回报给调用方 | `crates/r3e-runtime/src/cli.rs`、`crates/r3e-app` |
| E16 | 债务表里已为 CLI 埋了伏笔：`r3e-plugin` 的 `physics_simulate` 本地步进「需由引擎侧物理 RPC 或 `r3e-cli --features rapier` 承接」 | `xtask/dep_debt.json`、`docs/M0S5_PROTOCOL.md` §13.5 |
| E17 | `verify` 现为十项串行（fmt / dep-check / clippy / test / doc-gate / lint-size / replay / gen-capabilities / invariants / scene-verify） | `xtask/src/main.rs` |

**由 E8 + E9 + E10 推出的硬结论**：CLI 需要的两块"重"能力（TCP 客户端、确定性回放）在仓库里都已存在，但一块在违规位置上（plugin 有客户端却欠着 →core 的债），一块在依赖方向外（replay 依赖 core+doc）。S5 的正解不是重写，而是**把客户端下沉到 protocol 一份**、**把回放当外部进程调用**。

---

## 2. 交付物拆解

| 卡 | 内容 | 交付物 | 预估 |
|---|---|---|---|
| **S5-A-1** | `r3e-protocol` 增 `client` 模块：`Endpoint`（env/显式）+ `RpcClient`（长连接、`call(method, params) -> Result<Value, ClientError>`），只 `std::net` + `serde_json`（**零新第三方依赖**，protocol 仍不依赖任何引擎 crate）；同时 `canonical` 公开为 `pub fn canonical_method` | `crates/r3e-protocol/src/client.rs`（新增）、`src/lib.rs`（`pub use`）、`src/dispatch/mod.rs:35` | 1 提交 |
| **S5-A-2** | 新 crate `crates/r3e-cli`：`main.rs`（参数解析 + 退出码）+ `args.rs` + `output.rs`（human / `--json`）+ 四个子命令模块 `cmd/{tx,query,verify,replay}.rs` + `errmap.rs`（错误码 → 指路） | `crates/r3e-cli/**` | 1 提交 |
| **S5-A-3** | 门禁与台账联动：workspace `members` + `[workspace.dependencies]`、`xtask/dep_rules.json` 登记 `r3e-cli`（`allow: ["r3e-protocol"]`）、`xtask/tests_baseline.json` 加 `r3e-cli` 条目并上调 `total` | `Cargo.toml`、`xtask/*.json` | 并入 A-2 提交 |
| **S5-B** | 三范式脚本（**可复制粘贴、无 jq、无 shell 变量**）：`docs/norms/norm1_build.md` / `norm2_verify.md` / `norm3_rollback_retry.md`，每份是可整段粘进终端的命令序列 + 期望输出片段 | `docs/norms/*.md` | 1 提交 |
| **S5-C** | Gate 1 演练：按三范式实跑一遍，记录时间戳 / 命令 / 输出 / 结论，写进 `AI_ROADMAP` §八 验收表新增 Gate 1 行 + 本文件 §8 | `docs/AI_ROADMAP.md`、`docs/M1_S5_PLAN.md` | 1 提交 |

合计预估 **4 提交**（A-1 / A-2+A-3 / B / C），全程不 push。

---

## 3. `r3e-cli` 命令契约（提案）

### 3.1 全局参数

| 参数 | 缺省 | 说明 |
|---|---|---|
| `--addr <host:port>` | `$R3E_RPC_ADDR` → `127.0.0.1:8787` | 与 `r3e-plugin` 同名同义，不另立约定 |
| `--token <s>` | `$R3E_RPC_TOKEN` → 无 | 走 `params.token`（服务端既有约定） |
| `--json` | 关 | 机器可读；开时 stdout 只输出**一行** JSON，人类提示全走 stderr |
| `--state-file <path>` | `.r3e-cli/session.json`；传 `""` 关 | 记住当前 `txId`，让后续 `tx` 命令免写 id |
| `--timeout <ms>` | 20000（对齐 `r3e-plugin` 的 `IO_TIMEOUT`） | 单次读写超时 |

### 3.2 四个子命令

| 子命令 | 底层 RPC | 关键行为 |
|---|---|---|
| `r3e-cli tx begin` | `tx.begin` | 打印 `txId`（JSON 里也给），写 state-file |
| `r3e-cli tx apply <op> [--args '<json>']` | `tx.apply` | `--args` 缺省 `{}`；发送前用 `canonical_method` 归一化，并对照 `RPC_METHODS` 校验，未知方法**本地拒绝**并给最近邻建议（不烧一次往返） |
| `r3e-cli tx intent <name> [--args '<json>']` | `tx.intent` | 同上；白名单在宿主，CLI 不抄（避免两处白名单） |
| `r3e-cli tx preview` / `commit` / `rollback` | `tx.preview` / `tx.commit` / `tx.rollback` | `commit`/`rollback` 成功后清 state-file |
| `r3e-cli query <find\|describe\|stats> [--args '<json>']` | `query.*` | 缺省参数透传宿主默认 |
| `r3e-cli verify [--targets '<json>'] [--limit N]` | `scene.verify` | **退出码由 `passed` 决定**（E6），并单独高亮 `notChecked`（非空即提示"这一趟没验全"） |
| `r3e-cli replay [--seed N] [--frames N] [--hash-every N] [--expect <hash>] [--replay-bin <path>]` | 无（子进程） | 转发给 `r3e-replay`；逐行校验输出契约（`final=<hex>` 必须出现）；给 `--expect` 时做金标比对，不一致退出码 1 |

### 3.3 退出码语义（Gate 1 的判据）

| 码 | 含义 | 典型场景 |
|---|---|---|
| 0 | 成功 | `commit` 成功；`verify` 的 `passed=true` |
| 1 | 业务否定（命令跑通了，但结论是不通过） | `verify` 的 `passed=false`；`tx` 被 `TX_REJECTED` |
| 2 | 用法 / 参数错误 | 未知子命令、`--args` 不是合法 JSON |
| 3 | 环境错误（连不上 / 鉴权失败 / 超时） | 引擎没起、token 不对、`SERVER_BUSY` |
| 4 | 依赖缺失 | 找不到 `r3e-replay` 二进制 |

**为什么必须区分 0/1**：Gate 1 的剧本是"验证 → 失败 → 回滚 → 重试"，如果失败也回 0，脚本只能靠 grep 输出文本判断，AI 侧无法可靠分支。

### 3.4 错误码 → 指路层（`errmap.rs`）

把协议错误码翻成**下一步做什么**，而不是把 `-32020` 原样丢给用户/AI：

| 错误码 | 提示文案 |
|---|---|
| `PHYSICS_DISABLED` -32020 | 物理未开：先用 `--enable-physics` 重启引擎，或先跑 `r3e-cli tx apply physics.bind` |
| `MESH_SOURCE_UNAVAILABLE` -32023 | 没有网格来源：`physics.bind` 前需先 `assets.load` / 指定网格提供者 |
| `NAV_NOT_BAKED` -32022 | 导航未烘焙：先 `nav.bake` 再 `verify --targets` |
| `TX_NOT_FOUND` -32010 | 会话里没有这个事务：重新 `tx begin`（引擎重启后 txId 失效，见 E4） |
| `TX_CONFLICT` -32013 | 基哈希已漂移：`tx rollback` 后重开，不要强推 |
| `METHOD_NOT_FOUND` -32601 | 方法名不存在：附「归一化后形态」+ 最近邻候选（从 `RPC_METHODS` 里取编辑距离最小的 3 条） |

---

## 4. 三个范式（S5-B，Gate 1 素材）

写进 `docs/norms/` 的三份文档，共同约束：**整段可粘、无 `jq`、无 shell 变量、每步都有期望输出片段**。

| 范式 | 剧本 | 演示的能力位 |
|---|---|---|
| 范式 1「从零生成可通行小场景」 | `tx begin` → `tx intent scene.spawn_prefab`（地面 + 跳台 + 目标点）→ `tx intent scene.place_light` → `tx preview`（看哈希与受影响节点数）→ `tx commit` → `verify --targets` | 委托式写入：一次意图 = N 条命令 = 一格撤销 |
| 范式 2「验证并读懂失败」 | `verify --targets` 打到不可达点 → 读 issue 的 `code` / `path` / `location`（坐标）/ `hint` → 不提交，`tx rollback` | 坐标化 issue 的可操作性（S4-E 的成果在这里被消费） |
| 范式 3「失败回滚重试到成功」 | 故意生成缺口 → `verify` 回 1 → `rollback` → 修正参数重生 → `commit` → `verify` 回 0 | Gate 1 的主剧本：**AI 不改引擎代码，仅靠协议闭环** |

引擎起法（每份范式开头同样两行，复制即用）：

```
r3e-app --rpc-port 8787 --rpc-token dev --enable-physics
r3e-cli verify --token dev
```

> 固定端口 8787 的理由见 §0 第 3 条（E15：`--rpc-port 0` 的真实端口脚本拿不到）。同时端口可由 `R3E_RPC_ADDR` 覆盖，和 plugin 一致。

---

## 5. 裁决点（开工前需你拍板）

| ID | 问题 | 选项 | 推荐 |
|---|---|---|---|
| **D1** | `replay` 怎么实现 | **A1** 子进程调用 `r3e-replay`（CLI 校验输出契约 + 金标比对）／ A2 协议面新增 `replay.run` RPC（要四层同改、RPC 58→59，且回放会推进主场景，破坏只读语义）／ A3 CLI 内重写（两份实现，R5 高危） | **A1**：零协议改动、零重复、`main.rs` 注释本就指向这条路 |
| **D2** | CLI 要不要 state-file | **有**（缺省 `.r3e-cli/session.json`，`--state-file ""` 可关）／ 无（靠 `--tx <id>` 显式传） | **有**：无它则范式脚本必须用 shell 变量接续 txId，"可复制粘贴"破功；显式 `--tx` 同时保留 |
| **D3** | 错误码翻译层是否 S5 就做 | 做 / 推到 M2 | **做**：成本一个文件，Gate 1 的"AI 能读懂失败原因"直接依赖它 |
| **D4** | 引擎启动是否纳入 CLI | 不纳入（固定端口 + 两行命令）／ 加 `--spawn <bin>` 自动拉起 | **不纳入**：CLI 保持纯客户端，进程生命周期归使用者；自动拉起要处理窗口退出/端口占用/僵尸进程，收益不抵 |
| **D5** | 是否现在就加 `--features rapier`（E16 的伏笔） | 不加（S5 只依赖 protocol）／ 加 | **不加**：一旦引入 rapier，`r3e-cli` 就不再是 `allow: ["r3e-protocol"]` 的纯叶子，dep-check 立刻红；这条留在 S1-D 与 `r3e-plugin` 债务一起裁 |

---

## 6. 落地步骤（按 D1=A1 / D2=有 / D3=做 / D4=不纳入 / D5=不加）

1. `r3e-protocol/src/client.rs`：从 `r3e-plugin/src/rpc.rs` 移植 `Endpoint` + `RpcClient`，去掉 plugin 特有的措辞，错误枚举改名 `ClientError`（`Transport` / `Remote{code,message}` — 与宿主错误码共用常量）。`dispatch/mod.rs:35` 的 `canonical` 改 `pub`，`lib.rs` 导出 `pub fn canonical_method`。**不动 plugin**（它切到 protocol 是 S1-D 的事，现在动会碰债务边界）。
2. `client.rs` 单测（`tests/client_rpc.rs`，避开 800 行上限）：本地起一个 `TcpListener` 假服务端，覆盖「正常 result / error 回包 / token 注入 / 连接拒绝 / 超长行拒绝 / 半关闭连接」六条。
3. `crates/r3e-cli`：`main.rs` 只做分发与退出码（每文件 < 300 行）；`cmd/*` 每子命令一个文件；`output.rs` 统一 human/json 双通道（human 走表格、json 走单行）。
4. `replay` 子命令：`--replay-bin` 缺省依次尝试「`$R3E_REPLAY_BIN` → 同目录 `r3e-replay.exe` → `cargo run -p r3e-replay --`（debug 便利）」，全失败回退出码 4 并给出三种修法。
5. 门禁联动（A-3）：`Cargo.toml` members 与 `[workspace.dependencies]` 各加一行；`dep_rules.json` 加 `"r3e-cli": {"allow": ["r3e-protocol"], "forbid_all_engine": true, "note": "AI 远程调用的唯一入口，只认协议"}`；`tests_baseline.json` 加 `"r3e-cli": N` 并同步 `total`。
6. 提交前跑 `cargo fmt --all` + `cargo xtask verify`，确认十项全绿（新增两个 crate 会让 clippy/test 首次编译变慢，属预期）。
7. S5-B 写三范式文档并**实跑验证**每一行命令（不许写没跑过的命令）。
8. S5-C 跑 Gate 1 全流程，记录到 `AI_ROADMAP` §八（新增 Gate 1 行：日期 / 命令条数 / 单测数 / verify 时长 / 结论）。

---

## 7. 风险与回退

| ID | 风险 | 触发信号 | 缓解 / 回退 |
|---|---|---|---|
| S5-R1 | 新 crate 未登记 `dep_rules` → `dep-check` 直接红 | 首次 `verify` 报 `missing_rule` | A-2 与 A-3 同一提交落地，不允许分两次 |
| S5-R2 | `r3e-cli` 误引入非 protocol 内部依赖（例如为了复用 `Issue` 结构去 `use r3e-doc`） | dep-check 违规 1 条 | `--json` 输出**不反序列化**成引擎结构体，只做 `serde_json::Value` 透传 + 少量字段读取；这是本 Sprint 必须守住的纪律 |
| S5-R3 | `replay` 子进程路径漂移（release 布局不同） | Gate 1 演练回退出码 4 | 三种探测 + `R3E_REPLAY_BIN` 显式覆盖；文档写明 |
| S5-R4 | Gate 1 需要窗口 + GPU，无头机器跑不了 | `r3e-app` 启动即崩 | Gate 1 记录环境（GPU + 桌面会话）；CI 上只跑 `verify` 十项，不跑 Gate 1；若后续要自动化再议 `--headless`（属引擎改造，另开卡） |
| S5-R5 | CLI 主文件超 800 行被 `lint-size` 拦 | verify 红 | 一开始就按 `cmd/*` 拆，`main.rs` 只留 dispatch |
| S5-R6 | 三范式文档写了没跑通的命令（文档幻觉） | 演练时命令报错 | 文档里每条命令的期望输出片段必须来自真实运行粘贴，禁止手写想象输出 |
| S5-R7 | `client.rs` 下沉后与 plugin 的 `rpc.rs` 短期双份 | grep 出两个 `RpcClient` | 明确记账：S5 只**新增** protocol 版，plugin 版在 S1-D 一并收敛；期间 `dep_debt.json` 不动（这不是依赖违规，只是重复实现，风险登记在此） |

---

## 8. 验收标准

| # | 项 | 判据 |
|---|---|---|
| 1 | CLI 可用 | `r3e-cli --help` 见四个子命令；`r3e-cli tx begin` 对着活引擎返回 `txId` |
| 2 | 命令可复制粘贴 | 三范式文档中的命令**无** 变量赋值、**无** `jq`、**无** 管道取字段 |
| 3 | 退出码可靠 | `verify` 通过=0 / 不通过=1 / 参数错=2 / 连不上=3 / 缺 replay=4，四条各实测一次 |
| 4 | 失败可读 | 故意关物理跑 `verify`，输出含 `PHYSICS_DISABLED` 与「下一步」提示，而非裸 `-32020` |
| 5 | 门禁联动 | `cargo xtask verify` 十项全绿；`dep-check` 报告含 `r3e-cli` 且违规 0 |
| 6 | 单测基线 | `#[test]` 1,725 → 约 1,750（client 6 条 + CLI 参数/退出码/errmap 约 20 条），`tests_baseline.json` 同步 |
| 7 | **Gate 1 通过** | 不改引擎代码，仅用 `r3e-cli` + 协议完成「生成可通行小场景 → 验证 → 失败回滚重试 → 成功提交」，全过程命令可复制粘贴，`AI_ROADMAP` §八 有记录行 |
| 8 | 依赖方向不变 | `cargo tree -p r3e-cli` 只含 `r3e-protocol` + `serde_json`；`cargo tree -p r3e-protocol` 仍只有 serde |

---

## 9. 与后续卡的衔接

- **S1-D 还债**：本 Sprint 之后 `r3e-cli` 与 `r3e-plugin` 共用 protocol 的 `client` 模块，plugin 的 `rpc.rs` 可整体删除 → plugin 的 `→ core` 债务里「自己造客户端」这条自然消解；plugin 的 `physics_simulate` 是否转 `r3e-cli --features rapier` 仍按 D5 留到那时再裁。
- **M1-DOC-FIELDS**（顺延 M2）：CLI 的 `--json` 输出字段即 AI 的稳定接口面，M2 做字段文档时以 CLI 子命令为单位生成。
- **M1-ASSET-0**（未决）：与本 Sprint 无耦合。
*（内容由AI生成，仅供参考）*
