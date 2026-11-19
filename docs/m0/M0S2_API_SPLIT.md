# M0-S2：`api.rs` 双份收口 + 拆分方案（4,070 / 3,552 → 每文件 ≤800）

> 目标：把"两份命令实现"变成"**一份命令表 + 两个薄适配**"，同时把两个单体文件拆到 `lint-size` 门槛内。
> **零行为变更**：rhai 函数名/参数/返回类型逐字不变，RPC 方法与 JSON 形状逐字不变，像素基线不变。
> 与 M0-S1（renderer 拆分）同路线，但 M0-S2 多一条腿：**拆分只是行数门禁，命令表才是这一周的价值点**（`AI_ROADMAP_EXECUTION.md` §M0-S2 原话）。

## 1. 实测基线（不是文档里的 3386 / 3320 / 24）

| 项 | 实测值 | 文档旧口径 | 差异原因 |
| --- | --- | --- | --- |
| `r3e-script/src/api.rs` | **4,070 行** | 3,386 | M0 之后新增 SSAO/混合空间/图层同步/拾取等绑定 |
| `r3e-editor-host/src/api.rs` | **3,552 行** | 3,320 | 同上（后处理/SSAO/动画层） |
| 脚本侧 `register_fn` 调用 | **364 次 / 353 个唯一函数名** | 364 | 11 处同名声明的重载 |
| RPC 侧 `EngineApi` 方法 | **39 个** | 24 | 文档口径过时；实测 39 个方法全部被 `dispatch` 调用 |
| RPC 协议分支 | **40 个分支**（1 `ping` + 39 命令） | — | `dispatch` 位于 `r3e-editor/src/lib.rs:1024-1785`（762 行） |
| 单测基线 | `r3e-script = 128`，`r3e-editor-host = 55`，workspace 合计 **1,343** | 128 / 55 | 一致 |
| `size_debt.json` | 26 项，含这两个文件（limit 800） | 27 | M0-S1 已删 `renderer.rs` |

`r3e-editor-host/src/api.rs` 内部分段（实测）：

| 行段 | 内容 |
| --- | --- |
| `1-48` | crate 头注释 + `use`（`r3e_core` / `r3e_editor` / `r3e_runtime`） |
| `49-68` | `pub struct LocalEngineApi`（持有 runtime 与解释器句柄） |
| `70-1326` | `impl EngineApi for LocalEngineApi`：**39 个方法、1,257 行** |
| `1327-1764` | 自由函数区：**23 个**（`postfx_value` / `ssao_value` / `anim_layer_value` / `apply_ui_patch` / `ui_element_value` / `fmt_ui_value` …）438 行 |
| `1765-3460` | `mod tests`：**51 个 `#[test]`**，1,696 行 |
| `3461-3552` | `mod scene_route_tests`：**4 个 `#[test]`**，92 行 |

`r3e-script/src/api.rs` 内部分段：`1-96` 头 + 三个公开类型；`97-269` 七个辅助函数；`270-3947` **`bind_all` 单体 3,678 行**（364 次 `register_fn`）；`3948-4070` UI/文本辅助 + `serde_json_lite`。

`bind_all` 实测分域（原行段 = 天然切缝，来自 41 条分区注释经内容核对后合并）：

