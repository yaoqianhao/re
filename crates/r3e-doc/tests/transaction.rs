//! 事务（S4-02）验收：preview 零副作用、原子回退、版本粒度。
//!
//! 全部走公开 API。借用约束：`Transaction` 独占 `&mut Document`，所以事务期间的读取
//! 一律走 `tx.document()` 投影（这也是上层真实用法），命令要么在 `begin` 前备好，
//! 要么用 `tx.stage(|scene| ...)` 现造。

use r3e_core::{ObjectDesc, Scene, Transform, Vec3};
use r3e_doc::{CommandKind, DocError, Document, Transaction};

/// 三个节点的干净文档（a / b / c，初始变换为 IDENTITY），dirty 已清。
fn seeded() -> Document {
    let mut doc = Document::new(Scene::new());
    doc.write(|s| {
        for name in ["a", "b", "c"] {
            s.spawn(ObjectDesc {
                name: name.to_string(),
                ..ObjectDesc::default()
            })
            .unwrap();
        }
    });
    doc.mark_clean();
    doc
}

/// 冻结状态下造一条改变换命令（前像从给定场景抓）。
fn transform_to(doc: &Document, name: &str, x: f32) -> CommandKind {
    let id = doc.find_id(name).expect("节点存在");
    let old = doc.get(id).expect("节点存在").transform;
    CommandKind::SetTransform {
        id,
        old,
        new: Transform::from_position(Vec3::new(x, 0.0, 0.0)),
    }
}

fn hide(doc: &Document, name: &str) -> CommandKind {
    let id = doc.find_id(name).expect("节点存在");
    CommandKind::SetVisible {
        id,
        old: true,
        new: false,
    }
}

/// 事务内「造 + 执行」：前像从当前（含已暂存命令的）状态抓。
fn stage_move(tx: &mut Transaction<'_>, name: &str, x: f32) -> Result<(), DocError> {
    tx.stage(|s| {
        let id = s
            .find_id(name)
            .ok_or_else(|| DocError::NotFound(name.to_string()))?;
        let old = s.get(id).expect("节点存在").transform;
        Ok(CommandKind::SetTransform {
            id,
            old,
            new: Transform::from_position(Vec3::new(x, 0.0, 0.0)),
        })
    })
}

/// 指向不存在节点的命令：`apply` 必失败（`NotFound`）。
fn dangling() -> CommandKind {
    CommandKind::SetVisible {
        id: u64::MAX,
        old: false,
        new: true,
    }
}

#[test]
fn preview_is_side_effect_free() {
    let mut doc = seeded();
    let cmds = vec![transform_to(&doc, "a", 3.0), hide(&doc, "b")];
    let before_hash = doc.state_hash();
    let before_version = doc.version();
    let before_writes = doc.write_count();
    let before_depth = doc.undo_depth();

    let tx = doc.begin();
    let previewed = tx.preview(&cmds).expect("预演应当成功");

    assert_ne!(previewed, before_hash, "预演应当算出不同的新状态");
    assert_eq!(
        tx.document().state_hash(),
        before_hash,
        "预演不得改动主文档"
    );
    assert_eq!(tx.document().version(), before_version, "预演不得推进版本");
    assert_eq!(
        tx.document().write_count(),
        before_writes,
        "预演不得算一次写"
    );
    assert_eq!(tx.document().undo_depth(), before_depth, "预演不得进撤销栈");
    assert!(tx.is_empty(), "预演不得暂存命令");
    tx.rollback().expect("空事务回滚");
}

#[test]
fn preview_matches_real_execution() {
    let mut doc = seeded();
    let cmds = vec![
        transform_to(&doc, "a", 1.5),
        hide(&doc, "b"),
        transform_to(&doc, "c", -2.0),
    ];
    let previewed = doc.begin().preview(&cmds).expect("预演");

    // 同样的三条命令真跑一遍（单命令路径，语义 = 顺序应用）。
    let mut real = seeded();
    for cmd in &cmds {
        real.apply(cmd).expect("真执行");
    }
    assert_eq!(previewed, real.state_hash(), "预演哈希必须等于真执行");
}

