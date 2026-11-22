//! `CharacterHost` 的**实现**（M2-B01）：把 [`KinematicSolver`] 接成脚本 / 宿主侧能力面。
//!
//! 能力 trait 住在 `r3e-core::host`（公共契约层），本文件是唯一实现点：
//! 「场景配置 → 控制器」「意图 → 一帧位移」「属性面板 → 运行态快照」三处转换
//! 全部集中在这一个 `match` 里，`r3e-script` 因此不必认识 `CharacterConfig` /
//! `CharacterState` / `Physics` 等本 crate 与 physics 的类型。
//!
//! 三条纪律（与 crate 文档的边界一致，改之前先读）：
//!
//! 1. **本文件不写场景**：`step` 只回新的脚底位置，写回 `Transform` 是宿主的活
//!    （`r3e-editor-host` / `r3e-runtime`）。控制器一旦自己写场景，
//!    「拖拽角色后它自己弹回去」这类问题的责任就说不清了。
//! 2. **配置以场景为真值**：每次 `bind` / `step` 都比对 `Node::character`，
//!    不一致就按场景刷新 `CharacterConfig`，但**保留运行态**——编辑器里改个身高
//!    不该把正在跑的角色传回出发点。
//! 3. **只认端口、不认门面**：物理以 `Rc<dyn PhysicsQueryPort>` 注入。装配层手里
//!    那个共享槽位由 `r3e-physics` 侧 `impl PhysicsQueryPort for RefCell<Physics>`
//!    接成端口，本 crate 因此不认识 `Physics` 具体类型（`doc-gate` 规则 8 看守）。

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use glam::Vec3;
use r3e_core::host::{CharReq, CharRsp, CharacterHost, CharacterStateDto, CharacterStatsDto};
use r3e_core::{CharacterSpec, Node};
use r3e_physics::port::{MoveIntent, PhysicsQueryPort};

use crate::config::CharacterConfig;
use crate::contract::CharacterSolver;
use crate::state::CharacterState;
use crate::step::KinematicSolver;

/// 后端名：如实告诉调用方「接的是运动学求解器」。
const BACKEND: &str = "kinematic";

/// 场景配置 → 求解器配置。
///
/// 运动常量（速度 / 跳跃 / 重力）暂用 [`CharacterConfig::default`]：
/// B 系列只让几何参数可配置，把速度也放开会让「同一场景两次回放差一格」
/// 变成常态，先收在常量里（后续卡若要做参数化，从这里加字段）。
fn config_of(spec: &CharacterSpec) -> CharacterConfig {
    CharacterConfig {
        radius: spec.radius,
        height: spec.height,
        max_slope_deg: spec.slope_limit_deg,
        step_height: spec.step_height,
        snap_distance: spec.snap_distance,
        ..CharacterConfig::default()
    }
}

/// 节点的出生脚底坐标。
///
/// `world` 是**缓存矩阵**：调用方没跑过 `update_world_transforms` 时它是
/// `IDENTITY`，`world_position()` 会返回原点 —— 症状是「角色永远出生在世界原点」，
/// 而且只在「先建角色、后刷新场景」的顺序下复现。所以这里做一次保守回退：
/// 根节点、或缓存明显没刷过（世界平移为 0、本地不为 0）时用本地位置。
fn feet_of(node: &Node) -> Vec3 {
    let local = node.transform.position;
    let world = node.world_position();
    if node.parent.is_none() || (world == Vec3::ZERO && local != Vec3::ZERO) {
        local
    } else {
        world
    }
}

/// 一个角色的控制器：配置 + 运行态。
struct Ctrl {
    /// 场景里那份配置的副本，用来判「编辑器改过没有」。
    spec: CharacterSpec,
    config: CharacterConfig,
    state: CharacterState,
}

/// 累计统计与全部控制器。
#[derive(Default)]
struct Inner {
    ctrls: BTreeMap<String, Ctrl>,
    steps: u64,
    blocked: u64,
    step_ups: u64,
}

