// 屏幕空间文本通道：单通道字形图集 + 每字形一个实例四边形。
//
// 与 3D 主通道完全解耦：独立 pipeline、独立 bind group、独立顶点布局，
// 不共享相机矩阵、深度缓冲或阴影贴图。四边形用 vertex_index 拼出，
// 因此不需要顶点缓冲，整个字形批次只有一段实例缓冲。

struct Globals {
    // xy = 视口像素尺寸；zw 保留（凑 16 字节对齐）
    screen: vec4<f32>,
};

// 字形图集：R8Unorm，只用红通道当覆盖度（alpha）。
@group(0) @binding(0)
var t_atlas: texture_2d<f32>;

@group(0) @binding(1)
var s_atlas: sampler;

@group(1) @binding(0)
var<uniform> u_globals: Globals;

struct Instance {
    // 四边形左上角（屏幕像素坐标，原点在左上）
    @location(0) origin: vec2<f32>,
    // 四边形尺寸（像素）
    @location(1) size: vec2<f32>,
    // 图集内的 uv 范围
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    // 颜色 + 全局透明度
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    let p = inst.origin + c * inst.size;
    let screen = max(u_globals.screen.xy, vec2<f32>(1.0, 1.0));
    // 像素坐标 → NDC：x 向右、y 向下，y 需要翻转
    let ndc = vec2<f32>(
        p.x / screen.x * 2.0 - 1.0,
        1.0 - p.y / screen.y * 2.0,
    );

    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(inst.uv_min, inst.uv_max, c);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // 单通道图集：红通道即覆盖度，直接当 alpha 用（直通 alpha 混合）
    let coverage = textureSample(t_atlas, s_atlas, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
