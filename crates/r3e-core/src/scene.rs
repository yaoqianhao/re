//! 场景图：物件容器、生命周期（生成 / 消失）、层次变换与文件加载。

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Vec3, Vec4};
use serde::{Deserialize, Serialize};

use crate::anim_blend::{
    apply_additive, blend_override, effective_phase, phase_source, sample_layer_pose,
    sample_layer_pose_additive, sample_layer_pose_additive_synced, sample_layer_pose_synced,
    sync_probe_with, AnimLayer, BoneMask, LayerMode, LayerSync, MAX_ANIM_LAYERS,
};
use crate::blend_space::BlendSpace;
use crate::light::{self, LocalLight};
use crate::morph::{MorphInstance, NO_EXCLUSIVE_GROUP};
use crate::text::ScreenText;
use crate::ui::UiElement;

use crate::skin::SkinInstance;
use crate::{
    AnimClip, AnimInstance, Camera, EnvironmentMapSettings, EnvironmentSettings, Light, MeshRef,
    Node, NodeId, OcclusionSettings, PostFxSettings, SurfaceMaterial, Transform,
};

/// 单个物件在场景文件 / 脚本 `spawn` 中的描述。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ObjectDesc {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// 物理描述：只做数据承载，真正生效取决于启用的 `PhysicsBackend`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<PhysicsDesc>,
    /// PBR 表面材质（metallic / roughness / emissive）；省略表示不覆盖网格资产材质。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<SurfaceMaterial>,
}

/// 物理刚体描述（预留）。当前 `NullBackend` 会忽略，`rapier` 后端会消费。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PhysicsDesc {
    /// "dynamic" | "static" | "kinematic"
    #[serde(
        default = "default_body_kind",
        skip_serializing_if = "is_default_body_kind"
    )]
    pub body: String,
    #[serde(default = "default_mass", skip_serializing_if = "is_default_mass")]
    pub mass: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restitution: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<f32>,
    /// "box" | "sphere" | "cylinder"，缺省由 mesh 推导
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collider: Option<String>,
    /// 碰撞体半尺寸，缺省由 scale 推导
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub half_extents: Option<[f32; 3]>,
    /// 连续碰撞检测（CCD）：高速物体不穿过薄墙。
    ///
    /// 省略表示「跟场景级 `physics.ccd`」（再缺省为开）；显式写 `false` 可让
    /// 单个物件退出 CCD（例如大量慢速道具，省掉 sweep 开销）。
    /// 后端不申报 `ccd` 能力时该字段无效。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccd: Option<bool>,
}

/// 场景级物理配置：对应场景 JSON 顶层 `physics` 字段。
///
/// 所有字段都是可选的，缺省值由 `r3e_physics::PhysicsConfig` 决定：
/// 默认 enabled=false（物理关闭）、backend="builtin"、重力 -9.81、地面 y=0。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PhysicsSceneDesc {
    /// 是否启用物理模拟。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// "builtin"（内置最简物理）| "null"（空转，只登记刚体）| "rapier"（需构建时启用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// 重力加速度向量，缺省 [0.0, -9.81, 0.0]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<[f32; 3]>,
    /// 地面平面高度，缺省 0.0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<f32>,
    /// 所有刚体的 CCD 缺省开关（逐物件 `physics.ccd` 优先）。
    ///
    /// 缺省 `true`：后端申报 `ccd` 能力时默认开启，避免「高速物件穿墙」
    /// 这种一看就是缺陷的表现。脚本可用 `physics_set_ccd(false)` 全局关。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccd: Option<bool>,
}

fn default_body_kind() -> String {
    "dynamic".to_string()
}
fn default_mass() -> f32 {
    1.0
}

/// `PhysicsDesc::body` 的缺省值是 `"dynamic"`：等于缺省值时不写进 JSON。
fn is_default_body_kind(body: &str) -> bool {
    body == "dynamic"
}

/// `PhysicsDesc::mass` 的缺省值是 `1.0`：等于缺省值时不写进 JSON。
fn is_default_mass(mass: &f32) -> bool {
    *mass == 1.0
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CameraDesc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct LightDesc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<[f32; 3]>,
    /// 阴影配置（阶段 D）；省略时沿用引擎默认（开启，2048²，自适应半宽）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowDesc>,
}

/// 场景文件里的阴影配置（阶段 D）。字段都可省略，省略即沿用默认值。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ShadowDesc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    /// 正交相机半宽；省略或 0 表示按场景包围球自适应
    #[serde(default, alias = "ortho_size", skip_serializing_if = "Option::is_none")]
    pub ortho: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bias: Option<f32>,
    #[serde(default, alias = "normalBias", skip_serializing_if = "Option::is_none")]
    pub normal_bias: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f32>,
    /// 级联级数 1..=4（扩展十六）；省略或 1 = 单级，等价阶段 D 的老行为。
    #[serde(default, alias = "cascade", skip_serializing_if = "Option::is_none")]
    pub cascades: Option<u32>,
    /// 级间混合带宽（占该级跨度的比例，0..0.5）；省略沿用默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<f32>,
    /// 级联最远距离（世界单位，沿相机前后方向）；省略或 0 = 用相机 `zfar`。
    ///
    /// 有意不复用上面的 `distance`：那个是阶段 D 阴影正交相机的前后偏移，
    /// 两者语义不同，混用会让老场景在开启 CSM 后阴影突然被裁掉一截。
    #[serde(
        default,
        rename = "cascadeDistance",
        alias = "cascade_distance",
        skip_serializing_if = "Option::is_none"
    )]
    pub cascade_distance: Option<f32>,
    /// 遮挡体沿光源方向的最大回退距离（扩展二十四）；省略或 0 = 不扩张。
    ///
    /// 写成正值才能让「接收区上方」的物件（屋檐、天桥、二层平台）把影子投进该级；
    /// 省略时深度范围与扩展二十三完全一致，老场景画面不受影响。
    #[serde(
        default,
        rename = "casterExtend",
        alias = "caster_extend",
        skip_serializing_if = "Option::is_none"
    )]
    pub caster_extend: Option<f32>,
    /// 是否按级裁剪遮挡体（扩展二十四）；省略 = 开。
    ///
    /// 只有排查「某级影子似乎缺了一块」时才需要显式写 `false` 做对照。
    #[serde(
        default,
        rename = "casterCull",
        alias = "caster_cull",
        skip_serializing_if = "Option::is_none"
    )]
    pub caster_cull: Option<bool>,
}

/// 资产自带的动画播放声明（P3 起）。
///
/// 写在场景 `assets[]` 项里，语义是「导入后立刻给这个资产实例挂一个播放器」。
/// `clip` 省略时取该资产动画栈里的第一个片段。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AnimDecl {
    /// 片段名（大小写不敏感）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    /// 播放速度，缺省 1.0；负值为倒放。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// 是否循环，缺省 true。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looping: Option<bool>,
    /// 起始时间（秒），缺省 0。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<f32>,
    /// 是否立即播放，缺省 true。false 时实例存在但处于暂停状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play: Option<bool>,
}

/// 场景文件里声明的外部资产（P2 起）。路径相对场景文件所在目录解析。
///
/// 声明后由应用层导入并在导入成功后按资产内网格 spawn 物件；
/// 物件也可以通过 `"mesh": "asset:<路径>#<序号>"` 直接引用同一资产。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AssetDecl {
    /// 资产路径（FBX 等）。
    pub path: String,
    /// 该资产下所有网格的整体偏移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    /// 整体缩放（三轴相同时即可当作单位换算，例如 FBX 以厘米建模就填 0.01）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    /// 只导入不 spawn（供脚本 / 其它物件的 `asset:` 引用使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<bool>,
    /// 导入后自动挂载的动画播放器（P3 起）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<AnimDecl>,
}

/// 场景文件结构（JSON）。
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SceneFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraDesc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<LightDesc>,
    /// 环境探针（IBL）。缺省表示沿用 `light.ambient` 常数环境光，与旧场景完全兼容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentSettings>,
    /// HDR 后处理链（扩展十三）。缺省表示直通（曝光 0 / 不调色 / 无泛光 / 无 FXAA），
    /// 画面与「没有 postfx 段」的旧场景逐像素一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postfx: Option<PostFxSettings>,
    /// 局部光源表（扩展十五：点光 / 聚光）。空表示场景只有方向光，画面与旧场景一致。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<LocalLight>,
    /// 遮挡剔除（扩展二十二）。缺省 / `enabled = false` 表示不剔除：渲染层不建深度
    /// 金字塔、不发深度读回，GPU 提交量与旧场景完全一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occlusion: Option<OcclusionSettings>,
    /// HDR 立方体反射探针（扩展二十三）。缺省表示没有环境图，镜面 / 漫反射环境项
    /// 仍由 `environment` 的解析天空 + SH 系数给出，画面与旧场景逐像素一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_map: Option<EnvironmentMapSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectDesc>,
    /// 2D 屏幕空间文本（HUD）。空表示场景没有文本层。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<ScreenText>,
    /// 2D 屏幕空间 UI 元素（矩形 / 边框 / 图片）。空表示场景没有 UI 层。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<UiElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<PhysicsSceneDesc>,
    /// 外部资产声明（应用层负责真正导入）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetDecl>,
}

/// 场景：所有可渲染物件的唯一真源。
#[derive(Debug)]
pub struct Scene {
    nodes: HashMap<NodeId, Node>,
    order: Vec<NodeId>,
    name_index: HashMap<String, NodeId>,
    next_id: NodeId,

