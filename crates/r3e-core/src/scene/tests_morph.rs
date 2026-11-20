//! 形变（morph）的场景侧测试：独占组投射与掩码读写。

use super::*;

/// 扩展三十八：场景侧只负责「把组号落到实例上」，归一化本身是纯函数，
/// 这里断言的是**读到的东西**与**传给渲染层的东西**一致。
#[test]
fn scene_carries_exclusive_groups_to_the_instance() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "face".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    assert!(scene.bind_morph_by_name("face", 2));
    assert!(scene.set_morph_weight_by_name("face", 0, 1.0));
    assert!(scene.set_morph_weight_by_name("face", 1, 1.0));

    // 没设组时：两个形状各自独立，归一化后仍是 1.0 / 1.0
    assert_eq!(scene.morph_exclusive_by_name("face", 0), None);
    assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 1.0).abs() < 1e-6);

    // 同一组：组内和 2.0 被等比压回 1.0，各得 0.5
    assert!(scene.set_morph_exclusive_by_name("face", 0, 0));
    assert!(scene.set_morph_exclusive_by_name("face", 1, 0));
    assert_eq!(scene.morph_exclusive_by_name("face", 0), Some(0));
    let a = scene.morph_normalized_weight_by_name("face", 0).unwrap();
    let b = scene.morph_normalized_weight_by_name("face", 1).unwrap();
    assert!((a - 0.5).abs() < 1e-6, "实际 {a}");
    assert!((b - 0.5).abs() < 1e-6, "实际 {b}");
    // 场景原权重要原样留着，脚本调参会读自己写进去的值
    assert_eq!(scene.morph_weight_by_name("face", 0), Some(1.0));

    // 哨兵值 = 取出该形状，取出后回到互不影响
    assert!(scene.set_morph_exclusive_by_name("face", 1, NO_EXCLUSIVE_GROUP));
    assert_eq!(scene.morph_exclusive_by_name("face", 1), None);
    assert!((scene.morph_normalized_weight_by_name("face", 1).unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn exclusive_api_rejects_unknown_names_and_bad_index() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "face".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    // 没有绑定时一律拒绝
    assert!(!scene.set_morph_exclusive_by_name("face", 0, 0));
    assert_eq!(scene.morph_exclusive_by_name("face", 0), None);
    assert_eq!(scene.morph_normalized_weight_by_name("face", 0), None);
    assert!(!scene.set_morph_exclusive_by_name("nobody", 0, 0));

    assert!(scene.bind_morph_by_name("face", 2));
    // 超出 target_count 的下标：写不进、读不到
    assert!(!scene.set_morph_exclusive_by_name("face", 5, 0));
    assert_eq!(scene.morph_exclusive_by_name("face", 5), None);
    assert_eq!(scene.morph_normalized_weight_by_name("face", 5), None);
}

/// 履历：`target_count = 0` 的网格不建绑定，互斥组自然也无处可落。
#[test]
fn bind_morph_rejects_zero_targets() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "block".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    assert!(!scene.bind_morph_by_name("block", 0));
    assert!(!scene.set_morph_exclusive_by_name("block", 0, 0));
}

fn face_scene(shapes: usize) -> Scene {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "face".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    assert!(scene.bind_morph_by_name("face", shapes));
    scene
}

/// 契约一：默认全启用，读写都在 `target_count` 范围内。
#[test]
fn scene_mask_defaults_to_all_enabled() {
    let scene = face_scene(3);
    assert_eq!(scene.morph_mask_by_name("face"), Some(0b111));
    assert_eq!(scene.morph_shape_enabled_by_name("face", 0), Some(true));
    assert_eq!(
        scene.morph_shape_enabled_by_name("face", 3),
        None,
        "越界读不出「启用」"
    );
}

/// 契约二：整块写掩码时越界位被裁掉，回显的是**生效值**。
///
/// 脚本写 `0xFFFF` 是常见手滑（把 u32 当全 1 用），既不能报错也不能
/// 原样回显 —— 否则脚本读回来的数与渲染层吃的数不是一回事。
#[test]
fn scene_mask_write_returns_clipped_value() {
    let mut scene = face_scene(3);
    assert_eq!(scene.set_morph_mask_by_name("face", u32::MAX), Some(0b111));
    assert_eq!(scene.morph_mask_by_name("face"), Some(0b111));
}

