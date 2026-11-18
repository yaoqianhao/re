---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_ac3e6792e28b11f18c5752540024e231
    ReservedCode1: QBJUfvnmjYun4GCv42SvuleiJtfFvUvPNeVlOK38xrJt78ZGkOxpFvIz/ZpMeCRahFYETma/u4xvNt63CbqPTxyTZgKIlxkhdLECvNFal+2/jI6HT1pUF2r+qC2FoiVLrYEbRzLZ552O2BcqXgRXenTj5pyGkHt6BS6Oy5PHy1i584A/vsyNWS8qLiQ=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_ac3e6792e28b11f18c5752540024e231
    ReservedCode2: QBJUfvnmjYun4GCv42SvuleiJtfFvUvPNeVlOK38xrJt78ZGkOxpFvIz/ZpMeCRahFYETma/u4xvNt63CbqPTxyTZgKIlxkhdLECvNFal+2/jI6HT1pUF2r+qC2FoiVLrYEbRzLZ552O2BcqXgRXenTj5pyGkHt6BS6Oy5PHy1i584A/vsyNWS8qLiQ=
---



# M1-S4 方案 V3：能力全量前置（导航 + 材质全功能）

> 状态：S4-A / S4-B / S4-C / S4-D / S4-E 已落地（S4-E 含前置补债 + 三层验证器 + 坐标化 Issue，2026-11-18）；S4-F 待执行（V3 取代 V2 的「部分外排」裁决）
> 日期：2026-11-17
> 归属：`docs/AI_ROADMAP.md` §3.2 M1「AI 写入闭环」第四 Sprint
> 前置：S1（事务协议）、S2（WorldQuery）、S3（意图命令 + impact）已收口，单测基线 1,574，`cargo xtask verify` 十项全绿
> 变更要点：**取消 V2 的三项外排**（可通行性 → S4.5、材质缺失 → M3、装配→验证），改为「引擎能力全量补齐 → 验证器只做消费」。Gate 1 顺延至能力补齐完成后触发。

---

## 0. 一句话结论

V2 的裁决是「能补的补、补不了的记账外排」。V3 改为：**不存在外排项**。M1-S4 先补三类引擎能力——① 场景材质资源引用体系 ② 导航子系统 ③ 装配/查询公共面与协议面——把这三类做成可被任意消费者（验证器 / CLI / ai_ide 插件）调用的正式能力，然后验证器才落地。验证器上线时 `unchecked` 必须为空。

---

## 1. 目的

M1-S4 原计划两卡（验证器五类检查 + 接入 verify 链与 impact）。只读核查结论：五类检查里三类在原方案下无法给出可信结论，根因在引擎侧缺能力而非验证器写法。V3 明确取「先补能力、后接验证器」，并把这期定性为**引擎能力 Sprint**，而非验证器 Sprint。

**硬约束（不变）**：

1. 验收口径不变——「故意生成不可达关卡 → 报出**具体失败点位坐标**，不是布尔值」；
2. 分层不破——`r3e-doc` 只依赖 `core`（禁止反向新增依赖）；
3. 不假绿——能补的必须本期真补，不允许用近似值或 `unchecked` 蒙混（V3 下 `unchecked` 目标是**空**）。

---

## 2. 现状实证（只读核查，V3 修订）

> 本表是 **V3 修订时点（2026-11-17）的快照**，用来证明「为什么先补能力」。S4-A…D 落地后其中多条已过时：C1 的 24 → **30** 条不变量、C3/C7 的 48 → **56** RPC、C6 的装配已下沉为公共入口、C9 的导航底座已建 `r3e-nav`、C10 的材质引用已可写入文档。原表保留原文不改写历史。