    pub name: String,
    pub time: f32,
    pub frame: u64,
    pub camera: Camera,
    pub light: Light,
    /// 环境探针（IBL，扩展十二）。`None` 表示环境光继续用 `light.ambient` 常数项；
    /// `Some` 时渲染层改用三色天空 + 太阳 + 球谐辐照度做漫反射 / 镜面环境光。
    /// 与 `light` 的关系是二选一：两者都进 uniform，由开关位决定本帧用哪套，不会叠加过亮。
    pub environment: Option<EnvironmentSettings>,
    /// HDR 后处理链参数（扩展十三）。`None` 与 `Some(PostFxSettings::default())` 等价：
    /// 渲染层仍然走「离屏 HDR 目标 → composite」，只是 composite 里只剩 clamp。
    pub postfx: Option<PostFxSettings>,
    pub background: Vec4,
    /// 场景级物理配置，随场景一起热重载。
    pub physics: PhysicsSceneDesc,
    /// 场景文件声明的外部资产（含未导入成功前的原始声明），供应用层导入与热重载比对。
    pub assets: Vec<AssetDecl>,
    /// 场景内的动画实例（P3 起）。每个资产实例（或脚本手动挂载的播放器）一条。
    pub animations: Vec<AnimInstance>,
    /// 可切换的片段目录（P3 起）：键为 `"<资产名>/<片段名>"`，值与其他实例共享同一份
    /// `Arc<AnimClip>`。应用层导入资产后登记，脚本用 `set_anim_clip` 按名字切片段。
    pub anim_clips: HashMap<String, std::sync::Arc<AnimClip>>,
    /// 场景内的蒙皮实例（P4 起）。每个带蒙皮的网格实例一条，每帧由
    /// `update_skinning` 求解关节矩阵，渲染层直接上传。
    pub skins: Vec<SkinInstance>,
    /// 顶点形变绑定（扩展三十六）。每个带 morph target 的网格实例一条，
    /// 记录各形状的当前权重；渲染层每帧直接把它写进实例缓冲，
    /// 形变本身在顶点着色器里完成。顺序与 `skins` 无关。
    pub morphs: Vec<MorphInstance>,
    /// 2D 屏幕空间文本（M1 文字渲染）。渲染层每帧据此重新排版，是文本的唯一真源。
    /// 顺序即绘制顺序（后画的盖住先画的）。
    pub texts: Vec<ScreenText>,
    /// 2D 文本通道总开关（脚本 `set_text_enabled`）。false 时渲染层整帧跳过文本通道，
    /// 场景里的文本数据保留，再打开即原样恢复。
    pub text_enabled: bool,
    /// 2D 屏幕空间 UI 元素（M1 最小 UI）。渲染层每帧按层级排序后重新解析像素矩形，
    /// 窗口 resize 不需要任何额外通知。加入顺序 = 同层内的绘制顺序。
    pub ui: Vec<UiElement>,
    /// 2D UI 通道总开关（脚本 `set_ui_enabled`）。false 时整帧跳过 UI 通道，数据保留。
    pub ui_enabled: bool,
    /// UI 与文本的前后关系（脚本 `ui_above_text`）。默认 false：文本画在 UI 之上，
    /// HUD 底框不会盖住文字；置 true 时 UI 通道在文本之后绘制。
    pub ui_above_text: bool,
    /// 局部光源表（扩展十五）。与方向光 `light` 并存：方向光负责主光与阴影，
    /// 这里放灯泡 / 台灯 / 手电这类「有位置、有衰减、有方向锥」的光。
    /// 顺序即上传顺序，上限 [`MAX_LOCAL_LIGHTS`]（超出的新增被拒绝，覆盖不受限）。
    pub lights: Vec<LocalLight>,
    /// 局部光源总开关（脚本 `set_lights_enabled`）。false 时渲染层整帧跳过局部光循环，
    /// 数据保留，再打开即原样恢复——用于对比「有灯 / 无灯」或临时省 GPU。
    pub lights_enabled: bool,
    /// 遮挡剔除参数（扩展二十二）。`enabled = false`（缺省）时渲染层整帧跳过遮挡判定，
    /// 不建金字塔、不读回深度，画面与「只有视锥剔除」逐像素一致。
    pub occlusion: OcclusionSettings,
    /// HDR 立方体反射探针（扩展二十三）。`None` 或未激活时，镜面 / 漫反射环境项完全由
    /// `environment` 的解析天空 + SH 9 系数给出，画面与扩展二十三之前逐像素一致。
    /// 激活后金属 / 光滑材质改从预滤波 cubemap 按粗糙度选 mip 采样。
    pub environment_map: Option<EnvironmentMapSettings>,
    /// 场景**结构**的版本号（扩展二十五）：增删节点时自增，变换 / 可见性变化不碰它。
    ///
    /// 渲染层据此缓存遮挡体的层级树——拓扑不变时树可以跨帧复用，只需刷新节点盒。
    layout_version: u64,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            order: Vec::new(),
            name_index: HashMap::new(),
            next_id: 1,
            layout_version: 1,
            name: "untitled".to_string(),
            time: 0.0,
            frame: 0,
            camera: Camera::default(),
            light: Light::default(),
            environment: None,
            postfx: None,
            background: Vec4::new(0.05, 0.06, 0.08, 1.0),
            physics: PhysicsSceneDesc::default(),
            assets: Vec::new(),
            animations: Vec::new(),
            anim_clips: HashMap::new(),
            skins: Vec::new(),
            morphs: Vec::new(),
            texts: Vec::new(),
            text_enabled: true,
            ui: Vec::new(),
            ui_enabled: true,
            ui_above_text: false,
            lights: Vec::new(),
            lights_enabled: true,
            occlusion: OcclusionSettings::default(),
            environment_map: None,
        }
    }

    // ---------- 局部光源（扩展十五） ----------

    /// 按 key 覆盖式写入一盏局部光源。
    ///
    /// 语义与 `ui::upsert` 一致：同 key 原地替换（顺序不变），新 key 追加；
    /// 满载时只拒绝新增，同 key 更新仍然生效。返回值即「这次写入是否生效」。
    /// 入参在这里统一跑 `sanitized()`，所以脚本传 NaN 也污染不了场景。
    pub fn upsert_local_light(&mut self, light: LocalLight) -> bool {
        if light.key.is_empty() {
            return false;
        }
        light::upsert_local(&mut self.lights, light)
    }

    /// 按 key 移除一盏局部光源。
    pub fn remove_local_light(&mut self, key: &str) -> bool {
        light::remove_local(&mut self.lights, key)
    }

    /// 按 key 查一盏局部光源。
    pub fn local_light(&self, key: &str) -> Option<&LocalLight> {
        light::find_local(&self.lights, key)
    }

    /// 按 key 改一盏局部光源；key 不存在时返回 `None`（不新建，避免拼错名字后
    /// 悄悄多出一盏灯）。
    pub fn with_local_light<F>(&mut self, key: &str, f: F) -> bool
    where
        F: FnOnce(&mut LocalLight),
    {
        light::with_local(&mut self.lights, key, f).is_some()
    }

    /// 清空全部局部光源。
    pub fn clear_local_lights(&mut self) -> usize {
        let n = self.lights.len();
        self.lights.clear();
        n
    }

    // ---------- 遮挡剔除（扩展二十二） ----------

    /// 遮挡剔除总开关（脚本 `set_occlusion_enabled`）。
    ///
    /// 关掉后渲染层整帧跳过遮挡判定：不读回深度、不建金字塔，提交序列退化成
    /// 「只有视锥剔除」——也就是加这个功能之前的样子。参数（偏置 / tile 等）保留，
    /// 再打开即按原参数恢复。返回切换后的状态。
    pub fn set_occlusion_enabled(&mut self, on: bool) -> bool {
        self.occlusion.enabled = on;
        self.occlusion.enabled
    }

    /// 当前是否启用遮挡剔除。
    pub fn occlusion_enabled(&self) -> bool {
        self.occlusion.enabled
    }

    /// 覆盖式写入整段遮挡剔除参数（脚本 / RPC 用），写入前统一收敛。
    pub fn set_occlusion(&mut self, settings: OcclusionSettings) -> OcclusionSettings {
        self.occlusion = settings.sanitized();
        self.occlusion
    }

    // ---------- HDR 立方体反射探针（扩展二十三） ----------

    /// 覆盖式写入整段探针配置，写入前统一收敛（脚本 / RPC / 场景 JSON 共用入口）。
    ///
    /// 返回写入后的配置；传入的路径全是空白时配置仍在，只是
    /// [`EnvironmentMapSettings::is_active`] 为 false，渲染层退回解析 IBL。
    pub fn set_environment_map(
        &mut self,
        settings: EnvironmentMapSettings,
    ) -> EnvironmentMapSettings {
        let settings = settings.sanitized();
        self.environment_map = Some(settings.clone());
        settings
    }

    /// 探针总开关（脚本 `set_env_map_enabled`）。关掉后路径保留，渲染层退回解析 IBL。
    pub fn set_environment_map_enabled(&mut self, on: bool) -> bool {
        let mut settings = self.environment_map.clone().unwrap_or_default();
        settings.enabled = on;
        self.environment_map = Some(settings.sanitized());
        on
    }

    /// 当前是否有**生效中**的 HDR 探针（开关打开且来源有效）。
    pub fn environment_map_active(&self) -> bool {
        self.environment_map
            .as_ref()
            .map(EnvironmentMapSettings::is_active)
            .unwrap_or(false)
    }

    /// 移除探针配置，彻底退回解析 IBL（脚本 `clear_env_map`）。
    pub fn clear_environment_map(&mut self) -> bool {
        self.environment_map.take().is_some()
    }

    /// 来源标签（`none` / `disabled` / `equirect` / `faces`），供日志与编辑器面板显示。
    pub fn env_map_source(&self) -> String {
        self.environment_map
            .as_ref()
            .map(EnvironmentMapSettings::source_label)
            .unwrap_or("none")
            .to_string()
    }

    // ---------- 生命周期 ----------

    /// 生成物件。重名会返回 `Err`，避免脚本里悄悄覆盖掉已有物件。
    pub fn spawn(&mut self, desc: ObjectDesc) -> Result<NodeId, String> {
        if desc.name.is_empty() {
            return Err("spawn 需要一个非空 name".to_string());
        }
        if self.name_index.contains_key(&desc.name) {
            return Err(format!("物件 `{}` 已存在", desc.name));
        }

        let id = self.next_id;
        self.next_id += 1;

        let mut node = Node::new(id, desc.name.clone());
        node.mesh = desc
            .mesh
            .as_deref()
            .and_then(MeshRef::parse)
            .unwrap_or_default();
        if let Some(p) = desc.position {
            node.transform.position = Vec3::from(p);
        }
        if let Some(r) = desc.rotation {
            node.transform.set_euler_deg(Vec3::from(r));
        }
        if let Some(s) = desc.scale {
            node.transform.scale = Vec3::from(s);
        }
        if let Some(c) = desc.color {
            node.color = Vec4::from(c);
        }
        node.visible = desc.visible.unwrap_or(true);
        node.physics = desc.physics;
        // 场景文件里的材质在写入前收敛：NaN / inf / 超界值不进场景
        node.material = desc.material.map(|m| m.sanitized());

        if let Some(parent_name) = desc.parent {
            let pid = *self
                .name_index
                .get(&parent_name)
                .ok_or_else(|| format!("父物件 `{parent_name}` 不存在"))?;
            node.parent = Some(pid);
            if let Some(parent) = self.nodes.get_mut(&pid) {
                parent.children.push(id);
            }
        }

        self.name_index.insert(desc.name, id);
        self.order.push(id);
        self.nodes.insert(id, node);
        self.touch_layout();
        Ok(id)
    }

    /// 按名字移除物件（含子物件）。
    pub fn despawn(&mut self, name: &str) -> bool {
        match self.name_index.get(name).copied() {
            Some(id) => {
                self.despawn_id(id);
                true
            }
            None => false,
        }
    }

    pub fn despawn_id(&mut self, id: NodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        self.name_index.remove(&node.name);
        self.order.retain(|n| *n != id);
        self.touch_layout();

        // 从父节点摘除
        if let Some(pid) = node.parent {
            if let Some(parent) = self.nodes.get_mut(&pid) {
                parent.children.retain(|c| *c != id);
            }
        }
        // 递归移除子物件
        for child in node.children {
            self.despawn_id(child);
        }

        // P4：蒙皮实例引用节点，节点没了就顺手剪掉，避免悬空 NodeId
        if !self.skins.is_empty()
            && (self.skins.iter().any(|s| s.mesh_node == id)
                || self
                    .skins
                    .iter()
                    .any(|s| s.bindings.iter().any(|b| b.node == id)))
        {
            self.prune_skins();
        }

        // 扩展三十六：形变绑定同样指向网格节点，一并剪掉
        if !self.morphs.is_empty() && self.morphs.iter().any(|m| m.mesh_node == id) {
            self.prune_morphs();
        }
    }

    /// 清空所有物件（脚本热重载时用）。
    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
        self.order.clear();
        self.name_index.clear();
        // 蒙皮实例全部指向已清空的节点，一并丢弃
        self.skins.clear();
        // 形变绑定同理
        self.morphs.clear();
    }

    // ---------- 查询 ----------

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn find(&self, name: &str) -> Option<&Node> {
        self.name_index.get(name).and_then(|id| self.nodes.get(id))
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Node> {
        let id = *self.name_index.get(name)?;
        self.nodes.get_mut(&id)
    }

    pub fn find_id(&self, name: &str) -> Option<NodeId> {
        self.name_index.get(name).copied()
    }

    // ---------- PBR 材质 ----------

    /// 读取物件当前生效的材质。
    ///
    /// 物件不存在返回 `None`；物件存在但没声明材质时返回缺省材质
    /// （与渲染侧「回退到网格资产材质 / 默认材质」的可见结果一致）。
    pub fn material_of(&self, name: &str) -> Option<SurfaceMaterial> {
        self.find(name)
            .map(|n| n.material.unwrap_or_default().sanitized())
    }

    /// 整体写入物件材质。写入前统一收敛（NaN / inf / 超界值不进场景）。
    ///
    /// 返回 `false` 表示物件不存在。
    pub fn set_material(&mut self, name: &str, material: SurfaceMaterial) -> bool {
        match self.find_mut(name) {
            Some(node) => {
                node.material = Some(material.sanitized());
                true
            }
            None => false,
        }
    }

    /// 只改 metallic / roughness，保留已有 emissive。
    pub fn set_material_params(&mut self, name: &str, metallic: f32, roughness: f32) -> bool {
        let mut material = self.material_of(name).unwrap_or_default();
        material.metallic = metallic;
        material.roughness = roughness;
        self.set_material(name, material)
    }

    /// 只改自发光（线性 RGB），保留 metallic / roughness。
    pub fn set_emissive(&mut self, name: &str, emissive: [f32; 3]) -> bool {
        let mut material = self.material_of(name).unwrap_or_default();
        material.emissive = emissive;
        self.set_material(name, material)
    }

    /// 撤销物件级材质覆盖，回退到网格资产 / 默认材质。返回 `false` 表示物件不存在。
    pub fn clear_material(&mut self, name: &str) -> bool {
        match self.find_mut(name) {
            Some(node) => {
                node.material = None;
                true
            }
            None => false,
        }
    }

    /// 按名字前缀查找，返回名字列表。
    pub fn find_all(&self, prefix: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.name.starts_with(prefix))
            .map(|n| n.name.clone())
            .collect();
        out.sort();
        out
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 按插入顺序遍历所有节点。
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().filter_map(move |id| self.nodes.get(id))
    }

    /// 场景结构的版本号（扩展二十五）：只在增删节点时前进。
    pub fn layout_version(&self) -> u64 {
        self.layout_version
    }

    /// 结构变了：推进版本号，让渲染层缓存的层级树失效。
    fn touch_layout(&mut self) {
        self.layout_version = self.layout_version.wrapping_add(1);
    }

    // ---------- 变换 ----------

    /// 重算世界矩阵。每帧在脚本执行后、渲染前调用一次。
    pub fn update_world_transforms(&mut self) {
        let ids: Vec<NodeId> = self.order.clone();
        let mut cache: HashMap<NodeId, Mat4> = HashMap::with_capacity(ids.len());

        for id in ids {
            let world = self.resolve_world(id, 0, &mut cache);
            if let Some(node) = self.nodes.get_mut(&id) {
                node.world = world;
                node.world_normal = crate::transform::world_normal_matrix(world);
            }
        }

        // P4：世界矩阵一刷新就跟着求解蒙皮矩阵，渲染层拿到的永远是本帧结果，
        // 调用方无需记得额外多做一步。
        self.update_skinning();
    }

    fn resolve_world(&self, id: NodeId, depth: u32, cache: &mut HashMap<NodeId, Mat4>) -> Mat4 {
        if let Some(m) = cache.get(&id) {
            return *m;
        }
        let Some(node) = self.nodes.get(&id) else {
            return Mat4::IDENTITY;
        };
        let local = node.transform.local_matrix();
        let world = match node.parent {
            Some(p) if depth < 64 && p != id => self.resolve_world(p, depth + 1, cache) * local,
            _ => local,
        };
        cache.insert(id, world);
        world
    }

    // ---------- 蒙皮（P4） ----------

    /// 登记一个蒙皮实例，返回其在 `Scene::skins` 中的下标。
    ///
    /// 约定：调用前场景节点应处于**静止姿态**（未推进动画），
    /// 关节的 `rest_world` 需要由调用方用当前世界矩阵填好（见 `add_joint`）。
    pub fn add_skin(&mut self, skin: SkinInstance) -> usize {
        self.skins.push(skin);
        self.skins.len() - 1
    }

    /// 按网格节点找蒙皮实例。
    pub fn skin_of(&self, mesh_node: NodeId) -> Option<&SkinInstance> {
        self.skins.iter().find(|s| s.mesh_node == mesh_node)
    }

    // ---------- 顶点形变（扩展三十六） ----------

    /// 登记一条形变绑定（同一网格节点重复登记时覆盖旧权重），返回其下标。
    ///
    /// 渲染层每帧从 `Scene::morphs` 取权重写进实例缓冲，因此脚本改权重后
    /// 下一帧就生效，不需要重建缓冲或重新上传网格。
    pub fn add_morph(&mut self, morph: MorphInstance) -> usize {
        if let Some(index) = self
            .morphs
            .iter()
            .position(|m| m.mesh_node == morph.mesh_node)
        {
            self.morphs[index] = morph;
            return index;
        }
        self.morphs.push(morph);
        self.morphs.len() - 1
    }

    /// 按网格节点找形变绑定。
    pub fn morph_of(&self, mesh_node: NodeId) -> Option<&MorphInstance> {
        self.morphs.iter().find(|m| m.mesh_node == mesh_node)
    }

    /// 按网格节点改一处权重。节点没有绑定、下标越界或权重非有限值都返回 `false`。
    pub fn set_morph_weight(&mut self, mesh_node: NodeId, index: usize, weight: f32) -> bool {
        match self.morphs.iter_mut().find(|m| m.mesh_node == mesh_node) {
            Some(morph) => morph.set_weight(index, weight),
            None => false,
        }
    }

    /// 按物件名改一处权重（脚本侧便利入口）。
    pub fn set_morph_weight_by_name(&mut self, name: &str, index: usize, weight: f32) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        self.set_morph_weight(node, index, weight)
    }

    /// 把某个物件名下的全部形变权重清零。返回是否真的找到并清过。
    pub fn reset_morph(&mut self, name: &str) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => {
                morph.reset();
                true
            }
            None => false,
        }
    }

    /// 按物件名登记形变绑定（脚本侧便利入口）。
    ///
    /// `target_count` 是该网格实际存在的形状数，权重只对前这么多个分量有意义；
    /// 传 0 表示「这个网格没有形状」，此时不建绑定、直接返回 `false`。
    /// 已存在的绑定会被覆盖，权重回到全 0。
    pub fn bind_morph_by_name(&mut self, name: &str, target_count: usize) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        if target_count == 0 {
            return false;
        }
        self.add_morph(MorphInstance::new(node, target_count));
        true
    }

    /// 按物件名读一处权重；没有绑定或下标越界时返回 `None`。
    pub fn morph_weight_by_name(&self, name: &str, index: usize) -> Option<f32> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.weight_of(index)
    }

    /// 按物件名的绑定形状数；没有绑定返回 `None`。
    pub fn morph_target_count(&self, name: &str) -> Option<usize> {
        let node = self.find_id(name)?;
        self.morph_of(node).map(|m| m.target_count)
    }

    /// 扩展三十八：把物件名的第 `index` 个形状划进互斥组 / 从组里取出。
    ///
    /// 同一组内的形状本帧权重之和超过 1 时，渲染层会等比压回 1 —— 用来表达
    /// 「张嘴和闭嘴不能同时 100%」这类表情互斥。`group` 传
    /// [`NO_EXCLUSIVE_GROUP`] 表示取出。返回是否找到绑定且下标合法。
    pub fn set_morph_exclusive_by_name(&mut self, name: &str, index: usize, group: u8) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        let slot = if group == NO_EXCLUSIVE_GROUP {
            None
        } else {
            Some(group)
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => morph.set_exclusive_group(index, slot),
            None => false,
        }
    }

    /// 扩展三十八：读某个形状所在的互斥组；没有绑定 / 下标越界返回 `None`。
    pub fn morph_exclusive_by_name(&self, name: &str, index: usize) -> Option<u8> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.exclusive_group_of(index)
    }

    /// 扩展三十八：读归一化之后的权重（脚本可据此断言「被压回 1」）。
    ///
    /// 与渲染层剔除用的是**同一个纯函数**，因此脚本里看到的数就是进缓冲的数。
    pub fn morph_normalized_weight_by_name(&self, name: &str, index: usize) -> Option<f32> {
        let node = self.find_id(name)?;
        let morph = self.morph_of(node)?;
        if index >= morph.target_count {
            return None;
        }
        let (weights, _) = morph.normalized_weights();
        weights.get(index).copied()
    }

    /// 扩展四十：读某个实例的生效形状掩码；没有绑定返回 `None`。
    ///
    /// 返回值已按 `target_count` 裁剪 —— 脚本写进去的越界位不会回显。
    pub fn morph_mask_by_name(&self, name: &str) -> Option<u32> {
        let node = self.find_id(name)?;
        Some(self.morph_of(node)?.shape_mask())
    }

    /// 扩展四十：整块替换形状掩码，返回裁剪后的生效值；没有绑定返回 `None`。
    pub fn set_morph_mask_by_name(&mut self, name: &str, mask: u32) -> Option<u32> {
        let node = self.find_id(name)?;
        let morph = self.morphs.iter_mut().find(|m| m.mesh_node == node)?;
        Some(morph.set_shape_mask(mask))
    }

    /// 扩展四十：开 / 关单个形状；没有绑定或下标越界返回 `false`。
    pub fn set_morph_shape_enabled_by_name(
        &mut self,
        name: &str,
        index: usize,
        enabled: bool,
    ) -> bool {
        let Some(node) = self.find_id(name) else {
            return false;
        };
        match self.morphs.iter_mut().find(|m| m.mesh_node == node) {
            Some(morph) => morph.set_shape_enabled(index, enabled),
            None => false,
        }
    }

    /// 扩展四十：读某个形状是否启用；没有绑定或下标越界返回 `None`。
    pub fn morph_shape_enabled_by_name(&self, name: &str, index: usize) -> Option<bool> {
        let node = self.find_id(name)?;
        self.morph_of(node)?.shape_enabled(index)
    }

    /// 丢弃指向已删除节点的形变绑定（与 `prune_skins` 同策）。
    pub fn prune_morphs(&mut self) {
        if self.morphs.is_empty() {
            return;
        }
        self.morphs
            .retain(|m| self.nodes.contains_key(&m.mesh_node));
    }

    /// 只沿局部链求 `anchor⁻¹ · joint`（不经过任何世界矩阵）。
    ///
    /// 要求 `anchor` 是 `joint` 的祖先；成立时结果为纯局部矩阵累乘，
    /// 与资产实例变换 `X` 无关 —— 整体刚体运动下逐位不变，脏段上传因此不会误判。
    ///
    /// 返回 `None` 表示 `anchor` 不在 `joint` 的父链上、父链成环或超过深度上限。
    pub fn relative_transform_from(&self, anchor: NodeId, joint: NodeId) -> Option<Mat4> {
        const MAX_DEPTH: usize = 64;
        let mut chain: Vec<Mat4> = Vec::new();
        let mut cur = joint;
        let mut reached = false;
        for _ in 0..MAX_DEPTH {
            if cur == anchor {
                reached = true;
                break;
            }
            let node = self.nodes.get(&cur)?;
            chain.push(node.transform.local_matrix());
            match node.parent {
                Some(p) if p != cur => cur = p,
                _ => return None,
            }
        }
        if !reached {
            return None;
        }
        // `chain` 是「关节自身 → anchor 的直接子节点」，倒过来才是从根往下的累乘顺序。
        let mut rel = Mat4::IDENTITY;
        for m in chain.iter().rev() {
            rel *= *m;
        }
        Some(rel)
    }

    /// 节点所在父子树的顶点（一路沿 `parent` 上溯到的根）。
    pub fn tree_root(&self, node: NodeId) -> Option<NodeId> {
        const MAX_DEPTH: usize = 64;
        let mut cur = node;
        for _ in 0..MAX_DEPTH {
            let n = self.nodes.get(&cur)?;
            match n.parent {
                Some(p) if p != cur => cur = p,
                _ => return Some(cur),
            }
        }
        None
    }

    /// 用当前世界矩阵给第 `index` 个蒙皮固定「相对基准」（骨骼树顶点 + `frame_local`）。
    ///
    /// 必须在静止姿态调用；返回 `false` 表示没有可用关节或父链异常。
    /// [`Self::update_skinning`] 会对尚未固定的蒙皮自动调一次（惰性兜底）。
    pub fn align_skin_frame(&mut self, index: usize) -> bool {
        let (mesh_node, first) = match self.skins.get(index) {
            Some(skin) => match skin.bindings.first() {
                Some(b) => (skin.mesh_node, b.node),
                None => return false,
            },
            None => return false,
        };
        let Some(anchor) = self.tree_root(first) else {
            return false;
        };
        let (Some(anchor_world), Some(mesh_world)) = (
            self.nodes.get(&anchor).map(|n| n.world),
            self.nodes.get(&mesh_node).map(|n| n.world),
        ) else {
            return false;
        };
        let Some(skin) = self.skins.get_mut(index) else {
            return false;
        };
        skin.bind_frame(anchor, anchor_world, mesh_world);
        true
    }

    /// 关节相对网格节点的变换（等价于 `mesh_world⁻¹ * joint_world`），
    /// 但只沿两者之间的**局部**链路累乘，不经过资产实例变换。
    ///
    /// 走世界矩阵相减时，`X⁻¹ · X` 在浮点上会留下末位抖动：实例整体旋转、骨骼没动，
    /// 关节矩阵却位级变了（扩展三十二的脏段上传因此失效）。沿局部链求相对变换后，
    /// 结果与实例变换无关，「刚体整体运动」也能保持位级稳定。
    ///
    /// 返回 `None` 表示两者不在同一父子树内、父链成环或超过深度上限，
    /// 调用方应回退到世界矩阵相减。
    ///
    /// 注意：真实导入路径里网格节点是独立根、骨骼挂在资产实例根下，两者**没有**
    /// 公共祖先，这时本方法返回 `None`；蒙皮求解走 [`Self::relative_transform_from`]
    /// 加 `frame_local` 常量的路径（见 `SkinInstance::frame_anchor`）。
    pub fn relative_transform(&self, mesh: NodeId, joint: NodeId) -> Option<Mat4> {
        const MAX_DEPTH: usize = 64;
        // 关节上溯链（含自身），用于找公共祖先。
        let mut up: Vec<NodeId> = Vec::new();
        let mut cur = joint;
        for _ in 0..MAX_DEPTH {
            self.nodes.get(&cur)?;
            up.push(cur);
            match self.nodes.get(&cur)?.parent {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        // 从网格节点上溯，找到第一个落在关节链上的节点即公共祖先。
        let mut ancestor = None;
        let mut cur = mesh;
        for _ in 0..MAX_DEPTH {
            if let Some(pos) = up.iter().position(|id| *id == cur) {
                ancestor = Some((cur, pos));
                break;
            }
            match self.nodes.get(&cur)?.parent {
                Some(p) if p != cur => cur = p,
                _ => break,
            }
        }
        let (ancestor, joint_pos) = ancestor?;

        // 关节侧：从紧邻公共祖先那层往下累乘到关节自身。
        let mut rel = Mat4::IDENTITY;
        for i in (0..joint_pos).rev() {
            let id = up[i];
            rel *= self.nodes.get(&id)?.transform.local_matrix();
        }
        // 网格侧：公共祖先到网格节点之间的链取逆，左乘上去即可抵消。
        if ancestor != mesh {
            let mut chain: Vec<NodeId> = Vec::new();
            let mut cur = mesh;
            for _ in 0..MAX_DEPTH {
                chain.push(cur);
                match self.nodes.get(&cur)?.parent {
                    Some(p) if p != cur && p != ancestor => cur = p,
                    _ => break,
                }
            }
            let mut to_mesh = Mat4::IDENTITY;
            for i in (0..chain.len()).rev() {
                to_mesh *= self.nodes.get(&chain[i])?.transform.local_matrix();
            }
            rel = to_mesh.inverse() * rel;
        }
        Some(rel)
    }

    /// 求解所有蒙皮实例的关节矩阵。
    ///
    /// `update_world_transforms` 会自动调用本方法，一般不需要手动调。
    /// 关节节点被销毁时该关节退化为单位矩阵，不会让整块网格塌到原点。
    pub fn update_skinning(&mut self) {
        if self.skins.is_empty() {
            return;
        }
        // 惰性兜底：调用方没显式固定基准的蒙皮（例如脚本 API 直接建蒙皮），
        // 在第一次求解时以当前姿态为静止姿态固定一次，之后恒定复用。
        let pending_frames: Vec<usize> = (0..self.skins.len())
            .filter(|&i| self.skins[i].frame_anchor.is_none())
            .collect();
        for i in pending_frames {
            self.align_skin_frame(i);
        }
        let worlds: HashMap<NodeId, Mat4> = self
            .nodes
            .iter()
            .map(|(id, node)| (*id, node.world))
            .collect();
        // 先按「基准节点 → 关节的局部链」求解（读借用），再统一写回 skins，
        // 避免实例整体变换把 X⁻¹·X 的浮点抖动带进关节矩阵。
        let mut pending: Vec<Vec<Mat4>> = Vec::with_capacity(self.skins.len());
        for skin in &self.skins {
            let model = worlds
                .get(&skin.mesh_node)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            let inv_model = model.inverse();
            // 常量：网格局部空间 ← 骨骼树顶点空间。位级恒定，不会随实例变换抖动。
            let inv_frame = skin.frame_local.inverse();
            let mut rels = Vec::with_capacity(skin.bindings.len());
            for b in &skin.bindings {
                let rel = skin
                    .frame_anchor
                    .and_then(|anchor| self.relative_transform_from(anchor, b.node))
                    .map(|chain| inv_frame * chain)
                    .unwrap_or_else(|| {
                        // 断链（多个骨架根 / 节点被销毁）：回退世界矩阵相减，
                        // 结果正确但位级不稳定，脏段上传会保守地判全脏。
                        let now = worlds.get(&b.node).copied().unwrap_or(Mat4::IDENTITY);
                        inv_model * now
                    });
                rels.push(rel);
            }
            pending.push(rels);
        }
        for (skin, rels) in self.skins.iter_mut().zip(pending.iter()) {
            skin.solve_from_relative(rels);
        }
    }

    /// 移除绑定了已销毁节点的蒙皮实例（网格节点或任一关节没了就整条丢）。
    pub fn prune_skins(&mut self) {
        if self.skins.is_empty() {
            return;
        }
        let alive: std::collections::HashSet<NodeId> = self.nodes.keys().copied().collect();
        self.skins.retain(|s| {
            alive.contains(&s.mesh_node) && s.bindings.iter().all(|b| alive.contains(&b.node))
        });
    }

    /// 蒙皮统计：`(实例数, 关节总数)`，供日志与验收使用。
    pub fn skin_stats(&self) -> (usize, usize) {
        (
            self.skins.len(),
            self.skins.iter().map(|s| s.joint_count()).sum(),
        )
    }

    // ---------- 动画（P3） ----------

    /// 推进所有动画实例并把采样结果写回节点局部变换。
    ///
    /// 必须在 `update_world_transforms` 之前调用：动画改的是局部 TRS，世界矩阵由
    /// 场景图统一合成。已销毁节点的绑定会被就地剔除，避免每帧都做无效查找。
    pub fn update_animations(&mut self, dt: f32) {
        if self.animations.is_empty() {
            return;
        }

        // 先算出所有待写回项，再统一落到节点上，避免同时可变借用 animations 与 nodes
        let mut writes: Vec<(NodeId, Transform)> = Vec::new();
        for inst in self.animations.iter_mut() {
            inst.advance(dt);
            let time = inst.sample_time();
            let weight = inst.fade_weight();
            let fade_time = inst.fade_sample_time();

            // 第一遍：逐绑定求出主片段姿态（`None` = 该节点没有主片段轨道）
            let mut poses: Vec<Option<Transform>> = Vec::with_capacity(inst.bindings.len());
            for (i, b) in inst.bindings.iter().enumerate() {
                let fresh = if b.track == crate::anim::NO_TRACK {
                    None
                } else {
                    inst.clip
                        .tracks
                        .get(b.track as usize)
                        .and_then(|t| t.sample(time))
                };
                // 过渡中的旧片段：只知道与绑定的对应关系，缺轨道时下标为 NO_TRACK
                let old = match (inst.fade.as_ref(), fade_time) {
                    (Some(fade), Some(t)) => fade
                        .tracks
                        .get(i)
                        .copied()
                        .filter(|t| *t != crate::anim::NO_TRACK)
                        .and_then(|idx| fade.clip.tracks.get(idx as usize))
                        .and_then(|track| track.sample(t)),
                    _ => None,
                };
                poses.push(match (fresh, old) {
                    // 新片段缺该轨道：过渡期间由旧片段继续驱动，结束后停在末姿态
                    (None, Some(o)) => Some(o),
                    (Some(n), Some(o)) => Some(crate::anim::blend_transform(&o, &n, weight)),
                    (Some(n), None) => Some(n),
                    (None, None) => None,
                });
            }

            // 第二遍：混合树层按挂载顺序叠加到主姿态上
            //
            // 扩展四十五：挂了混合空间的层先用当前参数重算层内片段权重，再照旧叠加。
            // 没挂空间的层这一步是空操作，权重仍是脚本/场景写死的值——零变化验收靠这一行保证。
            // 扩展四十六：开了相位锁的层改用「归一化相位 × 片段时长」采样每路片段，
            // `LayerSync::Off` 的层仍取同一个层时间，逐位不变。
            // 扩展四十七：相位锁层的相位可以跟随别的层（上下半身对齐），
            // 所以先按全层视角把每层的有效相位算好，再进可变借用的混合循环。
            // 没挂源层 / 没开相位锁时 effective_phase 就是本层 phase()，逐位不变。
            let sync_phases: Vec<Option<f32>> = (0..inst.layers.len())
                .map(|i| match inst.layers[i].sync {
                    LayerSync::Off => None,
                    LayerSync::Phase => Some(effective_phase(&inst.layers, i)),
                })
                .collect();
            for (layer_index, layer) in inst.layers.iter_mut().enumerate() {
                layer.apply_blend_space();
                let layer_weight = layer.effective_weight();
                if layer_weight <= 0.0 {
                    continue;
                }
                let layer_time = layer.sample_time();
                let sync_phase = sync_phases[layer_index];
                for (i, b) in inst.bindings.iter().enumerate() {
                    let effective = layer_weight * layer.mask.weight_for(&b.path);
                    if effective <= 0.0 {
                        continue;
                    }
                    // 扩展五十：加法层与覆盖层吃同一份份额，但算的不是一回事——
                    // 覆盖层要把多路姿态按份额平均成"那个姿态"，加法层要把多路**增量**按份额叠加。
                    // 故加法层走 sample_layer_pose_additive（单路时零算术，逐位不变）。
                    let sample = match (sync_phase, layer.mode) {
                        (Some(phase), LayerMode::Additive) => sample_layer_pose_additive_synced(
                            &layer.clips,
                            phase,
                            b.node_index,
                            &b.path,
                        ),
                        (Some(phase), LayerMode::Override) => {
                            sample_layer_pose_synced(&layer.clips, phase, b.node_index, &b.path)
                        }
                        (None, LayerMode::Additive) => sample_layer_pose_additive(
                            &layer.clips,
                            layer_time,
                            b.node_index,
                            &b.path,
                        ),
                        (None, LayerMode::Override) => {
                            sample_layer_pose(&layer.clips, layer_time, b.node_index, &b.path)
                        }
                    };
                    let Some(sample) = sample else {
                        continue;
                    };
                    poses[i] = Some(match (layer.mode, poses[i]) {
                        // 主片段没有该节点轨道：层姿态直接作为绝对姿态
                        (_, None) => sample.pose,
                        (LayerMode::Additive, Some(base)) => {
                            apply_additive(&base, &sample.reference, &sample.pose, effective)
                        }
                        (LayerMode::Override, Some(base)) => {
                            blend_override(&base, &sample.pose, effective)
                        }
                    });
                }
            }

            for (i, b) in inst.bindings.iter().enumerate() {
                if let Some(transform) = poses[i] {
                    writes.push((b.node, transform));
                }
            }
        }

        if writes.is_empty() {
            return;
        }
        for (id, transform) in writes {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.transform = transform;
            }
        }
    }

    /// 移除绑定了已销毁节点的死绑定（销毁节点后调用，保持实例表干净）。
    pub fn prune_anim_bindings(&mut self) {
        if self.animations.is_empty() {
            return;
        }
        let alive: std::collections::HashSet<NodeId> = self.nodes.keys().copied().collect();
        for inst in &mut self.animations {
            inst.bindings.retain(|b| alive.contains(&b.node));
        }
    }

    pub fn add_anim_instance(&mut self, instance: AnimInstance) -> usize {
        self.animations.push(instance);
        self.animations.len() - 1
    }

    pub fn anim_names(&self) -> Vec<String> {
        self.animations.iter().map(|a| a.name.clone()).collect()
    }

    /// 按名字找实例；`name` 为空时取第一个（脚本里 `play_anim()` 的默认行为）。
    pub fn anim_instance(&self, name: &str) -> Option<&AnimInstance> {
        if name.is_empty() {
            return self.animations.first();
        }
        self.animations
            .iter()
            .find(|a| a.name == name || a.clip.name.eq_ignore_ascii_case(name))
    }

    pub fn anim_instance_mut(&mut self, name: &str) -> Option<&mut AnimInstance> {
        if name.is_empty() {
            return self.animations.first_mut();
        }
        self.animations
            .iter_mut()
            .find(|a| a.name == name || a.clip.name.eq_ignore_ascii_case(name))
    }

    /// 播放（从当前时间继续）。返回是否命中实例。
    pub fn play_anim(&mut self, name: &str) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.playing = true;
                true
            }
            None => false,
        }
    }

    /// 暂停并保留当前时间。
    pub fn stop_anim(&mut self, name: &str) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.playing = false;
                true
            }
            None => false,
        }
    }

    /// 设置播放速度（负值为倒放）。
    pub fn set_anim_speed(&mut self, name: &str, speed: f32) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.speed = speed;
                true
            }
            None => false,
        }
    }

    /// 跳转到指定时间（秒）。超出片段范围时按循环/钳制规则处理。
    pub fn set_anim_time(&mut self, name: &str, time: f32) -> bool {
        match self.anim_instance_mut(name) {
            Some(inst) => {
                inst.time = time;
                true
            }
            None => false,
        }
    }

    /// 查询播放时间（秒），未命中返回 `None`。
    pub fn anim_time(&self, name: &str) -> Option<f32> {
        self.anim_instance(name).map(|a| a.sample_time())
    }

    /// 登记一份可切换的片段，键为 `"<资产名>/<片段名>"`。
    pub fn register_anim_clip(&mut self, key: impl Into<String>, clip: std::sync::Arc<AnimClip>) {
        self.anim_clips.insert(key.into(), clip);
    }

    /// 某个资产（实例名前缀）下所有可切换片段名，按名称排序。
    pub fn anim_clip_names(&self, prefix: &str) -> Vec<String> {
        let head = format!("{prefix}/");
        let mut out: Vec<String> = self
            .anim_clips
            .keys()
            .filter_map(|k| k.strip_prefix(&head).map(str::to_string))
            .collect();
        out.sort();
        out
    }

    /// 把实例切换到同一资产的另一个片段（如 idle → run → atk）。
    ///
    /// 实例名形如 `<资产名>/<片段名>`，片段名从该资产已登记的片段里按名字（大小写不敏感）查找，
    /// 因此脚本可以写 `set_anim_clip("tirger/idle_loop", "run_loop")`。
    pub fn set_anim_clip(&mut self, instance: &str, clip_name: &str) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                inst.set_clip(clip);
                true
            }
            None => false,
        }
    }

    /// 带交叉淡入地切换片段，`duration` 为过渡秒数（`<= 0` 等价 [`Self::set_anim_clip`]）。
    pub fn set_anim_clip_fade(&mut self, instance: &str, clip_name: &str, duration: f32) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                inst.cross_fade(clip, duration);
                true
            }
            None => false,
        }
    }

    /// 交叉淡入剩余进度：新片段当前权重（`1.0` 表示没有过渡正在进行）。
    pub fn anim_fade_weight(&self, instance: &str) -> Option<f32> {
        self.anim_instance(instance).map(|a| a.fade_weight())
    }

    /// 过渡中的旧片段名；没有过渡时返回空串（脚本日志用）。
    pub fn anim_fade_source(&self, instance: &str) -> String {
        self.anim_instance(instance)
            .and_then(|a| a.fade.as_ref().map(|f| f.clip.name.clone()))
            .unwrap_or_default()
    }

    // ---------- 动画混合树（扩展十八 A） ----------

    /// 给实例挂一层（同名则整层覆盖）。
    ///
    /// 层与主片段互不影响：主片段负责「当前动作」，层负责同时进行的附加动作
    /// （上半身攻击、呼吸叠加）。超过 [`MAX_ANIM_LAYERS`] 返回 `false`。
    pub fn add_anim_layer(&mut self, instance: &str, layer: AnimLayer) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        if let Some(existing) = anim_layer_mut(inst, &layer.name) {
            *existing = layer;
            return true;
        }
        if inst.layers.len() >= MAX_ANIM_LAYERS {
            return false;
        }
        inst.layers.push(layer);
        true
    }

    /// 按片段名直接建层（脚本 / 编辑器入口，省去调用方自己取 `Arc<AnimClip>`）。
    ///
    /// 片段名按「实例名前缀」解析；同名层覆盖。
    pub fn add_anim_layer_by_clip(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
        mode: LayerMode,
    ) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        self.add_anim_layer(
            instance,
            AnimLayer::new(layer, clip, weight).with_mode(mode),
        )
    }

    /// 层名列表（按挂载顺序，顺序即叠加顺序）。
    pub fn anim_layer_names(&self, instance: &str) -> Vec<String> {
        self.anim_instance(instance)
            .map(|inst| inst.layers.iter().map(|l| l.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 层数量；实例不存在时返回 `None`。
    pub fn anim_layer_count(&self, instance: &str) -> Option<usize> {
        self.anim_instance(instance).map(|inst| inst.layers.len())
    }

    /// 按名字取层；`layer` 为空时取第一层。
    pub fn anim_layer(&self, instance: &str, layer: &str) -> Option<&AnimLayer> {
        let inst = self.anim_instance(instance)?;
        if layer.is_empty() {
            return inst.layers.first();
        }
        inst.layers
            .iter()
            .find(|l| l.name == layer || l.name.eq_ignore_ascii_case(layer))
    }

    /// 层当前有效权重（含进行中的过渡）。
    pub fn anim_layer_effective_weight(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer(instance, layer)
            .map(|l| l.effective_weight())
    }

    /// 层信息摘要（脚本日志 / RPC 回显用）。
    pub fn anim_layer_report(&self, instance: &str, layer: &str) -> Option<String> {
        self.anim_layer(instance, layer)
            .map(crate::anim_blend::layer_report)
    }

    /// 直接设定层权重（取消进行中的过渡）。
    pub fn set_anim_layer_weight(&mut self, instance: &str, layer: &str, weight: f32) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.set_weight(weight);
                true
            }
            None => false,
        }
    }

    /// 层权重过渡到 `weight`（`duration <= 0` 等价直接设定）。
    pub fn fade_anim_layer(
        &mut self,
        instance: &str,
        layer: &str,
        weight: f32,
        duration: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.fade_to(weight, duration);
                true
            }
            None => false,
        }
    }

    /// 设置叠加模式（override / additive）。
    pub fn set_anim_layer_mode(&mut self, instance: &str, layer: &str, mode: LayerMode) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.mode = mode;
                true
            }
            None => false,
        }
    }

    /// 设置骨骼遮罩。
    pub fn set_anim_layer_mask(&mut self, instance: &str, layer: &str, mask: BoneMask) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.mask = mask;
                true
            }
            None => false,
        }
    }

    /// 设置层播放速度（负值倒放）。
    pub fn set_anim_layer_speed(&mut self, instance: &str, layer: &str, speed: f32) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.speed = speed;
                true
            }
            None => false,
        }
    }

    // ---------- 层内相位同步（扩展四十六） ----------

    /// 开关层内相位锁：`true` = 各片段按归一化相位缩放采样（走跑不再错拍）。
    ///
    /// 返回 `false` 表示实例 / 层不存在，层状态不变。
    pub fn set_anim_layer_sync(&mut self, instance: &str, layer: &str, enabled: bool) -> bool {
        self.set_anim_layer_sync_strategy(
            instance,
            layer,
            if enabled {
                LayerSync::Phase
            } else {
                LayerSync::Off
            },
        )
    }

    /// 直接指定相位策略（`off` / `phase`）。
    pub fn set_anim_layer_sync_strategy(
        &mut self,
        instance: &str,
        layer: &str,
        sync: LayerSync,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.sync = sync;
                true
            }
            None => false,
        }
    }

    /// 层当前是否开了相位锁。实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync(&self, instance: &str, layer: &str) -> Option<bool> {
        self.anim_layer(instance, layer)
            .map(|l| l.sync == LayerSync::Phase)
    }

    /// 层当前相位（0..1）。实例 / 层不存在返回 `None`。
    pub fn anim_layer_phase(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer(instance, layer).map(|l| l.phase())
    }

    /// 相位同步单行探针（日志用，含跨层源与偏移段）。实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync_probe(&self, instance: &str, layer: &str) -> Option<String> {
        let inst = self.anim_instance(instance)?;
        let (index, phase, source) = self.anim_layer_phase_view(instance, layer)?;
        Some(sync_probe_with(
            &inst.layers[index],
            phase,
            source.as_deref(),
        ))
    }

    // ---------- 相位偏移量与跨层对齐（扩展四十七） ----------

    /// 层下标 + 有效相位 + 源层名（跨层对齐后真正用于采样的相位）。
    ///
    /// 内部用：`Off` 的层不给源层名，免得探针里出现「跟着某层但没生效」的误导。
    fn anim_layer_phase_view(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<(usize, f32, Option<String>)> {
        let inst = self.anim_instance(instance)?;
        let index = if layer.is_empty() {
            if inst.layers.is_empty() {
                return None;
            }
            0
        } else {
            inst.layers
                .iter()
                .position(|l| l.name == layer || l.name.eq_ignore_ascii_case(layer))?
        };
        let phase = effective_phase(&inst.layers, index);
        let source = if inst.layers[index].sync == LayerSync::Phase {
            phase_source(&inst.layers, index).map(|i| inst.layers[i].name.clone())
        } else {
            None
        };
        Some((index, phase, source))
    }

    /// 设某一路片段的相位偏移：`offset` 折进 `[0, 1)`（`-0.25` 等价 `0.75`）。
    ///
    /// `clip` 为空时作用于层内第一路；片段名大小写不敏感。
    /// 返回 `false` 表示实例 / 层 / 片段不存在，片段状态不变。
    pub fn set_anim_layer_clip_phase_offset(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
        offset: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.set_phase_offset(offset);
                true
            }
            None => false,
        }
    }

    /// 读某一路片段的相位偏移（没设过是 0.0）。实例 / 层 / 片段不存在返回 `None`。
    pub fn anim_layer_clip_phase_offset(
        &self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> Option<f32> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        Some(l.clips[index].phase_offset)
    }

    /// 让层的相位跟随 `source` 层（跨层对齐：上下半身各层同一相位起步）。
    ///
    /// 只在层开了相位锁（[`LayerSync::Phase`]）时影响采样；`None` / 空串 = 清除，跟回自己。
    ///
    /// 返回 `false` 表示：实例 / 层不存在、源层不存在、指向自己、或会形成跟随环
    /// （A→B→A）。**失败时层状态原样回滚**，不会留下半截链。
    pub fn set_anim_layer_sync_source(
        &mut self,
        instance: &str,
        layer: &str,
        source: Option<&str>,
    ) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        let Some(index) = anim_layer_index(inst, layer) else {
            return false;
        };
        match source.map(str::trim).filter(|s| !s.is_empty()) {
            None => {
                inst.layers[index].set_sync_source(None);
                true
            }
            Some(name) => {
                if anim_layer_index(inst, name).is_none() {
                    return false;
                }
                let before = inst.layers[index].sync_source.clone();
                inst.layers[index].set_sync_source(Some(name));
                // 自引用 / 成环 / 解析不到：chain 走不出来，回滚
                if phase_source(&inst.layers, index).is_some() {
                    true
                } else {
                    inst.layers[index].sync_source = before;
                    false
                }
            }
        }
    }

    /// 层当前跟随的源层名（没设过返回空串，不返回 `None` 以便脚本直接打印）。
    /// 实例 / 层不存在返回 `None`。
    pub fn anim_layer_sync_source(&self, instance: &str, layer: &str) -> Option<String> {
        self.anim_layer(instance, layer)
            .map(|l| l.sync_source.clone().unwrap_or_default())
    }

    /// 跨层对齐后的**有效相位**（含源层跟随）。实例 / 层不存在返回 `None`。
    ///
    /// 没挂源层时与 [`Self::anim_layer_phase`] 相同。
    pub fn anim_layer_sync_phase(&self, instance: &str, layer: &str) -> Option<f32> {
        self.anim_layer_phase_view(instance, layer)
            .map(|(_, phase, _)| phase)
    }

    // ---------- 片段自带速率与循环标志（扩展四十八） ----------

    /// 设某一路片段的播放速率与循环标志：这一路从此按自己的时间轴走。
    ///
    /// `speed` 负值倒放、`0` 定格首帧、非有限值按原速；`looping = false` 时到末帧停住。
    /// 只在层没开相位锁（[`LayerSync::Off`]）时影响采样——相位锁层的每路时间由相位
    /// 决定，要变速请调层 [`Self::set_anim_layer_speed`]。
    ///
    /// `clip` 为空时作用于层内第一路；片段名大小写不敏感。
    /// 返回 `false` 表示实例 / 层 / 片段不存在，片段状态不变。
    pub fn set_anim_layer_clip_playback(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
        speed: f32,
        looping: bool,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.set_playback(speed, looping);
                true
            }
            None => false,
        }
    }

    /// 清除某一路片段的播放参数：回到「跟着层时间轴走」的默认口径。
    /// 返回 `false` 表示实例 / 层 / 片段不存在。
    pub fn clear_anim_layer_clip_playback(
        &mut self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
            .and_then(|l| layer_clip_mut(l, clip))
        {
            Some(c) => {
                c.clear_playback();
                true
            }
            None => false,
        }
    }

    /// 读某一路片段的播放参数：`None` = 没设过（跟着层时间轴走）。
    pub fn anim_layer_clip_playback(
        &self,
        instance: &str,
        layer: &str,
        clip: &str,
    ) -> Option<(f32, bool)> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        l.clips[index].playback.map(|pb| (pb.speed, pb.looping))
    }

    /// 某一路片段**这一帧实际采样到的时间**（秒，含速率/循环折算）。
    ///
    /// 真机取证用：同一个层时间下，两路的这个值不同就说明速率生效了。
    pub fn anim_layer_clip_time(&self, instance: &str, layer: &str, clip: &str) -> Option<f32> {
        let l = self.anim_layer(instance, layer)?;
        let index = layer_clip_index(l, clip)?;
        l.clip_times().get(index).copied()
    }

    /// 把层的片段替换为指定片段（层内收敛为单片段、权重 1.0，其余属性保留）。
    ///
    /// 片段名按「实例名前缀」解析（`"tirger/idle_loop"` 的实例可只写 `"atk"`）。
    pub fn set_anim_layer_clip(&mut self, instance: &str, layer: &str, clip_name: &str) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.clips = vec![crate::anim_blend::LayerClip::new(clip, 1.0)];
                true
            }
            None => false,
        }
    }

    /// 给层增加一路参与混合的片段（走 / 跑按权重加权）。
    pub fn add_anim_layer_clip(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
    ) -> bool {
        let Some(clip) = self.find_anim_clip(instance, clip_name) else {
            return false;
        };
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.add_clip(clip, weight),
            None => false,
        }
    }

    /// 调整层内某一路片段的权重。
    pub fn set_anim_layer_clip_weight(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        weight: f32,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.set_clip_weight(clip_name, weight),
            None => false,
        }
    }

    /// 移除层内某一路片段（最后一路不可移除）。
    pub fn remove_anim_layer_clip(&mut self, instance: &str, layer: &str, clip_name: &str) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.remove_clip(clip_name),
            None => false,
        }
    }

    /// 移除一整层。
    pub fn remove_anim_layer(&mut self, instance: &str, layer: &str) -> bool {
        let Some(inst) = self.anim_instance_mut(instance) else {
            return false;
        };
        let before = inst.layers.len();
        inst.layers
            .retain(|l| !(l.name == layer || l.name.eq_ignore_ascii_case(layer)));
        inst.layers.len() != before
    }

    /// 清空实例的所有层。
    pub fn clear_anim_layers(&mut self, instance: &str) -> bool {
        match self.anim_instance_mut(instance) {
            Some(inst) => {
                let had = !inst.layers.is_empty();
                inst.layers.clear();
                had
            }
            None => false,
        }
    }

    // ---------- 1D / 2D / 3D 混合空间（扩展四十五 / 四十九） ----------

    /// 给层挂一维混合空间（`param` 是参数名，例如 `"speed"`），样本表清空重建。
    ///
    /// 换维度 / 换参数名等于换了一张图，旧采样点留着只会错配，因此不保留。
    pub fn set_anim_layer_blend_space_1d(
        &mut self,
        instance: &str,
        layer: &str,
        param: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_1d(param))
    }

    /// 给层挂二维混合空间（`x` / `y` 两个参数名），样本表清空重建。
    pub fn set_anim_layer_blend_space_2d(
        &mut self,
        instance: &str,
        layer: &str,
        x: &str,
        y: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_2d(x, y))
    }

    /// 给层挂三维混合空间（`x` / `y` / `z` 三个参数名），样本表清空重建。
    ///
    /// 扩展四十九：把「八向移动 × 三档速度」这种过去要拿两层 2D 叠起来的表达，
    /// 收成一张三维单纯形图（四面体剖分 + 重心插值）。
    pub fn set_anim_layer_blend_space_3d(
        &mut self,
        instance: &str,
        layer: &str,
        x: &str,
        y: &str,
        z: &str,
    ) -> bool {
        self.set_anim_layer_blend_space(instance, layer, BlendSpace::new_3d(x, y, z))
    }

    /// 给层挂一个现成的混合空间（覆盖原有，连同样本一起）。
    pub fn set_anim_layer_blend_space(
        &mut self,
        instance: &str,
        layer: &str,
        space: BlendSpace,
    ) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => {
                l.blend_space = Some(space);
                true
            }
            None => false,
        }
    }

    /// 摘掉层的混合空间。返回 `true` 表示确实摘掉了（层不存在或本来就没挂 → `false`）。
    ///
    /// 层内片段权重**不会**回滚成挂空间之前的值，而是冻结在最后一次由空间算出的结果上
    /// （想改手写值就接着调 `set_anim_layer_clip_weight`）。这样切换时不会突然跳一下。
    pub fn clear_anim_layer_blend_space(&mut self, instance: &str, layer: &str) -> bool {
        match self
            .anim_instance_mut(instance)
            .and_then(|inst| anim_layer_mut(inst, layer))
        {
            Some(l) => l.blend_space.take().is_some(),
            None => false,
        }
    }

    /// 往层的混合空间加一个样本。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 样本被拒
    /// （片段名解析不到 / 坐标重复 / 非有限值 / 超过 16 个上限）。
    ///
    /// 片段名走 `find_anim_clip` 解析（可以只写实例前缀段），入库用**解析后的规范名**，
    /// 保证样本名与层内片段名同一口径，不会被前缀差异坑掉。
    pub fn add_anim_layer_blend_sample(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        at: [f32; 2],
    ) -> Option<bool> {
        // 先解析片段（只读借用到此结束），再取层与空间的可变借用
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.add_sample(c.name.clone(), at),
            None => {
                space.note_rejected();
                false
            }
        })
    }

    /// 设定混合空间参数（一维只认 `x`，`y` 传 0 即可）。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 参数非有限，整条拒绝、保持旧值。
    pub fn set_anim_layer_blend_value(
        &mut self,
        instance: &str,
        layer: &str,
        x: f32,
        y: f32,
    ) -> Option<bool> {
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        layer_ref.blend_space.as_mut().map(|s| s.set_value(x, y))
    }

    /// 往层的三维混合空间加一个样本（扩展四十九）。
    ///
    /// 与二维版同语义：`None` = 层不存在或没挂空间；`Some(false)` = 样本被拒
    /// （片段名解析不到 / 坐标重复 / 非有限值 / 超过上限，上限已由 16 抬到 32）。
    pub fn add_anim_layer_blend_sample_3d(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        at: [f32; 3],
    ) -> Option<bool> {
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.add_sample_3d(c.name.clone(), at),
            None => {
                space.note_rejected();
                false
            }
        })
    }

    /// 设定三维混合空间参数。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 参数非有限，整条拒绝、保持旧值。
    pub fn set_anim_layer_blend_value_3d(
        &mut self,
        instance: &str,
        layer: &str,
        x: f32,
        y: f32,
        z: f32,
    ) -> Option<bool> {
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        layer_ref
            .blend_space
            .as_mut()
            .map(|s| s.set_value_3d(x, y, z))
    }

    /// 混合空间所属维度（`"1d"` / `"2d"` / `"3d"`）。没挂空间返回 `None`。
    pub fn anim_layer_blend_dim(&self, instance: &str, layer: &str) -> Option<&'static str> {
        self.blend_space_of(instance, layer)
            .map(|s| s.dim().as_str())
    }

    /// 混合空间单行摘要（日志 / RPC 回显用）。没挂空间返回 `None`。
    pub fn anim_layer_blend_brief(&self, instance: &str, layer: &str) -> Option<String> {
        self.blend_space_of(instance, layer).map(|s| s.brief())
    }

    /// 当前参数下的权重（按名字合并、和为 1、零权重不出现）。没挂空间返回 `None`。
    pub fn anim_layer_blend_weights(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<Vec<(String, f32)>> {
        self.blend_space_of(instance, layer).map(|s| s.weights())
    }

    fn blend_space_of(&self, instance: &str, layer: &str) -> Option<&BlendSpace> {
        self.anim_layer(instance, layer)?.blend_space.as_ref()
    }

    /// 混合空间的剖分是否挂起（扩展五十一）。没挂空间返回 `None`。
    ///
    /// `true` = 样本加进来了但剖分还没提交。求值仍然精确（现场算临时拓扑），
    /// 编辑器据这个标记决定「要不要现在就画线框」——挂起时 `triangles()` 是空的。
    pub fn anim_layer_blend_stale(&self, instance: &str, layer: &str) -> Option<bool> {
        self.blend_space_of(instance, layer).map(|s| s.is_stale())
    }

    /// 把挂起的剖分一次性提交（扩展五十一）。没挂空间返回 `None`。
    ///
    /// 返回 `Some(true)` = 这次真的重建了、`Some(false)` = 本来就最新（没付钱）。
    /// 逐帧 advance 里也会自动提交一次（`AnimLayer::apply_blend_space`），
    /// 这个入口是给「批量装配完立刻画线框 / 立刻探针」的编辑器与脚本用的。
    pub fn build_anim_layer_blend_pending(&mut self, instance: &str, layer: &str) -> Option<bool> {
        let inst = self.anim_instance_mut(instance)?;
        let space = anim_layer_mut(inst, layer)?.blend_space.as_mut()?;
        Some(space.build_pending())
    }

    /// 剖分提交账本（扩展五十三）：`(增量续算次数, 整表重剖次数)`。
    /// 编辑器 / 探针靠它确认「补点真的走了续算」，而不是每次都整表重来。
    pub fn anim_layer_blend_build_stats(
        &self,
        instance: &str,
        layer: &str,
    ) -> Option<(usize, usize)> {
        Some(self.blend_space_of(instance, layer)?.build_stats())
    }

    /// 按样本设层速率（扩展五十二）：改了之后这层的时间推进就跟着参数走了。
    ///
    /// `instance` / `clip_name` 的解析口径与 `add_anim_layer_blend_sample` 一致
    /// （片段名可只写实例前缀段，入库用规范名，大小写不敏感）。
    ///
    /// `None` = 层不存在或没挂空间；`Some(false)` = 片段在空间里没有样本
    /// （或速率非有限）。空间里没有该样本时**不改**任何东西——这是编辑器最常见的
    /// 「先 set 后 add」顺序错，静默生效反而查不出问题。
    pub fn set_anim_layer_blend_sample_speed(
        &mut self,
        instance: &str,
        layer: &str,
        clip_name: &str,
        speed: f32,
    ) -> Option<bool> {
        let clip = self.find_anim_clip(instance, clip_name);
        let layer_ref = anim_layer_mut(self.anim_instance_mut(instance)?, layer)?;
        let space = layer_ref.blend_space.as_mut()?;
        Some(match clip {
            Some(c) => space.set_sample_speed(&c.name, speed),
            // 解析不到片段时仍允许按原始名字命中原名样本（片段可能已被卸载，
            // 但样本表还在——此时按名字改速率是合理的，不该整条失败）。
            None => space.set_sample_speed(clip_name, speed),
        })
    }

    /// 当前按样本插值出的层速率倍率（扩展五十二）。没挂空间返回 `None`。
    ///
    /// 全原速（没有变速样本）时返回 `Some(1.0)`——探针据此判断「这一层到底有没有在变速」，
    /// 不必自己扫样本表。
    pub fn anim_layer_blend_speed(&self, instance: &str, layer: &str) -> Option<f32> {
        self.blend_space_of(instance, layer).map(|s| s.speed_at())
    }

    /// 按「实例名前缀 + 片段名（大小写不敏感）」解析出可切换片段。
    fn find_anim_clip(&self, instance: &str, clip_name: &str) -> Option<std::sync::Arc<AnimClip>> {
        let inst_key = self
            .anim_instance(instance)?
            .name
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let head = format!("{inst_key}/");
        self.anim_clips
            .iter()
            .find(|(k, _)| {
                k.strip_prefix(&head)
                    .map(|n| n.eq_ignore_ascii_case(clip_name))
                    .unwrap_or(false)
            })
            .map(|(_, c)| c.clone())
    }

    // ---------- 加载 ----------

    pub fn from_json_str(src: &str) -> Result<Self, String> {
        // Windows 上用 PowerShell / 记事本保存的 UTF-8 文件常带 BOM，serde_json 会直接
        // 报 "expected value at line 1 column 1"，这里统一剥掉以免用户以为场景写错了。
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let file: SceneFile =
            serde_json::from_str(src).map_err(|e| format!("场景 JSON 解析失败: {e}"))?;
        Self::from_file(file)
    }

    pub fn from_file(file: SceneFile) -> Result<Self, String> {
        let mut scene = Scene::new();
        scene.name = file.name.unwrap_or_else(|| "untitled".to_string());
        scene.physics = file.physics.unwrap_or_default();
        scene.assets = file.assets;

        if let Some(bg) = file.background {
            scene.background = Vec4::from(bg);
        }
        if let Some(cam) = file.camera {
            if let Some(p) = cam.position {
                scene.camera.position = Vec3::from(p);
            }
            if let Some(t) = cam.target {
                scene.camera.target = Vec3::from(t);
            }
            if let Some(f) = cam.fov {
                scene.camera.fov_deg = f;
            }
        }
        if let Some(l) = file.light {
            if let Some(d) = l.direction {
                let d = Vec3::from(d);
                if d.length_squared() > 1e-8 {
                    scene.light.direction = d.normalize();
                }
            }
            if let Some(c) = l.color {
                scene.light.color = Vec3::from(c);
            }
            if let Some(i) = l.intensity {
                scene.light.intensity = i;
            }
            if let Some(a) = l.ambient {
                scene.light.ambient = Vec3::from(a);
            }
            if let Some(sd) = l.shadow {
                let mut s = scene.light.shadow;
                if let Some(v) = sd.enabled {
                    s.enabled = v;
                }
                if let Some(v) = sd.size {
                    s.size = v;
                }
                if let Some(v) = sd.ortho {
                    s.ortho_size = v;
                }
                if let Some(v) = sd.bias {
                    s.bias = v;
                }
                if let Some(v) = sd.normal_bias {
                    s.normal_bias = v;
                }
                if let Some(v) = sd.distance {
                    s.distance = v;
                }
                if let Some(v) = sd.strength {
                    s.strength = v;
                }
                if let Some(v) = sd.cascades {
                    s.cascade.count = v;
                }
                if let Some(v) = sd.blend {
                    s.cascade.blend = v;
                }
                if let Some(v) = sd.cascade_distance {
                    s.cascade.max_distance = v;
                }
                if let Some(v) = sd.caster_extend {
                    s.cascade.caster_extend = v;
                }
                if let Some(v) = sd.caster_cull {
                    s.cascade.caster_cull = v;
                }
                scene.light.shadow = s.sanitized();
            }
        }

        // 环境探针整段覆盖：这是「一个完整的光源配置」，逐字段可缺省会得到四不像的光，
        // 缺省值统一由 EnvironmentSettings::default() 提供（未写的字段在反序列化时就已补默认）
        scene.environment = file.environment.map(|e| e.sanitized());
        // 后处理同样整段覆盖：缺段 = None（直通），有段则按 PostFxSettings 收敛
        scene.postfx = file.postfx.map(|p| p.sanitized());
        // 遮挡剔除：缺段 = 缺省（关闭），有段则收敛到合法区间
        scene.occlusion = file.occlusion.unwrap_or_default().sanitized();
        // HDR 反射探针：缺段 = None（沿用解析 IBL），有段则收敛路径与旋钮
        scene.environment_map = file.environment_map.map(|m| m.sanitized());
        // 局部光源：逐盏收敛 + 按 key 去重（后者覆盖前者），超上限的截断。
        // 空 key 是无效光源（脚本 / RPC 拿不到它），直接跳过而不是静默塞个默认名。
        for light in file.lights {
            if light.key.trim().is_empty() {
                continue;
            }
            let _ = light::upsert_local(&mut scene.lights, light);
        }

        for obj in file.objects {
            scene.spawn(obj)?;
        }
        scene.texts = file.texts.into_iter().map(|t| t.sanitized()).collect();
        scene.ui = file.ui.into_iter().map(|u| u.sanitized()).collect();
        scene.update_world_transforms();
        Ok(scene)
    }

    // ---------- 保存 ----------

    /// 把当前场景的**源数据**导回 [`SceneFile`]，与 [`Scene::from_file`] 严格对称。
    ///
    /// 只写「场景文件里本来就有的东西」：场景名、背景、相机、光照、场景级物理声明、
    /// 物件描述与资产声明。运行时派生状态一律不写：
    ///
    /// - 每帧由 [`Scene::update_world_transforms`] 算出的 `world` / `world_normal` 矩阵缓存；
    /// - 物理后端的刚体句柄 `body`（句柄只对本次运行的 `Physics` 有效）；
    /// - `next_id` / `children` / `time` / `frame` 这类引擎内部记账；
    /// - 动画实例、蒙皮实例、GPU 侧资源与实例索引——它们由应用层按资产声明在加载后重建。
    ///
    /// 于是 `load -> save -> load` 得到的场景与第一次加载语义等价。
    pub fn to_scene_file(&self) -> SceneFile {
        SceneFile {
            name: Some(self.name.clone()),
            background: Some(self.background.to_array()),
            camera: Some(CameraDesc {
                position: Some(self.camera.position.to_array()),
                target: Some(self.camera.target.to_array()),
                fov: Some(self.camera.fov_deg),
            }),
            light: Some(self.light_desc()),
            environment: self.environment.map(|e| e.sanitized()),
            postfx: self.postfx.map(|p| p.sanitized()),
            lights: self.lights.clone(),
            // 全是缺省值时不写这一段：旧场景 roundtrip 不该凭空长出 occlusion 字段
            occlusion: if self.occlusion == OcclusionSettings::default() {
                None
            } else {
                Some(self.occlusion.sanitized())
            },
            // 同理：没配探针时不该凭空长出 environment_map 段
            environment_map: self
                .environment_map
                .as_ref()
                .filter(|m| !m.is_default())
                .map(|m| m.clone().sanitized()),
            objects: self
                .order
                .iter()
                .filter_map(|id| self.nodes.get(id))
                .map(|node| self.node_desc(node))
                .collect(),
            physics: if physics_desc_is_empty(&self.physics) {
                None
            } else {
                Some(self.physics.clone())
            },
            assets: self.assets.clone(),
            texts: self.texts.clone(),
            ui: self.ui.clone(),
        }
    }

    /// 场景 → 紧凑 JSON 文本（`Scene::from_json_str` 的逆操作）。
    pub fn to_json_string(&self) -> Result<String, String> {
        serde_json::to_string(&self.to_scene_file()).map_err(|e| format!("场景序列化失败: {e}"))
    }

    /// 场景 → 带缩进、末尾带换行的 JSON 文本（给人读 / 进版本库的形态）。
    pub fn to_json_pretty(&self) -> Result<String, String> {
        let mut text = serde_json::to_string_pretty(&self.to_scene_file())
            .map_err(|e| format!("场景序列化失败: {e}"))?;
        text.push('\n');
        Ok(text)
    }

    /// 把场景写回文件；父目录不存在时自动创建，写入是整体覆盖。
    ///
    /// 返回的错误是给人看的中文串，与应用层其它读场景处的风格一致。
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("创建场景目录 {} 失败: {e}", dir.display()))?;
            }
        }
        let text = self.to_json_pretty()?;
        std::fs::write(path, text).map_err(|e| format!("写入场景 {} 失败: {e}", path.display()))
    }

    /// 光照 → `light` 段。方向光方向统一写成单位向量（加载侧也会归一化）。
    fn light_desc(&self) -> LightDesc {
        let direction = self.light.direction;
        let direction = if direction.length_squared() > 1e-12 {
            direction.normalize()
        } else {
            Vec3::NEG_Y
        };
        let shadow = self.light.shadow;
        LightDesc {
            direction: Some(direction.to_array()),
            color: Some(self.light.color.to_array()),
            intensity: Some(self.light.intensity),
            ambient: Some(self.light.ambient.to_array()),
            shadow: Some(ShadowDesc {
                enabled: Some(shadow.enabled),
                size: Some(shadow.size),
                // 0 是「按包围球自适应」的哨兵值，写 0 与省略等价，省掉噪声
                ortho: (shadow.ortho_size > 0.0).then_some(shadow.ortho_size),
                bias: Some(shadow.bias),
                normal_bias: Some(shadow.normal_bias),
                distance: Some(shadow.distance),
                strength: Some(shadow.strength),
                // 1 级 + 缺省混合比是「没开 CSM」的等价形态，省掉噪声；
                // 一旦开了多级或调过混合比就必须写进文件，否则重开场景会静默退回单级。
                cascades: (shadow.cascade.count > 1).then_some(shadow.cascade.count),
                blend: (shadow.cascade.blend != light::CascadeSettings::default().blend)
                    .then_some(shadow.cascade.blend),
                cascade_distance: (shadow.cascade.max_distance > 0.0)
                    .then_some(shadow.cascade.max_distance),
                // 0 是「不扩张」的缺省形态，省略；`caster_cull` 缺省为 true，
                // 只有显式关掉时才写进文件（否则重开场景又会静默打开）。
                caster_extend: (shadow.cascade.caster_extend > 0.0)
                    .then_some(shadow.cascade.caster_extend),
                caster_cull: (!shadow.cascade.caster_cull).then_some(false),
            }),
        }
    }

    /// 单个物件 → `objects[]` 项。
    ///
    /// 旋转按欧拉角写回（场景文件的可读形态），退回时 `set_euler_deg` 还原成同一旋转；
    /// 身份旋转（欧拉角恰为 0）与 `visible = true` 这类缺省值省略，保持文件干净。
    fn node_desc(&self, node: &Node) -> ObjectDesc {
        let transform = &node.transform;
        let euler = transform.euler_deg();
        ObjectDesc {
            name: node.name.clone(),
            mesh: Some(node.mesh.as_string()),
            position: Some(transform.position.to_array()),
            rotation: (euler.length_squared() > 0.0).then_some(euler.to_array()),
            scale: Some(transform.scale.to_array()),
            color: Some(node.color.to_array()),
            parent: node
                .parent
                .and_then(|id| self.nodes.get(&id))
                .map(|parent| parent.name.clone()),
            visible: (!node.visible).then_some(false),
            physics: node.physics.clone(),
            material: node.material,
        }
    }
}

