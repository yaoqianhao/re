//! `terrain.*` 里**只读**的四条：`sample` / `blocks` / `stats` / `mesh`。
//!
//! 一个字节都不写场景：派生量永远现算（`stats` 与 `mesh` 走同一个 `mesh_totals`，
//! 所以「逐块取网格再求和」必然等于 `stats` 的数）。

use super::*;
use r3e_terrain::{sample_height_at, sample_normal, TerrainLayerConfig, DEFAULT_BLOCK_SIZE};

// ── 只读的四条 ────────────────────────────────────────────────────────────

/// 由高度与法线算四层权重，坡度取 `acos(n.y)`。
///
/// 缺省层表在这里直接取 [`TerrainLayerConfig::default`]：`sample` 不接层配置参数
/// （协议里没有这个字段），拿一个「用户改过的配置」去算会让回包与网格对不上。
fn layer_weights(height: f32, normal: [f32; 3]) -> [f32; 4] {
    let slope = normal[1].clamp(-1.0, 1.0).acos().to_degrees();
    TerrainLayerConfig::default().weights(height, slope)
}

/// `terrain.sample`：世界点高度 + 法线。
///
/// 给了 `name` 只算那一块（点落在块外回 `inside: false`）；不给则跨全部块取最高层
/// （与 `r3e-nav` 烘焙期同一裁决）。
pub fn sample(
    doc: &Document,
    name: Option<&str>,
    x: f32,
    z: f32,
) -> Result<SampleOutcome, TerrainOpError> {
    if !x.is_finite() || !z.is_finite() {
        return Err(TerrainOpError::Invalid(format!(
            "采样坐标必须为有限值，收到 ({x}, {z})"
        )));
    }
    let patches = &doc.as_scene().terrain;
    let key = name.map(str::trim).filter(|s| !s.is_empty());

    if let Some(key) = key {
        let patch = find_patch(patches, key)
            .ok_or_else(|| TerrainOpError::PatchMissing(format!("场景里没有地形块 `{key}`")))?;
        let single = std::slice::from_ref(patch);
        return Ok(match sample_height_at(single, x, z) {
            Some(height) => {
                let normal = sample_normal(single, x, z, NORMAL_EPS).unwrap_or([0.0, 1.0, 0.0]);
                SampleOutcome {
                    name: patch.name.clone(),
                    height,
                    normal,
                    inside: true,
                    weights: layer_weights(height, normal),
                }
            }
            // 点在这块地形的矩形之外：不是「高度 0」，是「这里没有这块地形」。
            None => SampleOutcome {
                name: patch.name.clone(),
                height: 0.0,
                normal: [0.0, 1.0, 0.0],
                inside: false,
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        });
    }

    // 未指定块名：找一个给出最高层的块，同时把它的名字回出去（AI 下一步多半要用）。
    let mut best: Option<(&TerrainPatch, f32)> = None;
    for patch in patches {
        if let Some(h) = patch.sample_height(x, z) {
            if best.map_or(true, |(_, cur)| h > cur) {
                best = Some((patch, h));
            }
        }
    }
    Ok(match best {
        Some((patch, height)) => {
            let normal = sample_normal(patches, x, z, NORMAL_EPS).unwrap_or([0.0, 1.0, 0.0]);
            SampleOutcome {
                name: patch.name.clone(),
                height,
                normal,
                inside: true,
                weights: layer_weights(height, normal),
            }
        }
        None => SampleOutcome {
            name: String::new(),
            height: 0.0,
            normal: [0.0, 1.0, 0.0],
            inside: false,
            weights: [1.0, 0.0, 0.0, 0.0],
        },
    })
}

/// `terrain.blocks`：按块边长切块，回逐块清单与覆盖统计（只读）。
pub fn blocks(doc: &Document, block_size: Option<f32>) -> Result<BlocksOutcome, TerrainOpError> {
    let size = block_size_or_default(block_size)?;
    let patches = &doc.as_scene().terrain;
    if union_rect(patches).is_none() {
        // 空地形不是错误：回一张空表，让调用方自己决定要不要建地形。
        return Ok(BlocksOutcome {
            rect: [0.0; 4],
            block_size: size,
            dims: [0, 0],
            count: 0,
            covered: 0,
            blocks: Vec::new(),
        });
    };
    let grid = grid_of(patches, size)?;
    let mut out = Vec::with_capacity(grid.block_count() as usize);
    let mut covered = 0_u32;
    for block in grid.blocks() {
        let names = cover_names(patches, block.rect);
        if !names.is_empty() {
            covered += 1;
        }
        out.push(BlockOut {
            index: block.index,
            rect: block.rect,
            patches: names,
        });
    }
    Ok(BlocksOutcome {
        rect: grid.rect(),
        block_size: grid.block_size(),
        dims: grid.dims(),
        count: grid.block_count(),
        covered,
        blocks: out,
    })
}

/// `terrain.stats`：地形汇总（只读），三角/顶点与 `terrain.mesh` 同口径。
pub fn stats(doc: &Document) -> Result<StatsOutcome, TerrainOpError> {
    let patches = &doc.as_scene().terrain;
    let samples = patches.iter().map(|p| p.expected_samples()).sum();
    let size = DEFAULT_BLOCK_SIZE;
    if union_rect(patches).is_none() {
        return Ok(StatsOutcome {
            patches: 0,
            samples: 0,
            blocks: 0,
            covered: 0,
            triangles: 0,
            vertices: 0,
            lod: DEFAULT_LOD,
            block_size: size,
        });
    };
    let grid = grid_of(patches, size)?;
    let covered = grid
        .blocks()
        .filter(|b| !cover_names(patches, b.rect).is_empty())
        .count() as u32;
    let totals = mesh_totals(patches, &grid, DEFAULT_LOD, None)?;
    Ok(StatsOutcome {
        patches: patches.len(),
        samples,
        blocks: grid.block_count(),
        covered,
        triangles: totals.triangles,
        vertices: totals.vertices,
        lod: DEFAULT_LOD,
        block_size: size,
    })
}

/// `terrain.mesh`：派生网格统计（只读）。只回统计不回顶点数组——一块 128×128
/// 的网格有十几万浮点，搬过 RPC 对「确认这块地形有没有生成出来」毫无助益。
pub fn mesh(
    doc: &Document,
    block: Option<[u32; 2]>,
    lod: Option<f32>,
    block_size: Option<f32>,
) -> Result<MeshOutcome, TerrainOpError> {
    let ladder = match lod {
        Some(step) => lod_of(step)?,
        None => DEFAULT_LOD,
    };
    let size = block_size_or_default(block_size)?;
    let patches = &doc.as_scene().terrain;
    if union_rect(patches).is_none() {
        // 指定了块却没有任何地形：那是「块不存在」，不能回 0 顶点假装算过了。
        if let Some(index) = block {
            return Err(TerrainOpError::BlockMissing(format!(
                "场景里没有任何地形块，块索引 {index:?} 无法计算"
            )));
        }
        return Ok(MeshOutcome {
            lod: ladder,
            block: None,
            blocks: 0,
            vertices: 0,
            triangles: 0,
            bounds: None,
        });
    }
    let grid = grid_of(patches, size)?;
    mesh_totals(patches, &grid, ladder, block)
}
