# M0-S1：`renderer.rs` 拆分方案（5582 行 → 每文件 ≤800）

> 目标：把 `crates/r3e-render/src/renderer.rs` 从单文件 5582 行拆成 `renderer/` 目录下的多文件模块，
> 单文件全部 ≤800 行，并从 `xtask/size_debt.json` 删除该条目，让 `lint-size` 阈值直接管住它。
> **零行为变更**：不改任何渲染逻辑、不改任何对外 API 路径、不改测试语义。

## 1. 实测基线（不是文档里的 4487）

| 项 | 实测值 | 来源 |
| --- | --- | --- |
| `renderer.rs` 行数 | **5582** | 2026-11-15 实测 |
| 其中非测试代码 | 5141 | `5142-5582` 为两个内嵌 `#[cfg(test)] mod` |
| 其中内嵌测试 | 441 | `morph_table_tests` 341 + `morph_upload_tests` 100 |
| `size_debt.json` 登记 | `renderer.rs: 5582`，`limit: 800` | `xtask/size_debt.json` |
| 单测基线 | `r3e-render = 307` | `xtask/tests_baseline.json` |
| 门禁 | `fmt --check` / `clippy -D warnings`（default + rapier）/ `test` / `lint-size` / `check-tests` / `dep-check` / 像素回归 | `.github/workflows/ci.yml` |

对外引用面（决定 re-export 兜底范围）：

- `lib.rs:43` `pub use renderer::{CapturedFrame, CullStats, LayeredProvider, LightsUniform, LocalLightRaw, LocalShadowUniform, MeshCache, RenderStats, Renderer, LIGHTS_UNIFORM_SIZE, LOCAL_LIGHT_STRIDE, LOCAL_SHADOW_NORMAL_BIAS, LOCAL_SHADOW_SLOT_STRIDE, LOCAL_SHADOW_UNIFORM_SIZE, MSAA_SAMPLE_COUNT}` —— **15 项，一个都不能少**。
- 集成测试直接走子路径：`tests/caster_cull_contract.rs`、`instance_upload_contract.rs`、`joint_upload_contract.rs` 用 `r3e_render::renderer::{RenderStats, InstanceUploadStats}`，`tests/local_shadow_contract.rs` 用 `r3e_render::renderer::LocalShadowUniform`。
- 跨 crate 使用 `Renderer`：`r3e-runtime/src/{app,port,capture}.rs`、`r3e-editor-host/src/api.rs`、`r3e-app/tests/visual_regression.rs`、`r3e-render/src/gfx/{api,mod}.rs`。全部只经 `r3e_render::Renderer`，不依赖文件位置。

## 2. 三条硬约束

1. **路径不变**：`renderer.rs` → `renderer/mod.rs`，模块名仍是 `renderer`；`lib.rs` 的 `pub mod renderer;` 与原 `pub use renderer::{...}` **一行不改**。子模块类型在 `mod.rs` 里 `pub use` 回同一路径，使 `r3e_render::renderer::RenderStats` 等旧路径继续有效。
2. **字段可见性放宽到 `pub(crate)`**：`Renderer` 的 78 个字段全部 private，`impl Renderer` 分散到多个文件后子模块拿不到私有字段。做法是**只把字段可见性改成 `pub(crate)`**（结构体定义位置保持不变，仍是 `pub struct Renderer`），对外部 crate 完全不可见，属机械改动。**不拆字段成子结构体**——那是逻辑重构，违反本次「只搬家不改逻辑」。
3. **禁止顺手重构**：`prepare_instances`（643 行单函数）、`render_to_view`（332 行单函数）、`Renderer::new`（610 行单函数）**整体搬迁，不切子函数、不合并分支、不改注释**。若某个文件必须改逻辑才能压到 800 行以下，按熔断处理（见 §6）。

## 3. 目标文件树

