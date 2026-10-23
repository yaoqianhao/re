---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_bbbaed85ced411f1acb8525400638852
    ReservedCode1: pdkvi+EGIrTSJHMbkgwrPnfButCvCNJ5KdcVlGzNoM3AjUmgkoaHOiVqVUqmXfFjBF9KeScdJcQcPqCDEfc29q9oCY7LI+kS5Q915f6PZB/bp3SSm0AEvSPWLE6EO/TDJ1vSUrSvhC7PVw4+vSvp+OGcOdqXEmJwXNb8aJ8+wrCHRMpADJf+R5ywhYM=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_bbbaed85ced411f1acb8525400638852
    ReservedCode2: pdkvi+EGIrTSJHMbkgwrPnfButCvCNJ5KdcVlGzNoM3AjUmgkoaHOiVqVUqmXfFjBF9KeScdJcQcPqCDEfc29q9oCY7LI+kS5Q915f6PZB/bp3SSm0AEvSPWLE6EO/TDJ1vSUrSvhC7PVw4+vSvp+OGcOdqXEmJwXNb8aJ8+wrCHRMpADJf+R5ywhYM=
---

# r3e 资产层规划：FBX 外部资产加载

> 状态：**规划中，尚未实现**（本文档是下一步的工作定义，不是现状描述）
> 当前代码现状见 [`ARCHITECTURE.md`](ARCHITECTURE.md)；本文档描述「加载 `.fbx` 资产并放入场景」这一阶段的
> 目标、改动面、数据结构、阶段拆分与验收标准。

---

## 1. 为什么是现在

引擎到目前为止刻意保持「零外部资源依赖」：几何只有 `cube / sphere / plane / cylinder` 四种程序化网格，
材质是每物件一个 RGBA 纯色，动画只能靠脚本每帧改 transform。

这条设计让引擎从零到可运行几乎没有门槛，但它同时是当前最大的表达力边界：

| 现状 | 后果 |
| --- | --- |
| 只有 4 种内置网格 | 任何真实造型（角色、载具、场景道具）都做不出来 |
| 无 UV、无贴图 | 表面只能是纯色 + Blinn-Phong 高光 |
| 无动画数据 | 「模型动起来」必须逐帧算三角函数写进脚本 |

`.fbx` 是 DCC 工具链（Blender / Maya / 3ds Max / ZBrush）事实上的通用交换格式，
一个文件里同时带**模型、贴图、材质、动画**四类信息，正好覆盖上述全部缺口。
因此下一步定为：**能加载 `.fbx`，并把其中的资产放进场景**。

---

## 2. 目标与不做的事

### 2.1 目标（本阶段要达成的）

1. 从 `.fbx` 读出**静态网格**（三角化后的顶点位置 / 法线 / UV），作为物件放进场景并按 `Transform` 摆放。
2. 读出**材质**（base color、金属度 / 粗糙度近似映射到现有 Blinn-Phong 参数）与**贴图**
   （支持嵌入贴图与外链贴图两种）。
3. 读出**动画栈**（节点级 TRS 关键帧曲线），能被脚本按名字播放、暂停、循环。
4. 一条命令能加载并看到画面：场景 JSON 里一行 `assets` 声明 + `mesh: "@robot"` 就能出模型。
5. 保持既有设计原则不破：**文本即接口**（资产也是文本里声明）、**热重载**（改场景/脚本 ≤0.4s 生效）、
   **分层单向依赖**（core 不认识 wgpu，render 不认识文件 IO）。

### 2.2 不做的事（明确排除，避免范围失控）

