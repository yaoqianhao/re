---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_ffa0332be48811f1a5ef525400638852
    ReservedCode1: AIFvAkOGR5Eus/bI8UK3LkQv1p8tQv4yZLPPQhwFBisexRUM8gS7yGkPCxeHJ96xleguF8UBorCzztz0abVsggQl6btCwlLyScyDh9rqLoxAMKP+eZWSt8dcarx1oSqQVVYxtJK12UfPm10zK43c+1iCyBGEO6zLiexBiTn/LDt+hyvlmO7YfOJ3SvA=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_ffa0332be48811f1a5ef525400638852
    ReservedCode2: AIFvAkOGR5Eus/bI8UK3LkQv1p8tQv4yZLPPQhwFBisexRUM8gS7yGkPCxeHJ96xleguF8UBorCzztz0abVsggQl6btCwlLyScyDh9rqLoxAMKP+eZWSt8dcarx1oSqQVVYxtJK12UfPm10zK43c+1iCyBGEO6zLiexBiTn/LDt+hyvlmO7YfOJ3SvA=
---

# M2-A03 物理查询端口 · 落点改到 `r3e-physics` 的实施规划

- 卡号：A03（M2 第 3 张卡，`M2_PLAN.md` §3.1）
- 状态：**已确认，可开工**（2026-11-20 老板对四项待定全部拍板；结论见 §5、§5.2、§7 与 `M2_DECISIONS.md`「补充决定（2026-11-20）」；未修改任何代码，`r3e-core` 侧已落盘草稿的回滚仍按 §9 执行）
- 代码状态：**尚未动手**。`crates/r3e-core/src/physics_port.rs`（406 行草稿）与 `lib.rs` 的 `mod` 声明仍待回滚（§9 第一步）。
- 依据：`M2_PLAN.md` §3.1 A03、`M2_DECISIONS.md` D1=B / D2=B / D13=B；**老板 2026-11-20 指令：这部分代码放 `crates/r3e-physics` 目录下，整体功能不变**
- 与既有拍板的差异：D1 落地约定第 2 条原写「端口 trait 定义放 `r3e-core`」，本规划改为落 `r3e-physics`，冲突处置见 §5
- 日期：2026-11-20

---

## 0. 一句话

端口契约（3 组 trait + 5 个数据类型）从原定 `r3e-core/src/physics_port.rs` 改落 **`crates/r3e-physics/src/port.rs`**，后端实现拆进 `builtin_backend.rs` / `rapier_backend.rs`，门面实现落 `port_impl.rs`；**契约条目、双后端判据、测试预算与 core 版逐条等价**（§2），`r3e-core` 一行不留。默认后端随 D1=B 转 `rapier`。

---

## 1. 变更内容

| 项 | 原定（D1 约定第 2 条） | 现定 | 影响 |
|---|---|---|---|
| 端口契约（trait + 数据类型） | `r3e-core/src/physics_port.rs` | `crates/r3e-physics/src/port.rs` | `r3e-core` 零新增；端口与实现同 crate |
| 门面实现（`impl … for Physics`） | 原本计划由 physics 侧 impl core 的 trait | `crates/r3e-physics/src/port_impl.rs` | 文件数 +1 |
| 内置 / 空后端 | 原计划迁 `builtin_backend.rs` | 同（迁出 `lib.rs` 728–1217 行区段） | `lib.rs` 大幅净减，为 A05 提前达标 |
| Rapier 侧实现 | `rapier_backend.rs` 增三组查询 | 同 | 保持「rapier 类型不外泄」不变 |
| `r3e-core` 改动 | 新增模块 + `lib.rs` 挂载 + 导出 | **零改动**（草稿回滚） | 依赖方向不变（core 仍零引擎依赖） |
| 默认后端 | D1=B：`default = ["rapier"]` | 同 | 不变 |
| 契约测试 | 同一套用例双后端各跑一遍 | 同 | 不变 |

---

## 2. 功能等价性对照（「整体功能不变」的定义）

本规划的「功能不变」指：**端口契约的能力面与判据与 core 版逐条等价，换落点不增减能力、不降低验收强度、不减少测试预算。** 逐条对照如下。

