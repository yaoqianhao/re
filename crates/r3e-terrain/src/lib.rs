//! # r3e-terrain —— 地形派生层
//!
//! 把场景里的 [`r3e_core::TerrainPatch`]（平坦数据壳）变成能用来看、能用来走的几何：
//! **块划分 → 多分辨率 LOD → 高度/法线采样 → 三角网**。全部是**纯函数**：
//! 输入 `&[TerrainPatch]`，输出裸数组，不留任何自有状态。
//!
//! 采样有两档口径：全分辨率用 [`sample_height_at`]，按 LOD 降精度用
//! [`sample_height_lod`]（块内跳采、块边界带强制全采样，接缝两侧逐点相等）。
//! 三角网由 [`block_mesh`] 逐块生成：顶点高度即 `sample_height_lod` 的读数，
//! 法线由网格自身差分，4 层混合权重按 [`TerrainLayerConfig`] 程序化判定。
//!
//! ## 为什么是派生层（CD-2）
//!
//! 场景文件、存档、`state_hash`、撤销栈、校验器认的**唯一真值**始终是
//! `r3e_core::TerrainPatch`。地形网格是「按那份数据算出来的视图」，跟 navmesh 同一性质：
//! 存一份网格下来就等于给了两套真相，改一处忘了重算就漂移。
//!
//! 由此推出两条硬边界：
//!
//! * 本 crate **只依赖 `r3e-core`**（`dep_rules` 的 `allow` 仅此一项），
//!   不依赖 render / physics / asset —— 反过来 `core` 也绝不能依赖本 crate；
//! * 本 crate **不产出任何新场景类型**。要进场景的东西，先回 `core` 补契约。
//!
//! ## 三层职责（谁实现什么）
//!
//! | 层 | 职责 | 落到哪 |
//! |---|---|---|
//! | 数据 | 一块地表的高度样本 / 尺寸 / 材质 | `r3e-core::TerrainPatch`（M1 已有） |
//! | 派生几何 | 块划分、LOD、采样、三角网、权重 | **本 crate** |
//! | 呈现与碰撞 | 渲染管线 / 地形场查询 | `r3e-render` / `r3e-physics`（C03b / C04） |
//!
//! ## 用法
//!
//! ```
//! use r3e_core::TerrainPatch;
//! use r3e_terrain::{sample_height_at, BlockGrid, DEFAULT_BLOCK_SIZE};
//!
//! // 200 × 200 米的地面，按 50 米切块 → 4 × 4 = 16 块
//! let grid = BlockGrid::new([0.0, 0.0, 200.0, 200.0], DEFAULT_BLOCK_SIZE).unwrap();
//! assert_eq!(grid.dims(), [4, 4]);
//! assert!(grid.covers());
//!
//! let ground = TerrainPatch::flat("ground", [0.0, 0.0], [200.0, 200.0]);
//! assert_eq!(sample_height_at(&[ground], 12.0, 34.0), Some(0.0));
//! ```
//!
//! ## 单位与确定性
//!
//! 世界单位一律是米。采样是确定性的（双线性、同点同值），块划分与 LOD 只做整数索引，
//! 因此结果可以直接进产物哈希；同一份地形算两次必须逐位相同。

#![forbid(unsafe_code)]

mod block;
mod generate;
mod lod;
mod lod_sample;
mod mesh;
mod sample;

pub use block::{Block, BlockGrid};
pub use generate::{
    generate, GENERATE_JITTER, GENERATE_MAX_HEIGHT, GENERATE_MIN_HEIGHT, GENERATE_RESOLUTION,
    GENERATE_SAMPLES_PER_BLOCK, MAX_GENERATE_SIZE,
};
pub use lod::{lod_for, Lod, LodConfig, LOD_DISTANCES};
pub use lod_sample::sample_height_lod;
pub use mesh::{block_mesh, TerrainLayerConfig, TerrainMesh};
pub use sample::{sample_height_at, sample_normal};

/// 默认块边长（米）：200 × 200 米的地形按 50 米切 → 4 × 4 = 16 块，
/// 与 D10 拍板的「4×4=16 块、驻留 3×3」同一口径。可配置，见 [`BlockGrid::new`]。
pub const DEFAULT_BLOCK_SIZE: f32 = 50.0;

/// 块边长上限（米）。再大就没有 LOD 意义了，也挡住「一块盖住整张图」的退化用法。
pub const MAX_BLOCK_SIZE: f32 = 1000.0;

/// 一张地形最多切多少块。与 [`r3e_core::TERRAIN_MAX_PATCHES`] 同值：
/// 单块场景最多 32 片地表，切块粒度不该产出一张地图上千块网格。
pub const MAX_BLOCKS: u32 = 32;

/// LOD 三档的顶点步长（每档隔几个高度样本取一个顶点）。严格递增且均为 2 的幂，
/// 这样相邻档的顶点位置互相包含（不会出现「细档的顶点落在粗档网格缝里」）。
pub const LOD_STEPS: [u32; 3] = [1, 2, 4];
