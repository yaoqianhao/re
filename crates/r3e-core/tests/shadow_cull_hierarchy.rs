//! 扩展二十五：级联遮挡体的**层级剔除**契约。
//!
//! 扩展二十四的判定逐实例做（`O(级 × 实例)`）。层级剔除沿场景树自顶向下判：
//! 子树包围盒整块落在级外就整枝跳过，只剩叶子需要逐实例判。收益来源是「少判」，
//! 因此这一层的红线有两条：
//!
//! * **掩码必须与逐实例判定逐位相同**——正交投影下并集的投影范围等于各成员投影
//!   范围的并集，所以整枝裁掉等价于枝内实例逐个裁掉；一旦不相等就是丢影子；
//! * **计数必须自洽**——`instance_tests + pruned == 级数 × 遮挡体数`，不能出现
//!   「判了又不算、算了又没判」的糊涂账。

use glam::{Mat4, Vec3};
use r3e_core::{plan_casters, Aabb, CasterNode, CasterTree};

fn ortho(scale: f32) -> Mat4 {
    Mat4::from_scale(Vec3::splat(scale))
}

fn box_at(x: f32, y: f32, z: f32, half: f32) -> Aabb {
    Aabb::from_center_half(Vec3::new(x, y, z), Vec3::splat(half))
}

/// 平铺场景：每个实例都是自己的根，没有父子关系。
fn flat_tree(boxes: &[Aabb]) -> CasterTree {
    CasterTree::new(boxes.iter().copied().map(CasterNode::leaf).collect())
}

/// 一棵两层树：根 = 装配体，下面挂 4 个零件；零件全部聚在 `center` 附近。
fn assembly_tree(center: Vec3, spread: f32) -> (CasterTree, Vec<usize>) {
    let mut nodes = vec![CasterNode::group(None)];
    for index in 0..4 {
        let offset = match index {
            0 => Vec3::new(-spread, 0.0, 0.0),
            1 => Vec3::new(spread, 0.0, 0.0),
            2 => Vec3::new(0.0, -spread, 0.0),
            _ => Vec3::new(0.0, spread, 0.0),
        };
        nodes.push(
            CasterNode::leaf(Aabb::from_center_half(center + offset, Vec3::splat(0.2))).under(0),
        );
    }
    // 实例顺序：先根（纯分组节点没有实例，实例从下标 1 开始）
    let instance_nodes: Vec<usize> = (1..nodes.len()).collect();
    (CasterTree::new(nodes), instance_nodes)
}

/// 子树盒子已经被聚出来了：根节点的子树盒必须覆盖所有零件。
#[test]
fn subtree_bounds_union_children() {
    let (tree, _) = assembly_tree(Vec3::new(5.0, 0.0, 0.0), 1.0);
    let root = tree.subtree_bounds(0);
    assert!(!root.is_empty());
    assert!(root.min.x <= 3.8 + 1e-4 && root.max.x >= 6.2 - 1e-4);
    assert_eq!(tree.total_casts(), 4, "整棵树的遮挡体实例数");
    assert_eq!(tree.subtree_casts(0), 4, "根节点子树里的实例数");
    assert_eq!(tree.subtree_casts(1), 1, "叶子自己的实例数");
}

/// 平铺树的层级判定必须与逐实例判定**逐位相同**（差分）。
#[test]
fn flat_tree_matches_per_instance_plan() {
    let boxes: Vec<Aabb> = (0..12)
        .map(|i| {
            let f = i as f32;
            box_at(f * 1.7 - 9.0, f * 0.3 - 1.5, f * 0.5, 0.4 + f * 0.05)
        })
        .collect();
    let levels = [ortho(0.5), ortho(0.2), ortho(0.08)];
    let expected = plan_casters(&levels, &boxes);

    let tree = flat_tree(&boxes);
    let plan = tree.plan(&levels);
    let mask = r3e_core::expand_masks(&plan.masks, &(0..boxes.len()).collect::<Vec<_>>());
    assert_eq!(mask, expected, "平铺树的掩码必须与逐实例判定一致");
}