impl Inner {
    fn snapshot(ctrl: &Ctrl) -> CharacterStateDto {
        CharacterStateDto {
            position: [
                ctrl.state.feet.x as f64,
                ctrl.state.feet.y as f64,
                ctrl.state.feet.z as f64,
            ],
            vertical_speed: ctrl.state.velocity_y as f64,
            grounded: ctrl.state.grounded,
            blocked: ctrl.state.blocked,
        }
    }
}

/// 角色控制器门面：宿主与脚本共用的那一个「角色系统」。
///
/// 与 `r3e-physics` 的 `PhysicsFacade` 同一套路：与真门面共享同一个后端，
/// `Rc<RefCell<_>>` 里放运行态，`CharacterHost` 的方法全是 `&self`，
/// 所以内部可变性是必需的。物理侧只以端口形态持有，见文件头第 3 条。
pub struct CharacterRuntime {
    physics: Rc<dyn PhysicsQueryPort>,
    solver: KinematicSolver,
    inner: RefCell<Inner>,
}

impl CharacterRuntime {
    /// 用共享的物理槽位建一个角色系统。
    ///
    /// 装配层把 `Rc<RefCell<Physics>>` 直接传进来即可：`r3e-physics` 为
    /// `RefCell<Physics>` 实现了端口 trait，`Rc<P>` 到 `Rc<dyn PhysicsQueryPort>`
    /// 是自动强制转换。泛型 `P` 只受端口 trait 约束，本 crate 因此仍不认识
    /// `Physics` 具体类型。
    pub fn new<P: PhysicsQueryPort + 'static>(physics: Rc<P>) -> Self {
        let physics: Rc<dyn PhysicsQueryPort> = physics;
        Self {
            physics,
            solver: KinematicSolver,
            inner: RefCell::new(Inner::default()),
        }
    }

    /// 包成能力面。返回 `Rc<dyn CharacterHost>`，装配层只需要 trait。
    ///
    /// 名字不叫 `new`：返回的不是 `Self`，`clippy::new_ret_no_self` 会拦。
    pub fn wrap<P: PhysicsQueryPort + 'static>(physics: Rc<P>) -> Rc<dyn CharacterHost> {
        Rc::new(Self::new(physics))
    }

    /// 当前登记在册的角色数量（诊断用；`stats()` 也能拿到）。
    pub fn controller_count(&self) -> usize {
        self.inner.borrow().ctrls.len()
    }

    /// 按节点配置建立/刷新控制器。返回是否成功建立。
    fn bind(&self, node: &Node) -> bool {
        let Some(spec) = node.character else {
            return false;
        };
        // 非法配置不建控制器：宁可在编辑器里看到「建不起来」，
        // 也不要让一个半径为 0 的角色每帧算出 NaN 后污染整帧。
        if spec.validate().is_err() {
            return false;
        }
        let mut inner = self.inner.borrow_mut();
        match inner.ctrls.get_mut(&node.name) {
            // 幂等：重复 bind 只刷新配置，位置与速度不动。
            Some(ctrl) => {
                ctrl.spec = spec;
                ctrl.config = config_of(&spec);
            }
            None => {
                inner.ctrls.insert(
                    node.name.clone(),
                    Ctrl {
                        spec,
                        config: config_of(&spec),
                        // 初始脚底取节点的世界位置（缓存没刷新时回退本地位置，
                        // 理由见 [`feet_of`]）。
                        state: CharacterState::new(feet_of(node)),
                    },
                );
            }
        }
        true
    }

    /// 推进一帧；没有控制器返回 `None`（调用方据此提示「先 create」）。
    fn step(&self, node: &Node, wish: [f64; 2], jump: bool, dt: f64) -> Option<CharacterStateDto> {
        let mut inner = self.inner.borrow_mut();

        // 控制器只在这一块里可变借用：借完就把快照拿出来，统计字段才能再借一次。
        let (snapshot, blocked, step_up) = {
            let ctrl = inner.ctrls.get_mut(&node.name)?;

            // 配置漂移：以场景为准刷新，保留运行态。
            if let Some(spec) = node.character {
                if spec != ctrl.spec && spec.validate().is_ok() {
                    ctrl.spec = spec;
                    ctrl.config = config_of(&spec);
                }
            }

            let intent = MoveIntent {
                wish_dir: Vec3::new(wish[0] as f32, 0.0, wish[1] as f32),
                jump,
            };
            let port: &dyn PhysicsQueryPort = &*self.physics;
            // TEMP-DIAG（定位真窗口不贴地，定位后删除）
            {
                use std::cell::Cell;
                thread_local!(static N: Cell<u32> = const { Cell::new(0) });
                N.with(|n| {
                    let i = n.get();
                    if i < 5 {
                        n.set(i + 1);
                        let f = ctrl.state.feet;
                        let g = r3e_physics::port::GroundPort::sample_ground(port, f.x, f.z);
                        eprintln!(
                            "[diag] i={i} feet={:?} vy={} ground_some={}",
                            f,
                            ctrl.state.velocity_y,
                            g.is_some()
                        );
                    }
                });
            }
            let outcome = self
                .solver
                .step(&ctrl.config, &mut ctrl.state, intent, dt as f32, port);
            (Inner::snapshot(ctrl), outcome.blocked, outcome.step_up)
        };

        // 位置与落地状态已由求解器写回 `CharacterState`，这里只补统计。
        inner.steps += 1;
        if blocked {
            inner.blocked += 1;
        }
        if step_up > 0.0 {
            inner.step_ups += 1;
        }
        Some(snapshot)
    }
}

