---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_f277f7e8e4d211f1bfe1525400dc9a98
    ReservedCode1: 1wwBIq2y4SwzwevZybwTmKiHJYDjQq+vk5+An1nnaW3fOfe8SaO3cV402mQuwShubyJoZ5ZOkiNyJQrDLHOZrvTnKvjTRhOKFeawdXIQDsPMZHK0WSSaIKoLnXEH86xvFFlbJsIDu4mLjMaj4kpS6SIzJBeN6lIVVEiCBm8DI9vZLNa7Y+7HOrVkzVw=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_f277f7e8e4d211f1bfe1525400dc9a98
    ReservedCode2: 1wwBIq2y4SwzwevZybwTmKiHJYDjQq+vk5+An1nnaW3fOfe8SaO3cV402mQuwShubyJoZ5ZOkiNyJQrDLHOZrvTnKvjTRhOKFeawdXIQDsPMZHK0WSSaIKoLnXEH86xvFFlbJsIDu4mLjMaj4kpS6SIzJBeN6lIVVEiCBm8DI9vZLNa7Y+7HOrVkzVw=
---

# M2 · A05 `size_debt` 收紧 3 项 —— 实施方案（规划稿）

> 状态：**待老板拍板 A05-D1（档位选择）后开工**
> 上游：`M2_PLAN.md` §3.1 A05 行、`M2_A04_SCENE_PLAN.md`（拆域方法论与判据模板）
> 基线口径：`M2_BASELINE.md`；本稿数字全部为 2026-11-20 A04 提交 `5b18252` 之后的实测值

---

## 0. 一句话结论

A05 计划口径的三处数字**已全部过时**（physics 1929 / plugin 1405 / app 3040），实测为 **1542 / 1125 / 3032**。本稿把 A05 重新定义为「**收紧 + 两条债务清零**」：把三个文件的**内嵌测试块与契约类型段横切出去**，使 `r3e-physics/lib.rs` 1542 → **≈472**、`r3e-plugin/tools.rs` 1125 → **≈716**、`r3e-runtime/app.rs` 3032 → **≈2856**，顺带把 `size_debt` 债务从 **22 降到 20**（比原计划只做「数字不涨」彻底得多），且**零行为变化**。

---

## 1. 计划口径 vs 实测对账（A05-D1 的由来）

| 项 | M2_PLAN 原口径 | 实测（A04 后） | 状态判定 |
| --- | --- | --- | --- |
| `r3e-physics/src/lib.rs` | 1929 → ≤1924（"A03 双后端不得抬高本行数"） | **1542** | 原目标已被 A03 超额达成（净减 387），**≤1924 已无约束意义** |
| `r3e-plugin/src/tools.rs` | 1405 → ≤1125 | **1125** | **恰好卡在目标线上**，等于本卡无事可做 |
| `r3e-runtime/src/app.rs` | 3040 → ≤3028 | **3032** | 尚差 4 行，按原口径只需减 4 行即可交差 |

三个数字全部来自 A01 基线（`M2_BASELINE.md`），A03 / A03b 已把它们大幅改写。**若照原口径执行，A05 会退化成「减 4 行」的空卡**，而 22 条债务一条也没动——这与 A05 立项意图（"别让 A 阶段的上限变成新天花板"）相反。

### A05-D1（唯一待拍板项）

| 档位 | 动作 | 结果 | 债务 | 工作量 |
| --- | --- | --- | --- | --- |
| **档 A（最小）** | 三个文件只搬内嵌测试块 | physics 1170、tools 716、app 2856 | 22 → **21** | 小（纯搬移，零可见性改动） |
| **档 B（推荐）** | 档 A ＋ physics 的契约类型段与 backend 契约横切 | physics **≈472**、tools 716、app 2856 | 22 → **20** | 中（多 2 个新文件 + re-export 表） |
| 档 C（不推荐） | 档 B ＋ `app.rs` 的 `impl EngineApp` 拆域 | app ≈1500 | 22 → 20 | 大（需把约 40 个私有字段升 `pub(super)`，与 G02 完全重叠） |

**推荐档 B**：physics 一次到底（降到 ≤800 后删条目，门禁 800 直接接管），`app.rs` 只做判据要求的最小动作、把真正的拆域留给 G02（`M2_PLAN.md` 第 220 行已定「G02 必须保证 `git revert` 后门禁仍绿」，两张卡不该抢同一件事）。