| # | 能力 | 实证位置 | 结论 |
|---|---|---|---|
| C1 | 校验器 v0 | `r3e-doc/src/validate/{mod,bounds,refs,invariants}.rs` | 24 条不变量（bounds 18 + refs 6），**只查场景自洽**，无空间关系、无坐标 |
| C2 | `Issue` 结构 | `r3e-doc/src/validate/mod.rs:47` | 仅 `{severity, code, path, msg}`，**无坐标 / 对象 id / 修复建议** |
| C3 | 校验器暴露面 | `r3e-doc/src/lib.rs:156` | 仅 crate 内调用；协议 48 RPC 中无 `scene.verify` |
| C4 | 物理查询原语 | `r3e-physics/src/lib.rs:720/728/736`、`1028/1038` | `raycast` / `overlap_sphere` / `shape_cast` 三原语已实现，`BackendCaps` 有能力位 |
| C5 | 物理查询上层面 | `r3e-physics/src/lib.rs:1430~1520` | `Physics::raycast/overlap_sphere/shape_cast/shape_cast_sphere/shape_cast_box` 已齐；关闭或能力缺失时诚实返回 `None`/空 |
| C6 | 场景→刚体装配 | `r3e-runtime/src/app.rs:1501 sync_physics_bindings()` | 装配埋在 runtime 私有方法，**验证器 / CLI / 插件不可复用**——双口径风险源 |
| C7 | 协议面 | `r3e-protocol/src/lib.rs` `RPC_METHODS` | 48 条中 **无 `physics.*`、无 `nav.*`、无 `scene.material.*`** |
| C8 | 几何工具 | `r3e-core/src/aabb.rs` | 有 `ray_hit/transformed/from_points/union/inflate`，**无 `intersects`**，nav 栅格化与穿模初筛都要用 |
| C9 | 导航能力 | 全仓检索 | **零底座**：无 NavGrid / NavMesh / agent 参数 / 出生点 / 连通块 / 寻路；`NodeKind::Character` 只是「会动的网格」，不是玩家实体 |
| C10 | 材质模型 | `r3e-core/src/material.rs:39`、`mesh.rs:296-345`、`texture_slot.rs` | `SurfaceMaterial` 仅 `metallic/roughness/emissive` 三标量，**无资源引用**；`MaterialData` 已有 4 个纹理槽 `Option<(AssetId,u32)>`、`MeshProvider::texture(AssetId,u32)`（`r3e-render/src/mesh_cache.rs:177`）、`TextureSlot` 六分类齐全 — **但 `AssetId` 是运行期 id，场景文档写不进去，独立 png 也无导入通道** |
| C11 | 内置网格尺度 | `r3e-core/src/procedural.rs` | cube 1×1×1、plane 1×1、sphere/cylinder 单位——尺寸由 `Transform.scale` 决定 |
| C12 | 碰撞体描述 | `r3e-core/src/scene_desc.rs PhysicsDesc` + `r3e-physics/src/lib.rs:238/262/343` | `body`（dynamic/static/kinematic）、`collider`（box/sphere/cylinder/capsule）、`half_extents`；`ColliderShape::from_node_with` 在 physics，doc 不可用 |
| C13 | 贴图解码能力 | `r3e-asset/src/gltf_import.rs:490` | `image::load_from_memory` 已在用 — 独立 png/jpg 导入通道**可复用同一解码栈，工作量可控** |
| C14 | 地形高度图 | `r3e-core/src/terrain.rs:48/99` | `TerrainPatch.heights` + `height_at(x,y)` 已就绪 — nav 栅格化的地形输入不需要新造 |
| C15 | 文档命令面 | `r3e-doc/src/command/mod.rs` | `CommandKind` 15 变体（含 `UpsertTerrainPatch`）；新增材质/导航命令须同步 undo 覆盖与写点台账（`docs/M1_PLAN.md` §5） |

---

## 3. 裁决（V3，全面内补）

| 能力域 | 缺什么 | V2 裁决 | **V3 裁决** | 卡片 |
|---|---|---|---|---|
| 场景材质资源引用 | 场景层无材质资源引用；`AssetId` 不可写入文档；独立贴图无导入通道 | 外排 M3，本期只判参数越界 | **本期真补全**：场景 `materials[]` 表 + 节点引用 + 贴图资产通道 + doc 命令 + state_hash + 渲染消费 + 验证器四类检查 | S4-A |
| 导航子系统 | 零底座：无 NavGrid / agent / 连通块 / 寻路 / 出生点 | 外排 M1-S4.5 | **本期真补全**：新 crate `r3e-nav`（栅格化 + 连通块 + A* + 漏斗 + agent 参数 + 出生点语义） | S4-B |
| 装配 / 查询公共面 | 装配在 runtime 私有；协议面无 `physics.*` / `nav.*` / `scene.material.*` | 装配下沉 + `physics.*` 本期，`nav.*` 等 S4.5 | **三组全上**：`Physics::bind_scene` / `Nav::bake_scene` + `physics.*3` + `nav.*3` + `scene.material.*2` | S4-C / S4-D |
| 验证器 | 无坐标 Issue、无空间检查 | 当期落地，`nav` 与材质缺失记 `unchecked` | **延后到能力之后落地**，三层（静态 / 物理 / 导航）全绿，`unchecked` 目标为空 | S4-E / S4-F |

**术语统一**：V2 的「M1-S4.5 导航 Sprint」与「M3 材质管线」在本方案内**取消编号占位**，导航与材质引用并入 M1-S4 本体；M3 只保留其原有职责（素材保真 / 资产管线扩展），不再承接「材质资源引用」这项基础能力。

---

## 4. 卡片拆分（6 卡，严格串行为主）

### S4-A 场景材质资源引用体系（全功能）

**目标**：让场景文档能声明「这份材质用哪几张贴图」，并让渲染、文档命令、哈希、验证器四处一致消费。

**schema（`r3e-core/src/scene.rs`）**：

```jsonc
"materials": [
  { "name": "brick",
    "metallic": 0.0, "roughness": 0.8, "emissive": [0,0,0],   // 标量兜底，语义同 SurfaceMaterial
    "albedo": "tex/brick_albedo.png",                          // 相对场景文件目录
    "normal": "tex/brick_normal.png",
    "metallicRoughness": "tex/brick_mr.png",
    "occlusion": "tex/brick_ao.png",
    "emissiveMap": "tex/brick_emissive.png",
    "uvScale": [1,1] }
]
```

