//! 地形高度场契约（M2 · C04 / D13）：物理侧独立地形通道。
//!
//! 覆盖三件事：
//! 1. **数据契约**：构造校验、采样、法线、包围盒、场外语义。
//! 2. **查询合成**：地形与刚体候选如何择优（射线 / 重叠 / 扫掠 / 地面采样）。
//! 3. **后端无关**：同一份场在两套后端上给出**同一个答案**，装卸地形不动刚体表与能力位。
//!
//! 所有查询类用例都对 `selectable()` 循环：`--no-default-features` 只剩内置后端时
//! 这些断言同样成立，因此「换后端不换数值」不是靠注释保证的，是靠这里跑的。

mod common;

use common::*;
use r3e_physics::heightfield::TERRAIN_NODE;
use r3e_physics::{HeightField, PhysicsBackend};

/// 平地场：20x20 格、边长 1、覆盖 x/z ∈ [-10, 10]，全部高度 `h`。
fn flat(h: f32) -> HeightField {
    HeightField::flat(-10.0, -10.0, 1.0, 21, 21, h).expect("平地场应合法")
}

/// 斜坡场：沿 +X 从 0 升到 4（斜率 0.2），Z 方向不变。
fn ramp() -> HeightField {
    HeightField::ramp_x(-10.0, -10.0, 1.0, 21, 21, 0.0, 4.0).expect("斜坡场应合法")
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3
}

// ---------------------------------------------------------------- 数据契约

#[test]
fn 非法参数构造_一律拒绝而不是静默修正() {
    // cell 必须为正且有限
    assert!(HeightField::new(0.0, 0.0, 0.0, 2, 2, vec![0.0; 4]).is_none());
    assert!(HeightField::new(0.0, 0.0, -1.0, 2, 2, vec![0.0; 4]).is_none());
    assert!(HeightField::new(f32::NAN, 0.0, 1.0, 2, 2, vec![0.0; 4]).is_none());
    // 少于 2 行 / 列没有可插值的面积
    assert!(HeightField::new(0.0, 0.0, 1.0, 1, 2, vec![0.0; 2]).is_none());
    // 长度必须正好 cols * rows
    assert!(HeightField::new(0.0, 0.0, 1.0, 2, 2, vec![0.0; 3]).is_none());
    // 高度必须有限
    assert!(HeightField::new(0.0, 0.0, 1.0, 2, 2, vec![0.0, f32::INFINITY, 0.0, 0.0]).is_none());
    // 合法的一条
    let ok = HeightField::new(0.0, 0.0, 1.0, 2, 2, vec![0.0; 4]).expect("合法");
    assert_eq!(ok.heights().len(), 4);
}

#[test]
fn 平地场_插值精确且法线朝上() {
    let field = flat(2.5);
    for (x, z) in [(0.0, 0.0), (-10.0, -10.0), (10.0, 10.0), (3.3, -7.1)] {
        assert_eq!(field.height_at(x, z), Some(2.5), "平地 ({x}, {z}) 应精确");
        let n = field.normal_at(x, z).expect("场内应有法线");
        assert!(near(n.y, 1.0), "平地表法线应朝上，实际 {n:?}");
        assert!(near(n.x, 0.0) && near(n.z, 0.0));
    }
    assert_eq!(field.width(), 20.0);
    assert_eq!(field.depth(), 20.0);
    let (lo, hi) = field.aabb();
    assert!(near(lo.y, 2.5) && near(hi.y, 2.5));
    assert!(near(lo.x, -10.0) && near(hi.x, 10.0));
}

#[test]
fn 斜坡场_法线与解析斜率一致() {
    let field = ramp();
    // 斜率 0.2 → 法线 = normalize(-0.2, 1, 0)
    let n = field.normal_at(0.0, 0.0).expect("场内");
    let expect = Vec3::new(-0.2, 1.0, 0.0).normalize();
    assert!((n - expect).length() < 1e-3, "斜坡法线 {n:?} vs {expect:?}");
    // 沿 X 单调上升，沿 Z 不变
    assert!(field.height_at(5.0, 0.0).unwrap() > field.height_at(0.0, 0.0).unwrap());
    assert_eq!(field.height_at(0.0, 3.0), field.height_at(0.0, -3.0));
    // 端点：x=-10 → 0、x=10 → 4
    assert!(near(field.height_at(-10.0, 0.0).unwrap(), 0.0));
    assert!(near(field.height_at(10.0, 0.0).unwrap(), 4.0));
}

