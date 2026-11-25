//! # r3e-nav —— 导航子系统
//!
//! 把**场景几何**变成三样东西：一张可走位图、一组连通块、一条能走的路。
//! 数据是**运行时按场景自动生成**的（烘焙），不写进场景文件——场景里只声明
//! 「出生点在哪儿」，导航网格永远是它的派生物。这条口径的理由：
//!
//! * 场景文件里存一份 navmesh，等于给了两套真相，改一个物件忘了重烘就漂移；
//! * AI 写入闭环（M1 的主题）要的是「写完立刻能验证能不能走到」，派生数据才跟得上；
//! * 烘焙是确定性的，同一份场景烘两次结果必须逐位相同（可进产物哈希）。
//!
//! ## 用法
//!
//! ```
//! use glam::Vec3;
//! use r3e_core::{Scene, SpawnDecl, TerrainPatch};
//! use r3e_nav::{spawn_point, Nav};
//!
//! let mut scene = Scene::new();
//! scene
//!     .terrain
//!     .push(TerrainPatch::flat("ground", [0.0, 0.0], [16.0, 16.0]));
//! scene.upsert_spawn(SpawnDecl::new("start", [2.0, 0.0, 2.0]));
//! scene.update_world_transforms();
//!
//! let nav = Nav::bake_scene(&scene).expect("场景可烘焙");
//! let start = spawn_point(&scene, "start").unwrap();
//! assert!(nav.is_walkable(start));
//! assert!(nav.reachable(start, Vec3::new(12.0, 0.0, 12.0)));
//! // 1.5 m 高的墙把场景切成两块：走过去要绕，直接穿不过去。
//! ```
//!
//! ## 口径与边界
//!
//! * **2.5D**：每个 XZ 格一个地面高度 + 一个「站得住」的 bit，不做垂直分层
//!   （上层平台压住下层地面的场景，M1 按「取最高层」处理）。
//! * **保守优先**：估不准就判不可走。宁可让关卡作者看到「这儿走不了」去补几何，
//!   也不要给出一个实际会被卡住的假绿结论。
//! * **外部网格资产**没有几何可查（`asset:` 网格的真实形状在资产文件里），
//!   只能按 `scale * 0.5` 或场景显式 `physics.half_extents` 估，
//!   这类资产名会进 [`NavReport::unresolved`]——**别把带未解析资产的结论当准的**。
//! * **不依赖** render / runtime / doc：只有 `core`（场景数据）与 `physics`
//!   （碰撞形状语义），所以渲染不启动也能在工具链里烘焙与校验。

#![forbid(unsafe_code)]

mod bake;
mod grid;
mod path;
mod product;

use glam::Vec3;
use r3e_core::{Scene, SpawnDecl};
use r3e_physics::{HeightField, Physics};

pub use grid::{NavGrid, NAV_MAX_CELLS, NAV_MAX_SIDE};
pub use path::{Endpoint, PathOutcome, NAV_PATH_MAX_EXPANSIONS};
pub use product::{CellClass, NavProduct, NAV_FORMAT};

/// 场景 → 高度场（真值链第一段）。想先看地形被采成什么样，从这里取。
pub use bake::field_of;

/// 烘焙参数。所有长度单位都是世界单位（米）。
#[derive(Debug, Clone, PartialEq)]
pub struct NavConfig {
    /// 格子边长。越小越精细，内存按平方增长（0.5 m 时 1 km² 约 400 万格）。
    pub cell_size: f32,
    /// 角色半径：障碍按这个半径外扩，等价于「身体过不去的缝不算可走」。
    pub agent_radius: f32,
    /// 角色身高：头顶低于这个高度的横梁不挡路。
    pub agent_height: f32,
    /// 单步能跨上的高度差。
    pub step_height: f32,
    /// 可行走坡度上限（度）。
    pub max_slope_deg: f32,
    /// 烘焙范围 `[min_x, min_z, max_x, max_z]`；`None` = 按场景内容自动取。
    pub bounds: Option<[f32; 4]>,
    /// 带这些标签的节点不算静态障碍（会动的东西不该被烘进导航数据）。
    pub ignore_tags: Vec<String>,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            cell_size: 0.5,
            agent_radius: 0.4,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            bounds: None,
            ignore_tags: vec![
                "character".to_string(),
                "actor".to_string(),
                "player".to_string(),
                "npc".to_string(),
                "dynamic".to_string(),
            ],
        }
    }
}

