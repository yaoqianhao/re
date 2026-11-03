// HDR 后处理链（扩展十三）：亮部提取 → 三级降采样泛光 → 上采样累加 → composite。
//
// 所有 pass 都用「覆盖全屏的大三角形」，不建顶点缓冲：顶点位置由 vertex_index
// 现算，因此六个 pass 共享同一个顶点入口，只有片元入口与渲染目标不同。
//
// 数值契约与 `r3e-core/src/postfx.rs` 一一对应（ACES 系数、FXAA 双阈值、
// 软膝公式），`crates/r3e-render/tests/shader_wgsl.rs` 用常量比对锁死两边，
// 改 CPU 参考实现必须同步改这里。

struct Params {
    // ---- composite ----
    exposure_scale: f32,
    tonemap: u32,
    bloom_intensity: f32,
    bloom_enabled: u32,
    fxaa_enabled: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // ---- 全链共用 ----
    hdr_texel: vec2<f32>,
    threshold: f32,
    knee: f32,
    src_texel: vec2<f32>,
    _pad3: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;

// 主输入：泛光 pass 是上一级纹理，composite 是场景 HDR 目标。
@group(1) @binding(0) var t_src: texture_2d<f32>;
@group(1) @binding(1) var s_src: sampler;
// 副输入：只有 composite 用（泛光结果）。泛光 pass 会把它绑成与主输入同一张图，
// 布局统一的好处是六个 pass 共用一份 bind group layout。
@group(1) @binding(2) var t_aux: texture_2d<f32>;
@group(1) @binding(3) var s_aux: sampler;
// 遮蔽图（扩展十七）：只有 composite 读，泛光 pass 绑的是 1×1 白图。
@group(1) @binding(4) var t_ao: texture_2d<f32>;

// ACES 拟合系数（Narkowicz 2015），与 core::postfx 的 ACES_A..ACES_E 同值。
const ACES_A: f32 = 2.51;
const ACES_B: f32 = 0.03;
const ACES_C: f32 = 2.43;
const ACES_D: f32 = 0.59;
const ACES_E: f32 = 0.14;

// FXAA 双阈值：绝对 1/32 压暗部噪点，相对 1/8 避免把亮区渐变当边缘。
const FXAA_EDGE_THRESHOLD_MIN: f32 = 0.03125;
const FXAA_EDGE_THRESHOLD_RELATIVE: f32 = 0.125;

// Rec.709 亮度权重。
const LUMA_WEIGHTS: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> VsOut {
    // 单三角形覆盖 viewport：(-1,-1) (3,-1) (-1,3)，屏幕外由光栅化裁掉。
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = corners[index];
    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    // 纹理 uv：左上原点，与场景背景的书写方向一致
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, 1.0 - (p.y + 1.0) * 0.5);
    return out;
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, LUMA_WEIGHTS);
}

/// 亮部提取（软膝），与 `core::postfx::bright_pass` 逐行等价。
///
/// 比例按「最亮通道」算，三通道同乘：这样 1.5 的蓝分量不会被阈值单独削掉，
/// 光晕保持原色相，不会出现红色高光周围一圈纯红。
fn bright_pass(color: vec3<f32>, threshold: f32, knee: f32) -> vec3<f32> {
    let c = max(color, vec3<f32>(0.0));
    let brightness = max(c.x, max(c.y, c.z));
    if (brightness <= 1e-6) {
        return vec3<f32>(0.0);
    }
    var contribution = 0.0;
    if (knee <= 1e-4) {
        contribution = max(brightness - threshold, 0.0) / brightness;
    } else {
        let ramp = clamp(brightness - threshold + knee, 0.0, 2.0 * knee);
        let soft = ramp * ramp / (4.0 * knee);
        let hard = max(brightness - threshold, 0.0);
        contribution = max(soft, hard) / brightness;
    }
    return c * clamp(contribution, 0.0, 1.0);
}

