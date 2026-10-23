//! r3e-core —— 引擎内核：数学、变换、场景图与生命周期。
//!
//! 该 crate 不依赖任何图形 API，可被渲染层、物理层、脚本层同时复用。

pub mod camera;
pub mod input;
pub mod light;
pub mod mesh;
pub mod node;
pub mod scene;
pub mod transform;

pub use camera::Camera;
pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
pub use input::{ButtonEdge, InputState, PadState};
pub use light::Light;
pub use mesh::{MeshData, Vertex};
pub use node::{AssetId, BuiltinMesh, MeshRef, Node, NodeId};
pub use scene::{ObjectDesc, PhysicsDesc, PhysicsSceneDesc, Scene, SceneFile};
pub use transform::Transform;
