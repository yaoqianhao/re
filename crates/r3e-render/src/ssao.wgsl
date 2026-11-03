// SSAO（扩展十七）的 GPU 侧：深度 → 遮蔽图 → 4×4 散焦。
//
// 公式的权威实现在 `r3e_core::ssao`（`kernel_dir` / `kernel_scale` /
// `reconstruct_view_pos` / `is_occluded` / `ao_from_occlusion`），
// 这里只做搬运；二者由 `tests/ssao_shader.rs` 的常量与函数体比对锁住。
//
// 与 CPU 参考实现的唯一差别：采样点是否在屏幕内、以及在屏外时直接跳过
// （CPU 侧没有屏幕概念）。判定条件本身（法线半球 / 半径区间 / 偏置）
// 与 `is_occluded` 逐条对应。

struct SsaoParams {
    // 相机投影与逆投影（rh，DirectX 风格 NDC：深度 0..1、y 向上）
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    // 半球核：与 `kernel_uniform` 同序，未用槽位为 0
    kernel: array<vec4<f32>, 16>,
    // xy = 1/尺寸；z = 半径；w = 偏置
    screen: vec4<f32>,
    // x = 强度；y = 样本数
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: SsaoParams;

// 生成 pass 的输入：主通道的深度缓冲
@group(1) @binding(0) var depth_tex: texture_depth_2d;

// 散焦 pass 的输入：刚生成的遮蔽图（与上面同一个 group/binding 位置，
// 但两个 pass 用各自的绑定组布局，所以不会同时挂在一张纹理上）
@group(1) @binding(1) var src_ao: texture_2d<f32>;

// 全屏三角形：顶点由 vertex_index 现算，不需要顶点缓冲。
struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// 深度 + 屏幕 uv → 视空间位置（与 `reconstruct_view_pos` 等价）。
// wgpu 的 uv 原点在左上、NDC y 向上，故 y 要翻一次。
fn view_pos(depth: f32, uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let p = params.inv_proj * vec4<f32>(ndc, depth, 1.0);
    if (abs(p.w) < 1e-6) {
        return vec3<f32>(0.0);
    }
    return p.xyz / p.w;
}

fn uv_of(coord: vec2<i32>, size: vec2<f32>) -> vec2<f32> {
    return (vec2<f32>(coord) + vec2<f32>(0.5)) / size;
}

// 生成 pass：每个像素数一遍半球核里的遮挡比例，写成遮蔽系数。
@fragment
fn fs_ssao(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size_f = vec2<f32>(textureDimensions(depth_tex));
    let size_i = vec2<i32>(size_f);
    let max_i = size_i - vec2<i32>(1, 1);
    let coord = clamp(vec2<i32>(frag.xy), vec2<i32>(0, 0), max_i);
    let depth = textureLoad(depth_tex, coord, 0);

    // 远平面（天空 / 背景）：没有几何，遮蔽系数恒为 1。
    if (depth >= 1.0) {
        return vec4<f32>(1.0);
    }

    let pos = view_pos(depth, uv_of(coord, size_f));

    // 法线由左右 / 上邻域的位置叉乘得到（与 `normal_from_neighbors` 同法），
    // 再按「朝向相机」掰正符号：相机在视空间原点，故法线应指向 -pos。
    let c_r = clamp(coord + vec2<i32>(1, 0), vec2<i32>(0, 0), max_i);
    let c_u = clamp(coord + vec2<i32>(0, 1), vec2<i32>(0, 0), max_i);
    let depth_r = textureLoad(depth_tex, c_r, 0);
    let depth_u = textureLoad(depth_tex, c_u, 0);
    let pos_r = view_pos(depth_r, uv_of(c_r, size_f));
    let pos_u = view_pos(depth_u, uv_of(c_u, size_f));
    let cross_n = cross(pos_r - pos, pos_u - pos);
    if (length(cross_n) < 1e-6) {
        // 退化（邻域共线 / 深度不可用）：不做遮蔽，避免刷出黑边。
        return vec4<f32>(1.0);
    }
    var n = normalize(cross_n);
    if (dot(n, -pos) < 0.0) {
        n = -n;
    }

    // 以法线为 z 轴建一组正交基，把核从「法线半球」旋到世界方向。
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let tangent = normalize(cross(helper, n));
    let bitangent = cross(n, tangent);

    let radius = params.screen.z;
    let bias = params.screen.w;
    let count = i32(params.misc.y);
    var occluded = 0.0;

    for (var i = 0; i < count; i = i + 1) {
        let k = params.kernel[i].xyz;
        let s = pos + tangent * k.x + bitangent * k.y + n * k.z;
        let clip = params.proj * vec4<f32>(s, 1.0);
        if (clip.w <= 0.0) {
            continue;
        }
        let ndc = clip.xyz / clip.w;
        let suv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
            continue;
        }
        let sc = clamp(vec2<i32>(suv * size_f), vec2<i32>(0, 0), max_i);
        let sd = textureLoad(depth_tex, sc, 0);
        if (sd >= 1.0) {
            continue;
        }
        let sp = view_pos(sd, suv);
        // 与 `is_occluded` 三条判定逐条对应：法线半球 → 半径区间 → 偏置
        let delta = sp - pos;
        if (dot(delta, n) <= 0.0) {
            continue;
        }
        let dist = length(delta);
        if (dist <= bias || dist >= radius) {
            continue;
        }
        occluded = occluded + 1.0;
    }

    let ratio = select(0.0, occluded / f32(count), count > 0);
    // 与 `ao_from_occlusion` 等价
    let ao = clamp(1.0 - ratio * params.misc.x, 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

// 散焦 pass：4×4 盒式平均，压掉单像素噪声（不做双边加权——
// 深度引导的双边模糊要再读一遍深度，收益在这个尺度上小于成本）。
@fragment
fn fs_blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size_i = vec2<i32>(textureDimensions(src_ao));
    let max_i = size_i - vec2<i32>(1, 1);
    let base = vec2<i32>(frag.xy) - vec2<i32>(2, 2);
    var sum = 0.0;
    for (var y = 0; y < 4; y = y + 1) {
        for (var x = 0; x < 4; x = x + 1) {
            let c = clamp(base + vec2<i32>(x, y), vec2<i32>(0, 0), max_i);
            sum = sum + textureLoad(src_ao, c, 0).r;
        }
    }
    let ao = sum / 16.0;
    return vec4<f32>(ao, ao, ao, 1.0);
}
