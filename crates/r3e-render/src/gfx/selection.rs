//! 图形 API 选择策略（扩展四十四）。
//!
//! 有三层选择，**优先级从高到低**：
//!
//! 1. `R3E_GFX_API` 环境变量（本模块）：钉死 API 家族，拼错直接报错不启动；
//! 2. 平台偏好表（[`GfxPlatform`] + [`super::preference`]）：不设变量时的自动选择；
//! 3. 后端实现内部的细节选择（如 wgpu 的 `WGPU_BACKEND`）：由 `gpu_caps` 负责，
//!    属于「同一种 API 内选哪条驱动路径」，与本模块正交。
//!
//! 第 1 层拼错为什么**必须**失败而不是警告：API 家族一旦被静默换成别的，
//! 用户以为自己在验证 Vulkan 路径，实际跑的是 wgpu 的 DX12 —— 这正是
//! 扩展四十三在「后端」层面已经踩过并堵住的坑，这里沿用同一原则。

use super::api::{default_api, preference, GfxApi, GfxPlatform};

/// 钉死图形 API 的环境变量名。与 wgpu 的 `WGPU_BACKEND` 刻意区分：
/// 前者选「哪套 API 家族」，后者选「wgpu 内部走哪条驱动」。
pub const API_ENV: &str = "R3E_GFX_API";

/// 取值为 [`AUTO`] 或留空 = 交给平台偏好表。
pub const AUTO: &str = "auto";

/// 这次选择是怎么来的 —— 日志要能区分「用户指定」与「平台默认」，
/// 否则现场排障时无法判断该改环境变量还是该改平台表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSource {
    /// 平台偏好表自动选择（未设 `R3E_GFX_API`）。
    Platform,
    /// `R3E_GFX_API` 显式指定。
    Env,
}

impl SelectionSource {
    pub const fn name(self) -> &'static str {
        match self {
            SelectionSource::Platform => "platform",
            SelectionSource::Env => "env",
        }
    }
}

/// 一次选择的结果：选定 API + 来源 + 原始取值（日志回显用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GfxSelection {
    /// 首选 API。真正用上的可能排在 [`GfxSelection::candidates`] 更后面（首选起不来时）。
    pub api: GfxApi,
    pub source: SelectionSource,
    /// 环境变量原始文本（`None` = 未设）。
    pub requested: Option<String>,
}

impl GfxSelection {
    /// 纯平台默认：不读任何环境变量。测试与「忽略环境」的场景用。
    pub fn auto() -> Self {
        Self::from_platform(GfxPlatform::current())
    }

    /// 指定平台的默认选择。
    pub fn from_platform(platform: GfxPlatform) -> Self {
        Self {
            api: default_api(platform),
            source: SelectionSource::Platform,
            requested: None,
        }
    }

    /// 读 `R3E_GFX_API` 做选择。取值无法识别时返回 `Err`（不静默回落）。
    pub fn from_env() -> Result<Self, String> {
        Self::from_spec(std::env::var(API_ENV).ok(), GfxPlatform::current())
    }

    /// [`GfxSelection::from_env`] 的纯函数版本：入参即环境变量取值。
    pub fn from_spec(spec: Option<String>, platform: GfxPlatform) -> Result<Self, String> {
        let source = match spec {
            None => return Ok(Self::from_platform(platform)),
            Some(text) => text,
        };
        let token = source.trim().to_ascii_lowercase();
        if token.is_empty() || token == AUTO {
            let mut selection = Self::from_platform(platform);
            selection.requested = Some(source);
            return Ok(selection);
        }
        match GfxApi::from_name(&token) {
            Some(api) => Ok(Self {
                api,
                source: SelectionSource::Env,
                requested: Some(source),
            }),
            None => Err(format!(
                "{API_ENV} = `{source}` 不是已知的图形 API（可选: {} 或 {AUTO}）—— \
                 拒绝静默回落到默认实现，否则你以为在验证 {source}，实际跑的是别的",
                GfxApi::known_names()
            )),
        }
    }

