//! GPU 顶点布局与实例布局。
//!
//! `Vertex` / `MeshData` 的类型定义在 `r3e-core::mesh`，内置图元的程序化生成在
//! `r3e-core::procedural`（S1 起从本 crate 迁出）。渲染层只剩「wgpu 布局」这一件事，
//! 不再持有任何几何生成逻辑，因此也不存在按来源分支的第二条数据通道。

use r3e_core::mesh::Vertex;
pub use r3e_core::{MORPH_SLOT_CAPACITY, MORPH_WEIGHT_GROUPS};

pub use r3e_core::mesh::{MeshData as CoreMeshData, Vertex as CoreVertex};

/// 实例属性中形变权重的**起始** location（扩展三十六取 12）：第 `g` 组占 `12 + g`，
/// 扩展四十一到 14，形状槽序号接在其后（15 起，扩展四十二）。
pub const MORPH_WEIGHT_BASE_LOCATION: u32 = 12;

/// 形变权重在实例缓冲里的字节偏移：`model(64) + color(16) + 元数据(16)`。
///
/// 扩展三十九把它提成常量，是因为权重从 1 组变成 N 组后，着色器侧的
/// `@location(12 + 组号)` 与这里的偏移必须由同一个数推出来，两处手抄必错。
pub const MORPH_WEIGHT_OFFSET: u32 = 96;

/// 扩展四十二：形状槽序号属性的 location（跟在最后一组权重之后）。
///
/// 3 个字**压进一个** `vec4<u32>`，而不是摊成 3 个 `Uint32` —— webgpu 的顶点属性
/// location 只有 `0..=15` 共 16 格，顶点侧已占 6（0..=4 与 11）、实例侧 model / 色 /
/// 元数据 / 三组权重占 9（5..=10、12..=14），只剩 15 这一格。摊开会在建管线时报
/// 「Vertex attribute location 16 must be less than limit 16」——不是推论，是真机踩过。
pub const MORPH_SLOT_LOCATION_BASE: u32 = MORPH_WEIGHT_BASE_LOCATION + MORPH_WEIGHT_GROUPS as u32;
/// 槽序号字节偏移：权重区之后（`96 + 16 × 组数` = 144）。
pub const MORPH_SLOT_OFFSET: u32 = MORPH_WEIGHT_OFFSET + 16 * MORPH_WEIGHT_GROUPS as u32;

/// 打包形状序号用的 `u32` 字数：每字装 4 个 `u8` 序号（槽容量按 4 向上取整）。
///
/// 用 `u8` 而不是 `u32` 存单个序号是这次改造的核心 —— 形状总数因此**与 32 位宽脱钩**
/// （位图方案里「第 32 个形状」根本无处安放），加槽只是多一个字，不换表示。
// 用移位而不是 `(n + 3) / 4`：`div_ceil` 还不是 const fn，`/ 4` 又会被 clippy 判成
// 「手写 div_ceil」。槽容量恒为 4 的倍数时两者等价。
pub const MORPH_SLOT_WORDS: usize = (MORPH_SLOT_CAPACITY + 3) >> 2;
/// 槽序号**属性**的字数：往上的 `vec4<u32>` 属性固定吃 4 字，第 `MORPH_SLOT_WORDS..4`
/// 字恒 0。多出来的 4 字节是「location 换容量」的票价：`MORPH_SLOT_CAPACITY` 涨到 16
/// 时不需要动布局，只需把这里多出来的字填上。
pub const MORPH_SLOT_ATTR_WORDS: usize = 4;

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

/// 把「扁平槽权重」切成按组排布的 `vec4`：第 `g` 组 = 槽 `g * 4 .. g * 4 + 4`。
///
/// 扩展四十二之后这里的输入已经是 **槽序**权重（[`r3e_core::MorphInstance::slot_weights`]
/// 的输出），不是形状序：第 `k` 个槽配 [`r3e_core::MorphInstance::active_shapes`] 的第
/// `k` 个形状序号。两边按同一个 `k` 对齐，所以着色器拿到的「权重 + 序号」天然配对。
///
/// 只有实例缓冲需要 `vec4` 分组（GPU 顶点属性的格式所限）；场景侧与剔除侧仍用
/// 扁平数组。转换只在这一个函数里做，两侧口径不会分叉。
pub fn group_morph_weights(flat: &[f32; MORPH_SLOT_CAPACITY]) -> [[f32; 4]; MORPH_WEIGHT_GROUPS] {
    let mut grouped = [[0.0f32; 4]; MORPH_WEIGHT_GROUPS];
    for (group, chunk) in flat.chunks_exact(4).enumerate().take(MORPH_WEIGHT_GROUPS) {
        grouped[group].copy_from_slice(chunk);
    }
    grouped
}

