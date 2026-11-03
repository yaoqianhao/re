//! 单方向光 + 环境光（Blinn-Phong 光照模型），含方向光阴影配置（阶段 D）。

use crate::camera::Camera;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

/// 方向光阴影贴图配置（阶段 D）。
///
/// 阴影相机是一台沿 `Light::direction` 观察场景包围球的正交相机：
/// `ortho_size == 0` 时半宽按包围球每帧自适应，大于 0 时固定为该半宽
/// （适合只看某个兴趣区域，或减小贴图拉伸）。
#[derive(Debug, Clone, Copy)]
pub struct ShadowSettings {
    /// 是否启用阴影。
    pub enabled: bool,
    /// 阴影贴图分辨率（正方形，像素）。规范化后夹到 [64, 8192]。
    pub size: u32,
    /// 正交相机半宽（世界单位）。0 = 按场景包围球自适应。
    pub ortho_size: f32,
    /// 深度比较偏移，抑制自阴影条纹（acne）。
    pub bias: f32,
    /// 法线方向偏移（单位：纹素），斜射面抗 acne 且比纯深度偏移更少漂浮。
    pub normal_bias: f32,
    /// 光源相机后退距离 = 包围球半径 × distance + 1。
    pub distance: f32,
    /// 阴影强度 0..1：1 = 完全遮挡，0 = 无阴影。
    pub strength: f32,
    /// 级联阴影（扩展十六）。`cascade.count == 1` 时退化为阶段 D 的单张全场景阴影图。
    pub cascade: CascadeSettings,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 2048,
            ortho_size: 0.0,
            bias: 0.0015,
            normal_bias: 1.5,
            distance: 2.0,
            strength: 1.0,
            cascade: CascadeSettings::default(),
        }
    }
}

impl ShadowSettings {
    /// 规范化：把分辨率与各标量夹到可用区间，避免脚本/场景传入坏值。
    pub fn sanitized(mut self) -> Self {
        self.size = self.size.clamp(64, 8192);
        if !self.ortho_size.is_finite() || self.ortho_size < 0.0 {
            self.ortho_size = 0.0;
        }
        self.bias = if self.bias.is_finite() {
            self.bias.clamp(0.0, 0.05)
        } else {
            0.0
        };
        self.normal_bias = if self.normal_bias.is_finite() {
            self.normal_bias.clamp(0.0, 8.0)
        } else {
            0.0
        };
        self.distance = if self.distance.is_finite() {
            self.distance.clamp(0.05, 16.0)
        } else {
            2.0
        };
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.cascade = self.cascade.sanitized();
        self
    }
}

// ---------------------------------------------------------------------------
// 扩展十六：级联阴影（CSM）
// ---------------------------------------------------------------------------
//
// 阶段 D 只有一张覆盖「整个场景包围球」的正交阴影图：视距一拉远，所有纹素都被摊到
// 几十米上，近处物件立刻出锯齿；视距拉近又浪费分辨率。级联把相机视锥按深度切成
// 若干段，每段一张同分辨率的图，近段纹素密度高、远段只求覆盖。
//
// 这一层同样遵守「core 只放纯函数」：切分点、子视锥角点、光源正交相机拟合全部是
// 纯计算，零 wgpu 依赖，GPU 侧只照着矩阵与切分点做选择，不再重复推导。

/// 级联数上限。渲染层纹理数组的层数、shader 里定长数组的长度都等于它。
pub const MAX_CASCADES: usize = 4;

/// 级联阴影配置（扩展十六）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeSettings {
    /// 级联数 1..=4。1 = 单级（等价阶段 D 的全场景阴影图）。
    pub count: u32,
    /// 切分混合系数：0 = 纯均匀切分（远处的段很长），1 = 纯对数切分（近处更密）。
    pub lambda: f32,
    /// 级间过渡带宽（占该级跨度的比例，0 = 硬切，越大接缝越柔）。
    pub blend: f32,
    /// 阴影最远距离（世界单位，沿相机前后方向）。0 = 用相机 `zfar`。
    pub max_distance: f32,
}

impl Default for CascadeSettings {
    fn default() -> Self {
        Self {
            // 默认单级：老场景、老测试的画面与统计口径完全不变，要 CSM 就显式开
            count: 1,
            lambda: 0.6,
            blend: 0.1,
            max_distance: 0.0,
        }
    }
}

impl CascadeSettings {
    /// 规范化：级数夹到 1..=4，比例与距离收敛到有限区间。
    pub fn sanitized(mut self) -> Self {
        self.count = self.count.clamp(1, MAX_CASCADES as u32);
        self.lambda = if self.lambda.is_finite() {
            self.lambda.clamp(0.0, 1.0)
        } else {
            0.6
        };
        self.blend = if self.blend.is_finite() {
            self.blend.clamp(0.0, 0.5)
        } else {
            0.1
        };
        self.max_distance = if self.max_distance.is_finite() && self.max_distance > 0.0 {
            self.max_distance.clamp(0.5, 1.0e5)
        } else {
            0.0
        };
        self
    }

    /// 规范化后的级数（usize，便于当数组长度用）。
    pub fn levels(&self) -> usize {
        self.sanitized().count as usize
    }
}

