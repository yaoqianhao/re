//! wgpu 渲染器：单一前向管线 + 实例化绘制。
//!
//! 场景（`r3e_core::Scene`）是唯一真源，渲染器每帧从场景重建实例数据，
//! 因此脚本或 JSON 对物件的任何修改都会在下一帧直接反映到画面上。

mod backend;
mod buffers;
mod frame;
mod init;
mod instancing;
mod material;
mod mesh_cache;
mod pass;
mod shadow;
mod staging;
mod state;
mod stats;
mod types;
mod uniforms;
mod upload;

#[cfg(test)]
mod tests;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const INITIAL_INSTANCE_CAPACITY: u64 = 512;
/// 主通道的 MSAA 采样数（扩展十九）。
///
/// 4× 是 wgpu 三大后端（Vulkan / Metal / DX12）对 `Rgba16Float` 的共同下限，
/// 取它才能保证「同一份代码在哪都能跑」。主通道在多重采样附件上光栅化，
/// pass 结束时由硬件 resolve 到单采样 HDR 目标 —— 后处理链仍然只看到单采样纹理。
pub const MSAA_SAMPLE_COUNT: u32 = 4;
/// 蒙皮网格做视锥剔除时的包围盒外扩比例：动画形变会超出静止姿态的 AABB。
const SKIN_CULL_MARGIN: f32 = 0.25;

// ---- 对外 API（路径与拆分前一致，`lib.rs` 与本 crate 外部引用均不变）----
pub use mesh_cache::{LayeredProvider, MeshCache};
pub use state::Renderer;
pub use stats::{CapturedFrame, CullStats, InstanceUploadStats, RenderStats};
pub use uniforms::{
    LightsUniform, LocalLightRaw, LocalShadowUniform, LIGHTS_UNIFORM_SIZE, LOCAL_LIGHT_STRIDE,
    LOCAL_SHADOW_NORMAL_BIAS, LOCAL_SHADOW_SLOT_STRIDE, LOCAL_SHADOW_UNIFORM_SIZE,
};

// ---- crate 内部跨模块可见（拆分前是同文件私有项）----
pub(crate) use crate::pbr::SceneMaterialKey;
pub(crate) use buffers::{
    create_black_cube_texture, create_depth_target, create_flat_normal_texture,
    create_local_shadow_target, create_msaa_targets, create_shadow_bind_group,
    create_shadow_target, create_white_texture, upload_texture, ShadowBindInputs,
    TextureColorSpace,
};
pub(crate) use material::MaterialCache;
pub(crate) use mesh_cache::{GpuMesh, GpuPart, MeshMorph, MAX_MORPH_TABLE_FLOATS};
pub(crate) use staging::{staging_enabled_from_env, StagingTarget, UploadStaging};
pub(crate) use stats::StagingStats;
pub(crate) use types::{
    instance_count, morph_upload, shadow_runs, DrawGroup, EnvProbeGpu, InstancePlan, MorphUpload,
    ShadowPlan, ShadowSubmit,
};
pub(crate) use uniforms::{
    max_axis_scale, CameraUniform, MaterialKey, ShadowPassUniform, ShadowUniform,
};
pub(crate) use upload::upload_mesh;
