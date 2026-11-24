use r3e_core::Scene;

use crate::mesh::InstanceRaw;
use crate::postfx::PostFxStats;
use crate::ssao::SsaoStats;

use super::*;

impl Renderer {
    /// 绘制一帧。场景的任何变化都会在下一帧生效，不需要重建管线。
    pub fn render(&mut self, scene: &Scene) {
        let Some(frame) = self.acquire_surface_frame() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(scene, &view);
        self.queue.present(frame);
    }

    /// 取一帧 swapchain 纹理。`None` = 本帧应跳过（surface 失效 / 非致命错误）。
    ///
    /// 抽出来是为了让 [`Self::render`] 与 [`Self::capture_frame`] 共用同一段
    /// 「拿到可绘制目标」前后的错误处理，而不是各抄一份。
    pub(crate) fn acquire_surface_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.surface.configure(&self.device, &self.config);
                Some(f)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("surface 已失效，重新配置");
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                log::debug!("跳帧: {other:?}");
                None
            }
        }
    }

    /// 把场景渲染到任意目标视图（窗口 swapchain 纹理或离屏纹理）。
    ///
    /// 这是**唯一**的绘制实现：窗口渲染与截图都走这里，所以截图内容与用户看到的
    /// 画面逐像素同源（阴影 / 遮蔽 / 后处理 / 文本 / UI 全部包含）。
    pub(crate) fn render_to_view(&mut self, scene: &Scene, view: &wgpu::TextureView) {
        // 扩展二十二：先把上一轮读回的深度取回来（非阻塞），再看本帧有没有金字塔可用。
        // 关闭时这里只做一次 `enabled` 判断，不 poll、不动 buffer。
        let occlusion_settings = scene.occlusion.sanitized();
        self.occlusion.begin_frame(&self.device, occlusion_settings);
        // 扩展三十五：本帧暂存写入计数归零（`prepare_instances` 里攒包、`flush` 落槽）
        self.staging.begin_frame();
        // 扩展三十六：形变表有变动（新网格带形状 / 换资产库）就整表重写一次
        self.upload_morph_table();
        let plan = self.prepare_instances(scene);
        let groups = plan.groups;
        let transparent_start = plan.transparent_start;
        let (culled, cull_unknown) = (plan.culled, plan.cull_unknown);
        // 扩展三十六：形变计数在上传前定型，这里先取出来（plan 后面会按字段消费）
        let morph_instances = plan.morph_instances;
        // 扩展三十八：互斥归一化的实例数与逐形状剔除余量合计
        let (morph_normalized, morph_cull_inflate) =
            (plan.morph_normalized, plan.morph_cull_inflate);
        // 扩展四十：掩码摘掉形变的实例数，与因此省下的非零权重槽数
        let (morph_masked, morph_masked_shapes) = (plan.morph_masked, plan.morph_masked_shapes);
        self.prepare_materials(&groups);
        // M2-C03b：地形通道的上传与 uniform。没有地形时这一步零动作。
        self.prepare_terrain(scene);
        // 阶段 D：先把本帧的阴影贴图渲好，主通道再采样它
        // 扩展二十四：多带一份「每级保留哪些实例」的掩码
        let shadow_plan = self.prepare_shadow(
            scene,
            &groups,
            plan.scene_bounds,
            scene.light,
            &scene.camera,
            &plan.caster_bounds,
            &plan.caster_nodes,
        );
        // 扩展十五：局部光源表每帧重传（灯可以被脚本逐帧移动，缓存反而要判等）
        let local_lights = self.prepare_local_lights(scene);
        // 扩展十八 C：本帧要跑深度 pass 的局部阴影层（空 = 没有灯投影）
        let (local_shadow_layers, local_shadow_casters) = self.prepare_local_shadow(scene);

        let bg = scene.background;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("r3e-encoder"),
            });

        // 扩展三十五：把本帧暂存环里的块拷贝录进编码器 —— 之前 `prepare_*` 阶段
        // 只往暂存槽写了一次，真正的「槽 → 实例/关节缓冲」搬运在这里按段展开。
        // 必须紧跟编码器创建、在任何绘制 pass 之前，且本帧一定会被提交。
        self.staging
            .record(&mut encoder, &self.instance_buffer, &self.joint_buffer);

        let stride = std::mem::size_of::<InstanceRaw>() as u64;
        let mut shadow_draws = 0u32;

        // 阶段 D / 扩展十六：方向光阴影。每级一次 pass，只写进对应层的深度图，
        // 不触碰颜色附件；级数 = 1 时与阶段 D 的单一全场景阴影图完全等价。
        // 扩展二十四：每级只提交落在该级覆盖范围内的遮挡体（掩码为空即全量提交）。
        for level in 0..shadow_plan.levels as usize {
            let runs = shadow_runs(&groups, shadow_plan.masks.get(level).map(|m| m.as_slice()));
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-shadow-pass",
                &self.shadow_layer_views[level],
                &self.shadow_pass_bind_groups[level],
                ShadowSubmit {
                    groups: &groups,
                    runs: &runs,
                    stride,
                },
            );
        }

        // 扩展十八 C：局部光源阴影。层号由 `plan_local_shadows` 的固定槽位决定
        // （点光 6 面、聚光 1 面），每层一次 pass、一次矩阵 uniform。
        // 没有任何灯投影时 `local_shadow_layers` 为空，这一段整体跳过。
        // 局部阴影不做按级裁剪：它们的视锥互不重叠且每盏灯覆盖范围独立，
        // 用方向光的级联掩码套上去只会更慢（扩展二十四明确不覆盖这条路径）。
        let local_runs = shadow_runs(&groups, None);
        for layer in &local_shadow_layers {
            let idx = *layer as usize;
            shadow_draws += self.draw_shadow_pass(
                &mut encoder,
                "r3e-local-shadow-pass",
                &self.local_shadow_layer_views[idx],
                &self.local_shadow_pass_bind_groups[idx],
                ShadowSubmit {
                    groups: &groups,
                    runs: &local_runs,
                    stride,
                },
            );
        }

        // 扩展十九：深度预通道。主通道的深度挂在 4× 附件上、不能给遮蔽通道采样，
        // 所以先用单采样 `depth_view` 把不透明批次的深度铺一遍。
        // 只画不透明段：半透明物件本来就不写深度。
        let depth_prepass_draws =
            self.draw_depth_prepass(&mut encoder, &groups[..transparent_start]);

        // 扩展二十二：把这张单采样深度整张 copy 到回读缓冲，供**后续帧**做遮挡判定。
        // 读回是低频的（默认每 4 帧一次），且在飞期间不再发起新的 copy；
        // 关闭时 `encode_readback` 根本不会被调用，命令缓冲与之前逐字节一致。
        let depth_readback = occlusion_settings.enabled
            && self.occlusion.encode_readback(
                &self.device,
                &mut encoder,
                &self.depth_texture,
                self.size.0,
                self.size.1,
                scene.frame,
            );

        let (transparent_draws, transparent_instances);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r3e-main-pass"),
                // 扩展十三：主通道写离屏 HDR 目标（Rgba16Float，线性），
                // 高亮不再被 sRGB 8 位目标直接切平；贴到屏幕由 composite 完成。
                // 扩展十九：颜色先光栅化到 4× 多重采样附件上，由硬件 resolve
                // 进同一个 HDR 目标 —— 后处理链照旧只看到单采样纹理。
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_color_view,
                    depth_slice: None,
                    resolve_target: Some(self.postfx.hdr_view()),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.x as f64,
                            g: bg.y as f64,
                            b: bg.z as f64,
                            a: bg.w as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    // 多重采样深度：只服务于本通道的深度测试，
                    // 内容用完即弃（单采样深度已由预通道写好）
                    view: &self.msaa_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(2, &self.joint_bind_group, &[]);
            // 阶段 D：主通道采样上一步渲好的阴影贴图
            pass.set_bind_group(3, &self.shadow_bind_group, &[]);
            // 不透明段：写深度、顺序无关，分组顺序即首次出现顺序
            let (opaque_draws, opaque_instances) =
                self.draw_group_range(&mut pass, &groups[..transparent_start]);

            // M2-C03b：地形是不透明几何，插在不透明段之后、半透明段之前。
            // 独立管线而不是折进 PBR 批次：地形顶点只带 position/normal/uv/4 层权重，
            // 硬塞进 PBR 布局要动每一个既有着色器入口（`mesh.rs` 的顶点槽已用满 15）。
            let terrain_draws = self.draw_terrain(&mut pass);

            // 扩展十九：半透明段。绘制顺序已在 `prepare_instances` 里排成
            // 「由远及近」，这里换成不写深度的管线按同一顺序提交，混合才正确。
            pass.set_pipeline(&self.transparent_pipeline);
            let transparent = self.draw_group_range(&mut pass, &groups[transparent_start..]);
            transparent_draws = transparent.0;
            transparent_instances = transparent.1;

            let draw_calls = opaque_draws + transparent_draws + terrain_draws;
            let instances = opaque_instances + transparent_instances;

            self.stats = RenderStats {
                draw_calls,
                instances,
                culled,
                cull_unknown,
                shadow_draws,
                shadow_cascades: shadow_plan.levels,
                shadow_casters: shadow_plan.tested,
                shadow_culled: shadow_plan.culled,
                shadow_pruned: shadow_plan.pruned,
                shadow_branch_pruned: shadow_plan.branch_pruned,
                shadow_box_tests: shadow_plan.box_tests,
                caster_tree_builds: self.caster_tree_builds,
                cull: CullStats {
                    tree_builds: self.cull_tree_builds,
                    box_tests: plan.cull_box_tests,
                    instance_tests: plan.cull_instance_tests,
                    pruned: plan.cull_pruned,
                    branch_pruned: plan.cull_branch_pruned,
                },
                frame_index: scene.frame,
                text_glyphs: 0,
                text_draws: 0,
                text_missing: 0,
                ui_quads: 0,
                ui_draws: 0,
                postfx: PostFxStats::default(),
                ssao: SsaoStats::default(),
                local_lights,
                local_shadow_casters,
                local_shadow_layers: local_shadow_layers.len() as u32,
                transparent_draws,
                transparent_instances,
                depth_prepass_draws,
                occlusion: self.occlusion.stats(),
                // 扩展二十三：探针状态在装载时就定了，逐帧原样带出
                env_probe: self.env_probe_stats(),
                // 扩展三十一：上传读数在上传那一刻就定型，这里原样抄进来
                instance_upload: self.instance_upload,
                // 扩展三十二：关节矩阵的脏段读数，同样是上传时定型
                joint_upload: self.joint_upload,
                // 扩展三十五：暂存环（一帧一次写入 + 多帧飞行）的累计读数
                upload_staging: self.staging.snapshot(),
                // 扩展三十六：形变表规模与累计重传次数
                morph_floats: self.morph_floats,
                morph_uploads: self.morph_uploads,
                // 扩展三十六：真正参与形变的实例数（权重非零且网格带形状）
                morph_instances,
                // 扩展三十八：其中权重被互斥组归一化过的数量，以及本帧余量合计
                morph_normalized,
                morph_cull_inflate,
                morph_masked,
                morph_masked_shapes,
            };
        }

        // 扩展十三：后处理链在文本 / UI 之前收口。
        // 顺序很关键——composite 是唯一写 surface 的通道，文本与 UI 随后往它上面
        // 用 LoadOp::Load 叠加，这样 HUD 的颜色不会被曝光与色调映射二次调色。
        let postfx_settings = scene.postfx.unwrap_or_default();
        // 扩展十七：遮蔽夹在「主通道画完」与「composite 写 surface」之间。
        // 早一步深度还没写满，晚一步就没有第二个通道能把它乘回颜色了。
        self.ssao.render(
            &self.queue,
            &mut encoder,
            &scene.camera,
            &postfx_settings.ssao,
        );
        self.stats.ssao = self.ssao.stats();
        // 世代号没变就不会重建绑定组，可以放心每帧调
        let (ao_key, ao_view) = if self.stats.ssao.enabled {
            (self.ssao.generation(), Some(self.ssao.ao_view()))
        } else {
            (0, None)
        };
        self.postfx.bind_ao(&self.device, ao_key, ao_view);
        self.postfx.render(
            &self.device,
            &self.queue,
            &mut encoder,
            view,
            &postfx_settings,
        );
        self.stats.postfx = self.postfx.stats();

        // M1 文字渲染：主通道画完后叠加屏幕空间文本。
        // 独立 pass（LoadOp::Load）+ 独立管线，不共享深度与相机，关掉即彻底不跑。
        // 场景开关（脚本 set_text_enabled）优先：关掉时只传空切片，通道一次 GPU 命令都不发
        let texts = if scene.text_enabled {
            &scene.texts[..]
        } else {
            &[][..]
        };

        // M1 最小 UI：与文本同构的独立 passes。层次关系只需调换两个通道的提交顺序：
        // 默认 UI 先画、文本后画 = 文本压在 UI 之上（HUD 数字盖在血条上）；
        // 脚本 set_ui_above_text(true) 则反过来。两者都写 LoadOp::Load，顺序即层次。
        let uis = if scene.ui_enabled {
            &scene.ui[..]
        } else {
            &[][..]
        };
        if scene.ui_above_text {
            self.ui.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                uis,
                self.size,
            );
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                texts,
                self.size,
            );
        } else {
            self.text.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                texts,
                self.size,
            );
            self.ui.render(
                &self.device,
                &self.queue,
                &mut encoder,
                view,
                uis,
                self.size,
            );
        }
        // 统计在真正提交后读取，避免拿到上一帧的数值
        let text_stats = self.text.stats();
        self.stats.text_glyphs = text_stats.glyphs;
        self.stats.text_draws = text_stats.draw_calls;
        self.stats.text_missing = text_stats.missing;
        let ui_stats = self.ui.stats();
        self.stats.ui_quads = ui_stats.quads;
        self.stats.ui_draws = ui_stats.draw_calls;

        self.queue.submit(Some(encoder.finish()));
        // 扩展三十五：槽 → 实例/关节的块拷贝已随本帧提交，此刻才允许把影子
        // 对齐到本帧内容；提交前更新会让「没提交的帧」永久漏传（扩展三十一不变量）。
        self.staging
            .commit_shadows(&mut self.instance_shadow, &mut self.joint_shadow);
        // 扩展二十二：命令提交后才挂 map_async —— 此时 copy 已入队，
        // 回调在下一次 `poll` 里被触发，本帧绝不等待 GPU。
        if depth_readback {
            self.occlusion.after_submit();
        }
    }

    /// 离屏渲染一帧并把像素读回 CPU（P0-4 截图）。
    ///
    /// 走的是 [`Self::render_to_view`]，因此画面与窗口里看到的同源：阴影 / 遮蔽 /
    /// 后处理 / 文本 / UI 全部在内。区别只有两点：目标是普通纹理（不碰 swapchain，
    /// 不依赖窗口是否可见），以及读回后 alpha 一律写成 255（surface 的 alpha 模式
    /// 可能是 Opaque，原样落盘会得到一张全透明 PNG）。
    ///
    /// 本方法会推进一帧：`frame_index`、阴影 pass、后处理累积历史都会前进一格。
    pub fn capture_frame(&mut self, scene: &Scene) -> Result<CapturedFrame, String> {
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return Err(format!("绘制尺寸异常（{width}x{height}），无法截图"));
        }
        let format = self.config.format;
        // 复制粒度决定了读回的排布，非 8bit RGBA/BGRA 的 surface 这里直接拒绝，
        // 而不是给出一张被解读错位的花屏图。
        let swizzle_bgra = match format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            other => {
                return Err(format!(
                    "surface 格式 {other:?} 不是 8bit RGBA/BGRA，暂不支持截图"
                ))
            }
        };

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r3e-capture-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to_view(scene, &view);

        // copy_texture_to_buffer 要求每行按 256 字节对齐：行尾填充必须自己丢掉，
        // 否则图像会沿对角线逐行倾斜。
        let unpadded_row = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r3e-capture-readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("r3e-capture-blit"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| format!("等待 GPU 读回失败: {err:?}"))?;
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(format!("映射截图缓冲失败: {err:?}")),
            Err(err) => return Err(format!("截图回调未送达: {err}")),
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|err| format!("读取截图缓冲失败: {err:?}"))?;
        let mut rgba8 = Vec::with_capacity((unpadded_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_row) as usize;
            let line = &mapped[start..start + unpadded_row as usize];
            for px in line.chunks_exact(4) {
                if swizzle_bgra {
                    rgba8.extend_from_slice(&[px[2], px[1], px[0], 255]);
                } else {
                    rgba8.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        drop(mapped);
        buffer.unmap();

        Ok(CapturedFrame {
            width,
            height,
            rgba8,
            format,
        })
    }
}
