//! 物理层复核（M1-S4-E）：把静态层的 AABB 近似结论**换成确定结论**。
//!
//! 静态层（`r3e-doc/validate/{support,overlap}.rs`）手里没有碰撞体，只能拿包围盒猜，
//! 于是它会报两类噪声：「装饰物之间的 AABB 交叠」和「球体被当成方块后的悬空」。
//! 本层用物理侧的装配描述（[`BodyDesc::from_node`]，与 `Physics::bind_scene` 同一个
//! 入口）重新判一遍，口径是：
//!
//! * **只判装配体**：没有 `physics` 段的节点（纯视觉物件）物理层根本没有碰撞体，
//!   对它下结论就是编——这类节点留给静态层的近似条目；
//! * **只判能判死的**：球-球、盒-盒、球-盒三种组合有解析解；`capsule` / `cylinder`
//!   的解析相交不在本卡范围，**不报**（不报 ≠ 通过：静态层的 `support/overlap.suspect`
//!   仍然留着，标着近似）；
//! * **只判无旋转的**：本层用的是轴对齐解析式。节点带世界旋转时结论会偏，此时整对
//!   跳过并在 `unchecked` 里记账，而不是拿错几何硬报。
//!
//! 铁律②（近似被精确覆盖）靠 `r3e_doc::dedup_superseded` 落地：本层不删静态层的条目，
//! 只在同 `(category, target)` 上给出 `approximate = false` 的结论，由组装层统一覆盖。

use r3e_core::{Node, Scene, Vec3};
use r3e_doc::{is_dynamic_node, nodes_related, terrain_height_at, Issue};
use r3e_physics::{BodyDesc, CastShape, ColliderShape, Physics};

use super::{Layer, Unchecked};

/// 贴地容差（米），与静态层初筛同口径。
///
/// 两层用同一个数不是为了好看：容差不同会出现「静态层说贴地、物理层说悬空 3 厘米」，
/// 而用户看到的只有一条结论，另一条会在去重时被吞掉，谁对谁错无从追查。
pub(super) const SUPPORT_TOLERANCE: f32 = 0.05;

/// 支撑探针的球半径。小到不会把邻座物件当支撑面，又大于数值噪声。
const PROBE_RADIUS: f32 = 0.02;

/// 相切不算穿模：重叠量必须大于这个才算「真的插进去了」。
const OVERLAP_EPS: f32 = 1e-3;

/// 世界基向量与坐标轴的最大偏差（含缩放归一化后）。超过即视为「带旋转」。
const AXIS_ALIGNED_EPS: f32 = 1e-4;

/// 一个「会进物理世界」的节点及其装配描述。
struct Body<'a> {
    /// 在 `scene.iter()` 里的序号，用于拼 `nodes[i]` 路径（与静态层同格式）。
    index: usize,
    node: &'a Node,
    desc: BodyDesc,
    center: Vec3,
}

impl Body<'_> {
    /// 装配实际用的半尺寸（球体退化成 `half.x` 三轴同值，见 `r3e_physics`）。
    fn half(&self) -> Vec3 {
        match self.desc.shape {
            ColliderShape::Sphere => Vec3::splat(self.desc.half_extents.x.abs()),
            _ => self.desc.half_extents,
        }
    }
}

/// 物理层总入口：悬空精确判定 + 穿模精确判定。
pub(super) fn check(
    scene: &Scene,
    physics: &Physics,
    out: &mut Vec<Issue>,
    unchecked: &mut Vec<Unchecked>,
) {
    check_with(scene, physics, None, out, unchecked);
}

/// 影子场景的物理层入口（M1-S4-G）：世界**没装配**时也要交出能交的结论。
///
/// 事务期没有对影子装配刚体的入口（`bind_scene` 要写句柄，而 `Document::write`
/// 归文档层所有），所以悬空判定拿不到「这一笔新放的物件有没有碰撞体」。这时候：
///
/// * 穿模（纯解析，不查世界）照跑——它本来就只看 `BodyDesc` 与包围盒；
/// * 悬空一律记 `unchecked` 并写清原因，**绝不**退化成「反正没报就是没悬空」。
///
/// `why` 由调用方给：原因可能是「拿不到网格来源」，也可能是「世界没跟着影子重建」，
/// 只有调用方知道自己为什么装配不了。
pub(super) fn check_unbound(
    scene: &Scene,
    physics: &Physics,
    why: &str,
    out: &mut Vec<Issue>,
    unchecked: &mut Vec<Unchecked>,
) {
    check_with(scene, physics, Some(why), out, unchecked);
}

