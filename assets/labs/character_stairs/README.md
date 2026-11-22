---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_2d0fd6dce62f11f19db6525400638852
    ReservedCode1: HAmU/NBXRuVpZrhOHD45r4Jgu0T16vEP6uVb0rI6sxav6ZouEv5/Bur0OdQVlvS5HUkPEbAWSyXFVHPNjagEVJ9x/eYA44ziTHysFlcbDLmSIBC3kSUcqwFIzfIzXKmZtdDRL5FSwTMMld8yZvJpqrE3gqAi2qDsW4On/YTkInAU1tot/4mj+ynLhhs=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_2d0fd6dce62f11f19db6525400638852
    ReservedCode2: HAmU/NBXRuVpZrhOHD45r4Jgu0T16vEP6uVb0rI6sxav6ZouEv5/Bur0OdQVlvS5HUkPEbAWSyXFVHPNjagEVJ9x/eYA44ziTHysFlcbDLmSIBC3kSUcqwFIzfIzXKmZtdDRL5FSwTMMld8yZvJpqrE3gqAi2qDsW4On/YTkInAU1tot/4mj+ynLhhs=
---

# r3e 楼梯 / 斜坡 + 键盘圆柱实验场

一个自包含的可跑样例：平面上并排铺 **9 条地形赛道**（5 组楼梯 + 4 组斜坡），场上放一个**圆柱角色**，
用 **WASD 真实键盘**驱动它走上去 / 被挡住，用来回归"角色控制器 + 地形碰撞 + 渲染可见性"三件事。

- 场景：`stairs_lab.r3e.json`（55 个物件，全部地形为静态 box collider）
- 角色：`player`，`mesh: cylinder`，scale `0.8 / 1.8 / 0.8`（直径 0.8m、高 1.8m）
- 地面：`ground_plane` 静态盒承担碰撞（`terrain` 段仅渲染，物理射线打不到它）

## 一、目录结构

本样例自包含，全部内容集中在 `assets/labs/character_stairs/`，不污染 `scripts/` 与 `assets/` 根：

```
assets/labs/character_stairs/
├── README.md                    本文档
├── stairs_lab.r3e.json          场景（9 条赛道 + 平台 + 地面 + 圆柱玩家）
├── scripts/                     rhai 脚本（全部可直接喂 r3e.exe 第二个参数）
│   ├── keyboard_probe.rhai      真键盘探针版
│   ├── walk.rhai                手动交互版（含 C 全景视角）
│   ├── panorama.rhai            高位全景静帧
│   └── auto_*.rhai              4 个无人值守回归夹具
├── tools/                       真窗口驱动 / 采集脚本（依赖 PIL）
│   ├── drive_keyboard.py        合成真实按键 + 分步截图
│   └── shoot_panorama.py        全景静帧采集
└── evidence/                    本机实测截图（运行脚本会覆盖同名文件）
    ├── kbd_0*.png               键盘驱动四步截图
    ├── shot_auto_*.png          各夹具运行截图
    └── shot_panorama.png        全景图
```

两个 python 脚本已改为按 `__file__` 定位自身（`LAB = .../character_stairs`，`ROOT = 项目根`），
**换机器 / 换克隆路径不需要改任何常量**，只要从项目根执行即可；运行日志落在 `temp/`（已在 `.gitignore`）。

## 二、地形布局（x 轴 4m 一条，z 轴向前为 -Z）

| 车道 x | 名称 | 几何 | 级高 / 坡角 | 预期 |
|---|---|---|---|---|
| -16 | `stairs_h10` | 12 级台阶 + 顶端平台 | 0.10 m | 可上 |
| -12 | `stairs_h18` | 10 级台阶 + 平台 | 0.18 m | 可上 |
| -8 | `stairs_h25` | 8 级台阶 + 平台 | 0.25 m | 可上 |
| -4 | `stairs_h32` | 6 级台阶 + 平台 | 0.32 m | 可上（逼近限值） |
| 0 | `stairs_h50` | 4 级台阶 + 平台 | 0.50 m | 被挡（> 0.35 m 限值） |
| 4 | `ramp_d12` | 单块斜面 + 平台 | 12° | 可攀 |
| 8 | `ramp_d25` | 单块斜面 + 平台 | 25° | 可攀 |
| 12 | `ramp_d40` | 单块斜面 + 平台 | 40° | 可攀（逼近限值） |
| 16 | `ramp_d60` | 单块斜面 + 平台 | 60° | 被挡（> 45° 限值） |

角色出生点 `start` = (-16, 0.9, 6.0)，即 0.10m 楼梯车道起点前的平地。

## 三、角色运动学与限值口径

`keyboard_probe.rhai` / `walk.rhai` / `auto_*.rhai` 四个脚本共用同一套 `walk_step()` 判定，
不依赖 runtime 的 `character_*` 宿主接口（`CharacterPort` 尚未接线，走脚本层运动学可先闭环）：