| 原行段 | 行数 | 注册数 | 实际内容（抽样） |
| --- | --- | --- | --- |
| `270-541` | 272 | 13 | `log` / `create_node_json` / `remove_node(s)` / `exists` / `count` |
| `542-811` | 270 | 20 | `anims` / `play_anim` / `stop_anim` / `set_anim_speed` |
| `812-939` | 128 | 8 | `anim_clip` / `set_anim_clip` / `anim_fade` / `anim_layers` |
| `940-1076` | 137 | 1 | `clear_anim_layers`（含较长遍历闭包） |
| `1077-1533` | 457 | 7 | `anim_layer_sync` / `anim_layer_phase` / `orbit` / `fov` / `set_light*` |
| `1534-1711` | 178 | 14 | `set_env_intensity` / `set_env_map*` / `has_environment` |
| `1712-1954` | 243 | 29 | `clear_postfx` / `postfx_*` / `set_ssao_*` |
| `1955-2294` | 340 | 41 | `set_background` / `time` / `frame` / `dt` / `fps` / `rand` |
| `2295-2598` | 304 | 16 | `body_count` / `physics_*` |
| `2599-2801` | 203 | 17 | `text_set_*` |
| `2802-3037` | 236 | 21 | `ui_set_*` |
| `3038-3199` | 162 | 7 | `light_remove` / `light_clear` / `light_set_*` / `set_lights_enabled` |
| `3200-3648` | 449 | 49 | `set_occlusion_*` / `local_light_*` |
| `3649-3947` | 299 | 26 | `audio_spatial_*` |

364 次注册全部落在 `bind_all` 内；表中"注册数"是单行写法可被正则命中的 305 条，余下 59 条为多行写法（`register_fn(` 与函数名分两行），按段内顺序归属，执行期由 `temp/m0s2_diff.py` 逐条归位后再核。**这 14 段与 `ARCHITECTURE.md:162` 列的能力表（文本 / UI / 音频 / 材质 / 环境 / 遮蔽 / 动画层 / 拾取绑定）逐项对得上**，故以此作为文件边界，不再另造 8 域粗分类。

对外引用面（re-export 兜底范围，一个都不能丢）：

- `r3e_script::api::{bind_all, Clock, ScriptContext, UiWriteLog}` —— `lib.rs` 只 `pub mod api` + `pub use Clock`，其余被 `r3e-script/tests/*`（16 个文件）与 `r3e-runtime` 经 `r3e_script::api::ScriptContext` 使用。
- `r3e_editor_host::LocalEngineApi` —— 被 `host.rs` / `lib.rs` / `runtime/port.rs` 引用，全部经 crate 根，不看文件位置。

## 2. 五条硬约束

1. **路径不变**：`api.rs` → `api/mod.rs`，`mod api` 声明与 `lib.rs` 的 `pub use` 一行不改；子模块符号在 `mod.rs` 里 `pub use` 回原路径。
2. **零逻辑改动**：`bind_all` 的每个闭包体、`impl EngineApi` 的每个方法体**整体搬运**，不改分支、不改注释、不合并函数。唯一允许的机械改写是 §3 的三处下沉（每处 ≤3 行）。
3. **测试只搬不删**：`r3e-script` 128 / `r3e-editor-host` 55 一条不减；`mod tests` 搬成 `api/tests/mod.rs` 后保留 `use super::*`，靠 `mod.rs` 的 `pub(crate) use` 兜住（与 M0-S1 同构）。
4. **顺序不可调换**：批 0（三处下沉 + 测绘）→ 批 1（命令表 + 一致性测试）→ 批 2/3（拆 script / host）→ 批 4（门禁与文档）。**先立表后拆文件**，否则拆完就再也对不齐了。
5. **命令表只做声明与校验，不做代码生成**：v0 的 `CommandSpec` 不驱动 `register_fn` 生成，只被一致性测试与后续 `capabilities.json` 消费。生成是 M1 的事。

## 3. 双份收口：三处逐字重复（批 0 一次性解决）

| # | 重复点 | 位置 | 重复形态 | 下沉去向 |
| --- | --- | --- | --- | --- |
| 1 | `parse_ui_value` | `script:3948` / `host:1565` | **逐字相同 13 行** | `r3e_core::ui::parse_ui_value` |
| 2 | SSAO 采样数夹取 | `script:102` `ssao_samples_of(i64)` / `host:603` `i32→u8` | 同语义两套整数类型 | `r3e_core::ssao::clamp_samples(i64) -> u8` |
| 3 | UI 数值 → JSON 字符串 | `host:1580` `fmt_ui_value` / `script` 无对应 | 能力不对称（host 有、script 无） | `r3e_core::ui::format_ui_value` |

