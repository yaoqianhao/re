//! M2-E03 判据（doc 侧 2 条）：`DefinePrefab` 与 `SetNodePrefabOverride` 的 undo 前像。
//!
//! 对应 `docs/m2/M2_E_PLAN.md` §6.2 的判据⑧（模板撤销：`old = None` 删条目、
//! `old = Some` 写回旧模板）与判据⑫（覆盖撤销：前像写回后场景状态与执行前一致）。
//! 其余 10 条（①②③④⑤⑥⑦⑨⑩⑪）在 `crates/r3e-core/tests/prefab_diff.rs`。
//!
//! 本文件刻意**不引入 `serde_json`**：`r3e-doc` 的依赖表里没有它，而 E03 卡明确
//! 「不改 Cargo 依赖」。所以这里比重口径换成
//! ①`Document::state_hash()`（它就是场景落盘序列化结果的哈希，等价于字节相等）
//! ②逐字段比对（`proto.mesh` / `proto.visible` / `proto.tags`），够用来证明前像被写回。
//! 「一笔命令 = 一次 `apply`」的记账与 `push_group` 机制本卡不动。

use r3e_core::prefab::{FieldOverride, ObjectOverride, PrefabInstance, PrefabTemplate};
use r3e_core::{ObjectDesc, PhysicsDesc, Scene, SurfaceMaterial};
use r3e_doc::{CommandKind, Document};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 全 13 字段填满的原型（与 core 侧判据⑨⑩ 的夹具同源，便于横向对照）。
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

/// 表里某个模板的原型（模板本体不进场景文件，撤销是否写回只能看模板自己）。
fn template_proto<'a>(doc: &'a Document, id: &str) -> Option<&'a ObjectDesc> {
    doc.as_scene().prefabs.get(id).map(|handle| &handle.proto)
}

/// 表里某个模板的网格路径（用来判「替换成功」与「撤销写回」）。
fn template_mesh(doc: &Document, id: &str) -> Option<String> {
    template_proto(doc, id).and_then(|proto| proto.mesh.clone())
}

// ---------------------------------------------------------------------------
// 判据⑧：`DefinePrefab` 的 undo 前像
// ---------------------------------------------------------------------------

/// 判据⑧：`old = None` 时撤销要**删条目**；`old = Some` 时撤销要把旧模板**写回**。
#[test]
fn define_prefab_undo_restores_template_table() {
    let mut doc = Document::new(Scene::new());
    let proto = full_proto();
    let first = PrefabTemplate::new("prop.crate", proto.clone());
    let hash_empty = doc.state_hash();

    // 首次登记：前像 `old = None`。
    doc.apply(&CommandKind::DefinePrefab {
        next: first.clone(),
        old: None,
    })
    .expect("模板 ID 形状合法、表未满");
    assert!(
        template_proto(&doc, "prop.crate").is_some(),
        "apply 后模板应在表里"
    );

    doc.undo().expect("刚记了一笔，撤销栈非空");
    assert!(
        template_proto(&doc, "prop.crate").is_none(),
        "`old = None` 的撤销必须删掉条目（而不是留一条空模板）"
    );
    assert_eq!(doc.state_hash(), hash_empty, "撤销后哈希回到执行前");

    // 替换登记：前像 `old = Some(旧模板)`。
    doc.apply(&CommandKind::DefinePrefab {
        next: first.clone(),
        old: None,
    })
    .expect("重新登记");
    let mut retargeted = proto.clone();
    retargeted.mesh = Some("props/props_library/crate_small.glb".to_string());
    retargeted.visible = Some(false);
    let second = PrefabTemplate::new("prop.crate", retargeted);

    doc.apply(&CommandKind::DefinePrefab {
        next: second,
        old: Some(first),
    })
    .expect("同 ID 再登记是替换");
    assert_eq!(
        template_mesh(&doc, "prop.crate"),
        Some("props/props_library/crate_small.glb".to_string()),
        "替换必须真的改到模板，否则判据恒真"
    );
    assert_eq!(
        template_proto(&doc, "prop.crate").and_then(|proto| proto.visible),
        Some(false)
    );

    doc.undo().expect("撤销替换");
    assert_eq!(
        template_mesh(&doc, "prop.crate"),
        proto.mesh,
        "`old = Some` 的撤销必须把旧模板的网格写回"
    );
    assert_eq!(
        template_proto(&doc, "prop.crate").and_then(|proto| proto.visible),
        Some(true),
        "非网格字段也必须一并写回（前像是整份模板，不是单字段补丁）"
    );
    assert_eq!(
        template_proto(&doc, "prop.crate").and_then(|proto| proto.tags.clone()),
        proto.tags,
        "标签表同样要回到旧模板"
    );
}

