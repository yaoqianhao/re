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

> 状态：**P0–P4 已落地**（静态网格、材质贴图、场景 `assets` 段、节点动画、骨骼蒙皮）
> 本文档前半部分是阶段规划，第 11 – 14 章是已实现口径与验收记录。
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
| ~~骨骼蒙皮（skin / joint weights）~~ | 原计划排除，实际作为 P4 单独立项落地（见第 14 章）；主因是它需要独立的顶点属性、关节矩阵通道与绑定姿态口径 |
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

### 5.1 场景 JSON（P2 已按此口径实现）

```json
{
  "name": "fbx_demo",
  "assets": [
    { "path": "D:/models/robot.fbx", "position": [0, 0, 0], "scale": [1, 1, 1] },
    { "path": "D:/models/robot.fbx", "position": [3, 0, 0] },
    { "path": "D:/models/weapon.fbx", "spawn": false }
  ],
  "objects": [
    { "name": "sword", "mesh": "asset:D:/models/weapon.fbx#0", "position": [1, 1, 0] }
  ]
}
```

- `assets[].path`：绝对路径或相对**场景文件所在目录**的路径；导入后按网格逐个 spawn 成物件。
- `assets[].position / rotation / scale`（可选）：该资产所有网格的整体变换；不写就是单位变换。
- `assets[].spawn`（可选，默认 `true`）：写 `false` 表示只导入不 spawn，供 `asset:` 引用或脚本使用。
- `mesh: "asset:<路径>#<序号>"`：序号是资产内网格下标（`--dump-asset` 可查）。
  路径大小写、`/` 与 `\` 不敏感（`AssetId::from_path` 内部归一化）。
- 同一路径被多条声明或命令行同时引用 = 只解析一次、只上传一次 GPU 缓冲（命令行的声明排在最前）。

### 5.1.1 光照阴影段（阶段三 D）

```json
"light": {
  "direction": [-0.55, -0.62, -0.35],
  "color": [1.0, 0.95, 0.88],
  "intensity": 1.25,
  "ambient": [0.12, 0.14, 0.18],
  "shadow": {
    "enabled": true,
    "size": 2048,
    "ortho_size": 0.0,
    "bias": 0.0015,
    "normal_bias": 1.5,
    "distance": 2.0,
    "strength": 1.0
  }
}
```

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 关闭后整帧跳过阴影 pass，零开销 |
| `size` | `2048` | 阴影贴图边长，夹到 64..8192；改值会重建纹理 |
| `ortho_size` | `0` | 正交相机半宽；0 = 按场景包围球自适应 |
| `bias` | `0.0015` | 深度比较偏移，抑制自阴影条纹 |
| `normal_bias` | `1.5` | 沿法线的世界空间偏移（按纹素换算），斜射面抗 acne |
| `distance` | `2.0` | 光源后退距离 = 包围球半径 × 该值 + 1 |
| `strength` | `1.0` | 阴影强度 0..1 |

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
| `--assets-root <dir>` | 场景 `assets` 段里相对路径的解析根。不指定时按场景文件目录；指定后资产可集中放在一个公共目录，场景文件可随意挪位置。**已落地（阶段三 A）** |

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

### P2：材质与贴图 —— ✅ 已完成（2026-10-24）

- 材质表 + 每材质 bind group + 纹理上传（`Rgba8UnormSrgb`，UV 采样）；
- `MaterialAsset` 字段映射：`pbr.base_color` → `base_color`；`pbr.metallic / roughness` → 高光指数与强度的近似；
- 采样器：`Linear` + `Repeat`，无 mipmap（后续可加）；
- 验收：带贴图样本（嵌入 + 外链各一）颜色正确、无 UV 错位、无上下颠倒（FBX 的 V 轴与 wgpu 的 UV 原点不同，需显式翻转）。

### P3：节点级动画 —— ✅ 已完成（2026-10-24）

- `AnimClip / AnimTrack / AnimInstance` + 每帧采样（线性插值 + 时间轴重基），骨架节点树按路径建；
- 场景 JSON `anim` 段 + 脚本 `play_anim` / `set_anim_clip` 系列 + `--dump-anim` / `--show-bones`；
- 验收：老虎样本 7 个片段可轮播，`speed` / `looping` / 切片段生效，细节见第 13 章。

### P4：骨骼蒙皮 —— ✅ 已完成（2026-10-24）

- `SkinData`（关节表）/ `SkinInstance`（场景侧绑定 + 每帧关节矩阵）落在 `r3e-core::skin`；
  `Vertex` 加 `joints(u16x4) / weights(f32x4)`，`stride 32 → 56`；
- 资产层从 ufbx `skin_deformers → clusters` 提取关节与每顶点权重，`MeshAsset.skin` 填槽位；
- 渲染层 `group(2)` 关节矩阵 storage buffer（4096 槽位、256 KiB）+ 顶点着色器 LBS，
  实例数据加 `joint_base`；
- 验收：老虎样本 37 关节 / 3084 顶点带权重，4 s 内蒙皮质心位移 0.164、四肢逐顶点形变，
  整体平移实例不产生形变。细节见第 14 章。

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

### 11.4 尚未做（P3 及之后）

- 场景 JSON 的 `assets` 段已接通（见第 12 章），`--assets-root` 已落地（阶段三 A）；
- 物理体推导已用上真实 AABB（阶段三 A），脚本 `attach_body` 对导入模型立即可用；仍不做凸包/网格碰撞；
- 顶点未去重到「位号级」共享（角点级拆分是有意为之，压缩留到后续 LOD 阶段）；
- 法线贴图 / AO / 金属粗糙度贴图未进通道，目前只有 `base_color` 贴图参与着色。

## 12. P2（材质贴图 + 场景 assets 段）实施现状

### 12.1 落地的内容

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/mesh.rs` | `MeshPartData`（索引区间 + 材质下标）、`MaterialData`（base_color / metallic / roughness / base_color_texture）、`TextureImage`；`MeshProvider` 增加 `mesh_parts` / `material` / `texture` |
| `crates/r3e-asset/src/lib.rs` | 贴图解码（`image` + tga/bmp 特性，按扩展名兜底）、材质因子提取、`mesh_parts` 分区输出 |
| `crates/r3e-render/src/renderer.rs` | `MaterialCache`（材质 uniform + 贴图 view + 采样器）、bind group 1、按 `GpuPart` 逐段 `draw_indexed` |
| `crates/r3e-render/src/shader.wgsl` | `group(1)` 材质 uniform 与贴图采样，albedo = 物件色 × 材质因子 × 贴图 |
| `crates/r3e-app/src/app.rs` | 场景 `assets` 段导入 + 按声明 spawn（position/rotation/scale/`spawn:false`）、路径去重 |
| `crates/r3e-core/src/scene.rs` | `from_json_str` 剥 UTF-8 BOM（PowerShell/记事本保存的场景不再解析失败） |

