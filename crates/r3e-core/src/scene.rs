//! 场景图：物件容器、生命周期（生成 / 消失）、层次变换与文件加载。

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use serde::Deserialize;

use crate::skin::SkinInstance;
use crate::{AnimClip, AnimInstance, Camera, Light, MeshRef, Node, NodeId, Transform};

/// 单个物件在场景文件 / 脚本 `spawn` 中的描述。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectDesc {
    pub name: String,
    #[serde(default)]
    pub mesh: Option<String>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    #[serde(default)]
    pub scale: Option<[f32; 3]>,
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub visible: Option<bool>,
    /// 物理描述：只做数据承载，真正生效取决于启用的 `PhysicsBackend`。
    #[serde(default)]
    pub physics: Option<PhysicsDesc>,
}

/// 物理刚体描述（预留）。当前 `NullBackend` 会忽略，`rapier` 后端会消费。
#[derive(Debug, Clone, Deserialize)]
pub struct PhysicsDesc {
    /// "dynamic" | "static" | "kinematic"
    #[serde(default = "default_body_kind")]
    pub body: String,
    #[serde(default = "default_mass")]
    pub mass: f32,
    #[serde(default)]
    pub restitution: Option<f32>,
    #[serde(default)]
    pub friction: Option<f32>,
    /// "box" | "sphere" | "cylinder"，缺省由 mesh 推导
    #[serde(default)]
    pub collider: Option<String>,
    /// 碰撞体半尺寸，缺省由 scale 推导
    #[serde(default)]
    pub half_extents: Option<[f32; 3]>,
}

/// 场景级物理配置：对应场景 JSON 顶层 `physics` 字段。
///
/// 所有字段都是可选的，缺省值由 `r3e_physics::PhysicsConfig` 决定：
/// 默认 enabled=false（物理关闭）、backend="builtin"、重力 -9.81、地面 y=0。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PhysicsSceneDesc {
    /// 是否启用物理模拟。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// "builtin"（内置最简物理）| "null"（空转，只登记刚体）| "rapier"（需构建时启用）
    #[serde(default)]
    pub backend: Option<String>,
    /// 重力加速度向量，缺省 [0.0, -9.81, 0.0]。
    #[serde(default)]
    pub gravity: Option<[f32; 3]>,
    /// 地面平面高度，缺省 0.0。
    #[serde(default)]
    pub ground: Option<f32>,
}

fn default_body_kind() -> String {
    "dynamic".to_string()
}
fn default_mass() -> f32 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct CameraDesc {
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub target: Option<[f32; 3]>,
    #[serde(default)]
    pub fov: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LightDesc {
    #[serde(default)]
    pub direction: Option<[f32; 3]>,
    #[serde(default)]
    pub color: Option<[f32; 3]>,
    #[serde(default)]
    pub intensity: Option<f32>,
    #[serde(default)]
    pub ambient: Option<[f32; 3]>,
    /// 阴影配置（阶段 D）；省略时沿用引擎默认（开启，2048²，自适应半宽）。
    #[serde(default)]
    pub shadow: Option<ShadowDesc>,
}

/// 场景文件里的阴影配置（阶段 D）。字段都可省略，省略即沿用默认值。
#[derive(Debug, Clone, Deserialize)]
pub struct ShadowDesc {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub size: Option<u32>,
    /// 正交相机半宽；省略或 0 表示按场景包围球自适应
    #[serde(default, alias = "ortho_size")]
    pub ortho: Option<f32>,
    #[serde(default)]
    pub bias: Option<f32>,
    #[serde(default, alias = "normalBias")]
    pub normal_bias: Option<f32>,
    #[serde(default)]
    pub distance: Option<f32>,
    #[serde(default)]
    pub strength: Option<f32>,
}

/// 资产自带的动画播放声明（P3 起）。
///
/// 写在场景 `assets[]` 项里，语义是「导入后立刻给这个资产实例挂一个播放器」。
/// `clip` 省略时取该资产动画栈里的第一个片段。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimDecl {
    /// 片段名（大小写不敏感）。
    #[serde(default)]
    pub clip: Option<String>,
    /// 播放速度，缺省 1.0；负值为倒放。
    #[serde(default)]
    pub speed: Option<f32>,
    /// 是否循环，缺省 true。
    #[serde(default)]
    pub looping: Option<bool>,
    /// 起始时间（秒），缺省 0。
    #[serde(default)]
    pub time: Option<f32>,
    /// 是否立即播放，缺省 true。false 时实例存在但处于暂停状态。
    #[serde(default)]
    pub play: Option<bool>,
}

/// 场景文件里声明的外部资产（P2 起）。路径相对场景文件所在目录解析。
///
/// 声明后由应用层导入并在导入成功后按资产内网格 spawn 物件；
/// 物件也可以通过 `"mesh": "asset:<路径>#<序号>"` 直接引用同一资产。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetDecl {
    /// 资产路径（FBX 等）。
    pub path: String,
    /// 该资产下所有网格的整体偏移。
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// 整体缩放（三轴相同时即可当作单位换算，例如 FBX 以厘米建模就填 0.01）。
    #[serde(default)]
    pub scale: Option<[f32; 3]>,
    /// 只导入不 spawn（供脚本 / 其它物件的 `asset:` 引用使用）。
    #[serde(default)]
    pub spawn: Option<bool>,
    /// 导入后自动挂载的动画播放器（P3 起）。
    #[serde(default)]
    pub anim: Option<AnimDecl>,
}