```
crates/r3e-render/src/renderer/
├── mod.rs            110   模块声明 + re-export + 顶层常量 + 头 use
├── state.rs          273   Renderer 结构体（字段 pub(crate)）+ 全部只读/简单访问器
├── uniforms.rs       363   max_axis_scale / MaterialKey / CameraUniform / LocalLightRaw /
│                           LightsUniform / ShadowUniform / LocalShadowUniform / ShadowPassUniform
├── mesh_cache.rs     435   GpuMesh / GpuPart / MeshMorph / LayeredProvider / MeshCache /
│                           MAX_MORPH_TABLE_FLOATS
├── stats.rs          197   CapturedFrame / CullStats / InstanceUploadStats / StagingStats / RenderStats
├── staging.rs        259   UploadStaging + impl + staging_enabled_from_env
├── material.rs       342   GpuMaterial / MaterialCache
├── types.rs          180   EnvProbeGpu / DrawGroup / InstancePlan / ShadowRun / ShadowPlan /
│                           MorphUpload + morph_upload / shadow_runs / ShadowSubmit
├── buffers.rs        363   upload_texture / create_*_texture / create_shadow* / create_msaa_targets /
│                           create_depth_target / Renderer::resize
├── init.rs           735   Renderer::new / set_env_probe / clear_env_probe / env_probe_stats
├── frame.rs          507   render / acquire_surface_frame / render_to_view / capture_frame
├── shadow.rs         439   prepare_shadow / plan_casters_hierarchical / layout_slots /
│                           sync_caster_layout / sync_cull_layout / resize_shadow / draw_shadow_pass /
│                           draw_depth_prepass / prepare_local_shadow / local_shadow_size / _layer_count
├── instancing.rs     663   prepare_instances
├── upload.rs         235   upload_mesh / upload_morph_table / upload_instance_runs /
│                           upload_joint_runs / mesh_material_alpha
├── pass.rs           104   draw_group_range / prepare_materials / prepare_local_lights
├── backend.rs         24   impl crate::gfx::RenderBackend for Renderer
└── tests.rs          441   morph_table_tests + morph_upload_tests（原内嵌测试，`use super::*` 语义保持）
```

合计约 5400 行（比原文件多约 200 行，全是各文件重复的 `use` 头与 `impl Renderer` 前导）。**最大文件 `init.rs = 735`**，余量 65 行。

原行段映射（用于校验搬运完整性）：

| 原行段 | 去向 |
| --- | --- |
| `1-46` 头 use / const | `mod.rs`（常量）+ 各分片按需复制 use |
| `48-114` `max_axis_scale` + `MaterialKey` | `uniforms.rs` |
| `116-410` 六个 uniform 家族 | `uniforms.rs` |
| `411-597` Gpu* / MeshMorph / LayeredProvider | `mesh_cache.rs` |
| `598-845` `MeshCache` | `mesh_cache.rs` |
| `846-1042` 五个 stats 类型 | `stats.rs` |
| `1043-1301` `UploadStaging` | `staging.rs` |
| `1303-1465` `Renderer` 结构体 | `state.rs` |
| `1467-1646` plan 类型组 | `types.rs` |
| `1647-4308` `impl Renderer`（51 个方法，2662 行） | 按 §3 分入 `init/frame/shadow/instancing/upload/pass/state/buffers` |
| `4309-4376` `upload_mesh` | `upload.rs` |
| `4377-4718` `MaterialCache` | `material.rs` |
| `4719-5117` 纹理/目标 free fns | `buffers.rs` |
| `5118-5141` `impl RenderBackend` | `backend.rs` |
| `5142-5582` 内嵌测试 | `tests.rs` |

`impl Renderer` 2662 行的切分明细：

| 方法 | 原行 | 行数 | 目标文件 |
| --- | --- | --- | --- |
| `new` | 1648 | 610 | `init.rs` |
| `set_env_probe` / `clear_env_probe` / `env_probe_stats` | 2258/2334/2342 | 86 | `init.rs` |
| `resize` | 2460 | 32 | `buffers.rs` |
| `render` / `acquire_surface_frame` / `render_to_view` / `capture_frame` | 2493/2508/2531/2864 | 487 | `frame.rs` |
| `prepare_shadow` 起至 `draw_depth_prepass`、`prepare_local_shadow` 等 11 项 | 2984-3476 | 419 | `shadow.rs` |
| `prepare_instances` | 3518 | 643 | `instancing.rs` |
| `upload_morph_table` / `upload_instance_runs` / `upload_joint_runs` / `mesh_material_alpha` | 4162/4195/4250/4303 | 143 | `upload.rs` |
| `draw_group_range` / `prepare_materials` / `prepare_local_lights` | 2997/3485/3453 | 84 | `pass.rs` |
| 其余 25 个只读访问器 / setter（`postfx`/`ssao`/`size`/`gfx_info`/`stats`/`text*`/`ui*`/`mesh_cache`/`set_asset_library`/`mesh_aabb`/`environment*`/`aspect`/`caps_line`/`local_shadow_*`） | 2258-3509 | 约 110 | `state.rs` |

## 4. 分批执行（每批结束必须 `cargo check -p r3e-render` + `cargo clippy -p r3e-render --all-targets -- -D warnings` 双绿）

> `clippy -D warnings` 是硬约束：每批新增的 `use` 必须精确，不许留 unused import，也不许用 `#[allow(unused_imports)]` 就地压掉。

- **批 A · 叶子模块（6 文件）**：`buffers.rs` → `material.rs` → `uniforms.rs` → `mesh_cache.rs` → `stats.rs` → `staging.rs`。
  这批全是「自包含类型 + free fn + 只依赖 wgpu/r3e_core」的内容，无 `impl Renderer` 交叉，风险最低。
  注意 `MeshCache` 内用到 `upload_mesh` 吗——不；`material.rs` 依赖 `buffers.rs` 的 `create_white_texture` / `create_flat_normal_texture` / `create_black_cube_texture`，所以 `buffers.rs` 先落。
