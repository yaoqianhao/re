//! [`EngineApi`]：宿主（`r3e-app`）要实现的引擎操作集合。
//!
//! 39 个方法全部在**主线程**调用，实现方可以直接借用 `Scene` / `Physics` / 脚本宿主；
//! 缺省实现返回「未实现」，宿主可以只点亮当期需要的方法（`METHOD_NOT_FOUND`）。

use serde_json::Value;

use crate::msg::{EngineInfo, NodeBrief, RpcFailure};
use crate::patch::*;

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
}