- `SceneMaterialDecl`：`name`（唯一）+ 3 标量与 `SurfaceMaterial` 同口径 + 5 张贴图路径（全部 `Option<String>`）+ `uv_scale`；
- 节点侧：`SceneNode` 新增 `materialRef: Option<String>`，与既有内联 `material: Option<SurfaceMaterial>` **互斥**；两者同时出现 → 静态层 Error `material.ref_conflict`（保留内联以兼容存量场景，不删字段）；
- 命名空间裁决：`materialRef` 指向 `materials[].name`，**不复用** `mesh: "asset:path#n"` 的 asset 语法，避免两套路径语义混淆。

**贴图资产通道（`r3e-asset`）**：

- 新增 `import_texture(path) -> Result<(AssetId, u32), ImportError>`，复用 `image::load_from_memory` 解码栈（C13）+ `texture_slot::classify_slot` 定槽；
- 运行时新增纹理注册表（`r3e-runtime`）：`resolve_material(&SceneMaterialDecl) -> MaterialData`，把路径解析为 `(AssetId, u32)` 后交给既有 `MeshProvider::texture` —— **渲染层取图路径一行不改**；
- 同一路径复用同一 `AssetId`（引用计数），避免重复解码。

**doc 命令（`r3e-doc/src/command/mod.rs`，15 → 18 变体）**：`UpsertMaterial` / `RemoveMaterial` / `SetNodeMaterialRef`；三者必须进 undo 覆盖矩阵，并在 `docs/M1_PLAN.md §5` 登记（`doc_gate` 规则 7）。

**state_hash（`r3e-doc/src/state_hash.rs`）**：`scene_hash` 在 terrain 段之后插入 `materials` 表（条数 + 逐项 name/标量/5 路径/uvScale，顺序即插入序），节点段增混 `materialRef`。**哈希契约测试同步**（`tests/state_hash.rs`）。

**schema 产物**：`r3e-core/src/scene_schema.rs` + `assets/scene.schema.json` 重生成；`schema` 版本号按仓库现行规则递增，并在 `docs/` 登记变更。

**新增校验码（静态层）**：

| code | 严重度 | 判据 |
|---|---|---|
| `material.dup_name` | Error | `materials[].name` 重复 |
| `material.ref_missing` | Error | 节点 `materialRef` 无对应声明 |
| `material.ref_conflict` | Error | `materialRef` 与内联 `material` 同时出现 |
| `material.texture_missing` | Error | 声明的贴图路径在场景目录下不存在（doc 层做存在性判定，不做解码） |
| `material.params_out_of_range` | Error | 标量越界（沿用 V2 口径） |
| `material.unreferenced` | Warn | 声明未被任何节点引用 |

**验收**：一份带 5 张贴图的材质场景可保存 → 重载 → 渲染一致；改任一贴图路径后 `state_hash` 必变、改无关字段不变；三条 doc 命令 undo/redo 逐位可逆；六个 code 各有反例单测。

---

### S4-B 导航子系统（新 crate `r3e-nav`，零 → 一）

**状态：✅ 已落地（2026-11-18）**。口径按用户拍板为「前者：运行时按场景几何自动生成」，非「文档侧 navmesh 声明」。
交付：workspace 新增 `r3e-nav` 成员（`lib.rs` 402 / `bake.rs` 312 / `grid.rs` 546 / `path.rs` 422 行）+ `r3e-core` 新增 `spawn.rs`（`SpawnDecl`）+ `SceneFile.spawns`；测试 35 条（`tests/bake.rs` 17 / `tests/path.rs` 10 / 内联 8），已入 `xtask/tests_baseline.json` 硬门禁。

**落地与本文原始草案的差异**（以代码为准，此处记录裁决理由）：

| 草案 | 实际 | 理由 |
| --- | --- | --- |
| `NavConfig { cell_size 0.25 / agent_radius 0.35 / agent_height 1.8 / max_slope_deg 45 / max_step_height 0.35 }` | `NavConfig { cell_size 0.5 / agent_radius 0.4 / agent_height 1.8 / step_height 0.4 / max_slope_deg 45 / bounds / ignore_tags }` + `sanitized()` | 字段名统一为 `step_height`；补 `bounds`（可固定烘焙范围）与 `ignore_tags`（排除会动的节点——静态烘焙的前提） |
| `Nav::bake_scene(&Scene, &NavConfig) -> BakeReport` | `Nav::bake` / `bake_scene` / `bake_with` → `Nav { grid, report }`，`NavReport` 由 `Nav::report()` 取 | 报告与网格必须绑在一起：只拿报告没法接着寻路，只拿网格没法解释「为什么这里不可走」 |
| `agent_radius` 做腐蚀（erosion） | 障碍 AABB **inflate** | 一次判定同时吃掉身体宽度与贴墙缝隙，省掉一遍栅格腐蚀，且窄缝不会被两次收缩吃掉 |
| 俯视栅格化「每格取顶面高度」+ 法线坡度 | 地形高度场双线性采样 + 「格柱 × 障碍 AABB」判定，坡度/步高取「或」 | M1 场景绝大多数无地形段，需要「无 terrain 即默认平地」的兜底；坡度与步高二选一会误判斜面或台阶 |
| `path -> Option<Vec<[f32;3]>>` | `PathOutcome` 四态（`Found` / `Unreachable` / `InvalidEndpoint` / `BudgetExceeded`）+ `describe()` | 「摆件悬空」「图太大没算完」都不是「走不到」，混在一起会骗到自己 |
| `reachable(from) -> RegionSet` | `reachable(from, to) -> bool` + `regions() -> Vec<RegionStat>` | 原签名需要「以 from 为根的子图」，对 A\* 无增量收益；改为「点对可达 + 全区统计」，直接对上 S4-D 的 `nav.reachability` 回包 |
| `reachability() -> Vec<RegionStat>` | `Nav::regions()`（`id` / `cells` / 世界包围盒） | 同上 |
| 漏斗法拉直 | 8 邻 A\* + 视线拉直（简化版） | 体素网格上没有多边形边可做漏斗；视线拉直已消除绝大部分锯齿 |

