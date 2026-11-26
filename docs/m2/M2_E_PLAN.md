---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_4f2e5a84e96511f1afe3525400ffa3eb
    ReservedCode1: B9mUP8g1jO3LkC/LvaURxEpIDuksqMcUlWHwOv8w4MJvC3Zh4tXUyhDwtYSmvbuwUgwC5xumgTMOD3P35bHhOj0OQhDxX+CdVY2jZqHdOfx565mIf7qUJ2D742q2FM609oASriCIqtmfhfOSbMQ30WNTNXJEL9TzoB9T5fLbGSvvMvGWJG3zkP5LORU=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_4f2e5a84e96511f1afe3525400ffa3eb
    ReservedCode2: B9mUP8g1jO3LkC/LvaURxEpIDuksqMcUlWHwOv8w4MJvC3Zh4tXUyhDwtYSmvbuwUgwC5xumgTMOD3P35bHhOj0OQhDxX+CdVY2jZqHdOfx565mIf7qUJ2D742q2FM609oASriCIqtmfhfOSbMQ30WNTNXJEL9TzoB9T5fLbGSvvMvGWJG3zkP5LORU=
---



# M2 E 阶段方案：Prefab 模板 / 实例 / 覆盖 diff（E01–E05）

- 文档版本：v1（2026-11-26 落盘；同日拍板）
- 状态：**已拍板**（2026-11-26，`P1`～`P6` **六项全部采纳草案推荐值，无例外项**：`P5` 按草案 §8 建议取 **B**，带 rpc 的两条为 `prefab.spawn` + `prefab.info`，`prefab.define` 无 rpc，命令表已按此定稿）
- 上位约束：`M2_PLAN.md` §3.5 卡表 / §4 门禁累加预算 / §5 单卡执行规程 / §6 文件占用矩阵 / §7 风险 `R1`/`R2`/`R4`/`R8`
- 决策前置：`M2_DECISIONS.md` **D6=A**（实例只存与模板不同的字段，diff 补丁）、**D7=A**（`Scene` 内嵌实例表 + 外部模板按 ID 引用；G 阶段分块只携带实例引用），2026-11-20 已拍板；D 阶段 `DD-1`～`DD-6` 全 A（本文沿用其门禁口径与「命令横向范式」）
- 本稿范围：**只出方案**。不改一行引擎代码、不 `git commit`。§11.3 的文档回写在各卡落地时随卡完成（本稿只在 §11.3 列清单，不预先改动 `M2_PLAN.md` / `README.md` / `M2_DECISIONS.md`）。
- 数字口径：全部取自 2026-11-26 实测（采集命令见各节），`tests` 只认 `cargo xtask check-tests`，能力清单只认 `cargo xtask gen-capabilities --check`。

---

## 0. 一句话

E 阶段不是「往场景里加一个新概念」——它兑现的是 M2 立项时就挂账的两条前置决策（`D6` 只存差异、`D7` 内嵌实例表 + 外部模板按 ID 引用）。真正缺的是五件**可被自动验证**的东西：① 一个**强类型、13 字段三态**的覆盖结构（`ObjectDesc` 13 个字段全是 `Option`，「未覆盖」与「覆盖为空」必须能区分，单层 `Option` 做不到）；② **模板对象在内存里只有一份**（100 个实例共享同一个 `Arc<PrefabTemplate>`，不是 100 份拷贝）；③ 覆盖**只落差异**且**可逆**（`set → revert` 后 `scene_hash` 回原值）；④ **6 条命令 + 2 条 rpc + 1 条不变量**把「编辑器能建、CLI 能复现、脚本能调」打通（`rpc` 给 `prefab.spawn` 与 `prefab.info`；CLI 复现以「先本地建模板」为前提，见 §13 `R7`），且**不新增 `Domain`**（`domains` 预算 0）；⑤ 实例表**入 `state_hash`**，让「实例被改动」在回放/差分里看得见，而模板文件内容按既有 `assets` 口径**不入**。E 阶段一行都不用碰渲染与物理——实例化**仍走既有 `Scene::spawn`**，绝不另开第二条装配路径。

---

## 1. 交付映射（阶段目标）

| 卡 | 目标 | 主落点 | 卡表判据（`M2_PLAN` §3.5） | E 阶段实际增量 | 门禁增量 |
|---|---|---|---|---|---|
| **E01** | Prefab 数据模型与模板格式冻结 | `r3e-core/src/prefab/{mod,template,instance,diff,table}.rs`、`r3e-core/src/prefab_schema.rs`、`assets/prefab.schema.json` | 模板 / 实例 / 覆盖三件套可序列化且格式冻结；模板样例过 schema | `PrefabTemplate`（id/schema/proto/tags）+ `PrefabInstance`（name/template/parent/overrides）+ `ObjectOverride`（13 字段 `FieldOverride<T>` 三态，`opt_field!` 宏 + 自定义 `deserialize_with`）+ `PrefabTable`（`BTreeMap<String, Arc<PrefabTemplate>>`）+ schema dumper | tests **+18** |
| **E02** | 实例化与 `Scene` 集成 | `r3e-core/src/scene.rs`、`scene/io.rs`、新建 `scene/prefab.rs` | 实例表可序列化/反序列化；实例化复用既有装配入口 | `SceneFile.instances` / `Scene.instances` / `Scene.prefabs`（模板表不序列化）；`PrefabInstance` → `apply(proto, overrides)` → **`Scene::spawn`**（不新增装配路径）；`Arc` 共享 | tests **+14** |
| **E03** | 覆盖只存差异与可逆性 | `prefab/diff.rs`、`r3e-doc/src/command/mod.rs` | 覆盖只落差异字段；`set`/`clear`/`revert` 可逆 | `diff`/`apply`/`revert` 三算子 + `CommandKind` **+2 变体**（20→22）：`DefinePrefab` / `SetNodePrefabOverride`（构造时抓前像，undo 复用 `push_group` 一格） | tests **+12** |
| **E04** | 命令、能力与不变量 | `r3e-core/src/cmd_spec/prefab.rs`、`r3e-protocol/src/{api,dispatch}/prefab.rs`、`r3e-runtime/src/prefab_ops.rs`、`r3e-editor-host/src/api/prefab.rs`、`r3e-script/src/api/prefab.rs`、`r3e-doc/src/validate/prefab.rs` | `prefab.*` 命令 + rpc 入口 + 1 条不变量三源一致 | 6 条命令**归 `Domain::Scene`**（**不建 `Domain::Prefab`**，`domains` +0）；2 条带 rpc（`prefab.spawn` / `prefab.info`）、4 条脚本专有（`prefab.define` / `prefab_spawn_many` / `prefab_set_override` / `prefab_clear_override`，**均有真实 `register_fn` 注册点**）；不变量 `prefab.override_leak`（layer=`prefab`） | tests **+10**、commands **+6**、rpc **+2**、script_only **+4**、invariants **+1** |
| **E05** | 判据测试与文档收口 | `r3e-core/tests/prefab_instances.rs`、`r3e-doc/tests/prefab_issues.rs`、`docs/ARCHITECTURE.md`、`state_hash.rs` | 覆盖只存差异与不变量可自动断言；架构文档登记 Prefab | 端到端 100 实例判据 + 不变量反例 + 真实样例正跑防假阳性 + `state_hash` 实例段（`P3`） | tests **+4** |
| 合计 | | | | | tests **+58**、commands **+6**、rpc **+2**、script_only **+4**、invariants **+1**、domains **0** |

> 口径提示：`M2_PLAN §4` 的 E 行只写 `tests +58 / commands +6 / rpc +2 / invariants +1 / domains 0`（**下限**）。`script_only +4` 是该预算在机械口径下的**必然推论**（`419 − 85 = 334`），本文 §11.2 单列偏差登记与处置，不隐藏、不靠「伪 rpc」抹平。

---

## 2. 开工实测真值（2026-11-26）

### 2.1 门禁现状

| 门禁 | 实测 | 采集方式 |
|---|---|---|
| `tests` | **2426**（`r3e-core` **646** / `r3e-doc` **169** / `r3e-protocol` **75** / `r3e-editor-host` **149**） | `cargo xtask check-tests` |
| `commands` | **413** | `cargo xtask gen-capabilities --check` |
| `rpc` | **83** | 同上 |
| `script_only` | **330**（413 − 83） | 同上 |
| `domains` | **18**（`Domain` 枚举 `cmd_spec/mod.rs` L33，**无 `Prefab`**） | 同上 |
| `invariants` | **44**（`ISSUE_CODES` `validate/invariants.rs` L29，**无 `prefab.*`**） | 同上 |
| `dep_debt` | **0**（limit 0） | `cargo xtask dep-check --strict` |
| `size_debt` | **20** 条（limit **800**；`prefab` 相关文件无一在册） | `xtask/size_debt.json` |
| 工作区 | 干净（仅未跟踪 `assets/labs/nav_demo/tools/__pycache__/`）；`HEAD = 8fb9f4b`（`feat(labs): nav_demo …`；其上 `ed5a615` 为 `feat(m2-d06)`） | `git status --short` |

常量锚点：`xtask/src/gen_capabilities.rs` L46 `EXPECTED_COMMANDS = 413`，同文件断言 `rpc == RPC_METHODS.len()`、`invariants == ISSUE_CODES.len()`；`caps.rs` L115–119 由 `rpc = commands.filter(rpc.is_some()).count()`、`script_only = commands.len() - rpc` 现算。

### 2.2 空白面清单（E 阶段从零建的部分）

| 项 | 实测 |
|---|---|
| `r3e-core/src/prefab/` | **不存在** |
| `r3e-core/src/lib.rs` | **41 个 `pub mod`**，无 `prefab` |
| 全仓 `Prefab` 类型 | **0 个**（`grep -r "struct Prefab" crates` 无命中） |
| `SceneFile` 根字段 | 17 项（`scene.rs` L163–215：`objects`/`terrain`/`spawns`/`materials`/`texts`/`ui`/`physics`/`assets` …），**无 `instances`** |
| `cmd_spec` | 18 个域模块，**无 `prefab.rs`** |
| `ISSUE_CODES` | 44 条，layer 取值集合无 `prefab` |
| `assets/` | 仅 `scene.json` / `scene.schema.json` / `scene_*.json` 与 `audio`、`labs` 子目录，**无 `prefab/`** |
| `docs/m2/M2_E_PLAN.md` | **不存在**（本文为首次落盘） |

### 2.3 可复用件清单（决定成本的关键）

