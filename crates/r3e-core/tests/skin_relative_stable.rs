//! 扩展三十三（关节侧消伪脏）：关节矩阵沿「相对网格节点的局部链」求解。
//!
//! 背景：扩展三十二给关节矩阵加了脏段上传，但实测「骨骼不动、模型整体在转」时
//! 37 个关节全部被判脏、整张上传，收益归零。原因是求解式
//! `M_i = X⁻¹ · B_now · L_rest⁻¹` 里实例变换 `X` 参与运算，`X⁻¹ · X` 在浮点上
//! 留末位抖动 —— 数学上不变的矩阵位级变了，`memcmp` 只能判脏。
//!
//! 本层守三件事：
//!
//! 1. **刚体运动不产生伪脏**：资产实例整体旋转/平移后，关节矩阵与运动前**逐位相同**；
//! 2. **真变化不漏**：关节自身在网格局部空间里动了，矩阵必须跟着变（不是把变化吞掉）；
//! 3. **与旧路径数值一致**：相对链求解与 `mesh_world⁻¹ * joint_world` 的差 < 1e-5，
//!    不存在「为了稳定顺手改了画面」。
//!
//! 全程不开窗口、不建 GPU：只吃矩阵。

use glam::{Mat4, Quat, Vec3};
use r3e_core::{ObjectDesc, Scene, SkinInstance, Transform};

fn bits(m: Mat4) -> [u32; 16] {
    m.to_cols_array().map(f32::to_bits)
}

fn max_abs_diff(a: Mat4, b: Mat4) -> f32 {
    a.to_cols_array()
        .iter()
        .zip(b.to_cols_array().iter())
        .fold(0.0f32, |acc, (x, y)| acc.max((x - y).abs()))
}

/// 资产实例 `tiger`（骨骼树顶点）→ 关节链 `hips -> spine`；
/// 网格节点 `diaojingbaihu` 是**独立根**、自带同一份实例变换。
///
/// 这正是真实导入路径的结构（见 `app::spawn_asset_nodes`）：网格节点 `parent: None`、
/// 骨架各根节点挂在资产实例根下，两者没有 NodeId 意义上的公共祖先 —— 上一版测试
/// 让网格挂在 `asset` 下，掩盖了这个断链，导致「关节侧消伪脏」在真机上失效。
struct Rig {
    scene: Scene,
    asset: u64,
    mesh: u64,
    hips: u64,
    spine: u64,
}

impl Rig {
    fn new() -> Self {
        let mut scene = Scene::new();
        let asset = scene
            .spawn(ObjectDesc {
                name: "tiger".into(),
                visible: Some(false),
                ..ObjectDesc::default()
            })
            .expect("spawn asset");
        let mesh = scene
            .spawn(ObjectDesc {
                name: "diaojingbaihu".into(),
                mesh: Some("tiger".into()),
                parent: None,
                ..ObjectDesc::default()
            })
            .expect("spawn mesh");
        let hips = scene
            .spawn(ObjectDesc {
                name: "tiger/hips".into(),
                parent: Some("tiger".into()),
                ..ObjectDesc::default()
            })
            .expect("spawn hips");
        let spine = scene
            .spawn(ObjectDesc {
                name: "tiger/hips/spine".into(),
                parent: Some("tiger/hips".into()),
                ..ObjectDesc::default()
            })
            .expect("spawn spine");
        // 给骨骼一点静止姿态的局部偏移/旋转，避免单位阵把问题掩盖掉。
        if let Some(node) = scene.find_mut("tiger/hips") {
            node.transform = Transform::new(
                Vec3::new(0.0, 1.25, 0.0),
                Quat::from_euler(glam::EulerRot::XYZ, 0.15, -0.4, 0.05),
                Vec3::ONE,
            );
        }
        if let Some(node) = scene.find_mut("tiger/hips/spine") {
            node.transform = Transform::new(
                Vec3::new(0.0, 0.7, 0.1),
                Quat::from_euler(glam::EulerRot::XYZ, -0.2, 0.3, 0.1),
                Vec3::ONE,
            );
        }
        let mut rig = Self {
            scene,
            asset,
            mesh,
            hips,
            spine,
        };
        rig.bind();
        rig
    }

