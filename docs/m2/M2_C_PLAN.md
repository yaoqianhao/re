---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_36056659e63711f1bfe1525400dc9a98
    ReservedCode1: BUBKQFq1LqM30OtAnPqlm4tnrg7s3Btq9nRVmWj5qkCHarAm0SVUBEF/Au7jtIq0Lp461aJGl2DaUI3yQjRu8bq5I5LagdT/C/gX0EJs1FYPbPggqbzAitzmBGkknjp4tegYbGGNr4L0iSIFXXZmPb9iexuaG3wX39ArG3CBwQzr8wxke0Lo9lGxG8E=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_36056659e63711f1bfe1525400dc9a98
    ReservedCode2: BUBKQFq1LqM30OtAnPqlm4tnrg7s3Btq9nRVmWj5qkCHarAm0SVUBEF/Au7jtIq0Lp461aJGl2DaUI3yQjRu8bq5I5LagdT/C/gX0EJs1FYPbPggqbzAitzmBGkknjp4tegYbGGNr4L0iSIFXXZmPb9iexuaG3wX39ArG3CBwQzr8wxke0Lo9lGxG8E=
---

# M2-C 阶段实施方案（C01–C07）

> 版本：2026-11-22 首稿（承接 `M2_PLAN.md` §3.2 C 阶段卡表；格式对齐 `M2_B_PLAN.md`）
> 状态：**待老板拍板 §12 七项决策**（CD-1 为硬冲突，不拍不能开工）
> 统计口径：本文所有「实测」均为 2026-11-22 在本机 `D:\rust3d-engine` HEAD `712824a` 上跑出来的真值；标「预算」的是规划值，落地时以实测覆盖并记录偏差。

---

## 0. 一句话

C 阶段把「地形」从 M1 的数据壳（只能存高度样本、不能采样、不能看见、不能站上去）变成**可生成、可采样、可渲染、可站立**的一等能力：
新建 `r3e-terrain` 派生层 crate（不持有持久状态）+ 新增 `Terrain` 能力域（8 条命令）+ 物理侧新增地形场查询通道（双后端各验）+ CLI 一条 `terrain.gen` 走通 200×200/seed 42 的确定性哈希。

---

## 1. 本阶段要交付什么（对卡表判据的逐项映射）

| 卡 | `M2_PLAN` 判据原文（摘要） | 本文落地形态 | 卡号（本文拆分后） |
|---|---|---|---|
| C01 | 建 `r3e-terrain` crate + 高度场存取 + 块划分（默认 50 米，可配置），成员 18→19，`dep_rules` allow 仅 `r3e-core` | crate 骨架 + `BlockGrid` + 单块/LOD 采样基础（采样基础函数上移 `r3e-core`） | C01 |
| C02 | 多分辨率 LOD 采样 | 三级 LOD（步长 1/2/4）+ 接缝零差 + 误差上界 | C02 |
| C03 | 分块网格 + 材质混合，`r3e-terrain` 不依赖 `r3e-render` | **拆两卡**：C03a 网格与 4 层权重（headless 可判）/ C03b 渲染通道与混合着色（GPU + 截图） | C03a / C03b |
| C04 | 地形碰撞（heightfield 查询，双后端各验） | 物理侧独立「地形场」通道 + builtin 自实现 + rapier `HeightField3d` | C04 |
| C05 | CLI 生成确定性哈希：`terrain.gen --size 200x200 --seed 42` | `r3e-cli rpc --method terrain.gen` 通道 + 16 位十六进制哈希 + 两次调用逐字节一致 | C05 |
| C06 | 判据测试与 `domains` 增 `terrain` | `Domain::Terrain` + 2 条 invariant + `Category::Terrain` + 能力清单重生成 | C06 |
| C07 | 可玩性回归 2 | `assets/labs/terrain_roam/` 自包含夹具 + 5 路点行走 + 证据截图 | C07 |

拆分理由见 §12 CD-4：C03 是**本阶段唯一需要改渲染管线**的卡，混在一起会让「网格生成是否正确」与「着色是否正确」两个完全不同性质的失败原因互相掩盖。

---

## 2. 开工前实测真值（2026-11-22 / HEAD `712824a`）

### 2.1 门禁基线

| 项 | 实测 | 取数命令 |
|---|---|---|
| `#[test]` 总数 | **2150** | `cargo xtask check-tests` |
| commands | **398** | `cargo xtask gen-capabilities --check` |
| rpc | **68** | 同上 |
| script_only | **330**（= 398 − 68） | 同上 |
| domains | **17** | 同上 |
| invariants | **40** | `cargo xtask invariants` |
| workspace 成员 | **18**（17 crate + `xtask`） | `Cargo.toml` |
| `size_debt` 条目 | **20**（只许减） | `xtask/size_debt.json` |
| `dep_debt` 条目 | **0** | `cargo xtask dep-check` |
| 回放金标 | **`385482eb033d2964`**（seed 42 / 600 帧 / hash_every 60） | `cargo xtask replay --seed 42` |

### 2.2 地形现状（M1 已落地的东西，**不要重造**）

| 资产 | 位置 | 实测规模 | 与本阶段的关系 |
|---|---|---|---|
| `TerrainPatch` + 表语义 | `crates/r3e-core/src/terrain.rs` | 238 行 | **唯一权威数据**；本轮只在**同文件追加**采样函数 |
| `Scene` 地形访问器 | `crates/r3e-core/src/scene_tables.rs` | 92 行 | 零改动（`upsert/remove/find_terrain_patch` 已在） |
| 地形撤销命令 | `crates/r3e-doc/src/command/terrain.rs` | 37 行 | C01 建块/删块命令复用它，不新写命令类型 |
| 哈希覆盖 | `crates/r3e-doc/src/state_hash.rs` `mix_terrain`（L505/L586） | 高度样本逐点入哈希 | C05 直接取用，**不新建哈希** |
| 导航采样 | `crates/r3e-nav/src/bake.rs` 私有 `patch_height`（双线性） | 私有 | 保留不动（见 CD-3），登记为 D 阶段收口项 |
| 物理地面 | `crates/r3e-physics` `Physics::set_ground` / `sample_ground` / `ColliderShape{Box,Sphere,Capsule,Cylinder}` | `lib.rs` 1924 行（在 `size_debt`） | **无高度场形状**，C04 全部新增 |
| 渲染 | `crates/r3e-render/src/`（18 个文件） | 无任何 terrain/splat/混合命中 | C03b 全部新增 |
| 命令面 | `crates/r3e-core/src/cmd_spec/` 12 个域文件 | 无 `terrain` 域、无 terrain 命令 | C01 新建 `cmd_spec/terrain.rs` |
| 脚本地形 | 只有 `scene.paint_terrain`（事务意图）经 `r3e-editor-host/src/api/tx.rs`（636 行） | 单点写入 | C 阶段的新命令是**能力化**封装，`paint_terrain` 语义保持不变 |

### 2.3 动手前必须知道的三个硬约束（都实测过）

1. **顶点属性 location 只剩 1 格**。`crates/r3e-render/src/mesh.rs` L11 注释原文：顶点占 6（0..=4 与 11）、实例占 9（5..=10、12..=14），**只剩 15**。
   → 地形若想「每顶点 4 层混合权重」走既有实例管线，会把 location 撑爆（真机踩过：`Vertex attribute location 16 must be less than limit 16`）。
   **C03b 因此必须建独立的 terrain 管线与独立顶点缓冲**，不得复用 PBR 管线。
2. **`app.rs` 在 `size_debt` 里**（实测 2821 行）。R2（G 阶段前 `app.rs` 新增 >100 行要提前 G02）在 C 阶段等于「装配一律落新文件，`app.rs` 增量 ≈ 0」。
3. **`args.rs` 余量 97 行**（实测 703 / 阈值 800）。C05 走 `r3e-cli rpc` 通道即不动它（见 CD-7）。

---

## 3. 总体设计

### 3.1 C 阶段末的依赖图（新增边只有 3 条）

```
r3e-core ──→ r3e-terrain        （新 crate，allow 仅 r3e-core）
   │              │
   │              ├──→ r3e-runtime        （装配：地形网格 → 渲染通道；地形场 → 物理）
   │              └──→ r3e-editor-host    （8 条 rpc 的实现）
   │
   ├─ r3e-render   （**不**依赖 terrain：只吃 core::MeshData + &[[f32;4]] 权重切片）
   ├─ r3e-script   （**不**依赖 terrain：7 条脚本命令用 core 采样函数实现）
   └─ r3e-physics  （**不**依赖 terrain：高度场以裸数组 + 网格尺寸传入）
```

`dep_rules.json` 需登记 3 处，全部在 C01 同卡：
1. 新条目 `"r3e-terrain": { "allow": ["r3e-core"] }`
2. `r3e-runtime.allow` 追加 `"r3e-terrain"`
3. `r3e-editor-host.allow` 追加 `"r3e-terrain"`

R8 红线（`M2_PLAN` §7）：`r3e-terrain` 的 allow 永远只有 `["r3e-core"]`。**渲染与物理都不许反向进它的依赖表**——这是 D12=B 的全部意义，也是本阶段最容易在赶工时被破的线。

### 3.2 `r3e-terrain` 是**纯派生层**（没有自有持久状态）

这是本阶段最重要的架构决定（CD-2）。它不定义任何进场景、进存档、进哈希的新类型：

```rust
// 输入永远是场景里已有的东西
pub fn grid(rect: Rect2, block_size: f32) -> Option<BlockGrid>;
pub fn sample(&self, patches: &[TerrainPatch], x: f32, z: f32) -> Option<Sample>;
pub fn lod_of(&self, block: BlockId, distance: f32) -> Lod;
pub fn mesh(&self, patches: &[TerrainPatch], block: BlockId, lod: Lod) -> Option<TerrainMesh>;
pub fn generate(size: (u32, u32), seed: u64, block_size: f32) -> Vec<TerrainPatch>; // 产出 core 类型
```

好处：`state_hash`、存档格式、撤销栈、校验器**全部零改动**；C 阶段不新增任何「第二套地形表示」。代价：每帧采样都要重算块索引（`O(块数)`，16 块，可忽略）。

### 3.3 三层职责切分（谁实现什么）

| 层 | 责任 | 不允许 |
|---|---|---|
| `r3e-core` | 数据（`TerrainPatch`）+ **单块**双线性采样 + 边界清洗 | 引入块划分 / LOD / 网格概念 |
| `r3e-terrain` | 块划分、LOD 选择、采样组合、网格与权重、生成、哈希 | 持有持久状态、依赖 render/physics/asset |
| `r3e-render` / `r3e-physics` | 各自消费 core 类型或裸切片 | 反向依赖 `r3e-terrain` |

### 3.4 确定性口径（贯穿 C05/C06）

- 生成算法必须是**自实现 PRNG**（`SplitMix64`，12 行），零新依赖；同 seed → 逐字节相同。
- 哈希取 `r3e-doc::state_hash` 的既有产物（16 位十六进制小写），与回放金标同格式；**不新建哈希函数**。
- C 阶段新增的命令**不得改变既有场景的回放哈希**（金标 `385482eb033d2964` 必须不漂）。

---

## 4. C01：建 `r3e-terrain` + 采样基础 + 块划分（tests **+16**）

### 4.1 文件与预算

| 文件 | 预算行数 | 内容 |
|---|---|---|
| `crates/r3e-terrain/Cargo.toml` | 18 | 依赖仅 `r3e-core`（`glam` 走 workspace 继承） |
| `src/lib.rs` | ~120 | 常量（`DEFAULT_BLOCK_SIZE = 50.0`、`MAX_BLOCKS`、`LOD_STEPS = [1,2,4]`）+ 模块声明 + crate 文档 |
| `src/block.rs` | ~200 | `BlockGrid::new(rect, block_size)`：校验（`block_size > 0`、有限、矩形面积 > 0）、`block_at(x,z)`、`blocks()` 枚举、`covers()` 无缝判定 |
| `src/sample.rs` | ~180 | `sample(patches, x, z)`：跨块命中选择 + 调用 core 单块采样；`normal()` 中心差分 |
| `src/lod.rs` | ~150 | `Lod::{L0,L1,L2}`、`lod_for(distance, cfg)`、`step()` |
| `tests/block.rs` / `tests/sample.rs` | — | 见 4.4 |

