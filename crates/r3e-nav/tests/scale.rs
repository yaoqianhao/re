//! D05 判据卡：**路径长度 ≤ 直线 × 阈值**（默认 20 组 / seed 42 / 1.6×，三项均可覆盖）。
//!
//! 这里断言的是**判据本身**，不是某一组具体数字：换夹具只要障碍还是「缓坡 + 稀疏」，
//! 结论就得一样。所以每条断言都写成「方向」（超限为空 / 收紧阈值只会更容易红 /
//! 非法配置必须被拒），不写死某个比值。

use glam::Vec3;
use r3e_core::{ObjectDesc, Scene, SpawnDecl, TerrainPatch};
use r3e_nav::{
    measure, Nav, PathOutcome, PathScale, ScaleError, DEFAULT_SCALE_CASES, DEFAULT_SCALE_LIMIT,
    DEFAULT_SCALE_SEED,
};

/// 判据夹具：48×48 缓坡地面（沿 +X 抬高 1.5 m ≈ 1.8°）+ 12 个 2×2 稀疏障碍。
///
/// 障碍刻意做小、做稀：判据考的是「路有没有绕得离谱」，不是「能不能穿过迷宫」。
/// 迷宫式的夹具会把比值推到 2 以上，那是夹具结论不是路径质量结论。
fn scale_scene() -> Scene {
    let mut scene = Scene::new();
    let mut patch = TerrainPatch::flat("ground", [0.0, 0.0], [48.0, 48.0]);
    let (rx, ry) = (33u32, 33u32);
    patch.resolution = [rx, ry];
    let mut heights = Vec::with_capacity((rx * ry) as usize);
    for _j in 0..ry {
        for i in 0..rx {
            heights.push(i as f32 * (1.5 / (rx - 1) as f32));
        }
    }
    patch.heights = heights;
    scene.terrain.push(patch);
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    // 障碍落点：坐标 × 0.03125 就是该处地面高度，箱体底座贴着地面放。
    // 密度按「绕一下就能过」控制：太稀判据测不出东西，太密会把比值顶到 2 以上。
    for (name, x, z) in [
        ("box_a", 12.0f32, 10.0f32),
        ("box_b", 30.0, 26.0),
        ("box_c", 38.0, 12.0),
        ("box_d", 18.0, 38.0),
        ("box_e", 8.0, 30.0),
        ("box_f", 42.0, 36.0),
        ("box_g", 22.0, 18.0),
        ("box_h", 26.0, 8.0),
        ("box_i", 14.0, 24.0),
        ("box_j", 34.0, 32.0),
        ("box_k", 6.0, 16.0),
        ("box_l", 40.0, 24.0),
    ] {
        let ground = x * 0.03125;
        scene
            .spawn(ObjectDesc {
                name: name.to_string(),
                mesh: Some("cube".to_string()),
                position: Some([x, ground + 1.0, z]),
                scale: Some([2.0, 2.0, 2.0]),
                ..Default::default()
            })
            .expect("障碍可生成");
    }
    scene.update_world_transforms();
    scene
}

fn flat_scene(size: f32) -> Scene {
    let mut scene = Scene::new();
    scene
        .terrain
        .push(TerrainPatch::flat("ground", [0.0, 0.0], [size, size]));
    scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
    scene.update_world_transforms();
    scene
}

/// 沿 Z 切一刀的墙：`z_center` 中心、`z_len` 长度。
fn add_wall(scene: &mut Scene, name: &str, x: f32, z_center: f32, z_len: f32) {
    scene
        .spawn(ObjectDesc {
            name: name.to_string(),
            mesh: Some("cube".to_string()),
            position: Some([x, 1.5, z_center]),
            scale: Some([0.2, 3.0, z_len]),
            ..Default::default()
        })
        .expect("墙可生成");
    scene.update_world_transforms();
}