| # | 契约条目 | core 版（草稿实测） | physics 版 | 等价 |
|---|---|---|---|---|
| 1 | 数据类型 | `CastRay` / `PortHit` / `Capsule` / `GroundSample` / `PortCaps`，5 个 | 同名同字段，同一份代码迁移 | ✅ 逐字节搬迁 |
| 2 | 查询 trait | `RaycastPort` / `CapsulePort` / `GroundPort`，3 个 | 同 | ✅ |
| 3 | 组合 trait | `PhysicsQueryPort: RaycastPort + CapsulePort + GroundPort` | 同 | ✅ |
| 4 | 缺省实现 | 三组 trait 的 `port_caps()` 默认 `NONE`、查询默认空结果（"没实现就是没有"） | 同 | ✅ |
| 5 | 口径约定 | 方向归一化、`distance` 为世界距离、起点重叠 `distance==0 && started_inside`、申报与行为一致 | 同（注释随代码迁移） | ✅ |
| 6 | 胶囊几何 | 轴对齐 `Capsule`（中心 + 半高 + 半径），整体高 `2*(h+r)` | 同 | ✅ |
| 7 | `PortHit.body` | `u64`（对上层不透明 id，不暴露 `BodyHandle`） | `u64`（转发时取 `BodyHandle.0`） | ✅ |
| 8 | 模块内单测 | 8 个 `#[test]`（归一化/退化输入、胶囊几何与非正半径、地面可站立方向、`PortCaps` 键稳定、缺省实现、对象安全性） | 同 8 个，迁入 `port.rs` 的 `#[cfg(test)] mod tests` | ✅ |
| 9 | 双后端 | builtin / rapier 各一套实现，`--no-default-features` 回落 builtin | 同 | ✅ |
| 10 | 契约测试 | 同一套断言双后端各跑一遍 | `tests/port_contract.rs`，复用既有 `selectable()` 循环模式 | ✅ |
| 11 | 测试预算 | A03 `+200`（`M2_PLAN` §门禁预算，r3e-physics 38 → 238） | 同额，落 physics crate | ✅ 预算不挪 |
| 12 | 上层可见面 | `&dyn PhysicsQueryPort` 注入 | 同（因 crate 变了，注入侧依赖声明见 §4） | ⚠ 见 §5.1 |

**唯一实质差异**：trait 的所在 crate 变了，于是「只依赖 core 就能拿到端口」这一属性不再成立 —— 这是老板本次指令的直接后果，处置见 §5。

---

## 3. 文件落点与行数预算

| 文件 | 动作 | 实测/预算行数 | 备注 |
|---|---|---|---|
| `crates/r3e-physics/src/port.rs` | 新建 | 406 行（core 草稿实数）+ 后端契约说明 ≈ 430 | 纯 trait + 数据 + 单测，`use glam::Vec3; use r3e_core::NodeId;`，**零 rapier 依赖** |
| `crates/r3e-physics/src/port_impl.rs` | 新建 | ≈ 220 | `impl RaycastPort / CapsulePort / GroundPort for Physics` + `PortCaps` 推导 + 单测 |
| `crates/r3e-physics/src/builtin_backend.rs` | 新建（迁出） | ≈ 420 | `BuiltinBackend`（现 823–1217，395 行）+ 三个端口新方法 |
| `crates/r3e-physics/src/rapier_backend.rs` | 改 | 625 → ≈ 720 | 增胶囊重叠/扫掠、地面采样；rapier 类型仍只在本文件 |
| `crates/r3e-physics/src/lib.rs` | 改 | 1924 → **≈ 1465**（实测为准） | 迁出 `NullBackend`(728–801, 74 行) + `BuiltinBackend`(823–1217, 395 行)；新增 3 个 `mod` + `pub use` + `pub(crate)` 访问器（≈ -445 净减） |
| `crates/r3e-physics/tests/port_contract.rs` | 新建 | ≈ 260 | 双后端契约（镜像 `backend_contract.rs` 的 `selectable()` 模式） |
| `crates/r3e-physics/Cargo.toml` | 改 | `default = []` → `default = ["rapier"]` | D1=B |
| `crates/r3e-core/src/physics_port.rs` | **删除** | 406 行 | 草稿回滚（§9） |
| `crates/r3e-core/src/lib.rs` | **回滚** | 恢复 HEAD | `git checkout -- crates/r3e-core/src/lib.rs` |

`size_debt.json`：`crates/r3e-physics/src/lib.rs` 登记 base 1929（实测 1924）。本卡把它降到 ≈1465 后，A05 的「≤1924」目标**提前达成**，届时该条目可整条删除；`port.rs`(430) / `port_impl.rs`(220) / `builtin_backend.rs`(420) 均 < 800，不触发登记。

---

## 4. 依赖与 feature 方案

