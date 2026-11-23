//! 地形命令面的引擎侧实现（M2-C06）：`terrain.*` 八条的重活全在这里。
//!
//! 为什么不在 `r3e-editor-host` 里直接写：那八条里有一半要碰派生几何
//! （块划分 / LOD / 采样 / 三角网），而 `r3e-editor-host` 的 `allow` 是
//! `["r3e-core", "r3e-editor", "r3e-runtime"]` —— 它**不依赖** `r3e-terrain`（CD-2 的红线）。
//! 于是本模块承担「协议字节 ↔ `r3e-terrain` 纯函数」的翻译，宿主只做
//! 「参数校验 → 调本模块 → 拼 JSON / 映射错误码」，脚本宿主（`api/terrain.rs` 的
//! `terrain_sample` / `terrain_stats`）也能复用同一份实现。
//!
//! ## 三条不变式（八条命令共同遵守）
//!
//! 1. **真值只有 `Scene::terrain`**：`create` / `remove` / `resample` / `gen` 经
//!    [`CommandKind`] 写入（可撤销、进 `state_hash`、走事务）；`sample` / `blocks` /
//!    `stats` / `mesh` 一个字节都不写。
//! 2. **「没有这块地形」≠「这块地形是平的」**：前者 [`TerrainOpError::PatchMissing`]，
//!    后者高度 `0.0`。混成同一件事，调用方会照着零点去摆物件。
//! 3. **同名覆盖、上限只拦新增**：与 `core::upsert_patch` 同一语义，本模块不另立规则。
//!
//! ## 派生量永远现算
//!
//! `stats` 的 `triangles` / `vertices` 与 `mesh` 走**同一个**内部函数
//! （[`mesh_totals`]），因此「逐块取网格再求和」必须等于 `stats` 的数。两处算法一旦
//! 分叉，测试立刻红——这是刻意的，不是巧合。

use r3e_core::terrain::find_patch;
use r3e_core::TerrainPatch;
use r3e_doc::{CommandKind, DocError, Document, StateHash};
use r3e_terrain::{
    block_mesh, BlockGrid, Lod, TerrainLayerConfig, DEFAULT_BLOCK_SIZE, LOD_STEPS, MAX_BLOCK_SIZE,
};

/// 中心差分法线的步长（米）。与 `r3e-nav` 烘焙期的口径同量级：
/// 太大抹平地堑、太小把噪声当坡。地形网格的采样间距在 0.1~16 米之间，
/// 0.5 米落在「比最细网格粗、比整体起伏细」的中间。
pub const NORMAL_EPS: f32 = 0.5;

/// `stats` / `mesh` 缺省用的顶点步长档位（1 = 全分辨率）。
pub const DEFAULT_LOD: Lod = Lod::L0;

/// 地形命令面的引擎侧错误。
///
/// 承载「发生了什么」而不是「该回哪个码」：协议码（`TERRAIN_PATCH_MISSING` /
/// `TERRAIN_BLOCK_MISSING` / `-32602`）由各个宿主自己映射，本 crate 不认识协议。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerrainOpError {
    /// 场景里没有这块地形（名字不对，或刚被删）。
    PatchMissing(String),
    /// 块索引越界 / 该块没有任何地形覆盖。
    BlockMissing(String),
    /// 参数本身不合法（空名、尺寸为 0、分辨率越界、步长非法……）。
    Invalid(String),
    /// 场景拒绝了这笔写入（表满、名字收敛后为空）。
    Rejected(String),
    /// 命令层其他失败（撤销栈 / 序列化）。
    Doc(String),
}

impl TerrainOpError {
    /// 给人看的一句话说明（回包里的 `detail` 用它）。
    pub fn detail(&self) -> &str {
        match self {
            TerrainOpError::PatchMissing(s)
            | TerrainOpError::BlockMissing(s)
            | TerrainOpError::Invalid(s)
            | TerrainOpError::Rejected(s)
            | TerrainOpError::Doc(s) => s,
        }
    }
}

