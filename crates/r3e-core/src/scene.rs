//! 场景图：物件容器、生命周期（生成 / 消失）、层次变换与文件加载。

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use serde::Deserialize;

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
    }

    /// 清空所有物件（脚本热重载时用）。
    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
        self.order.clear();
        self.name_index.clear();
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
            for b in inst.bindings.iter() {
                let Some(track) = inst.clip.tracks.get(b.track as usize) else {
                    continue;
                };
                if let Some(transform) = track.sample(time) {
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
        let Some(inst_key) = self
            .anim_instance(instance)
            .map(|a| a.name.split('/').next().unwrap_or("").to_string())
        else {
            return false;
        };
        let head = format!("{inst_key}/");
        let Some(clip) = self
            .anim_clips
            .iter()
            .find(|(k, _)| {
                k.strip_prefix(&head)
                    .map(|n| n.eq_ignore_ascii_case(clip_name))
                    .unwrap_or(false)
            })
            .map(|(_, c)| c.clone())
        else {
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
