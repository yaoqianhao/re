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
use std::time::{SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec3};
use r3e_audio::{Audio, DistanceModel, MAX_SPATIAL_SOURCES};
use r3e_core::light::MAX_LOCAL_LIGHTS;
use r3e_core::postfx::{PostFxSettings, TonemapMode};
use r3e_core::scene_stack::{SceneArchive, SceneCommand, ARCHIVE_FORMAT, ARCHIVE_VERSION};
use r3e_core::ssao::{SsaoSettings, MAX_SSAO_SAMPLES, MIN_SSAO_SAMPLES};
use r3e_core::ui::{self, Anchor, UiElement, UiKind, UiUnit, UiUpsert, UiValue};
use r3e_core::{AnimLayer, BoneMask, LayerMode, LayerSync, MAX_ANIM_LAYERS};
use r3e_core::{LocalLight, LocalLightKind, Node, ObjectDesc, Scene};
use r3e_core::{ShadowSettings, MAX_CASCADES, MAX_SCENE_STACK};
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

/// 帧时间窗口的容量：约 4 秒 @60fps。
///
/// 太短会让 p99 落在噪声上（一帧抖动就顶满），太长则改完场景要等好几秒才看得到
/// 分位变化。120 帧是两者之间的折中。
pub const FRAME_TIME_WINDOW: usize = 120;

/// 最近若干帧的 CPU 帧时间分布（毫秒），`engine.stats` 的 `frame.frame_ms` 就是它。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameTimeStats {
    /// 窗口里实际有多少个样本（启动初期远小于容量）。
    pub samples: usize,
    /// 最近一帧的耗时。
    pub last_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
}

impl FrameTimeStats {
    /// 从一批样本算出分布。
    ///
    /// 分位用 nearest-rank（`ceil(p·n)` 后取该名次），不是插值：样本量小的时候
    /// 插值会造出「没有一帧真的这么慢」的数字，回归基线比的是实际观测值。
    pub fn from_samples(samples: &[f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pick = |p: f32| -> f32 {
            let n = sorted.len();
            let rank = ((p * n as f32).ceil() as usize).clamp(1, n);
            sorted[rank - 1]
        };
        Self {
            samples: sorted.len(),
            last_ms: samples[samples.len() - 1],
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            p99_ms: pick(0.99),
            max_ms: sorted[sorted.len() - 1],
        }
    }
}

/// 固定容量的帧时间环形缓冲：满了就覆盖最旧的样本。
#[derive(Debug, Clone)]
pub struct FrameTimeWindow {
    buf: Vec<f32>,
    next: usize,
    /// 最近一次采到的帧时间。
    ///
    /// 不能靠 `buf.last()` 得到它：环形缓冲写满后物理顺序是「覆盖顺序」，
    /// 末元素不再是最后发生的那一帧。`last_ms` 在面板上表示「当前帧耗时」，
    /// 必须与采样时刻一致，所以单独记一份。
    last: Option<f32>,
}

impl FrameTimeWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity.max(1)),
            next: 0,
            last: None,
        }
    }

    pub fn push(&mut self, ms: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        if self.buf.len() < self.buf.capacity() {
            self.buf.push(ms);
        } else {
            let cap = self.buf.len();
            self.buf[self.next % cap] = ms;
        }
        self.next = self.next.wrapping_add(1);
        self.last = Some(ms);
    }

    pub fn snapshot(&self) -> FrameTimeStats {
        let mut stats = FrameTimeStats::from_samples(&self.buf);
        if let Some(last) = self.last {
            stats.last_ms = last;
        }
        stats
    }
}

impl Default for FrameTimeWindow {
    fn default() -> Self {
        Self::new(FRAME_TIME_WINDOW)
    }
}

/// 把截图目标路径规范成绝对路径：相对路径按进程当前目录解析。
///
/// `capture.frame` 对外承诺「返回绝对路径」，所以进来先在宿主侧定死基准，
/// 免得下游拿到 `captures\\a.png` 这种相对串还得再猜一次基准目录。
fn absolute_path(path: &Path) -> Result<PathBuf, RpcFailure> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|err| RpcFailure::engine(format!("无法解析相对路径的基准目录: {err}")))
}

/// 默认截图文件名：毫秒时间戳，连续截图不会互相覆盖。
fn default_capture_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("capture_{stamp}.png")
}

/// 主线程上的 [`EngineApi`] 实现。
///
/// 所有方法都在事件循环线程调用，所以可以直接 `borrow_mut` 场景与物理，
/// 既不需要锁，也不会出现「RPC 线程改场景、渲染线程正在读」的撕裂。
pub struct LocalEngineApi<'a> {
    pub scene: &'a Rc<RefCell<Scene>>,
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

