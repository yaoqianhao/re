---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_8f4c2259e33d11f18c5752540024e231
    ReservedCode1: aqyqS0ntEzkbAUeTNhtaVGi5dfoFVU3iylvVEKtFvcQD3HlDL4GgIA9kkMPvhxq5ELae0+ejnDE8cEmS50Whuds1Rey9wXaT/K3bpu0r425m3yh2gjvQtyqL/s9Ber4NQ0kaXTYxoOFXsMEtCr3cjflakMEfEWFABzFKVZwQXU7piYnu8bpOznpe7OQ=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_8f4c2259e33d11f18c5752540024e231
    ReservedCode2: aqyqS0ntEzkbAUeTNhtaVGi5dfoFVU3iylvVEKtFvcQD3HlDL4GgIA9kkMPvhxq5ELae0+ejnDE8cEmS50Whuds1Rey9wXaT/K3bpu0r425m3yh2gjvQtyqL/s9Ber4NQ0kaXTYxoOFXsMEtCr3cjflakMEfEWFABzFKVZwQXU7piYnu8bpOznpe7OQ=
---

# M1-S4-F 执行方案 V1：`scene.verify` + `impact.violations` + 三夹具 + Gate 1 前置

> 状态：**已落地**（D1=A / D2=P1 / D3=子目录夹具；两提交，本地未 push）
> 补充：D1 的 B/C（物理 / 导航层 `violations`）已于 M1-S4-G 补齐，见 §5.1 末两行与 §8
> 日期：2026-11-18
> 前置：S4-E 已收口（commit `7b2a929`）；RPC 57、命令表 387、`ISSUE_CODES` 38、`#[test]` 1,718
> 归属：`docs/M1_S4_PLAN.md` §4 S4-F；父计划 `docs/AI_ROADMAP.md` §3.2 M1-S4

---

## 0. 一句话结论

父计划写的「`tx.intent` 的 `impact` 增 `violations`（**提交后**自动复验）」在编辑态**不成立**：宿主没有物理世界装配入口，也拿不到 `MeshProvider`，提交后复验的物理层会基于过期世界下结论——这是在假绿。本方案把 `violations` 落在**事务影子场景的静态层**（零副作用、可判死、便宜），物理 / 导航层在回包里如实记账为未复验；Gate 1 要求的「`unchecked` 为空」由 `xtask scene-verify` 在自有物理 / 导航实例上满足（那边世界是刚装配的，结论可信）。

---

## 1. 实证快照（开工前只读核查）

| # | 事实 | 实证位置 |
|---|---|---|
| E1 | 协议面四处同改点：trait 默认 `Err(RpcFailure::unsupported(..))` / dispatch 分派 / `RPC_METHODS` / `cmd_spec` 命令表（带 `rpc: Some(..)`） | `crates/r3e-editor/src/api.rs`（trait，`physics.raycast` 样板在 `crates/r3e-protocol/src/api.rs:420` 附近）、`crates/r3e-protocol/src/dispatch/physics.rs:19`、`crates/r3e-protocol/src/lib.rs:90/110`、`crates/r3e-core/src/cmd_spec/meta.rs:297` |
| E2 | 三层验证器入口已就绪：`VerifyReport::collect(scene, physics, nav)` / `collect_with_targets(.., targets)`，内部顺序 static → physics → nav → `dedup_superseded` | `crates/r3e-editor-host/src/verify/mod.rs:82/89` |
| E3 | `passed()` 只看 `error_count`，调用方**必须同时看 `unchecked`**（S4-E 注释已写明） | `crates/r3e-editor-host/src/verify/mod.rs`（`unchecked()` 访问器） |
| E4 | 宿主持三处引用：`scene: &Rc<RefCell<Document>>`、`physics: &Rc<RefCell<Physics>>`、`nav: Option<&RefCell<Option<Nav>>>` | `crates/r3e-editor-host/src/api/mod.rs`（`pub struct LocalEngineApi`） |
| E5 | **编辑态不存在物理世界装配**：全仓 `bind_scene` 调用点只有 `r3e-physics/tests/*` 与 `r3e-runtime/src/physics_bind.rs:50` | `git grep 'bind_scene('` |
| E6 | 宿主拿不到网格来源：`MeshProvider` 实现只有 `AssetLibrary` / `MeshAabbCache` / `PickBounds` / `ProceduralProvider` / `LayeredProvider`(render) / `SceneMeshProvider`(runtime)；`LocalEngineApi` 无 assets 字段，`host.rs:48` 构造处也没有 | `crates/r3e-editor-host/Cargo.toml`、`crates/r3e-editor-host/src/host.rs` |
| E7 | `SceneMeshProvider` + `rebind` 是 runtime **私有**（`mod physics_bind;`），宿主无法复用 | `crates/r3e-runtime/src/lib.rs`、`crates/r3e-runtime/src/physics_bind.rs:28/50` |
| E8 | 事务影子可读且零副作用：`tx_session()`、`TxSession::shadow() -> &Scene`；`tx_intent` 的命令只在影子上 `apply` | `crates/r3e-doc/src/tx.rs:118/205/298/326` |
| E9 | `tx.intent` 回包位置与 `Impact::to_json()` 手写序列化 | `crates/r3e-editor-host/src/api/tx.rs`（`tx_intent` 末尾 `json!`）、`Impact` 定义于同文件 |
| E10 | 夹具目录约束：`assets/` **顶层**的 `scene*.json` 会被 schema 全量扫（非递归）；`shipped_scenes_have_no_errors` 是显式 11 条清单 | `crates/r3e-core/tests/scene_schema.rs:121`、`crates/r3e-doc/tests/validate.rs` |
| E11 | `cargo xtask verify` 现 10 步；`xtask` 依赖仅 core/doc/protocol，且**不在 dep-check 扫描范围** | `xtask/src/main.rs:141`、`xtask/Cargo.toml` |
| E12 | `Issue` 已坐标化：`severity/code/category/path/target/msg/location/hint/approximate` | `crates/r3e-doc/src/validate/mod.rs` |

