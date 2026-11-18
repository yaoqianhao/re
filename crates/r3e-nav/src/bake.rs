//! 烘焙：把场景几何压成一张「每个 XZ 格站不站得住」的位图。
//!
//! 三个来源，按优先级：
//!
//! 1. **地形**（[`TerrainPatch`]）——提供每格的地面高度，双线性采样；
//! 2. **节点几何**（内置网格 + 显式 `physics.half_extents`）——提供阻挡体；
//! 3. **无几何的资产网格**（glTF 等外部网格）——形状只能按 `scale * 0.5` 或
//!    显式 `half_extents` 估，**估不准的绝不假装准**：这类资产名会进
//!    [`NavReport::unresolved`]，让调用方知道「这份可达性结论里有多少是靠猜的」。
//!
//! 阻挡判定用「格柱 × 障碍 AABB」：格柱是 `[格中心 ± cell/2] × [地面, 地面+agent_height]`，
//! 障碍 AABB 先按角色半径 inflate——这样一次判定同时吃掉了「身体宽度」与
//! 「贴墙走要留缝」，不需要再做多遍形态学腐蚀。
//!
//! 高度判定有两条豁免：障碍顶低于地面（埋在地下的地基）不挡，障碍底高于
//! 「地面 + 身高」（头顶横梁、门楣）不挡。

use glam::Vec3;
use r3e_core::{Aabb, BuiltinMesh, MeshRef, Node, Scene};

use crate::grid::{NavGrid, NAV_MAX_CELLS, NAV_MAX_SIDE};
use crate::{NavConfig, NavError};

/// 判定「贴地」的容差：障碍顶比地面高出不到 2 cm 就当它不挡路。
const FLOOR_EPS: f32 = 0.02;

/// 烘焙入口。
pub(crate) fn bake(scene: &Scene, cfg: NavConfig) -> Result<NavGrid, NavError> {
    let cfg = cfg.sanitized();
    let mut unresolved: Vec<String> = Vec::new();
    let obstacles = collect_obstacles(scene, &cfg, &mut unresolved);

    let bounds = match cfg.bounds {
        Some(b) => b,
        None => auto_bounds(scene, &obstacles, &cfg),
    };
    let (min, dims) = plan(bounds, cfg.cell_size)?;
    let cells = dims[0] as usize * dims[1] as usize;
    if cells as u64 > NAV_MAX_CELLS {
        return Err(NavError::TooLarge {
            cells: cells as u64,
            limit: NAV_MAX_CELLS,
        });
    }

    // 高度场：逐格采样地形；无地形覆盖的格按 y = 0 的默认地面处理
    // （M1 里绝大多数场景就是一块平地，让它们免填 terrain 段）。
    //
    // 初始可走性由「这一格有没有地面」决定：场景填了 `terrain` 时，地形范围之外
    // 一律不可走——否则烘焙范围外扩出来的一圈空白会把被墙隔开的两半从外面又连起来，
    // 让「墙到底有没有隔断」这种最基本的问题都判错。没填 `terrain` 的场景视为
    // 一整块默认平地（否则一个只有物件的场景会变成「处处不可走」）。
    let has_terrain = !scene.terrain.is_empty();
    let mut heights = vec![0.0f32; cells];
    let mut grounded = vec![true; cells];
    for j in 0..dims[1] {
        for i in 0..dims[0] {
            let (x, z) = cell_center_xz(min, cfg.cell_size, i, j);
            let idx = j as usize * dims[0] as usize + i as usize;
            let sampled = sample_ground_opt(scene, x, z);
            heights[idx] = sampled.unwrap_or(0.0);
            grounded[idx] = !has_terrain || sampled.is_some();
        }
    }

    let mut grid = NavGrid::new(cfg.clone(), dims, min, heights);
    for j in 0..dims[1] {
        for i in 0..dims[0] {
            grid.set_walkable(i, j, grounded[j as usize * dims[0] as usize + i as usize]);
        }
    }
    for ob in &obstacles {
        stamp_obstacle(&mut grid, ob);
    }
    grid.flood_regions();
    for name in unresolved {
        grid.note_unresolved(name);
    }
    log::debug!(
        "导航烘焙完成：{}×{} 格，可走 {} 格，{} 个连通块",
        dims[0],
        dims[1],
        grid.walkable_count(),
        grid.region_count()
    );
    Ok(grid)
}

