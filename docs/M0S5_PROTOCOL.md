---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_888c8ed6e16711f18c5752540024e231
    ReservedCode1: sYigOJdWIRzUy54kgzCV9CTTUY/xp6NLNDbrvwDEnVPHUdtx0ja82FdXZdvjUzIWN09/6LOP/ojlbU+DNAN/5LvotArWMNd0lDLiOZfFlb1ySvs35HnbxIv1HjJIjT/5FZGpfP2cvpNddhVZ/UVDzeZaUzmrmowP7OY4KUrSYLq9EkNuZ6uWU00eax0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_888c8ed6e16711f18c5752540024e231
    ReservedCode2: sYigOJdWIRzUy54kgzCV9CTTUY/xp6NLNDbrvwDEnVPHUdtx0ja82FdXZdvjUzIWN09/6LOP/ojlbU+DNAN/5LvotArWMNd0lDLiOZfFlb1ySvs35HnbxIv1HjJIjT/5FZGpfP2cvpNddhVZ/UVDzeZaUzmrmowP7OY4KUrSYLq9EkNuZ6uWU00eax0=
---

# M0-S5：协议剥离 + 能力清单 + 门禁转硬（方案）

> 实测日期 2026-11-16 · 仓库 `D:\rust3d-engine` · 全部数字由 §1.5 命令现场产出，无一处沿用旧文档估值
> 出方案纪律：先实测、再纠正口径、最后给决定。凡与路线文档冲突处一律在 §1.1 / §5 显式记账。

## 0. 摘要（一屏）

M0 最后一个 Sprint。七张卡里 **5 张可原样执行**，**1 张（S5-04）必须改判**，**1 张（S5-06）需补一个此前不存在的门禁**。

| 判断 | 结论 |
|---|---|
| protocol 剥离是否可行 | ✅ 可行且风险极低。`r3e-editor/src/lib.rs` 2597 行里 **1873 行是纯数据层 + dispatch，零 `r3e_core`/`glam` 引用**（8 处命中全为注释），天然可搬 |
| 依赖违规 9 条边能否全修 | ❌ 不能。`script→physics/audio` 3 边可修（21 处引用点）；`editor-host` 5 边**不该修**——`LocalEngineApi` 结构体天然持有 `Physics`/`ScriptHost`/`Renderer`，规则写错了不是代码写错了；`plugin` 2 边需重写 1405 行 `tools.rs`，属 M1 |
| S5-05「转硬 + 0 违规」 | ⚠️ 改为「白名单外 0 违规 + 债务 2 条（只减不增）」，复用 `size_debt.json` 已验证的机制 |
| Gate 0「>800 行文件 24 → ~12」 | ❌ 该判据在 M0 内不成立：S5 七张卡无一含拆文件任务，剥离后实测只能 **24 → 23**。顺延 M1，本次显式修订 |
| 能力清单真源 | `cmd_spec` 369 条 = **39 条 RPC + 330 条仅脚本**，与 `EngineApi` trait 的 39 个方法**精确相等**，与 `dispatch` 的 37 个 match 臂相容 → 三源交叉可做硬一致性门 |
| 新增门禁 | `gen-capabilities --check`（防清单漂移）+ `invariants`（25 个 issue code 的元测试）|
| 排期 | D1–D7（业余 15–20h/周口径），D7 收 Gate 0 |
| 验收 | `verify` 绿 / `<120s` / `dep-check --strict` 白名单外 0 / 清单无漂移 / 测试 1457 不减 |

**一句话**：协议先搬出去、能力清单从代码长出来、门禁从"报告"转成"只减不增"，然后诚实承认两条依赖债务属于 M1。

---

## 1. 实测基线（先纠正路线文档口径）

### 1.1 路线文档的说法 vs 代码现实

| # | 文档说法 | 实测现实 | 处置 |
|---|---|---|---|
| 1 | `docs/AI_ROADMAP.md` 合并后 698 行 | **697 行**（`splitlines()` 口径） | §11 同频时按实测口径回填 |
| 2 | §2.2 表：`script` 允许 `doc, protocol + trait 端口` | `dep_rules.json` 实为 `allow=[r3e-core, r3e-doc]`；文档允 protocol 禁 core，规则表恰好相反 | §5 统一为 `[core, doc, protocol]` |
| 3 | §2.2 表：`editor-host` 允许 `protocol, doc, runtime` | 实际直连 **6 条边**：core / doc / ~~asset~~ / ~~audio~~ / ~~render~~ / ~~physics~~ / ~~script~~ / runtime | 决定 2：**改规则**，不改成代码 |
| 4 | §2.2 表有 `game` 行 | 无 `game` crate，实名 `r3e-app` | §5 改为 `app` |
| 5 | §2.2 表有 `protocol` 行（仅 serde） | `r3e-protocol` **尚不存在**；`dep_rules.json` 里 `r3e-editor` 反而 allow 了 core | S5-01 创建 + §5 改规则表两条 |
| 6 | §3.1.7 Gate 0：`>800` 行文件 24 → **~12** | S5 七卡零拆文件任务；剥离后 `r3e-editor/src/lib.rs`(2597) 消失，其余 23 条不动 → **24 → 23** | §5 显式修订为 24→≤23，余 11 个顺延 M1 |