1. `crates/r3e-physics/Cargo.toml`：`default = ["rapier"]`，`rapier3d` 由可选转默认。
2. `port.rs` 与 `port_impl.rs` **不得出现 `rapier3d` 任何类型**，`#[cfg(feature = "rapier")]` 只允许出现在 `rapier_backend.rs`（沿用既有硬约束，`doc-gate` 会查）。
3. 下游消费端（若 B01 落 `r3e-character`）声明 `r3e-physics = { workspace = true, default-features = false }`，只吃端口契约、不主动拉 `rapier3d`。
   - 注意：Cargo 的 feature 在单次构建内**全局合并**，同一 workspace 里只要有别的 crate 开了 `rapier`，`r3e-character` 也会编到含 rapier 的那份 `r3e-physics`。`default-features = false` 只保证它自己不主动引入；编译时间代价是 D1=B 已接受项。
4. `xtask/dep_rules.json` 唯一变更：`r3e-character.allow` 追加 `r3e-physics`（§5.1）。`r3e-script` 的 allow 表**不动**（§5.2）。

---

## 5. 与既有拍板的两处冲突及处置（**已于 2026-11-20 老板拍板：两项均按本处置执行**）

D1 第 2 条原本把「trait 落 core」写成硬约束，理由正是 D2=B：`r3e-character` 只许依赖 core，若 trait 在 physics，角色 crate 就得再加一条依赖边。本次改落点后，这个理由失效，两处冲突必须显式处置，否则后续 B01 / A03b 会撞门禁。

### 5.1 冲突一：`r3e-character`（D2=B / B01）

| 项 | 原定 | 改落点后 | 处置 |
|---|---|---|---|
| `dep_rules.r3e-character.allow` | `["r3e-core"]` | 必须 `["r3e-core", "r3e-physics"]` | 改白名单，**同卡登记**（B01 的判据文字同步改为「allow = core + physics(with port)」） |
| 控制器如何拿端口 | 注入 `&dyn PhysicsQueryPort`（trait 在 core） | 同，但 trait 来自 `r3e_physics::port` | B01 的 `pub use` 改路径即可，设计不变 |
| 隔离性 | 角色 crate 完全不知道物理实现存在 | 角色 crate 依赖 physics 的 `port` 模块，且以 `default-features = false` 不拉 rapier | 隔离性下降一档：可接受但需记档 |

**代价说清**：控制器的"物理无关"从"编译期不可能"降级为"约定上只用 `port` 子模块"。补救措施（建议同时落）：`r3e-character` 的 `dep-check` 之外，新增一条 `doc-gate` 级别的守卫 —— `r3e-character` 源码中出现 `r3e_physics::` 后跟 `port` 以外路径即失败（约 30 行 xtask 规则，可挂在 A03 或 B01）。

> **拍板结论（2026-11-20）**：老板回「可以加白名单」。`dep_rules.r3e-character.allow` 扩为 `["r3e-core", "r3e-physics"]`，并**把守卫与白名单写进同一张卡（B01）**，不留「先放宽、后守卫」的空窗。已回写 `M2_DECISIONS.md` D2 第 2 条、`M2_PLAN.md` §3.2 B01 判据。

### 5.2 冲突二：`r3e-script` 端口化（D13=B / A03b）

- `r3e-script.allow = ["r3e-core", "r3e-doc", "r3e-protocol"]`，且 `dep_debt` 里正要偿还 `script→physics` 这条直连债。**trait 落到 physics 后，script 依旧不能引用该 trait**。
- 处置（**原稿，已被 2026-11-20 拍板替代**）：A03b 的 `CapabilityHost` 物理入口不引用端口 trait，改为 script 侧自定函数式入口（`Box<dyn Fn(CastRayQuery) -> Option<HitDto> + Send + Sync>` 一类），由 `r3e-runtime` 在装配时用 `Physics` 的端口实现填进去。即：**DTO 与注入点在 script，端口实现在 physics，桥在 runtime**。
- 原稿的缺陷（老板追问后暴露）：这层桥只覆盖**查询类**能力（`physics_raycast` / `physics_raycast_full` / `physics_overlap` / `physics_shape_cast_*` / `physics_ground`）。而 `api/physics.rs` 的 24 个注册绑定里，`attach_body` / `detach_body` / `body_impulse` / `physics_set_enabled` / `physics_set_backend` / `physics_take_events` / `physics_set_ccd` / `body_set_ccd` 等**写操作与状态变更**同样在摸 `c.physics`（22 处 `borrow` 调用点中的大部分）。写操作无法用「返回 `Option<HitDto>` 的闭包」表达，仍会留下 `Rc<RefCell<Physics>>` 直连 → `dep_debt` 那条边删不掉 → D13=B 的「债务归零」判据不成立。
- **拍板结论（2026-11-20，老板选「trait 默认方法 + 函数式适配器」）**：A03b 改为在 `r3e-script` 侧自建 `PhysicsHost` / `AudioHost` trait，方法签名全用脚本原生类型（零 physics 类型），每个能力都有**默认实现**并统一转发到唯一必实现的 `execute(Req) -> Rsp`，再由 `impl<F: Fn(Req)->Rsp> Host for F` 让闭包本身成为适配器；实现方 `RhaiPhysicsHost` / `RhaiAudioHost` 落 `r3e-runtime/src/script_bind.rs`。完整设计见 **`docs/m2/M2_A03B_HOST_PLAN.md`**。
- 影响：A03b 的 `+56` 测试预算与「`dep_debt` 归零」判据**不变**；范围按实测细化为 **69 个绑定调用点 / 706 行**（物理 24 / 340 行、音频 45 / 366 行），trait 能力面为 **53 个去重方法**（物理 16 / 音频 37）—— 「53 个 / 704 行」与「69 个 / 706 行」是两个维度（去重方法数 vs `register_fn` 注册数），详见 `M2_DECISIONS.md` D13 第 2 条。新增一处**必须接受的代价**：30 处 `ScriptHost::new` 调用点的 import 由 `r3e_script` 改指 `r3e_runtime` 的同名再导出（详见设计稿 §4.2）。
- 若既不加 trait 又不同步搬走实现：唯一替代是让 `r3e-script` 直接依赖 `r3e-physics` —— 与 D13=B「债务归零」正面冲突，**已明确否决**。

