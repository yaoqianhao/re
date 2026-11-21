---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_b4d373eae4d711f19db6525400638852
    ReservedCode1: EkcEe9uXiQ6h76carJ9ubVM7z3RsxR5Y24VmPGQIaqhFoGkgGRauxntq19PFlgKsQglFZKCin8AExU9bmh80zFYhFQYSRXODgHx/JTceY8KJwoHZWUAin+rufwt0lUWs3EGUJ26/JhM+0n+oN+txBT3P6K5xpHCpeZrYD1brW36qIOHbitMCpMN8aK0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_b4d373eae4d711f19db6525400638852
    ReservedCode2: EkcEe9uXiQ6h76carJ9ubVM7z3RsxR5Y24VmPGQIaqhFoGkgGRauxntq19PFlgKsQglFZKCin8AExU9bmh80zFYhFQYSRXODgHx/JTceY8KJwoHZWUAin+rufwt0lUWs3EGUJ26/JhM+0n+oN+txBT3P6K5xpHCpeZrYD1brW36qIOHbitMCpMN8aK0=
---

# M2-B 角色控制器（项 1）· B01–B05 五卡实施规划

> 阶段：M2 · B（角色控制器，roadmap 项 1）
> 前置：A 阶段 6 卡全完（A05 提交 `d53db48`，未 push），`cargo xtask verify` 11 步全绿
> 日期：2026-11-20
> 状态：**方案待拍板**（§9 五条决策 BD-1 ~ BD-5，其中 BD-1 是硬冲突，必须先拍）
> 上游：`M2_PLAN.md` §3.2（B 阶段卡表）、`M2_DECISIONS.md` D1–D13、`M2_BASELINE.md`

---

## 0. 一句话

B 阶段把「物理能查」变成「角色能走」：新建 `r3e-character` crate 承载角色控制器状态机，控制器只经 `r3e-physics::port` 这一个端口取地面与碰撞，用五张卡依次交付「站得住（B02）→ 上得去（B03）→ 量得准（B04）→ 走得完（B05）」，每卡一提交、11 步门禁全绿才开下一卡。

**本阶段不新增后端能力、不改回放路径、不动 `Scene` 写入口**——所有几何事实仍由 A03 端口给出，`r3e-character` 只做决策。

---

## 1. 本阶段交付什么（对卡表判据的逐项映射）

卡表原文（`M2_PLAN.md` §3.2）的完成判据不做改写，本方案只把它翻译成可执行动作：

| 卡 | 判据原文要点 | 本方案落点 |
|---|---|---|
| B01 | 新 crate 入 workspace（17→**18**）；同卡登记 `dep_rules`（`allow=["r3e-core","r3e-physics"]`）与 `tests_baseline`；控制器 trait 契约落 `r3e-physics/src/port.rs`、实现落本 crate；**同卡加守卫**：本 crate 出现 `r3e_physics::` 后接 `port` 以外路径即 `doc-gate` 失败；`character.*` 入 `cmd_spec`，`capabilities.json` 重生成；参数（半径/高度/坡度/台阶/吸附）全可配置；`r3e-script` allow 表**不新增**本 crate；tests **+12** | §4（含新增 `doc_gate` 规则 8） |
| B02 | headless 平面行走不下沉、不掉离地面；吸附距离内贴地；**builtin 与 rapier 双后端各验一遍** | §5（双后端=同一构建内两实例各跑一遍，见 §5.3） |
| B03 | 台阶高度内可上、超限被挡；斜坡按坡度上限判可过/滑落；**双后端各验一遍**（任一套不过即本卡不过） | §6 |
| B04 | **斜坡上行高度误差 <2cm 自动断言**（多坡度采样，双后端各断言一次）；新增 **2 条 invariant** 且三源一致 | §7 |
| B05 | CLI 起带斜坡/台阶场景，脚本驱动角色走完全程，截图或路点日志留证 | §8 |

门禁增量（`M2_PLAN.md` §4 的 B 行）：tests **+82**、commands **+6**、rpc **+2**、invariants **+2**、domains **0**。
其中 **rpc +2 与守恒规则 4 冲突**，见 §9 BD-1（本方案的核心待拍板项）。

---

## 2. 开工前实测真值（2026-11-20，均来自本仓源码而非文档转述）

| 项 | 实测值 | 来源 |
|---|---|---|
| workspace 成员 | **17** | 根 `Cargo.toml` |
| tests 基线 | **2025** | `xtask/tests_baseline.json`（A03b +8 后） |
| 命令表 | **392** 条 / 17 域 | `cmd_spec` 各域 `name:` 计数：anim45 + anim_blend33 + audio45 + env19 + input28 + light18 + meta39 + render36 + scene59 + sim24 + ui46 |
| RPC 方法 | **62** | `r3e-protocol` `RPC_METHODS`；`EngineApi` 方法数同为 62 |
| 脚本注册 | **353 唯一名 / 364 次注册**（11 处重载，16 个域文件） | `r3e-script/tests/cmd_spec_contract.rs` L99/L100/L103 硬断言 |
| `script_only`（= 命令 − rpc） | **330** | 392 − 62 |
| invariants | **38** | `r3e-doc/src/validate/invariants.rs` `ISSUE_CODES` |
| `size_debt` | limit **800**，条目 **20** | `xtask/size_debt.json`（A05 22→20） |
| `dep_debt` | limit **0**，entries **[]** | `xtask/dep_debt.json`（A03b 已清零） |
| 回放金标 | `final=385482eb033d2964`，seed 42 / 600 帧 / hash_every 60 | `xtask/src/main.rs` `REPLAY_ARGS` |
| 端口现状 | `port.rs` **435 行**（11 单测）；`port_impl.rs` **731 行**（双后端契约 69 条） | 源文件 |
| `Physics` 门面 | `Physics::new()`（默认后端，`rapier` 为默认 feature）/ `Physics::with_backend(Box<dyn PhysicsBackend>)` / `BuiltinBackend`、`NullBackend`、`PhysicsFacade` 均公开导出 | `r3e-physics/src/lib.rs` |
| 回放器依赖 | 仅 `r3e-core` + `r3e-doc`，动作集 = `SpawnNode`/`SetTransform`/`SetVisible`/`SetNodeColor`，**不碰物理** | `r3e-replay/src/lib.rs` Doc 头 |
| `doc_gate` 规则数 | **7 条**（无「只准用 port」这条） | `xtask/src/doc_gate.rs` 表头 |

三条对方案有决定性影响的实测结论：

1. **A03 端口已经够用**：`RaycastPort` / `CapsulePort`（`capsule_overlap` + `capsule_sweep`）/ `GroundPort::sample_ground` 三组 trait 正好是角色控制器需要的全部几何输入。B 阶段**不需要**新增物理能力，只需要把「一次移动怎么决策」放在端口之上（§4.3）。
2. **回放金标天然安全**：回放器只依赖 core + doc、动作集里没有任何物理或角色步骤。只要 B 阶段不往 `Scene` 里加「推进即变化」的角色状态，`385482eb033d2964` 不会漂移（§3.3 把它变成硬约束）。
3. **命令面数字是硬编码的**：加 6 条命令会同时触碰 6 处断言/常量（`EXPECTED_COMMANDS` 392、`RPC_METHODS` 62、`EngineApi` 62、editor-host 契约 62×2、editor 契约 ≥62/62、script 契约 353/364/353）。这是 B01 的主要机械成本，不是设计问题（§4.4）。

---

## 3. 总体设计

### 3.1 分层与依赖

