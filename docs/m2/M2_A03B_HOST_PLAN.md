---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: 04526fdd866a8c5c9aceee6b83ed2c05_d92e2129e48c11f1a5ef525400638852
    ReservedCode1: Flp1thwxsm1y3rK+1m6FGvyVyNqKme31dlae2wRqjRiyRH6Q49Bg4zwH5ySMsAKnxJ2WnfhgkMjRU4hV+6Z2ekIwmM9K7chnsyQueMKiAPBsjJrd1KHPyDXEgMG+YiCUxRSuEWQHYo3lFH2UW4aN6BRiRszIJzMS3mq4PF7VA+yfhJ3ijXiGIfrv0Q8=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: 04526fdd866a8c5c9aceee6b83ed2c05_d92e2129e48c11f1a5ef525400638852
    ReservedCode2: Flp1thwxsm1y3rK+1m6FGvyVyNqKme31dlae2wRqjRiyRH6Q49Bg4zwH5ySMsAKnxJ2WnfhgkMjRU4hV+6Z2ekIwmM9K7chnsyQueMKiAPBsjJrd1KHPyDXEgMG+YiCUxRSuEWQHYo3lFH2UW4aN6BRiRszIJzMS3mq4PF7VA+yfhJ3ijXiGIfrv0Q8=
---

# M2-A03b `script` 绑定端口化 · 设计稿（trait 默认方法 + 函数式适配器）

- 卡号：A03b（`M2_PLAN.md` §3.1，D13=B 偿还 `dep_debt` 两条）
- 状态：**设计草案，随 A03 一并确认（2026-11-20 老板拍板：形态采用 trait 默认方法 + 函数式适配器）**
- 依据：`M2_DECISIONS.md` D13=B、D1=B 落地约定第 2 条（A03 起端口落 `r3e-physics`）、`M2_A03_PORT_PLAN.md` §5.2
- 关联：`docs/m0/M0S5_PROTOCOL.md` §13.6（53 条端口契约草案）
- 日期：2026-11-20

---

## 0. 一句话

`r3e-script` 侧自建 `PhysicsHost` / `AudioHost` 两个 trait，**方法签名全部使用脚本原生类型（`&str` / `f64` / `bool` / `Array`），不含任何 `r3e-physics` / `r3e-audio` 类型**；每个方法都**有默认实现**，默认实现统一转发到唯一必实现的函数式入口 `execute(PhysReq) -> PhysRsp`；`r3e-runtime` 装配时用 `Rc<RefCell<Physics>>` 实现该入口。结果：两条 `dep_debt` 真清偿、`r3e-script` 的 allow 表仍为 3 项、`ScriptHost::new` 的 30 处调用点零改动、零行为变化。

---

## 1. 为什么 `r3e-script` 不能依赖 `r3e-physics`

### 1.1 制度层（硬门禁，不是风格偏好）

| 事实 | 位置 | 后果 |
|---|---|---|
| `r3e-script.allow = ["r3e-core", "r3e-doc", "r3e-protocol"]`，不含 `r3e-physics` / `r3e-audio` | `xtask/dep_rules.json` | 现状的两条边属于「不该有但确实存在」，只能靠 `dep_debt` 免死 |
| `dep_debt.limit = 2`，且已被 `script→physics`、`script→audio` **占满** | `xtask/dep_debt.json` | 顶格。M2 期间任何新增跨层边 → `dep-check --strict` 立刻红 |
| D2=B 建 `r3e-character`、D12=B 建 `r3e-terrain` 之后，「脚本调角色 / 脚本调地形」的诱惑必然出现 | `M2_DECISIONS.md` D2/D12 | 债务不清零，B01 / C01 之后的每张卡都要在「脚本要能力」和「不许加边」之间反复绕 |
| D13=B 的收口条件写死：`dep_debt.json` 删两条、`limit` 2 → 0 | `M2_DECISIONS.md` D13 | 不还债 = M2 不通过 |

所以「不能依赖」首先是**账面上没有这条边的额度**。这不是可以商量的偏好，是 D13=B 的验收条件。

### 1.2 架构层（依赖倒置）