#[test]
fn 场外采样返回空_边界算场内() {
    let field = flat(1.0);
    assert_eq!(field.height_at(-10.5, 0.0), None);
    assert_eq!(field.height_at(10.5, 0.0), None);
    assert_eq!(field.height_at(0.0, 10.5), None);
    assert_eq!(field.height_at(f32::NAN, 0.0), None);
    assert!(field.normal_at(-11.0, 0.0).is_none());
    assert!(field.sample(-11.0, 0.0).is_none());
    // 闭区间边界仍在场内
    assert!(field.covers(10.0, -10.0));
    assert_eq!(field.height_at(10.0, -10.0), Some(1.0));
}

#[test]
fn footprint_取覆盖范围内最高地表() {
    let field = ramp();
    // 坡上：x=0 处 2.0，向右 1 米半径覆盖到 2.2
    let peak = field.peak_over(0.0, 0.0, 1.0).expect("场内");
    assert!(near(peak, 2.2), "peak 应为 2.2，实际 {peak}");
    // 完全在场外 → None
    assert!(field.peak_over(-30.0, 0.0, 0.5).is_none());
}

// ---------------------------------------------------------------- 装卸与后端

#[test]
fn 装卸地形_不动刚体表也不动能力位() {
    for kind in selectable() {
        let (mut physics, _ids) = world_shapes(kind, &[cube()]);
        let bodies = physics.body_count();
        let caps = physics.capabilities();
        assert!(physics.heightfield().is_none(), "{kind:?} 默认无地形");

        physics.set_heightfield(Some(flat(2.0)));
        let field = physics.heightfield().expect("应装上");
        assert_eq!(field.cols(), 21);
        assert_eq!(physics.body_count(), bodies, "地形不进刚体表");
        assert_eq!(physics.capabilities(), caps, "地形不占能力位");

        physics.set_heightfield(None);
        assert!(physics.heightfield().is_none(), "卸掉后应为空");
        assert_eq!(physics.body_count(), bodies);
    }
}

#[test]
fn 切后端_地形原样保留() {
    let kinds = selectable();
    if kinds.len() < 2 {
        return; // 未编译 rapier 时没有第二个后端可切
    }
    let mut physics = physics_of(kinds[0]);
    physics.set_heightfield(Some(ramp()));
    physics.set_kind(kinds[1]).expect("可切");
    let field = physics.heightfield().expect("切后端后地形应还在");
    assert_eq!(field.rows(), 21);
    assert!(near(field.height_at(10.0, 0.0).unwrap(), 4.0));
    // 切回来也还在（两边都能搬）
    physics.set_kind(kinds[0]).expect("可切回");
    assert!(physics.heightfield().is_some());
}

#[test]
fn null后端装不上地形() {
    let mut null = NullBackend::new();
    null.set_heightfield(Some(flat(1.0)));
    assert!(null.heightfield().is_none(), "空后端不提供地形通道");
}

#[test]
fn 两套后端对同一场地给出同一答案() {
    let kinds = selectable();
    if kinds.len() < 2 {
        return;
    }
    let probes = [(0.0, 0.0), (5.0, -3.0), (-7.5, 8.25), (9.9, -9.9)];
    let mut results = Vec::new();
    for kind in kinds {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(ramp()));
        let ground: Vec<f32> = probes
            .iter()
            .map(|(x, z)| physics.sample_ground(*x, *z).expect("场内应有地面").height)
            .collect();
        let ray = physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 20.0, 0.0), Vec3::NEG_Y, 40.0))
            .expect("应命中地形");
        results.push((ground, ray.node, ray.distance));
    }
    let (a, b) = (&results[0], &results[1]);
    for (i, (ga, gb)) in a.0.iter().zip(b.0.iter()).enumerate() {
        assert!(near(*ga, *gb), "第 {i} 个探针地面高度漂移：{ga} vs {gb}");
    }
    assert_eq!(a.1, b.1, "命中节点应一致");
    assert!(near(a.2, b.2), "命中距离应一致：{} vs {}", a.2, b.2);
}

