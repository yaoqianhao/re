---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_105106dace9411f1ad5e525400638852
    ReservedCode1: EgohxgHBk+gfs/voa1np2ISIrdHNsSbtcYlQr8/cf2AXTzfRtLzjZfUYhuzi0tR52eNJUpb31vdvMQVo7AqufEEf9rPmZrHw7nQWB3HeAB5HMeL4Rh7V6oBPEND3Rh0avVrwz0I8RUM00dlL+9BRxu1FhmJwE3m0RVS9BILMK/xhas9EMdbcDjYb5o8=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_105106dace9411f1ad5e525400638852
    ReservedCode2: EgohxgHBk+gfs/voa1np2ISIrdHNsSbtcYlQr8/cf2AXTzfRtLzjZfUYhuzi0tR52eNJUpb31vdvMQVo7AqufEEf9rPmZrHw7nQWB3HeAB5HMeL4Rh7V6oBPEND3Rh0avVrwz0I8RUM00dlL+9BRxu1FhmJwE3m0RVS9BILMK/xhas9EMdbcDjYb5o8=
---

# r3e 引擎架构文档

> 版本：0.1.0
> 状态：已实现并通过实机验证（Windows 11 / RTX 5060 Laptop / Vulkan 后端）
> 文档定位：描述**当前代码真实实现**的分层结构、数据流、扩展点与已知边界，不是设计愿景稿。

---

## 1. 引擎定位

r3e 是一个用纯 Rust 实现的**最小可用 3D 游戏引擎**，核心命题只有一个：

> **用文本（JSON / 脚本 / 终端指令）就能完整驱动一个 3D 场景。**

围绕这个命题，形成三条不可动摇的设计原则：

| 原则 | 具体含义 | 在代码中的落点 |
| --- | --- | --- |
| 零外部资源依赖 | 不需要任何 `.obj` / `.gltf` / 贴图文件即可跑起来 | `r3e-render/src/mesh.rs` 全程序化生成 4 种内置网格 |
| 文本即接口 | 场景、行为、调试全部可由文本表达，且改动即时可见 | 场景 JSON + Rhai 脚本 + stdin REPL，三者写同一份 `Scene` |
| 物理可插拔 | 物理是抽象层，不是硬依赖；有实现也可整体关掉 | `PhysicsBackend` trait + Null / Builtin 双实现 + 运行时开关 |

这决定了引擎当前的技术形态：**单 pass 前向渲染 + 程序化几何 + 实例化绘制 + 脚本驱动状态**，而不是一个追求画面效果的完整渲染器。

---

## 2. 技术栈

全部依赖在根 `Cargo.toml` 的 `[workspace.dependencies]` 统一声明，各 crate 只引用不指定版本，避免版本漂移。

| 领域 | 依赖 | 版本 | 用途与选型理由 |
| --- | --- | --- | --- |
| 图形 API | `wgpu` | 30.0 | 跨后端（DX12 / Vulkan / GLES），无需手写平台代码 |
| 窗口与事件 | `winit` | 0.30 | `ApplicationHandler` 模型，与 `wgpu` surface 生命周期契合 |
| 手柄 | `gilrs` | 0.11 | 跨平台手柄抽象（XInput / DirectInput / evdev），把各厂商按键与轴归一到统一枚举 |
| 数学 | `glam` | 0.34（`bytemuck` feature） | SIMD 友好的 Vec/Mat/Quat，矩阵可直接零拷贝上传 GPU |
| 脚本 | `rhai` | 1.27（`serde` feature） | 纯 Rust 脚本，无 C 依赖，编译期即可嵌入式集成 |
| 序列化 | `serde` / `serde_json` | 1.0 | 场景文件解析，全部字段 `Option` + `default` 容错 |
| GPU 数据转换 | `bytemuck` | 1.21（`derive`） | `Pod` / `Zeroable` 派生，顶点与实例数据零成本转字节 |
| 异步桥接 | `pollster` | 0.4 | 把 `request_adapter` / `request_device` 的异步调用同步化 |
| 日志 | `log` + `env_logger` | 0.4 / 0.11 | 分级日志，默认 `info`，`RUST_LOG=debug` 看细节 |
| 错误 | `anyhow` / `thiserror` | 1.0 / 2.0 | 应用层用 anyhow，库层返回 `String` 错误保持 API 简单 |

编译配置：`[profile.dev] opt-level = 1`，且依赖统一 `opt-level = 3`——调试构建下 `wgpu` / `glam` 仍按优化编译，否则帧率会低到无法交互。

Rust 版本要求：`1.79`（`rust-version` 字段）。

---

## 3. Workspace 结构

```
rust3d-engine/
├─ Cargo.toml                  # workspace 根，统一依赖版本
├─ run.bat                     # 一键运行
├─ README.md
├─ .cargo/config.toml          # 构建环境配置
├─ assets/
│  └─ scene.json               # 默认场景（也是编译期兜底场景）
├─ scripts/
│  ├─ main.rhai                # 默认行为脚本
│  └─ example_swarm.rhai       # 群体动画示例
├─ docs/
│  └─ ARCHITECTURE.md          # 本文档
└─ crates/
   ├─ r3e-core/                # 内核：数学、变换、节点、场景图、相机、光照
   ├─ r3e-render/              # 渲染：wgpu 设备、管线、程序化网格、WGSL
   ├─ r3e-physics/             # 物理：后端契约 + Null / 内置最简实现
   ├─ r3e-script/              # 脚本：Rhai 宿主 + 引擎 API 绑定
   └─ r3e-app/                 # 应用：窗口、事件循环、主循环、REPL、热重载
```

### 3.1 依赖关系（严格单向，无环）

```
                       ┌──────────────┐
                       │  r3e-app     │  可执行体
                       └──┬────┬───┬──┘
                 ┌────────┘    │   └────────┐
                 ▼             ▼            ▼
          ┌────────────┐ ┌────────────┐ ┌────────────┐
          │ r3e-render │ │ r3e-script │ │r3e-physics │
          └─────┬──────┘ └────┬───┬───┘ └─────┬──────┘
                │             │   │           │
                └─────────────┴───┴───────────┘
                              ▼
                       ┌──────────────┐
                       │  r3e-core    │  无图形/窗口依赖
                       └──────────────┘
```

依赖约束及其意义：

- `r3e-core` **不依赖任何图形 API、窗口库或脚本引擎**，只依赖 `glam` + `serde`。因此它是可测试、可无头运行的真源层。
- `r3e-render`、`r3e-script`、`r3e-physics` 三者**互不依赖**：渲染器不认识 Rhai，脚本引擎不认识 wgpu。它们唯一的交点是 `r3e-core::Scene`。
- `r3e-app` 是唯一的组装层，负责把三方接进同一个帧循环。

这条约束是引擎"文本能控制画面"的结构性保证：**所有子系统读写同一份内存中的 `Scene`，而不是通过消息或事件副本通信。**

### 3.2 代码规模

| crate | 主要文件 | 行数量级 |
| --- | --- | --- |
| r3e-core | `scene.rs`(343) / `input.rs`(219) / `transform.rs`(86) / `node.rs`(76) / `camera.rs`(65) / `light.rs`(19) | ~824 |
| r3e-render | `renderer.rs`(423) / `mesh.rs`(213) / `shader.wgsl`(55) | ~639 |
| r3e-physics | `lib.rs`(858) | ~858 |
| r3e-script | `api.rs`(577) / `lib.rs`(218) | ~795 |
| r3e-app | `app.rs`(668) / `main.rs`(164) | ~832 |
| **合计** | | **约 3950 行 Rust + 55 行 WGSL** |

Release 二进制 21.8 MB（含 wgpu 后端静态链接与调试符号剥离前的体积）。

---

## 4. 内核层：r3e-core

### 4.1 模块划分

