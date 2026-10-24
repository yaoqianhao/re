# r3e —— Rust 3D 游戏引擎（文本可编程）

一个用 Rust 写的最小可用 3D 引擎内核：场景与行为全部由**文本**描述，
保存文件即**热重载**生效，画面里的物件可以实时移动、变形、生成、消失。

- 渲染：`wgpu` 30（DX12 / Vulkan / GLES 自动选择）
- 窗口：`winit` 0.30
- 数学：`glam`（wgpu 兼容的 NDC 深度 0..1 投影）
- 脚本：`rhai`（Rust 原生嵌入式脚本，语法接近 JS/Rust 混合体）
- 物理：内置最简实现（重力 / 地面碰撞 / 弹性 / 摩擦 / 休眠），**默认关闭**，可选 `null` 空转；
  契约固定，随时可换成 `rapier3d`

---

## 1. 目录结构

```
D:\rust3d-engine
├─ Cargo.toml                 # workspace 根
├─ .cargo/config.toml         # 国内镜像 + MinGW 链接器（本机 gnu 工具链）
├─ assets/
│   ├─ scene.json             # 场景描述（纯文本，可随时改）
│   ├─ scene_physics.json     # 物理演示场景（已启用物理）
│   └─ scene_shadow.json      # 阴影演示场景（地面 + 柱/方块/球/圆柱）
├─ scripts/
│   ├─ main.rhai              # 行为脚本（纯文本，保存即热重载）
│   ├─ input_demo.rhai        # 鼠标 / 手柄演示
│   ├─ physics_demo.rhai      # 物理演示（掉落 + 弹跳 + 运行时开关）
│   ├─ anim_demo.rhai         # 动画轮播（每 3 秒带 0.35s 交叉淡入切片段）
│   ├─ anim_state.rhai        # 动画状态机（待机 / 跑 / 攻击，均为淡入切换）
│   └─ shadow_demo.rhai       # 阴影演示（光源扫动 + 强度呼吸 + 右键开关）
├─ crates/
│   ├─ r3e-core/              # 数学 / 节点 / 场景 / 相机 / 光照
│   ├─ r3e-render/            # wgpu 渲染器、程序化网格、WGSL 着色器
│   ├─ r3e-physics/           # 物理（PhysicsBackend trait + Null / Builtin 实现 + 开关）
│   ├─ r3e-script/            # Rhai 引擎、API 绑定、热重载宿主
│   └─ r3e-app/               # 主循环、窗口事件、终端指令
└─ docs/
    ├─ ARCHITECTURE.md        # 架构总览：crate 边界、数据流、扩展路线
    ├─ PHYSICS.md             # 物理子系统：三层开关、场景字段、脚本 API、内置规则
    └─ ASSETS.md              # FBX 资产加载（P0–P4 已落地，含骨骼蒙皮）
```

各 crate 单向依赖：`app → render / script / physics → core`，核心层不认识 wgpu。

---

## 2. 构建与运行

本机已配置 MinGW-w64 + `stable-x86_64-pc-windows-gnu` 工具链：

```powershell
cd D:\rust3d-engine
cargo run --release
```

或指定自定义场景 / 脚本：

```powershell
cargo run --release -- assets\scene.json scripts\main.rhai
```

编译产物：`D:\rust3d-engine\target\x86_64-pc-windows-gnu\debug\r3e.exe`（或 `release\r3e.exe`；
`.cargo/config.toml` 指定了 `x86_64-pc-windows-gnu` 目标，故产物带三元组目录），
双击 exe 时会自动向上查找 `assets` 目录。

### 运行时操作

| 输入 | 作用 |
| --- | --- |
| `W A S D` / 方向键 | 水平移动相机 |
| `Q` / `E` | 相机升降 |
| 鼠标滚轮 | 推拉相机 |
| 鼠标左键拖拽 | 环绕转视角 |
| 鼠标右键拖拽 | 平移视角 |
| 手柄左摇杆 | 水平移动相机（模拟量，含死区） |
| 手柄右摇杆 | 环绕转视角 |
| 手柄 `RT` / `LT` | 相机拉近 / 推远 |
| 手柄 `Start` | 暂停 / 继续 |
| 手柄 `Back` | 重载脚本（等同 `F5`） |
| `空格` | 暂停 / 继续（暂停时脚本不推进） |
| `F5` | 手动重载脚本 |
| `Esc` | 退出 |
| 终端输入一行代码 | 立即执行一行 Rhai，作用于当前场景 |

