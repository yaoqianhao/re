//! 局部改动体（`*Patch`）：只给要改的字段，其余保持原值。
//!
//! 每个 `*Patch` 都带 `is_empty()`；`dispatch` 用它拒绝「一个字段都没给」的空调用
//! （空调用对宿主是纯噪声，且容易掩盖客户端的序列化 bug）。

/// `scene.set_transform` 的局部改动：只给要改的字段，其余保持原值。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransformPatch {
    pub position: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
}

impl TransformPatch {
    pub fn is_empty(&self) -> bool {
        self.position.is_none() && self.rotation.is_none() && self.scale.is_none()
    }
}

/// `scene.set_material` 的局部改动：只给要改的字段，其余保持原值。
///
/// `metallic` / `roughness` 是 0..1 标量，`emissive` 是线性 RGB。传输层只做
/// 类型与范围校验，真正的收敛（NaN / 超界 / 负值）由宿主的 `SurfaceMaterial`
/// 完成，保证「非法值在写入场景前消失」。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MaterialPatch {
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub emissive: Option<[f32; 3]>,
}

impl MaterialPatch {
    pub fn is_empty(&self) -> bool {
        self.metallic.is_none() && self.roughness.is_none() && self.emissive.is_none()
    }
}

/// `scene.set_environment` 的局部改动：只给要改的字段，其余保持原值。
///
/// `enabled = Some(false)` 表示关掉环境探针、退回 `light.ambient` 常数项；
/// 其余字段与 `r3e_core::env::EnvironmentSettings` 一一对应。传输层只做类型 / 范围
/// 校验，真正的收敛（NaN / 超界 / 方向归一化）由宿主的 `EnvironmentSettings::sanitized`
/// 完成，与 `MaterialPatch` 同一条原则。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EnvironmentPatch {
    pub enabled: Option<bool>,
    pub sky_zenith: Option<[f32; 3]>,
    pub sky_horizon: Option<[f32; 3]>,
    pub ground: Option<[f32; 3]>,
    pub sun_dir: Option<[f32; 3]>,
    pub sun_color: Option<[f32; 3]>,
    pub sun_intensity: Option<f32>,
    pub intensity: Option<f32>,
}

impl EnvironmentPatch {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.sky_zenith.is_none()
            && self.sky_horizon.is_none()
            && self.ground.is_none()
            && self.sun_dir.is_none()
            && self.sun_color.is_none()
            && self.sun_intensity.is_none()
            && self.intensity.is_none()
    }
}

/// `scene.set_environment_map` 的局部改动（扩展二十三）：HDR 立方体反射探针。
///
/// 与 [`EnvironmentPatch`] 是**两条独立的线**：`environment` 是三色解析天空 + SH 系数，
/// 本补丁是「预滤波 cubemap」这条 HDR 支线；两者可以同时存在，互不覆盖。
///
/// 来源只有两种，`equirect`（一张等距柱状 `.hdr`）与 `faces`（六面 `.hdr`，顺序固定
/// +X/-X/+Y/-Y/+Z/-Z）；两者同时给时宿主按 `faces` 优先。要给「清空来源」时传空串 /
/// 空数组，收敛后等价于没写——这条路径比 `enabled = false` 更彻底（连配置一起丢掉）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvironmentMapPatch {
    pub enabled: Option<bool>,
    pub equirect: Option<String>,
    pub faces: Option<Vec<String>>,
    /// 预滤波基准面边长（像素）。有符号整数：负数留给宿主夹到区间下限。
    pub face_size: Option<i32>,
    /// 镜面 mip 链层数。同样有符号，理由同上。
    pub mip_levels: Option<i32>,
    pub intensity: Option<f32>,
}

impl EnvironmentMapPatch {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.equirect.is_none()
            && self.faces.is_none()
            && self.face_size.is_none()
            && self.mip_levels.is_none()
            && self.intensity.is_none()
    }
}

