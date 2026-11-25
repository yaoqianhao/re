//! `nav.*` 里「要算一段」或「要读写字节」的四条（M2 · D04）：`smooth` / `stats` /
//! `export` / `import`。
//!
//! 为什么落在 runtime 而不是宿主：这四条不碰 `Scene`、不碰协议 JSON，只吃一份
//! `Nav` 加上坐标或路径，属于「导航产物的常规操作」。放这里，宿主只做协议翻译与
//! 状态哈希纪律，CLI 与无头运行也能调同一份实现——判据 6 的
//! `rpc nav.export --path ...` 走的就是这条路径。
//!
//! 与 `terrain_ops` 同口径：**返回结构化结果，不返回 JSON**；错误用本模块的
//! [`NavOpError`] 分类，由宿主翻成 `RpcFailure`。这样「算错了」和「参数错了」
//! 在协议面上仍然分得开（前者是导航数据问题，后者是调用方问题）。
//!
//! 三条纪律的落地点：
//!
//! * **export 的路径必须由调用方显式给出**（§7 回退档纪律）：本模块没有默认文件名，
//!   也不猜「工程目录在哪」。没有路径就是参数错，绝不悄悄写到当前工作目录。
//! * **import 只读不烘**：读回来的产物用 `Nav::from_product` 恢复，不重跑烘焙、
//!   不重判可走性——否则「存盘再读回」会静默换一张网格（判据 5）。
//! * **stats 是唯一真值的派生**：所有数字都从同一份 `Nav` 现算，不缓存、不补齐，
//!   `unresolved` 原样透出——它是「这份网格不可信」的直接证据。

use std::path::Path;

use glam::Vec3;
use r3e_nav::{polyline_length, smooth_path, Nav, NavError};

/// 导航操作失败的两类原因。
#[derive(Debug, Clone, PartialEq)]
pub enum NavOpError {
    /// 入参不合法：非有限值、空点列、半径非正、路径为空串。
    Invalid(String),
    /// 导航数据 / 产物格式层面的错误（版本不符、尺寸非法、字节流损坏）。
    Nav(String),
}

impl std::fmt::Display for NavOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "导航命令参数非法：{reason}"),
            Self::Nav(reason) => write!(f, "导航数据不可用：{reason}"),
        }
    }
}

impl std::error::Error for NavOpError {}

impl From<NavError> for NavOpError {
    fn from(err: NavError) -> Self {
        Self::Nav(err.to_string())
    }
}

/// `nav.smooth` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedOutcome {
    /// 平滑后的折线（世界坐标，端点保持不变）。
    pub points: Vec<[f32; 3]>,
    /// 折线总长（米），按平滑后的点现算。
    pub length: f32,
    /// 输入折线的点数，供调用方对比「少了多少点」。
    pub input_count: usize,
}

/// `nav.smooth`：把一条密集折线剪成可直行的折线。
///
/// 只做**几何**剪枝，不重算路径：给的点无论从哪来（A\* 输出、手工画、别的工具），
/// 剪枝的判据都是「这一段视线是否全程可走」——与 [`r3e_nav::smooth_path`] 同一条。
/// 少于两个点属于参数错（一个点的折线没有「可直行」可言），而不是返回空表。
pub fn smooth(nav: &Nav, points: &[Vec3]) -> Result<SmoothedOutcome, NavOpError> {
    if points.len() < 2 {
        return Err(NavOpError::Invalid(format!(
            "平滑至少需要 2 个点，收到 {} 个",
            points.len()
        )));
    }
    for (idx, p) in points.iter().enumerate() {
        if !p.is_finite() {
            return Err(NavOpError::Invalid(format!("第 {idx} 个点不是有限值")));
        }
    }
    let smoothed = smooth_path(nav.grid(), points);
    let length = polyline_length(&smoothed.points);
    Ok(SmoothedOutcome {
        points: smoothed.points.iter().map(|p| p.to_array()).collect(),
        length,
        input_count: points.len(),
    })
}

/// `nav.stats` 的结果。
///
/// 字段与 `nav.bake` 的回包**故意有重叠也有差异**：重叠的是体检数字（都从
/// `NavReport` 来，两处必然一致），差异的是这里多了产物哈希与分类直方图
/// ——它们是「缓存命中」与「哪类格子在拖后腿」的判据，而 `nav.bake` 在写槽位
/// 那一刻还没必要给。
#[derive(Debug, Clone, PartialEq)]
pub struct NavStats {
    pub dims: [u32; 2],
    pub cell_size: f32,
    pub cells: usize,
    pub walkable: usize,
    pub walkable_ratio: f32,
    pub regions: u32,
    pub region_sizes: Vec<u32>,
    pub class_histogram: [usize; 4],
    pub unresolved: Vec<String>,
    pub product_hash: String,
    pub cfg_digest: String,
    pub input_digest: String,
}