```text
            r3e-character（本阶段新建，成员 18）
              │  ├─ 只依赖 r3e-core（数据类型）+ r3e-physics::port（唯一几何入口）
              │  └─ &dyn PhysicsQueryPort（A03 端口，不认后端）
              ▼
    r3e-physics::port  ── port_impl.rs ──► Physics 门面 ──► Builtin / Rapier / Null
              ▲
              │
    r3e-runtime（B05 装配）   r3e-script（**不新增依赖**，卡表硬约束）
```

依赖边只有两条：`character → core`、`character → physics`（且后者在源码层被 `doc_gate` 规则 8 压成「只准用 `port`」）。

**`r3e-script` 的 allow 表不新增 `r3e-character`**（卡表明写）。含义是：脚本侧调角色能力只能走 `character.*` 命令（命令表登记 → `cmd_spec` → 宿主 dispatch），不引入 crate 依赖边。这也是 §9 BD-1 把命令面做全的主要动机——命令面是脚本唯一的合法入口。

### 3.2 `r3e-character` 内部结构（单文件 ≤ 800 行，不新增 `size_debt` 条目）

| 文件 | 职责 | 行数预算 |
|---|---|---|
| `src/lib.rs` | crate 门面、`pub use`、模块文档（「角色为何不写场景」） | ≤ 180 |
| `src/config.rs` | `CharacterConfig`（半径/高度/坡度上限/台阶高度/吸附距离/最大步长）+ 校验（非法值返回 `Err`，不 panic） | ≤ 240 |
| `src/contract.rs` | 控制器对物理的最小需求抽象：`CharacterSolver` trait（`step(config, state, intent, port) -> MoveOutcome`）+ DTO 转接 | ≤ 220 |
| `src/state.rs` | `CharacterState`（位置/竖直速度/着地标志/上一帧地面法线）+ 状态推进（纯函数，无 IO） | ≤ 240 |
| `src/step.rs` | 单帧步进实现：水平扫掠 → 台阶探测 → 斜坡判定 → 地面吸附 → 结果汇总 | ≤ 400 |
| `tests/*.rs` | 见各卡测试明细 | 分散 |

关键约束：**单文件 ≤ 800 行**（`lint-size`），新增文件一律不得进 `size_debt.json`（该表只许减）。

### 3.3 为什么角色状态不回写 `Scene`（本次定死的硬约束）

D2 落地约定第 4 条要求「`Scene` 侧新增的角色相关数据结构落 `r3e-core`」。本方案严格区分两件事：

* **`CharacterSpec`（配置，进 `Scene`）**：半径、高度、坡度上限、台阶高度、吸附距离。**静态数据**，改动走命令 → 事务 → 可撤销，`StateHash` 覆盖它。
* **`CharacterState`（运行态，不进 `Scene`）**：当前位置、竖直速度、着地标志。**每帧变化**，由 `r3e-character` 持有、由 `r3e-runtime` 装配持有，**不落场景、不进 `StateHash`**。

理由三条：

1. **回放金标**：回放器每帧只做 `time += DT; frame += 1` 与随机命令。若角色运行态进 `Scene`，`StateHash` 就会随物理步进变化 → `385482eb033d2964` 必漂移 → 11 步门禁第 4 步红。这是**本次唯一不可让步的技术约束**。
2. **`doc_gate` 规则 7**：`Scene` 写点有台账（`.write(` 计数），运行态写场景等于每帧新增写点，台账只许减 → 必然冲突。
3. **确定性**：运行态与场景真值解耦后，「同一配置 + 同一输入序列 → 同一轨迹」只在 `r3e-character` 内部成立，可用纯 headless 测试断言（B02/B03/B04 全靠这条）。

因此：`r3e-character` 源码内 `.write(` 出现次数必须为 **0**（B01 验收项，由 `doc_gate` 规则 7 自然看守）。

### 3.4 确定性口径（五卡共用）

* 固定步长：`1/60` 秒，与 `r3e_runtime::HEADLESS_FIXED_DT`、`r3e_replay::REPLAY_DT` 同值；`r3e-character` 内**自定义同名常量**并在注释里交叉引用（不引 runtime，避免把 render/audio 拖进依赖图，与回放器处理方式一致）。
* 步进顺序固定为：**水平扫掠 → 台阶抬升试探 → 斜坡坡度判定 → 重力积分 → 地面吸附 → 落地判定**。顺序变即视为行为变更，必须在提交信息里写明。
* 浮点比较一律用绝对容差（`1e-4` 量级），**禁止**用 `==` 比位置；判据阈值（2cm）以 `0.02` 常量暴露。
* 输入为 `MoveIntent { wish_dir: Vec3 (XZ 平面，未归一化), jump: bool }`；`wish_dir.y` 一律被丢弃（角色不自己飞）。

---

## 4. B01 建 crate + 契约 + 命令注册 + 防走私守卫（tests +12）

### 4.1 文件落点与行数预算

| 文件 | 动作 | 预算 |
|---|---|---|
| `crates/r3e-character/Cargo.toml` | 新建 | — |
| `crates/r3e-character/src/{lib,config,contract,state,step}.rs` | 新建 | ≤ 1280 合计（§3.2） |
| `crates/r3e-character/tests/{config,contract,state}.rs` + `tests/common/mod.rs` | 新建，**+12** | 各 ≤ 400 |
| `Cargo.toml`（根） | `members` 17 → **18** | — |
| `xtask/dep_rules.json` | 新增条目 `allow = ["r3e-core", "r3e-physics"]` | — |
| `xtask/tests_baseline.json` | 新增 `r3e-character: 12`，`total` 2025 → **2037** | — |
| `crates/r3e-physics/src/port.rs` | 追加 `CharacterPort` + `MoveIntent`/`MoveOutcome` DTO（435 → ≤ 590 行） | ≤ 160 追加 |
| `crates/r3e-physics/src/port_impl.rs` | 若 `CharacterPort` 走默认实现则**零改动**（§4.3） | 0 |
| `crates/r3e-core/src/scene/character.rs` | 新建 `CharacterSpec`（D2 约定第 4 条；A04 已拆域，**按子模块追加**合 §6 矩阵） | ≤ 200 |
| `crates/r3e-core/src/cmd_spec/character.rs` | 新建 6 条命令登记（`Domain::Physics`，域不新增） | ≤ 220 |
| `crates/r3e-core/src/cmd_spec/mod.rs` | `mod character;` + `all()` 拼接（按域文件名序，`character` 在 `anim` 之前 —— **顺序变化会让快照类断言全红**，需在提交信息注明） | ≤ 5 行 |
| `crates/r3e-script/src/api/character.rs` | 新建 6 个 `register_fn` + `api/mod.rs` 挂载 | ≤ 240 |
| `crates/r3e-protocol/src/api.rs` | `EngineApi` 62 → 68（新增 6 方法） | ≤ 60 追加 |
| `crates/r3e-protocol/src/dispatch/character.rs` | 新建 dispatch 臂 | ≤ 200 |
| `crates/r3e-editor-host/src/api/character.rs` | 宿主实现（`LocalEngineApi`） | ≤ 220 |
| `xtask/src/doc_gate.rs` | **新增规则 8**（§4.5） | ≤ 70 追加 |
| `docs/m0/M0S3_DOC_LAYER.md` | §5 规则表加第 8 条 | — |

B01 同时触及 4 个 crate + xtask + 文档，是本阶段唯一「宽」卡。**风险点：卡表主文件列只写了 `crates/r3e-character/`，本方案额外登记 `r3e-core`（spec + cmd_spec）、`r3e-physics`（port 追加）、`r3e-script`/`r3e-protocol`/`r3e-editor-host`（命令面）三族**——按 §5.2「需要跨界时先改计划文件登记，再动手」，这一步在 B01 开工前必须先回写 `M2_PLAN.md` §3.2 主文件列（见 §14）。

