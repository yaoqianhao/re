# M2 计划：游戏可用最小集（串行 45 卡）

- 阶段定位：`AI_ROADMAP §3.3` M2，结构期，原估 6–8 周
- 计划形态：**严格串行**，一卡一提交，前卡 verify 全 11 步绿方可开下一卡
- 基线口径：全部数字取自 `docs/m2/M2_BASELINE.md`（M2-A01 实测，2026-11-20）
- 当前进度：**A01 已完成**（提交 `6876995`）；**A02 已完成**（提交 `810d5ce`，D1–D13 结论见 `docs/m2/M2_DECISIONS.md`）；**A03 已完成**（物理查询端口 trait 化 + `rapier` 转默认后端，实施记录见 `M2_A03_PORT_PLAN.md` §16：tests 1817 → **2017**、`r3e-physics/lib.rs` 1929 → **1542** 行（净减 387，判据 ≤0 满足）、`r3e-core` 零改动、verify 11 步全绿）；**A03b 已完成**（`script` 绑定端口化，能力面落 `r3e-core::host`、适配器落 physics/audio 各自 `host_adapter.rs`，实施记录见 `M2_A03B_HOST_PLAN.md` §9：tests 2017 → **2025**、`dep_debt` 清空且 `limit = 0`、`r3e-script/lib.rs` 2018 → **1979** 行、回放金标未漂移、verify 11 步全绿）；**下一张卡：A04（`scene.rs` 拆域解债）**
- 拍板后果（已计入本文件全部数字）：D1/D2/D12/D13 四项选 B → 卡数 44 → **45**、tests 目标 +546 → **+750**、引擎 crate 16 → **19**、`dep_debt` 2 → **0**
- 日期：2026-11-20

---

## 0. 本目录文件

| 文件 | 作用 | 状态 |
|---|---|---|
| `M2_BASELINE.md` | M2-A01 基线对账、verify 11 步留证、债务台账现状 | 已完成（实测值，唯一权威口径） |
| `M2_PLAN.md` | 本文件：目标、45 卡串行计划、门禁预算、执行规程 | 生效中 |
| `M2_DECISIONS.md` | A02 决策清单 D1–D13，已拍板（含 4 项非建议项的落地约定） | **已拍板**（A02 完成，不再阻塞） |
| `M2_PERF_PLAN.md` | 性能验收口径与跨硬件测试计划（D11 附加要求），H05 按此出结论 | 生效中，M2 全程参考 |
| `M2_A03_PORT_PLAN.md` | A03 物理查询端口实施规划（落点改 `crates/r3e-physics`，2026-11-20 老板指令） | **已确认**（四项决定拍板），A03 可开工 |
| `M2_A03B_HOST_PLAN.md` | A03b `script` 绑定端口化设计稿（trait 默认方法 + 函数式适配器） | **已实施**：§9 为实施记录（5 处偏离：trait 落 `r3e-core`、适配器落各域 crate、`new` 签名改 trait 对象、match 内嵌、tests +8 而非 +56）|
| `M2_PROPOSAL_CAPABILITY_SLOTS.md` | 插件写侧能力槽位提案，提交给 `D:\ai_ide` | 对外提案，**不在 M2 执行链路内**，与本计划无依赖关系 |

阅读顺序：`README.md` → 本文件 → `M2_DECISIONS.md` →（需要核对数字时）`M2_BASELINE.md`。

---

## 1. 目标与判据

来自 `AI_ROADMAP §3.3`，七项，后项依赖前项，**不可并行**。

| 序 | 内容 | 前置 | 判据（可自动判定） | 本计划阶段 |
|---|---|---|---|---|
| 1 | 角色控制器（胶囊/台阶/斜坡/地面吸附） | 物理端口 trait 化完成 | headless 里角色沿斜坡上行高度误差 <2cm | A03 + B |
| 2 | 地形（高度图/分块/材质混合/碰撞） | 1 | 200×200 米地形可由 CLI 生成，哈希可复现 | C |
| 3 | NavMesh + A\* | 1、2 | 给定起终点，路径可通行且长度 ≤ 直线 1.6 倍 | D |
| 4 | Prefab / 实例化 + 覆盖 | 无（排在 3 后保持串行） | 同 prefab 实例 100 个，覆盖字段只存差异 | E |
| 5 | IK + 根运动 + 动画事件 | 1 | 动画事件在固定帧触发，可断言 | F |
| 6 | 场景分块流式加载 | 2、4 | 分块切换不产生帧尖峰（<2ms） | G |
| 7 | `r3e-game` 垂直切片 | 全部 | 可走、可交互、有 NPC、有任务，60fps | H |

