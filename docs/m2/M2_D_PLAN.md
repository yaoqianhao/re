---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_dcc02578e80b11f1a03c525400638852
    ReservedCode1: zoCOnQ3GIrrli8KP8EXrNvlb8q0WpDKHjh6vuapZH2OiZQhUvdZvQEdrrQ+5HEZmdvIj00dblaidC6U5mRhR1765xu+quRLhBF9AUthKIlY+AkdzmD4r2QxUYiWBl+isaixwnTrZjYDkUqgLWWZ7KxkRNqKjHcryRV7W1XU/+G4gbMM48w1l+/PrzPs=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_dcc02578e80b11f1a03c525400638852
    ReservedCode2: zoCOnQ3GIrrli8KP8EXrNvlb8q0WpDKHjh6vuapZH2OiZQhUvdZvQEdrrQ+5HEZmdvIj00dblaidC6U5mRhR1765xu+quRLhBF9AUthKIlY+AkdzmD4r2QxUYiWBl+isaixwnTrZjYDkUqgLWWZ7KxkRNqKjHcryRV7W1XU/+G4gbMM48w1l+/PrzPs=
---

# M2 D 阶段方案：NavMesh 烘焙 + A* 寻路（D01–D06）

- 文档版本：v2（2026-11-24 落盘；同日拍板后订正，见 §10「拍板结论」与 §11.3）
- 状态：**已拍板**（2026-11-24，`DD-1`～`DD-6` **全选 A 档**；结论落 `M2_DECISIONS.md` D17–D19，按 §12 顺序逐卡落地）
- 上位约束：`M2_PLAN.md` §3.4 卡表 / §4 门禁累加预算 / §5 单卡执行规程 / §6 文件占用矩阵 / §7 风险 `R7`/`R8`
- 决策前置：`M2_DECISIONS.md` **D4=A**（编辑期预烘焙，产物随场景缓存，运行期只加载、G04 不给烘焙预算）、**D5=默认 + 可配置**（20 组 / seed 42 / 1.6×）、**D14/D15/D16**（C 阶段已落：`rpc` 预算、`r3e-terrain` 纯派生层、地形查询独立通道）
- 本稿范围：**只出方案**。不改一行引擎代码；§11 的配平已在 2026-11-24「拍板提交」中一次性回写 `M2_PLAN.md` / `README.md` / `M2_DECISIONS.md`（逐项状态见 §11.3）。
- 数字口径：全部取自 2026-11-24 实测（采集命令见各节），`tests` 只认 `cargo xtask check-tests`，能力清单只认 `cargo xtask gen-capabilities --check`。

---

## 0. 一句话

D 阶段不是「从零写 NavMesh 与 A*」——`crates/r3e-nav` 已有 1691 行生产代码（体素烘焙 / `NavGrid` / A\* / 四态失败码）与 37 条测试，**真正缺的是四件对外可验证的东西**：① 烘焙产物自身携带**坡度 / 台阶分类**与**可复现哈希**（现在只有「这一格站得住」的位图，坡度是遍历期的边属性）；② **双后端烘焙一致性的判据与留证方式**（现在 `bake` 一次都没碰过 `r3e-physics`，尽管 allow 表早已放开）；③ **路径平滑与代理沿路行走**的判据（现在没有漏斗、没有 string-pulling，路径就是格中心折线）；④ **7 条 `nav.*` 协议面 + 2 条不变量**，把「编辑器能烘、CLI 能复现、脚本能调」打通。

---

## 1. 交付映射

| 卡 | 目标 | 主落点 | 卡表判据（`M2_PLAN` §3.4） | D 阶段实际增量 | 门禁增量 |
|---|---|---|---|---|---|
| **D01** | 导航网格烘焙（D4=A 编辑期预烘焙） | `r3e-nav/src/{bake,grid,product}.rs` | 由地形与碰撞几何烘焙出可行走多边形；斜坡/台阶按坡度与高度分类；**双后端各烘一次，产物哈希须一致** | 逐格 `CellClass`（Flat/Ramp/Step/Blocked）+ `NavProduct`（含 `input_digest`/`product_hash`）+ 地形高度改走 `core::TerrainPatch::sample_height`（删私有插值）+ `bake_with_field` 双后端入口 | tests **+26** |
| **D02** | A\* 寻路 | `r3e-nav/src/path.rs` | 给定起终点返回可通行路径；不可达返回明确失败码而非空路径 | 已有 A\* 不重写：接分类图（斜坡代价）、失败码细分到可诊断子类、tie-break 固定保证同输入同路径、200×200 场景展开上界实测 | tests **+16** |
| **D03** | 路径平滑与代理移动 | `r3e-nav/src/{smooth,agent}.rs` | 路径经过漏斗算法平滑；代理沿路径行走不切角穿墙 | 新增 string-pulling（视线剪枝复用既有 `grid.line_walkable`）+ 拐角按 `agent_radius` 内缩 + `NavAgent` 路径跟随（不依赖 `r3e-character`） | tests **+16** |
| **D04** | 命令与能力登记 | `r3e-core/src/cmd_spec/meta.rs`、`r3e-runtime/src/nav_ops.rs`、`r3e-editor-host/src/api/nav.rs`、`capabilities.json` | `nav.*` 命令 + rpc 入口；`unsupported` 不残留旧条目 | 新增 7 条 `nav.*`（全带 rpc）；命令面实现落 `r3e-runtime::nav_ops`（宿主共用，与 C06 `terrain_ops` 同址）；**不建 `Domain::Nav`**（`domains` 预算 0，见 `DD-4`） | tests **+8**、commands **+7**、rpc **+7**、invariants **+2** |
| **D05** | 判据测试（D5 默认 + 可配置） | `r3e-nav/tests/` | 路径长度 ≤ 直线 **1.6 倍**自动断言（20 组起终点 / seed 42）；组数、seed、阈值抽为可覆盖参数；非法配置（0 组 / 阈值 ≤0）必须被拒；不可达有反例 | `PathScale { cases, seed, limit }` 可覆盖结构 + 判据脚本化；20 组固定用例表入测试夹具 | tests **+10** |
| **D06** | 可玩性回归 3 | `r3e-nav/tests/`、`r3e-runtime` | 地形上放一个跟随目标，代理自行绕障抵达 | headless 集成回归：代理逐帧跟随目标 → 抵达、不穿墙、帧数上限、双后端终点差 <0.5 m；附 CLI 一键复现命令 | tests **+6** |
| 合计 | | | | | tests **+82**、commands **+7**、rpc **+7**、invariants **+2**、domains **0** |

---

## 2. 开工实测真值（2026-11-24）

### 2.1 门禁现状

| 门禁 | 实测 | 采集方式 |
|---|---|---|
| `tests` | **2338**（`r3e-nav` **37**：`grid.rs` 内嵌 8 + `tests/bake.rs` 19 + `tests/path.rs` 10） | `cargo xtask check-tests` |
| `commands` | **406** | `cargo xtask gen-capabilities --check` |
| `rpc` | **76** | 同上 |
| `script_only` | **330**（406 − 76） | 同上 |
| `domains` | **18**（含 `terrain`，无 `nav`） | 同上 |
| `invariants` | **42**（`nav.*` 已占 4） | 同上 |
| `dep_debt` | **0**（limit 0） | `cargo xtask dep-check --strict` |
| `size_debt` | **20** 条（limit **800**；20 条全部 >800，`r3e-nav` 无一上榜） | `xtask/size_debt.json` |
| 工作区 | 干净；`HEAD = 4ea213d` | `git status --short` |

### 2.2 `r3e-nav` 存量清单

| 文件 | 行数 | 内容 |
|---|---|---|
| `src/bake.rs` | 312 | `bake(scene, cfg)`：`plan` 定网格 → `collect_obstacles`（节点 AABB，带三条豁免：低于 `step_height` / 高于 `agent_height` / 带 `ignore_tags`）→ `inflate_xz(agent_radius)` → `stamp_obstacle` → `bake_heights` → `flood_regions`；地形高度走**私有** `sample_ground_opt` + `patch_height`（双线性，多块取最高） |
| `src/grid.rs` | 546 | `NavGrid { cfg, dims, min, heights: Vec<f32>, walkable: bitset, region: Vec<u32>, region_cells, unresolved }`；常量 `NAV_MAX_SIDE = 2048` / `NAV_MAX_CELLS = 4_000_000`；`can_step`（`grid.rs:377-384`：`slope_limit = dist · tan(max_slope_deg)`，`slope_ok || step_ok`）/ `line_walkable` / `cell_of` / 区域统计 |
| `src/lib.rs` | 411 | `NavConfig`（`cell 0.5` / `radius 0.4` / `height 1.8` / `step 0.4` / `max_slope 45°` / `bounds` / `ignore_tags`，`sanitized` 收敛非法值）+ `Nav::{bake_scene, bake_scene_with, path, reachable, nearest_walkable, region_of, ground_height, is_walkable, disconnect_point, check_reach, spawn_point*}` + `NavReport::walkable_ratio / floating_spawns` |
| `src/path.rs` | 422 | A\*（8 邻、对角不切角）、`PathOutcome::{Found, Unreachable, InvalidEndpoint, BudgetExceeded}` + `describe()`、`NAV_PATH_MAX_EXPANSIONS = 400_000` |
| `tests/bake.rs` / `tests/path.rs` | 395 / 206 | 19 + 10 条 |
| `Cargo.toml` | — | `r3e-core` / **`r3e-physics`** / `glam` / `log` |

> **关键实测**：`r3e-physics` 在本 crate **零调用**（`grep -r "r3e_physics" crates/r3e-nav/src` 无命中）。allow 表注释写着「physics = 碰撞形状语义」，但 M1 零到一阶段没接上——D01 正好是兑现这条依赖的那张卡。

### 2.3 现状能力自检六问

| 问 | 答 | 证据 | D 阶段处置 |
|---|---|---|---|
| 能烘吗 | **能**，但只到「站得住」 | `bake.rs` 312 行产出 `heights` + `walkable` bitset + `region` | 保留，补分类与哈希 |
| 能按坡度/台阶分类吗 | **不能** | `grid.rs:40` 注释原文：「可走位图：bit = 1 表示这一格站得住（**不含坡度/步高约束，那是边属性**）」；`max_slope_deg` 只在 `can_step` 的**边判定**里使用 | **D01 新增逐格 `CellClass`**，且分类与 `can_step` 共用同一判定函数（单点真值，防两套语义漂移） |
| 能找路吗 | **能** | A\* 8 邻 + 对角不切角 + 四态码 + 展开上限 400k + 10 条测试 | **不重写**，D02 只做分类图接入 / 失败码细分 / 确定性 tie-break |
| 能平滑吗 | **不能** | 无漏斗、无 string-pulling；`Found(points)` 是格中心折线，点数 = 步数 | D03 新增 |
| 能缓存产物吗 | **不能** | 无序列化、无 `input_digest`、无 `product_hash`；`Nav` 只活在 `LocalEngineApi::nav` 槽位（`RefCell<Option<Nav>>`），进程退出即丢 | **D01 新增 `NavProduct`**，D04 给 `nav.export/import/hash` |
| 能双后端各烘一次吗 | **谈不上** | `bake` 只读 `Scene`（地形 patch + 节点 AABB），完全不碰 physics | **D01 核心新增**，见 §3.3 |

### 2.4 地形真值链（D 阶段必须遵守）

```
core::TerrainPatch            ← 唯一真值（C01 上移，terrain.rs 282 行；sample_height 在 :117）
        │  HeightField::from_patches（派生，不新增真值）
        ▼
physics::heightfield::HeightField  ← 派生层：height_at / normal_at / sample / peak_over / covers
        │  门面共用一份合成（port_terrain.rs 择优 + terrain_query.rs merge_ground）
        ├──► builtin 后端：只存数据
        └──► rapier 后端：建静态 HeightField3d（不进 records，body_count 与能力位不变）
        ▼
nav::NavGrid.heights          ← D 阶段派生（只许从上一层的值算，不许自己插值）
```

- 现状违规点：`bake.rs:278 patch_height` 是 nav 内**自写的一份双线性插值**，与 `core::TerrainPatch::sample_height` 重复。C 阶段 `CD-3` 明确「本轮不动、以 C01 的 20 点交叉断言锁等价」，**D01 是这条欠账的到期日**。
- D01 红线：删 `patch_height`，`sample_ground_opt` 只做「多块取最高」的挑选，插值一律调 `TerrainPatch::sample_height`。行为等价由 C01 的 20 点断言 + D01 新增 32 点断言（含块边界两侧各 16 点）双锁。

### 2.5 协议面与宿主现状

| 项 | 现状 | 备注 |
|---|---|---|
| `nav.bake` / `nav.path` / `nav.reachability` | 已登记，**挂在 `Meta` 域**，`since M1-S4-D`，三条均带 rpc；`nav.bake`/`nav.path` 为 `Heavy`、`nav.reachability` 为 `Linear` | `cmd_spec/meta.rs` 注释明确「未烘焙回 `NAV_NOT_BAKED` 而非空表」 |
| 宿主实现 | `r3e-editor-host/src/api/nav.rs` 187 行：`nav_bake` / `nav_path`（四态透出 `found/outcome/reason/points/count`）/ `nav_reachability`（`reachable/reason/fromRegion/toRegion/disconnectPoint`）；写 `LocalEngineApi::nav` 槽位，**不写场景、不改 `state_hash`** | 丢弃 `Nav` 即丢弃产物——无持久化 |
| CLI | `r3e-cli` 已支持 `rpc nav.bake` 透传 | `args.rs` **783 行 / 限 800，余量 17 行** → D 阶段**不得**新增子命令解析 |
| 不变量 | `nav.spawn_offmesh`(Error) / `nav.target_offmesh`(Error) / `nav.no_path`(Error) / `nav.region_split`(Warn)，`layer = "nav"` | D 阶段只能再加 2 条（预算 +2） |
| 命令归属 | `Domain` 枚举**无 `Nav`**；13 个域模块无 `nav.rs` | `domains` 预算 D 阶段 **0** → 继续挂 `Meta` |