---

## 2. 三项实测结构

### 2.1 `crates/r3e-physics/src/lib.rs` — 1542 行

| 行区间 | 行数 | 内容 | 可否外移 |
| --- | --- | --- | --- |
| 1–74 | 74 | 文件头 + `use` + 5 个 `mod` 声明（`bind` / `builtin_backend` / `host_adapter` / `port_impl` / `rapier_backend`） | 否（门面） |
| 76–203 | 128 | `impl BackendCaps`（`NONE` / `BASIC` / 若干能力查询） | **是** → `backend.rs` |
| 207–338 | 132 | `PhysicsEventKind` / `PhysicsEvent` / `BodyKind` / `ColliderShape` / `BodyDesc` 及其 `impl` | **是** → `types.rs` |
| 343–409 | 67 | `half_extents_from`（私有）、`infer_shape`（`pub`） | **是** → `types.rs` |
| 418–527 | 110 | `PhysicsKind` / `PhysicsConfig` 及其 `impl` | **是** → `types.rs` |
| 539–636 | 98 | `Ray` / `RayHit` / `OverlapHit` / `CastShape` / `ShapeCastHit` 及其 `impl` | **是** → `types.rs` |
| 639–776 | 138 | `trait PhysicsBackend`（后端契约，全部方法签名） | **是** → `backend.rs` |
| 780–791 | 12 | `cast_candidate_better`（`pub(crate)`） | **是** → `backend.rs` |
| 798–831 | 34 | `Physics` 结构体（**全私有字段**）+ `Debug` impl | 否（门面类型） |
| 839–1168 | 330 | `impl Physics`（**35 个方法**，`self.backend` 出现 34 次、`self.enabled` 12 次…） | 否（见 §3.3） |
| 1171–1542 | **372** | `mod tests`（**16 条**，`use super::*`） | **是** → `tests.rs` |

**关键约束**：`impl Physics` 的 35 个方法重度读写私有字段（`backend` 34 次、`enabled` 12 次、`kind` 5 次、`gravity` 4 次），外移必须把字段全升 `pub(super)`——这是档 B **不**碰该块的原因。测试块反过来可以安全搬走：`mod tests;` 仍声明在 crate 根，`use super::*` 的 `super` 依旧指向 `r3e_physics`，私有项照样可见（A04 已在 `scene/` 验证同一手法）。

### 2.2 `crates/r3e-plugin/src/tools.rs` — 1125 行

| 行区间 | 行数 | 内容 |
| --- | --- | --- |
| 1–712 | 712 | `ToolError` / `HostBridge` / `invoke` / `invoke_command` / 19 个工具实现（`engine_info`、`scene_patch` 98 行、`capture_frame` 38 行…）/ 9 个参数解析 helper |
| 715–1125 | **411** | `mod tests`（**23 条**，`use super::*`，含 TCP 打桩 `rpc_error` 用例） |

搬走测试后 **714 行**，加 2 行 `mod` 声明为 716 → **≤800，可直接删条目**。原计划只需「≤1125（不涨）」，本方案实为超额达成。

### 2.3 `crates/r3e-runtime/src/app.rs` — 3032 行

| 行区间 | 行数 | 内容 |
| --- | --- | --- |
| 46–262 | 217 | `FsUiImageProvider` / `MoveKeys` / `HeadlessReport` / `CliOptions` |
| 264–365 | 102 | `EngineApp` 结构体（**约 40 个私有字段**） |
| 367–2412 | **2046** | `impl EngineApp`（主体，重度用私有字段） |
| 2414–2543 | 130 | `impl ApplicationHandler for EngineApp` |
| 2546–2841 | 296 | 12 个自由函数（`spawn_console` / `load_env_probe` / `collect_asset_specs` / `dump_anim` …） |
| 2851–2932 | 82 | `mod morph_bind_tests`（**4 条**，`use super::*`） |
| 2935–3032 | **98** | `mod key_event_tests`（**4 条**，`use super::*`） |

只搬两个测试模块（180 行）→ **≈2856 行**，过 A05 原口径 ≤3028。`impl EngineApp`（2046 行）与 296 行自由函数段**本卡不动**，理由同上：改私有字段可见性 40 处属于 G02 的作业面。

