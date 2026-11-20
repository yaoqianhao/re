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
    AnimLayerPatch, AudioSpatialPatch, CastQuery, EngineApi, EngineInfo, EnvironmentMapPatch,
    EnvironmentPatch, LightPatch, ListenerPatch, MaterialPatch, MaterialUpsert, MeshStatsQuery,
    NavBakeOptions, NavPathQuery, NavReachQuery, NodeBrief, OverlapQuery, PhysicsInfo, PostFxPatch,
    QuerySpec, RayQuery, RpcFailure, ShadowPatch, SimulateQuery, SnapshotQuery, SsaoPatch,
    TransformPatch, UiPatch, ValidateQuery, VerifyQuery, PROTOCOL_VERSION, SERVER_BUSY,
};
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
mod env;
mod info;
mod inspect;
mod light;
mod material;
mod nav;
mod physics;
mod postfx;
mod query;
mod scene;
mod scene_flow;
mod script;
mod shadow;
mod support;
mod tx;
mod tx_violations;
mod ui_rpc;
mod verify;

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
    fn nav_bake(&mut self, options: NavBakeOptions) -> Result<Value, RpcFailure> {
        nav::nav_bake(self, options)
    }
    fn nav_path(&mut self, query: NavPathQuery) -> Result<Value, RpcFailure> {
        nav::nav_path(self, query)
    }
    fn nav_reachability(&mut self, query: NavReachQuery) -> Result<Value, RpcFailure> {
        nav::nav_reachability(self, query)
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
}

#[cfg(test)]
mod tests;
