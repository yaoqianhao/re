---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_acd78303e0bb11f18c5752540024e231
    ReservedCode1: VLIY4M/Gzs1ShtWQx1DtYoXOiEXWdQL/aYXmQ+0rkh655FAXxphIneGtKfn+WaLL2aR+GohDs731UaAGlTXIz4nGveMOK+fKHOygTjUVXZSba0VNVOeuPAELYN6iwOkT9Oc1m6Cj3amsfYFgHMKBcSh8uoCEgavOLDQUERh+IHllg9uchSl9WKKjMxs=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_acd78303e0bb11f18c5752540024e231
    ReservedCode2: VLIY4M/Gzs1ShtWQx1DtYoXOiEXWdQL/aYXmQ+0rkh655FAXxphIneGtKfn+WaLL2aR+GohDs731UaAGlTXIz4nGveMOK+fKHOygTjUVXZSba0VNVOeuPAELYN6iwOkT9Oc1m6Cj3amsfYFgHMKBcSh8uoCEgavOLDQUERh+IHllg9uchSl9WKKjMxs=
---

# M0-S3：`r3e-doc` 立项 —— Document 接管场景唯一真值（方案）

> 状态：方案待确认（未落代码）
> 基线日期：2026-11-15，本地 main = S2 四笔提交（1639d5f / d6c7a59 / a342e64 / 5f63c54）
> 目标卡：`AI_ROADMAP_EXECUTION.md` Sprint M0-S3（S3-01 ~ S3-05）

---

## 0. 摘要（一屏）

| 项 | 结论 |
|---|---|
| 真难点 | 不是路线文档写的"`&mut Scene` 遍地"，而是 **`Rc<RefCell<Scene>>` 被 5 处共享持有**，任意持有者随时可写 |
| 核心设计 | `Document` 持有 `Scene`，**实现 `Deref<Target=Scene>`、拒绝 `DerefMut`**；写只能走 `doc.write(|s| ...)` |
| 改造量（实测） | 写入口 **129** 处（script 91 / host 18 / runtime 11 / script-tests 9）、只读点 **185** 处（Deref 后仅需改名） |
| 熔断判定 | **不触发**：显式 `&mut Scene` 仅 15 处，其中 11 处是"接收者"不改；无跨 crate 散落 `&mut` |
| 工作包 | 5 个（P0 立项 / P1 Document / P2 Command / P3 Validator / P4 入口迁移） |
| 新增测试 | `r3e-doc` ≥45（含 ≥8 个必须失败的负向用例），全量 1364 → ≥1409 |
| 预计提交 | 6~7 笔（按 crate 分迁移提交，便于单笔回退） |
| 零行为变化 | 场景 JSON 格式、帧循环顺序、渲染输入全部不动 |

---

## 1. 实测基线（先纠正文档口径）

### 1.1 路线文档的说法 vs 代码现实

| 项 | `AI_ROADMAP_EXECUTION.md` S3-02 说法 | 实测 |
|---|---|---|
| 写入口形态 | "`core::scene::Scene` 不再被 runtime/editor-host 直接可变借用" | 真正的通道是 `Rc<RefCell<Scene>>`（`borrow_mut()`），不是签名上的 `&mut Scene` |
| `&mut Scene` | 隐含"遍地" | **仅 15 处**，其中 11 处是下游接收者（见 1.3） |

**结论**：S3-02 的验收若按字面执行（只看 `&mut Scene`），第一天就能"通过"，但场景仍然全仓可写——那是假收口。本方案把验收改成"**`Rc<RefCell<Scene>>` 在 core 与 doc 之外绝迹**"，可自动判定（见 §5）。

### 1.2 `Rc<RefCell<Scene>>` 持有者（全部 5 处 + 测试）

| 位置 | 角色 |
|---|---|
| `crates/r3e-runtime/src/app.rs:273` `scene: Rc<RefCell<Scene>>` | 主循环真值持有者 |
| `crates/r3e-runtime/src/port.rs:28` `pub scene: &'a Rc<RefCell<Scene>>` | CapabilityHost 端口，外借 |
| `crates/r3e-script/src/lib.rs:29 / 58 / 73` | 脚本宿主注册与重载钩子 |
| `crates/r3e-script/src/api/mod.rs:47` `pub scene: Rc<RefCell<Scene>>` | `ScriptContext` 字段（364 个绑定共用） |
| `crates/r3e-editor-host/src/api/mod.rs:51` `pub scene: &'a Rc<RefCell<Scene>>` | 39 个 RPC 方法共用 |
| `crates/r3e-script/tests/*.rs`（12 文件） | 测试桩 |

