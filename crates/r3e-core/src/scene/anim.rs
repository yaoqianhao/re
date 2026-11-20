//! 动画实例与片段的登记、播放控制与逐帧推进。

use super::*;

impl Scene {
    /// 推进所有动画实例并把采样结果写回节点局部变换。
    ///
    /// 必须在 `update_world_transforms` 之前调用：动画改的是局部 TRS，世界矩阵由
    /// 场景图统一合成。已销毁节点的绑定会被就地剔除，避免每帧都做无效查找。
    pub fn update_animations(&mut self, dt: f32) {
        if self.animations.is_empty() {
            return;
        }

        // 先算出所有待写回项，再统一落到节点上，避免同时可变借用 animations 与 nodes
        let mut writes: Vec<(NodeId, Transform)> = Vec::new();
        for inst in self.animations.iter_mut() {
            inst.advance(dt);
            let time = inst.sample_time();
            let weight = inst.fade_weight();
            let fade_time = inst.fade_sample_time();

            // 第一遍：逐绑定求出主片段姿态（`None` = 该节点没有主片段轨道）
            let mut poses: Vec<Option<Transform>> = Vec::with_capacity(inst.bindings.len());
            for (i, b) in inst.bindings.iter().enumerate() {
                let fresh = if b.track == crate::anim::NO_TRACK {
                    None
                } else {
                    inst.clip
                        .tracks
                        .get(b.track as usize)
                        .and_then(|t| t.sample(time))
                };
                // 过渡中的旧片段：只知道与绑定的对应关系，缺轨道时下标为 NO_TRACK
                let old = match (inst.fade.as_ref(), fade_time) {
                    (Some(fade), Some(t)) => fade
                        .tracks
                        .get(i)
                        .copied()
                        .filter(|t| *t != crate::anim::NO_TRACK)
                        .and_then(|idx| fade.clip.tracks.get(idx as usize))
                        .and_then(|track| track.sample(t)),
                    _ => None,
                };
                poses.push(match (fresh, old) {
                    // 新片段缺该轨道：过渡期间由旧片段继续驱动，结束后停在末姿态
                    (None, Some(o)) => Some(o),
                    (Some(n), Some(o)) => Some(crate::anim::blend_transform(&o, &n, weight)),
                    (Some(n), None) => Some(n),
                    (None, None) => None,
                });
            }

            // 第二遍：混合树层按挂载顺序叠加到主姿态上
            //
            // 扩展四十五：挂了混合空间的层先用当前参数重算层内片段权重，再照旧叠加。
            // 没挂空间的层这一步是空操作，权重仍是脚本/场景写死的值——零变化验收靠这一行保证。
            // 扩展四十六：开了相位锁的层改用「归一化相位 × 片段时长」采样每路片段，
            // `LayerSync::Off` 的层仍取同一个层时间，逐位不变。
            // 扩展四十七：相位锁层的相位可以跟随别的层（上下半身对齐），
            // 所以先按全层视角把每层的有效相位算好，再进可变借用的混合循环。
            // 没挂源层 / 没开相位锁时 effective_phase 就是本层 phase()，逐位不变。
            let sync_phases: Vec<Option<f32>> = (0..inst.layers.len())
                .map(|i| match inst.layers[i].sync {
                    LayerSync::Off => None,
                    LayerSync::Phase => Some(effective_phase(&inst.layers, i)),
                })
                .collect();
            for (layer_index, layer) in inst.layers.iter_mut().enumerate() {
                layer.apply_blend_space();
                let layer_weight = layer.effective_weight();
                if layer_weight <= 0.0 {
                    continue;
                }
                let layer_time = layer.sample_time();
                let sync_phase = sync_phases[layer_index];
                for (i, b) in inst.bindings.iter().enumerate() {
                    let effective = layer_weight * layer.mask.weight_for(&b.path);
                    if effective <= 0.0 {
                        continue;
                    }
                    // 扩展五十：加法层与覆盖层吃同一份份额，但算的不是一回事——
                    // 覆盖层要把多路姿态按份额平均成"那个姿态"，加法层要把多路**增量**按份额叠加。
                    // 故加法层走 sample_layer_pose_additive（单路时零算术，逐位不变）。
                    let sample = match (sync_phase, layer.mode) {
                        (Some(phase), LayerMode::Additive) => sample_layer_pose_additive_synced(
                            &layer.clips,
                            phase,
                            b.node_index,
                            &b.path,
                        ),
                        (Some(phase), LayerMode::Override) => {
                            sample_layer_pose_synced(&layer.clips, phase, b.node_index, &b.path)
                        }
                        (None, LayerMode::Additive) => sample_layer_pose_additive(
                            &layer.clips,
                            layer_time,
                            b.node_index,
                            &b.path,
                        ),
                        (None, LayerMode::Override) => {
                            sample_layer_pose(&layer.clips, layer_time, b.node_index, &b.path)
                        }
                    };
                    let Some(sample) = sample else {
                        continue;
                    };
                    poses[i] = Some(match (layer.mode, poses[i]) {
                        // 主片段没有该节点轨道：层姿态直接作为绝对姿态
                        (_, None) => sample.pose,
                        (LayerMode::Additive, Some(base)) => {
                            apply_additive(&base, &sample.reference, &sample.pose, effective)
                        }
                        (LayerMode::Override, Some(base)) => {
                            blend_override(&base, &sample.pose, effective)
                        }
                    });
                }
            }

            for (i, b) in inst.bindings.iter().enumerate() {
                if let Some(transform) = poses[i] {
                    writes.push((b.node, transform));
                }
            }
        }

        if writes.is_empty() {
            return;
        }
        for (id, transform) in writes {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.transform = transform;
            }
        }
    }

    /// 移除绑定了已销毁节点的死绑定（销毁节点后调用，保持实例表干净）。
    pub fn prune_anim_bindings(&mut self) {
        if self.animations.is_empty() {
            return;
        }
        let alive: std::collections::HashSet<NodeId> = self.nodes.keys().copied().collect();
        for inst in &mut self.animations {
            inst.bindings.retain(|b| alive.contains(&b.node));
        }
    }

    pub fn add_anim_instance(&mut self, instance: AnimInstance) -> usize {
        self.animations.push(instance);
        self.animations.len() - 1
    }

    pub fn anim_names(&self) -> Vec<String> {
        self.animations.iter().map(|a| a.name.clone()).collect()
    }

    /// 按名字找实例；`name` 为空时取第一个（脚本里 `play_anim()` 的默认行为）。
    pub fn anim_instance(&self, name: &str) -> Option<&AnimInstance> {
        if name.is_empty() {
            return self.animations.first();
        }
        self.animations
            .iter()
            .find(|a| a.name == name || a.clip.name.eq_ignore_ascii_case(name))
    }

    pub fn anim_instance_mut(&mut self, name: &str) -> Option<&mut AnimInstance> {
        if name.is_empty() {
            return self.animations.first_mut();
        }
        self.animations
            .iter_mut()
            .find(|a| a.name == name || a.clip.name.eq_ignore_ascii_case(name))
    }

    /// 播放（从当前时间继续）。返回是否命中实例。
    pub fn play_anim(&mut self, name: &str) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.playing = true;
                true
            }
            None => false,
        }
    }

    /// 暂停并保留当前时间。
    pub fn stop_anim(&mut self, name: &str) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.playing = false;
                true
            }
            None => false,
        }
    }

    /// 设置播放速度（负值为倒放）。
    pub fn set_anim_speed(&mut self, name: &str, speed: f32) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.speed = speed;
                true
            }
            None => false,
        }
    }

    /// 跳转到指定时间（秒）。超出片段范围时按循环/钳制规则处理。
    pub fn set_anim_time(&mut self, name: &str, time: f32) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.time = time;
                true
            }
            None => false,
        }
    }

    /// 查询播放时间（秒），未命中返回 `None`。
    pub fn anim_time(&self, name: &str) -> Option<f32> {
        self.anim_instance(name).map(|a| a.sample_time())
    }

    /// 登记一份可切换的片段，键为 `"<资产名>/<片段名>"`。
    pub fn register_anim_clip(&mut self, key: impl Into<String>, clip: std::sync::Arc<AnimClip>) {
        self.anim_clips.insert(key.into(), clip);
    }

    /// 某个资产（实例名前缀）下所有可切换片段名，按名称排序。
    pub fn anim_clip_names(&self, prefix: &str) -> Vec<String> {
        let head = format!("{prefix}/");
        let mut out: Vec<String> = self
            .anim_clips
            .keys()
            .filter_map(|k| k.strip_prefix(&head).map(str::to_string))
            .collect();
        out.sort();
        out
    }

    /// 把实例切换到同一资产的另一个片段（如 idle → run → atk）。
    ///
    /// 实例名形如 `<资产名>/<片段名>`，片段名从该资产已登记的片段里按名字（大小写不敏感）查找，
    /// 因此脚本可以写 `set_anim_clip("tirger/idle_loop", "run_loop")`。
    pub fn set_anim_clip(&mut self, instance: &str, clip_name: &str) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                inst.set_clip(clip);
                true
            }
            None => false,
        }
    }

    /// 带交叉淡入地切换片段，`duration` 为过渡秒数（`<= 0` 等价 [`Self::set_anim_clip`]）。
    pub fn set_anim_clip_fade(&mut self, instance: &str, clip_name: &str, duration: f32) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                inst.cross_fade(clip, duration);
                true
            }
            None => false,
        }
    }

    /// 交叉淡入剩余进度：新片段当前权重（`1.0` 表示没有过渡正在进行）。
    pub fn anim_fade_weight(&self, instance: &str) -> Option<f32> {
        self.anim_instance(instance).map(|a| a.fade_weight())
    }

    /// 过渡中的旧片段名；没有过渡时返回空串（脚本日志用）。
    pub fn anim_fade_source(&self, instance: &str) -> String {
        self.anim_instance(instance)
            .and_then(|a| a.fade.as_ref().map(|f| f.clip.name.clone()))
            .unwrap_or_default()
    }

    // ---------- 动画混合树（扩展十八 A） ----------

    /// 按「实例名前缀 + 片段名（大小写不敏感）」解析出可切换片段。
    pub(super) fn find_anim_clip(
        &self,
        instance: &str,
        clip_name: &str,
    ) -> Option<std::sync::Arc<AnimClip>> {
        let inst_key = self
            .anim_instance(instance)?
            .name
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let head = format!("{inst_key}/");
        self.anim_clips
            .iter()
            .find(|(k, _)| {
                k.strip_prefix(&head)
                    .map(|n| n.eq_ignore_ascii_case(clip_name))
                    .unwrap_or(false)
            })
            .map(|(_, c)| c.clone())
    }

    // ---------- 加载 ----------
}