做法（最小改动原则）：**在 core 里新增函数，两侧原函数改成薄包装转发**，调用点一行不动。副作用面 = 3 个函数体，每处 ≤3 行，由现有 SSAO/UI 测试 + core 新增单测共同守护。

不做的收口：`ui_element`（script，37 行）与 `apply_ui_patch` + `ui_element_value`（host，45 + 18 行）语义相近但字段处理不同（script 是 patch 进 `UiElement`，host 是 patch 进 `Scene` 并回显快照），合并等于改逻辑 → 列入 §10 不做。

**13 组"同能力双实现"是这一周测绘的主产出**（`S2-01` 三张清单）：

| 能力组 | script 侧（rhai 函数） | host 侧（RPC） |
| --- | --- | --- |
| 变换 / 材质 | `set_transform` / `set_material` / … | `scene.settransform` / `scene.setmaterial` |
| 环境与探针 | `set_env_map` / `set_env_map_faces` / `set_env_intensity` / `clear_environment` | `scene.setenvironment` / `scene.setenvironmentmap` |
| 后处理 | `postfx_*` / `clear_postfx` | `scene.setpostfx` |
| SSAO | `set_ssao_*` / `ssao_*` | `scene.setssao` |
| 动画层 | `set_anim_layer_*` / `anim_layer_*` / `anim_clip_*` | `scene.animlayers` / `scene.animlayerset` |
| UI | `ui_set_*` / `ui_remove` / `ui_state` | `ui.set` / `ui.remove` / `ui.state` / `ui.list` |
| 文本 | `text_set_*` | 无（仅 script） |
| 灯光 | `light_*` / `set_light` / `set_lights_enabled` | `light.set` / `light.list` / `light.remove` / `light.clear` / `light.setenabled` |
| 局部光与遮挡 | `local_light_*` / `set_occlusion_*` | 无（仅 script） |
| 音频空间化 | `audio_spatial_*`（26） | `audio.spatial.*`（6） |
| 场景流 | `scene_switch` / `scene_save_archive` / … | `scene.stack` / `scene.switch` / `scene.back` / `scene.archive.*` |
| 脚本控制 | `log` 等 | `script.eval` / `script.reload` |
| 引擎信息 | 无 | `engine.info` / `engine.stats` / `capture.frame` |

完整三张清单（仅 script / 仅 host / 同名异签名）由批 0 的 `temp/m0s2_diff.py` 脚本生成，人工复核后并入本文 §9。

## 4. `CommandSpec` v0（本周真正的价值点）

**真源判定（`S2-02`）**：命令的单一真源是**表**，不是任何一侧的实现。

**存放位置**：`crates/r3e-core/src/cmd_spec/`。理由是依赖规则表（`xtask/dep_rules.json`）里 `r3e-script.allow = ["r3e-core"]`、`r3e-editor-host.allow = ["r3e-core", "r3e-editor", "r3e-runtime"]` —— 放 core 两侧都合法且**零新增依赖边**；放新 crate 会让两侧各多一条白名单外依赖，直接制造 2 条新违规（`dep-check` 现有 3 条已登记债务，不许再涨）。core 是纯数据层，命令元数据符合其定位。

**与路线 §2.1 / S5-02 的衔接（防误读）**：`CommandSpec` 在 `r3e-core` 是命令的**声明真源**（全仓唯一手写处）；`r3e-protocol` 只承载其**序列化格式与传输语义**，不重复声明命令元数据；S5-02 的 `gen-capabilities` 从 core 导出 JSON。即：core 定"有哪些命令"，protocol 定"命令长什么样、怎么传"——两者不是双份实现，也不构成 §1 要去掉的那种重复。