impl NavConfig {
    /// 收敛到合法区间：非法值一律回默认，而不是让烘焙带着 NaN 跑出诡异结果。
    pub fn sanitized(mut self) -> Self {
        let def = Self::default();
        let fix = |v: f32, fallback: f32, lo: f32, hi: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                fallback
            }
        };
        self.cell_size = fix(self.cell_size, def.cell_size, 0.05, 16.0);
        self.agent_radius = fix(self.agent_radius, def.agent_radius, 0.0, 8.0);
        self.agent_height = fix(self.agent_height, def.agent_height, 0.1, 16.0);
        self.step_height = fix(self.step_height, def.step_height, 0.0, 8.0);
        self.max_slope_deg = fix(self.max_slope_deg, def.max_slope_deg, 0.0, 89.0);
        self.bounds = self.bounds.filter(|b| b.iter().all(|v| v.is_finite()));
        self.ignore_tags = self
            .ignore_tags
            .iter()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        self
    }

    /// 该标签的节点是否应排除在静态障碍之外。
    pub fn is_ignored_tag(&self, tag: &str) -> bool {
        let t = tag.trim().to_ascii_lowercase();
        self.ignore_tags.iter().any(|x| x == &t)
    }

    /// 链式指定烘焙范围。
    pub fn with_bounds(mut self, bounds: [f32; 4]) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// 链式指定格边长。
    pub fn with_cell_size(mut self, cell: f32) -> Self {
        self.cell_size = cell;
        self
    }
}

/// 烘焙失败的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum NavError {
    /// 烘焙范围非法（反向、非有限值）。
    InvalidBounds { reason: String },
    /// 规模超限（格数或单轴格数），提示调用方放大 `cell_size` 或缩小范围。
    TooLarge { cells: u64, limit: u64 },
    /// 高度场构造失败：范围非法、样本数与分辨率不符、或高度含非有限值。
    FieldInvalid { reason: String },
    /// 后端没有按「装上去的那份」还回来——换后端会改变烘焙输入，D01 的防线。
    BackendMismatch { backend: String, reason: String },
    /// 产物格式不兼容（魔数/版本/长度/编码），拒绝半懂不懂地读。
    ProductMismatch { format: u16, reason: String },
}

impl std::fmt::Display for NavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBounds { reason } => write!(f, "导航烘焙范围非法：{reason}"),
            Self::TooLarge { cells, limit } => write!(
                f,
                "导航烘焙规模超限：{cells} 格 > 上限 {limit}，请放大 cell_size 或缩小范围"
            ),
            Self::FieldInvalid { reason } => write!(f, "导航高度场非法：{reason}"),
            Self::BackendMismatch { backend, reason } => {
                write!(f, "物理后端 {backend} 的高度场不一致：{reason}")
            }
            Self::ProductMismatch { format, reason } => {
                write!(f, "导航产物格式 {format} 无法读取：{reason}")
            }
        }
    }
}

impl std::error::Error for NavError {}

/// 单个连通块的统计（`id` 从 1 开始）。
#[derive(Debug, Clone, PartialEq)]
pub struct RegionStat {
    pub id: u32,
    /// 块内可走格数。
    pub cells: u32,
    /// 世界空间包围盒下界 `[x, y, z]`。
    pub min: [f32; 3],
    /// 世界空间包围盒上界 `[x, y, z]`。
    pub max: [f32; 3],
}