> 第 6 条是本次最关键的口径纠正：若照抄原判据，Gate 0 在结构上无法宣告通过。

### 1.2 剥离对象：`crates/r3e-editor/src/lib.rs` 分段实测

| 行范围 | 内容 | 行数 | 归属 |
|---|---|---|---|
| 1–28 | 模块文档 + use（`std` / `serde` / `serde_json` / `log`） | 28 | 待判 |
| 29–34 | `pub mod server;` + `pub use server::{Job, RpcServer};` | 6 | **editor** |
| 37–55 | `JSONRPC` / `PROTOCOL_VERSION` / 11 个错误码常量 | 19 | protocol |
| 61–204 | `Request` / `Response` / `RpcError` / `RpcFailure` | 144 | protocol |
| 206–240 | `EngineInfo` / `PhysicsInfo` / `NodeBrief` | 35 | protocol |
| 242–618 | 14 个 `*Patch` / `*Brief`（Transform…Listener） | 377 | protocol |
| 620–1021 | `trait EngineApi`（**实测 39 个方法**） | 402 | protocol |
| 1023–1918 | `pub fn dispatch(...)`（896 行，**37 个 match 名字臂**） | 896 | protocol |
| 1919–2597 | 4 个内嵌测试 mod（`save_rpc_tests` 4 / `scene_stack_rpc_tests` 7 / `audio_spatial_rpc_tests` 9 / `stats_capture_rpc_tests` 9 = **29 条**） | 679 | protocol |

**纯度实测**：全文搜索 `r3e_core|glam::|Vec3|Quat` 共 8 处命中，**全部位于 doc 注释**（如"本层**不认识** `r3e_core` 的任何类型"）。→ 协议层事实上已经是零引擎依赖，S5-01 是**纯搬迁**，不是重构。

`crates/r3e-editor/src/server.rs` 470 行 / 3 测试 = `Job` + `RpcServer` + `check_token` + TCP/NDJSON 传输，**留在 editor 不动**。

`Cargo.toml` 实测：`dependencies` 仅 `serde / serde_json / log`（已零引擎依赖）；`dev-dependencies` 仅 `r3e-core`（供 `tests/cmd_spec_contract.rs` 把 `known_methods()` 与命令表对齐）。

### 1.3 依赖违规 9 条边的真实成本（逐边定级）

| 违规 crate | 边 | 代码引用点实测 | 成本 | 结论 |
|---|---|---|---|---|
| `r3e-script` | → `r3e-physics` + → `r3e-audio`（两条件合计） | `lib.rs` 16 行 + `api/mod.rs` 4 行 + `api/audio.rs` 1 行 = **21 行**（按行计，未按符号细分） | 中：`CapabilityHost` 端口已存在，需补物理/音频入口 | **S5-04 内修掉** |
| `r3e-editor-host` | → `asset` / `audio` / `render` / `physics` / `script` | `api/mod.rs`:19,36,37,38（`use r3e_audio::{Audio,…}` / `r3e_physics::Physics` / `r3e_render::Renderer` / `r3e_script::ScriptHost`）+ `info.rs`:93 | **极高**：`LocalEngineApi` 字段直接是 `&Rc<RefCell<Physics>>`、`&mut ScriptHost`、`Option<&mut Renderer>`；要满足规则必须把 4000 行 `api/*` 整体下沉 `r3e-runtime` | **改规则**（决定 2） |
| `r3e-plugin` | → `r3e-core` | `locate.rs`:18,19,20 + `tools.rs`:11,12,13,241–244,1000,1012,1031 | 高：需把场景解析改成协议 DTO，重写 1405 行 `tools.rs` | **登记债务 → M1** |
| `r3e-plugin` | → `r3e-physics` | `Cargo.toml` `rapier = ["r3e-physics/rapier"]`（独立 bin 的可选后端） | 高：feature 直通，需拆出独立后端 crate | **登记债务 → M1** |

### 1.4 门禁与测试现状

| 门禁 | 现状 | S5 后 |
|---|---|---|
| `fmt` / `clippy` / `test` | 绿；`test` 1 457 单测 / 0 failed | 不变 |
| `lint-size` | 扫 246 个 `.rs`，阈值 800，白名单 24 项全未上涨 | 白名单 **24 → 23** |
| `check-tests` | 13 crate 全部持平基线 | `editor 101→72`、新增 `protocol 29`，**total 1457** |
| `doc-gate` | 6 条规则 / 白名单 10 项 ok | 不变 |
| `dep-check` | **v0 报告态**，违规 3 条（9 条边），CI `continue-on-error: true` | `--strict` 硬门禁，白名单外 0 / 债务 2 |
| `replay` | 600 帧 / 10 采样 / 两次哈希一致 `final=59d48dedf2b0677e` | 不变 |
| `gen-capabilities` | 存量实现 37 行：扫 `r3e-script` 的 `.register_fn("` 计数并打印 `TODO(M1)`，**不读 `cmd_spec`、不产 JSON** | 重写为三源交叉生成器 |
| `invariants` | **不存在** | 新建（元测试门） |
| `verify` 总耗时 | 热缓存 **14.7s** | 预估 18–22s（预算 120s） |