| 排除项 | 理由 |
| --- | --- |
| 骨骼蒙皮（skin / joint weights） | 需要新增顶点着色器 skinning 路径、关节调色板 uniform、骨架层级图，改动量约等于前四项之和，单独立一阶段 |
| 形变目标（morph target / blend shape） | 同上，属于顶点级动画 |
| 动画混合 / 状态机 / 过渡 | 属动画系统，不属资产加载；先给「播一个 clip」 |
| FBX 写出（导出） | 引擎只读 |
| 非 FBX 格式（glTF / OBJ / USD） | 分层设计会留出扩展点，但本阶段只做通一条链路 |
| 内嵌编辑器的资产浏览器 | 无 GUI，用 CLI 与 REPL 观测 |
| 纹理压缩（BCn / KTX2） | 先直接 `Rgba8UnormSrgb` 上传 |

---

## 3. 依赖选型

| 候选 | 结论 | 理由 |
| --- | --- | --- |
| **`ufbx`**（0.11） | **采用** | 单文件 C 库（`ufbx.h` / `ufbx.c`），无外部依赖；对 ASCII / 二进制 FBX 7.x 覆盖完整；API 已经把「三角化面」「按材质分组」「嵌入贴图 blob」「动画曲线采样」都做好了；本机已用它成功解析 4 个不同来源的样本文件（见 §9） |
| `fbxcel` | 备选 | 纯 Rust，无 C 工具链依赖，但格式覆盖与文档成熟度不及 ufbx |
| `assimp` | 否决 | 依赖较重、构建链复杂，且把资产转换逻辑藏进 C++ 侧，不利于按引擎需要裁剪 |
| 自写解析器 | 否决 | 二进制 FBX 的类型系统（属性数组、deflate 压缩段）工作量大且没有额外收益 |

引入代价：`ufbx` crate 通过 `cc` 编译一份 C 源码，需要本机有可用的 C 编译器。
本机是 `x86_64-pc-windows-gnu`（MinGW-w64），第一阶段第一件事就是验证 `cargo build` 能过，
不通则退到 `fbxcel`——这条验证放在 P0，避免后面所有工作建在沙地上。

坐标与单位统一：加载时显式设置 `LoadOpts.target_axes = (+X right, +Y up, +Z front)` 与
`target_unit_meters = 1.0`，由 ufbx 在读取阶段完成换算，引擎内部只认「Y 轴向上、单位米」。
（不同 DCC 导出的 FBX 朝向与单位差异极大，实测样本里就有一个 `up = +Z`、`unit = 0.0254`（英寸）的文件。）

---

## 4. 架构落点

### 4.1 新增 crate：`r3e-asset`

```
crates/
├─ r3e-core/     ← 新增纯数据类型（顶点、网格、材质、动画 clip、句柄）
├─ r3e-asset/    ← 新增：FBX 解析、坐标换算、资产缓存、贴图解码
├─ r3e-render/   ← 网格缓存键从枚举改为句柄；新增材质 bind group 与纹理上传
├─ r3e-script/   ← 新增 load_asset / create_asset_node / play_anim 等 API
└─ r3e-app/      ← 组装：场景加载后灌资产、每帧推进动画
```

依赖方向（保持严格单向，不出现环）：

```
app → asset → core
app → render / script / physics → core
asset ↛ render    （asset 只产出 CPU 数据，不碰 wgpu）
render ↛ asset    （render 只吃 core 里的网格/材质数据）
```

**为什么 `Vertex` / `MeshData` 要下沉到 `r3e-core`**：现在它们在 `r3e-render/src/mesh.rs` 里，
若 asset 直接引用会导致 `asset → render`，破坏「render 是叶子」的结构。
下沉后这两个类型只是 `[f32; N]` 数组 + `Vec<u32>`，不含任何图形 API 概念，core 的无依赖约束依然成立。

### 4.2 句柄化：`MeshKind` 从值类型变引用类型

这是**唯一的结构性改造**，也是文档里早就标注的坑（见 ARCHITECTURE 第 12 章）。