/// 判据 1：默认三项（20 组 / seed 42 / 1.6×）是**契约数字**，改动等于改判据。
#[test]
fn default_scale_config_is_frozen() {
    let scale = PathScale::default();
    assert_eq!(DEFAULT_SCALE_CASES, 20, "默认组数是卡面契约");
    assert_eq!(DEFAULT_SCALE_SEED, 42, "默认 seed 是卡面契约");
    assert_eq!(
        DEFAULT_SCALE_LIMIT.to_bits(),
        1.6f32.to_bits(),
        "默认阈值是卡面契约"
    );
    assert_eq!(scale.cases, DEFAULT_SCALE_CASES);
    assert_eq!(scale.seed, DEFAULT_SCALE_SEED);
    assert_eq!(scale.limit.to_bits(), DEFAULT_SCALE_LIMIT.to_bits());

    // 默认配置必须自己就是合法的；`allows` 的边界与 NaN 口径一并钉住。
    assert_eq!(scale.validate(), Ok(()));
    assert!(scale.allows(1.6), "等于阈值算通过（≤ 而非 <）");
    assert!(!scale.allows(1.600_1), "超过阈值必须不通过");
    assert!(!scale.allows(f32::NAN), "NaN 必须判不通过，不许静默恒绿");
    assert!(!scale.allows(f32::INFINITY));
}

/// 判据 1：默认跑一遍，20 组全部评到、无一组超 1.6×，且同配置重跑逐位相同。
#[test]
fn default_twenty_cases_stay_within_one_point_six() {
    let nav = Nav::bake(&scale_scene()).expect("缓坡 + 稀疏障碍可烘焙");
    let report = measure(&nav, &PathScale::default()).expect("默认配置合法");

    assert_eq!(
        report.cases(),
        20,
        "默认必须评满 20 组：{}",
        report.summary()
    );
    assert!(
        report.complete(),
        "未凑满组数等于判据不成立：{}",
        report.summary()
    );
    assert!(
        report.within(),
        "默认 1.6× 判据必须通过：{}",
        report.summary()
    );
    assert!(
        report.max_ratio() <= DEFAULT_SCALE_LIMIT,
        "最大比值越界：{}",
        report.summary()
    );

    let cell = nav.report().cell_size;
    for s in &report.samples {
        assert!(
            s.ratio.is_finite() && s.ratio > 0.0,
            "第 {} 组比值非法：{}",
            s.index,
            s.ratio
        );
        assert!(
            s.length + 1e-3 >= s.straight,
            "第 {} 组路径 ({}) 短于直线 ({})，几何上不可能",
            s.index,
            s.length,
            s.straight
        );
        // 抽样间距下限是判据的一部分：太近的点比值没有意义。
        assert!(
            s.straight >= 16.0 * cell - 1e-3,
            "第 {} 组直线只有 {} m，低于抽样间距下限",
            s.index,
            s.straight
        );
        // 比值口径钉死：就是 路径长度 / 直线距离，不许有别的算法。
        assert_eq!(
            s.ratio.to_bits(),
            (s.length / s.straight).to_bits(),
            "第 {} 组比值不是 length/straight",
            s.index
        );
    }
    println!("D05 默认判据：{}", report.summary());

    // 确定性：同配置重跑必须逐位相同（点集 / 每个比值 / 跳过次数）。
    let again = measure(&nav, &PathScale::default()).expect("合法");
    assert_eq!(
        report.points_digest, again.points_digest,
        "同 seed 点集必须相同"
    );
    assert_eq!(report.skipped, again.skipped);
    for (x, y) in report.samples.iter().zip(again.samples.iter()) {
        assert_eq!(
            x.ratio.to_bits(),
            y.ratio.to_bits(),
            "第 {} 组比值漂了",
            x.index
        );
        assert_eq!(x.length.to_bits(), y.length.to_bits());
        assert_eq!(
            x.from.to_array().map(f32::to_bits),
            y.from.to_array().map(f32::to_bits)
        );
    }
}

/// 判据 2：三项一起覆盖（40 组 / seed 7 / 1.4×），覆盖后仍按**同方向**断言。
#[test]
fn override_to_forty_cases_seed_seven_tighter_limit() {
    let nav = Nav::bake(&scale_scene()).expect("烘焙成功");
    let base = measure(&nav, &PathScale::default()).expect("合法");
    let scale = PathScale::new(40, 7, 1.4).expect("覆盖值合法");
    let report = measure(&nav, &scale).expect("覆盖值合法");

    assert_eq!(report.cases(), 40, "组数覆盖没生效：{}", report.summary());
    assert!(report.complete(), "{}", report.summary());
    assert_eq!(report.scale.seed, 7);
    assert_eq!(report.scale.limit.to_bits(), 1.4f32.to_bits());
    // 同方向断言：更紧的阈值配上新点集，仍然要「无一组超限」才算过。
    assert!(
        report.within(),
        "覆盖后 1.4× 判据必须通过：{}",
        report.summary()
    );
    assert!(
        report.max_ratio() <= 1.4,
        "覆盖后最大比值越界：{}",
        report.summary()
    );
    assert_ne!(
        report.points_digest, base.points_digest,
        "覆盖 seed 后点集必须变"
    );

    // 只换 seed（组数与阈值不变）也必须换点集，否则 seed 覆盖形同虚设。
    let seed_only = measure(&nav, &PathScale::new(40, 8, 1.4).expect("合法")).expect("合法");
    assert_ne!(
        seed_only.points_digest, report.points_digest,
        "只换 seed 也要换点集"
    );
}