/// 一台相机的可见子视锥的 8 个世界角点（先近平面 4 个，再远平面 4 个）。
///
/// 顺序：近平面 (-x,-y) (+x,-y) (+x,+y) (-x,+y)，远平面同序。
pub fn frustum_slice_corners(camera: &Camera, near: f32, far: f32) -> [Vec3; 8] {
    // 近远平面夹到合法区间：near 必须是正数，far 不得小于 near
    let near = if near.is_finite() && near > 1e-4 {
        near
    } else {
        1e-3
    };
    let far = if far.is_finite() && far > near {
        far
    } else {
        near + 1.0
    };
    // 相机基：与 `Camera::view` 同一套 look_at 约定（-z 为前）
    let view = camera.view();
    let inv = view.inverse();
    let right = inv.x_axis.truncate().normalize_or_zero();
    let up = inv.y_axis.truncate().normalize_or_zero();
    let forward = -inv.z_axis.truncate().normalize_or_zero();
    let right = if right.length_squared() > 0.5 {
        right
    } else {
        Vec3::X
    };
    let up = if up.length_squared() > 0.5 {
        up
    } else {
        Vec3::Y
    };
    let forward = if forward.length_squared() > 0.5 {
        forward
    } else {
        Vec3::NEG_Z
    };
    let tan_half_v = (camera.fov_deg.clamp(1.0, 179.0) * 0.5).to_radians().tan();
    let tan_half_h = tan_half_v * camera.aspect.max(1e-4);
    let mut out = [Vec3::ZERO; 8];
    for (i, d) in [near, far].into_iter().enumerate() {
        let center = camera.position + forward * d;
        let hx = right * (tan_half_h * d);
        let hy = up * (tan_half_v * d);
        out[i * 4] = center - hx - hy;
        out[i * 4 + 1] = center + hx - hy;
        out[i * 4 + 2] = center + hx + hy;
        out[i * 4 + 3] = center - hx + hy;
    }
    out
}

/// 级联切分点（沿相机深度的距离，单调递增，长度 = 级数 + 1，首元素是 `znear`）。
///
/// 采用 practical split scheme：均匀切分与对数切分按 `lambda` 混合，
/// 近处因此拿到更短的段长、更密的纹素。
pub fn cascade_splits(camera: &Camera, cascade: CascadeSettings) -> Vec<f32> {
    let c = cascade.sanitized();
    let levels = c.levels();
    let near = if camera.znear.is_finite() && camera.znear > 1e-4 {
        camera.znear
    } else {
        1e-3
    };
    let far_limit = if c.max_distance > 0.0 {
        c.max_distance
    } else if camera.zfar.is_finite() && camera.zfar > near {
        camera.zfar
    } else {
        near + 100.0
    };
    let far = far_limit.max(near * 1.001);
    let ratio = far / near;
    let mut splits = Vec::with_capacity(levels + 1);
    splits.push(near);
    for i in 1..=levels {
        let f = i as f32 / levels as f32;
        let log_split = near * ratio.powf(f);
        let uniform_split = near + (far - near) * f;
        let d = uniform_split * (1.0 - c.lambda) + log_split * c.lambda;
        // 浮点误差可能让相邻切分点贴死，强制留出一点点间隔保证严格递增
        let prev = splits[i - 1];
        let floor = prev + (far - near) * 1e-4;
        splits.push(d.clamp(floor, far).max(floor));
    }
    // 最后一级必须精确落在 far 上，边界不要飘
    if let Some(last) = splits.last_mut() {
        *last = far;
    }
    splits
}

/// 单级阴影相机（光源正交相机）的拟合结果。
#[derive(Debug, Clone, Copy)]
pub struct CascadeFit {
    /// 光源正交相机的 view-projection（右乘、NDC 深度 0..1，与 wgpu 一致）。
    pub view_proj: Mat4,
    /// 正交半宽（世界单位）。纹素尺寸 = `2 * radius / resolution`。
    pub radius: f32,
    /// 该级切片的中心（世界坐标），调试与文档用。
    pub center: Vec3,
}

/// 把一组世界角点拟合成沿 `light_dir` 观察的光源正交相机。
///
/// 关键处理：光空间 AABB 用**正方形**外接（x/y 取同一个半宽，保证纹素各向同性），
/// 中心再量化到纹素网格上。量化这一步是 CSM 的命门——相机缓慢平移时若每帧都
/// 重新取中心，整张阴影图的采样位置会随亚纹素抖动，表现为边缘「爬行」。
pub fn fit_cascade(light_dir: Vec3, corners: &[Vec3; 8], resolution: u32) -> CascadeFit {
    let dir = if light_dir.length_squared() > 1e-8 {
        light_dir.normalize()
    } else {
        Vec3::NEG_Y
    };
    // 光源接近竖直时换 Z 轴当 up，避免 look_at 退化
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    // 只取旋转（eye 在原点，translation 为 0）：世界 → 光相机空间
    let rot = glam::camera::rh::view::look_at_mat4(Vec3::ZERO, dir, up);

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for c in corners {
        let p = rot.transform_point3(*c);
        min = min.min(p);
        max = max.max(p);
    }
    if !min.is_finite() || !max.is_finite() {
        return CascadeFit {
            view_proj: Mat4::IDENTITY,
            radius: 1.0,
            center: Vec3::ZERO,
        };
    }

    let res = resolution.max(1) as f32;
    let extent = (max - min).max(Vec3::splat(1e-3));
    // 正方形半宽：取 x/y 里更大的那半，再加 2% 余量防边缘裁切
    let half = (extent.x.max(extent.y) * 0.5 * 1.02).max(1e-3);
    let texel = 2.0 * half / res;

    let center_local = (min + max) * 0.5;
    // 中心量化到纹素网格（只量化 x/y，深度方向由 min/max 决定，不需要量化）
    let cx = (center_local.x / texel).round() * texel;
    let cy = (center_local.y / texel).round() * texel;

    // 光相机沿 `dir` 观察，视空间的「前」是 -z，所以最近点的深度 = -max.z、最远 = -min.z。
    // 把最近点搬到 depth = 0.05 处（贴住近平面的余量），最远点自然落在 far 之前。
    let depth_near = -max.z;
    let depth_far = -min.z;
    let shift = depth_near - 0.05;
    let span = ((depth_far - depth_near) + 0.2).max(0.1);

    // view：先转到光空间，再把 (cx, cy) 与深度起点搬到原点附近
    let view = Mat4::from_translation(Vec3::new(-cx, -cy, shift)) * rot;
    let proj =
        glam::camera::rh::proj::directx::orthographic(-half, half, -half, half, 0.05, 0.05 + span);

    // 世界坐标下的切片中心（把光空间里的量化中心搬回世界）
    let center = rot
        .inverse()
        .transform_point3(Vec3::new(cx, cy, center_local.z));

    CascadeFit {
        view_proj: proj * view,
        radius: half,
        center,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Light {
    /// 平行光方向（从光源射出的方向）。
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
    pub ambient: Vec3,
    /// 方向光阴影配置（阶段 D）。
    pub shadow: ShadowSettings,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.45, -0.8, -0.4).normalize(),
            color: Vec3::new(1.0, 0.97, 0.92),
            intensity: 1.0,
            ambient: Vec3::new(0.18, 0.19, 0.23),
            shadow: ShadowSettings::default(),
        }
    }
}

