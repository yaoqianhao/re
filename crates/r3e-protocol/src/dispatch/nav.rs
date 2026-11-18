//! `nav.*` 臂（M1-S4-D）：`bake` / `path` / `reachability`。
//!
//! 本组是兜底链的最后一环（`query` → `tx` → `scene` → `media` → `physics` → 本组），
//! 命中不到就直接回 `METHOD_NOT_FOUND`。
//!
//! 参数只做形状校验：`NAV_*` 的缺省值（格边长、坡度、忽略标签表）全在 `r3e-nav`
//! 的 `NavConfig::default()`，协议层不抄一份——理由见 [`crate::nav`]。

use super::*;

pub(super) fn dispatch_nav(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "nav.bake" => {
            let opts = match nav_options_from(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.nav_bake(opts) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "nav.path" => {
            let Some(from) = (match opt_vec3(params, "from") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `from`"));
            };
            let Some(to) = (match opt_vec3(params, "to") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `to`"));
            };
            match api.nav_path(NavPathQuery { from, to }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "nav.reachability" => {
            let Some(from) = (match opt_vec3(params, "from") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `from`"));
            };
            let Some(to) = (match opt_vec3(params, "to") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `to`"));
            };
            match api.nav_reachability(NavReachQuery { from, to }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        other => Response::fail(
            id,
            RpcFailure::new(METHOD_NOT_FOUND, format!("未知方法 `{}`", raw))
                .with_data(json!({ "normalized": other })),
        ),
    }
}

/// 解析 `nav.bake` 的烘焙参数（全部可缺省，一个 `{}` 就是全缺省）。
fn nav_options_from(params: &Value) -> Result<NavBakeOptions, RpcFailure> {
    // `ignoreTags` 用 `opt_str_list` 的「三态」语义：`None`（没给 / null）= 用缺省
    // 忽略表，`Some([])` = 一张都不忽略。这两种意图必须分得开——合成一个之后
    // 「把会动的物件也烘成障碍」就没法表达了。
    Ok(NavBakeOptions {
        cell_size: opt_num(params, "cellSize")?,
        bounds: opt_f32_array::<4>(params, "bounds")?,
        agent_radius: opt_num(params, "agentRadius")?,
        agent_height: opt_num(params, "agentHeight")?,
        step_height: opt_num(params, "stepHeight")?,
        max_slope_deg: opt_num(params, "maxSlopeDeg")?,
        ignore_tags: opt_str_list(params, "ignoreTags")?,
    })
}