| 模块 | 职责 | 关键类型 |
| --- | --- | --- |
| `transform.rs` | 单节点局部变换数学 | `Transform` |
| `node.rs` | 物件本体与网格种类枚举 | `Node`, `NodeId`, `MeshKind` |
| `scene.rs` | 场景容器、生命周期、层次变换、文件加载 | `Scene`, `ObjectDesc`, `SceneFile` |
| `camera.rs` | 透视相机与轨道控制 | `Camera` |
| `light.rs` | 单方向光 + 环境光 | `Light` |

`lib.rs` 只做 re-export：`pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4}`，让上层 crate 不必各自引入 `glam`。

### 4.2 Transform：变换表示

```rust
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
```

设计要点：

1. **旋转用四元数存储，用欧拉角交互**。存储层 `Quat` 避免万向节死锁与插值退化；接口层提供 `set_euler_deg` / `euler_deg`（XYZ 顺序、角度制），因为脚本用户写 `set_rot("box", 0, 45, 0)` 最自然。
2. **局部矩阵组合顺序固定 `T * R * S`**（`Mat4::from_scale_rotation_translation`），与主流引擎一致；先缩放再旋转再平移，非均匀缩放不会污染旋转。
3. **`look_at` 需要取逆**。`glam` 的 `look_at_quat` 返回的是"相机朝向"（前向为 -Z），而物件的"朝向"是 +Z 前沿，因此 `Transform::look_at` 内部调用 `.inverse()`。这是易错点，注释已在代码中标明。
4. **`normal_matrix` 在 CPU 侧不用于渲染**。着色器用伴随矩阵近似，CPU 侧该函数预留给将来需要 CPU 侧法线计算的功能（如包围盒、拾取）。

### 4.3 Node：场景中的物件

```rust
pub struct Node {
    pub id: NodeId,               // u64，单调递增、永不复用
    pub name: String,             // 场景内唯一
    pub transform: Transform,
    pub color: Vec4,              // 线性空间 RGBA
    pub mesh: MeshKind,
    pub visible: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub body: Option<u64>,        // 物理刚体句柄（物理关闭时恒为 None）
    pub world: Mat4,              // 每帧刷新的世界矩阵缓存
    pub world_normal: glam::Mat3, // 世界法线矩阵缓存
}
```

- **`NodeId` 不复用**是刻意的：脚本/物理持有的是句柄，如果 ID 回收，一个残留句柄就会静默指向新物件，产生极难排查的 bug。代价只是 64 位空间缓慢增长，可以忽略。
- **`world` / `world_normal` 是缓存而非真源**，由 `Scene::update_world_transforms` 每帧重算。渲染器只读它，从不自己算矩阵。
- **`MeshKind` 仅 4 种**：`Cube` / `Sphere` / `Plane` / `Cylinder`，并支持别名解析（`box`→Cube、`ball`→Sphere、`ground`/`quad`→Plane、`tube`→Cylinder）。没有自由网格，是当前几何表达力的主要边界。

### 4.4 Scene：唯一真源

```rust
pub struct Scene {
    nodes: HashMap<NodeId, Node>,   // ID → 节点
    order: Vec<NodeId>,             // 插入顺序（绘制顺序稳定性）
    name_index: HashMap<String, NodeId>, // 名字 → ID（脚本按名寻址）
    next_id: NodeId,

    pub name: String,
    pub time: f32,      // 引擎时间（秒）
    pub frame: u64,     // 帧序号
    pub camera: Camera,
    pub light: Light,
    pub background: Vec4,
}
```

**三索引结构**是核心设计：`HashMap` 保证 O(1) 按 ID 访问，`name_index` 保证 O(1) 按名访问（脚本几乎全按名操作），`order` 保证遍历顺序确定——渲染器按 `order` 遍历分组，因此同一份场景每次渲染的实例顺序完全一致，画面不会出现闪烁。

生命周期 API：

| 方法 | 行为 | 关键约束 |
| --- | --- | --- |
| `spawn(desc) -> Result<NodeId, String>` | 生成物件 | 名字必须非空、不得重名、父物件必须已存在；任一违反返回 `Err` |
| `despawn(name) -> bool` | 按名移除 | 不存在返回 `false`，不 panic |
| `despawn_id(id)` | 按 ID 移除 | **递归移除所有子物件**，并从父节点 `children` 中摘除 |
| `clear_nodes()` | 清空所有物件 | 脚本热重载时用，保留相机/光照/背景 |

重名直接报错而不是覆盖，是刻意的保守选择：脚本里 `create_node` 重复创建同名物件时，静默覆盖会让用户以为"物件消失了"，报错更容易定位。

查询 API：`get` / `get_mut`（按 ID）、`find` / `find_mut` / `find_id`（按名）、`find_all(prefix)`（按名前缀，返回排序后的名字列表）、`len` / `is_empty` / `iter`。

**层次变换求解**：

```rust
fn update_world_transforms(&mut self) {
    // 1. 一次性收集所有 id（避免迭代中变借用）
    // 2. 对每个 id 调 resolve_world（递归 + HashMap 缓存）
    // 3. 写回 node.world 与 node.world_normal
}

fn resolve_world(&self, id, depth, cache) -> Mat4 {
    // 缓存命中直接返回
    // world = parent_world * local，depth >= 64 或自引用时退化为 local
}
```

三层防护：

- **记忆化缓存**：父链只算一次，深层次结构不会退化成 O(n·depth)。
- **深度上限 64**：防环（`A.parent = B, B.parent = A` 这种数据错误在脚本里可能出现）。
- **`p != id` 检查**：直接阻断自引用。

法线矩阵用 `Mat3::from_mat4(world).inverse().transpose()`——逆矩阵奇异时 `glam` 会产出 NaN/Inf，这是已知风险，当前由"缩放不为 0"的使用约定兜住（`Transform::normal_matrix` 里有 `max(1e-6)` 保护，但 Scene 里的世界法线矩阵没有，属可加固点）。

**文件加载**：

`SceneFile` 的每个字段都是 `Option` 并带 `#[serde(default)]`，因此最小合法场景就是 `{}`——任何字段缺失都退化为默认值，不会解析失败。`from_file` 的流程是：先造空 `Scene` → 应用 `background` / `camera` / `light`（光照方向做零向量保护）→ 逐个 `spawn(objects)` → 最后 `update_world_transforms()`。

### 4.5 Camera

```rust
pub struct Camera {
    pub position: Vec3,  // 默认 (4, 3, 6)
    pub target: Vec3,    // 默认原点
    pub up: Vec3,        // 默认 +Y
    pub fov_deg: f32,    // 默认 60
    pub znear: f32,      // 默认 0.05
    pub zfar: f32,       // 默认 500
    pub aspect: f32,     // 每帧由渲染器覆盖为真实窗口比例
}
```

**投影矩阵必须用 DirectX 风格**：`glam::camera::rh::proj::directx::perspective`。原因是 wgpu 的 NDC 深度区间是 `[0, 1]` 而不是 OpenGL 的 `[-1, 1]`。用错会导致深度全部被裁剪或远近颠倒——这是 wgpu 移植最常见的坑，代码中有显式注释。

`orbit(yaw_deg, pitch_deg)` 实现轨道控制：把 position 相对 target 的偏移转为球坐标，加角度后 `pitch` 钳制在 ±1.5 rad（约 ±86°），避免在极点处翻转。滚轮推拉在 `app.rs` 里做，半径钳制在 `0.8 ~ 80.0`。

`aspect` 是唯一由外部每帧写入的字段：`renderer.prepare_instances` 中 `camera.aspect = self.aspect()`，因此在场景 JSON 里写 aspect 无意义。

### 4.6 光照模型

```rust
pub struct Light {
    pub direction: Vec3,  // 默认归一化的 (-0.45, -0.8, -0.4)，即光线传播方向
    pub color: Vec3,      // 默认 (1.0, 0.97, 0.92) 略偏暖
    pub intensity: f32,   // 默认 1.0
    pub ambient: Vec3,    // 默认 (0.18, 0.19, 0.23) 略偏冷
}
```