各 crate 规模与测试（实测）：

| crate | 行数 | 最大单文件 | 测试 |
|---|---|---|---|
| r3e-core | 31 236 | 4 057 | 576 |
| r3e-render | 13 074 | 1 112 | 307 |
| r3e-script | 6 230 | 2 018 | 132 |
| r3e-asset | 5 979 | 2 010 | 61 |
| r3e-runtime | 4 338 | 3 060 | 14 |
| r3e-editor-host | 3 979 | — | 58 |
| r3e-plugin | 3 703 | 1 405 | 47 |
| r3e-editor | 3 067 | 2 597 | 101 |
| r3e-physics | 2 548 | 1 929 | 28 |
| r3e-doc | 2 454 | — | 82 |
| r3e-audio | 2 335 | 1 375 | 29 |
| r3e-replay | 546 | — | 11 |
| r3e-app | 36 | — | 11 |
| xtask | 875 | 562 | 1 |

`r3e-editor` 101 测试拆解：内嵌 lib 29 + server 3 + 集成 `tests/*.rs` 69（9 文件 1 856 行，`cmd_spec_contract.rs` 4 条依赖 `r3e-core`）。

### 1.5 本次实测所用命令（可复跑）

```powershell
# 行数（注意：Measure-Object -Line 与 splitlines() 差 1，本方案统一用后者）
python -c "print(len(open('docs/AI_ROADMAP.md',encoding='utf-8').read().splitlines()))"

# 命令表三源
python -c "import re,glob;t=[open(f,encoding='utf-8').read() for f in glob.glob('crates/r3e-core/src/cmd_spec/*.rs') if 'mod.rs' not in f];print('rpc',sum(len(re.findall(r'rpc:\s*Some\(',x)) for x in t),'script_only',sum(len(re.findall(r'rpc:\s*None',x)) for x in t))"

# EngineApi 方法数 / dispatch 臂数
python -c "import re;L=open('crates/r3e-editor/src/lib.rs',encoding='utf-8').read().splitlines();print('methods',len([l for l in L[619:1022] if re.match(r'^\s+fn [a-z_]+',l)]));print('arms',len([l for l in L[1022:1918] if re.search(r'^\s+\"[a-z]+\.[a-z.]+\"\s*=>',l)]))"

# 协议层纯度（应为 0 处代码引用）
cargo tree -p r3e-editor --edges normal      # 现：serde / serde_json / log
cargo xtask dep-check --strict               # 现：3 条
cargo xtask verify                           # 热缓存 14.7s
```

---

## 2. 核心设计：五个决定

### 决定 1：protocol 只搬「数据层 + dispatch + 能力 DTO」，传输留 editor，靠 re-export 保零改动

```
r3e-protocol  (dependencies: serde, serde_json)   ← 零引擎依赖，连 dev-dep 也零
├── lib.rs        骨架 + re-export（~60 行）
├── msg.rs        Request / Response / RpcError / RpcFailure / EngineInfo / PhysicsInfo / NodeBrief（~210）
├── patch.rs      14 个 *Patch（~380）
├── api.rs        trait EngineApi（39 方法，~410）
├── dispatch/
│   ├── mod.rs    骨架 + 参数解码 + 错误码映射 + engine.* 元命令（~260）
│   ├── scene.rs  scene.* / anim.* 臂（~340）
│   └── media.rs  ui.* / text.* / light.* / audio.* / shadow.* / postfx.* 臂（~300）
└── caps.rs       Capability DTO + `to_json()`（~150）→ 供 xtask 与 ai_ide 共用
```

`r3e-editor` 保留 `server.rs`（470 行传输）并**整体 re-export**：

```rust
pub use r3e_protocol::*;          // 常量 / 消息 / Patch / EngineApi / dispatch
pub mod server;
pub use server::{Job, RpcServer};
```

**为什么 re-export 是关键**：全仓 `use r3e_editor::{TransformPatch, EngineApi, …}` 的调用方共 2 处外部（`r3e-editor-host/src/api/*`、`r3e-editor/tests/*.rs` 69 条），re-export 让它们**一行不改**。剥离风险从"改 10 个文件"降到"改 2 个 Cargo.toml"。

`dispatch` 拆 3 文件的硬约束：单文件 ≤800 行、臂总数不变（37）、`--check` 行为等价（29 条内嵌测试全绿）。896 行按 rpc 前缀切分，落地时以实测行数为准。

**不搬的东西**：`server` / `Job` / `RpcServer` / `check_token` / TCP+NDJSON / `PROTOCOL_VERSION` 保持 1（本次不改协议版本）。

### 决定 2：S5-04 从「修 3 条」改为「修 1 + 改规则 1 + 记账 1」

原卡验收是"`dep-check` 后 0 违规"。实测三条违规的性质完全不同：

