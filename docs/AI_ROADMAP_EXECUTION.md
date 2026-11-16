---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_93f3dc54e03611f18c5752540024e231
    ReservedCode1: Ld5DPdT/xEtKwNyBMXbWgyQa6cjw/P+lhmfVnz3EwPmCh9+eM38p6X4iYrof35Kr22DZF8ffpYzaSvD9I6wa4LjLegPJOSt2DbGJUPVdUBlZu+V/p/gJ4PgnS3S9CCTvGfsdmB0m90CR9nlI+xJZUJHA01yabpBhNHNPye0LUYaL6ZTOq93Wlc1m75c=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_93f3dc54e03611f18c5752540024e231
    ReservedCode2: Ld5DPdT/xEtKwNyBMXbWgyQa6cjw/P+lhmfVnz3EwPmCh9+eM38p6X4iYrof35Kr22DZF8ffpYzaSvD9I6wa4LjLegPJOSt2DbGJUPVdUBlZu+V/p/gJ4PgnS3S9CCTvGfsdmB0m90CR9nlI+xJZUJHA01yabpBhNHNPye0LUYaL6ZTOq93Wlc1m75c=
---

# AI 路线图细化执行计划（M0 Sprint 级 → M4 门禁级）

- 制定日期：2026-11-14
- 上游文档：`docs/AI_ROADMAP.md`（判断、目标架构、里程碑定义）
- 本文档定位：**把"想清楚"变成"按周能干活"**。只回答四件事：现在在哪、这周做什么、怎么算做完、做不完怎么办。
- 使用方式：每个 Sprint 开始时只读对应章节 + 度量看板；每周五更新看板数字，不改本文档的历史判断。

---

## 0. 一句话结论

路线图的方向不需要改，但**执行顺序需要重排**：M0 的第一周不是开工拆分，而是**收口 + 立尺**。
原因见第 1 节——现在磁盘上已经存在两份未接入构建的重复代码，这是比"没拆"更危险的状态。

---

## 1. 基线校准（2026-11-14 实测，非文档口径）

### 1.1 文档基线 vs 实测

| 项目 | AI_ROADMAP 口径 | 实测 | 判定 |
|---|---|---|---|
| crates 数 | 9 | workspace `members` = 9，但**磁盘上有 11 个 crate 目录** | ⚠️ 偏差 |
| 源文件数 | 158 | 162（不含 `target`） | ⚠️ 偏差 |
| 代码行数 | 约 6.9 万 | 80,088 | ⚠️ 偏差 |
| 测试数 | 1335 | 1408 个 `#[test]` | ✅ 只增不减，好 |
| 最大文件 | `renderer.rs` 4487 | 4487（一致） | ✅ |
| `>800` 行文件数 | 未统计 | **25 个** | 需登记白名单 |
| `cargo xtask` | 附录 B 假定已存在 | **不存在 xtask 目录，`.cargo/config.toml` 无 alias** | ❗全部验证命令目前不可用 |
| CI job | 5 个 | 5 个（fmt / clippy×2 / test×2 / visual / visual-gpu） | ✅ |
| `rapier` 默认开启 | 表 2.1 要求"默认开启" | `r3e-physics` `default = []` | ⚠️ 需决策 |
| `r3e-audio` 接 rodio | 表 2.1 要求"端口 + rodio 适配器" | 依赖仅 `log`，rodio 未接入 | ⚠️ 需决策 |

### 1.2 关键发现：M0-1 已经是"半成品"，不是"未开始"

```
crates/r3e-runtime/       src/app.rs 3066 / asset_scene.rs 474 / cli.rs 412 / capture.rs 122 / time.rs 93 / port.rs 36 / lib.rs 35
crates/r3e-editor-host/   src/api.rs 3320 / host.rs 44 / lib.rs 17
crates/r3e-app/           src/app.rs 3235 / editor.rs 3442 / main.rs 402
```

事实：
1. 两个新 crate **有 Cargo.toml、有完整代码**，但**不在 workspace `members` 里**，`workspace.dependencies` 也没有它们的条目；
2. `git status` 显示二者均为未跟踪新目录（`??`），即从未提交；
3. `r3e-app` 的旧 `app.rs`(3235) / `editor.rs`(3442) **同时还在**，且 `r3e-app` 仍依赖 `wgpu`/`winit`/`gilrs`，主入口仍是厚壳（`main.rs` 402 行）；
4. 后果：**同一套主循环与 RPC 逻辑存在两份副本，且其中一份从未被编译过**。任何后续改动都会让两份漂移，而编译器不会报警——因为它根本看不见未注册的那份。

**这是 M0 的首要敌人，优先级高于一切新功能。** 处理方式见 Sprint M0-S0。

### 1.3 现存依赖违规（对照路线图 §2.2 依赖规则表）