// ---------------------------------------------------------------------------
// 判据⑫：`SetNodePrefabOverride` 的 undo 前像
// ---------------------------------------------------------------------------

/// 判据⑫：覆盖撤销后，场景状态哈希与执行前相等，实例覆盖回到前像。
#[test]
fn set_node_prefab_override_undo_restores_state() {
    let proto = full_proto();
    let mut scene = Scene::new();
    scene
        .define_prefab(PrefabTemplate::new("prop.crate", proto.clone()))
        .expect("模板 ID 形状合法、表未满");
    scene
        .spawn_instance(PrefabInstance::new("crate#1", "prop.crate"))
        .expect("模板已登记，实例名未占用");

    let mut doc = Document::new(scene);
    doc.mark_clean();
    let hash_before = doc.state_hash();

    // 前像 = 当前覆盖（空），后像 = 一条 `Set` + 一条 `Clear`（三态都过一遍命令层）。
    let old = doc
        .as_scene()
        .instance("crate#1")
        .expect("实例记录已登记")
        .overrides
        .clone();
    assert!(old.is_empty(), "初始实例不该带覆盖");

    let new = ObjectOverride {
        visible: Some(FieldOverride::set(false)),
        tags: Some(FieldOverride::clear()),
        ..ObjectOverride::default()
    };

    doc.apply(&CommandKind::SetNodePrefabOverride {
        name: "crate#1".to_string(),
        old: old.clone(),
        new: new.clone(),
    })
    .expect("实例记录存在");

    // 写入事实的判据落在实例记录上，**不是**落在 `state_hash` 的变化上：
    // `state_hash` 的遍历表（`state_hash.rs` 文件头「覆盖 / 不覆盖」两张表）目前还没有
    // `instances` 段，P3 的「实例段入 hash」尚未兑现。补 hash 不在本卡文件族内
    // （E03 只动 `prefab/diff.rs` + `command/mod.rs`），已在 M2_E_PLAN §6.5 实施记录
    // 里作为口径差异留痕，交给后续补丁处理。因此这里只断言「前像 → 后像 → 前像」两端一致。
    assert_eq!(
        doc.as_scene()
            .instance("crate#1")
            .expect("实例记录仍在")
            .overrides
            .count(),
        2,
        "两条覆盖都要落到实例记录上（`Clear` 也算一条）"
    );
    // 本命令只改实例记录：节点上的可见性要等宿主重算（E04），这里必须还没被动过。
    assert_eq!(
        doc.as_scene()
            .find_id("crate#1")
            .and_then(|id| doc.as_scene().get(id))
            .map(|node| node.visible),
        Some(true),
        "本命令只改实例记录，不动节点（节点重算归 E04 宿主）"
    );

    doc.undo().expect("撤销这一笔");
    assert_eq!(doc.state_hash(), hash_before, "撤销后场景状态回到执行前");
    let restored = &doc
        .as_scene()
        .instance("crate#1")
        .expect("实例记录仍在")
        .overrides;
    assert!(restored.is_empty(), "撤销后覆盖必须回到前像（空）");
    assert!(
        restored.visible.is_none() && restored.tags.is_none(),
        "前像写回后不应残留任何字段的覆盖态"
    );
}
