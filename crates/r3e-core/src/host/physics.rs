//! 物理能力面：请求 / 应答封闭集合与 `PhysicsHost` 契约。
//!
//! 拆自 `host.rs`（单文件曾超 800 行阈值），语义不变。

use std::any::Any;

use crate::{Node, PhysicsSceneDesc};

// ======================================================================
// 物理
// ======================================================================

/// 一次物理请求：脚本全部物理诉求的封闭集合。
///
/// 只带 core 类型（`&Node` / `u64` 刚体号 / 纯数值），构造 `BodyDesc` 这类
/// 物理类型的工作全部留给实现方。
pub enum PhysReq<'a> {
    /// 给节点挂刚体：`desc` 由实现方从 `node` 推断，重复挂时旧刚体必须先回收。
    Attach {
        node: &'a Node,
        kind: &'a str,
        mass: f64,
    },
    /// 按刚体号销毁。
    Detach {
        handle: u64,
    },
    Impulse {
        handle: u64,
        v: [f64; 3],
    },
    BodyCount,
    Enabled,
    SetEnabled(bool),
    Backend,
    SetBackend(&'a str),
    Capabilities,
    HasCapability(&'a str),
    Raycast {
        origin: [f64; 3],
        dir: [f64; 3],
        max: f64,
    },
    ShapeCastSphere {
        center: [f64; 3],
        radius: f64,
        dir: [f64; 3],
        max: f64,
    },
    ShapeCastBox {
        center: [f64; 3],
        half: [f64; 3],
        dir: [f64; 3],
        max: f64,
    },
    /// 球体重叠：只要命中节点的 id（按 id 升序）。
    Overlap {
        center: [f64; 3],
        radius: f64,
    },
    SetCcd(bool),
    CcdDefault,
    BodySetCcd {
        handle: u64,
        on: bool,
    },
    BodyCcd {
        handle: u64,
    },
    /// 取走自上次调用以来的碰撞事件（已渲染成文本）。
    TakeEvents,
    Gravity,
    SetGravity([f64; 3]),
    Ground,
    SetGround(f64),
    /// 场景重建：清空刚体并应用场景文件声明的物理配置。
    /// 错误文本原样回传（脚本宿主只打印，不中断重放）。
    ResetScene {
        desc: &'a PhysicsSceneDesc,
    },
    /// 只清空刚体、不动物理配置（「无基础场景」的热重载路径）。
    ClearBodies,
}

/// 一次查询命中：脚本侧的**扁平**结构，不含任何物理类型。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HitDto {
    pub node: i64,
    pub handle: i64,
    pub distance: f64,
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// shape-cast 专用：起始就重叠（raycast 恒 `false`）。
    pub inside: bool,
}

/// 物理应答。变体故意保持纤细，解包逻辑集中在默认方法里。
pub enum PhysRsp {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Texts(Vec<String>),
    Hit(Option<HitDto>),
    Nodes(Vec<i64>),
    /// 带可选错误文本的「动作完成」应答（`ResetScene`）。
    Err(Option<String>),
    Unit,
}

/// 脚本对物理的全部诉求。实现方只需给出 `execute`。
pub trait PhysicsHost {
    /// 函数式适配器入口：`r3e-runtime` 用它把真实 `Physics` 接进来。
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp;

    /// 让装配层（`r3e-runtime`）能从 trait 背后取回自己的实现体。
    fn as_any(&self) -> &dyn Any;

    // ---- 以下全是默认方法：ScriptHost 内部与 api 层统一走这些形态 ----

    /// 挂刚体成功返回刚体号；物理未启用返回 `None`（脚本侧打印提示后返回 false）。
    fn attach(&self, node: &Node, kind: &str, mass: f64) -> Option<u64> {
        match self.execute(PhysReq::Attach { node, kind, mass }) {
            PhysRsp::Int(h) if h >= 0 => Some(h as u64),
            _ => None,
        }
    }

    fn detach(&self, handle: u64) {
        self.execute(PhysReq::Detach { handle });
    }

    fn impulse(&self, handle: u64, v: [f64; 3]) {
        self.execute(PhysReq::Impulse { handle, v });
    }

    fn body_count(&self) -> i64 {
        as_int(self.execute(PhysReq::BodyCount))
    }

    fn enabled(&self) -> bool {
        as_bool(self.execute(PhysReq::Enabled))
    }

    /// 返回**切换后**的状态（与脚本 `physics_set_enabled` 的返回语义一致）。
    fn set_enabled(&self, on: bool) -> bool {
        as_bool(self.execute(PhysReq::SetEnabled(on)))
    }

    fn backend(&self) -> String {
        as_text(self.execute(PhysReq::Backend))
    }

    fn set_backend(&self, name: &str) -> bool {
        as_bool(self.execute(PhysReq::SetBackend(name)))
    }

    fn capabilities(&self) -> String {
        as_text(self.execute(PhysReq::Capabilities))
    }

    fn has_capability(&self, key: &str) -> bool {
        as_bool(self.execute(PhysReq::HasCapability(key)))
    }

