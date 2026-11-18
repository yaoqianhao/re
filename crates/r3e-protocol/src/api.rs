//! [`EngineApi`]：宿主（`r3e-app`）要实现的引擎操作集合。
//!
//! 56 个方法全部在**主线程**调用，实现方可以直接借用 `Scene` / `Physics` / 脚本宿主；
//! 缺省实现返回「未实现」，宿主可以只点亮当期需要的方法（`METHOD_NOT_FOUND`）。

use serde_json::Value;

use crate::material::MaterialUpsert;
use crate::msg::{EngineInfo, NodeBrief, RpcFailure};
use crate::nav::{NavBakeOptions, NavPathQuery, NavReachQuery};
use crate::patch::*;
use crate::probe::{CastQuery, OverlapQuery, RayQuery};
use crate::query::QuerySpec;

/// 宿主（`r3e-app`）要实现的引擎操作集合。
///
/// 全部方法都在**主线程**调用；实现方可以直接借用 `Scene` / `Physics` / 脚本宿主。
/// 缺省实现返回「未实现」，这样宿主可以只点亮本期需要的方法。
pub trait EngineApi {
    fn info(&mut self) -> EngineInfo;

    fn scene_list(
        &mut self,
        _filter: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<NodeBrief>, RpcFailure> {
        Err(RpcFailure::unsupported("scene.list"))
    }

    fn scene_get(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.get"))
    }

    fn scene_set_transform(
        &mut self,
        _node: &str,
        _patch: TransformPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_transform"))
    }

    fn scene_spawn(&mut self, _object: Value) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.spawn"))
    }

    /// 改写物件的 PBR 材质（`metallic` / `roughness` / `emissive` 可分别给）。
    /// 宿主应把补丁合并进现有材质后立即收敛，并把合并结果回给调用方。
    fn scene_set_material(
        &mut self,
        _node: &str,
        _patch: MaterialPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_material"))
    }

    /// 改写场景级环境探针（三色天空 / 太阳 / 总强度）。
    /// 宿主应把补丁合并进现有环境后立即收敛，并把合并结果回给调用方；
    /// `enabled = false` 时清空环境（回到常数 `ambient`），返回值里 `environment` 为 `null`。
    fn scene_set_environment(&mut self, _patch: EnvironmentPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_environment"))
    }

    /// 改写场景级 HDR 立方体反射探针（扩展二十三：`equirect` 一张图 / `faces` 六面图）。
    ///
    /// 宿主应把补丁合并进现有 `environment_map` 段后立即收敛，并把合并结果回给调用方；
    /// 返回值契约：`{ "enabled": bool, "source": "equirect"|"faces"|"disabled"|"none",
    /// "environment_map": {...}|null }`。`source` 是**收敛后实际生效**的来源标签，
    /// 前端据此判断面板该显示「已加载」还是「配置无效退回解析 IBL」。
    ///
    /// 与 `scene_set_environment` 的区别：那条改三色天空 / SH 解析 IBL，这条改预滤波
    /// cubemap；关掉这条（`enabled = false`）不会影响解析 IBL，反之亦然。
    fn scene_set_environment_map(
        &mut self,
        _patch: EnvironmentMapPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_environment_map"))
    }

    /// 改写场景级 HDR 后处理段（曝光 / 色调映射 / 泛光 / FXAA）。
    /// 宿主应把补丁合并进现有设置后立即收敛，并把合并结果回给调用方；
    /// `enabled = false` 时清空该段（直通），返回值里 `postfx` 为 `null`。
    fn scene_set_postfx(&mut self, _patch: PostFxPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_postfx"))
    }

    /// 改写后处理段下的屏幕空间环境光遮蔽（扩展十七）。
    /// 宿主应把补丁合并进现有设置后立即收敛，并把合并结果回给调用方；
    /// 与 `scene_set_postfx` 的区别：`enabled = false` 只掐遮蔽这一条支线，
    /// 不清空整段后处理（曝光 / 泛光 / FXAA 照旧生效）。
    fn scene_set_ssao(&mut self, _patch: SsaoPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_ssao"))
    }

    fn scene_remove(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.remove"))
    }

    /// 把内存中的当前场景写回磁盘（`path` 为 `None` 时由宿主决定默认路径，
    /// 例如「写回本场景的原始文件」）。实现方应返回落盘后的绝对路径与物件数量。
    fn scene_save(&mut self, _path: Option<&str>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.save"))
    }