    /// 按优先级排列的候选：首选在最前，其后是平台表的其余项（去重）。
    ///
    /// 显式指定 API 时只有它自己 —— 用户点名要什么就给什么，
    /// 悄悄退到别的 API 会让「指定」失去意义。
    pub fn candidates(&self) -> Vec<GfxApi> {
        match self.source {
            SelectionSource::Env => vec![self.api],
            SelectionSource::Platform => {
                let mut out: Vec<GfxApi> = Vec::new();
                for api in preference(GfxPlatform::current()) {
                    if !out.contains(api) {
                        out.push(*api);
                    }
                }
                if !out.contains(&self.api) {
                    out.push(self.api);
                }
                out
            }
        }
    }

    /// 是否是用户显式指定（= 起不来时该报错退出，而不是当成「本机没显卡」悄悄跳过）。
    pub fn is_explicit(&self) -> bool {
        self.source == SelectionSource::Env
    }

    /// 一行可读摘要：`wgpu (platform)` / `wgpu (env=R3E_GFX_API=wgpu)`。
    pub fn describe(&self) -> String {
        match self.source {
            SelectionSource::Platform => format!("{} (platform)", self.api),
            SelectionSource::Env => format!(
                "{} ({API_ENV}={})",
                self.api,
                self.requested.as_deref().unwrap_or("")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_env_uses_platform_default() {
        let selection = GfxSelection::from_spec(None, GfxPlatform::Windows).expect("未设应成功");
        assert_eq!(selection.api, default_api(GfxPlatform::Windows));
        assert_eq!(selection.source, SelectionSource::Platform);
        assert!(!selection.is_explicit());
        assert_eq!(selection.requested, None);
    }

    #[test]
    fn auto_and_blank_mean_platform_default() {
        for value in ["auto", "AUTO ", "  "] {
            let selection = GfxSelection::from_spec(Some(value.to_string()), GfxPlatform::Linux)
                .expect("应成功");
            assert_eq!(selection.source, SelectionSource::Platform, "{value}");
            assert_eq!(selection.api, default_api(GfxPlatform::Linux));
        }
    }

    #[test]
    fn named_api_is_honoured() {
        let selection = GfxSelection::from_spec(Some("Wgpu".to_string()), GfxPlatform::Windows)
            .expect("应成功");
        assert_eq!(selection.api, GfxApi::Wgpu);
        assert_eq!(selection.source, SelectionSource::Env);
        assert!(selection.is_explicit());
        assert_eq!(selection.requested.as_deref(), Some("Wgpu"));
    }

    #[test]
    fn unknown_api_is_an_error_not_a_silent_fallback() {
        // 静默回落会让「我在验证 vulkan」变成一句谎话
        let err = GfxSelection::from_spec(Some("vulkan".to_string()), GfxPlatform::Windows)
            .expect_err("未实现的 API 必须报错");
        assert!(err.contains("vulkan"));
        assert!(err.contains(API_ENV));
        assert!(err.contains(GfxApi::known_names().as_str()));
    }

    #[test]
    fn explicit_selection_has_exactly_one_candidate() {
        let selection = GfxSelection::from_spec(Some("wgpu".to_string()), GfxPlatform::Windows)
            .expect("应成功");
        assert_eq!(selection.candidates(), vec![GfxApi::Wgpu]);
    }

    #[test]
    fn automatic_selection_lists_platform_preference_first() {
        let selection = GfxSelection::from_platform(GfxPlatform::MacOs);
        let candidates = selection.candidates();
        assert_eq!(
            candidates.first().copied(),
            Some(default_api(GfxPlatform::MacOs))
        );
        // 去重：同一 API 不能在候选里出现两次（否则会重复尝试同一个后端）
        let mut unique = candidates.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), candidates.len());
    }

    #[test]
    fn describe_mentions_source() {
        let auto = GfxSelection::from_platform(GfxPlatform::Windows);
        assert!(auto.describe().contains("platform"));
        let env = GfxSelection::from_spec(Some("wgpu".to_string()), GfxPlatform::Windows)
            .expect("应成功");
        assert!(env.describe().contains(API_ENV));
    }
}
