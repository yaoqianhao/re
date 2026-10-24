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

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // M1 起顶点带 UV（uv 原点左上，v 已由资产层做 v' = 1 - v 翻转），
    // 本阶段仅透传到片元着色器，尚未接入采样（贴图绑定在后续阶段）。
    @location(2) uv: vec2<f32>,
    // 实例矩阵按列传入
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let world = model * vec4<f32>(in.position, 1.0);

    // 非均匀缩放下严格做法是用逆转置；此处用伴随矩阵近似，均匀缩放时精确。
    let normal_matrix = mat3x3<f32>(
        cross(model[1].xyz, model[2].xyz),
        cross(model[2].xyz, model[0].xyz),
        cross(model[0].xyz, model[1].xyz),
    );

    var out: VsOut;
    out.clip_position = u_camera.view_proj * world;
    out.world_position = world.xyz;
    out.world_normal = normalize(normal_matrix * in.normal);
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
