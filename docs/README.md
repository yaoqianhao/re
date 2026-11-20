# docs/ 目录规范与索引

> 本文件是 `docs/` 的唯一入口。新增、移动、删除任何文档前先读这里，改完同步更新这里的索引与 §二 规则。

---

## 一、当前布局

```
docs/
├─ README.md                     # 本文件：目录规范 + 索引
├─ ARCHITECTURE.md               # 活文档：架构总览、crate 边界、数据流、门禁
├─ ASSETS.md                     # 活文档：FBX/glTF 资产加载
├─ PHYSICS.md                    # 活文档：物理子系统
├─ AI_ROADMAP.md                 # 活文档：路线图、状态表、看板
├─ AI_WORKFLOW.md                # 活文档：M0 之后的标准工作流
├─ CI_GPU_RUNNER.md              # 活文档：自建 GPU runner 配置
├─ plugin-packaging.md           # 活文档：插件打包与拒装规则
├─ capabilities.json             # 机读产物：`cargo xtask gen-capabilities` 生成，路径锁定
├─ m0/                           # M0 阶段私有文档（S0–S5）
├─ m1/                           # M1 阶段私有文档
│   └─ norms/                    # M1-S5 三范式（norm1–3）+ 夹具
└─ m2/                           # M2 阶段文档：索引 / 计划 / 决策 / 基线（入口 m2/README.md）
```

| 路径 | 归属 | 说明 |
|---|---|---|
| `docs/ARCHITECTURE.md` | 活文档 | 架构总览；§3.3 定义五道门，读架构先看这里 |
| `docs/ASSETS.md` | 活文档 | 资产加载与已知限制 |
| `docs/PHYSICS.md` | 活文档 | 物理三层开关、场景字段、脚本 API |
| `docs/AI_ROADMAP.md` | 活文档 | 阶段路线图、§0.2 状态表、§八 看板、附录 B 门禁清单 |
| `docs/AI_WORKFLOW.md` | 活文档 | 开工基线 → 改码闭环 → 判定树 → 记账 → 文档同频 |
| `docs/CI_GPU_RUNNER.md` | 活文档 | `visual-gpu` 自建 runner 配置方法 |
| `docs/plugin-packaging.md` | 活文档 | 插件打包流程与能力白名单拒装规则 |
| `docs/capabilities.json` | 机读产物 | 三源交叉生成，入版本库；**路径被代码硬编码，不要移动** |
| `docs/m0/M0S1_RENDERER_SPLIT.md` | M0 归档 | S1 渲染层拆分方案 |
| `docs/m0/M0S2_API_SPLIT.md` | M0 归档 | S2 脚本 API 拆分（命令表/签名口径） |
| `docs/m0/M0S3_DOC_LAYER.md` | M0 归档 | S3 文档层（规则来源：`xtask` doc-gate） |
| `docs/m0/M0S4_TRANSACTION.md` | M0 归档 | S4 事务层 |
| `docs/m0/M0S5_PROTOCOL.md` | M0 归档 | S5 协议层与 Gate 0 收口（§3 counts / §13 偏差） |
| `docs/m1/M1_PLAN.md` | M1 归档 | M1 总计划；**§5 写点台账被 doc-gate 规则 7 机读** |
| `docs/m1/M1_S1D_SUBSTITUTION.md` | M1 归档 | S1-D 能力名替换裁决 |
| `docs/m1/M1_S4_PLAN.md` | M1 归档 | S4 计划（§4 S4-E 三条铁律） |
| `docs/m1/M1_S4F_PLAN.md` | M1 归档 | S4-F 收尾计划（D2=P1 等决策） |
| `docs/m1/M1_S5_PLAN.md` | M1 归档 | S5 计划；§3.3 退出码/错误码契约（E6） |
| `docs/m1/norms/norm1_build.md` | M1 归档 | 范式①：构建 |
| `docs/m1/norms/norm2_verify.md` | M1 归档 | 范式②：验证 |
| `docs/m1/norms/norm3_rollback_retry.md` | M1 归档 | 范式③：回滚与重试 |
| `docs/m1/norms/scene_norms.json` | M1 夹具 | 三范式验证场景 |
| `docs/m1/norms/silent.rhai` | M1 夹具 | 三范式静默脚本 |
| `docs/m2/README.md` | M2 索引 | M2 文档入口：文件清单、状态、命名约定 |
| `docs/m2/M2_PLAN.md` | M2 计划 | M2 主计划：七项目标判据、44 卡串行卡表、门禁预算、单卡规程、文件占用、风险 |
| `docs/m2/M2_DECISIONS.md` | M2 决策 | A02 决策清单 D1–D13（待拍板，红/黄灯分级，含默认建议） |
| `docs/m2/M2_BASELINE.md` | M2 实测 | A01 基线对账：verify 11 步留证、唯一基线数字、偏差处置、债务台账 |
| `docs/m2/M2_PROPOSAL_CAPABILITY_SLOTS.md` | M2 归档 | 能力槽位扩展提案（ai_ide 侧），不在 M2 执行链路内 |

