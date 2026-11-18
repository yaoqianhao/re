use std::collections::HashMap;

use glam::{Mat4, Vec4};
use r3e_core::{Aabb, CasterTree, Frustum, MeshRef, Scene, Sh9};

use crate::mesh::{self, InstanceRaw};
use crate::pbr::SurfaceKey;
use crate::transparency;

use super::*;

/// 批次键：网格 + 物件级 PBR 材质 + 场景材质（`materials[]` 表项解析结果）。
///
/// 抽成别名只为压平 clippy 的 `type_complexity`：这个三元组在分组键、
/// 分桶键、`PreparedNode` 里反复出现，语义是同一个「批次键」。
type GroupKey = (
    MeshRef,
    Option<SurfaceKey>,
    Option<crate::pbr::SceneMaterialKey>,
);

/// 分桶键：批次键三元组平铺 + 是否半透明段（同一个批次键可能在两段各成一个桶）。
///
/// 用平铺而不是 `(GroupKey, bool)`：插入与取出两侧都按「三元组 + 段标记」书写，
/// 嵌套写法会让 `buckets.entry((k.0, k.1, k.2, transparent))` 这类调用点全要改。
type BucketKey = (
    MeshRef,
    Option<SurfaceKey>,
    Option<crate::pbr::SceneMaterialKey>,
    bool,
);

