//! 物理后端契约：`PhysicsBackend` trait + 能力位构造 + 跨后端共用的择优规则。
//!
//! 后端实现（`builtin_backend` / `rapier_backend`）只实现这里的 trait，不反向依赖门面。

use super::*;

use crate::heightfield::HeightField;

impl BackendCaps {
    /// 什么都不做（`null` 后端）。
    pub const NONE: Self = Self {
        body_body_contacts: false,
        rotational_dynamics: false,
        joints: false,
        ccd: false,
        collision_events: false,
        raycast: false,
        overlap: false,
        shape_cast: false,
    };

    /// 内置后端：只有「刚体 vs 地面」这一条接触规则，无旋转、无事件、无 CCD，
    /// 但查询（射线 / 重叠）用解析法真实可查。
    pub const BASIC: Self = Self {
        body_body_contacts: false,
        rotational_dynamics: false,
        joints: false,
        ccd: false,
        collision_events: false,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 有真实约束求解器的档位：刚体互撞 + 旋转动力学 + 碰撞事件（Rapier 当前落在这一档）。
    pub const CONTACT: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: false,
        ccd: false,
        collision_events: true,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 在 [`BackendCaps::CONTACT`] 之上再点亮 CCD：高速刚体不穿透薄壁。
    ///
    /// 目前只有 Rapier 落在这一档（`ccd_solver` 每帧都参与 `pipeline.step`，
    /// 只差逐刚体的 `enable_ccd`）。
    pub const CONTACT_CCD: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: false,
        ccd: true,
        collision_events: true,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 门面（`PhysicsBackend` trait + 脚本 `physics_*` / `body_*`）当前真正暴露给上层的上限。
    ///
    /// CCD 已补齐（`set_ccd` / `ccd_of` / `physics_set_ccd` / `body_set_ccd`），
    /// 所以这一档允许申报 `ccd`；关节仍没有 trait 方法与脚本入口，
    /// 任何后端都**不得**申报 `joints`，否则脚本通过 `physics_has_capability`
    /// 拿到的是一个无法使用的承诺。
    pub const EXPOSED: Self = Self::CONTACT_CCD;

    /// 完整能力，仅作「等门面补齐关节接口后」的目标档位预留。
    ///
    /// 当前没有任何后端可以返回它：申报能力必须以门面已暴露的接口为准，
    /// 详见 [`BackendCaps::EXPOSED`]。
    pub const FULL: Self = Self {
        body_body_contacts: true,
        rotational_dynamics: true,
        joints: true,
        ccd: true,
        collision_events: true,
        raycast: true,
        overlap: true,
        shape_cast: true,
    };

    /// 申报是否落在门面已暴露的能力范围内（越界即虚报）。
    ///
    /// 只剩 `joints` 一个禁区：CCD 已有完整入口，可以申报。
    pub fn is_within_exposed(&self) -> bool {
        !self.joints
    }

    /// 能力键名（稳定标识，供日志与脚本消费）。
    pub fn enabled_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.body_body_contacts {
            keys.push("body_body");
        }
        if self.rotational_dynamics {
            keys.push("rotation");
        }
        if self.joints {
            keys.push("joints");
        }
        if self.ccd {
            keys.push("ccd");
        }
        if self.collision_events {
            keys.push("events");
        }
        if self.raycast {
            keys.push("raycast");
        }
        if self.overlap {
            keys.push("overlap");
        }
        if self.shape_cast {
            keys.push("shape_cast");
        }
        keys
    }

    /// 逗号分隔的能力摘要；全关时返回 `none`。
    pub fn summary(&self) -> String {
        let keys = self.enabled_keys();
        if keys.is_empty() {
            "none".to_string()
        } else {
            keys.join(",")
        }
    }

    /// 按能力名查询，未知键返回 false。
    pub fn has(&self, key: &str) -> bool {
        self.enabled_keys().contains(&key)
    }
}

/// 物理后端契约。实现者负责积分、碰撞与把结果写回场景。
pub trait PhysicsBackend {
    fn name(&self) -> &'static str;