`core/src/terrain.rs`：238 → **约 300 行**（追加 `sample_height` 与 2 个私有辅助），远低于 800 阈值，不进 `size_debt`。

### 4.2 关键契约

```rust
// r3e-core/src/terrain.rs 追加（不是新类型，只是把「单块采样」这件事说清楚）
impl TerrainPatch {
    /// 块内双线性采样；点落在块外返回 None。NaN 输入返回 None（不 panic）。
    pub fn sample_height(&self, x: f32, z: f32) -> Option<f32>;
}
```

语义必须与 `r3e-nav/src/bake.rs` 的私有 `patch_height` **逐位一致**（同样的边界取整与双线性权重顺序）。C01 加一条**交叉断言**：在 nav 的测试夹具里取 20 个点，`nav::bake::patch_height` 与 `core::sample_height` 结果完全相同（差 = 0），保证「未来 D 阶段收口时删旧实现不会改行为」。

> 这条断言写在 `crates/r3e-terrain/tests/sample.rs` 里，需要 `r3e-terrain` 的 dev-dependency 加 `r3e-nav`。**dev-dependency 不进 `dep_rules` 的 allow 表**（`dep-check` 只扫常规依赖），实测 `r3e-nav` 已有 `r3e-core` 边，故只新增 `r3e-terrain` 的 dev 边一条，属测试期单向，可接受。若老板要求 dev 边也登记，则退化为「把 20 个点的期望值写死成常量」——档 B 备选，成本高但零新边（CD-3 附注）。

### 4.3 块划分的可配置面（D10 落地）

| 参数 | 默认 | 合法域 | 非法处置 |
|---|---|---|---|
| `block_size` | 50.0 米 | `(0, 1000]` 且有限 | `BlockGrid::new` 返回 `None`；命令返回 `false` |
| `max_blocks` | 32（与 `TERRAIN_MAX_PATCHES` 同值） | `[1, 32]` | 同上 |
| `lod_steps` | `[1, 2, 4]` | 严格递增、均为 2 的幂 | 编译期常量，不入配置面 |

200×200 米 + 50 米 → 4×4 = **16 块**，符合 D10 的「4×4=16 块、驻留 3×3」描述中的块数一半（驻留策略属运行时，C03b 装配时按 `distance` 选择，不做剔除）。

### 4.4 C01 的 16 条测试（必须各自独立 `#[test]`）

| # | 文件 | 断言 |
|---|---|---|
| 1 | `tests/block.rs` | 200×200 / 50 → 4×4 = 16 块，且 `covers()` 为真 |
| 2 | `tests/block.rs` | `block_size = 0` / 负 / NaN / inf → `None` |
| 3 | `tests/block.rs` | `block_size = 0.4` → 块数超 32 → `None`（不是截断） |
| 4 | `tests/block.rs` | `block_at` 在矩形外返回 `None` |
| 5 | `tests/block.rs` | 块矩形并集 == 总矩形（面积和相等、无重叠：逐格计数） |
| 6 | `tests/block.rs` | 边界的 4 个角点各落唯一一块（不吃双份） |
| 7-8 | `tests/sample.rs` | 常数高度块 → 任意点采样 == 常数；块外点 → `None` |
| 9-10 | `tests/sample.rs` | 线性斜坡块 → 采样与解析解误差 < 1e-4；NaN 坐标 → `None` 不 panic |
| 11 | `tests/sample.rs` | 跨块点在两块重叠处只取先命中的一块，结果确定（同点两次调用相等） |
| 12 | `tests/sample.rs` | 与 `r3e-nav::bake::patch_height` 20 点逐位一致（见 4.2） |
| 13 | `tests/sample.rs` | 法线：平地 → (0,1,0)；东西向斜坡 → 法线 x 分量符号正确且模长 1±1e-5 |
| 14 | `tests/lod.rs` | `lod_for` 阈值单调：距离升 → LOD 不降级为更细 |
| 15 | `tests/lod.rs` | `Lod::step()` 与 `LOD_STEPS` 一致（1/2/4） |
| 16 | `tests/lod.rs` | 0 块地形（空 `patches`）→ 所有采样返回 `None`，不 panic |

### 4.5 顺带必改的机械项

- `Cargo.toml` workspace 成员 18 → **19**。
- `xtask/tests_baseline.json` 新增 `r3e-terrain: 16`（**只许升不许降**；`check-tests` 会读这个条目）。
- `dep_rules.json` 三处（§3.1）。
- `cmd_spec/mod.rs` 加 `mod terrain;` + `terrain::ALL` —— **命令条目在 C06 才真正进表**；C01 只声明一个空 `ALL: &[CommandSpec] = &[]` 占位吗？**不**：空数组会让 `EXPECTED_COMMANDS` 不变而域数为 18，`gen-capabilities --check` 立刻红。故 **C01 不建 `cmd_spec/terrain.rs`**，域与命令全部推迟到 C06 一次性进（C06 是「判据测试与域登记」卡，本来就该它干）。C01 只做 crate。

---

## 5. C02：多分辨率 LOD 采样（tests **+14**）

### 5.1 设计

- 三级：`L0` 步长 1（全采样）、`L1` 步长 2、`L2` 步长 4。
- 选择：`lod_for(distance)` 纯函数，阈值常量 `LOD_DISTANCES = [50.0, 120.0]`（米），可配（`LodConfig`）。
- 采样：`sample_lod(patches, x, z, lod)` 在块内按 `step` 跳采样并插值。**不是**「降分辨率量化的最近点」——降分辨率量化会在接缝处产生跳变，接缝断言会立刻红。

### 5.2 接缝规则（本卡唯一容易出错的地方）

相邻两块共享边界线。若两侧按各自 `lod` 独立跳采样，边界样本会落在不同位置 → 出现裂缝。
规则：**边界行/列强制用 `L0`（步长 1）样本**，块内才按 `lod` 跳采。代价是边界多 4×(N+1) 次采样，可忽略；换来「任意两个不同 LOD 的相邻块，接缝处采样差 == 0」。

### 5.3 14 条测试

| # | 断言 |
|---|---|
| 1-3 | 同一斜坡地形：`L0/L1/L2` 采样在块内点上的最大绝对误差 ≤ 0.5m（三级各 1 条） |
| 4-6 | 相邻块 `L2` vs `L1`、`L2` vs `L0`、`L1` vs `L0` 在边界线上逐点差 == 0（三级组合各 1 条） |
| 7 | 同一 `(x,z)` 同 `lod` 两次调用逐位相等（确定性） |
| 8 | `lod` 越界（��造枚举值不可行 → 用 `Lod::from_step(3)` 返回 `None`） |
| 9 | `LodConfig` 阈值非法（负数 / 非递增）→ 构造返回 `None` |
| 10 | 平地 + `L2`：所有采样 == 常数（跳采样不引入插值误差） |
| 11 | 单块地形（1×1 网格）：`L2` 不越界、不 panic |
| 12 | 地形总矩形小于一块（如 30×30 / 50）→ 块数 = 1，LOD 正常 |
| 13 | `lod_for` 在阈值点上归到「更细」一侧（`<=` 而非 `<`，边界明确） |
| 14 | 采样点落在两块重叠区域时，`lod` 不影响命中块的选择（同块同点） |

### 5.4 实施记录（2026-11-22 完成，提交 `c2d533b`）

**落地形态**

```rust
pub fn sample_height_lod(
    patches: &[TerrainPatch],
    grid: &BlockGrid,
    x: f32, z: f32,
    lod: Lod,
) -> Option<f32>
```

**与 §5.1 的唯一偏差**：签名比计划多了 `grid: &BlockGrid`。

原因：`TerrainPatch` 自己的采样网格（`resolution` / `size`）与 `BlockGrid` 的块边界**不整除**
（200 米 / 128 格 = 1.5625 米一格，块边长 50 米），只拿 `(x, z, lod)` 无法界定「这块是哪一块」
更无法界定「边界带有多宽」，§5.2 的接缝规则就落不了地。测试 4–6 也因此改成「取块 `(0,0)` 与
`(1,0)` 的共享边界 x = 50 上 5 个点，两个不同档位逐点 `to_bits` 相等」——比原计划的
「边界线上逐点差 == 0」更严（要求逐位，不是浮点近似）。

**边界带宽度**取「粗档一个顶点跨度」= `patch 采样步长 × lod.step()`（默认约 6.25 米），
不取固定米数：这样无论块边长与采样步长是否整除，边界两侧至少各有两排细档样本，
接缝位置必然对齐。代价是块边缘 6.25 米内用全采样，块中心 43.75 米仍按粗档——LOD 的意义保留。

**细档不另写插值**：`step == 1` 时直接转调 `TerrainPatch::sample_height`（C01 上移的那份），
避免「LOD 细档」与「全采样」成为两套实现、日后单边改动造成档位间漂移。测试 1 用
`err == 0.0`（逐位）钉住这件事，而不是「误差 ≤ 0.5 米」。

**判据（14 条全部通过）**：块内误差 L0/L1/L2 各 1（对全采样参考 ≤ 0.5 米，实测 L0 = 0、
L1/L2 在 1.5625 米步长、0.02 米/米缓坡 + 1.5 米正弦的夹具上均 < 0.01 米）、接缝 3（三档两两组合）、
确定性 1、非法步长/序号 1、`LodConfig` 非法阈值 1、平地 L2 常数 1、单块 1×1 网格四角 1、
30×30 地形切一块 1、`lod_for` 阈值归细档 1、重叠区换档不改命中层 1。

**门禁数字**：`r3e-terrain` 16 → **30**（`tests/lod_sample.rs` 14 条），全局 2166 → **2180**；
`size_debt` 20 不变（两文件均远低于 800 行）；`dep_rules` 仍 19 个 crate、`dep_debt` 0；
能力面零变化（commands 398 / rpc 68 / domains 17 / invariants 40）；回放金标 `385482eb033d2964` 未漂移。

---

## 6. C03a：分块网格 + 4 层材质权重（tests **+14**）

### 6.1 产出物（纯数据，进渲染前可完全 headless 判定）

```rust
pub struct TerrainMesh {
    pub positions: Vec<[f32; 3]>,   // (cols+1)*(rows+1)
    pub normals:   Vec<[f32; 3]>,
    pub uvs:       Vec<[f32; 2]>,
    pub weights:   Vec<[f32; 4]>,   // 4 层混合权重，和 == 1
    pub indices:   Vec<u32>,        // cols*rows*6
    pub lod:       Lod,
}
```

`r3e-terrain` **不认识** `r3e-render` 的任何类型：它的输出是裸 `Vec`，`r3e-core::mesh::MeshData` 的转换由 `r3e-runtime` 装配层做（§3.1）。

### 6.2 权重语义（不引入新数据源）

4 层权重由高度 + 坡度程序化判定，**不读贴图、不读新资源**：

| 层 | 判据（默认） | 落地 |
|---|---|---|
| 0（草） | 坡度 ≤ 25° 且高度 < 80m | `TerrainLayerConfig` 可配 |
| 1（岩） | 坡度 > 25° | 同上 |
| 2（沙） | 高度 ≤ 5m | 同上 |
| 3（雪） | 高度 > 80m | 同上 |

权重归一化：算出 4 个原始分后除以和；**和为 0 时（配置重叠导致全负）退化为层 0 = 1**，并有一条测试专门打这个退化分支。
层材质（基色/粗糙度）**引用既有 `SceneMaterialDecl` 名字**（M1-S4-A 已有），名字缺省时用常量色——不新建材质类型。

### 6.3 14 条测试

