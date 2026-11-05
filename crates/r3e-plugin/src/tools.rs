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
use r3e_core::{air, scene::SceneFile, BuiltinMesh, MeshData, NodeId, Scene, Vec3};
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};
use serde_json::{json, Value};

use crate::host::HostError;
use crate::rpc::{self, Endpoint, RpcClient};

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
        // —— 写侧（risk = write）：全部经引擎既有写 RPC ——
        "scene_patch" => scene_patch(args),
        "node_spawn" => node_spawn(args),
        "script_write" => script_write(args),
        "scene_save" => scene_save(args),
        // —— 反馈（risk = read）：只读引擎的统计与截图，同样走引擎 RPC ——
        "engine_stats" => engine_stats(args),
        "capture_frame" => capture_frame(args),
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
        // 后处理整段（扩展十三起）。这里只摘要字段，不复刻 core 的收敛规则：
        // 插件报的是场景文件里写了什么，实际生效值由引擎的 sanitized 决定。
        "postfx": scene.postfx.map(|fx| json!({
            "exposure": round(fx.exposure),
            "tonemap": fx.tonemap.name(),
            "fxaa": fx.fxaa,
            "bloom": fx.bloom.enabled,
            // 扩展十七：遮蔽配置。模型要判断「墙角发黑」是遮蔽还是曝光/泛光所致，
            // 得先看得到这一段；`samples` 是写的原值，引擎会夹进 [4, 16]。
            "ssao": {
                "enabled": fx.ssao.enabled,
                "radius": round(fx.ssao.radius),
                "bias": round(fx.ssao.bias),
                "intensity": round(fx.ssao.intensity),
                "samples": fx.ssao.samples,
            },
        })),
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
    let mut unvalidated = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_issues = 0usize;

    // P0-2：每条问题都带 `location`（节点 id/名字、场景文件 `objects` 下标、
    // 字段路径、资源来源、网格锚点、材质上下文），模型拿到就能直接定位要改哪一处，
    // 不用再解析 `detail` 里那句人话。`objects` 顺序与 `scene.iter()` 一致，
    // 因为 `to_scene_file` 就是按同一个 `order` 表导出的。
    for (object_index, node) in scene.iter().enumerate() {
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
                        "issues": issues
                            .iter()
                            .take(MAX_LISTED_ISSUES)
                            .map(|issue| crate::locate::issue_json(&scene, node, object_index, issue, Some(&mesh)))
                            .collect::<Vec<Value>>(),
                    },
                })
            }
            None => {
                // 资产网格要走 r3e-asset 导入，本插件不带资产管线——如实标注 skipped，
                // 而不是给一个「0 问题」的假绿灯；同时在 `unvalidated` 里留一条带定位
                // 的账（不并进 `issueCount`，否则「有资产 = 必失败」会让 ok 失去意义）。
                unvalidated.push(crate::locate::skipped_issue_json(
                    &scene,
                    node,
                    object_index,
                ));
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
    if reports
        .iter()
        .any(|report| report["air"]["skipped"] == json!(true))
    {
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
        // 没校验成的节点也给出定位：模型应当知道「这里没结论」，而不是以为通过。
        "unvalidatedCount": unvalidated.len(),
        "unvalidated": unvalidated,
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
        let end = scene
            .get(*id)
            .map(|node| node.world_position())
            .unwrap_or(*start);
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

// ---------------------------------------------------------------- 写侧工具（P0-3）
//
// 四个写工具全部是「薄壳 + 复用引擎既有写 RPC」，不新造协议：
// 参数校验在插件侧只做「够不够/类型对不对」，值域与语义仍由引擎裁决，
// 这样插件永远与终端的 Ctrl+S / 脚本 API 走同一条路，不会出现两套真相。
//
// 与只读工具的差别只有三点：
// 1) `risk = write`，宿主会走授权流程；
// 2) 需要知道引擎地址（`endpoint` / `R3E_RPC_ADDR`），读工具则完全离线；
// 3) 改完都回读一次（`snapshot` / `result`），让模型确认「改成了什么样」。

/// 连引擎；连不上时给一条能照着修的提示，而不是光说失败。
fn connect_engine(args: &Value) -> Result<RpcClient, ToolError> {
    let endpoint = Endpoint::from_args(args);
    let addr = endpoint.addr.clone();
    RpcClient::connect(&endpoint).map_err(|err| ToolError {
        // -32011 提供方自定义段：区别于「参数错」（-32602）与「数据坏」（-32010）——
        // 这种失败的正确反应是「去启动引擎」，改参数没用。
        code: -32011,
        message: format!("引擎未连接（{addr}）：{}", err.advice()),
    })
}

/// 引擎侧错误 → 工具错误。引擎的错误码承载语义（-32602 参数错、-32004 找不到
/// 物件…），原样透传，只补上方法名，方便模型判断该改参数还是改场景。
fn rpc_error(method: &str, err: rpc::RpcError) -> ToolError {
    match err {
        rpc::RpcError::Remote { code, message } => ToolError {
            code,
            message: format!("`{method}` 被引擎拒绝：{message}"),
        },
        other => ToolError {
            code: -32011,
            message: format!("`{method}` 调用失败：{}", other.advice()),
        },
    }
}

fn call_engine<T>(
    client: &mut RpcClient,
    method: &str,
    params: Value,
    f: impl FnOnce(Value) -> T,
) -> Result<T, ToolError> {
    client
        .call(method, params)
        .map(f)
        .map_err(|err| rpc_error(method, err))
}

/// 补丁式改一个已有节点：只改给了的字段，没给的保持原样。
///
/// 之所以要 `scene_patch` 而不是让模型直接拼 `scene.settransform`：
/// 模型手上拿到的是 `scene_inspect` 的扁平快照，一次「把 hero 抬高 0.5 并调亮材质」
/// 在 RPC 层是两个方法、两套参数；这里合成一次调用，失败也不会只改一半。
fn scene_patch(args: &Value) -> Result<Value, ToolError> {
    let node = require_str(args, "node")?;
    let position = opt_f32_list::<3>(args, "position")?;
    let rotation = opt_f32_list::<4>(args, "rotation")?;
    let scale = opt_f32_list::<3>(args, "scale")?;

    let material = match args.get("material") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_object().ok_or_else(|| {
            ToolError::invalid_params(
                "material 必须是对象，形如 {\"metallic\":0.2,\"roughness\":0.4}",
            )
        })?),
    };
    if let Some(material) = material {
        // 未知键直接拒绝：模型写 `emissiveColor` / `roughnessFactor` 时若不报错，
        // 调用会「成功」但什么也没改，那是最难查的一类哑失败。
        const KNOWN: [&str; 3] = ["metallic", "roughness", "emissive"];
        if let Some(unknown) = material.keys().find(|key| !KNOWN.contains(&key.as_str())) {
            return Err(ToolError::invalid_params(format!(
                "material 不支持字段 `{unknown}`；可用：{}",
                KNOWN.join(" / ")
            )));
        }
    }
    let metallic = material
        .map(|m| opt_material_f32(m, "metallic"))
        .transpose()?
        .flatten();
    let roughness = material
        .map(|m| opt_material_f32(m, "roughness"))
        .transpose()?
        .flatten();
    let emissive = material
        .map(|m| opt_material_f32_list::<3>(m, "emissive"))
        .transpose()?
        .flatten();

    let mut calls: Vec<(&'static str, Vec<String>, Value)> = Vec::new();
    let mut transform_params = json!({ "node": node });
    let mut transform_fields = Vec::new();
    if let Some(position) = position {
        transform_params["position"] = json!(position);
        transform_fields.push("position".to_string());
    }
    if let Some(rotation) = rotation {
        transform_params["rotation"] = json!(rotation);
        transform_fields.push("rotation".to_string());
    }
    if let Some(scale) = scale {
        transform_params["scale"] = json!(scale);
        transform_fields.push("scale".to_string());
    }
    if !transform_fields.is_empty() {
        calls.push(("scene.settransform", transform_fields, transform_params));
    }

    let mut material_params = json!({ "node": node });
    let mut material_fields = Vec::new();
    if let Some(metallic) = metallic {
        material_params["metallic"] = json!(metallic);
        material_fields.push("material.metallic".to_string());
    }
    if let Some(roughness) = roughness {
        material_params["roughness"] = json!(roughness);
        material_fields.push("material.roughness".to_string());
    }
    if let Some(emissive) = emissive {
        material_params["emissive"] = json!(emissive);
        material_fields.push("material.emissive".to_string());
    }
    if !material_fields.is_empty() {
        calls.push(("scene.setmaterial", material_fields, material_params));
    }

    if calls.is_empty() {
        return Err(ToolError::invalid_params(
            "至少要改一个字段：position / rotation / scale / material.metallic / material.roughness / material.emissive",
        ));
    }

    let mut client = connect_engine(args)?;
    let mut applied = Vec::new();
    for (method, fields, params) in calls {
        let result = call_engine(&mut client, method, params, |value| value)?;
        applied.push(json!({ "method": method, "fields": fields, "result": result }));
    }
    // 回读：模型拿到的是「改完之后」的真实状态，不用自己猜数值是否被夹取。
    let snapshot = call_engine(&mut client, "scene.get", json!({ "node": node }), |value| {
        value
    })?;
    Ok(json!({
        "ok": true,
        "node": node,
        "applied": applied,
        "snapshot": snapshot,
    }))
}

/// 往运行中的场景里加一个物件（复用 `scene.spawn`）。
fn node_spawn(args: &Value) -> Result<Value, ToolError> {
    let object = match args.get("object") {
        Some(value @ Value::Object(_)) => value.clone(),
        None | Some(Value::Null) => {
            return Err(ToolError::invalid_params(
                "缺少 `object`（物件描述对象，至少含 name；可选 mesh / position / rotation / scale / color / material / physics）",
            ))
        }
        Some(other) => {
            return Err(ToolError::invalid_params(format!(
                "object 必须是对象，实际是 {}",
                type_name(other)
            )))
        }
    };
    if object.get("name").and_then(Value::as_str).is_none() {
        return Err(ToolError::invalid_params(
            "object.name 必填（后续 scene_patch / scene_get 都按名字找人）",
        ));
    }
    let mut client = connect_engine(args)?;
    let spawned = call_engine(
        &mut client,
        "scene.spawn",
        json!({ "object": object }),
        |v| v,
    )?;
    Ok(json!({ "ok": true, "method": "scene.spawn", "result": spawned }))
}

/// 把一段 rhai 脚本交给运行中的引擎立即执行（复用 `script.eval`）。
///
/// 这是 P0-3 清单里 `script_write` 的落地形态：写侧工具的价值是「让场景真的变了」，
/// 引擎里让脚本生效的唯一通道就是 `script.eval`——脚本文件落盘由宿主工作区负责，
/// 插件不越权写用户磁盘（那是 `fs.write` 之外的事）。
fn script_write(args: &Value) -> Result<Value, ToolError> {
    let source = require_str(args, "source")?;
    if source.trim().is_empty() {
        return Err(ToolError::invalid_params("source 不能是空白脚本"));
    }
    let mut client = connect_engine(args)?;
    let result = call_engine(
        &mut client,
        "script.eval",
        json!({ "source": source }),
        |v| v,
    )?;
    Ok(json!({
        "ok": true,
        "method": "script.eval",
        "sourceBytes": source.len(),
        "result": result,
    }))
}

/// 把运行中的场景存回磁盘（复用 `scene.save`）。
fn scene_save(args: &Value) -> Result<Value, ToolError> {
    let path = opt_str(args, "path")?;
    if let Some(path) = path {
        if path.trim().is_empty() {
            return Err(ToolError::invalid_params("path 不能是空白字符串"));
        }
    }
    let mut params = json!({});
    if let Some(path) = path {
        params["path"] = json!(path);
    }
    let mut client = connect_engine(args)?;
    let saved = call_engine(&mut client, "scene.save", params, |v| v)?;
    Ok(json!({ "ok": true, "method": "scene.save", "result": saved }))
}

/// 读引擎的「数字画像」（复用 `engine.stats`）。
///
/// 这一层刻意**不做字段裁剪**：契约里的每个数字都是回归/调试的依据，
/// 少透传一个 `shadow_cascades`，调用方就会以为引擎没开级联阴影。
/// 回包体积由宿主 64 KB 上限兜底，而这份快照只有几百字节。
fn engine_stats(args: &Value) -> Result<Value, ToolError> {
    let mut client = connect_engine(args)?;
    let stats = call_engine(&mut client, "engine.stats", json!({}), |v| v)?;
    // 引擎把「没有渲染器」表达成 renderer:false —— 这不是错误，但必须让调用方
    // 一眼看到，否则拿到一堆场景数字会以为画面也在正常出。
    let renderer = stats
        .get("renderer")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(json!({
        "ok": true,
        "method": "engine.stats",
        "renderer": renderer,
        "stats": stats,
    }))
}

/// 让引擎离屏出一张 PNG（复用 `capture.frame`）。
///
/// 路径校验放在插件侧，是为了把「模型常犯的错」在本地拦下：给空串、
/// 给目录、给 `.jpg` 而引擎写的是 PNG 字节——这三种在引擎侧要么被误当成
/// 「默认路径」，要么产出一个名不副实的文件，都不如直接报参数错清楚。
fn capture_frame(args: &Value) -> Result<Value, ToolError> {
    let path = opt_str(args, "path")?;
    if let Some(path) = path {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(ToolError::invalid_params(
                "path 不能是空白字符串；想用默认位置就整个省略 path",
            ));
        }
        if trimmed.ends_with('/') || trimmed.ends_with('\\') {
            return Err(ToolError::invalid_params(format!(
                "path `{trimmed}` 是目录而不是文件，请给出以 .png 结尾的完整文件路径"
            )));
        }
        if !trimmed.to_ascii_lowercase().ends_with(".png") {
            return Err(ToolError::invalid_params(format!(
                "path `{trimmed}` 必须以 .png 结尾：引擎写出的是 PNG 字节，其它后缀只会得到名不副实的文件"
            )));
        }
    }

    let mut params = json!({});
    if let Some(path) = path {
        params["path"] = json!(path.trim());
    }
    let mut client = connect_engine(args)?;
    let captured = call_engine(&mut client, "capture.frame", params, |v| v)?;
    Ok(json!({
        "ok": true,
        "method": "capture.frame",
        "path": captured.get("path").cloned().unwrap_or(Value::Null),
        "width": captured.get("width").cloned().unwrap_or(Value::Null),
        "height": captured.get("height").cloned().unwrap_or(Value::Null),
        "bytes": captured.get("bytes").cloned().unwrap_or(Value::Null),
        "format": captured.get("format").cloned().unwrap_or(Value::Null),
        "result": captured,
    }))
}

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

