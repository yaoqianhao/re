//! 图形设备能力协商（扩展四十三）：后端钉死 + 特性按需申请。
//!
//! 这两件事原先都是「写死」的：
//!
//! * **后端**：`InstanceDescriptor::new_without_display_handle()` 保持 wgpu 默认
//!   （= 编译期编进来的全部后端），实际用哪个由 `request_adapter` 的优先级决定。
//!   结果是 Windows 上 DX12 被编进来、却从未被任何一次运行验证过 —— 编进来的代码
//!   不等于能跑的代码。现在 `WGPU_BACKEND`（与 wgpu 自己的环境变量同名）能把它钉死，
//!   并且**强制校验**实际拿到的 adapter 就是要求的那个：不校验的话，`dx12` job 在
//!   没有 DX12 的机器上会静默退到 Vulkan，跑完一片绿，等于什么都没验证。
//! * **特性**：`request_device` 原先 `required_features = Features::empty()`。现在按需
//!   申请（时间戳查询 + 线框 / 点模式），**能拿到多少用多少**，缺失项逐条记进
//!   [`FeatureReport::missing`]；整批申请失败则回退空集重试，绝不因此起不来。
//!
//! 本模块只有纯逻辑（解析 / 协商 / 描述）+ 两处薄薄的环境读取，零 GPU 调用，
//! 因此可以在无显卡的机器上单测。

use std::fmt::Write as _;

/// 后端环境变量名。刻意与 wgpu 自己认的 `WGPU_BACKEND` 同名：
/// 这样 CI 里设一个变量，wgpu 的 `*_from_env` 路径和 r3e 的显式钉死看起来是同一件事。
pub const BACKEND_ENV: &str = "WGPU_BACKEND";

/// 逐个后端探测用的「名字 → 位」表。顺序即日志顺序。
///
/// 只列**本仓库可能编进来的**后端：wgpu 30 的后端集合里 `Empty` / `Noop` 不参与协商。
pub const KNOWN_BACKENDS: &[(&str, wgpu::Backends)] = &[
    ("vulkan", wgpu::Backends::VULKAN),
    ("dx12", wgpu::Backends::DX12),
    ("metal", wgpu::Backends::METAL),
    ("gl", wgpu::Backends::GL),
    ("browser-webgpu", wgpu::Backends::BROWSER_WEBGPU),
];

/// 逐个特性探测用的「名字 → 位」表。顺序即日志顺序。
///
/// 只列 [`wanted_features`] 会申请的那几项 —— 这张表是给人读的，
/// 列全 40 多个特性只会把日志淹掉。
pub const PROBED_FEATURES: &[(&str, wgpu::Features)] = &[
    ("timestamp-query", wgpu::Features::TIMESTAMP_QUERY),
    (
        "timestamp-in-encoders",
        wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
    ),
    ("polygon-mode-line", wgpu::Features::POLYGON_MODE_LINE),
    ("polygon-mode-point", wgpu::Features::POLYGON_MODE_POINT),
];

/// 向后端**按需**申请的特性集合。
///
/// 选这四项的理由各不相同：
/// * `TIMESTAMP_QUERY` / `TIMESTAMP_QUERY_INSIDE_ENCODERS`：GPU 侧计时（帧内各 pass 的
///   真实耗时）。没有它就只能拿 CPU 墙钟猜 GPU 的开销，§11 的性能段全是这么来的；
/// * `POLYGON_MODE_LINE` / `POLYGON_MODE_POINT`：线框与点云调试视图。编辑器要看
///   碰撞体、法线、UV 时必需，且是「有则更好」的纯增量 —— 缺了不影响任何正常渲染。
///
/// **刻意不要**的特性（写了就是自找麻烦，列在这里防止下次有人顺手加）：
/// `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`（会改变纹理能力询问口径）、
/// `EXPERIMENTAL_*` 系列（不稳定）、`RAY_QUERY` / `RAY_TRACING`（与后端无关性冲突，
/// 见 §2 的技术路线取舍）。
pub fn wanted_features() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        | wgpu::Features::POLYGON_MODE_LINE
        | wgpu::Features::POLYGON_MODE_POINT
}