/// 一次烘焙的体检报告：拿它判断「这份导航数据可不可信」。
#[derive(Debug, Clone, PartialEq)]
pub struct NavReport {
    pub dims: [u32; 2],
    pub cell_size: f32,
    pub cells: usize,
    pub walkable: usize,
    pub regions: u32,
    /// 各连通块的格数，按标注顺序（`[0]` 是第一个被扫到的块，不保证最大）。
    pub region_sizes: Vec<u32>,
    /// 出生点 → 所在连通块（`0` = 出生点不在任何可走格上，即悬空/埋进几何）。
    pub spawns: Vec<(String, u32)>,
    /// 按 `scale * 0.5` 估形状的外部网格资产名：这些几何是猜的。
    pub unresolved: Vec<String>,
}

impl NavReport {
    /// 可走面积占比（0..=1）。
    pub fn walkable_ratio(&self) -> f32 {
        if self.cells == 0 {
            return 0.0;
        }
        self.walkable as f32 / self.cells as f32
    }

    /// 悬空的出生点名字（所在格不可走）。
    pub fn floating_spawns(&self) -> Vec<&str> {
        self.spawns
            .iter()
            .filter(|(_, r)| *r == 0)
            .map(|(n, _)| n.as_str())
            .collect()
    }

    /// 一句话摘要，供日志与校验报告使用。
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{}×{} 格（{:.2} m）：可走 {} / {}（{:.1}%），{} 个连通块",
            self.dims[0],
            self.dims[1],
            self.cell_size,
            self.walkable,
            self.cells,
            self.walkable_ratio() * 100.0,
            self.regions
        );
        let floating = self.floating_spawns();
        if !floating.is_empty() {
            s.push_str(&format!("；悬空出生点：{}", floating.join(", ")));
        }
        if !self.unresolved.is_empty() {
            s.push_str(&format!("；未解析资产 {} 个", self.unresolved.len()));
        }
        s
    }
}

/// 烘焙好的导航数据 + 它的体检报告。
///
/// 除了网格本身，还带着**两级输入摘要**（[`Self::cfg_digest`] / [`Self::input_digest`]）：
/// 产物哈希对不上时，先看是哪一级变了，就知道该怀疑参数还是场景内容。
#[derive(Debug, Clone)]
pub struct Nav {
    grid: NavGrid,
    report: NavReport,
    cfg_digest: u64,
    input_digest: u64,
}

impl Nav {
    /// 用默认参数烘焙一个场景。
    pub fn bake(scene: &Scene) -> Result<Self, NavError> {
        Self::bake_with(scene, NavConfig::default())
    }

    /// 与 [`Self::bake`] 同义，用「场景 → 导航数据」这个动作本身命名。
    ///
    /// S4-C 把它作为公共装配入口（与 `Physics::bind_scene` 对称），
    /// 这两个名字都要留：老调用点写 `bake`，新代码写 `bake_scene`。
    pub fn bake_scene(scene: &Scene) -> Result<Self, NavError> {
        Self::bake(scene)
    }

    /// 与 [`Self::bake_with`] 同义，用动作命名 + 显式参数，和 `bake_scene` 配成一对。
    ///
    /// 编辑器改格子参数、验证器按内容自带的 `nav` 段烘焙时都走这里，
    /// 不要各自 `NavConfig { .. }` 拼完再调 `bake`——那样参数收敛
    /// （[`NavConfig::sanitized`]）就有被绕过的机会。
    pub fn bake_scene_with(scene: &Scene, cfg: NavConfig) -> Result<Self, NavError> {
        Self::bake_with(scene, cfg.sanitized())
    }

    /// 用指定参数烘焙（内部先经 [`field_of`] 造场，再到 [`Self::bake_with_field`]）。
    pub fn bake_with(scene: &Scene, cfg: NavConfig) -> Result<Self, NavError> {
        let baked = bake::bake(scene, cfg)?;
        Ok(Self::from_baked(scene, baked))
    }