- **`r3e-script` 是策略层**：宿主把一段外部输入的 rhai 文本编译成调用序列，脚本内容在运行期才知道、可能由编辑器 / AI 生成 / 玩家存档携带。它对「物理用哪个后端、音频用什么设备」没有任何知情权，也不该有。
- **`r3e-physics` 是实体的实现层**：同一个 `Physics` 门面背后有 `null` / `builtin` / `rapier` 三种后端（D1=B 后默认 rapier）。让脚本层依赖它，等于让「脚本的语言绑定」与「后端的具体类型」在编译期绑死：换后端要重编脚本 crate，`--no-default-features` 回落 builtin 时脚本侧代码路径也跟着变。
- **能力面 vs 类型面**：脚本需要的是「一次射线查询」这个**能力**，不是 `Ray` / `RayHit` / `Capsule` 这些**类型**。`capabilities.json` 三源门禁（`commands 392 / rpc 62 / script_only 330 / domains 17 / invariants 38`）之所以能当门禁，前提就是脚本侧只声明能力、不认实现。
- **沙箱与插件化**：M0S5 协议与能力槽位提案的方向是「脚本只能看到被授予的能力」。若脚本 crate 直接 `use r3e_physics::Physics`，任何拿到 `ScriptContext` 的闭包都能摸到后端私有字段，能力裁剪就无从谈起。

### 1.3 口径校准：`dep_debt` 的「53 方法 / 704 行」与实测的「69 个绑定 / 706 行」不是同一维度

`dep_debt.json` 里物理那条写「53 个绑定方法 / 704 行」，容易让本卡按 53 个点估工。逐项核对后确认：两个数字都对，只是**统计维度不同**，开工前必须分清。

| 维度 | 数值 | 来源与算法 |
|---|---|---|
| **A. 去重被调方法数**（trait 需要覆盖的能力面） | **53**（`physics 16` + `audio 37`） | `M0S5_PROTOCOL.md` §13.4 的按符号细分；本次复核 `api/physics.rs` 去重 16 个（`body_count` / `capabilities` / `ccd_default` / `ccd_of` / `destroy_body` / `drain_events` / `enabled` / `gravity` / `ground` / `kind` / `raycast` / `set_ccd` / `set_ground` / `set_kind` / `shape_cast` / `try_create_body`），audio 侧 36–37 个（`M0S5` 记 37，本次正则计 36，差异在 `play` / `set_source_position` 等少数被局部变量绑定的调用未被单行正则捕获，属统计噪声，不改变量级） |
| **B. `register_fn` 注册数**（要改的调用点数量） | **69**（`physics 24` + `audio 45`） | 本次实测 `api/physics.rs` 24 个 `engine.register_fn("…")`、`api/audio.rs` 45 个。**同一个后端方法可被注册成多个脚本名**（如 `raycast` / `physics_raycast`），故 B > A |
| **C. 绑定代码行数** | **706**（`physics 340` + `audio 366`） | `M0S5` 记 339 + 365 = 704，本次多 2 行（后续小改），属正常漂移 |

对 A03b 的影响，两条都要用：

- **trait 方法数按 A（≈53）设计** —— `PhysicsHost` / `AudioHost` 各覆盖 16 / 37 个能力，一个能力一个方法。
- **改动点与工时按 B（69）估** —— 69 个 `register_fn` 的闭包体各要改一处；行数按 C（706）估。
- **`dep_debt` 记账问题**：`entries` 里物理那条写了「704 行」，而 704 是两条债的行数**合计**（`M0S5` 原文并列写出 339 + 365）。即：物理条目多记了音频的量、音频条目未单列行数。这不影响偿还动作（本卡两条一起删），但提醒后续引用 `dep_debt` 数字时要回查 `M0S5` 原文，别把合计当单条。

（采集方式：`register_fn` 计数用 `register_fn\(\s*"([^"]+)"` 正则；去重方法数用 `c\.(physics|audio)\.borrow(?:_mut)?\(\)\.(\w+)` 正则取第二组去重，另核对少数局部变量绑定调用。）

---

## 2. `r3e-script` 与 `r3e-runtime` 的职责边界

