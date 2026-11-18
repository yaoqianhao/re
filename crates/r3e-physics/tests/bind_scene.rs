//! `Physics::bind_scene` 的公共装配契约。
//!
//! 这些用例针对的是**下沉出来的公共入口**，不是某个后端内部实现，因此全部走
//! `Physics` 门面 + 公开类型。四件事必须成立，缺一件就说明装配还是「各写一份」：
//!
//! 1. 按 `physics` 段产出句柄表，未声明的节点一个都不碰；
//! 2. 重复调用幂等（第二次全是复用）；
//! 3. 物理关闭时**不产出**句柄，报告如实说「没装」；
//! 4. 碰撞体尺寸跟着网格几何走，不退回 `scale * 0.5`。
//!
//! 装配只读场景、句柄通过 `BindReport::bindings` 交回，写回由调用方完成——
//! 下面的 [`bind`] 就是运行时那半边的缩影：拿到表后写进 `Node::body`。

use glam::Vec3;
use r3e_core::procedural::{cube, ProceduralProvider};
use r3e_core::{Aabb, MeshData, MeshProvider, MeshRef, ObjectDesc, PhysicsDesc, Scene, Vertex};
use r3e_physics::{BindReport, BuiltinBackend, NullBackend, Physics, Ray};

fn dynamic() -> PhysicsDesc {
    PhysicsDesc {
        body: "dynamic".into(),
        mass: 1.0,
        restitution: None,
        friction: None,
        collider: None,
        half_extents: None,
        ccd: None,
    }
}

fn builtin_physics() -> Physics {
    // 地面放到 -100：本文件只关心装配，不关心落地。
    Physics::with_backend(Box::new(BuiltinBackend::new(
        Vec3::new(0.0, -9.81, 0.0),
        -100.0,
    )))
}

/// 「装配 → 写回」的完整一步：照运行时的做法，把报告里的句柄表落到 `Node::body`。
fn bind(physics: &mut Physics, scene: &mut Scene, provider: &dyn MeshProvider) -> BindReport {
    let report = physics.bind_scene(scene, provider);
    for &(id, handle) in &report.bindings {
        if let Some(node) = scene.get_mut(id) {
            node.body = Some(handle);
        }
    }
    report
}

fn spawn(scene: &mut Scene, name: &str, mesh: &str, physics: Option<PhysicsDesc>) -> u64 {
    scene
        .spawn(ObjectDesc {
            name: name.into(),
            mesh: Some(mesh.into()),
            position: Some([0.0, 0.0, 0.0]),
            scale: Some([1.0, 1.0, 1.0]),
            physics,
            ..Default::default()
        })
        .expect("spawn 应成功")
}

/// 只应答单个「大盒子」网格的来源：用来把碰撞体尺寸与单位立方体假设区分开。
struct BigBoxProvider {
    bounds: Aabb,
}

impl MeshProvider for BigBoxProvider {
    fn mesh_data(&self, _key: MeshRef) -> Option<MeshData> {
        let min = self.bounds.min;
        let max = self.bounds.max;
        let mut vertices = Vec::with_capacity(8);
        for i in 0..8u32 {
            let p = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            vertices.push(Vertex::new(p.to_array(), [0.0, 1.0, 0.0], [0.0, 0.0]));
        }
        Some(MeshData::new(vertices, vec![0, 1, 2, 0, 2, 3]))
    }
}

#[test]
fn binds_only_declared_nodes_and_writes_handles() {
    let mut scene = Scene::new();
    let body = spawn(&mut scene, "crate", "cube", Some(dynamic()));
    let plain = spawn(&mut scene, "deco", "cube", None);

    let mut physics = builtin_physics();
    let report = bind(&mut physics, &mut scene, &ProceduralProvider::new());

    assert!(report.enabled, "物理开着就必须真的装");
    assert_eq!(report.candidates, 1, "只该认出声明了 physics 的那个节点");
    assert_eq!(report.created, 1);
    assert_eq!(report.reused, 0);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.bound(), 1);
    assert!(report.simulated(), "内置后端申报了能力位，应当会模拟");

    assert!(scene.get(body).unwrap().body.is_some(), "句柄要写回节点");
    assert!(
        scene.get(plain).unwrap().body.is_none(),
        "没声明 physics 的节点不能凭空长出刚体"
    );
    assert_eq!(physics.body_count(), 1);
}

