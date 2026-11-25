//! D01 卡的门禁测试：**导航网格烘焙的唯一入口与逐位产物**。
//!
//! 这张卡要锁死四件事，任何一件被别人改回去，这里必须红：
//!
//! 1. **真值链唯一**：真值链是 `TerrainPatch::sample_height` → `HeightField` →
//!    `NavGrid.heights`，中间不许再有第二份插值。`bake_with_field` 是唯一烘焙入口，
//!    网格范围与格长一律来自场，`NavConfig::bounds` / `cell_size` 不再能覆盖它。
//! 2. **双后端同产物**：两个后端各装一次场、各烘一次，产物**逐位**相等。
//!    一致不是「比出来的」，是构造出来的（唯一入口吃同一份场）。
//! 3. **产物可序列化**：`to_bytes` / `from_bytes` 往返不丢位，坏输入一律报错不静默容忍。
//! 4. **逐格分类与可走位图同源**：`CellClass::Blocked` ⟺ `!is_walkable_cell`，
//!    坡度/台阶判定与 `can_step` 共用同一个函数。
//!
//! 这里的断言都按**口径**写，不绑具体格数：默认 `cell_size` 一调，格数就变，但
//! 「换后端产物一致」「Blocked 只在不可走格上」是能力承诺。

use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{CellClass, Nav, NavConfig, NavError, NavProduct};
use r3e_physics::{HeightField, Physics, PhysicsKind};

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 一块 16×16 的平地 + 一个出生点（与 `tests/bake.rs` 同构的基准场景）。
fn flat_scene() -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 16×16 的斜坡地形：沿 +X 均匀上升 `rise` 米。
///
/// 用 17×17 个样本点（每米一个）而不是 2×2，是为了让「格心处的采样值」
/// 与「双线性插值的解析值」在恰好落在样本点上时**逐位相同**，断言才谈得上逐位。
fn ramp_scene(rise: f32) -> Scene {
    let mut scene = Scene::new();
    let mut patch = TerrainPatch::flat("slope", [0.0, 0.0], [16.0, 16.0]);
    patch.resolution = [17, 17];
    patch.heights = (0..17 * 17)
        .map(|idx| {
            let i = (idx % 17) as f32;
            rise * (i / 16.0)
        })
        .collect();
    scene.terrain.push(patch);
    scene.update_world_transforms();
    scene
}

/// 加一堵沿 Z 方向、把场地切成左右两半的墙。
fn add_wall(scene: &mut Scene, name: &str, x: f32, height: f32) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some([x, height * 0.5, 8.0]),
            scale: Some([0.2, height, 16.0]),
            ..Default::default()
        })
        .expect("墙必须能生成");
    scene.update_world_transforms();
}

/// 手搓一份高度场：`h(i, j)` 给出第 `(i, j)` 个采样点的高度。
fn crafted_field(
    cell: f32,
    cols: usize,
    rows: usize,
    h: impl Fn(usize, usize) -> f32,
) -> HeightField {
    let mut heights = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        for i in 0..cols {
            heights.push(h(i, j));
        }
    }
    HeightField::new(0.0, 0.0, cell, cols, rows, heights).expect("手搓场必须合法")
}

/// 造一个指定后端的物理实例（`Null` 之外都要求当前构建真的编译了）。
///
/// 顺序不能反：`set_enabled(true)` 见到后端还是 `null` 就会把它升级成默认后端，
/// 所以**先开总开关、再显式选后端**，否则想测 `null` 后端会测到 rapier 上去。
fn physics_with(kind: PhysicsKind) -> Physics {
    let mut physics = Physics::new();
    physics.set_enabled(true);
    physics
        .set_kind(kind)
        .unwrap_or_else(|e| panic!("后端 {} 应当可用：{e}", kind.name()));
    physics
}

/// 当前构建里可用的**真实**后端（不含 `Null`：它装不上地形场）。
fn real_backends() -> Vec<PhysicsKind> {
    PhysicsKind::ALL
        .into_iter()
        .filter(|k| *k != PhysicsKind::Null && k.is_available())
        .collect()
}

/// 高度数组的比特形态，用来做「逐位相等」断言。
fn heights_bits(h: &[f32]) -> Vec<u32> {
    h.iter().map(|v| v.to_bits()).collect()
}

// ---------------------------------------------------------------------------
// 一、真值链唯一（5）
// ---------------------------------------------------------------------------

