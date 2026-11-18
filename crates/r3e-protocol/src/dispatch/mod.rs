//! 请求派发：`Request` → [`EngineApi`] → `Response`。
//!
//! ```text
//! dispatch()                      元命令 ping / engine.* / script.* / capture.*
//!   └─ query::dispatch_query      query.*（M1-S2-B 语义查询，只读）
//!        └─ tx::dispatch_tx       tx.*（M1-S1 事务协议）
//!             └─ scene::dispatch_scene scene.* / anim.*
//!                  └─ media::dispatch_media ui.* / light.* / audio.*
//!                       └─ physics::dispatch_physics physics.*（M1-S4-D，只读）
//!                            └─ nav::dispatch_nav nav.*（M1-S4-D）
//!                                 └─ 未知方法 → METHOD_NOT_FOUND
//! ```
//!
//! 参数解码辅助（`param` / `opt_f32*` / `with_node` …）也在这里，子模块通过
//! `use super::*` 取用。

use serde_json::{json, Value};

use crate::api::EngineApi;
use crate::material::MaterialUpsert;
use crate::msg::{Request, Response, RpcFailure};
use crate::nav::{NavBakeOptions, NavPathQuery, NavReachQuery};
use crate::patch::*;
use crate::probe::{CastQuery, CastShapeSpec, OverlapQuery, RayQuery};
use crate::{INTERNAL_ERROR, METHOD_NOT_FOUND, PROTOCOL_VERSION};

mod media;
mod nav;
mod physics;
mod query;
mod scene;
mod tx;

/// 方法名归一化：忽略大小写与下划线，`scene.setTransform` 与 `scene.set_transform` 等价。
fn canonical(method: &str) -> String {
    method
        .chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn param<'a>(params: &'a Value, key: &str) -> Option<&'a Value> {
    params.get(key)
}

fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcFailure> {
    param(params, key)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcFailure::invalid_params(format!("缺少字符串参数 `{key}`")))
}

/// 取出可选数字参数；缺省得到 `None`，给了但不是数字则报错。
fn opt_f32(params: &Value, key: &str) -> Result<Option<f32>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    value
        .as_f64()
        .map(|v| Some(v as f32))
        .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为数字")))
}

/// 取出可选字符串参数；缺省得到 `None`（含显式 `null`），给了但不是字符串则报错。
fn opt_str<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为字符串"
        ))),
    }
}

/// 取出可选布尔参数；缺省得到 `None`，给了但不是布尔则报错。
/// 取出可选整数参数；缺省得到 `None`，给了但不是整数则报错。
fn opt_i32(params: &Value, key: &str) -> Result<Option<i32>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let n = value.as_i64().ok_or_else(|| {
        RpcFailure::invalid_params(format!("参数 `{key}` 应为整数，收到 {value}"))
    })?;
    i32::try_from(n)
        .map(Some)
        .map_err(|_| RpcFailure::invalid_params(format!("参数 `{key}` 超出 i32 范围")))
}

/// 取出可选字符串数组参数；缺省（含显式 `null`）得到 `None`，给了但不是字符串数组则报错。
///
/// 元素个数**不在这里校验**：`faces` 必须恰好 6 条这件事由宿主的
/// `EnvironmentMapSettings::sanitized` 判（长度不对时整段作废、退回解析 IBL），
/// 本层多拦一道只会让「想用空数组清空来源」这种合法写法被提前拒掉。
fn opt_str_list(params: &Value, key: &str) -> Result<Option<Vec<String>>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => {
                        return Err(RpcFailure::invalid_params(format!(
                            "参数 `{key}` 应为字符串数组，其中含非字符串元素"
                        )))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为字符串数组"
        ))),
    }
}

fn opt_bool(params: &Value, key: &str) -> Result<Option<bool>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为布尔值")))
}

fn opt_f32_array<const N: usize>(
    params: &Value,
    key: &str,
) -> Result<Option<[f32; N]>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    let arr = value.as_array().ok_or_else(|| {
        RpcFailure::invalid_params(format!("参数 `{key}` 应为 {N} 个数字组成的数组"))
    })?;
    if arr.len() != N {
        return Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 需要 {N} 个数字，收到 {} 个",
            arr.len()
        )));
    }
    let mut out = [0.0f32; N];
    for (slot, item) in out.iter_mut().zip(arr) {
        *slot = item
            .as_f64()
            .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 含非数字元素")))?
            as f32;
    }
    Ok(Some(out))
}

/// 把一条请求派发到 [`EngineApi`]，永不 panic（未知方法走 `METHOD_NOT_FOUND`）。
///
/// 分组只是为了让单文件留在 800 行内：`ping` / `engine.*` / `script.*` / `capture.*`
/// 在本文件，`query.*` 在 [`query`]，`tx.*` 在 [`tx`]，`scene.*` 与 `anim.*` 在
/// [`scene`]，`ui.*` / `light.*` / `audio.*` 在 [`media`]。组间靠兜底分支串联：命中不到
/// 本组就顺序交给下一组，最后一组回 `METHOD_NOT_FOUND`。因此「某条臂放在哪个文件」
/// 不影响任何命令的行为。
pub fn dispatch(api: &mut dyn EngineApi, req: &Request) -> Response {
    let id = req.id.clone();
    let params = &req.params;
    let method = canonical(&req.method);
    match method.as_str() {
        "ping" => Response::ok(
            id,
            json!({ "pong": true, "protocol": PROTOCOL_VERSION, "version": env!("CARGO_PKG_VERSION") }),
        ),
        "engine.info" => match serde_json::to_value(api.info()) {
            Ok(value) => Response::ok(id, value),
            Err(e) => Response::fail(id, RpcFailure::new(INTERNAL_ERROR, e.to_string())),
        },
        // P0-4 只读反馈：统计采样不改场景，直接透传宿主的快照。
        // 别名 `enginestats`：`canonical` 会吃掉下划线，`engine_stats` 落到这里正是这个名字，
        // 让插件工具名与 RPC 方法名保持一致，免得模型两边记两套写法。
        "engine.stats" | "enginestats" => match api.engine_stats() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        // P0-4 只读反馈：截图路径可省略（宿主决定默认落盘位置），空串一律拒绝，
        // 免得宿主把「没给」当成「写到空路径」。
        "capture.frame" | "captureframe" => {
            let path = match param(params, "path") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `path` 应为非空字符串（可省略）"),
                    )
                }
            };
            match api.capture_frame(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "script.eval" => {
            let source =
                match require_str(params, "source").or_else(|_| require_str(params, "code")) {
                    Ok(s) => s,
                    Err(f) => return Response::fail(id, f),
                };
            match api.script_eval(source) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "script.reload" => match api.script_reload() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        _ => query::dispatch_query(api, id, params, &method, &req.method),
    }
}