只有一个平行光 + 环境光，没有点光、聚光、阴影。`direction` 的语义是**光线行进方向**（从光源射出），不是"指向光源的方向"——着色器里取 `normalize(-light_dir)` 得到表面指向光源的向量。

---

## 5. 渲染层：r3e-render

### 5.1 GPU 数据布局

**顶点格式 `Vertex`（stride 24 字节）**

| 字段 | 类型 | location | 说明 |
| --- | --- | --- | --- |
| `position` | `[f32; 3]` | 0 | 模型空间位置 |
| `normal` | `[f32; 3]` | 1 | 模型空间法线（已归一化） |

**实例格式 `InstanceRaw`（stride 80 字节，`step_mode = Instance`）**

| 字段 | 类型 | location | 说明 |
| --- | --- | --- | --- |
| `model` | `[[f32; 4]; 4]` | 2,3,4,5 | 世界矩阵，按 4 个 vec4 列传入 |
| `color` | `[f32; 4]` | 6 | 线性空间 RGBA |

之所以把模型矩阵放进实例缓冲而不是 uniform：这样**整个场景一次上传、按网格分组一次绘制**，不随物件数量增加 draw call。`step_mode = Instance` 让每个实例读同一份顶点但不同矩阵。

### 5.2 程序化网格

`MeshData::for_kind(kind)` 按类型分派，全部在 CPU 侧生成后上传，无文件 IO：

| 网格 | 参数 | 实现要点 |
| --- | --- | --- |
| `cube()` | 边长 1，中心原点 | **24 顶点 / 36 索引**：每个面独立 4 顶点，因此有硬边法线（复用顶点会得到"圆角"着色，视觉上像被磨过） |
| `sphere(28, 18)` | UV 球，半径 0.5 | `segments × rings` 双层循环，法线即单位球面方向；极点处有三角形退化，视觉上不可见 |
| `plane(10)` | XZ 平面 1×1，+Y 法线 | **10×10 细分**：单四边形在方向光下只需插值一个法线，本可不用细分；细分是为了将来接逐顶点光照/顶点动画时不出块状失真，代价仅 121 顶点 |
| `cylinder(28)` | 半径 0.5，高 1，轴向 Y | 侧面 28 段 + 上下盖扇形；盖法线固定 ±Y，侧面法线水平辐射 |

所有网格**统一尺寸为 1 单位**，物件的实际大小完全由 `Transform.scale` 决定——这样 `half_extents = scale * 0.5` 的物理碰撞体推导才能成立。

### 5.3 Renderer 初始化链路

`Renderer::new(window: Arc<Window>)`：

```
1. wgpu::Instance::new(Descriptor::new_without_display_handle())
2. instance.create_surface(window.clone())         → wgpu::Surface<'static>
3. instance.request_adapter(HighPerformance)        → 优先独显
4. adapter.get_info() → 日志打印 GPU 名/后端/设备类型
5. adapter.request_device(空 features, 默认 limits)
6. surface.get_capabilities + get_default_config
7. 若候选格式中存在 sRGB → 覆盖 config.format   ← 关键：否则画面发灰
8. surface.configure
9. 创建 CameraUniform buffer (128 B, UNIFORM | COPY_DST)
10. 创建 BindGroupLayout (binding 0, VERTEX|FRAGMENT, uniform buffer)
11. 创建 BindGroup
12. 编译 shader.wgsl（include_str! 编译期内嵌，无运行时文件依赖）
13. 创建 PipelineLayout → RenderPipeline
14. 创建深度纹理 Depth32Float 并取 view
15. 创建实例缓冲（初始 512 个实例容量）
```

**便携性处理**：`InstanceDescriptor::new_without_display_handle()` 是 wgpu 30 的新 API，不传 display handle，避免在 Windows 上额外的平台句柄耦合。

**Pipeline 固定状态**：

| 项 | 取值 | 说明 |
| --- | --- | --- |
| 拓扑 | `TriangleList` | |
| 正面 / 剔除 | `Ccw` / 剔除背面 | 绕序错了会看到"物体内部" |
| 深度格式 | `Depth32Float` | 32 位浮点，避免 z-fighting 更敏感 |
| 深度写入 / 比较 | `true` / `Less` | |
| 混合 | `ALPHA_BLENDING` | 支持半透明色，但**未做前后排序**，半透明叠加顺序依赖 `order` |
| 多重采样 | 默认（1x） | 无 MSAA，边缘有锯齿 |

管线只创建一次，之后场景怎么变都不重建——这是"文本改动下一帧即生效"的前提。

### 5.4 单帧渲染流程

```
render(scene):
  ├─ prepare_instances(scene)
  │   ├─ camera.aspect ← 真实窗口比例
  │   ├─ 组装 CameraUniform 并 write_buffer（view_proj / camera_pos / light_dir / light_color+intensity / ambient）
  │   ├─ 按固定顺序 [Cube, Sphere, Plane, Cylinder] 遍历 scene.iter()
  │   │     └─ 跳过 !visible；用 node.world 与 node.color 组装 InstanceRaw
  │   │        每组记录 Range<u32>（连续区间）
  │   ├─ 惰性上传尚未创建的网格（首次用某网格才传）
  │   ├─ 实例数超过容量 → 2 倍扩容重建 buffer
  │   └─ write_buffer 上传全部实例
  ├─ surface.get_current_texture()
  │   ├─ Success    → 直接用
  │   ├─ Suboptimal → 重新 configure 后用
  │   ├─ Outdated / Lost → 重新 configure 并 return（跳过本帧）
  │   └─ 其它        → 记 debug 日志并 return
  ├─ 开 RenderPass：clear 颜色 = scene.background，clear 深度 = 1.0
  ├─ for (kind, range) in groups:
  │     set_vertex_buffer(0, 该网格顶点)
  │     set_vertex_buffer(1, instance_buffer[range])
  │     set_index_buffer(Uint32)
  │     draw_indexed(index_count, 0, 0..1)     ← 1 次 call 画完该网格全部实例
  ├─ 统计 RenderStats { draw_calls, instances, frame_index }
  ├─ queue.submit + queue.present
```

**关键性质**：每帧从 `Scene` **全量重建**实例数据，不维护任何 GPU 侧持久状态（除了静态网格与管线）。因此脚本删物件、改颜色、改位置，都不需要通知渲染器，也不存在"状态没同步"的问题。代价是每帧一次 O(可见物件数) 的 CPU 组装与全量上传——在当前规模（数千物件）完全够用，上万物件时会成为瓶颈。

**Draw call 数 = 实际使用的网格种类数**（≤ 4）。1000 个立方体是 1 个 draw call，不是 1000 个。

### 5.5 着色器（shader.wgsl）

**Uniform 布局（128 字节，16 字节对齐）**

| 字段 | 类型 | 内容 |
| --- | --- | --- |
| `view_proj` | `mat4x4<f32>` | 视图投影矩阵 |
| `camera_pos` | `vec4<f32>` | xyz = 相机世界位置 |
| `light_dir` | `vec4<f32>` | xyz = 光线行进方向 |
| `light_color` | `vec4<f32>` | xyz = 光色，**w = 强度** |
| `ambient` | `vec4<f32>` | xyz = 环境光色 |

**顶点阶段**

```wgsl
let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
let world = model * vec4(in.position, 1.0);

// 法线矩阵用伴随矩阵（余子式转置），非均匀缩放下是严格逆转置的近似
let normal_matrix = mat3x3<f32>(
    cross(model[1].xyz, model[2].xyz),
    cross(model[2].xyz, model[0].xyz),
    cross(model[0].xyz, model[1].xyz),
);
```

