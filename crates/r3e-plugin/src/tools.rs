//! 工具实现：全部是纯计算 + 只读宿主能力，没有一条代码会写盘或执行外部命令。
//!
//! 设计约定：
//!
//! * 入参一律走 `opt_*` 取，类型不对就报 `-32602`，绝不静默用默认值糊过去——
//!   模型传错参数时拿到明确错误，比拿到一份「看起来正常」的错结果好得多；
//! * 场景可以内联（`scene`）或走 `host/fs.read`（`path`），后者受宿主 64 KB 限制；
//! * 回包控制在 64 KB 以内（宿主单次工具结果上限），所以节点/刚体/问题清单都截断
//!   并显式给出 `truncated` 标记，不假装全量。

use r3e_core::procedural::builtin_mesh;
use r3e_core::{
    air, scene::SceneFile, BuiltinMesh, MeshData, NodeId, Scene, Vec3,
};
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};
use serde_json::{json, Value};

use crate::host::HostError;

/// 单个工具调用内最多回多少字节的明细（宿主上限 64 KB，留出结构开销的余量）。
const MAX_LISTED_NODES: usize = 100;
const MAX_LISTED_ISSUES: usize = 20;

/// 工具层错误：直接映射成 JSON-RPC 错误帧。
#[derive(Debug)]
pub struct ToolError {
    pub code: i64,
    pub message: String,
}

impl ToolError {
    /// 入参不合法（缺字段、类型错、超出范围、工具不存在）。
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    /// 入参合法但内容解析不了（场景 JSON 坏了、物理配置不被后端接受）。
    ///
    /// 用 -32010 而不是 -32602：这不是「调用姿势错」，是「数据本身有问题」，
    /// 便于宿主/模型区分「改参数」还是「改场景」。
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            code: -32010,
            message: message.into(),
        }
    }

    /// 插件内部异常。
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.code)
    }
}

/// 工具需要的宿主能力（只读两个）。
pub trait HostBridge {
    fn fs_read(&mut self, path: &str) -> Result<String, HostError>;
    fn workspace_info(&mut self) -> Result<Value, HostError>;
}

/// 工具分发。
pub fn invoke(name: &str, args: &Value, bridge: &mut dyn HostBridge) -> Result<Value, ToolError> {
    if !args.is_null() && !args.is_object() {
        return Err(ToolError::invalid_params("arguments 必须是 JSON 对象"));
    }
    match name {
        "engine_info" => engine_info(),
        "scene_inspect" => scene_inspect(args, bridge),
        "mesh_stats" => mesh_stats(args),
        "scene_validate" => scene_validate(args, bridge),
        "physics_simulate" => physics_simulate(args, bridge),
        other => Err(ToolError::invalid_params(format!(
            "未知工具 `{other}`；本插件提供：{}",
            crate::spec::tool_names().join(", ")
        ))),
    }
}

/// 命令分发（命令面板入口，不是模型工具）。
pub fn invoke_command(name: &str, bridge: &mut dyn HostBridge) -> Result<Value, ToolError> {
    match name {
        "engine_diagnostics" => {
            let mut report = json!({ "engine": engine_info()? });
            match bridge.workspace_info() {
                Ok(info) => {
                    report["workspace"] = json!({
                        "available": !info.is_null(),
                        "info": info,
                    });
                }
                Err(error) => {
                    report["workspace"] = json!({
                        "available": false,
                        "error": error.to_string(),
                    });
                }
            }
            Ok(report)
        }
        other => Err(ToolError::invalid_params(format!(
            "未知命令 `{other}`；本插件提供：{}",
            crate::spec::command_names().join(", ")
        ))),
    }
}

// ---------------------------------------------------------------- 工具实现