---

## 6. 接口扩展清单（最小侵入原则）

端口三组查询里，`raycast` 后端已有现成实现，胶囊与地面没有。为避免动 `BackendCaps`（`r3e-protocol` 有 DTO 映射、`capabilities.json` 三源门会跟着变），**一律用带默认实现的可选后端方法**扩展：

| 新增项 | 落点 | 破坏性 | 说明 |
|---|---|---|---|
| `PhysicsBackend::overlap_capsule(&self, Capsule) -> Vec<PortHit>` | `lib.rs` trait 内，默认 `Vec::new()` | 无（默认实现） | 内置/rapier 各自 override |
| `PhysicsBackend::sweep_capsule(&self, Capsule, dir, max_distance) -> Option<PortHit>` | 同上，默认 `None` | 无 | |
| `PhysicsBackend::sample_ground(&self, x, z) -> Option<GroundSample>` | 同上，默认 `None` | 无 | 内置走地面平面；rapier 走地面 + 向下射线 |
| `pub(crate) fn backend(&self) -> &dyn PhysicsBackend` | `lib.rs` `impl Physics` | 无（crate 内可见） | `port_impl.rs` 访问私有字段的唯一入口，≈6 行 |
| `PortCaps` 推导 | `port_impl.rs` | 无 | 按 `PhysicsKind` 静态映射：Null→`NONE`、Builtin/Rapier→`FULL`；**不动 `BackendCaps`** |

`BackendCaps`、`PhysicsBackend` 既有方法签名、`docs/capabilities.json`、命令面（`commands=392 / rpc=62`）**全部零变化** —— 本卡不新增任何命令 / RPC / 不变量。

---

## 7. 默认后端转 `rapier` 的行为变化点与受影响断言

D1=B 的必然结果，不是本卡偷偷加的变化，但必须逐条记账。**2026-11-20 老板确认：默认后端改为 rapier，本节 5 条全部执行，不做例外。**

| # | 位置 | 现状 | 改成 | 受影响的既有断言 |
|---|---|---|---|---|
| 1 | `Physics::set_enabled`（lib.rs:1298–1308） | 从 `Null` 自动升级到 `PhysicsKind::Builtin` | 升级到「当前构建的默认可用后端」：`rapier` 可用则 `Rapier`，否则 `Builtin` | lib.rs:1634 起 `set_enabled(true)` 相关断言（约 2 处） |
| 2 | 默认构造的落点 | `Physics::new()` 恒 `Null` + 关闭 | **保持不变**（`new()` 仍 null/关闭，避免"开箱即跑物理"的隐性行为变化） | 无 |
| 3 | `scene-verify` 6 个夹具 | 基线记录为「物理后端 builtin」 | 默认构建下将跑 `rapier` | 夹具期望值需逐个复核；夹具若显式写了 `physics.kind` 则不受影响 |
| 4 | `bind_scene.rs` 测试 | 显式构造 `BuiltinBackend` | 不变（显式构造绕过默认） | 无 |
| 5 | `tests/backend_contract.rs` | `selectable()` 默认构建只含 builtin | 默认构建将含 builtin + rapier（两个都跑） | 该文件本就是循环，天然兼容 |