**铁律**：每完成 1–2 项做一次「可玩性回归」，不许 7 项做完才第一次试玩 → 落为 B05 / C07 / D06 / F06 / G05 / H05 六张回归卡。

**退出条件**：`r3e-game` 里一个 200×200 米地形场景，玩家可自由行走、NPC 能寻路追击、目标硬件稳定 60fps，且全部可由 CLI 生成与验证。

---

## 2. 基线（引自 M2_BASELINE.md，勿再手改）

| 指标 | 基线值 | 目标值 |
|---|---|---|
| `#[test]` 总数 | 1817 | ≥ 2567（+750） |
| commands | 392 | ≥ 433（+41） |
| rpc | 62 | ≥ 76（+14） |
| script_only | 330 | ≤ 330（不新增仅脚本命令） |
| domains | 17 | 19（+2：terrain、game） |
| invariants | 38 | ≥ 48（+10） |
| 依赖违规 / 债务 | 0 / 2 | 违规 0；债务 **2 → 0**（A03b 偿还后 `limit` 降 0，D13=B） |
| 引擎 crate 数 | 16（+xtask 共 17 成员） | **19**（+xtask 共 20 成员；新增 `r3e-character`、`r3e-terrain`、`r3e-game`） |
| 最大文件 | `r3e-core/src/scene.rs` 3935 | ≤ 1200（A04 拆域后） |
| 次大文件 | `r3e-runtime/src/app.rs` 3028 | ≤ 1200（G02 拆域后） |
| 回放金标 | `final=385482eb033d2964` | 每卡允许推进，但必须两次运行同哈希 |

**两条硬约束**（决定了卡序设计）：

1. **依赖边口径分两段**（D13=B 后不再是"一律顶格"）：
   - **A03b 之前**：`dep_debt` 的 `limit=2` 仍顶格（`script→physics`、`script→audio`），此阶段任何新增跨层边立刻让 `dep-check --strict` 变红。A03 / A04 / A05 不得给 `r3e-script` 加任何依赖。
   - **A03b 之后**：债务清零、`limit=0`，但 `dep_rules.json` 白名单仍是硬门禁 —— 新 crate 不登记即失败（doc_gate 规则 5）。故全部新能力走 **core 契约 + 命令注册**，`r3e-script` 永远不直连新 crate（`r3e-character` / `r3e-terrain` 都不进它的 allow 表）。
2. `app.rs` 3028 行，项 1/6 都要往这里加帧驱动与流式提交，行数会继续涨。故把 app.rs 拆域提前到 **G02**，并规定：G 阶段开始前任何卡在 `app.rs` 新增超 100 行即触发提前拆分。
3. **新 crate 登记三件套**（B01 / C01 / H01 三卡各自必须同卡完成，缺一门禁必红）：`xtask/dep_rules.json`（规则 5）、`xtask/tests_baseline.json`、`workspace Cargo.toml` 成员；文件超 800 行还需 `xtask/size_debt.json`。

---

## 3. 阶段与卡片总览

| 阶段 | 卡号 | 对应项 | 卡数 | 门禁增量（tests / cmd / rpc / inv / domains） |
|---|---|---|---|---|
| A 清障与准备 | A01–A05 + **A03b** | 前置 | **6** | **+256** / 0 / 0 / 0 / 0 |
| B 角色控制器 | B01–B05 | 项 1 | 5 | **+82** / +6 / +2 / +2 / 0 |
| C 地形 | C01–C07 | 项 2 | 7 | **+108** / +8 / +3 / +2 / +1 |
| D 导航 | D01–D06 | 项 3 | 6 | **+82** / +7 / +2 / +2 / 0 |
| E Prefab | E01–E05 | 项 4 | 5 | +58 / +6 / +2 / +1 / 0 |
| F 动画 | F01–F06 | 项 5 | 6 | +62 / +6 / +2 / +1 / 0 |
| G 流式 | G01–G05 | 项 6 | 5 | **+58** / +5 / +2 / +1 / 0 |
| H 垂直切片 | H01–H05 | 项 7 | 5 | **+44** / +3 / +1 / +1 / +1 |
| **合计** | 45 卡 | 7 项 | **45** | **+750 / +41 / +14 / +10 / +2** |