/// 一次协商的结果：**实际启用的**与**申请了但没拿到的**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureReport {
    /// 真正写进 `DeviceDescriptor::required_features` 的集合（= 想要 ∩ 支持）。
    pub enabled: wgpu::Features,
    /// 想要但驱动不支持的。空集 = 全部拿到。
    pub missing: wgpu::Features,
}

impl FeatureReport {
    /// 拿 adapter 报的支持集做协商：**只申请它支持的**，其余记进 `missing`。
    ///
    /// 这一步不能省。`request_device` 的 `required_features` 是**全有或全无**的：
    /// 多要一个不支持的特性，整次请求直接失败，连渲染器都建不起来。所以「按需」
    /// 的正确做法是先取交集，而不是把想要的丢进去赌一把。
    pub fn negotiate(available: wgpu::Features) -> Self {
        let wanted = wanted_features();
        let enabled = available & wanted;
        Self {
            enabled,
            missing: wanted & !enabled,
        }
    }

    /// 是否有任何一项没拿到。CI 里不因此失败，只用来决定日志级别。
    pub fn is_partial(&self) -> bool {
        !self.missing.is_empty()
    }

    /// 一行可读摘要，如 `timestamp-query|polygon-mode-line / 缺 timestamp-in-encoders`。
    pub fn summary(&self) -> String {
        let mut out = describe_features(self.enabled);
        if !self.missing.is_empty() {
            let _ = write!(out, " / 缺 {}", describe_features(self.missing));
        }
        out
    }
}

/// 把特性位集翻译成名字列表；空集返回 `none`。
///
/// 只翻译 [`PROBED_FEATURES`] 里那几项，表外的位用 `+N` 概括 —— 真出现表外的位，
/// 说明有人绕过了 [`wanted_features`]，值得在日志里显形。
pub fn describe_features(features: wgpu::Features) -> String {
    if features.is_empty() {
        return "none".to_string();
    }
    let mut names: Vec<&str> = Vec::new();
    for (name, bit) in PROBED_FEATURES {
        if features.contains(*bit) {
            names.push(name);
        }
    }
    // 表外位的个数 = 总位数 - 表内命中数（同一位置不会重复计）
    let out_of_table = features.iter().count().saturating_sub(names.len());
    if names.is_empty() {
        return format!("+{out_of_table}");
    }
    if out_of_table > 0 {
        names.push("...");
    }
    names.join("|")
}

/// 解析 `WGPU_BACKEND` 的取值（逗号或空格分隔，大小写不敏感）。
///
/// 返回**已识别的位集**与**不认识的名字**。后者不丢弃：拼错 `dx12` 为 `d3d12x`
/// 时静默回落到「全开」，CI 就会在错误的硬件路径上假绿，所以调用方必须把
/// `unknown` 打出来。
pub fn parse_backends(spec: &str) -> (wgpu::Backends, Vec<String>) {
    let mut backends = wgpu::Backends::empty();
    let mut unknown: Vec<String> = Vec::new();
    for raw in spec.split([',', ' ', '\t', ';']) {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        let mut hit = None;
        for (name, bit) in KNOWN_BACKENDS {
            if token == *name {
                hit = Some(*bit);
                break;
            }
        }
        // 常见别名：wgpu 文档里 d3d12 / mtl / vk 都出现过，不收会平白多一次踩坑
        if hit.is_none() {
            hit = match token.as_str() {
                "vk" => Some(wgpu::Backends::VULKAN),
                "d3d12" | "directx12" => Some(wgpu::Backends::DX12),
                "mtl" => Some(wgpu::Backends::METAL),
                "opengl" | "gles" => Some(wgpu::Backends::GL),
                "all" => Some(wgpu::Backends::all()),
                _ => None,
            };
        }
        match hit {
            Some(bits) => backends |= bits,
            None => unknown.push(token),
        }
    }
    (backends, unknown)
}

