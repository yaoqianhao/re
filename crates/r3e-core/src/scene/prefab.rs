//! Prefab 实例化入口（M2-E02）。
//!
//! # 本模块只做三件事
//!
//! 1. **回表**：实例名 → 模板句柄（`prefab_handles`）→ 模板原型；
//! 2. **算实体**：[`crate::prefab::apply`] 把「模板原型 + 实例覆盖 + 实例名」叠成
//!    一条 [`crate::ObjectDesc`]；
//! 3. **装配**：把这条 `ObjectDesc` 交给 [`Scene::spawn`]。
//!
//! # 为什么不新增第二条装配路径
//!
//! E02 判据⑬⑭要的就是**一条路**：脚本 `spawn` 和 prefab 实例化必须落到同一个函数，
//! 否则重名拦截、父级解析、`touch_layout`、材质 / 标签 / 交互半径收敛这些逻辑就会
//! 在两条路上慢慢分叉。所以本模块里没有任何一处直接写 `self.nodes` / `name_index`
//! / `order`——实例化省下的只有「先回表取原型、再叠覆盖」这两步。
//!
//! # 为什么模板句柄与实例记录要分开存
//!
//! * `instances: Vec<PrefabInstance>` —— 落盘源数据（`scene.json` 的 `instances` 段）；
//! * `prefab_handles: HashMap<String, Arc<PrefabTemplate>>` —— 内存句柄，**不进序列化**。
//!
//! 100 个实例共享 1 份模板：`Arc::ptr_eq` 全等、`strong_count == 101`（100 个实例
//! + 表里那份），这也是「模板只有一个实体」的可执行口径。

use std::collections::HashSet;
use std::sync::Arc;

use crate::prefab::{apply, DefineOutcome, PrefabInstance, PrefabTableError, PrefabTemplate};
use crate::scene_material::normalize_material_ref;
use crate::{
    normalize_interact_radius, normalize_tags, MeshRef, Node, NodeId, ObjectDesc, Vec3, Vec4,
};

use super::Scene;

/// 未注册模板的报错前缀。
///
/// 单独定成常量是为了让上层（E04 的 `PrefabOpError::Conflict` 映射）能按前缀分流，
/// 而不必解析整句中文。E02 的判据⑪要的是「明确报错、不 panic」，所以这里是
/// `Result` 而不是 `expect`。
pub const ERR_UNKNOWN_TEMPLATE: &str = "prefab 模板未注册";

/// 场景文件归一化：去空白、丢重名、校验模板 ID 形状。
///
/// # 为什么重名要**丢后者**而不是报错
///
/// `instances` 是「声明的实例清单」，不是命令流：同一个名字声明两遍，前者与后者
/// 都描述同一个节点，保留先出现的那条顺序最稳定（与 `materials` 的「后覆盖前」不同，
/// 因为材质是纯数据、实例要落节点，顺序变了节点顺序就变了）。
///
/// # 为什么模板 ID 形状非法要**报错**
///
/// 形状即路径（`prefab/prop/crate.json`）。放行一个写不出文件的 ID，会让错误
/// 推迟到「保存模板」那一刻才炸，且现场已失去「是哪个实例写坏的」这条线索。
pub(crate) fn normalize_instances(
    list: Vec<PrefabInstance>,
) -> Result<Vec<PrefabInstance>, String> {
    let mut out: Vec<PrefabInstance> = Vec::with_capacity(list.len());
    let mut seen: HashSet<String> = HashSet::new();
    for (index, raw) in list.into_iter().enumerate() {
        let instance = raw.sanitized();
        if instance.name.is_empty() {
            return Err(format!("instances[{index}] 的 name 为空"));
        }
        if let Err(error) = instance.validate() {
            return Err(format!(
                "instances[{index}]（{}）的模板 ID `{}` 形状非法：{error}",
                instance.name, instance.template
            ));
        }
        if !seen.insert(instance.name.clone()) {
            continue;
        }
        out.push(instance);
    }
    Ok(out)
}

impl Scene {
    // ---------- 模板表 ----------

    /// 登记模板（会话内，不落场景文件）。
    ///
    /// 同 ID 再 `define` 是**替换**而不是追加：模板是素材，改模板文件后重新载入
    /// 走的就是这条路径。已实例化的节点握的是旧 `Arc`，不追溯刷新（见字段文档）。
    pub fn define_prefab(
        &mut self,
        template: PrefabTemplate,
    ) -> Result<DefineOutcome, PrefabTableError> {
        self.prefabs.define(template)
    }

    /// 按 ID 取模板句柄（`Arc` 克隆，共享同一份模板实体）。
    pub fn prefab_handle(&self, id: &str) -> Option<Arc<PrefabTemplate>> {
        self.prefabs.handle(id)
    }

