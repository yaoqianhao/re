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
use r3e_core::{Node, ObjectDesc, Scene};
use r3e_editor::{
    EngineApi, EngineInfo, EnvironmentPatch, MaterialPatch, NodeBrief, PhysicsInfo, PostFxPatch,
    RpcFailure,
    TransformPatch, PROTOCOL_VERSION,
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

    fn script_eval(&mut self, source: &str) -> Result<Value, RpcFailure> {
        let value = self.scripts.eval(source).map_err(RpcFailure::engine)?;
        Ok(dynamic_to_json(&value))
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
}