---

## 8. 测试计划与计数口径

**口径先说清（这是踩过坑的地方）**：`cargo xtask check-tests` 是**静态扫源码里 `#[test]` 出现次数**（`xtask/src/main.rs` `text.matches("#[test]").count()`），不是运行结果数。所以：

- 「同一套用例双后端各跑一遍」靠 `for kind in selectable()` 循环，**运行期跑两遍、静态只计一次**；
- A03 的 `+200` 预算必须靠**新增 `#[test]` 函数**落够，不能指望循环凑数；
- 双后端两轮验证由构建参数决定：默认构建（`rapier`）一轮、`--no-default-features` 一轮，两轮都要绿。

| 落点 | 内容 | 静态 `#[test]` 建议 |
|---|---|---|
| `src/port.rs` | core 草稿 8 条原样迁移（类型/口径/缺省实现/对象安全） | 8 |
| `src/port_impl.rs` | 门面委托正确性：raycast 转发、胶囊重叠排序稳定、`PortCaps` 与 `kind` 一致、Null 后端全空、申报与行为一致（申报 false 必返空） | ≈ 40 |
| `tests/port_contract.rs` | 双后端契约：胶囊站在地面不穿地、斜坡法线上行可行性、胶囊扫掠命中距离、地面采样高度、重叠结果按 node 升序、切换后端后端口仍自洽 | ≈ 90 |
| `tests/rapier_query.rs` | rapier 专属语义（`#[cfg(feature = "rapier")]`）：parry 胶囊 vs 网格形状、CCD 下扫掠不穿透 | ≈ 70 |
| **合计** | r3e-physics 38 → **≥238** | **≥200** |

其余门禁增量：`commands 392 / rpc 62 / script_only 330 / domains 17 / invariants 38` **全部不变**。

---

## 9. `r3e-core` 侧已落盘草稿的回滚（必做第一步）

当前工作区状态（`git status`）：`M crates/r3e-core/src/lib.rs`、`?? crates/r3e-core/src/physics_port.rs`（406 行 / 8 测试）。搬迁步骤：

1. 复制 `crates/r3e-core/src/physics_port.rs` → `crates/r3e-physics/src/port.rs`。
2. 改 import：`use crate::node::NodeId;` → `use r3e_core::NodeId;`（`glam::Vec3` 不变；确认 `r3e-physics` 已依赖 `r3e-core` 与 `glam`，是的，`Cargo.toml` 已有）。
3. 模块文档头改写：把「为什么要在 `r3e-core`」一段改为「为什么在 `r3e-physics` 里单独成模块 + 为什么仍然只暴露 trait」（复用 `MeshProvider` 同一套「以 trait 为边界、上层不认实现」的叙述）。
4. 删除 `crates/r3e-core/src/physics_port.rs`；`git checkout -- crates/r3e-core/src/lib.rs` 恢复 HEAD。
5. `cargo check -p r3e-core -p r3e-physics` 确认 core 零改动、physics 新模块可编译。

---

## 10. 执行步骤（每步都可单独 verify）

| 步 | 动作 | 完成判据 |
|---|---|---|
| S0 | 前置检查：`cargo build -p r3e-physics --features rapier` 是否已绿（rapier3d 0.36 已在 workspace） | 绿，或记录首次拉取耗时 |
| S1 | §9 回滚 + `port.rs` 落位 + `lib.rs` 挂 `pub mod port;` | `cargo test -p r3e-physics` 绿（38+8 条），core 干净 |
| S2 | `builtin_backend.rs` 迁出 `BuiltinBackend` + `NullBackend`，`lib.rs` 净减 | `cargo test -p r3e-physics` 全绿；`lint-size` 绿；`lib.rs` 实测行数记档 |
| S3 | `PhysicsBackend` 三个可选方法 + builtin / rapier 各实现 + `port_impl.rs` 门面 impl | `cargo test -p r3e-physics` 绿；`--no-default-features` 也绿 |
| S4 | `Cargo.toml` `default = ["rapier"]` + `set_enabled` 默认后端修正 + 受影响断言更新 | 两轮构建全绿；`scene-verify` 6 夹具逐个复核 |
| S5 | `tests/port_contract.rs` + `tests/rapier_query.rs` 落够 +200 | `check-tests` 显示 `r3e-physics 38 -> ≥238`，`tests_baseline.json` 同步 |
| S6 | 全 11 步 verify + 文档同频（§13）+ 单卡提交 | 11 步退出码全 0；`replay` 哈希两次一致 |

**验证组合必须跑两遍**：`cargo test -p r3e-physics`（默认 = rapier）与 `cargo test -p r3e-physics --no-default-features`（= builtin），两轮结果都记进卡尾留证。