```rust
// crates/r3e-core/src/cmd_spec/mod.rs
pub struct CommandSpec {
    pub name: &'static str,          // 脚本侧函数名；仅 RPC 有的用点分名
    pub rpc: Option<&'static str>,   // 对应 dispatch 归一后的分支键（无则 None）
    pub domain: Domain,              // 与 §1 的 14 段一一对应
    pub params: &'static [&'static str], // 多签名即多元素（覆盖 11 处重载）
    pub returns: &'static str,
    pub reversible: bool,            // 写操作可否撤销（M0-S4 事务要用）
    pub cost: Cost,                  // Cheap / Linear / Heavy
    pub since: &'static str,         // v0 一律 "M0-backfill"
}
pub static ALL: &[CommandSpec] = &[/* 各域数组拼接 */];
```

**规模与文件切分**（392 条 ≈ 353 + 39，单文件必须 ≤800，否则新增超标直接红）：

| 文件 | 内容 | 估算行 |
| --- | --- | --- |
| `cmd_spec/mod.rs` | 类型 + `Domain`/`Cost` 枚举 + `ALL` 拼接 + 查询函数 | 130 |
| `cmd_spec/scene.rs` | 变换/材质/环境/场景流/生命周期/查询 | 320 |
| `cmd_spec/anim.rs` | 播放/片段/图层/同步/混合空间/IK | 260 |
| `cmd_spec/render.rs` | 后处理/SSAO/阴影/遮挡/局部光/背景 | 240 |
| `cmd_spec/ui.rs` | UI + 文本 | 220 |
| `cmd_spec/audio.rs` | 音频与空间化 | 160 |
| `cmd_spec/sim.rs` | 物理/输入/拾取/引擎信息 | 260 |

填表来源：**脚本生成，人工校对**。批 1 用 `temp/gen_cmd_spec.py` 扫 `register_fn` 与 `dispatch` 分支产出草稿，人只做 `reversible` / `cost` 两列的裁决（默认 `false` / `Cheap` 的必须有明确依据才改）。

**三个一致性测试（`S2-06`，缺一条即红）**：

| 测试 | 做法 | 放在 |
| --- | --- | --- |
| 表 ↔ 脚本注册 | `engine.gen_fn_signatures(false)` 取实际注册名集合，与 `ALL` 中 `rpc.is_none() \| name` 集合互查；只比函数名，不比参数串（rhai 签名格式随版本变） | `r3e-script/tests/cmd_spec_contract.rs` |
| 表 ↔ RPC 分支 | 在 `r3e-editor` 增 `pub fn known_methods() -> &'static [&'static str]`（由 `dispatch` 分支键手写维护，紧邻 dispatch），与 `ALL` 的 `rpc` 集合互查（排除 `ping`） | `r3e-editor/tests/` 既有套件内新增 1 测 |
| 表 ↔ host 实现 | `KNOWN` 集合 ⊇ 39 个 `EngineApi` 方法在 `dispatch` 中的映射；反向确认没有 `rpc: Some(..)` 指向不存在的方法 | `r3e-editor-host/tests/cmd_spec_contract.rs` |

## 5. 目标文件树

### 5.1 `crates/r3e-script/src/api/`（20 文件）

| 文件 | 行数 | 来源 |
| --- | --- | --- |
| `mod.rs` | 70 | `bind_all` 编排（`set_max_operations` + 14 次 `domain::register`）+ re-export |
| `context.rs` | 105 | `Clock` / `ScriptContext` / `UiWriteLog` + impl（原 `32-96`） |
| `support.rs` | 95 | `f32_of` / `ssao_samples_of` / `parse_bone_mask` / `edit` / `xorshift` / `queue_scene_cmd` / `non_empty_path` |
| `result.rs` | 120 | `vec3_array` / `empty_query_map` / `hit_map` / `ray_hit_map` / `pick_map` / `shape_cast_map` / `serde_json_lite` |
| `ui_value.rs` | 60 | `parse_ui_value`（→ 转发 core）/ `with_ui` / `ui_element` / `submit_ui` / `with_text` |
| `output.rs` | 285 | `270-541` |
| `anim_play.rs` | 285 | `542-811` |
| `anim_clip.rs` | 145 | `812-939` |
| `anim_layer.rs` | 155 | `940-1076` |
| `anim_sync.rs` | 475 | `1077-1533` |
| `env.rs` | 195 | `1534-1711` |
| `postfx.rs` | 260 | `1712-1954` |
| `frame.rs` | 358 | `1955-2294` |
| `physics.rs` | 322 | `2295-2598` |
| `text.rs` | 220 | `2599-2801` |
| `ui.rs` | 255 | `2802-3037` |
| `light.rs` | 180 | `3038-3199` |
| `occlusion.rs` | 468 | `3200-3648` |
| `audio.rs` | 318 | `3649-3947` |

