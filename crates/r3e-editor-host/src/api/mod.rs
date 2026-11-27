//! 编辑器桥接线：在 `r3e-app` 里实现 [`EngineApi`]，把 RPC 请求落到主线程持有的
//! `Scene` / `Physics` / `ScriptHost` 上。
//!
//! 这里是 `r3e-editor` 的**唯一宿主实现**。`r3e-editor` 只认 JSON 值与
//! [`EngineApi`] 抽象，不认识 `NodeId` / `Vec3`；`r3e-app` 负责把两者对上：
//!
//! ```text
//! 编辑器 --TCP--> r3e-editor(传输层) --Job--> 主线程 LocalEngineApi --> Scene/Physics/ScriptHost
//! ```
//!
//! [`LocalEngineApi`] 借用宿主的三块状态，一次请求构造一次、用完即丢，
//! 因此生命周期与 `Job::run` 严格对齐：RPC 线程永远拿不到引擎内部引用。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use glam::{Quat, Vec3};
use r3e_audio::{Audio, DistanceModel, MAX_SPATIAL_SOURCES};
use r3e_core::host::CharacterHost;
use r3e_core::light::MAX_LOCAL_LIGHTS;
use r3e_core::postfx::{PostFxSettings, TonemapMode};
use r3e_core::scene_stack::{SceneArchive, SceneCommand, ARCHIVE_FORMAT, ARCHIVE_VERSION};
use r3e_core::ssao::{SsaoSettings, MAX_SSAO_SAMPLES, MIN_SSAO_SAMPLES};
use r3e_core::ui::{self, Anchor, UiElement, UiKind, UiUpsert, UiValue};
use r3e_core::ui_text;
use r3e_core::Aabb;
use r3e_core::{AnimLayer, BoneMask, LayerMode, LayerSync, MeshProvider, MAX_ANIM_LAYERS};
use r3e_core::{LocalLight, LocalLightKind, Node, ObjectDesc, Scene};
use r3e_core::{ShadowSettings, MAX_CASCADES, MAX_SCENE_STACK};
use r3e_doc::Document;
use r3e_editor::{
    AnimLayerPatch, AudioSpatialPatch, CastQuery, CharacterBindQuery, CharacterNameQuery,
    CharacterStepQuery, CharacterTeleportQuery, EngineApi, EngineInfo, EnvironmentMapPatch,
    EnvironmentPatch, LightPatch, ListenerPatch, MaterialPatch, MaterialUpsert, MeshStatsQuery,
    NavApi, NavBakeOptions, NavFileTarget, NavNearestQuery, NavPathQuery, NavPointQuery,
    NavReachQuery, NavSmoothQuery, NodeBrief, OverlapQuery, PhysicsInfo, PostFxPatch, QuerySpec,
    RayQuery, RpcFailure, ShadowPatch, SimulateQuery, SnapshotQuery, SsaoPatch, TerrainBlocksQuery,
    TerrainCreateQuery, TerrainGenQuery, TerrainMeshQuery, TerrainNameQuery, TerrainResampleQuery,
    TerrainSampleQuery, TransformPatch, UiPatch, ValidateQuery, VerifyQuery, PROTOCOL_VERSION,
    SERVER_BUSY,
};
use r3e_editor::{PrefabApi, PrefabInfoRequest, PrefabSpawnRequest};
use r3e_nav::Nav;
use r3e_physics::Physics;
use r3e_render::Renderer;
use r3e_script::ScriptHost;
use serde_json::{json, Value};

/// 引擎信息里的引擎名（与 Cargo 包名、窗口标题保持一致）。
const ENGINE_NAME: &str = "r3e";

use r3e_runtime::capture::{absolute_path, capture_to_file, default_capture_name};
use r3e_runtime::time::FrameTimeStats;

