//! r3e-editor —— 编辑器桥（phase 1）。
//!
//! 职责只有两件事：
//!
//! 1. **协议**：JSON-RPC 2.0 的请求 / 响应 / 错误码 / 方法表，以及 `EngineApi` 抽象；
//! 2. **传输**：`127.0.0.1:<port>` 上的 TCP + NDJSON，读写各在自己的线程里，
//!    **引擎状态只由主线程碰**（见 [`server::Job`]）。
//!
//! # 为什么本 crate 不依赖 `r3e-core`
//!
//! 协议层只认 JSON 值，不认 `NodeId` / `Vec3`。这样编辑器桥与引擎内核之间是
//! 单向依赖（宿主 `r3e-app` 实现 [`EngineApi`]），换内部类型不必改协议，
//! 也不会出现 `core -> editor` 的反向耦合。
//!
//! # 线程模型
//!
//! ```text
//! 编辑器(客户端)  <-- TCP -->  连接线程（读行 / 写行）
//!                                  |  Job{request, reply}
//!                                  v
//!                            mpsc 队列（只在主线程 drain）
//!                                  |
//!                            主线程 dispatch(EngineApi)  --> reply
//! ```
//!
//! 主线程每帧只取有限条（默认 8），保证一帧的 RPC 开销有上界；RPC 线程从不
//! 触碰 `Scene` / `Physics` / 脚本引擎。

pub mod server;

pub use server::{Job, RpcServer};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// JSON-RPC 版本字符串。
pub const JSONRPC: &str = "2.0";
/// 桥协议版本（方法表或字段语义不兼容地变更时 +1）。
pub const PROTOCOL_VERSION: u32 = 1;

// JSON-RPC 2.0 标准错误码
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// 引擎侧业务错误（例如「找不到物件」）。
pub const ENGINE_ERROR: i32 = -32000;
/// 鉴权失败（配置了 `--rpc-token` 但请求没带或带错）。
pub const UNAUTHORIZED: i32 = -32001;
/// 桥过载：待处理队列已满，或并发连接数达到上限。客户端应稍后重试。
pub const SERVER_BUSY: i32 = -32002;
/// 主线程在超时时间内没有回包（引擎卡顿或主循环已停）。
pub const RPC_TIMEOUT: i32 = -32003;

/// 请求：`{"jsonrpc":"2.0","id":1,"method":"scene.list","params":{...}}`
///
/// `params` 缺省为空对象；`id` 缺省表示通知（服务器不回包）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "default_version")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

fn default_version() -> String {
    JSONRPC.to_string()
}

impl Request {
    /// 从 JSON 值构造并做最小校验（版本号 + 方法名非空）。
    pub fn from_value(value: Value) -> Result<Self, RpcFailure> {
        let req: Request = serde_json::from_value(value)
            .map_err(|e| RpcFailure::new(INVALID_REQUEST, format!("请求结构不合法: {e}")))?;
        if req.jsonrpc != JSONRPC {
            return Err(RpcFailure::new(
                INVALID_REQUEST,
                format!("jsonrpc 字段必须是 `{JSONRPC}`，收到 `{}`", req.jsonrpc),
            ));
        }
        if req.method.trim().is_empty() {
            return Err(RpcFailure::new(INVALID_REQUEST, "method 不能为空"));
        }
        Ok(req)
    }

    /// 是否通知（无 `id`，按 JSON-RPC 规范不回包）。
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 响应：成功给 `result`，失败给 `error`，二者互斥。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn fail(id: Option<Value>, failure: RpcFailure) -> Self {
        Self {
            jsonrpc: JSONRPC.to_string(),
            id,
            result: None,
            error: Some(RpcError::from(failure)),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// 序列化成单行（NDJSON 不允许裸换行）。
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"{JSONRPC}","id":null,"error":{{"code":{INTERNAL_ERROR},"message":"响应序列化失败: {e}"}}}}"#
            )
        })
    }
}

/// 错误对象（线上表示）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 引擎侧失败原因，`EngineApi` 用它报错，再被转成 [`RpcError`]。
#[derive(Debug, Clone, PartialEq)]
pub struct RpcFailure {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcFailure {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// 参数缺失 / 类型不对。
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    /// 物件 / 资源不存在。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ENGINE_ERROR, message)
    }

    /// 引擎执行失败（脚本报错、spawn 失败等）。
    pub fn engine(message: impl Into<String>) -> Self {
        Self::new(ENGINE_ERROR, message)
    }

    /// 本期未实现的方法。
    pub fn unsupported(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("方法 `{method}` 未实现"))
    }
}

impl From<RpcFailure> for RpcError {
    fn from(f: RpcFailure) -> Self {
        Self {
            code: f.code,
            message: f.message,
            data: f.data,
        }
    }
}

/// 引擎信息快照（`engine.info` 的 `result`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub scene: String,
    pub script: String,
    pub frames: u64,
    pub fps: f32,
    pub uptime_ms: u64,
    pub paused: bool,
    pub nodes: usize,
    pub physics: PhysicsInfo,
}

/// 物理子系统状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicsInfo {
    pub enabled: bool,
    pub backend: String,
    pub capabilities: Vec<String>,
    pub bodies: usize,
    pub ccd_default: bool,
}

/// `scene.list` 的单条结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeBrief {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    pub position: [f32; 3],
    pub visible: bool,
}

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
}

