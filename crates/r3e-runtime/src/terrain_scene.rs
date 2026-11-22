//! 地形装配层（M2-C03b）：`Scene.terrain` → 分块网格 → GPU 顶点数组。
//!
//! # 为什么需要一个独立文件，而不是塞进 `app.rs`
//!
//! `app.rs` 在 `size_debt` 白名单里（2497 行 > 800 行阈值），**只许减不许涨**。地形的装配
//! 逻辑（读场景、算包围矩形、切块、按相机距离选 LOD、逐块生成网格、统计重建次数）实测
//! 两百多行，全塞进 `app.rs` 会直接把债务顶上去。所以规则是：`app.rs` 只留「声明模块 +
//! 调一次」，实现全在这个文件里。
//!
//! # 三层各管一段
//!
//! ```text
//! r3e-core::TerrainPatch   权威数据（高度样本、矩形、材质）—— 谁都不能改它
//!        ↓  r3e_terrain::{BlockGrid, block_mesh}     纯函数，无状态
//! 本装配层                  切块 / 选 LOD / 攒顶点 / 记事件
//!        ↓  r3e_render::terrain::{TerrainVertex}    裸数组
//! GPU
//! ```
//!
//! 本层**不持有 GPU 资源**：它产出的是「这帧该上传什么」的纯数据计划，`TerrainPass` 才是
//! 拿计划去写缓冲的那一方。这样在没有 GPU 的 CI 里，装配逻辑可以整段被测试覆盖。
//!
//! # 零地形零开销
//!
//! 场景没有地形块时 [`TerrainScene::plan`] 直接返回空计划，调用方据此**不创建管线**、
//! 每帧不调用任何 GPU 接口。地形是可选内容，不该让不用它的场景多付一份常驻成本。

use r3e_core::TerrainPatch;
use r3e_render::terrain::{normalise_weights, TerrainVertex};
use r3e_terrain::{
    block_mesh, lod_for, BlockGrid, Lod, LodConfig, TerrainLayerConfig, DEFAULT_BLOCK_SIZE,
    MAX_BLOCK_SIZE,
};

/// 一块地形的装配结果：已可直接上传的顶点/索引 + 本帧选中的 LOD。
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainBlock {
    /// 块索引 `[列, 行]`。
    pub index: [u32; 2],
    /// 世界矩形 `[min_x, min_z, max_x, max_z]`。
    pub rect: [f32; 4],
    /// 本帧选中档位。
    pub lod: Lod,
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

/// 装配层产出的事件：只用于观测，不参与判定。
///
/// 全部以「结构体 + 枚举」返回而不是 `log::warn!` 直接喷，是为了让测试能断言「非法配置
/// 到底记了几条」，而不是去抓日志。
#[derive(Clone, Debug, PartialEq)]
pub enum TerrainSceneEvent {
    /// 配置非法（块尺寸越界、矩形退化等），本帧整段跳过。
    InvalidConfig { reason: String },
    /// 某块没有任何 patch 覆盖，跳过它（不生成空网格）。
    BlockSkipped { index: [u32; 2] },
    /// 某块的 LOD 变了，需要重建顶点缓冲。
    LodChanged { index: [u32; 2], lod: Lod },
}

/// 装配层配置。默认值即「50 米块 + 默认 LOD 距离 + 默认层参数」。
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainSceneConfig {
    /// 块边长（米）。来自 D10 的可配置口子，默认 [`DEFAULT_BLOCK_SIZE`]。
    pub block_size: f32,
    /// LOD 距离阈值。
    pub lod: LodConfig,
    /// 4 层材质的原始分参数。
    pub layers: TerrainLayerConfig,
}

impl Default for TerrainSceneConfig {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            lod: LodConfig::default(),
            layers: TerrainLayerConfig::default(),
        }
    }
}

/// 一帧的装配计划。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainPlan {
    pub blocks: Vec<TerrainBlock>,
    /// 本帧发生的事件。
    pub events: Vec<TerrainSceneEvent>,
    /// 需要重建顶点缓冲的块数（调用方拿它决定要不要 `upload`）。
    pub rebuilds: usize,
    /// 装配层累计重建次数（跨帧累加，测试 9 用它核对「一次 LOD 变化 = 一次重建」）。
    pub total_rebuilds: u64,
}

impl TerrainPlan {
    /// 没有可画的块：调用方应据此跳过管线创建与绘制调用。
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn vertex_count(&self) -> usize {
        self.blocks.iter().map(|b| b.vertices.len()).sum()
    }

    pub fn index_count(&self) -> usize {
        self.blocks.iter().map(|b| b.indices.len()).sum()
    }
}

/// 装配器。持有跨帧状态：每块上一帧的 LOD（用于判断要不要重建）。
#[derive(Debug, Default)]
pub struct TerrainScene {
    config: TerrainSceneConfig,
    /// 上一帧每块（按 `[列, 行]` 扁平下标）的 LOD；`None` 表示上一帧没有这块。
    last_lod: Vec<Option<Lod>>,
    /// 上一帧的网格列数，块表长度随之变化。
    last_dims: [u32; 2],
    total_rebuilds: u64,
}

impl TerrainScene {
    pub fn new(config: TerrainSceneConfig) -> Self {
        Self {
            config,
            last_lod: Vec::new(),
            last_dims: [0, 0],
            total_rebuilds: 0,
        }
    }

