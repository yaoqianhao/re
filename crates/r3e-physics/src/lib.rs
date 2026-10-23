//! r3e-physics —— 物理抽象层（预留）。
//!
//! 设计目标：引擎内核与具体物理库解耦。默认 `NullBackend` 是纯空实现，
//! 物件完全由脚本驱动；将来接 Rapier / Jolt 时只替换后端，场景与脚本层零改动。

use glam::Vec3;
use r3e_core::{NodeId, Scene};

/// 刚体句柄，由后端分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Static,
    Dynamic,
    Kinematic,
}

impl BodyKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "static" | "fixed" => Self::Static,
            "kinematic" => Self::Kinematic,
            _ => Self::Dynamic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColliderShape {
    Box,
    Sphere,
    Capsule,
    Cylinder,
}

/// 创建一个刚体所需的全部信息。
#[derive(Debug, Clone)]
pub struct BodyDesc {
    pub kind: BodyKind,
    pub shape: ColliderShape,
    /// 半尺寸（Box / 碰撞体缩放）。
    pub half_extents: Vec3,
    pub mass: f32,
    pub restitution: f32,
    pub friction: f32,
    /// 初始世界位置 / 旋转 / 缩放，来自节点当下状态。
    pub position: Vec3,
    pub rotation: glam::Quat,
    pub scale: Vec3,
}

impl Default for BodyDesc {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            shape: ColliderShape::Box,
            half_extents: Vec3::splat(0.5),
            mass: 1.0,
            restitution: 0.2,
            friction: 0.6,
            position: Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

/// 物理后端契约。实现者负责积分、碰撞与把结果写回场景。
pub trait PhysicsBackend {
    fn name(&self) -> &'static str;

    /// 为节点创建刚体。返回句柄，写入 `Node::body`。
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;

    fn destroy_body(&mut self, handle: BodyHandle);

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3);

    fn set_gravity(&mut self, gravity: Vec3);

    /// 推进模拟。
    fn step(&mut self, dt: f32);

    /// 把物理解算结果同步回场景节点（位置 / 旋转）。
    fn sync_to_scene(&mut self, scene: &mut Scene);

    fn body_count(&self) -> usize;

    /// 可选：返回所有现存句柄，便于整体清空。默认返回 `None`（后端自行管理）。
    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        None
    }
}

// ---------------------------------------------------------------------------
// Null 后端：不做任何模拟，只记录句柄，保证脚本仍完全掌控物件。
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct NullBackend {
    next: u64,
    bodies: Vec<BodyHandle>,
}

impl NullBackend {
    pub fn new() -> Self {
        Self {
            next: 1,
            bodies: Vec::new(),
        }
    }
}

impl PhysicsBackend for NullBackend {
    fn name(&self) -> &'static str {
        "null"
    }

    fn create_body(&mut self, _node: NodeId, _desc: BodyDesc) -> BodyHandle {
        let handle = BodyHandle(self.next);
        self.next += 1;
        self.bodies.push(handle);
        handle
    }

    fn destroy_body(&mut self, handle: BodyHandle) {
        self.bodies.retain(|h| *h != handle);
    }

    fn apply_impulse(&mut self, _handle: BodyHandle, _impulse: Vec3) {}

    fn set_gravity(&mut self, _gravity: Vec3) {}

    fn step(&mut self, _dt: f32) {}

    fn sync_to_scene(&mut self, _scene: &mut Scene) {}

    fn body_count(&self) -> usize {
        self.bodies.len()
    }

    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        Some(self.bodies.clone())
    }
}

/// 物理世界门面：持有当前后端，并提供统一的空实现兜底。
pub struct Physics {
    backend: Box<dyn PhysicsBackend>,
    pub gravity: Vec3,
}

impl std::fmt::Debug for Physics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Physics")
            .field("backend", &self.backend.name())
            .field("gravity", &self.gravity)
            .finish()
    }
}

impl Default for Physics {
    fn default() -> Self {
        Self::new()
    }
}

impl Physics {
    pub fn new() -> Self {
        Self {
            backend: Box::new(NullBackend::new()),
            gravity: Vec3::new(0.0, -9.81, 0.0),
        }
    }

    pub fn with_backend(mut backend: Box<dyn PhysicsBackend>) -> Self {
        backend.set_gravity(Vec3::new(0.0, -9.81, 0.0));
        Self {
            backend,
            gravity: Vec3::new(0.0, -9.81, 0.0),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    pub fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle {
        self.backend.create_body(node, desc)
    }

    pub fn destroy_body(&mut self, handle: BodyHandle) {
        self.backend.destroy_body(handle)
    }

    pub fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3) {
        self.backend.apply_impulse(handle, impulse)
    }

    pub fn step(&mut self, dt: f32, scene: &mut Scene) {
        self.backend.step(dt);
        self.backend.sync_to_scene(scene);
    }

    pub fn body_count(&self) -> usize {
        self.backend.body_count()
    }

    /// 清空所有刚体（脚本热重载时用）。
    pub fn clear_bodies(&mut self) {
        // 后端契约只给了单个销毁，这里用句柄表兜底：后端自己保证幂等。
        if let Some(list) = self.backend.take_all_handles() {
            for h in list {
                self.backend.destroy_body(h);
            }
        }
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.gravity = gravity;
        self.backend.set_gravity(gravity);
    }
}