fn map_doc(err: DocError) -> TerrainOpError {
    match err {
        DocError::Rejected(detail) => TerrainOpError::Rejected(detail),
        DocError::NotFound(detail) => TerrainOpError::PatchMissing(detail),
        other => TerrainOpError::Doc(other.to_string()),
    }
}

/// `terrain.create` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CreateOutcome {
    /// 写入后场景里的那块地形（已 `sanitized`，回包字段直接取它）。
    pub patch: TerrainPatch,
    /// 是否新建（`false` = 同名覆盖）。
    pub created: bool,
    /// 写之后的场景哈希。
    pub state_hash: StateHash,
}

/// `terrain.remove` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    /// 场景里还剩几块地形。
    pub remaining: usize,
    /// 写之后的场景哈希。
    pub state_hash: StateHash,
}

/// `terrain.sample` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SampleOutcome {
    /// 实际出数的块名（未指定名字时 = 给出最高层的那块；空地形为空串）。
    pub name: String,
    /// 地面高度（`inside == false` 时为 `0.0`，即「没有地形」）。
    pub height: f32,
    /// 单位法线（`inside == false` 时回竖直向上，明确是占位而非测量值）。
    pub normal: [f32; 3],
    /// 点是否落在（指定的 / 任意一块）地形矩形内。
    pub inside: bool,
    /// 该点的四层混合权重（草坪 / 岩石 / 沙 / 雪，和为 1）。
    ///
    /// 与 `r3e-terrain` 网格顶点用的是同一张表（[`TerrainLayerConfig::weights`]）：
    /// AI 想「在坡上铺岩石」时得先知道这个点是不是坡，凭空猜坡度会错得很难看。
    pub weights: [f32; 4],
}

/// `terrain.blocks` 里的一块。
#[derive(Debug, Clone, PartialEq)]
pub struct BlockOut {
    /// 块索引 `[列, 行]`。
    pub index: [u32; 2],
    /// 块的世界矩形 `[min_x, min_z, max_x, max_z]`。
    pub rect: [f32; 4],
    /// 覆盖该块的地形名（空 = 空白块）。
    pub patches: Vec<String>,
}

/// `terrain.blocks` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct BlocksOutcome {
    /// 整张地形的世界矩形；空地形为全 `0`。
    pub rect: [f32; 4],
    /// 块边长（米）。
    pub block_size: f32,
    /// 块数 `[列数, 行数]`；空地形为 `[0, 0]`。
    pub dims: [u32; 2],
    /// 总块数（含空白块）。
    pub count: u32,
    /// 有地形覆盖的块数。与 `count` 一起给：`4×4 块只铺 9 块` 与 `铺满 16 块`
    /// 在后续装配里差别很大。
    pub covered: u32,
    /// 逐块清单（含空白块）。
    pub blocks: Vec<BlockOut>,
}

/// `terrain.resample` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ResampleOutcome {
    /// 重采样前的分辨率。
    pub from: [u32; 2],
    /// 重采样后的分辨率（可能被 `maxResolution` / 全局上限夹过）。
    pub to: [u32; 2],
    /// 新网格的样本数（`to.0 * to.1`）。
    pub samples: usize,
    /// 同一批世界点上新旧网格的最大高度差（米）。
    pub max_error: f32,
    /// 写之后的场景哈希。
    pub state_hash: StateHash,
}

/// `terrain.gen` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenOutcome {
    /// 生成并写进场景的块数。
    pub created: usize,
    /// 生成的块名（行主序）。
    pub names: Vec<String>,
    /// 写之后的场景哈希。
    pub state_hash: StateHash,
}

/// `terrain.stats` 的结果（口径与 `terrain.mesh` 完全一致）。
#[derive(Debug, Clone, PartialEq)]
pub struct StatsOutcome {
    /// 地形块数。
    pub patches: usize,
    /// 高度样本总数。
    pub samples: usize,
    /// 切出来的总块数。
    pub blocks: u32,
    /// 有地形覆盖的块数。
    pub covered: u32,
    /// 全块（全分辨率）三角数。
    pub triangles: usize,
    /// 全块（全分辨率）顶点数。
    pub vertices: usize,
    /// 统计用的顶点步长档位（1 = 全分辨率）。
    pub lod: Lod,
    /// 切块用的块边长（米）。
    pub block_size: f32,
}

