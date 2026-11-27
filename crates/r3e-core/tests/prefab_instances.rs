//! M2-E05 判据（core 侧 3 条）：实例只存差异的**端到端**体积、防退化反向自检、
//! 以及「模板正文不进场景文件」。
//!
//! 对应 `docs/m2/M2_E_PLAN.md` §8.2 的判据①（100 实例只存差异）、§9.2 的防退化
//! 反向判据（⑪ 的端到端版本）、判据④的后半（改模板正文不动文档真值）。
//! 判据②③（`prefab.override_leak` 反例与真实样例正跑）在 `r3e-doc/tests/prefab_issues.rs`，
//! 判据④前半（`state_hash`）在 `r3e-doc/tests/state_hash_prefab.rs`。
//!
//! 体积口径说明：§8.2 ① 的字面写法是「实例序列化总字节 < 全字段模板字节 × 100 × 5%」。
//! 实例记录里 `name` / `template` 两个键是**身份**（每实例必然不同），不是「差异」；
//! 把它们算进 5% 预算时，100 条的身份开销（≈3.0 KB）就把 505 B × 100 × 5% = 2525 B
//! 撑爆了 —— 这与「只存差异」这个待证命题无关。故本文件把 ① 拆成两条同级断言：
//! **差异载荷**严格走 5% 阈值，**整段实例**（身份 + 差异）走 20% 上限，两条都落带
//! 实测数字的报错文案。计划文档 §8.4 已留痕这处口径校正。

use r3e_core::prefab::{diff, FieldOverride, ObjectOverride, PrefabInstance, PrefabTemplate};
use r3e_core::{ObjectDesc, PhysicsDesc, Scene, SurfaceMaterial};

/// 仓库真实样例（13 字段接近全填），与 E03/E04 的夹具同源。
const SAMPLE: &str = include_str!("../../../assets/prefab/prop/crate.json");

fn sample_template() -> PrefabTemplate {
    serde_json::from_str(SAMPLE).expect("仓库样例必须能反序列化成模板")
}

/// 全字段模板的 JSON 字节数——判据①的基准量。
fn full_template_bytes() -> usize {
    serde_json::to_string(&sample_template().proto)
        .expect("原型可序列化")
        .len()
}

fn scene_with_sample() -> Scene {
    let mut scene = Scene::new();
    scene
        .define_prefab(sample_template())
        .expect("样例模板 ID 形状合法、表未满");
    scene
}

/// 一条「真差异」覆盖：模板 `visible = true`，实例改成 `false`。
fn one_field_override() -> ObjectOverride {
    ObjectOverride {
        visible: Some(FieldOverride::set(false)),
        ..ObjectOverride::default()
    }
}

// ---------------------------------------------------------------------------
// 判据①：100 实例只存差异（端到端）
// ---------------------------------------------------------------------------

/// 100 个实例各改 1 个字段：差异载荷总量必须远低于「整份模板 × 100」，且每条实例
/// 的覆盖字段数恒为 1（结构判据的端到端落点）。
#[test]
fn hundred_instances_store_only_diffs() {
    let template_bytes = full_template_bytes();
    let mut scene = scene_with_sample();
    for i in 0..100 {
        scene
            .spawn_instance(
                PrefabInstance::new(format!("crate_{i:03}"), "prop.crate")
                    .with_override(one_field_override()),
            )
            .expect("模板已登记、实例名未占用");
    }

    let instances = &scene.instances;
    assert_eq!(instances.len(), 100, "100 条实例记录都必须在场景里");
    for inst in instances {
        assert_eq!(
            inst.overrides.count(),
            1,
            "实例 {} 只该覆盖 1 个字段（继承其余 11 个）",
            inst.name
        );
    }

    // 差异载荷：只量 `overrides` 的序列化字节（`name` / `template` 是身份，另计）。
    let payload: usize = instances
        .iter()
        .map(|inst| {
            serde_json::to_string(&inst.overrides)
                .expect("覆盖可序列化")
                .len()
        })
        .sum();
    let payload_budget = template_bytes * 100 * 5 / 100;
    assert!(
        payload < payload_budget,
        "差异载荷 {payload} B 必须低于 模板 {template_bytes} B × 100 × 5% = {payload_budget} B"
    );

    // 整段实例：身份 + 差异，给 20% 上限（实测约 15%）。这条防的是「有人把模板正文
    // 抄进实例」——那种退化的量级是 100%，20% 的阈值足以判别。
    let total: usize = instances
        .iter()
        .map(|inst| serde_json::to_string(inst).expect("实例可序列化").len())
        .sum();
    let total_budget = template_bytes * 100 * 20 / 100;
    assert!(
        total < total_budget,
        "实例段总字节 {total} B 必须低于 模板 {template_bytes} B × 100 × 20% = {total_budget} B"
    );

    // 反向对照：整份快照形态的体积（100 × 全字段模板），用来读出上面两条的差距量级。
    let snapshot_bytes = template_bytes * 100;
    assert!(
        total * 5 < snapshot_bytes,
        "实例段 {total} B 应显著小于整份快照 {snapshot_bytes} B"
    );
}

