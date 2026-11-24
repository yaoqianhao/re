# M2 D 阶段方案：NavMesh 烘焙 + A* 寻路（D01–D06）

- 文档版本：v1（2026-11-24 落盘）
- 状态：**待拍板**（`DD-1`～`DD-6` 全项未决；拍板后按 §12 顺序逐卡落地）
- 上位约束：`M2_PLAN.md` §3.4 卡表 / §4 门禁累加预算 / §5 单卡执行规程 / §6 文件占用矩阵 / §7 风险 `R7`/`R8`
- 决策前置：`M2_DECISIONS.md` **D4=A**（编辑期预烘焙，产物随场景缓存，运行期只加载、G04 不给烘焙预算）、**D5=默认 + 可配置**（20 组 / seed 42 / 1.6×）、**D14/D15/D16**（C 阶段已落：`rpc` 预算、`r3e-terrain` 纯派生层、地形查询独立通道）
- 本稿范围：**只出方案**。不改一行引擎代码，不改既有计划里的任何数字；§11 的配平在 `DD` 拍板后随「拍板提交」一次性回写 `M2_PLAN.md` / `README.md` / `M2_DECISIONS.md`。
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

---

## 4. D01 导航网格烘焙

- **目标**：一次烘焙产出**自描述**的 `NavProduct`——格高、可走位图、逐格分类、连通块、两级输入摘要与产物哈希；地形高度不再由 nav 自造。
- **判据**（全部可自动断言）：
  1. 同一场景烘两次 → `product_hash` 相同（确定性）。
  2. 改动地形任一样本或任一 `NavConfig` 字段 → `input_digest` / `cfg_digest` 变化，`product_hash` 随之变化（摘要确实覆盖了输入）。
  3. 45° 缓坡逐格 `class == Ramp`；50° 陡坡逐格 `class == Blocked`；0.3 m 台阶 `class == Step`、0.5 m 台阶 `Blocked`（默认 `step_height = 0.4`）。
  4. **双后端各烘一次 → `product_hash` 位相等**，且 `heights` 逐格位相等、`class` 逐格相等（`DD-2` 档 A）。
  5. 删除 `bake.rs::patch_height` 后，C01 的 20 点交叉断言 + 新增 32 点断言全绿（含块边界两侧各 16 点）。
  6. 块边界处相邻格高差不产生「假台阶」：12 块拼接场景下，跨块接缝处 `Blocked` 格数为 0。
- **涉及文件**：`bake.rs`、`grid.rs`、`lib.rs`、新增 `product.rs`、`tests/bake.rs`、`tests/product.rs`、`xtask/tests_baseline.json`。
- **依赖前置**：C07 完成（已满足，`4ea213d` 之后）；`core::TerrainPatch::sample_height` 可用（已满足）；`physics::{HeightField, Backend::set_heightfield}` 可用（C04 已满足）。
- **风险与回退**：最大风险是「删私有插值 → 行为微变」导致 C 阶段既有断言红。处置：先加 32 点断言**证明新旧等价**（红则说明旧实现有 bug，按 R7「不糊」原则记录进卡备注），再删；仍不绿则 `git checkout -- .` 回滚本卡重来，不带红进 D02。

## 5. D02 A\* 寻路

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

## 6. D03 路径平滑与代理移动

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
- **依赖前置**：D03。
- **风险与回退**：`nav.export/import` 涉及文件 IO，是**新增对外副作用面**——写盘路径必须来自调用方显式参数，禁止默认落到工程目录；若 io 权限模型在 headless 与编辑器不一致，则 `nav.export` 降级为「返回字节、由宿主落盘」，保持 nav 侧零 IO（回退档写进卡备注）。

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

### DD-1 导航形态：位图 + string-pulling，还是真多边形 NavMesh？

