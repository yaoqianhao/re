//! C03b 渲染侧可判定面：布局、打包、索引类型、着色器语法、混合口径。
//!
//! 这里一条像素断言都没有——CI 无 GPU，任何「画出来对不对」的断言都会变成 flaky 的
//! 伪测试。地形真正的观感由 C07 的人工截图（`evidence/`）负责，本文件只钉住那些
//! 「改错了不会有编译错误、但一定会出事」的接缝。

use r3e_render::mesh::{vertex_layout, InstanceRaw, MORPH_SLOT_LOCATION_BASE};
use r3e_render::terrain::{
    layer_index, mix_layers, needs_u32_indices, normalise_weights, pack_camera, pack_vertices,
    terrain_vertex_layout, TerrainDrawItem, TerrainLayersUniform, TerrainVertex,
    CAMERA_UNIFORM_BYTES, CAMERA_UNIFORM_FLOATS, TERRAIN_ATTRIBUTES, TERRAIN_LAYER_COUNT,
    TERRAIN_VERTEX_STRIDE, TERRAIN_WGSL,
};

/// 测试 1：地形布局是独立的一份，不复用 PBR/实例布局的任何一步。
///
/// 断言口径不是「location 编号不重叠」——PBR 已把 16 格占满，重叠不可避免，也不该介意：
/// 两条管线的 location 编号空间互相独立。真正要防的是**手滑复用**：把
/// `vertex_layout()` 或 `InstanceRaw::layout()` 直接塞进地形管线的 `buffers`。所以这里钉
/// 「地形只有 1 个缓冲、步进为 Vertex、属性恰好 4 条、编号连续从 0 起、不带实例属性」。
#[test]
fn 地形布局与_pbr_实例布局互不复用() {
    let terrain = terrain_vertex_layout();
    assert_eq!(terrain.step_mode, wgpu::VertexStepMode::Vertex);
    assert_eq!(terrain.attributes.len(), 4);
    let locations: Vec<u32> = terrain
        .attributes
        .iter()
        .map(|a| a.shader_location)
        .collect();
    assert_eq!(locations, vec![0, 1, 2, 3]);
    let formats: Vec<wgpu::VertexFormat> = terrain.attributes.iter().map(|a| a.format).collect();
    assert_eq!(
        formats,
        vec![
            wgpu::VertexFormat::Float32x3,
            wgpu::VertexFormat::Float32x3,
            wgpu::VertexFormat::Float32x2,
            wgpu::VertexFormat::Float32x4,
        ]
    );

    // 地形布局与 PBR 顶点布局是两份不同的表：stride 不同、属性条数不同
    let pbr = vertex_layout();
    assert_ne!(terrain.array_stride, pbr.array_stride);
    assert_ne!(terrain.attributes.len(), pbr.attributes.len());
    // 且地形不碰实例侧任何一格（实例属性从 5 起，含 shape-slot 的 15）
    let instance = InstanceRaw::layout();
    assert_eq!(instance.step_mode, wgpu::VertexStepMode::Instance);
    for attr in terrain.attributes.iter() {
        assert!(
            attr.shader_location < 4,
            "地形属性落进了实例侧编号：{}",
            attr.shader_location
        );
    }
    // 实例侧形变槽从 15 起（PBR 顶点占满外加实例 4 格后的最后一格）
    const { assert!(MORPH_SLOT_LOCATION_BASE >= 4) };

    // 属性表常量与布局函数必须同源（防有人在两处各写一份）
    assert_eq!(terrain.attributes, &TERRAIN_ATTRIBUTES[..]);
}

/// 测试 2：stride 与结构体逐字节对齐，没有 padding 漂移。
#[test]
fn 地形顶点_stride_等于_48_字节() {
    assert_eq!(TERRAIN_VERTEX_STRIDE, 3 * 4 + 3 * 4 + 2 * 4 + 4 * 4);
    assert_eq!(
        std::mem::size_of::<TerrainVertex>(),
        TERRAIN_VERTEX_STRIDE as usize
    );
    assert_eq!(
        terrain_vertex_layout().array_stride,
        TERRAIN_VERTEX_STRIDE as u64
    );
    // 属性偏移必须是紧密排列的
    let offsets: Vec<u64> = TERRAIN_ATTRIBUTES.iter().map(|a| a.offset).collect();
    assert_eq!(offsets, vec![0, 12, 24, 32]);
}

