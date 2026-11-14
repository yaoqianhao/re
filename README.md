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
│   ├─ scene_local_shadow.json # 局部光源阴影演示场景（点光 + 聚光各一盏，均开 cast_shadow）
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
│   ├─ blend_space_demo.rhai  # 1D / 2D 混合空间（推参数看权重插值 + 越界钳制 + 重复样本被拒）
│   ├─ layer_sync_demo.rhai   # 层内相位同步（前 1 秒 off 后切 phase，看两路 drift 归零）
│   ├─ phase_group_demo.rhai  # 相位同步组（错步偏移 + 跨层对齐，三段切换看落点搬家）
│   ├─ clip_playback_demo.rhai # 片段播放参数（三段：原速 / 2 倍速非循环停末帧 / 清除回原速）
│   ├─ blend_space_3d_demo.rhai # 三维混合空间（四面体权重插值 + 越界钳到凸包面 + 重复样本被拒）
│   ├─ blend_cache_demo.rhai  # 剖分缓存（装配期只置脏标记 + 显式提交 + 补样本后自动提交）
│   ├─ shadow_demo.rhai       # 阴影演示（光源扫动 + 强度呼吸 + 右键开关）
│   ├─ ssao_demo.rhai         # SSAO 演示（强度呼吸 + 1/2/3 切半径 + 4/5 切样本 + 右键开关）
│   └─ local_shadow_demo.rhai # 局部光源阴影演示（点光绕圈 + 聚光摆头，0.5s 打印投影盏数）
├─ crates/
│   ├─ r3e-core/              # 数学 / 节点 / 场景 / 相机 / 光照
│   ├─ r3e-render/            # wgpu 渲染器、程序化网格、WGSL 着色器
│   ├─ r3e-physics/           # 物理（PhysicsBackend trait + Null / Builtin 实现 + 开关）
│   ├─ r3e-script/            # Rhai 引擎、API 绑定、热重载宿主
│   ├─ r3e-audio/             # 音频（rodio 后端 + 静默降级 + 混音门面 + 3D 空间化）
│   ├─ r3e-editor/            # 编辑器 RPC 协议（EngineApi / dispatch）+ TCP 桥
│   ├─ r3e-plugin/            # IDE 插件进程（工具 schema + 读写侧工具集，JSON-RPC over stdio）
│   └─ r3e-app/               # 主循环、窗口事件、终端指令
└─ docs/
    ├─ ARCHITECTURE.md        # 架构总览：crate 边界、数据流、扩展路线
    ├─ PHYSICS.md             # 物理子系统：三层开关、场景字段、脚本 API、内置规则
    └─ ASSETS.md              # FBX 资产加载（P0–P4 已落地，含骨骼蒙皮）
```

workspace 共 9 个 crate，各 crate 单向依赖：`app → editor / render / script / physics / audio → core`，核心层不认识 wgpu。

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

**完整字段契约**：上面的示例只是子集。全字段（含取值范围、枚举、默认值）见
`assets/scene.schema.json`（JSON Schema 2020-12），由 `r3e-core` 的 serde 结构派生，
`cargo run -p r3e-core --bin scene_schema_dump` 重新生成——改了场景结构却没重生成会让
`cargo test` 直接红灯（防漂移测试）。注意两个语义：字段**全部可省略**（省略取默认值）；
**未知字段被静默忽略**，所以拼错字段名不会报错，判断字段是否存在要以 schema 的 `properties` 为准。

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
| `morph_set(name, index, weight) -> bool` | 设第 `index` 个形变形状的权重（扩展三十六，`index` 取 0..3，非有限值 / 越界拒绝） |
| `morph_reset(name) -> bool` | 所有形状权重归零（回到绑定姿态） |
| `morph_active(name) -> bool` | 是否任意形状权重非 0（脚本据此决定要不要每帧刷） |
| `morph_bind(name, count) -> bool` | 手动登记形状数。**一般不用**：形状数是网格属性，引擎在场景就位与热重载后按网格的 `MorphSet` 自动登记 |
| `morph_count(name) -> int` / `morph_weight(name, index) -> float` | 读回形状数与权重（诊断用） |
| `morph_exclusive(name, index, group) -> bool` | 把第 `index` 个形状挂进互斥组 `group`（扩展三十八，`group` 取 `-1`（退组）或 `0..=254`，同组形状的正权重之和 > 1 时在**渲染层**等比压回 1，场景里存的值不变；扩到 12 个形状后一组可横跨三组权重） |
| `morph_group(name, index) -> int` | 读回该形状所在的互斥组，`-1` 表示没挂组 |
| `morph_norm(name, index) -> float` | 读回**归一化之后**实际参与计算的权重（不挂组时与 `morph_weight` 相同） |
| `morph_mask(name) -> int` | 读回该实例的形状掩码（扩展四十，第 `i` 位 = 形状 `i` 启用；已按网格形状数裁剪，未登记形状时为 0） |
| `morph_mask_set(name, mask) -> int` | 写形状掩码，返回**裁剪后实际生效**的值（超过形状数的位被丢掉）。**只做取舍、不改权重**：写 `0` 等于整段摘掉形变分支 |
| `morph_show(name, index, bool) -> bool` / `morph_visible(name, index) -> bool` | 单形状开关 / 读回（等价于 `morph_mask_set` 里翻第 `index` 位，越界拒绝） |

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

### 遮挡剔除（扩展二十二）

`occlusion` 段缺省 `enabled: false`，整段跳过，画面与扩展二十一逐像素一致。
开启后用**上一帧深度金字塔**在 CPU 侧判「物件整块被挡住」，只影响不透明批次的绘制。

| 函数 | 说明 |
| --- | --- |
| `set_occlusion_enabled(on)` | 总开关（打开后先有几帧在等深度读回，探针会显示「开关 true / 生效 false」） |
| `occlusion_enabled() -> bool` | 当前开关 |
| `set_occlusion_bias(v) -> f64` | 深度偏置，夹 `0..0.5`（默认 0.05；**越大约保守**，返回夹取后的实际值） |
| `occlusion_bias() -> f64` | 当前偏置 |

细节（金字塔、两阶段判定、读回布局、代价与边界）见 [§26 遮挡剔除](#26-遮挡剔除扩展二十二)。

### HDR 立方体反射探针（扩展二十三）

`environment_map` 段缺省不写，整段跳过，镜面反射仍走解析 IBL（三色天空 + SH 9），画面与扩展二十二逐像素一致。
写了来源（等距柱状 `.hdr` 或立方体六面）后，金属 / 光滑材质改用**离线预滤波的 mip 链**采样，粗糙度决定取哪一层。

| 函数 | 说明 |
| --- | --- |
| `set_env_map(path) -> bool` | 等距柱状 HDR（Radiance RGBE），相对路径以场景文件所在目录为基准；空串 = 清掉探针 |
| `set_env_map_faces(array) -> bool` | 立方体六面，顺序固定 `+X -X +Y -Y +Z -Z`，少于 6 条整段作废并返回 `false` |
| `set_env_map_intensity(v) -> f64` | 探针总强度，只乘环境反射项（与 `set_env_intensity` 互不影响），返回夹取后的实际值 |
| `set_env_map_enabled(on) -> bool` | 开关；`false` 只是不生效，路径仍留着，便于 A/B |
| `clear_env_map() -> bool` | 清掉来源（等价 `set_env_map("")`） |
| `has_env_map() -> bool` | 当前是否真的生效（有来源 + `enabled`） |
| `env_map_source() -> string` | 来源标签：`none` / `disabled` / `equirect` / `faces` |
| `env_map_intensity() -> f64` | 回读强度（没配探针时给缺省 1.0） |

细节（预滤波、mip 取层、绑定槽位、失败回退与边界）见 [§27 HDR 立方体反射探针](#27-hdr-立方体反射探针扩展二十三)。

### 级联遮挡体按级裁剪与层级剔除（扩展二十四 / 二十五）

`casterCull` 缺省 `true`，但**缺省不改画面**：外扩量 `casterExtend` 缺省 0 时拟合矩阵与扩展二十三逐位相同，
只是每级少提交不可能投到本级的遮挡体。写场景 `light.shadow` 段或调下面三个函数都可。
扩展二十五把判定本身换成按场景树自顶向下（不新增开关，掩码逐位不变），细节见
[§9 方向光阴影](#9-方向光阴影阶段三-d-已完成)。

| 函数 | 说明 |
| --- | --- |
| `set_shadow_caster_extend(m) -> f64` | 级联近端沿光方向外扩米数，夹到 `0..=65536`；`0` = 紧贴切片（缺省，屋檐 / 天桥可能丢影子） |
| `set_shadow_caster_cull(on) -> bool` | 按级裁剪开关，返回实际值；`false` = 回到「每级全量提交」 |
| `shadow_caster_cull() -> bool` | 回读开关 |

细节（判定保守性、掩码与实例缓冲对齐、段数与 draw call、实测裁剪量）见
[§9 方向光阴影](#9-方向光阴影阶段三-d-已完成)。

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

### 音频：音效 / 音乐 / 3D 空间化

音频走独立 crate `r3e-audio`，**不碰场景图**：`core` 里没有任何声音对象，脚本用「自己起的 key」管理实例。
默认是**平面声**；把某个 key 注册成 3D 声源后，它才会按「到听者的距离」衰减、按「相对速度」做多普勒。
不做 HRTF / 左右声像 / 遮挡（听者朝向只记录、不参与增益），细节见 [§14](#14-音频播放非空间化--3d-空间化)。

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

**3D 空间化（路线表第十项，24 个 API，纯叠加在平面声之上）**

没调过 `audio_spatial_attach*` 的 key 音量与历史版本逐位一致；注册之后才乘距离增益、下发多普勒倍速。

| 函数 | 说明 |
| --- | --- |
| `audio_spatial_attach(key, min, max)` | 注册为 3D 声源（`inverse` 模型 + 多普勒强度 1.0）；已注册则只更新设置、保留位置 |
| `audio_spatial_attach_ex(key, min, max, model, doppler)` | 指定距离模型（`inverse` / `linear` / `exponential`，别名 `inv` / `lin` / `exp`）与多普勒强度（`0` = 关） |
| `audio_spatial_attach_node(key, node, min, max)` | 注册并**绑到场景节点**：位置 / 速度每帧由引擎从节点世界变换灌入 |
| `audio_spatial_detach(key)` / `audio_spatial_clear()` | 解除注册 / 全清；**声音继续播**，只是回到平面音量 |
| `audio_spatial_set_position(key, x, y, z)` | 手动给坐标（绑定节点的会被引擎覆盖） |
| `audio_spatial_set_model(key, model)` / `audio_spatial_set_doppler(key, f)` | 只改一个旋钮，其余设置保留 |
| `audio_spatial_count()` / `audio_spatial_keys()` | 已注册数量 / 全部 key（上限 32，满员拒绝新注册，不挤掉老的） |
| `audio_spatial_gain(key)` / `audio_spatial_rate(key)` / `audio_spatial_distance(key)` | 探针：当前距离增益（`1.0` = 没衰减）/ 播放倍速（`1.0` = 相对静止）/ 到听者距离（米） |
| `audio_spatial_position(key)` / `audio_spatial_model(key)` / `audio_spatial_doppler(key)` / `audio_spatial_node(key)` | 回读坐标 `[x,y,z]` / 模型名 / 多普勒强度 / 绑定节点名 |
| `audio_spatial_set_listener(x,y,z, fx,fy,fz)` / `audio_spatial_listener()` | 手动给 / 读听者位姿（引擎每帧用**活动相机**覆盖，脚本一般不用写） |
| `audio_spatial_set_enabled(on)` / `audio_spatial_enabled()` | 空间化总开关；关闭后一切增益与倍速回 `1.0`（等价平面声） |
| `audio_spatial_tick(dt)` | 手动推进一帧并返回声源数（引擎每帧自动调，留给无窗口测试） |
| `audio_spatial_status()` | 一行摘要：开关 / 听者坐标 / 声源数（上限）/ 倍速可用性 / 最近声源 |
| `audio_supports_speed()` | 后端能否实时变速（静默降级时 `false`，此时多普勒只算不下发） |

听者不用脚本管：用 WASD/QE 飞、鼠标转头，远近关系就跟着画面走。完整演示见
`assets/scene_spatial_audio.json` + `scripts/spatial_audio_demo.rhai`
（`cargo run -- assets/scene_spatial_audio.json scripts/spatial_audio_demo.rhai --run-seconds 8`）：
`horn` 绕圈巡游（绑节点 + `inverse` + 多普勒 1.0），`bell` 静止（手动坐标 + `linear` + 多普勒关）。

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
层内「参数 → 权重」走 6 个 `anim_layer_blend*` API（扩展四十五：1D 线性 / 2D 重心插值），
见 [§29 1D / 2D 混合空间](#29-1d--2d-混合空间扩展四十五)。

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
  参数 metallic / roughness / emissive）。环境探针是解析式（三色天空 + 太阳 + 9 项球谐），另有 **HDR 立方体反射探针**
  （扩展二十三：离线预滤波 mip 链，配了探针时金属能映出探针里的环境、镜面按粗糙度取 mip），
  但探针是**静态**的——不抓取运行中场景、没有平面反射与 SSR，物件动起来不会出现在金属上，见
  [§16 IBL 环境探针](#16-ibl-环境探针) 与 [§27 HDR 立方体反射探针](#27-hdr-立方体反射探针扩展二十三)。方向光带单张正交阴影贴图（3×3 PCF，默认开启）；
  扩展十六把这张图升级为**最多 4 级级联**（`light.shadow.cascades`，缺省 1 = 单级，老场景画面不变）：
  视锥按深度切段、每段独立拟合光源正交相机，近处纹素更密、远处只求覆盖，级间按 `blend` 混合消接缝。
  阴影限制：级联上限 4、透明物件也投实心影；扩展二十四起每级只提交落在本级覆盖范围内的
  遮挡体（`casterCull`，缺省开，保守裁剪不改画面），扩展二十五起判定按**场景树层级**自顶向下做
  （整棵子树落在级外就跳过，只有叶子才逐实例判；层级平坦的场景净收益为 0），且 `casterExtend`
  缺省 0 时紧贴级边界的遮挡体（屋檐 / 天桥 / 二层地板）可能丢影子，见
  [§9 方向光阴影](#9-方向光阴影阶段三-d-已完成)。
- 主通道视锥剔除自扩展二十九起同样走**场景树层级**（整枝跳过，相机背后成组的实例不再逐个判），
  遮挡剔除（扩展二十二）自扩展三十起也走**同一棵场景树**（子树盒判为被挡，则其所有子实例必然被挡，
  整枝跳过、不再逐个查金字塔）；两层都复用上一帧金字塔，所以**物体瞬移那一帧**（金字塔落后一帧）
  可能出现“闪一下”——整枝误判的代价是一整枝在画面里短暂缺失，而逐实例时代价只是一个实例，这是
  层级化的取舍，需要肉眼确认观感；层级平坦或实例很少时层级化是净亏（多存一份中间态），见
  [§9 方向光阴影](#9-方向光阴影阶段三-d-已完成) 末段。
- 局部光源：最多 16 盏点光 / 聚光，扩展十八 C 起其中**每帧最多 4 盏**可投影（`cast_shadow`，
  点光 6 面 / 聚光单面，共用一张固定 512² × 24 层深度数组）；无 IES / 面光源，只进直接光项、
  不参与环境项，见 [§19 局部光源](#19-局部光源扩展十五) 与 [§23 局部光源阴影](#23-局部光源阴影扩展十八-c)。
- 后处理：常开 `Rgba16Float` 离屏 HDR 目标 + composite（曝光 → 色调映射 → FXAA）+ 可选三级泛光，
  缺省直通，见 [§17 HDR 后处理链](#17-hdr-后处理链)；可选屏幕空间遮蔽（两个全屏 pass）叠在 composite 之前，
  见 [§20 屏幕空间遮蔽](#20-屏幕空间遮蔽ssao扩展十七)。**无运动模糊、无 TAA、无自动曝光**。
- 遮蔽是**整幅相乘**的近似：没有只压环境项的版本、无多尺度、无双边滤波，快速转相机时靠 4×4 散焦压噪而非时间累积。
- 像素级回归（[§24](#24-captureframe-像素级回归扩展二十)）是**逐像素**的：只覆盖单一夹具场景的 640x360 单帧，跨 GPU / 驱动 /
  系统字体渲染不保证一致，换机器要先 `R3E_UPDATE_BASELINE=1` 重建；无 GPU 的机器整条用例自动跳过（计 passed），
  等于那台机器没有这层保护。动画、输入、交互、编辑器 RPC 产出的图都不在基线内。
- 网格除四种程序化网格外，可加载外部 `.fbx`（含 base_color 贴图、材质因子、节点级 TRS 动画、
  骨骼蒙皮、交叉淡入与脚本状态机、分层遮罩混合，见 [§21 动画混合树](#21-动画混合树扩展十八-a)）；
  法线 / 粗糙度 / AO 贴图已支持（扩展二十一，见 [§25](#25-材质数据贴图法线--粗糙度--ao扩展二十一)），
  morph target（表情 / 口型形变，blend shape）已支持（扩展三十六，切线通道扩展三十七，见
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §4.20 / §5.28），metallic 贴图仍不支持。
  用法与阶段现状见 [`docs/ASSETS.md`](docs/ASSETS.md)。
- **退化缩放（某轴 `scale` = 0）不再产 `NaN`**（扩展二十六）：世界法线矩阵改由 `r3e_core::world_normal_matrix` 给出——矩阵可逆时与原写法逐位相同，不可逆时退化为「逐列方向 × 缩放倒数」的有限兜底；主通道 `vs_main` 在 `normalize` 前另加一道长度判零，退化时退回 `+Y`。同时 `Transform::normal_matrix` 不再用 `scale.max(1e-6)` 把负缩放抹平，镜像物件的法线方向得以保留。代价与边界：缩放到 0 的物件本身仍是退化物体，拿到的是「最不坏的有限值」而不是有意义的几何；正常场景画面逐步（逐位）不变，像素基线无需重建。
- 实例缓冲已持久化、上传按脏段（扩展三十一）：缓冲跨帧复用，另有 148 B/实例的 CPU 影子存上一帧真正传过的字节，每帧只把变了的记录按连续区间 `write_buffer`（空洞 ≤ 8 条合并、段数 > 24 退全量、实例数变化或扩容换代当帧全量）——1,537 实例静态场景实测 `脏段 0 / 写 0 / 跳过 1,539`（0 B/帧），只转两个物件时 3 条记录。实例收集本身是「一趟 `scene.iter()` + 按（网格, 材质, 段）分桶」（扩展二十七 / 二十八，节点只进自己那一段），省掉的是旧写法按「键数 × 节点数」的重复过滤与同键跨两段的重复绘制；视锥剔除自扩展二十九起按场景树整枝跳判，遮挡剔除自扩展三十起同样按场景树整枝跳判。**仍在的热点**：实例装配仍逐实例（掩码要一格对一格对齐缓冲）、每帧一次 O(实例数) 的 memcmp、半透明批次重排会把这层省下的带宽吃掉大半。关节矩阵自扩展三十二起也走同一个 `plan_upload`（stride 64 B）：完全静止的蒙皮 0 B/帧，动画播放中 37 条连续脏区合并成 1 段（`write_buffer` 调用 37 次 → 1 次）；自扩展三十三起求解改走「骨骼树顶点为基准的常量相对链」（不含实例变换 `X`），「骨骼不动、整只模型在转」也从全量变成 0 B/帧（浮点伪脏消除），断链时回退世界矩阵、保守判全。自扩展三十五起这些脏段不再直传：先拷进 3 槽暂存环、一帧只 `write_buffer` 一次，再逐段 `copy_buffer_to_buffer` 落位（40 实例 / 每帧 4 段实测 4 次写 → 1 次写，`R3E_UPLOAD_STAGING=0` 可关）。细节见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §5.23 / §5.24 / §5.25 / §5.27 / §8.26 / §8.27 / §8.28 / §8.30。
- **形变（morph target，扩展三十六 + 三十七）**：位置 / 法线 / 切线三条增量通道（切线段自扩展三十七进表，每顶点 9 个 `f32`；UV 仍不参与形变）；
  每实例上限 **12 个形状**（三组 `vec4` 权重，扩展三十九建的机制 / 扩展四十一把组数抬到 3；`MORPH_WEIGHT_GROUPS × 4`，超出的形状静默不入表）；权重不夹 0..1、默认不归一化（允许负权重做反向形状，
  也允许 >1 的夸张表情）；**同网格的实例共享同一段形状增量数据，但各自带一张形状掩码**（扩展四十）——
  默认全开，脚本用 `morph_mask_set` / `morph_show` 只留自己那套形状，被掩掉的形状权重原样保着但不再进乘法，掩码全 0 的实例整段跳过形变分支；
  仍不能逐实例换**增量数据**（那需要换网格）；表情互斥由扩展三十八补上——
  可给形状挂互斥组（`morph_exclusive`），同组正值之和 > 1 时渲染层等比压回 1（0.5 + 0.5 ≡ 只开一个），场景权重本身不改写。所有带形状网格的增量常驻一块
  上限 1 MiB 的全局表（每顶点 9 个 `f32` = 位置 3 + 法线 3 + 切线 3，1 MiB 约装 7,281 顶点 × 4 形状；
  切线进表后同容量能装的顶点数比扩展三十六少了 1/3），引擎目前没有网格卸载路径，这份显存只增不减（换场景靠整表重建回收）。
  权重缺省全 0，所以不带表情的场景像素与扩展三十五逐位相同；夹具无 `TANGENT` 目标时切线段全 0，画面与扩展三十六逐位相同。
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
形变（morph target / blend shape）由扩展三十六补上、扩展三十七把切线段并入同一张表、扩展三十八补上互斥组归一化、扩展三十九把形状数从 4 抬到 8（多挂一组 `vec4`）、扩展四十补上逐实例形状掩码、扩展四十一再把权重组数抬到 3（12 个形状）：形状增量按网格上传进全局表，
实例只带 12 个权重 + 12 个互斥组编号 + 1 个形状掩码（实例记录 112 → 128 → 132 → 148 B），顶点着色器里**先叠增量、再做蒙皮**（见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §5.28）。
互斥归一化只在渲染层算副本：`prepare_instances` 第一遍把权重定稿，剔除外扩与着色器吃同一份，剔除余量按**逐形状**最大位移加权求和
（`Σ|w_t|×d_t`），比扩展三十六用峰值权重一律按最大形状外扩更紧。
尚未做：DQS、GPU skinning。

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
    "cascadeDistance": 60.0, // 阴影最远距离；0 / 缺省 = 用相机 zfar 或包围球最远点
    "casterExtend": 12.0,   // 扩展二十四：遮挡体沿光方向的外扩量；缺省 0 = 紧贴切片
    "casterCull": true      // 扩展二十四：按级裁剪遮挡体；缺省 true（保守裁剪，不改画面）
  }
}
```