手柄由 `gilrs` 轮询（每帧排空事件队列后取状态快照），支持 Xbox / PS / Switch 布局，
按键与轴已归一化到统一的 16 键 + 8 轴编号（见 `crates/r3e-core/src/input.rs`）。
窗口标题会显示当前手柄的连接状态与设备名；窗口失焦时自动清空所有持续按下状态，避免粘键。

---

## 3. 三种文本控制通道

### 3.1 场景文件 `assets/scene.json`

静态描述初始世界。字段全部可选，缺省有默认值。

```json
{
  "name": "demo",
  "background": [0.04, 0.05, 0.07, 1.0],
  "camera": { "position": [7, 5, 9], "target": [0, 1, 0], "fov": 55 },
  "light": {
    "direction": [-0.45, -0.9, -0.35],
    "color": [1.0, 0.96, 0.9],
    "intensity": 1.15,
    "ambient": [0.16, 0.18, 0.23]
  },
  "objects": [
    {
      "name": "spinner",
      "mesh": "cube",                // cube | sphere | plane | cylinder
      "position": [0, 1, 0],
      "rotation": [0, 0, 0],         // 欧拉角，角度制
      "scale": [1.6, 1.6, 1.6],
      "color": [0.9, 0.45, 0.25, 1.0],
      "visible": true,
      "physics": { "body": "static", "collider": "box", "mass": 1.0 }
    }
  ]
}
```

`physics` 段在物理开启时真正参与模拟（默认关闭，见第 5 章）。只有声明了该段的物件才会被自动绑成刚体。

### 3.2 行为脚本 `scripts/main.rhai`

脚本拥有两个约定钩子：

```rhai
fn on_start() {              // 载入 / 热重载时执行一次
    log("boot");
}

fn on_update(dt) {           // 每帧执行，dt 为秒
    rotate("spinner", 0.0, 70.0 * dt, 0.0);
}
```

**脚本顶层语句会被 Rhai 在每次 `on_update` 前重复执行**（引擎载入时会给出提示），
因此所有逻辑与常量都应写进 `on_start` / `on_update` 内部。

文件保存后 400 ms 内自动检测到 mtime 变化 → 重放场景 → 重新执行脚本，
无需重启进程，也无需重新编译 Rust。

### 3.3 终端实时指令

引擎进程的控制台里直接输入一行 Rhai：

```
[repl] > create_node("box_1", "cube", 2.0, 1.0, 0.0)
ok
[repl] > set_scale_uniform("box_1", 3.0)
ok
[repl] > get_pos("box_1")
[2.0, 1.0, 0.0]
```

`let` 定义的变量保存在脚本作用域里，可跨行复用。

---

## 4. 脚本 API

### 生成 / 消失

| 函数 | 说明 |
| --- | --- |
| `create_node(name, mesh, x, y, z) -> bool` | 生成物件，`mesh` ∈ `cube/sphere/plane/cylinder` |
| `create_node_at_origin(name, mesh) -> bool` | 原点生成 |
| `create_node_json(json_text) -> bool` | 用 JSON 描述生成（字段同场景文件） |
| `remove_node(name) -> bool` | 销毁单个物件 |
| `remove_nodes(prefix) -> int` | 按前缀批量销毁，返回数量 |
| `exists(name) -> bool` | 是否存在 |
| `count() -> int` | 物件总数 |
| `names(prefix) -> array` | 按前缀列出名字 |

### 移动 / 变形

| 函数 | 说明 |
| --- | --- |
| `set_pos(name, x, y, z) -> bool` | 设置位置 |
| `move_by(name, dx, dy, dz) -> bool` | 相对位移 |
| `get_pos(name) -> array` | 读取位置 |
| `set_rot(name, rx, ry, rz) -> bool` | 设置欧拉角（角度制） |
| `rotate(name, dx, dy, dz) -> bool` | 欧拉角增量 |
| `set_scale(name, sx, sy, sz) -> bool` | 三轴缩放（变形） |
| `set_scale_uniform(name, s) -> bool` | 等比缩放 |
| `set_mesh(name, "sphere") -> bool` | 运行时换网格 |
| `set_color(name, r, g, b) -> bool` | 改颜色 |
| `set_color4(name, r, g, b, a) -> bool` | 改颜色（带透明度） |
| `set_visible(name, bool) -> bool` | 显隐 |

### 相机 / 光照 / 环境

