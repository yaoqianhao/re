# M2-A01 基线对账与 verify 留证

- 执行时间：2026-11-20 08:56:53 启动，`Elapsed = 255.6s`，约 09:01:09 结束
- 仓库：`D:\rust3d-engine`，分支 `main`，HEAD `0e1644c`（`docs: 按 M 阶段归档 docs 目录并同步全仓引用，清除 AI 生成水印`）
- 工作树：`git status --short` 空（执行前干净）
- 命令：`cargo xtask verify`
- **结论：EXITCODE=0，11 步全绿，无一条失败项**

---

## 1. verify 11 步实测

| # | 步骤 | 结果 | 实测摘要 |
|---|---|---|---|
| 1 | `cargo fmt --all -- --check` | ✅ | 无 diff |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ | 零警告 |
| 3 | `cargo test --workspace` | ✅ | 126 个 test result 块全部 `ok` |
| 4 | `replay`（600 帧 / 10 采样点） | ✅ | 两次运行哈希完全一致，`final=385482eb033d2964` |
| 5 | `lint-size` | ✅ | 扫描 332 个 `.rs`，阈值 800 行，债务白名单 23 项，无上涨 |
| 6 | `check-tests` | ✅ | 16 个 crate 全部 ≥ 基线，求和 **1817** |
| 7 | `doc-gate` | ✅ | 332 个 `.rs` / **16 个 crate** / 7 条规则 / 白名单 10 项 / 写点 5 crate 234 处 / 0 违规 |
| 8 | `dep-check --strict` | ✅ | 白名单外违规 **0**，债务 2 条（`script→audio` / `script→physics`）均在账 |
| 9 | `gen-capabilities --check` | ✅ | `commands=392 rpc=62 script_only=330 domains=17 invariants=38`，与 `docs/capabilities.json` 逐字节一致 |
| 10 | `invariants` | ✅ | A(登记表)=38 / B(反例测试)=38 / C(能力清单)=38，三源一致 |
| 11 | `scene-verify` | ✅ | 6 个夹具（物理后端 builtin，导航逐场景烘焙）全部符合期望：反例报得出坐标、正向对照干净 |

**耗时构成**：255.6s 为冷态（`xtask` 自身重编译 21.28s + clippy + workspace test + 引擎跑夹具），**不作为门禁判据**；门禁判据只取退出码。热态九道门历史口径 14.6–20.4s 仍成立。

---

## 2. 唯一基线（对账结论）

`check-tests` 为权威口径（逐 crate 比对 `xtask/tests_baseline.json`），`cargo test` 的块求和高 1（126 块合计 1818，含一条不计入 crate 基线的块）——**以 1817 为准**。

| 指标 | 唯一基线值 | 采集命令 | 与 M1 记载 |
|---|---|---|---|
| `#[test]` 总数 | **1817** | `cargo xtask check-tests` | 一致 |
| commands | **392** | `cargo xtask gen-capabilities --check` | 一致（M1-S4 时 388） |
| rpc | **62** | 同上 | 一致（M1-S4 时 58） |
| script_only | **330** | 同上 | 一致 |
| domains | **17** | 同上 | 一致 |
| invariants | **38** | `cargo xtask invariants` | 一致 |
| `ISSUE_CODES` | **38** | `r3e-doc` | 一致 |
| crate 数（workspace 成员） | **17**（16 个 `r3e-*` + `xtask`） | `Cargo.toml` / `doc-gate` | **M1 文档写 14，已过时** |
| `.rs` 文件数 / 总行数 | **332 / 119,404** | `lint-size` + 实测 | 未记载 |
| `>800` 行文件数 | **23**（白名单 23 项，无上涨） | `lint-size` | 一致 |
| 最大文件 | `r3e-core/src/scene.rs` **3935** 行 | `lint-size` | **M1 文档写 3,947，已过时** |
| 依赖违规 / 债务 | **0 / 2** | `dep-check --strict` | 一致 |
| 回放金标 | `final=385482eb033d2964` | `cargo xtask replay --seed 42` | **M1 文档写 15b5652e98b3346c，已过时** |