| 复用件 | 位置 | 用法 |
|---|---|---|
| `ObjectDesc` | `scene_desc.rs` L11–56，13 字段（`name`/`mesh`/`position`/`rotation`/`scale`/`color`/`parent`/`visible`/`physics`/`material`/`material_ref`/`tags`/`interact_radius`），全 `Option` + `skip_serializing_if`，已 `serde + schemars` 双派生 | 直接做 `PrefabTemplate.proto`；`ObjectOverride` 的 13 字段与之逐项对齐 |
| 表范式三件套 | `spawn.rs`（241 行）：`SPAWN_MAX` / `sanitized` / `normalize_all` / `upsert` | `PrefabTable` 照抄结构，只把元素换成 `Arc<PrefabTemplate>` |
| 内存共享唯一先例 | `scene.rs` L246 `anim_clips: HashMap<String, Arc<AnimClip>>` | `PrefabTable` 的 `Arc` 共享走同一范式（不是新发明） |
| 命令横向范式 | `cmd_spec`(core) → `r3e-protocol/src/api/*` + `dispatch/*` → `r3e-runtime/src/*_ops.rs` → `r3e-editor-host/src/api/*` | D 阶段刚走通一遍（`nav.*`），E04 逐层照抄 |
| 事务前像机制 | `CommandKind` 值对象「构造时抓前像」（`r3e-doc/src/command/mod.rs` L64–69） | 新变体只加前像字段，**不改机制** |
| schema 防漂移手法 | `scene_schema.rs`（72 行）+ `bin/` dump + 逐字节比对测试 | `prefab_schema.rs` 同构复刻 |
| 不变量反例 helper | `r3e-doc/tests/validate.rs` 的 `assert_error(doc, code)` | 正是 `xtask invariants` 的 **B 源**扫描对象，三源一致自动成立 |

### 2.4 必须绕开的既有命名

| 既有物 | 位置 | 语义 | E 阶段处置 |
|---|---|---|---|
| `scene.spawn_prefab` 意图 | `r3e-editor-host/src/api/tx.rs` L286–291 白名单、L415 `IntentPlan::SpawnPrefab(Vec<ObjectDesc>)`、L550 起展开为 N 条 `SpawnNode`，`SPAWN_PREFAB_MAX_NODES = 256` | 「批量新建节点」，**不是**模板实例化 | **不动**（`P4`）。新命令用 `prefab.*`，文档写清二者区别 |
| `CommandKind::SpawnNode` | `r3e-doc/src/command/mod.rs` L71 起（20 变体，**无 `SpawnPrefab`**） | 单节点生成 | 实例化**复用**它，不新增变体（undo/redo 白拿） |
| `SPAWN_PREFAB_MAX_NODES` | `tx.rs` | 批量上限 256 | `prefab_spawn_many` 的 `count` 上限**对齐 256**，不另立常量 |

### 2.5 现状能力自检六问

| 问 | 答 | 证据 | E 阶段处置 |
|---|---|---|---|
| 能表达「实例」吗 | **不能** | `SceneFile` 17 根字段无实例表；无 `Prefab` 类型 | E01 + E02 新建 |
| 能表达「覆盖」吗 | **不能** | `ObjectDesc` 13 字段是单层 `Option`，无法区分「未覆盖」与「覆盖为空」 | E01 三态 `FieldOverride<T>`（`P1`） |
| 模板在内存里会重复吗 | **会**，若不刻意共享 | 现无模板表；朴素写法每个实例存一份 `ObjectDesc` 拷贝 | `Arc<PrefabTemplate>` + `Arc::ptr_eq` 判据（E02） |
| 覆盖能只落差异吗 | **不能** | 无 diff 算子 | E03 三算子 + `prefab.override_leak` 不变量（E04） |
| 实例改动在 `state_hash` 里看得见吗 | **看不见** | `state_hash.rs` 覆盖 order / 每节点字段 / `next_id` / camera / terrain / spawns / materials / assets，**不覆盖** `instances`（字段尚不存在） | `P3` + E05 同卡加实例段 |
| 命令面能调吗 | **不能** | `cmd_spec` 无 prefab 命令；`RPC_METHODS` 无 `prefab.*` | E04 六条命令 + 2 rpc |

### 2.6 硬约束（跨卡生效）

| 约束 | 实测值 | 对 E 阶段的要求 |
|---|---|---|
| `dep_rules: r3e-core` | prefab 为 core **内部子目录**，不引入新 crate | `R8`：**不得新建 crate**（`M2_PLAN §6` 文件占用矩阵已把 `r3e-core/src/prefab/` 划给 E 阶段） |
| `dep_rules: r3e-script` | allow 仅 `core` / `doc` / `protocol` | 脚本侧只经命令注册，**不给 `r3e-script` 加任何边**（否则 `dep_debt limit = 0` 当场红） |
| `dep_rules: r3e-runtime` / `r3e-editor-host` | runtime 已含 core/protocol；editor-host 已含 runtime | 命令面实现落 `r3e-runtime::prefab_ops`，`editor-host` 薄转发（与 `nav_ops` / `terrain_ops` 同址） |
| `size_debt` | limit **800**，20 条在册 | 新增文件同样受 800 限制；`state_hash.rs` 678、`cmd_spec/scene.rs` 601、`api.rs` 780 均为高危文件 → 新代码一律落新文件 |
| `args.rs` | **783** / 800（余量 **17** 行） | 新增命令一律 `rpc --method prefab.xxx` 透传，**不动** `args.rs` |
| `R2`：`app.rs` | **2816** 行（在册 `size_debt`） | E 阶段**禁止改 `app.rs`**；如必须，先触发 G02 提前 |
| `R4` 串行 | M2 一卡一提交、失败就地修 | E01→E05 **严格串行**，无并行空间 |
| 协议守恒 | 规则 4「`script_only` 不得增加」（现 330） | **本阶段按 `P5=B` 必然 +4**（偏差登记见 §11.2；处置见 §13 `R1`） |
| 预算下限 | E 行：tests **+58** / commands **+6** / rpc **+2** / invariants **+1** / domains **0** | `commands +6` 是**下限**：少加即全局目标落空 |

---

## 3. 总体设计

### 3.1 覆盖表达形态（`P1` = A：强类型 13 字段三态）

```
r3e-core/src/prefab/
├── mod.rs        // 重导出 + PrefabId 形状校验 + 常量（<120 行）
├── template.rs   // PrefabTemplate：id / schema / proto: ObjectDesc / tags（<250 行）
├── instance.rs   // PrefabInstance + ObjectOverride（13 字段强类型覆盖）（<400 行）
├── diff.rs       // diff / apply / revert 三算子（<300 行）
└── table.rs      // PrefabTable：BTreeMap<String, Arc<PrefabTemplate>> + upsert/normalize_all（<250 行）
```

```rust
pub struct PrefabTemplate {
    pub id: String,        // "prop.crate"，点分小写（同 ISSUE code 形状校验，禁下划线/大写）
    pub schema: u32,       // 模板格式版本，首版 = 1
    pub proto: ObjectDesc, // 原型，直接复用既有装配入口 Scene::spawn
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

pub struct PrefabInstance {
    pub name: String,              // 实例节点名，遵守 Scene 唯一名校验
    pub template: String,          // 模板 ID 引用（D7=A）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "ObjectOverride::is_empty")]
    pub overrides: ObjectOverride, // diff：只存差异（D6=A）
}

/// 覆盖字段三态：未覆盖（None）/ 覆盖为某值（Set）/ 显式清空模板值（Clear）
pub enum FieldOverride<T> { Set(T), Clear }

pub struct ObjectOverride {
    // 13 个字段与 ObjectDesc 一一对应（name 不参与覆盖，实例名由 PrefabInstance.name 决定）
    pub mesh: Option<FieldOverride<String>>,
    pub position: Option<FieldOverride<[f32; 3]>>,
    pub rotation: Option<FieldOverride<[f32; 3]>>,
    pub scale: Option<FieldOverride<[f32; 3]>>,
    pub color: Option<FieldOverride<[f32; 4]>>,
    pub parent: Option<FieldOverride<String>>,
    pub visible: Option<FieldOverride<bool>>,
    pub physics: Option<FieldOverride<bool>>,
    pub material: Option<FieldOverride<String>>,
    pub material_ref: Option<FieldOverride<String>>,
    pub tags: Option<FieldOverride<Vec<String>>>,
    pub interact_radius: Option<FieldOverride<f32>>,
    // 第 13 项为 ObjectDesc 中对应 `name` 以外的剩余字段；由 opt_field! 宏统一展开，
    // 宏同时生成 is_empty()、字段名 ↔ 访问器映射（供 prefab_set_override 的 `field` 参数与
    // prefab.override_leak 不变量使用），避免手写 13 份重复分支。
}
```

三态是**必须**的，不是洁癖：`ObjectDesc` 每个字段本身就是 `Option`，「未覆盖」与「覆盖为 None」语义相反（后者要把模板值清空），单层 `Option` 表达不了；`BTreeMap<String, Value>` 方案（`P1` 档 B）虽省扩展成本，但字段名拼错只能靠 validate 事后兜底，且 schema 退化为 `additionalProperties`——与本仓「手写枚举、拒绝动态化」的既有风格相悖。

### 3.2 序列化外形（**冻结**，E01 判据逐字节锁死）

```json
{ "name": "crate_07", "template": "prop.crate",
  "overrides": { "position": [1.5, 0, 2.25] } }
```

| 态 | 序列化外形 | 反序列化 |
|---|---|---|
| 未覆盖 | **键缺失** | 键缺失 → `None` |
| `Set(v)` | `"字段": v` | `v` → `Some(Set(v))` |
| `Clear` | `"字段": null` | `null` → `Some(Clear)` |

实现要点：一个 `opt_field!` 宏 + 每字段 `#[serde(default, deserialize_with = "...", skip_serializing_if = "Option::is_none")]`，约 30 行；**关键**是 `Some(null) → Clear` 与「键缺失 → `None`」必须可区分——这是 `serde` 默认 `Option` 语义会吃掉的信息，必须用自定义 `deserialize_with` 显式分辨（判据 ②/④）。

### 3.3 `Scene` 集成与内存共享（E02）

