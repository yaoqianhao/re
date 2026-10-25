//! 骨骼蒙皮（P4）：关节绑定、逆绑定矩阵与每帧关节矩阵求解。
//!
//! 这一层只放**与图形 API 无关**的数据与数学：
//!
//! * [`SkinData`] —— 资产侧的原始蒙皮描述（哪些 FBX 节点是关节）；
//! * [`SkinInstance`] —— 场景侧的运行实例（关节已绑到真实 `NodeId`，
//!   并在绑定时抓到「静止姿态世界矩阵」作为逆绑定矩阵）；
//! * [`SkinInstance::solve`] —— 每帧把「当前关节世界矩阵」折算成着色器要的蒙皮矩阵。
//!
//! # 顶点空间约定（与 P2/P3 一脉相承）
//!
//! 资产层导入网格时已把顶点烘焙到**世界空间**（`geometry_to_world` 之后），
//! 且这份坐标是「资产实例变换之前」的：设 `p` 为顶点缓冲里的位置，`X` 为
//! 网格节点世界矩阵，则静止姿态下顶点应落在 `X * p`（与 P3 静态画面一致）。
//!
//! 设 `B_i` 为关节世界矩阵，绑定时记下网格局部空间的绑定姿态
//!
//! ```text
//! L_rest_i = X_rest⁻¹ * B_rest_i
//! ```
//!
//! 每帧关节矩阵取
//!
//! ```text
//! M_i = X⁻¹ * B_now_i * L_rest_i⁻¹
//! ```
//!
//! 渲染层算 `world = X * (Σ w_i M_i p)`。静止姿态下 `B_now = B_rest`，
//! `world = X * p`；整体平移 / 旋转实例时 `X` 与 `B` 同步变化，`M_i` 仍为单位阵，
//! 网格跟着走而不变形 —— 这正是 P4 的数值验收口径。

use glam::Mat4;

use crate::NodeId;

/// 每顶点最多参与蒙皮的关节数（与 `Vertex::joints/weights` 长度一致）。
pub const MAX_JOINTS_PER_VERTEX: usize = 4;

/// 单帧关节矩阵总槽位上限（GPU storage buffer 容量）。
///
/// 超出时渲染层会截断并告警；老虎样本只用 37 个，余量充足。
pub const MAX_JOINT_MATRICES: usize = 4096;

/// 资产侧的一个关节：指向 FBX 节点树里的某个节点。
#[derive(Debug, Clone, Default)]
pub struct SkinJoint {
    /// 在 `AssetEntry::nodes` 中的下标（等价于动画轨道里的 `node_index`）。
    pub node_index: u32,
    /// `Root/Arm/Hand` 形式的节点路径，用于日志与脚本定位。
    pub path: String,
    /// `cluster.bind_to_world`，仅作元数据保留，不参与求解。
    ///
    /// 引擎的逆绑定矩阵取自「场景静止姿态世界矩阵」。`skin_probe` 实测：老虎样本用
    /// `bind_to_world⁻¹` 时，即使动画停在第 0 帧，1018 个顶点仍全体偏移（max 0.6715）；
    /// 改用静止姿态逆矩阵后同一帧偏移为 0 —— 说明该资产的 bind 姿态与节点默认姿态不一致，
    /// 以导入位姿为静止姿态才能保证「不动画时画面与 P3 一致」。
    pub bind_world: Mat4,
}

/// 资产侧的蒙皮描述：一个变形器的一组关节。
#[derive(Debug, Clone, Default)]
pub struct SkinData {
    pub name: String,
    pub joints: Vec<SkinJoint>,
}

impl SkinData {
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }
}

/// 场景侧的一条关节绑定：FBX 节点已落到真实场景节点上。
#[derive(Debug, Clone)]
pub struct SkinBinding {
    pub node: NodeId,
    /// 来源 FBX 节点下标（日志用）。
    pub node_index: u32,
    pub path: String,
    /// 绑定时抓到的静止姿态世界矩阵（含资产实例变换）。
    pub rest_world: Mat4,
    /// 逆绑定矩阵 = `L_rest⁻¹`，其中 `L_rest = X_rest⁻¹ * rest_world`
    /// 是关节在网格局部空间的绑定姿态。
    pub inverse_bind: Mat4,
}

/// 场景侧的一个蒙皮实例：驱动某个网格节点的顶点形变。
#[derive(Debug, Clone)]
pub struct SkinInstance {
    pub name: String,
    /// 使用本蒙皮的网格节点（渲染层据此把关节矩阵槽位塞进实例数据）。
    pub mesh_node: NodeId,
    pub bindings: Vec<SkinBinding>,
    /// 每帧求解出的蒙皮矩阵，顺序与 `bindings` 一致，也是顶点的关节下标空间。
    pub joint_matrices: Vec<Mat4>,
}

impl SkinInstance {
    pub fn new(name: impl Into<String>, mesh_node: NodeId) -> Self {
        Self {
            name: name.into(),
            mesh_node,
            bindings: Vec::new(),
            joint_matrices: Vec::new(),
        }
    }

