//! 三层验证器的集成测试（M1-S4-E）。
//!
//! 这里只测「三层合起来的行为」，不重复测各层内部算法（那在 `r3e-doc` / `r3e-nav`
//! 自己的单测里）。重点有三件事：
//!
//! 1. 近似结论会被精确结论**覆盖**，而不是并列出现（铁律②）；
//! 2. 精确层判不了的对，静态层的近似条目会**留着**（不丢信息）；
//! 3. 某一层缺能力时进 `unchecked`，绝不当成「通过」（铁律③）。

use glam::Vec3;
use r3e_core::{ObjectDesc, PhysicsDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_doc::Category;
use r3e_editor_host::{Layer, VerifyReport};
use r3e_nav::Nav;
use r3e_physics::{BuiltinBackend, Physics};

// ------------------------------------------------------------------ 夹具

/// 一份「物理开着、后端申报 shape_cast / overlap」的世界。
///
/// 用内置后端而不是 rapier：内置后端的查询是解析法真实可查（`BackendCaps::BASIC`），
/// 且零外部依赖，跑测试不用等 20 秒链接。
fn physics_on() -> Physics {
    Physics::with_backend(Box::new(BuiltinBackend::new(
        Vec3::new(0.0, -9.81, 0.0),
        0.0,
    )))
}

fn phys(collider: &str, half: [f32; 3]) -> PhysicsDesc {
    PhysicsDesc {
        body: "static".to_string(),
        mass: 1.0,
        restitution: None,
        friction: None,
        collider: Some(collider.to_string()),
        half_extents: Some(half),
        ccd: None,
    }
}

fn spawn(scene: &mut Scene, name: &str, pos: [f32; 3], scale: f32, physics: Option<PhysicsDesc>) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            position: Some(pos),
            scale: Some([scale, scale, scale]),
            physics,
            ..Default::default()
        })
        .expect("节点应能落进场景");
}

fn flat(name: &str, origin: [f32; 2], size: [f32; 2]) -> TerrainPatch {
    TerrainPatch::flat(name, origin, size)
}

fn codes(report: &VerifyReport) -> Vec<&'static str> {
    report.issues().iter().map(|i| i.code).collect()
}

/// 报告里有没有某个 code（首个字符串实参固定为 code，`xtask invariants` 按此约定
/// 扫源码——B 源正因为能读到这一层，物理 / 导航层的码才算「有反例测试兜住」）。
fn has_code(report: &VerifyReport, code: &str) -> bool {
    report.issues().iter().any(|i| i.code == code)
}

fn find<'a>(report: &'a VerifyReport, code: &str) -> Option<&'a r3e_doc::Issue> {
    report.issues().iter().find(|i| i.code == code)
}

/// 出生点是独立于节点的声明（`scene.spawns`），不是场景里的物件——
/// 所以它不会给自己的脚下添一个碰撞体。
fn spawn_at(scene: &mut Scene, name: &str, pos: [f32; 3]) {
    scene.spawns.push(SpawnDecl::new(name, pos));
}

// ------------------------------------------------------------------ 物理层

#[test]
fn disabled_physics_is_unchecked_not_passed() {
    let mut scene = Scene::new();
    spawn(&mut scene, "island", [0.0, 8.0, 0.0], 2.0, None);
    scene.update_world_transforms();

    // 默认 `Physics::new()`：关闭 + null 后端。
    let report = VerifyReport::collect(&scene, &Physics::new(), None);
    assert!(
        report
            .unchecked()
            .iter()
            .any(|u| u.layer() == Layer::Physics),
        "物理关闭必须留一条「没检查」的原因"
    );
    // 静态层的近似结论保留：物理层没给结论，就不能凭空消失。
    let suspect = find(&report, "support.suspect").expect("静态层应报悬空候选");
    assert!(suspect.approximate);
}