/// 必填字符串（写侧工具用自己的错误文案，指明缺的是哪个键）。
fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match opt_str(args, key)? {
        Some(text) if !text.trim().is_empty() => Ok(text),
        Some(_) => Err(ToolError::invalid_params(format!("{key} 不能是空白字符串"))),
        None => Err(ToolError::invalid_params(format!("缺少 {key}"))),
    }
}

/// 定长浮点数组参数，如 `position: [x, y, z]`。
fn opt_f32_list<const N: usize>(args: &Value, key: &str) -> Result<Option<[f32; N]>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => f32_list::<N>(value, key).map(Some),
    }
}

/// 定长浮点数组的核心解析；`label` 只用于错误文案（可能是 `material.emissive`）。
fn f32_list<const N: usize>(value: &Value, label: &str) -> Result<[f32; N], ToolError> {
    let items = value.as_array().ok_or_else(|| {
        ToolError::invalid_params(format!(
            "{label} 必须是长度为 {N} 的数值数组，实际是 {}",
            type_name(value)
        ))
    })?;
    if items.len() != N {
        return Err(ToolError::invalid_params(format!(
            "{label} 需要 {N} 个数，实际给了 {}",
            items.len()
        )));
    }
    let mut out = [0.0f32; N];
    for (slot, item) in out.iter_mut().zip(items) {
        let number = item.as_f64().ok_or_else(|| {
            ToolError::invalid_params(format!(
                "{label} 的元素必须是数字，实际是 {}",
                type_name(item)
            ))
        })?;
        if !number.is_finite() {
            return Err(ToolError::invalid_params(format!("{label} 含 NaN/Inf")));
        }
        *slot = number as f32;
    }
    Ok(out)
}