---

## 3. 方案（档 B）

### 3.1 目标文件与预算

| crate | 新文件 | 行数预算 | 承担内容 |
| --- | --- | --- | --- |
| `r3e-physics` | `src/types.rs`（新建） | ≈407 | `PhysicsEvent*` / `BodyKind` / `ColliderShape` / `BodyDesc` / `PhysicsKind` / `PhysicsConfig` / `Ray*` / `CastShape` / `ShapeCastHit` + `infer_shape`、`half_extents_from` |
| `r3e-physics` | `src/backend.rs`（新建） | ≈290 | `trait PhysicsBackend` + `impl BackendCaps` + `cast_candidate_better` |
| `r3e-physics` | `src/tests.rs`（新建） | 372 | 原 `mod tests`（16 条） |
| `r3e-physics` | `src/lib.rs`（改） | **≈472** | 文件头、`use`、`mod` 声明与 re-export 表、`Physics` 结构体、`impl Physics`、`Debug`/`Default` |
| `r3e-plugin` | `src/tools/tests.rs`（新建） | 411 | 原 `mod tests`（23 条） |
| `r3e-plugin` | `src/tools.rs`（改） | **≈716** | 全部生产代码 + `#[cfg(test)] mod tests;` |
| `r3e-runtime` | `src/app/tests_morph_bind.rs`（新建） | 82 | 原 `morph_bind_tests`（4 条） |
| `r3e-runtime` | `src/app/tests_key_event.rs`（新建） | 98 | 原 `key_event_tests`（4 条） |
| `r3e-runtime` | `src/app.rs`（改） | **≈2856** | 其余全部内容 + 两行 `#[cfg(test)] mod` |

> 命名说明：`r3e-plugin` 已在 `src/` 有 `host.rs` / `rpc.rs` / `spec.rs` 等扁平文件，测试落 `tools/tests.rs` 用**目录式**（`tools.rs` 与 `tools/` 并存，A04 已确认 Rust 2021 允许）；`r3e-runtime` 同理用 `app/`。

### 3.2 physics 的模块组织与 re-export 表（本方案最需谨慎处）

`lib.rs` 顶部改为：

```rust
mod backend;
mod bind;
mod builtin_backend;
mod host_adapter;
mod port_impl;
mod rapier_backend;
mod types;

#[cfg(test)]
mod tests;                      // 16 条原样

pub use backend::{BackendCaps, PhysicsBackend};
pub use types::*;               // 描述类型：路径仍是 r3e_physics::BodyDesc 等
pub(crate) use types::{half_extents_from, infer_shape};   // 兼容 bind.rs 的 `use crate::{…}`
pub use bind::BindReport;       // 既有，不动
```

**为什么 `pub use types::*` 而不是逐个点名**：外部只用到 `r3e_physics::BodyDesc`（`r3e-editor-host/src/api/verify.rs:54`）与文档注释里提到的 `BodyDesc::half_extents`（`r3e-protocol/src/probe.rs:51`），但 `bind.rs` / `builtin_backend.rs` / `rapier_backend.rs` 内部通过 `use crate::{BodyDesc, BodyKind, ColliderShape, CastShape, BackendCaps, …}` 一次性导入十余项（`bind.rs:15`、`builtin_backend.rs:23`）。逐个点名极易漏项；通配再导出能让**三个后端文件一行都不用改**。

**`half_extents_from` / `infer_shape`**：前者原为 `fn`（crate 私有），后者为 `pub fn`。搬进 `types.rs` 后统一写 `pub(crate) fn half_extents_from`、`pub fn infer_shape`，再由 lib.rs `pub(crate) use` 暴露回 `crate::` 路径，`bind.rs:15/222/227` 三处调用点零改动。

**`infer_shape` 是否对外**：`git grep` 只有 10 处引用，全部在本 crate 内（含 `bind.rs` 与测试），无外 crate 依赖，故不强制 `pub use`；但保持 `pub fn` 以免改变既有可见性。

### 3.3 明确不做的事