    // ---------- 实例表 ----------

    /// 按实例名取落盘记录。
    pub fn instance(&self, name: &str) -> Option<&PrefabInstance> {
        self.instances.iter().find(|item| item.name == name)
    }

    /// 按实例名取**可写**记录（M2-E04 的 `set_override` / `clear_override` 入口）。
    ///
    /// 只借出这一条记录，不暴露整个 `instances` 向量：调用方改完覆盖表要立刻
    /// `resolve_overrides` 重算实体，能拿到「整表可写」反而容易绕过这条链路
    /// 直接改节点，留下「表里说 A、节点是 B」的漂移。
    pub fn instance_mut(&mut self, name: &str) -> Option<&mut PrefabInstance> {
        self.instances.iter_mut().find(|item| item.name == name)
    }

    /// 按实例名取它握着的模板句柄（会话内句柄表，不是落盘数据）。
    pub fn instance_template(&self, name: &str) -> Option<&Arc<PrefabTemplate>> {
        self.prefab_handles.get(name)
    }

    /// 这个名字是不是「prefab 实例节点」。
    ///
    /// `to_scene_file` 据此把实例节点从 `objects` 里排除：实例的真源是 `instances`，
    /// 两边都写会让同一个物件有两份数据，改一处不改另一处就漂移。
    pub fn is_instance_node(&self, name: &str) -> bool {
        self.instances.iter().any(|item| item.name == name)
    }

    /// 摘掉一条实例记录与它的模板句柄（节点被删时调用）。
    pub fn forget_instance(&mut self, name: &str) {
        self.instances.retain(|item| item.name != name);
        self.prefab_handles.remove(name);
    }

    // ---------- 实例化 ----------

    /// 算出实例要写进场景的 `ObjectDesc`（**不落节点**）。
    ///
    /// 判据⑬「实例化结果 == `apply(proto, overrides)`」比的就是本函数的返回值：
    /// 名字换成实例名，覆盖按三态（`Set` 替换 / `Clear` 清空 / `None` 继承）叠上去，
    /// 父名取实例自己的声明（`None` 即挂场景根），不沿用模板原型里的 `parent`——
    /// 那是模板内部的相对父名，直接带到场景里会指到别处去。
    pub fn resolve_overrides(&self, instance: &PrefabInstance) -> Result<ObjectDesc, String> {
        let handle = self
            .prefab_handles
            .get(&instance.name)
            .or_else(|| self.prefabs.get(&instance.template))
            .ok_or_else(|| {
                format!(
                    "{ERR_UNKNOWN_TEMPLATE}：实例 `{}` 引用的模板 `{}` 没登记（Conflict）",
                    instance.name, instance.template
                )
            })?;
        if handle.id != instance.template {
            // 句柄表按实例名索引，与模板 ID 不一致说明两者失配（例如实例被改名后
            // 记录没跟上）。宁可在这一层喊出来，也不要把 A 模板的实体装成 B 实例。
            return Err(format!(
                "实例 `{}` 的模板句柄指向 `{}`，与声明的 `{}` 不一致",
                instance.name, handle.id, instance.template
            ));
        }
        let mut desc = apply(&handle.proto, &instance.overrides, &instance.name);
        desc.parent = instance.parent.clone();
        Ok(desc)
    }

    /// 实例化一条实例：回表 → 算实体 → [`Scene::spawn`]。
    ///
    /// 重名拒绝、父级按名解析、`touch_layout` 全部由 `Scene::spawn` 负责——
    /// 本函数不重复实现，也不绕过（判据⑭）。
    pub fn spawn_instance(&mut self, instance: PrefabInstance) -> Result<NodeId, String> {
        self.instantiate_one(instance, false)
    }

    /// 实例化的唯一实体：`recorded = true` 表示这条实例**记录已在表里**
    /// （`instantiate_declared` 的补建路径），此时不再重复登记、也不判重。
    fn instantiate_one(
        &mut self,
        instance: PrefabInstance,
        recorded: bool,
    ) -> Result<NodeId, String> {
        let instance = instance.sanitized();
        if instance.name.is_empty() {
            return Err("prefab 实例需要一个非空 name".to_string());
        }
        if !recorded && self.instances.iter().any(|item| item.name == instance.name) {
            return Err(format!("prefab 实例 `{}` 已存在", instance.name));
        }

        // 先回表：模板不在就别留下「实例记录在、节点没有」的半截状态
        let handle = self
            .prefabs
            .get(&instance.template)
            .ok_or_else(|| {
                format!(
                    "{ERR_UNKNOWN_TEMPLATE}：实例 `{}` 引用的模板 `{}` 没登记（Conflict）",
                    instance.name, instance.template
                )
            })?
            .clone();

        let mut desc = apply(&handle.proto, &instance.overrides, &instance.name);
        desc.parent = instance.parent.clone();

        // 实体交给 spawn：重名 / 父级不存在都在这里被拒，失败时不留任何记录
        let id = self.spawn(desc)?;

        self.prefab_handles.insert(instance.name.clone(), handle);
        if !recorded {
            self.instances.push(instance);
        }
        Ok(id)
    }