/// 真正干活的那份：`unbound = None` 表示「世界与这份场景已对齐」。
fn check_with(
    scene: &Scene,
    physics: &Physics,
    unbound: Option<&str>,
    out: &mut Vec<Issue>,
    unchecked: &mut Vec<Unchecked>,
) {
    if !physics.enabled() {
        unchecked.push(Unchecked::new(
            Layer::Physics,
            "physics.enabled = false：没有物理世界可复核，悬空 / 穿模只有静态层的近似结论",
        ));
        return;
    }

    let bodies = collect(scene);
    let caps = physics.capabilities();

    match unbound {
        // 世界没装配：连「后端有没有 shape_cast」都不用问，直接记原因。
        Some(why) => unchecked.push(Unchecked::new(Layer::Physics, why.to_string())),
        None if caps.shape_cast => support(scene, physics, &bodies, out),
        None => unchecked.push(Unchecked::new(
            Layer::Physics,
            format!(
                "后端 `{}` 未申报 shape_cast：悬空复核跳过，静态层近似结论保留",
                physics.backend_name()
            ),
        )),
    }

    // 穿模判定是纯解析几何（不查后端），但需要知道有多少对被旋转挡在门外。
    let skipped = overlap(scene, &bodies, out);
    if skipped > 0 {
        unchecked.push(Unchecked::new(
            Layer::Physics,
            format!("{skipped} 对穿模候选带世界旋转：轴对齐解析式不适用，保留静态层近似结论"),
        ));
    }
}

/// 收集装配体：判据与 [`Physics::bind_scene`] 一致（有 `physics` 段且能推出 `BodyDesc`）。
fn collect(scene: &Scene) -> Vec<Body<'_>> {
    let mut bodies = Vec::new();
    for (index, node) in scene.iter().enumerate() {
        let Some(desc) = BodyDesc::from_node(node) else {
            // 有 `physics` 段但推不出描述（字段被清空 / 非法）：绑定时会被 `skipped`，
            // 物理层也就没有它的碰撞体，这里同样不收——由静态层报 bounds 问题。
            continue;
        };
        bodies.push(Body {
            index,
            node,
            desc,
            center: node.world.w_axis.truncate(),
        });
    }
    bodies
}

