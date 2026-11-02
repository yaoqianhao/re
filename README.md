---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_809d28cbd4fb11f19fe5525400638852
    ReservedCode1: U+pj9KT2xscmIHCzAuSN9AmjPyy/B7/Bu1oAJj+qxnIsjkWrhuTu5hRLupQsdazDkP7TA8yHWUyveuVb0t22NZXZU9FWVXnnNlVfudtWdsicm056lgmaBP0zKP36AaHZbS7IPn9bzaJvc6i//q/yhFh2FMvumOpPo6vPdfTcWIjH9bcvX6l1BW3SXqc=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_809d28cbd4fb11f19fe5525400638852
    ReservedCode2: U+pj9KT2xscmIHCzAuSN9AmjPyy/B7/Bu1oAJj+qxnIsjkWrhuTu5hRLupQsdazDkP7TA8yHWUyveuVb0t22NZXZU9FWVXnnNlVfudtWdsicm056lgmaBP0zKP36AaHZbS7IPn9bzaJvc6i//q/yhFh2FMvumOpPo6vPdfTcWIjH9bcvX6l1BW3SXqc=
---



# r3e —— Rust 3D 游戏引擎（文本可编程）

一个用 Rust 写的最小可用 3D 引擎内核：场景与行为全部由**文本**描述，
保存文件即**热重载**生效，画面里的物件可以实时移动、变形、生成、消失。

- 渲染：`wgpu` 30（DX12 / Vulkan / GLES 自动选择）
- 窗口：`winit` 0.30
- 数学：`glam`（wgpu 兼容的 NDC 深度 0..1 投影）
- 脚本：`rhai`（Rust 原生嵌入式脚本，语法接近 JS/Rust 混合体）
- 物理：内置最简实现（重力 / 地面碰撞 / 弹性 / 摩擦 / 休眠），**默认关闭**，可选 `null` 空转；
  契约固定，随时可换成 `rapier3d`；查询层 `raycast` / `overlap` / `shape_cast` 后端无关

---

## 1. 目录结构