### 2.6 硬约束（跨卡生效）

| 约束 | 实测值 | 对 D 阶段的要求 |
|---|---|---|
| `dep_rules: r3e-nav` | `allow = [r3e-core, r3e-physics]`；**不得**依赖 render / runtime / doc；**也不含 `r3e-terrain`** | nav 不许为省事去调 `r3e-terrain::sample`；地形真值只走 core，双后端只走 physics |
| `dep_rules: r3e-runtime` | allow 含 `r3e-nav` / `r3e-terrain` | 命令面共享实现落 `r3e-runtime::nav_ops`，`editor-host` 薄转发（与 C06 `terrain_ops` 同址） |
| `dep_rules: r3e-script` | allow 仅 `core`/`doc`/`protocol` | 脚本侧经宿主注入，**不给 script 加 nav 边**（否则 `dep_debt limit = 0` 当场红） |
| `size_debt` | limit **800**，20 条在册（`r3e-nav` 无） | 新增文件同样受 800 限制；≥800 即入债，等于当场破坏「只降不升」 |
| `args.rs` | **783** / 800 | 新增命令一律 `rpc --method nav.xxx`，**不动** `args.rs` |
| 协议守恒 | 规则 4：`script_only` 不得增加（现 330） | 新增 7 条命令 → 必须 7 条全带 rpc，见 `DD-4` |
| 预算下限 | D 行：tests +82 / commands +7 / rpc +2 / invariants +2 / domains 0 | `commands +7` 是**下限**：少加即全局 433 落空 |

---

## 3. 总体设计

### 3.1 导航形态（`DD-1`）

三条路，成本差 3 倍以上，必须先拍：

| 档 | 形态 | 代码量估算 | 判据可达性 |
|---|---|---|---|
| **A（推荐）** | 体素网格 + string-pulling。`NavGrid` 保持位图/格高/区域，新增逐格分类；路径平滑用「视线剪枝 + 拐角内缩」而非严格漏斗 | 新增约 **700–900 行**（`product.rs` + `smooth.rs` + `agent.rs`） | 1.6× 可稳定达成（经验值 1.05–1.25×）；「不切角」由 `line_walkable` + 半径内缩直接可证 |
| B | 真多边形 NavMesh：体素 → 轮廓追踪 → 凸多边形分解 → 多边形邻接图 → 严格漏斗 | 新增约 **1800–2400 行**，且按 800 行/文件需再拆 4–5 个文件；`grid.rs` 语义要并存或整体替换 | 严格漏斗点更少，但「双后端哈希一致」在多边形化阶段会被浮点顺序彻底放大，红卡概率高 |
| C | A + 后置多边形化（只用于平滑，不换产物格式） | 约 **1200 行** | 平滑质量接近 B，但仍要背多边形分解的不确定性 |

- 推荐 **A**。理由：① D05 判据是「≤1.6× 直线」，A 的余量足够，B 的额外精度买不到任何判据；② B 与「双后端产物哈希位相等」（`DD-2` 档 A）结构性冲突——多边形化的顶点合并顺序对浮点误差极敏感，会把一条「构造保证」的判据变成「每天都可能红的判据」；③ `size_debt` 800 行/文件下，B 需要再造 5 个文件，D 阶段 6 卡的节奏撑不住。
- **卡表措辞**：`M2_PLAN` §3.4 D01 写的是「烘焙出可行走多边形」。选 A 则需在拍板提交里把该词改为「可行走**区域**（格级）」，否则实现交付与计划字面不一致——这正是 `CD-1` 那类「口径回写」的处理方式。

### 3.2 烘焙流水线（D01 设计）

```
Scene ──┬─ terrain patches ──► core::TerrainPatch::sample_height ─┐
        │                                                          ├─► NavGrid.heights
        └─ nodes ──► AABB（3 条豁免）──► inflate(radius) ──► stamp ──┘
                                                                   │
                                    ┌──────────────────────────────┘
                                    ▼
                       classify（逐格 CellClass，与 can_step 共用判定）
                                    ▼
                            flood_regions（连通块）
                                    ▼
        NavProduct { format, cfg_digest, input_digest, dims, heights, walkable, class, region } ──► product_hash()
```

逐格分类规则（**与 `can_step` 共用同一个 `step_verdict(rise, dist, cfg)` 函数**，避免两套）：

| 类 | 判定 | 语义 |
|---|---|---|
| `Blocked` | 障碍 stamp 命中，或四邻高差全部超限 | 不可进入 |
| `Step` | 存在邻格高差 `rise ≤ step_height` 且 `rise > slope_limit` | 台阶，可跨、代价 ×1.0 |
| `Ramp` | 存在邻格 `slope_limit` 内可通，且该格自身四邻高差非零 | 斜坡，代价 ×(1 + slope/45° · 0.5)（D02 用） |
| `Flat` | 四邻高差 < 1e-4 | 平地 |

`CellClass` 用 `u8` 存（4 类 + 保留），16 万格 = 160 KB，与 `heights` 同量级，不触发内存问题。

### 3.3 双后端一致性的真实含义（`DD-2`，本方案最关键的一节）

**先说事实**：C04 依 `D16` 已把地形查询收敛成「只写一份、后端只存数据」——`physics::terrain_query` 与 `heightfield.rs` 里两套后端共用同一份场数据与同一套合成路径，`tests/terrain_field.rs` 已循环断言**逐位一致**（C04 的 +26 中占 8 组）。

因此「双后端各烘一次、产物哈希须一致」**不可能也不应该**是「写两份独立烘焙实现互相比对」：

- 那样会把 `R7`（双后端工作量翻倍）原封不动搬进导航层，且必然因浮点差异红卡——C04 之所以能一致，是因为**没有第二份实现**。
- 卡表这句话的可验证含义应该是：**烘焙不认后端，后端不污染烘焙**。

三档：

| 档 | 口径 | 判据 | 成本 |
|---|---|---|---|
| **A（推荐）** | **构造性一致 + 回归锁**：`bake_with_field(scene, &HeightField, cfg)` 是唯一烘焙入口；`HeightField` 由 `core` patches 构造，分别经 builtin / rapier 两后端 `set_heightfield` → 取回 → 各烘一次 | `product_hash` **位相等** + 逐格 `heights` 位相等 + 逐格 `class` 相等；相差即红并 dump 首个差异格 | 桥接约 **60–80 行**（`physics` 已是 allow 内依赖，此前零使用） |
| B | 独立实现对照：builtin 走解析插值、rapier 走 `HeightField3d` raycast，比「可走格差异 ≤ 0.2%」 | 带容差，永不位相等 | 桥接 **200–300 行**，且两套采样语义长期并存 → `R7` 复现 |
| C | 只烘 builtin，rapier 千分之一抽样比对 | 无法满足卡表「双后端各烘一次」 | 最省，但等于放弃判据（须回写卡表） |

- 推荐 **A**，并把它写进卡备注：**「一致」是构造保证，测试锁的是「切后端不改 field 内容 + 烘焙完全确定」这两件事**——这才是能长期防回归的资产。若有人日后在后端里塞私货（比如 rapier 侧加噪声抑制），A 档立刻红，这正是我们想要的红。
- 无论哪档，红线不变：`nav` 不许自创地形真值（§2.4 真值链）。

### 3.4 产物哈希与缓存（`DD-3`）

`NavProduct` 两个摘要，职责不同、不可合并：

| 字段 | 内容 | 用途 |
|---|---|---|
| `cfg_digest` | `NavConfig` 七字段 + `bounds` + `ignore_tags`（排序后）→ SplitMix64 | 判「参数变了要重烘」 |
| `input_digest` | 地形 patch 摘要（name/origin/size/样本数 + 样本哈希）+ 障碍 AABB 列表（按 name 排序）+ 未解析资产名 | 判「场景变了要重烘」（`D4=A` 的「输入哈希」） |
| `product_hash` | 上述两摘要 + 格式版本 + dims/min/cell + `heights`/`walkable`/`class`/`region` 全量折叠 | 双后端一致性判据（`DD-2`）+ 缓存命中校验 |

- 哈希算法：SplitMix64 折叠（`h = mix(h ^ x)` 逐 `to_bits()` 送入），与 `r3e-terrain::generate` 同族参数但**不共享代码**——`dep_rules` 静态锁死 nav 不得依赖 terrain；两边各自实现是边界要求，不是复制粘贴的偷懒。输出固定 16 位小写 hex。
- 序列化（档 A）：二进制 `.navmesh`，头 `magic "R3ENAV\0" + u16 format + u32 header_len + header(JSON: cfg/digests/统计)`，体部按 `heights(f32 LE) → walkable(bitset) → class(u8) → region(u32)` 定长排列；小端、无对齐填充、`#[repr(C)]` 不参与。读入时校验 `product_hash` 与头中记录一致，不一致报 `NavError::ProductMismatch`（新增变体）。
- 明确**不进** `state_hash`：`nav` 是烘焙产物，写进场景会让「同一场景两次打开」哈希漂移（C 阶段 `CD-6` 已定「场景数据才进 `state_hash`」）。

### 3.5 A\* 契约（D02，不重写）

| 项 | 现状 | D02 增量 |
|---|---|---|
| 邻接 | 8 邻，对角不切角 | 保留 |
| 代价 | 均匀 | 接 `CellClass`：`Ramp` 按 `slope/45°` 上浮（系数见 §3.2），`Step` 加固定 0.15 惩罚（避免无谓跨台阶） |
| 失败码 | `Unreachable` / `InvalidEndpoint` / `BudgetExceeded` + `describe()` | `InvalidEndpoint` 细分 `OffMesh` / `OutOfBounds` / `NonFinite` 三个可诊断子类（`describe()` 文案随之固定，作为契约） |
| 确定性 | 依赖容器遍历顺序 | 开放表按 `(f, 格索引)` 双键排序，**同输入同路径**可断言 |
| 上界 | 400k 展开 | 200×200 m / 0.5 m = **16 万格**，最坏全展开 0.25× 上限；D02 实测最坏场景展开数并写入卡备注 |

### 3.6 平滑与代理移动（D03）

- **string-pulling**（档 A 口径下的「漏斗」）：从起点向前找最远可见点（`grid.line_walkable` 已是现成视线检验），逐段剪枝；输出点数应 ≤ 原始折线的 40%（判据取「严格减少」而非固定比例，防夹具敏感）。
- **拐角内缩**：保留拐点时把顶点沿两段夹角的角平分线退 `agent_radius + 0.05` 米，且退点本身必须 `is_walkable`；退不进去则保留原格中心点（宁钝不穿）。
- **`NavAgent`**：`{ pos, path, seg, }` 上实现 `advance(dt, speed) -> AgentStep`：投影到当前段 → 到点切换 → `y` 跟 `grid.ground_height` → 返回 `{ arrived, off_path, progressed }`。**不依赖 `r3e-character`**（allow 表没有、也不该有）：D 阶段只要求「沿路不切角」，真实角色驱动留给 `H03`（NPC 追击）用 `r3e-character` 拼装。

### 3.7 协议面 7 条（D04，`DD-4`）

| 命令 | 入参要点 | 出参要点 | rpc | 备注 |
|---|---|---|---|---|
| `nav.smooth` | `points` 或 `from`/`to` | `points` / `count` / `length` | ✔ | 平滑后的折线 + 长度，便于前端直接画 |
| `nav.probe` | `point` | `walkable` / `class` / `region` / `ground` | ✔ | 单点探针，调试烘焙必备 |
| `nav.nearest` | `point` / `maxRadius` | `point` / `distance` / `region` | ✔ | 复用既有 `nearest_walkable` |
| `nav.hash` | — | `productHash` / `inputDigest` / `cfgDigest` / `cells` | ✔ | 缓存命中的判据出口 |
| `nav.export` | `path` | `bytes` / `productHash` | ✔ | 写 `.navmesh`（`DD-3` 档 A） |
| `nav.import` | `path` | `loaded` / `productHash` / `cells` | ✔ | 运行期只加载（`D4=A`） |
| `nav.stats` | — | `cells` / `walkableRatio` / `regions` / `classHistogram` / `unresolved` | ✔ | 取代「只能靠 `NavReport::summary()` 打日志」 |
| （保留）`nav.bake` / `nav.path` / `nav.reachability` | 已在，D04 只扩出参（`nav.bake` 加 `productHash`/`classHistogram`，`nav.path` 加 `smooth`/`length`） | | ✔ | 出参扩展不算新增命令 |

- **不建 `Domain::Nav`**：`domains` 预算 D 阶段为 **0**，全局 19 靠 `H01` 的 `game` 撑；建域会立刻顶穿（18 → 19，H 阶段再加 `game` → 20）。继续挂 `Meta`，在 `meta.rs` 里加分组注释把 10 条 `nav.*` 归成一段。
- 落点：命令表 `cmd_spec/meta.rs`；共享实现 `r3e-runtime/src/nav_ops.rs`（约 360 行，7 个 `pub(crate) fn`）；`editor-host/api/nav.rs` 187 → ≤430 行做薄转发；`capabilities.json` 由 `gen-capabilities` 重生成（禁手改）；`TX_ONLY_METHODS` **不动**（7 条全带 rpc）。

