use r3e_core::{Aabb, MeshRef, Sh9};

use crate::envprobe::EnvProbeStats;
use crate::mesh::{self};
use crate::pbr::SurfaceKey;

/// 已上传到 GPU 的反射探针（扩展二十三）。纹理本体必须由渲染器持有：
/// 材质缓存只存 `TextureView`，若纹理被提前释放，视图就会悬空。
pub(crate) struct EnvProbeGpu {
    /// 立方体纹理本体（`Rgba16Float`，`mip_level_count` 层）。
    pub(crate) _texture: wgpu::Texture,
    /// 本帧进 `RenderStats` 的统计。
    pub(crate) stats: EnvProbeStats,
    /// 漫反射辐照度：由基准层（最清晰那层）投影出的 9 项球谐。
    /// 上传到 `Camera::env_sh`，于是着色器的漫反射分支一行都不用改。
    pub(crate) irradiance: Sh9,
}

/// 一个绘制批次：网格 + 实例区间 + 物件级 PBR 覆盖 + 场景共享材质（S4-A）。
///
/// 不透明批次与半透明批次共用同一份实例缓冲，区别只有「落在 `InstancePlan`
/// 的哪一段」以及绘制时用哪条管线（见 [`InstancePlan`] 的透明段说明）。
///
/// 第四位是 M1-S4-A 的场景材质：`Some` 时它**整份取代**网格资产材质与物件级覆盖
/// （完整材质语义），`None` 时走原来的「网格分区材质 + 物件覆盖」链。之所以放在
/// 批次上而不是只放在材质键里：场景材质是物件级属性，同一节点的所有网格分区都必须
/// 用同一份，放进分区键会被网格资产材质盖掉。
pub(crate) type DrawGroup = (
    MeshRef,
    std::ops::Range<u32>,
    Option<SurfaceKey>,
    Option<crate::pbr::SceneMaterialKey>,
);

/// `prepare_instances` 的返回值。
///
/// 分组键是 `(网格, 物件级 PBR 材质)`：同一网格的物件若 metallic / roughness
/// 不同，会被拆成多个批次，各自绑定自己的材质 uniform。
///
/// 半透明批次（扩展十九）**一律排在分组末尾**：
/// `groups[..transparent_start]` 走不透明管线（写深度），
/// `groups[transparent_start..]` 走半透明管线（不写深度、由远及近）。
/// `transparent_start == groups.len()` 表示本帧没有半透明物件。
pub(crate) struct InstancePlan {
    pub(crate) groups: Vec<DrawGroup>,
    pub(crate) transparent_start: usize,
    pub(crate) culled: u32,
    pub(crate) cull_unknown: u32,
    pub(crate) scene_bounds: Option<Aabb>,
    /// 与实例缓冲同序的遮挡体包围盒（扩展二十四）。
    ///
    /// 阴影通道按级裁剪时要用它；拿不到网格 AABB 的实例填
    /// [`r3e_core::fail_open_bounds`]，语义是「一律不裁」，宁可多画也不少画。
    pub(crate) caster_bounds: Vec<Aabb>,
    /// 与实例缓冲同序的节点 id（扩展二十五）。层级剔除用它把实例挂回场景树：
    /// 按实例顺序展开成树节点掩码时，下标一一对应。
    pub(crate) caster_nodes: Vec<r3e_core::NodeId>,
    /// 扩展二十九：主通道视锥树本帧的层级判定读数（进 `RenderStats::cull`）。
    pub(crate) cull_box_tests: u32,
    pub(crate) cull_instance_tests: u32,
    pub(crate) cull_pruned: u32,
    pub(crate) cull_branch_pruned: u32,
    /// 扩展三十六：本帧真正被形变的实例数（网格带形状 + 权重非零）。
    /// 进 `RenderStats::morph_instances`；0 表示顶点路径与扩展三十五逐位相同。
    pub(crate) morph_instances: u32,
    /// 扩展三十八：其中互斥组超预算、权重被归一化过的实例数
    /// （进 `RenderStats::morph_normalized`）。
    pub(crate) morph_normalized: u32,
    /// 扩展三十八：本帧逐形状剔除余量合计（进 `RenderStats::morph_cull_inflate`）。
    pub(crate) morph_cull_inflate: f32,
    /// 扩展四十：被形状掩码整段摘掉形变的实例数（进 `RenderStats::morph_masked`）。
    /// 这些实例的 `morph_count` 已写 0，顶点路径与无形变网格逐位相同。
    pub(crate) morph_masked: u32,
    /// 扩展四十：被掩码挡在门外的**非零权重槽**合计
    /// （进 `RenderStats::morph_masked_shapes`）—— 本帧省下的形变乘加组数。
    pub(crate) morph_masked_shapes: u32,
}

/// 一次阴影提交的实例区间：`(groups 下标, 实例缓冲区间)`。
///
/// 扩展二十四之前这里是「每个分组一段全量」，现在可能被级联掩码切成若干段。
pub(crate) type ShadowRun = (usize, std::ops::Range<u32>);