fn engine_info() -> Result<Value, ToolError> {
    let backends: Vec<Value> = PhysicsKind::ALL
        .iter()
        .map(|kind| json!({ "name": kind.name(), "available": kind.is_available() }))
        .collect();
    let meshes: Vec<Value> = BuiltinMesh::ALL
        .iter()
        .map(|kind| json!({ "name": kind.as_str() }))
        .collect();
    Ok(json!({
        "engine": {
            "name": "rust3d-engine",
            "version": env!("CARGO_PKG_VERSION"),
            "crates": [
                { "name": "r3e-core", "role": "场景图 / 网格 / 动画 / AIR 校验，无图形依赖" },
                { "name": "r3e-render", "role": "渲染后端（插件不链接，headless 用不上）" },
                { "name": "r3e-physics", "role": "物理门面：null / builtin / rapier 三种后端" },
                { "name": "r3e-asset", "role": "资产导入（FBX/glTF 等，插件不链接）" },
                { "name": "r3e-script", "role": "脚本运行时" },
                { "name": "r3e-app", "role": "引擎主程序 r3e.exe" },
                { "name": "r3e-editor", "role": "编辑器 RPC 接口" }
            ]
        },
        "plugin": {
            "id": crate::spec::PLUGIN_ID,
            "version": env!("CARGO_PKG_VERSION"),
            "pluginApi": crate::spec::PLUGIN_API,
            "transport": "ndjson-stdio",
            "frameLimitBytes": crate::protocol::MAX_FRAME_BYTES,
            "hostCallQuotaPerTool": crate::host::QUOTA_PER_CALL,
            "capabilities": crate::spec::CAPABILITIES,
            "tools": crate::spec::tool_names(),
        },
        "physics": {
            "backends": backends,
            "features": { "rapier": cfg!(feature = "rapier") },
        },
        "meshes": meshes,
    }))
}

fn scene_inspect(args: &Value, bridge: &mut dyn HostBridge) -> Result<Value, ToolError> {
    let scene = load_scene(args, bridge)?;
    let limit = opt_u32(args, "maxNodes")?.unwrap_or(50).clamp(1, 500) as usize;

    let mut nodes = Vec::new();
    let mut asset_meshes = 0usize;
    for node in scene.iter() {
        if node.mesh.builtin().is_none() {
            asset_meshes += 1;
        }
        if nodes.len() >= limit {
            continue;
        }
        nodes.push(json!({
            "id": node.id,
            "name": node.name,
            "mesh": node.mesh.as_string(),
            "meshKind": if node.mesh.builtin().is_some() { "builtin" } else { "asset" },
            "visible": node.visible,
            "parent": node.parent,
            "localPosition": vec3_json(node.transform.position),
            "worldPosition": vec3_json(node.world_position()),
            "scale": vec3_json(node.transform.scale),
            "physics": node.physics.as_ref().map(|desc| json!({
                "body": desc.body,
                "mass": desc.mass,
                "collider": desc.collider,
            })),
        }));
    }

    Ok(json!({
        "name": scene.name,
        "nodeCount": scene.len(),
        "listedCount": nodes.len(),
        "truncated": scene.len() > nodes.len(),
        "assetMeshCount": asset_meshes,
        "camera": camera_json(&scene.camera),
        "light": light_json(&scene.light),
        "physics": physics_desc_json(&scene.physics),
        "nodes": nodes,
    }))
}