    /// 读一帧的运行时统计（P0-4）：帧时间分位 + 绘制统计 + 场景规模。
    ///
    /// 返回值契约（宿主必须给全这些键，缺项会让上游的回归基线无从比对）：
    /// `{ "frame": { "index", "fps", "frame_ms": {"p50","p95","p99","max","samples"} },
    ///    "render": { "draw_calls", "instances", "culled", "cull_unknown",
    ///                "shadow_draws", "shadow_cascades", "ssao", "local_lights",
    ///                "ui_draws", "text_draws" },
    ///    "scene": { "objects", "meshes", "materials", "animations", "scripts" },
    ///    "size": { "width", "height" } }`
    ///
    /// 全是只读采样：调用本方法不改场景、不推进时间。
    fn engine_stats(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("engine.stats"))
    }

    /// 离屏渲染一帧并落盘成图片（P0-4），返回 `{ "path", "width", "height", "bytes", "format" }`。
    ///
    /// `path` 为 `None` 时由宿主决定默认落盘位置（通常是工作目录下的 captures/）。
    /// 与截图不同，这里**会推进一帧**（阴影 / 后处理历史前进一格），
    /// 因此上游不该在逐帧动画的关键帧之间插调用。
    fn capture_frame(&mut self, _path: Option<&str>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("capture.frame"))
    }

    /// 列出动画混合树层（扩展十八 A）：`{ count, limit, layers: [...] }`。
    ///
    /// `instance` 为 `None` 时列出全部动画实例的层；给了名字但实例不存在，
    /// 宿主应报参数错误（编辑器拼错实例名时，回空列表比回错误更难排查）。
    fn scene_anim_layers(
        &mut self,
        _instance: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.anim_layers"))
    }

    /// 改动一个混合树层（权重 / 模式 / 遮罩 / 速度 / 片段），返回收敛后的层对象。
    ///
    /// 编辑器侧改动一律**立即生效**，不做过渡——过渡是玩法逻辑，属于脚本。
    fn scene_anim_layer_set(&mut self, _patch: AnimLayerPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.anim_layer_set"))
    }

    /// 列出场景栈（扩展十八 B）：`{ count, current, max, entries: [...] }`。
    ///
    /// 每层只给**声明态**（层号 / 名字 / 来源路径 / 源文本字节数 / 帧号与时间），
    /// 不给场景内容本身——面板刷新是每帧都在发的请求，把十几层的场景 JSON 一起搬
    /// 过去会让序列化变成瓶颈。层的内容只在 `scene.back` 时由宿主装载。
    fn scene_stack(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.stack"))
    }

    /// 切到一个场景文件：宿主把当前场景压栈，再把目标场景装进**同一个**场景真源。
    ///
    /// 这条路必须由宿主走完（换资产库、重建动画实例、重跑脚本、刷新渲染层缓存），
    /// 编辑器只发路径，不关心顺序。
    fn scene_switch(&mut self, _path: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.switch"))
    }

    /// 回到栈里的上一层（不删层，可再用前进语义回来）。
    fn scene_back(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.back"))
    }

    /// 存一份带运行时快照的存档（`path` 为 `None` 时由宿主决定默认路径）。
    ///
    /// 与 `scene_save` 的区别：`scene_save` 只写场景**声明**（重开即源状态），
    /// 存档还要把"这一局跑到哪"（时间 / 帧号 / 通道开关 / 动画播放进度）一起装进去。
    fn scene_archive_save(&mut self, _path: Option<&str>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.archive.save"))
    }

    /// 读档：按存档重建当前场景并恢复运行时状态。
    ///
    /// 读档前宿主应把当前场景压栈，因此「读档 → 反悔」等价于一次 `scene.back`。
    fn scene_archive_load(&mut self, _path: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.archive.load"))
    }

    /// 列出当前 2D UI 元素（扩展十四）。
    ///
    /// 返回的是**声明**（单位串、层级、可见性），不是换算后的像素矩形——像素要屏幕
    /// 尺寸才能算，而屏幕尺寸属于宿主状态，编辑器要显示实际矩形时应自己带上视口大小。
    fn ui_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.list"))
    }

    /// 写入 / 局部更新一个 UI 元素（同 key 覆盖）。
    fn ui_set(&mut self, _patch: UiPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.set"))
    }

    /// 按 key 删除一个 UI 元素。
    fn ui_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.remove"))
    }

    /// 读当前指针交互状态（hover / active / 本帧事件），供编辑器显示与联调。
    fn ui_state(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.state"))
    }

    /// 列出当前局部光源（扩展十五）：`{ enabled, count, lights: [...] }`。
    ///
    /// 每盏灯给出**声明态**（kind / pos / dir / color / intensity / range / 内外角），
    /// 不含衰减后的辐亮度——那取决于查询点，属于 GPU / 脚本的认知范围。
    fn light_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.list"))
    }

    /// 写入 / 局部更新一盏局部光源（同 key 覆盖）。
    fn light_set(&mut self, _patch: LightPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.set"))
    }

    /// 按 key 删除一盏局部光源。
    fn light_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.remove"))
    }

    /// 清空全部局部光源，返回删掉的盏数。
    fn light_clear(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.clear"))
    }

    /// 读 / 写局部光照总开关（`None` 表示只读）。
    ///
    /// 关掉只是不上传（场景数据保留），因此编辑器的「取消勾选」不会丢灯。
    fn light_set_enabled(&mut self, _enabled: Option<bool>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.set_enabled"))
    }

    /// 列出已注册的 3D 空间化声源（§12 第十项）：
    /// `{ enabled, count, limit, max, supports_speed, listener, sources: [...] }`。
    ///
    /// 每个声源同时给**声明态**与**上一帧结算结果**：`position` / `node` /
    /// `model` / `minDistance` / `maxDistance` / `doppler` 是编辑器能改的，
    /// `gain` / `rate` / `distance` 是宿主每帧推进出来的——面板要显示「这个声源
    /// 现在多响、倍速多少」，编辑器自己算不出来，所以必须由宿主回传。
    fn audio_spatial_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.list"))
    }

    /// 注册 / 局部更新一个空间化声源（同 key 覆盖），返回收敛后的声源对象。
    ///
    /// 注册与摆位是**同一个方法**：编辑器新建声源时一次把位置与衰减参数发全，
    /// 免得先 attach 再 setPosition 发两趟。超上限时宿主应报参数错误而不是静默丢。
    fn audio_spatial_set(&mut self, _patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.set"))
    }

    /// 解除一个声源的注册（正在播的声音继续播完，只是不再做空间化）。
    fn audio_spatial_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.remove"))
    }

    /// 清空全部空间化声源，返回解除的条数。
    fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.clear"))
    }

    /// 读 / 写听者位姿（`None` = 只读）。
    ///
    /// 正常运行时听者由「当前活动相机」每帧推导，编辑器手改只在暂停调试时有用；
    /// 因此这里的写入是**一次性**的，下一帧会被 `app` 的相机推导覆盖。
    fn audio_spatial_listener(
        &mut self,
        _patch: Option<ListenerPatch>,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.listener"))
    }

    /// 读 / 写空间化总开关（关掉 = 全部声源退回平面声，声源数据保留）。
    fn audio_spatial_set_enabled(&mut self, _enabled: Option<bool>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.set_enabled"))
    }

    /// 读 / 写方向光阴影设置（扩展十六）：`None` = 只读，`Some(patch)` = 合并后回写。
    ///
    /// 回包固定给出收敛后的全部字段（含 `cascades` 与 `distance`），
    /// 编辑器据此刷新面板，不必再补一次读。
    fn scene_shadow(&mut self, _patch: Option<ShadowPatch>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.shadow"))
    }

    fn script_eval(&mut self, _source: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.eval"))
    }

    fn script_reload(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.reload"))
    }

    /// 开一笔事务，返回事务句柄（M1-S1）。
    ///
    /// 回包契约：`{ "txId": String, "baseHash": u64, "version": u64, "staged": 0 }`。
    /// `baseHash` 是开事务那一刻的内容哈希，**`commit` 前任何操作都不许改它**——上层用
    /// 这一条断言「`preview` / `apply` 真的没动场景」。
    ///
    /// 同一时刻只允许一笔未决事务：已有未决事务时宿主应回 `TX_BUSY`
    /// （错误码见 crate 根的 `TX_*` 常量），而不是排队或隐式提交上一笔。
    fn tx_begin(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.begin"))
    }

    /// 把一条**意图**暂存进事务。
    ///
    /// `op` 是白名单内的点分方法名（如 `scene.settransform`），`args` 是它的具名参数字典
    /// （缺省 `null`）。宿主负责把「方法名 + 参数」翻译成 `r3e-doc` 的命令值对象——前像
    /// 必须在执行当下的 `&mut Document` 里就地抓，不能让远端伪造（见 `dispatch/tx.rs` 头注释）。
    ///
    /// 回包契约：`{ "txId", "staged": usize, "op": String }`。暂存不落场景、不推版本。
    fn tx_apply(&mut self, _tx_id: &str, _op: &str, _args: &Value) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.apply"))
    }

    /// 把一条**意图**暂存进事务（M1-S3-A）：一次意图展开成 N 条命令，一起入栈。
    ///
    /// 与 `tx.apply` 的分工：`tx.apply` 收的是**裸命令**（一个 `op` 一条命令），
    /// 供 M1-S1 的原子写用；本方法收的是**意图**（`spawn_prefab` / `place_light` /
    /// `paint_terrain`），一个 `intent` 在宿主侧展开成多条命令，记账时占撤销栈一格，
    /// 因此「一条意图 = 一次 undo 全退」。
    ///
    /// 回包契约：`{ "txId", "intent": String, "commands": usize, "staged": usize,
    /// "intents": usize }`。`commands` 是本条意图展开的命令数，`staged` 是事务内命令
    /// 总数，`intents` 是事务内步数。
    ///
    /// 失败语义：意图内某条命令失败时，本意图已应用的部分会逆序回退，**事务保留**
    /// 供调用方改参数重试（裸命令的 `tx.apply` 失败则整笔作废）——见
    /// `r3e_doc::Document::tx_apply_intent` 的注释。
    fn tx_intent(
        &mut self,
        _tx_id: &str,
        _intent: &str,
        _args: &Value,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.apply_intent"))
    }

    /// 影子预演：在**不改场景**的前提下算出提交后的哈希与受影响对象。
    ///
    /// 回包契约：`{ "txId", "baseHash": u64, "previewHash": u64, "staged": usize,
    /// "changed": [{ "kind": String, "target": String|null }] }`。
    /// `baseHash` 与 `tx.begin` 返回值必须一致，否则说明有东西在事务外改了文档。
    fn tx_preview(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.preview"))
    }

    /// 提交：n 条命令作为**一笔**入撤销栈，版本只 +1，一次 `undo` 整笔退回。
    ///
    /// 回包契约：`{ "txId", "committed": usize, "hash": u64, "version": u64, "undoDepth": usize }`。
    /// 任一条命令失败则整笔不落（原子性），返回错误且事务仍保持未决——由调用方决定
    /// `rollback` 还是修完重试。
    fn tx_commit(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.commit"))
    }

    /// 回滚：丢弃暂存，场景回到 `tx.begin` 时的基哈希。
    ///
    /// 回包契约：`{ "txId", "undone": usize, "hash": u64, "version": u64 }`。
    /// 由于暂存期场景本就没被改动，`undone` 通常是 0；非 0 只有在宿主把暂存实现成
    /// 「真应用 + 影子记录」时才可能出现。
    fn tx_rollback(&mut self, _tx_id: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("tx.rollback"))
    }

    /// 语义查询（M1-S2-B）：按类别 / 标签 / 区域 / 组件位过滤，返回五字段投影。
    ///
    /// 回包契约：`{ "count": usize, "limit": usize, "truncated": bool,
    /// "unfiltered": bool, "nodes": [NodeView...] }`。
    ///
    /// `limit` 缺省由宿主填 `r3e_core::query::DEFAULT_LIMIT`，`limit: 0` 表示不限；
    /// `truncated` 说明「还有更多命中的没返回」，AI 该接着问而不是以为这就是全部。
    /// 本方法**只读**：不建事务、不推版本，`StateHash` 前后必须一致。
    fn query_find(&mut self, _spec: QuerySpec) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("query.find"))
    }

    /// 单节点语义投影（M1-S2-B）：五字段 + 命中的组件位与类别。
    ///
    /// 回包契约：`{ "node": NodeView, "components": [String...], "kinds": [String...] }`。
    /// 节点不存在回 `NOT_FOUND`（不是空对象）——「查不到」和「查到了但啥都没有」
    /// 对 AI 是两种完全不同的下一步。
    fn query_describe(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("query.describe"))
    }

    /// 场景普查（M1-S2-B）：总量与分布，用来「先看全貌再决定查什么」。
    ///
    /// 回包契约：`{ "nodes", "visible", "interactable", "tagged",
    /// "byKind": {String: usize}, "tags": {String: usize} }`。
    fn query_stats(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("query.stats"))
    }

    // ---- M1-S4-D：物理探测 / 导航查询 / 场景材质表（八条）----
    //
    // 这八条的共同口径（三条铁律，任何实现都不得违反）：
    //
    // 1. **能力位随回包走**：`physics.*` 三条必须在回包里带 `capabilities` 数组，
    //    调用方不必先问一次「你支持不支持」；空结果 + 能力位才解释得清楚。
    // 2. **区分「没有」和「不会」**：物理关着回 `PHYSICS_DISABLED`、后端不申报该位回
    //    `BACKEND_UNSUPPORTED`、导航没烘回 `NAV_NOT_BAKED`。**绝不回空表**——
    //    空表会被 AI 读成「场景里真的没有东西」，从而去改本来就对的场景。
    // 3. **只读不写**：八条里只有 `scene.material.upsert` 写场景，其余七条
    //    `StateHash` 前后必须一致；`nav.bake` 的产物挂在宿主侧，不写节点。

    /// 射线查询（M1-S4-D）：从一点朝一个方向打射线，拿最近命中体。
    ///
    /// 回包契约：`{ "hit": bool, "distance": f32, "point": [f32;3], "normal": [f32;3],
    /// "node": String|null, "handle": u64|null, "backend": String,
    /// "capabilities": [String...] }`。没打到时 `hit: false`，其余字段缺省。
    ///
    /// `maxDistance` 缺省由宿主填；方向由宿主归一化，零方向判参数错误。
    fn physics_raycast(&mut self, _q: RayQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("physics.raycast"))
    }

    /// 球体重叠查询（M1-S4-D）：给球心与半径，拿覆盖范围内的刚体。
    ///
    /// 回包契约：`{ "count": usize, "limit": usize, "truncated": bool,
    /// "hits": [{ "node": String, "handle": u64, "distance": f32 }...],
    /// "backend": String, "capabilities": [String...] }`。
    fn physics_overlap(&mut self, _q: OverlapQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("physics.overlap"))
    }

    /// 扫掠查询（M1-S4-D）：把球 / 盒沿方向推过去，拿最近一次接触。
    ///
    /// 回包契约与 `physics.raycast` 同构（`hit` / `distance` / `point` / `normal` /
    /// `node` / `handle` / `backend` / `capabilities`）。
    fn physics_cast(&mut self, _q: CastQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("physics.cast"))
    }

    /// 烘焙导航网格（M1-S4-D）。产物挂在宿主侧，**不写场景**。
    ///
    /// 回包契约：`{ "baked": bool, "dims": [u32;2], "cellSize": f32, "cells": usize,
    /// "walkable": usize, "walkableRatio": f32, "regions": u32,
    /// "regionSizes": [u32...], "spawns": [{ "name": String, "region": u32 }...],
    /// "unresolved": [String...], "floatingSpawns": [String...] }`。
    ///
    /// `unresolved` / `floatingSpawns` 非空必须如实带出：它们是「这份导航数据
    /// 不可信」的直接证据，藏起来会让 AI 基于错网格报「路走不通」。
    fn nav_bake(&mut self, _opts: NavBakeOptions) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.bake"))
    }

    /// 寻路（M1-S4-D）：A* 求一条可走路径。
    ///
    /// 回包契约：`{ "found": bool, "outcome": String("found"|"unreachable"|
    /// "invalidEndpoint"|"budgetExceeded"), "reason": String（人话）,
    /// "points": [[f32;3]...], "count": usize }`。
    ///
    /// 四种结局都要如实区分：`unreachable`（分属不同连通块，几何上走不到）与
    /// `budgetExceeded`（算不完，结论未知）对调用方是完全不同的两个下一步。
    fn nav_path(&mut self, _q: NavPathQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.path"))
    }

    /// 可达性（M1-S4-D）：两点是否在同一连通块，断连时给断点。
    ///
    /// 回包契约：`{ "reachable": bool, "reason": String, "fromRegion": u32,
    /// "toRegion": u32, "disconnectPoint": [f32;3]|null }`。
    fn nav_reachability(&mut self, _q: NavReachQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.reachability"))
    }

    /// 列场景材质表（M1-S4-D）。
    ///
    /// 回包契约：`{ "count": usize, "materials": [{ "name", "metallic", "roughness",
    /// "emissive": [f32;3], "textures": {String: String}, "uvScale": [f32;2],
    /// "users": [String...] }...] }`。
    ///
    /// `users` 是按 `materialRef` 引用这份材质的物件名——「这份材质有没有人用」
    /// 决定了能不能删，比让 AI 自己反查全部节点省一圈。
    fn material_list(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.material.list"))
    }

    /// 写入 / 覆盖一行场景材质（M1-S4-D）。同名整体覆盖、顺序不变，上限只拦新增。
    ///
    /// 回包契约：`{ "name": String, "created": bool, "count": usize,
    /// "material": { ...与 material.list 单项同构... } }`。
    ///
    /// 名字收敛后为空回参数错误；表满且是新名字回 `ENGINE_ERROR`（不是静默失败）。
    fn material_upsert(&mut self, _decl: MaterialUpsert) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.material.upsert"))
    }
}