/// 由世界范围算出 `(min, dims)`，范围非法或超过单轴上限直接报错。
fn plan(bounds: [f32; 4], cell: f32) -> Result<([f32; 2], [u32; 2]), NavError> {
    let [x0, z0, x1, z1] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || x1 <= x0 || z1 <= z0 {
        return Err(NavError::InvalidBounds {
            reason: format!("范围非法：{bounds:?}"),
        });
    }
    let nx = ((x1 - x0) / cell).ceil().max(1.0);
    let nz = ((z1 - z0) / cell).ceil().max(1.0);
    if nx > NAV_MAX_SIDE as f32 || nz > NAV_MAX_SIDE as f32 {
        return Err(NavError::TooLarge {
            cells: (nx as u64) * (nz as u64),
            limit: NAV_MAX_CELLS,
        });
    }
    Ok(([x0, z0], [nx as u32, nz as u32]))
}

fn cell_center_xz(min: [f32; 2], cell: f32, i: u32, j: u32) -> (f32, f32) {
    (
        min[0] + (i as f32 + 0.5) * cell,
        min[1] + (j as f32 + 0.5) * cell,
    )
}

/// 自动范围：地形 + 障碍 + 出生点的并集，再外扩一格。
///
/// 场景里什么都没有时给一个以原点为中心的 8×8 格方场——烘焙不该因为
/// 「空场景」直接失败，调用方要的是「这里没地方可走」这个结论。
fn auto_bounds(scene: &Scene, obstacles: &[Aabb], cfg: &NavConfig) -> [f32; 4] {
    let mut acc = Aabb::EMPTY;
    let mut any = false;
    for patch in &scene.terrain {
        let x0 = patch.origin[0];
        let z0 = patch.origin[1];
        acc = acc.union(&Aabb::new(
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x0 + patch.size[0], 0.0, z0 + patch.size[1]),
        ));
        any = true;
    }
    for ob in obstacles {
        acc = acc.union(ob);
        any = true;
    }
    for sp in &scene.spawns {
        let p = Vec3::new(sp.position[0], sp.position[1], sp.position[2]);
        acc = acc.union(&Aabb::new(p, p));
        any = true;
    }
    if !any {
        let half = cfg.cell_size * 4.0;
        return [-half, -half, half, half];
    }
    let pad = cfg.cell_size * 2.0;
    acc = acc.inflate(pad);
    [acc.min.x, acc.min.z, acc.max.x, acc.max.z]
}

/// 收集阻挡体（世界 AABB，已按角色半径 inflate）。
///
/// 跳过：不可见节点、`plane` 网格（地面本身，不是墙）、带 `ignore_tags` 的节点
/// （角色/载具这类会动的物件不该被烘进静态导航数据，否则它站过的地方就永久不可走）。
fn collect_obstacles(scene: &Scene, cfg: &NavConfig, unresolved: &mut Vec<String>) -> Vec<Aabb> {
    let mut out = Vec::new();
    for node in scene.iter() {
        if !node.visible {
            continue;
        }
        // 名字以 `#` 开头 = 约定的「非实体」（组、锚点），不参与导航。
        if node.name.starts_with('#') {
            continue;
        }
        if node.tags.iter().any(|t| cfg.is_ignored_tag(t)) {
            continue;
        }
        if let Some(aabb) = node_obstacle(node, unresolved) {
            out.push(inflate_xz(aabb, cfg.agent_radius));
        }
    }
    out
}

/// 单个节点的阻挡盒；`None` 表示它不挡路。
fn node_obstacle(node: &Node, unresolved: &mut Vec<String>) -> Option<Aabb> {
    // 显式声明了半尺寸 → 以场景为准（人工调过的值优先，与物理装配口径一致）。
    if let Some(p) = node.physics.as_ref() {
        if let Some(half) = p.half_extents {
            let half = Vec3::from(half).abs();
            if half.max_element() <= 0.0 {
                return None;
            }
            return Some(Aabb::from_center_half(node.world_position(), half));
        }
    }

    let local_half = match node.mesh {
        MeshRef::Builtin(BuiltinMesh::Plane) => return None,
        MeshRef::Builtin(_) => Vec3::splat(0.5),
        MeshRef::Asset { asset, mesh } => {
            // 外部网格没有几何可查：按 scale*0.5 估（单位盒），并登记为未解析。
            let name = format!("{asset}#{mesh}");
            if !unresolved.iter().any(|n| n == &name) {
                unresolved.push(name);
            }
            Vec3::splat(0.5)
        }
    };
    let aabb = Aabb::from_center_half(Vec3::ZERO, local_half).transformed(&node.world);
    (!aabb.is_empty()).then_some(aabb)
}

