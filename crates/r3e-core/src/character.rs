//! 角色控制器的场景侧配置（M2-B01）。
//!
//! 这里只放**静态配置**：一个物件「能不能当角色走、走起来多高多胖、能上多陡的坡」。
//! 每帧变化的运行态（位置 / 竖直速度 / 着地标志）故意不放这里，也不进
//! [`Node`](crate::Node)——运行态归 `r3e-character` 的控制器持有，理由见
//! `docs/m2/M2_B_PLAN.md` §3.3：回放金标 `385482eb033d2964` 依赖「场景真值不随
//! 物理步进漂移」，运行态一旦进场景，每帧都会改 `StateHash`。
//!
//! 配置进场景的三个直接后果（新增/调整本结构时都要重新想一遍）：
//!
//! 1. **可撤销**：改配置走 `r3e-doc` 的 `SetCharacter` 命令，撤销靠前像。
//! 2. **可差分**：`StateHash` 覆盖它（仅在 `Some` 时混入，见 `state_hash.rs`），
//!    AI 改了一个台阶高度能在哈希上看出来。
//! 3. **可校验**：非法值在写入前收敛掉，不留到步进时再 panic。

use std::fmt;

/// 半径下限（米）。再小就不是角色，是一根针。
pub const CHARACTER_MIN_RADIUS: f32 = 0.05;
/// 半径上限（米）。
pub const CHARACTER_MAX_RADIUS: f32 = 2.0;
/// 身高下限（米）。
pub const CHARACTER_MIN_HEIGHT: f32 = 0.2;
/// 身高上限（米）。
pub const CHARACTER_MAX_HEIGHT: f32 = 4.0;
/// 坡度上限的合法区间（度）：0° 只能走平地，90° 等于不限坡度。
pub const CHARACTER_MAX_SLOPE_DEG: f32 = 89.0;
/// 台阶高度上限（米）：超过身高的台阶本来也上不去。
pub const CHARACTER_MAX_STEP_HEIGHT: f32 = 2.0;
/// 地面吸附距离上限（米）。
pub const CHARACTER_MAX_SNAP_DISTANCE: f32 = 2.0;

/// 角色配置的非法原因。
///
/// 一个枚举而不是字符串：调用方（`r3e-doc` / RPC 宿主 / 脚本）要按类别给出
/// 各自的错误码与文案，字符串匹配会让「改一个字就失配」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterSpecError {
    /// 半径不在 `[0.05, 2.0]` 或非有限。
    Radius,
    /// 身高不在 `[0.2, 4.0]` 或非有限。
    Height,
    /// 坡度上限不在 `[0, 89]` 或非有限。
    Slope,
    /// 台阶高度不在 `[0, 2.0]` 或非有限。
    StepHeight,
    /// 吸附距离不在 `[0, 2.0]` 或非有限。
    SnapDistance,
    /// 台阶高度超过了身高（站上去等于把脚抬到头顶）。
    StepTallerThanBody,
    /// 吸附距离超过了身高（吸附范围比身体还长，等于瞬移）。
    SnapLongerThanBody,
}

impl CharacterSpecError {
    /// 给调用方拼错误文案用的短中文描述。
    pub fn reason(self) -> &'static str {
        match self {
            Self::Radius => "半径需在 0.05~2.0 米之间",
            Self::Height => "身高需在 0.2~4.0 米之间",
            Self::Slope => "坡度上限需在 0~89 度之间",
            Self::StepHeight => "台阶高度需在 0~2.0 米之间",
            Self::SnapDistance => "吸附距离需在 0~2.0 米之间",
            Self::StepTallerThanBody => "台阶高度不得超过身高",
            Self::SnapLongerThanBody => "吸附距离不得超过身高",
        }
    }
}

impl fmt::Display for CharacterSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

/// 角色的静态配置。
///
/// 字段都是米 / 度，与场景其余部分同一单位，避免再引入一套「角色单位」。
/// 默认值取「普通成年人」量级：半径 0.3、身高 1.8、能上 45° 坡、台阶 0.35、吸附 0.1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterSpec {
    /// 胶囊半径（米）。
    pub radius: f32,
    /// 站立总高（米）。胶囊中心高度 = `height / 2`，脚底在 `position` 处。
    pub height: f32,
    /// 可站立的坡度上限（度）。法线与竖直轴夹角超过它就算「站不住」。
    pub slope_limit_deg: f32,
    /// 单帧能抬腿越过的最大台阶（米）。0 表示完全不跨台阶。
    pub step_height: f32,
    /// 下坡时向下吸附的最大距离（米），避免走下缓坡时一格一格地「悬空—落地」抖。
    pub snap_distance: f32,
}

impl Default for CharacterSpec {
    fn default() -> Self {
        Self {
            radius: 0.3,
            height: 1.8,
            slope_limit_deg: 45.0,
            step_height: 0.35,
            snap_distance: 0.1,
        }
    }
}