**定位**：`r3e-nav` 依赖 `r3e-core` + `r3e-physics`（不含 render / runtime / doc），workspace 新增成员。

**数据模型**：

```rust
pub struct NavConfig {           // 全部有缺省值，场景可覆盖
    pub cell_size: f32,          // 缺省 0.25
    pub agent_radius: f32,       // 缺省 0.35
    pub agent_height: f32,       // 缺省 1.8
    pub max_slope_deg: f32,      // 缺省 45.0
    pub max_step_height: f32,    // 缺省 0.35
}
pub struct NavGrid { origin: [f32;3], dims: [u32;2], cell_size: f32,
                     walkable: BitVec, region: Vec<u32>, /* 0 = 不可走 */
                     heights: Vec<f32> }
```

**烘焙（`Nav::bake_scene(&Scene, &NavConfig) -> BakeReport`）**：

1. 收集静态碰撞体（`ColliderShape` box/cylinder/sphere，C12）与 `TerrainPatch` 高度图（C14）；
2. 俯视栅格化：每格取「顶面高度」，与邻格比 `max_step_height`、与自身法线比 `max_slope_deg`；
3. `agent_radius` 做腐蚀（erosion），保证贴墙格不可走；
4. 连通块标注（4 邻域 BFS）→ `region` 图；
5. `BakeReport { cells, walkable, regions, bbox, config, warnings }`。

**查询**：

- `path(from, to) -> Option<Vec<[f32;3]>>`：网格 A*（八邻域 + 斜角代价）+ 漏斗法拉直；
- `reachable(from) -> RegionSet` / `reachability() -> Vec<RegionStat>`；
- `nearest_walkable(p) -> Option<[f32;3]>`（验证器报坐标要用）；
- `disconnect_point(a, b) -> Option<[f32;3]>`：两连通块最近点对——**这就是「不可达关卡」失败点位的来源**。

**出生点 / 目标语义（必须补的引擎能力）**：`SceneFile` 增 `spawns: Vec<SpawnDecl { name, position, yaw }>`，节点侧不加新 kind（避免与 `NodeKind::Character` 混淆）；验证器以首个 spawn 为起点、以「可交互目标组」为终点判定可达性。

**验收**：① 平地 + 一堵墙 → 两侧必分属不同 region；② 45° 斜坡可走、60° 不可走；③ 0.3m 台阶可跨、0.6m 不可跨；④ 孤立平台（悬空岛）必自成 region 并给出 `disconnect_point` 坐标；⑤ 烘焙 1km² 场景 < 500ms（单线程基线，记录到文档）。

---

### S4-C 装配 / 查询公共面下沉

**状态：✅ 已落地（2026-11-18）**。

- `r3e-physics`：新增 `src/bind.rs`（`lib.rs` 在债务白名单里只许减不许增，装配与 `BodyDesc::from_node*` 一并搬进该文件），公开 `Physics::bind_scene(&mut self, &Scene, &dyn MeshProvider) -> BindReport`；`BindReport` 增 `bindings: Vec<(NodeId, u64)>` 句柄表；
- **签名与计划的一处偏离（有意）**：`bind_scene` 收的是 `&Scene` 而**不是** `&mut Scene`，装配本身不回写 `Node::body`，只产出句柄表。原因：内容验证器（S4-E）必须能调同一个入口，而它手里只有只读场景；若签名是 `&mut Scene`，验证器要么伪造可变场景、要么绕开入口另写一份装配——正是本卡要消灭的分叉。写回交给持有写通道的一方（运行时的 `Document::write` 闭包）。
- **只读入口的幂等靠两级复用**：`Node::body` 里仍存活的句柄优先，其次是 `Physics::last_bindings`（上一轮 `bind_scene` 自己的产出）。只读调用方不写回场景，若幂等只依赖 `Node::body`，验证器每调一次就重建一批刚体，两次校验拿到两套句柄、结论跟着漂。`rebuild_backend()` 清空该缓存（切后端后旧句柄全部失效）；
- `r3e-runtime`：新增 `src/physics_bind.rs`（`SceneMeshProvider` + `rebind()`），`app.rs::sync_physics_bindings` 只剩「判世代 → 调用 → 记日志」；`node_mesh_data` 提为 `pub(crate)` 供两侧共用，装配与渲染仍看同一份网格；
- `r3e-nav`：`Nav::bake_scene` / `bake_scene_with` 公共入口就位（S4-B 自带，本卡只确认运行时侧调用口径）；
- `r3e-core`：`Aabb::intersects` 已就位（含相切语义、空盒一律 `false`，带两组单测）。