/// 场景级物理配置是否「什么都没声明」：全 None 时整段省略，JSON 更干净。
fn physics_desc_is_empty(desc: &PhysicsSceneDesc) -> bool {
    desc.enabled.is_none()
        && desc.backend.is_none()
        && desc.gravity.is_none()
        && desc.ground.is_none()
        && desc.ccd.is_none()
}

/// 按名字取可变层；名字为空时取第一层（与 [`Scene::anim_layer`] 口径一致）。
fn anim_layer_mut<'a>(inst: &'a mut AnimInstance, name: &str) -> Option<&'a mut AnimLayer> {
    if name.is_empty() {
        return inst.layers.first_mut();
    }
    inst.layers
        .iter_mut()
        .find(|l| l.name == name || l.name.eq_ignore_ascii_case(name))
}

/// 按名字取层下标（`name` 为空取 0），口径与 [`anim_layer_mut`] 一致。
fn anim_layer_index(inst: &AnimInstance, name: &str) -> Option<usize> {
    if name.is_empty() {
        return if inst.layers.is_empty() {
            None
        } else {
            Some(0)
        };
    }
    inst.layers
        .iter()
        .position(|l| l.name == name || l.name.eq_ignore_ascii_case(name))
}

/// 按名字取层内片段下标；`clip` 为空取第一路，大小写不敏感（扩展四十七）。
fn layer_clip_index(layer: &AnimLayer, clip: &str) -> Option<usize> {
    if clip.is_empty() {
        return if layer.clips.is_empty() {
            None
        } else {
            Some(0)
        };
    }
    layer
        .clips
        .iter()
        .position(|c| c.clip.name == clip || c.clip.name.eq_ignore_ascii_case(clip))
}