/// 场景文件结构（JSON）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SceneFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
    #[serde(default)]
    pub camera: Option<CameraDesc>,
    #[serde(default)]
    pub light: Option<LightDesc>,
    #[serde(default)]
    pub objects: Vec<ObjectDesc>,
    #[serde(default)]
    pub physics: Option<PhysicsSceneDesc>,
    /// 外部资产声明（应用层负责真正导入）。
    #[serde(default)]
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
            name: "untitled".to_string(),
            time: 0.0,
            frame: 0,
            camera: Camera::default(),
            light: Light::default(),
            background: Vec4::new(0.05, 0.06, 0.08, 1.0),
            physics: PhysicsSceneDesc::default(),
            assets: Vec::new(),
            animations: Vec::new(),
            anim_clips: HashMap::new(),
            skins: Vec::new(),
        }
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
                || self.skins.iter().any(|s| s.bindings.iter().any(|b| b.node == id)))
        {
            self.prune_skins();
        }
    }

    /// 清空所有物件（脚本热重载时用）。
    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
        self.order.clear();
        self.name_index.clear();
        // 蒙皮实例全部指向已清空的节点，一并丢弃
        self.skins.clear();
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

    // ---------- 变换 ----------

    /// 重算世界矩阵。每帧在脚本执行后、渲染前调用一次。
    pub fn update_world_transforms(&mut self) {
        let ids: Vec<NodeId> = self.order.clone();
        let mut cache: HashMap<NodeId, Mat4> = HashMap::with_capacity(ids.len());

        for id in ids {
            let world = self.resolve_world(id, 0, &mut cache);
            if let Some(node) = self.nodes.get_mut(&id) {
                node.world = world;
                node.world_normal = glam::Mat3::from_mat4(world).inverse().transpose();
            }
        }

        // P4：世界矩阵一刷新就跟着求解蒙皮矩阵，渲染层拿到的永远是本帧结果，
        // 调用方无需记得额外多做一步。
        self.update_skinning();
    }

    fn resolve_world(
        &self,
        id: NodeId,
        depth: u32,
        cache: &mut HashMap<NodeId, Mat4>,
    ) -> Mat4 {
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

    /// 求解所有蒙皮实例的关节矩阵。
    ///
    /// `update_world_transforms` 会自动调用本方法，一般不需要手动调。
    /// 关节节点被销毁时该关节退化为单位矩阵，不会让整块网格塌到原点。
    pub fn update_skinning(&mut self) {
        if self.skins.is_empty() {
            return;
        }
        let worlds: HashMap<NodeId, Mat4> = self
            .nodes
            .iter()
            .map(|(id, node)| (*id, node.world))
            .collect();
        for skin in &mut self.skins {
            let model = worlds
                .get(&skin.mesh_node)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            skin.solve(model, |id| worlds.get(&id).copied());
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
                let transform = match (fresh, old) {
                    // 新片段缺该轨道：过渡期间由旧片段继续驱动，结束后停在末姿态
                    (None, Some(o)) => o,
                    (Some(n), Some(o)) => crate::anim::blend_transform(&o, &n, weight),
                    (Some(n), None) => n,
                    (None, None) => continue,
                };
                writes.push((b.node, transform));
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
                scene.light.shadow = s.sanitized();
            }
        }

        for obj in file.objects {
            scene.spawn(obj)?;
        }
        scene.update_world_transforms();
        Ok(scene)
    }
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
        inst.bindings = vec![crate::anim::AnimBinding { node: id, node_index: 1, path: "root/arm".to_string(), track: 0 }];
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
        inst.bindings = vec![crate::anim::AnimBinding { node: id, node_index: 1, path: "root/arm".to_string(), track: 0 }];
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
        inst.bindings = vec![crate::anim::AnimBinding { node: id, node_index: 1, path: "root/arm".to_string(), track: 0 }];
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
        assert!((p.x - 10.0).abs() < 1e-4, "切片段后应采样新片段，实际 {p:?}");
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
        inst.bindings = vec![crate::anim::AnimBinding { node: id, node_index: 1, path: "root/arm".to_string(), track: 0 }];
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
        assert_eq!(scene.anim_fade_source("demo/walk"), "", "0 时长不应留下过渡");
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
        assert!((x - 4.5).abs() < 1e-3, "缺轨道时应由旧片段驱动到 walk(0.45)=4.5，实际 {x}");

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
        inst.bindings = vec![crate::anim::AnimBinding { node: id, node_index: 1, path: "root/arm".to_string(), track: 0 }];
        scene.add_anim_instance(inst);

        scene.despawn("arm");
        scene.prune_anim_bindings();
        assert_eq!(
            scene.anim_instance("demo/walk").unwrap().bindings.len(),
            0
        );
        // 没有绑定也不该 panic
        scene.update_animations(0.1);
    }
}