/// `scene.set_postfx` 的局部改动（扩展十三）：只给要改的字段，其余保持原值。
///
/// `enabled = Some(false)` 表示关掉整个后处理段，回到「离屏 HDR 目标 + composite
/// 直通」；`tonemap` 为规范名（`none` / `reinhard` / `aces`，大小写不敏感），
/// 传输层只校验类型，名字是否认得由宿主的 `TonemapMode::from_name` 判定后报参数错误
/// ——与 `EnvironmentPatch` 同一条原则：本层不认识 `r3e_core` 的任何类型。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostFxPatch {
    pub enabled: Option<bool>,
    pub exposure: Option<f32>,
    pub tonemap: Option<String>,
    pub bloom_enabled: Option<bool>,
    pub bloom_threshold: Option<f32>,
    pub bloom_knee: Option<f32>,
    pub bloom_intensity: Option<f32>,
    pub fxaa: Option<bool>,
}

impl PostFxPatch {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.exposure.is_none()
            && self.tonemap.is_none()
            && self.bloom_enabled.is_none()
            && self.bloom_threshold.is_none()
            && self.bloom_knee.is_none()
            && self.bloom_intensity.is_none()
            && self.fxaa.is_none()
    }
}

/// `scene.set_ssao` 的局部改动（扩展十七）：只给要改的字段，其余保持原值。
///
/// 遮蔽图挂在后处理段下（`postfx.ssao`），宿主在段缺失时可先实体化成缺省值再写；
/// 本层只做类型校验与转发，半径 / 偏置 / 强度 / 样本数的区间收敛一律由宿主的
/// `SsaoSettings::sanitized` 负责——与 `PostFxPatch` 同一条原则：本层不认识
/// `r3e_core` 的任何类型。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SsaoPatch {
    pub enabled: Option<bool>,
    pub radius: Option<f32>,
    pub bias: Option<f32>,
    pub intensity: Option<f32>,
    /// 样本数用有符号整数传输：负数留给宿主夹到区间下限，好过在传输层静默改成 0。
    pub samples: Option<i32>,
}

impl SsaoPatch {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.radius.is_none()
            && self.bias.is_none()
            && self.intensity.is_none()
            && self.samples.is_none()
    }
}

/// `scene.anim_layer_set` 的补丁（扩展十八 A）：只给要改的字段，其余保持原值。
///
/// 层由 `instance` + `layer` 定位（`layer` 给空串表示该实例的第一层）。
/// 与 `SsaoPatch` 同一条原则：本层**不认识** `r3e_core` 的任何类型——`mode` /
/// `mask` / `clip` 都是字符串，合法性由宿主判定后报参数错误；`mask` 的写法与脚本侧
/// 完全一致（`"arm,hand"` / `"!root"` / `""` 或 `"*"` 表示整副骨架）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimLayerPatch {
    pub instance: String,
    pub layer: String,
    /// 目标权重（0 = 不参与合成，1 = 全量）；带过渡请走脚本的 `fade_anim_layer`。
    pub weight: Option<f32>,
    /// 叠加模式规范名（`override` / `additive`，大小写不敏感）。
    pub mode: Option<String>,
    /// 骨骼遮罩（逗号串，`!` 前缀表示排除）。
    pub mask: Option<String>,
    /// 层播放速度（负值倒放）。
    pub speed: Option<f32>,
    /// 把层内片段整体换成这一路（层内收敛为单片段）。
    pub clip: Option<String>,
    /// 层内采样时间策略（扩展四十六）：`off` / `phase`，大小写不敏感。
    pub sync: Option<String>,
    /// 跨层相位源层名（扩展四十七）：空串 = 清除。只在 `sync = phase` 下影响采样。
    pub sync_source: Option<String>,
    /// 相位偏移（扩展四十七）：作用于本补丁 `clip` 指定的那一路；没给 `clip` 时作用于第一路。
    pub clip_offset: Option<f32>,
    /// 片段自带播放速率（扩展四十八）：作用于本补丁 `clip` 指定的那一路；没给 `clip` 时作用于第一路。
    /// 负值倒放、0 定格首帧；只在层 `sync = off` 时影响采样。
    pub clip_speed: Option<f32>,
    /// 片段循环标志（扩展四十八）：`false` 时到末帧停住。
    pub clip_looping: Option<bool>,
}

