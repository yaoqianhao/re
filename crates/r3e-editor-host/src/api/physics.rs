//! `physics.*` 三条探测查询的宿主实现（M1-S4-D）。
//!
//! 协议层只搬值（`[f32; 3]` + 字符串标签），引擎类型全部在这里翻译：
//! 归一化、缺省射程、退化形状、NaN 检查一律落在宿主侧——同一个非法入参
//! 在两层各有一套「容错」，最后谁都说不清哪层生效。
//!
//! 三条铁律（见 `r3e_protocol::api` 的 M1-S4-D 段）的落地点：
//!
//! * **能力位随回包走**：三条的成功回包都带 `backend` + `capabilities`，
//!   调用方不必先探一次「你支不支持」。
//! * **区分「没有」和「不会」**：物理关着回 [`PHYSICS_DISABLED`]，后端不申报
//!   该位回 [`BACKEND_UNSUPPORTED`]，**不返回空表**——空表会被读成
//!   「场景里真没东西」，从而去改本来就对的场景。
//! * **只读**：三条都不 `borrow_mut`，`StateHash` 前后一致。

use super::support::check_finite;
use super::*;
use r3e_core::query::DEFAULT_LIMIT;
use r3e_core::NodeId;
use r3e_editor::CastShapeSpec;
use r3e_editor::{BACKEND_UNSUPPORTED, PHYSICS_DISABLED};
use r3e_physics::{CastShape, Physics, Ray};

/// `maxDistance` 的宿主缺省射程（米）。
///
/// 缺省留在宿主而不是协议层：这个数改了只该改一处，抄到协议层就会变成
/// 「协议说 100 米、宿主打 80 米」的第二个真源。编辑器拾取场景规模远小于此值。
pub(super) const DEFAULT_PROBE_DISTANCE: f32 = 100.0;

/// 物理关闭 / 后端不申报该能力位 → 明确错误码，绝不返回空结果。
fn probe_gate(physics: &Physics, capability: &str, supported: bool) -> Result<(), RpcFailure> {
    if !physics.enabled() {
        return Err(RpcFailure::new(
            PHYSICS_DISABLED,
            "物理已关闭（physics.enabled = false）：本次没有发出任何查询，先打开物理再探测",
        ));
    }
    if !supported {
        return Err(RpcFailure::new(
            BACKEND_UNSUPPORTED,
            format!(
                "当前物理后端 `{}` 未申报 `{capability}` 能力：换后端或改用其它探测方式",
                physics.backend_name()
            ),
        ));
    }
    Ok(())
}

/// 后端申报的能力位（随每条回包带走）。
fn capabilities(physics: &Physics) -> Vec<&'static str> {
    physics.capabilities().enabled_keys()
}

/// 射程口径：`None` 取宿主缺省，显式给的必须是正的有限值。
///
/// 负射程不解释成「不限」：那会让「参数写错」和「真的想打穿全场景」共用一种写法。
fn probe_distance(asked: Option<f32>) -> Result<f32, RpcFailure> {
    match asked {
        None => Ok(DEFAULT_PROBE_DISTANCE),
        Some(d) if d.is_finite() && d > 0.0 => Ok(d),
        Some(d) => Err(RpcFailure::invalid_params(format!(
            "maxDistance 必须是 > 0 的有限值（收到 {d}）：负数不是「不限」，是非法射程"
        ))),
    }
}

/// 方向必须非零：零向量没有「朝哪打」可言，早报比让后端拿默认方向打一枪好。
fn direction(raw: [f32; 3], key: &str) -> Result<Vec3, RpcFailure> {
    check_finite(&raw, key)?;
    let dir = Vec3::from_array(raw);
    if dir.length() <= 1e-6 {
        return Err(RpcFailure::invalid_params(format!(
            "{key} 是零向量：没有方向就无从发射"
        )));
    }
    Ok(dir)
}

/// `NodeId` → 物件名。节点可能在查询与投影之间被删，故给 `Option`。
fn node_name(scene: &Scene, id: NodeId) -> Option<String> {
    scene.get(id).map(|node| node.name.clone())
}

/// `physics.raycast`：一点朝一个方向打射线，拿最近命中体。
pub(super) fn physics_raycast(
    api: &mut LocalEngineApi<'_>,
    q: RayQuery,
) -> Result<Value, RpcFailure> {
    let origin = {
        check_finite(&q.origin, "origin")?;
        Vec3::from_array(q.origin)
    };
    let dir = direction(q.dir, "dir")?;
    let max_distance = probe_distance(q.max_distance)?;

    let physics = api.physics.borrow();
    let supported = physics.capabilities().raycast;
    probe_gate(&physics, "raycast", supported)?;

    let caps = capabilities(&physics);
    let backend = physics.backend_name();
    let Some(hit) = physics.raycast(Ray::new(origin, dir), max_distance) else {
        return Ok(json!({
            "hit": false,
            "backend": backend,
            "capabilities": caps,
        }));
    };
    // 场景只读借用：命中的 `NodeId` 要翻成名字，AI 才认得出打到了谁。
    let document = api.scene.borrow();
    let scene: &Scene = &document;
    Ok(json!({
        "hit": true,
        "distance": hit.distance,
        "point": hit.point.to_array(),
        "normal": hit.normal.to_array(),
        "node": node_name(scene, hit.node),
        "handle": hit.handle.0,
        "backend": backend,
        "capabilities": caps,
    }))
}