```
D:\rust3d-engine
├─ Cargo.toml                 # workspace 根
├─ .cargo/config.toml         # 国内镜像 + MinGW 链接器（本机 gnu 工具链）
├─ assets/
│   ├─ scene.json             # 场景描述（纯文本，可随时改）
│   ├─ scene_physics.json     # 物理演示场景（已启用物理）
│   ├─ scene_rapier.json      # Rapier 演示场景（堆叠 / 弹跳 / 圆柱）
│   ├─ scene_ccd.json         # CCD 穿透对照场景（薄墙 + 两颗只差 ccd 一位的子弹）
│   ├─ scene_shadow.json      # 阴影演示场景（地面 + 柱/方块/球/圆柱）
│   ├─ scene_ibl.json         # IBL 演示场景（三色天空 + 三档金属球 + 亮面/哑光介电）
│   ├─ audio/
│   │   ├─ blip.wav           # 0.18s 短音效（880Hz 指数衰减，脚本合成，无版权素材）
│   │   └─ ambience.wav       # 2.0s 可无缝循环环境音（220Hz + 330Hz，脚本合成）
│   └─ ui/icon.png            # UI 演示图标（右上角 HUD，可换任意 png）
├─ scripts/
│   ├─ main.rhai              # 行为脚本（纯文本，保存即热重载）
│   ├─ input_demo.rhai        # 键盘 / 鼠标 / 手柄演示
│   ├─ physics_demo.rhai      # 物理演示（掉落 + 弹跳 + 运行时开关）
│   ├─ rapier_demo.rhai       # Rapier 演示（持续丢方块堆叠 + 碰撞事件）
│   ├─ ccd_demo.rhai          # CCD 演示（开/关 CCD 两球射同一面薄墙）
│   ├─ query_demo.rhai        # 物理查询演示（raycast 射线 + overlap 球体圈选 + 球/盒扫掠）
│   ├─ anim_demo.rhai         # 动画轮播（每 3 秒带 0.35s 交叉淡入切片段）
│   ├─ anim_state.rhai        # 动画状态机（待机 / 跑 / 攻击，均为淡入切换）
│   └─ shadow_demo.rhai       # 阴影演示（光源扫动 + 强度呼吸 + 右键开关）
├─ crates/
│   ├─ r3e-core/              # 数学 / 节点 / 场景 / 相机 / 光照
│   ├─ r3e-render/            # wgpu 渲染器、程序化网格、WGSL 着色器
│   ├─ r3e-physics/           # 物理（PhysicsBackend trait + Null / Builtin 实现 + 开关）
│   ├─ r3e-script/            # Rhai 引擎、API 绑定、热重载宿主
│   ├─ r3e-audio/             # 非空间化音频（rodio 后端 + 静默降级 + 混音门面）
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

> **工具链版本**：workspace 声明 `rust-version = 1.79`，但 `crates/r3e-audio` 依赖的 `rodio 0.23`
> 要求 rustc ≥ 1.87，该 crate 单独声明了 `rust-version = 1.87`。因此**编整个 workspace 需要 ≥ 1.87**
> （本机实测 `rustc 1.99.1`）；只有 1.79–1.86 的工具链会在 `r3e-audio` 处报 MSRV 错误。

```powershell
cd D:\rust3d-engine
cargo run --release
```

或指定自定义场景 / 脚本：

```powershell
cargo run --release -- assets\scene.json scripts\main.rhai
```

> workspace 里有**两个 bin**：主程序 `r3e`（`crates/r3e-app`）与 IDE 插件 `r3e-plugin`（`crates/r3e-plugin`）。
> `crates/r3e-app/Cargo.toml` 已声明 `default-run = "r3e"`，所以在仓库根目录直接 `cargo run` / `cargo run --release`
> 启动的就是主程序，不需要再加 `--bin`；要单独跑插件时显式指定包：`cargo run -p r3e-plugin -- --help`。

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
| `F2` | 把内存中的场景保存回场景 JSON（也可用 `--save-scene` 无窗口规范化保存） |
| `F5` | 手动重载脚本 |
| `J` | 播放一次短音效（`audio_play("blip", …)`，连按只重启同一实例，不叠加） |
| `K` | 暂停 / 继续环境音（暂停保留播放位置；没在播则重新开始） |
| `L` | 停止 / 重新开始环境音 |
| `M` | 总音量在 `0.8` / `0.2` 之间切换 |
| `Esc` | 退出 |
| 终端输入一行代码 | 立即执行一行 Rhai，作用于当前场景 |

键盘事件由 winit 的 `KeyboardInput` 送入引擎自有的 `KeyCode`（98 键，含字母、数字、F1~F12、
方向键、导航键、左右修饰键与小键盘），状态区分「持续按住」与「本帧边沿」，边沿在每帧末清空；
系统自动重复只更新按住状态，不会重复触发快捷键。

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

### 3.4 编辑器 RPC 通道（TCP + JSON-RPC 2.0）

`r3e-editor` 定义协议与 `EngineApi` trait，`r3e-app` 里的 `LocalEngineApi` 是它的**唯一实现者**，
直接桥接同一份内存里的 `Scene` / `Physics` / `ScriptHost`，再挂到主循环上——所以外部编辑器改的是画面里
真正的那份状态，不存在"消息副本"和"事件同步"。

```powershell
# 开局即开端口；token 可省略（省略则不校验）
cargo run --release -- --rpc-port 7878 --rpc-token secret --rpc-max-per-frame 4
```

| 选项 | 默认 | 说明 |
| --- | --- | --- |
| `--rpc-port <port>` | 关闭 | 绑定 `127.0.0.1:<port>` 并接受外部连接；端口占用只警告、不阻断引擎启动 |
| `--rpc-token <tok>` | 无 | 每条请求的 `params.token` 都要带对，否则该条回 `-32001`（连接保留，可继续发） |
| `--rpc-max-per-frame <n>` | `8` | 每帧最多消化多少条请求，防止编辑器刷屏把帧时间顶爆 |

协议：一行一个 JSON-RPC 2.0 请求 / 响应（NDJSON）。方法名大小写与下划线不敏感
（`scene.setTransform` = `scene.set_transform`），共 12 个：

| 方法 | 作用 |
| --- | --- |
| `ping` | 探活，回 `pong` + 协议版本 |
| `engine.info` | 场景/脚本路径、帧号、FPS、运行时长、暂停状态、物件数 |
| `scene.list` | 列物件，支持 `filter`（名字子串）与 `limit` |
| `scene.get` | 单个物件的完整快照（局部 TRS、世界矩阵、父子、颜色、可见性） |
| `scene.set_transform` | 改 `position` / `rotation` / `scale`，三者至少给一个 |
| `scene.set_material` | 改物件的 PBR 材质（`metallic` / `roughness` / `emissive`，可分别给），回收敛后的完整材质 + 节点快照 |
| `scene.spawn` | 按物件描述新建，回新节点 id |
| `scene.remove` | 按名字删除 |
| `scene.set_environment` | 改场景环境探针（`sky_zenith` / `sky_horizon` / `ground` / `sun_dir` / `sun_color` / `sun_intensity` / `intensity`，可分别给；`enabled: false` 清空），回收敛后的 `environment` |
| `scene.save` | 把内存场景写回 JSON（`path` 省略时为原场景文件），回 `{path, objects, bytes}` |
| `script.eval` | 执行一段 Rhai，返回其求值结果（JSON 化） |
| `script.reload` | 重载脚本文件 |

除 `ping` 外，每条请求的 `params` 里都要带对 `params.token`（前提是启动时给了 `--rpc-token`）。

资源上限（都是硬上限，写死在 `crates/r3e-editor/src/server.rs`）：

| 项 | 上限 | 超限行为 |
| --- | --- | --- |
| 请求队列 | 64 条（有界 `sync_channel`） | 立即回 `-32002 Server busy`，不断连、不阻塞引擎帧 |
| 并发连接 | 16 条（`ConnSlot` RAII 计数） | 第 17 条回 `-32002` 后立即关闭 |
| 单行请求体 | 1 MiB（`BufReader` + `read_until` + `take` 限长） | 回 `-32600` 后断开该连接，绝不会把超长行读进内存 |
| 响应等待 | 5 s（`recv_timeout`） | 超时回 `-32003 RPC timeout`，客户端不会傻等 |
| 空闲连接 | 30 s 无数据（`READ_TIMEOUT`） | 服务端主动断开，长期挂着的连接不会白占线程 |

```powershell
# 手动验一发（PowerShell）
$c = New-Object Net.Sockets.TcpClient('127.0.0.1', 7878)
$s = $c.GetStream(); $w = New-Object IO.StreamWriter($s); $w.AutoFlush = $true
$w.WriteLine('{"jsonrpc":"2.0","id":1,"method":"engine.info","params":{"token":"secret"}}')
(New-Object IO.StreamReader($s)).ReadLine()
```

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

### 材质：PBR 参数

| 函数 | 说明 |
| --- | --- |
| `set_material(name, metallic, roughness)` | `metallic` 0 = 绝缘体 / 1 = 纯金属（漫反射归零）；`roughness` 0 = 镜面 / 1 = 完全漫反射 |
| `set_emissive(name, r, g, b)` | 自发光（线性 RGB，可 > 1，上限 16），只加不减、不吃阴影 |
| `material_of(name)` | 读回收敛后真正生效的快照：`found` / `metallic` / `roughness` / `emissive` |
| `clear_material(name)` | 删除物件级材质，回到缺省 |

三件套（GGX / Smith / Schlick）细节见 [§15 PBR 材质](#15-pbr-材质直接光照-cook-torrance)。

### 环境探针：IBL

`scene.environment` 为 `None` 时仍走 `set_ambient` 的常数项（老场景行为不变）；一旦写入环境，
漫反射改由 9 项球谐给出方向感，金属靠环境高光不再死黑。

| 函数 | 说明 |
| --- | --- |
| `set_sky(zr, zg, zb, hr, hg, hb, gr, gg, gb)` | 三色天空：天顶 / 地平 / 地面（线性 RGB），9 个参数一次给全 |
| `set_env_sun(r, g, b, intensity, dx, dy, dz)` | 太阳项：颜色 + 强度 + 指向太阳的方向（零向量时保留原方向） |
| `set_env_intensity(v)` | 环境总强度，整体乘在天空 / 地面 / 太阳项上（1.0 = 物理量原值） |
| `has_environment() -> bool` | 当前是否启用环境探针 |
| `clear_environment()` | 关掉环境探针，退回 `light.ambient` 常数项 |

细节（球谐投影、解析高光、缓存）见 [§16 IBL 环境探针](#16-ibl-环境探针)。

### 时间 / 随机 / 输出

| 函数 | 说明 |
| --- | --- |
| `time() -> f64` | 启动至今秒数 |
| `dt() -> f64` | 上一帧时长 |
| `fps() -> f64` | 上一帧瞬时帧率（`1/dt`，首帧为 0），HUD 直接显示用 |
| `frame() -> int` | 帧序号 |
| `rand(min, max) -> f64` | 均匀随机 |
| `log(text)` | 输出到控制台 |

### 输入：键盘 / 鼠标 / 手柄

按键名不区分大小写，也可直接传索引（`0` = `A` 键 / 鼠标左键）。

| 函数 | 说明 |
| --- | --- |
| `key_down(key) -> bool` | 持续按住 |
| `key_pressed(key) -> bool` | 本帧刚按下（边沿，按住不放只触发一次） |
| `key_released(key) -> bool` | 本帧刚松开（边沿） |
| `key_any() -> bool` | 是否有任意键按住 |
| `key_any_pressed() -> bool` | 本帧是否有任意键按下（"按任意键开始"用） |
| `keys_held() -> array` | 当前按住的所有键名，如 `["a", "space"]` |
| `key_held_count() -> int` | 当前按住的键数 |

键盘键名：字母 `a`~`z`、数字 `0`~`9`、`f1`~`f12`、`up` `down` `left` `right`、`space` `enter`
`escape`(`esc`) `tab` `backspace` `delete`(`del`) `insert` `home` `end` `pageup` `pagedown`、
`shift`(`shift_left`/`lshift`) `shift_right` `ctrl`(`control`/`ctrl_left`) `ctrl_right` `alt`(`alt_left`)
`alt_right` `super`(`win`/`cmd`/`super_left`) `super_right` `caps_lock`、符号键 `minus` `equal`
`bracket_left` `bracket_right` `backslash` `semicolon` `quote` `comma` `period` `slash` `backquote`、
小键盘 `numpad0`~`numpad9` `numpad_add` `numpad_subtract` `numpad_multiply` `numpad_divide` `numpad_decimal`。

字符键按逻辑键解析（跟随键盘布局，AZERTY 上按 `a` 就是 `a`），修饰键与数字小键盘按物理键解析
（左右 Shift / Ctrl 可区分，小键盘数字与主键区数字互不干扰）。窗口失焦时所有键盘状态清零，
不会出现"按住 W 切窗口后一直往前走"。

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
所以判定"按一下"请用 `*_pressed`，判定"按住"请用 `*_down`。键盘的 `key_pressed` 即使按住不放
（系统的自动重复）也只在该帧为 true，可直接写 `if key_pressed("space")` 做开关。
示例见 `scripts/input_demo.rhai`（左键生成方块、右键清空、摇杆驱动探针、键盘 WASD 驱动探针移动）。

### 文本：屏幕空间 HUD（M1）

文本节点存在 `Scene::texts` 里，和物件共享"场景是唯一真源"的规则：脚本改完下一帧自动重排重画，
不需要任何刷新调用。坐标单位是**窗口像素**，原点在左上角；`(x, y)` 是文本框第一行**行顶**的锚点
（对齐方式只改 x 的含义，y 恒为行顶）。所有接口按 `key` 定位，**同名即覆盖**，每帧调用不会堆积节点。

| 函数 | 说明 |
| --- | --- |
| `text_add(key, content, x, y, size, r, g, b) -> bool` | 新建文本；`key` 已存在时只更新内容/位置/字号/颜色（保留对齐、换行、行高），返回 `false` |
| `text_set(key, content) -> bool` | 只改文字内容 |
| `text_set_pos(key, x, y) -> bool` | 改屏幕坐标 |
| `text_set_size(key, size) -> bool` | 改字号（自动夹到 1..512） |
| `text_set_color(key, r, g, b) -> bool` | 改 RGB（0..1），alpha 保持不变 |
| `text_set_alpha(key, a) -> bool` | 改透明度（0..1，走 alpha 混合） |
| `text_set_align(key, mode) -> bool` | `left` / `center` / `right`（别名 `l` `c` `r` `start` `end`）；未知值返回 `false` |
| `text_set_wrap(key, px) -> bool` | 自动换行宽度（像素）；`0` = 只按 `\n` 硬换行。设了宽度后对齐相对整个块宽计算 |
| `text_set_line_height(key, factor) -> bool` | 行高倍数（相对字号）；`0` = 字体自然行高 |
| `text_set_visible(key, on) -> bool` | 显示 / 隐藏 |
| `text_remove(key) -> bool` | 删除该文本，返回是否真的删掉了 |
| `text_clear()` | 清空全部文本（换场景 / 清 HUD 用） |
| `text_exists(key) -> bool` | 是否存在 |
| `text_content(key) -> string` | 读回内容（不存在返回空串） |
| `text_count() -> int` | 文本节点数 |
| `text_keys() -> array` | 全部 key |
| `set_text_enabled(on)` / `text_enabled() -> bool` | 文本通道总开关；关掉后渲染层整帧跳过文本 pass，3D 通道行为不变 |
| `text_width(content, size) -> f64` | 像素宽度估算（不加载字体，排版预算用，不是精确值） |
| `text_height(content, size) -> f64` | 像素高度估算 |

示例（`scripts/main.rhai` 里就是这套写法，左上角固定一行中文 + 实时帧率）：

```rhai
fn on_start() {
    text_add("hud_title", "r3e 引擎 · 屏幕空间文字", 24.0, 20.0, 30.0, 1.0, 0.92, 0.72);
    text_add("hud_status", "", 24.0, 96.0, 16.0, 0.62, 0.70, 0.80);
}

