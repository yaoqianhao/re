//! 动画图层的一/二/三维混合空间设置与权重求解。

use super::anim_layer::anim_layer_mut;
use super::*;

impl Scene {
    /// 给层挂一维混合空间（`param` 是参数名，例如 `"speed"`），样本表清空重建。
    ///
    /// 换维度 / 换参数名等于换了一张图，旧采样点留着只会错配，因此不保留。
    pub fn set_anim_layer_blend_space_1d(
        &mut self,
        instance: &str,
        layer: &str,
        param: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_1d(param))
    }

    /// 给层挂二维混合空间（`x` / `y` 两个参数名），样本表清空重建。
    pub fn set_anim_layer_blend_space_2d(
        &mut self,
        instance: &str,
        layer: &str,
        x: &str,
        y: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_2d(x, y))
    }

    /// 给层挂三维混合空间（`x` / `y` / `z` 三个参数名），样本表清空重建。
    ///
    /// 扩展四十九：把「八向移动 × 三档速度」这种过去要拿两层 2D 叠起来的表达，
    /// 收成一张三维单纯形图（四面体剖分 + 重心插值）。
    pub fn set_anim_layer_blend_space_3d(
        &mut self,
        instance: &str,
        layer: &str,
        x: &str,
        y: &str,
        z: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_3d(x, y, z))
    }

    /// 给层挂一个现成的混合空间（覆盖原有，连同样本一起）。
    pub fn set_anim_layer_blend_space(
        &mut self,
        instance: &str,
        layer: &str,
        space: BlendSpace,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.blend_space = Some(space);
                true
            }
            None => false,
        }
    }

    /// 摘掉层的混合空间。返回 `true` 表示确实摘掉了（层不存在或本来就没挂 → `false`）。
    ///
    /// 层内片段权重**不会**回滚成挂空间之前的值，而是冻结在最后一次由空间算出的结果上
    /// （想改手写值就接着调 `set_anim_layer_clip_weight`）。这样切换时不会突然跳一下。
    pub fn clear_anim_layer_blend_space(&mut self, instance: &str, layer: &str) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.blend_space.take().is_some(),
            None => false,
        }
    }

    /// 往层的混合空间加一个样本。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 样本被拒
    /// （片段名解析不到 / 坐标重复 / 非有限值 / 超过 16 个上限）。
    ///
    /// 片段名走 `find_anim_clip` 解析（可以只写实例前缀段），入库用**解析后的规范名**，
    /// 保证样本名与层内片段名同一口径，不会被前缀差异坑掉。
    pub fn add_anim_layer_blend_sample(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        at: [f32; 2],
    ) -> Option<bool> {
        // 先解析片段（只读借用到此结束），再取层与空间的可变借用
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.add_sample(c.name.clone(), at),
            None => {
                space.note_rejected();
                false
            }
        })
    }

    /// 设定混合空间参数（一维只认 `x`，`y` 传 0 即可）。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 参数非有限，整条拒绝、保持旧值。
    pub fn set_anim_layer_blend_value(
        &mut self,
        instance: &str,
        layer: &str,
        x: f32,
        y: f32,
    ) -> Option<bool> {
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        layer_ref.blend_space.as_mut().map(|s| s.set_value(x, y))
    }

    /// 往层的三维混合空间加一个样本（扩展四十九）。
    ///
    /// 与二维版同语义：`None` = 层不存在或没挂空间；`Some(false)` = 样本被拒
    /// （片段名解析不到 / 坐标重复 / 非有限值 / 超过上限，上限已由 16 抬到 32）。
    pub fn add_anim_layer_blend_sample_3d(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        at: [f32; 3],
    ) -> Option<bool> {
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.add_sample_3d(c.name.clone(), at),
            None => {
                space.note_rejected();
                false
            }
        })
    }

    /// 设定三维混合空间参数。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 参数非有限，整条拒绝、保持旧值。
    pub fn set_anim_layer_blend_value_3d(
        &mut self,
        instance: &str,
        layer: &str,
        x: f32,
        y: f32,
        z: f32,
    ) -> Option<bool> {
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        layer_ref
            .blend_space
            .as_mut()
            .map(|s| s.set_value_3d(x, y, z))
    }

    /// 混合空间所属维度（`"1d"` / `"2d"` / `"3d"`）。没挂空间返回 `None`。
    pub fn anim_layer_blend_dim(&self, instance: &str, layer: &str) -> Option<&'static str> {
        self.blend_space_of(instance, layer)
            .map(|s| s.dim().as_str())
    }

    /// 混合空间单行摘要（日志 / RPC 回显用）。没挂空间返回 `None`。
    pub fn anim_layer_blend_brief(&self, instance: &str, layer: &str) -> Option<String> {
        self.blend_space_of(instance, layer).map(|s| s.brief())
    }

    /// 当前参数下的权重（按名字合并、和为 1、零权重不出现）。没挂空间返回 `None`。
    pub fn anim_layer_blend_weights(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<Vec<(String, f32)>> {
        self.blend_space_of(instance, layer).map(|s| s.weights())
    }

    fn blend_space_of(&self, instance: &str, layer: &str) -> Option<&BlendSpace> {
        self.anim_layer(instance, layer)?.blend_space.as_ref()
    }

    /// 混合空间的剖分是否挂起（扩展五十一）。没挂空间返回 `None`。
    ///
    /// `true` = 样本加进来了但剖分还没提交。求值仍然精确（现场算临时拓扑），
    /// 编辑器据这个标记决定「要不要现在就画线框」——挂起时 `triangles()` 是空的。
    pub fn anim_layer_blend_stale(&self, instance: &str, layer: &str) -> Option<bool> {
        self.blend_space_of(instance, layer).map(|s| s.is_stale())
    }

    /// 把挂起的剖分一次性提交（扩展五十一）。没挂空间返回 `None`。
    ///
    /// 返回 `Some(true)` = 这次真的重建了、`Some(false)` = 本来就最新（没付钱）。
    /// 逐帧 advance 里也会自动提交一次（`AnimLayer::apply_blend_space`），
    /// 这个入口是给「批量装配完立刻画线框 / 立刻探针」的编辑器与脚本用的。
    pub fn build_anim_layer_blend_pending(&mut self, instance: &str, layer: &str) -> Option<bool> {
        let inst = self.anim_instance_mut(instance)?;
        let space = anim_layer_mut(inst, layer)?.blend_space.as_mut()?;
        Some(space.build_pending())
    }

    /// 剖分提交账本（扩展五十三）：`(增量续算次数, 整表重剖次数)`。
    /// 编辑器 / 探针靠它确认「补点真的走了续算」，而不是每次都整表重来。
    pub fn anim_layer_blend_build_stats(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<(usize, usize)> {
        Some(self.blend_space_of(instance, layer)?.build_stats())
    }

    /// 按样本设层速率（扩展五十二）：改了之后这层的时间推进就跟着参数走了。
    ///
    /// `instance` / `clip_name` 的解析口径与 `add_anim_layer_blend_sample` 一致
    /// （片段名可只写实例前缀段，入库用规范名，大小写不敏感）。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 片段在空间里没有样本
    /// （或速率非有限）。空间里没有该样本时**不改**任何东西——这是编辑器最常见的
    /// 「先 set 后 add」顺序错，静默生效反而查不出问题。
    pub fn set_anim_layer_blend_sample_speed(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        speed: f32,
    ) -> Option<bool> {
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.set_sample_speed(&c.name, speed),
            // 解析不到片段时仍允许按原始名字命中原名样本（片段可能已被卸载，
            // 但样本表还在——此时按名字改速率是合理的，不该整条失败）。
            None => space.set_sample_speed(clip_name, speed),
        })
    }

    /// 当前按样本插值出的层速率倍率（扩展五十二）。没挂空间返回 `None`。
    ///
    /// 全原速（没有变速样本）时返回 `Some(1.0)`——探针据此判断「这一层到底有没有在变速」，
    /// 不必自己扫样本表。
    pub fn anim_layer_blend_speed(&self, instance: &str, layer: &str) -> Option<f32> {
        self.blend_space_of(instance, layer).map(|s| s.speed_at())
    }
}