用伴随矩阵而不是真的算逆转置，是为了避免在顶点着色器里做 `inverse`（GLSL/WGSL 的 3x3 逆代价高且部分后端实现差异大）。**均匀缩放下该近似是精确的**；非均匀缩放时法线会有轻微偏差——注释已明示这一取舍。

**片元阶段（Blinn-Phong）**

```wgsl
n = normalize(world_normal)
l = normalize(-light_dir)
v = normalize(camera_pos - world_position)
h = normalize(l + v)

ndl  = max(dot(n, l), 0)
spec = pow(max(dot(n, h), 0), 48.0) * 0.35 * step(0.001, ndl)

color = base.rgb * (ambient + light_color * intensity * ndl) + spec
```

- 高光指数 48、强度系数 0.35 是硬编码的"塑料感"参数。
- `step(0.001, ndl)` 让背光面不出现高光泄漏（否则高光会穿透到背面）。
- **没有 gamma 校正代码**，依赖 surface 用 sRGB 格式自动完成转换；颜色值按线性空间解释。
- 没有任何纹理采样、没有阴影贴图、没有 PBR 参数。

### 5.6 RenderStats 与 HUD

```rust
pub struct RenderStats { pub draw_calls: u32, pub instances: u32, pub frame_index: u64 }
```

`app.rs` 每帧把统计写进窗口标题：`r3e — Rust 3D Engine | N 物件 / M draw / X.X ms | 运行中`。这是当前唯一的运行时观测手段（无 GUI overlay）。

---

## 6. 物理层：r3e-physics

> 完整用法、规则与实测数据见 [`docs/PHYSICS.md`](PHYSICS.md)。本节只讲架构。

### 6.1 设计目的：契约先于实现

物理层被切成**两层**，互不耦合：

1. **契约层**：`PhysicsBackend` trait，让场景格式（`physics` 段）与脚本 API（`attach_body` 等）先能写。
2. **实现层**：默认 `NullBackend`（空转）/ `BuiltinBackend`（自实现最简物理）/ 预留 `RapierBackend`（未编译进默认构建）。

好处是「有没有物理」变成**运行时开关**，而不是编译期分叉：同一份场景与脚本，开物理会掉、关物理由脚本驱动。

### 6.2 后端契约

```rust
pub trait PhysicsBackend {
    fn name(&self) -> &'static str;
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;
    fn destroy_body(&mut self, handle: BodyHandle);
    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3);
    fn set_gravity(&mut self, gravity: Vec3);
    fn step(&mut self, dt: f32);
    fn sync_to_scene(&mut self, scene: &mut Scene);
    fn body_count(&self) -> usize;
    // 可选（有默认实现，内置后端会覆盖）
    fn set_ground(&mut self, ground: f32) {}
    fn pull_from_scene(&mut self, scene: &Scene) {}
    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> { None }
}
```

职责边界写得很清楚：**后端负责积分与碰撞，并负责把结果写回 Scene**（`sync_to_scene`）。
上层不关心物理怎么算，只在帧序里调 `step(dt, &mut scene)`。

`set_ground` / `pull_from_scene` 之所以放进 trait 而不是内置后端私有，是为了让 Rapier 之类外部后端也能接：
前者映射到地面体，后者映射到 `sync_back`/读回变换。

| 类型 | 内容 |
| --- | --- |
| `BodyHandle(u64)` | 后端分配的句柄，写回 `Node::body` |
| `BodyKind` | `Static` / `Dynamic` / `Kinematic`，`parse` 支持 `fixed` 别名 |
| `ColliderShape` | `Box` / `Sphere` / `Capsule` / `Cylinder` |
| `BodyDesc` | kind / shape / half_extents / mass / restitution(0.2) / friction(0.6) / position / rotation / scale |

`BodyDesc::infer(shape, scale)` 从网格与缩放推导碰撞体：球 → 半径 `scale.x/2`，柱/胶囊 → 半高 `scale.y/2`，
其余 → 盒 `scale/2`。脚本 `attach_body(name, kind, mass)` 走这条路，所以 `create_node` 出来的物件可直接挂刚体。

### 6.3 三种后端

| 后端 | 状态 | 行为 |
| --- | --- | --- |
| `NullBackend` | 默认 | 只分配句柄并登记，`step` / `sync_to_scene` 空转。用于「只想在脚本里表达物理意图、暂不模拟」 |
| `BuiltinBackend` | 可用 | 半隐式欧拉 + 无限地面平面碰撞 + 弹性 + 摩擦 + 休眠，纯自实现零依赖，约 400 行 |
| `RapierBackend` | 预留 | `physics_set_backend("rapier")` 会返回 false 并打印「未编译进本构建」，不静默降级 |

`NullBackend` 的 `take_all_handles` 返回 `Some(列表)`，让 `Physics::clear_bodies` 能整体清空——热重载不留幽灵句柄。

### 6.4 Physics 门面：开关 + 世代号

```rust
pub struct Physics {
    backend: Box<dyn PhysicsBackend>,   // 动态分发，运行时可换
    kind: PhysicsKind,                  // Null / Builtin / Rapier
    enabled: bool,                      // 总开关
    pub gravity: Vec3,                  // 默认 (0, -9.81, 0)
    ground: f32,                        // 地面平面高度，默认 0
    generation: u64,                    // 配置世代号
    overrides: PhysicsOverrides,        // 命令行粘性覆盖
}
```

- **`enabled=false` 时**：`step` 立即返回、`create_body` 直接返回 `false`、`body_count()` 报 0。
  上层无需到处写 `if`，开关语义收敛在门面里。
- **`set_enabled(true)` 且后端仍是 `null`** → 自动升级到 `builtin`。避免「开了却没动」。
- **`generation` 自增**于开关切换、后端重建、`clear_bodies`。`app` 只在世代变化时重绑场景刚体，
  所以脚本手动 `attach_body` 的刚体不会被每帧覆盖。
- **`overrides` 记命令行值**，热重载重放场景 JSON 后重新合并，实现「命令行粘性、场景层随文件变、脚本改动易失」。
- `step(dt, scene)` 串联 `pull_from_scene` + `backend.step(dt)` + `backend.sync_to_scene(scene)`，
  并统一 `update` 重力/地面到后端。调用方只有一个入口。
- 手工实现 `Debug`（`Box<dyn Trait>` 不自动实现 Debug）。

三层配置（命令行 / 场景文件 / 内置默认）的合并逻辑放在 `PhysicsConfig::from_scene` + `apply_config` / `apply_override`，
放在 `r3e-physics` 而不是 `app`，是为了让无窗口的测试也能覆盖优先级。

### 6.5 app 层：自动绑定

`app` 每帧在脚本之后、物理之前调 `sync_physics_bindings`：

1. 世代号没变 → 什么都不做（零成本）。
2. 世代号变了（开关翻转 / 后端切换 / 热重载）→ 遍历场景，把**声明了 `physics` 段**的物件逐个 `attach_body` 式绑定。
3. 纯装饰物件（无 `physics` 段）永远不由引擎自动绑，只能脚本显式 `attach_body`。

这条规则让「场景是静态描述、脚本是行为」的分工成立：场景写 `"physics": {...}` 就该掉，
没写就归脚本管。窗口标题实时显示 `物理 builtin / N 刚体` 或 `物理关`。

### 6.6 接入外部后端（Rapier）的路径

改动面固定在三处，其余零改动：

1. 新建 `crates/r3e-physics/src/rapier_backend.rs` 实现 `PhysicsBackend`：`BodyDesc.shape` → `SharedShape`，
   `BodyKind` → `RigidBodyType`，`step` 内 `pipeline.step(&gravity, ...)`，`sync_to_scene` 内把
   `translation` / `rotation` 写回节点，`update_world_transforms` 由 `app` 统一调。
2. `PhysicsKind::Rapier` 分支接上真实实例（`Physics::rebuild_backend` 里一行）。
3. `Cargo.toml` 加 `rapier3d`（当前刻意零依赖）。