| 违规 | 性质 | 处置 |
|---|---|---|
| `script → physics/audio` | **代码没守规矩**，3 条边、21 处引用，`CapabilityHost` 端口已存在 | 修代码 |
| `editor-host → 5 条边` | **规矩没描准现实**。`r3e-editor-host` 的职责就是"把 RPC 落到主线程持有的 Scene/Physics/ScriptHost/Renderer 上"（`api/mod.rs` 头注释原话）。它必须是适配器，适配器必须认识被适配的东西 | 改 `dep_rules.json` + §2.2 表：`editor-host allow = [core, doc, protocol, runtime, render, physics, audio, script, asset]`，附 note「**叶子适配器**：可依赖下层全部；禁止被任何下层反向依赖；对 protocol 只认 `EngineApi` trait，不认识 JSON 细节」 |
| `plugin → core/physics` | **历史路径依赖**。plugin 是独立 bin，`tools.rs` 1405 行直接用 `Scene`/`BodyDesc` 做场景定位 | `dep_debt.json` 记账 2 条，`plan: M1-S1` |

改规则的正当性依据：真正该守的架构不变量是「**下层不得反向依赖上层**」与「**协议层不得认识引擎类型**」——这两条由 `forbid_engine_all` + protocol 零依赖保证。原规则把"适配器不许认识子系统"写进去，等于要求适配器是空壳，属规则笔误。本次修订**不放宽任何反向依赖**，只纠正适配器层的允许集。

副作用：M1 少一张"host 收窄"的卡，`editor-host` 从"6 条技术债"变为"合法设计"。这是诚实的减法。

### 决定 3：dep-check 转硬用 debt 白名单 + 只减不增（复用 size_debt 机制）

```json
// xtask/dep_debt.json（新建）
{
  "_comment": "依赖债务白名单。只许减不许增：条目数上涨即门禁失败；边已消失却未删条目也失败。思考过程写在 reason 里，偿还计划写 plan。",
  "edges": [
    { "from": "r3e-plugin", "to": "r3e-core",
      "reason": "tools.rs/locate.rs 直接用 Scene 类型做场景定位与构造", "plan": "M1-S1 改协议 DTO" },
    { "from": "r3e-plugin", "to": "r3e-physics",
      "reason": "独立 bin 的 rapier 可选后端 feature 直通", "plan": "M1-S1 拆独立后端 crate" }
  ]
}
```

`dep-check --strict` 四条判定：

1. 白名单外任何违规 → exit 1（这就是 S5-05 要的"故意加违规依赖 → CI 红"）
2. `edges` 条目数 > 2 → exit 1（只减不增）
3. 某条 edge 在依赖图里已消失却没从 debt 删除 → exit 1（防僵化，与 `size_debt` 同款）
4. 违规 crate 未登记 `dep_rules.json` → exit 1（沿用现有规则 5 语义）

输出契约：`[dep-check] 违规 0 条；债务 2 条（基线 2）`。

**为什么不直接要求 0 违规**：`plugin` 的两条边要靠重写 1405 行 `tools.rs` 才能消，会把 S5 从 1 周拉成 2 周且破坏 ai_ide 现有集成。用 debt 表把它变成**可见、可数、可压**的债务，比"转硬失败所以继续报告态"严格得多——现状 `continue-on-error: true` 是零压力。

### 决定 4：capabilities.json 由三源交叉生成，`--check` 防漂移

生成器 `xtask/src/gen_capabilities.rs`（重写存量 37 行骨架），输入三源：

| 源 | 提供 | 实测 |
|---|---|---|
| `r3e_core::cmd_spec::all()` | `commands[]`（name / domain / rpc / params / returns / reversible / cost / since） | **369 条**（`rpc=Some` **39** + `rpc=None` **330**） |
| `r3e_doc::ISSUE_CODES`（**新增常量表**） | `invariants[]`（code / layer / severity） | **25 条**（`validate/bounds.rs` 19 + `validate/refs.rs` 6） |
| `r3e_protocol::caps` | schema / 序列化 / 排序规则 | 新写 |

`xtask` 需加 `r3e-core` + `r3e-doc` 依赖——**已核实安全**：`dep_check()` 用 `crate_dirs()` 只遍历 `crates/` 下目录（`main.rs:411`），`xtask` 不在其中，故不产生依赖违规；`dep_rules.json` 里的 `"xtask"` 条目仅供 `doc-gate` 规则 5 登记用。

**硬断言（S5-03 的实质）**——生成时即校验，任一不成立直接非 0：
- `commands.len() == cmd_spec::len() == 369`
- `commands 中 rpc 非空数 == 39`
- `39 == EngineApi trait 方法数`（由 `r3e-protocol` 导出 `const RPC_METHODS: &[&str]`，与 `dispatch` 臂数 37 相容：2 条走别名/默认分支）
- `invariants.len() == 25` 且每个 code 在 `r3e-doc` 测试源码中被断言过

`--check` 模式：重新生成到内存，与仓库内 `docs/capabilities.json` 逐字节比对（键排序 + 2 空格缩进，保证 `git diff` 可读可审）。不一致 → exit 1 并打印首个差异键路径。

### 决定 5：invariants 门是元测试，不新写场景测试

`cargo xtask invariants` 做三源集合比对，不跑引擎：