impl EngineApi for LocalEngineApi<'_> {
    /// P0-4：一帧的「数字画像」——帧时间分位 + 渲染统计 + 场景规模。
    ///
    /// AI 改完场景后先看这里：`draw_calls` / `instances` / `shadow_draws` 有没有按预期
    /// 涨跌，`ssao.passes`、`shadow_cascades` 有没有真的打开，比肉眼看画面更可靠。
    /// 渲染器缺席时 `render` 直接给 `null`（`renderer: false`），绝不填零冒充正常。
    fn engine_stats(&mut self) -> Result<Value, RpcFailure> {
        let (scene_name, objects, visible, mesh_kinds, material_overrides, physics_bodies) = {
            let scene = self.scene.borrow();
            let mut mesh_kinds = std::collections::HashSet::new();
            let mut visible = 0usize;
            let mut material_overrides = 0usize;
            let mut physics_bodies = 0usize;
            for node in scene.iter() {
                mesh_kinds.insert(node.mesh);
                visible += usize::from(node.visible);
                material_overrides += usize::from(node.material.is_some());
                physics_bodies += usize::from(node.physics.is_some());
            }
            (
                scene.name.clone(),
                scene.len(),
                visible,
                mesh_kinds.len(),
                material_overrides,
                physics_bodies,
            )
        };

        let frame = self.frame_ms;
        let (size, stats) = match self.renderer.as_deref() {
            Some(renderer) => (renderer.size(), Some(renderer.stats())),
            None => ((0u32, 0u32), None),
        };
        let render = match stats {
            Some(s) => json!({
                "draw_calls": s.draw_calls,
                "instances": s.instances,
                "culled": s.culled,
                "cull_unknown": s.cull_unknown,
                "shadow_draws": s.shadow_draws,
                "shadow_cascades": s.shadow_cascades,
                // 扩展二十四：按级裁剪遮挡体的判定量与裁掉量。`culled` 为 0 且
                // `tested` 为 0 = 裁剪没开（或阴影关闭），此时 `shadow_draws` 会回到
                // 扩展二十三的「批次数 × 级数」。
                "shadow_casters": s.shadow_casters,
                "shadow_culled": s.shadow_culled,
                // 扩展二十五：层级剔除。`shadow_pruned` 是整枝跳过省下的实例判定数
                // （与 `shadow_casters` 相加才是逐实例时代的总量），
                // `shadow_box_tests` 是被下探的树节点数 × 级数，
                // `caster_tree_builds` 稳态不变 = 层级树缓存生效。
                "shadow_pruned": s.shadow_pruned,
                "shadow_branch_pruned": s.shadow_branch_pruned,
                "shadow_box_tests": s.shadow_box_tests,
                "caster_tree_builds": s.caster_tree_builds,
                // 扩展二十九：主通道视锥的层级剔除。`pruned` 是「没做实例判定就被
                // 整枝剪掉」的实例数，`instance_tests + pruned` 应等于可见节点数；
                // 只有 `branch_pruned` 是相对逐实例判定的净收益。
                "cull": {
                    "box_tests": s.cull.box_tests,
                    "instance_tests": s.cull.instance_tests,
                    "pruned": s.cull.pruned,
                    "branch_pruned": s.cull.branch_pruned,
                    "tree_builds": s.cull.tree_builds,
                },
                // 扩展十九：半透明批次与 MSAA。`transparent_draws` 为 0 = 本帧没有
                // 半透明物件；`depth_prepass_draws` 应恒等于不透明批次的 draw call 数，
                // 掉成 0 说明遮蔽通道的单采样深度来源断了。
                "transparent_draws": s.transparent_draws,
                "transparent_instances": s.transparent_instances,
                "depth_prepass_draws": s.depth_prepass_draws,
                // 扩展二十二：遮挡剔除。`enabled` 是「真的在判」（开关开 + 金字塔就绪），
                // `occluded` 是本帧因此少画的实例数，`readback` 表示本帧刚取回一份新深度。
                // 扩展三十：`hierarchy` 为 true 时走的是按子树预筛（`box_tests` /
                // `pruned` / `branch_pruned` 才有意义），`tested + pruned` = 过视锥的
                // 不透明实例数；`hierarchy` 为 false 时退回逐实例，`tested` 就是全部。
                "occlusion": {
                    "enabled": s.occlusion.enabled,
                    "tested": s.occlusion.tested,
                    "occluded": s.occlusion.occluded,
                    "readback": s.occlusion.readback,
                    "levels": s.occlusion.levels,
                    "hierarchy": s.occlusion.hierarchy,
                    "box_tests": s.occlusion.box_tests,
                    "pruned": s.occlusion.pruned,
                    "branch_pruned": s.occlusion.branch_pruned,
                },
                "msaa_samples": r3e_render::MSAA_SAMPLE_COUNT,
                // 扩展三十一：实例缓冲的脏段上传。`recorded + skipped` = 本帧实例
                // 记录总数；`bytes` 是真正传上 GPU 的字节，`full_bytes` 是全量上传
                // 本该写的字节，相除就是本帧省下的比例。`full` 在首帧、扩容换代、
                // 实例数变化或脏段碎到超限时为 true；`resets` 累计增长说明容量估算失效。
                "instance_upload": {
                    "ranges": s.instance_upload.ranges,
                    "recorded": s.instance_upload.recorded,
                    "skipped": s.instance_upload.skipped,
                    "bytes": s.instance_upload.bytes,
                    "full_bytes": s.instance_upload.full_bytes,
                    "full": s.instance_upload.full,
                    "resets": s.instance_upload.resets,
                },
                // 扩展三十二：关节矩阵的脏段上传。口径同上，记录换成 mat4（64 B）；
                // 关节缓冲容量固定不换代，`resets` 恒为 0。
                "joint_upload": {
                    "ranges": s.joint_upload.ranges,
                    "recorded": s.joint_upload.recorded,
                    "skipped": s.joint_upload.skipped,
                    "bytes": s.joint_upload.bytes,
                    "full_bytes": s.joint_upload.full_bytes,
                    "full": s.joint_upload.full,
                    "resets": s.joint_upload.resets,
                },
                // 扩展三十五：暂存环（持久映射槽 + 多帧飞行）。`writes` 是队列级
                // write_buffer 次数，`saves` 是被合掉的次数，`copies` 是块拷贝条数。
                "upload_staging": {
                    "enabled": s.upload_staging.enabled,
                    "slots": s.upload_staging.slots,
                    "capacity": s.upload_staging.capacity,
                    "frames": s.upload_staging.frames,
                    "writes": s.upload_staging.writes,
                    "copies": s.upload_staging.copies,
                    "bytes": s.upload_staging.bytes,
                    "saves": s.upload_staging.saves,
                    "rebuilds": s.upload_staging.rebuilds,
                    "fallbacks": s.upload_staging.fallbacks,
                },
                "render_frame_index": s.frame_index,
                "ssao": {
                    "enabled": s.ssao.enabled,
                    "passes": s.ssao.passes,
                    "samples": s.ssao.samples,
                    "width": s.ssao.size.0,
                    "height": s.ssao.size.1,
                },
                "postfx": {
                    "bloom_passes": s.postfx.bloom_passes,
                    "composite_passes": s.postfx.composite_passes,
                    "bloom_levels": s.postfx.bloom_levels,
                },
                "local_lights": s.local_lights,
                "local_shadow_casters": s.local_shadow_casters,
                "local_shadow_layers": s.local_shadow_layers,
                "text_glyphs": s.text_glyphs,
                "text_draws": s.text_draws,
                "text_missing": s.text_missing,
                "ui_quads": s.ui_quads,
                "ui_draws": s.ui_draws,
            }),
            None => Value::Null,
        };

        Ok(json!({
            "renderer": stats.is_some(),
            "frame": {
                "index": self.frames,
                "fps": self.fps,
                "uptime_ms": self.uptime_ms,
                "paused": self.paused,
                "frame_ms": {
                    "samples": frame.samples,
                    "last": frame.last_ms,
                    "p50": frame.p50_ms,
                    "p95": frame.p95_ms,
                    "p99": frame.p99_ms,
                    "max": frame.max_ms,
                },
            },
            "scene": {
                "name": scene_name,
                "objects": objects,
                "visible": visible,
                "mesh_kinds": mesh_kinds,
                "material_overrides": material_overrides,
                "physics_bodies": physics_bodies,
            },
            "render": render,
            "size": { "width": size.0, "height": size.1 },
        }))
    }

    /// P0-4：把当前一帧渲染到离屏纹理并落成 PNG，返回绝对路径。
    ///
    /// 这是「看画面」的唯一入口：AI 拿到路径后可以直接看图，不必依赖窗口。
    /// 落盘后回读文件长度做一次自检——空文件当失败上报，避免下游拿到一张坏图。
    fn capture_frame(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return Err(RpcFailure::engine(
                "渲染器未初始化（窗口 / GPU 未就绪），当前进程无法截图",
            ));
        };
        let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(raw) => absolute_path(Path::new(raw))?,
            None => self.capture_dir.join(default_capture_name()),
        };
        if target.is_dir() {
            return Err(RpcFailure::invalid_params(format!(
                "截图路径是目录而不是文件: {}",
                target.display()
            )));
        }

        let captured = {
            let scene = self.scene.borrow();
            renderer.capture_frame(&scene).map_err(RpcFailure::engine)?
        };
        r3e_asset::write_png_rgba(&target, captured.width, captured.height, &captured.rgba8)
            .map_err(|err| RpcFailure::engine(format!("截图落盘失败: {err}")))?;
        let bytes = std::fs::metadata(&target)
            .map(|meta| meta.len())
            .map_err(|err| RpcFailure::engine(format!("截图落盘后读元数据失败: {err}")))?;
        if bytes == 0 {
            return Err(RpcFailure::engine(format!(
                "截图文件为空（{}），已按失败上报",
                target.display()
            )));
        }

        Ok(json!({
            "path": target.display().to_string(),
            "width": captured.width,
            "height": captured.height,
            "bytes": bytes,
            "pixels": captured.byte_len(),
            "format": format!("{:?}", captured.format),
        }))
    }

    fn info(&mut self) -> EngineInfo {
        let (scene_name, nodes) = {
            let scene = self.scene.borrow();
            (scene.name.clone(), scene.len())
        };
        let physics = self.physics.borrow();
        EngineInfo {
            name: ENGINE_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION,
            scene: if self.scene_label.is_empty() {
                scene_name
            } else {
                self.scene_label.clone()
            },
            script: self.script_label.clone(),
            frames: self.frames,
            fps: self.fps,
            uptime_ms: self.uptime_ms,
            paused: self.paused,
            nodes,
            physics: PhysicsInfo {
                enabled: physics.enabled(),
                backend: physics.backend_name().to_string(),
                capabilities: physics
                    .capabilities()
                    .enabled_keys()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                bodies: physics.body_count(),
                ccd_default: physics.ccd_default(),
            },
        }
    }

    fn scene_list(
        &mut self,
        filter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<NodeBrief>, RpcFailure> {
        let needle = filter
            .map(|f| f.trim().to_ascii_lowercase())
            .filter(|f| !f.is_empty());
        let scene = self.scene.borrow();
        let mut out = Vec::new();
        for node in scene.iter() {
            if let Some(needle) = &needle {
                if !node.name.to_ascii_lowercase().contains(needle) {
                    continue;
                }
            }
            if let Some(limit) = limit {
                if out.len() >= limit {
                    break;
                }
            }
            out.push(NodeBrief {
                name: node.name.clone(),
                mesh: Some(node.mesh.as_string()),
                position: node.transform.position.to_array(),
                visible: node.visible,
            });
        }
        Ok(out)
    }

    fn scene_get(&mut self, node: &str) -> Result<Value, RpcFailure> {
        let scene = self.scene.borrow();
        Ok(node_value(find_node(&scene, node)?))
    }

    fn scene_set_transform(
        &mut self,
        node: &str,
        patch: TransformPatch,
    ) -> Result<Value, RpcFailure> {
        if patch.is_empty() {
            return Err(RpcFailure::invalid_params("变换补丁为空"));
        }
        let mut scene = self.scene.borrow_mut();
        let id = find_node_id(&scene, node)?;
        let snapshot = {
            let target = scene
                .get_mut(id)
                .ok_or_else(|| RpcFailure::not_found(format!("物件 `{node}` 不存在")))?;
            if let Some(position) = patch.position {
                check_finite(&position, "position")?;
                target.transform.position = Vec3::from_array(position);
            }
            if let Some(rotation) = patch.rotation {
                check_finite(&rotation, "rotation")?;
                let quat = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
                if quat.length_squared() < 1e-12 {
                    return Err(RpcFailure::invalid_params(
                        "rotation 需要非零四元数 [x,y,z,w]（不能全为 0）",
                    ));
                }
                target.transform.rotation = quat.normalize();
            }
            if let Some(scale) = patch.scale {
                check_finite(&scale, "scale")?;
                target.transform.scale = Vec3::from_array(scale);
            }
            node_value(target)
        };
        // 局部变换改完立刻刷新世界矩阵：编辑器下一次 `scene.get` 应看到新的 world
        scene.update_world_transforms();
        Ok(snapshot)
    }

    fn scene_spawn(&mut self, object: Value) -> Result<Value, RpcFailure> {
        let desc: ObjectDesc = serde_json::from_value(object)
            .map_err(|e| RpcFailure::invalid_params(format!("物件描述不合法: {e}")))?;
        let name = desc.name.clone();
        let mut scene = self.scene.borrow_mut();
        let id = scene.spawn(desc).map_err(RpcFailure::engine)?;
        scene.update_world_transforms();
        Ok(json!({ "id": id, "name": name, "nodes": scene.len() }))
    }

    fn scene_remove(&mut self, node: &str) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let id = find_node_id(&scene, node)?;
        let name = scene
            .get(id)
            .map(|n| n.name.clone())
            .ok_or_else(|| RpcFailure::not_found(format!("物件 `{node}` 不存在")))?;
        scene.despawn_id(id);
        scene.update_world_transforms();
        Ok(json!({ "removed": name, "nodes": scene.len() }))
    }

    /// 材质补丁：在现有材质上叠加，再交给 `SurfaceMaterial` 统一收敛。
    /// 返回合并后的完整材质（读回入口），调用方据此确认非法值已被夹取。
    fn scene_set_material(
        &mut self,
        node: &str,
        patch: MaterialPatch,
    ) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let mut material = scene.material_of(node).unwrap_or_default();
        if let Some(metallic) = patch.metallic {
            material.metallic = metallic;
        }
        if let Some(roughness) = patch.roughness {
            material.roughness = roughness;
        }
        if let Some(emissive) = patch.emissive {
            material.emissive = emissive;
        }
        if !scene.set_material(node, material) {
            return Err(RpcFailure::not_found(format!("物件 `{node}` 不存在")));
        }
        let applied = scene.material_of(node).unwrap_or_default();
        let snapshot = find_node(&scene, node)
            .map(node_value)
            .unwrap_or(Value::Null);
        Ok(json!({
            "material": material_value(applied),
            "node": snapshot,
        }))
    }

    /// 环境探针补丁：在现有环境上叠加（未启用时从缺省起步），再交给 `EnvironmentSettings`
    /// 统一收敛。`enabled = false` 清空环境、退回 `light.ambient` 常数项。
    fn scene_set_environment(&mut self, patch: EnvironmentPatch) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        if patch.enabled == Some(false) {
            scene.environment = None;
            return Ok(json!({ "environment": Value::Null, "enabled": false }));
        }
        let mut env = scene.environment.unwrap_or_default();
        if let Some(v) = patch.sky_zenith {
            env.sky_zenith = v;
        }
        if let Some(v) = patch.sky_horizon {
            env.sky_horizon = v;
        }
        if let Some(v) = patch.ground {
            env.ground = v;
        }
        if let Some(v) = patch.sun_dir {
            env.sun_dir = v;
        }
        if let Some(v) = patch.sun_color {
            env.sun_color = v;
        }
        if let Some(v) = patch.sun_intensity {
            env.sun_intensity = v;
        }
        if let Some(v) = patch.intensity {
            env.intensity = v;
        }
        let applied = env.sanitized();
        scene.environment = Some(applied);
        Ok(json!({
            "enabled": true,
            "environment": environment_value(applied),
        }))
    }

    /// 反射探针补丁（扩展二十三）：在现有 `environment_map` 段上叠加（段缺失时从缺省起步），
    /// 再交给 `EnvironmentMapSettings::sanitized` 统一收敛。
    ///
    /// 三条与「解析 IBL」不同的语义，都在这里定死：
    /// * `enabled = false` 只翻开关，路径保留——A/B 对比时不必重新填路径；
    /// * **显式给了来源却收敛成空**（`equirect: ""` / `faces: []` / 六面不足 6 条）时整段丢弃，
    ///   等价于「没有探针」，比翻开关更彻底（面板上的清空按钮走这条）；
    /// * `face_size` / `mip_levels` 用有符号整数收，负数在这里夹到区间下限，不静默改 0。
    fn scene_set_environment_map(
        &mut self,
        patch: EnvironmentMapPatch,
    ) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let source_given = patch.equirect.is_some() || patch.faces.is_some();
        let mut map = scene.environment_map.clone().unwrap_or_default();
        if let Some(v) = patch.enabled {
            map.enabled = v;
        }
        if let Some(v) = patch.equirect {
            map.equirect = Some(v);
        }
        if let Some(v) = patch.faces {
            map.faces = Some(v);
        }
        if let Some(v) = patch.face_size {
            // 负数 / 0 一律落到 4 像素下限，再由 sanitized 向上取到 2 的幂
            map.face_size = v.max(4) as usize;
        }
        if let Some(v) = patch.mip_levels {
            map.mip_levels = v.clamp(1, r3e_core::envmap::MAX_SPECULAR_MIPS as i32) as u32;
        }
        if let Some(v) = patch.intensity {
            map.intensity = v;
        }
        let applied = map.sanitized();
        if source_given && applied.equirect.is_none() && applied.faces.is_none() {
            // 来源被清空：整段丢掉，渲染层立刻退回解析 IBL
            scene.environment_map = None;
            return Ok(json!({
                "enabled": false,
                "source": "none",
                "environment_map": Value::Null,
            }));
        }
        let snapshot = environment_map_value(applied.clone());
        scene.environment_map = Some(applied.clone());
        Ok(json!({
            "enabled": applied.is_active(),
            "source": applied.source_label(),
            "environment_map": snapshot,
        }))
    }

    /// 后处理补丁（扩展十三）：在现有设置上叠加（未开启时从缺省起步），
    /// 再交给 `PostFxSettings::sanitized` 统一收敛。
    /// `enabled = false` 清空整段、回到 composite 直通。
    fn scene_set_postfx(&mut self, patch: PostFxPatch) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        if patch.enabled == Some(false) {
            scene.postfx = None;
            return Ok(json!({ "postfx": Value::Null, "enabled": false }));
        }
        let mut fx = scene.postfx.unwrap_or_default();
        if let Some(v) = patch.exposure {
            fx.exposure = v;
        }
        // 名字的合法性在这里判定：认不出来就报参数错误，不做静默回落。
        if let Some(name) = patch.tonemap.as_deref() {
            fx.tonemap = TonemapMode::from_name(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "未知色调映射 `{name}`（可选：none / reinhard / aces）"
                ))
            })?;
        }
        if let Some(v) = patch.bloom_enabled {
            fx.bloom.enabled = v;
        }
        if let Some(v) = patch.bloom_threshold {
            fx.bloom.threshold = v;
        }
        if let Some(v) = patch.bloom_knee {
            fx.bloom.knee = v;
        }
        if let Some(v) = patch.bloom_intensity {
            fx.bloom.intensity = v;
        }
        if let Some(v) = patch.fxaa {
            fx.fxaa = v;
        }
        let applied = fx.sanitized();
        scene.postfx = Some(applied);
        Ok(json!({
            "enabled": true,
            "postfx": postfx_value(applied),
        }))
    }

    /// 遮蔽补丁（扩展十七）：在现有后处理段的 `ssao` 上叠加（段缺失时从缺省起步），
    /// 再交给 `SsaoSettings::sanitized` 统一收敛。
    /// 与 `scene_set_postfx` 的区别：`enabled = false` 只关遮蔽这一条支线，
    /// 曝光 / 泛光 / FXAA 全部保留——遮挡本来就该能单独开关。
    fn scene_set_ssao(&mut self, patch: SsaoPatch) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let mut fx = scene.postfx.unwrap_or_default();
        if let Some(v) = patch.enabled {
            fx.ssao.enabled = v;
        }
        if let Some(v) = patch.radius {
            fx.ssao.radius = v;
        }
        if let Some(v) = patch.bias {
            fx.ssao.bias = v;
        }
        if let Some(v) = patch.intensity {
            fx.ssao.intensity = v;
        }
        if let Some(v) = patch.samples {
            // 越界直接夹：样本数区间是引擎的硬约束，编辑器拖滑块越界是常态，
            // 报参数错误只会让前端反复弹错。
            fx.ssao.samples =
                v.clamp(i32::from(MIN_SSAO_SAMPLES), i32::from(MAX_SSAO_SAMPLES)) as u8;
        }
        let applied = fx.sanitized();
        scene.postfx = Some(applied);
        Ok(json!({
            "enabled": applied.ssao.enabled,
            "neutral": applied.ssao.is_neutral(),
            "ssao": ssao_value(applied.ssao),
            "postfx": postfx_value(applied),
        }))
    }

    /// 列出混合树层（扩展十八 A）。`instance` 缺省列出全部动画实例的层。
    ///
    /// 显式点名却找不到实例时报参数错误：编辑器把实例名拼错时，回空列表会让人以为
    /// 「这个实例没有层」，比直接说「没有这个实例」难排查得多。
    fn scene_anim_layers(
        &mut self,
        instance: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Value, RpcFailure> {
        let scene = self.scene.borrow();
        let mut layers = Vec::new();
        let (mut total, mut instances) = (0usize, 0usize);
        match instance {
            Some(name) => {
                let inst = scene.anim_instance(name).ok_or_else(|| {
                    RpcFailure::invalid_params(format!("没有名为 `{name}` 的动画实例"))
                })?;
                instances = 1;
                total = inst.layers.len();
                for layer in inst.layers.iter().take(limit.unwrap_or(usize::MAX)) {
                    layers.push(anim_layer_value(&inst.name, layer));
                }
            }
            None => {
                for inst in &scene.animations {
                    instances += 1;
                    total += inst.layers.len();
                    for layer in &inst.layers {
                        layers.push(anim_layer_value(&inst.name, layer));
                    }
                }
                layers.truncate(limit.unwrap_or(usize::MAX));
            }
        }
        Ok(json!({
            "count": total,
            "instances": instances,
            "limit": MAX_ANIM_LAYERS,
            "layers": layers,
        }))
    }

    /// 混合树层补丁（扩展十八 A）：与 `light_set` 不同，层**不能**凭空创建——
    /// 层必须绑定片段，凭空建层等于建一个播不了任何动作的空层，因此层不存在一律报错。
    /// 所有字符串先解析完再落地，写错的模式 / 片段不会留下半个被改过的层。
    fn scene_anim_layer_set(&mut self, patch: AnimLayerPatch) -> Result<Value, RpcFailure> {
        let mode = match patch.mode.as_deref() {
            Some(name) => Some(LayerMode::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "未知叠加模式 `{name}`（可选：override / additive）"
                ))
            })?),
            None => None,
        };
        let mask = patch.mask.as_deref().map(bone_mask_of);
        let sync = match patch.sync.as_deref() {
            Some(name) => Some(LayerSync::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!("未知采样策略 `{name}`（可选：off / phase）"))
            })?),
            None => None,
        };
        let mut scene = self.scene.borrow_mut();
        if scene.anim_instance(&patch.instance).is_none() {
            return Err(RpcFailure::invalid_params(format!(
                "没有名为 `{}` 的动画实例",
                patch.instance
            )));
        }
        if let Some(clip) = patch.clip.as_deref() {
            if !scene.set_anim_layer_clip(&patch.instance, &patch.layer, clip) {
                return Err(RpcFailure::invalid_params(format!(
                    "实例 `{}` 上没有层 `{}`，或片段 `{clip}` 解析不到",
                    patch.instance, patch.layer
                )));
            }
        }
        if let Some(v) = patch.weight {
            if !scene.set_anim_layer_weight(&patch.instance, &patch.layer, v) {
                return Err(layer_missing(&patch.instance, &patch.layer));
            }
        }
        if let Some(v) = mode {
            if !scene.set_anim_layer_mode(&patch.instance, &patch.layer, v) {
                return Err(layer_missing(&patch.instance, &patch.layer));
            }
        }
        if let Some(v) = mask {
            if !scene.set_anim_layer_mask(&patch.instance, &patch.layer, v) {
                return Err(layer_missing(&patch.instance, &patch.layer));
            }
        }
        if let Some(v) = patch.speed {
            if !scene.set_anim_layer_speed(&patch.instance, &patch.layer, v) {
                return Err(layer_missing(&patch.instance, &patch.layer));
            }
        }
        if let Some(v) = sync {
            if !scene.set_anim_layer_sync_strategy(&patch.instance, &patch.layer, v) {
                return Err(layer_missing(&patch.instance, &patch.layer));
            }
        }
        let layer = scene
            .anim_layer(&patch.instance, &patch.layer)
            .ok_or_else(|| layer_missing(&patch.instance, &patch.layer))?;
        Ok(json!({
            "layer": anim_layer_value(&patch.instance, layer),
        }))
    }

    fn light_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        let scene = self.scene.borrow();
        let lights: Vec<Value> = scene
            .lights
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(local_light_value)
            .collect();
        Ok(json!({
            "count": scene.lights.len(),
            "limit": MAX_LOCAL_LIGHTS,
            "enabled": scene.lights_enabled,
            "lights": lights,
        }))
    }

    /// 局部光源补丁（扩展十五）：key 存在则叠加，不存在则从默认值起步新建，
    /// 最后统一交给 `LocalLight::sanitized`——编辑器传 NaN / 负强度也污染不了场景。
    fn light_set(&mut self, patch: LightPatch) -> Result<Value, RpcFailure> {
        if patch.key.trim().is_empty() {
            return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
        }
        // 名字先解析完再落地：写错的 kind 不该留下半盏被改的灯。
        let kind = match patch.kind.as_deref() {
            Some(name) => Some(LocalLightKind::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!("未知光源类型 `{name}`（可用：point / spot）"))
            })?),
            None => None,
        };

        let mut scene = self.scene.borrow_mut();
        let existing = scene.local_light(&patch.key).cloned();
        let inserted = existing.is_none();
        let mut light = existing.unwrap_or_else(|| LocalLight {
            key: patch.key.clone(),
            ..LocalLight::default()
        });
        if let Some(v) = kind {
            light.kind = v;
        }
        if let Some(v) = patch.pos {
            light.pos = v;
        }
        if let Some(v) = patch.dir {
            light.dir = v;
        }
        if let Some(v) = patch.color {
            light.color = v;
        }
        if let Some(v) = patch.intensity {
            light.intensity = v;
        }
        if let Some(v) = patch.range {
            light.range = v;
        }
        if let Some(v) = patch.inner_deg {
            light.inner_deg = v;
        }
        if let Some(v) = patch.outer_deg {
            light.outer_deg = v;
        }
        if let Some(v) = patch.cast_shadow {
            light.cast_shadow = v;
        }
        if let Some(v) = patch.shadow_bias {
            light.shadow_bias = v;
        }
        let applied = light.sanitized();
        // `upsert_local_light` 满载时只拒绝「新增」，同 key 覆盖照常生效——
        // 因此这里的失败一定是容量问题，报错文案直接点明上限。
        if !scene.upsert_local_light(applied) {
            return Err(RpcFailure::invalid_params(format!(
                "局部光源已达上限 {MAX_LOCAL_LIGHTS}，无法新增；请先删除或复用已有 key"
            )));
        }
        let value = scene
            .local_light(&patch.key)
            .map(local_light_value)
            .unwrap_or(Value::Null);
        Ok(json!({
            "key": patch.key,
            "inserted": inserted,
            "count": scene.lights.len(),
            "light": value,
        }))
    }

    fn light_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let removed = scene.remove_local_light(key);
        Ok(json!({ "key": key, "removed": removed, "count": scene.lights.len() }))
    }

    fn light_clear(&mut self) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let removed = scene.clear_local_lights();
        Ok(json!({ "removed": removed, "count": 0 }))
    }

    fn light_set_enabled(&mut self, enabled: Option<bool>) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        if let Some(on) = enabled {
            scene.lights_enabled = on;
        }
        Ok(json!({
            "enabled": scene.lights_enabled,
            "count": scene.lights.len(),
        }))
    }

    /// 空间化音频列表（§12 第十项）：声明态 + 上一帧结算结果一起给。
    ///
    /// 这里**借用 `ScriptHost` 的 Audio 句柄**而不是另存一份：脚本、编辑器、帧驱动
    /// 三方必须看到同一张声源表，否则面板上调完的参数会被脚本下一次 attach 覆盖掉。
    fn audio_spatial_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        let audio = self.scripts.audio_handle();
        let a = audio.borrow();
        let sources: Vec<Value> = a
            .spatial_keys()
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(|key| spatial_source_value(&a, key))
            .collect();
        let listener = a.listener();
        Ok(json!({
            "enabled": a.spatial_enabled(),
            "count": a.spatial_count(),
            "limit": MAX_SPATIAL_SOURCES,
            "listener": {
                "position": listener.position,
                "forward": listener.forward,
            },
            "sources": sources,
        }))
    }

    /// 注册 / 局部更新一个声源：模型名与距离先解析完再落地，写错的名字不留半改状态。
    fn audio_spatial_set(&mut self, patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
        if patch.key.trim().is_empty() {
            return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
        }
        let model = match patch.model.as_deref() {
            Some(name) => Some(DistanceModel::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "未知距离模型 `{name}`（可用：{}）",
                    DistanceModel::all()
                        .iter()
                        .map(|m| m.name())
                        .collect::<Vec<_>>()
                        .join(" / ")
                ))
            })?),
            None => None,
        };

        let audio = self.scripts.audio_handle();
        let mut a = audio.borrow_mut();
        let existed = a.spatial_settings(&patch.key);
        let inserted = existed.is_none();
        let mut settings = existed.unwrap_or_default();
        if let Some(v) = patch.min_distance {
            settings.min_distance = v;
        }
        if let Some(v) = patch.max_distance {
            settings.max_distance = v;
        }
        if let Some(v) = model {
            settings.model = v;
        }
        if let Some(v) = patch.doppler {
            settings.doppler_factor = v;
        }
        // `attach_spatial` 内部会走 `sanitized()`，并把已有声源就地更新（不清位置）。
        // 满载时它只拒绝「新增」，同 key 覆盖照常生效——因此失败一定是容量问题。
        if !a.attach_spatial(&patch.key, settings) {
            return Err(RpcFailure::invalid_params(format!(
                "空间化声源已达上限 {MAX_SPATIAL_SOURCES}，无法新增；请先删除或复用已有 key"
            )));
        }
        if let Some(pos) = patch.position {
            a.set_source_position(&patch.key, pos);
        }
        if let Some(node) = patch.node.as_deref() {
            // 空串 = 解绑（回到「只按手动 position 算」），与脚本侧 `audio_spatial_attach_node`
            // 的语义一致：传空节点名就是清绑定。
            let settings = a.spatial_settings(&patch.key).unwrap_or_default();
            if !a.attach_spatial_node(&patch.key, node, settings) {
                return Err(RpcFailure::invalid_params(format!(
                    "声源 `{}` 注册失败，节点 `{node}` 未绑定",
                    patch.key
                )));
            }
        }
        Ok(json!({
            "key": patch.key,
            "inserted": inserted,
            "count": a.spatial_count(),
            "source": spatial_source_value(&a, &patch.key),
        }))
    }

    fn audio_spatial_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        let audio = self.scripts.audio_handle();
        let mut a = audio.borrow_mut();
        let removed = a.detach_spatial(key);
        Ok(json!({ "key": key, "removed": removed, "count": a.spatial_count() }))
    }

    fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
        let audio = self.scripts.audio_handle();
        let mut a = audio.borrow_mut();
        let removed = a.spatial_count();
        a.clear_spatial();
        Ok(json!({ "removed": removed, "count": 0 }))
    }

    fn audio_spatial_listener(
        &mut self,
        patch: Option<ListenerPatch>,
    ) -> Result<Value, RpcFailure> {
        let audio = self.scripts.audio_handle();
        let mut a = audio.borrow_mut();
        if let Some(patch) = patch {
            let mut pose = a.listener();
            if let Some(pos) = patch.position {
                pose.position = pos;
            }
            if let Some(forward) = patch.forward {
                // 零向量是无效朝向（归一化会炸成 NaN），当作没给
                if !forward.iter().all(|v| *v == 0.0) {
                    pose.forward = forward;
                }
            }
            a.set_listener(pose);
            // 立刻结算一次：编辑器改完听者想马上看到增益变化，不该等下一帧
            a.tick_spatial(0.0);
        }
        let listener = a.listener();
        Ok(json!({
            "position": listener.position,
            "forward": listener.forward,
            "count": a.spatial_count(),
        }))
    }

    fn audio_spatial_set_enabled(&mut self, enabled: Option<bool>) -> Result<Value, RpcFailure> {
        let audio = self.scripts.audio_handle();
        let mut a = audio.borrow_mut();
        if let Some(on) = enabled {
            a.set_spatial_enabled(on);
            a.tick_spatial(0.0);
        }
        Ok(json!({
            "enabled": a.spatial_enabled(),
            "count": a.spatial_count(),
        }))
    }

    fn scene_shadow(&mut self, patch: Option<ShadowPatch>) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let Some(patch) = patch else {
            // 只读：直接回当前生效值，不做任何收敛（它本来就已经是收敛过的）
            return Ok(shadow_value(&scene.light.shadow));
        };
        let mut shadow = scene.light.shadow;
        if let Some(v) = patch.enabled {
            shadow.enabled = v;
        }
        if let Some(v) = patch.size {
            shadow.size = v;
        }
        if let Some(v) = patch.strength {
            shadow.strength = v;
        }
        if let Some(v) = patch.ortho_size {
            shadow.ortho_size = v;
        }
        if let Some(v) = patch.bias {
            shadow.bias = v;
        }
        if let Some(v) = patch.cascades {
            shadow.cascade.count = v;
        }
        if let Some(v) = patch.blend {
            shadow.cascade.blend = v;
        }
        if let Some(v) = patch.distance {
            shadow.cascade.max_distance = v;
        }
        // 一次性收敛：级数越界 / 混合比越界 / 尺寸非 2 的幂都在这里归位，
        // 免得「编辑器改完看起来生效了，渲染器又按另一套规则夹一遍」。
        shadow = shadow.sanitized();
        scene.light.shadow = shadow;
        Ok(shadow_value(&shadow))
    }

    fn script_eval(&mut self, source: &str) -> Result<Value, RpcFailure> {
        let value = self.scripts.eval(source).map_err(RpcFailure::engine)?;
        Ok(dynamic_to_json(&value))
    }

    fn ui_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        let scene = self.scene.borrow();
        let items: Vec<Value> = scene
            .ui
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(ui_element_value)
            .collect();
        Ok(json!({
            "count": scene.ui.len(),
            "limit": ui::MAX_UI_ELEMENTS,
            "enabled": scene.ui_enabled,
            "elements": items,
        }))
    }

    fn ui_set(&mut self, patch: UiPatch) -> Result<Value, RpcFailure> {
        if patch.key.trim().is_empty() {
            return Err(RpcFailure::invalid_params("参数 `key` 不能为空"));
        }
        // 名字先全部解析完再落地：一个写错的锚点不该留下半条被改的元素。
        let kind = match patch.kind.as_deref() {
            Some(name) => Some(UiKind::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "未知元素类型 `{name}`（可用：rect / border / image）"
                ))
            })?),
            None => None,
        };
        let anchor = match patch.anchor.as_deref() {
            Some(name) => Some(Anchor::parse(name).ok_or_else(|| {
                RpcFailure::invalid_params(format!(
                    "未知锚点 `{name}`（可用：top_left / center ...）"
                ))
            })?),
            None => None,
        };

        let mut scene = self.scene.borrow_mut();
        let existing = ui::find(&scene.ui, &patch.key).cloned();
        let inserted = existing.is_none();
        let mut element = existing.unwrap_or_else(|| UiElement {
            key: patch.key.clone(),
            ..UiElement::default()
        });
        apply_ui_patch(&mut element, &patch, kind, anchor);
        // 用 `upsert`（裸全量覆盖）而不是 `upsert_styled`：RPC 的语义是「给了才改」，
        // 改动已经在 clone 上做完了；`merge_same_key` 会强制沿用旧的
        // `layer` / `visible` / `uv`，那是脚本每帧重发位置时该有的行为，放在这里会把
        // 编辑器对层级 / 可见性的修改吞掉。
        let outcome = ui::upsert(&mut scene.ui, element.sanitized());
        if outcome == UiUpsert::Rejected {
            return Err(RpcFailure::invalid_params(format!(
                "UI 元素已达上限 {}，无法新增",
                ui::MAX_UI_ELEMENTS
            )));
        }
        let value = ui::find(&scene.ui, &patch.key)
            .map(ui_element_value)
            .unwrap_or(Value::Null);
        Ok(json!({
            "key": patch.key,
            "inserted": inserted,
            "replaced": outcome == UiUpsert::Replaced,
            "count": scene.ui.len(),
            "element": value,
        }))
    }

    fn ui_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
        let mut scene = self.scene.borrow_mut();
        let removed = ui::remove(&mut scene.ui, key);
        Ok(json!({ "key": key, "removed": removed, "count": scene.ui.len() }))
    }

    fn ui_state(&mut self) -> Result<Value, RpcFailure> {
        let interaction = self.scripts.ui_interaction();
        let state = interaction.borrow();
        let events: Vec<Value> = state
            .events()
            .iter()
            .map(|e| json!({ "kind": e.kind.name(), "key": e.key }))
            .collect();
        let (sw, sh) = state.screen();
        Ok(json!({
            "hover": state.hover(),
            "active": state.active(),
            "pointer_over_ui": state.pointer_over_ui(),
            "screen": [sw, sh],
            "events": events,
        }))
    }

    fn scene_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        // 目标路径：显式给了就用它，否则写回本场景的原始文件（编辑器「Ctrl+S」语义）。
        let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None => {
                if self.scene_label.trim().is_empty() {
                    return Err(RpcFailure::invalid_params(
                        "当前没有场景文件路径，请显式给出 `path`",
                    ));
                }
                self.scene_label.clone()
            }
        };
        let scene = self.scene.borrow();
        scene.save_to_path(&target).map_err(RpcFailure::engine)?;
        // 相对路径按进程工作目录解析：回给调用方的是绝对路径与实际字节数
        let abs =
            std::fs::canonicalize(&target).unwrap_or_else(|_| std::path::PathBuf::from(&target));
        let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        Ok(json!({
            "path": plain_abs_path(&abs),
            "objects": scene.len(),
            "bytes": bytes,
        }))
    }

    /// 列出场景栈（扩展十八 B）。
    ///
    /// 每层只给**声明态**（层号 / 名字 / 来源路径 / 源文本字节数）：面板刷新每帧都在发，
    /// 真把十几层的场景 JSON 全塞进一个响应，网络和 Rust 侧的序列化都会变成瓶颈。
    fn scene_stack(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
        let handle = self.scripts.scene_stack_handle();
        let stack = handle.borrow();
        let total = stack.depth();
        let take = limit.unwrap_or(total).min(total);
        let current = stack.current_index();
        let entries: Vec<Value> = stack
            .entries()
            .iter()
            .enumerate()
            .take(take)
            .map(|(index, entry)| {
                json!({
                    "index": index,
                    "name": entry.name,
                    "path": entry.path,
                    "source_bytes": entry.source.len(),
                    "frame": entry.runtime.frame,
                    "time": entry.runtime.time,
                    "current": current == Some(index),
                })
            })
            .collect();
        Ok(json!({
            "count": total,
            "current": current,
            "max": MAX_SCENE_STACK,
            "entries": entries,
        }))
    }

    /// 请求切到某个场景文件：只做「路径存在性校验 + 排队」，真正换世界由宿主帧首执行。
    fn scene_switch(&mut self, path: &str) -> Result<Value, RpcFailure> {
        let target = resolve_request_path(self.scene_label.as_str(), path);
        if !target.is_file() {
            return Err(RpcFailure::invalid_params(format!(
                "场景文件不存在：{}",
                target.display()
            )));
        }
        queue_scene_command(
            self.scripts,
            SceneCommand::switch(target.display().to_string()),
        )
    }

    /// 请求回到栈里的上一层（不删层）。
    fn scene_back(&mut self) -> Result<Value, RpcFailure> {
        // 先看一眼栈：已经在最早一层时立刻报错，比"排队成功但下一帧静默失败"好得多
        let blocked = {
            let handle = self.scripts.scene_stack_handle();
            let stack = handle.borrow();
            match stack.current_index() {
                Some(0) | None => true,
                Some(_) => false,
            }
        };
        if blocked {
            return Err(RpcFailure::engine(
                "已在最早一层，无法回退（可用 `scene.stack` 查看栈）",
            ));
        }
        queue_scene_command(self.scripts, SceneCommand::back())
    }

    /// 存一份带运行时快照的存档。
    ///
    /// 存档只写盘、不动世界，所以**当场写完**再返回真实路径：调用方拿到的是
    /// 「文件已存在」这一确定结果，而不是一句"排队成功"。
    fn scene_archive_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
        let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                if self.scene_label.trim().is_empty() {
                    return Err(RpcFailure::invalid_params(
                        "当前没有场景文件路径，请显式给出 `path`",
                    ));
                }
                crate::app::default_archive_path(std::path::Path::new(&self.scene_label))
            }
        };
        if let Some(dir) = target.parent() {
            if !dir.as_os_str().is_empty() && !dir.exists() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| RpcFailure::engine(format!("创建存档目录失败: {e}")))?;
            }
        }
        let archive = SceneArchive::from_scene(&self.scene.borrow());
        archive.save_to_path(&target).map_err(RpcFailure::engine)?;
        let abs =
            std::fs::canonicalize(&target).unwrap_or_else(|_| std::path::PathBuf::from(&target));
        let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        Ok(json!({
            "path": plain_abs_path(&abs),
            "format": ARCHIVE_FORMAT,
            "version": ARCHIVE_VERSION,
            "bytes": bytes,
            "objects": archive.scene.objects.len(),
            "anims": archive.runtime.as_ref().map(|r| r.anims.len()).unwrap_or(0),
            "queued": false,
        }))
    }

    /// 请求读档：与 `scene_switch` 同理，只校验 + 排队。
    fn scene_archive_load(&mut self, path: &str) -> Result<Value, RpcFailure> {
        let target = resolve_request_path(self.scene_label.as_str(), path);
        if !target.is_file() {
            return Err(RpcFailure::invalid_params(format!(
                "存档文件不存在：{}",
                target.display()
            )));
        }
        queue_scene_command(
            self.scripts,
            SceneCommand::load_archive(target.display().to_string()),
        )
    }

    fn script_reload(&mut self) -> Result<Value, RpcFailure> {
        let path = self
            .scripts
            .script_path()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| RpcFailure::engine("当前宿主没有载入脚本文件，无法重载"))?;
        self.scripts.load_file(&path).map_err(RpcFailure::engine)?;
        Ok(json!({ "reloaded": path.display().to_string() }))
    }
}