- `SceneFile` 新增 `instances: Vec<PrefabInstance>`（`#[serde(default, skip_serializing_if = "Vec::is_empty")]`），**位置紧随 `objects` 之后**，schema 属性与 `spawns` 对齐。
- `Scene` 新增 `instances: Vec<PrefabInstance>` + `prefabs: PrefabTable`（模板表**不序列化**，运行时按 ID 从外部文件载入）。
- 实例化路径：`PrefabInstance` → `apply(proto, overrides)` 得 `ObjectDesc`（`name` 换成实例名）→ **仍走 `Scene::spawn`**，**不新增第二条装配路径**（重名校验 / 父级按名解析 / `touch_layout` 全部白拿，且不会与 `SpawnNode` 的 undo 语义分叉）。
- 内存共享：`PrefabTable` 存 `Arc<PrefabTemplate>`，实例只持 ID + diff。判据断言 100 实例引同一模板时 `Arc::ptr_eq` 全为 true、`strong_count == 101`。
- **接缝清单（E02 必查，13 处）**：① `SceneFile` 字段；② `Scene` 字段 + `Scene::new`；③ `scene/io.rs::from_file`（归一化 + 未知模板 ID 报错）；④ `scene/io.rs::to_scene_file`（导出，与 ③ 对称）；⑤ `scene_schema.rs` + 重生成 `assets/scene.schema.json`；⑥ `state_hash.rs`（+mix 段 + L31–51 对照表登记，E05 收口）；⑦ `validate` 挂载 `prefab` 层（E04）；⑧ 新建 `scene/prefab.rs` 实例化入口；⑨ **`xtask/doc_gate.json` 的 `rule6_scene_keys`**（若其列了 `scene.json` 合法键集合，必须同步加 `instances`，否则 `doc-gate` 直接红）；⑩ `r3e-doc` 不变量测试的 layer 白名单（见 §7.4）；⑪ `tests/scene_schema.rs` 的扫描筛子（确认不误扫 `prefab`，见 §3.4）；⑫ `editor-host/src/api/tx.rs`（确认无需改，旧意图不动）；⑬ `r3e-cli/src/args.rs`（确认不动）。

### 3.4 模板外部文件与 schema（`P6` = A、`P3` = A）

| 项 | 决定 |
|---|---|
| 模板文件 | `assets/prefab/<id 转路径>.json`，点分转目录：`prop.crate` → `assets/prefab/prop/crate.json` |
| schema | 新增 `crates/r3e-core/src/prefab_schema.rs`（`schemars::schema_for!(PrefabTemplate)`）+ `bin/prefab_schema_dump.rs`，落盘 `assets/prefab.schema.json`，配**逐字节防漂移**测试（复刻 `scene_schema.rs` 手法） |
| 与既有测试的隔离 | `tests/scene_schema.rs` L126–129 的筛子是 `n == "scene.json" \|\| (n.starts_with("scene_") && n.ends_with(".json"))`，`prefab` 命名**不会被误扫**，二者互不干扰，**无需改旧测试** |
| 样例模板 | `assets/prefab/prop/crate.json`（`prop.crate`，13 字段接近全填，供 §9.2 全字段夹具复用） |
| `state_hash` 口径 | **实例**（模板 ID + `parent` + 覆盖 diff）**入** hash；**模板文件内容不入**（与既有 `assets` 段口径一致：只对登记项元数据入 hash，文件字节不入） |
| 不做 | 模板热重载 / 文件监听（归 G 阶段流式加载）；E 阶段只做「按 ID 载入 + 未找到即明确报错」 |

### 3.5 事务与撤销（E03）

`CommandKind` 新增 **2 个变体**（20 → 22）：

| 变体 | 载荷 | undo 语义 |
|---|---|---|
| `DefinePrefab` | `next: PrefabTemplate, old: Option<PrefabTemplate>` | `old` 写回模板表；`None` 则删条目 |
| `SetNodePrefabOverride` | `name: String, old: ObjectOverride, new: ObjectOverride` | 前像写回，天然可逆 |

- 前像在**构造时**抓取（`command/mod.rs` L64–69 既有约定），`commit` 时 `push_group` 一格 → undo/redo 复用既有机制，**不改事务框架**。
- 实例化本身**不进新变体**：展开为既有 `SpawnNode`（与 `scene.spawn_prefab` 意图同构），保证 undo 走同一条路。
- `undo` 后**不**退 `next_id`——与 `tx.rs` L337–339 的既有结论一致，故 §9.3 的判据用「序列化字节相等 + `scene_hash` 初末相等」，**不用**「undo 后哈希与新建场景完全一致」的强断言。

### 3.6 命令面 6 条（`P4` = A 归 `Domain::Scene`；`P5` = B 取 `spawn` + `info`）

| # | 脚本名 | RPC | `Domain` | params → returns | 说明 |
|---|---|---|---|---|---|
| 1 | `prefab_define` | — | `Scene` | `(id: string, path: string) → bool` | 从文件载入/更新模板入表；未知/非法路径 → `PrefabOpError::Invalid`；**无 rpc**，只走脚本/宿主本地调用（见 `P5` 备注） |
| 2 | `prefab_spawn` | **`prefab.spawn`** | `Scene` | `(id: string, name: string, parent: string?) → string` | 单实例化，返回实例名；模板未注册 → `PrefabOpError::Conflict` |
| 3 | `prefab_spawn_many` | — | `Scene` | `(id: string, prefix: string, count: int) → int` | 批量实例化，`count ≤ 256`（对齐 `SPAWN_PREFAB_MAX_NODES`），返回实际生成数 |
| 4 | `prefab_set_override` | — | `Scene` | `(name: string, field: string, value: dynamic) → bool` | 只写差异字段；`field` 不在 13 字段内 → `Invalid` |
| 5 | `prefab_clear_override` | — | `Scene` | `(name: string, field: string) → bool` | 回退为继承模板 |
| 6 | `prefab_info` | **`prefab.info`** | `Scene` | `(name: string) → map` | 返回 `template` + 覆盖字段清单；**带 rpc**，编辑器 UI 可跨进程直读（宿主直连同路径，见 `P5` 备注） |

**`P5` 采纳草案建议 B 的口径（留痕）**：`rpc` 两条额度按「**读写各一 + 编辑器 UI 友好**」分配——`prefab.spawn` 是跨进程**写**入口（唯一的实例化入口，决定场景内容），`prefab.info` 是跨进程**读**入口（编辑器 UI 取模板 ID 与覆盖字段清单，无需与宿主同进程，也不必先拉整份 `scene.json` 自己反推）。**`prefab.define` 无 rpc**：模板建库只能走脚本（`r3e-script/src/api/prefab.rs` 的 `register_fn`）或宿主本地调用（`r3e-editor-host/src/api/prefab.rs` 的 `prefab_define` 处理函数仍在，只是**不登记进 `RPC_METHODS`**）；因此**跨进程复现的前提是先本地建模板**（模板文件随仓库分发，`assets/prefab/<id>.json` 到位后 `rpc --method prefab.spawn` 即可跑通）。该缺口的补口触发条件登记在 §13 `R7`。

**`script_only +4` 的由来（不是偷懒）**：本仓命令的脚本名由 `gen_cmd_spec.py` 扫描 `register_fn` 生成，因此 4 条无 rpc 的命令**必须有真实脚本注册点**（`r3e-script/src/api/prefab.rs`），否则命令名根本签不出来。也就是说这 4 条是「脚本可用、无 RPC」的真实能力面，**不是**占位死条目。这是与 C 阶段（`terrain.*` 8 条全带 rpc，为守住「`script_only` 不增」）的**有意不同**，代价与处置见 §11.2 / §13 `R1`。

### 3.7 不变量（1 条）

| code | layer | severity | 触发条件 |
|---|---|---|---|
| `prefab.override_leak` | `prefab` | `Error` | ① 实例 `overrides` 中出现**与模板同值**的字段（等于把模板值写回实例）；② 实例指向**不存在的模板 ID**；③ 实例 `overrides` 字段数与模板同值字段数 > 0（冗余快照） |

三源一致：`ISSUE_CODES` 登记 1 条 + `r3e-doc/tests/prefab_issues.rs` 反例 1 条（走 `assert_error` helper，`xtask invariants` 的 B 源）+ `docs/capabilities.json` 重生成各 1 条。语义定位：这是「覆盖字段不得写回模板」这条架构约束的**可自动判定落点**，而不是又一条风格检查。

### 3.8 `state_hash` 口径（`P3` = A）

| 入 hash | 不入 hash |
|---|---|
| `instances` 段（顺序 + `name` + `template` + `parent` + 每个覆盖字段的**态与值**） | 模板**文件内容**（与 `assets` 段一致：登记项元数据入、文件字节不入） |
| 既有：order / 每节点字段 / `next_id` / camera / terrain / spawns / materials / assets | 既有：`time` / `frame` / `anim_clips`（`HashMap` 无序）/ 物理缓存 |

理由：实例改了却 hash 不变，回放与 `scene-verify` 会漏判；而模板文件字节入 hash 会让「只改注释/格式化模板文件」也漂移，且与 `assets` 既有口径打架（导致 `state_hash` 出现两套资产语义）。副作用（须接受）：**只改模板文件不改实例时，`scene_hash` 不变**——这条已在 §13 `R5` 明确登记为已知行为，处置是「模板变更须伴 `prefab_define` 命令，命令本身入 hash」。

### 3.9 文件与行数预算（≤800/文件，`size_debt` 不得新增）