#[test]
fn nav_heights_are_field_heights_bit_for_bit() {
    // 场里放一批「双线性插值算不出来」的怪值（非单调、带负零），
    // 只要网格高度与场高度逐位相同，就说明烘焙期没有第二次插值。
    let field = crafted_field(0.5, 7, 5, |i, j| {
        let v = (i as f32 * 0.37 + j as f32 * 1.13).sin() * 3.25;
        if (i + j) % 3 == 0 {
            -0.0
        } else {
            v
        }
    });
    let nav =
        Nav::bake_with_field(&Scene::new(), &field, NavConfig::default()).expect("手搓场必须能烘");
    assert_eq!(
        heights_bits(nav.product().heights()),
        heights_bits(field.heights()),
        "网格高度必须就是场高度（逐位），烘焙期不许再插值一次"
    );
    assert_eq!(nav.product().dims(), [7, 5], "网格维度必须等于场的采样点数");
}

#[test]
fn nav_heights_match_terrain_sample_at_cell_centers() {
    // 真值链第二段的落地检查：场 → 网格这一段不许改动数值。
    // 地形那一端的插值由 `TerrainPatch::sample_height` 负责，这里用格心重新采一次对账。
    let scene = ramp_scene(6.0);
    let cfg = NavConfig::default();
    let field = r3e_nav::field_of(&scene, &cfg).expect("斜坡可采成场");
    let nav = Nav::bake_with_field(&scene, &field, cfg).expect("斜坡可烘焙");
    let grid = nav.grid();
    let patch = &scene.terrain[0];
    let [cols, rows] = grid.dims();
    for j in 0..rows {
        for i in 0..cols {
            let center = grid.cell_center(i, j);
            let expect = patch.sample_height(center.x, center.z).unwrap_or(0.0);
            assert_eq!(
                grid.ground(i, j).to_bits(),
                expect.to_bits(),
                "格 ({i}, {j}) 的高度必须等于地形在格心的采样值"
            );
        }
    }
}

#[test]
fn bake_with_field_ignores_cfg_bounds() {
    // 范围由场说了算：`cfg.bounds` 在这里只提供判定参数，不许再把世界改成另一个样子。
    let field = crafted_field(1.0, 6, 4, |i, j| (i + j) as f32 * 0.1);
    let cfg = NavConfig::default().with_bounds([100.0, 100.0, 108.0, 104.0]);
    let nav = Nav::bake_with_field(&Scene::new(), &field, cfg).expect("必须能烘");
    assert_eq!(
        nav.product().dims(),
        [6, 4],
        "网格维度必须来自场，`cfg.bounds` 不许覆盖"
    );
    assert_eq!(
        nav.product().min(),
        [-0.5, -0.5],
        "网格原点必须由场原点反推，`cfg.bounds` 不许覆盖"
    );
}

#[test]
fn field_cell_size_overrides_cfg_cell_size() {
    // 格长同理由场说了算：装进去的场格长是 0.75，配置写 2.0 也不许生效。
    let field = crafted_field(0.75, 5, 5, |_, _| 0.0);
    let cfg = NavConfig::default().with_cell_size(2.0);
    let nav = Nav::bake_with_field(&Scene::new(), &field, cfg).expect("必须能烘");
    assert_eq!(nav.product().cell_size(), 0.75, "格长必须来自场");
    assert_eq!(
        nav.grid().config().cell_size,
        0.75,
        "收敛后的配置也要跟着场"
    );
}

#[test]
fn tiny_field_pads_to_two_rows_and_keeps_coverage_honest() {
    // 极端小范围：`plan` 只给出 1×1 格，场必须补到 2×2 才合法；
    // 补出来的那一圈落在真实地形之外，必须如实记为「没有地面」而不是「高度 0 就能走」。
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("tiny", [0.0, 0.0], [0.5, 0.5]));
    scene.update_world_transforms();
    let cfg = NavConfig::default()
        .with_bounds([0.0, 0.0, 0.5, 0.5])
        .with_cell_size(1.0);
    let field = r3e_nav::field_of(&scene, &cfg).expect("极小范围也要能成场");
    assert_eq!((field.cols(), field.rows()), (2, 2), "场至少 2×2");

    let nav = Nav::bake_with_field(&scene, &field, cfg).expect("必须能烘");
    let grid = nav.grid();
    assert_eq!(grid.dims(), [2, 2]);
    assert_eq!(
        grid.walkable_count(),
        1,
        "只有真正被地形覆盖的那一格可走，补出来的一圈必须不可走"
    );
    assert!(grid.is_walkable_cell(0, 0), "(0,0) 格心在 0.5×0.5 的块内");
    assert!(!grid.is_walkable_cell(1, 0), "(1,0) 格心已在块外");
}