/// 后处理快照：字段名与场景 JSON 的 `postfx` 段完全一致，前端拿到即可回写。
fn postfx_value(fx: PostFxSettings) -> Value {
    json!({
        "exposure": fx.exposure,
        "tonemap": fx.tonemap.name(),
        "bloom": {
            "enabled": fx.bloom.enabled,
            "threshold": fx.bloom.threshold,
            "knee": fx.bloom.knee,
            "intensity": fx.bloom.intensity,
        },
        "fxaa": fx.fxaa,
        "ssao": ssao_value(fx.ssao),
    })
}

/// 遮蔽快照：字段名与场景 JSON 的 `postfx.ssao` 段一致，前端拿到即可回写。
/// `neutral` 是渲染侧真正用的判据（关闭或强度为 0 时为 true，此时整帧跳过两个 pass），
/// 单看 `enabled` 会把「开了但强度 0」误报成在算遮蔽。
fn ssao_value(s: SsaoSettings) -> Value {
    json!({
        "enabled": s.enabled,
        "radius": s.radius,
        "bias": s.bias,
        "intensity": s.intensity,
        "samples": s.samples,
        "neutral": s.is_neutral(),
    })
}

/// 先按名字找，再按 `NodeId` 数字找：编辑器两种句柄都能用。
fn find_node<'s>(scene: &'s Scene, key: &str) -> Result<&'s Node, RpcFailure> {
    if let Some(node) = scene.find(key) {
        return Ok(node);
    }
    if let Ok(id) = key.trim().parse::<u64>() {
        if let Some(node) = scene.get(id) {
            return Ok(node);
        }
    }
    Err(RpcFailure::not_found(format!("物件 `{key}` 不存在")))
}

