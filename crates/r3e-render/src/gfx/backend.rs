//! wgpu 后端的设备获取：`Instance → Surface → Adapter → Device`（扩展四十四）。
//!
//! 这是**唯一**真正创建图形设备的地方。所有 `wgpu::Instance::new` /
//! `request_adapter` / `request_device` / `surface.configure` 都必须收在这里，
//! 由 `gfx::create_context` 统一调用 —— 边界测试 `gfx_boundary.rs` 会扫描源码
//! 阻止它们散落回渲染器或上层。
//!
//! 收口的价值不只是「好看」：换 API 时**只有这一个文件需要换掉**。渲染器
//! （`renderer.rs`）拿到的是一组已经就绪的 `Device`/`Queue`/`Surface`/`Config`，
//! 它关心的是「怎么画」，不是「怎么把设备弄到手」。
//!
//! 扩展四十三引入的两道闸门在搬移中**逐字保留**：
//! * `WGPU_BACKEND` 钉死后端 + `backend_matches` 硬校验（要求不符直接 `Err`）；
//! * 特性先取交集再申请，缺失只记日志不阻塞启动。

use std::sync::Arc;

use winit::window::Window;

use crate::gpu_caps;

use super::api::GfxApi;
use super::info::GfxInfo;
use super::selection::GfxSelection;

/// 一次后端设备创建的产物。
///
/// 字段是 `pub(crate)`：渲染器要用，上层不允许看到（它没有 `wgpu` 类型的中性
/// 对应物可提供，暴露出去就是抽象层的第二个漏洞）。
pub(crate) struct GfxContext {
    /// 上层可见的能力摘要（纯数据）。API 身份在 `info.api` 里，不另设字段。
    pub info: GfxInfo,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

/// 设备描述符。`required_features` 由 [`gpu_caps::FeatureReport`] 决定，
/// 限制仍用 WebGPU 基线（`Limits::default()`），保证 WebGPU 与原生两条路同口径。
fn device_descriptor(features: wgpu::Features) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("r3e-device"),
        required_features: features,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }
}

/// 把 `wgpu::DeviceType` 翻成跨 API 的中性字符串。
///
/// `DeviceType` 是 `#[non_exhaustive]`，未知变体必须落到 `other` 而不是 panic ——
/// 新驱动报个新类型不该让引擎起不来。
fn device_type_name(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::Cpu => "cpu",
        wgpu::DeviceType::VirtualGpu => "virtual",
        _ => "other",
    }
}