    /// 静止姿态下抓 `rest_world` 建蒙皮 + 固定相对基准（与资产导入路径一致）。
    fn bind(&mut self) {
        self.scene.update_world_transforms();
        let mesh_world = self.scene.get(self.mesh).expect("mesh 存在").world;
        let hips_world = self.scene.get(self.hips).expect("hips 存在").world;
        let spine_world = self.scene.get(self.spine).expect("spine 存在").world;
        let mut skin = SkinInstance::new("tiger", self.mesh);
        skin.add_joint(self.hips, 0, "hips", hips_world, mesh_world);
        skin.add_joint(self.spine, 1, "hips/spine", spine_world, mesh_world);
        let index = self.scene.add_skin(skin);
        assert!(
            self.scene.align_skin_frame(index),
            "静止姿态下应能固定相对基准"
        );
        self.scene.update_world_transforms();
    }

    /// 模拟资产实例整体变换：网格节点与骨骼树顶点同时吃同一份 TRS（`assets[].position/...`）。
    fn set_instance(&mut self, position: Vec3, rotation: Quat) {
        let transform = Transform::new(position, rotation, Vec3::ONE);
        for id in [self.asset, self.mesh] {
            if let Some(node) = self.scene.get_mut(id) {
                node.transform = transform;
            }
        }
        self.scene.update_world_transforms();
    }

    fn matrices(&self) -> Vec<Mat4> {
        self.scene
            .skin_of(self.mesh)
            .expect("蒙皮存在")
            .joint_matrices
            .clone()
    }
}

/// 网格节点在骨架树之外：真实结构下没有公共祖先，靠「骨骼树顶点 + 常量基准」接上。
#[test]
fn mesh_outside_skeleton_tree_has_no_common_ancestor() {
    let rig = Rig::new();
    assert!(
        rig.scene.relative_transform(rig.mesh, rig.hips).is_none(),
        "扁平结构下不应求出公共祖先相对变换（真实资产的形态）"
    );
    let (frame, anchor) = {
        let skin = rig.scene.skin_of(rig.mesh).expect("蒙皮");
        (skin.frame_local, skin.frame_anchor)
    };
    assert_eq!(anchor, Some(rig.asset), "相对基准应落在骨骼树顶点上");
    // 基准自身是常量：任何一帧都不该被改写
    let mut rig = rig;
    rig.set_instance(
        Vec3::new(3.0, 1.0, -2.0),
        Quat::from_euler(glam::EulerRot::XYZ, 0.4, 1.1, -0.7),
    );
    assert_eq!(
        bits(rig.scene.skin_of(rig.mesh).expect("蒙皮").frame_local),
        bits(frame),
        "frame_local 是常量，刚体运动后不得变动"
    );
}

/// 静止姿态下关节矩阵必须是单位阵（相对网格不动），顺带确认绑定没算反。
#[test]
fn rest_pose_yields_identity() {
    let rig = Rig::new();
    let mats = rig.matrices();
    assert_eq!(mats.len(), 2);
    for (i, m) in mats.iter().enumerate() {
        assert!(
            max_abs_diff(*m, Mat4::IDENTITY) < 1e-5,
            "第 {i} 个关节在静止姿态下应为单位阵，实际 {m:?}"
        );
    }
}

/// 核心验收：资产实例整体刚体运动（旋转 + 平移）后，关节矩阵**逐位**不变。
///
/// 这正是扩展三十二失效的场景：旧的世界矩阵路径下这里全脏。
#[test]
fn rigid_instance_motion_keeps_joint_matrices_bit_identical() {
    let mut rig = Rig::new();
    let before = rig.matrices();

    // 只动资产实例本身，骨骼与网格的局部变换一个字节都没改。
    let poses = [
        (0.0f32, 0.0f32, 0.0f32),
        (0.7, -1.1, 0.35),
        (2.9, 0.2, -1.4),
    ];
    for (i, rot) in poses.iter().enumerate() {
        rig.set_instance(
            Vec3::new(i as f32 * 3.5, 0.4 * i as f32, -1.2 * i as f32),
            Quat::from_euler(glam::EulerRot::XYZ, rot.0, rot.1, rot.2),
        );
        let after = rig.matrices();
        for (j, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            assert_eq!(
                bits(*b),
                bits(*a),
                "第 {i} 组实例变换 / 第 {j} 个关节：刚体运动产生了伪脏（位级变了）\nbefore {b:?}\nafter {a:?}"
            );
        }
    }
}

