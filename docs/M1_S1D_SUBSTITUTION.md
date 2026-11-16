# M1-S1-D 替代清单（插件去 core/physics 依赖的前置）

- 生成时刻：2026-11-16（M1-S1 收尾）
- 对应卡：`M1_PLAN.md` §3 M1-S1-D
- 结论一句话：**四条依赖债务（`dep_debt.json` limit=4）里的 `plugin → core` / `plugin → physics` 两条，本轮不还**。理由是 R2 的"替代清单先行"约束尚未满足——插件有 4 项离线能力只能靠直连 `r3e-core` / `r3e-physics` 实现，协议面还没有等价承接方。削掉依赖 = 插件功能回退，这比"债务多挂一个 Sprint"严重得多。

## 1. 为什么不能直接削依赖

`crates/r3e-plugin/src/tools.rs`（1,324 行）里对 `r3e-core` / `r3e-physics` 的引用集中在**计算型工具**上，不是简单的 DTO 转换：

| 引用点 | 用到的东西 | 能否用 protocol 现有能力替代 |
|---|---|---|
| `tools.rs:11` | `r3e_core::procedural::builtin_mesh` | 否：要在本地生成网格才能算面数 |
| `tools.rs:12` | `Scene` / `SceneFile` / `MeshData` / `NodeId` / `Vec3` / `air` / `BuiltinMesh` / `Camera` / `Light` | 部分：场景数据可走 RPC JSON，但 `air::validate` 必须在有网格的地方跑 |
| `tools.rs:13` | `BodyDesc` / `Physics` / `PhysicsConfig` / `PhysicsKind` | 否：物理仿真是本地计算循环 |

而 `r3e-protocol` 是**纯数据层**（`serde` + 方法表 + dispatch，零引擎依赖）。把上面这些搬进 protocol 会直接破坏 M0-S5 立下的依赖方向，等于把协议层变成第二个引擎。

## 2. 四项离线能力的替代清单

| # | 工具 | 入口 | 现在怎么实现 | 替代承接方 | 承接方状态 | 依赖可否解除 |
|---|---|---|---|---|---|---|
| 1 | `scene_inspect` | `tools.rs:168` | 经 `HostBridge` 拿场景 + 本地遍历节点，输出扁平快照（`max_nodes` 截断） | 宿主已有 `scene.get` / `scene.list` / `engine.info`；S2-B 再补 `query.find` / `query.describe` 收敛投影 | **`scene.*` 三条已可用**，`query.*` 未落地 | 可（需把本地遍历改成 RPC 回包直传） |
| 2 | `mesh_stats` | `tools.rs:229` | `r3e_core::procedural::cube/sphere/plane/cylinder` 现造网格 → 数顶点/三角面 → `air::validate` | 无宿主 RPC；只能由**引擎进程侧**新增能力（如 `mesh.stats`）或 `r3e-cli` 直连 core | 未落地 | 否 |
| 3 | `scene_validate` | `tools.rs:273` | 逐节点取网格 → `air::validate` → `crate::locate::skipped_issue_json` 定位；校验不了的进 `unvalidated` | M1-S4 内容验证器（五类检查 + 失败点位坐标）落地后由引擎侧提供；替代形态是 `verify.*` RPC | 未落地（M1-S4） | 否 |
| 4 | `physics_simulate` | `tools.rs:359` | `Physics::new(PhysicsConfig)` 本地步进，`:D` 的 `rapier` feature 把物理后端编进插件二进制 | 引擎进程侧物理 RPC（宿主已含 `r3e-physics`）或 `r3e-cli --features rapier` | 未落地 | 否（且需同时处理 `features.rapier` 的间接依赖） |

**小计**：4 项里只有 1 项（`scene_inspect`）具备"低风险改造"条件；其余 3 项都要先在宿主/引擎侧开出计算类 RPC，否则插件无法在只依赖 protocol 的前提下保持行为不变。

## 3. 依赖削减的前置条件（判定为"未满足"）

| 前置 | 归属卡 | 状态 | 缺了会怎样 |
|---|---|---|---|
| `query.find` / `query.describe` / `query.stats` 三个 RPC 可用 | M1-S2-B | 未落地 | `scene_inspect` 换成 RPC 后投影字段对不上（`tags` / `interact_radius` 还没进 `node_desc`，见 R3） |
| 计算类 RPC（`mesh.stats` / `verify.*` / `physics.simulate`）中至少 `verify.*` 落地 | M1-S4 | 未落地 | `mesh_stats` / `scene_validate` 无处安放 |
| `r3e-cli` 最小版（只依赖 protocol，能跑 `query` / `verify` 子命令） | M1-S5-A | 未落地 | 没有第二条独立通路可对照，削依赖后无法验证"能力没丢" |
| ai_ide 宿主认 `scene.write` / `doc.transaction` 两个能力名 | 跨仓（`D:\ai_ide`） | 未确认 | 单方面在 `spec.rs::CAPABILITIES` 里加未知能力名，有被宿主**拒装**的风险 |

## 4. 本轮实际交付

| 项 | 处置 | 理由 |
|---|---|---|
| `dep_debt.json` 的 `plugin → core` / `plugin → physics` 两条 | **保留**，`plan` 字段改写为真实前置条件（见 §3），不再写"M1-S1 偿还" | 原 plan 承诺的前提不存在，留着假承诺比留债更糟 |
| `spec.rs::CAPABILITIES` | **不动**（仍 `["fs.read"]`） | 写侧工具的授权模型是 `risk: "write"` 触发人工确认，不靠文件系统能力扩权；`scene.write` / `doc.transaction` 属宿主协商项，需 ai_ide 侧先认 |
| `tools.rs` 四个入口 | 各加一行指针注释，指向本文件 | 让"我为什么还连着 core"在源码里可见，不必翻历史 commit |
| `Cargo.toml` | 不动 | 削依赖是结论，不是起点 |

## 5. 什么时候可以回来还这两条

按 `M1_PLAN.md` §9 的串行顺序，最早的时间点是 **S2-B + S5-A 双双落地之后**：

1. 先做 `scene_inspect` 这一项（唯一低风险项）——改完 `r3e-plugin` 仍需 `r3e-core` 的 `Scene` 类型？不需要：`scene.get` 的 JSON 可直接投影，此时先削掉 core 依赖里"场景类型"那一半；
2. `mesh_stats` / `scene_validate` 等 `verify.*` RPC 就位后，改为调用宿主计算，此时 `r3e-core::procedural` / `air` 的引用归零；
3. `physics_simulate` 最后走：把 `features.rapier` 从插件挪到引擎/CLI，插件只发 RPC；
4. 全部做完再删 `dep_debt.json` 两条，`dep-check --strict` 的违规数从 4 降到 2（另两条是 `r3e-script → physics/audio`，属 §13.6 端口化，与本卡无关）。

> 判据：删掉 `Cargo.toml` 里 `r3e-core` / `r3e-physics` 两行后 `cargo build -p r3e-plugin --all-features` 与
> `cargo test -p r3e-plugin` 全绿，且 `smoke` 自检四项能力输出与改造前逐字节一致——这才是"能力没丢"的证明。
