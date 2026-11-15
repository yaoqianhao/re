//! 物理 / 物理开关与后端 / CCD。
//!
//! 由 M0-S2 从单体 `api.rs` 原样搬入，闭包体未做任何改写。

use super::*;

/// 注册「物理 / 物理开关与后端 / CCD」域的脚本函数。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // ---------------- 物理 ----------------
    // 物理默认关闭：null 后端只登记刚体。通过 `physics_set_enabled(true)` 或
    // 场景 JSON / 命令行开启后，内置后端开始积分与地面碰撞。
    let c = ctx.clone();
    engine.register_fn("attach_body", move |name: &str, kind: &str, mass: f64| -> bool {
        // 脚本里常见「create_node → attach_body」连写，此时节点的世界矩阵还没刷新，
        // 直接读 world_position() 会拿到原点，刚体就会落在错误位置（查询也会跟着偏）。
        c.write(|s| s.update_world_transforms());
        let scene = c.doc.borrow();
        let Some(node) = scene.find(name) else {
            println!("[script] attach_body: 物件 `{name}` 不存在");
            return false;
        };
        let mut desc = BodyDesc::from_node(node).unwrap_or_else(|| BodyDesc::infer(node));
        desc.kind = BodyKind::parse(kind);
        desc.mass = if mass > 0.0 { f32_of(mass) } else { 1.0 };
        desc.position = node.world_position();
        let id = node.id;
        // 重复 attach 时先回收旧刚体，避免同一节点挂两个
        let old = node.body.map(BodyHandle);
        drop(scene);
        if let Some(h) = old {
            c.physics.borrow_mut().destroy_body(h);
        }

        let handle = match c.physics.borrow_mut().try_create_body(id, desc) {
            Some(h) => h,
            None => {
                println!("[script] attach_body: 物理未启用（physics_set_enabled(true) 或 --physics on）");
                return false;
            }
        };
        c.write(|s| {
            if let Some(n) = s.get_mut(id) {
                n.body = Some(handle.0);
            }
        });
        true
    });

    let c = ctx.clone();
    engine.register_fn("detach_body", move |name: &str| -> bool {
        let id = {
            let scene = c.doc.borrow();
            let Some(node) = scene.find(name) else {
                return false;
            };
            node.id
        };
        let handle = c.write(|s| s.get_mut(id).and_then(|n| n.body.take()).map(BodyHandle));
        if let Some(h) = handle {
            c.physics.borrow_mut().destroy_body(h);
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
                    Some(h) => BodyHandle(h),
                    None => return false,
                }
            };
            c.physics
                .borrow_mut()
                .apply_impulse(handle, Vec3::new(f32_of(x), f32_of(y), f32_of(z)));
            true
        },
    );

    let c = ctx.clone();
    engine.register_fn("body_count", move || -> i64 {
        c.physics.borrow().body_count() as i64
    });

    // ---- 物理开关 / 后端 / 环境参数 ----

    let c = ctx.clone();
    engine.register_fn("physics_enabled", move || -> bool {
        c.physics.borrow().enabled()
    });

    let c = ctx.clone();
    engine.register_fn("physics_set_enabled", move |on: bool| -> bool {
        let mut physics = c.physics.borrow_mut();
        physics.set_enabled(on);
        let now = physics.enabled();
        println!("[script] 物理{}", if now { "已启用" } else { "已关闭" });
        now
    });

    let c = ctx.clone();
    engine.register_fn("physics_backend", move || -> String {
        c.physics.borrow().kind().name().to_string()
    });

    // 运行时切换后端；切换会清空已有刚体。`rapier` 在当前构建不可用时返回 false。
    let c = ctx.clone();
    engine.register_fn("physics_set_backend", move |name: &str| -> bool {
        let Some(kind) = PhysicsKind::parse(name) else {
            println!(
                "[script] 未知物理后端 `{name}`（当前构建可用：{}）",
                PhysicsKind::available_names().join(" / ")
            );
            return false;
        };
        match c.physics.borrow_mut().set_kind(kind) {
            Ok(()) => {
                println!("[script] 物理后端 -> {}", kind.name());
                true
            }
            Err(e) => {
                println!("[script] {e}");
                false
            }
        }
    });

    // 后端能力位：脚本据此判断能不能指望互撞 / 旋转 / 碰撞事件，别按后端名写分支。
    let c = ctx.clone();
    engine.register_fn("physics_capabilities", move || -> String {
        c.physics.borrow().capabilities().summary()
    });

    let c = ctx.clone();
    engine.register_fn("physics_has_capability", move |key: &str| -> bool {
        c.physics.borrow().capabilities().has(key)
    });

    // 射线查询：命中返回 [node, distance, px, py, pz]，未命中返回空数组。
    // 方向不必先归一化（Ray 构造时归一），max_dist 用世界单位。
    let c = ctx.clone();
    engine.register_fn(
        "physics_raycast",
        move |ox: f64, oy: f64, oz: f64, dx: f64, dy: f64, dz: f64, max_dist: f64| -> Array {
            let ray = Ray::new(
                Vec3::new(f32_of(ox), f32_of(oy), f32_of(oz)),
                Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz)),
            );
            match c.physics.borrow().raycast(ray, f32_of(max_dist)) {
                Some(hit) => vec![
                    Dynamic::from(hit.node as i64),
                    Dynamic::from(hit.distance as f64),
                    Dynamic::from(hit.point.x as f64),
                    Dynamic::from(hit.point.y as f64),
                    Dynamic::from(hit.point.z as f64),
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
            let ray = Ray::new(
                Vec3::new(f32_of(ox), f32_of(oy), f32_of(oz)),
                Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz)),
            );
            let hit = c.physics.borrow().raycast(ray, f32_of(max_dist));
            ray_hit_map(hit)
        },
    );

    // 球体重叠：返回命中节点的 id 数组（按 id 升序），空数组即没碰到。
    let c = ctx.clone();
    engine.register_fn(
        "physics_overlap",
        move |x: f64, y: f64, z: f64, radius: f64| -> Array {
            c.physics
                .borrow()
                .overlap_sphere(Vec3::new(f32_of(x), f32_of(y), f32_of(z)), f32_of(radius))
                .iter()
                .map(|h| Dynamic::from(h.node as i64))
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
            let hit = c.physics.borrow().shape_cast(
                CastShape::sphere(f32_of(radius)),
                Vec3::new(f32_of(ox), f32_of(oy), f32_of(oz)),
                Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz)),
                f32_of(max_dist),
            );
            shape_cast_map(hit)
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
            let hit = c.physics.borrow().shape_cast(
                CastShape::boxed(Vec3::new(f32_of(hx), f32_of(hy), f32_of(hz))),
                Vec3::new(f32_of(ox), f32_of(oy), f32_of(oz)),
                Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz)),
                f32_of(max_dist),
            );
            shape_cast_map(hit)
        },
    );

    // ---- CCD（连续碰撞检测）----

    // 全局开关：改的是「刚体没显式指定时」的缺省值，并把当前所有刚体一起改掉。
    // 返回 true 表示后端真的支持 CCD；不支持（如内置后端）时如实返回 false。
    let c = ctx.clone();
    engine.register_fn("physics_set_ccd", move |on: bool| -> bool {
        let mut physics = c.physics.borrow_mut();
        let supported = physics.set_ccd_default(on);
        let changed = physics.set_ccd_all(on);
        if !supported {
            println!("[script] 当前物理后端不支持 CCD（能力位 ccd=false），physics_set_ccd 无效");
            return false;
        }
        println!(
            "[script] CCD {}（已更新 {changed} 个刚体，缺省={}）",
            if on { "已开启" } else { "已关闭" },
            if physics.ccd_default() { "开" } else { "关" }
        );
        true
    });

    let c = ctx.clone();
    engine.register_fn("physics_ccd_default", move || -> bool {
        c.physics.borrow().ccd_default()
    });

    // 逐刚体开关：目标是动态刚体才有意义（静态 / 运动学由脚本驱动）。
    let c = ctx.clone();
    engine.register_fn("body_set_ccd", move |name: &str, on: bool| -> bool {
        let handle = {
            let scene = c.doc.borrow();
            match scene.find(name).and_then(|n| n.body) {
                Some(h) => BodyHandle(h),
                None => {
                    println!("[script] body_set_ccd: 物件 `{name}` 没有刚体");
                    return false;
                }
            }
        };
        let ok = c.physics.borrow_mut().set_ccd(handle, on);
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
                Some(h) => BodyHandle(h),
                None => return false,
            }
        };
        c.physics.borrow().ccd_of(handle).unwrap_or(false)
    });

    // 取走自上次调用以来的碰撞事件（形如 "started:7-none"）；不支持事件的后端返回空数组。
    let c = ctx.clone();
    engine.register_fn("physics_take_events", move || -> Array {
        let events = c.physics.borrow_mut().drain_events();
        events.iter().map(|e| Dynamic::from(e.to_text())).collect()
    });

    let c = ctx.clone();
    engine.register_fn("physics_gravity", move || -> f64 {
        c.physics.borrow().gravity.y as f64
    });

    let c = ctx.clone();
    engine.register_fn("physics_set_gravity", move |x: f64, y: f64, z: f64| {
        c.physics
            .borrow_mut()
            .set_gravity(Vec3::new(f32_of(x), f32_of(y), f32_of(z)));
    });

    let c = ctx.clone();
    engine.register_fn("physics_ground", move || -> f64 {
        c.physics.borrow().ground() as f64
    });

    let c = ctx.clone();
    engine.register_fn("physics_set_ground", move |y: f64| {
        c.physics.borrow_mut().set_ground(f32_of(y));
    });
}