/// 曝光 + 色调映射，与 `core::postfx::tonemap(expose(..), mode)` 等价。
/// `tonemap`: 0 = clamp / 1 = Reinhard / 2 = ACES。
fn tonemap_color(color: vec3<f32>, exposure_scale: f32, mode: u32) -> vec3<f32> {
    let c = max(color, vec3<f32>(0.0)) * exposure_scale;
    if (mode == 1u) {
        return c / (vec3<f32>(1.0) + c);
    }
    if (mode == 2u) {
        let num = c * (vec3<f32>(ACES_A) * c + vec3<f32>(ACES_B));
        let den = c * (vec3<f32>(ACES_C) * c + vec3<f32>(ACES_D)) + vec3<f32>(ACES_E);
        return clamp(num / den, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // clamp：8 位 sRGB 目标上超过 1.0 的部分本来就会被切平，参考实现保持一致
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

/// 局部对比度是否够得上「边缘」，与 `core::postfx::fxaa_edge_visible` 同源。
fn edge_visible(luma_min: f32, luma_max: f32) -> bool {
    let contrast = luma_max - luma_min;
    let floor = max(FXAA_EDGE_THRESHOLD_MIN, luma_max * FXAA_EDGE_THRESHOLD_RELATIVE);
    return contrast >= floor;
}

/// composite 的单个像素：`(hdr + bloom·强度) → 曝光 → 色调映射`。
///
/// 泛光在**曝光之前**叠加：两者都是辐亮度，先加再曝光才等价于
/// 「同一台相机换一档曝光」，而不是「先把泛光调好再当 LDR 贴图叠加」。
fn composite_color(uv: vec2<f32>) -> vec3<f32> {
    var c = textureSampleLevel(t_src, s_src, uv, 0.0).rgb;
    // 遮蔽压的是直接光。泛光源自散射，遮它等于让暗角把光晕也吃掉，
    // 所以顺序是「先乘 AO、再叠泛光」。AO 关闭时这里采样到 1.0，等于没乘。
    c = c * textureSampleLevel(t_ao, s_src, uv, 0.0).r;
    if (params.bloom_enabled != 0u) {
        c = c + textureSampleLevel(t_aux, s_aux, uv, 0.0).rgb * params.bloom_intensity;
    }
    return tonemap_color(c, params.exposure_scale, params.tonemap);
}

/// pass 1：HDR 目标 → 半分辨率亮部。阈值以下直接出黑，泛光只由真高光驱动。
@fragment
fn fs_prefilter(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = textureSampleLevel(t_src, s_src, in.uv, 0.0).rgb;
    return vec4<f32>(bright_pass(hdr, params.threshold, params.knee), 1.0);
}

/// pass 2..4：逐级降采样（4 抽头盒式）。缩小本身即低通，
/// 光晕因此天然柔化，不必再单独做高斯卷积。
@fragment
fn fs_downsample(in: VsOut) -> @location(0) vec4<f32> {
    let t = params.src_texel;
    var sum = textureSampleLevel(t_src, s_src, in.uv + vec2<f32>(-t.x, -t.y), 0.0).rgb;
    sum = sum + textureSampleLevel(t_src, s_src, in.uv + vec2<f32>(t.x, -t.y), 0.0).rgb;
    sum = sum + textureSampleLevel(t_src, s_src, in.uv + vec2<f32>(-t.x, t.y), 0.0).rgb;
    sum = sum + textureSampleLevel(t_src, s_src, in.uv + vec2<f32>(t.x, t.y), 0.0).rgb;
    return vec4<f32>(sum * 0.25, 1.0);
}

/// pass 5..6：上采样累加（混合状态 One/One 加法）。
/// 把粗层的宽光晕叠回细层，得到「近处紧、远处散」的层次。
@fragment
fn fs_upsample(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(t_src, s_src, in.uv, 0.0).rgb, 1.0);
}

/// pass 7：composite —— 写回 sRGB 表面，文本 / UI 通道在它之后叠加。
@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    var mapped = composite_color(in.uv);

    if (params.fxaa_enabled != 0u) {
        // 十字五抽样：中心 + 上下左右。边缘判定用「显示空间」的亮度——
        // HDR 域里 20.0 与 21.0 的差在屏幕上根本看不出来，不能当边缘。
        let tx = params.hdr_texel.x;
        let ty = params.hdr_texel.y;
        let left = composite_color(in.uv - vec2<f32>(tx, 0.0));
        let right = composite_color(in.uv + vec2<f32>(tx, 0.0));
        let up = composite_color(in.uv - vec2<f32>(0.0, ty));
        let down = composite_color(in.uv + vec2<f32>(0.0, ty));
        let l_center = luminance(mapped);
        let l_min = min(
            min(min(luminance(left), luminance(right)), min(luminance(up), luminance(down))),
            l_center,
        );
        let l_max = max(
            max(max(luminance(left), luminance(right)), max(luminance(up), luminance(down))),
            l_center,
        );
        if (edge_visible(l_min, l_max)) {
            // 取对比更强的那条轴做一次半程内插：FXAA 的极简版——
            // 不做梯度搜索，只消一层台阶，代价是 4 次额外采样，
            // 换掉整条 MSAA 通路。
            var average = mapped;
            if (abs(luminance(left) - luminance(right)) >= abs(luminance(up) - luminance(down))) {
                average = (left + right) * 0.5;
            } else {
                average = (up + down) * 0.5;
            }
            mapped = mix(mapped, average, 0.5);
        }
    }
    return vec4<f32>(mapped, 1.0);
}