/// `nav.stats`：把 `NavReport::summary()` 里打日志的那些数字变成结构化回包。
pub fn stats(nav: &Nav) -> NavStats {
    let report = nav.report();
    NavStats {
        dims: report.dims,
        cell_size: report.cell_size,
        cells: report.cells,
        walkable: report.walkable,
        walkable_ratio: report.walkable_ratio(),
        regions: report.regions,
        region_sizes: report.region_sizes.clone(),
        class_histogram: nav.grid().class_histogram(),
        unresolved: report.unresolved.clone(),
        product_hash: nav.product_hex(),
        cfg_digest: format!("{:016x}", nav.cfg_digest()),
        input_digest: format!("{:016x}", nav.input_digest()),
    }
}

/// `nav.export` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOutcome {
    /// 实际写到的绝对路径（把调用方给的相对路径按当前工作目录解析后回填）。
    pub path: String,
    /// 写出的字节数。
    pub bytes: usize,
    /// 写出内容的产物哈希（与 `nav.hash` 的 `productHash` 必须相同）。
    pub product_hash: String,
}

/// `nav.export`：把当前网格按 `NAV_FORMAT` 写成 `.navmesh`。
///
/// 路径必须显式给出。`path` 为空串或只含空白一律判参数错——把「没给路径」
/// 当成「用缺省名写到当前目录」，等于让一次查询悄悄产生磁盘副作用。
/// 父目录不存在也直接报错，不自动建目录树：写盘位置是调用方的决定，
/// 不是本函数的推断。
pub fn export(nav: &Nav, path: &str) -> Result<ExportOutcome, NavOpError> {
    let key = path.trim();
    if key.is_empty() {
        return Err(NavOpError::Invalid(
            "export 必须给出显式 `path`（不提供缺省落盘位置）".to_string(),
        ));
    }
    let target = crate::capture::absolute_path(Path::new(key)).map_err(NavOpError::Invalid)?;
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(NavOpError::Invalid(format!(
                "父目录不存在：{}",
                parent.display()
            )));
        }
    }
    let product = nav.product();
    let bytes = product.to_bytes();
    let product_hash = product.hash_hex();
    std::fs::write(&target, &bytes)
        .map_err(|err| NavOpError::Invalid(format!("写不进 {}：{err}", target.display())))?;
    Ok(ExportOutcome {
        path: target.display().to_string(),
        bytes: bytes.len(),
        product_hash,
    })
}

/// `nav.import` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOutcome {
    pub path: String,
    pub bytes: usize,
    pub product_hash: String,
    pub cells: usize,
    pub dims: [u32; 2],
}

/// `nav.import`：读一个 `.navmesh` 恢复成可查询的网格。
///
/// 返回的是 `(Nav, ImportOutcome)`：网格交给宿主写进槽位，回包只带
/// 「调用方需要用来核对的东西」（路径、字节数、产物哈希、格数与尺寸）。
pub fn import(path: &str) -> Result<(Nav, ImportOutcome), NavOpError> {
    let key = path.trim();
    if key.is_empty() {
        return Err(NavOpError::Invalid(
            "import 必须给出显式 `path`".to_string(),
        ));
    }
    let target = crate::capture::absolute_path(Path::new(key)).map_err(NavOpError::Invalid)?;
    let bytes = std::fs::read(&target)
        .map_err(|err| NavOpError::Invalid(format!("读不了 {}：{err}", target.display())))?;
    let product = r3e_nav::NavProduct::from_bytes(&bytes)?;
    let product_hash = product.hash_hex();
    let cells = product.walkable_cells();
    let dims = product.dims();
    let nav = Nav::from_product(product)?;
    Ok((
        nav,
        ImportOutcome {
            path: target.display().to_string(),
            bytes: bytes.len(),
            product_hash,
            cells,
            dims,
        },
    ))
}

/// `nav.nearest` 在调用方没给半径时的缺省搜索半径（米）。
///
/// 取 4 m 的理由：常见角色半径在 0.3～0.6 m、单格 0.5 m 上下，4 m 约等于
/// 「八格之内」——足够把站在墙边、贴着台阶的点吸回可走面，又不至于把
/// 隔着一道墙的连通块拉过来。要给别的值就显式传 `radius`。
pub const DEFAULT_NEAREST_RADIUS: f32 = 4.0;