| 不做 | 理由 |
| --- | --- |
| 外移 `impl Physics`（330 行 / 35 方法） | 需把 `backend`/`enabled`/`kind`/`gravity` 等私有字段升 `pub(super)`，属真实封装破坏；且搬走后 lib.rs 也只降到 ~150，收益不值 |
| 外移 `impl EngineApp`（2046 行） | 同上，约 40 个私有字段；且是 **G02** 的作业面，`M2_PLAN.md` 第 220 行要求两卡各自独立可 `git revert` |
| 动 `Physics` 结构体本身 | 门面类型的字段声明是私有封装的载体，任何搬移都得放宽可见性 |
| 改任何签名 / 删除任何测试 | 零行为变化是本卡的唯一硬判据 |

### 3.4 可见性处置矩阵

| 符号 | 原可见性 | 搬后 | 是否需要 re-export | 影响面 |
| --- | --- | --- | --- | --- |
| `BodyDesc` / `BodyKind` / `ColliderShape` / `PhysicsKind` / `PhysicsConfig` | `pub` | `pub`（`types.rs`） | **是**（`pub use types::*`） | 外部 1 处、内部 3 文件 |
| `PhysicsEvent` / `PhysicsEventKind` / `Ray` / `RayHit` / `OverlapHit` / `CastShape` / `ShapeCastHit` | `pub` | `pub` | **是**（同上） | 内部 + `r3e-editor-host` |
| `BackendCaps` | `pub` | `pub`（`backend.rs`） | **是**（`pub use backend::{BackendCaps, PhysicsBackend}`） | `bind.rs`、`builtin_backend.rs`、`rapier_backend.rs`、`editor-host` 注释 |
| `trait PhysicsBackend` | `pub` | `pub` | **是**（同上） | 全部后端实现 |
| `infer_shape` | `pub fn` | `pub fn` | **是**（`pub(crate) use`） | 本 crate 内 4 处 |
| `half_extents_from` | 私有 `fn` | `pub(crate) fn` | **是**（`pub(crate) use`） | 本 crate 内 3 处 |
| `cast_candidate_better` | `pub(crate) fn` | `pub(crate) fn` | **是**（`pub(crate) use`） | `builtin_backend.rs`、`rapier_backend.rs` |
| `Physics` / `impl Physics` | `pub` | 留在 `lib.rs` | 否 | — |
| `ToolError` / `invoke` / 19 个工具 fn / 9 个 helper | 各自 | 留在 `tools.rs` | 否 | 测试经 `super::*` 访问，照旧 |
| `EngineApp` / `impl EngineApp` / 12 个自由函数 | 各自 | 留在 `app.rs` | 否 | 测试经 `super::*` 访问，照旧 |

---

## 4. 零行为四保证

1. **公开 API 零变化**：所有 `pub` 项的**最终路径不变**（靠 re-export 兜底，与 `r3e-editor` 只做 `pub use r3e_protocol::*` 的既有先例一致，见 `ARCHITECTURE.md:149`）。签名、trait 方法集合、泛型参数一概不动。
2. **测试零删减**：physics 16 条、plugin 23 条、runtime 8 条原样搬移，`use super::*` 语义等价（`mod` 声明仍在原模块根，`super` 解析不变）。全仓合计 **2025 → 2025**。
3. **最小可见性放宽**：本卡只把 2 个 crate 私有 fn 提为 `pub(crate)`（均为同 crate 内跨模块调用所必需），**不新增任何 `pub(super)` 字段**、不新增对外 `pub` 项。
4. **回放金标不漂移**：`final=385482eb033d2964`（600 帧 / 10 采样点）两次运行同哈希。

---

## 5. 执行批次（6 批，串行）

| 批 | 动作 | 验证 |
| --- | --- | --- |
| **B0** 预检 | 备份三文件；记录测试数（physics 16 / plugin 23 / runtime 8）与外部引用面（`git grep r3e_physics::`、`crate::half_extents_from` 等）；确认无 `include_str!` 读这三个文件源码 | 采集结果入档 |
| **B1** physics 类型段 | 建 `types.rs`（207–636 共 430 行，含 `infer_shape`/`half_extents_from`）、`backend.rs`（76–203 + 639–791 共 278 行）；lib.rs 加 `mod` 与 re-export 表，删除已搬块 | `cargo check -p r3e-physics` 零 error/warning |
| **B2** physics 测试 | 建 `tests.rs`（1171–1542）；lib.rs 加 `#[cfg(test)] mod tests;` | `cargo test -p r3e-physics` 16 条全绿 |
| **B3** plugin 测试 | `tools.rs` 的 `mod tests` → `tools/tests.rs`；加 `#[cfg(test)] mod tests;` | `cargo test -p r3e-plugin` 23 条全绿 |
| **B4** runtime 测试 | 两个测试模块 → `app/tests_morph_bind.rs`、`app/tests_key_event.rs`；加两行 `#[cfg(test)] mod` | `cargo test -p r3e-runtime` 8 条全绿 |
| **B5** 台账与收口 | `xtask/size_debt.json` 删 `r3e-physics/lib.rs`、`r3e-plugin/tools.rs` 两条（22 → 20），`app.rs` 的 `base` 3032 → 实测；`cargo fmt --all`、`clippy -D warnings`、`cargo xtask verify`（11 步） | 全绿后单次提交 |