    fn raycast(&self, origin: [f64; 3], dir: [f64; 3], max: f64) -> Option<HitDto> {
        as_hit(self.execute(PhysReq::Raycast { origin, dir, max }))
    }

    fn shape_cast_sphere(
        &self,
        center: [f64; 3],
        radius: f64,
        dir: [f64; 3],
        max: f64,
    ) -> Option<HitDto> {
        as_hit(self.execute(PhysReq::ShapeCastSphere {
            center,
            radius,
            dir,
            max,
        }))
    }

    fn shape_cast_box(
        &self,
        center: [f64; 3],
        half: [f64; 3],
        dir: [f64; 3],
        max: f64,
    ) -> Option<HitDto> {
        as_hit(self.execute(PhysReq::ShapeCastBox {
            center,
            half,
            dir,
            max,
        }))
    }

    fn overlap(&self, center: [f64; 3], radius: f64) -> Vec<i64> {
        match self.execute(PhysReq::Overlap { center, radius }) {
            PhysRsp::Nodes(v) => v,
            _ => Vec::new(),
        }
    }

    fn set_ccd(&self, on: bool) -> bool {
        as_bool(self.execute(PhysReq::SetCcd(on)))
    }

    fn ccd_default(&self) -> bool {
        as_bool(self.execute(PhysReq::CcdDefault))
    }

    fn body_set_ccd(&self, handle: u64, on: bool) -> bool {
        as_bool(self.execute(PhysReq::BodySetCcd { handle, on }))
    }

    fn body_ccd(&self, handle: u64) -> bool {
        as_bool(self.execute(PhysReq::BodyCcd { handle }))
    }

    fn take_events(&self) -> Vec<String> {
        match self.execute(PhysReq::TakeEvents) {
            PhysRsp::Texts(v) => v,
            _ => Vec::new(),
        }
    }

    fn gravity(&self) -> f64 {
        as_float(self.execute(PhysReq::Gravity))
    }

    fn set_gravity(&self, g: [f64; 3]) {
        self.execute(PhysReq::SetGravity(g));
    }

    fn ground(&self) -> f64 {
        as_float(self.execute(PhysReq::Ground))
    }

    fn set_ground(&self, y: f64) {
        self.execute(PhysReq::SetGround(y));
    }

    /// 只清空刚体（不动物理配置）。
    fn clear_bodies(&self) {
        self.execute(PhysReq::ClearBodies);
    }

    /// 场景重建：`Some(错误文本)` 表示配置应用失败（刚体已清空）。
    fn reset_scene(&self, desc: &PhysicsSceneDesc) -> Option<String> {
        match self.execute(PhysReq::ResetScene { desc }) {
            PhysRsp::Err(e) => e,
            _ => None,
        }
    }
}

/// 函数式适配器：任何满足 `Fn(PhysReq) -> PhysRsp` 的闭包**天然就是一个 host**。
impl<F> PhysicsHost for F
where
    F: for<'a> Fn(PhysReq<'a>) -> PhysRsp + 'static,
{
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp {
        (self)(req)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 空实现：未接入物理时（编辑器 / 单测）的默认值，语义等同今日的 `null` 后端。
///
/// 行为对齐依据：`null` 后端下刚体不积分、查询恒不命中、能力位全 `false`。
pub struct NullPhysicsHost;

impl PhysicsHost for NullPhysicsHost {
    fn execute(&self, req: PhysReq<'_>) -> PhysRsp {
        match req {
            PhysReq::BodyCount => PhysRsp::Int(0),
            PhysReq::Backend => PhysRsp::Text("null".to_string()),
            PhysReq::TakeEvents => PhysRsp::Texts(Vec::new()),
            PhysReq::Overlap { .. } => PhysRsp::Nodes(Vec::new()),
            PhysReq::Raycast { .. }
            | PhysReq::ShapeCastSphere { .. }
            | PhysReq::ShapeCastBox { .. } => PhysRsp::Hit(None),
            PhysReq::SetBackend(name) => PhysRsp::Bool(name == "null"),
            PhysReq::ResetScene { .. } => PhysRsp::Err(None),
            _ => PhysRsp::Bool(false),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ======================================================================
// 解包工具
// ======================================================================

fn as_bool(rsp: PhysRsp) -> bool {
    matches!(rsp, PhysRsp::Bool(true))
}

fn as_int(rsp: PhysRsp) -> i64 {
    match rsp {
        PhysRsp::Int(v) => v,
        _ => 0,
    }
}

fn as_float(rsp: PhysRsp) -> f64 {
    match rsp {
        PhysRsp::Float(v) => v,
        PhysRsp::Int(v) => v as f64,
        _ => 0.0,
    }
}

fn as_text(rsp: PhysRsp) -> String {
    match rsp {
        PhysRsp::Text(v) => v,
        _ => String::new(),
    }
}

fn as_hit(rsp: PhysRsp) -> Option<HitDto> {
    match rsp {
        PhysRsp::Hit(h) => h,
        _ => None,
    }
}
