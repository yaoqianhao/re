//! 形变（morph）绑定、权重、掩码与独占组的场景侧读写。

use super::*;

impl Scene {
    /// 登记一条形变绑定（同一网格节点重复登记时覆盖旧权重），返回其下标。
    ///
    /// 渲染层每帧从 `Scene::morphs` 取权重写进实例缓冲，因此脚本改权重后
    /// 下一帧就生效，不需要重建缓冲或重新上传网格。
    pub fn add_morph(&mut self, morph: MorphInstance) -> usize {
        if let Some(index) = self
            .morphs
            .iter()
            .position(|m| m.mesh_node == morph.mesh_node)
        {
            self.morphs[index] = morph;
            return index;
        }
        self.morphs.push(morph);
        self.morphs.len() - 1
    }

    /// 按网格节点找形变绑定。
    pub fn morph_of(&self, mesh_node: NodeId) -> Option<&MorphInstance> {
        self.morphs.iter().find(|m| m.mesh_node == mesh_node)
    }

    /// 按网格节点改一处权重。节点没有绑定、下标越界或权重非有限值都返回 `false`。
    pub fn set_morph_weight(&mut self, mesh_node: NodeId, index: usize, weight: f32) -> bool {
        match self.morphs.iter_mut().find(|m| m.mesh_node == mesh_node) {
            Some(morph) => morph.set_weight(index, weight),
            None => false,
        }
    }

    /// 按物件名改一处权重（脚本侧便利入口）。
    pub fn set_morph_weight_by_name(&mut self, name: &str, index: usize, weight: f32) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        self.set_morph_weight(node, index, weight)
    }

    /// 把某个物件名下的全部形变权重清零。返回是否真的找到并清过。
    pub fn reset_morph(&mut self, name: &str) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => {
                morph.reset();
                true
            }
            None => false,
        }
    }

    /// 按物件名登记形变绑定（脚本侧便利入口）。
    ///
    /// `target_count` 是该网格实际存在的形状数，权重只对前这么多个分量有意义；
    /// 传 0 表示「这个网格没有形状」，此时不建绑定、直接返回 `false`。
    /// 已存在的绑定会被覆盖，权重回到全 0。
    pub fn bind_morph_by_name(&mut self, name: &str, target_count: usize) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        if target_count == 0 {
            return false;
        }
        self.add_morph(MorphInstance::new(node, target_count));
        true
    }

    /// 按物件名读一处权重；没有绑定或下标越界时返回 `None`。
    pub fn morph_weight_by_name(&self, name: &str, index: usize) -> Option<f32> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.weight_of(index)
    }

    /// 按物件名的绑定形状数；没有绑定返回 `None`。
    pub fn morph_target_count(&self, name: &str) -> Option<usize> {
        let node = self.find_id(name)?;
        self.morph_of(node).map(|m| m.target_count)
    }

    /// 扩展三十八：把物件名的第 `index` 个形状划进互斥组 / 从组里取出。
    ///
    /// 同一组内的形状本帧权重之和超过 1 时，渲染层会等比压回 1 —— 用来表达
    /// 「张嘴和闭嘴不能同时 100%」这类表情互斥。`group` 传
    /// [`NO_EXCLUSIVE_GROUP`] 表示取出。返回是否找到绑定且下标合法。
    pub fn set_morph_exclusive_by_name(&mut self, name: &str, index: usize, group: u8) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        let slot = if group == NO_EXCLUSIVE_GROUP {
            None
        } else {
            Some(group)
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => morph.set_exclusive_group(index, slot),
            None => false,
        }
    }

    /// 扩展三十八：读某个形状所在的互斥组；没有绑定 / 下标越界返回 `None`。
    pub fn morph_exclusive_by_name(&self, name: &str, index: usize) -> Option<u8> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.exclusive_group_of(index)
    }

    /// 扩展三十八：读归一化之后的权重（脚本可据此断言「被压回 1」）。
    ///
    /// 与渲染层剔除用的是**同一个纯函数**，因此脚本里看到的数就是进缓冲的数。
    pub fn morph_normalized_weight_by_name(&self, name: &str, index: usize) -> Option<f32> {
        let node = self.find_id(name)?;
        let morph = self.morph_of(node)?;
        if index >= morph.target_count {
            return None;
        }
        let (weights, _) = morph.normalized_weights();
        weights.get(index).copied()
    }

    /// 扩展四十：读某个实例的生效形状掩码；没有绑定返回 `None`。
    ///
    /// 返回值已按 `target_count` 裁剪 —— 脚本写进去的越界位不会回显。
    pub fn morph_mask_by_name(&self, name: &str) -> Option<u32> {
        let node = self.find_id(name)?;
        Some(self.morph_of(node)?.shape_mask())
    }

    /// 扩展四十：整块替换形状掩码，返回裁剪后的生效值；没有绑定返回 `None`。
    pub fn set_morph_mask_by_name(&mut self, name: &str, mask: u32) -> Option<u32> {
        let node = self.find_id(name)?;
        let morph = self.morphs.iter_mut().find(|m| m.mesh_node == node)?;
        Some(morph.set_shape_mask(mask))
    }

    /// 扩展四十：开 / 关单个形状；没有绑定或下标越界返回 `false`。
    pub fn set_morph_shape_enabled_by_name(
        &mut self,
        name: &str,
        index: usize,
        enabled: bool,
    ) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => morph.set_shape_enabled(index, enabled),
            None => false,
        }
    }

    /// 扩展四十：读某个形状是否启用；没有绑定或下标越界返回 `None`。
    pub fn morph_shape_enabled_by_name(&self, name: &str, index: usize) -> Option<bool> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.shape_enabled(index)
    }

    /// 丢弃指向已删除节点的形变绑定（与 `prune_skins` 同策）。
    pub fn prune_morphs(&mut self) {
        if self.morphs.is_empty() {
            return;
        }
        self.morphs
            .retain(|m| self.nodes.contains_key(&m.mesh_node));
    }
}
