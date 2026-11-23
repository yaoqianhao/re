//! `character.*` 六条命令的派发臂（M2-B01）。
//!
//! 命中不到本组时交给 [`super::nav::dispatch_nav`]，不在本文件回错误。链上的位置在
//! `physics` 之后、`nav` 之前：角色命令吃物理查询，出问题时按「物理 → 角色 → 导航」
//! 的顺序读下来最顺。
//!
//! 本文件只做**参数形状**校验，一行业务判断都不做：五个参数字段的缺省值、非法值
//! （半径 <= 0、坡度 > 89°）的裁决全在宿主 `CharacterSpec::validate`。这里若也判一遍，
//! 同一个上限就会有两个真源，改一处必漏一处。
//!
//! 一条例外：`character.step` 的 `dt <= 0` 在这里就拒。理由不是「怕宿主算错」，而是
//! `dt = 0` 会让求解器**静默不动**——脚本以为在走路、画面却纹丝不动，这种「不报错
//! 但不生效」最难查。参数显然非法时早报错比晚报错便宜。

use super::*;
use crate::character::*;

pub(super) fn dispatch_character(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        // create 与 config 共用一套参数：区别只在宿主侧语义（建 / 改），协议层不重复两遍。
        "character.create" | "character.config" => {
            let node = match require_str(params, "node") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let body = match body_params(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let q = CharacterBindQuery { node, body };
            let result = if method == "character.create" {
                api.character_create(q)
            } else {
                api.character_config(q)
            };
            match result {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "character.step" => {
            let node = match require_str(params, "node") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let wish = match opt_f32_array::<2>(params, "wish") {
                Ok(v) => v.unwrap_or([0.0, 0.0]),
                Err(f) => return Response::fail(id, f),
            };
            let jump = match opt_bool(params, "jump") {
                Ok(v) => v.unwrap_or(false),
                Err(f) => return Response::fail(id, f),
            };
            let dt = match opt_f32(params, "dt") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            if let Some(dt) = dt {
                // `dt` 为 NaN 时比较全 false，`is_finite` 兜住它：NaN 也不许静默推进。
                if dt <= 0.0 || !dt.is_finite() {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params(format!("参数 `dt` 必须为正数，收到 {dt}")),
                    );
                }
            }
            match api.character_step(CharacterStepQuery {
                node,
                wish,
                jump,
                dt,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "character.state" => {
            let node = match require_str(params, "node") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            match api.character_state(CharacterNameQuery { node }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "character.teleport" => {
            let node = match require_str(params, "node") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            // 位置是**脚底**：协议层只搬值，脚底 ↔ 胶囊中心的换算在宿主。
            let position = match opt_vec3(params, "position") {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("缺少三元数组参数 `position`"),
                    )
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.character_teleport(CharacterTeleportQuery { node, position }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 无参：统计是全局汇总，没有「统计哪个」的粒度。
        // 显式忽略 `params` 是因为客户端习惯性带一个 `{}`，不该因此报参数错误。
        "character.stats" => match api.character_stats() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        _ => terrain::dispatch_terrain(api, id, params, method, raw),
    }
}

/// 解析五个几何参数：全缺省是合法的（`create` 走缺省、`config` 等于不改）。
fn body_params(params: &Value) -> Result<CharacterBodyParams, RpcFailure> {
    Ok(CharacterBodyParams {
        radius: opt_f32(params, "radius")?,
        height: opt_f32(params, "height")?,
        slope_limit_deg: opt_f32(params, "slopeLimitDeg")?,
        step_height: opt_f32(params, "stepHeight")?,
        snap_distance: opt_f32(params, "snapDistance")?,
    })
}