fn on_update(dt) {
    text_set("hud_status", "帧率 " + ((fps() * 10.0).round() / 10.0).to_string()
        + " / 帧号 " + frame().to_string());
}
```

### UI：矩形 / 边框 / 图片（M1）

UI 元素存在 `Scene::ui` 里，与文本同构：脚本改完下一帧自动重算重画，**不需要刷新调用**。
定位用「9 锚点 + 偏移」，尺寸/偏移都可以写像素或屏幕百分比，窗口 resize 后逐帧重新解析，
不会有残留错位。所有接口按 `key` 定位，**同名即覆盖**，每帧重复提交不会堆积节点。
元素总数上限 `MAX_UI_ELEMENTS = 1024`（`ui_capacity()` 可读），**只拦新增、不拦覆盖**：
满载时同 key 仍可刷新，新 key 会被拒绝并返回 `false`。

单位写法：`"24px"` 像素、`"50%"` 屏幕宽/高的百分比、`"12"` 等价 `"12px"`。
锚点：`topleft` / `topcenter` / `topright` / `midleft` / `center` / `midright` /
`bottomleft` / `bottomcenter` / `bottomright`（也接受 `top_left`、`bottom-center` 等写法）。

| 函数 | 说明 |
| --- | --- |
| `ui_rect(key, anchor, x, y, w, h, r, g, b, a) -> bool` | 纯色矩形（走 alpha 混合）；超限未写入返回 `false` |
| `ui_border(key, anchor, x, y, w, h, width, r, g, b, a) -> bool` | 空心边框（四条边各一个矩形，全部落在给定矩形**边界内**，四角不重叠；粗细夹到短边一半）；超限未写入返回 `false` |
| `ui_image(key, anchor, x, y, w, h, path, r, g, b, a) -> bool` | 纹理图片（`r,g,b,a` 乘到纹理上，全 1 = 原色）；路径相对工作目录或 exe 目录；超限未写入返回 `false` |
| `ui_set_pos(key, x, y) -> bool` | 改相对锚点的偏移 |
| `ui_set_size(key, w, h) -> bool` | 改尺寸 |
| `ui_set_anchor(key, anchor) -> bool` | 改锚点；未知值返回 `false` |
| `ui_set_color(key, r, g, b, a) -> bool` | 整体改颜色（含 alpha） |
| `ui_set_alpha(key, a) -> bool` | 只改 alpha |
| `ui_set_layer(key, layer) -> bool` | 改层级（整数，小的先画） |
| `ui_set_visible(key, on) -> bool` | 显示 / 隐藏 |
| `ui_set_border(key, width) -> bool` | 改边框粗细（对非边框元素无效果） |
| `ui_set_image(key, path) -> bool` | 换贴图路径 |
| `ui_set_uv(key, u0, v0, u1, v1) -> bool` | 取纹理子区间（图集裁切；九宫格在此基础上再切，见下节） |
| `ui_set_slice(key, left, top, right, bottom) -> bool` | 设置图片九宫格边距（像素或百分比，顺序左/上/右/下）；**无参调用 `ui_set_slice(key)` 等于关闭九宫格**；对非图片元素无效果 |
| `ui_remove(key) -> bool` | 删除该元素，返回是否真的删掉了 |
| `ui_clear()` | 清空全部 UI |
| `ui_exists(key) -> bool` / `ui_count() -> int` / `ui_keys() -> array` | 查询 |
| `ui_capacity() -> int` | UI 元素数量上限（当前 `1024`，由 `r3e_core::ui::MAX_UI_ELEMENTS` 定义） |
| `ui_last_error() -> string` | 最近一次 UI 写入被拒 / 被纠正的原因；无则空串 |
| `ui_error_count() -> int` | 累计被拒 / 被纠正次数（同 key 覆盖不算错误） |
| `set_ui_enabled(on)` / `ui_enabled() -> bool` | UI 通道总开关；关掉后整帧跳过 UI pass，3D / 文本不受影响 |
| `set_ui_above_text(on)` / `ui_above_text() -> bool` | **默认 `false`：文本画在 UI 之上**（HUD 底框不会盖字）；置 `true` 则 UI 盖住文本 |

**同 key 更新语义**（实现在 `r3e-core::ui`，脚本层不再自己判断）：

| 字段 | 同 key 重提时 |
| --- | --- |
| `kind` / `anchor` / `offset` / `size` / `color` / `image` | 取新值——每次提交都在描述「这一帧长什么样」 |
| `layer` / `visible` / `uv` / `slice_margin` | 沿用旧值——属于样式与开关，要改请走 `ui_set_layer` / `ui_set_visible` / `ui_set_uv` / `ui_set_slice` |
| `border_width` | 边框元素：入参 `> 0` 取新值，否则沿用旧线宽（旧值也没有就回落默认 `2.0`）；非边框恒为 `0` |

`ui_rect` / `ui_border` / `ui_image` 的返回值含义：`true` = 新建成功；`false` = 没新增
（同 key 覆盖，或已达上限被拒）。想区分这两者看 `ui_error_count()` / `ui_last_error()`——
被拒绝不静默丢弃。空 key（含纯空白）一律忽略并记一笔账。

元素按 `layer` 升序稳定排序绘制（同层按加入顺序），文本与 UI 的前后关系由
`set_ui_above_text` 决定，需要「某个 UI 盖住文字」时可整体切换或拆到不同通道处理。

#### 图片九宫格（9-slice）

`ui_image` 的元素可以在 `ui_set_slice(key, left, top, right, bottom)` 上设四条边距，
之后这张图**无论拉到多大，四角都保持原像素尺寸不拉伸**（对话气泡、面板边框、按钮底板
的常规做法）。字段名 `slice_margin`，存在 `r3e_core::ui::UiElement` 上。

- **单位**：与 `offset` / `size` 同一套 `UiValue`——`"8px"` 像素、`"25%"` 百分比、
  `"8"` 等价 `"8px"`。百分比分别相对**元素最终宽**（left/right）与**最终高**（top/bottom）
  换算，所以同一个元素改尺寸后比例边距会跟着变。
- **夹取**：求解时按元素**最终尺寸**收敛——`left + right` 不得超过宽、`top + bottom`
  不得超过高；超了就**等比缩到刚好放下**（各占一半），不会出现负宽度的块。负数、NaN、
  无穷一律收敛为 `0`（非法值不报错，只清零该条边）。
- **退化**：夹取后如果中间列 / 中间行的宽高变成 `0`，那几块直接不生成——只剩四角或四边，
  不会画出零面积 / 负面积四边形。
- **与 `uv` 的关系**：`uv` 是**先**在整张纹理上取出的子区间（图集裁切），九宫格再在**这个
  子区间内**按边距比例切 9 份。两者可叠加：`uv = [0.25,0.25,0.75,0.75]` + 左右各 10% 的元素宽，
  切出来的左列 uv 就是子区间左侧 10% 那一条。
- **切分布局**：3 列 × 3 行。四角尺寸 = 边距本身（不拉伸）；上/下边只用元素的横向拉伸、
  纵向保持边距高；左/右边只用纵向拉伸；中心双向拉伸。9 块的面积之和恒等于整块矩形。
- **关闭**：`slice_margin` 全 `0`（默认）或调用 `ui_set_slice(key)` 无参清零，走的是原来的
  单四边形路径，**不产生额外四边形**。
- **与四边形预算的关系**：一个九宫格元素最多 9 个四边形，仍受 `MAX_UI_QUADS`（4096）约束，
  且切分是**原子的**——剩余预算装不下整个 9 块时该元素整体丢弃（计入 `UiStats::skipped`），
  绝不会只画一半留下残片。只对 `UiKind::Image` 生效；矩形 / 边框 / 文本不受影响。

```rhai
// 32x32 的小图拉到 220x120：四角仍是 8px 原寸，中间拉伸
ui_image("panel", "topleft", "24px", "176px", "220px", "120px", "assets/ui/icon.png", 0.85, 0.74, 0.42, 0.95);
ui_set_slice("panel", "8px", "8px", "8px", "8px");
```

示例（`scripts/main.rhai` 内的最小 HUD）：

```rhai
fn on_start() {
    // 左上角血条：背景 + 当前值两层矩形
    ui_rect("hp_bg", "topleft", "24px", "132px", "260px", "22px", 0.04, 0.05, 0.07, 0.78);
    ui_rect("hp_fill", "topleft", "27px", "135px", "180px", "16px", 0.88, 0.20, 0.24, 0.95);
    // 底部对话框：宽度按屏幕百分比 + 边框
    ui_rect("dlg_bg", "bottomcenter", "0px", "-56px", "72%", "132px", 0.03, 0.04, 0.06, 0.62);
    ui_border("dlg_frame", "bottomcenter", "0px", "-56px", "72%", "132px", 2.0, 0.85, 0.74, 0.42, 0.9);
    // 右上角图标（png）
    ui_image("hud_icon", "topright", "-40px", "24px", "48px", "48px", "assets/ui/icon.png", 1.0, 1.0, 1.0, 1.0);
    // 九宫格面板：小图拉大，四角保持原寸
    ui_image("panel", "topleft", "24px", "176px", "220px", "120px", "assets/ui/icon.png", 0.85, 0.74, 0.42, 0.95);
    ui_set_slice("panel", "8px", "8px", "8px", "8px");
}