| 函数 | 说明 |
| --- | --- |
| `camera(px, py, pz, tx, ty, tz)` | 直接摆位 |
| `orbit(yaw_deg, pitch_deg)` | 绕目标点旋转 |
| `fov(deg)` | 视场角 |
| `set_light(dx, dy, dz)` | 平行光方向 |
| `set_light_color(r, g, b, intensity)` | 光色与强度 |
| `set_ambient(r, g, b)` | 环境光 |
| `set_background(r, g, b)` | 背景色 |
| `set_shadow(on)` / `shadow_enabled()` | 阴影开关（关后整帧跳过阴影通道） |
| `set_shadow_strength(v)` | 阴影强度 0..1 |
| `set_shadow_size(px)` | 阴影贴图边长（64..8192，改值重建纹理） |
| `set_shadow_extent(half)` | 正交半宽；0 = 按场景包围球自适应 |

### 时间 / 随机 / 输出

| 函数 | 说明 |
| --- | --- |
| `time() -> f64` | 启动至今秒数 |
| `dt() -> f64` | 上一帧时长 |
| `frame() -> int` | 帧序号 |
| `rand(min, max) -> f64` | 均匀随机 |
| `log(text)` | 输出到控制台 |

### 输入：鼠标 / 手柄

按键名不区分大小写，也可直接传索引（`0` = 左键 / `A` 键）。

| 函数 | 说明 |
| --- | --- |
| `mouse_x() / mouse_y() -> f64` | 光标位置（窗口像素，左上角原点） |
| `mouse_pos() -> array` | `[x, y]` |
| `mouse_dx() / mouse_dy() -> f64` | 本帧光标位移 |
| `mouse_moved() -> bool` | 本帧是否移动过 |
| `scroll() -> f64` | 本帧滚轮行数（向上为正） |
| `mouse_down(btn) -> bool` | 持续按下 |
| `mouse_pressed(btn) -> bool` | 本帧刚按下（边沿） |
| `mouse_released(btn) -> bool` | 本帧刚松开（边沿） |
| `gamepad_connected() -> bool` | 是否已连接手柄 |
| `gamepad_name() -> string` | 设备名 |
| `pad_down(btn) -> bool` | 按键持续按下 |
| `pad_pressed(btn) -> bool` | 本帧刚按下（边沿） |
| `pad_released(btn) -> bool` | 本帧刚松开（边沿） |
| `pad_axis(axis) -> f64` | 轴值，摇杆 −1.0~1.0，扳机 0.0~1.0（未做死区） |

鼠标按键名：`left`(`lmb`/`mouse1`) / `right`(`rmb`/`mouse2`) / `middle`(`mmb`/`mouse3`) / `back`(`mouse4`) / `forward`(`mouse5`)。

手柄按键名：`a` `b` `x` `y` `lb` `rb` `back`(`select`/`share`) `start`(`options`) `guide`(`home`)
`ls`(`l3`) `rs`(`r3`) `up` `down` `left` `right`；PS 风格可用 `cross` `circle` `square` `triangle` `l1` `r1`。

手柄轴名：`left_x`(`lx`) `left_y`(`ly`) `right_x`(`rx`) `right_y`(`ry`) `lt`(`l2`) `rt`(`r2`)。

边沿语义：`pressed` / `released` 只在状态翻转的那一帧为 true，帧末由引擎清空；
所以判定"按一下"请用 `*_pressed`，判定"按住"请用 `*_down`。
示例见 `scripts/input_demo.rhai`（左键生成方块、右键清空、摇杆驱动探针）。

### 物理

| 函数 | 说明 |
| --- | --- |
| `attach_body(name, kind, mass) -> bool` | `kind` ∈ `dynamic/static/kinematic`；物理关闭时返回 `false` |
| `detach_body(name) -> bool` | 销毁刚体 |
| `body_impulse(name, x, y, z) -> bool` | 施加冲量，会唤醒休眠体 |
| `body_count() -> int` | 刚体数量（物理关闭时恒为 0） |
| `physics_enabled() -> bool` | 物理是否开启 |
| `physics_set_enabled(on) -> bool` | 开关物理，返回操作后的状态 |
| `physics_backend() -> string` | 当前后端名：`null` / `builtin` / `rapier` |
| `physics_set_backend(name) -> bool` | 运行时换后端（会清空已有刚体） |
| `physics_gravity() -> f64` | 重力 y 分量 |
| `physics_set_gravity(x, y, z)` | 设置重力向量 |
| `physics_ground() -> f64` | 地面平面高度 |
| `physics_set_ground(y)` | 设置地面平面高度 |

---

