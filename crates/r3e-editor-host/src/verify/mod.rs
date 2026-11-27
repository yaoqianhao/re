//! 三层验证器（M1-S4-E）：静态近似 → 物理精确 → 导航可达，合并成一份体检报告。
//!
//! 三条铁律（`docs/m1/M1_S4_PLAN.md` §4 S4-E）在这里落地：
//!
//! ① **近似只做候选**：`r3e-doc` 的 `support/overlap.suspect` 全部带
//!    `approximate = true`——它手里只有包围盒，拿不到碰撞体形状；
//! ② **精确覆盖近似，不是并列**：物理层 / 导航层对同一 `(category, target)` 给出
//!    `approximate = false` 的结论后，静态层那条候选会被
//!    [`r3e_doc::dedup_superseded`] 丢掉。用户永远只看到一条结论，不会出现
//!    「静态层说可能穿模、物理层说没穿」两条打架的东西；
//! ③ **算不了就报算不了**：哪一层缺能力（物理关闭 / 后端没申报 `shape_cast` /
//!    导航没烘焙 / 宿主没接导航槽位），就把原因写进 [`VerifyReport::unchecked`]。
//!    **绝不允许**把「没检查」当成「通过」——那会让门禁变成绿灯装饰。
//!
//! 本模块只组装，不判定：几何口径在 `r3e-doc`（静态）、`r3e-physics`（装配）、
//! `r3e-nav`（烘焙），三处都是既有实现，这里**不复制第二份**。

mod nav;
mod physics;

use r3e_core::{Scene, Vec3};
use r3e_doc::{dedup_superseded, validate_geometry, Category, Issue, Severity};
use r3e_nav::Nav;
use r3e_physics::Physics;

/// 校验层。报告里带层名，前端才能把结论归到「几何 / 物理 / 导航」哪一类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// 静态层：`r3e-doc`，只读场景结构，包围盒近似。
    Static,
    /// 物理层：`r3e-physics`，按装配形状精确复核。
    Physics,
    /// 导航层：`r3e-nav`，按烘焙结果判可走 / 可达。
    Nav,
}

impl Layer {
    /// 稳定字符串（进 JSON 回包 / 日志用，别改）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Physics => "physics",
            Self::Nav => "nav",
        }
    }
}

/// 某一层**没能**得出结论的原因。
///
/// 存在的意义只有一个：让「没检查」和「检查通过」在回包里长得不一样。
pub struct Unchecked {
    layer: Layer,
    reason: String,
}