#[test]
fn commit_bumps_version_once_and_stages_all() {
    let mut doc = seeded();
    let base_version = doc.version();
    let base_depth = doc.undo_depth();

    let cmds = vec![transform_to(&doc, "a", 2.0), hide(&doc, "b")];
    let mut tx = doc.begin();
    for cmd in cmds {
        tx.apply(cmd).expect("暂存");
    }
    assert_eq!(tx.len(), 2);
    assert_eq!(tx.document().version(), base_version, "暂存期间不推进版本");
    let committed = tx.commit().expect("提交");

    assert_eq!(doc.version(), base_version + 1, "一笔事务只 +1 版本");
    assert_eq!(doc.undo_depth(), base_depth + 2, "n 条命令一次进栈");
    assert_eq!(doc.state_hash(), committed);
}

#[test]
fn empty_commit_is_a_noop() {
    let mut doc = seeded();
    let base_version = doc.version();
    let base_hash = doc.state_hash();
    let hash = doc.begin().commit().expect("空提交");
    assert_eq!(hash, base_hash);
    assert_eq!(doc.version(), base_version, "空事务不推进版本");
}

#[test]
fn failed_apply_rolls_back_to_base() {
    let mut doc = seeded();
    let base_hash = doc.state_hash();
    let base_version = doc.version();
    let base_depth = doc.undo_depth();

    let first = transform_to(&doc, "a", 4.0);
    let second = hide(&doc, "b");
    let mut tx = doc.begin();
    tx.apply(first).expect("第 1 条");
    tx.apply(second).expect("第 2 条");
    // 第 3 条目标不存在 → 整体回滚。
    let err = tx.apply(dangling()).expect_err("第 3 条必失败");
    assert!(matches!(err, DocError::NotFound(_)), "错误应当是 NotFound");

    assert_eq!(tx.document().state_hash(), base_hash, "失败后必须回到 base");
    assert_eq!(tx.document().version(), base_version, "失败不推进版本");
    assert_eq!(tx.document().undo_depth(), base_depth, "失败不进撤销栈");
    assert!(!tx.document().is_dirty(), "干净文档不该因为一次失败变脏");
}

#[test]
fn poisoned_transaction_rejects_further_apply() {
    let mut doc = seeded();
    let mut tx = doc.begin();
    stage_move(&mut tx, "a", 9.0).expect("第 1 条");
    let first_err = tx.apply(dangling()).expect_err("第 2 条必失败");
    let second_err = stage_move(&mut tx, "b", 1.0).expect_err("已作废");
    assert_eq!(first_err, second_err, "后续调用返回首错");
    assert!(tx.is_empty(), "作废后不留暂存");
    assert!(tx.commit().is_err(), "作废的事务不能提交");
}

#[test]
fn rollback_restores_base_without_version_bump() {
    let mut doc = seeded();
    let base_hash = doc.state_hash();
    let base_version = doc.version();
    let base_depth = doc.undo_depth();

    let cmds = vec![
        transform_to(&doc, "a", -1.0),
        hide(&doc, "b"),
        transform_to(&doc, "c", 5.0),
    ];
    let mut tx = doc.begin();
    for cmd in cmds {
        tx.apply(cmd).expect("暂存");
    }
    let restored = tx.rollback().expect("回滚");

    assert_eq!(restored, base_hash, "回滚后哈希 == base");
    assert_eq!(doc.state_hash(), base_hash);
    assert_eq!(doc.version(), base_version, "回滚不推进版本");
    assert_eq!(doc.undo_depth(), base_depth, "回滚不留撤销条目");
    assert!(!doc.is_dirty());
}

#[test]
fn state_hash_sees_staged_changes_via_cache_invalidation() {
    // 事务期间版本不动：若不清缓存，`state_hash()` 会把中间态缓存成「版本 N 的值」，
    // 回滚后再读就拿到那个中间态。这里正面钉住缓存失效。
    let mut doc = seeded();
    let base_hash = doc.state_hash(); // 先把 clean 状态写进缓存
    let cmd = transform_to(&doc, "a", 7.0);
    let mut tx = doc.begin();
    tx.apply(cmd).expect("暂存");
    let mid = tx.document().state_hash();
    assert_ne!(mid, base_hash, "暂存后应看到新状态");

    let restored = tx.rollback().expect("回滚");
    assert_eq!(restored, base_hash);
    assert_eq!(doc.state_hash(), base_hash, "回滚后缓存不得残留中间态");
}

#[test]
fn begin_is_read_only() {
    let mut doc = seeded();
    let base_version = doc.version();
    let base_writes = doc.write_count();
    let tx = doc.begin();
    assert_eq!(tx.document().version(), base_version);
    assert_eq!(tx.document().write_count(), base_writes);
    tx.rollback().expect("空事务回滚");
}