**验收结果**：runtime 全量测试**未改一行**、`cargo test --workspace` 122 套全绿；`bind_scene` 连调两次句柄表逐项相同（`readonly_bind_scene_is_idempotent`）；`NullBackend` 下属实 `simulated() == false` 并在 `summary()` 里写明（`readonly_bind_scene_reports_unsimulated_backend`）；`cargo fmt --all` + `cargo xtask verify` 十项全绿。

---

### S4-D 协议面（RPC 48 → 56，八条）

| 方法 | 语义 | 关键回包字段 |
|---|---|---|
| `physics.raycast` | 射线查询 | `hit{node,name,distance,point,normal,startedInside}` + `capabilities` |
| `physics.overlap` | 球形重叠 | `hits[]` + `capabilities` |
| `physics.cast` | 形状扫掠 | `hit` + `capabilities` |
| `nav.bake` | 烘焙导航网格 | `report{cells,walkable,regions,bbox}` + `config` |
| `nav.path` | 寻路 | `path[[x,y,z]]` / `failure{reason,nearest}` |
| `nav.reachability` | 可达区统计 | `regions[{id,cells,bbox}]` |
| `scene.material.list` | 材质表查询（只读） | `materials[]` |
| `scene.material.upsert` | 材质声明写入（走事务） | `hash` + `version` |

**能力位透传（铁律）**：`physics.*` 回包必须带 `capabilities:{raycast,overlap,shapeCast}`；后端不申报能力时返回 `BACKEND_UNSUPPORTED`，物理关闭时返回 `PHYSICS_DISABLED`，**绝不返回空表**（空表会被 AI 读成「没有东西挡住」）。`nav.*` 在未 bake 时返回 `NAV_NOT_BAKED`。

**契约同步（四处，漏一处即红）**：`cmd_spec_contract` 下限 48 → 56；`r3e-protocol/tests/rpc_contract.rs` 的 `SOURCES` 补 `dispatch/{physics,nav}.rs`（`scene.material.*` 两臂落在已有的 `dispatch/scene.rs` 里，被原有扫描覆盖，**没有** `dispatch/material.rs`——计划原文写 `{physics,nav,material}.rs` 与落地不符，此处订正）；`RPC_METHODS` 条数断言；`capabilities.json` 重生成。

**状态：✅ 已落地（2026-11-18）**。四层同改 + 四处契约同步全部到位。

**落地实证**：

1. **core / 依赖层**：S4-C 的 `Physics::bind_scene`、`Nav::bake_scene` 直接复用，本卡零改动；
2. **protocol 层**：`probe.rs`（`RayQuery` / `OverlapQuery` / `CastQuery` / `CastShapeSpec`）、`nav.rs`（`NavBakeOptions` / `NavPathQuery` / `NavReachQuery`）、`material.rs`（`MaterialUpsert`）三个 DTO 文件；`dispatch/physics.rs` + `dispatch/nav.rs` 两条臂（`physics.rs` 只做参数形状校验，命中不到转 `dispatch_nav`，业务裁决一律在宿主），`scene.rs` 加两条 `scene.material.*`；`RPC_METHODS` 48 → **56**；`api.rs` 八条默认 `unsupported`（带三条铁律注释）；
3. **editor-host 层**：`src/api/{physics,nav,material}.rs` 三个新文件；`host.rs` 增 `RefCell<Option<Nav>>` 槽位（导航产物住宿主、不进 `Document`），`poll_one` 注入 `LocalEngineApi`；
4. **cmd_spec 层**：八条登记完毕，`cargo xtask gen-capabilities` 重生成 `capabilities.json`（`commands=386 / rpc=56`）。

**验收结果（`cargo xtask verify` 十项全绿）**：

| 口径 | 结果 |
|---|---|
| 命令表 / RPC | 386 / 56（`gen-capabilities --check` 字节一致） |
| 四处契约 | `r3e-protocol/tests/rpc_contract.rs` 30 条（含补两条 `SOURCES` 后 dispatch 臂全量对齐）、`r3e-editor-host/tests/cmd_spec_contract.rs` 3 条（方法数 48 → 56）、`r3e-editor/tests/cmd_spec_contract.rs` 4 条（补 `physics`/`nav` 两段扫描后「表在骗人」归零） |
| 宿主层单测 | `physics_nav.rs` **13 条**全绿：物理关闭回 `PHYSICS_DISABLED`、`NullBackend` 不申报能力回 `BACKEND_UNSUPPORTED`（两者不许退化成空表）、成功回包自带 `capabilities` 连空表也带（「没打到」≠「不会打」）、四类非法入参、三条探测不动 `StateHash` 与写次数；材质 upsert 新建/覆盖与 **整行声明语义**（没给贴图＝这一行没有贴图）、`users` 按 `materialRef` 反查、空名拒绝且不留写操作；nav 无槽位与未烘焙同码、`bake` 只写槽位（哈希与写次数都不变）、烘焙失败保留上一次网格 |
| 写点台账 | `r3e-editor-host` 18 → **22**（1 处真实写点 + 3 处测试夹具），合计 234 处，理由已登记 `docs/M1_PLAN.md §5` |
| 单测基线 | 1650 → **1677**（host 81 → 94） |

