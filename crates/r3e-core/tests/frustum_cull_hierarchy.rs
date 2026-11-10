//! 扩展二十九：主通道**视锥剔除的层级化**契约。
//!
//! 扩展二十五把级联遮挡体的判定改成了沿场景树自顶向下（`CasterTree::plan`），
//! 这里把同一套下探搬到主通道的视锥剔除上：子树盒整块在视锥外就整枝跳过，
//! 连实例装配都省掉。判定必须与逐实例 `Frustum::intersects_aabb` **逐位相同**，
//! 否则表现就是「物件凭空消失」——比白做的性能损失严重得多。
//!
//! 等价性依据：AABB 的平面判定取沿法线的 p-vertex，子盒 ⊆ 子树盒，因此
//! 子树盒在某平面外侧 ⇒ 枝内每个子盒都在该平面外侧。整枝跳过只少判、不改判。

use glam::Vec3;
use r3e_core::{fail_open_bounds, Aabb, Camera, CasterNode, CasterTree, Frustum};

fn box_at(x: f32, y: f32, z: f32, half: f32) -> Aabb {
    Aabb::from_center_half(Vec3::new(x, y, z), Vec3::splat(half))
}

/// 一台看向 `target` 的相机，转成视锥。
fn frustum_at(position: Vec3, target: Vec3) -> Frustum {
    let camera = Camera {
        position,
        target,
        ..Default::default()
    };
    Frustum::from_view_projection(&camera.view_projection())
}

/// 平铺场景：没有父子关系，每个实例一个根。
fn flat_tree(boxes: &[Aabb]) -> CasterTree {
    CasterTree::new(boxes.iter().copied().map(CasterNode::leaf).collect())
}

/// 最小线性同余发生器：只为「多相机 × 多盒子」的差分覆盖，不引入 rand 依赖。
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32) / ((1u64 << 31) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// 平铺树的层级判定必须与逐实例判定**逐位相同**（多相机差分）。
#[test]
fn flat_tree_matches_per_instance_frustum() {
    let mut rng = Rng(0x5eed_1234);
    let boxes: Vec<Aabb> = (0..24)
        .map(|_| {
            let center = Vec3::new(
                rng.range(-20.0, 20.0),
                rng.range(-20.0, 20.0),
                rng.range(-30.0, 10.0),
            );
            Aabb::from_center_half(center, Vec3::splat(rng.range(0.1, 2.5)))
        })
        .collect();
    let tree = flat_tree(&boxes);
    let instance_nodes: Vec<usize> = (0..boxes.len()).collect();

    for step in 0..8 {
        let angle = step as f32 * 0.7;
        let position = Vec3::new(
            angle.cos() * 12.0,
            angle.sin() * 6.0,
            angle.cos() * 10.0 + 8.0,
        );
        let frustum = frustum_at(position, Vec3::ZERO);
        let expected: Vec<bool> = boxes
            .iter()
            .map(|aabb| frustum.intersects_aabb(aabb))
            .collect();
        let plan = tree.plan_frustum(&frustum);
        let actual: Vec<bool> = instance_nodes
            .iter()
            .map(|index| plan.mask[*index])
            .collect();
        assert_eq!(actual, expected, "第 {step} 台相机下掩码与逐实例判定不一致");
    }
}

/// 层级树的掩码同样必须与逐实例判定逐位相同——整枝剔除只能因为枝内全该剔。
#[test]
fn hierarchy_matches_per_instance_frustum() {
    let mut rng = Rng(0xabc_777);
    // 三棵装配体：一棵在原点附近、一棵远在侧后方、一棵跨在远平面上
    let centers = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(60.0, 12.0, 40.0),
        Vec3::new(-8.0, 3.0, -70.0),
    ];
    let mut nodes = vec![CasterNode::group(None)];
    let mut boxes = Vec::new();
    let mut instance_nodes = Vec::new();
    for center in centers.iter() {
        let parent = nodes.len();
        nodes.push(CasterNode::group(Some(0)));
        for _ in 0..5 {
            let offset = Vec3::new(
                rng.range(-1.5, 1.5),
                rng.range(-1.5, 1.5),
                rng.range(-1.5, 1.5),
            );
            let bounds = Aabb::from_center_half(*center + offset, Vec3::splat(0.4));
            boxes.push(bounds);
            instance_nodes.push(nodes.len());
            nodes.push(CasterNode::leaf(bounds).under(parent));
        }
    }
    let tree = CasterTree::new(nodes);
    assert_eq!(
        instance_nodes.len(),
        boxes.len(),
        "实例与节点的对应表要齐全"
    );

    for step in 0..8 {
        let angle = step as f32 * 0.9;
        let position = Vec3::new(angle.sin() * 10.0, 2.0, angle.cos() * 10.0 + 6.0);
        let frustum = frustum_at(position, Vec3::ZERO);
        let expected: Vec<bool> = boxes
            .iter()
            .map(|aabb| frustum.intersects_aabb(aabb))
            .collect();
        let plan = tree.plan_frustum(&frustum);
        let actual: Vec<bool> = instance_nodes
            .iter()
            .map(|index| plan.mask[*index])
            .collect();
        assert_eq!(actual, expected, "第 {step} 台相机下掩码与逐实例判定不一致");
    }
}

