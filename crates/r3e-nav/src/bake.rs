//! 烘焙：把场景几何压成一张「每个 XZ 格站不站得住」的位图。
//!
//! ## 真值链（M2 · D01 起锁死）
//!
//! ```text
//! core::TerrainPatch::sample_height   ← 地形真值（唯一一份插值实现，在 r3e-core）
//!        │  field_of()：逐格中心采样一次，装进规则网格
//!        ▼
//! physics::HeightField                ← 后端可装载/可查询的地形通道（C04）
//!        │  bake_with_field()：**只读这份场**，按格索引直接取高度
//!        ▼
//! NavGrid.heights                     ← 导航网格的每格地面高度
//! ```
//!
//! 这条链的每一段都是「派生」，没有第二套真相：
//!
//! * 本模块**不再自带双线性插值**（D01 删掉了旧的私有 `patch_height`）——
//!   两份插值迟早分叉，表现是「物理说脚下 1.0 m，导航说 1.02 m」，
//!   角色贴地时被反复弹起，而查错要跨两个 crate 对齐浮点次序；
//! * 两个物理后端（builtin / rapier）只做 `set_heightfield` 装、`heightfield()` 取，
//!   装取之间不动一个比特，所以「换后端」在烘焙里**不可观测**——
//!   [`NavProduct::product_hash`](crate::NavProduct::product_hash) 逐位相等是**构造**保证的，
//!   不是靠两段代码算出同样结果赌来的。
//!
//! ## 三个来源，按优先级
//!
//! 1. **地形**（[`TerrainPatch`](r3e_core::TerrainPatch)）——提供每格的地面高度；
//! 2. **节点几何**（内置网格 + 显式 `physics.half_extents`）——提供阻挡体；
//! 3. **无几何的资产网格**（glTF 等外部网格）——形状只能按 `scale * 0.5` 或
//!    显式 `half_extents` 估，**估不准的绝不假装准**：这类资产名会进
//!    [`NavReport::unresolved`](crate::NavReport::unresolved)，
//!    让调用方知道「这份可达性结论里有多少是靠猜的」。
//!
//! 阻挡判定用「格柱 × 障碍 AABB」：格柱是 `[格中心 ± cell/2] × [地面, 地面+agent_height]`，
//! 障碍 AABB 先按角色半径 inflate——这样一次判定同时吃掉了「身体宽度」与
//! 「贴墙走要留缝」，不需要再做多遍形态学腐蚀。
//!
//! 高度判定有两条豁免：障碍顶低于地面（埋在地下的地基）不挡，障碍底高于
//! 「地面 + 身高」（头顶横梁、门楣）不挡。

use glam::Vec3;
use r3e_core::{Aabb, BuiltinMesh, MeshRef, Node, Scene};
use r3e_physics::{HeightField, Physics};

use crate::grid::{NavGrid, NAV_MAX_CELLS, NAV_MAX_SIDE};
use crate::product::{cfg_digest, input_digest};
use crate::{NavConfig, NavError};

/// 判定「贴地」的容差：障碍顶比地面高出不到 2 cm 就当它不挡路。
const FLOOR_EPS: f32 = 0.02;

/// 一次烘焙的内部产物：网格 + 两级输入摘要。
///
/// 摘要与网格同生同灭：分开算就有一半机会忘算，
/// 而没有摘要的产物无法回答「两个哈希为什么不一样」。
pub(crate) struct Baked {
    pub grid: NavGrid,
    pub cfg_digest: u64,
    pub input_digest: u64,
}