/// `terrain.mesh` 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MeshOutcome {
    /// 计算用的顶点步长档位。
    pub lod: Lod,
    /// 只算了某一块时回该块索引；汇总全部块时为 `None`。
    pub block: Option<[u32; 2]>,
    /// 实际产出网格的块数。
    pub blocks: u32,
    /// 顶点总数。
    pub vertices: usize,
    /// 三角总数。
    pub triangles: usize,
    /// 顶点包围盒 `[min_x, min_y, min_z, max_x, max_y, max_z]`；无顶点为 `None`。
    pub bounds: Option<[f32; 6]>,
}

// ── 公共几何小工具 ────────────────────────────────────────────────────────

/// 所有地形块的世界并集矩形 `[min_x, min_z, max_x, max_z]`。
///
/// 空表或并集退化（零面积）返回 `None`：一个零面积矩形切不出块，
/// 硬切会得到「1 块、永远没覆盖」这种看不出问题的假结果。
pub fn union_rect(patches: &[TerrainPatch]) -> Option<[f32; 4]> {
    let mut rect = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for patch in patches {
        rect[0] = rect[0].min(patch.origin[0]);
        rect[1] = rect[1].min(patch.origin[1]);
        rect[2] = rect[2].max(patch.origin[0] + patch.size[0]);
        rect[3] = rect[3].max(patch.origin[1] + patch.size[1]);
    }
    if !rect.iter().all(|v| v.is_finite()) || rect[2] <= rect[0] || rect[3] <= rect[1] {
        return None;
    }
    Some(rect)
}

/// 块矩形与某块地形的交集是否非空（只比面积，不看高度）。
fn overlaps(patch: &TerrainPatch, rect: [f32; 4]) -> bool {
    let (w, d) = (patch.size[0], patch.size[1]);
    // NaN 尺寸也在这条拦下：`w <= 0.0` 对 NaN 为假，靠 `is_finite` 兜住。
    if !w.is_finite() || !d.is_finite() || w <= 0.0 || d <= 0.0 {
        return false;
    }
    let ix = (rect[2].min(patch.origin[0] + w) - rect[0].max(patch.origin[0])).min(w);
    let iz = (rect[3].min(patch.origin[1] + d) - rect[1].max(patch.origin[1])).min(d);
    ix > 0.0 && iz > 0.0
}

fn cover_names(patches: &[TerrainPatch], rect: [f32; 4]) -> Vec<String> {
    patches
        .iter()
        .filter(|p| overlaps(p, rect))
        .map(|p| p.name.clone())
        .collect()
}

fn block_size_or_default(given: Option<f32>) -> Result<f32, TerrainOpError> {
    let size = given.unwrap_or(DEFAULT_BLOCK_SIZE);
    if !size.is_finite() || size <= 0.0 || size > MAX_BLOCK_SIZE {
        return Err(TerrainOpError::Invalid(format!(
            "块边长必须在 (0, {MAX_BLOCK_SIZE}] 米之间，收到 {size}"
        )));
    }
    Ok(size)
}

fn grid_of(patches: &[TerrainPatch], block_size: f32) -> Result<BlockGrid, TerrainOpError> {
    let rect = union_rect(patches).ok_or_else(|| {
        TerrainOpError::Invalid("地形包围矩形退化（空表或零面积），切不出块".to_string())
    })?;
    BlockGrid::new(rect, block_size).ok_or_else(|| {
        TerrainOpError::Invalid(format!(
            "矩形 {rect:?} 按 {block_size} 米切块超出上限（块数或矩形非法）"
        ))
    })
}

