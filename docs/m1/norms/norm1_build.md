# 范式 1：从零生成可通行小场景

> **验证目标**：一次意图 = N 条命令 = 一格撤销；提交前就能读到坐标化 issue。
> 全部命令在 **Windows cmd** 下可整段粘贴：无变量赋值、无 `jq`、无管道取字段。
> 本文所有「期望输出」都是实跑粘贴，未跑过的不写。

## 0. 前置

| 项 | 值 |
|---|---|
| 仓库根 | `D:\rust3d-engine` |
| 引擎 | `target\x86_64-pc-windows-gnu\debug\r3e.exe` |
| CLI | `target\x86_64-pc-windows-gnu\debug\r3e-cli.exe` |
| 场景 | `docs/m1/norms/scene_norms.json`（`objects` 为空，`spawns` 自带 `start=(0.35,0,0.35)`） |
| 脚本 | `docs/m1/norms/silent.rhai`（静默：不逐帧写 `scene.time` / `scene.frame`，`stateHash` 不漂移） |
| RPC | `127.0.0.1:8787`，令牌 `dev` |

三条硬约束（踩过才写）：

1. **二进制名是 `r3e.exe`**，不是 crate 名 `r3e-app`；物理开关是 `--physics on`，不是 `--enable-physics`。
2. **必须给静默脚本**。非静默脚本每帧写 `scene.time` / `scene.frame`，`baseHash` 一提交就过期，`tx commit` 直接回 `TX_CONFLICT`（-32013）。
3. **出生点只能写在场景文件的 `spawns` 里**。宿主没有写 `spawns` 的协议方法（命令表 388 条里没有），不给出生点则导航层记 `unchecked`，给错位置则 `nav.spawn_offmesh`。

## 1. 起引擎（终端 A，保持不关）

```bat
cd /d D:\rust3d-engine
target\x86_64-pc-windows-gnu\debug\r3e.exe --scene docs/m1/norms/scene_norms.json --script docs/m1/norms/silent.rhai --rpc-port 8787 --rpc-token dev --physics on --run-seconds 900
```

- 该终端被引擎占住，出现渲染窗口即就绪；`--run-seconds 900` 让引擎 15 分钟后自退。
- `--physics on` 决定物理层能不能给**精确**结论（关掉时 `physics.bind` 回 `PHYSICS_DISABLED`，体检的物理层恒 `unchecked`）。
- 端口不能写 `0`：`--rpc-port 0` 的真实端口只落在日志里，脚本拿不到。
- 后续命令一律在**终端 B** 执行。

## 2. 开事务

```bat
cd /d D:\rust3d-engine
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx begin --json
```

期望（实测）：

```json
{"baseHash":"0x919550369c90873c","staged":0,"txId":"tx-1","version":11076}
```

- `txId` 已写进缺省会话态 `.r3e-cli/session.json`，后面的命令不用再传；`--state-file ""` 可关掉它，改用 `--tx <id>` 显式传。
- `baseHash` 每次启动都不同（含运行时状态），`version` 随时间单调增长，两者都以实际输出为准——判读看的是**是否与前一格一致**，不是具体数值。
- 退出码 0 = 成功；引擎没起时这一步回 **3**（环境错）并列出三条自查项，没有未决事务时后续 `tx` 命令回 **2**（用法错）。

