//! GPU 顶点布局与实例布局。
//!
//! `Vertex` / `MeshData` 的类型定义在 `r3e-core::mesh`，内置图元的程序化生成在
//! `r3e-core::procedural`（S1 起从本 crate 迁出）。渲染层只剩「wgpu 布局」这一件事，
//! 不再持有任何几何生成逻辑，因此也不存在按来源分支的第二条数据通道。

use r3e_core::mesh::Vertex;
pub use r3e_core::MORPH_WEIGHT_GROUPS;

pub use r3e_core::mesh::{MeshData as CoreMeshData, Vertex as CoreVertex};

/// 实例属性中形变权重的起始 location（扩展三十六取 12，扩展三十九再加一组 → 13）。
pub const MORPH_WEIGHT_BASE_LOCATION: u32 = 12;

/// 形变权重在实例缓冲里的字节偏移：`model(64) + color(16) + 元数据(16)`。
///
/// 扩展三十九把它提成常量，是因为权重从 1 组变成 N 组后，着色器侧的
/// `@location(12 + 组号)` 与这里的偏移必须由同一个数推出来，两处手抄必错。
pub const MORPH_WEIGHT_OFFSET: u32 = 96;

/// 扩展四十：形状掩码的 location（跟在最后一组权重之后）与字节偏移。
///
/// 掩码占 4 字节，`InstanceRaw` 里没有别的 `u32` 能与它共用一个属性，
/// 因此实例步长从 128 涨到 **132**。这 4 字节换的是「整组形状短路」：
/// 一组形状全被掩码关掉时，着色器连 4 次权重比较都不做。
pub const MORPH_MASK_LOCATION: u32 = MORPH_WEIGHT_BASE_LOCATION + MORPH_WEIGHT_GROUPS as u32;
/// 掩码字节偏移：权重区之后（`96 + 16 × 组数`）。
pub const MORPH_MASK_OFFSET: u32 = MORPH_WEIGHT_OFFSET + 16 * MORPH_WEIGHT_GROUPS as u32;

/// 顶点属性：0 = position(f32x3)、1 = normal(f32x3)、2 = uv(f32x2)、
/// 3 = joints(u16x4)、4 = weights(f32x4)（P4 蒙皮）、11 = tangent(f32x4)（扩展二十一）。
///
/// 切线占用 location 11 而不是 5：5..=10 已被 `InstanceRaw` 的实例属性占用，
/// 顶点与实例共用同一套 shader location 命名空间，重号会直接让管线校验失败。
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Uint16x4,
    4 => Float32x4,
    11 => Float32x4
];

/// 顶点缓冲布局（stride = 72 字节：pos12 + normal12 + uv8 + joints8 + weights16 + tangent16）。
pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    }
}

/// 把「扁平权重」切成按组排布的 `vec4`：第 `g` 组 = 形状 `g * 4 .. g * 4 + 4`。
///
/// 场景侧（`MorphInstance`）与剔除侧（`cull_inflate_weights`）都用扁平数组，
/// 因为那边要按下标遍历形状、分成二维反而更绕；只有实例缓冲需要 `vec4` 分组
/// （GPU 顶点属性的格式所限）。转换只在这一个函数里做，两侧口径不会分叉。
pub fn group_morph_weights(
    flat: &[f32; r3e_core::MAX_MORPH_TARGETS],
) -> [[f32; 4]; MORPH_WEIGHT_GROUPS] {
    let mut grouped = [[0.0f32; 4]; MORPH_WEIGHT_GROUPS];
    for (group, chunk) in flat.chunks_exact(4).enumerate().take(MORPH_WEIGHT_GROUPS) {
        grouped[group].copy_from_slice(chunk);
    }
    grouped
}

/// 单个实例的模型矩阵、颜色、蒙皮槽位与形变权重。作为第二个顶点缓冲
/// （step_mode = Instance）上传。
///
/// 总长 = `96 + 16 × MORPH_WEIGHT_GROUPS + 4` 字节：扩展三十六的 1 组权重是 112，
/// 扩展三十九的 2 组是 **128**，扩展四十的形状掩码再加 4 → **132**。
/// 加一组的代价就是这 16 字节——形状数同时多 4 个。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    /// 蒙皮关节矩阵在本帧 storage buffer 中的起始槽位；非蒙皮实例填 0
    /// （此时顶点权重全 0，着色器不会读该槽位，见 `shader.wgsl`）。
    pub joint_base: u32,
    /// 扩展三十六：本实例的形变数据在全局 morph table 中的起始下标（**浮点数**，不是顶点）。
    pub morph_base: u32,
    /// 每个形状占多少个浮点（= 顶点数 × 9：位置 / 法线 / 切线三组增量）。
    pub morph_stride: u32,
    /// 可用形状数，夹到 [`r3e_core::MAX_MORPH_TARGETS`]；0 表示不做形变。
    pub morph_count: u32,
    /// 各形状权重，按 `vec4` 分组：第 `g` 组的第 `i` 个分量 = 形状 `g * 4 + i`。
    ///
    /// 扩展三十九把它从 `[f32; 4]` 改成二维数组，形状上限随之 `4 → 8`。
    pub morph_weights: [[f32; 4]; MORPH_WEIGHT_GROUPS],
    /// 扩展四十：逐实例形状掩码，第 `i` 位 = 形状 `i` 启用。
    ///
    /// 与 `morph_count`（网格有几个形状）不同，这一位是**实例级**的取舍：
    /// 同一个网格的十个实例可以各用一套形状子集。0 = 该实例整段摘掉形变。
    pub morph_mask: u32,
}