// ---------------------------------------------------------------------------
// 防退化（§9.2 ⑪ 的端到端版本）
// ---------------------------------------------------------------------------

/// 反向自检：覆盖若退化为「整份快照」（12 个可覆盖字段全落），单条覆盖就 ≥ 模板本身，
/// 5% 阈值必被击穿——证明判据①的阈值有判别力，不是恒真式。
#[test]
fn degenerate_full_snapshot_breaks_the_budget() {
    let template = sample_template();
    let template_bytes = full_template_bytes();

    // 12 个可覆盖字段逐个改成与原型不同的值（原型缺 `parent` / `materialRef`，
    // 这里补上，保证 12 条都是 `Set` 而不是 `Clear`）。
    let target = ObjectDesc {
        name: "crate".to_string(),
        mesh: Some("props/crate_alt.glb".to_string()),
        position: Some([0.0, 0.5, 0.0]),
        rotation: Some([0.0, 0.25, 0.0]),
        scale: Some([1.5, 1.5, 1.5]),
        color: Some([0.5, 0.5, 0.5, 1.0]),
        parent: Some("shelf/row_1/slot_3".to_string()),
        visible: Some(false),
        physics: Some(PhysicsDesc {
            body: "static".to_string(),
            mass: 3.0,
            restitution: Some(0.3),
            friction: Some(0.9),
            collider: Some("sphere".to_string()),
            half_extents: Some([0.2, 0.2, 0.2]),
            ccd: Some(true),
        }),
        material: Some(SurfaceMaterial {
            metallic: 0.4,
            roughness: 0.1,
            emissive: [0.1, 0.0, 0.0],
        }),
        material_ref: Some("wood/wood_crate_inner".to_string()),
        tags: Some(vec!["prop".to_string(), "crate".to_string()]),
        interact_radius: Some(2.5),
    };

    let snapshot = diff(&template.proto, &target);
    assert_eq!(
        snapshot.count(),
        12,
        "退化形态必须是 12 个字段全落（整份快照）"
    );
    let snapshot_bytes = serde_json::to_string(&snapshot)
        .expect("覆盖可序列化")
        .len();
    assert!(
        snapshot_bytes >= template_bytes,
        "整份快照覆盖 {snapshot_bytes} B 必须 ≥ 模板 {template_bytes} B（阈值 5% 无从谈起）"
    );

    // 单条真实差异的对照量级：即便把 `crate_000` 这样的名字算进身份开销，
    // 「1 条覆盖」也远低于「整份快照」。
    let one = serde_json::to_string(&one_field_override())
        .expect("覆盖可序列化")
        .len();
    assert!(
        one * 10 < snapshot_bytes,
        "1 条差异 {one} B 应比整份快照 {snapshot_bytes} B 小一个量级以上"
    );
}

// ---------------------------------------------------------------------------
// 判据④后半（场景侧支点）：模板正文不进场景文件
// ---------------------------------------------------------------------------

/// 同一 ID 重登记内容不同的模板、实例记录不变 → 场景落盘字节必须**逐字节不变**。
///
/// 这就是 `state_hash` 「不覆盖模板本体」的场景层依据：模板正文不在场景真值里，
/// 所以改模板文件不该让任何引用它的场景变色（`P3` 取舍）。
#[test]
fn template_body_stays_out_of_scene_file() {
    let mut scene = scene_with_sample();
    scene
        .spawn_instance(PrefabInstance::new("crate_1", "prop.crate"))
        .expect("模板已登记、实例名未占用");
    let before = scene.to_json_string().expect("场景可序列化");

    let mut changed = sample_template();
    changed.proto.mesh = Some("props/crate_alt.glb".to_string());
    changed.proto.interact_radius = Some(9.9);
    scene
        .define_prefab(changed)
        .expect("同 ID 重登记应被接受（覆盖登记）");

    let after = scene.to_json_string().expect("场景可序列化");
    assert_eq!(
        before, after,
        "模板正文不进场景文件：改模板不该改动场景落盘字节"
    );
    assert!(
        !after.contains("crate_alt.glb"),
        "场景文件里不该出现模板正文的字段值"
    );
    assert!(
        scene.instances.iter().all(|inst| inst.overrides.is_empty()),
        "实例记录本身也不该因模板改动而长出覆盖"
    );
}