impl Light {
    /// 阴影相机的 view-projection（正交、右乘、NDC 深度 0..1，与 wgpu 一致）。
    ///
    /// `center` / `radius` 为场景包围球；`radius <= 0` 时退回 1.0。
    /// 光源放在球心沿 `-direction` 方向 `radius × distance + 1` 处，
    /// 远平面留足球体前后余量，因此光源换向时不需要重算包围盒。
    pub fn shadow_view_projection(&self, center: Vec3, radius: f32) -> Mat4 {
        let s = self.shadow.sanitized();
        let radius = if radius.is_finite() && radius > 1e-4 {
            radius
        } else {
            1.0
        };
        let dir = if self.direction.length_squared() > 1e-8 {
            self.direction.normalize()
        } else {
            Vec3::NEG_Y
        };
        let half = if s.ortho_size > 0.0 {
            s.ortho_size
        } else {
            radius * 1.05
        };
        let back = radius * s.distance + 1.0;
        let eye = center - dir * back;
        // 光源接近竖直时换 Z 轴当 up，避免 look_at 退化
        let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        // glam 0.34 起相机辅助函数迁到 glam::camera::rh 命名空间：
        // 与 Camera 保持同一套右手系 + directx(NDC 深度 0..1) 约定，才和 wgpu 对齐
        let view = glam::camera::rh::view::look_at_mat4(eye, center, up);
        let far = back + radius * 2.0 + 1.0;
        let proj =
            glam::camera::rh::proj::directx::orthographic(-half, half, -half, half, 0.05, far);
        proj * view
    }
}

// ---------------------------------------------------------------------------
// 扩展十五：局部光源（点光 / 聚光）
// ---------------------------------------------------------------------------
//
// 与方向光（`Light`）并存：方向光负责全局主光 + 阴影，局部光源负责场景里的灯泡、
// 台灯、手电这类「有位置、有衰减、有方向锥」的光。两者互不替代，最终都叠在同一个
// `fs_main` 里求出射辐亮度。
//
// 这一层的原则与 UI / 环境探针一致：**core 只放纯函数与数据结构**，零 wgpu 依赖；
// 数值收敛（NaN / 负值 / 天文数字）全部在 `sanitized()` 里一次做完，GPU 侧直接照着
// 这里的公式实现，`tests/local_lights_shader.rs` 用 naga 解析 wgsl 比对常量防漂移。

/// 单帧最多参与计算的局部光源数。
///
/// 数字同时被三处引用，改这里必须三处同频：
/// 1. `Scene::upsert_local_light` 的容量上限（超出的新增被拒绝）；
/// 2. 渲染层 `LightsUniform` 的定长数组；
/// 3. `shader.wgsl` 里 `array<LocalLight, N>` 声明（有测试锁死）。
///
/// 取 16 的理由：前向渲染每个片元都要遍历一遍，16 盏灯 + 早退（距离/背面）在
/// 1080p 下仍是可控开销；再多就该上分块光照（clustered）了。
pub const MAX_LOCAL_LIGHTS: usize = 16;

/// 衰减距离下限（世界单位）。太小会让「光源贴着表面」的像素直接爆白。
pub const MIN_LIGHT_RANGE: f32 = 0.05;
/// 衰减距离上限：远超常规场景尺度，防止脚本传 1e9 把整个场景照成白纸。
pub const MAX_LIGHT_RANGE: f32 = 500.0;
/// 单盏灯强度上限（线性 HDR 乘数）。
pub const MAX_LIGHT_INTENSITY: f32 = 200.0;
/// 颜色单通道上限，避免 HDR 颜色把后处理的泛光阈值顶穿。
pub const MAX_LIGHT_COLOR: f32 = 16.0;
/// 平滑窗衰减分母里的软化常数：`1 / (d² + ε)` 让 d→0 时有限。
pub const ATTENUATION_EPSILON: f32 = 0.01;
/// 平滑窗幂次：`(1 - (d/range)^4)²` 里的 4（与 windowed inverse-square 一致）。
pub const ATTENUATION_WINDOW_POWER: i32 = 4;
/// 光锥外角上限（度）。逼近 90° 时 cos 趋零，再大就该用点光了。
pub const MAX_CONE_ANGLE_DEG: f32 = 89.9;
/// 外角必须至少比内角大这么多度，否则 smoothstep 退化成硬边。
pub const MIN_CONE_GAP_DEG: f32 = 0.1;

/// 局部光源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalLightKind {
    /// 点光：向四面八方发光，只看距离衰减。
    #[default]
    Point,
    /// 聚光：在点光基础上再乘一个方向锥因子。
    Spot,
}