| # | 断言 |
|---|---|
| 1 | 顶点数 = `(cols+1)*(rows+1)`（对 L0/L1/L2 三级各验一次 → 1 条参数化循环仍算 1 个 `#[test]`） |
| 2 | 索引数 = `cols*rows*6`，且所有索引 < 顶点数 |
| 3 | 三角数 = 索引数 / 3，且 `L2` 的三角数 = `L0` 的 1/16（±4 接缝修正） |
| 4 | 每个顶点 `weights` 和为 1（误差 < 1e-6） |
| 5 | 权重和退化分支：配置使 4 层全不满足 → 层 0 = 1 |
| 6 | 法线全部单位长（误差 < 1e-5）；平地法线 = (0,1,0) |
| 7 | uv 在 `[0,1]` 内且角点对齐 (0,0)/(1,0)/(0,1)/(1,1) |
| 8 | 索引绕序：任取 100 个三角，叉积与法线同向（面朝上） |
| 9 | 网格顶点高度与 `sample_lod` 在同一点一致（误差 < 1e-5）——网格不是另算一遍 |
| 10 | `patch` 缺失（块无对应 `TerrainPatch`）→ 返回 `None`，不生成空网格 |
| 11 | `block_size` 非整数（50.0 除外，如 33.3）仍能生成（网格列数取整规则明确，不 panic） |
| 12 | `Lod::L2` 下接缝顶点与相邻块 `L0` 边界顶点逐位相等 |
| 13 | 空地形 → `mesh()` 返回 `None` |
| 14 | 同输入两次生成，`positions/indices/weights` 逐位相同（确定性） |
| 15 | 顶点落在采样格点上：非整除夹具（20 / 50）下每个顶点 XZ 都是格距整数倍或块边界（2026-11-24 补，见 §6.5） |
| 16 | 三角面贴合真实地形面：重心 + 三边中点探针偏差 ≤ 0.01 米（同上） |
| 17 | 异档接缝在**边中间**采样也闭合：401 点密采样 L2-vs-L1 ≤ 0.35 米且裙边深于落差（同上） |
| 18 | 裙边顶点在块边界下方且三角竖直朝外（同上） |
| 19 | 相邻档位边界顶点互相包含：L1 ⊆ L0、L2 ⊆ L1（同上） |

### 6.4 实施记录（2026-11-22 完成，提交 `01fea8b`）

**落地形态**（`crates/r3e-terrain/src/mesh.rs`，公开导出 `block_mesh` / `TerrainMesh` / `TerrainLayerConfig`）

```rust
pub fn block_mesh(
    patches: &[TerrainPatch], grid: &BlockGrid, index: [u32; 2],
    lod: Lod, cfg: &TerrainLayerConfig,
) -> Option<TerrainMesh>

pub struct TerrainMesh { positions, normals, uvs, weights, indices, lod, segments }
```

`segments: [u32; 2]` 是方案 §6.1 之外的**新增字段**：它把「列数 × 行数」这个本来只能
从顶点数反推的量显式带出来。测试 1/2/3/11 都要用它核对形状，外部调用方（C03b 建索引缓冲、
C04 场地查询）也不必再自己反推。

**段数规则**（方案只说「取整规则明确」，这里定死）：`cols = ceil(块宽 / (patch 采样步长 × lod.step()))`，
行同理。于是 50 米块 + 1.5625 米步长时 `L0/L1/L2` = 32/16/8 段，三角数比正好 1:4:16，**不需要接缝修正**——
方案 §6.3 第 3 条给的「±4 接缝修正」容差实测只用于尾块（非整除块边长）取整差，不是 LOD 本身要修的。

**角度侧一个方案外的取舍**：法线不走 `sample_normal`，改为网格自身中心差分（边缘用单侧顶替）。
`sample_normal` 在块边界处会跨块取样，粗档段边缘的法线就与相邻细档块的法线对不上——
这正是「网格法线要跟本块顶点密度一致」的由来。代价是法线精度等于当前档位精度，符合 LOD 本意。

**权重口径**（§6.2 的「原始分」在这里落地成带过渡带的线性分）：

```
ramp(v) = clamp(v / fade, 0, 1)
草 = ramp(grass_slope_max - 坡度) × ramp(grass_height_max - 高度)
岩 = ramp(坡度 - rock_slope_min)
沙 = ramp(sand_height_max - 高度)
雪 = ramp(高度 - snow_height_min)
weights = 四个分 / 其和；和 <= 0 或非有限 → [1,0,0,0]
```

`fade` 默认 6（坡度用度、高度用米共用一个数值窗口，够用且只有一个旋钮）。默认配置下
平地（高度 0、坡度 0）落在草沙混合（约 0.55 / 0.45）而非纯沙——这是刻意的：低地本来就该是
「沙往草地过渡」的样子，测试 5 用反向对照钉住它（平地不得掺岩掺雪、但草沙都在）。

**测试 11 的夹具偏差**：方案写 `block_size = 33.3`，实测 `BlockGrid::new` 直接返回 `None`——
200 / 33.3 向上取整是 7，7×7 = 49 > `MAX_BLOCKS`(32)，块数上限先拒了。改用 **40.4 米**
（5×5 = 25 块，仍不整除 3.125），断言「顶点铺满整块」比原方案更实。

**门禁数字**：`r3e-terrain` 30 → **44**（`tests/mesh.rs` 14 条），全局 2180 → **2194**；
`size_debt` 20 不变（`mesh.rs` 约 300 行、测试约 300 行）；`dep_rules` 仍 19 个 crate、`dep_debt` 0；
能力面零变化；回放金标 `385482eb033d2964` 未漂移。

### 6.5 修订记录：顶点格点对齐 + 边界带 + 裙边（2026-11-24，提交 `fc50e9c`）

**触发**：H-01B 手工实验暴露现象②「灰色坡面穿透」与现象④「块间接缝错位」。

**根因**：§6.4 的段数规则只保证「块边长被切成整数段」，没保证「切点落在采样格点上」。
50 米块配 20 米格距时，32 段的切点是 16.67 / 33.33 米，三角面横跨 x = 20 米的
地形折线，面中间按线性插值算，比真实地形高出数米——**这是插值模型错，不是精度差**：

| 旧口径复算（folded 夹具） | L0 | L1 | L2 |
|---|---|---|---|
| 三角面内最大偏差 | 3.67 米 | 3.67 米 | 7.33 米 |
| L2-vs-L1 边中间落差 | — | — | 5.50 米 |

现象④同源：L2 的切点与相邻 L0 的切点互不包含，缝上高度对不上，肉眼即横缝错位
（实测 24 条块缝里只有横缝 z = -50 一条错位 7.30 米，正交方向恰好整除因此无症状）。

**修复**（`crates/r3e-terrain/src/mesh.rs`）：

- `axis_coords`：块内顶点改取 patch 采样格点的整数倍（粗档间隔 = 格距 × `lod.step()`），
  两端各补一个最近的细档格点形成 1 格宽**边界带**，块边界本身必取；坐标数超
  `MAX_AXIS_POINTS` 的病态输入退化成均匀切分（不 panic，也不影响主循环）。
  于是邻近档位顶点天然嵌套，缝两侧顶点逐位相等，§6.3 第 12 条从「碰巧成立」变成结构保证。
- `append_skirt`：四周加一圈竖直裙边兜住残余精度差，深度 = 粗档间距 × 1.5，夹在
  `[0.5, 6.0]` 米；`TerrainMesh` 新增 `body_vertices / body_indices / skirt_depth`
  三个**纯新增**字段（既有签名不变），并加 `skirt_triangles()` 便于判据计数。
- `r3e-runtime` 的 `terrain_draw` 合并几何判据同步区分：本体三角面朝上、裙边三角面竖直，
  各自与 `block_mesh` 计数对齐。

**判据**：`tests/mesh.rs` 14 → **19** 条，补的 5 条（§6.3 第 15~19 条）专盯「面内」与
「档位交界」，且全部用**非整除夹具**（20 / 50）：旧实现下 16 条红（3.67 米 ≫ 1 厘米）、
17 条红（5.5 米 ≫ 0.35 米），不会因为夹具凑巧整除而假绿。

**门禁数字**：`r3e-terrain` 59 条（`tests/mesh.rs` 19 条），全局 **2338**；`size_debt` 不变；
`dep_rules` / 能力面零变化；回放金标未漂移。

---

## 7. C03b：渲染通道 + 4 层混合着色（tests **+12**）

### 7.1 为什么必须独立管线（实测依据）

`mesh.rs` L11：location `0..=15` 共 16 格，顶点侧已占 6、实例侧占 9，**只剩 15**。地形要 4 层权重（1 个 `Float32x4`）刚好能塞进 15——但只要将来地形再加上「层索引」或「高度」就已无格。更关键：地形顶点布局与实例化 PBR 顶点布局**语义不同**（地形不做蒙皮、不做实例变换矩阵，但有逐顶点混合权重），硬塞要改既有着色器的每一个顶点入口。

结论：**新建 `terrain` 管线**——自有 `VertexBufferLayout`、自有 `wgsl`、自有 bind group。PBR 管线一行不改。

### 7.2 文件与预算

| 文件 | 预算 | 内容 |
|---|---|---|
| `crates/r3e-render/src/terrain.rs` | ~340 | 管线创建、顶点/索引缓冲、bind group（4 层材质参数）、`draw_terrain` |
| `crates/r3e-render/src/terrain.wgsl` | ~140 | 顶点（pos/normal/uv/weights）+ 片元（4 层按权重混合 lambert） |
| `crates/r3e-runtime/src/terrain_scene.rs` | ~260 | **装配层**：`Scene.terrain` → `r3e_terrain::mesh` → `r3e_render::terrain`；`app.rs` 只加 1 行 `mod` + 1 行调用（增量 ≈ 2 行） |
| `crates/r3e-render/src/lib.rs` | 56 → ~62 | `pub mod terrain;` 与再导出 |

`app.rs` 增量必须 ≈ 0（在 `size_debt` 里，见 §2.3）。

### 7.3 12 条测试（headless 能测什么，就只测什么）

无 GPU 环境下**不能**断言像素。本卡的可自动判定面：

| # | 断言 |
|---|---|
| 1 | terrain 顶点布局的 location 集合与 PBR/实例布局**不相交**（静态常量比对，防未来手滑复用） |
| 2 | `TerrainVertex` 的 `array_stride` == 3*4 + 3*4 + 2*4 + 4*4 = 48（字节，防填充漂移） |
| 3 | 权重在着色器输入端已归一化（CPU 侧打包函数断言：入参和 ≠ 1 时被打包函数纠正） |
| 4 | 从 `TerrainMesh` 到 GPU 顶点数组的转换：顶点数、索引数守恒 |
| 5 | 索引类型为 `u32`（地形顶点数可超 65535：200m/0.78m ≈ 256 列 → 66k > u16）→ 溢出场景 1 条 |
| 6 | 4 层材质 bind group 布局：缺省材质名 → 常量色，不 panic |
| 7 | `Scene` 无地形 → 装配层不创建管线（零地形零开销） |
| 8 | `Scene` 有地形但块无 patch → 跳过该块，其余正常 |
| 9 | LOD 变化时顶点缓冲重建次数 = 变化块数（记录计数，1 次 rebuild/块） |
| 10 | 装配层传参：`block_size` 从 D10 配置读，非法配置 → 装配跳过并记 1 条 warn 事件 |
| 11 | `terrain.wgsl` 能被 `naga` 解析（`wgsl` 语法校验，无需 GPU） |
| 12 | 着色函数 CPU 等价实现（`mix_layers(weights, colors)`）在权重 = [1,0,0,0] 时输出 == 层 0 颜色 |

真实观感走 C07 的人工截图（`evidence/`）。

### 7.4 实施记录（2026-11-22 完成）