// ---------------------------------------------------------------------------
// 二、双后端同产物（4）
// ---------------------------------------------------------------------------

#[test]
fn every_available_backend_bakes_the_same_product() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let cfg = NavConfig::default();

    let kinds = real_backends();
    assert!(!kinds.is_empty(), "至少要有一个真实后端可用");
    if !PhysicsKind::Rapier.is_available() {
        eprintln!("本构建未编译 rapier：只能用 {} 烘一次", kinds.len());
    } else {
        assert_eq!(kinds.len(), 2, "双后端口径要求 builtin + rapier 都在");
    }

    let mut baked = Vec::new();
    for kind in kinds {
        let mut physics = physics_with(kind);
        let nav = Nav::bake_with_backend(&scene, cfg.clone(), &mut physics)
            .unwrap_or_else(|e| panic!("后端 {} 烘焙失败：{e}", kind.name()));
        baked.push((kind.name(), nav.product(), nav.product_hex()));
    }

    let (first_name, first_product, first_hex) = &baked[0];
    for (name, product, hex) in &baked[1..] {
        assert_eq!(
            product, first_product,
            "后端 {name} 与 {first_name} 的产物必须逐位相等"
        );
        assert_eq!(
            hex, first_hex,
            "后端 {name} 与 {first_name} 的产物哈希必须相同"
        );
    }
}

#[test]
fn switching_backend_keeps_field_bit_identical() {
    if !PhysicsKind::Rapier.is_available() {
        eprintln!("本构建未编译 rapier：跳过换后端搬运场的断言");
        return;
    }
    let field = crafted_field(0.5, 6, 6, |i, j| i as f32 * 1.5 - j as f32 * 0.25);
    let mut physics = physics_with(PhysicsKind::Builtin);
    physics.set_heightfield(Some(field.clone()));
    assert_eq!(
        physics.heightfield().map(|f| heights_bits(f.heights())),
        Some(heights_bits(field.heights())),
        "装进去什么就要取回什么"
    );

    physics
        .set_kind(PhysicsKind::Rapier)
        .expect("rapier 必须可切");
    let carried = physics.heightfield().expect("换后端必须把场搬过去");
    assert_eq!(
        heights_bits(carried.heights()),
        heights_bits(field.heights()),
        "换后端后场必须逐位不变"
    );
    assert_eq!((carried.cols(), carried.rows()), (6, 6), "场的维度也不许变");

    physics
        .set_kind(PhysicsKind::Builtin)
        .expect("切回内置后端");
    let back = physics.heightfield().expect("切回来场还在");
    assert_eq!(heights_bits(back.heights()), heights_bits(field.heights()));
}

#[test]
fn backend_bake_equals_local_bake() {
    // 后端路径 = 本地路径：装一次、取回、烘一次，产物不许有任何差异。
    let scene = ramp_scene(2.0);
    let cfg = NavConfig::default();
    let local = Nav::bake_with(&scene, cfg.clone()).expect("本地烘焙");
    let mut physics = physics_with(physics_kind_first());
    let via = Nav::bake_with_backend(&scene, cfg, &mut physics).expect("后端烘焙");
    assert_eq!(local.product(), via.product(), "两条路径的产物必须逐位相等");
    assert_eq!(local.product_hex(), via.product_hex());
    assert_eq!(local.cfg_digest(), via.cfg_digest());
    assert_eq!(local.input_digest(), via.input_digest());
}

#[test]
fn null_backend_reports_mismatch_instead_of_falling_back() {
    // 后端没保留场时必须报错，绝不能悄悄退回本地场把不一致藏起来。
    let scene = flat_scene();
    let mut physics = physics_with(PhysicsKind::Null);
    let err = Nav::bake_with_backend(&scene, NavConfig::default(), &mut physics)
        .expect_err("null 后端装不上场，必须报错");
    match err {
        NavError::BackendMismatch { backend, .. } => assert_eq!(backend, "null"),
        other => panic!("期望 BackendMismatch，实际 {other:?}"),
    }
    assert!(
        physics.heightfield().is_none(),
        "报错之后后端里仍然不该有场"
    );
}

