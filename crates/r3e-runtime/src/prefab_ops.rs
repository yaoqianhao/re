//! `prefab.*` 六条的结构化实现（M2-E04）。
//!
//! 与 `nav_ops` / `terrain_ops` 同口径：**只做语义，不碰协议 JSON**。协议层
//! （`r3e-protocol`）负责参数形状，脚本层（`r3e-script`）负责 rhai 类型映射，
//! 本模块被两者共用，返回结构化结果与自有错误枚举。
//!
//! # 实例化只走 [`Scene::spawn_instance`]
//!
//! 六条命令里唯一会造节点的是 `spawn` / `spawn_many`，两者都调
//! [`Scene::spawn_instance`]（内部即 `Scene::spawn`）。本模块**不**自己拼
//! `ObjectDesc` 再直接 `spawn`——那样就绕过了 `resolve_overrides` 的三态语义与
//! 模板句柄一致性检查，出现「模板改了、覆盖没叠、节点却造出来了」的静默错误。
//!
//! # 覆盖写入之后为什么要同步节点
//!
//! `set_override` / `clear_override` 改的是**实例记录**（`instances` 的真源）。
//! 但节点已经被造出来了，覆盖表改了不重算，场景里那个物件就跟记载不一致。
//! 所以两条命令在写完表之后都走一遍
//! 「`resolve_overrides` → 把算出来的 `ObjectDesc` 套回节点」：
//!
//! * 套的字段与 [`Scene::spawn`] 同源（同样的 `MeshRef::parse` /
//!   `normalize_material_ref` / `normalize_tags` / `normalize_interact_radius` 收敛），
//!   不在本模块另立一套；
//! * **`parent` 例外**：改父级要动 `children` 向量与 `order`，属结构变更，本次
//!   只记进覆盖表、在下次实例化（重载场景）时生效。这个裁剪是**有意的**——
//!   为了一个字段把节点删了重建，会连带丢掉 `NodeId` 与挂在节点上的其它状态
//!   （角色绑定、物理句柄），代价远大于收益。

use std::sync::Arc;

use r3e_core::prefab::{
    DefineOutcome, FieldOverride, ObjectOverride, OverrideState, PrefabInstance, PrefabTableError,
    PrefabTemplate,
};
use r3e_core::Scene;
use r3e_core::ERR_UNKNOWN_TEMPLATE;

/// `prefab_spawn_many` 单次生成上限，与核心侧 `SPAWN_PREFAB_MAX_NODES` 对齐。
pub const PREFAB_SPAWN_MANY_LIMIT: i64 = 256;

/// prefab 操作的失败原因。
///
/// 只分两档，因为命令面只认这两档：
///
/// * [`PrefabOpError::Invalid`]：参数形状不对（路径读不出、字段名不认识、数值越界）
///   ——调用方改参数；
/// * [`PrefabOpError::Conflict`]：参数没毛病，但**当前状态**不答应（模板没登记、
///   实例不存在、名字撞了）——调用方得先改场景。
///
/// 两个合并成一个 `String` 会迫使脚本层用字符串 contains 猜分支，这条链路已经在
/// `Scene::resolve_overrides` 里出现过一次（`ERR_UNKNOWN_TEMPLATE` 前缀），
/// 这里不再重演。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefabOpError {
    /// 参数非法。
    Invalid(String),
    /// 状态冲突（模板未注册 / 实例不存在 / 名字已存在）。
    Conflict(String),
}

impl std::fmt::Display for PrefabOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(msg) | Self::Conflict(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PrefabOpError {}

/// `prefab_define` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefineResult {
    /// 实际登记的模板 ID（取自模板文件自身，或与调用方给的一致）。
    pub id: String,
    /// 是否替换了同 ID 的旧模板。
    pub replaced: bool,
    /// 模板文件路径（回显用）。
    pub path: String,
    /// 原型名字（`proto.name`）；实例化时会被实例名替换。
    pub proto_name: String,
    /// 模板标签条数。
    pub tags: usize,
}