/// 把「步长倍数」收敛到合法档位：非 1/2/4 的值取最近的合法值（等距取较细）。
fn lod_of(step: f32) -> Result<Lod, TerrainOpError> {
    if !step.is_finite() || step < 1.0 {
        return Err(TerrainOpError::Invalid(format!(
            "顶点步长倍数必须 ≥ 1，收到 {step}"
        )));
    }
    let want = if step >= LOD_STEPS[2] as f32 {
        LOD_STEPS[2]
    } else {
        let mut best = LOD_STEPS[0];
        let mut best_gap = f32::INFINITY;
        for s in LOD_STEPS {
            let gap = (s as f32 - step).abs();
            if gap < best_gap {
                best_gap = gap;
                best = s;
            }
        }
        best
    };
    Lod::from_step(want)
        .ok_or_else(|| TerrainOpError::Invalid(format!("步长 {} 不是合法档位", want)))
}

/// 逐块算网格并汇总顶点 / 三角 / 包围盒。
///
/// `only` 指定单块时只算那一块（块索引越界 = `BlockMissing`，块无覆盖也 = `BlockMissing`：
/// 「这块没地形」必须报出来，不能回一个 0 顶点 0 三角的"成功"）。
fn mesh_totals(
    patches: &[TerrainPatch],
    grid: &BlockGrid,
    lod: Lod,
    only: Option<[u32; 2]>,
) -> Result<MeshOutcome, TerrainOpError> {
    let cfg = TerrainLayerConfig::default();
    let mut vertices = 0_usize;
    let mut triangles = 0_usize;
    let mut blocks = 0_u32;
    let mut bounds: Option<[f32; 6]> = None;

    let indices: Vec<[u32; 2]> = match only {
        Some(index) => {
            if grid.block_rect(index[0], index[1]).is_none() {
                return Err(TerrainOpError::BlockMissing(format!(
                    "块索引 {index:?} 越界（dims {:?}）",
                    grid.dims()
                )));
            }
            vec![index]
        }
        None => grid.blocks().map(|b| b.index).collect(),
    };

    for index in indices {
        let Some(mesh) = block_mesh(patches, grid, index, lod, &cfg) else {
            // 汇总模式跳过空白块；单块模式已经在上面区分过「越界」，这里只能是没覆盖。
            if only.is_some() {
                return Err(TerrainOpError::BlockMissing(format!(
                    "块 {index:?} 没有任何地形覆盖"
                )));
            }
            continue;
        };
        blocks += 1;
        vertices += mesh.positions.len();
        triangles += mesh.indices.len() / 3;
        for p in &mesh.positions {
            let slot = bounds.get_or_insert([
                f32::INFINITY,
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ]);
            slot[0] = slot[0].min(p[0]);
            slot[1] = slot[1].min(p[1]);
            slot[2] = slot[2].min(p[2]);
            slot[3] = slot[3].max(p[0]);
            slot[4] = slot[4].max(p[1]);
            slot[5] = slot[5].max(p[2]);
        }
    }

    Ok(MeshOutcome {
        lod,
        block: only,
        blocks,
        vertices,
        triangles,
        bounds,
    })
}

// ── 子模块 ────────────────────────────────────────────────────────────────
//
// 拆成两个文件不是因为「一个域一个文件」的门面，而是 `lint-size` 的 800 行硬顶：
// 八条命令 + 共用几何 + 单测挤在一起正好越过阈值，而白名单只许减不许加。
// 对外沿用扁平路径（`terrain_ops::create`），调用方不需要知道文件怎么分。

mod read;
mod write;

