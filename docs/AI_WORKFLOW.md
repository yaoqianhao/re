---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_3c5ef342e18111f19919525400638852
    ReservedCode1: YFt7if9ocHoHbWQ0nLBD3WmIVmbjaeW2G98XcPtSrfMoYcw/LMjVGDpV7u7oee1fGIGJfUm1kUuL4Pwys2QE1bH+cB9WliuLL+7VIwbSjMg/DXu2k7uZimuMd7pIckyNar+vjOOzR+89yvSIPLzt1TxRHTwImkzks8wmsPLd0gwJ/l3AZ6HeRdVDzX0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_3c5ef342e18111f19919525400638852
    ReservedCode2: YFt7if9ocHoHbWQ0nLBD3WmIVmbjaeW2G98XcPtSrfMoYcw/LMjVGDpV7u7oee1fGIGJfUm1kUuL4Pwys2QE1bH+cB9WliuLL+7VIwbSjMg/DXu2k7uZimuMd7pIckyNar+vjOOzR+89yvSIPLzt1TxRHTwImkzks8wmsPLd0gwJ/l3AZ6HeRdVDzX0=
---

# AI_WORKFLOW：M0 之后的标准工作流

> 落地于 M0-S5（S5-07，2026-11-16）。适用于 M1 及之后所有冲刺。
> 一句话：**改码 → verify → 判定**，三步以外的一切动作都要先想清楚"这一步的验收命令是什么"。

---

## 零、三条铁律

| # | 铁律 | 违反的典型后果 |
|---|---|---|
| 1 | **先绿再动**：任何改动前 `cargo xtask verify` 必须 exit=0，不绿不准改 | 在红灯上叠加改动，最后分不清是谁弄红的 |
| 2 | **零行为变化**：纯搬迁/重构批次内不改语义，语义改动单独成批 | 测试数下降、像素基线漂移、回放哈希变化，责任无法定位 |
| 3 | **数值只用实测**：文档、卡片、方案里的行数/测试数/耗时，一律以 `verify` 或对应子命令当场输出为准 | 方案与代码口径漂移（历史上出现过「invariants 25 实为 24」「清单 2800 行实为 4744 行」） |

补充纪律：

- **本地提交不推送**，按 D1…Dn 分段提交，每段对应一次 verify 通过。
- **文档与代码同频**：改动触及 crate 结构 / 门禁 / 基线时，同一批提交里改文档，不留到下一个 Sprint（口径见 §五）。
- **不许硬扛**：某项工作包实测成本突破铁律 2（会改语义）时，按"降级记账"处理，见 §四。

---

## 一、命令块 1：开工前基线（每条命令可直接复制）

```bash
# 1) 确认工作区干净、起点是绿
git status --short          # 期望：空（有未提交改动先收尾或 stash）
cargo xtask verify          # 期望：exit=0，末尾出现回放哈希行

# 2) 记录本次工作的起点数值（写进 Sprint 卡或提交信息）
cargo xtask lint-size       # 期望：白名单外 0 文件
cargo xtask check-tests     # 期望：#\[test\] 总数 == 基线快照
cargo xtask dep-check       # 期望：0 违规 / 债务 2 条（limit=2）
cargo xtask gen-capabilities --check   # 期望：清单与代码一致
cargo xtask invariants      # 期望：38 = 38 = 38
```

`verify` 已包含上述全部子命令（九道门），**单独跑它们只为拿到可引用的数字**，判定仍以 `verify` 为准。

---

## 二、命令块 2：改码闭环（每个工作单元一次）

```bash
# ① 改一小步（一个文件、一个函数、一次纯搬迁）
# ② 先跑最窄的验证，再跑全量
cargo test -p <受影响的 crate>            # 期望：0 failed，测试数不降
cargo xtask verify                        # 期望：exit=0；热态 14.6–20.4s，含 crate 重编译最长 138.5s
# ③ 绿 → git add -A && git commit -m "<Sprint>-<序号> <一句话>"
# ④ 红 → 进 §三 判定树，禁止在红灯上继续加改动
```

判定标准（三问，全部满足才算这一步完成）：

1. **目标**：这一步声称要做的事，有可指认的产物（文件、crate、命令、测试名）？
2. **产物**：产物真实存在于磁盘，且被 `git status` 看见？
3. **缺口**：没做完的部分，写进 Sprint 卡"遗留项"或 `dep_debt.json`，不允许口头略过。

---

## 三、失败判定树（红在哪里 → 怎么办）

