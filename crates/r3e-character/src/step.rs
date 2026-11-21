//! 运动学求解器：一帧五步，顺序不能换。
//!
//! ```text
//!   ① 竖直速度积分（起跳 / 重力）
//!   ② 水平扫掠（直扫 → 台阶试探 → 贴墙停下）
//!   ③ 竖直结算（上升顶上 / 下落扫掠 / 吸附）
//!   ④ 撞头抽速
//!   ⑤ 坡度裁决（站不住的地面不算 grounded）
//! ```
//!
//! 顺序为什么是这样：先积竖直速度，才能让「落地当帧起跳」用上本帧的重力；
//! 先水平再竖直，台阶抬升才会在**同帧**被落地判据看见 —— 反过来会出现
//! 「先落地再抬升」，角色一帧悬空、下一帧才贴地，斜坡上表现为持续抖动。

use glam::Vec3;
use r3e_physics::port::{CharacterPort, MoveIntent, MoveOutcome, PhysicsQueryPort};

use crate::config::CharacterConfig;
use crate::contract::CharacterSolver;
use crate::state::CharacterState;

/// 下坠速度上限（米/秒）：防止长距离坠落把单帧位移撑爆扫掠精度。
const TERMINAL_VELOCITY: f32 = -60.0;

/// 撞头判据的容差（米）：本帧上升不足这个量就当「撞到了」。
const HEAD_HIT_EPS: f32 = 1e-4;

/// 运动学角色求解器：无状态、`Copy`，可以按需克隆进多个角色。
#[derive(Debug, Clone, Copy, Default)]
pub struct KinematicSolver;

impl KinematicSolver {
    /// 步长的唯一入口：裁剪 + 非法值归零。
    fn sanitize_dt(dt: f32) -> f32 {
        if !dt.is_finite() || dt <= 0.0 {
            0.0
        } else {
            dt.min(crate::MAX_DT)
        }
    }
}

impl CharacterSolver for KinematicSolver {
    fn step(
        &self,
        config: &CharacterConfig,
        state: &mut CharacterState,
        intent: MoveIntent,
        dt: f32,
        port: &dyn PhysicsQueryPort,
    ) -> MoveOutcome {
        let dt = Self::sanitize_dt(dt);
        if dt == 0.0 {
            // 本帧不动：位置原样返回，落地状态沿用上一次的结论。
            return MoveOutcome {
                position: state.center(config),
                grounded: state.grounded,
                ground_normal: state.ground_normal,
                blocked: false,
                step_up: 0.0,
            };
        }

        // ① 竖直速度：站稳时先清零再跳 / 再积重力，避免「站着一直加速」。
        if state.grounded {
            state.velocity_y = 0.0;
        }
        if state.grounded && intent.jump {
            state.velocity_y = config.jump_speed;
        } else {
            state.velocity_y = (state.velocity_y + config.gravity * dt).max(TERMINAL_VELOCITY);
        }

        // ② 水平扫掠：只有踩实才允许跨台阶，空中抬升等于穿墙。
        let center_before = state.center(config);
        let capsule_before = config.capsule(center_before);
        let wish = Vec3::new(intent.wish_dir.x, 0.0, intent.wish_dir.z);
        let wish_len = wish.length();
        let distance = config.move_speed * dt * wish_len.min(1.0);
        let max_step = if state.grounded {
            config.step_height
        } else {
            0.0
        };
        let (pos_h, blocked, step_up) =
            port.move_horizontal(&capsule_before, wish, distance, max_step);

        // ③ 竖直结算：下落时才吸附，上升中吸附会把角色粘在天花板上。
        let capsule_h = config.capsule(pos_h);
        let snap = if state.velocity_y <= 0.0 {
            config.snap_distance
        } else {
            0.0
        };
        let (pos_v, grounded_raw, normal) =
            port.settle_vertical(&capsule_h, state.velocity_y, dt, snap);

        // ④ 撞头抽速：想上升却没升高，说明头顶是实心的。
        if state.velocity_y > 0.0 && pos_v.y <= center_before.y + HEAD_HIT_EPS {
            state.velocity_y = 0.0;
        }

        // ⑤ 坡度裁决：站不住的面不算落地，否则陡坡上能连跳。
        let grounded = grounded_raw && config.is_walkable(normal);
        let outcome = MoveOutcome {
            position: pos_v,
            grounded,
            ground_normal: if grounded { normal } else { Vec3::ZERO },
            blocked,
            step_up,
        };
        state.apply(config, &outcome);
        if state.grounded {
            // 踩实后立刻清零：否则下坠速度会一路累积，走出平台边缘的第一帧
            // 就带着「已经下落很久」的速度砸下去。
            state.velocity_y = 0.0;
        }
        outcome
    }
}