- 移动：`W` 前 / `S` 后 / `A` 左 / `D` 右，速度 4.2 m/s，`Shift` 冲刺 8.0 m/s，斜向归一化。
- 贴地：每帧向脚下发一条 `physics_raycast`（向下 8m），命中点 y 即地面高度；
  **首帧物理未装配时射线全落空（返回 -999），此时只记录不写位**，避免把角色按进地下。
- 判定：`dy > 0.35` → 台阶超限阻挡；前方 0.55m 前瞻坡角 `> 45°` → 坡度超限阻挡；否则沿地面高度平滑上/下。

## 四、三种跑法

### 1. 手动真窗口（人坐屏幕前按键）

```powershell
cd D:\rust3d-engine
.\target\x86_64-pc-windows-gnu\debug\r3e.exe `
  "assets\labs\character_stairs\stairs_lab.r3e.json" "assets\labs\character_stairs\scripts\keyboard_probe.rhai" `
  --physics on --physics-backend rapier --window-size 1280x720 --run-seconds 600
```

窗口获得焦点后直接按 `W/A/S/D`，HUD 实时显示当前赛道、脚底高度、通行/阻挡与前方坡度，
控制台每帧打印 `kbd x=.. y=.. z=.. 通行/阻挡<-原因`。

### 2. 无人值守键盘驱动（合成真实按键 + 分步截图）

```powershell
python "assets\labs\character_stairs\tools\drive_keyboard.py"
```

依次 `D` 5.7s（平地横移 6 条车道到 x≈8）→ `W` 3.4s（爬 25° 坡）→ `S` 1.6s（倒退下坡），
在 `evidence/` 下产出 `kbd_00_start.png` / `kbd_01_lane_d.png` / `kbd_02_ramp_w.png` / `kbd_03_ramp_s.png`。

> **按键注入必踩的坑**：`SetForegroundWindow` 会被 Windows 前台锁定拒绝，
> 必须先 `AttachThreadInput(前台线程, 本线程, True)` 再置前，并回读 `GetForegroundWindow()` 校验；
> 否则 `keybd_event` / `SendInput` 全部打空，日志里角色一动不动（本样例踩过一次）。
> 另：`PostMessage(WM_KEYDOWN)` 方案实测不生效，只用 `keybd_event`。

### 3. 无输入的确定性巡检（回归夹具，适合 CI）

| 夹具 | 起点 | 断言 |
|---|---|---|
| `auto_stairs_ok.rhai` | x=-16 | 0.10m 楼梯全程可上 |
| `auto_stairs_blocked.rhai` | x=0 | 0.50m 台阶被挡 |
| `auto_ramp25.rhai` | x=8 | 25° 坡可攀 |
| `auto_ramp60.rhai` | x=16 | 60° 坡被挡 |

```powershell
.\target\x86_64-pc-windows-gnu\debug\r3e.exe "assets\labs\character_stairs\stairs_lab.r3e.json" "assets\labs\character_stairs\scripts\auto_ramp60.rhai" `
  --physics on --physics-backend rapier --window-size 960x540 --run-seconds 10
```

### 4. 全景静帧

```powershell
python "assets\labs\character_stairs\tools\shoot_panorama.py"
```

## 五、本机实测结果（rapier 后端，真窗口）

| 场景 | 观测 |
|---|---|
| `W` 2.6s（x=-16） | z 6.0 → -3.66，脚底 y 0 → **0.40**，状态"上坡 / 上台阶"——12 级 0.10m 楼梯可用 |
| `D` 5.7s | x -16 → **7.955**，全在平地，横穿 6 条车道不被邻道地形卡住 |
| `W` 3.4s（x≈8） | z -0.24 → -5.51，脚底 y 0 → **1.78**；继续到 z=-7.54 时 y=**2.54**（25° 坡爬满） |
| `S` 1.6s（坡上） | z -7.54 → -1.56，脚底 y 回落到 **0** —— 下坡方向同样正确 |
| `auto_stairs_blocked` | z=-1.99 处 `阻挡 <- 台阶超限 0.50m` |
| `auto_ramp60` | z=-1.62 处 `阻挡 <- 坡度超限 46.18°` |

## 六、文件清单

| 文件 | 作用 |
|---|---|
| `stairs_lab.r3e.json` | 场景（9 条赛道 + 平台 + 地面 + 圆柱玩家） |
| `scripts/keyboard_probe.rhai` | 真键盘探针：只读 `key_down`，每帧打位置，带 HUD |
| `scripts/walk.rhai` | 手动交互版（同判定，另有 `C` 全景视角） |
| `scripts/auto_*.rhai` | 4 个无人值守巡检夹具 |
| `scripts/panorama.rhai` | 高位全景静帧脚本 |
| `tools/drive_keyboard.py` | 合成真实按键 + 分步截图 |
| `tools/shoot_panorama.py` | 全景静帧采集 |
| `evidence/shot_panorama.png` | 全景图（多排楼梯 / 斜坡 + 圆柱角色） |
| `evidence/shot_auto_*.png` | 各夹具运行截图 |
| `evidence/kbd_0*.png` | 键盘驱动四步截图 |
*（内容由AI生成，仅供参考）*
