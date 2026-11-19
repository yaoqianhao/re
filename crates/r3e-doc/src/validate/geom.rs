//! 静态层几何近似（M1-S4-E）：只依赖 `r3e-core`，给 `support.rs` / `overlap.rs` 两个检查共用。
//!
//! **为什么是近似**：静态层手里只有场景结构，没有资产库（网格顶点 / 骨骼 / 蒙皮），
//! 拿不到真实包围盒。这里的口径是：
//!
//! 1. 半尺寸优先取节点自己声明的 `PhysicsDesc::half_extents`（物理层装配用的是同一份声明，
//!    于是静态层的盒子与物理层**同源**，结论方向不会打架）；
//! 2. 没声明就退到「内置单位网格（1×1×1，见 `r3e_core::procedural`）× |scale| × 0.5」；
//! 3. 世界盒 = 局部盒八个角点过 `Node::world` 后重新求包围（旋转会把盒子撑大，属预期）。
//!
//! 凡是吃了这份近似的结论，都必须标 `Issue::approximate = true`，由物理层出结论后覆盖——
//! 铁律见 `docs/m1/M1_S4_PLAN.md` §4 S4-E「铁律①」。

use r3e_core::{Aabb, Node, Scene, Vec3};

/// 无地形覆盖时的地面高度：与 `r3e_physics::PhysicsConfig` 的缺省地面 `y = 0` 同口径。
pub(crate) const FLOOR_Y: f32 = 0.0;

/// 节点近似世界 AABB。退化节点（scale 含 0 / 非有限）返回 `None`，不参与几何判定。
///
/// **半尺寸口径必须与物理层一致**（`r3e_physics::bind` 的装配规则）：
///
/// - 声明了 `PhysicsDesc::half_extents` → 直接当**世界半尺寸**用，不再乘 scale
///   （`bind.rs` 里显式声明就是最终值，人工调过的数优先）；
/// - 没声明 → `0.5 × |scale|`（内置单位网格 1×1×1 的半个身位，再按缩放拉伸）。
///
/// 早先的写法是「局部盒过一遍 `world` 矩阵」，那样声明的半尺寸会被 scale 再乘一次
/// （scale=8、声明 0.1 的物件被算成 0.8），与物理层装配结果差 8 倍——层间打架的来源。
///
/// 旋转不入盒：轴对齐近似会略小于真实占用，结论全部标 `approximate`，
/// 由物理层精确复核，符合铁律①「近似只做候选」。
pub(crate) fn node_aabb(node: &Node) -> Option<Aabb> {
    let s = node.transform.scale;
    let scale = Vec3::new(s.x.abs(), s.y.abs(), s.z.abs());
    if !scale.is_finite() || scale.min_element() <= 0.0 {
        return None;
    }

    let declared = node
        .physics
        .as_ref()
        .and_then(|p| p.half_extents)
        .filter(|h| h.iter().all(|v| v.is_finite() && *v > 0.0));
    let half = match declared {
        Some(h) => Vec3::from_array(h),
        None => scale * 0.5,
    };
    if !half.is_finite() || half.min_element() <= 0.0 {
        return None;
    }

    let center = node.world.w_axis.truncate();
    if !center.is_finite() {
        return None;
    }
    Some(Aabb::from_center_half(center, half))
}

/// 地面高度采样：所有覆盖该点的地形块取**最高**那层（与 `r3e-nav` 烘焙同口径）。
///
/// 没有任何地形块覆盖时返回 `None`——调用方决定要不要退到 [`FLOOR_Y`]。
pub(crate) fn terrain_height(scene: &Scene, x: f32, z: f32) -> Option<f32> {
    let mut h: Option<f32> = None;
    for patch in &scene.terrain {
        if let Some(y) = patch_height(patch, x, z) {
            h = Some(h.map_or(y, |cur: f32| cur.max(y)));
        }
    }
    h
}

/// 单块地形的双线性高度采样；点不在块内返回 `None`。
fn patch_height(patch: &r3e_core::TerrainPatch, x: f32, z: f32) -> Option<f32> {
    let (w, d) = (patch.size[0], patch.size[1]);
    if !(w.is_finite() && d.is_finite()) || w <= 0.0 || d <= 0.0 {
        return None;
    }
    let u = (x - patch.origin[0]) / w;
    let v = (z - patch.origin[1]) / d;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let [rx, ry] = patch.resolution;
    // 平坦地形：`heights` 空数组就是「全 0」的合法表示。
    if patch.heights.is_empty() || rx < 2 || ry < 2 {
        return Some(0.0);
    }
    if patch.heights.len() != (rx * ry) as usize {
        // 样本数与分辨率不符 → 数据已坏，按平坦处理而不是拿越界值硬算（与 nav 同口径）。
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
    let top = h00 * (1.0 - tx) + h10 * tx;
    let bottom = h01 * (1.0 - tx) + h11 * tx;
    Some(top * (1.0 - tz) + bottom * tz)
}

/// 一个物件是否「会动」：`body` 声明为 `dynamic` 的刚体每帧都在动，静态层不对它判悬空 / 穿模。
pub(crate) fn is_dynamic(node: &Node) -> bool {
    node.physics
        .as_ref()
        .map(|p| p.body.trim().eq_ignore_ascii_case("dynamic"))
        .unwrap_or(false)
}

/// `a` 是否是 `b` 的祖先或后代（父子之间体积重叠是正常的层次结构，不该报穿模）。
pub(crate) fn related(scene: &Scene, a: &Node, b: &Node) -> bool {
    // 上溯深度设上限：手工拼出来的父子环不该让校验器死循环（校验器不 panic 是铁律）。
    const MAX_DEPTH: usize = 64;

    fn ancestor_of(scene: &Scene, start: r3e_core::NodeId, want: r3e_core::NodeId) -> bool {
        let mut cur = scene.get(start).and_then(|n| n.parent);
        let mut depth = 0;
        while let Some(id) = cur {
            if id == want {
                return true;
            }
            depth += 1;
            if depth > MAX_DEPTH {
                // 结构病态：当成「有关」以抑制噪音，环本身归写入层 / refs 层管。
                return true;
            }
            cur = scene.get(id).and_then(|n| n.parent);
        }
        false
    }

    ancestor_of(scene, a.id, b.id) || ancestor_of(scene, b.id, a.id)
}
