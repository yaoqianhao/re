//! `nav.*` 三条的参数承载（M1-S4-D）。
//!
//! 与 `probe.rs` 同口径：协议层不依赖 `r3e-nav`，`NavConfig` 的字段在这里**平铺**成
//! 一堆 `Option`，`None` 一路传到宿主，由宿主取 `NavConfig::default()`。
//!
//! 为什么不把默认值抄一份到协议层：`NavConfig::default()` 里的 `agent_radius` /
//! `step_height` 会随烘焙算法调整（比如引入「能跨过的台阶高度」新判定），抄一份到
//! 协议层意味着每次调默认值都要改两处，漏改就变成「IDE 里烘出来的网格和运行时不是
//! 同一份」。默认值只有一个出处。

/// `nav.bake` 的烘焙参数。全部可缺省：一个 `{}` 进来就是「按缺省参数烘整个场景」。
///
/// `ignoreTags` 是**整体替换**而不是追加：给空数组表示「不忽略任何标签」，
/// 给 `None` 才表示「用缺省忽略表」。这两种意图必须分得开，否则用户无法表达
/// 「把会动的物件也烘成障碍」。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NavBakeOptions {
    /// 格边长（米）。
    pub cell_size: Option<f32>,
    /// 烘焙范围 `[minX, minZ, maxX, maxZ]`。
    pub bounds: Option<[f32; 4]>,
    /// 角色半径（障碍按它外扩）。
    pub agent_radius: Option<f32>,
    /// 角色身高（低于它的横梁不挡路）。
    pub agent_height: Option<f32>,
    /// 单步可跨高度差。
    pub step_height: Option<f32>,
    /// 可行走坡度上限（度）。
    pub max_slope_deg: Option<f32>,
    /// 不算静态障碍的标签表；`None` = 缺省忽略表。
    pub ignore_tags: Option<Vec<String>>,
}

/// `nav.path` 的起终点（世界坐标）。
#[derive(Debug, Clone, PartialEq)]
pub struct NavPathQuery {
    /// 起点。
    pub from: [f32; 3],
    /// 终点。
    pub to: [f32; 3],
}

/// `nav.reachability` 的起终点（世界坐标）。
#[derive(Debug, Clone, PartialEq)]
pub struct NavReachQuery {
    /// 起点。
    pub from: [f32; 3],
    /// 终点。
    pub to: [f32; 3],
}

/// `nav.smooth` 的待剪枝折线（M2 · D04）。
///
/// 入参是**折线**而不是起点 + 终点：平滑的对象是既有的密集路径（A\* 输出或工具生成），
/// 让它自己再求一遍路等于把「平滑」和「寻路」两件事搅在一起，调用方反而没法
/// 只对已有路径做后处理。
#[derive(Debug, Clone, PartialEq)]
pub struct NavSmoothQuery {
    /// 折线顶点（世界坐标，至少 2 个）。
    pub points: Vec<[f32; 3]>,
}

/// `nav.probe` 的单点查询（M2 · D04）。
#[derive(Debug, Clone, PartialEq)]
pub struct NavPointQuery {
    /// 世界坐标点。
    pub point: [f32; 3],
}

/// `nav.nearest` 的吸附查询（M2 · D04）。
#[derive(Debug, Clone, PartialEq)]
pub struct NavNearestQuery {
    /// 世界坐标点。
    pub point: [f32; 3],
    /// 搜索半径（米）；缺省由宿主给一个可解释的默认值。
    pub radius: Option<f32>,
    /// 限定在同一连通块内找（`None` = 全网格）。
    pub region: Option<u32>,
}

/// `nav.export` / `nav.import` 的目标文件（M2 · D04）。
///
/// 路径**必须**由调用方给出：两条命令都没有缺省文件名，也不猜工程目录。
/// 这是 §7 的回退档纪律——查询类命令不许悄悄产生磁盘副作用。
#[derive(Debug, Clone, PartialEq)]
pub struct NavFileTarget {
    /// 显式给出的文件路径（宿主负责解析成绝对路径并回填）。
    pub path: String,
}
