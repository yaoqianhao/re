// 屏幕空间 2D UI 通道着色器（M1 最小 UI）。
//
// 一个实例 = 一个四边形（矩形 / 边框的一条边 / 一个图片块），顶点位置由
// vertex_index 按单位方块的 6 个角推出来，不传顶点缓冲，只传实例缓冲。
// 颜色混合用 straight alpha（与文本通道一致），不写深度，整通道以 LoadOp::Load
// 叠在已画好的画面上。

struct Instance {
    @location(0) origin: vec2<f32>,   // 左上角（屏幕像素）
    @location(1) size: vec2<f32>,     // 像素宽高
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
    @location(4) color: vec4<f32>,    // 已乘过元素 alpha
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 到矩形边缘的有符号距离（像素，内部为负）：用来做 1px 抗锯齿边缘。
    // 现在圆角恒为 0，将来加圆角只需要在下面 sdf 里改 radius。
    @location(2) dist: f32,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

@group(1) @binding(0) var<uniform> screen: vec4<f32>;  // xy = 屏幕像素尺寸

// 单位方块的两个三角形（局部坐标 0..1）
const CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 1.0),
);

// 圆角矩形有符号距离场（radius = 0 即普通矩形）
fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius, radius);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = CORNERS[vi];
    let local = corner * inst.size;
    let px = inst.origin + local;

    // 像素坐标 → NDC（y 向下翻成 y 向上）
    let ndc = vec2<f32>(
        px.x / screen.x * 2.0 - 1.0,
        1.0 - px.y / screen.y * 2.0,
    );

    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(inst.uv_min, inst.uv_max, corner);
    out.color = inst.color;

    let half_size = inst.size * 0.5;
    let p = (corner - vec2<f32>(0.5, 0.5)) * inst.size;
    out.dist = rounded_box(p, half_size, 0.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(tex, tex_sampler, in.uv);
    let tint = in.color * texel;

    // 边缘 1px 过渡：内部 dist < 0 → alpha 1，外部 → 0
    let aa = 0.75;
    let coverage = 1.0 - smoothstep(-aa, aa, in.dist);
    let alpha = clamp(tint.a * coverage, 0.0, 1.0);
    return vec4<f32>(tint.rgb, alpha);
}