### 4.2 crate 三件套（缺一项 `verify` 必红，`doc_gate` 规则 5）

```jsonc
// xtask/dep_rules.json 新增条目（键按字典序，插在 r3e-cli 之前）
"r3e-character": {
  "allow": ["r3e-core", "r3e-physics"],
  "why": "角色控制器：只取 core 的数据类型与 physics::port 的几何查询（doc_gate 规则 8 把 physics 依赖收窄到 port）"
}
```

```jsonc
// xtask/tests_baseline.json（_comment 追加一段理由，total 同步）
"r3e-character": 12
```

三件套 = ① 根 `Cargo.toml` `members` ② `dep_rules.json` 条目 ③ `tests_baseline.json` 条目。B01 提交必须是**一次**提交同时含三者，否则 `check-tests` / `dep-check` / `doc-gate` 三处齐红。

### 4.3 契约：`CharacterPort` 追加在 `port.rs`

卡表要求「控制器 trait 契约落 `r3e-physics/src/port.rs`（与 A03 同处）」。落法如下（**最小侵入**，不给三套后端加实现负担）：

```rust
// crates/r3e-physics/src/port.rs 追加（约 160 行，含 3 条自测）

/// 角色控制器一次移动的意图。`wish_dir` 只用 XZ 分量（Y 被丢弃）。
pub struct MoveIntent { pub wish_dir: Vec3, pub jump: bool }

/// 一次移动的结算结果：调用方拿它更新自己的运行态，不涉及后端状态。
pub struct MoveOutcome {
    pub position: Vec3,        // 结算后的胶囊中心
    pub grounded: bool,        // 是否着地（吸附成功或竖直扫掠命中）
    pub ground_normal: Vec3,   // 着地面法线；未着地时全零
    pub blocked: bool,         // 水平方向被挡（含台阶超限）
    pub step_up: f32,          // 本帧实际抬升高度（0 = 未抬）
}

/// 角色控制器需要的全部物理能力。
///
/// 与 `RaycastPort` / `CapsulePort` / `GroundPort` 的关系：本 trait 是**组合 + 默认实现**，
/// 后端只要满足那三组查询就自动满足本 trait，不需要各写一遍。
pub trait CharacterPort: PhysicsQueryPort {
    /// 水平推进：先扫掠，被挡则尝试台阶抬升，抬升后仍被挡则 `blocked = true`。
    fn move_horizontal(&self, capsule: &Capsule, dir: Vec3, dist: f32,
                       max_step: f32) -> (Vec3, bool) { /* 默认实现：capsule_sweep + 试探 */ }
    /// 竖直结算：向下扫掠 → 命中则贴地，否则按重力下落；`snap` 是吸附距离。
    fn settle_vertical(&self, capsule: &Capsule, vy: f32, dt: f32,
                       snap: f32) -> (Vec3, bool, Vec3) { /* 默认实现 */ }
}
```

三条设计口径：

1. **默认实现即全部**：`impl CharacterPort for Physics {}`（空实现，一行）就够——因为默认方法只用 `capsule_sweep` / `capsule_overlap` / `sample_ground`，而这三个已经在 `port_impl.rs` 对 `Physics` 实现好了。**`port_impl.rs` 零改动**，三套后端（Builtin/Rapier/Null）自动满足。
2. **DTO 放 physics 不放 character**：`r3e-character` 依赖 physics 是被允许的（D2），反方向不存在；放 port.rs 也满足卡表「契约落 port.rs」的原文。
3. **`Null` 后端行为**：`sample_ground` 返回 `Some(GroundSample::flat(0.0))`（A03 口径），因此空后端下角色站在 y=0 平面上——B02 的「不下沉」用例可以拿 `Null` 当对照组，不必开 rapier。

若老板选 BD-2 档 B（契约留 `r3e-character`、`port.rs` 零改动），则本节全部删除、卡表 B01 判据的「契约落 port.rs」改为「契约落 `r3e-character/src/contract.rs`」，代价是 `r3e-character` 需要自己写一份 `CharacterPort` 而 physics 侧不再具有「后端已满足角色需求」的自我描述能力。

### 4.4 命令面（6 条）与硬编码数字的机械改动

六条命令（全部带 RPC，理由见 §9 BD-1 档 A）：

| 命令 | RPC 方法 | 域 | 参数 | 返回 | 语义 |
|---|---|---|---|---|---|
| `character_create` | `character.create` | Physics | `&str`（node 名）、`f64`×5（半径/高度/坡度/台阶/吸附） | `bool` | 给节点挂 `CharacterSpec`（写场景，可撤销） |
| `character_config` | `character.config` | Physics | `&str` + `f64`×5 | `bool` | 改配置（参数非法返回 `false` 且不改） |
| `character_step` | `character.step` | Physics | `&str`、`f64`×3（wish x/z、dt） | `()` | 推进一帧（headless 也走这条） |
| `character_state` | `character.state` | Physics | `&str` | `&str`（JSON：pos/grounded/vy） | 读运行态（只读） |
| `character_teleport` | `character.teleport` | Physics | `&str`、`f64`×3 | `bool` | 强制置位（回归脚本定位用） |
| `character_stats` | `character.stats` | Physics | — | `&str` | 控制器计数（步数/着地帧/被挡帧） |

必须同步的硬数字（6 处，缺一处 `cargo test` 红）：

| 位置 | 现值 | 改为 |
|---|---|---|
| `xtask/src/gen_capabilities.rs` `EXPECTED_COMMANDS` | 392 | **398** |
| `r3e-protocol` `RPC_METHODS` 条数 | 62 | **68** |
| `r3e-protocol/src/api.rs` `EngineApi` 方法 | 62 | **68** |
| `r3e-editor-host/tests/cmd_spec_contract.rs` L97/L99 + Doc 头 L1 | 62 / 62 / 62 | **68** |
| `r3e-editor/tests/cmd_spec_contract.rs` L99（下限）/ L144（等值） | 62 / 62 | **68** |
| `r3e-script/tests/cmd_spec_contract.rs` L99/L100/L103 | 353 / 364 / 353 | **359 / 370 / 359** |

`domains` 保持 **17**（`character.*` 挂 `Domain::Physics`，**不新增域**）→ B 阶段 `domains +0` 预算成立。
`script_only` 330 不变（6 条全带 rpc）→ 守恒规则 4 成立。

### 4.5 `doc_gate` 规则 8：把「只准用 port」从约定变成门禁

卡表要求：本 crate 源码出现 `r3e_physics::` 后接 `port` 以外路径即 `doc_gate` 失败。实现方式：

```rust
// xtask/src/doc_gate.rs 新增（规则 8）
// | 8 | `r3e-character` 只能通过 `r3e_physics::port` 使用物理 | 无 |
//
// 判据：扫描 crates/r3e-character/**/*.rs，每处 `r3e_physics::` 之后
//   必须紧跟 `port`（允许 `r3e_physics::port::{...}` 与 `use r3e_physics::port as p`），
//   出现 `r3e_physics::Physics` / `::BuiltinBackend` / `::RapierBackend` 等一律失败。
//   注释**也**算违规（BD-5 已拍板档 A）：本规则**不**调用 strip_comments，逐行整行
//   匹配，doc 注释与被注释掉的代码一视同仁——一条错误注释会带偏后来人。
//   与规则 1/3 的差异是刻意的：那两条怕误报，这条怕误导。
```

连带动作：
* `docs/m0/M0S3_DOC_LAYER.md` §5 规则表追加第 8 条（**必须同卡**，否则规则无出处）。
* `doc_gate` 运行摘要行「规则 N 条」7 → 8（该行是 `println!`，无断言，但有测试快照则需同步——实测无断言，安全）。
* 规则 8 只对 `crates/r3e-character/` 生效，其他 crate 不受影响（`r3e-runtime` 可以正常用 `Physics` 门面）。