    /// 用一份**已经构造好的高度场**烘焙（M2 · D01 的唯一烘焙入口）。
    ///
    /// 「两后端各烘一次、产物逐位相同」靠的就是这个函数：后端只负责把场装上/取回，
    /// 真正的烘焙只有这里一条路径。网格的范围与格长取自场本身，
    /// `cfg` 只提供角色半径/步高/坡度等**判定参数**，不再管范围——
    /// 否则「装进去的场」和「烘出来的网格」可以描述两个不同的世界。
    pub fn bake_with_field(
        scene: &Scene,
        field: &HeightField,
        cfg: NavConfig,
    ) -> Result<Self, NavError> {
        let baked = bake::bake_with_field(scene, field, cfg)?;
        Ok(Self::from_baked(scene, baked))
    }

    /// 经物理后端烘焙：场景 → 高度场 → `set_heightfield` 装进 `physics` →
    /// 取回（必须是装上去的那份）→ 烘焙。
    ///
    /// 后端没保留场时返回 [`NavError::BackendMismatch`] 而不是退回本地场：
    /// 静默退回等于把「后端不一致」永久藏起来。
    pub fn bake_with_backend(
        scene: &Scene,
        cfg: NavConfig,
        physics: &mut Physics,
    ) -> Result<Self, NavError> {
        let baked = bake::bake_via_backend(scene, cfg, physics)?;
        Ok(Self::from_baked(scene, baked))
    }

    fn from_baked(scene: &Scene, baked: bake::Baked) -> Self {
        let bake::Baked {
            grid,
            cfg_digest,
            input_digest,
        } = baked;
        let report = NavReport {
            dims: grid.dims(),
            cell_size: grid.config().cell_size,
            cells: grid.cell_count(),
            walkable: grid.walkable_count(),
            regions: grid.region_count(),
            region_sizes: grid.region_sizes().to_vec(),
            spawns: scene
                .spawns
                .iter()
                .map(|s| {
                    let p = position_of(s);
                    (s.name.clone(), grid.region_of(p))
                })
                .collect(),
            unresolved: grid.unresolved().to_vec(),
        };
        Self {
            grid,
            report,
            cfg_digest,
            input_digest,
        }
    }

    /// 收敛后参数（[`NavConfig`]）的摘要。
    pub fn cfg_digest(&self) -> u64 {
        self.cfg_digest
    }

    /// 烘焙输入（地形样本 + 障碍盒 + 未解析资产）的摘要。
    pub fn input_digest(&self) -> u64 {
        self.input_digest
    }

    /// 把整张网格摊平成一份可序列化、可逐位比较的产物（M2 · D01）。
    pub fn product(&self) -> NavProduct {
        self.grid.product(self.cfg_digest, self.input_digest)
    }

    /// 产物哈希：同一份输入烘两次必须逐位相同；换后端不改哈希（构造保证）。
    pub fn product_hash(&self) -> u64 {
        self.product().product_hash()
    }

    /// 产物哈希的十六进制形式（16 位定长小写）。
    pub fn product_hex(&self) -> String {
        self.product().hash_hex()
    }

    pub fn grid(&self) -> &NavGrid {
        &self.grid
    }

    pub fn report(&self) -> &NavReport {
        &self.report
    }

    /// 世界点是否站得住。
    pub fn is_walkable(&self, p: Vec3) -> bool {
        self.grid.is_walkable(p)
    }

    /// 世界点的地面高度（越界 `None`）。
    pub fn ground_height(&self, p: Vec3) -> Option<f32> {
        self.grid.ground_at(p)
    }

    /// 世界点所在连通块（`0` = 不可走）。
    pub fn region_of(&self, p: Vec3) -> u32 {
        self.grid.region_of(p)
    }