> **卡数演进**（三次变化，逐次留痕）：
> 1. 草案 42 卡 → **44 卡**：新增 `A05`（`size_debt` 收紧，来自 A01 发现）与 `G02`（`app.rs` 拆域提前）。草案引用 `scene.rs 3947` / crate 16 旧口径按 A01 实测修正为 `3935` / `16 个 r3e-* + xtask = 17 个成员`。
> 2. 44 卡 → **45 卡**（A02 拍板后）：D13=B 偿还 `dep_debt`，新插 `A03b`。
> 3. 预算 `+546` → **`+750`**：来自 4 项非建议项的连锁后果（D1 双后端、D2 新 crate、D12 新 crate、D13 偿债卡，以及 D5/D10 可配置、D11 采样脚本）。逐项差额见 `M2_DECISIONS.md`「非建议项影响总览」。
>
> 门禁目标值随之为 `tests ≥2567 / commands ≥433 / rpc ≥76 / invariants ≥48 / domains 19 / 债务 0 / crate 19`。

### 3.1 A 阶段：清障与准备（6 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| A01 | 基线对账与 verify 留证 | — | `docs/m2/M2_BASELINE.md` | 已完成（提交 `6876995`）：11 步全绿、基线唯一、偏差清单 8 条处置 |
| A02 | 决策拍板 D1–D13 | A01 | `docs/m2/M2_DECISIONS.md`、`M2_PERF_PLAN.md` | 已完成：13 项结论全部落盘，4 项非建议项的落地约定写明，计划与预算同步重算 |
| A03 | 物理端口 trait 化（**双后端 + `rapier` 转默认**，D1=B） | A02 | `r3e-physics`（`port.rs` / `port_impl.rs` / `builtin_backend.rs` / `rapier_backend.rs`），`r3e-core` **零改动** | **已完成**（实测见 `M2_A03_PORT_PLAN.md` §16）：capsule / raycast / heightfield 三组 trait **定义落 `crates/r3e-physics/src/port.rs`**（2026-11-20 修订，原「落 core」作废）；`r3e-physics` 两套实现并存；`default = ["rapier"]` 且 `--no-default-features` 回落 builtin 可跑；**同一套契约测试双后端各跑一遍全绿**；`lib.rs` 净增 ≤0（新代码进新文件）；`r3e-script` 不新增依赖边 |
| **A03b** | **`dep_debt` 两条偿还：`script` 绑定端口化**（D13=B） | A03 | `r3e-script`（`host.rs` 新建）、`r3e-runtime`（`script_bind.rs` 新建）、`xtask/dep_debt.json` | 形态 = **trait 默认方法 + 函数式适配器**（`PhysicsHost` / `AudioHost`，签名全脚本原生类型，统一转发到 `execute(Req)->Rsp`，闭包即适配器），见 `M2_A03B_HOST_PLAN.md`；实测范围 **69 个绑定调用点 / 706 行**（物理 24 / 340 行、音频 45 / 366 行），trait 能力面 **53 个去重方法**（物理 16 / 音频 37）；「53 个 / 704 行」与实测是两个维度，非粗估误差，详见 `M2_DECISIONS.md` D13 第 2 条口径校准；`dep_debt.json` 删两条、`limit` 2 → 0；`r3e-script` allow 表保持 3 项不变；**零行为变化**（1817 基线测试零删减、回放金标不漂移；接受 30 处 `ScriptHost::new` 的 import 改由 `r3e-runtime` 再导出）；tests **+56** → 实测 **+8**（2025 合计）｜**已完成**：trait 与 Req/Rsp 落 `r3e-core/src/host/`（`mod.rs` 158 / `physics.rs` 335 / `audio.rs` 510 行），`PhysicsFacade` / `AudioFacade` 落 `r3e-physics|r3e-audio/src/host_adapter.rs`，`ScriptHost` 构造搬 `r3e-script/src/host_wiring.rs`；偏离与理由见 `M2_A03B_HOST_PLAN.md` §9 |
| A04 | `scene.rs` 拆域解债 | A03b | `r3e-core/src/scene.rs` 3935 | 拆为 `scene/` 子模块，最大文件 ≤ 1200 行；`size_debt.json` 删 `scene.rs` 项；1817 基线测试零删减 |
| A05 | `size_debt` 收紧 3 项 | A04 | `xtask/size_debt.json` | `r3e-physics/lib.rs` 1929→≤1924（A03 双后端不得抬高本行数）、`r3e-plugin/tools.rs` 1405→≤1125、`r3e-runtime/app.rs` 3040→≤3028 |