**落地文件**（全部新增，既有 PBR 管线零改动）

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-render/src/terrain.rs` | 473 | `TERRAIN_LAYER_COUNT` / `TERRAIN_VERTEX_STRIDE=48` / `TerrainVertex` + `TERRAIN_ATTRIBUTES`(location 0..=3) / `terrain_vertex_layout` / `normalise_weights` / `pack_vertices` / `needs_u32_indices` / `mix_layers` / `TerrainLayersUniform` + `layer_index` / `TerrainDrawItem` / `TerrainPass` |
| `crates/r3e-render/src/terrain.wgsl` | 86 | 顶点（pos/normal/uv/weights 四入口）+ 片元 4 层按权重混合 lambert |
| `crates/r3e-runtime/src/terrain_scene.rs` | 293 | 装配层：`TerrainScene` / `TerrainSceneConfig`(D10) / `TerrainBlock` / `TerrainPlan` / `TerrainSceneEvent` |
| `crates/r3e-render/src/lib.rs` | 65 → 71 | `pub mod terrain;` + 6 个再导出 |
| `crates/r3e-runtime/Cargo.toml` | +1 | `r3e-terrain`（allow 表已有该边，`dep_debt` 不动） |

**测试 13 条**（方案 12 条 + 装配侧多 1 条）

`tests/terrain_pass.rs`（8）：location 集合与实例布局不相交 / stride==48 且与 `wgpu::VertexFormat` 累加一致 / 权重归一化纠偏（含全零→层 0）/ 顶点与索引数守恒 / `needs_u32_indices` 在 65536 边界翻转 / 缺省材质名回落常量色不 panic / `TERRAIN_WGSL` 过 naga 解析 / `mix_layers` 在 `[1,0,0,0]` 输出层 0 基色。

`tests/terrain_scene.rs`（5）：空地形 → `plan` 为空且不建管线 / 块无 patch → 跳过该块 / LOD 变化才重建（重建计数）/ D10 非法 `block_size` → 空计划 + 1 条 warn 事件 / 装配结果自洽（块数 4×4=16、顶点数与 `block_mesh` 一致）。

**两处方案外取舍**

1. **帧序列接线推迟到 C07**。`Renderer` 的入口签名改动量（`frame()` 要新增地形切片参数并波及所有调用点）远超 §7.2 给的「app.rs ≈ 2 行」预算，且在无 GPU 的 CI 上无法验，硬接只会把 C03b 的判定面搅浑。本卡交付「通道可用 + 装配纯数据可判」，`TerrainPass::draw` 由宿主在 C07 的 `terrain_roam` 里持有并调用——C07 的截图回归才是接线的真判据。
2. **LOD 重建口径**（测试 9 原本写「far.rebuilds == 16」）。实测 16 块里最远的 3 块首帧就已是 L2（块中心距 > 200m），相机拉到 5000 米外时档位不变、不该计重建，实际是 13。改成 `rebuilds == 块数 − 首帧已是 L2 的块数`——这比写死 16 更贴 LOD 语义，也防未来改阈值时测试假通过。

**门禁数字**：`r3e-render` 316 → **324**、`r3e-runtime` 14 → **19**，全局 2194 → **2207**（`check-tests` 全绿）；`size_debt` 20 不变（三个新文件分别为 473/86/293 行，均低于 800 阈值）；`dep_rules` 19 个 crate、`dep_debt` 0；能力面零变化（不进 commands/rpc）；回放金标 `385482eb033d2964` 未漂移；`cargo xtask verify` 11 步全绿。

---

## 8. C04：地形碰撞（双后端各验，tests **+20**）

### 8.1 落点（CD-5 档 B：独立地形场，不碰 `ColliderShape`）

`ColliderShape` 现只有 `Box/Sphere/Capsule/Cylinder`，加 `HeightField` 变体会让 `builtin_backend.rs` / `rapier_backend.rs` / `types.rs` 的**所有 shape 匹配分支**都要改（含解析、接触、射线、扫掠），风险与工作量都最高。改走独立通道：

| 文件 | 预算 | 内容 |
|---|---|---|
| `crates/r3e-physics/src/heightfield.rs`（新） | ~260 | `HeightField { origin: Vec3, cell: f32, cols, rows, heights: Vec<f32> }` + `height_at(x,z)` 双线性 + `normal_at` + 校验（尺寸/有限/`cols*rows == heights.len()`） |
| `crates/r3e-physics/src/port/heightfield.rs`（新） | ~120 | `HeightFieldPort`：`set_heightfield(&mut self, Option<HeightField>)`、`heightfield(&self) -> Option<&HeightField>`，默认实现空 |
| `src/builtin_heightfield.rs`（新） | ~200 | builtin 后端的 `sample_ground` 优先查地形场 → 回落 `set_ground` 平面 → 回落物体顶 |
| `src/rapier_heightfield.rs`（新） | ~160 | rapier：`HeightField3d` → 静态刚体（`BodyKind::Static`），`sample_ground` 用 `cast_ray` 竖直向下 |
| `src/lib.rs` | 1924（在 `size_debt`）**只加 `pub use`** | 不得追加实现代码 |

**为什么 `r3e-terrain` 不能直接进来**：`physics.allow` 只有 `r3e-core`。装配层（`r3e-runtime::terrain_scene`）把 `Vec<TerrainPatch>` 摊平成 `HeightField` 后注入物理，物理侧只见裸数组——这条边界同时满足「双后端可验」与「物理不依赖地形」。

### 8.2 20 条测试（10 断言 × 2 后端）

测试放 `crates/r3e-physics/tests/heightfield_builtin.rs` 与 `heightfield_rapier.rs`，同一组断言各写一遍（**按卡表「双后端各验」**）。

| # | 断言 |
|---|---|
| 1 | 无地形场：`sample_ground` 回落 `set_ground(0.0)` 平面，返回值与现行为一致（防回归） |
| 2 | 常数高度场（h=3）：任意点 `sample_ground` == 3（误差 <1e-4） |
| 3 | 30° 斜坡：高度与解析解误差 < 1cm |
| 4 | 跨块接缝：两块拼合处左右 0.1m 两点高度差 < 1e-4 |
| 5 | 射线竖直向下打斜坡：命中点误差 < 1cm |
| 6 | 射线射向天空：`None`（不得回落到平面） |
| 7 | 胶囊扫掠落地：落点不穿透地形（`y >= height`） |
| 8 | 台阶 0.3m：可跨越（与 B03 的 `max_step` 行为一致） |
| 9 | 60° 陡坡：`grounded = false`（超 `max_slope_deg`） |
| 10 | 清空地形场：回落平面，`body_count` 减少到清空前（静态体已销毁） |

两个后端同一断言失败即红；若 rapier 与 builtin 差 >1e-3，视为后端语义分叉（§15 R-3），必须在卡内解决并记录实测差。

### 8.3 实施记录（2026-11-22 完成）

**落地文件**

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-physics/src/heightfield.rs`（新） | 286 | `HeightField { origin, cell, cols, rows, heights }`：`new` 逐项校验（尺寸为 0 / 非有限 / `cols*rows != heights.len()` 一律 `Err`，非法构造不覆盖旧场）、`height_at` 双线性、`normal_at` 中心差分、`peak_over`、`aabb`、`covers`（边界算场内） |
| `crates/r3e-physics/src/terrain_query.rs`（新） | 212 | 与后端无关的解析查询：`raycast`（按格子步进，起点在表下报重叠）、`capsule_overlap`（地表穿胶囊）、`capsule_sweep`（最短落地距离）、`merge_ground`（地形 / 后端样本取更高者） |
| `crates/r3e-physics/src/port_terrain.rs`（新） | 109 | 端口层合成：`nearer`（重叠优先 → 距离 → `node` 升序）、`port_hit`、`cast_ray` / `capsule_overlap` / `capsule_sweep` 的择一 |
| `crates/r3e-physics/src/backend.rs` | +17 | `PhysicsBackend` 两个**可选默认方法** `set_heightfield` / `heightfield`（默认什么都不做 / `None`，`Null` 后端据此天然拒绝） |
| `crates/r3e-physics/src/builtin_backend.rs` | +14 | `heightfield: Option<HeightField>` 字段；只存数据，查询侧统一由 `terrain_query` 合并 |
| `crates/r3e-physics/src/rapier_backend.rs` | +67 | `TerrainField { body, field }`：装场建一块**静态 `HeightField3d` 碰撞体**（`scale.y` 固定 1.0、`x/z` 取网格跨度、刚体平移到网格中心），卸场整块移除。它**不进 `records`**，所以不影响 `body_count()`、不参与「脚本建的刚体」遍历 |
| `crates/r3e-physics/src/lib.rs` | +40 | `pub mod heightfield` / `mod terrain_query` / `mod port_terrain` / `pub use HeightField`；门面 `set_heightfield` / `heightfield`；`sample_ground` 合成；`rebuild_backend` 切后端时把场原样搬过去 |
| `crates/r3e-physics/src/port_impl.rs` | 826 → 768 | 端口适配只留转发，合成下沉到 `port_terrain.rs`（见取舍 4） |
| `crates/r3e-physics/tests/terrain_field.rs`（新） | 442 | 26 条用例，双后端由 `selectable()` 循环覆盖 |

**测试 26 条**（方案 20 条 + 6 条；断言合并成一份、对后端循环）

- 数据契约 6 条：非法尺寸 / 非有限高度 / 格数与样本数不符全拒且不覆盖旧场；`covers` 与 `peak_over`/`aabb` 含边界；同高度场两次构造逐位相等；平地表插值精确且法线朝上；斜坡法线与解析斜率一致；场外采样返回空。
- 装卸与切后端 4 条：装场不动刚体表与能力位；装场后切 builtin/rapier 场不丢；`Null` 后端装不上；清空后回落到地面平面。
- 双后端一致 1 条（内含 8 组断言）：常数场 / 斜坡解析解 / 接缝两侧 / 竖直射线命中点 / 天空无命中 / 胶囊扫掠落点不穿透 / 陡坡法线倾角 / 地形与刚体共存时射线取更近者，全部逐后端断言且互差 <1e-3。
- 地面采样 3 条：地形高于地面取地形、低于地面取地面、远离地形回落平面。
- 射线 / 胶囊 7 条：向下命中、起点在地表下报重叠、距离不足不命中、斜射进场命中、刚体近选刚体、地形近选地形、地表穿胶囊、悬空无重叠、向下扫掠给出落地距离、向上扫掠碰不到脚下地表、贴地扫掠报重叠、场外拿不到地形候选。
- 总闸 2 条：物理关闭时地形查询一律为空；未被地形覆盖时端口能力位不变。

**四处方案外取舍**

1. **不新建 `port/heightfield.rs` 与 `HeightFieldPort`**：改成在既有 `PhysicsBackend` 上加两个默认方法，与 `set_ground` / `set_ccd_default` 同款扩展方式。少一个 trait、少一套 DTO，门面 API 与 §8.1 写的完全一致，`Null` 后端的「不支持」也由默认实现自然给出。
2. **测试合并成单文件 + `selectable()` 循环**，而非 `heightfield_builtin.rs` / `heightfield_rapier.rs` 两文件各写一遍：同一份断言跑两遍才不会「改一边忘一边」——两套拷贝迟早正文漂移，而双后端分叉恰恰是本卡唯一想抓的东西（差 >1e-3 即红）。
3. **方案 #8 / #9（0.3m 台阶可跨、60° 陡坡 `grounded = false`）不在本卡判**：那是 `r3e-character` 的 `max_step` / `max_slope_deg` 判据，属于 B03 已验收的行为；物理侧本卡只保证「给出地表高度与法线」。已在 26 条里用「陡坡法线倾角」「胶囊贴地扫掠」占位，真正的行走判据留给 C07 的 `terrain_roam` 路点回归。
4. **`port_impl.rs` 一度顶穿 `lint-size`（826 > 800）**：`cargo xtask lint-size` 当场拦下，于是把「刚体候选 vs 地形候选择优」整块抽成 `port_terrain.rs`（109 行），`port_impl.rs` 回到 768 行、`size_debt` 白名单仍是 20 项且全部未上涨。同时把**地面合成从端口移到门面** `sample_ground`：否则「直接拿 `&Physics` 调」与「拿 `&dyn GroundPort` 调」两条路径会各算一遍，日后改一边漏一边的症状是「角色调用比端口调用低一截」。

**门禁数字**：`r3e-physics` 239 → **265**，全局 2207 → **2233**（`check-tests` 全绿）；`size_debt` 20 项不变；`dep_rules` 19 crate / `dep_debt` 0；能力面零变化（不动 commands/rpc/domains/invariants）；回放金标 `385482eb033d2964` 未漂移；`cargo xtask verify` 11 步全绿。

---

## 9. C05：CLI 生成与确定性哈希（tests **+10**）

### 9.1 通道（CD-7 档 A：不动 `args.rs`）

`args.rs` 实测 703 行、阈值 800、余 97 行。新增一个子命令（解析函数 + `Cmd` 变体 + dispatch）预算 60~90 行，落地后余量只剩个位数，D 阶段任何加子命令都会越线并被迫新增 `size_debt`（该账只许减）。

故 `terrain.gen` 走既有通用通道：

```powershell
r3e-cli rpc --method terrain.gen --args '{"size_x":200,"size_z":200,"seed":42}'
```

`args.rs` 已有 `rpc` 子命令与 `--args` 旗标（实测 L219 `Rpc {`、L41 `"args"`），**零改动**。

### 9.2 生成语义