- **批 B · 中间类型（1 文件）**：`types.rs`。
  `EnvProbeGpu` 依赖 `envprobe::EnvProbeStats`，`InstancePlan`/`ShadowPlan`/`MorphUpload` 依赖 `stats.rs`、`uniforms.rs`，此时都已就位。
- **批 C · 状态（1 文件）**：`state.rs`——先建结构体与访问器，同时把 78 个字段改 `pub(crate)`。
  这一步 `renderer.rs` 里删除结构体定义、保留 `impl Renderer`，`mod.rs` 里 `use state::Renderer;`。
- **批 D · impl 七分片（7 文件）**：`init.rs` → `frame.rs` → `shadow.rs` → `pass.rs` → `instancing.rs` → `upload.rs` → `backend.rs`。
  每搬一个文件就删掉原文件里对应的 `impl` 方法，保证 `renderer.rs` 单向缩小。
- **批 E · 测试与收口**：内嵌测试搬到 `tests.rs`；`renderer.rs` 内容清零后删除，`mod.rs` 完成最终 re-export 表；`xtask/size_debt.json` 删除 `crates/r3e-render/src/renderer.rs` 条目。
- **批 F · 文档同频**：`docs/ARCHITECTURE.md` §3.2 行数表用 `git numstat` 实测值更新（`renderer.rs 5582` → `renderer/` 各文件实测），`docs/AI_ROADMAP_EXECUTION.md` 的 M0-S1 状态改「已完成」并记录实测。行数一律用实测，禁止沿用本方案的预估值写进文档。

## 5. 验收清单（全绿才算 S1 关闭）

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`（default 与 `--features rapier` 两轮）
3. `cargo test -p r3e-render` ≥ **307**（`check-tests` 门禁）
4. `cargo xtask lint-size` 通过，且 `size_debt.json` 已无 `renderer.rs` 条目
5. `cargo xtask check-tests` 通过
6. 像素回归：`R3E_VISUAL_STRICT=1 cargo test -p r3e-app --test visual_regression`（有 GPU 时）基线不变
7. 机械核查：`renderer/` 下最大单文件 ≤800；原 `renderer.rs` 全部 5582 行在原行段映射表上「无遗漏、无重复」
8. 引用核查：`git grep "renderer::"` 命中的 4 个测试文件与 `lib.rs` 15 项 re-export 全部编译通过

## 6. 风险与熔断

| 风险 | 处置 |
| --- | --- |
| `init.rs` 实测超 800 | 把 `set_env_probe` / `clear_env_probe` / `env_probe_stats`（86 行）切到 `env.rs`，`init.rs` 回落到 ~650 |
| `instancing.rs` 实测超 800（`prepare_instances` 本身 643 行） | 先试把 `use` 头压到最小；仍超则把该函数整体留在 `instancing.rs` 并**登记回 `size_debt.json`（base 写实测值）**，在方案里记一行「待函数级拆分」——**不允许为了压行数把 643 行函数切成子函数** |
| 某方法依赖私有类型导致必须改逻辑 | 立即停手：在原位置留 `// TODO(split) <原因>`，该段不搬，文件保留白名单条目 |
| 内嵌测试 `use super::*` 在 `renderer/tests.rs` 里拿不到私有项 | 在 `mod.rs` 加 `pub(crate) use` 暴露所需私有项，不改测试代码 |
| `pub(crate)` 字段泄漏到 crate 外 | 不可能：`pub(crate)` 仅限 `r3e-render` 内部；对外仍是 `Renderer` 无 public 字段，API 面无变化 |
| 拆分引入 unused import / 顺序问题导致 clippy 红 | 每批单独 `clippy -D warnings`，不攒到最后 |

## 7. 本次不做（out of scope）

- 不拆 `Renderer` 的字段为子结构体（`GpuCore` / `ShadowState` / `CullState`），也不改所有权与借用结构。
- 不压缩 `prepare_instances` / `render_to_view` / `Renderer::new` 的复杂度。
- 不动 `postfx.rs`(817) / `text.rs`(869) / `ui.rs`(1112) 等其它白名单条目——它们是后续 Sprint 的活。
- 不推送远端；提交仍按 code / 门禁配置 / 文档 三笔拆，本地 ahead。

## 8. 提交规划

1. `refactor(renderer): 拆 renderer.rs 为 renderer/ 模块目录（单文件 ≤800）` —— 纯搬迁 + 字段 `pub(crate)`，含测试迁移
2. `chore(gate): size_debt 移除 renderer.rs 条目` —— 若有回落项，同笔登记实测 base
3. `docs: ARCHITECTURE §3.2 行数同频 + AI_ROADMAP_EXECUTION M0-S1 收口`
## 9. 执行结果（2026-11-15 落地）

