---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_e1c34ce7e94211f1aa8b525400dc9a98
    ReservedCode1: yV/2a4x2H8UI7EMMk8+x8uJYPMVgk5ii4GPEfYL75kEjowIWz6m7PM5SV9aQTKe71gFctQrz50Dc9ynhUg/KFURYtytwr/jR1PY2Mfl9cYeYTF60hY3RseHyZFt8AEjKNN4gawgKCRHAVSVWWtMvCWX0aKrClHLSZP5SoDMBJYfEilq52SimaQrQOl4=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_e1c34ce7e94211f1aa8b525400dc9a98
    ReservedCode2: yV/2a4x2H8UI7EMMk8+x8uJYPMVgk5ii4GPEfYL75kEjowIWz6m7PM5SV9aQTKe71gFctQrz50Dc9ynhUg/KFURYtytwr/jR1PY2Mfl9cYeYTF60hY3RseHyZFt8AEjKNN4gawgKCRHAVSVWWtMvCWX0aKrClHLSZP5SoDMBJYfEilq52SimaQrQOl4=
---

# 导航寻路测试样例 · nav_demo（M2-D07）

一句话：一张带坡度和障碍物的地图，一个圆柱体 agent 从起点出发，由 r3e 的 `nav.*` 系统自己算出路线、绕过两堵墙和两根柱子、爬上 19m 高台，红线就是它实际走的路线。

## 1. 产出物

| 文件 | 内容 |
| --- | --- |
| `nav_demo.r3e.json` | 场景地图（地形 + 2 墙 + 2 柱 + 起终点标记 + agent） |
| `nav_demo_path.png` | **主图**：引擎实拍俯视图 + 红色路线 + 障碍线框 + 起终点 + 坡度标注 |
| `nav_demo_profile.png` | 地形剖面图：平地 → 13.1° 缓坡 → 21.8° 陡坡 → 平台，障碍落位 |
| `nav_demo_steps.png` | 过程帧拼图：全程 36 帧里等间隔取 8 帧 |
| `nav_demo_walk.gif` | 过程动画：36 帧，红线随圆柱体推进而增长 |
| `tools/nav_demo_run.py` | 一键跑：起宿主 → 烘焙 → 求路 → 逐帧移动 agent 抓图 → 落 `evidence/data.json` |
| `tools/nav_demo_draw.py` | 一键出图：读 `evidence/data.json` 画上面三张图 + GIF |
| `evidence/data.json` | 全部原始观测数据（烘焙结果、坡度直方图、路径点、采样点、投影矩阵与标定残差） |

## 2. 地图设计

地形 120m × 50m，高度只沿 +X 方向变化（25×13 个顶点，5.0m × 4.17m 网格）：

| 区段 | X 范围 | 高程 | 坡度 |
| --- | --- | --- | --- |
| 平地 | -60 ~ -35 | 0 ~ 0 | 0° |
| 缓坡 | -35 ~ -5 | 0 → 7 | 13.1° |
| 陡坡 | -5 ~ 25 | 7 → 19 | 21.8° |
| 平台 | 25 ~ 60 | 19 | 0° |

障碍（全部落在坡面上，因此绕行时必然带高度变化）：

| 名称 | 位置 | 尺寸 | 作用 |
| --- | --- | --- | --- |
| `wall_a` | x 4~7，z -25~8 | 3m 厚 × 8m 高 | 封住 z<8，只留北侧缺口 |
| `wall_b` | x 16~19，z -3~25 | 3m 厚 × 8m 高 | 封住 z>-3，与 wall_a 反向交错 → 路径必须走 S 形 |
| `pillar_a` | (9.2, z=4) | 半径 1.5m × 高 7m | 贴着走廊西侧，把通路挤向东 |
| `pillar_b` | (13.8, z=-2) | 半径 1.5m × 高 7m | 贴着走廊东侧，形成蛇形夹缝 |

起点 `(-50, 0, -12)`，终点 `(48, 19, -12)`。画面标记 `#start_marker`（绿）与 `#goal_marker`（蓝）以 `#` 开头，按 `bake::collect_obstacles` 的约定不参与导航；agent 带 `player` 标签，`player` 在 `NavConfig::default().ignore_tags` 里，不会被烘成障碍。

## 3. 复跑

```bat
cd /d D:\rust3d-engine
target\x86_64-pc-windows-gnu\debug\r3e.exe assets\labs\nav_demo\nav_demo.r3e.json ^
    --window-size 1280x800 --rpc-port 8791 --rpc-token secret --physics off --run-seconds 900
```