**B02 实测偏差（2026-11-21）**：规则 8 的范围从 `crates/r3e-character/` 收窄为 `crates/r3e-character/src/`——本 crate 的 `tests/` 豁免。
原因不是放宽，而是 B02 的判据本身要求「builtin 与 rapier 双后端各验一遍」，装配真后端只能写 `r3e_physics::{Physics, PhysicsKind}`；不给测试口子就只剩一条腿，判据无法成立。
`src/` 侧一字未松：现在 `crates/r3e-character/src/` 里仍然只出现 `r3e_physics::port`。

### 4.6 参数可配置面（卡表「全部可配置」的落地）

`CharacterConfig` 六个字段全部可配，默认值写死在 `Default`，可在运行期经 `character.config` 覆盖：

| 字段 | 默认 | 合法域 | 非法处置 |
|---|---|---|---|
| `radius` | 0.3 | (0, 10] | `Err`，命令返回 `false` |
| `height` | 1.8 | `> 2*radius`，≤ 10 | 同上 |
| `max_slope_deg` | 45 | (0, 89] | 同上 |
| `max_step` | 0.35 | [0, 2] | 同上 |
| `snap_distance` | 0.3 | [0, 1] | 同上 |
| `move_speed` | 5.0 | (0, 100] | 同上 |

非法参数**不得 panic、不得静默截断**（B01 的三条配置测试就是打这个）。这与 `M2_PLAN` §3 的「D5 默认 + 可配置」口径一致：默认值可跑，非法配置必须被拒。

### 4.7 B01 测试明细（+12，正好等于 `tests_baseline` 增量）

| # | 文件 | 断言 |
|---|---|---|
| 1-3 | `tests/config.rs` | 默认值合法且 `height > 2*radius`；六个字段各自越界被拒（表驱动 1 条测不完，拆 3 条：负值/零值/超上限）；`config` 覆盖后原值不残留 |
| 4-5 | `tests/contract.rs` | `Physics`（rapier 默认后端）满足 `CharacterPort`（空 impl 可编译 + 默认方法可调）；`BuiltinBackend` 包装的 `Physics` 同样满足 |
| 6-7 | `tests/contract.rs` | `move_horizontal` 在空后端无障碍时位移 = `dir * dist`（误差 <1e-4）；被挡住时 `blocked = true` 且位移 < dist |
| 8-9 | `tests/state.rs` | 初始状态 `grounded = false`、`vy = 0`；`settle_vertical` 首次调用后 `grounded = true` 且 `vy` 归零 |
| 10-11 | `tests/state.rs` | `wish_dir` 的 Y 分量被丢弃（传 (1,99,0) 与 (1,0,0) 轨迹相同）；零 `wish_dir` 不动位置 |
| 12 | `tests/state.rs` | `MoveOutcome::step_up` 在平地上恒为 0（台阶行为留 B03 断言） |

「+12 = 12 条 `#[test]` 函数」——`check-tests` 按 crate 统计，不按断言数，故上表每行必须是一个独立 `#[test]`。

### 4.8 B01 验收清单

1. `cargo xtask verify` 11 步全绿（尤其：`dep-check` 违规 0 且债务 0；`doc-gate` 8 条规则 0 违规；`check-tests` = 2037；`gen-capabilities --check` 通过且清单含 6 条 `character.*`；`replay` 金标 `385482eb033d2964` 不变）。
2. `git status --short` 为空、单提交、信息 `feat(m2-B01): 建 r3e-character crate 并登记 character 命令面`。
3. `size_debt.json` 条目仍为 20、`r3e-character` 不在其中。
4. `r3e-script` 的 `Cargo.toml` 依赖**未变**（allow 表不新增本 crate）。

---

## 5. B02 胶囊碰撞 + 地面吸附（tests +20）

**目标**：角色站在平地上不下沉、不掉离地面，吸附距离内贴地。**不涉及台阶与斜坡**。

### 5.1 实现要点（`src/step.rs`）

单帧 `step()` 顺序（§3.4 口径）：

1. `move_horizontal(capsule, wish_dir_normalized * speed, dt*len, max_step)` → 新位置 + `blocked`（B02 阶段 `max_step` 传 0，台阶行为留 B03）。
2. 重力积分：`vy -= g*dt`（`g = 9.81`，常量可配但默认锁定）。
3. `settle_vertical(capsule, vy, dt, snap_distance)` → 新位置 + `grounded` + `normal`。
4. `grounded` 时 `vy = 0`，否则保留 `vy`。

### 5.2 判据到断言的翻译

| 卡表判据 | 断言（双后端各一条） |
|---|---|
| 水平面行走**不下沉** | 走 60 帧（1 秒）后 `position.y` 与初始差 < `1e-3` |
| **不掉离地面** | 全程 `grounded == true`（逐帧采样，0 次 false） |
| **吸附距离内贴地** | 初始位置抬高 `0.9 * snap_distance` 后单帧结算 → `grounded == true` 且 `|y − 地面高度| < 1e-3` |
| 吸附距离外不硬拉 | 初始位置抬高 `1.5 * snap_distance` → 首帧 `grounded == false`（在下落），且 `y` 单调下降 |
| 空后端对照 | `Null` 后端下 `sample_ground` 返回 `flat(0.0)`，角色站 y=0 |

### 5.3 「双后端各验一遍」的具体做法（B02/B03 共用）

`verify` 的 `cargo test --workspace` 跑**默认 features**（`rapier` 已是默认）。`BuiltinBackend` 与 `NullBackend` 都在默认构建内公开可用，因此双后端验证**不需要**新增 xtask 步骤、不需要 `--no-default-features` 二次构建：

```rust
// tests/common/mod.rs（与 A03 的 tests/common 同风格）
pub fn physics_of(kind: BackendKind) -> Physics { /* Rapier → Physics::new(); Builtin → with_backend(BuiltinBackend); Null → with_backend(NullBackend) */ }

// 每个用例用同一份断言函数跑三遍（rapier / builtin / null 中的前两者进判据）
macro_rules! both_backends { ($f:ident) => { #[test] fn $f() { run_on_rapier($f); run_on_builtin($f); } } }
```

> A03 的落地形态（`port_impl.rs` 的 69 条契约测试）已经是这个模式，B02 直接沿用，不重新发明。

### 5.4 测试明细（+20）

10 个断言点 × 2 后端 = 20 条 `#[test]`（每后端 10 条：下沉/不掉地/吸附内/吸附外/无重力时不动/低帧率下仍贴地/斜向行走 Y 不变/dt=0 幂等/巨大 dt 不穿透/被墙挡时仍着地）。

### 5.5 B02 实到（2026-11-21）

* 落点：`crates/r3e-character/tests/ground_spec.rs`（20 条，10 判据 × 双后端）、`tests/common/mod.rs` 补 `backends()`/`physics_of()`/`floor_world()`/`with_wall()` 真后端夹具（解析夹具仍留给 B01 的几何恒等式用）。
* 基线：`r3e-character` **18 → 38**（`tests_baseline` 已同步），全量 2062 → 2082。
* 判据实测：Rapier 与 Builtin 各 10 条全绿；行走 60 帧 `position.y` 与初值差 < `1e-3`，`grounded` 逐帧为真 0 次 false。

