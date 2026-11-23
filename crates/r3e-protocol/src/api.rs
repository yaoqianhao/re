//! [`EngineApi`]：宿主（`r3e-app`）要实现的引擎操作集合。
//!
//! 68 个方法全部在**主线程**调用，实现方可以直接借用 `Scene` / `Physics` / 脚本宿主；
//! 缺省实现返回「未实现」，宿主可以只点亮当期需要的方法（`METHOD_NOT_FOUND`）。

use serde_json::Value;

use crate::character::{
    CharacterBindQuery, CharacterNameQuery, CharacterStepQuery, CharacterTeleportQuery,
};
use crate::inspect::{MeshStatsQuery, SimulateQuery, SnapshotQuery, ValidateQuery};
use crate::material::MaterialUpsert;
use crate::msg::{EngineInfo, NodeBrief, RpcFailure};
use crate::nav::{NavBakeOptions, NavPathQuery, NavReachQuery};
use crate::patch::*;
use crate::probe::{CastQuery, OverlapQuery, RayQuery};
use crate::query::QuerySpec;
use crate::terrain::{
    TerrainBlocksQuery, TerrainCreateQuery, TerrainGenQuery, TerrainMeshQuery, TerrainNameQuery,
    TerrainResampleQuery, TerrainSampleQuery,
};
use crate::verify::VerifyQuery;

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

    /// 显式装配主世界的刚体（M1-S4-H）：把场景里声明了 `physics` 段的节点绑进
    /// **主世界**，让编辑态的 `physics.*` 查询有可信的世界可查。
    ///
    /// 为什么需要这条口子：装配在运行时是主循环的事（`if !paused` 才每帧 sync），
    /// 编辑态一暂停就永不装配，于是 `physics.raycast` 拿到的是「空世界」——
    /// 回 `hit: false`，与「真的没打中」用同一种回包，这是静默不可信。
    ///
    /// 回包契约：`{ "backend": String, "enabled": true,
    /// "capabilities": [String...], "simulated": bool,
    /// "candidates": usize, "created": usize, "reused": usize, "skipped": usize,
    /// "bound": usize, "bodyCount": usize,
    /// "bindings": [{ "node": String|null, "handle": u64 }...],
    /// "wroteBackScene": false, "stateHash": String, "summary": String }`。
    ///
    /// 三条硬约束：
    /// * **不写回场景**：`Node::body` 一个都不改，`stateHash` 前后必须一致，
    ///   回包显式带 `wroteBackScene: false`；句柄只活在世界的句柄表里。
    /// * **幂等**：连调两次，第二次 `created = 0`、`reused = bound()`（后端句柄表判活）。
    /// * **区分「没有」和「不会」**：物理关着回 `PHYSICS_DISABLED`、宿主没有网格来源
    ///   回 `MESH_SOURCE_UNAVAILABLE`，**绝不回 `bound: 0`** 糊过去。
    ///
    /// 幂等 + 不写回意味着它是「按需对齐」而不是「自动装配」：不想对齐就不调，
    /// 没人会在编辑时偷偷替你建世界。
    fn physics_bind(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("physics.bind"))
    }

    // ---- M2-B01：角色控制器（六条）----
    //
    // 六条的共同口径：
    //
    // 1. **配置真源在场景**：`character.create` / `character.config` 写的是
    //    `Node::character`，不是宿主内部的一份影子配置。写成功即 `stateHash` 变；
    //    宿主缓存的控制器只是它的运行副本。
    // 2. **运行态不写场景**：`step` / `state` / `teleport` 只动宿主侧控制器，
    //    `stateHash` 前后必须一致（脚底位置由宿主主循环同步回 `Transform`，
    //    不在协议臂里做）。
    // 3. **区分「没有」和「不会」**：节点不存在回 `INVALID_PARAMS`-级的具体原因，
    //    节点没配 `character` 段回「未创建控制器」，**绝不回一个全零状态**糊过去
    //    （全零会被读成「角色站在原点」）。

    /// 建立角色控制器（M2-B01）：按「给了的字段 + 缺省值」建，已存在则等价于 `config`。
    ///
    /// 回包契约：`{ "node": String, "created": bool, "spec": { "radius": f32,
    /// "height": f32, "slopeLimitDeg": f32, "stepHeight": f32,
    /// "snapDistance": f32 }, "stateHash": String }`。
    ///
    /// `created: false` 有两种情形（已存在 / 复用），宿主必须带 `spec` 让调用方看得见
    /// 实际生效的参数——「我明明传了 45° 为什么还是上不去」这类问题只能靠回读解释。
    fn character_create(&mut self, _q: CharacterBindQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.create"))
    }

    /// 修改已有控制器配置（M2-B01）：**只改给了的字段**，没给的保持现值。
    ///
    /// 回包契约与 `character.create` 同构；控制器不存在时回 `created: false` 且
    /// `spec` 反映新建后的值（等价于 `create`），不单独报错——脚本里
    /// 「先 config 再 create」和「先 create 再 config」都该工作。
    fn character_config(&mut self, _q: CharacterBindQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.config"))
    }

    /// 推进一个固定步长（M2-B01）。回的是**这一帧的结果**，不是缓存。
    ///
    /// 回包契约：`{ "node": String, "position": [f32;3], "verticalSpeed": f32,
    /// "grounded": bool, "blocked": bool, "stepUp": bool }`；
    /// `position` 是**脚底**。
    ///
    /// 未创建控制器回 `CharacterNotCreated` 类失败（`INVALID_PARAMS`）而不是
    /// 自动建一个：自动建会拿缺省尺寸把角色瞬移到原点，比不生效更糟。
    fn character_step(&mut self, _q: CharacterStepQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.step"))
    }

    /// 读运行态（M2-B01，只读）：字段与 `character.step` 的回包一致。
    fn character_state(&mut self, _q: CharacterNameQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.state"))
    }

    /// 直接落位（M2-B01）：把竖直速度清零并把脚底放到目标点，用于传送 / 复活。
    ///
    /// 回包契约：`{ "node": String, "position": [f32;3], "stateHash": String }`。
    /// 落位后 `grounded` 为 `false`（下一步由求解器裁决），宿主**不得**替它猜。
    fn character_teleport(&mut self, _q: CharacterTeleportQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.teleport"))
    }

    /// 累计统计（M2-B01，只读）：`{ "backend": String, "controllers": u64,
    /// "steps": u64, "blocked": u64, "stepUps": u64 }`。
    ///
    /// `backend` 必须如实带出（`"none"` 表示宿主没接真实现）：没有这一项，
    /// 「统计全是 0」会被读成「角色没动过」，而真相是「压根没接」。
    fn character_stats(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("character.stats"))
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

    /// 三层体检（M1-S4-F）：静态近似 → 物理精确 → 导航可达，合成一份报告。
    ///
    /// 回包契约：`{ "ok": bool, "passed": bool, "counts": { "error": usize,
    /// "warn": usize, "byLayer": { "static": usize, "physics": usize, "nav": usize } },
    /// "issues": [ { "code", "layer", "severity", "category", "path", "target", "msg",
    /// "location", "hint", "approximate" }... ], "truncated": bool,
    /// "unchecked": [ { "layer", "reason" }... ], "layers": [String...],
    /// "stateHash": String }`。
    ///
    /// `ok` / `passed` 的**分工**是这条方法的重点：
    ///
    /// * `ok` = 没有 `Error`（场景自洽）；
    /// * `passed` = `ok` **且** `unchecked` 为空（三层都真的跑过）。
    ///
    /// 少了后半条就成了「绿灯装饰」——物理关闭 / 后端不申报 `shape_cast` /
    /// 导航没烘焙时报告里明明什么都没查，`ok` 却会是 `true`。调用方判门禁请用
    /// `passed`，要看「哪里不对」用 `issues`，要看「哪里没查」用 `unchecked`。
    ///
    /// 能力缺失时**不回空表、也不报 `Error`**：那是「没检查」，落在 `unchecked` 里。
    fn scene_verify(&mut self, _query: VerifyQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.verify"))
    }

    /// 场景结构快照（M1-S1-D）：一份「这场景现在长什么样」的只读总览。
    ///
    /// 这条是插件 `scene_inspect` 的引擎侧承接方：插件原先在进程内遍历 `Scene`
    /// 自己拼 12 个字段，于是必须依赖 `r3e-core`。搬到宿主后插件只剩转发。
    ///
    /// 回包契约：`{ "source": "path"|"scene", "path": String|null,
    /// "nodeCount": usize, "listed": usize, "truncated": bool,
    /// "nodes": [{ "id", "name", "objectIndex", "mesh", "visible", "position",
    /// "rotation", "scale", "material", "body", "tags", "interactRadius" }...],
    /// "environment": {...}, "postFx": {...}, "physics": {...},
    /// "stateHash": String }`。
    ///
    /// `nodeCount` 是**真实节点总数**，`listed` 是截断后实际给出的条数——
    /// 两者不等时 `truncated` 必须为真。用 `listed` 冒充总数会让 AI 以为
    /// 「场景只有 100 个节点」（`maxNodes` 缺省就是 100）。
    ///
    /// `scene` / `path` 双源语义见 [`SnapshotQuery`]；两条都没给回参数错误。
    fn scene_snapshot(&mut self, _query: SnapshotQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.snapshot"))
    }

    /// 程序化网格统计 + AIR 校验（M1-S1-D）。
    ///
    /// 承接插件 `mesh_stats`：那是插件里唯一既依赖 `r3e_core::procedural`
    /// 又依赖 `r3e_core::air` 的工具。
    ///
    /// 回包契约：`{ "shape": String, "params": { ...生效后的形状参数... },
    /// "vertices": usize, "triangles": usize, "indices": usize,
    /// "aabb": { "min": [f32;3], "max": [f32;3] },
    /// "ok": bool, "issues": [{ "kind": String, "detail": String,
    /// "anchor": { "vertex": usize|null, "index": usize|null, "part": usize|null } }...],
    /// "issueCount": usize }`。
    ///
    /// `params` 回的是**夹取之后**的参数：缺省值补了什么、`segments` 从 0 被夹到几，
    /// 调用方要能看见，否则「我明明给了 0」会变成一份和预期不同的网格却查不出原因。
    fn mesh_stats(&mut self, _query: MeshStatsQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("mesh.stats"))
    }

    /// 逐节点网格体检（M1-S1-D）：AIR 结论 + 结构化定位。
    ///
    /// 承接插件 `scene_validate` + `locate`。与 `scene.verify`（M1-S4-F，三层体检）
    /// 的分工：这条只做**网格本身**的结构校验，回答「这份网格能不能用」；
    /// `scene.verify` 回答「这个场景跑起来会不会出问题」。别拿一条顶另一条。
    ///
    /// 回包契约：`{ "source": String, "path": String|null, "nodeCount": usize,
    /// "checked": usize, "ok": bool, "counts": { "error": usize, "warn": usize },
    /// "issues": [{ "kind", "detail", "location": { "node": { "id", "name",
    /// "objectIndex", "visible" }, "field", "path", "resource", "anchors",
    /// "mesh", "material", "hint" } }...], "truncated": bool }`。
    ///
    /// 外部资产网格没有 AIR 结论（本进程没有资产管线）：它们是 `mesh-not-validated`
    /// 而不是通过——`ok` 只统计真的验过的那些，别让「跳过」看起来像「合规」。
    fn scene_validate(&mut self, _query: ValidateQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.validate"))
    }

    /// 跑一段物理模拟（M1-S1-D）：在**临时世界**里步进，回报位移与事件。
    ///
    /// 承接插件 `physics_simulate`。这是四条里唯一会建物理世界的，但**仍然只读场景**：
    /// 世界是宿主临时建的，步进完只回位移，`Node` / `Scene` 一个字段都不写回。
    ///
    /// 回包契约：`{ "backend": String, "steps": u32, "dt": f32,
    /// "gravity": [f32;3], "ground": f32|null, "bodyCount": usize,
    /// "listed": usize, "truncated": bool,
    /// "bodies": [{ "node", "start": [f32;3], "end": [f32;3],
    /// "delta": [f32;3], "moved": bool }...], "events": usize,
    /// "summary": String }`。
    ///
    /// `backend` 回的是**真的用了哪个**：`rapier` 是可选 feature，没编进来就必须报
    /// 参数错误（`unchecked` 不冒充通过同理），绝不能静默退回内置后端——
    /// 「我要用 rapier 复现」变成「你其实用的是内置后端」，结论全错且无人察觉。
    fn physics_simulate(&mut self, _query: SimulateQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("physics.simulate"))
    }

    // ── 地形命令面（M2-C06）───────────────────────────────────────────────
    //
    // 八条命令的共同边界，先读这三条再看各方法：
    //
    // 1. **真值只有 `Scene::terrain`**。地形网格是算出来的视图（CD-2），
    //    所以 `resample` 改的是高度样本、`mesh` 算的是临时网格，没有任何一条命令
    //    把网格写进场景。写进去就等于给了两套真相。
    // 2. **「没有这块地形」和「这块地形是平的」必须分开**：前者回
    //    `TERRAIN_PATCH_MISSING`，后者回 `height: 0`。
    // 3. **写场景的命令走 `CommandKind`**（可撤销、进 `state_hash`、走事务），
    //    只读的命令不碰撤销栈。

    /// 建 / 改一块地形（M2-C06）：同名 = 整体覆盖，超上限只拦新增。
    ///
    /// 回包契约：`{ "name": String, "created": bool, "patch": { "name", "origin",
    /// "size", "resolution", "albedo", "roughness", "samples": usize }, "stateHash": String }`。
    ///
    /// `patch.samples` 是高度样本数（`resolution.0 * resolution.1`），不是样本本身——
    /// 128 × 128 的块有 16384 个浮点，回包里搬它没有意义，调用方要的是「写进去了几个」。
    fn terrain_create(&mut self, _q: TerrainCreateQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.create"))
    }

    /// 删一块地形（M2-C06）：按名字删，删不掉说明名字不对。
    ///
    /// 回包契约：`{ "name": String, "removed": bool, "remaining": usize,
    /// "stateHash": String }`。名字不存在**不报错**而是 `removed: false`——
    /// 删除天然幂等，脚本重跑一遍不该红。
    fn terrain_remove(&mut self, _q: TerrainNameQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.remove"))
    }

    /// 采样某个世界点的高度（M2-C06，只读）。
    ///
    /// 回包契约：`{ "name": String, "x": f32, "z": f32, "height": f32,
    /// "normal": [f32;3], "inside": bool }`。
    ///
    /// 给了 `name` 只算那一块（点落在块外回 `inside: false` + `TERRAIN_PATCH_MISSING`）；
    /// 不给 `name` 则跨全部块取最高层（多块重叠时的既定裁决）。
    fn terrain_sample(&mut self, _q: TerrainSampleQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.sample"))
    }

    /// 分块视图（M2-C06，只读）：按 `blockSize` 把地形矩形切成块，回块清单与块数。
    ///
    /// 回包契约：`{ "rect": [f32;4], "blockSize": f32, "dims": [u32;2],
    /// "count": u32, "covered": u32, "blocks": [{ "index": [u32;2], "rect": [f32;4],
    /// "patches": [String] }...] }`。
    ///
    /// `covered` 是有地形覆盖的块数（`count` 含空白块）。两个数都给，是因为
    /// 「地形 4×4 块、只铺了 9 块」和「4×4 块、铺满 16 块」在后续装配里差别很大。
    fn terrain_blocks(&mut self, _q: TerrainBlocksQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.blocks"))
    }

    /// 按新档位重采样高度网格并写回（M2-C06，写场景、可撤销）。
    ///
    /// 回包契约：`{ "name": String, "from": [u32;2], "to": [u32;2],
    /// "samples": usize, "maxError": f32, "stateHash": String }`。
    ///
    /// `maxError` 是重采样前后在同一批世界点上取样的最大高度差（米）。带上它是因为
    /// 「降到 1/4 分辨率」听起来无损、实际会有肉眼可见的地形走形，不给这个数就没人
    /// 知道代价是多少。`to` 可能小于请求值——那是被 `maxResolution` / 后端的硬上限夹过。
    fn terrain_resample(&mut self, _q: TerrainResampleQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.resample"))
    }

    /// 程序化生成一整片地形（M2-C06，写场景、可撤销）。
    ///
    /// 回包契约：`{ "seed": u64, "created": usize, "names": [String],
    /// "stateHash": String }`。
    ///
    /// 生成是**确定性的**：同一个 `seed` 与尺寸必须产出逐位相同的样本，
    /// 否则回放金标与 `state_hash` 全部失效。
    fn terrain_gen(&mut self, _q: TerrainGenQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.gen"))
    }

    /// 地形汇总（M2-C06，只读）：`{ "patches": usize, "samples": usize,
    /// "blocks": u32, "covered": u32, "triangles": usize, "vertices": usize,
    /// "lod": u32, "blockSize": f32 }`。
    ///
    /// `triangles` / `vertices` 与 `terrain.mesh` 同口径（全块、`lod` 缺省 1），
    /// 这样「逐块取网格再求和」必须等于这里的数——两处算法一旦分叉，求和就对不上，
    /// 测试会立刻红。
    fn terrain_stats(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.stats"))
    }

    /// 取某一块（或缺省全部块）的派生网格统计（M2-C06，只读）。
    ///
    /// 回包契约：`{ "lod": u32, "block": [u32;2]|null, "blocks": u32,
    /// "vertices": usize, "triangles": usize, "bounds": [f32;6]|null }`。
    ///
    /// 只回**统计**不回顶点数组：一块 128 × 128 的网格有十几万浮点，搬过来对
    /// 调用方（多半是想确认「这块地形到底有没有生成出来」）没有用处，还会把 RPC
    /// 回包撑爆。真要顶点应该走渲染侧，不该走协议。
    fn terrain_mesh(&mut self, _q: TerrainMeshQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("terrain.mesh"))
    }
}