## 3. 一条意图生成三件物件

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx intent scene.spawn_prefab --args "{\"nodes\":[{\"name\":\"floor\",\"mesh\":\"plane\",\"position\":[0.0,0.0,0.0],\"scale\":[16.0,1.0,16.0]},{\"name\":\"step\",\"mesh\":\"cube\",\"position\":[2.0,0.5,2.0],\"scale\":[1.0,1.0,1.0],\"physics\":{\"body\":\"static\"}},{\"name\":\"goal\",\"mesh\":\"cube\",\"position\":[-3.0,0.5,3.0],\"scale\":[0.5,1.0,0.5],\"physics\":{\"body\":\"static\"},\"tags\":[\"goal\"]}]}" --json
```

期望（实测，`impact` 已节选）：

```json
{"commands":3,"impact":{"lights":[],"materials":[],"objects":["floor","step","goal"],"terrain":[],"violations":{"count":3,"issues":[{"approximate":true,"category":"overlap","code":"overlap.suspect","layer":"overlap","location":[-3.0,0.25,3.0],"severity":"warn","target":"floor|goal"},{"approximate":true,"code":"overlap.suspect","location":[2.0,0.25,2.0],"severity":"warn","target":"floor|step"},{"approximate":true,"code":"support.suspect","layer":"support","location":[0.0,-0.5,0.0],"severity":"warn","target":"floor"}],"layers":["static","physics"],"notChecked":["nav"],"unchecked":[{"layer":"nav","reason":"主场景还没烘焙导航（先 nav.bake）"}]},"intent":"scene.spawn_prefab","intents":1,"staged":3,"txId":"tx-1"}
```

读法：

| 字段 | 含义 |
|---|---|
| `commands` / `staged` | 这一条意图展开成 3 条底层命令，全部进了暂存区 |
| `violations.issues[].severity` | `warn` 不拦提交，`error` 也不拦提交——**拦不拦由你判**，但 `verify` 的退出码会被 `error` 拖成 1 |
| `violations.issues[].approximate` | `true`=包围盒近似（下面三条都是）；`false`=物理层按装配形状算出来的精确结论 |
| `violations.unchecked[]` | 这一层**没查**以及为什么：此处导航还没烘焙，影子场景的可走性无从比对 |

三条 `warn` 的来历（都属「平面没有厚度」这种正常几何，留着不拦）：

- `floor`（plane）与 `step` / `goal` 的包围盒交叠：plane 的包围盒按 16×1×16 的盒子算，物件站在它上面必然交叠；
- `floor` 底面 y=-0.5 向下 0.05 内没有支撑面：它就是地面本体，悬空是几何事实而非错误。

## 4. 影子预演

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx preview --json
```

期望（实测）：

```json
{"base":"0x919550369c90873c","current":"0x919550369c90873c","shadow":"0xd9643937750cc4c8","staged":3,"txId":"tx-1"}
```

- `base` = `current`：提交前主场景**一动没动**；
- `shadow` ≠ `base`：影子场景已经变了。三者一致才是「预演没产生影响」的对证。

## 5. 提交

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx commit --json
```

期望（实测）：

```json
{"committed":3,"hash":"0xd9643937750cc4c8","txId":"tx-1","version":11185}
```

- `committed`=3 条命令**一次**进撤销栈：之后一次 `undo` 全退，不是三次。
- `hash` 与上一步的 `shadow` 相同，说明「预演过的就是提交的」。

## 6. 装配物理 + 烘焙导航

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev rpc physics.bind --json
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev rpc nav.bake --json
```

期望（实测）：

```json
{"backend":"builtin","bindings":[{"handle":1,"node":"step"},{"handle":2,"node":"goal"}],"bodyCount":2,"bound":2,"candidates":2,"capabilities":["raycast","overlap","shape_cast"],"created":2,"enabled":true,"reused":0,"simulated":true,"skipped":0,"stateHash":"0xd9643937750cc4c8","summary":"装配 2 个节点：新建 2 / 复用 0 / 跳过 0","wroteBackScene":false}
```

```json
{"baked":true,"cellSize":0.5,"cells":198,"dims":[18,11],"floatingSpawns":[],"regionSizes":[164],"regions":1,"spawns":[{"name":"start","region":1}],"unresolved":[],"walkable":164,"walkableRatio":0.8282828330993652}
```

- `candidates`=2 而不是 3：`floor` 是 plane、没有碰撞体，不进物理世界。
- `wroteBackScene:false`：装配只动作物理世界，不写回场景、不推版本。
- `spawns[].region=1` 且 `floatingSpawns` 为空：出生点落在可走格里，这是导航层能判「可达」的前提。

**这两步不能省**。`verify` 的 `passed` 定义含「`unchecked` 为空」，不调 `physics.bind` / `nav.bake`，物理层与导航层恒为 `unchecked`，退出码恒为 1——那不是场景有问题，是没装配。

## 7. 体检

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev verify --json
echo EXIT=%ERRORLEVEL%
```

期望（实测，退出码 **0**，`issues` 节选一条）：

```json
{"counts":{"byLayer":{"nav":0,"physics":0,"static":3},"error":0,"warn":3},"issues":[{"code":"overlap.suspect","layer":"overlap","location":[-3.0,0.25,3.0],"severity":"warn","target":"floor|goal"}],"layers":["static","physics","nav"],"ok":true,"passed":true,"stateHash":"0xd9643937750cc4c8","truncated":false,"unchecked":[]}
EXIT=0
```

判读三个字段就够：

| 字段 | 本范式值 | 含义 |
|---|---|---|
| `passed` | `true` | **退出码看它**：0 通过 / 1 未通过 |
| `unchecked` | `[]` | 三层都查过了；非空时 `passed` 必为 `false` |
| `counts.error` | `0` | 三条 `warn` 不拦通过 |

`ok` 与 `passed` 不同名不同义：`ok` 只说「这一趟跑完了」，`passed` 还要求「没有没查的层」。脚本分支一律判 `passed`。

## 8. 收尾

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev query stats --json
```

期望（实测）：

```json
{"byKind":{"any":3,"physical":2,"prop":1},"interactable":0,"nodes":3,"tagged":1,"tags":{"goal":1},"visible":3}
```

合法场景已落盘：3 个节点、其中 2 个有碰撞体（`physical`）、1 个是 `prop` 类、1 个带 `goal` 标签。引擎侧按 `Ctrl+C` 或等 `--run-seconds` 到期退出。

以上八条命令在一次干净引擎实例上连跑，总耗时 **0.3 秒**（实测），退出码依次 `0 0 0 0 0 0 0 0`。

## 9. 常见偏差

| 现象 | 原因 | 处置 |
|---|---|---|
| `tx begin` 退出码 3，`连不上引擎` | 引擎没起 / 没给 `--rpc-port` / 端口不一致 | 回步骤 1；`--addr` 与 `--rpc-port` 必须指向同一地址 |
| `tx commit` 回 `-32013 TX_CONFLICT` | 用了会逐帧写 `scene.time` 的脚本，`baseHash` 已过期 | 换 `docs/m1/norms/silent.rhai` |
| `verify` 恒为 1 且 `unchecked` 里有 `physics` 或 `nav` | 漏了步骤 6 | 先 `rpc physics.bind` / `rpc nav.bake` 再体检 |
| `tx intent` 回 `-32012`「物件 `xxx` 已存在」 | 上一次的场景还在同一个引擎进程里；`spawn_prefab` 只新建不覆盖 | 重启引擎，或换节点名 |
| `verify` 回 `-32020 PHYSICS_DISABLED` | 引擎用 `--physics off` 起的 | 用 `--physics on` 重起 |