impl Renderer {
    /// 把场景里可见的物件整理成按 `MeshRef` 分组的实例区间，并上传到 GPU。
    ///
    /// 返回值附带本帧的剔除统计
    /// `(绘制分组, 剔除数, 缺包围盒数, 全部可见物件的世界包围盒)`；
    /// 最后一项供阴影通道生成光源正交相机（阶段 D）。
    pub(crate) fn prepare_instances(&mut self, scene: &Scene) -> InstancePlan {
        // 更新相机 / 光照 uniform
        let mut camera = scene.camera;
        camera.aspect = self.aspect();
        let light = scene.light;
        // 环境探针：场景里写了 `environment` 才启用；球谐只在配置真变了时重投影
        let environment = scene.environment.map(|e| e.sanitized());
        if environment != self.env_cache {
            self.env_cache = environment;
            self.env_sh = match environment {
                Some(env) => Sh9::project(&env),
                None => Sh9::default(),
            };
            self.env_builds += 1;
        }
        let env = environment.unwrap_or_default();
        // 扩展二十三：配了 HDR 探针时，漫反射系数换成探针投影出的辐照度；
        // 否则维持 `environment` 段（或全零）—— 未配探针时这条路径与扩展十二逐字节相同。
        let sh_source = match self.env_probe.as_ref() {
            Some(probe) => probe.irradiance,
            None => self.env_sh,
        };
        let mut sh = [[0.0_f32; 4]; r3e_core::env::SH_COEFFICIENTS];
        for (dst, src) in sh.iter_mut().zip(sh_source.coeffs.iter()) {
            *dst = [src.x, src.y, src.z, 0.0];
        }
        // 扩展二十三：探针开关 + mip 层数 + 强度 + 面边长，着色器据此决定镜面路径
        let probe = self.env_probe.as_ref().map(|p| p.stats).unwrap_or_default();
        let probe_uniform = [
            if probe.active { 1.0 } else { 0.0 },
            probe.mip_levels as f32,
            probe.intensity,
            probe.face_size as f32,
        ];
        let uniform = CameraUniform {
            view_proj: camera.view_projection().to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_dir: [light.direction.x, light.direction.y, light.direction.z, 0.0],
            light_color: [light.color.x, light.color.y, light.color.z, light.intensity],
            ambient: [light.ambient.x, light.ambient.y, light.ambient.z, 1.0],
            env_zenith: [
                env.sky_zenith[0],
                env.sky_zenith[1],
                env.sky_zenith[2],
                env.intensity,
            ],
            // w 位就是「本帧用哪套环境光」的开关：0 = 退回常数 ambient
            env_horizon: [
                env.sky_horizon[0],
                env.sky_horizon[1],
                env.sky_horizon[2],
                if environment.is_some() { 1.0 } else { 0.0 },
            ],
            env_ground: [env.ground[0], env.ground[1], env.ground[2], 0.0],
            env_sun: [
                env.sun_color[0] * env.sun_intensity,
                env.sun_color[1] * env.sun_intensity,
                env.sun_color[2] * env.sun_intensity,
                0.0,
            ],
            env_sun_dir: [env.sun_dir[0], env.sun_dir[1], env.sun_dir[2], 0.0],
            env_sh: sh,
            env_probe: probe_uniform,
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // 分组键：网格 + 物件级 PBR 材质 + 是否半透明。内置网格与资产网格统一处理。
        //
        // 扩展十九：半透明的判定 = 节点颜色 alpha × 网格材质 alpha（两者都可能 < 1）。
        // 半透明键集中在 `transparent_keys`，排到所有不透明键之后 —— 于是
        // `groups` 天然形成「不透明段 + 半透明段」，绘制时只需切换一次管线。
        //
        // 扩展二十七：旧写法是「外层遍历批次键、内层遍历全场景」，同一份场景要按
        // `键数 × 节点数` 反复过滤一遍；键数（网格 × 材质组合）几十上百时，这层过滤
        // 的开销盖过了它省下的东西。现在改成**单次遍历 + 分桶**：每个实例只在自己的
        // 桶里被收一次，视锥/遮挡剔除与关节槽位分配一并跟进这一遍。桶内顺序仍是场景
        // 迭代顺序，键的登记顺序仍是「首次出现」且不透明在前，所以摊平出来的
        // `groups` 区间与旧实现逐个相同（像素基线不动）。
        let mut opaque_keys: Vec<GroupKey> = Vec::new();
        let mut transparent_keys: Vec<GroupKey> = Vec::new();
        // 桶键带「段」标志：同一个 (网格, 材质) 组合可能同时落在不透明段与半透明段
        // （同一网格的不同物件 alpha 不同），两段各自成桶、各自摊平 —— 与旧实现的
        // 「两段各自全场景过滤一遍」等价，但每个实例只被剔除/构造成一次。
        let mut buckets: HashMap<BucketKey, Vec<u32>> = HashMap::new();
        // 全部待摊平实例，按场景迭代顺序；桶里只存下标，两段共享同一份实例数据
        let mut staged_all: Vec<StagedInstance> = Vec::new();
        // 扩展三十六：本帧被形变的实例数（网格带形状 + 权重非零）
        let mut morph_instances = 0u32;
        // 扩展三十八：其中**互斥组超预算、权重被归一化过**的实例数。
        // 0 = 场景没配互斥组（或配了但没超预算），进缓冲的权重与场景原值逐位相同。
        let mut morph_normalized = 0u32;
        // 扩展三十八：本帧逐形状剔除余量合计（世界单位），进 `RenderStats`
        let mut morph_cull_inflate = 0.0f32;
        // 扩展四十：被逐实例形状掩码整段摘掉形变的实例数（`morph_count` 写 0 的那批）
        let mut morph_masked = 0u32;
        // 扩展四十：被掩码挡在门外的非零权重槽合计（省下的形变乘加组数）
        let mut morph_masked_shapes = 0u32;

        // 一个待摊平实例：实例数据 + 阴影用包围盒 + 归属节点 + 半透明排序深度
        struct StagedInstance {
            instance: InstanceRaw,
            bounds: Aabb,
            node: r3e_core::NodeId,
            depth: f32,
        }

        // 每帧重算关节槽位：同一块 storage buffer 按实例依次排布（P4）
        self.joint_scratch.clear();
        let mut joint_overflow = 0usize;
        // 阶段三 A：先用 6 个视锥平面过一遍节点，被剔除的实例不占实例槽位、也不占关节槽位
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let mut culled = 0u32;
        let mut cull_unknown = 0u32;
        // 扩展二十二：遮挡剔除。判定用的是**上一轮**读回的金字塔（比画面滞后几帧），
        // 开关关闭或金字塔未就绪时 `occlusion_ready == false`，下面这段整体跳过，
        // 实例数与关闭前逐个一致。
        let occlusion_settings = scene.occlusion.sanitized();
        let occlusion_ready = occlusion_settings.enabled && self.occlusion.pyramid().is_some();
        let view_projection = camera.view_projection();
        let (screen_width, screen_height) = self.size;
        // 阶段 D：把「所有可见（含被视锥剔除）物件」的包围盒并起来，
        // 交给阴影通道拟合正交相机 —— 阴影要考虑镜头外的遮挡物，
        // 所以这里统计在剔除判断之前。
        let mut scene_bounds: Option<Aabb> = None;
        // 同一网格常被多次引用，材质 alpha 只查一次
        let mut mesh_alphas: HashMap<MeshRef, f32> = HashMap::new();

        // 第一遍的中间态：判剔除 + 构造实例所需的全部信息。
        // 存下来是为了第二遍不必回查场景（树节点下标也算好了）。
        struct PreparedNode {
            id: r3e_core::NodeId,
            /// 场景树节点下标；`None` = 节点不在树里（不该发生，按保留处理）
            tree_slot: Option<usize>,
            /// 世界包围盒；`None` = 网格没有 AABB（fail-open，永不剔除）
            world: Option<Aabb>,
            key: (
                MeshRef,
                Option<SurfaceKey>,
                Option<crate::pbr::SceneMaterialKey>,
            ),
            world_matrix: Mat4,
            color: Vec4,
            /// 该网格的形变登记（无形状为 `None`），第二遍填进 `InstanceRaw`。
            morph: Option<MeshMorph>,
            /// 扩展四十二：裁剪并打包好的形状槽列表（槽序权重 + 序号字 + 槽数）。
            /// 与 `morph` 同生共死 —— `morph` 为 `None` 时这里是全 0。
            upload: MorphUpload,
        }

        // 扩展二十九：视锥剔除从「逐实例判定」升级成「按子树预筛」，复用扩展二十五
        // 在阴影通道验证过的 `CasterTree` 套路。办法是把原来的单遍拆成两遍：
        //   第一遍只收集 —— 算世界包围盒、并进 `scene_bounds`、把盒写回层级树节点。
        //     这一遍**不看视锥**：子树盒要等所有实例的盒都写进去才聚合得出来。
        //   第二遍才判视锥 —— 拿掩码，被整枝剪掉的实例连一次 AABB 测试都不做。
        // 中间态 `prepared` 保持场景迭代顺序，所以桶内顺序、键的首次出现顺序与
        // 单遍实现逐个相同，摊平出来的 `groups` 与像素基线都不动。
        let mut prepared: Vec<PreparedNode> = Vec::with_capacity(scene.len());
        self.sync_cull_layout(scene);
        let tree_len = self.cull_tree.as_ref().map_or(0, CasterTree::len);
        if self.cull_node_bounds.len() != tree_len {
            self.cull_node_bounds.resize(tree_len, Aabb::EMPTY);
            self.cull_node_casts.resize(tree_len, 0);
        }
        for bounds in self.cull_node_bounds.iter_mut() {
            *bounds = Aabb::EMPTY;
        }
        for casts in self.cull_node_casts.iter_mut() {
            *casts = 0;
        }

        for node in scene.iter() {
            if !node.visible {
                continue;
            }
            // S4-A：物件引用了场景材质表里的材质时，它整份取代网格资产材质与
            // 物件级覆盖，所以此时把 surface 抹掉 —— 留着只会让同一份材质按
            // 「永远被忽略的 metallic/roughness」拆成多个批次，画面完全一样。
            let scene_material = scene.node_scene_material(node.id).map(|decl| {
                crate::pbr::SceneMaterialKey::new(decl, scene.texture_base_dir.as_deref())
            });
            let surface = node.material.map(SurfaceKey::new);
            let surface = if scene_material.is_some() {
                None
            } else {
                surface
            };
            let key = (node.mesh, surface, scene_material);
            // 扩展三十八：形变权重每帧只在这里定一次 —— 互斥归一化出的是**副本**，
            // 场景里的权重保持不变（脚本写 1.0 读回还是 1.0，只有着色器吃到 0.5）。
            // 扩展四十：形状掩码也在这里一次定下来。归一层已经把掩码外的槽位清 0，
            // 所以「剔除用的权重」与「着色器吃到的权重」仍然是同一份。
            let (morph, morph_weights, morph_scaled, upload, masked_slots) = {
                let registered = self.mesh_cache.morph_of(node.mesh);
                match scene.morph_of(node.id) {
                    Some(instance) => {
                        let (weights, scaled) = instance.normalized_weights();
                        // 扩展四十二：槽列表按「网格真有几个形状」裁剪后再打包。
                        // 脚本里实例可能启用了网格并不存在的形状序号，这些槽不占
                        // 带宽也不进循环 —— 位图方案下它们只是「亮着但无表可读」。
                        let (upload, masked) = match registered {
                            Some(m) => (
                                morph_upload(instance, m.count as usize),
                                // 网格没登记形状时没有形变表可省，这个读数只对
                                // 「有表却被掩码挡在槽外」的形状有意义
                                instance.masked_nonzero_count(),
                            ),
                            None => (MorphUpload::zero(), 0),
                        };
                        (registered, weights, scaled, upload, masked)
                    }
                    None => (
                        registered,
                        [0.0; r3e_core::MAX_MORPH_TARGETS],
                        false,
                        // 没有实例绑定时权重本来就是全 0，「被掩掉的槽」无从谈起
                        MorphUpload::zero(),
                        0,
                    ),
                }
            };
            if morph_scaled && morph.is_some_and(|m| m.count > 0) {
                morph_normalized += 1;
            }
            // 扩展四十：掩码把形状集关空 → 这个实例整段摘掉形变。
            //
            // 这是掩码唯一的「硬」收益：`morph_count` 写 0 后着色器在
            // `imeta.w == 0` 处直接返回，连 8 次权重比较都不做。权重置 0 做不到
            // 这一点 —— 那是「算了再乘 0」。半掩的实例仍走完整路径（形状集是
            // 动态索引的，跳着取反而更贵），收益体现在这一格权重被归一化清 0。
            let morph = if morph.is_some_and(|m| m.count > 0) && upload.count == 0 {
                morph_masked += 1;
                morph_masked_shapes += masked_slots;
                None
            } else {
                morph_masked_shapes += masked_slots;
                morph
            };
            let world_bounds = match self.mesh_cache.bounds(node.mesh) {
                Some(local) => {
                    // 蒙皮网格的形变会跑出静止姿态包围盒，按比例留余量再测
                    let world = local.transformed(&node.world);
                    let world = if scene.skin_of(node.id).is_some() {
                        world.inflate_relative(SKIN_CULL_MARGIN)
                    } else {
                        world
                    };
                    // 扩展三十六 / 三十八：形变同理，但外扩量是绝对值 —— 逐形状
                    // 「位移模长 × 该形状权重（取绝对值）」求和 × 实例最大轴向缩放。
                    // 权重为 0（不播放表情）时外扩为 0，不吃任何额外代价；表情越夸张
                    // 盒越大，屏幕上就不会「笑着消失」。逐形状求和比扩展三十六的
                    // 「最长位移 × 峰值权重」更紧：只有眨眼在动时，盒不会被张嘴的
                    // 15 cm 位移连累撑大七倍。
                    let world = match morph {
                        Some(morph) if morph.max_displacement > 0.0 => {
                            let inflate = morph
                                .cull_inflate_weights(&morph_weights, max_axis_scale(&node.world));
                            if inflate > 0.0 {
                                morph_cull_inflate += inflate;
                                world.inflate(inflate)
                            } else {
                                world
                            }
                        }
                        _ => world,
                    };
                    scene_bounds = Some(match scene_bounds {
                        Some(b) => b.union(&world),
                        None => world,
                    });
                    Some(world)
                }
                // 拿不到包围盒就不剔除（fail-open），宁可多画也不让物件凭空消失
                None => {
                    cull_unknown += 1;
                    None
                }
            };
            // 写回层级树：缺盒的实例按 fail-open 巨盒登记 —— 它所在的那整枝都不能被
            // 预筛剪掉，与逐实例实现里「拿不到包围盒就不剔」是同一句话。
            //
            // 注：r3e 没有「纯分组节点」这一说（`ObjectDesc.mesh` 缺省就是内置 cube，
            // `MeshRef` 也没有空变体），所以每个节点自身都至少有一个实例、都是 `casts=1`。
            let tree_slot = self.cull_slots.get(&node.id).copied();
            if let Some(tree_slot) = tree_slot {
                self.cull_node_bounds[tree_slot] =
                    world_bounds.unwrap_or_else(r3e_core::fail_open_bounds);
                self.cull_node_casts[tree_slot] = 1;
            }
            prepared.push(PreparedNode {
                id: node.id,
                tree_slot,
                world: world_bounds,
                key,
                world_matrix: node.world,
                color: node.color,
                morph,
                upload,
            });
        }

        // 树从他「全场景可见节点」的盒聚合出子树盒，自顶向下筛一遍
        let cull_plan = match self.cull_tree.as_mut() {
            Some(tree) => {
                tree.refresh(&self.cull_node_bounds, &self.cull_node_casts);
                Some(tree.plan_frustum(&frustum))
            }
            None => None,
        };
        let (cull_box_tests, cull_instance_tests, cull_pruned, cull_branch_pruned) =
            match &cull_plan {
                Some(plan) => (
                    plan.counts.box_tests,
                    plan.counts.instance_tests,
                    plan.counts.pruned,
                    plan.counts.branch_pruned,
                ),
                None => (0, 0, 0, 0),
            };

        // 扩展三十：遮挡剔除同样升级成按子树预筛，复用上面刚 refresh 过的那棵树
        // （同一帧、同一批盒，不必重建）。与视锥不同的是这里要额外证明「整枝跳过
        // 不误剔」：子树盒 B 判遮挡 ⇒ 任意子盒 A ⊆ B 也判遮挡（A 的屏幕矩形更小、
        // 最近深度更远，而阈值随最近深度单调），证明写在 `plan_occlusion` 文档里，
        // 差分测试 `plan_occlusion_matches_per_instance` 钉住两路径逐实例一致。
        // 金字塔没就绪（首帧 / 刚 resize / 开关关闭）时 `plan` 返回 `None`，
        // 下面整段退回逐实例 `test`，实例数与关闭该功能时逐个相同。
        let occlusion_plan = match (self.cull_tree.as_ref(), occlusion_ready) {
            (Some(tree), true) => self.occlusion.plan(
                &occlusion_settings,
                tree,
                &view_projection,
                screen_width,
                screen_height,
            ),
            _ => None,
        };
        if let Some(plan) = &occlusion_plan {
            // 记账在循环外做一次：`record_hierarchy` 要 `&mut self`，循环里还要读
            // 掩码，分开恰好避开了同时借用。
            self.occlusion.record_hierarchy(plan);
        }

        // 第二遍：判剔除 + 入桶
        for item in &prepared {
            // 层级预筛：拿不到掩码（树没建起来 / 节点不在表里）一律保留，宁可多画
            if let Some(plan) = &cull_plan {
                let keep = item
                    .tree_slot
                    .and_then(|slot| plan.mask.get(slot).copied())
                    .unwrap_or(true);
                if !keep {
                    culled += 1;
                    continue;
                }
            }
            let key = item.key;
            let mesh_alpha = *mesh_alphas
                .entry(key.0)
                .or_insert_with(|| self.mesh_material_alpha(key.0));
            let transparent = transparency::is_transparent(item.color.w * mesh_alpha);
            // 键顺序与旧实现同构：不透明键一段、半透明键一段，各段内按首次出现
            let bucket_keys = if transparent {
                &mut transparent_keys
            } else {
                &mut opaque_keys
            };
            if !bucket_keys.contains(&key) {
                bucket_keys.push(key);
            }
            let mut occluded = false;
            let world_bounds = item.world;
            // 扩展二十二 / 三十：视锥内还要再问一句「是不是被前面的东西挡满了」。
            // 只剔不透明批次：半透明物件不写深度，金字塔里根本没有它们的贡献，
            // 用不透明深度去剔半透明会让玻璃 / 水面整层凭空消失。
            //
            // 判定走层级预筛：掩码直接给出「这个节点的实例要不要画」，被整枝跳过的
            // 实例连 `project_aabb` 都不做。拿不到掩码（没建树 / 节点不在表里 /
            // 金字塔未就绪）才退回逐实例 `test` —— 缺 AABB 的节点在树里登记的是
            // fail-open 巨盒，掩码必为「照画」，与逐实例口径一致。
            if occlusion_ready {
                let culled_by_occlusion = match (&occlusion_plan, item.tree_slot) {
                    (Some(plan), Some(slot)) => !plan.keeps(slot),
                    // 树里没有这个节点：掩码无从查起，按可见处理
                    (Some(_), None) => false,
                    (None, _) => world_bounds.is_some_and(|world| {
                        self.occlusion.test(
                            &occlusion_settings,
                            &world,
                            &view_projection,
                            screen_width,
                            screen_height,
                        )
                    }),
                };
                if culled_by_occlusion {
                    // 同一实例还会进半透明段（半透明不写深度，金字塔里没有它的
                    // 贡献），所以这里只标记「不透明段不要它」，不整体丢弃
                    occluded = true;
                }
            }
            // 扩展二十四：阴影裁剪同样 fail-open —— 没有 AABB 的实例
            // 一律当成「可能在任何一级里」，绝不因为缺数据把影子弄丢。
            let caster_box = world_bounds.unwrap_or_else(r3e_core::fail_open_bounds);
            let color: Vec4 = item.color;
            let joint_base = match scene.skin_of(item.id) {
                Some(skin) if skin.is_solved() => {
                    let need = skin.joint_matrices.len();
                    if self.joint_scratch.len() + need > r3e_core::MAX_JOINT_MATRICES {
                        joint_overflow += need;
                        0
                    } else {
                        let base = self.joint_scratch.len() as u32;
                        for m in &skin.joint_matrices {
                            self.joint_scratch.push(m.to_cols_array());
                        }
                        base
                    }
                }
                _ => 0,
            };
            // 扩展三十八：权重在第一遍就已按互斥组归一化好，这里只做搬运 ——
            // 剔除用的盒与着色器吃到的权重必须是同一份，否则「两个表情各 1.0」
            // 的实例会按两倍位移外扩，盒大一圈、遮挡剔除白白失效。
            let morph = item.morph;
            let upload = &item.upload;
            // 扩展三十六：只有「网格真有形状」且「实例至少有非零权重」时才计入，
            // 否则着色器不会进形变分支，顶点输出与扩展三十五逐位相同。
            // 扩展四十二：判据换成槽序权重（上传的那一份），与着色器同源。
            if upload.count > 0 && upload.weights.iter().any(|w| *w != 0.0) {
                morph_instances += 1;
            }
            let instance = InstanceRaw {
                model: item.world_matrix.to_cols_array_2d(),
                color: color.to_array(),
                joint_base,
                // 扩展三十六：形变登记（无形状时全 0，着色器看 morph_count = 0 直接跳过）
                morph_base: morph.map_or(0, |m| m.base),
                morph_stride: morph.map_or(0, |m| m.stride),
                // 扩展四十二：`morph_count` 现在是**槽数**（≤ 网格形状数），
                // 不是网格共有几个形状 —— 循环上界与槽列表长度必须一致。
                morph_count: upload.count,
                // 扩展三十九：扁平权重按 4 个一组切进 `vec4` 槽位；场景没登记形变时全 0
                morph_weights: mesh::group_morph_weights(&upload.weights),
                // 扩展四十二：形状序号字。第一遍已按网格形状数裁剪，
                // 且与 `morph_count == 0` 绑定 —— 槽清空的实例这里全是 0，
                // 着色器读到 `imeta.w == 0` 就先短路，序号字根本不会被拆。
                morph_slots: upload.slots,
            };
            // 平移列就是实例的世界位置；用它到相机的距离平方当半透明排序键
            let depth =
                transparency::view_depth(item.world_matrix.w_axis.truncate(), camera.position);
            let slot = staged_all.len() as u32;
            staged_all.push(StagedInstance {
                instance,
                bounds: caster_box,
                node: item.id,
                depth,
            });
            // 扩展二十八：实例**只进自己那一段**的桶。段归属由节点自身的透明性决定，
            // 不再"两段各来一份"。批次键是 (网格, 材质)，不含透明度，段内又收"所有匹配
            // 该键的节点"，于是同一个网格若既有不透明物件又有玻璃物件，不透明段会把玻璃
            // 也当不透明画一遍、半透明段会把实心物件当玻璃再画一遍 —— 一份几何，两次绘制。
            // 这两条入桶语句保持单行：`instance_buckets_contract` 与
            // `frustum_cull_wiring` 都在源码里定位 `buckets.entry(` 这个动作
            // 本身（以及它所在分支的缩进），被 `cargo fmt` 折行会让它们失配。
            if transparent {
                // 半透明物件不写深度，金字塔里没有它的贡献，因此永不被遮挡剔除
                #[rustfmt::skip]
                buckets.entry((key.0, key.1, key.2, true)).or_default().push(slot);
            } else if !occluded {
                #[rustfmt::skip]
                buckets.entry((key.0, key.1, key.2, false)).or_default().push(slot);
            }
        }

        // 摊平：不透明批次先入组，半透明批次攒到最后统一排序
        let opaque_key_count = opaque_keys.len();
        let mut keys = opaque_keys;
        keys.extend(transparent_keys);

        let mut all: Vec<InstanceRaw> = Vec::new();
        let mut groups: Vec<DrawGroup> = Vec::new();
        // 扩展二十四：与实例缓冲同序的遮挡体包围盒，供阴影按级裁剪使用
        let mut caster_bounds: Vec<Aabb> = Vec::new();
        // 扩展二十五：与实例缓冲同序的节点 id —— 层级剔除靠它把实例挂回场景树
        let mut caster_nodes: Vec<r3e_core::NodeId> = Vec::new();
        // 半透明批次 (组内最远深度, 网格, 材质, 实例)：等全部实例算完后统一排序
        type TransparentBatch = (
            f32,
            MeshRef,
            Option<SurfaceKey>,
            Option<crate::pbr::SceneMaterialKey>,
            Vec<u32>,
        );
        let mut pending_transparent: Vec<TransparentBatch> = Vec::new();
        for (index, entry) in keys.iter().enumerate() {
            let segment_transparent = index >= opaque_key_count;
            let (key, surface, scene_material) = *entry;
            let Some(order_list) =
                buckets.remove(&(key, surface, scene_material, segment_transparent))
            else {
                continue;
            };
            if order_list.is_empty() {
                continue;
            }
            if segment_transparent {
                // 组间排序键 = 组内最远实例的深度：一次排序决定「哪个网格先画」，
                // 组内顺序则再按 `back_to_front` 排细
                let farthest = order_list.iter().fold(0.0_f32, |far, &slot| {
                    let depth = staged_all[slot as usize].depth;
                    far.max(if depth.is_nan() { f32::INFINITY } else { depth })
                });
                pending_transparent.push((farthest, key, surface, scene_material, order_list));
                continue;
            }
            let start = all.len() as u32;
            for slot in order_list {
                let item = &staged_all[slot as usize];
                all.push(item.instance);
                caster_bounds.push(item.bounds);
                caster_nodes.push(item.node);
            }
            if all.len() as u32 > start {
                groups.push((key, start..all.len() as u32, surface, scene_material));
            }
        }

        // 半透明段起点：不透明批次全部入组之后的位置
        let mut transparent_start = groups.len();
        // 组间：远组先画（`total_cmp` 下降序，同深度保持首次出现顺序）
        pending_transparent.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, key, surface, scene_material, order_list) in pending_transparent {
            let start = all.len() as u32;
            let depths: Vec<f32> = order_list
                .iter()
                .map(|&slot| staged_all[slot as usize].depth)
                .collect();
            let order = transparency::back_to_front(&depths);
            // 按「由远及近」的下标顺序把实例搬进实例缓冲
            for slot in order {
                let item = &staged_all[order_list[slot as usize] as usize];
                all.push(item.instance);
                caster_bounds.push(item.bounds);
                caster_nodes.push(item.node);
            }
            groups.push((key, start..all.len() as u32, surface, scene_material));
        }

        if joint_overflow > 0 {
            log::error!(
                "关节矩阵槽位不足（缺 {joint_overflow} 个，上限 {}），超出部分实例按非蒙皮绘制",
                r3e_core::MAX_JOINT_MATRICES
            );
        }
        // 扩展三十二：关节矩阵也走脏段上传 —— 同一姿势的蒙皮物件（无动画的
        // 摆件、暂停的动画）每帧矩阵逐字节相同，一个字节都不用再传。
        let joints = std::mem::take(&mut self.joint_scratch);
        self.upload_joint_runs(&joints);
        // 归还容量（`take` 出的 Vec 直接还给字段，下一帧 push 不用重新分配）
        self.joint_scratch = joints;
        self.joint_scratch.clear();

        // 确保用到的网格都已上传（内置即时生成，资产从注入的 MeshProvider 取）
        let mut uploaded: Vec<MeshRef> = Vec::new();
        for (key, ..) in &groups {
            if self.mesh_cache.ensure(&self.device, &self.queue, *key) {
                uploaded.push(*key);
            }
        }
        // 上传失败的网格从绘制分组中剔除；
        // 组数变化时透明段起点要跟着前移，否则会把不透明批次当成半透明来画。
        if uploaded.len() != groups.len() {
            let mut removed_opaque = 0usize;
            let mut index = 0usize;
            groups.retain(|(key, ..)| {
                let keep = uploaded.contains(key);
                if !keep && index < transparent_start {
                    removed_opaque += 1;
                }
                index += 1;
                keep
            });
            transparent_start -= removed_opaque;
        }

        if all.is_empty() {
            // 没有实例可画：影子与读数都归零，下一帧若又有实例会自动走全量
            // （影子长度为 0 → 与新版长度不等 → `plan_upload` 退化为全量）。
            self.instance_shadow.clear();
            // 扩展三十五：关节那一侧可能已经排进暂存环，这里仍要把它推上去
            self.staging.flush(&self.queue);
            self.instance_upload = InstanceUploadStats {
                resets: self.instance_upload_resets,
                ..Default::default()
            };
            return InstancePlan {
                groups,
                transparent_start,
                culled,
                cull_unknown,
                scene_bounds,
                caster_bounds,
                caster_nodes,
                cull_box_tests,
                cull_instance_tests,
                cull_pruned,
                cull_branch_pruned,
                morph_instances,
                morph_normalized,
                morph_cull_inflate,
                morph_masked,
                morph_masked_shapes,
            };
        }

        // 容量不足则扩容（按 2 倍增长，避免每帧重建）。
        // 扩展三十一：换代后的缓冲是未初始化内容，影子必须一起清空，
        // 否则脏段判定会以为新缓冲里已经有上一帧的数据，跳掉本该上传的段。
        if all.len() as u64 > self.instance_capacity {
            let mut cap = self.instance_capacity;
            while cap < all.len() as u64 {
                cap *= 2;
            }
            self.instance_capacity = cap;
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r3e-instance-buffer"),
                size: cap * std::mem::size_of::<InstanceRaw>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_shadow.clear();
            self.instance_shadow.shrink_to_fit();
            self.instance_upload_resets += 1;
            // 扩展三十五：换代后旧缓冲作废，暂存环里已排但未提交的拷贝必须丢弃
            // （否则会拷进刚被替换掉的那个 `wgpu::Buffer`）
            self.staging.drop_pending();
        }

        // 扩展三十一：只把「与上一帧逐字节不同」的连续段传上去。
        // 记录内容完全不改，所以画面与全量上传逐位一致；静态场景下这一段
        // 通常只有几段、几十个实例，而不是整张 960 KiB。
        self.upload_instance_runs(&all);
        // 扩展三十五：本帧的实例 + 关节脏段都已打进暂存区，这里一次性写进当前槽。
        // 放在两条上传路径之后，所以一帧只有一次 `write_buffer`。
        self.staging.flush(&self.queue);

        InstancePlan {
            groups,
            transparent_start,
            culled,
            cull_unknown,
            scene_bounds,
            caster_bounds,
            caster_nodes,
            cull_box_tests,
            cull_instance_tests,
            cull_pruned,
            cull_branch_pruned,
            morph_instances,
            morph_normalized,
            morph_cull_inflate,
            morph_masked,
            morph_masked_shapes,
        }
    }
}