### 12.2 关键口径（后续不得回退）

1. **贴图三级回退**：`filename` 能读 → 用外部文件；否则用 `content` 嵌入字节；再失败 → 白纹理。
   缺贴图的材质仍然可渲染，只是纯色。
2. **解码要带扩展名提示**：FBX 内嵌图片的字节流没有名字，`image` 无法猜格式（老虎样本的
   嵌入 TGA 就解码失败过）。`decode_image_bytes` 必须带上贴图名做扩展名兜底。
3. **FBX 缺省因子归一**：FBX 默认 `diffuse = 0.588`，带贴图时它是缺省值而非美术意图，
   必须归一到 1.0，否则贴图被额外压暗 41%。
4. **颜色不再走顶点色**：资产节点颜色固定白色，材质因子只在 bind group 里生效一次；
   两处都乘会二次压暗（M1 的临时做法已在 P2 移除）。
5. **分段绘制**：`mesh_parts` 为空时渲染层补「整网格一段」的默认材质，越界分区裁切，避免 draw 越界。

### 12.3 验收记录

`cargo build --offline --workspace` exit=0，零 warning。

| 验证项 | 结果 |
| --- | --- |
| 老虎样本（内嵌 TGA） | 贴图上传 GPU 256×256，窗口中可见条纹毛色 |
| Npc1 样本（外部 PNG） | 贴图上传 GPU 512×512 |
| 场景 `assets` 段（两条声明同一 FBX，一条 `spawn:false`） | 只导入一次、只 spawn 1 个节点，日志与画面一致 |
| 场景 JSON 带 BOM | 正常解析（此前会报 `expected value at line 1 column 1`） |


## 13. P3（节点级动画 + 骨架节点树 + 片段切换）实施现状

P3 已落地：FBX 动画栈 → `AnimClip`，按骨架节点路径建节点树，逐帧采样写 `Transform`，
脚本与场景 JSON 都能声明/切换片段。P3 阶段蒙皮尚未接入，只动「节点变换」这一层；
**P4 已在同一套骨架节点树上补上蒙皮**，见第 14 章。