/// 层级树的掩码必须与逐实例判定**逐位相同**——整枝裁掉只能因为枝内每个实例都该裁。
#[test]
fn hierarchy_matches_per_instance_plan() {
    // 两棵装配体：一棵在近级覆盖范围内，一棵远在级外
    let near = Vec3::new(0.0, 0.0, 0.0);
    let far = Vec3::new(40.0, 0.0, 0.0);
    let mut nodes = vec![CasterNode::group(None), CasterNode::group(None)];
    let mut boxes = Vec::new();
    for (parent, center) in [(0usize, near), (1usize, far)] {
        for index in 0..4 {
            let offset = Vec3::new(index as f32 * 0.6 - 0.9, 0.0, 0.3);
            let bounds = Aabb::from_center_half(center + offset, Vec3::splat(0.25));
            boxes.push(bounds);
            nodes.push(CasterNode::leaf(bounds).under(parent));
        }
    }
    let instance_nodes: Vec<usize> = (2..nodes.len()).collect();
    let tree = CasterTree::new(nodes);

    let levels = [ortho(0.5), ortho(0.15), ortho(0.05), ortho(0.02)];
    let expected = plan_casters(&levels, &boxes);
    let plan = tree.plan(&levels);
    let actual = r3e_core::expand_masks(&plan.masks, &instance_nodes);
    assert_eq!(actual, expected, "层级判定与逐实例判定的掩码必须逐位一致");
}

/// 收益来源：远枝整块被裁时不再逐个实例判，`pruned` 要如实记下省下的量。
#[test]
fn pruned_branch_skips_instance_tests() {
    let far = Vec3::new(80.0, 0.0, 0.0);
    let (tree, instance_nodes) = assembly_tree(far, 1.0);
    let levels = [ortho(0.5), ortho(0.2)];
    let plan = tree.plan(&levels);
    let actual = r3e_core::expand_masks(&plan.masks, &instance_nodes);
    assert!(
        actual.iter().all(|mask| mask.iter().all(|keep| !*keep)),
        "远在级外的装配体必须整枝被裁"
    );
    // 两级 × 4 个实例 = 8 次实例判定全被省下，只剩每级 1 次子树盒判定
    assert_eq!(plan.pruned, 8);
    assert_eq!(plan.instance_tests, 0);
    assert_eq!(plan.box_tests, 2);
}

/// 计数自洽：判过的实例数 + 省下的实例数 == 级数 × 实例总数。
#[test]
fn counts_are_balanced() {
    let mut nodes = vec![CasterNode::group(None), CasterNode::group(None)];
    for (parent, x) in [(0usize, 0.0f32), (1usize, 12.0)] {
        for index in 0..3 {
            let bounds = box_at(x + index as f32 * 0.5, 0.0, 0.0, 0.2);
            nodes.push(CasterNode::leaf(bounds).under(parent));
        }
    }
    let tree = CasterTree::new(nodes);
    let levels = [ortho(0.6), ortho(0.25), ortho(0.1)];
    let plan = tree.plan(&levels);
    assert_eq!(
        plan.instance_tests + plan.pruned,
        levels.len() as u32 * tree.total_casts(),
        "判过的与省下的必须覆盖全部 (级, 实例) 对"
    );
}

/// 深树：中间节点的子树盒保留、但自身实例单独判——混合节点不能被漏掉。
#[test]
fn mixed_node_tests_own_bounds_after_subtree_pass() {
    // 根自身有一个实例在原点；子节点实例远在级外
    let mut nodes = vec![CasterNode::leaf(box_at(0.0, 0.0, 0.0, 0.2))];
    nodes.push(CasterNode::leaf(box_at(6.0, 0.0, 0.0, 0.2)).under(0));
    let tree = CasterTree::new(nodes);
    let levels = [ortho(0.5)]; // 覆盖 ±2
    let plan = tree.plan(&levels);
    assert!(plan.masks[0][0], "根自身实例在级内，必须保留");
    assert!(!plan.masks[0][1], "子节点实例在级外，必须裁掉");
    // 子树盒（覆盖 -0.2..6.2）与级相交 → 下探；根自身盒再判一次
    assert_eq!(plan.box_tests, 3, "子树盒 + 根自身盒 + 子叶盒");
}