```rust
// crates/r3e-core/src/node.rs —— 已落地
pub enum BuiltinMesh { Cube, Sphere, Plane, Cylinder }   // 原 MeshKind 的内置部分，语义不变

pub struct AssetId(pub u32);         // 由归一化路径的 FNV-1a 散列生成，跨会话稳定

pub enum MeshRef {
    Builtin(BuiltinMesh),
    Asset { asset: AssetId, mesh: u32 },   // 资产内的第 mesh 个子网格
}

// crates/r3e-core/src/mesh.rs —— 已下沉
pub struct Vertex { pub position: [f32; 3], pub normal: [f32; 3], pub uv: [f32; 2] }
pub struct MeshData { pub vertices: Vec<Vertex>, pub indices: Vec<u32> }

// crates/r3e-asset/src/lib.rs —— 资产层的富数据（比 MeshData 多材质分区与元信息）
pub struct MeshAsset {
    pub positions: Vec<[f32; 3]>,    // 已过 geometry_to_world（单位与轴向换算在此完成）
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,          // 已按 v' = 1 - v 翻转
    pub indices: Vec<u32>,           // 角点号已映射为顶点号
    pub material_parts: Vec<MaterialPart>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub node_local_matrix: [[f32; 4]; 4],
    // name / node_path / source_path / source_{vertex,face,triangle}_count
}
```

> 命名差异说明：`MeshKind` 拆成 `BuiltinMesh`（内置四件套）+ `MeshRef`（引用），
> 这样内置网格与资产网格在类型上不再混为一谈；`render` 侧按 `MeshRef` 建 `MeshCache`，
> 而不是按「枚举 + 资产 id」两套键。

连带的三个语义变化：

1. **`Scene` 多一张表**：`meshes: HashMap<AssetId, MeshAsset>` + `asset_index: HashMap<String, AssetId>`。
   网格数据与场景同寿命，`despawn` 物件**不回收**网格（多个物件可共享同一份）。
2. **场景 JSON 的 `mesh` 字段**：仍接受 `"cube"` 等内置名（向后兼容），新增 `"@robot"` 形式指向 `assets` 段声明。
3. **物理碰撞体推导**：`MeshKind::Asset` 没有天然的半尺寸，`BodyDesc::infer` 改为读 `MeshAsset::aabb`
   （AABB 盒）——脚本 `attach_body` 对导入模型立即可用，不做凸包。

### 4.3 渲染侧：分组键与材质

```rust
// 现状：groups: Vec<(MeshKind, Range<u32>)>，固定 4 组
// 计划：groups: Vec<(MeshKey, Range<u32>)>，按实际使用的网格 key 排序分组
enum MeshKey { Builtin(MeshKind), Asset(AssetId) }
```

- 网格缓存 `HashMap<MeshKey, GpuMesh>` 惰性上传，key 相同即复用；
- 顶点缓冲布局扩到 `stride 32`：`position(12) + normal(12) + uv(8)`；
- 材质从「实例上的 `color: vec4`」升级为**材质表**：`Vec4 base_color + 3 个标量参数 + 贴图句柄`，
  按材质建 bind group（`texture_2d + sampler + material uniform`）；
  `InstanceRaw` 增加一个 `material_index: u32`（实例缓冲 80 → 96 字节，仍 16 字节对齐）；
- 没有材质的物件走默认材质（base_color = 原 `color`，无贴图），**行为与现在完全一致**。

draw call 上限从「≤4」变成「≤ 使用的网格数 × 材质数」。这是外部资产的固有代价（每个子网格顶点缓冲不同），
优化留到 P2 之后：把同一材质的子网格合并进大缓冲、用 `draw_indexed` 的 range 切分，可压回「≤ 材质数」。

### 4.4 动画：节点级 TRS 播放器

```rust
// core
pub struct AnimClip {
    pub name: String,
    pub duration: f32,               // 秒
    pub tracks: Vec<AnimTrack>,      // 每个 track 对应一个节点相对路径
}

pub struct AnimTrack {
    pub target: Vec<String>,         // 节点路径，如 ["Hips", "Spine", "Head"]
    pub keys: Vec<AnimKey>,          // 已按时间排序
}

pub struct AnimKey { pub t: f32, pub pos: Vec3, pub rot: Quat, pub scale: Vec3 }

// Node 上增加
pub anim: Option<AnimState>,         // { clip: AssetId, time: f32, speed: f32, looping: bool }
```

