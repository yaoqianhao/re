//! 四条只读检视能力（M1-S1-D）：`scene.snapshot` / `mesh.stats` /
//! `scene.validate` / `physics.simulate` 的计算层。
//!
//! # 为什么在宿主里
//!
//! 这四件事原来由 `r3e-plugin` 在**插件进程内**直连 `r3e-core` / `r3e-physics` 算完
//! 再回给模型。功能没错，但代价是插件二进制把 core 与整个物理后端（含 `rapier`
//! feature）一起编进去，`dep_rules.json` 里 `plugin→core` / `plugin→physics` 两条债
//! 永远还不掉。把计算搬到引擎侧、插件只剩「发 RPC + 转结果」的薄壳，债才消得掉。
//!
//! # 口径（与插件原来逐字段一致，插件薄壳直接转发这里的回包）
//!
//! * **只读**：四条都不写 `Document`——`physics.simulate` 走的是**副本**上的临时世界，
//!   步进完只回位移，场景 `StateHash` 前后一致；
//! * **算不了就说算不了**：外部资产网格没有 AIR 结论，一律进 `unvalidated` 并附定位，
//!   **绝不**给一个「0 问题」的假绿灯；
//! * **后端点名要诚实**：点了没编进宿主二进制的后端要报参数错误，不能静默退回内置
//!   后端——否则「用 rapier 复现」会变成「其实用的是内置后端」，结论全错；
//! * **截断要明说**：回包控制在宿主单次结果上限内，节点 / 问题 / 刚体清单都截断并带
//!   `truncated`，同时保留真实计数。

mod locate;

use r3e_core::air;
use r3e_core::procedural::builtin_mesh;
use r3e_core::{BuiltinMesh, MeshData, NodeId, Scene, Vec3};
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};
use serde_json::{json, Value};

/// 单个回包里最多列多少个节点（与插件原来的 `MAX_LISTED_NODES` 同值）。
const MAX_LISTED_NODES: usize = 100;
/// 单个节点最多列多少条 AIR 问题（与插件原来的 `MAX_LISTED_ISSUES` 同值）。
const MAX_LISTED_ISSUES: usize = 20;
/// `scene.snapshot` 的节点上限硬顶（插件原来 `clamp(1, 500)`）。
const MAX_SNAPSHOT_NODES: u32 = 500;
/// `scene.snapshot` 缺省列多少节点（插件原来 50）。
const DEFAULT_SNAPSHOT_NODES: u32 = 50;

/// 检视层的错误：只区分「调用姿势错」与「数据本身有问题」两类。
///
/// 与插件原来的 `ToolError` 一一对应，`api::inspect` 负责翻成 [`r3e_editor::RpcFailure`]：
/// 前者 `-32602`（改参数），后者 `-32010`（改场景）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectError {
    /// 入参不合法（形状不认识、区间的物理含义不成立）。
    InvalidParams(String),
    /// 入参合法但内容解析不了（物理配置被后端拒绝）。
    InvalidInput(String),
}

impl InspectError {
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidParams(message) | Self::InvalidInput(message) => message,
        }
    }
}

impl std::fmt::Display for InspectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

// ---------------------------------------------------------------- scene.snapshot