impl AnimLayerPatch {
    pub fn is_empty(&self) -> bool {
        self.weight.is_none()
            && self.mode.is_none()
            && self.mask.is_none()
            && self.speed.is_none()
            && self.clip.is_none()
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

/// 宿主（`r3e-app`）要实现的引擎操作集合。
///
/// 全部方法都在**主线程**调用；实现方可以直接借用 `Scene` / `Physics` / 脚本宿主。
/// 缺省实现返回「未实现」，这样宿主可以只点亮本期需要的方法。
pub trait EngineApi {
    fn info(&mut self) -> EngineInfo;

    fn scene_list(
        &mut self,
        _filter: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<NodeBrief>, RpcFailure> {
        Err(RpcFailure::unsupported("scene.list"))
    }

    fn scene_get(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.get"))
    }

    fn scene_set_transform(
        &mut self,
        _node: &str,
        _patch: TransformPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_transform"))
    }

    fn scene_spawn(&mut self, _object: Value) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.spawn"))
    }

    /// 改写物件的 PBR 材质（`metallic` / `roughness` / `emissive` 可分别给）。
    /// 宿主应把补丁合并进现有材质后立即收敛，并把合并结果回给调用方。
    fn scene_set_material(
        &mut self,
        _node: &str,
        _patch: MaterialPatch,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_material"))
    }

    /// 改写场景级环境探针（三色天空 / 太阳 / 总强度）。
    /// 宿主应把补丁合并进现有环境后立即收敛，并把合并结果回给调用方；
    /// `enabled = false` 时清空环境（回到常数 `ambient`），返回值里 `environment` 为 `null`。
    fn scene_set_environment(&mut self, _patch: EnvironmentPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_environment"))
    }

    /// 改写场景级 HDR 后处理段（曝光 / 色调映射 / 泛光 / FXAA）。
    /// 宿主应把补丁合并进现有设置后立即收敛，并把合并结果回给调用方；
    /// `enabled = false` 时清空该段（直通），返回值里 `postfx` 为 `null`。
    fn scene_set_postfx(&mut self, _patch: PostFxPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_postfx"))
    }

    /// 改写后处理段下的屏幕空间环境光遮蔽（扩展十七）。
    /// 宿主应把补丁合并进现有设置后立即收敛，并把合并结果回给调用方；
    /// 与 `scene_set_postfx` 的区别：`enabled = false` 只掐遮蔽这一条支线，
    /// 不清空整段后处理（曝光 / 泛光 / FXAA 照旧生效）。
    fn scene_set_ssao(&mut self, _patch: SsaoPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.set_ssao"))
    }

    fn scene_remove(&mut self, _node: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.remove"))
    }

    /// 把内存中的当前场景写回磁盘（`path` 为 `None` 时由宿主决定默认路径，
    /// 例如「写回本场景的原始文件」）。实现方应返回落盘后的绝对路径与物件数量。
    fn scene_save(&mut self, _path: Option<&str>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.save"))
    }

    /// 列出动画混合树层（扩展十八 A）：`{ count, limit, layers: [...] }`。
    ///
    /// `instance` 为 `None` 时列出全部动画实例的层；给了名字但实例不存在，
    /// 宿主应报参数错误（编辑器拼错实例名时，回空列表比回错误更难排查）。
    fn scene_anim_layers(
        &mut self,
        _instance: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.anim_layers"))
    }

    /// 改动一个混合树层（权重 / 模式 / 遮罩 / 速度 / 片段），返回收敛后的层对象。
    ///
    /// 编辑器侧改动一律**立即生效**，不做过渡——过渡是玩法逻辑，属于脚本。
    fn scene_anim_layer_set(&mut self, _patch: AnimLayerPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.anim_layer_set"))
    }

    /// 列出场景栈（扩展十八 B）：`{ count, current, max, entries: [...] }`。
    ///
    /// 每层只给**声明态**（层号 / 名字 / 来源路径 / 源文本字节数 / 帧号与时间），
    /// 不给场景内容本身——面板刷新是每帧都在发的请求，把十几层的场景 JSON 一起搬
    /// 过去会让序列化变成瓶颈。层的内容只在 `scene.back` 时由宿主装载。
    fn scene_stack(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.stack"))
    }

    /// 切到一个场景文件：宿主把当前场景压栈，再把目标场景装进**同一个**场景真源。
    ///
    /// 这条路必须由宿主走完（换资产库、重建动画实例、重跑脚本、刷新渲染层缓存），
    /// 编辑器只发路径，不关心顺序。
    fn scene_switch(&mut self, _path: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.switch"))
    }

    /// 回到栈里的上一层（不删层，可再用前进语义回来）。
    fn scene_back(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.back"))
    }

    /// 存一份带运行时快照的存档（`path` 为 `None` 时由宿主决定默认路径）。
    ///
    /// 与 `scene_save` 的区别：`scene_save` 只写场景**声明**（重开即源状态），
    /// 存档还要把"这一局跑到哪"（时间 / 帧号 / 通道开关 / 动画播放进度）一起装进去。
    fn scene_archive_save(&mut self, _path: Option<&str>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.archive.save"))
    }

    /// 读档：按存档重建当前场景并恢复运行时状态。
    ///
    /// 读档前宿主应把当前场景压栈，因此「读档 → 反悔」等价于一次 `scene.back`。
    fn scene_archive_load(&mut self, _path: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.archive.load"))
    }

    /// 列出当前 2D UI 元素（扩展十四）。
    ///
    /// 返回的是**声明**（单位串、层级、可见性），不是换算后的像素矩形——像素要屏幕
    /// 尺寸才能算，而屏幕尺寸属于宿主状态，编辑器要显示实际矩形时应自己带上视口大小。
    fn ui_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.list"))
    }

    /// 写入 / 局部更新一个 UI 元素（同 key 覆盖）。
    fn ui_set(&mut self, _patch: UiPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.set"))
    }

    /// 按 key 删除一个 UI 元素。
    fn ui_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.remove"))
    }

