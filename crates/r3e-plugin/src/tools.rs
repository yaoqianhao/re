//! 工具实现：只读工具是**引擎侧计算的薄壳**，写工具复用引擎既有写 RPC；
//! 全文件没有一条代码写盘、执行外部命令，也没有一条自己算场景 / 网格 / 物理
//! （M1-S1-D 起插件不再链接 `r3e-core` / `r3e-physics`：四项离线计算搬成
//! `scene.snapshot` / `mesh.stats` / `scene.validate` / `physics.simulate`）。
//!
//! 设计约定：
//!
//! * 入参一律走 `opt_*` 取，类型不对就报 `-32602`，绝不静默用默认值糊过去——
//!   模型传错参数时拿到明确错误，比拿到一份「看起来正常」的错结果好得多；
//! * 场景三条来源：内联对象（这里序列化成文本）、内联文本、`path`（宿主解析）；
//!   一条都不给就取引擎的**当前场景**，不逼调用方回传一份可能已经过期的副本；
//! * 只读工具同样需要引擎在线（`endpoint` / `R3E_RPC_ADDR`），连不上报 `-32011`
//!   并给出一条能照着修的提示；
//! * 回包体积与截断由引擎侧负责（`truncated` 字段），插件不再二次截断。

use serde_json::{json, Value};

use crate::host::HostError;
use crate::rpc::{self, Endpoint, RpcClient};

/// 内置网格名（与 `r3e-core::procedural` 的图元一一对应）。
///
/// 这份表只做**参数前置**：形状名写错时不必连引擎就能报错，省一次往返。
const BUILTIN_MESHES: [&str; 4] = ["cube", "sphere", "plane", "cylinder"];

/// 物理后端名。**可用性一律由引擎侧裁决**：插件不链 `r3e-physics`，
/// 也就无从知道宿主编进了哪些后端，猜一个 `available` 等于编。
const PHYSICS_BACKENDS: [&str; 3] = ["null", "builtin", "rapier"];

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

    // 注：没有 `internal`（-32603）。插件已经不在本地解析场景、不在本地跑物理，
    // 「数据本身有问题」这类错全部由引擎裁决并用它自己的错误码回来（`rpc_error` 原样透传）；
    // 插件自己再造一个 -32010 反而会与引擎的 TX_NOT_FOUND 撞码。
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.code)
    }
}

/// 工具需要的宿主能力（只剩工作区信息，读盘已交给引擎解析 `path`）。
pub trait HostBridge {
    fn workspace_info(&mut self) -> Result<Value, HostError>;
}

