//! `terrain.*` 八条命令的派发臂（M2-C06）。
//!
//! 命中不到本组时交给 [`super::nav::dispatch_nav`]，不在本文件回错误。链上的位置在
//! `character` 之后、`nav` 之前：地形是角色脚下那层东西，「物理 → 角色 → 地形 → 导航」
//! 的读法最贴问题实际发生的位置（角色站不住 → 先查脚下地形，再查导航图）。
//!
//! 与 `character.rs` 同一条分工：本文件只做**参数形状**校验（缺参数、类型不对、`rect`
//! 不是四元数组），一行业务判断都不做。分辨率上限、块数上限、名字截断、步长夹取全在
//! 宿主——那里才有 `TERRAIN_MAX_RESOLUTION` / `TERRAIN_MAX_PATCHES` 的真源。
//!
//! 两条例外，理由都不是「怕宿主算错」而是「错得太安静」：
//!
//! * `terrain.create` 的 `rect` 必须**逐项有限**。`NaN` 尺寸会一路穿过 `sanitized`
//!   变成缺省尺寸，调用方看到 `size: [16,16]` 只会以为自己传错了参数名；
//! * `terrain.resample` 的 `lod` 必须 `>= 1`。步长 0 会让块内采样按 0 步取点，
//!   结果是所有顶点落在同一个样本上——地形整块拍平且不报错。

use super::*;
use crate::terrain::*;

pub(super) fn dispatch_terrain(
    api: &mut dyn EngineApi,
    id: Option<Value>,
    params: &Value,
    method: &str,
    raw: &str,
) -> Response {
    match method {
        "terrain.create" => {
            let name = match require_str(params, "name") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let rect = match require_rect(params, "rect") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let resolution = match opt_uint(params, "resolution") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_create(TerrainCreateQuery {
                name,
                rect,
                resolution,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "terrain.remove" => {
            let name = match require_str(params, "name") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_remove(TerrainNameQuery { name }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "terrain.sample" => {
            // 空名等价于「不给」：AI 常常只知道自己站的坐标，不知道块叫什么，
            // 于是把 `"name": ""` 一起带上。把它当错误拦下，等于逼它先查一遍块表。
            let name = match opt_str(params, "name") {
                Ok(Some(v)) if !v.trim().is_empty() => Some(v.to_string()),
                Ok(_) => None,
                Err(f) => return Response::fail(id, f),
            };
            let x = match require_num(params, "x") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let z = match require_num(params, "z") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_sample(TerrainSampleQuery { name, x, z }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "terrain.blocks" => {
            let block_size = match opt_num(params, "blockSize") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_blocks(TerrainBlocksQuery { block_size }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "terrain.resample" => {
            let name = match require_str(params, "name") {
                Ok(v) => v.to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let lod = match opt_num(params, "lod") {
                Ok(Some(v)) => v,
                Ok(None) => 1.0,
                Err(f) => return Response::fail(id, f),
            };
            // `NaN >= 1.0` 为 false，这一行同时挡住 NaN 与 0。
            if !lod.is_finite() || lod < 1.0 {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(format!(
                        "参数 `lod` 必须 >= 1（采样步长倍数），收到 {lod}"
                    )),
                );
            }
            let max_resolution = match opt_uint(params, "maxResolution") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_resample(TerrainResampleQuery {
                name,
                lod,
                max_resolution,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "terrain.gen" => {
            let size_x = match require_num(params, "sizeX") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let size_z = match require_num(params, "sizeZ") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let seed = match opt_u64(params, "seed") {
                Ok(v) => v.unwrap_or(0),
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_gen(TerrainGenQuery {
                size_x,
                size_z,
                seed,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 无参：汇总统计没有「统计哪块」的粒度，显式忽略客户端习惯性带的 `{}`。
        "terrain.stats" => match api.terrain_stats() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "terrain.mesh" => {
            let block = match opt_u32_pair(params, "block") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            let lod = match opt_num(params, "lod") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            if let Some(lod) = lod {
                if !lod.is_finite() || lod < 1.0 {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params(format!(
                            "参数 `lod` 必须 >= 1（采样步长倍数），收到 {lod}"
                        )),
                    );
                }
            }
            let block_size = match opt_num(params, "blockSize") {
                Ok(v) => v,
                Err(f) => return Response::fail(id, f),
            };
            match api.terrain_mesh(TerrainMeshQuery {
                block,
                lod,
                block_size,
            }) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        _ => nav::dispatch_nav(api, id, params, method, raw),
    }
}

/// 取四元数组参数（`rect = [x, z, sizeX, sizeZ]`），要求逐项有限。
fn require_rect(params: &Value, key: &str) -> Result<[f32; 4], RpcFailure> {
    match opt_f32_array::<4>(params, key)? {
        Some(v) if v.iter().all(|n| n.is_finite()) => Ok(v),
        Some(v) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 的每一项都必须是有限数，收到 {v:?}"
        ))),
        None => Err(RpcFailure::invalid_params(format!(
            "缺少四元数组参数 `{key}`（`[x, z, sizeX, sizeZ]`）"
        ))),
    }
}

/// 取必填数值参数：`opt_num` 的「必须有值」版本。
fn require_num(params: &Value, key: &str) -> Result<f32, RpcFailure> {
    match opt_num(params, key)? {
        Some(v) => Ok(v),
        None => Err(RpcFailure::invalid_params(format!("缺少数值参数 `{key}`"))),
    }
}

/// 取 `[i, j]` 块索引：整数比浮点更贴语义，宁可在这里严格也不让 `1.5` 悄悄变成 `1`。
fn opt_u32_pair(params: &Value, key: &str) -> Result<Option<[u32; 2]>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) if items.len() == 2 => {
            let mut out = [0u32; 2];
            for (i, item) in items.iter().enumerate() {
                match item.as_u64() {
                    Some(n) if n <= u32::MAX as u64 => out[i] = n as u32,
                    _ => {
                        return Err(RpcFailure::invalid_params(format!(
                            "参数 `{key}` 应为两个非负整数（块索引），第 {} 项收到 {item}",
                            i + 1
                        )))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(other) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为两个非负整数的数组，收到 {other}"
        ))),
    }
}

/// 取 u64 参数（`seed`）。走 `as_u64` 而不是 `opt_num`：种子要能表达 `u64::MAX`，
/// 转成 `f32` 会丢低位，那样「同 seed 逐位可复现」就成了空话。
fn opt_u64(params: &Value, key: &str) -> Result<Option<u64>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n)),
            None => Err(RpcFailure::invalid_params(format!(
                "参数 `{key}` 应为非负整数，收到 {v}"
            ))),
        },
    }
}
