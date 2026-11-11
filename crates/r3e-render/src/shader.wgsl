// 基础前向渲染：单方向光 + 环境光，直接光照用 Cook-Torrance BRDF
// （GGX 法线分布 / Smith-Schlick-GGX 遮蔽 / Schlick 菲涅尔），
// 漫反射为能量守恒形式；另有自发光项。每个物件通过实例顶点缓冲传入模型矩阵与颜色。

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    // 环境探针（扩展十二）：三色天空 + 太阳项，逐字段对应 r3e_core::env::EnvironmentSettings
    env_zenith: vec4<f32>,   // rgb = 天顶色，a = IBL 总强度
    env_horizon: vec4<f32>,  // rgb = 地平线色，a = 是否启用 IBL（0/1）
    env_ground: vec4<f32>,   // rgb = 地面色
    env_sun: vec4<f32>,      // rgb = 太阳色 × 太阳强度
    env_sun_dir: vec4<f32>,  // xyz = 指向太阳的单位向量
    // 9 项球谐辐亮度系数（rgb），CPU 侧由 env::Sh9::project 投影后上传。
    // 扩展二十三：配了 HDR 探针时这里换成探针自身投影出的辐照度（仍是 9 项），
    // 因此漫反射分支一字未改 —— 未配探针时系数与扩展十二完全一致。
    env_sh: array<vec4<f32>, 9>,
    // 扩展二十三：HDR 立方体反射探针。
    //   x = 是否启用（0/1），y = 镜面 mip 层数，z = 探针强度，w = 面边长（诊断用）
    // x 为 0 时着色器一次都不采样 `t_env_cube`，镜面 IBL 走 `env_radiance`。
    env_probe: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u_camera: Camera;