### 3.8 两条不变量（`DD-5`）

| code | layer | severity | 判什么 | 反例夹具 |
|---|---|---|---|---|
| `nav.bake.empty` | `nav` | Error | `walkable_ratio == 0`（或 < 0.1% 的退化阈值） | 全场陡壁 / 障碍铺满 → 烘出空网格仍被当成「烘好了」 |
| `nav.backend.mismatch` | `nav` | Error | 同一场景 builtin 与 rapier 烘焙 `product_hash` 不等 | 侧向比对两只夹具（`DD-2` 档 A 下应为恒真，红即证明后端被污染） |

- 三源一致（计划 §4 守恒规则 3）：登记表 / 反例测试 / 能力清单各 1。`layer = "nav"` 已存在，无需新增层或 `Category`。
- 为什么不选「路径过长」做不变量：长度取决于夹具几何，属**判据测试**范畴（D05），做成场景不变量会产生大量假红。

### 3.9 文件与行数预算（≤800/文件，`size_debt` 不得新增）

| 文件 | 现状 | D 末预算 | 说明 |
|---|---|---|---|
| `r3e-nav/src/bake.rs` | 312 | ≤ 470 | 删 `patch_height`、接 `bake_with_field`、写分类 |
| `r3e-nav/src/grid.rs` | 546 | ≤ 700 | 存 `class: Vec<u8>` + 访问器 + `step_verdict` 抽出共用 |
| `r3e-nav/src/lib.rs` | 411 | ≤ 520 | `Nav::product()` / `bake_with_field` / `NavError::ProductMismatch` |
| `r3e-nav/src/path.rs` | 422 | ≤ 560 | 代价函数、失败码细分、tie-break |
| `r3e-nav/src/product.rs` | — | ≤ 420 | 分类枚举、摘要、SplitMix64、序列化/反序列化 |
| `r3e-nav/src/smooth.rs` | — | ≤ 300 | string-pulling + 拐角内缩 |
| `r3e-nav/src/agent.rs` | — | ≤ 260 | `NavAgent` 路径跟随 |
| `r3e-nav/tests/{bake,path,product,smooth,agent,scale}.rs` | 395 / 206 | ≤ 700 / 380 / 260 / 260 / 240 / 220 | 37 → 119 条 |
| `r3e-runtime/src/nav_ops.rs` | — | ≤ 360 | 7 条命令共享实现 |
| `r3e-editor-host/src/api/nav.rs` | 187 | ≤ 430 | 10 条命令薄转发 |
| `r3e-core/src/cmd_spec/meta.rs` | — | 现状 + ≈ 95 | 7 条 `CommandSpec` + 分组注释 |
| `r3e-doc/src/validate/invariants.rs` | 337 | ≤ 400 | +2 条 `IssueCode` + 判定 |
| `xtask/tests_baseline.json` / `docs/capabilities.json` | — | 生成物 | 随卡更新，禁手改 |

> **预算订正（2026-11-25，D02 实测）**：`tests/path.rs` 206 → **688** 行，**超出本表原预算 380**。原因是 D02 的 16 条新测试自带三套夹具（200 m × 200 m 蛇形长廊、`ledge_field` 高度场矮坎、坡/坎对照场景）与逐段采样式邻接校验，夹具代码量已超过断言本身。**未抬 `size_debt`**（688 < 800 硬线，`lint-size` 绿），与 `tests/seams.rs`（606 行）同口径处理——测试文件按 800 行硬线管，本表的 380 是「预期量」而非门禁值。若 D03 还要往 `tests/path.rs` 加内容，先按 800 检查余量（112 行），必要时拆 `tests/path_cost.rs`；**不得**为此新增债务条目。

---

## 4. D01 导航网格烘焙

> **状态：已完成**（2026-11-25，本地提交未 push；遗留收口同轮补齐）。实测 `tests` 2338 → **2370**（`r3e-nav` 37 → **69**，**+32**＝原预算 26 + 接缝夹具收口 6），`verify` 11 步全绿；其余门禁全部持平——`commands` 406 / `rpc` 76 / `script_only` 330 / `domains` 18 / `invariants` 42 / `size_debt` 20 / `dep_debt` 0 / 回放金标 `385482eb033d2964` 未漂移。落地口径、容差与两处判据订正见 **§4.1**；两项遗留（`D18` 文面、12 块接缝夹具）已在同卡内销项闭合，见 §4.1 三.3 / 四.1。

- **目标**：一次烘焙产出**自描述**的 `NavProduct`——格高、可走位图、逐格分类、连通块、两级输入摘要与产物哈希；地形高度不再由 nav 自造。
- **判据**（全部可自动断言）：
  1. 同一场景烘两次 → `product_hash` 相同（确定性）。
  2. 改动地形任一样本或任一 `NavConfig` 字段 → `input_digest` / `cfg_digest` 变化，`product_hash` 随之变化（摘要确实覆盖了输入）。
  3. 45° 缓坡逐格 `class == Ramp`；50° 陡坡逐格 `class == Blocked`；0.3 m 台阶 `class == Step`、0.5 m 台阶 `Blocked`（默认 `step_height = 0.4`）。**口径已订正，见 §4.1 三.1**：`Ramp` 统一为 `Slope`；陡坡与超高台阶不再记 `Blocked`（改由 `can_step` / 可走位图区分，`Blocked` 只留给障碍与地形外）。
  4. **双后端各烘一次 → `product_hash` 位相等**，且 `heights` 逐格位相等、`class` 逐格相等（`DD-2` 档 A）。
  5. 删除 `bake.rs::patch_height` 后，C01 的 20 点交叉断言 + 新增 32 点断言全绿（含块边界两侧各 16 点）。
  6. 块边界处相邻格高差不产生「假台阶」：12 块拼接场景下，跨块接缝处 `Blocked` 格数为 0。**✅ 已落地**：`crates/r3e-nav/tests/seams.rs` 6 条（平地 / 缓坡 / 矮坎 / 超高台阶 / 逐格对账 / 确定性），见 §4.1 四.1。
- **涉及文件**：`bake.rs`、`grid.rs`、`lib.rs`、新增 `product.rs`、`tests/bake.rs`、`tests/product.rs`、**新增 `tests/seams.rs`**、`xtask/tests_baseline.json`。
- **依赖前置**：C07 完成（已满足，`4ea213d` 之后）；`core::TerrainPatch::sample_height` 可用（已满足）；`physics::{HeightField, Backend::set_heightfield}` 可用（C04 已满足）。
- **风险与回退**：最大风险是「删私有插值 → 行为微变」导致 C 阶段既有断言红。处置：先加 32 点断言**证明新旧等价**（红则说明旧实现有 bug，按 R7「不糊」原则记录进卡备注），再删；仍不绿则 `git checkout -- .` 回滚本卡重来，不带红进 D02。

### 4.1 D01 实施记录（2026-11-25）

**一、落地口径（五条，后续各卡按此读代码）**

1. **唯一烘焙入口吃 `HeightField`**：`Nav::bake_with_field(&scene, &field, cfg)` 是唯一真正烘焙的路径；`bake` / `bake_scene` / `bake_with` 都先经 `bake::field_of` 造场再落到这里，`Nav::bake_with_backend` 走「场景 → 场 → `set_heightfield` → 取回 → `bake_with_field`」。后端只搬运场、不参与烘焙，所以「两后端产物位相等」是**构造性结论**，不是两条独立算法碰巧算得一样。后端取不回自己的场时报 `NavError::BackendMismatch`，**不**静默退回本地场（`null_backend_reports_mismatch_instead_of_falling_back`）。
2. **范围与格长由场决定，`cfg` 只提供判定参数**：`bake_with_field` 的 `dims` / `min` / `cell_size` 全部取自场，`cfg.bounds` 与 `cfg.cell_size` 传进来也不生效（`bake_with_field_ignores_cfg_bounds`、`field_cell_size_overrides_cfg_cell_size`）。否则「装进去的场」和「烘出来的网格」可以描述两个不同的世界。极小范围下 `field_of` 把场补到至少 2×2，补出来的那一圈如实记为「没有地面」而不是「高度 0 就能走」（`tiny_field_pads_to_two_rows_and_keeps_coverage_honest`）。
3. **逐格 `CellClass` 与 `can_step` 共用 `step_verdict`**：`NavGrid::classify` 只看 +X / +Z 两条正邻边（每条无向边只被数一次），判据就是 `grid::step_verdict`，与 `can_step` 同一个函数。两处各写一遍迟早分叉，症状是「地图标着能走、A\* 说迈不过去」，且只在特定斜坡角度上复现。
4. **删私有插值，真值链锁死为 `TerrainPatch::sample_height` → `HeightField` → `NavGrid.heights`**：`bake.rs` 不再自带双线性插值（旧私有 `patch_height` 已删，全仓仅剩注释提及），地形采样只有 `r3e-core` 一份实现；`field_of` 把场的采样点对齐到格心，于是「第 `(i, j)` 格高度 ≡ `field.heights()[j * cols + i]`」是恒等式，烘焙期不再有第二次插值（`nav_heights_are_field_heights_bit_for_bit` 用一批「插值算不出来」的怪值 + 负零锁位相等）。
5. **`.navmesh` 二进制序列化含魔数与版本、尾部严格长度校验**：`to_bytes` 的字段顺序即格式约定——`魔数 "R3NV"(4) → 版本 u16 = 1 → 维度 / 原点 / 格长 → cfg_digest / input_digest → 高度 f32 → 分类 u8 → 连通块 u32 → 可走位图 u64`；`from_bytes` 逐段校验，末尾 `pos != bytes.len()` 直接报「字节流尾部有多余数据」，并额外拒绝外来魔数、版本漂移、截断、尾部多余、未知分类码（序列化 6 条测试：往返 1 + 拒绝 5）。**注意：尾部是长度校验，不是校验和**——本卡没有 CRC / 汇总哈希字段，完整性由 `product_hash`（两级摘要）+ 长度约束共同表达；D04 落真文件读写时若需要抗截断的强校验，得另加（见四.2）。

**二、容差披露（本卡没有放宽容差，但有三处固定 eps 必须写明）**

| 位置 | 常量 | 值 | 用途 | 是否影响「位相等」判据 |
|---|---|---|---|---|
| `bake.rs` | `FLOOR_EPS` | 0.02 m | 障碍顶比地面高出不到 2 cm 就当它不挡路（贴地判定） | 否：只影响可走位图，且是确定性比较 |
| `grid.rs` | `FLAT_EPS` | 1e-3 m | `SlopeOk` 分支里区分 `Flat`（高差 ≤1 mm）与 `Slope` | 否：分类结果确定 |
| `grid.rs` | `step_verdict` 内 | 1e-5 | 坡度 / 步高比较的统一容差，`classify` 与 `can_step` 共用 | 否：同上 |

「位相等」判据本身**不带任何容差**：`nav_heights_are_field_heights_bit_for_bit` 比的是 `f32::to_bits`，`product_bytes_roundtrip_preserves_hash` 比的是 `product_hash` 原值，`every_available_backend_bakes_the_same_product` 比的是整个 `NavProduct` 的 `PartialEq`。上面三个 eps 只决定「怎么分类 / 能不能跨」，不参与「两个产物是否相同」的判定。

**三、口径订正三处（前两条针对 §4 判据，第三条针对 `D18` 拍板结论，逐条照实记录）**

1. **判据 3 的分类名与「陡坡 = `Blocked`」改了口径**。原判据写「45° 缓坡 `class == Ramp`；50° 陡坡 `class == Blocked`；0.3 m 台阶 `Step`、0.5 m 台阶 `Blocked`」。实施后 `CellClass` 取 `Flat / Slope / Step / Blocked`（`Ramp` 改名 `Slope`），并锁定 **`Blocked` ⟺ 「这一格站不住」**（`class_blocked_iff_not_walkable` 逐格断言等价，且 `Blocked` 格不许还能跨向四邻）。陡坡与超高台阶**不再**记 `Blocked`：它们站得住，只是那条边迈不过去，这件事由 `can_step` 与连通块表达（`too_steep_edge_does_not_mark_cell_blocked`：3 m 断崖两侧都是 `Flat`、`can_step` 为假、`regions == 2`；`curb_above_slope_limit_classifies_step`：0.6 m 坎记 `Step` 且跨得过去）。**这是对旧实现的修正**——旧实现把 `StepVerdict::TooSteep` 记成 `Blocked`，会让「站得住」出现两种互斥说法，报告里的分类直方图也从关卡体检指标退化成噪声。
2. **判据 5 的「32 点等价」用更强的全格对账替代**。`nav_heights_match_terrain_sample_at_cell_centers` 对**每一格**（`dims` 全量）用 `TerrainPatch::sample_height` 在格心重采一次并比 `to_bits`，覆盖比原计划的 32 点（块边界两侧各 16 点）更全。**原先随附缺位的「多块拼接随附专项夹具」已于同轮补齐并销项，见四.1**。
3. **`.navmesh` 字节布局与 `D18` 的原字面不同（语义等价，布局不同）——已拍板 A 档并销项（2026-11-25）**。`D18` 原写的是「`magic`（8 B）+ **头 JSON**（`version` / `cells` / `cfg_digest` / `input_digest`）+ 四段定长体部（`heights` / `class` / `walkable` 1 bit/格 / `region`）」，本卡实际落成「**`"R3NV"`（4 B）+ `u16` 版本 + 定长二进制头**（`dims` / `min` / `cell_size` / `cfg_digest` / `input_digest` / `cells`）+ 体部 `heights:f32` → `class:u8` → `region:u32` → `walkable`（`u32` 字数 + `u64` 位打包）」。四处差异：① magic 4 B 非 8 B；② **头不是 JSON 而是定长二进制**；③ 位图是 `u64` 字数组 + 前置长度，不是紧凑 1 bit/格的字节流（语义仍是 1 bit/格）；④ `region` 与 `walkable` 两段顺序互换。理由：`D18` 自己要求 `product_hash` 位稳定，而 JSON 头会把 `f32` 的十进制往返引入解析漂移风险，且多一个 serde 依赖；定长二进制头字段与 JSON 头一一对应，信息量不减。

   **裁决（老板，2026-11-25）：选 A 档——订正 `D18` 文面，代码布局即准则**。`to_bytes` / `from_bytes` 一行未改，`NAV_FORMAT` 仍为 `1`，序列化 6 条测试的字节断言一条未动。`docs/m2/M2_DECISIONS.md` D18 已改写为逐字段布局表（12 段 + 四类硬拒绝 + 三条自洽校验 + 「尾部是长度校验非校验和」的明示），并在该节留下订正记录。**本节不再挂任何前置，D04 直接按新文面落盘**。