#[test]
fn floating_body_is_confirmed_precisely() {
    let mut scene = Scene::new();
    // 底面 y = 4.0，下方 0.05 米内空空如也。
    spawn(
        &mut scene,
        "crate",
        [0.0, 5.0, 0.0],
        2.0,
        Some(phys("box", [1.0, 1.0, 1.0])),
    );
    scene.update_world_transforms();

    let report = VerifyReport::collect(&scene, &physics_on(), None);
    assert!(has_code(&report, "support.floating"), "物理层确认悬空");
    let confirmed = find(&report, "support.floating").expect("物理层应确认悬空");
    assert!(!confirmed.approximate, "物理层结论不是近似");
    assert_eq!(confirmed.category, Category::Support);
    assert_eq!(
        codes(&report)
            .iter()
            .filter(|c| **c == "support.suspect" || **c == "support.floating")
            .count(),
        1,
        "同一物件的近似候选必须被精确结论覆盖，不能两条并存"
    );
}

#[test]
fn sphere_penetration_is_confirmed_without_approximation() {
    let mut scene = Scene::new();
    // 两个半径 0.5 的球，中心距 0.6 → 真的插进去了。
    spawn(
        &mut scene,
        "a",
        [0.0, 0.5, 0.0],
        1.0,
        Some(phys("sphere", [0.5, 0.5, 0.5])),
    );
    spawn(
        &mut scene,
        "b",
        [0.6, 0.5, 0.0],
        1.0,
        Some(phys("sphere", [0.5, 0.5, 0.5])),
    );
    scene.update_world_transforms();

    let report = VerifyReport::collect(&scene, &physics_on(), None);
    assert!(has_code(&report, "overlap.penetration"), "物理层确认穿透");
    let hit = find(&report, "overlap.penetration").expect("物理层应确认穿透");
    assert!(!hit.approximate);
    assert_eq!(hit.category, Category::Overlap);
    assert_eq!(hit.target.as_deref(), Some("a|b"));
    let loc = hit.location.expect("穿透必须给坐标");
    assert!((loc[0] - 0.3).abs() < 1e-2, "重叠中心应在两球之间：{loc:?}");
    assert_eq!(
        codes(&report)
            .iter()
            .filter(|c| **c == "overlap.suspect")
            .count(),
        0,
        "精确结论落地后不该再留静态层的近似候选"
    );
}

#[test]
fn decor_overlap_stays_approximate() {
    let mut scene = Scene::new();
    // 只有 `a` 有碰撞体；`b` 是纯装饰物，物理层根本没有它的碰撞体。
    spawn(
        &mut scene,
        "a",
        [0.0, 1.0, 0.0],
        2.0,
        Some(phys("box", [1.0, 1.0, 1.0])),
    );
    spawn(&mut scene, "b", [0.6, 1.0, 0.0], 2.0, None);
    scene.update_world_transforms();

    let report = VerifyReport::collect(&scene, &physics_on(), None);
    assert!(
        find(&report, "overlap.penetration").is_none(),
        "装饰物没有碰撞体：物理层不许替它下结论"
    );
    let suspect = find(&report, "overlap.suspect").expect("静态层近似结论应保留");
    assert!(suspect.approximate);
}

// ------------------------------------------------------------------ 导航层

#[test]
fn unbaked_nav_is_unchecked_not_passed() {
    let mut scene = Scene::new();
    spawn(&mut scene, "crate", [0.0, 1.0, 0.0], 2.0, None);
    scene.update_world_transforms();

    let report = VerifyReport::collect(&scene, &value_physics(), None);
    let nav = report
        .unchecked()
        .iter()
        .find(|u| u.layer() == Layer::Nav)
        .expect("没接导航槽位必须留一条原因");
    assert!(nav.reason().contains("导航"), "原因应说清是导航缺失");
    assert_eq!(
        report.count_by_layer(Layer::Nav),
        0,
        "没检查就不该有导航结论"
    );
}

/// 开着物理的桩：悬空判定的前置条件，导航测试只关心导航结论。
fn value_physics() -> Physics {
    Physics::with_backend(Box::new(BuiltinBackend::new(
        Vec3::new(0.0, -9.81, 0.0),
        0.0,
    )))
}