// P2：每材质一组 —— base color 染色 + PBR 标量 + 可选贴图。
// 布局必须与 `r3e-render::pbr::MaterialUniform` 逐字段一致（64 字节）。
struct Material {
    base_color: vec4<f32>,
    // x = metallic，y = roughness，z = 是否有 base color 贴图，w 保留
    params: vec4<f32>,
    // rgb = 自发光（线性 HDR，只加不减、不参与光照），a 保留
    emissive: vec4<f32>,
    // 扩展二十一：x/y/z = 法线 / 粗糙度 / AO 贴图是否挂载（0/1），w 保留。
    // 三位全 0 时着色器完全不采样这三张图，材质行为与扩展十一的纯标量一致。
    params2: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> u_material: Material;

@group(1) @binding(1)
var t_base_color: texture_2d<f32>;

@group(1) @binding(2)
var s_base_color: sampler;

// 扩展二十一：三张 PBR 数据贴图。它们都是**线性**纹理（`Rgba8Unorm`，
// 不是 sRGB）—— 法线、粗糙度、AO 存的是几何/材质数据而不是颜色，
// 按 sRGB 解码会把 0.5 的中性法线拉成 0.21，法线方向整体偏折。
// 三张图共用 binding(2) 的采样器：过滤方式相同，没必要重复绑定。
@group(1) @binding(3)
var t_normal_map: texture_2d<f32>;

@group(1) @binding(4)
var t_roughness_map: texture_2d<f32>;

@group(1) @binding(5)
var t_occlusion_map: texture_2d<f32>;

// 扩展二十三：HDR 反射探针 —— 预滤波 mip 链立方体贴图（`Rgba16Float`，线性 HDR）。
// 第 0 层是清晰原图，第 n 层对应粗糙度 n/(层数-1)；没有探针时绑的是 1×1 全黑兜底图，
// 配合 `u_camera.env_probe.x = 0` 保证着色器不会走到采样分支。
@group(1) @binding(6)
var t_env_cube: texture_cube<f32>;

// P4：关节矩阵表。所有蒙皮实例共用一块 storage buffer，实例用 `joint_base`
// 指出自己那段起点；非蒙皮实例权重全 0，不会读到这些槽位。
@group(2) @binding(0)
var<storage, read> u_joints: array<mat4x4<f32>>;

// 扩展三十六：morph target（表情 / 口型）增量表。所有带形状的网格共用一块
// storage buffer，每形状内按顶点紧密排列，每顶点 9 个 f32（扩展三十七加了切线通道）：
//
//   [0..3) = 位置增量 xyz    [3..6) = 法线增量 xyz    [6..9) = 切线增量 xyz
//
// 实例的 `instance_meta`（location 10）给出 `y = 起点（浮点下标）`、`z = 每形状浮点数`
// （= 顶点数 × 9）、`w = 可用形状数`；顶点下标由 `@builtin(vertex_index)` 提供，
// 所以没有形状的实例（instance_meta.w = 0）一次循环都不进。
@group(2) @binding(1)
var<storage, read> morph_table: array<f32>;

/// 形变结果：位置 / 法线 / 切线三组增量（法线在乘完模型伴随矩阵后统一 normalize）。
struct MorphDelta {
    position: vec3<f32>,
    normal: vec3<f32>,
    // 扩展三十七：切线增量。形状没给切线通道时这里是 0；
    // 切线的手性 w 不受形变影响（形变不改变 UV 镜像关系）。
    tangent: vec3<f32>,
};

/// 每顶点在形变表里占用的 f32 个数 —— 必须与 `r3e_core::morph::MORPH_VERTEX_FLOATS` 一致。
/// 布局：+0..3 = Δ位置，+3..6 = Δ法线，+6..9 = Δ切线（三个通道恒占槽位，缺的补 0）。
const MORPH_VERTEX_FLOATS: u32 = 9u;

/// 单实例可同时生效的形状数 —— 必须与 `r3e_core::MAX_MORPH_TARGETS` 一致（扩展三十九：8）。
///
/// 权重按 `vec4` 分组进顶点属性，所以这个数恒等于组数 × 4；`tests/morph_shader_parity.rs`
/// 会把这里与 Rust 常量逐字比对，改一处漏另一处会直接红。
const MORPH_SHAPES: u32 = 8u;

/// 累加单个形状的增量。`at` = 该形状在本顶点上的浮点起点，`w` = 权重。
///
/// 抽成函数是因为扩展三十九之后要累加 8 个形状 —— 每形状三行内联会写成 24 行，
/// 且「哪个分量配哪个形状」的对应关系会散进缩进里看不见。
fn morph_accumulate(delta: ptr<function, MorphDelta>, at: u32, w: f32) {
    (*delta).position += w * vec3<f32>(morph_table[at], morph_table[at + 1u], morph_table[at + 2u]);
    (*delta).normal += w * vec3<f32>(morph_table[at + 3u], morph_table[at + 4u], morph_table[at + 5u]);
    (*delta).tangent += w * vec3<f32>(morph_table[at + 6u], morph_table[at + 7u], morph_table[at + 8u]);
}

/// 扩展四十：把 4 位掩码展开成 `vec4<f32>` 的 0/1 乘子。
///
/// 用 `select` 而不是 4 个 `if`：掩码是**实例级**数据，同一批次里每个实例都可能
/// 不同，分支会让 warp 内部发散；`select` 是逐分量取值，没有控制流。
/// 展开后与权重相乘，掩码外的分量直接变 0 —— 下面的 `w != 0.0` 判断自然跳过。
fn morph_mask_vec(mask: u32, shift: u32) -> vec4<f32> {
    let m = mask >> shift;
    return vec4<f32>(
        select(0.0, 1.0, (m & 1u) != 0u),
        select(0.0, 1.0, (m & 2u) != 0u),
        select(0.0, 1.0, (m & 4u) != 0u),
        select(0.0, 1.0, (m & 8u) != 0u),
    );
}

/// 按权重线性累加各形状增量。与 `r3e_core::morph::MorphSet::apply` 逐式对应。
///
/// 权重按 `vec4` 分组传进来：`w0` 是形状 0..4，`w1` 是形状 4..8（扩展三十九）。
/// 分量逐个写而不是 `weights[i]` —— WGSL 不允许用运行时值索引向量分量，
/// 这也是 `skin_matrix` 采用同一写法的原因。形状数为 0 时整体零开销提前返回。
///
/// 扩展四十：`mask` 是逐实例形状掩码，仅在这里参与运算 —— 掩码外的形状权重先乘 0，
/// 后续「权重非零才累加」的判断就不会命中。掩码把全部形状关空的实例根本进不到这里：
/// 渲染层把它的 `morph_count`（`imeta.w`）写成 0，在下面的提前返回处就出去了。
fn morph_offset(
    imeta: vec4<u32>,
    w0: vec4<f32>,
    w1: vec4<f32>,
    mask: u32,
    vi: u32,
) -> MorphDelta {
    var delta: MorphDelta;
    delta.position = vec3<f32>(0.0);
    delta.normal = vec3<f32>(0.0);
    delta.tangent = vec3<f32>(0.0);
    let count = min(imeta.w, MORPH_SHAPES);
    // imeta.z = 0 表示该实例没有可用形状数据（未登记 / 校验未过），直接跳过
    if count == 0u || imeta.z == 0u {
        return delta;
    }
    // 顶点在该形状内的浮点起点；形状之间相隔一个 imeta.z
    let head = imeta.y + vi * MORPH_VERTEX_FLOATS;
    let stride = imeta.z;
    // 扩展四十：掩码外的分量先归零（乘子只有 0/1，权重原值不受影响）。
    // 不能写 `let w0 = w0 * ...` —— WGSL 不允许局部名遮蔽同名参数。
    let on0 = w0 * morph_mask_vec(mask, 0u);
    let on1 = w1 * morph_mask_vec(mask, 4u);
    // 第 1 组（形状 0..4）
    if on0.x != 0.0 {
        morph_accumulate(&delta, head, on0.x);
    }
    if count > 1u && on0.y != 0.0 {
        morph_accumulate(&delta, head + stride, on0.y);
    }
    if count > 2u && on0.z != 0.0 {
        morph_accumulate(&delta, head + stride * 2u, on0.z);
    }
    if count > 3u && on0.w != 0.0 {
        morph_accumulate(&delta, head + stride * 3u, on0.w);
    }
    // 第 2 组（形状 4..8）
    if count > 4u && on1.x != 0.0 {
        morph_accumulate(&delta, head + stride * 4u, on1.x);
    }
    if count > 5u && on1.y != 0.0 {
        morph_accumulate(&delta, head + stride * 5u, on1.y);
    }
    if count > 6u && on1.z != 0.0 {
        morph_accumulate(&delta, head + stride * 6u, on1.z);
    }
    if count > 7u && on1.w != 0.0 {
        morph_accumulate(&delta, head + stride * 7u, on1.w);
    }
    return delta;
}

// 阶段 D / 扩展十六：方向光阴影贴图（正交深度**数组**，每级一层）。
//   light_view_proj 按级下标存矩阵；split_ndc 存各级远端的相机裁剪空间 z
//   params: x = 1/size（纹素尺寸），y = 深度比较偏移，z = 是否启用（0/1），
//           w = 第 0 级的法线偏移量（世界单位，已按纹素与世界尺寸换算）
//   params2: x = 阴影强度，y = 本帧级数，z = 级间混合带宽（占该级跨度的比例）
// 长度 4 必须与 `r3e_core::MAX_CASCADES` 一致，`tests/shadow_wgsl.rs` 会解析比对。
struct Shadow {
    light_view_proj: array<mat4x4<f32>, 4>,
    split_ndc: vec4<f32>,
    params: vec4<f32>,
    params2: vec4<f32>,
};

@group(3) @binding(0)
var shadow_map: texture_depth_2d_array;

@group(3) @binding(1)
var shadow_sampler: sampler_comparison;

@group(3) @binding(2)
var<uniform> u_shadow: Shadow;

struct VsIn {
    // 扩展三十六：morph 增量表按顶点下标取，所以顶点着色器需要它
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // M1 起顶点带 UV（uv 原点左上，v 已由资产层做 v' = 1 - v 翻转），
    // 本阶段仅透传到片元着色器，尚未接入采样（贴图绑定在后续阶段）。
    @location(2) uv: vec2<f32>,
    // P4：每顶点 4 组关节槽位与权重（非蒙皮顶点权重为 0）
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    // 实例矩阵按列传入
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(9) color: vec4<f32>,
    // P4 + 扩展三十六：x = joint_base，y = morph_base，z = morph_stride，w = morph_count
    @location(10) instance_meta: vec4<u32>,
    // 扩展二十一：切线 xyz + 手性 w（bitangent = cross(N, T) * w）。
    // 用 location 11 而不是 5：5..=10 属于实例属性，两者共用同一命名空间。
    @location(11) tangent: vec4<f32>,
    // 扩展三十六 / 三十九：各形状权重，每组一个 `vec4`
    // （形状 g*4+i 取第 g 组的第 i 个分量；morph_count 之外的分量无意义）
    @location(12) morph_w0: vec4<f32>,
    @location(13) morph_w1: vec4<f32>,
    // 扩展四十：逐实例形状掩码，第 i 位 = 形状 i 启用（0 = 整段无形变）
    @location(14) morph_mask: u32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    // 世界空间切线（已去掉法线分量）+ 手性，供片元着色器构造 TBN
    @location(4) world_tangent: vec3<f32>,
    @location(5) tangent_handedness: f32,
};

// 线性混合蒙皮：顶点先在「关节空间」里被矩阵累加，再走实例矩阵。
//
// 顶点缓冲是资产导入时烘焙好的世界空间坐标，关节矩阵已经是
// `M_i = X⁻¹ · B_now_i · B_rest_i⁻¹`（X 为实例模型矩阵，见 core::skin）。
// 因此这里得到 `deformed = Σ w_i · M_i · p`，再乘 model 恰好回到世界空间；
// 静止姿态下 M_i = X⁻¹，world = p，与 P3 的静态画面完全一致。
fn skin_matrix(base: u32, joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    var m = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    if weights.x > 0.0 {
        m += u_joints[base + joints.x] * weights.x;
    }
    if weights.y > 0.0 {
        m += u_joints[base + joints.y] * weights.y;
    }
    if weights.z > 0.0 {
        m += u_joints[base + joints.z] * weights.z;
    }
    if weights.w > 0.0 {
        m += u_joints[base + joints.w] * weights.w;
    }
    return m;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);