/// 真变化不能漏：关节在网格局部空间里动了，矩阵必须跟着变。
#[test]
fn joint_local_motion_still_changes_matrices() {
    let mut rig = Rig::new();
    let before = rig.matrices();

    if let Some(node) = rig.scene.find_mut("tiger/hips/spine") {
        node.transform = Transform::new(
            Vec3::new(0.25, 0.7, 0.1),
            Quat::from_euler(glam::EulerRot::XYZ, 0.55, 0.3, 0.1),
            Vec3::ONE,
        );
    }
    rig.scene.update_world_transforms();

    let after = rig.matrices();
    assert_eq!(
        bits(before[0]),
        bits(after[0]),
        "没动的 hips 关节不应被带动"
    );
    assert_ne!(
        bits(before[1]),
        bits(after[1]),
        "动了的 spine 关节必须产生真实变化"
    );
    assert!(
        max_abs_diff(before[1], after[1]) > 1e-3,
        "spine 关节变化量太小，等于把真变化吞掉了"
    );
}

/// 与旧路径（世界矩阵相减）数值一致：差 < 1e-5，说明稳定化没改画面。
#[test]
fn relative_path_agrees_with_world_path() {
    let mut rig = Rig::new();
    rig.set_instance(
        Vec3::new(4.0, -2.0, 1.5),
        Quat::from_euler(glam::EulerRot::XYZ, 0.6, -1.2, 0.4),
    );
    if let Some(node) = rig.scene.find_mut("tiger/hips/spine") {
        node.transform = Transform::new(
            Vec3::new(0.1, 0.9, -0.2),
            Quat::from_euler(glam::EulerRot::XYZ, 0.3, 0.8, -0.5),
            Vec3::ONE,
        );
    }
    rig.scene.update_world_transforms();

    // 相对链求解结果（场景已算好）
    let relative = rig.matrices();
    // 旧世界矩阵路径：拿同一帧的世界矩阵重算一遍
    let mesh_world = rig.scene.get(rig.mesh).expect("mesh").world;
    let worlds = |id: u64| rig.scene.get(id).map(|n| n.world);
    let mut legacy = rig.scene.skin_of(rig.mesh).expect("蒙皮").clone();
    legacy.solve(mesh_world, worlds);

    assert_eq!(relative.len(), legacy.joint_matrices.len());
    for (i, (r, l)) in relative
        .iter()
        .zip(legacy.joint_matrices.iter())
        .enumerate()
    {
        assert!(
            max_abs_diff(*r, *l) < 1e-5,
            "第 {i} 个关节：相对链结果与世界矩阵路径差 {:e}",
            max_abs_diff(*r, *l)
        );
    }
}

/// 关节与网格各自独立成根（无公共祖先）时，回退到世界矩阵路径，仍要能解出矩阵。
#[test]
fn disconnected_joint_falls_back_to_world_path() {
    let mut scene = Scene::new();
    let mesh = scene
        .spawn(ObjectDesc {
            name: "mesh".into(),
            mesh: Some("tiger".into()),
            ..ObjectDesc::default()
        })
        .expect("spawn mesh");
    let lonely = scene
        .spawn(ObjectDesc {
            name: "lonely".into(),
            position: Some([1.0, 2.0, 3.0]),
            ..ObjectDesc::default()
        })
        .expect("spawn lonely");
    scene.update_world_transforms();

    let mesh_world = scene.get(mesh).expect("mesh").world;
    let joint_world = scene.get(lonely).expect("lonely").world;
    let mut skin = SkinInstance::new("odd", mesh);
    skin.add_joint(lonely, 0, "lonely", joint_world, mesh_world);
    // 树内无公共祖先 -> 相对链返回 None
    assert!(
        scene.relative_transform(mesh, lonely).is_none(),
        "两个独立根节点不应求出相对变换"
    );
    scene.add_skin(skin);
    scene.update_world_transforms();

    let mats = &scene.skin_of(mesh).expect("蒙皮").joint_matrices;
    assert_eq!(mats.len(), 1);
    assert!(
        max_abs_diff(mats[0], Mat4::IDENTITY) < 1e-5,
        "回退路径下静态绑定仍应是单位阵，实际 {:?}",
        mats[0]
    );
}