fn find_node_id(scene: &Scene, key: &str) -> Result<r3e_core::NodeId, RpcFailure> {
    if let Some(id) = scene.find_id(key) {
        return Ok(id);
    }
    if let Ok(id) = key.trim().parse::<u64>() {
        if scene.get(id).is_some() {
            return Ok(id);
        }
    }
    Err(RpcFailure::not_found(format!("物件 `{key}` 不存在")))
}

/// 单个物件的完整快照（`scene.get` 的结果）。
fn node_value(node: &Node) -> Value {
    let t = &node.transform;
    json!({
        "id": node.id,
        "name": node.name,
        "mesh": node.mesh.as_string(),
        "visible": node.visible,
        "parent": node.parent,
        "children": node.children,
        "position": t.position.to_array(),
        "rotation": t.rotation.to_array(),
        "scale": t.scale.to_array(),
        "euler_deg": t.euler_deg().to_array(),
        "color": node.color.to_array(),
        "material": node.material.map(material_value),
        "world": node.world.to_cols_array(),
    })
}

/// 材质快照（读回入口）：未显式设置材质的物件给出 `null`，
/// 显式设置过的给出收敛后的三件套。
/// 环境探针快照：字段名与场景 JSON 的 `environment` 段完全一致，前端拿到即可回写。
fn environment_value(env: r3e_core::env::EnvironmentSettings) -> Value {
    json!({
        "sky_zenith": env.sky_zenith,
        "sky_horizon": env.sky_horizon,
        "ground": env.ground,
        "sun_dir": env.sun_dir,
        "sun_color": env.sun_color,
        "sun_intensity": env.sun_intensity,
        "intensity": env.intensity,
    })
}

/// 反射探针快照（扩展二十三）：字段名与场景 JSON 的 `environment_map` 段一致，
/// 前端拿到即可回写；`source` 单独给在响应里，不在这个对象内重复。
fn environment_map_value(map: r3e_core::envmap::EnvironmentMapSettings) -> Value {
    json!({
        "enabled": map.enabled,
        "equirect": map.equirect,
        "faces": map.faces,
        "face_size": map.face_size,
        "mip_levels": map.mip_levels,
        "intensity": map.intensity,
    })
}

fn material_value(material: r3e_core::SurfaceMaterial) -> Value {
    json!({
        "metallic": material.metallic,
        "roughness": material.roughness,
        "emissive": material.emissive,
    })
}

fn check_finite<const N: usize>(values: &[f32; N], key: &str) -> Result<(), RpcFailure> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 含 NaN / 无穷大"
        )))
    }
}

/// Rhai 返回值转 JSON：普通标量 / 数组 / Map 走 serde 反序列化，
/// 带自定义类型的返回值退化成字符串，保证 `script.eval` 永远有结果可回。
fn dynamic_to_json(value: &rhai::Dynamic) -> Value {
    if value.is_unit() {
        return Value::Null;
    }
    match rhai::serde::from_dynamic::<Value>(value) {
        Ok(v) => v,
        Err(_) => Value::String(value.to_string()),
    }
}

/// 把 Windows 上 `canonicalize` 得到的 `\\?\` 逐字前缀去掉。
///
/// 编辑器客户端拿到 `\\?\D:\a\b.json` 这种路径无法直接展示 / 再拼接，
/// 而 `\\?\UNC\` 前缀则需要还原成 `\\`。
fn plain_abs_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

/// 把一条场景命令塞进脚本宿主的命令槽（单槽：一帧一条）。
///
/// 槽被占时如实回 `server busy`，而不是覆盖掉别人那条——脚本刚在 `on_start` 里
/// 申请的切场景如果被编辑器面板冲掉，表现就是"点了没反应"，比报错更难查。
fn queue_scene_command(scripts: &ScriptHost, cmd: SceneCommand) -> Result<Value, RpcFailure> {
    let label = cmd.label();
    if !scripts.request_scene_command(cmd) {
        return Err(RpcFailure::new(
            SERVER_BUSY,
            format!("场景命令槽已被占用，`{label}` 未排队，请下一帧重试"),
        ));
    }
    let handle = scripts.scene_stack_handle();
    let depth = handle.borrow().depth();
    Ok(json!({
        "queued": true,
        "command": label,
        "stack": depth,
    }))
}

/// 解析请求里的路径：绝对路径照用；相对路径先按工作目录，再按"当前场景文件所在目录"。
///
/// 后一条是给编辑器用的：面板里填 `lv2.json` 时，用户心里那个"当前目录"是场景目录，
/// 不是进程启动目录。两条都落空时返回原路径（由调用方按"不存在"报错）。
fn resolve_request_path(scene_label: &str, path: &str) -> std::path::PathBuf {
    let raw = std::path::PathBuf::from(path);
    if raw.is_absolute() || raw.exists() {
        return raw;
    }
    let beside_scene = std::path::Path::new(scene_label)
        .parent()
        .map(|dir| dir.join(&raw))
        .unwrap_or_else(|| raw.clone());
    if beside_scene.exists() {
        beside_scene
    } else {
        raw
    }
}

/// 把 `UiPatch` 的字段并进一个元素（只覆盖给了的字段）。
///
/// `kind` / `anchor` 已由调用方解析完毕；这里只做搬运与单位解析，不做任何策略判断。
fn apply_ui_patch(
    el: &mut UiElement,
    patch: &UiPatch,
    kind: Option<UiKind>,
    anchor: Option<Anchor>,
) {
    if let Some(kind) = kind {
        el.kind = kind;
    }
    if let Some(anchor) = anchor {
        el.anchor = anchor;
    }
    if let Some(text) = patch.x.as_deref() {
        el.offset[0] = parse_ui_value(text);
    }
    if let Some(text) = patch.y.as_deref() {
        el.offset[1] = parse_ui_value(text);
    }
    if let Some(text) = patch.width.as_deref() {
        el.size[0] = parse_ui_value(text);
    }
    if let Some(text) = patch.height.as_deref() {
        el.size[1] = parse_ui_value(text);
    }
    if let Some(color) = patch.color {
        el.color = color;
    }
    if let Some(layer) = patch.layer {
        el.layer = layer;
    }
    if let Some(visible) = patch.visible {
        el.visible = visible;
    }
    if let Some(width) = patch.border_width {
        el.border_width = width;
    }
    if let Some(image) = patch.image.as_deref() {
        el.image = image.to_string();
    }
}

/// 单位串 → `UiValue`：`"12"` / `"12px"` 为像素，`"50%"` 为屏幕百分比。
///
/// 解析失败折成 0 与脚本层同一口径——RPC 调用方拿到的是「渲染结果」而非异常，
/// 与 `scene.set_material` 对非法数值的处理保持一致。
fn parse_ui_value(text: &str) -> UiValue {
    let raw = text.trim();
    if let Some(num) = raw.strip_suffix('%') {
        return UiValue::pct(num.trim().parse::<f32>().unwrap_or(0.0));
    }
    let num = raw
        .strip_suffix("px")
        .unwrap_or(raw)
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    UiValue::px(num)
}

/// `UiValue` → 单位串（`"12"` / `"50%"`），供 RPC 回读。
fn fmt_ui_value(value: UiValue) -> String {
    match value.unit {
        UiUnit::Px => format!("{}", value.value),
        UiUnit::Percent => format!("{}%", value.value),
    }
}

/// 单个 UI 元素的 RPC 表示（声明层，未折算像素）。
/// RPC 回包用的「一套阴影设置」JSON 形态。
///
/// 字段名沿用场景文件里的驼峰写法（`orthoSize`），`cascades` / `maxCascades` 是
/// 扩展十六新增项；`maxCascades` 让编辑器不必自己硬编码上限就能画对滑块范围。
fn shadow_value(shadow: &ShadowSettings) -> Value {
    json!({
        "enabled": shadow.enabled,
        "size": shadow.size,
        "strength": shadow.strength,
        "orthoSize": shadow.ortho_size,
        "bias": shadow.bias,
        "normalBias": shadow.normal_bias,
        "cascades": shadow.cascade.count,
        "maxCascades": MAX_CASCADES,
        "blend": shadow.cascade.blend,
        "distance": shadow.cascade.max_distance,
        "lambda": shadow.cascade.lambda,
    })
}

/// 层不存在时的统一报错（`layer` 给空串表示第一层）。
fn layer_missing(instance: &str, layer: &str) -> RpcFailure {
    let which = if layer.trim().is_empty() {
        "（空名 = 第一层）".to_string()
    } else {
        format!("`{layer}`")
    };
    RpcFailure::invalid_params(format!("实例 `{instance}` 上没有名为 {which} 的层"))
}

/// 遮罩字符串 → [`BoneMask`]，规则与脚本侧 `parse_bone_mask` 完全一致：
/// `"arm,hand"` 只作用这几处，`"!root"` 除 root 外全部，空串 / `"*"` / 只有分隔符
/// 表示整副骨架——归一成 `pass_all` 而不是空遮罩，空遮罩会让整层都不出效果。
fn bone_mask_of(spec: &str) -> BoneMask {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return BoneMask::pass_all();
    }
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for part in trimmed.split(',') {
        let rule = part.trim();
        if rule.is_empty() {
            continue;
        }
        match rule.strip_prefix('!') {
            Some(rest) => {
                let name = rest.trim();
                // 只有 `!` 没有名字的规则直接丢掉，不造一条空排除规则。
                if !name.is_empty() {
                    exclude.push(name.to_string());
                }
            }
            None => include.push(rule.to_string()),
        }
    }
    if include.is_empty() && exclude.is_empty() {
        BoneMask::pass_all()
    } else {
        BoneMask { include, exclude }
    }
}