/// 按名字取层内可变片段（口径同 [`layer_clip_index`]）。
fn layer_clip_mut<'a>(
    layer: &'a mut AnimLayer,
    clip: &str,
) -> Option<&'a mut crate::anim_blend::LayerClip> {
    let index = layer_clip_index(layer, clip)?;
    layer.clips.get_mut(index)
}

#[cfg(test)]
mod anim_tests {
    use super::*;
    use crate::anim::{AnimClip, AnimInstance, AnimKey, AnimTrack};
    use glam::Quat;

    fn clip_with_move() -> AnimClip {
        let track = AnimTrack {
            node_path: "root/arm".to_string(),
            node_index: 1,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: Vec3::new(0.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                AnimKey {
                    time: 1.0,
                    position: Vec3::new(10.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            ],
        };
        AnimClip {
            name: "walk".to_string(),
            duration: 1.0,
            tracks: vec![track],
        }
    }

    #[test]
    fn instance_advances_and_writes_node_transform() {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();
        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        scene.add_anim_instance(inst);

        scene.update_animations(0.25);
        let p = scene.get(id).unwrap().transform.position;
        assert!((p.x - 2.5).abs() < 1e-4, "0.25s 应插值到 x=2.5，实际 {p:?}");

        // 循环：1.0s 后回到 0，再走 0.5s → x=5
        scene.update_animations(0.75);
        scene.update_animations(0.5);
        let p = scene.get(id).unwrap().transform.position;
        assert!((p.x - 5.0).abs() < 1e-4, "循环后 x 应为 5，实际 {p:?}");
    }

    #[test]
    fn stop_keeps_pose_and_speed_scales_time() {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();
        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        inst.looping = false;
        scene.add_anim_instance(inst);

        assert!(scene.set_anim_speed("demo/walk", 2.0));
        scene.update_animations(0.25); // 0.5s 处
        assert!((scene.anim_time("demo/walk").unwrap() - 0.5).abs() < 1e-4);

        assert!(scene.stop_anim("demo/walk"));
        scene.update_animations(0.5);
        let p = scene.get(id).unwrap().transform.position;
        assert!((p.x - 5.0).abs() < 1e-4, "暂停后姿态不应变化，实际 {p:?}");
    }

    #[test]
    fn non_looping_stops_at_end() {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();
        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        inst.looping = false;
        scene.add_anim_instance(inst);

        scene.update_animations(3.0);
        let node = scene.get(id).unwrap();
        assert!((node.transform.position.x - 10.0).abs() < 1e-4);
        assert!(!scene.anim_instance("demo/walk").unwrap().playing);
    }

    #[test]
    fn switching_clip_rebinds_tracks() {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();

        // 两个片段共用同一节点（下标 1 / 路径 root/arm），但终点位移不同
        let mut run = clip_with_move();
        run.name = "run".to_string();
        run.tracks[0].keys[1].position = Vec3::new(20.0, 0.0, 0.0);

        let walk = clip_with_move();
        scene.register_anim_clip("demo/walk", std::sync::Arc::new(walk.clone()));
        scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        scene.add_anim_instance(inst);

        assert_eq!(scene.anim_clip_names("demo"), vec!["run", "walk"]);
        assert!(scene.set_anim_clip("demo/walk", "run"), "应能切到 run");
        let inst = scene.anim_instance("demo/walk").unwrap();
        assert_eq!(inst.clip.name, "run");
        assert_eq!(inst.bindings.len(), 1, "切片段后应按节点下标重新绑定轨道");

        // 从 0 播到 0.5s（片段 1s 循环，直接播 1s 会绕回 0），x 应在 run 的中点 10
        scene.update_animations(0.5);
        let p = scene.get(id).unwrap().transform.position;
        assert!(
            (p.x - 10.0).abs() < 1e-4,
            "切片段后应采样新片段，实际 {p:?}"
        );
    }

    /// walk（x: 0→10）与 run（x: 0→20）共用同一节点，用于交叉淡入断言。
    fn fade_scene() -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();

        let walk = clip_with_move();
        let mut run = clip_with_move();
        run.name = "run".to_string();
        run.tracks[0].keys[1].position = Vec3::new(20.0, 0.0, 0.0);

        scene.register_anim_clip("demo/walk", std::sync::Arc::new(walk.clone()));
        scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        scene.add_anim_instance(inst);
        (scene, id)
    }

    #[test]
    fn cross_fade_blends_two_clips_then_settles() {
        let (mut scene, id) = fade_scene();
        scene.update_animations(0.25);
        assert!((scene.get(id).unwrap().transform.position.x - 2.5).abs() < 1e-4);

        // 过渡 0.4s：半程时新旧姿态各占一半权重
        assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.4));
        assert_eq!(scene.anim_fade_source("demo/walk"), "walk");
        assert!((scene.anim_fade_weight("demo/walk").unwrap() - 0.0).abs() < 1e-4);