```
A = r3e_doc::ISSUE_CODES 的 code 集合（25）
B = r3e-doc 测试源码里 `"xxx.yyy"` 形式的 code 断言集合
C = docs/capabilities.json 的 invariants[].code 集合
判据：A == B == C，任一差集非空 → exit 1，打印 `missing_in_tests` / `missing_in_caps` / `stale_in_tests`
```

价值：拦住两类真实漂移——「新增不变量但没写反例测试」（B 缺 A）与「删了 code 但清单没更新」（C 多 A）。零新增场景测试成本，故不动测试基线。

---

## 3. capabilities.json 规格

落盘位置 `docs/capabilities.json`（与 `AI_ROADMAP.md` 同级，属对外能力声明，进版本库）。

```json
{
  "schema": 1,
  "generated_by": "cargo xtask gen-capabilities",
  "engine": "r3e",
  "protocol_version": 1,
  "counts": { "commands": 369, "rpc": 39, "script_only": 330, "domains": 17, "invariants": 25 },
  "domains": ["scene", "scene_flow", "anim", "anim_blend", "anim_sync", "env", "render",
              "shadow", "ui", "text", "light", "audio", "physics", "input", "query", "pick", "meta"],
  "commands": [
    { "name": "scene.spawn", "domain": "scene", "rpc": "scene.spawn",
      "params": ["node"], "returns": "bool", "reversible": true, "cost": "cheap", "since": "M0-backfill" },
    { "name": "anim.blend.push", "domain": "anim_blend", "rpc": null,
      "params": ["layer", "clip"], "returns": null, "reversible": true, "cost": "cheap", "since": "M0-backfill" }
  ],
  "invariants": [
    { "code": "node.name.empty", "layer": "bounds", "severity": "error" },
    { "code": "ref.node_missing", "layer": "refs", "severity": "error" }
  ],
  "unsupported": [
    { "item": "physics.backend.rapier", "reason": "feature 未开启时退回内置积分器", "since": "M1" }
  ]
}
```

稳定性约定（写进 `caps.rs` doc）：
- `commands` 按 `name` 字典序、`invariants` 按 `code`、`domains` 按声明序（`Domain` 枚举序）
- 字段顺序固定，`rpc: null` 表示**仅脚本可用**（330 条），不省略键
- `since` 存量统一 `M0-backfill`（`cmd_spec::SINCE_V0`），新命令必须写真实 Sprint 号
- 任何字段增删都要 `schema++`，且 `--check` 与消费方（ai_ide）同步

---

## 4. 目标文件树与行数预算

```
crates/r3e-protocol/                    ← 新建（workspace members += 1，13 → 14）
  Cargo.toml        deps: serde, serde_json                                   ~12
  src/lib.rs        re-export + RPC_METHODS 常量                               ~60
  src/msg.rs        Request/Response/RpcError/RpcFailure/EngineInfo/…         ~210
  src/patch.rs      14 个 *Patch / *Brief                                     ~380
  src/api.rs        trait EngineApi（39 方法）                                 ~410
  src/dispatch/mod.rs   骨架 + engine.* 元命令                                 ~260
  src/dispatch/scene.rs scene.* / anim.*                                      ~340
  src/dispatch/media.rs ui.* / text.* / light.* / audio.* / shadow.* / postfx.* ~300
  src/caps.rs       Capability DTO + to_json()                                ~150
  tests/rpc_contract.rs   ← 29 条内嵌测试迁出（含 mock EngineApi）             ~700

crates/r3e-editor/                      ← 瘦身：3067 → 约 520 行
  src/lib.rs        骨架 + `pub use r3e_protocol::*;`                          ~40
  src/server.rs     Job / RpcServer / check_token / TCP+NDJSON（原样）          470
  tests/*.rs        9 文件 1856 行（**零改动**，靠 re-export 继续编译）        1856

xtask/
  src/gen_capabilities.rs  三源交叉生成器（重写 37 行骨架）                    ~280
  src/invariants.rs        元测试门                                            ~120
  src/main.rs              子命令 += invariants；dep-check 接 debt             562 → ~640
  dep_debt.json            新建（2 条）                                         ~20
  dep_rules.json           editor-host allow 放开 + editor→protocol + script 补 protocol

docs/
  capabilities.json        新建（由代码生成，入版本库）                        ~ 2 800（369 命令 × 7 行）
  M0S5_PROTOCOL.md         本文档
  AI_WORKFLOW.md           新建（S5-07）
```

**行数风险**：`capabilities.json` 约 2 800 行，但 `lint-size` 只扫 `*.rs`（实测"扫描 246 个 `.rs`"），不受 800 阈值约束。

**测试迁移记账（必须同批）**：

| crate | 现在 | S5 后 |
|---|---|---|
| `r3e-editor` | 101 | **72**（-29 内嵌） |
| `r3e-protocol` | — | **29** |
| 全仓 total | 1 457 | **1 457**（只搬不删） |

---

## 5. 门禁与规则变更记账（必须显式）

