//! r3e-core —— 引擎内核：数学、变换、场景图与生命周期。
//!
//! 该 crate 不依赖任何图形 API，可被渲染层、物理层、脚本层同时复用。

pub mod aabb;
pub mod air;
pub mod anim;
pub mod camera;
pub mod env;
pub mod frustum;
pub mod glyph_atlas;
pub mod input;
pub mod light;
pub mod material;
pub mod mesh;
pub mod node;
pub mod postfx;
pub mod procedural;
pub mod scene;
pub mod skin;
pub mod ssao;
pub mod text;
pub mod transform;
pub mod ui;

pub use aabb::Aabb;
pub use air::{AirIssue, AirIssueKind};
pub use anim::{AnimClip, AnimInstance, AnimKey, AnimTrack};
pub use camera::Camera;
pub use env::{
    env_brdf_approx, sh_basis, sky_radiance, sky_radiance_blurred, specular_dominant_dir,
    EnvironmentSettings, Sh9, DEFAULT_SH_SAMPLES, MAX_ENV_RADIANCE, MAX_IBL_INTENSITY,
    SH_COEFFICIENTS, SUN_SHAPE_EXPONENT,
};
pub use frustum::Frustum;
pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
pub use glyph_atlas::{
    AtlasInsert, AtlasSlot, GlyphAtlas, GlyphKey, ShelfAllocator, DEFAULT_MAX_PAGES,
    DEFAULT_PAGE_SIZE,
};
pub use input::{ButtonEdge, InputState, KeyCode, KeyboardState, PadState};
pub use light::{
    attenuation, cascade_splits, find_local, fit_cascade, frustum_slice_corners, remove_local,
    spot_factor, upsert_local, with_local, CascadeFit, CascadeSettings, Light, LocalLight,
    LocalLightKind, ShadowSettings, ATTENUATION_EPSILON, ATTENUATION_WINDOW_POWER, MAX_CASCADES,
    MAX_CONE_ANGLE_DEG, MAX_LIGHT_COLOR, MAX_LIGHT_INTENSITY, MAX_LIGHT_RANGE, MAX_LOCAL_LIGHTS,
    MIN_CONE_GAP_DEG, MIN_LIGHT_RANGE,
};
pub use material::{
    clamp_roughness, cook_torrance_brdf, direct_light, distribution_ggx, emissive_radiance,
    fresnel_f0, fresnel_schlick, geometry_smith, ggx_alpha, BrdfSample, GeoTerms, SurfaceMaterial,
    DEFAULT_ROUGHNESS, DIELECTRIC_F0, MAX_EMISSIVE, MIN_ROUGHNESS,
};
pub use mesh::{MaterialData, MeshData, MeshPartData, MeshProvider, TextureImage, Vertex};
pub use node::{AssetId, BuiltinMesh, MeshRef, Node, NodeId};
pub use postfx::{
    bright_pass, expose, exposure_scale, fxaa_edge_visible, luminance, resolve, tonemap,
    tonemap_aces, tonemap_reinhard, BloomSettings, PostFxSettings, TonemapMode, ACES_A, ACES_B,
    ACES_C, ACES_D, ACES_E, DEFAULT_BLOOM_INTENSITY, DEFAULT_BLOOM_KNEE, DEFAULT_BLOOM_THRESHOLD,
    DEFAULT_EXPOSURE_EV, FXAA_EDGE_THRESHOLD_MIN, FXAA_EDGE_THRESHOLD_RELATIVE, LUMA_WEIGHTS,
    MAX_BLOOM_INTENSITY, MAX_BLOOM_KNEE, MAX_BLOOM_THRESHOLD, MAX_EXPOSURE_EV, MIN_EXPOSURE_EV,
};
pub use procedural::ProceduralProvider;
pub use scene::{
    AnimDecl, AssetDecl, ObjectDesc, PhysicsDesc, PhysicsSceneDesc, Scene, SceneFile, ShadowDesc,
};
pub use skin::{
    SkinBinding, SkinData, SkinInstance, SkinJoint, MAX_JOINTS_PER_VERTEX, MAX_JOINT_MATRICES,
};
pub use ssao::{
    ao_from_occlusion, hash_u32, hemisphere_kernel, is_occluded, kernel_dir, kernel_scale,
    kernel_uniform, normal_from_neighbors, reconstruct_view_pos, SsaoSettings, DEFAULT_SSAO_BIAS,
    DEFAULT_SSAO_INTENSITY, DEFAULT_SSAO_RADIUS, DEFAULT_SSAO_SAMPLES, MAX_SSAO_INTENSITY,
    MAX_SSAO_RADIUS, MAX_SSAO_SAMPLES, MIN_AO_FACTOR, MIN_SSAO_BIAS, MIN_SSAO_RADIUS,
    MIN_SSAO_SAMPLES,
};
pub use text::{
    estimate_text_height, estimate_text_width, layout_text, measure_text, ApproxMetrics, FontSet,
    GlyphBitmap, GlyphMetric, GlyphSource, LaidOutLine, LineMetrics, MetricsSource, PlacedGlyph,
    ScreenText, TextAlign, TextLayout, TextMeasure,
};
pub use transform::Transform;
pub use ui::{
    find, find_mut, hit_test, hit_test_key, merge_same_key, remove, resolve_rect, sorted_for_draw,
    upsert, upsert_styled, Anchor, UiElement, UiEvent, UiEventKind, UiInteraction, UiKind, UiRect,
    UiUnit, UiUpsert, UiValue, DEFAULT_BORDER_WIDTH, MAX_UI_ELEMENTS, MAX_UI_EVENTS_PER_FRAME,
};