### 3.2 B 阶段：角色控制器（项 1，5 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| B01 | 建 `r3e-character` crate + 控制器契约 + 命令注册（**D2=B**） | A03b | `crates/r3e-character/`、workspace `Cargo.toml`、`xtask/dep_rules.json`、`xtask/tests_baseline.json` | 新 crate 入 workspace（成员 17→**18**）并**同卡**登记 `dep_rules`（`allow = ["r3e-core", "r3e-physics"]`，2026-11-20 修订：原「仅 `r3e-core`」，因 A03 端口落 physics 连带放宽）与 `tests_baseline`；控制器 trait 契约落 `r3e-physics/src/port.rs`（与 A03 同处）、实现落本 crate；**同卡加守卫**：本 crate 源码出现 `r3e_physics::` 后接 `port` 以外路径即 `doc-gate` 失败（补救「物理无关」从编译期保证降为约定守卫）；`character.*` 命令入 `cmd_spec`，`capabilities.json` 生成含新命令；参数（半径/高度/坡度上限/台阶高度/吸附距离）全部可配置；`r3e-script` allow 表**不新增**本 crate；tests +12 |
| B02 | 胶囊碰撞 + 地面吸附 | B01 | 同上 | headless 平面行走不下沉、不掉离地面；吸附距离内贴地；**builtin 与 rapier 双后端各验一遍** |
| B03 | 台阶与斜坡行进 | B02 | 同上 | 台阶高度内可上，超限被挡；斜坡按坡度上限判定可通过/滑落；**双后端各验一遍**（两套后端对斜坡接触判定可能不同，任一套不过即本卡不过） |
| B04 | 判据测试与不变量 | B03 | `crates/r3e-character/tests`、`xtask/invariants` | **斜坡上行高度误差 <2cm 自动断言**（多坡度采样，双后端各断言一次）；新增 2 条 invariant 且三源一致 |
| B05 | 可玩性回归 1 | B04 | `r3e-cli`、回归脚本 | CLI 起一个带斜坡/台阶的场景，脚本驱动角色走完全程，截图或路点日志留证 |

### 3.3 C 阶段：地形（项 2，7 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| C01 | 建 `r3e-terrain` crate + 高度图与块划分（**D12=B**） | B05 | `crates/r3e-terrain/`、workspace `Cargo.toml`、`xtask/dep_rules.json`、`xtask/tests_baseline.json` | 新 crate 入 workspace（成员 18→**19**）并**同卡**登记 `dep_rules`（`allow` 仅 `r3e-core`，不得依赖 render/asset/physics）与 `tests_baseline`；高度图存取、块划分（默认 50 米，D10 可配置）；对外**只产出 core 层数据类型**（块索引 / 高度场 / 材质权重）；tests +12 |
| C02 | 多分辨率 LOD 采样 | C01 | 同上 | 三级 LOD 高度采样误差在阈值内；LOD 切换无空洞 |
| C03 | 分块网格生成 + 材质混合 | C02 | `crates/r3e-terrain/`（生成）+ `r3e-render`（着色） | 块网格三角数受控；≥4 层材质按权重混合，权重和恒为 1；**边界约束**：`r3e-terrain` 不依赖 `r3e-render`，混合着色在 render 侧实现、输入为 core 类型，装配在 runtime 完成 |
| C04 | 地形碰撞 | C03 | `r3e-physics` heightfield | 角色在地形上行走不穿地；`A03` 的 trait 提供 heightfield 查询；**builtin 与 rapier 双后端各验一遍** |
| C05 | CLI 生成与确定性哈希 | C04 | `r3e-cli` | `terrain.gen --size 200x200 --seed 42` 产出一致哈希；同 seed 两次运行逐字节相同 |
| C06 | 判据测试与域登记 | C05 | `r3e-core/tests`、`xtask` | **200×200 米地形可由 CLI 生成且哈希可复现**自动断言；`domains` 增加 terrain |
| C07 | 可玩性回归 2 | C06 | 回归脚本 | 地形场景内行走/爬坡/从高处落下正常，无穿地、无浮空 |

