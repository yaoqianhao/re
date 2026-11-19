# 范式 3：失败回滚重试到成功（Gate 1 主剧本）

> **验证目标**：AI **不改引擎代码、只靠协议闭环**完成「生成 → 失败 → 读懂 → 修正 → 成功」。
> 全部命令在 **Windows cmd** 下可整段粘贴：无变量赋值、无 `jq`、无管道取字段。

## 0. 前置

仓库根 `D:\rust3d-engine`、场景 `docs\norms\scene_norms.json`、静默脚本 `docs\norms\silent.rhai`、端口 8787、令牌 `dev`。必须**从干净引擎**开始，否则节点名会撞（`spawn_prefab` 只新建不覆盖）。

```bat
cd /d D:\rust3d-engine
taskkill /f /im r3e.exe
timeout /t 1 >nul
target\x86_64-pc-windows-gnu\debug\r3e.exe --scene docs\norms\scene_norms.json --script docs\norms\silent.rhai --rpc-port 8787 --rpc-token dev --physics on --run-seconds 900
```

## 阶段一：提交一个「差 5 厘米」的场景

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx begin --json
```

期望（实测）：`{"baseHash":"0x27662528921dbd9b","staged":0,"txId":"tx-1","version":12543}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx intent scene.spawn_prefab --args "{\"nodes\":[{\"name\":\"floor\",\"mesh\":\"plane\",\"position\":[0.0,0.0,0.0],\"scale\":[16.0,1.0,16.0]},{\"name\":\"step\",\"mesh\":\"cube\",\"position\":[2.0,0.5,2.0],\"scale\":[1.0,1.0,1.0],\"physics\":{\"body\":\"static\"}},{\"name\":\"goal\",\"mesh\":\"cube\",\"position\":[2.0,1.2,2.0],\"scale\":[0.5,0.5,0.5],\"physics\":{\"body\":\"static\"},\"tags\":[\"goal\"]}]}" --json
```

期望（实测，`issues` 取关键条）：

```json
{"commands":3,"impact":{"objects":["floor","step","goal"],"violations":{"count":3,"issues":[{"approximate":false,"category":"overlap","code":"overlap.penetration","hint":"挪开其中一件，或把其中一件改成装饰物（去掉 physics 段）","layer":"physics","location":[2.0,0.975000023841858,2.0],"msg":"`step` 与 `goal` 的碰撞体实际相交（重叠中心 2.000, 0.975, 2.000）：不是包围盒误报，物理层按装配形状解析判定","path":"nodes[1]","severity":"error","target":"step|goal"}],"layers":["static","physics"],"notChecked":["nav"],"unchecked":[{"layer":"nav","reason":"主场景还没烘焙导航（先 nav.bake）：没有可比对的基线"}]}},"intent":"scene.spawn_prefab","intents":1,"staged":3,"txId":"tx-1"}
```

几何账：`step` 占 y∈[0, 1]；`goal` 中心 y=1.2、高 0.5，占 y∈[0.95, 1.45]。差 5 厘米没放上去 → 重叠中心 y=0.975，正是 `location` 给的坐标。

**注意：`impact` 里带 `error` 也不拦提交**。宿主不替人做决定，AI 想提交就提交——拦它的是下一步的 `verify` 退出码。

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx commit --json
```

期望（实测）：`{"committed":3,"hash":"0xe61628cd30cccd7f","txId":"tx-1","version":12620}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev rpc physics.bind --json
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev rpc nav.bake --json
```

期望（实测，节选）：

```json
{"backend":"builtin","bindings":[{"handle":1,"node":"step"},{"handle":2,"node":"goal"}],"bound":2,"created":2,"capabilities":["raycast","overlap","shape_cast"],"enabled":true,"reused":0,"simulated":true,"skipped":0,"stateHash":"0xe61628cd30cccd7f","wroteBackScene":false}
```

```json
{"baked":true,"cellSize":0.5,"cells":100,"dims":[10,10],"floatingSpawns":[],"regionSizes":[75],"regions":1,"spawns":[{"name":"start","region":1}],"unresolved":[],"walkable":75,"walkableRatio":0.75}
```