pub use read::{blocks, mesh, sample, stats};
pub use write::{create, generate_into, remove, resample};

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use r3e_core::{Scene, TERRAIN_MAX_PATCHES};

    fn doc() -> Document {
        Document::new(Scene::default())
    }

    #[test]
    fn create_then_sample_flat() {
        let mut d = doc();
        let out = create(&mut d, "ground", [0.0, 0.0, 100.0, 100.0], None).unwrap();
        assert!(out.created);
        assert_eq!(out.patch.resolution, [1, 1]);
        let s = sample(&d, None, 10.0, 10.0).unwrap();
        assert!(s.inside);
        assert_eq!(s.height, 0.0);
        assert_eq!(s.name, "ground");
    }

    #[test]
    fn create_overwrites_and_reports_not_created() {
        let mut d = doc();
        create(&mut d, "ground", [0.0, 0.0, 10.0, 10.0], None).unwrap();
        let again = create(&mut d, "ground", [0.0, 0.0, 20.0, 20.0], Some(4)).unwrap();
        assert!(!again.created);
        assert_eq!(again.patch.resolution, [4, 4]);
        assert_eq!(d.as_scene().terrain.len(), 1);
    }

    #[test]
    fn create_rejects_bad_resolution_and_zero_size() {
        let mut d = doc();
        assert!(matches!(
            create(&mut d, "a", [0.0, 0.0, 10.0, 10.0], Some(0)),
            Err(TerrainOpError::Invalid(_))
        ));
        assert!(matches!(
            create(&mut d, "a", [0.0, 0.0, 10.0, 10.0], Some(129)),
            Err(TerrainOpError::Invalid(_))
        ));
        assert!(matches!(
            create(&mut d, "a", [0.0, 0.0, 0.0, 10.0], None),
            Err(TerrainOpError::Invalid(_))
        ));
        assert!(d.as_scene().terrain.is_empty());
    }

    #[test]
    fn create_rejects_33rd_patch_but_allows_overwrite() {
        let mut d = doc();
        for i in 0..TERRAIN_MAX_PATCHES {
            create(&mut d, &format!("p{i}"), [0.0, 0.0, 10.0, 10.0], None).unwrap();
        }
        assert!(matches!(
            create(&mut d, "overflow", [0.0, 0.0, 10.0, 10.0], None),
            Err(TerrainOpError::Rejected(_))
        ));
        // 覆盖已有块不受上限影响。
        assert!(create(&mut d, "p0", [0.0, 0.0, 12.0, 12.0], None).is_ok());
    }

    #[test]
    fn remove_is_undoable_and_reports_missing() {
        let mut d = doc();
        create(&mut d, "ground", [0.0, 0.0, 10.0, 10.0], None).unwrap();
        let before = d.state_hash();
        let out = remove(&mut d, "ground").unwrap();
        assert_eq!(out.remaining, 0);
        assert_ne!(before, out.state_hash);
        assert!(matches!(
            remove(&mut d, "ground"),
            Err(TerrainOpError::PatchMissing(_))
        ));
    }

    #[test]
    fn sample_outside_patch_is_not_zero_height() {
        let mut d = doc();
        create(&mut d, "ground", [0.0, 0.0, 10.0, 10.0], None).unwrap();
        let s = sample(&d, Some("ground"), 50.0, 50.0).unwrap();
        assert!(!s.inside);
        assert_eq!(s.height, 0.0);
        assert!(matches!(
            sample(&d, Some("nope"), 1.0, 1.0),
            Err(TerrainOpError::PatchMissing(_))
        ));
    }

    #[test]
    fn blocks_reports_dimensions_and_coverage() {
        let mut d = doc();
        create(&mut d, "ground", [0.0, 0.0, 200.0, 200.0], None).unwrap();
        create(&mut d, "top", [0.0, 0.0, 50.0, 50.0], None).unwrap();
        let out = blocks(&d, None).unwrap();
        assert_eq!(out.dims, [4, 4]);
        assert_eq!(out.count, 16);
        assert_eq!(out.covered, 16);
        assert_eq!(out.blocks[0].patches.len(), 2);
    }

    #[test]
    fn empty_terrain_is_empty_view_not_error() {
        let d = doc();
        let out = blocks(&d, None).unwrap();
        assert_eq!(out.count, 0);
        let st = stats(&d).unwrap();
        assert_eq!(st.patches, 0);
        let m = mesh(&d, None, None, None).unwrap();
        assert_eq!(m.vertices, 0);
        assert!(matches!(
            mesh(&d, Some([0, 0]), None, None),
            Err(TerrainOpError::BlockMissing(_))
        ));
    }

    #[test]
    fn gen_is_deterministic_and_lands_in_scene() {
        let mut a = doc();
        let mut b = doc();
        let out_a = generate_into(&mut a, 200.0, 200.0, 7).unwrap();
        let out_b = generate_into(&mut b, 200.0, 200.0, 7).unwrap();
        assert_eq!(out_a.names, out_b.names);
        assert_eq!(out_a.state_hash, out_b.state_hash);
        assert_eq!(a.as_scene().terrain.len(), out_a.created);
    }

    #[test]
    fn gen_rejects_bad_size() {
        let mut d = doc();
        assert!(matches!(
            generate_into(&mut d, -1.0, 10.0, 1),
            Err(TerrainOpError::Invalid(_))
        ));
        assert!(matches!(
            generate_into(&mut d, 4096.0, 4096.0, 1),
            Err(TerrainOpError::Invalid(_))
        ));
    }

    #[test]
    fn resample_halves_resolution_and_reports_error() {
        let mut d = doc();
        generate_into(&mut d, 200.0, 200.0, 3).unwrap();
        let name = d.as_scene().terrain[0].name.clone();
        let before = d.state_hash();
        let out = resample(&mut d, &name, 2.0, None).unwrap();
        assert_eq!(out.from, [64, 64]);
        assert_eq!(out.to, [32, 32]);
        assert_eq!(out.samples, 32 * 32);
        assert_ne!(before, out.state_hash);
        // 抽稀必然丢细节，误差应该是个真数而不是 0（除非地形恰好是线性）。
        assert!(out.max_error.is_finite());
    }

    #[test]
    fn resample_flat_patch_does_not_touch_scene() {
        let mut d = doc();
        create(&mut d, "flat", [0.0, 0.0, 100.0, 100.0], Some(8)).unwrap();
        let before = d.state_hash();
        let out = resample(&mut d, "flat", 2.0, None).unwrap();
        assert_eq!(out.to, [8, 8]);
        assert_eq!(out.max_error, 0.0);
        assert_eq!(before, out.state_hash);
    }

    #[test]
    fn stats_matches_mesh_totals() {
        let mut d = doc();
        generate_into(&mut d, 200.0, 200.0, 11).unwrap();
        let st = stats(&d).unwrap();
        let m = mesh(&d, None, None, None).unwrap();
        assert_eq!(st.vertices, m.vertices);
        assert_eq!(st.triangles, m.triangles);
        assert_eq!(st.covered, m.blocks);
    }

    #[test]
    fn coarser_lod_has_fewer_vertices() {
        let mut d = doc();
        generate_into(&mut d, 200.0, 200.0, 5).unwrap();
        let fine = mesh(&d, None, Some(1.0), None).unwrap();
        let coarse = mesh(&d, None, Some(2.0), None).unwrap();
        assert!(coarse.vertices < fine.vertices);
        assert!(coarse.triangles < fine.triangles);
    }

    #[test]
    fn lod_rounds_to_nearest_legal_step() {
        assert_eq!(lod_of(1.0).unwrap(), Lod::L0);
        assert_eq!(lod_of(2.2).unwrap(), Lod::L1);
        assert_eq!(lod_of(3.9).unwrap(), Lod::L2);
        assert_eq!(lod_of(99.0).unwrap(), Lod::L2);
        assert!(lod_of(0.5).is_err());
        assert!(lod_of(f32::NAN).is_err());
    }

    #[test]
    fn mesh_bounds_cover_patch_rect() {
        let mut d = doc();
        create(&mut d, "ground", [10.0, 20.0, 100.0, 100.0], None).unwrap();
        let m = mesh(&d, None, None, None).unwrap();
        let b = m.bounds.unwrap();
        assert!((b[0] - 10.0).abs() < 1e-3);
        assert!((b[2] - 20.0).abs() < 1e-3);
        assert!((b[3] - 110.0).abs() < 1e-3);
        assert!((b[5] - 120.0).abs() < 1e-3);
    }
}
