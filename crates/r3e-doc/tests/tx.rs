//! 会话式事务（M1-S1）验收：影子隔离、提交一次记账、冲突显式化、错误路径不留半成品。
//!
//! 与 `transaction.rs`（借用式 `Transaction`）的分工：那边验「一次调用内跑完 n 条」，
//! 这边验「跨多次调用攒命令」——每次 `tx_*` 都当作一次独立 RPC 来调，
//! 中间不留任何借用，事务状态全在 `Document` 上。

use r3e_core::{ObjectDesc, Scene, Transform, Vec3};
use r3e_doc::{CommandKind, DocError, Document};

/// 三个节点的干净文档（a / b / c），版本 / 脏标记已归零。
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

/// 影子场景上的「改 x 变换」意图，等价宿主 `scene.settransform`。
fn move_to(s: &Scene, name: &str, x: f32) -> Result<CommandKind, DocError> {
    let id = s
        .find_id(name)
        .ok_or_else(|| DocError::NotFound(format!("物件 `{name}` 不存在")))?;
    CommandKind::set_transform(s, id, Transform::from_position(Vec3::new(x, 0.0, 0.0)))
}

fn hide(s: &Scene, name: &str) -> Result<CommandKind, DocError> {
    let id = s
        .find_id(name)
        .ok_or_else(|| DocError::NotFound(format!("物件 `{name}` 不存在")))?;
    CommandKind::set_visible(s, id, false)
}

fn x_of(doc: &Document, name: &str) -> f32 {
    let id = doc.find_id(name).expect("节点存在");
    doc.get(id).expect("节点存在").transform.position.x
}

#[test]
fn 开启事务不动主场景() {
    let mut doc = seeded();
    let hash = doc.state_hash();
    let version = doc.version();
    let depth = doc.undo_depth();

    let begun = doc.tx_begin("t1").expect("首次开启应当成功");
    assert_eq!(begun.base, hash, "基哈希应当取自开启时刻");
    assert_eq!(begun.version, version);
    assert_eq!(begun.id, "t1");

    assert_eq!(doc.state_hash(), hash, "开启不改变内容");
    assert_eq!(doc.version(), version, "开启不推进版本");
    assert_eq!(doc.undo_depth(), depth, "开启不进撤销栈");
}

#[test]
fn 暂存只推进影子_主场景一个字节都不动() {
    let mut doc = seeded();
    let base = doc.state_hash();
    doc.tx_begin("t1").unwrap();

    let staged = doc
        .tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .expect("暂存应当成功");
    assert_eq!(staged.staged, 1);
    assert_eq!(staged.op, "scene.settransform");

    let preview = doc.tx_preview("t1").unwrap();
    assert_eq!(preview.staged, 1);
    assert_eq!(preview.base, base);
    assert_eq!(preview.current, base, "暂存期间主场景不变");
    assert_ne!(preview.shadow, base, "影子应当已经变了");

    assert_eq!(x_of(&doc, "a"), 0.0, "主场景里 a 还没动");
    assert_eq!(doc.state_hash(), base);
    assert_eq!(doc.undo_depth(), 0, "暂存不进撤销栈");
}

#[test]
fn 影子叠加_后一条看到前一条的效果() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .unwrap();

    let session = doc.tx_session().expect("事务在手");
    assert_eq!(
        session
            .shadow()
            .get(session.shadow().find_id("a").unwrap())
            .unwrap()
            .transform
            .position
            .x,
        5.0,
        "影子里的 a 已经是 5"
    );

    // 第二条基于影子：把 visible 关掉，两个效果应当在影子上同时存在
    doc.tx_apply("t1", "scene.setvisible", |s| hide(s, "a"))
        .unwrap();
    let preview = doc.tx_preview("t1").unwrap();
    assert_eq!(preview.staged, 2);
    assert_ne!(preview.shadow, preview.current);
    assert_eq!(x_of(&doc, "a"), 0.0, "主场景仍然没动");
}