impl LocalLightKind {
    /// 规范名（脚本 / RPC 里用的小写串）。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Spot => "spot",
        }
    }

    /// 解析规范名（大小写不敏感）。
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "point" | "omni" => Some(Self::Point),
            "spot" | "cone" => Some(Self::Spot),
            _ => None,
        }
    }

    /// 上传给 GPU 的类型码（0 = 点光，1 = 聚光），与 wgsl 里 `kind > 0.5` 的判定一致。
    pub const fn gpu_code(self) -> f32 {
        match self {
            Self::Point => 0.0,
            Self::Spot => 1.0,
        }
    }
}

/// 一盏局部光源。
///
/// 字段布局刻意保持「能直接按 4 个 vec4 拍进 uniform」：位置 + 距离、颜色 + 强度、
/// 方向 + 内角余弦、外角余弦 + 类型码。字段用数组而非 `Vec3` 是为了让场景 JSON
/// （`"pos": [1, 2, 3]`）与序列化严格对称，不引入 glam 的 serde feature。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalLight {
    /// 脚本 / RPC / 场景里的唯一主键。重复写同一 key = 整体覆盖（不累积）。
    pub key: String,
    pub kind: LocalLightKind,
    /// 世界坐标下的光源位置。
    pub pos: [f32; 3],
    /// 线性 RGB 颜色（会被 `intensity` 放大）。
    pub color: [f32; 3],
    /// 强度乘数（线性 HDR）。
    pub intensity: f32,
    /// 影响半径：距离 ≥ range 的片元完全不受这盏灯影响。
    pub range: f32,
    /// 聚光的照射方向（从光源射出的方向）。点光忽略此字段。
    pub dir: [f32; 3],
    /// 光锥内角（度）：这个角度内是满强度。
    pub inner_deg: f32,
    /// 光锥外角（度）：这个角度外是零。
    pub outer_deg: f32,
}

impl Default for LocalLight {
    fn default() -> Self {
        Self {
            key: String::new(),
            kind: LocalLightKind::Point,
            pos: [0.0, 2.0, 0.0],
            color: [1.0, 0.95, 0.88],
            intensity: 1.0,
            range: 10.0,
            dir: [0.0, -1.0, 0.0],
            inner_deg: 15.0,
            outer_deg: 30.0,
        }
    }
}

impl LocalLight {
    /// 新建一盏点光（其余字段走默认值）。
    pub fn point(key: impl Into<String>, pos: [f32; 3], color: [f32; 3], intensity: f32) -> Self {
        Self {
            key: key.into(),
            kind: LocalLightKind::Point,
            pos,
            color,
            intensity,
            ..Self::default()
        }
    }

    /// 新建一盏聚光。
    #[allow(clippy::too_many_arguments)] // 七个几何/光学参数各自独立命名，打包成结构体反而降低可读性
    pub fn spot(
        key: impl Into<String>,
        pos: [f32; 3],
        dir: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        range: f32,
        inner_deg: f32,
        outer_deg: f32,
    ) -> Self {
        Self {
            key: key.into(),
            kind: LocalLightKind::Spot,
            pos,
            dir,
            color,
            intensity,
            range,
            inner_deg,
            outer_deg,
        }
        .sanitized()
    }

    /// 数值收敛：把脚本 / 场景里可能出现的 NaN、负数、天文数字一次性夹进可用区间。
    ///
    /// 与 `ShadowSettings::sanitized` 同一条原则——**坏值在进 uniform 之前消失**，
    /// 渲染层不需要再判一次。收敛后保证：位置 / 颜色 / 方向全有限，range 与强度非负，
    /// 内角 ∈ [0, MAX_CONE_ANGLE_DEG]，外角 ≥ 内角 + MIN_CONE_GAP_DEG（且不超过上限）。
    pub fn sanitized(mut self) -> Self {
        self.pos = sanitize_vec3(self.pos, [0.0, 0.0, 0.0]);
        self.color =
            sanitize_vec3(self.color, [0.0, 0.0, 0.0]).map(|c| c.clamp(0.0, MAX_LIGHT_COLOR));
        self.intensity = if self.intensity.is_finite() {
            self.intensity.clamp(0.0, MAX_LIGHT_INTENSITY)
        } else {
            0.0
        };
        self.range = if self.range.is_finite() {
            self.range.clamp(MIN_LIGHT_RANGE, MAX_LIGHT_RANGE)
        } else {
            LocalLight::default().range
        };
        // 方向为零向量时保留默认朝下，避免聚光整片全黑却查不出原因
        let dir = sanitize_vec3(self.dir, [0.0, -1.0, 0.0]);
        let dir_len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        self.dir = if dir_len > 1e-6 {
            [dir[0] / dir_len, dir[1] / dir_len, dir[2] / dir_len]
        } else {
            [0.0, -1.0, 0.0]
        };
        self.inner_deg = if self.inner_deg.is_finite() {
            self.inner_deg.clamp(0.0, MAX_CONE_ANGLE_DEG)
        } else {
            0.0
        };
        self.outer_deg = if self.outer_deg.is_finite() {
            self.outer_deg
        } else {
            LocalLight::default().outer_deg
        };
        // 外角必须比内角大一圈：先保证下界，再夹上界（内角已被夹到上限下方）
        let min_outer = (self.inner_deg + MIN_CONE_GAP_DEG).min(MAX_CONE_ANGLE_DEG);
        self.outer_deg = self.outer_deg.clamp(min_outer, MAX_CONE_ANGLE_DEG);
        self
    }

    /// 颜色 × 强度的线性辐亮度（不含距离与锥因子）。
    pub fn radiance(&self) -> Vec3 {
        Vec3::from(self.color) * self.intensity
    }