    // 非蒙皮顶点权重全 0：直接跳过 storage 读取，保持内置网格路径零开销
    var local_position = in.position;
    var local_normal = in.normal;
    var local_tangent = in.tangent.xyz;

    // 扩展三十六：先做形变，再做蒙皮（顺序与 core 侧文档一致）。
    // 没有形状的实例 instance_meta.w = 0，整段被跳过 —— 顶点坐标与扩展三十五逐位相同，
    // 像素基线与零差异要求因此不需要额外容忍度。
    if in.instance_meta.w != 0u {
        let shape = morph_offset(
            in.instance_meta,
            in.morph_w0,
            in.morph_w1,
            in.morph_mask,
            in.vertex_index,
        );
        local_position += shape.position;
        local_normal += shape.normal;
        // 扩展三十七：切线同样参与形变。形状没给切线通道时这里是 0，
        // 后面的正交化读到的仍是原切线，像素与扩展三十六逐位相同。
        local_tangent += shape.tangent;
    }

    if in.weights.x + in.weights.y + in.weights.z + in.weights.w > 0.0 {
        let skinned = skin_matrix(in.instance_meta.x, in.joints, in.weights);
        local_position = (skinned * vec4<f32>(local_position, 1.0)).xyz;
        // 蒙皮矩阵含非均匀缩放时法线要逆转置；这里用伴随矩阵近似，与下方模型矩阵一致
        let skin_adjugate = mat3x3<f32>(
            cross(skinned[1].xyz, skinned[2].xyz),
            cross(skinned[2].xyz, skinned[0].xyz),
            cross(skinned[0].xyz, skinned[1].xyz),
        );
        local_normal = skin_adjugate * local_normal;
        // 用累积后的 local_tangent 而不是 in.tangent.xyz —— 否则形变给出的切线增量
        // 会在蒙皮这一步被悄悄丢掉（只有同时带形变与蒙皮的资产才会暴露）。
        local_tangent = skin_adjugate * local_tangent;
    }

    let world = model * vec4<f32>(local_position, 1.0);

    // 非均匀缩放下严格做法是用逆转置；此处用伴随矩阵近似，均匀缩放时精确。
    let normal_matrix = mat3x3<f32>(
        cross(model[1].xyz, model[2].xyz),
        cross(model[2].xyz, model[0].xyz),
        cross(model[0].xyz, model[1].xyz),
    );

    var out: VsOut;
    out.clip_position = u_camera.view_proj * world;
    out.world_position = world.xyz;
    // 退化缩放（某轴 scale = 0 或 NaN）会让伴随矩阵整块归零，normalize(0) 直接产 NaN，
    // 一路传到着色就是成片刷黑。只在这种退化情形换成 +Y 兜底，正常路径仍走 normalize
    // ——逐位不变，像素基线不受影响。CPU 侧的同一保护见 `r3e_core::world_normal_matrix`。
    let raw_normal = normal_matrix * local_normal;
    if length(raw_normal) > 1e-20 {
        out.world_normal = normalize(raw_normal);
    } else {
        out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    }
    out.color = in.color;
    out.uv = in.uv;
    // 切线按与法线相同的伴随矩阵变换；未挂法线贴图时这些值不参与着色
    out.world_tangent = normal_matrix * local_tangent;
    out.tangent_handedness = select(-1.0, 1.0, in.tangent.w >= 0.0);
    return out;
}