#[test]
fn second_bind_reuses_live_handles() {
    let mut scene = Scene::new();
    let id = spawn(&mut scene, "crate", "cube", Some(dynamic()));
    let mut physics = builtin_physics();
    let provider = ProceduralProvider::new();

    let first = bind(&mut physics, &mut scene, &provider);
    let handle = scene.get(id).unwrap().body.expect("首次应写入句柄");

    let second = bind(&mut physics, &mut scene, &provider);

    assert_eq!(second.created, 0, "重复调用不该新建刚体");
    assert_eq!(second.reused, 1);
    assert_eq!(second.bound(), 1);
    assert_eq!(physics.body_count(), 1, "后端里也不该多出一个");
    assert_eq!(
        scene.get(id).unwrap().body,
        Some(handle),
        "复用时句柄必须原样保留"
    );
    assert!(physics.has_body(r3e_physics::BodyHandle(handle)));
    assert!(first.created == 1);
}

#[test]
fn disabled_physics_writes_no_handle_and_says_so() {
    let mut scene = Scene::new();
    let id = spawn(&mut scene, "crate", "cube", Some(dynamic()));

    // Physics::new() 默认关闭。
    let mut physics = Physics::new();
    let report = bind(&mut physics, &mut scene, &ProceduralProvider::new());

    assert!(!report.enabled);
    assert_eq!(report.candidates, 0, "关着的时候连数都不该数");
    assert_eq!(report.bound(), 0);
    assert!(!report.simulated());
    assert!(
        scene.get(id).unwrap().body.is_none(),
        "关着还写句柄，会让上层以为装配成功"
    );
    assert!(report.summary().contains("未开启"));
}

#[test]
fn authoritative_null_backend_reports_without_claiming_simulation() {
    let mut scene = Scene::new();
    spawn(&mut scene, "crate", "cube", Some(dynamic()));

    let mut physics = Physics::with_backend(Box::new(NullBackend::new()));
    let report = bind(&mut physics, &mut scene, &ProceduralProvider::new());

    assert!(report.enabled, "空转后端是开着但什么都不做的后端");
    assert_eq!(report.created, 1, "句柄记账仍然要做，否则回放/编辑器对不上");
    assert!(
        !report.simulated(),
        "NullBackend 一位能力都不申报，不能声称在模拟"
    );
    assert!(report.summary().contains("不模拟"));
}

#[test]
fn stale_handle_is_destroyed_not_leaked() {
    let mut scene = Scene::new();
    let id = spawn(&mut scene, "crate", "cube", Some(dynamic()));
    // 模拟「切后端 / 清空过，节点上留了个后端不认的旧句柄」。
    scene.get_mut(id).unwrap().body = Some(999);

    let mut physics = builtin_physics();
    let report = bind(&mut physics, &mut scene, &ProceduralProvider::new());

    assert_eq!(report.created, 1);
    assert_eq!(report.reused, 0);
    assert_eq!(
        physics.body_count(),
        1,
        "旧句柄应先销毁，否则后端里留一条永远够不到的孤儿刚体"
    );
    assert_ne!(scene.get(id).unwrap().body, Some(999));
}

#[test]
fn manual_attach_is_not_overwritten() {
    let mut scene = Scene::new();
    let id = spawn(&mut scene, "crate", "cube", Some(dynamic()));

    let mut physics = builtin_physics();
    let provider = ProceduralProvider::new();
    // 脚本先用 attach_body 之类手动绑了一个（这里直接走装配一次再改回等价路径）。
    bind(&mut physics, &mut scene, &provider);
    let manual = scene.get(id).unwrap().body.expect("应有句柄");

    let report = bind(&mut physics, &mut scene, &provider);
    assert_eq!(report.reused, 1);
    assert_eq!(report.created, 0);
    assert_eq!(scene.get(id).unwrap().body, Some(manual));
}