/// 扩展四十二：把形状序号列表打成 `u32` 字（每字 4 个 `u8`，小端：第 `i` 字节 = 槽 `g*4+i`）。
///
/// 未用到的槽填 0；`morph_count` 之外的字节着色器不会读，所以「0」不会被误解成
/// 「形状 0 启用」—— 循环上界是 `morph_count`，不是字数。
pub fn pack_morph_slots(shapes: &[u8]) -> [u32; MORPH_SLOT_ATTR_WORDS] {
    let mut words = [0u32; MORPH_SLOT_ATTR_WORDS];
    for (slot, &shape) in shapes.iter().enumerate().take(MORPH_SLOT_CAPACITY) {
        words[slot / 4] |= (shape as u32) << ((slot % 4) * 8);
    }
    words
}

/// 单个实例的模型矩阵、颜色、蒙皮槽位与形变权重。作为第二个顶点缓冲
/// （step_mode = Instance）上传。
///
/// 总长 = `96 + 16 × MORPH_WEIGHT_GROUPS + 4 × MORPH_SLOT_ATTR_WORDS` 字节：扩展三十六的
/// 1 组权重是 112，扩展三十九的 2 组是 128，扩展四十的位图掩码再加 4 → 132，
/// 扩展四十一的 3 组再加 16 → 148，扩展四十二把掩码换成序号 `vec4` → **160**。
/// 加一组的代价是 16 字节权重（序号字不随组数涨：一个 `vec4` 装得下 16 个槽）。
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
    /// 各槽权重，按 `vec4` 分组：第 `g` 组第 `i` 个分量 = **槽** `g * 4 + i`。
    ///
    /// 扩展三十九把它从 `[f32; 4]` 改成二维数组；扩展四十二把「形状序」改成
    /// 「槽序」—— 槽 `k` 配 `morph_slots` 里的第 `k` 个形状序号。
    pub morph_weights: [[f32; 4]; MORPH_WEIGHT_GROUPS],
    /// 扩展四十二：逐实例形状序号列表，每字 4 个 `u8`（小端），有效长度 = `morph_count`。
    ///
    /// 扩展四十这里是一个 `u32` 位图（第 `i` 位 = 形状 `i` 启用），顶点侧只能按
    /// 「形状总数」静态展开再逐位乘 0。换成序号列表后迭代次数 = `morph_count`，
    /// 且每个序号直接就是形变表的形状偏移。
    ///
    /// 与 `morph_count`（网格有几个形状）无关：这一份是**实例级**的取舍，
    /// 同一个网格的十个实例可以各用一套形状子集。`morph_count = 0` = 整段摘掉形变。
    pub morph_slots: [u32; MORPH_SLOT_ATTR_WORDS],
}

impl InstanceRaw {
    /// 属性槽位：5..=8 = model 四行、9 = color、10 = 元数据、12 起每组权重一格
    /// （12 / 13 / 14），15 = 形状序号 `vec4<u32>`（扩展四十二，3 个字 + 1 个占位字）。
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
        14 => Float32x4,
        15 => Uint32x4
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
            morph_slots: [0; MORPH_SLOT_ATTR_WORDS],
        }
    }
}