**由 E5 + E6 + E7 推出的硬结论**：在编辑态（`r3e-editor-host`）里对 `physics` 层下「基于当前场景」的结论，要么基于过期世界，要么需要宿主自己装配一套（等价于复制第三份装配 + 网格判定，违反「装配唯一入口」铁律）。

---

## 2. 交付物拆解

| 卡 | 内容 | 交付物 |
|---|---|---|
| F-1 | `scene.verify` 协议面（E1 四处同改，56 → 57） | `r3e-protocol/src/{lib,api,dispatch/*}`、`r3e-core/src/cmd_spec/meta.rs` |
| F-2 | 宿主实现 `api/verify.rs`：把三层结果翻成回包 JSON | `crates/r3e-editor-host/src/api/verify.rs` |
| F-3 | `impact.violations`（口径按 D1 裁决） | `crates/r3e-editor-host/src/api/tx.rs` |
| F-4 | 三个反例夹具 + 正向对照 | `assets/verify/*.json`（按 D3） |
| F-5 | `xtask scene-verify` + 接入 `verify` 第 11 步 | `xtask/src/scene_verify.rs`、`xtask/src/main.rs:141` |
| F-6 | 台账同步：`capabilities.json`、`AI_ROADMAP` §3.2、`M1_PLAN.md §5` 写点登记 | `docs/*`、`dist/capabilities.json` |

预估 2 提交：F-1+F-2（RPC 落地）→ F-3+F-4+F-5+F-6（impact + 夹具 + 门禁）。

---

## 3. `scene.verify` 回包契约（提案）

入参：

```json
{ "targets": [{"name": "boss", "at": [x, y, z]}], "layers": ["static","physics","nav"], "limit": 200 }
```

回包：

```json
{
  "ok": false,
  "passed": false,
  "counts": { "error": 2, "warn": 5, "total": 7 },
  "issues": [{
    "severity": "error", "code": "overlap.exact", "category": "overlap",
    "path": "nodes[3]", "target": "crate|barrel",
    "location": [1.25, 0.5, -3.0], "hint": "把 crate 抬高 0.35 米或水平移开",
    "approximate": false, "msg": "…"
  }],
  "unchecked": [{ "layer": "physics", "reason": "物理世界未装配当前场景：本次不下结论" }],
  "layers": { "static": true, "physics": false, "nav": false },
  "truncated": false,
  "stateHash": "0x…"
}
```

口径：

- `ok` = 无 `Error`（沿用 E3 的既有语义，**不重定义**）；
- `passed` = `ok && unchecked.is_empty()`——Gate 1 用的是这个；两者分开返回，避免「没检查」被读成「检查通过」；
- `limit` 截断时 `truncated: true`，与既有 `query.*` 同一套分页口径；
- 错误码复用既有族：物理关着 → `PHYSICS_DISABLED`；导航槽位 `None` → `NAV_NOT_BAKED`（不放回包，因为这三条是「层不可用」，进 `unchecked` 而不是整条 RPC 失败）。

---

## 4. 待裁决事项

### D1 `violations` 挂在哪里、覆盖哪几层

