// 地形通道着色器（M2-C03b）。
//
// 这是**独立管线**：顶点入口是 `vs_terrain`、片元入口是 `fs_terrain`，与 PBR 的
// `vs_main` / `fs_main` 完全不共享顶点缓冲布局。地形顶点只带 position / normal /
// uv / 4 层权重，不带关节、不带切线、不做实例化——把它硬塞进 PBR 顶点布局的代价是
// 动每一个既有着色器入口，收益为零。
//
// 着色口径：4 层材质颜色按逐顶点权重加权混合，然后走最朴素的 lambert 直射 + 半球环境。
// 这里刻意不做 PBR 的粗糙度/金属度/IBL：地形是背景大面，观感靠颜色层次而非高光细节，
// 而且不采样环境贴图意味着这条管线自己就能跑起来，不依赖 PBR 的 bind group。

struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x = 直射光强度，y = 环境强度，z/w 保留
    light: vec4<f32>,
    sun_dir: vec4<f32>,
};

struct Layers {
    // 4 层基色，逐层 rgb + 未用 alpha
    base: array<vec4<f32>, 4>,
    // x = 环境半球上色系数，y = 直射系数，z/w 保留
    shaping: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> layers: Layers;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights: vec4<f32>,
};

@vertex
fn vs_terrain(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.world = in.position;
    out.normal = in.normal;
    out.uv = in.uv;
    out.weights = in.weights;
    return out;
}

fn mix_layers(weights: vec4<f32>) -> vec3<f32> {
    var color = vec3<f32>(0.0);
    // 权重由 CPU 侧归一化过，这里不再除和：shader 里多一次除法不会更安全，
    // 只会让「权重和不为 1」这类 bug 在 GPU 上被悄悄吃掉。
    color += layers.base[0].rgb * weights.x;
    color += layers.base[1].rgb * weights.y;
    color += layers.base[2].rgb * weights.z;
    color += layers.base[3].rgb * weights.w;
    return color;
}

@fragment
fn fs_terrain(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let to_sun = normalize(-camera.sun_dir.xyz);
    // 半 lambert：地形法线朝下（背光面）不该全黑，抬一手衰减下限
    let diffuse = max(dot(n, to_sun), 0.0) * 0.5 + 0.5;
    let ambient = n.y * 0.5 + 0.5;

    let base = mix_layers(in.weights);
    let lit = base
        * (camera.light.y * (0.4 + 0.6 * ambient) + camera.light.x * diffuse);
    return vec4<f32>(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