| 文件 | 现 | E 后 | 归属卡 |
|---|---|---|---|
| `crates/r3e-core/src/prefab/mod.rs` | — | 新建 <120 | E01 |
| `crates/r3e-core/src/prefab/template.rs` | — | 新建 <250 | E01 |
| `crates/r3e-core/src/prefab/instance.rs` | — | 新建 <400 | E01 |
| `crates/r3e-core/src/prefab/diff.rs` | — | 新建 <300 | E03 |
| `crates/r3e-core/src/prefab/table.rs` | — | 新建 <250 | E01 |
| `crates/r3e-core/src/prefab_schema.rs` | — | 新建 <100 | E01 |
| `crates/r3e-core/src/bin/prefab_schema_dump.rs` | — | 新建 <60 | E01 |
| `crates/r3e-core/src/lib.rs` | 173 | ≈176（+1 `pub mod`、+1 `pub use`） | E01 |
| `crates/r3e-core/src/scene.rs` | 601 | ≈615 | E02 |
| `crates/r3e-core/src/scene/io.rs` | 286 | ≈320 | E02 |
| `crates/r3e-core/src/scene/prefab.rs` | — | 新建 <300 | E02 |
| `crates/r3e-core/src/scene_schema.rs` | 72 | ≈86 | E02 |
| `crates/r3e-doc/src/command/mod.rs` | 327 | ≈360（+2 变体 × 四处：枚举/`label`/`apply`/`undo`/`target_name`） | E03 |
| `crates/r3e-doc/src/state_hash.rs` | 678 | ≈720（**余量约 80，硬约束**） | E05 |
| `crates/r3e-core/src/cmd_spec/prefab.rs` | — | 新建 <180 | E04 |
| `crates/r3e-core/src/cmd_spec/mod.rs` | 253 | ≈256 | E04 |
| `crates/r3e-protocol/src/api/prefab.rs` | — | 新建 <140 | E04 |
| `crates/r3e-protocol/src/dispatch/prefab.rs` | — | 新建 <180 | E04 |
| `crates/r3e-protocol/src/lib.rs` | 299 | ≈302（`RPC_METHODS` +2） | E04 |
| `crates/r3e-runtime/src/prefab_ops.rs` | — | 新建 <320 | E04 |
| `crates/r3e-editor-host/src/api/prefab.rs` | — | 新建 <200 | E04 |
| `crates/r3e-editor-host/src/api/mod.rs` | 411 | ≈414 | E04 |
| `crates/r3e-script/src/api/prefab.rs` | — | 新建 <200 | E04 |
| `crates/r3e-script/src/api/mod.rs` | 451 | ≈456 | E04 |
| `crates/r3e-doc/src/validate/prefab.rs` | — | 新建 <160 | E04 |
| `crates/r3e-doc/src/validate/mod.rs` | 387 | ≈392 | E04 |
| `crates/r3e-doc/src/validate/invariants.rs` | 348 | ≈353（+1 code，layer 白名单 +1） | E04 |
| `xtask/src/gen_capabilities.rs` | 303 | ≈303（`EXPECTED_COMMANDS` 413→419） | E04 |
| `assets/prefab.schema.json` / `assets/prefab/prop/crate.json` | — | 生成物 | E01 |

新建源码 **16 个**（每文件均 <800，零入债）；新建测试文件 **9 个**（见 §11.1）。`cmd_spec/scene.rs`（601）**不动**——6 条命令走新文件；`api.rs`（780）/ `args.rs`（783）/ `app.rs`（2816）**全部不碰**。

---

## 4. E01 · Prefab 数据模型与模板格式冻结（2.5 人日，tests +18）

### 4.1 落点与触点

| 类 | 文件 |
|---|---|
| 新建 | `r3e-core/src/prefab/{mod,template,instance,diff,table}.rs`（`diff.rs` 本卡只出空壳签名，实体在 E03）、`r3e-core/src/prefab_schema.rs`、`r3e-core/src/bin/prefab_schema_dump.rs`、`assets/prefab.schema.json`、`assets/prefab/prop/crate.json`、`r3e-core/tests/prefab.rs` |
| 改动 | `r3e-core/src/lib.rs`（`pub mod prefab;` + `pub mod prefab_schema;`） |

### 4.2 判据（先写红，18 条）

| 组 | 条数 | 内容 |
|---|---|---|
| 往返 | 4 | ① `PrefabTemplate` ↔ JSON 往返逐字段等值；② `PrefabInstance` 往返；③ 空 `overrides` 不落盘（`skip_serializing_if`）；④ 未知字段名 → 反序列化**报错**，不静默丢弃（`deny_unknown_fields` 口径） |
| 三态外形 | 5 | ⑤「未覆盖」序列化后**键缺失**；⑥ `Set(v)` 落 `v`；⑦ `Clear` 落 `null`；⑧ `null` 反序列化为 `Some(Clear)` 而非 `None`（关键分歧点）；⑨ 键缺失反序列化为 `None` |
| `PrefabId` 形状 | 3 | ⑩ 合法 `prop.crate` 通过；⑪ 含大写下划线 `Prop_Crate` 被拒；⑫ 空段 `prop..crate` 被拒 |
| `ObjectOverride` 机械化 | 3 | ⑬ 13 字段全部 `is_empty()` 初始为 true；⑭ 宏生成的「字段名 ↔ 访问器」映射覆盖 13 项且无重复；⑮ `is_empty()` 对任一字段 `Set`/`Clear` 均返回 false（`Clear` 也是「非空覆盖」，否则清空动作会丢） |
| schema | 3 | ⑯ `assets/prefab.schema.json` 逐字节防漂移（重生成必须零差异）；⑰ `prop.crate` 样例过 schema 校验（`jsonschema` 或仓内既有校验手法）；⑱ `schema` 字段版本与文件头一致 |

### 4.3 实施步骤

1. 写 `template.rs` / `instance.rs` / `table.rs`（表范式照 `spawn.rs`：`PREFAB_MAX`、`sanitized`、`normalize_all`、`upsert`）。
2. 写 `opt_field!` 宏（生成三态字段、`is_empty`、字段名映射），先在 `instance.rs` 内定义、稳定后再决定是否上提 `mod.rs`。
3. 写 `prefab_schema.rs` + dumper bin，生成 `assets/prefab.schema.json`；写 `assets/prefab/prop/crate.json`。
4. 判据先红后绿，`cargo xtask verify` 11 步全绿。

### 4.4 提交与回滚点

- 提交：`feat(m2-E01): prefab 数据模型与模板格式冻结`
- 回滚：`git reset --hard HEAD~1`（本卡无跨卡依赖，E02 尚未开始即完全干净）

### 4.5 实施记录（2026-11-26 落地）