fn on_update(dt) {
    let hp = 0.35 + 0.6 * ((time() * 0.6).sin() * 0.5 + 0.5);
    ui_set_size("hp_fill", (180.0 * hp).round().to_string() + "px", "16px");
    ui_set_alpha("hp_fill", 0.55 + hp * 0.4);
}
```

人工验证：`cargo run` 起窗口后应看到左上血条（宽度随呼吸变化）、右侧/顶部文字、底部半透明对话框
底框与描边、右上角金色圆形图标；拖动窗口改变大小时对话框保持居中贴底、血条保持左上贴边。
把 `assets/ui/icon.png` 改名再热重载，图标位置会退化成纯色块（渲染层只记录一次加载失败，不刷日志、不崩）。

### 音频：音效 / 音乐（M1，非空间化）

音频走独立 crate `r3e-audio`，**不碰场景图**：`core` 里没有任何声音对象，脚本用「自己起的 key」管理实例。
范围严格限定在非空间化播放——没有 3D 声源衰减、没有多普勒、没有听者朝向。

**同名 key 的语义（写死在这里，避免踩坑）**：再次 `play` 同一个 key = **停掉旧实例、从头重播**，
既不是拒绝，也不是叠加第二份。想要同一文件同时响两份，就起两个不同的 key（`hit_a` / `hit_b`）。
改为 `set_loop` 同理——循环标志在解码源构造时确定，切换会从头开始。

| 函数 | 说明 |
| --- | --- |
| `audio_play(key, path) -> bool` | 用默认音量 `0.8`、不循环播放；成功返回 `true` |
| `audio_play_ex(key, path, volume, looping) -> bool` | 指定音量（`0–1`，自动夹取）与是否循环 |
| `audio_stop(key) -> bool` | 停止单条；该 key 不存在返回 `false` |
| `audio_stop_all()` | 停掉全部 |
| `audio_pause(key) -> bool` | 暂停，保留播放位置（静默降级时返回 `false`） |
| `audio_resume(key) -> bool` | 从暂停处继续 |
| `audio_set_volume(key, volume) -> bool` | 单条音量（`0–1`，NaN → 0，超界夹取） |
| `audio_set_loop(key, looping) -> bool` | 改循环标志（会从头重播） |
| `audio_is_playing(key) -> bool` / `audio_is_paused(key) -> bool` | 查询状态 |
| `audio_count() -> int` / `audio_keys() -> array` | 在播实例数 / 全部 key |
| `audio_set_master_volume(v)` / `audio_master_volume() -> f64` | 总音量（乘到每条声音上，`0` = 静音） |
| `audio_set_enabled(on)` / `audio_enabled() -> bool` | 音频通道总开关；关闭时停掉全部并拒绝新的播放请求 |
| `audio_available() -> bool` | 是否有真实设备（`false` = 静默降级中，可据此隐藏音量控件） |
| `audio_backend() -> string` | `rodio` / `silent` |
| `audio_last_error() -> string` / `audio_error_count() -> int` | 最近一次失败原因 / 累计失败次数 |
| `audio_status() -> string` | 一行状态串（后端、设备、总音量、在播明细） |

降级规则：设备打不开（无声卡 / 独占被占）→ 整个引擎切到 `silent`，`play` 仍然会**解码校验**文件
（所以坏文件照样返回 `false` 并记账），只是不出声；任何音频错误都不会 panic，也不阻断主循环。

示例（`scripts/main.rhai` 里的按键演示）：

```rhai
fn on_start() {
    audio_set_master_volume(0.8);
    audio_play_ex("ambience", "assets/audio/ambience.wav", 0.35, true);   // 背景循环
}