合计 ≈ 4,370（原 4,070 + 20 个文件的 `use` 头与 `register` 外壳）。**最大 `anim_sync.rs = 475`**，余量 325。

每个域文件统一形态（这是拆分可行性的关键，已核对 `bind_all` 每一段都自带 `let c = ctx.clone();`，不依赖跨段共享状态）：

```rust
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    let c = ctx.clone();                       // 原段首行原样搬过来
    engine.register_fn("xxx", move |...| { ... });   // 闭包体逐字不变
}
```

`mod.rs` 只做编排：
```rust
pub fn bind_all(engine: &mut Engine, ctx: &ScriptContext) {
    engine.set_max_operations(50_000_000);
    output::register(engine, ctx); anim_play::register(engine, ctx); /* … 14 行 */
}
```

### 5.2 `crates/r3e-editor-host/src/api/`（12 文件）

| 文件 | 行数 | 内容 |
| --- | --- | --- |
| `mod.rs` | 300 | 头注释 + `pub struct LocalEngineApi` + `impl EngineApi` 39 个薄壳（每壳 1~3 行转发）+ `pub(crate) use` 兜底 |
| `support.rs` | 250 | 原 `1327-1764` 的自由函数：`opt_*` 解析、`postfx_value` / `ssao_value` / `anim_layer_value` / `ui_element_value` / `fmt_ui_value`（转发 core） |
| `scene.rs` | 360 | `scene_list/get/set_transform/spawn/remove/save/set_material` 的 handler |
| `env.rs` | 180 | `scene_set_environment` / `set_environment_map` / `scene_shadow` |
| `anim.rs` | 165 | `scene_anim_layers` / `scene_anim_layer_set`（最长方法 115 行） |
| `ui.rs` | 150 | `ui_list/set/remove/state` + `apply_ui_patch` |
| `postfx.rs` | 190 | `scene_set_postfx` / `scene_set_ssao`（SSAO 夹取转发 core） |
| `light.rs` | 130 | `light_*` 五方法 |
| `audio.rs` | 130 | `audio_spatial_*` 六方法 |
| `flow.rs` | 125 | `scene_stack/switch/back/archive_save/archive_load` |
| `script.rs` | 165 | `script_eval` / `script_reload` / `engine_info` / `engine_stats`（178 行） / `capture_frame` |
| `tests/mod.rs` | 1,700 | 原 `mod tests` 51 测（`use super::*` 语义保持） |
| `route_tests.rs` | 92 | 原 `mod scene_route_tests` 4 测 |

**host 侧拆分选型：自由函数 handler + `impl` 薄壳转发**。

```rust
// api/scene.rs
pub(super) fn scene_list(api: &ApiParts<'_>, filter: Option<&str>, limit: Option<usize>) -> Result<Vec<Value>, RpcFailure> { /* 原方法体 */ }

// api/mod.rs
fn scene_list(&mut self, filter: Option<&str>, limit: Option<usize>) -> Result<Vec<Value>, RpcFailure> {
    scene::scene_list(&self.parts(), filter, limit)
}
```

代价：约 39 × 3 = 120 行转发样板，加上 `ApiParts` 借用拆分。收益：每个文件是合法的独立编译单元。
**否决 `include!("scene_api.rs")`**：被 include 的片段在 rust-analyzer / IDE 里报错（不是合法模块），且 `lint-size` 数的是文件行数，片段文件会成为"隐身大户"，与门禁初衷相反。