**同卡修掉 `CharacterPort` 默认实现的两处真实缺陷**（判据与阈值一条未放宽，属端口实现修正，不是调测试）：

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 1 | 角色**一落地就迈不开腿**：落地后每帧 `blocked = true`、`step_up = 0`、位置不动（两个后端都复现） | 站稳瞬间胶囊底面与地面零间隙，`capsule_sweep` 把「贴着地面」判成 `started_inside`，而 `move_horizontal` 的 `started_inside ⇒ 拒动` 分支直接把整个位移吃掉 | 扫掠前先把起点上抬 `SKIN` 脱开；抬了**仍与几何重叠**才判定「真卡在墙里」并拒动。结果里的 y 以原始中心为基准回写，抬升量不外泄 |
| 2 | 站着**缓慢下沉**：Rapier 上 60 帧沉 1.8 cm；同一帧既可能报 0.6 mm 命中、也可能「无命中」后者按自由落体下移 | `settle_vertical` 从贴地位置直接向下扫，恰好踩在 Rapier 的预测距离边界上，结论不稳定 | 向下探测一律把起点上抬 `SKIN`、扫 `reach + SKIN`，再把命中距离减掉 `SKIN`——与不抬等价，但不踩边界；起点上抬后仍 `started_inside` 时不下移 |

`BuiltinBackend` 是解析法、不会出现上面两处边缘，所以这两条缺陷只在真后端上暴露——这正是计划 §5.3 坚持「双后端各验一遍」的价值所在。

---

## 6. B03 台阶与斜坡行进（tests +20）

**目标**：台阶高度内可上、超限被挡；斜坡按坡度上限判可通过/滑落。

### 6.1 实现要点

`move_horizontal` 的默认实现（B02 已铺好骨架，B03 打开 `max_step`）：

1. 水平 `capsule_sweep(dir, dist)` 命中 → 尝试「抬升 `max_step` 后再扫」。
2. 抬升后仍命中 → `blocked = true`，位置不动（**不下滑**）。
3. 抬升后通过 → 记录 `step_up = 实际抬升量`，随后由 `settle_vertical` 自然落回台阶面。
4. 斜坡判定不看几何角，只看 `GroundSample.up_dot`：`is_walkable(min_up_dot)`，`min_up_dot = cos(max_slope_deg)`。不可走 → 视为滑落面：`grounded = true` 但 `wish_dir` 沿坡向的分量被丢弃（本卡只做「不可上行」，真滑动留 F 阶段或后续，见 §13）。

### 6.2 测试明细（+20，5 个断言点 × 2 后端 × 2 变体）

| 断言点 | 变体 |
|---|---|
| 台阶 `0.9 * max_step` → 可上，`step_up > 0` 且最终 `y` 抬升 | 上/下各一 |
| 台阶 `1.1 * max_step` → `blocked = true`，位置与 y 均不变 | 上/下各一 |
| 斜坡 `0.9 * max_slope_deg` → 可上行（沿坡走 60 帧 y 上升 > 0） | 双向 |
| 斜坡 `1.1 * max_slope_deg` → 不可上行（走 60 帧 y 不升，`blocked == false` 但无进展） | 双向 |
| `max_step = 0` → 任何台阶都挡（回归 B02 默认行为） | 单侧 ×2 |

> 卡表原话「两套后端对斜坡接触判定可能不同，任一套不过即本卡不过」——测试用 `both_backends!` 宏，任一后端 panic 即整条测试红，天然满足。

### 6.3 风险：后端差异

`BuiltinBackend` 的 `capsule_sweep` 是解析法、`RapierBackend` 是 CCD 数值法，两者在阶梯边缘可能出现亚毫米级差异。处置：**判据用容差不用等值**（位置差 ≤ `1e-3`、坡度判定用 `up_dot` 而非角度）、差异超限则在本卡备注记录实测差值，不调低阈值。

---

## 7. B04 判据测试与不变量（tests +18，invariants +2）

### 7.1 「斜坡上行高度误差 <2cm」自动断言

判据：角色沿坡面直线行走 N 步（N=120，2 秒），每 10 步采样一次「实测高度」与「按坡面解析解析应得高度」之差，**全部采样点 < 0.02**。

* 多坡度采样：`5° / 15° / 30° / 40°` 四档（均 < 默认 45° 上限）。
* 双后端各断言一次 → 4 档 × 2 后端 = 8 条测试。
* 容差常量 `RAMP_HEIGHT_TOL: f32 = 0.02` 暴露在 `tests/common`，任何放宽都需改本方案。
* 断言失败时打印「坡度 / 步数 / 实测 / 期望 / 差值」，避免只报一个 bool。

### 7.2 新增 2 条 invariant（三源一致）

**落法（BD-3 已拍板档 A + 新增枚举变体）**：码登记在 `r3e-doc/src/validate/invariants.rs`，校验实现在 `r3e-doc/src/validate/character.rs`（新层，`layer = "character"`），反例测试落 `crates/r3e-doc/tests/`。

两个**不可省的连锁改动**（实测补充，原稿漏项）：

1. `Category` 枚举要加 `Character` 变体：`Category::from_code()` 按 code 首段映射，`ref/material/support/overlap/nav` 之外一律 `_ => Bounds`——不扩枚举的话 `character.*` 会被静默归成 `bounds` 族，破坏 `mod.rs` §79 注释写明的「前缀与族一一对应」契约。
2. 扩枚举会连锁到 `r3e-editor-host/src/verify/mod.rs::layer_of()`：该 match 穷尽，编译即报错，须显式把 `Category::Character` 归到 `Layer::Static`；官方 `tests/verify_layers.rs` 同步加断言。

为什么档 B（码由 `r3e-character` 上报）连试都不要试：`xtask/dep_rules.json` 里 `r3e-doc` 的 allow 只有 `["r3e-core"]`，注释写明「文档层只许依赖 core（S3 立项）」。让 `r3e-doc` 依赖 `r3e-character` 会直接撞 `dep-gate` 红卡，过卡只能改 dep_rules——那是**放宽门禁铁律**，不是改代码。

注：`layer` 字段只是 `ISSUE_CODES` 登记表的文档性元数据（`Issue` 结构无此字段），归因分类走 `Category`，两者不要混。

| # | code | layer | severity | 判据 | 反例夹具 |
|---|---|---|---|---|---|
| 1 | `character.config.invalid` | character | Error | `CharacterSpec` 任一字段越界（半径 ≤0 / 高度 ≤ 2r / 坡度 ∉ (0,89] / 台阶 ∉[0,2] / 吸附 ∉[0,1]）即报出，`location` 带节点名 | 场景 JSON 内手写非法 spec |
| 2 | `character.ground.penetration` | character | Error | 角色的胶囊底部低于其支撑面高度超过 `snap_distance`（即已穿地且吸附拉不回） | 场景内节点 z 位置埋进支撑面 0.5m |

为什么不用「角色轨迹类」码：`ISSUE_CODES` 的语义是**场景自相矛盾**（静态可判），运行态问题不属于校验器职责——这与 B04「判据测试」分离、运行态由 `r3e-character` 自己的断言看守是一致的。

三源一致性（`xtask invariants` 看守）：

| 源 | 落点 |
|---|---|
| A 源 登记表 | `ISSUE_CODES` 38 → **40** |
| B 源 反例测试 | `crates/r3e-doc/tests/character_issues.rs` 2 条（`xtask/invariants.rs` 的 `TEST_DIRS` 已含 `r3e-doc/tests`，**零 xtask 改造**） |
| C 源 能力清单 | `docs/capabilities.json` 重生成（`gen-capabilities`，禁止手改），`invariants` 计数 38 → **40** |

### 7.3 测试明细（+18）

