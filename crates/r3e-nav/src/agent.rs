//! 沿路行走的代理（`M2·D03`）：只负责「贴着折线走、到点切段、脚贴地面」。
//!
//! ## 为什么不接 `r3e-character`
//!
//! `M2_D_PLAN` §3.6 的裁决是**不引**：`r3e-nav` 的允许依赖里没有它，也不该有
//! （导航不该反向依赖角色控制器）。D03 要证明的只有一件事——「按这条折线走，
//! 不会切角穿墙、不会浮空」；真实的角色驱动（转向、加减速、动画）留给 `H03`，
//! 由 `r3e-character` 在更高层拼装。于是这里的代理刻意做得很笨：**只沿线段推进**，
//! 不做任何避障试探。
//!
//! ## 三处必须自己做的修正
//!
//! 1. **投影纠偏**：位置若被推离当前段，先拉回段上的投影再推进，避免「浮点漂移 +
//!    段间累加」让轨迹慢慢歪出路面；
//! 2. **落脚判可走**：新位置若落进不可走格（理论上不会，平滑已保证看得通，
//!    但被外力推开后就可能了），本帧就停在原地并报 [`AgentStep::off_path`]——
//!    宁可卡一下，也不把代理放进墙里；
//! 3. **抬脚贴地**：每推进一次都用 [`NavGrid::ground_at`] 把 `y` 贴回地面，
//!    所以「浮空 / 下沉」在这里根本不会发生（判据 5 由构造保证）。

use glam::Vec3;

use crate::grid::NavGrid;

/// 纠偏阈值（倍格长）：当前位置离当前段中轴超过「`max(格长, 身位) × 这个系数」，
/// 就认定已经脱离路径，主动纠偏并把 `off_path` 报给调用方。
const OFF_PATH_SLACK: f32 = 1.0;

/// 一次 `advance` 的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentStep {
    /// 是否已经走到路径终点。
    pub arrived: bool,
    /// 本次是否发生了偏离（被推出路径 / 被挡回）。
    pub off_path: bool,
    /// 本次实际推进的水平距离（米）——「速度 × dt」是上限，不是保证。
    pub progressed: f32,
}

/// 沿折线行走的代理。
#[derive(Debug, Clone, PartialEq)]
pub struct NavAgent {
    pos: Vec3,
    path: Vec<Vec3>,
    /// 当前所在段的**起点**索引；段是 `path[seg] → path[seg + 1]`。
    seg: usize,
}

impl NavAgent {
    /// 站到路径起点，准备走完全程。
    pub fn new(path: &[Vec3]) -> Self {
        let path: Vec<Vec3> = path.to_vec();
        let pos = path.first().copied().unwrap_or(Vec3::ZERO);
        Self { pos, path, seg: 0 }
    }

    /// 从「不在路径起点」的位置续行（被击退、传送落地后都走这里）。
    ///
    /// 会把 `seg` 挪到离当前位置最近的那一段；已在终点附近则直接判定到达。
    pub fn at(path: &[Vec3], pos: Vec3) -> Self {
        let mut agent = Self::new(path);
        agent.pos = pos;
        agent.resync();
        agent
    }

    pub fn pos(&self) -> Vec3 {
        self.pos
    }

    /// 当前段序号（`0` = 第一段）；到点后为 `path.len() - 1`（越过最后一段）。
    pub fn seg(&self) -> usize {
        self.seg
    }

    pub fn path(&self) -> &[Vec3] {
        &self.path
    }

    /// 是否已到达（空路径 / 单点路径从一开始就算到达）。
    pub fn arrived(&self) -> bool {
        self.path.len() < 2 || self.seg + 1 >= self.path.len()
    }

    /// 剩余折线长度（米）：中途切段、加减速都不影响这个量。
    pub fn remaining(&self) -> f32 {
        if self.arrived() {
            return 0.0;
        }
        let mut rest = flat_len(self.pos, self.path[self.seg + 1]);
        for w in self.path[self.seg + 1..].windows(2) {
            rest += w[0].distance(w[1]);
        }
        rest
    }

