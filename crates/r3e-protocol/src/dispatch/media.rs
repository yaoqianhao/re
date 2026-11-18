//! `ui.*` / `light.*` / `audio.*` 臂。
//!
//! 命中不到本组时交给 [`super::physics::dispatch_physics`]，不在本文件回错误。

use super::*;

pub(super) fn dispatch_media(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        // UI 契约（扩展十四）：位置 / 尺寸用字符串单位透传，名字合法性由宿主判定。
        "ui.list" => {
            let limit = match param(params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.ui_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.set" => {
            let key = match require_str(params, "key") {
                Ok(k) if !k.trim().is_empty() => k.to_string(),
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            let patch = match (
                opt_str(params, "kind"),
                opt_str(params, "anchor"),
                opt_str(params, "x"),
                opt_str(params, "y"),
                opt_str(params, "width"),
                opt_str(params, "height"),
                opt_f32_array::<4>(params, "color"),
                opt_i32(params, "layer"),
                opt_bool(params, "visible"),
                opt_f32(params, "border_width"),
                opt_str(params, "image"),
            ) {
                (
                    Ok(kind),
                    Ok(anchor),
                    Ok(x),
                    Ok(y),
                    Ok(width),
                    Ok(height),
                    Ok(color),
                    Ok(layer),
                    Ok(visible),
                    Ok(border_width),
                    Ok(image),
                ) => UiPatch {
                    key: key.clone(),
                    kind: kind.map(str::to_string),
                    anchor: anchor.map(str::to_string),
                    x: x.map(str::to_string),
                    y: y.map(str::to_string),
                    width: width.map(str::to_string),
                    height: height.map(str::to_string),
                    color,
                    layer,
                    visible,
                    border_width,
                    image: image.map(str::to_string),
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            match api.ui_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.remove" => {
            let key = match require_str(params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.ui_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.state" => match api.ui_state() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        // 局部光源契约（扩展十五）：字段可分别给，一个都不给视为无意义调用；
        // kind 的合法性、数值收敛全部由宿主完成，本层只做类型与空值校验。
        "light.list" => {
            let limit = match opt_limit(params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.light_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.set" => {
            let patch = match light_patch_from(params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            if patch.is_key_only() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "除 `key` 外至少要给一个字段（否则这次调用没有意义）",
                    ),
                );
            }
            match api.light_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.remove" => {
            let key = match require_str(params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.light_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.clear" => match api.light_clear() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "light.setenabled" => {
            let enabled = match param(params, "enabled") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `enabled` 应为布尔（可省略表示只读）"),
                    )
                }
            };
            match api.light_set_enabled(enabled) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 3D 空间化音频契约（§12 第十项）：与 light.* 同构——传输层只校验
        // 类型与空值，距离收敛 / 模型解析 / 上限拒绝全在宿主侧。
        "audio.spatial.list" => {
            let limit = match opt_limit(params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.set" => {
            let patch = match audio_spatial_patch_from(params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            if patch.is_key_only() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "除 `key` 外至少要给一个字段（否则这次调用没有意义）",
                    ),
                );
            }
            match api.audio_spatial_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.remove" => {
            let key = match require_str(params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.clear" => match api.audio_spatial_clear() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "audio.spatial.listener" => {
            let patch = match listener_patch_from(params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_listener(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.setenabled" => {
            let enabled = match param(params, "enabled") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `enabled` 应为布尔（可省略表示只读）"),
                    )
                }
            };
            match api.audio_spatial_set_enabled(enabled) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        _ => physics::dispatch_physics(api, id, params, method, raw),
    }
}