| 组 | 条数 |
|---|---|
| 斜坡 2cm 断言（4 坡度 × 2 后端） | 8 |
| `character.config.invalid` 正/反例（合法不报 / 5 个字段各报 → 6 条，压成 1 条表驱动 + 1 条合法静默 = 2） | 2 |
| `character.ground.penetration` 正/反例 | 2 |
| ISSUE_CODES 表自检（新增 2 条存在、layer 拼写、`from_code` 命中 `Character`、未知前缀仍落 `Bounds`） | 1 |
| 三源一致（码在清单里可查、反例测试名匹配） | 1 |
| 采样器本身（0 步不报、1 步报差值） | 2 |
| 轨迹确定性（同输入两次跑逐位相同） | 2 |
| **合计** | **18** |

### 7.4 B03 / B04 实到（2026-11-21）

**落点**：`crates/r3e-character/tests/step_spec.rs`（B03，20 条）、`tests/ramp_accuracy.rs`（B04 §7.1，8 条）、
`tests/ramp_sampler.rs`（B04 采样器与确定性，4 条）、`tests/common/mod.rs` 补坡度夹具与 2cm 容差常量；
`r3e-doc/src/validate/character.rs`（新层，6 条单测）、`r3e-doc/tests/character_issues.rs`（两条码的正反例，6 条）、
`r3e-editor-host/tests/verify_layers.rs`（归层，2 条）；`r3e-physics/src/slope.rs` 前缘对齐口径 1 条。

**基线演进（实测）**：`r3e-character` 38 → 58 → 70，`r3e-physics` 238 → 239，`r3e-doc` 152 → 164，
`r3e-editor-host` 132 → 134，合计 2082 → 2129（`tests_baseline` 已按实测同步，含说明）。

**B03 判据跑出来的三处真实缺陷（阈值一条没放宽）**：

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 1 | 上坡三步即失速、重力接管、穿地（Rapier 40° 稳定复现；Builtin 无此现象） | `slope_verdict` 落点只按坡度增量外推，坡脚首帧的抬升相对真实坡面本就跳变，之后每帧差同一截、永远追不上，落点逐帧埋深后被重叠闸整体拒掉 | 落点取 `max(增量外推, 前缘坡面高度对齐)` |
| 2 | 落地后下一帧被读成「脚下没地面」 | 贴面落点零间隙，下一次竖直扫掠起点被判重叠（parry 预测距离 2mm），`started_inside` ⇒ 悬空 | 落点主动让出 1cm 由吸附落回；`settle_vertical` 加朝上面兜底 |
| 3 | 斜坡上脚底比平面接触高出 `radius*(1/n.y-1)` | 竖直胶囊贴斜面时圆心距离是法线方向的 `radius`，投影到竖直方向要除以 `n.y` | 落点按该量校正，Rapier 误差降到 0.5mm |

**B04 不变量落地（invariants 38 → 40）**：`character.config.invalid`（配置越界/非有限）与
`character.ground.penetration`（脚底低于支撑面超过 `snap_distance`）登记进 `validate/invariants.rs`，
判定在 `validate/character.rs`；`Category` 加第七变体 `Character`，`layer_of` 归 `Layer::Static`，
并由 `verify_layers.rs` 两条测试钉住「归层不错 + 报告里条数对得上」。
`character::check` 已接入 `validate_geometry`——不接的话 `scene.verify` 的静态层看不见加载期就能看见的结论，同一份场景两个答案。
`cargo xtask invariants` 现 A=B=C=40；`gen-capabilities` 输出 commands=398 / rpc=68 / script_only=330 / domains=17 / invariants=40。

---

## 8. B05 可玩性回归 1（tests +12）

**目标**：CLI 起一个带斜坡/台阶的场景，驱动角色走完全程，路点日志留证。

### 8.1 驱动通道（BD-4 已拍板档 A，措辞更正）

现成能力（实测）：`r3e-cli` 已有通用 `rpc` 子命令（`Cmd::Rpc { method, args }`），**零子命令改动**即可调 `character.*`。

但原稿「零 CLI 改动」**不成立**，实测有硬拦截：`r3e-cli/src/args.rs` 的 `TX_ONLY_METHODS` 硬编码 7 条「会改场景、只许走事务」的方法名。`character.create` / `character.config` / `character.teleport` 都改场景，不登记进这张表就会被 `rpc` **放行**——绕过事务栈、留下撤销不回去的改动，与 CLI 明确的设计意图（§229 注释）冲突。这比报错更糟：是静默后门。故 B01 同卡补 3 条。

由此脚本分两条通道——写场景走事务，推进与只读走 `rpc`：

```powershell
# 写场景：走事务（进撤销栈）
r3e-cli --addr <endpoint> tx begin
r3e-cli --addr <endpoint> tx apply character.create   --args '["ramp_walker", 0.3, 1.8, 45, 0.35, 0.3]'
r3e-cli --addr <endpoint> tx apply character.teleport --args '["ramp_walker", -8, 0, 0]'
r3e-cli --addr <endpoint> tx commit
# 推进与只读：走 rpc
r3e-cli --addr <endpoint> rpc character.step  --args '["ramp_walker", 1, 0, 0.0166]'   # 循环 240 次
r3e-cli --addr <endpoint> rpc character.state --args '["ramp_walker"]'                  # 每 10 步采样
```

**B01 内必须闭环的待确认项**：`tx apply` 要求协议事务侧有对应 op；若 `character.create/config/teleport` 只是 RPC 方法、事务侧无 op，则退化为「这三条仅 `rpc` 直调、不进撤销栈」——须在 B01 卡备注与 `M2_DECISIONS.md` 同时写明，不许两边含糊。

以下为原稿遗留的直调写法，**仅作历史记录，不再使用**：

```powershell
r3e-cli --addr <endpoint> rpc character.create  '["ramp_walker", 0.3, 1.8, 45, 0.35, 0.3]'
r3e-cli --addr <endpoint> rpc character.teleport '["ramp_walker", -8, 0, 0]'
r3e-cli --addr <endpoint> rpc character.step    '["ramp_walker", 1, 0, 0.0166]'   # 循环 240 次
r3e-cli --addr <endpoint> rpc character.state   '["ramp_walker"]'                  # 每 10 步采样
```

留证：路点日志（每 10 步一行 `frame / x / y / z / grounded`）由脚本收集到 `assets/verify/evidence/character_walk.log`，并把最终路点与期望区间做断言（末点 x 前进 ≥ 7.5m、y 抬升 ≥ 1.4m、全程 `grounded` 帧占比 ≥ 0.95）。

### 8.2 场景夹具

新增 `assets/verify/character_ramp.r3e.json`：平地 8m + 15° 斜坡 6m + 0.3m 台阶 3 级 + 收尾平台。可在 `assets/verify/expect.json` 增一条 case（期望 `passed = true` 且无 `character.*` 码），使 B04 的两条新 invariant 在 `scene-verify` 第 11 步也被正向覆盖。

### 8.3 测试明细（+12）

| 组 | 条数 |
|---|---|
| 全程路点首末断言（headless，走完 240 步） | 2 |
| `grounded` 占比断言（平地/斜坡/台阶各 1） | 3 |
| 台阶段 `step_up > 0` 至少 3 次 | 1 |
| 超限台阶挡停（场景尾部加一个 1m 台阶） | 2 |
| 日志可解析（行数、字段数、单调 frame） | 2 |
| 同输入重跑路点逐位相同（确定性） | 2 |

> 若老板选 BD-4 档 B（新增专用 `character walk` 子命令），则 `args.rs`/`help_text`/`r3e-cli/tests` 三处同步，本卡 +3 tests，需从 B02 挪 3 条预算过来（阶段总数不变）。

### 8.4 人工窗口验证项（自动门禁覆盖不到）