| 变更 | 文件 | 内容 |
|---|---|---|
| 1 | `xtask/dep_rules.json` | `r3e-editor-host.allow` → `[core, doc, protocol, runtime, render, physics, audio, script, asset]` + note「叶子适配器」；`r3e-script.allow` 补 `r3e-protocol`；新增 `r3e-protocol: allow=[]`；`r3e-editor.allow` → `[r3e-protocol]` |
| 2 | `docs/AI_ROADMAP.md` §2.2 | `game` → `app`；`editor-host` 行按上表重写；`script` 行统一为 `core, doc, protocol`；删除 `editor` 无行状态（明确"旧壳，M1 删除"） |
| 3 | `docs/AI_ROADMAP.md` §3.1.7 Gate 0 | 删除「`>800` 行文件 24 → ~12」，改为「24 → ≤23，余 11 个顺延 `M1-DOC-FIELDS`」；S5-04 / S5-05 验收按 §2 决定 2/3 重写 |
| 4 | `xtask/dep_rules.json` 头注释 | "v0 只报告不阻塞，M0-S5 后转硬" → "v0 报告态已于 M0-S5 转硬；债务见 dep_debt.json" |
| 5 | `.github/workflows/ci.yml` gate job | 第 59 行撤 `continue-on-error: true`；`cargo xtask dep-check` → `dep-check --strict`；新增 `cargo xtask invariants`、`cargo xtask gen-capabilities --check` |
| 6 | `xtask/tests_baseline.json` | `r3e-editor: 101→72`，新增 `r3e-protocol: 29`，`total` 保持 `1457` |
| 7 | `xtask/size_debt.json` | 删 `crates/r3e-editor/src/lib.rs: 2597`（文件将缩至 ~40 行）；**不新增任何条目** |
| 8 | `xtask/main.rs` 用法文本 | 子命令说明 += `invariants`；`dep-check` 说明改为"默认严格，债务见 dep_debt.json" |
| 9 | `xtask/Cargo.toml` | `dependencies` += `r3e-core`、`r3e-doc`（xtask 不在 `crates/`，`dep-check` 不覆盖，已核实） |
| 10 | `Cargo.toml`（workspace） | `members` += `crates/r3e-protocol` |

---

## 6. 工作包与分批

| 天 | 工作包 | 卡 | 交付判据 |
|---|---|---|---|
| D1 | **纯搬迁**：建 `r3e-protocol`，按 §1.2 分段切文件，dispatch 拆 3 文件，29 测试迁 `tests/rpc_contract.rs`；`r3e-editor` 留 `server.rs` + re-export；同步 workspace members、两个 Cargo.toml、`tests_baseline`、`size_debt` | S5-01 | `cargo tree -p r3e-protocol --edges normal,dev` 仅 serde/serde_json；`cargo test --workspace` 1 457 绿；`use r3e_editor::*` 的 69 条集成测试零改动通过 |
| D2 | **能力生成器**：`r3e-doc` 加 `ISSUE_CODES`（25 条）；`r3e-protocol::caps` 定义 DTO；`xtask/gen_capabilities.rs` 重写（读 `cmd_spec::all()` + `ISSUE_CODES` + `RPC_METHODS`）；`xtask` 加依赖 | S5-02 | `cargo xtask gen-capabilities` 产出 `docs/capabilities.json`，`counts` = 369/39/330/17/25，重跑字节一致 |
| D3 | **一致性门 + 演练**：`--check` 模式；临时注释 `cmd_spec` 里 `scene.spawn` 一条 → `--check` 必须红；恢复后绿 | S5-03 | 演练双向：删表项红 / 删实现（`dispatch` 臂）红；`git diff` 可审 |
| D4 | **script 端口化**：`CapabilityHost` 补物理/音频入口，替换 `r3e-script` 21 处引用，删 `Cargo.toml` 两条依赖 | S5-04(1/3) | `cargo tree -p r3e-script` 无 physics/audio；132 测试不减 |
| D5 | **dep 债务 + 转硬**：建 `dep_debt.json`（2 条）；`--strict` 四判定；改 `dep_rules.json`（host/script/editor/protocol）；CI 撤 `continue-on-error`；演练"临时给 `r3e-doc` 加 `r3e-render` 依赖 → CI 红" | S5-04(2/3,3/3) + S5-05 | `dep-check --strict` → `违规 0 条；债务 2 条`；故障注入演练红 |
| D6 | **invariants + verify 收敛**：`xtask/invariants.rs` 三源比对；`verify` 序列加 3 项（dep-check --strict / invariants / gen-capabilities --check）；计时 | S5-06 | `cargo xtask verify` 绿且 `<120s`，打印全段耗时 |
| D7 | **文档同频 + Gate 0**：写 `AI_WORKFLOW.md`；`AI_ROADMAP` 看板/§2.2/§3.1.7/附录回填；`ARCHITECTURE.md` 加 protocol 层职责与 dispatch 分层；本文档 §13 回填 | S5-07 | 三个可复制命令块；全仓 `grep r3e_editor::` 无悬空引用；最后一笔 docs 提交 |

关键提示：D1 是**纯搬迁**，必须先绿再动别的；一旦 D1 出现行为差异，立即 `git checkout` 回滚，不带着差异往下走。