/// 把一层混合树序列化成编辑器契约的 JSON。
///
/// `weight` 是层自己的目标权重，`effective_weight` 才含淡入淡出的当前值——
/// 编辑器画「目标 vs 实际」两条线时两个都要。
fn anim_layer_value(instance: &str, layer: &AnimLayer) -> Value {
    let mask = if layer.mask.is_pass_all() {
        "*".to_string()
    } else {
        let mut parts = layer.mask.include.clone();
        parts.extend(layer.mask.exclude.iter().map(|rule| format!("!{rule}")));
        parts.join(",")
    };
    let weights = layer.normalized_weights();
    let clips: Vec<Value> = layer
        .clips
        .iter()
        .zip(weights)
        .map(|(lc, w)| {
            json!({
                "name": lc.clip.name,
                "base_weight": lc.weight,
                "weight": w,
            })
        })
        .collect();
    json!({
        "instance": instance,
        "layer": layer.name,
        "mode": layer.mode.as_str(),
        "weight": layer.weight,
        "effective_weight": layer.effective_weight(),
        "fading": layer.fade.is_some(),
        "mask": mask,
        "mask_include": layer.mask.include.clone(),
        "mask_exclude": layer.mask.exclude.clone(),
        "clips": clips,
        "time": layer.sample_time(),
        "duration": layer.duration(),
        "speed": layer.speed,
        "looping": layer.looping,
        "playing": layer.playing,
        // 扩展四十六：采样策略 + 当前相位（`phase` 不受 `off` 影响，始终可读，
        // 编辑器画相位标尺时不用管策略开关）。
        "sync": layer.sync.as_str(),
        "phase": layer.phase(),
    })
}

/// RPC / 脚本共用的「一盏灯」JSON 形态。
///
/// 字段与 `LocalLight` 一一对应，`kind` 用规范名而不是数字码——编辑器面板要能
/// 直接显示 `spot`，也不想为了一个枚举在两侧同步一份常量表。
fn local_light_value(light: &LocalLight) -> Value {
    json!({
        "key": light.key,
        "kind": light.kind.name(),
        "pos": light.pos,
        "dir": light.dir,
        "color": light.color,
        "intensity": light.intensity,
        "range": light.range,
        "inner_deg": light.inner_deg,
        "outer_deg": light.outer_deg,
        "cast_shadow": light.cast_shadow,
        "shadow_bias": light.shadow_bias,
    })
}

/// 一个声源的完整快照：声明态（编辑器改的）+ 结算态（上一帧算出来的）。
///
/// 结算字段缺失时用 `null`，不伪造 `1.0`——「这一帧没算过」和「算出来正好是 1」
/// 对排查未注册 / 静默后端这两类问题完全是两回事。
fn spatial_source_value(audio: &Audio, key: &str) -> Value {
    let settings = audio.spatial_settings(key);
    json!({
        "key": key,
        "position": audio.source_position(key),
        "node": audio.spatial_node(key),
        "distance": audio.spatial_distance(key),
        "gain": settings.map(|_| audio.spatial_gain(key)),
        "rate": settings.map(|_| audio.spatial_rate(key)),
        "min_distance": settings.map(|s| s.min_distance),
        "max_distance": settings.map(|s| s.max_distance),
        "model": settings.map(|s| s.model.name()),
        "doppler": settings.map(|s| s.doppler_factor),
        "speed_of_sound": settings.map(|s| s.speed_of_sound),
    })
}

