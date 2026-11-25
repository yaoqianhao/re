//! 导航层校验（M1-S4-E）：出生点与目标点站不站得住、走不走得到。
//!
//! 静态层和物理层都只看「几何上有没有东西挡着」，它们看不见**导航数据**：一块斜坡在
//! 物理上完全合法，但没进 navmesh 就是走不上去的死区。本层拿 `r3e-nav` 的烘焙结果
//! 回答三个关卡作者真正关心的问题：
//!
//! 1. 出生点落在可走格上了吗（`nav.spawn_offmesh`）；
//! 2. 所有出生点在同一块连通区域里吗（`nav.region_split`）；
//! 3. 从出生点能走到要去的目标点吗（`nav.target_offmesh` / `nav.no_path`）。
//!
//! 另加两条**产物可信度**不变量（M2 · D04，`DD-5` 档 A）：
//!
//! 4. 烘出来的网格是不是一张空网格（`nav.bake.empty`）——`walkable == 0` 时
//!    上限不是「场景走不通」，而是「这份产物根本不能用」，必须比可达性先报；
//! 5. 这份网格是不是**当前后端**烘的（`nav.backend.mismatch`）——烘焙后端与校验
//!    后端不是同一个时，几何判定与导航判定描述了两个世界，产物不可信。
//!
//! **不可用就说不清**：宿主没接导航槽位、导航没烘焙、或烘焙出来一个可走格都没有时，
//! 本层一条结论都不下，只在 `unchecked` 里记原因。返回「全部通过」和「没法检查」
//! 是两件事，混在一起会让门禁变成绿灯装饰。

use r3e_core::{Scene, Vec3};
use r3e_doc::Issue;
use r3e_nav::{spawn_points, Nav};

use super::{Layer, Unchecked};

/// 给「最近可走点」建议用的吸附半径（米）。
const SNAP_RADIUS: f32 = 4.0;

/// 导航层总入口。`targets` 是「出生点能不能走到」的目标点清单（名字 + 世界坐标）。
pub(super) fn check(
    scene: &Scene,
    nav: Option<&Nav>,
    backend: Option<&str>,
    targets: &[(String, Vec3)],
    out: &mut Vec<Issue>,
    unchecked: &mut Vec<Unchecked>,
) {
    let Some(nav) = nav else {
        unchecked.push(Unchecked::new(
            Layer::Nav,
            "宿主未接导航槽位：出生点 / 可达性类结论全部跳过（不是通过）",
        ));
        return;
    };
    // 产物可信度先于可达性：一份空网格 / 换了后端的网格，后面每一条结论都建立在不成立的
    // 前提上，先报「产物不可信」再记「后续跳过」。
    backend_mismatch(nav, backend, out);
    if nav.report().walkable == 0 {
        out.push(
            Issue::error(
                "nav.bake.empty",
                "nav".to_string(),
                format!(
                    "导航网格 {}×{} 格（格长 {:.2} 米）里没有一个可走格：这份烘焙产物不可用",
                    nav.report().dims[0], nav.report().dims[1], nav.report().cell_size
                ),
            )
            .with_hint(
                "空网格多半来自盲目烘焙：先查那块地面有没有碰撞体、坡度是否超过上限、障碍是否铺满整片范围",
            ),
        );
        unchecked.push(Unchecked::new(
            Layer::Nav,
            "导航网格没有一个可走格：可达性类结论全部跳过（先查 bake 输入的碰撞体与坡度上限）",
        ));
        return;
    }

    let spawns = spawn_list(scene);
    if spawns.is_empty() {
        unchecked.push(Unchecked::new(
            Layer::Nav,
            "场景没有出生点：出生点相关结论跳过，目标点仍然检查可走性",
        ));
    } else {
        for (name, point) in &spawns {
            offmesh(nav, "nav.spawn_offmesh", name, *point, "出生点", out);
        }
        region_split(nav, &spawns, out);
    }

    for (name, point) in targets {
        if !offmesh(nav, "nav.target_offmesh", name, *point, "目标点", out) {
            continue;
        }
        for (spawn, from) in &spawns {
            unreachable(nav, spawn, *from, name, *point, out);
        }
    }
}