#[test]
fn 提交整笔只记一格版本只加一() {
    let mut doc = seeded();
    let base = doc.state_hash();
    let version = doc.version();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .unwrap();
    doc.tx_apply("t1", "scene.setvisible", |s| hide(s, "b"))
        .unwrap();
    let shadow = doc.tx_preview("t1").unwrap().shadow;

    let done = doc.tx_commit("t1").expect("提交应当成功");
    assert_eq!(done.committed, 2);
    assert_eq!(done.hash, shadow, "提交后的哈希应当等于影子哈希");
    assert_eq!(done.version, version + 1, "整笔事务只推进一次版本");
    assert_eq!(doc.version(), version + 1);
    assert_eq!(doc.undo_depth(), 1, "整笔只占撤销栈一格");
    assert_eq!(doc.undo_commands(), 2, "一格里有两条命令");
    assert!(doc.tx_session().is_none(), "提交后事务应当摘掉");
    assert_ne!(doc.state_hash(), base);

    // 一次 undo 退掉整笔
    doc.undo().expect("undo 应当成功");
    assert_eq!(doc.state_hash(), base, "一次 undo 回到事务之前");
    assert_eq!(x_of(&doc, "a"), 0.0);
    assert!(doc.get(doc.find_id("b").unwrap()).unwrap().visible);
}

#[test]
fn 提交后主场景等于影子() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "c", -2.5))
        .unwrap();
    doc.tx_commit("t1").unwrap();
    assert_eq!(x_of(&doc, "c"), -2.5);
}

#[test]
fn 回滚丢弃全部暂存_哈希与版本复位() {
    let mut doc = seeded();
    let base = doc.state_hash();
    let version = doc.version();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 9.0))
        .unwrap();
    doc.tx_apply("t1", "scene.setvisible", |s| hide(s, "a"))
        .unwrap();

    let rolled = doc.tx_rollback("t1").expect("回滚应当成功");
    assert_eq!(rolled.dropped, 2);
    assert_eq!(rolled.hash, base, "回滚后哈希应当等于基哈希");
    assert_eq!(rolled.version, version, "回滚不推进版本");
    assert!(doc.tx_session().is_none(), "回滚后事务应当摘掉");
    assert_eq!(x_of(&doc, "a"), 0.0);
}

#[test]
fn 同一时刻只允许一笔事务() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    match doc.tx_begin("t2") {
        Err(DocError::Busy(msg)) => assert!(msg.contains("t1"), "报错里应当点名未决事务：{msg}"),
        other => panic!("第二笔应当报 Busy，实际 {other:?}"),
    }
    assert!(doc.tx_session().is_some(), "报 Busy 不该顺手把原事务丢掉");
}

#[test]
fn 事务id对不上就拒绝() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    assert!(matches!(
        doc.tx_apply("t2", "scene.settransform", |s| move_to(s, "a", 1.0)),
        Err(DocError::TxNotFound(_))
    ));
    assert!(matches!(doc.tx_preview("t2"), Err(DocError::TxNotFound(_))));
    assert!(matches!(doc.tx_commit("t2"), Err(DocError::TxNotFound(_))));
    assert!(matches!(
        doc.tx_rollback("t2"),
        Err(DocError::TxNotFound(_))
    ));
    assert!(doc.tx_session().is_some(), "错 id 不该动到真事务");
}

#[test]
fn 没有事务时四种操作都报txnotfound() {
    let mut doc = seeded();
    assert!(matches!(
        doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 1.0)),
        Err(DocError::TxNotFound(_))
    ));
    assert!(matches!(doc.tx_preview("t1"), Err(DocError::TxNotFound(_))));
    assert!(matches!(doc.tx_commit("t1"), Err(DocError::TxNotFound(_))));
    assert!(matches!(
        doc.tx_rollback("t1"),
        Err(DocError::TxNotFound(_))
    ));
}

#[test]
fn 事务期间被外部改动_提交报冲突并作废() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .unwrap();

    // 事务期间绕过事务改主场景（模拟另一条 RPC / 手动编辑）
    doc.write(|s| {
        s.spawn(ObjectDesc {
            name: "d".to_string(),
            ..ObjectDesc::default()
        })
        .unwrap()
    });

    match doc.tx_commit("t1") {
        Err(DocError::Conflict(msg)) => {
            assert!(msg.contains("丢弃"), "冲突信息应当说明暂存已丢弃：{msg}")
        }
        other => panic!("应当报 Conflict，实际 {other:?}"),
    }
    assert!(doc.tx_session().is_none(), "冲突后事务应当作废");
    assert_eq!(x_of(&doc, "a"), 0.0, "冲突不该把旧前像盖上去");
}

