//! 场景节点：物件本体与内置网格种类。

use crate::transform::Transform;
use glam::{Mat4, Vec4};

/// 节点唯一标识。单调递增，永不复用，脚本句柄因此不会指向错误对象。
pub type NodeId = u64;

/// 引擎内置的程序化网格（无需外部模型文件即可运行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MeshKind {
    #[default]
    Cube,
    Sphere,
    Plane,
    Cylinder,
}

impl MeshKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cube" | "box" => Some(Self::Cube),
            "sphere" | "ball" => Some(Self::Sphere),
            "plane" | "ground" | "quad" => Some(Self::Plane),
            "cylinder" | "tube" => Some(Self::Cylinder),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cube => "cube",
            Self::Sphere => "sphere",
            Self::Plane => "plane",
            Self::Cylinder => "cylinder",
        }
    }
}

/// 场景中的一个物件。
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub transform: Transform,
    /// RGBA，线性空间。
    pub color: Vec4,
    pub mesh: MeshKind,
    pub visible: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// 物理后端的刚体句柄（预留）。
    pub body: Option<u64>,
    /// 场景文件里为该物件声明的物理描述（未声明为 None）。
    pub physics: Option<crate::scene::PhysicsDesc>,
    /// 每帧由 `Scene::update_world_transforms` 计算的缓存。
    pub world: Mat4,
    /// 世界法线矩阵缓存。
    pub world_normal: glam::Mat3,
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            transform: Transform::IDENTITY,
            color: Vec4::new(0.8, 0.8, 0.85, 1.0),
            mesh: MeshKind::Cube,
            visible: true,
            parent: None,
            children: Vec::new(),
            body: None,
            physics: None,
            world: Mat4::IDENTITY,
            world_normal: glam::Mat3::IDENTITY,
        }
    }

    /// 世界空间位置。
    pub fn world_position(&self) -> glam::Vec3 {
        self.world.w_axis.truncate()
    }
}