- `generate(200, 200, 42)` → 4×4 块 × 64×64 采样（`block_size = 50.0`，`resolution = 64`，上限 128 内）。
- 高度 = 三层正弦叠加 + `SplitMix64(seed)` 微扰（幅度 ≤ 0.05m），高度 ∈ `[-20, 120]`。
- 输出：`Vec<TerrainPatch>`，由 command 层写入 `Scene.terrain`（复用 `r3e-doc` 的 `UpsertTerrainPatch` 撤销命令，**天然可撤销**）。
- 返回值：`r3e-doc::state_hash` 的 16 位十六进制（如 `385482eb033d2964` 同格式）。

`terrain.gen` 必须登记进 `TX_ONLY_METHODS`（`args.rs` L252），否则会被 `rpc` 绕过事务（B01 已有先例，一条字符串，无行数压力）。

### 9.3 10 条测试

| # | 断言 |
|---|---|
| 1 | 同 seed 两次生成：块数、bbox、全部高度样本逐位相同 |
| 2 | 不同 seed：至少 1 个样本不同（且整体哈希不同） |
| 3 | 生成后 `state_hash` 与「重放同样命令序列」得到同一哈希 |
| 4 | 生成的地形 `resolution` 全部 ≤ 128、`TERRAIN_MAX_PATCHES` 未超 |
| 5 | 块覆盖矩形并集 == 请求尺寸（200×200，无缺口） |
| 6 | 生成结果 `sanitized()` 后与生成前逐位相同（不依赖 `sanitized` 兜脏数据） |
| 7 | seed = 0 与 seed = u64::MAX 均不 panic 且可复现 |
| 8 | 尺寸非法（0×0 / 负数 / >1024）→ 返回 `false` 且场景不变 |
| 9 | 生成后既有非地形节点不变（只动 `terrain` 表） |
| 10 | `terrain.gen` 在 `TX_ONLY_METHODS` 中（防未来被 `rpc` 绕过事务） |

### 9.4 实施记录（2026-11-22 完成）

**落地文件**

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-terrain/src/generate.rs`（新） | 156 | `generate(size_x, size_z, seed) -> Option<Vec<TerrainPatch>>`：`BlockGrid` 切块 → 逐块按**整张地形统一网格**填样本 → 块名 `terrain_<列>_<行>`、`resolution = [64, 64]`。公开 `GENERATE_RESOLUTION` / `GENERATE_SAMPLES_PER_BLOCK` / `GENERATE_MIN_HEIGHT` / `GENERATE_MAX_HEIGHT` / `GENERATE_JITTER` / `MAX_GENERATE_SIZE` |
| `crates/r3e-terrain/src/lib.rs` | +6 | `mod generate;` + `pub use generate::{...}`（门面导出，模块私有） |
| `crates/r3e-terrain/tests/generate.rs`（新） | 333 | 10 条用例 |

**生成口径（两个「只看」）**

1. **基高度只看世界坐标**：三层正弦叠加（~200 / ~90 / ~33 米周期）。同一点必同值，与块划分无关。
2. **微扰只看全局样本索引**：`SplitMix64(seed ^ SplitMix64(gx<<32 \| gz))`，取高 53 位除 `2^53` 映射到 `[0,1)`，幅度 ±0.05 米。**不拿世界坐标喂噪声**——接缝两侧的 `x` 由两条不同浮点表达式算出，可能差 1 ulp，一旦进噪声就是两个不相干的高度，地面会裂出一条可走的缝。

采样点按 `gx / 总段数` 定位、`f64` 算完落 `f32`（`f32` 下大尺寸会出现「相邻两点算出同一坐标」，白丢一个采样点）；段数 = 采样点数 − 1，相邻块共享边界那一列样本，故接缝逐位相等。代价：整张尺寸非 `block_size` 整数倍时，末块内采样间距与「均匀分布」有极小偏差（裁掉的那一小段摊到 63 个间隔里），换绝对无缝，值这个价。

**10 条测试**

- 可复现 4 条：同 seed 两次生成块名/矩形/分辨率/全部高度 `to_bits` 逐位相同；换 seed 至少改一个采样点且整张指纹不同；`seed = 0` 与 `u64::MAX` 均可复现且互不相同（覆盖 `SplitMix64` 回绕）；整张 FNV-1a 指纹覆盖「块名 + origin + size + resolution + 逐样本位模式」，同输入恒等、异尺寸必不同。
- 覆盖与预算 3 条：200×200 / 50 米必为 4×4 = 16 块、块数不超 `MAX_BLOCKS` 与 `TERRAIN_MAX_PATCHES`、`resolution` 全 64 且样本数与 `resolution` 自洽；块面积和 == 40000 且逐块 `origin`/`size` 与 `BlockGrid` 裁定逐位一致；175 米非整除尺寸仍 16 块、面积和严丝合缝、接缝逐位相等。
- 口径与拒错 3 条：生成物 `sanitized()` 后逐位不变（不靠 sanitize 兜脏数据）；全部样本有限且在 `[-20, 120]` 内，且左右/上下相邻块共享边界样本列逐位相等；非法尺寸 0 / 负 / `NaN` / `±inf` / 超 `MAX_GENERATE_SIZE` 一律 `None` 不返回半张地形（1024×1024 因块数超限被拒，150×150 可生成）。

**一处方案调整：第 10 条挪到 C06**

方案 §9.3 第 10 条写「`terrain.gen` 在 `TX_ONLY_METHODS` 中」，实测做不到单卡闭环：`crates/r3e-cli/src/args/tests.rs::tx_only_tables_are_consistent` 要求护栏表里的每个名字**必须是已知协议方法**。C05 只加护栏而不动 `RPC_METHODS`，该测试当场变红；反过来先动 `RPC_METHODS` 又必须同卡改 `dispatch` 臂、宿主实现与 cmd_spec 表，那是 C06 的整块工作。

结论：**护栏与整条协议链路一起并入 C06**，C05 交付生成器本体（纯函数）+ 确定性口径，第 10 条替换为「175 米非整除尺寸仍无缝铺满」（价值不低于护栏：护栏迟早要加，接缝错位则是一张永久裂开的地形）。测试总数不变（方案 10 条 = 实际 10 条）。

**门禁数字**：`r3e-terrain` 44 → **54**，全局 2233 → **2243**（`check-tests` 全绿）；`size_debt` 20 项不变（新文件 156 / 333 行，均远离 800）；`dep_rules` 19 crate / `dep_debt` 0（`r3e-terrain` 仍只依赖 `r3e-core`）；能力面零变化（不动 commands/rpc/domains/invariants）；回放金标 `385482eb033d2964` 未漂移；`cargo xtask verify` 11 步全绿。

---

## 10. C06：判据测试与域登记（tests **+12** / invariants **+2** / domains **+1**）

### 10.1 命令面（8 条，全部带 RPC——理由见 CD-1）

| 命令 | RPC 方法 | 域 | 参数 | 返回 | 语义 | 进表卡 |
|---|---|---|---|---|---|---|
| `terrain_create` | `terrain.create` | Terrain | `&str`、`f64`×4（矩形）、`i64`（分辨率） | `bool` | 建块（写场景，可撤销） | C06 |
| `terrain_remove` | `terrain.remove` | Terrain | `&str` | `bool` | 删块 | C06 |
| `terrain_sample` | `terrain.sample` | Terrain | `&str`（块名或空）、`f64`×2 | `&str`（JSON：高度/法线/权重） | 只读采样 | C06 |
| `terrain_blocks` | `terrain.blocks` | Terrain | — | `&str`（JSON：块清单 + LOD 分布） | 只读块划分 | C06 |
| `terrain_resample` | `terrain.resample` | Terrain | `&str`、`f64`、`i64` | `bool` | 按 LOD 重采样写回（写场景） | C06 |
| `terrain_gen` | `terrain.gen` | Terrain | `i64`×2、`i64` | `&str`（哈希） | 生成整张地形（重） | C05/C06 |
| `terrain_stats` | `terrain.stats` | Terrain | — | `&str` | 块数/采样总数/三角总数 | C06 |
| `terrain.mesh` | `terrain.mesh` | **Meta** | JSON `params` | `&str`（网格统计） | 只读重计算，脚本侧不绑定 | C03a/C06 |

**为什么 `terrain.mesh` 挂 `Meta`**：脚本侧 `r3e-script` 的 allow 只有 `[r3e-core, r3e-doc, r3e-protocol]`，碰不到 `r3e-terrain`；网格生成无法在脚本侧实现。放进 `Meta` 域（只 RPC、无脚本函数）是既有机制（39 条同款）。其余 7 条脚本可见命令的实现只需 `core::TerrainPatch::sample_height` 与块索引（C01 已上移 core），脚本侧零新依赖。

### 10.2 2 条 invariant（`r3e-doc`）

| 码 | 类别 | 判据 | 反例测试 |
|---|---|---|---|
| `terrain.block_gap` | `Category::Terrain`（新变体） | 地形块划分必须被至少 1 块 patch 覆盖；块矩形并集无空洞（允许未覆盖 → 报 warn） | 删一块 → 报 1 条 |
| `terrain.weight_not_normalized` | 同上 | 由 `terrain.mesh` 产出的权重若和 ≠ 1 → 报错（校验器只吃 core 数据，不依赖 terrain crate：按高度/坡度配置重算） | 故意传非法配置 → 报 1 条 |

连带动作（B04 已有同款先例）：
- `r3e-doc::validate::Category` 加 `Terrain` 变体 + `as_str` + `from_code` 前缀映射（`terrain.` → `Terrain`）。
- `r3e-editor-host::verify::layer_of` 穷尽归类加 `Terrain`。
- `xtask invariants` 三源一致（登记表 / 反例测试 / 能力清单）→ 40 → **42**。

### 10.3 必须同卡同步的 7 处硬数字

| 位置 | 现值（实测） | 改为 |
|---|---|---|
| `xtask/src/gen_capabilities.rs` `EXPECTED_COMMANDS`（L41） | 398 | **406** |
| `r3e-protocol/src/lib.rs` `RPC_METHODS`（L129） | 68 | **76** |
| `r3e-protocol/src/api.rs` `EngineApi` 方法数（694 行文件内） | 68 | **76** |
| `r3e-core/src/cmd_spec/mod.rs` 测试断言 `rpc == rpc_only + 29`（L~末） | 29 | **36**（7 条脚本可见 → 29+7；`terrain.mesh` 进 Meta → rpc_only 39→40，76−40=36） |
| `r3e-editor-host/tests/cmd_spec_contract.rs` 文档头 L1（文字仍写「62 个方法」，B01 漏改文案）+ 等值断言 L102 | 文字 62 / 断言 68 | **文字 76 / 断言 76**（顺带修正 L1 文案） |
| `r3e-editor/tests/cmd_spec_contract.rs` 下限断言（L102）+ 等值断言 | 68 / 68 | **76 / 76** |
| `r3e-script/tests/cmd_spec_contract.rs` L98/L103/L106/L109 | 359 / 370 / 359 | **366 / 377 / 366**（7 条脚本命令、无重载）；L98 注释「17 个域文件」→ **18 个** |

漏一处 `cargo test` 立刻红，且报错文案已写清「改命令表须同步本常量与 `docs/m0/M0S5_PROTOCOL.md` §3」——`M0S5_PROTOCOL.md` §3 的命令表清单也要同卡补 8 行。

**还有一处非表格化的同步点**：`domains` 计数的**枚举列举处**（`gen_capabilities` 汇总 `Domain` 变体时的那一处）本次未逐行定位；C06 开工第一步先跑 `cargo xtask gen-capabilities --check` 让它把期望值吐出来，再按报错定位改动——**不猜位置**，避免改错地方后靠试。

### 10.4 C06 的 12 条测试

| # | 断言 |
|---|---|
| 1-2 | `terrain_create` 合法参数建块成功 / 非法分辨率（0、129）→ `false` 且场景不变 |
| 3 | `terrain_create` 超 `TERRAIN_MAX_PATCHES` → `false`（不静默覆盖） |
| 4 | `terrain_remove` 存在 → `true`；不存在 → `false` |
| 5 | `terrain_sample` 返回 JSON 可解析且含 `height/normal/weights` 三键 |
| 6 | `terrain_sample` 空名 + 坐标 → 在所有块中命中正确块 |
| 7 | `terrain_blocks` 返回 16 条（200×200/50）且含 LOD 分布 |
| 8 | `terrain_stats` 三角总数 == 各块 `terrain.mesh` 三角数之和 |
| 9 | `terrain.mesh` 返回 JSON 含 `triangles/vertices/lod` |
| 10 | 8 条命令全部出现在 `capabilities.json` 且域为 `terrain`（除 `terrain.mesh` 为 `meta`） |
| 11 | `domains` 列表含 `terrain`，且总数为 18 |
| 12 | `terrain.block_gap` 反例：删块后校验报 1 条 `terrain.block_gap` |

### 10.5 实施记录（2026-11-23 完成）

**落地文件**

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-core/src/cmd_spec/terrain.rs`（新） | 105 | 8 条 `CommandSpec`：全部带 rpc（CD-1 档 A），`terrain.mesh` 挂 `Domain::Meta`（脚本侧碰不到 `r3e-terrain`） |
| `crates/r3e-core/src/host/terrain.rs`（新） | 371 | `TerrainHost` trait + DTO + 空实现（后端不支持时天然回 `none` / 0，不 panic） |
| `crates/r3e-protocol/src/terrain.rs`（新） | 104 | 8 个方法的请求/响应 DTO |
| `crates/r3e-protocol/src/dispatch/terrain.rs`（新） | 245 | 8 个 dispatch 臂，插入 `character → terrain → nav` 回落链（`character.rs` 尾部改指向 `terrain`） |
| `crates/r3e-runtime/src/terrain_ops/mod.rs`（新） | 593 | 引擎侧实现主体 + `TerrainOpError` + `mesh_totals`（`stats` 与 `mesh` 共用的唯一算法） |
| `crates/r3e-runtime/src/terrain_ops/read.rs`（新） | 200 | 只读四法：`sample` / `blocks` / `stats` / `mesh`，一个字节都不写 |
| `crates/r3e-runtime/src/terrain_ops/write.rs`（新） | 227 | 写四法：`create` / `remove` / `resample` / `gen`，一律经 `CommandKind` 进撤销栈与 `state_hash` |
| `crates/r3e-runtime/src/terrain_host.rs`（新） | 321 | 真 `TerrainHost` 适配器；`app.rs` 只调用（见下文取舍 4） |
| `crates/r3e-editor-host/src/api/terrain.rs`（新） | 229 | 宿主层：参数整形 → 调 `terrain_ops` → 拼 JSON / 映射错误码（`TERRAIN_PATCH_MISSING` / `-32602` / `tx_rejected`） |
| `crates/r3e-script/src/api/terrain.rs`（新） | 312 | 脚本 7 条函数（`terrain.mesh` 不绑定） |
| `crates/r3e-doc/src/validate/terrain.rs`（新） | 151 | 2 条判据：`weight_not_normalized`（判根因：样本数与分辨率不符 / 含非有限值）、`block_gap`（格级空洞判 `Warn`，`COVER_STEP = 8m` 采样口径） |
| `crates/r3e-doc/tests/terrain_issues.rs`（新） | 110 | 5 条判据测试 |
| `crates/r3e-editor-host/src/api/tests/terrain.rs`（新） | 362 | 12 条宿主测试 |
| `crates/r3e-script/tests/terrain_api.rs`（新） | 419 | 12 条脚本绑定测试 |

