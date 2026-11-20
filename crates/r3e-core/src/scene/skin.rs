//! 蒙皮实例的登记、骨骼帧对齐与权重刷新。

use super::*;

impl Scene {
    /// 登记一个蒙皮实例，返回其在 `Scene::skins` 中的下标。
    ///
    /// 约定：调用前场景节点应处于**静止姿态**（未推进动画），
    /// 关节的 `rest_world` 需要由调用方用当前世界矩阵填好（见 `add_joint`）。
    pub fn add_skin(&mut self, skin: SkinInstance) -> usize {
        self.skins.push(skin);
        self.skins.len() - 1
    }

    /// 按网格节点找蒙皮实例。
    pub fn skin_of(&self, mesh_node: NodeId) -> Option<&SkinInstance> {
        self.skins.iter().find(|s| s.mesh_node == mesh_node)
    }

    // ---------- 顶点形变（扩展三十六） ----------

    /// 用当前世界矩阵给第 `index` 个蒙皮固定「相对基准」（骨骼树顶点 + `frame_local`）。
    ///
    /// 必须在静止姿态调用；返回 `false` 表示没有可用关节或父链异常。
    /// [`Self::update_skinning`] 会对尚未固定的蒙皮自动调一次（惰性兜底）。
    pub fn align_skin_frame(&mut self, index: usize) -> bool {
        let (mesh_node, first) = match self.skins.get(index) {
            Some(skin) => match skin.bindings.first() {
                Some(b) => (skin.mesh_node, b.node),
                None => return false,
            },
            None => return false,
        };
        let Some(anchor) = self.tree_root(first) else {
            return false;
        };
        let (Some(anchor_world), Some(mesh_world)) = (
            self.nodes.get(&anchor).map(|n| n.world),
            self.nodes.get(&mesh_node).map(|n| n.world),
        ) else {
            return false;
        };
        let Some(skin) = self.skins.get_mut(index) else {
            return false;
        };
        skin.bind_frame(anchor, anchor_world, mesh_world);
        true
    }

    /// 求解所有蒙皮实例的关节矩阵。
    ///
    /// `update_world_transforms` 会自动调用本方法，一般不需要手动调。
    /// 关节节点被销毁时该关节退化为单位矩阵，不会让整块网格塌到原点。
    pub fn update_skinning(&mut self) {
        if self.skins.is_empty() {
            return;
        }
        // 惰性兜底：调用方没显式固定基准的蒙皮（例如脚本 API 直接建蒙皮），
        // 在第一次求解时以当前姿态为静止姿态固定一次，之后恒定复用。
        let pending_frames: Vec<usize> = (0..self.skins.len())
            .filter(|&i| self.skins[i].frame_anchor.is_none())
            .collect();
        for i in pending_frames {
            self.align_skin_frame(i);
        }
        let worlds: HashMap<NodeId, Mat4> = self
            .nodes
            .iter()
            .map(|(id, node)| (*id, node.world))
            .collect();
        // 先按「基准节点 → 关节的局部链」求解（读借用），再统一写回 skins，
        // 避免实例整体变换把 X⁻¹·X 的浮点抖动带进关节矩阵。
        let mut pending: Vec<Vec<Mat4>> = Vec::with_capacity(self.skins.len());
        for skin in &self.skins {
            let model = worlds
                .get(&skin.mesh_node)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            let inv_model = model.inverse();
            // 常量：网格局部空间 ← 骨骼树顶点空间。位级恒定，不会随实例变换抖动。
            let inv_frame = skin.frame_local.inverse();
            let mut rels = Vec::with_capacity(skin.bindings.len());
            for b in &skin.bindings {
                let rel = skin
                    .frame_anchor
                    .and_then(|anchor| self.relative_transform_from(anchor, b.node))
                    .map(|chain| inv_frame * chain)
                    .unwrap_or_else(|| {
                        // 断链（多个骨架根 / 节点被销毁）：回退世界矩阵相减，
                        // 结果正确但位级不稳定，脏段上传会保守地判全脏。
                        let now = worlds.get(&b.node).copied().unwrap_or(Mat4::IDENTITY);
                        inv_model * now
                    });
                rels.push(rel);
            }
            pending.push(rels);
        }
        for (skin, rels) in self.skins.iter_mut().zip(pending.iter()) {
            skin.solve_from_relative(rels);
        }
    }

    /// 移除绑定了已销毁节点的蒙皮实例（网格节点或任一关节没了就整条丢）。
    pub fn prune_skins(&mut self) {
        if self.skins.is_empty() {
            return;
        }
        let alive: std::collections::HashSet<NodeId> = self.nodes.keys().copied().collect();
        self.skins.retain(|s| {
            alive.contains(&s.mesh_node) && s.bindings.iter().all(|b| alive.contains(&b.node))
        });
    }

    /// 蒙皮统计：`(实例数, 关节总数)`，供日志与验收使用。
    pub fn skin_stats(&self) -> (usize, usize) {
        (
            self.skins.len(),
            self.skins.iter().map(|s| s.joint_count()).sum(),
        )
    }

    // ---------- 动画（P3） ----------
}
