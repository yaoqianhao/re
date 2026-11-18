//! `scene.verify` 的参数承载（M1-S4-F）。
//!
//! 这一条只承载**「查什么」**：要看哪些目标点、最多回多少条。三层各自的阈值
//! （悬空容差、相切 eps、可走坡度、A* 扩展预算）留在各 crate 的配置里，
//! 协议层不抄第二份——抄一份就多一个衰落点，改了引擎忘了改协议的事故最难查。
//!
//! 空 `targets` 不是错误：那就是「只做出生点 + 场景自洽的体检」，与
//! `VerifyReport::collect` 的语义一致。

/// 一个可达性目标点。
///
/// `name` 是业务名（例如 `"loot-chest-2"`），会原样进 issue 的 `target` 字段——
/// 前端拿它就能点亮那个目标点，所以**不要**用下标当名字。
#[derive(Debug, Clone)]
pub struct VerifyTarget {
    /// 业务名（进 issue 的 `target`）。
    pub name: String,
    /// 世界坐标。
    pub at: [f32; 3],
}

/// `scene.verify` 的入参。
#[derive(Debug, Clone, Default)]
pub struct VerifyQuery {
    /// 可达性目标点；空 = 只做场景自洽体检。
    pub targets: Vec<VerifyTarget>,
    /// 回包最多带多少条 issue（缺省 / `null` = 不截断）。计数不受截断影响。
    pub limit: Option<usize>,
}