同步修改：`cmd_spec/mod.rs`（`Domain::Terrain` + `terrain::ALL` + 断言 29→36）、`doc/validate/mod.rs`（三入口都调 + `Category::Terrain` 前后缀映射）、`editor-host/verify/mod.rs`（`layer_of` 归 `Static`，理由写进注释）、`protocol/lib.rs` `RPC_METHODS` 68→76、`protocol/api.rs` 68→76、`cli/args.rs` `TX_ONLY_METHODS` +4（`terrain.create` / `remove` / `resample` / `gen`）、`gen_capabilities.rs` 398→406 + `DOMAIN_ORDER` / `domain_name`、四个契约测试（`editor-host` 62/68→76、`editor` 68→76、`script` 359/370→366/377 与域文件 17→18、`core` rpc_only 断言 29→36）。

**测试 57 条**（方案 +12，实到 +57——偏差见取舍 1）

| crate | 条数 | 内容 |
|---|---|---|
| `r3e-runtime` | +25 | `terrain_ops/mod.rs` 16：建后采样为平地、同名覆盖且回报「非新建」、非法分辨率/零尺寸拒、第 33 块拒但覆盖允许、删块可撤销、块外采样**不是**零高度、`blocks` 报尺寸与覆盖率、空地形是空视图不是错误、`gen` 确定性并落场景、`gen` 拒非法尺寸、`resample` 减半并回报误差、平坦块重采样不写场景、`stats` 三角数 == 逐块网格之和、粗档顶点更少、LOD 就近取合法档、网格包围盒覆盖块矩形；`terrain_host.rs` 9：后端名非 `none`、平坦采样命中且权重归一、块外回「没命中」而非零高度地形、空地形统计全零、块划分覆盖数不超总数、同种子块名可复用、尺寸非法回 `none`、平坦块重采样不算改动、空实现与真实现可区分 |
| `r3e-editor-host` | +12 | 8 条命令各 1 条（`create` 回带哈希、`remove` 存在 `true` / 不存在 `false`、`sample` 含高度法线权重三键、空名跨块命中最高层、`blocks` 按块边长切 16 块、`stats` 三角数 == 逐块和、`mesh` 含三角/顶点/档位、`gen` 同种子一致）＋ 非法分辨率报参数错误且场景不变 ＋ 超上限报错不静默覆盖 ＋ 事务未决回 `tx_rejected` |
| `r3e-script` | +12 | `create` 写入且同名覆盖、`create` 拒非法参数、`remove` 幂等、`sample` 透传并回 JSON、空名请求「任意块」、未知块回空样本、`blocks`+`stats` 委托、`gen` 全成或全不成、后端拒绝时回 0 块、`resample` 只在真变时写、`resample` 拒未知块与非法上限、`NullBackend` 下建删仍可用 |
| `r3e-doc` | +5 | 样本数与分辨率不符报 error、非有限高度报 error、块间空洞报 warn、干净地形静默、空地形静默 |
| `r3e-core` | +3 | `TerrainHost` 契约：空实现一律回空且不 panic、空实现可 downcast、闭包即适配器 |

**四处方案外取舍**

1. **tests 实到 +57，超卡片预算 +12**。§10.4 那 12 条是站在「命令面 + 域登记」视角写的，实际每条命令要穿过五层（core 命令表 / protocol 方法表 + dispatch + DTO / runtime `terrain_ops` / 编辑器宿主 / 脚本绑定），任何一层没有独立测试，改一层就会在别处静默错位——八条命令 × 五层已是 40 条打底，再加 2 条 invariant 反例与 3 条 `TerrainHost` 契约。C 阶段 tests 因此变成 2150 → **2300**（+150，预算 +108）；卡表判据写的是「≥」，未失守，但**后续 D 阶段排预算时应按「命令数 × 层数」估，而不是按卡表给的 +N**。
2. **`terrain_ops` 落 `r3e-runtime` 而非 `r3e-editor-host`**。`editor-host` 的 allow 表不含 `r3e-terrain`（CD-2 红线），而八条命令里一半要碰块划分 / LOD / 三角网。放 runtime 后，编辑器宿主与脚本宿主共用同一份翻译；`terrain.stats` 与 `terrain.mesh` 也因此被强制走同一个 `mesh_totals`（两处算法分叉测试立刻红）。
3. **`M0S5_PROTOCOL.md` §3 没有逐条命令表**。该节是 `capabilities.json` 的格式规格 + counts 演进注记，逐条清单本就由生成器落 `docs/capabilities.json`。故本卡只在那段注记后追加一条 C06 的 counts 演进（406 / 76 / 330 / 18 / 42），**不手抄八条命令表**——会和生成器打架的表是负资产。
4. **`app.rs` 2821 → 2816（顺带还了 5 行债）**。装配调用改由 `crate::terrain_host::script_host` 提供后，`app.rs` 只剩一次调用 + 两行注释；`size_debt` 中该文件基线据实下修为 2816（原稿写 2813，是我第一次压行后的值，`cargo fmt` 又把四参数折回多行、实测 2816）。教训记一笔：**在 `size_debt` 文件里做「压行」型优化，必须以 fmt 后的行数记账**，否则 `lint-size` 会误报「上涨」并浪费一轮 `verify`（本轮实测 `verify` 单次 435s）。

**另两处台账同步**：`docs/m1/M1_PLAN.md` 的 `r3e-script` 写点数 138 → **140**（`api/terrain.rs` 的 `terrain_create` / `terrain_remove` 经 `ctx.write` 直写场景——脚本侧没有撤销栈，这几条命令 `reversible=false`，只能直写，与 `physics` / `character` 域同口径，M1-S3 收口时一并归并）；`docs/m0/M0S2_API_SPLIT.md` 保持 M0 拆分期实测原貌不动（它是历史快照，不是口径表）。

**门禁数字**：`r3e-core` 643 → **646**、`r3e-doc` 164 → **169**、`r3e-editor-host` 134 → **146**、`r3e-runtime` 19 → **44**、`r3e-script` 132 → **144**，全局 2243 → **2300**（`check-tests` 全绿）；`gen-capabilities` commands 398 → **406**、rpc 68 → **76**、`script_only` **330 不变**、domains 17 → **18**、invariants 40 → **42**（登记表 / 反例测试 / 能力清单三源一致）；`size_debt` 20 项不变、`dep_debt` 0、`dep_rules` 19 crate；回放金标 `385482eb033d2964` 未漂移；`cargo xtask verify` 11 步全绿。

---

## 11. C07：可玩性回归 2（tests **+10**）

### 11.1 夹具（复用 B05 归置模式）

```
assets/labs/terrain_roam/
├── terrain_roam.r3e.json      # 200×200 地形 + 1 个角色出生点 + 5 个路点空节点
├── scripts/roam.rhai          # 脚本化: 生成地形 → 逐路点移动 → 记录 grounded
├── tools/drive_roam.py        # 按 __file__ 定位、驱动 CLI（复用 B05 的 drive_keyboard.py 结构）
└── evidence/                  # 人工截图（上坡、跨块接缝、陡坡拒绝）
```

自包含、不污染 `scripts/` 与 `assets/` 根（B05 已确立）。

### 11.2 10 条测试

| # | 断言 |
|---|---|
| 1 | 从夹具加载 → 地形块数 16、角色初始 `grounded = true` |
| 2 | 走到路点 1（平地）距离误差 < 0.2m |
| 3 | 走到路点 2（缓坡 15°）`grounded` 全程为真 |
| 4 | 走到路点 3（陡坡 50°）中途 `grounded` 出现 false（角色滑下） |
| 5 | 跨块接缝行走（路点 4 在块边界）：无卡顿（单帧位移不为 0） |
| 6 | 路点 5（坡顶）：终点高度与 `terrain.sample` 一致（<1cm） |
| 7 | 全程无 panic、无 NaN 位置 |
| 8 | 双后端各跑一遍：`PhysicsKind::Builtin` 与 `Rapier` 终点位置差 < 0.5m |
| 9 | 回放确定性：同 seed 两次 run 的最终 `state_hash` 相同 |
| 10 | 夹具加载不改变既有金标场景（`cargo xtask replay` 仍 `385482eb033d2964`） |

人工栏（回填 `M2_BASELINE.md` §7.2）：截图 3 张 + 一句主观结论（观感/手感是否可接受）。

### 11.3 实施记录（2026-11-24 完成）