场景格式、脚本 API、`app` 层、`core::Scene` **均无需改动**。
唯一语义差异：外部后端做物件间碰撞与旋转动力学，内置后端不做——属能力升级，不是接口破坏。

---

## 7. 脚本层：r3e-script

### 7.1 共享真源的实现方式

```rust
pub struct ScriptHost {
    engine: Engine,
    clock: Rc<RefCell<Clock>>,
    scene: Rc<RefCell<Scene>>,       // 与 app 层共享同一份
    physics: Rc<RefCell<Physics>>,   // 与 app 层共享同一份
    ast: Option<AST>,
    scope: Scope<'static>,           // 持久作用域：跨帧保存脚本全局变量
    script_path: Option<PathBuf>,
    script_mtime: Option<SystemTime>,
    base_scene_json: Option<String>, // 热重载时用于重放
    pub last_error: Option<String>,
}
```

`Rc<RefCell<Scene>>` 是关键：脚本执行时**直接修改主循环持有的那份场景**，不产生副本，不需要"提交"步骤，因此改动在下一帧渲染立即可见。

代价是 `RefCell` 的运行时借用检查——API 绑定里必须小心避免重入 `borrow_mut()`（`attach_body` 里先 `borrow()` 读完信息、`drop(scene)`、再 `physics.borrow_mut()`，就是这个原因）。

### 7.2 脚本执行模型

**加载**（`load_str`）：

```
1. engine.compile(src)                        → AST，语法错误在此暴露
2. scope.clear()                              → 清空上次的全局变量
3. engine.run_ast_with_scope(&mut scope, &ast) → 执行顶层语句
4. ast 存起来
5. 若存在 on_start()（零参数）→ engine.call_fn("on_start")
```

**每帧**（`tick(dt, time, frame)`）：

```
1. 更新 Clock { dt, time, frame }
2. 若有 AST 且定义了 on_update(dt) → engine.call_fn("on_update", (dt,))
3. 出错写入 last_error，且同一条错误只打印一次（避免刷屏）
```

**顶层语句会被重复执行的陷阱**（已在代码注释中记录）：

Rhai 的 `call_fn(scope, ast, ...)` 会连同 AST 的**顶层语句**一起执行。因此如果把 `create_node("a","cube",0,0,0)` 写在顶层而不是 `on_start` 里，每帧都会执行一次——结果是第一帧后全部报"物件已存在"。这是当前最容易踩的坑，而 `spawn` 的重名校验恰好把它从"静默内存泄漏"变成了"明确报错"。

**函数存在性检查**（`has_fn`）在调用前完成：`ast.iter_functions().any(|f| f.name == name && f.params.len() == arity)`。因此脚本不写 `on_update` 也完全合法，引擎不会报错，只是物件静止。

### 7.3 热重载

```rust
pub fn reload_if_changed(&mut self) -> bool {
    // 1. 读 mtime，与记录值相同 → return false
    // 2. 重放基础场景：base_scene_json → Scene::from_json_str → 覆盖当前场景
    //    （无基础场景则 clear_nodes）
    // 3. physics.clear_bodies() 清空幽灵刚体
    // 4. load_file(path)：重新编译 + 重跑顶层 + on_start
    // 5. 成功打印"热重载完成"
}
```

**"重放基础场景"是热重载正确性的核心**：如果不还原场景，上一次脚本已经生成的物件会残留在场景里，新脚本再 `create_node` 同名物件就会撞重名。所以每次热重载都从 JSON 干净重建，等价于"重启进程但保留窗口与 GPU 状态"。

`rebuild_scene(json)` 走同一套重放逻辑，只是场景来源换成新文件内容——这样改 `scene.json` 和改 `main.rhai` 的行为一致。

### 7.4 引擎 API 绑定（完整清单）

所有函数通过 `engine.register_fn` 注入，命名风格偏"短动词 + 按名寻址"，因为脚本是手写的高频输入。

**输出**

| 函数 | 说明 |
| --- | --- |
| `log(msg)` | 打印 `[script] msg` |

**生命周期**

| 函数 | 签名 | 说明 |
| --- | --- | --- |
| `create_node(name, mesh, x, y, z)` | → bool | 生成物件 |
| `create_node_at_origin(name, mesh)` | → bool | 原点生成 |
| `create_node_json(json)` | → bool | 用单物件 JSON 生成，字段同场景文件 |
| `remove_node(name)` | → bool | 移除（含子物件） |
| `remove_nodes(prefix)` | → i64 | 按名前缀批量移除，返回移除数 |
| `exists(name)` | → bool | 是否存在 |
| `count()` | → i64 | 物件总数 |
| `names(prefix)` | → Array | 前缀匹配的名字列表 |

**变换**

| 函数 | 说明 |
| --- | --- |
| `set_pos(name, x, y, z)` | 设置位置 |
| `move_by(name, dx, dy, dz)` | 相对位移 |
| `get_pos(name)` | 返回 `[x, y, z]`，不存在返回 `[0,0,0]` |
| `set_rot(name, x, y, z)` | 设置欧拉角（角度制） |
| `rotate(name, x, y, z)` | 在当前欧拉角上叠加 |
| `set_scale(name, x, y, z)` | 非均匀缩放 |
| `set_scale_uniform(name, s)` | 均匀缩放 |

**外观**

| 函数 | 说明 |
| --- | --- |
| `set_color(name, r, g, b)` | 保留原 alpha |
| `set_color4(name, r, g, b, a)` | 含 alpha |
| `set_visible(name, bool)` | 显隐 |
| `set_mesh(name, mesh)` | 换网格类型（运行时变形） |

**相机 / 光照 / 环境**

| 函数 | 说明 |
| --- | --- |
| `camera(px, py, pz, tx, ty, tz)` | 直接设置位置与注视点 |
| `orbit(yaw_deg, pitch_deg)` | 轨道旋转 |
| `fov(deg)` | 视场角 |
| `set_light(dx, dy, dz)` | 光照方向（自动归一化，零向量忽略） |
| `set_light_color(r, g, b, intensity)` | 光色 + 强度 |
| `set_ambient(r, g, b)` | 环境光 |
| `set_background(r, g, b)` | 背景色 |

**时钟与随机**

| 函数 | 说明 |
| --- | --- |
| `time()` | 引擎时间（秒，f64） |
| `frame()` | 帧序号 |
| `dt()` | 上一帧耗时（秒） |
| `rand(min, max)` | xorshift64 伪随机，返回 `[min, max)` |

随机数用自实现的 xorshift64（种子 `0x2545F4914F6CDD1D`）而非 `rand` crate，是为了不引入额外依赖并保证可复现——种子固定，每次启动的随机序列相同。

**物理**

| 函数 | 说明 |
| --- | --- |
| `attach_body(name, kind, mass)` | 按物件网格自动推导碰撞体形状，`half_extents = scale * 0.5`；物理关闭时返回 `false` |
| `detach_body(name)` | 销毁刚体并清空 `Node::body` |
| `body_impulse(name, x, y, z)` | 施加瞬时冲量，会唤醒休眠体 |
| `body_count()` | 当前刚体数（物理关闭时恒为 0） |
| `physics_enabled()` / `physics_set_enabled(on)` | 读 / 写总开关，写入返回操作后的状态 |
| `physics_backend()` / `physics_set_backend(name)` | 读 / 换后端（`null` / `builtin` / 预留 `rapier`），换后端会清空已有刚体 |
| `physics_gravity()` / `physics_set_gravity(x,y,z)` | 读 / 写重力 |
| `physics_ground()` / `physics_set_ground(y)` | 读 / 写地面平面高度 |

**命名变更说明**：`spawn` / `despawn` 原本更自然，但 `spawn` 在 Rhai 中是异步相关的保留字，`despawn` 也随之改为对称的 `remove_node`。这是实现约束导致的命名妥协，已写入 README 与脚本示例。