**四、遗留项（不影响 D01 验收；D02 起要记账）**

1. **「12 块拼接场景跨块接缝 `Blocked` 数 = 0」专项断言 —— ✅ 已闭合（2026-11-25，同卡收口提交）**。（原判据 6，以及 §11.1 里「块边界 4」那一组同属此列。）
   - **落点**：新增 `crates/r3e-nav/tests/seams.rs`（606 行，**6 条**），**未动 `grid.rs`**（688 行原样，不顶穿 800 行债务线），只用公开面 `Nav::bake_with` / `NavGrid::{dims, cell_center, ground, class_of, is_walkable_cell, can_step, region_at, region_count}` / `CellClass`。
   - **夹具形态**：12 块 = **4 列 × 3 行**，每块 4 m × 4 m、`resolution = [9, 9]`、格长 0.5 m（每块 8 格），块间**零间隙**（后块左边界 = 前块右边界），高度按「块局部坐标」给函数，避免整块常数地形在块边界上出现第二种解释。四类接缝：连续平地（高差 0）/ 连续缓坡（沿 +X 每米 0.25 m，跨块全局线性）/ 矮坎（每块抬 0.3 m，配 `max_slope_deg = 10°` + `step_height = 0.5`，让「矮坎」与「缓坡」在判据上真正可区分）/ 超高坎（每块抬 1.5 m）。
   - **可核对计数（非空断言）**：跨块接缝邻边**动态算得并钉死 136 条**（竖接缝 `(4-1) × 24` + 横接缝 `(3-1) × 32`，外扩 padding 圈不计）；矮坎场景沿 +X 的**内侧接缝边逐条核对 69 条**（`3 × 23`），并断言「跳过数 = 外沿边数」，不许悄悄少核；逐格对账场景遍历 **4 种接缝 × 每张全网格**（`checked == cols * rows`，明令禁止抽样）。
   - **断言内容**：接缝两侧格 `Blocked = 0`（`blocked_cells` 去重集合为空）+ 接缝每条边双向 `can_step` 为真（计数等于接缝边数）+ 两侧 `CellClass` 与**独立复算的 `step_verdict` 同口径**（坡度上限 `cell_size·tan(max_slope)`、步高、`FLAT_EPS = 1e-3`、比较余量 `1e-5`，测试内各写一遍，容差被改动即红）+ 缓坡接缝不得退化成 `Step`（沿坡高差 = 块内步进 0.125 m，横坡高差 = 0）+ 矮坎低侧 `Step` / 高侧 `Flat` + 平地与可跨坎 `regions == 1`、超高坎 `regions == 4` + 同场景烘两次 `product_hash` 逐位相同且接缝边数/可跨数/Blocked 数三次量一致（确定性）。
   - **夹具不是空壳（变异已验证）**：对一块抬高 0.05 m、缓坡偏移 0.1 m、台阶偏移 0.05 m、块间距 +0.5 m 四种注入均使对应测试失败（`rc = 101`），证明接缝错位一出现就会红。
   - **观察项（不改判据，登记备查）**：`grid.rs::classify` 只看 +X / +Z 正邻边且不看邻格可走性，故**地形最外沿那一格**的正邻落在地形外空白（高度 0）时会被记成 `Step`。这是「地形边界 vs 虚空」的断面，**不是块拼接问题**；本夹具逐格 class 的强断言只做在「两侧正邻均在地形内」的内侧接缝段上，外沿段仍断言 `Blocked = 0` 与可跨性。**未为此放宽任何判据**。若 D05/D06 需要在最外圈也做分类断言，应单独拍「边界格分类口径」，不要顺手改 `classify`。
   - **接缝处未发现真实缺陷**：四类接缝全部按预期通过，`sample_height` 在块与块之间连续（缓坡跨块高差与块内步进一致到 1e-4，即 12 块拼出来是一块连续地形，没有拼缝台阶）。原担心的「假台阶来自 `sample_height` 块内/块外衔接」在零间隙拼接下不成立；**残留风险**是「块之间有间隙或重叠」（本夹具是零间隙紧贴，未覆盖），该情形属于编辑器侧的摆位校验，留给 D03 场景编排放行校验，不在 nav 判据内。
2. **`.navmesh` 文件层读写与错误码落位留给 D04**。本卡只保证 `to_bytes` / `from_bytes` 的字节流自洽，**没有任何文件 IO**，也没有新错误码——`nav.backend.mismatch` / `NAV_NOT_BAKED` 的落盘路径与触发点在 D04 重新定，届时 `NavProduct` 的版本号（当前 `NAV_FORMAT = 1`）与尾部校验要不要升级成带校验和的形态，一起拍。~~**外加一条硬前置**：`D18` 字面布局与本卡实测不一致，D04 落 `nav.export` / `nav.import` 之前必须先拍方向。~~ **该硬前置已销项（2026-11-25，老板拍 A 档）**：`D18` 文面已订正为实测布局（见 `M2_DECISIONS.md` D18 逐字段表），代码侧零改动（`to_bytes` / `from_bytes` 与 6 条字节断言原样）。**因此 `.navmesh` 自本条起可按「`NAV_FORMAT = 1` 稳定格式」对外表述，D04 可直接落盘，不必等新裁决**；仍保留一条纪律：只要改字段顺序/语义就必须递增 `NAV_FORMAT`，不许拿旧版本号装新布局。
3. **命令面零增量**：本卡没加 `nav.*` 命令，`commands` 406 / `rpc` 76 / `domains` 18 / `invariants` 42 / `script_only` 330 全部持平，`docs/capabilities.json` 未变；`rpc` 的 `+7` 预算整块留给 D04。
4. **记账已复位**：`xtask/tests_baseline.json` 同卡更新为 `r3e-nav = 69` / 合计 `2370`（本卡之前是 37 / 2338；首轮收口后为 63 / 2364，接缝夹具补齐 +6 后复位到 69 / 2370）。`check-tests` 只判「低于基线即红」，复位到实测值后 D02 的增量从 69 / 2370 起算。

**五、本卡文件清单（行数为落盘实测）**

| 文件 | 状态 | 行数 | 说明 |
|---|---|---|---|
| `crates/r3e-nav/src/product.rs` | 新增 | 534 | `NavProduct` / `CellClass` / `cfg_digest` / `input_digest` / `to_bytes` / `from_bytes` |
| `crates/r3e-nav/tests/product.rs` | 新增 | 660 | 26 条（真值链 5 / 双后端 4 / 确定性与两级摘要 6 / 序列化 6 / 分类与位图同源 5） |
| `crates/r3e-nav/tests/seams.rs` | 新增（收口补） | 606 | 6 条：12 块（4×3）拼接接缝 —— 平地 / 缓坡 / 矮坎 / 超高坎 / 逐格对账 / 确定性；不动 `grid.rs` |
| `crates/r3e-nav/src/bake.rs` | 修改 | 419 | 删私有 `patch_height`，新增 `field_of` / `bake_with_field` / 后端取场路径 |
| `crates/r3e-nav/src/grid.rs` | 修改 | 688 | 新增 `classify` / `class_of` / `class_at` / `class_histogram` / `step_verdict` |
| `crates/r3e-nav/src/lib.rs` | 修改 | 503 | 新入口 `bake_with_field` / `bake_with_backend` 与 `product` / `product_hash` / `product_hex` 投影 |

全部 ≤800，`size_debt` 白名单零命中，20 条债务未顶穿。**余量提醒**：`grid.rs` 已到 688 行（距 800 仅 112 行），D02「分类代价」若落在 `grid.rs`，须先规划切分（可拆 `grid/class.rs`，`product.rs` 同理可拆 `product/codec.rs`），不要靠抬 `size_debt` 解决（`D-R4`）。

## 5. D02 A\* 寻路

> **状态：已完成**（2026-11-25，本地提交未 push）。实测 `tests` 2370 → **2386**（`r3e-nav` 69 → **85**，**+16**＝预算 16，全部新增、零删减），`verify` 11 步全绿；其余门禁全部持平——`commands` 406 / `rpc` 76 / `script_only` 330 / `domains` 18 / `invariants` 42 / `size_debt` 20 / `dep_debt` 0 / 回放金标 `385482eb033d2964` 未漂移。**算法本体一行未重写**（8 邻、对角不切角、octile 启发式、`(f, 格索引)` 双键全保留），只补分类代价、失败码三子类与观测值；最坏场景实测展开 **142629**（上限 400000，`cost` 1570.66 m）。落地口径、实测值与三处实现坑见 **§5.1**；一处算术订正见 §5.1 三。

- **目标**：在既有 A\* 上补三件事——分类代价、可诊断失败码、确定性 tie-break；不重写算法。
- **判据**：
  1. 起终点均在可走格 → `Found` 且路径每段邻接（切比雪夫距离 = 1）、每段 `can_step` 为真。
  2. 终点在 `Blocked` → `InvalidEndpoint(OffMesh)`；越界 → `InvalidEndpoint(OutOfBounds)`；含 `NaN` → `InvalidEndpoint(NonFinite)`（三个子类各有独立断言，`describe()` 文案冻结）。
  3. 同输入跑 100 次，路径点序列逐点相同（确定性）。
  4. 斜坡路径 vs 平地路径：几何长度相近时，斜坡路径的累计代价更高（分类代价确实生效）。
  5. 200×200 m / 0.5 m 最坏场景（对角 + 迷宫夹具）单次展开数 < `NAV_PATH_MAX_EXPANSIONS`，并记录实测值。
- **涉及文件**：`path.rs`、`grid.rs`（`step_verdict` 共享）、`tests/path.rs`。
- **依赖前置**：D01（要 `CellClass`）。
- **风险与回退**：失败码细分会改变 `nav.path` 的 `reason` 文案 → 既有 `nav.path` 回归断言可能红。处置：`reason` 文案只做**追加式**细分（`offMesh` / `outOfBounds` / `nonFinite` 作为 `outcome` 更细的子值，保留 `invalidEndpoint` 作为兼容值），避免破坏已登记的协议面。

### 5.1 D02 实施记录（2026-11-25）

**一、落地口径（四条，后续各卡按此读代码）**

1. **分类代价挂在「边」上，不看「整格最坏边」**：`path::step_cost(grid, from, to, diagonal)` 只取**抵达格**的 `class_of`——`Slope` 按**这条边自身的高差角**上浮 `1 + (deg / 45°).clamp(0, 1) × 0.5`（`rise / base` 求角，`base` 是 `cell` 或对角 `cell·√2`），`Step` 加固定 `STEP_COST_PENALTY = 0.15` m，`Flat` / `Blocked` 就是几何长度（`Blocked` 已被 `can_step` 挡住，分支只为穷尽）。**不用整格最坏边**的理由：平地里嵌一格坡会把整片平路都算贵，A\* 反而去穿坡。启发式**没有动**——所有分类系数都 ≥ 1，边权恒 ≥ 几何长度，octile 仍是可采纳且一致的下界，最优性/一致性照旧（这是「不重写算法」的硬边界，不是顺手不动）。
2. **失败码细分是追加式的**：`PathOutcome::InvalidEndpoint { which, reason }` 多带一个 `InvalidReason`，枚举本身与 `Unreachable` / `BudgetExceeded` 的文面一字未改。子类判定顺序**固定**：`endpoint_reason` 先判 finite（`NaN` / `Inf` → `NonFinite`），再判 `cell_of` 有无（在范围内 → `OffMesh`，范围外 → `OutOfBounds`）——顺序有意义，否则 `NaN` 会被说成「范围外」，把调用方 bug 伪装成夹具问题。`astar` 内 `region_at == 0` 的格记 `OffMesh`（在范围内但站不住），不是 `OutOfBounds`。**两条字符串面已冻结并有测试钉死**：`as_str` = `nonFinite` / `outOfBounds` / `offMesh`，`describe` = `坐标含 NaN/Inf` / `落在网格范围外` / `不在任何可走格上（吸附半径内无可走点）`；`PathOutcome::describe` 的该分支文案为「不可达：<from|to> 端点{reason.describe()}」。
3. **端点吸附仍是「最多一格」，本卡把它显式化**：`snap_endpoint` 先原位判可走，再 `nearest_walkable(p, cfg.cell_size, None)`，吸附半径恰一格。角色略高于地面或压在两格交界上是常态，硬判会莫名报 `InvalidEndpoint`；吸附也失败才走 `endpoint_reason`。新增测试锁定「略高于表面的起点被吸附、不报错」这条既有行为（此前无断言）。
4. **确定性从「靠容器顺序」升级为「可断言」**：`(f, 格索引)` 双键定序沿用（`Frontier::cmp` 的 `then_with(idx)`），D02 补两条 100 次重跑断言（路径点逐点相同、`PathStats` 逐位相同）。`PathStats.expanded` 只数**真正扩展过**的节点（陈旧堆项在计数前就 `continue`），所以能直接与 `NAV_PATH_MAX_EXPANSIONS` 比；失败时 `cost = 0.0`（**不得读成零长路径**），同格短路返回 `PathStats::default()`（0 次扩展、0 代价）。

