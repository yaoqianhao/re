//! 多分辨率 LOD 选择：距离 → 三档之一。
//!
//! C01 只做「选档」，真正的多分辨率采样在 C02；这里先把档位枚举、阈值与
//! 单调性钉死，避免 C02 再改一次对外形态。

use crate::LOD_STEPS;

/// 默认 LOD 距离阈值（米）：`<= near` 用 L0，`(near, far]` 用 L1，`> far` 用 L2。
pub const LOD_DISTANCES: [f32; 2] = [64.0, 200.0];

/// 地形 LOD 档位。**顺序即粗细顺序**（`L0 < L1 < L2`），派生 `Ord` 因此可以
/// 直接表达「档位不降级为更细」这类单调性断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lod {
    /// 全分辨率（步长 1）。
    L0,
    /// 半分辨率（步长 2）。
    L1,
    /// 四分分辨率（步长 4）。
    L2,
}

impl Lod {
    /// 档位序号（`0/1/2`）。
    pub fn index(self) -> usize {
        match self {
            Lod::L0 => 0,
            Lod::L1 => 1,
            Lod::L2 => 2,
        }
    }

    /// 该档的顶点步长，取值必为 [`LOD_STEPS`] 之一。
    pub fn step(self) -> u32 {
        LOD_STEPS[self.index()]
    }

    /// 由步长反查档位；不是合法步长返回 `None`（不猜最接近的）。
    pub fn from_step(step: u32) -> Option<Self> {
        LOD_STEPS
            .iter()
            .position(|s| *s == step)
            .map(|i| [Lod::L0, Lod::L1, Lod::L2][i])
    }

    /// 由序号取档位；越界返回 `None`。
    pub fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Lod::L0),
            1 => Some(Lod::L1),
            2 => Some(Lod::L2),
            _ => None,
        }
    }
}

/// LOD 距离阈值（米）。构造即校验：`near` 必须为正、`far` 必须大于 `near`，
/// 否则返回 `None` —— 反序阈值会让 [`lod_for`] 跳过 L1，属于配置错误而非可容忍输入。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodConfig {
    /// L0 的最远距离。
    pub near: f32,
    /// L1 的最远距离。
    pub far: f32,
}

impl Default for LodConfig {
    fn default() -> Self {
        Self {
            near: LOD_DISTANCES[0],
            far: LOD_DISTANCES[1],
        }
    }
}

impl LodConfig {
    /// 显式给定阈值。
    pub fn new(near: f32, far: f32) -> Option<Self> {
        if !near.is_finite() || !far.is_finite() || near <= 0.0 || far <= near {
            return None;
        }
        Some(Self { near, far })
    }
}

/// 距离（米）→ LOD 档位。阈值边界含在细档（`d == near` 仍是 L0）。
///
/// `NaN` / `inf` 一律按**最粗档**处理（`L2`）：算不出距离时给出最省的几何，
/// 而不是让整块地形按全分辨率画出来。
pub fn lod_for(distance: f32, cfg: &LodConfig) -> Lod {
    if !distance.is_finite() {
        return Lod::L2;
    }
    if distance <= cfg.near {
        Lod::L0
    } else if distance <= cfg.far {
        Lod::L1
    } else {
        Lod::L2
    }
}