/// 工具分发。
///
/// 不再收 `HostBridge`：工具层两条路都不消费宿主能力——只读/写工具把入参翻译成
/// 引擎 RPC，`path` 由引擎按「绝对路径 / 工作目录 / 场景目录」自己解析。
/// 保留 `HostBridge` 是为了命令面板（`engine_diagnostics` 要问工作区）。
pub fn invoke(name: &str, args: &Value) -> Result<Value, ToolError> {
    if !args.is_null() && !args.is_object() {
        return Err(ToolError::invalid_params("arguments 必须是 JSON 对象"));
    }
    match name {
        "engine_info" => engine_info(),
        "scene_inspect" => scene_inspect(args),
        "mesh_stats" => mesh_stats(args),
        "scene_validate" => scene_validate(args),
        "physics_simulate" => physics_simulate(args),
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
    let backends: Vec<Value> = PHYSICS_BACKENDS
        .iter()
        .map(|name| json!({ "name": name }))
        .collect();
    let meshes: Vec<Value> = BUILTIN_MESHES
        .iter()
        .map(|name| json!({ "name": name }))
        .collect();
    Ok(json!({
        "engine": {
            "name": "rust3d-engine",
            "version": env!("CARGO_PKG_VERSION"),
            "crates": [
                { "name": "r3e-core", "role": "场景图 / 网格 / 动画 / AIR 校验，无图形依赖（插件不链接：计算走引擎侧 RPC）" },
                { "name": "r3e-render", "role": "渲染后端（插件不链接，headless 用不上）" },
                { "name": "r3e-physics", "role": "物理门面：null / builtin / rapier 三种后端（插件不链接）" },
                { "name": "r3e-asset", "role": "资产导入（FBX/glTF 等，插件不链接）" },
                { "name": "r3e-script", "role": "脚本运行时（插件不链接）" },
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
            // M1-S1-D：只读计算一律在引擎侧做，插件本体只有传输与参数前置。
            "compute": "engine-rpc",
        },
        "physics": {
            "backends": backends,
            // 可用性只有引擎自己知道：写死一个 available 就是编一个假事实。
            "availability": "由引擎侧决定；点未编入的后端时 `physics.simulate` 回参数错误并点名",
        },
        "meshes": meshes,
    }))
}

/// 把工具参数里的「一份场景」归一成引擎 RPC 的两条口子。
///
/// * `path` —— 原样给，宿主按「绝对路径 / 工作目录 / 场景目录」解析；
/// * `scene` 文本 —— 原样给；
/// * `scene` 对象 —— 先序列化成文本：引擎侧只认一种形态，否则那边要同时实现
///   「值」与「文本」两条解析路径，两条迟早给出不同的错误文案；
/// * 都没有 —— 不给，引擎取当前编辑场景。
fn scene_source(args: &Value) -> Result<Value, ToolError> {
    let mut params = Value::Object(serde_json::Map::new());
    if let Some(path) = opt_str(args, "path")? {
        params["path"] = json!(path);
    }
    match args.get("scene") {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) => params["scene"] = json!(text),
        Some(value @ Value::Object(_)) => params["scene"] = json!(value.to_string()),
        Some(other) => {
            return Err(ToolError::invalid_params(format!(
                "scene 必须是场景对象或场景 JSON 文本，实际是 {}",
                type_name(other)
            )))
        }
    }
    Ok(params)
}

fn scene_inspect(args: &Value) -> Result<Value, ToolError> {
    let mut params = scene_source(args)?;
    if let Some(limit) = opt_u32(args, "maxNodes")? {
        params["maxNodes"] = json!(limit);
    }
    let mut client = connect_engine(args)?;
    call_engine(&mut client, "scene.snapshot", params, |report| report)
}

fn mesh_stats(args: &Value) -> Result<Value, ToolError> {
    let mut params = Value::Object(serde_json::Map::new());
    if let Some(shape) = opt_str(args, "shape")? {
        if !BUILTIN_MESHES.contains(&shape) {
            return Err(ToolError::invalid_params(format!(
                "shape `{shape}` 不认识；可选：{}",
                BUILTIN_MESHES.join(" / ")
            )));
        }
        params["shape"] = json!(shape);
    }
    for key in ["segments", "rings", "divisions"] {
        if let Some(value) = opt_u32(args, key)? {
            params[key] = json!(value);
        }
    }
    let mut client = connect_engine(args)?;
    call_engine(&mut client, "mesh.stats", params, |report| report)
}

fn scene_validate(args: &Value) -> Result<Value, ToolError> {
    let params = scene_source(args)?;
    let mut client = connect_engine(args)?;
    call_engine(&mut client, "scene.validate", params, |report| report)
}

fn physics_simulate(args: &Value) -> Result<Value, ToolError> {
    let mut params = scene_source(args)?;
    if let Some(steps) = opt_u32(args, "steps")? {
        params["steps"] = json!(steps);
    }
    if let Some(dt) = opt_f64(args, "dt")? {
        params["dt"] = json!(dt);
    }
    if let Some(max_bodies) = opt_u32(args, "maxBodies")? {
        params["maxBodies"] = json!(max_bodies);
    }
    if let Some(backend) = opt_str(args, "backend")? {
        if !PHYSICS_BACKENDS.contains(&backend) {
            return Err(ToolError::invalid_params(format!(
                "backend `{backend}` 不认识；可选：{}",
                PHYSICS_BACKENDS.join(" / ")
            )));
        }
        params["backend"] = json!(backend);
    }
    if let Some(gravity) = opt_f64_list::<3>(args, "gravity")? {
        params["gravity"] = json!(gravity);
    }
    if let Some(ground) = opt_f64(args, "ground")? {
        params["ground"] = json!(ground);
    }
    let mut client = connect_engine(args)?;
    call_engine(&mut client, "physics.simulate", params, |report| report)
}

// ---------------------------------------------------------------- 写侧工具（P0-3）
//
// 四个写工具全部是「薄壳 + 复用引擎既有写 RPC」，不新造协议：
// 参数校验在插件侧只做「够不够/类型对不对」，值域与语义仍由引擎裁决，
// 这样插件永远与终端的 Ctrl+S / 脚本 API 走同一条路，不会出现两套真相。
//
// 与只读工具的差别只有两点：
// 1) `risk = write`，宿主会走授权流程；
// 2) 改完都回读一次（`snapshot` / `result`），让模型确认「改成了什么样」。
// 连引擎这件事两边一样：只读计算自 M1-S1-D 起也在引擎侧做。

/// 连引擎；连不上时给一条能照着修的提示，而不是光说失败。
fn connect_engine(args: &Value) -> Result<RpcClient, ToolError> {
    let endpoint = Endpoint::from_args(args);
    let addr = endpoint.addr.clone();
    RpcClient::connect(&endpoint).map_err(|err| ToolError {
        // -32011 提供方自定义段：区别于「参数错」（-32602）与引擎给的业务错
        // （如 -32010 TX_NOT_FOUND）——这种失败的正确反应是「去启动引擎」，改参数没用。
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

/// 读一个可选的 f64：只读工具把数值**原样**转给引擎，不经 f32 折损
/// （`-9.81` 转一趟 f32 会变成 `-9.8100004196167`，引擎和测试都会对不上）。
fn opt_f64(args: &Value, key: &str) -> Result<Option<f64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| ToolError::invalid_params(format!("{key} 不是有效数值"))),
        Some(other) => Err(ToolError::invalid_params(format!(
            "{key} 必须是数值，实际是 {}",
            type_name(other)
        ))),
    }
}

fn opt_f64_list<const N: usize>(args: &Value, key: &str) -> Result<Option<[f64; N]>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            if items.len() != N {
                return Err(ToolError::invalid_params(format!(
                    "{key} 必须是 {N} 个数值，实际 {} 个",
                    items.len()
                )));
            }
            let mut out = [0.0f64; N];
            for (slot, item) in out.iter_mut().zip(items) {
                *slot = item.as_f64().ok_or_else(|| {
                    ToolError::invalid_params(format!(
                        "{key} 里必须全是数值，实际是 {}",
                        type_name(item)
                    ))
                })?;
            }
            Ok(Some(out))
        }
        Some(other) => Err(ToolError::invalid_params(format!(
            "{key} 必须是 {N} 个数值的数组，实际是 {}",
            type_name(other)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    struct NoHost;

    impl HostBridge for NoHost {
        fn workspace_info(&mut self) -> Result<Value, HostError> {
            Ok(Value::Null)
        }
    }

    /// 假引擎：监听回环端口，按顺序回 `replies`，返回 (端口, 收到的请求帧)。
    ///
    /// 只读工具现在全是薄壳，「方法名 + 参数形状」就是插件与引擎之间唯一的契约；
    /// 光看返回值验不出来——返回值可以来自本地，也可以来自引擎。
    fn fake_engine(replies: Vec<Value>) -> (u16, std::thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
        let port = listener.local_addr().expect("本地地址").port();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            if let Ok((stream, _)) = listener.accept() {
                let mut writer = stream.try_clone().expect("复制连接");
                let mut reader = BufReader::new(stream);
                for reply in replies {
                    let mut line = String::new();
                    if reader.read_line(&mut line).expect("读请求") == 0 {
                        break;
                    }
                    let request: Value =
                        serde_json::from_str(line.trim_end()).expect("请求是合法 JSON");
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    seen.push(request);
                    let frame = json!({ "jsonrpc": "2.0", "id": id, "result": reply }).to_string();
                    writer.write_all(frame.as_bytes()).expect("写响应");
                    writer.write_all(b"\n").expect("换行");
                    writer.flush().expect("刷新");
                }
            }
            seen
        });
        (port, handle)
    }

    /// 假引擎：只回一帧错误，用来验「引擎错误码原样透传」。
    fn fake_engine_error(code: i64, message: &str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
        let port = listener.local_addr().expect("本地地址").port();
        let message = message.to_string();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut writer = stream.try_clone().expect("复制连接");
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).expect("读请求") > 0 {
                    let request: Value =
                        serde_json::from_str(line.trim_end()).expect("请求是合法 JSON");
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let frame = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": code, "message": message },
                    })
                    .to_string();
                    writer.write_all(frame.as_bytes()).expect("写响应");
                    writer.write_all(b"\n").expect("换行");
                    writer.flush().expect("刷新");
                }
            }
        });
        port
    }

    fn with_engine(port: u16, mut args: Value) -> Value {
        args["endpoint"] = json!({ "addr": format!("127.0.0.1:{port}") });
        args
    }

    #[test]
    fn engine_info_lists_backends_and_tools_without_faking_availability() {
        let info = invoke("engine_info", &json!({})).expect("engine_info");
        assert_eq!(info["engine"]["name"], json!("rust3d-engine"));
        assert_eq!(info["plugin"]["id"], json!(crate::spec::PLUGIN_ID));
        assert_eq!(info["plugin"]["compute"], json!("engine-rpc"));
        let backends = info["physics"]["backends"].as_array().expect("backends");
        assert!(backends.iter().any(|item| item["name"] == json!("builtin")));
        assert!(backends.iter().any(|item| item["name"] == json!("rapier")));
        // 后端可用性只有引擎知道：插件自述里不允许出现凭空断言的 available。
        for backend in backends {
            let available = backend.get("available");
            assert!(
                available.is_none() || available.map(Value::is_null).unwrap_or(false),
                "插件不再链物理 crate，不得凭空断言可用性：{backend}"
            );
        }
        assert_eq!(info["meshes"].as_array().expect("meshes").len(), 4);
    }

    #[test]
    fn scene_inspect_is_a_shell_over_scene_snapshot() {
        let (port, handle) = fake_engine(vec![json!({ "nodeCount": 2, "listedCount": 2 })]);
        let args = with_engine(
            port,
            json!({
                "scene": { "name": "demo", "objects": [ { "name": "floor", "mesh": "builtin:plane" } ] },
                "maxNodes": 1
            }),
        );
        let report = invoke("scene_inspect", &args).expect("scene_inspect");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["method"], json!("scene.snapshot"));
        assert_eq!(seen[0]["params"]["maxNodes"], json!(1));
        // 内联对象必须序列化成**文本**再下行：引擎侧只实现一条解析路径。
        let scene = seen[0]["params"]["scene"]
            .as_str()
            .expect("scene 应以文本形态下行");
        let parsed: Value = serde_json::from_str(scene).expect("序列化后仍是合法 JSON");
        assert_eq!(parsed["name"], json!("demo"));
        // 引擎回什么就回什么，插件不再二次加工。
        assert_eq!(report["nodeCount"], json!(2));
    }

    #[test]
    fn scene_source_defaults_to_current_scene() {
        let (port, handle) = fake_engine(vec![json!({})]);
        let args = with_engine(port, json!({}));
        invoke("scene_validate", &args).expect("scene_validate");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen[0]["method"], json!("scene.validate"));
        // 一条来源都不给时不要凭空塞 `scene: null`：宿主会把它当成「给了空文本」。
        let params = seen[0]["params"].as_object().expect("params 是对象");
        assert!(!params.contains_key("scene"));
        assert!(!params.contains_key("path"));
    }

    #[test]
    fn path_is_forwarded_as_is() {
        let (port, handle) = fake_engine(vec![json!({ "ok": true })]);
        let args = with_engine(port, json!({ "path": "scenes/demo.json" }));
        invoke("scene_validate", &args).expect("scene_validate");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen[0]["params"]["path"], json!("scenes/demo.json"));
    }

    #[test]
    fn mesh_stats_forwards_tessellation_to_engine() {
        let (port, handle) = fake_engine(vec![json!({ "shape": "sphere", "vertices": 42 })]);
        let args = with_engine(
            port,
            json!({ "shape": "sphere", "segments": 8, "rings": 4 }),
        );
        let report = invoke("mesh_stats", &args).expect("mesh_stats");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen[0]["method"], json!("mesh.stats"));
        assert_eq!(seen[0]["params"]["shape"], json!("sphere"));
        assert_eq!(seen[0]["params"]["segments"], json!(8));
        assert_eq!(seen[0]["params"]["rings"], json!(4));
        assert_eq!(report["vertices"], json!(42));
    }

    #[test]
    fn mesh_stats_rejects_unknown_shape_before_connecting() {
        // 端口 1 上不会有引擎：能拿到参数错就证明拒绝发生在连接之前。
        let error = invoke(
            "mesh_stats",
            &json!({ "shape": "teapot", "endpoint": { "port": 1 } }),
        )
        .expect_err("未知图元必须报错");
        assert_eq!(error.code, -32602);
        assert!(
            error.message.contains("cube"),
            "报错要列出可选形状：{}",
            error.message
        );
    }

    #[test]
    fn physics_simulate_forwards_simulation_params() {
        let (port, handle) = fake_engine(vec![json!({ "ok": true, "bodyCount": 2 })]);
        let args = with_engine(
            port,
            json!({
                "scene": { "name": "demo", "objects": [] },
                "steps": 12,
                "dt": 0.02,
                "maxBodies": 3,
                "backend": "builtin",
                "gravity": [0.0, -9.81, 0.0],
                "ground": -0.5
            }),
        );
        let report = invoke("physics_simulate", &args).expect("physics_simulate");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen[0]["method"], json!("physics.simulate"));
        let params = &seen[0]["params"];
        assert_eq!(params["steps"], json!(12));
        assert_eq!(params["maxBodies"], json!(3));
        assert_eq!(params["backend"], json!("builtin"));
        assert_eq!(params["gravity"], json!([0.0, -9.81, 0.0]));
        let dt = params["dt"].as_f64().expect("dt");
        assert!((dt - 0.02).abs() < 1e-6, "dt 应为 0.02，实际 {dt}");
        let ground = params["ground"].as_f64().expect("ground");
        assert!(
            (ground + 0.5).abs() < 1e-6,
            "ground 应为 -0.5，实际 {ground}"
        );
        assert!(params["scene"].is_string());
        assert_eq!(report["bodyCount"], json!(2));
    }

    #[test]
    fn physics_simulate_rejects_unknown_backend_before_connecting() {
        let error = invoke(
            "physics_simulate",
            &json!({ "scene": "{}", "backend": "bullet", "endpoint": { "port": 1 } }),
        )
        .expect_err("未知后端必须报错");
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("builtin"), "{}", error.message);
    }

    #[test]
    fn read_only_tools_report_missing_engine_as_actionable_error() {
        for tool in [
            "scene_inspect",
            "mesh_stats",
            "scene_validate",
            "physics_simulate",
        ] {
            let error = invoke(tool, &json!({ "endpoint": { "port": 1 } }))
                .expect_err("端口 1 上不该有引擎");
            assert_eq!(error.code, -32011, "{tool}");
            assert!(
                error.message.contains("引擎"),
                "{tool} 的报错要点明引擎：{}",
                error.message
            );
        }
    }

    #[test]
    fn engine_rejection_keeps_its_code_and_names_the_method() {
        let port = fake_engine_error(-32004, "场景里没有节点 `hero`");
        let args = with_engine(port, json!({ "scene": "{}" }));
        let error = invoke("scene_inspect", &args).expect_err("引擎拒绝");
        assert_eq!(error.code, -32004, "引擎错误码必须原样透传");
        assert!(
            error.message.contains("scene.snapshot"),
            "报错要点明是哪条方法：{}",
            error.message
        );
    }

    #[test]
    fn wrong_argument_type_is_rejected() {
        let error = invoke("scene_inspect", &json!({ "scene": {}, "maxNodes": "many" }))
            .expect_err("类型错");
        assert_eq!(error.code, -32602);
    }

    #[test]
    fn scene_must_be_object_or_text() {
        let error = invoke("scene_validate", &json!({ "scene": 42 })).expect_err("scene 类型不对");
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("scene"), "{}", error.message);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let error = invoke("nope", &json!({})).expect_err("未知工具");
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
            let error =
                invoke("capture_frame", &json!({ "path": bad })).expect_err("非 PNG 后缀应被拒绝");
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
            let error = invoke("capture_frame", &json!({ "path": bad }))
                .expect_err("空白或目录路径应被拒绝");
            assert_eq!(error.code, -32602, "path={bad:?}");
        }
        // 合法路径不该被参数校验拦下：应当越过校验、死在「连不上引擎」这一步，
        // 以此证明拒绝发生在连接之前而不是「反正连不上都报错」。
        let error = invoke(
            "capture_frame",
            &json!({ "path": "D:/tmp/shot.PNG", "endpoint": { "port": 1 } }),
        )
        .expect_err("端口 1 上不该有引擎");
        assert_ne!(error.code, -32602, "合法路径不应报参数错：{error:?}");
    }

    #[test]
    fn engine_stats_reports_missing_engine_as_actionable_error() {
        let error = invoke("engine_stats", &json!({ "endpoint": { "port": 1 } }))
            .expect_err("端口 1 上不该有引擎");
        // 错误必须告诉调用方「去找引擎」，而不是含糊的 IO 失败
        assert!(
            error.message.contains("引擎") || error.message.contains("RPC"),
            "报错应指出引擎连接问题，实际：{}",
            error.message
        );
    }

    #[test]
    fn scene_validate_forwards_inline_text_without_rewriting_it() {
        // 已是文本就原样下行：插件不再 `parse` 一遍，否则「文本」与「对象」两条路
        // 会在引擎侧给出两套不同的错误文案。
        let text = r#"{"name":"lv1","objects":[]}"#;
        let (port, handle) = fake_engine(vec![json!({ "ok": true })]);
        let args = with_engine(port, json!({ "scene": text }));
        invoke("scene_validate", &args).expect("scene_validate");
        let seen = handle.join().expect("假引擎线程");
        assert_eq!(seen[0]["method"], json!("scene.validate"));
        assert_eq!(seen[0]["params"]["scene"], json!(text));
    }

    #[test]
    fn scene_source_rejects_non_text_and_non_object_scene() {
        // 端口 1 上不会有引擎：拿到参数错就证明拒绝发生在连接之前。
        for bad in [json!(3), json!([1, 2]), json!(true)] {
            let error = invoke(
                "scene_inspect",
                &json!({ "scene": bad, "endpoint": { "port": 1 } }),
            )
            .expect_err("scene 只认场景对象或场景 JSON 文本");
            assert_eq!(error.code, -32602, "scene={bad}");
            assert!(
                error.message.contains("scene"),
                "报错要点名 scene，实际：{}",
                error.message
            );
        }
    }

    #[test]
    fn scene_inspect_omits_max_nodes_when_absent() {
        // 没给就是没给：插件不替引擎补默认值，否则两边的缺省值迟早会分叉。
        let (port, handle) = fake_engine(vec![json!({})]);
        let args = with_engine(port, json!({ "scene": "{}" }));
        invoke("scene_inspect", &args).expect("scene_inspect");
        let seen = handle.join().expect("假引擎线程");
        let params = seen[0]["params"].as_object().expect("params 是对象");
        assert!(!params.contains_key("maxNodes"), "{params:?}");
    }

    #[test]
    fn mesh_stats_omits_absent_tessellation_params() {
        let (port, handle) = fake_engine(vec![json!({})]);
        let args = with_engine(port, json!({ "shape": "sphere" }));
        invoke("mesh_stats", &args).expect("mesh_stats");
        let seen = handle.join().expect("假引擎线程");
        let params = seen[0]["params"].as_object().expect("params 是对象");
        for key in ["segments", "rings", "divisions"] {
            assert!(!params.contains_key(key), "不该补 `{key}`：{params:?}");
        }
    }

    #[test]
    fn physics_simulate_keeps_engine_error_code_and_names_method() {
        let port = fake_engine_error(-32004, "场景里没有节点 `hero`");
        let args = with_engine(port, json!({ "scene": "{}" }));
        let error = invoke("physics_simulate", &args).expect_err("引擎拒绝");
        assert_eq!(error.code, -32004, "引擎错误码必须原样透传");
        assert!(
            error.message.contains("physics.simulate"),
            "报错要点名方法，实际：{}",
            error.message
        );
    }

    #[test]
    fn gravity_must_be_exactly_three_numbers() {
        let error = invoke(
            "physics_simulate",
            &json!({ "scene": "{}", "gravity": [0.0, -9.81], "endpoint": { "port": 1 } }),
        )
        .expect_err("gravity 少一个分量必须报错");
        assert_eq!(error.code, -32602);
        assert!(
            error.message.contains("gravity") && error.message.contains('3'),
            "报错要点明需要 3 个分量，实际：{}",
            error.message
        );
    }
}