脚本侧十一个开关：`set_shadow(on)` / `shadow_enabled()` / `set_shadow_strength(v)` /
`set_shadow_size(px)` / `set_shadow_extent(half)`（半宽写 0 = 自适应）/
`set_shadow_cascades(n)` / `set_shadow_blend(v)` / `set_shadow_distance(m)` /
`set_shadow_caster_extend(m)` / `set_shadow_caster_cull(on)` / `shadow_caster_cull()`。

**级联（扩展十六）**：`cascades > 1` 时阴影图变成 `texture_depth_2d_array`（层 = 级数），
视锥按深度切段、每级独立拟合光源正交相机并做纹素量化（相机移动时影子不抖动），
片元按相机深度选级、在 `blend` 带内与下一级混合。级数 1 就是阶段 D 的单级行为，
因此老场景、老截图口径不变；代价是 `draw call × 级数` 的顶点开销。

**按级裁剪遮挡体（扩展二十四）**：每级只给「真的能把影子投进本级」的实例提交深度 pass。
判定在 core 侧的纯函数里（`aabb_in_cascade`：把遮挡体 AABB 八个角点换到该级光源裁剪空间，
`x/y` 全在外或 `z > 1` 即裁；靠近光源那一侧 `z < 0` **一律不裁**——这是保守方向，
级边界外站在光源一侧的物件仍会把影子投进来）。拿不到 AABB 的实例走
`fail_open_bounds`，任何级都不裁。

两个旋钮：

| 字段 / API | 缺省 | 作用 |
| --- | --- | --- |
| `casterCull` / `set_shadow_caster_cull(on)` | `true` | 关掉即回到「每级全量提交」（扩展二十三口径） |
| `casterExtend` / `set_shadow_caster_extend(m)` | `0` | 把级联近端朝光源推 `m` 米，让「贴着级边界、影子落进本级」的遮挡体进深度图 |

`casterExtend` 缺省 0 是**兼容优先**的选择：此时矩阵与拟合结果和扩展二十三逐位相同
（`fit_cascade_extended(.., 0.0)` == `fit_cascade(..)`，有单测钉住）。代价是屋檐、天桥、
二层地板这类「接收区上方、朝光源方向超出切片」的遮挡体可能丢影子——真实场景建议给
`casterExtend` 一个接近该级跨度的值（例：4 级 + `cascadeDistance` 60 时取 10~15）。

实测裁剪量（4 级 / 32 个沿视线均匀分布的遮挡体，最不利于裁剪的分布）：
逐级保留 `[9, 19, 32, 32]`，合计 92 / 全量 128，省 28%；物件铺开在 x/y 上时近级裁得更狠。

验证手段：`--run-seconds N` 的起止探针会打出
`阴影通道：级联 N / draw call X / 主通道实例 Y / 遮挡体判定 A 裁掉 B`，关闭阴影时 draw call
与主通道实例为 0——`draw call` 由 `合批数 × 级数` 变成「每级保留实例切出的连续段数之和」
（`A = 级数 × 遮挡体数`，`B` 是被裁掉的对数），这是「阴影通道真的跑了、且真的裁了」的数值证据。
分段依据是实例缓冲里保留项的**连续段**：零散分布时切出的段数会变多，段数超过保留个数本身
就说明该场景按级裁剪并不划算（此时关掉 `casterCull` 更快）。

设计细节与踩坑（独立 pipeline layout、深度图不可原地 resize）见
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) 5.7 与 [`docs/ASSETS.md`](docs/ASSETS.md) 17。

**判定走场景树层级（扩展二十五）**：同样是 `casterCull` 那一层的事，不新增开关。判定前先按场景
父子关系聚出每棵子树的并集包围盒，自顶向下判——子树盒整块落在级外就**整枝跳过**（枝内实例一次都
不判），子树盒与级相交才下探，到叶子才逐实例判。正交投影下「子树盒整块在外」等价于「枝内每个实例
都整块在外」，所以**掩码与逐实例判定逐位相同，画面不变**，只是 CPU 判定次数变少。

收益取决于场景有没有成组：`--run-seconds N` 的探针多打一行
`遮挡体层级剔除：盒判定 X / 未判即裁 Y / 其中整枝 Z / 建树次数 W`——
**只有 `其中整枝` 是真省下的**（叶子被裁在旧路径里本来也只判一次 AABB）。
实测：扁平场景（7 个实例）整枝 0、净收益 0%；4 组 × 4 零件（18 实例）整枝 20 / 72 对 = 28%；
3 组 × 8 零件（30 实例）整枝 36 / 120 对 = 30%。枝叶比例不到 1:1 时这层是纯亏，关掉 `casterCull` 更省。
`建树次数` 稳态应为常数 1，逐帧增长说明场景每帧都在增删节点。

**主通道实例收集改成单遍分桶（扩展二十七）**：与阴影侧不同，主通道原来的写法是「先按（网格, 材质）
收一遍键，再按键各扫一遍场景挑实例」——代价是 `键数 × 节点数` 次过滤，同材质格子越多越亏。
现在改成一趟 `scene.iter()` 把实例收进共享 `staged_all`、按下标分进
`HashMap<(MeshRef, Option<SurfaceKey>, bool), Vec<u32>>`（第三位是段标志），视锥/遮挡剔除、
关节槽位分配、场景包围盒并集都跟进同一遍；不透明段在前、半透明段次之、段内按首次出现登记，
半透明仍按「组间最远深度降序 + 组内 `back_to_front`」排。

这是一次**纯 CPU 侧重构**：没有新开关、没有新探针行，draw call、`groups` 区间、`caster_bounds` /
`caster_nodes` 与旧实现逐个相同，缺省场景像素逐位不变，所以像素基线不用重建。边界也没变——
**仍是每帧从零收集**，缓冲区仍每帧重建，单线程；热的还是实例写回与上传，不是收集本身。

**踩坑（已修）**：首版把键判重写成 `if !buckets.contains_key(&key)`。桶键不含段标志，同一网格的
玻璃节点在「不透明节点已登记同键」之后再登记不进 `transparent_keys`，半透明段的桶在摊平时根本轮不到、
**整桶静默丢弃**——玻璃面板消失、实例数 10→7，640×360 上仅 558 个坏点（0.24%，阈值 0.1%），
肉眼几乎看不出，只有 `R3E_VISUAL_STRICT=1` 的 `visual-gpu` 严格模式会红。修法：键仍按段列表判重、
桶键补段标志。修后本机 RTX 5060 严格模式与既有基线逐位一致。
契约由 `crates/r3e-render/tests/instance_buckets_contract.rs`（7 条静态用例）守，其中两条专门钉住
「键按所属段登记」与「桶键带段标志」，一条钉住「节点只进自己那一段」。

**段归属下沉到节点（扩展二十八）**：上面那个修复只解决了「桶登记不上」，实例其实**两段都进**——
`if !occluded { …false… }` 之后紧跟着无条件的 `…true…`。批次键是 `(网格, 材质)`、段内收「所有匹配该键
的节点」，于是同一份几何被两段各收一次。夹具里没露馅：玻璃与实心 `cube` 共用 `(cube, None)` 键，不透明段
（`blend = ALPHA_BLENDING` + `depth_write = true`）把玻璃按实心混合了一遍、半透明段再画时 `depth_compare =
Less` 判等失败被丢弃——两次只有一次生效，像素逐位正确，纯属绘制顺序碰巧。两块玻璃一旦重叠，生效顺序就从
「场景序」变成需要「`back_to_front`」，画面立刻错。

现在入桶按节点自身透明性分支：透明 → 只进半透明桶（且不吃遮挡剔除），不透明且未被遮挡 → 只进不透明桶。
一份几何只画一次。**像素仍然逐位不变、基线不用重建**（玻璃改由半透明段按 `back_to_front` 画，单块不重叠
玻璃结果相同）；探针上能看出来：`visual_regression` 夹具主通道实例 **10 → 7**、半透明段透明实例 **3 → 2**、
draw call 5 不变。

**主通道视锥剔除走场景树层级（扩展二十九）**：`prepare_instances` 原来对每个实例各做一次
`frustum.intersects_aabb`。现在复用扩展二十五在阴影通道验证过的 `CasterTree`：父链子树盒整块落在
视锥外就**整枝跳过**，枝内实例一次都不判。与阴影侧的差别只在判定函数（6 个视锥平面 vs 级联正交投影
区间），下探逻辑是同一个 `descend`。

为此收集拆成两遍：第一遍只算盒、并把盒写回层级树节点（这一遍不看视锥），`refresh` + `plan_frustum`
拿掩码，第二遍才按掩码筛、再走遮挡剔除与入桶。中间态 `prepared` 保持场景迭代序，所以桶序、`groups`
区间、`caster_bounds` / `caster_nodes` 与旧实现逐个相同，**像素逐位不变、基线不用重建**。

探针多一行 `视锥层级剔除：盒判定 X / 实例判定 Y / 未判即裁 Z / 其中整枝 W / 建树次数 V`：
`visual_regression` 夹具（7 实例、全扁平）是 **7 / 7 / 0 / 0**——净收益 0，开不开一个样；
自造两级场景（1 地面 + 6 父 × 4 子、父子树整体在相机背后）31 实例是 **9 / 3 / 30 / 30**，
旧路径 31 次实例 AABB 降到 12 次，30 个实例一次都没判。`建树次数` 稳态应为 1。
**踩坑**：探针首版把父节点放在 z = -40 当成「视锥外」，相机朝 -z 看时那是正前方锥内，读数像零收益；
要放到相机背后（z = +60）才看得见整枝。

**主通道遮挡剔除走场景树层级（扩展三十）**：视锥那关层级化之后，遮挡仍然逐个实例查金字塔，成了收集
循环里最后的热点。遮挡的层级化比视锥简单——它满足**单调性**：子树盒被判为被挡，子实例的盒必然也被挡
（子盒在父盒内），所以父链整枝判掉即整枝不判；视锥反而没有这条（父盒出视锥不代表子盒全出，得看盒本身）。
同样复用 `CasterTree` 与那个通用 `descend`，只是判定换成「盒的屏幕矩形查上一帧金字塔」。

因此掩码生成多一个来源：`plan_occlusion` 与 `plan_frustum` 各出一份掩码，第二遍按两者相与筛；金字塔
**尚未就绪**（前 1~4 帧、或刚关掉遮挡）时整枝回退逐实例判，与旧路径等价。探针多一行
`遮挡层级剔除：层级 X / 盒判定 Y / 判定 Z / 未判即裁 W / 其中整枝 V`：GPU 实跑用「1 面贴脸墙 + 3 父 × 3 子
= 13 实例」的自造场景取到 **4 / 1 / 12 / 12**（`draws` 3 → 1、`instances` 13 → 1），同场景关掉遮挡则是
`层级 false`、`draws` 3 / `instances` 13，两者第 40 帧截图 **sha256 完全相同**——整枝剪掉的 12 个实例在
关闭遮挡时也被那面墙挡住，一个可见像素都没吃掉。

代价：金字塔滞后一帧，整枝误判的代价从「一个实例闪一下」放大成「一整枝一起闪」，物体瞬移那一帧尤其明显；
要不要加迟滞 / 更保守的盒膨胀属于观感取舍，自动化只能证明「开关关闭路径与逐实例逐位相同」。

边界：视锥这一关自扩展二十九层级化，遮挡这一关自扩展三十也层级化（同一棵 `CasterTree`，子树盒判为
被挡即整枝跳过、只对「盒未判」的子树下探到实例），两层都在 `prepare_instances` 的两遍结构里完成；
树按 `layout_version` 重建，拓扑不变时每帧只 refresh；层级平坦或实例很少时这套是**净亏**（多存一份
`prepared`），收益只在成组场景里。

**实例上传走脏段（扩展三十一）**：收集与剔除都层级化之后，一帧里最贵的一步变成「把整张实例缓冲重传一遍」
——静态场景里这一百多 KiB 与上一帧逐字节相同。做法是把比较抽成纯函数 `plan_upload`（`r3e-render/src/
instance_upload.rs`），渲染器只多存一份 `instance_shadow`：上一帧**真正写进 GPU** 的那批实例记录
（扩展三十一时 96 B，扩展三十六加了形变元数据与权重后是 112 B，扩展三十九又加了第二组权重后是 128 B，扩展四十补掩码后是 132 B，扩展四十一补第三组权重后是 148 B——`plan_upload` 只吃 `record` 参数，一行没改）。每帧
按记录整段 memcmp，得到若干连续脏区，然后逐段 `write_buffer`；影子在写完 GPU **之后**才更新，中途 panic
宁可下一帧多传一遍，也不会让影子误以为传过了。

三个参数都是同一笔账的不同算法：空洞 ≤ `MERGE_GAP = 8` 条就并进同一段（用几百字节换一次调用），段数 >
`MAX_RANGES = 24` 就干脆整张全量（碎成上百段比全传更贵），长度不等（实例数变了 / 扩容换代）没有逐条可比
基线，也只能全量。位不同就算脏——NaN、`-0.0`、不同位模式的 `0.0` 都算「变了」，宁多传不漏传。

探针多一行 `实例上传：脏段 X / 写 Y 条 / 跳过 Z 条 / A KiB（全量 B KiB，省 C%）/ 全量 D / 换代 E`：1,537
实例的静态场景取到 `0 / 0 / 1,539 / 0.0 KiB（全量 144.3 KiB，省 100%）`，只旋转两个物件是 `1 / 3 / 3 /
省 50%`，脚本每帧增删临时节点则当帧 `全量 true / 写 7 / 跳过 0`。记录内容一个字节没改，所以像素与全量上传
**逐位一致**，基准图不需要重建。

代价：影子是 148 B/实例的额外 CPU 内存，每帧多一次 O(实例数) 的 memcmp；**半透明批次按深度重排**时相机一动
顺序整体变化，脏区连成一大片、退化成接近全量；也仍是每帧 `write_buffer`，没有持久映射缓冲与多帧飞行。

**关节矩阵也走同一套（扩展三十二）**：同一个 `plan_upload`，只把记录从 `InstanceRaw`（当时 96 B，扩展三十九后 128 B，扩展四十后 132 B，扩展四十一后 148 B）换成 `mat4`
（64 B）。`joint_shadow` 是定长 `MAX_JOINT_MATRICES × 64 B`，没有扩容换代，无蒙皮帧清空（下一帧有蒙皮自然
全量）。探针再多一行 `关节上传：脏段 X / 写 Y 条 / 跳过 Z 条 / A KiB（全量 B KiB，省 C%）/ 全量 D`：37 关节
角色「完全静止」取到 `0 / 0 / 37 / 0.0 KiB（全量 2.3 KiB，省 100%）`；正播动画是 `1 / 37 / 0`（字节省不下，
但 `write_buffer` 从 37 次合并成 1 次）。

扩展三十二落地时实测出一个边界：关节矩阵原写法是 `X⁻¹ · B_now · L_rest⁻¹`，**实例自身的整体变换在数学上
是共轭、结果应等于单位阵**，但浮点运算下只要 `X` 变了算出来的矩阵与上一帧就不是逐字节相同（`-0.0` 与末位
抖动）。于是「骨骼不动、整只模型在转」当时仍是 `1 / 37 / 0` 全量上传，收益只剩「实例变换与骨骼姿势都静止」。

**关节侧消伪脏（扩展三十三）**把这半块补上了：不再走 `X⁻¹ · B_now · L_rest⁻¹`，改以**骨骼树顶点**为基准、
沿局部链累乘求 `anchor⁻¹ · joint`，再乘绑定时算定的一次性常量 `frame_local⁻¹` 回到网格局部空间——整条链里
没有 `X`，整体刚体运动下关节矩阵**逐位不变**。真实导入路径里网格节点是独立根（`tirger` 的网格
`diaojingbaihu` 与骨架 `tirger/hips` 分属两棵树、没有 NodeId 意义上的公共祖先），所以基准取骨骼树顶点而不是
「网格与关节的最近公共祖先」；断链（多骨架根、关节被销毁）仍回退世界矩阵相减，正确性优先、脏段保守判全。
实测「冻结动画 + 整只模型在转」从 `1 / 37 / 0`（省 0%）变成 `0 / 0 / 37 / 0.0 KiB（全量 2.3 KiB，省 100%）`，
「正播动画」仍是 `1 / 37 / 0`（该帧确实变了，合并成 1 段）。细节见 §5.25 / §8.28。