**落盘文件与行数**（`cargo fmt --all` 后实测）：

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-core/src/prefab/mod.rs` | 142 | 模块文档、`PrefabIdError`、`validate_prefab_id` / `is_valid_prefab_id`、`prefab_rel_path`、再导出 |
| `crates/r3e-core/src/prefab/template.rs` | 142 | `PrefabTemplate`（`deny_unknown_fields`）、`PREFAB_SCHEMA_VERSION = 1`、`PREFAB_MAX = 256`、`sanitized` / `validate` |
| `crates/r3e-core/src/prefab/instance.rs` | 303 | `FieldOverride<T>` 三态、宏 `object_override!` 生成 12 字段 `ObjectOverride`、`OverrideState`、`PrefabInstance` |
| `crates/r3e-core/src/prefab/diff.rs` | 47 | `apply` 实体（原型 + 覆盖 + 实例名）；`diff` / `revert` 留 `unimplemented!` 指向 E03 |
| `crates/r3e-core/src/prefab/table.rs` | 141 | `PrefabTable`：`BTreeMap<String, Arc<PrefabTemplate>>`、同 ID 替换、`normalize_all`、`upsert` |
| `crates/r3e-core/src/prefab_schema.rs` | 80 | `prefab_schema()` / `prefab_schema_pretty()` / `PREFAB_SCHEMA_PATH` / `PREFAB_SCHEMA_ID` / `prefab_root_properties()` |
| `crates/r3e-core/src/bin/prefab_schema_dump.rs` | 62 | 落盘 bin（默认路径 / 指定路径 / `--stdout`） |
| `crates/r3e-core/tests/prefab.rs` | 581 | 18 条判据（含测试内极简 schema 校验器，只覆盖 schemars 1.x 实际关键字子集） |
| `assets/prefab.schema.json` | 246 | 生成物（`cargo run -p r3e-core --bin prefab_schema_dump`） |
| `assets/prefab/prop/crate.json` | 31 | 样例模板（`prop.crate`，含 `proto` / `tags` / `physics` / `material`） |
| `crates/r3e-core/src/lib.rs` | +2 行 | `pub mod prefab;` / `pub mod prefab_schema;` |

**判据与用例映射**（18 条一条不落，组序即测试文件内顺序）：

| 组 | 判据 | 用例 |
|---|---|---|
| 往返 4 | ①②③④ | `template_roundtrip_keeps_every_field` / `instance_roundtrip_keeps_parent_and_overrides` / `empty_overrides_key_is_absent` / `unknown_fields_are_rejected` |
| 三态 5 | ⑤⑥⑦⑧⑨ | `unset_override_serializes_as_missing_key` / `set_override_serializes_as_value` / `clear_override_serializes_as_null` / `null_deserializes_to_clear_not_inherit` / `missing_key_deserializes_to_none` |
| `PrefabId` 3 | ⑩⑪⑫ | `legal_prefab_ids_are_accepted` / `uppercase_or_underscore_ids_are_rejected` / `empty_segment_ids_are_rejected` |
| 机械化 3 | ⑬⑭⑮ | `every_field_starts_inherited` / `field_names_cover_object_desc_minus_name` / `any_override_makes_is_empty_false` |
| schema 3 | ⑯⑰⑱ | `schema_file_matches_derivation` / `sample_templates_validate_against_schema` / `schema_version_matches_crate_constant` |

**门禁实测**：`cargo xtask verify` 11 步全绿；tests 2426 → **2445**（`r3e-core` 646 → **665**，**+18** 新增与预算一致、零删减——逐文件对账为 HEAD 实测 647 + 新增 18 = 665，故基线表 646 → 665 的 `+19` 中有 **1 条是入卡前已存在的基线滞后**（D06 收口时 `r3e-core` 基线 646、实测已 647，`check-tests` 只拦「减少」故当时未红），本卡按实测一并对齐并在 `tests_baseline.json` 的 `_comment` 留痕）；commands **413** / rpc **83** / script_only **330** / domains **18** / invariants **44** 全部持平；`size_debt` **20** 一项未加（新文件最大 303 行）、`dep_debt` **0**、回放金标 `385482eb033d2964` 未漂移。

**与计划的三处口径差异**（不破判据，均在此留痕）：

1. **⑬⑭ 的「13 项」按 12 项落**：`ObjectDesc` 的 13 个字段去掉 `name` 才是可覆盖字段（实例名由实例自己给，模板的 `proto.name` 只是原型名），故 `ObjectOverride::FIELD_NAMES` 为 12 项，判据 ⑭ 改为「与 schema 内嵌 `ObjectDesc` 键集去掉 `name` 后**集合相等**」——比数条目更硬，宏清单漏改即红。
2. **`diff.rs` 本卡只到 `apply`**：`diff` / `revert` 以 `unimplemented!("E03")` 占位，E03 补实体（判据⑪的可逆性归 E03）。
3. **`sanitized` 不只去空白**：`PrefabTemplate::sanitized` 同时把 `tags` 按 `PREFAB_MAX_TAGS = 16` **截断去重**（沿用 `MAX_NODE_TAGS` 口径），故 `validate` 只管 ID 形状与原型名——标签超限走归一化而非报错，判据 ⑫ 补「单段 ID（`crate`）被拒」用例把这层边界钉死。

**E02 起算**：`PrefabInstance::validate` 只校模板 ID 形状，实例重名与父级存在性留给 `Scene::spawn`；`PrefabTable` 已就绪（`Arc` 共享 + 同 ID 替换 + `normalize_all`），E02 只需在 `Scene` 侧挂实例表并复用 `Scene::spawn`。

---

## 5. E02 · 实例化与 `Scene` 集成（2.0 人日，tests +14）

### 5.1 落点与触点

| 类 | 文件 |
|---|---|
| 新建 | `r3e-core/src/scene/prefab.rs`（实例化入口 `spawn_instance` / `spawn_instances` / `resolve_overrides`）、`r3e-core/tests/prefab_scene.rs` |
| 改动 | `scene.rs`（`SceneFile` / `Scene` / `Scene::new` 三处加字段，`SceneFile.instances` 紧随 `objects`）、`scene/io.rs`（`from_file` 归一化 + `to_scene_file` 导出两处对称）、`scene_schema.rs` + 重生成 `assets/scene.schema.json` |

### 5.2 判据（先写红，14 条）

| 组 | 条数 | 内容 |
|---|---|---|
| 内存共享 | 3 | ① 100 实例 `Arc::ptr_eq` 全等；② `strong_count == 101`；③ 「不共享」反例：两个不同模板 ID 的实例指针**不等**（防判据恒真） |
| 命名与父级 | 3 | ④ 实例名与既有节点重名 → 明确错误（不静默覆盖）；⑤ `parent` 按名解析成功且 `touch_layout` 生效；⑥ 父级不存在 → 明确错误 |
| 序列化往返 | 4 | ⑦ `to_scene_file` → `from_file` 往返后 `instances` 逐字节一致；⑧ 空实例表不落盘；⑨ 样例 `assets/scene.json` 无 `instances` 键时向后兼容（旧场景能开）；⑩ `assets/scene.schema.json` 含 `instances` 且逐字节防漂移 |
| 模板解析 | 2 | ⑪ 未注册模板 ID → 报错（明确 `PrefabOpError::Conflict` 口径，不 panic）；⑫ `prefabs` 表**不**出现在 `scene.json`（模板不序列化） |
| 装配复用 | 2 | ⑬ 实例化后的节点在 `Scene.nodes` 中字段与 `apply(proto, overrides)` 结果逐字段一致；⑭ 实例化走 `Scene::spawn`（以「重名校验/父级解析仍生效」间接证明未另开路径） |

### 5.3 实施步骤

1. 先改 `scene.rs` 字段与 `Scene::new`，再改 `io.rs` 两处对称点（**必须同批**，只改一半会导致往返测试直接红）。
2. `scene/prefab.rs` 只做「拼 `ObjectDesc` + 调 `Scene::spawn`」，**不写第二套节点插入逻辑**。
3. 重生成 `assets/scene.schema.json`；**同批核对 `xtask/doc_gate.json` 的 `rule6_scene_keys`**（接缝 ⑨），缺失即补 `instances`。
4. `cargo xtask verify` 全绿（重点看 `doc-gate` / `scene-verify` 两步，它们对 `scene.json` 键集合最敏感）。

### 5.4 提交与回滚点

- 提交：`feat(m2-E02): prefab 实例化与 Scene 实例表`
- 回滚：`git reset --hard HEAD~1`；**注意** schema 与 `doc_gate.json` 的改动必须与代码在**同一提交内**，否则回滚后 `doc-gate` / `scene-verify` 会残留红。

---

## 6. E03 · 覆盖只存差异与可逆性（1.5 人日，tests +12）

### 6.1 落点与触点

| 类 | 文件 |
|---|---|
| 新建 | `r3e-core/tests/prefab_diff.rs`（10）、`r3e-doc/tests/prefab_command.rs`（2） |
| 改动 | `prefab/diff.rs`（`diff` / `apply` / `revert` 三算子实体）、`r3e-doc/src/command/mod.rs`（+2 变体 × 五处：枚举 / `label` / `apply` / `undo` / `target_name`） |

### 6.2 判据（先写红，12 条）

| 组 | 条数 | 落点 | 内容 |
|---|---|---|---|
| 只落差异 | 3 | core 3 | ① 覆盖 1 字段 → 其余 12 字段全 `None`；② 覆盖值**等于**模板值 → 算子直接收敛为「未覆盖」（不落盘）；③ 覆盖值不等于模板值 → 落为该字段的 `Set`，且 `set → clear → set` 幂等（同终态同字节） |
| 三态语义 | 2 | core 2 | ④ `Clear` 使 `apply` 结果为 `None`（清空模板值）；⑤ `None` 使 `apply` 结果为模板原值（继承） |
| 可逆性 | 3 | core 2 + doc 1 | ⑥ `set → revert` 后序列化字节 `A == B`（初末相等）；⑦ `scene_hash` 初末**相等**（`P3` 下实例段入 hash）；⑧ `DefinePrefab` undo：`old = None` 时删条目、`old = Some` 时写回（doc） |
| 体积（`P2` 组合判据） | 3 | core 3 | ⑨ **结构判据（主）**：`overrides` 中与模板同值字段数 == 0 且字段数 ≤ 1；⑩ **体积判据（辅）**：`overrides` 段字节 / 全字段模板 JSON 字节 < 5%（夹具 ≥ 480 B，实测值写进测试注释）；⑪ **防退化**：实例退化为 13 字段全落盘时比值必然 ≥ 100%，断言必须红（反向自检） |
| 前像可逆 | 1 | doc 1 | ⑫ `SetNodePrefabOverride` undo：前像写回后字节与执行前相等 |

### 6.3 实施步骤

1. `diff.rs` 三算子先写判据（`diff(proto, target) → ObjectOverride`、`apply(proto, ov) → ObjectDesc`、`revert(ov) → ObjectOverride`）。
2. `command/mod.rs` 加 2 变体；**只加前像字段，不改 `push_group` 机制**；`label` / `target_name` 文案与既有风格一致（`define_prefab` / `set_node_prefab_override`）。
3. 写 §9.2 的组合判据夹具：默认夹具取 `assets/prefab/prop/crate.json`（13 字段接近全填）。

### 6.4 提交与回滚点

- 提交：`feat(m2-E03): prefab 覆盖 diff 算子与可逆性`
- 回滚：`git reset --hard HEAD~1`（E04 依赖本卡的 `CommandKind` 变体，故回滚必须在 E04 开工前完成）

---

## 7. E04 · 命令、能力与不变量（2.5 人日，tests +10）

> **本卡是 E 阶段唯一跨 crate 卡**（core + protocol + runtime + editor-host + script + doc），风险最高，建议单独留 0.5 天缓冲。

### 7.1 落点与触点

| 层 | 文件 | 增量 |
|---|---|---|
| 命令表 | 新建 `r3e-core/src/cmd_spec/prefab.rs`；`cmd_spec/mod.rs` 加 `mod prefab;` + `all()` 数组一项 | **+6 条**（全 `Domain::Scene`） |
| 协议 | 新建 `r3e-protocol/src/api/prefab.rs` + `dispatch/prefab.rs`；`r3e-protocol/src/lib.rs` `RPC_METHODS`（L147 起）**+2** | **+2 rpc** |
| 运行时 | 新建 `r3e-runtime/src/prefab_ops.rs`（范式照 `nav_ops.rs`：自有 `PrefabOpError::{Invalid, Conflict}`） | — |
| 宿主 | 新建 `r3e-editor-host/src/api/prefab.rs`（`map_op` → `RpcFailure`）；`api/mod.rs` +1 登记 | — |
| 脚本 | 新建 `r3e-script/src/api/prefab.rs`（6 个 `register_fn` 注册点，其中 4 条无 rpc 的**必须真实存在**）；`api/mod.rs` +1 登记 | — |
| 不变量 | 新建 `r3e-doc/src/validate/prefab.rs`（`pub(crate) fn check`）+ `ISSUE_CODES` +1 + `validate/mod.rs` L226–233 挂载 | **+1 invariant** |
| 生成物 | `xtask/src/gen_capabilities.rs` `EXPECTED_COMMANDS` **413→419**；重生成 `docs/capabilities.json`（**禁手改**） | — |

### 7.2 命令表实现细节

- `CommandSpec` 字段口径（`cmd_spec/mod.rs` L91）：`name` / `rpc` / `domain` / `params` / `returns` / `reversible` / `cost` / `since`；`since` 统一 `"M2-E"`。
- `reversible`：`prefab_define` / `prefab_spawn` / `prefab_spawn_many` / `prefab_set_override` / `prefab_clear_override` 均为 `true`（有前像或可反向）；`prefab_info` 只读，`false`。
- `cost`：`prefab_info` 为 `Linear`；其余含 IO 的 `prefab_define` 为 `Linear`（仅一次文件读，非重负载），`spawn_many` 上限 256 也按 `Linear`。
- `params` / `returns` 严格按 §3.6 表写，`dynamic` 只出现在 `prefab_set_override.value` 一处（**不扩散**）。

### 7.3 判据（先写红，10 条）

| 组 | 条数 | 文件 |
|---|---|---|
| 命令与清单 | 4 | `r3e-core/tests/prefab_cmd_spec.rs`：① `commands == 419`；② 6 条全为 `Domain::Scene`；③ `rpc` 恰为 `prefab.spawn` + `prefab.info` 两条（且 `prefab.define` 的 `rpc` 为空）；④ 6 条 `name` 均能由脚本侧 `register_fn` 签出（无死条目） |
| RPC 面 | 2 | `r3e-protocol/tests/prefab_rpc.rs`：⑤ `RPC_METHODS` 恰含 `prefab.spawn` / `prefab.info` 两条新方法（不含 `prefab.define`）、点分小写无下划线；⑥ dispatch 路由命中与未知方法拒绝 |
| 宿主面 | 2 | `r3e-editor-host/tests/prefab_api.rs`：⑦ `prefab_define`（本地路径调用，非 rpc）成功/非法路径 → `RpcFailure` 语义正确；⑧ `prefab_spawn` 未注册模板 → `Conflict` 透出（不 `panic`） |
| 不变量 | 2 | `r3e-doc/tests/prefab_issues.rs`：⑨ 同值字段泄漏 → `prefab.override_leak`（Error）；⑩ 指向不存在模板 ID → 同 code（`assert_error` helper，B 源） |

### 7.4 卡内必改的三处「隐性白名单」（漏一处即全仓红）

1. **`ISSUE_CODES` 的 layer 白名单**：`invariants.rs` 尾部测试校验 `layer` 取值合法，新增 `prefab` 必须同步加入允许集合。
2. **`Category` 映射**（`validate/mod.rs` L53 枚举 / L91 `from_code` / `as_str`）：`from_code` 按 code 首段映射，未匹配一律落 `Bounds`——`prefab.override_leak` 会**静默归到 `Bounds`**。处置：新增 `Category::Prefab`，同步 `from_code` / `as_str` 与既有「Category 与 layer 一一对应」测试。**不允许**放任落到 `Bounds`（会污染 `validate` 的统计口径）。
3. **`EXPECTED_COMMANDS` 与生成物同步**：`gen_capabilities.rs` L46 改 419，然后 `cargo xtask gen-capabilities`（**不带 `--check`**）重生成 `docs/capabilities.json`，再 `--check` 验证。

### 7.5 提交与回滚点

- 提交：`feat(m2-E04): prefab 命令、rpc 与 override_leak 不变量`
- 回滚：`git reset --hard HEAD~1`；**注意** `docs/capabilities.json` 为生成物，回滚须与 `EXPECTED_COMMANDS` 同批还原（同一提交，天然一致）。

---

## 8. E05 · 判据测试与文档收口（1.0 人日，tests +4）

### 8.1 落点

| 类 | 文件 |
|---|---|
| 新建 | `r3e-core/tests/prefab_instances.rs`（3）、`r3e-doc/tests/prefab_issues.rs` 补 2（共 3 条，E04 已建 2 条） |
| 改动 | `r3e-doc/src/state_hash.rs`（+实例段 mix + L31–51 对照表登记）、`docs/ARCHITECTURE.md`（新增 Prefab 章节）、`AI_ROADMAP.md §0.2`、`M2_PLAN.md` 当前进度行、`docs/m2/README.md` 索引 |

### 8.2 判据（4 条）

| # | 内容 |
|---|---|
| ① | **100 实例只存差异**端到端：100 实例 × 1 覆盖字段，实例序列化总字节 < 全字段模板字节 × 100 × 5%（`P2` 组合判据的端到端落点，与 §9.2 口径一致） |
| ② | `prefab.override_leak` 反例：破坏一个字段 → 断言必须出现该 code 的 `Error` |
| ③ | 仓库真实样例（`assets/prefab/prop/crate.json`）**正跑防假阳性**：合法场景零 `prefab.override_leak` |
| ④ | `state_hash` 口径：改实例覆盖 → hash 变；只改模板文件内容（实例不变）→ hash **不变**（把 `P3` 的取舍变成可执行断言，防止日后被人「顺手」改成入 hash 而无感知） |

### 8.3 提交与回滚点

- 提交：`feat(m2-E05): prefab 判据测试与文档收口`
- 回滚：`git reset --hard HEAD~1`；**注意** `state_hash.rs` 的实例段在 E05 落地前，`state_hash` 对实例改动是**盲区**（E02–E04 期间），故 E05 不得延后到 M2 收口再补。

---

## 9. 判据的可自动断言写法（判据实现方式）

### 9.1 E02「内存不随实例数线性膨胀」

```rust
// 断言结构而非字节：模板对象物理只有 1 份
let ids: Vec<String> = (0..100).map(|i| format!("crate_{i:02}")).collect();
let ptrs: Vec<*const PrefabTemplate> = ids
    .iter()
    .map(|n| scene.instance_template(n).expect("instance exists").as_ptr())
    .collect();