impl CharacterHost for CharacterRuntime {
    fn execute(&self, req: CharReq<'_>) -> CharRsp {
        match req {
            CharReq::Bind { node } => CharRsp::Bool(self.bind(node)),
            CharReq::Unbind { name } => {
                CharRsp::Bool(self.inner.borrow_mut().ctrls.remove(name).is_some())
            }
            CharReq::Step {
                node,
                wish,
                jump,
                dt,
            } => CharRsp::State(self.step(node, wish, jump, dt)),
            CharReq::State { name } => {
                let inner = self.inner.borrow();
                CharRsp::State(inner.ctrls.get(name).map(Inner::snapshot))
            }
            CharReq::Teleport { name, position } => {
                let mut inner = self.inner.borrow_mut();
                match inner.ctrls.get_mut(name) {
                    Some(ctrl) => {
                        // 与 `CharacterState::teleport` 同义：速度与落地状态一并清零，
                        // 否则「传送到空中」会带着上一段的下坠速度立刻再落一截。
                        ctrl.state.teleport(Vec3::new(
                            position[0] as f32,
                            position[1] as f32,
                            position[2] as f32,
                        ));
                        CharRsp::Bool(true)
                    }
                    None => CharRsp::Bool(false),
                }
            }
            CharReq::Stats => {
                let inner = self.inner.borrow();
                CharRsp::Stats(CharacterStatsDto {
                    controllers: inner.ctrls.len() as u64,
                    steps: inner.steps,
                    blocked: inner.blocked,
                    step_ups: inner.step_ups,
                })
            }
            CharReq::Backend => CharRsp::Text(BACKEND.to_string()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空端口：本文件的用例只验「配置 / 运行态 / 统计」的搬运，不验几何，
    /// 所以后端一律「什么都扫不到」——这恰好是「站不住但也不崩」的最坏情况。
    struct NullPort;

    impl r3e_physics::port::RaycastPort for NullPort {
        fn cast_ray(
            &self,
            _ray: &r3e_physics::port::CastRay,
        ) -> Option<r3e_physics::port::PortHit> {
            None
        }
    }

    impl r3e_physics::port::CapsulePort for NullPort {
        fn capsule_overlap(
            &self,
            _c: &r3e_physics::port::Capsule,
        ) -> Vec<r3e_physics::port::PortHit> {
            Vec::new()
        }

        fn capsule_sweep(
            &self,
            _c: &r3e_physics::port::Capsule,
            _dir: glam::Vec3,
            _max: f32,
        ) -> Option<r3e_physics::port::PortHit> {
            None
        }
    }

    impl r3e_physics::port::GroundPort for NullPort {
        fn sample_ground(&self, _x: f32, _z: f32) -> Option<r3e_physics::port::GroundSample> {
            None
        }
    }

    impl r3e_physics::port::PhysicsQueryPort for NullPort {
        fn port_caps(&self) -> r3e_physics::port::PortCaps {
            r3e_physics::port::PortCaps::NONE
        }
    }

    fn runtime() -> CharacterRuntime {
        CharacterRuntime::new(Rc::new(NullPort))
    }

    fn hero(spec: CharacterSpec) -> Node {
        let mut n = Node::new(1, "hero");
        n.character = Some(spec);
        n
    }

    /// 没配置的节点建不起来；配置合法的能建，且起步位置取节点世界坐标。
    #[test]
    fn bind_requires_valid_spec() {
        let rt = runtime();
        let plain = Node::new(2, "box");
        assert!(!rt.bind(&plain));
        assert_eq!(rt.controller_count(), 0);

        let mut node = hero(CharacterSpec::new());
        node.transform.position = glam::Vec3::new(1.0, 2.0, 3.0);
        assert!(rt.bind(&node));
        assert_eq!(rt.controller_count(), 1);
        let st = rt.state("hero").expect("控制器已建立");
        assert_eq!(st.position, [1.0, 2.0, 3.0]);
        // 新角色第一帧必须自己踩一次地，不能凭「看起来在地面上」直接给跳跃许可。
        assert!(!st.grounded);
    }

    /// 重复 bind 幂等：只刷新配置，不重置位置。
    #[test]
    fn bind_is_idempotent_and_keeps_state() {
        let rt = runtime();
        let node = hero(CharacterSpec::new());
        assert!(rt.bind(&node));
        assert!(rt.teleport("hero", [5.0, 0.0, 0.0]));

        let mut taller = CharacterSpec::new();
        taller.height = 1.2;
        let node2 = hero(taller);
        assert!(rt.bind(&node2));
        assert_eq!(rt.controller_count(), 1);
        assert_eq!(rt.state("hero").unwrap().position, [5.0, 0.0, 0.0]);
    }

    /// 未 bind 就 step：回 `None`，不 panic、不偷偷建一个。
    #[test]
    fn step_without_controller_is_none() {
        let rt = runtime();
        let node = hero(CharacterSpec::new());
        assert!(rt.step(&node, [1.0, 0.0], false, 1.0 / 60.0).is_none());
        assert_eq!(rt.stats().steps, 0);
    }

    /// 推进一帧会计数；dt 非法时求解器原地不动，但仍算一次调用。
    #[test]
    fn step_counts_and_rejects_bad_dt() {
        let rt = runtime();
        let node = hero(CharacterSpec::new());
        assert!(rt.bind(&node));
        let before = rt.state("hero").unwrap().position;
        let st = rt.step(&node, [1.0, 0.0], false, 0.0).expect("已 bind");
        assert_eq!(st.position, before);
        assert_eq!(rt.stats().steps, 1);
    }

    /// 传送与解绑：传送清速度，解绑后一切查询回「没有」。
    #[test]
    fn teleport_and_unbind() {
        let rt = runtime();
        let node = hero(CharacterSpec::new());
        assert!(rt.bind(&node));
        assert!(rt.teleport("hero", [0.0, 4.0, 0.0]));
        let st = rt.state("hero").unwrap();
        assert_eq!(st.position, [0.0, 4.0, 0.0]);
        assert_eq!(st.vertical_speed, 0.0);
        assert!(!rt.teleport("nobody", [0.0; 3]));

        assert!(rt.unbind("hero"));
        assert!(!rt.unbind("hero"));
        assert!(rt.state("hero").is_none());
    }

    /// 后端名如实带出：统计全 0 时也得能分清「没动过」与「没接」。
    #[test]
    fn backend_is_reported() {
        let rt = runtime();
        assert_eq!(rt.backend(), "kinematic");
        assert_eq!(rt.stats().controllers, 0);
    }
}
