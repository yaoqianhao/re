---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_814f577cd4fb11f19fe5525400638852
    ReservedCode1: rBV+Ga3vu/7Y+yhFql6P63uQ9l6MO/kol95gdS+n0wjXv0JB6Swd5kwKPGALud+ODb5hryogajr2NkCatFiCj/X4sMk+Rf7tXu10XBo+wL59XjcL8gpind8Tw6QvlDYsh1aNmtfzbnq20bnAoHkply67VA+LH/InHX+IgCC7i0e0bp3RyzW2SlLiHF4=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_814f577cd4fb11f19fe5525400638852
    ReservedCode2: rBV+Ga3vu/7Y+yhFql6P63uQ9l6MO/kol95gdS+n0wjXv0JB6Swd5kwKPGALud+ODb5hryogajr2NkCatFiCj/X4sMk+Rf7tXu10XBo+wL59XjcL8gpind8Tw6QvlDYsh1aNmtfzbnq20bnAoHkply67VA+LH/InHX+IgCC7i0e0bp3RyzW2SlLiHF4=
---





# r3e 引擎架构文档

> 版本：0.1.0
> 状态：已实现并通过实机验证（Windows 11 / RTX 5060 Laptop / Vulkan 后端）
> 文档定位：描述**当前代码真实实现**的分层结构、数据流、扩展点与已知边界，不是设计愿景稿。

---

## 1. 引擎定位

r3e 是一个用纯 Rust 实现的**最小可用 3D 游戏引擎**，核心命题只有一个：

> **用文本（JSON / 脚本 / 终端指令）就能完整驱动一个 3D 场景。**

围绕这个命题，形成三条不可动摇的设计原则：

| 原则 | 具体含义 | 在代码中的落点 |
| --- | --- | --- |
| 零外部资源依赖 | 不需要任何 `.obj` / `.gltf` / 贴图文件即可跑起来 | `r3e-render/src/mesh.rs` 全程序化生成 4 种内置网格 |
| 文本即接口 | 场景、行为、调试全部可由文本表达，且改动即时可见 | 场景 JSON + Rhai 脚本 + stdin REPL，三者写同一份 `Scene` |
| 物理可插拔 | 物理是抽象层，不是硬依赖；有实现也可整体关掉 | `PhysicsBackend` trait + Null / Builtin 双实现 + 运行时开关 |

这决定了引擎当前的技术形态：**单 pass 前向渲染 + 程序化几何 + 实例化绘制 + 脚本驱动状态**，而不是一个追求画面效果的完整渲染器。

---

## 2. 技术栈

全部依赖在根 `Cargo.toml` 的 `[workspace.dependencies]` 统一声明，各 crate 只引用不指定版本，避免版本漂移。

| 领域 | 依赖 | 版本 | 用途与选型理由 |
| --- | --- | --- | --- |
| 图形 API | `wgpu` | 30.0 | 跨后端（DX12 / Vulkan / GLES），无需手写平台代码 |
| 窗口与事件 | `winit` | 0.30 | `ApplicationHandler` 模型，与 `wgpu` surface 生命周期契合 |
| 手柄 | `gilrs` | 0.11 | 跨平台手柄抽象（XInput / DirectInput / evdev），把各厂商按键与轴归一到统一枚举 |
| 数学 | `glam` | 0.34（`bytemuck` feature） | SIMD 友好的 Vec/Mat/Quat，矩阵可直接零拷贝上传 GPU |
| 脚本 | `rhai` | 1.27（`serde` feature） | 纯 Rust 脚本，无 C 依赖，编译期即可嵌入式集成 |
| 序列化 | `serde` / `serde_json` | 1.0 | 场景文件解析，全部字段 `Option` + `default` 容错 |
| GPU 数据转换 | `bytemuck` | 1.21（`derive`） | `Pod` / `Zeroable` 派生，顶点与实例数据零成本转字节 |
| 异步桥接 | `pollster` | 0.4 | 把 `request_adapter` / `request_device` 的异步调用同步化 |
| 日志 | `log` + `env_logger` | 0.4 / 0.11 | 分级日志，默认 `info`，`RUST_LOG=debug` 看细节 |
| 音频播放 | `rodio`（含 `cpal` + `symphonia`） | 0.23（`default-features = false`，只开 `playback` / `wav` / `vorbis` / `mp3`） | 设备与解码全封装，上层零 `unsafe`；与 `Physics` 门面同构的 `Audio` 门面可直接替换真后端 / 静默后端 |
| 错误 | `anyhow` / `thiserror` | 1.0 / 2.0 | 应用层用 anyhow，库层返回 `String` 错误保持 API 简单 |

编译配置：`[profile.dev] opt-level = 1`，且依赖统一 `opt-level = 3`——调试构建下 `wgpu` / `glam` 仍按优化编译，否则帧率会低到无法交互。

Rust 版本要求：workspace 声明 `rust-version = 1.79`（core / 渲染 / 脚本链路的底线）；
`crates/r3e-audio` 因 `rodio 0.23` 自身要求 rustc ≥ 1.87，单独声明 `rust-version = 1.87`。
两者不冲突（crate 级声明覆盖 workspace 默认值），但**编译整个 workspace 需要 ≥ 1.87 的工具链**；
本机实测工具链为 `rustc 1.99.1`。若用 1.79–1.86 的工具链，`cargo build` 会在 `r3e-audio` 处报 MSRV 错误。

---

## 3. Workspace 结构

```
rust3d-engine/
├─ Cargo.toml                  # workspace 根，统一依赖版本
├─ run.bat                     # 一键运行
├─ README.md
├─ .cargo/config.toml          # 构建环境配置
├─ assets/
│  ├─ scene.json               # 默认场景（也是编译期兜底场景）
│  ├─ scene_*.json             # 其余 10 个演示场景（HDR / IBL / SSAO / 物理 / 动画 / 局部光源…）
│  └─ scene.schema.json        # 场景 JSON 的机器可读契约（由 serde 结构派生，见 §4.13）
├─ scripts/
│  ├─ main.rhai                # 默认行为脚本
│  └─ example_swarm.rhai       # 群体动画示例
├─ docs/
│  ├─ ARCHITECTURE.md          # 本文档
│  ├─ PHYSICS.md               # 物理子系统完整说明
│  └─ ASSETS.md                # 资产管线：FBX / glTF 导入、AIR 统一格式、已知限制
└─ crates/
   ├─ r3e-core/                # 内核：数学、变换、节点、场景图、相机、光照
   ├─ r3e-render/              # 渲染：wgpu 设备、管线、程序化网格、WGSL
   ├─ r3e-physics/             # 物理：后端契约 + Null / 内置最简 / Rapier（可选 feature）
   ├─ r3e-script/              # 脚本：Rhai 宿主 + 引擎 API 绑定（含 audio_*）
   ├─ r3e-audio/               # 音频：Audio 门面 + rodio / silent 后端 + 空间化覆盖层
   ├─ r3e-asset/               # 资产：FBX / glTF / GLB 导入，统一输出 AIR
   └─ r3e-app/                 # 应用：窗口、事件循环、主循环、REPL、热重载
```

音频演示资产在 `assets/audio/`：`blip.wav`（0.18s 短音效）与 `ambience.wav`（2.0s 循环环境音），
均为脚本合成的正弦波，非第三方版权素材。

### 3.1 依赖关系（严格单向，无环）

```
                        ┌──────────────────┐
                        │  r3e-app         │  可执行体（唯一组装层）
                        └─┬────┬────┬────┬─┘
              ┌───────────┘    │    │    └───────────┐
              ▼                ▼    ▼                ▼
      ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐
      │ r3e-render │  │ r3e-script │  │r3e-physics │  │ r3e-asset  │
      └─────┬──────┘  └────┬───┬───┘  └─────┬──────┘  └─────┬──────┘
            │              │   │            │               │
            │              │   └────────────┘               │
            │              │        （physics API 绑定）      │
            │        ┌─────┴───────┐                        │
            │        ▼             │                        │
            │  ┌────────────┐      │  仅 r3e-script /       │
            │  │ r3e-audio  │      │  r3e-app 认识它        │
            │  └─────┬──────┘      │                        │
            └────────┼─────────────┴────────────────────────┘
                     ▼
              ┌──────────────┐
              │  r3e-core    │  无图形/窗口/音频设备依赖
              └──────────────┘
```

依赖约束及其意义：

- `r3e-core` **不依赖任何图形 API、窗口库、脚本引擎或音频设备**，只依赖 `glam` + `serde`。因此它是可测试、可无头运行的真源层。
- `r3e-render`、`r3e-physics`、`r3e-asset`、`r3e-audio` 四者**互不依赖**：渲染器不认识 Rhai，资产层不认识 wgpu，物理层不认识 ufbx，音频层不认识场景图。
- `r3e-audio` **连 `r3e-core` 都不依赖**（只用 `rodio` + `log`）：它管的是"key → 声音实例"的账本，与节点、相机、场景完全无关，因此可以被任意宿主复用。
  空间化（§15.8）没有破坏这条：三维数学（距离衰减、径向速度、多普勒比值）以 `[f32; 3]` 与纯函数形式写在
  `r3e-audio::spatial` 里，**不引用 `Vec3` / `Transform` / `NodeId`**；"声源挂哪个节点"由宿主每帧把世界坐标
  灌进运行时注册表，音频层始终不认识场景图。
- 唯一的跨子系统依赖是 `r3e-script → r3e-physics` 与 `r3e-script → r3e-audio`：脚本 API 要直接持有 `Physics` / `Audio` 门面做绑定，这是刻意的（不是环，两个被依赖层都不反向认识脚本）。
- `r3e-app` 是唯一的组装层，负责把各子系统接进同一个帧循环。
- `r3e-editor` 只定义 JSON-RPC 协议与 `EngineApi` trait，**不依赖任何其它 r3e crate**，也不碰图形/窗口；
  它的唯一实现者是 `r3e-app::editor::LocalEngineApi`（桥接同一份 `Scene` / `Physics` / `ScriptHost`），
  由 `EngineApp::drain_editor_jobs` 每帧消化至多 `--rpc-max-per-frame` 条请求。
  依赖方向永远是 `r3e-app → r3e-editor`，编辑器层不认识渲染器。

这条约束是引擎"文本能控制画面"的结构性保证：**所有子系统读写同一份内存中的 `Scene`，而不是通过消息或事件副本通信。**

### 3.2 代码规模

| crate | src 行数 | tests/ 行数 | 单测数 | 主要文件 |
| --- | --- | --- | --- | --- |
| r3e-core | 18,734 | 3,187 | 410 | `scene.rs` / `anim_blend.rs` / `scene_stack.rs` / `scene_schema.rs` / `material.rs` / `env.rs` / `envmap.rs` / `postfx.rs` / `ssao.rs` / `occlusion.rs` / `shadow_cull.rs` / `tangent.rs` / `texture_slot.rs` / `text.rs` / `glyph_atlas.rs` / `ui.rs` / `skin.rs` / `anim.rs` / `aabb.rs` / `frustum.rs` |
| r3e-render | 8,595 | 2,346 | 172 | `renderer.rs` / `pbr.rs` / `postfx.rs` / `ssao.rs` / `occlusion.rs` / `envprobe.rs` / `shadow_cull.rs` / `transparency.rs` / `text.rs` / `ui.rs` / `mesh.rs` / `shader.wgsl` / `postfx.wgsl` / `ssao.wgsl` / `text.wgsl` / `ui.wgsl` |
| r3e-asset | 6,227 | 585 | 59 | `lib.rs`（FBX）/ `gltf_import.rs` / `importer.rs` / `visual_regression.rs`（像素比对纯函数） |
| r3e-physics | 2,550 | 662 | 28 | `lib.rs`（契约 + 内置后端）/ `rapier_backend.rs`（Rapier 后端，feature 可选） |
| r3e-audio | 2,338 | — | 29 | `lib.rs`（门面 / 账本 / 空间化覆盖层）/ `backend.rs`（rodio + silent，含 `set_speed`）/ `spatial.rs`（衰减 / 多普勒 / 听者纯函数） |
| r3e-script | 5,402 | 1,219 | 88 | `api.rs` / `lib.rs`（含文本、UI、音频、空间化音频、材质、环境、HDR 环境探针、遮蔽、遮挡剔除、动画层、局部阴影语义单测） |
| r3e-editor | 3,027 | 1,656 | 95 | `lib.rs`（协议 + `EngineApi`/`EnvironmentPatch`/`EnvironmentMapPatch`/`SsaoPatch`/`AnimLayerPatch`/`AudioSpatialPatch`）/ `server.rs`（TCP 桥 + 资源上限单测） |
| r3e-app | 6,893 | 231 | 61 | `app.rs`（含 `drive_spatial_audio` / `sync_env_probe` / `capture_frame_now`）/ `editor.rs`（`EngineApi` 实现）/ `main.rs`；`tests/visual_regression.rs`（像素回归入口 + 基准图） |
| r3e-plugin | 3,711 | — | 47 | `main.rs` / `host.rs`（宿主与 ABI）/ `protocol.rs` / `rpc.rs` / `spec.rs`（工具 schema）/ `tools.rs`（读写侧工具集）/ `locate.rs` / `smoke.rs` |
| **合计** | **57,477** | **9,886** | **989** | **约 5.7 万行 Rust + 1,310 行 WGSL；`cargo test --workspace` 69 个套件 / 985 passed / 0 failed** |

行数口径：`src` / `tests` 均为**含空行与注释的物理行数**，「src 行数」列只数 `.rs`（含 `examples/`），
着色器（1,310 行 WGSL，全在 `r3e-render`）单列在合计行；随扩展二十七（主通道实例收集单次遍历）收尾一次性重算，
扩展二十八（段归属下沉到节点）在其上增量 +6 src / +33 tests / +1 单测
（此前停在扩展二十三收尾快照：56,243 行 / 8,619 tests 行 / 932 单测 / 63 套件 928 passed；
本次重算并入扩展二十四~二十七的全部新增：层级剔除、着色器法线兜底、实例分桶契约等测试文件 + WGSL 13 行）。
单测数一列是 `#[test]` **标注数**，
其中 4 条只在 `--features rapier` 下参与（`crates/r3e-physics/tests/ccd_penetration.rs` 整文件 3 条 +
`backend_contract.rs::rapier_stacks_bodies_and_reports_collision_events`），故默认特性实跑 985。

Release 二进制 14.3 MB（14,949,888 字节；`[profile.release]` 已开 `lto = "thin"` / `codegen-units = 1` /
`strip = "symbols"` / `panic = "abort"`，比旧快照的 32.0 MB 瘦一半，静态链接的 wgpu 后端与 ufbx 仍在）。

### 3.3 质量门（每次改动后必过）

| 门 | 命令 | 当前口径 |
| --- | --- | --- |
| 格式 | `cargo fmt --all --check` | 0 diff（rustfmt 由 `rust-toolchain.toml` 固定组件） |
| 单测（默认特性） | `cargo test --workspace` | 985 passed / 0 failed（69 个套件；较扩展二十七收尾的 984 增 1，来自 `instance_buckets_contract::node_enters_only_its_own_segment`：扩展二十八把「两段各收全量」钉死） |
| 单测（Rapier） | `cargo test --workspace --features rapier` | 989 passed / 0 failed（默认特性 + `ccd_penetration.rs` 整文件 3 条 + `backend_contract.rs::rapier_stacks_bodies_and_reports_collision_events`）。**踩坑**：该轮若在链接 `r3e-app` bin 目标时报 `undefined reference to 'anon.*.llvm.*'`（mingw `ld` 与新 rustc 的 UB 检查符号命名不兼容、只报链接阶段），先 `cargo clean -p r3e-app -p r3e-render` 再重编即恢复，不是代码问题 |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings`（另跑 `--features rapier`） | 0 warning，**`-D warnings` 下零告警才算过** |
| 无头构建 | `cargo build --release` + `r3e.exe --run-seconds 2` | 零 warning，退出码 0 |
| 像素回归（CI 闸门） | `$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression` | 闸门开、有显卡才真比像素；托管 runner 无显卡时跳过，由同文件纯 CPU 用例守基准图（存在 / 可解码 / 640×360）。CI 内禁止 `R3E_UPDATE_BASELINE`（否则回归会被"新图覆盖旧基准"洗成永远绿） |
| 像素漂移（CI 闸门，自建 GPU） | 同上 + `$env:R3E_VISUAL_STRICT = "1"` | `visual-gpu` job，`runs-on: [self-hosted, windows, gpu]`，变量 `R3E_GPU_RUNNER=true` 时启用。严格模式下**跳过即失败**：显卡掉、驱动崩、窗口建不出来都算红，不再伪装成"这台机器不支持"。配置见 `docs/CI_GPU_RUNNER.md` |
| CI | `.github/workflows/ci.yml`（fmt / clippy / test / visual / visual-gpu，五个 job，全 Windows） | 每次 push、PR 自动跑，红则不合；`visual-gpu` 未配 runner 时显示 skipped（不阻塞合并，但可见） |

静态检查仅保留三处带注释的 `#[allow]`：`arc_with_non_send_sync`（渲染器主线程独占，`MeshProvider` 刻意不 `Send`）、
`too_many_arguments`（顶点装配入口参数天然是平行数组，`r3e-asset` 内两处）。
工具链固定在 `rust-toolchain.toml`（`stable-x86_64-pc-windows-gnu` + `rustfmt` + `clippy`），
写全 host triple 的原因见 README §11。

---

## 4. 内核层：r3e-core

### 4.1 模块划分

| 模块 | 职责 | 关键类型 |
| --- | --- | --- |
| `transform.rs` | 单节点局部变换数学 | `Transform` |
| `node.rs` | 物件本体与网格种类枚举 | `Node`, `NodeId`, `MeshKind` |
| `scene.rs` | 场景容器、生命周期、层次变换、文件加载 | `Scene`, `ObjectDesc`, `SceneFile` |
| `camera.rs` | 透视相机与轨道控制 | `Camera` |
| `light.rs` | 单方向光 + 环境光 + 局部光源表 + 局部光源阴影的选灯/槽位/面相机（纯函数，零 wgpu） | `Light`, `LocalLight`, `plan_local_shadows`, `point_shadow_face`, `shadow_face_view_projection` |
| `text.rs` | 字体加载、字形位图接口、排版（换行 / 对齐 / 缺字回退）、屏幕文本节点 | `FontSet`, `GlyphSource`, `TextLayout`, `ScreenText`, `TextAlign` |
| `ui.rs` | 屏幕空间 UI 元素、锚点与单位换算、层级排序（纯数学，不碰 GPU） | `UiElement`, `UiKind`, `Anchor`, `UiUnit`, `UiValue`, `UiRect`, `resolve_rect`, `sort_by_layer` |
| `glyph_atlas.rs` | 字形图集的货架式分配与分页 | `GlyphAtlas`, `ShelfAllocator`, `GlyphKey` |
| `material.rs` | PBR 材质参数、收敛与 Cook-Torrance 纯函数参考实现（零 wgpu 依赖，可单测） | `SurfaceMaterial`, `GeoTerms`, `BrdfSample`, `cook_torrance_brdf`, `direct_light` |
| `tangent.rs` | 切线空间基（TBN）构造：按三角形位置 / UV 差分累加再 Gram-Schmidt 正交化，UV 面积退化跳过、整网格退化退回任意正交基，手性来自副切线方向（扩展二十一；纯 CPU 纯函数） | `compute_tangents`, `MeshData::recompute_tangents` |
| `texture_slot.rs` | 贴图槽位分类：把 FBX / glTF 五花八门的槽位名收敛成「这张图是干什么用的」，并给出「是否按线性数据上传」判据（扩展二十一；导入层与渲染层共用同一套判据） | `TextureSlot`, `classify_slot` |
| `occlusion.rs` | 遮挡剔除的 CPU 判定：把上一帧深度压成 tile 最远深度金字塔（HZB 的保守替代），对世界 AABB 做屏幕空间两阶段遮挡测试（扩展二十二；金字塔是纯 `Vec<f32>`，判定是纯算术，零 wgpu 依赖） | `OcclusionSettings`, `DepthPyramid`, `ScreenBounds`, `OcclusionVerdict`, `project_aabb`, `test_aabb` |
| `env.rs` | 三色天空 + 太阳的解析环境探针，Sh9 球谐投影与解析环境 BRDF（零 wgpu，可单测） | `EnvironmentSettings`, `Sh9`, `env_brdf_approx` |
| `postfx.rs` | HDR 后处理的数值契约：曝光、软膝亮部提取、Reinhard / ACES 曲线、FXAA 边缘判定、composite 参考实现 | `PostFxSettings`, `BloomSettings`, `TonemapMode`, `expose`, `bright_pass`, `tonemap`, `resolve` |

| `scene_stack.rs` | 场景栈、运行时快照与存档（扩展十八 B）：层里存「离开时的样子」、浏览器式历史语义、命令信封（纯数据 + 纯函数，唯一 IO 是 `SceneArchive` 的读写盘） | `SceneStack`, `StackEntry`, `RuntimeSnapshot`, `AnimRuntime`, `SceneArchive`, `SceneCommand`, `MAX_SCENE_STACK` |
**为什么文本在 core 而不是 render**：字形栅格化（fontdue）与排版是纯 CPU 计算，与 wgpu 无关。
放 core 有两个直接好处——① `r3e-core` 保持零 GPU 依赖，`architecture_guard` 测试守着这条线；
② 排版、对齐、缺字回退这些逻辑可以用假字体在无 GPU 环境跑单测，不必起窗口。
`r3e-render` 只消费 `TextLayout`（每字形的位图 + 位置 + 尺寸），把它翻译成实例数据。


`lib.rs` 只做 re-export：`pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4}`，让上层 crate 不必各自引入 `glam`。

### 4.2 Transform：变换表示

```rust
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
```

设计要点：

1. **旋转用四元数存储，用欧拉角交互**。存储层 `Quat` 避免万向节死锁与插值退化；接口层提供 `set_euler_deg` / `euler_deg`（XYZ 顺序、角度制），因为脚本用户写 `set_rot("box", 0, 45, 0)` 最自然。
2. **局部矩阵组合顺序固定 `T * R * S`**（`Mat4::from_scale_rotation_translation`），与主流引擎一致；先缩放再旋转再平移，非均匀缩放不会污染旋转。
3. **`look_at` 需要取逆**。`glam` 的 `look_at_quat` 返回的是"相机朝向"（前向为 -Z），而物件的"朝向"是 +Z 前沿，因此 `Transform::look_at` 内部调用 `.inverse()`。这是易错点，注释已在代码中标明。
4. **`normal_matrix` 在 CPU 侧不用于渲染**。着色器用伴随矩阵近似，CPU 侧该函数预留给将来需要 CPU 侧法线计算的功能（如包围盒、拾取）。

### 4.3 Node：场景中的物件

```rust
pub struct Node {
    pub id: NodeId,               // u64，单调递增、永不复用
    pub name: String,             // 场景内唯一
    pub transform: Transform,
    pub color: Vec4,              // 线性空间 RGBA
    pub mesh: MeshKind,
    pub visible: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub body: Option<u64>,        // 物理刚体句柄（物理关闭时恒为 None）
    pub world: Mat4,              // 每帧刷新的世界矩阵缓存
    pub world_normal: glam::Mat3, // 世界法线矩阵缓存
}
```

- **`NodeId` 不复用**是刻意的：脚本/物理持有的是句柄，如果 ID 回收，一个残留句柄就会静默指向新物件，产生极难排查的 bug。代价只是 64 位空间缓慢增长，可以忽略。
- **`world` / `world_normal` 是缓存而非真源**，由 `Scene::update_world_transforms` 每帧重算。渲染器只读它，从不自己算矩阵。
- **`MeshKind` 仅 4 种**：`Cube` / `Sphere` / `Plane` / `Cylinder`，并支持别名解析（`box`→Cube、`ball`→Sphere、`ground`/`quad`→Plane、`tube`→Cylinder）。没有自由网格，是当前几何表达力的主要边界。

### 4.4 Scene：唯一真源

```rust
pub struct Scene {
    nodes: HashMap<NodeId, Node>,   // ID → 节点
    order: Vec<NodeId>,             // 插入顺序（绘制顺序稳定性）
    name_index: HashMap<String, NodeId>, // 名字 → ID（脚本按名寻址）
    next_id: NodeId,

    pub name: String,
    pub time: f32,      // 引擎时间（秒）
    pub frame: u64,     // 帧序号
    pub camera: Camera,
    pub light: Light,
    pub background: Vec4,   // RGBA
    // …… physics / assets / animations / anim_clips / skins（见 6~8 章）
    // M1：屏幕空间文本与文本通道开关
    pub texts: Vec<ScreenText>,   // 按 key 寻址、同名覆盖；顺序即绘制顺序
    pub text_enabled: bool,       // false 时渲染层整帧跳过文本通道，数据保留
}

/// 一条屏幕空间文本节点（`r3e_core::SceneText`）
pub struct ScreenText {
    pub key: String,        // 脚本侧寻址用，场景内唯一
    pub content: String,    // 任意 UTF-8
    pub x: f32, pub y: f32, // 第一行行顶锚点（窗口像素，原点左上）
    pub size: f32,          // 像素字号
    pub color: [f32; 4],    // 含 alpha
    pub align: TextAlign,   // Left / Center / Right
    pub wrap: f32,          // 自动换行宽度像素，0 = 只按 \n
    pub line_height: f32,   // 行高倍数，0 = 字体自然行高
    pub visible: bool,
}
```

**三索引结构**是核心设计：`HashMap` 保证 O(1) 按 ID 访问，`name_index` 保证 O(1) 按名访问（脚本几乎全按名操作），`order` 保证遍历顺序确定——渲染器按 `order` 遍历分组，因此同一份场景每次渲染的实例顺序完全一致，画面不会出现闪烁。

生命周期 API：

| 方法 | 行为 | 关键约束 |
| --- | --- | --- |
| `spawn(desc) -> Result<NodeId, String>` | 生成物件 | 名字必须非空、不得重名、父物件必须已存在；任一违反返回 `Err` |
| `despawn(name) -> bool` | 按名移除 | 不存在返回 `false`，不 panic |
| `despawn_id(id)` | 按 ID 移除 | **递归移除所有子物件**，并从父节点 `children` 中摘除 |
| `clear_nodes()` | 清空所有物件 | 脚本热重载时用，保留相机/光照/背景 |

重名直接报错而不是覆盖，是刻意的保守选择：脚本里 `create_node` 重复创建同名物件时，静默覆盖会让用户以为"物件消失了"，报错更容易定位。

查询 API：`get` / `get_mut`（按 ID）、`find` / `find_mut` / `find_id`（按名）、`find_all(prefix)`（按名前缀，返回排序后的名字列表）、`len` / `is_empty` / `iter`。

**层次变换求解**：

```rust
fn update_world_transforms(&mut self) {
    // 1. 一次性收集所有 id（避免迭代中变借用）
    // 2. 对每个 id 调 resolve_world（递归 + HashMap 缓存）
    // 3. 写回 node.world 与 node.world_normal
}

fn resolve_world(&self, id, depth, cache) -> Mat4 {
    // 缓存命中直接返回
    // world = parent_world * local，depth >= 64 或自引用时退化为 local
}
```

三层防护：

- **记忆化缓存**：父链只算一次，深层次结构不会退化成 O(n·depth)。
- **深度上限 64**：防环（`A.parent = B, B.parent = A` 这种数据错误在脚本里可能出现）。
- **`p != id` 检查**：直接阻断自引用。

法线矩阵用 `Mat3::from_mat4(world).inverse().transpose()`——逆矩阵奇异时 `glam` 会产出 NaN/Inf，这是已知风险，当前由"缩放不为 0"的使用约定兜住（`Transform::normal_matrix` 里有 `max(1e-6)` 保护，但 Scene 里的世界法线矩阵没有，属可加固点）。

**文件加载**：

`SceneFile` 的每个字段都是 `Option` 并带 `#[serde(default)]`，因此最小合法场景就是 `{}`——任何字段缺失都退化为默认值，不会解析失败。`from_file` 的流程是：先造空 `Scene` → 应用 `background` / `camera` / `light`（光照方向做零向量保护）→ 逐个 `spawn(objects)` → 最后 `update_world_transforms()`。

### 4.5 Camera

```rust
pub struct Camera {
    pub position: Vec3,  // 默认 (4, 3, 6)
    pub target: Vec3,    // 默认原点
    pub up: Vec3,        // 默认 +Y
    pub fov_deg: f32,    // 默认 60
    pub znear: f32,      // 默认 0.05
    pub zfar: f32,       // 默认 500
    pub aspect: f32,     // 每帧由渲染器覆盖为真实窗口比例
}
```

**投影矩阵必须用 DirectX 风格**：`glam::camera::rh::proj::directx::perspective`。原因是 wgpu 的 NDC 深度区间是 `[0, 1]` 而不是 OpenGL 的 `[-1, 1]`。用错会导致深度全部被裁剪或远近颠倒——这是 wgpu 移植最常见的坑，代码中有显式注释。

`orbit(yaw_deg, pitch_deg)` 实现轨道控制：把 position 相对 target 的偏移转为球坐标，加角度后 `pitch` 钳制在 ±1.5 rad（约 ±86°），避免在极点处翻转。滚轮推拉在 `app.rs` 里做，半径钳制在 `0.8 ~ 80.0`。

`aspect` 是唯一由外部每帧写入的字段：`renderer.prepare_instances` 中 `camera.aspect = self.aspect()`，因此在场景 JSON 里写 aspect 无意义。

### 4.6 光照模型

```rust
pub struct Light {
    pub direction: Vec3,       // 默认归一化的 (-0.45, -0.8, -0.4)，即光线传播方向
    pub color: Vec3,           // 默认 (1.0, 0.97, 0.92) 略偏暖
    pub intensity: f32,        // 默认 1.0
    pub ambient: Vec3,         // 默认 (0.18, 0.19, 0.23) 略偏冷
    pub shadow: ShadowSettings // 阶段 D：方向光阴影配置（见 5.8）
}

pub struct ShadowSettings {
    pub enabled: bool,      // 默认 true
    pub size: u32,          // 阴影贴图边长，默认 2048，夹到 [64, 8192]
    pub ortho_size: f32,    // 正交相机半宽，0 = 按场景包围球自适应（默认 0）
    pub bias: f32,          // 深度偏移，默认 0.0015，夹到 [0, 0.05]
    pub normal_bias: f32,   // 法线方向偏移（纹素），默认 1.5，夹到 [0, 8]
    pub distance: f32,      // 光源后退距离倍数，默认 2.0，夹到 [0.05, 16]
    pub strength: f32,      // 阴影强度 0..1，默认 1.0
    pub cascade: CascadeSettings, // 扩展十六：级联（count / lambda / blend / max_distance）
}

pub struct CascadeSettings {
    pub count: u32,         // 级联数 1..=MAX_CASCADES(4)，默认 1（单级 = 老行为）
    pub lambda: f32,        // 切分混合：0 = 均匀、1 = 对数，默认 0.6，夹到 [0, 1]
    pub blend: f32,         // 级间过渡带宽（占该级跨度比例），默认 0.1，夹到 [0, 0.5]
    pub max_distance: f32,  // 阴影最远距离，0 = 用相机 zfar（或场景包围球最远点）
}
```

方向光是唯一的**全局**光源；扩展十五起另有最多 16 盏**局部光源**（点光 / 聚光，见 5.10），
它们只影响直接光项；扩展十八 C 起其中**每帧最多 4 盏**可以投影（点光 6 面 / 聚光单面，共 24 层深度数组，见 5.12）。
方向光的阴影最多 4 级级联共用一张 `texture_depth_2d_array`（层 = 级数），与局部阴影是两张独立的图、两段独立的 pass。
`direction` 的语义是**光线行进方向**（从光源射出），不是"指向光源的方向"——着色器里取 `normalize(-light_dir)` 得到表面指向光源的向量。

`ShadowSettings::sanitized()` 负责把越界值夹回可用区间：这份数据会被场景 JSON、脚本 API、CLI 三处写入，
任何一处传 NaN / 负数 / 超大分辨率都不能让渲染器 panic 或悄悄失效。阴影相机的拟合逻辑在
`Light::shadow_view_projection(center, radius)`：光源挂到场景包围球外，正交半宽取 `ortho_size` 或 `radius × 1.05`，
因此光照换向时不需要重建任何包围盒数据。

---

### 4.7 材质模型（PBR 参数与 CPU 参考实现）

```rust
pub struct SurfaceMaterial {
    pub metallic: f32,       // 0 = 绝缘体（塑料 / 石头），1 = 纯金属（漫反射归零）
    pub roughness: f32,      // 0 = 镜面，1 = 完全漫反射；下限 MIN_ROUGHNESS = 0.045
    pub emissive: [f32; 3],  // 线性 HDR，只加不减，上限 MAX_EMISSIVE = 16
}

pub struct GeoTerms {        // 四个点积打包，避免 direct_light 参数爆炸
    pub n_dot_l: f32,
    pub n_dot_v: f32,
    pub n_dot_h: f32,
    pub v_dot_h: f32,
}
```

设计要点：

1. **物件级材质是"覆盖层"，不是"完整材质"**。`Node.material` 是 `Option<SurfaceMaterial>`：
   `None` 表示沿用网格资产自带材质（FBX / glTF 导入的 `MaterialData`），`Some` 表示在资产材质之上覆盖
   metallic / roughness / emissive（`pbr::MaterialData::overlay`）。因此材质只描述"表面怎么反射"，
   base color 与贴图仍由资产层提供——一份材质能被多个不同颜色的物件复用，材质键也只取决于这三个旋钮。
   脚本里 `material_of` 对 `None` 物件返回**缺省值快照**（`metallic 0` / `roughness 0.6` / 无自发光），
   这是"读不到资产值"的刻意取舍：脚本不感知资产层，避免同一 API 在不同物件上给出不一致的语义。
2. **收敛发生在写入前，不在着色器兜底**。`SurfaceMaterial::sanitized()` 把 `metallic` 夹到 0..1、
   `roughness` 夹到 `0.045..1`、`emissive` 每通道夹到 `0..16`，NaN 退回缺省值。场景 JSON、脚本 API、
   编辑器 RPC 三条写入路径共用 `set_material_params` / `set_emissive`，非法值进不了场景。着色器里再
   `clamp` 一次只是第二道防线。
3. **CPU 是纯函数参考实现，GPU 照抄公式**。`cook_torrance_brdf` / `distribution_ggx` / `geometry_smith` /
   `fresnel_schlick` 都是不依赖 wgpu 的纯函数，因此能在无 GPU 环境跑单测：白炉半球积分反射率 ≤ 1、
   金属度 1 时漫反射为零、退化输入（NaN / 0 / 负值）保持有限。`shader.wgsl` 里的同名函数与之一一对应，
   语义漂移会被这些测试卡住。
4. **`GeoTerms` 是为了接口可读性**。早期把四个点积平铺成参数时，`direct_light` 有 9 个参数——
   既触发 clippy `too_many_arguments`，调用点也看不出哪个 `1.0` 是 `n·l`、哪个是 `v·h`。打包后
   `direct_light(base, material, geo, color, intensity, shadow)` 一眼能读出含义。
5. **材质键用 `f32::to_bits` 精确比较**（`r3e-render::pbr::SurfaceKey`）。参数写入前已收敛，
   `-0.0` 也被归一成 `0.0`，所以精确比较是安全的；换成近似比较反而会把本应同批的物件拆开。
   代价是"语义相同但位不同"的参数会被当成两个材质（多一份 48 字节 uniform），这是刻意选择。

### 4.8 环境探针（IBL，扩展十二）

**不做 HDR 立方体贴图的原因**：现阶段没有产 cubemap 的资产管线，IBL 预过滤需要 compute 或多 pass，
成本高于收益。改用**解析环境 + 球谐（L2，9 项）**：天空三色（天顶 / 地平线 / 地面）给出低频方向感，
一个太阳方向给出金属里那团亮斑。金属的高频反射留给后续的反射探针。

```rust
pub struct EnvironmentSettings {
    pub sky_zenith: [f32; 3],   // 天顶辐亮度
    pub sky_horizon: [f32; 3],  // 地平线（天与地的过渡都经过它）
    pub ground: [f32; 3],       // 下半球
    pub sun_dir: [f32; 3],      // 指向太阳，写入前归一化
    pub sun_color: [f32; 3],
    pub sun_intensity: f32,     // 0 = 阴天
    pub intensity: f32,         // 美术旋钮，整体乘数
}
```

1. **`scene.environment: Option<EnvironmentSettings>`，`None` = 完全关闭**。此时漫反射退回
   `light.ambient` 常数项，画面与扩展十二之前逐像素一致；`Some` 时环境项**替代**（而非叠加）ambient。
   这是"老场景不改行为"的结构性保证。
2. **三处写入路径共用 `sanitized()`**：场景 JSON、脚本 `set_sky` / `set_env_sun` / `set_env_intensity`、
   编辑器 `scene.set_environment`。收敛口径与材质同一套：颜色夹 `0..=16`（`MAX_ENV_RADIANCE`）、
   `intensity` 夹 `0..=8`、`sun_dir` 归一化（零向量回退正上方）、NaN 回退缺省。
3. **球谐投影在 CPU 侧完成，是纯函数**（`env::Sh9::project`，默认 2048 采样）。所以能吃单测：
   投影确定性、天空亮于地面、太阳项方向性、`env_brdf_approx` 与着色器逐项对齐。
   着色器不做积分，只做 9 项系数加权。
4. **投影结果按配置位比较缓存**：`EnvironmentSettings` 的等值判断按 `f32` 位比较，配置没变就不重投。
   `renderer.environment_builds()` 暴露重投次数——实机跑 2 秒上百帧，该值稳定为 1。
5. **太阳项与环境项解耦**：主方向光负责阴影与直接高光，环境里的太阳只影响环境项。
   两层光照叠加后金属才既"有影子"又"暗部不死黑"。
6. **`CameraUniform` 从 128 涨到 352 字节**（环境 5 项 + 9 项球谐 = 14 个 `vec4`），仍 16 字节对齐。
   启用位编码在 `env_horizon.w`（0/1），着色器据此单分支切换，不额外传 flag。

**已知边界**：环境不参与阴影判定（影子边缘的环境亮度不随阴影变暗）；没有镜面反射探针，
金属里映不出场景物件，只能映出天 / 地 / 太阳。

### 4.9 HDR 后处理链（扩展十三）

扩展十二把光照换成了 HDR 辐亮度，但渲染色目标仍是 8 位 sRGB 表面：超过 1.0 的线性亮度
在写入时被直接切平，太阳盘、自发光、金属高光统统糊成一片死白，动态范围红利被浪费。
**本扩展只解决"接住"**：常开一个 `Rgba16Float` 离屏目标，主通道改写成线性 HDR，
再由一条后处理链压回可显示范围。

```text
主通道 ──▶ Rgba16Float HDR 目标（线性）
           ├─(SSAO，扩展十七) 深度 → 遮蔽图(R8) → 4×4 散焦 → composite 乘回
           ├─(bloom)  亮部提取 → 1/2 → 1/4 → 1/8 → 逐级加法回叠
           ▼
         composite: hdr + bloom·intensity → 曝光(2^ev) → 色调映射 → FXAA ──▶ sRGB surface
           ├─▶ 文本通道（Load）
           └─▶ UI 通道（Load）
```

1. **HDR 目标是常开的，`postfx` 段缺省为「直通」**。`Scene::postfx: Option<PostFxSettings>`，
   `None` 等价于 `PostFxSettings::default()`（曝光 0、直通曲线、无泛光、无 FXAA）。
   `is_neutral()` 为真时泛光链路整体跳过，composite 只剩一次 clamp——**老场景画面逐像素不变**。
   这也意味着离屏目标的显存常驻（`Rgba16Float` 每像素 8 字节），这是换取"任何场景都能吃到 HDR"的代价。
2. **公式的权威实现在 `r3e-core::postfx`**（零 wgpu 依赖的纯函数：`expose` / `bright_pass` /
   `tonemap` / `fxaa_edge_visible` / `resolve`），`postfx.wgsl` 只是把同样的公式搬到 GPU。
   两边用 `tests/postfx_shader.rs` 做常量比对（ACES 五系数、亮部阈值、Rec.709 权重、FXAA 两个阈值），
   改一边不改另一边会直接红。
3. **泛光是固定三级（1/2、1/4、1/8）**：亮部提取一次 + 降采样两次 + 上采样两次 + composite 一次，
   单帧 pass 上限 `MAX_PASSES_PER_FRAME = 6`（`tests/postfx_contract.rs` 锁住预算）。
   再细的层级在小窗口上会退化成一两个像素，只会把光晕抖成噪点。
4. **软膝亮部提取**：`knee = 0` 走硬阈值分支（`KNEE_EPSILON` 兜除零），否则在阈值上下各 `knee`
   的过渡带里做平滑，避免光晕边缘出现台阶。
5. **文本 / UI 放在 composite 之后**：这两个通道的顶点色是 LDR 用户数据，不该被曝光与 ACES
   二次调色；放在前面会让 HUD 随场景曝光整体漂移。
6. **三处写入路径共用 `PostFxSettings::sanitized()`**：场景 JSON、脚本 API、编辑器
   `scene.set_postfx`。口径：曝光夹 `[-8, 16]` EV、泛光阈值夹 `[0, 64]`、软膝 `[0, 8]`、
   强度 `[0, 4]`，NaN 一律回落缺省值（不是 0——0 阈值会把整幅画面当亮部）。
7. **色调映射名字的合法性由宿主判定**：`r3e-editor::PostFxPatch` 只带字符串，传输层不认识
   `r3e_core` 的任何类型；`LocalEngineApi` 用 `TonemapMode::from_name` 解析，认不出就报
   invalid params（不做静默回落）。脚本侧则相反——脚本是热重载的，未知名回落 `none` 并打印一行，
   免得一个拼写错误让整个脚本执行失败。

**已知边界**：无运动模糊、无时间性抗锯齿（TAA），FXAA 是纯后处理
（边缘判定在 composite 里做，无法恢复已经丢失的几何信息）；泛光层级与核大小固定，
不随分辨率自适应；曝光是手动旋钮，没有基于平均亮度的自动曝光。
遮蔽不在本链内，见 4.10（扩展十七）——它排在 composite **之前**，属于「喂给后处理的 AO」。

### 4.10 屏幕空间遮蔽（SSAO，扩展十七）

阴影解决了「直接光照不到的地方」，但环境项（天空 / 球谐漫反射）是**无遮挡**的常量项：
墙角的接缝、台阶与地面的夹角、两块贴着的板之间，亮度与开阔平地完全一样，读起来像纸片拼图。
屏幕空间遮蔽用主通道的深度图反推法线，在半球核里数遮挡比例，把结果当环境项的折扣乘回去。

**链路**

```text
主通道（写 HDR + Depth32Float）
   │
   ├─ pass 1：深度 → AO（R8Unorm，全屏三角形 + 16 样本半球核）
   ├─ pass 2：AO → AO（4×4 盒式散焦，抹掉逐像素旋转带来的椒盐噪声）
   ▼
composite：color *= AO.r ──▶ 曝光 → 色调映射 → FXAA
```

1. **公式的权威实现在 `r3e-core::ssao`**（零 wgpu 依赖）：`hemisphere_kernel`（确定性半球核，
   `hash_u32` 打散旋转）、`reconstruct_view_pos`（由深度与逆投影重建视空间坐标）、
   `normal_from_neighbors`（邻域深度差打分算法线）、`is_occluded`、`ao_from_occlusion`。
   `ssao.wgsl` 只是把这些公式搬到 GPU，参数与常量逐条对应。
2. **`SsaoSettings` 的 `sanitized()` 三处入口共用**：场景 JSON、脚本 API、编辑器 `scene.set_ssao`。
   口径：半径夹 `[0.01, 8]` 世界单位、偏置夹 `[0, 0.5]`、强度夹 `[0, 4]`、样本数夹 `[4, 16]`（默认 8）。
   样本数比 4 更少时核退化成几个方向、能看出条纹；比 16 更多时收益被 4×4 散焦抹平，白花采样。
3. **缺省中性**：`SsaoSettings::default()` 的 `enabled = false`，`is_neutral()` 为真时两个 pass 整段跳过、
   composite 拿到一张全 1 的遮蔽图——**不写 `postfx.ssao` 的老场景画面逐像素不变**。
   **但"全 1"必须真的写进纹理**：`create_ao_placeholder` 建的是 1×1 `R8Unorm`，纹理创建后内容未定义
   （多数后端给 0），早期实现只建不写，于是 composite 的 `color *= ao` 把整帧（含背景清除色）乘成纯黑，
   窗口出图全黑而 `RenderStats` 一切正常——这是扩展二十修掉的那个 bug，修法是在建纹理后
   `queue.write_texture` 一笔 `[255u8]`，`PostFxPass::new` 因此多收一个 `&wgpu::Queue`。
4. **`AO_FORMAT = R8Unorm`**：遮蔽是单个乘数，8 位足够；换成 `R16Float` 只会多一倍带宽。
   主深度纹理因此必须带 `TEXTURE_BINDING` 用法——缺这一条时绑定组在真机上校验失败，
   而纯文本测试察觉不到（这条是首次真机运行时才炸出来的）。
5. **AO 图有两份**：pass 1 写 A、pass 2 读 A 写 B，散焦后把 B 交给 composite。
   同一张图既读又写会触发 wgpu 的读写冲突（阴影通道踩过同一个坑，见 5.9）。
6. **每帧重算，没有历史累积**：相机一动遮蔽就要重算，靠「散焦 + 16 样本」而不是「时间累积」压噪。
7. **世代号（`SsaoPass::generation`）**只在遮蔽图按新尺寸重建时 +1，composite 的绑定组据此判等复用。
   没有它的话每帧都会重建一个绑定组——探针里「世代逐帧增长」就是这条失效的告警信号。

**代价**：两个全屏 pass（1080p 各约 2M 像素）+ 每像素 16 次深度 `textureLoad`。
实测开启后仍在满帧（集显未测）；关掉遮蔽段后 pass 数为 0，成本归零。

**已知边界**（都写进了「不打算做」的清单，而不是「待修 bug」）：

- **AO 乘在整幅画面上**（composite 里 `color *= ao`，见 `postfx.wgsl`），不是只乘环境项。
  正确做法是把 AO 单独送进 PBR 的环境项分支，但 composite 拿不到逐像素的法线与材质，需要
  多传一张 G-buffer 或在主通道里出图——代价远大于当前收益，先按近似接受：遮挡区整体压暗，
  直接光也被压一点点，视觉上是「缝隙更脏」，不细看分不出。
- **屏幕空间信息缺失处的伪影**：物件边缘外侧没有深度（背景是天空/远平面），采样会漏；
  `reconstruct_view_pos` 在远平面返回 1.0（不遮挡），法线退化时也返回 1.0，宁可漏遮不误遮。
- 不做双边滤波，4×4 盒式会在**深浅交界的台阶边缘**溢出一点灰边；半径调小可缓解。
- 没有多尺度（MSSAO / HBAO+ 那种），大幅提升质量需要分层深度 Mip，本期不做。

### 4.11 动画混合树（扩展十八 A）

一个 `AnimInstance` 只承载**一条主片段**（外加切片段时的交叉淡入），这足以表达 idle → run → atk
这类互斥动作，但表达不了「上半身挥砍 + 下半身走路」——后者要求**同时**播放多段动画并逐骨骼加权。
混合树补的就是这一层：主姿态之上再挂若干**层**，每层有自己的片段、权重、遮罩、时间轴与叠加模式。

**分层模型**（`r3e-core::anim_blend`，零 wgpu / 零场景依赖）

| 结构 | 字段口径 |
| --- | --- |
| `BoneMask` | `include` / `exclude` 两条规则表，按**节点路径后缀逐段**匹配（大小写不敏感） |
| `AnimLayer` | `clips: Vec<LayerClip>`（层内按权重归一化混合）+ `weight` + `mode` + `mask` + 自己的 `time / speed / looping / playing` + `fade` |
| `LayerMode` | `Override`（默认）/ `Additive` |
| `LayerFade` | 层权重过渡：smoothstep 缓入缓出，与交叉淡入同一口径 |

求值顺序（`Scene::update_animations` 第二遍）：

```text
主片段（含与旧片段的交叉淡入）→ 逐层按挂载顺序叠加
  ├─ override：lerp(base, pose, w)
  └─ additive：base + (pose - reference) * w     // reference = 片段首帧
其中 w = 层有效权重（含过渡）× 遮罩权重（0 / 1）
```

1. **层内多片段共享同一条时间轴**：走 / 跑 / 冲刺按速度加权是同一个动作的不同强度，相位必须同步，
   所以一层只有一个 `time`；不同层各有独立时间轴（上半身攻击与下半身走路本来就不该同步）。
2. **`additive` 的参考姿态固定取片段首帧**（`sample.reference`），因此叠加的是「相对首帧的增量」
   而不是绝对姿态——呼吸、受击抖动这类动作正是「首帧为基准的小幅偏移」。
   代价是不能指定别的参考姿势（见 11.1）。
3. **遮罩是二值的**：`BoneMask::weight_for` 只返回 0 或 1，没有软过渡。
   半边骨骼硬切在低模上看不出来；要做「肩部软化」得把遮罩做成浮点权重场，本期不做。
4. **`exclude` 优先于 `include`**：`"arm,!arm/hand"` 表示「手臂参与、手不参与」，与直觉一致。
   规则只写尾段即可（`"hand"` 命中 `RootNode/Arm/Hand`），与 `AnimClip::track_of` 的路径口径一致，
   导入 FBX 时不必抄全路径。
5. **空遮罩 = 整副骨架，不是「都不作用」**：`BoneMask::pass_all()` 等价于不设遮罩。
   这是个刻意的选择——把空遮罩解释成「全不通过」会让忘传遮罩的层完全静默失效，比画错更难查。
6. **上限**：`MAX_ANIM_LAYERS = 8`（单实例）、`MAX_LAYER_CLIPS = 8`（单层）。
   满了 `add_anim_layer` 返回 `false` 而不是顶掉已有层——游戏逻辑里「加不上」必须是显式信号。
7. **层不写回场景 JSON**：层是运行时状态，随实例生灭。热重载重放场景后实例重建、层清空，
   由脚本 `on_start` 重新挂（与状态机写在脚本层是同一个取舍，见 8.10）。
8. **无层时逐位等于老路径**：`inst.layers` 为空时整段跳过，交叉淡入的结果原封不动，
   因此老场景 / 老脚本的画面不变。
9. **遮罩串的解析不在 core**：core 只认 `BoneMask` 规则表；`"arm,!hand"` 这种串由
   脚本层 `parse_bone_mask` 与宿主层 `bone_mask_of` 各自解析（两边口径必须一致，各有单测）。

**已知边界**（写进 11.1 的「不做」清单）：

- 无 morph target（表情 / 口型），层只作用于骨骼 TRS。
- 无 IK / 无脚部对齐：走路滑不滑仍由片段与移动速度的匹配程度决定。
- `additive` 的参考帧不可指定（固定首帧），做「相对某个关键姿势的增量」需要扩接口。
- 层只能在末尾追加 / 按名删除，不能调整顺序；顺序即叠加顺序。
- 层内片段不能各自错相位（共享 `time`），做「每步触发一次受击抖」得靠层权重与时间手动控。

### 4.12 场景栈与存档（扩展十八 B）

前十七个扩展都建立在一个隐含前提上：**一个进程从头到尾只跑一个场景**。启动读一次 `scene.json`，
之后所有改动（脚本、RPC、热重载）都落在同一个 `Scene` 上。要做「主菜单 → 关卡 → 暂停设置 → 回关卡」，
缺的从来不是"再解析一份 JSON"（`Scene::from_json_str` 早就会），而是三件配套的东西：

| 缺的东西 | 落点 | 一句话 |
| --- | --- | --- |
| 来时的路 | `SceneStack` / `StackEntry` | 切走之后要能切回来，而且切回来是「接着那一局」，不是从头开始 |
| 运行时的账 | `RuntimeSnapshot` | `to_scene_file` 刻意只写**声明**，被它省略的 `time` / `frame` / 通道开关 / 动画进度在这里单独成一节 |
| 边界 | §11.1 的「场景」一行 | 同一时刻只有**一个**场景在跑；切换是**替换**，栈里存的是"离开时的样子"，不是活着的世界 |

**五个类型的分工**（全在 `crates/r3e-core/src/scene_stack.rs`：纯数据 + 纯函数，
唯一的 IO 是 `SceneArchive::save_to_path` / `load_path`，其余全是内存中的纯变换）：

| 类型 | 内容 | 关键约束 |
| --- | --- | --- |
| `RuntimeSnapshot` | `to_scene_file` **刻意不写**的那部分：`time` / `frame` / 文本 / UI / 局部光源三个通道开关 / `ui_above_text` / `anims` | 带 `version` 与 `is_compatible()`；`apply` 把快照写回场景并返回**成功恢复的动画实例数**（名字对不上就静默跳过）；不含相机 / 灯光 / 物件（它们在场景源里），也不含世界矩阵这类每帧重算的派生量 |
| `AnimRuntime` | 一条动画实例的运行时：`name` / `time` / `speed` / `playing` | 按 `name` 寻址；捕获时按名排序，序列化结果稳定、便于 diff |
| `SceneArchive` | 一份存档 = `format` + `version` + `scene`（`SceneFile` 声明）+ `runtime`（`Option<RuntimeSnapshot>`） | 单独一个类型、不往 `SceneFile` 加字段，为的是保住「`to_scene_file` 只写声明」那条被单测锁着的对称性；`runtime` 缺省即"只还原场景"；`data_only` = 另存为场景 |
| `StackEntry` | 一层 = `name`（展示名）+ `path`（来源路径，脚本 / 编辑器现场构建的场景为空串）+ `source`（源文本）+ `runtime`（离开时的快照） | `StackEntry::capture(name, path, &scene)` 一次性抄下源文本与快照；`to_json_pretty` 失败时退化为 `{}` 而不报错 |
| `SceneStack` | `entries` + `index` | 语义完全照抄浏览器历史：`push` 截断"前进"侧、满 16 层（`MAX_SCENE_STACK`）丢**最老**一层、`back` 只移游标不删层 |

**栈的语义**（每一条都有单测锁着，因为"回退到哪一层"是最容易写歪的地方）：

| 操作 | 行为 |
| --- | --- |
| `push(entry)` | 先截断当前层之后的层（"还没走的路"），再追加；超过 `MAX_SCENE_STACK` 时从队首丢最老一层；返回新层号 |
| `unique_name(base)` | 生成不重名的层名：`main` → `main#2` → `main#3`…（空串归 `scene`），保证面板里点得准 |
| `back()` | `index - 1`，已到 0 返回 `None`（**不移除**层，所以能再前进回去） |
| `forward()` | `index + 1`，已到末尾返回 `None` |
| `jump(i)` | 越界返回 `None`；否则移游标并返回该层 |
| `find(name)` | 从**当前层往回**找最近的同名层（往回找，所以"回主菜单"回的是上一个主菜单；当前层本身也匹配，切到自己即原地不动） |
| `switch_by_name(name)` | `find` + `jump` 的组合，供按名字回退 |
| `capture_current(&scene)` | 只刷新**当前层**的源文本与快照；栈为空时返回 `false` |
| `pop_current()` | 弹出当前层，游标落到前一层（没有前一层则清空），只在"真的销毁一层"时用 |

**命令信封 `SceneCommand`**：脚本与 RPC 都不能"直接换世界"（原因见 §8.16），只能申请一条命令。

| 变体 | 载荷 | `is_world_change` | 说明 |
| --- | --- | --- | --- |
| `Switch { path }` | 场景文件路径 | `true` | 换到另一个场景 |
| `Back` | — | `true` | 回退一层 |
| `SaveArchive { path }` | `Option<String>` | `false` | `None` = 让宿主挑默认位置 |
| `LoadArchive { path }` | 存档路径 | `true` | 从存档恢复（同样要先压栈） |

`label()` 给日志与错误回执用（`switch` / `back` / `save_archive` / `load_archive`），
`path()` 只对带路径的两个变体返回 `Some`，其余 `None`——这样宿主不用为"取路径"再写一遍 `match`。

**显式不做**：

- **不做并行场景**：没有"两个场景同时更新"，栈里的世界是冻结的源文本，不是后台还在跑的世界。
- **快照不含刚体位姿**：物理按场景**声明**重建（`rebuild_scene` 里 `clear_bodies`，`generation` 自增），动态刚体的位置靠脚本 `on_start` 重跑决定。
- **不接管热重载**：读档不修改 `scene_path`，热重载继续盯原来那个场景文件。
- **不做异步 / 进度条**：切场景在一帧内同步做完，大场景会有一次帧尖峰。

### 4.13 场景 JSON schema（P0-1）

场景的字段、取值范围、枚举选项、默认值此前只散落在本文档的散文与 `scene.rs` 的 serde 结构里，
生成式使用方（AI / 外部工具）只能照抄 `assets/scene_*.json` 示例。现在把它们做成**机器可读契约**：

| 项目 | 内容 |
| --- | --- |
| 契约文件 | `assets/scene.schema.json`（JSON Schema 2020-12，`$id` = `https://r3e.dev/schema/scene.schema.json`） |
| 生成方式 | `cargo run -p r3e-core --bin scene_schema_dump`（`--stdout` 只打印，`-- <路径>` 指定输出） |
| 来源 | `SceneFile` 及全部嵌套类型的 `schemars::JsonSchema` 派生，**不手写、不复制文档** |
| 代码入口 | `r3e_core::scene_schema::{scene_schema, scene_schema_pretty, SCENE_SCHEMA_PATH}` |

覆盖的结构：`SceneFile` / `CameraDesc` / `LightDesc` / `ShadowDesc` / `ObjectDesc` /
`PhysicsDesc` / `PhysicsSceneDesc` / `LocalLight` / `LocalLightKind` / `EnvironmentSettings` /
`PostFxSettings` / `BloomSettings` / `TonemapMode` / `SsaoSettings` / `SurfaceMaterial` /
`AssetDecl` / `AnimDecl` / `ScreenText` / `TextAlign` / `UiElement` / `UiKind` / `Anchor` /
`UiUnit` / `UiValue`。

两条守卫测试（`crates/r3e-core/tests/scene_schema.rs`）：

1. **防漂移**：磁盘上的 `scene.schema.json` 必须与派生结果**逐字节一致**；改了 serde 结构却没重新生成，
   测试直接红灯并打印首个差异行与重生成命令。这样「结构变更」与「契约变更」永远同步落地。
2. **交叉校验**：`assets/` 下 11 个演示场景全部必须通过该 schema 校验（含 1 个正例底线：空对象合法；
   另有负例自检确认校验器不是空跑）。校验器是测试内 ~130 行的极简实现，只覆盖 schemars 实际产出的
   关键字子集（`$ref` / `type` / `oneOf` / `anyOf` / `const` / `enum` / `properties` / `required` /
   `items` / `minItems` / `maxItems` / `minimum` / `maximum` / `additionalProperties`），
   不引入 JSON Schema 运行时依赖。

两个语义要点，schema 里也已表述：**所有字段都可省略**（省略即取引擎默认值）；**未声明的额外字段被 serde
静默忽略**，所以字段名拼错不会报错——判断"这个字段存不存在"要以 schema 的 `properties` 为准，而不是以
"加载没报错"为准。

### 4.14 材质数据贴图在 core 侧的契约（扩展二十一）

扩展二十一让材质支持**法线 / 粗糙度 / 遮蔽**三张数据图，但 core 只负责两件不碰 GPU 的事：给顶点补切线基，
把导入层的槽位名收敛成语义。渲染侧怎么采样见 §5.14。

**① 切线（`tangent.rs`）**：`Vertex.tangent` 是 `[f32; 4]`——xyz 是切线，**w 是手性**（±1），
法线贴图的副切线按 `bitangent = cross(normal, tangent.xyz) * tangent.w` 现算，不单独存。之所以不存副切线：

- 顶点从 56 涨到 72 字节换一个可以直接叉乘出来的向量，不划算；w 用 ±1 承载手性，精度足够且正好塞满 `Float32x4`。
- `compute_tangents` 按三角形遍历：位置边 `e1/e2` 与 UV 边 `duv1/duv2` 求每三角形的切线与副切线后累加到三个顶点，
  再对每个顶点做 Gram-Schmidt（`t -= n * dot(n, t)`）并归一化。累加而不是取"第一个用到该顶点的三角形"，
  是为了让共享顶点处的切线是周边三角形的平均——单三角形取法在球面 / 圆柱侧面这种高密度网格上会出现切线跳变，
  法线贴图表现为一条条细缝。
- **退化必须显式处理**：UV 面积为 0（所有三角形挤在同一条 UV 线上，胶带面 / 程序化平面常见）时该三角形跳过；
  顶点累加结果为 0（整网格无有效 UV 面积）时退回"与法线垂直的任意正交基"（挑法线绝对值最小的轴叉乘），
  而不是归一化一个 0 向量得到 NaN。NaN 切线会在着色器里传染成整片黑，比"切线不准"严重得多。
- 内置程序化网格**不跑**这段重算：`Vertex::new` 给的缺省切线是 `DEFAULT_TANGENT = (1, 0, 0, 1)`，
  与几何法线未必正交。着色器对此有兜底（Gram-Schmidt 后再判长度，取与法线最不平行的轴重建，见 §5.14），
  因此内置网格挂法线贴图不会 NaN，只是切向随缺省值走。真正需要正确切线的路径是导入资产——
  `MeshAsset::build` 装配网格后立刻 `recompute_tangents()`，也正因为重算发生在**装配时而不是首次绘制时**，
  切线才进得了顶点缓冲、不需要额外 GPU pass。

**② 槽位分类（`texture_slot.rs`）**：FBX 的 `DiffuseColor` / `NormalMap` / `Bump` / `ReflectionFactor`、
glTF 的 `baseColorTexture` / `metallicRoughnessTexture` / `occlusionTexture` / `normalTexture` 说的是同一件事
却不同名。`classify_slot(name)` 把槽位名（含路径分隔符与常见前缀后缀）**归一成 `TextureSlot` 枚举**
（`BaseColor` / `Normal` / `MetallicRoughness` / `Occlusion` / `Emissive` / `Unknown`），
导入层据此决定"这张图能不能当法线图用"，渲染层据此决定"用 sRGB 还是线性格式上传"。
两条硬口径：

- **数据图一律线性，颜色图一律 sRGB**。法线 / 粗糙度 / AO 存的是物理量，按 sRGB 上传会被 gamma 曲线掰弯，
  表现为法线贴图的平整面也泛蓝、AO 暗部整体偏亮。判定只写在 `TextureSlot::is_linear_data()` 一处，
  asset（挑图）与 render（选 GPU 格式：`Rgba8Unorm` vs `Rgba8UnormSrgb`）共用，避免两边各写一份 `matches!` 后漂移。
- **分类失败不猜**，也**不因为认不出就丢图**：`Unknown`（含 FBX 的 `bump` 高度图——按法线采样会得到错误方向）
  不参与法线 / 粗糙度 / AO 槽位，只保留 base color 兜底匹配能力。错把漫反射图当法线图的画面比"没有法线贴图"更糟且更难排查。
- **同一用途只取第一张**：`pick_pbr_textures` 按扁平表顺序取首个命中，不做"多张同名图混合"这种没人能预测的行为。

### 4.15 遮挡剔除在 core 侧的契约（扩展二十二）

视锥剔除（`frustum.rs`）只能扔掉「镜头外」的物件；镜头里被前面的墙完全挡住的物件照样进实例缓冲、
跑完整条顶点 + 片元流水线，最后一个像素不剩地被深度测试丢掉。扩展二十二把这类「画了也看不见」的物件提前摘掉，
core 侧只负责**判定**（金字塔是纯 `Vec<f32>`、判定是纯算术、零 wgpu），金字塔从哪来见 §5.15。

**为什么是 CPU 参照版而不是 GPU 遮挡查询**：`wgpu` 的 occlusion query 要建 query set、要等 resolve 读回，
而且结果只能用于下一帧——收益在物件量不大时被查询自身的开销吃掉。这里改成「读上一帧深度 → 压金字塔 → 纯 CPU 判」：
每条判定的成本是几十次 `f32` 比较，可以被单测逐条钉住（28 条），也不用为 query 池做资源配额。

**① 金字塔（`DepthPyramid::build`）**：把上一帧 `width × height` 的 NDC 深度按 `tile × tile` 分块，
每块只留**最远**深度。判定要回答的问题是「这块屏幕区域里有没有比物件更近的表面」，
最远值就是这个界限；存完整层级反而不是这个问题的答案。

| 常量 | 值 | 口径 |
| --- | --- | --- |
| `DEFAULT_TILE_SIZE` / `MIN_TILE_SIZE` / `MAX_TILE_SIZE` | 8 / 2 / 32 | 8 让 640×360 压成 80×45；1 像素 tile 等于不压缩，32 像素在 1080p 下只剩 60×34、遮挡关系被糊掉 |
| `DEFAULT_DEPTH_BIAS` / `MAX_DEPTH_BIAS` | 0.05 / 0.5 | **相对剩余深度余量**的比例，见下 |
| `DEFAULT_MARGIN_PX` / `MAX_MARGIN_PX` | 2 / 64 | 粗筛往外长一圈抵消投影取整误差；外扩越大越保守（越不剔除） |
| `DEFAULT_MAX_TILES` / `MIN` / `MAX` | 256 / 16 / 4096 | 单次判定最多遍历的 tile 数，超了就换更粗的一层 |
| `MAX_PYRAMID_LEVELS` | 8 | 每层边长翻倍，8 层足以从 2 像素 tile 收敛到 1 个 tile |

**② 深度偏置是相对比例，不是绝对差值**。透视投影的 NDC 深度在远处被压得极扁：相机在 12 米处时，
物件与墙面只差 `1e-4` 量级，而近处两个相邻表面能差 `0.1`。所以偏置按 `(1 - nearest)` 缩放——
`threshold = nearest - bias × (1 - nearest)`，等价于「视空间深度的相对误差」。
**这条不是可调口味而是正确性前提**：实测固定 `0.01` 的绝对偏置会让远处所有候选全部判可见（一个都剔不掉），
`is_occluded_works_far_from_camera` 就是钉这件事的回归测试。

**③ 两阶段判定**（`test_aabb` 包一层 `project_aabb` + `is_occluded`）：

| 阶段 | 参数 | 语义 |
| --- | --- | --- |
| 粗筛 | 用户设置（含 `margin_px` 外扩、`depth_bias`） | 8 个角点投影出包围矩形与最近深度；矩形覆盖的 tile **全部**满足「最远 ≤ 阈值」才判遮挡 |
| 复核 | `OcclusionSettings::review()`：`margin_px = 0`、`depth_bias × 4` | 对粗筛判遮挡的候选收紧再测一次；两次都过才返回 `Occluded` |

`OcclusionVerdict` 三态是刻意的：`Visible`（金字塔不可用 / 可能露出来）、`Occluded`（真剔除）、
`Rejected`（粗筛过、复核没过 → **疑罪从无**回退绘制，但计入统计）。`is_culled()` 只认 `Occluded`，
所以「复核挡住的那部分」永远还有统计口径可查，调参时不会变成黑盒。

**④ 保守性约定（宁可多画，不可让物件凭空消失）**：

- 金字塔为空 / `is_valid()` 为假 / 尺寸与当前画面对不上（首帧、刚 resize）→ 一律 `visible`；
- AABB 8 角点里只要有 `w <= 0`（跨近平面 / 在相机背后）→ `project_aabb` 返 `None`，调用方按可见处理；
- 矩形退化成一条线 / 一个点，或完全落在屏幕外 → 交给视锥判定，这里不剔；
- 深度图里的非有限值（NaN / inf，多半是没写过深度的像素）在 `build` 时按 `1.0`（最远）处理——
  把它们当遮挡物才会出事；
- `enabled = false` 时 `is_neutral()` 为真、`test_aabb` 直接返回 `Visible`，与「只做视锥剔除」逐像素一致。

**⑤ 场景契约**：`SceneFile` / `Scene` 的 `occlusion: Option<OcclusionSettings>` 段可省略；
缺省 `OcclusionSettings::default()` 是 `enabled = false` + 上述四个缺省参数，且 `default_occlusion_is_not_serialized`
保证默认值不会被写回场景文件。载入与脚本改参都过 `sanitized()`：`NaN` 偏置回落缺省，其余夹进各自区间。

**测试口径**（`crates/r3e-core/src/occlusion.rs` 28 条 + `crates/r3e-core/tests/occlusion_scene.rs` 8 条）：
设置缺省 / 收敛 / `review()` 严格性 / JSON 往返；金字塔拒短输入与退化尺寸、tile 夹取、取最远值、非有限值当最远、
逐层收敛到 1 tile；遮挡判定的「墙在物件前为真、物件在墙前为假」、偏置生效、**远处仍然有效**、任一 tile 空即可见、
屏外可见、超 `max_tiles` 换粗层；投影的居中 / 跨近平面 / 退化输入；场景段的缺省关闭、局部覆盖、非法值收敛、
默认不序列化、脚本开关与偏置。

**已知边界**

- **滞后一帧以上**：判定用的是上一份金字塔（读回本身也低频，见 §5.15），快速甩镜时会出现「刚转过来还没剔 / 已画完才剔」；
- **金字塔替不了逐像素解析**：tile 里只有一个最远值，细长物、栅栏、树叶这类「几乎透空」的遮挡体效果好不了；
- **只按 AABB 判定**：不做背面 / 包围球收缩，长条 AABB 的物件容易被保守放过；
- **不做遮挡体筛选**：没有「用上一帧已绘制的物件当遮挡体」这套逻辑（那是 GPU 侧 query 的强项）；
- **半透明物件不参与**：`renderer` 只剔除不透明批次（半透明不写深度，判了也不准），见 §5.15；
- **无跨帧稳定性机制**：被剔的物件下一帧可能又回来，未做「连续 N 帧遮挡才剔」的迟滞。

### 4.16 HDR 环境探针在 core 侧的契约（扩展二十三）

扩展十二的解析天空只有天顶 / 地平线 / 地面三色加太阳，方向感有了但**细节没有**——金属球映不出任何物件。
扩展二十三把「离线预滤波的 HDR 环境立方体图」做成 core 侧的纯数据契约：Radiance `.hdr` 解码 → 重采样成立方体 →
按粗糙度预滤波成 mip 链，全程零 wgpu、零文件 IO、可单测；渲染层只接收成品（见 §5.16）。

**① 常量**

| 常量 | 值 | 含义 |
| --- | --- | --- |
| `CUBE_FACES` | 6 | 立方体面数 |
| `DEFAULT_CUBE_FACE_SIZE` | 128 | 缺省面边长（像素） |
| `MAX_CUBE_FACE_SIZE` | 1,024 | 面边长上限 |
| `DEFAULT_SPECULAR_MIPS` | 6 | 缺省镜面 mip 层数 |
| `MAX_SPECULAR_MIPS` | 10 | 镜面 mip 层数上限 |
| `PREFILTER_SAMPLES` | 32 | 预滤波每像素采样数 |
| `MAX_EQUIRECT_PIXELS` | 64 Mi | 等距柱状图总像素上限（防坏文件撑爆内存） |

**② 解码**：`decode_radiance` 只认 Radiance RGBE（`32-bit_rle_rgbe` 的新旧两种扫描行编码），
配合 `linear_to_rgbe` 可做往返；`width × height > MAX_EQUIRECT_PIXELS` 或任一维为 0 直接返 `None`，
**不 panic 也不预分配巨块**——坏文件的表现被收敛为「探针加载失败，退回解析 IBL」。

**③ 重采样与预滤波**

- `equirect_to_cubemap(image, face_size)`：方向驱动的双线性（`HdrImage::sample_dir`，经度环绕、纬度夹到极点），
  `face_size` 先夹 `[4, MAX_CUBE_FACE_SIZE]` 再**向上取 2 的幂**（面必须 2 的幂才能逐层减半）；
- `prefilter_specular(base, mip_levels)`：第 0 层直接复用输入图（保清晰反射，不再卷积），第 `level` 层按
  `roughness = level / (levels - 1)` 做**余弦幂重要采样**卷积——权重指数由 GGX 的 `α = r²` 反解成 Phong 指数
  `k = 2/α² - 2`（`r → 1` 时 `k → 0`，退化成余弦加权半球），采样数固定 `PREFILTER_SAMPLES = 32`，
  用「分层 `u1` + 黄金角错开 `φ`」压掉同一方位上的条纹；
- 层数取 `mip_levels.clamp(1, MAX_SPECULAR_MIPS).min(face_size.trailing_zeros() + 1)`——保证每层至少 1 像素，
  配置写得再大也不会建出空层；
- `build_probe(image, face_size, mip_levels)` 把两步串起来；`PrefilteredEnv::roughness_of` / `mip_for_roughness`
  是层号与粗糙度的互逆映射，与渲染侧 `envprobe::mip_lod` 同一条公式（GPU 在相邻两层间做三线性插值）。

**④ 方向映射三个纯函数**：`equirect_uv`（方向 → 等距柱状 uv）、`cube_face_dir`（面 + uv → 方向）、
`cube_dir_face`（方向 → 面 + uv，与前者互逆，含边角取整的夹取）。`CUBE_FACE_NAMES = ["+X", "-X", "+Y", "-Y", "+Z", "-Z"]`
钉死面顺序，六面图输入按这个顺序对位。

**⑤ 设置与收敛**（`EnvironmentMapSettings`，`sanitized()` 幂等）

| 字段 | 缺省 | 收敛规则 |
| --- | --- | --- |
| `enabled` | `false` | — |
| `equirect` | `None` | 去首尾空白，空串视为「没写」 |
| `faces` | `None` | **必须恰好 6 条且条条非空**，否则整段作废（半套面拼不出 cubemap，宁可明确失效也不猜缺哪面） |
| `face_size` | 128 | 夹 `[4, MAX_CUBE_FACE_SIZE]` 并向上取 2 的幂 |
| `mip_levels` | 6 | 夹 `1..=MAX_SPECULAR_MIPS` |
| `intensity` | 1.0 | `NaN` 回落缺省，其余夹 `0..=env::MAX_IBL_INTENSITY`（与扩展十二共用强度上限） |

`is_active()` = 开关打开**且**至少一个来源可用；`source_label()` 给探针日志读（`disabled` / `faces` / `equirect` / `none`）；
`is_default()` 决定场景保存时是否省略整段。

**⑥ 场景段**：`Scene` / `SceneFile` 的 `environment_map: Option<EnvironmentMapSettings>` 可省略；载入、
脚本写入、RPC 写入三条路径都过 `sanitized()`；没配探针时**不会凭空长出** `environment_map` 段
（与扩展十二的 `environment` 同一套「缺省即省略」往返口径）。

**测试口径**（`crates/r3e-core/src/envmap.rs` 22 条 + `crates/r3e-core/tests/environment_map_scene.rs` 4 条）：
RGBE 往返与坏文件拒收、像素上限、方向映射三函数互逆与边角夹取、面顺序、等距柱状重采样的中心/极点取值、
预滤波层数收敛、`r = 0` 层逐像素等于原图、粗糙层方差单调下降、层号↔粗糙度互逆、
设置收敛（空串 / 半套面 / 越界面边长 / NaN 强度）、段缺省不序列化与往返。

**已知边界**

- **静态探针**：不做运行时抓取场景的反射捕捉，也没有平面反射 / 屏幕空间反射——物件动起来不会出现在金属上；
- **单探针全局生效**：一张立方体图管整个场景，没有区域化 / 分房间探针，也没有探针混合；
- **只认 Radiance RGBE**：不读 EXR / PFM / DDS；六面图必须是同尺寸等距结构，不支持逐面不同分辨率；
- **预滤波是 CPU 串行**：每层每像素 32 次立方体采样（128² + 6 层 ≈ 千万级采样），面边长 512 以上加载会有可感停顿，属加载期一次性开销；
- **无多重重要性采样校正**：强光小太阳这类高动态范围细节在粗糙层会被平均掉。

### 4.17 级联遮挡体按级裁剪在 core 侧的契约（扩展二十四）

扩展二十四回答的是「4 级级联为什么要为同一批物件跑 4 遍深度 pass」。答案在 core 侧的纯函数模块
`r3e_core::shadow_cull` 里：**只吃矩阵与 AABB，零 wgpu 依赖**，因此可以纯 CPU 单测（对应用法见 §5.17 / §8.23）。

**① 单级判定 `aabb_in_cascade(view_proj: &Mat4, aabb: &Aabb) -> bool`**：把 AABB 八个角点换到该级光源裁剪空间，
只做两种裁剪：

| 条件 | 处理 | 理由 |
| --- | --- | --- |
| 八个点 `x` 全在 `[-1, 1]` 外（或 `y` 同） | 裁 | 影子落不到本级覆盖区 |
| 八个点 `z > 1`（远于级远平面） | 裁 | 比级远平面还远，投不进本级深度图 |
| 八点 `z < 0`（**在光源这一侧**） | **不裁**（fail-open） | 站在级切片前方、朝光源延伸的物件，影子仍会落进本级 |
| AABB 非有限 / 退化 | **不裁** | 拿不到可信包围盒时宁可多跑一次 |

三个判据都用「八点全满足才裁」，因此判定是**保守**的——只会漏裁，不会错裁，所以
`casterCull = true` 缺省开也不会改画面。

**② 逐级计划 `plan_casters(level_view_projs: &[Mat4], bounds: &[Aabb]) -> Vec<Vec<bool>>`**：返回
`masks[level][caster]`，外层是级、内层与传入 `bounds` 严格同序（**顺序即契约**，渲染层靠它和实例缓冲对齐）。
`tested` / `culled` 由调用方按 `masks` 直接数出来，模块不自己统计。

**③ 区间提取 `split_runs(offset: u32, mask: &[bool]) -> Vec<Range<u32>>`**：把布尔掩码切成**连续保留段**的
实例缓冲区间，给深度 pass 的 `draw_indexed(.., instance_range)` 用。零散保留会切出很多短段，
段数本身就是「这场景按级裁剪划不划算」的度量。`keep_all(n)` 给的是恒 `true` 掩码，
`fail_open_bounds()` 给的是「任何级都不裁」的包围盒（`FAIL_OPEN_HALF_EXTENT = 1e6`）。

**④ 外扩拟合 `fit_cascade_extended(direction, corners, size, extend) -> CascadeFit`**：与扩展十六的
`fit_cascade` 同一套拟合，唯一区别是把级联**近端沿光源方向推 `extend` 米**再算包围盒——目的是让
「贴着级边界、影子落进本级」的遮挡体落进深度图。`extend = 0.0` 时与 `fit_cascade` **逐位相同**
（有单测钉住），所以缺省路径的画面与扩展二十三逐字节一致。`extend` 收敛到
`[0, MAX_CASTER_EXTEND = 65_536]`，NaN 视作 0。

**⑤ 场景段**：`light.shadow` 多两个**可选**字段，缺省省略（写回时 `extend > 0` 才写、
`cull == false` 才写，与扩展十二 / 二十三同一套「缺省即省略」往返口径）：

| JSON | Rust | 缺省 | 语义 |
| --- | --- | --- | --- |
| `casterExtend` | `CascadeSettings::caster_extend: f32` | `0.0` | 级联近端朝光源外扩的米数 |
| `casterCull` | `CascadeSettings::caster_cull: bool` | `true` | 是否按级裁剪遮挡体 |

**测试口径**（`shadow_cull.rs` 内联 10 + `light.rs` 内联 3 + `tests/cascade_scene.rs` 3 + `tests/shadow_cull_cascade.rs` 4）：
八点全外才裁、部分在外不裁、近光源侧 fail-open、非有限 AABB fail-open、`split_runs` 的区间边界与
空掩码、`keep_all`、`fail_open_bounds`、`extend = 0` 与 `fit_cascade` 逐位等价、`extend` 收敛与
`MAX_CASTER_EXTEND` 夹取、两个 JSON 字段的缺省省略与往返。真实级联矩阵那 4 条用的是 4 级 / 32 个
沿视线均匀分布的遮挡体（最不利于裁剪的分布），逐级保留实测 `[9, 19, 32, 32]`（合计 92 / 128；见 §8.23）。

**已知边界**

- **AABB 粒度**：不逐三角形，一个实例只判一个盒（层级剔除见 §4.18，扩展二十五）；
- **保守侧漏裁**：只裁「八点全在外」，部分相交的遮挡体一律保留，级边界附近的密度不会明显变稀；
- **判定成本 O(级 × 实例)**：扩展二十五把这条压成「O(级 × 下探节点数) + 保留实例数」，
  但件数上万、层级又平坦时它仍逼近 O(级 × 实例)，`casterCull = false` 依然是省 CPU 的退路；
- **外扩默认 0**：紧贴级边界的遮挡体（屋檐 / 天桥 / 二层地板）可能丢影子，要靠 `casterExtend` 手动补。

### 4.18 遮挡体层级剔除在 core 侧的契约（扩展二十五）

§4.17 的判定是**逐实例**的：每级对每个遮挡体各做一次八角点投影，成本 `O(级 × 实例)`。
但同一批实例绝大多数和它们的兄弟坐在同一棵子树上——父节点的并集盒一旦整块落在该级覆盖范围外，
整枝实例都不必再判。扩展二十五把场景树的父子关系接进判定，同一个 `r3e_core::shadow_cull` 模块内实现，
仍然**零 wgpu 依赖、纯 CPU 可测**。

**① 节点 `CasterNode { parent: Option<usize>, bounds: Aabb, casts: u32 }`**：`parent = None` 即根；
`bounds` 是**本节点自身实例**的世界包围盒（没有实例时 `Aabb::EMPTY`）；`casts` 是本节点自身承载的遮挡体数
（一个节点被多个实例共享时 `> 1`）。构造糖：`CasterNode::leaf(bounds)`（自带 1 个实例）、
`CasterNode::group(parent)`（纯变换节点，`casts = 0`）、`.under(parent)` 挂父。

**② 树 `CasterTree::new(nodes: Vec<CasterNode>)`**：建图 + 自底向上聚「子树并集盒 `subtree`」与
「子树实例数 `sizes`」。父链上的**越界 / 自指 / 成环一律降级成根**——层级数据坏了宁可少裁，
也不能让下探原地打转（有单测）。拓扑序 `order`（父一定在子前）存下来，之后反向扫一遍就能重聚合。

**③ 逐帧刷新 `refresh(&[Aabb], &[u32])`**：用本帧实例盒覆写各节点自身数据并重新聚合，
**不动拓扑**。所以 `new` 只在场景结构变化时跑一次，物件移动/增删实例走 `refresh`。
缺项按空处理，不越界 panic。

**④ 判定 `plan(levels: &[Mat4]) -> CasterHierarchyPlan`**：每级一份「按节点」掩码 `masks[level][node]`，
外加三个计数（见 ⑤）。单级算法是自顶向下的显式栈：

| 情形 | 动作 | 计入 |
| --- | --- | --- |
| `sizes[node] == 0`（枝内一个遮挡体都没有） | 不判、不下探 | — |
| 子树盒整块在级外 | 掩码全 false，**整枝跳过** | `pruned += sizes`，`sizes > 1` 时再 `branch_pruned += sizes` |
| 子树盒通过、`casts > 0`、**叶** | 保留（子树盒就是自身盒，已判过） | `instance_tests += casts` |
| 子树盒通过、`casts > 0`、**有子** | 自身盒再判一次（子树盒比自身盒大） | `instance_tests += casts`，`box_tests += 1` |

**⑤ 三个计数口径**（`CasterHierarchyPlan` / `LevelCounts`）：

| 字段 | 含义 |
| --- | --- |
| `box_tests` | 做过的盒判定次数（每节点每级至多一次，混合节点的自身盒再加一次） |
| `instance_tests` | 实际逐实例判过的实例数 |
| `pruned` | 没做实例判定就定成「不可见」的实例数（整枝跳过 + 叶子直接被裁） |
| `branch_pruned` | 上面那笔里**来自多实例子树**的部分 = 层级剔除的净收益 |

恒等式：`instance_tests + pruned == 级数 × 总实例数`（`total_casts() × levels`）。
`pruned` 单独看会骗人——平坦场景（一实例一根）叶子被裁也全算进 `pruned`，看起来「省了 70%」，
而旧路径里那本来就是一次 AABB；**只有 `branch_pruned` 是真省下来的**，所以探针两个都打（§8.24）。

**⑥ 掩码摊平 `expand_masks(masks, instance_nodes) -> Vec<Vec<bool>>`**：把「按节点」掩码按
`instance_nodes[i]`（第 i 个实例所在节点）摊成「按实例」，与实例缓冲严格同序。
下标失效（`usize::MAX`）按**保留**处理：错位的代价是「多画一个遮挡体」，不是「影子凭空缺一块」。

**保守性**：正交投影下 `union` 的投影范围恰好是各成员投影范围的并集，所以「子树盒整块在级外」
等价于「枝内每个实例都整块在级外」——层级剔除**只少判、不改判**，掩码与扩展二十四逐位相同
（差分测试 `hierarchy_matches_per_instance_plan` / `flat_tree_matches_per_instance_plan` 钉住）。

**测试口径**（`tests/shadow_cull_hierarchy.rs` 13 条）：与逐实例判定逐位等价（组装树 / 平坦树 / 三帧移动差分）、
整枝跳过与 `refresh` 的重聚合、混合节点自身盒复判、`casts = 0` 的纯分组节点与空树、
父链破损（越界 / 自指 / 成环）降级为根、`fail_open_bounds` 挡住整枝裁剪但叶子仍精确、
计数平衡 `instance_tests + pruned = 级 × 实例`、`branch_pruned` 只计整枝。

**已知边界**

- **吃拓扑质量**：收益全看场景树是否「成组」——FBX 导进来的模型通常自带层级，手搓的扁平场景
  （每个盒子一个根）净收益为 0，还多花一层盒判定；
- **盒判定不是免费的**：树很深且每层只有一个子节点时，盒判定次数会接近实例数，此时收益也接近 0；
- **只做 CPU 预筛**：不做 GPU 侧代理（球/盒实例化 + 硬件裁剪），也不做跨帧缓存（每帧重判）。

### 4.19 世界法线矩阵的奇异保护（扩展二十六）

法线不能用世界矩阵直接变换，要用逆转置。`Scene::update_world_transforms` 原来写的是
`Mat3::from_mat4(world).inverse().transpose()`，问题就出在 `inverse()`：父链上任一节某轴
`scale` 为 0（缩放进场动画的第一帧、把物件压扁临时隐藏、脚本把缩放算成 0），世界矩阵立刻退化成
奇异矩阵，求逆后每个分量都是 `NaN` / `inf`。`NaN` 不像普通错误值那样停在原地——它顺着光照、
SSAO、快照一路扩散，表现是一整片区域随机刷黑或噪点，而且**每帧还不一样**，极难定位。

**① 入口 `world_normal_matrix(world: Mat4) -> Mat3`**（`r3e_core::transform`，随 `lib.rs` 顶层导出）：

| 情形 | 走的路径 | 结果 |
| --- | --- | --- |
| `det` 有限且 `\|det\| >= SINGULAR_DET_EPS` | 直接 `inverse().transpose()` | 与非保护写法**逐位相同** |
| `det` 有限但求逆出 `NaN` / `inf` | 退化分支（②） | 有限值 |
| `det` 非有限或过小（某轴缩放 → 0） | 退化分支（②） | 有限值 |

**② 退化分支 `degenerate_normal_matrix`**：把线性部分的三个列拆成「方向 + 缩放」——
列长 `>= MIN_LINEAR_SCALE` 时方向取其单位向量、缩放倒数取 `1 / len`；列长过小或非有限时
方向退回对应单位轴（X / Y / Z），缩放倒数取 `1 / MIN_LINEAR_SCALE`。
最终矩阵 = `from_cols(三方向) × diag(三缩放倒数)`。只对 TRS 复合精确，父链带 shear 时是近似，
但这个分支只在真不可逆时才走到——精度让位于「必须给出有限值」。

**③ 两个常量**：`SINGULAR_DET_EPS = 1e-12`（`1e-6` 量级缩放三次相乘正好落在它附近，
再小求逆出来的就是 1e12 以上的噪声）、`MIN_LINEAR_SCALE = 1e-6`（挡 `1 / 0`）。

**④ 局部版 `Transform::normal_matrix` 同步修**：原实现三个轴都写 `scale.max(1e-6)`，
负缩放会被抹成 `+1e-6`——镜像物件的法线被放大一百万倍且方向反掉。改用
`safe_scale_component`（非有限 → `+MIN_LINEAR_SCALE`；过小按**原符号**夹到
`±MIN_LINEAR_SCALE`；其余原样返回），镜像方向得以保留。

**⑤ 接入点**：`Scene::update_world_transforms` 里那一行改成调 `world_normal_matrix`，
其余逻辑（含紧随其后的 `update_skinning`）一字未动。`Node::world_normal` 的语义与序列化口径
（运行时缓存、不落 JSON）都不变；渲染侧同一问题的另一处修复见 §5.19。

**测试口径**（`tests/normal_matrix_safe.rs` 9 条）：非退化 TRS 随机 200 组逐位等于朴素逆、
单轴 0 缩放有限且退化轴恰取 `1 / MIN_LINEAR_SCALE`（健康轴不受牵连）、三轴全 0 有限、
`NaN` / `inf` / 极小缩放有限、`1e-5` 缩放的 det 确实低于阈值（证明走的是退化分支，
而不是被 `inverse()` 放大成 1e18 的噪声）、负缩放保留 `-0.5`、`safe_scale_component` 六种输入、
场景集成（父节点 0 / 全 0 / `NaN` 时全场景 `world_normal` 有限）、健康父子链 50 组逐位等于朴素逆
（证明保护没有偷偷改动健康场景）。

**已知边界**

- **只修数值不修语义**：缩放到 0 的物件在世界里本来就是退化物体，这里给的只是「最不坏的有限值」，
  不是有意义的几何；
- **`world_normal` 目前没有下游消费者**：主通道与阴影通道各算各的（§5.19），
  这一条守的是 core 契约、脚本与编辑器快照；
- **父链带 shear** 时退化分支是近似（见 ②）；正常路径不受影响。

## 5. 渲染层：r3e-render

### 5.1 GPU 数据布局

**顶点格式 `Vertex`（stride 72 字节，`step_mode = Vertex`）**

| 字段 | 类型 | location | 偏移 | 说明 |
| --- | --- | --- | --- | --- |
| `position` | `[f32; 3]` | 0 | 0 | 模型空间位置 |
| `normal` | `[f32; 3]` | 1 | 12 | 模型空间法线（已归一化） |
| `uv` | `[f32; 2]` | 2 | 24 | 纹理坐标（v 已在资产层翻转，原点左上） |
| `joints` | `[u16; 4]` | 3 | 32 | 蒙皮关节槽位（`Uint16x4`，非蒙皮全 0） |
| `weights` | `[f32; 4]` | 4 | 40 | 蒙皮权重（非蒙皮全 0，动态分支由此跳过 storage 读） |
| `tangent` | `[f32; 4]` | **11** | 56 | xyz = 切线，w = 手性 ±1（扩展二十一） |

切线的 location 是 **11 而不是 5**：顶点与实例属性共用同一套 shader location 命名空间，`5..=10` 已被
`InstanceRaw` 占满，重号会让管线校验直接失败。`r3e-render/src/mesh.rs` 的两个单测断言这条链路
（`vertex_layout_carries_skinning` 查 stride 72 与 location 序列 `[0,1,2,3,4,11]`，
`vertex_layout_carries_tangent` 查偏移 56 且 `offset + 16 == stride`），改布局时先看它们。

**实例格式 `InstanceRaw`（stride 96 字节，`step_mode = Instance`）**

| 字段 | 类型 | location | 说明 |
| --- | --- | --- | --- |
| `model` | `[[f32; 4]; 4]` | 5,6,7,8 | 世界矩阵，按 4 个 vec4 列传入 |
| `color` | `[f32; 4]` | 9 | 线性空间 RGBA |
| `joint_base` | `u32` | 10 | 蒙皮关节矩阵表在本帧 storage buffer 中的起点；非蒙皮填 0 |

之所以把模型矩阵放进实例缓冲而不是 uniform：这样**整个场景一次上传、按网格分组一次绘制**，不随物件数量增加 draw call。`step_mode = Instance` 让每个实例读同一份顶点但不同矩阵。

### 5.2 程序化网格

`MeshData::for_kind(kind)` 按类型分派，全部在 CPU 侧生成后上传，无文件 IO：

| 网格 | 参数 | 实现要点 |
| --- | --- | --- |
| `cube()` | 边长 1，中心原点 | **24 顶点 / 36 索引**：每个面独立 4 顶点，因此有硬边法线（复用顶点会得到"圆角"着色，视觉上像被磨过） |
| `sphere(28, 18)` | UV 球，半径 0.5 | `segments × rings` 双层循环，法线即单位球面方向；极点处有三角形退化，视觉上不可见 |
| `plane(10)` | XZ 平面 1×1，+Y 法线 | **10×10 细分**：单四边形在方向光下只需插值一个法线，本可不用细分；细分是为了将来接逐顶点光照/顶点动画时不出块状失真，代价仅 121 顶点 |
| `cylinder(28)` | 半径 0.5，高 1，轴向 Y | 侧面 28 段 + 上下盖扇形；盖法线固定 ±Y，侧面法线水平辐射 |

所有网格**统一尺寸为 1 单位**，物件的实际大小完全由 `Transform.scale` 决定——这样 `half_extents = scale * 0.5` 的物理碰撞体推导才能成立。

### 5.3 Renderer 初始化链路

`Renderer::new(window: Arc<Window>)`：

```
1. wgpu::Instance::new(Descriptor::new_without_display_handle())
2. instance.create_surface(window.clone())         → wgpu::Surface<'static>
3. instance.request_adapter(HighPerformance)        → 优先独显
4. adapter.get_info() → 日志打印 GPU 名/后端/设备类型
5. adapter.request_device(空 features, 默认 limits)
6. surface.get_capabilities + get_default_config
7. 若候选格式中存在 sRGB → 覆盖 config.format   ← 关键：否则画面发灰
8. surface.configure
9. 创建 CameraUniform buffer (128 B, UNIFORM | COPY_DST)
10. 创建 BindGroupLayout (binding 0, VERTEX|FRAGMENT, uniform buffer)
11. 创建 BindGroup
12. 编译 shader.wgsl（include_str! 编译期内嵌，无运行时文件依赖）
13. 创建 PipelineLayout → 不透明 RenderPipeline + 半透明 RenderPipeline（唯一差别是深度写入）
14. 创建单采样深度纹理 Depth32Float 并取 view（供遮蔽通道采样）
15. 创建 MSAA 附件 create_msaa_targets(w, h)：4× 颜色（HDR_FORMAT）+ 4× 深度
16. 创建实例缓冲（初始 512 个实例容量）
```

`resize` 时第 14、15 步的两组附件一起按新尺寸重建（贴图句柄不能原地 resize）。

**便携性处理**：`InstanceDescriptor::new_without_display_handle()` 是 wgpu 30 的新 API，不传 display handle，避免在 Windows 上额外的平台句柄耦合。

**Pipeline 固定状态**：

| 项 | 取值 | 说明 |
| --- | --- | --- |
| 拓扑 | `TriangleList` | |
| 正面 / 剔除 | `Ccw` / 剔除背面 | 绕序错了会看到"物体内部" |
| 深度格式 | `Depth32Float` | 32 位浮点，避免 z-fighting 更敏感 |
| 深度写入 / 比较 | `true` / `Less` | |
| 混合 | `ALPHA_BLENDING` | 支持半透明色，半透明批次按相机深度**由远及近**排序后最后绘制（扩展十九） |
| 多重采样 | `MSAA_SAMPLE_COUNT = 4` | 主通道光栅化到 4× 颜色附件，硬件 resolve 进 HDR 目标；后处理链只看到单采样纹理 |

两条管线共用同一份 `PipelineLayout` 与顶点布局，**唯一差别是深度写入**：

| 管线 | 用途 | 混合 | 深度写入 / 比较 |
| --- | --- | --- | --- |
| `pipeline` | 不透明批次 `groups[..transparent_start]` | `ALPHA_BLENDING` | `true` / `Less` |
| `transparent_pipeline` | 半透明批次 `groups[transparent_start..]` | `ALPHA_BLENDING` | `false` / `Less` |

半透明不写深度的原因：它必须能和已画好的不透明内容混合，若写深度，后画的近处半透明
会因深度测试把更远的半透明挡掉，反而丢掉本该可见的层次。顺序正确性完全由 CPU 侧排序保证
（见 §5.4 与 §5.13）。

管线只创建一次，之后场景怎么变都不重建——这是"文本改动下一帧即生效"的前提。

### 5.4 单帧渲染流程

```
render(scene):
  ├─ prepare_instances(scene)
  │   ├─ camera.aspect ← 真实窗口比例
  │   ├─ 组装 CameraUniform 并 write_buffer（view_proj / camera_pos / light_dir / light_color+intensity / ambient）
  │   ├─ 先按网格 + 材质分出键表，再按「alpha = node.color.w × 网格材质 alpha」拆成
  │   │   opaque_keys / transparent_keys，拼出 keys = 不透明 ++ 半透明
  │   │   （`transparent_start` = 不透明批次数，绘制时切一次管线即可）
  │   ├─ 遍历 scene.iter()：跳过 !visible；用 node.world 与 node.color 组装 InstanceRaw
  │   │   ├─ 不透明实例直接进实例缓冲，每组记录 Range<u32>（连续区间）
  │   │   └─ 半透明实例先攒 staged（带 view_depth 排序键），等整批收齐再排
  │   ├─ 顺带 union 出可见物件的世界包围盒，返回给阴影通道拟合光源相机
  │   ├─ 半透明批次：组间按「组内最远深度」降序，组内按 transparency::back_to_front
  │   │   由远及近搬进实例缓冲（同分保持原顺序，顺序稳定可复现）
  │   ├─ 惰性上传尚未创建的网格（首次用某网格才传）
  │   ├─ 实例数超过容量 → 2 倍扩容重建 buffer
  │   └─ write_buffer 上传全部实例
  ├─ prepare_shadow(groups, bounds, light)  → bool 是否需要跑阴影通道
  │   ├─ shadow.size 变化 → 重建深度纹理 + 采样器 bind group（贴图句柄不能原地 resize）
  │   ├─ Light::shadow_view_projection(场景包围球) → 光源正交矩阵
  │   ├─ 写 ShadowUniform（矩阵 + 纹素尺寸 + bias + 启用位 + 法线偏移 + 强度）
  │   └─ 写 ShadowPassUniform（只有矩阵，见 5.9 的绑定组切分原因）
  ├─ surface.get_current_texture()
  │   ├─ Success    → 直接用
  │   ├─ Suboptimal → 重新 configure 后用
  │   ├─ Outdated / Lost → 重新 configure 并 return（跳过本帧）
  │   └─ 其它        → 记 debug 日志并 return
  ├─ [阴影通道] 开 RenderPass：无颜色附件，clear 深度 = 1.0（format = Depth32Float）
  │     set_pipeline(shadow_pipeline, vs_shadow 只写深度)
  │     set_bind_group(0..=2)  ← wgpu 要求布局里每个组都必须绑，哪怕着色器不用
  │     set_bind_group(3) = shadow_pass_bind_group（只含光源矩阵）
  │     for (mesh, range) in groups: draw_indexed(...)   → stats.shadow_draws++
  ├─ [深度预通道] 单采样 depth_view ≈ 不透明段铺一遍深度（扩展十九）
  │     ← 主通道的深度挂在 4× 附件上，纹理用法不能给遮蔽通道采样，
  │        所以先用单采样深度写满；只画 groups[..transparent_start]
  │     → stats.depth_prepass_draws
  ├─ [主通道] 开 RenderPass：
  │     颜色附件 = msaa_color_view（4× HDR），resolve_target = postfx.hdr_view()，
  │     clear 颜色 = scene.background；
  │     深度附件 = msaa_depth_view（4×，LoadOp::Clear(1.0)、StoreOp::Discard）
  │     set_pipeline(pipeline) → 不透明段 groups[..transparent_start]（写深度）
  │     set_pipeline(transparent_pipeline) → 半透明段 groups[transparent_start..]（不写深度）
  ├─ for (mesh, range) in 该段 groups:
  │     set_vertex_buffer(0, 该网格顶点)
  │     set_vertex_buffer(1, instance_buffer[range])
  │     set_index_buffer(Uint32)
  │     draw_indexed(index_count, 0, 0..1)     ← 1 次 call 画完该网格全部实例
  ├─ 统计 RenderStats { draw_calls, instances, shadow_draws, culled, cull_unknown,
  │                    transparent_draws, transparent_instances, depth_prepass_draws, frame_index }
  ├─ [UI 通道]（与文本通道同级，谁先谁后由 scene.ui_above_text 决定）
  │     ui_pass.render(scene.ui, (w, h))       ← ui 为空或 ui_enabled=false 则整段跳过
  │     ├─ sort_by_layer(scene.ui)              → 层级升序、同层按加入顺序（稳定）
  │     ├─ 每个元素 resolve_rect(锚点 + 单位) → 屏幕像素矩形；尺寸非正则丢弃
  │     ├─ 矩形 1 实例 / 边框 4 实例 / 图片 N 实例 → 按纹理分组
  │     ├─ 首次出现的图片路径 → provider 加载 → create_texture + bind group（失败只记一次）
  │     └─ 每组 draw(0..6, 实例区间)            → stats.ui_draws++ / ui_quads++
  ├─ [文本通道] 开 RenderPass：LoadOp::Load 叠加到已画好的颜色附件，**无深度附件**
  │     text_pass.render(texts)                ← texts 为空则一次 GPU 命令都不发
  │     ├─ build_text(字体, scene.texts, 图集) → 每字形一个 TextInstance + 新字形上传表
  │     ├─ 新字形 → write_texture 进对应图集页；新页 → 当场建纹理 + bind group
  │     ├─ 实例数 > 缓冲容量 → 2 倍扩容重建 instance_buffer
  │     └─ 按页分组 draw(0..6, 0..n)           → stats.text_draws++
  ├─ stats.text_glyphs / text_draws / text_missing ← text_pass.stats()
  ├─ stats.ui_quads / ui_draws                  ← ui_pass.stats()
  ├─ queue.submit + queue.present
```

**关键性质**：每帧从 `Scene` **全量重建**实例数据，不维护任何 GPU 侧持久状态（除了静态网格与管线）。因此脚本删物件、改颜色、改位置，都不需要通知渲染器，也不存在"状态没同步"的问题。代价是每帧一次 O(可见物件数) 的 CPU 组装与全量上传——在当前规模（数千物件）完全够用，上万物件时会成为瓶颈。

**Draw call 数 = 实际使用的网格种类数**（≤ 4）。1000 个立方体是 1 个 draw call，不是 1000 个。

### 5.5 文本通道（M1：文字 GPU 上屏）

文本是唯一"内容每帧可能变"的绘制通道，设计目标是：**CPU 只重排、GPU 只重传新字形**。

**位置与解耦**：文本 pass 排在**阴影与 3D 主通道之后**，是独立 `RenderPass`：

- **不共享深度**：`depth_stencil_attachment: None`。文本永远在最上层，既不受 3D 深度遮挡，
  也不会把深度写脏（主通道 `StoreOp::Store` 的深度结果保持原样）。
- **`LoadOp::Load`**：不 clear 颜色，直接叠在 3D 画面上；混合走 `ALPHA_BLENDING`，
  字形边缘覆盖率来自 R8 图集采样值（sRGB 目标已由管线 format 决定，着色器不做 gamma 反转）。
- **前置门控**：`scene.text_enabled == false` 或 `scene.texts` 为空时 `TextPass::render` 直接 return，
  encoder 里一条文本命令都没有 —— 这是"关掉文本后主通道行为与加文本前完全一致"的实现级保证。

**职责切分**：`r3e-core` 出位图与度量（`FontSet` + `GlyphAtlas` + `layout_text`），
`r3e-render` 一个字形都不解析——它只把 `TextLayout` 的每个字形翻译成一个 `TextInstance`：

```
// text.wgsl 的 Instance（@location 0..4），与 text.rs 的 TextInstance 一一对应
struct Instance {                  // 48 字节 = 12 个 f32，array_stride 取 size_of
    origin:  vec2<f32>,            // 四边形左上角，屏幕像素，原点在左上，已含对齐位移
    size:    vec2<f32>,            // 四边形尺寸（像素）
    uv_min:  vec2<f32>,            // 图集归一化 uv 左上角
    uv_max:  vec2<f32>,            // 图集归一化 uv 右下角
    color:   vec4<f32>,            // 含 alpha
}
```

顶点阶段**不用顶点缓冲**：`@builtin(vertex_index)` 拼出两个三角形（6 顶点），
位置＝`origin + corner * size`，再折算到 NDC（`x = 2x/w − 1`，`y = 1 − 2y/h`），
所以整批文本是"每图集页一次 `draw(0..6, 实例区间)`"，除实例缓冲外无任何上传。

片元阶段只取图集 R 通道当覆盖度：`vec4(color.rgb, color.a * coverage)`，靠 `ALPHA_BLENDING` 叠加。
uv 各向内收半个纹素（`UV_INSET`），避免线性采样吃到邻格字形。

**图集上传**：页格式 `R8Unorm`（纯覆盖率，不含颜色），1024×1024，初始 1 页，
`MAX_ATLAS_PAGES = 8`。键是 `(face, glyph_id, 量化字号)`：同字号重复出现直接命中，
因此稳态帧的上传表为空（这是"每帧全量重排但不重复上传"的关键）。某页分配不下即开新页，
新页当场 `create_texture` + `write_texture` + `create_bind_group`，绘制时按页分组。

**容量与安全阀**：

| 项 | 值 | 超限行为 |
| --- | --- | --- |
| 单帧字形实例 | `MAX_TEXT_INSTANCES = 32768` | 超出部分只累加 `overflow`，不绘制、不 panic |
| 图集页 | `MAX_ATLAS_PAGES = 8` | 分配失败的字形计入 `overflow`，不绘制 |
| 实例缓冲 | 首次 1 页容量，不足时 2 倍扩容 | 重建 buffer（每帧全量重写，无需保留旧内容） |

**测试策略**：`r3e-render/src/text.rs` 的单测全部跑在 `FakeFont`（纯算术假字形）上，
不加载系统字体、不建 `wgpu::Device`，因此无 GPU 的 CI 环境也能覆盖
"顶点数量与位置、对齐位移、空串不产生实例、超长文本不 panic、图集分页与复用"。

脚本层的"每帧提交不累积"语义由 `r3e-script/src/lib.rs::text_script_tests` 覆盖：
同一 `key` 连续 `text_add` 10 帧后 `Scene::texts.len()` 必须仍为 1（只有首帧返回 `true`），
另有 `text_set` 对缺失 key 返回 `false`、`text_clear` 与 `set_text_enabled` 互不干扰两条。
真实字体与上屏效果只由 `cargo run` 的人工验证覆盖（见第 13 章）。

### 5.6 UI 通道（M1：矩形 / 边框 / 图片）

**为什么不再开一类 pass**：三类元素在 GPU 侧是同一种东西 —— 一个带纹理的屏幕四边形。
所以只有一个 `UiPass`（一条 pipeline、一个实例缓冲），差异全在实例数据里：

| 元素 | 实例数 | 数据 |
| --- | --- | --- |
| `Rect` | 1 | 纯色四边形，`texture = None`（绑定 1×1 白纹理 + 顶点色调制） |
| `Border` | 4 | 上下左右四条矩形，全部落在给定矩形**边界内**（上下整条，左右夹在中间，四角不重叠），厚度夹到 `min(w,h)/2` |
| `Image` | 1（九宫格时最多 9） | 四边形 + 该路径的 `TextureImage`，颜色乘到采样值上（可做淡入淡出）；`slice_margin` 非全零时切 3×3，四角不拉伸、四边单向拉伸、中心双向拉伸，每块 uv 取 `uv` 子区间的对应份（见 §13 九宫格小节） |

同一帧里颜色相同（`texture == None`）的元素合并成一次 draw；图片按路径分组，
每张纹理一次 draw。因此「血条 2 层 + 对话框 + 边框 + 图标」是 2 个 draw call，
不是 5 个，也不随元素数量线性增长（受 `MAX_UI_QUADS = 4096` 兜底）。九宫格元素
的 9 块与普通四边形走同一缓冲，不额外增加 draw call；块数装不下剩余预算时该元素
整体丢弃（原子切分，不留残片）。

**与文本通道的层次**：两条通道都是 `LoadOp::Load` + `depth_stencil_attachment: None`，
即"后画的赢"。默认先 UI 后文本（`scene.ui_above_text == false`），这样对话框底框、
血条背景永远不会盖住文字；把 `set_ui_above_text(true)` 打开则顺序反转。顺序在
`Renderer::render` 里只有一处 `if`，不是两套代码路径。

**布局解析全在 CPU**（`r3e-core/src/ui.rs`，零 GPU 依赖、可单测）：

```
resolve_rect(el, (screen_w, screen_h)):
  size  = [w.resolve(screen_w), h.resolve(screen_h)]     // px 原值 / pct → 百分比
  size  ≤ 0 或非有限 → None（不画，不 panic）
  (ax, ay) = anchor.factors()                            // 9 锚点 → 0 / 0.5 / 1
  x = ax * (screen_w - w) + offset_x
  y = ay * (screen_h - h) + offset_y
```

`ax * (screen_w - w)` 而不是 `ax * screen_w` 是关键：右对齐元素在窗口变宽时保持贴边，
而不是被推出屏幕。窗口 resize 不需要任何通知 —— 每帧都拿当前 surface 尺寸重算一次，
这也是"resize 后不错位"的实现级保证（单测覆盖 800×600 / 1600×900 两组尺寸）。

九宫格边距在 `resolve_rect` **之后**求解（`slice_margins(&rect)`），因为百分比边距要按
元素最终宽高换算；尺寸已经是收敛过的有效值，所以夹取只需处理"边距之和超过尺寸"。

**图片加载的边界**：`r3e-render` 不碰文件 IO（不引 `image` 依赖），只定义
`UiImageProvider` trait；`r3e-app` 实现它并复用 `r3e-asset::load_texture_image`（png/jpg/tga/bmp）。
失败路径：加载报错 → 记录到 `failed_images` → 该元素**回退成纯色块**（用元素 color），
**同一条路径后续不再重试**，也不会每帧刷日志。

**容量与安全阀**：

| 项 | 值 | 超限行为 |
| --- | --- | --- |
| 元素总数 | `MAX_UI_ELEMENTS = 1024` | 只拦**新增**：新 key 返回 `UiUpsert::Rejected`，已有元素原样保留；同 key 覆盖不受限 |
| 单帧四边形实例 | `MAX_UI_QUADS = 4096` | 超出部分丢弃并累加 `skipped`，不 panic；九宫格元素按整元素原子丢弃（不留半张图） |
| 纹理边长 | `MAX_UI_TEXTURE_DIM = 8192` | 超限图片按失败处理并回退纯色 |
| 边框厚度 | `min(w, h) / 2` | 夹住，不会画出反向四边形 |

**唯一写点与同 key 语义**（这是本节的结构性约束，改动前先看）：

`Scene::ui` 只有两组写入口，都在 `r3e-core/src/ui.rs`，脚本层**不得**直接 `push`：

| 入口 | 语义 | 调用方 |
| --- | --- | --- |
| `upsert` | **裸全量覆盖**：同 key 连 `layer` / `visible` / `uv` 一起被新值重置 | 程序化一次性构造（测试、场景加载） |
| `upsert_styled` | **脚本唯一写点**：同 key 先走 `merge_same_key`，再跑 `sanitized` 收敛非法值 | `r3e-script` 的 `ui_rect` / `ui_border` / `ui_image` |

两者共用 `UiUpsert` 三态返回值（`Inserted` / `Replaced` / `Rejected`）与同一套上限判定，
差别只在"同 key 时怎么合并"。`merge_same_key` 的字段策略写死在 core：

| 字段 | 同 key 处理 |
| --- | --- |
| `kind` / `anchor` / `offset` / `size` / `color` / `image` | 取新值（每次提交都在描述"这一帧长什么样"） |
| `layer` / `visible` / `uv` | 沿用旧值（样式与开关，只能由 `ui_set_*` 改） |
| `border_width` | 边框元素：入参 `> 0` 取新值，否则沿用旧线宽，旧值也无则回落 `DEFAULT_BORDER_WIDTH = 2.0`；非边框恒 `0` |

脚本层的职责被压缩到"搬参数 + 记账"：`submit_ui` 把 `UiUpsert` 翻成 `bool`
（`Inserted = true`，`Replaced` / `Rejected = false`），空 key 与超限都写进 `UiWriteLog`
（`last_error` / `error_count`），由 `ui_last_error()` / `ui_error_count()` 观测，
不做静默丢弃。setter 类接口（`ui_set_*`）共用 `find_mut` 定位槽位，缺失 key 返回 `false`。

**测试策略**：`r3e-core/src/ui.rs` 覆盖锚点落点（9 种）、百分比换算、resize 重算、
空/负/非有限尺寸不 panic、层级稳定排序、serde 往返、`merge_same_key` 的三类字段策略、
`upsert_styled` 的合并与上限（满载可覆盖、满载拒新增）、入参非法值收敛，
以及九宫格边距的百分比换算、超尺寸等比夹取、负数/NaN 收敛、`has_slice` 只对图片为真、
`merge_same_key` 对 `slice_margin` 的沿用；
`r3e-script/src/lib.rs` 覆盖脚本路径的同一组保证（超限被拒且 `ui_count()` 不增长、
满载同 key 仍可更新、空 key 被拒、跨类别同 key 保留样式、非法参数收敛），
以及 `ui_set_slice` 的像素/百分比写入、无参清零、每帧重发几何后边距仍沿用；
`r3e-render/src/ui.rs` 覆盖
矩形 1 实例、边框 4 实例且厚度被夹、图片按纹理分组、隐藏与退化元素被跳过、
quad 预算丢弃、图片只请求一次、失败只记一次、`HudScene` 整体建批，
以及九宫格 9 块的位置/尺寸/uv（含面积守恒）、边距全零仍为 1 块、
超尺寸退化不产生零/负尺寸块、与 `uv` 子区间叠加、预算不足时整元素原子丢弃。

### 5.7 着色器（shader.wgsl）

**Bind group 布局**

`@group(0)`：相机 / 光（128 字节，16 字节对齐）

| 字段 | 类型 | 内容 |
| --- | --- | --- |
| `view_proj` | `mat4x4<f32>` | 视图投影矩阵 |
| `camera_pos` | `vec4<f32>` | xyz = 相机世界位置 |
| `light_dir` | `vec4<f32>` | xyz = 光线行进方向 |
| `light_color` | `vec4<f32>` | xyz = 光色，**w = 强度** |
| `ambient` | `vec4<f32>` | xyz = 环境光色 |

`@group(1)`：材质（`pbr::MaterialUniform`，64 字节）

| 字段 | 类型 | 内容 |
| --- | --- | --- |
| `base_color` | `vec4<f32>` | rgb = 材质染色因子（乘到物件颜色上），a 参与最终 alpha |
| `params` | `vec4<f32>` | **x = metallic，y = roughness，z = 是否有 base color 贴图**，w 保留 |
| `emissive` | `vec4<f32>` | rgb = 自发光（线性 HDR，只加不减），a 保留 |
| `params2` | `vec4<f32>` | **x/y/z = 法线 / 粗糙度 / AO 贴图开关**（0/1），w 保留（扩展二十一） |

绑定项：`0` = uniform buffer，`1` = base color 贴图，`2` = 采样器，`3/4/5` = 法线 / 粗糙度 / AO 贴图。
三张数据贴图**永远绑定**（没图时垫 `white` / `flat_normal` 占位），由 `params2` 决定是否采样 ——
动态 uniform 分支而不是切换 bind group，材质缓存键与绘制路径都不用为"有无贴图"分叉。占位图存在的作用
只是把 bind group 填满：开关为 0 时着色器根本不读它，所以"没图"不等于"画面变了"。

`@group(2)` = 关节矩阵（P4 蒙皮，storage buffer）；`@group(3)` = 阴影贴图 + 阴影 uniform（阶段 D） + 局部光源表（扩展十五，binding 4，与阴影同组见 5.10 的 `max_bind_groups` 说明）。
材质按 `MaterialKey`（`to_bits` 比较）分桶：同材质的物件共用一份 64 字节 buffer，不逐物件复制。

**顶点阶段**

```wgsl
let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
let world = model * vec4(in.position, 1.0);

// 法线矩阵用伴随矩阵（余子式转置），非均匀缩放下是严格逆转置的近似
let normal_matrix = mat3x3<f32>(
    cross(model[1].xyz, model[2].xyz),
    cross(model[2].xyz, model[0].xyz),
    cross(model[0].xyz, model[1].xyz),
);
```

用伴随矩阵而不是真的算逆转置，是为了避免在顶点着色器里做 `inverse`（GLSL/WGSL 的 3x3 逆代价高且部分后端实现差异大）。**均匀缩放下该近似是精确的**；非均匀缩放时法线会有轻微偏差——注释已明示这一取舍。

**片元阶段（Cook-Torrance 直接光照）**

着色器里的 BRDF 与 `r3e-core::material` 的 CPU 参考实现逐项对应——CPU 侧有单测（白炉积分、
退化输入、金属度归一）兜底，GPU 侧照抄公式，两边算的是同一套语义。

```wgsl
// 半程向量：l + v 退化时取法线，避免 normalize(0) 出 NaN
h = select(normalize(l + v), n, length(l + v) < 1e-4)

f0 = mix(vec3(DIELECTRIC_F0), albedo, metallic)   // 绝缘体 4%，金属染 base color
f  = fresnel_schlick(vdh, f0)                     // F
d  = distribution_ggx(ndh, roughness)             // D：GGX，α = r²
g  = geometry_smith(ndv, ndl, roughness)          // G：Schlick-GGX，k = (r+1)²/8

// 背面（n·l ≤ 0）直接给零，否则 0·inf 会变 NaN
specular = ndl > 0 ? f * d * g / (4 * ndv * ndl) : 0
diffuse  = (1 - f) * (1 - metallic) * albedo / PI

lit = (diffuse + specular) * light_color.rgb * light_color.w * ndl * shadow
    + albedo * ambient.rgb
    + emissive.rgb
```

- **阴影只乘直接光**：环境光与自发光不衰减，否则影子是纯黑块、丢暗部层次。
- **自发光只加不减**：不乘 `n·l`、不吃阴影、不参与光照计算，因此天然是"灯管 / 宝石"这类开口。
- **两道粗糙度下限**：CPU 侧写入前夹到 `0.045`，着色器再 `clamp` 一次，任一环节漏了也不会爆白。
- **反照率夹到非负**：负值会把光"减掉"，与 CPU 侧一致按 0 处理。
- **没有 gamma 校正代码**，依赖 surface 用 sRGB 格式自动完成转换；颜色值全线按线性空间解释。
- **没有 IBL**：金属只反射方向光，暗部由 `albedo * ambient` 常量项兜底。

### 5.8 RenderStats 与 HUD

```rust
pub struct RenderStats {
    pub draw_calls: u32,   // 主通道 draw call（按网格合批）
    pub instances: u32,    // 主通道绘制实例数
    pub shadow_draws: u32, // 阴影通道 draw call（= 合批数 × 级数）；0 = 本帧跳过了阴影通道
    pub shadow_cascades: u32, // 本帧实际生效的级联数；0 = 阴影关闭，1 = 单级
    pub culled: u32,       // 被视锥剔除的实例
    pub cull_unknown: u32, // 缺包围盒而 fail-open 放行的实例
    pub frame_index: u64,
    pub text_glyphs: u32,  // 文本通道四边形数
    pub text_draws: u32,   // 文本通道 draw call；0 = 关闭 / 无字体 / 无文本
    pub text_missing: u32, // 缺字数（只占位不绘制）
    pub ui_quads: u32,     // UI 通道四边形数（矩形 1 / 边框 4 / 图片 1）
    pub ui_draws: u32,     // UI 通道 draw call；0 = 关闭 / 无元素
    pub postfx: PostFxStats, // HDR 后处理链（扩展十三）：enabled / bloom_passes / passes
    pub ssao: SsaoStats,   // 屏幕空间遮蔽（扩展十七）：enabled / passes / samples / size
    pub local_lights: u32, // 真正上传并参与计算的局部光源数（扩展十五）
    pub transparent_draws: u32,     // 半透明通道 draw call（扩展十九）；0 = 本帧无半透明批次
    pub transparent_instances: u32, // 半透明通道实例数
    pub depth_prepass_draws: u32,   // 深度预通道 draw call（只画不透明段）
}
```

`ssao.passes` 的取值只有三种：`0`（场景没开遮蔽段，整条链跳过）、`2`（遮蔽 + 4×4 散焦各一次全屏 pass）。
`ssao.samples` 是**实际生效**的核样本数（已被夹进 `[4, 16]`），与场景里写的值可能不同——这正是要观测它的原因。

`--run-seconds` 的起止探针会把 `shadow_cascades`、`shadow_draws`、文本与 UI 通道的四边形/draw call 数一起打出来，
是「对应通道真的跑了」的数值证据（关闭时为 0）。UI 探针还会额外给出元素数、缓存贴图数、
缺图回退数与超限跳过数（来自 `UiPass::stats()`，比 `RenderStats` 更细）。

`app.rs` 每帧把统计写进窗口标题：`r3e — Rust 3D Engine | N 物件 / M draw / X.X ms | 运行中`。这是当前唯一的运行时观测手段（无 GUI overlay）。

### 5.9 方向光阴影贴图（阶段三 D）

单方向光的正交阴影，一张 `Depth32Float` 深度图 + 第二个 render pass。

**两趟 pass 的顺序不能反**：先渲染阴影通道（只写深度、无颜色附件），再渲染主通道。
主通道在片元着色器里采样这张深度图做遮挡判定，因此两张 pass 之间有隐式写后读依赖——
wgpu 依靠同一个 encoder 内的顺序与资源状态自动插入 barrier，无需手动同步。

**为什么单独拆一份 `ShadowPassUniform`（`group3@binding3`）**

最初的实现是让 `vs_shadow` 直接复用主通道的 `u_shadow`（`group3@binding2`），结果 wgpu 报
「纹理在同一 pass 内既被写入又被采样」：因为 `group3@binding0` 绑的就是这张深度图本身，
即使顶点着色器根本没读它，只要绑定组里有、且该组在 pass 里被 `set_bind_group`，就构成读写冲突。

修法是给阴影 pass 建**独立的 pipeline layout**（只有 group0 相机 / group1 材质 / group2 关节），
再加一个**只含光源矩阵**的 uniform（`group3@binding3`），阴影 pass 建 `set_bind_group(3, shadow_pass_bind_group)`。
两条硬约束：

1. 阴影 pipeline layout 里每个 group 都必须在 pass 内被 `set_bind_group`（wgpu 校验的是布局而非着色器实际引用），
   所以即使 `vs_shadow` 不用材质与关节，`0/1/2` 也必须绑占位组。
2. 深度图句柄不能原地 resize。`size` 变化时重建纹理 + 视图 + 比较采样器绑定组，
   句柄替换发生在 `prepare_shadow` 里，避免在 pass 中途换资源。

**遮挡计算（`shadow_factor`）**

| 步骤 | 做法 |
| --- | --- |
| 世界 → 光源裁剪 | `u_shadow.light_view_proj * world_position`，正交投影故 `w = 1`，`ndc.z` 直接就是 0..1 深度 |
| UV | `(x*0.5+0.5, 0.5-y*0.5)`（Y 翻转，与贴图采样口径一致） |
| 越界 | UV 出 [0,1] 或 `ndc.z > 1` 一律返回 1.0（受光），防止视锥外的物件出现硬边黑块 |
| 比较 | `cmp_depth = ndc.z - bias`，用 `textureSampleCompare` + `sampler_comparison`，硬件做比较采样 |
| 滤波 | 3×3 PCF（9 次比较平均），给出两三个纹素宽的柔和边缘 |
| 偏移 | 先按法线方向推 `normal_bias` 个世界单位再投影（`params.w`），斜射面抗 acne 且比纯深度偏移更少「影子漂移」 |
| 强度 | `mix(1.0, visible, strength)`，让强度可调而不是硬开关 |

**只有直接光被遮挡**：片元着色器里 `diffuse * ndl * shadow` 与 `spec * shadow`，环境光项不乘。
否则阴影区会掉到纯黑，暗部层次全丢（这一条是视觉验收时最先看出来的问题）。

**渲染统计**：`stats.shadow_draws` 记录阴影通道的 draw call 数。因为实例按网格合批，
这个数通常小于主通道实例数（例如演示场景 5 个物件、4 种网格 → 4 次 draw）；关闭阴影时为 0。
`app.rs` 的起止探针会同时打印两者，作为「阴影真的跑了 / 真的关了」的数值证据。

**代价**：阴影通道的顶点开销 = 主通道的顶点开销（同一批网格再画一遍），片元只写深度、无着色；
主通道每片元多 9 次比较采样。2048² 贴图在集显上实测仍为满帧，主要成本来自 PCF 采样而非贴图带宽。

---

### 5.10 局部光源表（点光 / 聚光，扩展十五）

方向光解决「整体明暗」，解决不了「一束光打在某个物件上」。扩展十五加**定长光源表**：
场景最多 16 盏点光 / 聚光，每帧整表打包进一个 uniform，片元着色器里循环累加。
选「定长数组 + 计数」而不是 per-light bind group，是为了不加 pass、不加 draw call——
灯多灯少都是同一套管线。

**CPU 侧契约（`r3e-core::light`，零 wgpu 依赖）**

| 项 | 内容 |
| --- | --- |
| 类型 | `LocalLight { key, kind: LocalLightKind::{Point, Spot}, pos, color, intensity, range, dir, inner_deg, outer_deg }` |
| 收敛 | `LocalLight::sanitized()`：强度夹 `0..200`、颜色夹 `0..16`、半径夹 `0.05..500`、外角夹到 `0..89.9`、内角至少比外角小 `0.1`（角度退化会让锥因子除零） |
| 衰减 | `attenuation(range, d) = (1 - (d/range)^4)² / (d² + ε)`，`d ≥ range` 直接 0，`ε = 0.01` 防 `d→0` 爆白 |
| 锥因子 | `spot_factor(cos, cos_inner, cos_outer)`：内外角之间 `smoothstep`，内角内为 1、外角外为 0；`cos_inner <= cos_outer` 时退化成硬边阶跃 |
| CPU 参考 | `LocalLight::irradiance(world_position)` 与着色器逐式对应，脚本 `local_light_irradiance` 直接用它——脚本读到的值就是画面里的值 |
| 上限 | `MAX_LOCAL_LIGHTS = 16`，场景层 `upsert_local` 满员时返回 `false` 而不顶掉别人 |

**GPU 侧布局**（`LightsUniform`，`1 + 16×64 = 1040` 字节）

| 偏移 | 内容 |
| --- | --- |
| 0 | `count: vec4<f32>`，`x` = 本帧有效灯数 |
| 16 | `items: array<LocalLight, 16>`，每盏 4 个 `vec4`：`pos_range` / `color_power` / `dir_inner` / `outer_kind` |

`outer_kind.y` 是类型码（0 点光 / 1 聚光），着色器据此决定要不要算锥因子；
余弦在 CPU 侧由 `cos_inner()` / `cos_outer()` 算好，片元里省掉两次 `cos`。

**绑定位置：`@group(3) @binding(4)`，与阴影贴图同组。**
WebGPU core 规范 `max_bind_groups = 4`（group 索引 0..3），第五组 `@group(4)` 在创建 pipeline layout 时
就过不了校验——第一版正是这么写的，启动即 `wgpu Validation Error: Shader @group(4) ... max_bind_groups`。
于是光源表并入场景级绑定组的第 5 个槽位：`0..3` 仍是实例 / 材质 / 关节 / 阴影，`4` 让给光源表。
`tests/local_lights_shader.rs` 现在同时断言「无 `@group(4)`」「`shadow_layout` 含 binding 4」「不存在独立
`lights_bind_group`」，这三条一起锁死这次迁移，避免有人手滑把组拆回去。

**总开关为什么写 `count.x = 0` 而不是跳过上传**：灯的数据留在 uniform 里，
片元循环上界取 `min(count.x, 16)`，关掉即零次迭代；再打开不需要重新打包，
也不存在「开关与数据不同步」的中间态。

**片元侧三处早退**：超半径 → 背面（`ndl <= 0`）→ 聚光锥外，任一命中就 `continue`，
没被照到的片元不为这盏灯付 GGX/Smith 的代价。局部光源复用方向光那套 Cook-Torrance 函数，
差别只有两点：光向量逐灯不同（`l = normalize(pos - p)`）、每盏额外乘距离衰减与锥因子。

**投影**：扩展十五首版**不投影**——16 盏灯各配一张立方图的成本（16×6 个面、每面一次场景重绘）远超当期收益。
扩展十八 C 补上了受控版本：`LocalLight` 增 `cast_shadow` / `shadow_bias`，**每帧最多 4 盏**真正投影，
点光 6 面、聚光单面，全部塞进同一张 24 层深度数组（见 §5.12）。缺省 `false`，
所以不写这两个字段的场景画面与扩展十五逐像素一致。

---

### 5.11 级联阴影（CSM，扩展十六）

阶段 D 的阴影只有一张覆盖「整个场景包围球」的正交图：视距一拉远，全部纹素摊到几十米上，近处物件立刻出锯齿；
想兼顾近处就得把 `ortho_size` 调小，远处又整片没影子。级联把相机视锥**按深度切段**，每段一张同分辨率的图。

**切分（`r3e_core::cascade_splits`）**

| 项 | 内容 |
| --- | --- |
| 切分公式 | `split_i = near·(far/near)^(i/n)` 与 `near + (far-near)·i/n` 按 `lambda` 线性插值：`lambda = 0` 均匀、`1` 纯对数 |
| 首尾 | 钉在 `camera.znear` 与「`max_distance` 或 `zfar`」上 |
| 自适应上限 | `max_distance = 0` 时取「场景包围球最远点 + 1.05·半径」，几何之外没有可投影的东西，摊过去纯属浪费纹素 |
| 输出 | `Vec<f32>`，长度 = 级数 + 1（`splits[i]..splits[i+1]` 是第 i 级的深度区间） |

**每级拟合（`r3e_core::frustum_slice_corners` + `fit_cascade`）**

1. 用逆相机矩阵把该级 8 个视锥角点变换到世界空间；
2. 光源沿 `-direction` 架正交相机罩住这 8 点，取**正方形外接**（半宽 = 8 点在光源空间 XY 的最大半径），
   避免宽高比不一致导致纹素各向异性；
3. **纹素量化**：把中心按 `2·radius/size` 对齐到纹素格，相机移动时阴影边缘不会「游动」（shadow shimmer）；
4. `ortho_size > 0` 时作为**单级覆盖半宽上限**，超出就把矩阵整体缩放——保留阶段 D 那个手动旋钮的语义。

**GPU 接线（`group(3)` 内完成，不开新组）**

| 槽位 | 阶段 D | 扩展十六 |
| --- | --- | --- |
| `binding 0` | `texture_depth_2d` | `texture_depth_2d_array`（层数 = `MAX_CASCADES`），`binding 1` 采样器不变 |
| `binding 2` | `ShadowUniform { light_view_proj: mat4, params }` | `light_view_proj: array<mat4x4<f32>, 4>` + `split_ndc: vec4<f32>`（相机空间切分点换算到 NDC 深度） |
| `binding 3` | `ShadowPassUniform`（深度 pass 专用） | 不变，改为**每级一份** buffer，深度 pass 逐级写 |

`max_bind_groups = 4` 的硬限制没有变通空间，级联信息全部塞进既有阴影 uniform，因此**没有新增绑定组、没有新增纹理槽**。
未使用的级（`count < 4`）复制最后一级的矩阵与切分点：着色器即使因浮点误差选到它，也只会重复一次最近级的采样，不会采到未初始化的层。

**采样（`shadow_factor`）**

1. 相机空间深度 `view_z` 与 `split_ndc` 比较，二分/线性定位级联下标 `level`；
2. 在 `level` 这一层做 3×3 PCF（`shadow_sample_level`）；
3. 距该级远端 `blend` 比例以内的片元，同时采 `level + 1` 并按比例混合——级间接缝处不会有硬线；
4. UV 越界或 `ndc.z > 1` 仍返回 1.0（受光），与阶段 D 口径一致。

**统计口径**：`stats.shadow_draws = 合批数 × 级数`（每级一次全场景深度 pass），`stats.shadow_cascades` 记实际级数。
冒烟实测（4 物件 4 网格）：`级联 1 → 4 draws`、`2 → 8`、`4 → 16`，线性可验；关阴影时两者都是 0。

**已知边界**

- 级数上限 4（`MAX_CASCADES`），由 `texture_depth_2d_array` 层数与 shader 定长数组共同决定；
- 全部级共用一张图，因此**分辨率按级摊薄**：4 级 2048² 等于每级 2048²，但总显存与绘制开销 ×4；
- 级间混合是「采两级取平均」，不是硬件过滤，两级覆盖率差异大时仍有轻微接缝；
- 局部光源阴影（扩展十八 C）是**独立一张图**（24 层、512²、每帧最多 4 盏投影，见 §5.12），两者互不影响：
  关掉 `cast_shadow` 的局部灯不产生任何额外 pass，也不会因为级联调整而改变；
- 按级裁剪已落地（扩展二十四），级内判定改走场景树层级（扩展二十五）：每级只提交落在本级覆盖范围内的遮挡体，
  整棵子树在级外就整枝跳过。**剩余优化点**是判定仍在 CPU 侧做盒测试、没有 GPU 代理预筛、
  遮挡体不变时每帧重判一遍（掩码未跨帧复用），见 §11.3 第 6 条。

---

### 5.12 局部光源阴影（点光立方 / 聚光单面，扩展十八 C）

级联阴影只解决方向光。一盏台灯照在桌面上却没有影子，物件看起来像贴纸——扩展十八 C 补这一块，
但**不**给每盏灯配独立立方图（16×6 面 = 96 次全场景重绘，代价远超收益），而是：
**一张 24 层深度数组 + 每帧最多 4 盏投影灯 + 每面一次 pass**。

**CPU 侧契约（`r3e-core::light`，零 wgpu 依赖）**

| 项 | 内容 |
| --- | --- |
| 新字段 | `LocalLight` 增 `cast_shadow: bool`（缺省 `false`）与 `shadow_bias: f32`（夹 `0..MAX_LOCAL_SHADOW_BIAS = 0.05`，`sanitized()` 一并收敛） |
| 预算 | `MAX_LOCAL_SHADOW_CASTERS = 4`、`POINT_SHADOW_FACES = 6`、`MAX_LOCAL_SHADOW_LAYERS = 4 × 6 = 24`、`DEFAULT_LOCAL_SHADOW_SIZE = 512`；层数组长度是**编译期常量**，`shader.wgsl` 的 `array<mat4x4<f32>, 24>` 与之硬对齐 |
| 选灯 | `plan_local_shadows(&[LocalLight]) -> Vec<LocalShadowCaster>`：只挑 `cast_shadow`，按「辐亮度亮度 × `range²`」降序取前 4 盏（`total_cmp` + 下标兜底，同序输入必得同序输出），返回按 `light_index` 升序 |
| 槽位 | `layer_base = slot × 6`，聚光只用 `+0` 那一层、其余 5 层空着仍占位——「第 i 盏的第 f 面 = `layer_base + f`」，着色器零查表 |
| 选面 | `point_shadow_face(direction)`：主轴绝对值最大者胜出、符号定正负，顺序 `+X, -X, +Y, -Y, +Z, -Z`，与 `POINT_SHADOW_FACE_DIRS` / `POINT_SHADOW_FACE_UPS` 两张常量表逐项对应 |
| 面相机 | `shadow_face_view_projection(face)`：点光每面 90° 透视（`POINT_SHADOW_FOV_DEG`），聚光用「外角 ×2」作 FOV 并夹 `2..170`；近平面取 `max(MIN_LOCAL_SHADOW_NEAR, range × LOCAL_SHADOW_NEAR_RATIO)`，远平面 = `range`（`local_shadow_plane`） |

**GPU 侧布局**（`LocalShadowUniform`，仍在 `@group(3)`，不新增绑定组）

| 槽位 | 内容 |
| --- | --- |
| `binding 5` | `local_shadow_map: texture_depth_2d_array`（24 层，每层一个 `local_shadow_layer_views[i]` 作深度 pass 目标） |
| `binding 6` | `local_shadow_sampler: sampler_comparison`（PCF 用硬件比较采样，与方向光同型） |
| `binding 7` | `LocalShadowUniform { params: vec4, params2: vec4, view_proj: array<mat4x4, 24>, slots: array<vec4, 16> }` |

`params = (投影灯数, 1/size, 是否启用, _)`，`params2 = (阴影强度, 占用层数, _, _)`；
`slots[i] = (layer_base, 面数, 深度偏移, 法线偏移)`，`layer_base = -1` 表示这盏灯本帧不投影（初始值即 -1，
片元读到就早退，不依赖数组清零）。法线偏移按**该灯单纹素的世界尺寸**换算，所以半径 2 米和半径 20 米的灯
偏移量不同——同一套偏置在两种尺度下不可能都对。

**单帧顺序（`Renderer::render`）**

1. `prepare_shadow`（方向光级联）→ `prepare_local_shadow`（局部）：两者都只写 uniform，不算矩阵以外的事；
2. 方向光每级一次 `draw_shadow_pass` → 局部每层一次 `draw_shadow_pass`（label `r3e-local-shadow-pass`），
   层列表为空时整段跳过——**没灯投影的场景与扩展十七开销完全一致**；
3. 主 pass 一次绑 `group(3)`，片元里的局部光循环顺手取阴影。

**采样（`local_shadow_factor`）**

1. `params.z < 0.5` 或该灯 `slot.x < 0` → 返回 1.0（全亮）；
2. 点光（`slot.y > 1.5`）用 `-to_light` 选面并加到 `layer_base` 上，聚光直接取 `layer_base`；层号 `clamp(0, 23)`；
3. 世界位置先按 `slot.w` 沿法线推出，再用**该层自己的** `view_proj[layer]` 投影——矩阵由 CPU 一次算好、
   深度 pass 与主通道共用同一份，不存在「写读各算一次」导致的半像素偏移；
4. `clip.w <= 1e-6`、UV 出界、`ndc.z > 1` 一律返回 1.0（面边界邻域与视锥外按受光处理，与方向光同口径，避免立方体面接缝出现硬黑边）；
5. 3×3 PCF 后 `mix(1.0, lit, 强度)` 混回 1——局部阴影**只压该盏灯的直接光项**，环境光与其它灯不受影响。

**统计口径**：`stats.local_shadow_casters` = 本帧真正投影的灯数（≤ 4），`stats.local_shadow_layers` = 提交的深度 pass 层数
（点光 6 / 聚光 1），两者都计进 `stats.shadow_draws`。冒烟实测（起止各打一行探针）：`意愿 2 盏 → 本帧投影 2 盏 → 占用 7 层`
（1 点光 6 面 + 1 聚光 1 面），图幅 `512² × 24 层`。

**已知边界**

- 每帧只有 **4 盏**灯能投影，第 5 盏及以后即使 `cast_shadow = true` 也被静默丢弃（不做排队、不轮流）；
- 层数组固定 24 层 = 4 盏 × 6 面，**聚光与点光同占 6 层的槽位**，因此 4 盏聚光也只用到 4 层、其余 20 层闲置；
- 图幅固定 512²，不随方向光 `shadow.size` 变化（点光 6 层，跟着 4096 走会吃掉 1 GiB 以上显存），也**不**按灯的距离自适应；
- 阴影强度是全局标量（`params2.x` 当前恒 1.0），暂没有逐灯强度；
- 立方体面接缝处靠「邻域越界按受光」回避，没有做跨面连续 PCF，掠射角下仍可能看到一道轻微亮度线。

### 5.13 半透明排序与 MSAA（扩展十九）

两件事都收在渲染器内部，`r3e-core` 的场景结构与脚本 API 一字未改。

**A. 半透明按深度由远及近（`r3e-render/src/transparency.rs` + `renderer.rs`）**

前向管线没有逐像素排序，半透明只能靠"先画远的、后画近的"让 alpha 混合叠出正确结果；
顺序反了近处物体会被远处物体盖住。判定与排序都是纯函数，因而可脱离 GPU 单测：

| 函数 | 语义 |
| --- | --- |
| `is_transparent(alpha)` | `alpha < ALPHA_OPAQUE_THRESHOLD(0.999)` 才算半透明；`NaN` / 无穷按**不透明**处理（半透明通道不写深度，排序键里混进 `NaN` 会让"谁更远"失去意义） |
| `view_depth(world_pos, camera_pos)` | 排序键 = 距离**平方**，与真实深度同序但省一次 `sqrt` |
| `back_to_front(depths)` | 返回由远及近的下标序列；稳定排序（同深度保持场景顺序）→ 同帧可复现、不抖动；`NaN` 视为最远以保证比较器全序 |

批次的 alpha 取 **`node.color.w × 网格材质 alpha`**（两者都可能 < 1，只看节点色会漏判材质半透明）。
键表拆成 `opaque_keys` / `transparent_keys` 后拼接，于是 `groups` 天然是「不透明段 + 半透明段」，
绘制时只切一次管线：`transparent_start` = 不透明批次数，等于 `groups.len()` 表示本帧没有半透明物件。

排序分两层：**组间**按「组内最远实例深度」降序（一次排序决定先画哪个网格，保持合批），**组内**再按
`back_to_front` 排细。半透明批次因不写深度，`stats.transparent_draws / transparent_instances`
直接反映它是否真的存在——默认 `assets/scene.json` 全不透明，这两个值恒为 0。

**B. MSAA 4×（`MSAA_SAMPLE_COUNT = 4`，`r3e-render` 导出常量）**

主通道不再直接写 `postfx.hdr_view()`，而是光栅化到 4× 颜色附件、由硬件 resolve 进同一个 HDR 目标：

- **颜色附件**：`msaa_color_view`（`HDR_FORMAT` = `Rgba16Float`，4 样本），`resolve_target = postfx.hdr_view()`；
- **深度附件**：`msaa_depth_view`（4 样本，`LoadOp::Clear(1.0)` / `StoreOp::Discard`——只服务本通道深度测试，用完即弃）；
- **单采样深度仍保留**：遮蔽通道（扩展十七）要采样主通道深度，多重采样纹理不能当采样源，
  因此新增**深度预通道** `draw_depth_prepass` 先把不透明段的深度写进单采样 `depth_view`。
  它只画 `groups[..transparent_start]`（半透明本就不写深度），空批次也照常 clear 深度，避免上一帧残留；
- **后处理链零改动**：bloom / composite / FXAA 全部照旧读单采样 HDR 纹理，resolve 由硬件完成。

**代价与边界**

| 项 | 影响 |
| --- | --- |
| 显存 / 带宽 | 多一组 4× 颜色（`Rgba16Float`）+ 一组 4× 深度附件，1920×1080 下约 4×1920×1080×(8+4) ≈ 95 MiB 量级 |
| 额外 pass | 每帧多一次深度预通道，顶点开销 ≈ 不透明段重画一遍（片元只写深度） |
| 排序粒度 | 按**实例**（中心点距离）排序，不做逐三角形、不用 OIT；同一网格内的相交半透明仍可能出错 |
| 采样数 | 固定 4× 常量，不做运行时切换，也没有 per-scene 配置项 |
| 阴影通道 | 阴影与局部阴影 pass 仍是单采样深度（不需要颜色 resolve），未受影响 |

**取证口径**：`--run-seconds` 探针新增一行
`[起始] 半透明与 MSAA：采样数 4× / 透明 draw N / 透明实例 M / 深度预通道 draw K`；
编辑器 `engine_stats` 的 `render` 段同步新增 `msaa_samples` / `transparent_draws` / `transparent_instances` /
`depth_prepass_draws` 四个字段。

### 5.14 PBR 数据贴图在渲染侧的契约（扩展二十一）

core 侧只管"哪张图是什么用途"（§4.14），渲染侧负责"怎么绑、怎么采、怎么不出事"。

**绑定布局**：`@group(1)` 的 `binding 3/4/5` 依次是法线 / 粗糙度 / AO，采样器与 base color **共用**
`binding 2`（同一材质的贴图寻址与滤波需求一致，多绑三个采样器只是浪费 bind 额度）。
三张图**始终绑定**，缺图时垫 1×1 占位：法线用 `(128,128,255)` 的 `flat_normal`（线性格式，
避免 0.5 被 sRGB 解码成 0.21 把法线整体压弯），粗糙度与 AO 用 `white`。开关是否采样完全由
`params2.xyz` 决定，因此"没图"不改变任何像素 —— 这是 `R3E_VISUAL_REGRESSION=1` 基线能原样通过的前提。

**开关只认真绑上的图**：`resolve_pbr_map` 只有在 `texture_view` 真正返回视图时才把开关置 1。
超 `max_texture_dim`、像素不完整、`MeshProvider::texture` 返 `None` 时视图退回占位、开关置 0，
着色器走标量分支 —— 否则会出现"明明没贴图却去采样一张白色图把粗糙度乘成 1.0"的静默行为变化。

**色彩空间**：`TextureColorSpace` 参与纹理缓存键 `(asset, index, space)`，所以同一张图被一个材质当颜色、
被另一个材质当数据时会上传成两份而不是串用。上传格式：颜色 → `Rgba8UnormSrgb`（采样自动线性化），
数据 → `Rgba8Unorm`（原值直读）。

**采样通道**（与 glTF 约定一致，改这里会让所有 glTF 资产的观感漂移）：

| 贴图 | 通道 | 解码 |
| --- | --- | --- |
| 法线 | `xyz` | `sample * 2 - 1` 还原到 `[-1, 1]`，`z` 夹到 `≥ 0.05` 防贴图误差把法线翻到背面 |
| 粗糙度 | `G` | 作为**乘数**乘到 `params.y`；独立灰度图与 glTF 的 metallicRoughness 合并图都读 G |
| AO | `R` | 夹到 `[0, 1]`，**只乘环境光**（含 IBL 的漫反射 + 镜面项），不动直接光 |

**AO 只压环境光**是刻意的：直接光暗部已由阴影（阶段 D / 扩展十六 / 扩展十八 C）负责，两边都压等于
在接触处叠两层黑，出现"脏黑边"。自发光不吃 AO。**法线参与全部受光项**（方向光、局部光源、阴影查询、
IBL），即着色法线 `n` 与几何法线 `geometric_n` 分开保存：TBN 的切向基用几何法线构造（避免贴图自身的
倾向被二次投影），BRDF 与阴影查询用着色法线 —— 凹凸表面的阴影偏移跟着贴图走才不会有锯齿状的自遮挡线。

**切线兜底**：`Gram-Schmidt` 之后切线长度 `< 1e-5`（UV 退化、或内置网格只用缺省切线）时，
取与法线最不平行的轴（`|n.x| > 0.9` 用 +Y 否则 +X）重建，再投影一次。宁可切线方向不精确，
也不能让 `normalize(0)` 产出 NaN —— NaN 会在光照里传染成整片黑，比"切线不准"难排查得多。

**测试口径**（`crates/r3e-render/tests/pbr_maps_contract.rs`，9 条）：锁 uniform 64 字节 / `params2` 偏移 48；
锁"没图 → `params2` 全 0"；锁三张贴图的 binding 不重复、`params2.xyz` 三个分支都在；
锁 TBN 构造（`cross(geometric_n, t) * handedness`）与 `*2-1` 解码；锁粗糙度取 G、AO 取 R；
锁 `bump` 不算法线。这些写错都不会编译失败，只会在实机上表现为"贴图没效果"，所以用源码级断言钉住。

**已知边界**

- **不读金属度贴图**：glTF 的 metallicRoughness 合并图只用了 G（粗糙度），B（金属度）仍是标量；
- **只有一套 UV**（`uv0`），不支持 `KHR_texture_transform`、不做 UV 集选择；
- **切线在装配期一次算定**，不随形变更新：蒙皮网格的切线跟着关节矩阵近似变换（与法线同用伴随矩阵），极端形变下会有偏差；
- **数据贴图不生成 mipmap**，远距离采样会有闪烁，未做各向异性过滤；
- 法线贴图的 `z` 下限 `0.05` 会削弱强凹凸贴图的陡峭度，属于防翻面优先的取舍。

### 5.15 遮挡剔除在渲染侧的契约（扩展二十二）

core 侧只管「怎么判」（§4.15），渲染侧只解决一件事：**金字塔从哪来**。答案是把深度预通道写出的
单采样 `Depth32Float` 低频读回 CPU，压成 tile 金字塔，供**后续帧**判定。`occlusion.rs` 里只有
`OcclusionCuller` 一个结构，常驻在 `Renderer` 上。

**三条设计约束**：

1. **可开关即零成本**：`enabled = false` 时 `begin_frame` 返回 `false`，本帧不 poll、不编码 copy、不读回，
   `prepare_instances` 也整段跳过判定——GPU 命令与实例数与加这个功能之前逐字节一致。
   `reset()` 会连已有金字塔一起丢弃，避免下次打开时拿过期画面剔东西。
2. **低频 + 非阻塞**：读回是「发起 → 若干帧后取回」的两段式。`encode_readback` 只把 copy 编进本帧命令缓冲，
   `after_submit` 发起 `map_async`；结果在之后某帧开头用 `device.poll(PollType::Poll)` **非阻塞**地取，
   没取到就沿用上一份金字塔，绝不原地等 GPU（会卡帧）。间隔由 `readback_interval` 控制：
   `DEFAULT_READBACK_INTERVAL = 4`（60fps 下约 15Hz），可设 `1..=60`。
3. **保守 fail-open**：金字塔未就绪（首帧、刚 resize、读回失败）、尺寸与当前画面对不上时一律判可见。

**读回布局**（`ReadbackLayout`）：`width × height × 4` 字节按 `COPY_BYTES_PER_ROW_ALIGNMENT`（256 字节）
行对齐，1080p ≈ 8.3 MiB/次；`width/height` 为 0 或乘积溢出时返回 `None`，本轮不发起读回。

**接入点**（`renderer.rs::prepare_instances` 阶段三 A）：视锥剔除之后、分配实例槽位之前。

| 位置 | 行为 | 理由 |
| --- | --- | --- |
| `scene_bounds` 统计 | 在视锥 / 遮挡剔除**之前** | 阴影拟合要考虑镜头外的遮挡物，被视锥或遮挡剔掉的物件仍应撑大阴影相机 |
| 蒙皮网格 | 先 `inflate_relative(SKIN_CULL_MARGIN)` 再判 | 形变会跑出静止姿态 AABB |
| 半透明批次 | **不参与**遮挡剔除 | 半透明不写深度，金字塔里没有它们的贡献，拿不透明深度去剔会让玻璃 / 水面整层凭空消失 |
| 拿不到局部 AABB | 不剔除，计入 `cull_unknown` | 与视锥同口径的 fail-open |

**统计契约**（`RenderStats::occlusion`，`OcclusionStats`）：

| 字段 | 含义 |
| --- | --- |
| `enabled` | 本帧是否**真的在判**（开关打开且金字塔就绪） |
| `tested` | 本帧做过遮挡判定的实例数（已通过视锥的那部分） |
| `occluded` | 本帧因遮挡被剔除的实例数；0 表示没剔到或开关关闭 |
| `readback` | 本帧是否取到了新金字塔（低频读回，多数帧为 `false`） |
| `levels` | 当前金字塔层数；0 = 还没有可用金字塔 |

`engine_stats` 的 `render` 键里透出这五个字段，脚本侧 `occlusion_enabled()` / `occlusion_bias()`
读同一份设置（写侧 `set_occlusion_enabled` / `set_occlusion_bias`，写入即过 `sanitized()`）。

**测试口径**（`crates/r3e-render/src/occlusion.rs`，6 条，纯 CPU）：`ReadbackLayout` 拒退化尺寸、
行按 256 对齐、恰好整数倍时不多补一行；`readback_interval` 夹到 `1..=60`；
新建的 `OcclusionCuller` 是 fail-open（无金字塔、`levels = 0`）；`reset()` 同时清金字塔与统计。

**已知边界**

- **只读单采样深度**：MSAA 下的深度是多采样目标，这里读的是深度预通道的单采样视图；
- **一次只允许一份在读回中**（`inflight`）：上一次没取回就不发起新的 copy，所以读回间隔的最坏延迟是「间隔 + 一次往返」；
- **不做遮挡体筛选 / 迟滞**：上一帧画过什么就全部作为潜在遮挡体，被剔物件下一帧可能又回来（见 §4.15）；
- **CPU 判定按实例串行**：实例量大时判定成本线性上升，`max_tiles` 只压单次判定的上界；
- **窗口 resize 后有一帧空窗**：尺寸对不上时整帧 fail-open，等下一次读回建好金字塔才恢复剔除。

### 5.16 HDR 反射探针在渲染侧的接线（扩展二十三）

core 侧给了成品 mip 链（§4.16），渲染侧负责三件事：**上传成一张立方体纹理**、**绑到材质组的固定槽位**、
**在着色器里按粗糙度取层**。整条链路只在场景加载 / 换探针时执行，**不进每帧循环**。

**① 绑定槽位**：`envprobe::ENV_CUBE_BINDING = 6`，落在**材质绑定组 `@group(1) @binding 6`**（`texture_cube<f32>`，
采样器复用 base color 的 `s_base_color`）。选址理由与扩展十五同一条：WebGPU 的 `max_bind_groups = 4`
已经被相机 / 材质 / 场景 / 光源组占满，第五组过不了校验；而按材质分桶的组本来就是「逐次绘制会换的组」，
加一张贴图不会牵动别的组。

**② 上传**：`set_env_probe(Some(&EnvProbeSource))` 建 `Rgba16Float` 立方体纹理
（`depth_or_array_layers = CUBE_FACES`、`mip_level_count = mip_levels`、`TEXTURE_BINDING | COPY_DST`），
然后按 `encode_faces(prefiltered)` 的条目逐 (mip, face) 做 `write_texture`——每条上传自带
`bytes_per_row() = size * 8`（f16 RGBA 无行距 padding，正好能吃）。f32 → f16 是 crate 内自实现的位运算
（`f32_to_f16` / `f16_to_f32`），保住 4.0 / 2.0 / 1.0 这类常用值的精确往返，溢出走 `+inf` 而不是饱和值。
预滤波层数多于纹理层数时多出来的层直接丢弃。换图后必须 `material_cache.set_env_cube(view)`
**重建全部材质 bind group**（视图写在组里）。

**③ 卸载**：`clear_env_probe()` / `set_env_probe(None)` 把视图换回 1×1 的占位立方体
（`DUMMY_CUBE_FACE_SIZE = 1`）、`env_probe.x` 归 0——**着色器因此一次都不采样** `t_env_cube`，
镜面反射走回扩展十二的解析天空，画面逐像素不变。

**④ uniform 与着色器**：`CameraUniform` 从 352 → **368 字节**，末位新增 `env_probe: vec4<f32>`
（`x` = 是否启用 0/1、`y` = 镜面 mip 层数、`z` = 强度、`w` = 面边长，仅诊断用）。
片元阶段：

| 分支 | 条件 | 行为 |
| --- | --- | --- |
| 探针 | `env_probe.x > 0.5` | `probe_specular(dir, roughness)`：`lod = roughness × (levels - 1)`（与 CPU `envprobe::mip_lod` 同式），`textureSampleLevel` 取层后 `max(rgb, 0) × intensity` |
| 解析 | 否则 | 原 `env_radiance`（三色天空 + 太阳高光），与扩展十二逐字节一致 |

**漫反射不新增绑定**：配了探针时 `env_sh` 9 项系数换成**探针第 0 层的辐照度投影**
（`PrefilteredEnv::base().project_irradiance()`），球谐分支一行未改；用最清晰那层而不是模糊层，
是为了避免「已经按粗糙度卷积过再积分」的双重衰减。

**⑤ 统计与日志**（`EnvProbeStats`）：`active` / `source`（`faces` \| `equirect` \| `disabled` \| `none`）/
`face_size` / `mip_levels` / `intensity`。`--run-seconds` 探针与编辑器 `engine_stats.render.env_probe`
输出同一份；`Renderer::env_probe_stats()` 在没配探针时给全零缺省。

**测试口径**（`crates/r3e-render/src/envprobe.rs` 7 条 + `crates/r3e-render/tests/env_probe_shader.rs` 9 条 +
`renderer.rs` 内 1 条）：f16 往返 / 溢出 / 次正规边界、`encode_faces` 的面数与层序、每行字节数、
`mip_lod` 与层布局一致（含单层链与 NaN）、naga 无窗口校验 `shader.wgsl`（入口点、`@group(1) @binding(6)`
绑定声明、`env_probe` 四分量语义、`probe_specular` 的 lod 表达式与 CPU 常量对齐）。

**已知边界**

- **探针是静态的**：没有每帧 / 每 N 帧重抓场景的反射捕捉，也没有平面反射与 SSR，物件移动不反映到金属上；
- **整场景一张图**：没有区域化探针与探针混合，进屋不会自动换环境；
- **一次 `write_texture` 一条**：`6 × mip_levels` 次上传在加载时完成，不支持运行时流式更新（也没有 mip 之外的额外链）；
- **依赖材质组重建**：换探针会重建整批材质 bind group，属可预期的偶发停顿，不放在帧循环里就不会掉帧。

---

## 6. 物理层：r3e-physics

> 完整用法、规则与实测数据见 [`docs/PHYSICS.md`](PHYSICS.md)。本节只讲架构。

### 6.1 设计目的：契约先于实现

物理层被切成**两层**，互不耦合：

1. **契约层**：`PhysicsBackend` trait，让场景格式（`physics` 段）与脚本 API（`attach_body` 等）先能写。
2. **实现层**：默认 `NullBackend`（空转）/ `BuiltinBackend`（自实现最简物理）/ `RapierBackend`（真实后端，`rapier` feature 才编译）。

好处是「有没有物理」变成**运行时开关**，而不是编译期分叉：同一份场景与脚本，开物理会掉、关物理由脚本驱动。

### 6.2 后端契约

```rust
pub trait PhysicsBackend {
    fn name(&self) -> &'static str;
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;
    fn destroy_body(&mut self, handle: BodyHandle);
    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3);
    fn set_gravity(&mut self, gravity: Vec3);
    fn step(&mut self, dt: f32);
    fn sync_to_scene(&mut self, scene: &mut Scene);
    fn body_count(&self) -> usize;
    // 可选（有默认实现，内置后端会覆盖）
    fn set_ground(&mut self, ground: f32) {}
    fn pull_from_scene(&mut self, scene: &Scene) {}
    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> { None }
}
```

职责边界写得很清楚：**后端负责积分与碰撞，并负责把结果写回 Scene**（`sync_to_scene`）。
上层不关心物理怎么算，只在帧序里调 `step(dt, &mut scene)`。

`set_ground` / `pull_from_scene` 之所以放进 trait 而不是内置后端私有，是为了让 Rapier 之类外部后端也能接：
前者映射到地面体，后者映射到 `sync_back`/读回变换。

| 类型 | 内容 |
| --- | --- |
| `BodyHandle(u64)` | 后端分配的句柄，写回 `Node::body` |
| `BodyKind` | `Static` / `Dynamic` / `Kinematic`，`parse` 支持 `fixed` 别名 |
| `ColliderShape` | `Box` / `Sphere` / `Capsule` / `Cylinder` |
| `BodyDesc` | kind / shape / half_extents / mass / restitution(0.2) / friction(0.6) / position / rotation / scale |

`BodyDesc::infer(shape, scale)` 从网格与缩放推导碰撞体：球 → 半径 `scale.x/2`，柱/胶囊 → 半高 `scale.y/2`，
其余 → 盒 `scale/2`。脚本 `attach_body(name, kind, mass)` 走这条路，所以 `create_node` 出来的物件可直接挂刚体。

### 6.3 三种后端

| 后端 | 状态 | 行为 |
| --- | --- | --- |
| `NullBackend` | 默认 | 只分配句柄并登记，`step` / `sync_to_scene` 空转。用于「只想在脚本里表达物理意图、暂不模拟」 |
| `BuiltinBackend` | 可用 | 半隐式欧拉 + 无限地面平面碰撞 + 弹性 + 摩擦 + 休眠，纯自实现零依赖，约 400 行 |
| `RapierBackend` | 可用（`--features rapier`） | Rapier3D 真实物理：物件互撞 / 堆叠 / 旋转动力学 / 碰撞事件 / CCD / 射线-重叠-扫掠查询，约 520 行。未用该 feature 构建时 `physics_set_backend("rapier")` 返回 false 并打印「未编译进本构建」，不静默降级 |

`NullBackend` 的 `take_all_handles` 返回 `Some(列表)`，让 `Physics::clear_bodies` 能整体清空——热重载不留幽灵句柄。

### 6.4 Physics 门面：开关 + 世代号

```rust
pub struct Physics {
    backend: Box<dyn PhysicsBackend>,   // 动态分发，运行时可换
    kind: PhysicsKind,                  // Null / Builtin / Rapier
    enabled: bool,                      // 总开关
    pub gravity: Vec3,                  // 默认 (0, -9.81, 0)
    ground: f32,                        // 地面平面高度，默认 0
    generation: u64,                    // 配置世代号
    overrides: PhysicsOverrides,        // 命令行粘性覆盖
}
```

- **`enabled=false` 时**：`step` 立即返回、`create_body` 直接返回 `false`、`body_count()` 报 0。
  上层无需到处写 `if`，开关语义收敛在门面里。
- **`set_enabled(true)` 且后端仍是 `null`** → 自动升级到 `builtin`。避免「开了却没动」。
- **`generation` 自增**于开关切换、后端重建、`clear_bodies`。`app` 只在世代变化时重绑场景刚体，
  所以脚本手动 `attach_body` 的刚体不会被每帧覆盖。
- **`overrides` 记命令行值**，热重载重放场景 JSON 后重新合并，实现「命令行粘性、场景层随文件变、脚本改动易失」。
- `step(dt, scene)` 串联 `pull_from_scene` + `backend.step(dt)` + `backend.sync_to_scene(scene)`，
  并统一 `update` 重力/地面到后端。调用方只有一个入口。
- 手工实现 `Debug`（`Box<dyn Trait>` 不自动实现 Debug）。

三层配置（命令行 / 场景文件 / 内置默认）的合并逻辑放在 `PhysicsConfig::from_scene` + `apply_config` / `apply_override`，
放在 `r3e-physics` 而不是 `app`，是为了让无窗口的测试也能覆盖优先级。

### 6.5 app 层：自动绑定

`app` 每帧在脚本之后、物理之前调 `sync_physics_bindings`：

1. 世代号没变 → 什么都不做（零成本）。
2. 世代号变了（开关翻转 / 后端切换 / 热重载）→ 遍历场景，把**声明了 `physics` 段**的物件逐个 `attach_body` 式绑定。
3. 纯装饰物件（无 `physics` 段）永远不由引擎自动绑，只能脚本显式 `attach_body`。

这条规则让「场景是静态描述、脚本是行为」的分工成立：场景写 `"physics": {...}` 就该掉，
没写就归脚本管。窗口标题实时显示 `物理 builtin / N 刚体` 或 `物理关`。

### 6.6 外部后端接入：Rapier（已落地）

这条路在阶段一（Rapier 后端）与阶段七（CCD）里走完了，改动面确实只有下面三处，其余零改动：

1. `crates/r3e-physics/src/rapier_backend.rs`（约 520 行）实现 `PhysicsBackend`：`BodyDesc.shape` → `SharedShape`，
   `ColliderShape::Capsule` / `Cylinder` → 对应 `ColliderBuilder`，`BodyKind` → `RigidBodyType`，
   `step` 内 `pipeline.step(&gravity, ...)`，`sync_to_scene` 内把 `translation` / `rotation` 写回节点，
   `update_world_transforms` 仍由 `app` 统一调。
2. `PhysicsKind::Rapier` 分支接上真实实例（`Physics::rebuild_backend` 里一行），能力位由 `BackendCaps` 自报
   （`EXPOSED` 放开到 `CONTACT_CCD`，只留 `joints` 一个禁区）。
3. `Cargo.toml` 把 `rapier3d` 挂成**可选依赖**（`rapier = ["dep:rapier3d"]`），默认构建仍然零物理依赖。

场景格式、脚本 API、`app` 层、`core::Scene` **均无需改动**。
唯一语义差异：外部后端做物件间碰撞与旋转动力学，内置后端不做——属能力升级，不是接口破坏。

---

## 7. 脚本层：r3e-script

### 7.1 共享真源的实现方式

```rust
pub struct ScriptHost {
    engine: Engine,
    clock: Rc<RefCell<Clock>>,
    scene: Rc<RefCell<Scene>>,       // 与 app 层共享同一份
    physics: Rc<RefCell<Physics>>,   // 与 app 层共享同一份
    ast: Option<AST>,
    scope: Scope<'static>,           // 持久作用域：跨帧保存脚本全局变量
    script_path: Option<PathBuf>,
    script_mtime: Option<SystemTime>,
    base_scene_json: Option<String>, // 热重载时用于重放
    pub last_error: Option<String>,
}
```

`Rc<RefCell<Scene>>` 是关键：脚本执行时**直接修改主循环持有的那份场景**，不产生副本，不需要"提交"步骤，因此改动在下一帧渲染立即可见。

代价是 `RefCell` 的运行时借用检查——API 绑定里必须小心避免重入 `borrow_mut()`（`attach_body` 里先 `borrow()` 读完信息、`drop(scene)`、再 `physics.borrow_mut()`，就是这个原因）。

### 7.2 脚本执行模型

**加载**（`load_str`）：

```
1. engine.compile(src)                        → AST，语法错误在此暴露
2. scope.clear()                              → 清空上次的全局变量
3. engine.run_ast_with_scope(&mut scope, &ast) → 执行顶层语句
4. ast 存起来
5. 若存在 on_start()（零参数）→ engine.call_fn("on_start")
```

**每帧**（`tick(dt, time, frame)`）：

```
1. 更新 Clock { dt, time, frame }
2. 若有 AST 且定义了 on_update(dt) → engine.call_fn("on_update", (dt,))
3. 出错写入 last_error，且同一条错误只打印一次（避免刷屏）
```

**顶层语句会被重复执行的陷阱**（已在代码注释中记录）：

Rhai 的 `call_fn(scope, ast, ...)` 会连同 AST 的**顶层语句**一起执行。因此如果把 `create_node("a","cube",0,0,0)` 写在顶层而不是 `on_start` 里，每帧都会执行一次——结果是第一帧后全部报"物件已存在"。这是当前最容易踩的坑，而 `spawn` 的重名校验恰好把它从"静默内存泄漏"变成了"明确报错"。

**函数存在性检查**（`has_fn`）在调用前完成：`ast.iter_functions().any(|f| f.name == name && f.params.len() == arity)`。因此脚本不写 `on_update` 也完全合法，引擎不会报错，只是物件静止。

### 7.3 热重载

```rust
pub fn reload_if_changed(&mut self) -> bool {
    // 1. 读 mtime，与记录值相同 → return false
    // 2. 重放基础场景：base_scene_json → Scene::from_json_str → 覆盖当前场景
    //    （无基础场景则 clear_nodes）
    // 3. physics.clear_bodies() 清空幽灵刚体
    // 4. load_file(path)：重新编译 + 重跑顶层 + on_start
    // 5. 成功打印"热重载完成"
}
```

**"重放基础场景"是热重载正确性的核心**：如果不还原场景，上一次脚本已经生成的物件会残留在场景里，新脚本再 `create_node` 同名物件就会撞重名。所以每次热重载都从 JSON 干净重建，等价于"重启进程但保留窗口与 GPU 状态"。

`rebuild_scene(json)` 走同一套重放逻辑，只是场景来源换成新文件内容——这样改 `scene.json` 和改 `main.rhai` 的行为一致。

### 7.4 引擎 API 绑定（完整清单）

所有函数通过 `engine.register_fn` 注入，命名风格偏"短动词 + 按名寻址"，因为脚本是手写的高频输入。

**输出**

| 函数 | 说明 |
| --- | --- |
| `log(msg)` | 打印 `[script] msg` |

**生命周期**

| 函数 | 签名 | 说明 |
| --- | --- | --- |
| `create_node(name, mesh, x, y, z)` | → bool | 生成物件 |
| `create_node_at_origin(name, mesh)` | → bool | 原点生成 |
| `create_node_json(json)` | → bool | 用单物件 JSON 生成，字段同场景文件 |
| `remove_node(name)` | → bool | 移除（含子物件） |
| `remove_nodes(prefix)` | → i64 | 按名前缀批量移除，返回移除数 |
| `exists(name)` | → bool | 是否存在 |
| `count()` | → i64 | 物件总数 |
| `names(prefix)` | → Array | 前缀匹配的名字列表 |

**变换**

| 函数 | 说明 |
| --- | --- |
| `set_pos(name, x, y, z)` | 设置位置 |
| `move_by(name, dx, dy, dz)` | 相对位移 |
| `get_pos(name)` | 返回 `[x, y, z]`，不存在返回 `[0,0,0]` |
| `set_rot(name, x, y, z)` | 设置欧拉角（角度制） |
| `rotate(name, x, y, z)` | 在当前欧拉角上叠加 |
| `set_scale(name, x, y, z)` | 非均匀缩放 |
| `set_scale_uniform(name, s)` | 均匀缩放 |

**外观**

| 函数 | 说明 |
| --- | --- |
| `set_color(name, r, g, b)` | 保留原 alpha |
| `set_color4(name, r, g, b, a)` | 含 alpha |
| `set_visible(name, bool)` | 显隐 |
| `set_mesh(name, mesh)` | 换网格类型（运行时变形） |

**相机 / 光照 / 环境**

| 函数 | 说明 |
| --- | --- |
| `camera(px, py, pz, tx, ty, tz)` | 直接设置位置与注视点 |
| `orbit(yaw_deg, pitch_deg)` | 轨道旋转 |
| `fov(deg)` | 视场角 |
| `set_light(dx, dy, dz)` | 光照方向（自动归一化，零向量忽略） |
| `set_light_color(r, g, b, intensity)` | 光色 + 强度 |
| `set_ambient(r, g, b)` | 环境光 |
| `set_background(r, g, b)` | 背景色 |
| `set_shadow(on)` / `shadow_enabled()` | 阴影总开关（关闭后整帧跳过阴影通道，零开销） |
| `set_shadow_strength(v)` | 阴影强度 0..1，`mix(1.0, visible, v)` |
| `set_shadow_size(px)` | 阴影贴图边长，夹到 64..8192，变化时重建深度纹理 |
| `set_shadow_extent(half)` | 正交半宽，0 = 按场景包围球自适应；开级联时作单级半宽上限 |
| `set_shadow_cascades(n)` / `shadow_cascades()` | 级联数 1..=4（写 `1` 即退回阶段 D 的单级阴影） |
| `set_shadow_blend(v)` | 级间过渡带宽（占该级跨度比例），夹到 0..0.5 |
| `set_shadow_distance(m)` | 阴影最远距离（世界单位，沿相机前后）；0 = 用相机 `zfar` 或场景包围球最远点 |
| `set_shadow_caster_extend(m)` | 扩展二十四：级联近端沿光方向外扩米数，夹到 0..=`MAX_CASTER_EXTEND`；0 = 紧贴切片（缺省） |
| `set_shadow_caster_cull(on)` / `shadow_caster_cull()` | 扩展二十四：按级裁剪遮挡体开关，缺省开；关掉即回到「每级全量提交」 |

**材质（PBR）**

| 函数 | 说明 |
| --- | --- |
| `set_material(name, metallic, roughness)` | 物件级材质，写入前由 `SurfaceMaterial::sanitized` 收敛；物件不存在返回 `false` |
| `set_emissive(name, r, g, b)` | 自发光（线性 HDR，每通道夹到 0..16），只加不减 |
| `material_of(name)` | 读回收敛后的快照：`found` / `metallic` / `roughness` / `emissive` |
| `clear_material(name)` | 删除物件级材质，回到缺省（`metallic 0` / `roughness 0.6` / 无自发光） |

**环境探针（IBL）**

| 函数 | 说明 |
| --- | --- |
| `set_sky(zenith r,g,b, horizon r,g,b, ground r,g,b)` | 一次写三色天空；缺省 `EnvironmentSettings` 打底后写场景 `environment` |
| `set_env_sun(color r,g,b, intensity, dir x,y,z)` | 太阳色 / 强度（夹 0..16）/ 指向太阳的方向（写入即归一化） |
| `set_env_intensity(v)` | 环境总强度，夹 0..8；无环境段时先建缺省再写 |
| `has_environment()` | 场景是否带环境段（`None` 为 `false`） |
| `clear_environment()` | 删掉环境段，退回 `light.ambient` 常数项 |

**HDR 立方体反射探针**

| 函数 | 说明 |
| --- | --- |
| `set_env_map(path)` | 等距柱状单张 `.hdr` 作来源（缺省 `face_size 128` / `mip_levels 6` / `intensity 1.0`）；空串等于清掉探针。返回「写入后是否真的激活」 |
| `set_env_map_faces(paths)` | 立方体六面路径，**按 `+X -X +Y -Y +Z -Z` 顺序**；少于 6 条整段作废并返回 `false` |
| `set_env_map_intensity(v)` | 探针强度，夹 `0..=MAX_IBL_INTENSITY`（无探针时先建缺省再写），返回收敛后的值 |
| `set_env_map_enabled(on)` | 只切开关，不丢来源（关掉再打开画面立刻回到探针） |
| `clear_env_map()` | 删掉整段 `environment_map`，退回扩展十二的解析 IBL |
| `has_env_map()` | 探针是否**真的生效**（开关打开且来源可用，同 `is_active()`）；不是「有没有写过」 |
| `env_map_source()` | 来源标签 `none` / `disabled` / `equirect` / `faces`，编辑器面板直接显示 |
| `env_map_intensity()` | 读回收敛后的强度，没配探针给缺省 `1.0` |

**HDR 后处理**

| 函数 | 说明 |
| --- | --- |
| `set_postfx(exposure, tonemap, bloom, intensity, fxaa)` | 一击式全量：EV / 曲线名 / 泛光开关 / 泛光强度 / FXAA |
| `set_exposure(ev)` | 曝光，夹 -8..16 |
| `set_tonemap(name)` | `none` / `reinhard` / `aces`；未知名回落 `none` 并打印一行（脚本不该因拼写中止） |
| `set_bloom(on, threshold, knee, intensity)` | 阈值夹 0..64、软膝 0..8、强度 0..4 |
| `set_fxaa(on)` | 在 composite 里做，零额外 pass |
| `has_postfx()` / `clear_postfx()` | 是否带后处理段 / 清空回直通 |
| `postfx_exposure()` / `postfx_tonemap()` | 回读（未开启时给缺省 0 / `none`） |

**屏幕空间遮蔽（扩展十七）**

写入 `postfx.ssao`（没有 `postfx` 段时自动建一个），渲染层下一帧重建 AO 图后生效。

| 函数 | 说明 |
| --- | --- |
| `set_ssao(enabled, radius, bias, intensity, samples)` | 一击式全量：开关 / 半径(夹 0.01..8) / 偏置(0..0.5) / 强度(0..4) / 样本数(4..16) |
| `set_ssao_enabled(on)` | 只要开关；关掉后两个 pass 整段跳过 |
| `set_ssao_radius(v)` / `set_ssao_intensity(v)` / `set_ssao_samples(n)` | 单字段写，逐个走 `sanitized()`（样本数按整数夹） |
| `has_ssao()` / `clear_ssao()` | 是否带遮蔽段 / 清空回中性（遮蔽图全 1） |
| `ssao_enabled()` / `ssao_radius()` / `ssao_intensity()` / `ssao_samples()` / `ssao_bias()` | 回读（未开启时给 `SsaoSettings::default()`：false / 0.6 / 1.0 / 8 / 0.02） |

**动画混合树（扩展十八 A）**

写入 `AnimInstance.layers`（`Scene::add_anim_layer` 按层名覆盖，满 8 层返回 `false` 不顶人）。
遮罩用逗号串表达：`"arm,hand"` 只作用于这两处骨骼，`"!root"` 表示除 root 外全部，
空串或 `"*"` 表示整副骨架；层按挂载顺序叠加。

| 函数 | 说明 |
| --- | --- |
| `add_anim_layer(实例, 层, 片段, 权重)` / `add_anim_layer_additive(...)` | 挂一层，前者 `override`、后者 `additive`；片段名按实例前缀解析 |
| `remove_anim_layer(实例, 层)` / `clear_anim_layers(实例)` | 删一层 / 清空该实例全部层 |
| `set_anim_layer_weight(实例, 层, w)` / `fade_anim_layer(实例, 层, w, 秒)` | 直接设权重 / 带 smoothstep 过渡地改权重 |
| `set_anim_layer_mode(实例, 层, 模式)` | `override` / `additive`，大小写不敏感；不认识时打日志并返回 `false` |
| `set_anim_layer_mask(实例, 层, 串)` / `anim_layer_mask(实例, 层)` | 设遮罩 / 回显（全通过为 `"*"`，否则按 `include,!exclude` 还原） |
| `set_anim_layer_speed(实例, 层, 倍率)` | 层自己的播放倍率（与主片段的 `set_anim_speed` 互不影响） |
| `add_anim_layer_clip(实例, 层, 片段, w)` / `set_anim_layer_clip(实例, 层, 片段)` | 层内加一路片段（走 / 跑混合）/ 整体换成单片段 |
| `remove_anim_layer_clip(实例, 层, 片段)` | 层内移除一路片段（最后一路不可移除） |
| `anim_layers(实例)` / `anim_layer_count(实例)` | 层名列表（挂载顺序）/ 层数 |
| `anim_layer_info(实例, 层)` | 整层快照（模式 / 权重 / 有效权重 / 遮罩 / 片段与归一权重 / 时间 / 速度 / 循环 / 播放中） |
| `anim_layer_weight(实例, 层)` / `anim_layer_clips(实例, 层)` | 有效权重（含过渡）/ 层内片段名列表 |

**场景切换与存档（扩展十八 B）**

这一组函数全是**申请**而不是**执行**：参数被塞进宿主的单槽，由 `EngineApp` 在**帧首**统一落地
（原因见 §8.16）。因此它们在 `on_start` / `on_update` 里随便调都不会把世界撕成两半，
返回值 `true` 只表示"这一帧的换世界请求收下了"。

| 函数 | 说明 |
| --- | --- |
| `scene_switch(path)` | 申请切到另一个场景文件；`path` 为空白返回 `false`，槽已占也返回 `false` |
| `scene_back()` | 申请回到栈里上一层（不删层，可再 `forward` 回来）；已在第 0 层时命令照收、帧首报错打日志 |
| `scene_save_archive(path)` | 申请存档；`path` 传空串 = 让宿主挑默认位置（`<场景名>.archive.json`，与场景文件同目录） |
| `scene_load_archive(path)` | 申请读档；`path` 为空白返回 `false` |
| `scene_command_pending()` | 槽里是否已有待落地命令（脚本配合限流用：`if !scene_command_pending() { ... }`） |
| `scene_stack_names()` | 层名数组（按压栈顺序） |
| `scene_stack_depth()` / `scene_stack_index()` | 层数 / 当前层号；**空栈的 `index` 是 `-1`**，与"第 0 层"区分开 |
| `scene_stack_current()` | 当前层名；空栈返回 `""` |

**局部光源（扩展十五）**

写入 `Scene::lights`（`upsert_local` 按 key 覆盖，满 16 盏返回 `false` 不顶人），渲染层下一帧自动打包上传。
`light_*` 是写，`local_light_*` 是读——读侧走 CPU 参考实现，返回值与 GPU 算的一致。

| 函数 | 说明 |
| --- | --- |
| `light_add_point(key, x, y, z, r, g, b, intensity, range)` | 建 / 覆盖一盏点光（9 参数，字段跑 `sanitized`） |
| `light_add_spot(key, x,y,z, dx,dy,dz, r,g,b, intensity, range, inner_deg, outer_deg)` | 建 / 覆盖一盏聚光（13 参数，方向自动归一化，内外角自动收敛） |
| `light_set_pos(key, x, y, z)` / `light_set_dir(key, x, y, z)` | 改位置 / 聚光朝向 |
| `light_set_color(key, r, g, b)` / `light_set_intensity(key, v)` / `light_set_range(key, v)` | 改颜色 / 强度 / 影响半径 |
| `light_set_cone(key, inner_deg, outer_deg)` | 改锥角（自动保证内角比外角小至少 0.1°） |
| `light_set_cast_shadow(key, on)` / `light_set_shadow_bias(key, v)` | 扩展十八 C：允许这盏灯投影 / 改深度偏移（偏移夹 `0..0.05`）。`on` 只是**意愿**——是否真投影由渲染层按亮度取前 4 盏决定 |
| `light_remove(key)` / `light_clear()` | 删一盏 / 清空（返回删除数） |
| `set_lights_enabled(on)` / `lights_enabled()` | 总开关：关掉只是 `count.x = 0`，灯的数据留着 |
| `local_light_count()` / `local_light_keys()` / `local_light_exists(key)` | 数量 / key 列表 / 是否存在 |
| `local_light(key)` | 整盏快照（`found` / `kind` / `pos` / `color` / `intensity` / `range` / `dir` / `inner_deg` / `outer_deg` / `cast_shadow` / `shadow_bias`） |
| `local_light_intensity(key)` / `local_light_kind(key)` | 单字段快捷读（不存在时返回 0 / `""`） |
| `local_light_cast_shadow(key)` | 该灯的**投影意愿**（场景里写的值），不是本帧实际是否投影 |
| `local_shadow_caster_count()` | 本帧真正在投影的灯数（渲染层统计，≤ 4）——与上一条区分开，脚本不用自己复算亮度排序 |
| `local_light_irradiance(x, y, z)` | 世界点上所有局部光源的累加辐照度（CPU 参考：`LocalLight::irradiance`） |

**时钟与随机**

| 函数 | 说明 |
| --- | --- |
| `time()` | 引擎时间（秒，f64） |
| `frame()` | 帧序号 |
| `dt()` | 上一帧耗时（秒） |
| `fps()` | 上一帧瞬时帧率（`1/dt`，首帧 0），由 `app` 每帧算好经 `ScriptRuntime::tick(dt, fps)` 传入 |
| `rand(min, max)` | xorshift64 伪随机，返回 `[min, max)` |

随机数用自实现的 xorshift64（种子 `0x2545F4914F6CDD1D`）而非 `rand` crate，是为了不引入额外依赖并保证可复现——种子固定，每次启动的随机序列相同。

**屏幕空间文本（M1）**

写入 `Scene::texts`，渲染层下一帧自动重排重画；**按 `key` 寻址、同名即覆盖**，所以脚本每帧调用也不会堆积节点。
坐标单位是窗口像素、原点左上；`(x, y)` 是文本框第一行的**行顶**锚点。

| 函数 | 说明 |
| --- | --- |
| `text_add(key, content, x, y, size, r, g, b)` | 新建；key 已存在时只更新内容/位置/字号/颜色并返回 `false`（保留对齐、换行、行高） |
| `text_set(key, content)` | 改内容（最常用，单独给最简签名） |
| `text_set_pos(key, x, y)` / `text_set_size(key, size)` | 改位置 / 字号（字号夹到 1..512） |
| `text_set_color(key, r, g, b)` / `text_set_alpha(key, a)` | 改 RGB（保留 alpha）/ 改 alpha |
| `text_set_align(key, mode)` | `left` / `center` / `right`（别名 `l` `c` `r` `start` `end`）；未知值返回 `false` |
| `text_set_wrap(key, px)` | 自动换行宽度；`0` = 只按 `\n` 硬换行。设宽度后对齐相对整个块宽 |
| `text_set_line_height(key, f)` | 行高倍数（相对字号），`0` = 字体自然行高 |
| `text_set_visible(key, on)` | 显隐 |
| `text_remove(key)` / `text_clear()` | 删一条 / 清空 |
| `text_exists(key)` / `text_content(key)` / `text_count()` / `text_keys()` | 查询 |
| `set_text_enabled(on)` / `text_enabled()` | 文本通道总开关；关掉后整帧跳过文本 pass |
| `text_width(content, size)` / `text_height(content, size)` | 像素尺寸估算（不加载字体，仅供排版预算，非精确值） |

命名刻意避开 Rhai 保留字（`text` / `draw` / `font` 都不做前缀裸名），并保持本仓库"短动词 + 按名寻址"的一致性：
`text_*` 是数据操作，`set_text_enabled` 是通道级开关，与 `set_shadow(on)` 同一个写法。

**UI（M1 矩形 / 边框 / 图片）**

元素存在 `Scene::ui`，与 `texts` 同构：脚本是唯一入口，渲染层每帧全量重建。
单位以字符串给：`"24px"` / `"50%"` / `"12"`（等价 `"12px"`）；锚点为 9 种
（`topleft` `topcenter` `topright` `midleft` `center` `midright` `bottomleft` `bottomcenter` `bottomright`，
兼容 `top_left`、`bottom-center` 等写法）。

| 函数 | 说明 |
| --- | --- |
| `ui_rect(key, anchor, x, y, w, h, r, g, b, a)` | 纯色矩形，alpha 混合 |
| `ui_border(key, anchor, x, y, w, h, width, r, g, b, a)` | 空心边框，厚度夹到 `min(w,h)/2`，画在矩形外侧 |
| `ui_image(key, anchor, x, y, w, h, path, r, g, b, a)` | 纹理图片，颜色乘到纹理（`1,1,1,1` = 原色）；路径相对工作目录或 exe 目录 |
| `ui_set_pos` / `ui_set_size` / `ui_set_anchor` | 改偏移 / 尺寸 / 锚点（未知锚点返回 `false`） |
| `ui_set_color(key, r, g, b, a)` / `ui_set_alpha(key, a)` | 改整体颜色（含 alpha）/ 只改 alpha |
| `ui_set_layer(key, layer)` | 改层级（整数，小的先画） |
| `ui_set_visible(key, on)` | 显隐 |
| `ui_set_border(key, width)` / `ui_set_image(key, path)` | 改边框厚度 / 换贴图 |
| `ui_set_uv(key, u0, v0, u1, v1)` | 取纹理子区间（图集裁切；九宫格在子区间内再切） |
| `ui_set_slice(key, left, top, right, bottom)` | 设置图片九宫格边距（顺序左/上/右/下，像素或百分比）；无参 = 关闭；非图片元素无效果 |
| `ui_remove(key)` / `ui_clear()` | 删一条 / 清空 |
| `ui_exists(key)` / `ui_count()` / `ui_keys()` | 查询 |
| `ui_capacity()` | 元素数量上限（`MAX_UI_ELEMENTS = 1024`），供脚本判断"还能不能再加" |
| `ui_last_error()` / `ui_error_count()` | 最近一次写入被拒/被纠正的原因（无则空串）/ 累计次数 |
| `set_ui_enabled(on)` / `ui_enabled()` | UI 通道总开关；关掉后整帧跳过 UI pass，3D 与文本不受影响 |
| `set_ui_above_text(on)` / `ui_above_text()` | 默认 `false`（文本在 UI 之上）；置 `true` 时 UI 通道后画、盖住文本 |

**同名即覆盖**：`ui_rect` / `ui_border` / `ui_image` 全部走 `core::ui::upsert_styled` 这一个写点，
key 已存在时只刷新几何/颜色，**保留** `layer` / `visible` / `uv` / `slice_margin`；边框线宽"给了就覆盖、没给沿用"。
返回值 `true` 仅代表"新建成功"，`false` 表示"没新增"——可能是同 key 覆盖，也可能是超限被拒，
后者可通过 `ui_error_count()` 增量与 `ui_last_error()` 区分。因此 `on_update` 里每帧重复提交
不会累积节点，由 `per_frame_ui_rect_does_not_accumulate`（不累积）与
`ui_capacity_is_enforced_on_script_path`（超限被拒且计数不增长）两条单测锁住。

**图片九宫格（9-slice）**

`UiElement::slice_margin: [UiValue; 4]` 存左/上/右/下四条边距，默认全 `0`（= 关闭）。
它和 `uv` 一样是**样式字段**：同 key 每帧重发几何时沿用旧值，改它只能走 `ui_set_slice`
（`ui_set_slice(key)` 无参即清零关闭）。这样脚本每帧刷 `ui_image` 不会把面板边框切分冲掉。

求解链（全部在 `core::ui`，render 只消费结果）：

1. `sanitized()` 把每条边距里的负数 / `NaN` / 无穷收敛为 `0`（不报错，只清零该条边）。
2. `slice_margins(&rect)` 按元素**最终尺寸**换算：像素直接用，百分比相对最终宽（左/右）
   与最终高（上/下）。超尺寸时**等比夹取**——`left+right > w` 就两者各让步到和恰为 `w`，
   `top+bottom > h` 同理；保证切分永不出现负宽高的块。
3. `has_slice()` 判"是图片元素且四条边距换算后非全零"，render 只在它为真时才走九宫格分支。

渲染侧（`r3e-render::ui::plan_slice`）按 3 列 × 3 行生成：列宽 `[left, w-left-right, right]`、
行高 `[top, h-top-bottom, bottom]`；四角用边距原尺寸不拉伸，上/下边只横向拉伸、左/右边只纵向
拉伸、中心双向拉伸。每块 uv 在元素的 `uv` 子区间内按同一组比例切，所以 `uv` 裁切的图集区域
也能正常走九宫格。夹取后中间列/行宽高为 `0` 时对应块**不生成**（只剩 4 或 6 块），
不留零面积 / 负面积四边形；边距全零则完全走原单四边形路径，零额外开销。

预算交互：九宫格元素标记为**原子**（一个元素最多 9 个四边形）。`plan_ui_limited` 在
"剩余 `MAX_UI_QUADS` 装不下整元素块数"时整体丢弃该元素并计入 `UiStats::skipped`，
**不会只画前几块留下残片**。该行为由 `slice_is_atomic_under_quad_budget` 锁住。

已知取舍：九宫格只做拉伸，**圆角留待后续**；四角不缩放也不做角部描边，边缘像素按线性采样
直接拉伸，因此边距很小（< 2px）或纹理没有留白时边缘会糊——这是九宫格美术本身的约束，
不在渲染层补偿。

**音频（M1 非空间化）**

音频**不进 `Scene`**：实例由脚本自己起 key 管理，账本在 `r3e_audio::Audio` 里（见 §15）。
所有函数都不 panic，失败一律返回 `false` 并写进 `audio_last_error()`。

| 函数 | 说明 |
| --- | --- |
| `audio_play(key, path)` | 默认音量 0.8、不循环；同名 key = 停旧实例后从头重播 |
| `audio_play_ex(key, path, volume, looping)` | 指定音量（夹到 `0..1`）与循环 |
| `audio_stop(key)` / `audio_stop_all()` | 停一条 / 全停 |
| `audio_pause(key)` / `audio_resume(key)` | 暂停（保留位置）/ 续播 |
| `audio_set_volume(key, v)` | 单条音量；改总音量时会与总音量一起重算下发 |
| `audio_set_loop(key, on)` | 改循环（循环标志在源构造时确定，切换会从头播） |
| `audio_is_playing(key)` / `audio_is_paused(key)` | 查询状态（静默后端恒 `false`） |
| `audio_count()` / `audio_keys()` | 在播实例数 / 全部 key |
| `audio_set_master_volume(v)` / `audio_master_volume()` | 总音量（乘到每条上） |
| `audio_set_enabled(on)` / `audio_enabled()` | 音频通道开关；关闭时停掉全部并拒绝新播放 |
| `audio_available()` / `audio_backend()` | 是否有真设备 / 后端名（`rodio` 或 `silent`） |
| `audio_last_error()` / `audio_error_count()` | 最近失败原因 / 累计失败次数 |
| `audio_status()` | 一行状态串（后端、设备、总音量、在播明细），HUD 与日志复用 |

**音频（3D 空间化，路线表第十项）**

与上面 21 个**纯叠加**：没调过 `audio_spatial_attach*` 的 key 就是平面声，音量与历史版本逐位一致；
调过之后覆盖层才会给它乘距离衰减、并把多普勒算成播放倍速（详见 §15.8）。

| 函数 | 说明 |
| --- | --- |
| `audio_spatial_attach(key, min, max)` | 注册为 3D 声源（`inverse` 模型、多普勒强度 1.0）；已注册则**只更新设置、保留位姿** |
| `audio_spatial_attach_ex(key, min, max, model, doppler)` | 指定距离模型（`inverse` / `linear` / `exponential`，也收 `inv` / `lin` / `exp`）与多普勒强度（`0` = 关） |
| `audio_spatial_attach_node(key, node, min, max)` | 注册并绑到场景节点，位置每帧由宿主从节点世界变换灌入 |
| `audio_spatial_detach(key)` / `audio_spatial_clear()` | 解除注册 / 全清；**都不影响播放**，声音回到平面音量继续播 |
| `audio_spatial_set_position(key, x, y, z)` | 手动给声源坐标（绑定节点的会被宿主覆盖） |
| `audio_spatial_set_model(key, model)` / `audio_spatial_set_doppler(key, f)` | 改单个旋钮，其余设置沿用 |
| `audio_spatial_count()` / `audio_spatial_keys()` | 已注册声源数 / 全部 key（上限 32，满员拒绝新注册并记错误，不挤掉老的） |
| `audio_spatial_gain(key)` / `audio_spatial_rate(key)` / `audio_spatial_distance(key)` | 探针：当前距离增益 / 播放倍速 / 到听者距离（未注册时 1.0 / 1.0 / -1） |
| `audio_spatial_position(key)` / `audio_spatial_model(key)` / `audio_spatial_doppler(key)` / `audio_spatial_node(key)` | 回读坐标 `[x,y,z]`、模型名、多普勒强度、绑定节点名（未注册 → 空值） |
| `audio_spatial_set_listener(x,y,z, fx,fy,fz)` / `audio_spatial_listener()` | 手动给 / 读听者位姿（宿主每帧用活动相机覆盖，脚本一般不用写） |
| `audio_spatial_set_enabled(on)` / `audio_spatial_enabled()` | 空间化总开关；关闭时所有 key 增益与倍速回 1.0（等价平面声） |
| `audio_spatial_tick(dt)` | 手动推进一帧并返回声源数（宿主每帧自动调，留给无窗口测试） |
| `audio_spatial_status()` | 一行空间化摘要（开关、听者坐标、声源数/上限、倍速可用性、最近声源） |
| `audio_supports_speed()` | 后端能否实时变速（多普勒落地前提；静默降级时 `false`，多普勒只算不下发） |

**物理**

| 函数 | 说明 |
| --- | --- |
| `attach_body(name, kind, mass)` | 按物件网格自动推导碰撞体形状，`half_extents = scale * 0.5`；物理关闭时返回 `false` |
| `detach_body(name)` | 销毁刚体并清空 `Node::body` |
| `body_impulse(name, x, y, z)` | 施加瞬时冲量，会唤醒休眠体 |
| `body_count()` | 当前刚体数（物理关闭时恒为 0） |
| `physics_enabled()` / `physics_set_enabled(on)` | 读 / 写总开关，写入返回操作后的状态 |
| `physics_backend()` / `physics_set_backend(name)` | 读 / 换后端（`null` / `builtin` / `rapier`；`rapier` 需 `--features rapier` 构建，否则返回 false），换后端会清空已有刚体 |
| `physics_gravity()` / `physics_set_gravity(x,y,z)` | 读 / 写重力 |
| `physics_ground()` / `physics_set_ground(y)` | 读 / 写地面平面高度 |

**命名变更说明**：`spawn` / `despawn` 原本更自然，但 `spawn` 在 Rhai 中是异步相关的保留字，`despawn` 也随之改为对称的 `remove_node`。这是实现约束导致的命名妥协，已写入 README 与脚本示例。

**安全阀**：`engine.set_max_operations(50_000_000)`，防止脚本里写死循环把主线程卡死。

### 7.5 RELP 求值

`eval(src)` 用 `eval_with_scope::<Dynamic>` 在持久作用域里执行一行代码，直接返回结果。因为共享同一份 `Scene`，所以 REPL 里 `set_pos("box", 0, 5, 0)` 会立刻改变画面。

调用栈为：`app.pump_console` → `scripts.eval` → 引擎 API → `Scene`。REPL 与脚本走的是**同一套 API 绑定**，不存在两套语义。

---

## 8. 应用层：r3e-app

### 5.17 级联遮挡体按级裁剪在渲染侧的接线（扩展二十四）

core 侧给了掩码（§4.17），渲染侧要把它变成**每级只提交该级的实例区间**。四个落点：

**① `InstancePlan.caster_bounds: Vec<Aabb>`**：与实例缓冲**逐项同序**，每个保留实例一份世界空间 AABB；
拿不到实测包围盒的实例（无 `MeshProvider::aabb`、退化网格）填 `fail_open_bounds()`，
保证「不知道就不裁」。顺序错位会安静地裁错物件，所以这是渲染层最需要守住的不变量。

**② `ShadowPlan { levels, masks, tested, culled }`**：`levels` 是本帧实际生效级数（0 = 关闭），
`masks[level]` 与 `caster_bounds` 同序；`tested = levels × casters`、`culled` 是被裁掉的
（级, 遮挡体）对数，两者都进 `RenderStats`。

**③ `shadow_runs(groups: &[BatchGroup], mask: Option<&[bool]>) -> Vec<ShadowRun>`**：把每个合批组
按其保留位的**连续段**切开（`shadow_cull::split_runs`），`ShadowRun` 是 `(group_index, Range<u32>)`。
`mask = None` 时每个组合成一段——这就是扩展二十三的「每级全量」，局部光源阴影走这条路径
（`shadow_runs(groups, None)`），因为点光 / 聚光的裁剪不属本扩展。

**④ 深度 pass 循环**：`for level in 0..shadow_plan.levels`，每级取 `shadow_plan.masks.get(level)`；
拿不到掩码（`levels` 大于实际掩码数）时按全量提交，行为退回扩展二十三。级矩阵改由
`fit_cascade_extended(.., caster_extend)` 产生，`shadow_matrix` 数组里**未用到的级复制最后一级**
（着色器按相机深度选级，越界索引不该取到垃圾矩阵）。

编辑器的 `shadow_casters` / `shadow_culled` 快照字段随之出现（§8.11），脚本与 RPC 都能读到裁剪量。

**已知边界**

- **只裁方向光级联**：局部光源（点光 6 面 / 聚光单面）仍是全量提交；
- **段数会反噬**：零散保留把合批切成很多短段，段数超过被裁前的组数时 draw call 反而变多——
  这正是 §8.23 探针要看「判定 A 裁掉 B」与 draw call 一起读的原因；
- **掩码与 caster_bounds 强耦合**：两者长度或顺序不一致时本模块检测不到，只能靠单测
  （`render/tests/caster_cull_contract.rs` 4 条）与探针里的 `tested` 数目对齐；
- **`casterCull` 缺省开**且缺省外扩 0：缺省路径的**像素**与扩展二十三一致（`extend = 0` 逐位等价），
  但 draw call 数会变小，因此拿 draw call 当回归基准的脚本需要更新预期。

### 5.18 遮挡体层级剔除在渲染侧的接线（扩展二十五）

core 侧给了「按节点」掩码（§4.18），渲染侧要做三件事：把实例挂回场景树、只在结构变化时建树、
把节点掩码摊成实例掩码后接回 §5.17 的 `masks`。**没有一个字段改变渲染结果**——它只改变判定的成本。

**① `InstancePlan.caster_nodes: Vec<NodeId>`**：与 `caster_bounds` 逐项同序、也与实例缓冲同序，
每个遮挡体实例记下它来自哪个场景节点。这是实例 → 树节点的唯一线索。

**② `Renderer` 的六个字段**（同寿命）：

| 字段 | 作用 |
| --- | --- |
| `caster_tree: Option<CasterTree>` | 层级树本体 |
| `caster_slots: HashMap<NodeId, usize>` | 节点 id → 树下标 |
| `caster_layout: u64` | 建树时的 `scene.layout_version()`，用来判「结构变没变」 |
| `caster_node_bounds / caster_node_casts` | 逐帧填的「节点自身」盒与实例数（`refresh` 的入参） |
| `caster_instance_nodes: Vec<usize>` | 实例 → 树下标，`expand_masks` 的入参 |
| `caster_tree_builds: u32` | 建树次数，进探针（§8.24） |

**③ `sync_caster_layout(scene)`**：`caster_layout == scene.layout_version()` 时**直接返回**，
否则按场景节点新建一棵树。父指针**分两趟填**：第一趟按迭代序把节点排进 `nodes` 并登记 `slots`，
第二趟才用 `node.parent` 查表填 `parent` 下标（父可能排在兄弟后面，一趟里查不到）。
`Scene::layout_version()` 只在增删节点/改父子关系时变，所以稳态每帧只走 `refresh`。

**④ `plan_casters_hierarchical(scene, caster_nodes, caster_bounds, level_mats) -> Option<CasterHierarchyPlan>`**：
建/刷新树 → `refresh` → `plan(level_mats)` → `expand_masks(&masks, &caster_instance_nodes)`。
返回 `None`（场景为空、树为空）时 `prepare_shadow` 退回 §4.17 的 `plan_casters` 逐实例路径，
掩码结果与层级路径**逐位相同**，只是判定次数回到 `级 × 实例`。

**⑤ `ShadowPlan` 与 `RenderStats`**：`ShadowPlan` 多了 `pruned` / `branch_pruned` / `box_tests`；
`RenderStats` 对应三个字段 `shadow_pruned` / `shadow_branch_pruned` / `shadow_box_tests`，
外加 `caster_tree_builds`。`shadow_casters + shadow_pruned == 级数 × 遮挡体数` 恒成立，
是「计数没漏」的自检式；`shadow_branch_pruned` 才是层级相对逐实例的净收益。

**已知边界**

- **每节点只记 1 个实例**：`caster_node_casts[slot] = 1` 是当前写死的（`Scene` 里一个节点通常只出
  一个实例）；同一节点承载多个实例时计数会少算，掩码**不会错**（保留方向 fail-open），
  但 `shadow_pruned` 会偏小。要修正得先让 `InstancePlan` 记「每节点实例数」；
- **节点盒取的是「被保留实例」的盒**：被主通道视锥剔除掉的实例不进树，因此它的影子在层级判定里
  根本不参与——这与扩展二十四一致（视锥外实例本来就不投递）；
- **建树看 `layout_version`**：只改 transform 不触发重建（正确，`refresh` 会跟上）；但若调用方
  **原地换了节点父子关系却没 bump 版本**，树会停在旧拓扑上——此时 `caster_tree_builds` 不涨，
  探针能看出来；
- **收益全在 CPU**：draw call / 像素不变，视觉基线必须逐像素一致（`R3E_VISUAL_REGRESSION=1` 跑）。

### 5.19 顶点法线的退化保护（扩展二十六）

主通道顶点着色器里原来是 `out.world_normal = normalize(normal_matrix * local_normal)`。
`normal_matrix` 是 CPU 侧的逆转置（贴合 TS 的伴随矩阵近似），一旦某轴 `scale` 为 0，
整块矩阵归零，`normalize(vec3(0))` 在 GPU 上直接产 `NaN`。与 CPU 侧不同，GPU 上的 `NaN`
不会在单一节点停住：它进插值器后能在整个三角形上扩散，表现为整片刷黑 / 随机噪点，
而且每帧结果不同、无显示器环境根本跑不出来。

**① 着色器改法**（`vs_main`，其余分支与插值布局一字未动）：

```wgsl
let raw_normal = normal_matrix * local_normal;
if length(raw_normal) > 1e-20 {
    out.world_normal = normalize(raw_normal);
} else {
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
}
```

| 选择 | 理由 |
| --- | --- |
| 正常路径仍是 `normalize(raw_normal)` | 只多读了一次长度，算式没变；换成 `v / length(v)` 会引入 1ulp 差异，把像素基线与 naga 的实现细节绑死 |
| 兜底方向取 `+Y` | 退化物件的法线「朝上」没有偏好也不至于全黑；随机化能让画面更像噪声，反而更难排查 |
| 阈值 `1e-20` | 远低于任何有意义法线长度（单位向量为 1），只挡数值意义上的「全零」 |

**② 为什么不在 GPU 侧补 `scale`**：`normal_matrix` 是 CPU 算好传进来的 3×3，GPU 拿不到
原始 `scale`，只能在输出端判长度。源头那道在 §4.19（`world_normal_matrix`），
这里是**独立的第二道防线**——两条路径各自守，任何一环被绕过都不会把 `NaN` 送进像素。

**③ 静态闸门**（`tests/normal_guard_shader.rs` 3 条，纯源码文本断言；naga 的
`parse_str` + `validate` 已由 `shader_wgsl.rs` 覆盖）：退化分支三段必须齐（`raw_normal` 取值 /
长度判零 / `+Y` 兜底）、正常路径必须是 `normalize(raw_normal)` 且源码里不得出现手写除法、
`out.world_normal =` 恰好两处写入（正常 + 兜底），防止日后有人补了兜底又在下游覆盖成裸 `normalize`。

**④ 视觉基线**：不改几何、不改健康输入的算式，缺省场景（无 0 缩放）像素与扩展二十五基线
逐像素一致，基线无需重建。

**已知边界**

- **阴影通道不算法线**：`vs_shadow` 只输出深度，天然不受影响，因此不加同样的分支；
- **兜底方向不随机**：退化物件之间会呈现同向光照，看起来像「统一朝上」而不是噪声——
  这是刻意的，可解释性优先；
- **只挡「全零」**：极小但非零的法线仍会走正常路径（`normalize` 会把它们放大成单位向量），
  方向精度随之下降，这与「保护有限性」不冲突。

### 5.20 主通道实例收集：单次遍历 + 分桶（扩展二十七）

`prepare_instances` 原来按「外层遍历批次键、内层遍历全场景」收集实例：每个键（网格 × 物件级材质）
都要把整份场景再过滤一遍，于是每帧的过滤次数是 `键数 × 节点数`。键数随材质组合增长（几十到上百很常见），
这层重复过滤的开销最终盖过了它省下的东西——同一节点的包围盒、蒙皮信息、材质 alpha 会被反复重算。

**① 新写法**：一趟 `scene.iter()` 把所有实例收进共享的 `staged_all`，再按
`buckets: HashMap<(MeshRef, Option<SurfaceKey>, bool), Vec<u32>>` 把**下标**分进桶，
键的**登记顺序**与**段归属**（不透明段在前、半透明段在后）各自独立推进：

| 结构 | 作用 |
| --- | --- |
| `opaque_keys` / `transparent_keys` | 键的首次出现顺序；某个键第一次被遇到时按**当次节点**的透明性归段 |
| `staged_all` | 全部待摊平实例，按场景迭代序一份（`instance` / `bounds` / `node` / `depth`） |
| `buckets` | `(网格, 材质, 段标志)` → `staged_all` 下标；同一个键在两段各成桶 |

桶键里的段标志不是装饰：同一个 `(网格, 材质)` 组合会被「不透明节点」和「半透明节点」共用
（玻璃面板与实心方块同网格、只差颜色 alpha），两段必须各自成桶、各自摊平。

视锥剔除、遮挡剔除、关节槽位分配、场景包围盒并集全部跟进这一遍：被剔除的节点 `continue` 掉，
既不进桶也不占关节槽位（与旧实现一致，`joint_scratch` 的推进顺序仍是场景迭代序）。

**② 为什么顺序逐位不变**：桶内实例天然按场景迭代序排列（与旧实现「内层 `scene.iter()` 的过滤结果」
完全同序）；键列表仍是「不透明段 + 半透明段」两段、段内按首次出现；摊平时不透明段直接展平，
半透明段沿用原来的「组间按最远深度降序 + 组内 `back_to_front`」两级排序。因此 `groups` 的每个区间、
`caster_bounds` / `caster_nodes` 的每个下标都与旧实现逐个相同——像素基线不需要重建。

**③ 空桶不等于空组**：某键的全部节点都被视锥/遮挡剔掉时，键被登记但桶不存在（`buckets.remove` 返回 `None`），
摊平阶段直接跳过，不会产生长度 0 的 `DrawGroup`。旧实现里这类键同样不产组，行为一致。

**④ 静态闸门**（`crates/r3e-render/tests/instance_buckets_contract.rs` 6 条，纯源码文本断言）：
`prepare_instances` 体内 `scene.iter()` 恰好 1 次；实例经 `staged_all.push(StagedInstance` 收拢、
下标入桶（`.entry((key.0, key.1, false))` / `.entry((key.0, key.1, true))` 两段都在）且剔除语句在其之前；
键按**所属段列表**判重（`if !bucket_keys.contains(&key)`），**禁止** `if !buckets.contains_key(&key)`；
摊平按 `(键, 段)` 取桶（`buckets.remove(&(key, surface, segment_transparent))`）；
`opaque_key_count` 的记录早于 `keys.extend(transparent_keys)`；剔除统计与半透明两级排序的调用点仍在。
六条一起把「有人又把内层遍历加回来」「有人把段顺序换了」「有人把键判重判到桶上」变成测试失败。

**⑤ 半透明段丢桶：`visual-gpu` 第一次变红的根因（已修）**：最初的版本把键判重写成
`if !buckets.contains_key(&key)`。桶是跨段共用的，同一个 `(网格, 材质)` 键先被不透明节点登记过之后，
后面同网格的玻璃节点就再也登记不进 `transparent_keys` —— 半透明段的键不存在，摊平阶段
`buckets.remove(...)` 根本轮不到这个桶，**整桶实例静默丢弃**。表现是玻璃面板区域从粉色变成背景色，
实例数从 10 掉到 7，而 640×360 上只有 558 个像素坏点（0.24%，阈值 0.1%）——肉眼看画面几乎正常，
只有严格模式报红。修法是把键判重挪回段列表、桶键补上段标志；改完本机 RTX 5060 严格模式
（`R3E_VISUAL_STRICT=1`）原基线逐位通过，无需重建基准图。

**⑥ 段归属下沉到节点（扩展二十八）**：⑤ 修的是「桶登记不上」，但**实例仍然两段都进**——
`if !occluded { buckets.entry((key.0, key.1, false)) }` 之后紧跟着无条件的
`buckets.entry((key.0, key.1, true))`。批次键 `(网格, 材质)` 不含透明度、段内收的是「所有匹配该键的节点」，
于是同一份几何被两段各收一次。

这在 `visual_regression` 夹具里没暴露：玻璃 `glass_panel` 与实心 `cube` 共用 `(cube, None)` 键，
不透明段把玻璃当实心画了一遍（不透明管线 `blend = ALPHA_BLENDING` + `depth_write = true`，
于是那次绘制**本身就完成了混合**并写下了深度），半透明段再画时 `depth_compare = Less` 判等失败被丢弃。
两次绘制只有一次生效，所以像素逐位正确 —— 靠绘制顺序碰巧对，不是语义对。两块玻璃一旦互相重叠，
生效顺序就从「场景序」变成需要「`back_to_front`」，画面立刻出错。

扩展二十八把入桶改成按节点自身透明性分支：

```rust
if transparent {
    // 半透明物件不写深度，金字塔里没有它的贡献，因此永不被遮挡剔除
    buckets.entry((key.0, key.1, true)).or_default().push(slot);
} else if !occluded {
    buckets.entry((key.0, key.1, false)).or_default().push(slot);
}
```

于是「段」不再是事后按首次出现判定的属性，而是**节点自己的属性**：一份几何只画一次，
不透明物件不进半透明段、玻璃不进不透明段、玻璃也不吃遮挡剔除。

**像素**：夹具场景逐位不变（`R3E_VISUAL_STRICT=1` 原基线通过，无需重建）——因为上面那条「碰巧生效」
的路径消失后，玻璃改由半透明段按 `back_to_front` 绘制，单块不重叠玻璃的混合结果与原路径一致。
**探针**（`visual_regression` 夹具，改前 / 改后）：主通道实例 **10 → 7**、draw call 5 不变；
半透明段透明实例 **3 → 2**。少的 3 个正是重复份——玻璃 2 个挤在不透明段、实心 `cube` 1 个挤在半透明段。

**已知边界**

- **仍是单线程 CPU、仍是每帧重建**：省掉的是重复过滤，不是「每帧从零收集」本身；上万实例该走持久化实例缓冲
  （见 README §7 已知边界），这条重构不改变那个结论；
- **HashMap 每帧重新分配**：`buckets` 与 `mesh_alphas` 都是局部变量，没有跨帧复用（要做需要把它们提成
  `Renderer` 字段并 `clear()`，代价是可读性）；实测收益在键数上百的场景，键只有个位数时两者差别在噪声里；
- **批次粒度仍是「网格 × 物件级材质」**：段已并入桶键，但同一段内仍把「同网格同材质的多个节点」压进一次
  实例化绘制。要让不透明与半透明真正各走各的批次序列，键里还得带段（已带）；再细就只能拆到逐节点，代价是
  draw call 暴涨，不做；
- **段标志是判重的一部分**：`opaque_keys` / `transparent_keys` 各自维护，桶键带第三位段标志。
  任何「把两张表合成一张」或「用桶判重」的写法都会让某一整段静默丢桶（见 §5.20 ⑤）。

### 8.1 程序入口（main.rs）

```rust
r3e [场景.json] [脚本.rhai]
```

未指定参数时按 `find_project_root()` 定位：

1. 当前工作目录（若存在 `assets/` 子目录）——覆盖 `cargo run` 与在项目根目录双击 exe 的情况；
2. 可执行文件所在目录向上最多 3 层（若找到 `assets/`）——覆盖 exe 被放在 `target/release/` 时的情况；
3. 编译期路径 `CARGO_MANIFEST_DIR` 的父目录——最后的兜底。

三级兜底的意义：**双击 `target/release/r3e.exe` 和在项目根目录 `cargo run` 都能直接跑起来**，不要求用户理解工作目录。

事件循环用 `ControlFlow::Poll`（连续重绘，不受 vsync 节流），配合 `about_to_wait` 里主动 `request_redraw()` 形成不阻塞的连续帧。日志默认 `info` 级。

### 8.2 EngineApp 结构

```rust
pub struct EngineApp {
    scene_path: PathBuf,
    script_path: PathBuf,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,          // resumed 后才存在
    scene: Rc<RefCell<Scene>>,
    physics: Rc<RefCell<Physics>>,
    audio: Rc<RefCell<Audio>>,           // 门面：内部可能是 rodio 真设备，也可能是 silent 降级
    scripts: ScriptHost,
    last_frame: Instant,
    last_watch: Instant,                 // 文件监视节流
    time: f64,
    frame_index: u64,
    paused: bool,
    keys: HashSet<Key>,                  // 当前按下的键
    input: Rc<RefCell<InputState>>,      // 键盘/鼠标/手柄帧级快照，与脚本层共用同一份
    gilrs: Option<gilrs::Gilrs>,         // 手柄后端；初始化失败降级为 None
    cursor_valid: bool,                  // 是否已收到过 CursorMoved（首帧不产生位移）
    last_dt_ms: f32,                     // 供 HUD 显示
    console: mpsc::Receiver<String>,     // REPL 输入通道
    scene_mtime: Option<SystemTime>,
}
```

`window` / `renderer` 是 `Option`：`winit 0.30` 的 `resumed` 之前不能创建窗口，这是 winit 新版生命周期的强制约束，不是可选的容错设计。

**兜底场景**：`const FALLBACK_SCENE: &str = include_str!("../../../assets/scene.json")` 在编译期内嵌。场景文件缺失或解析失败时用它，保证**引擎在任何情况下都能启动并出一幅画面**，而不是黑屏退出。

### 8.3 单帧主循环（step_and_render）

顺序是严格设计的，每一步都依赖前一步：

```
1. pump_console()                      处理 REPL 输入（可能改变 paused 与场景）
1.5 poll_gamepad()                     排空 gilrs 事件队列，取手柄状态写入 InputState
2. dt = now - last_frame，clamp 到 [0, 0.1]   ← 防卡顿后一次跳跃过大（切窗口回来常见）
3. if !paused:
   3.1 time += dt; frame_index += 1
   3.2 scripts.tick(dt, time, frame)    ← 脚本可能生成/销毁/移动物件
   3.3 apply_camera_input(dt)           ← 键盘快照 + 手柄摇杆/扳机驱动相机
   3.4 scene.time / scene.frame ← 当前值
   3.45 sync_physics_bindings()          ← 物理世代号变了才重绑场景声明的刚体
   3.5 physics.step(dt, &mut scene)     ← 开关关闭时内部直接返回
   3.6 scene.update_world_transforms()  ← 必须在脚本与物理之后！
   3.65 audio.update()                  ← 回收已播完的实例（暂停时也跑，避免恢复后残留空 Player）
4. renderer.render(&scene)              ← 从 world 矩阵组装实例并绘制
5. 更新窗口标题 HUD（含手柄连接状态）
6. input.end_frame()                    ← 清边沿与增量，down 状态保留
```

`poll_gamepad` 放在最前，是因为手柄状态要在本帧脚本与相机读取之前就绪；
`end_frame` 放在最后，是因为脚本在 `scripts.tick` 里读的 `pressed` / `released`
必须是本帧的值，清早了脚本就永远看不到边沿。暂停时两者都照跑——暂停只是冻结
时间推进与脚本/相机，输入采集不能停，否则恢复时状态是脏的。

**为什么 `update_world_transforms` 必须在最后**：脚本和物理改的都是 `transform`（局部），渲染读的是 `world`（世界）。如果顺序颠倒，画面会永远慢一帧——脚本改了位置，这一帧还画在旧位置上。

**暂停时的行为**：脚本、物理、相机输入、时间推进全部冻结，但**渲染继续**，窗口标题显示"已暂停"。这样暂停期间仍可拖动窗口、缩放、观察当前帧画面。

### 8.4 输入

输入分三层，职责单向依赖，设备层不认识脚本层：

```
core/input.rs   InputState / KeyboardState / PadState / ButtonEdge   ← 纯数据 + 边沿语义，无 I/O
   ↑ 写
app/app.rs      winit 事件（键盘 / 鼠标）、gilrs 轮询（手柄）
   ↑ 读
script/api.rs   key_* / mouse_* / gamepad_* / pad_* 注入 Rhai
```

放 core 层的原因：脚本层要读输入，但脚本层不依赖窗口或设备 crate；
设备侧写入由 app 层完成，其余层只读。

键盘的设备无关性靠 `KeyCode` 保证：core 自带 98 键的 `#[repr(u8)]` 连续枚举（字母、数字、`F1`~`F12`、
方向键、导航键、左右修饰键、符号键、小键盘），配合同序的 `ALL_KEYS` / `KEY_NAMES` 表，
`from_index` / `index` / `name` 三者严格一致。winit 的 `Key` / `PhysicalKey` 只在 app 层被翻译成
`KeyCode`，core 公开 API 里不出现任何 winit 类型（`crates/r3e-core/tests/architecture_guard.rs` 守着）。

app 层的翻译规则是"字符键看逻辑键、其余看物理键"：

| 事件 | 取值 | 原因 |
| --- | --- | --- |
| `Key::Character(c)` | `key_from_char(c)` | 跟随键盘布局：AZERTY 上按标着 `A` 的键就是 `a` |
| 小键盘字符 | 物理键 `Numpad*` | 保留小键盘独立身份，与主键区数字互不干扰 |
| `NamedKey::Shift` 等修饰键 | 物理键 `ShiftLeft` / `ShiftRight` | 逻辑键只给出通用 `Shift`，物理键才能区分左右 |
| 导航键 / 功能键 | 物理键，回退逻辑键 | 两条路径都覆盖，`F5`、`ArrowLeft` 不会漏 |

| 输入 | 行为 |
| --- | --- |
| `W` / `A` / `S` / `D` | 相机沿前向/右向前后左右平移 |
| `↑` `↓` `←` `→` | 同 WASD |
| `Q` / `E` | 相机下降 / 上升 |
| 鼠标滚轮 | 沿视线推拉，半径钳制 0.8 ~ 80 |
| 鼠标左键拖拽 | 环绕转视角（`orbit`，绕 `camera.target`） |
| 鼠标右键拖拽 | 抓取式平移，位移随相机距离缩放 |
| 手柄左摇杆 | 相机水平移动，模拟量直接叠加（死区 0.15） |
| 手柄右摇杆 | 环绕转视角 |
| 手柄 `RT` / `LT` | 相机拉近 / 推远 |
| 手柄 `Start` / `Back` | 暂停切换 / 重载脚本 |
| `空格` | 暂停 / 继续 |
| `F2` | 把内存场景保存回场景 JSON（`EngineApp::save_scene`） |
| `F5` | 手动重新载入脚本 |
| `Esc` | 退出 |

相机移动是**整体平移**：`camera.position` 与 `camera.target` 同加位移，因此朝向不变（自由飞行视角，不是环绕轨道）。`MOVE_SPEED = 6.0` 单位/秒。

键鼠状态存在 `InputState` 里：鼠标按键是 `ButtonEdge` 数组，键盘是 `KeyboardState`（98 个 `ButtonEdge`
的连续数组，`down` / `pressed` / `released` 位查询，`index` 直接当下标用，无哈希开销）。支持多键同时按下，
斜着走会归一化；`MoveKeys` 只在帧首取一次快照，避免相机计算过程中反复 `borrow`。手柄左摇杆走
`right * lx + forward * ly` 的模拟量路径，幅度由推杆深度决定，不参与归一化。

**边沿语义**是这块的核心约定：`ButtonEdge` 同时记录 `down`（持续）、`pressed`（刚按下）、`released`（刚松开），后两者只在状态翻转的那个逻辑帧内为 true，由帧末的 `InputState::end_frame` 清掉。脚本侧因此可以安全地区分"按住"与"按一下"，不会漏事件也不会重复触发。

**粘键防护**：窗口 `Focused(false)` 时立即 `InputState::reset_held()`——一次清掉鼠标、键盘、手柄的
持续按下状态。否则在窗口外松开按键，引擎永远不知道该键已抬起，相机会一直朝一个方向飞。
失焦只清"按住"，不伪造 `released` 边沿，避免脚本收到一次假的松开。

**手柄降级**：`gilrs::Gilrs::new()` 失败只是 `warn`，`gilrs` 字段为 `None`，鼠标键盘照常工作；运行中未连接手柄时 `pad.connected == false`，所有 `pad_*` API 返回 `false` / `0.0`，脚本无需分支判断设备是否存在。手柄热插拔由每帧轮询自然覆盖，标题栏显示连接状态与设备名。

**系统自动重复**：winit 的 `KeyboardInput` 带 `repeat` 标志，按住不放会持续送事件。app 层对 `repeat`
只更新按住状态、直接 `return`，因此 `Space` 不会在按住时反复切换暂停，脚本里的 `key_pressed("space")`
也只在该键真正按下的那一帧为 true。`Esc` / `F2` / `F5` 等快捷键同理只认首次按下。

**边沿清空点**：`KeyboardState::clear_frame()` 挂在 `InputState::end_frame()` 里，与鼠标、手柄同步，
每帧渲染末尾调用一次；`set()` 是唯一写入点，`down/ pressed / released` 全是只读查询，
因此"边沿只触发一次"由调用时序保证，不依赖调用方自觉。

### 8.5 文件监视与热重载

```rust
fn watch_files(&mut self) {
    if last_watch.elapsed() < 400ms { return; }   // 节流
    last_watch = now;

    // 优先级 1：场景文件
    if scene_mtime 变化 {
        读取 → scripts.rebuild_scene(json) → return
    }
    // 优先级 2：脚本文件
    scripts.reload_if_changed()
}
```

- **400ms 节流**是性能与手感的折中：`std::fs::metadata` 每帧调一次在 Windows 上代价不低（尤其是被杀软挂钩的文件）；400ms 意味着"保存后最多 0.4 秒生效"，手写脚本时几乎无感。
- 场景变化时**先返回**，不同一轮同时处理两个文件，避免"场景和脚本都改了"时重放两次、状态混乱。
- 都在 `RedrawRequested` 里调用，与渲染同线程，不需要锁。

### 8.6 终端 REPL

```rust
fn spawn_console() -> mpsc::Receiver<String> {
    // 独立线程（名 "r3e-console"）阻塞读 stdin，逐行 send 到 channel
}
```

主循环每帧 `try_recv()` 非阻塞取行，收到后：

1. **自动暂停**：若当前未暂停，置 `paused = true`（提示"已暂停脚本驱动，空格恢复"）。这是必要的——否则脚本每帧都在改物件，REPL 改完立刻被覆盖，用户会以为指令无效。
2. `scripts.eval(line)`，成功打印返回值（unit 打印 `ok`），失败打印 `执行失败: {e}`。

因此在窗口运行时，控制台可以直接敲：

```text
set_pos("ground", 0, -1, 0)
create_node("probe", "sphere", 2, 2, 0)
set_color("probe", 1, 0.2, 0.2)
camera(6, 4, 8, 0, 0, 0)
physics_backend()
```

### 8.7 动画驱动（P3）

FBX 动画走「CPU 采样 → 写节点 `Transform` → 渲染层照常读世界矩阵」的路径，渲染层完全不知道动画存在：

```
ufbx AnimStack ──r3e-asset──▶ AnimClip{name,duration,tracks[]}
                                   │ track: {node_index, path, T/R/S keys}
                                   ▼
app: spawn_asset_anim ──建骨架节点树（<资产>/<节点路径>）+ AnimInstance 绑轨道
                                   │
              每帧：脚本 on_update ──▶ scene.update_animations(dt) ──▶ 节点 local TRS
                                   │                              （父子链 → 世界矩阵）
                                   ▼
                          renderer.prepare_instances（原样）
```

落点与不变量：

| 事项 | 口径 |
| --- | --- |
| 片段注册表 | `Scene.anim_clips: HashMap<"<资产>/<片段>", Arc<AnimClip>>`，导入时登记资产内全部片段 |
| 实例命名 | `AnimInstance.name = "<资产名>/<片段名>"`（初始片段），切片段后名字保持不变，当前片段看 `anim_info` |
| 绑定 | `AnimBinding{node, node_index, path, track}`；切片段按 `node_index` 重绑，`path` 兜底 |
| 时间推进 | 帧循环顺序为「脚本 `on_update` → `update_animations` → 物理」，脚本改的位姿会被本帧动画覆盖（这是有意的：动画是位姿来源，脚本管逻辑） |
| 循环与停止 | `looping=true` 对 `duration` 取模；`false` 到 `duration` 停止并保留末帧 |
| 场景重建 | `ScriptHost::rebuild_hook` 在场景 JSON 重放（热重载 / F5）后、脚本 `on_start` 前，由 app 重新 spawn 资产节点与动画实例 |
| 启动顺序 | 场景 → 资产与动画实例 → 脚本，保证 `on_start` 里 `anims()` 非空 |

### 8.8 骨骼蒙皮（P4）

蒙皮接在 P3 的骨架节点树之后：动画写节点 TRS → 世界矩阵 → 关节矩阵 → 顶点着色器形变。
渲染层仍然只认「网格句柄 + 实例数据」，不认识 FBX 的骨骼概念：

```
ufbx skin_deformers ──r3e-asset──▶ SkinData{joints[{node_index,path}], 顶点 joints/weights}
                                          │
app: bind_asset_skins（静止姿态）──▶ SkinInstance{mesh_node, bindings[rest→inverse_bind]}
                                          │
     每帧：update_animations ─▶ update_world_transforms ─▶ update_skinning（M_i）
                                          │
                        renderer.prepare_instances：joint_base + storage buffer 上传
                                          ▼
                    shader.wgsl：world = X · Σ w_i · M_i · p（权重全 0 时跳过）
```

落点与不变量：

| 事项 | 口径 |
| --- | --- |
| 顶点属性 | `Vertex` 加 `joints(u16x4) / weights(f32x4)`，`stride 56`；属性位置 `3 / 4`，实例属性从 `5` 起 |
| 绑定姿态口径 | `L_rest = X_rest⁻¹ · B_rest`（网格局部空间），每帧 `M_i = X⁻¹ · B_now · L_rest⁻¹`，`world = X · Σ w M p` |
| 逆绑定来源 | 取「场景静止姿态」而不是 `cluster.bind_to_world`：该资产两者不一致，用后者即使不动画也会整体偏移（`skin_probe` 实测 max 0.6715） |
| 顶点空间 | 顶点是「资产世界空间」坐标，`X` 是网格节点世界矩阵；静止姿态 `world = X · p`，与 P3 画面一致 |
| 关节矩阵通道 | `@group(2) @binding(0)` storage buffer，`MAX_JOINT_MATRICES = 4096` 槽位（256 KiB），超出报错 |
| 实例寻址 | `InstanceRaw.joint_base` 指向该实例关节矩阵在本帧缓冲里的起点；非蒙皮实例填 0 且权重为 0 |
| 求解时机 | `update_world_transforms` 末尾自动 `update_skinning`；绑定必须在动画推进之前完成 |
| 关节丢失 | 关节节点被销毁时该关节矩阵退化为单位阵（不塌到原点），绑定失效的蒙皮由 `prune_skins` 清理 |
| 校验手段 | `max_deviation()` 断言静止姿态 ≈0；`log_skin_probe` 用 CPU 复算蒙皮质心做无头验收 |

---

### 8.9 视锥剔除与 AABB（阶段三 A）

剔除放在 `prepare_instances` 里，而不是 app 层：实例槽位与关节槽位都是在渲染器里分配的，
早一步在 app 剔除，渲染器仍然要为"已剔除的物件"留位置。

```
Node.mesh ──MeshProvider::mesh_aabb──▶ MeshCache.bounds（HashMap<MeshRef, Option<Aabb>>，只算一次）
                                              │
                node.world × 局部 AABB ──▶ Aabb::transformed（世界空间）
                                              │  蒙皮网格 inflate_relative(0.25)
                                              ▼
                    Frustum::from_view_projection(VP) ── intersects_aabb ──▶ 保留 / 剔除
```

落点与不变量：

| 事项 | 口径 |
| --- | --- |
| AABB 来源 | `MeshProvider::mesh_aabb(key) -> Option<Aabb>`，默认实现返回 `None`；内置网格由 `MeshData::aabb()` 现算，资产网格直接读导入时已算好的值（零拷贝） |
| fail-open | 返回 `None` 表示"提供者不支持"，调用方**必须照画**。宁可多画，也不让物件凭空消失；`RenderStats::cull_unknown` 持续非零说明资产层没喂 AABB |
| 缓存 | `MeshCache::bounds` 以 `MeshRef` 为键缓存 `Option<Aabb>`（`None` 也缓存），避免每帧走 `dyn MeshProvider` 与顶点重扫 |
| 变换口径 | 不用"中心点 × 缩放"，而是把 8 个角点经 `world` 变换后重取包围盒（`Aabb::transformed`）——带旋转的物件用中心点估算会漏掉角部 |
| 蒙皮余量 | 静止姿态 AABB 不含形变后的顶点，按 `SKIN_CULL_MARGIN = 0.25` 相对外扩，防止跳舞的角色在屏幕边缘突然消失 |
| 平面测试 | 6 个平面逐一对 AABB 做正/负顶点测试（`intersects_aabb`），不用逐角点全测，常数更小 |
| 剔除收益 | 被剔除实例既不占实例槽位、也不占关节槽位，`write_buffer` 的数据量随之下降 |
| 物理联动 | `ColliderDesc::infer_with / from_node_with(node, bounds)` 用同一份 AABB 推导 `half_extents = aabb.half_extents() × scale`，缺 AABB 才退回 `scale × 0.5`；场景里显式写的 `half_extents` 仍然优先 |
| 验收口径 | 启动/收尾各打一行「可见物件 / 绘制实例 / 剔除 / 缺包围盒」；资产场景实测 `缺包围盒 = 0`，转头后剔除数随视野变化 |

---

### 8.10 动画交叉淡入与状态机（阶段三 C）

阶段 C 只改「`AnimClip` 采样之后、写节点 TRS 之前」这一段，渲染层与蒙皮链路零改动：
过渡期间仍是**一套**骨架节点树、一份关节矩阵，因此不需要双份蒙皮权重或第二遍上传。

```
update_animations(dt)
  ├─ inst.advance(dt)：新片段时间 += dt·speed；fade.elapsed += dt，旧片段时间 += dt·旧speed
  └─ 逐绑定 i：
        fresh = 新片段.tracks[binding.track].sample(t)        // NO_TRACK 时为 None
        old   = fade.clip.tracks[fade.tracks[i]].sample(t_old) // 无过渡时为 None
        w     = smoothstep(fade.elapsed / fade.duration)       // 无过渡时 1.0
        node.transform = blend(old, fresh, w)                  // 位置/缩放 lerp，旋转 slerp
```

落点与不变量：

| 事项 | 口径 |
| --- | --- |
| 过渡状态 | `AnimInstance.fade: Option<AnimFade{clip, time, speed, looping, tracks[], elapsed, duration}>`；`None` 即单片段直出 |
| 权重曲线 | `smoothstep(t) = t²(3−2t)`，`t = elapsed/duration` 钳到 `[0,1]`——两端速度为 0，切换瞬间不抽搐 |
| 混合口径 | 位置/缩放线性、旋转 `Quat::slerp`；和关键帧插值同一套口径，避免四元数线性插值在 180° 附近塌缩 |
| 旧片段时间 | 切片段那一刻把 `time/speed/looping` 一起冻结进 `AnimFade`，过渡期间按旧速度继续走（倒放也成立），不跟 `set_anim_speed` 变动 |
| 缺轨道 | 新片段没有的轨道在绑定表里写 `NO_TRACK`（`u32::MAX`）：过渡期间由旧片段继续驱动，结束后节点停在旧片段末姿态，绝不回到绑定姿态硬跳 |
| 硬切等价 | `duration <= 0` 或目标片段与当前相同 → 直接 `set_clip`，不建 `AnimFade`，旧行为完全保留 |
| 打断过渡 | 过渡中再次 `cross_fade` 会用「当前混合后的片段状态」重建 `AnimFade`，只保留一层过渡，不会叠成多层淡入淡出 |
| 脚本 API | `set_anim_clip_fade(实例, 片段, 秒)`、`anim_fade(实例)`（新片段权重 0–1）、`anim_fade_from(实例)`（旧片段名，无过渡为空串） |
| 状态机归属 | **状态机写在脚本层**（见 `scripts/anim_state.rhai`）：引擎只提供「带过渡的切片段」这一原子操作，状态判定、优先级、攻击播完回落都由脚本按帧重算，热重载不残留状态 |
| 验收口径 | `cargo test -p r3e-core` 覆盖权重/边界/缺轨道三类用例；无头运行 `--run-seconds` 打印 `fade=旧→新 xx%` 与脚本侧权重曲线 |

---

### 8.11 编辑器桥：`EngineApi` 的唯一实现与每帧限流（阶段八）

`r3e-editor` 是**纯协议层**：`EngineApi` trait 的 8 个方法 + JSON-RPC 2.0 的请求/响应编解码 + 一个
`RpcServer`（TCP 监听、连接线程、有界队列）。它不知道 `Scene` 长什么样，也不依赖渲染器——这保证
"编辑器协议"不会反向污染渲染/物理层。trait 里那 8 个方法留着 `Err(unsupported(..))` 默认实现，
只是为了让 mock / 未来的第二后端少写样板；**`LocalEngineApi` 全部 8 个方法都覆盖了**——健康检查里"5 个场景方法默认返回 unsupported、trait 无实现者"
的问题到此为止（实现见 `crates/r3e-app/src/editor.rs`，7 个单测覆盖增删改查与脚本执行）。

真正把协议落到状态上的是 `r3e-app::editor::LocalEngineApi`：它同时持有
`Rc<RefCell<Scene>>`、`Rc<RefCell<Physics>>`、`Rc<RefCell<ScriptHost>>`，
每个 trait 方法都是对这三个门面的一次同步调用，**与 REPL、Rhai API 走的是同一批入口**，
所以编辑器改完物件，下一帧渲染出来的就是改后的样子，不存在消息副本或事件回灌。

```
TcpListener ──accept──> 连接线程（≤16）──read_until('\n')──> 有界队列 sync_channel(64) ──try_send
                                                                      │
step_and_render() 开头：drain_editor_jobs()                            │
  for _ in 0..rpc_max_per_frame { server.try_recv() }  ←───────────────┘
      → Job::run(&mut LocalEngineApi) → 结果经每请求的 reply 通道回写连接线程
```

四条资源上限，全部写死在 `server.rs`（常量名与注释同源）：

| 隐患 | 原写法 | 现写法 |
| --- | --- | --- |
| 队列无界（编辑器刷屏能把内存吃穿） | `mpsc::channel()` | `sync_channel(JOB_QUEUE_CAPACITY = 64)` + `try_send`，满则回 `-32002 Server busy`（不阻塞引擎帧、不断连） |
| 并发连接无上限 | 来一个起一个线程 | `ConnSlot` RAII 计数，`MAX_CONNECTIONS = 16`，超出写一条错误后直接关连接 |
| 等回包可能永久挂住连接线程 | `waiter.recv()` | `recv_timeout(REPLY_TIMEOUT = 5 s)`，超时回 `-32003` |
| 逐字节 `read` 拼行（慢且可被长行撑爆） | 单字节循环 | `BufReader` + `read_until` + `take(MAX_LINE_BYTES = 1 MiB)`，一次系统调用覆盖多行，超长行回 `-32600` 并断开 |

另外三点刻意取舍：

- **主线程只做 `try_recv`，永不等待**：编辑器卡死或网络抖动只影响它自己那条连接，帧时间不受外部影响。
- **`--rpc-max-per-frame`（默认 8）**：即使收到 64 条积压，也摊到多帧消化，避免单帧 >16 ms。
- **端口占用只警告**：`RpcServer::bind` 失败不阻断引擎启动——编辑器是可选通道，不能让它挡住渲染。

---

### 8.12 UI 指针交互层（扩展十四）

M1 的 UI 只是「画出来」：`resolve_rect` 把 `UiElement` 换算成像素矩形，渲染侧按 layer 排序后提交四边形。
扩展十四补上另一半——**指针能不能点到、点到了算给谁、脚本怎么知道**。三层职责刻意分开，
避免「渲染一套矩形、脚本另算一套、编辑器再算一套」：

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 契约 | `crates/r3e-core/src/ui.rs` | `UiRect` / `resolve_rect` / `hit_test`（取最上层）/ `UiInteraction`（hover + active + 事件队列），零 wgpu 零 winit，可在无窗口环境单测 |
| 驱动 | `crates/r3e-app/src/app.rs` | 每帧 `update_ui_interaction()`：屏幕尺寸 + 光标 + 左键 `pressed / down / released` 三个布尔量喂给状态机；`cursor_over_ui()` 让相机拖动让位 |
| 读取 | `crates/r3e-script/src/api.rs` / `r3e-editor` | 脚本 `ui_events()` / `ui_clicked()` / `ui_hit()` 等只读绑定；编辑器 `ui.state` 返回同一份状态 |

**命中为什么只判矩形**：边框的中间空隙、图片的透明像素都算「点得到」。对脚本作者而言
「元素的位置与大小」才是可预期的点击区域；按像素 alpha 命中会让人猜不到为什么点不动。
`visible = false` 的元素不参与命中——看不见的东西不该挡住点击，`ui_enabled = false` 时直接喂空元素表。

**状态机语义**（`UiInteraction::update` 一帧一次）：

| 事件 | 触发条件 |
| --- | --- |
| `hovered` / `unhovered` | 指针进入 / 离开元素（与按键无关，抬起后指针仍在元素上就保持 hover） |
| `pressed` | 在元素上按下（左键） |
| `clicked` | **同一个**元素上按下并抬起，先于 `released` 产出 |
| `cancelled` | 按下后没在同一元素上抬起（移出后释放 / 元素被删） |
| `released` | 只要有按下捕获，抬起**总会**收到一条 |

事件队列每帧有界（`MAX_UI_EVENTS_PER_FRAME = 64`）：异常场景下（脚本每帧新建上千元素来回切）
脚本侧 `ui_events()` 的数组不会膨胀到卡住运行时，超出部分直接丢弃并记账。
**hover / active 存的是 key 而不是索引**：元素被删、重排、改 layer 都不会让槽位指错人。

**相机让位**：`CursorMoved` 里若 `cursor_over_ui()` 为真就跳过相机旋转 / 平移。
不这么做会出现「点 UI 按钮顺手把镜头转飞」——UI 与相机抢同一个左键时的必然冲突。

**编辑器侧写语义与脚本不同，这是刻意的**：RPC `ui.set` 是「给了才改」——`LocalEngineApi` 先把当前
元素 clone 出来、逐字段覆盖补丁，再走 `ui::upsert`（裸全量覆盖）；脚本每次提交走 `upsert_styled`
（同 key 时 `layer` / `visible` / `uv` 沿用旧值，因为脚本每帧重发位置尺寸不该把样式重置掉）。
早先版本 `ui.set` 误用了 `upsert_styled`，导致编辑器改 `layer` 被静默吞掉——`app.rs` 的
`ui_set_patches_existing_element_in_place` 单测就是这条回归的守门人。

---

### 8.13 局部光源驱动与探针（扩展十五）

渲染层只认 `Scene::lights` 这一份真源，app 层不额外维护光源缓存：

| 环节 | 位置 | 做法 |
| --- | --- | --- |
| 打包 | `renderer.rs` 每帧 `LightsUniform::from_lights(&scene.lights, scene.lights_enabled)` | 逐盏再跑一次 `sanitized()`，越界值不上 GPU |
| 上传 | 主通道 `prepare` 阶段一次 `write_buffer`（1040 B） | 与实例 / 材质共用同一个队列提交，不额外 sync |
| 探针 | 起止日志 `局部光源：场景 N 盏 / 上传 M 盏 / 开关 bool` | 「场景」是 `Scene::lights.len()`，「上传」是 uniform 里实际 `count.x`，两者不等就是被截断或被开关清零 |
| 演示 | `scripts/lights_demo.rhai` | 暖色点光绕原点画圆 + 半径呼吸、冷色聚光俯照；每 0.5 s 打印原点照度与单盏字段 |

**探针为什么区分「场景」与「上传」**：开关关闭时 `count.x = 0` 但场景里灯还在；
只看场景数会以为「关了还在算」，只看上传数又看不出是否超了 16 盏被截断。两个数一起打，
一眼能区分「关掉了」和「太多被截」。

**演示脚本踩到的两个坑**（都已写进脚本注释）：

1. `light_add_spot` 是 **13 个位置参数**（key / 位置 / 方向 / 颜色 / 强度 / 半径 / 内角 / 外角），
   少给一个不会编译报错，而是 `on_start` 里抛 `Function not found`——早期 `on_start` 的错误只记进
   `last_error` 不打印，表现为「开场什么都没发生」。现在 `on_start` 出错会和 `on_update` 一样打一行
   `[script] on_start 出错: ...`，并把重复报错压掉（同一句只打一次）。
2. 一行里串 4 段 `+` 拼接会撞上 Rhai 的 `Expression exceeds maximum complexity`——
   这是**编译期**失败，`load_file` 返回 `Err` 后整脚本不执行。所以 `format` 不支持多参数签名时，
   也不要靠长拼接绕，先算到局部变量、再分两行 `log`。

**编译守卫**：`crates/r3e-script/tests/scripts_compile.rs` 会把 `scripts/*.rhai` 全部 `load_file` 一遍。
加这条的直接原因是上面第 2 点——编译期错误在 `cargo build` 里看不见、运行时只打一行 `log::error!`
然后窗口照开、画面不变，极易被误判成渲染 bug。守卫上线当天就抓到 `ccd_demo.rhai` 早已踩中同一个坑
（一行里两段 `if` 表达式 + 三段拼接），已一并修掉。

### 8.14 屏幕空间遮蔽的驱动与探针（扩展十七）

遮蔽不需要驱动——它是一个**无状态的全屏后处理**，每帧从当前深度图重算，没有「把输入喂给状态机」这一步
（这是它比局部光源、CSD 那类驱动型扩展省事的地方）。app 层唯一要做的是**把统计打进探针**：

| 环节 | 位置 | 做法 |
| --- | --- | --- |
| 打包 | `renderer.rs` 每帧 `SsaoParams` 从 `postfx.ssao.sanitized()` 出发 | 核数组由 `kernel_uniform()` 现算，416 B 一次 `write_buffer` |
| 提交 | 主通道结束后、`postfx.render` 之前 | 场景没开遮蔽段时不进这个分支，`passes = 0` |
| 探针 | 起止日志 `遮蔽探针：段内启用 b / GPU 启用 b / 样本 M/M / pass N / AO 图 WxH / 世代 g / 半径 r / 偏置 b / 强度 i` | 前后两组「启用 / 样本」是「场景里写的」与「GPU 实际用的」对照，不相等就是被夹紧或没接上 |
| 回显 | 插件 `scene_inspect` 的 `postfx.ssao` 字段 | 与 HDR 后处理同一段摘要，编辑器/插件不必再单开一条读取路径 |
| 演示 | `assets/scene_ssao.json` + `scripts/ssao_demo.rhai` | 台阶 + 内凹墙角 + 悬空板；脚本按键切换遮蔽、扫半径、1 s 打印一次探针 |

**为什么探针把「启用」与「pass 数」分开打**：`enabled = true` 但 `passes = 0` 说明设置进了场景却没进渲染分支
（例如渲染器拿的是另一份 `postfx` 快照），只有一个数时这种「配置生效但没画」的故障完全看不出来。

**真机验收记录**（`--run-seconds 4`，集显）：`ssao.enabled = true` 时 `pass 2 / 样本 8`；
把场景里 `ssao.enabled` 改成 `false` 后 `pass 0 / GPU 启用 false`，画面回到无遮蔽状态、draw call 不变——
证明整段跳过而不是「画一张全 1 的图」。

### 8.15 混合树的驱动与探针（扩展十八 A）

与 8.14 的遮蔽相反，混合树是**有状态**的：每层有自己的 `time` 与 `fade`，必须逐帧推进。
推进点就是 `Scene::update_animations` 的第一遍之后——主片段推进完，紧接着按挂载顺序把层叠上去，
**没有额外的驱动入口**，因此 app 层不需要为它新增帧内探针，也不需要改渲染侧任何代码。
app 侧只补两条 RPC，把「看」和「调」两件事交给编辑器：

| 环节 | 做法 |
| --- | --- |
| 列层 | `scene.animlayers` —— 给 `instance` 就只列该实例的层，不给就列出全部实例（`instances` 计数）；`limit` 截断、`count` 给总数、`limit` 字段回显层上限 `MAX_ANIM_LAYERS = 8` |
| 调参 | `scene.animlayerset` —— `AnimLayerPatch`：`clip / weight / mode / mask / speed` 任一子集 |
| 回显 | `anim_layer_value`：`mode / weight / effective_weight / fading / mask / mask_include / mask_exclude / clips[{name, base_weight, weight}] / time / duration / speed / looping / playing` |
| 脚本绑定 | `r3e-script` 18 个 `*_anim_layer*` 函数（见 7.4），层由脚本建，RPC 只调参 |
| 测试 | workspace 48 个测试套件 / 628 passed：core 18 项（混合算子与层求值 11 项：override 端点、additive 增量、additive 旋转绕行、层内加权采样、缺轨道跳过、淡入曲线、时间推进与顺序、片段管理边界、模式解析；场景层管理 7 项：override 只动遮罩内骨骼、additive 叠加增量、淡入缩放贡献、管理 API 往返、容量上限、切主片段后层存活、清层回主姿态），宿主补丁 6 项（列层 / 补丁落地 / 未知模式无副作用 / 缺实例或层报错 / limit 截断但 count 报总数 / 遮罩归一），编辑器 RPC 11 项（`tests/anim_layer_rpc.rs`） |

**为什么 RPC 只能调参、不能建层**：层必须绑定片段，凭空建一层等于建一个播不了任何动作的空层，
只会让「层存在但画面不动」这类故障更难查。建层走脚本的 `add_anim_layer`（片段名一次给定），
RPC 里层不存在一律返回 `invalid_params` 报错，与 `light_set`（key 不存在则新建）刻意不同。

**为什么补丁先把字符串解析完再落地**：`mode` / `clip` 先解析、`mask` 先归一，
任何一项写错都直接报错返回，不留下「模式改成功了但片段没换成」的半残状态。
这与扩展十五 `light_set` 的「先解析 kind 再改」是同一条纪律。

**为什么要同时回 `weight` 与 `effective_weight`**：前者是目标权重，后者含淡入淡出的当前值。
编辑器画「目标 vs 实际」两条线必须两个都要——只有一个数时，
`fade_anim_layer` 没生效（权重卡在原地）这种故障完全看不出来。

**遮罩串的归一规则**（脚本 `parse_bone_mask` 与宿主 `bone_mask_of` 必须逐条一致，两边各有单测）：

| 输入 | 归一结果 |
| --- | --- |
| `""` / `"*"` / 只有逗号 | `pass_all()`（整副骨架） |
| `"cube, sphere"` | `include = [cube, sphere]` |
| `"!root"` | `exclude = [root]`（其余全通过） |
| `"cube, !root"` | 两者并存，`exclude` 优先 |
| `"!"` / `"! "` | 该条规则丢弃（不造空排除规则），只剩空规则时归为 `pass_all()` |

**已知边界**：没有帧内 GPU 探针（混合树是纯 CPU 姿态契约，出错只会表现成画面僵硬或骨骼跳动）；
`effective_weight` 的过渡只有 smoothstep 一种曲线，没有自定义曲线；
层不能调整顺序、也不能在中间插入，顺序即叠加顺序。

### 8.16 场景栈的驱动与探针（扩展十八 B）

场景切换是**跨子系统**的动作：换世界要换资产库、重建动画实例、重跑脚本、刷新渲染层缓存。
把它写进 `ScriptHost::rebuild_scene` 是错的——那会让 core / script 反过来认识宿主手里的资产账本。
落地方式是「**命令信封 + 帧首统一执行**」：

| 环节 | 做法 |
| --- | --- |
| 申请 | 脚本 `scene_switch` / `scene_back` / `scene_save_archive` / `scene_load_archive` 只往**单槽**里塞一条 `SceneCommand`（`ScriptContext::scene_cmd`）；槽被占就返回 `false`（不排队、不覆盖） |
| 落地 | `EngineApp::step_and_render` 的帧首调 `apply_pending_scene_command`：取一条、执行、`info` 打成功日志 / `error` 打失败原因 |
| 顺序（硬要求） | 先换资产库（`refresh_assets_from_json`，重建回调要按新库补挂节点）→ 再**就地**换场景（`scripts.rebuild_scene` 在同一个 `Rc<RefCell<Scene>>` 上替换内部值，清刚体、跑 rebuild hook、重跑 `on_start`）→ 最后叠运行时快照（时间 / 帧号 / 通道开关 / 动画进度） |
| 切场景 | `Switch`：先校验读得到文件 → `capture_current`（把"离开时那一帧"写回当前层）→ `enter_scene_source(src, Some(path))`（`scene_path` 跟随新文件，热重载要盯它）→ `push_live_layer`（新场景压成当前层，层名取场景 `name`，重名走 `unique_name`） |
| 回退 | `Back`：先 `capture_current` 再 `back()`；已在第 0 层直接报错（**回调层本身不 push 新层**，否则回退会越回越多） |
| 读档 | `LoadArchive`：同样先压栈，但**不接管** `scene_path`——存档是一次性输入，热重载仍盯原来那个场景文件 |
| 存档 | RPC `scene.archive.save` **当场写盘**再返回真实路径（只写文件、不动世界，`queued: false`）；脚本侧同名命令仍走队列（脚本没有"调用即返回结果"的位置） |
| 路径解析 | 绝对路径直接用；相对路径先按进程工作目录，落空再按"当前场景文件所在目录"——面板里填 `lv2.json` 时，用户心里的当前目录是场景目录 |
| 编辑器 | `scene.stack`（只给声明态：层号 / 名字 / 来源路径 / 源文本字节数 / 帧号 / 时间 / 是否当前层 + `count` 总数 + `max` 上限）/ `scene.switch` / `scene.back` / `scene.archive.save` / `scene.archive.load` |
| 脚本 | 9 个绑定：写 `scene_switch` / `scene_back` / `scene_save_archive` / `scene_load_archive`，读 `scene_command_pending()` / `scene_stack_names()` / `scene_stack_depth()` / `scene_stack_index()` / `scene_stack_current()`（见 7.4） |
| 测试 | core 27 项（快照往返 / 存档兼容裸场景 / `to_scene_file` 不含运行时状态 / 栈的截断与容量 / back-forward-jump / `find` 从当前层往回 / `capture_current` 只刷当前层 / 命令信封的 `label`、`is_world_change`、`path`）、脚本 7 项（绑定往返与槽满拒绝）、宿主 4 项（默认存档路径 / 绝对与已存在路径 / 相对回退场景目录 / 存档写盘读回）、编辑器 RPC 7 项 |

**为什么命令必须等到帧首才执行**：脚本完全可能在 `on_start` 里申请切场景。若当场执行，
就是"正在重建的场景被自己打断"——旧脚本作用域还在栈上、物理刚体刚被清空、渲染缓存刚更新一半，
世界会停在半新半旧的状态。把执行点钉在帧首（脚本推进之前），一条命令要么完整生效、要么整帧不生效。

**为什么是单槽而不是队列**：`on_update` 里写 `if hp <= 0 { scene_switch("over.json"); }` 是常见写法。
有队列就会堆成无限长待办，还会出现"一帧切了三次最后停在第三个"的怪状。单槽 + 拒绝的语义是
"这一帧已经有一条换世界请求在手，你这条没进来"，脚本拿到 `false` 就知道该等下一帧。

**为什么存档不复用 `SceneFile`**：`to_scene_file` 只写**声明**（core 有单测锁着"保存再加载等于原场景"）。
运行时状态自带 `version`，单独成节且缺省即"只还原场景"，所以裸 `scene.json` 也能被
`SceneArchive::from_json_str` 直接吃进来——这条兼容是有意的，编辑器里"把当前关卡的初始状态另存为场景"
与"存一局进度"走的正是同一条代码路径。

**为什么 `scene.stack` 不返回完整源文本**：面板每帧都在刷新，十几层场景 JSON 全塞进一个响应，
序列化与网络都会变成瓶颈。要给"这一层有多大"，`source_bytes` 就够了；真要拿源文本那是另一个动作。

**已知边界**（与 §4.12 的"显式不做"同源，此处只列对调试最要紧的三条）：

- **读档后动态刚体回到声明位置**：`rebuild_scene` 会 `clear_bodies`（`generation` 自增），下一帧 `sync_physics_bindings` 按声明重新绑定，位置由脚本 `on_start` 决定。这不是丢失状态，而是快照**有意不含**刚体位姿。
- **脚本变量与随机序列不随存档还原**：脚本随场景重建重跑 `on_start`，`rand` 回到固定种子起点。
- **切场景是同步的**：换资产库 + 重建 + 重跑脚本都在一帧内做完，大场景会有一次帧尖峰；没有异步加载、没有加载画面，也没有 `scene.forward` RPC（core 的 `forward` / `switch_by_name` 已就绪，面板只需要"回退"这一个动作）。

### 8.17 局部光源阴影的驱动与探针（扩展十八 C）

局部阴影没有独立的数据通道：`cast_shadow` / `shadow_bias` 就写在 `LocalLight` 上，脚本调
`light_set_cast_shadow` / `light_set_shadow_bias`，编辑器走 `light.set` 的补丁字段，渲染器下一帧自动生效——
**与扩展十五的灯表完全同一条链路**，不新增 RPC、不新增场景段。

| 环节 | 做法 |
| --- | --- |
| 探针 | 每次探针刷新打一行：`局部阴影：意愿 N 盏 / 本帧投影 M 盏（上限 4）/ 占用 L 层 / 图 512² × 24 层`，数据来自 `stats.local_shadow_casters` / `stats.local_shadow_layers` 与 `renderer.local_shadow_size()` / `local_shadow_layer_count()` |
| 读法 | **意愿**= 场景里 `cast_shadow` 为真的盏数（脚本自己数的）；**本帧投影**= `plan_local_shadows` 按亮度选中的盏数。意愿 > 投影 → 被 4 盏上限截断或总开关关了；投影 = 0 而层数非 0 → uniform 与深度 pass 的层号脱节（图里会留上一帧的旧矩阵），这是最值得盯的一对数字 |
| 层数校验 | 占用层数应恒等于「各投影灯面数之和」：1 点光 = 6、1 聚光 = 1、两者各一 = 7。对不上就是槽位计算错了（`layer_base = slot × 6`） |
| 图幅 | 固定 `512²`（`DEFAULT_LOCAL_SHADOW_SIZE`），**不跟**方向光 `shadow.size` 走；容量恒为 `MAX_LOCAL_SHADOW_LAYERS = 24` 层，与投影盏数无关 |
| 演示 | `assets/scene_local_shadow.json`（方向光压暗并关掉自己的阴影，一盏 lamp 点光 + 一盏 beam 聚光均 `cast_shadow: true`）+ `scripts/local_shadow_demo.rhai`（`on_start` 打开投影、每 0.5 秒打印投影盏数，`on_update` 让点光绕圈、聚光摆头） |
| 冒烟 | `cargo run -p r3e-app -- --scene assets/scene_local_shadow.json --script scripts/local_shadow_demo.rhai`，跑 5 秒；起止探针应稳定为「意愿 2 / 投影 2 / 占用 7 层 / 图 512² × 24 层」，脚本日志同步打出 2 盏 |
| 测试 | core 9 项（`light.rs` 的选灯排序与截断、槽位、选面六分支、面相机近远平面、字段收敛；`scene` 的 `cast_shadow` 往返）、render 14 项（`local_shadow_contract.rs` 8 项锁 uniform 布局与纯函数一致性、`local_shadow_shader.rs` 6 项解析 `shader.wgsl` 比对 24 / 16 字面量与同组绑定）、脚本 6 项（写入读回 / 意愿与实际分离 / 槽满）、编辑器 1 项（`light.set` 补丁 `cast_shadow`）、宿主 1 项（探针字段可读） |

**为什么把「意愿」和「本帧投影」拆成两个数字**：只用一个布尔时，用户打开第 5 盏灯的投影却看不到影子，
无从判断是「灯的问题」还是「预算的问题」。两个数字一起看，`意愿 5 / 投影 4` 直接说明被上限截断，
`意愿 2 / 投影 0` 则指向总开关。脚本侧同理：`local_light_cast_shadow(key)` 读的是意愿，
`local_shadow_caster_count()` 读的是本帧实际值，脚本不必自己复算亮度排序。

**已知边界**：与 §5.12 末尾同源，不重复——4 盏上限、聚光同占 6 层槽位、图幅固定 512²、强度全局、面接缝未做跨面 PCF。

### 8.18 空间化音频的驱动与探针（路线表第十项）

宿主是"世界"与"耳朵"之间唯一的翻译层：`r3e-audio` 不认识 `Scene`，`app` 也不替它算衰减，
只负责每帧把**世界坐标**灌进覆盖层，再由覆盖层结算增益与倍速。入口是 `EngineApp::drive_spatial_audio(dt)`，
在 `update_world_transforms` 之后、`audio.update()` 回收实例之前调用。

| 环节 | 做法 |
| --- | --- |
| 听者 | 直接取 `Scene::camera`（视角输入改的就是这台），`forward = normalize(target - position)`，与 view 矩阵同一套语义；**不另建一份相机状态**，避免手柄 / 脚本各推一个位置对不上 |
| 声源 | `audio.spatial_bindings()` 给出「key → 节点名」，逐条 `scene.find(node)` 后把 `world.w_axis` 当世界坐标写进声源；节点被物理推、被脚本动、被动画改都会自然跟上 |
| 结算顺序 | **先灌位置、后 `tick_spatial(dt)`**：这样多普勒用的速度是本帧真实位移差分，而不是"位置落后一帧"的假速度 |
| 暂停 | `dt` 传 0（`tick_spatial` 内部对 `0` / `NaN` / 负数一律按 0 处理）：速度不更新、增益仍按当前位置重算——相机在暂停时还能转，若连增益都不算，恢复播放会听到一声爆音 |
| 倍速回正 | 速度归零 → `doppler_ratio` 返回 1.0，所以暂停期间不会把声调拖偏；`tick_spatial` 还会把 `dt` 夹到 `0.25`，掉帧时不会算出瞬时超大速度 |
| 探针 | 启动 / 收尾的 `音频：...` 一行由 `Audio::status_line()` 输出，其中已含空间化短摘要 `开（N 源）` / `关（0 源）`；更细的一行走脚本 `audio_spatial_status()`（开关、听者坐标、声源数/上限、倍速可用性、最近声源的 `gain` / `rate` / 距离） |
| 演示 | `assets/scene_spatial_audio.json` + `scripts/spatial_audio_demo.rhai`：`horn`（橙球，绕半径 6 巡游，绑节点 + `inverse` 模型，多普勒强度 1.0）与 `bell`（蓝柱，静止在 `z = -6`，手动坐标 + `linear` 模型、多普勒关）；脚本每 0.5 秒打印两者的 `gain` / `rate` / 距离 |
| 冒烟 | `cargo run -- assets/scene_spatial_audio.json scripts/spatial_audio_demo.rhai --run-seconds 8`，飞近 / 飞远应看到 `gain` 单调变化、`horn` 的 `rate` 在接近与远离时分别跨过 1.0 |
| 测试 | 音频 20 项（`spatial.rs` 11：模型解析往返、`sanitized` 夹取、三模型衰减曲线与边界、平滑窗在 `max` 处归零、`NaN` 距离保守静音、径向速度方向与重合退化、多普勒超音速夹取；门面 9：注册 / 满 32 拒绝不顶人、绑定节点与 `spatial_bindings`、`detach` 回平面音量、开关归 1、`tick_spatial` 下发 `set_speed`）、脚本 6 项、编辑器 RPC 9 项、宿主帧驱动 5 项 |

**为什么把多普勒落到"倍速"而不是"变调滤镜"**：`rodio` 的 `Player` 只有 `set_speed`（改采样推进速率），
它同时改音高与时长——正是多普勒在物理上该有的效果，且不需要额外的 DSP 链。代价是**只对有设备的后端成立**：
`SilentBackend::supports_speed()` 返回 `false`，`audio_supports_speed()` 让脚本能提前判断，
此时多普勒只算不下发（探针里 `rate` 仍是真实值，便于无卡环境验证公式）。

**已知边界**：无 HRTF、无左右声像（`forward` 只记录不参与增益，所以"声音没有左右之分"是设计取舍）、
无遮挡衍射、无房间混响；距离衰减与多普勒都是**逐实例标量**，不做每帧重采样卷积。

### 8.19 半透明与 MSAA 的探针（扩展十九）

这两项都发生在渲染器内部，没有脚本 API，观测口径只有两条：`--run-seconds` 的起止探针行，与编辑器
`engine_stats` RPC。

| 观测点 | 内容 |
| --- | --- |
| 探针行 | `[起始] 半透明与 MSAA：采样数 4× / 透明 draw N / 透明实例 M / 深度预通道 draw K`，紧跟在既有阴影探针之后打印，退出前再打一次 |
| 字段来源 | `RenderStats.transparent_draws` / `transparent_instances` / `depth_prepass_draws`，采样数读常量 `MSAA_SAMPLE_COUNT` |
| 编辑器 | `engine_stats` 的 `render` 段新增 `msaa_samples` / `transparent_draws` / `transparent_instances` / `depth_prepass_draws`，与探针同源 |
| 判读 | `assets/scene.json` 全不透明 → 透明两项恒 0，这是**正确**的（不是没生效）；把任一物件 `color` 的第四位改成 0.35（或用临时场景文件）后应看到「透明 draw ≥ 1 / 透明实例 ≥ 1」，且深度预通道的 draw 数相应减少一个批次 |
| 冒烟 | `cargo run -- ... --run-seconds 2`，退出码 0 + 探针显示 `采样数 4×` 即 MSAA 生效；含半透明物件的场景应看到透明两项非零 |
| 测试 | `tests/transparency_contract.rs` 6 项：常量等于 4、`RenderStats` 默认值无半透明工作、不透明批次排在半透明之前、半透明实例由远及近、相机绕到另一侧后顺序整体反转、排序结果始终是所有实例的一个排列 |

**为什么用"批次切段"而不是另开一个 pass**：主通道已经绑好相机 / 材质 / 阴影三组、实例缓冲也已连续上传，
只要保证半透明批次**排在最后**并切换一次管线（关深度写入），就能在不增加 pass、不重传数据的前提下拿到正确顺序；
多开一个 pass 反而要重绑四组绑定组、再走一次 render pass 开销。

### 8.20 capture.frame 的像素级回归基线（扩展二十）

`capture.frame` 此前只能证明"拍到了一张 PNG"，证明不了"这张 PNG 和上一版一模一样"。
扩展二十把这条链路接成可自动断言的像素基线，**分工刻意切在 GPU 边界上**：

| 层 | 位置 | 职责 | 需要 GPU |
| --- | --- | --- | --- |
| 比对逻辑（纯 CPU） | `r3e-asset::visual_regression`（`compare` / `compare_png` / `assert_png_matches`、`Tolerance`、`DiffReport`、`PixelDiff`、`PixelBox`、`RgbaFrame`） | 逐像素通道差、坏点统计与比例、最坏 8 个点、差异图落盘；无 wgpu、无窗口 | 否 |
| 产图（唯一要显卡的一步） | `r3e-app/tests/visual_regression.rs` → 起 `r3e` 子进程 | 固定尺寸跑到约定帧截图，再交给上面的库比对 | 是 |

**三条命令行选项**（都在 `main.rs` 解析、`CliOptions` 承载）：

| 选项 | 语义 | 默认 |
| --- | --- | --- |
| `--window-size WxH` | 窗口**物理像素**尺寸（`PhysicalSize`，不乘 DPI 缩放；`x` / `X` / `*` 都可作分隔符，宽高必须 ≥1） | 1280x720 逻辑尺寸 |
| `--capture-frame PATH` | 跑到约定帧后截图落盘并退出；**复用编辑器 `capture.frame` 的同一条链路**（`LocalEngineApi::capture_frame` → 离屏渲染 → `write_png_rgba`），保证回归图与 RPC 产图同口径 | 无（不产图） |
| `--capture-after N` | 第 N 帧之后截（夹到 ≥1：首帧后处理目标与字形图集还没热） | 2 |

**为什么用物理像素**：基准图是逐像素比的，逻辑尺寸在 125% / 150% 缩放下会拍出不同分辨率，同一张基线在别人机器上必然红。
**为什么另建夹具**：`crates/r3e-app/tests/fixtures/visual_regression/`（`scene.json` 7 个物件 + 空 `static.rhai`）
不复用演示场景——演示脚本会转物件、每帧刷帧率文本，同一帧号下画面随运行时长相变，那种输入会让用例"代码没改也红"。

**三道闸门，避免在没显卡的机器上假失败**：

1. `R3E_VISUAL_REGRESSION=1` 才真跑，否则直接返回并打印跳过原因（无 GPU 的 CI 上仍是 passed）；
2. 子进程退出码 `2` = 渲染器 / 截图链路不可用（无 GPU、窗口建不出来），按跳过处理；
3. 基准图缺失时既不"通过"也不"失败"，而是 `assert` 并且提示用 `R3E_UPDATE_BASELINE=1` 建档。

**环境变量**：`R3E_VISUAL_REGRESSION`（总开关）、`R3E_VISUAL_STRICT`（严格模式：跳过即失败，见下文与 `docs/CI_GPU_RUNNER.md`）、`R3E_UPDATE_BASELINE`（覆盖基准图）、
`R3E_VISUAL_CHANNEL_DELTA`（单通道容差，默认 `DEFAULT_CHANNEL_DELTA = 2`）、
`R3E_VISUAL_BAD_RATIO`（坏点比例上限，默认 `DEFAULT_BAD_PIXEL_RATIO = 0.001`）。

**进 CI：闸门开着跑，但"跳过"不许等于"没人管"**。`.github/workflows/ci.yml` 的 `visual` job
用 `R3E_VISUAL_REGRESSION: "1"` 环境变量把闸门强行打开。托管 runner 没有显卡，子进程会以退出码 2
返回，`captured_frame_matches_baseline` 仍按跳过计 passed——如果只有这一条，那道门在 CI 里等于空跑。
所以同一个测试文件里另有一条**纯 CPU** 用例补上另一半：

| 用例 | 需要 GPU | 在 CI 上的作用 |
| --- | --- | --- |
| `captured_frame_matches_baseline` | 是 | 有显卡（自建 runner / 本机）时逐像素比；没显卡时按退出码 2 跳过并打印原因 |
| `baseline_png_is_present_with_capture_size` | **否** | 校验基准图存在、可解码、尺寸恰为 `CAPTURE_WIDTH × CAPTURE_HEIGHT`、像素数与声明吻合。基准图被误删 / 被截断 / 尺寸漂移，任何 runner 都直接红 |

配套两道防呆：job 里先跑一步 pwsh 检查——① `R3E_UPDATE_BASELINE` 一旦出现在 CI 环境里就 `exit 1`
（CI 上重建基准会让"新画面覆盖旧基准"，回归被自我洗白，永远绿）；② 基准图文件缺失当场红，
不等用例报错。失败时（`if: always()`）把 `crates/r3e-app/target/visual-regression/` 的实际图与差异图
作为 artifact 上传，排查不用靠猜；成功时该目录通常不存在，`if-no-files-found: ignore` 兜住。

这条分工的边界要说清楚：托管 runner 上的 `visual` job 真正跑的是**基准图的自洽性**，不是**像素是否漂移**。

**补齐另一半：`visual-gpu`（自建 GPU runner）**。像素漂移只在带显卡的机器上才比得出来，所以
`.github/workflows/ci.yml` 另有一个 `runs-on: [self-hosted, windows, gpu]` 的 job，由仓库变量
`R3E_GPU_RUNNER=true` 控制启用（没配 runner 时 job 显示 skipped——可见的"没跑"，不是伪装的绿）。
它额外设 `R3E_VISUAL_STRICT=1`，语义是**"我保证这台机器有 GPU，请把结果当真"**：

| 情形 | 非严格（本机 / 托管 runner） | 严格（`R3E_VISUAL_STRICT=1`） |
| --- | --- | --- |
| 闸门 `R3E_VISUAL_REGRESSION` 没开 | 打印原因，计 passed | **panic**（CI 漏配就是漏跑，不许静默通过） |
| 子进程退出码 `2`（无 GPU / 窗口建不出来） | 按跳过计 passed | **panic**（显卡 / 驱动 / 会话坏了，是故障不是豁免） |
| 图拍到了但像素漂了 | 红 | 红（同左） |

没有这一条，自建 runner 上的门和托管 runner 上一样是摆设——"跳过即通过"是这道门唯一的致命伤。
顺带说清一个容易被忽略的口径：退出码 `2` 不只代表"没显卡"，截图**落盘失败**（路径不可写、磁盘满）同样走 `2`，
非严格模式下它也按跳过处理；只有严格模式才把这类真故障从"环境不支持"里摘出来变成红。
job 的第一步是环境自检：打印显卡型号与驱动版本、检查 `cargo` 与 `x86_64-w64-mingw32-gcc` 在 `PATH` 里、
基准图存在、并拒绝 `R3E_UPDATE_BASELINE`，缺项当场红。

自建 runner 的关键坑（`docs/CI_GPU_RUNNER.md` 详述）：**不能装成 Windows 服务**——服务跑在 Session 0，
没有窗口站，`r3e` 建窗口必然失败、看起来像"没显卡"；必须跑在已登录用户的交互式会话里（`run.cmd` 前台，
或任务计划程序"用户登录时启动"且不勾"不管用户是否登录都运行"）。基线是逐像素的，换 GPU / 换驱动大版本后
要在本机 `R3E_UPDATE_BASELINE=1` 重建并提交新基线，且 PR 里说清楚原因。

**产物布局**：基准图 `crates/r3e-app/tests/baselines/capture_frame_640x360.png` 进版本库（640x360 只有 1280x720 的四分之一体积，
排序 / MSAA / 后处理的变化已经看得出来）；实际图与差异图落 `target/visual-regression/`，天然不进版本库。

**判读**：截图成功时日志是 `[capture] 第 N 帧截图完成: {...} | draws=5 instances=10 culled=0 cull_unknown=0`——
把绘制统计和路径一起打出来，像素基线挂掉时才能先分清"画了但颜色不对"和"一个 draw 都没提交"。
截图失败走 `std::process::exit(2)`，回归脚本靠"退出码 + 文件存在性"区分"没 GPU 跳过"和"拍了但坏了"。

### 8.21 遮挡剔除的探针（扩展二十二）

遮挡剔除的失败模式是「物件凭空消失」，这类问题只在实机上看得见，所以探针要把**四层信息**一起打出来：
场景段里的开关、本帧真的生效没生效、金字塔几层、本帧判了多少 / 剔了多少。

**① 运行日志**（`app.rs`，与遮蔽 / 半透明探针同一处）：

```
[run] 遮挡剔除：段内开关 true / 生效 true / 金字塔 4 层 / 本帧读回 false / 判定 42 / 遮挡剔除 17（视锥剔除 3）
```

| 字段 | 判读 |
| --- | --- |
| 段内开关 | `scene.occlusion.enabled` 经 `sanitized()` 后的值（脚本改的也是它） |
| 生效 | 渲染侧 `stats.occlusion.enabled`：开关**且**金字塔就绪。刚打开时会先有一段 `段内开关 true / 生效 false` 的过渡期，正常 |
| 金字塔 N 层 | 0 = 还没读到深度。长时间为 0 说明读回链路没跑起来（无深度预通道 / 尺寸对不上） |
| 本帧读回 | 只在取到新深度的那帧为 `true`（默认 4 帧一次），偶发 `true` 才是对的 |
| 判定 / 遮挡剔除 | `tested` / `occluded`。`occluded = 0` 且 `tested > 0` 有两种可能：场景确实没遮挡关系，或 `depth_bias` 给得太大（**越大越保守**，调小它，见 §4.15 的相对偏置口径） |
| 视锥剔除 | `stats.culled`，与遮挡剔除分开计数——两者混在一起就没法判断是「镜头外」还是「被挡住」 |

**② `engine_stats` 的 `render.occlusion`**（`editor.rs`）：`{ enabled, tested, occluded, readback, levels }` 五个字段，
RPC 与日志同源，便于脚本 / 外部工具直接断言。

**③ 脚本 API**（`r3e-script/src/api.rs`，4 个函数）：

| 函数 | 语义 |
| --- | --- |
| `set_occlusion_enabled(bool)` | 开 / 关；写入即 `sanitized()`，关掉时渲染侧连金字塔一起丢 |
| `occlusion_enabled() -> bool` | 读当前开关 |
| `set_occlusion_bias(f64) -> f64` | 改深度偏置（相对比例），返回夹取后的实际值 |
| `occlusion_bias() -> f64` | 读当前偏置 |

**④ 怎么验证**：

```powershell
# CPU 侧契约（28 + 8 + 6 条，不需要显卡）
cargo test -p r3e-core -p r3e-render -p r3e-script occlusion

# 画面没漂：默认关闭，必须与扩展二十一基线逐像素一致
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture
```

**视觉观感待人工确认**：「该剔的有没有剔干净」「有没有物件闪进来又闪出去」属于取舍判断，
自动化只能证明「开关关闭时没变化」。本次收尾实测基线原样通过（`culled=0`，与扩展二十一一致），
即缺省关闭的路径没有引入任何像素变化。

### 8.22 HDR 反射探针的驱动与探针（扩展二十三）

**① 宿主侧同步**（`app.rs::sync_env_probe`，每帧在渲染前调用一次）三条判定，顺序即优先级：

| 顺序 | 条件 | 行为 |
| --- | --- | --- |
| 1 | 段缺失 / 未启用 / 来源不可用（`sanitized().is_active() == false`） | `renderer.clear_env_probe()`，镜面退回解析 IBL，只在**首次**卸载时打一行日志 |
| 2 | 与已上传的配置**相等** | 直接返回，不碰 GPU——也就不会重建材质 bind group |
| 3 | 与上次**加载失败**的配置相等 | 也返回，坏图只在切换那一次刷日志，不逐帧刷屏 |

只有走到下面才真正干活：按 `assets_root`（缺省回落到场景文件所在目录）解析相对路径，
`load_env_probe` 解码 + 重采样 + 预滤波（CPU 一次性重活），成功后
`EnvProbeSource::from_settings` → `renderer.set_env_probe(Some(..))`，成功 / 失败都记日志：

```
反射探针已加载：来源 equirect / 面边长 128² / mip 6 层 / 强度 1.00
反射探针加载失败（等距柱状 HDR 尺寸非法）：镜面反射退回解析 IBL
反射探针已卸载：镜面反射退回解析 IBL
```

失败**不装死**：明确退回解析 IBL，并把这份配置记进 `env_map_error`，避免每帧重试。

**② `engine_stats` 的 `render.env_probe`**（`editor.rs`）：`{ active, source, face_size, mip_levels, intensity }`，
与日志同源；`active = false` 时其余字段给全零缺省。

**③ 编辑器 RPC `scene.set_environment_map`**（传输层 `EnvironmentMapPatch`，字段全可选）：

| 字段 | 类型 | 备注 |
| --- | --- | --- |
| `enabled` | bool | 只切开关，不动来源 |
| `equirect` | string | 空串 = 清空来源（比 `enabled=false` 更彻底，连配置一起丢） |
| `faces` | string[6] | 顺序固定 `+X -X +Y -Y +Z -Z`；长度不对整段作废 |
| `face_size` | i32 | 有符号：负数留给宿主夹到区间下限 |
| `mip_levels` | i32 | 同上 |
| `intensity` | f32 | 夹 `0..=MAX_IBL_INTENSITY` |

宿主把补丁**叠加在现有段上**（段缺失时从缺省起步）→ `sanitized()` → 回包
`{ "environment_map": {...}|null }`，`null` 表示整段被丢掉；`source` 字段是**收敛后实际生效**的来源标签。
补丁一个字段都不给时报参数错误（「environment_map 补丁至少要给一个字段（或 enabled=false 临时关闭）」）。
它动的是 `environment_map` 段，与 `scene.set_environment`（三色天空 + SH）是**两条独立的线**，互不覆盖。

**④ 脚本 API**：8 个（见 §7.4「HDR 立方体反射探针」表）。

**⑤ 怎么验证**：

```powershell
# CPU 侧契约（22 + 4 + 7 + 9 + 4 + 7 条，不需要显卡）
cargo test -p r3e-core -p r3e-render -p r3e-script -p r3e-editor env

# 画面没漂：不配探针时必须与扩展二十二基线逐像素一致
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture
```

**视觉观感待人工确认**：「金属上到底映出了什么」「粗糙度梯度是否连续」属取舍判断，
自动化只能证明「不配探针时没变化」。本次收尾实测基线原样通过（`draws=5 instances=10 culled=0`，
与扩展二十二一致），即缺省路径没有引入任何像素变化。

### 8.23 级联遮挡体按级裁剪的驱动与探针（扩展二十四）

**① 探针**：`--run-seconds N` 的起止两行里，阴影那行从三段变五段：

```
[frame] 阴影通道：级联 4 / draw call 11 / 主通道实例 32 / 遮挡体判定 128 裁掉 36
```

| 字段 | 来源 | 读法 |
| --- | --- | --- |
| `级联` | `stats.shadow_cascades` | 实际生效级数，1 = 单级（等价阶段 D） |
| `draw call` | `stats.shadow_draws` | 由「合批数 × 级数」变成「每级保留段数之和」 |
| `主通道实例` | `stats.instances` | 参照量：遮挡体不该多于它 |
| `遮挡体判定` | `stats.shadow_casters` | `级数 × 遮挡体数`；为 0 = 裁剪没开或阴影关闭 |
| `裁掉` | `stats.shadow_culled` | 被裁的（级, 遮挡体）对数；为 0 = 本级一个都没裁掉 |

**② 三个脚本 API**（`r3e-script`）：`set_shadow_caster_extend(m: f64)` / `set_shadow_caster_cull(on: bool)` /
`shadow_caster_cull() -> bool`。外扩量是唯一能让「级边界外的物件」重新进深度图的手段，裁开关是
唯一的「回到全量」的退路。

**③ 怎么验证**：

```powershell
# CPU 侧契约（纯函数，不需要显卡）
cargo test -p r3e-core shadow_cull
cargo test -p r3e-core --test shadow_cull_cascade -- --nocapture
cargo test -p r3e-render --test caster_cull_contract -- --nocapture

# 画面没漂：缺省（extend=0 / cull=true）必须与扩展二十三基线逐像素一致
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture

# 探针读数（4 级阴影场景）
cargo run -- assets/scene_shadow.json scripts/shadow_demo.rhai --run-seconds 3
```

**实测**：4 级 / 32 个沿视线均匀分布的遮挡体（最不利于裁剪的分布）逐级保留 `[9, 19, 32, 32]`，
合计 92 / 全量 128（省 28%）；物件在 x/y 上铺开时近两级裁得更狠。缺省路径视觉基线原样通过
（`draws=5 instances=10 culled=0`——夹具场景物件少且都在近级里，故 `裁掉 = 0`，符合预期）。

> 扩展二十五把这条判定的**做法**换成了按场景树的层级剔除（探针多一行、掩码逐位不变），
> 见 §4.18 / §5.18 / §8.24。

### 8.24 遮挡体层级剔除的驱动与探针（扩展二十五）

**① 探针**：`--run-seconds N` 的起止两行，阴影那行不变（§8.23），后面多一行：

```
[frame] 遮挡体层级剔除：盒判定 64 / 未判即裁 22 / 其中整枝 20 / 建树次数 1
```

| 字段 | 来源 | 读法 |
| --- | --- | --- |
| `盒判定` | `stats.shadow_box_tests` | 下探过的树节点次数（× 级数）；溢出成倍增长说明树很碎 |
| `未判即裁` | `stats.shadow_pruned` | 没做实例判定就定成不可见的实例数；`判定 + 未判即裁` 恒等于 `级数 × 遮挡体数` |
| `其中整枝` | `stats.shadow_branch_pruned` | **净收益**：省下的实例判定里来自多实例子树的部分 |
| `建树次数` | `stats.caster_tree_builds` | 稳态应保持不变；逐帧增长 = 每帧都在增删节点、缓存没生效 |

**② 没有新增脚本 API**：层级剔除是纯内部优化，开关仍是 §8.23 的 `set_shadow_caster_cull`。
想量「层级剔除值不值」，就同一场景跑两次 `casterCull` 开 / 关，比 `遮挡体判定` 与 `其中整枝`。

**③ 怎么验证**：

```powershell
# CPU 侧：层级掩码必须与逐实例判定逐位相同（差分测试是这层唯一的正确性保证）
cargo test -p r3e-core --test shadow_cull_hierarchy -- --nocapture

# 画面没漂：层级剔除只改判定成本，像素必须与扩展二十四基线一致
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture

# 探针读数（带父子层级的阴影场景）
cargo run -- <场景>.json --run-seconds 3
```

**④ 实测**（RTX 5060 Laptop / Vulkan / 4 级级联）：

| 场景 | 级 × 实例 | 遮挡体判定 | 未判即裁 | 其中整枝 | 净收益 | 盒判定 |
| --- | --- | --- | --- | --- | --- | --- |
| `assets/scene_shadow.json`（7 个扁平实例） | 28 | 23 | 5 | **0** | 0% | 28 |
| 4 组 × 4 零件（18 实例） | 72 | 50 | 22 | **20** | 28% | 64 |
| 3 组 × 8 零件（30 实例） | 120 | 82 | 38 | **36** | 30% | 96 |

读法：**扁平场景净收益为 0**（整枝 0 —— 每个实例都是自己一根，被裁也只省一次 AABB，
而扩展二十四本来就只判一次），这是「层级剔除只在有层级时划算」的直接证据；
组装场景里 3 组零件有 2 组整枝落在近级之外，整枝跳过占总量约三成。
`盒判定` 与 `未判即裁` 一起看才是成本账：前者是**多花**的节点级判定，后者是**省下**的实例级判定，
枝叶比例不到 1:1 时这一层就是纯亏（此时关掉 `casterCull` 更省）。

---

## 9. 端到端数据流

```
   ┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
   │ assets/scene.json│   │ scripts/*.rhai   │   │ 终端 stdin (REPL)│
   └────────┬─────────┘   └────────┬─────────┘   └────────┬─────────┘
            │ serde_json            │ Rhai AST              │ mpsc
            ▼                      ▼                       ▼
   ┌──────────────────────────────────────────────────────────────────┐
   │                         r3e-app 主循环                            │
   │ dt → scripts.tick → camera → sync_bindings → physics.step → world  │
   └───────────────────────────┬──────────────────────────────────────┘
                               │ Rc<RefCell<Scene>>
                               ▼
   ┌──────────────────────────────────────────────────────────────────┐
   │  r3e-core::Scene  ← 唯一真源                                      │
   │  nodes / order / name_index / camera / light / background         │
   └───────────────┬──────────────────────────────┬───────────────────┘
                   │ node.world 只读               │ Node::body 句柄
                   ▼                              ▼
   ┌──────────────────────────────┐   ┌──────────────────────────────┐
   │ r3e-render                   │   │ r3e-physics                  │
   │ 分组实例 → 4 条 draw_indexed  │   │ Null / Builtin（可关开关）     │
   └───────────────┬──────────────┘   └──────────────────────────────┘
                   │ wgpu
                   ▼
            ┌─────────────┐
            │  GPU / 窗口  │
            └─────────────┘
```

**三种控制通道的对比**

| 通道 | 表达能力 | 生效时机 | 持久性 | 适合场景 |
| --- | --- | --- | --- | --- |
| 场景 JSON | 静态结构：初始物件、材质色、相机、光照、背景 | 载入 / 改文件（≤0.4s 热重载） | 持久（文件即状态） | 搭场景、摆初始布局 |
| Rhai 脚本 | 完整逻辑：条件、循环、函数、时间驱动 | 每帧 `on_update`；改文件 ≤0.4s 热重载 | 持久（文件即行为） | 动画、交互、群体运动 |
| 终端 REPL | 任意一行表达式求值 | 立即（暂停脚本驱动后） | 不持久（重启丢失） | 调参数、临时验证、探针 |

三者的语义完全统一：都最终落到 `Scene` 的同一组方法上，因此**没有"脚本能做的 REPL 不能做"的割裂**。

---

## 10. 架构决策记录（ADR）

| 决策 | 选择 | 被否决的方案 | 理由 |
| --- | --- | --- | --- |
| 场景数据归属 | 单一 `Scene`，`Rc<RefCell<>>` 共享 | ECS + 消息传递；双缓冲快照 | 规模小、需求直接，单真源让"改了下帧就看见"零成本成立；ECS 会引入大量样板并让脚本 API 失去按名寻址的直观性 |
| 渲染策略 | 实例化 + 按网格合并 | 每物件一个 draw call | 4 种网格 → 最多 4 次 draw call，物件数量增长不增加 CPU 提交开销 |
| 实例数据更新 | 每帧全量重建并上传 | 增量更新 / 脏标记 | 场景是唯一真源，全量重建保证绝不出现"忘了同步"；当前规模下带宽不是瓶颈 |
| 几何来源 | 程序化生成 + 外部 AIR 资产（S0–S5 后统一） | GLTF 加载后各自为政 | 引擎要"零资源即可运行"；同一份 AIR 也让物理碰撞体有了确定的几何语义（见下两行） |
| 资产格式 | **AIR 单一中间表示**：`.fbx`/`.glb`/`.gltf` 导入即归一 | 每种格式各建一套运行时结构 | "存储一种、输出分层"：统计、审计、渲染、物理只有一条路径，跨格式等价可被测试锁死 |
| 内置图元归属 | `r3e-core::procedural` + `ProceduralProvider` | 留在渲染层特判 `MeshRef::Builtin` | 输出侧禁止来源分支；内置与外部资产共用同一条上传/绘制/碰撞体推断路径（`architecture_guard` 测试守门） |
| 脚本引擎 | Rhai | Lua (mlua) / WASM | 纯 Rust 无 C 依赖，`cargo build` 即可，跨平台零配置 |
| 脚本命名 | `create_node` / `remove_node` | `spawn` / `despawn` | `spawn` 是 Rhai 保留字，被迫妥协并保持对称 |
| 物理 | trait + Null/Builtin 双实现 + 运行时开关 | 直接硬依赖 Rapier | 契约先钉死；内置实现只做「刚体 vs 地面」，够用且零依赖；换成 Rapier 时场景与脚本零改动 |
| 投影矩阵 | DirectX 风格（NDC 0..1） | OpenGL 风格（-1..1） | wgpu 的 NDC 定义如此，用错深度全错 |
| 色彩 | sRGB surface + 线性光照计算 | 手写 gamma 校正 | 让 surface 格式自动完成编码，着色器保持简单 |
| 热重载 | 重放基础场景 + 重跑脚本 | 增量应用脚本 diff | 脚本是命令式的，无法 diff；干净重建是唯一可靠做法 |
| 错误处理 | 库层返回 `String`，API 返回 `bool` | 全链路 `thiserror` | 脚本侧 `bool` 直接可判断，避免 Dynamic 错误类型膨胀 |

---

## 11. 已知边界与性能特征

### 11.1 能力边界（明确的"不做"）

| 维度 | 现状 | 影响 |
| --- | --- | --- |
| 几何 | 4 种内置程序化网格 + 外部 FBX 静态网格 | 可表达复杂模型（含 UV）；FBX 已支持节点级动画（P3）与骨骼蒙皮（P4），见 [`ASSETS.md`](ASSETS.md) |
| 材质 | 每物件颜色 × 材质因子 × 四类贴图（扩展二十一：base_color / 切线空间法线 / 粗糙度 / AO），PBR 三旋钮（metallic / roughness / emissive） | 金属度只有标量旋钮、**无 metallic 贴图**（glTF 的 `metallicRoughness` 图只取 G 通道当粗糙度，B 通道丢弃）；贴图无 UV 变换（平铺 / 偏移）、无三平面映射；法线贴图只改着色法线，不改几何与阴影；材质由脚本 / 场景 / RPC 三处写入 |
| 光照 | 1 方向光 + 环境探针（扩展十二：三色天空 + 太阳 + 9 项球谐漫反射，等价于一次"预积分环境光"），直接光照走 **Cook-Torrance**（GGX + Smith + Schlick）；方向光带阴影贴图（阶段三 D 单级 / 扩展十六最多 4 级 CSM / 扩展二十四按级裁剪遮挡体 + 扩展二十五按场景树层级剔除，缺省开且不改画面）；屏幕空间遮蔽（扩展十七）叠在 composite 前；**HDR 立方体反射探针**（扩展二十三：离线预滤波 mip 链，面边长 128 缺省 / 上限 1024、镜面最多 10 层） | 环境细节已可上金属：配了探针时镜面 IBL 按粗糙度取 mip，漫反射改用探针第 0 层的辐照度投影；**但探针是静态的**——不抓取运行中场景，没有平面反射与 SSR，物件动起来不会出现在金属上；整场景只用一张图，无区域化探针与探针混合；只认 Radiance `.hdr`（不读 EXR / PFM / DDS），六面图必须同尺寸；预滤波是 CPU 串行的一次性开销（512 面以上加载有明显停顿）；不配探针时金属只映天 / 地 / 太阳，与扩展十二逐像素一致；环境项不参与阴影判定（影子边缘的环境亮度不随阴影变暗）；环境退化为 `None` 时仍靠 `albedo × ambient` 常量项兜底；局部光源（扩展十五）最多 16 盏点光/聚光、扩展十八 C 起其中最多 4 盏可投影（点光 6 面 / 聚光单面、图幅固定 512²、聚光与点光同占 6 层槽位），无 IES/面光源、不参与环境项、阴影强度只有全局标量、立方体面接缝未做跨面 PCF；遮蔽是**整幅相乘的近似**（并非只压环境项）、无多尺度、无双边滤波；无全局光照；级联上限 4，扩展二十四起每级只提交落在本级覆盖范围内的遮挡体（`casterCull`，缺省开、保守裁剪不改画面），扩展二十五起这次「落在级内」的判定按场景树层级自顶向下做（整棵子树在级外就整枝跳过、只有叶子逐实例判，层级平坦的场景净收益为 0），不再每级重渲全场景；远景阴影质量取决于级数与图幅 |
| 后处理 | 常开 `Rgba16Float` 离屏 HDR 目标 + composite（曝光 → 色调映射 → FXAA）+ 可选三级泛光（扩展十三）+ 可选屏幕空间遮蔽两个全屏 pass（扩展十七）；缺省直通 | **无运动模糊、无 TAA、无自动曝光**：泛光层级与核大小固定、不随分辨率自适应；FXAA 是纯后处理，救不回已丢失的几何信息；曝光为手动旋钮；遮蔽无历史累积，快速转动相机时高频噪声靠 4×4 散焦压制而非时间累积 |
| 抗锯齿 | **MSAA 4×**（扩展十九，固定采样数，主通道 4× 附件 resolve 进 HDR 目标）；后处理末端另有 FXAA（扩展十三） | 采样数写死为 `MSAA_SAMPLE_COUNT = 4`，不可运行时切换；MSAA 只作用于几何边缘，后处理之后画的文本 / UI 走自己的通道、边际改善有限；无 TAA（需要历史缓冲） |
| 透明 | alpha blend + 半透明批次**按相机深度由远及近**排序（扩展十九），半透明不写深度、最后绘制 | 排序粒度是**实例**（世界位置到相机的距离平方，稳定排序），不做逐三角形 / 逐像素排序，也无 OIT；同一网格内的相交半透明、或一个物件内部自遮挡仍可能出错；不透明与半透明之间靠深度测试、半透明之间靠绘制顺序 |
| 剔除 | 视锥剔除（AABB × 6 平面）+ **遮挡剔除**（扩展二十二：上一帧深度金字塔 + 两阶段 AABB 判定，缺省关闭）；无层级剔除 | 逐实例用网格局部 AABB 变换到世界后与视锥测试，蒙皮网格按 `SKIN_CULL_MARGIN`（0.25）放大包围盒避免形变越界被误剔；取不到包围盒时保守绘制。遮挡剔除只剔**不透明**批次（半透明不写深度，判了也不准），金字塔低频读回（默认 4 帧）因而滞后几帧，且只按 AABB 粗判——细长物 / 栅栏类遮挡体效果有限；无 GPU 遮挡查询、无「连续 N 帧才剔」的迟滞 |
| 物理 | 默认关闭；`builtin` 后端仅刚体 vs 地面平面；`rapier` 后端（`--features rapier`）为完整刚体动力学 | `builtin` 下物件互不碰撞、无旋转动力学、无地形碰撞，物件不会堆叠或翻滚；切到 `rapier` 才有互撞 / 堆叠 / 翻滚 / 碰撞事件 / CCD，但**仍无关节、无布娃娃、无角色控制器**，也**没有地形/网格三角面碰撞体**（只有 box / sphere / capsule / cylinder 四种基本体） |
| 动画 | 脚本改 transform + FBX 节点级 TRS 动画（P3）+ 线性混合蒙皮（P4）+ 交叉淡入/脚本状态机（阶段三 C）+ 分层混合树（十八 A：最多 8 层、override / additive、骨骼遮罩） | 有关键帧插值、速度/循环/片段切换、smoothstep 双片段加权混合（位置/缩放 lerp、旋转 slerp）、脚本层状态机；分层侧有骨骼遮罩（`BoneMask`）、每层权重淡入淡出、`override` / `additive` 两种合成模式，18 个 `anim_layer*` 脚本 API 与 2 条编辑器 RPC（`scene.anim_layers` / `scene.anim_layer_set`，见 §4.11 / §8.15）；**仍无 morph target / 表情、无混合空间（blend space）、无 IK、无根运动提取**
| 拾取 | 无 | 无射线检测、无鼠标点选物件 |
| 多相机 | 单相机 + 1 台阴影正交相机（渲染器内部，非场景可操作对象） | 无分屏、无后视镜；阴影相机不可由脚本直接摆位 |
| 场景 | **一个进程一个场景**（扩展十八 B）：启动读一份场景文件，脚本 / RPC 只能**申请切换**，由宿主在帧首把世界整个换掉；`SceneStack` 上限 16 层，存档 = 场景声明 + 运行时快照（见 §4.12） | 没有并行场景、没有异步加载与加载画面（切场景当帧同步做完，大场景有一次帧尖峰）；快照**不含**刚体位姿（物理按声明重建，动态刚体靠脚本 `on_start` 重跑）与脚本变量 / 随机种子；栈超限丢最老一层；回退只移游标、不删层，但没有 `scene.forward` RPC |
| 音频 | wav / ogg / mp3 一次性解码播放、多路混音 + 总音量、暂停/循环；**3D 空间化**（路线表第十项）：声源可绑场景节点或给手动坐标，三种距离模型（inverse / linear / exponential）+ 边界平滑窗，多普勒经播放倍速落地，听者每帧由活动相机推导 | **无 HRTF、无左右声像**（增益是逐实例标量，相机 `forward` 只记录不参与计算）、无遮挡衍射、无房间混响、无 DSP 效果链（EQ / 压缩）；无流式/边下载边播；单文件整体解码进内存，长音乐占内存；空间化声源上限 32，多普勒只对有设备的后端生效（静默后端只算不下发） |
| 视觉回归 | `capture.frame` 像素基线（扩展二十）：640x360 物理像素、单一静止夹具场景（7 物件 + 空脚本），默认容差 单通道 2 / 坏点 0.1%（见 §8.20） | 基线是**逐像素**的：跨 GPU / 驱动 / 系统字体渲染不保证一致，换机器先用 `R3E_UPDATE_BASELINE=1` 重建再比；无 GPU 时逐像素那条自动跳过（计 passed），CI 里改由纯 CPU 用例 `baseline_png_is_present_with_capture_size` 守基准图存在性 / 尺寸（见 §8.20），像素漂移本身仍需带 GPU 的 runner 才能拦住；只覆盖单帧静画面，动画 / 交互 / 输入路径不在基线内 |

### 11.2 性能特征

- **Draw call**：≤ 4（实际使用的网格种类数），与物件数无关。
- **每帧 CPU 成本**：O(物件数) 的视锥测试 + O(可见物件数) 的 `InstanceRaw`（96 B/物件）组装与一次 `write_buffer`；
  视锥测试前先查 `MeshCache::bounds`（哈希，`MeshRef` 为键），不重扫顶点、不走 `dyn`。
  蒙皮实例额外做一次关节矩阵求解（O(关节数)）与第二路 `write_buffer`。
- **上传带宽**：实例数 × 96 B/帧 + 关节矩阵 64 B/关节/帧。1000 物件 ≈ 94 KB/帧，
  60 FPS 下约 5.6 MB/s；单个 37 关节的角色每帧仅 2.4 KB，可忽略。
- **世界矩阵求解**：`update_world_transforms` 带记忆化缓存 + 深度上限，深层次结构下仍是 O(n)。
- **实测规模**：数千物件的群体动画（`example_swarm.rhai`）在 RTX 5060 Laptop / Vulkan 下运行正常，帧时间 1 ms 量级。
- **遮挡剔除成本**（扩展二十二，缺省关闭，开启后）：CPU 侧每条判定是投影 + 至多 `max_tiles`（默认 256）次 `f32` 比较，
  在通过视锥的实例上线性叠加；GPU 侧每 `readback_interval` 帧多一次 `copy_texture_to_buffer`（1080p ≈ 8.3 MiB），
  读回是异步的、不在帧内阻塞。收益取决于遮挡密度：物件大多互相遮挡的场景（走廊、室内）才能抵掉这部分开销，
  开阔场景基本无收益——这也是它默认关闭的原因。
- **反射探针成本**（扩展二十三，只在加载 / 换探针时）：CPU 侧 `equirect_to_cubemap` 是 `6 × face_size²` 次
  方向采样，`prefilter_specular` 是每层每像素 `PREFILTER_SAMPLES = 32` 次立方体采样——缺省
  128 面 / 6 层约 **千万级采样**，实测量级为几十毫秒，加载期一次性；512 面以上会到百毫秒级，属可感停顿。
  上传是一次性 `6 × mip_levels` 次 `write_texture`（缺省 36 次，约 1 MiB f16 数据）。
  **每帧开销为零**：探针配置不变时 `sync_env_probe` 在比较后直接返回，镜头 / 物件数都不影响它；
  唯一的偶发尖峰是「换探针」那一帧重建整批材质 bind group。
- **扩容策略**：实例缓冲按 2 倍增长（起始 512），避免每帧重建 buffer；只有跨越容量边界时才有一次分配。
- **阴影通道成本**：单级时顶点开销 ≈ 主通道一遍（同批网格再画一次，片元只写深度、无着色）；
  开启 N 级级联则为 N 倍（每级一次全场景深度 pass）。主通道每片元多 9 次 `textureSampleCompare`
  （3×3 PCF），级联过渡带内翻倍到 18 次。2048² 单级贴图实测仍满帧；4 级 2048² 实测亦满帧
  （集显未测），瓶颈在 PCF 采样而非贴图带宽。关闭阴影后该 pass 整段跳过，
  `draw_calls`、`shadow_draws`、`shadow_cascades` 同时归零。

### 11.3 可加固点（按收益排序）

1. **遮挡剔除的增量改进 / 层级剔除**：遮挡剔除（扩展二十二）已按「上一帧深度金字塔 + 两阶段 AABB」落地；进一步可换成 GPU 遮挡查询（省掉 CPU 判定与读回）、给金字塔加 mip 级渐进复核、或先按父节点包围盒做层级剔除——后者目前**只在阴影视锥里做了**（扩展二十五：`CasterTree`），主通道的物件剔除仍是逐实例测试，层级深时仍有重复开销。
2. **半透明排序细化**：实例级由远及近已在扩展十九落地；进一步可做逐三角形排序或 OIT（权重无关透明），解决同网格内相交半透明。
3. **MSAA 可配置化**：4× 已落地（扩展十九）；进一步可做运行时切换 `1×/2×/4×/8×` 与 `sample_count` 自适应，或上 TAA 补时间维度抗锯齿。
4. **脏标记**：若物件规模上万，把"每帧全量上传"改为"版本号变化才上传"。
5. **级联遮挡体的 GPU 代理预筛**：扩展二十四按 AABB 裁掉级外遮挡体、扩展二十五把判定改成
   按场景树的层级剔除（§4.18 / §5.18，扁平场景净收益为 0，组装场景实测约三成）。进一步可把判定
   搬到 GPU：级联层级包围盒做成实例化球/盒代理 + 硬件裁剪，或跨帧复用上一帧的掩码。

---

## 12. 扩展路线

按"改动范围 ÷ 用户价值"排序，建议的演进顺序：

| 阶段 | 内容 | 改动位置 | 破坏性 |
| --- | --- | --- | --- |
| **一（已完成：Rapier 后端）** | **Rapier 物理后端**：`rapier_backend.rs` 实现 `PhysicsBackend`（形状/刚体类型映射、`pipeline.step`、`sync_to_scene` 回写、碰撞事件、射线/重叠/扫掠查询），`rapier3d` 挂可选依赖（`rapier = ["dep:rapier3d"]`），默认构建仍零物理依赖 | 新增 `r3e-physics/src/rapier_backend.rs` + 接上 `PhysicsKind::Rapier` 分支 + `Cargo.toml` 可选依赖 | 无（契约与开关早已就位；不开 feature 时行为与内置后端完全一致） |
| 二 | 纹理与 UV | `Vertex` 加 `uv`、`MeshData` 生成 UV、新增材质表与 bind group | 场景 JSON 向后兼容（新增可选字段） |
| **三（已完成：A、C）** | **A 视锥剔除 + AABB**：`MeshProvider::aabb()` 打通到 `MeshRef::Asset`，`prepare_instances` 6 平面测试，物理碰撞体改用真实 AABB；**C 动画交叉淡入 + 脚本状态机**：`AnimInstance.fade` 双片段加权混合、`set_anim_clip_fade` 脚本 API、`scripts/anim_state.rhai` 三态示例 | `core/mesh.rs` 加 `aabb()` 与 `MeshProvider::aabb`；`render` 增 `MeshCache::bounds` + `Frustum`；`r3e-physics` 按 AABB 推导半长；`core/anim.rs` 加 `AnimFade`/`blend_transform`/`NO_TRACK`，`scene.rs` 混合写回 | 无（场景 JSON 不变，`set_anim_clip` 仍为硬切） |
| **四（已完成：D）** | **D 方向光阴影贴图**：`ShadowSettings` 进 `Light`、场景 `light.shadow` 段、`ShadowPassUniform` 独立绑定、`vs_shadow` 深度 pass、3×3 PCF + 法线偏移、4 个脚本 API | `core/light.rs` 加 `ShadowSettings` / `shadow_view_projection`；`render` 加深度纹理 + 第二 pass + 独立 pipeline layout；`script/api.rs` 注册 `set_shadow*` / `shadow_enabled`；`assets/scene_shadow.json` + `scripts/shadow_demo.rhai` | 着色器与 uniform 布局变化（场景 JSON 向后兼容：`light.shadow` 缺省即默认开启阴影，可用 `enabled:false` 关闭） |
| **五（已完成：P0–P4）** | **FBX 资产加载**：模型 / 贴图 / 材质 / 节点级动画 / 骨骼蒙皮 | 新增 `r3e-asset` crate；`Vertex`/`MeshData` 下沉 `r3e-core`；`MeshKind` 扩展为资源句柄；render 分组键改为 `MeshKey` 并新增材质 bind group；场景 JSON `assets` 段；脚本新增 `load_asset` / `play_anim` | **需改 `MeshKind` 语义**（枚举 → 资源句柄）；场景 JSON 向后兼容 |
| 六（已完成，并入阶段五 P4） | 骨骼蒙皮 | 顶点属性加 `joints`/`weights`、关节层级 + 逆绑定矩阵、顶点着色器 skinning 路径 | 着色器与顶点布局变化（阶段五已预留位置） |
| **七（已完成：P-CCD）** | **CCD 连续碰撞检测**：契约 `set_ccd`/`ccd_of`/`set_ccd_default`、门面批量接口、场景 `physics.ccd`（全局 + 逐物件，缺省开）、脚本 4 个 API、`--ccd` 命令行、`BackendCaps::EXPOSED` 升到 `CONTACT_CCD`（只留 `joints` 一个禁区）、穿透对照测试 | `r3e-physics` 契约 + 门面 + `rapier_backend` 建体时 `enable_ccd`；`core/scene.rs` 加 `ccd` 字段；`script/api.rs` 实现 `physics_set_ccd` 并新增 `physics_ccd_default`/`body_set_ccd`/`body_ccd`；新增 `tests/ccd_penetration.rs` | 无（场景 JSON 向后兼容：缺省即开；`builtin` 仍如实报 `ccd=false`） |
| **八（已完成：编辑器桥）** | **编辑器协议**：从 stdin REPL 升级为 TCP + JSON-RPC 2.0（NDJSON）。新增 `r3e-editor` crate（协议 + `EngineApi` + `RpcServer`），`r3e-app::editor::LocalEngineApi` 作为唯一实现者桥接 `Scene`/`Physics`/`ScriptHost`，主循环每帧限流消化。资源上限：有界队列 64、并发连接 16、单行 1 MiB、等回包 5 s | 新增 `crates/r3e-editor`；`r3e-app` 加 `editor.rs` + `--rpc-port/--rpc-token/--rpc-max-per-frame` | 无（不传 `--rpc-port` 时行为与之前完全一致，端口占用只警告不阻断） |

| **九（已完成：M1 收尾）** | **非空间化音频**：后端 `rodio`（cpal + symphonia），`r3e-audio` 门面（key 账本 / 单条 × 总音量 / 通道开关 / 错误记账 / 静默降级），21 个 `audio_*` 脚本 API，`assets/audio/` 两个合成 wav + `main.rhai` 按键演示 | 新增 `crates/r3e-audio`；`r3e-script` 加 `new_with_audio` 与音频绑定；`r3e-app` 持句柄并每帧 `update()` | 无（`ScriptHost::new` 仍为静默后端，旧调用不受影响） |
| **十（已完成：3D 空间化音频）** | **3D 空间化音频**：声源挂场景节点、按距离衰减 + 多普勒 + 听者由活动相机推导 | `r3e-audio` 增 `SpatialSource`，`r3e-core::Scene` 不动（声源仍走运行时注册表），`AudioBackend` 契约复用 | 已落地 3D 空间化音频（见 §15.8 / §8.18）：未调 `audio_spatial_attach*` 的 key 逐位不变（平面语义），关闭开关即完全回退；无 HRTF / 声像 / 遮挡（纯增量；`kira` 届时可作为第二后端接入同一契约） |
| **十一（已完成：PBR 直接光照）** | **Cook-Torrance 直接光照**：`r3e-core::material` 纯函数参考实现（GGX / Smith / Schlick）+ `SurfaceMaterial`（metallic / roughness / emissive）三处写入路径共用收敛；`r3e-render::pbr` 材质 uniform 与 `SurfaceKey` 分桶；着色器片元阶段由 Blinn-Phong 换成 PBR；4 个脚本 API（`set_material` / `set_emissive` / `material_of` / `clear_material`）；`main.rhai` 三档粗糙度 + 自发光演示 | `core/material.rs` 新增，`scene.rs` / `node.rs` 加材质字段与写入入口；`render/pbr.rs` 新增 + `renderer.rs` / `shader.wgsl` 换 PBR；`script/api.rs` 注册 4 个 API；`app.rs` / `editor.rs` 透传 | 无（场景 JSON 向后兼容：`material` 段缺省即默认材质）；观感变化——默认参数下高光比旧 Blinn-Phong 更收敛 |
| **十二（已完成：IBL 环境探针）** | **解析环境 + 球谐漫反射**：`r3e-core::env` 三色天空 + 太阳方向，CPU 侧投影 9 项 Sh9（纯函数）+ 解析环境 BRDF；`r3e-render` `CameraUniform` 加环境 5 项与球谐 9 项（128→352 字节）、投影按配置位缓存；着色器加球谐漫反射与太阳高光，`env_horizon.w` 作启用位；5 个脚本 API（`set_sky` / `set_env_sun` / `set_env_intensity` / `has_environment` / `clear_environment`）；编辑器 `scene.set_environment` 补丁式 RPC；演示场景 `assets/scene_ibl.json` 三档金属球 | `core/env.rs` 新增；`scene.rs` 加 `environment` 字段；`renderer.rs` / `shader.wgsl` 环境项；`script/api.rs` 注册 5 个 API；`editor/lib.rs` `EnvironmentPatch`；`app.rs` / `editor.rs` 合并补丁与日志探针 | 无（场景 JSON 向后兼容：`environment` 缺省即退回 `light.ambient`）；**不加载 HDR 贴图、无反射探针**——金属只映天 / 地 / 太阳，映不出场景物件 |
| **十三（已完成：HDR 后处理链）** | **bloom + tonemapping + FXAA**：`r3e-core::postfx` 纯函数契约（`expose` / `bright_pass` / `tonemap` / `fxaa_edge_visible` / `resolve`）+ `PostFxSettings` 收敛；常开 `Rgba16Float` 离屏 HDR 目标，主通道改写成线性 HDR；`postfx.wgsl` 三级泛光上采样 + composite（曝光→ACES/Reinhard→FXAA）；7 个脚本 API（`set_postfx` / `set_exposure` / `set_tonemap` / `set_bloom` / `set_fxaa` / `clear_postfx` / `has_postfx`，外加 `postfx_exposure` / `postfx_tonemap` 回读）；编辑器 `scene.set_postfx` 补丁式 RPC；演示场景 `assets/scene_hdr.json` | `core/postfx.rs` 新增；`scene.rs` 加 `postfx` 字段；`render/postfx.rs` + `postfx.wgsl` 新增，`renderer.rs` 主通道改 HDR 视图、文本 / UI 前插 composite；`script/api.rs` 注册 9 个 API；`editor/lib.rs` `PostFxPatch`；`app.rs` 后处理探针日志；`tests/postfx_shader.rs` / `postfx_contract.rs` / `r3e-editor/tests/postfx_rpc.rs` 锁契约 | 无（场景 JSON 向后兼容：`postfx` 缺省即"直通"，画面与扩展十二逐像素一致）；**不做 SSAO / 运动模糊 / 时间性抗锯齿**，泛光固定三级、单帧 pass 上限 6 |

| **十四（已完成：UI 指针交互层）** | **命中测试 + 指针状态机 + 事件回灌脚本**：`r3e-core::ui` 增 `UiRect` / `resolve_rect` / `hit_test`（按 layer 取最上层、只判矩形、visible=false 点不到）/ `UiInteraction`（hover / active 两槽位 + 每帧有界事件队列 `MAX_UI_EVENTS_PER_FRAME = 64`，`clicked` 只在同一元素内按下并抬起时产出，`released` 有按下捕获时总发，`cancelled` 是移出后释放）；`r3e-app` 每帧把光标与左键边沿喂给状态机，**指针压在 UI 上时相机不吃这次拖动**；脚本注册 10 个只读 API（`ui_events` / `ui_event_count` / `ui_event_kind_name` / `ui_event_key` / `ui_hover` / `ui_active` / `ui_clicked` / `ui_hovered` / `ui_pointer_over_ui` / `ui_hit`）；编辑器新增 `ui.list` / `ui.set` / `ui.remove` / `ui.state` 四条 RPC（`UiPatch` 字段全可选，`"50%"` / `"120"` 两种写法都可）；演示 `scripts/ui_interaction_demo.rhai` | `core/ui.rs` 加交互层；`script/api.rs` 注册只读绑定；`r3e-editor` 加 `UiPatch` + 四个 trait 方法；`r3e-app/editor.rs` 实现四个方法 + `apply_ui_patch`；`app.rs` 加 `ui_interaction` 字段、`update_ui_interaction`、`cursor_over_ui`；`tests/ui_scene.rs` / `r3e-editor/tests/ui_rpc.rs` / `app.rs` 单测锁语义 | 无（纯增量；不接输入时行为与原 UI 层一致）。**不做**：键盘焦点 / Tab 遍历、滚轮与拖拽、IME、长按与双击、控件语义（按钮状态机仍在脚本里） |
| **十五（已完成：局部光源）** | **点光 / 聚光定长光源表**：`r3e-core::light` 增 `LocalLight` / `LocalLightKind` / `attenuation`（平滑窗逆平方，`d ≥ range` 归零）/ `spot_factor`（内外角 smoothstep，退化时硬边）/ `irradiance`（CPU 参考，脚本与 GPU 同式）；`Scene` 提供 `upsert_local_light` / `remove_local_light` / `clear_local_lights` / `local_light` / `lights_enabled`，上限 `MAX_LOCAL_LIGHTS = 16`（满员返回 `false` 不顶人）；`r3e-render` 每帧把整表打包进 `LightsUniform`（1 + 16×64 = 1040 B）并挂在 **`@group(3) @binding(4)`**——WebGPU `max_bind_groups = 4`，第五组 `@group(4)` 直接过不了校验，故并入场景级绑定组；总开关写 `count.x = 0`（数据留着，循环零次迭代）；片元侧超半径 / 背面 / 锥外三处早退；脚本 19 个 API（`light_add_point` / `light_add_spot` / `light_set_*` / `light_remove` / `light_clear` / `set_lights_enabled` / `local_light*`）；编辑器 5 个 RPC （`light.list` / `light.set` / `light.remove` / `light.clear` / `light.set_enabled`，补丁式「给了才改」）；`scripts/lights_demo.rhai` + 起止探针 | `core/light.rs` 加局部光源契约与常量；`core/scene.rs` 加 `lights` + 增删改查；`render` 加 `LightsUniform` / `LocalLightRaw` / `shader.wgsl` 的 `local_lighting`；`script/api.rs` 注册读写；`r3e-editor` 加 `LightPatch`；`app/editor.rs` 实现 5 个 RPC | 无（场景 JSON 向后兼容：`lights` 缺省为空；着色器绑定从 5 组收到 4 组，是**修正**而非破坏——原先的 `@group(4)` 在任何合规设备上都创建失败） |
| **十六（已完成：级联阴影 CSM）** | **方向光级联阴影**：`r3e-core::light` 增 `CascadeSettings`（count / lambda / blend / max_distance）与纯函数 `cascade_splits`（均匀↔对数插值切分）、`frustum_slice_corners`（该级 8 角点）、`fit_cascade`（光源正交相机拟合 + 正方形外接 + 纹素量化），上限 `MAX_CASCADES = 4`；`r3e-render` 把阴影图换成 `texture_depth_2d_array`（层 = 级数）、`ShadowUniform` 改为 `array<mat4x4,4>` + `split_ndc`、深度 pass 逐级渲染、`shadow_pass_uniforms` 每级一份；`shader.wgsl` 的 `shadow_factor` 按相机深度选级 + 级间按 `blend` 混合；`RenderStats.shadow_cascades` 记录生效级数；脚本 4 个 API（`set_shadow_cascades` / `shadow_cascades` / `set_shadow_blend` / `set_shadow_distance`）；编辑器 `scene.shadow` RPC 的 `ShadowPatch`；场景 `light.shadow` 段新增 `cascades` / `blend` / `cascadeDistance` | `core/light.rs` 加级联契约与三个纯函数、`core/scene.rs` 加三个可选字段与往返；`render/renderer.rs` 数组化阴影目标与逐级 pass、`shader.wgsl` 换级联采样；`script/api.rs` 注册 4 API；`r3e-editor` 加 `ShadowPatch` 与 `scene.shadow` 分支；`app/editor.rs` 实现 RPC；新增 `tests/cascade_scene.rs` / `shadow_cascades_shader.rs` / `shadow_rpc.rs` | 无（场景 JSON 向后兼容：`cascades` 缺省 1 = 单级，老场景画面与统计口径不变；绑定槽位未新增，仍为 4 组） |

| **十七（已完成：屏幕空间遮蔽 SSAO）** | **深度反推的半球核遮蔽**：`r3e-core::ssao` 纯函数契约（`hemisphere_kernel` 确定性核 + `reconstruct_view_pos` 深度重建 + `normal_from_neighbors` 邻域法线 + `is_occluded` / `ao_from_occlusion`）+ `SsaoSettings`（enabled / radius / bias / intensity / samples，`sanitized()` 夹 0.01..8 / 0..0.5 / 0..4 / 4..16）挂进 `PostFxSettings.ssao`；`r3e-render::ssao` 两个全屏 pass（深度→`R8Unorm` 遮蔽图、4×4 盒式散焦，`AO_PASSES_PER_FRAME = 2`）+ `ssao.wgsl`，composite 在曝光前 `color *= ao`；`RenderStats.ssao`；12 个脚本 API（写 6：`set_ssao` / `set_ssao_enabled` / `set_ssao_radius` / `set_ssao_intensity` / `set_ssao_samples` / `clear_ssao`；读 6：`has_ssao` / `ssao_enabled` / `ssao_radius` / `ssao_bias` / `ssao_intensity` / `ssao_samples`，其中 `bias` **只读**——写入走场景 `postfx.ssao.bias` 与 RPC `scene.set_ssao`）；编辑器 `scene.set_ssao`（`SsaoPatch`）；演示 `assets/scene_ssao.json` + `scripts/ssao_demo.rhai` | `core/ssao.rs` 新增、`postfx.rs` 挂段、`lib.rs` 导出；`render/ssao.rs` + `ssao.wgsl` 新增，`renderer.rs` 主通道深度加 `TEXTURE_BINDING` 用法并在 composite 前插入两个 pass；`script/api.rs` 注册；`r3e-editor` 加 `SsaoPatch`；`app.rs` 遮蔽探针；`tests/ssao_contract.rs` / `ssao_scene.rs` / `ssao_shader.rs` / `ssao_api.rs` / `ssao_rpc.rs` 锁契约 | 无（场景 JSON 向后兼容：`postfx.ssao` 缺省 `enabled = false`，整段跳过、画面与扩展十三逐像素一致）。**边界**：AO 乘整幅画面（非只乘环境项）、无非双边滤波、无多尺度、屏幕外信息缺失处保守取 1 |
| **十八（A、B、C 已完成；D 候选）** | **A 动画混合树**（已落地：分层 mask / additive，补 11.1 的动画边界，见 §4.11 / §7.4 / §8.15）；**B 场景切换 + 存档**（已落地：场景栈 + 运行时快照 + 命令信封 + 5 条 RPC / 9 个脚本绑定，补「一个进程一个场景」的边界，见 §4.12 / §7.4 / §8.16）；**C 局部光源阴影**（已落地：点光立方 6 面 / 聚光单面，24 层深度数组，每帧最多 4 盏投影，见 §5.12 / §8.17）；**D 时间性抗锯齿 / 运动模糊**（需要历史缓冲与重投影，与当前无历史累积的架构冲突最大） | `core` 契约先行（A 落 `anim_blend.rs` + `scene.rs`，B 落 `scene_stack.rs`，C 落 `light.rs` 的 `plan_local_shadows` / `shadow_face_view_projection`）；C 动了 render 的多目标与 pass 预算（新增 24 层深度数组 + 每层一次 pass），D 尚未开始 | 无（A、B、C 均为纯增量：场景 JSON 与既有 RPC 一字不改——`cast_shadow` 缺省 `false`，没灯投影时整段 pass 跳过，画面与扩展十七逐像素一致） |
| **十九（已完成：半透明排序 + MSAA）** | **半透明按深度由远及近**：新增 `r3e-render::transparency`（`is_transparent` / `view_depth` / `back_to_front` 三个纯函数），`prepare_instances` 把批次拆成「不透明段 + 半透明段」（`transparent_start`），组间按最远深度降序、组内稳定排序后搬进实例缓冲，半透明批次用**不写深度**的 `transparent_pipeline` 最后绘制；**MSAA 4×**：`MSAA_SAMPLE_COUNT = 4` + `create_msaa_targets`（4× `Rgba16Float` 颜色 + 4× 深度），主通道 resolve 进 `postfx.hdr_view()`，新增单采样**深度预通道**保证遮蔽通道仍有可采样深度；`RenderStats` 增 `transparent_draws` / `transparent_instances` / `depth_prepass_draws`，`--run-seconds` 探针与编辑器 `engine_stats.render` 同频输出（见 §5.13） | `render/transparency.rs` 新增 + `lib.rs` 导出 `MSAA_SAMPLE_COUNT`；`render/renderer.rs` 加两条管线与两组附件、`prepare_instances` 拆段排序、新增深度预通道；`app.rs` 与 `editor.rs` 各加探针字段；新增 `tests/transparency_contract.rs` 6 项 | 无（场景 JSON、脚本 API、RPC 一字不改：全不透明场景 `transparent_*` 恒 0，画面只多出 MSAA 边缘平滑；显存多一组 4× 颜色 + 4× 深度附件） |
| **二十（已完成：capture.frame 像素级回归 + AO 占位修复）** | **像素级回归基线**：`r3e-asset::visual_regression` 纯 CPU 比对库（逐像素通道差 / 坏点比例 / 最坏 8 点 / 差异图，`Tolerance::sanitized()` 收敛容差，默认 `DEFAULT_CHANNEL_DELTA = 2` 与 `DEFAULT_BAD_PIXEL_RATIO = 0.001`）；`r3e-app/tests/visual_regression.rs` 起子进程产图后比对基准图 `tests/baselines/capture_frame_640x360.png`，三道闸门（开关 / 退出码 2 / 基准缺失）保证无 GPU 机器不假失败；**AO 占位图修复**：`create_ao_placeholder` 建完 1×1 `R8Unorm` 后补一笔 `queue.write_texture([255])`，此前不写初值导致 AO 关闭时 `color *= ao` 把整帧乘成纯黑（见 §4.10 / §8.20） | `r3e-asset/src/visual_regression.rs` 新增 + `lib.rs` 导出；`r3e-app/tests/`（用例 + 基准图 + 夹具）新增；`r3e-app/src/main.rs` 加 `--window-size` / `--capture-frame` / `--capture-after` 与 `parse_window_size`，`app.rs` 加 `window_attrs_size` / `capture_frame_now` / 第 N 帧截图退出的钩子；`r3e-render/postfx.rs` 的 `PostFxPass::new` 多收 `&wgpu::Queue`（`renderer.rs` 同步传参） | 无（命令行新增三项均为可选：不传时窗口仍 1280x720 逻辑尺寸、本进程不产图；`PostFxPass::new` 是 crate 内 API，签名变化不涉及场景 JSON、脚本与 RPC） |

| **二十一（已完成：材质数据贴图）** | **法线 / 粗糙度 / AO 三张贴图**：`r3e-core::tangent` 按三角形位置与 UV 差分累加算切线基（UV 面积退化跳过、整网格退化兜底正交基、手性来自副切线方向），`Vertex` 补 `tangent: [f32; 4]`（location 11）；`r3e-core::texture_slot` 把 FBX / glTF 五花八门的槽位名收敛成 `TextureSlot`（Normal / Roughness / Occlusion / Unclassified）并给出线性数据判据 `is_linear_data()`，`bump` 是高度图、**不算**法线；`r3e-asset` 在 FBX 与 glTF 两条导入路径上按槽位挑图（同用途只取第一张，`Unknown` 不猜也不丢 base color），`MeshAsset::build` 对外部网格重算切线；`r3e-render::pbr` 加 `PbrMapFlags` 与 `MaterialUniform::build_with_maps`（`params2.xyz` 三个开关，w 保留），`renderer.rs` 把三张图绑到 `@group(1) binding 3/4/5`、复用 base color 采样器、缺图垫 1×1 占位（法线用线性 `flat_normal`、其余 `white`）、`resolve_pbr_map` 只在真拿到视图时置位；`shader.wgsl` 用几何法线 + 切线构造 TBN 采样法线（`*2-1` 解码、`z` 下限 `0.05`），粗糙度取 G 作乘数，AO 取 R **只压环境光**（直接光交给阴影）。详见 §4.14 / §5.14 | `core/tangent.rs` / `core/texture_slot.rs` 新增，`core/mesh.rs` 加切线字段与 `DEFAULT_TANGENT`、`lib.rs` 导出；`asset/air_import.rs` / `gltf_import.rs` / `lib.rs` 接槽位筛选与切线重算；`render/pbr.rs` / `renderer.rs` / `shader.wgsl` 接绑定与采样；新增 `asset/tests/pbr_map_import.rs`（4）/ `render/tests/pbr_maps_contract.rs`（9） | 无（场景 JSON、脚本 API、RPC 一字不改；**没挂图时 `params2` 全 0，画面与扩展二十逐像素一致**，`R3E_VISUAL_REGRESSION=1` 基线无需重建）。顶点 stride 56 → 72，内置网格不重算、用 `DEFAULT_TANGENT` 由着色器兜底。**边界**：不读金属度贴图、只有一套 UV、切线装配期一次算定、数据贴图无 mipmap |

| **二十二（已完成：遮挡剔除）** | **上一帧深度金字塔 + 两阶段 AABB 判定**：`r3e-core::occlusion` 纯 CPU 契约（`DepthPyramid::build` 把深度图按 `DEFAULT_TILE_SIZE = 8` 压成 tile 最远深度金字塔、`is_occluded` 用 AABB 投影矩形做「粗筛 → 复核」两阶段保守判定，`OcclusionVerdict::{Visible, Occluded, Rejected}` 只让 `Occluded` 触发剔除）；深度偏置是**相对余量比例**（`DEFAULT_DEPTH_BIAS = 0.05`，按 `(1 - nearest)` 缩放以抵消透视深度的非线性压缩，固定绝对值会让远处物件一个都剔不掉）；空金字塔 / 跨近平面 / 屏外一律 fail-open；`OcclusionSettings`（enabled / depth_bias / tile_size / max_tiles）挂场景 `occlusion` 段，缺省关闭；`r3e-render::occlusion` 的 `OcclusionCuller` 低频**非阻塞**读回深度预通道（`DEFAULT_READBACK_INTERVAL = 4` 帧、`device.poll(PollType::Poll)` 取回、`inflight` 只允许一份在读），`ReadbackLayout` 按 256 字节行对齐（1080p ≈ 8.3 MiB/次），尺寸不符即整帧 fail-open；`prepare_instances` 阶段三 A 在视锥剔除之后接入，**只剔不透明批次**（半透明不写深度），蒙皮先 `inflate_relative(SKIN_CULL_MARGIN)` 再判；`RenderStats::occlusion` 五个字段（enabled / tested / occluded / readback / levels）进日志与 `engine_stats`；脚本 4 个 API（`set_occlusion_enabled` / `occlusion_enabled` / `set_occlusion_bias` / `occlusion_bias`）。详见 §4.15 / §5.15 / §8.21 | `core/occlusion.rs` 新增，`core/scene.rs` 加 `occlusion` 段、`core/lib.rs` 导出；`render/occlusion.rs` 新增，`render/renderer.rs` 接入判定与统计；`script/api.rs` 注册 4 个绑定；`app.rs` 加探针、`app/editor.rs` 透出 `render.occlusion`；新增 `core/tests/occlusion_scene.rs`（8）/ `render/tests/*` 内 6 条 | 无（场景只多一个**可选**段，不写即关闭；**开关关闭时 GPU 命令与实例数与扩展二十一逐字节一致**，`R3E_VISUAL_REGRESSION=1` 基线原样通过、无需重建）。**边界**：无 GPU 遮挡查询、无迟滞（被剔物件下一帧可能回来）、不筛遮挡体、读回滞后 4 帧、只覆盖不透明批次 |

| **二十三（已完成：HDR 立方体反射探针）** | **离线预滤波 cubemap + 按粗糙度取 mip**：`r3e-core::envmap` 纯 CPU 契约——`decode_radiance` 只认 Radiance RGBE（新旧两种扫描行编码、`MAX_EQUIRECT_PIXELS = 64 Mi` 防爆）、`equirect_to_cubemap` 方向驱动双线性（经度环绕、纬度夹极）、`prefilter_specular` 按 `roughness = level/(levels-1)` 做余弦幂卷积（GGX `α = r²` 反解 Phong 指数 `k = 2/α² - 2`、32 次分层黄金角采样）、第 0 层复用原图保清晰反射；`equirect_uv` / `cube_face_dir` / `cube_dir_face` 三个纯函数互逆，`CUBE_FACE_NAMES` 钉死 `+X -X +Y -Y +Z -Z`；`EnvironmentMapSettings` 挂场景 `environment_map` 段（六面必须恰好 6 条否则整段作废、`face_size` 夹 4..1024 并向上取 2 的幂、`mip_levels` 夹 1..10），缺省不序列化；`r3e-render::envprobe` 把成品编码成 `Rgba16Float` 立方体纹素（自实现 `f32_to_f16` / `f16_to_f32`、无行距 padding），绑到**材质组 `@group(1) @binding 6`**（复用 base color 采样器），`CameraUniform` 352 → 368 字节加 `env_probe: vec4`（启用位 / mip 层数 / 强度 / 面边长），着色器 `probe_specular` 用 `textureSampleLevel` 按 `roughness × (levels-1)` 取层、漫反射改用探针第 0 层的辐照度投影（不新增绑定），无探针时 1×1 占位图 + `x=0` 完全短路；换图重建材质 bind group；脚本 8 个 API（`set_env_map` / `set_env_map_faces` / `set_env_map_intensity` / `set_env_map_enabled` / `clear_env_map` / `has_env_map` / `env_map_source` / `env_map_intensity`）；编辑器 `scene.set_environment_map` 补丁式 RPC（`EnvironmentMapPatch`，与 `scene.set_environment` 是两条独立的线）；`app.rs::sync_env_probe` 三道判定（未激活→卸载 / 配置未变→不碰 GPU / 上次失败→不重试）与加载 / 失败 / 卸载三种日志。详见 §4.16 / §5.16 / §8.22 | `core/envmap.rs` 新增（1,288 行）、`core/scene.rs` 加 `environment_map` 段与 `set/clear/active/source` 入口、`core/lib.rs` 导出；`render/envprobe.rs` 新增（415 行）、`render/renderer.rs` 加 `set_env_probe` / `clear_env_probe` / `env_probe_stats`、`shader.wgsl` 加探针分支、`pbr.rs` 绑定槽位；`script/api.rs` 注册 8 个绑定；`editor/lib.rs` `EnvironmentMapPatch` + dispatch；`app.rs` 探针同步与日志、`app/editor.rs` 补丁合并与快照；新增 `core/tests/environment_map_scene.rs`（4）/ `core/tests/env_probe.rs`（6）/ `render/tests/env_probe_shader.rs`（9）/ `editor/tests/environment_map_rpc.rs`（7）/ `script/tests/env_map_api.rs`（4）+ 各自内联 22 / 7 / 7 | 无（场景只多一个**可选**段，不写即不启用；**不配探针时 GPU 命令与像素与扩展二十二逐字节一致**，`R3E_VISUAL_REGRESSION=1` 基线原样通过、无需重建）。**边界**：静态探针（不抓运行中场景）、无平面反射 / SSR、整场景单探针无混合、只认 Radiance `.hdr`、预滤波 CPU 串行（512 面以上加载可感停顿）、无多重重要性采样校正 |
| **二十四（已完成：级联遮挡体按级裁剪）** | **让每级只提交能投进本级的遮挡体**：`r3e-core::shadow_cull` 纯函数模块（零 wgpu）——`aabb_in_cascade` 把 AABB 八角点换到该级光源裁剪空间，**只裁**「八点 `x`/`y` 全在 ±1 外」或「八点 `z > 1`」，靠近光源侧（`z < 0`）与不可信 AABB 一律 fail-open；`plan_casters` 出 `masks[level][caster]`（与 `caster_bounds` 严格同序）、`split_runs` 把掩码切成连续保留段、`fail_open_bounds` / `keep_all` 兜底；`light.rs` 加 `CascadeSettings::caster_extend` / `caster_cull`（缺省 `0.0` / `true`）与 `fit_cascade_extended`（把级近端沿光方向推 `extend`，`extend = 0` 与 `fit_cascade` **逐位相同**，`MAX_CASTER_EXTEND = 65_536`）；`scene.rs` 的 `shadow.casterExtend` / `casterCull` 可选字段（缺省省略往返）；渲染侧 `InstancePlan.caster_bounds` + `ShadowPlan { levels, masks, tested, culled }` + `shadow_runs(groups, mask)`，深度 pass 循环按级取掩码，局部光源走 `mask = None` 全量，`RenderStats.shadow_casters` / `shadow_culled` 进探针与编辑器快照；脚本 3 个 API（`set_shadow_caster_extend` / `set_shadow_caster_cull` / `shadow_caster_cull`）。详见 §4.17 / §5.17 / §8.23 | `core/shadow_cull.rs` 新增、`core/light.rs` 加两字段 + `fit_cascade_extended` + `MAX_CASTER_EXTEND`、`core/scene.rs` 两字段读写回、`core/lib.rs` 导出；`render/renderer.rs` 加 `ShadowPlan` / `ShadowRun` / `shadow_runs` / `caster_bounds` 与逐级掩码接线、stats 两字段；`script/api.rs` 注册 3 个绑定；`app.rs` 探针多打「遮挡体判定 A 裁掉 B」、`app/editor.rs` 快照两字段；新增 `core/tests/shadow_cull_cascade.rs`（4）/ `render/tests/caster_cull_contract.rs`（4）+ `core/src/shadow_cull.rs` 内联 10 / `core/src/light.rs` 内联 3 / `core/tests/cascade_scene.rs` +3；`render/tests/shadow_cascades_shader.rs` 的断言改随 `fit_cascade_extended` 与 `shadow_plan.levels`；重新生成 `assets/scene.schema.json` | 无（两个字段都是**可选**，不写即 `extend = 0` / `cull = true`；`extend = 0` 时拟合矩阵与扩展二十三逐位相同，视觉基线原样通过）。**行为变化**：`casterCull` 缺省开，`shadow_draws` 从「合批数 × 级数」变成「每级保留段数之和」，拿 draw call 当基准的脚本需更新预期。**边界**：AABB 粒度、不做层级剔除、判定 O(级 × 实例)、只裁方向光级联、零散保留会把合批切碎 |
| **二十五（已完成：遮挡体层级剔除）** | **把逐实例判定换成按场景树的层级剔除**：`r3e-core::shadow_cull` 扩出 `CasterNode` / `CasterTree`（`new` 建拓扑 + 自底向上聚子树并集盒与实例数、`refresh` 逐帧只重聚合不动拓扑、父链越界/自指/成环降级成根）、`plan` 自顶向下判（子树盒整块在级外就整枝跳过，混合节点自身盒复判、叶子逐实例）、`expand_masks` 把节点掩码按实例摊平（下标失效 fail-open）；计数拆成 `box_tests` / `instance_tests` / `pruned` / `branch_pruned`，只有 `branch_pruned`（来自多实例子树的部分）是净收益；渲染侧 `InstancePlan.caster_nodes` + `Renderer` 六个缓存字段 + `sync_caster_layout` 按 `scene.layout_version()` 建树，`plan_casters_hierarchical` 返回 `None` 时退回逐实例路径；探针加「盒判定 / 未判即裁 / 其中整枝 / 建树次数」 | `core/shadow_cull.rs`（+`tests/shadow_cull_hierarchy.rs` 13 条）、`render/renderer.rs`、`app/src/app.rs` 探针、`app/src/editor.rs` stats | 无（掩码与扩展二十四逐位相同，视觉基线逐像素一致；场景 JSON 不变） |

| **二十六（已完成：世界法线矩阵奇异保护）** | **scale 含 0 时法线不再产 NaN**：`r3e-core` 新增 `world_normal_matrix`（det 正常且求逆有限时走朴素逆、逐位不变；否则退化为「逐列方向 × 缩放倒数」的兜底分支，三列任一退化时方向退回单位轴）与 `safe_scale_component`（负缩放按原符号夹取），`Transform::normal_matrix` 一并改掉 `scale.max(1e-6)` 抹掉镜像符号的旧写法；`update_world_transforms` 接入新入口；主通道 `vs_main` 在 `normalize` 前加长度判零，退化时退回 `+Y` | `core/transform.rs`（`SINGULAR_DET_EPS` / `MIN_LINEAR_SCALE` / `safe_scale_component` / `world_normal_matrix` / `degenerate_normal_matrix`）、`core/lib.rs` 导出、`core/scene.rs` 接线、`render/shader.wgsl`；新增 `core/tests/normal_matrix_safe.rs`（9）、`render/tests/normal_guard_shader.rs`（3） | 无（健康场景逐位不变，视觉基线原样通过；退化场景从 `NaN` 变成有限值，属缺陷修复不属视觉特性）。**边界**：只修数值不修语义、`world_normal` 仍无下游消费者、父链带 shear 时退化分支为近似 |
| **二十七（已完成：主通道实例收集单次遍历）** | **`prepare_instances` 不再按「键数 × 节点数」重复过滤**：改成一趟 `scene.iter()` 把实例收进共享 `staged_all`、按 `HashMap<(MeshRef, Option<SurfaceKey>, bool), Vec<u32>>` 把下标分进桶（第三位是**段标志**），视锥/遮挡剔除、关节槽位分配、场景包围盒并集跟进同一遍；键的登记顺序（不透明段在前、半透明段次之、段内按首次出现）与半透明两级排序（组间最远深度降序 + 组内 `back_to_front`）原样保留，`groups` 区间与 `caster_bounds` / `caster_nodes` 逐位不变。**同日修复**：首版把键判重写成 `if !buckets.contains_key(&key)`，同一网格的玻璃节点因此登记不进 `transparent_keys`，半透明桶整桶静默丢弃（玻璃面板消失、实例数 10→7、558 坏点），`visual-gpu` 严格模式因此变红；改为按段列表判重 + 桶键带段标志后，本机 RTX 5060 严格模式逐位通过，基线无需重建 | `render/renderer.rs`（`prepare_instances` 主体重写为分桶摊平 + 半透明段丢桶修复）；新增 `render/tests/instance_buckets_contract.rs`（4 → 6：补「键按段登记」「桶键带段标志」两条防回归） | 无（纯 CPU 侧收集路径重构，draw call / 像素不动，视觉基线原样通过）。**边界**：仍单线程、仍每帧从零收集、`HashMap` 不跨帧复用；同键跨两段时实例在两段各出现一次（历史行为，见 §5.20 已知边界） |
两条与顺序相关的说明：

- **阶段五吸收阶段二的纹理与 UV**：引入外部模型必然同时带 UV 与贴图，单独先做「程序化网格加 UV」等于写两遍顶点布局。
  因此阶段二的内容并入阶段五的 P2（材质与贴图）一起落，届时内置 4 种网格也一并补上 UV。
- **阶段五是唯一的结构性改造**：一旦引入外部模型，`MeshKind` 这个"值类型枚举"必须变成"引用类型句柄"，
  渲染器的网格缓存键、物理碰撞体推导、场景 JSON 的 `mesh` 字段语义都要跟着改。
  在此之前 `MeshKind` 枚举是刻意保持简单的——这也是为什么它被排在物理、剔除、阴影之后才做。

阶段五的完整设计（目标、依赖选型、数据结构、P0–P4 拆分与验收标准）见 [`docs/ASSETS.md`](ASSETS.md)。
核心取舍：FBX 用 `ufbx` 解析（单文件 C 库，覆盖模型/材质/贴图/动画四类信息），
第一步只做「解析并打印统计」以验证 C 工具链与数据正确性，不碰渲染器；
坐标与单位在解析期统一换算为「Y-up、单位米」，引擎内部不感知 DCC 差异。

---

## 13. 运行与验证

> workspace 含两个 bin：主程序 `r3e`（`crates/r3e-app`）与 IDE 插件 `r3e-plugin`（`crates/r3e-plugin`）。
> `crates/r3e-app/Cargo.toml` 里的 `default-run = "r3e"` 让仓库根的 `cargo run` 及其带参形态（含 `--features rapier`）
> 默认启动主程序；插件必须显式指定包：`cargo run -p r3e-plugin -- --help`。

```powershell
cd D:\rust3d-engine

# 开发运行（默认 assets/scene.json + scripts/main.rhai）
cargo run

# 指定场景与脚本
cargo run -- assets/scene.json scripts/example_swarm.rhai

# 发布构建（无警告）
cargo build --release
.\target\x86_64-pc-windows-gnu\release\r3e.exe

# 或一键
.\run.bat

# 视觉回归：固定物理像素尺寸跑到第 3 帧截图（与 tests/visual_regression.rs 同口径）
cargo run -- crates/r3e-app/tests/fixtures/visual_regression/scene.json `
              crates/r3e-app/tests/fixtures/visual_regression/static.rhai `
              --window-size 640x360 --capture-frame target\shot.png --capture-after 3

# 真跑像素回归（需要 GPU；无显卡时用例自动跳过并计 passed）
$env:R3E_VISUAL_REGRESSION = "1"; cargo test -p r3e-app --test visual_regression -- --nocapture
# 首建档或确认画面变化是有意的：加 $env:R3E_UPDATE_BASELINE = "1"
```

**实机验证记录**（Release 构建）：

| 项 | 结果 |
| --- | --- |
| GPU 初始化 | RTX 5060 Laptop，Vulkan 后端，窗口正常出图 |
| 渲染 | 1920×1080 窗口，4 网格实例化绘制，无花屏/闪烁 |
| 脚本 | `on_start` / `on_update` 正常调度，`log` 输出可见 |
| 热重载 | 改 `scene.json` 与 `main.rhai` 后 ≤0.4s 生效，场景干净重建 |
| REPL | 输入指令后自动暂停脚本，指令立即反映到画面 |
| 物理 | 默认关：`attach_body` 返回 false、`body_count()` 为 0；`--physics on` 后 5 个刚体落地反弹并进入休眠 |
| 编译 | `cargo build --release` 零 warning |
| 像素回归（扩展二十） | RTX 5060 Laptop / Vulkan 实跑 `R3E_VISUAL_REGRESSION=1 cargo test -p r3e-app --test visual_regression`：`captured_frame_matches_baseline ... ok`（5.20s），子进程日志 `渲染器就绪: 640x360` + `[capture] 第 3 帧截图完成: {"bytes":21049,...,"width":640} \| draws=5 instances=10`，与基准图逐像素比对通过 |
| 材质数据贴图（扩展二十一） | CPU 侧 `cargo test -p r3e-core -p r3e-asset -p r3e-render`：`tangent` 5 / `texture_slot` 3 / `pbr` 4 / `pbr_maps_contract` 9 / `pbr_map_import` 4 全绿；GPU 侧三张图是否真的改变观感需**待人工验证**（拖入带 `NormalMap` / `Roughness` / `AO` 槽位的模型，逐张开关对比亮度与高光形状），无 GPU 断言不下结论 |
| 遮挡剔除（扩展二十二） | CPU 侧 `cargo test -p r3e-core -p r3e-render -p r3e-script occlusion`：`occlusion`（core）28 / `occlusion_scene` 8 / `occlusion`（render）6 / script 2 全绿；`cargo test --workspace` 59 套件 **870 passed / 0 failed**，`--features rapier` **874 passed / 0 failed**；`cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets -- -D warnings`（含 `--features rapier`）均零告警。**像素回归**：`R3E_VISUAL_REGRESSION=1 cargo test -p r3e-app --test visual_regression` → `captured_frame_matches_baseline ... ok`，缺省关闭路径与扩展二十一基线逐像素一致（遮挡剔除计数 `culled=0`），基线无需重建。**观感待人工验证**：打开 `occlusion.enabled` 后「该剔的有没有剔干净」「有没有物件闪进来又闪出去」属于取舍判断，自动化只证明开关关闭时无变化 |
| HDR 立方体反射探针（扩展二十三） | CPU 侧：`envmap`（core 内联）22 / `environment_map_scene` 4 / `env_probe`（core）6 / `envprobe`（render 内联）7 / `env_probe_shader` 9 / `environment_map_rpc` 7 / `env_map_api` 4 全绿；`cargo test --workspace` 63 套件 **928 passed / 0 failed**，`--features rapier` **932 passed / 0 failed**；`cargo fmt --all -- --check` 0 diff；`cargo clippy --workspace --all-targets -- -D warnings` 0 warning。**像素回归**：`R3E_VISUAL_REGRESSION=1 cargo test -p r3e-app --test visual_regression` → `ok`（1 passed，基线无需重建），不配探针时实例数与绘制数与扩展二十二一致（`draws=5 instances=10 culled=0`）。**GPU 实跑取证**（Release，RTX 5060 Laptop / Vulkan，自造 64×32 Radiance 等距柱状图 + IBL 演示场景，`--run-seconds 3`，退出码 0）：`反射探针已加载：来源 equirect / 面边长 64² / mip 5 层 / 强度 1.50`，收尾探针 `HDR 反射探针：段内 true / 开关 true / 来源 equirect / 生效 true / 面边长 64² / mip 5 / 强度 1.50`，同期 `视锥剔除：可见物件 8 / 绘制实例 8 / 剔除 0`。**坏图回退取证**：把 `.hdr` 换成两字节截断文件后实跑 → 仅一行 `反射探针加载失败（… 不是有效的 Radiance HDR（RGBE））：镜面反射退回解析 IBL`，收尾探针 `生效 false / 面边长 0² / mip 0 / 强度 0.00`，进程不 panic、不逐帧重试刷屏，画面退回解析 IBL。**观感待人工验证**：「金属上到底映出了什么」「粗糙度梯度是否连续」属取舍判断，自动化只证明链路与回退正确 |
| 视觉回归进 CI（四道门） | `cargo test -p r3e-app --test visual_regression`：闸门关闭 → `2 passed`（逐像素那条打印跳过原因）；`R3E_VISUAL_REGRESSION=1` 实跑 → `2 passed`（扩展二十四收尾实测 8.03s，RTX 5060 Laptop / Vulkan / 渲染器就绪 640x360 / `[capture] 第 3 帧截图完成: {"bytes":21049,...,"width":640} \| draws=5 instances=10`，基线原样通过、无需重建）。`cargo test --workspace` 66 套件 **966 passed / 0 failed**（扩展二十四收尾为 65 套件 953 passed），`--features rapier` 66 套件 **970 passed / 0 failed**（扩展二十四收尾为 957 passed；本轮 966 = 扩展二十五新增 13 条 core 用例，+4 为 rapier 特有用例）；`cargo fmt --all --check` 0 diff；`cargo clippy -p r3e-app --all-targets -- -D warnings` 0 warning。`ci.yml` 经 YAML 解析确认 `jobs = [fmt, clippy, test, visual, visual-gpu]`（YAML 合法、`visual-gpu` 的 `runs-on = [self-hosted, windows, gpu]` 与 `if: vars.R3E_GPU_RUNNER == 'true'` 均按预期解析）。**严格模式的三个分支实测**（同一台 RTX 5060 Laptop 上手工跑，作为它不会退化成空跑的证据）：① 只设 `R3E_VISUAL_STRICT=1`、闸门未开 → `FAILED 1 passed / 1 failed`，panic 文案点明「闸门必须开着」；② 闸门 + 严格模式下把产物目录用同名文件占住（模拟拍不到图）→ `FAILED`，panic 文案 `严格模式（R3E_VISUAL_STRICT=1）下禁止跳过：建产物目录失败`；③ 闸门 + 严格模式下正常跑 → `2 passed`（5.09s），证明严格模式不会误伤正常路径 |
| 文本上屏（M1） | **待人工验证**：`cargo run` 后左上角出现中文 HUD（标题 / 超宽自动折行副标题 / 每帧刷新的帧率状态行），右上右对齐、中上居中对齐各有样例；字形边缘无黑边、无错位。无头环境无法断言像素，此处只记录待验状态 |
| 音频设备（M1） | 启动日志与收尾探针实测：`后端 rodio / 设备 可用 / 在播 1（实例 1） / 总音量 0.80 / 开关 开 / 错误 0 / 最近 无`；`--run-seconds 3` 全程无 panic、无 rodio 退出提示，退出码 0 |
| 音频出声（M1） | **待人工验证**：按 J 触发 `blip`、K 暂停/续、L 停/重起 `ambience`、M 切总音量；音量是否合适、循环接缝是否可闻需人耳判定（自动化只能断言调用链与状态机） |
| 世界法线矩阵奇异保护（扩展二十六） | `cargo test -p r3e-core --test normal_matrix_safe` → 9 passed；`cargo test -p r3e-render --test normal_guard_shader` → 3 passed；`cargo test --workspace` 68 套件 **978 passed / 0 failed**（扩展二十五收尾为 966），`--features rapier` 68 套件 **982 passed / 0 failed**（原 970）；`cargo fmt --all -- --check` 0 diff；`cargo clippy --workspace --all-targets -- -D warnings` 0 warning。**像素回归**：缺省场景不含 0 缩放，着色器正常路径算式未变（只多读一次长度），基线无需重建。**退化路径取证**：`scale` 某轴设 0 后 `--run-seconds 3` 实跑，画面由整片刷黑变为正常着色（朝上的平光），收尾探针与编辑器快照的 `world_normal` 全部分量有限、无 `NaN` / `inf`；CPU 侧同类断言见 `normal_matrix_safe` 的场景集成用例 |
| 主通道实例收集单次遍历（扩展二十七） | `cargo test -p r3e-render --test instance_buckets_contract` → 6 passed；`cargo test --workspace` 69 套件 **984 passed / 0 failed**（扩展二十六收尾为 68 套件 978）；`cargo fmt --all -- --check` 0 diff；`cargo clippy --workspace --all-targets -- -D warnings` 0 warning。**像素回归**：收集顺序、`groups` 区间、`caster_bounds` / `caster_nodes` 与旧实现逐个相同，缺省场景 draw call / 实例数 / 像素全不变，基线无需重建。**严格模式（本机 RTX 5060 实测）**：`R3E_VISUAL_REGRESSION=1 R3E_VISUAL_STRICT=1` → `captured_frame_matches_baseline ... ok`，与既有基准图逐位一致；修复前同一命令 558 坏点（0.24%，阈值 0.1%）失败，差异区 x290-332 / y137-157 正是玻璃面板。**`--features rapier`**：69 套件 **988 passed / 0 failed**（`r3e-app` 首次链接报 `anon.*.llvm.*` 未定义符号，`cargo clean -p r3e-app -p r3e-render` 后复跑全绿）。 |
| 段归属下沉到节点（扩展二十八） | `cargo test -p r3e-render --test instance_buckets_contract` → 7 passed（新增 `node_enters_only_its_own_segment`）；`cargo test --workspace` 69 套件 **985 passed / 0 failed**（扩展二十七收尾为 984）；`--features rapier` **989 passed / 0 failed**；`cargo fmt --all -- --check` 0 diff；`cargo clippy --workspace --all-targets -- -D warnings`（含 `--features rapier`）0 warning；`cargo build --release` + `r3e.exe --run-seconds 2` 退出码 0（二进制 14,949,888 字节）。**像素回归**：`R3E_VISUAL_REGRESSION=1 R3E_VISUAL_STRICT=1` → `captured_frame_matches_baseline ... ok`，与既有基准图**逐位一致、无需重建** —— 旧的「两段全收」之所以像素也对，是因为不透明段那次绘制自己做完了混合并写了深度、半透明段重画被 `depth_compare = Less` 判等丢弃，只有一次生效；去掉重复份后玻璃改由半透明段按 `back_to_front` 画，单块不重叠玻璃结果相同。**探针取证（改前 → 改后）**：`visual_regression` 夹具主通道实例 **10 → 7**、半透明段透明实例 **3 → 2**、draw call 5 不变；`--run-seconds 2` 缺省场景主通道实例 8 / 透明实例 0（缺省场景无半透明物件）。 |

---

## 14. 关键约束速查

写场景 / 脚本时最容易踩的几条：

1. 物件 `name` **全局唯一**，重名 `spawn` 直接失败。
2. 父物件必须**先于**子物件定义（JSON 里 `objects` 的顺序有意义）。
3. 脚本逻辑**必须写进 `on_start` / `on_update`**，顶层语句会被每帧重复执行。
4. 脚本函数名是 `create_node` / `remove_node`，不是 `spawn` / `despawn`。
5. 角度单位是**度**（`set_rot` / `orbit` / `fov`），但 `rotate_axis` 是弧度。
6. `set_light` 的 `direction` 是**光线行进方向**，要照亮地面就给负 Y 分量。
7. `color` 是**线性空间**值，不是 sRGB；写 0.5 不等于视觉上的中灰。
8. 相机 `aspect` 由渲染器每帧覆盖，写在 JSON 里无效。
9. 热重载会**重建整个场景**，脚本里的运行时累积状态（计数器、随机序列）会归零。
10. REPL 输入会自动暂停脚本驱动，要恢复动画按空格。
11. 阴影**默认开启**（`light.shadow` 缺省 = `enabled:true`、2048²、正交范围自适应）；要关掉写 `"shadow": { "enabled": false }` 或脚本 `set_shadow(false)`。
12. `set_shadow_extent` 传 0 表示"跟着场景包围球自适应"；写死一个小的半宽会把远景影子裁掉（表现是远处物件突然没有影子，不是黑块）。
13. 编辑器 RPC **默认不开**：不带 `--rpc-port` 时连监听都没有；开了也只绑 `127.0.0.1`，不对外网暴露。
14. 编辑器请求的改动**发生在主线程**（`drain_editor_jobs`），不要指望多线程并发写场景；同一帧内多请求按到达顺序执行。
15. 队列满回 `-32002`、等回包超时回 `-32003`、无权限回 `-32001`——客户端要按码退避重试，别当成引擎崩溃。
16. 屏幕文本按 **`key` 寻址**：同一 key 重复 `text_add` 是**覆盖**（位置/字号/颜色被刷新，对齐与换行保留），不是叠加；要画多行就换 key 或用 `\n` 换行。
17. 文本坐标是**屏幕像素**原点在左上角、Y 向下，与 `set_resolution` 无关；窗口缩放后按新尺寸重算，别把坐标写成 0~1 的比例值。
18. 文本通道**不吃深度**：文字永远在最上层，不存在"被物体挡住"；反过来也无法用文本做贴墙标牌。
19. 找不到中文字体时文本**不报错只是不出字**（图集为空），先看启动日志里的"文本：已加载系统字体 / 未找到可用字体"。
20. UI 元素同样按 **`key` 寻址**：重复 `ui_rect` / `ui_border` / `ui_image` 是覆盖（保留 `layer` / `visible` / `uv`），不是叠加。
21. UI 坐标是**锚点 + 偏移**，不是左上角绝对坐标：写 `topright` 就该给负的 x 偏移把元素往里收；尺寸/偏移都可写 `%`，窗口 resize 后自动跟随。
22. `ui_border` 的矩形是**外框**：边框画在给定矩形之外，所以要和同尺寸的 `ui_rect` 对齐时两者给一样的几何参数即可，不需要减厚度。
23. 图片加载失败**不崩也不刷日志**：该元素退化成纯色块、同一路径只尝试一次；换图用 `ui_set_image` 或改名后热重载脚本。
24. UI 与文本的前后关系是**通道级**开关（`set_ui_above_text`），不是逐元素：需要"某块 UI 盖住文字"时整体切换，或把该元素画成图片/自绘。
25. 音频 key 与文本/UI 的 key 是**两套命名空间**，互不影响；但音频 key 重复播放是**重播**（停旧播新），不是覆盖刷新，也不是叠加。
26. 音频音量两层相乘：`audio_set_volume` 是单条，`audio_set_master_volume` 是总音量，最终有效音量 = 两者乘积，别指望其中一个能"盖住"另一个。
27. `audio_set_loop` 改循环会**从头重播**（rodio 的循环在解码源构造时确定），不要用它做"无缝切换当前播放位置"。
28. 没有声卡时引擎**不报错也不崩**：`audio_available()` 返回 `false`、`audio_backend()` 返回 `"silent"`，`audio_play` 仍返回 `true`（文件解码通过）但不会出声——排查"为什么没声音"先看这两个值。
29. 数据贴图导出必须**线性**：法线 / 粗糙度 / AO 若按 sRGB 导出，采样时被自动线性化，法线整体偏软、AO 偏亮；引擎按槽位名自动选格式，但槽位名不认识时（`Unclassified`）按颜色处理。
30. FBX 的 `bump` 槽位是**高度图不是法线图**，引擎不当法线用；要做凹凸请导 `NormalMap`（或 `Maya|normalCamera`），否则表现为"贴了图但没有凹凸感"。
31. 法线贴图依赖切线基：网格 UV 退化（整片 UV 相同）时切线是兜底基，凹凸方向会与预期不符——这是 UV 的问题，不是贴图的问题。
32. AO **只压环境光**，不会让直接光变暗；想看到明显差别需要有环境光/IBL 或阴影之外的暗部，光秃秃一个方向光下几乎看不出 AO。
33. 三张贴图**没有对应脚本 API 与 RPC**，只能由导入的模型自带；手动换图要改资产或走 base color 通道。
34. 遮挡剔除**默认关闭**：场景写 `"occlusion": { "enabled": true }` 或脚本 `set_occlusion_enabled(true)`；打开后前几帧仍不生效（金字塔要在读回路上跑一圈），日志里表现为「段内开关 true / 生效 false」，不是开关坏了。
35. 遮挡剔除**只剔不透明批次**：玻璃、水面等半透明永远不参与（半透明不写深度，判了也是错的），想让它们省下 draw 得另做方案。
36. 遮挡剔除用的是**上一帧**深度（默认滞后 4 帧），镜头快速平移 / 旋转时可能看到物件先消失又出现；这是「低频读回 + 无迟滞」的取舍，不是渲染 bug。
37. `depth_bias` **越大约保守**（越难剔）：默认 `0.05`，调到 `0` 最容易剔也最容易误剔；它是相对余量比例而非世界单位，换相机近远平面不必重调。
38. 遮挡判定只看**实例 AABB 在屏幕上的矩形**：细长遮挡体（栅栏、树枝）与「从门缝里露一条」这类遮挡关系判不出来，别期待它等价于真实可见性，它只负责扔掉「整块被墙挡住」的那批。

---

## 15. 音频层：r3e-audio（3D 空间化）

M1 的收尾项，目标是**让脚本能放出声音，而且放不出声音时引擎不许崩**；
路线表第十项在此基础上加了**3D 空间化**（§15.8），但保留了同一句底线：不调空间化 API 时，行为与 M1 逐位一致。

### 15.1 后端选型

候选三个，最终选 **`rodio`**（其内部就是 `cpal` 播放 + `symphonia` 解码）：

| 方案 | 取舍 | 结论 |
| --- | --- | --- |
| `kira` | 功能最强（tween、mixer、clock、未来直接上 3D 声源），但是异步/时钟架构，需要独立的 `AudioManager` 生命周期与 `update()` 时间推进，和本仓库"同步、单线程、每帧显式 tick"的写法正好相反；依赖树也更重 | 否。等真要做空间化/复杂音效图时再考虑，届时 `AudioBackend` 契约可直接对接 |
| `cpal` + `symphonia` 手搓 | 零中间层、最可控，但要自己写回调、样本格式转换、重采样、混音累加与断点续播，一次性代码量数百行且极易出爆音/欠载 | 否。这是 `rodio` 已经做完的事，自己写等于重造且更容易出 bug |
| **`rodio`** | **同步 API**（`Player::append` 即播）、纯 Rust 无 unsafe（`#![forbid(unsafe_code)]` 级别的依赖约束）、解码格式由 symphonia 覆盖 wav/ogg/mp3/flac，`Player` 天然支持每实例音量与暂停 —— 与本工程"门面管策略、后端只出声"的分层高度契合 | **选它**。`rodio 0.23` + `cpal 0.18`，全 workspace 因此新增 25 个传递依赖，仍在"轻依赖"可接受范围 |

### 15.2 分层

```
script/api.rs   audio_play/...            ← 只做参数转换与错误转 bool
   ↑
r3e-audio::Audio（门面）                    ← 策略全在这里：账本 HashMap、总音量、开关、错误计数、降级
   ↑ trait AudioBackend
r3e-audio::backend::RodioBackend / SilentBackend
   ↑
rodio → cpal（设备） + symphonia（解码）
```

- **`AudioBackend` 契约**：`play/stop/stop_all/pause/resume/set_volume/is_playing/is_paused/active_keys/update`。
  后端**不知道**总音量与 key 语义，只接收"已经算好的有效音量"，因此可以被 `MockBackend` 整体替换——9 个单测全在无声卡环境跑。
- **`r3e-audio` 不依赖 `r3e-core`**：音频实例不进 `Scene`（音频不是场景数据，是运行时状态），
  依赖方向只有 `r3e-app → r3e-audio`、`r3e-script → r3e-audio`，core 依旧零设备依赖。
- **主循环接入点**：`EngineApp::new` 里 `Audio::open_default()` 一次，`ScriptHost::new_with_audio` 注入同一份 `Rc<RefCell<Audio>>`；
  `step_and_render` 每帧 `audio.update()` 回收播完的实例（暂停时也跑，避免恢复后残留空 `Player`）。

### 15.3 音量模型与混音

- **有效音量 = 单条音量 × 总音量**，每次下发前算好；`set_master_volume` 会遍历账本逐条重算，
  所以后端不需要支持 master 音量，换成任何后端语义都不变。
- **多路混音**：一个 key 一个 `rodio::Player`，全部接到同一个 `MixerDeviceSink` 的 mixer 上，由 rodio 做样本累加。
  这就是"简单混音"——没有总线、没有效果器、没有优先级抢占。
- **音量清洗**：`sanitize_volume` 统一处理 NaN → 0、负数 → 0、>1 → 1。
  脚本侧与门面共用同一个函数，不存在两套 clamp。`0` 是合法的（静音但不停止，仍算在播）。

### 15.4 降级策略（关键：引擎不因音频崩）

| 情况 | 行为 |
| --- | --- |
| 打开设备失败（无声卡 / 被独占 / 驱动异常） | `Audio::open_default` 捕获错误 → 换 `SilentBackend` → `warn!` 一行 + 记 `last_error`；**引擎照常启动**，`audio_available()` 返回 `false`，`audio_backend()` 返回 `"silent"` |
| 静默模式下的 `play` | **仍会真正解码一次文件**：解码成功返回 `true`（脚本逻辑不用改写），解码失败返回 `false` 并记错。这样"没声卡"和"文件坏了"是两种可区分的情况，不会互相掩盖 |
| 文件不存在 / 解码失败 | `play` 返回 `false`，`last_error` 记"打开/解码失败 + 路径 + 原因"，`error_count += 1`；不 panic、不中断脚本 |
| 音频通道被 `audio_set_enabled(false)` | 先停掉全部实例并清空账本，之后 `play` 直接返回 `false` 并记一条错误（避免"明明关了还在响"） |
| 空 key | 拒绝播放，返回 `false` 并记错（空 key 无法寻址，静默丢弃比崩掉好） |
| 正常退出 | `log_on_drop(false)` 关掉 rodio 退出时的提示行，收尾日志保持干净 |

### 15.5 同名 key 语义（写进文档的行为约定）

**同名 key 再次播放 = 停掉旧实例、从头重播。** 既不叠加（不会出现两份声音），也不拒绝（不返回错误）。

理由：脚本里最典型的用法是"按键触发音效"，连打时必须只响最新的那一份；
若采用叠加语义，玩家狂按键盘会瞬间堆出几十个 `Player` 把内存和混音推爆。
脚本层 `audio_play` / `audio_play_ex` 与门面 `Audio::play`、`set_loop`（改循环同样是重播）共用这一条语义，
由 `audio_duplicate_key_does_not_stack_or_log_errors` 单测锁住。

### 15.6 格式与素材

- **格式**：wav / ogg（Vorbis）/ mp3 —— 由 `Cargo.toml` 里显式开启的 rodio feature 决定：
  `default-features = false` + `playback` / `wav` / `vorbis` / `mp3`。
  **未开 flac / aac / opus**：`default-features = false` 关掉了 `symphonia-all`，
  这些格式的 `Decoder` 会在构造时报错（`play` 返回 `false` 并记入 `audio_last_error`），不是静默忽略。
  需要加格式时，在 workspace `Cargo.toml` 的 rodio features 里补对应项再重编。
- **素材来源**：仓库原本没有音频资产，因此 `assets/audio/` 下的两个 wav 由脚本合成（16-bit PCM 单声道，
  非第三方素材，无版权问题）：
  - `blip.wav`：880 Hz 指数衰减短音效（约 0.18 s），用于按键触发；
  - `ambience.wav`：220 Hz + 330 Hz 双音铺底（44.1 kHz 单声道 2.0 s，首尾相接无明显接缝），用于循环播放演示。
- **路径解析**：脚本里给相对路径（`assets/audio/blip.wav`），由 `r3e-app` 的工作目录兜底解析，与 `ui_image` 的路径规则一致。

### 15.7 测试覆盖与未验证项

单测 29（`r3e-audio`：门面 18 + 空间化 11）+ 11（`r3e-script` 脚本语义）：

1. 缺失文件 → `Err`，不 panic，`error_count` 递增；
2. 非法内容（随机字节伪装 `.bin`）→ 解码失败 `Err`，不 panic；
3. 音量边界：`-1 → 0`、`2 → 1`、`NaN → 0`；
4. 默认音量与 `play_default` 的下发值；
5. 同名 key 重播：`plays` 计数递增但实例数恒为 1；
6. 静默后端：`device_available() == false`，`play` 仍返回 `Ok`（解码通过），但 `is_playing()` 恒 `false`、在播数不虚高；
7. 禁用通道：`play` 被拒 + 停掉全部；
8. 空 key 被拒；
9. `update()` 回收播完实例（`MockBackend` 驱动）；
10. 脚本侧 5 项：静默降级、加载失败返 false 并记账、音量夹取、重复 key 不堆叠、开关切换阻止播放；
11. 空间化 11 项（`spatial.rs` 纯函数）：模型名往返与别名、`SpatialSettings::sanitized` 夹取、三种模型在 `min` / 中点 / `max` 的衰减值与单调性、平滑窗让逆距离曲线在 `max` 处归零、`NaN` / `inf` 距离保守静音、径向速度正向/反向/重合退化、多普勒静止为 1 与超音速夹取、`doppler_factor == 0` 恒 1、`SpatialSource::advance` 在 `dt <= 0` 时不更新速度但仍重算增益。

**待人工验证**：真实出声无法自动断言 —— 自动化只能证明"调用链不 panic、状态机正确"，
音量是否合适、`blip` 是否刺耳、循环 `ambience` 接缝是否可闻，以及空间化后的多普勒听感是否自然，
需要人带着耳朵跑一次 `cargo run` 后按 J/K/L/M 听；空间化另有 `scripts/spatial_audio_demo.rhai`（见 §8.18）。

### 15.8 3D 空间化（路线表第十项）

**定义**：声源能挂到场景节点上，按到听者的距离衰减、按相对速度做多普勒，听者每帧由**活动相机**推导。
**边界**：不做 HRTF、不做左右声像（相机 `forward` 只记录不参与增益，是明确的取舍而非遗漏）、不做遮挡衍射与房间混响。

#### 分层（`r3e-core` 依旧零设备依赖）

```
script/api.rs    24 个 audio_spatial_* API         ← 只做参数转换与错误转 bool
   ↑
r3e-audio::Audio（门面）                            ← 覆盖层：spatial 注册表 + 增益/倍速合成 + 开关 + 上限
   ↑            ↑
spatial.rs（纯函数层）  trait AudioBackend          ← 契约新增 set_speed / supports_speed
            attenuation / radial_velocity / doppler_ratio      （默认实现 → 老后端不改也能编）
```

- **`SpatialSource` 不进 `Scene`**：音频不是场景数据而是运行时状态，位置解析成坐标是 `app` 的事，
  所以 `r3e-audio` 只存节点**名字**，`spatial_bindings()` 把「key → node」交给宿主每帧解析（§8.18）。
- **`AudioBackend` 契约增量是向后兼容的**：`set_speed` / `supports_speed` 带默认实现（忽略 / 返回 `false`），
  `MockBackend`、`SilentBackend` 不必改签名即可通过；`RodioBackend` 把倍速夹在 `0.1 ..= 8.0` 再下发。

#### 增益与倍速（三个纯函数，CPU 侧可单测）

| 量 | 规则 |
| --- | --- |
| 距离衰减 `attenuation(d, s)` | `d <= min` → `1.0`（近场不放大）；`d >= max` → `0.0`（城外**完全听不见**，不是很小声）；`NaN` / `inf` → `0.0`（保守静音，绝不把 `NaN` 灌进后端污染这条实例的音量）。`Inverse` = `min/d`、`Exponential` = `(min/d)²`、`Linear` = `1 - t`，前两者额外乘 `1 - smoothstep(t)` 平滑窗——逆距离曲线在 `max` 处本不为 0，不乘窗就会"啪"地静音，玩家能听出断点 |
| 径向速度 `radial_velocity(v, p, l)` | 源速度在「源 → 听者」单位向量上的投影，正 = 靠近、负 = 远离；两点重合（< 1e-4）无方向可言 → `0` |
| 多普勒 `doppler_ratio(v, s)` | `c / (c - v·factor)`：静止 `1.0`、靠近 `> 1`（音高升）、远离 `< 1`；`factor == 0` 恒 `1.0`；分母被推过零（超音速）时夹到 `MAX_DOPPLER_SHIFT` 上下限，**绝不返回负数或 0** |

- **速度来自位置差分 + 一阶平滑（系数 0.4）**：脚本瞬移节点时不做平滑会出现单帧 300 m/s 的假速度，音高"咔"一下。
- **有效音量 = 单条音量 × 总音量 × 距离增益**：增益是**乘在既有音量上的系数**，所以 §15.3 的音量模型不变，
  只是多一个因子；`set_master_volume` 逐条重算时同样带上增益，不存在两套算法。
- **倍速只对已注册的 key 生效**，且只在 `supports_speed()` 为真时下发：静默后端仍然算（探针里 `rate` 是真实值），只是不下发。

#### 状态与回退

| 操作 | 语义 |
| --- | --- |
| `attach_spatial(key, s)` | 空 key 拒绝；**已注册 → 只更新设置、保留位姿**（脚本调完参数不用重新摆位置）；超过 `MAX_SPATIAL_SOURCES = 32` 拒绝并记错误，**不挤掉已有声源** |
| `attach_spatial_node(key, node, s)` | 先注册再记节点名；`node` 为空串等价"不绑定"（位置纯由脚本给） |
| `detach_spatial` / `clear_spatial` | **不影响播放**：声音继续以平面音量播着，只是不再被衰减——解除的是"覆盖层"，不是"声音" |
| `set_spatial_enabled(false)` | 全部 key 的增益与倍速立即回 `1.0`（等价 M1 平面声），注册表保留，重新开启即恢复 |
| `tick_spatial(dt)` | 按 key 有序推进并逐条下发 `set_volume` / `set_speed`；`dt <= 0` / `NaN` 按 0 处理（只重算增益、不动速度），`dt` 上限夹 `0.25` 防掉帧尖峰 |

**为什么多普勒落到"倍速"而不是"变调滤镜"**：`rodio::Player` 只有 `set_speed`（改采样推进速率），
它同时改音高与时长——正是多普勒在物理上该有的效果，且不需要额外 DSP 链；代价是只对有设备的后端成立（见上）。

---

## 附：模块与文件对照表

| 文件 | 核心内容 |
| --- | --- |
| `crates/r3e-core/src/transform.rs` | `Transform`、欧拉角互转、`look_at`（含取逆）、`local_matrix`、`normal_matrix` |
| `crates/r3e-core/src/node.rs` | `NodeId`、`MeshKind`（含别名解析）、`Node` 字段与 `world_position` |
| `crates/r3e-core/src/scene.rs` | `ObjectDesc` / `PhysicsDesc` / `CameraDesc` / `LightDesc` / `SceneFile`、`Scene` 三索引、生命周期、`resolve_world`、`from_json_str` / `to_scene_file` / `save_to_path`（加载与保存严格对称） |
| `crates/r3e-core/src/camera.rs` | `Camera`、DirectX 风格投影、`orbit` |
| `crates/r3e-core/src/text.rs` | `FontSet`（系统字体查找 / 指定字体加载）、`GlyphSource` 抽象、`layout_text`（换行 / 对齐 / 缺字回退）、`ScreenText` / `TextAlign` |
| `crates/r3e-core/src/ui.rs` | `UiElement` / `UiKind` / `Anchor` / `UiUnit` / `UiValue`、`resolve_rect`（锚点 + 像素/百分比解析）、`slice_margins`（九宫格边距换算 + 夹取）、`sort_by_layer`（稳定排序） |
| `crates/r3e-core/src/glyph_atlas.rs` | `GlyphAtlas` 分页图集、`ShelfAllocator` 货架分配、`GlyphKey`（face+glyph+像素量化） |
| `crates/r3e-core/src/light.rs` | `Light` 默认值与语义、`ShadowSettings`（含 `sanitized`）、`shadow_view_projection` 正交光源相机；扩展十五的 `LocalLight` / `LocalLightKind`、`attenuation` / `spot_factor` / `irradiance` CPU 参考实现、`upsert_local` / `remove_local` / `with_local`、`MAX_LOCAL_LIGHTS = 16` 等常量；扩展十六的 `CascadeSettings`（含 `sanitized`）、`cascade_splits` / `frustum_slice_corners` / `fit_cascade` 三个纯函数、`MAX_CASCADES = 4` |
| `crates/r3e-core/src/material.rs` | `SurfaceMaterial` 与 `sanitized`、`GeoTerms`、GGX / Smith / Schlick / `cook_torrance_brdf` / `direct_light` 纯函数参考实现（零 wgpu 依赖） |
| `crates/r3e-core/src/env.rs` | `EnvironmentSettings`（7 字段）与 `sanitized`、`Sh9::project` 球谐投影、`env_brdf_approx` 解析环境项（零 wgpu） |
| `crates/r3e-core/src/postfx.rs` | `PostFxSettings` / `BloomSettings` / `SsaoSettings`（扩展十七挂载）/ `TonemapMode`、`sanitized`、`expose` / `bright_pass` / `tonemap` / `fxaa_edge_visible` / `resolve` 参考实现（零 wgpu） |
| `crates/r3e-core/src/ssao.rs` | `SsaoSettings`（5 字段 + `sanitized` / `is_neutral` / `effective_samples`）、`hemisphere_kernel`（确定性半球核）、`reconstruct_view_pos`、`normal_from_neighbors`、`is_occluded`、`ao_from_occlusion`、`MAX/MIN/DEFAULT_SSAO_*` 常量（零 wgpu） |
| `crates/r3e-render/src/mesh.rs` | `Vertex` / `InstanceRaw` 布局、`cube` / `sphere` / `plane` / `cylinder` |
| `crates/r3e-render/src/renderer.rs` | `Renderer` 初始化、`CameraUniform`、`ShadowUniform` / `ShadowPassUniform`、`LocalShadowUniform`（扩展十八 C：`view_proj[24]` + `slots[16]`，`@group(3) @binding(7)`）、`LightsUniform`、`prepare_instances`、`prepare_shadow`、`prepare_local_shadow`（按层写矩阵并返回要跑的层号）、`render`（方向光阴影 pass → 局部阴影逐层 pass → 主 pass 写 HDR 目标 → `SsaoPass::render` 两个遮蔽 pass → `PostFxPass::render` composite → 文本 / UI）、`resize`、`RenderStats` |
| `crates/r3e-render/src/text.rs` | `TextPass`（独立无深度管线）、`build_text` 顶点装配、图集动态纹理按需扩容、`MAX_TEXT_INSTANCES` 安全阀 |
| `crates/r3e-render/src/ui.rs` | `UiPass`（单管线覆盖矩形/边框/图片）、`plan_ui` 纯 CPU 建批、`plan_slice`（九宫格切 3×3）、`UiImageProvider`、`images_to_load`、`MAX_UI_QUADS` 安全阀（九宫格原子丢弃）、`UiStats` |
| `crates/r3e-render/src/ui.wgsl` | UI 着色器：屏幕像素 → NDC、纹理/纯色调制、alpha 混合 |
| `crates/r3e-render/src/text.wgsl` | 文本着色器：屏幕像素 → NDC、图集 R8 采样、alpha 混合 |
| `crates/r3e-render/src/shader.wgsl` | Cook-Torrance 前向着色（GGX / Smith / Schlick）、伴随矩阵法线变换、`vs_shadow` 深度 pass（方向光级联与局部光源阴影共用）、3×3 PCF 阴影采样、`LocalShadow` / `local_shadow_factor`（点光选面 + 聚光单面，扩展十八 C）、环境光与自发光不吃阴影 |
| `crates/r3e-render/src/pbr.rs` | `MaterialUniform`（48 B）组装、`SurfaceKey`（`to_bits` 精确键）、`MaterialData` 与 `overlay`（物件级覆盖文档级） |
| `crates/r3e-render/src/postfx.rs` | `PostFxPass`：`Rgba16Float` 离屏目标、三级泛光纹理、composite pipeline、`PostFxParams`（64 B）、`PostFxStats`、`planned_passes` 预算 |
| `crates/r3e-render/src/postfx.wgsl` | 亮部提取 / 降采样 / 加性上采样 / composite（曝光 → Reinhard·ACES → FXAA，遮蔽图在此乘入）四个片元入口 |
| `crates/r3e-render/src/ssao.rs` | `SsaoPass`：`R8Unorm` 遮蔽图两份（raw / final）、生成 + 散焦两条全屏管线、`SsaoParams`（416 B，含 16 样本核）、`SsaoStats`、`generation` 世代号、`planned_passes` |
| `crates/r3e-render/src/ssao.wgsl` | 全屏三角形顶点现算、`fs_ssao`（深度重建 + 半球核遮挡率）、`fs_blur`（4×4 盒式散焦） |
| `crates/r3e-physics/src/lib.rs` | `PhysicsBackend` trait、`BackendCaps`、`BodyDesc`、`NullBackend`、`BuiltinBackend`、`PhysicsConfig`、`Physics` 门面、查询层契约（`Ray` / `OverlapHit` / `CastShape`） |
| `crates/r3e-physics/src/rapier_backend.rs` | Rapier 后端：刚体/碰撞体映射、接触求解、碰撞事件回传、parry 窄相查询（raycast / overlap / shape-cast） |
| `crates/r3e-audio/src/lib.rs` | `Audio` 门面：实例账本、单条 × 总音量、通道开关、错误计数、`sanitize_volume`、静默降级；空间化覆盖层（注册表 / `tick_spatial` / 增益与倍速合成 / 开关 / 32 上限）、18 个单测（含 `MockBackend`） |
| `crates/r3e-audio/src/backend.rs` | `AudioBackend` 契约（含 `set_speed` / `supports_speed`）、`RodioBackend`（一 key 一 `Player` + mixer 混音，倍速夹 `0.1 ..= 8.0`）、`SilentBackend`（解码校验但不出声，`supports_speed() == false`） |
| `crates/r3e-audio/src/spatial.rs` | 3D 空间化纯函数层（零设备依赖）：`DistanceModel` / `SpatialSettings`（含 `sanitized`）、`attenuation` / `radial_velocity` / `doppler_ratio`、`ListenerPose`、`SpatialSource`（位置差分 + 0.4 一阶平滑的速度估计、`advance`）、`MAX_SPATIAL_SOURCES = 32`、`MAX_DOPPLER_SHIFT`、11 个单测 |
| `crates/r3e-core/src/air.rs` | AIR 契约与 12 条不变量、`finalize` / `audit_entry` 统一校验 |
| `crates/r3e-core/src/procedural.rs` | 内置程序化图元（cube / sphere / plane / cylinder）统一产出 `MeshData` |
| `crates/r3e-asset/src/lib.rs` | FBX 导入（ufbx）、材质/贴图元数据、动画与蒙皮数据、`AssetLibrary` |
| `crates/r3e-asset/src/importer.rs` | `AssetImporter` + `ImporterRegistry`：按扩展名分派，未知扩展名显式报错 |
| `crates/r3e-asset/src/gltf_import.rs` | glTF / GLB 导入（gltf 1.4.1）与 GLB 头完整性校验 |
| `crates/r3e-script/src/lib.rs` | `ScriptHost`、加载/每帧/热重载/重放、`has_fn` |
| `crates/r3e-script/src/api.rs` | 全部 Rhai API 绑定、`Clock`、`ScriptContext`、xorshift 随机 |
| `crates/r3e-app/src/app.rs` | `EngineApp`、帧循环、输入、文件监视、REPL 通道、`drive_spatial_audio`（听者 ← 活动相机、声源 ← 绑定节点世界坐标，再 `tick_spatial`） |
| `crates/r3e-app/src/main.rs` | 参数解析、项目根三级兜底、事件循环启动 |
| `docs/PHYSICS.md` | 物理子系统完整说明：三层开关、场景字段、脚本 API、内置规则、实测与限制 |
| `docs/ASSETS.md` | 资产管线完整说明：FBX / glTF 导入、AIR 统一格式与不变量、动画/蒙皮、分阶段验收与已知限制 |
*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
