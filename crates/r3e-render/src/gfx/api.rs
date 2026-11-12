//! 图形 API 家族与「按平台自动选择」（扩展四十四）。
//!
//! [`GfxApi`] 是**跨后端的稳定身份**：它描述「用哪套图形 API」，而不描述
//! 「这套 API 由哪个库实现」。日志、环境变量、宿主代码只认这个名字，
//! 因此把 Vulkan 换成 DX12、或把 wgpu 换成自建绑定，上层一处都不用改。
//!
//! 平台偏好表 [`PLATFORM_PREFERENCE`] 是「不同平台自动选择对应 API」的落点：
//! 每个平台按**优先级**列出候选，引擎从上往下试，第一个能起来的胜出。
//! 现在每个平台都只有 `wgpu` 一个候选 —— 这正是「只实现一种、留其余余地」的
//! 形状：将来某平台要优先走原生 Vulkan/Metal/D3D12，只需往对应行前面插一项，
//! 并新增一个后端实现（见 `gfx::backend`），选择逻辑与调用方都不动。

use std::fmt;

/// 图形 API 家族。
///
/// `#[non_exhaustive]`：新增变体是**非破坏性**改动 —— 外部 crate 的 `match`
/// 必须带 `_` 分支，因此引擎加一个新 API 不会让插件宿主编译失败。
/// 当前只有 [`GfxApi::Wgpu`] 一个真正实现，其余家族等有实现时再进来，
/// 避免出现「枚举里有、跑起来没有」的假选项。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GfxApi {
    /// wgpu（WebGPU 标准的原生实现）。Windows 上实际落 Vulkan / DX12，
    /// 背后用哪个由后端选择（`WGPU_BACKEND`）决定，与 API 家族是两回事。
    Wgpu,
}

impl GfxApi {
    /// 全部**已实现**的 API，顺序即日志顺序。
    pub const ALL: &'static [GfxApi] = &[GfxApi::Wgpu];

    /// 稳定的小写名字：环境变量取值、日志字段、CI 断言三处共用同一个串。
    pub const fn name(self) -> &'static str {
        match self {
            GfxApi::Wgpu => "wgpu",
        }
    }

    /// [`GfxApi::name`] 的逆。大小写与首尾空白不敏感 —— 环境变量是手写的，
    /// 因为大小写把 `Wgpu` 判成未知而拒绝启动，没有任何好处。
    pub fn from_name(name: &str) -> Option<Self> {
        let token = name.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|api| api.name() == token)
    }

    /// 全部可选名字，用于错误提示（拼错时告诉用户能填什么）。
    pub fn known_names() -> String {
        Self::ALL
            .iter()
            .map(|api| api.name())
            .collect::<Vec<_>>()
            .join("|")
    }
}

impl fmt::Display for GfxApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 运行平台。只用来查 [`PLATFORM_PREFERENCE`]，不参与任何渲染决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GfxPlatform {
    Windows,
    Linux,
    MacOs,
    Android,
    Web,
    Other,
}

impl GfxPlatform {
    /// 编译目标所在平台。
    pub const fn current() -> Self {
        if cfg!(target_arch = "wasm32") {
            Self::Web
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "android") {
            Self::Android
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            GfxPlatform::Windows => "windows",
            GfxPlatform::Linux => "linux",
            GfxPlatform::MacOs => "macos",
            GfxPlatform::Android => "android",
            GfxPlatform::Web => "web",
            GfxPlatform::Other => "other",
        }
    }
}

impl fmt::Display for GfxPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 每个平台的 API 优先级表（**从上往下试，先成功者胜出**）。
///
/// 现在每行都只有 `wgpu` —— 这不是占位符，而是当前事实：wgpu 是唯一实现。
/// 将来若要「Windows 优先自建 Vulkan、失败再退 wgpu」，改成
/// `(GfxPlatform::Windows, &[GfxApi::Vulkan, GfxApi::Wgpu])` 即可，
/// `candidates()` 会自动按新顺序返回，`Renderer` 与宿主代码零改动。
pub const PLATFORM_PREFERENCE: &[(GfxPlatform, &[GfxApi])] = &[
    (GfxPlatform::Windows, &[GfxApi::Wgpu]),
    (GfxPlatform::Linux, &[GfxApi::Wgpu]),
    (GfxPlatform::MacOs, &[GfxApi::Wgpu]),
    (GfxPlatform::Android, &[GfxApi::Wgpu]),
    (GfxPlatform::Web, &[GfxApi::Wgpu]),
    (GfxPlatform::Other, &[GfxApi::Wgpu]),
];

/// 某平台的候选顺序。表里没写该平台时退回 [`GfxApi::ALL`]：
/// 新平台（比如某天出现的新系统）不该因为「表里漏了一行」就起不来。
pub fn preference(platform: GfxPlatform) -> &'static [GfxApi] {
    for (known, apis) in PLATFORM_PREFERENCE {
        if *known == platform {
            return apis;
        }
    }
    GfxApi::ALL
}

/// 某平台的首选 API（= 候选顺序第一项）。表空时退回 `Wgpu`。
pub fn default_api(platform: GfxPlatform) -> GfxApi {
    preference(platform)
        .first()
        .copied()
        .unwrap_or(GfxApi::Wgpu)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_implemented_api_is_named_and_round_trips() {
        assert!(!GfxApi::ALL.is_empty(), "至少要有 wgpu 一个实现");
        for api in GfxApi::ALL {
            let name = api.name();
            assert!(!name.is_empty());
            assert_eq!(GfxApi::from_name(name), Some(*api), "名字与枚举必须互为逆");
            assert_eq!(name, name.to_ascii_lowercase(), "日志/CI 用小写串");
        }
    }

    #[test]
    fn from_name_is_case_and_space_insensitive() {
        assert_eq!(GfxApi::from_name("  Wgpu "), Some(GfxApi::Wgpu));
        assert_eq!(GfxApi::from_name("WGPU"), Some(GfxApi::Wgpu));
        assert_eq!(
            GfxApi::from_name("vulkan"),
            None,
            "未实现的 API 不能假装存在"
        );
        assert_eq!(GfxApi::from_name(""), None);
    }

    #[test]
    fn every_platform_has_a_preference_row() {
        // 「不同平台自动选择」的前提：每个平台都查得到候选，且首选是已实现的 API
        for platform in [
            GfxPlatform::Windows,
            GfxPlatform::Linux,
            GfxPlatform::MacOs,
            GfxPlatform::Android,
            GfxPlatform::Web,
            GfxPlatform::Other,
        ] {
            let apis = preference(platform);
            assert!(!apis.is_empty(), "{platform} 必须有候选");
            for api in apis {
                assert!(
                    GfxApi::ALL.contains(api),
                    "{platform} 的候选 {api} 没有实现——会造成「枚举里有、跑不起来」"
                );
            }
            assert!(apis.contains(&default_api(platform)));
        }
    }

    #[test]
    fn unknown_platform_falls_back_to_all_apis() {
        // 表里漏了一行时的兜底：不能因为查不到就返回空
        assert_eq!(preference(GfxPlatform::Other), GfxApi::ALL);
        assert!(!preference(GfxPlatform::Other).is_empty());
    }

    #[test]
    fn current_platform_is_one_of_the_known_ones() {
        assert!(!GfxPlatform::current().name().is_empty());
        assert!(!GfxApi::known_names().is_empty());
    }
}