// 阶段 D：阴影通道只写深度。顶点变换与主通道一致（含线性混合蒙皮与 morph
// 形变），因此形变后的网格投出的影子跟随姿势 —— 「张嘴时下巴的影子」也对得上。
struct ShadowVsIn {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(10) instance_meta: vec4<u32>,
    // 扩展三十九：形变权重两组，口径与主通道的 `VsIn` 完全一致
    @location(12) morph_w0: vec4<f32>,
    @location(13) morph_w1: vec4<f32>,
    // 扩展四十：口径与主通道一致
    @location(14) morph_mask: u32,
};

// 深度 pass 专用：只含光源正交矩阵，故意不复用阴影贴图那一组绑定，
// 否则同一张纹理会在本 pass 内「既被写入又被采样」。
struct ShadowPass {
    light_view_proj: mat4x4<f32>,
    pad: vec4<f32>,
};

@group(3) @binding(3)
var<uniform> u_shadow_pass: ShadowPass;

// ---------------- 扩展十五：局部光源（点光 / 聚光） ----------------
// 与 `r3e-core::light::{LocalLight, attenuation, spot_factor}` 逐式对应。
// 每盏灯 4 个 vec4（64 字节），定长 16 盏 —— 长度必须与 `MAX_LOCAL_LIGHTS` 一致，
// `tests/local_lights_shader.rs` 会解析本文件比对，改一处必须改三处。
//
//   pos_range   : xyz = 世界位置，w = 影响半径
//   color_power : rgb = 线性颜色，w = 强度
//   dir_inner   : xyz = 单位方向（聚光朝向），w = 内角余弦
//   outer_kind  : x = 外角余弦，y = 类型码（0 点光 / 1 聚光），zw 保留
struct LocalLight {
    pos_range: vec4<f32>,
    color_power: vec4<f32>,
    dir_inner: vec4<f32>,
    outer_kind: vec4<f32>,
};

struct Lights {
    // x = 本帧有效灯数，yzw 保留
    count: vec4<f32>,
    items: array<LocalLight, 16>,
};

// 光源表挂在 group(3)（与阴影贴图同组）：WebGPU core 规范 `max_bind_groups` = 4，
// 索引 4 的组直接过不了创建校验，所以这里复用场景级绑定组的第 5 个槽位。
@group(3) @binding(4)
var<uniform> u_lights: Lights;

// ---------------- 扩展十八 C：局部光源阴影（点光立方 6 面 / 聚光单面） ----------------
// 深度数组共 `MAX_LOCAL_SHADOW_LAYERS` 层，每盏投影灯固定占 6 层（聚光只用第 0 层，
// 其余空着），因此「第 i 盏投影灯的第 f 面」= `slots[i].x + f`，着色器不需要任何查表。
//
//   params        : x = 本帧投影灯数，y = 1/size（纹素尺寸），z = 是否启用（0/1），w 保留
//   params2       : x = 阴影强度，y = 本帧占用层数，zw 保留
//   view_proj[l]  : 该层「光源 → 裁剪」矩阵（CPU 侧与深度 pass 共用同一份，不会写读错位）
//   slots[i]      : x = layer_base（< 0 表示这盏灯本帧不投影），
//                   y = 面数（点光 6 / 聚光 1），z = 深度偏移（NDC），
//                   w = 法线偏移（世界单位，已按该灯单纹素世界尺寸换算）
//
// 长度 24 / 16 必须与 `r3e_core::MAX_LOCAL_SHADOW_LAYERS` / `MAX_LOCAL_LIGHTS` 一致：
// `tests/local_shadow_shader.rs` 会解析本文件比对，改一处必须改三处。
struct LocalShadow {
    params: vec4<f32>,
    params2: vec4<f32>,
    view_proj: array<mat4x4<f32>, 24>,
    slots: array<vec4<f32>, 16>,
};

@group(3) @binding(5)
var local_shadow_map: texture_depth_2d_array;

@group(3) @binding(6)
var local_shadow_sampler: sampler_comparison;

@group(3) @binding(7)
var<uniform> u_local_shadow: LocalShadow;

// 层数上界（与 CPU 侧 MAX_LOCAL_SHADOW_LAYERS 一致，作为下标夹取的上限）
const MAX_LOCAL_SHADOW_LAYERS: i32 = 24;

/// 点光立方体选面：主轴绝对值最大者胜出，符号定正负。
/// 与 `r3e_core::point_shadow_face` 逐分支对应（顺序 +X, -X, +Y, -Y, +Z, -Z）。
fn point_shadow_face(direction: vec3<f32>) -> i32 {
    let a = abs(direction);
    if a.x >= a.y && a.x >= a.z {
        if direction.x >= 0.0 {
            return 0;
        }
        return 1;
    }
    if a.y >= a.z {
        if direction.y >= 0.0 {
            return 2;
        }
        return 3;
    }
    if direction.z >= 0.0 {
        return 4;
    }
    return 5;
}

/// 单盏局部光源的阴影因子：1 = 全亮，0 = 全遮。
///
/// 点光先按「光源→片元」方向选出立方体的那一面，再只用该面的矩阵投影；
/// 聚光只有一面，直接用第 0 层。越出该面视锥（面边界上的邻域采样）按受光处理，
/// 与方向光「视锥外不投影」同一口径，避免面接缝出现硬黑边。
fn local_shadow_factor(
    index: u32,
    world_position: vec3<f32>,
    n: vec3<f32>,
    to_light: vec3<f32>,
) -> f32 {
    if u_local_shadow.params.z < 0.5 {
        return 1.0;
    }
    let slot = u_local_shadow.slots[index];
    // slot.x < 0：这盏灯本帧不投影（槽位默认值），直接全亮
    if slot.x < 0.0 || slot.y <= 0.0 {
        return 1.0;
    }
    var layer = i32(slot.x);
    if slot.y > 1.5 {
        layer = layer + point_shadow_face(-to_light);
    }
    let clamped = clamp(layer, 0, MAX_LOCAL_SHADOW_LAYERS - 1);
    let biased = world_position + n * slot.w;
    let clip = u_local_shadow.view_proj[clamped] * vec4<f32>(biased, 1.0);
    if clip.w <= 1e-6 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let cmp_depth = ndc.z - slot.z;
    let texel = u_local_shadow.params.y;
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            sum = sum + textureSampleCompare(
                local_shadow_map,
                local_shadow_sampler,
                uv + offset,
                clamped,
                cmp_depth,
            );
        }
    }
    let lit = sum / 9.0;
    // 强度混回 1：局部光源阴影只压直接光项，环境光不参与（否则暗部会死黑）
    return mix(1.0, lit, clamp(u_local_shadow.params2.x, 0.0, 1.0));
}