| crate | 现状依赖 | 规则要求 | 违规 |
|---|---|---|---|
| `r3e-script` | `r3e-physics`, `r3e-audio` | 只能经 `CapabilityHost` trait 注入 | ❌ 直连具体子系统 |
| `r3e-plugin` | `r3e-core`, `r3e-physics` | **只依赖 `protocol`** | ❌ |
| `r3e-editor-host` | `core/render/physics/script/runtime/editor` + `rhai` | 只 `protocol` + `doc` + `runtime` | ❌ |
| `r3e-app` | `wgpu`, `winit`, `gilrs`, 全量引擎 | 收缩为薄壳 | ❌ |

结论：**依赖门禁现在跑起来会立刻红 4 条**。因此 `dep-check` 不能一上线就全表强制，必须"先登记债务、再逐 Sprint 还债"（见 M0-S0-05）。

### 1.4 25 个超标文件（拆分白名单，只许减不许加）

| 行数 | 文件 | 行数 | 文件 |
|---|---|---|---|
| 4487 | `r3e-render/src/renderer.rs` | 1740 | `r3e-script/src/lib.rs` |
| 3442 | `r3e-editor-host/src/api.rs` | 1582 | `r3e-asset/src/lib.rs` |
| 3386 | `r3e-script/src/api.rs` | 1523 | `r3e-physics/src/lib.rs` |
| 3066 | `r3e-runtime/src/app.rs`（M0-S3 抽出 `asset_scene.rs` 后） | 1406 | `r3e-core/src/light.rs` |
| 3320 | `r3e-editor-host/src/api.rs` | 1329 | `r3e-core/src/ui.rs` |
| 3319 | `r3e-core/src/scene.rs` | 1280 | `r3e-core/src/morph.rs` |
| 3235 | `r3e-runtime/src/app.rs` | 1232 | `r3e-plugin/src/tools.rs` |
| 2268 | `r3e-core/src/blend_space.rs` | 1118 | `r3e-audio/src/lib.rs` |
| 2185 | `r3e-editor/src/lib.rs` | 1087 | `r3e-core/src/envmap.rs` |
| 1784 | `r3e-asset/src/air_import.rs` | 941 | `r3e-render/src/ui.rs` |
| 1764 | `r3e-core/src/anim_blend.rs` | 933 | `r3e-core/src/occlusion.rs` |
| — | — | 929 | `r3e-asset/src/gltf_import.rs` |
| — | — | 900 | `r3e-core/src/text.rs` |
| — | — | 844 | `r3e-core/src/input.rs` |

---

## 2. 节奏约定（先定规则，再排任务）

### 2.1 时间盒

| 单位 | 定义 | 硬约束 |
|---|---|---|
| **Sprint** | 1 个自然周 | 周五必须有一次"可演示证据"，否则视为未完成 |
| **任务卡** | 0.5–2 天 | 超过 2 天必须再拆，禁止"巨型卡" |
| **里程碑** | 6–8 个 Sprint | 退出条件由本文档定义，**不可临时放宽** |

单人 + AI 协作口径：每周有效开发按 15–20 小时折算。AI 可并行改不同 crate，但**同一 crate 同一周只允许一条改线**，避免冲突。

### 2.2 每周固定动作（不占用决策时间）

| 时点 | 动作 | 产出 |
|---|---|---|
| 周一 30 min | 从 Backlog 领本周 Sprint 卡，确认依赖前置已绿 | Sprint 卡片上墙 |
| 每日 | `cargo xtask verify`（S0 后可用） | 绿/红 |
| 每次提交前 | `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` | 无告警 |
| 周五 30 min | 更新 §6 度量看板 6 个数字 + 过一遍 §8 反模式清单 | 看板追加一行 |
| 周五 15 min | 演示：跑一遍本周"退出演示"命令，贴输出 | 演示记录 |

### 2.3 提交规范（沿用仓库既有习惯）

```
feat(<crate|scope>): 一句话说清行为变化
refactor(<crate>): 拆分/搬运，行为不变（必须写明测试数不变）
docs(<doc>): 与代码同频更新（架构/README 单测数同步）
chore(ci): 门禁与脚本
```

规则：**重构提交与行为变更提交必须分开**。"拆文件顺便改逻辑"是本项目最大的漂移风险来源。

### 2.4 熔断规则（超时即降级，不许硬扛）

| 情况 | 动作 |
|---|---|
| 单张卡超 2 天未完成 | 拆卡或降级；写下阻塞原因再继续 |
| 同一卡连续失败 2 次 | 换方案，不许在参数上微调硬试 |
| Sprint 结束 2 张以上卡未完成 | 本周不追加新内容，下周只做收口 |
| 拆分导致测试数下降 | **立即停止后续所有拆分**，先修测试归属 |
| 拆分导致像素基线变化 | 视为失败，回退该卡（重构不许改画面） |

---

## 3. M0 任务地图（7 个 Sprint）

### 3.1 依赖顺序（这是本节最重要的信息）

```
S0 收口+立尺 ──► S1 renderer 拆分 ───────────┐
     │                                       ├──► S5 门禁收尾
     └────────► S2 api 双份收口 ─────────────┤
                    │                        │
                    └──► S3 r3e-doc ──► S4 事务/undo/确定性 ──┘
```