### 3.4 D 阶段：NavMesh + A*（项 3，6 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| D01 | 导航网格烘焙（**D4=A** 编辑期预烘焙） | C07 | `r3e-nav` | 由地形与碰撞几何烘焙出可行走多边形；斜坡/台阶按坡度与高度分类；**双后端**（builtin / rapier）各烘一次，产物哈希须一致——不一致则本卡红并记录差异 |
| D02 | A\* 寻路 | D01 | 同上 | 给定起终点返回可通行路径；不可达返回明确失败码而非空路径 |
| D03 | 路径平滑与代理移动 | D02 | 同上 | 路径经过漏斗算法平滑；代理沿路径行走不切角穿墙 |
| D04 | 命令与能力登记 | D03 | `cmd_spec`、`capabilities.json` | `nav.*` 命令 + rpc 入口；`unsupported` 不残留旧条目 |
| D05 | 判据测试（**D5 默认 + 可配置**） | D04 | `r3e-nav/tests` | **路径长度 ≤ 直线 1.6 倍**自动断言（默认 20 组起终点 / seed 42）；组数、seed、阈值比例抽为可覆盖参数，覆盖后仍按同方向断言；非法配置（0 组 / 阈值 ≤0）必须被拒；不可达场景有反例测试 |
| D06 | 可玩性回归 3 | D05 | 回归脚本 | 地形上放一个跟随目标，代理自行绕障抵达 |

### 3.5 E 阶段：Prefab（项 4，5 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| E01 | Prefab 数据模型 | D06、D6/D7 拍板 | `r3e-core/src/prefab/` | 模板 + 实例 + 覆盖 diff（只存差异字段）三件套；序列化格式冻结 |
| E02 | 实例化与 Scene 集成 | E01 | `scene/` 子模块 | 同 prefab 生成 100 个实例，共享模板数据，内存不随实例数线性膨胀 100 倍 |
| E03 | 覆盖只存差异 | E02 | 同上 | 覆盖 1 个字段的实例序列化体积 < 模板 5%；diff 合并与回退可逆 |
| E04 | 命令、能力与不变量 | E03 | `cmd_spec`、`xtask/invariants` | `prefab.*` 命令 + 1 条 invariant（覆盖字段不得写回模板） |
| E05 | 判据测试与文档 | E04 | `r3e-core/tests`、`docs/ARCHITECTURE.md` | **100 实例覆盖只存差异**自动断言；ARCHITECTURE 补 Prefab 章节 |

### 3.6 F 阶段：IK + 根运动 + 动画事件（项 5，6 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| F01 | 动画事件轨道数据 | E05、D8 拍板 | 动画数据模块 | 事件以帧号存（整数），随片段采样 |
| F02 | 固定帧触发与确定性 | F01 | `r3e-runtime` 动画驱动 | 同一片段在固定帧触发事件，重放两次事件序列一致 |
| F03 | 根运动提取与应用 | F02 | 动画模块 | 根位移从片段提取后驱动实体，不产生滑步（位移与速度积分一致） |
| F04 | 两骨骼 IK（脚部贴地） | F03、D9 拍板 | 动画模块 | 地形起伏下双脚 IK 贴合高度误差 <2cm；与根运动叠加顺序为「先根运动后 IK」 |
| F05 | 判据测试与能力登记 | F04 | `r3e-*/tests` | **事件在固定帧触发且可断言**自动断言；1 条 invariant |
| F06 | 可玩性回归 4 | F05 | 回归脚本 | 角色在地形上行走：脚不悬空、事件驱动的脚步声在正确帧出现 |

