//! 渲染层对外出口。
//!
//! S1 起内置图元的几何生成不再属于本 crate：请用 `r3e_core::procedural::builtin_mesh`
//! 或 `r3e_core::ProceduralProvider`。这里只导出 wgpu 布局与渲染器本体。

pub mod mesh;
pub mod pbr;
pub mod postfx;
pub mod renderer;
pub mod text;
pub mod ui;

pub use mesh::{vertex_layout, InstanceRaw};
pub use pbr::{MaterialUniform, SurfaceKey};
pub use postfx::{PostFxPass, PostFxStats, BLOOM_LEVELS, HDR_FORMAT};
pub use renderer::{LayeredProvider, MeshCache, RenderStats, Renderer};
pub use text::{GlyphUpload, TextBuild, TextInstance, TextPass, TextStats};
pub use ui::{
    images_to_load, plan_ui, plan_ui_limited, UiBatch, UiBuild, UiImageProvider, UiInstance,
    UiPass, UiStats, MAX_UI_QUADS, MAX_UI_TEXTURE_DIM,
};
