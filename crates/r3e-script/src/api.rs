//! 脚本可调用的引擎 API 绑定。
//!
//! 全部函数都通过 `Rc<RefCell<..>>` 访问场景 / 物理 / 时钟，
//! 因此脚本执行与主循环渲染共享同一份真源数据，改动下一帧即生效。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::{Vec3, Vec4};
use r3e_audio::Audio;
use r3e_core::input::{parse_key, parse_mouse_button, parse_pad_axis, parse_pad_button};
use r3e_core::postfx::TonemapMode;
use r3e_core::text::{estimate_text_height, estimate_text_width, sanitize_size};
use r3e_core::ui::{
    find_mut, upsert_styled, Anchor, UiElement, UiKind, UiUpsert, UiValue, MAX_UI_ELEMENTS,
};
use r3e_core::{InputState, KeyCode, MeshRef, NodeId, ObjectDesc, Scene, ScreenText, TextAlign};
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
    /// 上一帧的瞬时帧率（`fps()` 绑定）；首帧为 0。
    pub fps: f64,
}

/// 脚本侧共享上下文。
#[derive(Clone)]
pub struct ScriptContext {
    pub scene: Rc<RefCell<Scene>>,
    pub physics: Rc<RefCell<Physics>>,
    pub input: Rc<RefCell<InputState>>,
    pub audio: Rc<RefCell<Audio>>,
    pub clock: Rc<RefCell<Clock>>,
    pub rng: Rc<Cell<u64>>,
    /// UI 写入被拒 / 被纠正的记录（`ui_last_error` / `ui_error_count` 的来源）。
    pub ui_log: Rc<RefCell<UiWriteLog>>,
}

/// UI 写入的「不能静默失败」账本：只记最后一次原因 + 累计次数。
///
/// 与音频的 `audio_last_error` / `audio_error_count` 同构。脚本每帧调用也不会刷屏，
/// 因为它只存状态、不打印。
#[derive(Debug, Default)]
pub struct UiWriteLog {
    last: Option<String>,
    count: u64,
}

impl UiWriteLog {
    fn record(&mut self, msg: impl Into<String>) {
        self.last = Some(msg.into());
        self.count += 1;
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last.as_deref()
    }

    pub fn error_count(&self) -> u64 {
        self.count
    }
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

