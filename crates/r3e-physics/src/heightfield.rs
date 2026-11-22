//! 地形高度场（M2 · C04 / D13）：物理侧的**独立地形碰撞通道**。
//!
//! ## 为什么是独立通道而不是新的 `ColliderShape`
//!
//! `ColliderShape` 是脚本能写的刚体形状枚举（`sphere` / `box` / `capsule` / `cylinder`），
//! 加一个 `HeightField` 变体意味着：`BodyDesc` 要带上万级浮点数组、`r3e-protocol` 的 DTO
//! 要跟着改、`capabilities.json` 三源门跟着变、每个后端每条查询都要多一个分支。
//! 而地形在本引擎里**不是刚体**：它不动、不参与积分、不需要质量与摩擦求解，
//! 上层真正需要的只有三件事 —— 这里多高、法线朝哪、这个点在不在场内。
//!
//! 所以 C04 走 D13 的决定：`HeightField` 是后端的一个**附加查询层**，
//! 由 `PhysicsBackend::set_heightfield` 装上去，查询时与刚体结果合并择优。
//! `ColliderShape` 与三源门一个字节都不动。
//!
//! ## 与 `r3e-terrain` 的关系
//!
//! 本模块**不认识** `r3e-terrain`（`r3e-physics` 不依赖它，也不许依赖——见 `dep_rules.json`）。
//! `r3e-terrain` 的 `TerrainMesh` 转成这里的 `HeightField` 是 `r3e-runtime` 装配层的事，
//! 和「渲染侧拿顶点数据、物理侧拿高度场」一样，属于同一次装配的两条出口。
//!
//! ## 坐标约定
//!
//! * `heights` 是**世界 Y**，不是相对基准的偏移；行主序 `heights[j * cols + i]`。
//! * `(0, 0)` 号顶点在世界 `(x0, heights[0], z0)`，`i` 沿 +X、`j` 沿 +Z 递增。
//! * 采样是**双线性**：与三角形化后的地形表面最多差半个 cell 的曲率误差，
//!   但只依赖高度数组本身，两套后端算出来的数**逐位相同**（换后端不换手感）。
//! * 网格外一律返回 `None`：地形是有限大小的场，场外的事交给地面平面与刚体。

use glam::Vec3;

use crate::port::GroundSample;

/// 地形场在 `PortHit` 里的伪句柄：地形不是刚体，没有 `BodyHandle`。
///
/// 上层只把它当「不是某个场景刚体」的标记用（和 `GroundSample::body == None` 同义）。
pub const TERRAIN_BODY: u64 = 0;

/// 地形场在 `PortHit` 里的伪节点：地形不绑定场景节点。
///
/// 取 `u64::MAX` 而不是 `0`，是因为 `NodeId` 从 1 开始发号（`Scene::spawn` 的
/// 首节点是 1），用 0 会让「未初始化」和「地形」混在一起，将来加断言时说不清。
pub const TERRAIN_NODE: u64 = u64::MAX;

/// 一块规则网格高度场。
///
/// 只装数据与采样，不装任何后端句柄：`BuiltinBackend` 直接用它做解析查询，
/// `RapierBackend` 拿它建一块 `HeightField3d` 静态碰撞体，两者读的是同一份数组。
#[derive(Debug, Clone, PartialEq)]
pub struct HeightField {
    x0: f32,
    z0: f32,
    cell: f32,
    cols: usize,
    rows: usize,
    heights: Vec<f32>,
}