#[test]
fn 应用失败即整笔作废() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .unwrap();
    // 目标不存在：构造阶段就失败
    assert!(matches!(
        doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "ghost", 1.0)),
        Err(DocError::NotFound(_))
    ));
    assert!(
        doc.tx_session().is_some(),
        "构造失败不该作废事务（调用方补参数还能接着攒）"
    );

    // 执行阶段失败：场景里已有同名节点，再 spawn 同名会失败
    let mut doc2 = seeded();
    doc2.tx_begin("t1").unwrap();
    assert!(doc2
        .tx_apply("t1", "scene.spawn", |_| Ok(CommandKind::spawn_node(
            ObjectDesc {
                name: "x".to_string(),
                ..ObjectDesc::default()
            }
        )))
        .is_ok());
    let dup = doc2.tx_apply("t1", "scene.spawn", |_| {
        Ok(CommandKind::spawn_node(ObjectDesc {
            name: "a".to_string(),
            ..ObjectDesc::default()
        }))
    });
    assert!(dup.is_err(), "同名 spawn 应当失败");
    assert!(doc2.tx_session().is_none(), "执行失败即整笔作废");
    assert!(matches!(doc2.tx_commit("t1"), Err(DocError::TxNotFound(_))));
}

#[test]
fn 空事务提交是安全空操作() {
    let mut doc = seeded();
    let base = doc.state_hash();
    let version = doc.version();
    doc.tx_begin("t1").unwrap();
    let done = doc.tx_commit("t1").expect("空事务提交应当成功");
    assert_eq!(done.committed, 0);
    assert_eq!(done.hash, base);
    assert_eq!(done.version, version, "空事务不推进版本");
    assert!(doc.tx_session().is_none(), "空事务提交后也要摘掉");
    assert_eq!(doc.undo_depth(), 0);
}

#[test]
fn 换场景会清掉未决事务() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 5.0))
        .unwrap();

    let old = doc.replace_scene(Scene::new());
    assert!(old.iter().count() >= 3, "旧场景被换出来");
    assert!(doc.tx_session().is_none(), "换场景后未决事务作废");
    assert!(matches!(doc.tx_commit("t1"), Err(DocError::TxNotFound(_))));
}

// ---------- M1-S3-A：意图（一次意图 = N 条命令 = 一格撤销）----------

/// 「实例化一组节点」的意图：第一条为根，其余挂到根下（父名 = 批内第一个）。
///
/// 故意让后续组件依赖批内第一条命令的效果——若文档层不按顺序把命令应用到影子，
/// 第 2 条起会因为「父物件不存在」直接失败，这个用例就会红。
fn prefab(s: &Scene, names: &[&str]) -> Result<Vec<CommandKind>, DocError> {
    let _ = s;
    Ok(names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            CommandKind::spawn_node(ObjectDesc {
                name: (*name).to_string(),
                parent: if i == 0 {
                    None
                } else {
                    Some(names[0].to_string())
                },
                ..ObjectDesc::default()
            })
        })
        .collect())
}

#[test]
fn 一条意图展开三条命令只占一格撤销() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    let staged = doc
        .tx_apply_intent("t1", "spawn_prefab", |s| {
            prefab(s, &["desk", "leg1", "leg2"])
        })
        .expect("意图应当被接受");

    assert_eq!(staged.intent, "spawn_prefab");
    assert_eq!(staged.commands, 3, "一条意图展开三条命令");
    assert_eq!(staged.staged, 3, "`staged` 报的是命令总数");
    assert_eq!(staged.intents, 1, "`intents` 报的是步数");

    let preview = doc.tx_preview("t1").expect("预演应当成功");
    assert_eq!(
        preview.intents,
        vec![("spawn_prefab".to_string(), 3)],
        "预演要能列出每条意图的展开规模"
    );

    let done = doc.tx_commit("t1").expect("提交应当成功");
    assert_eq!(done.committed, 3);
    assert_eq!(doc.undo_depth(), 1, "整笔事务只占一格");
    assert_eq!(doc.undo_commands(), 3, "一格里有三条命令");
    assert!(doc.find_id("leg2").is_some());
    let parent = doc.get(doc.find_id("leg1").unwrap()).unwrap().parent;
    assert_eq!(parent, doc.find_id("desk"), "批次内的父子关系要保住");

    doc.undo().expect("undo 应当成功");
    // 注意：`SpawnNode` 的 undo 只按名删节点，`next_id` 不回退（撤销不等于时空倒流），
    // 所以这里不能拿 `state_hash` 断言「回到提交前」——只断言节点全退干净。
    for name in ["desk", "leg1", "leg2"] {
        assert!(doc.find_id(name).is_none(), "`{name}` 应当被一起退掉");
    }
    assert_eq!(doc.undo_depth(), 0, "一格退完，撤销栈见底");
}

