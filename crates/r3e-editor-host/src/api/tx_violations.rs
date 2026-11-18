//! 事务回包里的 `impact.violations`：一层一层跑、只报「这一笔新引入」的结论。
//!
//! 拆出 `tx.rs` 的原因只有行数：意图解析与差集采集是两件事，混在一个文件里
//! 谁都不好读（`xtask lint-size` 也是这么要求的）。

use super::*;
use r3e_doc::Issue;

use super::tx::Impact;

use crate::verify::{NavProbe, PhysicsProbe, VerifyReport};

/// 事务里「这一笔**新引入**」的三层违规（静态层 M1-S4-F，物理 / 导航层 M1-S4-G）。
///
/// 口径四条：
///
/// 1. **三层都跑**，但每层都按「能不能代表影子场景」显式选姿态（[`PhysicsProbe`] /
///    [`NavProbe`]），判不了的层把原因写进 `unchecked`，绝不当成通过。
/// 2. **基线取主场景，不取空集**。事务期主场景零写入（命令全部落在影子上），所以
///    主场景的三层结果就是 `tx.begin` 那一刻的快照。只报差集，「本来就有的违规」
///    才不会每条意图重报一遍。
/// 3. **键与去重同口径**：`(code, target, path)`——同一对象同一条码视为同一条问题。
/// 4. **`layers` 只列真下过结论的层，`notChecked` 列一层都没跑成的层**：AI 要能一眼
///    分清「这段场景确实没新问题」与「这层压根没检查」。
pub(super) fn introduced_violations(api: &LocalEngineApi<'_>) -> Value {
    let doc = api.scene.borrow();
    let Some(session) = doc.tx_session() else {
        return no_violations();
    };
    let after = probe_layers(api, session.shadow());
    let base = probe_layers(api, &doc);

    let mut known: std::collections::HashSet<(&str, Option<&str>, &str)> =
        std::collections::HashSet::with_capacity(base.issues.len());
    for issue in &base.issues {
        known.insert(issue_key(issue));
    }
    let fresh: Vec<&Issue> = after
        .issues
        .iter()
        .filter(|issue| !known.contains(&issue_key(issue)))
        .collect();

    // 一层都没跑成的：`checked` 里没有它，这里必须出现，否则读起来像「查过没问题」。
    let not_checked: Vec<&'static str> = after
        .unchecked
        .iter()
        .filter(|(layer, _)| !after.checked.contains(layer))
        .map(|(layer, _)| *layer)
        .collect();

    json!({
        "count": fresh.len(),
        "layers": after.checked,
        "issues": fresh
            .iter()
            .map(|issue| super::verify::issue_value(issue))
            .collect::<Vec<_>>(),
        "notChecked": not_checked,
        // 逐条原因：`notChecked` 只说「哪层没跑」，这里说「为什么、哪一段没跑」。
        "unchecked": after
            .unchecked
            .iter()
            .map(|(layer, reason)| json!({ "layer": layer, "reason": reason }))
            .collect::<Vec<_>>(),
    })
}

/// 差集键：同一对象 + 同一条码 = 同一条问题（跨层去重也用这套口径）。
fn issue_key(issue: &Issue) -> (&'static str, Option<&str>, &str) {
    (issue.code, issue.target.as_deref(), issue.path.as_str())
}