/// 从环境变量读到的后端要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedBackends {
    /// 要钉死的后端位集（可能为空 —— 全是不认识的名字时就是空）。
    pub backends: wgpu::Backends,
    /// 拼错 / 不认识的名字（原样小写）。
    pub unknown: Vec<String>,
    /// 环境变量的原始取值，日志里原样回显，便于对照 CI 配置。
    pub source: String,
}

impl RequestedBackends {
    /// 一位都没识别出来 —— 调用方应警告并当作「没指定」。
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }
}

/// 读 `WGPU_BACKEND`。`None` = 环境变量没设 = 用 wgpu 默认（编进来的后端全开）。
pub fn requested_backends() -> Option<RequestedBackends> {
    requested_backends_from(std::env::var(BACKEND_ENV).ok())
}

/// [`requested_backends`] 的纯函数版本：入参即环境变量取值。
pub fn requested_backends_from(spec: Option<String>) -> Option<RequestedBackends> {
    let source = spec?;
    if source.trim().is_empty() {
        return None;
    }
    let (backends, unknown) = parse_backends(&source);
    Some(RequestedBackends {
        backends,
        unknown,
        source,
    })
}

/// 单个 `wgpu::Backend` 对应的位。
///
/// `wgpu::Backend` 是 `#[non_exhaustive]` 的，未知变体返回空集 —— 空集 `contains`
/// 恒 false，于是「未知后端」在 [`backend_matches`] 里会判为不匹配，这是对的方向。
pub fn backend_bit(backend: wgpu::Backend) -> wgpu::Backends {
    match backend {
        wgpu::Backend::Vulkan => wgpu::Backends::VULKAN,
        wgpu::Backend::Dx12 => wgpu::Backends::DX12,
        wgpu::Backend::Metal => wgpu::Backends::METAL,
        wgpu::Backend::Gl => wgpu::Backends::GL,
        wgpu::Backend::BrowserWebGpu => wgpu::Backends::BROWSER_WEBGPU,
        _ => wgpu::Backends::empty(),
    }
}

/// 后端名字（小写），用于日志与错误信息。
pub fn backend_name(backend: wgpu::Backend) -> &'static str {
    for (name, bit) in KNOWN_BACKENDS {
        if backend_bit(backend) == *bit {
            return name;
        }
    }
    "unknown"
}

/// [`backend_name`] 的逆：把日志里的名字解析回 `wgpu::Backend`。
///
/// 用途是把「引擎日志说它跑在哪个后端」变成可断言的类型值 —— CI 的矩阵 job
/// 拿它可以自己复核一遍，不必只信进程内那道校验。
pub fn backend_from_name(name: &str) -> Option<wgpu::Backend> {
    let token = name.trim().to_ascii_lowercase();
    let (bits, _) = parse_backends(&token);
    if bits.is_empty() {
        return None;
    }
    // 只认单个后端：多选的位集没有唯一对应的 Backend
    for (_, bit) in KNOWN_BACKENDS {
        if bits == *bit {
            return Some(backend_of(*bit));
        }
    }
    None
}

/// 单个后端位 → `wgpu::Backend`。入参必须是单一位，否则返回 `Vulkan` 之外的值没有意义。
fn backend_of(bit: wgpu::Backends) -> wgpu::Backend {
    match bit {
        wgpu::Backends::DX12 => wgpu::Backend::Dx12,
        wgpu::Backends::METAL => wgpu::Backend::Metal,
        wgpu::Backends::GL => wgpu::Backend::Gl,
        wgpu::Backends::BROWSER_WEBGPU => wgpu::Backend::BrowserWebGpu,
        _ => wgpu::Backend::Vulkan,
    }
}

/// 实际拿到的 adapter 是否落在要求的后端集合里。
///
/// **这是后端矩阵的唯一防线**：没指定后端时恒 `true`（行为与扩展四十二完全一致）；
/// 指定了就必须严格命中。`request_adapter` 的语义是「按位集过滤后再挑最合适的」，
/// 它不会告诉你「我尽力了但只剩别的后端」—— 只有这道校验能。
pub fn backend_matches(requested: Option<wgpu::Backends>, actual: wgpu::Backend) -> bool {
    match requested {
        None => true,
        Some(bits) => {
            if bits.is_empty() {
                // 空集等于没要求（拼错的情况已在解析侧警告）
                return true;
            }
            bits.contains(backend_bit(actual))
        }
    }
}