### 13.1 落地的内容

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/anim.rs` | `AnimClip`（名/时长/轨道）、`AnimTrack`（节点路径 + T/R/S 关键帧）、`AnimKey`、`AnimInstance`（时间轴 + 速度 + 循环 + `AnimBinding`） |
| `crates/r3e-core/src/scene.rs` | `update_animations(dt)`、`play_anim/stop_anim/set_anim_speed/set_anim_time/anim_time/anim_names/add_anim_instance/prune_anim_bindings`；片段注册表 `register_anim_clip/anim_clip_names/set_anim_clip` |
| `crates/r3e-asset/src/lib.rs` | 从 `scene.anim_stacks` 提取全部栈（`time_begin` 重基到 0）、从 `scene.nodes` 提取节点树（local TRS + parent 下标 + 路径） |
| `crates/r3e-app/src/app.rs` | `spawn_asset_anim`：按节点路径建骨架节点树（`<资产名>/<节点路径>`）、按 `node_index` 绑轨道、建播放器；`--show-bones` 挂标记方块 |
| `crates/r3e-script/src/api.rs` | `anims/anim_of/anim_info/anim_clip/anim_clips/play_anim/stop_anim/set_anim_clip/set_anim_speed/set_anim_time/anim_time/set_anim_looping/anim_playing` |
| `crates/r3e-app/src/main.rs` | `--dump-anim <fbx>`、`--anim-clip <名>`、`--show-bones` |
| `scripts/anim_demo.rhai`、`assets/scene_anim.json` | 演示：每 3 秒轮播该资产 7 个片段，打印实例与片段信息 |

### 13.2 关键口径（后续不得回退）

1. **时间轴重基**：ufbx 的 `AnimStack.time_begin` 不一定是 0（Maya 样本是 0.234），
   提取时统一 `t -= time_begin`，否则起播瞬间会跳帧；`AnimClip.duration = time_end - time_begin`。
2. **绑定靠 `node_index` 而非名字**：轨道与骨骼同名的情况并不罕见（Blender 常见 `Bone.001`），
   `AnimBinding` 同时记 `node_index` 与 `path`，切片段时按 `node_index` 优先、`path` 兜底重绑。
3. **切片段 = 原地重绑**：`set_anim_clip` 复用同一个 `AnimInstance`（不重建节点树、不重置节点位姿），
   时间归零后按新片段采样；同一骨架不同动作严禁重复 `spawn`。
4. **空片段降级**：`AnimClip` 无轨道（如只含单帧的 `sleep_loop`）时不建实例，只 `warn`，不阻断资产导入。
5. **非循环到点即停**：`looping: false` 在 `duration` 处停止并保留末帧姿态，不回 0。
6. **场景重建后必须补挂**：`ScriptHost` 增加 `rebuild_hook`，场景热重载 / F5 重放 JSON 后由 app
   把资产节点与动画实例重新 spawn 回去，且要早于脚本 `on_start`（否则 `anims()` 为空）。
7. **脚本先于资产是错序**：启动顺序固定为「建场景 → spawn 资产与动画 → 载入脚本」，`on_start` 才能枚举到实例。
8. **标记方块是调试件**：`--show-bones` 的方块尺寸按资产缩放反算（`0.25 / scale`），
   贴在骨骼节点本地原点，常被网格包住看不清——确认动画请用 `--anim-clip` 逐个对照或看日志位移。

### 13.3 验收记录

`cargo build -p r3e-app` exit=0；`cargo test -p r3e-core` 5 passed（动画采样、非循环停止、
速度缩放、切片段重绑，以及旧的场景用例）。

| 验证项 | 结果 |
| --- | --- |
| 老虎样本 `--dump-anim` | 7 个栈：`atk / atk2 / die / free / idle_loop / run_loop / sleep_loop`，时长 0.03–2.93s，位移最大轨道幅度合理 |
| 场景声明 `anim: { clip: "idle_loop" }` | 日志 `动画实例 \`tirger/idle_loop\`：40 条轨道 / 2.10s / 播放中` |
| 骨骼位移探针 | 26s 内 `tirger/.../Bone03` 世界位移 0.131，证明采样真的写进了节点 TRS |
| 脚本轮播 7 个片段 | 每 3 秒一条 `切到 tirger/<片段>`，切换后轨道数仍 40、时长为该片段自身时长、时间归零 |
| 脚本热重载（改 `.rhai` 触发） | 日志 `资产节点已随场景重建补挂: 1 个` + `动画实例数: 1`，模型与动画未丢失 |
| 窗口渲染 | 老虎模型正常显示（贴图/材质沿用 P2），骨架标记方块被网格包住、肉眼难辨，见 13.2-8 |

### 13.4 尚未做（P3 范围外）

- ~~**蒙皮（P4）**~~：已在 P4 落地（见第 14 章），四肢逐顶点形变已生效。
- **动画混合/分层**：阶段三 C 已补 cross-fade（见第 16 章），仍无 upper-body mask 与 additive 叠加。
- ~~**场景 JSON 改 `assets` 段的热生效**~~：已在阶段三 A 落地——热重载时比较 `assets` 声明，
  变了才重新导入并换库，没变则连资产都不重导；但**运行时脚本已改过的 `anim` 参数**仍会被
  `set_anim_clip` 的运行时状态覆盖，改片段请重启确认。
- ~~**`--assets-root`**~~：已落地（阶段三 A）。
- ~~**物理 AABB**~~：已落地（阶段三 A），`ColliderDesc::infer_with / from_node_with` 用真实 AABB 推导半尺寸。

## 14. P4（骨骼蒙皮）实施现状

P4 已落地：FBX 的 `skin_deformers` 变成引擎侧的关节绑定，动画推进节点 TRS 后由
`update_skinning` 折算成关节矩阵，顶点着色器做线性混合蒙皮（LBS）。老虎样本的四肢现在真正形变。

