// 基础前向渲染：单方向光 + 环境光 + Blinn-Phong 高光。
// 每个物件通过实例顶点缓冲传入模型矩阵与颜色。

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u_camera: Camera;

// P2：每材质一组 —— base color 染色 + PBR 标量 + 可选贴图。
struct Material {
    base_color: vec4<f32>,
    // x = metallic，y = roughness，z = 是否有贴图，w 保留
    params: vec4<f32>,
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
fn skin_matrix(in: VsIn) -> mat4x4<f32> {
    let base = in.joint_base;
    var m = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    if in.weights.x > 0.0 {
        m += u_joints[base + in.joints.x] * in.weights.x;
    }
    if in.weights.y > 0.0 {
        m += u_joints[base + in.joints.y] * in.weights.y;
    }
    if in.weights.z > 0.0 {
        m += u_joints[base + in.joints.z] * in.weights.z;
    }
    if in.weights.w > 0.0 {
        m += u_joints[base + in.joints.w] * in.weights.w;
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
        let skinned = skin_matrix(in);
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(-u_camera.light_dir.xyz);
    let v = normalize(u_camera.camera_pos.xyz - in.world_position);
    let h = normalize(l + v);

    // base color：贴图（若存在）乘以材质染色因子，再乘物件颜色。
    // 贴图是 sRGB 纹理，采样后已是线性值；材质因子按线性因子直接相乘。
    var albedo = in.color.rgb * u_material.base_color.rgb;
    if u_material.params.z > 0.5 {
        let sampled = textureSample(t_base_color, s_base_color, in.uv);
        albedo = albedo * sampled.rgb;
    }

    // 粗糙度影响高光锐度：粗糙 → 更散；金属度提高高光占比。
    let roughness = clamp(u_material.params.y, 0.05, 1.0);
    let metallic = clamp(u_material.params.x, 0.0, 1.0);
    let shininess = mix(96.0, 8.0, roughness);
    let spec_strength = mix(0.25, 0.9, metallic);

    let ndl = max(dot(n, l), 0.0);
    let spec = pow(max(dot(n, h), 0.0), shininess) * spec_strength * step(0.001, ndl);

    let lit = albedo
        * (u_camera.ambient.rgb + u_camera.light_color.rgb * u_camera.light_color.w * ndl)
        + vec3<f32>(spec);

    return vec4<f32>(lit, in.color.a * u_material.base_color.a);
}