---

## 7. 风险与熔断

| 编号 | 风险 | 概率 | 熔断条件与回退 |
|---|---|---|---|
| R1 | dispatch 896 行拆 3 文件时臂顺序/默认分支错乱，导致某命令静默失效 | 中 | 判据：29 条迁移测试 + 69 条集成测试全绿才算过；任一红 → 退回单文件 `dispatch.rs`（接受 896 行进 `size_debt`，登记理由"M0 收尾稳定性优先"） |
| R2 | `r3e-protocol` 的 dev-dependency 被拉进 `r3e-core`（想复用测试夹具） | 低 | 硬规矩：protocol 的 `tests/` 只许用内部 mock，`cargo tree --edges dev` 出现任何 `r3e-*` 即失败 |
| R3 | `script` 端口化后 132 测试中 3–5 条物理/音频行为测试失效 | 中 | 改测试夹具走 `CapabilityHost` mock，**不许删测试**（`check-tests` 会红）；超 8 条失效则 S5-04 只做 physics 一边，audio 边入 debt |
| R4 | Gate 0 的"24 → ~12"做不到 | **必然** | 已在 §1.1/§5 显式修订为 24 → ≤23，余 11 个顺延 M1；**不因判据修订而放松其余门禁** |
| R5 | `capabilities.json` 2 800 行每次生成产生噪声 diff | 中 | 键排序 + 固定字段序 + 2 空格缩进；若仍噪声大则改生成"每行一条命令"的紧凑数组 |
| R6 | `editor-host` 规则放开被后续当作"什么都能依赖"的口实 | 中 | 规则 note 写明三条不变式，且 `forbid_engine_all` 不动；反向依赖仍是一条都红 |
| R7 | 业余时间：单人 15–20h/周，7 个工作包偏紧 | 中 | 允许砍序：D3 演练 → D6 invariants → D7 文档 为可延后项；S5-01/02/05 是不可延后核心 |
| R8 | 文档再次漂移（本次已发现 6 处口径不一致） | 高 | 本文档所有数字标 §1.5 命令；`capabilities.json` 与看板数字全部由 xtask 产出，禁止手抄 |

---

## 8. 不做（明确排除）

| 项 | 理由 |
|---|---|
| 重写 `r3e-plugin/src/tools.rs`（1405 行）只依赖 protocol | M1-S1；本次入 debt 表 |
| `editor-host` 的 4 000 行 `api/*` 下沉 `r3e-runtime` | 架构重排，与 M0"零行为变化"冲突；改为修规则 |
| `r3e-editor/src/server.rs` 传输层改动（TCP / NDJSON / 鉴权） | 本次只做协议数据层剥离 |
| `PROTOCOL_VERSION` 升到 2 | 无协议语义变更 |
| 拆 `scene.rs`(4 057) 等 11 个超标文件 | M1-`DOC-FIELDS`；见 R4 |
| `capabilities.json` 的 `unsupported` 全量自动化 | v0 只登记 rapier 后端一条，其余靠 `since` + `rpc: null` 表达 |
| 多客户端 / gateway / 协议压缩 | M1 及以后 |
| 推送远端 | 全程本地提交（纪律：未指令不推） |

---

## 9. 验收清单（全部可自动判定）

```powershell
# 1. protocol 纯依赖（含 dev）——输出只应出现 serde / serde_json
cargo tree -p r3e-protocol --edges normal,dev

# 2. 全仓测试 1 457 不减、0 failed
cargo test --workspace

# 3. 单文件行数：白名单外 0；债务 23 条（=24-1）
cargo xtask lint-size

# 4. 依赖：白名单外 0 违规，债务 2 条（只减不增）
cargo xtask dep-check --strict

# 5. 能力清单无漂移
cargo xtask gen-capabilities --check

# 6. 不变量三源一致（25 == 25 == 25）
cargo xtask invariants

# 7. 总门禁 <120s 且含回放哈希 final=59d48dedf2b0677e
cargo xtask verify
```

| # | 硬判据 | 目标值 |
|---|---|---|
| A1 | `r3e-protocol` 引擎依赖 | **0**（normal + dev 均 0） |
| A2 | 全仓单测 | **1 457 / 0 failed** |
| A3 | `lint-size` 白名单外 | **0 文件**；债务 24 → 23 |
| A4 | `dep-check --strict` | 白名单外 **0**；债务 **2**（plugin×2） |
| A5 | `capabilities.json` | 369 命令 / 39 rpc / 330 脚本独有 / 17 域 / 25 不变量，重跑字节一致 |
| A6 | 一致性门演练 | 删 `cmd_spec` 表项 → 红；删 `dispatch` 臂 → 红；恢复 → 绿 |
| A7 | dep 门演练 | 临时加违规依赖 → CI 红；移除 → 绿 |
| A8 | `verify` 耗时 | **< 120s**（当前热 14.7s，预估 18–22s） |
| A9 | 回放哈希 | `final=59d48dedf2b0677e` 不变 |
| A10 | 集成测试零改动 | `crates/r3e-editor/tests/*.rs` 9 文件 1 856 行 `git diff` 为空 |