### 14.1 落地的内容

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/mesh.rs` | `Vertex` 加 `joints: [u16; 4]` / `weights: [f32; 4]`（stride 56）、`Vertex::is_skinned()`、`with_skin()` |
| `crates/r3e-core/src/skin.rs` | `SkinData`（关节表）、`SkinJoint`、`SkinInstance`（`SkinBinding` 静止姿态 + 逆绑定矩阵、`joint_matrices`）、`solve()` / `update_skinning()`、`MAX_JOINT_MATRICES = 4096` |
| `crates/r3e-core/src/scene.rs` | `skins` 表与 `add_skin / skin_of / update_skinning / prune_skins`；`update_world_transforms` 末尾自动求解 |
| `crates/r3e-asset/src/lib.rs` | `skin_deformers → clusters` 提取关节（`node_index` / `path` / `bind_to_world`）与每顶点 `joints/weights`（归一化、超 4 权重取大者），`MeshAsset.skin: Option<u32>`、`AssetEntry.skins`、`skin_stats()` |
| `crates/r3e-render/src/mesh.rs` | 顶点属性 5 个（`0..=4`，`3 = Uint16x4`、`4 = Float32x4`）；`InstanceRaw` 加 `joint_base`（location 10），stride 96 |
| `crates/r3e-render/src/shader.wgsl` | `@group(2) @binding(0) u_joints: array<mat4x4>`；`skin_matrix()` 做 LBS，权重全 0 时跳过取样（内置网格零成本） |
| `crates/r3e-render/src/renderer.rs` | 关节 storage buffer + bind group + pipeline layout `group(2)`；`prepare_instances` 按 `scene.skin_of(mesh_node)` 填 `joint_base` 并上传本帧关节矩阵（溢出报错不静默截断） |
| `crates/r3e-app/src/app.rs` | `bind_asset_skins`（静止姿态绑关节，按 `node_index` 对齐）、`log_skin_probe`（CPU 复算蒙皮质心 / 最大偏差，做无头验收） |
| `crates/r3e-asset/examples/skin_probe.rs` | 口径探针：同一帧分别用 `bind_to_world⁻¹` 与静止姿态逆矩阵做 CPU 蒙皮，对比顶点位移，作为「逆绑定取静止姿态」的证据 |

### 14.2 关键口径（后续不得回退）

1. **顶点空间约定**：资产层把顶点烘焙到「资产世界空间」（实例变换之前）。绑定姿态存的是
   **网格局部空间**的关节矩阵 `L_rest = X_rest⁻¹ · B_rest`，每帧
   `M_i = X⁻¹ · B_now_i · L_rest_i⁻¹`，着色器 `world = X · Σ w_i M_i p`。
   直接用 `X⁻¹ · B_now · B_rest⁻¹` 会让实例平移在静止姿态被抵消（网格钉在原点），这条是踩过的坑。
2. **逆绑定矩阵取「静止姿态」而非 `cluster.bind_to_world`**：`skin_probe` 实测该老虎资产
   两者不一致 —— 用 `bind_to_world⁻¹` 时动画停在第 0 帧仍有 1018/1018 顶点偏移（max 0.6715），
   用导入位姿逆矩阵时同帧偏移为 0。`bind_to_world` 仅作元数据保留。
3. **静止姿态必须等于 P3 画面**：`max_deviation()` 在未推进动画时应≈0，否则说明逆绑定矩阵口径错。
4. **绑定时机**：必须在场景节点建好、**动画推进之前**绑定（`spawn_asset_anim` 之后、首帧
   `update_animations` 之前），否则逆绑定矩阵吃到动画位姿，模型会整体扭曲。
5. **权重归一化在资产层做掉**：ufbx 的 cluster weight 不保证和为 1；同时每顶点只保留最大的 4 个
   权重并重新归一，多余权重丢弃时 `warn` 一次。
6. **关节矩阵槽位上限**：单帧关节矩阵总槽位 `MAX_JOINT_MATRICES = 4096`（256 KiB storage）；
   多蒙皮实例按 `joint_base` 顺排，超出直接报错，不允许静默截断成错形变。
7. **非蒙皮网格零成本**：内置网格与无 skin 的资产顶点权重全 0，着色器提前 return，
   不读 storage buffer，也不占槽位。
8. **CPU 探针是唯一无头验收手段**：窗口里肉眼难判断四肢是否真的形变，`log_skin_probe`
   用同一套关节矩阵在 CPU 上重算蒙皮质心与最大偏差，配合骨骼位移量交叉验证。

### 14.3 验收记录

`cargo test --workspace` 全绿（core 9 / asset 1 / render 3 / app 10）；
`cargo build --release` exit=0，零 warning。

| 验证项 | 结果 |
| --- | --- |
| 老虎样本导入 | `蒙皮 tirger/skin#0：37 个关节 / 3084 顶点带权重，绑定到网格节点 diaojingbaihu` |
| 4 s 动画运行 | 骨骼最大位移 0.3175（`Bone04`）；蒙皮质心 `(-0.0046,0.6364,0.1883) → (-0.0164,0.7700,0.2178)`，位移 0.1374（量值随播放相位浮动，多次运行 0.14–0.16） |
| 静止姿态偏差 | 绑定时不推进动画，`max_deviation()` 的 ≈0 断言放在单元测试（静止 / 整体平移 / 整体缩放三种情形均 <1e-5）；运行期探针打印的是「相对基准帧」的偏差（首帧 1.37），不是静止偏差 |
| 实例平移 [2.5,0,0] | 质心 x 从 -0.004 变为 2.4962，位移量误差 <0.01，且仍在形变（位移 0.2380）→ 平移不被蒙皮吞掉 |
| 权重自洽性单测 | 每顶点权重和 = 1（±1e-3）、关节下标 < 关节数、至少一个顶点带权重 |
| 顶点/实例布局单测 | 顶点 stride 56、`3/4` 号为 `Uint16x4 / Float32x4`；实例 stride 96、`joint_base` 在 location 10 |
| 内置网格回归 | `cube/sphere/plane/cylinder` 权重全 0，走非蒙皮分支，画面与 P3 逐像素一致 |
| 口径探针 `skin_probe` | 老虎样本每顶点最多 2 个权重（1=482 / 2=536）；`bind_to_world` 口径在第 0 帧就整体偏移（max 0.6715），静止姿态口径同帧 0.0000 |

### 14.4 尚未做（P4 范围外）

- **多套蒙皮 / 每网格多 skin**：目前一网格一蒙皮实例，`MeshAsset.skin` 只存一个槽位。
- **双四元数蒙皮（DQS）**：现在是 LBS，关节大角度扭转时会有糖纸效应。
- **GPU 侧蒙皮（compute skinning）**：关节矩阵在 CPU 求解后上传，顶点量大时可改 compute。
- **动画混合 / 状态机**：cross-fade 与脚本状态机已在阶段三 C 落地（见第 16 章）；分层 mask、additive、相位对齐仍未做。
- **表情 / morph target**：未支持。

## 15. 阶段三 A（AABB + 视锥剔除 + 两个小尾巴）实施现状

A 阶段的目标是「让外部资产也拥有包围盒」，它同时是阴影、剔除、物理三件事的公共前置。