/// `nav.probe` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeOutcome {
    /// 探测点本身（原样回填，便于调用方对齐多条结果）。
    pub point: [f32; 3],
    /// 这个点站不站得住（可走格内且落在容差高度内）。
    pub walkable: bool,
    /// 所属连通块（`0` = 不可走）。
    pub region: u32,
    /// 脚下地面高度；越界为 `None`。
    pub ground_height: Option<f32>,
    /// 点高减地面高：正数悬空、负数埋进几何。
    pub height_delta: Option<f32>,
    /// 当前网格的格边长，供调用方把「差多少米」换算成「差几格」。
    pub cell_size: f32,
}

/// `nav.probe`：单点体检——站不站得住、脚下多高、属于哪个连通块。
///
/// 与 `nav.path` 的区别是**没有起点也没有终点**：关卡作者手上常有的只是「这个
/// 出生点/传送点/道具点」，需要单独判断它有没有落在地上。用寻路去探一个点会
/// 被「起点吸附」掩盖问题（吸附成功就看起来没事），这里报的是原始判定。
pub fn probe(nav: &Nav, point: Vec3) -> Result<ProbeOutcome, NavOpError> {
    if !point.is_finite() {
        return Err(NavOpError::Invalid("probe 的点不是有限值".to_string()));
    }
    let ground = nav.ground_height(point);
    Ok(ProbeOutcome {
        point: point.to_array(),
        walkable: nav.is_walkable(point),
        region: nav.region_of(point),
        ground_height: ground,
        height_delta: ground.map(|g| point.y - g),
        cell_size: nav.grid().config().cell_size,
    })
}

/// `nav.nearest` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct NearestOutcome {
    /// 是否找到了可走点。
    pub found: bool,
    /// 命中点（世界坐标）；没找到是 `None`，不是原点。
    pub point: Option<[f32; 3]>,
    /// 命中点到查询点的距离；没找到是 `None`。
    pub distance: Option<f32>,
    /// 命中点所在连通块（没找到为 `0`）。
    pub region: u32,
    /// 实际使用的搜索半径。
    pub radius: f32,
}

/// `nav.nearest`：把任意一点吸到最近的可走面上。
///
/// `region` 给定时只在该连通块内找——这正是「这个门到底连不连通主区域」的判据：
/// 不限定时总能吸到某个可走点，限定后吸不到，才说明这一片是孤岛。
///
/// 半径必须为正：`0` 或负数不是「不搜」，而是调用方算错了，直接判参数错。
pub fn nearest(
    nav: &Nav,
    point: Vec3,
    radius: Option<f32>,
    region: Option<u32>,
) -> Result<NearestOutcome, NavOpError> {
    if !point.is_finite() {
        return Err(NavOpError::Invalid("nearest 的点不是有限值".to_string()));
    }
    let radius = radius.unwrap_or(DEFAULT_NEAREST_RADIUS);
    if !radius.is_finite() || radius <= 0.0 {
        return Err(NavOpError::Invalid(format!(
            "搜索半径必须为正的有限值，收到 {radius}"
        )));
    }
    match nav.nearest_walkable(point, radius, region) {
        Some(hit) => Ok(NearestOutcome {
            found: true,
            point: Some(hit.to_array()),
            distance: Some((hit - point).length()),
            region: nav.region_of(hit),
            radius,
        }),
        None => Ok(NearestOutcome {
            found: false,
            point: None,
            distance: None,
            region: 0,
            radius,
        }),
    }
}

/// `nav.hash` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct HashOutcome {
    pub product_hash: String,
    pub cfg_digest: String,
    pub input_digest: String,
    pub dims: [u32; 2],
    pub cell_size: f32,
    pub cells: usize,
}

/// `nav.hash`：只回「这份网格是哪一份」的三级指纹。
///
/// 与 [`stats`] 分开是为了让缓存命中判定便宜：判断「要不要重烘」只需要指纹，
/// 不必把分类直方图与连通块表都算一遍。反过来，要排查「为什么哈希变了」时
/// 再看 `cfgDigest` / `inputDigest` 是哪一级动过。
pub fn hash(nav: &Nav) -> HashOutcome {
    let dims = nav.report().dims;
    HashOutcome {
        product_hash: nav.product_hex(),
        cfg_digest: format!("{:016x}", nav.cfg_digest()),
        input_digest: format!("{:016x}", nav.input_digest()),
        dims,
        cell_size: nav.report().cell_size,
        cells: nav.report().cells,
    }
}