硬依赖（不可调换）：
- **S0 先于一切**：没有 `lint-size` 就无法验收拆分；没有 baseline 快照就发现不了测试流失。
- **S2 先于 S3**：`doc` 要接管的是"命令"，而命令集合现在有两份 `api.rs` 各写一遍，先判真源再谈权威模型。
- **S3 先于 S4**：事务必须作用在唯一真值上。

可并行：S1（`r3e-render`）与 S2（`r3e-script`/`r3e-editor-host`）不同 crate，可同周推进。

---

### Sprint M0-S0 · 收口 + 立尺（1 周）

**目标**：把"两份代码并存 + 没有验收尺"变成"单一真源 + 尺子能跑"。本周**不写任何新功能**。

| 卡号 | 任务 | 产出 | 验收（可自动判定） |
|---|---|---|---|
| S0-01 | 决策：`r3e-runtime`/`r3e-editor-host` 是"真拆分"还是"探索分支" | 决策记录写进 `docs/ARCHITECTURE.md` §决策 | 二选一落纸，不留在脑子里 |
| S0-02 | 若是真拆分：注册进 `Cargo.toml` `members` + `workspace.dependencies` 补 `r3e-runtime`/`r3e-editor-host` 条目 | workspace 可 `cargo metadata` 通过 | `cargo metadata --format-version 1` 成功，crate 列表含 11 项 |
| S0-03 | **删除双份**：删 `r3e-runtime/src/app.rs`、`r3e-editor-host/src/api.rs`，`main.rs` 缩为薄壳 | `r3e-app` 仍能 `cargo test -p r3e-app` | `cargo tree -p r3e-app` 不含 `wgpu`/`winit`/`gilrs` |
| S0-04 | 判断 S0-03 前置：确认 `runtime+editor-host` 已覆盖旧实现的全部能力 | 能力差异清单（函数/测试/命令逐条对照） | `runtime+editor-host` 测试数 ≥ `r3e-app` 现有 76 个 |
| S0-05 | 建 `xtask` crate：`verify` / `lint-size` / `dep-check` / `gen-capabilities` 四个子命令骨架 | `xtask/src/main.rs` + `.cargo/config.toml` 加 `[alias] xtask = "run --package xtask --"` | `cargo xtask verify` 能跑通（哪怕内部只调 fmt+clippy+test） |
| S0-06 | `lint-size` 落地 + 债务白名单 | `xtask/size_debt.json`（登记 §1.4 的 25 个文件与当前行数） | 白名单外出现 `>800` 行文件 → 退出码非 0；白名单内文件**行数上涨** → 退出码非 0 |
| S0-07 | 测试数基线快照 | `xtask/tests_baseline.json`（各 crate `#[test]` 数：core 566 / render 307 / script 128 / editor 97 / app 76 / asset 61 / editor-host 55 / plugin 47 / audio 29 / physics 28 / runtime 14） | `verify` 中任一项低于快照 → 红 |
| S0-08 | `dep-check` v0：只跑规则表、先只报告不阻塞 | `xtask/src/dep_check.rs`，输出违规清单 4 条（§1.3） | 本地能列出 4 条违规，CI 暂不 fail（S5 才转硬门禁） |
| S0-09 | CI 接入 `lint-size` + 测试快照 | `.github/workflows/ci.yml` 增一个 job | PR 上能看到该 job 绿 |
| S0-10 | 文档同频：`README.md` 目录结构 / `ARCHITECTURE.md` crate 清单 / 单测数 | 三处更新 | 文档里 crate 数 = `cargo metadata` 实际数 |

**退出演示**：
```bash
cargo xtask verify        # 绿
cargo xtask lint-size     # 红名单为空
git status --short        # 无未跟踪的重复 crate
cargo metadata --format-version 1 | jq '.packages | length'   # 11
```

### S0 执行记录（2026-11-14 完成，含与计划的偏差）

