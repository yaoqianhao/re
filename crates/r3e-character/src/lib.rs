//! 角色控制器（M2 · B01）：把「输入意图 + 几何查询」算成「这一帧该站在哪」。
//!
//! ## 边界：这里不写场景
//!
//! 本 crate **不持有** `Document` / `Scene`，也不认 `Node`。输入是配置、状态与意图
//! （`CharacterConfig` + `CharacterState` + `MoveIntent`），输出是 `MoveOutcome`
//! （胶囊中心 + 落地状态）。谁把结果写回场景、写回哪个节点，由宿主
//! （`r3e-editor-host` / `r3e-runtime`）决定。这条边界是 B 系列的第一条纪律：
//! 控制器一旦会写场景，就没法在无场景的单元测试里复现「台阶边缘抖动」这类问题。
//!
//! ## 边界：这里不认物理后端
//!
//! 唯一的几何入口是 [`PhysicsQueryPort`]（`r3e-physics::port`）。本 crate 只依赖
//! `r3e-core`（`NodeId` 等纯数据类型）与 `r3e-physics`（端口 trait），
//! `xtask/dep_rules.json` 的白名单按此登记 —— 反向依赖（physics → character）不存在，
//! 端口与本 crate 的依赖方向始终保持单向。
//!
//! ## 三个文件各管什么
//!
//! | 文件 | 职责 |
//! | --- | --- |
//! | [`config`] | 尺寸 / 坡度 / 台阶 / 吸附的**参数与校验**，非法参数在这里就被拒 |
//! | [`state`] | 角色**唯一真值**：脚底位置 + 竖直速度 + 落地状态 |
//! | [`contract`] | 求解器 trait：换算法（运动学 / 动力学 / 回放）不动调用方 |
//! | [`step`] | 运动学求解器的单帧实现 |
//!
//! ## 为什么脚底是真值
//!
//! 对外读写都是**脚底**（`feet`）：关卡作者量台阶、地编摆角色、存档都按「站在多高」算，
//! 没人会说「枢轴在 1.35 米处」。胶囊中心只是几何量，由 [`CharacterConfig::center`]
//! 单点换算 —— 换算写两处，蹲下改高度时迟早出现「人陷进地里」。

mod config;
mod contract;
mod host_adapter;
mod state;
mod step;

pub use config::{CharacterConfig, ConfigError};
pub use contract::CharacterSolver;
pub use host_adapter::CharacterRuntime;
pub use r3e_physics::port::{MoveIntent, MoveOutcome, PhysicsQueryPort};
pub use state::CharacterState;
pub use step::KinematicSolver;

/// 控制器的额定步长：60 Hz 固定步（与宿主主循环同频）。
///
/// 物理侧积分若改成子步，也应保持「每子步 ≤ 这个值」，
/// 否则水平扫掠单步跨过薄墙（B02 的穿墙判据就是按这个步长定的）。
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// 单帧步长硬上限：卡顿后的一帧不许把角色甩到半个地图外。
///
/// 裁剪而不是拒绝：拒绝会让角色在掉帧时**静止**，比多走一点更难受；
/// 裁剪后竖直方向同样按裁剪值积分，落地判据不会被「一帧掉进地里」污染。
pub const MAX_DT: f32 = 0.1;
