//! 主通道视锥剔除层级化的静态契约（扩展二十九，不需要 GPU）。
//!
//! 扩展二十九把 `prepare_instances` 的视锥判定从「逐实例 `Frustum::intersects_aabb`」
//! 换成「按子树预筛 → 拿掩码」。正确性由 `r3e-core` 的差分测试
//! （`frustum_cull_hierarchy.rs`：掩码与逐实例判定逐位相同）兜底，这里钉住**接线**：
//!
//! 1. 判定走 `CasterTree::plan_frustum`，不再逐实例调 `frustum.intersects_aabb`；
//! 2. 层级树与阴影的 `caster_tree` 是**两棵**（阴影那棵只装进桶的遮挡体，装不下
//!    被视锥剔掉的实例；共用一个字段就会互相覆盖，子树盒缩水成「只剩可见实例」）；
//! 3. 拓扑只在 `Scene::layout_version` 前进时重建（否则每帧重建，收益倒挂）；
//! 4. 掩码取不到时 fail-open（保留实例），绝不因为数据缺失让物件消失；
//! 5. 掩码筛选排在入桶之前，被剪掉的实例不占实例槽位、不占关节槽位。

const SOURCE: &str = include_str!("../src/renderer.rs");

fn prepare_instances_body() -> &'static str {
    let start = SOURCE
        .find("fn prepare_instances")
        .expect("renderer.rs 里必须还有 prepare_instances");
    let rest = &SOURCE[start..];
    let end = rest[1..]
        .find("\n    /// ")
        .or_else(|| rest[1..].find("\n    fn "))
        .or_else(|| rest[1..].find("\n    pub fn "))
        .expect("prepare_instances 之后应该还有别的成员函数");
    &rest[..end + 1]
}

/// 视锥判定必须走层级路径，逐实例的 AABB 测试不能再出现。
#[test]
fn frustum_cull_goes_through_hierarchy() {
    let body = prepare_instances_body();
    assert!(
        body.contains("plan_frustum(&frustum)"),
        "主通道视锥剔除必须走 CasterTree::plan_frustum"
    );
    assert!(
        !body.contains("frustum.intersects_aabb"),
        "逐实例视锥测试应已下线；留着它说明层级掩码没真正接管判定"
    );
}

/// 阴影的 caster 树装不下「被视锥剔掉的实例」，两棵树必须分开。
#[test]
fn cull_tree_is_separate_from_caster_tree() {
    let body = prepare_instances_body();
    assert!(
        body.contains("self.sync_cull_layout(scene)"),
        "主通道视锥树必须走自己的 sync_cull_layout"
    );
    assert!(
        !body.contains("self.sync_caster_layout(scene)"),
        "主通道不能复用阴影的 caster 树：那棵只装进桶的遮挡体，被剔实例不在其中"
    );
    assert!(
        SOURCE.contains("fn sync_cull_layout(&mut self, scene: &Scene)"),
        "sync_cull_layout 丢失"
    );
}

/// 拓扑缓存必须带 layout_version 守卫。
#[test]
fn cull_topology_is_cached_by_layout_version() {
    let start = SOURCE
        .find("fn sync_cull_layout")
        .expect("sync_cull_layout 丢失");
    let rest = &SOURCE[start..];
    let end = rest[1..].find("\n    fn ").unwrap_or(rest.len() - 1);
    let body = &rest[..end + 1];
    assert!(
        body.contains("self.cull_layout == scene.layout_version()"),
        "重建必须由 layout_version 守卫，否则每帧重建树、收益倒挂"
    );
    assert!(
        body.contains("self.cull_tree_builds = self.cull_tree_builds.saturating_add(1)"),
        "建树计数丢失"
    );
}

/// 掩码缺失时必须保留实例（fail-open）。
#[test]
fn missing_mask_keeps_instance() {
    let body = prepare_instances_body();
    assert!(
        body.contains(".unwrap_or(true)"),
        "掩码取不到（树未建 / 节点不在表里）必须按保留处理"
    );
    assert!(
        body.contains("r3e_core::fail_open_bounds"),
        "缺包围盒的节点要按 fail-open 巨盒登记进树，否则所在整枝会被误剪"
    );
}

/// 掩码筛选必须在入桶之前：被剪的实例不占实例槽位。
#[test]
fn mask_filter_runs_before_bucketing() {
    let body = prepare_instances_body();
    let mask = body.find("plan.mask.get(slot)").expect("掩码取值丢失");
    let push = body
        .find(".entry((key.0, key.1, false))")
        .expect("分桶丢失");
    assert!(
        mask < push,
        "掩码筛选必须排在入桶之前，否则被剪实例仍占实例槽位"
    );
    let staged = body
        .find("staged_all.push(StagedInstance")
        .expect("实例收拢丢失");
    assert!(mask < staged, "掩码筛选必须排在实例构造之前");
}

/// 统计要能对账：`instance_tests + pruned` 与「过筛实例数」同量级。
#[test]
fn cull_stats_are_reported() {
    let body = prepare_instances_body();
    assert!(body.contains("plan.counts.box_tests"), "盒判定计数丢失");
    assert!(
        body.contains("plan.counts.instance_tests"),
        "实例判定计数丢失"
    );
    assert!(body.contains("plan.counts.pruned"), "未判即裁计数丢失");
    assert!(
        body.contains("plan.counts.branch_pruned"),
        "整枝跳过（净收益）计数丢失"
    );
}