- **为什么必须拍**：直接决定 D 阶段代码量（700 vs 2400 行）、文件数、以及与 `DD-2` 哈希判据的兼容性；动手后再改等于重做 D01/D03。
- **选项**：
  - **档 A（推荐）**：保持 `NavGrid` 体素形态，新增逐格分类，平滑用「视线剪枝 + 拐角内缩」。
  - 档 B：体素 → 轮廓 → 凸多边形 → 邻接图 → 严格漏斗（真 NavMesh）。
  - 档 C：A + 后置多边形化（只服务平滑）。
- **推荐档 A**，理由见 §3.1：判据 1.6× 有余量、与双后端哈希判据不冲突、6 卡节奏可交付。
- **不复核的后果**：若事后才想改 B，`product.rs`（分类/哈希/序列化）与 `grid.rs`（位图）全部作废，D01/D03 返工，估算 **+5～8 人日**，且 D05 的 20 组夹具要按多边形重标；若选 B 而 `DD-2` 又取档 A，则「哈希位相等」大概率天天红（顶点合并顺序受浮点影响），M2 串行链条会被一卡卡死。

### DD-2 双后端一致性的验收口径

- **为什么必须拍**：卡表写的是「双后端各烘一次，产物**哈希须一致**」，字面上可被读成「写两份实现比对」——那是 `R7` 的翻版，必须现在把口径钉死。
- **选项**：
  - **档 A（推荐）**：构造性一致 + 回归锁。唯一烘焙入口吃 `HeightField`，两后端各 `set_heightfield` 后取回，比 `product_hash` 与逐格值位相等。
  - 档 B：两套独立采样实现 + 容差（可走格差异 ≤ 0.2%）。
  - 档 C：只烘 builtin，rapier 抽样。
- **推荐档 A**：与 C04/D16「查询只写一份」的既有架构同向；测试锁的是「切后端不改 field + 烘焙确定」，可长期防回归。
- **不复核的后果**：取 B 会在导航层复刻 `R7`——两套采样语义并存，此后任何地形改动都要双份维护，且 `product_hash` 永远不能作为缓存键（`DD-3` 与 `D4=A` 的缓存命中判据随之失效），回归时间翻倍。取 C 则卡表判据实质放弃，需要在 `M2_PLAN` 里把 D01 判据划掉——等于 M2 的「双后端」承诺在导航层打折。

### DD-3 产物缓存形态与哈希算法

- **为什么必须拍**：`D4=A` 说「产物随场景缓存、运行期只加载」，但**载体没定**；不定则 `nav.export/import` 无从下手，D04 命令面写不出来。
- **选项**：
  - **档 A（推荐）**：二进制 `.navmesh`（magic + 头 JSON + 定长体部），SplitMix64 两级摘要 + 产物哈希。
  - 档 B：JSON（可读、可 diff，体积 3–5 倍、浮点文本化有精度争议）。
  - 档 C：不落盘，只在内存槽位（等于放弃 `D4=A` 的「随场景缓存」）。
- **推荐档 A**：16 万格 × (4 B 高度 + 1 B 分类 + 1 bit 可走 + 4 B 区域) ≈ **1.5 MB**，二进制一眼可算；JSON 化会到 6–8 MB 且解析慢。
- **不复核的后果**：取 B 后 `nav.export` 的字节数与 `product_hash` 都会随序列化器实现漂移（浮点格式化策略差异），缓存命中率失真；取 C 则 `D4=A` 落空——每次开场景都要重烘，`G04` 的帧尖峰判据（<2 ms）会被编辑器启动时的烘焙吞掉，属于跨阶段返工。

### DD-4 命令面条数与 `rpc` 预算（关键：连锁改全局目标）