fn on_update(dt) {
    if key_pressed("j") {
        audio_play("blip", "assets/audio/blip.wav");                      // 连按只重启，不叠加
    }
    if key_pressed("k") {
        if audio_is_paused("ambience") { audio_resume("ambience"); }
        else if !audio_pause("ambience") { audio_play_ex("ambience", "assets/audio/ambience.wav", 0.35, true); }
    }
}
```

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
| `physics_raycast(ox,oy,oz, dx,dy,dz, max_dist) -> array` | 射线查询，命中返回 `[node, 距离, x, y, z]`，未命中为空数组 |
| `physics_overlap(x,y,z, radius) -> array` | 球体重叠查询，返回命中节点 id（升序） |
| `physics_raycast_full(ox,oy,oz, dx,dy,dz, max_dist) -> map` | 射线查询具名版：`hit / node / handle / distance / point / normal / inside` |
| `physics_shape_cast_sphere(ox,oy,oz, dx,dy,dz, radius, max_dist) -> map` | 球扫掠，字段同上（`inside` = 起点已重叠） |
| `physics_shape_cast_box(ox,oy,oz, dx,dy,dz, hx,hy,hz, max_dist) -> map` | 盒扫掠（半尺寸），字段同上 |
| `physics_has_capability(name) -> bool` | 查能力位（`body_body` / `rotation` / `ccd` / `events` / `raycast` / `overlap` / `shape_cast` …） |
| `physics_set_ccd(on) -> bool` | 设置 CCD 缺省值并同步刷新现存刚体，返回是否被后端接受 |
| `physics_ccd_default() -> bool` | 当前 CCD 缺省值 |
| `body_set_ccd(name, on) -> bool` | 逐刚体开关 CCD（固定体返回 false） |
| `body_ccd(name) -> bool` | 查询某刚体的 CCD 是否真开着 |

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
| `--ccd` | `on` / `off` | CCD 连续碰撞检测缺省值，默认 `on`（仅 `rapier` 后端支持） |

场景文件里的写法（`physics` 段可整体省略）：

```json
{
  "physics": { "enabled": true, "backend": "builtin", "gravity": [0, -9.81, 0], "ground": 0.0, "ccd": true },
  "objects": [
    { "name": "ball", "mesh": "sphere", "position": [-3, 6, 0],
      "physics": { "body": "dynamic", "mass": 1.0, "restitution": 0.85, "friction": 0.3, "ccd": true } }
  ]
}
```

**只有声明了 `physics` 段的物件才会被自动绑成刚体**；没写的（纯装饰）始终由脚本驱动。

内置后端只做**刚体 vs 无限地面平面**：重力积分 + 弹性反弹 + 摩擦 + 休眠，纯自实现零依赖。
**物件之间不碰撞、无旋转动力学、无地形碰撞**——需要这些就换成 Rapier（见下）。

完整规则、字段表、实测数据见 [`docs/PHYSICS.md`](docs/PHYSICS.md)。

### 换成 Rapier

Rapier 后端已落地（`crates/r3e-physics/src/rapier_backend.rs`，`rapier = ["dep:rapier3d"]` feature），
加 `--features rapier` 构建后 `--physics-backend rapier` 即可切换，随时切回 `builtin`：

```powershell
cargo run --release --features rapier -- --physics on --physics-backend rapier
```

Rapier 后端额外提供：刚体互撞与堆叠、旋转动力学、碰撞事件、**CCD 高速防穿透**（默认开）、
射线 / 重叠 / 扫掠查询。能力位由后端自报，脚本用 `physics_has_capability("ccd")` 判断，
**不要用 `physics_backend() == "rapier"` 判**——那会把后端名和能力绑死。

`joints` 仍是唯一禁区：底层支持，门面与脚本都没接口，因此两边都如实报 `false`。

### CCD 穿透对照

```powershell
cargo run --release --features rapier -- assets/scene_ccd.json scripts/ccd_demo.rhai
```

同一面 4 cm 厚运动学薄墙，两颗只差 `ccd` 一位的球同时以 120 m/s 射出（每帧走 2 m）：

| 物件 | `ccd` | 结果 |
| --- | --- | --- |
| `bullet_ccd`（蓝） | `true` | 停在墙前 `x ≈ -0.22`，不再前进 |
| `bullet_no_ccd`（黄） | `false` | 整帧跨过墙，飞向 +x 无穷远 |

墙刻意用 `kinematic` 而非 `static`：rapier 对固定碰撞体本来就做自动扫掠，静态墙下开关两边都挡得住，看不出差别。

---

## 6. 用 D:\ai_ide 编辑

引擎刻意把"可编程面"做成了普通文本文件，任何编辑器都能接管：

1. 在 `D:\ai_ide` 里 `File → Open Folder → D:\rust3d-engine`；
2. 打开 `scripts\main.rhai` 或 `assets\scene.json`，改完保存；
3. 引擎窗口内 0.4 秒后自动生效（场景文件改动需重启或按 F5 重载脚本）。

如果想在 IDE 里直接跑：终端执行 `cargo run --release`，工作目录设为 `D:\rust3d-engine`。

---

## 7. 已知边界

- 单方向光 + 环境光，直接光照走 **Cook-Torrance**（GGX + Smith + Schlick，参数 metallic / roughness / emissive）；
  **暂无 IBL / 环境探针**：金属只反射方向光，暗部靠环境光常量项兜底，所以纯金属背光时偏黑——
  后续接 IBL（预过滤环境图 + BRDF LUT）补这一块。方向光带单张正交阴影贴图（3×3 PCF，默认开启），无后处理。
  阴影限制：无点光/聚光阴影、无级联、透明物件也投实心影。
- 网格除四种程序化网格外，可加载外部 `.fbx`（含 base_color 贴图、材质因子、节点级 TRS 动画、
  骨骼蒙皮、交叉淡入与脚本状态机）；法线/粗糙度贴图、分层/叠加混合、morph target 尚未支持。
  用法与阶段现状见 [`docs/ASSETS.md`](docs/ASSETS.md)。
- 每帧重建实例缓冲，物件量级到数千仍可运行；上万需改成持久化实例缓冲。
- 脚本单线程同步执行，长循环会卡帧（Rhai 已限制操作数上限）。
- UI 只有矩形 / 边框 / 图片三类基本元素：**无圆角、无渐变、无旋转/缩放动画**；图片支持
  **九宫格（9-slice）拉伸**，圆角留待后续（先把九宫格做对再做角部描边更省事）；
  文本与 UI 的前后关系只有通道级开关（`set_ui_above_text`），不能逐元素互相穿插。
- 物理：内置后端物件互不碰撞、无旋转、无地形碰撞（无 CCD，高速可能穿地），默认关闭；
  换 Rapier 后端后支持互撞 / 旋转 / 碰撞事件 / **CCD 高速防穿透**（默认开，可逐物件关掉）。

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

---

## 10. 统一资产流水线（AIR，S0–S5 已完成）

引擎内**只存一种 3D 资产格式**：AIR（`r3e_core::AssetEntry`）。FBX、glb、gltf 都只是「入口」，
进来立刻归一成 AIR；渲染、物理、脚本都只消费 AIR，不再各认一套。

| 环节 | 唯一入口 |
| --- | --- |
| 导入 | `r3e_asset::ImporterRegistry`（`.fbx` / `.glb` / `.gltf`，未知扩展名显式报错） |
| 归一 | `r3e_core::air::normalize`（轴系/绕序/索引/法线/权重/分区/AABB 各一份实现） |
| 存储 | `AssetEntry` + `MeshData`（+Y up、+Z front、CCW、u32 索引、权重≤4 归一） |
| 输出 | `MeshProvider` 分层：`LayeredProvider` = 资产层 → 内置图元层，同一形态返回 |
| 消费 | `MeshCache`（单条上传路径）/ `r3e-physics`（按几何推断球/圆柱/盒） |

```powershell
# 任何支持的格式，同一套报告（含 AIR 不变量审计）
cargo run -- --dump-asset D:\models\character.fbx
cargo run -- --dump-asset D:\_probe_glb\samples\BoxAnimated.glb

