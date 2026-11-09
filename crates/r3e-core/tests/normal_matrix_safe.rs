//! 扩展二十六：世界法线矩阵的奇异保护。
//!
//! 背景：`Scene::update_world_transforms` 原来直接写
//! `Mat3::from_mat4(world).inverse().transpose()`。只要父链上任何一节某轴缩放为 0
//! （缩放进场动画的起点、把物件压扁临时隐藏、脚本手滑传 0），世界矩阵就退化成奇异矩阵，
//! 求逆后的每个分量都是 `NaN` / `inf`；`NaN` 一旦进了法线，光照、SSAO、快照一起废掉。
//! 着色器侧同理——伴随矩阵整块归零后 `normalize(0)` 直接产 `NaN`。
//!
//! 这一层守三件事：
//!
//! 1. **正常路径一位不差**：非退化 TRS 上，保护版与朴素 `inverse().transpose()` 逐位相同，
//!    不存在「加个保护顺手改了画面」的可能；
//! 2. **退化路径必须有限**：某轴缩放为 0 / 极小 / `NaN` 时，结果每个分量都必须是有限值；
//! 3. **负缩放方向不丢**：镜像物件的法线要跟着翻，不能被 `max(1e-6)` 抹成与正缩放同向。
//!
//! 全程不开窗口、不建 GPU：只吃矩阵。

use glam::{Mat3, Mat4, Quat, Vec3};
use r3e_core::{
    safe_scale_component, world_normal_matrix, ObjectDesc, Scene, Transform, MIN_LINEAR_SCALE,
    SINGULAR_DET_EPS,
};

/// 朴素（无保护）写法，用来对照正常路径。
fn naive(world: Mat4) -> Mat3 {
    Mat3::from_mat4(world).inverse().transpose()
}

fn is_finite(m: Mat3) -> bool {
    m.to_cols_array().iter().all(|v| v.is_finite())
}

fn bits(m: Mat3) -> [u32; 9] {
    m.to_cols_array().map(f32::to_bits)
}

/// 确定性伪随机：测试要能复现，不引 rand。
struct Lcg(u32);

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / 16_777_216.0
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// 造一条「父 → 子」链，spawn 之后再直接改 `transform.scale`：
/// 这样 0 / NaN 这类 spawn 校验可能拦下的值也能落到节点上，
/// 与脚本运行时把缩放算成 0 的真实路径一致。
fn chain(parent_scale: Vec3, child_scale: Vec3) -> Scene {
    let mut scene = Scene::new();
    scene
        .spawn(ObjectDesc {
            name: "parent".into(),
            mesh: Some("cube".into()),
            scale: Some([parent_scale.x, parent_scale.y, parent_scale.z]),
            ..ObjectDesc::default()
        })
        .expect("spawn parent");
    scene
        .spawn(ObjectDesc {
            name: "child".into(),
            mesh: Some("cube".into()),
            scale: Some([child_scale.x, child_scale.y, child_scale.z]),
            parent: Some("parent".into()),
            ..ObjectDesc::default()
        })
        .expect("spawn child");
    scene
}

fn set_scale(scene: &mut Scene, name: &str, scale: Vec3) {
    let node = scene.find_mut(name).expect("节点存在");
    node.transform.scale = scale;
}

/// 正常路径：随机 200 组非退化 TRS，保护版必须与朴素逆**逐位**相同。
#[test]
fn normal_path_is_bit_identical_to_naive_inverse() {
    let mut rng = Lcg(0x5eed_1234);
    for i in 0..200 {
        let scale = Vec3::new(
            rng.range(0.2, 3.5),
            rng.range(0.2, 3.5),
            rng.range(0.2, 3.5),
        );
        let rotation = Quat::from_euler(
            glam::EulerRot::XYZ,
            rng.range(-3.0, 3.0),
            rng.range(-3.0, 3.0),
            rng.range(-3.0, 3.0),
        );
        let position = Vec3::new(
            rng.range(-10.0, 10.0),
            rng.range(-10.0, 10.0),
            rng.range(-10.0, 10.0),
        );
        let world = Mat4::from_scale_rotation_translation(scale, rotation, position);
        let guarded = world_normal_matrix(world);
        assert_eq!(
            bits(guarded),
            bits(naive(world)),
            "第 {i} 组：保护版在非退化输入上偏离了朴素逆——正常画面不该有丝毫变化"
        );
    }
}

/// 单轴缩放到 0：结果必须全有限，且退化轴按最小缩放放大而不是 `inf`。
#[test]
fn zero_axis_scale_is_finite_and_uses_min_scale() {
    let world = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));
    assert!(!is_finite(naive(world)), "朴素写法在这里本来就是 NaN/inf");

    let guarded = world_normal_matrix(world);
    assert!(is_finite(guarded), "退化缩放必须给出有限法线矩阵");
    // y 轴方向退化：方向退回单位 Y，缩放倒数取 1 / MIN_LINEAR_SCALE
    let y = guarded * Vec3::Y;
    assert!(
        (y.y - 1.0 / MIN_LINEAR_SCALE).abs() <= 1.0 / MIN_LINEAR_SCALE * 1e-3,
        "退化轴的缩放倒数应为 1/MIN_LINEAR_SCALE，实际 {}",
        y.y
    );
    // 健康的 x 轴仍是 1
    let x = guarded * Vec3::X;
    assert!((x.x - 1.0).abs() < 1e-6, "x 轴不该被牵连，实际 {}", x.x);
}