// 衰减分母的软化常数（与 CPU 侧 ATTENUATION_EPSILON 一致）：d→0 时仍有限
const ATTENUATION_EPSILON: f32 = 0.01;
// 平滑窗幂次（与 CPU 侧 ATTENUATION_WINDOW_POWER 一致）
const ATTENUATION_WINDOW_POWER: f32 = 4.0;
// 定长数组上界（与 CPU 侧 MAX_LOCAL_LIGHTS 一致，作为循环夹取的上限）
const MAX_LOCAL_LIGHTS: u32 = 16u;

/// 平滑窗逆平方衰减：(1 - (d/range)^4)² / (d² + ε)，d ≥ range 时为 0。
fn local_attenuation(range: f32, distance: f32) -> f32 {
    if range <= 0.0 {
        return 0.0;
    }
    let d = max(distance, 0.0);
    if d >= range {
        return 0.0;
    }
    let t = clamp(d / range, 0.0, 1.0);
    let window = 1.0 - pow(t, ATTENUATION_WINDOW_POWER);
    return (window * window) / (d * d + ATTENUATION_EPSILON);
}

/// 聚光锥因子：内外角相等（角度退化）时退成硬边阶跃，避免除零。
fn local_spot_factor(cos_angle: f32, cos_inner: f32, cos_outer: f32) -> f32 {
    if cos_inner <= cos_outer {
        return select(0.0, 1.0, cos_angle >= cos_inner);
    }
    let t = clamp((cos_angle - cos_outer) / (cos_inner - cos_outer), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

/// 把场景里所有局部光源算一遍，返回累加的出射辐亮度。
///
/// 与方向光用的是同一套 Cook-Torrance 函数，区别只有两处：
/// 光向量逐灯不同（`l = normalize(pos - p)`），且每盏灯额外乘距离衰减与锥因子。
/// 三处早退（超半径 / 背面 / 锥外）保证没被照到的片元不为它付 BRDF 的代价。
fn local_lighting(
    n: vec3<f32>,
    v: vec3<f32>,
    world_position: vec3<f32>,
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,
    f0: vec3<f32>,
) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let raw_count = u32(max(u_lights.count.x, 0.0));
    let count = min(raw_count, MAX_LOCAL_LIGHTS);
    for (var i = 0u; i < count; i = i + 1u) {
        let light = u_lights.items[i];
        let to_light = light.pos_range.xyz - world_position;
        let distance = length(to_light);
        let range = light.pos_range.w;
        // 早退 1：超出影响半径
        if range <= 0.0 || distance >= range {
            continue;
        }
        // 片元与光源重合时方向无意义，退回法线（衰减此时仍有限，不会爆白）
        let l = select(normalize(to_light), n, distance < 1e-4);
        let ndl = clamp(dot(n, l), 0.0, 1.0);
        // 早退 2：背面不接收这盏灯的光
        if ndl <= 0.0 {
            continue;
        }
        // 早退 3：聚光锥外
        var cone = 1.0;
        if light.outer_kind.y > 0.5 {
            let cos_angle = dot(light.dir_inner.xyz, -l);
            cone = local_spot_factor(cos_angle, light.dir_inner.w, light.outer_kind.x);
            if cone <= 0.0 {
                continue;
            }
        }
        let half_sum = l + v;
        let h = select(normalize(half_sum), n, length(half_sum) < 1e-4);
        let ndv = max(clamp(dot(n, v), 0.0, 1.0), 1e-4);
        let ndh = clamp(dot(n, h), 0.0, 1.0);
        let vdh = clamp(dot(v, h), 0.0, 1.0);
        let f = fresnel_schlick(vdh, f0);
        let d = distribution_ggx(ndh, roughness);
        let g = geometry_smith(ndv, ndl, roughness);
        let specular = f * d * g / (4.0 * ndv * ndl);
        let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
        let diffuse = kd * albedo / PI;
        let radiance = light.color_power.rgb * light.color_power.w;
        let falloff = local_attenuation(range, distance) * cone;
        // 扩展十八 C：只有投影灯才走深度比较，未投影的灯 slots 里是 -1，直接返回 1
        let shadow = local_shadow_factor(i, world_position, n, to_light);
        sum += (diffuse + specular) * radiance * ndl * falloff * shadow;
    }
    return sum;
}

@vertex
fn vs_shadow(in: ShadowVsIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    // 与主通道同序：先形变，再蒙皮（没有形状时整段跳过，结果逐位不变）
    var local_position = in.position;
    if in.instance_meta.w != 0u {
        let shape = morph_offset(
            in.instance_meta,
            in.morph_w0,
            in.morph_w1,
            in.morph_mask,
            in.vertex_index,
        );
        local_position += shape.position;
    }
    if in.weights.x + in.weights.y + in.weights.z + in.weights.w > 0.0 {
        let skinned = skin_matrix(in.instance_meta.x, in.joints, in.weights);
        local_position = (skinned * vec4<f32>(local_position, 1.0)).xyz;
    }
    return u_shadow_pass.light_view_proj * (model * vec4<f32>(local_position, 1.0));
}

// 单级采样的 3×3 PCF：把深度图比较结果在邻域里平均，边缘得到两三个纹素的柔和过渡。
// 该级阴影图按「世界 → 光源裁剪空间」定位，越界或超出远平面一律按「受光」处理，
// 避免视锥外物件出现硬边黑块。
fn shadow_sample_level(level: i32, biased_world: vec3<f32>, texel: f32) -> f32 {
    let clip = u_shadow.light_view_proj[level] * vec4<f32>(biased_world, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let cmp_depth = ndc.z - u_shadow.params.y;
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            sum = sum + textureSampleCompare(
                shadow_map,
                shadow_sampler,
                uv + offset,
                level,
                cmp_depth,
            );
        }
    }
    return sum / 9.0;
}

