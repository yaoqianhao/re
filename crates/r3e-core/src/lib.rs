//! r3e-core —— 引擎内核：数学、变换、场景图与生命周期。
//!
//! 该 crate 不依赖任何图形 API，可被渲染层、物理层、脚本层同时复用。

pub mod aabb;
pub mod air;
pub mod anim;
pub mod camera;
pub mod frustum;
pub mod input;
pub mod light;
pub mod mesh;
pub mod node;
pub mod procedural;
pub mod scene;
pub mod skin;
pub mod transform;

pub use aabb::Aabb;
pub use air::{AirIssue, AirIssueKind};
pub use anim::{AnimClip, AnimInstance, AnimKey, AnimTrack};
pub use camera::Camera;
pub use frustum::Frustum;
pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
pub use input::{ButtonEdge, InputState, PadState};
pub use light::{Light, ShadowSettings};
pub use mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage, Vertex};
pub use node::{AssetId, BuiltinMesh, MeshRef, Node, NodeId};
pub use procedural::ProceduralProvider;
pub use scene::{
    AnimDecl, AssetDecl, ObjectDesc, PhysicsDesc, PhysicsSceneDesc, Scene, SceneFile, ShadowDesc,
};
pub use skin::{
    SkinBinding, SkinData, SkinInstance, SkinJoint, MAX_JOINTS_PER_VERTEX, MAX_JOINT_MATRICES,
};
pub use transform::Transform;