**已知债（登记在案，本卡不处理）**：

1. `scene.material.upsert` 走 `Document::write` 直写——推版本 / 置脏 / 计写次，但**不入撤销栈**，且在 `tx.begin` 期间提交会改到主场景（宿主其余单条写 RPC 同构，非本卡新引入）。命令层 `CommandKind::UpsertMaterial` **早已就绪**（S4-A 落地），缺的只是本 RPC 还没进 `tx.apply` 白名单；
2. 补齐代价很小：`api/tx.rs` 白名单加一条 `scene.material.upsert` → `CommandKind::upsert_scene_material` + 宿主改走 `tx_apply`，随后删掉写点台账里这一处登记。**建议随 S4-E 一起做**（S4-E 要复用材质校验，改同一批文件，避免两次触碰测试基线）。

> **裁决（2026-11-18，用户确认）**：该债**并入 S4-E 一起做**，不单独开卡。执行位置见 S4-E 段「前置补债」，完成后同步删除写点台账（M1-S4-E 已并入 `docs/M1_PLAN.md` §5）中 `r3e-editor-host/src/api/material.rs` 这一处写点登记。
>
> **上述两项已知债已在本卡收口后由 S4-E 前置补债清完**（2026-11-18）：`scene.material.upsert` 两条路都走命令层，写点登记 `r3e-editor-host` 22 → 21、合计 234 → 233。落点与验收见 S4-E 段「前置补债（已完成）」。

其余口径已实测：`material.list` 只读、`material.upsert` 的整行声明语义与 `patch` 系 `*Patch` 明确区分（没给的贴图槽 = 这一行没有贴图，想保留就回传 `list` 的原值）。

---

### S4-E 分层验证器 + 坐标化 Issue

**前置补债（已完成，2026-11-18；S4-D 那笔已知债）**：`scene.material.upsert` 从 `Document::write` 直写改走命令层。落点：

1. `api/tx.rs`：`tx.apply` 白名单加 `scene.material.upsert` → `CommandKind::upsert_scene_material`；`IntentPlan` 同步新增 `MaterialUpsert` 变体并入 `INTENTS`，于是 `tx.intent` 也收这个 op。容量预检（`SCENE_MATERIAL_MAX`）只拦「新增」，与单条路径同一口径——否则事务与非事务两条路对「表满」的判定会分叉。参数解析复用 `material::upsert_from_args` + `decl_from_upsert`，收敛与空名拒绝只做一次。
2. `api/material.rs`：删除 `Document::write` 直写。无事务走 `Document::apply`（**入撤销栈**），有事务走 `document.tx_apply`（进影子，回包 `{name,txId,op,staged,committed:false}`）。
3. `api/support.rs`：`required_str` / `optional_array` 从 `tx.rs` 迁入，并补 `optional_f32` / `optional_str`——两条材质入口的键名口径（`metallicRoughness` / `emissiveMap` / `uvScale`）必须一套，否则同一份参数在两条路上会解析成两行不同材质。
4. `docs/M1_PLAN.md §5`：删 `src/api/material.rs` 写点登记，`r3e-editor-host` **22 → 21**，**合计 234 → 233**（`cargo xtask doc-gate` 打印 233，一致）；该行说明改写为「M1-S4-D +3，全是 `#[cfg(test)]` 夹具」。
5. 测试：host 新增三条——单条 `upsert` **进撤销栈且一次 undo 撤回**、事务期进影子（哈希不动、`list` 看不到、提交后落地、整笔事务只占一格撤销）、`tx.intent` 白名单认 `scene.material.upsert` 且 `metallic` 被搬进表。host 单测 91 → **94**，全仓 **1,677 → 1,680**，`cargo xtask verify` 十项全绿。

| 层 | crate | 判定对象 | 检查项 |
|---|---|---|---|
| 静态层 | `r3e-doc/validate/*`（只依赖 core） | 几何/引用上确定错 | 引用断裂、材质六码（S4-A）、AABB 重叠初筛、AABB 悬空初筛、表容量越界 |
| 物理层 | `r3e-editor-host/src/verify/*`（已依赖 physics） | 物理世界确定错 | 悬空精确判定（向下 `shape_cast` 找支撑面）、穿透精确判定（`overlap_sphere` 逐对候选） |
| 导航层 | `r3e-editor-host/src/verify/*`（新增依赖 nav） | 可达性确定错 | spawn → 目标无路径（报 `disconnect_point`）、spawn 落非 walkable 格、目标落非 walkable 格、region 数异常 |

`Issue` 扩展（已完成）：新增 `category`（**六族**：`Reference|Bounds|Material|Support|Overlap|Reachability`——`nav.*` 归 `Reachability`，不另立 `Nav` 族，族是「问题族」不是「发现它的层」）、`target: Option<String>`、`location: Option<[f32;3]>`、`hint: Option<String>`、`approximate: bool`。全部为**只增不改**，旧字段与旧构造入口（`Issue::error` / `Issue::warn`）签名不变，三十余处既有调用点零改动；新增链式构造 `with_target` / `at` / `with_hint` / `approx`。

