//! CCD（连续碰撞检测）穿透对照测试：同一发子弹、同一面薄墙，只改 CCD 开关。
//!
//! 为什么靶体要用**运动学**薄板而不是静态墙：rapier 0.36 对固定碰撞体本来就做
//! 自动扫掠（`max_ccd_substeps != 0`），静态墙在 CCD 关掉时也未必会穿；
//! `enable_ccd(true)` 的增量价值恰好是「把扫掠扩到运动学 / 动态刚体」。
//! 所以只有拿运动学薄板做靶，才能观察到「关必穿、开必不穿」的真实差异。
//!
//! 这个测试不开窗口，纯数值：
//! * 关 CCD → 子弹直接飞过薄板（穿）；
//! * 开 CCD → 子弹被挡在板前（不穿）。

#![cfg(feature = "rapier")]

use glam::Vec3;
use r3e_core::scene::PhysicsDesc;
use r3e_core::{NodeId, ObjectDesc, Scene};
use r3e_physics::{BodyDesc, Physics, PhysicsConfig, PhysicsKind};

/// 薄板半厚度：0.02 → 整块 4cm，子弹一步要飞 5m，必然跨过去。
const PLATE_HALF: f32 = 0.02;
/// 子弹初速（m/s）；dt=0.05 时单步位移 5m。
const SPEED: f32 = 300.0;
const DT: f32 = 0.05;
const STEPS: usize = 20;
/// 地面沉到 -100m：本测试只关心墙，别让无限地面在 y=0 上蹭掉子弹的动量。
const GROUND: f32 = -100.0;

fn spawn(
    scene: &mut Scene,
    name: &str,
    mesh: &str,
    body: &str,
    pos: [f32; 3],
    half: [f32; 3],
    ccd: Option<bool>,
) -> NodeId {
    scene
        .spawn(ObjectDesc {
            name: name.into(),
            mesh: Some(mesh.into()),
            position: Some(pos),
            scale: Some([1.0, 1.0, 1.0]),
            physics: Some(PhysicsDesc {
                body: body.into(),
                mass: 1.0,
                restitution: None,
                friction: None,
                collider: Some(if mesh == "sphere" { "sphere" } else { "box" }.into()),
                half_extents: Some(half),
                ccd,
            }),
            ..Default::default()
        })
        .expect("spawn")
}

/// 跑一遍，返回子弹最终的 x 坐标。
fn shoot(ccd_on: bool) -> f32 {
    let mut scene = Scene::new();
    // 运动学薄板：位置由场景节点驱动，不会被子弹推走
    let plate = spawn(
        &mut scene,
        "plate",
        "cube",
        "kinematic",
        [0.0, 0.0, 0.0],
        [PLATE_HALF, 2.0, 2.0],
        None,
    );
    // 子弹：动态球，从 -2m 处朝 +x 冲
    let bullet = spawn(
        &mut scene,
        "bullet",
        "sphere",
        "dynamic",
        [-2.0, 0.0, 0.0],
        [0.05, 0.05, 0.05],
        Some(ccd_on),
    );
    scene.update_world_transforms();

    let mut physics = Physics::new();
    physics
        .apply_config(PhysicsConfig {
            enabled: Some(true),
            kind: Some(PhysicsKind::Rapier),
            gravity: Some(Vec3::ZERO),
            ground: Some(GROUND),
            ccd: Some(ccd_on),
        })
        .expect("rapier 后端应可用（本测试按 feature 条件编译）");
    assert!(physics.capabilities().ccd, "Rapier 必须申报 ccd 能力位");
    assert_eq!(physics.ccd_default(), ccd_on);

    let plate_desc = BodyDesc::from_node(scene.get(plate).unwrap()).expect("plate 有 physics 段");
    let plate_h = physics.try_create_body(plate, plate_desc).expect("建板");
    let bullet_desc =
        BodyDesc::from_node(scene.get(bullet).unwrap()).expect("bullet 有 physics 段");
    assert_eq!(bullet_desc.continuous, Some(ccd_on));
    let bullet_h = physics
        .try_create_body(bullet, bullet_desc)
        .expect("建子弹");

    // 逐刚体接口要能读回真实状态：运动学 / 固定体不参与扫掠，如实返回 false
    assert_eq!(physics.ccd_of(plate_h), Some(false), "运动学刚体不参与扫掠");
    assert_eq!(physics.ccd_of(bullet_h), Some(ccd_on));

    physics.apply_impulse(bullet_h, Vec3::new(SPEED, 0.0, 0.0));
    for _ in 0..STEPS {
        physics.step(DT, &mut scene);
    }
    scene.get(bullet).unwrap().transform.position.x
}

#[test]
fn ccd_off_tunnels_through_thin_plate() {
    let x = shoot(false);
    assert!(
        x > 1.0,
        "关掉 CCD 时子弹应穿过薄板（x 远大于 0），实际 x = {x}"
    );
}

#[test]
fn ccd_on_stops_at_thin_plate() {
    let x = shoot(true);
    assert!(
        x < 0.0,
        "开启 CCD 时子弹应被挡在板前（x < 0），实际 x = {x}"
    );
}

#[test]
fn ccd_toggle_is_reflected_by_backend() {
    let mut physics = Physics::new();
    physics
        .apply_config(PhysicsConfig {
            enabled: Some(true),
            kind: Some(PhysicsKind::Rapier),
            gravity: Some(Vec3::ZERO),
            ground: Some(GROUND),
            ccd: Some(false),
        })
        .expect("rapier 可用");
    assert!(!physics.ccd_default());

    let mut scene = Scene::new();
    let ball = spawn(
        &mut scene,
        "ball",
        "sphere",
        "dynamic",
        [0.0, 3.0, 0.0],
        [0.1, 0.1, 0.1],
        None,
    );
    scene.update_world_transforms();
    let h = physics
        .try_create_body(ball, BodyDesc::from_node(scene.get(ball).unwrap()).unwrap())
        .expect("建球");

    // 缺省关 → 新建刚体不带 CCD
    assert_eq!(physics.ccd_of(h), Some(false));
    // 全局打开：既改缺省值，也把现存刚体一起改掉
    assert!(physics.set_ccd_default(true));
    assert_eq!(physics.set_ccd_all(true), 1);
    assert_eq!(physics.ccd_of(h), Some(true));
    // 逐刚体关掉再读回
    assert!(physics.set_ccd(h, false));
    assert_eq!(physics.ccd_of(h), Some(false));
    // 不存在的句柄：如实返回失败，不静默假成功
    assert!(!physics.set_ccd(r3e_physics::BodyHandle(9999), true));
    assert_eq!(physics.ccd_of(r3e_physics::BodyHandle(9999)), None);
}