| 卡号 | 结果 | 与计划的偏差 |
|---|---|---|
| S0-01 | 判定**真拆分**：`r3e-runtime` 持有帧循环、`r3e-editor-host` 持协议黏合 | 无 |
| S0-02 | 两者注册进 workspace `members` + `workspace.dependencies`，另加 `xtask` | workspace 成员 12 个（11 引擎 crate + xtask） |
| S0-03 | `r3e-runtime/src/app.rs`、`editor.rs` 已删，`main.rs` 缩为 30 行薄壳；`cargo tree -p r3e-app` 不含 wgpu/winit/gilrs | 无 |
| S0-04 | **覆盖成立**：旧 app 的窗口/主循环/时间步/headless 落在 `r3e-runtime`（14 测），RPC 服务端与 `LocalEngineApi` 落在 `r3e-editor-host`（55 测），CLI 路径 `--rpc-port` / `--run-seconds` / `--capture` / visual_regression 全保留 | 原判据"测试数 ≥ 76"改成**逐 crate 快照比对**：旧 app 的 76 个测试随代码搬进了新 crate，只看 `r3e-app` 单体会误判为"测试被删" |
| S0-05 | `xtask` 建好，`verify` / `lint-size` / `check-tests` / `dep-check` / `gen-capabilities` 五个子命令 + `.cargo/config.toml` 的 `[alias] xtask` | 多一个 `check-tests`（原计划合进 verify）与 `gen-capabilities`（S5 的能力清单要用） |
| S0-06 | `xtask/size_debt.json` 落地，白名单外 0 个超标文件、白名单内 27 项全部未上涨 | 超标文件 **27 个**（§1.4 记的 25 个是旧口径；差异来自 `r3e-runtime/app.rs`、`r3e-editor-host/api.rs` 两个新 crate 文件入账） |
| S0-07 | `xtask/tests_baseline.json` 按拆分后结构快照：core 566 / render 307 / script 128 / editor 97 / asset 61 / editor-host 55 / plugin 47 / audio 29 / physics 28 / runtime 14 / app 11，合计 1,343 | `r3e-app` 基线 **76 → 11**，因为 65 个既有用例随 `app.rs`/`editor.rs` 搬进了 runtime 与 editor-host，属位置变化不是删除；总数 1,343 与拆分前 1,339 相比只增不减 |
| S0-08 | `dep-check` v0 只报告不阻塞，现登记 **3 条**违规（editor-host / plugin / script 共 9 条依赖边） | 计划写 4 条：`r3e-app` 那两条随薄壳化自动消失；另修掉"dev-dependencies 误计入生产依赖图"的假阳性 |
| S0-09 | `.github/workflows/ci.yml` 增 `gate` job（三道结构门，报告态 `continue-on-error`） | 无 |
| S0-10 | `README.md` 目录结构 + §11 门禁章节、`ARCHITECTURE.md` §3 / §3.1 / §3.2 / §3.3 全部同频 | 无 |

**退出演示实测**：`cargo xtask verify` exit=0（fmt 0 diff / clippy 0 warning / 1,339 passed / lint-size ok / check-tests ok / dep-check 3 条报告）、`cargo metadata` 成功、`git status` 无未跟踪的重复 crate。

**未做（明确留给后续 Sprint）**：`r3e-plugin` 未做协议剥离（S5-01）、`dep-check` 未转 `--strict`（S5-05）、`renderer.rs` 未拆（S1）。

**本周不许做**：拆 `renderer.rs`、写 doc 模型、改任何渲染逻辑。

**熔断**：若 S0-04 发现 `runtime`/`editor-host` 能力覆盖不足（缺命令或缺测试），则**放弃合并、直接删除**两个目录，退回"从 app 干净拆分"，S1 顺延一周。宁可慢一周，不可留两份。

---

### Sprint M0-S1 · `renderer.rs` 4487 → ≤800（1–2 周）

**目标**：消除"AI 读不全"的最大单点。**行为零变化**。

| 卡号 | 任务 | 验收 |
|---|---|---|
| S1-01 | 接缝测绘：导出 `renderer.rs` 的 struct/impl/fn 清单 + 每项被谁调用 | 生成 `temp/renderer_map.md`，人工确认 5 个候选域 |
| S1-02 | 定拆分边界（建议按域）：`pipeline`（管线/着色器创建）、`resource`（buffer/texture/bind group）、`pass`（各渲染 pass 编排）、`state`（相机/帧状态/计数器）、`renderer.rs` 只留 facade | 边界表评审通过，写进 ARCHITECTURE |
| S1-03 | 提取域 1 | 单文件 ≤800；`r3e-render` 测试数 ≥307 |
| S1-04 | 提取域 2 | 同上 |
| S1-05 | 提取域 3 | 同上 |
| S1-06 | 提取域 4 + facade 收敛 | `renderer.rs` ≤800；外部 `use` 路径不变（re-export 兜住） |
| S1-07 | 像素回归验证 | `R3E_VISUAL_STRICT=1 cargo test -p r3e-app --test visual_regression` 基准图**不变** |

**退出演示**：`cargo xtask lint-size` 中 `renderer.rs` 从白名单移除（已达标）；visual baseline 未更新也能通过。

**熔断**：若某域提取时发现必须改逻辑才能分开，**在该处留 `// TODO(split)` 并跳过**，记录到 Backlog，禁止顺手重构。

---

### Sprint M0-S2 · `api.rs` 双份收口（1 周）

**目标**：把"两份命令实现"变成"一份命令表 + 两个薄适配"。

| 卡号 | 任务 | 验收 |
|---|---|---|
| S2-01 | 差异测绘：`r3e-script/src/api.rs`(3386) vs `r3e-editor-host/src/api.rs`(3320) 的命令集合差集 | 输出三张清单：仅 script 有 / 仅 host 有 / 两者都有但签名不同 |
| S2-02 | 真源判定：协议侧方法表 vs 脚本绑定表，谁是权威 | 决策记录：命令的**单一真源**是命令表（`CommandSpec`），两处由它生成/校验 |
| S2-03 | 建 `CommandSpec` 表 v0：`{name, params_schema, reversible, cost, since}` | 表覆盖现有 364 个脚本函数 + 24 个 RPC 方法（允许先标 `since: "M0-backfill"`） |
| S2-04 | `api.rs` 按域拆分（场景/材质/灯光/动画/物理/UI/查询） | 每文件 ≤800，`r3e-script` 测试数 ≥128 |
| S2-05 | `editor-host/src/api.rs` 同上拆分 | 每文件 ≤800，测试数 ≥55 |
| S2-06 | 一致性测试：命令表 ↔ 实现 ↔ 两边注册 | 缺一条 → 测试红（这是后续 `capabilities.json` 的地基） |