    /// 内角余弦（GPU 侧同值，见 `shader.wgsl` 的 `cone.x`）。
    pub fn cos_inner(&self) -> f32 {
        self.inner_deg.to_radians().cos()
    }

    /// 外角余弦（GPU 侧同值，见 `shader.wgsl` 的 `cone.y`）。
    pub fn cos_outer(&self) -> f32 {
        self.outer_deg.to_radians().cos()
    }

    /// 距离衰减。与 `shader.wgsl::local_attenuation` 逐式一致。
    pub fn attenuation_at(&self, distance: f32) -> f32 {
        attenuation(self.range, distance)
    }

    /// 锥因子：点光恒 1.0，聚光按「内角满强度 → 外角零」平滑过渡。
    pub fn cone_factor(&self, world_position: Vec3) -> f32 {
        if self.kind == LocalLightKind::Point {
            return 1.0;
        }
        let to_fragment = world_position - Vec3::from(self.pos);
        if to_fragment.length_squared() <= 1e-12 {
            // 片元与光源重合：方向无意义，按满强度处理（衰减会把它压到极小）
            return 1.0;
        }
        let to_fragment = to_fragment.normalize();
        let cos_angle = Vec3::from(self.dir).dot(to_fragment);
        spot_factor(cos_angle, self.cos_inner(), self.cos_outer())
    }

    /// 该光源对某世界坐标点的「入射辐亮度」（不含 BRDF、不含 n·l）。
    ///
    /// 这是 CPU 侧参考实现：GPU 侧只多乘一次 `max(dot(n, l), 0) / π` 与 BRDF 项。
    /// 距离 ≥ range 时返回零向量——GPU 侧在循环里早退，语义相同。
    pub fn irradiance(&self, world_position: Vec3) -> Vec3 {
        let delta = world_position - Vec3::from(self.pos);
        let distance = delta.length();
        if distance >= self.range {
            return Vec3::ZERO;
        }
        self.radiance() * (self.attenuation_at(distance) * self.cone_factor(world_position))
    }
}

/// 平滑窗逆平方衰减：`(1 - (d/range)^4)² / (d² + ε)`，`d ≥ range` 时为 0。
///
/// 为什么不用裸 `1/d²`：光会在 range 边界上「啪」地消失。窗口项让它在边界处
/// 平滑归零，且 d→range 时导数也趋零，看不出接缝。
pub fn attenuation(range: f32, distance: f32) -> f32 {
    if !range.is_finite() || !distance.is_finite() || range <= 0.0 {
        return 0.0;
    }
    let d = distance.max(0.0);
    if d >= range {
        return 0.0;
    }
    let t = (d / range).clamp(0.0, 1.0);
    let window = 1.0 - t.powi(ATTENUATION_WINDOW_POWER);
    (window * window) / (d * d + ATTENUATION_EPSILON)
}

/// 聚光锥因子：`cos_angle ≤ cos_outer` 为 0，`≥ cos_inner` 为 1，中间 smoothstep。
///
/// 注意余弦的单调性：角度越大余弦越小，所以 `cos_inner > cos_outer` 才是正常配置。
/// 两者相等（角度退化）时退成硬边阶跃，而不是让 smoothstep 除零。
pub fn spot_factor(cos_angle: f32, cos_inner: f32, cos_outer: f32) -> f32 {
    if !cos_angle.is_finite() || !cos_inner.is_finite() || !cos_outer.is_finite() {
        return 0.0;
    }
    if cos_inner <= cos_outer {
        return if cos_angle >= cos_inner { 1.0 } else { 0.0 };
    }
    let t = ((cos_angle - cos_outer) / (cos_inner - cos_outer)).clamp(0.0, 1.0);
    // 3t² - 2t³：与 wgsl 的 smoothstep 同式
    t * t * (3.0 - 2.0 * t)
}

/// 单通道夹取：非有限值退回 `fallback`。
fn sanitize_vec3(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let mut out = fallback;
    for i in 0..3 {
        if v[i].is_finite() {
            out[i] = v[i];
        }
    }
    out
}

/// 场景层面的局部光源表操作：按 key 覆盖式写入（同 key 原地替换、顺序不变）。
///
/// 与 `ui::upsert` 同一套语义：**上限只拦新增、不拦覆盖**——脚本每帧重发同一盏灯的
/// 位置时必须能写进去，否则会被静默丢弃。
pub fn upsert_local(elements: &mut Vec<LocalLight>, light: LocalLight) -> bool {
    let light = light.sanitized();
    if let Some(slot) = elements.iter_mut().find(|e| e.key == light.key) {
        *slot = light;
        return true;
    }
    if elements.len() >= MAX_LOCAL_LIGHTS {
        return false;
    }
    elements.push(light);
    true
}

/// 按 key 移除；返回是否真的删掉了。
pub fn remove_local(elements: &mut Vec<LocalLight>, key: &str) -> bool {
    match elements.iter().position(|e| e.key == key) {
        Some(idx) => {
            elements.remove(idx);
            true
        }
        None => false,
    }
}

/// 按 key 查一盏灯。
pub fn find_local<'a>(elements: &'a [LocalLight], key: &str) -> Option<&'a LocalLight> {
    elements.iter().find(|e| e.key == key)
}