| 维度 | `r3e-script`（`crates/r3e-script`，2019 + 6249 行） | `r3e-runtime`（`crates/r3e-runtime`，`app.rs` 3040 行） |
|---|---|---|
| 定位 | **脚本语言绑定层**：rhai `Engine` + 364 个 `register_fn` + 按域拆分的 `api/` 子模块 | **装配与帧循环层**：持有真实 `Physics` / `Audio` / `Input` / 渲染器实例 |
| 拥有的东西 | 只有**句柄**：`ScriptContext` 里 10 个 `Rc<RefCell<..>>`（doc / physics / input / audio / clock / rng / ui_log / ui_interaction / scene_stack / scene_cmd / pick_bounds），全是共用引用，不持有实体 | 拥有**实体**：`Physics` 的构造、后端选择、`set_enabled` 升级、每帧 `step`；`Audio` 设备；窗口与 GPU 资源 |
| 决策权 | 无。脚本写场景必须走 `ScriptContext::write`；切场景只能往 `scene_cmd` 槽塞命令，由宿主帧首执行 | 全部。帧序、后端、资源生命周期、场景命令的实际执行都在这里 |
| 依赖方向 | `→ core, doc, protocol`（+ 两条待偿还的 debt） | `→ script, physics, audio, render, asset, ...`（装配者，最上层，允许认识所有人） |
| 失败语义 | 只记不抛：`ui_log` / `audio_last_error` / 脚本 `last_error`，因为脚本每帧调用不能把宿主带崩 | 决定是否致命：后端初始化失败、设备丢失在这里终止或降级 |
| 换实现时谁改 | **不该改**。这是 A03b 的验收标准：`r3e-script` 源码与 allow 表都不因后端切换而变 | 只改装配处（一处 `impl PhysicsHost for ..`） |

一句话：**script 定义「脚本能说什么」，runtime 决定「话由谁去执行」**。A03b 要做的是把这条缝补成显式的 trait，而不是让 script 认识 physics。

---

## 3. 三种候选形态对比

| 方案 | 形态 | 物理侧改动 | script 侧改动 | 债能还清 | 评价 |
|---|---|---|---|---|---|
| **A**（原 `M2_A03_PORT_PLAN.md` §5.2 规划） | script 侧自定 `Box<dyn Fn(CastRayQuery) -> Option<HitDto>>` 函数式入口，桥接层落 `r3e-runtime`，DTO ↔ port 类型在 runtime 转换 | 无 | 查询类绑定改走注入；**写操作仍需** `Rc<RefCell<Physics>>` | ❌ 只解查询，写操作仍依赖 → debt 删不掉 | 原规划的权宜之计，被 D13=B 的「债务归零」判据否掉 |
| **B**（本次采纳） | script 侧 `trait PhysicsHost` / `AudioHost`，签名全脚本原生类型，**每个方法有默认实现**，统一转发到唯一必实现的 `execute(Req) -> Rsp`（函数式适配器） | 无（`port.rs` 与本卡解耦） | 25 处 `c.physics.xxx` → `c.host().execute(..)`，绑定体结构不变 | ✅ 两条全清 | 工作量略高于 A，但一次做完、语义显式、可单测 |
| C | 让 `r3e-script` 直接依赖 `r3e-physics`，把 `dep_rules` allow 表加进去 | 无 | 零改动 | ❌ 与 D13=B 正面冲突 | 禁止 |

**为什么 B 的「trait 默认方法」不是花架子**：24 + 45 个方法若全部要求实现者逐条实现，测试就得先造一个假 `Physics`；而默认方法让实现者只需给一个 `execute`，测试可以写 `NullHost`（全拒）/ `EchoHost`（回放固定应答）两个十几行的小结构，A03b 的单测成本因此从「模拟物理」降为「断言转发」。这正是「零行为变化」重构最需要的性质：**能力面在 script 定义完整，实现面收敛到一个函数入口**。

---

## 4. 方案 B 的具体设计

### 4.1 `r3e-script` 侧新增 `src/host.rs`（≈ 210 行，含单测）