- 采样在 `app` 帧序里、`scripts.tick` 之后、`physics.step` 之前：对每个带 `AnimState` 的节点，
  `time += dt * speed`（`looping` 时取模），线性插值出 TRS 写回 `transform`。
- 曲线插值模式（linear / step / cubic）在加载阶段统一 bake 成线性关键帧，运行时只做一次 lerp/slerp——
  避免把三种插值模式的分支带进每帧热路径。
- 动画作用的是**节点自身的 `transform`**，因此与脚本 `set_pos` 会互相覆盖：同一物件
  「脚本驱动」与「动画驱动」二选一，规则写进文档与 README，不做优先级仲裁。

### 4.5 资产缓存与热重载

```rust
pub struct AssetDb {
    cache: HashMap<PathBuf, CachedAsset>,   // key = 规范化绝对路径
}
struct CachedAsset {
    mtime: SystemTime,
    size: u64,
    meshes: Vec<MeshAsset>,
    materials: Vec<MaterialAsset>,
    images: Vec<ImageAsset>,     // 已解码的 RGBA8
    clips: Vec<AnimClip>,
}
```

- 缓存键带 `mtime + size`：文件变了就重新解析，没变就复用，避免每次热重载重读几百 MB。
- 贴图外链路径解析：先按 FBX 内记录的相对路径相对 FBX 所在目录找，再退到「只取文件名、查 FBX 目录及其 `textures/` 子目录」。
  找不到不报错退出，只 `warn` 并退化为该材质无贴图。
- 嵌入贴图（`texture.content` 非空）直接取 blob 解码，不落盘。
- 热重载链路不变：**重放场景 → 重跑脚本**，资产表随之重建；`AssetDb` 的缓存让「重放」不付出重复解析代价。

---

## 5. 对外接口（文本，全部保持可手写）

### 5.1 场景 JSON

```json
{
  "name": "fbx_demo",
  "assets": [
    { "name": "robot", "path": "assets/models/robot.fbx" }
  ],
  "objects": [
    { "name": "robot_1", "mesh": "@robot", "position": [0, 0, 0], "rotation": [0, 90, 0] },
    { "name": "robot_2", "mesh": "@robot", "position": [3, 0, 0],
      "anim": { "clip": "Idle", "speed": 1.0, "looping": true } }
  ]
}
```

- `assets[].path` 相对**项目根**；`assets[].name` 是脚本与 `mesh` 字段引用用的短名。
- `mesh: "@robot"` 表示「取该资产的第一个子网格」；需要指定子网格写 `"@robot#Body"`。
- 一个资产被多个物件引用 = 只解析一次、只上传一次 GPU 缓冲。

### 5.2 脚本 API（新增）

| 函数 | 说明 |
| --- | --- |
| `load_asset(name, path) -> bool` | 运行时加载，返回是否成功 |
| `create_asset_node(name, asset, x, y, z) -> bool` | 用资产建物件（可选 `submesh` 版本） |
| `asset_info(name) -> string` | 统计摘要：子网格数 / 顶点数 / 材质数 / 贴图数 / 动画名列表 |
| `anim_clips(name) -> array` | 该资产可播放的动画名 |
| `play_anim(node, clip, looping) -> bool` | 给物件挂上播放器 |
| `stop_anim(node) -> bool` | 停止并保留当前姿态 |
| `set_anim_speed(node, s) -> bool` / `anim_time(node) -> f64` | 调速 / 读进度 |
| `set_material_color(node, r, g, b)` | 覆盖材质基色（调试用） |

### 5.3 命令行

| 参数 | 说明 |
| --- | --- |
| `--dump-asset <fbx>` | 只解析并打印统计后退出，不进窗口。P0/P1 的验证手段，也是排查「模型加载了但看不见」的第一入口 |
| `--asset <fbx>` | 导入 FBX，并按资产内节点路径逐个 spawn 到场景（可重复传入多个文件），已落地 |
| `--assets-root <dir>` | 覆盖默认资产根目录（默认项目根 `assets/`），尚未落地（P1 后续） |

