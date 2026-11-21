//! `physics.*` 查询臂（M1-S4-D）：`raycast` / `overlap` / `cast`。
//!
//! 命中不到本组时交给 [`super::character::dispatch_character`]（M2-B01 插在
//! physics 与 nav 之间），不在本文件回错误。
//!
//! 本文件只做**参数形状**校验（三元数组够不够、半径给没给），一行业务判断都不做：
//! 射程缺省、方向归一化、`radius <= 0` 的裁决全在宿主——协议层的 `dir` 归一化
//! 与宿主的归一化若各写一套，迟早出现「这里能用那里用不了」。

use super::*;

pub(super) fn dispatch_physics(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "physics.raycast" => {
            let origin = match require_vec3(params, "origin") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let dir = match require_vec3(params, "dir") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let max_distance = match opt_num(params, "maxDistance") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.physics_raycast(RayQuery {
                origin,
                dir,
                max_distance,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "physics.overlap" => {
            let center = match require_vec3(params, "center") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let Some(radius) = (match opt_num(params, "radius") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少数字参数 `radius`"));
            };
            let limit = match opt_limit(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.physics_overlap(OverlapQuery {
                center,
                radius,
                limit,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "physics.cast" => {
            let origin = match require_vec3(params, "origin") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let dir = match require_vec3(params, "dir") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let max_distance = match opt_num(params, "maxDistance") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let shape = match cast_shape_from(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.physics_cast(CastQuery {
                origin,
                dir,
                max_distance,
                shape,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 无参：装配谁、装哪一层都由宿主按「主场景 + 主世界」定，协议层不搬任何选择。
        // 显式忽略 `params` 是因为客户端习惯性带一个 `{}`，不该因此报参数错误。
        "physics.bind" => match api.physics_bind() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        _ => character::dispatch_character(api, id, params, method, raw),
    }
}

/// 必填三元数组（`origin` / `dir` / `center`）。缺了就是参数错误，不做「默认原点」。
fn require_vec3(params: &Value, key: &str) -> Result<[f32; 3], RpcFailure> {
    match opt_vec3(params, key)? {
        Some(v) => Ok(v),
        None => Err(RpcFailure::invalid_params(format!(
            "缺少三元数组参数 `{key}`"
        ))),
    }
}

/// 解析 `shape`：`{ "kind": "sphere"|"box", "radius"|"half": ... }`。
///
/// `kind` 缺省按 `sphere` 处理（最常见的用法是「把角色当球推过去」）；给了未知名
/// 一律参数错误而不是悄悄退化成球——静默降级会让「为什么我的盒子扫掠像球」变成悬案。
fn cast_shape_from(params: &Value) -> Result<CastShapeSpec, RpcFailure> {
    let shape = match param(params, "shape") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    };
    let Some(shape) = shape else {
        // 没给 shape 段：退回「按 radius 构造球」的简写，radius 也缺就报错。
        let Some(radius) = opt_num(params, "radius")? else {
            return Err(RpcFailure::invalid_params(
                "缺少 `shape`（或简写的 `radius`）参数",
            ));
        };
        return Ok(CastShapeSpec::Sphere { radius });
    };
    if !shape.is_object() {
        return Err(RpcFailure::invalid_params("参数 `shape` 应为对象"));
    }
    let kind = match shape.get("kind") {
        None | Some(Value::Null) => "sphere",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err(RpcFailure::invalid_params("参数 `shape.kind` 应为字符串")),
    };
    // 对象内的数字也用同一套「显式 null 视为没给」的解码器，客户端把没填的字段
    // 序列化成 null 是很常见的写法。
    let num = |key: &str| -> Result<Option<f32>, RpcFailure> {
        match shape.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .map(|n| Some(n as f32))
                .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `shape.{key}` 应为数字"))),
        }
    };
    // 用 if / else if 而不是 match：`cmd_spec_contract` 会把 dispatch 目录下**8 空格
    // 缩进的分支键**当成 RPC 方法名去和命令表对双射，这里若写成 `match kind {`，
    // `"sphere"` / `"box"` 就会被当成两个不存在的方法，测试直接红。
    if kind == "sphere" {
        let Some(radius) = num("radius")? else {
            return Err(RpcFailure::invalid_params("球体扫掠需要 `shape.radius`"));
        };
        return Ok(CastShapeSpec::Sphere { radius });
    }
    if kind == "box" {
        let Some(half) = shape.get("half").filter(|v| !v.is_null()) else {
            return Err(RpcFailure::invalid_params(
                "盒体扫掠需要 `shape.half`（半尺寸三元数组）",
            ));
        };
        let arr = half
            .as_array()
            .ok_or_else(|| RpcFailure::invalid_params("参数 `shape.half` 应为三元数组"))?;
        if arr.len() != 3 {
            return Err(RpcFailure::invalid_params(format!(
                "参数 `shape.half` 需要 3 个数字，收到 {} 个",
                arr.len()
            )));
        }
        let mut out = [0.0f32; 3];
        for (slot, item) in out.iter_mut().zip(arr) {
            *slot = item
                .as_f64()
                .map(|n| n as f32)
                .ok_or_else(|| RpcFailure::invalid_params("参数 `shape.half` 含非数字元素"))?;
        }
        return Ok(CastShapeSpec::Box { half: out });
    }
    Err(RpcFailure::invalid_params(format!(
        "参数 `shape.kind` 不认识：`{kind}`（支持 sphere / box）"
    )))
}
