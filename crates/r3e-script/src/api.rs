//! 脚本可调用的引擎 API 绑定。
//!
//! 全部函数都通过 `Rc<RefCell<..>>` 访问场景 / 物理 / 时钟，
//! 因此脚本执行与主循环渲染共享同一份真源数据，改动下一帧即生效。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::{Vec3, Vec4};
use r3e_core::input::{parse_mouse_button, parse_pad_axis, parse_pad_button};
use r3e_core::{InputState, MeshRef, NodeId, ObjectDesc, Scene};
use r3e_physics::{
    BodyDesc, BodyHandle, BodyKind, CastShape, Physics, PhysicsKind, Ray, RayHit, ShapeCastHit,
};
use rhai::{Array, Dynamic, Engine, Map};

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
    pub input: Rc<RefCell<InputState>>,
    pub clock: Rc<RefCell<Clock>>,
    pub rng: Rc<Cell<u64>>,
}

fn f32_of(v: f64) -> f32 {
    v as f32
}

fn vec3_array(v: Vec3) -> Array {
    vec![
        Dynamic::from(v.x as f64),
        Dynamic::from(v.y as f64),
        Dynamic::from(v.z as f64),
    ]
}

/// 查询结果的空模板：字段名必须与命中版本**完全一致**。
/// 这样脚本里 `if r.hit { r.node }` 不会因为「没命中就缺键」而报错，IDE 也能一次补全。
fn empty_query_map() -> Map {
    let mut m = Map::new();
    m.insert("hit".into(), Dynamic::from(false));
    m.insert("node".into(), Dynamic::from(-1_i64));
    m.insert("handle".into(), Dynamic::from(-1_i64));
    m.insert("distance".into(), Dynamic::from(-1.0_f64));
    m.insert("point".into(), Dynamic::from(Array::new()));
    m.insert("normal".into(), Dynamic::from(Array::new()));
    m.insert("inside".into(), Dynamic::from(false));
    m
}

/// 命中结果 → 脚本 Map（raycast / shape-cast 共用同一套字段名）。
fn hit_map(hit: Option<(NodeId, BodyHandle, f32, Vec3, Vec3, bool)>) -> Map {
    let Some((node, handle, distance, point, normal, inside)) = hit else {
        return empty_query_map();
    };
    let mut m = Map::new();
    m.insert("hit".into(), Dynamic::from(true));
    m.insert("node".into(), Dynamic::from(node as i64));
    m.insert("handle".into(), Dynamic::from(handle.0 as i64));
    m.insert("distance".into(), Dynamic::from(distance as f64));
    m.insert("point".into(), Dynamic::from(vec3_array(point)));
    m.insert("normal".into(), Dynamic::from(vec3_array(normal)));
    m.insert("inside".into(), Dynamic::from(inside));
    m
}

fn ray_hit_map(hit: Option<RayHit>) -> Map {
    hit_map(hit.map(|h| (h.node, h.handle, h.distance, h.point, h.normal, false)))
}

fn shape_cast_map(hit: Option<ShapeCastHit>) -> Map {
    hit_map(hit.map(|h| {
        (
            h.node,
            h.handle,
            h.distance,
            h.point,
            h.normal,
            h.started_inside,
        )
    }))
}

