//! 脚本可调用的引擎 API 绑定。
//!
//! 全部函数都通过 `Rc<RefCell<..>>` 访问场景 / 物理 / 时钟，
//! 因此脚本执行与主循环渲染共享同一份真源数据，改动下一帧即生效。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::{Vec3, Vec4};
use r3e_core::{MeshKind, ObjectDesc, Scene};
use r3e_physics::{BodyDesc, BodyKind, ColliderShape, Physics};
use rhai::{Array, Dynamic, Engine};

/// 引擎递给脚本的运行时时钟。
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    pub dt: f64,
    pub time: f64,
    pub frame: u64,
}

/// 脚本侧共享上下文。
#[derive(Clone)]
pub struct ScriptContext {
    pub scene: Rc<RefCell<Scene>>,
    pub physics: Rc<RefCell<Physics>>,
    pub clock: Rc<RefCell<Clock>>,
    pub rng: Rc<Cell<u64>>,
}

fn f32_of(v: f64) -> f32 {
    v as f32
}

/// 对指定名字的物件做一次可变访问；找不到返回 false。
fn edit(
    scene: &Rc<RefCell<Scene>>,
    name: &str,
    f: impl FnOnce(&mut r3e_core::Node),
) -> bool {
    let mut s = scene.borrow_mut();
    match s.find_mut(name) {
        Some(node) => {
            f(node);
            true
        }
        None => false,
    }
}