#[test]
fn collider_size_follows_mesh_geometry() {
    let mut scene = Scene::new();
    let id = spawn(&mut scene, "wall", "cube", Some(dynamic()));

    // 8×2×6 的盒子（半尺寸 4×1×3），远超单位立方体。
    let provider = BigBoxProvider {
        bounds: Aabb::new(Vec3::new(-4.0, -1.0, -3.0), Vec3::new(4.0, 1.0, 3.0)),
    };
    let mut physics = builtin_physics();
    let report = bind(&mut physics, &mut scene, &provider);
    assert_eq!(report.created, 1);

    // 从 -Z 方向打进来：命中在 z = -3 处，distance ≈ 7。
    // 若退回 scale * 0.5，半尺寸只有 0.5，distance 会变成 9.5。
    let ray = Ray::new(Vec3::new(0.0, 0.0, -10.0), Vec3::Z);
    let hit = physics.raycast(ray, 30.0).expect("这么宽的盒子必须能打到");
    assert_eq!(hit.node, id);
    assert!(
        (hit.distance - 7.0).abs() < 0.2,
        "碰撞体应贴合网格几何，实测 distance = {}",
        hit.distance
    );

    // 侧面同理：从 -X 打进来命中在 x = -4，distance ≈ 6。
    let side = Ray::new(Vec3::new(-10.0, 0.0, 0.0), Vec3::X);
    let hit = physics.raycast(side, 30.0).expect("侧面也要能打到");
    assert!(
        (hit.distance - 6.0).abs() < 0.2,
        "X 轴半尺寸应取网格宽度，实测 distance = {}",
        hit.distance
    );
}

#[test]
fn unit_cube_default_still_works_without_provider_geometry() {
    let mut scene = Scene::new();
    spawn(&mut scene, "crate", "cube", Some(dynamic()));

    // 直接用内置图元网格（1×1×1），确认下沉后默认路径没退化。
    let mesh = cube();
    assert!(!mesh.aabb_box().is_empty(), "前置：内置立方体应有非空 AABB");

    let mut physics = builtin_physics();
    let report = bind(&mut physics, &mut scene, &ProceduralProvider::new());
    assert_eq!(report.created, 1);
    let hit = physics
        .raycast(Ray::new(Vec3::new(0.0, 0.0, -5.0), Vec3::Z), 20.0)
        .expect("默认路径必须仍可被射线命中");
    assert!(
        (hit.distance - 4.5).abs() < 0.2,
        "distance = {}",
        hit.distance
    );
}

#[test]
fn readonly_bind_scene_is_idempotent() {
    // 验收口径：验证器这类**只读**调用方连调两次，产出的句柄表必须一模一样。
    // 只读入口一旦偷偷改场景状态（或每次新建句柄），验证器与运行时就各说各话。
    let mut scene = Scene::new();
    let a = spawn(&mut scene, "a", "cube", Some(dynamic()));
    spawn(&mut scene, "static", "cube", None);

    let mut physics = builtin_physics();
    let provider = ProceduralProvider::new();

    let first = physics.bind_scene(&scene, &provider);
    let second = physics.bind_scene(&scene, &provider);

    assert_eq!(first.candidates, 1);
    assert_eq!(
        first.bindings, second.bindings,
        "两次只读装配的句柄表必须一致"
    );
    assert_eq!(second.created, 0, "第二次不该再新建刚体");
    assert_eq!(physics.body_count(), 1);
    assert!(
        scene.get(a).unwrap().body.is_none(),
        "只读入口不许回写场景，写回是调用方的事"
    );
}

#[test]
fn readonly_bind_scene_reports_unsimulated_backend() {
    // 后端不支持模拟时要如实回报，不能因为「报了句柄」就当装配成功。
    let mut scene = Scene::new();
    spawn(&mut scene, "a", "cube", Some(dynamic()));
    let mut physics = Physics::with_backend(Box::new(NullBackend::new()));
    physics.set_enabled(true);

    let report = physics.bind_scene(&scene, &ProceduralProvider::new());
    assert_eq!(report.bound(), 1, "Null 后端仍要登记句柄");
    assert!(!report.simulated(), "空转后端必须在报告里说不模拟");
    assert!(report.summary().contains("null") || report.summary().contains("后端"));
}