| 选项 | 做法 | 代价 |
|---|---|---|
| **A（建议）** | `tx.intent` 回包的 `impact` 增 `violations`：对**影子场景**跑静态层，与「事务基线（`tx.begin` 时主场景的静态层结果）」求差，只报新增；字段 `{ count, layers: ["static"], issues: [...], notChecked: ["physics","nav"] }` | 只能覆盖静态层；物理 / 导航层的新增要等 `scene.verify` |
| B | `tx.commit` 回包增 `violations`：提交后三层复核，与提交前快照 diff | 编辑态物理层基于过期世界（E5/E6）→ 假绿；每笔提交跑一次 `shape_cast`，大场景开销随装配体数线性增长 |
| C | A + B 双写 | 范围翻倍，且 B 的缺陷仍在 |

**建议 A**，并把「提交后全层复验」明确写进 M2 债：等资产库注入宿主（E6 缺口补上）再做。

### D2 `scene.verify` 的物理 / 导航层如何保真

| 选项 | 做法 | 代价 |
|---|---|---|
| **P1（建议）** | 不自行装配。物理层照跑 `verify::physics::check`（它只读 `&Physics` 并从场景推 `BodyDesc`），但用**世界新鲜度守卫**：`physics.body_count()` 与场景装配体数不一致时，物理层条目降级为静态近似 + `unchecked: world_not_synced`，回包 `layers.physics = false` | 编辑器态通常拿不到物理层精确结论；Gate 1 由 `xtask scene-verify` 满足 |
| P2 | 宿主内 `bind_scene` 后再复核 | 需要 `MeshProvider`：`SceneMeshProvider` 现私有（E7），且宿主无 assets（E6）→ 必须先把资产库注入 `LocalEngineApi` 并公开 runtime 的装配半边，范围外扩张 |
| P3 | 强制调用方先跑 `physics.bind` 新 RPC | 新增一条 RPC，超 S4-F 范围 |

**建议 P1**；P2 作为 M2 前置债单独登记（收益：编辑器内 `physics.*` 探测与 verify 结论同时变可信）。

### D3 夹具位置与 `xtask` 依赖

- 夹具放 `assets/verify/`（**子目录**）：遵守 E10 的顶层扫描约束，`scene_schema.rs` 非递归 → 零污染；`shipped_scenes_have_no_errors` 是显式清单 → 也不受影响。
- `xtask` 新增 `r3e-physics` / `r3e-nav` / `r3e-editor-host` 依赖（E11：xtask 不在 dep-check 扫描范围，`Cargo.toml` 注释已声明该前提）。
- 三个夹具的预期结论（Gate 1 验收表，**实际码名为 S4-E 落地值**——提案时的 `*.exact` 是设想，`ISSUE_CODES` 收口用的是下面这三个）：

| 夹具 | 必报码（带 `location`） | 层 | 正向对照 |
|---|---|---|---|
| `overlap_bad.json` | `overlap.penetration` | physics | `overlap_ok.json`（拉开到 1 米间隙） |
| `floating_bad.json` | `support.floating`（**warn 级**：报出但不拦 `passed`） | physics | `floating_ok.json`（落回 y=0.5 贴地） |
| `unreachable_bad.json` | `nav.no_path` | nav | `unreachable_ok.json`（目标点落回西岛） |

每个夹具同时要有一条**正向对照**（同一几何改一处即 `passed`），防止「永远报错」的假红。夹具自带地形与 `spawns`，因此 `xtask scene-verify` 侧导航可逐场景烘焙、`unchecked` 为空。

---

## 5. 验收口径

1. `scene.verify` 对三夹具稳定报出 `location`（非布尔），且 `xtask scene-verify` 侧 `unchecked` 为空；
2. RPC 56 → 57、命令表 386 → 387、`ISSUE_CODES` 38 → 38（新增码须先登记，否则 `xtask invariants` 红）；
3. `#[test]` 1,705 → 实测 **1,718**（协议契约 4 条并入 `tests/scene_verify_rpc.rs`、宿主实现 18 条、`impact.violations` 4 条）；
4. `cargo xtask verify` **11 步全绿**；`capabilities.json` 重生成后 `--check` 通过；
5. 提交仍**只本地 commit，不 push**（沿用既定约束）。

### 5.1 落地结果（2026-11-18 实测）

| 项 | 结果 |
|---|---|
| 提交 | `a9019a1`（F-1+F-2：RPC + 宿主体检入口）· 第二提交（F-3+F-4+F-5+F-6：impact + 六夹具 + `scene-verify` + 台账） |
| `scene.verify` | RPC 57、命令表 387；回包 `ok` / `passed` 分离，`unchecked` 逐层记账 |
| `impact.violations` | 影子静态层 vs `tx.begin` 基线求差，`notChecked: ["physics","nav"]`；4 条单测（新增要报 / 既有不重报 / 无新问题报零 / 提交后基线推进） |
| 夹具 | `assets/verify/` 6 个（3 反例 + 3 正向对照）+ `expect.json` |
| `xtask scene-verify` | `verify` 第 11 步；6/6 通过；**元检查**：故意把 `overlap_ok` 的 `must` 写成 `overlap.penetration` → 门禁按预期红（`rc=1`），证明它抓得住 |
| ~~未做~~ → 已补 | 物理 / 导航层的 `violations`（D1=B/C）已由 **M1-S4-G** 补齐：`impact.violations` 的 `layers` 现在按层登记 `static` / `physics` / `nav`，`notChecked` 只留真的没跑的层 |
| 未做 | `physics.bind` 类新 RPC（D2=P3，仍列 M2 前置债：编辑态一次「先绑后验」是原子动作，值得一条专用 RPC，本轮不扩） |