/// 首选的真实后端（当前构建的默认后端）。
fn physics_kind_first() -> PhysicsKind {
    PhysicsKind::preferred_default()
}

// ---------------------------------------------------------------------------
// 三、确定性与两级摘要（6）
// ---------------------------------------------------------------------------

#[test]
fn two_bakes_of_the_same_scene_are_bit_identical() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let cfg = NavConfig::default();
    let a = Nav::bake_with(&scene, cfg.clone()).expect("第一次烘焙");
    let b = Nav::bake_with(&scene, cfg).expect("第二次烘焙");
    assert_eq!(a.product_hash(), b.product_hash(), "同输入必须同产物哈希");
    assert_eq!(a.product(), b.product(), "同输入必须同产物");
    assert_eq!(a.cfg_digest(), b.cfg_digest());
    assert_eq!(a.input_digest(), b.input_digest());
}

#[test]
fn hash_changes_when_terrain_height_changes() {
    let flat = Nav::bake(&flat_scene()).expect("平地");
    let ramp = Nav::bake(&ramp_scene(1.0)).expect("斜坡");
    assert_ne!(
        flat.product_hash(),
        ramp.product_hash(),
        "地形变了哈希必须变"
    );
    assert_ne!(
        flat.input_digest(),
        ramp.input_digest(),
        "输入摘要必须跟着变"
    );
}

#[test]
fn hash_changes_when_cell_size_changes() {
    let scene = flat_scene();
    let fine = Nav::bake_with(&scene, NavConfig::default().with_cell_size(0.5)).expect("细格");
    let coarse = Nav::bake_with(&scene, NavConfig::default().with_cell_size(1.0)).expect("粗格");
    assert_ne!(fine.product_hash(), coarse.product_hash());
    assert_ne!(fine.cfg_digest(), coarse.cfg_digest(), "参数摘要必须跟着变");
    assert_eq!(
        fine.input_digest(),
        coarse.input_digest(),
        "场景没变，输入摘要不该跟着格长变"
    );
}

#[test]
fn cfg_digest_ignores_ignore_tag_order() {
    let scene = flat_scene();
    let a = NavConfig {
        ignore_tags: vec!["actor".into(), "npc".into(), "dynamic".into()],
        ..Default::default()
    };
    let b = NavConfig {
        ignore_tags: vec!["dynamic".into(), "actor".into(), "npc".into()],
        ..Default::default()
    };
    let na = Nav::bake_with(&scene, a).expect("烘焙 A");
    let nb = Nav::bake_with(&scene, b).expect("烘焙 B");
    assert_eq!(
        na.cfg_digest(),
        nb.cfg_digest(),
        "标签集合的顺序不是输入变化"
    );
    assert_eq!(na.product_hash(), nb.product_hash());
}

#[test]
fn input_digest_ignores_node_creation_order() {
    // 同一批障碍，创建顺序不同 → 障碍盒数组顺序不同，但输入摘要必须相同。
    let mut a = flat_scene();
    add_wall(&mut a, "wall_a", 4.0, 2.0);
    add_wall(&mut a, "wall_b", 12.0, 2.0);

    let mut b = flat_scene();
    add_wall(&mut b, "wall_b", 12.0, 2.0);
    add_wall(&mut b, "wall_a", 4.0, 2.0);

    let na = Nav::bake(&a).expect("烘焙 A");
    let nb = Nav::bake(&b).expect("烘焙 B");
    assert_eq!(
        na.input_digest(),
        nb.input_digest(),
        "节点遍历顺序不是输入变化"
    );
    assert_eq!(na.product_hash(), nb.product_hash(), "产物也必须一致");
}

#[test]
fn product_hex_is_sixteen_lowercase_hex() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let hex = nav.product_hex();
    assert_eq!(hex.len(), 16, "哈希按 u64 的 16 位十六进制打印");
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "必须是小写十六进制：{hex}"
    );
    assert_eq!(hex, format!("{:016x}", nav.product_hash()));
}

// ---------------------------------------------------------------------------
// 四、序列化（6）
// ---------------------------------------------------------------------------