fn mesh_stats(args: &Value) -> Result<Value, ToolError> {
    let shape = opt_str(args, "shape")?.unwrap_or("cube");
    let kind = BuiltinMesh::parse(shape).ok_or_else(|| {
        ToolError::invalid_params(format!(
            "shape `{shape}` 不认识；可选：cube / sphere / plane / cylinder"
        ))
    })?;
    let segments = opt_u32(args, "segments")?.unwrap_or(32).clamp(3, 256);
    let rings = opt_u32(args, "rings")?.unwrap_or(16).clamp(2, 256);
    let divisions = opt_u32(args, "divisions")?.unwrap_or(1).clamp(1, 128);

    let mesh = match kind {
        BuiltinMesh::Cube => r3e_core::procedural::cube(),
        BuiltinMesh::Sphere => r3e_core::procedural::sphere(segments, rings),
        BuiltinMesh::Plane => r3e_core::procedural::plane(divisions),
        BuiltinMesh::Cylinder => r3e_core::procedural::cylinder(segments),
    };
    let (min, max) = mesh.aabb();
    let issues = air::validate(&mesh);

    Ok(json!({
        "shape": kind.as_str(),
        "tessellation": {
            "segments": if matches!(kind, BuiltinMesh::Sphere | BuiltinMesh::Cylinder) { Some(segments) } else { None },
            "rings": if matches!(kind, BuiltinMesh::Sphere) { Some(rings) } else { None },
            "divisions": if matches!(kind, BuiltinMesh::Plane) { Some(divisions) } else { None },
        },
        "vertices": mesh.vertex_count(),
        "triangles": mesh.triangle_count(),
        "indices": mesh.indices.len(),
        "aabb": {
            "min": vec3_json(min),
            "max": vec3_json(max),
            "size": vec3_json(max - min),
        },
        "fingerprint": fingerprint(&mesh),
        "air": {
            "ok": issues.is_empty(),
            "issueCount": issues.len(),
            "issues": issue_list(&issues),
        },
    }))
}

fn scene_validate(args: &Value, bridge: &mut dyn HostBridge) -> Result<Value, ToolError> {
    let scene = load_scene(args, bridge)?;

    let mut reports = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_issues = 0usize;

    for node in scene.iter() {
        let entry = match node.mesh.builtin() {
            Some(kind) => {
                let mesh = builtin_mesh(kind);
                let issues = air::validate(&mesh);
                total_issues += issues.len();
                json!({
                    "name": node.name,
                    "mesh": node.mesh.as_string(),
                    "air": {
                        "ok": issues.is_empty(),
                        "issueCount": issues.len(),
                        "issues": issue_list(&issues),
                    },
                })
            }
            None => {
                // 资产网格要走 r3e-asset 导入，本插件不带资产管线——如实标注 skipped，
                // 而不是给一个「0 问题」的假绿灯。
                json!({
                    "name": node.name,
                    "mesh": node.mesh.as_string(),
                    "air": {
                        "ok": Value::Null,
                        "skipped": true,
                        "reason": "external-asset",
                    },
                })
            }
        };
        if reports.len() < MAX_LISTED_NODES {
            reports.push(entry);
        }
    }

    // 重名无需在这里检测：`Scene::from_file` 构建阶段就会因重名直接报错
    // （实测「物件 `x` 已存在」），走不到这一步。这里只补场景级的一般性提醒。
    if scene.is_empty() {
        warnings.push("场景没有任何节点".to_string());
    }
    if reports.iter().any(|report| report["air"]["skipped"] == json!(true)) {
        warnings.push(
            "部分节点使用外部资产网格，AIR 校验需要资产管线（r3e-asset），本插件跳过".to_string(),
        );
    }

    Ok(json!({
        "ok": total_issues == 0,
        "name": scene.name,
        "nodeCount": scene.len(),
        "listedCount": reports.len(),
        "truncated": scene.len() > reports.len(),
        "issueCount": total_issues,
        "warnings": warnings,
        "nodes": reports,
    }))
}