/// 取出 `node` 参数并交给闭包，统一参数缺失的错误。
fn with_node<T>(
    params: &Value,
    f: impl FnOnce(&str) -> Result<T, RpcFailure>,
) -> Result<T, RpcFailure> {
    let node = require_str(params, "node")?;
    if node.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `node` 不能为空"));
    }
    f(node)
}

/// 从 RPC 参数解析局部光源补丁（全部字段可选，没给的留给宿主沿用原值）。
fn light_patch_from(params: &Value) -> Result<LightPatch, RpcFailure> {
    let key = match require_str(params, "key") {
        Ok(k) if !k.trim().is_empty() => k.to_string(),
        Ok(_) => return Err(RpcFailure::invalid_params("参数 `key` 不能为空")),
        Err(f) => return Err(f),
    };
    Ok(LightPatch {
        key,
        kind: opt_str(params, "kind")?.map(str::to_string),
        pos: opt_vec3(params, "pos")?,
        dir: opt_vec3(params, "dir")?,
        color: opt_vec3(params, "color")?,
        intensity: opt_num(params, "intensity")?,
        range: opt_num(params, "range")?,
        inner_deg: opt_num(params, "inner_deg")?,
        outer_deg: opt_num(params, "outer_deg")?,
        cast_shadow: opt_bool(params, "cast_shadow")?,
        shadow_bias: opt_num(params, "shadow_bias")?,
    })
}

/// 从 RPC 参数解析空间化声源补丁（`key` 必填，其余可选）。
fn audio_spatial_patch_from(params: &Value) -> Result<AudioSpatialPatch, RpcFailure> {
    let key = match require_str(params, "key") {
        Ok(k) if !k.trim().is_empty() => k.to_string(),
        Ok(_) => return Err(RpcFailure::invalid_params("参数 `key` 不能为空")),
        Err(f) => return Err(f),
    };
    Ok(AudioSpatialPatch {
        key,
        position: opt_vec3(params, "position")?,
        node: opt_str(params, "node")?.map(str::to_string),
        min_distance: opt_num(params, "minDistance")?,
        max_distance: opt_num(params, "maxDistance")?,
        model: opt_str(params, "model")?.map(str::to_string),
        doppler: opt_num(params, "doppler")?,
    })
}

/// 从 RPC 参数解析听者补丁：两个字段都没有 = `None`（纯只读）。
fn listener_patch_from(params: &Value) -> Result<Option<ListenerPatch>, RpcFailure> {
    let patch = ListenerPatch {
        position: opt_vec3(params, "position")?,
        forward: opt_vec3(params, "forward")?,
    };
    Ok(if patch.is_read_only() {
        None
    } else {
        Some(patch)
    })
}

/// 从 RPC 参数解析阴影补丁：一个字段都没有 = `None`（纯只读）。
fn shadow_patch_from(params: &Value) -> Result<Option<ShadowPatch>, RpcFailure> {
    let patch = ShadowPatch {
        enabled: match param(params, "enabled") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(RpcFailure::invalid_params("参数 `enabled` 应为布尔")),
        },
        size: opt_uint(params, "size")?,
        strength: opt_num(params, "strength")?,
        ortho_size: opt_num(params, "orthoSize")?,
        bias: opt_num(params, "bias")?,
        cascades: opt_uint(params, "cascades")?,
        blend: opt_num(params, "blend")?,
        distance: opt_num(params, "distance")?,
    };
    Ok(if patch.is_read_only() {
        None
    } else {
        Some(patch)
    })
}

/// 可选无符号整数字段（`size` / `cascades`）；显式 `null` 当作没给。
fn opt_uint(params: &Value, key: &str) -> Result<Option<u32>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .map(Some)
            .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为非负整数"))),
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为非负整数"
        ))),
    }
}

/// 显式 `null` 等同于「没给」——不少 JSON 客户端会把没填的字段序列化成 `null`，
/// 对这种输入报「类型错误」纯属刁难。真正的类型错误（字符串 / 布尔 / 短数组）照旧拒绝。
fn opt_num(params: &Value, key: &str) -> Result<Option<f32>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => opt_f32(params, key),
    }
}

/// 同 [`opt_num`]，用于三元数组字段。
fn opt_vec3(params: &Value, key: &str) -> Result<Option<[f32; 3]>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => opt_f32_array::<3>(params, key),
    }
}

/// 把 `limit` 参数解析成 `Option<usize>`（缺省 / `null` 表示不截断）。
fn opt_limit(params: &Value) -> Result<Option<usize>, RpcFailure> {
    match param(params, "limit") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n as usize)),
            None => Err(RpcFailure::invalid_params("参数 `limit` 应为非负整数")),
        },
    }
}
