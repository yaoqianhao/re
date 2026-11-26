//! Prefab 数据模型（M2-E01）。
//!
//! 三件套：**模板**（[`PrefabTemplate`]，可复用的物件原型）、**实例**
//! （[`PrefabInstance`]，场景里对模板的一次引用）、**覆盖**（[`ObjectOverride`]，
//! 实例相对模板的差异）。
//!
//! # 为什么独立成模块，不塞进 `scene.rs`
//!
//! `scene.rs` 已登记为行数债务文件（见 `xtask/size_debt.json`），且「实例表接进
//! `Scene`」是 E02 的事。本卡只冻结**数据外形**：结构体、三态覆盖的 JSON 形状、
//! 模板 ID 形状、模板表的收敛规则。接场景（E02）、diff 算子（E03）、
//! 命令与不变量（E04）各自在自己卡里做，互不越界。
//!
//! # 三态覆盖（本模块最容易被写错的一处）
//!
//! 覆盖字段的取值集合是 `值 | null | 键缺失`：
//!
//! | JSON | 含义 | 结构体 |
//! |---|---|---|
//! | `"mesh": "a.glb"` | 覆盖为这个值 | `Some(Set(..))` |
//! | `"mesh": null` | **显式清空**模板值 | `Some(Clear)` |
//! | 键不存在 | 继承模板值 | `None` |
//!
//! serde 的 `Option<T>` 会把前两者都读成 `None`，丢失「清空」语义，
//! 所以字段类型是 `Option<FieldOverride<T>>` 并配了自定义反序列化
//! （见 [`instance`] 模块文档）。
//!
//! # 模板 ID 形状
//!
//! 点分小写 ASCII：至少两段（`域.名`，避免裸名将来撞保留字）、最多
//! [`PREFAB_ID_MAX_SEGMENTS`] 段、总长 ≤ [`PREFAB_ID_MAX_LEN`]、每段非空。
//! 形状即路径——[`prefab_rel_path`] 把 `prop.crate` 映射到 `prefab/prop/crate.json`，
//! 所以校验必须在写盘前做，而不是等文件系统报错。

pub mod diff;
pub mod instance;
pub mod table;
pub mod template;

pub use diff::{apply, diff, revert};
pub use instance::{FieldOverride, ObjectOverride, OverrideState, PrefabInstance};
pub use table::{DefineOutcome, PrefabTable, PrefabTableError};
pub use template::{
    PrefabTemplate, PrefabValidateError, PREFAB_MAX, PREFAB_MAX_TAGS, PREFAB_SCHEMA_VERSION,
};

/// 模板 ID 最小段数：至少「域.名」两段。
pub const PREFAB_ID_MIN_SEGMENTS: usize = 2;

/// 模板 ID 最大段数：再深下去路径就比 ID 更难读了。
pub const PREFAB_ID_MAX_SEGMENTS: usize = 4;

/// 模板 ID 最大字符数（按 `char` 计）。
pub const PREFAB_ID_MAX_LEN: usize = 64;

/// 模板 ID 形状非法的原因。
///
/// 每一条都带着定位信息（哪一段、哪个字符），工具层可以直接原样转述给用户，
/// 不必再自己猜「哪个 ID 写错了」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefabIdError {
    /// 空字符串（或只有空白）。
    Empty,
    /// 超过 [`PREFAB_ID_MAX_LEN`] 个字符。
    TooLong,
    /// 段数不在 [`PREFAB_ID_MIN_SEGMENTS`]..=[`PREFAB_ID_MAX_SEGMENTS`]。
    SegmentsOutOfRange { segments: usize },
    /// 出现 `a-z` / `0-9` / `.` 以外的字符。
    BadChar { ch: char, index: usize },
    /// 相邻两个点或首尾点造成的空段。
    EmptySegment { index: usize },
}

impl std::fmt::Display for PrefabIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "模板 ID 不能为空"),
            Self::TooLong => write!(f, "模板 ID 超过 {PREFAB_ID_MAX_LEN} 个字符"),
            Self::SegmentsOutOfRange { segments } => write!(
                f,
                "模板 ID 段数为 {segments}，要求 {PREFAB_ID_MIN_SEGMENTS}..={PREFAB_ID_MAX_SEGMENTS} 段"
            ),
            Self::BadChar { ch, index } => write!(
                f,
                "模板 ID 第 {index} 个字节出现非法字符 {ch:?}（只允许小写字母、数字与点）"
            ),
            Self::EmptySegment { index } => {
                write!(f, "模板 ID 第 {index} 段为空（不允许首尾点或连续点）")
            }
        }
    }
}

impl std::error::Error for PrefabIdError {}

/// 校验模板 ID 形状（点分小写 ASCII，段数与非空规则见模块文档）。
///
/// 首尾空白先 trim 再判——写盘路径与用户输入都可能带空格，
/// 与其让 `" prop.crate "` 变成一个打不开的目录，不如在这里收敛。
pub fn validate_prefab_id(id: &str) -> Result<(), PrefabIdError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(PrefabIdError::Empty);
    }
    if id.chars().count() > PREFAB_ID_MAX_LEN {
        return Err(PrefabIdError::TooLong);
    }
    let segments: Vec<&str> = id.split('.').collect();
    if !(PREFAB_ID_MIN_SEGMENTS..=PREFAB_ID_MAX_SEGMENTS).contains(&segments.len()) {
        return Err(PrefabIdError::SegmentsOutOfRange {
            segments: segments.len(),
        });
    }
    for (segment_index, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            return Err(PrefabIdError::EmptySegment {
                index: segment_index,
            });
        }
        for (offset, ch) in segment.char_indices() {
            if !(ch.is_ascii_lowercase() || ch.is_ascii_digit()) {
                return Err(PrefabIdError::BadChar { ch, index: offset });
            }
        }
    }
    Ok(())
}

/// [`validate_prefab_id`] 的布尔版（表查询、命令入参校验等「只要结论」的场合）。
pub fn is_valid_prefab_id(id: &str) -> bool {
    validate_prefab_id(id).is_ok()
}

/// 模板 ID → 仓库内相对路径（`prop.crate` → `prefab/prop/crate.json`）。
///
/// 非法 ID 返回 `None`：调用方要么已经校验过，要么就得先处理这个 `None`，
/// 不会出现「拼出一个必然写不出的路径」。
pub fn prefab_rel_path(id: &str) -> Option<String> {
    validate_prefab_id(id).ok()?;
    Some(format!("prefab/{}.json", id.trim().replace('.', "/")))
}
