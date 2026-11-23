//! `terrain.*` 八条命令的参数承载（M2-C06）。
//!
//! 协议层**零引擎依赖**，所以这里不出现 `r3e_core::TerrainPatch` / `BlockGrid`：
//! 参数是一组扁平数值，由宿主决定「没给 = 缺省值」还是「按场景现值」。
//!
//! 三条口径与 `character.*` 完全一致（写之前先看一遍 `character.rs`）：
//!
//! * **配置真源在场景**：`terrain.create` / `terrain.remove` / `terrain.resample` /
//!   `terrain.gen` 写的是 `Scene.terrain`，可撤销、进 `state_hash`；
//!   `terrain.sample` / `terrain.blocks` / `terrain.stats` / `terrain.mesh` 只读。
//! * **只读命令不写回**：查询类命令一个字节都不改场景，`stateHash` 前后必须一致。
//! * **区分「没有」和「不会」**：块不存在回具体失败，**绝不回全零采样值**糊过去——
//!   全零高度会被读成「这里就是平地」，然后照着它去摆物件。
//!
//! 参数名一律 camelCase（`blockSize` / `maxResolution` / `sizeX`），Rust 侧 snake_case；
//! 与 `character.*` 同一约定，映射由 dispatch 层显式写，不做隐式转换。

/// `terrain.create` 的入参：一块地表的名字、世界矩形与采样分辨率。
///
/// `rect` 是 `[x, z, size_x, size_z]`——左下角 + 尺寸，与 `TerrainPatch` 的
/// `origin` / `size` 一一对应，也和 `BlockGrid` 的矩形口径相同。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainCreateQuery {
    /// 块名（唯一主键；重复写同名 = 整体覆盖）。
    pub name: String,
    /// 世界矩形 `[x, z, sizeX, sizeZ]`。
    pub rect: [f32; 4],
    /// 采样分辨率（列数 = 行数）；`None` = 按场景现值 / 缺省（1）。
    pub resolution: Option<u32>,
}

/// 只带块名的入参（`terrain.remove`）。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainNameQuery {
    /// 块名。
    pub name: String,
}

/// `terrain.sample` 的入参：块名可空，空 = 在所有块里按坐标命中。
///
/// 「空名自动命中」是给 AI 用的顺手口子：它多半只知道自己站的坐标，
/// 不知道那块地表叫什么；让调用方先 `terrain.blocks` 再逐个试是多余的一步。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainSampleQuery {
    /// 块名；`None` / 空串 = 按坐标在所有块中命中。
    pub name: Option<String>,
    /// 世界坐标 X。
    pub x: f32,
    /// 世界坐标 Z。
    pub z: f32,
}

/// `terrain.blocks` 的入参：只有块尺寸，且可省。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainBlocksQuery {
    /// 块边长（米）；`None` = 宿主缺省（50 米，与 `r3e-terrain` 的 `DEFAULT_BLOCK_SIZE` 同值）。
    pub block_size: Option<f32>,
}

/// `terrain.resample` 的入参：按 LOD 步长重采样并写回场景。
///
/// `lod` 是**采样步长倍数**（1 = 全分辨率，2 = 隔点取一，4 = 隔三点取一），不是
/// 「档位序号」：档位到步长的映射住在 `r3e-terrain::Lod`，把它复制进协议层就等于
/// 给了第二份真值。传非 1/2/4 的值由宿主夹到最近的合法步长并如实回包。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainResampleQuery {
    /// 块名。
    pub name: String,
    /// 采样步长倍数（≥1）。
    pub lod: f32,
    /// 写回时的分辨率上限；`None` = 沿用当前分辨率（仍受 `TERRAIN_MAX_RESOLUTION` 约束）。
    pub max_resolution: Option<u32>,
}

/// `terrain.mesh` 的入参：可选地只算某一块，可选 LOD 步长。
///
/// 全缺省 = 汇总所有块（与 `terrain.stats` 同一口径，步长取 1），这是「我只想知道
/// 这块地形一共多少三角」的常用问法；给了 `block` 就只算那一块。
///
/// `block` 是整数块索引 `[i, j]`（不是世界坐标）：调用方多半刚读过 `terrain.blocks`，
/// 手里正握着索引；让它再把索引换算成世界坐标是多余的一步。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMeshQuery {
    /// 块索引 `[i, j]`；`None` = 汇总全部块。
    pub block: Option<[u32; 2]>,
    /// 采样步长倍数（≥1，缺省 1 = 全分辨率），语义与 `terrain.resample` 的 `lod` 同源。
    pub lod: Option<f32>,
    /// 块边长（米）；`None` = 宿主缺省（50 米）。
    pub block_size: Option<f32>,
}

/// `terrain.gen` 的入参：整张地形尺寸与随机种子。
///
/// 生成是**写场景**的重操作，登记在 `TX_ONLY_METHODS` 里：事务未决时不许走
/// `rpc` 直连（与 B01 的护栏同款）。
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainGenQuery {
    /// 整张地形 X 向尺寸（米）。
    pub size_x: f32,
    /// 整张地形 Z 向尺寸（米）。
    pub size_z: f32,
    /// 随机种子（同 seed 逐位可复现）。
    pub seed: u64,
}