    /// 为节点创建刚体。返回句柄，写入 `Node::body`。
    fn create_body(&mut self, node: NodeId, desc: BodyDesc) -> BodyHandle;

    fn destroy_body(&mut self, handle: BodyHandle);

    fn apply_impulse(&mut self, handle: BodyHandle, impulse: Vec3);

    fn set_gravity(&mut self, gravity: Vec3);

    /// 推进模拟。
    fn step(&mut self, dt: f32);

    /// 把物理解算结果同步回场景节点（位置 / 旋转）。
    fn sync_to_scene(&mut self, scene: &mut Scene);

    fn body_count(&self) -> usize;

    /// 可选：句柄在本后端是否仍然活着。
    ///
    /// [`Physics::bind_scene`] 的幂等判定全靠它：节点上记着 `body` 只是「上次绑过」的
    /// 痕迹，后端重建（切后端 / 清空刚体）之后同一个数字指向的已经是别人的刚体，
    /// 或者干脆什么都不指。默认 `false` 是有意的——**没自己实现的后端一律按「不知道
    /// 就当没有」处理，宁可重建一次，也不假装复用一条死句柄**。
    fn has_body(&self, _handle: BodyHandle) -> bool {
        false
    }

    /// 可选：返回所有现存句柄，便于整体清空。默认返回 `None`（后端自行管理）。
    fn take_all_handles(&self) -> Option<Vec<BodyHandle>> {
        None
    }

    /// 可选：step 之前从场景回收节点位置。
    /// 内置后端用它识别「脚本直接挪了节点」并唤醒刚体；外部后端可忽略。
    fn pull_from_scene(&mut self, _scene: &Scene) {}

    /// 可选：设置地面平面高度（仅内置后端消费）。
    fn set_ground(&mut self, _ground: f32) {}

    /// 可选：给单个刚体开关 CCD（连续碰撞检测）。
    ///
    /// 返回 `true` 表示真的改到了；不支持 CCD 的后端返回 `false`，
    /// 调用方（脚本 / IDE）据此如实回报，不要假装设置成功。
    fn set_ccd(&mut self, _handle: BodyHandle, _on: bool) -> bool {
        false
    }

    /// 可选：读回某刚体当前的 CCD 状态；不支持的后端返回 `None`。
    fn ccd_of(&self, _handle: BodyHandle) -> Option<bool> {
        None
    }

    /// 可选：设置「刚体没显式指定时」的 CCD 默认值（`BodyDesc::continuous = None`）。
    ///
    /// 不需要重建刚体：已有刚体由脚本 / 上层用 [`PhysicsBackend::set_ccd`] 逐个改。
    fn set_ccd_default(&mut self, _on: bool) {}

    /// 可选：当前 CCD 默认值。不支持 CCD 的后端恒为 `false`。
    fn ccd_default(&self) -> bool {
        false
    }

    /// 可选：如实申报本后端能做什么。默认「什么都不做」。
    ///
    /// 这是「可切换性」的关键位：上层按能力分支，不要按 `name()` 分支。
    fn capabilities(&self) -> BackendCaps {
        BackendCaps::NONE
    }

    /// 可选：取走自上次调用以来的碰撞事件（不清空刚体）。
    /// 默认后端不产生事件；支持事件的后端需在 `capabilities().collision_events` 里如实置位。
    fn drain_events(&mut self) -> Vec<PhysicsEvent> {
        Vec::new()
    }

    /// 可选：射线查询，返回 `max_distance` 内最近的命中体。
    ///
    /// 默认实现返回 `None`；能查的后端必须同时把 `capabilities().raycast` 置真，
    /// 否则脚本会拿到「申报能查、实际查不到」的假承诺。
    fn raycast(&self, _ray: Ray, _max_distance: f32) -> Option<RayHit> {
        None
    }