/// 收益来源：整枝在视锥外时一次盒判定顶掉枝内全部实例判定。
#[test]
fn culled_branch_skips_instance_tests() {
    // 相机在 +z 看向原点；这棵装配体在相机背后
    let mut nodes = vec![CasterNode::group(None)];
    for index in 0..6 {
        let bounds = box_at(index as f32 * 0.5 - 1.0, 0.0, 40.0, 0.3);
        nodes.push(CasterNode::leaf(bounds).under(0));
    }
    let tree = CasterTree::new(nodes);
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);

    let plan = tree.plan_frustum(&frustum);
    assert!(!plan.mask[0], "纯分组根在视锥外也必须被剔");
    assert!(
        plan.mask.iter().all(|visible| !*visible),
        "相机背后的装配体必须整枝被剔"
    );
    // 6 个实例一个都没判，只有根的一次子树盒判定
    assert_eq!(plan.counts.pruned, 6);
    assert_eq!(plan.counts.instance_tests, 0);
    assert_eq!(plan.counts.box_tests, 1);
    assert_eq!(plan.counts.branch_pruned, 6, "整枝省下 6 次实例判定");
}

/// 扁平场景里层级化不该有净收益：`pruned` 会等于逐实例剔除数，`branch_pruned` 恒为 0。
#[test]
fn flat_tree_has_no_branch_benefit() {
    let boxes: Vec<Aabb> = (0..10)
        .map(|index| box_at(index as f32 * 8.0 - 20.0, 0.0, 0.0, 0.5))
        .collect();
    let tree = flat_tree(&boxes);
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 30.0), Vec3::ZERO);
    let plan = tree.plan_frustum(&frustum);

    let expected_culled = boxes
        .iter()
        .filter(|aabb| !frustum.intersects_aabb(aabb))
        .count() as u32;
    assert_eq!(plan.counts.pruned, expected_culled);
    assert_eq!(
        plan.counts.branch_pruned, 0,
        "叶子被剔在逐实例判定里也要一次盒判定"
    );
    assert!(
        expected_culled > 0,
        "测试场景本身要真的剔掉东西，否则断言没意义"
    );
}

/// 计数自洽：判过的实例数 + 省下的实例数 == 树里的实例总数。
#[test]
fn counts_are_balanced() {
    let mut nodes = vec![CasterNode::group(None), CasterNode::group(None)];
    for (parent, x) in [(0usize, 0.0f32), (1usize, 40.0)] {
        for index in 0..4 {
            let bounds = box_at(x + index as f32 * 0.6, 0.0, 0.0, 0.25);
            nodes.push(CasterNode::leaf(bounds).under(parent));
        }
    }
    let tree = CasterTree::new(nodes);
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 12.0), Vec3::ZERO);
    let plan = tree.plan_frustum(&frustum);
    assert_eq!(
        plan.counts.instance_tests + plan.counts.pruned,
        tree.total_casts(),
        "判过的与省下的必须覆盖全部实例"
    );
}

/// 拿不到包围盒的实例（渲染层传 `fail_open_bounds`）不能被整枝剔除连累。
#[test]
fn fail_open_bounds_never_culled() {
    // 同一枝里：一个正常盒在视锥外，一个兜底巨盒
    let mut nodes = vec![CasterNode::group(None)];
    nodes.push(CasterNode::leaf(box_at(0.0, 0.0, 60.0, 0.5)).under(0));
    nodes.push(CasterNode::leaf(fail_open_bounds()).under(0));
    let tree = CasterTree::new(nodes);
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
    let plan = tree.plan_frustum(&frustum);

    assert!(plan.mask[2], "兜底巨盒任何视锥下都必须保留");
    assert!(!plan.mask[1], "同一枝里的正常远盒仍要被剔");
    // 远盒叶子的「整枝判定」就是它自己的盒判定：被剔掉，记进 pruned
    assert_eq!(plan.counts.pruned, 1);
    // 巨盒叶子的子树盒保留，做了一次实例判定
    assert_eq!(plan.counts.instance_tests, 1);
    assert_eq!(plan.counts.box_tests, 3, "根 + 两个叶子各一次盒判定");
}

/// 混合节点：自身实例在视锥内、子节点实例在视锥外，各自判定互不牵连。
#[test]
fn mixed_node_tests_own_bounds() {
    let mut nodes = vec![CasterNode::group(None)];
    // 混合节点：自身有实例，直接挂在根下
    nodes.push(CasterNode {
        parent: Some(0),
        bounds: box_at(0.0, 0.0, 0.0, 0.5),
        casts: 1,
    });
    nodes.push(CasterNode::leaf(box_at(0.0, 0.0, 70.0, 0.5)).under(1));
    let tree = CasterTree::new(nodes);
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
    let plan = tree.plan_frustum(&frustum);

    assert!(plan.mask[1], "混合节点自身的实例在视锥内，必须保留");
    assert!(!plan.mask[2], "它的子节点实例在相机背后，必须被剔");
}

/// 空树 / 纯分组树不能 panic，掩码长度与节点数一致。
#[test]
fn empty_and_group_only_trees_are_safe() {
    let frustum = frustum_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
    let empty = CasterTree::new(Vec::new());
    assert!(empty.plan_frustum(&frustum).mask.is_empty());

    let groups = CasterTree::new(vec![CasterNode::group(None), CasterNode::group(Some(0))]);
    let plan = groups.plan_frustum(&frustum);
    assert_eq!(plan.mask.len(), 2);
    assert_eq!(plan.counts.pruned, 0, "没有实例就没有可剔的东西");
    assert_eq!(plan.counts.instance_tests, 0);
}
