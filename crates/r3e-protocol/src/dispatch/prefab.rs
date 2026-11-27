//! `prefab.*` 两条（M2-E04）：`spawn` / `info`。
//!
//! 本组在兜底链里排在 `nav` 之后、`inspect` 之前
//! （`query` → `tx` → `scene` → `media` → `physics` → `terrain` → `nav` → **本组** →
//! `inspect`），命中不到就交给下一组。
//!
//! 参数只做**形状**校验：模板 ID 的合法形状、实例名的非空、`parent` 的三态由
//! `r3e-core` 的 `validate_prefab_id` / `Scene::spawn` 判，协议层不抄一份
//! ——抄了就会和 core 静默失配（同一套规则两份实现）。
//!
//! 两条共同口径：
//!
//! * **`parent` 省略或 `null` = 挂场景根，空串一律拒**：空串在节点名域里是个
//!   合法写法，让它兼职表达「挂根」，两种意图就分不开了。
//! * **模板没登记回 `CONFLICT`，参数写错回 `INVALID_PARAMS`**：前者调用方的下一步是
//!   「先 `prefab_define`」，后者是「改参数」，合并成一个码等于把下一步藏起来。

use super::*;
use crate::api::{PrefabInfoRequest, PrefabSpawnRequest};

pub(super) fn dispatch_prefab(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "prefab.spawn" => {
            let template = match require_str(params, "id") {
                Ok(v) if !v.trim().is_empty() => v.to_string(),
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `id` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            let name = match require_str(params, "name") {
                Ok(v) if !v.trim().is_empty() => v.to_string(),
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `name` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            let parent = match opt_str(params, "parent") {
                Ok(None) => None,
                Ok(Some(v)) if v.trim().is_empty() => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params(
                            "参数 `parent` 不能为空字符串；挂场景根请省略该参数",
                        ),
                    )
                }
                Ok(Some(v)) => Some(v.to_string()),
                Err(f) => return Response::fail(id, f),
            };
            match api.prefab_spawn(PrefabSpawnRequest {
                id: template,
                name,
                parent,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "prefab.info" => {
            let name = match require_str(params, "name") {
                Ok(v) if !v.trim().is_empty() => v.to_string(),
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `name` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.prefab_info(PrefabInfoRequest { name }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        other => inspect::dispatch_inspect(api, id, params, other, raw),
    }
}