/// 三轴全 0（整条链被压成一点）：仍然要有限。
#[test]
fn all_axes_zero_is_finite() {
    let guarded = world_normal_matrix(Mat4::from_scale(Vec3::ZERO));
    assert!(is_finite(guarded));
}

/// NaN / inf / 极小缩放：都要有限，绝不把 NaN 传出去。
#[test]
fn nan_inf_and_tiny_scale_are_finite() {
    let cases = [
        ("nan", Vec3::new(f32::NAN, 1.0, 1.0)),
        ("inf", Vec3::new(f32::INFINITY, 1.0, 1.0)),
        ("tiny", Vec3::new(1e-9, 1e-9, 1e-9)),
        ("mixed", Vec3::new(1.0, 0.0, -0.0)),
    ];
    for (label, scale) in cases {
        let world = Mat4::from_scale(scale);
        let guarded = world_normal_matrix(world);
        assert!(is_finite(guarded), "{label} 缩放产出了非有限法线矩阵");
    }
}

/// 极小缩放落在 det 阈值之内，说明确实走了退化分支而不是被 `inverse()` 放大成 1e18 噪声。
#[test]
fn tiny_scale_falls_into_degenerate_branch() {
    let world = Mat4::from_scale(Vec3::splat(1e-5));
    let det = Mat3::from_mat4(world).determinant();
    assert!(
        det.abs() < SINGULAR_DET_EPS,
        "1e-5 三次方应低于阈值，实际 det = {det}"
    );
    let guarded = world_normal_matrix(world);
    let longest = guarded
        .to_cols_array()
        .iter()
        .fold(0.0f32, |acc, v| acc.max(v.abs()));
    assert!(
        longest <= 1.0 / MIN_LINEAR_SCALE * 1.001,
        "退化分支不该把微小缩放放大到 {} 以上",
        1.0 / MIN_LINEAR_SCALE
    );
}

/// 负缩放（镜像）：`Transform::normal_matrix` 必须保留符号。
/// 旧写法 `scale.max(1e-6)` 会把 -2 变成 1e-6，法线被放大一百万倍且方向反掉。
#[test]
fn negative_scale_keeps_mirror() {
    let transform = Transform {
        scale: Vec3::new(-2.0, 1.0, 1.0),
        ..Transform::IDENTITY
    };
    let m = transform.normal_matrix();
    let x = m * Vec3::X;
    assert!(
        (x.x + 0.5).abs() < 1e-5,
        "镜像轴的逆转置应为 -0.5，实际 {}",
        x.x
    );

    // 世界层面：det 非零，走正常逆，方向同样保留
    let world = Mat4::from_scale(Vec3::new(-2.0, 1.0, 1.0));
    let guarded = world_normal_matrix(world);
    assert_eq!(bits(guarded), bits(naive(world)));
    let world_x = (guarded * Vec3::X).x;
    assert!(
        (world_x + 0.5).abs() < 1e-5,
        "世界层面的镜像轴同样应为 -0.5，实际 {world_x}"
    );
}

/// `safe_scale_component` 的夹紧规则。
#[test]
fn safe_scale_component_rules() {
    assert_eq!(safe_scale_component(2.5), 2.5);
    assert_eq!(safe_scale_component(-2.5), -2.5);
    assert_eq!(safe_scale_component(0.0), MIN_LINEAR_SCALE);
    assert_eq!(safe_scale_component(-1e-9), -MIN_LINEAR_SCALE);
    assert_eq!(safe_scale_component(f32::NAN), MIN_LINEAR_SCALE);
    assert_eq!(safe_scale_component(f32::INFINITY), MIN_LINEAR_SCALE);
}

/// 场景集成：父节点被压到 0 / NaN、子节点正常——刷完世界矩阵后，
/// 全场景的 `world_normal` 都必须有限。
#[test]
fn scene_world_normal_stays_finite() {
    for bad in [
        Vec3::new(0.0, 1.0, 1.0),
        Vec3::splat(0.0),
        Vec3::new(f32::NAN, 1.0, 1.0),
    ] {
        let mut scene = chain(Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 1.0, 1.0));
        set_scale(&mut scene, "parent", bad);
        scene.update_world_transforms();

        for node in scene.iter() {
            assert!(
                is_finite(node.world_normal),
                "缩放 {bad:?} 下节点 {} 的 world_normal 出现非有限值",
                node.name
            );
        }
    }
}

/// 场景集成：正常缩放的父子链上，`world_normal` 必须与朴素逆逐位一致——
/// 保证保护逻辑没有偷偷改动健康场景。
#[test]
fn scene_world_normal_matches_naive_on_healthy_chain() {
    let mut rng = Lcg(0xabc_9876);
    for i in 0..50 {
        let parent = Vec3::new(
            rng.range(0.4, 2.5),
            rng.range(0.4, 2.5),
            rng.range(0.4, 2.5),
        );
        let child = Vec3::new(
            rng.range(0.4, 2.5),
            rng.range(0.4, 2.5),
            rng.range(0.4, 2.5),
        );
        let mut scene = chain(parent, child);
        scene.update_world_transforms();

        for node in scene.iter() {
            assert_eq!(
                bits(node.world_normal),
                bits(naive(node.world)),
                "第 {i} 组 / 节点 {}：健康链上的 world_normal 被改动了",
                node.name
            );
        }
    }
}
