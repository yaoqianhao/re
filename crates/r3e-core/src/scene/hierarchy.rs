//! 场景图的层次查询：根节点回溯与两个节点之间的相对变换。

use super::*;

impl Scene {
    /// 只沿局部链求 `anchor⁻¹ · joint`（不经过任何世界矩阵）。
    ///
    /// 要求 `anchor` 是 `joint` 的祖先；成立时结果为纯局部矩阵累乘，
    /// 与资产实例变换 `X` 无关 —— 整体刚体运动下逐位不变，脏段上传因此不会误判。
    ///
    /// 返回 `None` 表示 `anchor` 不在 `joint` 的父链上、父链成环或超过深度上限。
    pub fn relative_transform_from(&self, anchor: NodeId, joint: NodeId) -> Option<Mat4> {
        const MAX_DEPTH: usize = 64;
        let mut chain: Vec<Mat4> = Vec::new();
        let mut cur = joint;
        let mut reached = false;
        for _ in 0..MAX_DEPTH {
            if cur == anchor {
                reached = true;
                break;
            }
            let node = self.nodes.get(&cur)?;
            chain.push(node.transform.local_matrix());
            match node.parent {
                Some(p) if p != cur => cur = p,
                _ => return None,
            }
        }
        if !reached {
            return None;
        }
        // `chain` 是「关节自身 → anchor 的直接子节点」，倒过来才是从根往下的累乘顺序。
        let mut rel = Mat4::IDENTITY;
        for m in chain.iter().rev() {
            rel *= *m;
        }
        Some(rel)
    }

    /// 节点所在父子树的顶点（一路沿 `parent` 上溯到的根）。
    pub fn tree_root(&self, node: NodeId) -> Option<NodeId> {
        const MAX_DEPTH: usize = 64;
        let mut cur = node;
        for _ in 0..MAX_DEPTH {
            let n = self.nodes.get(&cur)?;
            match n.parent {
                Some(p) if p != cur => cur = p,
                _ => return Some(cur),
            }
        }
        None
    }

    /// 关节相对网格节点的变换（等价于 `mesh_world⁻¹ * joint_world`），
    /// 但只沿两者之间的**局部**链路累乘，不经过资产实例变换。
    ///
    /// 走世界矩阵相减时，`X⁻¹ · X` 在浮点上会留下末位抖动：实例整体旋转、骨骼没动，
    /// 关节矩阵却位级变了（扩展三十二的脏段上传因此失效）。沿局部链求相对变换后，
    /// 结果与实例变换无关，「刚体整体运动」也能保持位级稳定。
    ///
    /// 返回 `None` 表示两者不在同一父子树内、父链成环或超过深度上限，
    /// 调用方应回退到世界矩阵相减。
    ///
    /// 注意：真实导入路径里网格节点是独立根、骨骼挂在资产实例根下，两者**没有**
    /// 公共祖先，这时本方法返回 `None`；蒙皮求解走 [`Self::relative_transform_from`]
    /// 加 `frame_local` 常量的路径（见 `SkinInstance::frame_anchor`）。
    pub fn relative_transform(&self, mesh: NodeId, joint: NodeId) -> Option<Mat4> {
        const MAX_DEPTH: usize = 64;
        // 关节上溯链（含自身），用于找公共祖先。
        let mut up: Vec<NodeId> = Vec::new();
        let mut cur = joint;
        for _ in 0..MAX_DEPTH {
            self.nodes.get(&cur)?;
            up.push(cur);
            match self.nodes.get(&cur)?.parent {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        // 从网格节点上溯，找到第一个落在关节链上的节点即公共祖先。
        let mut ancestor = None;
        let mut cur = mesh;
        for _ in 0..MAX_DEPTH {
            if let Some(pos) = up.iter().position(|id| *id == cur) {
                ancestor = Some((cur, pos));
                break;
            }
            match self.nodes.get(&cur)?.parent {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        let (ancestor, joint_pos) = ancestor?;

        // 关节侧：从紧邻公共祖先那层往下累乘到关节自身。
        let mut rel = Mat4::IDENTITY;
        for i in (0..joint_pos).rev() {
            let id = up[i];
            rel *= self.nodes.get(&id)?.transform.local_matrix();
        }
        // 网格侧：公共祖先到网格节点之间的链取逆，左乘上去即可抵消。
        if ancestor != mesh {
            let mut chain: Vec<NodeId> = Vec::new();
            let mut cur = mesh;
            for _ in 0..MAX_DEPTH {
                chain.push(cur);
                match self.nodes.get(&cur)?.parent {
                    Some(p) if p != cur && p != ancestor => cur = p,
                    _ => break,
                }
            }
            let mut to_mesh = Mat4::IDENTITY;
            for i in (0..chain.len()).rev() {
                to_mesh *= self.nodes.get(&chain[i])?.transform.local_matrix();
            }
            rel = to_mesh.inverse() * rel;
        }
        Some(rel)
    }
}
