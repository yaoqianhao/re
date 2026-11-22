use std::collections::HashMap;

use glam::{Mat4, Vec3};
use r3e_core::{Aabb, CasterNode, CasterTree, Light, Scene, MAX_CASCADES};

use super::*;

impl Renderer {
    /// 扩展十九：深度预通道。只写单采样 `depth_view`、无颜色目标。
    ///
    /// 存在的理由是 MSAA：主通道的深度挂在 4× 附件上，多重采样纹理不能直接
    /// 采样，而遮蔽通道要的恰恰是一张可采样的单采样深度图。这里只画不透明段
    /// ——半透明物件不写深度，画了也不会留下有效数据。
    ///
    /// 没有不透明批次时整个通道跳过（返回 0），不提交空 pass。
    pub(crate) fn draw_depth_prepass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        groups: &[DrawGroup],
    ) -> u32 {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("r3e-depth-prepass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.depth_prepass_pipeline);
        // 一帧里没有可画的不透明批次时仍保留这次 pass：它的作用是清空
        // 单采样深度，否则遮蔽通道会拿上一帧的深度继续算
        if groups.is_empty() {
            return 0;
        }
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.joint_bind_group, &[]);
        // 顶点阶段不采阴影贴图，但管线布局里有这一组，必须绑上才合法
        pass.set_bind_group(3, &self.shadow_bind_group, &[]);
        // 无颜色目标：占位绑定默认材质，避免依赖「驱动是否容忍空 group1」
        if let Some(default) = self.material_cache.get(MaterialKey::Default) {
            pass.set_bind_group(1, &default.bind_group, &[]);
        }

        self.draw_group_range(&mut pass, groups).0
    }

    // 参数虽多，但每个都来自调用方现成的字段；包进结构体只会让调用点更绕。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_shadow(
        &mut self,
        scene: &Scene,
        groups: &[DrawGroup],
        bounds: Option<Aabb>,
        light: Light,
        camera: &r3e_core::Camera,
        caster_bounds: &[Aabb],
        caster_nodes: &[r3e_core::NodeId],
    ) -> ShadowPlan {
        let shadow = light.shadow.sanitized();

        // 尺寸变化就重建纹理 + bind group（贴图句柄必须换新，不能原地 resize）
        if shadow.size != self.shadow_size {
            self.resize_shadow(shadow.size);
        }

        let levels = shadow.cascade.levels();
        // 用户没指定 max_distance 时，用「场景包围球最远点」当上限：几何之外的视距
        // 本来就没有可投影的东西，把级联摊到那里纯属浪费纹素。
        let mut cascade = shadow.cascade;
        if cascade.max_distance == 0.0 {
            if let Some(b) = bounds.filter(|b| !b.is_empty()) {
                let radius = (b.size().length() * 0.5).max(0.25);
                let reach = (b.center() - camera.position).length() + radius * 1.05;
                cascade.max_distance =
                    reach.clamp(camera.znear * 2.0, camera.zfar.max(camera.znear * 2.0));
            }
        }
        // 切分点长度 = 级数 + 1，首尾钉在 znear / 「max_distance 或 zfar」上
        let splits = r3e_core::cascade_splits(camera, cascade);
        let cam_vp = camera.view_projection();

        let mut matrices = [[[0.0f32; 4]; 4]; MAX_CASCADES];
        let mut split_ndc = [1.0f32; 4];
        let mut texel_world = 1e-5;
        // 按下标写回定长数组，用索引循环最直观；clippy 的迭代器建议在这里反而更难读。
        #[allow(clippy::needless_range_loop)]
        for level in 0..MAX_CASCADES {
            // 未使用的级复制最后一级矩阵：shader 若因浮点误差选到它也不会采到垃圾
            let i = level.min(levels - 1);
            let (near, far) = (splits[i], splits[i + 1]);
            let corners = r3e_core::frustum_slice_corners(camera, near, far);
            // 扩展二十四：把级联的近端朝光源方向推 `caster_extend`，
            // 让「站在级边界外侧、却把影子投进本级」的遮挡体不被裁掉。
            // extend = 0 时与 `fit_cascade` 逐位相同（有单测钉住）。
            let fit = r3e_core::fit_cascade_extended(
                light.direction,
                &corners,
                shadow.size,
                cascade.caster_extend,
            );

            let mut view_proj = fit.view_proj;
            let mut radius = fit.radius;
            // ortho_size > 0：作为单级覆盖半宽的上限（等价于用户手动收紧覆盖范围）
            if shadow.ortho_size > 0.0 && fit.radius > shadow.ortho_size {
                let scale = shadow.ortho_size / fit.radius;
                view_proj = Mat4::from_scale(Vec3::new(scale, scale, 1.0)) * view_proj;
                radius = shadow.ortho_size;
            }
            matrices[level] = view_proj.to_cols_array_2d();
            if level == 0 {
                texel_world = (2.0 * radius / shadow.size as f32).max(1e-5);
            }
            if level < levels {
                // 该级远端在相机 NDC 里的 z：远平面四角中心的投影深度
                let far_center = (corners[4] + corners[5] + corners[6] + corners[7]) * 0.25;
                let clip = cam_vp * far_center.extend(1.0);
                split_ndc[level] = if clip.w.abs() > 1e-6 {
                    (clip.z / clip.w).clamp(0.0, 1.0)
                } else {
                    1.0
                };
            }
        }

        let uniform = ShadowUniform {
            light_view_proj: matrices,
            split_ndc,
            params: [
                1.0 / shadow.size as f32,
                shadow.bias,
                if shadow.enabled { 1.0 } else { 0.0 },
                shadow.normal_bias * texel_world,
            ],
            params2: [shadow.strength, levels as f32, shadow.cascade.blend, 0.0],
        };
        self.queue
            .write_buffer(&self.shadow_uniform, 0, bytemuck::bytes_of(&uniform));

        // 深度 pass 只认矩阵（不碰会被主通道采样的 ShadowUniform），且每级一份
        for (level, matrix) in matrices.iter().enumerate().take(MAX_CASCADES) {
            let pass_uniform = ShadowPassUniform {
                light_view_proj: *matrix,
                pad: [0.0; 4],
            };
            self.queue.write_buffer(
                &self.shadow_pass_uniforms[level],
                0,
                bytemuck::bytes_of(&pass_uniform),
            );
        }

        // 扩展二十四：按级决定「这一级要提交哪些遮挡体」。
        // 关闭裁剪 / 阴影关闭 / 没有遮挡体时 `masks` 为空，调用方据此走全量提交，
        // 与扩展二十三的开销与画面逐字相同。
        let mut plan = ShadowPlan {
            levels: 0,
            masks: Vec::new(),
            tested: 0,
            culled: 0,
            pruned: 0,
            branch_pruned: 0,
            box_tests: 0,
        };
        if shadow.enabled && !groups.is_empty() {
            plan.levels = levels as u32;
            if cascade.caster_cull && !caster_bounds.is_empty() {
                let level_mats: Vec<Mat4> = (0..levels)
                    .map(|level| Mat4::from_cols_array_2d(&matrices[level]))
                    .collect();
                // 扩展二十五：先按场景树做层级判定——整枝落在级外的子树直接跳过，
                // 只有子树盒与级相交才下探，叶子才逐实例判。树不可用时（场景结构
                // 刚变、实例挂不上节点）退回扩展二十四的逐实例路径，掩码结果相同。
                match self.plan_casters_hierarchical(
                    scene,
                    caster_nodes,
                    caster_bounds,
                    &level_mats,
                ) {
                    Some(hierarchy) => {
                        plan.masks = hierarchy.masks;
                        plan.tested = hierarchy.instance_tests;
                        plan.pruned = hierarchy.pruned;
                        plan.branch_pruned = hierarchy.branch_pruned;
                        plan.box_tests = hierarchy.box_tests;
                    }
                    None => {
                        plan.masks = r3e_core::plan_casters(&level_mats, caster_bounds);
                        plan.tested = (levels as u32).saturating_mul(caster_bounds.len() as u32);
                    }
                }
                plan.culled = plan
                    .masks
                    .iter()
                    .map(|mask| mask.iter().filter(|keep| !**keep).count() as u32)
                    .sum();
            }
        }
        plan
    }

    /// 扩展二十五：把本帧的遮挡体挂回场景树，做层级判定。
    ///
    /// 返回 `None` 表示层级树不可用（场景里一个节点都没有），调用方退回逐实例判定。
    /// 拓扑只在 `Scene::layout_version` 前进时重建，其余帧只刷新节点盒 —— 因此
    /// 稳态下的额外开销就是「把实例盒写回对应节点」这一遍 O(实例) 的搬运。
    pub(crate) fn plan_casters_hierarchical(
        &mut self,
        scene: &Scene,
        caster_nodes: &[r3e_core::NodeId],
        caster_bounds: &[Aabb],
        level_mats: &[Mat4],
    ) -> Option<r3e_core::CasterHierarchyPlan> {
        self.sync_caster_layout(scene);
        let len = self.caster_tree.as_ref()?.len();
        if self.caster_node_bounds.len() != len {
            self.caster_node_bounds.resize(len, Aabb::EMPTY);
            self.caster_node_casts.resize(len, 0);
        }
        for bounds in self.caster_node_bounds.iter_mut() {
            *bounds = Aabb::EMPTY;
        }
        for casts in self.caster_node_casts.iter_mut() {
            *casts = 0;
        }
        self.caster_instance_nodes.clear();
        self.caster_instance_nodes.reserve(caster_nodes.len());
        for (id, bounds) in caster_nodes.iter().zip(caster_bounds.iter()) {
            match self.caster_slots.get(id) {
                Some(&slot) => {
                    self.caster_node_bounds[slot] = *bounds;
                    self.caster_node_casts[slot] = 1;
                    self.caster_instance_nodes.push(slot);
                }
                // 映射不到的实例填越界下标：`expand_masks` 会 fail-open 成全保留
                None => self.caster_instance_nodes.push(usize::MAX),
            }
        }
        let tree = self.caster_tree.as_mut()?;
        tree.refresh(&self.caster_node_bounds, &self.caster_node_casts);
        let hierarchy = tree.plan(level_mats);
        let masks = r3e_core::expand_masks(&hierarchy.masks, &self.caster_instance_nodes);
        Some(r3e_core::CasterHierarchyPlan {
            masks,
            box_tests: hierarchy.box_tests,
            instance_tests: hierarchy.instance_tests,
            pruned: hierarchy.pruned,
            branch_pruned: hierarchy.branch_pruned,
        })
    }

    /// 把场景树压成「扁平节点表 + id → 下标表」，供层级剔除建树用。
    ///
    /// `Scene` 按创建序迭代，父节点通常先于子节点入表，但兄弟间顺序不保证，
    /// 所以分两趟：先占位拿全 id，第二趟才回填父指针。
    pub(crate) fn layout_slots(
        scene: &Scene,
    ) -> (Vec<CasterNode>, HashMap<r3e_core::NodeId, usize>) {
        let mut nodes: Vec<CasterNode> = Vec::with_capacity(scene.len());
        let mut slots: HashMap<r3e_core::NodeId, usize> = HashMap::with_capacity(scene.len());
        for node in scene.iter() {
            slots.insert(node.id, nodes.len());
            nodes.push(CasterNode {
                parent: None,
                bounds: Aabb::EMPTY,
                casts: 0,
            });
        }
        for (index, node) in scene.iter().enumerate() {
            nodes[index].parent = node.parent.and_then(|parent| slots.get(&parent).copied());
        }
        (nodes, slots)
    }

    /// 场景结构变了才重建层级树（`caster_tree` 与 `caster_slots` 同寿命）。
    pub(crate) fn sync_caster_layout(&mut self, scene: &Scene) {
        if self.caster_tree.is_some() && self.caster_layout == scene.layout_version() {
            return;
        }
        let (nodes, slots) = Self::layout_slots(scene);
        self.caster_node_bounds = vec![Aabb::EMPTY; nodes.len()];
        self.caster_node_casts = vec![0; nodes.len()];
        self.caster_tree = Some(CasterTree::new(nodes));
        self.caster_slots = slots;
        self.caster_layout = scene.layout_version();
        self.caster_tree_builds = self.caster_tree_builds.saturating_add(1);
    }

    /// 场景结构变了才重建主通道视锥树（扩展二十九，与 `sync_caster_layout` 同套路）。
    pub(crate) fn sync_cull_layout(&mut self, scene: &Scene) {
        if self.cull_tree.is_some() && self.cull_layout == scene.layout_version() {
            return;
        }
        let (nodes, slots) = Self::layout_slots(scene);
        self.cull_node_bounds = vec![Aabb::EMPTY; nodes.len()];
        self.cull_node_casts = vec![0; nodes.len()];
        self.cull_tree = Some(CasterTree::new(nodes));
        self.cull_slots = slots;
        self.cull_layout = scene.layout_version();
        self.cull_tree_builds = self.cull_tree_builds.saturating_add(1);
    }

    /// 重建阴影深度图数组与对应的 bind group（改 `shadow.size` 时触发）。
    ///
    /// 局部光源阴影图不在这里重建：它的尺寸固定（24 层 × 512²），
    /// 跟着方向光的 `shadow.size` 走会让显存按平方增长（4096 时 24 层 ≈ 1.6 GiB）。
    pub(crate) fn resize_shadow(&mut self, size: u32) {
        let (texture, view, layer_views) = create_shadow_target(&self.device, size);
        self.shadow_bind_group = create_shadow_bind_group(
            &self.device,
            &self.shadow_layout,
            ShadowBindInputs {
                view: &view,
                sampler: &self.shadow_sampler,
                uniform: &self.shadow_uniform,
                lights: &self.lights_buffer,
                // 局部阴影贴图与 uniform 都没变，重建时按原样重新绑定
                local_view: &self.local_shadow_view,
                local_uniform: &self.local_shadow_uniform,
            },
        );
        self.shadow_view = view;
        self.shadow_layer_views = layer_views;
        self._shadow_texture = texture;
        self.shadow_size = size;
        log::info!("阴影贴图重建: {size}x{size} × {MAX_CASCADES} 层");
    }

    /// 提交一次阴影深度 pass（方向光级联与局部光源共用同一份管线）。
    ///
    /// 两者的顶点布局、实例缓冲、剔除结果完全一致，差别只有「写哪一层 + 用哪份矩阵」，
    /// 因此抽成一个方法：任何阴影管线调整（斜坡偏移、剔除策略）只需改一处。
    /// 返回本次提交的 draw call 数，供 `RenderStats::shadow_draws` 累计。
    ///
    /// `runs` 由 [`shadow_runs`] 生成：扩展二十四起，方向光的每一级可能只提交
    /// 分组内的一段实例（其余落在本级覆盖范围外，被裁掉）。
    pub(crate) fn draw_shadow_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        pass_bind_group: &wgpu::BindGroup,
        submit: ShadowSubmit<'_>,
    ) -> u32 {
        let ShadowSubmit {
            groups,
            runs,
            stride,
        } = submit;
        let mut draws = 0u32;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.shadow_pipeline);
        // 阴影管线只真正用到 group(2)（蒙皮关节矩阵），但 wgpu 要求管线布局里
        // 低索引的绑定组必须逐个 set（否则报 "expects a BindGroup at index 0"），
        // 所以这里顺带绑上相机与默认材质 —— 顶点着色器不采样它们，纯占位。
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        if let Some(default_material) = self.material_cache.get(MaterialKey::Default) {
            pass.set_bind_group(1, &default_material.bind_group, &[]);
        }
        pass.set_bind_group(2, &self.joint_bind_group, &[]);
        // group(3) 这里是「只含该层矩阵」的阴影通道专用绑定组，
        // 不是主通道那张比较采样器 —— 深度图此刻正被本 pass 写入。
        pass.set_bind_group(3, pass_bind_group, &[]);
        for (group_index, range) in runs {
            let Some((key, ..)) = groups.get(*group_index) else {
                continue;
            };
            let Some(gpu) = self.mesh_cache.get(*key) else {
                continue;
            };
            pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
            pass.set_vertex_buffer(
                1,
                self.instance_buffer
                    .slice(range.start as u64 * stride..range.end as u64 * stride),
            );
            pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            // 与主通道同一条不变量：实例数取该 run 的区间长度，不是恒 1。
            // 漏改的症状是「主通道画全了，但影子只投每批第一个实例」。
            let batch = instance_count(range);
            for part in &gpu.parts {
                pass.draw_indexed(
                    part.first_index..part.first_index + part.index_count,
                    0,
                    0..batch,
                );
                draws += 1;
            }
        }
        draws
    }

    /// 局部光源阴影图的边长（扩展十八 C）。固定 `DEFAULT_LOCAL_SHADOW_SIZE`，
    /// 不随方向光 `shadow.size` 变化：一盏点光要 6 层，跟着 4096 走会吃掉 1 GiB 以上。
    pub fn local_shadow_size(&self) -> u32 {
        self.local_shadow_size
    }

    /// 局部光源阴影图的总层数（`MAX_LOCAL_SHADOW_LAYERS` = 4 灯 × 6 面）。
    pub fn local_shadow_layer_count(&self) -> usize {
        self.local_shadow_layer_views.len()
    }

    /// 扩展十八 C：打包本帧的局部光源阴影 uniform，返回需要渲染的层号。
    ///
    /// 返回空 vec 表示「本帧没有任何灯投影」：渲染层据此整段跳过深度 pass，
    /// 与 `cast_shadow == false` 的旧场景完全等价（开销为零）。
    pub(crate) fn prepare_local_shadow(&mut self, scene: &Scene) -> (Vec<u32>, u32) {
        let enabled = scene.lights_enabled && !scene.lights.is_empty();
        let uniform = LocalShadowUniform::from_lights(&scene.lights, enabled);
        let layers = uniform.layers();
        let casters = uniform.params[0].max(0.0) as u32;
        self.queue
            .write_buffer(&self.local_shadow_uniform, 0, bytemuck::bytes_of(&uniform));
        // 每层一份矩阵 uniform：深度 pass 只认 group(3) binding(3)，与主通道解耦
        for layer in &layers {
            let matrix = uniform.view_proj[*layer as usize];
            let pass_uniform = ShadowPassUniform {
                light_view_proj: matrix,
                pad: [0.0; 4],
            };
            self.queue.write_buffer(
                &self.local_shadow_pass_uniforms[*layer as usize],
                0,
                bytemuck::bytes_of(&pass_uniform),
            );
        }
        (layers, casters)
    }
}