**脚本能点选物件了（扩展三十四）**：新增 `pick_at(x, y)` / `pick_under_mouse()` / `pick_all_at(x, y)`——
像素坐标进，命中物件出（`hit` / `node` / `name` / `distance` / `point`，未命中给完整空模板而不是空 Map，
免得脚本读缺键把 `on_update` 打断）。它和物理的 `raycast` 是两件事：`raycast` 答「物理世界打中了谁」，
`pick_*` 答「画面上光标压着谁」，不要求刚体、也不走物理后端。盒来自渲染层的 `MeshCache`（与视锥剔除共用同一份，
每帧幂等登记、新网格才算一次），内置图元由 `ProceduralProvider` 现算不占表位。
边界说清楚：**AABB 而非三角形精确相交**（凹形资产「点在洞里也算命中」）、隐藏节点不参与、
取不到盒的网格直接不命中（不伪造单位盒，假盒比点不到更难查）、不知道 UI 层级（要「UI 上不算点穿」脚本先 `ui_hit` 判空）、
线性遍历不建加速结构。实测 `assets/scene.json` 画布 1920×1080，屏幕中心自检命中 `spinner`（距离 11.009），
悬停依次报 `ground` / `spinner` / `pick_a` 无脚本报错。细节见 §5.26 / §8.29。

---

**一帧只写一次（扩展三十五）**：脏段上传省的是带宽，`write_buffer` 的**调用次数**没降——40 个实例分成
4 段脏区就是每帧 4 次写。这一层补在它后面：脏段不再直传，先按 `stride` 拷进一块 64 KiB 起的**暂存环**
（`STAGING_SLOTS = 3` 槽，飞行三帧），一帧只 `write_buffer` 一次，随后逐段 `copy_buffer_to_buffer` 落到
真正的实例 / 关节缓冲。实例与关节两路共用同一个槽，`R3E_UPLOAD_STAGING=0` 可关回逐段直传。
实测 40 实例 / 每帧 4 段：695 帧 `写 695 / 拷贝 2,777 / 省写 2,082`，即每帧 4 次写 → 1 次写；
开关两模式的第 40 帧截图 sha256 完全相同，像素零差异。反例也留档：37 关节动画场景 `plan_upload`
早就把 37 个矩阵并成 1 段，暂存层在这里 `省写 1`，**收益为 0**。
边界：wgpu 的拷贝源必须 unmapped，所以**不是**长期 `map` 而是「一帧一次写 + 环形槽飞行」；
多一次 CPU memcpy；超过 8 MiB 直接退化逐段；不省带宽、不省 `plan_upload` 的 memcmp、不减少 draw call。
细节见 §5.27 / §8.30。

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

细节与 glb 已知限制（Draco / meshopt / KTX2 不支持；morph 自扩展三十六、切线通道自扩展三十七已支持）见
[`docs/ASSETS.md`](docs/ASSETS.md) 第 18 章。

---

## 11. 工程化：格式化、静态检查与 CI

工具链由 `rust-toolchain.toml` 固定为 `stable-x86_64-pc-windows-gnu`（含 `rustfmt` + `clippy` 组件）。
之所以写全 host triple：本工程用 MinGW 链接（`.cargo/config.toml` 指定 `build.target`），
若 rustup 默认 host 是 msvc，只写 `stable` 会切到 msvc 工具链，导致 build script / proc-macro
这些 host 产物去找 `link.exe` 而整片编译失败。

四道门每次改动后都要过，本地跑法：

```powershell
cargo fmt --all --check                        # 格式：只检查，不改文件
cargo fmt --all                                # 需要修格式时
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features rapier -- -D warnings
cargo test --workspace                         # 默认特性
cargo test --workspace --features rapier       # Rapier 物理后端

$env:R3E_VISUAL_REGRESSION = "1"               # 像素级回归（需要 GPU，见 §24）
cargo test -p r3e-app --test visual_regression -- --nocapture
```

`cargo clippy` 不加 `-D warnings` 等于没跑——本仓库的口径是**零 warning**，
例外只允许带注释说明的 `#[allow]`，当前全库共三处：`r3e-render/src/renderer.rs` 的
`arc_with_non_send_sync`（`MeshProvider` 刻意不 `Send`，渲染主线程独占），以及
`r3e-asset/src/lib.rs`、`r3e-asset/src/gltf_import.rs` 各一处 `too_many_arguments`（顶点装配 / 导入入口参数天然是平行数组）。

CI 配置在 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)，四道门对应
`rustfmt` / `clippy -D warnings` / `test（默认 + rapier 两轮）` / `visual`（像素回归，拆两个 job），
都跑在 `windows-latest` 上
（Linux runner 无法完成 MinGW 交叉链接，硬跑只会得到误导性的红），
并用 PATH 里的 MinGW 覆盖 `.cargo/config.toml` 中的开发者本机绝对路径。

像素那道门分成两半，因为托管 runner 没有显卡：

- `visual`（`windows-latest`，**必跑**）：`R3E_VISUAL_REGRESSION=1` 打开闸门，同时挡住两类会让这道门形同虚设的用法：
  **禁止在 CI 重建基准图**（环境里出现 `R3E_UPDATE_BASELINE` 就 `exit 1`——那等于拿新画面覆盖旧基准，回归永远是绿的）；
  **跳过 ≠ 没人管**（托管 runner 没显卡，逐像素那条按退出码 2 跳过计 passed，所以同文件里还有一条纯 CPU 用例
  `baseline_png_is_present_with_capture_size`，校验基准图存在、可解码、尺寸恰为 640×360，退化时任何 runner 都红）。
- `visual-gpu`（**带 GPU 的自建 runner**，`runs-on: [self-hosted, windows, gpu]`，变量 `R3E_GPU_RUNNER=true` 时启用）：
  额外设 `R3E_VISUAL_STRICT=1`，把"拍不到图就跳过"升级成"拍不到图就红"。**这道才是真正拦像素漂移的那一道**，
  没有它，CI 里的像素回归等于空跑。注册 runner（含"不要装成 Windows 服务"这个坑）、机器前置、变量开关、
  以及"故意改红一次验它真的在拦"的流程见 [`docs/CI_GPU_RUNNER.md`](docs/CI_GPU_RUNNER.md)。

失败时 `crates/r3e-app/target/visual-regression/` 下的实际图与差异图会作为 artifact 上传
（`visual-regression-artifacts` / `visual-gpu-artifacts`）。

**中间产物往哪放**：调试日志、临时截图、一次性探针工程一律落 `temp/` 或 `target/`（都已被 `.gitignore` 挡住），
仓库根目录只留版本库跟踪的文件。历史遗留的根目录游离日志（66 个 `.log` / `.err` / `.out`）与
`_probe/` 探针工程已归档到 `temp/root_logs_20261109/`、`temp/_probe/`——现在
`git status --ignored` 在根目录只剩 `target/` 与 `temp/` 两项，不再是一屏忽略项。

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

## 14. 音频播放（非空间化 → 3D 空间化）

**目标**：能放背景音乐、能敲音效、能同时响多条，并且**没声卡也绝不崩**。
路线表第十项在其上加了一层 **3D 空间化**（声源挂场景节点、距离衰减 + 多普勒、听者随活动相机），
见本节末尾「3D 空间化」；不调用它时，行为与 M1 逐位一致。

| 层 | 模块 | 职责 |
| --- | --- | --- |
| `r3e-audio` | `lib.rs` | `Audio` 门面：key 账本、单条音量 × 总音量、开关、错误记账、`update()` 回收；空间化覆盖层（注册表 / `tick_spatial` / 增益与倍速合成 / 32 上限） |
| `r3e-audio` | `backend.rs` | `AudioBackend` 契约（含 `set_speed` / `supports_speed`）+ `RodioBackend`（cpal 设备 + symphonia 解码）+ `SilentBackend` |
| `r3e-audio` | `spatial.rs` | 空间化纯函数层：`DistanceModel` / `attenuation` / `radial_velocity` / `doppler_ratio` / `ListenerPose` / `SpatialSource` |
| `r3e-script` | `api.rs` | `audio_*` 与 24 个 `audio_spatial_*` 绑定，全部返回 `bool` / 数值 / 字符串，不 panic |
| `r3e-app` | `app.rs` | `EngineApp::audio` 持唯一设备句柄，每帧 `update()` 回收；`drive_spatial_audio` 灌听者与声源坐标后结算 |

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

- `cargo test -p r3e-audio`（29 个 = 门面 18 + 空间化 11）：缺失文件 / 非音频内容 → `Err` 而非 panic、
  音量边界（0 / 1 / 超界 / NaN）、静默后端不登记实例、通道关闭拒绝播放、空 key 拒绝、
  同名 key 重播停止旧实例不叠加、`set_loop` 切换重播且幂等、播完回收；空间化侧为纯函数测试
  （模型名往返、`sanitized` 夹取、三种模型在 `min` / 中点 / `max` 的取值与单调性、平滑窗让逆距离在 `max` 归零、
  `NaN` / `inf` 距离保守静音、径向速度方向与重合退化、多普勒静止为 1 与超音速夹取、`dt <= 0` 不动速度仍算增益）。
- `cargo test -p r3e-script`（`audio_script_tests`，11 个）：静默降级路径（`audio_available() == false` 时
  播放返回 `true` 但不占实例）、加载失败返回 `false` 并记账、音量上下界夹取、重复 key 不叠加且不算错误、
  通道开关拦截播放；空间化侧 6 个：注册 / 满 32 拒绝不顶人、绑定节点与 `spatial_bindings`、
  `detach` 回平面音量、总开关归 1、`tick_spatial` 把倍速下发到后端、探针未注册时的默认值。
- 编辑器 `audio.spatial.*` 6 条 RPC 另有 9 个测试（方法名大小写归一化、补丁合并、空 key 拒绝、只读 listener）。
- 真实出声、mp3 / ogg 的实际听感、设备热插拔、多普勒听感是否自然：**待人工验证**（无法自动化）。

### 3D 空间化（路线表第十项）

**一句话**：声源能挂到场景节点上，按到听者的距离衰减、按相对速度做多普勒，听者每帧由**活动相机**推导
（`forward` 只记录，不参与增益——**没有左右声像**，这是明确取舍）。

- **纯叠加**：未注册的 key 逐位不变；`audio_spatial_set_enabled(false)` 立刻回退成平面声；`detach` 只摘覆盖层、不停声音。
- **三种距离模型**：`inverse` = `min/d`、`exponential` = `(min/d)²`、`linear` = `1 - t`；
  前两者额外乘 `1 - smoothstep(t)` 平滑窗，保证 `d >= max` 时**真的归零**，不在边界上"啪"一声。
- **多普勒**落到 `set_speed`（改采样推进速率，音高与时长一起变，正是物理效果）：`c / (c - v_toward)`，
  夹在 `1/4 ..= 4`；速度由位置差分 + 0.4 一阶平滑得来（脚本瞬移不会咔一下）；暂停 `dt = 0` 时速度不更新、倍速回 1。
- **上限 32 条**，满员拒绝新注册并记错误，不挤掉已有声源。
- **静默降级**：`audio_supports_speed() == false` 时多普勒只算不下发（探针里 `rate` 仍是真实值，可在无卡环境验证公式）。