    /// 推进一帧：`dt` 秒 × `speed` 米每秒的位移预算，可以跨多段。
    ///
    /// 非法 `dt` / `speed`（非有限、非正）按 0 处理——不 panic，也不让 `NaN`
    /// 写进位置（那会把后续所有判定都变成假）。
    pub fn advance(&mut self, grid: &NavGrid, dt: f32, speed: f32) -> AgentStep {
        let budget = if dt.is_finite() && speed.is_finite() && dt > 0.0 && speed > 0.0 {
            dt * speed
        } else {
            0.0
        };
        if self.arrived() {
            self.snap(grid);
            return AgentStep {
                arrived: true,
                off_path: false,
                progressed: 0.0,
            };
        }

        let mut left = budget;
        let mut progressed = 0.0f32;
        let mut off_path = false;
        // 段数有限，循环次数就有界；余量一格用来容纳「零长段」。
        let mut guard = self.path.len() + 1;
        while left > 1e-6 && !self.arrived() && guard > 0 {
            guard -= 1;
            let target = self.path[self.seg + 1];
            let dist = flat_len(self.pos, target);
            if dist <= 1e-4 {
                self.advance_seg(grid);
                continue;
            }
            match self.aim(grid, target, left) {
                Some(step) => {
                    left -= step;
                    progressed += step;
                    if step >= dist - 1e-4 {
                        self.advance_seg(grid);
                    }
                }
                None => {
                    // 候选不可走：这一帧原地不动，把偏离如实报出去。
                    off_path = true;
                    break;
                }
            }
        }

        self.snap(grid);
        AgentStep {
            arrived: self.arrived(),
            off_path,
            progressed,
        }
    }

    /// 朝 `target` 走最多 `budget` 米，返回实际推进距离；候选不可走时 `None`。
    ///
    /// 顺序很重要：**先纠偏、再判可走、最后才落点**。落到最后才能保证
    /// 「不可走就不动」，而不是「先动进去再退回」。
    fn aim(&mut self, grid: &NavGrid, target: Vec3, budget: f32) -> Option<f32> {
        let from = self.path[self.seg];
        let origin = self.pos;
        let slack = grid.config().cell_size.max(grid.config().agent_radius) * OFF_PATH_SLACK;

        // 1) 纠偏：位置若被推离当前段，先拉回段上的投影。
        let seg = Vec3::new(target.x - from.x, 0.0, target.z - from.z);
        let seg_len2 = seg.length_squared();
        if seg_len2 > 1e-8 {
            let t = ((origin.x - from.x) * seg.x + (origin.z - from.z) * seg.z) / seg_len2;
            let t = t.clamp(0.0, 1.0);
            let proj = Vec3::new(from.x + seg.x * t, origin.y, from.z + seg.z * t);
            if flat_len(origin, proj) > slack {
                self.pos = proj;
            }
        }

        // 2) 推进：按到目标的水平距离截断，一帧最多走完这一段。
        let to = Vec3::new(target.x - self.pos.x, 0.0, target.z - self.pos.z);
        let dist = to.length();
        if !dist.is_finite() {
            return None;
        }
        let step = budget.min(dist);
        let candidate = if dist > 1e-6 {
            let dir = to / dist;
            Vec3::new(
                self.pos.x + dir.x * step,
                self.pos.y,
                self.pos.z + dir.z * step,
            )
        } else {
            self.pos
        };
        // 3) 落脚判可走：不许把代理放进墙里。
        if !grid.is_walkable(candidate) {
            return None;
        }
        self.pos = candidate;
        Some(step)
    }

    /// 切到下一段；走完最后一段就把位置钉在终点上，并把 `seg` 推过最后一段。
    ///
    /// 这里必须把 `seg` 设成 `len - 1`（而不是 `len - 2`）：[`Self::arrived`] 的
    /// 判据是 `seg + 1 >= len`，停在最后一个段号上会**永远到不了点**——
    /// 后续每帧都在「零距离段」上空转，直到调用方超时。
    fn advance_seg(&mut self, grid: &NavGrid) {
        if self.seg + 2 >= self.path.len() {
            self.seg = self.path.len() - 1;
            self.pos = *self.path.last().expect("非空路径");
            self.snap(grid);
            return;
        }
        self.seg += 1;
    }

    /// 把 `y` 贴回所在格的地面。
    fn snap(&mut self, grid: &NavGrid) {
        if let Some(y) = grid.ground_at(self.pos) {
            self.pos.y = y;
        }
    }

    /// 续行时把 `seg` 挪到「离当前位置最近」的那一段。
    fn resync(&mut self) {
        if self.path.len() < 2 {
            self.seg = 0;
            return;
        }
        let mut best = (f32::INFINITY, 0usize);
        for k in 0..self.path.len() - 1 {
            let (a, b) = (self.path[k], self.path[k + 1]);
            let seg = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
            let len2 = seg.length_squared();
            let t = if len2 > 1e-8 {
                ((self.pos.x - a.x) * seg.x + (self.pos.z - a.z) * seg.z) / len2
            } else {
                1.0
            };
            let t = t.clamp(0.0, 1.0);
            let p = Vec3::new(a.x + seg.x * t, self.pos.y, a.z + seg.z * t);
            let d = flat_len(self.pos, p);
            if d < best.0 {
                best = (d, k);
            }
        }
        self.seg = best.1;
    }
}

/// 水平距离。
fn flat_len(a: Vec3, b: Vec3) -> f32 {
    ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt()
}
