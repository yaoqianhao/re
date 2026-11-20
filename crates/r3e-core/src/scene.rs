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

use crate::scene_material::{self, SceneMaterialDecl};
use crate::skin::SkinInstance;
use crate::spawn::{self, SpawnDecl};
use crate::terrain::{self, TerrainPatch};
use crate::{
    AnimClip, AnimInstance, Camera, EnvironmentMapSettings, EnvironmentSettings, Light, MeshRef,
    Node, NodeId, OcclusionSettings, PostFxSettings, Transform,
};

pub use crate::scene_desc::{ObjectDesc, PhysicsDesc, PhysicsSceneDesc};

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
    /// 地形数据壳（M1-S3-C）：一块地表 = 名字 + 世界矩形 + 高度样本 + 地表材质参数。
    /// 空表示场景没有地形。M1 只承载数据、不出几何（M2 才求值三角网）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terrain: Vec<TerrainPatch>,
    /// 出生点声明（M1-S4-B）：导航验证的起点来源。空表示场景没有声明出生点，
    /// 导航层只能按「调用方给的起点」判定可达性。**不新增节点种类**——出生点不是物件。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawns: Vec<SpawnDecl>,
    /// 场景材质表（M1-S4-A）：材质名 → 参数 + 五张贴图路径。空表示没有共享材质，
    /// 所有物件走内联 `material` 或网格资产材质。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<SceneMaterialDecl>,
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

/// 场景：所有可渲染物件的唯一真源。`Clone` 供文档层事务的影子快照（preview 在克隆上预演，主真值不碰）。
#[derive(Debug, Clone)]
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
    /// 地形数据壳（M1-S3-C）。每块地表 = 名字 + 世界矩形 + 高度样本 + 地表材质参数，
    /// 名字是唯一主键（同名覆盖、顺序不变），上限 [`TERRAIN_MAX_PATCHES`]。
    /// M1 只承载数据：渲染层不读它，几何求值属 M2。
    pub terrain: Vec<TerrainPatch>,
    /// 出生点表（M1-S4-B）。名字是唯一主键（同名覆盖、顺序不变），上限
    /// [`crate::spawn::SPAWN_MAX`]。导航子系统以首个出生点为默认起点，
    /// 验证器据此判断「出生点是否落在可走格上」「能不能走到目标」。
    pub spawns: Vec<SpawnDecl>,
    /// 场景材质表（M1-S4-A）。每行 = 名字 + 三标量 + 五张贴图路径 + uv 缩放，
    /// 名字是唯一主键（同名覆盖、顺序不变），上限 [`SCENE_MATERIAL_MAX`]。
    /// 节点通过 [`crate::Node::material_ref`] 按名字引用，一份材质可被多个节点复用。
    pub materials: Vec<SceneMaterialDecl>,
    /// 场景材质贴图相对路径的基准目录（M1-S4-A）。
    ///
    /// 为什么放在场景上而不是只放在 `Document` 上：渲染层要拿它把 `materials[]` 里的
    /// 相对路径算成 `AssetId`，而渲染层只看得见 `Scene`。它与 **不进序列化** ——
    /// 目录是「这个文件被打开时在哪」，属于运行期状态，写进 .r3e 会让同一份场景
    /// 换个目录就变成另一个文件。载入方（`Document::set_base_dir`）负责同步设置。
    pub texture_base_dir: Option<std::path::PathBuf>,
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
            terrain: Vec::new(),
            spawns: Vec::new(),
            materials: Vec::new(),
            texture_base_dir: None,
            occlusion: OcclusionSettings::default(),
            environment_map: None,
        }
    }

    // ---------- 局部光源（扩展十五） ----------

    // ---------- 遮挡剔除（扩展二十二） ----------

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
        // 材质引用同理：去空白、空串即「不引用」。表里有没有这份材质由校验器判（可跨文件改）。
        node.material_ref = scene_material::normalize_material_ref(desc.material_ref);
        // 标签与交互半径同理：收敛在唯一入口做，查询面看到的就是文件里语义上的那部分
        node.tags = desc
            .tags
            .as_deref()
            .map(crate::normalize_tags)
            .unwrap_or_default();
        node.interact_radius = desc
            .interact_radius
            .and_then(crate::normalize_interact_radius);

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

    /// 下一个待分配节点 id：状态哈希必须覆盖它，否则「同脚本分配不同 `NodeId`」会假绿（M0-S4）。
    pub fn next_id(&self) -> NodeId {
        self.next_id
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
}

// ── 域子模块：impl Scene 按职责拆分（M2-A04），字段仍归本文件 ──
mod anim;
mod anim_blendspace;
mod anim_layer;
mod hierarchy;
mod io;
mod morph;
mod occlusion;
mod skin;

#[cfg(test)]
mod tests_anim_a;
#[cfg(test)]
mod tests_anim_b;
#[cfg(test)]
mod tests_anim_support;
#[cfg(test)]
mod tests_morph;
#[cfg(test)]
mod tests_save;