---

## 二、落点规则（后续每个阶段沿用）

1. **阶段私有文档进 `docs/m<N>/`**：Sprint 方案、计划、收口记录、偏差登记、决策裁决（如 `M3_PLAN.md` → `docs/m3/M3_PLAN.md`），**不再平铺在 `docs/` 根**。
2. **阶段内的范式/夹具进 `docs/m<N>/norms/`**：norm 文档与其配套 json/rhai 夹具放同目录，保持相对引用有效。
3. **跨阶段活文档留 `docs/` 根**：ARCHITECTURE / ASSETS / PHYSICS / AI_ROADMAP / AI_WORKFLOW / CI_GPU_RUNNER / plugin-packaging 等持续被各阶段引用的文档**不随阶段移动**。
4. **机读产物留根且路径锁定**：`docs/capabilities.json` 被 `xtask/src/gen_capabilities.rs:94` 与 `xtask/src/invariants.rs:138` 以 `root.join(...)` 硬编码；`docs/m1/M1_PLAN.md` 被 `xtask/src/doc_gate.rs` 的 `LEDGER_DOC` 常量引用。移动前必须同批改常量，否则 `verify` 红。
5. **引用一律写仓库根相对全路径**：`docs/m1/M1_PLAN.md`，禁止裸文件名（`M1_PLAN.md`），避免目录分层后指向不明。
6. **新增目录/移动文档后同步三处**：本文件 §一 索引、根 `README.md` §1 目录树、涉及引用的文档与代码注释。
7. **历史文档内的文件树/路径树是当时快照**：如 `docs/m0/M0S5_PROTOCOL.md` 中的 `docs/` 树保持原样，不改写历史；当前位置一律以本文件 §一 为准。

---

## 三、迁移记录（2026-11-19：按阶段分目录）

| 旧路径 | 新路径 |
|---|---|
| `docs/M0S1_RENDERER_SPLIT.md` … `docs/M0S5_PROTOCOL.md`（5 个） | `docs/m0/` 同名 |
| `docs/M1_PLAN.md`、`docs/M1_S1D_SUBSTITUTION.md`、`docs/M1_S4_PLAN.md`、`docs/M1_S4F_PLAN.md`、`docs/M1_S5_PLAN.md` | `docs/m1/` 同名 |
| `docs/norms/norm1_build.md`、`norm2_verify.md`、`norm3_rollback_retry.md`、`scene_norms.json`、`silent.rhai` | `docs/m1/norms/` 同名 |
| `docs/M2_PROPOSAL_CAPABILITY_SLOTS.md` | `docs/m2/` 同名 |
| `docs/` 根活文档与 `capabilities.json` | 不动 |

同批完成的引用同步：全仓 86 处 `docs/M0S*` / `docs/M1_*` / `docs/norms/` / `docs/M2_PROPOSAL*` 前缀改写，覆盖 `xtask/src/doc_gate.rs`（`LEDGER_DOC` 常量 + 8 处注释）、`crates/` 注释、`.github/workflows/ci.yml` 注释等；迁移用 `git mv`，保留文件历史。