#[test]
fn product_bytes_roundtrip_preserves_hash() {
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙");
    let product = nav.product();
    let bytes = product.to_bytes();
    let back = NavProduct::from_bytes(&bytes).expect("自家产物必须能读回");
    assert_eq!(back, product, "往返必须逐位相同");
    assert_eq!(back.product_hash(), product.product_hash());
    assert_eq!(back.to_bytes(), bytes, "再序列化一次必须字节相同");
    assert_eq!(
        NavProduct::from_bytes(&back.to_bytes())
            .expect("二次往返")
            .product_hash(),
        product.product_hash()
    );
}

#[test]
fn product_bytes_reject_foreign_magic() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let mut bytes = nav.product().to_bytes();
    bytes[0] = b'X';
    match NavProduct::from_bytes(&bytes) {
        Err(NavError::ProductMismatch { reason, .. }) => {
            assert!(reason.contains("魔数"), "错误信息要点明魔数：{reason}")
        }
        other => panic!("坏魔数必须报错，实际 {other:?}"),
    }
}

#[test]
fn product_bytes_reject_version_skew() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let mut bytes = nav.product().to_bytes();
    bytes[4] = 99;
    bytes[5] = 0;
    match NavProduct::from_bytes(&bytes) {
        Err(NavError::ProductMismatch { reason, .. }) => {
            assert!(reason.contains("版本"), "错误信息要点明版本：{reason}")
        }
        other => panic!("版本不符必须报错，实际 {other:?}"),
    }
}

#[test]
fn product_bytes_reject_truncated_stream() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let bytes = nav.product().to_bytes();
    let head = &bytes[..bytes.len() / 2];
    match NavProduct::from_bytes(head) {
        Err(NavError::ProductMismatch { reason, .. }) => {
            assert!(
                reason.contains("截断") || reason.contains("短") || reason.contains("长度"),
                "提示要看得出是截断：{reason}"
            )
        }
        other => panic!("截断必须报错，实际 {other:?}"),
    }
}

#[test]
fn product_bytes_reject_trailing_garbage() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let mut bytes = nav.product().to_bytes();
    bytes.push(0);
    match NavProduct::from_bytes(&bytes) {
        Err(NavError::ProductMismatch { reason, .. }) => {
            assert!(
                reason.contains("多余"),
                "提示要看得出是尾部多余数据：{reason}"
            )
        }
        other => panic!("尾部垃圾必须报错，实际 {other:?}"),
    }
}