**安全阀**：`engine.set_max_operations(50_000_000)`，防止脚本里写死循环把主线程卡死。

### 7.5 RELP 求值

`eval(src)` 用 `eval_with_scope::<Dynamic>` 在持久作用域里执行一行代码，直接返回结果。因为共享同一份 `Scene`，所以 REPL 里 `set_pos("box", 0, 5, 0)` 会立刻改变画面。

调用栈为：`app.pump_console` → `scripts.eval` → 引擎 API → `Scene`。REPL 与脚本走的是**同一套 API 绑定**，不存在两套语义。

---

## 8. 应用层：r3e-app

### 8.1 程序入口（main.rs）

```rust
r3e [场景.json] [脚本.rhai]
```

未指定参数时按 `find_project_root()` 定位：

1. 当前工作目录（若存在 `assets/` 子目录）——覆盖 `cargo run` 与在项目根目录双击 exe 的情况；
2. 可执行文件所在目录向上最多 3 层（若找到 `assets/`）——覆盖 exe 被放在 `target/release/` 时的情况；
3. 编译期路径 `CARGO_MANIFEST_DIR` 的父目录——最后的兜底。

三级兜底的意义：**双击 `target/release/r3e.exe` 和在项目根目录 `cargo run` 都能直接跑起来**，不要求用户理解工作目录。

事件循环用 `ControlFlow::Poll`（连续重绘，不受 vsync 节流），配合 `about_to_wait` 里主动 `request_redraw()` 形成不阻塞的连续帧。日志默认 `info` 级。

### 8.2 EngineApp 结构

```rust
pub struct EngineApp {
    scene_path: PathBuf,
    script_path: PathBuf,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,          // resumed 后才存在
    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    scripts: ScriptHost,
    last_frame: Instant,
    last_watch: Instant,                 // 文件监视节流
    time: f64,
    frame_index: u64,
    paused: bool,
    keys: HashSet<Key>,                  // 当前按下的键
    input: Rc<RefCell<InputState>>,      // 鼠标/手柄帧级快照，与脚本层共用同一份
    gilrs: Option<gilrs::Gilrs>,         // 手柄后端；初始化失败降级为 None
    cursor_valid: bool,                  // 是否已收到过 CursorMoved（首帧不产生位移）
    last_dt_ms: f32,                     // 供 HUD 显示
    console: mpsc::Receiver<String>,     // REPL 输入通道
    scene_mtime: Option<SystemTime>,
}
```

`window` / `renderer` 是 `Option`：`winit 0.30` 的 `resumed` 之前不能创建窗口，这是 winit 新版生命周期的强制约束，不是可选的容错设计。

**兜底场景**：`const FALLBACK_SCENE: &str = include_str!("../../../assets/scene.json")` 在编译期内嵌。场景文件缺失或解析失败时用它，保证**引擎在任何情况下都能启动并出一幅画面**，而不是黑屏退出。

### 8.3 单帧主循环（step_and_render）

顺序是严格设计的，每一步都依赖前一步：

```
1. pump_console()                      处理 REPL 输入（可能改变 paused 与场景）
1.5 poll_gamepad()                     排空 gilrs 事件队列，取手柄状态写入 InputState
2. dt = now - last_frame，clamp 到 [0, 0.1]   ← 防卡顿后一次跳跃过大（切窗口回来常见）
3. if !paused:
   3.1 time += dt; frame_index += 1
   3.2 scripts.tick(dt, time, frame)    ← 脚本可能生成/销毁/移动物件
   3.3 apply_camera_input(dt)           ← 键盘 + 手柄摇杆/扳机驱动相机
   3.4 scene.time / scene.frame ← 当前值
   3.45 sync_physics_bindings()          ← 物理世代号变了才重绑场景声明的刚体
   3.5 physics.step(dt, &mut scene)     ← 开关关闭时内部直接返回
   3.6 scene.update_world_transforms()  ← 必须在脚本与物理之后！
4. renderer.render(&scene)              ← 从 world 矩阵组装实例并绘制
5. 更新窗口标题 HUD（含手柄连接状态）
6. input.end_frame()                    ← 清边沿与增量，down 状态保留
```

`poll_gamepad` 放在最前，是因为手柄状态要在本帧脚本与相机读取之前就绪；
`end_frame` 放在最后，是因为脚本在 `scripts.tick` 里读的 `pressed` / `released`
必须是本帧的值，清早了脚本就永远看不到边沿。暂停时两者都照跑——暂停只是冻结
时间推进与脚本/相机，输入采集不能停，否则恢复时状态是脏的。

**为什么 `update_world_transforms` 必须在最后**：脚本和物理改的都是 `transform`（局部），渲染读的是 `world`（世界）。如果顺序颠倒，画面会永远慢一帧——脚本改了位置，这一帧还画在旧位置上。

**暂停时的行为**：脚本、物理、相机输入、时间推进全部冻结，但**渲染继续**，窗口标题显示"已暂停"。这样暂停期间仍可拖动窗口、缩放、观察当前帧画面。

### 8.4 输入

输入分三层，职责单向依赖，设备层不认识脚本层：

```
core/input.rs   InputState / PadState / ButtonEdge   ← 纯数据 + 边沿语义，无 I/O
   ↑ 写
app/app.rs      winit 事件（键盘 / 鼠标）、gilrs 轮询（手柄）
   ↑ 读
script/api.rs   mouse_* / gamepad_* / pad_* 注入 Rhai
```

放 core 层的原因：脚本层要读输入，但脚本层不依赖窗口或设备 crate；
设备侧写入由 app 层完成，其余层只读。

| 输入 | 行为 |
| --- | --- |
| `W` / `A` / `S` / `D` | 相机沿前向/右向前后左右平移 |
| `↑` `↓` `←` `→` | 同 WASD |
| `Q` / `E` | 相机下降 / 上升 |
| 鼠标滚轮 | 沿视线推拉，半径钳制 0.8 ~ 80 |
| 鼠标左键拖拽 | 环绕转视角（`orbit`，绕 `camera.target`） |
| 鼠标右键拖拽 | 抓取式平移，位移随相机距离缩放 |
| 手柄左摇杆 | 相机水平移动，模拟量直接叠加（死区 0.15） |
| 手柄右摇杆 | 环绕转视角 |
| 手柄 `RT` / `LT` | 相机拉近 / 推远 |
| 手柄 `Start` / `Back` | 暂停切换 / 重载脚本 |
| `空格` | 暂停 / 继续 |
| `F5` | 手动重新载入脚本 |
| `Esc` | 退出 |

相机移动是**整体平移**：`camera.position` 与 `camera.target` 同加位移，因此朝向不变（自由飞行视角，不是环绕轨道）。`MOVE_SPEED = 6.0` 单位/秒。

按键状态用 `HashSet<Key>` 维护：按下插入、抬起移除，因此支持多键同时（斜着走更快会归一化）。手柄左摇杆走 `right * lx + forward * ly` 的模拟量路径，幅度由推杆深度决定，不参与归一化。

**边沿语义**是这块的核心约定：`ButtonEdge` 同时记录 `down`（持续）、`pressed`（刚按下）、`released`（刚松开），后两者只在状态翻转的那个逻辑帧内为 true，由帧末的 `InputState::end_frame` 清掉。脚本侧因此可以安全地区分"按住"与"按一下"，不会漏事件也不会重复触发。

**粘键防护**：窗口 `Focused(false)` 时立即 `keys.clear()` + `InputState::reset_held()`。
否则在窗口外松开鼠标，引擎永远不知道该键已抬起，相机会一直朝一个方向飞。

**手柄降级**：`gilrs::Gilrs::new()` 失败只是 `warn`，`gilrs` 字段为 `None`，鼠标键盘照常工作；运行中未连接手柄时 `pad.connected == false`，所有 `pad_*` API 返回 `false` / `0.0`，脚本无需分支判断设备是否存在。手柄热插拔由每帧轮询自然覆盖，标题栏显示连接状态与设备名。