---

## 6. 阶段拆分与验收

按「可独立验证、每阶段结束都能跑起来」切分，每阶段一个提交：

### P0：管道打通（不含渲染）—— ✅ 已完成

- 新建 `r3e-asset` crate，接 `ufbx`，完成坐标/单位换算与三角化；
- `--dump-asset` 打印：子网格数 / 顶点与三角形数 / 材质名 / 贴图（嵌入 or 外链 + 是否存在）/ 动画名与时长；
- 验收：对 4 个来源不同的样本（Blender 2.7x、Blender 2.9x 带嵌入贴图、3ds Max 带外链贴图、Maya 带动画）
  统计数字全部正确，且 MinGW 下 `cargo build` 零 warning。
- **不碰渲染器**，风险最低，先把「解析对不对」这件事单独坐实。

### P1：静态网格进场景 —— ✅ 已完成（2026-10-23）

- `Vertex` / `MeshData` 下沉 core，加 `uv` 字段，`stride 24 → 32`；
- `MeshKind::Asset(AssetId)` + `Scene::meshes` 表 + 场景 JSON `assets` 段；
- render 的网格缓存键改为 `MeshKey`，分组数量动态化；
- `bootstrap` 里按 `assets` 段预加载；
- 验收：`assets/scene_fbx.json` 能显示导入模型的正确造型与朝向；1000 个导入物件仍是「网格数」次 draw call。

### P2：材质与贴图

- 材质表 + 每材质 bind group + 纹理上传（`Rgba8UnormSrgb`，UV 采样）；
- `MaterialAsset` 字段映射：`pbr.base_color` → `base_color`；`pbr.metallic / roughness` → 高光指数与强度的近似；
- 采样器：`Linear` + `Repeat`，无 mipmap（后续可加）；
- 验收：带贴图样本（嵌入 + 外链各一）颜色正确、无 UV 错位、无上下颠倒（FBX 的 V 轴与 wgpu 的 UV 原点不同，需显式翻转）。

### P3：节点级动画

- `AnimClip / AnimTrack / AnimState` + 每帧采样；
- 场景 JSON `anim` 段 + 脚本 `play_anim` 系列；
- 验收：Maya 动画样本能连续播放，`speed` / `looping` 生效；与脚本 `set_pos` 的覆盖关系符合文档描述。

### P4（后置）：骨骼蒙皮

单独立项：新增 `SkinAsset`（关节索引 + 权重）、关节层级与逆绑定矩阵、顶点着色器 skinning、
关节矩阵 uniform（或 storage buffer）。P0–P3 的数据结构为此预留：`Vertex` 再加 `joints/weights` 两个属性，
`MeshAsset` 再多一个 `skin: Option<AssetId>`，不需要推翻已有设计。

---

## 7. 风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| `ufbx` 的 C 源码在 MinGW 下编译不过 | 整条链路阻断 | P0 第一件事就验证；不通则切 `fbxcel`（纯 Rust），数据结构不变 |
| 不同 DCC 的轴向/单位差异 | 模型躺倒、比例离谱 | 统一由 `LoadOpts.target_axes` + `target_unit_meters` 在解析期解决，引擎内部只认 Y-up / 米 |
| FBX 的 UV 原点与 wgpu 相反 | 贴图上下颠倒 | 加载期将 `v` 翻转为 `1.0 - v`，并在文档中固化这一约定 |
| 顶点去重导致顶点数暴涨 | 内存与带宽 | 按 `(pos_idx, normal_idx, uv_idx)` 三元组做哈希去重，硬边法线天然被保留 |
| 子网格过多导致 draw call 暴涨 | 帧率下降 | P2 后做「同材质合并大缓冲 + range 绘制」；P1 先如实记录实测上限 |
| 动画曲线插值模式差异（linear/step/cubic） | 动画抖动或跳变 | 加载期 bake 成线性关键帧，运行时只做 lerp；step 用重复帧点表达 |
| 资产失控（大文件、贴图未找到） | 启动卡顿、材质全黑 | 缓存 + `warn` 降级 + `--dump-asset` 可观测；单文件超过阈值时打印耗时 |