每批之间跑一次 `cargo fmt --all`，避免 B5 一次性重排掩盖真实 diff。

---

## 6. 判据（11 条，全过才算 A05 完成）

| # | 判据 | 口径 |
| --- | --- | --- |
| 1 | `cargo xtask verify` | 11 步全绿 |
| 2 | `r3e-physics/src/lib.rs` | **≤800**，且 `size_debt.json` 该条已删 |
| 3 | `r3e-plugin/src/tools.rs` | **≤800**，且该条已删 |
| 4 | `r3e-runtime/src/app.rs` | **≤3028**（A05 原口径），条目保留、`base` 收紧为实测值 |
| 5 | 债务条数 | **22 → 20**，`lint-size` 白名单 20 项全部未上涨 |
| 6 | 测试总数 | **2025 → 2025**（physics 16 / plugin 23 / runtime 8 原样） |
| 7 | 公开 API | `r3e_physics::{BodyDesc, BodyKind, ColliderShape, PhysicsKind, PhysicsConfig, BackendCaps, PhysicsBackend, Physics, BindReport, …}` 路径与签名不变；`git grep 'r3e_physics::'` 的调用方零改动 |
| 8 | 回放金标 | `385482eb033d2964` 两次一致 |
| 9 | `fmt --check` / `clippy -D warnings` | 全绿、零警告 |
| 10 | 新文件体量 | 4 个新文件全部 ≤800（最大 `tools/tests.rs` 411） |
| 11 | 门禁资产 | 只改 `size_debt.json`；`tests_baseline.json`、`dep_rules.json`、`dep_debt.json`、`docs/capabilities.json` 未动 |

---

## 7. 风险与处置

| # | 风险 | 触发信号 | 处置 |
| --- | --- | --- | --- |
| R1 | re-export 漏项导致后端文件编译失败 | `cargo check -p r3e-physics` error | 用 `pub use types::*` 通配兜底而非点名；报错即补，不猜 |
| R2 | `crate::half_extents_from` 路径断裂 | `bind.rs:15` unresolved import | 保留 `pub(crate) use types::{half_extents_from, infer_shape}` |
| R3 | 测试搬移后 `super` 解析错位 | 测试编译失败或 0 条运行 | `mod` 声明必须留在原模块根（`lib.rs` / `tools.rs` / `app.rs`），不得搬进子目录的 `mod.rs` |
| R4 | 搬移时用裸 `}` 判界导致函数体残缺 | 编译报 `unexpected closing delimiter` / 语法错 | 一律用**括号深度计数**定位块尾（M1-S4 与 A04 各踩过一次） |
| R5 | doc_gate 因 pub 项缺文档失败 | verify doc 步红 | 搬移保持原 doc 注释一字不减，新增 re-export 行补一行 `///` |
| R6 | 后续卡把代码写回 `tools.rs` / `physics/lib.rs` 使其涨回 800 以上 | 下一卡 `lint-size` 红 | 写入本稿 §9 禁区：两个文件此后只做门面，新代码落子树 |
| R7 | `rustfmt` 重排跨文件 `use` 顺序，diff 变大 | B5 出现大量仅顺序变化的 hunk | 预期行为，不算偏离；每批先 fmt 以隔离 |
| R8 | 误把 `app.rs` 拆域一并做进来 | 改动量突然膨胀、需改 40 处字段可见性 | 严格按 §3.3 禁区执行，`app.rs` 只搬测试块 |

---

## 8. 与后续卡的咬合

