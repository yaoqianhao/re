//! 物理 / 物理开关与后端 / CCD。
//!
//! M2-A03b：全部物理调用经 [`crate::host::PhysicsHost`] 走能力面，
//! 本文件只做「脚本参数 ↔ 脚本友好 DTO」的搬运，不认任何物理实现类型。

use super::*;

/// 注册「物理 / 物理开关与后端 / CCD」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 物理 ----------------
    // 物理默认关闭：null 后端只登记刚体。通过 `physics_set_enabled(true)` 或
    // 场景 JSON / 命令行开启后，内置后端开始积分与地面碰撞。
    let c = ctx.clone();
    engine.register_fn(
        "attach_body",
        move |name: &str, kind: &str, mass: f64| -> bool {
            // 脚本里常见「create_node → attach_body」连写，此时节点的世界矩阵还没刷新，
            // 直接读 world_position() 会拿到原点，刚体就会落在错误位置（查询也会跟着偏）。
            c.write(|s| s.update_world_transforms());
            let scene = c.doc.borrow();
            let Some(node) = scene.find(name) else {
                println!("[script] attach_body: 物件 `{name}` 不存在");
                return false;
            };
            let id = node.id;
            let handle = c.phys.attach(node, kind, mass);
            drop(scene);
            let Some(handle) = handle else {
                println!(
                    "[script] attach_body: 物理未启用（physics_set_enabled(true) 或 --physics on）"
                );
                return false;
            };
            c.write(|s| {
                if let Some(n) = s.get_mut(id) {
                    n.body = Some(handle);
                }
            });
            true
        },
    );

    let c = ctx.clone();
    engine.register_fn("detach_body", move |name: &str| -> bool {
        let id = {
            let scene = c.doc.borrow();
            let Some(node) = scene.find(name) else {
                return false;
            };
            node.id
        };
        let handle = c.write(|s| s.get_mut(id).and_then(|n| n.body.take()));
        if let Some(h) = handle {
            c.phys.detach(h);
            true
        } else {
            false
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "body_impulse",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            let handle = {
                let scene = c.doc.borrow();
                match scene.find(name).and_then(|n| n.body) {
                    Some(h) => h,
                    None => return false,
                }
            };
            c.phys.impulse(handle, [x, y, z]);
            true
        },
    );

    let c = ctx.clone();
    engine.register_fn("body_count", move || -> i64 { c.phys.body_count() });

    // ---- 物理开关 / 后端 / 环境参数 ----

    let c = ctx.clone();
    engine.register_fn("physics_enabled", move || -> bool { c.phys.enabled() });

    let c = ctx.clone();
    engine.register_fn("physics_set_enabled", move |on: bool| -> bool {
        let now = c.phys.set_enabled(on);
        println!("[script] 物理{}", if now { "已启用" } else { "已关闭" });
        now
    });

    let c = ctx.clone();
    engine.register_fn("physics_backend", move || -> String { c.phys.backend() });

    // 运行时切换后端；切换会清空已有刚体。`rapier` 在当前构建不可用时返回 false。
    let c = ctx.clone();
    engine.register_fn("physics_set_backend", move |name: &str| -> bool {
        c.phys.set_backend(name)
    });

    // 后端能力位：脚本据此判断能不能指望互撞 / 旋转 / 碰撞事件，别按后端名写分支。
    let c = ctx.clone();
    engine.register_fn("physics_capabilities", move || -> String {
        c.phys.capabilities()
    });

    let c = ctx.clone();
    engine.register_fn("physics_has_capability", move |key: &str| -> bool {
        c.phys.has_capability(key)
    });

    // 射线查询：命中返回 [node, distance, px, py, pz]，未命中返回空数组。
    // 方向不必先归一化（Ray 构造时归一），max_dist 用世界单位。
    let c = ctx.clone();
    engine.register_fn(
        "physics_raycast",
        move |ox: f64, oy: f64, oz: f64, dx: f64, dy: f64, dz: f64, max_dist: f64| -> Array {
            match c.phys.raycast([ox, oy, oz], [dx, dy, dz], max_dist) {
                Some(hit) => vec![
                    Dynamic::from(hit.node),
                    Dynamic::from(hit.distance),
                    Dynamic::from(hit.point[0]),
                    Dynamic::from(hit.point[1]),
                    Dynamic::from(hit.point[2]),
                ],
                None => Array::new(),
            }
        },
    );

    // 射线查询的**具名**版本：字段与 shape-cast 完全一致
    // （hit / node / handle / distance / point / normal / inside），
    // 老脚本继续用数组版 `physics_raycast`（行为不变），新代码 / IDE 侧建议用这个：
    // 两者返回同一套语义，只是取值方式不同，迁移时把下标换成 `r.node` / `r.distance` 即可。
    let c = ctx.clone();
    engine.register_fn(
        "physics_raycast_full",
        move |ox: f64, oy: f64, oz: f64, dx: f64, dy: f64, dz: f64, max_dist: f64| -> Map {
            shape_cast_map(c.phys.raycast([ox, oy, oz], [dx, dy, dz], max_dist))
        },
    );

    // 球体重叠：返回命中节点的 id 数组（按 id 升序），空数组即没碰到。
    let c = ctx.clone();
    engine.register_fn(
        "physics_overlap",
        move |x: f64, y: f64, z: f64, radius: f64| -> Array {
            c.phys
                .overlap([x, y, z], radius)
                .into_iter()
                .map(Dynamic::from)
                .collect()
        },
    );

    // 形状查询（shape-cast）：把「球 / 盒」沿方向推过去，回答「推多远会撞到谁」。
    //
    // 返回**具名 Map**（不是下标数组），IDE 补全 / 脚本按名取值都不用记顺序：
    //   #{ hit: bool, node: int, handle: int, distance: float,
    //      point: [x, y, z], normal: [x, y, z], inside: bool }
    // 未命中时字段齐备：hit=false、node/handle=-1、distance=-1、point/normal 为空数组，
    // 脚本统一写 `if r.hit { ... }` 即可，不需要先判长度。
    let c = ctx.clone();
    engine.register_fn(
        "physics_shape_cast_sphere",
        move |ox: f64,
              oy: f64,
              oz: f64,
              dx: f64,
              dy: f64,
              dz: f64,
              radius: f64,
              max_dist: f64|
              -> Map {
            shape_cast_map(
                c.phys
                    .shape_cast_sphere([ox, oy, oz], radius, [dx, dy, dz], max_dist),
            )
        },
    );

    // 盒扫掠：hx / hy / hz 是**半尺寸**（不是整尺寸，别把整宽填进来）。
    let c = ctx.clone();
    engine.register_fn(
        "physics_shape_cast_box",
        move |ox: f64,
              oy: f64,
              oz: f64,
              dx: f64,
              dy: f64,
              dz: f64,
              hx: f64,
              hy: f64,
              hz: f64,
              max_dist: f64|
              -> Map {
            shape_cast_map(c.phys.shape_cast_box(
                [ox, oy, oz],
                [hx, hy, hz],
                [dx, dy, dz],
                max_dist,
            ))
        },
    );

    // ---- CCD（连续碰撞检测）----

    // 全局开关：改的是「刚体没显式指定时」的缺省值，并把当前所有刚体一起改掉。
    // 返回 true 表示后端真的支持 CCD；不支持（如内置后端）时如实返回 false。
    let c = ctx.clone();
    engine.register_fn("physics_set_ccd", move |on: bool| -> bool {
        c.phys.set_ccd(on)
    });

    let c = ctx.clone();
    engine.register_fn("physics_ccd_default", move || -> bool {
        c.phys.ccd_default()
    });

    // 逐刚体开关：目标是动态刚体才有意义（静态 / 运动学由脚本驱动）。
    let c = ctx.clone();
    engine.register_fn("body_set_ccd", move |name: &str, on: bool| -> bool {
        let handle = {
            let scene = c.doc.borrow();
            match scene.find(name).and_then(|n| n.body) {
                Some(h) => h,
                None => {
                    println!("[script] body_set_ccd: 物件 `{name}` 没有刚体");
                    return false;
                }
            }
        };
        let ok = c.phys.body_set_ccd(handle, on);
        if !ok {
            println!("[script] body_set_ccd: `{name}` 不是动态刚体，或后端不支持 CCD");
        }
        ok
    });

    // 读回 CCD 状态：无刚体 / 后端不支持时返回 false。
    let c = ctx.clone();
    engine.register_fn("body_ccd", move |name: &str| -> bool {
        let handle = {
            let scene = c.doc.borrow();
            match scene.find(name).and_then(|n| n.body) {
                Some(h) => h,
                None => return false,
            }
        };
        c.phys.body_ccd(handle)
    });

    // 取走自上次调用以来的碰撞事件（形如 "started:7-none"）；不支持事件的后端返回空数组。
    let c = ctx.clone();
    engine.register_fn("physics_take_events", move || -> Array {
        c.phys
            .take_events()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("physics_gravity", move || -> f64 { c.phys.gravity() });

    let c = ctx.clone();
    engine.register_fn("physics_set_gravity", move |x: f64, y: f64, z: f64| {
        c.phys.set_gravity([x, y, z]);
    });

    let c = ctx.clone();
    engine.register_fn("physics_ground", move || -> f64 { c.phys.ground() });

    let c = ctx.clone();
    engine.register_fn("physics_set_ground", move |y: f64| {
        c.phys.set_ground(y);
    });
}