fn physics_simulate(args: &Value, bridge: &mut dyn HostBridge) -> Result<Value, ToolError> {
    let mut scene = load_scene(args, bridge)?;
    let steps = opt_u32(args, "steps")?.unwrap_or(120).clamp(1, 2000);
    let dt = opt_f32(args, "dt")?.unwrap_or(1.0 / 60.0);
    if !(0.001..=0.1).contains(&dt) {
        return Err(ToolError::invalid_params(format!(
            "dt={dt} 超出范围 0.001..=0.1"
        )));
    }
    let max_bodies = opt_u32(args, "maxBodies")?.unwrap_or(20).clamp(1, 64) as usize;

    // 模拟场景就是要算：显式把物理打开（场景写 enabled:false 时也照跑），
    // 但后端选择尊重调用方 > 场景 > 引擎默认的优先级。
    let mut config = PhysicsConfig::from_scene(&scene.physics).with_enabled(true);
    if let Some(name) = opt_str(args, "backend")? {
        let kind = PhysicsKind::parse(name).ok_or_else(|| {
            ToolError::invalid_params(format!(
                "backend `{name}` 不认识；可选：null / builtin / rapier"
            ))
        })?;
        if !kind.is_available() {
            let available: Vec<&str> = PhysicsKind::ALL
                .iter()
                .filter(|kind| kind.is_available())
                .map(|kind| kind.name())
                .collect();
            return Err(ToolError::invalid_params(format!(
                "物理后端 `{name}` 未编进本插件二进制（当前可用：{}）；需要 Rapier 请用 `--features rapier` 重新打包",
                available.join(" / ")
            )));
        }
        config = config.with_kind(kind);
    }
    if let Some(gravity) = opt_vec3(args, "gravity")? {
        config.gravity = Some(gravity);
    }
    if let Some(ground) = opt_f32(args, "ground")? {
        config.ground = Some(ground);
    }

    let mut physics = Physics::new();
    physics
        .apply_config(config)
        .map_err(|err| ToolError::invalid_input(format!("物理配置被后端拒绝：{err}")))?;
    // 后端名以后端自报为准：调用方没指定时也能如实说明「实际用的是哪个」
    let backend_name = physics.backend_name();

    // 逐节点建刚体：只有声明了 physics 的节点才进物理世界。
    let mut bound: Vec<(NodeId, String, Vec3)> = Vec::new();
    for node in scene.iter() {
        if node.physics.is_none() {
            continue;
        }
        let mesh = node.mesh.builtin().map(builtin_mesh);
        let Some(desc) = BodyDesc::from_node_with(node, mesh.as_ref()) else {
            continue;
        };
        let id = node.id;
        let name = node.name.clone();
        let start = node.world_position();
        if physics.try_create_body(id, desc).is_some() {
            bound.push((id, name, start));
        }
    }

    let mut event_total = 0usize;
    let mut event_sample: Vec<String> = Vec::new();
    scene.update_world_transforms();
    for _ in 0..steps {
        physics.step(dt, &mut scene);
        scene.update_world_transforms();
        let events = physics.drain_events();
        event_total += events.len();
        for event in events {
            if event_sample.len() < 10 {
                event_sample.push(event.to_text());
            }
        }
    }

    let mut bodies = Vec::new();
    let mut max_displacement = 0.0f32;
    for (id, name, start) in &bound {
        let end = scene.get(*id).map(|node| node.world_position()).unwrap_or(*start);
        let displacement = (end - *start).length();
        max_displacement = max_displacement.max(displacement);
        if bodies.len() < max_bodies {
            bodies.push(json!({
                "node": name,
                "nodeId": id,
                "start": vec3_json(*start),
                "end": vec3_json(end),
                "displacement": round(displacement),
            }));
        }
    }

    let caps = physics.capabilities();
    Ok(json!({
        "ok": true,
        "backend": backend_name,
        "steps": steps,
        "dt": round(dt),
        "simulatedSeconds": round(steps as f32 * dt),
        "bodyCount": bound.len(),
        "listedCount": bodies.len(),
        "truncated": bound.len() > bodies.len(),
        "maxDisplacement": round(max_displacement),
        "events": {
            "total": event_total,
            "sample": event_sample,
            "sampledOut": event_total > event_sample.len(),
        },
        "bodies": bodies,
        "capabilities": {
            "bodyBodyContacts": caps.body_body_contacts,
            "rotationalDynamics": caps.rotational_dynamics,
            "joints": caps.joints,
            "ccd": caps.ccd,
            "collisionEvents": caps.collision_events,
            "raycast": caps.raycast,
            "overlap": caps.overlap,
            "shapeCast": caps.shape_cast,
        },
        "scene": { "name": scene.name, "nodeCount": scene.len() },
    }))
}