`ISSUE_CODES` **24 → 38**（+14）：`overlap.suspect`、`support.suspect`（静态层近似）、`overlap.penetration`、`support.floating`（物理层精确）、`nav.spawn_offmesh` / `nav.target_offmesh` / `nav.no_path` / `nav.region_split`（导航层），以及 S4-A 已落的材质六码；`layer` 白名单扩到 `refs|bounds|material|support|overlap|physics|nav`，自检测试口径同步。

**铁律**：① doc 层只做 AABB 近似并标 `approximate`，物理/导航层出结论后**覆盖**而非并列；② 覆盖按 **`(category,target)` 族级**判定——静态层报 `support.suspect`、物理层确认 `support.floating`，两条码不同但同族同对象，近似那条必须被顶掉；精确条目之间再按 `(category,target,code)` 压重，无 `target` 的汇总类条目不参与覆盖；③ 物理或 nav 不可用时整体降级进 `unchecked` 并写明原因，不报假警；④ 层与层共用同一份几何口径（`node_bounds` / `is_dynamic_node` / `nodes_related`），否则两层对「这个物件多大」各有一套判断。

#### 落地结果（已完成，2026-11-18）

**静态层**（`r3e-doc/src/validate/`，仍只依赖 `r3e-core`）：

- `geom.rs`：唯一一份近似几何口径。`half_extents` 声明优先、否则 `0.5 × |scale|`；`is_dynamic`（`body = "dynamic"` 不判）、`related`（父子体积重叠是层次结构的正常形态，不当穿模）。
- `support.rs`：AABB 悬空初筛，报 `support.suspect`（`Warn` + `approximate`），坐标取**底面中心**；容差 5cm，单次上报上限 128 条，超出合并成一条汇总——一份烂场景报上千条悬空没有可读性。
- `overlap.rs`：AABB 两两求交，**相切不算**（贴边摆放是关卡设计日常），交集体积 > `1e-4` 才报 `overlap.suspect`（对象对名 `a|b` 按名字排序）；节点数上限 512，超限只扫前 512 并明说，宁可少报不拖死编辑器。
- 入口 `validate_geometry(scene)` **故意不进** `Document::validate`：后者判「场景自相矛盾」（结论确定、每次加载必跑），前者判「空间关系可疑」（结论全是近似、只该在验证链里跑）。混在一起会让每次加载都多出一批需要下游自己筛掉的近似条目，`Warn` 就退化成噪声。

**物理层 / 导航层**（`r3e-editor-host/src/verify/{mod,physics,nav}.rs`）：

- `VerifyReport::collect(scene, &physics, Option<&Nav>)` 与 `collect_with_targets(…, &[(String, Vec3)])`；报告面 `issues()` / `unchecked()` / `count_by_layer(Layer)`。
- 物理层：向下 `shape_cast` 找支撑面（`support.floating`）、`overlap_sphere` 逐对候选精确复核（`overlap.penetration`），坐标取重叠中心 / 支撑缺口。
- 导航层：`nav.spawn_offmesh` / `nav.target_offmesh` / `nav.no_path`（报 `disconnect_point`，不是目标点）/ `nav.region_split`（汇总警告，逐点报会淹掉关卡作者）。目标点自己站不住时只报 `target_offmesh`，不再往下算 `no_path`——那是上一个结论的下游噪音。
- 不可用就说不清：物理关闭 / 后端无能力、宿主未接 nav 槽位 / 未烘焙 / 一个可走格都没有，一律进 `unchecked`，一条结论都不下。「全部通过」和「没法检查」是两件事。

**门禁配套（本轮唯一改门禁的地方）**：`xtask invariants` 的 B 源（反例测试）从 `crates/r3e-doc/tests` 扩到 **`crates/r3e-doc/tests` + `crates/r3e-editor-host/tests`**，断言 helper 白名单 2 → 4（`assert_error` / `assert_warn` / `assert_geometry_warn` / `has_code`）。理由：物理层 / 导航层的码只能在 host 侧构造反例（需要物理后端与烘焙好的 navmesh），只扫 `r3e-doc/tests` 会让这 6 条码永远「登记了却没有反例测试」——门禁要么常年红，要么被逼着写没有真实断言的空测试。

**测试**：`r3e-doc` lib 22 → **34**（geom / support / overlap 各自单测）、`tests/validate.rs` 30 → **34**（`support.suspect` / `overlap.suspect` 两条反例走 `Scene` 建档而非 `doc.write`——几何判据读世界变换，刷新它的职责在宿主）、新增 `crates/r3e-editor-host/tests/verify_layers.rs` **9** 条；全仓 **1,680 → 1,705**，`cargo xtask verify` 十项全绿。clippy 拦了一处 `sort_issues(&mut Vec<Issue>)` → `&mut [Issue]`（`ptr_arg`）。

---

### S4-F `scene.verify` + impact + 夹具 + Gate 1 前置

1. RPC `scene.verify`（56 → 57），四层同改；
2. `tx.intent` 的 `impact` 增 `violations`（提交后自动复验，只报新增）；
3. `xtask scene-verify` + 三个反例夹具：`scene_overlap_bad.json`（穿模）、`scene_floating_bad.json`（悬空）、`scene_unreachable_bad.json`（**S4-B 落地后才能做的那个**）；
4. 接入 `cargo xtask verify` 第 11 步；`docs/M1_PLAN.md §5` 台账同步。