# 真实样本进窗口
cargo run -- --asset D:\_probe_glb\samples\ironsword.glb
```

内置图元（cube / sphere / plane / cylinder）已从渲染层迁到 `r3e_core::procedural`，
与外部资产走**同一条**上传与绘制路径；`r3e-render/tests/architecture_guard.rs` 会在
`cargo test` 时扫描源码，任何 `MeshRef::Builtin` 来源分支复活都会直接让测试失败。

细节与 glb 已知限制（Draco / meshopt / KTX2 / morph 不支持）见
[`docs/ASSETS.md`](docs/ASSETS.md) 第 18 章。

---

## 11. 工程化：格式化、静态检查与 CI

工具链由 `rust-toolchain.toml` 固定为 `stable-x86_64-pc-windows-gnu`（含 `rustfmt` + `clippy` 组件）。
之所以写全 host triple：本工程用 MinGW 链接（`.cargo/config.toml` 指定 `build.target`），
若 rustup 默认 host 是 msvc，只写 `stable` 会切到 msvc 工具链，导致 build script / proc-macro
这些 host 产物去找 `link.exe` 而整片编译失败。

三道门每次改动后都要过，本地跑法：

```powershell
cargo fmt --all --check                        # 格式：只检查，不改文件
cargo fmt --all                                # 需要修格式时
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features rapier -- -D warnings
cargo test --workspace                         # 默认特性
cargo test --workspace --features rapier       # Rapier 物理后端
```

`cargo clippy` 不加 `-D warnings` 等于没跑——本仓库的口径是**零 warning**，
例外只允许带注释说明的 `#[allow]`，当前全库共三处：`r3e-render/src/renderer.rs` 的
`arc_with_non_send_sync`（`MeshProvider` 刻意不 `Send`，渲染主线程独占），以及
`r3e-asset/src/lib.rs`、`r3e-asset/src/gltf_import.rs` 各一处 `too_many_arguments`（顶点装配 / 导入入口参数天然是平行数组）。

CI 配置在 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)，三个 job 分别对应
`rustfmt` / `clippy -D warnings` / `test（默认 + rapier 两轮）`，都跑在 `windows-latest` 上
（Linux runner 无法完成 MinGW 交叉链接，硬跑只会得到误导性的红），
并用 PATH 里的 MinGW 覆盖 `.cargo/config.toml` 中的开发者本机绝对路径。

---

## 12. 文字渲染（M1 GPU 上屏）

**目标**：窗口里能画出任意 UTF-8 文本，含中文。

CPU 侧（`r3e-core`）负责"字 → 位图 + 度量 + 排版"，GPU 侧（`r3e-render`）只负责"把四边形画上去"：

| 层 | 模块 | 职责 |
| --- | --- | --- |
| `r3e-core` | `text.rs` | fontdue 字形栅格化、`layout_text`（换行 / 对齐 / 基线 / 缺字）、`FontSet` 多字体面回退 |
| `r3e-core` | `glyph_atlas.rs` | 货架式图集分配，键 = `(face, glyph, 量化字号)`，页满自动开新页（不丢字） |
| `r3e-render` | `text.rs` | `build_text` 建批（每字形一实例）、`TextPass`（pipeline + 图集纹理 + 实例缓冲） |
| `r3e-render` | `text.wgsl` | 实例化四边形 + 图集采样，片元只取 R8 覆盖度做 alpha |

**单帧顺序**：阴影通道 → 3D 主通道 → **文本通道**。文本是独立的 `RenderPass`，
`LoadOp::Load` 叠加在已画好的画面上，**自己的 pipeline 且不带深度附件** —— 所以文本永远在最上层，
不受 3D 深度测试影响，也不会写脏主通道的深度缓冲。

**图集按需增长**：图集初始 1 页（1024×1024 R8）；新字形分配不下时开新页并即时创建纹理 + bind group，
页数上限 8 页（`MAX_ATLAS_PAGES`）。同一字符同一字号第二次出现直接命中缓存，零上传——
所以每帧重排只上传"这一帧新出现的字"，稳态下通常是 0。

**上限与降级**：单帧字形实例上限 32768（`MAX_TEXT_INSTANCES`），超出的字只计入 `overflow` 不绘制，
不会 panic 也不会爆显存；字体文件找不到时通道存在但画不出字（`text_stats()` 里 `has_font = false`），
3D 渲染完全不受影响。

**字体查找顺序**（`r3e_core::text::system_font_candidates()`）：Windows 依次尝试 `msyh.ttc` / `msyh.ttf` /
`msyhbd.ttc` / `simhei.ttf` / `msyhl.ttc` / `Deng.ttf` / `simsun.ttc` / `segoeui.ttf`，macOS 与 Linux 各有一组
等宽 CJK 候选（PingFang / 文泉驿 / Noto CJK）。第一个存在的即用，失败不影响 3D 渲染；
也可用 `Renderer::load_text_fonts(&[路径])` 显式指定。

**人工验证**（需要真实窗口，CI 里跑不到）：

```powershell
cargo run
```

窗口左上角应出现三行中文：标题「r3e 引擎 · 屏幕空间文字」、副标题（含英文与数字，宽度超过 520px 自动折行）、
以及每帧刷新的状态行「帧率 xx.x / 帧号 … / 时间 …」；右上角是右对齐两行、顶部居中是对齐示例。
控制台会打印 `文本：已加载系统字体 [...]`。若逐帧观察状态行的帧率在 55~62 之间跳动且文字清晰不糊，
说明实例缓冲、图集上传与 alpha 混合都正常。

脚本侧关掉文本通道做对照：在 REPL 里执行 `set_text_enabled(false)`，文字立刻消失、3D 场景不变；
再 `set_text_enabled(true)` 恢复。这一步验证的是"文本与主通道完全解耦"。

**测试覆盖**（`cargo test -p r3e-render`，不依赖 GPU 与系统字体——用纯算术假字体）：

- 顶点生成数量与位置：`builds_one_quad_per_visible_glyph`、`multiline_text_offsets_baselines`
- 对齐换算：`align_shifts_quads_within_wrap_block`（left/center/right 相对块宽右移 0 / 一半 / 全部）
- 空串不产生 draw：`empty_or_invisible_text_produces_nothing`
- 超长文本不 panic：`very_long_text_does_not_panic`（3200 字）、`glyph_cap_drops_extra_instances_without_panic`（超限只记 overflow）
- 图集复用 / 分页 / 缺字：`repeated_glyphs_reuse_atlas_slot`、`full_atlas_spills_to_second_page`、`missing_glyph_is_counted_but_not_drawn`
- 脚本每帧提交不泄漏（`cargo test -p r3e-script`）：`per_frame_text_add_does_not_accumulate`（同一 key 连提交 10 帧，节点数恒为 1）、
  `text_set_on_missing_key_reports_false`、`text_clear_and_channel_switch`

---

## 13. UI 渲染（M1：矩形 / 边框 / 图片）

**目标**：HUD 与对话框所需的全部基本件——纯色矩形、空心边框、纹理图片，配合锚点自适应布局。

| 层 | 模块 | 职责 |
| --- | --- | --- |
| `r3e-core` | `ui.rs` | `UiElement` 数据、9 锚点 + 像素/百分比解析（`resolve_rect`）、层级稳定排序 |
| `r3e-render` | `ui.rs` | `plan_ui` 纯 CPU 建批、`UiPass` 单管线、`UiImageProvider`（图片解码由 app 注入） |
| `r3e-render` | `ui.wgsl` | 屏幕像素 → NDC、纹理/纯色调制、alpha 混合 |

**只有一条管线**：矩形 / 边框 / 图片在 GPU 侧都是"带纹理的屏幕四边形"，差异全在实例数据里——
矩形 1 实例（绑 1×1 白纹理）、边框 4 实例（上下左右，画在矩形外侧）、图片 1 实例。
同色矩形合并成一次 draw、图片按路径分组，所以演示里的 6 个元素 = **12 个四边形 / 2 次 draw call**。

**单帧顺序**：阴影通道 → 3D 主通道 → **UI 通道 → 文本通道**（默认）。两条 2D 通道都是
`LoadOp::Load` + 无深度附件，谁后画谁在上：默认文本在 UI 之上（对话框底框不会盖住台词），
`set_ui_above_text(true)` 可整体反转。

**布局**：`x = anchor_ax * (screen_w - w) + offset_x`，所以右对齐元素在窗口变宽时贴边而不是飞出屏幕；
每帧用当前 surface 尺寸重算，resize 不需要任何通知，也就不会错位。