---

## 10. 提交规划（本地提交，**不推送**）

| # | commit | 覆盖 |
|---|---|---|
| 1 | `refactor(protocol): 抽出 r3e-protocol，editor 留传输层` | 新建 crate + dispatch 三分 + 29 测试迁移 + 两个 Cargo.toml + workspace members + tests_baseline + size_debt 删条 |
| 2 | `feat(doc): ISSUE_CODES 常量表（25 条不变量）` | `r3e-doc` 导出 + 测试改为引用常量 |
| 3 | `feat(xtask): gen-capabilities 三源交叉生成 capabilities.json` | 生成器 + `r3e-protocol::caps` + `docs/capabilities.json` + xtask 依赖 |
| 4 | `feat(xtask): capabilities --check 一致性门` | `--check` + 沿 `assert` 硬判据 |
| 5 | `refactor(script): CapabilityHost 端口化，去除 physics/audio 直连` | `r3e-script` 21 处引用替换 + Cargo.toml 两条删除 |
| 6 | `feat(xtask): dep-check --strict + dep_debt 只减不增` | `dep_debt.json` + 四判定 + `dep_rules.json` 修订 |
| 7 | `ci: gate 撤 continue-on-error，接 strict/invariants/caps-check` | `ci.yml` + `xtask/main.rs` 用法文本 |
| 8 | `feat(xtask): invariants 三源比对门` | `invariants.rs` + verify 序列 |
| 9 | `docs: M0S5 收尾——AI_WORKFLOW + 路线图看板/规则/附录同频` | `AI_WORKFLOW.md` + `AI_ROADMAP.md` + `ARCHITECTURE.md` + 本文档 §13 |

纪律：day 内小步提交；9 笔可合并但**顺序不可乱**（1 在 3 前，5 在 6 前，6 在 7 前）。

---

## 11. 文档同频清单（D7 一并刷新，以当时实测为准）

| 文件 | 回填点 |
|---|---|
| `docs/AI_ROADMAP.md` | 看板六项数字（最大文件 4 057 不变 / `>800` 23 / 测试 1 457 / 依赖违规 0 / verify 耗时实测 / 回放哈希）；§2.2 依赖规则表按 §5 改；§3.1.7 S5-04/05 验收重写 + Gate 0 判据修订；§八 kanban 把 M0 标收口；附录说明本次口径纠正 6 处 |
| `docs/ARCHITECTURE.md` | 新增「协议层」职责段：`r3e-protocol` 只认 JSON + `EngineApi`，`r3e-editor` 只做传输；dispatch 三分文件表；能力清单真源三源交叉图 |
| `docs/AI_WORKFLOW.md`（新建） | 三个可复制命令块：①日常循环（改码 → `cargo xtask verify`）②提交前（`dep-check --strict` + `gen-capabilities --check`）③门禁红了怎么查（lint-size / check-tests / dep-check / invariants 四个典型） |
| `docs/M0S5_PROTOCOL.md` | §13 收尾实测回填（预算 vs 实际、门禁结果、提交记录、与方案的偏差） |

---

## 12. S5 之后（M1 预留）

| 项 | 内容 |
|---|---|
| M1-S1 | `r3e-plugin` 去 `core`/`physics`：场景访问改协议 DTO，rapier 拆独立后端 crate → `dep_debt` 归零 |
| M1-S1 | `r3e-editor` 旧壳删除（全仓 `use r3e_editor::` 迁 `r3e_protocol::`），`dep_rules` 删 editor 行 |
| M1-DOC-FIELDS | `scene.rs`(4 057) 等 11 个超标文件拆分，`>800` 23 → ≤12（原 Gate 0 目标落到这里） |
| M1 | `capabilities.json` 的 `since` 从 `M0-backfill` 改为真实 Sprint；`unsupported` 自动化（从 `EngineApi` 的 `RpcFailure::unsupported` 分支提取） |
| M1+ | ai_ide 消费 `capabilities.json` 做插件能力发现，替代硬编码方法表 |

---

## 13. 收尾实测（落地后回填）

> 待 S5-07 收尾时按实测填写，禁止预估。

### 13.1 预算 vs 实际

| 项 | 预算 | 实际 |
|---|---|---|
| `r3e-protocol` 文件数 / 最大文件行数 | 9 / ≤800 | 待填 |
| `r3e-editor` 剥离后行数 | ~520 | 待填 |
| `capabilities.json` 行数 | ~2 800 | 待填 |
| `verify` 耗时 | 18–22s | 待填 |

### 13.2 门禁结果

| 门禁 | 结果 |
|---|---|
| `cargo test --workspace` | 待填 |
| `lint-size`（债务数） | 待填 |
| `dep-check --strict`（违规 / 债务） | 待填 |
| `gen-capabilities --check` | 待填 |
| `invariants`（25/25/25） | 待填 |
| 回放哈希 | 待填 |

### 13.3 提交记录（本地，未推送）

待填。

### 13.4 与方案的偏差

待填（预期偏差点：dispatch 三分行数、script 端口化测试数、host 规则修订措辞）。
*（内容由AI生成，仅供参考）*
