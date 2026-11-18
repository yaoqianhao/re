//! `scene.verify` 的宿主实现（M1-S4-F）：把三层验证器接上协议面。
//!
//! 本文件只做三件「协议 ↔ 内部」的事，判定一律不在这里：
//!
//! 1. **参数落地**：`VerifyTarget` 的坐标是否有限——协议层只管形状，语义错归宿主；
//! 2. **能力如实上报**：物理 / 导航拿不到就把原因写进 `unchecked`，**绝不**把
//!    「没检查」写成「通过」（`passed` 的第二个条件就是 `unchecked` 为空）；
//! 3. **投影成 JSON**：`Issue` 的字段与 `docs/capabilities.json` 里 `issueCodes`
//!    的登记层保持同一口径，前端拿 `layer` 就能把结论归到「几何 / 物理 / 导航」。
//!
//! 为什么不在宿主里**自动**装配物理：装配是**运行态的一部分**
//! （`r3e-runtime` 决定何时装、装哪一层），本命令顺手一装就等于在编辑态偷偷建世界——
//! 那会让「同一份场景在编辑器里体检通过、运行时崩」变成可能。所以这里只做
//! **新鲜度守卫**：世界里的刚体数与场景可装配体数对不上时，把物理层结论标成
//! 「可能过期」，并在 `reason` 里指路——调用方要可信结论，先显式调 `physics.bind`
//! 装配主世界（M1-S4-H）再重跑本命令，守卫自然消失（`docs/M1_S4F_PLAN.md` D2=P1）。

use super::tx::hex;
use super::*;
use crate::verify::{Layer, VerifyReport};
use r3e_core::Vec3;
use r3e_doc::{issue_code_by_code, Issue, Severity};

/// `scene.verify` 的宿主实现。
pub(super) fn scene_verify(
    api: &mut LocalEngineApi<'_>,
    query: VerifyQuery,
) -> Result<Value, RpcFailure> {
    let mut targets = Vec::with_capacity(query.targets.len());
    for (i, t) in query.targets.iter().enumerate() {
        if t.at.iter().any(|v| !v.is_finite()) {
            return Err(RpcFailure::invalid_params(format!(
                "参数 `targets[{i}].at` 必须全为有限数"
            )));
        }
        targets.push((t.name.clone(), Vec3::from_array(t.at)));
    }

    let doc = api.scene.borrow();
    let physics = api.physics.borrow();
    // 导航产物住宿主的槽位（不进 `Document`）：槽位没接 / 还没烘焙都算「没检查」，
    // 由 `verify::nav` 自己记 unchecked，这里只负责把 `Option<&Nav>` 传下去。
    let nav_slot = api.nav.map(|slot| slot.borrow());
    let nav = nav_slot.as_ref().and_then(|slot| slot.as_ref());

    let mut report = VerifyReport::collect_with_targets(&doc, &physics, nav, &targets);

    // 世界新鲜度守卫（D2=P1）：装配是运行态的事，宿主不代做。
    // 只在「物理开着但世界里的刚体数与场景对不上」时开口——否则每次编辑都在喊，
    // 喊久了就没人看 `unchecked` 了。
    if physics.enabled() {
        let expected = doc
            .iter()
            .filter(|node| r3e_physics::BodyDesc::from_node(node).is_some())
            .count();
        let actual = physics.body_count();
        if actual != expected {
            report.note_unchecked(
                Layer::Physics,
                format!(
                    "物理世界与场景不同步（世界 {actual} 个刚体 / 场景 {expected} 个可装配体）：\
                     宿主不自动装配，物理层结论可能过期；先调 `physics.bind` 装配主世界再重跑本命令"
                ),
            );
        }
    }

    let total = report.issues().len();
    let keep = query.limit.map_or(total, |n| n.min(total));
    let error_count = report.error_count();
    let warn_count = report.warn_count();
    let unchecked_empty = report.unchecked().is_empty();

    let mut by_layer = std::collections::BTreeMap::new();
    for issue in report.issues() {
        *by_layer.entry(layer_bucket(issue.code)).or_insert(0usize) += 1;
    }
    let bucket = |name: &str| by_layer.get(name).copied().unwrap_or(0);

    // 「跑过的层」：静态层永远跑（`r3e-doc` 不需要任何外部能力），物理 / 导航看
    // 有没有对应的 unchecked——有就是没跑成，不能算进 `layers`。
    let mut layers = vec![Layer::Static.as_str()];
    for layer in [Layer::Physics, Layer::Nav] {
        if !report.unchecked().iter().any(|u| u.layer() == layer) {
            layers.push(layer.as_str());
        }
    }

    Ok(json!({
        // `ok` 只说「没有 Error」；判门禁请看 `passed`。
        "ok": error_count == 0,
        "passed": error_count == 0 && unchecked_empty,
        "counts": {
            "error": error_count,
            "warn": warn_count,
            "byLayer": {
                "static": bucket("static"),
                "physics": bucket("physics"),
                "nav": bucket("nav"),
            },
        },
        "issues": report
            .issues()
            .iter()
            .take(keep)
            .map(issue_value)
            .collect::<Vec<_>>(),
        // 截断只截明细，不动计数：调用方看 `truncated` 决定要不要带 `limit` 重问。
        "truncated": keep < total,
        "unchecked": report
            .unchecked()
            .iter()
            .map(|u| json!({ "layer": u.layer().as_str(), "reason": u.reason() }))
            .collect::<Vec<_>>(),
        "layers": layers,
        "stateHash": hex(doc.state_hash()),
    }))
}

/// 一条 `Issue` 的出网形态。
///
/// `pub(super)`：`api/tx.rs` 的 `impact.violations` 复用同一个投影，避免同一件事
/// 在 `scene.verify` 与 `tx.intent` 两处出现两套字段名。
pub(super) fn issue_value(issue: &Issue) -> Value {
    json!({
        "code": issue.code,
        // 层用**登记表**里的那一份（`bounds` / `geom` / `support` / `overlap` /
        // `physics` / `nav`），不自造一套：`docs/capabilities.json` 的 `issueCodes`
        // 里已经公开了这层，回包换一套写法等于给同一件事两个名字。
        "layer": issue_code_by_code(issue.code).map_or("static", |c| c.layer),
        "severity": match issue.severity {
            Severity::Error => "error",
            Severity::Warn => "warn",
        },
        "category": issue.category.as_str(),
        "path": issue.path,
        "target": issue.target,
        "msg": issue.msg,
        "location": issue.location,
        "hint": issue.hint,
        // 近似 = 静态层的包围盒猜测。`false` 表示这一条背后有精确复核（物理 / 导航）。
        "approximate": issue.approximate,
    })
}

/// 登记层 → 三分类桶（回包 `counts.byLayer` 用）。
///
/// `support` / `overlap` 这两个登记层是**静态近似**的老名字（`*.suspect`）；
/// 它们的精确结论登记在 `physics` 层（`*.penetration` / `*.floating`），
/// 所以这里按「谁算出来的」而不是「叫什么」归类。
fn layer_bucket(code: &str) -> &'static str {
    match issue_code_by_code(code).map(|c| c.layer) {
        Some("physics") => "physics",
        Some("nav") => "nav",
        _ => "static",
    }
}