**上限与降级**：元素数上限 1024（`r3e_core::ui::MAX_UI_ELEMENTS`，脚本 `ui_capacity()` 可读）——
**只拦新增、不拦覆盖**，满载时同 key 刷新照样写进去，新 key 被拒并返回 `false`，不挤掉已有元素；
单帧四边形上限 4096（`MAX_UI_QUADS`），超出丢弃计 `skipped`；图片加载失败
只记一次并在该元素上回退成纯色块（`missing_images` 计数），不刷日志、不 panic、不反复重试。

**人工验证**：

```powershell
cargo run -- --run-seconds 3   # 看控制台 UI 探针
cargo run                      # 看窗口
```

控制台应打印 `UI 通道：节点 6 / 绘制元素 6 / 四边形 12 / draw call 2 / 贴图 1 / 缺图回退 0 / 超限跳过 0 / 开关 true / UI 在文本之上 false`。
窗口里应看到：左上血条（宽度随呼吸变化）、底部半透明对话框底框 + 描边、右上角金色圆形图标（`assets/ui/icon.png`）；
拖动窗口缩放时对话框保持居中贴底、血条保持左上贴边。REPL 里 `set_ui_enabled(false)` 可对照验证 UI 与 3D/文本解耦。

**测试覆盖**（`cargo test -p r3e-core -p r3e-render -p r3e-script`）：

- 锚点落点 9 种、百分比换算、resize 前后重算、空/负/非有限尺寸与超界偏移不 panic、层级稳定排序、
  同名 key 覆盖不累积且不改槽位顺序（`upsert`）、数量上限只拦新增、同 key 合并策略与边框线宽回落
  （`merge_same_key` / `upsert_styled`）、非法值写入前收敛（`r3e-core/src/ui.rs`）
- 矩形 1 实例 / 边框 4 实例且厚度被夹 / 图片按纹理分组 / 隐藏与退化元素跳过 / quad 预算丢弃 /
  图片只请求一次、失败只记一次（`r3e-render/src/ui.rs`）
- 脚本每帧提交不累积（`per_frame_ui_rect_does_not_accumulate`）、同名覆盖保留 `layer`/`visible`/`uv`、
  脚本路径超限被拒且计数不增长、满载时同 key 仍可更新、空 key 被拒、非法参数收敛、
  通道开关与前后关系（`r3e-script/src/lib.rs`）

## 14. 音频播放（M1：非空间化音效与音乐）

**目标**：能放背景音乐、能敲音效、能同时响多条，并且**没声卡也绝不崩**。3D 声源衰减 / 多普勒不在本阶段范围。

| 层 | 模块 | 职责 |
| --- | --- | --- |
| `r3e-audio` | `lib.rs` | `Audio` 门面：key 账本、单条音量 × 总音量、开关、错误记账、`update()` 回收 |
| `r3e-audio` | `backend.rs` | `AudioBackend` 契约 + `RodioBackend`（cpal 设备 + symphonia 解码）+ `SilentBackend` |
| `r3e-script` | `api.rs` | `audio_*` 绑定，全部返回 `bool` / 数值 / 字符串，不 panic |
| `r3e-app` | `app.rs` | `EngineApp::audio` 持有唯一设备句柄，主循环每帧 `update()` 回收播完的实例 |

**后端选型：`rodio`（内含 `cpal` + `symphonia`）**，理由：

- **无 `unsafe` 代码到我们这一层**：`rodio` 把 cpal 的设备线程、symphonia 的解码器封装成 `Sink` / `Decoder`，
  上层不需要写任何平台代码；`r3e-core` 仍然零音频依赖。
- **依赖面小且可控**：`default-features = false` + 只开 `playback` / `wav` / `vorbis` / `mp3`，
  不把 flac / AAC 等用不上的解码器拖进二进制；相对 `cpal + symphonia` 手搓，省掉自己写环形缓冲、
  重采样与设备热切换的那几百行容易出错的代码。
- **与现有风格一致**：`Audio` 门面 + `AudioBackend` trait 的写法，与 `Physics` 门面 + `PhysicsBackend`
  完全同构（真后端 / Null 后端 / 可替换 / 单测用替身），团队不需要学第二套心智模型。
- 未选 `kira`：它的强项是音轨时钟、参数自动化与空间化，本阶段用不上，反而带来更大的配置面。

**支持的格式**：`.wav`（PCM）、`.ogg`（Vorbis）、`.mp3` —— 由 symphonia 解码并统一重采样到设备采样率。
解码是**整文件进内存**，不是流式；演示素材只有几十 KB，够用。

**混音**：多条声音各自一个 `Sink`，由 cpal 设备混音；门面维护"单条音量"，
每次下发的有效音量 = `单条音量 × 总音量`，改总音量时逐条重算——后端不需要支持 master。

**降级策略（关键）**：

| 情况 | 行为 |
| --- | --- |
| 打不开默认设备 | `log::warn!` + 门面切 `SilentBackend`，`audio_available() = false`，引擎照常跑 |
| 文件不存在 / 不是音频 / 解码失败 | `play` 返回 `Err` → 脚本侧 `false`，原因写进 `audio_last_error()`，`error_count` 递增 |
| key 为空 | 拒绝并记账，不 panic |
| 音频通道关闭 | 停掉全部并拒绝新的播放请求 |
| 音量 NaN / < 0 / > 1 | `sanitize_volume`：NaN → 0，其余夹到 `[0, 1]` |

静默模式下 `play` 仍然会**打开并解码校验**文件——所以"文件坏了"和"没声卡"是两件可分的事：
前者返回 `false`，后者返回 `true` 但不出声。静默后端不会登记实例，因此 `audio_count()` 不会虚高。

**回收**：播完的实例不会自己消失，主循环每帧调用 `Audio::update()`，把后端已播完的 key 从账本里摘掉。
摘账发生在实例播完的那一帧（暂停时也照跑，避免恢复后残留空实例），`RUST_LOG=debug` 下会打印
`音频实例播完回收: [...]`；收尾探针里的"在播 N"是回收后的实时账本，不是累计播放次数。

**演示素材来源**：`assets/audio/blip.wav` 与 `ambience.wav` 都是**用脚本合成**的正弦波
（880Hz 指数衰减 / 220Hz + 330Hz 叠加，16-bit 单声道 44.1kHz），不是第三方版权素材，可随仓库分发。

**人工验证**（真实出声只能人工听，CI 无法自动验证）：

```powershell
cargo run                 # 启动即循环播放环境音
cargo run -- --run-seconds 3   # 看控制台音频探针
```

控制台应打印 `音频：后端 rodio / 设备 可用 / 在播 1（实例 1） / 总音量 0.80 / 开关 开 / 错误 0 / 最近 无`；
窗口标题末尾显示 `音频 N 实例`（无设备时显示 `音频静默`）。按 `J` 应听到短促「嗒」声，
连按不会变成一片叠加的噪音；`K` 暂停后环境音停在原处、再按从原处继续；`L` 停止 / 重新开始；
`M` 把总音量在 0.8 / 0.2 之间切换（音效与环境音同时变小）。
把某个 wav 改名热重载，`audio_last_error()` 里会出现 `打开音频文件失败（…）`，窗口照常运行、不崩。

**测试覆盖**：

- `cargo test -p r3e-audio`（9 个）：缺失文件 / 非音频内容 → `Err` 而非 panic、音量边界（0 / 1 / 超界 / NaN）、
  静默后端不登记实例、通道关闭拒绝播放、空 key 拒绝、同名 key 重播停止旧实例不叠加、
  `set_loop` 切换重播且幂等、播完回收。
- `cargo test -p r3e-script`（`audio_script_tests`，5 个）：静默降级路径（`audio_available() == false` 时
  播放返回 `true` 但不占实例）、加载失败返回 `false` 并记账、音量上下界夹取、重复 key 不叠加且不算错误、
  通道开关拦截播放。
- 真实出声、mp3 / ogg 的实际听感、设备热插拔：**待人工验证**（无法自动化）。