```rust
//! 物理 / 音频的**能力面**：只声明脚本能要求什么，不认任何具体实现 crate。
//! 依赖倒置点：`r3e-script` 不认识 `r3e-physics` / `r3e-audio`，
//! 由 `r3e-runtime` 在装配时提供一个「函数式适配器」实现。

/// 一次物理请求：24 个绑定的全部诉求，一个枚举封闭。
pub enum PhysReq<'a> {
    Attach { node: &'a Node, kind: &'a str, mass: f64 },
    Detach { node: NodeId },
    Impulse { node: NodeId, v: [f64; 3] },
    BodyCount,
    Enabled,
    SetEnabled(bool),
    Backend,
    SetBackend(&'a str),
    Capabilities,
    HasCapability(&'a str),
    Raycast { origin: [f64; 3], dir: [f64; 3], max: f64 },
    RaycastFull { origin: [f64; 3], dir: [f64; 3], max: f64 },
    Overlap { center: [f64; 3], half_height: f64, radius: f64 },
    ShapeCastSphere { center: [f64; 3], radius: f64, dir: [f64; 3], max: f64 },
    ShapeCastBox { center: [f64; 3], half: [f64; 3], dir: [f64; 3], max: f64 },
    SetCcd { node: NodeId, on: bool },
    CcdDefault,
    BodySetCcd { node: NodeId, on: bool },
    BodyCcd { node: NodeId },
    TakeEvents,
    Gravity,
    SetGravity { y: f64 },
    Ground,
    SetGround { y: f64 },
}

/// 应答：脚本可见的**扁平**结构，不含 physics 类型。
pub enum PhysRsp {
    Bool(bool),
    Float(f64),
    Text(String),
    Int(i64),
    Hits(Vec<HitDto>),
    Events(Vec<EventDto>),
    None,
}

#[derive(Clone, Copy, Default)]
pub struct HitDto { pub body: u64, pub distance: f64, pub point: [f64; 3], pub normal: [f64; 3] }

/// 唯一必须实现的方法就是 `execute`；其余 23 个能力全是默认方法。
pub trait PhysicsHost {
    /// 函数式适配器入口：`r3e-runtime` 用它把 `Physics` 接进来。
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp;

    // ---- 以下是默认方法：调用点统一形态，实现者无需重写 ----
    fn attach(&self, node: &Node, kind: &str, mass: f64) -> bool {
        matches!(self.execute(PhysReq::Attach { node, kind, mass }), PhysRsp::Bool(true))
    }
    fn detach(&self, node: NodeId) { self.execute(PhysReq::Detach { node }); }
    fn raycast(&self, o: [f64; 3], d: [f64; 3], max: f64) -> Option<HitDto> { /* … */ }
    // … 共 23 条，逐条一行转发 + 解包
}

/// 函数式适配器：任何满足 `Fn(PhysReq) -> PhysRsp` 的闭包**天然就是一个 host**。
/// 测试里写 `let host = |req| PhysRsp::Bool(false);` 即可充当「全拒后端」。
impl<F> PhysicsHost for F where F: Fn(PhysReq<'_>) -> PhysRsp { fn execute(&self, r: PhysReq<'_>) -> PhysRsp { (self)(r) } }

/// 空实现：未接入物理时（编辑器 / 单测）的默认值，行为等同今日的 `null` 后端语义。
pub struct NullPhysicsHost;
impl PhysicsHost for NullPhysicsHost { fn execute(&self, _: PhysReq<'_>) -> PhysRsp { PhysRsp::Bool(false) } }
```

要点：
1. **签名不含 physics 类型** —— `HitDto` / `EventDto` 是 script 自己的扁平 DTO，`Node` / `NodeId` 来自 `r3e-core`（allow 表内）。
2. **`PhysReq::Attach` 带 `&Node`**：因为 `BodyDesc::from_node(node).unwrap_or_else(|| BodyDesc::infer(node))`（现 `api/physics.rs:22`）是**物理类型构造**，必须搬出 script。节点快照走 core 类型，允许。
3. **泛型 `impl<F> PhysicsHost for F`** 让「函数式适配器」成为字面意义：一个闭包就是后端。

### 4.2 `r3e-script` 侧 `ScriptContext` 与 `ScriptHost::new` 的兼容改法

