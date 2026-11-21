//! 角色状态：脚底位置 + 竖直速度 + 落地状态。
//!
//! 这是 B 系列里唯一「可被存档」的角色数据。设计上有两条硬约束：
//!
//! 1. **脚底是真值**：`feet` 而不是胶囊中心。理由见 crate 文档。
//! 2. **落地状态只能由求解器写**：宿主可以 `teleport`（一次性重置），
//!    但不该手改 `grounded` —— 那种「看起来站住了」的假状态会让跳跃判据放行，
//!    症状是「空中二段跳」，且只在特定地图上复现。

use glam::Vec3;
use r3e_physics::port::{Capsule, MoveOutcome};

use crate::config::CharacterConfig;

/// 角色的运行期状态。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterState {
    /// 脚底世界坐标（角色真值）。
    pub feet: Vec3,
    /// 竖直速度（米/秒，向上为正）。水平速度不进状态：它由输入意图每帧给出。
    pub velocity_y: f32,
    /// 上一帧是否踩实。
    pub grounded: bool,
    /// 踩实处的表面法线；没踩实时为 [`Vec3::ZERO`]。
    pub ground_normal: Vec3,
    /// 上一帧水平移动是否被挡住。
    pub blocked: bool,
    /// 上一帧靠台阶抬升吃掉的高度差。
    pub step_up: f32,
}

impl CharacterState {
    /// 从脚底位置建一个「静止、悬空、无历史」的状态。
    ///
    /// `grounded` 初始为 `false`：新角色第一帧必须自己踩一次地，
    /// 不能凭「看起来在地面上」就直接给一个跳跃许可。
    pub fn new(feet: Vec3) -> Self {
        Self {
            feet,
            velocity_y: 0.0,
            grounded: false,
            ground_normal: Vec3::ZERO,
            blocked: false,
            step_up: 0.0,
        }
    }

    /// 把角色瞬移到某个脚底位置：速度与落地状态一并清空。
    ///
    /// 保留旧速度的「传送」会让角色带着下坠速度出现在目标点，
    /// 落地判定还没跑就已经位移一截，观感与断言都会漂。
    pub fn teleport(&mut self, feet: Vec3) {
        self.feet = feet;
        self.velocity_y = 0.0;
        self.grounded = false;
        self.ground_normal = Vec3::ZERO;
        self.blocked = false;
        self.step_up = 0.0;
    }

    /// 当前胶囊中心（脚底 + height/2）。
    pub fn center(&self, config: &CharacterConfig) -> Vec3 {
        config.center(self.feet)
    }

    /// 当前查询胶囊。
    pub fn capsule(&self, config: &CharacterConfig) -> Capsule {
        config.capsule(self.center(config))
    }

    /// 用一次求解结果推进状态。
    ///
    /// 只改「位置 + 落地相关」四个字段，**不动 `velocity_y`**：
    /// 速度的积分与清零属于求解器的职责（撞头要清零、落地要清零、
    /// 起跳要置正），混在这里会让「为什么我的角色跳不起来」变成猜谜。
    pub fn apply(&mut self, config: &CharacterConfig, outcome: &MoveOutcome) {
        self.feet = config.feet(outcome.position);
        self.grounded = outcome.grounded;
        self.ground_normal = outcome.ground_normal;
        self.blocked = outcome.blocked;
        self.step_up = outcome.step_up;
    }
}