## 5. 物理系统

**默认关闭**，行为与"没有物理"完全一致。三种方式可开，优先级：**命令行 > 场景文件 > 内置默认**。

```powershell
# 1) 命令行（最高优先级，粘住热重载）
cargo run --release -- --physics on
cargo run --release -- --physics on --gravity 0,-3,0 --ground 0.5

# 2) 场景文件：assets/scene_physics.json 里已启用
cargo run --release -- assets\scene_physics.json scripts\physics_demo.rhai

# 3) 脚本运行时
#    physics_set_enabled(true);  physics_set_backend("builtin");
```

命令行参数：

| 参数 | 取值 | 说明 |
| --- | --- | --- |
| `--physics` | `on` / `off` | 物理总开关，缺省沿用场景文件，都没有则关闭 |
| `--physics-backend` | `builtin` / `null` / `rapier` | 后端；`rapier` 未编译进本构建，选它会提示并拒绝 |
| `--gravity` | `x,y,z` | 重力加速度，默认 `0,-9.81,0` |
| `--ground` | 浮点数 | 地面平面高度，默认 `0` |

场景文件里的写法（`physics` 段可整体省略）：

```json
{
  "physics": { "enabled": true, "backend": "builtin", "gravity": [0, -9.81, 0], "ground": 0.0 },
  "objects": [
    { "name": "ball", "mesh": "sphere", "position": [-3, 6, 0],
      "physics": { "body": "dynamic", "mass": 1.0, "restitution": 0.85, "friction": 0.3 } }
  ]
}
```

**只有声明了 `physics` 段的物件才会被自动绑成刚体**；没写的（纯装饰）始终由脚本驱动。

内置后端只做**刚体 vs 无限地面平面**：重力积分 + 弹性反弹 + 摩擦 + 休眠，纯自实现零依赖。
**物件之间不碰撞、无旋转动力学、无地形碰撞**——需要这些就换成 Rapier（见下）。

完整规则、字段表、实测数据见 [`docs/PHYSICS.md`](docs/PHYSICS.md)。

### 换成 Rapier

契约（`PhysicsBackend` 的 8 个必需方法）与开关层已经就位，改动面固定三处：

1. 新增 `crates/r3e-physics/src/rapier_backend.rs` 实现 `PhysicsBackend`；
2. `crates/r3e-physics/Cargo.toml` 打开 `rapier = ["dep:rapier3d"]` feature；
3. `Physics::rebuild_backend` 里把 `PhysicsKind::Rapier` 指向新实例。

场景 JSON、脚本 API、`app` 层、`core::Scene` **零改动**；
之后 `--physics-backend rapier` 即可切换，随时切回 `builtin`。

---

## 6. 用 D:\ai_ide 编辑

引擎刻意把"可编程面"做成了普通文本文件，任何编辑器都能接管：

1. 在 `D:\ai_ide` 里 `File → Open Folder → D:\rust3d-engine`；
2. 打开 `scripts\main.rhai` 或 `assets\scene.json`，改完保存；
3. 引擎窗口内 0.4 秒后自动生效（场景文件改动需重启或按 F5 重载脚本）。

如果想在 IDE 里直接跑：终端执行 `cargo run --release`，工作目录设为 `D:\rust3d-engine`。

---

## 7. 已知边界

- 单方向光 + 环境光，Blinn-Phong 着色；方向光带单张正交阴影贴图（3×3 PCF，默认开启），无后处理。
  阴影限制：无点光/聚光阴影、无级联、透明物件也投实心影。
- 网格除四种程序化网格外，可加载外部 `.fbx`（含 base_color 贴图、材质因子、节点级 TRS 动画、
  骨骼蒙皮、交叉淡入与脚本状态机）；法线/粗糙度贴图、分层/叠加混合、morph target 尚未支持。
  用法与阶段现状见 [`docs/ASSETS.md`](docs/ASSETS.md)。
- 每帧重建实例缓冲，物件量级到数千仍可运行；上万需改成持久化实例缓冲。
- 脚本单线程同步执行，长循环会卡帧（Rhai 已限制操作数上限）。
- 物理：物件互不碰撞、无旋转、无地形碰撞、高速可能穿地（无 CCD）；默认关闭。

---

## 8. FBX 资产加载（P0–P4 已完成）