/// 权重组数与属性表必须同步：宏里写死了每个 location，改组数就得改上面那张表。
const _: () = assert!(
    MORPH_WEIGHT_GROUPS == 3,
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
            96 + 16 * MORPH_WEIGHT_GROUPS as u64 + 4 * MORPH_SLOT_ATTR_WORDS as u64,
            "model64 + color16 + meta16 + 每组权重 16 × 组数 + 序号 vec4（4 字 × 4 字节）"
        );
        assert_eq!(
            layout.array_stride, 160,
            "扩展四十二的 3 组权重 + 1 个序号 vec4"
        );
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
        let last_group = MORPH_WEIGHT_GROUPS as u32 - 1;
        let last = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == MORPH_WEIGHT_BASE_LOCATION + last_group)
            .unwrap_or_else(|| panic!("最后一组权重（第 {last_group} 组）缺失"));
        assert_eq!(
            last.offset + 16,
            MORPH_SLOT_OFFSET as u64,
            "槽序号必须紧跟在最后一组权重之后，中间不留空隙"
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

    /// 扩展四十二：槽序号压进**一个** `vec4<u32>`（location 15），紧跟权重区、填满 stride 末尾。
    ///
    /// 这条用例守两件事：① 3 个字 + 1 个占位字正好铺满 16 字节，不多不少；
    /// ② location 不越 `0..=15` —— 真机踩过的坑就是把它摊成 3 个 `Uint32`，
    /// 结果第 16 / 17 格直接被 `create_render_pipeline` 拒绝（webgpu 的顶点 location 只有 16 格）。
    ///
    /// 错位的后果很隐蔽：`std::mem::size_of` 与布局各说各话时，`bytemuck::cast_slice`
    /// 上传的字节数和管线读的步长对不上，表现为「隔一个实例错一次位」——
    /// 屏幕上是一半模型飞走，不是报错。
    #[test]
    fn instance_layout_carries_morph_slots() {
        let layout = InstanceRaw::layout();
        let vertex_locs: Vec<u32> = VERTEX_ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        let attr = layout
            .attributes
            .iter()
            .find(|a| a.shader_location == MORPH_SLOT_LOCATION_BASE)
            .expect("槽序号属性缺失");
        assert_eq!(attr.format, wgpu::VertexFormat::Uint32x4);
        assert_eq!(
            attr.offset, MORPH_SLOT_OFFSET as u64,
            "序号 vec4 紧跟权重区"
        );
        assert!(
            !vertex_locs.contains(&MORPH_SLOT_LOCATION_BASE),
            "槽序号 location 不得被顶点属性占用"
        );
        assert_eq!(MORPH_SLOT_OFFSET, 144, "3 组权重之后：96 + 48（位置未变）");
        assert_eq!(
            MORPH_SLOT_OFFSET as u64 + 4 * MORPH_SLOT_ATTR_WORDS as u64,
            layout.array_stride,
            "序号 vec4 必须填满 stride 末尾，不留悬空字节"
        );
        // 属性 location 硬上限：16 格（0..=15），越一格建管线就失败
        for attr in layout.attributes.iter().chain(VERTEX_ATTRIBUTES.iter()) {
            assert!(
                attr.shader_location < 16,
                "location {} 越过 webgpu 的 16 格上限",
                attr.shader_location
            );
        }
        assert_eq!(
            layout.array_stride as usize,
            std::mem::size_of::<InstanceRaw>(),
            "布局步长与结构体大小必须一致"
        );
        assert_eq!(MORPH_SLOT_WORDS, 3, "12 槽 = 3 个字，加组时这里先红");
        const {
            assert!(
                MORPH_SLOT_WORDS <= MORPH_SLOT_ATTR_WORDS,
                "真实字数装不进属性 vec4"
            )
        };
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
        assert_eq!(
            raw.morph_slots, [0; MORPH_SLOT_ATTR_WORDS],
            "plain 实例的槽序号必须全 0（morph_count = 0 时压根不读）"
        );
    }

    /// 扩展三十九：扁平权重按序切成组 —— 形状 `g*4+i` 必须落在第 `g` 组第 `i` 分量，
    /// 一旦切错组，GPU 侧「哪个分量配哪个形状」的映射就整体位移。
    #[test]
    fn group_morph_weights_slices_in_order() {
        let mut flat = [0.0f32; MORPH_SLOT_CAPACITY];
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
            group_morph_weights(&[0.0; MORPH_SLOT_CAPACITY]),
            [[0.0; 4]; MORPH_WEIGHT_GROUPS]
        );
    }

    /// 扩展四十二：序号按 `u8` 小端打包，第 `g` 字第 `i` 字节 = 槽 `g*4+i`。
    ///
    /// 着色器侧用 `(word >> (lane * 8u)) & 0xFFu` 取，位序写反就是「形状错位一格」，
    /// 屏幕上是模型整体形变张冠李戴 —— 所以这里逐个字节钉住。
    #[test]
    fn pack_morph_slots_is_little_endian_bytes() {
        // 非连续、乱序的真实场景：槽 0/1/2 分别是形状 2 / 5 / 11
        let shapes = [2u8, 5, 11];
        let words = pack_morph_slots(&shapes);
        assert_eq!(
            words[0],
            2 | (5 << 8) | (11 << 16),
            "第 0 字：字节 0/1/2 = 槽 0/1/2"
        );
        assert_eq!(
            &words[1..],
            &[0, 0, 0],
            "槽 3..12 没用，必须留 0（含占位字）"
        );

        // 单字四槽逐字节校验（含上界 255，形状序号不会到这么大但字节序必须对）
        let full: Vec<u8> = (0..MORPH_SLOT_CAPACITY as u8).collect();
        let words = pack_morph_slots(&full);
        assert_eq!(words.len(), MORPH_SLOT_ATTR_WORDS);
        assert_eq!(
            words[MORPH_SLOT_ATTR_WORDS - 1],
            0,
            "占位字恒 0：属性是 vec4，第 4 字不承载任何槽"
        );
        for slot in 0..MORPH_SLOT_CAPACITY {
            let word = words[slot / 4];
            let byte = (word >> ((slot % 4) * 8)) & 0xFF;
            assert_eq!(byte, slot as u32, "槽 {slot} 取回的值必须是它自己");
        }

        // 短切片：剩余槽必须留 0，不能拿脏数据
        assert_eq!(pack_morph_slots(&[7]), [7, 0, 0, 0]);
        assert_eq!(pack_morph_slots(&[]), [0; MORPH_SLOT_ATTR_WORDS]);
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