`ApiParts` 只做字段借用拆分（`&mut Runtime` / `&mut Interpreter` / 计数器），**不封装行为**。若某方法拆不出干净借用（如同时可变借用 runtime 与解释器），按 §9 熔断：整方法留在 `mod.rs`，标 `// TODO(split)`。

## 6. 分批执行（8 个工作包，1 周）

| 批次 | 作业 | 产出 | 耗时 |
| --- | --- | --- | --- |
| **批 0** | 测绘脚本 + S2-01 三张清单 + 三处下沉 | `temp/m0s2_diff.py`、`temp/api_map.md`、core 新增 2 函数（`parse_ui_value` / `format_ui_value` / `clamp_samples`） | 0.5 天 |
| **批 1a** | `CommandSpec` 类型 + 7 个域文件骨架 + 填表 392 条 | `core/src/cmd_spec/` | 0.5 天 |
| **批 1b** | 三个一致性测试，红 → 绿 | 3 个测试文件/套件 | 0.25 天 |
| **批 2a** | script：`api/mod.rs` + `context/support/result/ui_value` 建好，`bind_all` 先原样搬进 `mod.rs` | 编译绿、128 测不变 | 0.25 天 |
| **批 2b** | script：14 个域文件按 §5.1 切出，`bind_all` 缩到 20 行 | 最大 475 行 | 0.75 天 |
| **批 3a** | host：`support.rs` + 39 个 handler 建好，`impl` 改薄壳 | 编译绿、55 测不变 | 0.5 天 |
| **批 3b** | host：`tests/mod.rs` 与 `route_tests.rs` 搬迁 + 借用冲突收尾 | 同上 | 0.25 天 |
| **批 4** | 全门禁 + 像素回归 + 文档同频 + `size_debt` / `tests_baseline` 更新 | 见 §7 | 0.5 天 |

每批结束必须双绿（`cargo test -p <crate>` + `cargo xtask lint-size`），不允许连做两批后再修。