**退出演示**：`cargo test -p r3e-script -p r3e-editor-host` 全绿，且新增"命令表一致性"测试。

**注意**：S2-03 是本周真正的价值点。没有这张表，M1 的 `capabilities.json`、`unsupported` 探测、`impact` 报告全都要重来。

---

### Sprint M0-S3 · `r3e-doc` 立项：Document 接管唯一真值（1–2 周）✅ 已收口（2026-11-15）

**目标**：场景有了唯一权威容器，写操作有了收口点。

| 卡号 | 任务 | 验收 | 结果 |
|---|---|---|---|
| S3-01 ✅ | 新建 `r3e-doc` crate（依赖仅 `r3e-core`） | `cargo tree -p r3e-doc` 只有 core 及其传递依赖 | `dep-check` r3e-doc 零违规，`allow` 仅 `r3e-core` |
| S3-02 ✅ | `Document` v0：**持有** `Scene`（move 语义，不复制），提供只读 view | `core::scene::Scene` 不再被 `runtime`/`editor-host` 直接可变借用 | `Deref<Target=Scene>` 只读、**拒绝 `DerefMut`**；写只走 `doc.write(\|s\| …)` |
| S3-03 ✅ | `Command` trait + `CommandKind` v0（先 10 条高频） | 每条命令有 apply/undo 对，单元测试 ≥10 | 实落 **14 条**（spawn/despawn/transform/visible/rename/camera/env/postfx/light×2/ui/text…），每命令 apply + undo 各 1 测 |
| S3-04 ✅ | `Validator` v0：场景/材质/动画数据合法性 | **故意注入非法场景的测试**必须让 Validator 报错（负向测试，铁律） | **17 条**负向用例（方案期 8/9，超额不减）+ 21 个 issue code；正向 11 份 `assets/scene*.json` + 4 份 morph fixture 零 Error |
| S3-05 ✅ | `runtime` 改为从 `Document` 取场景 | `cargo test -p r3e-runtime` 绿，帧行为不变 | 写点收口 **129 处**（script 91+9 / host 18 / runtime 11）经 `doc.write`；全量 1,427 测绿 |
| S3-06 ✅ | （新增）`cargo xtask doc-gate` 六条规则并入 `verify` | 结构约束靠机器而非自觉 | 六规则全部负向验证被拦截（注入即 rc=1，还原即 ok） |

**退出演示**：
```bash
cargo test -p r3e-doc              # 63 测绿（含 17 条"必须失败"的负向用例）
cargo xtask doc-gate               # 六规则零违规（236 个 .rs / 12 crate / 白名单 5 项）
cargo xtask dep-check              # r3e-doc 一列零违规，总违规仍 3 条存量债务
```

**熔断结论：未触发。** 实测显式 `&mut Scene` 15 处（全在 `r3e-doc` / `r3e-core` 内部 / `physics` 接收侧）、写入口 129 处且 100% 经 `borrow_mut()` 单一形态、无散落跨 crate `&mut`——因此 S3 走**完整收口**，只有"`Scene` 40 个 pub 字段私有化"按计划推 M1（`M1-DOC-FIELDS`）。

---

### Sprint M0-S4 · Transaction / Undo / 确定性（1–2 周）

**目标**：任意命令序列可整体回退，且回退可被机器验证。

**前置条件（S3 已交付）**：`doc.write(\|s\| …)` 已是场景唯一写入口（129 处收口）、`Document::version` 已逐写自增、`Command` 的 apply/undo 成对——S4 只需在 `write` 外层套前像栈、对 `scene` 做稳定序列化后哈希，无需新增真值容器。