/// 本帧方向光阴影的执行计划（扩展二十四）。
pub(crate) struct ShadowPlan {
    /// 生效级数，0 = 本帧阴影关闭。
    pub(crate) levels: u32,
    /// 每级一份「保留」掩码，下标与实例缓冲一致；按级裁剪关闭时为空。
    pub(crate) masks: Vec<Vec<bool>>,
    /// 判过的 (级, 实例) 对数，进 `RenderStats::shadow_casters`。
    pub(crate) tested: u32,
    /// 被裁掉的 (级, 实例) 对数，进 `RenderStats::shadow_culled`。
    pub(crate) culled: u32,
    /// 扩展二十五：没做实例判定就定成不可见的实例数（整枝跳过 + 叶子直接被裁），
    /// 进 `RenderStats::shadow_pruned`。0 = 树为空 / 层级剔除关闭。
    pub(crate) pruned: u32,
    /// 扩展二十五：其中来自多实例子树的部分，进 `RenderStats::shadow_branch_pruned`。
    /// 这才是层级剔除相对「逐实例判定」的净收益。
    pub(crate) branch_pruned: u32,
    /// 扩展二十五：真正做的层级盒判定数，进 `RenderStats::shadow_box_tests`。
    pub(crate) box_tests: u32,
}

/// 扩展四十二：一个实例上传给 GPU 的形变槽数据（第一遍算好、第二遍直接搬）。
///
/// 与扩展四十的 `u32` 位图不只是「换了表示」：这里是**压缩过的**槽列表 ——
/// 被掩码挡掉的形状不占槽、不占带宽，循环次数是「真用到的形状数」而不是
/// 「网格形状总数」。位图方案里那 4 个字节是死的，还得逐个形状比较一次。
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct MorphUpload {
    /// 槽序权重：第 `k` 个 = 第 `k` 槽的形状权重（已互斥归一化）。
    pub(crate) weights: [f32; r3e_core::MAX_MORPH_TARGETS],
    /// 槽序形状序号，每字 4 个 `u8`（小端），有效长度 = `count`。
    pub(crate) slots: [u32; mesh::MORPH_SLOT_ATTR_WORDS],
    /// 槽数 = 上传的 `morph_count`。0 = 该实例整段摘掉形变。
    pub(crate) count: u32,
}

impl MorphUpload {
    /// 没有形变的实例（无绑定 / 网格无形状 / 掩码清空）。
    pub(crate) const fn zero() -> Self {
        Self {
            weights: [0.0; r3e_core::MAX_MORPH_TARGETS],
            slots: [0; mesh::MORPH_SLOT_ATTR_WORDS],
            count: 0,
        }
    }
}

/// 扩展四十二：把实例的形状槽列表压成上传三元组（槽序权重 / 序号字 / 槽数）。
///
/// 两处裁剪在这里一次做完，且**顺序不能换**：
///
/// 1. 按实例 `target_count`：`active_shapes()` 已经截过，`slot_weights()` 与它同源；
/// 2. 按网格 `mesh_count`：脚本可以给实例启用网格并不存在的形状序号，那些槽在
///    着色器里算出来的表偏移会落到形变表之外 —— 必须丢掉并**左移压缩**，
///    否则后面的槽位跟着错位，屏幕上是「形变张冠李戴」而不是越界报错。
///
/// 权重取自 `slot_weights()`（互斥归一化后的那一份），所以「剔除算的余量」与
/// 「着色器吃到的位移」仍然是同一份数据。
pub(crate) fn morph_upload(instance: &r3e_core::MorphInstance, mesh_count: usize) -> MorphUpload {
    let (slot_weights, _) = instance.slot_weights();
    let mut shapes = [0u8; r3e_core::MORPH_SLOT_CAPACITY];
    let mut weights = [0.0f32; r3e_core::MAX_MORPH_TARGETS];
    let mut count = 0usize;
    for (slot, &shape) in instance.active_shapes().iter().enumerate() {
        if shape as usize >= mesh_count {
            continue;
        }
        weights[count] = slot_weights[slot];
        shapes[count] = shape;
        count += 1;
    }
    MorphUpload {
        weights,
        slots: mesh::pack_morph_slots(&shapes[..count]),
        count: count as u32,
    }
}

/// `mask = None` 表示不裁剪（局部光源阴影、或按级裁剪被关掉）：每个分组一段全量，
/// 行为与扩展二十三逐字相同。`Some` 时按组切出保留的连续段。
pub(crate) fn shadow_runs(groups: &[DrawGroup], mask: Option<&[bool]>) -> Vec<ShadowRun> {
    match mask {
        None => groups
            .iter()
            .enumerate()
            .map(|(index, (_, range, ..))| (index, range.clone()))
            .collect(),
        Some(mask) => {
            let mut runs = Vec::new();
            for (index, (_, range, ..)) in groups.iter().enumerate() {
                let start = range.start as usize;
                let end = (range.end as usize).min(mask.len());
                if start >= end {
                    continue;
                }
                for kept in r3e_core::split_runs(range.start, &mask[start..end]) {
                    runs.push((index, kept));
                }
            }
            runs
        }
    }
}

/// 一次阴影深度 pass 要提交的内容（扩展二十四）。
///
/// 参数已经到 7 个，再逐个传会过 clippy 的 `too_many_arguments`；而且这三样
/// 在语义上是一体的：**画什么**（分组）、**画其中哪几段**（区间）、**实例间步子多大**。
pub(crate) struct ShadowSubmit<'a> {
    pub(crate) groups: &'a [DrawGroup],
    pub(crate) runs: &'a [ShadowRun],
    pub(crate) stride: u64,
}
