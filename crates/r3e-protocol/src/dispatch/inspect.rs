//! 检视类只读臂（M1-S1-D）：`scene.snapshot` / `mesh.stats` / `scene.validate` /
//! `physics.simulate`。
//!
//! 本组是兜底链的最后一环（`query` → `tx` → `scene` → `media` → `physics` → `nav`
//! → 本组），命中不到就直接回 `METHOD_NOT_FOUND`。
//!
//! 四条都是「原插件进程内直连 core / physics 的离线工具」搬到引擎侧的出口，
//! 所以本层只做**形状校验**：缺省值、夹取范围、`dt` 区间、后端是否编进来
//! 一律由宿主与 core 裁决。协议层抄一份就是两份真相。

use super::*;
use crate::inspect::{MeshStatsQuery, SimulateQuery, SnapshotQuery, ValidateQuery};

pub(super) fn dispatch_inspect(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "scene.snapshot" => {
            let (scene, path) = match scene_source(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let query = SnapshotQuery {
                scene,
                path,
                max_nodes: match opt_uint(params, "maxNodes") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
            };
            match api.scene_snapshot(query) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "mesh.stats" => {
            let query = MeshStatsQuery {
                shape: match opt_str(params, "shape") {
                    Ok(v) => v.map(str::to_string),
                    Err(f) => return Response::fail(id, f),
                },
                segments: match opt_uint(params, "segments") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                rings: match opt_uint(params, "rings") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                divisions: match opt_uint(params, "divisions") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
            };
            match api.mesh_stats(query) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.validate" => {
            let (scene, path) = match scene_source(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_validate(ValidateQuery { scene, path }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "physics.simulate" => {
            let (scene, path) = match scene_source(params) {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let query = SimulateQuery {
                scene,
                path,
                steps: match opt_uint(params, "steps") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                dt: match opt_num(params, "dt") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                max_bodies: match opt_uint(params, "maxBodies") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                backend: match opt_str(params, "backend") {
                    Ok(v) => v.map(str::to_string),
                    Err(f) => return Response::fail(id, f),
                },
                gravity: match opt_vec3(params, "gravity") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
                ground: match opt_num(params, "ground") {
                    Ok(v) => v,
                    Err(f) => return Response::fail(id, f),
                },
            };
            match api.physics_simulate(query) {
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

/// 解出「一份场景」的两种来源：内联文本（对象已序列化）与文件路径。
///
/// 两条都缺省不拦，宿主会取**当前编辑场景**；这层只管形状。
/// 内联对象在这里就转成文本，宿主只认一种形态——否则宿主得同时实现「值」与
/// 「文本」两条解析路径，两条迟早会给出不同的错误文案。
fn scene_source(params: &Value) -> Result<(Option<String>, Option<String>), RpcFailure> {
    let scene = match param(params, "scene") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(value @ Value::Object(_)) => Some(value.to_string()),
        Some(_) => {
            return Err(RpcFailure::invalid_params(
                "参数 `scene` 应为场景对象或场景 JSON 文本",
            ))
        }
    };
    let path = opt_str(params, "path")?.map(str::to_string);
    Ok((scene, path))
}
