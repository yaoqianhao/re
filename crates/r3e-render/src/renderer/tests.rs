use std::sync::Arc;

use r3e_core::mesh::MeshData;

use crate::mesh::{self};

use super::*;

#[cfg(test)]
mod morph_table_tests {
    use super::*;
    use r3e_core::morph::{MorphSet, MorphTarget, MORPH_VERTEX_FLOATS};

    /// 造一个 `vertices` 个顶点的网格数据，挂 `targets` 个形状（形状增量全为 +1）。
    pub(crate) fn mesh_with(vertices: usize, targets: usize) -> MeshData {
        let verts = vec![r3e_core::Vertex::default(); vertices];
        let indices: Vec<u32> = (0..vertices as u32).collect();
        let shapes: Vec<MorphTarget> = (0..targets)
            .map(|t| MorphTarget::new(format!("shape{t}"), vec![[1.0, 0.0, 0.0]; vertices]))
            .collect();
        MeshData::new(verts, indices).with_morph(MorphSet::new(shapes))
    }

    /// 两个网格顺排：`base` 指向各自第一段，`stride` = 顶点数 × 每顶点浮点数。
    #[test]
    pub(crate) fn bases_are_laid_out_back_to_back() {
        let per_vertex = MORPH_VERTEX_FLOATS as u32;
        let mut cache = MeshCache::new();
        let a = cache
            .register_morph(&mesh_with(4, 1))
            .expect("四边形应可登记");
        assert_eq!(
            (a.base, a.stride, a.count),
            (0, 4 * per_vertex, 1),
            "4 顶点 × {per_vertex} 分量"
        );

        let stride_b = 3 * per_vertex;
        let b = cache
            .register_morph(&mesh_with(3, 2))
            .expect("三顶点应可登记");
        assert_eq!(
            (b.base, b.stride, b.count),
            (4 * per_vertex, stride_b, 2),
            "接在 A 之后"
        );
        assert_eq!(
            cache.morph_table().len() as u32,
            4 * per_vertex + 2 * stride_b,
            "两个形状各占 3 顶点 × {per_vertex} 个 float"
        );
        assert_eq!(
            cache.morph_table()[b.base as usize],
            1.0,
            "B 的第一段是 B 第一个形状"
        );
        assert_eq!(cache.morph_version(), 2, "每次登记都让版本前进");
    }

    /// 增量长度与顶点数对不上时按无形变处理，表与版本都不许变。
    #[test]
    pub(crate) fn mismatched_length_is_rejected_without_touching_table() {
        let mut cache = MeshCache::new();
        let ok = mesh_with(4, 1);
        cache.register_morph(&ok);

        let mut bad = mesh_with(4, 1);
        bad.morph = MorphSet::new(vec![MorphTarget::new("bad", vec![[1.0, 0.0, 0.0]; 3])]);
        assert!(
            cache.register_morph(&bad).is_none(),
            "长度不等必须被拒，否则 GPU 上会越界读"
        );
        assert_eq!(
            cache.morph_table().len(),
            4 * MORPH_VERTEX_FLOATS,
            "被拒的网格不进表"
        );
        assert_eq!(cache.morph_version(), 1, "被拒的网格不动版本号");
    }

    /// 形状数超上限只取前 `MAX_MORPH_TARGETS` 个：每实例权重是 2 组 vec4（扩展三十九）。
    #[test]
    pub(crate) fn target_count_is_capped_at_max() {
        let mut cache = MeshCache::new();
        let morph = cache
            .register_morph(&mesh_with(2, r3e_core::MAX_MORPH_TARGETS + 3))
            .expect("超量形状应被截断而不是整份拒绝");
        assert_eq!(morph.count as usize, r3e_core::MAX_MORPH_TARGETS);
        assert_eq!(
            cache.morph_table().len(),
            2 * MORPH_VERTEX_FLOATS * r3e_core::MAX_MORPH_TARGETS
        );
    }