### 1.3 借用点精确计数

| 层 | `scene.borrow_mut()`（写） | `scene.borrow()`（读） |
|---|---|---|
| `r3e-script/src`（17 文件） | **91** | 115 |
| `r3e-editor-host/src`（15 文件） | **18** | 39 |
| `r3e-runtime/src`（app.rs） | **11** | 31 |
| `r3e-script/tests`（12 文件） | **9** | 29 |
| 合计 | **129** | 214 |

写入口分布明细（可直接作为 P4 的分工表）：

| 文件 | 写 | 读 | 文件 | 写 | 读 |
|---|---|---|---|---|---|
| `script/api/light.rs` | 15 | 18 | `script/api/anim_blend.rs` | 7 | 10 |
| `script/api/env.rs` | 12 | 15 | `script/api/anim_morph.rs` | 7 | 13 |
| `script/api/postfx.rs` | 12 | 18 | `script/api/basic.rs` | 6 | 11 |
| `script/api/shadow.rs` | 10 | 15 | `script/api/mod.rs` | 4 | 4 |
| `script/api/text.rs` | 4 | 7 | `script/api/ui.rs` | 4 | 8 |
| `script/api/anim_sync.rs` | 1 | 1 | `script/api/pick.rs` | 0 | 3 |
| `script/api/physics.rs` | 2 | 7 | `script/api/query.rs` | 0 | 6 |
| `script/api/audio.rs` / `input.rs` / `scene_flow.rs` | 0 | 0 | `editor-host/api/{scene,light,env,postfx,shadow,ui_rpc,anim}.rs` | 18 | 39 |

### 1.4 显式 `&mut Scene` 的 15 处（判断可否不动）

| 位置 | 语义 | S3 处置 |
|---|---|---|
| `r3e-physics/src/lib.rs:664/790/1005/1533` | `sync_to_scene` / `step(dt, &mut Scene)` | **不改**，由 `doc.write()` 传入 |
| `r3e-physics/src/rapier_backend.rs`、`physics/tests/ccd_penetration.rs` | 同上 | 不改 |
| `r3e-script/src/lib.rs:24/127` `RebuildHook = FnMut(&mut Scene)` | 热重载钩子 | 改签名收 `&mut Scene` 来源，改由 doc 提供 |
| `r3e-runtime/src/app.rs:2868/2953/2984/3019/3094` | 4 个自由函数 helper | **P4 内消除**（改吃 `&mut Scene` 由 write 闭包给） |
| `r3e-core/src/scene_stack.rs:1`、`core/tests/*` | 内部/测试 | 不动 |

### 1.5 容器结构（决定"字段收口"能否推到 M1）

- `crates/r3e-core/src/scene.rs` = **4053 行**；`Scene` 定义在 301 行。
- 私有字段 4 个：`nodes / order / name_index / next_id`（节点访问本来就必须 `&mut self`）。
- **pub 字段 40 个**（`camera`、`light`、`environment`、`postfx`、`lights`、`ui`、`texts`、`animations`、`skins`、`morphs`、`occlusion`、`environment_map` …）。
- `#[derive(Debug)]`，**无 `Clone`** → 禁止用整场景克隆做 undo（见 §7 R5）。
- `Node: Clone`、`Transform: Clone + Copy` → 命令前像可行。

### 1.6 门禁与测试现状（S2 收尾值）

| 门禁 | 现状 |
|---|---|
| `check-tests` | 1364（core 576 / render 307 / script 132 / editor 101 / asset 61 / host 58 / plugin 47 / audio 29 / physics 28 / runtime 14 / app 11） |
| `lint-size` | 225 文件，白名单 24 项，全部未涨 |
| `dep-check` | 12 crate，违规 **3 条 / 9 条依赖边**（host→asset/audio/physics/render/script；plugin→core/physics；script→audio/physics） |
| `dep_rules.json` | **无 `r3e-doc` 条目** → 新 crate 会被报"未登记规则" |
| `tests_baseline.json` | **无 `r3e-doc` 条目** → 会被报"新 crate，请更新基线" |