/// 只在水平面上外扩（角色半径是「身体有多宽」，不是「人有多高」）。
///
/// 这里必须按轴分开写：均匀 `inflate` 会把障碍往上也长 0.4 m，
/// 于是「低于 step_height 的台阶可以跨过去」这条判定会被自己抬高的包围盒推翻，
/// 5 cm 的路沿瞬间变成 45 cm 的墙。
fn inflate_xz(mut aabb: Aabb, r: f32) -> Aabb {
    if r <= 0.0 {
        return aabb;
    }
    aabb.min.x -= r;
    aabb.min.z -= r;
    aabb.max.x += r;
    aabb.max.z += r;
    aabb
}

/// 把障碍投影到格子上：与格柱相交的格一律置为不可走。
fn stamp_obstacle(grid: &mut NavGrid, ob: &Aabb) {
    let cell = grid.config().cell_size;
    let agent_height = grid.config().agent_height;
    let [min_x, min_z, _max_x, _max_z] = grid.bounds();
    let dims = grid.dims();

    let lo_i = ((ob.min.x - min_x) / cell).floor() as i64;
    let hi_i = ((ob.max.x - min_x) / cell).ceil() as i64 - 1;
    let lo_j = ((ob.min.z - min_z) / cell).floor() as i64;
    let hi_j = ((ob.max.z - min_z) / cell).ceil() as i64 - 1;
    let lo_i = lo_i.max(0);
    let lo_j = lo_j.max(0);
    let hi_i = hi_i.min(dims[0] as i64 - 1);
    let hi_j = hi_j.min(dims[1] as i64 - 1);

    for j in lo_j..=hi_j {
        for i in lo_i..=hi_i {
            let (i, j) = (i as u32, j as u32);
            let (cx, cz) = cell_center_xz([min_x, min_z], cell, i, j);
            let half = cell * 0.5;
            // 格柱 XZ 与障碍盒必须真正重叠（严格贴边算重叠，保守优先）。
            if ob.max.x <= cx - half
                || ob.min.x >= cx + half
                || ob.max.z <= cz - half
                || ob.min.z >= cz + half
            {
                continue;
            }
            let ground = grid.ground(i, j);
            let rise = ob.max.y - ground;
            if rise <= FLOOR_EPS {
                continue; // 埋在地下：不挡
            }
            if rise <= grid.config().step_height {
                continue; // 路边石这种高度：抬脚就过去了，不该封路
            }
            if ob.min.y >= ground + agent_height {
                continue; // 头顶横梁：不挡
            }
            grid.set_walkable(i, j, false);
        }
    }
}

/// 地面高度采样：所有覆盖该点的地形块取「最高的那一层」。
///
/// 多块重叠时取最高而不是平均，是为了让「上层平台」压住「下层地面」——
/// 角色站在哪一层由它自己的 Y 决定，烘焙时只认最高的可站面。
fn sample_ground_opt(scene: &Scene, x: f32, z: f32) -> Option<f32> {
    let mut h: Option<f32> = None;
    for patch in &scene.terrain {
        if let Some(y) = patch_height(patch, x, z) {
            h = Some(h.map_or(y, |cur: f32| cur.max(y)));
        }
    }
    h
}

/// 单个地形块上的双线性高度采样；点不在块内返回 `None`。
fn patch_height(patch: &r3e_core::TerrainPatch, x: f32, z: f32) -> Option<f32> {
    let (w, d) = (patch.size[0], patch.size[1]);
    if w <= 0.0 || d <= 0.0 {
        return None;
    }
    let u = (x - patch.origin[0]) / w;
    let v = (z - patch.origin[1]) / d;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let [rx, ry] = patch.resolution;
    if patch.heights.is_empty() || rx < 2 || ry < 2 {
        // 平坦地形：`heights` 空数组就是「全 0」的合法表示。
        return Some(0.0);
    }
    if patch.heights.len() != (rx * ry) as usize {
        // 样本数与分辨率不符 → 数据已坏，按平坦处理而不是拿越界值硬算。
        return Some(0.0);
    }
    let fx = u * (rx - 1) as f32;
    let fz = v * (ry - 1) as f32;
    let x0 = fx.floor() as u32;
    let z0 = fz.floor() as u32;
    let x1 = (x0 + 1).min(rx - 1);
    let z1 = (z0 + 1).min(ry - 1);
    let tx = fx - x0 as f32;
    let tz = fz - z0 as f32;
    let h00 = patch.height_at(x0, z0);
    let h10 = patch.height_at(x1, z0);
    let h01 = patch.height_at(x0, z1);
    let h11 = patch.height_at(x1, z1);
    let a = h00 + (h10 - h00) * tx;
    let b = h01 + (h11 - h01) * tx;
    Some(a + (b - a) * tz)
}
