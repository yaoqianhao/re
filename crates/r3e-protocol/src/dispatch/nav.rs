//! `nav.*` 臂（M1-S4-D + M2-D04）：`bake` / `path` / `reachability` /
//! `smooth` / `probe` / `nearest` / `stats` / `hash` / `export` / `import`。
//!
//! 本组在兜底链里排在 `physics` 之后、`inspect` 之前（`query` → `tx` → `scene`
//! → `media` → `physics` → 本组 → `inspect`），命中不到就交给下一组。
//!
//! 参数只做形状校验：`NAV_*` 的缺省值（格边长、坡度、忽略标签表）全在 `r3e-nav`
//! 的 `NavConfig::default()`，协议层不抄一份——理由见 [`crate::nav`]。
//!
//! D04 补齐七条之后，`nav.*` 十条**全部带 RPC**：卡表 CD 档要求「命令面不留只可脚本
//! 调用的半截入口」，否则 AI 在协议侧看不到 `export` / `import`，`.navmesh` 的
//! 存盘往返就只能靠 GUI 点。

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
        // ---- M2-D04：补七条 ----
        "nav.smooth" => {
            let points = match opt_point_list(params, "points") {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("缺少 `points` 数组（三元数组的数组）"),
                    )
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.nav_smooth(NavSmoothQuery { points }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "nav.probe" => {
            let Some(point) = (match opt_vec3(params, "point") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `point`"));
            };
            match api.nav_probe(NavPointQuery { point }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "nav.nearest" => {
            let Some(point) = (match opt_vec3(params, "point") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            }) else {
                return Response::fail(id, RpcFailure::invalid_params("缺少三元数组参数 `point`"));
            };
            let radius = match opt_num(params, "radius") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let region = match opt_u32(params, "region") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.nav_nearest(NavNearestQuery {
                point,
                radius,
                region,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "nav.stats" => match api.nav_stats() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "nav.hash" => match api.nav_hash() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "nav.export" | "nav.import" => {
            // 路径是必填的：两条命令都不提供缺省落盘位置（§7 回退档纪律）。
            let path = match require_str(params, "path") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let target = NavFileTarget { path };
            let result = if method == "nav.export" {
                api.nav_export(target)
            } else {
                api.nav_import(target)
            };
            match result {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        other => inspect::dispatch_inspect(api, id, params, other, raw),
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

/// 解析 `points`：三元数组的数组（`nav.smooth` 的折线）。
///
/// 逐点校验长度与非有限值：**任何一点不合法就整条拒**，不做「跳过坏点继续剪」——
/// 折线跳过一个点，剪出来的路径就不再是调用方给的那条了。
fn opt_point_list(params: &Value, key: &str) -> Result<Option<Vec<[f32; 3]>>, RpcFailure> {
    let Some(raw) = params.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let array = raw
        .as_array()
        .ok_or_else(|| RpcFailure::invalid_params(format!("`{key}` 必须是三元数组的数组")))?;
    let mut out = Vec::with_capacity(array.len());
    for (idx, item) in array.iter().enumerate() {
        let triple = item
            .as_array()
            .ok_or_else(|| RpcFailure::invalid_params(format!("`{key}[{idx}]` 必须是三元数组")))?;
        if triple.len() != 3 {
            return Err(RpcFailure::invalid_params(format!(
                "`{key}[{idx}]` 必须是 3 个数，收到 {} 个",
                triple.len()
            )));
        }
        let mut point = [0.0f32; 3];
        for (axis, value) in triple.iter().enumerate() {
            let num = value.as_f64().ok_or_else(|| {
                RpcFailure::invalid_params(format!("`{key}[{idx}][{axis}]` 不是数字"))
            })? as f32;
            if !num.is_finite() {
                return Err(RpcFailure::invalid_params(format!(
                    "`{key}[{idx}][{axis}]` 不是有限值"
                )));
            }
            point[axis] = num;
        }
        out.push(point);
    }
    Ok(Some(out))
}

/// 解析非负整数（`nav.nearest` 的 `region` 限定）。
///
/// 走 `as_f64` 再判整：JSON 里 `region: 2.5` 这类值必须报参数错，而不是被 `as u32`
/// 静默截成 2 —— 截断之后查的是「另一个连通块」，回包看不出被改过。
fn opt_u32(params: &Value, key: &str) -> Result<Option<u32>, RpcFailure> {
    let Some(raw) = params.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let num = raw
        .as_f64()
        .ok_or_else(|| RpcFailure::invalid_params(format!("`{key}` 必须是整数")))?;
    if !num.is_finite() || num < 0.0 || num.fract() != 0.0 || num > u32::MAX as f64 {
        return Err(RpcFailure::invalid_params(format!(
            "`{key}` 必须是非负整数，收到 {num}"
        )));
    }
    Ok(Some(num as u32))
}