/// `prefab_spawn` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnResult {
    /// 实例名（即场景节点名）。
    pub name: String,
    /// 模板 ID。
    pub template: String,
    /// 父节点名；`None` = 挂场景根。
    pub parent: Option<String>,
    /// 新节点的 `NodeId`。
    pub node: u64,
}

/// `prefab_info` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInfo {
    /// 实例名。
    pub name: String,
    /// 模板 ID。
    pub template: String,
    /// 父节点名。
    pub parent: Option<String>,
    /// 实例对应的场景节点名（节点已不在时为 `None`）。
    pub node: Option<String>,
    /// 逐字段覆盖态（只含 `set` / `clear`，继承态不出现在这里）。
    pub overrides: Vec<(String, &'static str)>,
    /// 覆盖字段名清单（与 `overrides` 的字段顺序一致）。
    pub fields: Vec<String>,
    /// 模板是否还在会话模板表里。
    pub template_registered: bool,
}

/// 从模板文件登记模板（`prefab_define`）。
///
/// `id` 传空串表示「以模板文件里写的 `id` 为准」；非空则要求两者一致，
/// 不一致按参数非法拒——放行会让「调用方以为登记在 A 名下、实际在 B 名下」
/// 这种事静默发生，之后 `spawn("A")` 报未注册时现场已无迹可循。
pub fn define(scene: &mut Scene, id: &str, path: &str) -> Result<DefineResult, PrefabOpError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(PrefabOpError::Invalid("模板路径不能为空".to_string()));
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|err| PrefabOpError::Invalid(format!("读不到模板文件 `{path}`：{err}")))?;
    let template: PrefabTemplate = serde_json::from_str(&raw)
        .map_err(|err| PrefabOpError::Invalid(format!("模板文件 `{path}` 解析失败：{err}")))?;
    let want = id.trim();
    if !want.is_empty() && want != template.id {
        return Err(PrefabOpError::Invalid(format!(
            "模板文件里的 id 是 `{}`，与请求的 `{want}` 不一致",
            template.id
        )));
    }
    let registered = template.id.clone();
    let proto_name = template.proto.name.clone();
    let tags = template.tags.len();
    let outcome = scene.define_prefab(template).map_err(|err| match err {
        // ID 形状 / 空原型名 / 表满：都是「这份文件本身不合格」，不是状态冲突
        PrefabTableError::Id(inner) => PrefabOpError::Invalid(format!("模板 ID 非法：{inner}")),
        other => PrefabOpError::Invalid(other.to_string()),
    })?;
    Ok(DefineResult {
        id: registered,
        replaced: matches!(outcome, DefineOutcome::Replaced),
        path: path.to_string(),
        proto_name,
        tags,
    })
}

/// 单实例化（`prefab_spawn`）。`parent` 为 `None` 时挂场景根。
pub fn spawn(
    scene: &mut Scene,
    id: &str,
    name: &str,
    parent: Option<&str>,
) -> Result<SpawnResult, PrefabOpError> {
    let id = id.trim();
    let name = name.trim();
    if id.is_empty() {
        return Err(PrefabOpError::Invalid("模板 ID 不能为空".to_string()));
    }
    if name.is_empty() {
        return Err(PrefabOpError::Invalid("实例名不能为空".to_string()));
    }
    if scene.prefab_handle(id).is_none() {
        return Err(PrefabOpError::Conflict(format!(
            "{ERR_UNKNOWN_TEMPLATE}：`{id}` 还没登记（先 prefab_define）"
        )));
    }
    let parent = parent.map(str::trim).filter(|value| !value.is_empty());
    let mut instance = PrefabInstance::new(name, id);
    instance.parent = parent.map(str::to_string);
    let node = scene
        .spawn_instance(instance)
        .map_err(|err| classify_spawn_error(err, name))?;
    Ok(SpawnResult {
        name: name.to_string(),
        template: id.to_string(),
        parent: parent.map(str::to_string),
        node,
    })
}

