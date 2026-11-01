//! r3e-core —— 引擎内核：数学、变换、场景图与生命周期。
//!
//! 该 crate 不依赖任何图形 API，可被渲染层、物理层、脚本层同时复用。

pub mod aabb;
pub mod air;
pub mod anim;
pub mod camera;
pub mod frustum;
pub mod glyph_atlas;
pub mod input;
pub mod light;
pub mod material;
pub mod mesh;
pub mod node;
pub mod procedural;
pub mod scene;
pub mod skin;
pub mod text;
pub mod transform;
pub mod ui;

pub use aabb::Aabb;
pub use air::{AirIssue, AirIssueKind};
pub use anim::{AnimClip, AnimInstance, AnimKey, AnimTrack};
pub use camera::Camera;
pub use frustum::Frustum;
pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
pub use glyph_atlas::{
    AtlasInsert, AtlasSlot, GlyphAtlas, GlyphKey, ShelfAllocator, DEFAULT_MAX_PAGES,
    DEFAULT_PAGE_SIZE,
};
pub use input::{ButtonEdge, InputState, KeyCode, KeyboardState, PadState};
pub use light::{Light, ShadowSettings};
pub use material::{
    clamp_roughness, cook_torrance_brdf, direct_light, distribution_ggx, emissive_radiance,
    fresnel_f0, fresnel_schlick, geometry_smith, ggx_alpha, BrdfSample, GeoTerms, SurfaceMaterial,
    DEFAULT_ROUGHNESS, DIELECTRIC_F0, MAX_EMISSIVE, MIN_ROUGHNESS,
};
pub use mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage, Vertex};
pub use node::{AssetId, BuiltinMesh, MeshRef, Node, NodeId};
pub use procedural::ProceduralProvider;
pub use scene::{
    AnimDecl, AssetDecl, ObjectDesc, PhysicsDesc, PhysicsSceneDesc, Scene, SceneFile, ShadowDesc,
};
pub use skin::{
    SkinBinding, SkinData, SkinInstance, SkinJoint, MAX_JOINTS_PER_VERTEX, MAX_JOINT_MATRICES,
};
pub use text::{
    estimate_text_height, estimate_text_width, layout_text, measure_text, ApproxMetrics, FontSet,
    GlyphBitmap, GlyphMetric, GlyphSource, LaidOutLine, LineMetrics, MetricsSource, PlacedGlyph,
    ScreenText, TextAlign, TextLayout, TextMeasure,
};
pub use transform::Transform;
pub use ui::{
    find, find_mut, merge_same_key, remove, resolve_rect, sorted_for_draw, upsert, upsert_styled,
    Anchor, UiElement, UiKind, UiRect, UiUnit, UiUpsert, UiValue, DEFAULT_BORDER_WIDTH,
    MAX_UI_ELEMENTS,
};