实现期与提案的两处偏差，均为实证修正而非缩水：

1. **码名**：提案写 `overlap.exact` / `support.exact`，S4-E 实际落的是 `overlap.penetration` / `support.floating` / `nav.no_path`；
2. **悬空是 `warn` 级**：`support.floating` 必须报出、但不拦 `passed`，所以 `floating_bad` 的期望是「报出 + `passed = true`」——门禁的 `must` 与 `passed` 是两件独立的事，写反了会得到一条永远红的假红。

---

## 6. 风险与回退

| # | 风险 | 对策 |
|---|---|---|
| R1 | `violations` 基线求差写错，把「老问题」当「新引入」反复报 | 基线用 `tx.begin` 时的静态层快照（不是空集）；单测覆盖「事务前后都存在的违规 → `violations` 为空」 |
| R2 | 世界新鲜度守卫误判（`body_count` 相等但世界是旧的） | 守卫取 `(body_count, 场景装配体数)` + 不相等即判 stale；宁可多报 `unchecked`，不报错结论 |
| R3 | 夹具放进 `assets/` 顶层被 schema 测试扫到 | 子目录方案（D3），并在测试里断言「夹具目录不被 schema 扫描」 |
| R4 | `xtask` 新依赖把 workspace 编译时间拉长 | 只加必需的三个 crate；`scene-verify` 走已编译产物，无额外构建 |

**回退**：F-1/F-2（RPC）与 F-3（impact）分两个提交，可分别 revert；夹具与 `xtask` 步骤独立提交，删除即回退。

---

## 7. 明确不做

- 不做 `physics.bind` / `scene.rebind` 类新 RPC（D2 的 P3）——已登记为 M2 前置债，见 §8；
- ~~不做 `violations` 的物理 / 导航层（D1 的 B/C）~~ → M1-S4-G 已补，见 §8；
- 不做 Gate 1 演练本身（S5 范围），本卡只交付「Gate 1 可以开始了」的前置校验。
*（内容由AI生成，仅供参考）*

---

## 8. 补记：M1-S4-G（物理 / 导航层 `violations`，2026-11-18）

D1=B/C 落地方式与三处口径：

| 项 | 结论 |
|---|---|
| 入口 | `tx.intent` 的 `impact.violations` 仍是唯一出口，**不新增 RPC**（RPC 57 / 命令表 387 不变） |
| 物理层 | 影子场景 → 临时 `Physics` 世界**当场重建**（`bind_scene` 用运行时同一份 `MeshProvider`），跑装配检查；拿到网格来源时悬空是精确结论（`approximate=false`），拿不到就退回「只出解析穿模 + 逐条记 `unchecked`」 |
| 导航层 | 影子场景当场烘一份影子网格再查可达；烘不出来（无 nav 槽位 / 烘焙失败）如实进 `unchecked`，绝不把「没网格」当「哪都能去」 |
| 网格来源 | `EngineAccess::meshes: Option<Arc<dyn MeshProvider>>` 由 `r3e-runtime` 逐帧递进（与主循环装配同一个「资产优先、图元兜底」口径）；宿主**不自己造 provider**，否则影子装配与主循环装配迟早看两个不同的网格 |
| 求差口径 | 与静态层同一个基线（`tx.begin` 快照），物理 / 导航只在「事务前没有、事务后出现」时报，不会把老问题当新引入 |
| 测试 | `r3e-editor-host` 116 → 119（物理开着 / 导航已烘焙 / 有网格来源出精确结论 / 未烘逐条交代；同批清掉一个临时基准文件里的 1 条测试）；`#[test]` 合计 1,718 → **1,721** |

`cargo xtask verify` 11 步仍全绿（lint-size 白名单 23 项未涨、doc-gate 台账 234 处、invariants A=B=C=38）。

尚未做的一件事留待 M2：`physics.bind` / `scene.rebind` 类 RPC（D2=P3）——它要解决的是「编辑态想先绑后验」，与本卡的「影子世界当场重建」不冲突，属于把临时世界换成持久世界的另一条路。