真窗口下「肉眼看着不下沉、不抖、贴地自然」属主观项，按 M1 惯例记入 `docs/m2/M2_BASELINE.md` 的人工验证栏，不写成自动断言（写不出来的东西不装成判据）。

---

## 9. 决策清单（**2026-11-20 老板已拍板：全选推荐档**）

拍板结果：BD-1 档 A（rpc 预算 +2→+6，全局 76→80）/ BD-2 档 A（`CharacterPort` 并列不并超 trait）/ BD-3 档 A + 新增 `Category::Character` / BD-4 档 A + 补 `TX_ONLY_METHODS` 3 条 / BD-5 档 A。五条均无悬空项，B01 可开工。

### BD-1（**硬冲突，必须先拍**）命令面 → rpc 预算

事实：卡表要 6 条 `character.*` 命令；`M2_PLAN` §4 给 B 阶段 **rpc +2**；守恒规则 4 是「`script_only` 不得增加」。
三者不能同时成立——6 条命令若只有 2 条带 rpc，则 `script_only` 330 → 334，**直接违反规则 4**。

| 档 | 做法 | commands | rpc | script_only | 全局 rpc 终局 | 代价 |
|---|---|---|---|---|---|---|
| **A（推荐）** | 6 条全带 rpc | +6 ✓ | **+6**（改预算 +2→+6） | 330 不变 ✓ | 76 → **80** | 改 6 处硬编码数字（机械、一次到位） |
| B | 收窄到 2 条 RPC（`character.step`/`character.state`），其余 4 条只做 Rust API | **+2**（改预算 +6→+2） | +2 ✓ | 330 不变 ✓ | 76 不变 | 脚本/能力清单看不到完整角色能力，B05 只能 CLI 驱动；能力面残缺 |
| C | 2 条 RPC + 4 条纯脚本 | +6 ✓ | +2 ✓ | **334（涨 4）** | 76 不变 | 需修订守恒规则 4 为「登记后允许增加」，等于放宽门禁 |

推荐 A：命令面完整、脚本可驱动、能力清单可枚举，且不碰守恒规则。代价只是 §4.4 那 6 处数字的机械同步。

### BD-2 契约落点

* **A（已拍板）**：`CharacterPort` + 2 DTO 追加进 `r3e-physics/src/port.rs`（现 435 行 → 约 510，未越 `lint-size` 800 阈值），`#[allow]` 默认实现，`port_impl.rs` 零改动（满足卡表原文「契约落 port.rs」）。**子决策已定**：`CharacterPort` **不并入** `PhysicsQueryPort` 超 trait，保持并列——并进去会强制所有现有后端实现，「`port_impl` 零改动」即告吹。
* B：契约落 `r3e-character/src/contract.rs`，`port.rs` 零改动（更干净的分层，但卡表 B01 判据需改写）。

### BD-3 两条 invariant 的归属

* **A（已拍板）**：`layer="character"`，校验器在 `r3e-doc/src/validate/character.rs`，反例测试在 `r3e-doc/tests/`（`xtask` 零改造，`r3e-doc` 不反向依赖 character）。**附加动作**：`Category` 加 `Character` 变体 + `from_code` 映射 + `r3e-editor-host::verify::layer_of` 穷尽归类（详见 §7.2）。
* B（不采纳）：码由 `r3e-character` 上报 → 需 `r3e-doc` 依赖 `r3e-character`，而 `dep_rules.json` 里 `r3e-doc` 的 allow 仅 `r3e-core` → **直接撞 `dep-gate` 红卡**，过卡等于放宽文档层铁律。

### BD-4 B05 驱动通道

* **A（已拍板，措辞更正）**：复用 `r3e-cli` 的 `rpc` + `tx apply` 通用通道 + 一个 PowerShell 回归脚本。**零子命令改动，但须往 `args.rs::TX_ONLY_METHODS` 补 3 条写场景方法**（原稿「零 CLI 改动」不成立，见 §8.1）。
* B（不采纳）：新增 `r3e-cli character walk` 子命令。代价不止 +3 tests 与三处同步——`args.rs` 现 770 行、阈值 800，余量仅 30 行，极可能越线并被迫新增 `size_debt` 条目，而该账「只许减不许增」。

### BD-5 `doc_gate` 规则 8 的严格程度

* **A（已拍板）**：源码 + 注释都算违规（一条错误注释会带偏后来人）。实现上**不**调 `strip_comments`，逐行整行匹配。
* B（不采纳）：只查代码部分（与规则 1/3 口径一致）。两档实现成本几乎相同——`doc_gate.rs` 已有 `strip_comments`，差别只在语义与维护摩擦，故此项是四条里风险最低的。

---

## 10. 门禁累加与预算配平

### 10.1 tests +82 的五卡分配

| 卡 | tests | 内容 |
|---|---|---|
| B01 | +12 | 配置校验 3 / 契约满足 4 / 状态垫底 5 |
| B02 | +20 | 10 断言点 × 2 后端 |
| B03 | +20 | 5 断言点 × 2 变体 × 2 后端 |
| B04 | +18 | 斜坡 2cm × 8 / invariant × 6 / 采样器与确定性 × 4 |
| B05 | +12 | 路点与挡停回归 |
| **合计** | **+82** ✓ | 与 `M2_PLAN` §4 B 行一致 |

`tests_baseline` 演进：2025 → 2037（B01）→ 2057（B02）→ 2077（B03）→ 2095（B04）→ 2107（B05）。

**实到对照**：B02 收口 **2082**、B03 收口 **2103**、B04 收口 **2129**——比计划高，差额来自同卡修缺陷时补的夹具（`r3e-physics` 前缘对齐 1 条）与 `r3e-doc`/`r3e-editor-host` 两侧的归层测试，卡内判据数一条未减。

### 10.2 其他门禁项

| 项 | 基线 | B 阶段增量 | 终值 |
|---|---|---|---|
| commands | 392 | +6 | 398 |
| rpc | 62 | **+6**（BD-1 档 A；原计划 +2） | 68 |
| invariants | 38 | +2 | 40 |
| domains | 17 | 0 | 17 |
| workspace 成员 | 17 | +1 | 18 |
| `size_debt` 条目 | 20 | 0（只许减） | 20 |
| `dep_debt` 条目 | 0 | 0 | 0 |
| 回放金标 | `385482eb033d2964` | **0**（必须不变） | 同值 |

### 10.3 拍板后需要改的权威文档（顺序不能颠倒）

1. `M2_PLAN.md` §4 预算表 B 行 `rpc +2` → `+6`，并在表下加一行实测说明（连带把「目标 rpc 76」改为 **80**）。
2. `M2_PLAN.md` §3.2 卡表 B01 的「主文件」列补登记：`r3e-core`（spec + cmd_spec）、`r3e-physics/src/port.rs`、`r3e-protocol`/`r3e-script`/`r3e-editor-host`（命令面）、`xtask/doc_gate.rs`。
3. `M2_PLAN.md` §3.2 B 阶段表下加一行「本阶段详细方案见 `M2_B_PLAN.md`」。
4. `README.md`（docs/m2）索引登记本文件。
5. `docs/m0/M0S3_DOC_LAYER.md` §5 加规则 8（B01 同卡）。
6. `M2_BASELINE.md` 人工验证栏：B05 主观项。
7. `r3e-cli/src/args.rs::TX_ONLY_METHODS` 补 `character.create` / `character.config` / `character.teleport`（**B01 同卡**，防 `rpc` 绕过事务）。
8. `r3e-doc::validate::Category` 加 `Character` 变体 + `r3e-editor-host::verify::layer_of` 归类（**B04 同卡**，与 2 条 invariant 同时进）。

---

## 11. 执行顺序（每步都可单独 `verify`）