    /// 距 `p` 最近的可走点；`region` 给定时限定在同一连通块内。
    pub fn nearest_walkable(&self, p: Vec3, max_radius: f32, region: Option<u32>) -> Option<Vec3> {
        self.grid.nearest_walkable(p, max_radius, region)
    }

    /// 求路径。端点在不可走处时先做一格吸附，仍不行则报 `InvalidEndpoint`。
    pub fn path(&self, from: Vec3, to: Vec3) -> PathOutcome {
        path::find_path(&self.grid, from, to)
    }

    /// 是否可达（只有真找到路才算可达；`BudgetExceeded` 不算）。
    pub fn reachable(&self, from: Vec3, to: Vec3) -> bool {
        self.path(from, to).is_found()
    }

    /// 可达性诊断：从 `from` 能走到、且离 `to` 最近的点。
    pub fn frontier(&self, from: Vec3, to: Vec3) -> Option<Vec3> {
        path::frontier_nearest(&self.grid, from, to)
    }

    /// 每个连通块的统计（供 `nav.reachability` 直接回包）。
    ///
    /// 按标注顺序返回，`id` 从 1 开始（`0` 是「不可走」的哨兵值，不出现在这里）。
    pub fn regions(&self) -> Vec<RegionStat> {
        let mut out: Vec<RegionStat> = Vec::new();
        let dims = self.grid.dims();
        for j in 0..dims[1] {
            for i in 0..dims[0] {
                let id = self.grid.region_at(i, j);
                if id == 0 {
                    continue;
                }
                let c = self.grid.cell_center(i, j);
                let half = self.grid.config().cell_size * 0.5;
                let lo = [c.x - half, c.y, c.z - half];
                let hi = [c.x + half, c.y, c.z + half];
                match out.iter_mut().find(|r| r.id == id) {
                    Some(r) => {
                        r.cells += 1;
                        for k in 0..3 {
                            r.min[k] = r.min[k].min(lo[k]);
                            r.max[k] = r.max[k].max(hi[k]);
                        }
                    }
                    None => out.push(RegionStat {
                        id,
                        cells: 1,
                        min: lo,
                        max: hi,
                    }),
                }
            }
        }
        out.sort_by_key(|r| r.id);
        out
    }

    /// 两个连通块之间「断在哪儿」的坐标：从 `from` 出发能到达、且离 `to` 最近的点。
    ///
    /// 这就是「不可达关卡」失败点位的来源——只报 `false` 没用，
    /// 关卡作者要知道把路补在哪。
    pub fn disconnect_point(&self, from: Vec3, to: Vec3) -> Option<Vec3> {
        self.frontier(from, to)
    }

    /// 可达性 + 人话说明，适合直接塞进场景校验的 issue 消息里。
    pub fn check_reach(&self, from: Vec3, to: Vec3) -> (bool, String) {
        let outcome = self.path(from, to);
        let ok = outcome.is_found();
        let mut msg = outcome.describe();
        if !ok {
            if let Some(p) = self.frontier(from, to) {
                msg.push_str(&format!(
                    "；从起点最多走到 ({:.2}, {:.2}, {:.2})",
                    p.x, p.y, p.z
                ));
            }
        }
        (ok, msg)
    }
}

/// 出生点的世界坐标。
pub fn spawn_point(scene: &Scene, name: &str) -> Option<Vec3> {
    scene.spawn_decl(name).map(position_of)
}

/// 默认出生点（场景里的第一个出生点）的世界坐标。
pub fn default_spawn_point(scene: &Scene) -> Option<Vec3> {
    scene.first_spawn().map(position_of)
}

/// 场景里所有出生点的世界坐标。
pub fn spawn_points(scene: &Scene) -> Vec<(String, Vec3)> {
    scene
        .spawns
        .iter()
        .map(|s| (s.name.clone(), position_of(s)))
        .collect()
}

fn position_of(s: &SpawnDecl) -> Vec3 {
    Vec3::new(s.position[0], s.position[1], s.position[2])
}