/// 这份网格是不是当前后端烘的（`nav.backend.mismatch`，M2 · D04）。
///
/// 判据落地口径见 `docs/m2/M2_D_PLAN.md` §3.8：`Nav` 在 `nav.bake` 成功时被宿主贴上
/// 当次物理后端名，这里拿它和**校验时**的后端名对账。名字对不上，说明这份产物出自
/// 另一套采样实现——几何层说「这块地站得住」，导航层说的却是另一个世界的地面。
///
/// `backend = None`（宿主没跑物理层）或 `nav.backend() = None`（`nav.import` 读回的
/// 产物、按场直接烘的产物）时**不下结论**：不知道来源不等于来源不一致，报红只会制造噪音。
fn backend_mismatch(nav: &Nav, backend: Option<&str>, out: &mut Vec<Issue>) {
    let (Some(baked), Some(current)) = (nav.backend(), backend) else {
        return;
    };
    if baked == current {
        return;
    }
    out.push(
        Issue::error(
            "nav.backend.mismatch",
            "nav".to_string(),
            format!(
                "这份导航网格由 `{baked}` 后端烘焙，当前校验用的是 `{current}` 后端：两侧的高度采样与形状判定不是同一套实现，逐格分类与可走性都不可比"
            ),
        )
        .with_hint(format!(
            "换用 `{baked}` 后端重跑校验，或在本后端下重新 `nav.bake` 再验"
        )),
    );
}

/// 出生点清单：优先场景声明的出生点；一个都没有时退到默认出生点（运行时实际用的那个）。
fn spawn_list(scene: &Scene) -> Vec<(String, Vec3)> {
    let declared = spawn_points(scene);
    if !declared.is_empty() {
        return declared;
    }
    r3e_nav::default_spawn_point(scene)
        .map(|p| vec![("<default>".to_string(), p)])
        .unwrap_or_default()
}

/// 点是否站得住；站不住时写一条 issue 并返回 `false`。
fn offmesh(
    nav: &Nav,
    code: &'static str,
    name: &str,
    point: Vec3,
    kind: &str,
    out: &mut Vec<Issue>,
) -> bool {
    if nav.is_walkable(point) {
        return true;
    }
    let hint = match nav.nearest_walkable(point, SNAP_RADIUS, None) {
        Some(q) => format!(
            "最近可走点在 ({:.2}, {:.2}, {:.2})，把{kind}挪过去即可",
            q.x, q.y, q.z
        ),
        None => format!(
            "{SNAP_RADIUS} 米内没有可走格：这一段多半根本没烘焙进导航网格，先查那块地面有没有碰撞体、坡度是否超限"
        ),
    };
    out.push(
        Issue::error(
            code,
            format!("spawns[{name}]"),
            format!(
                "{kind} `{name}` 位于 ({:.2}, {:.2}, {:.2})，不在任何可走格上：落地即在导航网格外或埋进几何",
                point.x, point.y, point.z
            ),
        )
        .with_target(name.to_string())
        .at(point.to_array())
        .with_hint(hint),
    );
    false
}

/// 出生点分散在多个互不连通的区域时给一条汇总警告（逐点报会把关卡作者淹掉）。
fn region_split(nav: &Nav, spawns: &[(String, Vec3)], out: &mut Vec<Issue>) {
    let mut ids: Vec<u32> = spawns
        .iter()
        .map(|(_, p)| nav.region_of(*p))
        .filter(|id| *id != 0)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 {
        return;
    }

    let regions = nav.regions();
    let detail: Vec<String> = ids
        .iter()
        .map(|id| {
            let cells = regions
                .iter()
                .find(|r| r.id == *id)
                .map(|r| r.cells)
                .unwrap_or(0);
            format!("#{id}({cells} 格)")
        })
        .collect();
    let names: Vec<&str> = spawns.iter().map(|(n, _)| n.as_str()).collect();
    let Some((_, first)) = spawns.first() else {
        return;
    };
    out.push(
        Issue::warn(
            "nav.region_split",
            "spawns[]".to_string(),
            format!(
                "出生点分布在 {} 个互不连通的导航区域：{}；出生点集 {} 里任意两点之间都走不到",
                ids.len(),
                detail.join(" / "),
                names.join(", ")
            ),
        )
        .with_target(names.join("|"))
        .at(first.to_array())
        .with_hint(
            "这些出生点之间的关卡路线不成立：要么补通道把区域连起来，要么删掉孤立区域的出生点",
        ),
    );
}

/// 从某个出生点走不到目标点时，报出「断在哪儿」的坐标。
fn unreachable(nav: &Nav, spawn: &str, from: Vec3, target: &str, to: Vec3, out: &mut Vec<Issue>) {
    if nav.reachable(from, to) {
        return;
    }
    let cut = nav.disconnect_point(from, to).unwrap_or(to);
    out.push(
        Issue::error(
            "nav.no_path",
            format!("spawns[{spawn}]"),
            format!(
                "从出生点 `{spawn}` 走不到目标点 `{target}`：连通性判定无路径，路径最远推进到 ({:.2}, {:.2}, {:.2})",
                cut.x, cut.y, cut.z
            ),
        )
        .with_target(format!("{spawn}>{target}"))
        .at(cut.to_array())
        .with_hint("缺口就在报出的坐标附近：补一段可走地面 / 坡道，或把目标点移回同一连通区域"),
    );
}