fn ui_element_value(el: &UiElement) -> Value {
    json!({
        "key": el.key,
        "kind": el.kind.name(),
        "anchor": el.anchor.name(),
        "x": fmt_ui_value(el.offset[0]),
        "y": fmt_ui_value(el.offset[1]),
        "width": fmt_ui_value(el.size[0]),
        "height": fmt_ui_value(el.size[1]),
        "color": el.color,
        "border_width": el.border_width,
        "image": el.image,
        "layer": el.layer,
        "visible": el.visible,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::anim::{AnimBinding, AnimInstance};
    use r3e_core::light::{MAX_LOCAL_SHADOW_BIAS, MIN_LIGHT_RANGE};
    use r3e_core::{AnimClip, AnimKey, AnimTrack, InputState};
    use std::sync::Arc;

    type Fixture = (Rc<RefCell<Scene>>, Rc<RefCell<Physics>>, ScriptHost);

    fn fixture() -> Fixture {
        let mut scene = Scene::new();
        scene.name = "editor-test".to_string();
        for name in ["cube", "sphere", "cube_2"] {
            scene
                .spawn(ObjectDesc {
                    name: name.to_string(),
                    mesh: Some("cube".to_string()),
                    ..Default::default()
                })
                .expect("测试场景 spawn 不应失败");
        }
        let scene = Rc::new(RefCell::new(scene));
        let physics = Rc::new(RefCell::new(Physics::new()));
        let scripts = ScriptHost::new(
            scene.clone(),
            physics.clone(),
            Rc::new(RefCell::new(InputState::default())),
        );
        (scene, physics, scripts)
    }

    fn api<'a>(
        scene: &'a Rc<RefCell<Scene>>,
        physics: &'a Rc<RefCell<Physics>>,
        scripts: &'a mut ScriptHost,
    ) -> LocalEngineApi<'a> {
        LocalEngineApi {
            scene,
            physics,
            scripts,
            // 单元测试没有窗口 / GPU：渲染器缺席正是「headless 也得给出可解释结果」的场景。
            renderer: None,
            frame_ms: FrameTimeStats {
                samples: 4,
                last_ms: 16.0,
                p50_ms: 16.0,
                p95_ms: 18.5,
                p99_ms: 20.0,
                max_ms: 20.0,
            },
            capture_dir: PathBuf::from("assets/captures"),
            scene_label: "assets/scene.json".to_string(),
            script_label: "scripts/main.rhai".to_string(),
            frames: 42,
            fps: 60.0,
            uptime_ms: 700,
            paused: false,
        }
    }

    #[test]
    fn info_reports_scene_and_physics() {
        let (scene, physics, mut scripts) = fixture();
        let info = api(&scene, &physics, &mut scripts).info();
        assert_eq!(info.name, "r3e");
        assert_eq!(info.protocol, PROTOCOL_VERSION);
        assert_eq!(info.nodes, 3);
        assert_eq!(info.frames, 42);
        assert_eq!(info.scene, "assets/scene.json");
        assert!(!info.physics.backend.is_empty());
    }

    #[test]
    fn scene_list_filters_and_limits() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let all = api.scene_list(None, None).expect("应成功");
        assert_eq!(all.len(), 3);

        let cubes = api.scene_list(Some("cube"), None).expect("应成功");
        assert_eq!(cubes.len(), 2);
        assert!(cubes.iter().all(|n| n.name.contains("cube")));

        // 大小写不敏感
        let upper = api.scene_list(Some("CUBE"), None).expect("应成功");
        assert_eq!(upper.len(), 2);

        let limited = api.scene_list(None, Some(2)).expect("应成功");
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn set_transform_writes_local_transform() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let patched = api
            .scene_set_transform(
                "cube",
                TransformPatch {
                    position: Some([1.0, 2.0, 3.0]),
                    rotation: Some([0.0, 0.0, 0.0, 1.0]),
                    scale: Some([2.0, 2.0, 2.0]),
                },
            )
            .expect("应成功");
        assert_eq!(patched["position"], json!([1.0, 2.0, 3.0]));

        let node = scene.borrow().find("cube").expect("物件应存在").clone();
        assert_eq!(node.transform.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(node.transform.scale, Vec3::splat(2.0));
        // 世界矩阵已按新局部变换刷新
        assert!((node.world.w_axis.x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn set_transform_rejects_bad_input() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let zero_quat = api.scene_set_transform(
            "cube",
            TransformPatch {
                rotation: Some([0.0, 0.0, 0.0, 0.0]),
                ..Default::default()
            },
        );
        assert!(zero_quat.is_err(), "全零四元数必须被拒绝");

        let missing = api.scene_set_transform(
            "not-there",
            TransformPatch {
                position: Some([0.0, 0.0, 0.0]),
                ..Default::default()
            },
        );
        assert!(missing.is_err(), "不存在的物件必须报错");
    }

    #[test]
    fn scene_save_writes_requested_path() {
        let (scene, physics, mut scripts) = fixture();
        let expected = scene.borrow().len();
        let dir = std::env::temp_dir().join(format!("r3e-editor-save-{}", std::process::id()));
        let path = dir.join("nested/out.json");
        let literal = path.display().to_string().replace('\\', "/");

        let mut api = api(&scene, &physics, &mut scripts);
        let saved = api.scene_save(Some(&literal)).expect("显式路径保存应成功");
        assert_eq!(saved["objects"], json!(expected), "物件数应与场景一致");
        assert!(saved["bytes"].as_u64().unwrap_or(0) > 0, "应报告写入字节数");
        let reported = saved["path"].as_str().unwrap_or_default();
        assert!(
            !reported.contains(r"\\?\"),
            "回报路径不应带 Windows 逐字前缀: {reported}"
        );
        assert!(
            reported.ends_with("out.json"),
            "回报路径应是绝对路径: {reported}"
        );

        let text = std::fs::read_to_string(&path).expect("文件应已落盘");
        assert!(text.contains("cube"), "落盘内容应含物件名");
        assert!(!text.contains("world"), "不应写入运行时派生状态");
        let back = Scene::from_json_str(&text).expect("落盘内容必须能回读");
        assert_eq!(back.len(), expected, "回读后物件数应一致");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scene_save_without_path_needs_label() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.scene_label = String::new();
        let err = api
            .scene_save(None)
            .expect_err("无路径且无场景文件时应报错");
        assert!(
            format!("{err:?}").contains("path"),
            "错误信息应提示需要 path: {err:?}"
        );

        // 空白路径等同未给
        assert!(api.scene_save(Some("   ")).is_err(), "空白 path 应被拒绝");
    }

    #[test]
    fn spawn_and_remove_roundtrip() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let spawned = api
            .scene_spawn(json!({ "name": "probe", "mesh": "sphere", "position": [0.0, 1.0, 0.0] }))
            .expect("应成功");
        assert_eq!(spawned["name"], "probe");
        assert_eq!(spawned["nodes"], 4);

        let snapshot = api.scene_get("probe").expect("应成功");
        assert_eq!(snapshot["mesh"], "sphere");
        assert_eq!(snapshot["position"], json!([0.0, 1.0, 0.0]));

        // 重名必须报错，不能悄悄覆盖
        assert!(api
            .scene_spawn(json!({ "name": "probe", "mesh": "cube" }))
            .is_err());

        let removed = api.scene_remove("probe").expect("应成功");
        assert_eq!(removed["removed"], "probe");
        assert_eq!(scene.borrow().len(), 3);
    }

    #[test]
    fn script_eval_returns_json() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        assert_eq!(api.script_eval("1 + 1").expect("应成功"), json!(2));
        assert_eq!(api.script_eval("\"r3e\"").expect("应成功"), json!("r3e"));
        assert_eq!(api.script_eval("()").expect("应成功"), Value::Null);
        assert!(api.script_eval("this is not rhai").is_err());
        // 脚本侧改场景后，scene.* 立即可见（同一个 Scene 实例）
        api.script_eval("create_node_at_origin(\"from_script\", \"cube\")")
            .expect("应成功");
        assert!(scene.borrow().find("from_script").is_some());
    }

    #[test]
    fn set_postfx_merges_and_sanitizes() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        // 逐项给：未给的字段保持原值（首次从未开启的缺省起步）。
        let first = api
            .scene_set_postfx(PostFxPatch {
                exposure: Some(0.5),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(first["enabled"], json!(true));
        assert_eq!(first["postfx"]["exposure"], json!(0.5));
        assert_eq!(first["postfx"]["tonemap"], json!("none"));

        let second = api
            .scene_set_postfx(PostFxPatch {
                tonemap: Some("aces".into()),
                fxaa: Some(true),
                ..Default::default()
            })
            .expect("应成功");
        // 曝光保留上一次的值，不被这次补丁清零。
        assert_eq!(second["postfx"]["exposure"], json!(0.5));
        assert_eq!(second["postfx"]["tonemap"], json!("aces"));
        assert_eq!(second["postfx"]["fxaa"], json!(true));

        // 越界值收敛到上界，而不是写进 uniform。
        let clamped = api
            .scene_set_postfx(PostFxPatch {
                exposure: Some(9999.0),
                bloom_intensity: Some(-4.0),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(
            clamped["postfx"]["exposure"],
            json!(r3e_core::postfx::MAX_EXPOSURE_EV)
        );
        assert_eq!(clamped["postfx"]["bloom"]["intensity"], json!(0.0));

        // 认不出的曲线名报参数错误，且不改动现有设置。
        let bad = api
            .scene_set_postfx(PostFxPatch {
                tonemap: Some("cinematic".into()),
                ..Default::default()
            })
            .expect_err("未知曲线名应报错");
        assert!(
            format!("{bad:?}").contains("cinematic"),
            "错误信息应带上给错的名字: {bad:?}"
        );
        assert_eq!(
            scene.borrow().postfx.expect("postfx 仍在").tonemap,
            r3e_core::postfx::TonemapMode::Aces
        );

        // enabled=false 是唯一的清空入口。
        let cleared = api
            .scene_set_postfx(PostFxPatch {
                enabled: Some(false),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(cleared["postfx"], Value::Null);
        assert!(scene.borrow().postfx.is_none());
    }

    #[test]
    fn script_reload_without_file_is_an_error() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        assert!(api.script_reload().is_err());
    }

    #[test]
    fn ui_set_inserts_with_parsed_units() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let out = api
            .ui_set(UiPatch {
                key: "btn_start".to_string(),
                kind: Some("image".to_string()),
                anchor: Some("bottom_center".to_string()),
                x: Some("0".to_string()),
                y: Some("-48".to_string()),
                width: Some("50%".to_string()),
                height: Some("64".to_string()),
                layer: Some(7),
                ..Default::default()
            })
            .expect("应成功");

        assert_eq!(out["inserted"], true);
        assert_eq!(out["count"], 1);
        assert_eq!(out["element"]["kind"], "image");
        assert_eq!(out["element"]["anchor"], "bottom_center");
        // 百分比与像素各回读成自己的单位串。
        assert_eq!(out["element"]["width"], "50%");
        assert_eq!(out["element"]["height"], "64");

        let scene = scene.borrow();
        assert_eq!(scene.ui.len(), 1);
        assert_eq!(scene.ui[0].size[0], UiValue::pct(50.0));
        assert_eq!(scene.ui[0].size[1], UiValue::px(64.0));
        assert_eq!(scene.ui[0].offset[1], UiValue::px(-48.0));
        assert_eq!(scene.ui[0].layer, 7);
    }

    #[test]
    fn ui_set_patches_existing_element_in_place() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.ui_set(UiPatch {
            key: "btn".to_string(),
            kind: Some("image".to_string()),
            image: Some("ui/btn.png".to_string()),
            width: Some("120".to_string()),
            layer: Some(3),
            ..Default::default()
        })
        .expect("首次写入应成功");

        // 只改 layer：其余字段必须保持原值（局部更新而不是整体覆盖）。
        let out = api
            .ui_set(UiPatch {
                key: "btn".to_string(),
                layer: Some(9),
                ..Default::default()
            })
            .expect("局部更新应成功");
        assert_eq!(out["inserted"], false);
        assert_eq!(out["replaced"], true);
        assert_eq!(out["count"], 1, "同 key 不应累积出新元素");

        let scene = scene.borrow();
        assert_eq!(scene.ui.len(), 1);
        assert_eq!(scene.ui[0].layer, 9);
        assert_eq!(scene.ui[0].kind, UiKind::Image);
        assert_eq!(scene.ui[0].image, "ui/btn.png");
        assert_eq!(scene.ui[0].size[0], UiValue::px(120.0));
    }

    #[test]
    fn ui_set_rejects_unknown_names_without_side_effect() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let bad_anchor = api
            .ui_set(UiPatch {
                key: "panel".to_string(),
                anchor: Some("斜着".to_string()),
                ..Default::default()
            })
            .expect_err("未知锚点应报错");
        assert!(format!("{bad_anchor:?}").contains("斜着"));

        let bad_kind = api
            .ui_set(UiPatch {
                key: "panel".to_string(),
                kind: Some("圆角矩形".to_string()),
                ..Default::default()
            })
            .expect_err("未知类型应报错");
        assert!(format!("{bad_kind:?}").contains("圆角矩形"));

        assert!(scene.borrow().ui.is_empty(), "报错时不应留下半条元素");
    }

    #[test]
    fn ui_list_and_remove_round_trip() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        for key in ["a", "b"] {
            api.ui_set(UiPatch {
                key: key.to_string(),
                ..Default::default()
            })
            .expect("应成功");
        }
        let listed = api.ui_list(None).expect("应成功");
        assert_eq!(listed["count"], 2);
        assert_eq!(listed["elements"].as_array().expect("数组").len(), 2);

        let removed = api.ui_remove("a").expect("应成功");
        assert_eq!(removed["removed"], true);
        assert_eq!(removed["count"], 1);

        let again = api.ui_remove("a").expect("重复删除应成功");
        assert_eq!(again["removed"], false, "已不存在时如实回报 false");
    }

    #[test]
    fn ui_state_reports_pointer_interaction() {
        let (scene, physics, mut scripts) = fixture();
        scene.borrow_mut().ui.push(UiElement {
            key: "btn".to_string(),
            ..Default::default()
        });
        // 直接把光标按在元素上（默认 100x100 从左上角起算），模拟一帧按下。
        {
            let interaction = scripts.ui_interaction();
            let mut state = interaction.borrow_mut();
            state.reset();
            state.update(
                &scene.borrow().ui,
                (800.0, 600.0),
                Some((10.0, 10.0)),
                true,
                true,
                false,
            );
        }
        let mut api = api(&scene, &physics, &mut scripts);
        let state = api.ui_state().expect("应成功");
        assert_eq!(state["hover"], "btn");
        assert_eq!(state["active"], "btn");
        assert_eq!(state["pointer_over_ui"], true);
        assert_eq!(state["events"].as_array().expect("数组").len(), 2);
    }

    // ---------- 扩展十五：局部光源 RPC ----------

    /// 新建点光：缺省字段取 `LocalLight::default`，给了的字段逐个落地。
    #[test]
    fn light_set_inserts_point_light() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let out = api
            .light_set(LightPatch {
                key: "lamp".to_string(),
                pos: Some([1.0, 2.0, 3.0]),
                intensity: Some(8.0),
                range: Some(12.0),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(out["inserted"], true);
        assert_eq!(out["count"], 1);
        assert_eq!(out["light"]["kind"], "point");
        assert_eq!(out["light"]["pos"], json!([1.0, 2.0, 3.0]));

        let scene = scene.borrow();
        let light = scene.local_light("lamp").expect("场景里应有这盏灯");
        assert_eq!(light.intensity, 8.0);
        assert_eq!(light.range, 12.0);
        // 没给的字段保持默认，而不是被清零
        assert_eq!(light.color, LocalLight::default().color);
    }

    /// 同 key 再写一次是叠加而非替换：没给的字段保留上一次的值。
    #[test]
    fn light_set_merges_into_existing() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.light_set(LightPatch {
            key: "lamp".to_string(),
            kind: Some("spot".to_string()),
            pos: Some([0.0, 5.0, 0.0]),
            dir: Some([0.0, -1.0, 0.0]),
            inner_deg: Some(15.0),
            outer_deg: Some(25.0),
            ..Default::default()
        })
        .expect("应成功");
        let out = api
            .light_set(LightPatch {
                key: "lamp".to_string(),
                intensity: Some(20.0),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(out["inserted"], false, "第二次应是更新");
        assert_eq!(out["count"], 1, "不应重复新增");
        assert_eq!(out["light"]["kind"], "spot", "未给的 kind 保留");
        assert_eq!(out["light"]["intensity"], 20.0);
        assert_eq!(out["light"]["inner_deg"], 15.0, "未给的锥角保留");
    }

    /// 未知 kind 必须报参数错误，且不能留下半盏灯。
    #[test]
    fn light_set_rejects_unknown_kind_without_side_effect() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        let err = api
            .light_set(LightPatch {
                key: "bad".to_string(),
                kind: Some("laser".to_string()),
                ..Default::default()
            })
            .expect_err("应报参数错误");
        assert!(err.message.contains("未知光源类型"), "{}", err.message);
        assert_eq!(scene.borrow().lights.len(), 0, "坏 kind 不应落地");
    }

    /// 坏值收敛：NaN / 负强度 / 零半径 / 内外角倒挂都必须在进场景前被夹住。
    #[test]
    fn light_set_sanitizes_bad_values() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.light_set(LightPatch {
            key: "bad".to_string(),
            pos: Some([f32::NAN, 0.0, 0.0]),
            intensity: Some(-3.0),
            range: Some(0.0),
            inner_deg: Some(40.0),
            outer_deg: Some(10.0),
            ..Default::default()
        })
        .expect("应成功（收敛后）");

        let scene = scene.borrow();
        let light = scene.local_light("bad").expect("应已落地");
        assert!(light.pos[0].is_finite(), "NaN 位置必须被抹平");
        assert_eq!(light.intensity, 0.0, "负强度夹到 0");
        assert_eq!(light.range, MIN_LIGHT_RANGE, "零半径抬到下限");
        assert!(light.outer_deg > light.inner_deg, "内外角必须修正");
    }

    /// 扩展十八 C：阴影两项走同一补丁通道，缺省不动原值，非法偏移被夹进合法区间。
    #[test]
    fn light_set_merges_shadow_fields() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.light_set(LightPatch {
            key: "lamp".to_string(),
            intensity: Some(2.0),
            ..Default::default()
        })
        .expect("新建应成功");
        assert!(
            !scene.borrow().local_light("lamp").unwrap().cast_shadow,
            "新灯默认不投影"
        );

        // 只给 cast_shadow：shadow_bias 必须沿用原值
        let before_bias = scene.borrow().local_light("lamp").unwrap().shadow_bias;
        let resp = api
            .light_set(LightPatch {
                key: "lamp".to_string(),
                cast_shadow: Some(true),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(resp["inserted"], json!(false), "同 key 是更新而非新增");
        assert_eq!(resp["light"]["cast_shadow"], json!(true), "回包带新值");
        assert_eq!(resp["light"]["shadow_bias"], json!(before_bias));
        assert!(scene.borrow().local_light("lamp").unwrap().cast_shadow);

        // 超上限的偏移被夹住，不会原样落进场景
        api.light_set(LightPatch {
            key: "lamp".to_string(),
            shadow_bias: Some(99.0),
            ..Default::default()
        })
        .expect("应成功（收敛后）");
        let bias = scene.borrow().local_light("lamp").unwrap().shadow_bias;
        assert!(bias <= MAX_LOCAL_SHADOW_BIAS, "偏移必须被夹到上限内");

        // 关掉投影同样走这条通道
        api.light_set(LightPatch {
            key: "lamp".to_string(),
            cast_shadow: Some(false),
            ..Default::default()
        })
        .expect("应成功");
        assert!(!scene.borrow().local_light("lamp").unwrap().cast_shadow);
    }

    /// 满载：新增被拒并报上限，同 key 覆盖仍生效。
    #[test]
    fn light_set_capacity_limit() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        for i in 0..MAX_LOCAL_LIGHTS {
            api.light_set(LightPatch {
                key: format!("l{i}"),
                intensity: Some(1.0),
                ..Default::default()
            })
            .expect("前 MAX_LOCAL_LIGHTS 盏应成功");
        }
        let err = api
            .light_set(LightPatch {
                key: "extra".to_string(),
                intensity: Some(1.0),
                ..Default::default()
            })
            .expect_err("满载后新增应报错");
        assert!(
            err.message.contains(&MAX_LOCAL_LIGHTS.to_string()),
            "{}",
            err.message
        );

        api.light_set(LightPatch {
            key: "l0".to_string(),
            intensity: Some(9.0),
            ..Default::default()
        })
        .expect("满载时同 key 覆盖必须仍生效");
        assert_eq!(scene.borrow().local_light("l0").unwrap().intensity, 9.0);
    }

    /// list / remove / clear / enabled 四件套。
    #[test]
    fn light_list_remove_clear_and_switch() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        for key in ["a", "b"] {
            api.light_set(LightPatch {
                key: key.to_string(),
                pos: Some([0.0, 1.0, 0.0]),
                intensity: Some(1.0),
                ..Default::default()
            })
            .expect("应成功");
        }
        let listed = api.light_list(None).expect("应成功");
        assert_eq!(listed["count"], 2);
        assert_eq!(listed["enabled"], true);
        let arr = listed["lights"].as_array().expect("数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["key"], "a");
        assert_eq!(arr[0]["kind"], "point");
        // 九个字段都要在，编辑器靠这个面板直接渲染
        for field in [
            "key",
            "kind",
            "pos",
            "dir",
            "color",
            "intensity",
            "range",
            "inner_deg",
            "outer_deg",
        ] {
            assert!(arr[0].get(field).is_some(), "快照缺少字段 {field}");
        }

        let limited = api.light_list(Some(1)).expect("应成功");
        assert_eq!(limited["count"], 2, "count 是全量");
        assert_eq!(
            limited["lights"].as_array().unwrap().len(),
            1,
            "limit 只截结果"
        );

        let removed = api.light_remove("a").expect("应成功");
        assert_eq!(removed["removed"], true);
        assert_eq!(removed["count"], 1);
        assert_eq!(
            api.light_remove("a").expect("应成功")["removed"],
            false,
            "重复删除如实回报"
        );

        let switched = api.light_set_enabled(Some(false)).expect("应成功");
        assert_eq!(switched["enabled"], false);
        assert_eq!(switched["count"], 1, "关灯不删数据");
        assert_eq!(
            api.light_set_enabled(None).expect("应成功")["enabled"],
            false,
            "不给参数是只读"
        );

        let cleared = api.light_clear().expect("应成功");
        assert_eq!(cleared["removed"], 1);
        assert_eq!(scene.borrow().lights.len(), 0);
    }

    /// 扩展十六：`scene.shadow` 的读 / 写 / 收敛三段语义。
    #[test]
    fn shadow_rpc_reads_writes_and_clamps() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        // 只读：不该改动任何值
        let before = scene.borrow().light.shadow;
        let read = api.scene_shadow(None).expect("应成功");
        assert_eq!(read["enabled"], before.enabled);
        assert_eq!(read["cascades"], before.cascade.count);
        assert_eq!(
            read["maxCascades"], MAX_CASCADES,
            "上限必须回给编辑器，面板才能画对滑块范围"
        );
        for field in [
            "enabled",
            "size",
            "strength",
            "orthoSize",
            "bias",
            "normalBias",
            "cascades",
            "maxCascades",
            "blend",
            "distance",
            "lambda",
        ] {
            assert!(read.get(field).is_some(), "回包缺少字段 {field}");
        }

        // 写入：补丁合并后落地
        let written = api
            .scene_shadow(Some(ShadowPatch {
                cascades: Some(4),
                blend: Some(0.2),
                distance: Some(80.0),
                ..Default::default()
            }))
            .expect("应成功");
        assert_eq!(written["cascades"], 4);
        assert_eq!(written["blend"], 0.2_f64 as f32);
        assert_eq!(written["distance"], 80.0);
        let shadow = scene.borrow().light.shadow;
        assert_eq!(shadow.cascade.count, 4);
        assert_eq!(shadow.cascade.blend, 0.2);

        // 越界一次性收敛，且回包给的是收敛后的值（不能「回包好看、实际另一套」）
        let clamped = api
            .scene_shadow(Some(ShadowPatch {
                cascades: Some(99),
                blend: Some(4.0),
                size: Some(1000),
                ..Default::default()
            }))
            .expect("应成功");
        assert_eq!(clamped["cascades"], MAX_CASCADES);
        assert_eq!(
            clamped["blend"], 0.5,
            "混合比上限是 0.5（再大就跨级采到不存在的层）"
        );
        assert_eq!(
            clamped["size"], 1000,
            "只要在 64..8192 内就原样保留（不强行取整）"
        );
        let shadow = scene.borrow().light.shadow;
        assert_eq!(shadow.cascade.count, MAX_CASCADES as u32);
        assert_eq!(shadow.size, 1000);

        // 关掉阴影只关开关，级数等配置保留（编辑器取消勾选不该丢配置）
        let off = api
            .scene_shadow(Some(ShadowPatch {
                enabled: Some(false),
                ..Default::default()
            }))
            .expect("应成功");
        assert_eq!(off["enabled"], false);
        assert_eq!(off["cascades"], MAX_CASCADES, "关阴影不重置级联配置");
    }

    /// 空 key 一律拒绝（传输层已拦一次，宿主侧再兜一次，防止别的调用方绕过）。
    #[test]
    fn light_set_rejects_blank_key() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        assert!(api
            .light_set(LightPatch {
                key: "   ".to_string(),
                ..Default::default()
            })
            .is_err());
        assert_eq!(scene.borrow().lights.len(), 0);
    }

    /// 遮蔽补丁落在后处理段的 `ssao` 子段里：既有设置（曝光）必须原样留着，
    /// 越界样本数在宿主侧夹进 [4, 16] 而不是报参数错误。
    #[test]
    fn set_ssao_merges_and_clamps() {
        let (scene, physics, mut scripts) = fixture();
        {
            let mut api = api(&scene, &physics, &mut scripts);
            api.scene_set_postfx(PostFxPatch {
                exposure: Some(1.5),
                ..Default::default()
            })
            .expect("postfx 补丁应成功");
            let v = api
                .scene_set_ssao(SsaoPatch {
                    enabled: Some(true),
                    radius: Some(0.9),
                    samples: Some(99),
                    ..Default::default()
                })
                .expect("ssao 补丁应成功");
            assert_eq!(v["ssao"]["samples"], json!(16), "样本数应夹到上限");
            assert_eq!(v["enabled"], true);
            assert_eq!(v["neutral"], false);
        }
        let fx = scene.borrow().postfx.expect("后处理段应已实体化");
        assert!(fx.ssao.enabled);
        assert!((fx.ssao.radius - 0.9).abs() < 1e-6);
        assert!(
            (fx.exposure - 1.5).abs() < 1e-6,
            "遮蔽补丁不该动曝光: {fx:?}"
        );
    }

    /// `enabled = false` 只关遮蔽支线：曝光 / 泛光 / FXAA 保留，段本身不清空。
    #[test]
    fn disable_ssao_keeps_rest_of_postfx() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.scene_set_postfx(PostFxPatch {
            exposure: Some(-0.5),
            bloom_enabled: Some(true),
            fxaa: Some(true),
            ..Default::default()
        })
        .expect("postfx 补丁应成功");
        api.scene_set_ssao(SsaoPatch {
            enabled: Some(true),
            intensity: Some(2.0),
            ..Default::default()
        })
        .expect("ssao 补丁应成功");
        let off = api
            .scene_set_ssao(SsaoPatch {
                enabled: Some(false),
                ..Default::default()
            })
            .expect("应成功");
        assert_eq!(off["enabled"], false);
        assert_eq!(off["neutral"], true);
        let fx = scene.borrow().postfx.expect("段不该被清空");
        assert!(!fx.ssao.enabled);
        assert!((fx.ssao.intensity - 2.0).abs() < 1e-6, "强度应保留");
        assert!((fx.exposure + 0.5).abs() < 1e-6);
        assert!(fx.bloom.enabled);
        assert!(fx.fxaa);
    }
    /// 造一个带混合树的场景：`hero/walk` 播 walk，`hero/atk` 供层叠加。
    fn anim_fixture() -> Fixture {
        let (scene, physics, scripts) = fixture();
        {
            let mut scene_ref = scene.borrow_mut();
            let cube = scene_ref.find_id("cube").expect("cube 应存在");
            let walk = Arc::new(anim_clip("walk", 10.0));
            let mut inst = AnimInstance::new("hero/walk", walk);
            inst.bindings = vec![AnimBinding {
                node: cube,
                node_index: 0,
                path: "cube".to_string(),
                track: 0,
            }];
            scene_ref.add_anim_instance(inst);
            scene_ref.register_anim_clip("hero/atk", Arc::new(anim_clip("atk", 100.0)));
        }
        (scene, physics, scripts)
    }

    fn anim_clip(name: &str, end_x: f32) -> AnimClip {
        AnimClip {
            name: name.to_string(),
            duration: 1.0,
            tracks: vec![AnimTrack {
                node_path: "cube".to_string(),
                node_index: 0,
                keys: vec![anim_key(0.0, 0.0), anim_key(1.0, end_x)],
            }],
        }
    }

    fn anim_key(time: f32, x: f32) -> AnimKey {
        AnimKey {
            time,
            position: Vec3::new(x, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    #[test]
    fn anim_layers_lists_instances_and_layers() {
        let (scene, physics, mut scripts) = anim_fixture();
        // 层只能由脚本建（层必须绑定片段），编辑器只能读和调参。
        let _ = scripts
            .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0)"#)
            .expect("建层应成功");
        let mut api = api(&scene, &physics, &mut scripts);

        let all = api.scene_anim_layers(None, None).expect("列层应成功");
        assert_eq!(all["count"], json!(1));
        assert_eq!(all["instances"], json!(1), "应报告扫了几个实例");
        assert_eq!(all["limit"], json!(MAX_ANIM_LAYERS));
        let layer = &all["layers"][0];
        assert_eq!(layer["instance"], json!("hero/walk"));
        assert_eq!(layer["layer"], json!("upper"));
        assert_eq!(layer["mode"], json!("override"));
        assert_eq!(layer["mask"], json!("*"), "默认整副骨架");
        assert_eq!(layer["clips"][0]["name"], json!("atk"));
        assert_eq!(layer["playing"], json!(true));

        let one = api
            .scene_anim_layers(Some("hero/walk"), None)
            .expect("点名实例应成功");
        assert_eq!(one["count"], json!(1));

        let missing = api.scene_anim_layers(Some("nobody"), None);
        assert!(missing.is_err(), "点名的实例不存在必须报错");
    }

    #[test]
    fn anim_layer_set_patches_and_returns_layer() {
        let (scene, physics, mut scripts) = anim_fixture();
        let _ = scripts
            .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0)"#)
            .expect("建层应成功");
        let mut api = api(&scene, &physics, &mut scripts);

        let out = api
            .scene_anim_layer_set(AnimLayerPatch {
                instance: "hero/walk".to_string(),
                layer: "upper".to_string(),
                weight: Some(0.4),
                mode: Some("additive".to_string()),
                mask: Some("cube,!root".to_string()),
                speed: Some(2.0),
                clip: None,
                sync: Some("phase".to_string()),
            })
            .expect("补丁应成功");
        let layer = &out["layer"];
        // f32→JSON 会有二进制尾差，按近似比。
        let weight = layer["weight"].as_f64().expect("weight 应是数字");
        assert!((weight - 0.4).abs() < 1e-6, "权重应为 0.4，实得 {weight}");
        assert_eq!(layer["mode"], json!("additive"));
        assert_eq!(layer["speed"], json!(2.0));
        assert_eq!(layer["mask_include"], json!(["cube"]));
        assert_eq!(layer["mask_exclude"], json!(["root"]));
        assert_eq!(layer["mask"], json!("cube,!root"), "遮罩应能原样回写成串");
        // 扩展四十六：采样策略随补丁落到 core，并可回读
        assert_eq!(layer["sync"], json!("phase"), "策略应原样回显");
        assert!(layer["phase"].is_number(), "相位应始终可读");

        // 落到了 core 上，而不是只回了个好看的 JSON。
        let stored = scene
            .borrow()
            .anim_layer("hero/walk", "upper")
            .cloned()
            .expect("层应存在");
        assert!((stored.weight - 0.4).abs() < 1e-6);
        assert_eq!(stored.mode, LayerMode::Additive);
        assert!((stored.speed - 2.0).abs() < 1e-6);
        assert_eq!(stored.sync, LayerSync::Phase);
        assert!(stored.mask.contains("cube") && !stored.mask.contains("root"));
    }

    #[test]
    fn anim_layer_set_rejects_unknown_sync_without_side_effect() {
        let (scene, physics, mut scripts) = anim_fixture();
        let _ = scripts
            .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 0.8)"#)
            .expect("建层应成功");
        let mut api = api(&scene, &physics, &mut scripts);

        // 策略名写错：整张补丁都不该落地，权重保持 0.8、策略保持 off。
        let bad = api.scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            weight: Some(0.2),
            sync: Some("warp".to_string()),
            ..Default::default()
        });
        assert!(bad.is_err(), "未知采样策略必须报错");

        let stored = scene
            .borrow()
            .anim_layer("hero/walk", "upper")
            .cloned()
            .expect("层应存在");
        assert!((stored.weight - 0.8).abs() < 1e-6, "权重不该被改");
        assert_eq!(stored.sync, LayerSync::Off, "策略不该被改");
    }

    #[test]
    fn anim_layer_set_rejects_unknown_mode_without_side_effect() {
        let (scene, physics, mut scripts) = anim_fixture();
        let _ = scripts
            .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 0.8)"#)
            .expect("建层应成功");
        let mut api = api(&scene, &physics, &mut scripts);

        let bad = api.scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "upper".to_string(),
            weight: Some(0.1),
            mode: Some("multiply".to_string()),
            ..Default::default()
        });
        assert!(bad.is_err(), "未知模式必须报错");

        // 关键：模式写错时，同一次补丁里的权重也不该落地——不留半个改过的层。
        let stored = scene
            .borrow()
            .anim_layer("hero/walk", "upper")
            .cloned()
            .expect("层应存在");
        assert!((stored.weight - 0.8).abs() < 1e-6, "权重不该被改");
        assert_eq!(stored.mode, LayerMode::Override, "模式不该被改");
    }

    #[test]
    fn anim_layer_set_rejects_missing_instance_or_layer() {
        let (scene, physics, mut scripts) = anim_fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        let no_instance = api.scene_anim_layer_set(AnimLayerPatch {
            instance: "nobody".to_string(),
            weight: Some(0.5),
            ..Default::default()
        });
        assert!(no_instance.is_err(), "实例不存在必须报错");

        // 实例在、层不在：同样报错，不凭空建层。
        let no_layer = api.scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            layer: "legs".to_string(),
            weight: Some(0.5),
            ..Default::default()
        });
        assert!(no_layer.is_err(), "层不存在必须报错");
        assert!(
            scene.borrow().anim_layer_names("hero/walk").is_empty(),
            "失败调用不该建出任何层"
        );

        // 片段名解析不到也报错。
        let no_clip = api.scene_anim_layer_set(AnimLayerPatch {
            instance: "hero/walk".to_string(),
            clip: Some("nosuchclip".to_string()),
            ..Default::default()
        });
        assert!(no_clip.is_err(), "片段解析不到必须报错");
    }

    #[test]
    fn anim_layers_limit_truncates_but_count_is_total() {
        let (scene, physics, mut scripts) = anim_fixture();
        let _ = scripts
            .eval(r#"add_anim_layer("hero/walk", "upper", "atk", 1.0); add_anim_layer("hero/walk", "lower", "atk", 0.5)"#)
            .expect("建层应成功");
        let mut api = api(&scene, &physics, &mut scripts);

        let list = api.scene_anim_layers(None, Some(1)).expect("列层应成功");
        assert_eq!(list["count"], json!(2), "count 报总数");
        assert_eq!(
            list["layers"].as_array().map(|v| v.len()),
            Some(1),
            "limit 只截断明细"
        );
    }

    #[test]
    fn bone_mask_spec_is_normalized() {
        // 空 / `*` / 只有分隔符 → 整副骨架（空遮罩会让整层都不出效果）。
        for spec in ["", "  ", "*", " , ", ",,"] {
            let mask = bone_mask_of(spec);
            assert!(mask.is_pass_all(), "`{spec}` 应归一成整副骨架");
            assert_eq!(mask.rule_count(), 0);
        }
        let include = bone_mask_of("cube, sphere");
        assert_eq!(
            include.include,
            vec!["cube".to_string(), "sphere".to_string()]
        );
        assert!(include.exclude.is_empty());

        let exclude = bone_mask_of("!root");
        assert!(exclude.include.is_empty());
        assert_eq!(exclude.exclude, vec!["root".to_string()]);

        let mixed = bone_mask_of("cube, !root");
        assert_eq!(mixed.include, vec!["cube".to_string()]);
        assert_eq!(mixed.exclude, vec!["root".to_string()]);
        assert!(!mixed.is_pass_all());

        // 只有 `!` 的规则没有名字，忽略而不是造一条空规则。
        assert!(bone_mask_of("!").is_pass_all());
    }

    // ---- 3D 空间化音频（§12 第十项）宿主层 ----

    fn spatial_patch(key: &str) -> AudioSpatialPatch {
        AudioSpatialPatch {
            key: key.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn audio_spatial_set_upserts_and_lists() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        let created = api
            .audio_spatial_set(AudioSpatialPatch {
                position: Some([1.0, 0.0, 0.0]),
                min_distance: Some(2.0),
                max_distance: Some(20.0),
                model: Some("inverse".to_string()),
                doppler: Some(0.5),
                ..spatial_patch("bell")
            })
            .expect("注册应成功");
        assert_eq!(created["inserted"], json!(true));
        assert_eq!(created["count"], json!(1));
        assert_eq!(created["source"]["model"], json!("inverse"));
        assert_eq!(created["source"]["min_distance"], json!(2.0));

        // 同 key 再打一次是覆盖，不是新增
        let updated = api
            .audio_spatial_set(AudioSpatialPatch {
                max_distance: Some(35.0),
                ..spatial_patch("bell")
            })
            .expect("覆盖应成功");
        assert_eq!(updated["inserted"], json!(false));
        assert_eq!(updated["count"], json!(1));
        assert_eq!(updated["source"]["max_distance"], json!(35.0));
        // 没给的字段保持原值，不被默认值冲掉
        assert_eq!(updated["source"]["min_distance"], json!(2.0));
        assert_eq!(updated["source"]["position"], json!([1.0, 0.0, 0.0]));

        let list = api.audio_spatial_list(None).expect("列表应成功");
        assert_eq!(list["count"], json!(1));
        assert_eq!(list["enabled"], json!(true));
        assert_eq!(list["sources"].as_array().map(|v| v.len()), Some(1));
        // 没注册的 key 不算声源
        assert_eq!(api.audio_spatial_list(Some(0)).unwrap()["count"], json!(1));
        assert_eq!(
            api.audio_spatial_list(Some(0)).unwrap()["sources"]
                .as_array()
                .map(|v| v.len()),
            Some(0),
            "limit 只截断明细"
        );
    }

    #[test]
    fn audio_spatial_set_rejects_bad_input_without_side_effect() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        assert!(
            api.audio_spatial_set(spatial_patch("   ")).is_err(),
            "空 key 必须报错"
        );
        let bad_model = api.audio_spatial_set(AudioSpatialPatch {
            model: Some("quadratic".to_string()),
            ..spatial_patch("bell")
        });
        assert!(bad_model.is_err(), "未知距离模型必须报错");
        // 失败不留半改：模型名解析在校验前完成，声源表应当还是空的
        assert_eq!(
            api.audio_spatial_list(None).unwrap()["count"],
            json!(0),
            "报错的调用不该注册出任何声源"
        );
    }

    #[test]
    fn audio_spatial_listener_drives_distance_and_gain() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.audio_spatial_set(AudioSpatialPatch {
            position: Some([0.0, 0.0, 0.0]),
            min_distance: Some(1.0),
            max_distance: Some(20.0),
            ..spatial_patch("bell")
        })
        .expect("注册应成功");

        let near = api
            .audio_spatial_listener(Some(r3e_editor::ListenerPatch {
                position: Some([0.0, 0.0, 0.0]),
                forward: Some([0.0, 0.0, -1.0]),
            }))
            .expect("写听者应成功");
        assert_eq!(near["position"], json!([0.0, 0.0, 0.0]));

        let far = api
            .audio_spatial_listener(Some(r3e_editor::ListenerPatch {
                position: Some([15.0, 0.0, 0.0]),
                ..Default::default()
            }))
            .expect("写听者应成功");
        assert_eq!(far["position"], json!([15.0, 0.0, 0.0]));

        // 写完立刻结算：不用等下一帧就能在面板上看到距离与增益变化
        let list = api.audio_spatial_list(None).expect("列表应成功");
        let src = &list["sources"][0];
        let distance = src["distance"].as_f64().expect("应有距离");
        assert!(
            (distance - 15.0).abs() < 1e-3,
            "距离应约为 15，实际 {distance}"
        );
        let gain = src["gain"].as_f64().expect("应有增益");
        assert!(gain < 1.0 && gain > 0.0, "远处应衰减但仍可闻，实际 {gain}");
        assert_eq!(list["listener"]["position"], json!([15.0, 0.0, 0.0]));

        // 只读模式（省略参数）不该改掉刚写的听者
        let read = api.audio_spatial_listener(None).expect("只读应成功");
        assert_eq!(read["position"], json!([15.0, 0.0, 0.0]));

        // 零向量朝向被忽略，不能把朝向归一成 NaN
        api.audio_spatial_listener(Some(r3e_editor::ListenerPatch {
            forward: Some([0.0, 0.0, 0.0]),
            ..Default::default()
        }))
        .expect("零朝向应被静默忽略");
        let list = api.audio_spatial_list(None).expect("列表应成功");
        assert_eq!(list["listener"]["forward"], json!([0.0, 0.0, -1.0]));
    }

    #[test]
    fn audio_spatial_node_binding_and_detach() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        api.audio_spatial_set(AudioSpatialPatch {
            node: Some("cube".to_string()),
            ..spatial_patch("bell")
        })
        .expect("带节点注册应成功");
        assert_eq!(
            api.audio_spatial_list(None).unwrap()["sources"][0]["node"],
            json!("cube")
        );

        // 空串 = 解绑（回到只按手动 position 算）
        api.audio_spatial_set(AudioSpatialPatch {
            node: Some("  ".to_string()),
            ..spatial_patch("bell")
        })
        .expect("解绑应成功");
        assert_eq!(
            api.audio_spatial_list(None).unwrap()["sources"][0]["node"],
            Value::Null,
            "解绑后不该残留节点名"
        );

        let removed = api.audio_spatial_remove("bell").expect("移除应成功");
        assert_eq!(removed["removed"], json!(true));
        assert_eq!(removed["count"], json!(0));
        // 重复移除不报错，只是 false
        assert_eq!(
            api.audio_spatial_remove("bell").unwrap()["removed"],
            json!(false)
        );
    }

    #[test]
    fn audio_spatial_capacity_and_clear() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        for i in 0..MAX_SPATIAL_SOURCES {
            api.audio_spatial_set(spatial_patch(&format!("s{i}")))
                .unwrap_or_else(|e| panic!("第 {i} 个声源应注册成功: {e:?}"));
        }
        let over = api.audio_spatial_set(spatial_patch("overflow"));
        assert!(over.is_err(), "满载后新增必须报错而不是挤掉别人");
        assert_eq!(
            api.audio_spatial_list(None).unwrap()["count"],
            json!(MAX_SPATIAL_SOURCES)
        );

        let cleared = api.audio_spatial_clear().expect("清空应成功");
        assert_eq!(cleared["removed"], json!(MAX_SPATIAL_SOURCES));
        assert_eq!(cleared["count"], json!(0));
        // 清空后能重新注册（不会因为计数残留而卡死）
        assert!(api.audio_spatial_set(spatial_patch("fresh")).is_ok());
    }

    // ------------------------------------------------------------ P0-4 反馈工具

    #[test]
    fn engine_stats_without_renderer_reports_null_render() {
        let (scene, physics, mut scripts) = fixture();
        let stats = api(&scene, &physics, &mut scripts)
            .engine_stats()
            .expect("无渲染器时 engine_stats 仍然要给场景快照");

        // 关键契约：没有渲染器就说没有，而不是把 draw_calls 填 0 冒充「渲染了但没画」。
        assert_eq!(stats["renderer"], json!(false));
        assert!(stats["render"].is_null(), "render 应为 null: {stats}");
        assert_eq!(stats["size"]["width"], json!(0));
        assert_eq!(stats["size"]["height"], json!(0));
    }

    #[test]
    fn engine_stats_summarizes_scene_and_frame_time() {
        let (scene, physics, mut scripts) = fixture();
        let stats = api(&scene, &physics, &mut scripts)
            .engine_stats()
            .expect("engine_stats");

        assert_eq!(stats["scene"]["name"], json!("editor-test"));
        assert_eq!(stats["scene"]["objects"], json!(3));
        assert_eq!(stats["scene"]["visible"], json!(3));
        // 三个物件都是 cube：去重后的网格种类是 1，不是 3
        assert_eq!(stats["scene"]["mesh_kinds"], json!(1));
        assert_eq!(stats["scene"]["material_overrides"], json!(0));
        assert_eq!(stats["frame"]["index"], json!(42));
        // fixture 里预置的帧时间分布要原样透传，不能被 fps 覆盖
        assert_eq!(stats["frame"]["frame_ms"]["samples"], json!(4));
        assert_eq!(stats["frame"]["frame_ms"]["p95"], json!(18.5));
    }

    #[test]
    fn capture_frame_without_renderer_reports_unavailable() {
        let (scene, physics, mut scripts) = fixture();
        let err = api(&scene, &physics, &mut scripts)
            .capture_frame(Some("captures/x.png"))
            .expect_err("没有渲染器时截图必须失败");
        // 报错要指向「为什么不能截」，而不是让调用方以为路径写错了
        assert!(err.message.contains("渲染器"), "{}", err.message);
        assert_eq!(err.code, r3e_editor::ENGINE_ERROR);
    }

    #[test]
    fn frame_time_percentiles_use_nearest_rank() {
        // 1..=100 ms：nearest-rank 下 p50=50、p95=95、p99=99、max=100，都是真实观测值
        let samples: Vec<f32> = (1..=100).map(|ms| ms as f32).collect();
        let stats = FrameTimeStats::from_samples(&samples);
        assert_eq!(stats.samples, 100);
        assert_eq!(stats.last_ms, 100.0);
        assert_eq!(stats.p50_ms, 50.0);
        assert_eq!(stats.p95_ms, 95.0);
        assert_eq!(stats.p99_ms, 99.0);
        assert_eq!(stats.max_ms, 100.0);

        // 空样本给全零，避免出现 NaN 分位
        assert_eq!(FrameTimeStats::from_samples(&[]), FrameTimeStats::default());
    }

    #[test]
    fn frame_time_window_wraps_and_ignores_bad_samples() {
        let mut window = FrameTimeWindow::new(4);
        window.push(f32::NAN);
        window.push(-1.0);
        assert_eq!(window.snapshot().samples, 0, "非法样本不应进入缓冲");

        for ms in [10.0, 20.0, 30.0, 40.0] {
            window.push(ms);
        }
        assert_eq!(window.snapshot().samples, 4);
        assert_eq!(window.snapshot().max_ms, 40.0);

        // 满载后覆盖最旧的：10.0 被 5.0 顶掉（样本变 [5,20,30,40]），
        // 于是 p50 从 10.0 抬到 20.0 —— 这就是「旧样本真的被淘汰」的证据。
        window.push(5.0);
        let snapshot = window.snapshot();
        assert_eq!(snapshot.samples, 4);
        assert_eq!(snapshot.last_ms, 5.0, "last 必须是最后推入的样本");
        assert_eq!(snapshot.max_ms, 40.0);
        assert_eq!(snapshot.p50_ms, 20.0);
        assert_eq!(snapshot.p99_ms, 40.0);
    }

    #[test]
    fn default_capture_name_is_png_and_unique() {
        let first = default_capture_name();
        assert!(first.ends_with(".png"), "{first}");
        assert!(first.starts_with("capture_"), "{first}");
        // 同一毫秒内可能相同，这里只验证格式：名字里必须是纯数字时间戳
        let stamp = first
            .trim_start_matches("capture_")
            .trim_end_matches(".png");
        assert!(
            stamp.chars().all(|c| c.is_ascii_digit()),
            "时间戳应为纯数字: {first}"
        );
    }
    fn env_map_patch(equirect: Option<&str>, faces: Option<Vec<String>>) -> EnvironmentMapPatch {
        EnvironmentMapPatch {
            equirect: equirect.map(str::to_string),
            faces,
            ..Default::default()
        }
    }

    /// 宿主合并语义：段缺失时从缺省起步，未给的字段保留原值，回包给的是**收敛后**的值。
    #[test]
    fn environment_map_patch_merges_and_reports_source() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        let first = api
            .scene_set_environment_map(EnvironmentMapPatch {
                equirect: Some("env/studio.hdr".into()),
                face_size: Some(100),
                ..Default::default()
            })
            .expect("首次设置应成功");
        assert_eq!(first["enabled"], true, "有来源即生效: {first}");
        assert_eq!(first["source"], "equirect");
        // 100 不是 2 的幂，收敛后应向上取到 128（默认值恰好也是 128，这里验证的是「被写成收敛值」）
        assert_eq!(first["environment_map"]["face_size"], 128);

        // 第二次只调强度：路径 / 面边长必须原样保留，不能被默认值覆盖
        let second = api
            .scene_set_environment_map(EnvironmentMapPatch {
                intensity: Some(2.5),
                ..Default::default()
            })
            .expect("局部补丁应成功");
        assert_eq!(second["environment_map"]["equirect"], "env/studio.hdr");
        assert_eq!(second["environment_map"]["intensity"], 2.5);
        assert_eq!(second["environment_map"]["face_size"], 128);

        let map = scene.borrow().environment_map.clone().expect("段应落地");
        assert_eq!(map.face_size, 128);
        assert!(map.is_active());
    }

    /// 越界参数由宿主先夹到区间、再交给 `sanitized`：负数面边长不得变成 0 或 panic。
    #[test]
    fn environment_map_patch_clamps_sizes() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        let resp = api
            .scene_set_environment_map(EnvironmentMapPatch {
                equirect: Some("env/a.hdr".into()),
                face_size: Some(-8),
                mip_levels: Some(999),
                ..Default::default()
            })
            .expect("越界值应被夹取而不是报错");
        assert_eq!(
            resp["environment_map"]["face_size"], 4,
            "负数落到 4 像素下限"
        );
        assert_eq!(
            resp["environment_map"]["mip_levels"],
            r3e_core::envmap::MAX_SPECULAR_MIPS,
            "层数上限必须夹到 MAX_SPECULAR_MIPS"
        );

        let low = api
            .scene_set_environment_map(EnvironmentMapPatch {
                mip_levels: Some(0),
                ..Default::default()
            })
            .expect("层数 0 应被夹到 1");
        assert_eq!(low["environment_map"]["mip_levels"], 1);
    }

    /// 六面来源：长度不是 6 时 `sanitized` 判为无效 → 来源清空 → 整段丢弃，
    /// 渲染层立刻退回解析 IBL（这是「配错了不会静默用半套图」的关键路径）。
    #[test]
    fn environment_map_bad_faces_drop_section() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        let resp = api
            .scene_set_environment_map(env_map_patch(
                None,
                Some(vec!["px.hdr".into(), "nx.hdr".into()]),
            ))
            .expect("无效来源应给出可解释回包");
        assert_eq!(resp["enabled"], false, "只有 2 面拼不出立方体: {resp}");
        assert_eq!(resp["source"], "none");
        assert_eq!(resp["environment_map"], Value::Null);
        assert!(
            scene.borrow().environment_map.is_none(),
            "来源被判无效时应整段丢弃，而不是留个半残配置"
        );
    }

    /// 关开关只翻 `enabled`，路径必须留着——这正是「A/B 切换不丢配置」的用法。
    #[test]
    fn environment_map_disable_keeps_paths() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);

        api.scene_set_environment_map(env_map_patch(Some("env/studio.hdr"), None))
            .expect("先设置");
        let off = api
            .scene_set_environment_map(EnvironmentMapPatch {
                enabled: Some(false),
                ..Default::default()
            })
            .expect("关开关应成功");
        assert_eq!(off["enabled"], false);
        assert_eq!(off["source"], "disabled");
        assert_eq!(off["environment_map"]["equirect"], "env/studio.hdr");

        let map = scene
            .borrow()
            .environment_map
            .clone()
            .expect("段仍在，只是没启用");
        assert!(!map.is_active());
    }

    /// 快照字段名必须与场景 JSON 的 `environment_map` 段逐字一致，前端才能直接回写。
    #[test]
    fn environment_map_snapshot_matches_scene_schema() {
        let settings = r3e_core::envmap::EnvironmentMapSettings::with_equirect("env/x.hdr");
        let value = environment_map_value(settings.sanitized());
        let keys: Vec<&str> = value
            .as_object()
            .expect("快照应为对象")
            .keys()
            .map(String::as_str)
            .collect();
        for key in [
            "enabled",
            "equirect",
            "faces",
            "face_size",
            "mip_levels",
            "intensity",
        ] {
            assert!(keys.contains(&key), "快照缺少字段 `{key}`");
        }
        assert_eq!(keys.len(), 6, "不多不少六个字段，避免回写时多出未知键");
    }
}

