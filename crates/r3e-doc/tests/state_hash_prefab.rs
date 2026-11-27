//! M2-E05 判据④（doc 侧 2 条）：`state_hash` 的 Prefab 实例段口径。
//!
//! 对应 `docs/m2/M2_E_PLAN.md` §8.2 ④ 的两个方向：
//! - **改实例覆盖 → hash 变**：实例记录是文档真值，AI 写入必须能被差分哈希看见；
//! - **只改模板正文（实例不变）→ hash 不变**：模板是被引用物，不是实例真值，
//!   否则改一次模板文件会让所有引用它的场景集体变色（`P3` 取舍）。
//!
//! 本文件刻意**不引入 `serde_json`**（`r3e-doc` 依赖表里没有它，E03 卡已定「不改
//! Cargo 依赖」），比较口径一律用 `Document::state_hash()` —— 它就是对场景落盘
//! 序列化结果做 FNV-1a 的指纹，等价于「字节相等」的快速判据。

use r3e_core::prefab::{FieldOverride, ObjectOverride, PrefabInstance, PrefabTemplate};
use r3e_core::{ObjectDesc, PhysicsDesc, Scene, SurfaceMaterial};
use r3e_doc::{CommandKind, Document};

/// 全 13 字段填满的原型（与 E03 `prefab_command.rs` 的夹具同源）。
fn full_proto() -> ObjectDesc {
    ObjectDesc {
        name: "crate".to_string(),
        mesh: Some("props/props_library/crate_large_wooden_v2.glb".to_string()),
        position: Some([0.0, 0.4, 0.0]),
        rotation: Some([0.0, 0.0, 0.0]),
        scale: Some([1.0, 1.0, 1.0]),
        color: Some([0.72, 0.55, 0.34, 1.0]),
        parent: Some("shelf/shelf_row_1/slot_3".to_string()),
        visible: Some(true),
        physics: Some(PhysicsDesc {
            body: "dynamic".to_string(),
            mass: 12.0,
            restitution: Some(0.15),
            friction: Some(0.6),
            collider: Some("box".to_string()),
            half_extents: Some([0.4, 0.4, 0.4]),
            ccd: Some(false),
        }),
        material: Some(SurfaceMaterial {
            metallic: 0.0,
            roughness: 0.62,
            emissive: [0.0, 0.0, 0.0],
        }),
        material_ref: Some("wood/wood_crate_outer_v2".to_string()),
        tags: Some(vec!["prop".to_string(), "crate".to_string()]),
        interact_radius: Some(1.2),
    }
}

/// 一个模板 + 一个实例的场景（实例可带覆盖）。
fn doc_with_instance(overrides: ObjectOverride) -> Document {
    let mut scene = Scene::new();
    scene
        .define_prefab(PrefabTemplate::new("prop.crate", full_proto()))
        .expect("模板 ID 形状合法、表未满");
    scene
        .spawn_instance(PrefabInstance::new("crate#1", "prop.crate").with_override(overrides))
        .expect("模板已登记、实例名未占用");
    Document::new(scene)
}

// ---------------------------------------------------------------------------
// 判据④前半：改实例覆盖 → hash 变
// ---------------------------------------------------------------------------

/// 覆盖是文档真值：同场景重复取哈希稳定，改动任一覆盖（含 `Set → Clear` 的态变）
/// 都必须改变哈希——四种形态两两不同。
#[test]
fn instance_override_change_moves_hash() {
    let none = doc_with_instance(ObjectOverride::default());
    let set_false = doc_with_instance(ObjectOverride {
        visible: Some(FieldOverride::set(false)),
        ..ObjectOverride::default()
    });
    let set_true = doc_with_instance(ObjectOverride {
        visible: Some(FieldOverride::set(true)),
        ..ObjectOverride::default()
    });
    let cleared = doc_with_instance(ObjectOverride {
        visible: Some(FieldOverride::clear()),
        ..ObjectOverride::default()
    });

    assert_eq!(
        none.state_hash(),
        none.state_hash(),
        "同一文档两次取哈希必须相等（缓存不得引入抖动）"
    );

    let h_none = none.state_hash();
    let h_false = set_false.state_hash();
    let h_true = set_true.state_hash();
    let h_clear = cleared.state_hash();

    assert_ne!(h_none, h_false, "无覆盖 → `Set(false)` 必须改变哈希");
    assert_ne!(h_false, h_true, "`Set(false)` → `Set(true)` 必须改变哈希");
    assert_ne!(
        h_false, h_clear,
        "`Set(false)` → `Clear` 必须改变哈希（清空是动作，不是「继承」）"
    );
    assert_ne!(
        h_none, h_clear,
        "无覆盖 → `Clear` 必须改变哈希（`null` 与键缺失不同态）"
    );
}

// ---------------------------------------------------------------------------
// 判据④后半：只改模板正文（实例不变）→ hash 不变
// ---------------------------------------------------------------------------

/// 模板正文不在场景真值里：同 ID 重登记内容不同的模板，实例记录一字未动，
/// 哈希必须纹丝不动。
#[test]
fn template_body_change_keeps_hash() {
    let mut doc = doc_with_instance(ObjectOverride::default());
    let hash_before = doc.state_hash();

    // 模板正文换掉（网格与质量都变），实例记录不变。
    let mut changed = full_proto();
    changed.mesh = Some("props/props_library/crate_alt_v9.glb".to_string());
    if let Some(physics) = changed.physics.as_mut() {
        physics.mass = 99.0;
    }
    let old = doc
        .as_scene()
        .prefabs
        .get("prop.crate")
        .map(|handle| (**handle).clone());
    doc.apply(&CommandKind::DefinePrefab {
        next: PrefabTemplate::new("prop.crate", changed),
        old,
    })
    .expect("同 ID 重登记应被接受（走 DefinePrefab 的唯一写通道）");

    assert_eq!(
        doc.state_hash(),
        hash_before,
        "模板正文是「不覆盖」项：改模板不得让引用了它的场景哈希变色"
    );
    assert!(
        doc.as_scene()
            .instance("crate#1")
            .expect("实例记录仍在")
            .overrides
            .is_empty(),
        "模板改动不该让实例长出覆盖"
    );
}