---

## 11. 门禁与验收（11 步 + 本卡专项）

| # | 门 | 本卡专项关注 |
|---|---|---|
| 1 | `cargo fmt --all -- --check` | 新文件全部格式化 |
| 2 | `clippy --workspace --all-targets -D warnings` | `--no-default-features` 下 clippy 也要过（易漏） |
| 3 | `cargo test --workspace` | 两轮（默认 / `--no-default-features`） |
| 4 | `replay` | `final=385482eb033d2964` 不漂移（默认 rapier 后物理无关帧仍应稳定；若漂移必须解释清楚是不是 rapier 引入） |
| 5 | `lint-size` | `lib.rs` 下降；三个新文件 < 800 |
| 6 | `check-tests` | `r3e-physics ≥ 238`、总量 ≥ 2017 |
| 7 | `doc-gate` | 新 crate 内模块文档齐；rapier 类型不外泄 |
| 8 | `dep-check --strict` | 违规 0；`dep_debt` 仍 2 条（本卡不动债务，A03b 才还） |
| 9 | `gen-capabilities --check` | `392 / 62 / 330 / 17 / 38` 一字不变 |
| 10 | `invariants` | 三源仍 38 |
| 11 | `scene-verify` | 6 夹具后端从 builtin 变 rapier 后逐个复核通过 |

---

## 12. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| `lib.rs` 下巴的区段引用交叉（`BuiltinBackend` 被 `Physics::rebuild_backend`、`with_backend` 使用） | 迁出后编译错一片 | S2 单独成一步、只挪不改逻辑；`pub use` 保持外部路径不变（`r3e_physics::BuiltinBackend` 仍可用） |
| `scene-verify` 期望值隐含 builtin 语义 | 门禁假红 | S4 前先跑一遍留底（baseline），差异逐条解释，不许直接改期望值凑绿 |
| `replay` 金标漂移 | 第 4 步红 | 先确认漂移是否来自 rapier 默认；若来自本卡端口化则本卡不过 |
| 双后端契约在 `--no-default-features` 下漏跑 | 假绿 | S5 起固定两轮命令，卡尾留证写明两轮后端名 |
| `r3e-character` 未来直接 `use r3e_physics::Physics` | 隔离性彻底失守 | §5.1 补救守卫（源码路径白名单检查） |

**回滚**：A03 单卡提交，出问题 `git revert` 单提交即可回到「四道门全绿 + 端口未落位」；`r3e-core` 侧草稿不提交，回滚零成本。

---

## 13. 确认后需要同步的权威文档

> 同步状态（2026-11-20）：**前 3 行已回写完毕**；后 3 行属实施期动作（A03 内 S5 步执行时改），尚未开始，符合预期。

| 文档 | 改哪 | 状态 |
|---|---|---|
| `M2_DECISIONS.md` D1 落地约定第 2 条 | 改为「端口 trait 放 `r3e-physics/src/port.rs`」，并注明 2026-11-20 老板指令修订；同时说明 §5.1/§5.2 的连锁后果 | ✅ 已回写（原文划线保留 + 修订说明） |
| `M2_DECISIONS.md` D2 落地约定第 2 条 | `allow` 从 `["r3e-core"]` 改为 `["r3e-core","r3e-physics"]`，并注销「不得依赖 physics」的表述 | ✅ 已回写（含 B01 守卫条款） |
| `M2_PLAN.md` §3.1 A03 行 / §3.2 B01 行 | A03 判据「trait 定义落 core」→「落 `r3e-physics`」；B01 判据的 allow 白名单同步 | ✅ 已回写（另新增 §索引行 `M2_A03B_HOST_PLAN.md`） |
| `M2_DECISIONS.md` D13 落地约定第 2 / 2b 条 | 形态改为 trait 默认方法 + 函数式适配器；口径校准为「53 个去重方法与 69 个注册点、706 行」两维度 | ✅ 已回写（2026-11-20，老板第三问结论） |
| `docs/PHYSICS.md` | 新增「查询端口」小节：三组 trait、`PortCaps` 口径、注入方式、双后端矩阵 | ✅ 已回写（§13，另在 §7.4 开头加指引） |
| `xtask/tests_baseline.json` | `r3e-physics` 38 → 实测值（S5 同卡） | ✅ 已更新：`r3e-physics` 38 → **238**，合计 1817 → **2017**（+200，与 §8 预算一致） |
| `xtask/size_debt.json` | `r3e-physics/src/lib.rs` 行数按实测收紧（A05 统一删条目） | ✅ 已收紧：1929 → **1542**（条目留到 A05 随拆域一并删） |