    // ---------------- PBR 材质 ----------------
    // 三个旋钮：metallic / roughness / emissive。base color 仍是物件颜色，
    // 语义不变。所有写入都走 `Scene::set_material*`，由 `SurfaceMaterial::sanitized`
    // 在**写入前**收敛（NaN / ±inf / 超界），脚本侧不做第二套夹取。
    let c = ctx.clone();
    engine.register_fn(
        "set_material",
        move |name: &str, metallic: f64, roughness: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_material_params(name, f32_of(metallic), f32_of(roughness))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_emissive",
        move |name: &str, r: f64, g: f64, b: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_emissive(name, [f32_of(r), f32_of(g), f32_of(b)])
        },
    );

    let c = ctx.clone();
    engine.register_fn("clear_material", move |name: &str| -> bool {
        c.scene.borrow_mut().clear_material(name)
    });

    // 读回：收敛之后真正生效的材质快照；物件没有显式材质时给缺省值。
    let c = ctx.clone();
    engine.register_fn("material_of", move |name: &str| -> Map {
        let mut out = Map::new();
        let Some(material) = c.scene.borrow().material_of(name) else {
            out.insert("found".into(), Dynamic::from(false));
            return out;
        };
        out.insert("found".into(), Dynamic::from(true));
        out.insert("metallic".into(), Dynamic::from(material.metallic as f64));
        out.insert("roughness".into(), Dynamic::from(material.roughness as f64));
        out.insert(
            "emissive".into(),
            Dynamic::from(
                material
                    .emissive
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        out
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

    // ---------------- 环境探针（IBL，扩展十二） ----------------
    // 语义：`scene.environment` 为 `None` 时走 `set_ambient` 的常数项（老场景行为不变）；
    // 一旦调用 set_sky / set_env_sun / set_env_intensity，就切到环境探针：
    // 漫反射由 9 项球谐给出方向感，金属靠环境高光不再死黑。
    // 所有写入都经 `sanitized()`，脚本传 NaN / 天文数字不会污染渲染。

    let c = ctx.clone();
    engine.register_fn(
        "set_sky",
        move |zr: f64, zg: f64, zb: f64, hr: f64, hg: f64, hb: f64, gr: f64, gg: f64, gb: f64| {
            let mut s = c.scene.borrow_mut();
            let mut env = s.environment.unwrap_or_default();
            env.sky_zenith = [f32_of(zr), f32_of(zg), f32_of(zb)];
            env.sky_horizon = [f32_of(hr), f32_of(hg), f32_of(hb)];
            env.ground = [f32_of(gr), f32_of(gg), f32_of(gb)];
            s.environment = Some(env.sanitized());
        },
    );

    // 太阳项：颜色 + 强度 + 指向太阳的方向。方向为零向量时保留原值（与 set_light 同规矩）。
    let c = ctx.clone();
    engine.register_fn(
        "set_env_sun",
        move |r: f64, g: f64, b: f64, intensity: f64, dx: f64, dy: f64, dz: f64| {
            let mut s = c.scene.borrow_mut();
            let mut env = s.environment.unwrap_or_default();
            env.sun_color = [f32_of(r), f32_of(g), f32_of(b)];
            env.sun_intensity = f32_of(intensity);
            let d = Vec3::new(f32_of(dx), f32_of(dy), f32_of(dz));
            if d.length_squared() > 1e-8 {
                let n = d.normalize();
                env.sun_dir = [n.x, n.y, n.z];
            }
            s.environment = Some(env.sanitized());
        },
    );

    // 环境总强度：整体乘在天空 / 地面 / 太阳项上（1.0 = 物理量原值）。
    let c = ctx.clone();
    engine.register_fn("set_env_intensity", move |v: f64| {
        let mut s = c.scene.borrow_mut();
        let mut env = s.environment.unwrap_or_default();
        env.intensity = f32_of(v);
        s.environment = Some(env.sanitized());
    });

    // 关掉环境探针，退回 `light.ambient` 常数项。
    let c = ctx.clone();
    engine.register_fn("clear_environment", move || {
        c.scene.borrow_mut().environment = None;
    });

    let c = ctx.clone();
    engine.register_fn("has_environment", move || -> bool {
        c.scene.borrow().environment.is_some()
    });

    // ---------------- HDR 后处理链（扩展十三） ----------------
    // 语义：`scene.postfx` 为 `None` 时整条链退化成「离屏 HDR 目标 + composite 直通」，
    // 画面与不开启时一致；任何 set_postfx* 调用都会把这一段实体化。
    // 与 set_sky 一族同规矩：每次写入都过 `sanitized()`，
    // 脚本里算出来的 NaN / 超界值不会一路灌进 uniform。

    // 一击式全量设置。`tonemap` 取 "none" / "reinhard" / "aces"，未知名字按 none
    // 处理并打印一行说明——比静默忽略更容易在编辑器里定位脚本写错。
    let c = ctx.clone();
    engine.register_fn(
        "set_postfx",
        move |exposure: f64, tonemap: &str, bloom: bool, intensity: f64, fxaa: bool| {
            let mode = match TonemapMode::from_name(tonemap) {
                Some(m) => m,
                None => {
                    println!("[script] set_postfx: 未知色调映射 `{tonemap}`，按 none 处理");
                    TonemapMode::None
                }
            };
            let mut s = c.scene.borrow_mut();
            let mut fx = s.postfx.unwrap_or_default();
            fx.exposure = f32_of(exposure);
            fx.tonemap = mode;
            fx.bloom.enabled = bloom;
            fx.bloom.intensity = f32_of(intensity);
            fx.fxaa = fxaa;
            s.postfx = Some(fx.sanitized());
        },
    );

    // 曝光（EV）。正数提亮、负数压暗，范围由 core 夹在 [-8, 16]。
    let c = ctx.clone();
    engine.register_fn("set_exposure", move |ev: f64| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.exposure = f32_of(ev);
        s.postfx = Some(fx.sanitized());
    });

    // 换色调映射曲线。
    let c = ctx.clone();
    engine.register_fn("set_tonemap", move |name: &str| {
        let mode = match TonemapMode::from_name(name) {
            Some(m) => m,
            None => {
                println!("[script] set_tonemap: 未知色调映射 `{name}`，按 none 处理");
                TonemapMode::None
            }
        };
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.tonemap = mode;
        s.postfx = Some(fx.sanitized());
    });

    // 泛光：开关 + 阈值 + 软膝 + 强度。阈值小于 1.0 会让整片天空都发光，
    // 想要「只有高光溢出」就保持 ≥ 1.0。
    let c = ctx.clone();
    engine.register_fn(
        "set_bloom",
        move |enabled: bool, threshold: f64, knee: f64, intensity: f64| {
            let mut s = c.scene.borrow_mut();
            let mut fx = s.postfx.unwrap_or_default();
            fx.bloom.enabled = enabled;
            fx.bloom.threshold = f32_of(threshold);
            fx.bloom.knee = f32_of(knee);
            fx.bloom.intensity = f32_of(intensity);
            s.postfx = Some(fx.sanitized());
        },
    );

    // FXAA：在 composite 里做一次边缘内插，弥补没有 MSAA 的锯齿。
    let c = ctx.clone();
    engine.register_fn("set_fxaa", move |enabled: bool| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.fxaa = enabled;
        s.postfx = Some(fx.sanitized());
    });

    // 关掉后处理段，退回直通。
    let c = ctx.clone();
    engine.register_fn("clear_postfx", move || {
        c.scene.borrow_mut().postfx = None;
    });

    let c = ctx.clone();
    engine.register_fn("has_postfx", move || -> bool {
        c.scene.borrow().postfx.is_some()
    });

    // 当前生效值快照（脚本侧断言 / 编辑器回读用），未开启时给出缺省值。
    let c = ctx.clone();
    engine.register_fn("postfx_exposure", move || -> f64 {
        f64::from(c.scene.borrow().postfx.unwrap_or_default().exposure)
    });

    let c = ctx.clone();
    engine.register_fn("postfx_tonemap", move || -> String {
        c.scene
            .borrow()
            .postfx
            .unwrap_or_default()
            .tonemap
            .name()
            .to_string()
    });

    // ---------------- 场景保存（M1） ----------------
    // 只读导出：当前场景 → JSON 文本（相对路径无关，可赋值给变量做 diff / 打印）。
    // 失败返回空串并打印原因——与 create_node 一类写接口的返回风格保持一致。
    let c = ctx.clone();
    engine.register_fn("scene_json", move || -> String {
        match c.scene.borrow().to_json_string() {
            Ok(text) => text,
            Err(e) => {
                println!("[script] scene_json 失败: {e}");
                String::new()
            }
        }
    });

    // 落盘：把内存里的当前场景写回 path（相对路径按进程工作目录解析）。
    // 成功返回 true；失败返回 false 并打印原因。
    let c = ctx.clone();
    engine.register_fn("save_scene", move |path: &str| -> bool {
        match c.scene.borrow().save_to_path(path) {
            Ok(()) => true,
            Err(e) => {
                println!("[script] save_scene 失败: {e}");
                false
            }
        }
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

    // 瞬时帧率（1/dt）。HUD 文本常用，省得脚本自己算倒数还得防 0
    let c = ctx.clone();
    engine.register_fn("fps", move || -> f64 { c.clock.borrow().fps });

    let c = ctx.clone();
    engine.register_fn("rand", move |min: f64, max: f64| -> f64 {
        min + xorshift(&c.rng) * (max - min)
    });

    // ---------------- 输入：键盘 ----------------
    // 名字与鼠标 / 手柄保持同一套语义：down = 持续按住，pressed / released = 本帧边沿。
    let c = ctx.clone();
    engine.register_fn("key_down", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.down(k),
            None => {
                println!("[script] 未知按键 `{key}`（例：w / space / shift / f1 / up）");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_down", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.down(k),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_pressed", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.pressed(k),
            None => {
                println!("[script] 未知按键 `{key}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_pressed", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.pressed(k),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_released", move |key: &str| -> bool {
        match parse_key(key) {
            Some(k) => c.input.borrow().keyboard.released(k),
            None => {
                println!("[script] 未知按键 `{key}`");
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn("key_released", move |idx: i64| -> bool {
        match KeyCode::from_index(idx.max(0) as usize) {
            Some(k) => c.input.borrow().keyboard.released(k),
            None => false,
        }
    });

    // 任意键：脚本做「按任意键开始」这类交互时不必逐个枚举。
    let c = ctx.clone();
    engine.register_fn("key_any", move || -> bool {
        c.input.borrow().keyboard.any_down()
    });

    let c = ctx.clone();
    engine.register_fn("key_any_pressed", move || -> bool {
        c.input.borrow().keyboard.any_pressed()
    });

    // 当前按住的键名列表（HUD / 调试面板）。
    let c = ctx.clone();
    engine.register_fn("keys_held", move || -> Array {
        c.input
            .borrow()
            .keyboard
            .held()
            .into_iter()
            .map(|k| Dynamic::from(k.name().to_string()))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("key_held_count", move || -> i64 {
        c.input.borrow().keyboard.held_count() as i64
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

    // ---------------- 2D 屏幕空间文本（M1 文字渲染）----------------
    // 场景里的 `texts` 是唯一真源：这些绑定只改数据，渲染层下一帧自动重排重画。
    // 都用 `key` 定位，同名即覆盖 —— 脚本每帧调用也不会重复堆积。
    let c = ctx.clone();
    engine.register_fn(
        "text_add",
        move |key: &str,
              content: &str,
              x: f64,
              y: f64,
              size: f64,
              r: f64,
              g: f64,
              b: f64|
              -> bool {
            let text = ScreenText {
                key: key.to_string(),
                content: content.to_string(),
                x: f32_of(x),
                y: f32_of(y),
                size: f32_of(size),
                color: [f32_of(r), f32_of(g), f32_of(b), 1.0],
                ..Default::default()
            }
            .sanitized();
            let mut scene = c.scene.borrow_mut();
            match scene.texts.iter_mut().find(|t| t.key == key) {
                Some(slot) => {
                    // 颜色沿用新给的，其余样式（对齐 / 换行）保持不变
                    let align = slot.align;
                    let wrap = slot.wrap;
                    let line_height = slot.line_height;
                    *slot = text;
                    slot.align = align;
                    slot.wrap = wrap;
                    slot.line_height = line_height;
                    false
                }
                None => {
                    scene.texts.push(text);
                    true
                }
            }
        },
    );

    // 改内容：最常用的一条，单独给最简签名。
    let c = ctx.clone();
    engine.register_fn("text_set", move |key: &str, content: &str| -> bool {
        with_text(&c, key, |t| {
            if t.content != content {
                t.content = content.to_string();
            }
        })
    });

    let c = ctx.clone();
    engine.register_fn("text_set_pos", move |key: &str, x: f64, y: f64| -> bool {
        with_text(&c, key, |t| {
            t.x = f32_of(x);
            t.y = f32_of(y);
        })
    });

    let c = ctx.clone();
    engine.register_fn("text_set_size", move |key: &str, size: f64| -> bool {
        with_text(&c, key, |t| t.size = sanitize_size(f32_of(size)))
    });

    let c = ctx.clone();
    engine.register_fn(
        "text_set_color",
        move |key: &str, r: f64, g: f64, b: f64| -> bool {
            with_text(&c, key, |t| {
                t.color = [
                    f32_of(r).clamp(0.0, 1.0),
                    f32_of(g).clamp(0.0, 1.0),
                    f32_of(b).clamp(0.0, 1.0),
                    t.color[3],
                ];
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("text_set_alpha", move |key: &str, a: f64| -> bool {
        with_text(&c, key, |t| t.color[3] = f32_of(a).clamp(0.0, 1.0))
    });

    // 对齐：left / center / right（也接受 l / c / r 等别名）
    let c = ctx.clone();
    engine.register_fn("text_set_align", move |key: &str, align: &str| -> bool {
        match TextAlign::parse(align) {
            Some(a) => with_text(&c, key, |t| t.align = a),
            None => {
                println!("[script] 未知对齐方式 `{align}`（可选 left / center / right）");
                false
            }
        }
    });

    // 自动换行宽度（像素）；0 = 只按 \n 硬换行
    let c = ctx.clone();
    engine.register_fn("text_set_wrap", move |key: &str, width: f64| -> bool {
        with_text(&c, key, |t| t.wrap = f32_of(width).max(0.0))
    });

    // 行高倍数（相对字号）；0 = 用字体自然行高
    let c = ctx.clone();
    engine.register_fn(
        "text_set_line_height",
        move |key: &str, factor: f64| -> bool {
            with_text(&c, key, |t| t.line_height = f32_of(factor).max(0.0))
        },
    );

    let c = ctx.clone();
    engine.register_fn("text_set_visible", move |key: &str, on: bool| -> bool {
        with_text(&c, key, |t| t.visible = on)
    });

    let c = ctx.clone();
    engine.register_fn("text_remove", move |key: &str| -> bool {
        let mut scene = c.scene.borrow_mut();
        let before = scene.texts.len();
        scene.texts.retain(|t| t.key != key);
        scene.texts.len() != before
    });

    let c = ctx.clone();
    engine.register_fn("text_clear", move || {
        c.scene.borrow_mut().texts.clear();
    });

    let c = ctx.clone();
    engine.register_fn("text_exists", move |key: &str| -> bool {
        c.scene.borrow().texts.iter().any(|t| t.key == key)
    });

    let c = ctx.clone();
    engine.register_fn("text_content", move |key: &str| -> String {
        c.scene
            .borrow()
            .texts
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.content.clone())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn("text_count", move || -> i64 {
        c.scene.borrow().texts.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("text_keys", move || -> Array {
        c.scene
            .borrow()
            .texts
            .iter()
            .map(|t| Dynamic::from(t.key.clone()))
            .collect()
    });

    // 文本通道总开关：关掉后渲染层整帧跳过文本 pass（3D 主通道行为不变）。
    let c = ctx.clone();
    engine.register_fn("set_text_enabled", move |on: bool| {
        c.scene.borrow_mut().text_enabled = on;
    });

    let c = ctx.clone();
    engine.register_fn("text_enabled", move || -> bool {
        c.scene.borrow().text_enabled
    });

    // 宽度 / 高度估算：不加载字体文件的近似值，脚本排版预算用（不是最终像素宽度）。
    engine.register_fn("text_width", |content: &str, size: f64| -> f64 {
        estimate_text_width(content, sanitize_size(f32_of(size))) as f64
    });

    engine.register_fn("text_height", |content: &str, size: f64| -> f64 {
        estimate_text_height(content, sanitize_size(f32_of(size))) as f64
    });

    // ---------------- 2D 屏幕空间 UI（M1 最小 UI）----------------
    // 与 text_* 同构：场景里的 `ui` 是唯一真源，这些绑定只改数据。
    // 都用 `key` 定位，同名即整体覆盖 —— 脚本每帧调用也不会重复堆积。
    // 尺寸 / 偏移写成带单位字符串："24px"、"50%"（百分比按屏幕宽高算）。
    //
    // 三条画元素的入口（`ui_rect` / `ui_border` / `ui_image`）全部走 core 的
    // `r3e_core::ui::upsert_styled`：数量上限与「同 key 怎么合并」都由 core 定，
    // 脚本层只负责把参数搬过去。返回值：true = 新建成功，false = 没新增（同 key
    // 覆盖 或 被上限拒绝）；被拒原因留在 `ui_last_error()` / `ui_error_count()`。
    let c = ctx.clone();
    engine.register_fn(
        "ui_rect",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let el = ui_element(
                &c,
                "ui_rect",
                key,
                UiKind::Rect,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "ui_border",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              width: f64,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let mut el = ui_element(
                &c,
                "ui_border",
                key,
                UiKind::Border,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            el.border_width = f32_of(width);
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "ui_image",
        move |key: &str,
              anchor: &str,
              x: &str,
              y: &str,
              w: &str,
              h: &str,
              path: &str,
              r: f64,
              g: f64,
              b: f64,
              a: f64|
              -> bool {
            let mut el = ui_element(
                &c,
                "ui_image",
                key,
                UiKind::Image,
                anchor,
                ([x, y], [w, h]),
                [r, g, b, a],
            );
            el.image = path.to_string();
            submit_ui(&c, el)
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_set_pos", move |key: &str, x: &str, y: &str| -> bool {
        with_ui(&c, key, |el| {
            el.offset = [parse_ui_value(x), parse_ui_value(y)]
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_size", move |key: &str, w: &str, h: &str| -> bool {
        with_ui(&c, key, |el| {
            el.size = [parse_ui_value(w), parse_ui_value(h)]
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_anchor", move |key: &str, anchor: &str| -> bool {
        match Anchor::parse(anchor) {
            Some(a) => with_ui(&c, key, |el| el.anchor = a),
            None => {
                println!(
                    "[script] 未知锚点 `{anchor}`（可选 topleft / topcenter / ... / bottomright）"
                );
                false
            }
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "ui_set_color",
        move |key: &str, r: f64, g: f64, b: f64, a: f64| -> bool {
            with_ui(&c, key, |el| {
                el.color = [
                    f32_of(r).clamp(0.0, 1.0),
                    f32_of(g).clamp(0.0, 1.0),
                    f32_of(b).clamp(0.0, 1.0),
                    f32_of(a).clamp(0.0, 1.0),
                ]
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_set_alpha", move |key: &str, a: f64| -> bool {
        with_ui(&c, key, |el| el.color[3] = f32_of(a).clamp(0.0, 1.0))
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_layer", move |key: &str, layer: i64| -> bool {
        with_ui(&c, key, |el| {
            el.layer = layer.clamp(i32::MIN as i64, i32::MAX as i64) as i32
        })
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_visible", move |key: &str, on: bool| -> bool {
        with_ui(&c, key, |el| el.visible = on)
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_border", move |key: &str, width: f64| -> bool {
        with_ui(&c, key, |el| el.border_width = f32_of(width))
    });

    let c = ctx.clone();
    engine.register_fn("ui_set_image", move |key: &str, path: &str| -> bool {
        with_ui(&c, key, |el| el.image = path.to_string())
    });

    // uv 子区间（图集裁切；九宫格切分时作为整体外边界）
    let c = ctx.clone();
    engine.register_fn(
        "ui_set_uv",
        move |key: &str, u0: f64, v0: f64, u1: f64, v1: f64| -> bool {
            with_ui(&c, key, |el| {
                el.uv = [f32_of(u0), f32_of(v0), f32_of(u1), f32_of(v1)]
            })
        },
    );

    // 九宫格边距（左 / 上 / 右 / 下）：像素 "24px" 或百分比 "10%"（按元素自身宽 / 高）
    // 无参调用 = 关闭九宫格（四条边距清零），与传全 0 等价
    let c = ctx.clone();
    engine.register_fn("ui_set_slice", move |key: &str| -> bool {
        with_ui(&c, key, |el| {
            el.slice_margin = [
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
                UiValue::px(0.0),
            ]
        })
    });

    let c = ctx.clone();
    engine.register_fn(
        "ui_set_slice",
        move |key: &str, left: &str, top: &str, right: &str, bottom: &str| -> bool {
            with_ui(&c, key, |el| {
                el.slice_margin = [
                    parse_ui_value(left),
                    parse_ui_value(top),
                    parse_ui_value(right),
                    parse_ui_value(bottom),
                ]
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("ui_remove", move |key: &str| -> bool {
        let mut scene = c.scene.borrow_mut();
        let before = scene.ui.len();
        scene.ui.retain(|el| el.key != key);
        scene.ui.len() != before
    });

    let c = ctx.clone();
    engine.register_fn("ui_clear", move || {
        c.scene.borrow_mut().ui.clear();
    });

    let c = ctx.clone();
    engine.register_fn("ui_exists", move |key: &str| -> bool {
        c.scene.borrow().ui.iter().any(|el| el.key == key)
    });

    let c = ctx.clone();
    engine.register_fn("ui_count", move || -> i64 {
        c.scene.borrow().ui.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("ui_keys", move || -> Array {
        c.scene
            .borrow()
            .ui
            .iter()
            .map(|el| Dynamic::from(el.key.clone()))
            .collect()
    });

    // 容量上限（core 定值，脚本侧只读）：脚本可以据此提前决定要不要新建元素。
    engine.register_fn("ui_capacity", move || -> i64 { MAX_UI_ELEMENTS as i64 });

    // 被拒 / 被纠正的写入：原因 + 累计次数。与 `audio_last_error` 同构，不刷屏。
    let c = ctx.clone();
    engine.register_fn("ui_last_error", move || -> String {
        c.ui_log.borrow().last_error().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("ui_error_count", move || -> i64 {
        c.ui_log.borrow().error_count() as i64
    });

    // UI 通道总开关：关掉后渲染层整帧跳过 UI pass（3D / 文本行为不变）。
    let c = ctx.clone();
    engine.register_fn("set_ui_enabled", move |on: bool| {
        c.scene.borrow_mut().ui_enabled = on;
    });

    let c = ctx.clone();
    engine.register_fn("ui_enabled", move || -> bool {
        c.scene.borrow().ui_enabled
    });

    // 文本与 UI 的层次：false（默认）= 文本在上；true = UI 盖住文本。
    let c = ctx.clone();
    engine.register_fn("set_ui_above_text", move |on: bool| {
        c.scene.borrow_mut().ui_above_text = on;
    });

    let c = ctx.clone();
    engine.register_fn("ui_above_text", move || -> bool {
        c.scene.borrow().ui_above_text
    });

    // ------------------------------------------------------------------
    // 音频（非空间化）：只做常规音效 / 音乐，没有 3D 衰减与多普勒。
    //
    // 统一约定：
    // * 参数 key 是调用方自己起的**声音名字**（不是文件路径），同名 key 重播 = 停掉旧实例从头播，
    //   不会叠加第二份；想同时放两份同一文件，就起两个 key。
    // * 所有函数都不 panic：无设备 / 文件打不开 / 解码失败 一律返回 false，并把原因留在
    //   `audio_last_error()`，主循环继续跑。
    // ------------------------------------------------------------------

    let c = ctx.clone();
    engine.register_fn("audio_play", move |key: &str, path: &str| -> bool {
        c.audio.borrow_mut().play_default(key, path).is_ok()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_play_ex",
        move |key: &str, path: &str, volume: f64, looping: bool| -> bool {
            c.audio
                .borrow_mut()
                .play(key, path, f32_of(volume), looping)
                .is_ok()
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_stop", move |key: &str| -> bool {
        c.audio.borrow_mut().stop(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_stop_all", move || {
        c.audio.borrow_mut().stop_all();
    });

    let c = ctx.clone();
    engine.register_fn("audio_pause", move |key: &str| -> bool {
        c.audio.borrow_mut().pause(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_resume", move |key: &str| -> bool {
        c.audio.borrow_mut().resume(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_set_volume", move |key: &str, volume: f64| -> bool {
        c.audio.borrow_mut().set_volume(key, f32_of(volume))
    });

    let c = ctx.clone();
    engine.register_fn("audio_set_loop", move |key: &str, looping: bool| -> bool {
        c.audio.borrow_mut().set_loop(key, looping)
    });

    let c = ctx.clone();
    engine.register_fn("audio_is_playing", move |key: &str| -> bool {
        c.audio.borrow_mut().is_playing(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_is_paused", move |key: &str| -> bool {
        c.audio.borrow().is_paused(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_count", move || -> i64 {
        c.audio.borrow().active_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_keys", move || -> Array {
        c.audio
            .borrow()
            .keys()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    // 总音量：与单条音量相乘后下发给混音器，0 也合法（等于静音）。
    let c = ctx.clone();
    engine.register_fn("audio_set_master_volume", move |volume: f64| {
        c.audio.borrow_mut().set_master_volume(f32_of(volume));
    });

    let c = ctx.clone();
    engine.register_fn("audio_master_volume", move || -> f64 {
        c.audio.borrow().master_volume() as f64
    });

    // 总开关：关掉后新建的播放请求直接拒绝（返回 false），已有声音全部停掉。
    let c = ctx.clone();
    engine.register_fn("audio_set_enabled", move |on: bool| {
        c.audio.borrow_mut().set_enabled(on);
    });

    let c = ctx.clone();
    engine.register_fn("audio_enabled", move || -> bool {
        c.audio.borrow().enabled()
    });

    // 探针：有真实设备 = true；静默降级 = false。脚本可据此不显示音量控件。
    let c = ctx.clone();
    engine.register_fn("audio_available", move || -> bool {
        c.audio.borrow().device_available()
    });

    let c = ctx.clone();
    engine.register_fn("audio_backend", move || -> String {
        c.audio.borrow().backend_name().to_string()
    });

    let c = ctx.clone();
    engine.register_fn("audio_last_error", move || -> String {
        c.audio.borrow().last_error().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("audio_error_count", move || -> i64 {
        c.audio.borrow().error_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_status", move || -> String {
        c.audio.borrow_mut().status_line()
    });
}

/// 解析带单位的 UI 数值："24px" / "50%" / "12"（默认像素）。
fn parse_ui_value(text: &str) -> UiValue {
    let raw = text.trim();
    if let Some(num) = raw.strip_suffix('%') {
        return UiValue::pct(num.trim().parse::<f32>().unwrap_or(0.0));
    }
    let num = raw
        .strip_suffix("px")
        .unwrap_or(raw)
        .trim()
        .parse::<f32>()
        .unwrap_or(0.0);
    UiValue::px(num)
}

/// 按 key 修改场景里的一条 UI 元素；找不到返回 false。
fn with_ui(ctx: &ScriptContext, key: &str, edit: impl FnOnce(&mut UiElement)) -> bool {
    let mut scene = ctx.scene.borrow_mut();
    match find_mut(&mut scene.ui, key) {
        Some(el) => {
            edit(el);
            true
        }
        None => false,
    }
}

/// 把脚本参数搬成 core 的 [`UiElement`]（只做解析与搬运，不含任何合并 / 覆盖策略）。
///
/// 锚点写错时回退左上角并记一笔账，但不阻断写入——每帧都在调的脚本不该因为一个笔误
/// 整块 UI 消失。
fn ui_element(
    ctx: &ScriptContext,
    func: &str,
    key: &str,
    kind: UiKind,
    anchor: &str,
    geom: ([&str; 2], [&str; 2]),
    color: [f64; 4],
) -> UiElement {
    let (offset, size) = geom;
    let anchor = match Anchor::parse(anchor) {
        Some(a) => a,
        None => {
            println!("[script] {func}: 未知锚点 `{anchor}`，回退为 topleft");
            ctx.ui_log
                .borrow_mut()
                .record(format!("{func}: 未知锚点 `{anchor}`，回退 topleft"));
            Anchor::TopLeft
        }
    };
    let mut el = UiElement::new(key, kind, anchor);
    el.offset = [parse_ui_value(offset[0]), parse_ui_value(offset[1])];
    el.size = [parse_ui_value(size[0]), parse_ui_value(size[1])];
    el.color = [
        f32_of(color[0]),
        f32_of(color[1]),
        f32_of(color[2]),
        f32_of(color[3]),
    ];
    el
}

/// 脚本侧唯一 UI 写入口：策略（同 key 合并 / 数量上限 / 非法值收敛）全在 core，
/// 这里只做提交与拒绝记账。
///
/// 返回值沿用 `text_add` 的约定：`true` = 新建了元素，`false` = 没新增（同 key 覆盖
/// 或 超限被拒）。想区分「覆盖」和「被拒」，看 `ui_error_count()` / `ui_last_error()`。
fn submit_ui(ctx: &ScriptContext, el: UiElement) -> bool {
    let key = el.key.trim().to_string();
    if key.is_empty() {
        let msg = format!("{}: key 不能为空，已忽略", el.kind.name());
        println!("[script] {msg}");
        ctx.ui_log.borrow_mut().record(msg);
        return false;
    }
    let mut el = el;
    el.key = key.clone();
    let mut scene = ctx.scene.borrow_mut();
    match upsert_styled(&mut scene.ui, el) {
        UiUpsert::Inserted => true,
        UiUpsert::Replaced => false,
        UiUpsert::Rejected => {
            drop(scene);
            let msg = format!("UI 元素数量已达上限 {MAX_UI_ELEMENTS}，key `{key}` 未写入");
            println!("[script] {msg}");
            ctx.ui_log.borrow_mut().record(msg);
            false
        }
    }
}

/// 按 key 修改场景里的一条文本；找不到返回 false（脚本可据此判断要不要 `text_add`）。
fn with_text(ctx: &ScriptContext, key: &str, edit: impl FnOnce(&mut ScreenText)) -> bool {
    let mut scene = ctx.scene.borrow_mut();
    match scene.texts.iter_mut().find(|t| t.key == key) {
        Some(t) => {
            edit(t);
            true
        }
        None => false,
    }
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