/// 批量实例化（`prefab_spawn_many`）：按 `<prefix><序号>` 命名，序号从 1 起。
///
/// 返回实际生成的条数。中途失败会**回滚已建的节点**：留一半实例在场景里，
/// 调用方拿到的既不是成功也不是干净失败，比整体失败更难收拾。
pub fn spawn_many(
    scene: &mut Scene,
    id: &str,
    prefix: &str,
    count: i64,
) -> Result<usize, PrefabOpError> {
    let prefix = prefix.trim();
    if !(1..=PREFAB_SPAWN_MANY_LIMIT).contains(&count) {
        return Err(PrefabOpError::Invalid(format!(
            "count 必须在 1..={PREFAB_SPAWN_MANY_LIMIT} 之间（收到 {count}），不静默截断"
        )));
    }
    if prefix.is_empty() {
        return Err(PrefabOpError::Invalid("prefix 不能为空".to_string()));
    }
    if scene.prefab_handle(id.trim()).is_none() {
        return Err(PrefabOpError::Conflict(format!(
            "{ERR_UNKNOWN_TEMPLATE}：`{}` 还没登记（先 prefab_define）",
            id.trim()
        )));
    }
    let mut created: Vec<String> = Vec::with_capacity(count as usize);
    for index in 1..=count {
        let name = format!("{prefix}{index}");
        match spawn(scene, id, &name, None) {
            Ok(_) => created.push(name),
            Err(err) => {
                for done in created.iter().rev() {
                    scene.despawn(done);
                }
                return Err(err);
            }
        }
    }
    Ok(created.len())
}

/// 写一个覆盖字段（`prefab_set_override`）。
///
/// `value` 的形状按字段类型解析（`[f32;3]` / `bool` / 材质对象…），解析失败即
/// 参数非法，不进表——半条非法覆盖会一路带到 `resolve_overrides` 才炸。
pub fn set_override(
    scene: &mut Scene,
    name: &str,
    field: &str,
    value: &serde_json::Value,
) -> Result<bool, PrefabOpError> {
    if !ObjectOverride::FIELD_NAMES.contains(&field) {
        return Err(PrefabOpError::Invalid(format!(
            "字段 `{field}` 不在可覆盖集 {:?} 里",
            ObjectOverride::FIELD_NAMES
        )));
    }
    let target = scene
        .instance_mut(name)
        .ok_or_else(|| PrefabOpError::Conflict(format!("实例 `{name}` 不存在")))?;
    store_field(&mut target.overrides, field, value)?;
    scene.sync_instance_node(name);
    Ok(true)
}

/// 把一个字段退回「继承模板」（`prefab_clear_override`）。
///
/// 实现走「序列化 → 删键 → 反序列化」：12 个字段类型各不相同，写 12 个
/// `&mut Option<FieldOverride<T>>` 分支无法汇成一个返回类型，只能靠
/// `ObjectOverride` 自己的反序列化（`deny_unknown_fields` + 逐字段三态）兜底。
pub fn clear_override(scene: &mut Scene, name: &str, field: &str) -> Result<bool, PrefabOpError> {
    if !ObjectOverride::FIELD_NAMES.contains(&field) {
        return Err(PrefabOpError::Invalid(format!(
            "字段 `{field}` 不在可覆盖集 {:?} 里",
            ObjectOverride::FIELD_NAMES
        )));
    }
    let target = scene
        .instance_mut(name)
        .ok_or_else(|| PrefabOpError::Conflict(format!("实例 `{name}` 不存在")))?;
    let mut json = serde_json::to_value(&target.overrides)
        .map_err(|err| PrefabOpError::Invalid(format!("覆盖表无法序列化：{err}")))?;
    let changed = match json.as_object_mut() {
        Some(map) => map.remove(field).is_some(),
        None => {
            return Err(PrefabOpError::Invalid(
                "覆盖表不是 JSON 对象，拒绝继续".to_string(),
            ))
        }
    };
    target.overrides = serde_json::from_value(json)
        .map_err(|err| PrefabOpError::Invalid(format!("覆盖表回读失败：{err}")))?;
    scene.sync_instance_node(name);
    Ok(changed)
}

