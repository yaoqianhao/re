//! 四条只读检视 RPC 的宿主实现（M1-S1-D）：`scene.snapshot` / `mesh.stats` /
//! `scene.validate` / `physics.simulate`。
//!
//! 本文件只做「协议 ↔ 内部」的翻译，计算一律在 [`crate::inspect`]：
//!
//! 1. **场景来源**：`path` 按「绝对路径 / 进程工作目录 / 当前场景目录」三档解析
//!    （与 `scene.switch` 同一套 `resolve_request_path`），`scene` 吃 JSON 文本；
//!    两条都缺省时取**当前编辑场景**——宿主手里本来就有一份，没必要逼调用方回传。
//! 2. **错误码分工**：形状/区间错归 `invalid_params`（-32602，改参数），数据本身解析
//!    不了归 `engine`（-32001，改场景）——与 `scene.switch` 读不了文件/解析失败同一口径。
//! 3. **只读**：四条都不借出 `Document` 的可变引用；`physics.simulate` 在**副本**上跑，
//!    真文档的 `StateHash` 前后不变，所以它们是只读命令而不是写命令。

use super::support::resolve_request_path;
use super::*;
use crate::inspect::{self, InspectError, SimulateOptions};
use r3e_editor::{MeshStatsQuery, SimulateQuery, SnapshotQuery, ValidateQuery};

/// `scene.snapshot`：场景结构快照。
pub(super) fn scene_snapshot(
    api: &mut LocalEngineApi<'_>,
    query: SnapshotQuery,
) -> Result<Value, RpcFailure> {
    let loaded = scene_from_source(&query.scene, &query.path, &api.scene_label)?;
    let borrowed;
    let scene: &Scene = match &loaded {
        Some(scene) => scene,
        None => {
            borrowed = api.scene.borrow();
            &borrowed
        }
    };
    Ok(inspect::snapshot(scene, query.max_nodes))
}

/// `mesh.stats`：程序化网格统计 + AIR 校验（不碰场景）。
pub(super) fn mesh_stats(
    _api: &mut LocalEngineApi<'_>,
    query: MeshStatsQuery,
) -> Result<Value, RpcFailure> {
    inspect::mesh_stats(
        query.shape.as_deref(),
        query.segments,
        query.rings,
        query.divisions,
    )
    .map_err(translate)
}

/// `scene.validate`：逐节点网格体检（AIR），每条问题带结构化定位。
pub(super) fn scene_validate(
    api: &mut LocalEngineApi<'_>,
    query: ValidateQuery,
) -> Result<Value, RpcFailure> {
    let loaded = scene_from_source(&query.scene, &query.path, &api.scene_label)?;
    let borrowed;
    let scene: &Scene = match &loaded {
        Some(scene) => scene,
        None => {
            borrowed = api.scene.borrow();
            &borrowed
        }
    };
    Ok(inspect::validate(scene))
}

/// `physics.simulate`：在临时世界跑一段，回报位移与事件。
///
/// 副本 `Scene::clone` 是必要条件：`physics.step` 会写节点变换，直接在真文档上跑
/// 会让「查一下会不会掉下去」变成「把场景挪了」——那是写命令，不是只读命令。
pub(super) fn physics_simulate(
    api: &mut LocalEngineApi<'_>,
    query: SimulateQuery,
) -> Result<Value, RpcFailure> {
    let scene = match scene_from_source(&query.scene, &query.path, &api.scene_label)? {
        Some(scene) => scene,
        None => Scene::clone(&api.scene.borrow()),
    };
    let options = SimulateOptions {
        steps: query.steps,
        dt: query.dt,
        max_bodies: query.max_bodies,
        backend: query.backend.as_deref(),
        gravity: query.gravity,
        ground: query.ground,
    };
    inspect::simulate(scene, options).map_err(translate)
}

/// 场景来源解析：`Some(scene)` = 按 `path` / `scene` 现解析一份；`None` = 用当前编辑场景。
///
/// 只认 JSON 文本（内联对象已由 `dispatch` 序列化），所以这里不必实现两条解析路径。
fn scene_from_source(
    scene: &Option<String>,
    path: &Option<String>,
    scene_label: &str,
) -> Result<Option<Scene>, RpcFailure> {
    if let Some(path) = path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        let target = resolve_request_path(scene_label, path);
        if !target.is_file() {
            return Err(RpcFailure::invalid_params(format!(
                "场景文件不存在：{}",
                target.display()
            )));
        }
        let text = std::fs::read_to_string(&target).map_err(|err| {
            RpcFailure::engine(format!("读取场景 `{}` 失败：{err}", target.display()))
        })?;
        return r3e_doc::parse_scene(&text).map(Some).map_err(|err| {
            RpcFailure::engine(format!("场景 `{}` 解析失败：{err}", target.display()))
        });
    }
    match scene.as_deref() {
        // 只给空白也算没给：空场景文本解析出来是「零个节点」，那是个假答案。
        Some(text) if !text.trim().is_empty() => r3e_doc::parse_scene(text)
            .map(Some)
            .map_err(|err| RpcFailure::engine(format!("scene 文本解析失败：{err}"))),
        _ => Ok(None),
    }
}

/// 检视层错误 → RPC 失败：参数错与数据错必须分开报，调用方才知道该改哪边。
fn translate(error: InspectError) -> RpcFailure {
    match error {
        InspectError::InvalidParams(message) => RpcFailure::invalid_params(message),
        InspectError::InvalidInput(message) => RpcFailure::engine(message),
    }
}