**落地文件**

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/r3e-runtime/tests/terrain_roam.rs`（新） | 388 | 10 条行走回归（`parse_scene` 载夹具 → 双后端各跑一遍轨迹） |
| `crates/r3e-runtime/src/terrain_draw.rs`（新） | 460 | 地形绘制组帧：把 `r3e-terrain` 的块 / LOD / 权重翻译成 `TerrainDrawItem` 并按相机裁块，含 11 条测试 |
| `crates/r3e-runtime/src/terrain_field.rs`（新） | 181 | 地形→物理场地绑定（高度场采样面即碰撞源），含 5 条测试 |
| `crates/r3e-render/src/renderer/terrain_pass.rs`（新） | 98 | 地形通道装配（独立管线 + 每帧提交），从 `renderer/` 里拆出以免碰 800 行线 |
| `crates/r3e-render/src/terrain.rs` | 494 | 新增 `pack_camera` / `CAMERA_UNIFORM_FLOATS`：uniform 打包与缓冲分配共用同一常量（见取舍 3） |
| `crates/r3e-render/tests/terrain_pass.rs` | 279 | 8 → **9** 条：新增「相机 uniform 打包长度等于缓冲字节数」 |
| `crates/r3e-physics/src/port_terrain.rs` / `tests/terrain_field.rs` | 65 / 518 | 地形场地查询（raycast / sweep / overlap 走高度场），+3 条测试 |
| `assets/labs/terrain_roam/`（新） | — | 自包含夹具：`terrain_roam.r3e.json`（200×200 m、11×11 顶点、cell 20 m）+ `scripts/roam.rhai`（键盘版）+ `scripts/auto_roam.rhai`（自动巡检版）+ `tools/drive_roam.py` / `drive_roam_auto.py` + `evidence/`（11 张截图） |

同步修改：`r3e-render/src/renderer/{mod,init,state,frame}.rs`（通道注册与每帧提交）、`r3e-runtime/src/{lib,app,physics_bind,physics_sync}.rs` + `Cargo.toml`（装配改调 `terrain_field` / `terrain_draw`）、`r3e-physics/src/lib.rs`（门面转发地形端口）。

**测试 30 条**（方案 +10，实到 +30——偏差见取舍 1）

| crate | +条数 | 内容 |
|---|---|---|
| `r3e-runtime` | +26 | `tests/terrain_roam.rs` 10：夹具出 16 块且角色初始贴地、平地段位移、15° 缓坡全程 `grounded`、50° 陡坡推不上去、跨块接缝不卡顿、坡顶高度 == `terrain.sample`（<1 cm）、全程无 NaN、双后端终点差 <0.5 m、同种子两次 `state_hash` 相同、夹具不改变金标场景；`terrain_draw.rs` 11：块裁剪、LOD 档位选择、权重打包、材质层数、空地形不产 draw item、相机换位后裁剪面变化等；`terrain_field.rs` 5：场地采样面与渲染同源、后端不支持时回 `none`、射线命中高度场、扫掠/重叠含地形候选、场地换块后不再命中旧块 |
| `r3e-physics` | +3 | `Physics` 门面三个查询在地形高度场上各 1 条（打得到地形 / 刚体更近时仍取刚体 / 扫掠与重叠都含地形） |
| `r3e-render` | +1 | `相机_uniform_打包长度等于缓冲字节数`：钉住「打包长度 × 4 == `CAMERA_UNIFORM_BYTES`」 |

**五处方案外取舍与一次判据偏差**

1. **tests 实到 +30，超卡片预算 +10**。§11.2 那 10 条是站在「行走回归」视角写的，实测发现要真跑起来必须先补两处缺口：①无 `terrain_draw` 时绘制组帧无产（画面里根本没有地形，截图证据拿不到）；②无 `terrain_field` 时物理层拿不到高度场（角色直接穿地，回归全红）。故本卡实为「补缺口 + 回归」两件事，与 C06 的教训一致：**可玩性回归的报价必须先算清「让它跑起来」的成本**。
2. **判据 #4 口径变更（方案 → 实测）**。原写「走到陡坡 50° 中途 `grounded` 出现 false（角色滑下）」，实测该坡的表现是**推进失败**（位置几乎不动、HUD 报「阻挡 <- 坡度超限 36.6°」），**不存在滑落帧**。改为断言「推不上去（终点距起点 <0.3 m）且全程无 panic / 无 NaN / 不下沉」。理由：滑落与否是物理后端实现细节，B05 的 60° 坡同样是「阻挡<-坡度超限」而非滑落，C07 与 B05 保持同口径。
3. **顺手修掉一处真机崩溃（C03b 遗留接缝）**。`r3e-render/src/terrain.rs::update_uniforms` 用 `[f32; 32]`（128 字节）打包 camera，而 `camera_buffer` 按 `CAMERA_UNIFORM_BYTES = 112` 分配——真窗口**首帧** `write_buffer` 越界 panic；headless 测试覆盖不到（不写 GPU 缓冲）。抽出 `pack_camera()` 与 `CAMERA_UNIFORM_FLOATS` 常量，并加 1 条测试钉住两者相等。**教训：uniform 的打包与分配必须共用同一常量，任何一处写死字面量都会在真机上炸，而 headless 永远绿。**
4. **人工证据的抓帧方式重做**。原 `drive_roam.py` 按「相对启动的秒数」截图，首轮产出的四张「关键帧」里三张 MD5 完全相同（`8D354808`）——抓的是同一帧的重复，且是**整屏桌面**（含任务栏与桌面图标），不是游戏画面。改为按**状态标记**抓（`auto_roam.rhai` 每帧打轨迹行 + HUD 文案，驱动脚本判定「到路点 / 被挡 / 到顶」后才抓），6 张全部互异且只含游戏窗口。**教训：截图证据必须由「被验证的状态」触发，不能由时钟触发。**
5. **键盘注入路径降级为「手动实验场」**。`drive_roam.py`（真 `key_down`）依赖 `SetForegroundWindow`，实测被前台锁定拒绝（`foreground ok: False`），窗口不抢前台时引擎一帧不动、截图全停在出生点，证据不可复用。故：判读一律以 `auto_roam_*` 为准；`roam.rhai` + `drive_roam.py` + 5 张 `roam_0*.png` 保留给人手动玩，**不作判读依据**（其中与 `roam_02` 内容重复的 `roam_01_slope15.png` 已删，`_tmp_probe.rhai` 临时脚本已删）。

**门禁数字**：`r3e-physics` 265 → **268**、`r3e-render` 324 → **325**、`r3e-runtime` 44 → **70**，全局 2300 → **2330**（`check-tests` 全绿）；`commands` **406** / `rpc` **76** / `domains` **18** / `invariants` **42** / `script_only` **330** 全部持平；`size_debt` 20 项不变、`dep_debt` 0；回放金标 `385482eb033d2964` 未漂移；`cargo xtask verify` 11 步全绿。

**收口三事**

- `doc-gate` **规则 4** 首轮实跑红：`tests/terrain_roam.rs:65` 直接 `Scene::from_json_str`（`rule4_scope` 含 `crates/r3e-runtime/`，tests 目录同样在扫描范围，不能靠白名单绕）。改走 `r3e_doc::parse_scene` 并留注释说明，与上层其它调用方同口径。
- **`verify` 时长重标定**：`AI_ROADMAP §0.2` 记的「冷态 255.6s」是 M2-A01 时的规模，C 阶段收口实测冷态 **≥600s**（600 s 观察窗内未跑完）、暖态 **208s**。旧口径「热态 <120s」在 M2 规模下已失效，详见 `AI_ROADMAP §0.2` 该行注记。
- 仓库根 **14 个 `.log`** 临时文件（未入版本库）已清空；`temp_check/temp_clippy/...` 等本轮排查遗留一并删除。
- **文档与人工栏回填**：`M2_BASELINE.md` 新增 §7.2（5 项人工判读 + 6 张证据清单与 MD5，写明键盘注入路径不作判据、首帧 HUD 说明）；`M2_PLAN.md` 首行进度串补 C07 条目并推进到「C 阶段 8 卡全部完成 / 下一张卡 D01」；`tests_baseline.json` 落 2330 与差额说明。

---

## 12. 决策清单（**7 条，CD-1 必须先拍**）

### CD-1（**硬冲突**）命令面 → rpc 预算

事实三方冲突：
- 卡表要 C 阶段 `commands +8`、`rpc +3`、`domains +1(terrain)`；
- 守恒规则 4：`script_only` **不得增加**；
- 实测关系：`398 = 68 rpc + 330 script_only`，而 `script_only` 的判据是「`rpc == None`」——**任何不带 rpc 的新命令都会涨 `script_only`**。

故 8 条命令若只有 3 条带 rpc，`script_only` 330 → 335，直接违反规则 4。三者不能同时成立。

| 档 | 做法 | commands | rpc | script_only | 全局 rpc 终局 | 代价 |
|---|---|---|---|---|---|---|
| **A（推荐）** | 8 条全带 rpc | +8 ✓ | **+8**（预算 +3→+8） | 330 不变 ✓ | 80 → **85** | 改 7 处硬数字（§10.3，机械） |
| B | 命令面压到 3 条（`terrain_gen`/`terrain_sample`/`terrain_stats`），其余 5 项只做 Rust API 不进表 | +3（预算 +8→+3） | +3 ✓ | 330 不变 ✓ | 80 不变 | 能力清单残缺：脚本/RPC 看不到建块、块划分、LOD 重采样、网格统计；C07 只能靠 CLI 夹具 |
| C | 8 条全挂 `Meta` 域（只 RPC、不绑脚本） | +8 ✓ | +8 | 330 不变 ✓ | 85 | 脚本侧无地形能力，且能力清单里 `terrain` 域下 0 条命令（`domains +1` 变成空域，语义荒谬） |

推荐 **A**：命令面完整、脚本可驱动、`domains` 有实质内容，代价只是 §10.3 的机械同步 + 两处预算文档回写。

### CD-2 地形数据的权威源

* **A（推荐）**：`r3e-terrain` 是纯派生层，输入 `&[TerrainPatch]`，输出裸 `Vec`；`core::TerrainPatch` 仍是唯一进场景/存档/哈希的类型。`state_hash`、撤销栈、校验器零改动。
* B（不成立）：把 `TerrainPatch` 迁进 `terrain` crate → `r3e-core` 要反向依赖，而 `core.allow` 为空；`r3e-doc` / `r3e-nav` / `state_hash` 全要改。撞 `dep-check` 红线。

### CD-3 采样实现重复（`nav` 私有 `patch_height`）

* **A（推荐）**：单块采样上移 `core::TerrainPatch::sample_height`，`r3e-terrain` 用它；**`r3e-nav` 的私有实现本轮不动**，登记为 D 阶段收口项（nav 是 D 阶段独占文件，C 阶段动它违反「严格串行、每卡文件区域不重叠」）。用交叉断言（C01 #12）锁住两者等价，保证未来收口不改行为。
* B（不采纳）：C01 同卡收敛 nav（删 30 行重复）→ 省代码但跨界改 D 独占文件，且 `nav/bake.rs` 测试数字会动。
* 附注：若老板不接受 `r3e-terrain` 的 dev-dependency 边 `r3e-nav`，则退化为把 20 个点的期望值写死为常量表（零新边，成本 +半小时）。

### CD-4 C03 是否拆两卡

* **A（推荐）**：拆 `C03a`（网格 + 权重，headless 全可判）与 `C03b`（独立管线 + 混合着色，GPU 观感走人工）。C 阶段变 **8 卡**。先例：A03→A03b 的插入方式。
* B：单卡做完。风险：C03 成为本阶段唯一「失败原因不可分离」的卡（网格错 vs 着色错表现相同：画面不对），且渲染改造一旦超期会拖垮后面三卡。

### CD-5 地形碰撞的实现层

* **A（推荐，即 §8 方案）**：物理侧独立「地形场」通道（`HeightField` + `HeightFieldPort` + 双后端实现），**不改 `ColliderShape`**。注入方是 `r3e-runtime::terrain_scene`，物理不依赖 terrain。
* B：给 `ColliderShape` 加 `HeightField` 变体 → 要改 `types.rs` 的解析、`builtin_backend.rs` 与 `rapier_backend.rs` 的全部 shape 匹配分支（接触/射线/扫掠/胶囊），预算 +250 行且触碰三处在 `size_debt` 的文件。
* C（不采纳）：绕过物理，在角色控制器里直接采样抬升 `ground`。工作量 1/5，但「双后端各验」判据落空，nav 与物理语义分叉，等于把 C04 的判据删掉。

### CD-6 确定性哈希口径

* **A（推荐）**：两层——① 场景数据哈希取 `r3e-doc::state_hash`（已含 `mix_terrain` 逐点）② 命令返回值即该哈希的 16 位十六进制。零新哈希函数、零新依赖。
* B（不采纳）：为地形另起专用哈希（如 FNV over heights）→ 与 `StateHash` 双轨，`replay` 与命令校验对不上。

### CD-7 `terrain.gen` 的 CLI 落地形式

* **A（推荐）**：走 `r3e-cli rpc --method terrain.gen --args '{...}'`。`args.rs` 零改动，保住 97 行余量给 D 阶段。
* B：新增 `gen-terrain` 子命令（`--size 200x200 --seed 42`）→ 预算 60~90 行，`args.rs` 703 → 763~793，余量剩个位数，D 阶段第一次加子命令即越 800 并被迫新增 `size_debt`（账只许减）。
* 卡表原文写作 `terrain.gen --size 200x200 --seed 42`，档 A 理解为「能力形态」而非「CLI 语法」，C05 判据以「同 seed 两次逐字节一致 + 返回哈希」为准。

---

## 13. 门禁累加与预算配平

### 13.1 tests +108 的八卡分配

| 卡 | tests | 内容 |
|---|---|---|
| C01 | +16 | 块划分 6 / 采样 5 / 与 nav 等价 1 / 法线 1 / LOD 3 |
| C02 | +14 | 三级误差 3 / 接缝 3 / 确定性与配置 4 / 边界 4 |
| C03a | +14 | 顶点/索引/三角 3 / 权重 4 / 网格与采样一致 2 / 边界与确定性 5 |
| C03b | +12 | 布局与打包 5 / 装配行为 4 / wgsl 与混色等价 3 |
| C04 | +20 | 10 断言 × 2 后端 |
| C05 | +10 | 生成确定性 5 / 边界 3 / 事务与不变性 2 |
| C06 | +12 | 8 命令行为 9 / 能力清单与域 2 / invariant 反例 1 |
| C07 | +10 | 路点与双后端 8 / 回放与金标 2 |
| **合计** | **+108** ✓ | 与 `M2_PLAN` §4 C 行一致 |

`tests_baseline` 演进：2150 → 2166(C01) → 2180(C02) → 2194(C03a) → 2206(C03b) → 2226(C04) → 2236(C05) → 2248(C06) → 2258(C07)。
**注**：B 阶段实到比计划高 26（计划 +82 / 实到 +125，差额来自同卡修缺陷补夹具）。C 阶段同样可能超配——卡内判据一条不许减，超出的部分照 B 阶段做法记「实到对照」，不回退门禁。

### 13.2 其他门禁项

| 项 | 基线（实测） | C 阶段增量 | 终值 |
|---|---|---|---|
| commands | 398 | +8 | **406** |
| rpc | 68 | **+8**（CD-1 档 A；原计划 +3） | **76** |
| script_only | 330 | 0 | 330 |
| invariants | 40 | +2 | **42** |
| domains | 17 | +1（`terrain`） | **18** |
| workspace 成员 | 18 | +1（`r3e-terrain`） | **19** |
| `size_debt` 条目 | 20 | **0（只许减）** | 20 |
| `dep_debt` 条目 | 0 | 0 | 0 |
| `dep_rules` 新增边 | — | 3 处（§3.1） | 登记理由见 §3.1 |
| 回放金标 | `385482eb033d2964` | **0（必须不变）** | 同值 |

### 13.3 拍板后需改的权威文档（顺序不能颠倒）

1. `M2_PLAN.md` §4 预算表 C 行：`rpc +3` → **`+8`**；全局 rpc 目标 `80` → **`85`**；表下加实测说明。
2. `M2_PLAN.md` §3.2 C 阶段卡表：C03 拆 `C03a/C03b`（8 卡）；表下加一行「本阶段详细方案见 `M2_C_PLAN.md`」。
3. `docs/m2/README.md` 索引登记本文。
4. `M2_DECISIONS.md` 追加 **D14**（C 阶段 rpc 预算 `+3` → `+8`，全局 80 → 85）/ **D15**（terrain 为纯派生层 + 50 米块可配）/ **D16**（地形碰撞走独立地形场通道，不进 `ColliderShape`）。编号从 D14 起——D13 已被 A02 的 `dep_debt` 偿还占用。
5. `docs/m0/M0S5_PROTOCOL.md` §3 命令表清单补 8 行（C06 同卡）。
6. `r3e-cli/src/args.rs::TX_ONLY_METHODS` 补 `terrain.create` / `terrain.remove` / `terrain.resample` / `terrain.gen`（**C06 同卡**，防 `rpc` 绕过事务）。
7. `M2_BASELINE.md` 人工验证栏：C07 主观项（§7.2）。

---

## 14. 执行顺序（每步都可单独 `verify`）

| 步 | 动作 | 门禁预期 |
|---|---|---|
| S1 | 回写 §13.3 的第 1–4 项（**纯文档提交**，不含代码） | 11 步全绿 |
| S2 | **C01-a**：crate 骨架 + `dep_rules` 三处 + `tests_baseline` 条目 + `lib.rs` 常量 | `dep-check` 0 违规；`check-tests` 2166；成员 19 |
| S3 | **C01-b**：`core::TerrainPatch::sample_height` + 采样辅助（`core/terrain.rs` 238→约 300） | `cargo test -p r3e-core` 绿；`size_debt` 仍 20 |
| S4 | **C01-c**：`block.rs` / `sample.rs` / `lod.rs` + 16 条测试（含与 nav 的等价断言） | `cargo test -p r3e-terrain` 绿；C01 提交 |
| S5 | **C02**：LOD 三级 + 接缝规则 + 14 条测试 | `check-tests` 2180；提交 |
| S6 | **C03a**：`mesh.rs` + 权重 + 14 条测试 | 2194；提交 |
| S7 | **C03b-a**：`render/terrain.rs` + `terrain.wgsl` + 8 条布局/打包测试（无 GPU） | 2206（部分）；`naga` 解析通过 |
| S8 | **C03b-b**：`runtime/terrain_scene.rs` 装配 + 4 条装配测试；`app.rs` 增量 ≤2 行 | 2206；`size_debt` 未涨；提交 |
| S9 | **C04-a**：`physics/heightfield.rs` + `port/heightfield.rs` + builtin 实现 + 10 条测试 | `cargo test -p r3e-physics` 绿 |
| S10 | **C04-b**：rapier 实现 + 另 10 条测试（同断言）+ 双后端一致性 | 2226；提交 |
| S11 | **C05**：`generate` + `SplitMix64` + command 层接入 + `TX_ONLY_METHODS` + 10 条测试 | 2236；提交 |
| S12 | **C06**：`cmd_spec/terrain.rs` + 8 条命令 + `Domain::Terrain` + 2 条 invariant + `Category::Terrain` + §10.3 七处数字 + 重生成 `capabilities.json` | `gen-capabilities --check` 过；`invariants` 42；2248；提交 |
| S13 | **C07**：`assets/labs/terrain_roam/` 夹具 + 10 条测试 + 人工截图 + `M2_BASELINE.md` §7.2 回填 | 2258；金标不变；提交并 push |

「判据先行」（`M2_PLAN` §5.3）：C02/C04 的误差与接缝断言先写（红）再补实现（绿）。

---

## 15. 风险与回滚

| # | 风险 | 触发信号 | 处置 |
|---|---|---|---|
| R-1 | rpc 预算 +3→+8 的连锁遗漏 | `gen-capabilities --check` 报 commands/rpc 不符 | §10.3 七处表格逐条核；漏的必在 `r3e-*-tests/cmd_spec_contract.rs` 三处之一 |
| R-2 | 渲染复用既有管线导致管线校验失败 | `Vertex attribute location 16 must be less than limit 16` | 已按 §7.1 规避：独立管线；一旦发现有人把 terrain 顶点塞进 `InstanceRaw`，直接回滚该改动 |
| R-3 | 双后端地形语义分叉（rapier `HeightField3d` 的列/行/缩放顺序与 builtin 约定相反） | C04 第 10 条断言差 >1e-3 | 在 `rapier_heightfield.rs` 顶部写死「行=z、列=x」并加 1 条专项断言；分叉不可接受时降级为「rapier 侧高度场单位 = 米、`cell` 显式传入」 |
| R-4 | `app.rs` 增量超 100 行 → 触发 R2 提前 G02 | `app.rs` 行数从 2821 涨 | 装配一律落 `terrain_scene.rs`；`app.rs` 只留 2 行 |
| R-5 | `physics/lib.rs`（1924，在 `size_debt`）被追加实现 | 该文件行数上涨 | 只允许加 `pub use`；实现一律落 `*_heightfield.rs` |
| R-6 | C03b 无 GPU 自动判定弱 → 视觉效果错到 C07 才发现 | C07 截图明显不对 | C03b-b 加「CPU 等价混色函数」测试（#12）先兜住算法，观感留人工 |
| R-7 | `r3e-terrain` 越界依赖 render/physics | `dep-check` 违规 1 | 回滚该依赖；走装配层传裸 `Vec` |
| R-8 | 金标漂移 | `cargo xtask replay` ≠ `385482eb033d2964` | C 阶段不改任何既有命令语义；`terrain.gen` 只写 `Scene.terrain`，金标场景无地形 → 不应漂。漂了就是误改了别的路径，按卡二分回滚 |

**回滚点**：每卡一个独立提交（S4/S5/S6/S8/S10/S11/S12/S13），任何一卡红都可 `git revert` 该提交而不影响前序卡。

---

## 16. 明确不做（防范围蔓延）

1. **不做样条/手绘地形编辑**（`AI_ROADMAP` crate 表提过「样条编辑」，C 卡表无此判据）→ 留给后续。
2. **不做地形贴图资源管线**：4 层混合只用常量色/程序化权重 + 既有 `SceneMaterialDecl` 名字引用，不引入新贴图格式、不新增 asset 依赖。
3. **不做 navmesh 重烘焙**：地形改变后导航数据不自动重建（D 阶段专属）。
4. **不做块剔除/驻留策略**（D10 的「驻留 3×3」）：C03b 只按距离选 LOD，不做视锥剔除与流式加载。
5. **不引入任何新第三方依赖**（PRNG 自实现 12 行；哈希复用 `state_hash`）。
6. **不动 D/F/G/H 阶段的独占文件**（`nav/bake.rs`、`character/*`、`app.rs` 装配区）。

---

## 17. 与既有拍板的一致性核对

| 既有拍板 | C 阶段如何遵守 |
|---|---|
| D1：物理双后端各验一遍 | C04 的 20 条 = 10 断言 × 2 后端；C07 双后端各跑一遍 |
| D10：分块默认 50 米、可配置、非法配置须拒 | C01 的 `BlockGrid::new` 校验 + 4 条非法配置测试；C05 用 50 米 |
| D12=B：建独立 `r3e-terrain`，allow 仅 `r3e-core` | §3.1；R8 红线写进 §15 R-7 |
| B 阶段 BD-1 档 A（命令全带 rpc，不动守恒规则 4） | CD-1 档 A 同构处理 |
| 守恒规则 4：`script_only` 不得增 | 8 条命令全带 rpc → 330 不变 |
| `size_debt` 只许减 | 新增代码全落新文件，触碰的两处在 `size_debt` 的文件只加 `pub use` |
| 回放金标必须不漂 | §13.2 与 §15 R-8 |

---

## 18. 一页速览

- **新建 1 个 crate**：`r3e-terrain`（纯派生层，输入 `&[TerrainPatch]`，输出裸 `Vec`），成员 18 → 19。
- **新增 1 个能力域**：`Terrain`（8 条命令，其中 `terrain.mesh` 挂 `Meta`），domains 17 → 18。
- **新增 4 个跨层契约**：`core::TerrainPatch::sample_height`、`physics::HeightField`、`physics::port::HeightFieldPort`、`render::terrain` 管线。
- **新增依赖边 3 条**：terrain→core（新 crate）、runtime→terrain、editor-host→terrain。render/physics/script 一行依赖都不加。
- **8 张卡**（C03 拆 a/b），严格串行，每卡独立提交与回滚。
- **门禁**：tests 2150 → **2258**（+108）、commands 398 → **406**、rpc 68 → **76**、invariants 40 → **42**、domains 17 → **18**、成员 18 → **19**、`size_debt` 20 不变、`dep_debt` 0 不变、金标 `385482eb033d2964` 不变。
- **必须先拍**：CD-1（rpc 预算 +3 → +8，全局 80 → 85）。其余六项 §12 都有推荐档，全选推荐档即可开工。
*（内容由AI生成，仅供参考）*