    pub fn config(&self) -> &TerrainSceneConfig {
        &self.config
    }

    pub fn set_block_size(&mut self, block_size: f32) {
        self.config.block_size = block_size;
        // 块尺寸变了，块表整体作废：不重置会让新旧下标互相当成同一块
        self.last_lod.clear();
        self.last_dims = [0, 0];
    }

    /// 生成这一帧的装配计划。
    ///
    /// `eye_xz` 是相机在 XZ 平面的位置，只用于选 LOD；给 `None` 表示「没有相机」
    /// （headless / 离线烘焙），此时**一律按 L0 全精度**装配——离线产物要能直接对比，
    /// 不能让「相机在哪」影响结果。
    pub fn plan(&mut self, patches: &[TerrainPatch], eye_xz: Option<[f32; 2]>) -> TerrainPlan {
        let mut events = Vec::new();

        // 零地形：直接空计划，连 grid 都不建
        if patches.is_empty() {
            self.last_lod.clear();
            self.last_dims = [0, 0];
            return TerrainPlan::default();
        }

        let Some(grid) = self.make_grid(patches, &mut events) else {
            return TerrainPlan {
                events,
                total_rebuilds: self.total_rebuilds,
                ..Default::default()
            };
        };

        let dims = grid.dims();
        if dims != self.last_dims {
            // 块表尺寸变了：整表重置，避免把「第 3 块」错当成上一帧的第 3 块
            self.last_lod.clear();
            self.last_lod.resize((dims[0] * dims[1]) as usize, None);
            self.last_dims = dims;
        }

        let mut plan = TerrainPlan {
            total_rebuilds: 0,
            ..Default::default()
        };
        for block in grid.blocks() {
            let flat = (block.index[1] * dims[0] + block.index[0]) as usize;
            let lod = match eye_xz {
                // 块中心到相机距离 → 档位
                Some(eye) => {
                    let cx = (block.rect[0] + block.rect[2]) * 0.5;
                    let cz = (block.rect[1] + block.rect[3]) * 0.5;
                    let dx = cx - eye[0];
                    let dz = cz - eye[1];
                    lod_for((dx * dx + dz * dz).sqrt(), &self.config.lod)
                }
                None => Lod::L0,
            };

            let Some(mesh) = block_mesh(patches, &grid, block.index, lod, &self.config.layers)
            else {
                // 这块没有任何 patch 覆盖：跳过，不生成空网格
                events.push(TerrainSceneEvent::BlockSkipped { index: block.index });
                if let Some(slot) = self.last_lod.get_mut(flat) {
                    *slot = None;
                }
                continue;
            };

            let previous = self.last_lod.get(flat).copied().flatten();
            if previous != Some(lod) {
                events.push(TerrainSceneEvent::LodChanged {
                    index: block.index,
                    lod,
                });
                plan.rebuilds += 1;
                self.total_rebuilds += 1;
            }
            if let Some(slot) = self.last_lod.get_mut(flat) {
                *slot = Some(lod);
            }

            // `block_mesh` 已保证权重归一化；这里再过一遍 `normalise_weights` 是入口守卫，
            // 与 `r3e_render::terrain::pack_vertices` 的口径完全一致（同一函数）。
            debug_assert!(mesh.weights.iter().all(|w| {
                let sum: f32 = w.iter().sum();
                (sum - 1.0).abs() < 1e-4
            }));
            let vertices = mesh
                .positions
                .iter()
                .zip(&mesh.normals)
                .zip(&mesh.uvs)
                .zip(&mesh.weights)
                .map(|(((p, n), uv), w)| TerrainVertex {
                    position: *p,
                    normal: *n,
                    uv: *uv,
                    weights: normalise_weights(*w),
                })
                .collect();

            plan.blocks.push(TerrainBlock {
                index: block.index,
                rect: block.rect,
                lod,
                vertices,
                indices: mesh.indices,
            });
        }

        plan.total_rebuilds = self.total_rebuilds;
        plan.events = events;
        plan
    }

    /// 由所有 patch 的并集矩形切块。配置非法时记一条事件并返回 `None`。
    fn make_grid(
        &self,
        patches: &[TerrainPatch],
        events: &mut Vec<TerrainSceneEvent>,
    ) -> Option<BlockGrid> {
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
            events.push(TerrainSceneEvent::InvalidConfig {
                reason: format!("地形包围矩形退化：{rect:?}"),
            });
            return None;
        }

        let block_size = self.config.block_size;
        if !block_size.is_finite() || block_size <= 0.0 || block_size > MAX_BLOCK_SIZE {
            // 非法块尺寸不是「用默认值兜底」而是「这帧不画地形」：配置写错却照常出画，
            // 会让人以为配置生效了
            events.push(TerrainSceneEvent::InvalidConfig {
                reason: format!("块尺寸非法：{block_size}"),
            });
            return None;
        }

        match BlockGrid::new(rect, block_size) {
            Some(grid) => Some(grid),
            None => {
                events.push(TerrainSceneEvent::InvalidConfig {
                    reason: format!("矩形 {rect:?} 按 {block_size} 米切块超出上限"),
                });
                None
            }
        }
    }
}