/// 按 key 改一盏灯（找不到返回 `None`，调用方据此报错而不是新建）。
pub fn with_local<'a, F>(elements: &'a mut [LocalLight], key: &str, f: F) -> Option<&'a LocalLight>
where
    F: FnOnce(&mut LocalLight),
{
    let slot = elements.iter_mut().find(|e| e.key == key)?;
    f(slot);
    *slot = slot.clone().sanitized();
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 场景中心必须落在阴影相机 NDC 的深度容差内、x/y 约为 0。
    #[test]
    fn shadow_frames_scene_center() {
        let light = Light::default();
        let center = Vec3::new(2.0, 1.0, -3.0);
        let vp = light.shadow_view_projection(center, 5.0);
        let clip = vp * center.extend(1.0);
        let ndc = clip / clip.w;
        assert!(ndc.x.abs() < 1e-4, "ndc.x = {}", ndc.x);
        assert!(ndc.y.abs() < 1e-4, "ndc.y = {}", ndc.y);
        assert!(ndc.z >= 0.0 && ndc.z <= 1.0, "ndc.z = {}", ndc.z);
    }

    /// 包围球内的点必须落在正交视锥内；球外偏移点必须落出去。
    #[test]
    fn shadow_extents_follow_radius() {
        let light = Light::default();
        let center = Vec3::ZERO;
        let radius = 4.0;
        let vp = light.shadow_view_projection(center, radius);
        let inside = center + Vec3::new(radius * 0.9, 0.0, 0.0);
        let clip = vp * inside.extend(1.0);
        assert!(clip.x.abs() <= clip.w, "inside 应落在视锥内");
        let outside = center + Vec3::new(radius * 3.0, 0.0, 0.0);
        let clip = vp * outside.extend(1.0);
        assert!(clip.x.abs() > clip.w, "outside 应落在视锥外");
    }

    /// 固定 ortho_size 时半宽不再随半径变化。
    #[test]
    fn fixed_ortho_size_overrides_radius() {
        let mut light = Light::default();
        light.shadow.ortho_size = 1.0;
        let vp = light.shadow_view_projection(Vec3::ZERO, 10.0);
        let p = Vec3::new(2.0, 0.0, 0.0);
        let clip = vp * p.extend(1.0);
        assert!(clip.x.abs() > clip.w, "半径 2 的点应超出半宽 1 的视锥");
    }

    /// 坏参数被 sanitize 兜住，不产生 NaN。
    #[test]
    fn sanitize_handles_garbage() {
        let s = ShadowSettings {
            enabled: true,
            size: 0,
            ortho_size: -3.0,
            bias: f32::NAN,
            normal_bias: f32::INFINITY,
            distance: 0.0,
            strength: 4.0,
            cascade: CascadeSettings {
                count: 9,
                lambda: f32::NAN,
                blend: 9.0,
                max_distance: -1.0,
            },
        }
        .sanitized();
        assert_eq!(s.size, 64);
        assert_eq!(s.ortho_size, 0.0);
        assert_eq!(s.bias, 0.0);
        assert!(s.normal_bias.is_finite());
        assert!(s.distance >= 0.05);
        assert_eq!(s.strength, 1.0);
        assert_eq!(s.cascade.count, MAX_CASCADES as u32);
        assert!(s.cascade.lambda.is_finite());
        assert!(s.cascade.blend <= 0.5);
        assert_eq!(s.cascade.max_distance, 0.0);
        let vp = Light::default().shadow_view_projection(Vec3::ZERO, 2.0);
        assert!(vp.to_cols_array().iter().all(|v| v.is_finite()));
    }

    // ---------------- 扩展十六：级联阴影 ----------------

    /// 切分点必须严格递增、首尾落在 near/far 上。
    #[test]
    fn cascade_splits_are_monotonic_and_cover_range() {
        let camera = Camera {
            znear: 0.1,
            zfar: 200.0,
            ..Camera::default()
        };
        let splits = cascade_splits(
            &camera,
            CascadeSettings {
                count: 4,
                lambda: 0.6,
                blend: 0.1,
                max_distance: 0.0,
            },
        );
        assert_eq!(splits.len(), 5);
        assert!((splits[0] - 0.1).abs() < 1e-4, "首切分 = znear");
        assert!((splits[4] - 200.0).abs() < 1e-2, "末切分 = zfar");
        for w in splits.windows(2) {
            assert!(w[1] > w[0], "切分点必须严格递增: {w:?}");
        }
    }

    /// 单级时等于「相机整段视距」，即阶段 D 的行为。
    #[test]
    fn cascade_splits_single_level_is_full_range() {
        let camera = Camera {
            znear: 0.5,
            zfar: 120.0,
            ..Camera::default()
        };
        let splits = cascade_splits(&camera, CascadeSettings::default());
        assert_eq!(splits.len(), 2);
        assert!((splits[1] - 120.0).abs() < 1e-2);
    }

    /// `max_distance` 覆盖 `zfar`；lambda 越大，第一级越短。
    #[test]
    fn cascade_splits_respects_max_distance_and_lambda() {
        let camera = Camera {
            znear: 0.1,
            zfar: 500.0,
            ..Camera::default()
        };
        let capped = cascade_splits(
            &camera,
            CascadeSettings {
                count: 2,
                max_distance: 40.0,
                ..CascadeSettings::default()
            },
        );
        assert!(
            (capped[2] - 40.0).abs() < 1e-2,
            "末切分被 max_distance 截断"
        );

        let uniform = cascade_splits(
            &camera,
            CascadeSettings {
                count: 3,
                lambda: 0.0,
                ..CascadeSettings::default()
            },
        );
        let log = cascade_splits(
            &camera,
            CascadeSettings {
                count: 3,
                lambda: 1.0,
                ..CascadeSettings::default()
            },
        );
        assert!(
            log[1] < uniform[1],
            "对数切分的第一级应比均匀切分更短: {} vs {}",
            log[1],
            uniform[1]
        );
    }

    /// 子视锥角点必须真的贴在相机的近 / 远平面与四个边界上。
    #[test]
    fn frustum_corners_match_projection() {
        let camera = Camera {
            position: Vec3::new(3.0, 2.0, -4.0),
            target: Vec3::new(0.5, 0.0, 1.0),
            znear: 0.1,
            zfar: 50.0,
            aspect: 1.5,
            ..Camera::default()
        };
        let vp = camera.view_projection();
        let corners = frustum_slice_corners(&camera, 0.1, 50.0);
        for (i, c) in corners.iter().enumerate() {
            let clip = vp * c.extend(1.0);
            let ndc = clip / clip.w;
            assert!((ndc.x.abs() - 1.0).abs() < 1e-3, "角点 {i} x = {}", ndc.x);
            assert!((ndc.y.abs() - 1.0).abs() < 1e-3, "角点 {i} y = {}", ndc.y);
            let want = if i < 4 { 0.0 } else { 1.0 };
            assert!((ndc.z - want).abs() < 1e-3, "角点 {i} z = {}", ndc.z);
        }
    }

    /// 拟合后的光源正交相机必须把该级 8 个角点全部包住（NDC 各分量在 [-1, 1]）。
    #[test]
    fn fit_cascade_contains_all_corners() {
        let camera = Camera::default();
        let light = Light::default();
        let splits = cascade_splits(
            &camera,
            CascadeSettings {
                count: 3,
                ..CascadeSettings::default()
            },
        );
        for w in splits.windows(2) {
            let corners = frustum_slice_corners(&camera, w[0], w[1]);
            let fit = fit_cascade(light.direction, &corners, 2048);
            assert!(fit.radius > 0.0 && fit.radius.is_finite());
            for c in corners {
                let clip = fit.view_proj * c.extend(1.0);
                assert!(clip.w.abs() > 1e-6, "正交投影 w 应恒为 1");
                let ndc = clip / clip.w;
                assert!(ndc.x >= -1.0 - 1e-3 && ndc.x <= 1.0 + 1e-3, "x = {}", ndc.x);
                assert!(ndc.y >= -1.0 - 1e-3 && ndc.y <= 1.0 + 1e-3, "y = {}", ndc.y);
                assert!(ndc.z >= -1e-3 && ndc.z <= 1.0 + 1e-3, "z = {}", ndc.z);
            }
        }
    }

    /// 同一输入必须得到同一矩阵（纹素量化是确定性的，不能引入随机抖动）。
    #[test]
    fn fit_cascade_is_deterministic() {
        let camera = Camera::default();
        let corners = frustum_slice_corners(&camera, 0.1, 30.0);
        let a = fit_cascade(Light::default().direction, &corners, 2048);
        let b = fit_cascade(Light::default().direction, &corners, 2048);
        assert_eq!(a.view_proj, b.view_proj);
        assert_eq!(a.radius, b.radius);
    }

    /// 坏输入（全 NaN 角点 / 零方向）不能产出 NaN 矩阵。
    #[test]
    fn fit_cascade_handles_garbage() {
        let corners = [Vec3::splat(f32::NAN); 8];
        let fit = fit_cascade(Vec3::ZERO, &corners, 1024);
        assert!(fit.view_proj.to_cols_array().iter().all(|v| v.is_finite()));
        let corners = frustum_slice_corners(&Camera::default(), 0.1, 20.0);
        let fit = fit_cascade(Vec3::ZERO, &corners, 1024);
        assert!(fit.view_proj.to_cols_array().iter().all(|v| v.is_finite()));
    }

    // ---------------- 扩展十五：局部光源 ----------------

    /// 平滑窗衰减：距离 0 时有限、range 边界归零、单调递减。
    #[test]
    fn attenuation_is_windowed_and_finite() {
        let a0 = attenuation(10.0, 0.0);
        assert!(a0.is_finite() && a0 > 0.0, "d=0 必须有限正数，实际 {a0}");
        assert_eq!(attenuation(10.0, 10.0), 0.0, "range 边界必须归零");
        assert_eq!(attenuation(10.0, 25.0), 0.0, "range 之外必须为零");
        let mut prev = f32::INFINITY;
        for step in 0..20 {
            let d = step as f32 * 0.5;
            let a = attenuation(10.0, d);
            assert!(a.is_finite(), "d={d} 出现非有限值");
            assert!(a <= prev + 1e-6, "衰减必须单调不增，d={d}");
            prev = a;
        }
        assert_eq!(attenuation(0.0, 1.0), 0.0, "range=0 直接为零");
        assert_eq!(attenuation(f32::NAN, 1.0), 0.0, "range=NaN 直接为零");
    }

    /// 聚光锥：内角内满强度、外角外为零、中间平滑且严格介于两者之间。
    #[test]
    fn spot_cone_ramps_between_angles() {
        let l = LocalLight {
            kind: LocalLightKind::Spot,
            inner_deg: 20.0,
            outer_deg: 40.0,
            ..LocalLight::default()
        }
        .sanitized();
        let pos = Vec3::from(l.pos);
        // 沿方向轴正下方（角度 0）→ 满强度
        assert!((l.cone_factor(pos + Vec3::new(0.0, -1.0, 0.0)) - 1.0).abs() < 1e-6);
        // 偏 20°（= 内角）：仍应接近满强度
        let at_inner =
            l.cone_factor(pos + Vec3::new(0.0, -1.0, 0.0).rotate_x(20f32.to_radians()) * 3.0);
        assert!(at_inner > 0.95, "内角边缘应接近满强度，实际 {at_inner}");
        // 偏 60°（> 外角 40°）：零
        let outside = pos + Vec3::new(0.0, -1.0, 0.0).rotate_x(60f32.to_radians()) * 3.0;
        assert_eq!(l.cone_factor(outside), 0.0);
        // 偏 30°（内外角之间）：落在 (0,1) 开区间
        let mid = pos + Vec3::new(0.0, -1.0, 0.0).rotate_x(30f32.to_radians()) * 3.0;
        let at_mid = l.cone_factor(mid);
        assert!(
            at_mid > 0.01 && at_mid < 0.99,
            "锥中间应平滑过渡，实际 {at_mid}"
        );
    }

    /// 角度退化（内外角相等）不能除零：退成硬边阶跃。
    #[test]
    fn spot_factor_degrades_to_step() {
        let cos_edge = 30.0_f32.to_radians().cos();
        assert_eq!(spot_factor(cos_edge + 0.01, cos_edge, cos_edge), 1.0);
        assert_eq!(spot_factor(cos_edge - 0.01, cos_edge, cos_edge), 0.0);
    }

    /// 点光的锥因子恒 1，与方向 / 位置无关。
    #[test]
    fn point_light_has_no_cone() {
        let l = LocalLight::point("p", [0.0, 5.0, 0.0], [1.0, 1.0, 1.0], 2.0).sanitized();
        assert_eq!(l.cone_factor(Vec3::new(100.0, 0.0, 100.0)), 1.0);
        assert_eq!(l.kind.gpu_code(), 0.0);
        assert_eq!(LocalLightKind::Spot.gpu_code(), 1.0);
    }

    /// 辐亮度在 range 外为零、在光源附近有限且随距离下降。
    #[test]
    fn irradiance_falls_off_with_distance() {
        let l = LocalLight::point("p", [0.0, 0.0, 0.0], [1.0, 0.5, 0.25], 4.0).sanitized();
        assert_eq!(l.irradiance(Vec3::new(0.0, 20.0, 0.0)), Vec3::ZERO);
        let near = l.irradiance(Vec3::new(0.1, 0.0, 0.0));
        let far = l.irradiance(Vec3::new(3.0, 0.0, 0.0));
        assert!(near.is_finite() && near.length() > 0.0);
        assert!(near.length() > far.length(), "近处应比远处亮");
        // 颜色比例保持：rgb = 1 : 0.5 : 0.25
        assert!((near.y / near.x - 0.5).abs() < 1e-5);
    }

    /// 坏值全部被收敛：NaN / 负数 / 天文数字都不会漏进 uniform。
    #[test]
    fn local_light_sanitize_handles_garbage() {
        let l = LocalLight {
            key: "bad".into(),
            kind: LocalLightKind::Spot,
            pos: [f32::NAN, 1.0, f32::INFINITY],
            color: [f32::NAN, -3.0, 1e9],
            intensity: f32::INFINITY,
            range: -5.0,
            dir: [0.0, 0.0, 0.0],
            inner_deg: 120.0,
            outer_deg: 5.0,
        }
        .sanitized();
        assert!(l.pos.iter().all(|v| v.is_finite()));
        assert_eq!(l.intensity, 0.0);
        assert_eq!(l.range, MIN_LIGHT_RANGE);
        assert_eq!(l.color[0], 0.0);
        assert_eq!(l.color[1], 0.0);
        assert_eq!(l.color[2], MAX_LIGHT_COLOR);
        assert_eq!(l.dir, [0.0, -1.0, 0.0], "零向量方向退回朝下");
        assert!((l.dir[0].powi(2) + l.dir[1].powi(2) + l.dir[2].powi(2) - 1.0).abs() < 1e-5);
        assert_eq!(l.inner_deg, MAX_CONE_ANGLE_DEG);
        assert!(l.outer_deg >= l.inner_deg);
        assert!(l.outer_deg <= MAX_CONE_ANGLE_DEG);
    }

    /// 数量上限只拦新增、不拦覆盖：满载时同 key 更新仍然生效。
    #[test]
    fn upsert_local_caps_new_entries_only() {
        let mut lights: Vec<LocalLight> = Vec::new();
        for i in 0..MAX_LOCAL_LIGHTS {
            assert!(upsert_local(
                &mut lights,
                LocalLight::point(format!("l{i}"), [i as f32, 0.0, 0.0], [1.0; 3], 1.0)
            ));
        }
        assert_eq!(lights.len(), MAX_LOCAL_LIGHTS);
        assert!(
            !upsert_local(
                &mut lights,
                LocalLight::point("overflow", [0.0; 3], [1.0; 3], 1.0)
            ),
            "超出上限的新增必须被拒绝"
        );
        // 同 key 覆盖：数量不变、内容更新、顺序不变
        assert!(upsert_local(
            &mut lights,
            LocalLight::point("l0", [9.0, 9.0, 9.0], [0.0, 1.0, 0.0], 3.0)
        ));
        assert_eq!(lights.len(), MAX_LOCAL_LIGHTS);
        assert_eq!(lights[0].key, "l0");
        assert_eq!(lights[0].pos, [9.0, 9.0, 9.0]);
        assert!(remove_local(&mut lights, "l0"));
        assert!(!remove_local(&mut lights, "l0"));
        assert_eq!(lights.len(), MAX_LOCAL_LIGHTS - 1);
        assert!(find_local(&lights, "l1").is_some());
        assert!(find_local(&lights, "l0").is_none());
    }

    /// `with_local` 找不到 key 时返回 None（调用方据此报错，而不是悄悄新建）。
    #[test]
    fn with_local_reports_missing_key() {
        let mut lights = vec![LocalLight::point("a", [0.0; 3], [1.0; 3], 1.0)];
        assert!(with_local(&mut lights, "missing", |l| l.intensity = 5.0).is_none());
        let found = with_local(&mut lights, "a", |l| {
            l.intensity = 5.0;
            l.range = 3.0;
        });
        assert!(found.is_some());
        assert_eq!(lights[0].intensity, 5.0);
        assert_eq!(lights[0].range, 3.0);
    }
}