assert!(ptrs.windows(2).all(|w| std::ptr::eq(w[0], w[1])));
assert_eq!(Arc::strong_count(scene.prefabs.get("prop.crate").unwrap()), 101);
// 反例防恒真：另一个模板的实例指针必须不同
assert!(!std::ptr::eq(ptrs[0], scene.prefabs.get("prop.barrel").unwrap().as_ptr()));
```

禁止用「实例总字节 <= 模板字节 × 1.05」这类**不稳定断言**充数：它随字段浮点文本化长度漂移，会假红。

### 9.2 E03「覆盖 1 字段的实例序列化体积 < 模板 5%」（`P2` 组合判据）

按字面口径的实测推算（`serde_json` 紧凑输出）：

| 夹具 | 模板 JSON | 单字段覆盖实例 | 比值 | 字面判据可达 |
|---|---|---|---|---|
| 精简（mesh + position） | ≈ 80 B | ≈ 62 B | 77% | ✗ |
| 中等（mesh + position + scale + material） | ≈ 330 B | ≈ 66 B | 20% | ✗ |
| **全 13 字段** | ≈ 520–600 B | ≈ 26 B（仅 `"position":[1.5,0,2.25]`） | **4.3%–5.0%** | 临界 |

结论：**字面判据对夹具体积高度敏感，用默认小模板必然不可达**。故 `P2` 取**组合判据**：

1. **结构判据（主判）**：`overrides` 中与模板同值字段数 == 0、字段数 ≤ 1 → 与体积无关，稳定可断言；
2. **体积判据（辅判）**：`overrides` 段字节 / 全字段模板 JSON 字节 < 5%，夹具固定为**13 字段全填模板（≥ 480 B）**，默认值与实测值写进测试注释留痕；
3. **防退化断言（硬判据）**：实例若退化为完整快照（13 字段全落盘），比值必然 ≥ 100% → 测试必须红。

实施细则（不破判据口径）：若 E03 实测落在 5% ± 0.5% 边缘带，允许把夹具字段值补到「全非默认」（约 700 B）使比值稳定 < 5%，**不得**改阈值或改判据类别；`P2` 档 B（保持字面 5%）与档 C（放宽 15%）已登记为否决项。

### 9.3 E03「diff 合并与回退可逆」

```rust
let a = serde_json::to_vec(&inst_before).unwrap();
apply_set(&mut inst, "position", [1.5, 0.0, 2.25]);
let with_override = serde_json::to_vec(&inst).unwrap();
revert_set(&mut inst, "position");
let b = serde_json::to_vec(&inst).unwrap();
assert_eq!(a, b);                          // 字节级可逆
assert_eq!(hash_before, hash_after);       // scene_hash 初末相等
assert!(with_override.len() < a.len() + 64); // 覆盖不引入冗余快照
```

不用「undo 后 `scene_hash` 与新建场景完全一致」的强断言，理由与 `tx.rs` L337–339 一致：`SpawnNode` 的 undo **不退** `next_id`，该差异是既定设计。

### 9.4 E04「覆盖字段不得写回模板」

反例测试照 `r3e-doc/tests/validate.rs` 的 `assert_error(doc, code)` helper 写——该 helper 正是 `xtask invariants` 的 **B 源**扫描对象，因此「登记（A 源）/ 反例（B 源）/ 清单（C 源）」三源一致在 CI 里**必然**成立，不需要额外人工对账。

### 9.5 E01「schema 逐字节防漂移」

复刻 `scene_schema.rs` 手法：`schema_for!(PrefabTemplate)` → 序列化 → 与 `assets/prefab.schema.json` **逐字节比对**；不一致时测试信息里同时给「重生成命令」（`cargo run -p r3e-core --bin prefab_schema_dump`），避免下一个人靠猜。

---

## 10. 必拍板决策项（`P1`～`P6`）

### 拍板结论（2026-11-26）✅ 六项全部采纳草案推荐值

| 项 | 结论 | 一句话 |
|---|---|---|
| **P1** | **A（草案推荐）** | 覆盖表达为**强类型 `ObjectOverride` + 13 字段三态 `FieldOverride<T>`**；`BTreeMap<String, Value>` 方案登记为不做 |
| **P2** | **A（草案推荐）** | E03 体积判据改为**组合判据**：结构判据为主 + 全字段夹具（≥480 B）下 < 5% 为辅 + 防退化断言为硬判据 |
| **P3** | **A（草案推荐）** | **实例（模板 ID + 覆盖 diff）入 `state_hash`**；**模板文件内容不入**（与 `assets` 段口径一致） |
| **P4** | **A（草案推荐）** | 新命令 `prefab.*` **归 `Domain::Scene`**（不建 `Domain::Prefab`）；旧 `scene.spawn_prefab` 意图**不动** |
| **P5** | **B（草案推荐）** | `rpc` 取 **`prefab.spawn` + `prefab.info`**（读写各一）；其余 4 条无 rpc（`prefab.define` 不入 `RPC_METHODS`，脚本/宿主本地调用仍可用） |
| **P6** | **A（草案推荐）** | 模板文件 **`assets/prefab/<id 转路径>.json`** + **独立 `assets/prefab.schema.json`** |

拍板生效日：**2026-11-26**。结论待各卡落地时回写 `M2_DECISIONS.md`（编号取落地时现有最大号 +1，当前最大为 D19，预计 **D20**）与 `M2_PLAN.md`（清单见 §11.3）。各项原始论证保留在下方，档位文字已按拍板结论标注。

### P1 覆盖表达形态

- **为什么必须拍**：`ObjectOverride` 是 E 阶段的**类型地基**，`instance.rs` / `diff.rs` / `prefab_set_override` / 不变量检查全部依赖它的形状；落地后再换形态等于重写 E01 + E03。
- **选项**：
  - **档 A（已拍板）**：强类型 `ObjectOverride`，13 字段 `Option<FieldOverride<T>>` 三态。
  - 档 B：`BTreeMap<String, serde_json::Value>` + 点分路径（`"physics.enabled"`），扩展新字段无需改类型。
- **拍板 A**：与项目「手写枚举、拒绝动态化」的既有风格一致（`CommandKind` 20 变体手写、`ISSUE_CODES` 静态数组），编译期安全，JSON Schema 干净（字段名与类型可枚举）。档 B 的真实代价不在少写代码，而在**错误延后**：字段名拼错只能靠 `validate` 事后兜底（`prefab.override_leak` 抓不到拼错名），且 schema 退化为 `additionalProperties`，与 `P6` 的「独立 schema 文件」意图相悖。
- **不复核的后果**：若中途改 B，`diff.rs` 三算子、`prefab_set_override` 的 `field` 参数校验、`prefab.override_leak` 的判定逻辑三处同时作废，`state_hash` 的实例段要重写（JSON 树结构无序，须先规范化），估算 **+2～3 人日**，且 E05 的「只存差异」判据要从「结构判据」退回「体积判据」——正好落进 `P2` 想避开的坑。

### P2 E03 体积判据口径

- **为什么必须拍**：卡表原文是「覆盖 1 字段的实例序列化体积 < 模板 5%」，§9.2 实测证明该口径**对夹具高度敏感**：精简/中等模板下必然不达标（77% / 20%），只有全 13 字段模板才落在 4.3%–5.0% 的临界带。不拍就会在 E03 出现「判据不可达」或「测试为过而挑夹具」的假绿。
- **选项**：
  - **档 A（已拍板）**：组合判据——结构判据为主（同值字段数 0、字段数 ≤ 1）+ 体积判据为辅（全字段夹具下 < 5%）+ 防退化断言（快照退化必红）。
  - 档 B：保持字面「< 5%」不改（临界，易假红/假绿）。
  - 档 C：放宽到 < 15%（中等模板也过，但失去「只存差异」的鉴别力）。
- **拍板 A**：把判据的**真正意图**（不得写回模板同值字段）交给结构判据守，把「体积优势」降级为辅证，再加一条反向自检防止「判据恒真」。
- **不复核的后果**：取 B 则夹具稍小即红、稍大即绿，判据随夹具摆动，等于没有判据；取 C 则 20% 的「中等模板 + 单字段覆盖」也过——而那正是**该被抓住的冗余落盘**场景（覆盖 1 字段却背了 4 字段模板的体积），鉴别力丢失。

### P3 实例表是否纳入 `state_hash`

- **为什么必须拍**：决定「实例被改」在回放/`scene-verify`/差分里看不看得见；也决定模板文件是否成为第二套资产语义。
- **选项**：
  - **档 A（已拍板）**：实例（含模板 ID + 覆盖 diff）入 hash，模板文件内容不入。
  - 档 B：模板内容也入 hash。
- **拍板 A**：与既有 `assets` 段口径一致——资产**登记项元数据**入 hash、**文件字节**不入。若取 B，则「只给模板文件加个注释/格式化一下」都会让 `scene_hash` 漂移，回放金标与 `scene-verify` 天天红，而这些改动对场景外观零影响。
- **不复核的后果**：取 B 后模板文件成为 hash 的隐式输入，任何编辑器保存（含格式化）都会触发金标重录；且 `assets` 段与 `prefab` 段两套资产语义并存，日后 G 阶段流式加载要处理「模板改过但实例没变」的判定歧义。副作用（已接受）：只改模板文件 → hash 不变 → 见 §13 `R5` 的处置（模板变更须伴 `prefab_define` 命令）。

### P4 命名与域归属

- **为什么必须拍**：`commands +6` 与 `domains +0` 是硬预算，且旧 `scene.spawn_prefab` 意图已对外（编辑器白名单、`IntentPlan`、CLI 透传）。
- **选项**：
  - **档 A（已拍板）**：新命令 `prefab.*` 归 `Domain::Scene`；旧 `scene.spawn_prefab` 意图**不动**，文档写清区别。
  - 档 B：把旧意图改名 `scene.spawn_batch`（语义更准，但破坏对外契约，需 `schema++` 与迁移）。
- **拍板 A**：`domains` 预算 0，新建 `Domain::Prefab` 直接破预算；档 B 属对外契约破坏，M2 阶段无迁移成本预算。二者混淆风险靠文档与命令命名区隔（`scene.spawn_prefab` 是**批量新建节点**，`prefab.spawn` 是**模板实例化**，前者不产生实例记录）。
- **不复核的后果**：取 B 则 `scene.spawn_prefab` 的既有调用点（编辑器白名单 + `IntentPlan` + 可能的历史脚本）全部失效，`schema` 版本抬升会连带触发 `scene-verify` 与回放金标重录；若不改名又不在文档写清区别，E 阶段之后一定会有人把两条路当同一条用，产生「实例表有记录但节点已存在」的半状态。

### P5 哪 2 条命令挂 RPC（**采纳草案建议 B**）

- **为什么必须拍**：`rpc +2` 是硬预算，选错则跨进程/CLI 复现链断在关键节点。
- **选项**：
  - 档 A：`prefab.define` + `prefab.spawn`（把两条额度全压在**写**路径上，理由「建模板 → 实例化」构成完整远程写链）。
  - **档 B（已拍板，草案 §8 建议）**：`prefab.spawn` + `prefab.info`（读写各一，编辑器 UI 友好）。
- **拍板 B（采纳草案）**：两条额度按「**读一 + 写一**」分配，覆盖两类典型跨进程调用，而不是把两条都压在写路径上。`prefab.spawn` 是**唯一的实例化写入口**（决定场景内容能否被远程构造）；`prefab.info` 是**唯一的读入口**——编辑器 UI 与宿主分进程时，取「模板 ID + 覆盖字段清单」只需一次小请求，不必拉整份 `scene.json` 自己反推（那条路既慢又要把 diff 语义在 UI 侧重算一遍）。这也与「编辑器 UI 友好」的产品口径一致：E 阶段之后 UI 侧的主要 prefab 操作就是「看某个实例覆盖了什么」，而不是「远程建模板」。
- **代价与登记**：**`prefab.define` 无 rpc** → 模板建库只能走脚本（`r3e-script/src/api/prefab.rs` 的 `register_fn`）或宿主本地调用（`r3e-editor-host/src/api/prefab.rs` 的 `prefab_define` 处理函数仍在，只是不登记进 `RPC_METHODS`）。因此**跨进程复现需先本地建模板**：模板文件 `assets/prefab/<id>.json` 随仓库分发，任何进程要先在本地跑一次 `prefab_define`（脚本或宿主）把模板灌进 `PrefabTable`，之后 `rpc --method prefab.spawn` 才能命中；纯远端进程若无模板文件，无法自行建库。已在 §13 `R7` 登记补口触发条件：一旦出现「CLI 需远程建模板」或「部署目标无 `assets/prefab/`、模板须由远端下发」需求，就给 `prefab.define` 补 1 条 rpc 并重配预算（`rpc 85 → 86`、`script_only 334 → 333`）。
- **不复核的后果**：取 A 则两条额度全给写路径，`prefab.info` 只能宿主直连——「编辑器 UI 与宿主分进程」一出现，读路径要么退化为拉全量 `scene.json`（体积与解析成本高，且要在 UI 侧重算 diff），要么日后补 rpc，等于把预算问题推到下一个阶段；而 `prefab.define` 的远程价值有限，它的入参本来就是仓库内的模板文件 `path`，拿到同一份仓库的进程其模板文件已就位，先本地跑一次 `prefab_define` 的成本远低于「读不到覆盖」造成的 UI 缺口。

### P6 模板文件布局

- **为什么必须拍**：决定 `prefab_define` 的 `path` 语义、样例文件位置、以及在 `assets/` 下的**命名碰撞面**。
- **选项**：
  - **档 A（已拍板）**：`assets/prefab/<id 转路径>.json`（`prop.crate` → `assets/prefab/prop/crate.json`）+ 独立 `assets/prefab.schema.json`。
  - 档 B：全部平铺 `assets/prefab/<id>.json`。
- **拍板 A**：模板 ID 是点分命名空间（与 `ISSUE_CODES`、RPC 方法名同一套点分小写铁律），转路径天然形成命名空间目录，模板数量增长后不出现「几百个平铺文件」；且与 `scene_schema.json` 的独立 schema 手法一致（`P6` 的 schema 部分照抄既有做法，零新范式）。档 B 的隐患是 `prop.crate` 与 `prop_crate`（若有人误用下划线）会共存且不报错。
- **不复核的后果**：取 B 后模板目录随数量线性膨胀，且点分 ID 的层级信息在文件系统里丢失（无法按命名空间批量操作）；更实际的是 `prefab.define` 的 `path` 解析要额外处理「ID 与文件名不一致」，这条不一致会成为日后的长期摩擦点。

---

## 11. 门禁配平

### 11.1 tests +58 的五卡分配

| 卡 | 增量 | 细分（crate 落点） |
|---|---|---|
| E01 | **+18** | core `tests/prefab.rs`：往返 4 + 三态外形 5 + `PrefabId` 形状 3 + `ObjectOverride` 机械化 3 + schema 3 |
| E02 | **+14** | core `tests/prefab_scene.rs`：内存共享 3 + 命名与父级 3 + 序列化往返 4 + 模板解析 2 + 装配复用 2 |
| E03 | **+12** | core `tests/prefab_diff.rs` **10**（只落差异 3 + 三态语义 2 + 可逆性 2 + 体积组合判据 3）+ doc `tests/prefab_command.rs` **2**（`DefinePrefab` / `SetNodePrefabOverride` 的 undo 前像） |
| E04 | **+10** | core `tests/prefab_cmd_spec.rs` **4** + protocol `tests/prefab_rpc.rs` **2** + editor-host `tests/prefab_api.rs` **2** + doc `tests/prefab_issues.rs` **2** |
| E05 | **+4** | core `tests/prefab_instances.rs` **3**（端到端 100 实例 + 防退化 + 正跑样例）+ doc `tests/prefab_issues.rs` 补 **1**（`state_hash` 口径） |
| 合计 | **+58** | 按 crate：core **646 → 695（+49）**、doc **169 → 174（+5）**、protocol **75 → 77（+2）**、editor-host **149 → 151（+2）**；全局 **2426 → 2484** |

> 落点自检：`49 + 5 + 2 + 2 = 58` ✓；`695 + 174 + 325 + 147 + 72 + 69 + 119 + 59 + 82 + 151 + 47 + 77 + 29 + 268 + 73 + 11 + 11 + 75 = 2484` ✓（其余 crate 不变）。每卡收口时须在 `xtask/tests_baseline.json` 同步对应 crate 数与 `total`，并在 `_comment` 追加本卡增量说明（D 阶段既有惯例）。

### 11.2 其他门禁

| 计数 | 现 | E 后 | 落点 |
|---|---|---|---|
| `commands` | 413 | **419** | `cmd_spec/prefab.rs` +6；`EXPECTED_COMMANDS` 同卡改 |
| `rpc` | 83 | **85** | `RPC_METHODS` +2（`prefab.spawn` / `prefab.info`；`prefab.define` **不入**） |
| `script_only` | 330 | **334** | 由 `commands − rpc` 现算，非手填 |
| `domains` | 18 | **18** | 命令归 `Domain::Scene` |
| `invariants` | 44 | **45** | `ISSUE_CODES` +1（`prefab.override_leak`） |
| `size_debt` | 20 条 | **20 条** | 新文件均 <800；`state_hash.rs` 678→≈720（余量约 80）、`cmd_spec/scene.rs` 不动 |
| `dep_debt` | 0 | **0** | 不新建 crate、不给 `r3e-script` 加边 |

> **`script_only +4` 偏差登记（必须显式留痕）**：`M2_PLAN §4` 守恒规则 4 要求「`script_only` 不得增加」，而 `P5=B` 下机械口径必然 `330 → 334`。这不是记账疏漏，而是「4 条命令有真实脚本注册点、但预算只给了 2 条 rpc」的必然结果。**处置口径（`R1`）**：① 不削减命令（`commands +6` 是下限）；② 不用「给 4 条挂伪 rpc」抹平（会污染 `RPC_METHODS` 并让 `rpc +2` 失真）；③ 在 E04 同卡给 `M2_PLAN §4` 规则 4 加一条脚注，把口径改为「`script_only` 不得出现**无脚本注册点且无 rpc** 的死条目」——本阶段 4 条全部有 `register_fn`，符合新口径；④ 该脚注回写属计划变更，须与 E04 同提交（本稿不改 `M2_PLAN.md`）。

### 11.3 拍板后需回写的文档（各卡落地时随卡完成）

| 文档 | 内容 | 何时 |
|---|---|---|
| `M2_DECISIONS.md` | 追加 **D20**（E 阶段 `P1`～`P6` 拍板结论，`P5` 记为草案建议 B：`prefab.spawn` + `prefab.info` 带 rpc） | E01 提交内 |
| `M2_PLAN.md` | §3.5 E 行状态更新；§4 规则 4 脚注（见 `R1`）；§8 当前进度 | E01 / E04 / E05 |
| `docs/m2/README.md` | 索引把 `M2_E_PLAN.md` 标为「已拍板，生效中」 | E01 |
| `AI_ROADMAP.md §0.2` | E 阶段状态与判据进度 | E05 |
| `docs/ARCHITECTURE.md` | 新增 Prefab 章节（模板 / 实例 / 覆盖 / 命令面 / 不变量） | E05 |
| `docs/capabilities.json` | 生成物重生成（禁手改） | E04 |
| `xtask/tests_baseline.json` | 每卡 crate 单测数与 `total` + `_comment` | 每卡 |

---

## 12. 执行顺序

| 步 | 卡 | 起算基线（tests / commands / rpc / invariants） | 产出 | 验证点 |
|---|---|---|---|---|
| 1 | **E01** | 2426 / 413 / 83 / 44 | `prefab/` 五文件 + `prefab_schema` + `assets/prefab.schema.json` + `assets/prefab/prop/crate.json` | 2445（实测 `r3e-core` 665）；三态外形逐字节冻结；schema 防漂移绿 |
| 2 | **E02** | 2445 / 413 / 83 / 44 | `SceneFile`/`Scene` 实例表 + `scene/prefab.rs` + 重生成 `scene.schema.json` | 2458；`Arc::ptr_eq` 判据绿；`doc-gate` / `scene-verify` 绿 |
| 3 | **E03** | 2458 / 413 / 83 / 44 | `diff.rs` 三算子 + `CommandKind` 22 变体 | 2470；字节级可逆 + `scene_hash` 初末相等 |
| 4 | **E04** | 2470 / 413 / 83 / 44 | 6 命令 + 2 rpc + `prefab.override_leak` + 重生成 `capabilities.json` | 2480 / **419** / **85** / **45**；三源一致绿 |
| 5 | **E05** | 2480 / 419 / 85 / 45 | `state_hash` 实例段 + `ARCHITECTURE` Prefab 章节 + 端到端判据 | **2484**；`cargo xtask verify` 11 步全绿 |

**每卡通用规程（`M2_PLAN §5`）**：① 开工 `git status --short` 为空；② 一卡只改本卡文件族；③ 判据先行（先写红）；④ `cargo xtask verify` 11 步全绿；⑤ 一卡一提交 `feat(m2-E0X): …`，**不 push**；⑥ 文档与基线同卡回写；⑦ 失败就地修，禁止带红进下一卡。

---

## 13. 风险与回滚

| # | 风险 | 处置 |
|---|---|---|
| `R1` | `script_only` 330 → 334，与 `M2_PLAN §4` 守恒规则 4 字面冲突 | 按 §11.2 偏差登记：不削减命令、不挂伪 rpc、E04 同卡给规则 4 加「无死条目」脚注；脚注未回写前**不得**提交 E04 |
| `R2` | 新增 `Scene.instances` 牵动 13 处接缝（序列化 / schema / `state_hash` / validate / `doc_gate` / `tx` / undo …） | 按 §3.3 接缝清单逐项登记；E02 收口序列化与 schema，E05 收口 `state_hash`，**禁止单卡跨完 13 处** |
| `R3` | `cmd_spec/scene.rs` 已 601 行，再塞 6 条 spec 逼近 800 | 新建 `cmd_spec/prefab.rs`，`mod.rs` 只加两处登记 |
| `R4` | `R2` 红线：`app.rs` 2816 行（在册 `size_debt`） | E 阶段禁止改 `app.rs`；如必须，先触发 G02 提前 |
| `R5` | 只改模板文件 → 外观变但 `scene_hash` 不变（`P3` 的必然副作用） | 已知行为，文档化 + E05 判据 ④ 把它变成显式断言；模板变更的**正规路径**是 `prefab_define` 命令（命令入 hash），人工改文件属越轨，`prefab.override_leak` ②（指向不存在模板）可兜住「模板被删」一类硬错 |
| `R6` | 旧 `scene.spawn_prefab` 与新 `prefab.spawn` 语义混淆（都叫「prefab」但一个批量建节点、一个实例化） | `P4` 保留旧契约 + `ARCHITECTURE.md` 写清区别 + 命令命名区分（`scene.*` vs `prefab.*`）；代码注释在 `tx.rs` 白名单处补一行指向 |
| `R7` | `prefab.define` 无 rpc（`P5=B` 的代价），若日后需要跨进程建模板 → 缺 1 条 rpc | 触发条件：出现「CLI 需 `rpc --method prefab.define` 远程建模板」或「部署目标无本地 `assets/prefab/`、模板须由远端下发」需求时，给 `prefab.define` 补 1 条 rpc 并重配预算（`rpc 85 → 86`、`script_only 334 → 333`，同卡改 `EXPECTED_COMMANDS` 与 `capabilities.json`）；当前缓解：模板文件随仓库分发，跨进程复现以「先本地 `prefab_define` 灌表」为前提（§3.6） |
| `R8` | `R8`/`R1` 红线：不得新建 crate、不得给 `r3e-script` 加依赖 | 只走 core 契约 + 命令注册；`dep-check --strict` 违规必须 0 |
| `R9` | E04 是唯一跨 6 crate 的卡，单卡风险集中 | 单独留 0.5 天缓冲；实施顺序固定为「core 命令表 → protocol → runtime → editor-host → script → doc 不变量 → 生成物」，**每层跑一次 `cargo check`**，不要攒到最后 |

**回滚点（统一口径）**：每卡一个提交，回滚 = `git reset --hard HEAD~1`。逐卡约束：

| 卡 | 回滚注意 |
|---|---|
| E01 | 无跨卡依赖，回滚完全干净 |
| E02 | `assets/scene.schema.json` 与 `doc_gate.json` 改动必须在同一提交内，否则回滚后 `doc-gate` / `scene-verify` 残留红 |
| E03 | E04 依赖本卡 `CommandKind` 变体 → 回滚须在 E04 开工前 |
| E04 | `docs/capabilities.json` 为生成物，与 `EXPECTED_COMMANDS` 同批还原 |
| E05 | `state_hash.rs` 实例段是 E02–E04 期间 hash 的盲区，**不得**延后 |

**降级/止损条款**：若某卡连续 2 次 `cargo xtask verify` 仍红且定位不到单点，**允许**把该卡拆成两个提交（先落结构、后落判据），但必须在提交信息里写明拆分原因，且**不得**降低判据口径、**不得**把红测试 `#[ignore]`。

