# 范式 2：验证并读懂失败

> **验证目标**：失败必须「可读、可定位、可放弃」——issue 带 `code` / `path` / `location` / `hint`，坐标指到厘米级；不提交就回滚，场景不留痕。
> 全部命令在 **Windows cmd** 下可整段粘贴：无变量赋值、无 `jq`、无管道取字段。

## 0. 前置

与「范式 1」同一套前置（仓库根 `D:\rust3d-engine`、场景 `docs\norms\scene_norms.json`、静默脚本 `docs\norms\silent.rhai`、端口 8787、令牌 `dev`）。差别只有一处：本范式故意写一个**穿模**的物件，让物理层给精确结论。

默认承接范式 1 的引擎实例也能跑，但**节点名会撞**（`spawn_prefab` 只新建不覆盖，重名回 `-32012`）。要复现下面每一行输出，请先重起引擎：

```bat
cd /d D:\rust3d-engine
taskkill /f /im r3e.exe
timeout /t 1 >nul
target\x86_64-pc-windows-gnu\debug\r3e.exe --scene docs\norms\scene_norms.json --script docs\norms\silent.rhai --rpc-port 8787 --rpc-token dev --physics on --run-seconds 900
```

## 1. 开事务，塞一个会穿模的物件

```bat
cd /d D:\rust3d-engine
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx begin --json
```

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx intent scene.spawn_prefab --args "{\"nodes\":[{\"name\":\"floor\",\"mesh\":\"cube\",\"position\":[0.0,0.0,0.0],\"scale\":[8.0,1.0,8.0],\"physics\":{\"body\":\"static\"}},{\"name\":\"goal\",\"mesh\":\"cube\",\"position\":[2.0,0.6,2.0],\"scale\":[0.5,0.5,0.5],\"physics\":{\"body\":\"static\"},\"tags\":[\"goal\"]}]}" --json
```

期望（实测，`issues` 三条全给）：

```json
{"commands":2,"impact":{"lights":[],"materials":[],"objects":["floor","goal"],"terrain":[],"violations":{"count":3,"issues":[{"approximate":false,"category":"overlap","code":"overlap.penetration","hint":"挪开其中一件，或把其中一件改成装饰物（去掉 physics 段）","layer":"physics","location":[2.0,0.42500001192092896,2.0],"msg":"`floor` 与 `goal` 的碰撞体实际相交（重叠中心 2.000, 0.425, 2.000）：不是包围盒误报，物理层按装配形状解析判定","path":"nodes[0]","severity":"error","target":"floor|goal"},{"approximate":true,"category":"support","code":"support.suspect","layer":"support","location":[0.0,-0.5,0.0],"severity":"warn","path":"nodes[1]","target":"floor"},{"approximate":true,"category":"support","code":"support.suspect","layer":"support","location":[2.0,0.3500000238418579,2.0],"severity":"warn","path":"nodes[2]","target":"goal"}],"layers":["static","physics"],"notChecked":["nav"],"unchecked":[{"layer":"nav","reason":"主场景还没烘焙导航（先 nav.bake）：没有可比对的基线，影子的可走性 / 可达性未查"}]}},"intent":"scene.spawn_prefab","intents":1,"staged":2,"txId":"tx-1"}
```

几何账（对得上才算真读懂）：`floor` 是中心 y=0、高 1 的立方体，占 y∈[-0.5, 0.5]；`goal` 中心 y=0.6、高 0.5，占 y∈[0.35, 0.85]。两者在 y∈[0.35, 0.5] 段相交 → 重叠中心落在 y=0.425，正是 `location` 给的 `[2.0, 0.425, 2.0]`。

## 2. 逐字段读 issue

| 字段 | 本例值 | 怎么用 |
|---|---|---|
| `code` | `overlap.penetration` | 稳定标识，脚本按它分支；`*.suspect` 是近似推断，`penetration` 是精确结论 |
| `severity` | `error` | `error` 会拖垮 `verify` 的退出码；`warn` 不拦 |
| `approximate` | `false` | `false`=物理层按装配形状算的，与包围盒无关，不必再复核 |
| `layer` | `physics` | 哪一层判的（`static` / `physics` / `nav` / `overlap` / `support`） |
| `target` | `floor\|goal` | 冲突的两个物件（`support` 类只有单边） |
| `path` | `nodes[0]` | 回到场景 JSON 的路径：`nodes[0]` 就是 `floor`，改参数时按它定位 |
| `location` | `[2.0, 0.425, 2.0]` | 冲突中心的世界坐标，可直接拿去写新的 `position` |
| `hint` | 「挪开其中一件，或…去掉 physics 段」 | 处置建议；`hint` 是给人看的，别当判据 |
| `msg` | 「…不是包围盒误报…」 | 一句人话，含「为什么判成这样」 |

`impact.unchecked[]` 是本范式里最容易被忽略的一条：导航层这次**没查**，原因是主场景还没有导航基线。`unchecked` 会让 `verify` 的 `passed` 变 `false`——但这不是场景的错，是没装配（见范式 1 步骤 6）。

## 3. 不提交，直接回滚

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev tx rollback --json
```

期望（实测）：

```json
{"dropped":2,"hash":"0x27662528921dbd9b","txId":"tx-1","version":9979}
```

- `dropped`=2：两条暂存命令全丢；
- `hash` 回到 `tx begin` 时的 `baseHash`（本例 `0x27662528921dbd9b`）：**主场景一个字节没变**。

## 4. 证明「没落盘」

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token dev query stats --json
```

期望（实测）：

```json
{"byKind":{},"interactable":0,"nodes":0,"tagged":0,"tags":{},"visible":0}
```

`nodes`=0：`floor` 与 `goal` 都没进场景。**回滚的判据不是「回滚命令成功」，而是「场景普查归零」**——这一步别省。

## 5. 如果已经提交了

`tx rollback` 只管暂存区。已经 `tx commit` 的改动要用新事务改回去（范式 3 就是这条路）。别指望 `rollback` 撤销已提交的版本。

## 6. 常见偏差

| 现象 | 原因 | 处置 |
|---|---|---|
| `tx rollback` 回「没有可用的 txId」 | 上一笔已 `commit` 或已 `rollback`，会话态里没有未决事务 | 先 `tx begin`；已经是提交态的走新事务 |
| `violations.count` 为 0 | 物件没落进彼此的形状里（只是包围盒贴着） | 精确判定看 `approximate`：`false` 且 `count=0` 才是真干净 |
| `verify` 回 `-32601` / `-32012` | 方法名或参数写错（`rpc` 只认协议方法名，`tx apply` 只认可写方法名） | 回 `r3e-cli --help` 对方法名 |
| 同一份 `--args` 在 PowerShell 里报 JSON 非法 | PS 5.1 传原生程序时会吃掉内层双引号 | 用 cmd 粘贴；PS 需自行加 `--%` 或改转义 |