- **B 阶段（B01 起）**：`r3e-physics/lib.rs` 与 `r3e-plugin/tools.rs` 删条目后受 800 硬门禁约束，B 阶段若要给物理加查询能力、给插件加工具，一律落 `types.rs` / `backend.rs` 或新建子树文件，**不回写主文件**。
- **G02（`app.rs` 拆域）**：本卡把 `app.rs` 降到 ≈2856 并保留条目；G02 的目标仍是 ≤1200（`M2_PLAN.md` 第 59 行），届时需处理 `impl EngineApp` 的字段可见性——本卡已把测试块清走，G02 可直接面对 2046 行主体。
- **G04 / H05**：本卡不动任何热路径代码（physics 只搬类型声明与契约 trait，`impl Physics` 一行未动），不产生性能回归面。

---

## 9. 禁区

1. 不改 `impl Physics` 的 35 个方法（含 `step` / `raycast` / `shape_cast` 等热路径），不改其私有字段可见性。
2. 不改 `impl EngineApp`、不改 `EngineApp` 的任何字段可见性（G02 作业面）。
3. 不删除、不重命名、不合并任何测试函数；不改任何断言。
4. 不改 `xtask/tests_baseline.json`（测试数不变，无需动）。
5. 不动 `dep_rules.json` / `dep_debt.json` / `docs/capabilities.json`（A05 无新增依赖、无新增命令）。
6. 不为了让数字好看而改 `size_debt.json` 的 `base` 之外的任何字段；`app.rs` 的 `base` 只允许改成**实测行数**。
7. 不新增 `pub` 项、不改既有 `pub` 项签名。

---

## 10. 采集命令（PowerShell）

```powershell
# 三文件实测行数与块边界
(Get-Content crates/r3e-physics/src/lib.rs).Count
(Get-Content crates/r3e-plugin/src/tools.rs).Count
(Get-Content crates/r3e-runtime/src/app.rs).Count

# 外部引用面（决定 re-export 兜底范围）
git grep -n 'r3e_physics::' -- crates
git grep -n 'use crate::{' -- crates/r3e-physics/src

# 测试条数
git grep -c '#\[test\]' -- crates/r3e-physics/src/lib.rs crates/r3e-plugin/src/tools.rs crates/r3e-runtime/src/app.rs
cargo xtask check-tests

# 白名单现状（22 项）
Get-Content xtask/size_debt.json -Raw | ConvertFrom-Json | Select-Object -Expand files |
  Get-Member -MemberType NoteProperty | Where-Object Name -match 'physics|tools|app'
```

---

## 11. 待老板确认的一处

**A05-D1**：执行 **档 A**（只搬测试块：physics 1170 / tools 716 / app 2856，债务 22 → 21）还是 **档 B**（推荐：再加 physics 的 `types.rs` + `backend.rs` 横切，physics ≈472、债务 22 → 20）。

档 B 多出的收益是 `r3e-physics/lib.rs` 也进 800 硬门禁（债务再少一条），代价是引入 2 个新文件与一张 re-export 表；风险已由 `pub use types::*` 通配导出压到最低，且 `bind.rs` / 两个后端文件零改动。档 C（连 `app.rs` 一起拆）不推荐，与 G02 撞车。
*（内容由AI生成，仅供参考）*

---

## 12. 实施记录（2026-11-20，A05-D1 取档 B）

### 12.1 实际落点与行数

| 文件 | 前 | 后 | 说明 |
|---|---|---|---|
| `crates/r3e-physics/src/lib.rs` | 1542 | **461** | 只留门面：`mod` 声明 + `pub use` re-export；`size_debt` 条目已删 |
| `crates/r3e-physics/src/types.rs` | — | **438** | 新建：契约类型段 + `infer_shape` / `half_extents_from`（后者升 `pub(crate)`） |
| `crates/r3e-physics/src/backend.rs` | — | **289** | 新建：`BackendCaps` impl + `PhysicsBackend` trait + `cast_candidate_better` |
| `crates/r3e-physics/src/tests.rs` | — | **372** | 新建：原内嵌 16 条测试整块，逐条搬移零删减（去 `mod tests` 包裹，`use super::*` 留在文件顶层） |
| `crates/r3e-plugin/src/tools.rs` | 1125 | **715** | `size_debt` 条目已删 |
| `crates/r3e-plugin/src/tools/tests.rs` | — | **411** | 新建：原内嵌 23 条测试 |
| `crates/r3e-runtime/src/app.rs` | 3032 | **2848** | 留 base、条目保留（G02 拆域用）；base 由 3032 → 2848 |
| `crates/r3e-runtime/src/app/morph_bind_tests.rs` | — | **82** | 新建：`#[path]` 引入 |
| `crates/r3e-runtime/src/app/key_event_tests.rs` | — | **98** | 新建：`#[path]` 引入 |
| `crates/r3e-runtime/src/asset_scene.rs` | 493 | **500** | 承接 `app.rs` 尾部孤儿文档注释（见 §12.3） |