---

## 8. 与既有设计原则的对齐

| 原则 | 本阶段如何继续成立 |
| --- | --- |
| 零外部资源依赖 | **被有条件放宽**：内置 4 种网格与默认材质仍在，没有 FBX 的工程照样 `cargo run` 出画面；兜底场景不含资产 |
| 文本即接口 | 资产声明写在场景 JSON，加载/播放由脚本一行调用，`--dump-asset` 可命令行验证 |
| 分层单向依赖 | `render` 仍不认识文件 IO 与 ufbx，`core` 仍不认识 wgpu；新 crate 只向上提供 CPU 数据 |
| 单一真源 | 网格与动画数据挂在 `Scene` 上，脚本/渲染/物理读同一份，不引入第二套副本 |
| 热重载 | 重放场景 + 资产缓存（mtime/size 键），改 FBX 后 ≤0.4s 生效 |
| 可无头测试 | `r3e-asset` 的解析与采样是纯 CPU 逻辑，可在无窗口测试里断言顶点数、包围盒、动画采样值 |

---

## 9. 已完成的前置验证（本规划的实测依据）

以下结论来自本机真实跑过的解析验证，不是纸面推断：

| 样本来源 | 内容 | 验证结果 |
| --- | --- | --- |
| Blender 2.72 导出 | 纯立方体 | 8 顶点 / 12 三角形，无 UV、无材质——说明「没有 UV 的网格」必须能正常退化 |
| Blender 2.93 导出 | 嵌入贴图 + PBR 材质 | 1 个材质（`base_color = 0.8`，`base_color_texture` 有效），5 张贴图全部为内嵌 blob 且解出可用图片 |
| 3ds Max 2009 导出 | 外链贴图 + 英制单位 | `unit = 0.0254`、`up = +Z`（证实单位/朝向必须显式换算），外链 PNG 能按记录路径找到 |
| Maya 导出 | 动画栈 | `Take 001`，时长 2 s，10 个关键帧，曲线可采样 |

同时确认了 ufbx 侧的关键 API 形态：`Mesh` 提供按索引访问的 `vertex_position / vertex_normal / vertex_uv` 与面材质分组、
`Material` 有 `fbx`（Phong 系）与 `pbr` 两组映射、
`Texture` 同时提供 `filename`（外链）与 `content`（嵌入 blob）、
`AnimStack → layers → anim_values → curves` 是标准的四级结构。
这决定了 §4 的数据结构可以按「直接映射」而不是「强行归一」来设计。

---

## 10. 验收总纲

单次 `cargo run` 满足以下全部条件，视为本阶段完成：

1. 场景 JSON 声明一个 FBX，画面出现该模型，朝向、比例、位置正确；
2. 贴图正确显示（嵌入与外链各一），无 UV 错位与上下颠倒；
3. 播放指定动画名，模型连续动起来，`speed` / `looping` 生效；
4. 同一资产被 N 个物件引用时，解析与 GPU 上传各只发生一次；
5. 改 `.fbx` 文件或改 `scene.json` 后 ≤0.4s 自动生效，无需重启；
6. 没有 FBX 的默认场景行为与现在逐像素一致（回归不破）；
7. `cargo build --release` 零 warning，`cargo test` 全绿。
*（内容由AI生成，仅供参考）*

---

## 11. P1（M1 静态网格）实施现状