// ---------------------------------------------------------------- 公共小工具

/// 场景来源：内联对象 / 内联 JSON 文本 / 工作区文件（走 `host/fs.read`）。
fn load_scene(args: &Value, bridge: &mut dyn HostBridge) -> Result<Scene, ToolError> {
    if let Some(path) = opt_str(args, "path")? {
        let text = match bridge.fs_read(path) {
            Ok(text) => text,
            // 宿主明确拒绝（未授权 / 越界 / 文件不存在）：属于「输入有问题」，让调用方换路径
            Err(err) if err.is_remote() => {
                return Err(ToolError::invalid_input(format!(
                    "读取场景 `{path}` 失败：{err}"
                )))
            }
            // 管道断了：重试也没用，按内部错误上报
            Err(err) => {
                return Err(ToolError::internal(format!(
                    "与宿主通信失败，读不到场景 `{path}`：{err}"
                )))
            }
        };
        return Scene::from_json_str(&text)
            .map_err(|err| ToolError::invalid_input(format!("场景 `{path}` 解析失败：{err}")));
    }
    match args.get("scene") {
        Some(Value::String(text)) => Scene::from_json_str(text)
            .map_err(|err| ToolError::invalid_input(format!("scene 文本解析失败：{err}"))),
        Some(value @ Value::Object(_)) => {
            let file: SceneFile = serde_json::from_value(value.clone())
                .map_err(|err| ToolError::invalid_input(format!("scene 对象结构不对：{err}")))?;
            Scene::from_file(file)
                .map_err(|err| ToolError::invalid_input(format!("scene 构造场景失败：{err}")))
        }
        Some(_) => Err(ToolError::invalid_params("scene 必须是对象或 JSON 文本")),
        None => Err(ToolError::invalid_params(
            "必须提供 scene（场景对象 / JSON 文本）或 path（工作区相对路径）",
        )),
    }
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(other) => Err(ToolError::invalid_params(format!(
            "{key} 必须是字符串，实际是 {}",
            type_name(other)
        ))),
    }
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .map(Some)
            .ok_or_else(|| {
                ToolError::invalid_params(format!(
                    "{key} 必须是非负整数，实际是 {}",
                    type_name(value)
                ))
            }),
    }
}

fn opt_f32(args: &Value, key: &str) -> Result<Option<f32>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|value| Some(value as f32))
            .ok_or_else(|| ToolError::invalid_params(format!("{key} 不是有效数值"))),
        Some(other) => Err(ToolError::invalid_params(format!(
            "{key} 必须是数值，实际是 {}",
            type_name(other)
        ))),
    }
}

fn opt_vec3(args: &Value, key: &str) -> Result<Option<Vec3>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) if items.len() == 3 => {
            let mut values = [0f32; 3];
            for (index, item) in items.iter().enumerate() {
                let number = item.as_f64().ok_or_else(|| {
                    ToolError::invalid_params(format!("{key}[{index}] 必须是数值"))
                })?;
                values[index] = number as f32;
            }
            Ok(Some(Vec3::from(values)))
        }
        Some(_) => Err(ToolError::invalid_params(format!(
            "{key} 必须是长度为 3 的数值数组"
        ))),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn issue_list(issues: &[air::AirIssue]) -> Vec<Value> {
    issues
        .iter()
        .take(MAX_LISTED_ISSUES)
        .map(|issue| json!({ "kind": issue.kind.as_str(), "detail": issue.detail }))
        .collect()
}

fn vec3_json(value: Vec3) -> Value {
    json!([round(value.x), round(value.y), round(value.z)])
}