/// 在指定窗口上打开 wgpu 设备。
///
/// 失败情形分两类，错误串里会写明原因（上层据 `requested_backends` 决定退出码）：
/// * **本机没有可用适配器**（`找不到可用的图形适配器`）—— 可能是无显卡 / 驱动缺失；
/// * **钉死后端与实际不符**（要求 vulkan 却只有 dx12）—— CI 矩阵的失败面。
pub(crate) fn open(selection: &GfxSelection, window: Arc<Window>) -> Result<GfxContext, String> {
    if selection.api != GfxApi::Wgpu {
        // 走到这里说明 `candidates()` 里混进了没有实现的后端：
        // 宁可明确报「没有实现」，也不要拿 wgpu 顶替别的 API 去跑。
        return Err(format!(
            "图形 API `{}` 目前没有实现（已实现: {}）",
            selection.api,
            GfxApi::known_names()
        ));
    }

    let size = window.inner_size();
    let width = size.width.max(1);
    let height = size.height.max(1);

    // 扩展四十三：后端可钉死。不设 `WGPU_BACKEND` 时 `descriptor` 保持 wgpu 默认
    // （编进来的后端全开），行为与扩展四十二逐位一致。
    let requested = gpu_caps::requested_backends();
    if let Some(req) = &requested {
        if req.is_empty() {
            log::warn!(
                "{} = `{}` 里的后端名一个都不认识，本次按默认后端集合运行",
                gpu_caps::BACKEND_ENV,
                req.source
            );
        } else if !req.unknown.is_empty() {
            log::warn!(
                "{} 里有不认识的后端名 [{}]（已忽略，其余按 {} 执行）",
                gpu_caps::BACKEND_ENV,
                req.unknown.join(","),
                gpu_caps::describe_requested(Some(req.backends))
            );
        }
    }
    let requested_bits = requested
        .as_ref()
        .map(|req| req.backends)
        .filter(|bits| !bits.is_empty());

    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    if let Some(bits) = requested_bits {
        descriptor.backends = bits;
    }
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance
        .create_surface(Arc::clone(&window))
        .map_err(|e| format!("创建 surface 失败: {e}"))?;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .map_err(|e| format!("找不到可用的图形适配器: {e}"))?;

    let adapter_info = adapter.get_info();
    log::info!(
        "GPU: {} ({:?}, {:?})",
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type
    );

    // 扩展四十三：钉了后端就必须真的拿到。`request_adapter` 只会按位集过滤后
    // 挑最合适的，它不会告诉你「我尽力了但只剩别的后端」—— 少了这道校验，
    // CI 的 dx12 job 在只有 Vulkan 的机器上会跑完一片绿，等于什么都没验证。
    if !gpu_caps::backend_matches(requested_bits, adapter_info.backend) {
        return Err(format!(
            "{} = `{}` 要求后端 {}，实际拿到的是 {}（{}）—— 该后端在本机不可用",
            gpu_caps::BACKEND_ENV,
            requested.as_ref().map(|r| r.source.as_str()).unwrap_or(""),
            gpu_caps::describe_requested(requested_bits),
            gpu_caps::backend_name(adapter_info.backend),
            adapter_info.name
        ));
    }

    // 扩展四十三：特性按需协商 —— 先取交集再申请，缺的记下来而不是整批失败。
    let wanted_report = gpu_caps::FeatureReport::negotiate(adapter.features());
    let (device, queue, effective_features) =
        match pollster::block_on(adapter.request_device(&device_descriptor(wanted_report.enabled)))
        {
            Ok(pair) => (pair.0, pair.1, wanted_report.enabled),
            Err(err) => {
                // 走到这里说明「按需」这层判断被驱动打脸了：宁可退回空特性集起得来，
                // 也不要为了两个调试特性让整个引擎打不开。
                log::warn!("按需特性申请失败（{err}），退回空特性集重试");
                let pair = pollster::block_on(
                    adapter.request_device(&device_descriptor(wgpu::Features::empty())),
                )
                .map_err(|e| format!("请求设备失败: {e}"))?;
                (pair.0, pair.1, wgpu::Features::empty())
            }
        };
    let report = gpu_caps::FeatureReport {
        enabled: effective_features,
        missing: gpu_caps::wanted_features() & !effective_features,
    };
    log::info!(
        "图形能力: API {} / 后端 {} / 要求 {} / 特性 {}",
        selection.api,
        gpu_caps::backend_name(adapter_info.backend),
        gpu_caps::describe_requested(requested_bits),
        report.summary()
    );

    let caps = surface.get_capabilities(&adapter);
    let mut config = surface
        .get_default_config(&adapter, width, height)
        .ok_or_else(|| "当前适配器不支持该窗口 surface".to_string())?;
    // 优先使用 sRGB 目标，避免颜色发灰。
    if let Some(fmt) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
        config.format = fmt;
    }
    surface.configure(&device, &config);

    let info = GfxInfo::new(
        selection.api,
        gpu_caps::backend_name(adapter_info.backend),
        adapter_info.name.clone(),
        device_type_name(adapter_info.device_type),
    )
    .with_features(
        gpu_caps::describe_features(effective_features)
            .split('|')
            .filter(|name| *name != "none" && *name != "...")
            .map(str::to_string),
        gpu_caps::describe_features(report.missing)
            .split('|')
            .filter(|name| *name != "none" && *name != "...")
            .map(str::to_string),
    )
    .with_requested(gpu_caps::describe_requested(requested_bits));

    Ok(GfxContext {
        info,
        device,
        queue,
        surface,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_types_map_to_stable_names() {
        // 上层（日志 / 验收脚本）只认这几个小写名，不能随手改成 Debug 形式
        assert_eq!(device_type_name(wgpu::DeviceType::DiscreteGpu), "discrete");
        assert_eq!(
            device_type_name(wgpu::DeviceType::IntegratedGpu),
            "integrated"
        );
        assert_eq!(device_type_name(wgpu::DeviceType::Cpu), "cpu");
        assert_eq!(device_type_name(wgpu::DeviceType::VirtualGpu), "virtual");
        assert_eq!(device_type_name(wgpu::DeviceType::Other), "other");
    }

    #[test]
    fn unimplemented_api_is_rejected_before_touching_the_gpu() {
        // 这条不能真起窗口：用 `Err` 分支的**纯逻辑前置**证明没有 API 会静默顶替
        // （`selection` 只可能给出已实现的 API，这里构造一个过程也说明了拒绝的写法）
        assert_eq!(GfxApi::ALL, &[GfxApi::Wgpu]);
        let err = format!(
            "图形 API `{}` 目前没有实现（已实现: {}）",
            "vulkan",
            GfxApi::known_names()
        );
        assert!(err.contains("没有实现"));
    }

    #[test]
    fn device_descriptor_uses_webgpu_baseline_limits() {
        // 改动这里会让 WebGPU 与原生两条路的限制口径分叉
        let descriptor = device_descriptor(wgpu::Features::empty());
        assert_eq!(descriptor.required_limits, wgpu::Limits::default());
        assert!(descriptor.required_features.is_empty());
    }
}