**Gate 1 触发条件（V3 改写）**：S4-A…S4-F 全部收口，且 `scene.verify` 对三个反例夹具稳定报出**坐标**、`unchecked` 为空，方可进 S5 Gate 1 演练。

---

## 5. 执行顺序与规模

```
S4-A 材质引用（2–3 提交）
  → S4-B 导航子系统（3–4 提交）
    → S4-C 装配下沉（1–2 提交）
      → S4-D 协议面八条（2 提交）
        → S4-E 三层验证器（2 提交）
          → S4-F verify + impact + 夹具（2 提交 + Gate 1 前置校验）
```

| 指标 | 现值 | 预期 |
|---|---|---|
| RPC 条数 | 48 | 57 |
| workspace crate | 15 | 16（`r3e-nav`） |
| `CommandKind` | 15 | 18 |
| `ISSUE_CODES` | 24 | 38（实测 S4-E 收口 **38**，比预估多 2：`support.suspect` / `overlap.suspect` 与精确码分开登记） |
| 单测 | 1,574 | 1,705（实测 S4-D 收口 1,677、S4-E 收口 **1,705**） |
| 预估工期 | — | 2.5–3.5 周（比 V2 多约 1–1.5 周，Gate 1 相应顺延） |

**S4-B 与 S4-A 无代码依赖，可并行**；S4-C 之后所有卡严格串行。

---

## 6. 风险与回退

| 编号 | 风险 | 影响 | 对策 |
|---|---|---|---|
| R1 | `bind_scene` 下沉改变 runtime 行为 | 物理回归红 | 下沉前后同一批物理测试对拍，`infer` 一行不改 |
| R2 | `Issue` 加字段 / `CommandKind` 增项破坏对外契约 | ai_ide 解析失败 | 只增不改，`capabilities.json` 增项不删项，旧字段全保留 |
| R3 | 材质引用改动扩散到 schema/state_hash/命令/渲染四处 | 任一处漏改 → 存盘不一致 | 四处同改 + 契约测试锁死；`state_hash` 契约测试覆盖新增字段 |
| R4 | 独立贴图导入与 glTF 内嵌贴图资产 id 冲突 | 纹理串图 | 统一 AssetId 分配出口 + 路径去重 + 引用计数；单测覆盖「同一 png 被两处引用」 |
| R5 | 导航烘焙性能不可控（大场景） | 编辑器卡顿 | 先做单线程基线，超阈值再上分层栅格；阈值写入文档 |
| R6 | `max_slope_deg` / `max_step_height` 语义与地形实现不一致 | 可走判定与视觉不符 | 以 `terrain.rs height_at` 为唯一地形真值；三组参数（平地/坡/台阶）夹具对拍 |
| R7 | 三层验证器重复上报 | AI 收到矛盾结论 | 去重键 + `approximate` 覆盖语义 |
| R8 | 能力补齐后 `unchecked` 仍有残留 | Gate 1 验收失真 | `ok` 语义定义为「无 Error」；`unchecked` 单独计数，Gate 1 验收表要求为 0 |

**回退**：S4-A / S4-B 各自独立提交、可单独 revert；`r3e-nav` 是新 crate，删除即回退，不影响其余 crate。

---

## 7. 未决事项

| # | 事项 | 需谁裁决 | 建议 |
|---|---|---|---|
| U1 | 出生点用 `SceneFile.spawns` 声明，还是新增 `NodeKind::SpawnPoint` | 架构 | 用 `spawns` 声明。节点表是「可渲染物体」，出生点没有几何；`spawns` 也不参与渲染装配 |
| U2 | 导航是否本期就支持动态障碍（门 / 移动平台） | 产品口径 | 本期不支持，`NavGrid` 只烘焙静态层；动态障碍进 nav 重烘接口预留（不做实现） |
| U3 | 材质 `uvScale` 是否本期 | 实现细节 | 本期加。贴图平铺是「材质看起来对不对」的最低要求，后补要动渲染 uniform |
| U4 | 材质贴图路径支持内嵌 base64 / 打包归档（`scene.archive.*`） | 架构 | 本期只支持相对路径 + 归档内路径（沿用 `archive` 语义），base64 不做 |
| U5 | `Aabb::intersects` 补进 `r3e-core` | 实现细节 | 补。穿模初筛与栅格化共用，避免各 crate 各写一份 |

---

## 8. 与既有文档的关系

- `docs/AI_ROADMAP.md` §3.2 M1-S4：按本方案改写为 6 卡，删除「S4.5 导航 Sprint」占位，Gate 1 触发条件改写为本文件 §4 S4-F 末段；
- `docs/M1_PLAN.md`：S4 章节按本文件替换，保留原验收口径原文（不可达关卡报坐标）；
- `docs/M1_PLAN.md §5`：S4-A 三条命令 + S4-F 写点须先登记（`doc_gate` 规则 7）；
- `docs/M1_S1D_SUBSTITUTION.md`：`physics_simulate` 承接方细化为 `physics.*` 三方法 + `nav.*` 三方法 + `scene.verify`。
*（内容由AI生成，仅供参考）*
