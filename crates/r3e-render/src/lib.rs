//! 渲染层对外出口。
//!
//! S1 起内置图元的几何生成不再属于本 crate：请用 `r3e_core::procedural::builtin_mesh`
//! 或 `r3e_core::ProceduralProvider`。这里只导出 wgpu 布局与渲染器本体。

pub mod mesh;
pub mod renderer;

pub use mesh::{vertex_layout, InstanceRaw};
pub use renderer::{LayeredProvider, MeshCache, RenderStats, Renderer};