**二、实测值与边界（判据 5）**

| 项 | 值 | 说明 |
|---|---|---|
| 夹具 | 200 m × 200 m / `cell = 0.5` → 400×400 = **16 万格**，蛇形长廊，起终点取对角 `(5,5)` → `(195,195)` | `serpentine_scene` + `NavConfig::default().with_cell_size(0.5)` |
| 单次展开数 | **142629** | 上限 400000 的 **35.7%**；等于网格总格数的 89.1%（近全展开，蛇形路线确实逼到最坏） |
| 路径代价 | **1570.66 m** | 两端直线 ≈ 268.7 m，蛇形绕行后约为直线 5.8 倍；断言只钉 `> 500 m` 作「确实绕远」的下界 |
| 耗时 | 0.04 s（`dev` 优化 profile，单测） | 未做性能断言（D 阶段不设时间判据） |

判据 5 的断言只有一条：`expanded < NAV_PATH_MAX_EXPANSIONS`（外加 `cost > 500`）。**没有**把 142629 钉成常量——夹具几何一变就该变，钉常量等于把夹具细节写成契约。

**三、口径订正一处（§3.5 的算术）**

§3.5 表末格原写「200×200 m / 0.5 m = 16 万格，最坏全展开 **0.25×** 上限」。这个比例算错了：16 万 / 40 万 = **40%**，即「全部格各扩展一次」就已经是上限的 0.4×，不是 0.25×。实测蛇形最坏场景 142629 次 = 上限的 **0.357×**、格总数的 **0.891×**。**订正为「最坏全展开 ≈ 0.4× 上限；实测蛇形 14.3 万次（≈ 0.36×）」**，判据 5 的 `< 400000` 门槛不变（本就宽松，实测留有 2.8 倍余量）。

**四、三处实现坑（照实记录，防后来人重踩）**

1. **`Step` 格在 0.5 m 格长下是空集**：`step_verdict` 判 `Step` 的区间是 `cell·tan(max_slope) < rise ≤ step_height`，0.5 m 格 + 45° 上限 → 下界 `0.5` > 上界 `0.4`，区间为空，怎么造都烘不出 `Step`。所以「跨坎加惩罚」判据只能用 **≤ 0.2 m 格长**（本测试取 0.2：下界 0.2 < 0.3 ≤ 0.4 → `Step`）。这不是缺陷而是参数关系；D05 若想在大格长下考 `Step`，须先调 `step_height` 或 `max_slope_deg`，不能改 `step_verdict`。
2. **用「矮盒子」当坎造不出 `Step`**：`bake` 的 `FLOOR_EPS = 0.02` 与障碍顶高判定只影响可走位图，**不改地形高度**，于是 stamp 出来的矮坎格 `ground` 没变、仍读 `Flat`。要造真 `Step` 必须走**地形高度场**：测试内 `ledge_field(side, cell, rise)` 直接造 `HeightField`（x ≥ side/2 抬 `rise`），配 `bake_with_field` 烘焙，基线用同一函数 `rise = 0`，两边几何长度完全一致，差值只剩惩罚本身。首版用的是 stamp 矮盒的 `step_scene`，断言红在「0.3 m 矮坎在 0.2 m 格下必须烘成 `Step` 格」——按 `R7`「不糊」如实换夹具，**没有**放宽断言。
3. **`Unreachable` 的 `expanded` 必须是 0**：`astar` 先用 `region_at` 做连通块预检，两端不同块直接返回，压根不展点。旧断言方向反了（以为「不可达也要保留展开数」），已改为断言 `expanded == 0`，理由写进测试注释：一旦这里非 0，说明有人在注定不可达的场景上白烧 40 万次扩展上限。

**五、遗留项（不影响 D02 验收）**

1. **`nav.path` 协议面透出 `reason` 留给 D04**：当前只在 Rust 侧提供 `PathOutcome::invalid_reason()`，命令面 `outcome` 仍是 `invalidEndpoint`、`reason` 作为更细的子值（§5 风险条的「追加式细分」按此落地）。D04 落协议时把 `reason` 透出即可，**已登记的 `nav.path` 语义与既有断言未被破坏**（本卡无一处在 nav 之外消费这串文案）。
2. **`BudgetExceeded` 分支本卡无法真触发**：在 200 m 上限下最坏也只有 16 万格，重复扩展到不了 40 万；硬造只能把上限调小，那等于「改常量测常量」。**未加测试**，判据 5 只覆盖「实测远低于上限」。若 D05/D06 要覆盖该分支，建议给 `NAV_PATH_MAX_EXPANSIONS` 加 `#[cfg(test)]` 可注入入口，**不要**调小生产上限。
3. **`grid.rs` 一行未改**（仍 688 行）：分类代价与失败码细分全部落在 `path.rs`，`size_debt` 20 未动，也未新增文件。D02 原计划「`grid.rs` 承接代价逻辑」实际不需要——`class_of` / `ground` / `can_step` 已够用，少改一个接近 800 行的文件。

**六、本卡文件清单（行数为落盘实测）**

| 文件 | 状态 | 行数 | 说明 |
|---|---|---|---|
| `crates/r3e-nav/src/path.rs` | 修改 | 422 → **592** | `InvalidReason`（三子类 + 冻结文案）/ `PathStats` / `step_cost` 分类代价 / `snap_endpoint` / `endpoint_reason` / `failed` / `find_path` 与 `astar` 改返回元组 |
| `crates/r3e-nav/src/lib.rs` | 修改 | 503 → **514** | 导出 `InvalidReason` / `PathStats` / 三个代价常量；新增 `Nav::path_with_stats`（`path` 保留为忽略观测值的薄封装） |
| `crates/r3e-nav/tests/path.rs` | 修改 | 206 → **688** | 10 → **26** 条（**+16**）：邻接与可走 3 / 失败码三子类与文案冻结 6 / 确定性 3 / 分类代价 3 / 退化与上界 4 |
| `crates/r3e-nav/src/grid.rs` | **未改** | 688 | 复用既有 `class_of` / `ground` / `can_step` / `region_at` / `nearest_walkable` |
| `xtask/tests_baseline.json` | 修改 | — | `r3e-nav` 69 → **85**、合计 2370 → **2386**（`check-tests` 实测 85 / 2386 自洽） |

全部 ≤800 行，`size_debt` 白名单零命中。

## 6. D03 路径平滑与代理移动 ✅

- **目标**：把格中心折线变成可走直线段 + 内缩拐点，并给出「沿路行走不切角」的可断言代理。
- **判据**：
  1. 平滑后点数严格少于原始点数（≥ 2 段的夹具），且端点不变。
  2. 平滑后每一段 `line_walkable` 为真（不许用直线穿过障碍换长度）。
  3. 平滑后总长度 ≤ 平滑前总长度 + 1e-4（宁长不穿）。
  4. 代理沿平滑路径走完全程，全程与最近障碍距离 ≥ `agent_radius − 1e-3`（不切角穿墙）。
  5. 代理 `y` 全程 = `grid.ground_height`（误差 ≤ 1 cm），不浮空不下沉。
  6. 走完后 `arrived == true`，且步数 ≤ 路径长度 / (速度 · dt) · 1.5。
- **涉及文件**：新增 `smooth.rs`、`agent.rs`；`lib.rs` 导出；`tests/smooth.rs`、`tests/agent.rs`。
- **依赖前置**：D02。
- **风险与回退**：拐角内缩可能把代理挤进「视觉上不美观但合法」的区域；若判据 4 在窄走廊夹具上无法同时满足判据 3，则放宽为「窄走廊单独一条断言，允许回退到格中心点」，并把豁免写进卡备注（不许直接改判据）。

### 6.1 D03 实施记录（2026-11-25）

**一、落地口径（五条，后续各卡按此读代码）**

1. **这里是「视线剪枝 + 拐角内缩」，不是漏斗**：`DD-1 = A` 定了导航域只到**格中心折线 + 可走位图**，没有多边形轮廓、没有端口（portal），严格漏斗（simple stupid funnel）无从下手。所以 `smooth_path(grid, raw)` 分两步：①**string-pulling**——从当前锚点用二分找「最远仍可见」的后续点，成功就跳过中间点；逐步推进到底得到 `pruned`；②**拐角内缩**——对 `pruned` 的每个内部顶点沿「角的内部」退一点，给代理留出身位。判据 2 的「段可走」完全复用 D02 的 `path::segment_ok`（逐半格采样判可走 + `can_step` 判相邻可跨），本卡把它从私有提为 `pub(crate)`，**算法一行没改**。
2. **内缩方向是 `-sum.normalize()`**：顶点 `v[i]` 两侧的单位方向 `a = (v[i-1]-v[i]).normalize()`、`b = (v[i+1]-v[i]).normalize()`，内缩向量取 `-(a+b).normalize()`——即指向折线**弯曲内侧**。首版写成了 `(a+b).normalize()`（朝外侧），结果是顶点被推到墙上、第二道防线整段回退，`tests/smooth.rs` 的「内缩确实生效」那条红得很有说服力（剪枝后点列与输入逐点相同）。内缩量受 `CORNER_INSET_MARGIN = 0.05`（防抖）与 `agent_radius` 共同约束。
3. **三道防线，任一道不过就退回上一个候选**：①剪枝阶段每段必须 `segment_ok`（不可走就绝不剪）；②内缩候选点必须 `grid().is_walkable` 且离最近障碍 ≥ `agent_radius`；③**总长预算是硬约束**——候选点若使总长超过剪枝前长度，就撤回该点、从末点回退。第三道是判据 3「宁长不穿」在代码里的落点，不是测试里补的断言；最坏情况整体回退到 `pruned` 本身。
4. **代理** **不做局部避障**：`NavAgent::advance(grid, dt, speed)` 只沿折线推进；每帧先用「当前段上的投影」把横向偏移纠回来（`OFF_PATH_SLACK` 之内不纠，避免抖动），再按 `dt × speed` 走。落脚点不可走时把 `AgentStep::off_path` 置真并**不再移动**。这是**故意的**：D03 的判据是「沿这条路走会不会切角」，代理一旦自己绕障，就无法再用「离障碍距离」证伪路径本身——那会把 smooth 的 bug 藏进 agent 的聪明里。
5. **到点是一台显式状态机，`seg` 会越过最后一段**：`advance_seg` 走完最后一段时把 `seg` 设成 `path.len() - 1`（越过最后一段），`arrived()` 的判据是 `seg + 1 >= len`。首版设成 `len - 2`（「停在最后一个段号上」），结果 `arrived()` 恒假、代理在零长段上空转到调用方超时——首轮 4 条测试同时红（大预算一帧不到点 / 114 步破 113 上界 / `at` 续行永不抵达 / 山脊首点判据 5）全是这一个 bug。**理由已写进 `advance_seg` 的注释**，防后来人「顺手改回 len-2」。

**二、实测值与边界（判据 4 / 5 / 6）**

| 项 | 值 | 说明 |
|---|---|---|
| 缺口墙夹具 | 16 m × 16 m 平地，两段墙各 6 m 长、中间留 **4 m** 缺口；`cell = 0.5` | 起终点 `(2,2)` → `(13,2)`，必须绕缺口 |
| 平滑结果 | raw **4** 点 → smooth **4** 点（该夹具本身直连可走，无内部点可剪）；台阶夹具（含 `(7,2)` 台阶点）**剪枝后点数严格减少** | 判据 1 的两面都有覆盖：「≥ 2 段的绕行夹具必须减点」与「本来就直连时不许为了减点而绕远」 |
| 内缩位移 | 台阶夹具内部顶点被推到**非整数坐标**，离最近墙角 **> 1.5 m** | 证明内缩真的动了手，不是恒等变换 |
| 代理身位 | 全程与最近障碍距离 **≥ 0.4 − 1e-3**（`NavConfig::default().agent_radius = 0.4`） | 判据 4 |
| 贴地 | 轨迹每点 `\|y − grid.ground_at(p)\| ≤ 1e-3`（判据原文 1 cm，实测收紧到 1 mm），且山脊夹具 `max_y − min_y > 1.0` | 判据 5；`y` 不是常量，确实随坡爬升 |
| 步数 | 路径 **14.971 m**、`dt = 0.1`、`speed = 2`：**75 步**到点，上界 `cap = ceil(14.971 / 0.2 × 1.5) = 113` | 判据 6；理想值 74.85 → 75 步，说明 `dt × speed` 的预算没有被浮点零头白吃帧（bug 版是 114 步，正好破界） |

判据 6 的上界**按卡原文取 1.5 倍**，没有自造「点数 × 2」之类的口径；`cap` 由夹具算出，不写死在断言里。

**三、口径披露三处（都不改判据）**

