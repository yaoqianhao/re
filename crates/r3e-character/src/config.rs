//! 角色参数：尺寸、坡度、台阶、吸附与运动常量。
//!
//! 参数在这里**一次校验、处处可信**：`step` 不重复判 `radius > 0`，
//! 宿主拿到 `config.validate()?` 之后就可以当真值用。
//!
//! 尺寸口径（全篇一致）：
//!
//! ```text
//!               ┬  center + height/2      ← 头顶（脚底 + height）
//!               │  center + half_height   ← 圆柱段上端（+ 半球）
//!             center                      ← 胶囊中心 = 脚底 + height/2
//!               │  center - half_height   ← 圆柱段下端（+ 半球）
//!               ┴  center - height/2      ← 脚底 feet（角色真值）
//!
//!   half_height = height/2 - radius   （圆柱段半高，不含两端半球）
//!   height      = 2 * (half_height + radius)
//! ```

use std::fmt;

use glam::Vec3;
use r3e_physics::port::Capsule;

/// 一套角色参数：尺寸 + 可站立条件 + 运动常量。
///
/// 全部字段都是 `pub` 且 `Copy`：调参不该需要 builder，也不该逼调用方 clone。
/// 合法性由 [`CharacterConfig::validate`] 统一裁决 —— 构造结构体本身不拦，
/// 是为了让配置文件反序列化出来的中间态也能先构造再校验。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterConfig {
    /// 胶囊半径（米）。站姿与蹲姿共用，蹲下只改 [`Self::height`]。
    pub radius: f32,
    /// 站立总高（米，含两端半球）。必须 ≥ `2 * radius`。
    pub height: f32,
    /// 可站立坡度上限（度，0..90）。`90` 表示墙也算「可站」。
    pub max_slope_deg: f32,
    /// 台阶抬升上限（米）。`0` 表示不做台阶试探（跨不上任何台阶）。
    pub step_height: f32,
    /// 落地吸附距离（米）。下楼梯 / 走下缓坡时向下补探的最大距离。
    pub snap_distance: f32,
    /// 期望水平速度（米/秒）。控制器把「方向 × dt」换算成扫掠距离。
    pub move_speed: f32,
    /// 起跳初速度（米/秒，竖直向上）。
    pub jump_speed: f32,
    /// 重力加速度（米/秒²，负值向下）。
    pub gravity: f32,
}

impl Default for CharacterConfig {
    /// 一个「人形、能在 45° 坡上站住、能上一层 35 cm 台阶」的默认值。
    ///
    /// 数值取整不是随手写的：`step_height` 与 `snap_distance` 都小于默认半径，
    /// 保证「抬升试探」与「落地吸附」不会把角色直接推到障碍顶上或穿到地面下。
    fn default() -> Self {
        Self {
            radius: 0.35,
            height: 1.8,
            max_slope_deg: 45.0,
            step_height: 0.35,
            snap_distance: 0.4,
            move_speed: 4.5,
            jump_speed: 6.0,
            gravity: -20.0,
        }
    }
}

