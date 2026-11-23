//! `terrain.*` 里**改场景**的四条：`create` / `remove` / `resample` / `gen`。
//!
//! 四条一律经 [`CommandKind`] 落笔，因此可撤销、进 `state_hash`、能进事务。
//! 校验一律在落笔前做完（`terrain_ops::mod` 的不变式 3：同名覆盖、上限只拦新增），
//! 免得「改了一半发现参数非法」留下半个场景。

use super::*;
use r3e_core::terrain::find_patch;
use r3e_core::{TERRAIN_MAX_PATCHES, TERRAIN_MAX_RESOLUTION, TERRAIN_MIN_SIZE};
use r3e_terrain::generate;

// ── 写场景的四条 ──────────────────────────────────────────────────────────

/// `terrain.create`：同名整体覆盖，超上限只拦新增。
///
/// 参数在这里就掐死（而不是交给 `sanitized` 兜）：`size = 0` 被 `sanitized` 换成
/// 缺省的 16 米、`resolution = 200` 被夹成 128，都会让调用方以为「我写的就是这个」
/// —— 那是两套真值。**能收敛的走收敛（名字截断），会变形的直接拒。**
pub fn create(
    doc: &mut Document,
    name: &str,
    rect: [f32; 4],
    resolution: Option<u32>,
) -> Result<CreateOutcome, TerrainOpError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TerrainOpError::Invalid("地形块名字不能为空".to_string()));
    }
    if !rect.iter().all(|v| v.is_finite()) {
        return Err(TerrainOpError::Invalid(format!(
            "地形矩形含非有限值：{rect:?}"
        )));
    }
    if rect[2] < TERRAIN_MIN_SIZE || rect[3] < TERRAIN_MIN_SIZE {
        return Err(TerrainOpError::Invalid(format!(
            "地形尺寸必须 ≥ {TERRAIN_MIN_SIZE} 米，收到 [{}, {}]",
            rect[2], rect[3]
        )));
    }
    let res = resolution.unwrap_or(1);
    if res == 0 || res > TERRAIN_MAX_RESOLUTION {
        return Err(TerrainOpError::Invalid(format!(
            "分辨率必须在 1..={TERRAIN_MAX_RESOLUTION} 之间，收到 {res}"
        )));
    }

    let existed = find_patch(&doc.as_scene().terrain, name).is_some();
    if !existed && doc.as_scene().terrain.len() >= TERRAIN_MAX_PATCHES {
        return Err(TerrainOpError::Rejected(format!(
            "地形块表已满（上限 {TERRAIN_MAX_PATCHES}），`{name}` 未写入"
        )));
    }

    let mut patch = TerrainPatch::flat(name, [rect[0], rect[1]], [rect[2], rect[3]]);
    // `resolution` 为 1 时 heights 保持空（= 平坦），与 M1 的缺省形态一致。
    patch.resolution = [res, res];

    let cmd = CommandKind::upsert_terrain_patch(doc.as_scene(), patch);
    doc.apply(&cmd).map_err(map_doc)?;

    let stored = find_patch(&doc.as_scene().terrain, name)
        .cloned()
        .ok_or_else(|| TerrainOpError::Doc("写入后在地形表里找不到刚写的块".to_string()))?;
    Ok(CreateOutcome {
        patch: stored,
        created: !existed,
        state_hash: doc.state_hash(),
    })
}

/// `terrain.remove`：按名字删；名字不存在回 [`TerrainOpError::PatchMissing`]。
///
/// 注意协议层的口径是「名字不存在**不报错**，回 `removed: false`」——
/// 那一层由宿主把 `PatchMissing` 翻成 `removed: false`（删除天然幂等）。
/// 这里保留错误是因为脚本侧 `terrain_remove` 需要区分「删掉了」和「本来就没有」。
pub fn remove(doc: &mut Document, name: &str) -> Result<RemoveOutcome, TerrainOpError> {
    let key = name.trim();
    if key.is_empty() {
        return Err(TerrainOpError::Invalid("地形块名字不能为空".to_string()));
    }
    let cmd = CommandKind::remove_terrain_patch(doc.as_scene(), key)
        .ok_or_else(|| TerrainOpError::PatchMissing(format!("场景里没有地形块 `{key}`")))?;
    doc.apply(&cmd).map_err(map_doc)?;
    Ok(RemoveOutcome {
        remaining: doc.as_scene().terrain.len(),
        state_hash: doc.state_hash(),
    })
}