```rust
pub struct ScriptContext {
    // 旧：pub physics: Rc<RefCell<Physics>>,
    phys_host: Rc<dyn PhysicsHost>,      // 私有，杜绝 api/ 子模块再摸到实现
    pub audio_host: Rc<dyn AudioHost>,
    // 其余 9 个字段不变
}
impl ScriptContext { pub(crate) fn host(&self) -> &dyn PhysicsHost { &*self.phys_host } }
```

```rust
// 保留旧签名：30 处调用点（8 处内部 + app.rs:515 + 21 处测试）零改动
impl ScriptHost {
    pub fn new(doc, physics: Rc<RefCell<Physics>>, input) -> Self {
        Self::new_with_audio(doc, physics, input, Audio::silent("…"))
    }
    pub fn new_with_audio(doc, physics: Rc<RefCell<Physics>>, input, audio: Rc<RefCell<Audio>>) -> Self {
        // 兼容层：这里把实体包成适配器，签名不变，测试不受影响
        Self::new_with_hosts(doc, RhaiPhysicsHost::new(physics), input, RhaiAudioHost::new(audio))
    }
    /// 新入口：只为「不想拉实现 crate 的宿主」存在（编辑器 / headless / 未来插件）
    pub fn new_with_hosts(doc, physics: Rc<dyn PhysicsHost>, input, audio: Rc<dyn AudioHost>) -> Self { … }
}
```

兼容层文本上仍出现 `Rc<RefCell<Physics>>` —— **它会随第二个 crate 被搬走**：`ScriptHost::new` / `new_with_audio` 两个便捷构造随 `RhaiPhysicsHost` 一起移入 `r3e-runtime` 的 `script_bind.rs`（约 60 行），`r3e-script` 里只留 `new_with_hosts`。届时 `r3e-script/Cargo.toml` 删 `r3e-physics` / `r3e-audio` 两行，`dep_debt` 两条同时删、`limit` → 0。

> 这是本卡唯一需要「跨文件搬迁」而非「就地改」的部分，也是唯一会让 30 处调用点改 import 的地方（把 `r3e_script::ScriptHost::new` 换成 `r3e_runtime::ScriptHost::new`）。若老板希望 30 处一处不动，则退化方案是：`RhaiPhysicsHost` 留在 `r3e-script` 内 —— 但那意味着 `r3e-script` 仍依赖 physics，债务删不掉。**二者不可兼得，本稿选前者（债要真还）。**

### 4.3 `r3e-runtime` 侧（`src/script_bind.rs`，新建 ≈ 180 行）

```rust
//! 脚本能力面的**实现方**：把 `Physics` / `Audio` 实体接成 `PhysicsHost` / `AudioHost`。

pub struct RhaiPhysicsHost { pub inner: Rc<RefCell<Physics>> }

impl PhysicsHost for RhaiPhysicsHost {
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp {
        match req {
            PhysReq::Attach { node, kind, mass } => {
                // 原 api/physics.rs:13–40 的闭包体搬到这里（Node 快照 → BodyDesc 构造 → try_create_body）
                let mut desc = BodyDesc::from_node(node).unwrap_or_else(|| BodyDesc::infer(node));
                desc.kind = BodyKind::parse(kind);
                desc.mass = if mass > 0.0 { mass as f32 } else { 1.0 };
                desc.position = node.world_position();
                let old = node.body.map(BodyHandle);
                let mut p = self.inner.borrow_mut();
                if let Some(h) = old { p.destroy_body(h); }
                match p.try_create_body(node.id, desc) { Some(h) => PhysRsp::Int(h.0 as i64), None => PhysRsp::Bool(false) }
            }
            PhysReq::Raycast { origin, dir, max } => { /* Ray 构造 + p.raycast → HitDto */ }
            // … 24 个分支，与今日闭包体一一对应
        }
    }
}
```

`r3e-runtime` 的 `allow` 已是全量装配方（`app.rs` 同时用 script + physics + audio），新增本文件不产生任何新依赖边，`dep_rules.json` 无需改。

### 4.4 为什么这样可以宣称「零行为变化」