/// 测试 3：权重在打包阶段就被纠正成「和为 1」。
#[test]
fn 权重打包时纠正归一化() {
    // 和 = 2 的输入被压回 1，且比例不变
    let w = normalise_weights([1.0, 1.0, 0.0, 0.0]);
    assert!((w[0] - 0.5).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6);
    // 和 = 0.5 的输入被抬到 1
    let w = normalise_weights([0.25, 0.25, 0.0, 0.0]);
    assert!((w[0] - 0.5).abs() < 1e-6);
    // 负分量夹成 0 后重新归一
    let w = normalise_weights([2.0, -1.0, 2.0, 0.0]);
    assert!((w[0] - 0.5).abs() < 1e-6 && w[1] == 0.0 && (w[2] - 0.5).abs() < 1e-6);
    // 全 0 / NaN / 无穷 → 层 0 独占，绝不产出 NaN
    for bad in [
        [0.0, 0.0, 0.0, 0.0],
        [f32::NAN, 0.0, 0.0, 0.0],
        [f32::INFINITY, 0.0, 0.0, 0.0],
    ] {
        assert_eq!(normalise_weights(bad), [1.0, 0.0, 0.0, 0.0]);
    }
    // 已归一化的输入原样通过（只允许浮点误差）
    let w = normalise_weights([0.2, 0.3, 0.4, 0.1]);
    let sum: f32 = w.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6);
}

/// 测试 4：`TerrainMesh` → GPU 顶点数组的转换守恒（顶点数、索引数都不许少）。
#[test]
fn 顶点与索引数量守恒() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ];
    let normals = [[0.0, 1.0, 0.0]; 4];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let weights = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let indices = [0u32, 1, 2, 0, 2, 3];
    let item = TerrainDrawItem::from_parts(&positions, &normals, &uvs, &weights, &indices);
    assert_eq!(item.vertices.len(), positions.len());
    assert_eq!(item.indices.len(), indices.len());
    assert!(!item.is_empty());
    // 顶点内容逐字段对齐（第 2 个顶点带岩层权重）
    assert_eq!(item.vertices[1].position, positions[1]);
    assert_eq!(item.vertices[1].weights, [0.0, 1.0, 0.0, 0.0]);

    // 长度不齐时以最短者为准，不 panic
    let short = pack_vertices(&positions, &normals[..2], &uvs, &weights);
    assert_eq!(short.len(), 2);

    // 空索引 = 空绘制项，渲染循环据此跳过
    let empty = TerrainDrawItem::from_parts(&positions, &normals, &uvs, &weights, &[]);
    assert!(empty.is_empty());
}

/// 测试 5：索引一律 `u32`——地形顶点数会超过 `u16` 上限，这条不是「以防万一」。
#[test]
fn 索引类型为_u32_且覆盖溢出场景() {
    assert!(!needs_u32_indices(65_536));
    assert!(needs_u32_indices(65_537));
    // 200 米 / 0.78 米 ≈ 256 列 × 256 行 ≈ 66k 顶点，确实越界
    assert!(needs_u32_indices(66_049));

    // 索引字段本身就是 u32：编译期即保证，这里用一个超 u16 的值跑通一次打包
    let big = 70_000u32;
    let indices = [0u32, big];
    let item = TerrainDrawItem::from_parts(
        &[[0.0, 0.0, 0.0]; 2],
        &[[0.0, 1.0, 0.0]; 2],
        &[[0.0, 0.0]; 2],
        &[[1.0, 0.0, 0.0, 0.0]; 2],
        &indices,
    );
    assert_eq!(item.indices[1], big);
    assert_eq!(
        std::mem::size_of::<u32>(),
        std::mem::size_of_val(&item.indices[0])
    );
}

/// 测试 6：材质名对不上时回落常量色，不 panic、不吞整块地形。
#[test]
fn 缺省材质回落常量色() {
    let layers = TerrainLayersUniform::default();
    assert_eq!(TERRAIN_LAYER_COUNT, 4);
    for (name, idx) in [("grass", 0), ("Rock", 1), (" sand ", 2), ("snow", 3)] {
        assert_eq!(layer_index(name), Some(idx));
        assert_eq!(layers.color_of(name), layers.base[idx]);
    }
    // 拼错的名字 / 空串 / 未知层：落到层 0 常量色
    for name in ["graass", "", "water", "雪"] {
        assert_eq!(layer_index(name), None);
        assert_eq!(
            layers.color_of(name),
            TerrainLayersUniform::default().base[0]
        );
    }
    // set_layer 认名字返回 true，不认返回 false 且不改动任何层
    let mut custom = TerrainLayersUniform::default();
    assert!(custom.set_layer("rock", [0.1, 0.2, 0.3, 1.0]));
    assert_eq!(custom.base[1], [0.1, 0.2, 0.3, 1.0]);
    let before = custom;
    assert!(!custom.set_layer("magma", [1.0, 0.0, 0.0, 1.0]));
    assert_eq!(custom, before);
}