### 2.1 逐 crate 测试基线（`tests_baseline.json`，实测全等）

| crate | 基线 | 实测 | crate | 基线 | 实测 |
|---|---|---|---|---|---|
| r3e-core | 625 | 625 | r3e-editor-host | 123 | 123 |
| r3e-render | 307 | 307 | r3e-protocol | 75 | 75 |
| r3e-doc | 152 | 152 | r3e-cli | 75 | 75 |
| r3e-script | 132 | 132 | r3e-editor | 72 | 72 |
| r3e-asset | 69 | 69 | r3e-plugin | 47 | 47 |
| r3e-physics | 38 | 38 | r3e-nav | 37 | 37 |
| r3e-audio | 29 | 29 | r3e-runtime | 14 | 14 |
| r3e-app | 11 | 11 | r3e-replay | 11 | 11 |

---

## 3. 口径偏差清单与处置

> A01 只修「现状章节」的硬事实错误；历史 Sprint 记录（`S1/S2/S3 ... 十项全绿` 等当时口径）保留不动。

| # | 位置 | 偏差 | 处置 |
|---|---|---|---|
| 1 | `AI_ROADMAP §0.3` | crate 目录写「14 个」，实测 16 个（漏 `r3e-nav` / `r3e-cli`） | 已修正为 16（含 `xtask` 计 17 个成员） |
| 2 | `AI_ROADMAP §0.2 / §3.1` | `scene.rs` 行数两处不一致：3,935（门禁表）vs 3,947（M1-DOC-FIELDS 待办） | 统一为实测 3935；M1 收尾时为 3,947，M1-S1-D 删除 `rapier` 依赖边后落到 3935 |
| 3 | `AI_ROADMAP §0.2` | 回放金标 `15b5652e98b3346c` 已被 M1 后续变更推进 | 更新为 `385482eb033d2964`（门禁仍绿：两次运行同哈希） |
| 4 | `AI_ROADMAP §0.2` | `verify` 表述为「九道门」/「十项」，代码实测 **11 步**（`scene-verify` 为第 11 步） | 现状章节改为 11 步；M2 计划同步改为「全 11 步」 |
| 5 | `AI_ROADMAP §0.2` | `doc-gate` 记 299 个 `.rs` / 15 crate | 更新为 332 个 `.rs` / 16 crate / 写点 234 处 |
| 6 | `AI_ROADMAP §0.3` | `r3e-protocol` 记「34 测」 | 更新为 75 测 |
| 7 | `M1_S4F_PLAN E11` | 「现 10 步」+ `xtask` 不在 dep-check 扫描范围 | 历史记录保留；现状以本文 §1 为准（dep-check 扫 16 个引用 crate） |
| 8 | 上轮 `M2_PLAN_SERIAL` 草案 | 引用 file-agent 报告的 `scene.rs 3,947` / `crate 16 个含 xtask` 口径；该草案**未落盘**（`docs/m2/` 现有 `M2_PROPOSAL_CAPABILITY_SLOTS.md` 与本文件） | 计划落盘时一律以本文 §2 实测值为准，并把 §5 的 D12/D13 并进决策清单 |

---

## 4. 债务台账现状（M2 开工前）

### 4.1 `size_debt.json`：23 项，**3 项已下降**（门禁提示「可更新白名单」，非失败）

| 文件 | 登记 | 实测 | 差 |
|---|---|---|---|
| `crates/r3e-physics/src/lib.rs` | 1929 | 1924 | -5 |
| `crates/r3e-plugin/src/tools.rs` | 1405 | 1125 | -280 |
| `crates/r3e-runtime/src/app.rs` | 3040 | 3028 | -12 |