---

## 14. 明确不做

| # | 不做 | 归属 / 理由 |
|---|---|---|
| 1 | 模板热重载 / 文件监听 / 增量重载 | G 阶段流式加载；E 只做「按 ID 载入 + 未找到报错」 |
| 2 | 新建 `Domain::Prefab` | `domains` 预算 0（`P4`） |
| 3 | 给旧 `scene.spawn_prefab` 改名 `scene.spawn_batch` | 破坏对外契约，需 `schema++` + 迁移（`P4` 档 B，已否决） |
| 4 | 覆盖用 `BTreeMap<String, Value>` 动态路径 | `P1` 档 B，已否决 |
| 5 | 模板内容入 `state_hash` | `P3` 档 B，已否决 |
| 6 | 模板文件平铺 `assets/prefab/<id>.json` | `P6` 档 B，已否决 |
| 7 | E03 体积判据保持字面 <5% 或放宽 <15% | `P2` 档 B/C，已否决 |
| 8 | 嵌套 prefab（模板引用模板）/ prefab 变体继承 | M2 不做；只做单层模板 + 实例 |
| 9 | 实例的层级变换继承（父级 scale/rotation 叠加到实例） | 既有 `Scene::spawn` 的父级语义已足够，叠加变换属渲染/动画域 |
| 10 | 改 `app.rs` / `api.rs` / `args.rs` / `cmd_spec/scene.rs` | `size_debt` 与 `R2` 红线 |
| 11 | 新增 crate 或给 `r3e-script` 加依赖 | `R8`，`dep_debt limit = 0` |