/// 判据 2：只覆盖组数时是**截断**而不是重抽——前 N 组必须与默认跑逐位相同。
#[test]
fn cases_override_truncates_instead_of_resampling() {
    let nav = Nav::bake(&scale_scene()).expect("烘焙成功");
    let base = measure(&nav, &PathScale::default()).expect("合法");
    let five = measure(
        &nav,
        &PathScale::new(5, DEFAULT_SCALE_SEED, DEFAULT_SCALE_LIMIT).expect("合法"),
    )
    .expect("合法");

    assert_eq!(five.cases(), 5, "组数覆盖没生效：{}", five.summary());
    assert!(five.complete());
    assert!(five.within(), "{}", five.summary());
    assert_ne!(
        five.points_digest, base.points_digest,
        "5 组的点集哈希必须与 20 组不同"
    );
    for i in 0..5 {
        assert_eq!(
            five.samples[i].from.to_array().map(f32::to_bits),
            base.samples[i].from.to_array().map(f32::to_bits),
            "第 {i} 组起点变了：组数覆盖只能是截断"
        );
        assert_eq!(
            five.samples[i].to.to_array().map(f32::to_bits),
            base.samples[i].to.to_array().map(f32::to_bits),
            "第 {i} 组终点变了：组数覆盖只能是截断"
        );
    }
}

/// 判据 2：阈值只参与比较、不参与测量——收紧到必红时，测量值一位都不许变。
#[test]
fn tightened_limit_flips_verdict_without_touching_measurement() {
    let nav = Nav::bake(&scale_scene()).expect("烘焙成功");
    let loose = measure(&nav, &PathScale::new(20, 42, 1.6).expect("合法")).expect("合法");
    // 0.5 一定全红：任何路径长度都 ≥ 直线距离。
    let strict = measure(&nav, &PathScale::new(20, 42, 0.5).expect("合法")).expect("合法");

    assert_eq!(
        loose.points_digest, strict.points_digest,
        "换阈值不许换点集"
    );
    assert_eq!(loose.samples.len(), strict.samples.len());
    for (x, y) in loose.samples.iter().zip(strict.samples.iter()) {
        assert_eq!(
            x.length.to_bits(),
            y.length.to_bits(),
            "第 {} 组测量值被阈值影响了",
            x.index
        );
        assert_eq!(x.ratio.to_bits(), y.ratio.to_bits());
    }
    assert!(loose.within(), "1.6× 应通过：{}", loose.summary());
    assert!(
        !strict.within(),
        "0.5× 必须红（同方向断言的反面）：{}",
        strict.summary()
    );
    assert_eq!(strict.exceeded().len(), 20, "0.5× 下 20 组必须全超限");
}

/// 判据 3：`cases == 0` 必须被拒（0 组没有判据可言）。
#[test]
fn zero_cases_is_rejected() {
    let err = PathScale::new(0, 42, 1.6).expect_err("0 组必须被拒");
    assert_eq!(err, ScaleError::ZeroCases);
    assert_eq!(err.as_str(), "zeroCases");
    assert_eq!(err.describe(), "判据组数必须 ≥ 1（0 组没有判据可言）");
    assert_eq!(
        err.to_string(),
        err.describe(),
        "Display 必须与冻结文案一致"
    );

    // 绕过 `new` 手改字段也得在校验处拦住，而且 `measure` 一个格子都不许跑。
    let hand_made = PathScale {
        cases: 0,
        ..PathScale::default()
    };
    assert_eq!(hand_made.validate(), Err(ScaleError::ZeroCases));
    let nav = Nav::bake(&flat_scene(8.0)).expect("烘焙成功");
    assert_eq!(measure(&nav, &hand_made), Err(ScaleError::ZeroCases));
}