/// fail-open 的巨盒会撑大整条父链：子树盒因此永远与级相交，整枝不会被整块裁，
/// 枝内仍按各自盒逐实例判——巨盒只能「让别人多判」，不能「让别人被误裁」。
#[test]
fn fail_open_box_prevents_branch_pruning_but_keeps_leaves_precise() {
    let mut nodes = vec![CasterNode::leaf(r3e_core::fail_open_bounds())];
    for index in 0..3 {
        nodes.push(CasterNode::leaf(box_at(50.0 + index as f32, 0.0, 0.0, 0.2)).under(0));
    }
    let tree = CasterTree::new(nodes);
    let plan = tree.plan(&[ortho(0.5)]); // 覆盖 ±2

    assert!(plan.masks[0][0], "巨盒自身必须保留（fail-open）");
    assert!(
        plan.masks[0][1..].iter().all(|keep| !*keep),
        "枝内实例仍按各自盒判定，远的照样裁"
    );
    // 根子树盒被巨盒撑开 → 下探；根自身实例判过；三个远处的叶子各自被裁
    assert_eq!(plan.instance_tests, 1);
    assert_eq!(plan.pruned, 3);
    assert_eq!(plan.instance_tests + plan.pruned, 4);
}

/// 坏层级数据（越界 / 自指 / 成环）不能让遍历打转或 panic。
#[test]
fn broken_parent_links_degrade_to_roots() {
    let nodes = vec![
        CasterNode {
            parent: Some(9),
            bounds: box_at(0.0, 0.0, 0.0, 0.2),
            casts: 1,
        },
        CasterNode {
            parent: Some(1),
            bounds: box_at(1.0, 0.0, 0.0, 0.2),
            casts: 1,
        },
        CasterNode {
            parent: Some(3),
            bounds: box_at(2.0, 0.0, 0.0, 0.2),
            casts: 1,
        },
        CasterNode {
            parent: Some(2),
            bounds: box_at(3.0, 0.0, 0.0, 0.2),
            casts: 1,
        },
    ];
    let tree = CasterTree::new(nodes.clone());
    assert_eq!(tree.len(), 4);
    let plan = tree.plan(&[ortho(0.5)]);
    assert_eq!(plan.masks[0].len(), 4);
    let boxes: Vec<Aabb> = nodes.iter().map(|node| node.bounds).collect();
    let expected = plan_casters(&[ortho(0.5)], &boxes);
    assert_eq!(
        r3e_core::expand_masks(&plan.masks, &[0, 1, 2, 3]),
        expected,
        "坏层级只影响判定路径，不能改变结果"
    );
}

/// 空输入与纯分组节点：形状正确、零判定、不 panic。
#[test]
fn empty_and_group_only_trees_are_safe() {
    let empty = CasterTree::new(Vec::new());
    assert!(empty.is_empty());
    let plan = empty.plan(&[ortho(0.5)]);
    assert_eq!(plan.masks, vec![Vec::<bool>::new()]);
    assert_eq!(plan.box_tests, 0);
    assert_eq!(plan.instance_tests + plan.pruned, 0);

    // 只有纯分组节点、没有任何实例：整棵树下探一次都不判
    let groups = CasterTree::new(vec![CasterNode::group(None), CasterNode::group(Some(0))]);
    let plan = groups.plan(&[ortho(0.5), ortho(0.2)]);
    assert_eq!(plan.box_tests, 0, "没有遮挡体的枝不该产生判定");
    assert_eq!(plan.pruned, 0);
    assert_eq!(plan.instance_tests, 0);
}

/// 下标越界的实例映射按保留处理（fail-open），不能因为映射错位丢影子。
#[test]
fn expand_masks_fails_open_on_unknown_node() {
    let masks = vec![vec![true, false]];
    let expanded = r3e_core::expand_masks(&masks, &[0, 1, 7]);
    assert_eq!(expanded, vec![vec![true, false, true]]);
}