### 3.7 G 阶段：分块流式加载（项 6，5 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| G01 | 分块数据结构与边界（**D10 默认 + 可配置**） | F06 | `r3e-core/src/streaming/` | 200×200 米按默认 50 米切 16 块；块边界对象不重复、不丢失；块尺寸 / 驻留半径 / 预取圈数抽为可配置结构（默认 50 米 / 3×3 / 1 圈），非法配置（尺寸不整除、驻留 <1）必须被拒 |
| G02 | `app.rs` 拆域 | G01 | `r3e-runtime/src/app.rs` 3028 | 拆为 `app/` 子模块，最大文件 ≤ 1200 行；`size_debt.json` 删该项 |
| G03 | 异步加载 / 提交 / 卸载 | G02 | `app/` 子模块 | 默认驻留 3×3 策略下，进出块无重复加载；卸载释放资源且不留悬空句柄；驻留半径覆盖为非默认值时行为仍自洽 |
| G04 | 帧尖峰判据 | G03 | `r3e-runtime` 打点 | **分块切换单帧开销 <2ms**（headless 打点，≥50 次切换采样，取 p99） |
| G05 | 命令、能力、测试与回归 5 | G04 | `cmd_spec`、`r3e-*/tests` | 流式相关命令入清单；判据自动断言；回归：跨块行走无卡顿、无丢块 |

### 3.8 H 阶段：`r3e-game` 垂直切片（项 7，5 卡）

| ID | 任务 | 依赖 | 主文件 | 完成判据 |
|---|---|---|---|---|
| H01 | crate 骨架与主循环 | G05、D11 拍板 | `crates/r3e-game/` | 新 crate 入 workspace，帧循环打点可读；`domains` 增加 game |
| H02 | 玩家控制与摄像机 | H01 | 同上 | 接入 B 阶段控制器，第三人称摄像机无穿墙 |
| H03 | NPC 与导航追击 | H02 | 同上 | 接入 D 阶段寻路，NPC 能追击玩家并绕过地形障碍 |
| H04 | 任务/交互最小闭环 | H03 | 同上 | 可交互物件 + 一条任务链（接取→完成→奖励提示），状态机可断言 |
| H05 | 集成 200×200 场景与 M2 收口 | H04 | `r3e-game`、`docs/`、`docs/m2/M2_PERF_PLAN.md` | **退出条件全项验收**：可走、可交互、有 NPC、有任务、目标硬件 60fps；帧性能按 `M2_PERF_PLAN.md` 口径采样（60 秒、p99 ≤16.6ms、profile 留证），并**同卡把跨硬件矩阵的「本机」一列填实**；CLI 一条命令生成并验证整场景；M2 收口文档写入；tests 预算含采样脚本复用 +8 |

---

## 4. 门禁累加预算

预算为**阶段下限**，只增不减；每卡落地后 `check-tests` 只准上升。

| 阶段 | tests | commands | rpc | invariants | domains |
|---|---|---|---|---|---|
| 基线 | 1817 | 392 | 62 | 38 | 17 |
| A（A03 / A03b / A04 / A05） | **+256** | 0 | 0 | 0 | 0 |
| B | **+82** | +6 | +2 | +2 | 0 |
| C | **+108** | +8 | +3 | +2 | +1（terrain） |
| D | **+82** | +7 | +2 | +2 | 0 |
| E | +58 | +6 | +2 | +1 | 0 |
| F | +62 | +6 | +2 | +1 | 0 |
| G | **+58** | +5 | +2 | +1 | 0 |
| H | **+44** | +3 | +1 | +1 | +1（game） |
| **目标** | **2567** | **433** | **76** | **48** | **19** |