#[test]
fn spawn_off_mesh_is_reported_with_coordinate() {
    let mut scene = Scene::new();
    scene.terrain.push(flat("ground", [0.0, 0.0], [10.0, 10.0]));
    spawn_at(&mut scene, "start", [2.5, 0.0, 2.5]);
    // 地形之外：烘焙范围会扩张到这里，但这一格没有地面 → 不可走。
    spawn_at(&mut scene, "stray", [50.0, 0.0, 50.0]);
    scene.update_world_transforms();

    let nav = Nav::bake_scene(&scene).expect("场景应可烘焙");
    let report = VerifyReport::collect(&scene, &value_physics(), Some(&nav));

    assert!(
        has_code(&report, "nav.spawn_offmesh"),
        "地形外的出生点不可走"
    );
    let off = find(&report, "nav.spawn_offmesh").expect("地形外的出生点应报不可走");
    assert_eq!(off.category, Category::Reachability);
    assert!(
        off.target.as_deref() == Some("stray"),
        "只该报那个错位的出生点：{:?}",
        off.target
    );
    let loc = off.location.expect("必须给坐标");
    assert!((loc[0] - 50.0).abs() < 1e-2 && (loc[2] - 50.0).abs() < 1e-2);
    assert!(
        !report
            .issues()
            .iter()
            .any(|i| i.code == "nav.spawn_offmesh" && i.target.as_deref() == Some("start")),
        "站在地形上的出生点不该被报"
    );
}

#[test]
fn unreachable_target_reports_the_cut_point() {
    let mut scene = Scene::new();
    // 两块互不相连的地形，中间 10 米虚空。
    scene.terrain.push(flat("west", [0.0, 0.0], [10.0, 10.0]));
    scene.terrain.push(flat("east", [20.0, 0.0], [10.0, 10.0]));
    spawn_at(&mut scene, "start", [2.5, 0.0, 2.5]);
    scene.update_world_transforms();

    let nav = Nav::bake_scene(&scene).expect("场景应可烘焙");
    let targets = vec![("boss".to_string(), Vec3::new(25.0, 0.0, 5.0))];
    let report = VerifyReport::collect_with_targets(&scene, &value_physics(), Some(&nav), &targets);

    assert!(has_code(&report, "nav.no_path"), "跨岛目标不可达");
    let no_path = find(&report, "nav.no_path").expect("跨岛目标应报不可达");
    assert_eq!(no_path.target.as_deref(), Some("start>boss"));
    let cut = no_path.location.expect("不可达必须给断点坐标");
    assert!(
        cut[0] <= 10.0 + 1e-2,
        "断点应在西侧岛边，而不是目标点：{cut:?}"
    );
    assert!(
        find(&report, "nav.target_offmesh").is_none(),
        "目标点本身站在地形上，不该报 off-mesh"
    );
}

#[test]
fn spawns_in_two_islands_warn_region_split() {
    let mut scene = Scene::new();
    scene.terrain.push(flat("west", [0.0, 0.0], [10.0, 10.0]));
    scene.terrain.push(flat("east", [20.0, 0.0], [10.0, 10.0]));
    spawn_at(&mut scene, "start", [2.5, 0.0, 2.5]);
    spawn_at(&mut scene, "alt", [25.0, 0.0, 2.5]);
    scene.update_world_transforms();

    let nav = Nav::bake_scene(&scene).expect("场景应可烘焙");
    let report = VerifyReport::collect(&scene, &value_physics(), Some(&nav));

    assert!(
        has_code(&report, "nav.region_split"),
        "出生点分散在孤立区域"
    );
    let split = find(&report, "nav.region_split").expect("出生点分散在孤立区域应给汇总警告");
    assert!(
        split.msg.contains("2 个"),
        "消息要点明连通块数量：{}",
        split.msg
    );
    assert!(
        !report
            .issues()
            .iter()
            .any(|i| i.code == "nav.spawn_offmesh"),
        "两个出生点都站在地形上：不该报 off-mesh"
    );
}