    /// 批量实例化（场景文件载入 / 命令层一次 spawn 多个时用）。
    ///
    /// 失败即停：已成功的实例保留（节点已进场景，无法无损回滚），错误里带上是第几条
    /// 与它的名字，调用方据此定位。
    pub fn spawn_instances<I>(&mut self, instances: I) -> Result<Vec<NodeId>, String>
    where
        I: IntoIterator<Item = PrefabInstance>,
    {
        let mut ids = Vec::new();
        for (index, instance) in instances.into_iter().enumerate() {
            let name = instance.name.clone();
            match self.spawn_instance(instance) {
                Ok(id) => ids.push(id),
                Err(error) => {
                    return Err(format!(
                        "第 {index} 条 prefab 实例 `{name}` 实例化失败：{error}（已成功 {} 条）",
                        ids.len()
                    ))
                }
            }
        }
        Ok(ids)
    }

    /// 把当前 `instances` 全表实例化（节点已存在的跳过）。
    ///
    /// `from_file` 只把实例**记录**读进来，不展开节点：载入那一刻模板表还是空的
    /// （模板属素材，由应用层 `prefab_define` 灌入）。这条函数就是「模板灌好之后，
    /// 把场景里声明的实例补出来」的开关；已实例化的跳过，所以重复调用是幂等的。
    pub fn instantiate_declared(&mut self) -> Result<Vec<NodeId>, String> {
        let pending: Vec<PrefabInstance> = self
            .instances
            .iter()
            .filter(|item| self.find(&item.name).is_none())
            .cloned()
            .collect();
        let mut ids = Vec::with_capacity(pending.len());
        for instance in pending {
            let name = instance.name.clone();
            match self.instantiate_one(instance, true) {
                Ok(id) => ids.push(id),
                Err(error) => return Err(format!("实例 `{name}` 实例化失败：{error}")),
            }
        }
        Ok(ids)
    }

    // ---------- 覆盖变更后重算节点（M2-E04） ----------

    /// 按实例当前覆盖表重算**已存在**的节点（`parent` 除外）。
    ///
    /// 覆盖表是实例的真源、节点是算出来的结果：`set_override` / `clear_override` 只
    /// 改表，改完必须走这里，否则会出现「表里说 A、节点还是 B」的漂移（这正是
    /// `prefab.override_leak` 要抓的东西）。
    ///
    /// 返回 `false` 表示没算成：实例不在表里、模板未注册（该缺口由 doc 校验器报，
    /// 不是本函数的事）、或节点不存在（声明的实例还没 `instantiate_declared`）。
    ///
    /// 为什么不改 `parent`：父名变更要动 `name_index` / `order` / 布局，属 `Scene::reparent`
    /// 的活儿；在重算里顺手改会绕过那套逻辑，留下「父子链对不上」的中间态。
    pub fn sync_instance_node(&mut self, name: &str) -> bool {
        let desc = match self.instance(name) {
            Some(instance) => match self.resolve_overrides(instance) {
                Ok(desc) => desc,
                Err(_) => return false,
            },
            None => return false,
        };
        let Some(node) = self.find_mut(name) else {
            return false;
        };
        apply_desc_to_node(node, &desc);
        true
    }
}

/// 把 [`ObjectDesc`] 的各字段套到节点上（覆盖重算的唯一收敛点）。
///
/// 收敛口径与 [`Scene::spawn`] 用的是同一批函数（`MeshRef::parse` /
/// `normalize_material_ref` / `normalize_tags` / `normalize_interact_radius`）：
/// 两条路径若口径不同，同一个覆盖值经 spawn 与经 `sync_instance_node` 会落成两个样子。
fn apply_desc_to_node(node: &mut Node, desc: &ObjectDesc) {
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
    node.physics = desc.physics.clone();
    node.material = desc.material.map(|m| m.sanitized());
    node.material_ref = normalize_material_ref(desc.material_ref.clone());
    node.tags = desc.tags.as_deref().map(normalize_tags).unwrap_or_default();
    node.interact_radius = desc.interact_radius.and_then(normalize_interact_radius);
}