/// 一次「按层复核」的产出：结论 + 谁真跑过 + 谁没跑成及原因。
struct LayerProbe {
    issues: Vec<Issue>,
    /// 真下过结论的层（`static` / `physics` / `nav` 的子集，按层序）。
    checked: Vec<&'static str>,
    /// 没跑成 / 只跑了一部分的层与原因，顺序即发现顺序。
    unchecked: Vec<(&'static str, String)>,
}

/// 对一个场景跑三层复核（M1-S4-G）。
///
/// **影子场景与主场景走同一份代码**：两条路径的差异只应该在「姿态」，不该在
/// 「判定口径」，否则差集里会出现「主场景用精确、影子用近似」这种口径漂移。
fn probe_layers(api: &LocalEngineApi<'_>, scene: &Scene) -> LayerProbe {
    // 世界矩阵：影子从没跑过一帧，主场景在 headless 下也未必刷过。判定读 `node.world`，
    // 不刷就会把新放的节点一律当成「还在原点」，凭空报穿模 / 悬空。
    let mut probe = scene.clone();
    probe.update_world_transforms();
    let scene = &probe;

    let mut checked: Vec<&'static str> = vec!["static"];
    let mut extra: Vec<(&'static str, String)> = Vec::new();

    // ---- 物理层：优先按这份场景重建一个临时世界；重建不了就只出解析结论。
    let physics = api.physics.borrow();
    // 只在这一份场景「能装配」时才建世界：`bind_scene` 需要一个可用的形状来源。
    let world = if physics.enabled() {
        api.meshes.as_ref().map(|meshes| {
            let mut fresh = shadow_world(&physics);
            // 句柄丢弃：这一份世界只用来做 `shape_cast` 查询，不参与模拟、不回写场景。
            fresh.bind_scene(scene, meshes.as_ref());
            fresh
        })
    } else {
        None
    };
    let physics_probe = match world.as_ref() {
        Some(fresh) => PhysicsProbe::Bound(fresh),
        // 世界没建：要么物理本来就关着（交给本层记标准原因），要么缺网格来源。
        None if !physics.enabled() => PhysicsProbe::Bound(&physics),
        None => PhysicsProbe::Unbound(
            &physics,
            "宿主没拿到网格来源（`EngineAccess::meshes` 为空）：影子世界装配不了，\
             悬空判定跳过，穿模只出纯解析结论"
                .to_string(),
        ),
    };
    if physics.enabled() {
        // 解析式穿模不依赖世界，世界装配不了时这一层仍算「跑过」（部分指标在 unchecked 里）。
        checked.push("physics");
    }

    // ---- 导航层：影子必须**当场烘**，主场景那格烘焙结果对影子前提不成立。
    let slot_filled = matches!(api.nav, Some(slot) if slot.borrow().is_some());
    let mut baked: Option<r3e_nav::Nav> = None;
    let mut skip_reason: Option<String> = None;
    if api.nav.is_some() && !slot_filled {
        skip_reason = Some(
            "主场景还没烘焙导航（先 nav.bake）：没有可比对的基线，影子的可走性 / 可达性未查"
                .to_string(),
        );
    } else if slot_filled {
        match r3e_nav::Nav::bake_scene(scene) {
            Ok(nav) => {
                baked = Some(nav);
                checked.push("nav");
            }
            Err(err) => {
                skip_reason = Some(format!(
                    "影子的导航烘焙失败（{err:?}）：可走性 / 可达性未查，不代表通"
                ));
            }
        }
    }
    let nav_probe = match (baked.as_ref(), skip_reason) {
        (Some(nav), _) => NavProbe::Run(Some(nav)),
        (None, Some(why)) => NavProbe::Skip(why),
        // 宿主压根没接导航槽位：交给本层记标准原因（与 `scene.verify` 口径一致）。
        (None, None) => NavProbe::Run(None),
    };

    let report = VerifyReport::collect_with(scene, Some(physics_probe), nav_probe, &[]);
    for unchecked in report.unchecked() {
        extra.push((unchecked.layer().as_str(), unchecked.reason().to_string()));
    }
    // 事务不带目标点：`nav.no_path` 这一档在本接口里必然没查，如实记账。
    if checked.contains(&"nav") {
        extra.push((
            "nav",
            "tx.intent 不带目标点：`nav.no_path`（目标可达性）未查，只查了出生点与连通性"
                .to_string(),
        ));
    }
    let issues = report.issues().to_vec();
    LayerProbe {
        issues,
        checked,
        unchecked: extra,
    }
}

/// 造一个「与主世界同配置、但不带任何旧刚体」的临时世界。
///
/// 只借**地面高度**：它参与悬空判据（`physics.ground()` 是场景级真值，装配体下方
/// 有没有地板跟物理开关、后端种类无关）。重力不搬——本世界只做 `shape_cast` /
/// overlap 查询，不跑模拟，重力既不改变「此刻有没有东西挡着」，也不该被顺手改写。
fn shadow_world(source: &r3e_physics::Physics) -> r3e_physics::Physics {
    let mut world = r3e_physics::Physics::with_backend(Box::new(r3e_physics::BuiltinBackend::new(
        r3e_core::Vec3::ZERO,
        source.ground(),
    )));
    world.set_ground(source.ground());
    world
}

/// 没有事务在跑时的 `violations`：三层全没查，结构恒定（五个键都输出）。
///
/// 这里**不能**沿用「`layers: ["static"]` + 空 issues」的老形状：那是「查过、没问题」
/// 的样子，而此刻连差集基线都取不到，读起来会骗人。
fn no_violations() -> Value {
    json!({
        "count": 0,
        "layers": [],
        "issues": [],
        "notChecked": ["static", "physics", "nav"],
        "unchecked": [{
            "layer": "static",
            "reason": "当前没有进行中的事务：没有影子场景可比，「这一笔新引入」无从谈起",
        }],
    })
}

/// 把 [`Impact`] 与「这笔意图新引入的三层违规」拼成回包里的 `impact`。
///
/// `violations` 挂在 `impact` 下而不是回包顶层：它回答的是同一个问题——
/// 「这一笔到底动了什么、动出了什么后果」，AI 读一处就够，不必记两个位置。
pub(super) fn impact_with_violations(api: &LocalEngineApi<'_>, impact: Impact) -> Value {
    let mut value = impact.to_json();
    let violations = introduced_violations(api);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("violations".to_string(), violations);
    }
    value
}