impl HeightField {
    /// 构造并校验。任何一项不合法都返回 `None`（**不**静默修正成合法值）。
    ///
    /// 校验项：`cell` 有限且为正、`cols` / `rows` 至少 2（少于 2 就没有可插值的面积）、
    /// 高度数组长度正好 `cols * rows`、每个高度都是有限值。
    pub fn new(
        x0: f32,
        z0: f32,
        cell: f32,
        cols: usize,
        rows: usize,
        heights: Vec<f32>,
    ) -> Option<Self> {
        if !x0.is_finite() || !z0.is_finite() || !cell.is_finite() || cell <= 0.0 {
            return None;
        }
        if cols < 2 || rows < 2 {
            return None;
        }
        if heights.len() != cols * rows {
            return None;
        }
        if heights.iter().any(|h| !h.is_finite()) {
            return None;
        }
        Some(Self {
            x0,
            z0,
            cell,
            cols,
            rows,
            heights,
        })
    }

    /// 平地场：所有采样点同一个高度。测试与「退化成平台」的场景用。
    pub fn flat(
        x0: f32,
        z0: f32,
        cell: f32,
        cols: usize,
        rows: usize,
        height: f32,
    ) -> Option<Self> {
        if !height.is_finite() {
            return None;
        }
        Self::new(x0, z0, cell, cols, rows, vec![height; cols * rows])
    }

    /// 线性斜坡场：沿 +X 从 `low` 升到 `high`，`Z` 方向不变。
    ///
    /// 给「坡度 / 法线」类测试一个可解析计算的真值：斜率 = `(high - low) / width`。
    pub fn ramp_x(
        x0: f32,
        z0: f32,
        cell: f32,
        cols: usize,
        rows: usize,
        low: f32,
        high: f32,
    ) -> Option<Self> {
        if !low.is_finite() || !high.is_finite() || cols < 2 {
            return None;
        }
        let last = (cols - 1) as f32;
        let mut heights = Vec::with_capacity(cols * rows);
        for _j in 0..rows {
            for i in 0..cols {
                heights.push(low + (high - low) * (i as f32 / last));
            }
        }
        Self::new(x0, z0, cell, cols, rows, heights)
    }

    /// 网格 `(0, 0)` 号顶点的世界位置（y 取 0：高度逐个存在数组里）。
    pub fn origin(&self) -> Vec3 {
        Vec3::new(self.x0, 0.0, self.z0)
    }

    /// 采样间距（世界单位）。
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// X 方向的采样点数。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Z 方向的采样点数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 原始高度数组（行主序，长度 `cols * rows`）。
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// X 方向的世界跨度 `(cols - 1) * cell`。
    pub fn width(&self) -> f32 {
        (self.cols - 1) as f32 * self.cell
    }

    /// Z 方向的世界跨度 `(rows - 1) * cell`。
    pub fn depth(&self) -> f32 {
        (self.rows - 1) as f32 * self.cell
    }

    /// 第 `i` 列的采样点世界 X。
    pub fn world_x(&self, i: usize) -> f32 {
        self.x0 + i as f32 * self.cell
    }

    /// 第 `j` 行的采样点世界 Z。
    pub fn world_z(&self, j: usize) -> f32 {
        self.z0 + j as f32 * self.cell
    }