| 门的报错 | 含义 | 正确动作 |
|---|---|---|
| `fmt` 非 0 | 格式未统一 | `cargo fmt --all` 后重跑 |
| `clippy -D warnings` | 有警告 | 修代码，**不要** 加 `#[allow]` 绕过（除非该 allow 有注释说明理由） |
| `test` 数下降 | 删了测试或测试被 feature 屏蔽 | 恢复测试；确实要删 → 先改 `tests_baseline.json` 并在提交信息写明原因 |
| `doc-gate` 违规 | 出现第二份真值 / 绕过写入口 | 改回单一真值，不准把文件加进白名单 |
| `lint-size` 白名单外超标 | 新文件 >800 行 | 拆文件；确需保留 → 进 `size_debt.json`（只减不增） |
| `check-tests` | `#[test]` 总数低于快照 | 同上 |
| `gen-capabilities --check` | 清单与代码漂移 | 跑 `cargo xtask gen-capabilities` 重新导出并审 `git diff`，**不许手改 json** |
| `invariants` 三源不等 | `ISSUE_CODES` / `RPC_METHODS` / `cmd_spec` 不一致 | 补其中最该有的那一源，另两源跟随，不做"择一为准" |
| `dep-check` 命中四判定 | 白名单外依赖 / 债务残留 / 超限 / 未登记 | 见 §四 |
| `replay` 两次哈希不同 | 引入了非确定性 | 查 HashMap 迭代序、浮点累加顺序、时间源；回放门是确定性唯一守门人 |

---

## 四、记账与降级（只减不增的三本账）

三本账都在 `xtask/` 下，**是数据文件不是垃圾桶**，每条都要能被下一个人读懂：

| 账本 | 记什么 | 关键约束 |
|---|---|---|
| `dep_debt.json` | 允许存在的依赖边（`from` / `to` / `why` / `plan`） | `limit` 只减不增；`plan` 必须写清"哪个 Sprint 还、还成什么形态"；债还清后必须销账，**保留条目也算违规** |
| `size_debt.json` | 允许超 800 行的文件及当前行数 | 行数不得上涨；只减不增 |
| `tests_baseline.json` | 全仓 `#[test]` 按 crate 的快照 | 任何一项低于快照即红 |

**降级记账流程**（当"改代码修掉"会突破零行为变化纪律时使用）：

1. 用 `grep` 全仓确认假设真伪（不凭方案文档下结论）；
2. 量化真实成本（方法数 / 行数 / 测试数 / 涉及 crate）；
3. 核对目标收益是否已被其他机制覆盖（例：能力面收敛已由 `capabilities.json` 承担）；
4. 在方案文档写"偏差"行 + 专门小节，注明路线图目标态与现状的区别；
5. 把降级项计入对应账本，给出**偿还触发条件与落点**；
6. 提交信息里写明"降级"与对应账本条目编号。

> 案例：M0-S5 的 S5-04（`script → physics/audio` 端口化）实测 53 方法 / 704 行 / 14 测试直连，改造成本突破零行为变化 → 降级入 `dep_debt.json`，偿还落点 M1 端口契约。

---

## 五、文档同频检查表

改动触及下表左侧内容时，**同一批提交**里更新右侧文档：

| 改了什么 | 必须同频的文档 |
|---|---|
| crate 数量 / 目录 / 依赖方向 | `ARCHITECTURE.md` §2 结构树、§3.1 依赖说明、`AI_ROADMAP.md` §0.3 + §2.2 |
| 单文件规模 / 测试总数 | `ARCHITECTURE.md` §3.2 规模表、`AI_ROADMAP.md` §0.2 + §八 看板 |
| `xtask` 子命令 / 门禁组成 | `ARCHITECTURE.md` §3.3、`AI_ROADMAP.md` 附录 B、本文档 §一 |
| `capabilities.json` 结构或数量 | `ARCHITECTURE.md` 能力清单节、`AI_ROADMAP.md` §0.2 |
| 账本条目（债务还清 / 新增） | `AI_ROADMAP.md` §0.2 / §0.3、方案文档偏差表 |
| Sprint 收口 | 对应 Sprint 卡（加 ✅ 日期 + 收尾实测）、§0.1 状态表、§八 看板追加一行 |

**文档数值禁止沿用预估**：写之前先跑命令，把输出贴进去；数字带日期（如"2026-11-16 实测"）。

---

## 六、停机条件（遇到就停，不要硬扛）

1. `verify` 连续 2 次因同一门失败且原因不明 → 停，写清现象后回退到上一个绿提交；
2. 改动需要动 `core` 的公共类型且影响 ≥3 个 crate → 停，先出方案对比，不边改边想；
3. 出现"测试数下降 / 回放哈希变化 / 像素基线漂移"任一项 → 停，这是零行为变化被破坏的信号；
4. 方案与实测不符（行数、数量、方法数）→ 先改方案文档再继续，不带着错口径往下走。

---

## 七、M1 起点

- 起点状态：M0 全绿（1,476 测试 / 九道门 / 20.4s 热缓存），债 4 条、`>800` 行 23 项已记账；
- 第一优先级：`M1-DOC-FIELDS`（`r3e-core/src/scene.rs` 4,057 行拆分）+ 端口契约还债；
- 每个 Sprint 收口必须留下"可演示证据"（一条命令 + 一段输出），否则视为未完成。
*（内容由AI生成，仅供参考）*