/// 实例信息（`prefab_info`）：模板 ID + 逐字段覆盖态。
pub fn info(scene: &Scene, name: &str) -> Result<InstanceInfo, PrefabOpError> {
    let instance = scene
        .instance(name)
        .ok_or_else(|| PrefabOpError::Conflict(format!("实例 `{name}` 不是 prefab 实例")))?;
    let mut overrides = Vec::new();
    for field in ObjectOverride::FIELD_NAMES {
        match instance.overrides.field_state(field) {
            Some(OverrideState::Inherit) | None => {}
            Some(state) => overrides.push(((*field).to_string(), state.as_str())),
        }
    }
    let fields = overrides.iter().map(|(f, _)| f.clone()).collect();
    Ok(InstanceInfo {
        name: instance.name.clone(),
        template: instance.template.clone(),
        parent: instance.parent.clone(),
        node: scene.find(name).map(|node| node.name.clone()),
        overrides,
        fields,
        template_registered: scene.prefab_handle(&instance.template).is_some(),
    })
}

// ---------- 内部 ----------

/// 把 `spawn_instance` 的错误串分流成 [`PrefabOpError`]。
///
/// 只认三种「状态」说法（模板未注册 / 名字已存在 / 父物件不存在），其余按参数非法：
/// 底线是**不要把它们都变成 Invalid**——那样调用方会一直改参数，而不是先去
/// `prefab_define` 或换个名字。
fn classify_spawn_error(err: String, name: &str) -> PrefabOpError {
    if err.contains(ERR_UNKNOWN_TEMPLATE) {
        return PrefabOpError::Conflict(err);
    }
    if err.contains("已存在") || err.contains("不存在") {
        return PrefabOpError::Conflict(format!("{err}（实例名 `{name}`）"));
    }
    PrefabOpError::Invalid(err)
}

/// 按字段类型解析并写入覆盖表。
fn store_field(
    overrides: &mut ObjectOverride,
    field: &str,
    value: &serde_json::Value,
) -> Result<(), PrefabOpError> {
    macro_rules! store {
        ($slot:ident, $ty:ty) => {{
            let parsed: $ty = serde_json::from_value(value.clone()).map_err(|err| {
                PrefabOpError::Invalid(format!("字段 `{field}` 的取值形状不对：{err}"))
            })?;
            overrides.$slot = Some(FieldOverride::Set(parsed));
        }};
    }
    match field {
        "mesh" => store!(mesh, String),
        "position" => store!(position, [f32; 3]),
        "rotation" => store!(rotation, [f32; 3]),
        "scale" => store!(scale, [f32; 3]),
        "color" => store!(color, [f32; 4]),
        "parent" => store!(parent, String),
        "visible" => store!(visible, bool),
        "physics" => store!(physics, r3e_core::PhysicsDesc),
        "material" => store!(material, r3e_core::SurfaceMaterial),
        "materialRef" => store!(material_ref, String),
        "tags" => store!(tags, Vec<String>),
        "interact_radius" => store!(interact_radius, f32),
        other => {
            return Err(PrefabOpError::Invalid(format!(
                "字段 `{other}` 不在可覆盖集里"
            )))
        }
    }
    Ok(())
}

/// 模板句柄（脚本侧建 [`PrefabInstance`] 时要确认模板在不在，顺手暴露）。
pub fn template_handle(scene: &Scene, id: &str) -> Option<Arc<PrefabTemplate>> {
    scene.prefab_handle(id)
}