1. **判据 1 的「点数严格少于」只在有内部点的夹具上成立**：直连场景 raw 本来就是 2 点，没有可删项。测试按判据原文的「≥ 2 段的夹具」取绕行/台阶夹具，**没有**放宽成「≤」。
2. **判据 5 的首点 `y` 由调用方负责**：`NavAgent::new(path)` 不持有 grid，构造期无法贴地；从**第一帧**起才由 `snap` 贴地。测试从 `trail[1]` 起逐点断言，并在测试注释里写明这条边界（否则「起点 `y` 给 0、地面 0.84」会被误读成代理缺陷）。
3. **判据 4 的量法是 XZ 投影到墙的轴对齐包围盒距离**，不是「格中心距离」：格中心距离在 `cell = 0.5` 下最坏差半格对角线，用它必然要么放宽要么假红。包围盒按墙的 `position ± scale/2` 现算，不加夹具侧手工常量。

**四、两处实现坑（照实记录）**

1. **内缩方向取反**：见口径 2。`(a+b)` 归一化指向弯曲**外侧**，用它会先把顶点推到墙里，第二道防线整段回退，表面上「测试全绿」但内缩从未生效——是「内缩生效」那条专门的测试（断言内部顶点非整数、离墙角 > 1.5 m）把它揪出来的。**教训：恒等变换最容易伪装成通过**。
2. **到点状态机的 `seg` 语义**：见口径 5。`seg()` 到点后返回 `len - 1`，这个值**不是**「最后一段的段号」，而是「越过了所有段」。任何按 `seg == len - 2` 判断到点的写法都会错。

**五、遗留项（不影响 D03 验收）**

1. **窄走廊「判据 3 vs 判据 4 冲突」的回退条款未启用**：§6 风险条预备的「窄走廊允许回退到格中心点 + 写卡备注」本卡没触发——现有夹具下内缩与长度预算可同时满足。条款保留在文档里，**判据一条也没改**。
2. **`CORNER_INSET_MARGIN = 0.05` 是防抖经验值**，不是契约：D05 若要更严的身位可调，但**不得**动判据 4 的 `agent_radius − 1e-3` 门限。
3. **`tests/path.rs` 一行未动**（仍 688 行，距 800 硬线 112 行）：D03 的测试全部落在两个新文件里，没有去挤那个已经接近硬线的文件。
4. **`grid.rs` 一行未改**（仍 688 行）：`smooth` / `agent` 只消费 `cell_of` / `cell_center` / `ground_at` / `is_walkable` / `is_walkable_cell` / `config()`，现有公开面已够用。

**六、本卡文件清单（行数为落盘实测）**

| 文件 | 状态 | 行数 | 说明 |
|---|---|---|---|
| `crates/r3e-nav/src/smooth.rs` | **新增** | **219** | `smooth_path` / `SmoothedPath` / `polyline_length` / `CORNER_INSET_MARGIN`：二分视线剪枝 `pull` + 内缩候选 `inset_candidates` + 预算内逐点申请 `compose` |
| `crates/r3e-nav/src/agent.rs` | **新增** | **251** | `NavAgent`（`new` / `at` / `advance` / `arrived` / `remaining` / `pos` / `seg` / `resync` / `snap`）+ `AgentStep`（`arrived` / `progressed` / `off_path`） |
| `crates/r3e-nav/tests/smooth.rs` | **新增** | **367** | **9** 条：点数减少 / 端点逐位不变 / 每段 `line_walkable` / 总长不增 / 拐点离墙角 ≥ `agent_radius − 1e-3` / 二次平滑稳定（幂等）/ 退化输入（空、单点、重复点、零长段）/ 内缩失败回退保顶点 / 台阶拐点内缩确实生效 |
| `crates/r3e-nav/tests/agent.rs` | **新增** | **310** | **7** 条：跨多段到点且精确停在终点 / 山脊 `y` 全程贴地且确实爬升 / 全程踏在可走格 / 全程身位 ≥ `agent_radius − 1e-3` / 步数 ≤ 卡原文 1.5 倍上界且剩余长度归零 / 退化输入（空、单点、`dt` 与 `speed` 非法）零位移不出 NaN / `at` 从偏离位置续行仍到点 |
| `crates/r3e-nav/src/lib.rs` | 修改 | 514 → **518** | 声明 `mod agent` / `mod smooth`，导出 `NavAgent` / `AgentStep` / `smooth_path` / `SmoothedPath` / `polyline_length` / `CORNER_INSET_MARGIN` |
| `crates/r3e-nav/src/path.rs` | 修改 | 592（不动行数口径） | 仅把 `segment_ok` 由私有提为 `pub(crate)`，供 `smooth` 复用；**逻辑一行未改** |
| `crates/r3e-nav/src/grid.rs` | **未改** | 688 | 现有公开面已够用 |
| `xtask/tests_baseline.json` | 修改 | — | `r3e-nav` 85 → **101**、合计 2386 → **2402**（`check-tests` 实测 101 / 2402 自洽） |

全部 ≤800 行，`size_debt` 白名单零命中（仍 20 项、全部未上涨）。

## 7. D04 命令与能力登记

- **目标**：7 条新命令 + 7 条 rpc + 2 条不变量，协议面与实现同卡落地，`unsupported` 无旧残留。
- **判据**：
  1. `cargo xtask gen-capabilities --check` 通过：`commands 413 / rpc 83 / script_only 330 / domains 18 / invariants 44`。
  2. 10 条 `nav.*` 全部带 rpc，`TX_ONLY_METHODS` 无 `nav.*`。
  3. 未烘焙时调 `nav.path` / `nav.smooth` / `nav.probe` 等一律 `NAV_NOT_BAKED`（沿用既有错误码，不新增错误面）。
  4. 两条不变量三源一致：登记表 + 反例测试 + 能力清单。
  5. `nav.export` → `nav.import` 往返后 `product_hash` 不变、逐格 `heights` 位相等（`.navmesh` 格式自洽）。
  6. `args.rs` 行数不变（783），CLI 走 `rpc nav.export --path ...` 可用。
- **涉及文件**：`cmd_spec/meta.rs`、`runtime/src/nav_ops.rs`（新）、`editor-host/src/api/nav.rs`、`doc/src/validate/invariants.rs`、`capabilities.json`（生成）、`tests_baseline.json`。
- **依赖前置**：D03。**`D18` 布局裁决已销项（2026-11-25，A 档）**：`.navmesh` 文面与实现已一致（`R3NV` 4 B + `u16` 版本 + 定长二进制头 + `heights`/`class`/`region` + `u64` 位打包 `walkable`，尾部粗粒度长度校验 + 四类硬拒绝），**D04 可直接按 `NAV_FORMAT = 1` 落 `nav.export` / `nav.import`，不需要任何新裁决**；纪律：改字段顺序或语义必须递增 `NAV_FORMAT`。
- **风险与回退**：`nav.export/import` 涉及文件 IO，是**新增对外副作用面**——写盘路径必须来自调用方显式参数，禁止默认落到工程目录；若 io 权限模型在 headless 与编辑器不一致，则 `nav.export` 降级为「返回字节、由宿主落盘」，保持 nav 侧零 IO（回退档写进卡备注）。

> **状态：已完成**（2026-11-25，本地提交未 push）。实测 `tests` 2402 → **2410**（`r3e-nav` 101 → **103**、`r3e-runtime` 70 → **73**、`r3e-editor-host` 146 → **149**，**+8**＝预算 8，全部新增、零删减），`verify` 11 步全绿；门禁 `commands` **413** / `rpc` **83** / `script_only` 330 / `domains` 18 / `invariants` **44**（三源 A/B/C 皆 44），`size_debt` **20 未动**、`dep_debt` **0**。
>
> **落地要点（判据 1–6 全绿）**：
>
> 1. **`api.rs` 顶穿后当场切分，未加白名单（`D-R4` 口径）**：`nav.*` 十条默认实现把 `r3e-protocol/src/api.rs` 顶到 894 行，超 `size_debt` 的 800 行阈值。处置是把 nav 段抽成**子 trait**——新建 `crates/r3e-protocol/src/api/nav.rs`（`pub trait NavApi`，十条 `nav.*` 默认实现＋文档，155 行），`api.rs` 只留 `mod nav; pub use nav::NavApi;` 并把 `EngineApi` 改成 `pub trait EngineApi: NavApi`，行数 **894 → 780**；`lib.rs` 补 `pub use`。**白名单一项未加**，`lint-size` 扫描 447 个 `.rs` 仍是 20 项债务、全部未上涨。
> 2. **宿主侧拆两块，契约测试跟着改**：`r3e-editor-host/src/api/mod.rs`（445 行）把十条 `nav_*` 委托从 `impl EngineApi` 移到 `impl NavApi for LocalEngineApi`；`tests/cmd_spec_contract.rs` 改为**同时扫两个 impl 块**再合并方法表，否则「宿主方法都能在表里找到 rpc」会假红。宿主方法数 **76 → 83**，`cmd_spec/mod.rs` 的 `rpc == rpc_only + N` 由 `+36` 改 **`+43`**（83 RPC / 47 只 RPC）。协议层 11 个测试桩同步补 `impl NavApi for X {}` 空实现。
> 3. **`nav.path` 失败 `reason` 透出（§5.1 遗留 1 销项）**：回包把 `outcome` 细分为 `found` / `unreachable` / `invalidEndpoint` / `budgetExceeded`，并附 `reason`（`PathOutcome::describe()` 的人话，`InvalidEndpoint` 三种子类文案已在 D02 冻结）；`found: false` 时 `points` 为空表、`count` 为 0——「走不到」与「算不完」不再合并成同一个 `false`。
> 4. **`export` / `import` 的路径纪律**：`path` 必须由调用方显式给出，空串与纯空白判参数错，父目录不存在同样判参数错（**不自动建目录树**、不猜工程目录），失败后不留半个文件；`import` 只读不烘（`Nav::from_product`），往返后 `product_hash` 一字不差。`args.rs` 一行未改，CLI 走 `rpc nav.export --path ...`。
> 5. **两条不变量 `nav.bake.empty` / `nav.backend.mismatch`**：三源（登记表 / 反例测试 / 能力清单）集合相等。反例分别钉「烘焙结果一格不可走 ⇒ `Error` 且文案说清可走格为 0」与「网格记录的后端名 ≠ 当前物理后端 ⇒ `Error` 且两侧后端各点名」；`nav.bake` 落槽位时 `attach_backend` 只贴名，**不改网格、不改产物哈希**（D01 判据 4 不受影响）。
> 6. **`docs/capabilities.json` 重生成**：`EXPECTED_COMMANDS` 406 → **413**，生成器实测 `commands=413 / rpc=83 / script_only=330 / domains=18 / invariants=44`，`--check` 通过。
>
> **遗留（不影响本卡验收）**：`nav.*` 里 `probe` / `nearest` / `hash` 的边界参数校验仍只在 `r3e-nav` 侧覆盖，宿主回包层只钉了「无槽位统一 `NAV_NOT_BAKED`」，逐条参数错映射留待 D05 判据卡一并补。

## 8. D05 判据测试（D5 默认 + 可配置）

- **目标**：把「≤ 1.6× 直线」做成脚本化判据，并让组数 / seed / 阈值可覆盖。
- **判据**：
  1. 默认配置（20 组 / seed 42 / 1.6×）下，20 组起终点路径长度 / 直线距离的最大值 ≤ 1.6（记录实测最大值，如 1.23）。
  2. 覆盖为 40 组 / seed 7 / 1.4× 后，同方向断言仍成立（按 1.4 判），且用例集确实变了（点集哈希不同）。
  3. 非法配置被拒：`cases == 0` → 报错；`limit <= 0` → 报错；`limit` 为 `NaN` → 报错（错误类型固定，文案冻结）。
  4. 不可达反例：起终点分处两个连通块 → 断言「不返回空路径」而是 `Unreachable` + `disconnectPoint` 非 `None`。
  5. 直线被障碍切断的夹具：断言路径确实绕行（长度 > 直线）而非穿墙。
- **涉及文件**：`tests/scale.rs`（新）、`tests/path.rs`。
- **依赖前置**：D04（要用命令面或 `Nav` 公开 API）。
- **风险与回退**：20 组夹具若含「必须绕远路」的极端地形，1.6× 会假红。处置：夹具按「缓坡 + 稀疏障碍」构造，并单独给一条「强绕行场景」用宽松阈值断言，两条判据分开登记。

## 9. D06 可玩性回归 3（`DD-6`）

- **目标**：地形上放一个跟随目标，代理自行绕障抵达——headless 可复现，不依赖人眼。
- **判据**：
  1. 代理从出生点出发、目标沿预设航点移动，代理在 **≤ 40 s** 游戏时间内抵达每个航点（累计误差 ≤ 1 m）。
  2. 全程 `walkable` 恒真、与障碍距离 ≥ `agent_radius − 1e-3`、`y` 与地面误差 ≤ 1 cm。
  3. 双后端（builtin / rapier）各跑一遍，终点差 < 0.5 m、`state_hash` 相同（与 C07 回归口径一致）。
  4. CLI 一条命令可复现：`r3e-cli rpc nav.bake --path ... && r3e-cli rpc nav.path --from ... --to ...`，输出与测试内断言一致。
  5. 目标移动到不可达区域时，代理停在断连边界（`disconnectPoint` 附近 ≤ 2 m）而非原地抖动或穿墙。
- **涉及文件**：`tests/nav_agent_follow.rs`（新）、`r3e-runtime` 装配点（如需）。
- **依赖前置**：D05。
- **风险与回退**：若 headless 逐帧推进太慢（> 30 s 墙钟），采样步长放大到 1/30 s 并加「等效位移」断言；若真机 GPU 与 headless 结果不一致，以 headless 为准（回归判据口径与 C07 一致），真机只留证不判据。

---

## 10. 必拍板决策项（DD-1 ～ DD-6）

