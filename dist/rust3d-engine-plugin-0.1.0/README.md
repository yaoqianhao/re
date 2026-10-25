# rust3d-engine 插件：打包、安装与挂载

本插件把 rust3d-engine 的**纯逻辑层**（场景解析、网格统计、AIR 校验、物理步进）挂给
ai_ide。形态是「原生 exe 子进程 + 一行一个 JSON 帧（NDJSON）走 stdio」，不需要图形栈，
也不需要 Node 运行时。

> 协议事实来源：ai_ide `crates/plugin/src/{manifest.rs,host.rs,loader.rs,install.rs}`。
> 清单**不手写**——由二进制 `--emit-manifest` 现算，与握手回包同源，避免两处漂移导致
> 6106「握手工具集与清单不一致」。

## 一、产物布局

```
rust3d-engine-plugin-0.1.0/
  plugin.json            # 清单（--emit-manifest 生成）
  bin/r3e-plugin.exe     # 入口（plugin.json 的 main 指向这里）
  examples/*.json        # 可直接喂给 scene_inspect / physics_simulate 的示例场景
  README.md              # 本文件
```

`dist/` 下同时产出 `rust3d-engine-plugin-0.1.0.zip` 与 `SHA256SUMS.txt`（含 zip 与
exe 两个 SHA256）。

## 二、构建与打包

```powershell
# 只编译（开发机调试用）
cargo build --release -p r3e-plugin

# 打完整插件包：构建 → 生成 plugin.json → 跑自检 → zip → SHA256SUMS
pwsh scripts/package-plugin.ps1

# 把 Rapier 物理后端一起编进去（体积变大，physics_simulate 可用 backend=rapier）
pwsh scripts/package-plugin.ps1 -Features rapier
```

打包脚本内置三道闸门，任一处失败都不会产出 zip：

1. 清单里的 `main` 必须真实存在于包内（否则宿主装载报 6108）；
2. 对**包内那份 exe** 跑一次 `--smoke-test`：拉起子进程 → `plugin/handshake` →
   逐个工具 `plugin/tool-invoke` → `plugin/command-invoke` → `plugin/shutdown`，
   并断言「握手工具集 == 清单工具集」；
3. SHA256 写入 `SHA256SUMS.txt`，供分发时校验。

校验（收到 zip 后先做这一步）：

```powershell
Get-FileHash .\rust3d-engine-plugin-0.1.0.zip -Algorithm SHA256
```

## 三、清单字段

| 字段 | 值 | 说明 |
|---|---|---|
| `id` | `rust3d-engine` | 目录名与账本键，`^[a-z0-9][a-z0-9-]{1,63}$` |
| `name` / `version` | `rust3d-engine` / `0.1.0` | 版本取自 `workspace.package.version` |
| `pluginApi` | `"1.0"` | 宿主 `HOST_PLUGIN_API_MAJOR=1`，主版本不符直接拒装 |
| `main` | `bin/r3e-plugin.exe` | 相对包根；非 `.js/.mjs/.cjs` 即直接 spawn，不经 Node |
| `capabilities` | `["fs.read"]` | 只读。`shell.exec` 是宿主不提供的能力，写了会被拒装 |
| `tools` | 5 个，全部 `risk=read` | `engine_info` / `scene_inspect` / `mesh_stats` / `scene_validate` / `physics_simulate` |
| `commands` | 1 个 | `engine_diagnostics`（命令面板入口，不进模型工具表） |
| `subscriptions` | `[]` | 不订阅宿主事件，减少无谓唤醒 |
| `timeoutMs` | `60000` | 宿主允许 1000..=120000；物理步进留足余量 |

## 四、放置路径

宿主数据目录 = `%APPDATA%\ai-ide`，插件装到：

```
%APPDATA%\ai-ide\plugins\rust3d-engine\
```