/// 场景 → 高度场（真值链的第一段，**唯一**的地形采样点）。
///
/// 做法：把 [`HeightField`] 的采样点**对齐到导航格中心**，
/// 于是在烘焙时「第 `(i, j)` 格的高度 = `field.heights()[j * cols + i]`」是一条恒等式，
/// 不需要第二次插值——这不是省事，而是「只留一条真值链」的具体落法。
///
/// 场比网格多一行一列（[`HeightField::new`] 要求两个方向至少 2 个采样点），
/// 多出来的那一圈落在烘焙范围外，不影响任何格。
pub fn field_of(scene: &Scene, cfg: &NavConfig) -> Result<HeightField, NavError> {
    let cfg = cfg.clone().sanitized();
    let mut unresolved: Vec<String> = Vec::new();
    let obstacles = collect_obstacles(scene, &cfg, &mut unresolved);
    let bounds = match cfg.bounds {
        Some(b) => b,
        None => auto_bounds(scene, &obstacles, &cfg),
    };
    let (min, dims) = plan(bounds, cfg.cell_size)?;
    let cells = dims[0] as u64 * dims[1] as u64;
    if cells > NAV_MAX_CELLS {
        return Err(NavError::TooLarge {
            cells,
            limit: NAV_MAX_CELLS,
        });
    }

    // 场至少 2×2；不足的方向补到 2，多出的采样点落在范围外（不会被任何格读到）。
    let cols = dims[0].max(2) as usize;
    let rows = dims[1].max(2) as usize;
    let cell = cfg.cell_size;
    let x0 = min[0] + cell * 0.5;
    let z0 = min[1] + cell * 0.5;
    let mut heights = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        for i in 0..cols {
            let x = x0 + i as f32 * cell;
            let z = z0 + j as f32 * cell;
            // 无地形覆盖处按 y = 0 的默认地面填（M1 里绝大多数场景就是一块平地，
            // 让它们免填 terrain 段）；「这一格到底有没有地面」另由
            // `covered_by_terrain` 判，两者不能混：高度 0 不等于有地面。
            heights.push(sample_terrain_opt(scene, x, z).unwrap_or(0.0));
        }
    }
    HeightField::new(x0, z0, cell, cols, rows, heights).ok_or_else(|| NavError::FieldInvalid {
        reason: format!("{cols}×{rows} 格、格长 {cell} m 的高度场构造失败（含非有限值或尺寸非法）"),
    })
}

/// 唯一烘焙入口：吃一份高度场 + 场景（只为障碍与出生点），产出网格。
///
/// 两个后端走的是同一条路径（[`bake_via_backend`]），差别只在外层
/// 「把场装进哪个后端」。因此「换后端产物是否一致」这个问题在这里**没有分支可走**。
///
/// 网格的范围与格长一律取自高度场：场是怎么装的，网格就怎么烘。
/// 让 `cfg.bounds` 在这里还能覆盖场，等于允许「装进去的场」和「烘出来的网格」
/// 描述两个不同的世界——那正是 D01 要根除的漂移源。
pub(crate) fn bake_with_field(
    scene: &Scene,
    field: &HeightField,
    cfg: NavConfig,
) -> Result<Baked, NavError> {
    let mut cfg = cfg.sanitized();
    let cell = field.cell();
    cfg.cell_size = cell;
    let cols = field.cols();
    let rows = field.rows();
    let dims = [cols as u32, rows as u32];
    // 场的 0 号采样点就在第 (0, 0) 格的中心。
    let min = [field.origin().x - cell * 0.5, field.origin().z - cell * 0.5];
    let cells = cols * rows;
    if cells as u64 > NAV_MAX_CELLS {
        return Err(NavError::TooLarge {
            cells: cells as u64,
            limit: NAV_MAX_CELLS,
        });
    }

    let mut unresolved: Vec<String> = Vec::new();
    let obstacles = collect_obstacles(scene, &cfg, &mut unresolved);

    // 高度直接取场里的数（不重算），可走性由「这一格有没有地形覆盖」定：
    // 场景填了 `terrain` 时，地形范围之外一律不可走——否则烘焙范围外扩出来的一圈
    // 空白会把被墙隔开的两半从外面又连起来，让「墙到底有没有隔断」都判错。
    let has_terrain = !scene.terrain.is_empty();
    let mut heights = Vec::with_capacity(cells);
    let mut grounded = vec![true; cells];
    for j in 0..rows {
        for i in 0..cols {
            let idx = j * cols + i;
            heights.push(field.heights()[idx]);
            grounded[idx] =
                !has_terrain || covered_by_terrain(scene, field.world_x(i), field.world_z(j));
        }
    }

    let mut grid = NavGrid::new(cfg.clone(), dims, min, heights);
    for j in 0..dims[1] {
        for i in 0..dims[0] {
            grid.set_walkable(i, j, grounded[j as usize * cols + i as usize]);
        }
    }
    for ob in &obstacles {
        stamp_obstacle(&mut grid, ob);
    }
    grid.flood_regions();
    // 分类必须排在「障碍投影 + 连通块」之后：它读的是最终可走位图。
    grid.classify();
    for name in unresolved.clone() {
        grid.note_unresolved(name);
    }
    log::debug!(
        "导航烘焙完成：{}×{} 格，可走 {} 格，{} 个连通块",
        dims[0],
        dims[1],
        grid.walkable_count(),
        grid.region_count()
    );
    Ok(Baked {
        grid,
        cfg_digest: cfg_digest(&cfg),
        input_digest: input_digest(scene, &obstacles, &unresolved),
    })
}

