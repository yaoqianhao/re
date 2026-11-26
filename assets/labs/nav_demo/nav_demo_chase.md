---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_18cb9a6fe94911f1aa8b525400dc9a98
    ReservedCode1: JETIDuhgjZJoZO9bLzNfECQHaqoud31Wm0E0nAPMc8X5vMYmWjf2JW039qYDDVnZly824hUwGAp1RT1OfF7uqU6QC07KLIjkReoI7ERS2IWomP6FotxS5LLNokLC3QCgcd5S2Bt14MMcZk+Xp35Vb8xqH8w9oKWtiCBTnY7SCyXwOA9in0rCCTQTSHM=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_18cb9a6fe94911f1aa8b525400dc9a98
    ReservedCode2: JETIDuhgjZJoZO9bLzNfECQHaqoud31Wm0E0nAPMc8X5vMYmWjf2JW039qYDDVnZly824hUwGAp1RT1OfF7uqU6QC07KLIjkReoI7ERS2IWomP6FotxS5LLNokLC3QCgcd5S2Bt14MMcZk+Xp35Vb8xqH8w9oKWtiCBTnY7SCyXwOA9in0rCCTQTSHM=
---

# nav_demo 追踪镜头版（nav_demo_chase.py）

在 `nav_demo` 静态俯视演示基础上，补一个**相机跟随圆柱体 agent 移动**的追踪镜头版本，用于看「寻路过程本身」而不是只看结果路线。

## 与 nav_demo_run.py 的差异

| 项 | nav_demo_run.py（俯视版） | nav_demo_chase.py（追踪版） |
| --- | --- | --- |
| 相机 | 固定 (0,132,92)→(0,9,-2) fov46 | 每帧按 agent 位置重算机位 |
| 画面主体 | 全图 + 整条红线 | agent 在画面中部、身前路径可见 |
| 标记块 | `#start_marker` / `#goal_marker` 留在场景里 | 起宿主后 `scene.remove` 摘掉（2×2×2 方块会包住 agent、遮挡视线），起终点改在图上画圈+文字 |
| 产物 | 主图/剖面图/过程拼图/GIF | 追踪 GIF / 关键帧拼图 / 俯视总览图 |
| 数据 | evidence/data.json | evidence_chase/data_chase.json |

导航链路完全复用：`nav.bake → nav.stats → nav.nearest → nav.path → nav.smooth`，场景仍为 `nav_demo.r3e.json`（120×50，平地 + 13.1° 缓坡 + 21.8° 陡坡 + 平台，两墙两柱）。

## 追踪机位

```python
CAM_BACK   = 24.0   # 机位在 agent 后方的水平距离
CAM_HEIGHT = 16.0   # 机位相对 agent 的高度（低于俯视机位，但高于 8m 的墙）
LOOK_AHEAD = 9.0    # 视线落点在 agent 前方距离 → agent 偏画面下方，能看见身前路径
LOOK_LIFT  = 1.0    # 落点相对 agent 的抬高
FOV        = 50.0
```

- 机位方向按**当前段路径切线**取反推水平偏移，路径转弯时机位跟着转，不穿墙（墙高 8m，机位离地 ≥16m）。
- 每帧 3 条 RPC：`scene.settransform`（移动 agent）→ `script.eval "camera(...)"` → `capture.frame`。
- 沿 `nav.smooth` 等弧长取 `CHASE_FRAMES` 个点，默认 48 帧。

## 红线绘制

追踪镜头视野只有身前几十米，整条路径大半落在相机后方，所以分两层画：

- **淡红线**：整条路线（48 个等弧长点，比 `nav.smooth` 的 8 点折线更贴合弯道），画在已走过部分之下。
- **实红线**：`walked`（0..k 的轨迹），带深红描边。

投影矩阵逐帧按该帧机位重算，并把 agent 世界坐标投到屏幕做校验。

## 总览图（补全貌）

追踪帧交代不了全程，脚本最后把相机切回场景俯视机位 `(0,132,92)→(0,9,-2) fov46` 再抓一帧，按**该机位**重算投影，画出完整 `walked` 路径与起终点圈 → `nav_demo_chase_overview.png`。

## 运行

```bat
python assets\labs\nav_demo\tools\nav_demo_chase.py
```

帧数可调（冒烟用 12 帧，成品 48 帧）：

```bat
set CHASE_FRAMES=12 && python assets\labs\nav_demo\tools\nav_demo_chase.py
```

依赖：`numpy`、`Pillow`；宿主 `target\debug\r3e.exe` 需先 build。字体走 `C:\Windows\Fonts\msyhbd.ttc`（成员函数 `font()` 带绝对路径回退，避免 PIL 静默退成默认小字）。

## 实测结果（48 帧）

| 指标 | 数值 |
| --- | --- |
| bake 可走格 | 22879 / 88.5%（1 区） |
| 路径长度 | 116.73 m（path 8 点 → smooth 116.73 m） |
| agent 完整可见 | 42 / 48 帧（部分露出 3，被墙/柱遮挡 3） |
| 实红线出现 | 34 / 48 帧（其余帧只画淡红整线：走过段很短或弯出视野） |
| agent 投影残差 | 均值 1.23 px，最大 11.07 px（遮挡帧） |
| 俯视总览残差 | 0.79 px |
| 相机步进 | 最大 6.92 m/帧，均值 3.12 m |
| 偏航步进 | 最大 19.6°，均值 3.42° |

## 产物

| 文件 | 说明 |
| --- | --- |
| `nav_demo_chase.gif` | 48 帧追踪动画（832×520） |
| `nav_demo_chase_contact.png` | 8 格关键帧拼图 |
| `nav_demo_chase_overview.png` | 俯视总览（完整路径 + 起终点） |
| `evidence_chase/frames/*.png` | 48 张原尺寸帧（1280×800） |
| `evidence_chase/data_chase.json` | 每帧 agent/机位/投影矩阵/轨迹 |
*（内容由AI生成，仅供参考）*
