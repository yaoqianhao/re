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
  另有 `rapier3d` 真实后端（`--features rapier`）可换，两者共用同一套契约；查询层 `raycast` / `overlap` / `shape_cast` 后端无关

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
│   ├─ scene_hdr.json         # HDR 后处理演示场景（高亮自发光条 + 金属球 + 泛光/ACES）
│   ├─ scene_ssao.json        # SSAO 演示场景（墙角 / 台阶接缝 / 贴地球，遮蔽开关可对比）
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
│   ├─ shadow_demo.rhai       # 阴影演示（光源扫动 + 强度呼吸 + 右键开关）
│   └─ ssao_demo.rhai         # SSAO 演示（强度呼吸 + 1/2/3 切半径 + 4/5 切样本 + 右键开关）
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
（`scene.setTransform` = `scene.set_transform`），共 31 个：

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
| `scene.set_postfx` | 改 HDR 后处理（`exposure` / `tonemap` / `bloom_enabled` / `bloom_threshold` / `bloom_knee` / `bloom_intensity` / `fxaa`，可分别给；`enabled: false` 清空），回收敛后的 `postfx`（清空后为 `null`） |
| `scene.set_ssao` | 改屏幕空间遮蔽（`enabled` / `radius` / `bias` / `intensity` / `samples`，可分别给；`enabled: false` 关掉），回收敛后的 `postfx.ssao` |
| `scene.shadow` | 改级联阴影（`enabled` / `size` / `strength` / `orthoSize` / `bias` / `cascades` / `blend` / `distance`，可分别给），一个字段都不给即只读回当前段 |
| `scene.anim_layers` | 列动画实例与每实例的层摘要（只读，见 [§21 动画混合树](#21-动画混合树扩展十八-a)） |
| `scene.anim_layer_set` | 补丁式改单层：`weight` / `mode` / `mask` / `speed`；未知字段、未知模式、不存在的实例或层一律 `ok=false` 且不落副作用 |
| `scene.stack` | 场景栈声明态：`index` / `name` / `path` / `source_bytes` / `frame` / `time` / `current` + `count` / `max`，支持 `limit`（见 [§22](#22-场景栈与存档扩展十八-b)） |
| `scene.switch` | 请求切到另一个场景文件（`path` 必填且文件须存在），回 `{queued, command, stack}` |
| `scene.back` | 请求回退一层（无参）；已在最早一层直接报错，不排空命令 |
| `scene.archive.save` | 存档**当场写盘**后返回 `{path, format, version, bytes, objects, anims}`（`path` 省略则落 `<场景名>.archive.json`，所以是 `queued: false`） |
| `scene.archive.load` | 请求读档（`path` 必填），回 `{queued, command, stack}` |
| `scene.save` | 把内存场景写回 JSON（`path` 省略时为原场景文件），回 `{path, objects, bytes}` |
| `ui.list` | 列 UI 元素与总数，支持 `limit` |
| `ui.set` | 按 `key` 增改 UI 元素（`kind` / `anchor` / `x` / `y` / `width` / `height` / 颜色 / 边框 / 图片等，可分别给） |
| `ui.remove` | 按 `key` 删除 |
| `ui.state` | 指针交互状态（hover / active / 有界事件队列） |
| `light.list` | 列局部光源与总数，支持 `limit` |
| `light.set` | 按 `key` upsert 一盏（除 `key` 外至少给一个字段） |
| `light.remove` | 按 `key` 删除 |
| `light.clear` | 清空全部局部光源 |
| `light.set_enabled` | 开关局部光；省略 `enabled` 表示只读 |
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
| `set_shadow_extent(half)` | 正交半宽；0 = 按场景包围球自适应；开级联时作单级上限 |
| `set_shadow_cascades(n)` / `shadow_cascades()` | 级联数 1..=4（`1` = 单级，等同阶段 D） |
| `set_shadow_blend(v)` | 级间过渡带宽（占该级跨度比例），0..0.5 |
| `set_shadow_distance(m)` | 阴影最远距离；0 = 用相机 zfar 或场景包围球最远点 |

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

### 后处理：HDR 链

`scene.postfx` 缺省为直通（曝光 0、无泛光、无 FXAA），画面与扩展十三之前逐像素一致；
写这段只是"把已经渲出来的 HDR 亮部接住"，不改光照数值。

| 函数 | 说明 |
| --- | --- |
| `set_postfx(exposure, tonemap, bloom, intensity, fxaa)` | 一击式全量设置：EV、曲线名、泛光开关、泛光强度、抗锯齿开关 |
| `set_exposure(ev)` | 曝光（EV，夹 -8..16；0 = 原值，+1 = 亮一倍） |
| `set_tonemap(name)` | 曲线：`"none"` / `"reinhard"` / `"aces"`（认不出的名字回落 `none` 并打印一行） |
| `set_bloom(on, threshold, knee, intensity)` | 泛光：亮部阈值、软膝宽度、强度（关掉时后三个参数被忽略） |
| `set_fxaa(on)` | FXAA 开关（在 composite 里做，零额外 pass） |
| `has_postfx() -> bool` | 当前是否写了后处理段 |
| `clear_postfx()` | 清空后处理段，回到直通 |
| `postfx_exposure() -> f64` | 回读曝光 EV（未开启时给缺省 0） |
| `postfx_tonemap() -> string` | 回读曲线名（`"none"` / `"reinhard"` / `"aces"`） |

细节（HDR 目标、三级泛光、composite 顺序）见 [§17 HDR 后处理链](#17-hdr-后处理链)。

### 屏幕空间遮蔽：SSAO（扩展十七）

`postfx.ssao` 缺省 `enabled: false`，整段跳过，画面与扩展十三逐像素一致。
开启后从**深度缓冲**反推视空间位置与邻域法线，按半球核采样判断遮挡率，再乘进 composite。

| 函数 | 说明 |
| --- | --- |
| `set_ssao(enabled, radius, bias, intensity, samples)` | 一击式全量设置（半径 / 偏置 / 强度 / 样本数） |
| `set_ssao_enabled(on)` | 开关（关掉后两个遮蔽 pass 都不提交，探针里 `pass` 归 0） |
| `set_ssao_radius(r)` | 视空间半径，夹 `0.01..8`（默认 0.6：小只压缝隙，大则整面墙发灰） |
| `set_ssao_intensity(v)` | 强度，夹 `0..4`（默认 1.0；AO 下限有兜底，不会糊成死黑） |
| `set_ssao_samples(n)` | 每像素样本数，夹 `4..16`（默认 8；少则噪点多，散焦只能压一部分） |
| `clear_ssao()` | 关掉遮蔽（等价 `set_ssao_enabled(false)`） |
| `has_ssao() -> bool` | 当前遮蔽段是否非中性 |
| `ssao_enabled() -> bool` / `ssao_radius()` / `ssao_bias()` / `ssao_intensity()` / `ssao_samples()` | 回读收敛后的实际值（`bias` 只读，写入走场景或 RPC） |

细节（深度重建、半球核、散焦、代价与边界）见 [§20 屏幕空间遮蔽](#20-屏幕空间遮蔽ssao扩展十七)。

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

### 局部光源：点光 / 聚光（扩展十五）

场景最多 **16 盏**局部光源，`light_*` 写、`local_light_*` 读（读写同一份 `Scene::lights`）。
强度夹 `0..200`、颜色夹 `0..16`、半径夹 `0.05..500`，聚光内角自动比外角小至少 `0.1°`——写入即收敛，脚本不用自己夹。

```rhai
fn on_start() {
    light_add_point("torch", 0.0, 2.0, 6.0, 1.0, 0.72, 0.42, 6.0, 8.0);
    light_add_spot("beam", 0.0, 4.0, 0.0, 0.0, -1.0, 0.0, 0.55, 0.78, 1.0, 14.0, 8.0, 12.0, 26.0);
}
```

| 函数 | 说明 |
| --- | --- |
| `light_add_point(key, x,y,z, r,g,b, intensity, range)` | 建 / 覆盖点光（9 参数） |
| `light_add_spot(key, x,y,z, dx,dy,dz, r,g,b, intensity, range, inner_deg, outer_deg)` | 建 / 覆盖聚光（13 参数，方向自动归一化） |
| `light_set_pos` / `light_set_dir` / `light_set_color` / `light_set_intensity` / `light_set_range` | 逐字段改（第一参数都是 key） |
| `light_set_cone(key, inner_deg, outer_deg)` | 改锥角 |
| `light_remove(key)` / `light_clear()` | 删一盏 / 清空（返回删除数） |
| `set_lights_enabled(on)` / `lights_enabled()` | 总开关（关掉只是不再上传有效灯数，灯的数据留着） |
| `local_light_count()` / `local_light_keys()` / `local_light_exists(key)` | 数量 / key 列表 / 存在性 |
| `local_light(key)` | 整盏快照 map |
| `local_light_intensity(key)` / `local_light_kind(key)` | 单字段快捷读（`point` / `spot`） |
| `local_light_irradiance(x, y, z)` | 某点上所有局部光源的累加辐照度（与画面同式，可直接拿来做「越亮越暖」之类的反馈） |

满 16 盏后 `light_add_*` 返回 `false` 且**不顶掉已有灯**——要加新灯先 `light_remove`。
演示：`cargo run -- assets/scene.json scripts/lights_demo.rhai`。

### 指针交互：命中测试 / 悬停 / 点击（扩展十四）

UI 元素能被鼠标点到。命中测试只看**矩形**（边框空隙与图片透明像素也算点得到），
`visible = false` 的元素点不到，`set_ui_enabled(false)` 时整层不参与命中。
指针压在任意 UI 元素上时，**左键不再驱动相机**（`cursor_over_ui()` 为真即让位）——否则点按钮会顺手把镜头转飞。

| 函数 | 说明 |
| --- | --- |
| `ui_events() -> array` | 本帧事件数组，元素是 map：`kind`（`hovered` / `unhovered` / `pressed` / `released` / `clicked` / `cancelled`）、`key`、`x`、`y`。**每帧重置**，`on_update` 开头遍历一次即可 |
| `ui_event_count() -> int` | 本帧事件条数（上限 `64`，超出部分丢弃） |
| `ui_event_kind_name(ev) -> string` / `ui_event_key(ev) -> string` | 从事件 map 里取 `kind` / `key`（也可以直接 `ev.kind` / `ev.key`） |
| `ui_hover() -> string` | 当前指针悬停的元素 key；无则空串 |
| `ui_active() -> string` | 当前被按下（按住未抬）的元素 key；无则空串 |
| `ui_clicked(key) -> bool` | 本帧是否有该元素的 `clicked`（同元素内按下并抬起） |
| `ui_hovered(key) -> bool` | 本帧是否有该元素进入或处于悬停 |
| `ui_pointer_over_ui() -> bool` | 指针是否压在任何 UI 元素上（脚本可用它做「UI 打开时屏蔽游戏输入」） |
| `ui_hit(x, y) -> string` | 给定屏幕坐标反查最上层元素 key；未命中返回空串 |

事件语义（一帧一次推进，先于 `on_update` 生效）：

| 事件 | 何时产出 |
| --- | --- |
| `hovered` / `unhovered` | 指针进出元素。**与按键无关**：抬起后指针还在元素上就保持 hover，`ui_hover()` 仍返回该 key |
| `pressed` | 在元素上按下左键 |
| `clicked` | **同一个**元素上按下并抬起（先于 `released` 产出） |
| `cancelled` | 按下后没在同一元素上抬起（移出后释放、或元素被删） |
| `released` | 只要有按下捕获，抬起总会收到一条 |

最小例子（`scripts/ui_interaction_demo.rhai` 的骨架）：

```rhai
fn on_update(dt) {
    for ev in ui_events() {
        if ev.kind == "clicked" && ev.key == "play" {
            ui_set_color("spinner", 0.2, 0.9, 0.5, 1.0);
        }
        if ev.kind == "hovered" && ev.key == "play" {
            ui_set_alpha("play", 1.0);
        }
        if ev.kind == "unhovered" && ev.key == "play" {
            ui_set_alpha("play", 0.7);
        }
    }
}
```

**已知边界**：不做键盘焦点 / Tab 遍历、不派发滚轮与拖拽、无长按与双击、控件自身的状态机（按钮按下变色、
列表滚动）仍需脚本自己写——引擎只负责「谁被点到」，不假设控件类型。

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

### 动画：片段切换与分层混合（扩展十八 A）

片段播放走 `play_anim` / `set_anim_clip` / `set_anim_clip_fade`（交叉淡入，阶段三 C），
分层混合走 18 个 `anim_layer*` API（挂层 / 遮罩 / 权重淡入淡出 / override 与 additive），
**完整表与硬口径见 [§21 动画混合树](#21-动画混合树扩展十八-a)**，此处不重复。

### 场景切换与存档（扩展十八 B）

这一组函数全是**申请**而不是**执行**：参数进宿主的单槽，由 `EngineApp` 在**帧首**统一落地
（原因见 [§22](#22-场景栈与存档扩展十八-b)）。因此它们在 `on_start` / `on_update` 里随便调都不会
把世界撕成两半，返回值 `true` 只表示"这一帧的换世界请求收下了"。

| 函数 | 说明 |
| --- | --- |
| `scene_switch(path) -> bool` | 申请切到另一个场景文件；`path` 空白或槽已占返回 `false` |
| `scene_back() -> bool` | 申请回到栈里上一层（只移游标、不删层，可再前进回来） |
| `scene_save_archive(path) -> bool` | 申请存档；`path` 传空串 = 让宿主挑默认位置 |
| `scene_load_archive(path) -> bool` | 申请读档；`path` 空白返回 `false` |
| `scene_command_pending() -> bool` | 槽里是否已有待落地命令（配合限流：`if !scene_command_pending() { … }`） |
| `scene_stack_names() -> array` | 层名数组（按压栈顺序） |
| `scene_stack_depth() -> int` | 层数 |
| `scene_stack_index() -> int` | 当前层号；**空栈返回 `-1`**，与"第 0 层"区分开 |
| `scene_stack_current() -> string` | 当前层名；空栈返回 `""` |

```rhai
// 血量归零：单槽一帧只收一条，所以"先存一局、再切结算场景"要拆成两帧走
let saved = false;                       // 放在脚本顶层
// on_update 里：
if hp <= 0 && !scene_command_pending() {
    if !saved {
        saved = scene_save_archive("");   // 空串 = 默认落在场景文件旁边
    } else {
        scene_switch("assets/scene_over.json");
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
| `--physics-backend` | `builtin` / `null` / `rapier` | 后端；`rapier` 需 `--features rapier` 构建，未编译进当前构建时选它会提示并拒绝 |
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

- 单方向光 + **环境探针（IBL）**，直接光照走 **Cook-Torrance**（GGX + Smith + Schlick，
  参数 metallic / roughness / emissive）。环境探针是解析式（三色天空 + 太阳 + 9 项球谐），
  **没有 HDR 立方体贴图、没有反射探针**：金属只映天 / 地 / 太阳，映不出场景物件，见
  [§16 IBL 环境探针](#16-ibl-环境探针)。方向光带单张正交阴影贴图（3×3 PCF，默认开启）；
  扩展十六把这张图升级为**最多 4 级级联**（`light.shadow.cascades`，缺省 1 = 单级，老场景画面不变）：
  视锥按深度切段、每段独立拟合光源正交相机，近处纹素更密、远处只求覆盖，级间按 `blend` 混合消接缝。
  阴影限制：无点光/聚光阴影、级联上限 4、每级重复渲染全场景可见物件（未按级裁剪）、透明物件也投实心影。
- 局部光源：最多 16 盏点光 / 聚光，**不投影**（点光立方阴影代价太高），无 IES / 面光源，只进直接光项、
  不参与环境项，见 [§19 局部光源](#19-局部光源扩展十五)。
- 后处理：常开 `Rgba16Float` 离屏 HDR 目标 + composite（曝光 → 色调映射 → FXAA）+ 可选三级泛光，
  缺省直通，见 [§17 HDR 后处理链](#17-hdr-后处理链)；可选屏幕空间遮蔽（两个全屏 pass）叠在 composite 之前，
  见 [§20 屏幕空间遮蔽](#20-屏幕空间遮蔽ssao扩展十七)。**无运动模糊、无 TAA、无自动曝光**。
- 遮蔽是**整幅相乘**的近似：没有只压环境项的版本、无多尺度、无双边滤波，快速转相机时靠 4×4 散焦压噪而非时间累积。
- 网格除四种程序化网格外，可加载外部 `.fbx`（含 base_color 贴图、材质因子、节点级 TRS 动画、
  骨骼蒙皮、交叉淡入与脚本状态机、分层遮罩混合，见 [§21 动画混合树](#21-动画混合树扩展十八-a)）；
  法线/粗糙度贴图、morph target 尚未支持。
  用法与阶段现状见 [`docs/ASSETS.md`](docs/ASSETS.md)。
- 每帧重建实例缓冲，物件量级到数千仍可运行；上万需改成持久化实例缓冲。
- 脚本单线程同步执行，长循环会卡帧（Rhai 已限制操作数上限）。
- 场景切换：**一个进程同一时刻只跑一个场景**。场景栈只是"来时的路"——每层存的是离开那一刻的
  **源文本 + 运行时快照**，不是后台还在跑的世界；切走的场景不会继续更新物理与动画。
  栈上限 16 层（超了丢最老的一层），切换是**同步**的（换资产库 + 重建世界 + 重跑脚本都在一帧内完成，
  大场景会有一次帧尖峰，无加载画面、无异步加载、无 `scene.forward` RPC）。
  脚本变量与随机序列不随存档还原，动态刚体回到声明位置后由 `on_start` 重跑决定，见
  [§22 场景栈与存档](#22-场景栈与存档扩展十八-b)。
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
交叉淡入（`set_anim_clip_fade`，smoothstep 双片段加权，位置/缩放 lerp + 旋转 slerp）与脚本层状态机；
分层混合树（遮罩 / 权重 / override 与 additive）由扩展十八 A 补上，见
[§21 动画混合树](#21-动画混合树扩展十八-a)。
尚未做：morph target、DQS、GPU skinning。

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
  "shadow": {
    "enabled": true, "size": 2048, "strength": 1.0,
    "cascades": 4,          // 扩展十六：级联数 1..=4（缺省 1 = 单级）
    "blend": 0.15,          // 级间过渡带宽（占该级跨度比例，0..0.5）
    "cascadeDistance": 60.0 // 阴影最远距离；0 / 缺省 = 用相机 zfar 或包围球最远点
  }
}
```

脚本侧八个开关：`set_shadow(on)` / `shadow_enabled()` / `set_shadow_strength(v)` /
`set_shadow_size(px)` / `set_shadow_extent(half)`（半宽写 0 = 自适应）/
`set_shadow_cascades(n)` / `set_shadow_blend(v)` / `set_shadow_distance(m)`。

**级联（扩展十六）**：`cascades > 1` 时阴影图变成 `texture_depth_2d_array`（层 = 级数），
视锥按深度切段、每级独立拟合光源正交相机并做纹素量化（相机移动时影子不抖动），
片元按相机深度选级、在 `blend` 带内与下一级混合。级数 1 就是阶段 D 的单级行为，
因此老场景、老截图口径不变；代价是 `draw call × 级数` 的顶点开销。

验证手段：`--run-seconds N` 的起止探针会打出
`阴影通道：级联 N / draw call X / 主通道实例 Y`，关闭阴影时三者中后两项为 0——
`draw call = 合批数 × 级数`（每级一次全场景深度 pass），这是「阴影通道真的跑了」的数值证据。
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

## 17. HDR 后处理链

§15 把光照换成了线性 HDR 辐亮度，但渲染色目标还是 8 位 sRGB 表面：超过 1.0 的部分写进去就被切平，
太阳盘、自发光、金属高光统统糊成一块死白。本节把这块接住——**常开一个 `Rgba16Float` 离屏目标，
主通道改写成线性 HDR，再由 composite 压回可显示范围**。

**数据流**：

```text
主通道 ──▶ Rgba16Float 离屏目标（线性 HDR）
           ├─(ssao 开启) 深度 → 半球核遮蔽 → 4×4 散焦 ──▶ ao 图（R8Unorm，全尺寸）
           ├─(bloom 开启) 亮部提取 → 1/2 → 1/4 → 1/8 三级降采样
           │                └─▶ 1/8 → 1/4 → 1/2 逐级加法回叠
           ▼
         composite：hdr·ao + bloom·intensity → 曝光 2^EV → 色调映射 → FXAA ──▶ sRGB surface
           ├─▶ 文本通道（Load）
           └─▶ UI 通道（Load）
```

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/postfx.rs` | `PostFxSettings` + `sanitized()` + `expose` / `bright_pass` / `tonemap` / `resolve` / `fxaa_edge_visible`（零 wgpu 纯函数） |
| 上屏 | `crates/r3e-render/src/postfx.rs` + `postfx.wgsl` | `PostFxPass`：离屏目标、三级泛光纹理、composite pipeline |
| 传输 | `crates/r3e-editor/src/lib.rs` | `PostFxPatch`（字段全可选）+ `scene.set_postfx` 解析 |

**语义**：`scene.postfx == None` 等价于 `PostFxSettings::default()`（曝光 0、曲线 `none`、无泛光、无 FXAA），
此时泛光链路整体跳过、composite 只做一次 clamp——**不写后处理段的老场景画面逐像素不变**。
代价是离屏 HDR 目标常驻显存（`Rgba16Float`，每像素 8 字节）。

**为什么文本 / UI 在 composite 之后**：这两个通道的顶点色是 LDR 用户数据（脚本里写的 `[1,0,0,1]`
就是想要的红色），不该被曝光与 ACES 二次调色；放在前面会让 HUD 随场景曝光一起漂移，编辑器里一眼可见。

**泛光固定三级**（1/2、1/4、1/8）：亮部提取 1 + 降采样 2 + 上采样 2 + composite 1 = 单帧最多 6 个全屏 pass
（`MAX_PASSES_PER_FRAME`）。再细的层级在小窗口上退化成一两个像素，只会把光晕抖成噪点。
软膝 `knee = 0` 走硬阈值（`KNEE_EPSILON` 兜除零），否则在阈值上下各 `knee` 的过渡带里平滑，避免光晕边缘出台阶。

**收敛**：曝光夹 `-8..16` EV、泛光阈值 `0..64`、软膝 `0..8`、强度 `0..4`，NaN 回落**缺省值**而非 0
（阈值收敛成 0 会把整幅画面当亮部，泛光变成一层灰雾）。三处写入路径（场景 JSON / 脚本 / 编辑器 RPC）
共用同一个 `sanitized()`。

**曲线名字的合法性由谁判定**：编辑器 RPC 只传字符串，`LocalEngineApi` 用 `TonemapMode::from_name`
解析，认不出就回 invalid params——前端拿到明确失败比静默变直通好；脚本侧相反，是热重载的运行时代码，
未知名回落 `none` 并打印一行，一个拼写错误不该让整个脚本中止。

**演示**：`assets/scene_hdr.json` —— 高亮自发光条 + 三档粗糙度金属球 + 环境探针，专门用来看亮部是否被切平：

```powershell
cargo run -- assets/scene_hdr.json
```

脚本侧等价写法：

```rhai
set_postfx(0.35, "aces", true, 0.25, true);
// 或分项改：
set_exposure(0.35);
set_tonemap("aces");
set_bloom(true, 1.0, 0.5, 0.25);
set_fxaa(true);
```

**探针日志**：`--run-seconds 2` 跑 `assets/scene_hdr.json` 得到

```text
[起始] 后处理：postfx 段 true / 曝光 +0.35EV / 色调映射 aces / 泛光 true / FXAA true /
       HDR 目标 1920x1080 / 泛光层级 [(960, 540), (480, 270), (240, 135)] /
       pass 泛光 5 + composite 1 / 重建 2 次
```

`pass 泛光 5` = 亮部 1 + 降采样 2 + 上采样 2，整条链确实提交了；`重建 2 次`是首帧建目标 + 一次窗口尺寸收敛。

**已知边界**：

- 无运动模糊、无 TAA；FXAA 是纯后处理，救不回已经丢失的几何信息（细长物体边缘仍会闪）；
- 泛光层级与核大小固定，不随分辨率自适应；小窗口下 1/8 层只剩几十像素；
- 曝光是手动旋钮，**没有基于平均亮度的自动曝光**（不做降采样统计 pass）。

**测试覆盖**：

- `cargo test -p r3e-core`：`postfx` 单测（`tonemap` 三档单调性与值域、亮部软膝连续、`resolve` 直通即 clamp、NaN 回落与范围夹取）；
- `cargo test -p r3e-core`：`tests/postfx_scene.rs`（缺省 `None`、场景载入收敛、JSON 往返）；
- `cargo test -p r3e-render`：`tests/postfx_contract.rs`（pass 预算、泛光层级除数、`PostFxParams` 64 字节布局、`tonemap_code` 编码）；
- `cargo test -p r3e-render`：`tests/postfx_shader.rs`（naga 解析 `postfx.wgsl` + ACES 五系数 / Rec.709 权重 / 泛光与 FXAA 阈值与 Rust 常量逐项比对）；
- `cargo test -p r3e-editor`：`tests/postfx_rpc.rs`（全量 / 局部 / 清空 / 未知曲线名 / 空补丁 / 类型错误）；
- `cargo test -p r3e-script`：`set_postfx` 一族写入、逐项组合、越界收敛、未知名回落、清空回读；
- 视觉观感（泛光是否"糊"、ACES 是否偏灰）**待人工窗口确认**：无截图 / 像素比对测试。

---

## 18. UI 指针交互（扩展十四）

§13 的 UI 层只负责「画」；本节补上「点」。三处写入路径共用同一份契约（`r3e-core::ui`），
渲染与脚本都不自带一套矩形换算。

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/ui.rs` | `UiRect` / `resolve_rect` / `hit_test` / `UiInteraction`（hover + active + 有界事件队列 `MAX_UI_EVENTS_PER_FRAME = 64`） |
| 驱动 | `crates/r3e-app/src/app.rs` | `update_ui_interaction()` 每帧喂光标与左键边沿；`cursor_over_ui()` 让相机让位 |
| 读取 | `r3e-script` / `r3e-editor` | 脚本 10 个只读 API；编辑器 `ui.state` |

**编辑器 RPC**（与 `scene.set_*` 同一套补丁风格，字段全可选）：

| 方法 | 参数 | 说明 |
| --- | --- | --- |
| `ui.list` | 无 | 返回全部 UI 元素（含解析后的像素矩形） |
| `ui.set` | `key` + 可选 `kind` / `anchor` / `x` / `y` / `width` / `height` / `color` / `layer` / `visible` / `image` / `uv` / `slice` | 新建或局部改。**给了才改**：未给字段保留当前值；`key` 缺失回 invalid params |
| `ui.remove` | `key` | 返回 `removed` 布尔 |
| `ui.state` | 无 | 指针状态：`hover` / `active` / `pointer_over_ui` / 本帧事件列表 |

长度写法与脚本一致：`"50%"` 百分比、`"120"` 等价 `"120px"`。`ui.set` 走 `ui::upsert`（裸全量覆盖），
**不走**脚本那条 `upsert_styled`——否则编辑器改 `layer` 会被「沿用旧值」静默吞掉。

**探针日志**（`cargo run -- assets/scene.json scripts/ui_interaction_demo.rhai --run-seconds 1`）：

```text
[起始] UI 通道：节点 5 / 绘制元素 5 / 四边形 11 / draw call 1 / 贴图 0 / 缺图回退 0 / 超限跳过 0 /
       开关 true / UI 在文本之上 false / 交互 hover [] active [] 事件 0 指针压 UI false
```

无人值守运行时没有鼠标输入，`hover [] active [] 事件 0` 是**正确**读数：说明状态机跑过、这一帧确实没有事件，
而不是没接上。人工验证要真动鼠标：悬停按钮变亮、点「开始」给 spinner 上色、点「重置」还原。

**测试覆盖**：

- `cargo test -p r3e-core --lib ui`：`resolve_rect` 锚点与单位、`hit_test` 取最上层 / 跳过不可见、状态机六类事件（含移出后释放走 `cancelled`、抬起后 hover 保持）；
- `cargo test -p r3e-core --test ui_scene`：场景层 `ui` 段缺省 / 部分字段 / JSON 往返 / 收敛 / 未知 kind 报错 / 命中与事件驱动；
- `cargo test -p r3e-app --bin r3e`：`ui_set` 插入与就地打补丁、未知 key 不改状态、`ui_list` / `ui_remove` 往返、`ui_state` 反映指针；
- `cargo test -p r3e-editor --test ui_rpc`：全量 / 局部 / 坏 key / 未知 anchor / 类型错误；
- 视觉与手感（hover 反馈是否跟手、点击命中是否够宽容）**待人工窗口确认**，无自动化截图比对。

---

## 19. 局部光源（扩展十五）

§15/§16 解决「整体明暗」，本节解决「一束光打在某个物件上」。**定长光源表**：16 盏灯整表打包进一个
uniform（`1 + 16×64 = 1040` 字节），片元里循环累加，不加 pass、不加 draw call。

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/light.rs` | `LocalLight` / `LocalLightKind` / `attenuation` / `spot_factor` / `irradiance`（CPU 参考，零 wgpu） |
| 打包 | `crates/r3e-render/src/renderer.rs` | `LightsUniform::from_lights(&scene.lights, scene.lights_enabled)`，每帧一次 `write_buffer` |
| 驱动 | `r3e-script` / `r3e-editor` | 脚本 19 个 API；编辑器 `light.list` / `light.set` / `light.remove` / `light.clear` / `light.set_enabled`（补丁式） |

**衰减与锥因子**（与 `shader.wgsl` 逐式对应）：

| 项 | 公式 |
| --- | --- |
| 衰减 | `(1 - (d/range)^4)² / (d² + 0.01)`，`d ≥ range` 归零（平滑窗逆平方，没有硬截断的圆环感） |
| 锥因子 | 内角内 1、外角外 0，中间 `smoothstep`；内外角相等时退化成硬边阶跃，避免除零 |

**绑定在 `@group(3) @binding(4)`**：WebGPU `max_bind_groups = 4`（索引 0..3），第一版写成 `@group(4)`
启动即 `wgpu Validation Error`。现在光源表与阴影贴图同组，`tests/local_lights_shader.rs` 断言
「无 `@group(4)`」「`shadow_layout` 含 binding 4」「无独立 `lights_bind_group`」三条一起守。

**总开关写 `count.x = 0`**：数据留在 uniform 里，片元循环上界 `min(count.x, 16)`，关掉即零次迭代。
片元侧三处早退（超半径 / 背面 / 锥外）保证没被照到的片元不为该灯付 GGX/Smith 的代价。

**探针**（`cargo run -- assets/scene.json scripts/lights_demo.rhai --run-seconds 3`）：

```text
[起始] 局部光源：场景 2 盏 / 上传 2 盏 / 开关 true
```

「场景」是 `Scene::lights.len()`，「上传」是 uniform 里的 `count.x`：两者不等就是被截断（超 16 盏）或被开关清零。

**脚本侧两个坑**（写演示脚本时踩到的）：`light_add_spot` 是 13 个**位置参数**，少给一个不会编译报错而是
`on_start` 里抛 `Function not found`；一行里串四段 `+` 拼接会撞 Rhai 的 `Expression exceeds maximum complexity`
（编译期失败、整脚本不执行）。所以 `on_start` 的报错现在也会打一行 `[script] on_start 出错: ...`。

**测试覆盖**：`cargo test -p r3e-core`（衰减 / 锥因子 / 收集与收敛 / 场景往返）、`-p r3e-render`
（`LOCAL_LIGHT_STRIDE == 64`、`LIGHTS_UNIFORM_SIZE == 1040`、wgsl 常量比对）、`-p r3e-editor --test light_rpc`、
`-p r3e-app`（`light_set` 插入与合并、坏 kind 无副作用、容量上限、空 key 拒绝）、`-p r3e-script --test scripts_compile`（`scripts/*.rhai` 全量编译守卫——Rhai 的表达式复杂度超限是**编译期**
失败，运行时只打一行 error 不中断，这条守卫上线当天就抓到 `ccd_demo.rhai` 早已坏掉）。光照观感**待人工窗口确认**。

---

## 20. 屏幕空间遮蔽（SSAO，扩展十七）

§15 的直接光照与 §16 的 IBL 都是**无限远**的环境光：墙角、台阶接缝、贴地物件底部没有接触阴影，
物件像浮在场景里。几何级阴影（§9）只解决方向光，绕不过去；屏幕空间遮蔽从**已经有的深度缓冲**
反推遮挡，代价只是两个全屏 pass。

`postfx.ssao.enabled` 缺省 `false`，整段跳过——composite 采样一张 1×1 白色占位图，
画面与扩展十六逐像素一致。

**数据流**（主通道之后、composite 之前）：

```text
主通道深度（Depth32Float，TEXTURE_BINDING）
   ▼
fs_ssao：反推视空间位置 → 邻域法线 → 16 槽半球核逐样本判遮挡 ──▶ ao_raw（R8Unorm）
   ▼
fs_blur：4×4 盒式散焦（压噪，不模糊接缝）
   ▼
ao_final ──▶ composite：color *= ao（在曝光之前）
```

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/ssao.rs` | `hemisphere_kernel` / `kernel_uniform` / `reconstruct_view_pos` / `normal_from_neighbors` / `is_occluded` / `ao_from_occlusion` + `SsaoSettings::sanitized`（零 wgpu） |
| GPU | `crates/r3e-render/src/ssao.rs` + `ssao.wgsl` | `SsaoPass`：参数块、深度/遮蔽图两个绑定组、生成与散焦两条 pipeline、`AO_FORMAT` / `AO_PASSES_PER_FRAME` |
| 接入 | `crates/r3e-render/src/renderer.rs` + `postfx.rs` | 主通道深度加 `TEXTURE_BINDING` 用法；`ssao.render(...)` 在 `postfx.render(...)` 之前；composite 绑 AO 视图（世代号判等） |
| 驱动 | `r3e-script` / `r3e-editor` / `r3e-app` | 12 个脚本 API；`scene.set_ssao` RPC；`[起始]/[收尾] 遮蔽探针` |

**为什么用深度重建，而不是几何法线**：主通道没有 MRT（多渲染目标），写 G-buffer 要改所有材质的
写入与 pass 布局；深度缓冲本来就存在，代价只是每个像素两次 `textureLoad`。
邻域法线由左右/上下重建位置的叉乘得到，退化（邻域共线、深度为无穷远）返回 `None`，该像素直接取
`ao = 1`——**天空不会刷出黑边**。

**遮挡判定三条同时成立**（`is_occluded`，与 `ssao.wgsl` 逐句对应）：

| 条件 | 理由 |
| --- | --- |
| `bias < 样本到中心距离 < radius` | 偏置以内是浮点误差与曲率（会形成自遮挡条纹 acne），半径以外属于别的物件 |
| `dot(sample - pos, normal) > 0` | 法线半球外是同一面墙的自身厚度，算进去等于给自己压暗 |
| 深度 `== 1.0`（远平面）→ 直接取 1 | 天空没有几何，采样它只会得到噪点 |

`ao = clamp(1 - 遮挡比例 · 强度, 0, 1)`；强度 1 时全遮挡归 0，夹取上下限收口在 `MIN_AO_FACTOR` 一处。

**半球核是确定性的**：`hash_u32`（splitmix32 变体）+ 极坐标构造（`theta = 2π·h1`、`r = √h2`）
得到均匀分布的方向，长度按 `lerp(0.1, 1.0, t)²` 靠原点加密。不用 `rand` 的原因有两条——
核必须**每帧一致**（否则遮蔽图逐帧闪烁），且必须能在 WGSL 里用同样的整数运算复现。
上限 16 个样本（uniform 数组定长，`kernel_uniform` 填零槽位、着色器按 `sample_count` 截断）。

**三处实现坑**：

1. **读写同一视图会触发 wgpu 校验**：生成 pass 写 `ao_raw`、散焦 pass 才是最终图，`SsaoPass` 因此
   持有两张 `R8Unorm` 全尺寸纹理。想省一张就得把散焦并进生成 pass，那样没有可加的邻域权重来源。
2. **`AO_FORMAT = R8Unorm`**：遮蔽系数只有一维、且经过 4×4 散焦，8 位足够；换 `Rgba16Float`
   会让这张全尺寸纹理带宽翻 8 倍，肉眼没有差别。
3. **composite 绑定组不能每帧重建**：`SsaoPass::generation()` 只在尺寸变化时 +1，`PostFxPass`
   拿它判等，仅在新世代时重建绑定组；否则每帧重建一个绑定组会把驱动打满。

**参数块 416 字节**（`2×mat4 + 16×vec4 + 2×vec4`，`const _: () = assert!(size_of::<SsaoParams>() == 416)`）：
`proj` / `inv_proj` 用于深度重建，`kernel[16]` 是视空间偏移，`screen = (1/w, 1/h, radius, bias)`，
`misc = (intensity, sample_count, 0, 0)`。字段顺序与 `ssao.wgsl` 的 `struct SsaoParams` 严格一致。

**代价**：每帧 2 个全屏 pass（`AO_PASSES_PER_FRAME = 2`）+ 一张全尺寸 `R8Unorm`（1920×1080 约 2 MB）。
没有多尺度、没有时间累积、没有双边滤波——**关掉即零成本**（`pass 0`，两张纹理常驻但不被写）。

**收敛**：半径 `0.01..8`、偏置 `0..0.5`、强度 `0..4`、样本 `4..16`；NaN 回落缺省值
（偏置收敛成 0 会立刻出现自遮挡条纹）。三条写入路径（场景 JSON / 脚本 / 编辑器 RPC）共用 `sanitized()`。
演示场景用 `radius 0.85 / samples 16 / intensity 1.6`——比缺省 0.6 / 8 / 1.0 重，是为了在 1 米级
的墙角尺度上肉眼能一眼看出差别。

**探针**（`cargo run -- assets/scene_ssao.json scripts/ssao_demo.rhai --run-seconds 2`，实测 GPU 为
RTX 5060 Laptop / Vulkan / DiscreteGpu，1920×1080）：

```text
[起始] 遮蔽探针：段内启用 true / GPU 启用 true / 样本 16/16 / pass 2 / AO 图 1920×1080 / 世代 3 / 半径 0.85 / 偏置 0.020 / 强度 1.60
[收尾] 遮蔽探针：段内启用 true / GPU 启用 true / 样本 16/16 / pass 2 / AO 图 1920×1080 / 世代 3 / 半径 0.85 / 偏置 0.020 / 强度 2.40
```

同场景把 `postfx.ssao.enabled` 改成 `false` 再跑一次，探针变成
`段内启用 false / GPU 启用 false / …… / pass 0`，其余通道的 pass 数与重建次数一字不动
（证明遮蔽是真的整条跳过，而不是跑到一半被 clamp）。「世代」稳定不增长，说明 AO 图没有逐帧重建。

**脚本侧两个坑**：`on_start` 里一次性串很多 `+` 拼接会撞 Rhai 的表达式复杂度上限；少给位置参数
不报编译错、只在 `on_start` 抛 `Function not found`。`ssao_bias` 目前**只读**，写入走场景
`postfx.ssao.bias` 或 RPC `scene.set_ssao`——不在脚本里改成可写，是为了不出现两套写语义。

**已知边界**：AO 乘**整幅画面**（不是只乘环境项），极端参数下会把直接光也一起压暗；没有多尺度
（大尺度遮蔽只能靠调半径）；没有时间累积，快速转相机时遮蔽图会跟着跳；屏幕外几何无法参与遮蔽，
视口边缘的物件少一圈接触阴影（保守取 1，不硬造黑边）。观感（墙角是否自然、散焦是否够）**待人工
窗口确认**——右键开关即可对比。

**测试覆盖**：`crates/r3e-core/src/ssao.rs` 内联 12 条（核确定性、半径夹取、重建容差随深度放宽、
偏离/朝内/朝外三种遮挡判定、`ao_from_occlusion` 的 NaN 与上下限）；`tests/ssao_contract.rs`（跨层常量
与 `ssao.wgsl` 的 416 字节/16 槽/`MIN_SSAO_SAMPLES` 比对）、`tests/ssao_scene.rs`（`postfx.ssao` 往返与
缺省中性）、`tests/ssao_shader.rs`（wgsl 结构守卫）、`tests/ssao_api.rs`（脚本 12 个 API 的读写与夹取）、
`tests/ssao_rpc.rs`（补丁式 RPC 与坏值无副作用），合计 32 条独立用例。

---

## 21. 动画混合树（扩展十八 A）

§8 的交叉淡入解决的是「A 动作换成 B 动作」：两段互斥，任意时刻只有一段占住整副骨架。
真实角色还要**同时**播几段——上半身挥砍、下半身走路，每段只管一部分骨骼。逐骨骼加权就是混合树。

实现落在 `crates/r3e-core/src/anim_blend.rs`（纯数据 + 纯函数，不碰场景与渲染），
`Scene` 上只加一层管理薄壳。**渲染侧零改动**：层改的是 `update_animations` 求出的节点 TRS，
蒙皮仍是同一套关节矩阵与同一条写关节的着色路径。

### 21.1 三层结构

| 结构 | 口径 |
| --- | --- |
| `BoneMask` | 按资产内**节点路径**决定谁参与本层：`pass_all`（整副）/ `only`（白名单）/ `except`（黑名单）。匹配**大小写不敏感**，支持**后缀片段**（规则 `hand` 命中 `root/arm_l/hand`） |
| `AnimLayer` | 一层 = 若干片段及层内权重 + 层权重（可淡入淡出）+ 模式 + 遮罩 + 速度。上限 `MAX_ANIM_LAYERS = 8`，每层最多 `MAX_LAYER_CLIPS = 8` 个片段 |
| `LayerMode` | `override`：命中遮罩的骨骼直接取层姿态；`additive`：层姿态**相对片段首帧**的增量叠到主姿态上（呼吸、受击抖动靠它） |

两种模式的合成公式（`base` 是主片段求出的姿态，`w` = 层权重 × 遮罩权重）：

```text
override : pose = lerp(base, layer_pose, w)
additive : pose = base + (layer_pose - reference) * w     // reference = 片段首帧姿态
```

### 21.2 求值顺序

`Scene::update_animations` 逐实例按下面顺序走：

1. 主片段采样（含交叉淡入的双片段加权）→ 得到 `base`；
2. 按挂载顺序遍历各层：层内多片段按**归一化权重**加权采样出一个层姿态；
3. 用该层遮罩把层姿态与 `base` 逐骨骼混合，写回节点本地 TRS；
4. 遮罩外的骨骼原样保留 `base`，所以「只动上半身」不会把腿的相位带偏。

### 21.3 几条硬口径

- **层内多片段共享同一条时间轴**：走跑混合要求相位同步，各算各的计时会滑步；
  不同层各有独立时间轴，层 `speed` 可单独调。
- **遮罩是二值的，不是软权重**：命中就参与、没命中完全不参与；要渐变请调**层权重**
  （`fade_anim_layer` 走 smoothstep）。
- **`except` 优先于 `only`**：两者同时给定时黑名单生效，用来表达「整副骨架但别碰手指」。
- **空遮罩 = 整副骨架**（`""` 与 `"*"` 同义），不是「什么也不动」，避免手滑清空遮罩把层变成空转。
- **模式串写错不静默降级**：`set_anim_layer_mode` 收到 `override` / `additive` 以外的值打日志并返回
  `false`，层保持原模式；层数超限同样返回 `false`，不做 LRU 淘汰。
- **层是运行时状态，不写回场景 JSON**：热重载重导资产后层随实例重建，脚本在 `on_start` 挂一次即可；
  没有任何层时，求值路径与老版本逐位一致。
- **只作用于被绑定的节点**：层作用在 `AnimInstance` 的绑定节点上（FBX 资产实例即节点树），
  对普通物件的 `transform` 脚本动画没有影响。

### 21.4 脚本 API（18 个）

实例名取自 `anims()`（`anim_info` 可查片段与播放进度）：

| 用途 | API |
| --- | --- |
| 挂层 | `add_anim_layer(inst, layer, clip, w)`（override）/ `add_anim_layer_additive(inst, layer, clip, w)`（additive） |
| 层内片段 | `add_anim_layer_clip(inst, layer, clip, w)` / `set_anim_layer_clip` / `remove_anim_layer_clip` / `anim_layer_clips(inst, layer)` |
| 调参 | `set_anim_layer_weight` / `set_anim_layer_mode(inst, layer, "override"\|"additive")` / `set_anim_layer_mask(inst, layer, "arm,hand")` / `set_anim_layer_speed` |
| 过渡 | `fade_anim_layer(inst, layer, w, dur)`：从当前权重起步，`dur` 秒到位 |
| 删层 | `remove_anim_layer` / `clear_anim_layers` |
| 查询 | `anim_layers(inst)` / `anim_layer_count` / `anim_layer_weight` / `anim_layer_mask` / `anim_layer_info(inst, layer)`（人读的一行报告） |

遮罩串的归一化口径（写进去什么，`anim_layer_mask` 就回显什么，方便热重载后比对）：

| 输入 | 含义 | 回显 |
| --- | --- | --- |
| `""` / `"*"` | 整副骨架 | `*` |
| `"arm,hand"` | 只这些节点（含其子节点） | `arm,hand` |
| `"!root"` | 除这些之外的全副 | `!root` |
| `"arm,!hand"` | 白名单先收，黑名单再扣（黑名单优先生效） | `arm,!hand` |

```rhai
// on_start：走路当主片段，上半身挂一层挥砍（只占 arm/hand，腿继续走 walk）
let inst = anims()[0];
add_anim_layer(inst, "upper", "atk", 1.0);
set_anim_layer_mask(inst, "upper", "arm,hand");
fade_anim_layer(inst, "upper", 0.0, 0.6);   // 先挂上不显形，需要时再淡入
```

```rhai
// on_update：按住左键挥砍层 0.18s 淡入，松开 0.25s 淡出（层不删，避免反复重建）
if is_mouse_down(0) { fade_anim_layer(inst, "upper", 1.0, 0.18); }
else                { fade_anim_layer(inst, "upper", 0.0, 0.25); }
```

### 21.5 编辑器 RPC

| 方法 | 用途 |
| --- | --- |
| `scene.animlayers` | 列实例名与每实例的层摘要（只读） |
| `scene.animlayerset` | 补丁式改单层：`weight` / `mode` / `mask` / `speed`；未知字段、未知模式、不存在的实例或层一律返回 `ok=false` 且**不落任何副作用** |

层只能在 RPC 里**调参**，不能凭空挂层——挂层要片段句柄，那是资产导入侧的职责，
塞进 RPC 会绕过实例生命周期管理。

### 21.6 已知边界

能表达「上半身攻击 + 下半身走路」，但**不能调整层序**（固定按挂载顺序叠加）、
**additive 参考帧固定为片段首帧**（不能指定任意参考姿势）、**没有 morph target**（表情 / 口型）、
**没有 IK 与脚部对齐**（斜坡上脚会浮）、遮罩是二值而非软权重。
这几条与 `docs/ARCHITECTURE.md` §11 的动画边界保持同一口径。

### 21.7 测试覆盖

新增 44 条用例，全 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 11 | override 端点、additive 增量、additive 旋转绕行、层内加权采样、缺轨道跳过、淡入曲线、时间推进与顺序、片段管理边界、模式解析 |
| `crates/r3e-core/src/scene.rs` | 7 | override 只动遮罩内骨骼、additive 叠加、淡入缩放贡献、层管理往返、层数上限、主片段切换后层存活、清空后恢复主姿态 |
| `crates/r3e-script/tests/anim_layers_api.rs` | 9 | 真跑 Rhai：遮罩生效、遮罩串解析回显、模式串校验、权重过渡推进、上限与实例缺失的中性返回 |
| `crates/r3e-app`（宿主补丁） | 6 | 补丁生效、未知模式无副作用、缺实例 / 缺层拒绝、列表截断但计数为总数 |
| `crates/r3e-editor/tests/anim_layer_rpc.rs` | 11 | `scene.animlayers` / `scene.animlayerset` 的往返、坏值与越界 |

`cargo test --workspace` 现为 48 个套件 / 673 passed / 0 failed。

---

## 22. 场景栈与存档（扩展十八 B）

前十七个扩展共享一个隐含前提：**一个进程从头到尾只跑一个场景**。要表达「主菜单 → 关卡 → 暂停 → 回关卡」，
缺的并不是"再解析一份 JSON"（`Scene::from_json_str` 早就会），而是三件配套的东西：
**来时的路**（栈）、**运行时的账**（快照）、**谁来完成这个动作**（宿主在帧首落地）。

### 22.1 三层结构

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/scene_stack.rs` | `SceneStack` / `StackEntry` / `RuntimeSnapshot` / `AnimRuntime` / `SceneArchive` / `SceneCommand`，纯数据 + 纯函数，唯一 IO 是存档读写盘 |
| 传输 | `crates/r3e-editor/src/lib.rs` | `EngineApi` 五个方法（`scene_stack` / `scene_switch` / `scene_back` / `scene_archive_save` / `scene_archive_load`，默认 `unsupported`）+ dispatch 分支 + 参数校验 |
| 宿主 | `crates/r3e-app/src/app.rs`、`crates/r3e-app/src/editor.rs` | `EngineApp::apply_pending_scene_command` 在帧首取命令并执行；`LocalEngineApi` 只做校验后塞槽，重活留在 app 侧完整上下文里 |

契约放在 core、传输只做校验、执行留在宿主——与前面十七个扩展同一套路数：
**core 不知道渲染器存在，editor 不知道场景怎么装，只有 app 两边都摸得到。**

### 22.2 栈的语义

`SceneStack` 是"光标 + 线性列表"，不是"场景池"：

| 操作 | 语义 |
| --- | --- |
| `push(entry)` | 压入并**截断游标之后的分支**（浏览器式：回退后再前进到别处，老前向就没了） |
| `back()` / `forward()` | **只移游标，不删层**——所以"回主菜单再回关卡"能把关卡那层原样捡回来 |
| `pop_current()` | 删当前层，游标退到前一层 |
| `jump(i)` | 按层号跳；越界返回 `false` 不动栈 |
| `find(name)` | 从当前层**往回**找（要找的是"我来的那个"），命中返回层号；同名层用 `unique_name` 加 `#2` 后缀 |
| `capture_current(&scene)` | **只刷新栈顶**（当前层）的快照，别层的快照不许被当前世界污染 |
| `MAX_SCENE_STACK = 16` | 超限丢**最老**一层（近处的手感比远处的历史重要） |

栈里存的是 `StackEntry { name, path, source, runtime }`：**离开那一刻的源文本 + 运行时快照**。
不是后台还在跑的世界——这是本节所有边界的根。

### 22.3 快照里放什么

`RuntimeSnapshot` 只补 `to_scene_file` **刻意省略**的那部分运行时：

| 字段 | 内容 |
| --- | --- |
| `time` / `frame` | 场景时钟与帧号 |
| `text` / `ui` / `lights` | 三个通道级开关（文本、UI、局部光源） |
| `ui_above_text` | 文本与 UI 的前后关系 |
| `anims` | 每个动画实例的 `name` / `time` / `speed` / `playing`（`AnimRuntime`） |

刻意**不放**：相机、方向光、物件（都在场景源里）、世界矩阵等每帧重算的派生量。
快照带 `version` 与 `is_compatible()`；`apply` 返回真正恢复的动画实例数，
名字对不上的实例**静默跳过**（资产换了、实例消失了是常态，不该报错炸掉读档）。

### 22.4 命令信封：为什么不当场切

换世界要动四样东西：资产库、场景 `Rc` 里的内容、渲染缓存、脚本状态。谁都不能只动一半。
所以脚本与 RPC 都**只提交意图**，由宿主在帧首统一落地：

| 环节 | 位置 | 行为 |
| --- | --- | --- |
| 提交 | `ScriptHost::scene_cmd`（单槽）+ `LocalEngineApi` 校验 | 槽被占返回 `false` / `-32002`，脚本据此知道"该等" |
| 取出 | `EngineApp::step_and_render` 帧首 → `apply_pending_scene_command` | 一帧最多执行一条 |
| 落地 | `apply_scene_command` → `enter_scene_source` | **固定顺序**：`refresh_assets_from_json`（换库）→ `scripts.rebuild_scene`（同一个 `Rc` 就地换内部值）→ 叠 `RuntimeSnapshot` |

- **单槽而不是队列**：`on_update` 每帧都可能调 `scene_switch`（血量 0 切结算），有队列会堆成无限待办，
  还会出现"一帧切了三个、最后停在第三个"的怪状。单槽被占就返回 `false`，把节奏交给调用方。
- **`ScriptCommand` 一次性**：`take_scene_command` 取走即清空，落地失败只记 `log::error!`，不重试——
  把同一条命令反复重试会让脚本以为自己还在申请。
- **`scene_save_archive` 是唯一不换世界的命令**，所以 RPC 侧它**当场写盘**再返回真实路径
  （调用方拿到"文件已存在"，而不是一句"排队成功"）。

### 22.5 脚本 API（9 个）

| 函数 | 说明 |
| --- | --- |
| `scene_switch(path) -> bool` | 申请切场景；`path` 空白或槽已占返回 `false` |
| `scene_back() -> bool` | 申请回上一层 |
| `scene_save_archive(path) -> bool` | 申请存档；`path` 传空串 = 宿主挑默认位置 |
| `scene_load_archive(path) -> bool` | 申请读档；`path` 空白返回 `false` |
| `scene_command_pending() -> bool` | 槽里是否已有待落地命令 |
| `scene_stack_names() -> array` | 层名数组（按压栈顺序） |
| `scene_stack_depth() -> int` | 层数 |
| `scene_stack_index() -> int` | 当前层号；**空栈返回 `-1`**，与"第 0 层"区分开 |
| `scene_stack_current() -> string` | 当前层名；空栈返回 `""` |

### 22.6 编辑器 RPC（5 条）

| 方法 | 用途 |
| --- | --- |
| `scene.stack` | 只给**声明态**（不读盘）：`index` / `name` / `path` / `source_bytes` / `frame` / `time` / `current`，外加 `count` / `current` / `max`；支持 `limit`（截断 entries，`count` 仍是总数） |
| `scene.switch` | `path` 必填且文件须存在，回 `{queued, command, stack}` |
| `scene.back` | 无参；已在最早一层**当场报错**（`-32000`），而不是"排队成功但下一帧静默失败" |
| `scene.archive.save` | `path` 可省（省则与场景同目录、同名 `.archive.json`）；**当场写盘**，回 `{path, format, version, bytes, objects, anims}` |
| `scene.archive.load` | `path` 必填，回 `{queued, command, stack}` |

路径解析规则两处一致（`resolve_request_path` / `resolve_scene_path`）：绝对路径照用；
相对路径先按进程工作目录，落空再按**当前场景文件所在目录**——编辑器面板里填 `lv2.json`，
用户心里那个"当前目录"是场景目录，不是启动目录。

### 22.7 已知边界

- **一个进程同一时刻只跑一个场景**：切走的场景不会继续更新物理与动画，栈层是冻结的源文本 + 快照。
- 切换是**同步**的：换库 + 重建 + 重跑脚本都在一帧内完成，大场景会有一次帧尖峰；
  无异步加载、无加载画面、无 `scene.forward` RPC（回退后再前进请重走 `scene.switch`）。
- 读档/切场景后**动态刚体回到声明位置**（rebuild 重挂刚体、generation 自增），位置由 `on_start` 重跑决定。
- **脚本变量、随机序列、`on_update` 局部状态不随存档还原**：只还原场景源 + 上述运行时快照。
- 读档**不接管 `scene_path`**：热重载仍盯原来那个场景文件，存档是一次性输入，不是新的"当前场景来源"。
- 栈上限 16 层，超限丢最老一层；同一进程内无法同时观察两个场景的画面。

### 22.8 测试覆盖

新增 45 条用例，全 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/scene_stack.rs` | 27 | 快照 5（默认值对齐新场景 / JSON 往返 / 覆写开关与时间 / 未知实例跳过 / 拒绝未来版本）、存档 6（捕获源 + 运行时 / 纯数据模式省运行时 / 往返恢复场景与时钟 / 接受裸场景文件 / 版本与坏 JSON 拒绝 / 磁盘往返）、栈 11（空栈 / push 成当前 / back-forward 只移游标 / 回退后 push 截断前向分支 / 超限丢最老 / find 从当前往回 / jump 越界 / pop 移动游标 / unique_name 加后缀 / capture 保留源与运行时 / clear）、命令 5（标签稳定 / 路径跟随变体 / 只有 SaveArchive 不换世界 / 文本逐字携带 / capture_current 只刷栈顶） |
| `crates/r3e-script/src/lib.rs` | 7 | 真跑 Rhai：切场景排入 `Switch`、路径去空白与空白拒绝、单槽拒绝第二条且不覆盖、空串存档语义、栈读取函数跟着共享句柄、命令跨 rebuild 存活、宿主取用同一槽 |
| `crates/r3e-app/src/editor.rs` | 4 | 默认存档路径后缀 / 相对路径解析（绝对与原样保留）/ 落到场景目录 / 存档写盘后带运行时状态读回 |
| `crates/r3e-editor/src/lib.rs` | 7 | `scene.stack` 的 `limit` 透传与缺省、`scene.switch` 空路径拒绝、`scene.back` 无参、`scene.archive.save` 的路径语义、`scene.archive.load` 必须给路径、五个方法默认 `unsupported` |

`cargo test --workspace` 现为 48 个套件 / 673 passed / 0 failed。

---

*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
