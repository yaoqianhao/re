//! 撤销栈（S4-04）验收：以「笔」为单位的历史与全量回退。
//!
//! 覆盖方案 §9 的第 8/9/7 项：随机序列 N=100 全量回退哈希一致、一笔事务单次 undo 全退、
//! 撤销失败不静默丢历史。

use r3e_core::{ObjectDesc, Scene, Transform, Vec3, Vec4};
use r3e_doc::{CommandKind, DocError, Document};

/// 三个节点的干净文档（a / b / c）。
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

/// 确定性伪随机（不引外部依赖，种子固定便于复现）。
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(2) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// 从当前场景状态出发造一条自洽的命令（前像真实取值）。
fn random_cmd(s: &Scene, rng: &mut Lcg) -> Result<CommandKind, DocError> {
    const NAMES: [&str; 3] = ["a", "b", "c"];
    let name = NAMES[rng.below(3) as usize];
    let id = s
        .find_id(name)
        .ok_or_else(|| DocError::NotFound(name.to_string()))?;
    let node = s.get(id).expect("刚查到的节点必然存在");
    Ok(match rng.below(3) {
        0 => CommandKind::SetTransform {
            id,
            old: node.transform,
            new: Transform::from_position(Vec3::new(rng.below(1000) as f32, 0.0, 0.0)),
        },
        1 => CommandKind::SetVisible {
            id,
            old: node.visible,
            new: !node.visible,
        },
        _ => CommandKind::SetNodeColor {
            id,
            old: node.color,
            new: Vec4::new(
                rng.below(100) as f32 / 100.0,
                rng.below(100) as f32 / 100.0,
                rng.below(100) as f32 / 100.0,
                1.0,
            ),
        },
    })
}

#[test]
fn single_command_undo_semantics_unchanged() {
    // S3 老路径：一次 apply = 一笔一条，undo 一次退一条。
    let mut doc = seeded();
    let id = doc.find_id("a").expect("节点存在");
    for x in [1.0, 2.0, 3.0] {
        let old = doc.get(id).expect("节点存在").transform;
        doc.apply(&CommandKind::SetTransform {
            id,
            old,
            new: Transform::from_position(Vec3::new(x, 0.0, 0.0)),
        })
        .expect("apply");
    }
    assert_eq!(doc.undo_depth(), 3, "三次单命令 = 三笔");
    assert_eq!(doc.undo_commands(), 3);
    doc.undo().expect("undo");
    assert_eq!(doc.undo_depth(), 2);
    assert_eq!(
        doc.get(id).expect("节点存在").transform.position.x,
        2.0,
        "单次 undo 只退最后一条"
    );
}

#[test]
fn one_undo_reverts_whole_transaction() {
    let mut doc = seeded();
    let base_hash = doc.state_hash();
    let base_depth = doc.undo_depth();

    let mut tx = doc.begin();
    for i in 0..3 {
        tx.stage(|s| random_cmd(s, &mut Lcg::new(97 + i)))
            .expect("暂存");
    }
    // 上面 stage 用的是固定种子，确保三条命令互不相同即可。
    tx.commit().expect("提交");
    assert_eq!(doc.undo_depth(), base_depth + 1, "一笔事务 = 一笔");
    assert_eq!(doc.undo_commands(), 3, "一笔里含 3 条命令");

    doc.undo().expect("一次 undo 退整笔");
    assert_eq!(doc.undo_depth(), base_depth, "整笔出栈");
    assert_eq!(doc.undo_commands(), 0);
    assert_eq!(doc.state_hash(), base_hash, "一次 undo 必须回到事务前");
}