    /// 表容量满了之后，新网格降级为「无形变」，已经在表里的网格不受影响。
    #[test]
    pub(crate) fn table_capacity_falls_back_to_no_morph() {
        let mut small = MeshCache::new();
        // 差 2 顶点 × 每顶点浮点数 正好塞满最后一个双顶点网格
        small.morph_table = vec![0.0; MAX_MORPH_TABLE_FLOATS - 2 * MORPH_VERTEX_FLOATS];
        assert!(small.register_morph(&mesh_with(2, 1)).is_some());
        assert_eq!(small.morph_table().len(), MAX_MORPH_TABLE_FLOATS);
        assert!(
            small.register_morph(&mesh_with(2, 1)).is_none(),
            "再塞就超容量，必须按无形变处理"
        );
        assert_eq!(
            small.morph_table().len(),
            MAX_MORPH_TABLE_FLOATS,
            "不许扩容"
        );
    }

    /// 换资产库时整表作废：旧 `base` 指向的增量属于已丢弃的资产。
    #[test]
    pub(crate) fn swapping_library_clears_table_and_bumps_version() {
        let mut cache = MeshCache::new();
        cache.register_morph(&mesh_with(4, 1));
        let before = cache.morph_version();

        cache.set_library(Arc::new(r3e_core::ProceduralProvider::new()));
        assert!(cache.morph_table().is_empty(), "旧增量必须清空");
        assert_ne!(cache.morph_version(), before, "版本要变，让渲染层重传");
    }

    /// 剔除余量取「位置增量里最长的那个」，法线增量不参与 —— 它不影响几何边界。
    #[test]
    pub(crate) fn max_displacement_is_the_longest_position_delta() {
        let mut cache = MeshCache::new();
        let verts = vec![r3e_core::Vertex::default(); 3];
        let mut shape = MorphTarget::new(
            "jaw",
            vec![[3.0, 4.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.0, 0.0]],
        );
        // 法线增量故意写得比位置长：若被算进去，余量会虚高到 10
        shape = shape.with_normals(vec![[0.0, 0.0, 10.0]; 3]);
        let data = MeshData::new(verts, vec![0, 1, 2]).with_morph(MorphSet::new(vec![shape]));

        let morph = cache.register_morph(&data).expect("合法形状应登记");
        assert!(
            (morph.max_displacement - 5.0).abs() < 1e-5,
            "应为 (3,4,0) 的模长 5，实际 {}",
            morph.max_displacement
        );
    }

