//! 场景图：物件容器、生命周期（生成 / 消失）、层次变换与文件加载。

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use serde::Deserialize;

use crate::{Camera, Light, MeshRef, Node, NodeId};

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

    // ---------- 加载 ----------

    pub fn from_json_str(src: &str) -> Result<Self, String> {
        let file: SceneFile =
            serde_json::from_str(src).map_err(|e| format!("场景 JSON 解析失败: {e}"))?;
        Self::from_file(file)
    }

    pub fn from_file(file: SceneFile) -> Result<Self, String> {
        let mut scene = Scene::new();
        scene.name = file.name.unwrap_or_else(|| "untitled".to_string());
        scene.physics = file.physics.unwrap_or_default();

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