## 7. 验收清单（全部可自动判定）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features                      # ≥ 1,343，一条不减
cargo test -p r3e-script                                   # ≥ 128
cargo test -p r3e-editor-host                              # ≥ 55
cargo test -p r3e-script --test cmd_spec_contract          # 新增 3 类一致性测试全绿
cargo xtask lint-size                                      # 两文件从白名单消失，26 → 24 项，其余未上涨
cargo xtask check-tests
cargo xtask dep-check                                      # 违规数仍为 3（不许因新增 crate 变 4）
R3E_VISUAL_STRICT=1 R3E_VISUAL_REGRESSION=1 cargo test -p r3e-app --test visual_regression   # 基线不变
```

补充判据：

| 判据 | 阈值 |
| --- | --- |
| `api.rs` 目录内最大文件 | ≤ 800（aim ≤ 500） |
| `r3e-script` 中 `bind_all` 行数 | ≤ 30 |
| `impl EngineApi` 中单方法行数 | ≤ 5（转发除外：`script_reload`/`engine_stats` 的体在 handler 里） |
| 脚本示例可编译 | `scripts/*.rhai` 的 `scripts_compile` 套件全绿（函数名一个不改，应当零改动即过） |

## 8. 风险与熔断

| 风险 | 概率 | 处置 |
| --- | --- | --- |
| host 借用检查：多方法需同时可变借用 runtime 与解释器 | 中 | `ApiParts` 拆分；拆不出则整方法留 `mod.rs` 标 `// TODO(split)`，记 Backlog（与 M0-S1 熔断同规） |
| `mod tests` 的 `use super::*` 在搬迁后失效 | 中 | `mod.rs` 里 `pub(crate) use` 各域符号；先搬测试再拆实现，任何一批测试红立即回退该批 |
| 11 处重载导致"名字集合互查"假红 | 低 | `CommandSpec.params` 支持多签名；一致性测试按 `name` 去重比对 |
| `gen_fn_signatures` 格式随 rhai 版本变化 | 低 | 只比函数名，不比签名串；测试内注释写明版本约束 `rhai 1.27` |
| 命令表填错导致测试红但不知错在哪 | 中 | 一致性测试输出差集（缺哪条、多哪条），不用人工数 392 行 |
| 拆分后 `size_debt` 忘记删条目 | 低 | 批 4 固定动作；`lint-size` 对"白名单内未超标文件"会报脏数据 |

熔断规则（沿用 `AI_ROADMAP_EXECUTION.md` §2.4）：单批超 1 天未完成 → 拆小或降级；同批连续失败 2 次 → 换方案不许微调硬试；**测试数下降 → 立即停手先修归属**；像素基线变化 → 视为失败回退该批。

## 9. 不做（明确排除）

- 不改任何 rhai 函数名、参数顺序、返回类型，不改任何 RPC 方法名与 JSON 字段。
- 不合并 `ui_element` / `apply_ui_patch` 两套 UI 解析（字段处理不同，合并=改逻辑）。
- 不用 `CommandSpec` 生成 `register_fn` 或 `dispatch` 分支（M1 再谈）。
- 不把 `r3e-editor-host` 对 `r3e-render`/`r3e-script` 的既有依赖债务在本周清理（`dep-check` 3 条违规留给 S5）。
- 不新增 crate，不动 `workspace.members`（防御"再建一个 r3e-protocol"的冲动，那是 M0-5）。
- 不在本 Sprint 修 `script_reload` 458 行的内部结构（只搬不改）。

## 10. 提交规划（实际 4 笔）
| # | 提交 | 内容 |
| --- | --- | --- |
| 1 | `refactor(script): api.rs 按域拆分为 16 个域模块`（`1639d5f`） | script 侧拆分 + 三处重复实现下沉 `r3e-core`（原计划把 core 下沉单列一笔，因 script 侧同批引用 `r3e_core::ui_text` 才可编译，故合并） |
| 2 | `refactor(editor-host): api.rs 按域拆分为 13 个域模块`（`d6c7a59`） | host 侧拆分 |
| 3 | `feat(core): 引入 CommandSpec 命令表 + 三处契约测试`（`a342e64`） | S2-01…S2-03、S2-06；`size_debt` 两条已删条目随第 4 笔入库 |
| 4 | `docs: M0-S2 拆分方案与看板同频` | `ARCHITECTURE.md` §3.2 表、`AI_ROADMAP.md:43/139`、`AI_ROADMAP_EXECUTION.md` §6 看板（表头 + 新增 2026-11-15 行 + S5-04 由 4 改 3）、`xtask/tests_baseline.json`、`xtask/size_debt.json`，**逐项见 §10.1** |

### 10.1 基线数字刷新清单（全部并入提交 5，S2 执行期不动）

S2 执行期间文档与实际必然短暂不符，故**不在每批顺手改数字**，统一在提交 5 一次性刷新；刷新时一律以当时实测值为准，不许照抄本表括号内的预估值。

| # | 位置 | 现状（2026-11-14 口径） | 刷新为 |
| --- | --- | --- | --- |
| 1 | `ARCHITECTURE.md:162` `r3e-script/src/api.rs` 行数 | 3386 | `git numstat` 实测（预估 ~4070，拆分后该行消失改为 `api/` 目录） |
| 2 | `ARCHITECTURE.md:164` `r3e-editor-host/src/api.rs` 行数 | 3320 | 同上（预估 ~3552，拆后改 `api/` 目录） |
| 3 | `AI_ROADMAP.md:43` 最大文件行数 / `>800` 数 | 旧口径 | 拆完后由 `cargo xtask lint-size` 现取（预估 4070 → 新最大值，26 → 24 项） |
| 4 | `AI_ROADMAP.md:139` 同组数字 | 旧口径 | 同上 |
| 5 | `README.md` 目录结构 | 两个 `api.rs` | 改为 `api/`（script 20 模块 / host 12 模块） |
| 6 | `xtask/size_debt.json` | 26 项 | 24 项（提交 3、4 各删 1 条，批 4 校验） |
| 7 | `xtask/tests_baseline.json` | 1343 | 实测（只增不减；新增 cmd_spec 一致性测试后应上涨） |
| 8 | `AI_ROADMAP_EXECUTION.md` §6 看板表头「当前 (2026-11-14)」列 | 4487 / 25 / 1408 / 4 | 改为 2026-11-15 实测；**注意口径**：测试数此处 1408（`#[test]` 计数，`verify` 口径）与 `check-tests` 口径 1343 不是同一统计，刷新前先核对两者差异来源，不许直接把 1343 填进该列 |
| 9 | `AI_ROADMAP_EXECUTION.md` §6 记录表 | 仅 1 行（2026-11-14） | **新增一行**，按该表列序（周 / 最大行数 / >800 数 / 测试数 / 违规数 / verify(s) / 回放）填：`2026-11-15`、实测最大行数、实测 >800 数、verify 口径测试数、`3`、`—`、`—`（verify 与回放仍未实现，保持 `—`） |
| 10 | `AI_ROADMAP_EXECUTION.md:365` S5-04「修 4 条违规」 | 4 | 3（`dep-check` 现测 3 条已登记债务，S5 预算按 3 条算） |


## 11. 执行结果（2026-11-15 实测）

### 11.1 拆分规模
| 目标 | 拆前 | 拆后 | 最大文件 |
| --- | --- | --- | --- |
| `r3e-script/src/api.rs` | 4,070 行单文件 | `api/` 17 文件 | `mod.rs` 428 行 |
| `r3e-editor-host/src/api.rs` | 3,552 行单文件 | `api/` 13 文件 | `support.rs` 433 行 |
| `r3e-core/src/cmd_spec/`（新增） | — | 12 文件 369 条 `CommandSpec` | `scene.rs` 601 行 |

白名单外无新 `>800` 文件；`lint-size` 扫描 225 个 `.rs`，白名单 24 项、零超标（原 26 项，两条 `api.rs` 删除）。

### 11.2 测试
| 项 | 数值 |
| --- | --- |
| `#[test]` 总数 | 1,343 → **1,364**（+21） |
| 其中 `cmd_spec` 内联单测 | +6 |
| 其中 core 下沉模块单测 | +10（`ui_text` 3、`ssao` 新增 1；`ssao.rs` 原有 13 测保留） |
| 其中三处契约测试 | +11（script 4 / editor 4 / editor-host 3） |
| `cargo test --workspace` | 98 个 test target / **1,360 passed / 0 failed** |

### 11.3 门禁
- `cargo xtask lint-size`：rc 0，白名单 24 项未涨
- `cargo xtask dep-check`：仍 **3 条**已登记债务，未新增（`r3e-editor` 加的 `r3e-core` 是 dev-dependency，且其 `allow` 本就含 `r3e-core`）
- `cargo clippy --workspace --all-targets -- -D warnings`：rc 0

### 11.4 与计划的偏差
1. **提交数 5 → 4**：core 下沉并入 script 提交（编译顺序依赖）。
2. **模块数估计偏差**：计划写「script 20 模块 / host 12 模块」，实测 **17 / 13**（按原注释段自然切）。
3. **`editor` 声明面方案放弃**：原想在 `r3e-editor/src/lib.rs` 加 `known_methods` 数组，会把 `lib.rs` 从 2,597 行推到 2,649 行、触发 `lint-size` 债务上涨，改为契约测试直接 `include_str!` 扫 `dispatch` 的 `match` 分支键，文件零改动。
4. **`README.md` 目录结构无需改**：其代码树只到 crate 层，未列 `api.rs`（§10.1 第 5 项按此关闭）。
5. **1408 / 1343 口径并轨**：实测两口径同为 **1,364**，看板该列直接填 1,364 并注明口径。
