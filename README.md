# r3e —— Rust 3D 游戏引擎（文本可编程）

一个用 Rust 写的最小可用 3D 引擎内核：场景与行为全部由**文本**描述，
保存文件即**热重载**生效，画面里的物件可以实时移动、变形、生成、消失。

- 渲染：`wgpu` 30（DX12 / Vulkan / GLES 自动选择）
- 窗口：`winit` 0.30
- 数学：`glam`（wgpu 兼容的 NDC 深度 0..1 投影）
- 脚本：`rhai`（Rust 原生嵌入式脚本，语法接近 JS/Rust 混合体）
- 物理：**已预留**抽象层，默认 `null` 空后端，接入 `rapier3d` 只需实现一个 trait

---

## 1. 目录结构

```
D:\rust3d-engine
├─ Cargo.toml                 # workspace 根
├─ .cargo/config.toml         # 国内镜像 + MinGW 链接器（本机 gnu 工具链）
├─ assets/
│   └─ scene.json             # 场景描述（纯文本，可随时改）
├─ scripts/
│   └─ main.rhai              # 行为脚本（纯文本，保存即热重载）
└─ crates/
    ├─ r3e-core/              # 数学 / 节点 / 场景 / 相机 / 光照
    ├─ r3e-render/            # wgpu 渲染器、程序化网格、WGSL 着色器
    ├─ r3e-physics/           # 物理抽象（PhysicsBackend trait + NullBackend）
    ├─ r3e-script/            # Rhai 引擎、API 绑定、热重载宿主
    └─ r3e-app/               # 主循环、窗口事件、终端指令
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

编译产物：`D:\rust3d-engine\target\debug\r3e.exe`（或 `release\r3e.exe`），
双击 exe 时会自动向上查找 `assets` 目录。

### 运行时操作

| 输入 | 作用 |
| --- | --- |
| `W A S D` / 方向键 | 水平移动相机 |
| `Q` / `E` | 相机升降 |
| 鼠标滚轮 | 推拉相机 |
| `空格` | 暂停 / 继续（暂停时脚本不推进） |
| `F5` | 手动重载脚本 |
| `Esc` | 退出 |
| 终端输入一行代码 | 立即执行一行 Rhai，作用于当前场景 |

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

`physics` 段当前只做登记（`null` 后端），不影响画面，接入真实后端后立即参与模拟。

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

### 时间 / 随机 / 输出

| 函数 | 说明 |
| --- | --- |
| `time() -> f64` | 启动至今秒数 |
| `dt() -> f64` | 上一帧时长 |
| `frame() -> int` | 帧序号 |
| `rand(min, max) -> f64` | 均匀随机 |
| `log(text)` | 输出到控制台 |

### 物理（预留）

| 函数 | 说明 |
| --- | --- |
| `attach_body(name, kind, mass) -> bool` | `kind` ∈ `dynamic/static/kinematic` |
| `body_count() -> int` | 刚体数量 |
| `physics_backend() -> string` | 当前后端名（默认 `null`） |

---

## 5. 物理引擎的预留方式

`crates/r3e-physics/src/lib.rs`：

```rust
pub trait PhysicsBackend {
    fn name(&self) -> &str;
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;
    fn destroy_body(&mut self, handle: BodyHandle);
    fn step(&mut self, dt: f32, scene: &mut Scene);
    fn set_gravity(&mut self, gravity: Vec3);
    fn body_count(&self) -> usize;
}
```

`Physics` 是门面，主循环每帧调用 `physics.step(dt, &mut scene)`；
`NullBackend` 只记录刚体、不改场景，所以**现在完全零开销**。

接入真实物理只需两步：

1. 在 `crates/r3e-physics/Cargo.toml` 打开 `rapier` feature（已写好条件依赖）：
   ```toml
   rapier = ["dep:rapier3d"]
   ```
2. 新增 `RapierBackend` 实现 `PhysicsBackend`，在 `step()` 里把刚体位置回写到 `Scene`：

```rust
for (handle, body) in &self.bodies {
    scene.set_world_position(handle.node, body.position());
}
```

上层（脚本 / 场景 JSON / 渲染）一行都不用改。

---

## 6. 用 D:\ai_ide 编辑

引擎刻意把"可编程面"做成了普通文本文件，任何编辑器都能接管：

1. 在 `D:\ai_ide` 里 `File → Open Folder → D:\rust3d-engine`；
2. 打开 `scripts\main.rhai` 或 `assets\scene.json`，改完保存；
3. 引擎窗口内 0.4 秒后自动生效（场景文件改动需重启或按 F5 重载脚本）。

如果想在 IDE 里直接跑：终端执行 `cargo run --release`，工作目录设为 `D:\rust3d-engine`。

---

## 7. 已知边界

- 单方向光 + 环境光，Blinn-Phong 着色；无阴影、无后处理、无 PBR 贴图。
- 网格为程序化生成（立方体 / 球 / 平面 / 圆柱），未接 glTF 导入。
- 每帧重建实例缓冲，物件量级到数千仍可运行；上万需改成持久化实例缓冲。
- 脚本单线程同步执行，长循环会卡帧（Rhai 已限制操作数上限）。