安装是**目录包**：宿主只接受已解压的目录（zip 直接安装会被 6107 拒掉，见
`install.rs`「P1 仅支持目录形式的插件包」）。手工摆放与走 `plugin.install` 等价，
但推荐用 RPC——它会写账本、做 staging 原子替换，并在复制后复查入口文件。

## 五、install → grant → load

三个 RPC 走 ai_ide 的本地 socket/stdio 通道（设置面板的插件页封装的就是这三个）：

```jsonc
// 1) 安装：path 指向解压后的目录（不是 zip）
{"method":"plugin.install","params":{"path":"D:\\rust3d-engine\\dist\\rust3d-engine-plugin-0.1.0"}}

// 2) 授权：装完状态是 needs_grant（清单合法、只差用户点授权，进程尚未启动）
{"method":"plugin.grant","params":{"plugin_id":"rust3d-engine","capabilities":["fs.read"]}}

// 3) 加载：spawn 子进程 → 握手校验 → 工具入表 → 命令入面板
{"method":"plugin.load","params":{"plugin_id":"rust3d-engine"}}
```

状态机：`disabled`（装好未加载）→ `needs_grant`（缺授权）→ `loaded`（工具已可调用）。
排查用 `plugin.list` / `plugin.status` 看 `granted_capabilities` 与最近错误码。

常见错误码：

| 码 | 含义 | 处置 |
|---|---|---|
| 6106 | 握手工具集与清单不一致 | 用本仓库 `package-plugin.ps1` 重打包，别手改 plugin.json |
| 6107 | 包不合法（缺清单 / zip / 入口越界） | 先解压成目录，确认 `plugin.json` 与 `bin/r3e-plugin.exe` 都在 |
| 6108 | `main` 指向的入口不存在 | 见上，同样不要手改清单 |
| 6103 | 能力未授权 | `plugin.grant` 补 `fs.read` |

## 六、挂载后的调用

模型工具（`plugin.call` 网关自动路由）：

| 工具 | 用途 | 关键入参 |
|---|---|---|
| `engine_info` | 连通性自检：版本、crate 组成、物理后端可用性 | 无 |
| `scene_inspect` | 读场景：节点清单、网格引用、世界坐标、物理开关 | `scene` 或 `path` |
| `mesh_stats` | 内置图元网格统计（顶点/三角/AABB/指纹/AIR） | `shape`、分段参数 |
| `scene_validate` | 逐物件 AIR 校验 + 重名 + 外部资产统计 | `scene` 或 `path` |
| `physics_simulate` | headless 物理步进：位移、事件、后端能力 | `backend`、`steps`、`dt` |

命令面板：`engine_diagnostics`（`plugin.command`），输出引擎信息 + 工作区读取能力是否可用。

示例：

```jsonc
{"method":"plugin.call","params":{
  "plugin_id":"rust3d-engine","method":"tool-invoke",
  "params":{"tool":"physics_simulate","arguments":{"path":"examples/scene_physics.json"}}}}
```

`path` 走 `host/fs.read`，必须是**工作区相对路径**，单文件上限 64 KB；超过就改传 `scene`
对象。每次工具调用前插件会把宿主配额归零（配额按「一次调用」计）。

## 七、两条硬纪律

1. **stdout 只出协议帧**。`serve` 模式下任何一行日志混进 stdout，宿主立刻判协议违规并
   按崩溃处理（重启最多 3 次）。日志一律 `eprintln!`。`--emit-manifest` / `--help`
   是打包期的人机出口，不在 serve 路径上。
2. **后端不可用要报错，不许 panic、不许悄悄降级**。未编 `rapier` 时：
   - 显式 `backend="rapier"` → `-32602`，信息里列出现有后端并提示 `--features rapier`；
   - 场景文件自己写了 `backend="rapier"` → `-32010`（数据问题，与「参数写错」区分开）。
   宿主/模型因此能拿到可读错误，而不是一次崩溃重启。