### 15.1 落地的内容

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/aabb.rs` | 新增 `Aabb`（`min/max`、`EMPTY`、`new/from_points/from_center_half/union/expand/transformed/inflate_relative/half_extents/size/center/is_empty`） |
| `crates/r3e-core/src/frustum.rs` | 新增 `Frustum`（`from_view_projection`、`planes`、`contains_point`、`intersects_aabb`，正负顶点测试） |
| `crates/r3e-core/src/mesh.rs` | `MeshData::aabb()` / `aabb_box()`；`MeshProvider` 新增 `mesh_aabb(key) -> Option<Aabb>`（默认 `None`） |
| `crates/r3e-render/src/renderer.rs` | `MeshCache.bounds`（`HashMap<MeshRef, Option<Aabb>>`）+ `bounds()`；`prepare_instances` 里先过视锥再分配实例/关节槽位；`SKIN_CULL_MARGIN = 0.25`；`RenderStats` 增 `culled / cull_missing_bounds` |
| `crates/r3e-physics/src/lib.rs` | `ColliderDesc::infer_with / from_node_with(node, bounds)`，`half_extents_from` 用真实 AABB × scale 推导 |
| `crates/r3e-app/src/app.rs` | 资产库改为 `Rc<RefCell<Arc<AssetLibrary>>>`（可就地换库）；`refresh_assets_from_json` 比较 `assets` 声明后按需重导；`collect_asset_specs(scene, scene_path, cli)` 支持 `--assets-root`；启动/收尾打印剔除统计 |
| `crates/r3e-app/src/main.rs` | `--assets-root DIR` 解析与 USAGE |
| `crates/r3e-asset/src/lib.rs` | `mesh_aabb` 直接返回导入时算好的 AABB（零拷贝） |

### 15.2 关键口径（后续不得回退）

1. **fail-open 优先于剔除**：`mesh_aabb` 返回 `None` 时一律照画，并把计数堆到 `cull_missing_bounds`。
   剔除是优化，不能让物件因为"不知道多大"而消失。
2. **AABB 变换走角点**：`Aabb::transformed` 变换 8 个角点后重取包围盒。用「中心点 + 半尺寸 × scale」
   在带旋转的物件上会低估，表现为贴边时物件突然消失。
3. **蒙皮必须留余量**：AABB 是静止姿态的，形变会越界；`SKIN_CULL_MARGIN` 是相对外扩（按尺寸比例），
   不用固定值，避免大小模型余量失衡。
4. **局部 vs 世界不可混用**：`MeshProvider::mesh_aabb` 给的是**局部**空间；`MeshAsset.aabb_*`（资产层）
   给的是**资产世界空间**（导入时烘焙）。混用会让物件被放大的 `X` 二次缩放。
5. **热重载比较的是声明而非文件**：`asset_spec_eq` 逐字段比较路径、transform、`spawn`、`anim`
   参数。只改物件时不动资产库，避免重复解析几百 MB FBX、也避免打断播放进度与物理体。
6. **换库后必须重新补挂**：资产库换了，`MeshRef::Asset` 句柄全部失效，`rebuild_hook` 里按新声明重建
   资产节点与动画实例，并调 `renderer.set_asset_library`。

### 15.3 验收记录

`cargo build --release` exit=0；`cargo test --release` 全绿（36 个用例 / 0 失败）。

| 验证项 | 结果 |
| --- | --- |
| 视锥单测 | 正前可见、背后剔除、远平面外剔除、侧向剔除、跨界保留、空盒永不可见（共 6 例） |
| AABB 单测 | `union` / `transform` / `inflate` / `from_points` / 空盒语义（共 5 例） |
| 物理单测 | `infer_with_uses_mesh_aabb_instead_of_unit_cube`（AABB 参与推导）、`infer_without_bounds_falls_back_to_scale_half`（缺 AABB 退 `scale×0.5`） |
| `--assets-root` | 场景写 `models/tirger.fbx` 相对路径：不传 root 报 FileNotFound；传 `--assets-root` 后导入 1 资产 / spawn 1 节点 / GPU 上传成功，首帧 `可见 3 / 绘制 2 / 剔除 1 / 缺包围盒 0` |
| 场景热重载（assets 未变） | `资产节点已随场景重建补挂: 1 个`，**无**「assets 段已变更」日志 → 未重导资产 |
| 场景热重载（assets 已变） | `场景 assets 段已变更，重新导入资产库: 1 个资产` → 补挂 2 个节点，收尾 `可见 4 / 绘制 3 / 剔除 1 / 缺包围盒 0` |
| 缺包围盒计数 | 全程 `0`，说明资产层 AABB 已喂到渲染器 |
| 物理碰撞体尺寸（资产） | 场景用 `"mesh": "asset:<fbx>#0"` 引用资产并声明 `physics.body=dynamic`，从 y=5 落到地面后静止于 `y=0.9309`——即该网格 AABB 的半高；若仍按旧口径 `scale × 0.5` 会停在 `y≈0.5`。同时日志 `物理已绑定 1 个场景刚体`，验证 `BodyDesc::from_node_with` 走到了真实 AABB 分支 |
| 剔除随物件移动收敛 | 同一探针首帧资产位于 y=5（相机构图上方）时 `剔除 1`，落地后 `剔除 0`，说明剔除每帧按当前 `world` 矩阵重算，不是启动时的一次性判定 |

### 15.4 尚未做（A 阶段范围外）

- **遮挡剔除 / 层级剔除**：逐实例测试，深层次结构下父节点包围盒无法代表整棵子树。
- **剔除与阴影共用**：阴影 pass 尚未存在，届时需要单独一套以光源为视点的视锥（或复用阴影相机）。
- **碰撞体形状**：仍是盒/球/胶囊的近似，不做凸包与网格碰撞。

## 16. 阶段三 C（动画交叉淡入 + 脚本状态机）实施现状

C 阶段解决的是「切片段会跳」这个观感问题：`set_anim_clip` 原地重绑后时间归零，跨片段位姿差异直接体现为一帧突变。
现在多了一条 `set_anim_clip_fade` 路径，在 `duration` 秒内把旧片段的姿态平滑让位给新片段。

### 16.1 落地的内容

| 位置 | 内容 |
| --- | --- |
| `crates/r3e-core/src/anim.rs` | `NO_TRACK = u32::MAX`；`wrap_time(t, duration, looping)`（与时间轴取模同源）；`blend_transform(a, b, w)`（位置/缩放 lerp、旋转 slerp）；`AnimFade{clip, time, speed, looping, tracks, elapsed, duration}` + `weight()`（smoothstep）；`AnimInstance` 加 `fade: Option<AnimFade>`、`cross_fade(clip, duration)`、`fade_weight()`、`fade_source()`，`advance()` 里同步推进旧片段与 `elapsed`，到点自动 `fade = None` |
| `crates/r3e-core/src/scene.rs` | `update_animations` 改为「按绑定取新旧两路采样 + 加权混合」；`find_anim_clip` 抽出复用；新增 `set_anim_clip_fade / anim_fade_weight / anim_fade_source`；`set_anim_clip` 保持硬切语义 |
| `crates/r3e-script/src/api.rs` | 新增 `set_anim_clip_fade(实例, 片段, 秒)` / `anim_fade(实例)` / `anim_fade_from(实例)` |
| `crates/r3e-app/src/app.rs` | `log_anim_probe` 输出 `fade=旧片段→新片段 xx%` 或 `fade=无` |
| `scripts/anim_state.rhai` | 三态演示（idle / run / atk）：按住左键跑、点击左键攻击、攻击播完回落待机，全部带 0.15–0.25s 淡入 |
| `scripts/anim_demo.rhai` | 轮播改用 `set_anim_clip_fade(…, 0.35)`，过渡期间打印权重曲线 |

### 16.2 关键口径（后续不得回退）

1. **过渡期间只有一套骨架**：旧片段只提供采样值，不额外驱动一份节点树；关节矩阵/蒙皮那条链路一行未改，
   因此不会出现"双重蒙皮"或缺一张关节矩阵。
2. **权重曲线必须两端慢**：用 `smoothstep` 而非线性，是让切换起点/终点的速度连续；线性权重在
   切片段那一帧和结束那一帧仍有可见的速度突变，等于把"跳位姿"换成"跳速度"。
3. **旋转必须 slerp**：`Quat::lerp` 在夹角接近 180° 时长度塌缩、归一化后方向不可控；混合永远走 `slerp`。
4. **旧片段时间独立推进**：`AnimFade` 冻结 `time/speed/looping`，过渡期间不跟随 `set_anim_speed` 等外部改动，
   否则旧片段会在半途变速，混合结果是两个都不像。
5. **缺轨道靠 `NO_TRACK` 而不是补零**：新片段没有的轨道要由旧片段继续喂，写 `u32::MAX` 标记；
   若用 0 号轨道顶替，角色会长出别人的肢体动作。
6. **零时长等价硬切**：`duration <= 0` 直接走 `set_clip`；这条保证了既有场景 JSON 与旧脚本行为不变。
7. **状态机不进引擎**：状态判定、优先级（先按的时间片）、攻击播完回落全部在脚本里按帧重算。
   引擎只提供"带过渡的切片段"这一个原子操作，脚本热重载后状态自然从当前输入重建，不留脏状态。
8. **打不断的长动作要脚本自己管**：引擎不阻止过渡中再次 `cross_fade`（会按当前混合状态重建过渡），
   是否需要"攻击期间忽略移动输入"属于逻辑，由脚本判定。

### 16.3 验收记录

`cargo build --workspace` exit=0；`cargo test --workspace` 39 passed / 0 failed（其中 `r3e-core` 23 例，含新增 3 例）。

| 验证项 | 结果 |
| --- | --- |
| `cross_fade_blends_two_clips_then_settles` | 过渡中节点位置落在两片段采样值之间；`fade` 到点归 `None`，之后姿态只由新片段决定 |
| `zero_duration_is_hard_cut` | `duration=0` 时 `fade=None`，第一帧即为新片段姿态 |
| `keeps_old_track_when_new_clip_lacks_it` | 新片段缺该轨道时节点仍按旧片段推进（实测 3.25 而非绑定姿态），过渡结束后停在旧片段末姿态 |
| 权重曲线（`anim_demo.rhai` + `--run-seconds 8`） | 日志逐帧打印 `过渡中 idle_loop → atk：2%…99%`，8 秒内完成 3 次切换，收尾探针 `fade=无` |
| 三态状态机（`anim_state.rhai` + `--run-seconds 4`） | 脚本载入无报错，无输入时稳定停留 `idle_loop`（`t=1.903/2.100s`），未误触发 run/atk |
| 蒙皮未回归 | 收尾探针 `蒙皮质心位移 0.4565`、`最大偏差 1.5304`、`可见 44 / 绘制 44 / 剔除 0 / 缺包围盒 0`，与 A 阶段一致 |

### 16.4 尚未做（C 阶段范围外）

- **分层 / mask 混合**：无 upper-body 遮罩，无法「下半身跑 + 上半身攻击」。
- **additive 叠加**：无叠加层，呼吸、受击抖动这类增量动画只能当完整片段播。
- **同步组与相位对齐**：跑→跑（变速）这类同族片段按各自时间轴采样，不做步频对齐，脚步可能错拍。
- **状态机 DSL**：状态转移仍写在 Rhai 里，没有引擎侧的 transition 表或条件图。

---

## 17. 阶段三 D（方向光阴影贴图）实施现状

### 17.1 落地的内容

| 层 | 变更 |
| --- | --- |
| `r3e-core` | `ShadowSettings`（`enabled` / `size` / `ortho_size` / `bias` / `normal_bias` / `distance` / `strength`）挂在 `Light::shadow`；`sanitized()` 统一夹值；`Light::shadow_view_projection(center, radius)` 生成正交光源相机（右乘 + DirectX 深度 0..1，与主相机同套约定） |
| 场景 JSON | `light` 段新增可选 `shadow` 对象，字段与结构体同名；不写则用默认值（**默认开启**，2048²，正交范围按场景包围球自适应） |
| `r3e-render` | 新增 `Depth32Float` 阴影纹理 + `sampler_comparison`；`ShadowUniform`（矩阵 + 纹素 + bias + 启用位 + 法线偏移 + 强度）；阴影 pass 专用 `ShadowPassUniform` 与独立 pipeline layout；`prepare_shadow` 拟合相机并写 uniform；`render` 先跑深度 pass 再跑主 pass；`RenderStats.shadow_draws` |
| 着色器 | `vs_shadow`（含 LBS 蒙皮，形变后的网格投出跟随姿势的影子）、`shadow_factor`（3×3 PCF + 法线偏移 + 越界按受光 + 强度 `mix`）；主片段里只让直接光被遮挡，环境光不衰减 |
| `r3e-script` | `set_shadow(bool)` / `shadow_enabled() -> bool` / `set_shadow_strength(f64)` / `set_shadow_size(int)` / `set_shadow_extent(f64)` |
| 演示 | `assets/scene_shadow.json`（地面 + 高柱 + 方块 + 球 + 圆柱，五物件四种网格）+ `scripts/shadow_demo.rhai`（光源绕 Y 轴扫动、强度呼吸、右键切阴影） |

### 17.2 关键口径（后续不得回退）

1. **阴影 pass 的布局必须与主 pass 分离**：主 pass 的 `group3` 里绑着阴影贴图本身，
   若阴影 pass 复用同一个 pipeline layout，wgpu 会判定「同一张纹理在同 pass 内既写入又采样」而 panic
   （实测报错：`A RenderPass ... has a texture that is both written and sampled`）。
   因此阴影 pass 用独立 layout（group0/1/2 占位 + `group3@binding3 = ShadowPassUniform`）。
2. **布局里每个 bind group 都必须在 pass 内被 `set_bind_group`**，哪怕着色器根本不引用它——
   wgpu 校验的是 pipeline layout 而非着色器实际使用。踩坑记录：先是报 index 0 未绑，补完后报 index 3 未绑。
3. **深度图不能原地 resize**：`size` 变化必须重建纹理 + 视图 + 比较采样器绑定组（`prepare_shadow` 内完成）。
4. **只有直接光乘阴影系数**，环境光项不乘；否则阴影区变纯黑、暗部层次全丢。
5. **三维场景取不到包围盒时不得让阴影失效**：`radius <= 1e-4` 时退回 1.0，宁可精度差也不出 NaN 矩阵。
6. **越界一律按受光**（UV 出 [0,1] 或 `ndc.z > 1` 返回 1.0）：阴影正交范围小于视锥时，
   视锥外的物件不能出现硬边黑块——表现为「影子边界被一刀切断」，比没有影子更难排查。
7. **场景默认开启阴影**：语义变化要写进文档（`light.shadow` 缺省即开启），需要旧观感请显式 `enabled:false`。

### 17.3 验收记录（2026-10-24）

| 项 | 方法 | 结果 |
| --- | --- | --- |
| 编译 | `cargo build --workspace` | 通过，无 warning |
| 单测 | `cargo test --workspace` | 43 passed / 0 failed（core 27、physics 12、render 3、asset 1） |
| 阴影真的在画 | `--run-seconds 6` 起止探针 | `阴影通道：draw call 4 / 主通道实例 5`（5 物件按网格合批成 4 组） |
| 关闭即归零 | 同一场景 `shadow.enabled = false` 跑一遍 | 探针 `draw call 0`，且截图确认地面无任何投影 |
| 目视（开） | 窗口运行 9s 后截屏 | 柱子、方块、球在一致方向上有边缘清晰的投影 |
| 目视（关） | 同上参数，仅关阴影 | 地面干净、无投影残留（纯光照渐变） |
| 逐帧重渲 | `shadow_demo.rhai` 光源绕 Y 轴旋转 | 影子随时间扫动，说明光源矩阵每帧重算并写入 |

### 17.4 尚未做（D 阶段范围外）

- **点光 / 聚光阴影**：只支持单方向光；无立方体阴影、无聚光透视阴影。
- **级联阴影（CSM）**：无远近分级，正交范围覆盖整个场景时远景纹素密度随场景尺寸下降。
- **软阴影 / VSM / 接触硬化**：仅 3×3 PCF，无可变半影、无接触硬化。
- **多光源与阴影图集**：单张深度图，一个方向光。
- **半透明与自发光投影**：深度 pass 不区分材质，透明物件也会投实影。

---

## 18. 统一资产流水线（AIR）：单一存储格式 + 分级分层输出（S0–S5 已落地）

> 状态：**S0–S5 全部落地**（2026-10-25）。目标是把「引擎内存储的 3D 资产」收敛成
> **唯一一种格式**，并让「输出到虚拟世界」的过程**逐层分工**，而不是每条支路自己解释数据。

### 18.1 一句话结论

| 层 | 只认什么 | 不认什么 |
| --- | --- | --- |
| 导入层（`r3e-asset`） | 输入：`.fbx` / `.glb` / `.gltf` | 输出给下游的一律是 AIR（`AssetEntry`） |
| 存储层（`r3e-core::AssetEntry`） | **AIR 一种格式**，落盘/内存都只有它 | 不再有「FBX 资产」「glb 资产」之分 |
| 输出层（`MeshProvider` 分层） | 只问 `mesh_data / mesh_aabb / mesh_parts` | 不问数据是谁生成的 |
| 消费层（render / physics / script） | `MeshData` 顶点、索引、分区、AABB | `MeshRef::Builtin` 之类的来源判断（已被守卫测试禁止） |

### 18.2 AIR 不变量（`r3e-core::air`）

所有导入器都必须产出满足下列条件的网格，`finalize_mesh()` / `importer::audit_entry()` 在
**每次导入末尾**统一校验（默认只告警、不拦截，批量导入不会被一条坏数据卡住）：

1. 坐标系：**+Y up、+Z front、右手系**；
2. 正面：**逆时针（CCW）**；镜像/负行列式变换必须自动翻转绕序；
3. 索引：`u32` 三角列表，长度是 3 的倍数，且全部落在顶点范围内；
4. 法线：单位长度、无 NaN；
5. UV：无 NaN（缺失时补零，不做猜测）；
6. 蒙皮：每顶点权重 ≤ 4 个，和为 1（`compact_skinning` 负责裁剪并重新归一化）；
7. 分区：`material_parts` 连续覆盖全部三角形，无空洞、无越界；
8. AABB：与顶点实际范围一致、非空；
9. 确定性：同一文件导入两次，顶点/索引/分区逐字节一致。

### 18.3 单份归一化工具箱

`r3e-core::air::normalize`（唯一实现，禁止各导入器各写一套）：

`to_engine_axes` / `bake_transform` / `flip_winding` / `recalc_normals` /
`ensure_indices` / `strip→triangles` / `fan→triangles` / `compact_skinning` /
`weld_vertices` / `finalize_mesh`

FBX（ufbx）与 glTF 在各自结尾都调用同一批函数——这就是「同模型换格式，结果不变」的物理基础。

### 18.4 导入器注册表

`r3e_asset::importer::ImporterRegistry` 按扩展名分派（大小写不敏感）：

| 扩展名 | 导入器 | 备注 |
| --- | --- | --- |
| `.fbx` | `import_fbx`（ufbx） | 角点拆分、烘焙动画、P4 蒙皮 |
| `.glb` | `import_glb`（gltf） | 二进制容器，贴图从 BIN 块解码 |
| `.gltf` | `import_gltf`（gltf） | JSON + 外部 `.bin` / 图片 |
| 其它 | 显式报错 | 不静默跳过，错误信息列出支持的扩展名 |

新增格式只需实现 `AssetImporter { extensions, import }` 并注册，**下游零改动**。

### 18.5 输出侧分级分层

```
MeshRef ──▶ LayeredProvider ──┬─▶ AssetLibrary（外部资产，AIR）
                              └─▶ ProceduralProvider（内置图元，同样产出 AIR）
                                        │
                                        ▼
                       MeshData / MeshParts / AABB（唯一形态）
                              │                     │
                              ▼                     ▼
                    r3e-render::MeshCache    r3e-physics（几何推断碰撞体）
