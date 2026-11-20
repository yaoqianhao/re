//! 动画图层（基础 CRUD、权重、遮罩、同步、片段相位）。

use super::*;

impl Scene {
    /// 给实例挂一层（同名则整层覆盖）。
    ///
    /// 层与主片段互不影响：主片段负责「当前动作」，层负责同时进行的附加动作
    /// （上半身攻击、呼吸叠加）。超过 [`MAX_ANIM_LAYERS`] 返回 `false`。
    pub fn add_anim_layer(&mut self, instance: &str, layer: AnimLayer) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        if let Some(existing) = anim_layer_mut(inst, &layer.name) {
            *existing = layer;
            return true;
        }
        if inst.layers.len() >= MAX_ANIM_LAYERS {
            return false;
        }
        inst.layers.push(layer);
        true
    }

    /// 按片段名直接建层（脚本 / 编辑器入口，省去调用方自己取 `Arc<AnimClip>`）。
    ///
    /// 片段名按「实例名前缀」解析；同名层覆盖。
    pub fn add_anim_layer_by_clip(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
        mode: LayerMode,
    ) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        self.add_anim_layer(
            instance,
            AnimLayer::new(layer, clip, weight).with_mode(mode),
        )
    }

    /// 层名列表（按挂载顺序，顺序即叠加顺序）。
    pub fn anim_layer_names(&self, instance: &str) -> Vec<String> {
        self.anim_instance(instance)
            .map(|inst| inst.layers.iter().map(|l| l.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 层数量；实例不存在时返回 `None`。
    pub fn anim_layer_count(&self, instance: &str) -> Option<usize> {
        self.anim_instance(instance).map(|inst| inst.layers.len())
    }

    /// 按名字取层；`layer` 为空时取第一层。
    pub fn anim_layer(&self, instance: &str, layer: &str) -> Option<&AnimLayer> {
        let inst = self.anim_instance(instance)?;
        if layer.is_empty() {
            return inst.layers.first();
        }
        inst.layers
            .iter()
            .find(|l| l.name == layer || l.name.eq_ignore_ascii_case(layer))
    }

    /// 层当前有效权重（含进行中的过渡）。
    pub fn anim_layer_effective_weight(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer(instance, layer)
            .map(|l| l.effective_weight())
    }

    /// 层信息摘要（脚本日志 / RPC 回显用）。
    pub fn anim_layer_report(&self, instance: &str, layer: &str) -> Option<String> {
        self.anim_layer(instance, layer)
            .map(crate::anim_blend::layer_report)
    }

    /// 直接设定层权重（取消进行中的过渡）。
    pub fn set_anim_layer_weight(&mut self, instance: &str, layer: &str, weight: f32) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.set_weight(weight);
                true
            }
            None => false,
        }
    }

    /// 层权重过渡到 `weight`（`duration <= 0` 等价直接设定）。
    pub fn fade_anim_layer(
        &mut self,
        instance: &str,
        layer: &str,
        weight: f32,
        duration: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.fade_to(weight, duration);
                true
            }
            None => false,
        }
    }

    /// 设置叠加模式（override / additive）。
    pub fn set_anim_layer_mode(&mut self, instance: &str, layer: &str, mode: LayerMode) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.mode = mode;
                true
            }
            None => false,
        }
    }

    /// 设置骨骼遮罩。
    pub fn set_anim_layer_mask(&mut self, instance: &str, layer: &str, mask: BoneMask) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.mask = mask;
                true
            }
            None => false,
        }
    }

    /// 设置层播放速度（负值倒放）。
    pub fn set_anim_layer_speed(&mut self, instance: &str, layer: &str, speed: f32) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.speed = speed;
                true
            }
            None => false,
        }
    }

    // ---------- 层内相位同步（扩展四十六） ----------

    /// 开关层内相位锁：`true` = 各片段按归一化相位缩放采样（走跑不再错拍）。
    ///
    /// 返回 `false` 表示实例 / 层不存在，层状态不变。
    pub fn set_anim_layer_sync(&mut self, instance: &str, layer: &str, enabled: bool) -> bool {
        self.set_anim_layer_sync_strategy(
            instance,
            layer,
            if enabled {
                LayerSync::Phase
            } else {
                LayerSync::Off
            },
        )
    }

    /// 直接指定相位策略（`off` / `phase`）。
    pub fn set_anim_layer_sync_strategy(
        &mut self,
        instance: &str,
        layer: &str,
        sync: LayerSync,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.sync = sync;
                true
            }
            None => false,
        }
    }

    /// 层当前是否开了相位锁。实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync(&self, instance: &str, layer: &str) -> Option<bool> {
        self.anim_layer(instance, layer)
            .map(|l| l.sync == LayerSync::Phase)
    }

    /// 层当前相位（0..1）。实例 / 层不存在返回 `None`。
    pub fn anim_layer_phase(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer(instance, layer).map(|l| l.phase())
    }

    /// 相位同步单行探针（日志用，含跨层源与偏移段）。实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync_probe(&self, instance: &str, layer: &str) -> Option<String> {
        let inst = self.anim_instance(instance)?;
        let (index, phase, source) = self.anim_layer_phase_view(instance, layer)?;
        Some(sync_probe_with(
            &inst.layers[index],
            phase,
            source.as_deref(),
        ))
    }

    // ---------- 相位偏移量与跨层对齐（扩展四十七） ----------

    /// 层下标 + 有效相位 + 源层名（跨层对齐后真正用于采样的相位）。
    ///
    /// 内部用：`Off` 的层不给源层名，免得探针里出现「跟着某层但没生效」的误导。
    fn anim_layer_phase_view(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<(usize, f32, Option<String>)> {
        let inst = self.anim_instance(instance)?;
        let index = if layer.is_empty() {
            if inst.layers.is_empty() {
                return None;
            }
            0
        } else {
            inst.layers
                .iter()
                .position(|l| l.name == layer || l.name.eq_ignore_ascii_case(layer))?
        };
        let phase = effective_phase(&inst.layers, index);
        let source = if inst.layers[index].sync == LayerSync::Phase {
            phase_source(&inst.layers, index).map(|i| inst.layers[i].name.clone())
        } else {
            None
        };
        Some((index, phase, source))
    }

    /// 设某一路片段的相位偏移：`offset` 折进 `[0, 1)`（`-0.25` 等价 `0.75`）。
    ///
    /// `clip` 为空时作用于层内第一路；片段名大小写不敏感。
    /// 返回 `false` 表示实例 / 层 / 片段不存在，片段状态不变。
    pub fn set_anim_layer_clip_phase_offset(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
        offset: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.set_phase_offset(offset);
                true
            }
            None => false,
        }
    }

    /// 读某一路片段的相位偏移（没设过是 0.0）。实例 / 层 / 片段不存在返回 `None`。
    pub fn anim_layer_clip_phase_offset(
        &self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> Option<f32> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        Some(l.clips[index].phase_offset)
    }

    /// 让层的相位跟随 `source` 层（跨层对齐：上下半身各层同一相位起步）。
    ///
    /// 只在层开了相位锁（[`LayerSync::Phase`]）时影响采样；`None` / 空串 = 清除，跟回自己。
    ///
    /// 返回 `false` 表示：实例 / 层不存在、源层不存在、指向自己、或会形成跟随环
    /// （A→B→A）。**失败时层状态原样回滚**，不会留下半截链。
    pub fn set_anim_layer_sync_source(
        &mut self,
        instance: &str,
        layer: &str,
        source: Option<&str>,
    ) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        let Some(index) = anim_layer_index(inst, layer) else {
            return false;
        };
        match source.map(str::trim).filter(|s| !s.is_empty()) {
            None => {
                inst.layers[index].set_sync_source(None);
                true
            }
            Some(name) => {
                if anim_layer_index(inst, name).is_none() {
                    return false;
                }
                let before = inst.layers[index].sync_source.clone();
                inst.layers[index].set_sync_source(Some(name));
                // 自引用 / 成环 / 解析不到：chain 走不出来，回滚
                if phase_source(&inst.layers, index).is_some() {
                    true
                } else {
                    inst.layers[index].sync_source = before;
                    false
                }
            }
        }
    }

    /// 层当前跟随的源层名（没设过返回空串，不返回 `None` 以便脚本直接打印）。
    /// 实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync_source(&self, instance: &str, layer: &str) -> Option<String> {
        self.anim_layer(instance, layer)
            .map(|l| l.sync_source.clone().unwrap_or_default())
    }

    /// 跨层对齐后的**有效相位**（含源层跟随）。实例 / 层不存在返回 `None`。
    ///
    /// 没挂源层时与 [`Self::anim_layer_phase`] 相同。
    pub fn anim_layer_sync_phase(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer_phase_view(instance, layer)
            .map(|(_, phase, _)| phase)
    }

    // ---------- 片段自带速率与循环标志（扩展四十八） ----------

    /// 设某一路片段的播放速率与循环标志：这一路从此按自己的时间轴走。
    ///
    /// `speed` 负值倒放、`0` 定格首帧、非有限值按原速；`looping = false` 时到末帧停住。
    /// 只在层没开相位锁（[`LayerSync::Off`]）时影响采样——相位锁层的每路时间由相位
    /// 决定，要变速请调层 [`Self::set_anim_layer_speed`]。
    ///
    /// `clip` 为空时作用于层内第一路；片段名大小写不敏感。
    /// 返回 `false` 表示实例 / 层 / 片段不存在，片段状态不变。
    pub fn set_anim_layer_clip_playback(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
        speed: f32,
        looping: bool,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.set_playback(speed, looping);
                true
            }
            None => false,
        }
    }

    /// 清除某一路片段的播放参数：回到「跟着层时间轴走」的默认口径。
    /// 返回 `false` 表示实例 / 层 / 片段不存在。
    pub fn clear_anim_layer_clip_playback(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.clear_playback();
                true
            }
            None => false,
        }
    }

    /// 读某一路片段的播放参数：`None` = 没设过（跟着层时间轴走）。
    pub fn anim_layer_clip_playback(
        &self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> Option<(f32, bool)> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        l.clips[index].playback.map(|pb| (pb.speed, pb.looping))
    }

    /// 某一路片段**这一帧实际采样到的时间**（秒，含速率/循环折算）。
    ///
    /// 真机取证用：同一个层时间下，两路的这个值不同就说明速率生效了。
    pub fn anim_layer_clip_time(&self, instance: &str, layer: &str, clip: &str) -> Option<f32> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        l.clip_times().get(index).copied()
    }

    /// 把层的片段替换为指定片段（层内收敛为单片段、权重 1.0，其余属性保留）。
    ///
    /// 片段名按「实例名前缀」解析（`"tirger/idle_loop"` 的实例可只写 `"atk"`）。
    pub fn set_anim_layer_clip(&mut self, instance: &str, layer: &str, clip_name: &str) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.clips = vec![crate::anim_blend::LayerClip::new(clip, 1.0)];
                true
            }
            None => false,
        }
    }

    /// 给层增加一路参与混合的片段（走 / 跑按权重加权）。
    pub fn add_anim_layer_clip(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
    ) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.add_clip(clip, weight),
            None => false,
        }
    }

    /// 调整层内某一路片段的权重。
    pub fn set_anim_layer_clip_weight(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.set_clip_weight(clip_name, weight),
            None => false,
        }
    }

    /// 移除层内某一路片段（最后一路不可移除）。
    pub fn remove_anim_layer_clip(&mut self, instance: &str, layer: &str, clip_name: &str) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.remove_clip(clip_name),
            None => false,
        }
    }

    /// 移除一整层。
    pub fn remove_anim_layer(&mut self, instance: &str, layer: &str) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        let before = inst.layers.len();
        inst.layers
            .retain(|l| !(l.name == layer || l.name.eq_ignore_ascii_case(layer)));
        inst.layers.len() != before
    }

    /// 清空实例的所有层。
    pub fn clear_anim_layers(&mut self, instance: &str) -> bool {
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                let had = !inst.layers.is_empty();
                inst.layers.clear();
                had
            }
            None => false,
        }
    }

    // ---------- 1D / 2D / 3D 混合空间（扩展四十五 / 四十九） ----------
}