/// 主线程上的 [`EngineApi`] 实现。
///
/// 所有方法都在事件循环线程调用，所以可以直接 `borrow_mut` 场景与物理，
/// 既不需要锁，也不会出现「RPC 线程改场景、渲染线程正在读」的撕裂。
pub struct LocalEngineApi<'a> {
    pub scene: &'a Rc<RefCell<Document>>,
    pub physics: &'a Rc<RefCell<Physics>>,
    /// 混音器真句柄（M2-A03b）：`ScriptHost` 只向脚本暴露能力面，
    /// 音频面板要的细节查询（`spatial_settings` / `listener` / `attach_spatial`）
    /// 走这条直通借用，仍是 app 那一个 `Audio`。
    pub audio: &'a Rc<RefCell<Audio>>,
    /// 导航网格槽位：`nav.bake` 往里写，`nav.path` / `nav.reachability` 往前读。
    ///
    /// 为什么由宿主槽位持有而不是挂进场景：导航网格是**烘焙产物**，不是场景数据。
    /// 写进 `Document` 会让它参与 `state_hash`，于是「烘一次导航」看起来像「改了场景」，
    /// 撤销栈与脏标记全部失真。
    ///
    /// `None` = 该宿主压根没接导航槽位（headless 测试）：三条 `nav.*` 一律回
    /// `NAV_NOT_BAKED`，绝不回空表假装「烘过了但一格都没有」。
    pub nav: Option<&'a RefCell<Option<Nav>>>,
    /// 角色控制器能力面（M2-B01）：由装配层注入 `CharacterRuntime` 后的
    /// `Rc<dyn CharacterHost>`；headless 宿主为 `None`，六条 `character.*`
    /// 一律回 `CHARACTER_NOT_READY`，绝不回全零运行态假装「角色就站在原点」。
    ///
    /// 与控制器的运行态一样，它**不是场景数据**：拿的是 `&Rc`，不参与 `state_hash`。
    pub character: Option<&'a Rc<dyn CharacterHost>>,
    pub scripts: &'a mut ScriptHost,
    /// 网格来源（M1-S4-G）：影子场景复核要**装配**物理世界，装配就得有形状来源。
    ///
    /// 与 `nav` 槽位一样，`None` 是合法状态（headless 测试没有资产库）：此时
    /// 物理层只能给出纯解析的穿模结论，悬空一律记 `unchecked`，绝不用包围盒
    /// 顶替真实形状去下结论。
    pub meshes: Option<Arc<dyn MeshProvider>>,
    /// 渲染器：`engine.stats` 读它的 `RenderStats`，`capture.frame` 用它离屏出图。
    /// 窗口 / GPU 未就绪时为 `None`，此时两个方法都明确回报「不可用」而不是编数字。
    pub renderer: Option<&'a mut Renderer>,
    /// 最近的帧时间分布（由主循环的 [`FrameTimeWindow`] 算好后传进来）。
    pub frame_ms: FrameTimeStats,
    /// 截图默认落盘目录（`capture.frame` 不传 path 时用）。
    pub capture_dir: PathBuf,
    /// 场景文件路径（展示用；空串时退回场景自身的名字）。
    pub scene_label: String,
    /// 脚本文件路径（展示用）。
    pub script_label: String,
    pub frames: u64,
    pub fps: f32,
    pub uptime_ms: u64,
    pub paused: bool,
}

// ---- M0-S2：按功能域拆分的子模块（原 3553 行单体 impl 已全部下沉） ----
mod anim;
mod audio;
mod character;
mod env;
mod info;
mod inspect;
mod light;
mod material;
mod nav;
mod physics;
mod postfx;
mod prefab;
mod query;
mod scene;
mod scene_flow;
mod script;
mod shadow;
mod support;
mod terrain;
mod tx;
mod tx_violations;
mod ui_rpc;
mod verify;

/// 本地（非 rpc）的模板建库入口（M2-E04）。
///
/// `prefab_define` 在 `P5` = B 的口径下**不进 `RPC_METHODS`**，只走脚本或宿主本地
/// 调用；这里把它导到 `api` 层，装配层（`r3e-app` 载入 `assets/prefab/` 时）与集成
/// 测试（`tests/prefab_api.rs`）才有正当入口，不必为了调一次建库去伪造 `EngineApi`。
/// 另外四条无 rpc 的 prefab 命令（`spawn_many` / `set_override` / `clear_override`）
/// 仍保持 `pub(super)`：它们的正当调用方是脚本侧，宿主侧留口仅供 `R7` 触发时补 rpc。
pub use prefab::prefab_define;