fn xorshift(state: &Cell<u64>) -> f64 {
    let mut x = state.get();
    if x == 0 {
        x = 0x9E3779B97F4A7C15;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    state.set(x);
    ((x >> 11) as f64) / ((1u64 << 53) as f64)
}

/// 把引擎 API 注入 Rhai 引擎。
pub fn bind_all(engine: &mut Engine, ctx: &ScriptContext) {
    engine.set_max_operations(50_000_000);

    // ---------------- 输出 ----------------
    engine.register_fn("log", |msg: &str| {
        println!("[script] {msg}");
    });

    // ---------------- 生命周期：生成 / 消失 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "create_node",
        move |name: &str, mesh: &str, x: f64, y: f64, z: f64| -> bool {
            let desc = ObjectDesc {
                name: name.to_string(),
                mesh: Some(mesh.to_string()),
                position: Some([f32_of(x), f32_of(y), f32_of(z)]),
                ..Default::default()
            };
            match c.scene.borrow_mut().spawn(desc) {
                Ok(_) => true,
                Err(e) => {
                    println!("[script] create_node 失败: {e}");
                    false
                }
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn("create_node_at_origin", move |name: &str, mesh: &str| -> bool {
        let desc = ObjectDesc {
            name: name.to_string(),
            mesh: Some(mesh.to_string()),
            ..Default::default()
        };
        match c.scene.borrow_mut().spawn(desc) {
            Ok(_) => true,
            Err(e) => {
                println!("[script] create_node 失败: {e}");
                false
            }
        }
    });

    // 用 JSON 描述物件，字段与场景文件一致
    let c = ctx.clone();
    engine.register_fn("create_node_json", move |json: &str| -> bool {
        match serde_json_lite(json) {
            Ok(desc) => match c.scene.borrow_mut().spawn(desc) {
                Ok(_) => true,
                Err(e) => {
                    println!("[script] create_node_json 失败: {e}");
                    false
                }
            },
            Err(e) => {
                println!("[script] create_node_json 解析失败: {e}");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("remove_node", move |name: &str| -> bool {
        let ok = c.scene.borrow_mut().despawn(name);
        if !ok {
            println!("[script] remove_node: 物件 `{name}` 不存在");
        }
        ok
    });

    let c = ctx.clone();
    engine.register_fn("remove_nodes", move |prefix: &str| -> i64 {
        let names: Vec<String> = c.scene.borrow().find_all(prefix);
        let mut s = c.scene.borrow_mut();
        let mut n = 0;
        for name in names {
            if s.despawn(&name) {
                n += 1;
            }
        }
        n
    });

    let c = ctx.clone();
    engine.register_fn("exists", move |name: &str| -> bool {
        c.scene.borrow().find(name).is_some()
    });

    let c = ctx.clone();
    engine.register_fn("count", move || -> i64 { c.scene.borrow().len() as i64 });

    let c = ctx.clone();
    engine.register_fn("names", move |prefix: &str| -> Array {
        c.scene
            .borrow()
            .find_all(prefix)
            .into_iter()
            .map(|n| Dynamic::from(n))
            .collect()
    });

    // ---------------- 变换 ----------------
    let c = ctx.clone();
    engine.register_fn("set_pos", move |name: &str, x: f64, y: f64, z: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.position = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
        })
    });

    let c = ctx.clone();
    engine.register_fn("move_by", move |name: &str, dx: f64, dy: f64, dz: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.position += Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
        })
    });

    let c = ctx.clone();
    engine.register_fn("get_pos", move |name: &str| -> Array {
        let s = c.scene.borrow();
        match s.find(name) {
            Some(n) => vec![
                Dynamic::from(n.transform.position.x as f64),
                Dynamic::from(n.transform.position.y as f64),
                Dynamic::from(n.transform.position.z as f64),
            ],
            None => vec![Dynamic::from(0.0f64); 3],
        }
    });

    let c = ctx.clone();
    engine.register_fn("set_rot", move |name: &str, x: f64, y: f64, z: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.set_euler_deg(Vec3::new(f32_of(x), f32_of(y), f32_of(z)));
        })
    });

    let c = ctx.clone();
    engine.register_fn("rotate", move |name: &str, x: f64, y: f64, z: f64| -> bool {
        edit(&c.scene, name, |n| {
            let e = n.transform.euler_deg() + Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            n.transform.set_euler_deg(e);
        })
    });

    let c = ctx.clone();
    engine.register_fn("set_scale", move |name: &str, x: f64, y: f64, z: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.scale = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
        })
    });

    let c = ctx.clone();
    engine.register_fn("set_scale_uniform", move |name: &str, s: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.scale = Vec3::splat(f32_of(s));
        })
    });

    // ---------------- 外观 ----------------
    let c = ctx.clone();
    engine.register_fn("set_color", move |name: &str, r: f64, g: f64, b: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.color = Vec4::new(f32_of(r), f32_of(g), f32_of(b), n.color.w);
        })
    });

    let c = ctx.clone();
    engine.register_fn(
        "set_color4",
        move |name: &str, r: f64, g: f64, b: f64, a: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.color = Vec4::new(f32_of(r), f32_of(g), f32_of(b), f32_of(a));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("set_visible", move |name: &str, v: bool| -> bool {
        edit(&c.scene, name, |n| n.visible = v)
    });

    let c = ctx.clone();
    engine.register_fn("set_mesh", move |name: &str, mesh: &str| -> bool {
        match MeshKind::parse(mesh) {
            Some(kind) => edit(&c.scene, name, |n| n.mesh = kind),
            None => {
                println!("[script] 未知网格类型 `{mesh}`");
                false
            }
        }
    });

    // ---------------- 相机 / 光照 / 环境 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "camera",
        move |px: f64, py: f64, pz: f64, tx: f64, ty: f64, tz: f64| {
            let mut s = c.scene.borrow_mut();
            s.camera.position = Vec3::new(f32_of(px), f32_of(py), f32_of(pz));
            s.camera.target = Vec3::new(f32_of(tx), f32_of(ty), f32_of(tz));
        },
    );

    let c = ctx.clone();
    engine.register_fn("orbit", move |yaw: f64, pitch: f64| {
        c.scene.borrow_mut().camera.orbit(f32_of(yaw), f32_of(pitch));
    });

    let c = ctx.clone();
    engine.register_fn("fov", move |deg: f64| {
        c.scene.borrow_mut().camera.fov_deg = f32_of(deg);
    });

    let c = ctx.clone();
    engine.register_fn("set_light", move |dx: f64, dy: f64, dz: f64| {
        let mut s = c.scene.borrow_mut();
        let d = Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
        if d.length_squared() > 1e-8 {
            s.light.direction = d.normalize();
        }
    });

    let c = ctx.clone();
    engine.register_fn("set_light_color", move |r: f64, g: f64, b: f64, i: f64| {
        let mut s = c.scene.borrow_mut();
        s.light.color = Vec3::new(f32_of(r), f32_of(g), f32_of(b));
        s.light.intensity = f32_of(i);
    });

    let c = ctx.clone();
    engine.register_fn("set_ambient", move |r: f64, g: f64, b: f64| {
        c.scene.borrow_mut().light.ambient = Vec3::new(f32_of(r), f32_of(g), f32_of(b));
    });

    let c = ctx.clone();
    engine.register_fn("set_background", move |r: f64, g: f64, b: f64| {
        c.scene.borrow_mut().background = Vec4::new(f32_of(r), f32_of(g), f32_of(b), 1.0);
    });

    // ---------------- 时钟与随机 ----------------
    let c = ctx.clone();
    engine.register_fn("time", move || -> f64 { c.clock.borrow().time });

    let c = ctx.clone();
    engine.register_fn("frame", move || -> i64 { c.clock.borrow().frame as i64 });

    let c = ctx.clone();
    engine.register_fn("dt", move || -> f64 { c.clock.borrow().dt });

    let c = ctx.clone();
    engine.register_fn("rand", move |min: f64, max: f64| -> f64 {
        min + xorshift(&c.rng) * (max - min)
    });

    // ---------------- 物理（预留接口） ----------------
    // 当前 `null` 后端只登记刚体、不参与模拟；换上真实后端后这些调用立刻生效。
    let c = ctx.clone();
    engine.register_fn(
        "attach_body",
        move |name: &str, kind: &str, mass: f64| -> bool {
            let scene = c.scene.borrow();
            let Some(node) = scene.find(name) else {
                println!("[script] attach_body: 物件 `{name}` 不存在");
                return false;
            };
            let body_kind = BodyKind::parse(kind);
            let shape = match node.mesh {
                MeshKind::Sphere => ColliderShape::Sphere,
                MeshKind::Cylinder => ColliderShape::Cylinder,
                _ => ColliderShape::Box,
            };
            let desc = BodyDesc {
                kind: body_kind,
                shape,
                half_extents: node.transform.scale * 0.5,
                mass: f32_of(mass),
                position: node.world_position(),
                rotation: node.transform.rotation,
                scale: node.transform.scale,
                ..Default::default()
            };
            let id = node.id;
            drop(scene);
            let handle = c.physics.borrow_mut().create_body(id, desc);
            let mut scene = c.scene.borrow_mut();
            if let Some(n) = scene.get_mut(id) {
                n.body = Some(handle.0);
            }
            true
        },
    );

    let c = ctx.clone();
    engine.register_fn("body_count", move || -> i64 {
        c.physics.borrow().body_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("physics_backend", move || -> String {
        c.physics.borrow().backend_name().to_string()
    });
}

/// 轻量 JSON → ObjectDesc：避免为了脚本再引一个 serde 依赖。
fn serde_json_lite(json: &str) -> Result<ObjectDesc, String> {
    // 借助 r3e-core 的 serde 实现：包装成单物件场景再取出
    let wrapped = format!("{{\"objects\":[{json}]}}");
    let scene = Scene::from_json_str(&wrapped)?;
    let first = scene.iter().next().ok_or("空物件")?;
    let mut desc = ObjectDesc {
        name: first.name.clone(),
        mesh: Some(first.mesh.as_str().to_string()),
        position: Some(first.transform.position.to_array()),
        scale: Some(first.transform.scale.to_array()),
        color: Some(first.color.to_array()),
        visible: Some(first.visible),
        ..Default::default()
    };
    if let Some(rot) = Some(first.transform.euler_deg()) {
        desc.rotation = Some(rot.to_array());
    }
    Ok(desc)
}