// 级联阴影（扩展十六）：
//   1) 用像素在相机裁剪空间的 ndc.z 找它属于哪一级（split_ndc 存各级远端 z）；
//   2) 该级内做 3×3 PCF；
//   3) 靠近本级远端时按 blend 带宽向下级渐变，消掉级间接缝的硬边。
// 超过最后一级远端的像素直接判为受光（远处不再有阴影，但不会出现黑块）。
fn shadow_factor(world_position: vec3<f32>, n: vec3<f32>) -> f32 {
    if u_shadow.params.z < 0.5 {
        return 1.0;
    }
    let levels = i32(u_shadow.params2.y);
    if levels <= 0 {
        return 1.0;
    }
    let cam_clip = u_camera.view_proj * vec4<f32>(world_position, 1.0);
    if abs(cam_clip.w) < 1e-6 {
        return 1.0;
    }
    let cam_z = cam_clip.z / cam_clip.w;
    var level = -1;
    for (var i = 0; i < levels; i = i + 1) {
        if cam_z <= u_shadow.split_ndc[i] {
            level = i;
            break;
        }
    }
    if level < 0 {
        return 1.0;
    }
    let texel = u_shadow.params.x;
    let biased = world_position + n * u_shadow.params.w;
    let strength = u_shadow.params2.x;
    let visible = shadow_sample_level(level, biased, texel);

    let blend = u_shadow.params2.z;
    if blend > 0.0 && level + 1 < levels {
        var prev_split = 0.0;
        if level > 0 {
            prev_split = u_shadow.split_ndc[level - 1];
        }
        let band = (u_shadow.split_ndc[level] - prev_split) * blend;
        if band > 0.0 && cam_z > u_shadow.split_ndc[level] - band {
            let t = clamp((cam_z - (u_shadow.split_ndc[level] - band)) / band, 0.0, 1.0);
            let next_visible = shadow_sample_level(level + 1, biased, texel);
            return mix(1.0, mix(visible, next_visible, t), strength);
        }
    }
    return mix(1.0, visible, strength);
}

// ---------------- 环境探针（IBL，扩展十二） ----------------
// 与 `r3e-core::env` 的 CPU 实现逐项对应：
//   * 天空：天顶 / 地平线 / 地面三色按 |y| 开方插值，太阳是 cos^8 的亮斑
//   * 漫反射：9 项球谐（l ≤ 2）经 Lambertian 余弦卷积，直接算辐照度，不需要 cubemap
//   * 镜面：反射方向按 roughness² 拉向法线后取天空亮度，再用 Karis 解析 BRDF 加权
// 之所以不上预滤波 cubemap / BRDF LUT：本引擎要在无窗口、无 GPU 的环境里跑集成测试，
// 全部用解析式才能在 CPU 侧留一份可断言的参考实现。

// 太阳盘锐度：cos^k 的指数，与 CPU 侧 SUN_SHAPE_EXPONENT 一致
const SUN_SHAPE_EXPONENT: f32 = 8.0;
// 9 项球谐的基常数（与 CPU sh_basis 逐位一致）
const SH_Y0: f32 = 0.2820948;
const SH_Y1: f32 = 0.4886028;
const SH_Y2_A: f32 = 1.0925484;
const SH_Y2_B: f32 = 0.3153916;
const SH_Y2_C: f32 = 0.5462742;
// Lambertian 余弦卷积系数：A0 = π，A1 = 2π/3，A2 = π/4
// （直接写字面量而不是引用 PI：这几行出现在 PI 声明之前，避免依赖声明顺序）
const LAMBERT_A0: f32 = 3.141592653589793;
const LAMBERT_A1: f32 = 2.094395102393195;
const LAMBERT_A2: f32 = 0.785398163397448;

/// 三色天空插值（不含太阳）。开方让地平线附近的过渡带更宽。
fn env_sky_base(dir: vec3<f32>) -> vec3<f32> {
    let up = clamp(dir.y, -1.0, 1.0);
    if up >= 0.0 {
        return mix(u_camera.env_horizon.rgb, u_camera.env_zenith.rgb, sqrt(up));
    }
    return mix(u_camera.env_horizon.rgb, u_camera.env_ground.rgb, sqrt(-up));
}

/// 太阳亮斑：只在地平线以上、朝太阳的半球出现，cos^8 收成一个盘。
fn env_sun(dir: vec3<f32>) -> vec3<f32> {
    let cos_theta = clamp(dot(dir, u_camera.env_sun_dir.xyz), -1.0, 1.0);
    if cos_theta <= 0.0 {
        return vec3<f32>(0.0);
    }
    return u_camera.env_sun.rgb * pow(cos_theta, SUN_SHAPE_EXPONENT);
}

/// 整球平均辐亮度：l = 0 系数即球面平均 × √(4π)，反过来除回去就是平均亮度。
fn env_average() -> vec3<f32> {
    return u_camera.env_sh[0].rgb * SH_Y0;
}

/// 按粗糙度模糊后的环境亮度：r 越大越接近方向无关的平均值，与 CPU 侧一致。
fn env_radiance(dir: vec3<f32>, roughness: f32) -> vec3<f32> {
    let r = clamp(roughness, 0.0, 1.0);
    let t = r * r;
    let attenuation = (1.0 - t) * (1.0 - t);
    let sharp = env_sky_base(dir) + env_sun(dir) * attenuation;
    return mix(sharp, env_average(), t);
}