/// `Camera` / `Light` / `PhysicsSceneDesc` 都是「只 Deserialize」的运行时结构
/// （引擎不需要把它们写回场景文件），所以这里手写摘要，而不是硬要上游加 Serialize
/// ——插件不该为了自己的输出格式去改核心 crate 的公开契约。
fn camera_json(camera: &r3e_core::Camera) -> Value {
    json!({
        "position": vec3_json(camera.position),
        "target": vec3_json(camera.target),
        "up": vec3_json(camera.up),
        "fovDeg": round(camera.fov_deg),
        "znear": round(camera.znear),
        "zfar": round(camera.zfar),
        "aspect": round(camera.aspect),
    })
}

fn light_json(light: &r3e_core::Light) -> Value {
    json!({
        "direction": vec3_json(light.direction),
        "color": vec3_json(light.color),
        "intensity": round(light.intensity),
        "ambient": vec3_json(light.ambient),
        "shadow": {
            "enabled": light.shadow.enabled,
            "size": light.shadow.size,
            "orthoSize": round(light.shadow.ortho_size),
            "strength": round(light.shadow.strength),
        },
    })
}

fn physics_desc_json(desc: &r3e_core::scene::PhysicsSceneDesc) -> Value {
    json!({
        "enabled": desc.enabled,
        "backend": desc.backend,
        "gravity": desc.gravity.map(|value| json!([
            round(value[0]),
            round(value[1]),
            round(value[2])
        ])),
        "ground": desc.ground.map(round),
        "ccd": desc.ccd,
    })
}

/// 保留 4 位小数：工具回包给模型看，尾数过长的浮点纯属噪声。
fn round(value: f32) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    (f64::from(value) * 10_000.0).round() / 10_000.0
}