`renderer.rs`（5,582 行）已删除，内容全部迁到 `crates/r3e-render/src/renderer/` 下 **17 个模块**，无一行逻辑改动（字段仅改 `pub(crate)` 可见性）：

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `mod.rs` | 63 | 模块声明 + re-export 表（对外 15 项符号面不变，内部项走 `pub(crate) use`） |
| `types.rs` | 185 | `DrawGroup` / `InstancePlan` / `ShadowPlan` / `ShadowSubmit` / `MorphUpload` / `EnvProbeGpu` / `morph_upload` / `shadow_runs` |
| `uniforms.rs` | 374 | `MaterialKey` / `CameraUniform` / `ShadowUniform` / `ShadowPassUniform` / `LightsUniform` / `LocalLightRaw` / `LocalShadowUniform` / `max_axis_scale` 及 6 个布局常量 |
| `mesh_cache.rs` | 436 | `MeshCache` / `GpuMesh` / `GpuPart` / `MeshMorph` / `mesh_aabb`（`Arc<dyn MeshProvider>` 注入点） |
| `state.rs` | 306 | `Renderer` 字段与访问器（`postfx` / `ssao` / `size` / `gfx_info` / `gfx_api` / `caps_line` / `aspect` / `environment_builds` / `environment` / `stats` / `text`） |
| `stats.rs` | 204 | `RenderStats` / `CullStats` / `StagingStats` / `CapturedFrame` |
| `staging.rs` | 267 | `UploadStaging` / `StagingTarget` / `staging_enabled_from_env` |
| `instancing.rs` | 656 | `prepare_instances` 全段（视锥剔除 / 实例与关节槽位分配 / 溢出报错） |
| `shadow.rs` | 450 | `prepare_shadow` 级联 + `prep_local_shadow` 局部阴影 |
| `material.rs` | 350 | `MaterialCache`（材质 uniform / 贴图 view / 采样器 / bind group 1） |
| `buffers.rs` | 440 | `create_*` 系列（深度/MSAA/阴影目标、黑立方/白/平面法线纹理、`upload_texture`）+ `ShadowBindInputs` / `TextureColorSpace` |
| `pass.rs` | 94 | `prepare_local_lights` / `draw_group_range` / `prepare_materials` |
| `backend.rs` | 29 | `render` / `resize` 后端入口（透传给 `gfx::RenderBackend`） |
| `frame.rs` | 496 | `render_to_view` 主流程 + SSAO 参数打包与 composite 次序 |
| `init.rs` | 716 | `Renderer::new` 与全部管线初始化（env probe 三件套仍留在此文件：716 行未超 800，未触发第 6 节熔断） |
| `upload.rs` | 227 | `upload_mesh` / `upload_morph_table` / `upload_instance_runs` / `upload_joint_runs` / `mesh_material_alpha` |
| `tests.rs` | 450 | 原内嵌 441 行测试整体迁入（`morph_table_*` 等 15 条 `#[test]`） |

合计 5,743 行（+161 来自模块声明、`use` 头与 `pub(crate)` 可见性标注），最大单文件 `init.rs` 716 行，无文件触发熔断。

**验收实测（全部通过）**：

1. `cargo fmt --all --check` — exit 0
2. `cargo clippy --workspace --all-targets -- -D warnings`（default）— exit 0
3. `cargo clippy --workspace --all-targets --features rapier -- -D warnings` — exit 0
4. `cargo test --workspace` — exit 0，1,339 passed（`--lib` 850）
5. `cargo test --workspace --features rapier` — exit 0，1,343 passed（+4 条 rapier 专属）
6. `cargo xtask lint-size` — exit 0，26 项存量债务均未上涨，`renderer.rs` 条目已删
7. `cargo xtask check-tests` — exit 0，逐 crate 测试数一条未减（`r3e-render` 307）
8. `cargo xtask dep-check` — exit 0
9. 像素回归：`R3E_VISUAL_STRICT=1 R3E_VISUAL_REGRESSION=1 cargo test -p r3e-app --test visual_regression` — 2 passed（真机 GPU 8.10s），基线帧逐像素不变 —— **零行为变更成立**
10. 机械核查：`renderer/` 下最大 716 行 ≤800；原 5,582 行按第 2 节行段表全覆盖，无遗漏无重复

**遗留**：`prepare_instances`（643 行函数）、`render_to_view`（332 行）等长函数本次不做函数级拆分（见第 7 节）；`postfx.rs`(817) / `text.rs`(869) / `ui.rs`(1112) / `api.rs`(4,070) 仍是后续 Sprint 的账。