> 处置建议：收紧这 3 个 `base` 值（只降不升），归入**后续触及这些文件的卡**（A04 不涉及它们；建议随 F/B 阶段顺带），不在 A01 改门禁阈值。`scene.rs`（3935）与 `app.rs`（3028）是 M2 两个最大的行数债：前者由 A04 拆域清零，后者目前不在 M2 卡内（见 §5 缺口）。

### 4.2 `dep_debt.json`：**2 条仍在账，且 `plan` 标的是「M1」——M1 已收口但未偿还**

| 边 | why | plan（原文） | 现状 |
|---|---|---|---|
| `r3e-script → r3e-physics` | 脚本 API 直连 physics 门面；53 个绑定方法 / 704 行，端口化属架构重排 | 「M1：补 CapabilityHost 物理入口后删条」 | **已偿还**（M2-A03b：能力面 `PhysicsHost` 落 `r3e-core::host`，适配器 `PhysicsFacade`）|
| `r3e-script → r3e-audio` | 同上，音频绑定直连 audio 门面 | 「M1：端口注入音频能力后删条」 | **已偿还**（M2-A03b：`AudioHost` + `AudioFacade`）|

> 这两条是 M2 开工就存在的门禁债，且 `limit=2` 已顶格——**M2 任何新依赖边都会立刻红**。必须在 M2 内排卡或明确顺延，不能悬空。
> **2026-11-20 已清零**（M2-A03b）：两条同时删除，`dep_debt.limit = 0`、`entries = []`。今后没有免死金牌，新边一律改代码或先改 `dep_rules.json`。

---

## 5. 对 M2 计划的回写（喂 A02 的补充决策）

对照 `AI_ROADMAP` 与实测，A01 新发现 **2 个 A02 未覆盖的决策缺口**，已在 `M2_PLAN_SERIAL.md §2` 补为 D12/D13：

| ID | 新决策项 | 依据 | 建议 |
|---|---|---|---|
| D12 | 地形落 `r3e-core` 子模块，还是按 `AI_ROADMAP §crate 规划` 新建 `r3e-terrain` 独立 crate | 路线图 crate 表原文：「`r3e-terrain` \| M2 新建 \| 高度图地形（多分辨率 LOD、样条编辑）\| 独立 crate，不依赖渲染」；但 M2 七项里地形与 `Scene` 强耦合（项 2 判据要求 CLI 生成 + 哈希可复现，项 4/6 要引用块） | **先落 `r3e-core/src/terrain/`**，等项 2 收口、几何边界稳定后再抽 `r3e-terrain`；否则项 4/6 会在两个 crate 间反复搬迁 |
| D13 | `dep_debt` 2 条（script→physics / audio）在 M2 偿还是否占卡 | `limit=2` 顶格，M2 任何新边即红 | M2 期间**不新增跨层依赖**（项 1–7 全部走 core 契约 + 命令注册），两条债显式顺延到 M2 收口后单独立卡偿还，并在 `AI_ROADMAP §0.2` 注明 |

另补一条非决策缺口（不占卡，登记备查）：`r3e-runtime/src/app.rs` 3028 行，M2 项 1/6 都要往这里加帧驱动与流式提交，**行数会继续涨**，A04 只解决 `scene.rs`。若 G 阶段前不处理，`app.rs` 会撞 800 行线的对立面（债务上涨即红）。

---

## 6. 未做 / 待人工确认

| 项 | 说明 |
|---|---|
| `cargo test` 块求和 1818 vs 基线 1817 | 差 1 来自不计入 crate 基线的块（doctest / 集成测试文件），不影响门禁；如需精确到块，A02 后可加一条 `test` 计数守卫 |
| 回放金标漂移原因 | 门禁绿（两次运行一致），但值与 M1 记载不同；本文按实测更新，**未追溯是哪次变更推进的**（grep 提交历史可查，非阻塞） |
| `size_debt` 3 项收紧 | 只记录不下手，交后续触及文件者顺带 |
| A01 之前的散落日志 | 仓库根目录留有 **12 个 `.log`** 文件（`git status` 不显示，未入版本库跟踪），非本卡范围，仅备注 |