/// `physics.overlap`：球体覆盖范围内的刚体。
pub(super) fn physics_overlap(
    api: &mut LocalEngineApi<'_>,
    q: OverlapQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.center, "center")?;
    if !(q.radius.is_finite() && q.radius > 0.0) {
        return Err(RpcFailure::invalid_params(format!(
            "radius 必须是 > 0 的有限值（收到 {}）：零半径球查不到任何东西",
            q.radius
        )));
    }
    let center = Vec3::from_array(q.center);
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT);

    let physics = api.physics.borrow();
    let supported = physics.capabilities().overlap;
    probe_gate(&physics, "overlap", supported)?;

    let caps = capabilities(&physics);
    let backend = physics.backend_name();
    let document = api.scene.borrow();
    let scene: &Scene = &document;

    // 引擎只给「谁在里面」，不给距离；`distance` 按球心到物件世界原点的距离补，
    // 语义见 `r3e_protocol::api::physics_overlap`（不是穿透深度）。
    let mut hits: Vec<(Option<String>, u64, f32)> = physics
        .overlap_sphere(center, q.radius)
        .into_iter()
        .map(|hit| {
            let distance = scene
                .get(hit.node)
                .map(|node| node.transform.position.distance(center))
                .unwrap_or(f32::INFINITY);
            (node_name(scene, hit.node), hit.handle.0, distance)
        })
        .collect();
    let count = hits.len();
    // 近的排前面：同一次查询里「谁更近」是调用方最关心的排序依据。
    hits.sort_by(|a, b| a.2.total_cmp(&b.2));
    let truncated = limit != 0 && count > limit;
    if truncated {
        hits.truncate(limit);
    }
    Ok(json!({
        "count": count,
        "limit": limit,
        "truncated": truncated,
        "hits": hits
            .into_iter()
            .map(|(node, handle, distance)| json!({
                "node": node,
                "handle": handle,
                "distance": distance,
            }))
            .collect::<Vec<_>>(),
        "backend": backend,
        "capabilities": caps,
    }))
}

/// `physics.cast`：把球 / 盒沿方向推过去，拿最近一次接触。
pub(super) fn physics_cast(
    api: &mut LocalEngineApi<'_>,
    q: CastQuery,
) -> Result<Value, RpcFailure> {
    check_finite(&q.origin, "origin")?;
    let origin = Vec3::from_array(q.origin);
    let dir = direction(q.dir, "dir")?;
    let max_distance = probe_distance(q.max_distance)?;
    let shape = cast_shape(q.shape)?;

    let physics = api.physics.borrow();
    let supported = physics.capabilities().shape_cast;
    probe_gate(&physics, "shape_cast", supported)?;

    let caps = capabilities(&physics);
    let backend = physics.backend_name();
    let Some(hit) = physics.shape_cast(shape, origin, dir, max_distance) else {
        return Ok(json!({
            "hit": false,
            "backend": backend,
            "capabilities": caps,
        }));
    };
    let document = api.scene.borrow();
    let scene: &Scene = &document;
    Ok(json!({
        "hit": true,
        "distance": hit.distance,
        "point": hit.point.to_array(),
        "normal": hit.normal.to_array(),
        "startedInside": hit.started_inside,
        "node": node_name(scene, hit.node),
        "handle": hit.handle.0,
        "backend": backend,
        "capabilities": caps,
    }))
}

/// 协议侧形状标签 → 引擎查询形状。
///
/// 球取绝对值、盒取绝对半尺寸（与 `CastShape` 的构造器同口径），
/// 退化到 0 直接判参数错误：任何后端都查不出有意义的结果，不如早说。
fn cast_shape(spec: CastShapeSpec) -> Result<CastShape, RpcFailure> {
    let shape = match spec {
        CastShapeSpec::Sphere { radius } => {
            if !radius.is_finite() {
                return Err(RpcFailure::invalid_params("shape.radius 不是有限值"));
            }
            CastShape::sphere(radius)
        }
        CastShapeSpec::Box { half } => {
            check_finite(&half, "shape.half")?;
            CastShape::boxed(Vec3::from_array(half))
        }
    };
    if shape.is_degenerate() {
        return Err(RpcFailure::invalid_params(
            "扫掠形状尺寸退化到 0：给个正的半径或半尺寸",
        ));
    }
    Ok(shape)
}