---

## 15. PBR 材质（直接光照 Cook-Torrance）

三个旋钮：`metallic`（0..1）、`roughness`（0..1）、`emissive`（线性 RGB，上限 16）。
API 见 [§4 材质：PBR 参数](#材质pbr-参数)。

**语义边界：物件级材质是覆盖层**。`Node.material` 为 `Option<SurfaceMaterial>`——不写就是沿用网格资产
（FBX / glTF）自带的材质；写了就在资产材质之上覆盖这三个旋钮，base color 与贴图仍归资产层。
因此 `set_material("cube", ...)` 只对内置图元"从零建材质"，对导入模型则是"改它的表层参数"。
脚本 `material_of` 读 `None` 物件时给的是缺省快照（`0 / 0.6 / 无自发光`），不是资产里的真实数值。

**着色模型**：`crates/r3e-render/src/shader.wgsl` 与 `crates/r3e-core/src/material.rs` 逐项对应，
CPU 侧是纯函数、不依赖 wgpu，所以能吃单测。

| 项 | 形式 |
| --- | --- |
| D 法线分布 | GGX / Trowbridge-Reitz，α = roughness² |
| G 遮蔽 | Smith，Schlick-GGX 近似（直接光 k = (r+1)²/8） |
| F 菲涅尔 | Schlick，F0 = mix(0.04, albedo, metallic) |
| 漫反射 | (1 − F)·(1 − metallic)·albedo/π：金属无漫反射，且扣掉镜面已拿走的份额 |
| 自发光 | 直接加到出射亮度，不乘 n·l、不受阴影影响、不参与光照计算 |
| 阴影 | 只衰减直接光，环境光与自发光不衰减（否则影子是纯黑块） |

**收敛与夹取**（写入前完成，场景 / 脚本 / RPC 三条路径共用同一套）：

- `metallic` 夹到 0..1；`roughness` 夹到 `MIN_ROUGHNESS = 0.045`..1；`emissive` 每通道夹到 0..16；
- NaN 退回缺省值，`±inf` 按边界处理，`-0.0` 归一成 `0.0`——材质键按 `to_bits` 比较，
  等价键分裂会把同一材质的物件拆成两个绘制批次；
- 着色器再夹一次粗糙度下限，属第二道防线：CPU 侧漏了也不会出 NaN。

**退化处理**：`roughness → 0` 时 α 不趋零；`n·v` 掠射留 `1e-4` 下限；
`l + v` 退化时半程向量直接取法线；`n·l ≤ 0` 镜面项给零而不是除零。

**演示**：`scripts/main.rhai` 的 `on_start` —— `spinner`（metal 1.0 / rough 0.18）、
`orbiter`（0.0 / 0.85）、`sky_cube`（0.0 / 0.35），外加自发光 `lamp`。

**测试覆盖**：

- `cargo test -p r3e-core` 的 `material` 模块（11 个）+ `tests/material_roundtrip.rs`：
  参数收敛、金属度归零漫反射、退化输入保持有限、白炉半球积分反射率 ≤ 1（24×48 采样 × 6 档粗糙度 × 3 档金属度）、
  粗糙绝缘体反射率接近 albedo、直接光不超光色、自发光只加不减、场景序列化往返。
- `cargo test -p r3e-render`：材质 uniform 布局与着色器逐字段对齐（48 字节 / 16 对齐）、物件级材质键分桶。
- `cargo test -p r3e-script`：`set_material` / `set_emissive` / `material_of` / `clear_material` 的写入、收敛与读回。
- 视觉观感（金属是否像金属、高光带是否自然）**待人工窗口确认**：没有截图 / 像素比对测试。

---

## 16. IBL 环境探针

直接光照下金属的暗部是纯黑（没有环境可反射），观感立刻"塑料"。本节给一个**零外部依赖的解析环境探针**：
不加载 HDR 贴图、不做预过滤立方体贴图，只用三色天空 + 一个太阳方向，在 CPU 侧投影成 9 项球谐，
着色器侧再叠一个解析高光项。

**为什么不上 HDR 立方体贴图**：现阶段没有资产管线产 cubemap 的输入源，且 IBL 预过滤需要 compute 或
多 pass 渲染，成本远高于收益。球谐（L2，9 项）足以表达"天顶蓝、地平暖、地面暗"这类低频方向感，
而金属的高频反射本来就该来自反射探针——那属于后续阶段。

**数据流**：

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/env.rs` | `EnvironmentSettings`（7 个字段）+ `sanitized()` + Sh9 投影（CPU 纯函数） |
| 传输 | `crates/r3e-editor/src/lib.rs` | `EnvironmentPatch`（字段全可选）+ `scene.set_environment` 解析 |
| 缓存 | `crates/r3e-render/src/renderer.rs` | 环境配置变化才重投影，`environment_builds()` 暴露次数 |
| 上屏 | `crates/r3e-render/src/shader.wgsl` | 球谐漫反射 + 解析高光，`env_horizon.w > 0.5` 为启用位 |

**语义**：`scene.environment == None` 时完全走老的 `light.ambient` 常数项——**不写环境段的老场景行为不变**。
写入后环境项替代 ambient（不是叠加），太阳项是"环境里的太阳"，与主方向光独立：
主光负责阴影与直接高光，太阳项负责金属里那团亮斑。

**收敛**：与材质同一套规矩——颜色夹到 0..16、`intensity` 夹到 0..8、`sun_dir` 归一化（零向量保留原方向）、
NaN 退回缺省；`sun_intensity` 为 0 时太阳项整段消失（只剩天空与地面），不会把高光打进 NaN。

**缓存**：`EnvironmentSettings` 实现了按位比较的等值判断，配置没变就不重投 Sh9。
探针日志 `[起始] 环境探针：… 球谐重投影 N 次` 可直观看出来：跑 2 秒上百帧，`N` 稳定为 1。

**演示**：`assets/scene_ibl.json` —— 三档金属球（roughness 0.05 / 0.35 / 0.75）并排，
后排两颗亮面 / 哑光介电球做对照，加一条高强度自发光条：

```powershell
cargo run -- assets/scene_ibl.json
```

脚本侧等价写法（`scripts/main.rhai` 里可随时改）：

```rhai
set_sky(0.16, 0.26, 0.52,  0.62, 0.56, 0.48,  0.075, 0.068, 0.06);
set_env_sun(1.0, 0.95, 0.86, 1.4,  0.42, 0.84, 0.33);
set_env_intensity(1.0);
```

**已知边界**：

- 环境只有一阶方向（球谐 L2）与一个太阳，**没有镜面反射探针**：金属里映不出场景物件，只能映出"天/地/太阳"；
- `CameraUniform` 从 208 字节扩到 352 字节（环境 5 项 + 9 项球谐共 14 个 `vec4`），仍按 16 字节对齐；
- 环境项不参与阴影判定——影子边缘的环境亮度不会随阴影变暗，属当前妥协。

**测试覆盖**：

- `cargo test -p r3e-core`：`tests/env_probe.rs`（Sh9 投影确定性、天空-地面方向分布、太阳项方向性、收敛夹取）；
- `cargo test -p r3e-core`：`tests/environment_scene.rs`（缺省 `None`、场景载入收敛、JSON 往返、投影确定性）；
- `cargo test -p r3e-render`：`tests/shader_wgsl.rs`（naga 静态解析 + 语义校验 + 球谐数组与常量对齐 + CPU 参考实现一致）；
- `cargo test -p r3e-editor`：`tests/environment_rpc.rs`（字段逐一解析、部分补丁保留原值、`enabled: false` 清空、空补丁与类型错误拒绝）；
- 实机证据：`--run-seconds 2` 加载 `assets/scene_ibl.json`，`环境探针：启用 true / 总强度 1.00 / 太阳强度 1.40 / 球谐重投影 1 次`；
- 视觉观感（金属暗部是否"活"了）**待人工窗口确认**：无截图 / 像素比对测试。

---

*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