/// 悬空精确判定：向下 `shape_cast` 找支撑面。
///
/// 判据顺序有讲究——地面 / 地形先判，探针后发：
///
/// * 地面（`PhysicsConfig::ground`）与地形高度是**场景级真值**，不因有没有碰撞体而变；
/// * 探针从「底面下方一个容差」处向下扫，起点已在自身碰撞体之外（自身盒膨胀后底面
///   仍在起点上方），所以不会命中自己；
/// * 探针只扫一个容差深度，等于把静态层的「贴地」判据做成精确版，而不是「下方 2 米
///   内有东西就算站住了」——后者会把 1.9 米悬空的物件判成落地。
fn support(scene: &Scene, physics: &Physics, bodies: &[Body<'_>], out: &mut Vec<Issue>) {
    let floor = physics.ground();
    for body in bodies {
        // 动态体下一刻在哪由模拟决定，静态结论只会变噪声。
        if is_dynamic_node(body.node) {
            continue;
        }
        let half = body.half();
        let bottom = body.center.y - half.y;
        let terrain =
            terrain_height_at(scene, body.center.x, body.center.z).unwrap_or(f32::NEG_INFINITY);
        if bottom <= floor.max(terrain) + SUPPORT_TOLERANCE {
            continue;
        }

        let origin = Vec3::new(body.center.x, bottom - SUPPORT_TOLERANCE, body.center.z);
        let probe = physics.shape_cast(
            CastShape::sphere(PROBE_RADIUS),
            origin,
            Vec3::NEG_Y,
            SUPPORT_TOLERANCE * 2.0,
        );
        if probe.is_some() {
            continue;
        }

        out.push(
            Issue::warn(
                "support.floating",
                format!("nodes[{}]", body.index),
                format!(
                    "`{}` 底面在 y = {bottom:.3}，下方 {SUPPORT_TOLERANCE} 米内既没有地面 / 地形也没有碰撞体：物理层向下 shape_cast 无命中，确认为悬空",
                    body.node.name
                ),
            )
            .with_target(body.node.name.clone())
            .at([body.center.x, bottom, body.center.z])
            .with_hint("把节点下移到支撑面上、补一块地形 / 碰撞体，或把 body 声明为 dynamic 让它自己掉下去"),
        );
    }
}

/// 穿模精确判定：只对「双方都是装配体、都无世界旋转、形状有解析解」的对下结论。
///
/// 返回被旋转挡下的候选对数（组装层写进 `unchecked`，避免「没报 = 没问题」的误读）。
fn overlap(scene: &Scene, bodies: &[Body<'_>], out: &mut Vec<Issue>) -> usize {
    // 两两比较的上限：与静态层同规模（超出部分静态层的近似条目仍然保留）。
    const MAX_BODIES: usize = 512;

    let mut skipped_rotated = 0usize;
    let bodies = &bodies[..bodies.len().min(MAX_BODIES)];
    for (i, a) in bodies.iter().enumerate() {
        if is_dynamic_node(a.node) {
            continue;
        }
        for b in bodies.iter().skip(i + 1) {
            if is_dynamic_node(b.node) {
                continue;
            }
            // 父子体积重叠是层次结构的正常现象。
            if nodes_related(scene, a.node.id, b.node.id) {
                continue;
            }
            if !axis_aligned(a.node) || !axis_aligned(b.node) {
                skipped_rotated += 1;
                continue;
            }
            let Some(point) = penetrate(
                &a.desc.shape,
                a.half(),
                a.center,
                &b.desc.shape,
                b.half(),
                b.center,
            ) else {
                continue;
            };
            out.push(
                Issue::error(
                    "overlap.penetration",
                    format!("nodes[{}]", a.index),
                    format!(
                        "`{}` 与 `{}` 的碰撞体实际相交（重叠中心 {:.3}, {:.3}, {:.3}）：不是包围盒误报，物理层按装配形状解析判定",
                        a.node.name, b.node.name, point.x, point.y, point.z
                    ),
                )
                .with_target(format!("{}|{}", a.node.name, b.node.name))
                .at(point.to_array())
                .with_hint("挪开其中一件，或把其中一件改成装饰物（去掉 physics 段）"),
            );
        }
    }
    skipped_rotated
}

/// 解析相交：返回重叠中心；不相交返回 `None`。
///
/// `capsule` / `cylinder` 一律 `None`——没有解析式就不下结论（见模块头「只判能判死的」）。
fn penetrate(
    sa: &ColliderShape,
    ha: Vec3,
    ca: Vec3,
    sb: &ColliderShape,
    hb: Vec3,
    cb: Vec3,
) -> Option<Vec3> {
    match (sa, sb) {
        (ColliderShape::Sphere, ColliderShape::Sphere) => {
            let ra = ha.x;
            let rb = hb.x;
            let d = ca.distance(cb);
            (d < ra + rb - OVERLAP_EPS).then(|| {
                // 两球相交区域的重叠中心：沿连心线按半径比例取点。
                let t = if d <= 1e-6 { 0.5 } else { ra / (ra + rb) };
                ca.lerp(cb, t)
            })
        }
        (ColliderShape::Box, ColliderShape::Box) => box_box(ha, ca, hb, cb),
        (ColliderShape::Sphere, ColliderShape::Box) => sphere_box(ha.x, ca, hb, cb),
        (ColliderShape::Box, ColliderShape::Sphere) => sphere_box(hb.x, cb, ha, ca),
        _ => None,
    }
}

/// 盒-盒：三轴区间同时重叠才算（相切不算）。
fn box_box(ha: Vec3, ca: Vec3, hb: Vec3, cb: Vec3) -> Option<Vec3> {
    let lo = (ca - ha).max(cb - hb);
    let hi = (ca + ha).min(cb + hb);
    let depth = hi - lo;
    if depth.min_element() <= OVERLAP_EPS {
        return None;
    }
    Some((lo + hi) * 0.5)
}

/// 球-盒：把球心夹到盒面求最近点，比半径近才算。
fn sphere_box(radius: f32, center: Vec3, half: Vec3, box_center: Vec3) -> Option<Vec3> {
    let nearest = center.clamp(box_center - half, box_center + half);
    let d = center.distance(nearest);
    if d >= radius - OVERLAP_EPS {
        return None;
    }
    // 球心在盒内时最近点就是球心，重叠中心取球心与盒心的中点。
    Some(if d <= 1e-6 {
        (center + box_center) * 0.5
    } else {
        nearest
    })
}

/// 节点的世界旋转是否可忽略（三轴与世界基向量对齐）。
///
/// 看 `world` 而不是 `transform.rotation`：父级的旋转会通过矩阵传下来，只看局部四元数
/// 会漏掉「自己没转、爸妈转了」这种最常见的错位。
fn axis_aligned(node: &Node) -> bool {
    let axes = [
        (node.world.x_axis.truncate(), Vec3::X),
        (node.world.y_axis.truncate(), Vec3::Y),
        (node.world.z_axis.truncate(), Vec3::Z),
    ];
    axes.iter().all(|(axis, unit)| {
        let len = axis.length();
        len > 1e-6 && ((*axis / len) - *unit).abs().max_element() <= AXIS_ALIGNED_EPS
    })
}
