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

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // 实例矩阵按列传入
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
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
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(-u_camera.light_dir.xyz);
    let v = normalize(u_camera.camera_pos.xyz - in.world_position);
    let h = normalize(l + v);

    let ndl = max(dot(n, l), 0.0);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.35 * step(0.001, ndl);

    let lit = in.color.rgb
        * (u_camera.ambient.rgb + u_camera.light_color.rgb * u_camera.light_color.w * ndl)
        + vec3<f32>(spec);

    return vec4<f32>(lit, in.color.a);
}