/// 把要求编码成一行的可读形式（`any` = 未指定）。
pub fn describe_requested(requested: Option<wgpu::Backends>) -> String {
    match requested {
        None => "any".to_string(),
        Some(bits) if bits.is_empty() => "any".to_string(),
        Some(bits) => {
            let names: Vec<&str> = KNOWN_BACKENDS
                .iter()
                .filter(|(_, bit)| bits.contains(*bit))
                .map(|(name, _)| *name)
                .collect();
            if names.is_empty() {
                "any".to_string()
            } else {
                names.join("|")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_single_backend_name() {
        let (bits, unknown) = parse_backends("vulkan");
        assert_eq!(bits, wgpu::Backends::VULKAN);
        assert!(unknown.is_empty());
    }

    #[test]
    fn parses_lists_with_case_and_spaces() {
        let (bits, unknown) = parse_backends("  Vulkan , DX12 ");
        assert!(bits.contains(wgpu::Backends::VULKAN));
        assert!(bits.contains(wgpu::Backends::DX12));
        assert_eq!(bits, wgpu::Backends::VULKAN | wgpu::Backends::DX12);
        assert!(unknown.is_empty());
    }

    #[test]
    fn accepts_common_aliases() {
        let (bits, _) = parse_backends("vk,d3d12");
        assert_eq!(bits, wgpu::Backends::VULKAN | wgpu::Backends::DX12);
    }

    #[test]
    fn keeps_unrecognised_names_instead_of_dropping_them() {
        // 拼错的后端名不能静默变成「全开」：那会让 CI 在错误路径上假绿
        let (bits, unknown) = parse_backends("vulkan,d3d12x");
        assert_eq!(bits, wgpu::Backends::VULKAN);
        assert_eq!(unknown, vec!["d3d12x".to_string()]);
    }

    #[test]
    fn empty_spec_yields_nothing() {
        let (bits, unknown) = parse_backends("   ");
        assert!(bits.is_empty());
        assert!(unknown.is_empty());
        assert_eq!(requested_backends_from(Some("   ".to_string())), None);
        assert_eq!(requested_backends_from(None), None);
    }

    #[test]
    fn requested_backends_round_trips_the_source_string() {
        let req = requested_backends_from(Some("dx12".to_string())).expect("应解析成功");
        assert_eq!(req.source, "dx12");
        assert_eq!(req.backends, wgpu::Backends::DX12);
        assert!(!req.is_empty());
    }

    #[test]
    fn unset_backend_accepts_whatever_adapter_we_get() {
        // 不设环境变量时行为必须与扩展四十二逐位一致：任何后端都放行
        assert!(backend_matches(None, wgpu::Backend::Vulkan));
        assert!(backend_matches(None, wgpu::Backend::Dx12));
        assert!(backend_matches(None, wgpu::Backend::Gl));
    }

    #[test]
    fn requested_backend_must_match_the_actual_adapter() {
        // 矩阵的命门：要 dx12 却拿到 vulkan 必须判失败，否则 dx12 job 在
        // 只有 vulkan 的机器上会假装跑过
        assert!(!backend_matches(
            Some(wgpu::Backends::DX12),
            wgpu::Backend::Vulkan
        ));
        assert!(!backend_matches(
            Some(wgpu::Backends::VULKAN),
            wgpu::Backend::Dx12
        ));
        assert!(backend_matches(
            Some(wgpu::Backends::DX12),
            wgpu::Backend::Dx12
        ));
        // 多后端要求下命中其中任意一个都算过
        assert!(backend_matches(
            Some(wgpu::Backends::VULKAN | wgpu::Backends::DX12),
            wgpu::Backend::Dx12
        ));
    }

    #[test]
    fn empty_requested_bits_accept_anything() {
        assert!(backend_matches(
            Some(wgpu::Backends::empty()),
            wgpu::Backend::Vulkan
        ));
    }

    #[test]
    fn negotiate_takes_the_intersection() {
        let all = wanted_features();
        let full = FeatureReport::negotiate(all);
        assert_eq!(full.enabled, all);
        assert!(full.missing.is_empty());
        assert!(!full.is_partial());

        let none = FeatureReport::negotiate(wgpu::Features::empty());
        assert!(none.enabled.is_empty());
        assert_eq!(none.missing, all);
        assert!(none.is_partial());
    }

    #[test]
    fn negotiate_keeps_partial_features_instead_of_giving_up() {
        // 只有时间戳、没有线框的驱动是真实存在的（部分集成就这么报）
        let available = wgpu::Features::TIMESTAMP_QUERY;
        let report = FeatureReport::negotiate(available);
        assert_eq!(report.enabled, wgpu::Features::TIMESTAMP_QUERY);
        assert!(report.missing.contains(wgpu::Features::POLYGON_MODE_LINE));
        assert!(!report.enabled.contains(wgpu::Features::POLYGON_MODE_LINE));
    }

    #[test]
    fn wanted_features_cover_both_probed_families() {
        let wanted = wanted_features();
        assert!(wanted.contains(wgpu::Features::TIMESTAMP_QUERY));
        assert!(wanted.contains(wgpu::Features::POLYGON_MODE_LINE));
        // 表里列的全在申请集合内，否则日志会显示「没拿到」一件根本没要的东西
        for (_, bit) in PROBED_FEATURES {
            assert!(wanted.contains(*bit));
        }
    }

    #[test]
    fn backend_name_and_backend_from_name_round_trip() {
        for backend in [
            wgpu::Backend::Vulkan,
            wgpu::Backend::Dx12,
            wgpu::Backend::Metal,
            wgpu::Backend::Gl,
        ] {
            let name = backend_name(backend);
            assert_ne!(name, "unknown", "{backend:?} 必须在 KNOWN_BACKENDS 表里");
            assert_eq!(backend_from_name(name), Some(backend), "名字与位必须互为逆");
            // 位也要对得上，否则 backend_matches 的判定会与日志名脱节
            assert_eq!(backend_bit(backend), parse_backends(name).0);
        }
    }

    #[test]
    fn backend_from_name_rejects_unknown_and_ambiguous() {
        assert_eq!(backend_from_name("nope"), None);
        assert_eq!(backend_from_name(""), None);
        // 多选没有唯一对应的 Backend
        assert_eq!(backend_from_name("vulkan,dx12"), None);
        // 别名与大小写照样认
        assert_eq!(backend_from_name(" VK "), Some(wgpu::Backend::Vulkan));
        assert_eq!(backend_from_name("D3D12"), Some(wgpu::Backend::Dx12));
    }

    #[test]
    fn describe_features_is_human_readable() {
        assert_eq!(describe_features(wgpu::Features::empty()), "none");
        let line = describe_features(wgpu::Features::TIMESTAMP_QUERY);
        assert_eq!(line, "timestamp-query");
        let both =
            describe_features(wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::POLYGON_MODE_LINE);
        assert!(both.contains("timestamp-query"));
        assert!(both.contains("polygon-mode-line"));
    }

    #[test]
    fn summary_mentions_missing_items() {
        let report = FeatureReport::negotiate(wgpu::Features::TIMESTAMP_QUERY);
        let summary = report.summary();
        assert!(summary.contains("timestamp-query"));
        assert!(summary.contains("缺"));
    }

    #[test]
    fn describes_the_requested_set() {
        assert_eq!(describe_requested(None), "any");
        assert_eq!(describe_requested(Some(wgpu::Backends::empty())), "any");
        assert_eq!(
            describe_requested(Some(wgpu::Backends::VULKAN | wgpu::Backends::DX12)),
            "vulkan|dx12"
        );
    }
}
