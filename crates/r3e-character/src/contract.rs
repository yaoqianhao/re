//! 求解器契约：换算法不动调用方。
//!
//! B01 只交付运动学一套（[`KinematicSolver`](crate::KinematicSolver)），但契约先立好，
//! 后面要接「带惯性的动力学角色」或「回放里的确定性角色」时，
//! 宿主与脚本侧的调用代码一行都不用改。
//!
//! 契约故意做成 `&dyn PhysicsQueryPort` 而不是泛型参数：
//! 泛型会让每个求解器都被单态化进宿主二进制，且 trait object 才能放进
//! `Box<dyn CharacterSolver>` 让宿主按配置切算法。

use crate::config::CharacterConfig;
use crate::state::CharacterState;
use r3e_physics::port::{MoveIntent, MoveOutcome, PhysicsQueryPort};

/// 单帧角色求解器。
///
/// 实现方**必须**遵守的三条约定（B02 的判据就按这三条写）：
///
/// 1. 只经 `port` 读几何，不持有查询端口的生命周期，也不缓存命中结果。
/// 2. 结果通过返回值给出，同时把「位置 / 落地状态」写回 `state`；
///    不得只写回而不返回（宿主拿返回值做存档与断言）。
/// 3. `dt <= 0` 或非有限时按「本帧不动」处理并返回 `state` 现状，
///    而不是 panic 或返回原点。
pub trait CharacterSolver {
    /// 推进一帧：输入意图 + 上一帧状态 → 新状态与新结果。
    ///
    /// `state` 既要读（上一帧脚底与速度）也要写（本帧结果），
    /// 因此这里是 `&mut`；求解器本身保持无状态（`&self`），
    /// 这样同一套配置下多角色共用一个求解器实例是安全的。
    fn step(
        &self,
        config: &CharacterConfig,
        state: &mut CharacterState,
        intent: MoveIntent,
        dt: f32,
        port: &dyn PhysicsQueryPort,
    ) -> MoveOutcome;
}
