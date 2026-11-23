//! 地形（M2-C06）：`terrain_*` 七条命令的脚本侧绑定。
//!
//! 与 [`super::physics`] / [`super::character`] 同一分工：本文件只做「脚本参数 →
//! 场景写入 / 能力面诉求」的搬运与校验。**派生几何一行都不在这里算**——
//! 块划分、LOD 抽稀、程序化噪声、四层权重全在 `r3e-terrain`，经
//! [`TerrainHost`](r3e_core::host::TerrainHost) 能力面注入（方案 B，零新依赖边：
//! `r3e-script` 的 allow 里**没有** `r3e-terrain`，也不会有）。
//!
//! ## 三条脚本侧口径（与 RPC 侧刻意不同，改之前先想清楚）
//!
//! 1. **写场景的只有四条**：`create` / `remove` / `resample` / `gen` 直接调
//!    `Scene::upsert_terrain_patch` / `remove_terrain_patch` 落笔——脚本侧没有撤销栈
//!    （`cmd_spec` 里这几条的 `reversible` 都是 `false`），`state_hash` 照常覆盖，
//!    因为它算的是 `Scene` 本身；`sample` / `blocks` / `stats` 一个字节都不写。
//! 2. **`terrain.mesh` 没有脚本函数**：网格生成要 `r3e-terrain` 的 `block_mesh`，
//!    而 `core` 的 DTO 只承载统计量。它是**只 RPC** 的一条（`cmd_spec/terrain.rs`
//!    把它挂在 `Domain::Meta`），脚本里请用 `terrain_stats`。
//! 3. **非法参数一律 `false` / 空表，不 panic**：脚本每帧都在调，一个笔误不该让
//!    整段脚本挂掉。拒因一律 `println!` 一行，与 `character_*` 同风格。
//!
//! ## 为什么 `sample` / `blocks` / `stats` / `gen` 回 JSON 字符串
//!
//! 与 `r3e-protocol` 的回包同构：脚本里 `var r = terrain_stats();` 之后用
//! `r.height` 这种形状，比在 Rhai 里造一套 Map 更省事，也让「脚本看到的数」
//! 与「RPC 看到的数」字段名一一对得上，排查时不用做翻译。

use super::*;
use r3e_core::host::{TerrainBlocksDto, TerrainSampleDto, TerrainStatsDto};
use r3e_core::{TerrainPatch, TERRAIN_MAX_PATCHES, TERRAIN_MAX_RESOLUTION, TERRAIN_MIN_SIZE};
use serde_json::json;

/// 场景当前的全部地形块（克隆一份：本文件的所有调用都要在**放开 `doc` 借用之后**
/// 才能落笔或调能力面，否则重入 `RefCell` 会 panic）。
fn patches_of(ctx: &ScriptContext) -> Vec<TerrainPatch> {
    ctx.doc.borrow().as_scene().terrain.clone()
}

/// 写一块地形进场景；返回「是否已落笔」。
///
/// 用 `Scene::upsert_terrain_patch`（与 `terrain.create` 同一张表、同一套上限语义），
/// 不走 `CommandKind`：脚本侧的写入**不进编辑器撤销栈**（`cmd_spec` 里这四条
/// `reversible` 全是 `false`，就是这个意思）。`state_hash` 仍然照旧覆盖到——
/// 哈希是对 `Scene` 算的，与走不走命令无关。
fn upsert(ctx: &ScriptContext, patch: TerrainPatch) -> bool {
    let name = patch.name.clone();
    let ok = ctx.write(|scene| scene.upsert_terrain_patch(patch));
    if !ok {
        println!("[script] terrain: 场景拒绝了 `{name}` 的写入（名字收敛后为空或表满）");
    }
    ok
}

/// 采样结果 → JSON（字段名与 `terrain.sample` 的回包一致）。
fn sample_json(s: &TerrainSampleDto) -> String {
    json!({
        "name": s.name,
        "height": s.height,
        "normal": s.normal,
        "inside": s.inside,
        "weights": s.weights,
    })
    .to_string()
}

/// 块划分结果 → JSON。
fn blocks_json(b: &TerrainBlocksDto) -> String {
    let blocks: Vec<_> = b
        .blocks
        .iter()
        .map(|blk| {
            json!({
                "index": blk.index,
                "rect": blk.rect,
                "patches": blk.patches,
            })
        })
        .collect();
    json!({
        "rect": b.rect,
        "block_size": b.block_size,
        "dims": b.dims,
        "count": b.count,
        "covered": b.covered,
        "blocks": blocks,
    })
    .to_string()
}

