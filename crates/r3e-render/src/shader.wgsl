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
    // 9 项球谐辐亮度系数（rgb），CPU 侧由 env::Sh9::project 投影后上传
    env_sh: array<vec4<f32>, 9>,
};

@group(0) @binding(0)
var<uniform> u_camera: Camera;

// P2：每材质一组 —— base color 染色 + PBR 标量 + 可选贴图。
// 布局必须与 `r3e-render::pbr::MaterialUniform` 逐字段一致（48 字节）。
struct Material {
    base_color: vec4<f32>,
    // x = metallic，y = roughness，z = 是否有贴图，w 保留
    params: vec4<f32>,
    // rgb = 自发光（线性 HDR，只加不减、不参与光照），a 保留
    emissive: vec4<f32>,
};

@group(1) @binding(0)
var<uniform> u_material: Material;

@group(1) @binding(1)
var t_base_color: texture_2d<f32>;

@group(1) @binding(2)
var s_base_color: sampler;

// P4：关节矩阵表。所有蒙皮实例共用一块 storage buffer，实例用 `joint_base`
// 指出自己那段起点；非蒙皮实例权重全 0，不会读到这些槽位。
@group(2) @binding(0)
var<storage, read> u_joints: array<mat4x4<f32>>;

// 阶段 D：方向光阴影贴图（正交深度图）。params:
//   x = 1/size（纹素尺寸），y = 深度比较偏移，z = 是否启用（0/1），
//   w = 法线方向偏移量（世界单位，已按纹素与世界尺寸换算）
struct Shadow {
    light_view_proj: mat4x4<f32>,
    params: vec4<f32>,
    // x = 阴影强度，yzw 保留
    params2: vec4<f32>,
};

@group(3) @binding(0)
var shadow_map: texture_depth_2d;

@group(3) @binding(1)
var shadow_sampler: sampler_comparison;

@group(3) @binding(2)
var<uniform> u_shadow: Shadow;

struct VsIn {
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
    @location(10) joint_base: u32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
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
    if in.weights.x + in.weights.y + in.weights.z + in.weights.w > 0.0 {
        let skinned = skin_matrix(in.joint_base, in.joints, in.weights);
        local_position = (skinned * vec4<f32>(in.position, 1.0)).xyz;
        // 蒙皮矩阵含非均匀缩放时法线要逆转置；这里用伴随矩阵近似，与下方模型矩阵一致
        local_normal = mat3x3<f32>(
            cross(skinned[1].xyz, skinned[2].xyz),
            cross(skinned[2].xyz, skinned[0].xyz),
            cross(skinned[0].xyz, skinned[1].xyz),
        ) * in.normal;
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
    out.world_normal = normalize(normal_matrix * local_normal);
    out.color = in.color;
    out.uv = in.uv;
    return out;
}

// 阶段 D：阴影通道只写深度。顶点变换与主通道一致（含线性混合蒙皮），
// 因此形变后的蒙皮网格投出的影子跟随姿势。
struct ShadowVsIn {
    @location(0) position: vec3<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) m0: vec4<f32>,
    @location(6) m1: vec4<f32>,
    @location(7) m2: vec4<f32>,
    @location(8) m3: vec4<f32>,
    @location(10) joint_base: u32,
};

// 深度 pass 专用：只含光源正交矩阵，故意不复用阴影贴图那一组绑定，
// 否则同一张纹理会在本 pass 内「既被写入又被采样」。
struct ShadowPass {
    light_view_proj: mat4x4<f32>,
    pad: vec4<f32>,
};

@group(3) @binding(3)
var<uniform> u_shadow_pass: ShadowPass;

@vertex
fn vs_shadow(in: ShadowVsIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var local_position = in.position;
    if in.weights.x + in.weights.y + in.weights.z + in.weights.w > 0.0 {
        let skinned = skin_matrix(in.joint_base, in.joints, in.weights);
        local_position = (skinned * vec4<f32>(in.position, 1.0)).xyz;
    }
    return u_shadow_pass.light_view_proj * (model * vec4<f32>(local_position, 1.0));
}

// 3×3 PCF：把深度图比较结果在邻域里平均，边缘得到两三个纹素的柔和过渡。
// 阴影贴图按「世界 → 光源裁剪空间」定位，越界或超出远平面一律按「受光」处理，
// 避免视锥外物件出现硬边黑块。
fn shadow_factor(world_position: vec3<f32>, n: vec3<f32>) -> f32 {
    if u_shadow.params.z < 0.5 {
        return 1.0;
    }
    let texel = u_shadow.params.x;
    let biased_world = world_position + n * u_shadow.params.w;
    let clip = u_shadow.light_view_proj * vec4<f32>(biased_world, 1.0);
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
            sum = sum + textureSampleCompare(shadow_map, shadow_sampler, uv + offset, cmp_depth);
        }
    }
    let visible = sum / 9.0;
    return mix(1.0, visible, u_shadow.params2.x);
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
    let n = normalize(in.world_normal);
    let l = normalize(-u_camera.light_dir.xyz);
    let v = normalize(u_camera.camera_pos.xyz - in.world_position);
    // 光方向与视线正相反时 l+v 退化，直接取法线作半程向量，避免 normalize(0) 出 NaN
    let half_sum = l + v;
    let h = select(normalize(half_sum), n, length(half_sum) < 1e-4);

    // base color：贴图（若存在）乘以材质染色因子，再乘物件颜色。
    // 贴图是 sRGB 纹理，采样后已是线性值；材质因子按线性因子直接相乘。
    var albedo = in.color.rgb * u_material.base_color.rgb;
    if u_material.params.z > 0.5 {
        let sampled = textureSample(t_base_color, s_base_color, in.uv);
        albedo = albedo * sampled.rgb;
    }
    // 反照率为负会把光"减掉"，与 CPU 侧一样按 0 处理
    albedo = max(albedo, vec3<f32>(0.0));

    let metallic = clamp(u_material.params.x, 0.0, 1.0);
    let roughness = clamp(u_material.params.y, MIN_ROUGHNESS, 1.0);

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
        let specular_ibl = env_radiance(dominant, roughness) * (f0 * ab.x + ab.y);
        ambient_term = (diffuse_ibl + specular_ibl) * u_camera.env_zenith.w;
    }

    let lit = (diffuse + specular) * radiance * ndl * shadow
        + ambient_term
        // 自发光只加不减：不乘 n·l、不受阴影影响、不参与光照计算
        + u_material.emissive.rgb;

    return vec4<f32>(lit, in.color.a * u_material.base_color.a);
}