`Space` 与 `Esc`、`F5` 在处理后立即从 `keys` 移除并 `return`，避免被当作持续按键重复触发。

### 8.5 文件监视与热重载

```rust
fn watch_files(&mut self) {
    if last_watch.elapsed() < 400ms { return; }   // 节流
    last_watch = now;

    // 优先级 1：场景文件
    if scene_mtime 变化 {
        读取 → scripts.rebuild_scene(json) → return
    }
    // 优先级 2：脚本文件
    scripts.reload_if_changed()
}
```

- **400ms 节流**是性能与手感的折中：`std::fs::metadata` 每帧调一次在 Windows 上代价不低（尤其是被杀软挂钩的文件）；400ms 意味着"保存后最多 0.4 秒生效"，手写脚本时几乎无感。
- 场景变化时**先返回**，不同一轮同时处理两个文件，避免"场景和脚本都改了"时重放两次、状态混乱。
- 都在 `RedrawRequested` 里调用，与渲染同线程，不需要锁。

### 8.6 终端 REPL

```rust
fn spawn_console() -> mpsc::Receiver<String> {
    // 独立线程（名 "r3e-console"）阻塞读 stdin，逐行 send 到 channel
}
```

主循环每帧 `try_recv()` 非阻塞取行，收到后：

1. **自动暂停**：若当前未暂停，置 `paused = true`（提示"已暂停脚本驱动，空格恢复"）。这是必要的——否则脚本每帧都在改物件，REPL 改完立刻被覆盖，用户会以为指令无效。
2. `scripts.eval(line)`，成功打印返回值（unit 打印 `ok`），失败打印 `执行失败: {e}`。

因此在窗口运行时，控制台可以直接敲：

```text
set_pos("ground", 0, -1, 0)
create_node("probe", "sphere", 2, 2, 0)
set_color("probe", 1, 0.2, 0.2)
camera(6, 4, 8, 0, 0, 0)
physics_backend()
```

---

## 9. 端到端数据流

```
   ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
   │ assets/scene.json│   │ scripts/*.rhai   │   │ 终端 stdin (REPL)│
   └────────┬─────────┘   └────────┬─────────┘   └────────┬─────────┘
            │ serde_json            │ Rhai AST              │ mpsc
            ▼                      ▼                       ▼
   ┌──────────────────────────────────────────────────────────────────┐
   │                         r3e-app 主循环                            │
   │ dt → scripts.tick → camera → sync_bindings → physics.step → world  │
   └───────────────────────────┬──────────────────────────────────────┘
                               │ Rc<RefCell<Scene>>
                               ▼
   ┌──────────────────────────────────────────────────────────────────┐
   │  r3e-core::Scene  ← 唯一真源                                      │
   │  nodes / order / name_index / camera / light / background         │
   └───────────────┬──────────────────────────────┬───────────────────┘
                   │ node.world 只读               │ Node::body 句柄
                   ▼                              ▼
   ┌──────────────────────────────┐   ┌──────────────────────────────┐
   │ r3e-render                   │   │ r3e-physics                  │
   │ 分组实例 → 4 条 draw_indexed  │   │ Null / Builtin（可关开关）     │
   └───────────────┬──────────────┘   └──────────────────────────────┘
                   │ wgpu
                   ▼
            ┌─────────────┐
            │  GPU / 窗口  │
            └─────────────┘
```

**三种控制通道的对比**

| 通道 | 表达能力 | 生效时机 | 持久性 | 适合场景 |
| --- | --- | --- | --- | --- |
| 场景 JSON | 静态结构：初始物件、材质色、相机、光照、背景 | 载入 / 改文件（≤0.4s 热重载） | 持久（文件即状态） | 搭场景、摆初始布局 |
| Rhai 脚本 | 完整逻辑：条件、循环、函数、时间驱动 | 每帧 `on_update`；改文件 ≤0.4s 热重载 | 持久（文件即行为） | 动画、交互、群体运动 |
| 终端 REPL | 任意一行表达式求值 | 立即（暂停脚本驱动后） | 不持久（重启丢失） | 调参数、临时验证、探针 |

三者的语义完全统一：都最终落到 `Scene` 的同一组方法上，因此**没有"脚本能做的 REPL 不能做"的割裂**。

---

## 10. 架构决策记录（ADR）

| 决策 | 选择 | 被否决的方案 | 理由 |
| --- | --- | --- | --- |
| 场景数据归属 | 单一 `Scene`，`Rc<RefCell<>>` 共享 | ECS + 消息传递；双缓冲快照 | 规模小、需求直接，单真源让"改了下帧就看见"零成本成立；ECS 会引入大量样板并让脚本 API 失去按名寻址的直观性 |
| 渲染策略 | 实例化 + 按网格合并 | 每物件一个 draw call | 4 种网格 → 最多 4 次 draw call，物件数量增长不增加 CPU 提交开销 |
| 实例数据更新 | 每帧全量重建并上传 | 增量更新 / 脏标记 | 场景是唯一真源，全量重建保证绝不出现"忘了同步"；当前规模下带宽不是瓶颈 |
| 几何来源 | 程序化生成 | GLTF 加载 | 引擎要"零资源即可运行"，也给物理碰撞体提供了确定的半尺寸语义 |
| 脚本引擎 | Rhai | Lua (mlua) / WASM | 纯 Rust 无 C 依赖，`cargo build` 即可，跨平台零配置 |
| 脚本命名 | `create_node` / `remove_node` | `spawn` / `despawn` | `spawn` 是 Rhai 保留字，被迫妥协并保持对称 |
| 物理 | trait + Null/Builtin 双实现 + 运行时开关 | 直接硬依赖 Rapier | 契约先钉死；内置实现只做「刚体 vs 地面」，够用且零依赖；换成 Rapier 时场景与脚本零改动 |
| 投影矩阵 | DirectX 风格（NDC 0..1） | OpenGL 风格（-1..1） | wgpu 的 NDC 定义如此，用错深度全错 |
| 色彩 | sRGB surface + 线性光照计算 | 手写 gamma 校正 | 让 surface 格式自动完成编码，着色器保持简单 |
| 热重载 | 重放基础场景 + 重跑脚本 | 增量应用脚本 diff | 脚本是命令式的，无法 diff；干净重建是唯一可靠做法 |
| 错误处理 | 库层返回 `String`，API 返回 `bool` | 全链路 `thiserror` | 脚本侧 `bool` 直接可判断，避免 Dynamic 错误类型膨胀 |

---

## 11. 已知边界与性能特征

### 11.1 能力边界（明确的"不做"）

| 维度 | 现状 | 影响 |
| --- | --- | --- |
| 几何 | 只有 4 种内置程序化网格 | 无法表达复杂模型；无 UV、无顶点色属性以外的顶点数据 |
| 材质 | 每物件一个 RGBA 纯色 | 无贴图、无金属度/粗糙度、无法线贴图 |
| 光照 | 1 方向光 + 环境光，Blinn-Phong | 无阴影、无点光、无全局光照 |
| 后处理 | 无 | 无 bloom / tonemapping / FXAA |
| 抗锯齿 | 无 MSAA | 几何边缘有锯齿 |
| 剔除 | 无视锥剔除、无遮挡剔除 | 屏幕外的物件照样上传和绘制（GPU 会做顶点裁剪，但 CPU 仍付代价） |
| 透明 | 开启 alpha blend 但**不排序** | 多个半透明物件互相遮挡时顺序可能错误 |
| 物理 | 默认关闭；开启后仅刚体 vs 地面平面 | 物件互不碰撞、无旋转动力学、无地形碰撞，物件不会堆叠或翻滚 |
| 动画 | 只有脚本每帧改 transform | 无骨骼动画、无关键帧插值、无动画混合 |
| 拾取 | 无 | 无射线检测、无鼠标点选物件 |
| 多相机 | 单相机 | 无分屏、无阴影相机、无后视镜 |

