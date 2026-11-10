//! 渲染层对外出口。
//!
//! S1 起内置图元的几何生成不再属于本 crate：请用 `r3e_core::procedural::builtin_mesh`
//! 或 `r3e_core::ProceduralProvider`。这里只导出 wgpu 布局与渲染器本体。

pub mod envprobe;
pub mod mesh;
pub mod occlusion;
pub mod pbr;
pub mod postfx;
pub mod renderer;
pub mod ssao;
pub mod text;
pub mod transparency;
pub mod ui;

pub use envprobe::{
    encode_faces, f16_to_f32, f32_to_f16, mip_lod, EnvProbeFaceUpload, EnvProbeSource,
    EnvProbeStats, CUBE_FACES, DUMMY_CUBE_FACE_SIZE, ENV_CUBE_BINDING,
};
pub use mesh::{vertex_layout, InstanceRaw};
pub use occlusion::{
    OcclusionCuller, OcclusionStats, DEFAULT_READBACK_INTERVAL, MAX_READBACK_INTERVAL,
    MIN_READBACK_INTERVAL,
};
pub use pbr::{MaterialUniform, SurfaceKey};
pub use postfx::{PostFxPass, PostFxStats, BLOOM_LEVELS, HDR_FORMAT};
pub use renderer::{
    CapturedFrame, CullStats, LayeredProvider, LightsUniform, LocalLightRaw, LocalShadowUniform,
    MeshCache, RenderStats, Renderer, LIGHTS_UNIFORM_SIZE, LOCAL_LIGHT_STRIDE,
    LOCAL_SHADOW_NORMAL_BIAS, LOCAL_SHADOW_SLOT_STRIDE, LOCAL_SHADOW_UNIFORM_SIZE,
    MSAA_SAMPLE_COUNT,
};
pub use ssao::{SsaoPass, SsaoStats, AO_FORMAT, AO_PASSES_PER_FRAME};
pub use text::{GlyphUpload, TextBuild, TextInstance, TextPass, TextStats};
pub use ui::{
    images_to_load, plan_ui, plan_ui_limited, UiBatch, UiBuild, UiImageProvider, UiInstance,
    UiPass, UiStats, MAX_UI_QUADS, MAX_UI_TEXTURE_DIM,
};