| 行为面 | 变化 |
|---|---|
| 脚本 API 名字 / 参数 / 返回类型 | **零变化**（`register_fn` 的闭包签名保持 rhai 原生类型） |
| 后端选择、`set_enabled` 升级、CCD、事件抽取 | **零变化**（同一段 `Physics` 方法调用，只是换了文件） |
| 空后端语义 | **零变化**（`NullPhysicsHost` 的应答映射对齐今日 `null` 后端返回值） |
| 调用时序 | **零变化**（仍是同步直调，无队列、无缓冲） |
| 错误文案 | **零变化**（`println!("[script] attach_body: …")` 随闭包体一起搬走） |
| 1817 基线测试 | 测试调用 `ScriptHost::new` 时 import 变、断言不变 |

---

## 5. 执行步骤与判据

| 步 | 动作 | 完成判据 |
|---|---|---|
| S0 | 前置：A03 已收口（端口在 `r3e-physics/src/port.rs`），`cargo test -p r3e-script` 基线绿 | 记档 1817 基线实测数 |
| S1 | 新建 `r3e-script/src/host.rs`（`PhysReq` / `PhysRsp` / `PhysicsHost` / `NullPhysicsHost` + 单测） | `cargo test -p r3e-script` 绿，新增 `#[test]` 记账 |
| S2 | `ScriptContext.physics` → `phys_host`，`api/physics.rs` 25 处调用点改 `c.host()`；`BodyDesc` 构造逻辑暂留原处（临时桥） | 编译绿、测试绿（此步仍保留 physics 依赖，属过渡态） |
| S3 | `RhaiPhysicsHost` 落 `r3e-runtime/src/script_bind.rs`，`BodyDesc` 构造搬入；删 `r3e-script` 的 physics 依赖 | `dep-check --strict` 违规 0、债务剩 1 条 |
| S4 | 音频同构（45 个绑定、`api/audio.rs` 366 行） | 债务剩 0 条 |
| S5 | `dep_debt.json` 删两条、`limit` 2 → 0；`tests_baseline.json` 同步；`r3e-script.allow` 保持 3 项 | 全 11 步 verify 退出码 0；`dep_debt` 0 / `limit` 0 |
| S6 | 回放金标两次一致 + 文档同频 + 单卡提交 | `replay` 哈希一致；`check-tests` 显示 `r3e-script` 增量 ≥ +56 |

### 5.4 `+56` 测试预算落点（按 69 个绑定重分配）

| 落点 | 内容 | 静态 `#[test]` |
|---|---|---|
| `host.rs` 单测 | `NullPhysicsHost` 全拒语义、泛型闭包适配、`PhysRsp` 解包、24 条默认方法转发（可用表驱动一条 `#[test]` 覆盖多分支） | ≈ 18 |
| `tests/host_bind_physics.rs` | 24 个绑定经 host 后的脚本可见行为（双方言：Null 与 Fake 各一轮） | ≈ 22 |
| `tests/host_bind_audio.rs` | 45 个音频绑定的代表性 8 条 + 错误账本语义 | ≈ 10 |
| `r3e-runtime` 侧 | `RhaiPhysicsHost::execute` 分支覆盖（`--no-default-features` 与默认各一轮） | ≈ 6 |
| **合计** | `r3e-script` 增量 ≥ **+56** | **56** |

> 口径提醒：`check-tests` 是静态数 `#[test]` 出现次数，同一套用例双后端循环跑只计一次；因此上面的 56 条必须是真的 56 个测试函数。

---

## 6. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| 30 处 `ScriptHost::new` 调用点改 import | 改动面扩散到 editor-host 与 21 个测试文件 | `r3e-runtime` 里 `pub use script::ScriptHost;`（同类型再导出），调用点只删一个 `use` 行；S3 一步做完并全量 `cargo test --workspace` |
| 类型安全降级（`[f64;3]` / `PhysRsp` 取代 `Vec3` / `RayHit`） | 参数拼错从编译期错变成运行期错 | 转换点全部收敛在 `RhaiPhysicsHost::execute` 一个 match（24 处），并用 `tests/host_bind_physics.rs` 逐绑定断言 |
| `PhysReq` 的生命周期参数（`&Node` / `&str`）让 trait 不便 `Rc<dyn>` 共享 | 编译期报错 | `execute<'a>(&self, PhysReq<'a>)` 用 HRTB，trait 本身不含泛型参数，`Rc<dyn PhysicsHost>` 可用；`attch` 等默认方法仍可拿 `&Node` |
| 每绑定多一次 enum 构造 + match | 微秒级开销；脚本 `register_fn` 本身开销远大于此 | 不做优化；若 H05 采样显示异常，再测 `dyn Fn` 直调版 |
| 音频 45 个绑定比物理 24 个更碎 | S4 可能超期 | S4 允许单独提交、拆成 S4a（空间音频 20 个）/ S4b（其余 25 个） |