/// 便捷路径：场景 → 场 → 网格（不经过任何后端）。
pub(crate) fn bake(scene: &Scene, cfg: NavConfig) -> Result<Baked, NavError> {
    let cfg = cfg.sanitized();
    let field = field_of(scene, &cfg)?;
    bake_with_field(scene, &field, cfg)
}

/// 后端路径：场景 → 场 → **装进后端再取回** → 网格。
///
/// 取回来的场必须存在：`set_heightfield` 只是「装」，装完立刻取不到
/// 说明后端没实现这条通道（或把数据丢在了别处）。这时**报错而不是退回本地场**——
/// 静默退回落本地场，就等于让「后端不一致」这件事永远不被发现。
pub(crate) fn bake_via_backend(
    scene: &Scene,
    cfg: NavConfig,
    physics: &mut Physics,
) -> Result<Baked, NavError> {
    let cfg = cfg.sanitized();
    let field = field_of(scene, &cfg)?;
    physics.set_heightfield(Some(field));
    let loaded = physics
        .heightfield()
        .cloned()
        .ok_or_else(|| NavError::BackendMismatch {
            backend: physics.backend_name().to_string(),
            reason: "后端没有保留 set_heightfield 装上的高度场".to_string(),
        })?;
    bake_with_field(scene, &loaded, cfg)
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

/// 这一格有没有地形覆盖（与「高度是几」是两件事：默认平地高度 0 不代表有地形）。
///
/// 判定与 `TerrainPatch::sample_height` 的覆盖条件逐条对齐（尺寸为正 + `u/v ∈ [0,1]`），
/// 只是不需要算高度，所以不走插值。
pub(crate) fn covered_by_terrain(scene: &Scene, x: f32, z: f32) -> bool {
    scene.terrain.iter().any(|p| {
        let (w, d) = (p.size[0], p.size[1]);
        if !(w > 0.0 && d > 0.0) || !x.is_finite() || !z.is_finite() {
            return false;
        }
        let u = (x - p.origin[0]) / w;
        let v = (z - p.origin[1]) / d;
        (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
    })
}

/// 地面高度采样：所有覆盖该点的地形块取「最高的那一层」。
///
/// 多块重叠时取最高而不是平均，是为了让「上层平台」压住「下层地面」——
/// 角色站在哪一层由它自己的 Y 决定，烘焙时只认最高的可站面。
///
/// 插值本身**不在这里**：调用 `TerrainPatch::sample_height`（`r3e-core` 的唯一实现）。
fn sample_terrain_opt(scene: &Scene, x: f32, z: f32) -> Option<f32> {
    let mut h: Option<f32> = None;
    for patch in &scene.terrain {
        if let Some(y) = patch.sample_height(x, z) {
            h = Some(h.map_or(y, |cur: f32| cur.max(y)));
        }
    }
    h
}