> 拍板方式与 `CD-1~CD-7` 相同：逐项给结论，拍板提交里一次性回写 `M2_PLAN.md` / `README.md` / `M2_DECISIONS.md`（追加 D17–D19），再开 D01。

### 拍板结论（2026-11-24）✅ 六项全选 A 档

| 项 | 结论 | 一句话 |
|---|---|---|
| **DD-1** | **A** | 导航产物为**格级位图 `NavGrid` + string-pulling**；真多边形 NavMesh 与严格漏斗**登记为 M2 不做** |
| **DD-2** | **A** | 双后端一致性走**构造性保证 + 回归锁**：唯一烘焙入口吃 `HeightField`，判据是 `product_hash` 与逐格高度**位相等**，锁的是「切后端不改 field + 烘焙确定」 |
| **DD-3** | **A** | 产物载体为**二进制 `.navmesh`**（magic + 头 JSON + 四段定长体部），`product_hash` 可作缓存键 |
| **DD-4** | **A** | 7 条 `nav.*` **全部带 rpc**；D 行 `rpc +2` → **`+7`**，全局目标 `85` → **`90`**，`script_only` 保持 **330** |
| **DD-5** | **A** | 两条不变量选 **`nav.bake.empty`** + **`nav.backend.mismatch`**（不取 B 的重叠选题） |
| **DD-6** | **A** | D06 验收形态为 **headless 集成测试 + CLI 一键复现**，真机只留证不判据 |

结论落 `M2_DECISIONS.md` 的 **D17–D19**（D17 导航形态 / D18 产物载体与哈希 / D19 命令面、不变量与回归形态）；计划侧回写已完成于 `M2_PLAN.md` §2 / §3 / §3.4 / §4 / §6 / §7 / §8 与本文件 §11 / §14 / §15。逐项原始论证保留在下方，档位文字已按拍板结论标注。

### DD-1 导航形态：位图 + string-pulling，还是真多边形 NavMesh？

- **为什么必须拍**：直接决定 D 阶段代码量（700 vs 2400 行）、文件数、以及与 `DD-2` 哈希判据的兼容性；动手后再改等于重做 D01/D03。
- **选项**：
  - **档 A（推荐，已拍板）**：保持 `NavGrid` 体素形态，新增逐格分类，平滑用「视线剪枝 + 拐角内缩」。
  - 档 B：体素 → 轮廓 → 凸多边形 → 邻接图 → 严格漏斗（真 NavMesh）。
  - 档 C：A + 后置多边形化（只服务平滑）。
- **推荐档 A（已拍板 2026-11-24）**，理由见 §3.1：判据 1.6× 有余量、与双后端哈希判据不冲突、6 卡节奏可交付。
- **不复核的后果**：若事后才想改 B，`product.rs`（分类/哈希/序列化）与 `grid.rs`（位图）全部作废，D01/D03 返工，估算 **+5～8 人日**，且 D05 的 20 组夹具要按多边形重标；若选 B 而 `DD-2` 又取档 A，则「哈希位相等」大概率天天红（顶点合并顺序受浮点影响），M2 串行链条会被一卡卡死。

### DD-2 双后端一致性的验收口径

- **为什么必须拍**：卡表写的是「双后端各烘一次，产物**哈希须一致**」，字面上可被读成「写两份实现比对」——那是 `R7` 的翻版，必须现在把口径钉死。
- **选项**：
  - **档 A（推荐，已拍板）**：构造性一致 + 回归锁。唯一烘焙入口吃 `HeightField`，两后端各 `set_heightfield` 后取回，比 `product_hash` 与逐格值位相等。
  - 档 B：两套独立采样实现 + 容差（可走格差异 ≤ 0.2%）。
  - 档 C：只烘 builtin，rapier 抽样。
- **推荐档 A（已拍板 2026-11-24）**：与 C04/D16「查询只写一份」的既有架构同向；测试锁的是「切后端不改 field + 烘焙确定」，可长期防回归。
- **不复核的后果**：取 B 会在导航层复刻 `R7`——两套采样语义并存，此后任何地形改动都要双份维护，且 `product_hash` 永远不能作为缓存键（`DD-3` 与 `D4=A` 的缓存命中判据随之失效），回归时间翻倍。取 C 则卡表判据实质放弃，需要在 `M2_PLAN` 里把 D01 判据划掉——等于 M2 的「双后端」承诺在导航层打折。

### DD-3 产物缓存形态与哈希算法

- **为什么必须拍**：`D4=A` 说「产物随场景缓存、运行期只加载」，但**载体没定**；不定则 `nav.export/import` 无从下手，D04 命令面写不出来。
- **选项**：
  - **档 A（推荐，已拍板）**：二进制 `.navmesh`（magic + 头 JSON + 定长体部），SplitMix64 两级摘要 + 产物哈希。
  - 档 B：JSON（可读、可 diff，体积 3–5 倍、浮点文本化有精度争议）。
  - 档 C：不落盘，只在内存槽位（等于放弃 `D4=A` 的「随场景缓存」）。
- **推荐档 A（已拍板 2026-11-24）**：16 万格 × (4 B 高度 + 1 B 分类 + 1 bit 可走 + 4 B 区域) ≈ **1.5 MB**，二进制一眼可算；JSON 化会到 6–8 MB 且解析慢。
- **不复核的后果**：取 B 后 `nav.export` 的字节数与 `product_hash` 都会随序列化器实现漂移（浮点格式化策略差异），缓存命中率失真；取 C 则 `D4=A` 落空——每次开场景都要重烘，`G04` 的帧尖峰判据（<2 ms）会被编辑器启动时的烘焙吞掉，属于跨阶段返工。

### DD-4 命令面条数与 `rpc` 预算（关键：连锁改全局目标）

- **为什么必须拍**：`commands +7` 是**预算下限**，而守恒规则 4 规定 `script_only`（现 330）**不得增加**。两条一联立，`rpc` 必须同步涨。
- **选项**：
  - **档 A（推荐，已拍板）**：新增 7 条命令，**全部带 rpc** → `rpc +7`（D 行 `+2` → **`+7`**），全局目标 `85` → **`90`**；`commands 406 → 413`、`script_only` 保持 **330**。
  - 档 B：只新增 2 条命令（全带 rpc）→ `commands 406 → 408`，全局 **433 落空**（E+F+G+H 仅 +20，最终 428）。
  - 档 C：7 条命令里 5 条不带 rpc → 这 5 条只走命令表入口（**不进** `script_only`，故**不违反**守恒规则 4），`script_only` 仍 **330**、全局 `rpc` 仍 **85**、`commands` 仍 **413**，**账目完全自洽**；真实代价是 CLI / 脚本无法直调 `nav.stats` / `nav.probe` / `nav.nearest` / `nav.export` / `nav.import`，`D06` 的「CLI 一键复现」断在 `export` / `stats` 上。
- **推荐档 A（已拍板 2026-11-24）**：`M2_PLAN §4` 的原计划（7 条中 2 条带 rpc）**本身就是自洽的**，`commands 433` / `rpc 85` 恰好达成——档 A 不是「唯一能同时满足两条约束的档」，而是**用多 5 条 rpc 换取「全命令面可脚本化」的更优解**，代价是把全局目标 `85` 抬到 `90` 并回写 5 处文档。与 `CD-1`（8 条 `terrain.*` 全带 rpc）推理同构但动机不同：`CD-1` 是「不给 rpc 就破守恒规则 4」，本条是「不给 rpc 就断一键复现链」。
- **不复核的后果**：档 B 会让 `M2_PLAN §4` 的目标行 `commands 433` 在 M2 收口时对不上，必须在 E/F/G/H 阶段临时加塞 5 条命令（越到后期越难找合理落点）；档 C 的代价不在账目而在能力面——`capabilities.json` 里 5 条 `rpc = false`，日后被当脚本 API 用会踩空，且 `D05` / `D06` 的自动化链只覆盖 `bake` / `path`。选 A 则须同卡改 `M2_PLAN §4` 的 D 行与目标行、§3 阶段总览与 §2 基线目标、`AI_ROADMAP §0.2`、`docs/m2/README.md`、`M2_DECISIONS D14` 注记与新增 D19（漏任一处，`doc_gate` 或 H05 对账时会问）。

### DD-5 两条不变量的选题

- **为什么必须拍**：`invariants +2` 是硬预算，且三源一致（登记 / 反例 / 清单）必须同卡完成；选题错了要么产生假红、要么无法构造反例。
- **选项**：
  - **档 A（推荐，已拍板）**：`nav.bake.empty`（Error）+ `nav.backend.mismatch`（Error）。
  - 档 B：`nav.grid.degenerate` + `nav.path.too_long`。
  - 档 C：不加（全局 `invariants 48` 落空为 46）。
- **推荐档 A（已拍板 2026-11-24）**：两条都对应「烘焙产物本身不可信」这一最贵故障，且反例可低成本构造（全场陡壁 / 双后端夹具）；`nav.backend.mismatch` 在 `DD-2` 档 A 下应为恒真，一旦红即证明后端被污染——这是把架构约束变成可执行资产。
- **不复核的后果**：档 B 的 `nav.path.too_long` 与 D05 的 1.6× 判据重复（同一件事两处判，夹具一改双红），`nav.grid.degenerate` 又与 `nav.bake.empty` 语义重叠，最终会退化成「一条有效不变量 + 一条噪音」；档 C 则 `M2_PLAN` 目标行 `48` 少 2，H 阶段收口时对不上账。

### DD-6 D06 回归形态

- **为什么必须拍**：`M2_PLAN §7 R6` 明确六张回归卡不可省，但「怎么算回归过」C 阶段有两种形态（headless 断言 / 真机人工 + 证据图）。
- **选项**：
  - **档 A（推荐，已拍板）**：headless 集成测试（代理跟航点、双后端终点差、CLI 一键复现）+ 真机仅留证不判据。
  - 档 B：真机人工操作 + 截图留证（H-01B 那种）。
  - 档 C：只做单元断言，不设回归卡（违反 R6）。
- **推荐档 A（已拍板 2026-11-24）**：与 C07 `terrain_roam.rs`（10 条 headless 行走回归 + 双后端终点差 <0.5 m）口径一致，可进 CI 常态跑。
- **不复核的后果**：选 B 则 D06 不可自动化，M2 收口时「可玩性回归 3」只能靠人证，`H05` 的退出条件验收缺一环；选 C 直接违反 `R6`，等于 M2 少一张必过卡。

---

## 11. 门禁配平

### 11.1 tests +82 的六卡分配

| 卡 | 增量 | 细分 |
|---|---|---|
| D01 | **+26** | 分类 6（Ramp/Step/Blocked/Flat/边界/退化）、摘要与哈希 6（确定性 / 改地形 / 改 cfg / 覆盖范围 / hex 格式 / 反序列化校验）、双后端 4（两后端各烘 + 位相等 + 差异 dump + 切后端不改 field）、地形真值 6（32 点等价 / 块边界两侧 / 多块取最高 / 无 patch / 单 patch / 越界）、块边界 4 |
| D02 | **+16** | 邻接与可走 4、失败码 3 子类 3、确定性 3、分类代价 3、展开上界 3 |
| D03 | **+16** | 平滑 9（点数减少 / 段可走 / 长度不增 / 端点不变 / 拐点身位 / 二次稳定 / 退化输入 / 内缩回退保顶点 / 内缩确实生效）、代理 7（跨段抵达 / 贴地 / 全程可走 / 全程身位 / 步数上界 / 退化输入 / 偏离续行）——**实测落点**，原细目的「窄走廊 2 + 退化 2」因窄走廊回退条款未触发而并入身位与退化两组 |
| D04 | **+8** | 命令与清单 3（计数 / 无 `nav.*` 进 TX_ONLY / `NAV_NOT_BAKED`）、导出导入往返 3、两条不变量反例 2 |
| D05 | **+10** | 默认 20 组 2、参数覆盖 3、非法配置 3、不可达反例 1、绕行 1 |
| D06 | **+6** | 航点抵达 2、全程约束 2、双后端 1、CLI 复现 1 |
| 合计 | **+88** | 原预算 +82；D01 收口补 6（12 块接缝夹具），故 D 阶段终值再抬高 6 |

> **D01 实测（2026-11-25）**：**+32**（原预算 +26，收口补 6）——实际构成为「真值链 5 / 双后端 4 / 确定性与两级摘要 6 / 序列化 6 / 分类与位图同源 5 / **12 块拼接接缝 6**」。原细目里「地形真值 6（32 点等价…）」由**全格对账**替代（更强）；「块边界 4」那组多块拼接夹具首轮未落地，**已于同轮收口补齐 6 条销项**（`tests/seams.rs`，覆盖比「块边界 4」更宽：四类接缝 × 双向可跨 × 逐格口径对账 × 确定性，见 §4.1 四.1）。总数：`r3e-nav` 37 → **69**，全局 2338 → **2370**，**D02 从这里起算**。

> **D02 实测（2026-11-25）**：**+16**，与预算**恰好相等**（无超支、无欠账）——实际构成为「邻接与可走 **3** / 失败码三子类与文案冻结 **6** / 确定性 **3** / 分类代价 **3** / 退化与展开上界 **4**」。相对原细目的两处调整：①「邻接与可走 4」压到 3（原第 4 条「路径点全在可走格」与第 1 条逐段采样重复，合并进同一条断言）；②「展开上界 3」压到 2、另立「退化 2」（同格零代价零扩展、不可达零代价且 `expanded == 0`）——原先「同格」被算在上界组，实际它考的是**退化短路**不是**预算**，分开更清楚。总数：`r3e-nav` 69 → **85**，全局 2370 → **2386**；**D03 从这里起算**。