impl CharacterSpec {
    /// 全默认配置（脚本 `character_create` 不带参数时的形态）。
    pub const fn new() -> Self {
        Self {
            radius: 0.3,
            height: 1.8,
            slope_limit_deg: 45.0,
            step_height: 0.35,
            snap_distance: 0.1,
        }
    }

    /// 胶囊中心相对脚底的高度。
    pub fn center_offset(&self) -> f32 {
        self.height * 0.5
    }

    /// 胶囊的圆柱段半高（总高减去两端半球）。
    ///
    /// 半径比身高一半还大时退化为 0：几何上已经是个球，不再有圆柱段。
    pub fn cylinder_half_height(&self) -> f32 {
        (self.height * 0.5 - self.radius).max(0.0)
    }

    /// 坡度上限的余弦值（判定「站在多斜的面上仍算着地」时直接用）。
    pub fn slope_cos(&self) -> f32 {
        self.slope_limit_deg.to_radians().cos()
    }

    /// 逐字段收敛：非有限值与越界值一律拒绝，不做「悄悄夹到边界」。
    ///
    /// 为什么不夹：夹边界会让「我写的 100 度坡度」变成 89 度然后静默生效，
    /// 排查时只能在步进结果里猜。拒绝则一步到位地把调用方指回参数。
    pub fn validate(&self) -> Result<(), CharacterSpecError> {
        let r = self.radius;
        if !r.is_finite() || !(CHARACTER_MIN_RADIUS..=CHARACTER_MAX_RADIUS).contains(&r) {
            return Err(CharacterSpecError::Radius);
        }
        let h = self.height;
        if !h.is_finite() || !(CHARACTER_MIN_HEIGHT..=CHARACTER_MAX_HEIGHT).contains(&h) {
            return Err(CharacterSpecError::Height);
        }
        let s = self.slope_limit_deg;
        if !s.is_finite() || !(0.0..=CHARACTER_MAX_SLOPE_DEG).contains(&s) {
            return Err(CharacterSpecError::Slope);
        }
        let st = self.step_height;
        if !st.is_finite() || !(0.0..=CHARACTER_MAX_STEP_HEIGHT).contains(&st) {
            return Err(CharacterSpecError::StepHeight);
        }
        let sn = self.snap_distance;
        if !sn.is_finite() || !(0.0..=CHARACTER_MAX_SNAP_DISTANCE).contains(&sn) {
            return Err(CharacterSpecError::SnapDistance);
        }
        if st > h {
            return Err(CharacterSpecError::StepTallerThanBody);
        }
        if sn > h {
            return Err(CharacterSpecError::SnapLongerThanBody);
        }
        Ok(())
    }

    /// `validate` 的值形态：过了原样返回，没过给出原因。
    pub fn validated(self) -> Result<Self, CharacterSpecError> {
        self.validate().map(|()| self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认值自洽() {
        let s = CharacterSpec::default();
        assert!(s.validate().is_ok(), "默认配置必须合法");
        assert!((s.center_offset() - 0.9).abs() < 1e-6);
        assert!((s.cylinder_half_height() - 0.6).abs() < 1e-6);
        assert!(
            s.slope_cos() > 0.7 && s.slope_cos() < 0.71,
            "45° 余弦约 0.7071"
        );
    }

    #[test]
    fn 半径越界被拒() {
        let mut s = CharacterSpec {
            radius: 0.0,
            ..Default::default()
        };
        assert_eq!(s.validate(), Err(CharacterSpecError::Radius));
        s.radius = f32::NAN;
        assert_eq!(s.validate(), Err(CharacterSpecError::Radius));
    }

    #[test]
    fn 身高越界被拒() {
        let mut s = CharacterSpec {
            height: 9.0,
            ..Default::default()
        };
        assert_eq!(s.validate(), Err(CharacterSpecError::Height));
        s.height = 0.0;
        assert_eq!(s.validate(), Err(CharacterSpecError::Height));
    }

    #[test]
    fn 坡度越界被拒() {
        let mut s = CharacterSpec {
            slope_limit_deg: 90.0,
            ..Default::default()
        };
        assert_eq!(s.validate(), Err(CharacterSpecError::Slope));
        s.slope_limit_deg = -1.0;
        assert_eq!(s.validate(), Err(CharacterSpecError::Slope));
    }

    #[test]
    fn 台阶不得高过身高() {
        let s = CharacterSpec {
            step_height: 1.0,
            height: 0.9,
            ..Default::default()
        };
        assert_eq!(s.validate(), Err(CharacterSpecError::StepTallerThanBody));
    }

    #[test]
    fn 吸附不得长过身高() {
        let s = CharacterSpec {
            snap_distance: 1.5,
            height: 1.2,
            ..Default::default()
        };
        assert_eq!(s.validate(), Err(CharacterSpecError::SnapLongerThanBody));
    }

    #[test]
    fn 矮个子半径大于半身时圆柱段归零() {
        let s = CharacterSpec {
            radius: 0.4,
            height: 0.5,
            slope_limit_deg: 30.0,
            step_height: 0.1,
            snap_distance: 0.05,
        };
        assert_eq!(s.cylinder_half_height(), 0.0);
    }
}