---

## 2. 核心设计：三个决定

### 决定 1：`Deref` 只读透明 + 拒绝 `DerefMut`

```rust
impl std::ops::Deref for Document {
    type Target = Scene;
    fn deref(&self) -> &Scene { &self.scene }
}
// 没有 DerefMut（硬约束，见 §5 规则 3）
```

工程杠杆在这里：

- 214 处只读点：`scene.borrow()` → `doc.borrow()`，**字段/方法访问语法完全不变**（`&Ref<Document>` 经 Ref→Document→Scene 两级 deref coercion 到 `&Scene`，`renderer.render(&self.doc.borrow())` 这类调用不用改）。
- 129 处写点：`doc.borrow_mut()` 只给出 `RefMut<Document>`，**必须**改为 `doc.borrow_mut().write(|s| ...)`。
- 结果：编译器成为唯一裁判——任何漏改的写点都会直接编译失败，不存在"改一半还能跑"的灰区。

已抽检的两种真实写法，闭包化模板：

```rust
// 单表达式（占多数，如 api/light.rs 15 处中 14 处这种形态）
- c.scene.borrow_mut().upsert_local_light(light)
+ c.doc.borrow_mut().write(|s| s.upsert_local_light(light))

// 多行块（如 api/env.rs:14 的 camera 绑定）
- let mut s = c.scene.borrow_mut();
- s.camera.position = Vec3::new(px, py, pz);
- s.camera.target = Vec3::new(tx, ty, tz);
+ c.doc.borrow_mut().write(|s| {
+     s.camera.position = Vec3::new(px, py, pz);
+     s.camera.target = Vec3::new(tx, ty, tz);
+ });
```

### 决定 2：S3 只锁"可变借用的来源"，字段私有化推 M1

`Scene` 的 40 个 pub 字段本轮**不动**。理由：

1. 字段私有化要改 ~千处直接字段访问 + 写 getter/setter，是 M1 的量；
2. S3 的价值锚点是"每次写都经过一个函数"，`write()` 这个函数就是 S4 的 `Transaction` / `StateHash` / `UndoStack` 的挂载点；
3. 熔断条款本身就允许这个分界（"先做只读视图 + 写入口收口，把字段私有化推到 M1，并在 Backlog 记一笔"）。

**Backlog 记账（本方案写入）**：`Scene` 字段私有化 + `Scene` 的 `&mut self` API 面收敛 → M1-DOC-FIELDS。

### 决定 3：Command v0 只做"声明式写路径"样板，不做全量迁移

12 条命令覆盖 `editor-host` 现有 18 个写 RPC 的高频面；脚本 91 处写点继续走 `write()`，按域迁 Command 留在 M1。

不做的理由：脚本 API 是 364 个函数的参数搬运层，全量命令化等于在同一 Sprint 重写三条入口（脚本 / RPC / 运行时），风险与工时都不可控。S3 要先拿到"唯一真值 + 唯一写入口"这个结构性收益。

### 共享形态

`Rc<RefCell<Document>>`，与现状同构（单线程渲染线程，不引入 `Arc<Mutex>`）：

| 位置 | S3 后 |
|---|---|
| `app.rs` | `doc: Rc<RefCell<Document>>` |
| `ScriptContext` | `pub doc: Rc<RefCell<Document>>` |
| `editor-host/api/mod.rs` | `pub doc: &'a Rc<RefCell<Document>>` |
| `runtime/port.rs` | `pub doc: &'a Rc<RefCell<Document>>` |

---

## 3. Document API 契约（v0，冻结）

```rust
pub struct Document {
    scene: Scene,
    version: u64,          // 每次 write/apply/replace 自增；S4 StateHash 的节拍器
    dirty: bool,           // 未保存标记
    writes: u64,           // 写次数（诊断/测试用，不进序列化）
}

impl Document {
    pub fn new(scene: Scene) -> Self;
    pub fn from_json_str(src: &str) -> Result<Self, SceneError>;
    pub fn version(&self) -> u64;
    pub fn is_dirty(&self) -> bool;
    pub fn mark_clean(&mut self);
    /// 唯一可变通道：写后 version += 1、dirty = true。
    pub fn write<T>(&mut self, f: impl FnOnce(&mut Scene) -> T) -> T;
    /// 场景切换（app.rs:2101 / scene_stack 走这里），返回旧场景。
    pub fn replace_scene(&mut self, next: Scene) -> Scene;
    pub fn apply(&mut self, cmd: &dyn Command) -> Result<(), DocError>;
    pub fn undo(&mut self) -> Result<(), DocError>;   // v0 单步栈内置，S4 换 UndoStack
    pub fn validate(&self) -> Vec<Issue>;
}
```