- **为什么必须拍**：`commands +7` 是**预算下限**，而守恒规则 4 规定 `script_only`（现 330）**不得增加**。两条一联立，`rpc` 必须同步涨。
- **选项**：
  - **档 A（推荐）**：新增 7 条命令，**全部带 rpc** → `rpc +7`（D 行 `+2` → **`+7`**），全局目标 `85` → **`90`**；`commands 406 → 413`、`script_only` 保持 **330**。
  - 档 B：只新增 2 条命令（全带 rpc）→ `commands 406 → 408`，全局 **433 落空**（E+F+G+H 仅 +20，最终 428）。
  - 档 C：7 条命令里 5 条不带 rpc → `script_only` 330 → **335**，违反守恒规则 4。
- **推荐档 A**，且这是唯一能同时满足「commands 目标 433」与「script_only 不增」的档——与 `CD-1`（8 条 `terrain.*` 全带 rpc）的推理链完全同构。
- **不复核的后果**：档 B 会让 `M2_PLAN §4` 的目标行 `commands 433` 在 M2 收口时对不上，必须在 E/F/G/H 阶段临时加塞 5 条命令（越到后期越难找合理落点）；档 C 直接在 D04 让 `gen-capabilities --check` 与守恒规则打架，卡红。选 A 则须同卡改 `M2_PLAN §4` 的 D 行与目标行、`README` 当前状态里的 rpc 数字、以及 `M2_DECISIONS D14` 的注记（这三处漏改任一处，`doc_gate` 会在后续卡报「文档数字与清单不一致」）。

### DD-5 两条不变量的选题

- **为什么必须拍**：`invariants +2` 是硬预算，且三源一致（登记 / 反例 / 清单）必须同卡完成；选题错了要么产生假红、要么无法构造反例。
- **选项**：
  - **档 A（推荐）**：`nav.bake.empty`（Error）+ `nav.backend.mismatch`（Error）。
  - 档 B：`nav.grid.degenerate` + `nav.path.too_long`。
  - 档 C：不加（全局 `invariants 48` 落空为 46）。
- **推荐档 A**：两条都对应「烘焙产物本身不可信」这一最贵故障，且反例可低成本构造（全场陡壁 / 双后端夹具）；`nav.backend.mismatch` 在 `DD-2` 档 A 下应为恒真，一旦红即证明后端被污染——这是把架构约束变成可执行资产。
- **不复核的后果**：档 B 的 `nav.path.too_long` 与 D05 的 1.6× 判据重复（同一件事两处判，夹具一改双红），`nav.grid.degenerate` 又与 `nav.bake.empty` 语义重叠，最终会退化成「一条有效不变量 + 一条噪音」；档 C 则 `M2_PLAN` 目标行 `48` 少 2，H 阶段收口时对不上账。

### DD-6 D06 回归形态

- **为什么必须拍**：`M2_PLAN §7 R6` 明确六张回归卡不可省，但「怎么算回归过」C 阶段有两种形态（headless 断言 / 真机人工 + 证据图）。
- **选项**：
  - **档 A（推荐）**：headless 集成测试（代理跟航点、双后端终点差、CLI 一键复现）+ 真机仅留证不判据。
  - 档 B：真机人工操作 + 截图留证（H-01B 那种）。
  - 档 C：只做单元断言，不设回归卡（违反 R6）。
- **推荐档 A**：与 C07 `terrain_roam.rs`（10 条 headless 行走回归 + 双后端终点差 <0.5 m）口径一致，可进 CI 常态跑。
- **不复核的后果**：选 B 则 D06 不可自动化，M2 收口时「可玩性回归 3」只能靠人证，`H05` 的退出条件验收缺一环；选 C 直接违反 `R6`，等于 M2 少一张必过卡。

---

## 11. 门禁配平

### 11.1 tests +82 的六卡分配