        scene.update_animations(0.2);
        // 新 clip run 时间 0.2 → x=4；旧 clip walk 继续走到 0.45 → x=4.5；权重 smoothstep(0.5)=0.5
        let x = scene.get(id).unwrap().transform.position.x;
        assert!((x - 4.25).abs() < 1e-3, "半程应混合到 x=4.25，实际 {x}");

        scene.update_animations(0.3);
        // 过渡走完：只剩 run，时间 0.5 → x=10
        let x = scene.get(id).unwrap().transform.position.x;
        assert!((x - 10.0).abs() < 1e-3, "过渡结束后应等于新片段，实际 {x}");
        assert_eq!(scene.anim_fade_source("demo/walk"), "");
        assert!((scene.anim_fade_weight("demo/walk").unwrap() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn cross_fade_zero_duration_is_hard_cut() {
        let (mut scene, id) = fade_scene();
        scene.update_animations(0.25);

        assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.0));
        assert_eq!(
            scene.anim_fade_source("demo/walk"),
            "",
            "0 时长不应留下过渡"
        );
        scene.update_animations(0.2);
        let x = scene.get(id).unwrap().transform.position.x;
        assert!((x - 4.0).abs() < 1e-4, "硬切后直接采样 run，实际 {x}");
    }

    #[test]
    fn cross_fade_keeps_old_track_when_new_clip_lacks_it() {
        let (mut scene, id) = fade_scene();
        // 新片段只有另一条轨道（root/leg），因此 arm 在过渡期间只能由旧片段驱动
        let mut run = clip_with_move();
        run.name = "run".to_string();
        // 换一条轨道（下标与路径都不同），确保 arm 的绑定在新片段里找不到对应轨道
        run.tracks[0].node_index = 2;
        run.tracks[0].node_path = "root/leg".to_string();
        scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

        scene.update_animations(0.25);
        assert!(scene.set_anim_clip_fade("demo/walk", "run", 0.4));
        scene.update_animations(0.2);
        let x = scene.get(id).unwrap().transform.position.x;
        assert!(
            (x - 4.5).abs() < 1e-3,
            "缺轨道时应由旧片段驱动到 walk(0.45)=4.5，实际 {x}"
        );

        // 过渡结束后不再有轨道驱动，节点停在旧片段最后写入的姿态
        scene.update_animations(0.3);
        let x = scene.get(id).unwrap().transform.position.x;
        assert!((x - 4.5).abs() < 1e-3, "过渡结束后应停在末姿态，实际 {x}");
    }