```

- 内置图元已从渲染层迁到 `r3e-core::procedural`，`MeshCache` 里**只剩一条** `ensure` 路径；
- 物理不再按 `MeshRef` 猜形状，而是 `infer_shape(&MeshData)` 按顶点分布判定
  球 / 圆柱（轴向 Y）/ 长方体，半尺寸取 AABB × scale —— 外部导入的球形道具也能滚；
- 脚本层拿到的是同一份 AIR，`--dump-asset` 的审计段对任何格式输出同一套结论。

### 18.6 验收矩阵

| 测试 | 位置 | 锁死什么 |
| --- | --- | --- |
| `procedural_primitives_satisfy_air` | `r3e-core` | 内置图元本身就是合法 AIR |
| `gltf_and_glb_yield_the_same_air` | `r3e-asset/tests` | 同模型换容器，AIR 逐字段一致 |
| `import_is_deterministic_and_audited` | `r3e-asset/tests` | 导入确定 + 不变量 0 违规 |
| `shape_inference_reads_geometry_not_source` | `r3e-physics` | 形状只看几何、平移不影响判定 |
| `render/physics/app_layer_has_no_source_branch` | `r3e-render/tests` | 源码中不得出现来源分支 |
| `upload_path_is_single` | `r3e-render/tests` | `MeshCache::ensure` 只走统一 provider |

`cargo test --workspace`：**85 passed / 0 failed**（core 48、asset 6+3、physics 13+8、render 3+4）。

### 18.7 glb 已知限制（明示）

- 不支持 **Draco / meshopt** 压缩几何、**KTX2 / basis** 压缩贴图（需预转普通 PNG/JPEG 或 glTF）；
- 不支持 morph target（表情/形态键）、多套 UV、`KHR_materials_*` 扩展材质；
- 动画走 glTF 采样器原始关键帧（不做贝塞尔重采样），与 FBX 的「ufbx 烘焙」路径在极端插值下可能有细微差异；
- glTF 的右手 `-Z front` 会在导入时镜像到引擎 `+Z front`，并自动翻转绕序；镜像贴图 UV 不做额外翻转。