| 步 | 动作 | 门禁预期 |
|---|---|---|
| S1 | 回写 §10.3 的 1–4 四项文档（**纯文档提交**，不含代码） | 11 步全绿（文档改动不影响门禁，但 `doc-gate` 规则 7 台账需保持） |
| S2 | B01-a：建 crate 骨架 + `dep_rules` + `tests_baseline` 三件套，`lib.rs` 只有常量与文档 | `dep-check` 0 违规；`check-tests` 2037 |
| S3 | B01-b：`port.rs` 追加 `CharacterPort` + DTO + 3 条自测 | `cargo test -p r3e-physics` 绿；`size_debt` 仍 20 |
| S4 | B01-c：`core/src/scene/character.rs` + `cmd_spec/character.rs` + 命令面六处数字同步 + `args.rs::TX_ONLY_METHODS` 补 3 条 + 重生成 `capabilities.json` | `gen-capabilities --check` 过；`cargo test` 全绿（含 script/editor/editor-host 契约与 CLI 拦截用例） |
| S5 | B01-d：`doc_gate` 规则 8（档 A：不调 `strip_comments`，注释也算）+ `M0S3_DOC_LAYER` §5 | `doc-gate` 8 条 0 违规 |
| S6 | B01 提交 | 11 步全绿，**提交** |
| S7–S10 | B02 → B03 → B04 → B05 逐卡：判据测试先红 → 实现 → 门禁绿 → 提交（B04 另含 `Category::Character` + `layer_of` 归类 + 2 条 invariant） | 每卡 11 步全绿 |

「判据先行」按 `M2_PLAN` §5.3：B04 的 2cm 断言先写（红），再补实现（绿）。B02/B03 的判据测试作为实现前的骨架测试。

---

## 12. 风险与回滚

| 风险 | 影响 | 处置 |
|---|---|---|
| **回放金标漂移** | 门禁第 4 步红，全阶段阻塞 | 硬约束：角色运行态不进 `Scene`（§3.3）；每卡提交前单独跑 `xtask replay` 确认 `385482eb033d2964` |
| `cmd_spec` 域顺序变化 | 快照类断言全红 | `all()` 按域文件名字母序拼接，`character` 插在 `anim` 前 → **一次改到位**，不留到后续卡 |
| 契约测试 6 处硬数字漏改 | `cargo test` 红，且无法定位 | B01-c 单独一步只做数字同步，跑一遍 `cargo test --workspace` 再进下一步 |
| 双后端斜坡差异 > 容差 | B03 卡红 | 判据用 `up_dot` + 位置容差 1e-3；实测差值记入本卡备注，**不调低阈值换绿** |
| `r3e-character` 单文件超 800 | `lint-size` 红，且禁止加 `size_debt` | §3.2 已按职责拆 5 文件，`step.rs` 预算 400 行留了 2 倍余量 |
| `doc_gate` 规则 8 误伤 | 后续卡无法用 `Physics` 门面 | 规则 8 只作用 `crates/r3e-character/`；其他 crate（runtime/game）不受限 |
| 卡表 B01「契约落 port.rs」与分层原则张力 | 已消解 | BD-2 拍档 A：`CharacterPort` 并列、不并入超 trait |
| `Category` 扩变体漏改 `layer_of` | `r3e-editor-host` 编译失败 | 穷尽 match 立刻报错，属「改一处就红」，不会静默放行 |
| `tx apply` 无对应 op | B05 脚本跑不通 | B01 内闭环确认；退化方案写入卡备注 + `M2_DECISIONS.md` |

回滚点：每卡一次提交即回滚单元（`git revert` 后 11 步仍绿）。B01 涉及面最宽，但其新增文件占绝大多数，`revert` 后仅需注意 `dep_rules.json`/`tests_baseline.json`/6 处硬数字随提交一起回退——这四类改动都在同一提交内，`revert` 天然一致。

---

## 13. 不做清单（B 阶段明确不碰）

1. **不做角色动画/骨骼**（F 阶段 IK + 根运动），B 阶段角色是一颗胶囊，无网格无动画。
2. **不做真滑动/摩擦**：斜坡超限只判「不可上行」，坡面滑落物理留后续（B03 §6.1 已注明）。
3. **不做跳跃玩法**（`MoveIntent::jump` 字段预留但不实现起跳逻辑，B01 只保证字段被忽略不报错）。
4. **不做相机跟随**（H02 第三人称相机）。
5. **不新增物理后端能力**：`port.rs` 只加组合层 trait，不碰 `BackendCaps`。
6. **不改 `r3e-script` 依赖**（卡表硬约束）。
7. **不动回放路径**、**不动 `app.rs` 主循环的既有步进顺序**（B05 只在 CLR/脚本侧驱动，不改 runtime 帧序）。
8. **不做多角色同帧结算顺序**（单控制器；多角色留 H 阶段）。

---

## 14. 待同步文档清单（本方案拍板后）

| 文档 | 动作 |
|---|---|
| `docs/m2/M2_PLAN.md` | §4 预算表 `rpc +2`→`+6`、目标 76→80；§3.2 B01 主文件列补登记；B 阶段表下加本文件指引 |
| `docs/m2/README.md` | 索引表新增 `M2_B_PLAN.md` 一行（职责：B 阶段五卡方案） |
| `docs/m2/M2_BASELINE.md` | 记 B 阶段开工真值（2025/392/62/38/20/0）+ 人工验证栏 |
| `docs/m0/M0S3_DOC_LAYER.md` | §5 规则表第 8 条（B01 同卡） |
| `docs/m2/M2_DECISIONS.md` | D2 的落地约定补一句：`CharacterSpec` 进 `Scene`、`CharacterState` 不进（§3.3 的理由） |

---

## 15. 与既有拍板的对照（确认无冲突）

| 决策 | 内容 | 本方案是否受影响 |
|---|---|---|
| D1=B | `rapier` 转默认 feature，A03 双后端 | 受益：双后端验证不需 `--no-default-features`（§5.3） |
| D2=B | 新建 `r3e-character`，`allow=["r3e-core","r3e-physics"]` | 严格执行；`CharacterPort` 落 physics 满足 allow |
| D3=A | 命令用 `character.*` 命名空间 | 严格执行（§4.4） |
| D5 默认+可配置 | 判据参数可覆盖 | §4.6 六字段全可配，非法即拒 |
| D12=B | C 阶段新建 `r3e-terrain` | 无关（B05 用的是手工夹具场景，不是地形） |
| D13=B | A03b 偿还 `dep_debt` 至 0 | 严格执行：B 阶段 `dep-check` 债务必须保持 0 |

---

## 16. 一页速览（拍板时只看这页）

* **新增 crate**：`r3e-character`（成员 18），依赖 `r3e-core` + `r3e-physics`，后者被新 `doc_gate` 规则 8 压成「只准用 `port`」。
* **契约**：`port.rs` 追加 `CharacterPort`（组合 + 默认实现，后端零改动，**不并入** `PhysicsQueryPort` 超 trait）。
* **五卡**：B01 建桩+命令面（+12）→ B02 站立与吸附（+20）→ B03 台阶斜坡（+20）→ B04 2cm 判据 + 2 条 invariant（+18）→ B05 走完全程留证（+12）= **+82**。
* **三条不可让步**：回放金标不变、`script_only` 不涨、`dep_debt` 保持 0。
* **已拍板（2026-11-20，全选推荐档）**：BD-1 A（rpc `+2`→`+6`，全局 76→80）/ BD-2 A / BD-3 A + `Category::Character` / BD-4 A + `TX_ONLY_METHODS` 3 条 / BD-5 A。无悬空决策。
*（内容由AI生成，仅供参考）*