/// 测试 11：`terrain.wgsl` 能被 naga 解析 + 语义校验（不需要 GPU）。
#[test]
fn 地形着色器可被_naga_解析() {
    let module = naga::front::wgsl::parse_str(TERRAIN_WGSL).expect("terrain.wgsl 解析失败");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator
        .validate(&module)
        .expect("terrain.wgsl 语义校验失败");
    // 入口点必须就是管线创建处写的那两个名字
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert!(names.contains(&"vs_terrain"), "缺少 vs_terrain：{names:?}");
    assert!(names.contains(&"fs_terrain"), "缺少 fs_terrain：{names:?}");
    // 顶点入口必须吃满 4 个 location（少一个 = CPU 传了 GPU 不读，静默丢权重）
    let vs = module
        .entry_points
        .iter()
        .find(|e| e.name == "vs_terrain")
        .expect("vs_terrain");
    let arg = vs.function.arguments.first().expect("vs_terrain 无参数");
    let naga::TypeInner::Struct { members, .. } = &module.types[arg.ty].inner else {
        panic!("顶点入参不是 struct");
    };
    assert_eq!(members.len(), 4);
}

/// 测试 12：混合口径的 CPU 等价实现在单层权重下必须精确复现该层颜色。
#[test]
fn 混合函数在单层权重下等于该层颜色() {
    let base = TerrainLayersUniform::default().base;
    for idx in 0..TERRAIN_LAYER_COUNT {
        let mut w = [0.0f32; 4];
        w[idx] = 1.0;
        let mixed = mix_layers(w, &base);
        for c in 0..3 {
            assert!(
                (mixed[c] - base[idx][c]).abs() < 1e-6,
                "层 {idx} 通道 {c} 混合结果偏离"
            );
        }
    }
    // 对半混：结果恰是两层中点
    let mixed = mix_layers([0.5, 0.5, 0.0, 0.0], &base);
    for c in 0..3 {
        let want = (base[0][c] + base[1][c]) * 0.5;
        assert!((mixed[c] - want).abs() < 1e-6);
    }
    // 权重全 0 → 全黑（这里刻意不归一化：口径与 wgsl 的 mix_layers 一致）
    assert_eq!(mix_layers([0.0; 4], &base), [0.0, 0.0, 0.0]);
}

/// 测试 13：相机 uniform 的打包长度必须等于缓冲字节数。
///
/// 这条钉的是一个**只在真机第一帧才炸**的接缝：打包数组原来按 `[f32; 32]`（128 字节）
/// 开，而缓冲按 `CAMERA_UNIFORM_BYTES`（112 字节）开，headless 测试全绿，
/// `cargo run` 直接撞 `Queue::write_buffer` 的「128 字节写进 112 字节缓冲」校验并 panic
/// （C07 收口跑真窗口驱动时才暴露）。此处断言「打包长度 × 4 == 缓冲长度」并逐字段核对落位。
#[test]
fn 相机_uniform_打包长度等于缓冲字节数() {
    let vp: [[f32; 4]; 4] = [
        [1.0, 2.0, 3.0, 4.0],
        [5.0, 6.0, 7.0, 8.0],
        [9.0, 10.0, 11.0, 12.0],
        [13.0, 14.0, 15.0, 16.0],
    ];
    let packed = pack_camera(&vp, [0.5, 1.5, -2.5], [0.0, -1.0, 0.0], [0.8, 0.3]);
    assert_eq!(packed.len() * 4, CAMERA_UNIFORM_BYTES as usize);
    assert_eq!(packed.len(), CAMERA_UNIFORM_FLOATS);
    for (row, values) in vp.iter().enumerate() {
        assert_eq!(&packed[row * 4..row * 4 + 4], values.as_slice());
    }
    assert_eq!(&packed[16..19], &[0.5, 1.5, -2.5]);
    assert_eq!(packed[20], 0.8);
    assert_eq!(packed[21], 0.3);
    assert_eq!(&packed[24..27], &[0.0, -1.0, 0.0]);
    // eye.w / light.zw / sun_dir.w 是保留位，必须为 0，否则着色器读到脏值
    for idx in [19usize, 22, 23, 27] {
        assert_eq!(packed[idx], 0.0, "保留位 {idx} 不是 0");
    }
}