A 阶段 +256 的构成（供核对，不能只写总数）：A03 双后端 **+200**（原 +130，D1=B 增 +70）、A03b 端口化偿还 **+56**（D13=B）、A04/A05 各 0（零行为变化 / 纯台账收紧）。
> **实测（2026-11-20）**：A03 落 +200（1817 → 2017）与预算一致；A03b 落 **+8**（2017 → 2025）而非 +56 —— 端口化按「零行为变化」处理，69 个既有绑定测试原样保留（它们现在跑过 Facade，等于顺带覆盖适配器），新增的 8 条只测契约本身。差额不是漏做测试，而是「不重写已有断言」这一判据的必然结果，见 `M2_A03B_HOST_PLAN.md` §9.1 第 5 条。

守恒规则：

1. `tests_baseline.json` 每卡必须同步更新，且数值只升；`cargo xtask check-tests` 是唯一判据（不看 `cargo test` 块求和，差 1 的来源见 BASELINE §6）。
2. 新增命令必须同卡进 `cmd_spec` 并重生成 `capabilities.json`（生成物，禁止手改）。
3. 新增 invariant 必须三源一致：登记表 / 反例测试 / 能力清单各 1。
4. `script_only` 不得增加——任何新命令必须有 rpc 或命令表入口。
5. 每卡 `dep-check --strict` 违规必须为 0；债务条数 **A03b 之前保持 2，A03b 之后必须为 0 且 `limit` 降为 0**（D13=B）。
6. 新增 crate 三件套（`workspace` 成员 / `dep_rules.json` / `tests_baseline.json`）必须**同卡**完成，缺一项 verify 必红（doc_gate 规则 5）。

---

## 5. 单卡执行规程

1. 开工：`git status --short` 必须为空。非空先收尾或 stash，不在脏树上开卡。
2. 范围：一卡只改本卡「主文件」列出的文件族；需要跨界时先改计划文件登记，再动手。
3. 判据先行：先写判据测试（红），再实现（绿），最后过门禁。判据卡（B04/C06/D05/E05/F05/G04）不得省。
4. 门禁：`cargo xtask verify` **全 11 步**，退出码 0 才算过卡。
5. 提交：一卡一提交，信息格式 `feat(m2-XX): <一句话>`；`fmt`/`clippy` 修正在同提交内。
6. 回滚点：每卡提交即可回滚单元；A04、G02 两张重构卡必须保证 `git revert` 后门禁仍绿（拆域不夹带行为变更）。
7. 文档同卡：`AI_ROADMAP §0.2` 现状数字、`ARCHITECTURE.md` 对应章节、本计划「当前进度」随卡更新，不留到收尾补。
8. 失败处置：门禁红 → 就地修；修不动 → `git checkout -- .` 回滚并记录原因到本卡备注，禁止带红进入下一卡。

---

## 6. 文件占用矩阵

| 文件 / 文件族 | 独占阶段 | 约束 |
|---|---|---|
| `r3e-core/src/scene.rs` → `scene/` | A04 | 拆域后不再由其他卡整体重写，只按子模块追加 |
| `r3e-physics/src/*` | A03 | 只加 trait 与 builtin 实现，不改既有检测语义 |
| `crates/r3e-character/*` | B | 新建 crate（D2=B）；`allow` 仅 `r3e-core`；控制器 trait 契约在 core、实现在本 crate；不得依赖 `r3e-physics` |
| `crates/r3e-terrain/*` | C | 新建 crate（D12=B）；`allow` 仅 `r3e-core`；只产出 core 类型，不得依赖 render/asset/physics |
| `r3e-nav/src/*` | D | D 阶段专属 |
| `r3e-core/src/prefab/` | E | E 阶段专属 |
| 动画模块（片段/事件/IK） | F | F 阶段专属 |
| `r3e-core/src/streaming/`、`r3e-runtime/src/app.rs` → `app/` | G | app.rs 拆域在 G02 一次完成 |
| `crates/r3e-game/*` | H | 新 crate，H 阶段专属 |
| `xtask/tests_baseline.json`、`xtask/size_debt.json`、`xtask/dep_rules.json`、`xtask/dep_debt.json`、`docs/capabilities.json` | 全阶段 | 每卡随判据更新；`capabilities.json` 为生成物禁止手改；`dep_debt.json` 只许 A03b 删条（limit 2→0），其余卡不得动 |
| `docs/AI_ROADMAP.md §0.2` | 全阶段 | 只改现状数字，不动历史 Sprint 记录 |