> **D03 实测（2026-11-25）**：**+16**，与预算**恰好相等**（零超支、零删减）——实际构成为「平滑 **9** / 代理 **7**」。相对原细目的调整：原「窄走廊 2」未落地（§6 风险条的「窄走廊判据 3 vs 4 冲突回退」在本卡夹具上**未触发**，条款保留、判据未改），其覆盖面由「拐点离墙角 ≥ `agent_radius − 1e-3`」与「代理全程身位 ≥ `agent_radius − 1e-3`」两条更强的断言吸收；原「退化 2」按 smooth / agent 各一条拆开写。总数：`r3e-nav` 85 → **101**，全局 2386 → **2402**；**D04 从这里起算**。

`tests` 落点：**2338 + 88 = 2426**（原预算口径 +82 = 2420，D01 收口补 6 后抬到 2426）。注意 `M2_PLAN §4` 的计划口径是 2330 + 82 = 2412，差 **8** 来自 C 阶段实超（C 预算 +108，实际 +180 中 nav 之外的部分已含在 2338 里）——按「预算为下限、只增不减」，2412 是下限不是目标值，D 阶段落 2426 属正常。

### 11.2 其他门禁

| 门禁 | C 末实测 | D 末 | 说明 |
|---|---|---|---|
| `commands` | 406 | **413**（+7） | 与预算一致 |
| `rpc` | 76 | **83**（+7） | `DD-4` **已拍板档 A**（全局目标 `85` → `90`）；档 B / 档 C 的 78 仅存于 §10 的历史论证，不再生效 |
| `script_only` | 330 | **330** | 守恒规则 4：不得增加 |
| `invariants` | 42 | **44**（+2） | `DD-5` 档 A 的两条 |
| `domains` | 18 | **18** | 不建 `nav` 域；`19` 留给 `H01` 的 `game` |
| `dep_debt` | 0 | **0** | limit 保持 0 |
| `size_debt` | 20 | **20** | 新增文件一律 ≤800；若某文件顶穿，须当场切分（同 C04 `port_impl` 826 → 768 的处理方式） |
| crate 成员 | 18 | **18** | 不新建 crate（`R8` 红线） |

> **D04 实测（2026-11-25）**：`commands` **413** / `rpc` **83** / `script_only` **330** / `domains` **18** / `invariants` **44** 与本表「D 末」列逐项一致；`size_debt` **20**（未新增）、`dep_debt` **0**、crate 成员 **18** 均未动。

### 11.3 `DD` 拍板后需回写的文档（2026-11-24 拍板提交中已一次性完成）

| 文档 | 改动 | 状态 |
|---|---|---|
| `M2_PLAN.md` §2 | 基线表 `rpc` 目标 `≥85（+23）` → `≥90（+28）`，注明由 §4 拍板抬升 | ✅ |
| `M2_PLAN.md` §3 | 阶段总览 D 行 `rpc +2 → +7`、合计 `+23 → +28`、门禁目标值 `rpc ≥80 → ≥90`；顶部「当前进度」补 D 阶段拍板一句 | ✅ |
| `M2_PLAN.md` §3.4 | D01 判据「可行走多边形」→「可行走区域（格级位图 `NavGrid`）」+ 双后端「构造性一致、位相等」；D03「漏斗算法」→「string-pulling，不做严格漏斗」；D04 补「7 条全带 rpc + 二进制 `.navmesh`」；D06 补「headless + CLI 一键复现」 | ✅ |
| `M2_PLAN.md` §4 | D 行 `tests +82 / commands +7 / rpc +2 → +7 / invariants +2 / domains 0`；目标行 `rpc 85 → 90`；补「D 阶段 +82 构成」一段（对齐 C 阶段写法） | ✅ |
| `M2_PLAN.md` §6 | `r3e-nav/src/*` 行补注「命令面实现落 `r3e-runtime/src/nav_ops.rs`（D 阶段例外，理由同 C06 `terrain_ops`）」 | ✅ |
| `M2_PLAN.md` §7 | `R7` 处置补 D 阶段口径（`DD-2=A`：构造性一致、不以两套实现形态出现）；「M2 不做」清单登记「真多边形 NavMesh 与严格漏斗」 | ✅ |
| `M2_PLAN.md` §8 | 「下一张卡」改 `D01`，并列出开工前必读（本稿 §3.3 / §10 `DD` 结论 / `CD-3` 的 `patch_height` 到期项） | ✅ |
| `M2_DECISIONS.md` | 追加 **D17（导航形态）/ D18（产物载体与哈希）/ D19（命令面、不变量与回归形态）**，并在 D14 补 rpc 目标抬升注记 | ✅ |
| `docs/AI_ROADMAP.md` §3.3 | 引言里的 `rpc` 目标 `≥85` → `≥90`（口径订正：目标值在 §3.3 而非 §0.2，§0.2 是历史实测表、不含目标值，不动） | ✅ |
| `docs/m2/README.md` | 文件清单加 `M2_D_PLAN.md` 一行并改状态；当前状态补 D 阶段拍板结论与 `rpc` 口径；下一步改「D01」 | ✅ |

---

## 12. 执行顺序

| # | 步骤 | 产出 / 验证点 |
|---|---|---|
| 1 | ~~老板拍板 `DD-1`～`DD-6`~~ ✅ 已完成（2026-11-24，六项**全选 A 档**） | 决策结论落 `M2_DECISIONS.md`（D17–D19） |
| 2 | ~~回写计划与索引（§11.3 全表）~~ ✅ 已完成（单提交 `docs(m2): D 阶段拍板结论落盘…`，`doc_gate` 等文档类门禁绿） | 计划与索引数字自洽，`gen-capabilities --check` 通过 |
| 3 | ~~**D01** 分类 + 摘要 + 双后端入口 + 删 `patch_height`~~ ✅ 已完成（2026-11-25 单提交，未 push；遗留收口同轮补齐） | `tests 2338 → 2370` 已达成（`r3e-nav` 37 → 69，**+32**＝预算 26 + 接缝夹具收口 6）；`size_debt 20` / `dep_debt 0`；双后端 `product_hash` 位相等已留证（4 条）；落地口径五条、容差披露、三处口径订正；**两项遗留（`D18` 文面、12 块接缝夹具）均已销项闭合**，仅余 D04 文件层读写 1 条顺延 |
| 4 | ~~**D02** 分类代价 + 失败码细分 + tie-break~~ ✅ 已完成（2026-11-25 单提交，未 push） | `2370 → 2386` 已达成（`r3e-nav` 69 → 85，**+16**＝预算 16，零删减）；`nav.path` 兼容性不破（`InvalidEndpoint` 追加 `reason` 字段，枚举与既有文面未改）；判据 5 实测 142629 展开（上限 40 万）；**算法本体一行未重写**；`size_debt 20` / `dep_debt 0`；落地口径四条、三处实现坑、一处算术订正、三项遗留见 §5.1 |
| 5 | ~~**D03** 平滑 + `NavAgent`~~ ✅ 已完成（2026-11-25 单提交，未 push） | `2386 → 2402` 已达成（`r3e-nav` 85 → 101，**+16**＝预算 16，零删减）；不切角断言绿（全程离障 ≥ `agent_radius − 1e-3`）、贴地 1 mm（判据 1 cm 收紧）、步数 75 / 上界 113；**视线剪枝 + 拐角内缩，未做漏斗**（`DD-1=A`）；`size_debt 20` / `dep_debt 0` / 金标 `385482eb033d2964` 未漂移；落地口径五条、口径披露三处、两处实现坑见 §6.1 |
| 6 | ~~**D04** 7 命令 + 2 不变量 + 清单重生成~~ ✅ 已完成（2026-11-25 单提交，未 push） | `2402 → 2410` 已达成（`r3e-nav` 101 → 103、`r3e-runtime` 70 → 73、`r3e-editor-host` 146 → 149，**+8**＝预算 8，零删减）；`commands 413 / rpc 83 / script_only 330 / domains 18 / invariants 44` 全部实测通过，`gen-capabilities --check` 绿；**`api.rs` 894 → 780 当场切分（子 trait `NavApi`），白名单一项未加**；`nav.path` 失败 `reason` 已透出；`size_debt 20` / `dep_debt 0` / 金标未漂；落地要点六条见 §7 状态块 |
| 7 | **D05** 1.6× 判据 + 可配置 | `2410 → 2420`；非法配置被拒 |
| 8 | **D06** headless 跟随回归 | `2420 → 2426`；双后端 `state_hash` 同 |
| 9 | D 阶段收口 | `cargo xtask verify` 11 步全绿；`size_debt 20` / `dep_debt 0`；回放金标未漂；下一张卡改 `E01` |

每卡仍是「一卡一提交」（`feat(m2-dXX): …`），`fmt`/`clippy` 修正在同提交内；判据先行（先写红再实现）。

---

## 13. 风险与回滚

| # | 风险 | 触发条件 | 处置 |
|---|---|---|---|
| D-R1 | 删私有 `patch_height` 引起行为微变 | C01 20 点断言或新增 32 点断言红 | 先证等价再删；红则判定是旧实现偏差，写进卡备注（`R7` 不糊原则），必要时回滚本卡 |
| D-R2 | 双后端哈希不一致（`DD-2` 档 A 下不应发生） | `heights` 出现位差 | 立刻 dump 首个差异格（坐标 / 两后端值 / field 摘要）——若是 field 内容就说明后端污染，属最贵故障，**不得**改判据放宽 |
| D-R3 | 分类与 `can_step` 两套语义漂移 | 改一处判据另一处不红 | 强制共用 `step_verdict`；`grid.rs` 加一条「分类与边判定同源」的断言 |
| D-R4 | 新增文件顶穿 800 行 | `cargo xtask lint-size` 红 | 当场切分（`product.rs` 可拆 `product/codec.rs`），不靠加 `size_debt` 解决 |
| D-R5 | `commands` 与 `script_only` 冲突 | 若日后回退到档 B / 档 C（当前 `DD-4` **已定档 A**） | 档 B → 全局 433 落空需后期加塞 5 条命令；档 C → **账目自洽**（不进 `script_only`）、代价是 5 条命令 CLI / 脚本不可直调、`D06` 复现链断在 `export` / `stats`。**只能在拍板阶段解决**，实施阶段无解 |
| D-R6 | `nav.export` 引入文件 IO 副作用 | headless 与环境权限不一致 | 降级为「返回字节、宿主落盘」，nav 侧保持零 IO |
| D-R7 | 平滑在窄走廊同时满足「更短」与「不切角」不可能 | 判据 3 与 4 冲突 | 窄走廊单列夹具并豁免长度不增断言，**不改主判据** |
| D-R8 | 回归卡被跳（`R6`） | 赶进度 | D06 不可省；不可省的方式是「headless + CLI 一键复现」，不是「人眼看一下」 |

回滚点：每卡提交即回滚单元；D01 是本阶段唯一「删既有实现」的卡，回滚代价最高，必须先在分支外的临时验证里跑通等价断言再动主干。

---

## 14. 明确不做

- **不做**多边形 NavMesh 与严格漏斗——**已拍板**（2026-11-24，`DD-1 = A`），不是待定项：`DD-1` 取 A 的直接后果，已在 `M2_PLAN.md` §7「M2 不做」同步登记。若日后要 B，另立卡，不在 M2 强行塞入。
- **不建** `Domain::Nav`（`domains` 预算 0）。
- **不做**动态障碍重烘与运行时增量重烘（`D4=A` 已定编辑期预烘焙）。
- **不做**多代理避让（RVO / ORCA）——`H03` 的 NPC 追击只要求「各自寻路 + 简单推挤」，避让不在 M2 判据内。
- **不做** off-mesh links（跳跃 / 传送门），D 阶段不引入跨层连通。
- **不做** NavMesh 分块流式加载（归 `G` 阶段）。
- **不给** `r3e-script` 加 `nav` 依赖（`dep_debt limit = 0` 红线），脚本侧一律经宿主注入。
- **不改** `args.rs`（余量 17 行），新增命令只走 `rpc --method`。
- **不改** `state_hash` 口径：`nav` 产物不进场景哈希。

---

## 15. 速览

- **6 卡**：D01 烘焙（分类 + 摘要 + 双后端）✅ 已完成 → D02 A\*（不重写，补代价/失败码/确定性）✅ 已完成 → D03 平滑 + 代理 ✅ 已完成 → D04 命令与能力登记 ✅ 已完成 → **下一张：D05 1.6× 判据 + 可配置** → D06 headless 回归。
- **门禁**：tests **2338 → 2420**（+82；D01 实收 +32 后口径抬到 2426——实测落点 D02 2386 / D03 2402 / **D04 2410**）、commands **406 → 413**、rpc **76 → 83**（`DD-4` 档 A）、invariants **42 → 44**、domains **18 不变**、`size_debt` **20 不变**、`dep_debt` **0**。
- **最大风险**：`DD-1` 选错形态（返工 5–8 人日）与 `DD-2` 口径被读成「两份实现」（`R7` 复现）。
- **最大收获**：`D4=A` 的「编辑期预烘焙 + 运行期只加载」在 D 阶段才第一次有真实载体（`.navmesh` + 两级摘要 + 产物哈希），`H03` 的 NPC 追击直接吃这套产物。
- **开工前必读**：本稿 §10 拍板结论（六项全 A）、§3.3（双后端口径）、`M2_DECISIONS.md` D4 / D5 与 D17–D19，以及 `CD-3` 里那条「`bake.rs::patch_height` 本轮不动」的到期约定。
*（内容由AI生成，仅供参考）*