另开一个终端（PowerShell 直接写 JSON 引号会被吞，建议走 Python subprocess / `cmd /c` 两层转义）：

```bat
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token secret --json rpc nav.bake
target\x86_64-pc-windows-gnu\debug\r3e-cli.exe --token secret --json rpc nav.path --args "{\"from\":[-49.75,0,-12.15],\"to\":[48.25,19,-12.15]}"
```

或者直接：

```bat
python assets\labs\nav_demo\tools\nav_demo_run.py
python assets\labs\nav_demo\tools\nav_demo_draw.py
set NAV_STEPS=36 && python assets\labs\nav_demo\tools\nav_demo_run.py
```

`NAV_STEPS` 是过程帧数量，默认 8；跑动画用 36。

## 4. 实测数据

烘焙（`nav.bake` + `nav.stats`）：

| 项 | 值 |
| --- | --- |
| 网格 | 0.5m，244 × 106 = 25864 格 |
| 可走 | 22879 格（88.5%），单连通域，无悬空 spawn |
| 坡度分类 | flat 11900 / slope 10979 / blocked 2985 / step 0 |

寻路（`nav.path` + `nav.smooth`）：

- 起点吸附 `(-49.75, 0.0, -12.15)`，终点吸附 `(48.25, 19.0, -12.15)`（由 `nav.nearest` 给出，未用几何近似坐标）。
- 8 个 waypoint，长度 116.73m（含爬升 19m；平面投影 114.82m）。
- 路径形状：先向 +z 插到 `z≈8.85` 从 wall_a 北端缺口穿过 → 进入两墙之间的走廊 → `z 8.85 → 5.85 → 0.85 → -0.65 → -4.15` 贴着两柱之间穿下 → `z<-3` 从 wall_b 北侧缺口穿出 → 上平台到终点。
- `nav.smooth` 输入 8 点、输出 8 点（该路径的 waypoint 段本身已是可直连的直线，平滑器无需插点）。

过程帧（36 帧，agent 高 0.9m 立于地面）：

| 帧 | 位置 | 地面高 | 备注 |
| --- | --- | --- | --- |
| 0 | (-49.8, 0.90, -12.1) | 0.0 | 起点 |
| 8 | (-25.3, 3.16, -2.6) | 2.3 | 缓坡上 |
| 16 | (-0.9, 9.54, 7.0) | 8.6 | 陡坡，即将钻 wall_a 缺口 |
| 20 | (10.9, 14.27, 6.8) | 13.4 | 两墙之间的走廊，贴 pillar_a 东侧 |
| 24 | (13.0, 15.09, -4.5) | 14.2 | 已切到 pillar_b 西侧，穿 wall_b 缺口 |
| 28 | (25.8, 19.90, -7.3) | 19.0 | 上平台 |
| 35 | (48.2, 19.90, -12.1) | 19.0 | 终点 |

## 5. 图怎么来的（红线为什么可信）

1. `capture.frame` 出的底图是引擎真实渲染画面（1280×800，相机 `pos(0,132,92) → target(0,9,-2)`，fov 46°）。
2. 为了把 3D 路线叠到 2D 图上，先按相机参数算出投影矩阵，再用 8 个洋红标定块（`#cal_*`，spawn 后抓图、按像素聚类求质心、一对一匹配）反查：
   **实测残差最大 0.93px、中位 0.73px、rms 0.70px** —— 说明叠加位置与画面是同一套投影，红线不会"贴歪"。
3. 采样点坐标由 `nav.path` 的 waypoint 沿弧长等分得到，不是手画的示意线。
4. 标定块用完后 `scene.remove` 撤掉，`base_overview.png` 是干净底图。

## 6. 已知边界

- 本样例只验证"单 agent 沿静态导航网格求路并走过去"，不含动态障碍、不含多 agent 避让（`NavAgent` 的 follow 回归在 `crates/r3e-nav/tests/nav_agent_follow.rs`）。
- 相机固定、地形高度取自 `terrain.heights` 的 25×13 采样；若把网格从 5.0m 加密，坡度转折处的可走边界会略变，`nav.bake` 需重跑（`nav.hash` 会变）。
- 画面标注的"坡度 xx°"是剖面切线角，取自场景 `terrain_profile` 折点，与实际烘焙网格的台阶化边沿会有 <1° 差异。
*（内容由AI生成，仅供参考）*