    /// 余量随权重峰值与缩放线性放大，权重 0 时严格为 0（不播放表情就不吃额外代价）。
    #[test]
    pub(crate) fn cull_inflate_follows_weight_peak_and_scale() {
        let morph = MeshMorph {
            base: 0,
            stride: 18,
            count: 1,
            max_displacement: 2.0,
            max_displacements: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        assert_eq!(morph.cull_inflate(0.0, 1.0), 0.0, "权重 0 不外扩");
        assert_eq!(morph.cull_inflate(1.0, 1.0), 2.0);
        // 权重不做 clamp：夸张表情写 2.0、反向表情写负值都要按绝对值算
        assert_eq!(morph.cull_inflate(2.0, 1.0), 4.0);
        assert_eq!(morph.cull_inflate(-2.0, 1.0), 4.0);
        assert_eq!(
            morph.cull_inflate(1.0, 3.0),
            6.0,
            "实例放大 3 倍，位移也放大"
        );
        assert_eq!(morph.cull_inflate(1.0, 0.0), 0.0, "零缩放实例不外扩");
    }

    /// 扩展三十八：登记期要同时算出**逐形状**峰值位移，网格级粗界取其最大。
    #[test]
    pub(crate) fn register_morph_keeps_per_target_displacements() {
        let mut cache = MeshCache::new();
        let verts = vec![r3e_core::Vertex::default(); 3];
        // 形状 0 是「眨眼」：位移 0.02；形状 1 是「张嘴」：位移 0.15
        let blink = MorphTarget::new("blink", vec![[0.02, 0.0, 0.0]; 3]);
        let jaw = MorphTarget::new("jaw", vec![[0.0, 0.15, 0.0]; 3]);
        let data = MeshData::new(verts, vec![0, 1, 2]).with_morph(MorphSet::new(vec![blink, jaw]));

        let morph = cache.register_morph(&data).expect("合法形状应登记");
        assert_eq!(morph.count, 2);
        assert!((morph.max_displacements[0] - 0.02).abs() < 1e-6);
        assert!((morph.max_displacements[1] - 0.15).abs() < 1e-6);
        assert_eq!(morph.max_displacements[2], 0.0, "未用槽位必须是 0");
        assert!(
            (morph.max_displacement - 0.15).abs() < 1e-6,
            "粗界 = 逐形状表最大值"
        );
    }

    /// 扩展三十八：逐形状余量只算「真的在动的形状」。
    ///
    /// 只眨眼时余量必须等于眨眼那么点，而不是被张嘴的 0.15 连坐 —— 这正是
    /// 扩展三十六「最长位移 × 峰值权重」的粗界浪费掉的那部分包围盒。
    #[test]
    pub(crate) fn cull_inflate_weights_only_counts_active_targets() {
        let morph = MeshMorph {
            base: 0,
            stride: 36,
            count: 2,
            max_displacement: 0.15,
            max_displacements: [0.02, 0.15, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        // 只眨眼：余量 0.02，粗界口径会算成 0.15（大 7.5 倍）
        let only_blink = morph.cull_inflate_weights(
            &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            1.0,
        );
        assert!((only_blink - 0.02).abs() < 1e-6, "实际 {only_blink}");
        assert!(
            (morph.cull_inflate(1.0, 1.0) - 0.15).abs() < 1e-6,
            "粗界仍是 0.15"
        );
        // 两个一起动 = 各自贡献之和（互斥归一化后通常落在 1.0 预算内）
        let both = morph.cull_inflate_weights(
            &[0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            1.0,
        );
        assert!((both - 0.085).abs() < 1e-6, "实际 {both}");
        // 负权重按绝对值计：反向形变同样把顶点推出静止盒
        let negative = morph.cull_inflate_weights(
            &[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            1.0,
        );
        assert!((negative - 0.02).abs() < 1e-6);
        // 缩放线性放大、零权重严格为 0
        assert!(
            (morph.cull_inflate_weights(
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                3.0
            ) - 0.06)
                .abs()
                < 1e-6
        );
        assert_eq!(
            morph.cull_inflate_weights(&[0.0; r3e_core::MAX_MORPH_TARGETS], 1.0),
            0.0
        );
        assert_eq!(
            morph.cull_inflate_weights(
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                0.0
            ),
            0.0
        );
        // NaN 权重 / NaN 缩放按 0 处理，绝不让 NaN 泄进包围盒
        assert_eq!(
            morph.cull_inflate_weights(
                &[
                    f32::NAN,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0
                ],
                1.0
            ),
            0.0
        );
        assert_eq!(
            morph.cull_inflate_weights(
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                f32::NAN
            ),
            0.0
        );
        // 溢出时 fail-open 到无穷：宁可多画，不能把本该可见的实例剔除掉
        assert_eq!(
            morph.cull_inflate_weights(
                &[
                    f32::MAX,
                    f32::MAX,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0
                ],
                f32::MAX
            ),
            f32::MAX
        );
    }

    /// 扩展三十八：`count` 之外的槽位即使写了权重也不参与余量计算。
    #[test]
    pub(crate) fn cull_inflate_weights_ignores_slots_beyond_count() {
        let morph = MeshMorph {
            base: 0,
            stride: 18,
            count: 1,
            max_displacement: 0.5,
            max_displacements: [0.5, 9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let inflate = morph.cull_inflate_weights(
            &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            1.0,
        );
        assert_eq!(inflate, 0.0, "第二个槽位不在 count 内，不该产生余量");
    }

    /// 扩展三十九：剔除余量必须跟着「第 2 组」的形状走 —— 形状 4..8 只靠
    /// `max_displacements[4..]` 与权重后 4 位说话；这一条断了，播第 5 个表情
    /// 的实例会在屏幕上「笑着消失」（盒没撑大，被视锥当盒子外的物件剪掉）。
    #[test]
    pub(crate) fn cull_inflate_covers_second_weight_group() {
        let morph = MeshMorph {
            base: 0,
            stride: 18,
            count: 8,
            max_displacement: 0.4,
            max_displacements: [0.0, 0.0, 0.0, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        // 只有形状 4（第 2 组第 1 分量）有位移，其余全 0
        let inflate = morph.cull_inflate_weights(
            &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            2.0,
        );
        assert!(
            (inflate - 0.8).abs() < 1e-6,
            "形状 4 全权重 × 缩放 2 应得 0.8，实际 {inflate}"
        );
        // 第 1 组全 0 时余量必须恰好为 0：扩组不能让「没播放的实例」白撑盒子
        assert_eq!(
            morph.cull_inflate_weights(&[0.0; r3e_core::MAX_MORPH_TARGETS], 2.0),
            0.0
        );
        // 最后一个形状（下标 7）同理，防「循环少跑一格」
        let last = morph.cull_inflate_weights(
            &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            1.0,
        );
        assert_eq!(last, 0.0, "形状 7 没有登记位移，余量为 0");
    }
}

/// 扩展四十二（形状槽列表）的渲染侧合约：不开窗口、不建 GPU，只验上传口径。
#[cfg(test)]
mod morph_upload_tests {
    use super::*;

    /// 契约一：槽数 = 真用到的形状数，权重与序号按同一个槽下标对齐。
    ///
    /// 位图方案里 `morph_count` 是「网格共有几个形状」，着色器只好按形状总数静态
    /// 展开、逐个比较位。现在 `morph_count` 是**槽数**：用不到的形状不占槽、不占
    /// 带宽、不进循环。这条测试钉的就是这个换底 —— 一旦 `count` 又被写成网格形状数，
    /// 着色器就会按槽数去读超出列表的序号，形变表偏移直接越界。
    #[test]
    pub(crate) fn slot_list_carries_only_the_shapes_in_use() {
        let mut instance = r3e_core::MorphInstance::new(0, 4);
        instance.set_weight(1, 0.5);
        instance.set_weight(3, 0.25);
        instance.set_shape_mask(0b1010);

        let upload = morph_upload(&instance, 4);
        assert_eq!(upload.count, 2, "网格 4 个形状、实例只用 2 个 → 槽数 2");
        assert_eq!(&upload.weights[..2], &[0.5, 0.25], "槽 0/1 配形状 1/3");
        assert_eq!(upload.slots[0], 1 | (3 << 8), "序号按字节小端打包");
        assert_eq!(
            &upload.slots[1..],
            &[0, 0, 0],
            "未用槽（含 vec4 的第 4 个占位字）必须留 0"
        );
        assert_eq!(upload.weights[2], 0.0, "未用槽的权重不得残留");
    }

    /// 契约二：网格没有的形状序号必须丢掉并**左移压缩**。
    ///
    /// 脚本能给实例启用网格并不存在的形状（`morph_mask_set(node, 0xFFFF)` 而网格
    /// 只有 3 个形状是常见写法）。这些槽在着色器里算出的表偏移会落到形变表之外，
    /// 而且会让后面的槽位跟着错位 —— 表现是「形变张冠李戴」，不是越界报错。
    #[test]
    pub(crate) fn slots_beyond_mesh_shape_count_are_dropped_and_compacted() {
        let mut instance = r3e_core::MorphInstance::new(0, 8);
        instance.set_weight(1, 0.5);
        instance.set_weight(5, 0.75);
        // 只启用形状 1 / 5：掩码全开时形状 0..3 都会进槽，测不出「压缩」这件事
        instance.set_shape_mask(0b100010);

        // 网格只有 3 个形状 → 形状 5 没有表可读，只留形状 1
        let upload = morph_upload(&instance, 3);
        assert_eq!(upload.count, 1);
        assert_eq!(upload.weights[0], 0.5, "留下的槽不能被前面丢掉的槽顶掉");
        assert_eq!(upload.slots[0], 1, "序号 1 压缩到槽 0");

        // 网格一个形状都没有 → 一个槽都不留（着色器整段短路）
        assert_eq!(morph_upload(&instance, 0).count, 0);
    }

    /// 契约三：实例启用的形状全在网格之外时，槽数归 0 而不是留下脏槽。
    ///
    /// 这条路径就是第一遍里 `morph_masked` 的判据来源 —— 槽数 0 才能把
    /// `morph_count` 写成 0，让着色器在 `imeta.w == 0` 处提前返回。
    #[test]
    pub(crate) fn empty_slot_list_drops_the_morph_path() {
        let mut instance = r3e_core::MorphInstance::new(0, 8);
        instance.set_weight(6, 1.0);
        // 只启用形状 6 —— 网格只有 2 个形状，这个槽一个都留不下
        instance.set_shape_mask(1 << 6);
        let upload = morph_upload(&instance, 2);
        assert_eq!(upload.count, 0, "形状 6 超出网格 2 个形状的范围");
        assert_eq!(upload.slots, [0; mesh::MORPH_SLOT_ATTR_WORDS]);
        assert!(upload.weights.iter().all(|w| *w == 0.0));

        // 掩码全关（位图 0）也归 0，且与「形状在网格外」走同一出口
        instance.set_shape_mask(0);
        assert_eq!(morph_upload(&instance, 8).count, 0);
    }

    /// 契约四：实例 `target_count` 的截断在上传侧同样生效。
    ///
    /// `target_count` 是公开字段，脚本能改小；改小之后槽里可能残留更大的序号，
    /// 打包时必须按 `active_shapes()` 走，不能让旧槽漏进上传数据。
    #[test]
    pub(crate) fn upload_follows_instance_target_count() {
        let mut instance = r3e_core::MorphInstance::new(0, r3e_core::MAX_MORPH_TARGETS);
        instance.set_weight(1, 0.5);
        instance.set_weight(9, 0.25);
        let upload = morph_upload(&instance, r3e_core::MAX_MORPH_TARGETS);
        assert_eq!(upload.count, r3e_core::MAX_MORPH_TARGETS as u32);

        instance.target_count = 2;
        let upload = morph_upload(&instance, r3e_core::MAX_MORPH_TARGETS);
        assert_eq!(upload.count, 2, "只剩形状 0 / 1");
        assert_eq!(upload.weights[1], 0.5);

        // 互斥归一化后的值才是上传值：槽里 0.5 + 0.75 超预算 → 等比压回 1.0
        let mut normalized = r3e_core::MorphInstance::new(0, 4);
        normalized.set_weight(0, 0.5);
        normalized.set_weight(1, 0.75);
        assert!(normalized.set_exclusive_group(0, Some(1)));
        assert!(normalized.set_exclusive_group(1, Some(1)));
        let upload = morph_upload(&normalized, 4);
        assert!((upload.weights[0] - 0.5 / 1.25).abs() < 1e-6, "{upload:?}");
        assert!((upload.weights[1] - 0.75 / 1.25).abs() < 1e-6, "{upload:?}");
        assert_eq!(normalized.weight_of(0), Some(0.5), "场景原值不动");
    }
}

/// 真窗口回归：一个批次里有多少实例，`draw_indexed` 就必须画多少个。
///
/// 这条不变量曾被打成恒 `0..1`：统计口径（`RenderStats` 的实例数按区间长度累加）
/// 完全看不出问题，但 GPU 每批只画批首一个实例 —— 运行时 `scene.spawn` 的新节点
/// 永远排在批尾，于是「新加的物件不显示、角色可视体看不见」，而既有节点改变换
/// 仍然生效（它是批首），很容易被误判成物理或场景装配问题。
#[cfg(test)]
mod draw_instance_count_tests {
    use super::*;

    /// 批内 8 个实例、从第 3 个开始（半透明段常见）：长度仍是 5。
    #[test]
    pub(crate) fn whole_batch_is_drawn() {
        assert_eq!(instance_count(&(3..8)), 5, "3..8 是 5 个实例");
    }

    /// 单实例批次仍是 1（被误写成 0 会整体不画）。
    #[test]
    pub(crate) fn single_instance_batch_is_one() {
        assert_eq!(instance_count(&(7..8)), 1);
    }

    /// 空批次（被剔除干净的桶）画 0 个，不 panic。
    #[test]
    pub(crate) fn empty_batch_draws_nothing() {
        assert_eq!(instance_count(&(5..5)), 0);
    }

    /// 反向区间按 0 处理：`0..0` 是合法空绘制，比在 `slice()` 处 panic 更好定位。
    ///
    /// 区间用变量拼：`reversed_empty_ranges` 对字面量 `8..3` 直接 deny，而这里
    /// 要验的恰恰是「上游一旦算出反区间，绘制侧不许 panic」的防御分支。
    #[test]
    pub(crate) fn reversed_range_is_clamped_to_zero() {
        let start = 8u32;
        let end = 3u32;
        assert_eq!(instance_count(&(start..end)), 0);
    }

    /// 批首偏移与批长解耦：批次从第 100 个实例起、长度 7 时，`draw_indexed`
    /// 拿到的仍是 `0..7`（实例缓冲已按区间切片，绘制下标从 0 起）。
    #[test]
    pub(crate) fn batch_offset_does_not_leak_into_draw_range() {
        let count = instance_count(&(100..107));
        assert_eq!(count, 7);
        assert_eq!(0..count, 0..7, "绘制范围的起点恒为 0");
    }
}