    /// 场内 `(x, z)` 处的高度（双线性插值）；场外或输入非有限返回 `None`。
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let (i, j, u, v) = self.locate(x, z)?;
        let h = &self.heights;
        let h00 = h[j * self.cols + i];
        let h10 = h[j * self.cols + i + 1];
        let h01 = h[(j + 1) * self.cols + i];
        let h11 = h[(j + 1) * self.cols + i + 1];
        let low = h00 + (h10 - h00) * u;
        let high = h01 + (h11 - h01) * u;
        Some(low + (high - low) * v)
    }

    /// 场内 `(x, z)` 处的表面法线（+Y 朝上，已归一化）；场外或输入非有限返回 `None`。
    ///
    /// 用中心差分求斜率，采样点夹在网格范围内：边界处的法线因此是「单侧斜坡的
    /// 真实斜率」而不是某个外插值。平坦场恒返回 `Vec3::Y`。
    pub fn normal_at(&self, x: f32, z: f32) -> Option<Vec3> {
        self.height_at(x, z)?;
        let half = self.cell * 0.5;
        let xm = (x - half).clamp(self.x0, self.world_x(self.cols - 1));
        let xp = (x + half).clamp(self.x0, self.world_x(self.cols - 1));
        let zm = (z - half).clamp(self.z0, self.world_z(self.rows - 1));
        let zp = (z + half).clamp(self.z0, self.world_z(self.rows - 1));
        let dx = self.height_at(xp, z)? - self.height_at(xm, z)?;
        let dz = self.height_at(x, zp)? - self.height_at(x, zm)?;
        let sx = if xp - xm > 1e-6 { dx / (xp - xm) } else { 0.0 };
        let sz = if zp - zm > 1e-6 { dz / (zp - zm) } else { 0.0 };
        Some(Vec3::new(-sx, 1.0, -sz).normalize_or_zero())
    }

    /// 场内地表采样；场外返回 `None`（`body` / `node` 恒为 `None`：地形不是刚体）。
    pub fn sample(&self, x: f32, z: f32) -> Option<GroundSample> {
        let height = self.height_at(x, z)?;
        let normal = self.normal_at(x, z)?;
        Some(GroundSample {
            height,
            normal,
            body: None,
            node: None,
        })
    }

    /// 世界包围盒 `(min, max)`：`min.y` / `max.y` 取高度数组的极值。
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for h in &self.heights {
            lo = lo.min(*h);
            hi = hi.max(*h);
        }
        (
            Vec3::new(self.x0, lo, self.z0),
            Vec3::new(self.world_x(self.cols - 1), hi, self.world_z(self.rows - 1)),
        )
    }

    /// `(x, z)` 是否落在场内（闭区间：边界上的点算场内）。
    pub fn covers(&self, x: f32, z: f32) -> bool {
        x.is_finite()
            && z.is_finite()
            && x >= self.x0
            && x <= self.world_x(self.cols - 1)
            && z >= self.z0
            && z <= self.world_z(self.rows - 1)
    }

    /// 一个 footprint（`(x, z)` + 半径）覆盖范围内的最高地表；完全在场外返回 `None`。
    ///
    /// 取 5 个采样点（中心 + 四个轴向极点）而不是遍历整个覆盖面积：
    /// 角色胶囊半径通常只有 0.3 米，而 cell 也在 1 米量级，5 点足以避免「站在
    /// 单点采样正好漏掉身边那道坎」的情形，代价与 `sample` 同级。
    pub fn peak_over(&self, x: f32, z: f32, radius: f32) -> Option<f32> {
        let r = radius.abs();
        let mut best: Option<f32> = None;
        let mut take = |h: Option<f32>| {
            if let Some(h) = h {
                best = Some(best.map_or(h, |b| b.max(h)));
            }
        };
        take(self.height_at(x, z));
        take(self.height_at(x - r, z));
        take(self.height_at(x + r, z));
        take(self.height_at(x, z - r));
        take(self.height_at(x, z + r));
        best
    }

    /// 把世界 `(x, z)` 定位到网格单元：返回 `(i, j, u, v)`，
    /// `u` / `v` 是单元内的归一化位置（`[0, 1]`）。
    ///
    /// 落在最后一行 / 列上的点归到前一个单元（`u`/`v` = 1），避免 `i + 1` 越界。
    fn locate(&self, x: f32, z: f32) -> Option<(usize, usize, f32, f32)> {
        if !self.covers(x, z) {
            return None;
        }
        let gx = (x - self.x0) / self.cell;
        let gz = (z - self.z0) / self.cell;
        // `covers` 已保证 gx / gz 在 [0, cols-1] / [0, rows-1]
        let i = (gx.floor() as usize).min(self.cols - 2);
        let j = (gz.floor() as usize).min(self.rows - 2);
        Some((
            i,
            j,
            (gx - i as f32).clamp(0.0, 1.0),
            (gz - j as f32).clamp(0.0, 1.0),
        ))
    }
}