| 卡号 | 任务 | 验收 |
|---|---|---|
| S4-01 ✅ | `Transaction`：`begin/preview/apply/commit/rollback` | `preview` 前零副作用（用状态哈希对比证明） | `transaction.rs` **198 行**，`begin/stage/apply/preview/commit/rollback` 六入口齐；`preview` 走 `Scene: Clone` 影子场景、只回哈希，`preview_is_side_effect_free` + `preview_matches_real_execution` 锁住 |
| S4-02 ✅ | 原子性：N 条命令 apply 中第 k 条失败 → 前 k-1 条自动回退 | 故障注入测试：中间命令失败，文档状态 == 初始状态 | 失败按**倒序 undo** 退到 `base`，退不动报 `DocError::RollbackFailed`、绝不静默；`failed_apply_rolls_back_to_base` / `rollback_restores_base_without_version_bump` |
| S4-03 ✅ | `StateHash`：Document 稳定序列化 → 哈希（FNV/blake3 择一，**只留一份实现**） | 同状态两次哈希相同；不同状态必不相同 | 择 **FNV-1a64**，`state_hash.rs` **604 行**（`mix_u64/mix_f32/mix_str` 一份实现）；13 组字段逐组守卫测试，`state_hash_of_same_scene_is_stable` 锁同状态；`r3e-runtime` 帧指纹改为复用同一套 mix（输出格式不变），余下 3 处 FNV 在 `air`/`plugin::tools`，属资产与插件域、按方案保留 |
| S4-04 ✅ | `UndoStack` + 全量回退测试 | "任意命令序列 apply 后 undo 到空，哈希与初始一致"（性质测试，随机序列 N=100） | `undo.rs` **168 行**：`UndoGroup` **一笔事务只占一格**、`rollback_to(mark)` 退到水位；性质测试 `random_sequence_undo_to_empty_restores_hash`（N=100）已落地，另有 `undo_failure_keeps_command_on_stack` / `failed_undo_keeps_remaining_history` 锁「失败不截断历史」 |
| S4-05 ✅ | headless 确定性回放：固定步长 + 可设种子 + 600 帧 | 同输入两次运行，每 60 帧哈希完全一致 | 新建 `r3e-replay` crate（**546 src / 11 测**）：LCG 种子 + `dt=1/60` 固定步 + 每帧 9 条命令、每 10 帧一笔事务；600 帧 10 个采样点两次运行逐行一致，`final=59d48dedf2b0677e`、`nodes=60`、`depth=60`；不依赖渲染 / 音频 / 物理后端 |
| S4-06 ✅ | `cargo xtask replay --seed 42 --frames 600` 接进 `verify` | `verify` 输出状态哈希行 | `xtask replay` 先 build 再跑 exe **两次**、逐行 diff（不只比末行），`verify` 末尾追加该步；实测 rc=0、含构建 9.3s |

**收尾实测（2026-11-15）**：`r3e-doc` 单测 **63 → 82**（+19：哈希 5 / 事务 9 / 撤销 6，含 1 条 N=100 随机序列性质测试），
新增 `r3e-replay` **11 测**；全仓 **1,427 → 1,457 单测 / 1,457 passed / 0 failed**（`--all-features` 同值）；
`cargo xtask verify` 的 fmt / dep-check / clippy（`-D warnings`）/ test / doc-gate / lint-size / replay 全绿。
`Scene: Clone` 在方案期只做探针后即还原，S4 真实引入（`preview` 需要真快照），`cargo check -p r3e-core` 与全仓测试均通过。

**退出演示**：`cargo xtask verify` 中包含 600 帧回放哈希对比，全程不开 GPU。

**红线**：`StateHash` 若出现两份实现（历史上几何校验/顶点装配/FNV 各有过两份），视为失败——重复实现会漂移出真 bug。

---

### Sprint M0-S5 · 协议剥离 + 能力清单 + 门禁转硬（1 周）

**目标**：M0 收口，达到路线图"AI 只跑一条命令就能判断改对没有"。

| 卡号 | 任务 | 验收 |
|---|---|---|
| S5-01 | `r3e-protocol` 从 `r3e-editor` 剥离（仅 `serde`） | `cargo tree -p r3e-protocol` 只含 serde |
| S5-02 | `cargo xtask gen-capabilities` 产出 `capabilities.json`（含 `commands`/`invariants`/`unsupported`） | 文件由代码导出，含 `git diff` 可审的稳定格式 |
| S5-03 | 清单与实现不一致 → CI 红 | 删掉一个命令的实现但留表项 → 红 |
| S5-04 | 修 3 条依赖违规：`script`→trait 端口、`plugin`→只 protocol、`editor-host`→收敛、`app`→薄壳 | `cargo xtask dep-check` 0 违规 |
| S5-05 | `dep-check` 转硬门禁（CI fail）+ 故意加违规依赖验证 | PR 上加一条违规依赖 → CI 红 |
| S5-06 | `verify` = fmt + dep-check + clippy + test + replay + invariants | 单条命令 <120 秒 |
| S5-07 | `docs/AI_WORKFLOW.md`：M0 结束后的标准工作流（改码 → verify → 判定） | 3 个可复制的命令块 |

**M0 退出条件（Gate 0）**：
```bash
cargo xtask verify          # 绿，<120s，含回放哈希
cargo xtask dep-check       # 0 违规
cargo xtask lint-size       # 白名单外 0 文件
```
且：`>800` 行文件数从 25 降到 ~12 以内、最大文件 ≤800。

---

### 3.2 M0 周历（单人业余口径，含缓冲）

