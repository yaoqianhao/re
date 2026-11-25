//! [`NavApi`]：`nav.*` 十条的协议面（M1-S4-D 三条，M2-D04 补七条）。
//!
//! # 为什么从 `api.rs` 里拆出来
//!
//! 十条导航命令的契约注释（回包结构 / 四种结局 / 「没烘过」的口径）加起来一百多行，
//! 挤在 [`crate::api::EngineApi`] 里会让协议层的单文件先被**注释**撞破
//! `lint-size` 的 800 行上限。拆成子 trait 之后：
//!
//! * `api.rs` 只管「有哪些命令」，`api/nav.rs` 只管「导航这十句怎么说」；
//! * [`crate::api::EngineApi`] 用 `: NavApi` 继承——调用方一个字都不用改，
//!   `api.nav_bake(..)` 照旧在 `EngineApi` 上直接调；
//! * 宿主实现 `NavApi`，其余（测试桩）用本 trait 的缺省实现，缺省口径与拆分前
//!   **逐字相同**：`Err(RpcFailure::unsupported("<方法名>"))`。
//!
//! 拆的是文件，不是语义：方法名、参数 DTO、回包契约、错误码一个字都没动。

use serde_json::Value;

use crate::msg::RpcFailure;
use crate::nav::{
    NavBakeOptions, NavFileTarget, NavNearestQuery, NavPathQuery, NavPointQuery, NavReachQuery,
    NavSmoothQuery,
};