#[test]
fn product_bytes_reject_unknown_class_code() {
    let nav = Nav::bake(&flat_scene()).expect("烘焙");
    let product = nav.product();
    let mut bytes = product.to_bytes();
    let cells = product.dims()[0] as usize * product.dims()[1] as usize;
    // 头 46 字节（魔数 4 + 版本 2 + 维度 8 + 原点 8 + 格长 4 + 两级摘要 16 + 格数 4），
    // 之后是 `cells` 个 f32 高度，再之后是逐格分类字节。
    let class_at = 46 + cells * 4;
    assert!(bytes.len() > class_at, "布局变化，偏移需要跟着改");
    bytes[class_at] = 200;
    match NavProduct::from_bytes(&bytes) {
        Err(NavError::ProductMismatch { reason, .. }) => {
            assert!(reason.contains("分类"), "提示要点明分类编码：{reason}")
        }
        other => panic!("未知分类编码必须报错，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 五、逐格分类与可走位图同源（5）
// ---------------------------------------------------------------------------

#[test]
fn flat_field_is_entirely_flat_class() {
    let field = crafted_field(1.0, 6, 6, |_, _| 2.5);
    let nav = Nav::bake_with_field(&Scene::new(), &field, NavConfig::default()).expect("烘焙");
    let hist = nav.grid().class_histogram();
    assert_eq!(
        hist[CellClass::Flat.code() as usize],
        36,
        "平地必须全是 Flat"
    );
    assert_eq!(
        hist[CellClass::Blocked.code() as usize],
        0,
        "平地不该有不可走格"
    );
    assert_eq!(nav.grid().class_of(0, 0), Some(CellClass::Flat));
    assert_eq!(nav.grid().class_of(5, 5), Some(CellClass::Flat));
}

#[test]
fn gentle_ramp_classifies_slope() {
    // 格长 1 m、相邻采样差 0.5 m → 坡度约 26.6°，在 45° 上限内、超过 1 mm 平地容差。
    let field = crafted_field(1.0, 6, 4, |i, _| i as f32 * 0.5);
    let nav = Nav::bake_with_field(&Scene::new(), &field, NavConfig::default()).expect("烘焙");
    let grid = nav.grid();
    for j in 0..4 {
        for i in 0..5 {
            assert_eq!(
                grid.class_of(i, j),
                Some(CellClass::Slope),
                "({i}, {j}) 与 +X 邻居有 0.5 m 高差，必须记 Slope"
            );
        }
        assert_eq!(
            grid.class_of(5, j),
            Some(CellClass::Flat),
            "第 5 列没有更高/更低的邻居，必须是 Flat"
        );
    }
}

#[test]
fn curb_above_slope_limit_classifies_step() {
    // 格长 0.5 m、单格落差 0.6 m：超过坡度上限（0.5 m）但没超过步高（0.6 m）→ 台阶。
    let field = crafted_field(0.5, 8, 3, |i, _| if i >= 4 { 0.6 } else { 0.0 });
    let cfg = NavConfig {
        step_height: 0.6,
        ..Default::default()
    };
    let nav = Nav::bake_with_field(&Scene::new(), &field, cfg).expect("烘焙");
    let grid = nav.grid();
    assert_eq!(
        grid.class_of(3, 1),
        Some(CellClass::Step),
        "0.6 m 的坎必须记 Step 而不是 Slope"
    );
    assert!(
        grid.can_step([3, 1], [4, 1]),
        "0.6 m 的坎在 0.6 m 步高下必须能跨"
    );
    assert_eq!(
        grid.class_of(7, 1),
        Some(CellClass::Flat),
        "台面上没有高差，必须是 Flat"
    );
}

#[test]
fn class_blocked_iff_not_walkable() {
    // D01 锁死的口径：Blocked 只表示「这一格不可走」，坡度/台阶各有自己的值。
    let mut scene = flat_scene();
    add_wall(&mut scene, "wall", 8.0, 3.0);
    let nav = Nav::bake(&scene).expect("烘焙");
    let grid = nav.grid();
    let [cols, rows] = grid.dims();
    let mut blocked = 0;
    for j in 0..rows {
        for i in 0..cols {
            let class = grid.class_of(i, j).expect("每格都有分类");
            let walkable = grid.is_walkable_cell(i, j);
            assert_eq!(
                class == CellClass::Blocked,
                !walkable,
                "({i}, {j}) 的 Blocked 必须与不可走位图一致（class={class:?}, walkable={walkable}）"
            );
            if class == CellClass::Blocked {
                blocked += 1;
                for (di, dj) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let ni = i as i32 + di;
                    let nj = j as i32 + dj;
                    if ni < 0 || nj < 0 || ni >= cols as i32 || nj >= rows as i32 {
                        continue;
                    }
                    assert!(
                        !grid.can_step([i, j], [ni as u32, nj as u32]),
                        "Blocked 格 ({i}, {j}) 不该还能跨向 ({ni}, {nj})"
                    );
                }
            }
        }
    }
    assert!(blocked > 0, "墙必须制造出不可走格，否则这条断言没测到东西");
}

#[test]
fn too_steep_edge_does_not_mark_cell_blocked() {
    // 3 m 的断崖：两侧都还是「可走的地面」，只是过不去。
    // 这一条正是 D01 改掉旧口径的原因——旧实现把 TooSteep 记成了 Blocked。
    let field = crafted_field(1.0, 6, 3, |i, _| if i < 3 { 0.0 } else { 3.0 });
    let nav = Nav::bake_with_field(&Scene::new(), &field, NavConfig::default()).expect("烘焙");
    let grid = nav.grid();
    assert_eq!(
        grid.class_of(2, 0),
        Some(CellClass::Flat),
        "断崖这一侧仍是可走地面，必须记 Flat"
    );
    assert_eq!(grid.class_of(3, 0), Some(CellClass::Flat), "另一侧同理");
    assert!(!grid.can_step([2, 0], [3, 0]), "3 m 的断崖必须跨不过去");
    assert_eq!(nav.report().regions, 2, "断崖必须把场地切成两块");
    let hist = grid.class_histogram();
    assert_eq!(
        hist[CellClass::Blocked.code() as usize],
        0,
        "断崖不该把格子标成 Blocked"
    );
}
