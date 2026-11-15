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

use glam::{Quat, Vec3};
use r3e_audio::{Audio, DistanceModel, MAX_SPATIAL_SOURCES};
use r3e_core::light::MAX_LOCAL_LIGHTS;
use r3e_core::postfx::{PostFxSettings, TonemapMode};
use r3e_core::scene_stack::{SceneArchive, SceneCommand, ARCHIVE_FORMAT, ARCHIVE_VERSION};
use r3e_core::ssao::{SsaoSettings, MAX_SSAO_SAMPLES, MIN_SSAO_SAMPLES};
use r3e_core::ui::{self, Anchor, UiElement, UiKind, UiUpsert, UiValue};
use r3e_core::ui_text;
use r3e_core::{AnimLayer, BoneMask, LayerMode, LayerSync, MAX_ANIM_LAYERS};
use r3e_core::{LocalLight, LocalLightKind, Node, ObjectDesc, Scene};
use r3e_core::{ShadowSettings, MAX_CASCADES, MAX_SCENE_STACK};
use r3e_doc::Document;
use r3e_editor::{
    AnimLayerPatch, AudioSpatialPatch, EngineApi, EngineInfo, EnvironmentMapPatch,
    EnvironmentPatch, LightPatch, ListenerPatch, MaterialPatch, NodeBrief, PhysicsInfo,
    PostFxPatch, RpcFailure, ShadowPatch, SsaoPatch, TransformPatch, UiPatch, PROTOCOL_VERSION,
    SERVER_BUSY,
};
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
    pub scripts: &'a mut ScriptHost,
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
mod light;
mod postfx;
mod scene;
mod scene_flow;
mod script;
mod shadow;
mod support;
mod ui_rpc;

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
}

#[cfg(test)]
mod tests;