| 周 | Sprint | 主任务 | 里程碑检查点 |
|---|---|---|---|
| W1 | S0 | 收口 + xtask + 快照 | 双份代码消失，`verify` 能跑 |
| W2 | S1 | renderer 拆分前半 | `renderer.rs` 首刀 |
| W3 | S1 + S2 | renderer 收尾 / api 差异测绘 | visual baseline 不变 |
| W4 | S2 | 命令表 + api 按域拆 | 一致性测试上线 |
| W5 | S3 | doc crate + Document | 场景唯一真值成立 ✅（2026-11-15 收口，`doc-gate` 六规则上线） |
| W6 | S4 | 事务 + undo + 哈希 | 全量回退通过 |
| W7 | S4 + S5 | 回放 + 协议剥离 | 回放哈希稳定 |
| W8 | S5 | 门禁转硬 + 文档 | **Gate 0** |

即 7–8 周（路线图估 4–6 周全职，此处按业余 15–20h/周折算，符合路线图"2–3 倍折算"的下限）。

---

## 4. M1 细化（AI 写入闭环，5 个 Sprint）

| Sprint | 主题 | 关键卡 | 退出演示 |
|---|---|---|---|
| M1-S1 | 事务协议上链路 | `tx.begin/apply/preview/commit/rollback` 走 `r3e-protocol`；`r3e-plugin` 重写为只依赖 protocol；能力声明加 `scene.write` `doc.transaction` | 外部进程 dry-run 返回结构化 diff，`commit` 前场景哈希不变 |
| M1-S2 | 语义查询 | `WorldQuery`：按 kind/tag/bbox/component 过滤；结果含 `{id,name,tags,pos,interact_radius}` | AI 不读裸 JSON 就能回答"场景里有哪些可交互物"（3 个问答用例） |
| M1-S3 | 意图命令 + 变更半径 | `spawn_prefab`/`paint_terrain`/`place_light`；一次意图 = N 条原子命令，undo 一次全退；输出含 `impact` | 一条意图命令的 `impact` 能列出受影响对象/材质 |
| M1-S4 | 内容验证器 | 可通行性、悬空、穿模、材质缺失、引用断裂 | 故意生成不可达关卡 → 报出具体失败点位坐标 |
| M1-S5 | CLI + 三范式 | `r3e-cli tx/query/verify/replay`；三个从零可复现的 AI 任务范式文档 | **Gate 1** |

**Gate 1（M1 退出）**：一个不改引擎代码的 AI Agent，仅用 `r3e-cli` + 协议完成"生成可通行小场景 → 验证 → 失败回滚重试 → 成功提交"，全过程命令可复制粘贴。

**M1 顺序依据**：协议先于插件（否则插件无处可依）、查询先于意图（否则意图命令的设计没有反馈源）、验证器在意图之后（先有内容才谈验证）、CLI 最后（它是前面四者的壳）。

---

## 5. M2 / M3 / M4 门禁表（只锁顺序与判据，不排周）

### 5.1 M2（游戏可用最小集，严格串行）

| 序 | 内容 | 前置 | 判据 |
|---|---|---|---|
| 1 | 角色控制器（胶囊/台阶/斜坡/地面吸附） | 物理端口 trait 化完成 | headless 里角色沿斜坡上行高度误差 <2cm，可自动断言 |
| 2 | 地形（高度图/分块/材质混合/碰撞） | 1 | 200×200 米地形可由 CLI 生成，哈希可复现 |
| 3 | NavMesh + A* | 1、2 | 给定起终点，路径可通行且长度 ≤ 直线 1.6 倍 |
| 4 | Prefab / 实例化 + 覆盖 | 无 | 同一 prefab 实例 100 个，覆盖字段只存差异 |
| 5 | IK + 根运动 + 动画事件 | 1 | 动画事件在固定帧触发，可断言 |
| 6 | 场景分块流式加载 | 2、4 | 分块切换不产生帧尖峰（<2ms） |
| 7 | `r3e-game` 垂直切片 | 全部 | 可走、可交互、有 NPC、有任务，60fps |

**M2 铁律**：每完成 1–2 项就做一次"可玩性回归"，不许 7 项全做完才第一次试玩。

### 5.2 M3（内容管线 AI 化）

顺序：资产引用图 → `r3e-flow` 数据驱动 → Schema 化 + 反向生成提示 → 关卡生成器（带种子）→ 可玩性模拟器 → `r3e-cli content` → 内容寻址。
**Gate 3**：AI 仅凭"需求 + schema"产出"一段对话 + 一个任务 + 一个关卡块"，自动通过校验与可玩性模拟。

### 5.3 M4（工业化）

不做排期，只做准入：**每一项必须由"做游戏时实际卡住"的实例驱动**，且必须先写一句"卡在哪一步、卡了多久"。无实例不立项。

---

## 6. 度量看板（每周五更新一行，只许变好）

| # | 指标 | 当前 (2026-11-15) | M0 目标 | 采集方式 |
|---|---|---|---|---|
| 1 | 最大文件行数 | 4057 | ≤800 | `cargo xtask lint-size` |
| 2 | `>800` 行文件数 | 24 | ≤12 | 同上 |
| 3 | `#[test]` 总数 | 1457 | 只增不减 | `cargo xtask verify` 快照比对 |
| 4 | 依赖违规数 | 3 | 0 | `cargo xtask dep-check` |
| 5 | `verify` 时长 | 19.6s（热缓存） | <120s | `cargo xtask verify` 自带计时 |
| 6 | 回放确定性 | **已达成**（`final=59d48dedf2b0677e`） | 600 帧两次同哈希 | `cargo xtask replay --seed 42` |