impl Unchecked {
    pub fn new(layer: Layer, reason: impl Into<String>) -> Self {
        Self {
            layer,
            reason: reason.into(),
        }
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// 物理层这一趟的执行姿态（M1-S4-G）。
///
/// 事务影子场景没有自己的物理世界：`bind_scene` 要往节点写句柄，而写通道归文档层
/// 所有。于是「世界能不能代表这份场景」必须显式说出来，不许让调用方默默传一个
/// 过期世界下去——那种绿灯比红灯更有害。
pub enum PhysicsProbe<'a> {
    /// 世界与这份场景**已对齐**（主场景，或按影子刚重建过的临时世界）。
    Bound(&'a Physics),
    /// 世界代表不了这份场景：只出纯解析的穿模结论，悬空记 `unchecked`（附原因）。
    Unbound(&'a Physics, String),
}

/// 导航层这一趟的执行姿态（M1-S4-G）。
///
/// 与 [`PhysicsProbe`] 同因：影子场景的可达性只能靠「当场烘一份影子网格」判，
/// 烘不出来就必须说清楚为什么，不能把「没网格」当成「哪都能去」。
pub enum NavProbe<'a> {
    /// 跑这一层：`Some` = 本份场景刚烘出来的网格；`None` = 没烘过（由本层记账）。
    Run(Option<&'a Nav>),
    /// 不跑这一层：原因原样进 `unchecked`，绝不静默跳过。
    Skip(String),
}

/// 一次三层校验的完整结果。
pub struct VerifyReport {
    issues: Vec<Issue>,
    unchecked: Vec<Unchecked>,
}

impl VerifyReport {
    /// 不带目标点：只做出生点 + 场景自洽的体检。
    pub fn collect(scene: &Scene, physics: &Physics, nav: Option<&Nav>) -> Self {
        Self::collect_with_targets(scene, physics, nav, &[])
    }

    /// 带目标点：额外检查「出生点能不能走到这些点」。
    ///
    /// `targets` 是 `(名字, 世界坐标)`——名字进 issue 的 `target` 字段，前端点一下就能
    /// 定位到那个目标点，所以调用方给的应该是稳定的业务名而不是下标。
    pub fn collect_with_targets(
        scene: &Scene,
        physics: &Physics,
        nav: Option<&Nav>,
        targets: &[(String, Vec3)],
    ) -> Self {
        Self::collect_with(
            scene,
            Some(PhysicsProbe::Bound(physics)),
            NavProbe::Run(nav),
            targets,
        )
    }

    /// 按需跑层（M1-S4-G）：静态层**总是**跑，物理层看 `probe`，导航层看 `nav`。
    ///
    /// 为什么静态层不可关：它的近似条目是 [`dedup_superseded`] 的底稿。少了它，
    /// 精确层的结论不会被「覆盖」而是被当成新条目，差集口径会跟着漂。
    ///
    /// `probe = None` 表示这一趟不碰物理层（调用方自己往 `unchecked` 里写原因）。
    pub fn collect_with(
        scene: &Scene,
        probe: Option<PhysicsProbe<'_>>,
        nav: NavProbe<'_>,
        targets: &[(String, Vec3)],
    ) -> Self {
        // 静态层先跑：它的近似条目是后面的底稿，精确层只需覆盖、无需重复遍历场景。
        let mut issues = validate_geometry(scene);
        let mut unchecked = Vec::new();
        // 导航层的 `nav.backend.mismatch` 要对账「烘焙后端 vs 校验后端」，
        // 所以后端名必须在这里取出来，和 `physics::check` 用的是同一份世界。
        let backend_name = match &probe {
            Some(PhysicsProbe::Bound(physics)) => Some(physics.backend_name().to_string()),
            Some(PhysicsProbe::Unbound(physics, _)) => Some(physics.backend_name().to_string()),
            None => None,
        };
        match probe {
            Some(PhysicsProbe::Bound(physics)) => {
                physics::check(scene, physics, &mut issues, &mut unchecked);
            }
            Some(PhysicsProbe::Unbound(physics, why)) => {
                physics::check_unbound(scene, physics, &why, &mut issues, &mut unchecked);
            }
            None => {}
        }
        match nav {
            NavProbe::Run(nav) => nav::check(
                scene,
                nav,
                backend_name.as_deref(),
                targets,
                &mut issues,
                &mut unchecked,
            ),
            NavProbe::Skip(reason) => {
                unchecked.push(Unchecked::new(Layer::Nav, reason));
            }
        }
        Self {
            // 去重放在最后：先让物理 / 导航层的精确结论落地，才谈得上覆盖静态层近似。
            issues: dedup_superseded(issues),
            unchecked,
        }
    }

    /// 已排序、已去重的问题清单。
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// 各层「没检查」的原因（空 = 三层都真跑过）。
    pub fn unchecked(&self) -> &[Unchecked] {
        &self.unchecked
    }

    /// 追加一条「没检查」的原因（M1-S4-F：宿主的**世界新鲜度守卫**用）。
    ///
    /// 只许往里加 `unchecked`，不许改 `issues`：验证口径归三层各自拥有，
    /// 宿主能补的只有「这一层我没法替你保证」——那正好是 `unchecked` 的语义。
    pub fn note_unchecked(&mut self, layer: Layer, reason: impl Into<String>) {
        self.unchecked.push(Unchecked::new(layer, reason));
    }

    /// 某层报了多少条问题。
    pub fn count_by_layer(&self, layer: Layer) -> usize {
        self.issues
            .iter()
            .filter(|i| layer_of(i.category) == layer)
            .count()
    }

    /// `Error` 条数。
    pub fn error_count(&self) -> usize {
        self.count(Severity::Error)
    }

    /// `Warn` 条数。
    pub fn warn_count(&self) -> usize {
        self.count(Severity::Warn)
    }

    fn count(&self, severity: Severity) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == severity)
            .count()
    }

    /// 是否「可以继续」（没有 `Error`）。
    ///
    /// 注意：`unchecked` 非空时这个值仍然可能是 `true`——判「能不能过」的一方必须同时
    /// 看 [`VerifyReport::unchecked`]，否则会忽略「没检查」。
    pub fn passed(&self) -> bool {
        self.error_count() == 0
    }
}

/// 问题类别 → 归属层。
///
/// 与 [`Category`] 的划分对齐：几何近似类在静态层，穿透在物理层，可走 / 可达在导航层。
fn layer_of(category: Category) -> Layer {
    match category {
        // 角色配置 / 踩地判据是静态可判的确定性结论（不吃物理后端），归静态层
        Category::Reference | Category::Bounds | Category::Material | Category::Character => {
            Layer::Static
        }
        // 地形判据（M2-C06）同样只吃 core 数据：样本数与分辨率、块覆盖都是场景内真值，
        // 物理层拿地形场复核的是「碰撞形状对不对」，不是「块划分自相矛盾」，两码事。
        Category::Terrain => Layer::Static,
        // prefab 判据（M2-E04）：模板是否存在、覆盖字段是不是抄回模板原值，两者都只吃
        // `Scene` 里的模板表与实例表，不碰物理 / 导航后端。归静态层，`verify` 才不必
        // 为了「有没有白覆盖」去启动后端。
        Category::Prefab => Layer::Static,
        Category::Support | Category::Overlap => Layer::Physics,
        Category::Reachability => Layer::Nav,
    }
}