impl InstanceRaw {
    /// 属性槽位：5..=8 = model 四行、9 = color、10 = 元数据、12 起每组权重一格，
    /// 最后 14 = 形状掩码（扩展四十）。
    ///
    /// `vertex_attr_array!` 只吃字面量 location，所以组数无法在宏里循环展开 ——
    /// 下面的 `assert!` 把「改组数必须同步改这里」钉住，避免静默少传一组权重。
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 6 + MORPH_WEIGHT_GROUPS + 1] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Uint32x4,
        12 => Float32x4,
        13 => Float32x4,
        14 => Uint32
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// 没有形变的实例（绝大多数）。
    pub fn plain(model: [[f32; 4]; 4], color: [f32; 4], joint_base: u32) -> Self {
        Self {
            model,
            color,
            joint_base,
            morph_base: 0,
            morph_stride: 0,
            morph_count: 0,
            morph_weights: [[0.0; 4]; MORPH_WEIGHT_GROUPS],
            morph_mask: 0,
        }
    }
}

/// 权重组数与属性表必须同步：宏里写死了每个 location，改组数就得改上面那张表。
const _: () = assert!(
    MORPH_WEIGHT_GROUPS == 2,
    "`InstanceRaw::ATTRIBUTES` 里权重属性是手写的，改 MORPH_WEIGHT_GROUPS 必须同步增删"
);