| 卡 | 增量 | 细分 |
|---|---|---|
| D01 | **+26** | 分类 6（Ramp/Step/Blocked/Flat/边界/退化）、摘要与哈希 6（确定性 / 改地形 / 改 cfg / 覆盖范围 / hex 格式 / 反序列化校验）、双后端 4（两后端各烘 + 位相等 + 差异 dump + 切后端不改 field）、地形真值 6（32 点等价 / 块边界两侧 / 多块取最高 / 无 patch / 单 patch / 越界）、块边界 4 |
| D02 | **+16** | 邻接与可走 4、失败码 3 子类 3、确定性 3、分类代价 3、展开上界 3 |
| D03 | **+16** | 平滑 6（点数减少 / 段可走 / 长度不增 / 端点不变 / 单段 / 空路径）、代理 6（抵达 / 不切角 / 贴地 / 到点切换 / 偏离回正 / 速度即位移）、窄走廊 2、退化 2 |
| D04 | **+8** | 命令与清单 3（计数 / 无 `nav.*` 进 TX_ONLY / `NAV_NOT_BAKED`）、导出导入往返 3、两条不变量反例 2 |
| D05 | **+10** | 默认 20 组 2、参数覆盖 3、非法配置 3、不可达反例 1、绕行 1 |
| D06 | **+6** | 航点抵达 2、全程约束 2、双后端 1、CLI 复现 1 |
| 合计 | **+82** | |

`tests` 落点：**2338 + 82 = 2420**。注意 `M2_PLAN §4` 的计划口径是 2330 + 82 = 2412，差 **8** 来自 C 阶段实超（C 预算 +108，实际 +180 中 nav 之外的部分已含在 2338 里）——按「预算为下限、只增不减」，2412 是下限不是目标值，D 阶段落 2420 属正常。

### 11.2 其他门禁

| 门禁 | C 末实测 | D 末 | 说明 |
|---|---|---|---|
| `commands` | 406 | **413**（+7） | 与预算一致 |
| `rpc` | 76 | **83**（+7） | **依赖 `DD-4` 取档 A**；若取档 B 则 78 |
| `script_only` | 330 | **330** | 守恒规则 4：不得增加 |
| `invariants` | 42 | **44**（+2） | `DD-5` 档 A 的两条 |
| `domains` | 18 | **18** | 不建 `nav` 域；`19` 留给 `H01` 的 `game` |
| `dep_debt` | 0 | **0** | limit 保持 0 |
| `size_debt` | 20 | **20** | 新增文件一律 ≤800；若某文件顶穿，须当场切分（同 C04 `port_impl` 826 → 768 的处理方式） |
| crate 成员 | 18 | **18** | 不新建 crate（`R8` 红线） |

### 11.3 `DD` 拍板后需要改的文档（一次性，同一提交）

| 文档 | 改动 |
|---|---|
| `M2_PLAN.md` §3.4 | D01 判据「可行走多边形」→「可行走区域（格级）」（若 `DD-1 = A`）；D03「漏斗算法」→「string-pulling（挡 A 档等效口径）」 |
| `M2_PLAN.md` §4 | D 行 `tests +82 / commands +7 / rpc +2 → +7 / invariants +2 / domains 0`；目标行 `rpc 85 → 90`；补「D 阶段 +82 构成」一段（对齐 C 阶段 §4 的写法） |
| `M2_PLAN.md` §6 | `r3e-nav/src/*` 行补注「D 阶段独占；命令面实现落 `r3e-runtime/src/nav_ops.rs`（D 阶段例外，理由同 C06 `terrain_ops`）」 |
| `M2_PLAN.md` §7 | `R7` 触发条件补「`D01` 双后端结论不一致」——已在表内，D 阶段落地时按 `DD-2` 档 A 记录口径 |
| `M2_PLAN.md` §8 | 「下一张卡」改 `D01`，并列出开工前必读（本稿 §3.3 / `DD` 结论 / `CD-3` 的 `patch_height` 到期项） |
| `M2_DECISIONS.md` | 追加 **D17（导航形态）/ D18（双后端与产物缓存）/ D19（命令面与 rpc 预算）** 三组结论，编号沿用现有序列 |
| `docs/m2/README.md` | 文件清单加 `M2_D_PLAN.md` 一行；当前状态补 C 阶段实测与 D 阶段待拍板；下一步改「拍板 DD-1~DD-6 → D01」 |

