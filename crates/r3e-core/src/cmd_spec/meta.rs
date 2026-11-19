//! 仅有 RPC、没有脚本函数的命令（引擎信息 / 截图 / 脚本执行 / 只读装配 / 事务协议）。
//!
//! 生成方式见 `docs/M0S2_API_SPLIT.md` §4：脚本名与签名由 `temp/gen_cmd_spec.py` 扫
//! `register_fn` 得到，`rpc` 一列是人工对照 `dispatch` 分支裁决的结果；
//! `reversible` / `cost` 两列按文档记录的规则统一裁决，逐条复核留到 S5-02。
//!
//! M1-S1 在此追加 `tx.*` 五条：事务是**纯 RPC 能力**，脚本侧刻意不开放——脚本里
//! 若能手搓事务，就等于给了一条不经命令层记账的写入口，S4 验证器会形同虚设。

use super::{CommandSpec, Cost, Domain, SINCE_V0};

/// 本域全部命令。
pub(super) static ALL: &[CommandSpec] = &[
    CommandSpec {
        name: "engine.info",
        rpc: Some("engine.info"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "engine.stats",
        rpc: Some("engine.stats"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "capture.frame",
        rpc: Some("capture.frame"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Heavy,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.list",
        rpc: Some("scene.list"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Linear,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.get",
        rpc: Some("scene.get"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.settransform",
        rpc: Some("scene.settransform"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.setenvironment",
        rpc: Some("scene.setenvironment"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.shadow",
        rpc: Some("scene.shadow"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "scene.stack",
        rpc: Some("scene.stack"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "ui.set",
        rpc: Some("ui.set"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "ui.state",
        rpc: Some("ui.state"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "light.list",
        rpc: Some("light.list"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Linear,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "audio.spatial.list",
        rpc: Some("audio.spatial.list"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Linear,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "audio.spatial.set",
        rpc: Some("audio.spatial.set"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "script.eval",
        rpc: Some("script.eval"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Heavy,
        since: SINCE_V0,
    },
    CommandSpec {
        name: "script.reload",
        rpc: Some("script.reload"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Heavy,
        since: SINCE_V0,
    },
    // ---- M1-S1 事务协议：5 条纯 RPC 方法，无对应脚本函数 ----
    CommandSpec {
        name: "tx.begin",
        rpc: Some("tx.begin"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Cheap,
        since: "M1-S1",
    },
    CommandSpec {
        name: "tx.apply",
        rpc: Some("tx.apply"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只往事务里堆暂存，不碰场景、不推版本：文档状态没变，故按可逆标。
        reversible: true,
        cost: Cost::Cheap,
        since: "M1-S1",
    },
    CommandSpec {
        name: "tx.preview",
        rpc: Some("tx.preview"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 影子预演：结构上不可能写文档，是整条链上唯一「免费试」的方法。
        reversible: true,
        cost: Cost::Linear,
        since: "M1-S1",
    },
    CommandSpec {
        name: "tx.commit",
        rpc: Some("tx.commit"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 真正落场景、推版本，是 AI 写世界的唯一出口。
        reversible: false,
        cost: Cost::Linear,
        since: "M1-S1",
    },
    CommandSpec {
        name: "tx.rollback",
        rpc: Some("tx.rollback"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 终结一笔事务：虽不写场景，但把暂存一次性作废，属于状态变更。
        reversible: false,
        cost: Cost::Cheap,
        since: "M1-S1",
    },
    // ---- M1-S3-A 意图原子单元：1 条纯 RPC 方法 ----
    //
    // 与 `tx.apply` 并列而不合并，是因为两者的**记账单位**不同：`tx.apply` 攒的是
    // 一条命令、一次 undo 就是那条命令；`tx.intent` 攒的是「一条意图展开出的
    // N 条命令」，`staged` 涨 N、`intents` 涨 1，撤销时仍是整笔事务一格（M1-S3-A 契约：
    // 提交与撤销都以**事务**为单位）。合成一个方法只会让「这条到底算几步」靠 `op`
    // 里有没有冒号来猜。
    CommandSpec {
        name: "tx.intent",
        rpc: Some("tx.intent"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 同 `tx.apply`：只攒暂存，不碰场景、不推版本。
        reversible: true,
        cost: Cost::Linear,
        since: "M1-S3-A",
    },
    // ---- M1-S2-B 语义查询：3 条纯 RPC 方法，同样无对应脚本函数 ----
    //
    // 脚本侧不开放查询面的理由与 `tx.*` 不同：脚本里拿到的就是 `&mut Scene`，直接
    // 调 `Scene::query` 比绕一圈 RPC 更短；这三条是**给 AI / 远端**的语义入口。
    CommandSpec {
        name: "query.find",
        rpc: Some("query.find"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只读：不建事务、不产生写点、不动 StateHash。
        reversible: true,
        cost: Cost::Linear,
        since: "M1-S2-B",
    },
    CommandSpec {
        name: "query.describe",
        rpc: Some("query.describe"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 单节点投影 + 组件位：只看一个节点，不遍历场景。
        reversible: true,
        cost: Cost::Cheap,
        since: "M1-S2-B",
    },
    CommandSpec {
        name: "query.stats",
        rpc: Some("query.stats"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 普查要逐节点算九位组件与八个类别，量级随规模涨。
        reversible: true,
        cost: Cost::Linear,
        since: "M1-S2-B",
    },
    // ---- M1-S4-D 协议面八条：物理查询 / 导航查询 / 场景材质表 ----
    //
    // 这八条同样没有脚本函数，归 `Meta` 域（与 `tx.*` / `query.*` 一个口径）：脚本里
    // 拿到的是 `&mut Scene` 与 `&Physics`，直接调本 crate 的函数比绕一圈 RPC 更短；
    // 这三组是**给 AI / 远端**的查询面与素材入口。
    //
    // 回包契约的硬约束（与其余物理/导航入口一致）：
    // * `physics.*` 三条一律把 `capabilities` 数组带在回包里——AI 不必先问一次
    //   「这个后端支持不支持」，也能在拿到空结果时区分「真没打到」和「后端不会查」；
    // * 物理开关关闭回 `PHYSICS_DISABLED`、后端不申报该位回 `BACKEND_UNSUPPORTED`，
    //   **绝不回空表**；
    // * `nav.*` 三条在未烘焙时回 `NAV_NOT_BAKED`，而不是「路径为空」。
    CommandSpec {
        name: "physics.raycast",
        rpc: Some("physics.raycast"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只读：不建事务、不产生写点、不动 StateHash。
        reversible: true,
        // 后端按刚体表线性扫（内置后端是解析法求交）。
        cost: Cost::Linear,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "physics.overlap",
        rpc: Some("physics.overlap"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // 球体覆盖范围要逐刚体判，命中数还随场景规模涨。
        cost: Cost::Linear,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "physics.cast",
        rpc: Some("physics.cast"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        cost: Cost::Linear,
        since: "M1-S4-D",
    },
    // ---- M1-S4-H：编辑态的显式装配口 ----
    //
    // `physics.bind` 是这批里**唯一会动物理世界**的命令，但场景仍只读：装配产出句柄表，
    // 不写 `Node::body`、不进撤销栈、`StateHash` 前后一致，所以照样是 `reversible`。
    // 放在 `Meta` 域的理由与上面八条相同：脚本侧直接拿着 `&mut Physics` 与 `&Scene`，
    // 这条是给 AI / 远端补「编辑态没有装配入口」的那道口子。
    CommandSpec {
        name: "physics.bind",
        rpc: Some("physics.bind"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 不写场景 ⇒ 没有撤销语义要补偿。
        reversible: true,
        // 逐节点取网格推 `BodyDesc`；幂等那一路只查句柄表，代价按节点数线性。
        cost: Cost::Linear,
        since: "M1-S4-H",
    },
    CommandSpec {
        name: "nav.bake",
        rpc: Some("nav.bake"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只读场景：烘焙不写节点、不推版本，产物挂在宿主侧。
        reversible: true,
        // 整场景栅格化：格数随范围平方涨，是本期最重的一条只读命令。
        cost: Cost::Heavy,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "nav.path",
        rpc: Some("nav.path"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // A* 扩展次数上限 `NAV_PATH_MAX_EXPANSIONS`，量级随网格涨。
        cost: Cost::Heavy,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "nav.reachability",
        rpc: Some("nav.reachability"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // 先判连通块（常数级），断连时再找断点（一次邻域扫）。
        cost: Cost::Linear,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "scene.material.list",
        rpc: Some("scene.material.list"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // 遍历材质表（上限 `SCENE_MATERIAL_MAX` 条）。
        cost: Cost::Linear,
        since: "M1-S4-D",
    },
    CommandSpec {
        name: "scene.material.upsert",
        rpc: Some("scene.material.upsert"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 写场景材质表：可被事务回滚，但与 `scene.setmaterial` 一样算命令面。
        reversible: true,
        // 表内按名定位一次，常数级。
        cost: Cost::Cheap,
        since: "M1-S4-D",
    },
    // ---- M1-S4-F 协议面一条：三层体检 ----
    //
    // 同 `query.*` / `physics.*` 的口径：归 `Meta` 域、无脚本函数（脚本里直接调
    // `r3e-doc` / `r3e-physics` / `r3e-nav` 更短），这条是**给 AI / 远端 / CI** 的。
    //
    // 回包契约的硬约束（与其余「可能没查」的入口一致）：
    // * `ok` 只表示「没有 Error」，`passed` 才表示「三层都跑过且没有 Error」——
    //   `unchecked` 非空时 `ok` 仍可能为 `true`，调用方判门禁必须用 `passed`；
    // * 物理关闭 / 后端不申报 `shape_cast` / 导航未烘焙一律落 `unchecked`，
    //   **绝不**把「没检查」写成「通过」，也绝不回空表冒充「查过了」。
    CommandSpec {
        name: "scene.verify",
        rpc: Some("scene.verify"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只读：不建事务、不产生写点、不动 StateHash。
        reversible: true,
        // 静态层全场景扫一遍 + 物理层装配 + 导航层按目标点跑 A* 连通，量级随规模涨。
        cost: Cost::Heavy,
        since: "M1-S4-F",
    },
    // ---- M1-S1-D 协议面四条：把插件的离线只读工具搬到引擎侧 ----
    //
    // 这四条原先住在 `r3e-plugin` 里，直连 `r3e-core` / `r3e-physics` 在**插件进程内**
    // 算完再回给模型。代价是插件二进制把 core 与整个物理后端（含 `rapier` feature）
    // 一起编了进去，`dep_rules.json` 的 plugin→core / plugin→physics 两条债永远还不掉。
    //
    // 搬到宿主之后，插件只剩「发 RPC + 转结果」，债才消得掉。四条都**只读场景**：
    // 不建事务、不产生写点、不动 `StateHash`，所以一律 `reversible` 且落 `Meta` 域
    // （脚本侧直接拿 `&Scene` / `&Physics`，不需要这层转发）。
    //
    // 与 `scene.verify` 的分工必须守住：`scene.validate` 只查**网格本身**（AIR 结构校验），
    // `scene.verify` 查**场景跑起来会不会出问题**（静态/物理/导航三层）。别互相顶替。
    CommandSpec {
        name: "scene.snapshot",
        rpc: Some("scene.snapshot"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 只读：纯遍历，不改任何节点。
        reversible: true,
        // 全场景节点扫一遍（`maxNodes` 默认 100，但 `nodeCount` 要数全）。
        cost: Cost::Linear,
        since: "M1-S1-D",
    },
    CommandSpec {
        name: "mesh.stats",
        rpc: Some("mesh.stats"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // 现生一个程序化网格再做 AIR：顶点数随分段平方涨，但都有硬上限。
        cost: Cost::Linear,
        since: "M1-S1-D",
    },
    CommandSpec {
        name: "scene.validate",
        rpc: Some("scene.validate"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        reversible: true,
        // 逐节点取网格 + 逐顶点验法线/权重：随「节点数 × 顶点数」涨。
        cost: Cost::Heavy,
        since: "M1-S1-D",
    },
    CommandSpec {
        name: "physics.simulate",
        rpc: Some("physics.simulate"),
        domain: Domain::Meta,
        params: &[],
        returns: "Value",
        // 建的是**宿主侧的临时世界**，步进完只回位移，场景一个字段都不写。
        reversible: true,
        // 步数 × 刚体数；后者随场景规模涨，是这四条里最重的一条。
        cost: Cost::Heavy,
        since: "M1-S1-D",
    },
];