### 11.2 性能特征

- **Draw call**：≤ 4（实际使用的网格种类数），与物件数无关。
- **每帧 CPU 成本**：O(可见物件数)，主要是组装 `InstanceRaw`（80 B/物件）与一次 `write_buffer`。
- **上传带宽**：实例数 × 80 B/帧。1000 物件 ≈ 78 KB/帧，60 FPS 下约 4.7 MB/s，可忽略。
- **世界矩阵求解**：`update_world_transforms` 带记忆化缓存 + 深度上限，深层次结构下仍是 O(n)。
- **实测规模**：数千物件的群体动画（`example_swarm.rhai`）在 RTX 5060 Laptop / Vulkan 下运行正常，帧时间 1 ms 量级。
- **扩容策略**：实例缓冲按 2 倍增长（起始 512），避免每帧重建 buffer；只有跨越容量边界时才有一次分配。

### 11.3 可加固点（按收益排序）

1. **世界法线矩阵的奇异保护**：`Mat3::from_mat4(world).inverse()` 在 scale 含 0 时产 NaN。可在 `update_world_transforms` 里对 scale 做 `max(1e-6)` 保护，或直接复用 `Transform::normal_matrix` 的组合式算法。
2. **视锥剔除**：在 `prepare_instances` 里对 `node.world` 的中心点做 6 平面测试，可显著降低大场景的实例上传量。当前无包围盒，需先在 `MeshKind` 上定义 `aabb()`。
3. **半透明排序**：把 alpha < 1 的物件单独成组、按视深度排序后最后绘制。
4. **MSAA**：Pipeline 的 `multisample.count = 4` + resolve target，改动局限在渲染器内部。
5. **脏标记**：若物件规模上万，把"每帧全量上传"改为"版本号变化才上传"。

---

## 12. 扩展路线

按"改动范围 ÷ 用户价值"排序，建议的演进顺序：

| 阶段 | 内容 | 改动位置 | 破坏性 |
| --- | --- | --- | --- |
| 一 | Rapier 物理后端（替换内置实现） | 新增 `r3e-physics/src/rapier_backend.rs` + 接上 `PhysicsKind::Rapier` 分支 | 无（契约与开关已就位） |
| 二 | 纹理与 UV | `Vertex` 加 `uv`、`MeshData` 生成 UV、新增材质表与 bind group | 场景 JSON 向后兼容（新增可选字段） |
| 三 | 视锥剔除 + AABB | `mesh.rs` 加 `aabb()`，`prepare_instances` 加平面测试 | 无 |
| 四 | 阴影贴图 | 第二个 pass + 深度纹理 + 光照 VP 矩阵 | 着色器与 uniform 布局变化 |
| 五 | GLTF 加载 | 新增 `r3e-asset` crate，`MeshKind` 扩展为句柄 | **需改 `MeshKind` 语义**（枚举 → 资源句柄） |
| 六 | 编辑器协议 | 从 stdin REPL 升级为 TCP/WebSocket JSON-RPC | 无（复用同一套 API） |

其中**第五阶段是唯一的结构性改造**：一旦引入外部模型，`MeshKind` 这个"值类型枚举"必须变成"引用类型句柄"，渲染器的网格缓存键、物理碰撞体推导、场景 JSON 的 `mesh` 字段语义都要跟着改。在决定支持 GLTF 之前，`MeshKind` 枚举是刻意保持简单的。

---

## 13. 运行与验证

```powershell
cd D:\rust3d-engine

# 开发运行（默认 assets/scene.json + scripts/main.rhai）
cargo run

# 指定场景与脚本
cargo run -- assets/scene.json scripts/example_swarm.rhai

# 发布构建（无警告）
cargo build --release
.\target\release\r3e.exe

# 或一键
.\run.bat
```

**实机验证记录**（Release 构建）：

| 项 | 结果 |
| --- | --- |
| GPU 初始化 | RTX 5060 Laptop，Vulkan 后端，窗口正常出图 |
| 渲染 | 1920×1080 窗口，4 网格实例化绘制，无花屏/闪烁 |
| 脚本 | `on_start` / `on_update` 正常调度，`log` 输出可见 |
| 热重载 | 改 `scene.json` 与 `main.rhai` 后 ≤0.4s 生效，场景干净重建 |
| REPL | 输入指令后自动暂停脚本，指令立即反映到画面 |
| 物理 | 默认关：`attach_body` 返回 false、`body_count()` 为 0；`--physics on` 后 5 个刚体落地反弹并进入休眠 |
| 编译 | `cargo build --release` 零 warning |

---

## 14. 关键约束速查

写场景 / 脚本时最容易踩的十条：

1. 物件 `name` **全局唯一**，重名 `spawn` 直接失败。
2. 父物件必须**先于**子物件定义（JSON 里 `objects` 的顺序有意义）。
3. 脚本逻辑**必须写进 `on_start` / `on_update`**，顶层语句会被每帧重复执行。
4. 脚本函数名是 `create_node` / `remove_node`，不是 `spawn` / `despawn`。
5. 角度单位是**度**（`set_rot` / `orbit` / `fov`），但 `rotate_axis` 是弧度。
6. `set_light` 的 `direction` 是**光线行进方向**，要照亮地面就给负 Y 分量。
7. `color` 是**线性空间**值，不是 sRGB；写 0.5 不等于视觉上的中灰。
8. 相机 `aspect` 由渲染器每帧覆盖，写在 JSON 里无效。
9. 热重载会**重建整个场景**，脚本里的运行时累积状态（计数器、随机序列）会归零。
10. REPL 输入会自动暂停脚本驱动，要恢复动画按空格。

---

## 附：模块与文件对照表

| 文件 | 核心内容 |
| --- | --- |
| `crates/r3e-core/src/transform.rs` | `Transform`、欧拉角互转、`look_at`（含取逆）、`local_matrix`、`normal_matrix` |
| `crates/r3e-core/src/node.rs` | `NodeId`、`MeshKind`（含别名解析）、`Node` 字段与 `world_position` |
| `crates/r3e-core/src/scene.rs` | `ObjectDesc` / `PhysicsDesc` / `CameraDesc` / `LightDesc` / `SceneFile`、`Scene` 三索引、生命周期、`resolve_world`、`from_json_str` |
| `crates/r3e-core/src/camera.rs` | `Camera`、DirectX 风格投影、`orbit` |
| `crates/r3e-core/src/light.rs` | `Light` 默认值与语义 |
| `crates/r3e-render/src/mesh.rs` | `Vertex` / `InstanceRaw` 布局、`cube` / `sphere` / `plane` / `cylinder` |
| `crates/r3e-render/src/renderer.rs` | `Renderer` 初始化、`CameraUniform`、`prepare_instances`、`render`、`resize`、`RenderStats` |
| `crates/r3e-render/src/shader.wgsl` | Blinn-Phong 前向着色、伴随矩阵法线变换 |
| `crates/r3e-physics/src/lib.rs` | `PhysicsBackend` trait、`BodyDesc`、`NullBackend`、`BuiltinBackend`、`PhysicsConfig`、`Physics` 门面 |
| `crates/r3e-script/src/lib.rs` | `ScriptHost`、加载/每帧/热重载/重放、`has_fn` |
| `crates/r3e-script/src/api.rs` | 全部 Rhai API 绑定、`Clock`、`ScriptContext`、xorshift 随机 |
| `crates/r3e-app/src/app.rs` | `EngineApp`、帧循环、输入、文件监视、REPL 通道 |
| `crates/r3e-app/src/main.rs` | 参数解析、项目根三级兜底、事件循环启动 |
*（内容由AI生成，仅供参考）*