/// 场景结构快照：节点表 + 相机 / 灯光 / 后处理 / 物理摘要。
///
/// `max_nodes` 只截**明细**，`nodeCount` 永远是真实总数。
pub fn snapshot(scene: &Scene, max_nodes: Option<u32>) -> Value {
    let limit = max_nodes
        .unwrap_or(DEFAULT_SNAPSHOT_NODES)
        .clamp(1, MAX_SNAPSHOT_NODES) as usize;

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

    json!({
        "name": scene.name,
        "nodeCount": scene.len(),
        "listedCount": nodes.len(),
        "truncated": scene.len() > nodes.len(),
        "assetMeshCount": asset_meshes,
        "camera": camera_json(&scene.camera),
        "light": light_json(&scene.light),
        // 后处理整段：这里只摘要字段，不复刻 core 的收敛规则——
        // 报的是场景里写了什么，实际生效值由引擎的 sanitized 决定。
        "postfx": scene.postfx.map(|fx| json!({
            "exposure": round(fx.exposure),
            "tonemap": fx.tonemap.name(),
            "fxaa": fx.fxaa,
            "bloom": fx.bloom.enabled,
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
    })
}

// ---------------------------------------------------------------- mesh.stats

/// 程序化网格的统计 + AIR 校验。
///
/// 只认内置网格：外部资产要走资产管线，这条口子不为它开（与插件原来一致）。
pub fn mesh_stats(
    shape: Option<&str>,
    segments: Option<u32>,
    rings: Option<u32>,
    divisions: Option<u32>,
) -> Result<Value, InspectError> {
    let shape_name = shape.unwrap_or("cube");
    let kind = BuiltinMesh::parse(shape_name).ok_or_else(|| {
        InspectError::InvalidParams(format!(
            "shape `{shape_name}` 不认识；可选：cube / sphere / plane / cylinder"
        ))
    })?;
    let segments = segments.unwrap_or(32).clamp(3, 256);
    let rings = rings.unwrap_or(16).clamp(2, 256);
    let divisions = divisions.unwrap_or(1).clamp(1, 128);

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

// ---------------------------------------------------------------- scene.validate

/// 逐节点网格体检（AIR），每条问题带结构化定位。
///
/// 与 `scene.verify` 的分工必须守住：这条只查**网格本身**，`scene.verify` 查
/// **场景跑起来会不会出问题**（静态/物理/导航三层）。两条别互相顶替。
pub fn validate(scene: &Scene) -> Value {
    let mut reports = Vec::new();
    let mut unvalidated = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_issues = 0usize;

    // 每条问题都带 `location`（节点 id/名字、场景文件 `objects` 下标、字段路径、
    // 资源来源、网格锚点、材质上下文），调用方拿到就能直接定位要改哪一处。
    // `objects` 顺序与 `scene.iter()` 一致，因为 `to_scene_file` 就是按同一个
    // `order` 表导出的。
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
                            .map(|issue| locate::issue_json(scene, node, object_index, issue, Some(&mesh)))
                            .collect::<Vec<Value>>(),
                    },
                })
            }
            None => {
                // 资产网格要走 `r3e-asset` 导入，本命令不带资产管线——如实标注 skipped，
                // 而不是给一个「0 问题」的假绿灯；同时在 `unvalidated` 里留一条带定位
                // 的账（不并进 `issueCount`，否则「有资产 = 必失败」会让 ok 失去意义）。
                unvalidated.push(locate::skipped_issue_json(scene, node, object_index));
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

    // 重名不用在这里检测：`Scene::from_file` 构建阶段就会因重名直接报错。
    if scene.is_empty() {
        warnings.push("场景没有任何节点".to_string());
    }
    if reports
        .iter()
        .any(|report| report["air"]["skipped"] == json!(true))
    {
        warnings.push(
            "部分节点使用外部资产网格，AIR 校验需要资产管线（r3e-asset），本条命令跳过".to_string(),
        );
    }

    json!({
        "ok": total_issues == 0,
        "name": scene.name,
        "nodeCount": scene.len(),
        "listedCount": reports.len(),
        "truncated": scene.len() > reports.len(),
        "issueCount": total_issues,
        // 没校验成的节点也给出定位：调用方应当知道「这里没结论」，而不是以为通过。
        "unvalidatedCount": unvalidated.len(),
        "unvalidated": unvalidated,
        "warnings": warnings,
        "nodes": reports,
    })
}

// ---------------------------------------------------------------- physics.simulate

/// `physics.simulate` 的可选项（缺省值在这里定，协议层只管形状）。
#[derive(Debug, Clone, Default)]
pub struct SimulateOptions<'a> {
    pub steps: Option<u32>,
    pub dt: Option<f32>,
    pub max_bodies: Option<u32>,
    pub backend: Option<&'a str>,
    pub gravity: Option<[f32; 3]>,
    pub ground: Option<f32>,
}

/// 把场景丢进**临时**物理世界跑一段，回报位移与事件。
///
/// 入参 `scene` 是调用方给的**副本**（宿主不把真文档借出来）：步进会写节点的变换，
/// 但在副本上写，`Document` 的 `StateHash` 不受影响。
pub fn simulate(mut scene: Scene, options: SimulateOptions<'_>) -> Result<Value, InspectError> {
    let steps = options.steps.unwrap_or(120).clamp(1, 2000);
    let dt = options.dt.unwrap_or(1.0 / 60.0);
    if !(0.001..=0.1).contains(&dt) {
        return Err(InspectError::InvalidParams(format!(
            "dt={dt} 超出范围 0.001..=0.1"
        )));
    }
    let max_bodies = options.max_bodies.unwrap_or(20).clamp(1, 64) as usize;

    // 模拟场景就是要算：显式把物理打开（场景写 enabled:false 时也照跑），
    // 但后端选择尊重调用方 > 场景 > 引擎默认的优先级。
    let mut config = PhysicsConfig::from_scene(&scene.physics).with_enabled(true);
    if let Some(name) = options.backend {
        let kind = PhysicsKind::parse(name).ok_or_else(|| {
            InspectError::InvalidParams(format!(
                "backend `{name}` 不认识；可选：null / builtin / rapier"
            ))
        })?;
        if !kind.is_available() {
            let available: Vec<&str> = PhysicsKind::ALL
                .iter()
                .filter(|kind| kind.is_available())
                .map(|kind| kind.name())
                .collect();
            return Err(InspectError::InvalidParams(format!(
                "物理后端 `{name}` 未编进宿主二进制（当前可用：{}）；需要 Rapier 请用 \
                 `--features rapier` 重新构建宿主",
                available.join(" / ")
            )));
        }
        config = config.with_kind(kind);
    }
    if let Some(gravity) = options.gravity {
        config.gravity = Some(Vec3::from_array(gravity));
    }
    if let Some(ground) = options.ground {
        config.ground = Some(ground);
    }

    let mut physics = Physics::new();
    physics
        .apply_config(config)
        .map_err(|err| InspectError::InvalidInput(format!("物理配置被后端拒绝：{err}")))?;
    // 后端名以后端自报为准：调用方没指定时也能如实说明「实际用的是哪个」。
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

// ---------------------------------------------------------------- 辅助

fn issue_list(issues: &[air::AirIssue]) -> Vec<Value> {
    issues
        .iter()
        .take(MAX_LISTED_ISSUES)
        .map(|issue| {
            // `mesh.stats` 没有场景节点可挂，但锚点仍然要给——
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
/// （引擎不需要把它们写回场景文件），所以这里手写摘要，而不是硬要上游加 `Serialize`
/// ——检视层不该为了自己的输出格式去改核心 crate 的公开契约。
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

/// 保留 4 位小数：回包给人/模型看，尾数过长的浮点纯属噪声（与插件原口径一致）。
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
