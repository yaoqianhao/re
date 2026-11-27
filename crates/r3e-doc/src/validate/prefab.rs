//! Prefab 族静态校验（M2-E04）：`prefab.override_leak` 一个 code，三条触发面。
//!
//! # 为什么这条不变量必须存在
//!
//! E 阶段的架构约定是「实例只存差异」（`M2_E_PLAN` §9.4）：模板是唯一真源，
//! 实例文件里只该出现**与模板不同**的字段。可这条约定没写在任何类型里——
//! `ObjectOverride` 允许你把模板原值抄进覆盖，语义上完全合法（`Set` 就是 `Set`），
//! 只是把「只存差异」悄悄退化成「存快照」。退化之后：模板一改，实例因为抄了旧值
//! 而不跟随，外观「改了没变」，排查起来毫无线索。所以它需要一个能自动判定的落点。
//!
//! # 三条触发面（`M2_E_PLAN` §6.4）
//!
//! 1. 实例指向**不存在的模板 ID**——回不了表，实例就是一个装不出来的空壳；
//! 2. 覆盖里出现**与模板同值**的字段——把模板值写回了实例；
//! 3. 覆盖表整体退化成冗余快照——由 ② 逐字段判定后自然汇总，不单独成判据。
//!
//! # 为什么不引 `serde_json` 自己比 JSON
//!
//! `r3e-doc` 的依赖面只有 `r3e-core`（文档层不许反向依赖上层，见 `Cargo.toml` 注释），
//! 而 `PhysicsDesc` / `SurfaceMaterial` 也没有 `PartialEq`——逐字段 `==` 既借不到类型
//! 也借不到实现。真正的判据其实早就有了：E03 的 [`diff`] 定义就是「只记录与原型不同
//! 的字段」。于是「某字段是否与模板同值」等价于「把覆盖叠上去之后，`diff` 还认不认它
//! 是差异」——**复用同一份差异口径**，比另写一套 JSON 比较更不可能漂移（判据要的是
//! 「和 `diff` 一致」，不是「和某个第三实现一致」）。

use r3e_core::prefab::{apply, diff, ObjectOverride, OverrideState, PrefabInstance};
use r3e_core::{ObjectDesc, Scene};

use super::Issue;

/// 本族唯一的 code：覆盖泄漏（含指向不存在的模板）。
const OVERRIDE_LEAK: &str = "prefab.override_leak";

/// 扫描整份场景的 prefab 族问题。
///
/// **确定结论**（`approximate = false`）：模板存不存在、覆盖值与模板值是否相等，
/// 都能从场景内的真值算出来，没有更准的第二份口径去复核，因此不进近似覆盖规则。
pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    for (index, instance) in scene.instances.iter().enumerate() {
        let path = format!("instances[{index}]");
        // ① 模板必须在表里。回不了表的实例不是「待补」而是「装不出来」——
        //    `spawn_instance` 也会拒，但那是运行期一次一条，校验期要一次报全。
        let Some(template) = scene.prefab_handle(&instance.template) else {
            out.push(
                Issue::error(
                    OVERRIDE_LEAK,
                    path,
                    format!(
                        "实例 `{}` 引用的模板 `{}` 没登记：覆盖无从叠加，实例装不出来",
                        instance.name, instance.template
                    ),
                )
                .with_target(instance.name.clone())
                .with_hint("先用 prefab.define 登记模板，或把 instance.template 改成已存在的 ID"),
            );
            continue;
        };
        // ② 逐字段判「抄回模板值」。一次报一条、把字段名列全，避免同一实例刷出 12 条。
        let leaked = redundant_fields(&template.proto, instance);
        if leaked.is_empty() {
            continue;
        }
        out.push(
            Issue::error(
                OVERRIDE_LEAK,
                path,
                format!(
                    "实例 `{}` 的覆盖里有 {} 个字段与模板 `{}` 同值（{}）：等于把模板值抄回实例，\
                     模板再改也不会跟随",
                    instance.name,
                    leaked.len(),
                    instance.template,
                    leaked.join("、"),
                ),
            )
            .with_target(instance.name.clone())
            .with_hint("删掉这些覆盖（继承即取模板值）；只有真正不同的字段才该出现在 overrides 里"),
        );
    }
}

/// 覆盖里「与模板同值」的字段名（按 [`ObjectOverride::FIELD_NAMES`] 顺序，去重）。
///
/// 判据与 [`diff`] 同源：叠加覆盖后仍然算不出差异的字段，就是抄回来的模板值。
fn redundant_fields(proto: &ObjectDesc, instance: &PrefabInstance) -> Vec<&'static str> {
    // `apply` 会把名字换成实例名，但 `name` 不在可覆盖清单里，`diff` 也不比较它。
    let target = apply(proto, &instance.overrides, &instance.name);
    let minimal = diff(proto, &target);
    ObjectOverride::FIELD_NAMES
        .iter()
        .copied()
        .filter(|field| {
            // 只有 `Set` / `Clear` 是「覆盖了」；`Inherit` 本来就没抄。
            let overridden = matches!(
                instance.overrides.field_state(field),
                Some(OverrideState::Set | OverrideState::Clear)
            );
            // 真正必要的差异会留在 minimal 里；这里没有，说明它是模板值的副本。
            let still_differs = matches!(
                minimal.field_state(field),
                Some(OverrideState::Set | OverrideState::Clear)
            );
            overridden && !still_differs
        })
        .collect()
}