    /// 读当前指针交互状态（hover / active / 本帧事件），供编辑器显示与联调。
    fn ui_state(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("ui.state"))
    }

    /// 列出当前局部光源（扩展十五）：`{ enabled, count, lights: [...] }`。
    ///
    /// 每盏灯给出**声明态**（kind / pos / dir / color / intensity / range / 内外角），
    /// 不含衰减后的辐亮度——那取决于查询点，属于 GPU / 脚本的认知范围。
    fn light_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.list"))
    }

    /// 写入 / 局部更新一盏局部光源（同 key 覆盖）。
    fn light_set(&mut self, _patch: LightPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.set"))
    }

    /// 按 key 删除一盏局部光源。
    fn light_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.remove"))
    }

    /// 清空全部局部光源，返回删掉的盏数。
    fn light_clear(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.clear"))
    }

    /// 读 / 写局部光照总开关（`None` 表示只读）。
    ///
    /// 关掉只是不上传（场景数据保留），因此编辑器的「取消勾选」不会丢灯。
    fn light_set_enabled(&mut self, _enabled: Option<bool>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("light.set_enabled"))
    }

    /// 列出已注册的 3D 空间化声源（§12 第十项）：
    /// `{ enabled, count, limit, max, supports_speed, listener, sources: [...] }`。
    ///
    /// 每个声源同时给**声明态**与**上一帧结算结果**：`position` / `node` /
    /// `model` / `minDistance` / `maxDistance` / `doppler` 是编辑器能改的，
    /// `gain` / `rate` / `distance` 是宿主每帧推进出来的——面板要显示「这个声源
    /// 现在多响、倍速多少」，编辑器自己算不出来，所以必须由宿主回传。
    fn audio_spatial_list(&mut self, _limit: Option<usize>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.list"))
    }

    /// 注册 / 局部更新一个空间化声源（同 key 覆盖），返回收敛后的声源对象。
    ///
    /// 注册与摆位是**同一个方法**：编辑器新建声源时一次把位置与衰减参数发全，
    /// 免得先 attach 再 setPosition 发两趟。超上限时宿主应报参数错误而不是静默丢。
    fn audio_spatial_set(&mut self, _patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.set"))
    }

    /// 解除一个声源的注册（正在播的声音继续播完，只是不再做空间化）。
    fn audio_spatial_remove(&mut self, _key: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.remove"))
    }

    /// 清空全部空间化声源，返回解除的条数。
    fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.clear"))
    }

    /// 读 / 写听者位姿（`None` = 只读）。
    ///
    /// 正常运行时听者由「当前活动相机」每帧推导，编辑器手改只在暂停调试时有用；
    /// 因此这里的写入是**一次性**的，下一帧会被 `app` 的相机推导覆盖。
    fn audio_spatial_listener(
        &mut self,
        _patch: Option<ListenerPatch>,
    ) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.listener"))
    }

    /// 读 / 写空间化总开关（关掉 = 全部声源退回平面声，声源数据保留）。
    fn audio_spatial_set_enabled(&mut self, _enabled: Option<bool>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("audio.spatial.set_enabled"))
    }

    /// 读 / 写方向光阴影设置（扩展十六）：`None` = 只读，`Some(patch)` = 合并后回写。
    ///
    /// 回包固定给出收敛后的全部字段（含 `cascades` 与 `distance`），
    /// 编辑器据此刷新面板，不必再补一次读。
    fn scene_shadow(&mut self, _patch: Option<ShadowPatch>) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("scene.shadow"))
    }

    fn script_eval(&mut self, _source: &str) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.eval"))
    }

    fn script_reload(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("script.reload"))
    }
}

/// 方法名归一化：忽略大小写与下划线，`scene.setTransform` 与 `scene.set_transform` 等价。
fn canonical(method: &str) -> String {
    method
        .chars()
        .filter(|c| *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn param<'a>(params: &'a Value, key: &str) -> Option<&'a Value> {
    params.get(key)
}

fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcFailure> {
    param(params, key)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcFailure::invalid_params(format!("缺少字符串参数 `{key}`")))
}

/// 取出可选数字参数；缺省得到 `None`，给了但不是数字则报错。
fn opt_f32(params: &Value, key: &str) -> Result<Option<f32>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    value
        .as_f64()
        .map(|v| Some(v as f32))
        .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为数字")))
}

/// 取出可选字符串参数；缺省得到 `None`（含显式 `null`），给了但不是字符串则报错。
fn opt_str<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为字符串"
        ))),
    }
}

/// 取出可选布尔参数；缺省得到 `None`，给了但不是布尔则报错。
/// 取出可选整数参数；缺省得到 `None`，给了但不是整数则报错。
fn opt_i32(params: &Value, key: &str) -> Result<Option<i32>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let n = value.as_i64().ok_or_else(|| {
        RpcFailure::invalid_params(format!("参数 `{key}` 应为整数，收到 {value}"))
    })?;
    i32::try_from(n)
        .map(Some)
        .map_err(|_| RpcFailure::invalid_params(format!("参数 `{key}` 超出 i32 范围")))
}

fn opt_bool(params: &Value, key: &str) -> Result<Option<bool>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为布尔值")))
}

fn opt_f32_array<const N: usize>(
    params: &Value,
    key: &str,
) -> Result<Option<[f32; N]>, RpcFailure> {
    let Some(value) = param(params, key) else {
        return Ok(None);
    };
    let arr = value.as_array().ok_or_else(|| {
        RpcFailure::invalid_params(format!("参数 `{key}` 应为 {N} 个数字组成的数组"))
    })?;
    if arr.len() != N {
        return Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 需要 {N} 个数字，收到 {} 个",
            arr.len()
        )));
    }
    let mut out = [0.0f32; N];
    for (slot, item) in out.iter_mut().zip(arr) {
        *slot = item
            .as_f64()
            .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 含非数字元素")))?
            as f32;
    }
    Ok(Some(out))
}