#[test]
fn 裸命令与意图混攒_一次undo全退() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "a", 4.0))
        .unwrap();
    doc.tx_apply_intent("t1", "spawn_prefab", |s| prefab(s, &["p1", "p2"]))
        .unwrap();

    let done = doc.tx_commit("t1").expect("提交应当成功");
    assert_eq!(done.committed, 3, "一裸命令 + 两命令意图");
    assert_eq!(doc.undo_depth(), 1, "整笔事务一格，意图不另立一格");

    doc.undo().expect("undo 应当成功");
    assert!(doc.find_id("p1").is_none(), "意图那两条一起退掉");
    assert_eq!(x_of(&doc, "a"), 0.0, "裸命令也一起退掉");
}

#[test]
fn 意图内某条失败_影子回到意图前且事务保留() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    doc.tx_apply("t1", "scene.settransform", |s| move_to(s, "b", 7.0))
        .unwrap();
    let before_commands = doc.tx_session().expect("事务在").len();

    let err = doc
        .tx_apply_intent("t1", "spawn_prefab", |s| {
            let mut cmds = prefab(s, &["ghost"])?;
            // 第二条故意指向不存在的节点：apply 阶段必然失败（构造阶段拦不住）。
            cmds.push(CommandKind::SetVisible {
                id: 999_999,
                old: true,
                new: false,
            });
            Ok(cmds)
        })
        .expect_err("意图中途失败应当报错");

    assert!(
        matches!(err, DocError::NotFound(_)),
        "错误应当来自文档层: {err:?}"
    );
    assert_eq!(
        doc.tx_session().expect("事务在").len(),
        before_commands,
        "失败的意图不该留下暂存"
    );
    assert!(
        doc.tx_session().is_some(),
        "意图失败不作废事务，可改参数重试"
    );
    // 半截意图必须被清掉：提交只应带上前面那条裸命令，`ghost` 不许混进主场景。
    let done = doc.tx_commit("t1").expect("提交应当成功");
    assert_eq!(done.committed, 1, "只提交前一步那条裸命令");
    assert!(
        doc.find_id("ghost").is_none(),
        "失败的意图不该把半个 prefab 提交进主场景"
    );
    assert_eq!(x_of(&doc, "b"), 7.0, "前一步裸命令照常生效");
}

#[test]
fn 空意图被拒且事务保留() {
    let mut doc = seeded();
    doc.tx_begin("t1").unwrap();
    let err = doc
        .tx_apply_intent("t1", "paint_terrain", |_| Ok(Vec::new()))
        .expect_err("空意图应当被拒");
    assert!(
        matches!(err, DocError::Rejected(_)),
        "空意图按 Rejected 拒: {err:?}"
    );
    assert!(doc.tx_session().is_some(), "空意图不该作废事务");
    assert_eq!(doc.tx_session().expect("事务在").steps(), 0);
}

#[test]
fn 意图的txid对不上或没有事务都报txnotfound() {
    let mut doc = seeded();
    assert!(matches!(
        doc.tx_apply_intent("t1", "spawn_prefab", |s| prefab(s, &["x"])),
        Err(DocError::TxNotFound(_))
    ));

    doc.tx_begin("t1").unwrap();
    assert!(matches!(
        doc.tx_apply_intent("t2", "spawn_prefab", |s| prefab(s, &["x"])),
        Err(DocError::TxNotFound(_))
    ));
    assert_eq!(doc.tx_session().expect("事务在").len(), 0, "错 id 不碰影子");
}