/// `terrain.resample`：按步长重采样高度网格并写回（可撤销）。
///
/// 平坦块（`heights` 为空）没有可重采样的内容：**不写场景**，回 `maxError: 0`
/// —— 写一条内容相同的命令只会白占一格撤销栈。
pub fn resample(
    doc: &mut Document,
    name: &str,
    lod: f32,
    max_resolution: Option<u32>,
) -> Result<ResampleOutcome, TerrainOpError> {
    let key = name.trim();
    if key.is_empty() {
        return Err(TerrainOpError::Invalid("地形块名字不能为空".to_string()));
    }
    let ladder = lod_of(lod)?;
    let step = ladder.step();
    let current = find_patch(&doc.as_scene().terrain, key)
        .cloned()
        .ok_or_else(|| TerrainOpError::PatchMissing(format!("场景里没有地形块 `{key}`")))?;
    let from = current.resolution;
    if current.heights.is_empty() || from[0] < 2 || from[1] < 2 {
        // 平坦块：分辨率改了也不产生任何视觉/物理差异，如实回「没动」。
        return Ok(ResampleOutcome {
            from,
            to: from,
            samples: current.expected_samples(),
            max_error: 0.0,
            state_hash: doc.state_hash(),
        });
    }

    // 目标分辨率 = 旧网格按步长抽稀后的点数，再被 `maxResolution` 与全局上限夹；
    // 至少 2 行 2 列（1 行 1 列等于平坦，会把地形抹平，不是"降精度"）。
    let cap = max_resolution
        .unwrap_or(TERRAIN_MAX_RESOLUTION)
        .clamp(2, TERRAIN_MAX_RESOLUTION);
    let shrink = |n: u32| -> u32 {
        let kept = (n - 1) / step + 1;
        kept.clamp(2, cap.min(n.max(2)))
    };
    let to = [shrink(from[0]), shrink(from[1])];

    let mut heights = Vec::with_capacity((to[0] * to[1]) as usize);
    for j in 0..to[1] {
        let v = j as f32 / (to[1] - 1) as f32;
        let z = current.origin[1] + current.size[1] * v;
        for i in 0..to[0] {
            let u = i as f32 / (to[0] - 1) as f32;
            let x = current.origin[0] + current.size[0] * u;
            // 采样落空（矩形边界上的浮点误差）按 0：`heights` 里不能留 NaN。
            heights.push(current.sample_height(x, z).unwrap_or(0.0));
        }
    }

    let mut next = current.clone();
    next.resolution = to;
    next.heights = heights;
    let next = next.sanitized();

    // 误差在同一批世界点（**旧**网格的样本位置）上比：那是调用方原本看到的地形。
    let mut max_error = 0.0_f32;
    for j in 0..from[1] {
        let v = if from[1] > 1 {
            j as f32 / (from[1] - 1) as f32
        } else {
            0.0
        };
        let z = current.origin[1] + current.size[1] * v;
        for i in 0..from[0] {
            let u = if from[0] > 1 {
                i as f32 / (from[0] - 1) as f32
            } else {
                0.0
            };
            let x = current.origin[0] + current.size[0] * u;
            let before = current.sample_height(x, z).unwrap_or(0.0);
            let after = next.sample_height(x, z).unwrap_or(0.0);
            max_error = max_error.max((before - after).abs());
        }
    }

    let cmd = CommandKind::upsert_terrain_patch(doc.as_scene(), next);
    doc.apply(&cmd).map_err(map_doc)?;

    let stored = find_patch(&doc.as_scene().terrain, key)
        .cloned()
        .ok_or_else(|| TerrainOpError::Doc("写入后在地形表里找不到重采样结果".to_string()))?;
    Ok(ResampleOutcome {
        from,
        to: stored.resolution,
        samples: stored.expected_samples(),
        max_error,
        state_hash: doc.state_hash(),
    })
}

/// `terrain.gen`：程序化生成整张地形并写进场景（可撤销）。
///
/// 一生成就是几十块，**要么全写、要么全不写**：生成物先做容量预检，
/// 免得写到一半撞上表满，留下半张地图。
pub fn generate_into(
    doc: &mut Document,
    size_x: f32,
    size_z: f32,
    seed: u64,
) -> Result<GenOutcome, TerrainOpError> {
    let patches = generate(size_x, size_z, seed).ok_or_else(|| {
        TerrainOpError::Invalid(format!("地形尺寸非法或切块超上限：{size_x} × {size_z} 米"))
    })?;
    // 预检：生成的名字是 `terrain_<i>_<j>`，可能与已有块重名（那时是覆盖，不占新增额度）。
    let existing = doc.as_scene().terrain.clone();
    let mut fresh = 0_usize;
    for patch in &patches {
        if find_patch(&existing, &patch.name).is_none() {
            fresh += 1;
        }
    }
    if existing.len() + fresh > TERRAIN_MAX_PATCHES {
        return Err(TerrainOpError::Rejected(format!(
            "生成 {} 块会超出地形表上限 {TERRAIN_MAX_PATCHES}（现有 {} 块，其中 {} 块为新增）",
            patches.len(),
            existing.len(),
            fresh
        )));
    }

    let mut names = Vec::with_capacity(patches.len());
    for patch in patches {
        names.push(patch.name.clone());
        let cmd = CommandKind::upsert_terrain_patch(doc.as_scene(), patch);
        doc.apply(&cmd).map_err(map_doc)?;
    }
    Ok(GenOutcome {
        created: names.len(),
        names,
        state_hash: doc.state_hash(),
    })
}