/// 汇总统计 → JSON。
fn stats_json(s: &TerrainStatsDto) -> String {
    json!({
        "patches": s.patches,
        "samples": s.samples,
        "blocks": s.blocks,
        "covered": s.covered,
        "triangles": s.triangles,
        "vertices": s.vertices,
        "lod_step": s.lod_step,
        "block_size": s.block_size,
    })
    .to_string()
}

/// 注册「地形」域的脚本函数（七条；`terrain.mesh` 只 RPC，见文件头第 2 条）。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // 建块：同名整体覆盖（上限只拦新增），与 RPC 侧同一语义。
    let c = ctx.clone();
    engine.register_fn(
        "terrain_create",
        move |name: &str, min_x: f64, min_z: f64, size_x: f64, size_z: f64, resolution: i64| -> bool {
            let key = name.trim();
            if key.is_empty() {
                println!("[script] terrain_create: 地形块名字不能为空");
                return false;
            }
            if !(min_x.is_finite() && min_z.is_finite() && size_x.is_finite() && size_z.is_finite()) {
                println!("[script] terrain_create `{key}`: 参数含非有限值");
                return false;
            }
            if size_x < TERRAIN_MIN_SIZE as f64 || size_z < TERRAIN_MIN_SIZE as f64 {
                println!(
                    "[script] terrain_create `{key}`: 尺寸必须 ≥ {TERRAIN_MIN_SIZE} 米，收到 [{size_x}, {size_z}]"
                );
                return false;
            }
            // 分辨率**收敛会变形**，所以直接拒（RPC 侧 `terrain.create` 同一口径）：
            // 夹成 128 会让脚本以为「我写的就是这个」。
            if resolution < 1 || resolution > TERRAIN_MAX_RESOLUTION as i64 {
                println!(
                    "[script] terrain_create `{key}`: 分辨率必须在 1..={TERRAIN_MAX_RESOLUTION} 之间，收到 {resolution}"
                );
                return false;
            }

            let existed = {
                let doc = c.doc.borrow();
                r3e_core::terrain::find_patch(&doc.as_scene().terrain, key).is_some()
            };
            if !existed {
                let len = c.doc.borrow().as_scene().terrain.len();
                if len >= TERRAIN_MAX_PATCHES {
                    println!(
                        "[script] terrain_create: 地形块表已满（上限 {TERRAIN_MAX_PATCHES}），`{key}` 未写入"
                    );
                    return false;
                }
            }

            let mut patch = TerrainPatch::flat(
                key,
                [min_x as f32, min_z as f32],
                [size_x as f32, size_z as f32],
            );
            patch.resolution = [resolution as u32, resolution as u32];
            upsert(&c, patch)
        },
    );

    // 删块：名字不存在回 false（删除天然幂等，RPC 侧也是 `removed: false`）。
    let c = ctx.clone();
    engine.register_fn("terrain_remove", move |name: &str| -> bool {
        let key = name.trim();
        if key.is_empty() {
            println!("[script] terrain_remove: 地形块名字不能为空");
            return false;
        }
        let key = key.to_string();
        let existed = {
            let doc = c.doc.borrow();
            r3e_core::terrain::find_patch(&doc.as_scene().terrain, &key).is_some()
        };
        if !existed {
            println!("[script] terrain_remove: 场景里没有地形块 `{key}`");
            return false;
        }
        c.write(|scene| scene.remove_terrain_patch(&key))
    });

    // 采样：空名字表示跨全部块取最高层（与 RPC 侧 `name` 缺省同义）。
    let c = ctx.clone();
    engine.register_fn(
        "terrain_sample",
        move |name: &str, x: f64, z: f64| -> String {
            let key = {
                let k = name.trim();
                if k.is_empty() {
                    None
                } else {
                    Some(k.to_string())
                }
            };
            // 指定了块名但场景里没有这块：如实回「没命中」，不假装是平地。
            if let Some(k) = key.as_deref() {
                let patches = patches_of(&c);
                if r3e_core::terrain::find_patch(&patches, k).is_none() {
                    println!("[script] terrain_sample: 场景里没有地形块 `{k}`");
                    return sample_json(&TerrainSampleDto::default());
                }
            }
            let patches = patches_of(&c);
            let out = c.terrain.sample(&patches, key.as_deref(), x, z);
            sample_json(&out)
        },
    );

    // 块划分：空地形回空表（`count = 0`），不报错。
    let c = ctx.clone();
    engine.register_fn("terrain_blocks", move || -> String {
        let patches = patches_of(&c);
        let out = c.terrain.blocks(&patches, None);
        blocks_json(&out)
    });

    // 重采样：算出来的新网格由这里落笔（能力面只算不写）。
    // `maxResolution = 0` 表示「用全局上限」——Rhai 没有可选参数，用它当哨兵。
    let c = ctx.clone();
    engine.register_fn(
        "terrain_resample",
        move |name: &str, step: f64, max_resolution: i64| -> bool {
            let key = name.trim();
            if key.is_empty() {
                println!("[script] terrain_resample: 地形块名字不能为空");
                return false;
            }
            let cap = if max_resolution <= 0 {
                None
            } else if max_resolution < 2 || max_resolution > TERRAIN_MAX_RESOLUTION as i64 {
                println!(
                    "[script] terrain_resample `{key}`: 分辨率上限必须在 2..={TERRAIN_MAX_RESOLUTION} 之间，收到 {max_resolution}"
                );
                return false;
            } else {
                Some(max_resolution as u32)
            };
            let Some(current) = r3e_core::terrain::find_patch(&patches_of(&c), key).cloned() else {
                println!("[script] terrain_resample: 场景里没有地形块 `{key}`");
                return false;
            };
            let Some(out) = c.terrain.resample(&current, step, cap) else {
                println!("[script] terrain_resample `{key}`: 步长非法或地形后端未接入");
                return false;
            };
            if !out.changed {
                // 平坦块没有可抽稀的网格：不写场景（写一条内容相同的命令只白占撤销栈）。
                println!("[script] terrain_resample `{key}`: 平坦块无可重采样内容，未改动");
                return true;
            }
            upsert(&c, out.patch)
        },
    );

    // 生成：先算块，再对**真场景**做容量预检，最后逐块落笔——
    // 要么全写、要么全不写（与 RPC 侧 `terrain.gen` 同一不变式）。
    let c = ctx.clone();
    engine.register_fn(
        "terrain_gen",
        move |size_x: i64, size_z: i64, seed: i64| -> String {
            if size_x <= 0 || size_z <= 0 {
                println!("[script] terrain_gen: 尺寸必须为正，收到 {size_x} × {size_z}");
                return json!({ "created": 0, "names": [], "state_hash": "" }).to_string();
            }
            let Some(generated) = c
                .terrain
                .generate(size_x as f64, size_z as f64, seed as u64)
            else {
                println!("[script] terrain_gen: 尺寸非法或切块超上限（{size_x} × {size_z} 米）");
                return json!({ "created": 0, "names": [], "state_hash": "" }).to_string();
            };
            let existing = patches_of(&c);
            let fresh = generated
                .iter()
                .filter(|p| r3e_core::terrain::find_patch(&existing, &p.name).is_none())
                .count();
            if existing.len() + fresh > TERRAIN_MAX_PATCHES {
                println!(
                    "[script] terrain_gen: 生成 {} 块会超出地形表上限 {TERRAIN_MAX_PATCHES}（现有 {} 块，其中 {fresh} 块为新增）",
                    generated.len(),
                    existing.len()
                );
                return json!({ "created": 0, "names": [], "state_hash": "" }).to_string();
            }
            let names: Vec<String> = generated.iter().map(|p| p.name.clone()).collect();
            for patch in generated {
                if !upsert(&c, patch) {
                    // 预检已经过了还写失败：只能是命令层拒绝（名字收敛后为空之类）。
                    // 此时场景里已经有前几块，如实回「写了几块」而不是假装全成。
                    println!("[script] terrain_gen: 中途写入被拒，已写入的块保留在场景里");
                    break;
                }
            }
            let created = names
                .iter()
                .filter(|n| r3e_core::terrain::find_patch(&patches_of(&c), n).is_some())
                .count();
            let state_hash = c.doc.borrow().state_hash().to_hex();
            json!({
                "created": created,
                "names": names,
                "state_hash": state_hash,
            })
            .to_string()
        },
    );

    // 汇总：与 `terrain.mesh` 同源（同一份 `mesh_totals`），所以逐块求和必然等于这里。
    let c = ctx.clone();
    engine.register_fn("terrain_stats", move || -> String {
        let patches = patches_of(&c);
        let out = c.terrain.stats(&patches);
        stats_json(&out)
    });
}