#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::BuiltinMesh;

    /// P4：蒙皮属性必须落在顶点布局里，且 stride 与 `Vertex` 实际大小一致。
    #[test]
    fn vertex_layout_carries_skinning() {
        let layout = vertex_layout();
        assert_eq!(layout.array_stride as usize, std::mem::size_of::<Vertex>());
        assert_eq!(
            layout.array_stride, 72,
            "pos12+normal12+uv8+joints8+weights16+tangent16"
        );

        let locs: Vec<u32> = layout
            .attributes
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(
            locs,
            vec![0, 1, 2, 3, 4, 11],
            "顶点属性位置不得与实例属性冲突"
        );

        assert_eq!(layout.attributes[3].format, wgpu::VertexFormat::Uint16x4);
        assert_eq!(layout.attributes[4].format, wgpu::VertexFormat::Float32x4);
        assert_eq!(layout.attributes[3].offset, 32);
        assert_eq!(layout.attributes[4].offset, 40);
    }

    /// 扩展二十一：切线在顶点布局末尾（offset 56），供法线贴图构造 TBN。
    #[test]
    fn vertex_layout_carries_tangent() {
        let layout = vertex_layout();
        let tangent = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == 11)
            .expect("切线属性缺失");
        assert_eq!(tangent.format, wgpu::VertexFormat::Float32x4);
        assert_eq!(
            tangent.offset, 56,
            "切线紧跟 weights（40 + 16）之后，正好是结构体最后一个字段"
        );
        assert_eq!(
            tangent.offset + 16,
            layout.array_stride,
            "切线必须填满 stride 末尾，不留悬空字节"
        );
    }

    /// 实例布局新增 `joint_base`（关节槽位起点），位置不能与顶点属性重号。
    #[test]
    fn instance_layout_adds_joint_base() {
        let layout = InstanceRaw::layout();
        assert_eq!(
            layout.array_stride as usize,
            std::mem::size_of::<InstanceRaw>()
        );
        assert_eq!(
            layout.array_stride,
            96 + 16 * MORPH_WEIGHT_GROUPS as u64 + 4,
            "model64 + color16 + meta16 + 每组权重 16 × 组数 + 掩码 4"
        );
        assert_eq!(layout.array_stride, 132, "扩展四十的 2 组权重 + 掩码");
        assert_eq!(layout.attributes.len(), 6 + MORPH_WEIGHT_GROUPS + 1);
        assert_eq!(layout.attributes[5].shader_location, 10);
        assert_eq!(layout.attributes[5].format, wgpu::VertexFormat::Uint32x4);
        assert_eq!(layout.attributes[5].offset, 80);
    }

    /// 扩展三十六 / 三十九 / 四十：形变权重跟在实例元数据之后（location 12 起，每组 16 字节），
    /// 掩码独占最后一个属性；也不与顶点布局的 location 撞号。
    #[test]
    fn instance_layout_carries_morph_weights() {
        let layout = InstanceRaw::layout();
        for group in 0..MORPH_WEIGHT_GROUPS {
            let location = MORPH_WEIGHT_BASE_LOCATION + group as u32;
            let weights = layout
                .attributes
                .iter()
                .find(|a| a.shader_location == location)
                .unwrap_or_else(|| panic!("第 {group} 组形变权重属性缺失"));
            assert_eq!(weights.format, wgpu::VertexFormat::Float32x4);
            assert_eq!(
                weights.offset,
                MORPH_WEIGHT_OFFSET as u64 + 16 * group as u64,
                "第 {group} 组紧跟上一组"
            );
        }
        let last = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == MORPH_WEIGHT_BASE_LOCATION + 1)
            .expect("第 2 组权重");
        assert_eq!(
            last.offset + 16,
            MORPH_MASK_OFFSET as u64,
            "掩码必须紧跟在最后一组权重之后，中间不留空隙"
        );

        let vertex_locs: Vec<u32> = VERTEX_ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        for group in 0..MORPH_WEIGHT_GROUPS {
            let location = MORPH_WEIGHT_BASE_LOCATION + group as u32;
            assert!(
                !vertex_locs.contains(&location),
                "实例属性 location {location} 不得被顶点属性占用"
            );
        }
    }

    /// 扩展四十：掩码是独立的 `Uint32` 属性，位置在 stride 末尾、占满最后 4 字节。
    ///
    /// 错位的后果很隐蔽：`std::mem::size_of` 与布局各说各话时，`bytemuck::cast_slice`
    /// 上传的字节数和管线读的步长对不上，表现为「隔一个实例错一次位」——
    /// 屏幕上是一半模型飞走，不是报错。
    #[test]
    fn instance_layout_carries_morph_mask() {
        let layout = InstanceRaw::layout();
        let mask = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == MORPH_MASK_LOCATION)
            .expect("形状掩码属性缺失");
        assert_eq!(mask.format, wgpu::VertexFormat::Uint32);
        assert_eq!(mask.offset, MORPH_MASK_OFFSET as u64);
        assert_eq!(mask.offset, 128);
        assert_eq!(
            mask.offset + 4,
            layout.array_stride,
            "掩码必须填满 stride 末尾，不留悬空字节"
        );
        assert_eq!(
            layout.array_stride as usize,
            std::mem::size_of::<InstanceRaw>(),
            "布局步长与结构体大小必须一致"
        );
        let vertex_locs: Vec<u32> = VERTEX_ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert!(
            !vertex_locs.contains(&MORPH_MASK_LOCATION),
            "掩码 location 不得被顶点属性占用"
        );
    }

    /// `plain` 构造出的是「无蒙皮偏移 + 无形状」的中性实例，字段逐位可断言。
    #[test]
    fn plain_instance_has_no_morph() {
        let raw = InstanceRaw::plain([[0.0; 4]; 4], [1.0; 4], 7);
        assert_eq!(raw.joint_base, 7);
        assert_eq!(
            (raw.morph_base, raw.morph_stride, raw.morph_count),
            (0, 0, 0)
        );
        assert_eq!(raw.morph_weights, [[0.0; 4]; MORPH_WEIGHT_GROUPS]);
        assert_eq!(raw.morph_mask, 0, "plain 实例的掩码必须是 0（无形变）");
    }

    /// 扩展三十九：扁平权重按序切成组 —— 形状 `g*4+i` 必须落在第 `g` 组第 `i` 分量，
    /// 一旦切错组，GPU 侧「哪个分量配哪个形状」的映射就整体位移。
    #[test]
    fn group_morph_weights_slices_in_order() {
        let mut flat = [0.0f32; r3e_core::MAX_MORPH_TARGETS];
        for (i, w) in flat.iter_mut().enumerate() {
            *w = i as f32 + 1.0;
        }
        let grouped = group_morph_weights(&flat);
        for (group, lanes) in grouped.iter().enumerate() {
            for (lane, got) in lanes.iter().enumerate() {
                assert_eq!(
                    *got,
                    (group * 4 + lane) as f32 + 1.0,
                    "第 {group} 组第 {lane} 分量"
                );
            }
        }
        // 全 0 输入必须原样全 0：没有形状的实例不该因为转换产生非零权重
        assert_eq!(
            group_morph_weights(&[0.0; r3e_core::MAX_MORPH_TARGETS]),
            [[0.0; 4]; MORPH_WEIGHT_GROUPS]
        );
    }

    /// 内置网格已迁到 core，但「不蒙皮」这一约定仍要在此守住：
    /// 权重全 0 时着色器才会跳过 storage 读取。
    #[test]
    fn builtin_mesh_is_not_skinned() {
        for kind in BuiltinMesh::ALL {
            let mesh = r3e_core::procedural::builtin_mesh(kind);
            assert!(mesh.vertices.iter().all(|v| !v.is_skinned()), "{kind:?}");
        }
    }
}