/// 球谐辐照度 E(n)：投影系数 × 基函数 × Lambertian 权重，再夹 0 防二阶截断出负值。
fn env_irradiance(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var e = u_camera.env_sh[0].rgb * (SH_Y0 * LAMBERT_A0);
    e = e + u_camera.env_sh[1].rgb * (SH_Y1 * y * LAMBERT_A1);
    e = e + u_camera.env_sh[2].rgb * (SH_Y1 * z * LAMBERT_A1);
    e = e + u_camera.env_sh[3].rgb * (SH_Y1 * x * LAMBERT_A1);
    e = e + u_camera.env_sh[4].rgb * (SH_Y2_A * x * y * LAMBERT_A2);
    e = e + u_camera.env_sh[5].rgb * (SH_Y2_A * y * z * LAMBERT_A2);
    e = e + u_camera.env_sh[6].rgb * (SH_Y2_B * (3.0 * z * z - 1.0) * LAMBERT_A2);
    e = e + u_camera.env_sh[7].rgb * (SH_Y2_A * x * z * LAMBERT_A2);
    e = e + u_camera.env_sh[8].rgb * (SH_Y2_C * (x * x - y * y) * LAMBERT_A2);
    return max(e, vec3<f32>(0.0));
}

/// Karis 解析环境 BRDF（SIGGRAPH 2013 Mobile）：返回 (scale, bias)，
/// 高光亮度按 `F0 * scale + bias`。bias 夹 0（拟合在 r→1 会给负值）。
fn env_brdf(roughness: f32, ndv: f32) -> vec2<f32> {
    let r = clamp(roughness, 0.0, 1.0);
    let nv = clamp(ndv, 1e-4, 1.0);
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let k = r * c0 + c1;
    let a004 = min(k.x * k.x, exp2(-9.28 * nv)) * k.x + k.y;
    let scale = -1.04 * a004 + k.z;
    let bias = max(1.04 * a004 + k.w, 0.0);
    return vec2<f32>(scale, bias);
}

/// 扩展二十三：HDR 探针的镜面采样。
///
/// `lod` 与 CPU 侧 `r3e_render::envprobe::mip_lod` 是同一条映射
/// （`roughness × (层数-1)`），粗糙度越大取越糊的层；`Rgba16Float` 立方体贴图
/// 走 `textureSampleLevel` 显式给层号，GPU 在相邻两层之间做三线性插值。
/// 强度只乘在镜面项上（与 `EnvironmentMapSettings::intensity` 的语义一致）。
fn probe_specular(dir: vec3<f32>, roughness: f32) -> vec3<f32> {
    let levels = max(u_camera.env_probe.y, 1.0);
    let lod = clamp(roughness, 0.0, 1.0) * (levels - 1.0);
    let sampled = textureSampleLevel(t_env_cube, s_base_color, dir, lod);
    return max(sampled.rgb, vec3<f32>(0.0)) * u_camera.env_probe.z;
}