已落地：静态网格导入与 GPU 上传、材质分区 + base_color 贴图、场景 `assets` 段声明与 spawn、
骨架节点树 + 节点级动画（多片段、速度、循环、热切换、场景热重载后自动补挂）、
骨骼蒙皮（`skin_deformers` → 关节绑定 → `group(2)` 关节矩阵 + 顶点着色器 LBS，逐顶点形变）、
交叉淡入（`set_anim_clip_fade`，smoothstep 双片段加权，位置/缩放 lerp + 旋转 slerp）与脚本层状态机。
尚未做：分层 mask / additive 混合、morph target、DQS、GPU skinning。

```bash
# 临时加载一个 FBX 到默认场景（窗口里直接看造型与贴图）
cargo run -- --asset D:/models/robot.fbx
# 只看解析统计，不进窗口
cargo run -- --dump-asset D:/models/robot.fbx
# 列出 FBX 内所有动画栈、时长与轨道数
cargo run -- --dump-anim D:/models/robot.fbx
# 指定启动片段，并显示骨骼标记方块
cargo run -- --scene assets/scene_anim.json --anim-clip run_loop --show-bones
```

长期用法写在场景里（相对路径按场景文件目录解析，可用 `--assets-root` 改基准目录）：

```json
"assets": [
  { "path": "models/robot.fbx", "position": [0, 0, 0], "scale": [1, 1, 1] }
]
```

资产相关的两个小开关：

| 参数 | 说明 |
| --- | --- |
| `--assets-root DIR` | 场景 `assets` 段里**相对路径**的解析根。不指定时按场景文件所在目录（历史行为）；指定后场景文件可以随手挪位置，资产集中放在一个公共目录 |
| 改场景文件即热重载 | 场景 `assets` 段被改动（增删资产、改 transform / anim 参数）会在 ≤0.4s 内**重新导入并换库**；只改物件、相机、光照时不会重导资产（不打断播放进度与物理体） |

完整方案（选型、crate 落点、数据结构、阶段拆分、踩坑口径与验收）见
[`docs/ASSETS.md`](docs/ASSETS.md)，要点：

- 解析器选 **ufbx**（单文件 C 库，静态链接，本机已用 4 个样本实测）；
- 新增 `r3e-asset` crate，`Vertex` / `MeshData` 下沉到 `r3e-core`，render 只认 `MeshProvider` trait；
- `MeshRef` 句柄化（`asset:<路径>#<网格序号>`）+ 材质分区逐段 `draw_indexed`；
- 贴图 `filename` → 内嵌 `content` → 白纹理三级回退，缺图不阻断渲染；
- 动画按节点级动画栈（TRS 曲线采样）实现（P3）；蒙皮由 `SkinInstance` 提供关节矩阵、
  `shader.wgsl` 做 LBS（P4），顶点 `stride 56`、实例 `stride 96`（含 `joint_base`）；
- 交叉淡入（阶段三 C）由 `AnimInstance.fade` 双片段加权混合完成，仍是同一套骨架与关节矩阵，
  渲染层零改动；状态机放脚本层，`scripts/anim_state.rhai` 是可直接跑的示例：

```bash
# 待机 / 跑 / 攻击三态，切换全部带淡入（按住左键跑、点击左键攻击）
cargo run -- assets/scene_anim.json scripts/anim_state.rhai
```

---

## 9. 方向光阴影（阶段三 D 已完成）

一个方向光 + 一张正交深度图（`Depth32Float`）+ 第二个 render pass。默认开启，2048²，
正交范围按场景包围球自适应；3×3 PCF 软化边缘，法线偏移抑制自阴影条纹，强度可调。

```bash
# 阴影演示：光源绕 Y 轴扫动、强度呼吸、右键开关阴影
cargo run -- assets/scene_shadow.json scripts/shadow_demo.rhai
```

场景里写死（`light` 段下的 `shadow` 对象，缺省即默认值）：

```json
"light": {
  "direction": [-0.55, -0.62, -0.35],
  "intensity": 1.25,
  "shadow": { "enabled": true, "size": 2048, "strength": 1.0 }
}
```

脚本侧四个开关：`set_shadow(on)` / `shadow_enabled()` / `set_shadow_strength(v)` /
`set_shadow_size(px)` / `set_shadow_extent(half)`（半宽写 0 = 自适应）。

验证手段：`--run-seconds N` 的起止探针会打出
`阴影通道：draw call X / 主通道实例 Y`，关闭阴影时为 0——这是「阴影通道真的跑了」的数值证据。
设计细节与踩坑（独立 pipeline layout、深度图不可原地 resize）见
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) 5.7 与 [`docs/ASSETS.md`](docs/ASSETS.md) 17。