#[test]
fn target_off_mesh_is_reported_before_reachability() {
    let mut scene = Scene::new();
    scene.terrain.push(flat("ground", [0.0, 0.0], [10.0, 10.0]));
    spawn_at(&mut scene, "start", [2.5, 0.0, 2.5]);
    scene.update_world_transforms();

    let nav = Nav::bake_scene(&scene).expect("场景应可烘焙");
    // 目标点在虚空里：先报「站不住」，不该再沿它去算路径（那样只会得出一条
    // 从岛边断掉的 no_path，掩盖真正的问题——目标点坐标本身写错了）。
    let targets = vec![("boss".to_string(), Vec3::new(100.0, 0.0, 100.0))];
    let report = VerifyReport::collect_with_targets(&scene, &value_physics(), Some(&nav), &targets);

    assert!(
        has_code(&report, "nav.target_offmesh"),
        "虚空目标点应报不可走"
    );
    let off = find(&report, "nav.target_offmesh").expect("应有目标点不可走的结论");
    assert_eq!(off.category, Category::Reachability);
    assert_eq!(off.target.as_deref(), Some("boss"));
    let loc = off.location.expect("必须给坐标");
    assert!((loc[0] - 100.0).abs() < 1e-2 && (loc[2] - 100.0).abs() < 1e-2);
    assert!(
        !has_code(&report, "nav.no_path"),
        "目标点自己就站不住时不该再报不可达：那是上一个结论的下游噪音"
    );
}

// ------------------------------------------------------------------ 角色族归层（M2-B04）

/// 角色族的码由静态层判出，必须归 `Layer::Static`。
///
/// `layer_of` 是穷尽 `match`：新加 `Category::Character` 却漏了这一臂，编译就过不去；
/// 但**归对了没**只能靠这条测试钉——归错层会让报告把「角色埋进地里」说成几何问题，
/// 调用方按层过滤时就会漏看。
fn character_report(pos: [f32; 3], spec: r3e_core::CharacterSpec) -> VerifyReport {
    let mut scene = Scene::new();
    let id = scene
        .spawn(ObjectDesc {
            name: "hero".to_string(),
            position: Some(pos),
            ..Default::default()
        })
        .expect("角色节点应能落进场景");
    scene.get_mut(id).expect("刚建的节点").character = Some(spec);
    scene.update_world_transforms();
    // 物理关着：这两条码与物理无关，静态层必须独立判出来
    VerifyReport::collect(&scene, &Physics::new(), None)
}

fn static_count_without_character() -> usize {
    let mut scene = Scene::new();
    spawn(&mut scene, "hero", [0.0, -0.5, 0.0], 1.0, None);
    scene.update_world_transforms();
    VerifyReport::collect(&scene, &Physics::new(), None).count_by_layer(Layer::Static)
}

#[test]
fn character_penetration_lands_on_static_layer() {
    let spec = r3e_core::CharacterSpec::new();
    let report = character_report([0.0, -0.5, 0.0], spec);
    assert!(
        has_code(&report, "character.ground.penetration"),
        "角色脚底埋进地面半个身位，静态层必须报穿地"
    );
    assert_eq!(
        report.count_by_layer(Layer::Static),
        static_count_without_character() + 1,
        "多出来的那一条必须记在静态层（归错层等于让调用方漏看）"
    );
    let issue = find(&report, "character.ground.penetration").expect("应有穿地条目");
    assert_eq!(issue.category, Category::Character);
    assert_eq!(issue.target.as_deref(), Some("hero"));
}