// ---------------------------------------------------------------- 地面采样合成

#[test]
fn 地面采样_地形高于地面平面时取地形() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(2.0)));
        let g = physics.sample_ground(1.0, 1.0).expect("应有地面");
        assert!(
            near(g.height, 2.0),
            "{kind:?} 应取地形高度，实际 {}",
            g.height
        );
        assert!(g.body.is_none(), "地形不是刚体，body 应为 None");
        assert!(g.node.is_none(), "地形不绑节点");
        assert!(near(g.normal.y, 1.0));
    }
}

#[test]
fn 地面采样_地形低于地面平面时取地面() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(-3.0)));
        let g = physics.sample_ground(0.0, 0.0).expect("应有地面");
        assert!(
            near(g.height, 0.0),
            "{kind:?} 地面平面更高，实际 {}",
            g.height
        );
    }
}

#[test]
fn 地面采样_场外回落到地面平面() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(5.0)));
        let inside = physics.sample_ground(0.0, 0.0).expect("场内");
        assert!(near(inside.height, 5.0));
        let outside = physics.sample_ground(50.0, 0.0).expect("场外回退");
        assert!(near(outside.height, 0.0), "场外应回落到地面平面");
    }
}

// ---------------------------------------------------------------- 射线

#[test]
fn 射线向下命中地形() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(2.0)));
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
            .expect("应命中地形");
        assert!(near(hit.distance, 8.0), "{kind:?} 距离 {}", hit.distance);
        assert_eq!(hit.node, TERRAIN_NODE, "应报地形哨兵节点");
        assert!(near(hit.point.y, 2.0));
        assert!(!hit.started_inside);
        assert!(near(hit.normal.y, 1.0));
    }
}

#[test]
fn 射线起点在地表下_报重叠而不是没命中() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(0.0)));
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, -1.0, 0.0), 20.0))
            .expect("起点在地下应报重叠");
        assert_eq!(hit.distance, 0.0);
        assert!(hit.started_inside);
        assert_eq!(hit.node, TERRAIN_NODE);
    }
}

#[test]
fn 射线距离不足_不命中() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(2.0)));
        // 起点 y=10，地表 y=2，只查 5 米够不着
        assert!(physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 5.0))
            .is_none());
        // 向上打也够不着（地形在脚下）
        assert!(physics
            .cast_ray(&CastRay::new(Vec3::new(0.0, 10.0, 0.0), Vec3::Y, 5.0))
            .is_none());
    }
}

#[test]
fn 射线从场外斜射进场_命中() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(0.0)));
        // 起点 x=-15（场外），朝 +X 且向下 0.5 斜率
        let ray = CastRay::new(Vec3::new(-15.0, 5.0, 0.0), Vec3::new(1.0, -0.5, 0.0), 40.0);
        let hit = physics.cast_ray(&ray).expect("应命中");
        assert!(
            hit.point.x > -10.0,
            "命中点应在场内，实际 x={}",
            hit.point.x
        );
        assert!(
            near(hit.point.y, 0.0),
            "命中点应落在地表，实际 y={}",
            hit.point.y
        );
    }
}

#[test]
fn 射线_刚体比地形近时选刚体() {
    for kind in selectable() {
        // cube 顶面 y=1.0；地形平在 y=0
        let (mut physics, ids) = world_shapes(kind, &[cube()]);
        physics.set_heightfield(Some(flat(0.0)));
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
            .expect("应命中");
        assert!(near(hit.distance, 9.0), "{kind:?} 距离 {}", hit.distance);
        assert_eq!(hit.node, ids[0], "近者是刚体");
    }
}

#[test]
fn 射线_地形比刚体近时选地形() {
    for kind in selectable() {
        // 地形抬到 y=2，cube 顶面仍在 y=1
        let (mut physics, _ids) = world_shapes(kind, &[cube()]);
        physics.set_heightfield(Some(flat(2.0)));
        let hit = physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
            .expect("应命中");
        assert!(near(hit.distance, 8.0), "{kind:?} 距离 {}", hit.distance);
        assert_eq!(hit.node, TERRAIN_NODE, "近者是地形");
    }
}

// ---------------------------------------------------------------- 胶囊重叠 / 扫掠