impl EngineApi for LocalEngineApi<'_> {
    fn engine_stats(&mut self) -> Result<Value, RpcFailure> {
        info::engine_stats(self)
    }
    fn capture_frame(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        info::capture_frame(self, path)
    }
    fn info(&mut self) -> EngineInfo {
        info::info(self)
    }
    fn scene_list(
        &mut self,
        filter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<NodeBrief>, RpcFailure> {
        scene::scene_list(self, filter, limit)
    }
    fn scene_get(&mut self, node: &str) -> Result<Value, RpcFailure> {
        scene::scene_get(self, node)
    }
    fn scene_set_transform(
        &mut self,
        node: &str,
        patch: TransformPatch,
    ) -> Result<Value, RpcFailure> {
        scene::scene_set_transform(self, node, patch)
    }
    fn scene_spawn(&mut self, object: Value) -> Result<Value, RpcFailure> {
        scene::scene_spawn(self, object)
    }
    fn scene_remove(&mut self, node: &str) -> Result<Value, RpcFailure> {
        scene::scene_remove(self, node)
    }
    fn scene_set_material(
        &mut self,
        node: &str,
        patch: MaterialPatch,
    ) -> Result<Value, RpcFailure> {
        scene::scene_set_material(self, node, patch)
    }
    fn scene_set_environment(&mut self, patch: EnvironmentPatch) -> Result<Value, RpcFailure> {
        env::scene_set_environment(self, patch)
    }
    fn scene_set_environment_map(
        &mut self,
        patch: EnvironmentMapPatch,
    ) -> Result<Value, RpcFailure> {
        env::scene_set_environment_map(self, patch)
    }
    fn scene_set_postfx(&mut self, patch: PostFxPatch) -> Result<Value, RpcFailure> {
        postfx::scene_set_postfx(self, patch)
    }
    fn scene_set_ssao(&mut self, patch: SsaoPatch) -> Result<Value, RpcFailure> {
        postfx::scene_set_ssao(self, patch)
    }
    fn scene_anim_layers(
        &mut self,
        instance: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Value, RpcFailure> {
        anim::scene_anim_layers(self, instance, limit)
    }
    fn scene_anim_layer_set(&mut self, patch: AnimLayerPatch) -> Result<Value, RpcFailure> {
        anim::scene_anim_layer_set(self, patch)
    }
    fn light_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        light::light_list(self, limit)
    }
    fn light_set(&mut self, patch: LightPatch) -> Result<Value, RpcFailure> {
        light::light_set(self, patch)
    }
    fn light_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        light::light_remove(self, key)
    }
    fn light_clear(&mut self) -> Result<Value, RpcFailure> {
        light::light_clear(self)
    }
    fn light_set_enabled(&mut self, enabled: Option<bool>) -> Result<Value, RpcFailure> {
        light::light_set_enabled(self, enabled)
    }
    fn audio_spatial_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        audio::audio_spatial_list(self, limit)
    }
    fn audio_spatial_set(&mut self, patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
        audio::audio_spatial_set(self, patch)
    }
    fn audio_spatial_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        audio::audio_spatial_remove(self, key)
    }
    fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
        audio::audio_spatial_clear(self)
    }
    fn audio_spatial_listener(
        &mut self,
        patch: Option<ListenerPatch>,
    ) -> Result<Value, RpcFailure> {
        audio::audio_spatial_listener(self, patch)
    }
    fn audio_spatial_set_enabled(&mut self, enabled: Option<bool>) -> Result<Value, RpcFailure> {
        audio::audio_spatial_set_enabled(self, enabled)
    }
    fn scene_shadow(&mut self, patch: Option<ShadowPatch>) -> Result<Value, RpcFailure> {
        shadow::scene_shadow(self, patch)
    }
    fn script_eval(&mut self, source: &str) -> Result<Value, RpcFailure> {
        script::script_eval(self, source)
    }
    fn ui_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        ui_rpc::ui_list(self, limit)
    }
    fn ui_set(&mut self, patch: UiPatch) -> Result<Value, RpcFailure> {
        ui_rpc::ui_set(self, patch)
    }
    fn ui_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        ui_rpc::ui_remove(self, key)
    }
    fn ui_state(&mut self) -> Result<Value, RpcFailure> {
        ui_rpc::ui_state(self)
    }
    fn scene_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        scene_flow::scene_save(self, path)
    }
    fn scene_stack(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        scene_flow::scene_stack(self, limit)
    }
    fn scene_switch(&mut self, path: &str) -> Result<Value, RpcFailure> {
        scene_flow::scene_switch(self, path)
    }
    fn scene_back(&mut self) -> Result<Value, RpcFailure> {
        scene_flow::scene_back(self)
    }
    fn scene_archive_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        scene_flow::scene_archive_save(self, path)
    }
    fn scene_archive_load(&mut self, path: &str) -> Result<Value, RpcFailure> {
        scene_flow::scene_archive_load(self, path)
    }
    fn script_reload(&mut self) -> Result<Value, RpcFailure> {
        script::script_reload(self)
    }
    fn tx_begin(&mut self) -> Result<Value, RpcFailure> {
        tx::tx_begin(self)
    }
    fn tx_apply(&mut self, tx_id: &str, op: &str, args: &Value) -> Result<Value, RpcFailure> {
        tx::tx_apply(self, tx_id, op, args)
    }
    fn tx_intent(&mut self, tx_id: &str, intent: &str, args: &Value) -> Result<Value, RpcFailure> {
        tx::tx_intent(self, tx_id, intent, args)
    }
    fn tx_preview(&mut self, tx_id: &str) -> Result<Value, RpcFailure> {
        tx::tx_preview(self, tx_id)
    }
    fn tx_commit(&mut self, tx_id: &str) -> Result<Value, RpcFailure> {
        tx::tx_commit(self, tx_id)
    }
    fn tx_rollback(&mut self, tx_id: &str) -> Result<Value, RpcFailure> {
        tx::tx_rollback(self, tx_id)
    }
    fn query_find(&mut self, spec: QuerySpec) -> Result<Value, RpcFailure> {
        query::query_find(self, spec)
    }
    fn query_describe(&mut self, node: &str) -> Result<Value, RpcFailure> {
        query::query_describe(self, node)
    }
    fn query_stats(&mut self) -> Result<Value, RpcFailure> {
        query::query_stats(self)
    }
    // ---- M1-S4-D：探测查询 / 导航 / 材质表（协议面 48 → 56 条） ----
    fn physics_raycast(&mut self, query: RayQuery) -> Result<Value, RpcFailure> {
        physics::physics_raycast(self, query)
    }
    fn physics_overlap(&mut self, query: OverlapQuery) -> Result<Value, RpcFailure> {
        physics::physics_overlap(self, query)
    }
    fn physics_cast(&mut self, query: CastQuery) -> Result<Value, RpcFailure> {
        physics::physics_cast(self, query)
    }
    /// 显式装配主世界（M1-S4-H）：协议面 57 → 58 条里唯一会写物理世界的入口，
    /// 且**不写场景**——见 `api::physics::physics_bind`。
    fn physics_bind(&mut self) -> Result<Value, RpcFailure> {
        physics::physics_bind(self)
    }
    fn material_list(&mut self) -> Result<Value, RpcFailure> {
        material::material_list(self)
    }
    fn material_upsert(&mut self, decl: MaterialUpsert) -> Result<Value, RpcFailure> {
        material::material_upsert(self, decl)
    }
    // ---- M1-S4-F：三层体检（协议面 56 → 57 条） ----
    fn scene_verify(&mut self, query: VerifyQuery) -> Result<Value, RpcFailure> {
        verify::scene_verify(self, query)
    }

    // ---- M2-B01：角色控制器六条（协议面 62 → 68 条） ----
    // 配置进场景（可撤销），运行态留在控制器里——`step` / `teleport` 不回写节点。
    fn character_create(&mut self, query: CharacterBindQuery) -> Result<Value, RpcFailure> {
        character::character_create(self, query)
    }

    fn character_config(&mut self, query: CharacterBindQuery) -> Result<Value, RpcFailure> {
        character::character_config(self, query)
    }

    fn character_step(&mut self, query: CharacterStepQuery) -> Result<Value, RpcFailure> {
        character::character_step(self, query)
    }

    fn character_state(&mut self, query: CharacterNameQuery) -> Result<Value, RpcFailure> {
        character::character_state(self, query)
    }

    fn character_teleport(&mut self, query: CharacterTeleportQuery) -> Result<Value, RpcFailure> {
        character::character_teleport(self, query)
    }

    fn character_stats(&mut self) -> Result<Value, RpcFailure> {
        character::character_stats(self)
    }

    fn scene_snapshot(&mut self, query: SnapshotQuery) -> Result<Value, RpcFailure> {
        inspect::scene_snapshot(self, query)
    }

    fn mesh_stats(&mut self, query: MeshStatsQuery) -> Result<Value, RpcFailure> {
        inspect::mesh_stats(self, query)
    }

    fn scene_validate(&mut self, query: ValidateQuery) -> Result<Value, RpcFailure> {
        inspect::scene_validate(self, query)
    }

    fn physics_simulate(&mut self, query: SimulateQuery) -> Result<Value, RpcFailure> {
        inspect::physics_simulate(self, query)
    }

    // ---- M2-C06：地形命令面（八条） ----
    //
    // 实现在 `terrain.rs`，重活在 `r3e_runtime::terrain_ops`（它才依赖 `r3e-terrain`）。
    // 四条写、四条读，写的那四条在 `terrain.rs` 里统一挡事务未决。

    fn terrain_create(&mut self, query: TerrainCreateQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_create(self, query)
    }

    fn terrain_remove(&mut self, query: TerrainNameQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_remove(self, query)
    }

    fn terrain_sample(&mut self, query: TerrainSampleQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_sample(self, query)
    }

    fn terrain_blocks(&mut self, query: TerrainBlocksQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_blocks(self, query)
    }

    fn terrain_resample(&mut self, query: TerrainResampleQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_resample(self, query)
    }

    fn terrain_gen(&mut self, query: TerrainGenQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_gen(self, query)
    }

    fn terrain_stats(&mut self) -> Result<Value, RpcFailure> {
        terrain::terrain_stats(self)
    }

    fn terrain_mesh(&mut self, query: TerrainMeshQuery) -> Result<Value, RpcFailure> {
        terrain::terrain_mesh(self, query)
    }
}