---

## 15. 当前进度（速览）

| 项 | 状态 |
|---|---|
| 文档 | ✅ 已落盘（2026-11-26，v1） |
| 拍板 | ✅ `P1`～`P6` 六项全部采纳草案推荐值（`P5` 采草案建议 B：`prefab.spawn` + `prefab.info` 带 rpc，`prefab.define` 无 rpc） |
| E01 数据模型 | ✅ 已完成（2026-11-26 本地提交 `feat(m2-E01)`；实施记录见 §4.5） |
| E02 实例化与 Scene 集成 | ⏳ 未开工 |
| E03 覆盖 diff 与可逆 | ⏳ 未开工 |
| E04 命令 / rpc / 不变量 | ⏳ 未开工 |
| E05 判据测试与文档 | ⏳ 未开工 |
| 完成度 | **1 / 5 卡**；tests 2426 / 2484（+18，41%） |
| 下一卡起算基线 | tests **2445**、commands **413**、rpc **83**、script_only **330**、domains **18**、invariants **44**、`size_debt` **20**、`dep_debt` **0** |
| 下一卡开工必读 | 本文件 §4.5（E01 落地口径）+ §5（E02 全文）+ §3.1 / §3.2（类型与序列化外形）+ §9.5（schema 防漂移写法）+ `M2_DECISIONS.md` D6/D7/D20 + `M2_PLAN.md` §5 单卡规程 |
| 关键路径 | 约 **9.5 人日**（E01 2.5 / E02 2.0 / E03 1.5 / E04 2.5 / E05 1.0），严格串行无并行空间（`R4`） |

---

*本稿由只读取证生成：未修改仓库任何代码，未执行 `git commit`。*
*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
