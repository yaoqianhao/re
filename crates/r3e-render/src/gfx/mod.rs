//! 图形 API 抽象层（扩展四十四）：**引擎不再绑定单一图形 API**。
//!
//! # 为什么需要这一层
//!
//! 扩展四十三把「wgpu 内部走哪个后端」做成了可钉死、可校验；但那是**同一套 API
//! 内部**的事。真正的问题是更上面一层：`r3e-render` 的公开面把 `wgpu::AdapterInfo`、
//! `wgpu::Features`、`wgpu::Backends` 直接递给了宿主，于是「引擎能不能换一套图形
//! API」这件事，答案取决于宿主有没有跟着改。这不是抽象，是把耦合搬了个地方。
//!
//! # 两层边界
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────────────┐
//!  │ 宿主层  r3e-app / r3e-editor / 插件   ← 只认 GfxApi / GfxInfo │
//!  ├──────────────────────────────────────────────────────────────┤
//!  │ 门面层  Renderer + trait RenderBackend（本模块）              │
//!  │         跨 API 的稳定接口：身份 / 能力 / 尺寸 / resize        │
//!  ├──────────────────────────────────────────────────────────────┤
//!  │ 获取层  gfx::create_context → backend::open（设备怎么弄到手） │
//!  ├──────────────────────────────────────────────────────────────┤
//!  │ 实现层  renderer.rs：wgpu 具体绘制（管线 / 绑定 / pass）      │
//!  └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! **今天的样子**：只有一个后端 —— wgpu。所以「抽象」不是靠一堆 `dyn Trait`
//! 假装多态（那会让 456 处 wgpu 调用各自长出一条虚表分支），而是靠三件
//! 可以验证的事：
//!
//! 1. **一个入口**：设备只能由 [`create_context`] 创建，别处不许碰 Instance/Adapter；
//! 2. **一个身份**：[`GfxApi`] 描述「用哪套 API」，宿主读它做日志与分支；
//! 3. **一个中性能力面**：[`GfxInfo`] 用纯数据描述设备，**不含任何 `wgpu` 类型**。
//!
//! 因此新增一套 API（自建 Vulkan 绑定、Metal、D3D12 直连）时，改动局限在：
//! 往 [`GfxApi`] 加一个变体 → 在 `backend` 里加一个 `open` 实现 → 在
//! [`PLATFORM_PREFERENCE`] 对应平台行里插入优先级。渲染器与宿主代码不改。
//!
//! # 选择顺序
//!
//! `R3E_GFX_API`（显式指定，拼错即失败）→ 平台偏好表（自动选择）→
//! 候选按优先级逐个尝试。详见 [`selection`]。

pub mod api;
pub mod backend;
pub mod info;
pub mod selection;

use std::sync::Arc;

use winit::window::Window;

pub use api::{default_api, preference, GfxApi, GfxPlatform, PLATFORM_PREFERENCE};
pub(crate) use backend::GfxContext;
pub use info::GfxInfo;
pub use selection::{GfxSelection, SelectionSource, API_ENV, AUTO};

/// 跨 API 的渲染器契约：**任何图形后端都必须兑现的最小承诺**。
///
/// 这里刻意只收四件事。列全 `Renderer` 的 200 多个方法（设置阴影、后处理、
/// 拾取、文本……）会得到一份「wgpu 实现说明书」，而不是跨 API 契约：
/// 新后端要为每个 wgpu 特有的概念编一个假 API，抽象反而变成负担。
///
/// 需要更多能力的调用方应当依赖**具体类型** `Renderer`（它是 wgpu 实现），
/// 待到某能力在第二个后端上也有对应物，再提到这个 trait 上 —— **按证据扩面**，
/// 不按想象扩面。
pub trait RenderBackend {
    /// 该渲染器用的是哪套图形 API。
    fn gfx_api(&self) -> GfxApi;

    /// 设备能力摘要（纯数据，可放心进日志 / 遥测 / 插件上下文）。
    fn gfx_info(&self) -> &GfxInfo;

    /// 一行能力摘要，格式见 [`GfxInfo::caps_line`]。
    fn caps_line(&self) -> String;

    /// 当前绘制区尺寸（像素）。
    fn size(&self) -> (u32, u32);

    /// 窗口尺寸变化。跨 API 语义一致：重建尺寸相关的附件，不重建设备。
    fn resize(&mut self, width: u32, height: u32);
}

/// 读取当前 API 选择（`R3E_GFX_API` → 平台默认）。拼错时返回 `Err`。
pub fn selection() -> Result<GfxSelection, String> {
    GfxSelection::from_env()
}

/// 是否有人显式要求了 API。宿主用它区分「起不来是环境不满足」与「本机没显卡」。
pub fn api_was_explicitly_requested() -> bool {
    selection().map(|s| s.is_explicit()).unwrap_or(true)
}

/// 是否有人显式钉死了**后端**（`WGPU_BACKEND`）。
///
/// 这是给宿主退出码用的中性查询：返回布尔，不把 `wgpu::Backends` 交出去。
/// `true` = 后端起不来必须算失败（CI 矩阵靠它避免假绿）。
pub fn backend_was_explicitly_requested() -> bool {
    crate::gpu_caps::requested_backends().is_some()
}

/// 打开图形上下文：按 [`GfxSelection::candidates`] 的顺序**逐个尝试**，
/// 第一个成功的胜出；全部失败则返回最后一条错误。
///
/// 「逐个尝试」而不是「只试第一个」，是为将来的自动降级留的：某平台上自建
/// Vulkan 初始化失败时，应当能自动退到 wgpu，而不是整机起不来。现在候选只有
/// 一个，行为与扩展四十三完全一致。
pub(crate) fn create_context(
    selection: &GfxSelection,
    window: Arc<Window>,
) -> Result<GfxContext, String> {
    let candidates = selection.candidates();
    let mut last_error = String::from("没有可用的图形后端");

    for (index, api) in candidates.iter().enumerate() {
        let mut attempt = selection.clone();
        attempt.api = *api;
        // 候选回退时来源仍按原样保留：日志要能看出「用户点了名」还是「平台默认」
        match backend::open(&attempt, Arc::clone(&window)) {
            Ok(context) => {
                if index > 0 {
                    log::warn!(
                        "首选图形 API {} 起不来，已回退到 {}",
                        candidates[0],
                        attempt.api
                    );
                }
                return Ok(context);
            }
            Err(err) => {
                log::warn!("图形 API {} 初始化失败: {err}", attempt.api);
                last_error = err;
            }
        }
    }

    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_readable_from_env_path_without_a_gpu() {
        // 纯逻辑：不设变量时必须给出平台默认，不 panic
        let selection = selection().expect("未设 R3E_GFX_API 应成功");
        assert_eq!(selection.api, default_api(GfxPlatform::current()));
        assert!(GfxApi::ALL.contains(&selection.api));
    }

    #[test]
    fn explicit_backend_flag_defaults_to_false_in_clean_env() {
        // CI 不设 WGPU_BACKEND 时退出码语义必须与扩展四十三一致（不触发 3）
        if std::env::var(crate::gpu_caps::BACKEND_ENV).is_err() {
            assert!(!backend_was_explicitly_requested());
        }
    }

    #[test]
    fn candidates_always_contain_the_selected_api() {
        for platform in [GfxPlatform::Windows, GfxPlatform::Web] {
            let selection = GfxSelection::from_platform(platform);
            assert!(selection.candidates().contains(&selection.api));
        }
    }
}