**明确禁止**（`doc-gate` 静态拦截）：`DerefMut`、`fn scene_mut(&mut self)`、`fn scene(&mut self)`、任何返回 `&mut Scene` 的公开方法。

`Command` trait（`&self` 双方法，前像在构造期捕获 → S4 批量事务不需要 `&mut` 命令）：

```rust
pub trait Command {
    fn kind(&self) -> CommandKind;
    fn apply(&self, scene: &mut Scene) -> Result<(), DocError>;
    fn undo(&self, scene: &mut Scene) -> Result<(), DocError>;
}
```

12 条命令（每条 apply/undo 成对 + 前像）：

| # | CommandKind | 前像 | 覆盖的写点 |
|---|---|---|---|
| 1 | `SpawnNode(ObjectDesc)` | 新 NodeId | `scene.borrow_mut()` 建档 |
| 2 | `DespawnNode(NodeId)` | `Node` 克隆 + 原 order 位置 | 同上 |
| 3 | `SetTransform { id, new, old }` | `Transform`（Copy） | `api/basic.rs` 6 处 |
| 4 | `SetVisible { id, new, old }` | bool | 同上 |
| 5 | `RenameNode { id, new, old }` | String | 同上 |
| 6 | `SetCamera { new, old }` | `Camera`（Clone） | `api/env.rs` 4 处 |
| 7 | `SetEnvironment { new, old }` | `Option<EnvironmentSettings>` | `api/env.rs` |
| 8 | `SetPostFx { new, old }` | `Option<PostFxSettings>` | `api/postfx.rs` 12 处 |
| 9 | `AddLight(LocalLight)` | key | `api/light.rs` 15 处 |
| 10 | `RemoveLight { key, old }` | `LocalLight` | 同上 |
| 11 | `UpsertUi { key, new, old }` | `Option<UiElement>` | `api/ui.rs` 4 处 |
| 12 | `SetText { key, new, old }` | `Option<ScreenText>` | `api/text.rs` 4 处 |

构造器统一为 `CommandKind::ctor(&Scene, ...) -> Self`（读旧值不写），这是"前像在构造期捕获"的落地点。

---

## 4. `Validator` v0（S3-04）

三类检查，复用 core 已有实现，不重复造：

| 类 | 规则 | 复用 |
|---|---|---|
| 引用存在 | `Node.mesh` 指向的 `asset:` / `MeshRef` 在 `scene.assets` 或 `MeshAabbCache` 可解析；`animations[i].clip` 在 `anim_clips` 中存在；`skins[i]` 关节索引 < 骨骼数 | `air::validate(_parts)`、`morph::validate_against` |
| 索引越界 | 动画通道索引、morph target 索引、UI/文本层级索引 | `morph::validate`、`scene_stack::validate` |
| 必填非空 | 节点名非空且唯一（`name_index` 自洽）、`ui` key 非空且唯一、材质名非空、`bones` 非空 | `mesh::is_valid`、`occlusion::is_valid` |

`Issue { severity: Error|Warn, code: &'static str, path: String, msg: String }`；`validate()` 不做修复、不 panic。

**负向测试（铁律，≥8 条，全部必须报 `Error`）**：
1. 节点引用不存在的 mesh；2. skin 关节索引 = 骨骼数（越界 1）；3. morph 权重长度 ≠ target 数；4. 空节点名；5. 重名节点；6. UI key 重复；7. 动画实例引用不存在的 clip；8. `MAX_LOCAL_LIGHTS` 超限；9. 场景 `physics` 启用但 `gravity` NaN。

**正向测试**：仓库现有 5 份场景 JSON + `FALLBACK_SCENE` 全部 `validate()` 零 `Error`（保证 Validator 不是"什么都报错"的假阳性机器）。