---

## 14. 不做清单（本卡明确不碰）

1. 不新增任何命令 / RPC / 不变量，`capabilities.json` 一字不改。
2. 不改 `BackendCaps`、不改 `PhysicsBackend` 既有方法签名、不改 `PhysicsConfig`。
3. 不动 `dep_debt` 两条（归 A03b）。
4. 不拆 `rapier_backend.rs`（720 行 < 800，不触发 `size_debt`）。
5. 不在 `r3e-character` 里写任何代码（B01 才建 crate）；本卡只负责把端口做出来并把依赖方案定死。
6. 不碰 `app.rs`（3040 行，属 G02 拆分范围）。

---

## 15. 四项待定的拍板结论（2026-11-20，老板）

本规划提交时留了四项待定，老板已逐项答复。结论如下，**后续卡不得再改**，除非老板另下指令。

| # | 待定事项 | 老板答复 | 落地处置 | 回写位置 |
|---|---|---|---|---|
| 1 | D1 落地约定第 2 条「trait 落 core」是否改 | **确认修改** | 端口 trait + 数据类型落 `crates/r3e-physics/src/port.rs`；`r3e-core` 零改动；`physics_port.rs` 草稿回滚（§9） | `M2_DECISIONS.md` D1 第 2 条（划线 + 修订说明）；`M2_PLAN.md` §3.1 A03 行 |
| 2 | `r3e-character` 白名单是否加 `r3e-physics` | **可以加白名单** | `allow = ["r3e-core", "r3e-physics"]`；守卫（源码只许用 `port` 子模块）与白名单**同卡落地**，不留在 A03 先放开 | `M2_DECISIONS.md` D2 第 2 条；`M2_PLAN.md` §3.2 B01 行 |
| 3 | A03b 是否接受多一层 runtime DTO 桥 | **不接受浅桥，改「trait 默认方法 + 函数式适配器」** | 详见本节 §15.1 与 `M2_A03B_HOST_PLAN.md`；同时把「53 个方法 / 704 行」与实测「69 个注册点 / 706 行」校准为两个统计维度（去重方法数 vs `register_fn` 数） | `M2_DECISIONS.md` D13 第 2 / 2b 条；`M2_PLAN.md` §3.1 A03b 行；本文 §5.2 |
| 4 | 默认后端是否改 `rapier` | **默认后端改为 rapier** | `default = ["rapier"]`；`set_enabled` 自动升级目标改为「当前构建默认可用后端」；`scene-verify` 6 夹具逐个复核；`new()` 仍 null | `M2_DECISIONS.md` D1 第 1 条；本文 §7 |

### 15.1 第 3 项的完整回答（`script` 为何不能依赖 `physics`）

老板追问「为什么 `script` 不能依赖 `physics`」，完整论证见 **`docs/m2/M2_A03B_HOST_PLAN.md` §1–§4**，此处只留结论：

1. **账面没有额度**：`r3e-script.allow` 只有 3 项，两条 `script→physics` / `script→audio` 边靠 `dep_debt`（`limit=2`，已顶格）免死；D2/D12 建新 crate 后「脚本要角色/地形能力」的诉求必然出现，不清零就每卡撞墙。D13=B 的收口条件写死「删两条、limit→0」。
2. **依赖方向不对**：script 是策略层（脚本内容运行期才知道、可能由 AI/编辑器生成），physics 是实现层（`null` / `builtin` / `rapier` 三后端）；script 需要的是「一次射线查询」这个**能力**，不是 `Ray` / `RayHit` 这些**类型**。
3. **沙箱与能力面**：`capabilities.json` 三源门禁能成立的前提是「脚本只声明能力、不认实现」；一旦 `use r3e_physics::Physics`，任何闭包都能摸到后端私有状态，能力裁剪无从谈起。

**采纳方案的要点**：`PhysicsHost` / `AudioHost` 方法签名全用脚本原生类型（零 physics 类型）；每个方法**有默认实现**，统一转发到唯一必实现的 `execute(Req) -> Rsp`；`impl<F: Fn(Req)->Rsp> Host for F` 让闭包本身即适配器；实现方落 `r3e-runtime/src/script_bind.rs`。**收益**：24 + 45 个绑定的测试成本从「模拟物理」降为「断言转发」；**代价**：30 处 `ScriptHost::new` 的 import 改由 `r3e-runtime` 再导出（唯一取舍，见设计稿 §4.2）。

*（内容由AI生成，仅供参考）*

---

## 16. 实施记录（2026-11-20，S1–S6 已全部完成）