    #[test]
    fn dead_node_bindings_are_pruned() {
        let mut scene = Scene::new();
        let id = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();
        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(clip_with_move()));
        inst.bindings = vec![crate::anim::AnimBinding {
            node: id,
            node_index: 1,
            path: "root/arm".to_string(),
            track: 0,
        }];
        scene.add_anim_instance(inst);

        scene.despawn("arm");
        scene.prune_anim_bindings();
        assert_eq!(scene.anim_instance("demo/walk").unwrap().bindings.len(), 0);
        // 没有绑定也不该 panic
        scene.update_animations(0.1);
    }

    // ---------- 混合树（扩展十八 A） ----------

    /// 一条 0 → `end` 的直线轨道，用于混合树断言。
    fn line_track(index: u32, path: &str, end: Vec3) -> AnimTrack {
        AnimTrack {
            node_path: path.to_string(),
            node_index: index,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: Vec3::ZERO,
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                AnimKey {
                    time: 1.0,
                    position: end,
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            ],
        }
    }

    /// arm（下标 1）+ leg（下标 2）；主片段 walk 把两者都推到 x=10。
    fn layer_scene() -> (Scene, NodeId, NodeId) {
        let mut scene = Scene::new();
        let arm = scene
            .spawn(ObjectDesc {
                name: "arm".to_string(),
                ..Default::default()
            })
            .unwrap();
        let leg = scene
            .spawn(ObjectDesc {
                name: "leg".to_string(),
                ..Default::default()
            })
            .unwrap();

        let walk = AnimClip {
            name: "walk".to_string(),
            duration: 1.0,
            tracks: vec![
                line_track(1, "root/arm", Vec3::new(10.0, 0.0, 0.0)),
                line_track(2, "root/leg", Vec3::new(10.0, 0.0, 0.0)),
            ],
        };

        let mut inst = AnimInstance::new("demo/walk", std::sync::Arc::new(walk));
        inst.bindings = vec![
            crate::anim::AnimBinding {
                node: arm,
                node_index: 1,
                path: "root/arm".to_string(),
                track: 0,
            },
            crate::anim::AnimBinding {
                node: leg,
                node_index: 2,
                path: "root/leg".to_string(),
                track: 1,
            },
        ];
        scene.add_anim_instance(inst);
        (scene, arm, leg)
    }

    fn atk_clip() -> AnimClip {
        AnimClip {
            name: "atk".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
        }
    }

    #[test]
    fn override_layer_replaces_masked_bone_only() {
        let (mut scene, arm, leg) = layer_scene();
        let atk = atk_clip();
        scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));

        let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0)
            .with_mask(BoneMask::only(["arm"]));
        assert!(scene.add_anim_layer("demo/walk", layer));

        scene.update_animations(0.5);
        let arm_x = scene.get(arm).unwrap().transform.position.x;
        let leg_x = scene.get(leg).unwrap().transform.position.x;
        assert!(
            (arm_x - 50.0).abs() < 1e-4,
            "遮罩内骨骼应取层姿态，实际 {arm_x}"
        );
        assert!(
            (leg_x - 5.0).abs() < 1e-4,
            "遮罩外骨骼保持主片段，实际 {leg_x}"
        );
    }

    #[test]
    fn additive_layer_stacks_delta_on_base() {
        let (mut scene, arm, _leg) = layer_scene();
        let bob = AnimClip {
            name: "bob".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 2.0))],
        };
        let layer =
            AnimLayer::new("bob", std::sync::Arc::new(bob), 0.5).with_mode(LayerMode::Additive);
        assert!(scene.add_anim_layer("demo/walk", layer));

        scene.update_animations(0.5);
        let p = scene.get(arm).unwrap().transform.position;
        // 主片段 x=5；加法层在 0.5s 的相对增量是 (0,0,1)，按 0.5 权重叠成 z=0.5
        assert!((p.x - 5.0).abs() < 1e-4, "{p:?}");
        assert!((p.z - 0.5).abs() < 1e-4, "加法层应叠加相对增量，实际 {p:?}");
    }

    #[test]
    fn additive_layer_multiclip_uses_delta_semantics() {
        // 扩展五十：加法层多路份额合的是「增量」，不是「姿态平均」。
        // A：arm 绕 X 0 → 90°（增量 90°）；B：参考与姿态都绕 Y 80°（增量 0）。
        // 采样点 0.5 s 时 A 自身增量已是 45°，再乘 0.5 份额 → 22.5° 绕 X；
        // B 增量恒为零，不该把 A 的增量稀释成混轴姿态（覆盖路径会）。
        let rot_track = |deg: f32| AnimTrack {
            node_path: "root/arm".to_string(),
            node_index: 1,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                AnimKey {
                    time: 1.0,
                    position: Vec3::ZERO,
                    rotation: Quat::from_rotation_x(deg.to_radians()),
                    scale: Vec3::ONE,
                },
            ],
        };
        let hold = Quat::from_rotation_y(80f32.to_radians());
        let hold_track = AnimTrack {
            node_path: "root/arm".to_string(),
            node_index: 1,
            keys: vec![
                AnimKey {
                    time: 0.0,
                    position: Vec3::ZERO,
                    rotation: hold,
                    scale: Vec3::ONE,
                },
                AnimKey {
                    time: 1.0,
                    position: Vec3::ZERO,
                    rotation: hold,
                    scale: Vec3::ONE,
                },
            ],
        };

        let (mut scene, arm, _leg) = layer_scene();
        let a = AnimClip {
            name: "a".to_string(),
            duration: 1.0,
            tracks: vec![rot_track(90.0)],
        };
        let b = AnimClip {
            name: "b".to_string(),
            duration: 1.0,
            tracks: vec![hold_track],
        };
        let mut layer =
            AnimLayer::new("add", std::sync::Arc::new(a), 1.0).with_mode(LayerMode::Additive);
        assert!(layer.add_clip(std::sync::Arc::new(b), 1.0));
        // 用 1D 混合空间把两路份额拉到 0.5 / 0.5（不手写权重，走真实驱动链路）
        let mut space = crate::blend_space::BlendSpace::new_1d("w");
        assert!(space.add_sample("a", [0.0, 0.0]));
        assert!(space.add_sample("b", [1.0, 0.0]));
        space.set_value_1d(0.5);
        layer.blend_space = Some(space);
        assert!(scene.add_anim_layer("demo/walk", layer));

        scene.update_animations(0.5);
        let t = scene.get(arm).unwrap().transform;
        assert!(
            (t.position.x - 5.0).abs() < 1e-4,
            "主片段位置不受影响：{t:?}"
        );
        let want = Quat::from_rotation_x(22.5f32.to_radians()) * Vec3::Z;
        let got = t.rotation * Vec3::Z;
        assert!(
            (got - want).length() < 1e-3,
            "加法层多路应合成 22.5° 绕 X 的增量，实际 {got:?} 应为 {want:?}"
        );
    }

    #[test]
    fn layer_fade_in_scales_contribution() {
        let (mut scene, arm, _leg) = layer_scene();
        let atk = atk_clip();
        scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
        let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 0.0)
            .with_mask(BoneMask::only(["arm"]));
        assert!(scene.add_anim_layer("demo/walk", layer));

        scene.update_animations(0.5);
        let base_x = scene.get(arm).unwrap().transform.position.x;
        assert!(
            (base_x - 5.0).abs() < 1e-4,
            "权重 0 时层不生效，实际 {base_x}"
        );

        assert!(scene.fade_anim_layer("demo/walk", "upper", 1.0, 0.4));
        scene.update_animations(0.2);
        let mid = scene.get(arm).unwrap().transform.position.x;
        assert!(
            mid > 5.0 && mid < 70.0,
            "淡入半程应落在主姿态与层姿态之间，实际 {mid}"
        );

        scene.update_animations(0.2);
        // 时间累积 0.5 + 0.2 + 0.2 = 0.9s，层姿态 90 完全取代主片段
        let full = scene.get(arm).unwrap().transform.position.x;
        assert!(
            (full - 90.0).abs() < 1e-3,
            "淡入结束应完全取层姿态，实际 {full}"
        );
        assert_eq!(
            scene.anim_layer("demo/walk", "upper").unwrap().fade,
            None,
            "过渡完成后应清空"
        );
    }

    #[test]
    fn layer_management_api_roundtrip() {
        let (mut scene, _arm, _leg) = layer_scene();
        let atk = atk_clip();
        let breathe = AnimClip {
            name: "breathe".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 1.0))],
        };
        scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
        scene.register_anim_clip("demo/breathe", std::sync::Arc::new(breathe));

        let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 0.5);
        assert!(scene.add_anim_layer("demo/walk", layer));
        assert_eq!(
            scene.anim_layer_names("demo/walk"),
            vec!["upper".to_string()]
        );
        assert_eq!(scene.anim_layer_count("demo/walk"), Some(1));

        assert!(scene.set_anim_layer_weight("demo/walk", "upper", 1.0));
        assert_eq!(
            scene.anim_layer_effective_weight("demo/walk", "upper"),
            Some(1.0)
        );
        assert!(scene.set_anim_layer_mode("demo/walk", "upper", LayerMode::Additive));
        assert_eq!(
            scene.anim_layer("demo/walk", "upper").unwrap().mode,
            LayerMode::Additive
        );
        assert!(scene.set_anim_layer_mask("demo/walk", "upper", BoneMask::only(["arm"])));
        assert_eq!(
            scene.anim_layer("demo/walk", "upper").unwrap().mask.include,
            vec!["arm".to_string()]
        );
        assert!(scene.set_anim_layer_speed("demo/walk", "upper", 2.0));

        assert!(scene.add_anim_layer_clip("demo/walk", "upper", "breathe", 0.5));
        assert_eq!(
            scene.anim_layer("demo/walk", "upper").unwrap().clips.len(),
            2
        );
        assert!(scene.set_anim_layer_clip_weight("demo/walk", "upper", "breathe", 2.0));
        assert!(scene.remove_anim_layer_clip("demo/walk", "upper", "breathe"));

        assert!(scene.set_anim_layer_clip("demo/walk", "upper", "atk"));
        assert_eq!(
            scene.anim_layer("demo/walk", "upper").unwrap().clips.len(),
            1
        );
        assert!(scene
            .anim_layer_report("demo/walk", "upper")
            .unwrap()
            .contains("upper"));

        assert!(scene.remove_anim_layer("demo/walk", "upper"));
        assert_eq!(scene.anim_layer_count("demo/walk"), Some(0));
        assert!(!scene.remove_anim_layer("demo/walk", "upper"));
        assert!(scene.anim_layer("demo/walk", "upper").is_none());
        assert!(scene.anim_layer_names("nope").is_empty());
    }

    // ---------- 1D / 2D 混合空间（扩展四十五） ----------

    #[test]
    fn blend_space_drives_layer_clip_weights() {
        let (mut scene, arm, _leg) = layer_scene();
        // still 恒 0；fast 是 0→100 的直线，0.5s 处为 50
        let still = std::sync::Arc::new(AnimClip {
            name: "still".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
        });
        let fast = std::sync::Arc::new(AnimClip {
            name: "fast".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
        });
        scene.register_anim_clip("demo/still", still.clone());
        scene.register_anim_clip("demo/fast", fast.clone());

        let mut layer = AnimLayer::new("move", still.clone(), 1.0);
        assert!(layer.add_clip(fast.clone(), 0.0));
        assert!(scene.add_anim_layer("demo/walk", layer));

        // 没挂空间：照旧走手写权重（fast 权重 0 → arm 停在 0），零变化
        scene.update_animations(0.5);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!(x.abs() < 1e-4, "未挂空间时应走手写权重，实际 {x}");

        assert!(scene.set_anim_layer_blend_space_1d("demo/walk", "move", "speed"));
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "still", [0.0, 0.0]),
            Some(true)
        );
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "fast", [1.0, 0.0]),
            Some(true)
        );

        // 参数 0.5：两路各半 → 0*0.5 + 50*0.5 = 25
        assert_eq!(
            scene.set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0),
            Some(true)
        );
        scene.update_animations(0.0);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!((x - 25.0).abs() < 1e-3, "参数 0.5 应混合到 25，实际 {x}");

        // 参数推到底：fast 全额 → 50
        assert_eq!(
            scene.set_anim_layer_blend_value("demo/walk", "move", 1.0, 0.0),
            Some(true)
        );
        scene.update_animations(0.0);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!((x - 50.0).abs() < 1e-3, "参数 1.0 应取 fast，实际 {x}");
    }

    #[test]
    fn blend_space_3d_drives_layer_clip_weights() {
        let (mut scene, arm, _leg) = layer_scene();
        let still = std::sync::Arc::new(AnimClip {
            name: "still".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::ZERO)],
        });
        let fast = std::sync::Arc::new(AnimClip {
            name: "fast".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
        });
        // 只为凑出四面体：这两个片段不进层，权重份额应被忽略。
        let up = std::sync::Arc::new(AnimClip {
            name: "up".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 0.0))],
        });
        let far = std::sync::Arc::new(AnimClip {
            name: "far".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(0.0, 0.0, 0.0))],
        });
        scene.register_anim_clip("demo/still", still.clone());
        scene.register_anim_clip("demo/fast", fast.clone());
        scene.register_anim_clip("demo/up", up);
        scene.register_anim_clip("demo/far", far);

        let mut layer = AnimLayer::new("move", still.clone(), 1.0);
        assert!(layer.add_clip(fast.clone(), 0.0));
        assert!(scene.add_anim_layer("demo/walk", layer));

        assert!(scene.set_anim_layer_blend_space_3d("demo/walk", "move", "speed", "turn", "slope"));
        assert_eq!(
            scene.anim_layer_blend_dim("demo/walk", "move"),
            Some("3d"),
            "挂的应是三维空间"
        );
        for (name, at) in [
            ("still", [0.0, 0.0, 0.0]),
            ("fast", [1.0, 0.0, 0.0]),
            ("up", [0.0, 1.0, 0.0]),
            ("far", [0.0, 0.0, 1.0]),
        ] {
            assert_eq!(
                scene.add_anim_layer_blend_sample_3d("demo/walk", "move", name, at),
                Some(true),
                "{name}"
            );
        }
        let brief = scene
            .anim_layer_blend_brief("demo/walk", "move")
            .expect("刚挂过");
        assert!(brief.contains("3d(speed,turn,slope)"), "{brief}");
        assert!(
            brief.contains("tet=1"),
            "四个不共面样本应建出一个四面体：{brief}"
        );

        // 棱中点 still-fast：各半 → 0*0.5 + 50*0.5 = 25
        assert_eq!(
            scene.set_anim_layer_blend_value_3d("demo/walk", "move", 0.5, 0.0, 0.0),
            Some(true)
        );
        scene.update_animations(0.5);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!((x - 25.0).abs() < 1e-3, "三维插值应到 25，实际 {x}");

        // 顶点：fast 全额 → 50（层时间停在 0.5s，避免跨过 1s 循环点）
        assert_eq!(
            scene.set_anim_layer_blend_value_3d("demo/walk", "move", 1.0, 0.0, 0.0),
            Some(true)
        );
        scene.update_animations(0.0);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!((x - 50.0).abs() < 1e-3, "顶点应取 fast，实际 {x}");

        // 二维入口在三维空间上仍可用（前两维），不该 panic 或丢权重
        assert_eq!(
            scene.set_anim_layer_blend_value("demo/walk", "move", 0.5, 0.0),
            Some(true)
        );
        scene.update_animations(0.0);
        let x = scene.get(arm).unwrap().transform.position.x;
        assert!((x - 25.0).abs() < 1e-3, "二维入口后应回到 25，实际 {x}");
    }

    #[test]
    fn blend_space_api_reports_missing_layer_and_space() {
        let (mut scene, _arm, _leg) = layer_scene();
        let atk = atk_clip();
        scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));

        // 层不存在：整组 API 全是 None / false，不 panic
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "nope", "atk", [0.0, 0.0]),
            None
        );
        assert_eq!(
            scene.set_anim_layer_blend_value("demo/walk", "nope", 0.0, 0.0),
            None
        );
        assert!(scene.anim_layer_blend_brief("demo/walk", "nope").is_none());
        assert!(scene
            .anim_layer_blend_weights("demo/walk", "nope")
            .is_none());
        assert!(!scene.set_anim_layer_blend_space_1d("demo/walk", "nope", "speed"));
        assert!(!scene.clear_anim_layer_blend_space("demo/walk", "nope"));

        // 层存在但没挂空间：同样是 None
        let layer = AnimLayer::new("move", std::sync::Arc::new(atk), 1.0);
        assert!(scene.add_anim_layer("demo/walk", layer));
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
            None
        );
        assert_eq!(
            scene.set_anim_layer_blend_value("demo/walk", "move", 0.0, 0.0),
            None
        );
        assert!(scene.anim_layer_blend_brief("demo/walk", "move").is_none());
        assert!(!scene.clear_anim_layer_blend_space("demo/walk", "move"));

        // 挂上二维空间
        assert!(scene.set_anim_layer_blend_space_2d("demo/walk", "move", "mx", "my"));
        let brief = scene
            .anim_layer_blend_brief("demo/walk", "move")
            .expect("应已挂空间");
        assert!(brief.starts_with("2d(mx,my)"), "{brief}");
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
            Some(true)
        );
        // 重复位置被拒；片段名解析不到也被拒
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "atk", [0.0, 0.0]),
            Some(false)
        );
        assert_eq!(
            scene.add_anim_layer_blend_sample("demo/walk", "move", "ghost", [1.0, 0.0]),
            Some(false)
        );
        assert_eq!(
            scene.anim_layer_blend_weights("demo/walk", "move"),
            Some(vec![("atk".to_string(), 1.0)])
        );
        // 摘掉空间后回到手写权重
        assert!(scene.clear_anim_layer_blend_space("demo/walk", "move"));
        assert!(scene.anim_layer_blend_brief("demo/walk", "move").is_none());
        scene.update_animations(0.5);
        assert!((scene.get(_arm).unwrap().transform.position.x - 50.0).abs() < 1e-3);
    }

    // ---------- 层内相位同步（扩展四十六） ----------

    /// 「慢 1.0 s + 快 0.5 s」的层场景：两路都是 0→100 的直线，遮罩只开 arm。
    fn sync_scene() -> (Scene, NodeId) {
        let (mut scene, arm, _leg) = layer_scene();
        let slow = std::sync::Arc::new(AnimClip {
            name: "slow".to_string(),
            duration: 1.0,
            tracks: vec![line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0))],
        });
        let mut fast_track = line_track(1, "root/arm", Vec3::new(100.0, 0.0, 0.0));
        fast_track.keys[1].time = 0.5;
        let fast = std::sync::Arc::new(AnimClip {
            name: "fast".to_string(),
            duration: 0.5,
            tracks: vec![fast_track],
        });

        let mut layer = AnimLayer::new("move", slow, 1.0).with_mask(BoneMask::only(["arm"]));
        assert!(layer.add_clip(fast, 1.0));
        assert!(scene.add_anim_layer("demo/walk", layer));
        (scene, arm)
    }

    #[test]
    fn anim_layer_sync_removes_phase_drift() {
        let (mut scene, arm) = sync_scene();
        // 0.25 s：慢路 25%、快路 50%，等权平均 37.5 —— 两路步态已经错开
        scene.update_animations(0.25);
        let off = scene.get(arm).unwrap().transform.position.x;
        assert!((off - 37.5).abs() < 1e-3, "不锁相位应 37.5，实际 {off}");
        assert_eq!(scene.anim_layer_sync("demo/walk", "move"), Some(false));

        // 锁相位：同一时刻两路都走 25%，等权平均仍是 25%
        assert!(scene.set_anim_layer_sync("demo/walk", "move", true));
        assert_eq!(scene.anim_layer_sync("demo/walk", "move"), Some(true));
        scene.update_animations(0.0);
        let on = scene.get(arm).unwrap().transform.position.x;
        assert!((on - 25.0).abs() < 1e-3, "锁相位应 25，实际 {on}");

        // 推到层时间 0.6：锁相位时两路仍是 60%
        scene.update_animations(0.35);
        let on = scene.get(arm).unwrap().transform.position.x;
        assert!((on - 60.0).abs() < 1e-3, "锁相位应 60，实际 {on}");
        // 关掉相位锁：快路停在末帧(100)，混合值回到 80 —— 漂移重新出现
        assert!(scene.set_anim_layer_sync("demo/walk", "move", false));
        scene.update_animations(0.0);
        let off = scene.get(arm).unwrap().transform.position.x;
        assert!((off - 80.0).abs() < 1e-3, "不锁相位应 80，实际 {off}");
    }

    #[test]
    fn anim_layer_sync_probe_and_missing_layer() {
        let (mut scene, _arm) = sync_scene();
        scene.update_animations(0.3);
        let probe = scene
            .anim_layer_sync_probe("demo/walk", "move")
            .expect("层存在");
        assert!(probe.contains("sync=off"), "{probe}");
        assert!(probe.contains("drift=0."), "{probe}");

        assert!(scene.set_anim_layer_sync_strategy("demo/walk", "move", LayerSync::Phase));
        let probe = scene
            .anim_layer_sync_probe("demo/walk", "move")
            .expect("层存在");
        assert!(probe.contains("sync=phase"), "{probe}");
        assert!(probe.contains("drift=0.000"), "{probe}");
        let phase = scene.anim_layer_phase("demo/walk", "move").expect("层存在");
        assert!((phase - 0.3).abs() < 1e-4, "相位应 0.3，实际 {phase}");

        // 实例 / 层不存在：一律 None / false，不 panic
        assert_eq!(scene.anim_layer_sync("demo/walk", "nope"), None);
        assert_eq!(scene.anim_layer_phase("nope", "move"), None);
        assert!(scene.anim_layer_sync_probe("demo/walk", "nope").is_none());
        assert!(!scene.set_anim_layer_sync("demo/walk", "nope", true));
        assert!(!scene.set_anim_layer_sync_strategy("nope", "move", LayerSync::Phase));
    }

    // ---------- 相位偏移量与跨层对齐（扩展四十七） ----------

    #[test]
    fn anim_layer_clip_phase_offset_shifts_single_path() {
        let (mut scene, arm) = sync_scene();
        scene.update_animations(0.25);
        assert!(scene.set_anim_layer_sync("demo/walk", "move", true));
        scene.update_animations(0.0);
        // 相位锁：两路同为 25%，平均值仍是 25
        assert!((scene.get(arm).unwrap().transform.position.x - 25.0).abs() < 1e-3);

        // 给快路加半步偏移：它走到 75%，两路平均被拉到 50 —— 步幅没变、只是错开半步
        assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "fast", 0.5));
        assert_eq!(
            scene.anim_layer_clip_phase_offset("demo/walk", "move", "fast"),
            Some(0.5)
        );
        scene.update_animations(0.0);
        let offset = scene.get(arm).unwrap().transform.position.x;
        assert!((offset - 50.0).abs() < 1e-3, "错步应 50，实际 {offset}");

        // 归零即回到逐位相同的 25
        assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "fast", 0.0));
        scene.update_animations(0.0);
        let back = scene.get(arm).unwrap().transform.position.x;
        assert!((back - 25.0).abs() < 1e-3, "归零应 25，实际 {back}");

        // 片段 / 层写错都不该改状态
        assert!(!scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "ghost", 0.5));
        assert!(!scene.set_anim_layer_clip_phase_offset("demo/walk", "nope", "fast", 0.5));
        assert_eq!(
            scene.anim_layer_clip_phase_offset("demo/walk", "move", "ghost"),
            None
        );
        // 空片段名作用于层内第一路；负偏移按循环等价折叠
        assert!(scene.set_anim_layer_clip_phase_offset("demo/walk", "move", "", -0.25));
        let first = scene
            .anim_layer_clip_phase_offset("demo/walk", "move", "slow")
            .expect("第一路是 slow");
        assert!((first - 0.75).abs() < 1e-5, "{first}");
    }

    #[test]
    fn anim_layer_sync_source_aligns_across_layers() {
        let (mut scene, arm) = sync_scene();
        // 旁观层：只驱动 leg、时间不推进（相位恒 0），充当「下半身」相位源
        let idle = std::sync::Arc::new(AnimClip {
            name: "idle".to_string(),
            duration: 1.0,
            tracks: vec![line_track(2, "root/leg", Vec3::new(100.0, 0.0, 0.0))],
        });
        let upper = AnimLayer::new("upper", idle, 1.0).with_mask(BoneMask::only(["leg"]));
        assert!(scene.add_anim_layer("demo/walk", upper));
        assert!(scene.set_anim_layer_speed("demo/walk", "upper", 0.0));
        assert!(scene.set_anim_layer_sync("demo/walk", "move", true));

        scene.update_animations(0.25);
        assert!((scene.anim_layer_sync_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
        assert!((scene.get(arm).unwrap().transform.position.x - 25.0).abs() < 1e-3);

        // 跨层对齐：move 跟随 upper（相位 0）→ 采样相位变 0，arm 回到起点
        assert!(scene.set_anim_layer_sync_source("demo/walk", "move", Some("upper")));
        assert_eq!(
            scene.anim_layer_sync_source("demo/walk", "move").as_deref(),
            Some("upper")
        );
        scene.update_animations(0.0);
        assert!(
            scene
                .anim_layer_sync_phase("demo/walk", "move")
                .unwrap()
                .abs()
                < 1e-4
        );
        let aligned = scene.get(arm).unwrap().transform.position.x;
        assert!(aligned.abs() < 1e-3, "对齐后应 0，实际 {aligned}");
        // 本层相位不受影响（编辑器画标尺仍读自己的）
        assert!((scene.anim_layer_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
        // 探针带 src 段
        let probe = scene
            .anim_layer_sync_probe("demo/walk", "move")
            .expect("层存在");
        assert!(probe.contains("src=upper"), "{probe}");
        assert!(probe.contains("drift=0.000"), "{probe}");

        // 自引用 / 源层不存在：直接拒
        assert!(!scene.set_anim_layer_sync_source("demo/walk", "move", Some("move")));
        assert!(!scene.set_anim_layer_sync_source("demo/walk", "move", Some("nope")));
        assert!(!scene.set_anim_layer_sync_source("demo/walk", "nope", Some("upper")));
        // 先让 move 跟 upper 时就能看出环：upper 尚未跟人，这里成功
        assert!(scene.set_anim_layer_sync_source("demo/walk", "move", Some("upper")));
        // 再让 upper 反跟 move 就成环，必须拒
        assert!(!scene.set_anim_layer_sync_source("demo/walk", "upper", Some("move")));

        // 清除后相位跟回自己
        assert!(scene.set_anim_layer_sync_source("demo/walk", "move", None));
        assert_eq!(
            scene.anim_layer_sync_source("demo/walk", "move").as_deref(),
            Some("")
        );
        assert!((scene.anim_layer_sync_phase("demo/walk", "move").unwrap() - 0.25).abs() < 1e-4);
        // 不存在的层没有相位可读
        assert!(scene.anim_layer_sync_phase("demo/walk", "nope").is_none());
        assert!(scene.anim_layer_sync_source("demo/walk", "nope").is_none());
    }

    #[test]
    fn layer_capacity_is_bounded() {
        let (mut scene, _arm, _leg) = layer_scene();
        let atk = std::sync::Arc::new(atk_clip());
        for i in 0..MAX_ANIM_LAYERS {
            assert!(
                scene.add_anim_layer(
                    "demo/walk",
                    AnimLayer::new(format!("l{i}"), atk.clone(), 1.0)
                ),
                "第 {i} 层应能挂载"
            );
        }
        assert!(
            !scene.add_anim_layer("demo/walk", AnimLayer::new("overflow", atk.clone(), 1.0)),
            "超出上限应拒绝"
        );
        // 同名覆盖不受上限限制
        assert!(scene.add_anim_layer("demo/walk", AnimLayer::new("l0", atk, 0.5)));
        assert_eq!(scene.anim_layer_count("demo/walk"), Some(MAX_ANIM_LAYERS));
    }

    #[test]
    fn layers_survive_main_clip_switch() {
        let (mut scene, arm, _leg) = layer_scene();
        let atk = atk_clip();
        let run = AnimClip {
            name: "run".to_string(),
            duration: 1.0,
            tracks: vec![
                line_track(1, "root/arm", Vec3::new(20.0, 0.0, 0.0)),
                line_track(2, "root/leg", Vec3::new(20.0, 0.0, 0.0)),
            ],
        };
        scene.register_anim_clip("demo/atk", std::sync::Arc::new(atk.clone()));
        scene.register_anim_clip("demo/run", std::sync::Arc::new(run));

        let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0)
            .with_mask(BoneMask::only(["arm"]));
        assert!(scene.add_anim_layer("demo/walk", layer));

        assert!(scene.set_anim_clip("demo/walk", "run"));
        assert_eq!(
            scene.anim_layer_count("demo/walk"),
            Some(1),
            "换主片段不应丢弃层"
        );

        scene.update_animations(0.5);
        let arm_x = scene.get(arm).unwrap().transform.position.x;
        assert!(
            (arm_x - 50.0).abs() < 1e-4,
            "换主片段后层仍应生效，实际 {arm_x}"
        );
    }

    #[test]
    fn clearing_layers_restores_base_pose() {
        let (mut scene, arm, _leg) = layer_scene();
        let atk = atk_clip();
        let layer = AnimLayer::new("upper", std::sync::Arc::new(atk), 1.0)
            .with_mask(BoneMask::only(["arm"]));
        assert!(scene.add_anim_layer("demo/walk", layer));
        scene.update_animations(0.5);
        assert!((scene.get(arm).unwrap().transform.position.x - 50.0).abs() < 1e-4);

        assert!(scene.clear_anim_layers("demo/walk"));
        assert!(!scene.clear_anim_layers("demo/walk"), "已空应返回 false");
        // 只重算不推进时间：清层后应回到主片段在 0.5s 的姿态
        scene.update_animations(0.0);
        let base_x = scene.get(arm).unwrap().transform.position.x;
        assert!(
            (base_x - 5.0).abs() < 1e-4,
            "清层后应回到主片段姿态，实际 {base_x}"
        );
    }
}

#[cfg(test)]
mod save_tests {
    //! 场景保存（save）的往返测试：`load -> save -> load` 必须语义等价。
    //!
    //! 覆盖仓库里随包发布的全部场景文件；用 `include_str!` 内联，测试不依赖工作目录。

    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    /// 随仓库发布的场景：(文件名, 文本)。
    const SHIPPED_SCENES: [(&str, &str); 6] = [
        ("scene.json", include_str!("../../../assets/scene.json")),
        (
            "scene_physics.json",
            include_str!("../../../assets/scene_physics.json"),
        ),
        (
            "scene_rapier.json",
            include_str!("../../../assets/scene_rapier.json"),
        ),
        (
            "scene_ccd.json",
            include_str!("../../../assets/scene_ccd.json"),
        ),
        (
            "scene_shadow.json",
            include_str!("../../../assets/scene_shadow.json"),
        ),
        (
            "scene_anim.json",
            include_str!("../../../assets/scene_anim.json"),
        ),
    ];

    /// 浮点比较的相对容差：`quat -> euler -> quat` 之类的换算会带来末位噪声。
    const EPS: f64 = 1e-5;

    /// 递归比较两个 JSON 值：数字按容差比较，其它类型严格比较，键集合必须一致。
    fn assert_json_close(a: &Value, b: &Value, path: &str) {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                let (x, y) = (
                    x.as_f64().expect("数字必须能转 f64"),
                    y.as_f64().expect("数字必须能转 f64"),
                );
                assert!(
                    (x - y).abs() <= EPS * x.abs().max(1.0),
                    "`{path}` 数值不一致: {x} vs {y}"
                );
            }
            (Value::Array(xs), Value::Array(ys)) => {
                assert_eq!(xs.len(), ys.len(), "`{path}` 数组长度不一致");
                for (i, (x, y)) in xs.iter().zip(ys.iter()).enumerate() {
                    assert_json_close(x, y, &format!("{path}[{i}]"));
                }
            }
            (Value::Object(xs), Value::Object(ys)) => {
                let xk: BTreeSet<&String> = xs.keys().collect();
                let yk: BTreeSet<&String> = ys.keys().collect();
                assert_eq!(xk, yk, "`{path}` 字段集合不一致");
                for (k, x) in xs {
                    assert_json_close(x, &ys[k], &format!("{path}.{k}"));
                }
            }
            _ => assert_eq!(a, b, "`{path}` 不一致"),
        }
    }

    /// 场景的可保存投影（`SceneFile` 序列化后的 JSON），用于整体比对。
    fn scene_projection(scene: &Scene) -> Value {
        serde_json::to_value(scene.to_scene_file()).expect("SceneFile 必须可序列化")
    }

    /// 断言两个场景的源数据等价。
    fn assert_scene_equivalent(a: &Scene, b: &Scene, label: &str) {
        assert_json_close(&scene_projection(a), &scene_projection(b), label);
        assert_eq!(a.len(), b.len(), "`{label}` 物件数量不一致");
        assert_eq!(
            a.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            b.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            "`{label}` 物件顺序不一致"
        );
    }

    #[test]
    fn shipped_scenes_round_trip() {
        for (name, src) in SHIPPED_SCENES {
            let loaded =
                Scene::from_json_str(src).unwrap_or_else(|e| panic!("{name} 加载失败: {e}"));
            let saved = loaded
                .to_json_string()
                .unwrap_or_else(|e| panic!("{name} 序列化失败: {e}"));
            assert!(!saved.trim().is_empty(), "{name} 序列化结果为空");
            let reloaded = Scene::from_json_str(&saved)
                .unwrap_or_else(|e| panic!("{name} 回读失败: {e}\n{saved}"));

            assert_eq!(loaded.name, reloaded.name, "{name} 场景名丢失");
            assert_scene_equivalent(&loaded, &reloaded, name);
        }
    }

    #[test]
    fn resave_after_reload_is_byte_stable() {
        // 第一次 save 会把文件规范成「引擎的写法」（欧拉角、省略缺省字段）；
        // 再 load -> save 应当逐字节一致，否则说明往返里还有未收敛的噪声。
        for (name, src) in SHIPPED_SCENES {
            let once = Scene::from_json_str(src)
                .unwrap_or_else(|e| panic!("{name} 加载失败: {e}"))
                .to_json_pretty()
                .expect("序列化");
            let twice = Scene::from_json_str(&once)
                .unwrap_or_else(|e| panic!("{name} 回读失败: {e}\n{once}"))
                .to_json_pretty()
                .expect("序列化");
            assert_eq!(once, twice, "{name} 规范化后应逐字节稳定");
        }
    }

    #[test]
    fn save_omits_runtime_state() {
        let src = include_str!("../../../assets/scene.json");
        let scene = Scene::from_json_str(src).expect("scene.json 必须合法");
        let value: Value = serde_json::from_str(&scene.to_json_string().expect("序列化"))
            .expect("序列化结果必须是合法 JSON");

        // 顶层不得出现运行时记账
        let top: BTreeSet<&String> = value.as_object().expect("场景必须是对象").keys().collect();
        for forbidden in [
            "time",
            "frame",
            "skins",
            "animations",
            "anim_clips",
            "next_id",
        ] {
            assert!(
                !top.contains(&forbidden.to_string()),
                "顶层不应写 `{forbidden}`"
            );
        }

        // 物件不得写世界矩阵缓存、刚体句柄、子节点索引
        let objects = value["objects"].as_array().expect("objects 必须是数组");
        assert!(!objects.is_empty(), "scene.json 应当有物件");
        for object in objects {
            let keys: BTreeSet<&String> =
                object.as_object().expect("物件必须是对象").keys().collect();
            for forbidden in ["id", "world", "world_normal", "body", "children"] {
                assert!(
                    !keys.contains(&forbidden.to_string()),
                    "物件不应写 `{forbidden}`：{object}"
                );
            }
        }
    }

    #[test]
    fn save_reflects_runtime_edits() {
        let mut scene = Scene::from_json_str(include_str!("../../../assets/scene.json"))
            .expect("scene.json 必须合法");

        // 运行时改动：新增物件、移动、隐藏、挂物理描述、改相机与光
        let id = scene
            .spawn(ObjectDesc {
                name: "saved_probe".to_string(),
                mesh: Some("sphere".to_string()),
                position: Some([1.5, 2.5, -3.5]),
                rotation: Some([0.0, 45.0, 0.0]),
                scale: Some([0.5, 0.5, 0.5]),
                visible: Some(false),
                physics: Some(PhysicsDesc {
                    body: "dynamic".to_string(),
                    mass: 2.5,
                    restitution: None,
                    friction: None,
                    collider: None,
                    half_extents: None,
                    ccd: Some(true),
                }),
                ..Default::default()
            })
            .expect("spawn 应成功");
        scene.get_mut(id).expect("物件应存在").visible = false;
        scene.camera.position = Vec3::new(1.0, 2.0, 3.0);
        scene.light.intensity = 0.75;
        scene.name = "edited".to_string();
        scene.update_world_transforms();

        let text = scene.to_json_pretty().expect("序列化");
        let back = Scene::from_json_str(&text).expect("回读必须成功");
        assert_scene_equivalent(&scene, &back, "edited");

        let probe = back.find("saved_probe").expect("新增物件必须写回");
        assert_eq!(probe.mesh, MeshRef::Builtin(crate::BuiltinMesh::Sphere));
        assert!(!probe.visible, "隐藏状态必须写回");
        assert_eq!(probe.transform.position, Vec3::new(1.5, 2.5, -3.5));
        assert_eq!(probe.transform.scale, Vec3::splat(0.5));
        let physics = probe.physics.as_ref().expect("物理描述必须写回");
        assert_eq!(physics.mass, 2.5);
        assert_eq!(physics.ccd, Some(true));
    }

    #[test]
    fn save_to_path_writes_readable_file() {
        let scene = Scene::from_json_str(include_str!("../../../assets/scene_shadow.json"))
            .expect("scene_shadow.json 必须合法");
        let dir = std::env::temp_dir().join(format!("r3e-save-{}", std::process::id()));
        let path = dir.join("nested/scene_out.json");

        scene
            .save_to_path(&path)
            .unwrap_or_else(|e| panic!("保存失败: {e}"));
        let text = std::fs::read_to_string(&path).expect("写入后必须能读回");
        assert_eq!(
            text,
            scene.to_json_pretty().expect("序列化"),
            "落盘内容应与内存序列化一致"
        );
        assert!(text.ends_with("}\n"), "落盘文件应以换行结尾");
        let back = Scene::from_json_str(&text).expect("落盘内容必须能回读");
        assert_scene_equivalent(&scene, &back, "scene_shadow.json/disk");

        std::fs::remove_file(&path).expect("清理临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hierarchy_parent_survives_round_trip() {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "root".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        scene
            .spawn(ObjectDesc {
                name: "child".to_string(),
                parent: Some("root".to_string()),
                position: Some([0.0, 1.0, 0.0]),
                ..Default::default()
            })
            .expect("spawn 应成功");
        scene.update_world_transforms();

        let back = Scene::from_json_str(&scene.to_json_string().expect("序列化")).expect("回读");
        let child = back.find("child").expect("子物件必须存在");
        let parent = back.find("root").expect("父物件必须存在");
        assert_eq!(child.parent, Some(parent.id), "父子关系必须写回");
        assert!(parent.children.contains(&child.id), "父节点应列出子节点");
        // 世界矩阵是算出来的，不是写出来的
        assert!((child.world.w_axis.y - 1.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod morph_exclusive_tests {
    use super::*;

    /// 扩展三十八：场景侧只负责「把组号落到实例上」，归一化本身是纯函数，
    /// 这里断言的是**读到的东西**与**传给渲染层的东西**一致。
    #[test]
    fn scene_carries_exclusive_groups_to_the_instance() {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "face".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        assert!(scene.bind_morph_by_name("face", 2));
        assert!(scene.set_morph_weight_by_name("face", 0, 1.0));
        assert!(scene.set_morph_weight_by_name("face", 1, 1.0));

        // 没设组时：两个形状各自独立，归一化后仍是 1.0 / 1.0
        assert_eq!(scene.morph_exclusive_by_name("face", 0), None);
        assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 1.0).abs() < 1e-6);

        // 同一组：组内和 2.0 被等比压回 1.0，各得 0.5
        assert!(scene.set_morph_exclusive_by_name("face", 0, 0));
        assert!(scene.set_morph_exclusive_by_name("face", 1, 0));
        assert_eq!(scene.morph_exclusive_by_name("face", 0), Some(0));
        let a = scene.morph_normalized_weight_by_name("face", 0).unwrap();
        let b = scene.morph_normalized_weight_by_name("face", 1).unwrap();
        assert!((a - 0.5).abs() < 1e-6, "实际 {a}");
        assert!((b - 0.5).abs() < 1e-6, "实际 {b}");
        // 场景原权重要原样留着，脚本调参会读自己写进去的值
        assert_eq!(scene.morph_weight_by_name("face", 0), Some(1.0));

        // 哨兵值 = 取出该形状，取出后回到互不影响
        assert!(scene.set_morph_exclusive_by_name("face", 1, NO_EXCLUSIVE_GROUP));
        assert_eq!(scene.morph_exclusive_by_name("face", 1), None);
        assert!((scene.morph_normalized_weight_by_name("face", 1).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn exclusive_api_rejects_unknown_names_and_bad_index() {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "face".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        // 没有绑定时一律拒绝
        assert!(!scene.set_morph_exclusive_by_name("face", 0, 0));
        assert_eq!(scene.morph_exclusive_by_name("face", 0), None);
        assert_eq!(scene.morph_normalized_weight_by_name("face", 0), None);
        assert!(!scene.set_morph_exclusive_by_name("nobody", 0, 0));

        assert!(scene.bind_morph_by_name("face", 2));
        // 超出 target_count 的下标：写不进、读不到
        assert!(!scene.set_morph_exclusive_by_name("face", 5, 0));
        assert_eq!(scene.morph_exclusive_by_name("face", 5), None);
        assert_eq!(scene.morph_normalized_weight_by_name("face", 5), None);
    }

    /// 履历：`target_count = 0` 的网格不建绑定，互斥组自然也无处可落。
    #[test]
    fn bind_morph_rejects_zero_targets() {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "block".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        assert!(!scene.bind_morph_by_name("block", 0));
        assert!(!scene.set_morph_exclusive_by_name("block", 0, 0));
    }
}

/// 扩展四十：场景侧的形状掩码接口 —— 脚本按名字开 / 关单个形状。
#[cfg(test)]
mod morph_mask_tests {
    use super::*;

    fn face_scene(shapes: usize) -> Scene {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "face".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        assert!(scene.bind_morph_by_name("face", shapes));
        scene
    }

    /// 契约一：默认全启用，读写都在 `target_count` 范围内。
    #[test]
    fn scene_mask_defaults_to_all_enabled() {
        let scene = face_scene(3);
        assert_eq!(scene.morph_mask_by_name("face"), Some(0b111));
        assert_eq!(scene.morph_shape_enabled_by_name("face", 0), Some(true));
        assert_eq!(
            scene.morph_shape_enabled_by_name("face", 3),
            None,
            "越界读不出「启用」"
        );
    }

    /// 契约二：整块写掩码时越界位被裁掉，回显的是**生效值**。
    ///
    /// 脚本写 `0xFFFF` 是常见手滑（把 u32 当全 1 用），既不能报错也不能
    /// 原样回显 —— 否则脚本读回来的数与渲染层吃的数不是一回事。
    #[test]
    fn scene_mask_write_returns_clipped_value() {
        let mut scene = face_scene(3);
        assert_eq!(scene.set_morph_mask_by_name("face", u32::MAX), Some(0b111));
        assert_eq!(scene.morph_mask_by_name("face"), Some(0b111));
    }

    /// 契约三：单形状开关落到实例上，且直接影响归一化结果。
    ///
    /// 这是掩码与「权重置 0」的分界：关掉形状 1 后，形状 0 的权重不该再被
    /// 那个看不见的形状拖去平分。
    #[test]
    fn scene_mask_toggle_affects_normalized_weights() {
        let mut scene = face_scene(2);
        assert!(scene.set_morph_weight_by_name("face", 0, 1.0));
        assert!(scene.set_morph_weight_by_name("face", 1, 1.0));
        assert!(scene.set_morph_exclusive_by_name("face", 0, 0));
        assert!(scene.set_morph_exclusive_by_name("face", 1, 0));
        // 同组各 1.0 → 各 0.5
        assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 0.5).abs() < 1e-6);

        assert!(scene.set_morph_shape_enabled_by_name("face", 1, false));
        assert_eq!(scene.morph_shape_enabled_by_name("face", 1), Some(false));
        assert!(
            (scene.morph_normalized_weight_by_name("face", 0).unwrap() - 1.0).abs() < 1e-6,
            "被掩码挡住后组里只剩形状 0 独自 1.0，不该再折半"
        );
        assert_eq!(
            scene.morph_normalized_weight_by_name("face", 1),
            Some(0.0),
            "掩码外的形状读到 0"
        );
        // 场景原权重不被动过
        assert_eq!(scene.morph_weight_by_name("face", 1), Some(1.0));

        assert!(scene.set_morph_shape_enabled_by_name("face", 1, true));
        assert!((scene.morph_normalized_weight_by_name("face", 0).unwrap() - 0.5).abs() < 1e-6);
    }

    /// 契约五：掩码是**实例级**的 —— 同一网格的两个节点各持一份，互不影响。
    ///
    /// 这是扩展四十存在的理由：三十九之前 `count` 是网格级的，同网格所有实例
    /// 共享同一套形状，想「这个实例只开两个形状」只能把权重置 0 假装 ——
    /// 顶点照样过 8 次乘法。
    #[test]
    fn mask_is_per_instance_not_per_mesh() {
        let mut scene = Scene::new();
        for name in ["left", "right"] {
            scene
                .spawn(ObjectDesc {
                    name: name.to_string(),
                    ..Default::default()
                })
                .expect("spawn 应成功");
            assert!(scene.bind_morph_by_name(name, 3));
        }

        assert_eq!(scene.set_morph_mask_by_name("left", 0b011), Some(0b011));
        assert_eq!(scene.set_morph_mask_by_name("right", 0b110), Some(0b110));

        assert_eq!(scene.morph_mask_by_name("left"), Some(0b011));
        assert_eq!(scene.morph_mask_by_name("right"), Some(0b110));
        assert_eq!(scene.morph_shape_enabled_by_name("left", 0), Some(true));
        assert_eq!(scene.morph_shape_enabled_by_name("right", 0), Some(false));

        // 单形状开关也只动自己那一个实例
        assert!(scene.set_morph_shape_enabled_by_name("left", 1, false));
        assert_eq!(scene.morph_mask_by_name("left"), Some(0b001));
        assert_eq!(
            scene.morph_mask_by_name("right"),
            Some(0b110),
            "另一个实例的掩码不该被牵连"
        );
    }

    /// 契约四：没有绑定 / 名字不存在 / 下标越界都只返回 None 或 false，不静默改状态。
    #[test]
    fn scene_mask_rejects_unknown_targets() {
        let mut scene = Scene::new();
        scene
            .spawn(ObjectDesc {
                name: "block".to_string(),
                ..Default::default()
            })
            .expect("spawn 应成功");
        assert_eq!(scene.morph_mask_by_name("block"), None, "没绑定就没有掩码");
        assert_eq!(scene.set_morph_mask_by_name("block", 0), None);
        assert!(!scene.set_morph_shape_enabled_by_name("block", 0, false));
        assert_eq!(scene.morph_shape_enabled_by_name("block", 0), None);

        assert_eq!(scene.set_morph_mask_by_name("nobody", 0b1), None);
        assert!(scene.bind_morph_by_name("block", 2));
        assert!(
            !scene.set_morph_shape_enabled_by_name("block", 5, false),
            "越界下标拒绝"
        );
        assert_eq!(
            scene.morph_mask_by_name("block"),
            Some(0b11),
            "被拒的写不改变掩码"
        );
    }
}