/// 材质补丁里的单值字段（`metallic` / `roughness`）。
fn opt_material_f32(
    material: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f32>, ToolError> {
    match material.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value.as_f64().ok_or_else(|| {
                ToolError::invalid_params(format!(
                    "material.{key} 必须是数字，实际是 {}",
                    type_name(value)
                ))
            })?;
            if !number.is_finite() {
                return Err(ToolError::invalid_params(format!(
                    "material.{key} 不能是 NaN/Inf"
                )));
            }
            Ok(Some(number as f32))
        }
    }
}

/// 材质补丁里的数组字段（`emissive: [r, g, b]`）。
fn opt_material_f32_list<const N: usize>(
    material: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<[f32; N]>, ToolError> {
    match material.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => f32_list::<N>(value, &format!("material.{key}")).map(Some),
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
        .map(|issue| {
            // P0-2：`mesh_stats` 没有场景节点可挂，但锚点仍然要给——
            // 顶点号/索引号/分区号是「去网格里第几个元素看」的唯一线索。
            json!({
                "kind": issue.kind.as_str(),
                "detail": issue.detail,
                "anchors": {
                    "vertex": issue.anchor.vertex,
                    "index": issue.anchor.index,
                    "part": issue.anchor.part,
                },
            })
        })
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
            // 扩展十六：级联配置。模型据此判断「阴影糊」是分辨率不够还是级数太少。
            "cascades": light.shadow.cascade.count,
            "blend": round(light.shadow.cascade.blend),
            "distance": round(light.shadow.cascade.max_distance),
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
        assert!(backends.iter().any(|item| item["name"] == json!("rapier")
            && item["available"] == json!(cfg!(feature = "rapier"))));
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
    fn scene_inspect_reports_postfx_summary() {
        // 没有 postfx 段：整段给 null，模型据此判断「这个场景是直通画面」。
        let report = invoke("scene_inspect", &demo_args(), &mut NoHost).expect("scene_inspect");
        assert!(report["postfx"].is_null(), "无 postfx 段时应为 null");

        let mut args = demo_args();
        args["scene"] = json!(
            r#"{
            "objects": [{ "name": "floor", "mesh": "plane" }],
            "postfx": {
                "exposure": 0.2,
                "tonemap": "aces",
                "bloom": { "enabled": true },
                "ssao": { "enabled": true, "radius": 1.1, "intensity": 1.4, "samples": 12 }
            }
        }"#
        );
        let report = invoke("scene_inspect", &args, &mut NoHost).expect("scene_inspect");
        let postfx = &report["postfx"];
        assert_eq!(postfx["tonemap"], json!("aces"));
        assert_eq!(postfx["bloom"], json!(true));
        // 遮蔽段逐字段回显：半径 / 强度带两位小数，样本数原样。
        assert_eq!(postfx["ssao"]["enabled"], json!(true));
        assert_eq!(postfx["ssao"]["radius"], json!(1.1));
        assert_eq!(postfx["ssao"]["intensity"], json!(1.4));
        assert_eq!(postfx["ssao"]["samples"], json!(12));
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

        let sphere = invoke(
            "mesh_stats",
            &json!({ "shape": "sphere", "segments": 8, "rings": 4 }),
            &mut NoHost,
        )
        .expect("sphere");
        assert_eq!(sphere["shape"], json!("sphere"));
        assert!(sphere["vertices"].as_u64().expect("vertices") > 0);
        // 同参数两次生成必须同指纹
        let again = invoke(
            "mesh_stats",
            &json!({ "shape": "sphere", "segments": 8, "rings": 4 }),
            &mut NoHost,
        )
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
        let report =
            invoke("scene_validate", &json!({ "scene": scene }), &mut NoHost).expect("validate");
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
            assert!(
                error.message.contains("rapier"),
                "错误信息要点明后端名：{}",
                error.message
            );
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
        assert!(
            error.message.contains("--features rapier"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("builtin"),
            "错误里要列出可用后端：{}",
            error.message
        );
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
        let error = invoke(
            "scene_inspect",
            &json!({ "path": "assets/demo.json" }),
            &mut NoHost,
        )
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

    // —— P0-4 反馈工具：参数校验必须在「连引擎之前」就拦下 ——

    #[test]
    fn capture_frame_rejects_non_png_suffix() {
        // 引擎写的是 PNG 字节；后缀给 .jpg 只会产出一个名不副实的文件，
        // 这种错要在插件侧直接报参数错，而不是等用户打开图片才发现。
        for bad in ["a.jpg", "b.png.jpg", "c.jpeg", "d"] {
            let error = invoke("capture_frame", &json!({ "path": bad }), &mut NoHost)
                .expect_err("非 PNG 后缀应被拒绝");
            assert_eq!(error.code, -32602, "path={bad}");
            assert!(
                error.message.contains(".png"),
                "报错应点明要 .png，实际：{}",
                error.message
            );
        }
    }

    #[test]
    fn capture_frame_rejects_blank_and_directory_paths() {
        for bad in ["", "   ", "captures/", "captures\\"] {
            let error = invoke("capture_frame", &json!({ "path": bad }), &mut NoHost)
                .expect_err("空白或目录路径应被拒绝");
            assert_eq!(error.code, -32602, "path={bad:?}");
        }
        // 合法路径不该被参数校验拦下：应当越过校验、死在「连不上引擎」这一步，
        // 以此证明拒绝发生在连接之前而不是「反正连不上都报错」。
        let error = invoke(
            "capture_frame",
            &json!({ "path": "D:/tmp/shot.PNG", "endpoint": { "port": 1 } }),
            &mut NoHost,
        )
        .expect_err("端口 1 上不该有引擎");
        assert_ne!(error.code, -32602, "合法路径不应报参数错：{error:?}");
    }

    #[test]
    fn engine_stats_reports_missing_engine_as_actionable_error() {
        let error = invoke(
            "engine_stats",
            &json!({ "endpoint": { "port": 1 } }),
            &mut NoHost,
        )
        .expect_err("端口 1 上不该有引擎");
        // 错误必须告诉调用方「去找引擎」，而不是含糊的 IO 失败
        assert!(
            error.message.contains("引擎") || error.message.contains("RPC"),
            "报错应指出引擎连接问题，实际：{}",
            error.message
        );
    }
}