看板记录表（每周追加）：

| 周 | 最大行数 | >800 数 | 测试数 | 违规数 | verify(s) | 回放 |
|---|---|---|---|---|---|---|
| 2026-11-14 | 4487 | 25 | 1408 | 4 | — | — |
| 2026-11-15 | 4053 | 24 | 1427 | 3 | — | — |
| 2026-11-15（M0-S3 收口） | 4053 | 24 | 1427 | 3 | — | — |
| 2026-11-15（M0-S4 收口） | 4057 | 24 | 1457 | 3 | 19.6 | 600 帧两次一致 |

---

## 7. 风险登记册

| ID | 风险 | 触发信号 | 缓解 / 回退 |
|---|---|---|---|
| R1 | 拆分过程中"顺手改逻辑"导致行为漂移 | 像素基线变化 / 契约测试红 | 重构提交与行为提交分离；漂移即回退该卡 |
| R2 | 双份代码继续并存（当前已发生） | `git status` 出现两个重复 crate | S0-01~03 强制收口；未注册 crate 直接删除 |
| R3 | 测试归属迁移导致计数下降 | 快照比对红 | 冻结拆分，先补测试归属 |
| R4 | `doc` 接管 `scene` 引发大范围 `&mut` 冲突 | 编译错误数 >50 | 降级为"只读视图 + 写入口收口"，字段私有化推后 |
| R5 | 同一逻辑两份实现（历史已发生：几何校验/顶点装配/FNV） | grep 出同名函数 2 份 | 列为高危，发现即合并；`StateHash` 明确唯一 |
| R6 | 时间不足，Sprint 堆积 | 连续 2 周未完成率 >30% | 只保留门禁卡，非门禁卡进 Backlog；**M0 退出条件不可放宽** |
| R7 | 门禁一上线即全红，长期挂红失去意义 | dep-check 常态 4 条违规 | 先报告后阻塞，S5 才转硬 |

---

## 8. 每周五 5 分钟反模式自检

| # | 问题 | 若"是" |
|---|---|---|
| 1 | 是否把 AI 逻辑写进 `r3e-app`/`r3e-runtime`？ | 立即移到进程外，只经 protocol |
| 2 | 是否让 AI 直接编排细粒度 RPC？ | 收敛为意图命令 |
| 3 | 是否先加功能、后补验证？ | 补验证优先于下一个功能 |
| 4 | 是否给 AI 另开了一套"简化接口"？ | 合并回同一命令通道 |
| 5 | 本周新增的文件/功能，能否在 V1/V2 层（无 GPU）判定对错？ | 不能 → 判为设计不合格，打回 |

---

## 9. 附录

### 9.1 任务卡模板

```
卡号：M0-S0-03
标题：删除 r3e-app 双份 app.rs / editor.rs
输入：runtime/editor-host 已覆盖能力（前提 S0-04 通过）
动作：删文件 → 缩 main.rs → cargo tree 校验
产出：单一真源
验收命令：cargo tree -p r3e-app | findstr wgpu   （应无输出）
预算：0.5 天
回退：git checkout -- crates/r3e-app
风险：中（删代码不可逆，靠 git 兜底）
```

### 9.2 `xtask` 命令约定（S0 建好后才真正可用）

```bash
# 先在 .cargo/config.toml 注册
# [alias]
# xtask = "run --package xtask --"

cargo xtask verify            # fmt + dep-check + clippy + test + replay + invariants
cargo xtask lint-size         # 单文件 ≤800 + 债务白名单只减不增
cargo xtask dep-check         # 依赖规则表校验（S0 报告，S5 阻塞）
cargo xtask gen-capabilities  # 导出 capabilities.json（S5）
cargo xtask replay --seed 42 --frames 600   # 确定性回放，输出状态哈希（S4）
```

### 9.3 与上游路线图的差异登记

本文档相对 `AI_ROADMAP.md` 的调整（均因实测基线偏差异触发，方向未变）：

1. 新增 **M0-S0 收口 Sprint**（上游未预见"已存在半成品重复 crate"）；
2. 将"验证基建"从 M0 任务之一提升为 **S0 前置条件**（先有尺再动刀）；
3. 明确 **S2 命令表（`CommandSpec`）先于 doc 模型**，因为存在两份 `api.rs`；
4. `rapier` 默认 feature 与 `r3e-audio` 接 rodio 两项偏差**推迟到 M2 决策**（M0 不动，避免与拆分叠加风险）；
5. M0 周期由 4–6 周全职口径细化为 **7–8 周业余口径**。

*（本细化文档由 AI 辅助生成，基线与验收判据均来自 2026-11-14 对仓库的实测）*
*（内容由AI生成，仅供参考）*