/// `nav.*` 十条的协议面（M1-S4-D 三条查询，M2-D04 补七条常规操作）。
///
/// 全部方法都在主线程调用；缺省实现回 `METHOD_NOT_FOUND`，宿主可以只点亮当期
/// 需要的那几条。
pub trait NavApi {
    /// 烘焙导航网格（M1-S4-D）。产物挂在宿主侧，**不写场景**。
    ///
    /// 回包契约：`{ "baked": bool, "dims": [u32;2], "cellSize": f32, "cells": usize,
    /// "walkable": usize, "walkableRatio": f32, "regions": u32,
    /// "regionSizes": [u32...], "spawns": [{ "name": String, "region": u32 }...],
    /// "unresolved": [String...], "floatingSpawns": [String...] }`。
    ///
    /// `unresolved` / `floatingSpawns` 非空必须如实带出：它们是「这份导航数据
    /// 不可信」的直接证据，藏起来会让 AI 基于错网格报「路走不通」。
    fn nav_bake(&mut self, _opts: NavBakeOptions) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.bake"))
    }

    /// 寻路（M1-S4-D）：A* 求一条可走路径。
    ///
    /// 回包契约：`{ "found": bool, "outcome": String("found"|"unreachable"|
    /// "invalidEndpoint"|"budgetExceeded"), "reason": String（人话）,
    /// "points": [[f32;3]...], "count": usize }`。
    ///
    /// 四种结局都要如实区分：`unreachable`（分属不同连通块，几何上走不到）与
    /// `budgetExceeded`（算不完，结论未知）对调用方是完全不同的两个下一步。
    fn nav_path(&mut self, _q: NavPathQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.path"))
    }

    /// 可达性（M1-S4-D）：两点是否在同一连通块，断连时给断点。
    ///
    /// 回包契约：`{ "reachable": bool, "reason": String, "fromRegion": u32,
    /// "toRegion": u32, "disconnectPoint": [f32;3]|null }`。
    fn nav_reachability(&mut self, _q: NavReachQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.reachability"))
    }

    // ---- M2-D04：导航命令面补齐（四条常规操作 + 三条查询）----
    //
    // 七条的共同口径：
    //
    // 1. **没烘过照旧回 `NAV_NOT_BAKED`**：`nav.probe` / `nav.nearest` / `nav.stats` /
    //    `nav.hash` / `nav.export` / `nav.smooth` 六条吃的都是当前网格，槽位空着就是
    //    「没烘过」，绝不回零值或空表。
    // 2. **文件路径必须由调用方给出**：`nav.export` / `nav.import` 没有缺省文件名，
    //    也不猜工程目录——查询类命令不许产生隐式磁盘副作用。
    // 3. **`import` 只读不烘**：读回的产物直接恢复成可查询网格，不重跑烘焙、不重判
    //    可走性，否则「存盘再读回」会静默换一张网格。

    /// 折线平滑（M2-D04）：把一条密集路径剪成可直行的折线。
    ///
    /// 回包契约：`{ "inputCount": usize, "count": usize, "length": f32,
    /// "points": [[f32;3]...], "removed": usize }`。
    ///
    /// 只做几何剪枝、不重算路径：给的点无论从哪来，判据都是「这段视线全程可走」。
    /// 少于两点判参数错误，而不是回一条空折线。
    fn nav_smooth(&mut self, _q: NavSmoothQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.smooth"))
    }

    /// 单点探测（M2-D04）：这个点站不站得住、脚下地面多高、属于哪个连通块。
    ///
    /// 回包契约：`{ "walkable": bool, "region": u32, "groundHeight": f32|null,
    /// "heightDelta": f32|null, "cellSize": f32 }`。
    ///
    /// `heightDelta`（点高 − 地面高）是「悬空 / 埋进几何」的量化判据：
    /// 只回 `walkable: false` 时调用方无从判断差了多少，也就不知道该抬还是该降。
    fn nav_probe(&mut self, _q: NavPointQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.probe"))
    }

    /// 最近可走点（M2-D04）：给一点和半径，拿离它最近的可走点。
    ///
    /// 回包契约：`{ "found": bool, "point": [f32;3]|null, "distance": f32,
    /// "region": u32, "radius": f32 }`。
    ///
    /// 找不到时 `found: false` + `point: null`，**不回原点**——(0,0,0) 是个合法坐标，
    /// 用它表示「没找到」等于埋一个假锚点。
    fn nav_nearest(&mut self, _q: NavNearestQuery) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.nearest"))
    }

    /// 网格统计（M2-D04）：体检数字 + 产物哈希 + 分类直方图。
    ///
    /// 回包契约：`{ "dims": [u32;2], "cellSize": f32, "cells": usize,
    /// "walkable": usize, "walkableRatio": f32, "regions": u32,
    /// "regionSizes": [u32...], "classHistogram": { "flat": usize, "slope": usize,
    /// "step": usize, "blocked": usize }, "unresolved": [String...],
    /// "productHash": String, "cfgDigest": String, "inputDigest": String }`。
    ///
    /// 所有数字都从同一份 [`r3e_nav::Nav`] 现算，不缓存也不补齐——统计一旦和查询
    /// 各算各的，就会出现「报告说 80% 可走、查询却处处走不通」。
    fn nav_stats(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.stats"))
    }

    /// 产物哈希（M2-D04）：只回「这份网格是哪一份」的指纹。
    ///
    /// 回包契约：`{ "productHash": String, "cfgDigest": String, "inputDigest": String,
    /// "dims": [u32;2], "cellSize": f32, "cells": usize }`。
    ///
    /// 与 `nav.stats` 分开是为了让缓存命中判定**便宜**：判断「要不要重烘」只需要指纹，
    /// 不必把直方图与连通块表都算一遍。
    fn nav_hash(&mut self) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.hash"))
    }

    /// 导出 `.navmesh`（M2-D04）：把当前网格写成二进制产物。
    ///
    /// 回包契约：`{ "path": String（解析后的绝对路径）, "bytes": usize,
    /// "productHash": String }`。
    ///
    /// 路径为空即参数错误；父目录不存在也直接报错，不自动建目录树。
    fn nav_export(&mut self, _path: NavFileTarget) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.export"))
    }

    /// 导入 `.navmesh`（M2-D04）：把产物恢复成可查询网格写进宿主槽位。
    ///
    /// 回包契约：`{ "path": String, "bytes": usize, "productHash": String,
    /// "dims": [u32;2], "walkable": usize, "baked": false }`。
    ///
    /// `baked: false` 是**故意**的：这次没有烘焙，只是换上了一份现成产物。
    /// 调用方拿它区分「我刚烘的」和「我导入的」，`nav.backend.mismatch` 也因此
    /// 不会误报——导入的网格不来自任何后端。
    fn nav_import(&mut self, _path: NavFileTarget) -> Result<Value, RpcFailure> {
        Err(RpcFailure::unsupported("nav.import"))
    }
}
