//! `terrain.*` 八条命令的宿主侧实现（M2-C06）。
//!
//! 分工只有一句：**本文件把协议参数交给 `r3e_runtime::terrain_ops`，再把它的错误
//! 翻成协议码**。块划分、LOD、三角网、高度采样全在 `r3e-terrain` 侧算，
//! 宿主不重新实现任何一条——本 crate 的 `allow` 里没有 `r3e-terrain`（CD-2 红线），
//! 一旦在这里"顺手"算一遍地形，同一条曲线就会有两份实现，跨块接缝处先露馅。
//!
//! ## 写命令的四条护栏
//!
//! `create` / `remove` / `resample` / `gen` 都改场景，所以：
//!
//! 1. 事务未决时回 [`TX_REJECTED`]：地形写命令还没进 `tx.apply` 的 op 白名单，
//!    暂存到影子会让调用方以为"改了"，`tx.commit` 时才发现没这一步；
//! 2. 一律经 `CommandKind`（已在 `terrain_ops` 内），所以可撤销、进 `state_hash`；
//! 3. 回包必带 `stateHash`——「这次写到底改了没改场景」唯一可靠的自证；
//! 4. 失败不写场景：`terrain_ops` 先全部校验再落笔，本文件不做二次修补。
//!
//! ## 两种"没有"
//!
//! * 块不存在 → [`TERRAIN_PATCH_MISSING`]（`remove` 例外：回 `removed: false`，
//!   删除天然幂等，脚本重跑不该红）；
//! * 块存在但点落在块外 → **成功**回 `inside: false`。这不是错误，
//!   是「你问的地方没有地形」这个事实本身；报错会把 `inside` 字段变成永远为真的装饰。

use r3e_editor::{
    TerrainBlocksQuery, TerrainCreateQuery, TerrainGenQuery, TerrainMeshQuery, TerrainNameQuery,
    TerrainResampleQuery, TerrainSampleQuery, TERRAIN_BLOCK_MISSING, TERRAIN_PATCH_MISSING,
    TX_REJECTED,
};
use r3e_runtime::terrain_ops::{self as ops, TerrainOpError};
use serde_json::{json, Value};

use super::{tx, LocalEngineApi, RpcFailure};

/// 引擎侧错误 → RPC 失败。
///
/// `Rejected`（表满 / 名字收敛后为空）刻意映射成 `ENGINE` 而不是 `TX_REJECTED`：
/// 这两条 RPC 没在事务里跑，回事务码只会把调用方带去查 `tx.*` 的手册。
fn map_op(err: TerrainOpError) -> RpcFailure {
    match err {
        TerrainOpError::PatchMissing(detail) => RpcFailure::new(TERRAIN_PATCH_MISSING, detail),
        TerrainOpError::BlockMissing(detail) => RpcFailure::new(TERRAIN_BLOCK_MISSING, detail),
        TerrainOpError::Invalid(detail) => RpcFailure::invalid_params(detail),
        TerrainOpError::Rejected(detail) => RpcFailure::engine(detail),
        TerrainOpError::Doc(detail) => RpcFailure::engine(detail),
    }
}

/// 写命令的统一前置：事务未决就停手。
fn ensure_no_tx(api: &LocalEngineApi<'_>, op: &str) -> Result<(), RpcFailure> {
    if api.scene.borrow().tx_session().is_some() {
        return Err(RpcFailure::new(
            TX_REJECTED,
            format!("地形写入暂不进事务影子：先 `tx.commit` 或 `tx.rollback`，再调 `{op}`"),
        ));
    }
    Ok(())
}

/// 地形块的出网形态：样本数组只给数量（`samples`），不搬 16384 个浮点。
fn patch_value(patch: &r3e_core::TerrainPatch) -> Value {
    json!({
        "name": patch.name,
        "origin": [patch.origin[0], patch.origin[1]],
        "size": [patch.size[0], patch.size[1]],
        "resolution": [patch.resolution[0], patch.resolution[1]],
        "albedo": [patch.albedo[0], patch.albedo[1], patch.albedo[2]],
        "roughness": patch.roughness,
        "samples": patch.expected_samples(),
    })
}

/// `terrain.create`：同名整体覆盖，超上限只拦新增。
pub(super) fn terrain_create(
    api: &mut LocalEngineApi<'_>,
    query: TerrainCreateQuery,
) -> Result<Value, RpcFailure> {
    ensure_no_tx(api, "terrain.create")?;
    let mut document = api.scene.borrow_mut();
    let out =
        ops::create(&mut document, &query.name, query.rect, query.resolution).map_err(map_op)?;
    Ok(json!({
        "name": out.patch.name,
        "created": out.created,
        "patch": patch_value(&out.patch),
        "stateHash": tx::hex(out.state_hash),
    }))
}