/// 契约三：单形状开关落到实例上，且直接影响归一化结果。
///
/// 这是掩码与「权重置 0」的分界：关掉形状 1 后，形状 0 的权重不该再被
/// 那个看不见的形状拖去平分。
#[test]
fn scene_mask_toggle_affects_normalized_weights() {
    let mut scene = face_scene(2);
    assert!(scene.set_morph_weight_by_name("face", 0, 1.0));
    assert!(scene.set_morph_weight_by_name("face", 1, 1.0));
    assert!(scene.set_morph_exclusive_by_name("face", 0, 0));
    assert!(scene.set_morph_exclusive_by_name("face", 1, 0));
    // 同组各 1.0 → 各 0.5
    assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 0.5).abs() < 1e-6);

    assert!(scene.set_morph_shape_enabled_by_name("face", 1, false));
    assert_eq!(scene.morph_shape_enabled_by_name("face", 1), Some(false));
    assert!(
        (scene.morph_normalized_weight_by_name("face", 0).unwrap() - 1.0).abs() < 1e-6,
        "被掩码挡住后组里只剩形状 0 独自 1.0，不该再折半"
    );
    assert_eq!(
        scene.morph_normalized_weight_by_name("face", 1),
        Some(0.0),
        "掩码外的形状读到 0"
    );
    // 场景原权重不被动过
    assert_eq!(scene.morph_weight_by_name("face", 1), Some(1.0));

    assert!(scene.set_morph_shape_enabled_by_name("face", 1, true));
    assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 0.5).abs() < 1e-6);
}

/// 契约五：掩码是**实例级**的 —— 同一网格的两个节点各持一份，互不影响。
///
/// 这是扩展四十存在的理由：三十九之前 `count` 是网格级的，同网格所有实例
/// 共享同一套形状，想「这个实例只开两个形状」只能把权重置 0 假装 ——
/// 顶点照样过 8 次乘法。
#[test]
fn mask_is_per_instance_not_per_mesh() {
    let mut scene = Scene::new();
    for name in ["left", "right"] {
        scene
            .spawn(ObjectDesc {
                name: name.to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        assert!(scene.bind_morph_by_name(name, 3));
    }

    assert_eq!(scene.set_morph_mask_by_name("left", 0b011), Some(0b011));
    assert_eq!(scene.set_morph_mask_by_name("right", 0b110), Some(0b110));

    assert_eq!(scene.morph_mask_by_name("left"), Some(0b011));
    assert_eq!(scene.morph_mask_by_name("right"), Some(0b110));
    assert_eq!(scene.morph_shape_enabled_by_name("left", 0), Some(true));
    assert_eq!(scene.morph_shape_enabled_by_name("right", 0), Some(false));

    // 单形状开关也只动自己那一个实例
    assert!(scene.set_morph_shape_enabled_by_name("left", 1, false));
    assert_eq!(scene.morph_mask_by_name("left"), Some(0b001));
    assert_eq!(
        scene.morph_mask_by_name("right"),
        Some(0b110),
        "另一个实例的掩码不该被牵连"
    );
}

/// 契约四：没有绑定 / 名字不存在 / 下标越界都只返回 None 或 false，不静默改状态。
#[test]
fn scene_mask_rejects_unknown_targets() {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "block".to_string(),
            ..Default::default()
        })
        .expect("spawn 应成功");
    assert_eq!(scene.morph_mask_by_name("block"), None, "没绑定就没有掩码");
    assert_eq!(scene.set_morph_mask_by_name("block", 0), None);
    assert!(!scene.set_morph_shape_enabled_by_name("block", 0, false));
    assert_eq!(scene.morph_shape_enabled_by_name("block", 0), None);

    assert_eq!(scene.set_morph_mask_by_name("nobody", 0b1), None);
    assert!(scene.bind_morph_by_name("block", 2));
    assert!(
        !scene.set_morph_shape_enabled_by_name("block", 5, false),
        "越界下标拒绝"
    );
    assert_eq!(
        scene.morph_mask_by_name("block"),
        Some(0b11),
        "被拒的写不改变掩码"
    );
}