### 4.1 P3 实测偏差（落地时纠正，留痕以免后期误判）

上面表格是**方案期口径**，落地时发现有两类规则在 doc 层判不了，已按下述方式纠正。P5 文档同频时以本小节为准（届时删除方案期表述）。

| 方案期规则 | 为什么 doc 层判不了 | 落地替代 | 承接 |
|---|---|---|---|
| 引用存在：`Node.mesh` 的 `asset:` / `MeshRef` 可解析、`skins` 关节索引 < 骨骼数、`animations.clip` 在 `anim_clips` 中存在 | `MeshRef::Asset` 带的是**路径哈希** `AssetId(u32)`（`AssetId::from_path`），不是 `assets` 下标；骨骼数与 clip 存在性都要网格 / 资产导入结果，doc 只依赖 `r3e-core` | ① 物件 `asset:` 引用与 `assets[].path` 反推哈希比对，未声明即报 `ref.asset_missing`；② 动画绑定 / 蒙皮网格节点 / 蒙皮关节 / 形变网格节点做**节点存在性**检查（`ref.node_missing`）；③ 蒙皮关节矩阵与关节表长度不一致报 Error，未求解只报 Warn | M1 `DOC-ASSET-VALIDATE`：等 Validator 能拿到 `MeshData` / `MeshAabbCache` 再补资产级校验 |
| 索引越界：morph 权重长度 ≠ target 数 | 权重数组按 `MAX_MORPH_TARGETS` 固定长度，长度本身恒等；真正的不一致要对照网格的 target 数量 | 改判 `morph.target_count > MAX_MORPH_TARGETS`（越界）与权重值 NaN/Inf | 同上 |

落地实际规则（单测硬编码的 code，14 个）：
`node.name.empty`、`node.name.duplicate`、`node.transform.non_finite`、`node.color.non_finite`、`node.material.non_finite`、`ui.key.empty`、`ui.key.duplicate`、`text.key.empty`、`text.key.duplicate`、`light.key.empty`、`light.key.duplicate`、`light.value.non_finite`、`lights.over_limit`、`ref.node_missing`、`ref.asset_missing`、`skin.joint_matrices.len`、`skin.joint_matrices.unsolved`（Warn）、`morph.target_count.over_max`、`morph.weight.non_finite`、`camera.non_finite`、`scene.background.non_finite`、`physics.gravity.non_finite`。

负向用例 **17 条**（方案期写 ≥8，超额不减），正向回归从 5 份场景扩到 **11 份 `assets/scene*.json` + 4 份 morph fixture**——场景文件越多，假阳性越难藏。

---

## 5. 强制机制：`cargo xtask doc-gate`（新增子命令，纳入 `verify`）

架构约束靠文档约束不住，必须落到 CI 命令。6 条规则：

| # | 规则 | 例外白名单 |
|---|---|---|
| 1 | `RefCell<Scene>` / `Rc<RefCell<Scene>>` 只允许出现在 `crates/r3e-doc/` | `r3e-core` 自身单测 |
| 2 | 返回或接收 `&mut Scene` 的**公开** API 只允许在 `r3e-doc` | `r3e-physics`（`sync_to_scene` / `step`，下游接收者）、`r3e-core`（内部） |
| 3 | `DerefMut` 一词在 `crates/r3e-doc/` 中出现即失败 | 无 |
| 4 | `runtime` / `editor-host` / `script` 不得出现 `Scene::from_json_str`，场景解析统一走 `Document::from_json_str` | 无 |
| 5 | 未登记 `dep_rules.json` 的 crate 一律失败 | 无（现状已具备） |
| 6 | `doc.write(|s| ...)` 闭包体内不得出现 `.borrow()` / `.borrow_mut()`（RefCell 重入静态扫描） | 无 |

实现要点：复用 `rust_files()` / `rel()` 现有工具，约 120 行；输出格式与 `lint-size` 一致（违规逐条列出 = crate + 文件 + 行号 + 规则号）。

---

## 6. 目标文件树（全部 ≤800，符合 lint-size）