    /// 追加一条关节绑定。
    ///
    /// `rest_world` 是关节的静止姿态世界矩阵，`mesh_world` 是同一时刻网格节点的
    /// 世界矩阵；两者相除得到网格局部空间的绑定姿态，再取逆作为逆绑定矩阵。
    pub fn add_joint(
        &mut self,
        node: NodeId,
        node_index: u32,
        path: impl Into<String>,
        rest_world: Mat4,
        mesh_world: Mat4,
    ) {
        let rest_local = mesh_world.inverse() * rest_world;
        self.bindings.push(SkinBinding {
            node,
            node_index,
            path: path.into(),
            rest_world,
            inverse_bind: rest_local.inverse(),
        });
    }

    pub fn joint_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// 求解本帧关节矩阵。
    ///
    /// * `mesh_world` —— 网格节点的当前世界矩阵（资产实例变换）；
    /// * `world_of` —— 由 `NodeId` 取当前世界矩阵（关节节点被销毁时返回 `None`，
    ///   该关节退化为单位矩阵，避免整网格塌到原点）。
    pub fn solve<F>(&mut self, mesh_world: Mat4, world_of: F)
    where
        F: Fn(NodeId) -> Option<Mat4>,
    {
        let inv_model = mesh_world.inverse();
        self.joint_matrices.clear();
        self.joint_matrices.reserve(self.bindings.len());
        for b in &self.bindings {
            let now = world_of(b.node).unwrap_or(Mat4::IDENTITY);
            self.joint_matrices.push(inv_model * now * b.inverse_bind);
        }
    }

    /// 关节矩阵相对单位阵的最大偏移量（数值验收用：静止姿态应≈0）。
    pub fn max_deviation(&self) -> f32 {
        let mut max = 0.0f32;
        for m in &self.joint_matrices {
            let d = (*m - Mat4::IDENTITY).to_cols_array();
            for v in d {
                max = max.max(v.abs());
            }
        }
        max
    }

    /// 本帧是否已经求解过（矩阵数与关节数一致且有值）。
    pub fn is_solved(&self) -> bool {
        !self.bindings.is_empty() && self.joint_matrices.len() == self.bindings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn translation(x: f32, y: f32, z: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, y, z))
    }

    /// 静止姿态：顶点落在 `X * p`，与 P3 静态画面一致（实例平移不能丢）。
    #[test]
    fn rest_pose_keeps_vertices_at_model_space() {
        let mut skin = SkinInstance::new("s", 1);
        let model = translation(5.0, 0.0, 0.0);
        skin.add_joint(
            2,
            0,
            "Root/Joint",
            model * translation(0.0, 2.0, 0.0),
            model,
        );
        skin.solve(model, |id| {
            assert_eq!(id, 2);
            Some(model * translation(0.0, 2.0, 0.0))
        });
        let p = Vec3::new(1.0, 2.0, 3.0);
        let out = model.transform_point3(skin.joint_matrices[0].transform_point3(p));
        let want = model.transform_point3(p);
        assert!(
            (out - want).length() < 1e-5,
            "静止姿态应等于模型变换后的位置: {out:?}"
        );
    }

    /// 绑定后又整体平移实例：网格跟着走且不产生形变（M = I）。
    #[test]
    fn instance_translation_does_not_deform() {
        let mut skin = SkinInstance::new("s", 1);
        let rest_model = Mat4::IDENTITY;
        let joint_rest = translation(0.0, 1.0, 0.0);
        skin.add_joint(2, 0, "Root/Joint", joint_rest, rest_model);

        let moved = translation(3.0, 0.0, 0.0);
        skin.solve(moved, |_| Some(moved * joint_rest));
        assert!(skin.max_deviation() < 1e-5, "整体平移不应产生形变");

        let p = Vec3::new(0.0, 1.0, 0.0);
        let out = moved.transform_point3(skin.joint_matrices[0].transform_point3(p));
        assert!((out - Vec3::new(3.0, 1.0, 0.0)).length() < 1e-5, "{out:?}");
    }

    /// 关节相对网格运动：顶点按位移量跟随。
    #[test]
    fn bone_motion_moves_vertices_by_delta() {
        let mut skin = SkinInstance::new("s", 1);
        skin.add_joint(
            2,
            0,
            "Root/Joint",
            translation(0.0, 1.0, 0.0),
            Mat4::IDENTITY,
        );
        skin.solve(Mat4::IDENTITY, |_| Some(translation(0.0, 3.0, 0.0)));
        let p = Vec3::new(0.0, 1.0, 0.0);
        let out = skin.joint_matrices[0].transform_point3(p);
        assert!((out - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-5, "{out:?}");
        assert!(skin.max_deviation() > 1.0);
    }

    /// 实例缩放后仍以静止姿态为基准：M = I，顶点落在缩放后的位置。
    #[test]
    fn instance_scale_does_not_deform() {
        let mut skin = SkinInstance::new("s", 1);
        let rest_model = Mat4::IDENTITY;
        let joint_rest = translation(0.0, 2.0, 0.0);
        skin.add_joint(2, 0, "Root/Joint", joint_rest, rest_model);

        let scaled = Mat4::from_scale(Vec3::splat(2.0));
        skin.solve(scaled, |_| Some(scaled * joint_rest));
        assert!(skin.max_deviation() < 1e-5, "整体缩放不应产生形变");
        let p = Vec3::new(0.0, 1.0, 0.0);
        let out = scaled.transform_point3(skin.joint_matrices[0].transform_point3(p));
        assert!((out - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-5, "{out:?}");
    }
}