/// 参数非法的唯一原因类型。
///
/// 带字段名而不是只带值：配置文件的报错必须能让人一眼找到那一行。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigError {
    /// 字段不是有限数（NaN / ±∞）。
    NonFinite { field: &'static str },
    /// 字段必须严格为正，实际给了非正数。
    NonPositive { field: &'static str, value: f32 },
    /// 总高不足以容纳两端半球。
    HeightTooSmall { height: f32, min: f32 },
    /// 坡度上限超出 `(0, 90]` 度。
    SlopeOutOfRange { deg: f32 },
    /// 重力为零或向上：控制器不处理「失重 / 反重力」，那是另一个玩法。
    GravityNotDownward { value: f32 },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite { field } => write!(f, "角色参数 `{field}` 必须是有限数"),
            Self::NonPositive { field, value } => {
                write!(f, "角色参数 `{field}` 必须 > 0，实际为 {value}")
            }
            Self::HeightTooSmall { height, min } => write!(
                f,
                "角色总高 {height} 小于两端半球所需的最小高度 {min}（2 × radius）"
            ),
            Self::SlopeOutOfRange { deg } => {
                write!(f, "角色坡度上限必须在 (0, 90] 度内，实际为 {deg}")
            }
            Self::GravityNotDownward { value } => {
                write!(f, "角色重力必须为负值（向下），实际为 {value}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl CharacterConfig {
    /// 校验一套参数；所有字段都过一遍，不做「查到一个就返回」的短路。
    ///
    /// 不短路是有意为之：配置文件写错三处时，一次报全比改一处跑一次快得多。
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("radius", self.radius),
            ("height", self.height),
            ("max_slope_deg", self.max_slope_deg),
            ("step_height", self.step_height),
            ("snap_distance", self.snap_distance),
            ("move_speed", self.move_speed),
            ("jump_speed", self.jump_speed),
            ("gravity", self.gravity),
        ] {
            if !value.is_finite() {
                return Err(ConfigError::NonFinite { field });
            }
        }
        for (field, value) in [
            ("radius", self.radius),
            ("height", self.height),
            ("max_slope_deg", self.max_slope_deg),
            ("move_speed", self.move_speed),
        ] {
            if value <= 0.0 {
                return Err(ConfigError::NonPositive { field, value });
            }
        }
        // 台阶与吸附允许为 0（= 关闭该行为），只拦负数。
        for (field, value) in [
            ("step_height", self.step_height),
            ("snap_distance", self.snap_distance),
        ] {
            if value < 0.0 {
                return Err(ConfigError::NonPositive { field, value });
            }
        }
        if self.jump_speed < 0.0 {
            return Err(ConfigError::NonPositive {
                field: "jump_speed",
                value: self.jump_speed,
            });
        }
        if self.gravity >= 0.0 {
            return Err(ConfigError::GravityNotDownward {
                value: self.gravity,
            });
        }
        if self.max_slope_deg > 90.0 {
            return Err(ConfigError::SlopeOutOfRange {
                deg: self.max_slope_deg,
            });
        }
        let min_height = 2.0 * self.radius;
        if self.height < min_height {
            return Err(ConfigError::HeightTooSmall {
                height: self.height,
                min: min_height,
            });
        }
        Ok(())
    }

    /// 圆柱段半高（不含两端半球）。
    ///
    /// `validate` 之后恒非负；未校验的配置这里用 `max(0.0)` 兜底，
    /// 免得一个手滑的 `height` 让胶囊变成「反向胶囊」把扫掠算成负距离。
    pub fn half_height(&self) -> f32 {
        (self.height * 0.5 - self.radius).max(0.0)
    }

    /// 脚底 → 胶囊中心。
    pub fn center(&self, feet: Vec3) -> Vec3 {
        feet + Vec3::Y * (self.height * 0.5)
    }

    /// 胶囊中心 → 脚底（[`Self::center`] 的逆）。
    pub fn feet(&self, center: Vec3) -> Vec3 {
        center - Vec3::Y * (self.height * 0.5)
    }

    /// 以中心点构造查询胶囊。
    pub fn capsule(&self, center: Vec3) -> Capsule {
        Capsule::new(center, self.half_height(), self.radius)
    }

    /// 可站立判据的 `normal.y` 阈值（即 `cos(max_slope_deg)`）。
    pub fn min_up_dot(&self) -> f32 {
        self.max_slope_deg.to_radians().cos()
    }

    /// 这个法线算不算「站得住的地面」。
    ///
    /// 零法线一律判否：它是「没查到地面」的哨兵值，不是「垂直墙」。
    pub fn is_walkable(&self, normal: Vec3) -> bool {
        let n = normal.normalize_or_zero();
        n.length_squared() > 0.0 && n.y >= self.min_up_dot()
    }
}