impl AnimLayerPatch {
    pub fn is_empty(&self) -> bool {
        self.weight.is_none()
            && self.mode.is_none()
            && self.mask.is_none()
            && self.speed.is_none()
            && self.clip.is_none()
            && self.sync.is_none()
            && self.sync_source.is_none()
            && self.clip_offset.is_none()
            && self.clip_speed.is_none()
            && self.clip_looping.is_none()
    }
}

/// `ui.set` 的补丁（扩展十四）：只给要改的字段，其余沿用旧值。
///
/// 与 `PostFxPatch` 同一条原则：本层**不认识** `r3e_core` 的任何类型。位置与尺寸用
/// 字符串单位原样透传（`"12"` = 12px、`"50%"` = 屏幕一半），由宿主用
/// `UiValue::parse` 收敛；`kind` / `anchor` 的合法性同理由宿主判定后报参数错误。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiPatch {
    pub key: String,
    pub kind: Option<String>,
    pub anchor: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
    pub color: Option<[f32; 4]>,
    pub layer: Option<i32>,
    pub visible: Option<bool>,
    pub border_width: Option<f32>,
    pub image: Option<String>,
}

impl UiPatch {
    /// 除 `key` 之外一个字段都没给——对已有元素是无意义调用，对新元素则是「全部取默认」。
    pub fn is_geometry_only(&self) -> bool {
        self.kind.is_none()
            && self.anchor.is_none()
            && self.x.is_none()
            && self.y.is_none()
            && self.width.is_none()
            && self.height.is_none()
            && self.color.is_none()
            && self.layer.is_none()
            && self.visible.is_none()
            && self.border_width.is_none()
            && self.image.is_none()
    }
}

/// `light.set` 的补丁（扩展十五）：只给要改的字段，其余沿用旧值。
///
/// 与 `UiPatch` 同一条原则：本层**不认识** `r3e_core` 的任何类型。`kind` 用规范名
/// （`point` / `spot`，大小写不敏感）透传，真正收敛（NaN / 负强度 / 零半径 /
/// 内外角倒挂 / 朝向归一化）由宿主的 `LocalLight::sanitized` 完成。
///
/// 语义分两种，由宿主按「key 是否已存在」自行区分：
/// - key 不存在 → **新建**（缺省字段取 `LocalLight` 的默认值）；
/// - key 已存在 → **局部更新**（只覆盖给了的字段）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightPatch {
    pub key: String,
    pub kind: Option<String>,
    pub pos: Option<[f32; 3]>,
    pub dir: Option<[f32; 3]>,
    pub color: Option<[f32; 3]>,
    pub intensity: Option<f32>,
    pub range: Option<f32>,
    pub inner_deg: Option<f32>,
    pub outer_deg: Option<f32>,
    /// 扩展十八 C：是否允许这盏灯参与阴影（`None` = 沿用原值）。
    ///
    /// 这里给的是「意愿」；真正入选由渲染层的 `plan_local_shadows` 决定
    /// （全世界最多 `MAX_LOCAL_SHADOW_CASTERS` 盏，按亮度排序）。
    pub cast_shadow: Option<bool>,
    /// 深度偏移（`None` = 沿用原值；宿主会夹进 `[0, MAX_LOCAL_SHADOW_BIAS]`）。
    pub shadow_bias: Option<f32>,
}

impl LightPatch {
    /// 除 `key` 之外一个字段都没给——对已有灯是无意义调用，对新灯则是「全部取默认」。
    pub fn is_key_only(&self) -> bool {
        self.kind.is_none()
            && self.pos.is_none()
            && self.dir.is_none()
            && self.color.is_none()
            && self.intensity.is_none()
            && self.range.is_none()
            && self.inner_deg.is_none()
            && self.outer_deg.is_none()
            && self.cast_shadow.is_none()
            && self.shadow_bias.is_none()
    }
}