#[test]
fn rollback_to_mark_reverts_everything_after() {
    let mut doc = seeded();
    let mark = doc.undo_mark();
    let base_hash = doc.state_hash();

    // 三笔：两笔单命令 + 一笔事务（3 条）。
    for x in [1.0, 2.0] {
        let id = doc.find_id("a").expect("节点存在");
        let old = doc.get(id).expect("节点存在").transform;
        doc.apply(&CommandKind::SetTransform {
            id,
            old,
            new: Transform::from_position(Vec3::new(x, 0.0, 0.0)),
        })
        .expect("apply");
    }
    let mut tx = doc.begin();
    for i in 0..3 {
        tx.stage(|s| random_cmd(s, &mut Lcg::new(11 + i)))
            .expect("暂存");
    }
    tx.commit().expect("提交");
    assert_eq!(doc.undo_depth(), 3, "两笔单命令 + 一笔事务");

    let undone = doc.rollback_to(mark).expect("回滚到水位");
    assert!(undone >= 5, "实际撤销条数应当覆盖全部 5 条，得到 {undone}");
    assert_eq!(doc.undo_depth(), 0);
    assert_eq!(doc.state_hash(), base_hash);
}

#[test]
fn random_sequence_undo_to_empty_restores_hash() {
    let mut doc = seeded();
    let initial = doc.state_hash();
    let mut rng = Lcg::new(42);
    let mut applied = 0usize;

    for _ in 0..100 {
        if rng.below(2) == 0 {
            // 单命令笔
            let cmd = random_cmd(doc.as_scene(), &mut rng).expect("造命令");
            doc.apply(&cmd).expect("apply");
            applied += 1;
        } else {
            // 2..=2 条命令的事务笔
            let n = 1 + rng.below(2) as usize;
            let mut tx = doc.begin();
            for _ in 0..n {
                tx.stage(|s| random_cmd(s, &mut rng)).expect("暂存");
            }
            tx.commit().expect("提交");
            applied += n;
        }
    }

    assert_eq!(doc.undo_commands(), applied, "命令条数记账必须对得上");
    assert_eq!(doc.undo_depth(), 100, "100 轮 = 100 笔");
    assert_ne!(doc.state_hash(), initial, "跑完 100 轮状态应当变了");

    // 逐笔回退，每退一笔都要真实变化（除极端撞值外），最终回到初始哈希。
    while doc.undo_depth() > 0 {
        doc.undo().expect("undo");
    }
    assert_eq!(doc.undo_depth(), 0);
    assert_eq!(doc.undo_commands(), 0);
    assert_eq!(
        doc.state_hash(),
        initial,
        "N=100 全量回退后哈希必须等于初始"
    );
}

#[test]
fn failed_undo_keeps_remaining_history() {
    let mut doc = seeded();
    let a = doc.find_id("a").expect("节点存在");
    let b = doc.find_id("b").expect("节点存在");

    // 一笔事务：[改 a 可见, 改 b 可见]；随后把 a 删掉，让第一条命令的撤销必失败。
    let mut tx = doc.begin();
    tx.stage(|s| {
        let old = s.get(a).expect("a 存在").visible;
        Ok(CommandKind::SetVisible {
            id: a,
            old,
            new: !old,
        })
    })
    .expect("第 1 条");
    tx.stage(|s| {
        let old = s.get(b).expect("b 存在").visible;
        Ok(CommandKind::SetVisible {
            id: b,
            old,
            new: !old,
        })
    })
    .expect("第 2 条");
    tx.commit().expect("提交");

    doc.write(|s| s.despawn("a"));
    let err = doc.undo().expect_err("目标没了，整笔撤销必须失败");
    assert!(matches!(err, DocError::NotFound(_)));
    assert_eq!(doc.undo_depth(), 1, "失败的一笔不得凭空消失");
    assert_eq!(doc.undo_commands(), 1, "已撤掉的部分不再留在栈上");
}

#[test]
fn clear_undo_keeps_marks_monotonic() {
    let mut doc = seeded();
    let before = doc.undo_mark();
    let mut tx = doc.begin();
    tx.stage(|s| random_cmd(s, &mut Lcg::new(5))).expect("暂存");
    tx.commit().expect("提交");
    let after_push = doc.undo_mark();
    assert!(after_push > before, "入栈后水位必须上涨");

    doc.clear_undo();
    assert_eq!(doc.undo_depth(), 0);
    assert!(
        doc.undo_mark() >= after_push,
        "清栈不得让水位回退（旧标记不能与新笔撞号）"
    );
}