#[test]
fn 胶囊重叠_地表从胶囊中间穿过() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(1.0)));
        // 胶囊中心 y=1、半高 0.5、半径 0.4 → 底面 0.1、顶面 1.9，地表 1.0 夹在中间
        let cap = capsule(Vec3::new(0.0, 1.0, 0.0), 0.5, 0.4);
        let hits = physics.capsule_overlap(&cap);
        assert_eq!(hits.len(), 1, "{kind:?} 应有一条地形重叠");
        assert!(hits[0].started_inside);
        assert_eq!(hits[0].node, TERRAIN_NODE);
        assert!(near(hits[0].point.y, 1.0));
    }
}

#[test]
fn 胶囊悬空在地形上方_无重叠() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(0.0)));
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 0.5, 0.4);
        assert!(physics.capsule_overlap(&cap).is_empty());
    }
}

#[test]
fn 胶囊向下扫掠_给出落地距离() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(1.0)));
        // 底面 = 5 - 0.9 = 4.1；下落到 1.0 需 3.1
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 0.5, 0.4);
        let hit = physics
            .capsule_sweep(&cap, Vec3::NEG_Y, 10.0)
            .expect("应命中地形");
        assert!(near(hit.distance, 3.1), "{kind:?} 距离 {}", hit.distance);
        assert!(!hit.started_inside);
        assert_eq!(hit.node, TERRAIN_NODE);
        assert!(near(hit.point.y, 1.0));
    }
}

#[test]
fn 胶囊向上扫掠_碰不到脚下地表() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(0.0)));
        let cap = capsule(Vec3::new(0.0, 5.0, 0.0), 0.5, 0.4);
        assert!(physics.capsule_sweep(&cap, Vec3::Y, 10.0).is_none());
        // 水平扫掠同理：高度场是单值面，没有侧面可撞
        assert!(physics.capsule_sweep(&cap, Vec3::X, 10.0).is_none());
    }
}

#[test]
fn 胶囊贴地扫掠_报重叠() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(1.0)));
        // 底面正好 1.0（贴地）
        let cap = capsule(Vec3::new(0.0, 1.9, 0.0), 0.5, 0.4);
        let hit = physics
            .capsule_sweep(&cap, Vec3::NEG_Y, 2.0)
            .expect("贴地应报重叠");
        assert_eq!(hit.distance, 0.0);
        assert!(hit.started_inside);
    }
}

#[test]
fn 场地外的胶囊查询_拿不到地形候选() {
    for kind in selectable() {
        let mut physics = empty(kind);
        physics.set_heightfield(Some(flat(0.0)));
        let cap = capsule(Vec3::new(50.0, 5.0, 0.0), 0.5, 0.4);
        assert!(physics.capsule_overlap(&cap).is_empty());
        assert!(physics.capsule_sweep(&cap, Vec3::NEG_Y, 10.0).is_none());
    }
}

// ---------------------------------------------------------------- 总闸

#[test]
fn 物理关闭时_地形查询一律为空() {
    for kind in selectable() {
        let mut physics = physics_of(kind);
        physics.set_heightfield(Some(flat(1.0)));
        physics.set_enabled(false);
        assert!(physics
            .cast_ray(&down(Vec3::new(0.0, 10.0, 0.0), 20.0))
            .is_none());
        assert!(physics
            .capsule_overlap(&capsule(Vec3::new(0.0, 1.0, 0.0), 0.5, 0.4))
            .is_empty());
        assert!(physics
            .capsule_sweep(
                &capsule(Vec3::new(0.0, 5.0, 0.0), 0.5, 0.4),
                Vec3::NEG_Y,
                10.0
            )
            .is_none());
        assert!(physics.sample_ground(0.0, 0.0).is_none());
        // 地形数据本身还在：总闸关的是查询，不是数据
        assert!(physics.heightfield().is_some());
    }
}

#[test]
fn 未被地形覆盖的端口能力位不变() {
    for kind in selectable() {
        let mut physics = empty(kind);
        let caps = physics.port_caps();
        physics.set_heightfield(Some(flat(1.0)));
        assert_eq!(
            physics.port_caps(),
            caps,
            "{kind:?} 端口能力位不该因地形变动"
        );
    }
}