/// `scene.shadow` 的补丁（扩展十六）：只给要改的阴影字段，其余沿用旧值。
///
/// 一个方法同时管读和写：**全部字段都不给 = 只读**，给了任意字段 = 合并后回写并
/// 返回收敛后的最终值。这样编辑器拖滑块时每帧只发变化的那个字段，不必先读后写。
///
/// 与 `LightPatch` 同样的原则：本层不认识 `r3e_core` 的类型，收敛（级数夹到
/// 1..=MAX_CASCADES、混合比夹到 0..1、尺寸夹到 64..8192）由宿主的 `sanitized` 完成。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShadowPatch {
    /// 阴影总开关。
    pub enabled: Option<bool>,
    /// 阴影贴图边长（正方形）。
    pub size: Option<u32>,
    /// 阴影强度 0..1。
    pub strength: Option<f32>,
    /// 各级覆盖半宽的上限，0 = 自适应。
    pub ortho_size: Option<f32>,
    /// 深度比较偏移。
    pub bias: Option<f32>,
    /// 级联级数。
    pub cascades: Option<u32>,
    /// 级间混合带宽（占该级跨度的比例）。
    pub blend: Option<f32>,
    /// 阴影最远覆盖距离，0 = 由场景包围球推断。
    pub distance: Option<f32>,
}

impl ShadowPatch {
    /// 一个字段都没给——纯只读调用。
    pub fn is_read_only(&self) -> bool {
        self.enabled.is_none()
            && self.size.is_none()
            && self.strength.is_none()
            && self.ortho_size.is_none()
            && self.bias.is_none()
            && self.cascades.is_none()
            && self.blend.is_none()
            && self.distance.is_none()
    }
}

/// `audio.spatial.set` 的补丁（§12 第十项）：只给要改的字段，其余沿用原值。
///
/// 与 [`LightPatch`] 同一套路数：本层不认识 `SpatialSettings` / `Vec3`，
/// 距离夹到正数、模型名解析、上限拒绝全由宿主完成；这里只做类型与空值校验，
/// 让「编辑器拼错了模型名」这种错误在宿主侧报出来（并保住原值不被改坏）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioSpatialPatch {
    pub key: String,
    /// 声源坐标 `[x, y, z]`（`None` = 沿用原值）。
    pub position: Option<[f32; 3]>,
    /// 绑定的场景节点名；`Some("")` = 解除绑定（改回纯脚本摆位）。
    ///
    /// 绑定只是**记名字**：节点变换每帧由宿主灌进声源，传输层不解析节点。
    pub node: Option<String>,
    /// 距离衰减起点（该距离内不衰减）。
    pub min_distance: Option<f32>,
    /// 距离衰减终点（超过即彻底听不见）。
    pub max_distance: Option<f32>,
    /// 距离模型名：`linear` / `inverse` / `exponential`（不区分大小写，`exp` 亦可）。
    pub model: Option<String>,
    /// 多普勒系数（0 = 关掉多普勒）。
    pub doppler: Option<f32>,
}

impl AudioSpatialPatch {
    /// 除 `key` 之外一个字段都没给——对已注册声源是无意义调用。
    pub fn is_key_only(&self) -> bool {
        self.position.is_none()
            && self.node.is_none()
            && self.min_distance.is_none()
            && self.max_distance.is_none()
            && self.model.is_none()
            && self.doppler.is_none()
    }
}

/// `audio.spatial.listener` 的补丁：两个字段都不给 = 纯只读。
///
/// 听者的 `forward` 决定多普勒的径向速度符号，`position` 决定距离衰减，
/// 因此两者可以分别改（只挪位置不动朝向是常见操作）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListenerPatch {
    pub position: Option<[f32; 3]>,
    /// 朝向向量（宿主会归一化；零向量视为无效、沿用原值）。
    pub forward: Option<[f32; 3]>,
}

impl ListenerPatch {
    pub fn is_read_only(&self) -> bool {
        self.position.is_none() && self.forward.is_none()
    }
}