```
crates/r3e-doc/
├── Cargo.toml                 # 依赖仅 r3e-core
└── src/
    ├── lib.rs          (~180)  Document / Deref / DocError / 再导出
    ├── write.rs        (~140)  write 门禁 + version / dirty / writes
    ├── command/
    │   ├── mod.rs      (~140)  Command trait / CommandKind / 分发 / undo 栈
    │   ├── scene.rs    (~280)  1~5（spawn/despawn/transform/visible/rename）
    │   ├── env.rs      (~240)  6~10（camera/env/postfx/light×2）
    │   └── ui.rs       (~200)  11~12（ui/text）
    └── validate/
        ├── mod.rs      (~150)  Issue / Severity / 入口汇总
        ├── refs.rs     (~220)  引用存在（mesh / clip / bone / asset）
        └── bounds.rs   (~200)  索引越界 / 必填非空 / 唯一性
```

---

## 7. 工作包与分批

| 包 | 内容 | 卡 | 产出 | 验收 |
|---|---|---|---|---|
| **P0** | 立项 `r3e-doc`；同步 `dep_rules.json` / `tests_baseline.json`（r3e-doc: 0）；`cargo tree -p r3e-doc` 只 core | S3-01 | crate 骨架 | `dep-check` 无"未登记"、`r3e-doc` 一列零违规 |
| **P1** | `Document` v0：new / from_json_str / write / version / dirty / replace_scene / `Deref` | S3-02 | lib.rs + write.rs | 单测 ≥12（Deref 只读、write 后 version+1、replace 返回旧场景、writes 计数、脏标记、from_json_str 失败路径） |
| **P2** | `Command` v0 12 条 + 单步 undo | S3-03 | command/* | 单测 ≥24（每命令 apply 生效 + undo 复原各 1） |
| **P3** | `Validator` v0 + 负向 17 条（方案期写 8/9，见 §4.1 偏差） | S3-04 | validate/* | 负向全绿（必须报 Error），正向 11 场景 + 4 fixture 全过 |
| **P4** | 入口迁移：script 91+115 / host 18+39 / runtime 11+31 / script-tests 9+29；`RebuildHook` 收口；app 4 个 helper 消化；`replace_scene` 接场景切换 | S3-05 | 5 处持有者全部改 `Rc<RefCell<Document>>` | `doc-gate` 零违规 + 全量测试绿 + 实机帧行为不变 |
| **P5** | `xtask doc-gate` 落地 + 文档同频 | — | main.rs + md | `verify` 全绿 |

P0~P3 可在 `r3e-doc` 内独立完成并提交（不动任何现有 crate，风险为零）；**P4 是唯一有回归风险的工作包**，按 crate 拆 4 笔提交，每笔单独 `cargo test -p <crate>` 可回退。

---

## 8. 依赖规则变更（必须显式记账）

```jsonc
"r3e-doc":        { "allow": [], "note": "文档层只许依赖 core：唯一真值 + 写入口" },
"r3e-script":     { "allow": ["r3e-core", "r3e-doc"], "note": "脚本层经 doc 写入口改场景，不再直连 Scene 可变借用" },
"r3e-runtime":    { "allow": [... , "r3e-doc"], ... },
"r3e-editor-host":{ "allow": ["r3e-core", "r3e-editor", "r3e-runtime", "r3e-doc"], ... }
```

- 方向依然向下：`script/host/runtime → doc → core`，不产生环。
- `dep-check` 总违规必须**仍是 3 条**（9 条依赖边），不得因新增 doc 边而涨。
- `editor-host` 的 note 原文已写"只许 protocol + doc + runtime"，本次是把 doc 变成实体。

---

## 9. 风险与熔断

| 编号 | 风险 | 判据 / 应对 |
|---|---|---|
| R1 | 闭包化改变控制流：`write(\|s\| { ... return ... })` 里的 `return` 只退闭包不再退函数 | 129 处逐点核；`?` 同理。已抽检形态：单表达式链式调用占多数，多行块用闭包体 + `write` 返回值承接 |
| R2 | RefCell 重入 panic 面扩大（闭包体内再 `borrow`） | `doc-gate` 规则 6 静态扫描；运行时由既有测试与实机跑覆盖 |
| R3 | 场景切换漏改（`app.rs:2101` 重建 Scene + `scene_stack` 回退） | 统一走 `replace_scene`；单测断言"切换后 version 不自增回绕、旧场景被返回" |
| R4 | 既有 crate 测试数下降 | `check-tests` 硬门禁；P4 每笔提交前跑该 crate 的测试 |
| R5 | 用整场景克隆做 undo/快照 | `Scene` 无 `Clone`（实测），且 4053 行数据量；命令前像一律局部字段 |
| R6 | 性能回归 | `write` 是 `FnOnce` 零分配、`version += 1` 一条 add；不新增逐帧扫描 |
| **熔断** | 路线文档给的熔断条件"若 `Scene` 被全局可变借用太多（跨 crate `&mut` 遍地）" | **实测不触发**：显式 `&mut Scene` 15 处、写入口 129 处且 100% 经 `borrow_mut()` 单一形态、无散落跨 crate `&mut`。**S3 走完整收口**，字段私有化按 §2 决定 2 推 M1 |

---

## 10. 不做（明确排除）

- `Scene` 40 个 pub 字段的私有化 / getter 化 → **M1-DOC-FIELDS**
- `Transaction` / `preview` / `StateHash` / 确定性回放 → **M0-S4**
- 脚本 364 个函数全量命令化 → **M1**
- `r3e-protocol` 从 `r3e-editor` 剥离 → **M0-S5**
- 场景 JSON 序列化格式变更 → 不动（零行为变化的前提）
- 多线程 / `Arc<Mutex<Document>>` → 不做（当前单渲染线程）

---

## 11. 验收清单（全部可自动判定）

```bash
cargo test -p r3e-doc                      # ≥45 绿，含 ≥8 个"必须失败"的负向用例
cargo xtask doc-gate                       # 6 条规则零违规
cargo xtask dep-check                      # r3e-doc 零违规，总违规仍 3 条
cargo xtask lint-size                      # 24 项未涨，新文件 ≤800
cargo xtask check-tests                    # 全 crate 只涨不跌，新增 r3e-doc 行
cargo test --workspace                     # ≥1409（1364 + ≥45）
cargo clippy --workspace --all-targets     # 0 告警（S2 现状 rc 0）
cargo run -p r3e-app -- --run-seconds 20   # 帧行为不变：帧数 / 节点数 / 绘制调用与 S2 日志一致
```

---

## 12. 提交规划（预计 6~7 笔）

1. `doc: 立项 r3e-doc + Document v0（Deref 只读 / write 门禁）` — P0+P1
2. `doc: Command v0 十二条 apply/undo` — P2
3. `doc: Validator v0 + 9 条负向用例` — P3
4. `doc: xtask doc-gate 六条规则 + 规则表/测试基线登记` — P5（前半）
5. `迁移: runtime 从 Document 取场景` — P4/runtime
6. `迁移: script + editor-host 入口收口` — P4/script+host（若过大则拆两笔）
7. `docs: M0S3 方案 + 看板同频` — P5（后半）

---

## 13. 文档同频清单（S3 收尾时一并刷新）

| 文件 | 改什么 |
|---|---|
| `docs/AI_ROADMAP_EXECUTION.md` | S3-01~05 打勾；熔断结论落"未触发（实测 129 写点 / 15 处 `&mut`）"；S4 卡片前置条件补"doc.write 已就位" |
| `docs/ARCHITECTURE.md` | 新增 doc 层在依赖图中的位置与写入口说明 |
| `docs/AI_ROADMAP.md` | §2.2 依赖图 `S3 r3e-doc` 节点由虚线转实线 |
| `xtask/dep_rules.json` | 4 处：新增 r3e-doc + script/runtime/editor-host 加 doc |
| `xtask/tests_baseline.json` | 新增 `r3e-doc` 行 + total 刷新 |
| 看板 | 新增一行（日期 2026-11-1x）：最大文件 / `>800` 数 / 测试数 / 违规数 |

---

## 14. S3 之后（S4 的接口已预留）

- `doc.write()` 是 `Transaction::apply` 的唯一落地点 → S4 的原子性只需在 write 外面套"前像栈"。
- `Document::version` 是 `StateHash` 的节拍器 → S4-03 只需对 `scene` 做稳定序列化后哈希，无需新增真值容器。
- `Command` 的 `&self` + 构造期前像 → S4-02 的"第 k 条失败自动回退"可以直接倒序 `undo()`，不需要命令可变状态。
*（内容由AI生成，仅供参考）*