// ---------------- Cook-Torrance 直接光照 ----------------
// 与 `r3e-core::material` 的 CPU 参考实现逐项对应（那边有单测兜底）：
//   * D：GGX / Trowbridge-Reitz 法线分布
//   * G：Smith 遮蔽，用 Schlick-GGX 近似（直接光形式 k = (r+1)²/8）
//   * F：Schlick 菲涅尔近似（正入射 F0，掠射趋近 1）
//   * 漫反射：(1-metallic)·(1-F)·albedo/π —— 金属无漫反射，且扣掉镜面已拿走的份额
const PI: f32 = 3.141592653589793;
// 粗糙度下限：α = r²，r→0 时 D 项分母趋零会爆白，夹到与 CPU 侧一致的 0.045。
const MIN_ROUGHNESS: f32 = 0.045;
// 绝缘体基础反射率（4%）。
const DIELECTRIC_F0: f32 = 0.04;

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let r = clamp(roughness, MIN_ROUGHNESS, 1.0);
    let a = r * r;
    let a2 = a * a;
    let ndh = clamp(n_dot_h, 0.0, 1.0);
    // 分母 (n·h)²(α²-1)+1 恒 ≥ α² > 0，不会除零
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn geometry_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let r = clamp(roughness, MIN_ROUGHNESS, 1.0);
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    let nx = clamp(n_dot_x, 0.0, 1.0);
    return nx / (nx * (1.0 - k) + k);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    let c = clamp(1.0 - cos_theta, 0.0, 1.0);
    return f0 + (vec3<f32>(1.0) - f0) * pow(c, 5.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let geometric_n = normalize(in.world_normal);
    let l = normalize(-u_camera.light_dir.xyz);
    let v = normalize(u_camera.camera_pos.xyz - in.world_position);
    // 光方向与视线正相反时 l+v 退化，直接取法线作半程向量，避免 normalize(0) 出 NaN
    let half_sum = l + v;
    let h = select(normalize(half_sum), geometric_n, length(half_sum) < 1e-4);

    // base color：贴图（若存在）乘以材质染色因子，再乘物件颜色。
    // 贴图是 sRGB 纹理，采样后已是线性值；材质因子按线性因子直接相乘。
    var albedo = in.color.rgb * u_material.base_color.rgb;
    if u_material.params.z > 0.5 {
        let sampled = textureSample(t_base_color, s_base_color, in.uv);
        albedo = albedo * sampled.rgb;
    }
    // 反照率为负会把光"减掉"，与 CPU 侧一样按 0 处理
    albedo = max(albedo, vec3<f32>(0.0));

    // 扩展二十一：切线空间法线贴图 → 世界空间法线。
    // 未挂贴图（params2.x = 0）时直接沿用几何法线，与扩展十一完全一致。
    var n = geometric_n;
    if u_material.params2.x > 0.5 {
        // 用 Gram-Schmidt 把切线投影到法线的正交补上，抵消插值/缩放引入的偏差
        var tangent = in.world_tangent - geometric_n * dot(geometric_n, in.world_tangent);
        if length(tangent) < 1e-5 {
            // UV 退化或资产没算切线：任取一条与法线不平行的轴兜底，避免 normalize(0)
            tangent = select(
                vec3<f32>(1.0, 0.0, 0.0),
                vec3<f32>(0.0, 1.0, 0.0),
                abs(geometric_n.x) > 0.9,
            );
            tangent = tangent - geometric_n * dot(geometric_n, tangent);
        }
        let t = normalize(tangent);
        let b = cross(geometric_n, t) * in.tangent_handedness;
        // 线性纹理：采样值直接是 [-1,1] 的法线分量，正好用 *2-1 还原
        let sampled = textureSample(t_normal_map, s_base_color, in.uv).xyz * 2.0 - 1.0;
        // Z 分量至少留一点正值，防止贴图误差把法线翻到表面背面
        let tangent_normal = vec3<f32>(sampled.xy, max(sampled.z, 0.05));
        n = normalize(t * tangent_normal.x + b * tangent_normal.y + geometric_n * tangent_normal.z);
    }

    let metallic = clamp(u_material.params.x, 0.0, 1.0);
    // 粗糙度贴图（线性，取 G 通道：独立灰度图与 glTF 的 metallicRoughness 合并图
    // 都把粗糙度放在 G）。标量因子是乘数，缺贴图时退化为 params.y 本身。
    var roughness_scalar = u_material.params.y;
    if u_material.params2.y > 0.5 {
        roughness_scalar = roughness_scalar * textureSample(t_roughness_map, s_base_color, in.uv).g;
    }
    let roughness = clamp(roughness_scalar, MIN_ROUGHNESS, 1.0);
    // AO 贴图（线性，取 R 通道）：只衰减环境光，直接光由阴影负责，
    // 否则 AO 与阴影会重复压暗、接触处出现脏黑边。
    var ao = 1.0;
    if u_material.params2.z > 0.5 {
        ao = clamp(textureSample(t_occlusion_map, s_base_color, in.uv).r, 0.0, 1.0);
    }

    let ndl = clamp(dot(n, l), 0.0, 1.0);
    // n·v 掠射时留极小正数防除零；G 项在掠射本来就趋于 0，不会因此爆亮
    let ndv = max(clamp(dot(n, v), 0.0, 1.0), 1e-4);
    let ndh = clamp(dot(n, h), 0.0, 1.0);
    let vdh = clamp(dot(v, h), 0.0, 1.0);

    // 绝缘体 F0 = 0.04，金属用 base color 染色
    let f0 = mix(vec3<f32>(DIELECTRIC_F0), albedo, metallic);
    let f = fresnel_schlick(vdh, f0);
    let d = distribution_ggx(ndh, roughness);
    let g = geometry_smith(ndv, ndl, roughness);

    // 镜面项：D·G·F / (4·(n·v)·(n·l))；背面（n·l ≤ 0）直接给零，
    // 否则除以 0 会把 0·inf 变成 NaN
    var specular = vec3<f32>(0.0);
    if ndl > 0.0 {
        specular = f * d * g / (4.0 * ndv * ndl);
    }
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * albedo / PI;

    // 阶段 D：只有直接光（漫反射 + 镜面）被遮挡，环境光不衰减，
    // 否则影子会变成纯黑块、丢掉暗部层次。
    let shadow = shadow_factor(in.world_position, n);
    // 光色 / 强度语义不变：light_color.rgb 是线性颜色，.w 是强度乘数
    let radiance = u_camera.light_color.rgb * u_camera.light_color.w;

    // 环境光：场景写了 environment 段就走 IBL，否则退回常数 ambient（老场景画面不变）
    var ambient_term = albedo * u_camera.ambient.rgb;
    if u_camera.env_horizon.w > 0.5 {
        // 漫反射 IBL：辐照度 E(n) × albedo / π，金属没有漫反射
        let diffuse_ibl = albedo * (1.0 - metallic) * env_irradiance(n) / PI;
        // 镜面 IBL：粗糙表面看不到清晰的环境轮廓，先把反射方向按 roughness² 拉向法线。
        // R·n = n·v ≥ 0，理论上不会退化到零向量，仍留一道兜底防 normalize(0)
        let reflect_dir = reflect(-v, n);
        var dominant = mix(reflect_dir, n, roughness * roughness);
        dominant = select(normalize(dominant), n, length(dominant) < 1e-4);
        let ab = env_brdf(roughness, ndv);
        // 扩展二十三：配了 HDR 探针就按粗糙度取预滤波 mip 层，否则维持解析天空。
        // 分支条件是 uniform 常量，同一次 draw 内所有像素走同一条路径，没有散度代价。
        var env_specular = env_radiance(dominant, roughness);
        if u_camera.env_probe.x > 0.5 {
            env_specular = probe_specular(reflect_dir, roughness);
        }
        let specular_ibl = env_specular * (f0 * ab.x + ab.y);
        ambient_term = (diffuse_ibl + specular_ibl) * u_camera.env_zenith.w;
    }
    // 扩展二十一：AO 只压环境光（直接光已由阴影负责），emissive 不受影响
    ambient_term = ambient_term * ao;

    // 扩展十五：局部光源（点光 / 聚光）累加。与方向光同套 BRDF，只是逐灯重算光向量，
    // 并额外乘「平滑窗衰减 × 聚光锥因子」。扩展十八 C 起，标了 `cast_shadow` 的灯
    // 还会在灯内再乘一项局部阴影因子（点光立方 6 面 / 聚光单面），其余灯保持不投影。
    let local_term = local_lighting(n, v, in.world_position, albedo, metallic, roughness, f0);

    let lit = (diffuse + specular) * radiance * ndl * shadow
        + local_term
        + ambient_term
        // 自发光只加不减：不乘 n·l、不受阴影影响、不参与光照计算
        + u_material.emissive.rgb;

    return vec4<f32>(lit, in.color.a * u_material.base_color.a);
}