---

## 7. 与 A03 / B01 的咬合

1. **与 A03**：`port.rs` 的三组查询端口服务的是 **C 阶段角色控制器**（`r3e-character` 只许依赖 core + physics.port）；A03b 的 `PhysicsHost` 服务的是**脚本绑定**。两者互不引用：`PhysicsHost` 的 `RhaiPhysicsHost` 实现体内**可以直接调 `Physics` 的既有方法**，不必绕 `port.rs`（少一层转发、行为与今日完全一致）。若某天要求「脚本只能看到端口能力」，再让 `RhaiPhysicsHost` 改走 `port.rs` 即可 —— 因为脚本侧只认 `PhysReq`，实现侧换底座不影响脚本。
2. **与 B01**：B01 建 `r3e-character` 时 `allow = ["r3e-core", "r3e-physics"]`；A03b 完成后 `dep_debt.limit = 0`，B01 之后任何新边依然要靠 `dep_rules` 白名单合法化，不会再出现「顶格 + 免死金牌」的灰色地带。
3. **顺序不可换**：A03b 必须在 A04（`scene.rs` 拆域）之前 —— 拆域与绑定重写同批改会互相干扰（`M2_DECISIONS.md` D13 第 1 条）。

---

## 8. 不做清单

1. 不改任何脚本 API 的名字、参数个数或返回类型；不新增命令 / RPC / 不变量（`capabilities.json` 一字不改）。
2. 不把 `port.rs` 牵进脚本链路（`PhysicsHost` 与 `PhysicsQueryPort` 是两件事，见 §7.1）。
3. 不动 `r3e-physics` 的既有语义、不改 `BackendCaps`、不改 `PhysicsConfig`。
4. 不为省 30 处 import 改动而把 `RhaiPhysicsHost` 留在 `r3e-script`（那会保住依赖边，与 D13=B 冲突）。
5. 不在本卡做 `app.rs` 拆分（3040 行属 G02），只新增 `script_bind.rs` 一个文件（≈180 行 < 800，不触发 `size_debt`）。

## 9. 实施记录（2026-11-20 收口，实况与设计稿的偏离）

### 9.1 五处偏离（均已落地并 verify 全绿）

| # | 设计稿写的 | 实际做的 | 为什么 |
|---|---|---|---|
| 1 | trait 落 `r3e-script/src/host.rs` | 落 **`r3e-core/src/host/`**（`mod.rs` 158 行 + `physics.rs` 335 + `audio.rs` 510） | `r3e-physics` / `r3e-audio` 必须 `impl` 这两个 trait。trait 若落 script，则 `physics → script`、`audio → script` 成环，且直接违反 `dep_rules`。`r3e-core` 是三方的唯一共同下界，契约住这里才成立 |
| 2 | 适配器落 `r3e-runtime/src/script_bind.rs`（新建 ≈180 行） | 落 **`r3e-physics/src/host_adapter.rs`**（`PhysicsFacade`）与 **`r3e-audio/src/host_adapter.rs`**（`AudioFacade`） | 谁持有真门面谁做适配。调用方有三类（`app` / `editor-host` / `script` 单测），放在各域 crate 内三类都能直接用；经 runtime 绕一层反而把适配器变成 runtime 私有物 |
| 3 | 保留 `ScriptHost::new(doc, Rc<RefCell<Physics>>, input)` 旧签名（"调用点零改动"） | 签名改为 `Rc<dyn PhysicsHost>`，`new_with_audio` 改 `Rc<dyn AudioHost>`；调用点写 `PhysicsFacade::wrap(rc)` | 保留旧签名意味着 `new` 内部要知道 `Physics` 具体类型 —— 那 `r3e-script` 的 lib 依赖边删不掉，D13=B 就是空话。**改用 §4.2 的取舍方案**：30 处调用点改一行 wrapping，`ScriptHost::new` 名字与参数个数不变 |
| 4 | 新建 `script_bind.rs` 装 `RhaiPhysicsHost::execute` 的 24 处 match | match 直接写在 `host_adapter.rs` 的 `impl PhysicsHost for PhysicsFacade` 里 | 与偏离 2 同源：适配器在实现方，match 跟着走 |
| 5 | 测试预算 `+56`（按 69 个绑定重分配） | 实测 **+8**（`r3e-core` 625 → 633：host 契约 8 条） | 端口化按「零行为变化」处理，**不重写既有绑定测试**：69 个绑定的行为断言原样保留（它们现在跑过 Facade，等于顺带覆盖了适配器）。新增的 8 条只测「契约本身」——空后端全拒语义、闭包即适配器、`Node` 快照能跨 trait 边界、静默音频的状态行 |