#[cfg(test)]
mod scene_route_tests {
    //! 场景切换 / 存档这条链路上**不需要窗口**的那半段：路径解析规则与
    //! 「存档写盘 → 读回」的往返。真正换世界的部分在 `EngineApp` 里，需要
    //! 图形设备，交给 `cargo run` 的手动回归与 r3e-core 的纯函数单测覆盖。

    use super::resolve_request_path;
    use crate::app::default_archive_path;
    use r3e_core::scene_stack::SceneArchive;
    use r3e_core::Scene;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("r3e-scene-route-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn default_archive_path_appends_archive_suffix() {
        // `with_extension` 是整体替换后缀，所以 `lv1.json` → `lv1.archive.json`，
        // 而不是 `lv1.json.archive.json`——这条是面板上"另存存档"看到的默认名。
        assert_eq!(
            default_archive_path(std::path::Path::new("D:/a/lv1.json")),
            std::path::PathBuf::from("D:/a/lv1.archive.json")
        );
        // 没有后缀的场景文件也要产出可读的名字，不能变成 `lv1.archive`
        assert_eq!(
            default_archive_path(std::path::Path::new("D:/a/lv1")),
            std::path::PathBuf::from("D:/a/lv1.archive.json")
        );
    }

    #[test]
    fn resolve_request_path_keeps_absolute_and_existing() {
        let dir = temp_dir("abs");
        let file = dir.join("lv2.json");
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(
            resolve_request_path("D:/nowhere/lv1.json", file.to_str().unwrap()),
            file
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_request_path_falls_back_to_scene_directory() {
        let dir = temp_dir("rel");
        let scene = dir.join("lv1.json");
        std::fs::write(&scene, "{}").unwrap();
        let target = dir.join("lv2.json");
        std::fs::write(&target, "{}").unwrap();

        // 面板里填的是相对名：用户心里的"当前目录"是场景所在目录
        assert_eq!(
            resolve_request_path(scene.to_str().unwrap(), "lv2.json"),
            target
        );
        // 两边都不存在时原样返回，让调用方按"文件不存在"报错，而不是瞎猜
        assert_eq!(
            resolve_request_path(scene.to_str().unwrap(), "missing.json"),
            std::path::PathBuf::from("missing.json")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_written_to_disk_reads_back_with_runtime_state() {
        let dir = temp_dir("roundtrip");
        let mut scene =
            Scene::from_json_str(r#"{"name":"lv1","objects":[{"name":"box","mesh":"cube"}]}"#)
                .expect("场景解析");
        scene.time = 12.5;
        scene.frame = 750;

        let target = dir.join("lv1.archive.json");
        let archive = SceneArchive::from_scene(&scene);
        archive.save_to_path(&target).expect("存档写盘");

        let back = SceneArchive::load_path(&target).expect("存档读回");
        back.validate().expect("存档自校验");
        let (restored_scene, restored_anims) = back.build_scene().expect("重建场景");
        assert_eq!(restored_scene.len(), 1);
        assert_eq!(restored_scene.name, "lv1");
        // 声明态走 `scene` 段，运行时态走 `runtime` 段：时间 / 帧号这两样
        // `to_scene_file` 刻意不写的字段，存档必须原样带回来。
        assert!((restored_scene.time - 12.5).abs() < 1e-6);
        assert_eq!(restored_scene.frame, 750);
        // 场景里没有动画实例，恢复计数为 0 才是对的
        assert_eq!(restored_anims, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