/// 把一条请求派发到 [`EngineApi`]，永不 panic（未知方法走 `METHOD_NOT_FOUND`）。
pub fn dispatch(api: &mut dyn EngineApi, req: &Request) -> Response {
    let id = req.id.clone();
    match canonical(&req.method).as_str() {
        "ping" => Response::ok(
            id,
            json!({ "pong": true, "protocol": PROTOCOL_VERSION, "version": env!("CARGO_PKG_VERSION") }),
        ),
        "engine.info" => match serde_json::to_value(api.info()) {
            Ok(value) => Response::ok(id, value),
            Err(e) => Response::fail(id, RpcFailure::new(INTERNAL_ERROR, e.to_string())),
        },
        "scene.list" => {
            let filter = param(&req.params, "filter").and_then(Value::as_str);
            let limit = match param(&req.params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.scene_list(filter, limit) {
                Ok(nodes) => Response::ok(id, json!({ "count": nodes.len(), "nodes": nodes })),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.get" => match with_node(&req.params, |node| api.scene_get(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.settransform" => {
            let patch = match (
                opt_f32_array::<3>(&req.params, "position"),
                opt_f32_array::<4>(&req.params, "rotation"),
                opt_f32_array::<3>(&req.params, "scale"),
            ) {
                (Ok(p), Ok(r), Ok(s)) => TransformPatch {
                    position: p,
                    rotation: r,
                    scale: s,
                },
                (Err(f), _, _) | (_, Err(f), _) | (_, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "position / rotation / scale 至少要给一个（否则这次调用没有意义）",
                    ),
                );
            }
            match with_node(&req.params, |node| api.scene_set_transform(node, patch)) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.spawn" => {
            let object = param(&req.params, "object")
                .or_else(|| param(&req.params, "desc"))
                .cloned()
                .unwrap_or(Value::Null);
            if !object.is_object() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params("缺少对象参数 `object`（物件描述）"),
                );
            }
            match api.scene_spawn(object) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.remove" => match with_node(&req.params, |node| api.scene_remove(node)) {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        // 材质契约：metallic / roughness / emissive 三个字段可分别给，
        // 一个都不给视为无意义调用；具体收敛由宿主材质结构完成。
        "scene.setmaterial" => {
            let patch = match (
                opt_f32(&req.params, "metallic"),
                opt_f32(&req.params, "roughness"),
                opt_f32_array::<3>(&req.params, "emissive"),
            ) {
                (Ok(m), Ok(r), Ok(e)) => MaterialPatch {
                    metallic: m,
                    roughness: r,
                    emissive: e,
                },
                (Err(f), _, _) | (_, Err(f), _) | (_, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "metallic / roughness / emissive 至少要给一个（否则这次调用没有意义）",
                    ),
                );
            }
            match with_node(&req.params, |node| api.scene_set_material(node, patch)) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 环境探针契约：字段可分别给，一个都不给视为无意义调用；
        // `enabled = false` 是唯一的「清空」入口，与「什么都没给」区分开。
        "scene.setenvironment" => {
            let patch = match (
                opt_bool(&req.params, "enabled"),
                opt_f32_array::<3>(&req.params, "sky_zenith"),
                opt_f32_array::<3>(&req.params, "sky_horizon"),
                opt_f32_array::<3>(&req.params, "ground"),
                opt_f32_array::<3>(&req.params, "sun_dir"),
                opt_f32_array::<3>(&req.params, "sun_color"),
                opt_f32(&req.params, "sun_intensity"),
                opt_f32(&req.params, "intensity"),
            ) {
                (
                    Ok(enabled),
                    Ok(zenith),
                    Ok(horizon),
                    Ok(ground),
                    Ok(dir),
                    Ok(sun),
                    Ok(si),
                    Ok(i),
                ) => EnvironmentPatch {
                    enabled,
                    sky_zenith: zenith,
                    sky_horizon: horizon,
                    ground,
                    sun_dir: dir,
                    sun_color: sun,
                    sun_intensity: si,
                    intensity: i,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "environment 补丁至少要给一个字段（或 enabled=false 清空）",
                    ),
                );
            }
            match api.scene_set_environment(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 后处理契约（扩展十三）：字段可分别给，一个都不给视为无意义调用；
        // `enabled = false` 是唯一的「清空」入口。`tonemap` 名字的合法性由宿主判定。
        "scene.setpostfx" => {
            let patch = match (
                opt_bool(&req.params, "enabled"),
                opt_f32(&req.params, "exposure"),
                opt_str(&req.params, "tonemap"),
                opt_bool(&req.params, "bloom_enabled"),
                opt_f32(&req.params, "bloom_threshold"),
                opt_f32(&req.params, "bloom_knee"),
                opt_f32(&req.params, "bloom_intensity"),
                opt_bool(&req.params, "fxaa"),
            ) {
                (
                    Ok(enabled),
                    Ok(exposure),
                    Ok(tonemap),
                    Ok(bloom_enabled),
                    Ok(bloom_threshold),
                    Ok(bloom_knee),
                    Ok(bloom_intensity),
                    Ok(fxaa),
                ) => PostFxPatch {
                    enabled,
                    exposure,
                    tonemap: tonemap.map(str::to_string),
                    bloom_enabled,
                    bloom_threshold,
                    bloom_knee,
                    bloom_intensity,
                    fxaa,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "postfx 补丁至少要给一个字段（或 enabled=false 清空）",
                    ),
                );
            }
            match api.scene_set_postfx(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 环境光遮蔽契约（扩展十七）：字段可分别给，一个都不给视为无意义调用；
        // 与 set_postfx 不同，`enabled = false` 只关遮蔽，后处理段其余设置保留。
        "scene.setssao" => {
            // 显式 `null` 一律视为「没给」：不少 JSON 客户端会把未填字段序列化成 `null`。
            let enabled = match param(&req.params, "enabled") {
                None | Some(Value::Null) => Ok(None),
                Some(_) => opt_bool(&req.params, "enabled"),
            };
            let patch = match (
                enabled,
                opt_num(&req.params, "radius"),
                opt_num(&req.params, "bias"),
                opt_num(&req.params, "intensity"),
                opt_i32(&req.params, "samples"),
            ) {
                (Ok(enabled), Ok(radius), Ok(bias), Ok(intensity), Ok(samples)) => SsaoPatch {
                    enabled,
                    radius,
                    bias,
                    intensity,
                    samples,
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(id, RpcFailure::invalid_params("ssao 补丁至少要给一个字段"));
            }
            match api.scene_set_ssao(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 动画混合树契约（扩展十八 A）：`scene.anim_layers` 只读，`scene.anim_layer_set`
        // 补丁式写入。模式名 / 遮罩串原样透传，认不认识由宿主的 `LayerMode::parse`
        // 与遮罩解析决定——本层同样不认识 `r3e_core` 的任何类型。
        "scene.animlayers" => {
            // `opt_str` 已把显式 null 当「未给」：编辑器面板常把「没选实例」序列化成 null。
            let instance = match opt_str(&req.params, "instance") {
                Ok(v) => v.map(|s| s.to_string()),
                Err(f) => return Response::fail(id, f),
            };
            let limit = match param(&req.params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.scene_anim_layers(instance.as_deref(), limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.animlayerset" => {
            let instance = match require_str(&req.params, "instance") {
                Ok(s) if !s.trim().is_empty() => s.to_string(),
                Ok(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `instance` 不能为空"),
                    )
                }
                Err(f) => return Response::fail(id, f),
            };
            // `layer` 缺省 = 第一层，与脚本侧「空名取第一层」保持一致。
            let layer = match opt_str(&req.params, "layer") {
                Ok(v) => v.unwrap_or_default().to_string(),
                Err(f) => return Response::fail(id, f),
            };
            let patch = match (
                opt_num(&req.params, "weight"),
                opt_str(&req.params, "mode"),
                opt_str(&req.params, "mask"),
                opt_num(&req.params, "speed"),
                opt_str(&req.params, "clip"),
            ) {
                (Ok(weight), Ok(mode), Ok(mask), Ok(speed), Ok(clip)) => AnimLayerPatch {
                    instance,
                    layer,
                    weight,
                    mode: mode.map(|s| s.to_string()),
                    mask: mask.map(|s| s.to_string()),
                    speed,
                    clip: clip.map(|s| s.to_string()),
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            if patch.is_empty() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params("anim_layer_set 补丁至少要给一个字段"),
                );
            }
            match api.scene_anim_layer_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // UI 契约（扩展十四）：位置 / 尺寸用字符串单位透传，名字合法性由宿主判定。
        "ui.list" => {
            let limit = match param(&req.params, "limit") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) => Some(n as usize),
                    None => {
                        return Response::fail(
                            id,
                            RpcFailure::invalid_params("参数 `limit` 应为非负整数"),
                        )
                    }
                },
            };
            match api.ui_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.set" => {
            let key = match require_str(&req.params, "key") {
                Ok(k) if !k.trim().is_empty() => k.to_string(),
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            let patch = match (
                opt_str(&req.params, "kind"),
                opt_str(&req.params, "anchor"),
                opt_str(&req.params, "x"),
                opt_str(&req.params, "y"),
                opt_str(&req.params, "width"),
                opt_str(&req.params, "height"),
                opt_f32_array::<4>(&req.params, "color"),
                opt_i32(&req.params, "layer"),
                opt_bool(&req.params, "visible"),
                opt_f32(&req.params, "border_width"),
                opt_str(&req.params, "image"),
            ) {
                (
                    Ok(kind),
                    Ok(anchor),
                    Ok(x),
                    Ok(y),
                    Ok(width),
                    Ok(height),
                    Ok(color),
                    Ok(layer),
                    Ok(visible),
                    Ok(border_width),
                    Ok(image),
                ) => UiPatch {
                    key: key.clone(),
                    kind: kind.map(str::to_string),
                    anchor: anchor.map(str::to_string),
                    x: x.map(str::to_string),
                    y: y.map(str::to_string),
                    width: width.map(str::to_string),
                    height: height.map(str::to_string),
                    color,
                    layer,
                    visible,
                    border_width,
                    image: image.map(str::to_string),
                },
                (Err(f), ..)
                | (_, Err(f), ..)
                | (_, _, Err(f), ..)
                | (_, _, _, Err(f), ..)
                | (_, _, _, _, Err(f), ..)
                | (_, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, Err(f), ..)
                | (_, _, _, _, _, _, _, _, _, Err(f), _)
                | (_, _, _, _, _, _, _, _, _, _, Err(f)) => return Response::fail(id, f),
            };
            match api.ui_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.remove" => {
            let key = match require_str(&req.params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.ui_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "ui.state" => match api.ui_state() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.save" => {
            let path = match param(&req.params, "path") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `path` 应为非空字符串（可省略）"),
                    )
                }
            };
            match api.scene_save(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 局部光源契约（扩展十五）：字段可分别给，一个都不给视为无意义调用；
        // kind 的合法性、数值收敛全部由宿主完成，本层只做类型与空值校验。
        "light.list" => {
            let limit = match opt_limit(&req.params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.light_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.set" => {
            let patch = match light_patch_from(&req.params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            if patch.is_key_only() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "除 `key` 外至少要给一个字段（否则这次调用没有意义）",
                    ),
                );
            }
            match api.light_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.remove" => {
            let key = match require_str(&req.params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.light_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "light.clear" => match api.light_clear() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "light.setenabled" => {
            let enabled = match param(&req.params, "enabled") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `enabled` 应为布尔（可省略表示只读）"),
                    )
                }
            };
            match api.light_set_enabled(enabled) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 3D 空间化音频契约（§12 第十项）：与 light.* 同构——传输层只校验
        // 类型与空值，距离收敛 / 模型解析 / 上限拒绝全在宿主侧。
        "audio.spatial.list" => {
            let limit = match opt_limit(&req.params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_list(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.set" => {
            let patch = match audio_spatial_patch_from(&req.params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            if patch.is_key_only() {
                return Response::fail(
                    id,
                    RpcFailure::invalid_params(
                        "除 `key` 外至少要给一个字段（否则这次调用没有意义）",
                    ),
                );
            }
            match api.audio_spatial_set(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.remove" => {
            let key = match require_str(&req.params, "key") {
                Ok(k) if !k.trim().is_empty() => k,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `key` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_remove(key) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.clear" => match api.audio_spatial_clear() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "audio.spatial.listener" => {
            let patch = match listener_patch_from(&req.params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            match api.audio_spatial_listener(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "audio.spatial.setenabled" => {
            let enabled = match param(&req.params, "enabled") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(b)) => Some(*b),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `enabled` 应为布尔（可省略表示只读）"),
                    )
                }
            };
            match api.audio_spatial_set_enabled(enabled) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.shadow" => {
            let patch = match shadow_patch_from(&req.params) {
                Ok(p) => p,
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_shadow(patch) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        // 场景栈与存档契约（扩展十八 B）：只做参数校验，压栈 / 换资产库 / 重跑脚本
        // 的顺序全在宿主侧——传输层不认识 Scene，也不该知道"换场景要先换资产"。
        "scene.stack" => {
            let limit = match opt_limit(&req.params) {
                Ok(l) => l,
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_stack(limit) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.switch" => {
            let path = match require_str(&req.params, "path") {
                Ok(s) if !s.trim().is_empty() => s,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `path` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_switch(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.back" => match api.scene_back() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        "scene.archive.save" => {
            let path = match param(&req.params, "path") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
                Some(_) => {
                    return Response::fail(
                        id,
                        RpcFailure::invalid_params("参数 `path` 应为非空字符串（可省略）"),
                    )
                }
            };
            match api.scene_archive_save(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "scene.archive.load" => {
            let path = match require_str(&req.params, "path") {
                Ok(s) if !s.trim().is_empty() => s,
                Ok(_) => {
                    return Response::fail(id, RpcFailure::invalid_params("参数 `path` 不能为空"))
                }
                Err(f) => return Response::fail(id, f),
            };
            match api.scene_archive_load(path) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "script.eval" => {
            let source = match require_str(&req.params, "source")
                .or_else(|_| require_str(&req.params, "code"))
            {
                Ok(s) => s,
                Err(f) => return Response::fail(id, f),
            };
            match api.script_eval(source) {
                Ok(r) => Response::ok(id, r),
                Err(f) => Response::fail(id, f),
            }
        }
        "script.reload" => match api.script_reload() {
            Ok(r) => Response::ok(id, r),
            Err(f) => Response::fail(id, f),
        },
        other => Response::fail(
            id,
            RpcFailure::new(METHOD_NOT_FOUND, format!("未知方法 `{}`", req.method))
                .with_data(json!({ "normalized": other })),
        ),
    }
}

/// 取出 `node` 参数并交给闭包，统一参数缺失的错误。
fn with_node<T>(
    params: &Value,
    f: impl FnOnce(&str) -> Result<T, RpcFailure>,
) -> Result<T, RpcFailure> {
    let node = require_str(params, "node")?;
    if node.trim().is_empty() {
        return Err(RpcFailure::invalid_params("参数 `node` 不能为空"));
    }
    f(node)
}

/// 从 RPC 参数解析局部光源补丁（全部字段可选，没给的留给宿主沿用原值）。
fn light_patch_from(params: &Value) -> Result<LightPatch, RpcFailure> {
    let key = match require_str(params, "key") {
        Ok(k) if !k.trim().is_empty() => k.to_string(),
        Ok(_) => return Err(RpcFailure::invalid_params("参数 `key` 不能为空")),
        Err(f) => return Err(f),
    };
    Ok(LightPatch {
        key,
        kind: opt_str(params, "kind")?.map(str::to_string),
        pos: opt_vec3(params, "pos")?,
        dir: opt_vec3(params, "dir")?,
        color: opt_vec3(params, "color")?,
        intensity: opt_num(params, "intensity")?,
        range: opt_num(params, "range")?,
        inner_deg: opt_num(params, "inner_deg")?,
        outer_deg: opt_num(params, "outer_deg")?,
        cast_shadow: opt_bool(params, "cast_shadow")?,
        shadow_bias: opt_num(params, "shadow_bias")?,
    })
}

/// 从 RPC 参数解析空间化声源补丁（`key` 必填，其余可选）。
fn audio_spatial_patch_from(params: &Value) -> Result<AudioSpatialPatch, RpcFailure> {
    let key = match require_str(params, "key") {
        Ok(k) if !k.trim().is_empty() => k.to_string(),
        Ok(_) => return Err(RpcFailure::invalid_params("参数 `key` 不能为空")),
        Err(f) => return Err(f),
    };
    Ok(AudioSpatialPatch {
        key,
        position: opt_vec3(params, "position")?,
        node: opt_str(params, "node")?.map(str::to_string),
        min_distance: opt_num(params, "minDistance")?,
        max_distance: opt_num(params, "maxDistance")?,
        model: opt_str(params, "model")?.map(str::to_string),
        doppler: opt_num(params, "doppler")?,
    })
}

/// 从 RPC 参数解析听者补丁：两个字段都没有 = `None`（纯只读）。
fn listener_patch_from(params: &Value) -> Result<Option<ListenerPatch>, RpcFailure> {
    let patch = ListenerPatch {
        position: opt_vec3(params, "position")?,
        forward: opt_vec3(params, "forward")?,
    };
    Ok(if patch.is_read_only() {
        None
    } else {
        Some(patch)
    })
}

/// 从 RPC 参数解析阴影补丁：一个字段都没有 = `None`（纯只读）。
fn shadow_patch_from(params: &Value) -> Result<Option<ShadowPatch>, RpcFailure> {
    let patch = ShadowPatch {
        enabled: match param(params, "enabled") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(RpcFailure::invalid_params("参数 `enabled` 应为布尔")),
        },
        size: opt_uint(params, "size")?,
        strength: opt_num(params, "strength")?,
        ortho_size: opt_num(params, "orthoSize")?,
        bias: opt_num(params, "bias")?,
        cascades: opt_uint(params, "cascades")?,
        blend: opt_num(params, "blend")?,
        distance: opt_num(params, "distance")?,
    };
    Ok(if patch.is_read_only() {
        None
    } else {
        Some(patch)
    })
}

/// 可选无符号整数字段（`size` / `cascades`）；显式 `null` 当作没给。
fn opt_uint(params: &Value, key: &str) -> Result<Option<u32>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .map(Some)
            .ok_or_else(|| RpcFailure::invalid_params(format!("参数 `{key}` 应为非负整数"))),
        Some(_) => Err(RpcFailure::invalid_params(format!(
            "参数 `{key}` 应为非负整数"
        ))),
    }
}

/// 显式 `null` 等同于「没给」——不少 JSON 客户端会把没填的字段序列化成 `null`，
/// 对这种输入报「类型错误」纯属刁难。真正的类型错误（字符串 / 布尔 / 短数组）照旧拒绝。
fn opt_num(params: &Value, key: &str) -> Result<Option<f32>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => opt_f32(params, key),
    }
}

/// 同 [`opt_num`]，用于三元数组字段。
fn opt_vec3(params: &Value, key: &str) -> Result<Option<[f32; 3]>, RpcFailure> {
    match param(params, key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => opt_f32_array::<3>(params, key),
    }
}

/// 把 `limit` 参数解析成 `Option<usize>`（缺省 / `null` 表示不截断）。
fn opt_limit(params: &Value) -> Result<Option<usize>, RpcFailure> {
    match param(params, "limit") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Some(n as usize)),
            None => Err(RpcFailure::invalid_params("参数 `limit` 应为非负整数")),
        },
    }
}

#[cfg(test)]
mod save_rpc_tests {
    use super::*;

    /// 只关心 `scene.save` 的桩：记录收到的路径，回报固定应答。
    #[derive(Default)]
    struct SaveStub {
        seen: Option<Option<String>>,
    }

    impl EngineApi for SaveStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn scene_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
            self.seen = Some(path.map(str::to_string));
            Ok(json!({ "path": path.unwrap_or("<default>"), "objects": 0 }))
        }
    }

    /// 什么都没实现的宿主：验证 trait 缺省实现拒绝 `scene.save`。
    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn scene_save_passes_path_through() {
        let mut api = SaveStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.save", json!({ "path": "out/scene.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(
            api.seen,
            Some(Some("out/scene.json".to_string())),
            "路径应原样传给宿主"
        );
        assert_eq!(res.result.unwrap()["path"], json!("out/scene.json"));
    }

    #[test]
    fn scene_save_defaults_to_none_when_path_absent() {
        let mut api = SaveStub::default();
        // 方法名大小写不敏感：`Scene.Save` 与 `scene.save` 等价
        let res = dispatch(&mut api, &request("Scene.Save", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.seen, Some(None), "省略 path 时应交给宿主决定默认路径");
    }

    #[test]
    fn scene_save_rejects_blank_path() {
        let mut api = SaveStub::default();
        let res = dispatch(&mut api, &request("scene.save", json!({ "path": "   " })));
        let err = res.error.expect("空路径应报错");
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(api.seen, None, "参数非法时不应调用宿主");
    }

    #[test]
    fn scene_save_unsupported_by_default() {
        let mut api = BareStub;
        let res = dispatch(&mut api, &request("scene.save", json!({})));
        let err = res.error.expect("缺省实现应报未实现");
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("scene.save"), "错误消息应点名方法");
    }
}

#[cfg(test)]
mod scene_stack_rpc_tests {
    use super::*;

    /// 记录收到的参数、回报固定应答的桩宿主。
    #[derive(Default)]
    struct StackStub {
        stack_limit: Option<Option<usize>>,
        switched: Option<String>,
        back_calls: usize,
        archived: Option<Option<String>>,
        loaded: Option<String>,
    }

    impl EngineApi for StackStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn scene_stack(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
            self.stack_limit = Some(limit);
            Ok(json!({
                "count": 1,
                "current": 0,
                "max": 16, // 与 r3e-core 的 MAX_SCENE_STACK 对齐；传输层不依赖 core，故写字面量
                "entries": [{ "index": 0, "name": "main", "path": "main.json", "objects": 2 }],
            }))
        }

        fn scene_switch(&mut self, path: &str) -> Result<Value, RpcFailure> {
            self.switched = Some(path.to_string());
            Ok(json!({ "path": path, "depth": 2 }))
        }

        fn scene_back(&mut self) -> Result<Value, RpcFailure> {
            self.back_calls += 1;
            Ok(json!({ "name": "main", "depth": 1 }))
        }

        fn scene_archive_save(&mut self, path: Option<&str>) -> Result<Value, RpcFailure> {
            self.archived = Some(path.map(str::to_string));
            Ok(json!({ "path": path.unwrap_or("<default>"), "objects": 0 }))
        }

        fn scene_archive_load(&mut self, path: &str) -> Result<Value, RpcFailure> {
            self.loaded = Some(path.to_string());
            Ok(json!({ "path": path, "objects": 0, "anims": 0 }))
        }
    }

    /// 什么都没实现的宿主：验证缺省实现一律拒绝。
    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn scene_stack_passes_limit_through() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.stack", json!({ "limit": 3 })));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.stack_limit, Some(Some(3)), "limit 应原样传给宿主");
        assert_eq!(res.result.unwrap()["count"], json!(1));
    }

    #[test]
    fn scene_stack_defaults_to_no_limit() {
        let mut api = StackStub::default();
        // 归一化：`Scene.Stack` / `scene_stack` 都落到同一条分支
        let res = dispatch(&mut api, &request("Scene.Stack", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.stack_limit, Some(None));
    }

    #[test]
    fn scene_switch_requires_non_empty_path() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.switch", json!({ "path": "  " })));
        let err = res.error.expect("空路径应报错");
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(api.switched, None, "参数非法时不应调用宿主");

        let res = dispatch(&mut api, &request("scene.switch", json!({})));
        assert_eq!(res.error.expect("缺 path 应报错").code, INVALID_PARAMS);

        let res = dispatch(
            &mut api,
            &request("scene.switch", json!({ "path": "lv2.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.switched.as_deref(), Some("lv2.json"));
    }

    #[test]
    fn scene_back_needs_no_params() {
        let mut api = StackStub::default();
        let res = dispatch(&mut api, &request("scene.back", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.back_calls, 1);
    }

    #[test]
    fn scene_archive_save_path_semantics() {
        let mut api = StackStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.archive.save", json!({ "path": "slots/a.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.archived, Some(Some("slots/a.json".to_string())));

        let res = dispatch(&mut api, &request("scene.archive.save", json!({})));
        assert!(
            res.error.is_none(),
            "省略 path 应交给宿主决定: {:?}",
            res.error
        );
        assert_eq!(api.archived, Some(None));

        let res = dispatch(
            &mut api,
            &request("scene.archive.save", json!({ "path": "   " })),
        );
        assert_eq!(res.error.expect("空白路径应报错").code, INVALID_PARAMS);
    }

    #[test]
    fn scene_archive_load_requires_path() {
        let mut api = StackStub::default();
        let res = dispatch(
            &mut api,
            &request("scene.archive.load", json!({ "path": "slots/a.json" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.loaded.as_deref(), Some("slots/a.json"));

        let res = dispatch(&mut api, &request("scene.archive.load", json!({})));
        assert_eq!(res.error.expect("缺 path 应报错").code, INVALID_PARAMS);
        assert_eq!(
            api.loaded.as_deref(),
            Some("slots/a.json"),
            "参数非法时不改动"
        );
    }

    #[test]
    fn scene_stack_methods_unsupported_by_default() {
        for method in [
            "scene.stack",
            "scene.switch",
            "scene.back",
            "scene.archive.save",
            "scene.archive.load",
        ] {
            let mut api = BareStub;
            let res = dispatch(&mut api, &request(method, json!({ "path": "x.json" })));
            let err = res.error.expect("缺省实现应报未实现");
            assert_eq!(err.code, METHOD_NOT_FOUND, "{method}");
        }
    }
}

#[cfg(test)]
mod audio_spatial_rpc_tests {
    use super::*;

    /// 只关心空间化音频的桩：记下收到的补丁，回报最小应答。
    #[derive(Default)]
    struct SpatialStub {
        set: Option<AudioSpatialPatch>,
        removed: Option<String>,
        cleared: u32,
        listener: Option<Option<ListenerPatch>>,
        enabled: Option<Option<bool>>,
        limit: Option<Option<usize>>,
    }

    impl EngineApi for SpatialStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }

        fn audio_spatial_list(&mut self, limit: Option<usize>) -> Result<Value, RpcFailure> {
            self.limit = Some(limit);
            Ok(json!({ "count": 1, "sources": [{ "key": "bell" }] }))
        }

        fn audio_spatial_set(&mut self, patch: AudioSpatialPatch) -> Result<Value, RpcFailure> {
            let key = patch.key.clone();
            self.set = Some(patch);
            Ok(json!({ "key": key }))
        }

        fn audio_spatial_remove(&mut self, key: &str) -> Result<Value, RpcFailure> {
            self.removed = Some(key.to_string());
            Ok(json!({ "removed": true }))
        }

        fn audio_spatial_clear(&mut self) -> Result<Value, RpcFailure> {
            self.cleared += 1;
            Ok(json!({ "cleared": 2 }))
        }

        fn audio_spatial_listener(
            &mut self,
            patch: Option<ListenerPatch>,
        ) -> Result<Value, RpcFailure> {
            self.listener = Some(patch);
            Ok(json!({ "position": [0.0, 0.0, 0.0] }))
        }

        fn audio_spatial_set_enabled(
            &mut self,
            enabled: Option<bool>,
        ) -> Result<Value, RpcFailure> {
            self.enabled = Some(enabled);
            Ok(json!({ "enabled": enabled.unwrap_or(true) }))
        }
    }

    struct BareStub;

    impl EngineApi for BareStub {
        fn info(&mut self) -> EngineInfo {
            unimplemented!("本测试不查 info")
        }
    }

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: JSONRPC.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn spatial_set_passes_fields_through() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.set",
                json!({
                    "key": "bell",
                    "position": [1.0, 2.0, 3.0],
                    "node": "bell_mesh",
                    "minDistance": 2.0,
                    "maxDistance": 40.0,
                    "model": "INVERSE",
                    "doppler": 0.0,
                }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.set.expect("宿主应收到补丁");
        assert_eq!(patch.key, "bell");
        assert_eq!(patch.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(patch.node.as_deref(), Some("bell_mesh"));
        assert_eq!(patch.min_distance, Some(2.0));
        assert_eq!(patch.max_distance, Some(40.0));
        // 大小写归一化交给宿主，传输层原样转交
        assert_eq!(patch.model.as_deref(), Some("INVERSE"));
        assert_eq!(patch.doppler, Some(0.0));
    }

    #[test]
    fn spatial_set_requires_more_than_key() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request("audio.spatial.set", json!({ "key": "bell" })),
        );
        assert_eq!(
            res.error.expect("只给 key 应报错").code,
            INVALID_PARAMS,
            "无字段调用没有意义"
        );
        assert!(api.set.is_none(), "参数非法时不应调用宿主");
    }

    #[test]
    fn spatial_set_rejects_bad_types() {
        let mut api = SpatialStub::default();
        for params in [
            json!({ "key": "  ", "position": [0.0, 0.0, 0.0] }),
            json!({ "key": "bell", "minDistance": "近" }),
            json!({ "key": "bell", "position": [1.0, 2.0] }),
            json!({ "key": "bell", "node": 7 }),
        ] {
            let res = dispatch(&mut api, &request("audio.spatial.set", params.clone()));
            assert_eq!(
                res.error.unwrap_or_else(|| panic!("{params} 应报错")).code,
                INVALID_PARAMS
            );
        }
        assert!(api.set.is_none(), "全部非法，宿主不该被调用");
    }

    #[test]
    fn spatial_methods_are_name_normalized() {
        let mut api = SpatialStub::default();
        // 大小写不敏感（`canonical` 归一化）；点分是方法名的一部分，不能省
        for method in [
            "Audio.Spatial.List",
            "AUDIO.SPATIAL.LIST",
            "audio.spatial.list",
        ] {
            let res = dispatch(&mut api, &request(method, json!({ "limit": 5 })));
            assert!(res.error.is_none(), "{method} 不应报错: {:?}", res.error);
            assert_eq!(api.limit, Some(Some(5)), "{method}");
        }
    }

    #[test]
    fn spatial_remove_requires_non_empty_key() {
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request("audio.spatial.remove", json!({ "key": "   " })),
        );
        assert_eq!(res.error.expect("空白 key 应报错").code, INVALID_PARAMS);
        assert!(api.removed.is_none());

        let res = dispatch(
            &mut api,
            &request("audio.spatial.remove", json!({ "key": "bell" })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.removed.as_deref(), Some("bell"));
    }

    #[test]
    fn spatial_clear_needs_no_params() {
        let mut api = SpatialStub::default();
        let res = dispatch(&mut api, &request("audio.spatial.clear", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.cleared, 1);
        assert_eq!(res.result.unwrap()["cleared"], json!(2));
    }

    #[test]
    fn listener_patch_read_only_vs_write() {
        let mut api = SpatialStub::default();
        // 空参数 = 只读：宿主收到 None
        let res = dispatch(&mut api, &request("audio.spatial.listener", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.listener, Some(None), "空参数应走只读");

        // 只给 position
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.listener",
                json!({ "position": [4.0, 1.0, 0.0] }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.listener.flatten().expect("应转交补丁");
        assert_eq!(patch.position, Some([4.0, 1.0, 0.0]));
        assert_eq!(patch.forward, None, "没给的字段不填默认值");

        // 只给 forward
        let mut api = SpatialStub::default();
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.listener",
                json!({ "forward": [0.0, 0.0, -1.0] }),
            ),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        let patch = api.listener.flatten().expect("应转交补丁");
        assert_eq!(patch.forward, Some([0.0, 0.0, -1.0]));
        assert_eq!(patch.position, None);
    }

    #[test]
    fn enabled_is_readable_and_writable() {
        let mut api = SpatialStub::default();
        let res = dispatch(&mut api, &request("audio.spatial.setenabled", json!({})));
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.enabled, Some(None), "省略 enabled 应为只读");

        let res = dispatch(
            &mut api,
            &request("audio.spatial.setenabled", json!({ "enabled": false })),
        );
        assert!(res.error.is_none(), "不应报错: {:?}", res.error);
        assert_eq!(api.enabled, Some(Some(false)));

        let res = dispatch(
            &mut api,
            &request("audio.spatial.setenabled", json!({ "enabled": "yes" })),
        );
        assert_eq!(res.error.expect("非布尔应报错").code, INVALID_PARAMS);
    }

    #[test]
    fn spatial_methods_unsupported_by_default() {
        for method in [
            "audio.spatial.list",
            "audio.spatial.remove",
            "audio.spatial.clear",
            "audio.spatial.listener",
            "audio.spatial.setenabled",
        ] {
            let mut api = BareStub;
            let res = dispatch(&mut api, &request(method, json!({ "key": "bell" })));
            assert_eq!(
                res.error
                    .unwrap_or_else(|| panic!("{method} 应报未实现"))
                    .code,
                METHOD_NOT_FOUND,
                "{method}"
            );
        }
        let mut api = BareStub;
        let res = dispatch(
            &mut api,
            &request(
                "audio.spatial.set",
                json!({ "key": "bell", "doppler": 1.0 }),
            ),
        );
        assert_eq!(res.error.expect("set 也应报未实现").code, METHOD_NOT_FOUND);
    }
}