---

## 12. 执行顺序

| # | 步骤 | 产出 / 验证点 |
|---|---|---|
| 1 | 老板拍板 `DD-1`～`DD-6` | 决策结论落 `M2_DECISIONS.md`（D17–D19） |
| 2 | 回写计划与索引（§11.3 全表） | `cargo xtask verify` 全 11 步绿（文档改动即触发 `doc_gate`），单提交 `docs(m2): D 阶段拍板…` |
| 3 | **D01** 分类 + 摘要 + 双后端入口 + 删 `patch_height` | `tests 2338 → 2364`；`size_debt 20`；双后端哈希位相等留证 |
| 4 | **D02** 分类代价 + 失败码细分 + tie-break | `2364 → 2380`；`nav.path` 兼容性不破 |
| 5 | **D03** 平滑 + `NavAgent` | `2380 → 2396`；不切角断言绿 |
| 6 | **D04** 7 命令 + 2 不变量 + 清单重生成 | `2396 → 2404`；`commands 413 / rpc 83 / invariants 44`；`gen-capabilities --check` 通过 |
| 7 | **D05** 1.6× 判据 + 可配置 | `2404 → 2414`；非法配置被拒 |
| 8 | **D06** headless 跟随回归 | `2414 → 2420`；双后端 `state_hash` 同 |
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
| D-R5 | `commands` 与 `script_only` 冲突 | `DD-4` 若取 B/C | 取 B → 全局 433 落空需后期加塞；取 C → 当场违反守恒规则 4。**只能在拍板阶段解决**，实施阶段无解 |
| D-R6 | `nav.export` 引入文件 IO 副作用 | headless 与环境权限不一致 | 降级为「返回字节、宿主落盘」，nav 侧保持零 IO |
| D-R7 | 平滑在窄走廊同时满足「更短」与「不切角」不可能 | 判据 3 与 4 冲突 | 窄走廊单列夹具并豁免长度不增断言，**不改主判据** |
| D-R8 | 回归卡被跳（`R6`） | 赶进度 | D06 不可省；不可省的方式是「headless + CLI 一键复现」，不是「人眼看一下」 |

回滚点：每卡提交即回滚单元；D01 是本阶段唯一「删既有实现」的卡，回滚代价最高，必须先在分支外的临时验证里跑通等价断言再动主干。

---

## 14. 明确不做

- **不做**多边形 NavMesh 与严格漏斗（`DD-1` 取 A 时的直接后果）。若日后要 B，另立卡，不在 M2 强行塞入。
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

- **6 卡**：D01 烘焙（分类 + 摘要 + 双后端）→ D02 A\*（不重写，补代价/失败码/确定性）→ D03 平滑 + 代理 → D04 7 命令 + 2 不变量 → D05 1.6× 判据 → D06 headless 回归。
- **门禁**：tests **2338 → 2420**（+82）、commands **406 → 413**、rpc **76 → 83**（`DD-4` 档 A）、invariants **42 → 44**、domains **18 不变**、`size_debt` **20 不变**、`dep_debt` **0**。
- **最大风险**：`DD-1` 选错形态（返工 5–8 人日）与 `DD-2` 口径被读成「两份实现」（`R7` 复现）。
- **最大收获**：`D4=A` 的「编辑期预烘焙 + 运行期只加载」在 D 阶段才第一次有真实载体（`.navmesh` + 两级摘要 + 产物哈希），`H03` 的 NPC 追击直接吃这套产物。
- **开工前必读**：本稿 §3.3（双后端口径）、§11.3（拍板要改的 7 处文档）、`M2_DECISIONS.md` D4/D5，以及 `CD-3` 里那条「`bake.rs::patch_height` 本轮不动」的到期约定。