#[test]
fn character_config_invalid_lands_on_static_layer() {
    let mut spec = r3e_core::CharacterSpec::new();
    spec.radius = 0.0;
    let report = character_report([0.0, 0.0, 0.0], spec);
    assert!(
        has_code(&report, "character.config.invalid"),
        "半径为 0 的角色配置必须被静态层报出来，不能静默夹取"
    );
    assert_eq!(
        report.count_by_layer(Layer::Static),
        static_count_without_character() + 1,
        "配置非法也必须记在静态层"
    );
    assert!(
        !has_code(&report, "character.ground.penetration"),
        "配置非法时几何不可信，不该连带报穿地"
    );
}

// -------------------------------------------------- 导航产物可信度（M2 · D04）

/// 一张空网格：地形铺好、再压一块比它更大的碰撞体，可走格被吃到 0。
///
/// 不用「场景里什么都没有」来造空网格——那是「没烘」而不是「烘出来是空的」，
/// 两者的处置完全不同（前者进 `unchecked`，后者必须报产物不可用）。
fn empty_nav(scene: &mut Scene) -> Nav {
    scene.terrain.push(flat("ground", [0.0, 0.0], [10.0, 10.0]));
    spawn(
        scene,
        "slab",
        [0.0, 1.0, 0.0],
        1.0,
        Some(phys("box", [8.0, 1.0, 8.0])),
    );
    scene.update_world_transforms();
    // 半径再放大到 8 米：即便有缝隙也不该留下一格，确保夹具稳定复现空网格。
    let cfg = r3e_nav::NavConfig {
        agent_radius: 8.0,
        ..Default::default()
    };
    Nav::bake_scene_with(scene, cfg).expect("场景应可烘焙（烘焙成功但产物为空）")
}

#[test]
fn empty_bake_is_reported_as_unusable() {
    let mut scene = Scene::new();
    let nav = empty_nav(&mut scene);
    assert_eq!(nav.report().walkable, 0, "夹具要的是一张空网格");

    let report = VerifyReport::collect(&scene, &value_physics(), Some(&nav));
    assert!(
        has_code(&report, "nav.bake.empty"),
        "没有一个可走格时，第一句必须是「这份产物不可用」"
    );
    let empty = find(&report, "nav.bake.empty").expect("应报空网格");
    assert!(
        empty.msg.contains("没有一个可走格"),
        "文案要说清是可走格为 0：{}",
        empty.msg
    );
    // 空网格上谈可达性是空话：后续结论必须转成「没检查」，不能当作通过。
    assert!(
        !has_code(&report, "nav.spawn_offmesh") && !has_code(&report, "nav.no_path"),
        "空网格不该再逐点报可达性结论"
    );
    assert!(
        report.unchecked().iter().any(|u| u.layer() == Layer::Nav),
        "跳过可达性必须留下原因"
    );
}

#[test]
fn nav_from_another_backend_is_mismatch() {
    let mut scene = Scene::new();
    scene.terrain.push(flat("ground", [0.0, 0.0], [10.0, 10.0]));
    spawn_at(&mut scene, "start", [2.5, 0.0, 2.5]);
    scene.update_world_transforms();

    let mut nav = Nav::bake_scene(&scene).expect("场景应可烘焙");
    // 贴一个和校验后端不同的名字：这份网格出自另一套采样实现。
    nav.attach_backend("rapier");

    let report = VerifyReport::collect(&scene, &value_physics(), Some(&nav));
    assert!(
        has_code(&report, "nav.backend.mismatch"),
        "烘焙后端与校验后端不同时必须报产物不可信"
    );
    let issue = find(&report, "nav.backend.mismatch").expect("应报后端不一致");
    assert!(issue.msg.contains("rapier"), "文案要点名烘焙后端");
    assert!(issue.msg.contains("builtin"), "文案要点名校验后端");

    // 同一后端重贴一遍就不该再报——判据是名字是否相等，不是「有没有贴过名字」。
    nav.attach_backend("builtin");
    let same = VerifyReport::collect(&scene, &value_physics(), Some(&nav));
    assert!(
        !has_code(&same, "nav.backend.mismatch"),
        "同一后端烘的网格不该被报不一致"
    );
}