/// 判据 3：`limit ≤ 0` 必须被拒。
#[test]
fn non_positive_limit_is_rejected() {
    for limit in [0.0f32, -0.0, -1.5] {
        let err = PathScale::new(20, 42, limit).expect_err("非正阈值必须被拒");
        assert_eq!(err, ScaleError::NonPositiveLimit, "limit = {limit}");
        assert_eq!(err.as_str(), "nonPositiveLimit");
        assert_eq!(err.describe(), "阈值比例必须 > 0（路径长度不小于直线距离）");
    }
}

/// 判据 3：`NaN` / `±∞` 必须被拒——它们会让「超限为空」静默恒成立。
#[test]
fn non_finite_limit_is_rejected() {
    for limit in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let err = PathScale::new(20, 42, limit).expect_err("非有限阈值必须被拒");
        assert_eq!(err, ScaleError::NonFiniteLimit, "limit = {limit}");
        assert_eq!(err.as_str(), "nonFiniteLimit");
        assert_eq!(err.describe(), "阈值比例必须是有限数（NaN / 无穷大不接受）");
    }
}

/// 判据 4：不可达按约定处理——报 `Unreachable` + 断点，且**绝不**混进长度判据。
#[test]
fn unreachable_pairs_are_reported_then_skipped() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall", 8.0, 8.0, 16.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");
    assert_eq!(nav.report().regions, 2, "一堵到底的墙必须分成两块");

    let from = Vec3::new(2.0, 0.0, 2.0);
    let to = Vec3::new(13.0, 0.0, 2.0);
    match nav.path(from, to) {
        PathOutcome::Unreachable {
            from_region,
            to_region,
        } => assert_ne!(from_region, to_region, "不可达必须是跨块"),
        other => panic!("跨块必须报 Unreachable，实际：{other:?}"),
    }
    assert!(!nav.reachable(from, to));
    assert!(
        nav.disconnect_point(from, to).is_some(),
        "不可达必须给出断点坐标，否则关卡作者无处补路"
    );

    // 长度判据侧：跨块的对只能算「跳过」，不许进样本（否则会出现 0 或无穷比值）。
    let report = measure(&nav, &PathScale::default()).expect("合法");
    assert!(
        report.cases() > 0,
        "单块内的对仍须能凑出样本：{}",
        report.summary()
    );
    assert!(
        report.skipped > 0,
        "跨块的重抽必须有记录：{}",
        report.summary()
    );
    for s in &report.samples {
        assert!(
            nav.reachable(s.from, s.to),
            "第 {} 组居然是不可达对，不该进样本",
            s.index
        );
        assert!(s.ratio.is_finite());
    }
}

/// 判据 5：直线被墙切断时必须**绕行而不是穿墙**，且绕行仍在阈值内。
#[test]
fn blocked_straight_line_is_detoured_not_crossed() {
    let mut scene = flat_scene(16.0);
    add_wall(&mut scene, "wall_a", 8.0, 3.0, 6.0);
    add_wall(&mut scene, "wall_b", 8.0, 13.0, 6.0);
    let nav = Nav::bake(&scene).expect("烘焙成功");

    let from = Vec3::new(2.0, 0.0, 2.0);
    let to = Vec3::new(13.0, 0.0, 2.0);
    let points = match nav.path(from, to) {
        PathOutcome::Found(p) => p,
        other => panic!("有 4 m 缺口就必须能过去：{other:?}"),
    };
    assert!(
        points.len() > 2,
        "绕缺口必须有拐点，实际只有 {} 点",
        points.len()
    );

    let straight = from.distance(to);
    let length = r3e_nav::polyline_length(&points);
    assert!(
        length > straight * 1.05,
        "直线被墙切断，路径 ({length}) 不能几乎等于直线 ({straight})"
    );
    let ratio = length / straight;
    assert!(
        ratio <= DEFAULT_SCALE_LIMIT,
        "绕行 {ratio:.3} 超出默认阈值，判据会假红"
    );

    // 绕行不许穿过墙体：逐段密集采样，每一步都得站在可走格上。
    let cell = nav.report().cell_size;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let steps = (a.distance(b) / (cell * 0.5)).ceil() as usize + 1;
        for k in 0..=steps {
            let p = a.lerp(b, k as f32 / steps as f32);
            assert!(nav.is_walkable(p), "绕行段穿过了墙：{p:?}");
        }
    }
}
