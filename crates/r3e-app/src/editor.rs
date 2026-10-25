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
use r3e_core::{Node, ObjectDesc, Scene};
use r3e_editor::{
    EngineApi, EngineInfo, NodeBrief, PhysicsInfo, RpcFailure, TransformPatch, PROTOCOL_VERSION,
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

    fn script_eval(&mut self, source: &str) -> Result<Value, RpcFailure> {
        let value = self.scripts.eval(source).map_err(RpcFailure::engine)?;
        Ok(dynamic_to_json(&value))
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
        "world": node.world.to_cols_array(),
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn script_reload_without_file_is_an_error() {
        let (scene, physics, mut scripts) = fixture();
        let mut api = api(&scene, &physics, &mut scripts);
        assert!(api.script_reload().is_err());
    }
}