---

## 7. 风险与不做清单

| # | 风险 | 触发条件 | 处置 |
|---|---|---|---|
| R1 | `dep_debt` 顶格，新跨层边即红 | 任何卡想给 `r3e-script` 加新依赖 | 走 core 契约 + 命令注册；确需新边则先开偿还卡（D13） |
| R2 | `app.rs` 行数上涨 | G 阶段前任何卡在 `app.rs` 新增 >100 行 | 立即把 G02 提前到当前卡之后执行 |
| R3 | `scene.rs` 拆域是最大不确定点（3935 行 / 625 测试） | A04 门禁红或测试丢失 | 拆域零行为变更，分批搬移 + 每批跑 verify；必要时回滚重来 |
| R4 | 串行被打破导致后期返工 | 有人并行开两项 | 前卡 verify 未绿不得开下一卡，计划层面不排并行 |
| R5 | 回放金标漂移被误判为失败 | 判据含确定性 | 金标允许随卡推进，但必须两次运行同哈希；不追旧值 |
| R6 | 回归卡被跳过 | 赶进度 | 六张回归卡（B05/C07/D06/F06/G05/H05）不可省，不做则 M2 不算完成 |
| **R7** | **双后端工作量翻倍被低估**（D1=B） | A03 超出 3 天、或 B02/B03/C04/D01 任一卡双后端结论不一致 | 双后端不一致**不糊**：先判定是 builtin 语义缺陷还是 rapier 参数不当，结论写入卡备注；A03 若显著超期，**不得**改用「先缓存 builtin 结果」之类的折中，宁可拉长 A 阶段 |
| **R8** | **新 crate 边界漂移**（D2=B / D12=B） | `r3e-character` 想依赖 `r3e-physics`、`r3e-terrain` 想依赖 `r3e-render`、或 `r3e-script` allow 表被加上新 crate | 一律红线：改走 core 契约/端口注入，或把共享数据结构下沉 core；确需新边必须先改 `dep_rules.json` 并在本计划登记理由，不得在实现里偷渡 |
| **R9** | **A03b 端口化引入行为变更**（D13=B） | 1817 基线测试掉数、回放金标漂移 | 端口化按「零行为变化」重构处理：分批搬移 + 每批 verify；金标漂移必须定位到具体行为差异，不得直接改金标 |
| **R10** | 编译时间上涨影响开发节奏 | `rapier` 默认化后 `cargo test` 明显变慢 | 允许开发期用 `--no-default-features` 跑快速循环，但**提交前与 CI 必须跑默认（rapier）配置全绿** |

**M2 不做**（明确排除，避免范围蔓延）：`r3e-audio` 接 rodio、网络同步、TAA/运动模糊、编辑器 UI、`r3e-flow` 与内容管线（M3）、dedicated server / 多人联机、地形无缝 LOD 裂缝消除（只保证切换无空洞）。

> 已从「不做」中移出的三项（A02 拍板后改为要做）：`rapier` 转默认 feature → **D1=B 在 A03 做**；`r3e-terrain` 独立 crate → **D12=B 在 C01 建**；`dep_debt` 偿还 → **D13=B 在 A03b 做**。

---

## 8. 下一步

1. ~~老板拍板 `M2_DECISIONS.md` D1–D13~~ ✅ 已完成（2026-11-20，13 项全定，4 项非建议项）。
2. ~~A02 落盘拍板结论 → 提交~~ ✅ 已完成（决策落盘 + 计划与预算重算 + `M2_PERF_PLAN.md` 落盘）。
3. **下一张卡：A03 物理端口 trait 化**。开工前必读 D1 的 6 条落地约定（trait 落 core / builtin 保留 / `lib.rs` 净增 ≤0 / 双后端契约测试 / 判据卡双验 / 接受编译时间上涨）。
4. A03 收口后直接进 **A03b**（端口化偿还，`dep_debt` 2 → 0），再 A04 拆域。