### 9.2 实测数字

| 项 | 计划 | 实测 |
|---|---|---|
| `dep_debt` | 删 2 条，`limit` 2 → 0 | 已删，`limit = 0`、`entries = []`；`dep-check` 债务 0 条 |
| tests | +56（总 2017 → 2073） | **+8**（总 2017 → **2025**），2025 条全部通过 |
| `r3e-script` allow 表 | 保持 3 项不变 | 未动（`dep_rules.json` 无改动） |
| `lib.rs` 行数 | `r3e-script/lib.rs` 不涨 | 2018 → **1979**（构造三函数搬到新文件 `host_wiring.rs` 抵掉 trait 化新增行）；`r3e-audio/lib.rs` 1375 → 1375、`r3e-physics/lib.rs` 1542 → 1542（注释压缩抵掉 re-export 行） |
| 回放金标 | 不漂移 | `final=385482eb033d2964`，600 帧 / 10 采样点两次一致 |
| verify | 11 步全绿 | 全绿 |
| `capabilities.json` | 一字不改 | 未动（392 命令 / 62 RPC / 330 script-only / 38 不变量） |

### 9.3 落地文件清单

- 新增：`crates/r3e-core/src/host/{mod.rs,physics.rs,audio.rs}`、`crates/r3e-physics/src/host_adapter.rs`、`crates/r3e-audio/src/host_adapter.rs`、`crates/r3e-script/src/host_wiring.rs`
- 删除：`crates/r3e-script-bind/`（A03 遗留的一次性过渡 crate，能力面已收口到 `r3e-core::host`）
- 改造：`r3e-script/src/{lib.rs, api/mod.rs, api/physics.rs, api/audio.rs}` + 15 个集成测试文件（`PhysicsFacade::wrap` / `AudioFacade::wrap`）、`r3e-runtime/src/{app.rs,lib.rs,port.rs}`、`r3e-editor-host/src/{host.rs, api/*}`、workspace `Cargo.toml`
- 台账：`xtask/dep_debt.json`（清空）、`xtask/size_debt.json`（`r3e-script/lib.rs` 2018 → 1979 等 3 项下降登记）、`xtask/tests_baseline.json`（`r3e-core` 633、合计 2025）

### 9.4 对后续卡的结论

1. `dep_debt.limit = 0` 后**没有免死金牌**：B01 建 `r3e-character` 时若想连 `r3e-script`，门禁直接红——照旧走 core 契约 + 命令注册。
2. `PhysicsHost` / `AudioHost` 的「加一条脚本 API = 加一个 `Req` 变体 + 一行默认方法」这个性质要守住：新脚本 API 一律走能力面，不允许在 `api/physics.rs` 里再摸具体门面。
3. 接口分层的现状是 **两层**（不是设计稿设想的一层）：`PhysicsQueryPort`（A03，服务 C 阶段的 `r3e-character`，只读查询）与 `PhysicsHost`（A03b，服务脚本绑定，读写全含）。二者互不引用，不要为了"统一"把脚本链路改走 `port.rs`。

---

*（本稿为设计草案，实施前需老板确认 §4.2 的「30 处调用点改 import」这一取舍 —— 已按该取舍实施，见 §9.1 第 3 条。）*
*（内容由AI生成，仅供参考）*