/// `terrain.remove`：按名字删；名字不存在回 `removed: false`（幂等）。
pub(super) fn terrain_remove(
    api: &mut LocalEngineApi<'_>,
    query: TerrainNameQuery,
) -> Result<Value, RpcFailure> {
    ensure_no_tx(api, "terrain.remove")?;
    let mut document = api.scene.borrow_mut();
    match ops::remove(&mut document, &query.name) {
        Ok(out) => Ok(json!({
            "name": query.name.trim(),
            "removed": true,
            "remaining": out.remaining,
            "stateHash": tx::hex(out.state_hash),
        })),
        // 只有「名字不存在」被翻成幂等成功：其余失败（空名、命令层拒绝）照常报错，
        // 否则「删了但没删掉」会被 `removed: false` 掩盖成「本来就没有」。
        Err(TerrainOpError::PatchMissing(_)) => Ok(json!({
            "name": query.name.trim(),
            "removed": false,
            "remaining": document.as_scene().terrain.len(),
            "stateHash": tx::hex(document.state_hash()),
        })),
        Err(err) => Err(map_op(err)),
    }
}

/// `terrain.sample`：世界点高度 + 法线 + 混合权重（只读）。
pub(super) fn terrain_sample(
    api: &mut LocalEngineApi<'_>,
    query: TerrainSampleQuery,
) -> Result<Value, RpcFailure> {
    let document = api.scene.borrow();
    let out = ops::sample(&document, query.name.as_deref(), query.x, query.z).map_err(map_op)?;
    Ok(json!({
        "name": out.name,
        "x": query.x,
        "z": query.z,
        "height": out.height,
        "normal": [out.normal[0], out.normal[1], out.normal[2]],
        "weights": [out.weights[0], out.weights[1], out.weights[2], out.weights[3]],
        "inside": out.inside,
    }))
}

/// `terrain.blocks`：按块边长切块，回逐块清单与覆盖统计（只读）。
pub(super) fn terrain_blocks(
    api: &mut LocalEngineApi<'_>,
    query: TerrainBlocksQuery,
) -> Result<Value, RpcFailure> {
    let document = api.scene.borrow();
    let out = ops::blocks(&document, query.block_size).map_err(map_op)?;
    let blocks: Vec<Value> = out
        .blocks
        .iter()
        .map(|b| {
            json!({
                "index": [b.index[0], b.index[1]],
                "rect": [b.rect[0], b.rect[1], b.rect[2], b.rect[3]],
                "patches": b.patches,
            })
        })
        .collect();
    Ok(json!({
        "rect": [out.rect[0], out.rect[1], out.rect[2], out.rect[3]],
        "blockSize": out.block_size,
        "dims": [out.dims[0], out.dims[1]],
        "count": out.count,
        "covered": out.covered,
        "blocks": blocks,
    }))
}

/// `terrain.resample`：按步长重采样并写回（可撤销）。
pub(super) fn terrain_resample(
    api: &mut LocalEngineApi<'_>,
    query: TerrainResampleQuery,
) -> Result<Value, RpcFailure> {
    ensure_no_tx(api, "terrain.resample")?;
    let mut document = api.scene.borrow_mut();
    let out = ops::resample(&mut document, &query.name, query.lod, query.max_resolution)
        .map_err(map_op)?;
    Ok(json!({
        "name": query.name.trim(),
        "from": [out.from[0], out.from[1]],
        "to": [out.to[0], out.to[1]],
        "samples": out.samples,
        "maxError": out.max_error,
        "stateHash": tx::hex(out.state_hash),
    }))
}

/// `terrain.gen`：程序化生成整片地形并写进场景（可撤销）。
pub(super) fn terrain_gen(
    api: &mut LocalEngineApi<'_>,
    query: TerrainGenQuery,
) -> Result<Value, RpcFailure> {
    ensure_no_tx(api, "terrain.gen")?;
    let mut document = api.scene.borrow_mut();
    let out = ops::generate_into(&mut document, query.size_x, query.size_z, query.seed)
        .map_err(map_op)?;
    Ok(json!({
        "seed": query.seed,
        "created": out.created,
        "names": out.names,
        "stateHash": tx::hex(out.state_hash),
    }))
}

/// `terrain.stats`：地形汇总（只读，三角/顶点与 `terrain.mesh` 同口径）。
pub(super) fn terrain_stats(api: &mut LocalEngineApi<'_>) -> Result<Value, RpcFailure> {
    let document = api.scene.borrow();
    let out = ops::stats(&document).map_err(map_op)?;
    Ok(json!({
        "patches": out.patches,
        "samples": out.samples,
        "blocks": out.blocks,
        "covered": out.covered,
        "triangles": out.triangles,
        "vertices": out.vertices,
        "lod": out.lod.step(),
        "blockSize": out.block_size,
    }))
}

/// `terrain.mesh`：单块 / 全块的派生网格统计（只读，不回顶点）。
pub(super) fn terrain_mesh(
    api: &mut LocalEngineApi<'_>,
    query: TerrainMeshQuery,
) -> Result<Value, RpcFailure> {
    let document = api.scene.borrow();
    let out = ops::mesh(&document, query.block, query.lod, query.block_size).map_err(map_op)?;
    Ok(json!({
        "lod": out.lod.step(),
        "block": out.block.map(|b| json!([b[0], b[1]])).unwrap_or(Value::Null),
        "blocks": out.blocks,
        "vertices": out.vertices,
        "triangles": out.triangles,
        "bounds": out.bounds.map(|b| json!([b[0], b[1], b[2], b[3], b[4], b[5]])).unwrap_or(Value::Null),
    }))
}