/// 导航十条（M1-S4-D 三条 + M2-D04 七条）。
///
/// 单独成块是为了和协议层对齐：`nav.*` 的契约注释与缺省实现在
/// `r3e_protocol::api::nav` 里是子 trait（`EngineApi: NavApi`），本文件照同样的分层
/// 摆放，不把十条塞回引擎 API 的实现块去把它撑长。方法体只有一句委托——
/// 「协议翻译 + 状态哈希纪律」在本文件对应的 `api::nav` 子模块里，
/// 「算一段 / 读写字节」在 `r3e-runtime::nav_ops` 里。
impl NavApi for LocalEngineApi<'_> {
    fn nav_bake(&mut self, options: NavBakeOptions) -> Result<Value, RpcFailure> {
        nav::nav_bake(self, options)
    }
    fn nav_path(&mut self, query: NavPathQuery) -> Result<Value, RpcFailure> {
        nav::nav_path(self, query)
    }
    fn nav_reachability(&mut self, query: NavReachQuery) -> Result<Value, RpcFailure> {
        nav::nav_reachability(self, query)
    }
    fn nav_smooth(&mut self, query: NavSmoothQuery) -> Result<Value, RpcFailure> {
        nav::nav_smooth(self, query)
    }
    fn nav_probe(&mut self, query: NavPointQuery) -> Result<Value, RpcFailure> {
        nav::nav_probe(self, query)
    }
    fn nav_nearest(&mut self, query: NavNearestQuery) -> Result<Value, RpcFailure> {
        nav::nav_nearest(self, query)
    }
    fn nav_stats(&mut self) -> Result<Value, RpcFailure> {
        nav::nav_stats(self)
    }
    fn nav_hash(&mut self) -> Result<Value, RpcFailure> {
        nav::nav_hash(self)
    }
    fn nav_export(&mut self, target: NavFileTarget) -> Result<Value, RpcFailure> {
        nav::nav_export(self, target)
    }
    fn nav_import(&mut self, target: NavFileTarget) -> Result<Value, RpcFailure> {
        nav::nav_import(self, target)
    }
}

/// prefab 两条（M2-E04）：`prefab.spawn` / `prefab.info`。
///
/// 与 [`NavApi`] 同样单独成块，理由不是「文件太长」，而是分层对齐：
/// `prefab.*` 在协议层是 `EngineApi: PrefabApi` 子 trait，缺省实现回
/// `METHOD_NOT_FOUND`；宿主这边一句话委托，协议翻译（错误码分流 / 回包字段）
/// 全在 `api::prefab` 子模块。四条无 rpc 的 prefab 命令不在本块——
/// 它们没有 RPC 入口，走的是子模块里的本地函数。
impl PrefabApi for LocalEngineApi<'_> {
    fn prefab_spawn(&mut self, req: PrefabSpawnRequest) -> Result<Value, RpcFailure> {
        prefab::prefab_spawn(self, req)
    }
    fn prefab_info(&mut self, req: PrefabInfoRequest) -> Result<Value, RpcFailure> {
        prefab::prefab_info(self, req)
    }
}

#[cfg(test)]
mod tests;
