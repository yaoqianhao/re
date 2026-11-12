//! 扩展四十三的**对外契约**：`r3e_render::gpu_caps` 的公开面必须够 CI 与宿主自己用。
//!
//! 模块内的单测验算法，这个文件只验「从 crate 外部能不能按文档那样用」——
//! 导出漏了、函数签名变了，这里先红。

use r3e_render::gpu_caps::{
    backend_matches, backend_name, describe_features, describe_requested, parse_backends,
    requested_backends_from, FeatureReport, BACKEND_ENV,
};

#[test]
fn backend_env_name_is_the_documented_one() {
    // CI 与本模块共用同一个变量名，改名会让两边悄悄错位
    assert_eq!(BACKEND_ENV, "WGPU_BACKEND");
}

#[test]
fn parse_then_match_round_trip() {
    let req = requested_backends_from(Some("vulkan,dx12".to_string())).expect("应解析成功");
    assert!(backend_matches(Some(req.backends), wgpu::Backend::Vulkan));
    assert!(backend_matches(Some(req.backends), wgpu::Backend::Dx12));
    assert!(!backend_matches(Some(req.backends), wgpu::Backend::Gl));
}

#[test]
fn unknown_backend_names_surface_to_the_caller() {
    let req = requested_backends_from(Some("dx12x".to_string())).expect("非空即解析");
    assert_eq!(req.unknown, vec!["dx12x".to_string()]);
    // 一位都没识别出来 -> 空位集 -> 等价于「没指定」，但仍保留原始串供日志回显
    assert!(req.is_empty());
    assert_eq!(req.source, "dx12x");
    assert!(backend_matches(Some(req.backends), wgpu::Backend::Vulkan));
}

#[test]
fn feature_report_is_usable_from_outside() {
    let report = FeatureReport::negotiate(wgpu::Features::TIMESTAMP_QUERY);
    assert!(report.enabled.contains(wgpu::Features::TIMESTAMP_QUERY));
    assert!(report.is_partial());
    assert!(report.summary().contains("timestamp-query"));
    assert_eq!(describe_features(wgpu::Features::empty()), "none");
}

#[test]
fn backend_names_are_stable_strings() {
    // 日志与 CI 断言都靠这几个名字，不能随手改成 Debug 形式（`Vulkan`）
    assert_eq!(backend_name(wgpu::Backend::Vulkan), "vulkan");
    assert_eq!(backend_name(wgpu::Backend::Dx12), "dx12");
    assert_eq!(backend_name(wgpu::Backend::Metal), "metal");
    assert_eq!(backend_name(wgpu::Backend::Gl), "gl");
    assert_eq!(describe_requested(None), "any");
}

#[test]
fn parse_accepts_what_ci_actually_passes() {
    // 与 .github/workflows/ci.yml 里 visual-gpu 矩阵的取值一一对应
    for value in ["vulkan", "dx12"] {
        let (bits, unknown) = parse_backends(value);
        assert!(unknown.is_empty(), "CI 传的值必须能被解析: {value}");
        assert!(!bits.is_empty(), "CI 传的值不应解析成空位集: {value}");
    }
}