本卡已按 §10 的步骤落盘，**一卡一提交、未 push**。以下为实测结果，供 A03b 与 A05 对账。

### 16.1 文件落点与实测行数（对照 §3 预算）

| 文件 | 预算（§3） | 实测 | 说明 |
|---|---|---|---|
| `src/port.rs` | ~430 | **435** | 三组 trait + `PhysicsQueryPort` + `CastRay` / `Capsule` / `PortHit` / `GroundSample` / `PortCaps`，含 8 条单测 |
| `src/port_impl.rs` | ~220 | **731** | 比预算大：两种 DTO 的字段搬运 + `started_inside` 反推 + 门面三方法同源，注释占了约四成，未拆（<800） |
| `src/builtin_backend.rs` | ~420 | **627** | 从 `lib.rs` 整段迁出（AABB slab 射线、Minkowski 扫掠、平面地面），非改写 |
| `src/rapier_backend.rs` | 625 → ~720 | **784** | 新增胶囊重叠 / 扫掠 / 地面采样与 `PortCaps` 申报 |
| `src/lib.rs` | 1924 → ~1465 | **1542** | **净减 387 行**，满足「净增 ≤ 0」判据；`size_debt` 条目按实测收紧到 1542（A05 统一删） |
| `src/bind.rs` | 不变 | **241** | 未动 |

`r3e-core` **零改动**（§9 的草稿已回滚：`src/physics_port.rs` 删除、`lib.rs` 的 `mod` 声明 checkout 复原）。

### 16.2 测试（对照 §8 的 +200 预算）

| 文件 | 条数 | 是否受 feature 门控 |
|---|---|---|
| `tests/port_contract.rs` | 43 | 否（跑遍 `selectable()`） |
| `tests/port_capsule_contract.rs` | 27 | 否 |
| `tests/port_ground_contract.rs` | 24 | 否 |
| `tests/rapier_query.rs` | 23 | `#[cfg(feature = "rapier")]` |
| `tests/rapier_query_ground.rs` | 34 | `#[cfg(feature = "rapier")]` |
| `tests/common/mod.rs` | — | 共享夹具（`selectable` / `physics_of` / `world_shapes` / `world_specs`） |

`r3e-physics` 38 → **238**（**+200**，与预算一致）；合计 1817 → **2017**。

拆分原因：初版把 94 条契约测试写在一个 `port_contract.rs` 里（1466 行）、57 条 rapier 测试写在一个
`rapier_query.rs` 里（959 行），双双超 `lint-size` 的 800 行上限。按「射线 / 胶囊 / 地面」与
「法线与旋转体 / 地面与扫掠与校验」切成六个文件，夹具抽到 `tests/common/mod.rs`（172 行），最大的测试文件 663 行。

### 16.3 两轮构建与门禁

| 项 | 命令 | 结果 |
|---|---|---|
| 默认 features（rapier 为默认后端） | `cargo test -p r3e-physics` | 238 passed / 0 failed |
| 回落 builtin | `cargo test -p r3e-physics --no-default-features` | 238 条中 rapier 两族整文件不编译，其余全绿 |
| 格式 / 静态检查 | `cargo fmt --all`、`cargo clippy --workspace --all-targets -- -D warnings` | 0 error、0 warning |
| 全 11 步 | `cargo xtask verify` | **全部通过**（`lint-size` 白名单 23 项全部未上涨、`dep-check` 违规 0 / 债务 2、`scene-verify` 6 夹具全绿、`invariants` 三源 38） |

`clippy` 期间修掉 3 处 `neg_cmp_op_on_partial_ord`（`builtin_backend.rs` 一处、`port.rs` 两处，
一律改成 `is_nan() || <= 0.0`）与一处 `0.7071` 字面量（改 `std::f32::consts::FRAC_1_SQRT_2`）。

### 16.4 与 §5 / §15 的偏差说明

1. **`port_impl.rs` 超预算**（731 vs ~220）：适配层同时承担「门面三方法同源」与「两种 DTO 字段
   搬运 + 注释」，拆成两个文件只会让「同一规则两处维护」，故保留单文件并压在 800 行内。
2. **`GroundPort` 的曲面精度**：`sample_ground` 在 rapier 侧走真实查询，builtin 侧是平面近似；
   差异已在 `docs/PHYSICS.md` §13.5 列表并用 `rapier_query_ground.rs` 钉住（不是缺陷，是口径）。
3. **`scene-verify` 6 夹具在默认 rapier 下逐个复核通过**，无需改夹具或金标（§7 预判的 3 处受影响
   断言实际未触发）。

*（内容由AI生成，仅供参考）*