/// 按名字取可变层；名字为空时取第一层（与 [`Scene::anim_layer`] 口径一致）。
pub(super) fn anim_layer_mut<'a>(
    inst: &'a mut AnimInstance,
    name: &str,
) -> Option<&'a mut AnimLayer> {
    if name.is_empty() {
        return inst.layers.first_mut();
    }
    inst.layers
        .iter_mut()
        .find(|l| l.name == name || l.name.eq_ignore_ascii_case(name))
}

/// 按名字取层下标（`name` 为空取 0），口径与 [`anim_layer_mut`] 一致。
pub(super) fn anim_layer_index(inst: &AnimInstance, name: &str) -> Option<usize> {
    if name.is_empty() {
        return if inst.layers.is_empty() {
            None
        } else {
            Some(0)
        };
    }
    inst.layers
        .iter()
        .position(|l| l.name == name || l.name.eq_ignore_ascii_case(name))
}

/// 按名字取层内片段下标；`clip` 为空取第一路，大小写不敏感（扩展四十七）。
pub(super) fn layer_clip_index(layer: &AnimLayer, clip: &str) -> Option<usize> {
    if clip.is_empty() {
        return if layer.clips.is_empty() {
            None
        } else {
            Some(0)
        };
    }
    layer
        .clips
        .iter()
        .position(|c| c.clip.name == clip || c.clip.name.eq_ignore_ascii_case(clip))
}

/// 按名字取层内可变片段（口径同 [`layer_clip_index`]）。
pub(super) fn layer_clip_mut<'a>(
    layer: &'a mut AnimLayer,
    clip: &str,
) -> Option<&'a mut crate::anim_blend::LayerClip> {
    let index = layer_clip_index(layer, clip)?;
    layer.clips.get_mut(index)
}