`size_debt.json`：白名单 **22 → 20** 条（`physics/lib.rs`、`plugin/tools.rs` 移除；`runtime/app.rs` 记 2848）。新增 5 个文件全部 ≤800，最大 `types.rs` 438。

### 12.2 判据逐条核对（11 条）

| # | 判据 | 结果 |
|---|---|---|
| 1 | `cargo xtask verify` 11 步全绿 | ✅ RC 0 |
| 2 | `physics/lib.rs ≤800` 且删条 | ✅ 461，条目已删 |
| 3 | `plugin/tools.rs ≤800` 且删条 | ✅ 715，条目已删 |
| 4 | `runtime/app.rs ≤3028` 条留 base 收紧 | ✅ 2848 |
| 5 | `size_debt` 债务 22 → 20 | ✅ 20 |
| 6 | tests 2025 零删减 | ✅ 2025 → 2025（physics 238 / plugin 47 / runtime 14 均持平） |
| 7 | API 路径不变 | ✅ 编译 + 全测试通过；`pub use types::*` / `backend::*` 兜住原路径 |
| 8 | 回放金标不漂移 | ✅ `385482eb033d2964` |
| 9 | fmt / clippy `-D warnings` 绿 | ✅ |
| 10 | 新文件 ≤800 | ✅ 最大 438 |
| 11 | 只改 `size_debt.json`（台账） | ⚠️ 另有两处门禁接缝修正，见 §12.3，均为「拆文件后的必然适配」，不涉行为 |

### 12.3 计划外的三处接缝修正（不涉行为，需知晓）

1. **`xtask/doc_gate.json` 例外登记改路径**：`fn sync_to_scene(&mut self, scene: &mut Scene);` 的写点从 `r3e-physics/src/lib.rs` 移到 `src/backend.rs`，doc-gate 规则 2 白名单按「路径 + needle + count」精确匹配，不改路径则判「漏登记 1 条 + 可下调 1 条」而失败。只改该条 `path` 字段，例外条数不变（10 条）。
2. **`crates/r3e-render/tests/architecture_guard.rs` 扩一处排除**：守卫的 `production_source` 只按文本截断到第一个 `#[cfg(test)]`，对「整个文件就是测试模块」（`tests.rs` / `*_tests.rs`）没有标记可截，导致 `physics/tests.rs` 里的 `BuiltinMesh` 被误判为生产代码来源分支。新增 `is_test_module_file` 按文件名整文件排除，判定意图不变（测试里本可构造内置图元做对照）。同时因去掉 `mod tests` 包裹，顺便消掉 clippy `module_inception`。
3. **`app.rs` 尾部孤儿文档注释归位**：`app.rs` 原 2843–2849 行一段描述 `spawn_asset_nodes` 的 `///` 文档注释，A04/P5 把该函数迁到 `asset_scene.rs` 时注释留在原地，只因紧贴 `#[cfg(test)] mod morph_bind_tests` 才没被 clippy 抓到；A05 搬走测试块后 `empty_line_after_doc_comments` 立刻报错。处置 = 注释搬到 `asset_scene.rs` 的 `spawn_asset_nodes` 上方（原函数无文档注释），不删内容。

### 12.4 档 A 的取舍回收

档 A（只搬测试块）会把 physics 留 1170 行、仍超 `limit=800`，债务只能 22 → 21；档 B 用 `types.rs` + `backend.rs` 两刀横切一次清掉两条，且 `bind.rs` / `builtin_backend.rs` / `rapier_backend.rs` 因通配 re-export 零改动 —— 与规划预测一致。