**人工验证**：`cargo run -- assets/scene_spatial_audio.json scripts/spatial_audio_demo.rhai --run-seconds 8`。
脚本每 0.5 秒打印两个声源的 `gain` / `rate` / 距离；飞近 `bell` 应听到它变响，`horn` 冲近时 `rate > 1`、掉头远离时 `rate < 1`。

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
- 视觉观感（金属是否像金属、高光带是否自然）**待人工确认**：像素基线（[§24](#24-captureframe-像素级回归扩展二十)）只锁 `tests/fixtures/visual_regression` 那一个静止画面，覆盖面是「改了会不会变」而不是「好不好看」。

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
- 视觉观感（金属暗部是否"活"了）**待人工确认**：像素基线（[§24](#24-captureframe-像素级回归扩展二十)）锁的是默认夹具画面，金属观感仍需人眼判断。

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
- 视觉观感（泛光是否"糊"、ACES 是否偏灰）**待人工确认**：像素基线（[§24](#24-captureframe-像素级回归扩展二十)）能证明画面没漂移，不能证明观感达标。

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

> 本节写的是**不投影**的局部光源（扩展十五的初版口径）。扩展十八 C 给其中最多 4 盏加了可选的
> 「投影」能力（`cast_shadow`，点光立方 6 面 / 聚光单面），灯表本身、衰减与锥因子公式一个字没变，
> 阴影细节见 [§23](#23-局部光源阴影扩展十八-c)。

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
| 混合空间 | 扩展四十五另加 6 个（`set_anim_layer_blend_1d` / `_2d` / `clear_anim_layer_blend` / `add_anim_layer_blend_sample` / `set_anim_layer_blend_value` / `anim_layer_blend`），见 [§29](#29-1d--2d-混合空间扩展四十五) |
| 相位锁 | 扩展四十六另加 5 个（`set_anim_layer_sync` / `set_anim_layer_sync_mode` / `anim_layer_sync` / `anim_layer_phase` / `anim_layer_sync_probe`），见 [§30](#30-层内相位同步扩展四十六) |
| 相位同步组 | 扩展四十七另加 5 个（`set_anim_layer_clip_phase_offset` / `anim_layer_clip_phase_offset` / `set_anim_layer_sync_source` / `anim_layer_sync_source` / `anim_layer_sync_phase`），见 [§31](#31-相位同步组偏移量与跨层对齐扩展四十七) |
| 片段播放参数 | 扩展四十八另加 5 个（`set_anim_layer_clip_playback` / `clear_anim_layer_clip_playback` / `anim_layer_clip_speed` / `anim_layer_clip_looping` / `anim_layer_clip_time`），见 [§32](#32-片段自带播放速率与循环标志扩展四十八) |
| 三维混合空间 | 扩展四十九另加 3 个（`set_anim_layer_blend_3d` / `add_anim_layer_blend_sample_3d` / `set_anim_layer_blend_value_3d`），摘空间与回显复用上面的 `clear_anim_layer_blend` / `anim_layer_blend`，见 [§33](#33-三维混合空间扩展四十九) |
| 剖分缓存 | 扩展五十一另加 2 个（`anim_layer_blend_stale(inst, layer)` / `build_anim_layer_blend(inst, layer)`），见 [§35](#35-混合空间剖分缓存扩展五十一) |
| 加法层组合语义 | 扩展五十**不新增 API**：`add_anim_layer_additive` 建的多路层，空间分来的份额改为「各路增量的几成」（单路口快路径不变，覆盖层不受影响），见 [§34](#34-加法层与混合空间的组合语义扩展五十) |

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
**additive 参考帧固定为片段首帧**（不能指定任意参考姿势）、**没有形变与动画的联动**（morph 权重在扩展三十六里
是独立的一组标量，不随动画片段曲线走，表情切换要脚本自己插值）、
**没有 IK 与脚部对齐**（斜坡上脚会浮）、遮罩是二值而非软权重。
这几条与 `docs/ARCHITECTURE.md` §11 的动画边界保持同一口径。
层内「参数 → 权重」这步已由 [§29 1D / 2D 混合空间](#29-1d--2d-混合空间扩展四十五) 收进引擎（扩展四十五）。

### 21.7 测试覆盖

新增 44 条用例，全 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 11 | override 端点、additive 增量、additive 旋转绕行、层内加权采样、缺轨道跳过、淡入曲线、时间推进与顺序、片段管理边界、模式解析 |
| `crates/r3e-core/src/scene.rs` | 7 | override 只动遮罩内骨骼、additive 叠加、淡入缩放贡献、层管理往返、层数上限、主片段切换后层存活、清空后恢复主姿态 |
| `crates/r3e-script/tests/anim_layers_api.rs` | 9 | 真跑 Rhai：遮罩生效、遮罩串解析回显、模式串校验、权重过渡推进、上限与实例缺失的中性返回 |
| `crates/r3e-app`（宿主补丁） | 6 | 补丁生效、未知模式无副作用、缺实例 / 缺层拒绝、列表截断但计数为总数 |
| `crates/r3e-editor/tests/anim_layer_rpc.rs` | 11 | `scene.animlayers` / `scene.animlayerset` 的往返、坏值与越界 |

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed。

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

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed。

---

## 23. 局部光源阴影（扩展十八 C）

点光与聚光现在**可以投影**了，但预算是硬的：**每帧最多 4 盏**真正投影，其余灯照旧只照亮不投影。
一张图搞定全部——固定 `512² × 24 层` 的深度数组（4 盏 × 6 面），点光占 6 个面、聚光占 1 个面，
两者共用同一套槽位。缺省 `cast_shadow: false`，**不写这两个字段的场景与扩展十五逐像素一致**。

### 23.1 三条写入路径

| 位置 | 写法 |
| --- | --- |
| 场景 JSON | `lights: [{ key: "lamp", kind: "point", cast_shadow: true, shadow_bias: 0.004, ... }]`（`shadow_bias` 缺省 `0.002`，夹在 `0 ~ 0.05`） |
| 脚本 | `light_set_cast_shadow("lamp", true)` / `light_set_shadow_bias("lamp", 0.004)`，键不存在返回 `false` |
| 编辑器 RPC | `light.set` 的补丁里带 `cast_shadow` / `shadow_bias`，与 `intensity` / `range` 等字段同一份补丁 |

三处写的是**同一个 `Scene::lights`**，没有独立的阴影配置段、没有独立的 RPC、没有第二份灯表。
`cast_shadow` 是「意愿」，渲染器每帧重新决定谁真的投影。

### 23.2 谁在投影：亮度排序 + 4 盏上限

`r3e_core::plan_local_shadows` 每帧做一次纯函数决策：

1. 只留下 `cast_shadow == true` 的灯；
2. 按 `luma(radiance) × range²` 从大到小排（辐射亮度 = 颜色 × 强度，乘 `range²` 是因为灯越大照得越远）；
3. 截断前 4 盏，再按下标升序回到稳定顺序；
4. 第 `slot` 盏灯的层基址 `layer_base = slot × 6`——**聚光也占 6 层**（只用第 0 面，其余 5 层空着），
   这样层号永远是 `slot × 6 + face` 的简单算式，着色器不必查表。

**代价可预测**：最坏情况 4 盏点光 = 24 个深度 pass，每 pass 一次全场景重绘；1 点光 + 1 聚光 = 7 个。
探针里的「占用层数」就是这一帧实际跑的 pass 数。

### 23.3 GPU 侧布局（`group(3)`，与灯表同组）

| binding | 资源 | 说明 |
| --- | --- | --- |
| 5 | `texture_depth_2d_array` | 24 层、`512²`、`Depth32Float`，缺省全 1.0 |
| 6 | `sampler_comparison` | 3×3 PCF 用，与方向光阴影共用同一套采样器配置 |
| 7 | `LocalShadow` uniform | `params` / `params2` / `view_proj[24]` / `slots[16]` |

- `slots[i].x = layer_base`，**默认 -1**：片元读到 -1 立刻返回全亮，不必等数组清零；
- `slots[i].y` = 面数（6 = 点光，1 = 聚光），`z` = 该灯深度偏移，`w` = 该灯法线偏移（世界单位）；
- `view_proj[layer]` 在 CPU 侧一次性算好（`shadow_face_view_projection`），**深度 pass 与主通道用同一个矩阵**，
  不存在「写读不一致」的半像素偏移；
- 数组长度 `24` / `16` 是编译期字面量，`shader.wgsl` 与 CPU 常量硬对齐，改一处必须改三处
  （core 常量 / render uniform / wgsl），契约测试会盯着。

### 23.4 采样逻辑

片元侧对每盏灯调一次 `local_shadow_factor`：

1. 全局开关关、或这盏灯 `layer_base < 0` → 直接 `1.0`（全亮），不采样；
2. **点光选面**：拿 `-to_light`（光源相对片元的方向）的主轴绝对值最大的分量定面，与 CPU 的
   `point_shadow_face` 逐分支对应（`+X, -X, +Y, -Y, +Z, -Z`）；聚光固定第 0 面；
3. 法线偏移 `pos + n × slots[i].w`（`w = 1.5 × 单纹素世界尺寸`），再投影取 uv；
4. uv 出界或 `ndc.z > 1`（超出灯的 `range`）→ 全亮；否则 3×3 `textureSampleCompare` 求平均；
5. 最后 `mix(1.0, lit, 强度)`——阴影只压**直接光项**，环境光不参与，否则暗部会死黑。

### 23.5 探针与演示

```
[起始] 局部阴影：意愿 2 盏 / 本帧投影 2 盏（上限 4）/ 占用 7 层 / 图 512² × 24 层
```

| 环节 | 说明 |
| --- | --- |
| **意愿** vs **本帧投影** | 前者是场景里 `cast_shadow` 为真的盏数，后者是 `plan_local_shadows` 选中的盏数。`意愿 5 / 投影 4` = 被上限截断；`意愿 2 / 投影 0` = 总开关关了 |
| 占用层数 | 应恒等于各投影灯面数之和（1 点光 = 6、1 聚光 = 1、各一 = 7），对不上就是槽位算错 |
| 图幅 | 固定 `512²`，**不跟**方向光的 `shadow.size` 走；24 层容量与投影盏数无关 |
| 脚本读侧 | `local_light_cast_shadow(key)` 读意愿、`local_shadow_caster_count()` 读本帧实际投影数 |
| 演示 | `assets/scene_local_shadow.json`（方向光压暗并关掉自己的阴影）+ `scripts/local_shadow_demo.rhai`（点光绕圈 + 聚光摆头） |

```
cargo run -p r3e-app -- --scene assets/scene_local_shadow.json --script scripts/local_shadow_demo.rhai
```

### 23.6 已知边界

- **每帧最多 4 盏**投影，超出的灯退化为「只照亮不投影」，没有告警日志——看探针的意愿 / 投影差值；
- 聚光**占满 6 层槽位只用 1 层**：24 层里最多 12 层被浪费，换成更紧的打包能省 4~5 个面，暂未做；
- 图幅固定 `512²`、不可配；立方体面的透视相机 FOV 恒 90°，面接缝处**未做跨面 PCF**，掠射角度下有轻微接缝；
- 阴影强度是**全局标量**（`params2.x`），不能逐灯调；`shadow_bias` 逐灯可调但只有深度偏移一项，没有 slope-scaled 版本；
- 与方向光阴影是**两张独立的图、两段独立的 pass**，同一个物体会被两边各渲染一次；
- 移动端需要注意：4 盏点光 = 24 次全场景深度重绘，是本项最贵的路径。

### 23.7 测试覆盖

新增 31 条用例，全 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/light.rs` | 8 | 选灯按 `luma × range²` 排序与 4 盏截断、并列按下标稳定、槽位 `layer_base = slot × 6`、点光 6 面 / 聚光 1 面、选面六分支与边界、面相机近远平面（`near = 0.02 × range`）、`cast_shadow` / `shadow_bias` 收敛 |
| `crates/r3e-core/tests/local_lights_scene.rs` | 1 | 场景 JSON 的 `cast_shadow` / `shadow_bias` 往返 |
| `crates/r3e-render/tests/local_shadow_contract.rs` | 8 | uniform 字节布局与 `LOCAL_SHADOW_UNIFORM_SIZE`、`slots` 默认 -1、`from_lights` 与 `plan_local_shadows` 槽位一致、关闭时零投影、聚光只写 1 层 |
| `crates/r3e-render/tests/local_shadow_shader.rs` | 6 | 解析 `shader.wgsl` 比对 `array<mat4x4<f32>, 24>` / `array<vec4<f32>, 16>` 字面量、`@group(3)` 的 5/6/7 三处绑定、`point_shadow_face` 六分支与 CPU 同序 |
| `crates/r3e-script/tests/local_shadow_api.rs` | 6 | 写入读回、意愿与实际分离、键不存在返回 `false`、槽满时第 5 盏不投影、`shadow_bias` 夹取 |
| `crates/r3e-editor/src/lib.rs` | 1 | `light.set` 补丁透传 `cast_shadow` / `shadow_bias` |
| `crates/r3e-app/src/editor.rs` | 1 | 探针字段 `local_shadow_casters` / `local_shadow_layers` 可读 |

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed。

---

## 24. capture.frame 像素级回归（扩展二十）

`capture.frame`（P0-4 批次落库）解决了「能不能拍」，本节解决「拍出来的东西对不对」——
顺带修掉一个只有在**整机出图**时才暴露的渲染 bug。

### 24.1 先说那个 bug：AO 占位图没写初值，画面全黑

| 项 | 内容 |
| --- | --- |
| **位置** | `crates/r3e-render/src/postfx.rs` 的 `create_ao_placeholder`（`PostFxPass` 构造时调用），共 1 处；`renderer.rs` 只是把 `&queue` 透传进来 |
| **成因** | SSAO 关闭时 composite 仍要有一张遮蔽图可采样，代码建了 1×1 `R8Unorm` 纹理当占位图，**但只建不写**。WebGPU 规定纹理创建后内容未定义（实现上普遍清 0），而 composite 片元里是 `color *= ao` ——采到 0 就把整帧乘成黑色 |
| **表现** | 窗口全黑，**连背景清除色都一起没了**；`RenderStats`（draw / instance / cull）完全正常，日志里 `渲染器就绪`、探针一个不缺，纯肉眼看不出问题出在后处理 |
| **影响范围** | 默认配置（`postfx.ssao.enabled = false`）下**所有场景**；只有显式开 SSAO 时才有真实遮蔽图、才不黑。因此扩展十七之后的默认构建一直是坏的，肉眼验证存在窗口才会发现，无头测试全绿也发现不了 |
| **修法** | 建完纹理立刻 `queue.write_texture(...)` 写一笔 `[255u8]`（`R8Unorm` 的 255 = 1.0，乘上去等于没乘）；`PostFxPass::new` 因此多收一个 `&wgpu::Queue` 参数 |
| **为什么现在才炸出来** | 这条链路的产物此前只在人工看窗口时才被评价，而「全黑」与「场景本来就暗」在没有基准图时无法区分——这正是下面这套回归基线存在的理由 |

### 24.2 分工：GPU 边界划在「产图」这一步

| 层 | 位置 | 职责 | 要 GPU |
| --- | --- | --- | --- |
| 比对逻辑 | `crates/r3e-asset/src/visual_regression.rs` | `RgbaFrame`（RGBA8 + PNG 读写）、`Tolerance::sanitized`、`compare`、`compare_png`、`assert_png_matches`；产出 `DiffReport`（坏点数 / 坏点比例 / 最大与平均差 / 差异包围盒 / 最差 8 点） | 否 |
| 产图 | `crates/r3e-app/tests/visual_regression.rs` | 起 `r3e` 子进程，固定尺寸跑到约定帧截图 | 是 |

比对库**零 wgpu 依赖**，所以容差语义、差异统计算法在任何机器上都能单测；
真正需要显卡的只有「拍一张新图」。这也是把「纯逻辑做单测、GPU 部分可跳过」落到实处的切法。

### 24.3 三个命令行选项

| 选项 | 语义 | 默认 |
| --- | --- | --- |
| `--window-size WxH` | 窗口**物理像素**尺寸（不乘 DPI 缩放；`x` / `X` / `*` 均可作分隔符，宽高须 ≥ 1） | 1280x720 逻辑尺寸 |
| `--capture-frame PATH` | 跑到约定帧后截图、落盘并退出；**复用编辑器 `capture.frame` 同一条链路**（离屏渲染 → `write_png_rgba`） | 不产图 |
| `--capture-after N` | 第 N 帧之后截（夹到 ≥ 1；首帧的后处理目标与字形图集还没热） | 2 |

为什么要**物理**像素：基准图是逐像素比的，逻辑尺寸在 125% / 150% 缩放下会拍出不同分辨率，
同一张基线换台机器必红。

### 24.4 基准图、夹具与产物

| 路径 | 作用 | 是否进版本库 |
| --- | --- | --- |
| `crates/r3e-app/tests/baselines/capture_frame_640x360.png` | 基准图（640x360、第 3 帧、21 KB 量级） | 是 |
| `crates/r3e-app/tests/fixtures/visual_regression/scene.json` + `static.rhai` | 回归专用夹具：7 个静止物件 + 空脚本 | 是 |
| `crates/r3e-app/target/visual-regression/*.png` | 实际截图 / 差异图 / 文本报告 | 否（`target/` 天然忽略） |

刻意**不复用** `assets/scene.json` + `scripts/main.rhai`：演示脚本会转物件、每帧刷帧率文本，
同一帧号下画面随运行时长变化，那种输入会让用例「代码没改也红」。

### 24.5 怎么跑

```powershell
# 手动拍一张（与用例同口径），产物可直接打开看
cargo run -- crates\r3e-app\tests\fixtures\visual_regression\scene.json `
              crates\r3e-app\tests\fixtures\visual_regression\static.rhai `
              --window-size 640x360 --capture-frame target\shot.png --capture-after 3

# 真跑回归（需要 GPU）
$env:R3E_VISUAL_REGRESSION = "1"
cargo test -p r3e-app --test visual_regression -- --nocapture

# 首次建档，或确认画面变化是有意的
$env:R3E_UPDATE_BASELINE = "1"
cargo test -p r3e-app --test visual_regression -- --nocapture
```

三道闸门保证**没显卡的机器不假失败**：① 只有 `R3E_VISUAL_REGRESSION=1` 才真跑，
否则返回并打印跳过原因（CI 上仍计 passed）；② 子进程退出码 `2` = 渲染器 / 截图链路不可用，
按跳过处理；③ 基准图缺失时明确报错并提示用 `R3E_UPDATE_BASELINE=1` 建档，不静默通过。

但「跳过」不能等于「没人管」，所以 CI 的 `visual` job 是这么补的：

- 用 `R3E_VISUAL_REGRESSION=1` 把闸门强行打开，有显卡就真比像素；
- 另加 `baseline_png_is_present_with_capture_size`（**纯 CPU**）守基准图本身：存在、可解码、
  尺寸恰为 640×360、像素数与声明吻合——被误删 / 截断 / 尺寸漂移，没显卡也照红；
- CI 环境里出现 `R3E_UPDATE_BASELINE` 直接 `exit 1`，防止「新画面覆盖旧基准」把回归洗成永远绿；
- 失败时把 `target/visual-regression/` 的实际图与差异图作为 artifact 上传。

### 24.6 判定口径

- 单像素差 = 四通道 `|基准 - 实际|` 的**最大值**（alpha 也算：截图 alpha 恒 255，某条链路忘了写不透明会立刻炸出来）；
- 单像素差 > `Tolerance::channel_delta` 即坏点，坏点掩码用来画差异图；
- 通过 ⇔ `坏点比例 <= Tolerance::bad_pixel_ratio`；
- 默认 `DEFAULT_CHANNEL_DELTA = 2`（8bit 量化的两级抖动）、`DEFAULT_BAD_PIXEL_RATIO = 0.001`（640x360 下约 230 个像素）：
  单点噪声被容忍，整屏偏色 / 整体缺失必然超比例；
- 可用 `R3E_VISUAL_CHANNEL_DELTA` / `R3E_VISUAL_BAD_RATIO` 放宽（换驱动时）；
- 截图成功时日志：`[capture] 第 N 帧截图完成: {"bytes":21049,"width":640,...} | draws=5 instances=10`
  ——绘制统计和路径一起打出来，挂掉时才能先分清「画了但颜色不对」和「一个 draw 都没提交」。

### 24.7 已知边界

- 逐像素比对**跨 GPU / 驱动不保证一致**，换机器先 `R3E_UPDATE_BASELINE=1` 重建再比；
- 只覆盖单一夹具的**单帧静止画面**：动画、输入、交互、RPC 产图都不在基线内；
- 无 GPU 时逐像素那条跳过（计 passed）；CI 里由纯 CPU 用例守基准图存在性与尺寸，但**像素漂移**
  仍需带 GPU 的机器（本机或自建 runner）才能拦住；
- 无「多基准 / 分平台基线」机制，也没有对基准图做压缩以外的优化。

### 24.8 测试覆盖

新增 14 条，其中 12 条纯 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-asset/src/visual_regression.rs` | 11 | 尺寸不一致报错、全等图零坏点、单通道单点差异判据、容差边界（等于 / 大于 delta）、坏点比例阈值、最差像素按差值降序且截断、差异包围盒、`Tolerance::sanitized` 夹取、alpha 差异参与判定、差异图着色、摘要文本 |
| `crates/r3e-app/src/main.rs` | 2 | `parse_window_size` 的三种分隔符与非法输入（缺 `x`、0、非数字） |
| `crates/r3e-app/tests/visual_regression.rs` | 2 | 端到端像素回归（无 GPU 时经三道闸门跳过）+ 纯 CPU 的基准图自洽体检（存在 / 可解码 / 尺寸与像素数；CI 上就靠它不空跑） |

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed（`--features rapier` 为 874 passed）。

---

## 25. 材质数据贴图：法线 / 粗糙度 / AO（扩展二十一）

§15 的 PBR 只有 `metallic` / `roughness` 两个**标量**，§16 的 IBL 也只有天空，
所以导入一个正经游戏资产后，模型表面永远是"光滑塑料"——砖缝、螺丝孔、皮革纹路一个都看不出来。
本节把三张**逐像素**数据图接进材质：法线（凹凸光照）、粗糙度（高光形状）、AO（缝隙变暗）。

### 25.1 一句话结论与三条不变量

三张图都是**可选**的：没挂图时着色器走原来的标量分支，画面与扩展二十**逐像素一致**
（实测 `R3E_VISUAL_REGRESSION=1` 基线原样通过，基准图不需要重建）。三条不变量：

1. **开关只认真的绑上的图**：`resolve_pbr_map` 只有真拿到纹理视图才把 `params2` 对应位写 1，
   否则垫占位图 + 写 0。否则会出现"没贴图却去采一张白图把粗糙度乘成 1.0"这种静默行为变化。
2. **数据贴图按线性上传**：法线被 sRGB 解码会把 0.5 变成 0.21，整个模型光照发灰。
3. **AO 只压环境光**：直接光的暗部已由阴影负责，两边都压等于接触处叠两层黑。

### 25.2 core 侧：切线基（`r3e-core/src/tangent.rs`）

法线贴图存的是**切线空间**方向，必须有 TBN 才能转到世界空间。切线按经典做法算：
逐三角形用位置差与 UV 差解出 `dP/du`、`dP/dv`，按三角面积加权累加到三个顶点，再对每个顶点做 Gram-Schmidt 正交化。

| 退化情形 | 处理 | 理由 |
| --- | --- | --- |
| 三角形 UV 面积为 0（三点 UV 相同） | 跳过该三角形，不参与累加 | 除零会产 NaN，NaN 会顺着插值污染整片画面 |
| 整网格都累加不到（无 UV / 全退化） | 用与法线最不平行的轴重建 | 切线方向不精确好过 `normalize(0)` 产 NaN |
| 正交化后长度 `< 1e-5` | 同上兜底 | 同上 |

手性（`w = ±1`）来自 `cross(normal, tangent)` 与副切线累加方向是否同向，镜像 UV 的模型靠它翻正。
`Vertex` 因此多了 `tangent: [f32; 4]`（location 11），stride 从 56 涨到 72。
**内置的 4 种网格不跑重算**，直接用 `DEFAULT_TANGENT = [1,0,0,1]`，由着色器兜底——省掉每次建网格的扫描。

### 25.3 core 侧：槽位分类（`r3e-core/src/texture_slot.rs`）

不同 DCC 导出的槽位名五花八门（`NormalMap` / `Maya|normalCamera` / `Bump` / `Roughness` / `Maya|aiRoughness`…），
`classify_slot` 把它们收敛成 `TextureSlot::{Normal, Roughness, Occlusion, Unclassified}`，
`is_linear_data()` 给出"是否按数据（线性）上传"的判据。**导入层与渲染层共用同一套判据**，
避免"asset 认为是法线、render 认为是颜色"的错配。

这里有一个坑值得单列：**`bump` 是高度图，不是法线图**。当成法线用会把灰度图当方向，
表现是整片画面偏色且光照方向诡异，且不会报错。因此 `classify_slot("Maya|bump")` **不**返回 `Normal`。

### 25.4 导入侧：两条路径都按槽位挑图

| 路径 | 位置 | 行为 |
| --- | --- | --- |
| FBX（AIR） | `crates/r3e-asset/src/air_import.rs` | 遍历材质 `TextureInfo` 扁平表，按 `classify_slot` 挑出三张；同用途出现多张时**只取第一张**（不猜、不混） |
| glTF | `crates/r3e-asset/src/gltf_import.rs` | 从 `normalTexture` / `metallicRoughnessTexture` / `occlusionTexture` 取，`bump` 忽略 |

槽位判不出来（`Unclassified`）时**不猜**：它仍然按颜色上传、仍然可以当 base color 用，
只是不会进三个数据槽位——宁可少认，不可认错。

外部网格在 `MeshAsset::build` 里调一次 `recompute_tangents`；内置网格与已带切线的资产不重复计算。

### 25.5 渲染侧：绑定、占位与采样

`@group(1)` 的布局：

| binding | 内容 | 缺省时 |
| --- | --- | --- |
| 0 | `MaterialUniform`（64 B，`params2.xyz` = 三个开关） | — |
| 1 | base color `texture_2d` | `white` |
| 2 | `sampler`（三张数据图**共用**） | — |
| 3 | 法线图 | 1×1 `flat_normal` = `(128,128,255)`，**线性格式** |
| 4 | 粗糙度图 | 1×1 `white` |
| 5 | AO 图 | 1×1 `white` |

`flat_normal` 必须是线性格式（`Rgba8Unorm`）：若按 sRGB 上传，0.5 会被解码成 0.21，
乘以 2 减 1 得到 0.58 而不是 1.0，等于给所有没贴法线的物体加了一层斜的假凹凸。

纹理缓存键是 `(asset, index, space)`，`space` 参与其中，所以同一张图被一个材质当颜色、
被另一个材质当数据时会上传成两份而不是串用。

采样约定（与 glTF 一致）：

| 贴图 | 通道 | 解码 |
| --- | --- | --- |
| 法线 | `xyz` | `* 2 - 1` 到 `[-1,1]`，`z` 夹到 `≥ 0.05` 防翻面 |
| 粗糙度 | `G` | 乘到标量 `roughness` 上（不是替换） |
| AO | `R` | 夹到 `[0,1]`，只乘环境光（含 IBL） |

着色器里同时保留 `geometric_n`（几何法线）与 `n`（着色法线）：TBN 的切向基用几何法线构造，
BRDF 与阴影查询用着色法线。这样凹凸表面的阴影偏移跟着贴图走，不会出现锯齿状的自遮挡线。

### 25.6 怎么验证

```powershell
# CPU 侧契约（不需要显卡）
cargo test -p r3e-core -p r3e-asset -p r3e-render

# 画面没漂：没挂图的场景必须与扩展二十基线逐像素一致
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture

# 肉眼确认贴图真的生效：拖入带 NormalMap / Roughness / AO 槽位的模型，逐张开关对比
cargo run -- assets\scene.json
```

**视觉观感待人工确认**：法线强度是否合适、AO 是否过重属于审美判断，自动化只能证明"没图时没变化"。

### 25.7 测试覆盖

新增 25 条，全部 CPU 侧、无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/tangent.rs` | 5 | 平面网格切线正交且手性正确、UV 镜像翻手性、UV 退化三角形被跳过、全退化网格兜底基、切线单位化 |
| `crates/r3e-core/src/texture_slot.rs` | 3 | 常见槽位名归一、`bump` 不算法线、数据 / 颜色判据 |
| `crates/r3e-render/src/pbr.rs` | 4 | `PbrMapFlags` 打包顺序、`build_with_maps` 开关位、标量仍收敛、默认全关 |
| `crates/r3e-render/tests/pbr_maps_contract.rs` | 9 | uniform 64 B 与 `params2` 偏移 48、无图全 0、binding 不重复、着色器 `params2.xyz` 三分支齐全、TBN 构造与 `*2-1`、粗糙度取 G、AO 取 R、`bump` 非 Normal |
| `crates/r3e-asset/tests/pbr_map_import.rs` | 4 | 三槽位识别、同用途只取首张、`bump` 忽略、无图时三者皆空 |

这些写错都**不会编译失败**，只会在实机上表现为"贴图没效果"或"画面诡异"，
所以 `pbr_maps_contract.rs` 用源码级断言（`include_str!` 着色器 + 文本匹配 + naga 解析）把它们钉住。

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed（`--features rapier` 为 874 passed）。

### 25.8 已知边界

- **不读金属度贴图**：glTF 的 metallicRoughness 合并图只用了 G，B（金属度）仍是标量；
- **只有一套 UV**（`uv0`），不支持 `KHR_texture_transform`，不做 UV 集选择；
- **切线装配期一次算定**：蒙皮网格的切线跟法线同用伴随矩阵近似变换，极端蒙皮形变下仍有偏差（形变目标本身给出的切线增量
  自扩展三十七起直接参与计算，不再回退到绑定姿态的切线方向）；
- **数据贴图不生成 mipmap**：远距离采样会闪，未做各向异性过滤；
- **没有脚本 API 与 RPC**：三张图只能由导入的模型自带，运行时换图需要改资产；
- 法线贴图 `z` 下限 `0.05` 会削弱强凹凸的陡峭度，属于"防翻面优先"的取舍。

---

## 26. 遮挡剔除（扩展二十二）

一句话：**把上一帧的深度压成 tile 金字塔，用纯 CPU 判「这个物件的屏幕矩形是不是整块都比墙远」，是就本帧不画。**
缺省关闭，打开后开关关闭路径与扩展二十一逐像素一致。

### 26.1 三条不变量

1. **宁可多画，不可让物件凭空消失**：金字塔缺失 / 尺寸对不上 / 物件跨近平面 / 屏外 / 取不到 AABB，一律判可见；
2. **开关即零成本**：`enabled = false` 时 GPU 命令与实例数与加这个功能之前逐字节一致，视觉基线无需重建；
3. **判定是纯算术**：core 侧 `Vec<f32>` + 比较，零 wgpu，44 条用例全在无显卡机器上跑。

### 26.2 core 侧（`crates/r3e-core/src/occlusion.rs`）

- `DepthPyramid::build(width, height, tile, depths)`：把 NDC 深度按 `tile × tile` 分块，**每块只留最远值**
  （判「这块区域里有没有比物件更近的表面」，最远值就是界限），并逐层翻倍建到 1 个 tile（最多 8 层）；
- `OcclusionSettings { enabled, depth_bias, tile_size, margin_px, max_tiles }`：`sanitized()` 把越界值夹回去；
- `is_occluded(bounds, settings)`：**两阶段**——粗筛（矩形外扩 `margin_px`）与复核（`review()`：margin 归零、偏置 ×4），
  两次都判遮挡才返回 `Occluded`；`Rejected`（粗筛过、复核没过）**照画但计入统计**，调参时不是黑盒；
- 深度偏置是**相对比例**：`threshold = nearest - depth_bias × (1 - nearest)`。透视深度在远处被压得极扁，
  固定绝对值会让远处物件一个都剔不掉（`is_occluded_works_far_from_camera` 钉这件事）。

### 26.3 渲染侧（`crates/r3e-render/src/occlusion.rs`）

- `OcclusionCuller` 低频、**非阻塞**读回深度预通道：`encode_readback` 编 copy → `after_submit` 发 `map_async` →
  之后某帧 `device.poll(PollType::Poll)` 取回，取不到就沿用上一份，**绝不等 GPU**；
- `DEFAULT_READBACK_INTERVAL = 4`（60fps 下约 15Hz，可设 `1..=60`），`ReadbackLayout` 按 256 字节行对齐
  （1080p ≈ 8.3 MiB/次），`inflight` 只允许一份在读；
- 接入点在 `prepare_instances` 阶段三 A（视锥剔除之后）：**只剔不透明批次**——半透明不写深度，
  拿不透明深度去剔玻璃 / 水面会让它们整层消失；蒙皮先按 `SKIN_CULL_MARGIN` 放大 AABB 再判；
- 统计进 `RenderStats::occlusion`：`enabled / tested / occluded / readback / levels`，
  同时出现在运行日志的「遮挡剔除」行与编辑器 `engine_stats` 的 `render.occlusion`。

### 26.4 场景与脚本

```json
{ "occlusion": { "enabled": true, "depth_bias": 0.05, "tile_size": 8 } }
```

| 脚本 API | 语义 |
| --- | --- |
| `set_occlusion_enabled(bool)` / `occlusion_enabled()` | 开关（写入即 `sanitized()`） |
| `set_occlusion_bias(f64) -> f64` / `occlusion_bias()` | 深度偏置，返回夹取后的实际值 |

调参经验：`depth_bias` **越大约保守**（越难剔），调 0 最容易剔也最容易误剔；`tile_size` 越大金字塔越粗、
判定越快也越容易糊掉遮挡关系。

### 26.5 怎么验证

```powershell
# CPU 契约（core 28 + 场景 8 + render 6 + script 2）
cargo test -p r3e-core -p r3e-render -p r3e-script occlusion

# 画面没漂（缺省关闭，必须与扩展二十一基线一致）
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture
```

打开后看日志那行：`段内开关 true / 生效 true / 金字塔 N 层 / 判定 M / 遮挡剔除 K`。
刚打开时会有几帧「开关 true / 生效 false」——金字塔还在读回路上，属正常。
「该剔的有没有剔干净」「有没有物件闪进来又闪出去」需要人眼判断，自动化只能证明关闭时无变化。

### 26.6 测试覆盖

新增 44 条，全部 CPU 侧、无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/occlusion.rs` | 28 | 金字塔取 tile 最远值、非有限值当最远、逐层收缩且保持保守、退化输入被拒；物体在墙前判可见 / 在墙后判遮挡、偏置生效、固定偏置在远处失效（回归）、任一 tile 空即可见、屏外可见、超 `max_tiles` 换更粗一层；投影居中 / 退化输入 / 相机后方 / 跨近平面；设置夹取、`review()` 更严格、JSON 往返 |
| `crates/r3e-core/tests/occlusion_scene.rs` | 8 | 缺段 / 只写 `{}` 都保持关闭、部分字段保留、非法值加载时夹取、`set_occlusion` 吃掉 NaN 偏置、默认值不落盘、JSON 往返、开关不改其余参数 |
| `crates/r3e-render/src/occlusion.rs` | 6 | 读回布局拒退化尺寸、行按 256 对齐、整数倍不多补一行、`readback_interval` 夹到 `1..=60`、新建 culler fail-open、`reset()` 同时清金字塔与统计 |
| `crates/r3e-script/src/lib.rs` | 2 | `set_occlusion_enabled` 双向生效、偏置经脚本写入被夹取 |

`cargo test --workspace` 当时为 59 个套件 / 870 passed / 0 failed（`--features rapier` 为 874 passed）。

### 26.7 已知边界

- **不是 GPU 遮挡查询**：没有 query 池，判定在 CPU，成本随「通过视锥的实例数」线性上升；
- **滞后**：用上一帧深度（默认 4 帧一次），快速转相机时可能看到物件闪进闪出；无「连续 N 帧才剔」的迟滞；
- **不筛遮挡体**：上一帧画了什么就全当潜在遮挡体，没有「大遮挡物优先」的挑选；
- **只按 AABB 矩形判**：细长遮挡体（栅栏、树枝）和「从门缝露一条」判不出来；
- **只覆盖不透明批次**：半透明永不参与；层级剔除自扩展三十起已有（物件侧按 `CasterTree` 整枝跳，
  子树盒判为被挡即整枝不判；阴影遮挡体那一层早在扩展二十五就有，见
  [§9 方向光阴影](#9-方向光阴影阶段三-d-已完成)）——代价是整枝误判时一整枝会一起缺失，而不只是一个实例；
- **读回带宽不随场景缩放**：分辨率决定每帧 copy 的字节数（1080p ≈ 8.3 MiB/4 帧）。

---

## 27. HDR 立方体反射探针（扩展二十三）

一句话：**加载一张 Radiance `.hdr`（或立方体六面），在 CPU 上离线预滤波成一条 mip 链，再按材质粗糙度取层采样；
没配探针时镜面反射照旧走解析 IBL，画面逐像素不变。** 缺省不配，配置后是「整场景单探针」。

### 27.1 三条不变量

1. **不配就是零成本**：`Scene::environment_map == None` 或 `is_active() == false` 时，材质绑定的是 1×1 占位图、
   `CameraUniform.env_probe.x = 0`，着色器完全短路，GPU 命令与像素与扩展二十二一致（视觉基线无需重建）；
2. **写入一律收敛**：脚本与场景两条入口都过 `EnvironmentMapSettings::sanitized()`，
   `face_size` 夹 `4..1024` 并向上取 2 的幂、`mip_levels` 夹 `1..=10`、六面必须恰好 6 条否则整段作废——脚本传 NaN 不会污染 uniform；
3. **加载失败只退不退崩**：坏图 / 缺图 / 非 RGBE 只打一行 WARN 然后退回解析 IBL，不 panic、不逐帧重试刷屏。

### 27.2 core 侧（`crates/r3e-core/src/envmap.rs`）

- `decode_radiance(bytes)`：只认 Radiance RGBE——**新旧两种扫描行编码都支持**（新式 `2,2,hi,lo` 分通道 RLE + 旧式扁平），
  头部 `FORMAT=32-bit_rle_rgbe` 校验，`MAX_EQUIRECT_PIXELS = 64 Mi` 防尺寸炸弹；
- `equirect_to_cubemap(img, face_size)`：**方向驱动**的双线性重采样——每个立方体纹素先算方向再回查等距柱状 UV
  （经度环绕、纬度夹极），比「把图切六块」保真；
- `prefilter_specular(cube, levels)`：第 0 层**复用原图**保清晰反射，第 `i` 层按 `roughness = i/(levels-1)` 做余弦幂卷积
  （GGX `α = r²` 反解 Phong 指数 `k = 2/α² - 2`，32 次分层黄金角采样）；
- 三个纯函数互逆：`equirect_uv(dir)` / `cube_face_dir(face, u, v)` / `cube_dir_face(dir)`，
  `CUBE_FACE_NAMES` 钉死 `+X -X +Y -Y +Z -Z`；
- `EnvironmentMapSettings { enabled, equirect, faces, intensity, face_size, mip_levels }`，
  `with_equirect` / `with_faces` 两个构造器，`is_active()` 是唯一判据；常量 `CUBE_FACES = 6` / `DEFAULT_CUBE_FACE_SIZE = 128` / `MAX_SPECULAR_MIPS = 10`。

### 27.3 渲染侧（`crates/r3e-render/src/envprobe.rs`）

- 成品编码成 **`Rgba16Float` 立方体纹理**（自实现 `f32_to_f16` / `f16_to_f32`，纹素无行距 padding），逐 mip 逐一面上传；
- 绑到**材质组** `@group(1) @binding = 6`，采样器复用 base color 那个——不新增绑定、不改管线布局；
- `CameraUniform` 352 → **368 字节**，新加 `env_probe: vec4`（启用位 / mip 层数 / 强度 / 面边长）；
- 着色器里 `probe_specular` 用 `textureSampleLevel` 按 `roughness × (levels-1)` 取层；漫反射顺手改成探针第 0 层的辐照度投影，
  不额外加绑定；`env_probe.x == 0` 时整段短路；
- 换图 / 换面边长 / 换 mip 层数都要**重建材质 bind group**（纹理视图变了）；`EnvProbeStats` 记录来源、面边长、mip 层数、是否生效。

### 27.4 场景与脚本

```json
{ "environment_map": { "equirect": "assets/env/studio.hdr", "intensity": 1.5, "face_size": 128, "mip_levels": 6 } }
```

六面写法（顺序即 `+X -X +Y -Y +Z -Z`）：

```json
{ "environment_map": { "faces": ["px.hdr", "nx.hdr", "py.hdr", "ny.hdr", "pz.hdr", "nz.hdr"] } }
```

`faces` 与 `equirect` 都写时 `faces` 优先（省一次重采样）。脚本 API 见上文「HDR 立方体反射探针（扩展二十三）」表；
编辑器侧走 `scene.set_environment_map` 的补丁式 RPC（`EnvironmentMapPatch`），与 `scene.set_environment` 是两条独立的线。

宿主 `app.rs::sync_env_probe` 三道判定：**未激活 → 卸载**（清纹理 + 统计归零）/ **配置没变 → 不碰 GPU** / **上次失败 → 不重试**。

### 27.5 怎么验证

```powershell
# CPU 契约（core 22 + 场景 4 + env_probe 6 + render 7 + 着色器 9 + script 4 + editor 7）
cargo test -p r3e-core -p r3e-render -p r3e-script -p r3e-editor env

# 画面没漂（不配探针必须与扩展二十二基线一致）
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture

# 实机取证：跑 3 秒后看日志
cargo run --release -- <scene.json> <script.rhai> --assets-root <assets 目录> --window-size 480x270 --run-seconds 3
```

日志应出现 `反射探针已加载：来源 equirect / 面边长 128² / mip 6 层 / 强度 1.50`，
以及每帧收尾探针 `HDR 反射探针：段内 true / 开关 true / 来源 equirect / 生效 true / …`。
把 `.hdr` 换成截断文件再跑，只会看到一行 `反射探针加载失败（… 不是有效的 Radiance HDR（RGBE））：镜面反射退回解析 IBL`，
探针行变 `生效 false`，进程照常退出。

### 27.6 测试覆盖

新增 58 条，全部 CPU 侧、无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/envmap.rs` | 22 | RGBE 新旧两种扫描行解码、头部校验与坏输入拒收、`MAX_EQUIRECT_PIXELS` 边界；等距柱状 ↔ 立方体方向互逆、经度环绕与极点夹取、面边长归一化；预滤波层数与粗糙度单调、第 0 层复用原图、强度夹取、`f32` 往返 |
| `crates/r3e-core/tests/environment_map_scene.rs` | 4 | 缺段 / `{}` / 只有 `enabled` 都不激活、非法值加载时夹取、`set/clear` 不改其余字段 |
| `crates/r3e-core/tests/env_probe.rs` | 6 | 方向映射三函数互逆、六面顺序、面边长与 mip 层数收敛 |
| `crates/r3e-render/src/envprobe.rs` | 7 | `f32_to_f16` / `f16_to_f32` 往返与特殊值、mip 取层映射、上传纹素数量、`EnvProbeStats` 归零 |
| `crates/r3e-render/tests/env_probe_shader.rs` | 9 | naga 无窗口校验 WGSL：入口点、`env_probe` 字段、`probe_specular` 分支与混合状态 |
| `crates/r3e-script/tests/env_map_api.rs` | 4 | 8 个绑定写后读、六面不足 6 条返回值、非法强度被夹取、`clear` 后 `has_env_map` 为 false |
| `crates/r3e-editor/tests/environment_map_rpc.rs` | 7 | `EnvironmentMapPatch` 补丁合并、非法字段不透传、快照回显与 `scene.set_environment` 互不干扰 |

该节收尾时为 79 个套件 / 1,124 passed；截至扩展四十五为 **86 个套件 / 1,247 passed / 0 failed**（`--all-features` 同 86 套件 1,251 passed）。

### 27.7 已知边界

- **静态探针**：只吃离线 `.hdr`，不会抓运行中场景做实时反射；也没有平面反射 / SSR；
- **整场景单探针无混合**：所有物件共享一张环境图，没有「按区域挑探针 / 探针之间插值」；
- **只认 Radiance RGBE**：`.exr` / `.ktx2` / 浮点 PNG 一律不认；也没有硬件压缩（Rgba16Float 直传，显存按 `面边长² × 6 × mip ≈ 4B/纹素` 估）；
- **预滤波在 CPU 且串行**：128² × 6 面 × 6 层在实机上毫秒级，但 512 面以上加载时能感到停顿；
- **无多重重要性采样校正**：极粗糙金属的掠射角可能偏亮，属已知的 IBL 近似代价；
- **换图即重建材质 bind group**：频繁热切换 `.hdr` 会有一次 bind group 重建开销（不是每帧）。

> 注：仓库**不带**示例 `.hdr`（避免二进制资产进库）。要试效果，自己放一张 Radiance `.hdr`，按 §27.4 写进场景即可。

---

## 28. 图形 API 抽象层（扩展四十四）

一句话：**引擎的宿主面只认「中性身份 + 中性能力」，wgpu 是今天唯一的实现；接第二套图形 API 只改三处，
渲染器与调用方都不改。**

| 层 | 位置 | 只认什么 |
| --- | --- | --- |
| 宿主层 | `r3e-app` / `r3e-editor` / 插件 | `GfxApi` / `GfxInfo` / `caps_line()` |
| 门面层 | `Renderer` + `gfx::RenderBackend` | 身份 / 能力 / 尺寸 / resize |
| 获取层 | `gfx::selection` → `gfx::create_context` → `gfx::backend::open` | 设备怎么弄到手（唯一碰 Instance / Adapter 的地方） |
| 实现层 | `renderer.rs` 其余部分 | wgpu 具体绘制 |

### 28.1 怎么选 API

```powershell
cargo run                                       # 未设 → 平台偏好表首项（Windows 上 = wgpu）
$env:R3E_GFX_API = "wgpu"; cargo run            # 点名 wgpu；起不来 → 退出码 3
$env:R3E_GFX_API = "auto"; cargo run            # 同"未设"，但日志记 source=env
$env:R3E_GFX_API = "vulkan"; cargo run          # 拼错 → Err + 退出码 3（不静默回落）
```

`R3E_GFX_API` 管的是**API 家族**；`WGPU_BACKEND`（扩展四十三）管的是**wgpu 内部走哪个后端**。两者正交：
`R3E_GFX_API=wgpu` + `WGPU_BACKEND=dx12` 表示"用 wgpu 这套 API，落到 DX12"。

### 28.2 中性能力面

`renderer.gfx_info()` 返回 `GfxInfo`（`api` / `backend` / `adapter` / `device_type` / `features` / `missing` /
`requested`，全是纯数据）；`renderer.caps_line()` 压成一行进日志与遥测：

```
api=wgpu backend=vulkan device=NVIDIA GeForce RTX 5060 Laptop GPU (discrete) 特性 ... / 缺 ... 要求 ...
```

`RenderBackend` 只收 5 件事（`gfx_api` / `gfx_info` / `caps_line` / `size` / `resize`）——刻意不搬
`Renderer` 的其余方法：那是「wgpu 实现说明书」，新后端得为每个 wgpu 概念编假 API。按证据扩面。

### 28.3 怎么验证这层没被绕过

```powershell
cargo test -p r3e-render --test gfx_boundary     # 6 条源码级守卫
cargo test -p r3e-render gfx                     # 23 条内联单测（枚举 / 选路 / 能力面 / 设备描述）
```

`gfx_boundary.rs` 扫描生产源码，守住三件容易被"顺手绕过"的事：设备只在 `gfx::backend` 里获取、
`Renderer` 公开面不再出现 wgpu 类型、中性层（`mod` / `api` / `info` / `selection`）不出现 `wgpu::`。

### 28.4 测试覆盖

| 文件 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-render/src/gfx/api.rs` | 5 | `GfxApi` 名称往返、`from_name` 大小写与空格、每个平台都有偏好行、未知平台退回 `ALL` |
| `crates/r3e-render/src/gfx/selection.rs` | 7 | `R3E_GFX_API` 未设 / `auto` / 空 / 具名 / 拼错即 `Err`、显式时候选唯一、`describe` 带来源 |
| `crates/r3e-render/src/gfx/info.rs` | 5 | `caps_line` 保留探针字段、无特性时写 none、缺特性不失败、`device_type` 不碰后端类型 |
| `crates/r3e-render/src/gfx/backend.rs` | 3 | 设备类型名映射、未实现的 API 在碰 GPU 前就被拒、`DeviceDescriptor` 用 WebGPU 基线限制 |
| `crates/r3e-render/src/gfx/mod.rs` | 3 | 选路可在无 GPU 环境读、`backend_was_explicitly_requested` 在干净环境为 false、候选必含选中项 |
| `crates/r3e-render/tests/gfx_boundary.rs` | 6 | 设备获取唯一位置、`Renderer` 不吐后端类型、中性层保持中性、`Renderer` 实现契约、平台偏好表自洽、只列已实现 API |

### 28.5 已知边界

- **只实现 wgpu**：`GfxApi` 里没有 `Vulkan` / `Metal` / `D3D12` 变体 —— 枚举里不放假选项；
- **没有 `dyn` 运行时分发**：单后端下虚表只增加间接跳转；
- **启动时定死**：换 API 要重启，不支持一进程双后端；
- **跨 API 契约只有 5 项**：其余能力仍属 `Renderer`（wgpu 实现）；
- **接新 API 的改动面**：`GfxApi` 加变体 → `gfx::backend` 加 `open` → `PLATFORM_PREFERENCE` 插优先级（+ 新后端的绘制文件）。

---

## 29. 1D / 2D 混合空间（扩展四十五）

§21 的分层混合解决「哪部分骨骼听谁的」，但**同一层里几段片段各占多少**仍是手工数字：
`set_anim_layer_clip_weight("run", 0.35)` 这类写法把插值公式写在脚本里，走→跑→冲刺的过渡每加一档
就要重算一遍，参数一旦二维（八向移动的 x/y）还得自己拼三角形。混合空间把这段算术搬进引擎：
给层挂一个参数空间，往里摆样本（片段 + 坐标），每帧只推参数值，权重由几何位置算出来。

实现落在 `crates/r3e-core/src/blend_space.rs`（扩展四十五时 942 行 / 19 条单测，扩展四十九补三维后 1,775 行 / 30 条，纯 CPU、不碰 wgpu），
层与场景侧挂在 `anim_blend.rs` 与 `scene.rs`，脚本绑定在 `api.rs`。这是数据不是句柄：
`BlendSpace` 的 `dim` / `params` / `samples` / `value` / `triangles` / `rejected` 全部公开，渲染侧一行没改。

### 29.1 求值规则

| 维度 | 规则 |
| --- | --- |
| 1D | 样本按 `x` 排序后二分定位区间 → 区间内线性插值 → **区间外钳制到端点**（不外插：速度推到 3.0 不会把跑的动作放大成 2 倍权重，而是停在末样本 1.0） |
| 2D | Bowyer–Watson 增量剖分（`triangles` 可查）→ 定位包裹参数点的三角形 → 重心坐标当权重；**点落在剖分对角线上时两侧三角形一起 `merge` 归一**，四向中心得各 0.25，而不是「先遍历到的那一对各 0.5」 |
| 兜底 | 点不在任何三角形里（剖分有洞 / 共线退化）→ 遍历**全部点对**找最近线段投影插值，仍可用而不是掉回空权重 |
| 归一 | 权重按**片段名**合并后再归一（同名片段可出现在多个样本点），保证和恒为 1 |
| 拒收 | `add_sample` 拒空名 / 非有限坐标 / 同位置（`EPS = 1e-6`）并计入 `rejected`；片段名解析不到也计 `drop`（走 `note_rejected`），探针里看得见 |

`apply_blend_space()` 是**三态**语义：层没挂空间 → `false`（保持手写权重，老场景逐位不变）；
挂了空间但没有样本 → `false` 且**不清空现有权重**；有空间有样本 → **先清零再按名字写**，
避免参数跨档时残留出「越走越多路」的权重。

### 29.2 脚本 API（6 个）

| API | 语义 |
| --- | --- |
| `set_anim_layer_blend_1d(inst, layer, param) -> bool` | 挂一维空间，`param` 是参数名（用于日志 / 回显） |
| `set_anim_layer_blend_2d(inst, layer, px, py) -> bool` | 挂二维空间，两个参数名 |
| `clear_anim_layer_blend(inst, layer) -> bool` | 摘掉空间，层立刻回到手写权重（当前权重**冻结不跳变**） |
| `add_anim_layer_blend_sample(inst, layer, clip, x, y) -> bool` | 摆样本，`clip` 走片段表解析后存规范名；一维只认 `x`，`y` 入库归零 |
| `set_anim_layer_blend_value(inst, layer, x, y) -> bool` | 每帧推参数值，插值与钳制在内部完成 |
| `anim_layer_blend(inst, layer) -> string` | 回显：`1d(speed) n=2 drop=0 v=0.50 w=idle_loop=0.50,run_loop=0.50`，没挂空间返回空串 |

配套 `set_anim_layer_clip_weight` 仍是摘空间后接管权重的手段。样例脚本：`scripts/blend_space_demo.rhai`。

### 29.3 真机探针

```powershell
cargo test -p r3e-core blend_space                 # 30 条算法单测（排序 / 钳制 / 剖分 / 重心 / 兜底 / 拒收，含 3D 11 条）
cargo test -p r3e-script --test blend_space_api     # 5 条脚本行为（1D / 2D / 3D / 摘空间 / 缺目标）
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\scene_anim.json scripts\blend_space_demo.rhai --run-seconds 3
```

日志取证（rc 0）：1D 在 `v=0.50` 得 `idle_loop=0.50 / run_loop=0.50`，`v=1.00` 时 lerp 未用到的
`idle_loop` 权重自然归零消失，`v=1.49` 钳制在 `run_loop=1.00`；2D `n=3 tri=1 drop=1`
（重复样本被拒），三角形内重心插值权重连续且和恒为 1。

### 29.4 已知边界

- ~~**2D 剖分每次 `add_sample` 整表重建**~~：已由扩展五十一解决（样本改动只置脏标记 + 延迟提交，见 §35）；
  仍未做的是 Bowyer–Watson 的**增量插入 / 删除**（提交时仍是整表重剖，只是不再每个样本付一次钱）；
- **上限 32 样本 / 空间**（`MAX_BLEND_SAMPLES`，扩展四十九由 16 抬到 32）：三维四面体数随样本数增长，
  再往上抬要配采样策略（剪枝 / 分层），不是改个常量的事；
- ~~**无 3D 混合空间**~~：已由扩展四十九解决（`BlendDim::Volumetric3D` + Bowyer–Watson 四面体剖分，见 §33）；
- ~~**样本不携带开关**~~：已由扩展四十八解决——播放速率 / 循环标志下放到**每一路片段**
  （`ClipPlayback { speed, looping }` + `LayerClip.playback`，见 §32）。**残余**：仍是「一个片段一个速率」，
  同一片段在参数轴上不同区间要不同步频（走 → 跑的加速感）还没做，那需要「按参数区间配多份样本 + 速率随样本插值」；
- ~~**无相位同步**~~：已由扩展四十六解决（`LayerSync::Phase`，见 §30）；
  ~~**相位偏移量 / 跨层对齐**~~：已由扩展四十七补齐（`LayerClip.phase_offset` + `AnimLayer.sync_source`，见 §31）；
- **与 additive 层的组合语义未定义**：空间份额直接乘到 additive 层上，多路相加会让偏移量翻倍。

### 29.5 测试覆盖

新增 29 条用例，全 CPU 无 GPU 依赖：

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/blend_space.rs` | 19 | 1D 排序与二分、端点钳制、2D 三角形数、重心权重与和为 1、对角线两侧 `merge`、共线退化走兜底、同位置 / 空名 / 非有限拒收计数、16 上限、维度与参数名解析 |
| `crates/r3e-core`（`anim_blend` / `scene` 内联） | 6 | 三态语义（无空间 / 无样本 / 有样本）、按名字合并、先清零再写、摘空间后权重冻结、`layer_report` 的 `blend=` 段、`find_anim_clip` 解析失败进 `drop` |
| `crates/r3e-script/tests/blend_space_api.rs` | 4 | 挂空间 + 摆样本 + 推参数整链、越界钳制、重复样本被拒、摘空间回手写权重 |

该节收尾为 **86 个套件 / 1,247 passed / 0 failed**（扩展四十四为 85 套件 1,218）。

---

## 30. 层内相位同步（扩展四十六）

§29 解决「权重怎么分」，**这一层解决「分到权重之后每路从第几秒开始采」**。默认行为是整层一个时间：
层时间 1.05 s 时，2.1 s 的 `idle_loop` 走到 50%，0.8 s 的 `run_loop` 已经在 0.95 s 处停住（越界钳制）——
两路相位差 0.77，人眼里就是"跑着滑步"。打开相位锁后，每路按**归一化相位 × 自己时长**采样，
0.5 相位时两路一个停在 1.05 s、一个停在 0.4 s，脚掌落地时刻对齐。

### 30.1 机制

| 概念 | 规则 |
| --- | --- |
| `LayerSync` | `Off`（默认）/ `Phase` 两态，`parse` 大小写不敏感，`as_str` 给日志与脚本统一口径（`off` / `phase`） |
| 层相位 `phase()` | `sample_time() / duration()`，钳到 `[0, 1]`，非有限值按 0；`duration()` 取层内**最长**片段，与 `sample_time()` 同基准，故 `phase × duration == sample_time` 恒成立 |
| 每路采样时间 `synced_clip_time(i, phase)` | `相位 × 第 i 路片段时长`；层里没这一路返回 `None`；片段时长 0 返回 0 |
| 采样入口 | `sample_layer_pose` 抽出 `sample_layer_pose_impl` 作共用内核，`sample_layer_pose_synced` 只在"每路用哪个时间"上分叉——权重累加、骨骼遮罩、additive 参考帧判定三段逻辑只有一份 |
| 层时间 | **不动**：切锁那一帧 `phase` 不跳变，只有短片段的时间往回落到 `相位 × 时长` |
| 播放速率 | **不缩放时间**：相位锁只改"从哪个时刻采"，跑片段依旧按自己的时长循环；扩展四十八起每路片段可自带 `speed` / `looping`，但**相位锁层不读它**（见 §32） |

### 30.2 脚本 API（5 个）

| API | 语义 |
| --- | --- |
| `set_anim_layer_sync(inst, layer, on) -> bool` | `true` = 开相位锁，`false` = 回到整层一个时间；层不存在返回 `false` |
| `set_anim_layer_sync_mode(inst, layer, mode) -> bool` | 字符串版：`"off"` / `"phase"`（大小写不敏感），拼错返回 `false` 且不改动现状 |
| `anim_layer_sync(inst, layer) -> bool` | 回显是否开着相位锁（层不存在返回 `false`） |
| `anim_layer_phase(inst, layer) -> float` | 层相位 `0..1`，**没开锁也可读**（相位是层的属性，不是锁的产物） |
| `anim_layer_sync_probe(inst, layer) -> string` | 单行读数：`sync=phase phase=0.500 dur=2.100 drift=0.000 idle_loop=1.050/2.100,run_loop=0.400/0.800` |

`drift` 是各路片段相位与层相位的**最大绝对偏差**，开锁后按构造恒为 0。样例脚本：`scripts/layer_sync_demo.rhai`
（58 行，前 1 秒 `off`、1 秒后切 `phase`，每帧打一行 `SYNC`）。编辑器侧 `scene.anim_layer_set` 的 patch
新增 `sync` 字段，`scene.anim_layers` 快照回显 `sync` / `phase`。

### 30.3 真机探针

```powershell
cargo test -p r3e-core anim_tests                        # 其中 4 条属本次（默认 Off / 相位缩放 / 同相位对齐 / drift 量化）
cargo test -p r3e-script --test layer_sync_api            # 4 条脚本行为
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\scene_anim.json scripts\layer_sync_demo.rhai --run-seconds 2.4
```

rc 0，557 帧探针行：

| 时刻 | 策略 | 读回 | 含义 |
| --- | --- | --- | --- |
| `t=0.50` | off | `phase=0.238 drift=0.387 idle_loop=0.500/2.100,run_loop=0.500/0.800` | 两路同一个时间，折算相位 0.238 vs 0.625 |
| `t=0.95` | off | `phase=0.453 drift=0.736 run_loop=0.951/0.800` | 跑片段时间已越过自己时长，停在末帧 |
| `t=1.05` | phase | `phase=0.500 drift=0.000 run_loop=0.400/0.800` | 切锁瞬间：层相位不变，跑片段落到 `0.5 × 0.8` |
| `t=1.60` | phase | `phase=0.762 drift=0.000 run_loop=0.610/0.800` | 恒同相位；步频仍由 0.8 s 片段决定 |
| `t=2.10` | phase | `phase=0.998 drift=0.000 idle_loop=2.096/2.100,run_loop=0.798/0.800` | 循环边界两路同时回绕，不出现"一路回绕一路没回" |

### 30.4 已知边界

- ~~**不做相位偏移量**~~：已由扩展四十七补上（`LayerClip.phase_offset`，见 §31）；
- ~~**不做跨层对齐**~~：已由扩展四十七补上（`AnimLayer.sync_source` 相位源链，见 §31）；
- **不改播放速率**：相位锁不缩放时间，变速仍靠 `speed` 或样本时长；
- **短片段依靠自己的时长循环**：`Phase` 下每路按本片段时长独立回绕，长片段与短片段的"圈数"本来就不同；
- **`Off` 的层仍走老函数**：不是"新函数里再判一次分支"，因此不调新 API 的场景逐位不变，像素基线不重建。

### 30.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 4 | 默认 `Off` 且 `layer_report` 不出现 `sync=`、`Off` 下 `clip_times` 两路相等、相位锁下每路时间 = `相位 × 时长`、`sync_probe` 的 `drift` 量化为 0 |
| `crates/r3e-core/src/scene.rs` | 2 | 门面转发（`set_anim_layer_sync` / `anim_layer_sync` / `anim_layer_phase` 三态）、层不存在时返回 `None` 而不是改写别的层 |
| `crates/r3e-script/tests/layer_sync_api.rs` | 4 | 开关 + 回显整链、字符串策略解析与非法值拒绝、`Off` 下读相位仍有效、探针字符串格式 |
| `crates/r3e-editor/tests/anim_layer_rpc.rs` | 1（本次新增） | patch 带未知策略时 RPC 返错且不改层 |

该节收尾为 **87 个套件 / 1,263 passed / 0 failed**（扩展四十五为 86 套件 1,251），
`R3E_VISUAL_REGRESSION=1` 下同 87 套件 1,263 passed 且**基线图未重建**。

---

## 31. 相位同步组：偏移量与跨层对齐（扩展四十七）

§30 让同层几路**齐步走**，但编舞要的还有两种"**故意的不同相**"：同层两路错开半步（左右脚交替落地、
走+跑叠加时脚掌不撞拍），以及上下半身两层**同一个相位**（上身挥拳时下身不该还停在上一拍）。
这一节把这两件事补上，相位同步组到此收尾。

### 31.1 机制

| 概念 | 规则 |
| --- | --- |
| `LayerClip.phase_offset` | 默认 `0.0`。相位锁下该路采样相位 = `wrap(层相位 + offset)`；写入即折进 `[0, 1)`（`-0.25` 存成 `0.75`） |
| `wrap_phase(phase)` | 公开纯函数：折进 `[0, 1)`，非有限值按 0。`0.75 + 0.5 = 1.25 → 0.25` |
| 为什么偏移折、层相位不折 | 错半步是**循环量**；而层相位 `1.0` 是"末帧端点"，取模会退回首帧。故 `normalized_phase`（钳 `[0,1]`）管层相位、`wrap_phase`（折 `[0,1)`）只管"叠加偏移之后" |
| `AnimLayer.sync_source` | 默认 `None`：本层相位跟随哪一层，**按层名**（层可在运行中增删，下标不稳定） |
| `phase_source(layers, i)` | 沿链找终点层，最多 `MAX_ANIM_LAYERS`（8）跳；名字解析不到 / 指向自己 / 成环 → `None` |
| `effective_phase(layers, i)` | `Off` 直接返回本层相位；`Phase` 下挂了可用源层就取源层相位，否则本层相位 |
| 设置侧校验 | `set_anim_layer_sync_source` 写入后立刻验一次链，失败**回滚**并返回 `false`——不留半截链，采样期只需处理"有源层 / 无源层"两种情形 |
| 叠加顺序 | 「先取源层相位、再加本路偏移」。反序会算出完全不同的落点 |
| 采样时序 | `update_animations` 先算一遍 `sync_phases`（`Off` 层填 `None`），再进 `iter_mut()` 采样：可变借用期间借不到别的层，也杜绝迭代顺序依赖 |
| `drift` 的定义 | 期望相位 = `clip_phase(层相位, 该路 offset)`，**故设了偏移的层 drift 仍恒为 0**；一旦偏移没进 `synced_time`，drift 会立刻等于 `|offset|` |
| 零变化支点 | `clip_phase` 里 `offset == 0.0` 直接返回原值（含 `1.0` 端点），所以全零偏移的层与 §30 逐位相同 |

### 31.2 脚本 API（5 个）

| API | 语义 |
| --- | --- |
| `set_anim_layer_clip_phase_offset(inst, layer, clip, off) -> bool` | 给某一路设错步偏移（`off` 折进 `[0, 1)`）；实例 / 层 / 片段不存在返回 `false` |
| `anim_layer_clip_phase_offset(inst, layer, clip) -> float` | 回显偏移（没设过是 `0.0`） |
| `set_anim_layer_sync_source(inst, layer, src) -> bool` | 设相位源层名；空串 = 清除、跟回自己。源层不存在 / 指向自己 / 成环返回 `false` 且状态回滚 |
| `anim_layer_sync_source(inst, layer) -> string` | 回显源层名（没设过是空串，方便直接打印） |
| `anim_layer_sync_phase(inst, layer) -> float` | **跨层对齐后的有效相位**（真正用于采样的那个）；没挂源层时等于 `anim_layer_phase` |

样例脚本：`scripts/phase_group_demo.rhai`（67 行，三段切换，每帧一行 `PHASE`）。编辑器侧
`scene.anim_layer_set` 的 patch 新增 `phaseOffset`（片段名 → 偏移）与 `syncSource`，
`scene.anim_layers` 快照回显 `syncSource` / `syncPhase`。

### 31.3 真机探针

```powershell
cargo test -p r3e-core anim_tests                          # 其中 4 条属本次（偏移折回 / 零偏移老式子 / drift 仍为 0 / 相位源链与断环）
cargo test -p r3e-script --test layer_sync_group_api        # 5 条脚本行为
target_64-pc-windows-gnu\debug
3e.exe assets\scene_anim.json scripts\phase_group_demo.rhai --run-seconds 3.2
```

夹具 `assets/scene_anim.json` 挂两层：`lower` = `idle_loop`(2.100 s) + `run_loop`(0.800 s) 各 0.5 权重，
`upper` = 单路 `run_loop`、`speed 0.5`，两层都开相位锁。rc 0，835 行日志中 **749 行 `PHASE`**
（`off=0.0` 223 行 / `off=0.5` 无源层 239 行 / `src=upper` 287 行）：

| 时刻 | 阶段 | 探针读回（节选） | 含义 |
| --- | --- | --- | --- |
| `t=0.000` | 一 全默认 | `off=0.0 src= phase=0.000 sync_phase=0.000 … drift=0.000 idle_loop=0.000/2.100,run_loop=0.000/0.800` | 行进与 §30 逐字一致：**没有 `off=` / `src=` 段** |
| `t=0.996` | 一 | `phase=0.474 sync_phase=0.474 … idle_loop=0.996/2.100,run_loop=0.379/0.800` | 0.474 × 0.800 = 0.379：两路仍同相位 |
| `t=1.050` | 二 首行 | `off=0.5 … run_loop=0.800/0.800` | 只把 `run_loop` 推到 `wrap(0.5 + 0.5)` 的临门，`idle_loop` 一位未动 |
| `t=1.112` | 二 | `off=0.5 … idle_loop=1.112/2.100,run_loop=0.024/0.800` | **偏移进了采样**：跑片段从 0.800 附近跳回 0.024 = `wrap(1.030) × 0.800` |
| `t=0.699` | 三 挂源层 | `off=0.5 src=upper phase=0.333 sync_phase=0.750 … idle_loop=1.574/2.100,run_loop=0.200/0.800` | 本层相位 0.333 不再参与采样，落点全按 0.750 算 |
| `t=1.091` | 三 后段 | `off=0.5 src=upper phase=0.520 sync_phase=0.995 … idle_loop=2.089/2.100,run_loop=0.396/0.800` | 跨层与偏移**同时**生效：`0.995 × 2.100` 与 `wrap(0.995 + 0.5) × 0.800` |

阶段切换行只在状态真变时才打印：

```
[script] === 加错步半步 ok=true off=0.5
[script] === 跨层对齐 ok=true src=upper upper_phase=0.2503...
[script] 层已改：lower mode=override w=0.50 clips=["idle_loop", "run_loop"] mask=0 sync=phase src=upper
```

最后那行的 `sync=phase src=upper` 来自 `layer_report`，证明跨层关系进了**层状态**而不只是脚本局部变量。

### 31.4 已知边界

- **只在实例内跨层**：源层必须同属一个 `AnimInstance`，不做跨实例 / 跨角色对齐（多角色同步属上层编排）；
- **只对 `Phase` 生效**：`Off` 的层不读 `sync_source` 也不读 `phase_offset`（采样根本不看相位），但**回显照旧返回设定值**——允许先配好再拨开关；
- **偏移是常数**：不做"偏移随参数变化的曲线"（那要第二个混合轴）；
- **不改播放速率**：偏移只挪采样相位，不改层 `speed`、不缩放时间；
- **链长上限 8 跳**：更长的链按"无源层"处理，不做动态拓扑排序；
- **成环在设置侧就拒**：不留半截链，也不在采样期死循环。

### 31.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 4 | 偏移折进 `[0,1)`（`-0.25→0.75`）、零偏移与 §30 式子逐位相同（含 `1.0` 端点）、带偏移时 `drift` 仍为 0、相位源链跟随且自引用/成环断在 `None` |
| `crates/r3e-core/src/scene.rs` | 2 | 门面按片段名设偏移只挪那一路、跨层对齐后两层采样时间同相位 |
| `crates/r3e-script/tests/layer_sync_group_api.rs` | 5（新套件） | 偏移只影响目标路、未知目标返 false、跨层对齐整链、自引用/幽灵层/成环被拒、缺目标时回中性值 |
| `crates/r3e-editor/tests/anim_layer_rpc.rs` | 1（本次新增） | patch 带 `phaseOffset` / `syncSource` 时落进层状态并可回读 |

该节收尾为 **88 个套件 / 1,276 passed / 0 failed**（扩展四十六为 87 套件 1,263），
`R3E_VISUAL_REGRESSION=1` 下同 88 套件 1,276 passed 且**基线图未重建**。

## 32. 片段自带播放速率与循环标志（扩展四十八）

§30 让同层几路**齐步走**、§31 让它们**故意错开**，但"每路走多快、到头停不停"仍只有层一个开关：
`AnimLayer.speed` / `AnimLayer.looping` 是「整层共用一条时间轴」，一个层里呼吸（要慢）与摇头（要快）
长短不一就只能折中；而「到末帧停住」层级开关根本给不了——层一停整层都不动。这一节把这两件事下放到
**每一路片段**。

### 32.1 机制

| 概念 | 规则 |
| --- | --- |
| `ClipPlayback { speed, looping }` | 单路播放参数。`speed`：`1.0` 原速、`2.0` 快一倍、负值倒放、`0.0` 定格首帧；`looping = false` 到末帧停住 |
| `ClipPlayback::new(speed, looping)` | 构造即收敛：非有限速率按 `1.0`（原速），其余原样保留——`-2.0` 要能倒放、`0.0` 要能定格，`clamp` 会让这两个语义消失 |
| `ClipPlayback::time_at(time, duration)` | 本路应采样的片段内时间：`speed == 0` 恒 `0` → 否则 `time * speed` → 非有限归 `0` → 再走 `crate::anim::wrap_time`（与层时间轴**同一套**循环取模 / 非循环钳末帧） |
| `LayerClip.playback: Option<ClipPlayback>` | 默认 `None`。`None` 时这一路**原样使用层时间** |
| `LayerClip::playback_time(time)` | `playback == None` 时**原样返回 `time`**（不乘 `1.0`、不取模，超时长仍交给 track 钳末帧）——这是「不设 = 老行为」的算术支点 |
| `sample_layer_pose` | `time_of` 闭包改调 `lc.playback_time(time)`；与相位锁仍共用 `sample_layer_pose_impl`，只在"用哪个时间采每路"上分叉 |
| 时间口径 | `time` 参数始终是**层时间**，逐路折算发生在闭包里——所以层的 `time` / `speed` / `looping` / `playing` 语义一字未动 |
| `layer_report` / `sync_probe` 的 `pb=` 段 | 只列设过的路：`pb=run=2.00:once,walk=0.50`（循环是常态不标注）；一个都没设就整段不出现，默认日志行逐字不变 |
| `drift` 对主动不同速不作伪证 | `Off` + 该路有播放参数时，期望相位取**该路自己的相位**（自比恒 0）；其余情形仍是 `clip_phase(层相位, 偏移)`。没设参数的路照实报漂移，设了参数的路不背锅 |
| 为什么只在 `LayerSync::Off` 下生效 | 相位锁下每路时间由「相位 × 本片段时长」唯一决定，再按速率缩放会让同一相位采到不同位置——**相位对齐会被速率破坏**。要变速请调层 `speed`（全层统一缩放，不破坏相对对齐） |

定格为什么写成 `speed == 0 → 0` 而不是靠 `time * 0.0`：后者在 `time` 为 `NaN` / `inf` 时得 `NaN`，
虽被后面的非有限归零兜住，但那是"碰巧对"；写成显式分支，语义是"时间轴钉死在 0"，后人改不坏。
负速率能自然倒放同理：`wrap_time` 内部用 `rem_euclid`，`-0.6` 落进 `[0, d)` 就是 `d - 0.6` 一类结果，
不需要第二套采样代码，也不需要给 `AnimClip` 加反向播放状态。

### 32.2 脚本 API（5 个）

| API | 语义 |
| --- | --- |
| `set_anim_layer_clip_playback(inst, layer, clip, speed, looping) -> bool` | 设某一路的播放参数；`clip` 空串 = 层内第一路、名字大小写不敏感；实例 / 层 / 片段不存在返回 `false` 且状态不动 |
| `clear_anim_layer_clip_playback(inst, layer, clip) -> bool` | 清除，回到"跟着层时间轴走" |
| `anim_layer_clip_speed(inst, layer, clip) -> float` | 回显速率；**没设过返回 `-1`**（`0` 是合法的"定格首帧"，不能当缺省） |
| `anim_layer_clip_looping(inst, layer, clip) -> bool` | 回显循环标志；没设过返回 `true`（跟层走的默认口径） |
| `anim_layer_clip_time(inst, layer, clip) -> float` | 该路**这一帧实际采到的时间**（秒，含速率 / 循环折算）；层 / 片段不存在返回 `-1`。同一层时间下两路这个值不同，就是速率生效了 |

样例脚本：`scripts/clip_playback_demo.rhai`（86 行，三段：不设 / 给 `run_loop` 设 `1.5` 且不循环 / 清除，每帧一行 `CLIP`）。
编辑器侧 `scene.anim_layer_set` 的 patch 新增 `clip_speed` / `clip_looping` 两个 `Option` 字段，可单独下发：
只给循环时沿用当前速率（没设过按 `1.0`，避免"只想停住"被顺手改速），只给速率时沿用当前循环标志（没设过按循环）。

### 32.3 真机探针

```powershell
cargo test -p r3e-core anim_tests                          # 其中 4 条属本次
cargo test -p r3e-script --test clip_playback_api           # 4 条脚本行为（新套件）
target_64-pc-windows-gnu\debug
3e.exe assets\scene_anim.json scripts\clip_playback_demo.rhai --run-seconds 2.0
```

夹具 `assets/scene_anim.json` 的 `body` 层挂两路：`idle_loop` 2.100 s、`run_loop` 0.800 s。
rc 0，501 行日志中 **414 行 `CLIP`**，其中 **221 行带 `pb=`**：

| 时刻 | 阶段 | 探针读回（节选） | 含义 |
| --- | --- | --- | --- |
| `t=0.495` | 一 不设参数 | `run_t=0.495 run_speed=-1.0 … drift=0.383`，**无 `pb=` 段** | `run_t` 与 `idle_t` 逐位相等，行进与 §31 逐字一致 |
| `t=0.501` | 二 首行 | `pb=run_loop=1.50:once run_t=0.751 idle_t=0.501 drift=0.000` | `0.751 = 1.5 × 0.501`：速率进了采样时间，不是只写进字段 |
| `t=1.059` | 二 后段 | `run_t=0.800 idle_t=1.059` | 钳在片段时长 `0.800`（`looping=false` 停末帧），而 `idle_loop` 仍在走——层级开关做不到 |
| `t=1.503` | 三 清除后 | `run_speed=-1.0 run_t=1.503`，**`pb=` 消失** | 彻底回到阶段一那条老路，不是"速率改回 1.0 但仍走新代码" |

阶段二全 220 行 `drift=0.000`：`Off` + 有播放参数的那一路期望相位取它自己的相位，
"用户主动设的不同速"没被误报成滑步；同段未设参数的 `idle_loop` 仍按老口径比对。
阶段一那 0.383 的漂移是 0.8 s 短片段已经停住带来的真实滑步，旧口径的老毛病照实报。

### 32.4 已知边界

- **一个片段一个速率**：不做"速率随参数区间变化"（`walk→run` 的步频是两条片段各自的 `speed`，
  不是同一条片段在参数轴上插值速率）；
- **不改层时间轴**：`AnimLayer.time` / `speed` / `looping` / `playing` 一字未动，播放参数只决定"这一路用哪个时间采"；
- **只在 `Off` 下生效**：`Phase` 层不读 `playback`（回显照旧返回设定值，允许先配好、再切策略）；
- **不碰 `fade` / 权重 / 遮罩 / additive**：权重归一与合成口径不变，参数只作用于时间；
- **不倒放骨骼**：负速率只把采样时间折回区间，不做姿态镜像。

### 32.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 4 | 缩放与取模（2× / 非循环钳末帧 / 负速率 `rem_euclid` 落回 / 定格恒 0 / 非有限按原速 / 清除后原样透传）、2× 速真的改变采样落点（`x` 由 2.5 → 5.0，清除后逐位回到 2.5）、`pb=` 只在设过后出现且 `drift` 归零、相位锁不读播放参数 |
| `crates/r3e-core/src/scene.rs` | 2 | 门面按片段名设/清播放参数只动那一路、`anim_layer_clip_time` 随速率变化 |
| `crates/r3e-script/tests/clip_playback_api.rs` | 4（新套件） | 速率与循环经绑定层透传、回显缺省（`-1` / `true`）、清除回老口径、未知目标返 `false` |
| `crates/r3e-editor/tests/anim_layer_rpc.rs` | 1（本次新增） | patch 的 `clip_speed` / `clip_looping` 单独下发时落进层状态并可回读 |

该节收尾为 **89 个套件 / 1,285 passed / 0 failed**（扩展四十七为 88 套件 1,276），
`R3E_VISUAL_REGRESSION=1` 下同 89 套件 1,285 passed 且**基线图未重建**。

---

## 33. 三维混合空间（扩展四十九）

§29 的混合空间只有 1D / 2D 两档参数。「八向移动 × 三档速度」是三个自变量（`move.x` / `move.y` / `speed`），
用两层 2D 空间叠：上层 `(x, y)` 归一后和恒 1、下层速度档归一后和恒 1，两者相加为 2，还得手乘一个速度系数；
每加一档速度就要多挂一层、多维护一组系数。扩展四十九把第三轴做进同一张图：
`BlendDim::Volumetric3D` + Bowyer–Watson **四面体剖分**，24 个样本（八向 × 三档）一张图解决，
脚本里只剩一句 `set_anim_layer_blend_3d(...)` + 一句 `set_anim_layer_blend_value_3d(x, y, speed)`。

实现仍在 `crates/r3e-core/src/blend_space.rs`（三维落地后 **1,775 行 / 30 条内联单测**，纯 CPU、零 wgpu），
层与场景侧挂在 `anim_blend.rs` / `scene.rs`，脚本绑定在 `api.rs`。**层内 `apply_blend_space` 一行未改** ——
三维空间吐给层的还是「片段名 → 权重、和为 1」，所以**渲染侧零改动**：没有新 uniform、没碰着色器、
不调新 API 的场景像素逐位不变。

### 33.1 求值规则

| 情形 | 结果 |
| --- | --- |
| 样本数 ≥ 4 且不共面 | 体剖分成若干四面体；参数落到哪个四面体内就用它的**重心坐标**当权重（`volumetric_3d_single_tetra_has_exact_barycentric`） |
| 四个不共面点的最小配置 | 恰好 **1 个**四面体（探针里 `tet=1`），内部点四顶点重心，权重和恒 1 |
| 参数在凸包**外** | 权重**钳制**到最近的**凸包面**上，不外插（`volumetric_3d_outside_clamps_to_hull_face`）；`v` 仍存原值，只有权重被钳 —— `v=(1.49,0,0)` 得到 `run_loop=1.00` 而不是 1.49 |
| 参数正好在顶点 / 棱上 | 与顶点样本逐位一致；权重为 0 的片段**整条从结果里消失**，不打成 `=0.00`（`volumetric_3d_vertex_and_edge_are_exact`） |
| 同一片段占多个顶点 | 按**片段名合并**求和（`merge`）：四面体内部 `(0.30,0.30,0.30)` 里 `run_loop` 占两个顶点，合并成 0.60 |
| 样本少于 4 个 / 全部共面 | **不硬剖**，退「最近样本拿 1.0」（`volumetric_3d_coplanar_falls_back_to_nearest_sample`），`brief()` 里 `tet=0` 把这件事显式暴露 —— 要精确插值就给足四个不共面的样本 |
| 3 个样本 | 仍归一（`volumetric_3d_with_three_samples_still_normalizes`），只是走兜底 |
| 重复位置样本 | 挂样本时就被**拒收并计数**（`drop`），不是"接受后由合并抹平"（`volumetric_3d_rejects_duplicate_position_on_all_axes`） |
| 非有限参数（NaN / inf） | 拒收，保留旧值（`volumetric_3d_ignores_non_finite_value`） |
| 在 1D / 2D 空间上调 `_3d` 入口 | 等价于原入口：`add_sample(clip, [x, y])` 就是 `add_sample_3d(clip, [x, y, 0.0])` 的壳，`set_value(x, y)` 就是 `set_value_3d(x, y, 0.0)` 的壳（`set_value_1d(x)` 同理 = `set_value(x, 0.0)`），高位分量被维度归一吃掉 |

剖分的实现细节（f64 中间量、`VOLUME_EPS=1e-12`、超级四面体包含性校验、凸包面顺手留用、定向统一）见
`docs/ARCHITECTURE.md` §4.25；真机逐行取证见 §8.36。这里只记用户可见口径。

### 33.2 脚本 API

| API | 作用 |
| --- | --- |
| `set_anim_layer_blend_3d(inst, layer, "move.x", "move.y", "speed")` | 给层挂三维参数空间，后三个参数是**参数名字符串**。返回 `true` |
| `add_anim_layer_blend_sample_3d(inst, layer, clip, x, y, z)` | 摆一个样本。重复位置 / 非有限坐标返回 `false` 并计数 |
| `set_anim_layer_blend_value_3d(inst, layer, x, y, z)` | 每帧推参数三元组；权重由引擎算，不用自己归一 |
| `clear_anim_layer_blend(inst, layer)` | 摘掉空间（三维复用这个，没另起函数），层回到手写权重 |
| `anim_layer_blend(inst, layer)` | 回显，三维空间打印成 `3d(move.x,move.y,speed) n=4 tet=1 drop=1 v=(0.30,0.30,0.30) w=atk=0.30,run_loop=0.60,idle_loop=0.10` |

`tet=` 是体剖分出的四面体数（`0` = 没剖起来，走兜底），`drop=` 是累计被拒的样本数 —— 一行日志就能看出
空间是"真在插值"还是"塌成最近样本"。

### 33.3 真机探针

```powershell
cargo test -p r3e-core blend_space                  # 30 条算法单测（1D 19 + 3D 11）
cargo test -p r3e-core anim_tests                   # +2 条：三维空间驱动层权重 / 层报告附 3D 段
cargo test -p r3e-script --test blend_space_api     # 5 条脚本行为
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\scene_anim.json scripts\blend_space_3d_demo.rhai --run-seconds 3.2 --show-bones
```

演示脚本 `scripts\blend_space_3d_demo.rhai`（76 行）拿 `assets\scene_anim.json` 的 `tirger/idle_loop`（7 片段），
挂一层 `move`：`3d(move.x, move.y, speed)`，四个**不共面**样本 `idle_loop@(0,0,0)` / `run_loop@(1,0,0)` /
`atk@(0,1,0)` / `run_loop@(0,0,1)`，外加一条故意重复的 `idle_loop@(0,0,0)` 验拒绝路径。三段墙钟切换：
沿 x 轴扫 → 钻进四面体内部 `(0.30,0.30,0.30)` → 踩 `+z` 顶点。

`rc 0`，765 行日志，其中 **680 行 3D 探针**（阶段一 292 + 阶段二 234 + 阶段三 154），全段 `n=4 tet=1 drop=1`：

| 阶段 | 参数 | 读回权重 | 说明 |
| --- | --- | --- | --- |
| 一 | `v=(0.45,0,0)` | `idle_loop=0.55, run_loop=0.45` | 棱上线性，严格 `1−x / x` |
| 一 | `v=(1.00,0,0)` | `run_loop=1.00` | 顶点，`idle_loop` 整条消失（零权重不打印） |
| 一 | `v=(1.49,0,0)` | `run_loop=1.00` | 越界：权重钳在顶点，不外插 |
| 二 | `v=(0.30,0.30,0.30)` | `idle_loop=0.10, atk=0.30, run_loop=0.60` | 四面体**内部**真插值，`run_loop` 两顶点合并成 0.60 |
| 三 | `v=(0,0,1)` | `run_loop=1.00` | `+z` 顶点，塌成单路 |

批量核对（680 行逐行，不抽样）：权重和 `max|Σw − 1| = 0.0`；无 NaN；阶段一 **185 行**落在四面体内
（`x ≤ 1`）且 `run_loop` 权重与 `v.x` 在两位小数内零误差；**107 行**越界（`x ∈ (1.00, 1.49]`）权重只有
`run_loop=1.00`；阶段二 234 行、阶段三 154 行各自权重**逐帧不动**（参数不动则权重不抖）。

**1D / 2D 零变化**：同一二进制、同一份资产、跑 §29 的老脚本 `blend_space_demo.rhai`，与 §29.3 的表逐行对上
（`1d(speed) n=2 drop=0` / `2d(aim.x,aim.y) n=3 tri=1 drop=1`，权重与越界钳制一字不差）——
上限 16 → 32 与样本坐标升三维没有改变任何旧结果。

### 33.4 已知边界

- **上限 32 样本 / 空间**（`MAX_BLEND_SAMPLES`）：八向 × 三档 = 24 尚有余量；四面体数最坏 `O(n²)`，
  再往上抬要配采样策略（剪枝 / 分层）或直接分图，不是改个常量的事；
- **只做体剖分，不做曲面混合**：每轴不同量纲（速度 vs 方向）不会自动归一，量纲由调用方换算后喂进来；
- **共面 / 少于四样本不硬剖**：退最近样本（`tet=0`），只有"最近一路"，不会给一个看着平滑其实错的值；
- **不做 4D 及以上**；
- **一份样本一份片段**：同一片段可在多个顶点出现（按名合并），但"同一片段在参数轴上不同区间要不同步频"
  仍靠 §32 的 `ClipPlayback` 按路配，不随三维参数插值；
- **不碰权重归一 / 遮罩 / additive**：三维只决定层内片段权重，层间合成口径与 §22 一致。

### 33.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/blend_space.rs` | 11（内联，累计 30） | 四面体精确重心 / 顶点与棱 / 越界钳到凸包面 / 立方体剖分有效性 / 共面退最近样本 / 三样本仍归一 / 非有限拒收 / `brief()` 与维度解析 / 三维重复位置拒收 / 速度档边界（`z=0` 或 `1`）不串档 / 24 样本可剖 |
| `crates/r3e-core/src/anim_blend.rs` | 2 | 三维空间驱动层内片段权重、层报告追加 `3D` 段 |
| `crates/r3e-core/src/scene.rs` | 1 | 场景门面 `_3d` 入口改权重只动目标层 |
| `crates/r3e-script/tests/blend_space_api.rs` | 1（累计 5） | `script_3d_blend_space_drives_layer_weights`：绑定层透传三元组、权重进层、摘空间后回手写权重 |

该节收尾为 **89 个套件 / 1,296 passed / 0 failed**（扩展四十八为 89 套件 1,285），
`--all-features` 同 89 套件 **1,300 passed**；`R3E_VISUAL_REGRESSION=1` 下**基线图未重建**。

---

## 34. 加法层与混合空间的组合语义（扩展五十）

§29 / §33 讲的是「一层里的多路片段怎么按参数分份额」，§22 讲的是「加法层怎么把增量叠到基姿态上」，
两者相接处有个一直没写死的问题：**加法层里的份额，是「姿态的几成」还是「增量的几成」**。
扩展四十九之前加法层与覆盖层共用 `sample_layer_pose` 这同一条采样路径，于是多路份额被当成**姿态混合权重**——
先把各路姿态加权平均，再拿「平均姿态 − 平均参考」去和基姿态相加。位置项因为线性恰好等价，
**旋转（四元数混合）与缩放（比值混合）会被平均钝化**：`0.5 × 左倾 30° + 0.5 × 右倾 30°` 本该互相抵消，
旧口径给出的是「15° 的左倾」—— 增量为零的那一路，在稀释别人的增量。扩展五十把这条路径拆开。

实现全在 `crates/r3e-core/src/anim_blend.rs`（**+298**，纯 CPU、零 wgpu）：新增
`sample_layer_pose_additive` / `sample_layer_pose_additive_synced` + 共用内核 `sample_layer_pose_additive_impl`，
`scene.rs` 在挂层采样处按 `(sync_phase, layer.mode)` 分派四态，`apply_additive` 一行未改。
**没有新 uniform、没碰着色器、不加新脚本 API**：不调加法层多路混合的场景像素逐位不变。

### 34.1 求值规则

| 情形 | 结果 |
| --- | --- |
| 多路份额的含义 | 每路份额折算成**逐路增量系数** `k = 本路权重 / 有效路权重和`，各路增量按 `k` 缩放后再复合，不是先合成一条「平均姿态」 |
| 旋转 | `pose.rotation *= slerp(identity, pose × ref⁻¹, k)` 后 `normalize`：两路反向等份的增量**回到单位旋转**（抵消），不是幅度减半（`additive_sums_rotation_deltas_not_pose_average`） |
| 缩放 | `pose.scale *= lerp(1, pose_i / ref_i, k)` 逐分量连乘；参考分量接近 0 时按 1 处理（`scale_ratio`），不除零放大（`additive_sums_scale_ratios_not_pose_average`） |
| 位置 | 线性累加 `+= (pose − ref) × k`，与旧口径数值一致 |
| 零增量基准 | 固定取**第一路有效片段**的参考姿态 —— 顺序确定、结果可复现，单路退化时自动等于旧路径 |
| 单路口 | **原样返回那一路、零算术**，与扩展四十九之前逐位一致（`additive_single_clip_is_bitwise_unchanged`） |
| 零权重 / 缺轨道的路 | **不进分母**（`additive_skips_zero_weight_and_missing_track`）：没被选中的路不参与稀释 |
| 部分权重 | 按有效路权重和重归一（`additive_normalizes_partial_weights`） |
| 相位锁层（`sync_phase`） | 片段内时间 = `phase × 本片段时长`，按**各片段自己的时长**还原（`additive_synced_uses_own_clip_duration`） |
| 覆盖层 | **一字未动**：多路仍是姿态加权平均，那是覆盖层的正确语义 |
| 零份额片段 | 整条从结果里消失（不打成 `=0.00`），与混合空间的老口径一致 |

组合口径的推导与取舍（为什么不能 `slerp(pose, ref, 1−k)`、为什么缩放用比值连乘、为什么基准固定取第一路）
见 `docs/ARCHITECTURE.md` §4.26；真机逐行取证见 §8.37。这里只记用户可见口径。

### 34.2 脚本 API

**没有新 API**，`add_anim_layer_additive` 建的加法层、`set_anim_layer_blend_2d` 挂的样本、
`anim_layer_blend` 读的份额全部照旧 —— 被纠正的是 core 内的语义，脚本一个字不用改。

| API | 作用 |
| --- | --- |
| `add_anim_layer_additive(inst, layer, clip, weight)` | 建加法层（叠的是「片段当前姿态 − 该片段首帧」这份增量） |
| `add_anim_layer_clip(inst, layer, clip, weight)` | 同一层再挂路（份额留给下面的空间分配） |
| `set_anim_layer_blend_2d(inst, layer, "move.x", "amp")` | 挂 2D 参数空间（1D / 3D 同理） |
| `add_anim_layer_blend_sample(inst, layer, clip, x, y)` | 摆样本；同一片段可占多个样本，按名合并 |
| `set_anim_layer_blend_value(inst, layer, x, y)` | 每帧推参数，权重由引擎算 |
| `anim_layer_blend(inst, layer)` | 回显，如 `2d(move.x,amp) n=3 tri=1 drop=0 v=(1.00,0.00) w=run_loop=0.50,idle_loop=0.50` |

`n=` 样本数、`tri=` / `tet=` 剖分数、`drop=` 被拒样本数、`v=` 当前参数、`w=` 各路份额 ——
日志里这几段就是判断「份额进的是增量还是姿态」的唯一现场。

### 34.3 真机探针

```powershell
cargo test -p r3e-core additive                          # 10 条（本次 +7：anim_blend 6 + 场景 1）
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\scene_anim.json scripts\additive_blend_space_demo.rhai --run-seconds 3.6
```

演示脚本 `scripts\additive_blend_space_demo.rhai`（78 行）拿 `assets\scene_anim.json` 的 `tirger`（7 片段），
建**一层** `add`：`add_anim_layer_additive(inst, "add", idle_loop, 1.0)` + 两路 `0.0` 的
`add_anim_layer_clip`（`run_loop` / `atk`），挂 2D 空间 `(move.x, amp)`，三个样本铺成直角三角形
`idle_loop@(0,0)` / `run_loop@(2,0)` / `atk@(0,2)`。四段墙钟扫过去：

`rc 0`，830 行日志，其中 **744 行 2D 探针**（阶段一 193 + 阶段二 219 + 阶段三 217 + 阶段四 115）：

| 阶段 | 参数 | 读回份额 | 说明 |
| --- | --- | --- | --- |
| 一 | `v=(0.00,0.00)` | `idle_loop=1.00` | 单路口：走**原样返回**快路径，与扩展四十九之前逐位一致 |
| 二 | `v=(1.00,0.00)` | `run_loop=0.50, idle_loop=0.50` | 两路口：两份**增量**各半相加（不是把两条姿态平均成一个） |
| 三 | `v=(1.00,1.00)` | `run_loop=0.50, atk=0.50` | 落在三角形内部，重心 `idle=0`；零份额的 `idle` 整条消失 |
| 四 | `v=(0.00,2.00)` | `atk=1.00` | 顶点：塌回单路口，不残留别路份额 |

批量核对（744 行逐行，不抽样）：份额和 `max|Σw − 1| = 0.0`；无一行含 `nan`（旋转走 `slerp` 后 `normalize`，
除零会让这列直接爆掉）；每个阶段的份额**逐帧不动**（参数不动则份额不抖）；全程 `drop=0`
（三样本位置互不重复，不触发扩展四十九的重复拒收路径）。

**单路口零变化**：阶段一那 193 行与「没挂混合空间的加法层」走同一条快路径，`w=idle_loop=1.00`
且姿态不做任何算术 —— 这是本次改动的「零变化」侧；份额语义的差别只在两路以上出现，而那部分
由 `additive_sums_rotation_deltas_not_pose_average` 在单测里用具体角度钉住（真机日志只有份额，看不到姿态差）。

### 34.4 已知边界

- 只改**加法层多路份额**的含义，**覆盖层多路仍是姿态加权平均**（那是它的正确语义，未动）；
- 增量仍以**片段首帧**为参考（与 §33 同口径），不做「参考帧可配」；
- 各路增量按**线性**复合，不做惯性 / 弹簧 / 每路不同的衰减核；
- **不做逐路时间缩放**：每路走自己的 `ClipPlayback`（§32）或相位锁（§30 / §31），份额只决定幅度；
- **零变化**：单路口走原样返回的快路径，不调加法层多路混合的场景（含全部覆盖层用例）激励路径未变。

### 34.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/anim_blend.rs` | 6（内联） | 单路口逐位不变 / 旋转增量相加而非姿态平均 / 缩放比值连乘 / 部分权重重归一 / 零权重与缺轨道不进分母 / 相位锁版按各片段时长还原 |
| `crates/r3e-core/src/scene.rs` | 1 | `additive_layer_multiclip_uses_delta_semantics`：1D 空间 + 加法层这条真实链路，两路反向等份应回到基姿态而非半数幅度 |
| `crates/r3e-script` | 0 | 不加新脚本 API，既有的 `add_anim_layer_additive` / `blend_*` 套件全部照旧全绿 |

该节收尾为 **89 个套件 / 1,303 passed / 0 failed**（扩展四十九为 89 套件 1,296），
`--all-features` 同 89 套件 **1,307 passed**；`R3E_VISUAL_REGRESSION=1` 下**基线图未重建**。

## 35. 混合空间剖分缓存（扩展五十一）

§29 / §33 的混合空间在**每次** `add_anim_layer_blend_sample*` 之后都立刻整表重剖（2D 走三角剖分、
3D 走四面体剖分）。编辑器里拖样本是「一次改一个」的节奏：铺 12 个格子就付 12 次重建，而第 12 次的结果
已经把前 11 次全废掉了 —— 纯浪费。本次把「改样本」与「重剖」拆开。

### 35.1 口径

| 环节 | 旧行为 | 现在 |
| --- | --- | --- |
| `add_anim_layer_blend_sample*` | 立刻整表重剖 | 只置**脏标记**，剖分不动（挂起） |
| 求值 `weights_at` | 读缓存里的剖分 | 挂起时**现场算一份临时拓扑**，权重与提交后逐位相同 |
| 提交点 | — | ① 每帧 `advance` 自动提交；② 脚本显式 `build_anim_layer_blend` |
| 报数 `tri=` / `tet=` | 读缓存 | 挂起时现场数一遍，**不会出现「刚加完样本 `tri=0`」** |

要点：**挂起的是缓存，不是语义**。求值侧永远读不到陈旧拓扑，所以「延迟提交」不会变成「第一帧权重错」。

### 35.2 新 API

| API | 说明 |
| --- | --- |
| `anim_layer_blend_stale(inst, layer) -> bool` | 有未提交的样本改动返回 `true`。没挂空间也返回 `false`，探针里不用先判空间存不存在 |
| `build_anim_layer_blend(inst, layer) -> bool` | 立刻提交：`true` = 这次真的重建了，`false` = 本来就最新（编辑器装配完马上画线框时用，省得等一帧） |

回显 `anim_layer_blend` 的格式**一字未变**：`n=` 样本数、`tri=` / `tet=` 剖分数、`drop=` 被拒样本数、
`v=` 当前参数、`w=` 各路份额。

### 35.3 真机探针

```powershell
cargo test -p r3e-core blend_space                        # 37 条（本次 +7：deferred / build_pending / stale / clear）
cargo test -p r3e-script --test blend_space_api            # 7 条（本次 +2：脏标记翻转 + 手动建）
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\scene_anim.json scripts\blend_cache_demo.rhai --run-seconds 3.6
```

演示脚本 `scripts\blend_cache_demo.rhai`（114 行）拿 `assets\scene_anim.json` 的 `tirger`（7 片段）建**一层**
`cache`，铺四角 2D 空间 `(move.x, move.y)`：`idle_loop@(-1,-1)` / `run_loop@(1,-1)` / `atk@(-1,1)` / `atk2@(1,1)`，
四段墙钟走过去：

`rc 0`，885 行日志（帧数随负载浮动，两次实测 885 / 892），其中 **800 行 2D 探针**（段一 211 + 段二 238 + 段三 233 + 段四 118）：

```
[script] 装配完成：成功 4 个样本，stale=true
[script] 挂起中  2d(move.x,move.y) n=4 tri=2 drop=0 v=(0.00,0.00) w=idle_loop=0.50,atk2=0.50
[script] 显式提交：第一次=true 第二次=false stale=false
[script] 提交后  2d(move.x,move.y) n=4 tri=2 drop=0 v=(0.00,0.00) w=idle_loop=0.50,atk2=0.50
[script] 补样本后 stale=true
```

| 阶段 | 墙钟 | 参数 | `tri` | `stale` | 份额 |
| --- | --- | --- | --- | --- | --- |
| 一 装配 + 显式提交 | `et < 1.0` | `(0,0)` | `2` | 全 `false`（已提交） | `idle_loop=0.50, atk2=0.50` |
| 二 补第 5 个样本 | `1.0 ~ 2.0` | `(0.5,-0.5)` | `3` | **仅 1 行 `true`**，其余 `false` | `atk2=0.25, run_loop=0.75` |
| 三 右中 | `2.0 ~ 3.0` | `(1,0)` | `3` | 全 `false` | `atk2=0.50, run_loop=0.50` |
| 四 左上角 | `>= 3.0` | `(-1,1)` | `3` | 全 `false` | `atk=1.00` |

全 800 行逐行核对（不抽样）：

- **挂起期与提交后逐字符相同**：`v=(0,0)` 处「挂起中」与「提交后」两行除 `stale=` 外一模一样
  （`n=4 tri=2 drop=0 w=idle_loop=0.50,atk2=0.50`）；
- **`tri` 现场报数**：挂起期 `tri=2` 由临时拓扑当场算出（`self.triangles` 此时是空的），
  编辑器不会看到「刚加完样本 `tri=0`」这种中间态；
- **提交只付一次钱**：显式提交第一次 `true`、第二次 `false`；补样本后 `stale=true` **只出现 1 帧**
  （`et≈1.001`），那一帧已经 `n=5 tri=3` 且份额正确 —— 下一帧 advance 自动提交，`tri` 从 `2` 长到 `3` 不再回落；
- **份额和恒为 1**：`max |Σw − 1| = 0.0`，800/800 行通过；
- **无 NaN**、**`drop=0` 全程**（五个样本位置互不重复）。

### 35.4 已知边界

- 缓存的是**剖分**（拓扑），不是权重：`weights_at` 每次现算，没做权重缓存；
- **不做 Bowyer–Watson 增量插入 / 删除**：提交时仍是整表重剖，省的是「样本改一次剖一次」，
  而不是「重剖本身的代价」；
- 脏标记是**粗粒度**的：任何样本改动都让整张表重剖，没有「只补一个三角形」；
- 挂起期每帧求值要现算一份临时拓扑（`live_topology` 的 `Cow::Borrowed` / `Cow::Owned` 两条路），
  样本表很大且**长时间**挂起时这份重复计算会一直在；显式 `build_anim_layer_blend` 就是为了让它尽早结束；
- **零变化**：不碰样本表的场景剖分永远处于已提交态，`live_topology` 走 `Cow::Borrowed`，一次重剖都不发生。

### 35.5 测试覆盖

| 位置 | 条数 | 覆盖 |
| --- | --- | --- |
| `crates/r3e-core/src/blend_space.rs` | 7（内联，37 ← 30） | `add_sample_defers_triangulation` / `deferred_weights_bitwise_equal_to_committed_2d` / `deferred_weights_bitwise_equal_to_committed_3d` / `build_pending_commits_once` / `stale_space_still_refuses_extrapolation` / `clear_samples_drops_stale_mark` / `brief_reports_committed_topology_while_stale` |
| `crates/r3e-core/src/scene.rs` | 2 | `anim_layer_blend_stale` / `build_anim_layer_blend_pending` 两个门面（含「没挂空间返回 false、状态不动」） |
| `crates/r3e-script/tests/blend_space_api.rs` | 2（新套件内，7 ← 5） | `script_blend_stale_flag_and_manual_build` / `script_blend_stale_keeps_weights_and_pose_unchanged` |

该节收尾为 **89 个套件 / 1,312 passed / 0 failed**（扩展五十为 89 套件 1,303），
`--all-features` 同 89 套件 **1,316 passed**；`R3E_VISUAL_REGRESSION=1` 下**基线图未重建**。

---

*（内容由AI生成，仅供参考）*