/// 对指定名字的物件做一次可变访问；找不到返回 false。
fn edit(scene: &Rc<RefCell<Scene>>, name: &str, f: impl FnOnce(&mut r3e_core::Node)) -> bool {
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
    engine.register_fn(
        "create_node_at_origin",
        move |name: &str, mesh: &str| -> bool {
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
        },
    );

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
            .map(Dynamic::from)
            .collect()
    });

    // ---------------- 变换 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "set_pos",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.position = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "move_by",
        move |name: &str, dx: f64, dy: f64, dz: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.position += Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
            })
        },
    );

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
    engine.register_fn(
        "set_rot",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform
                    .set_euler_deg(Vec3::new(f32_of(x), f32_of(y), f32_of(z)));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "rotate",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                let e = n.transform.euler_deg() + Vec3::new(f32_of(x), f32_of(y), f32_of(z));
                n.transform.set_euler_deg(e);
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_scale",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.transform.scale = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("set_scale_uniform", move |name: &str, s: f64| -> bool {
        edit(&c.scene, name, |n| {
            n.transform.scale = Vec3::splat(f32_of(s));
        })
    });

    // ---------------- 外观 ----------------
    let c = ctx.clone();
    engine.register_fn(
        "set_color",
        move |name: &str, r: f64, g: f64, b: f64| -> bool {
            edit(&c.scene, name, |n| {
                n.color = Vec4::new(f32_of(r), f32_of(g), f32_of(b), n.color.w);
            })
        },
    );

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
        match MeshRef::parse(mesh) {
            Some(kind) => edit(&c.scene, name, |n| n.mesh = kind),
            None => {
                println!("[script] 未知网格类型 `{mesh}`");
                false
            }
        }
    });

    // ---------------- 动画（P3） ----------------
    // 一个「动画实例」= 一个资产实例 + 一个片段（导入时建好，名字为 `<资产名>/<片段名>`）。
    // 脚本侧只操作实例，不直接碰轨道；`anim_of(node)` 用于从骨骼节点反查它属于哪个实例。
    let c = ctx.clone();
    engine.register_fn("anims", move || -> Array {
        c.scene
            .borrow()
            .anim_names()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("anim_count", move || -> i64 {
        c.scene.borrow().animations.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("anim_of", move |node: &str| -> String {
        let s = c.scene.borrow();
        let Some(id) = s.find_id(node) else {
            return String::new();
        };
        for inst in &s.animations {
            if inst.bindings.iter().any(|b| b.node == id) {
                return inst.name.clone();
            }
        }
        String::new()
    });

    let c = ctx.clone();
    engine.register_fn("play_anim", move |name: &str| -> bool {
        c.scene.borrow_mut().play_anim(name)
    });

    let c = ctx.clone();
    engine.register_fn("stop_anim", move |name: &str| -> bool {
        c.scene.borrow_mut().stop_anim(name)
    });

    let c = ctx.clone();
    engine.register_fn("set_anim_speed", move |name: &str, speed: f64| -> bool {
        c.scene.borrow_mut().set_anim_speed(name, f32_of(speed))
    });

    let c = ctx.clone();
    engine.register_fn("set_anim_time", move |name: &str, t: f64| -> bool {
        c.scene.borrow_mut().set_anim_time(name, f32_of(t))
    });

    let c = ctx.clone();
    engine.register_fn("anim_time", move |name: &str| -> f64 {
        c.scene
            .borrow()
            .anim_time(name)
            .map(|t| t as f64)
            .unwrap_or(0.0)
    });

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_looping",
        move |name: &str, looping: bool| -> bool {
            match c.scene.borrow_mut().anim_instance_mut(name) {
                Some(inst) => {
                    inst.looping = looping;
                    true
                }
                None => false,
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn("anim_playing", move |name: &str| -> bool {
        c.scene
            .borrow()
            .anim_instance(name)
            .map(|a| a.playing)
            .unwrap_or(false)
    });

    let c = ctx.clone();
    engine.register_fn("anim_info", move |name: &str| -> String {
        let s = c.scene.borrow();
        match s.anim_instance(name) {
            Some(a) => format!(
                "{} / 片段 {} / {} 轨道 / 时长 {:.2}s / 当前 {:.2}s / 速度 {} / {} / {}",
                a.name,
                a.clip.name,
                a.track_count(),
                a.clip.duration,
                a.sample_time(),
                a.speed,
                if a.looping { "循环" } else { "单次" },
                if a.playing { "播放中" } else { "已暂停" }
            ),
            None => format!("没有名为 `{name}` 的动画实例"),
        }
    });

    let c = ctx.clone();
    engine.register_fn("anim_clip", move |name: &str| -> String {
        c.scene
            .borrow()
            .anim_instance(name)
            .map(|a| a.clip.name.clone())
            .unwrap_or_default()
    });

    // 切片段：同一骨架的不同动作共用一个实例，避免重复 spawn 节点树。
    let c = ctx.clone();
    engine.register_fn("set_anim_clip", move |inst: &str, clip: &str| -> bool {
        c.scene.borrow_mut().set_anim_clip(inst, clip)
    });

    // 带交叉淡入地切片段：`dur` 秒内旧动作逐渐淡出，避免 idle→run 这类切换硬跳。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_clip_fade",
        move |inst: &str, clip: &str, dur: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_clip_fade(inst, clip, dur as f32)
        },
    );

    // 过渡进度（新动作权重，1.0 = 已完成/无过渡），供脚本做状态机与日志验收。
    let c = ctx.clone();
    engine.register_fn("anim_fade", move |inst: &str| -> f64 {
        c.scene
            .borrow()
            .anim_fade_weight(inst)
            .map(|w| w as f64)
            .unwrap_or(1.0)
    });

    // 正在淡出的旧动作名（无过渡为空串）。
    let c = ctx.clone();
    engine.register_fn("anim_fade_from", move |inst: &str| -> String {
        c.scene.borrow().anim_fade_source(inst)
    });

    let c = ctx.clone();
    engine.register_fn("anim_clips", move |prefix: &str| -> Array {
        c.scene
            .borrow()
            .anim_clip_names(prefix)
            .into_iter()
            .map(Into::into)
            .collect()
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
        c.scene
            .borrow_mut()
            .camera
            .orbit(f32_of(yaw), f32_of(pitch));
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

    // ---------------- 阶段 D：方向光阴影 ----------------
    // 开关：set_shadow(true/false)。关闭后渲染器整帧跳过阴影通道，零开销。
    let c = ctx.clone();
    engine.register_fn("set_shadow", move |on: bool| {
        c.scene.borrow_mut().light.shadow.enabled = on;
    });

    // 只读查询：当前阴影是否开启（脚本里做开关叙事 / 打印状态用）。
    let c = ctx.clone();
    engine.register_fn("shadow_enabled", move || -> bool {
        c.scene.borrow().light.shadow.enabled
    });

    // 阴影强度 0..1：1 = 完全遮挡，0 = 无阴影（保留光照）。用于淡入/调试。
    let c = ctx.clone();
    engine.register_fn("set_shadow_strength", move |strength: f64| {
        c.scene.borrow_mut().light.shadow.strength = f32_of(strength);
    });

    // 阴影贴图分辨率（正方形），运行时改会触发贴图重建；非 2 的幂会被夹到 64..8192。
    let c = ctx.clone();
    engine.register_fn("set_shadow_size", move |size: i64| {
        c.scene.borrow_mut().light.shadow.size = size.clamp(64, 8192) as u32;
    });

    // 正交相机半宽：0 = 按场景包围球自适应（默认），>0 = 固定覆盖范围（更清晰但更易裁切）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_extent", move |half: f64| {
        c.scene.borrow_mut().light.shadow.ortho_size = f32_of(half).max(0.0);
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

    // ---------------- 输入：鼠标 ----------------
    // 读数均为"本逻辑帧"的快照：pressed / released 只在该帧为 true。
    let c = ctx.clone();
    engine.register_fn("mouse_x", move || -> f64 {
        c.input.borrow().mouse_x as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_y", move || -> f64 {
        c.input.borrow().mouse_y as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pos", move || -> Array {
        let i = c.input.borrow();
        vec![
            Dynamic::from(i.mouse_x as f64),
            Dynamic::from(i.mouse_y as f64),
        ]
    });

    let c = ctx.clone();
    engine.register_fn("mouse_dx", move || -> f64 {
        c.input.borrow().mouse_dx as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_dy", move || -> f64 {
        c.input.borrow().mouse_dy as f64
    });

    let c = ctx.clone();
    engine.register_fn("mouse_moved", move || -> bool {
        let i = c.input.borrow();
        i.mouse_dx != 0.0 || i.mouse_dy != 0.0
    });

    let c = ctx.clone();
    engine.register_fn("scroll", move || -> f64 { c.input.borrow().scroll as f64 });

    let c = ctx.clone();
    engine.register_fn("mouse_down", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_down(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`（可选 left/right/middle/back/forward）");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_down", move |idx: i64| -> bool {
        c.input.borrow().mouse_down(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pressed", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_pressed(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_pressed", move |idx: i64| -> bool {
        c.input.borrow().mouse_pressed(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("mouse_released", move |btn: &str| -> bool {
        match parse_mouse_button(btn) {
            Some(i) => c.input.borrow().mouse_released(i),
            None => {
                println!("[script] 未知鼠标按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("mouse_released", move |idx: i64| -> bool {
        c.input.borrow().mouse_released(idx.max(0) as usize)
    });

    // ---------------- 输入：手柄 ----------------
    let c = ctx.clone();
    engine.register_fn("gamepad_connected", move || -> bool {
        c.input.borrow().pad.connected
    });

    let c = ctx.clone();
    engine.register_fn("gamepad_name", move || -> String {
        c.input.borrow().pad.name.clone()
    });

    let c = ctx.clone();
    engine.register_fn("pad_down", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_down(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_down", move |idx: i64| -> bool {
        c.input.borrow().pad_down(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_pressed", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_pressed(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_pressed", move |idx: i64| -> bool {
        c.input.borrow().pad_pressed(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_released", move |btn: &str| -> bool {
        match parse_pad_button(btn) {
            Some(i) => c.input.borrow().pad_released(i),
            None => {
                println!("[script] 未知手柄按键 `{btn}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_released", move |idx: i64| -> bool {
        c.input.borrow().pad_released(idx.max(0) as usize)
    });

    let c = ctx.clone();
    engine.register_fn("pad_axis", move |axis: &str| -> f64 {
        match parse_pad_axis(axis) {
            // 摇杆做死区处理，扳机（lt/rt 为 6、7 号之后的索引）保留原始值
            Some(i) => {
                let pad = &c.input.borrow().pad;
                let v = if i <= r3e_core::input::PAD_RIGHT_Y {
                    pad.axis_deadzone(i)
                } else {
                    pad.axis(i)
                };
                v as f64
            }
            None => {
                println!("[script] 未知手柄轴 `{axis}`");
                0.0
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("pad_axis", move |idx: i64| -> f64 {
        c.input.borrow().pad.axis(idx.max(0) as usize) as f64
    });

    // ---------------- 物理 ----------------
    // 物理默认关闭：null 后端只登记刚体。通过 `physics_set_enabled(true)` 或
    // 场景 JSON / 命令行开启后，内置后端开始积分与地面碰撞。
    let c = ctx.clone();
    engine.register_fn("attach_body", move |name: &str, kind: &str, mass: f64| -> bool {
        // 脚本里常见「create_node → attach_body」连写，此时节点的世界矩阵还没刷新，
        // 直接读 world_position() 会拿到原点，刚体就会落在错误位置（查询也会跟着偏）。
        c.scene.borrow_mut().update_world_transforms();
        let scene = c.scene.borrow();
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
        if let Some(n) = c.scene.borrow_mut().get_mut(id) {
            n.body = Some(handle.0);
        }
        true
    });

    let c = ctx.clone();
    engine.register_fn("detach_body", move |name: &str| -> bool {
        let id = {
            let scene = c.scene.borrow();
            let Some(node) = scene.find(name) else {
                return false;
            };
            node.id
        };
        let handle = c
            .scene
            .borrow_mut()
            .get_mut(id)
            .and_then(|n| n.body.take())
            .map(BodyHandle);
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
                let scene = c.scene.borrow();
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
            let scene = c.scene.borrow();
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
            let scene = c.scene.borrow();
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

/// 轻量 JSON → ObjectDesc：避免为了脚本再引一个 serde 依赖。
fn serde_json_lite(json: &str) -> Result<ObjectDesc, String> {
    // 借助 r3e-core 的 serde 实现：包装成单物件场景再取出
    let wrapped = format!("{{\"objects\":[{json}]}}");
    let scene = Scene::from_json_str(&wrapped)?;
    let first = scene.iter().next().ok_or("空物件")?;
    let mut desc = ObjectDesc {
        name: first.name.clone(),
        mesh: Some(first.mesh.as_string()),
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