### 11.1 落地的模块与职责

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/mesh.rs` | `Vertex`（pos/normal/uv，stride 32）+ `MeshData`（纯 CPU，无 wgpu/ufbx 依赖）+ `MeshProvider` trait |
| `crates/r3e-core/src/node.rs` | `BuiltinMesh` / `AssetId` / `MeshRef`；`Node.mesh` 由 `MeshKind` 改为 `MeshRef` |
| `crates/r3e-asset/src/lib.rs` | `ufbx` 解析、`AssetLibrary`、`AssetEntry`、`MeshAsset`、`MaterialPart`、`TextureInfo`/`MaterialInfo`、`dump_fbx` |
| `crates/r3e-render/src/mesh.rs` | `MeshCache`：键 `MeshRef`，按 `MeshProvider` 取 `MeshData` 惰性上传 GPU |
| `crates/r3e-render/src/renderer.rs` | 顶点布局 32 字节、`Arc<dyn MeshProvider>` 注入点（不再依赖 `r3e-asset`） |
| `crates/r3e-app/src/main.rs` | `--asset` / `--dump-asset`、`spawn_asset_nodes`、启动时注入资产库 |

**依赖边界（已核实）**：`cargo tree -p r3e-render` 中不存在 `r3e-asset` 与 `ufbx`；
`r3e-core` 依旧不依赖 wgpu / ufbx；`r3e-asset` 不依赖 render。

### 11.2 三个关键实现口径（踩过的坑，后续不得回退）

1. **顶点必须按「角点」拆分**：`ufbx` 的 position / normal / uv 是三套独立索引空间
   （`mesh.vertex_indices[c]`、`vertex_normal.indices[c]`、`vertex_uv.indices[c]`）。
   以 `(pos_ix, nrm_ix, uv_ix)` 三元组哈希去重生成顶点，输出顶点数才会大于 ufbx 顶点数
   （Npc1：ufbx 1271 → 输出 4708），这是 UV 接缝与硬边被正确拆开的标志。
   **反例**：直接拿 ufbx 顶点号当索引，会得到越界索引（如 4707 > 1271），几何必然错乱。
2. **索引必须二次映射**：`ufbx::triangulate_face_vec` 输出的是角点号，需经 `corner_to_vertex`
   转成输出顶点号后再进索引缓冲。
3. **AABB 与顶点位置统一走 `geometry_to_world`**：单位缩放与轴向转换由根节点矩阵承担，
   与 M0 探针结论一致；`MeshData::aabb()`（局部顶点统计）与 `MeshAsset::aabb_*`（世界空间）不可混用。

### 11.3 验收记录

`cargo build --workspace` exit=0，零 warning（MinGW，x86_64-pc-windows-gnu）。

| 样本 | ufbx 顶点/三角 | 输出顶点/三角 | AABB（世界） | UV |
| --- | --- | --- | --- | --- |
| Npc1/test.FBX | 1271 / 2140 | 4708 / 2140 | `(-0.683,-0.000,-0.472)~(0.683,1.564,0.330)` | 有效，u 0.003~0.997 |
| tirger.fbx | 1018 / 1028 | 3084 / 1028 | `(-0.514,-0.016,-2.089)~(0.493,1.846,1.621)` | 有效 |
| Weapon/Test/test.FBX | 1179 / 1084 | 3252 / 1084 | `(-0.228,-0.768,-0.653)~(0.228,3.811,0.653)` | 有效，u 0.004~0.983 |

AABB 与独立校验工具（`D:\_probe\aabb_check`，直接调 ufbx 复算）三例完全一致；
窗口化联调 3 个资产 spawn 成功、网格上传 GPU，人形造型完整无炸面。

### 11.4 尚未做（留给 P2 及之后）

- 贴图只收集元数据（filename / 嵌入判定 / uv_set），未解码、未上传纹理；
- 材质只有 `base_color` 参与渲染着色，`metallic` / `roughness` 未进 uniform；
- 场景 JSON 的 `assets` 段与 `--assets-root` 未接（当前只走 CLI `--asset`）；
- 物理体推导仍只认内置网格，`MeshRef::Asset` 尚无 AABB 体；
- 顶点未去重到「位号级」共享（角点级拆分是有意为之，压缩留到后续 LOD 阶段）。
