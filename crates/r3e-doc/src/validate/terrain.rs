//! 地形数据自洽与地表覆盖（M2-C06）。
//!
//! 两条判据，都只吃**场景里写下的地形块**，不碰 `r3e-terrain`（`r3e-doc` 的依赖边
//! 只到 `r3e-core`，地形采样口径已在 C01 下沉到 [`r3e_core::TerrainPatch::sample_height`]）：
//!
//! 1. `terrain.weight_not_normalized` —— 块的高度样本与 `resolution` 对不上、或含
//!    非有限值。`terrain.mesh` 产出的顶点混合权重是「按坡度在 0..1 上归一」的结果，
//!    样本数不符 / 含 NaN 时归一的前提就不成立：要么权重整排变 `NaN`（渲染出来是
//!    一块黑），要么按补齐的 `0.0` 算出一份「看着正常、其实不是这块地」的地形。
//!    这条判定的是**根因**，不是让上层去逐个顶点比 1。
//! 2. `terrain.block_gap` —— 地表内部有洞：并集矩形内某个采样格没有块覆盖，而它的
//!    四邻至少有一格被覆盖。边界外的空白是正常的（关卡总有边），只有被地表**包起来**
//!    的空洞才可疑——角色走过去会掉出去。判 `Warn`：空洞也可能是设计（天坑、井口）。
//!
//! **为什么不报「块与块之间没接缝」**：块矩形由用户按世界坐标摆，接缝差几厘米是
//! 常态，报了会被当成噪声；`block_gap` 只看格级覆盖，不看厘米级对齐。

use r3e_core::Scene;

use super::Issue;

/// 地表覆盖的采样步长（米）。
///
/// 取 8 米：比 M2 地形块的常规尺寸（16 米起）细一档，能抓到「删掉中间一块」这种
/// 格级空洞，又不会因为一个块内部插值起伏误报。这是个**采样口径**，不是地形块的
/// 划分尺寸——后者归 `r3e-terrain`，两处不要混用。
const COVER_STEP: f32 = 8.0;

pub(crate) fn check(scene: &Scene, out: &mut Vec<Issue>) {
    for patch in &scene.terrain {
        let expected = patch.expected_samples();
        let mismatched = !patch.heights.is_empty() && patch.heights.len() != expected;
        let non_finite = patch.heights.iter().any(|h| !h.is_finite());
        if mismatched || non_finite {
            let why = if non_finite {
                "高度样本含非有限值".to_string()
            } else {
                format!(
                    "高度样本数 {} 与分辨率 {}×{} 不符（应为 {}）",
                    patch.heights.len(),
                    patch.resolution[0],
                    patch.resolution[1],
                    expected
                )
            };
            let center = [
                patch.origin[0] + patch.size[0] * 0.5,
                0.0,
                patch.origin[1] + patch.size[1] * 0.5,
            ];
            out.push(
                Issue::error(
                    "terrain.weight_not_normalized",
                    format!("terrain[{}]", patch.name),
                    format!(
                        "地形块「{}」{why}，坡度混合权重无法归一（`terrain.mesh` 会产出 NaN 或错位权重）",
                        patch.name
                    ),
                )
                .with_target(patch.name.clone())
                .at(center)
                .with_hint("重写这块地形：`terrain.create` 或 `terrain.resample` 会按分辨率重新采样"),
            );
        }
    }

    check_gaps(scene, out);
}

/// 并集矩形内「四周有地、自己没有」的采样格。
fn check_gaps(scene: &Scene, out: &mut Vec<Issue>) {
    if scene.terrain.len() < 2 {
        // 单块地形谈不上内部空洞，边界外全是空白——不判。
        return;
    }
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for p in &scene.terrain {
        min[0] = min[0].min(p.origin[0]);
        min[1] = min[1].min(p.origin[1]);
        max[0] = max[0].max(p.origin[0] + p.size[0]);
        max[1] = max[1].max(p.origin[1] + p.size[1]);
    }
    if !(min[0].is_finite() && min[1].is_finite() && max[0].is_finite() && max[1].is_finite()) {
        return;
    }

    let cols = (((max[0] - min[0]) / COVER_STEP).ceil() as i32).max(1);
    let rows = (((max[1] - min[1]) / COVER_STEP).ceil() as i32).max(1);
    // 采样格上限：并集矩形再大也只扫这么多（4 万格 ≈ 几公里见方），
    // 避免一份「故意摆远」的场景把校验拖成秒级。
    const MAX_CELLS: i32 = 40_000;
    if cols.saturating_mul(rows) > MAX_CELLS {
        return;
    }

    let covered: Vec<bool> = (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |col| {
                let x = min[0] + (col as f32 + 0.5) * COVER_STEP;
                let z = min[1] + (row as f32 + 0.5) * COVER_STEP;
                scene.terrain.iter().any(|p| inside(p.origin, p.size, x, z))
            })
        })
        .collect();
    let at = |col: i32, row: i32| -> bool {
        col >= 0 && row >= 0 && col < cols && row < rows && covered[(row * cols + col) as usize]
    };

    let mut holes = 0usize;
    let mut first: Option<[f32; 3]> = None;
    for row in 0..rows {
        for col in 0..cols {
            if at(col, row) {
                continue;
            }
            let ring = at(col - 1, row) || at(col + 1, row) || at(col, row - 1) || at(col, row + 1);
            if !ring {
                continue;
            }
            holes += 1;
            if first.is_none() {
                first = Some([
                    min[0] + (col as f32 + 0.5) * COVER_STEP,
                    0.0,
                    min[1] + (row as f32 + 0.5) * COVER_STEP,
                ]);
            }
        }
    }

    if holes > 0 {
        let mut issue = Issue::warn(
            "terrain.block_gap",
            "terrain",
            format!(
                "地表内部有 {holes} 处空地（按 {COVER_STEP} 米采样，四周有地形但该处无块覆盖）：角色走到这里会直接掉出场景"
            ),
        )
        .with_target("terrain".to_string())
        .with_hint("补一块地形填洞，或确认它是设计中的天坑 / 井口");
        if let Some(p) = first {
            issue = issue.at(p);
        }
        out.push(issue);
    }
}

fn inside(origin: [f32; 2], size: [f32; 2], x: f32, z: f32) -> bool {
    x >= origin[0] && x <= origin[0] + size[0] && z >= origin[1] && z <= origin[1] + size[1]
}
