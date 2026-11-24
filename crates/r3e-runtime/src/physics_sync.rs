//! 场景刚体重绑的判重：物理世代号 + 场景「结构摘要」。
//!
//! 从 `app.rs` 拆出（`app.rs` 在大文件债务白名单里，只许减不许增）：判重规则与
//! 窗口、事件循环无关，只有「哪些节点该重绑」这一件事，独立成文件后才能单独讲清楚。
//!
//! 装配本体仍在 [`r3e_physics::Physics::bind_scene`]，运行时独有的网格来源在
//! [`crate::physics_bind`]；本模块只负责回答一个问题：**这一帧要不要重绑**。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use r3e_asset::AssetLibrary;
use r3e_doc::Document;
use r3e_physics::Physics;

/// 上次成功绑定时的场景摘要，用来判「要不要再绑一次」。
pub(crate) struct PhysicsSync {
    /// 上次完成「场景声明刚体」绑定的物理世代号。
    gen: u64,
    /// 上次装配时场景的「结构摘要」：`(节点, 是否声明了 physics 段)` 的升序表。
    ///
    /// 只凭世代号判重会漏掉**脚本 / 编辑器新加的节点**：世代号只在开关物理、切后端、
    /// 清空刚体表时变，而 `scene.spawn` 加进来的地板既不改世代号、也没有任何
    /// 「请重绑」的钩子 —— 于是节点在场景里、看上去带着 `physics` 段，物理世界里
    /// 却没有它，角色一路掉进虚空（B05 真窗口实测就是这个症状）。
    /// 摘要只看「哪几个节点声明了物理」，节点每帧挪位置不会让它变，因此不会把
    /// 每帧 `settransform` 变成每帧全量重绑（重绑本身幂等，但没必要白跑）。
    structure: Vec<(u64, bool)>,
}

/// 首帧之前先装配一次，返回可继续复用的判重状态（M2-C07）。
///
/// 装配平时由首帧 `update` 里的 [`PhysicsSync::sync`] 触发，而脚本 `on_start` 在
/// **脚本载入那一刻**就跑：脚本第一次 `physics_raycast` 打下去时地形高度场还没进
/// 物理世界，回答是「脚下没有地面」，靠射线贴地的角色于是被当成悬空、原地卡死
/// （真窗口实测症状）。提前走同一入口装一次，脚本看到的世界与第一帧看到的是同一个。
///
/// 放在这里而不是 `app.rs`：`app.rs` 在 `size_debt` 白名单里，只许减不许增。
pub(crate) fn prebound(
    physics: &Rc<RefCell<Physics>>,
    scene: &Rc<RefCell<Document>>,
    assets: &Rc<RefCell<Arc<AssetLibrary>>>,
) -> PhysicsSync {
    let mut sync = PhysicsSync::new();
    sync.sync(physics, scene, assets);
    sync
}

impl PhysicsSync {
    pub(crate) fn new() -> Self {
        Self {
            gen: 0,
            structure: Vec::new(),
        }
    }

    /// 场景的物理结构摘要：`(节点, 是否声明 physics 段)` 按节点升序。
    ///
    /// 只描述「哪些节点该有刚体」，不含变换与刚体句柄：位置每帧都会变、句柄由装配自己
    /// 写回，两者混进来会让摘要每帧都不同，等于把幂等判重废掉。
    fn structure_of(scene: &Rc<RefCell<Document>>) -> Vec<(u64, bool)> {
        let doc = scene.borrow();
        let mut sig: Vec<(u64, bool)> = doc
            .as_scene()
            .iter()
            .map(|n| (n.id, n.physics.is_some()))
            .collect();
        sig.sort_unstable_by_key(|(id, _)| *id);
        sig
    }

    /// 把场景里声明了 `physics` 段的物件绑成刚体；返回本次是否真的重绑了。
    ///
    /// 物理开关 / 后端切换 / 刚体表清空都会让世代号自增，这里只在世代变化或结构变化时
    /// 重绑一次，因此脚本用 `attach_body` 手动绑的刚体不会被覆盖。
    ///
    /// 运行时只做三件本地事：判世代与结构、递网格来源、把报告写进日志。编辑器与内容
    /// 验证器走同一个装配入口，「运行时撞到的东西」与「校验器认为该有的东西」才不会
    /// 各说各话。
    pub(crate) fn sync(
        &mut self,
        physics: &Rc<RefCell<Physics>>,
        scene: &Rc<RefCell<Document>>,
        assets: &Rc<RefCell<Arc<AssetLibrary>>>,
    ) -> bool {
        let (gen, enabled) = {
            let p = physics.borrow();
            (p.generation(), p.enabled())
        };
        if !enabled {
            return false;
        }
        let structure = Self::structure_of(scene);
        if gen == self.gen && structure == self.structure {
            return false;
        }
        self.gen = gen;
        self.structure = structure;

        // 先取 `Arc` 快照再借 `physics`：装配在场景的 `write` 闭包里跑，
        // 借用顺序反了会与外层 `RefCell` 撞车。
        let assets: Arc<AssetLibrary> = Arc::clone(&assets.borrow());
        let mut physics = physics.borrow_mut();
        let report = crate::physics_bind::rebind(&mut physics, scene, &assets);

        if report.bound() > 0 {
            log::info!("物理已绑定 {} 个场景刚体", report.bound());
        }
        // 有节点推不出刚体参数时不静默：这类场景能跑起来但物件不会动，
        // 事后排查只看「没报错」是找不出原因的。
        if report.skipped > 0 {
            log::warn!("{}", report.summary());
        }
        if report.bound() > 0 && !report.simulated() {
            log::warn!("{}", report.summary());
        }
        true
    }
}