## 阶段二：体检拿到失败（退出码 1）

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev verify --json
echo EXIT=%ERRORLEVEL%
```

期望（实测，退出码 **1**）：

```json
{"counts":{"byLayer":{"nav":0,"physics":1,"static":2},"error":1,"warn":2},"issues":[{"approximate":false,"code":"overlap.penetration","layer":"physics","location":[2.0,0.975000023841858,2.0],"path":"nodes[1]","severity":"error","target":"step|goal"}],"layers":["static","physics","nav"],"ok":false,"passed":false,"stateHash":"0xe61628cd30cccd7f","truncated":false,"unchecked":[]}
EXIT=1
```

三件事同时成立，才叫「失败的读法正确」：

1. `ok:false` 与 `passed:false` **同时**为假——不是没查（`unchecked:[]` 说明三层都查了），是真的有问题；
2. `counts.byLayer.physics=1` 指到物理层，`counts.error=1` 与 `issues` 条数对得上；
3. `path:"nodes[1]"` + `location:[2.0,0.975,2.0]` 给足了修正参数——`nodes[1]` 就是 `step`，冲突中心在 y=0.975，把 `goal` 抬到 1.25 就够。

这条链子对应 `AI_ROADMAP` 里 Gate 1 的判据「AI 能靠退出码与 issue 定位问题，不需要人来翻译」。

## 阶段三：修正并重试（退出码 0）

已提交的改动不能用 `tx rollback` 撤（它只管暂存区），改法是一笔**新的修正事务**：

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx begin --json
```

期望（实测）：`{"baseHash":"0xe61628cd30cccd7f","staged":0,"txId":"tx-2","version":12772}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx apply scene.settransform --args "{\"node\":\"goal\",\"position\":[2.0,1.25,2.0]}" --json
```

期望（实测）：`{"op":"scene.settransform","staged":1,"txId":"tx-2"}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx preview --json
```

期望（实测）：`{"base":"0xe61628cd30cccd7f","current":"0xe61628cd30cccd7f","shadow":"0x43c5c864ff156aed","staged":1,"txId":"tx-2"}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx commit --json
```

期望（实测）：`{"committed":1,"hash":"0x43c5c864ff156aed","txId":"tx-2","version":12889}`

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev rpc physics.bind --json
```

期望（实测，注意 `reused` 涨、`created` 归 0）：

```json
{"backend":"builtin","bindings":[{"handle":1,"node":"step"},{"handle":2,"node":"goal"}],"bodyCount":2,"bound":2,"candidates":2,"capabilities":["raycast","overlap","shape_cast"],"created":0,"enabled":true,"reused":2,"simulated":true,"skipped":0,"stateHash":"0x43c5c864ff156aed","summary":"装配 2 个节点：新建 0 / 复用 2 / 跳过 0","wroteBackScene":false}
```

移动了物件必须重跑 `physics.bind`：物理世界里的形状不会自己跟着场景走，`reused` 涨说明这次是把已有刚体的位置按场景对齐，而不是新建一堆。

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev verify --json
echo EXIT=%ERRORLEVEL%
```

期望（实测，退出码 **0**）：

```json
{"counts":{"byLayer":{"nav":0,"physics":0,"static":2},"error":0,"warn":2},"issues":[{"approximate":true,"code":"overlap.suspect","layer":"overlap","location":[2.0,0.25,2.0],"severity":"warn","target":"floor|step"},{"approximate":true,"code":"support.suspect","layer":"support","location":[0.0,-0.5,0.0],"severity":"warn","target":"floor"}],"layers":["static","physics","nav"],"ok":true,"passed":true,"stateHash":"0x43c5c864ff156aed","truncated":false,"unchecked":[]}
EXIT=0
```

`error` 从 1 降到 0、`warn` 仍是 2 条（plane 没厚度、平面本体悬空），`passed:true` → 退出码 0。**修正成功的判据是退出码翻转，不是 issue 归零**：`warn` 留着才是正常的。

## 全链时间线（Gate 1 记录口径）

| 阶段 | 命令条数 | 退出码 | 状态哈希 |
|---|---|---|---|
| 生成（begin→intent→commit） | 3 | 0 | `0x2766…` → `0xe61628cd30cccd7f` |
| 装配 + 体检 | 3 | 0 / 1 | `0xe61628cd30cccd7f` |
| 修正（begin→apply→preview→commit） | 4 | 0 | → `0x43c5c864ff156aed` |
| 重装配 + 复检 | 2 | 0 / **0** | `0x43c5c864ff156aed` |

合计 12 条命令、两次 `verify`、退出码 `1 → 0` 翻转，全程只调协议方法，没动引擎代码。

## 常见偏差

| 现象 | 原因 | 处置 |
|---|---|---|
| 修正后 `verify` 仍是 1 | 忘了重跑 `rpc physics.bind`：物理世界里还是旧位置 | 移动类改动后固定跟上 `physics.bind`（`reused` 涨才对） |
| `tx begin` 报 `TX_CONFLICT` | 引擎里跑了非静默脚本，`baseHash` 漂移 | 换 `silent.rhai` 重起 |
| `tx apply scene.settransform` 回 `-32012` | `node` 名字写错（要写成场景里的 `name`，不是数字 id） | 先 `query describe <name>` 确认存在 |
| 用 `tx rollback` 想撤已提交的改动 | `rollback` 只清暂存区 | 开新事务改回去，或交给撤销栈（一次 `undo` 退一格意图） |