    /// 可选：球体重叠查询，返回球心 `center`、半径 `radius` 内相交的刚体。
    ///
    /// 结果按 `node` 升序，保证同一场景下多次查询顺序稳定。
    /// 默认实现返回空表；能查的后端需把 `capabilities().overlap` 置真。
    fn overlap_sphere(&self, _center: Vec3, _radius: f32) -> Vec<OverlapHit> {
        Vec::new()
    }

    /// 可选：扫掠查询（shape-cast），把 `shape` 从 `origin` 沿 `dir` 推 `max_distance`，
    /// 返回最近的一次接触。
    ///
    /// 三者（raycast / overlap / shape_cast）共用一套口径：`dir` 会归一化、
    /// `distance` 是世界距离、起点已重叠时距离为 0。默认实现返回 `None`；
    /// 能查的后端需把 `capabilities().shape_cast` 置真。
    fn shape_cast(
        &self,
        _shape: CastShape,
        _origin: Vec3,
        _dir: Vec3,
        _max_distance: f32,
    ) -> Option<ShapeCastHit> {
        None
    }

    /// 可选：胶囊与场景刚体的重叠查询（角色控制器的站姿 / 蹲姿判定入口）。
    ///
    /// 结果按 `node` 升序。默认实现返回空表。
    ///
    /// 申报口径与 `capabilities()` **刻意分开**：`BackendCaps` 描述「能模拟什么」，
    /// 端口能力 `PortCaps`（`port.rs`）描述「能查什么」。把胶囊查询塞进 `BackendCaps`
    /// 会牵动 `r3e-protocol` 的 DTO 映射与 `capabilities.json` 三源门，
    /// 而这里只是一条新的查询方法，不该有那个连带成本。
    fn overlap_capsule(&self, _capsule: Capsule) -> Vec<PortHit> {
        Vec::new()
    }

    /// 可选：把胶囊沿 `dir` 推 `max_distance`，返回最近一次接触（扫掠）。
    ///
    /// 起点已重叠时返回 `distance == 0` 且 `started_inside == true` 的命中。
    /// 默认实现返回 `None`。
    fn sweep_capsule(&self, _capsule: Capsule, _dir: Vec3, _max_distance: f32) -> Option<PortHit> {
        None
    }

    /// 可选：装 / 卸地形高度场（M2 · C04 / D13）。
    ///
    /// 地形是**独立通道**：它不是刚体，不进 `BodyDesc`、不进 `ColliderShape`，
    /// 也不占 `capabilities()` 的任何一位。装卸一次只换一块数据，
    /// 不重建任何刚体、不改变 `body_count()`。
    ///
    /// 默认实现什么都不做（`Null` 后端与将来「不支持地形」的后端据此天然拒绝），
    /// 门面用 [`PhysicsBackend::heightfield`] 的返回值如实回报是否装上。
    fn set_heightfield(&mut self, _field: Option<HeightField>) {}

    /// 可选：当前装着的地形场。默认 `None`。
    fn heightfield(&self) -> Option<&HeightField> {
        None
    }

    /// 可选：采样世界 `(x, z)` 处可站立的最高表面。
    ///
    /// 输入非有限值返回 `None`；后端自带的地面平面恒存在，正常情况下总能拿到样本。
    /// 默认实现返回 `None`（表示「本后端不提供地表采样」，不是「此处无地面」）。
    fn sample_ground(&self, _x: f32, _z: f32) -> Option<GroundSample> {
        None
    }
}

/// 扫掠候选的择优规则，两个后端共用同一套，保证换后端不换「谁更近」的判定：
/// 初始重叠优先 → 距离更近优先 → `node` 升序兜底（后端内部表顺序不保证稳定）。
pub(crate) fn cast_candidate_better(cand: &ShapeCastHit, best: Option<&ShapeCastHit>) -> bool {
    let Some(best) = best else {
        return true;
    };
    if cand.started_inside != best.started_inside {
        return cand.started_inside;
    }
    if (cand.distance - best.distance).abs() > 1e-4 {
        return cand.distance < best.distance;
    }
    cand.node < best.node
}
