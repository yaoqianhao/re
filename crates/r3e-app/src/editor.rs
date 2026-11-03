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
use std::rc::Rc;

use glam::{Quat, Vec3};
use r3e_core::postfx::{PostFxSettings, TonemapMode};
use r3e_core::light::MAX_LOCAL_LIGHTS;
use r3e_core::{MAX_CASCADES, ShadowSettings};
use r3e_core::ui::{self, Anchor, UiElement, UiKind, UiUnit, UiUpsert, UiValue};
use r3e_core::{LocalLight, LocalLightKind, Node, ObjectDesc, Scene};
use r3e_editor::{
    EngineApi, EngineInfo, EnvironmentPatch, LightPatch, MaterialPatch, NodeBrief, PhysicsInfo,
    PostFxPatch, RpcFailure, ShadowPatch, TransformPatch, UiPatch, PROTOCOL_VERSION,
};
use r3e_physics::Physics;
use r3e_script::ScriptHost;
use serde_json::{json, Value};

/// 引擎信息里的引擎名（与 Cargo 包名、窗口标题保持一致）。
const ENGINE_NAME: &str = "r3e";

/// 主线程上的 [`EngineApi`] 实现。
///
/// 所有方法都在事件循环线程调用，所以可以直接 `borrow_mut` 场景与物理，
/// 既不需要锁，也不会出现「RPC 线程改场景、渲染线程正在读」的撕裂。
pub struct LocalEngineApi<'a> {
    pub scene: &'a Rc<RefCell<Scene>>,
    pub physics: &'a Rc<RefCell<Physics>>,
    pub scripts: &'a mut ScriptHost,
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
                RpcFailure::invalid_params(format!(
                    "未知光源类型 `{name}`（可用：point / spot）"
                ))
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
                RpcFailure::invalid_params(format!("未知锚点 `{name}`（可用：top_left / center ...）"))
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
    use r3e_core::light::MIN_LIGHT_RANGE;
    use r3e_core::InputState;

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
        assert_eq!(clamped["postfx"]["exposure"], json!(r3e_core::postfx::MAX_EXPOSURE_EV));
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
        scene
            .borrow_mut()
            .ui
            .push(UiElement {
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
        assert!(err.message.contains(&MAX_LOCAL_LIGHTS.to_string()), "{}", err.message);

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
            "key", "kind", "pos", "dir", "color", "intensity", "range", "inner_deg", "outer_deg",
        ] {
            assert!(arr[0].get(field).is_some(), "快照缺少字段 {field}");
        }

        let limited = api.light_list(Some(1)).expect("应成功");
        assert_eq!(limited["count"], 2, "count 是全量");
        assert_eq!(limited["lights"].as_array().unwrap().len(), 1, "limit 只截结果");

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
            "enabled", "size", "strength", "orthoSize", "bias", "normalBias", "cascades",
            "maxCascades", "blend", "distance", "lambda",
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
        assert_eq!(clamped["blend"], 0.5, "混合比上限是 0.5（再大就跨级采到不存在的层）");
        assert_eq!(clamped["size"], 1000, "只要在 64..8192 内就原样保留（不强行取整）");
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
}