/// FNV-1a 64：给网格一个稳定指纹，用于「同一图元两次生成是否一致」的比对。
fn fingerprint(mesh: &MeshData) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    mix(&(mesh.vertices.len() as u64).to_le_bytes());
    mix(&(mesh.indices.len() as u64).to_le_bytes());
    for vertex in &mesh.vertices {
        let position = vertex.position();
        for axis in [position.x, position.y, position.z] {
            mix(&axis.to_bits().to_le_bytes());
        }
    }
    for index in &mesh.indices {
        mix(&index.to_le_bytes());
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost;

    impl HostBridge for NoHost {
        fn fs_read(&mut self, _path: &str) -> Result<String, HostError> {
            Err(HostError::Remote {
                code: 6103,
                message: "能力尚未授权：fs.read".to_string(),
            })
        }

        fn workspace_info(&mut self) -> Result<Value, HostError> {
            Ok(Value::Null)
        }
    }

    const DEMO: &str = r#"{
        "name": "demo",
        "camera": { "position": [0, 2, 6], "target": [0, 1, 0], "fov": 60 },
        "light": { "direction": [-0.4, -1, -0.3], "intensity": 1.2 },
        "physics": { "enabled": true, "backend": "builtin", "gravity": [0, -9.81, 0], "ground": 0 },
        "objects": [
            { "name": "floor", "mesh": "builtin:plane", "scale": [8, 1, 8], "physics": { "body": "static" } },
            { "name": "crate", "mesh": "builtin:cube", "position": [0, 3, 0], "physics": { "body": "dynamic" } }
        ]
    }"#;

    fn demo_args() -> Value {
        json!({ "scene": DEMO })
    }

    #[test]
    fn engine_info_lists_backends_and_tools() {
        let info = invoke("engine_info", &json!({}), &mut NoHost).expect("engine_info");
        assert_eq!(info["engine"]["name"], json!("rust3d-engine"));
        assert_eq!(info["plugin"]["id"], json!(crate::spec::PLUGIN_ID));
        let backends = info["physics"]["backends"].as_array().expect("backends");
        assert!(backends.iter().any(|item| item["name"] == json!("builtin")));
        assert!(backends
            .iter()
            .any(|item| item["name"] == json!("rapier") && item["available"] == json!(cfg!(feature = "rapier"))));
    }

    #[test]
    fn scene_inspect_reports_nodes() {
        let report = invoke("scene_inspect", &demo_args(), &mut NoHost).expect("scene_inspect");
        assert_eq!(report["nodeCount"], json!(2));
        assert_eq!(report["listedCount"], json!(2));
        assert_eq!(report["truncated"], json!(false));
        assert_eq!(report["nodes"][0]["name"], json!("floor"));
        assert_eq!(report["nodes"][0]["meshKind"], json!("builtin"));
        assert_eq!(report["nodes"][1]["physics"]["body"], json!("dynamic"));
    }

    #[test]
    fn scene_inspect_respects_max_nodes() {
        let mut args = demo_args();
        args["maxNodes"] = json!(1);
        let report = invoke("scene_inspect", &args, &mut NoHost).expect("scene_inspect");
        assert_eq!(report["listedCount"], json!(1));
        assert_eq!(report["truncated"], json!(true));
    }

    #[test]
    fn mesh_stats_matches_known_primitive_counts() {
        let cube = invoke("mesh_stats", &json!({}), &mut NoHost).expect("cube");
        assert_eq!(cube["shape"], json!("cube"));
        assert_eq!(cube["vertices"], json!(24));
        assert_eq!(cube["triangles"], json!(12));
        assert_eq!(cube["air"]["ok"], json!(true));

        let sphere = invoke("mesh_stats", &json!({ "shape": "sphere", "segments": 8, "rings": 4 }), &mut NoHost)
            .expect("sphere");
        assert_eq!(sphere["shape"], json!("sphere"));
        assert!(sphere["vertices"].as_u64().expect("vertices") > 0);
        // 同参数两次生成必须同指纹
        let again = invoke("mesh_stats", &json!({ "shape": "sphere", "segments": 8, "rings": 4 }), &mut NoHost)
            .expect("sphere again");
        assert_eq!(sphere["fingerprint"], again["fingerprint"]);
    }

    #[test]
    fn mesh_stats_rejects_unknown_shape() {
        let error = invoke("mesh_stats", &json!({ "shape": "teapot" }), &mut NoHost)
            .expect_err("未知图元必须报错");
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn scene_validate_rejects_duplicate_node_names() {
        // 引擎自己在建场景时就拒绝重名，插件只需如实转达这层约束
        let scene = json!({
            "name": "dup",
            "objects": [
                { "name": "same", "mesh": "builtin:cube" },
                { "name": "same", "mesh": "builtin:sphere" }
            ]
        });
        let error = invoke("scene_validate", &json!({ "scene": scene }), &mut NoHost)
            .expect_err("重名场景应当在构建阶段就被拒");
        assert_eq!(error.code, -32010);
        assert!(error.message.contains("same"));
    }

    #[test]
    fn scene_validate_marks_asset_meshes_as_skipped() {
        let scene = json!({
            "name": "asset",
            "objects": [ { "name": "hero", "mesh": "asset:models/hero.fbx#0" } ]
        });
        let report = invoke("scene_validate", &json!({ "scene": scene }), &mut NoHost).expect("validate");
        assert_eq!(report["nodes"][0]["air"]["skipped"], json!(true));
        assert_eq!(report["nodes"][0]["air"]["reason"], json!("external-asset"));
        assert_eq!(report["nodes"][0]["air"]["ok"], Value::Null);
    }

    #[test]
    fn physics_simulate_moves_dynamic_body() {
        let report = invoke("physics_simulate", &demo_args(), &mut NoHost).expect("simulate");
        assert_eq!(report["ok"], json!(true));
        assert_eq!(report["backend"], json!("builtin"));
        assert_eq!(report["bodyCount"], json!(2));
        // 两个刚体都登记了，动态的那个必须真的掉下来（重力生效）
        let max = report["maxDisplacement"].as_f64().expect("maxDisplacement");
        assert!(max > 0.0, "动态刚体应当发生位移，实际 {max}");
    }

    #[test]
    fn physics_simulate_rejects_unavailable_backend() {
        let mut args = demo_args();
        args["backend"] = json!("rapier");
        if cfg!(feature = "rapier") {
            invoke("physics_simulate", &args, &mut NoHost).expect("rapier 已编入");
        } else {
            let error = invoke("physics_simulate", &args, &mut NoHost).expect_err("未编入必须报错");
            assert_eq!(error.code, -32602);
            assert!(error.message.contains("rapier"));
        }
    }

    /// 场景文件自己写了 `physics.backend = "rapier"`、而本二进制没编 rapier 时，
    /// 必须走「结构化错误」而不是 panic（宿主看到的是一个正常错误帧，不是崩溃重启）。
    #[test]
    fn scene_declared_unavailable_backend_is_structured_error() {
        let scene = json!({
            "name": "rapier-scene",
            "physics": { "enabled": true, "backend": "rapier" },
            "objects": [
                { "name": "crate", "mesh": "builtin:cube", "position": [0, 3, 0], "physics": { "body": "dynamic" } }
            ]
        });
        let outcome = std::panic::catch_unwind(|| {
            invoke("physics_simulate", &json!({ "scene": scene }), &mut NoHost)
        });
        let result = outcome.expect("未编入的后端绝不允许 panic");
        if cfg!(feature = "rapier") {
            result.expect("rapier 已编入，应当跑通");
        } else {
            let error = result.expect_err("未编入必须返回错误");
            assert_eq!(error.code, -32010, "场景数据问题走 -32010");
            assert!(error.message.contains("rapier"), "错误信息要点明后端名：{}", error.message);
        }
    }

    /// `backend` 显式传 rapier（未编入）时，错误信息必须给出「怎么办」，方便模型自我纠正。
    #[test]
    fn unavailable_backend_error_is_actionable() {
        if cfg!(feature = "rapier") {
            return;
        }
        let mut args = demo_args();
        args["backend"] = json!("rapier");
        let error = invoke("physics_simulate", &args, &mut NoHost).expect_err("未编入必须报错");
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("--features rapier"), "{}", error.message);
        assert!(error.message.contains("builtin"), "错误里要列出可用后端：{}", error.message);
    }

    #[test]
    fn physics_simulate_rejects_out_of_range_dt() {
        let mut args = demo_args();
        args["dt"] = json!(1.5);
        let error = invoke("physics_simulate", &args, &mut NoHost).expect_err("dt 超范围");
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn path_without_permission_surfaces_host_error() {
        let error = invoke("scene_inspect", &json!({ "path": "assets/demo.json" }), &mut NoHost)
            .expect_err("无授权必须报错");
        assert_eq!(error.code, -32010);
        assert!(error.message.contains("6103"));
    }

    #[test]
    fn broken_scene_text_is_invalid_input() {
        let error = invoke("scene_inspect", &json!({ "scene": "{oops" }), &mut NoHost)
            .expect_err("坏 JSON");
        assert_eq!(error.code, -32010);
    }

    #[test]
    fn wrong_argument_type_is_rejected() {
        let mut args = demo_args();
        args["maxNodes"] = json!("many");
        let error = invoke("scene_inspect", &args, &mut NoHost).expect_err("类型错");
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let error = invoke("nope", &json!({}), &mut NoHost).expect_err("未知工具");
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn diagnostics_command_tolerates_missing_workspace() {
        let report = invoke_command("engine_diagnostics", &mut NoHost).expect("diagnostics");
        assert_eq!(report["engine"]["engine"]["name"], json!("rust3d-engine"));
        assert_eq!(report["workspace"]["available"], json!(false));
    }
}
