//! 跨 API 的图形能力描述（扩展四十四）。
//!
//! [`GfxInfo`] 是**唯一允许向上层暴露的设备身份**：纯字符串与枚举，不含
//! `wgpu::AdapterInfo`、`wgpu::Features` 之类的具体类型。理由很实在：
//! 宿主（`r3e-app`）与插件只要能读「跑在哪个 API / 哪条后端 / 什么设备 /
//! 拿到哪些特性」，就够写日志、够做验收断言、够决定要不要开某个可选特效；
//! 它们**没有任何理由**认识某一家的图形库类型。
//!
//! 一旦上层能拿到 `wgpu::AdapterInfo`，抽象就会从那个字段往回漏：下一个 API
//! 后端得伪造一个假 `wgpu::AdapterInfo` 才能满足签名，或者上层被迫写
//! `if api == wgpu` 的双分支 —— 这正是本项目要消灭的耦合。

use std::fmt::Write as _;

use super::api::GfxApi;

/// 一次成功的设备创建之后，上层能看到的全部图形能力。
///
/// 刻意**不实现 `Default`**：一个「空的能力报告」没有意义，谁想构造它就得先
/// 说清用的是哪套 API。少了这个默认值，就不会有人拿 `GfxInfo::default()`
/// 糊过去、把 `api` 留成某个固定后端的名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GfxInfo {
    /// 用的是哪套 API 家族。
    pub api: GfxApi,
    /// 该 API 内实际走的后端（`vulkan` / `dx12` / `metal` / `gl` …）。小写。
    pub backend: String,
    /// 适配器名字（如 `NVIDIA GeForce RTX 5060 Laptop GPU`）。
    pub adapter: String,
    /// 设备类型（`discrete` / `integrated` / `cpu` / `virtual` / `other`）。
    pub device_type: String,
    /// 实际启用的可选特性名，已排序。
    pub features: Vec<String>,
    /// 申请了但驱动没给的可选特性名。**空 = 全拿到**，非空不代表失败。
    pub missing: Vec<String>,
    /// 环境变量要求的后端（`any` = 未指定）。
    pub requested: String,
}

impl GfxInfo {
    /// 最小构造：只带设备身份，特性留空。
    pub fn new(
        api: GfxApi,
        backend: impl Into<String>,
        adapter: impl Into<String>,
        device_type: impl Into<String>,
    ) -> Self {
        Self {
            api,
            backend: backend.into(),
            adapter: adapter.into(),
            device_type: device_type.into(),
            features: Vec::new(),
            missing: Vec::new(),
            requested: "any".to_string(),
        }
    }

    /// 填入特性协商结果（保持调用方给的顺序，调用方负责排序稳定）。
    pub fn with_features(
        mut self,
        enabled: impl IntoIterator<Item = impl Into<String>>,
        missing: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.features = enabled.into_iter().map(Into::into).collect();
        self.missing = missing.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_requested(mut self, requested: impl Into<String>) -> Self {
        self.requested = requested.into();
        self
    }

    /// 是否有可选特性没拿到。只影响日志级别，不影响能否渲染。
    pub fn is_partial(&self) -> bool {
        !self.missing.is_empty()
    }

    /// 是否独立显卡（真机验收里用来区分「跑在 5060 上」和「跑在核显/软渲染上」）。
    pub fn is_discrete(&self) -> bool {
        self.device_type == "discrete"
    }

    /// 一行能力摘要，供启动日志与真机验收抓取。
    ///
    /// 格式**与扩展四十三逐字一致**（`backend=… device=… 特性 … 要求 …`）：
    /// 视觉回归测试按 `backend=` 字段做二次断言，改格式等于改契约。
    pub fn caps_line(&self) -> String {
        let mut features = if self.features.is_empty() {
            "none".to_string()
        } else {
            self.features.join("|")
        };
        if !self.missing.is_empty() {
            let _ = write!(features, " / 缺 {}", self.missing.join("|"));
        }
        format!(
            "api={} backend={} device={} ({}) 特性 {} 要求 {}",
            self.api, self.backend, self.adapter, self.device_type, features, self.requested
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GfxInfo {
        GfxInfo::new(
            GfxApi::Wgpu,
            "vulkan",
            "NVIDIA GeForce RTX 5060 Laptop GPU",
            "discrete",
        )
    }

    #[test]
    fn caps_line_keeps_the_probe_fields() {
        // 视觉回归脚本按 `backend=` 抓字段，这个前缀不能丢
        let info = sample()
            .with_features(["timestamp-query"], Vec::<String>::new())
            .with_requested("vulkan");
        let line = info.caps_line();
        assert!(line.contains("backend=vulkan"), "{line}");
        assert!(
            line.contains("device=NVIDIA GeForce RTX 5060 Laptop GPU"),
            "{line}"
        );
        assert!(line.contains("timestamp-query"), "{line}");
        assert!(line.contains("要求 vulkan"), "{line}");
        assert!(line.contains("api=wgpu"), "{line}");
    }

    #[test]
    fn caps_line_says_none_when_no_optional_feature() {
        let line = sample().caps_line();
        assert!(line.contains("特性 none"), "{line}");
        assert!(line.contains("要求 any"), "{line}");
        assert!(!line.contains("缺"));
    }

    #[test]
    fn missing_features_are_reported_but_not_fatal() {
        let info = sample().with_features(
            ["timestamp-query".to_string()],
            ["polygon-mode-line".to_string()],
        );
        assert!(info.is_partial());
        assert!(info.caps_line().contains("缺 polygon-mode-line"));
    }

    #[test]
    fn device_type_is_usable_without_touching_a_backend_type() {
        assert!(sample().is_discrete());
        let soft = GfxInfo::new(GfxApi::Wgpu, "vulkan", "llvmpipe", "cpu");
        assert!(!soft.is_discrete());
    }

    #[test]
    fn info_without_features_is_not_partial() {
        // 空特性集是**合法状态**（扩展四十三的约定）：不可因此报警或降级
        let info = sample();
        assert!(!info.is_partial());
        assert!(info.features.is_empty());
        assert!(!info.caps_line().is_empty());
    }
}
