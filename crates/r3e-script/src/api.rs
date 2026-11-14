//! 脚本可调用的引擎 API 绑定。
//!
//! 全部函数都通过 `Rc<RefCell<..>>` 访问场景 / 物理 / 时钟，
//! 因此脚本执行与主循环渲染共享同一份真源数据，改动下一帧即生效。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::{Vec3, Vec4};
use r3e_audio::{Audio, DistanceModel, SpatialSettings};
use r3e_core::input::{parse_key, parse_mouse_button, parse_pad_axis, parse_pad_button};
use r3e_core::postfx::TonemapMode;
use r3e_core::ssao::{MAX_SSAO_SAMPLES, MIN_SSAO_SAMPLES};
use r3e_core::text::{estimate_text_height, estimate_text_width, sanitize_size};
use r3e_core::ui::{
    find_mut, hit_test_key, upsert_styled, Anchor, UiElement, UiEventKind, UiInteraction, UiKind,
    UiUpsert, UiValue, MAX_UI_ELEMENTS,
};
use r3e_core::{
    pick_all_from_screen, pick_from_screen, plan_local_shadows, BoneMask, EnvironmentMapSettings,
    InputState, KeyCode, LayerMode, LayerSync, LocalLight, MeshAabbCache, MeshRef, NodeId,
    ObjectDesc, PickBounds, PickHit, Scene, SceneCommand, SceneStack, ScreenText, TextAlign,
    NO_EXCLUSIVE_GROUP,
};
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
    /// UI 指针交互状态（hover / active / 每帧事件）。
    ///
    /// 由 `app` 每帧 `update` 推进、脚本侧只读：两边共用同一个 `Rc`，所以脚本在
    /// `on_update` 里读到的事件一定是本帧的，不是上一帧的残留。
    pub ui_interaction: Rc<RefCell<UiInteraction>>,
    /// 场景栈（扩展十八 B）。与 `app` 共用同一份：脚本只读（`scene_stack_names` 等），
    /// 真正的压栈 / 回退由宿主做。
    pub scene_stack: Rc<RefCell<SceneStack>>,
    /// 场景命令槽（扩展十八 B）：脚本只能**申请**切场景 / 存档，宿主在帧首统一执行。
    ///
    /// 一个槽只放一条命令：满了就拒绝（`scene_switch` 等返回 `false`），这样脚本在
    /// `on_update` 里每帧无脑调用也不会把队列堆成无限长的待办。
    pub scene_cmd: Rc<RefCell<Option<SceneCommand>>>,
    /// 网格包围盒表（扩展三十四）：`app` 每帧把「场景里实际引用到的网格」登记进来，
    /// 脚本的 `pick_at` 用它把屏幕射线换成命中物件。
    ///
    /// 与 `ui_interaction` 同样是两边共用的 `Rc`：宿主负责填充，脚本只读。
    /// 表里查不到的网格一律当「不可拾取」——绝不退回原点盒，否则会点中空气。
    pub pick_bounds: Rc<RefCell<MeshAabbCache>>,
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

/// 样本数先上下夹再转 `u8`：直接 `as u8` 遇到负数是回绕，不是饱和。
fn ssao_samples_of(v: i64) -> u8 {
    v.clamp(i64::from(MIN_SSAO_SAMPLES), i64::from(MAX_SSAO_SAMPLES)) as u8
}

/// 骨骼遮罩字符串解析（扩展十八 A）：`"arm,hand"` 只作用于这两处；
/// `"!root"` 作用于除 root 外全部；空串或 `"*"` 表示整副骨架。
///
/// 写成字符串而不是数组，是因为脚本里 `["arm","hand"]` 要打字更多、日志也更难看；
/// 匹配规则与 [`BoneMask::path_matches`] 一致（按路径后缀），`"arm"` 命中 `root/spine/arm`。
fn parse_bone_mask(spec: &str) -> BoneMask {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return BoneMask::pass_all();
    }
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for part in trimmed.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p.strip_prefix('!') {
            Some(rest) if !rest.trim().is_empty() => exclude.push(rest.trim().to_string()),
            _ => include.push(p.to_string()),
        }
    }
    if include.is_empty() && exclude.is_empty() {
        return BoneMask::pass_all();
    }
    BoneMask { include, exclude }
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

/// 拾取结果的空模板（字段名与命中版完全一致，理由同 [`empty_query_map`]）。
///
/// 与物理 `raycast` 的字段是**并存**的两套：`raycast` 走物理后端、只有挂了刚体的物件
/// 才可能命中；`pick_at` 走渲染几何、任何有包围盒的可见物件都能点到。二者刻意不合并，
/// 否则「点到装饰物」和「点到刚体」会混成一个让人猜不透的接口。
fn empty_pick_map() -> Map {
    let mut m = Map::new();
    m.insert("hit".into(), Dynamic::from(false));
    m.insert("node".into(), Dynamic::from(-1_i64));
    m.insert("name".into(), Dynamic::from(String::new()));
    m.insert("distance".into(), Dynamic::from(-1.0_f64));
    m.insert("point".into(), Dynamic::from(Array::new()));
    m
}

/// 拾取命中 → 脚本 Map（`pick_at` / `pick_under_mouse` / `pick_all_at` 共用）。
fn pick_map(scene: &Scene, hit: Option<PickHit>) -> Map {
    let Some(hit) = hit else {
        return empty_pick_map();
    };
    let mut m = Map::new();
    m.insert("hit".into(), Dynamic::from(true));
    m.insert("node".into(), Dynamic::from(hit.node as i64));
    let name = scene
        .get(hit.node)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    m.insert("name".into(), Dynamic::from(name));
    m.insert("distance".into(), Dynamic::from(hit.distance as f64));
    m.insert("point".into(), Dynamic::from(vec3_array(hit.point)));
    m
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
/// 把一条场景命令放进槽里；槽已满（上一条还没被宿主取走）时返回 `false`。
///
/// 空路径一律拒绝：`scene_switch("")` 在宿主侧会退化成"重新载入当前场景"这类
/// 意外行为，不如在脚本边界上就说不。`scene_save_archive("")` 是例外——空串
/// 表示"存到宿主默认位置"，因此 `None` 与空串等价。
fn queue_scene_cmd(ctx: &ScriptContext, cmd: SceneCommand) -> bool {
    let mut slot = ctx.scene_cmd.borrow_mut();
    if slot.is_some() {
        return false;
    }
    *slot = Some(cmd);
    true
}

fn non_empty_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

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

    // ---------------- 顶点形变（扩展三十六） ----------------
    //
    // 脚本侧只做「登记绑定 + 改权重」两件事：形变在顶点着色器里算，
    // 权重变了下一帧就生效，不需要重建网格或重传缓冲。

    let c = ctx.clone();
    engine.register_fn("morph_bind", move |name: &str, target_count: i64| -> bool {
        if target_count <= 0 {
            return false;
        }
        c.scene
            .borrow_mut()
            .bind_morph_by_name(name, target_count as usize)
    });

    let c = ctx.clone();
    engine.register_fn(
        "morph_set",
        move |name: &str, index: i64, weight: f64| -> bool {
            if index < 0 {
                return false;
            }
            c.scene
                .borrow_mut()
                .set_morph_weight_by_name(name, index as usize, f32_of(weight))
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_reset", move |name: &str| -> bool {
        c.scene.borrow_mut().reset_morph(name)
    });

    let c = ctx.clone();
    engine.register_fn("morph_weight", move |name: &str, index: i64| -> f64 {
        if index < 0 {
            return -1.0;
        }
        c.scene
            .borrow()
            .morph_weight_by_name(name, index as usize)
            .map(|w| w as f64)
            .unwrap_or(-1.0)
    });

    let c = ctx.clone();
    engine.register_fn("morph_count", move |name: &str| -> i64 {
        c.scene
            .borrow()
            .morph_target_count(name)
            .map(|n| n as i64)
            .unwrap_or(-1)
    });

    // 权重是否全部为 0（渲染层据此跳过整段形变路径）。
    let c = ctx.clone();
    engine.register_fn("morph_active", move |name: &str| -> bool {
        let s = c.scene.borrow();
        s.find_id(name)
            .and_then(|node| s.morph_of(node))
            .map(|m| m.is_active())
            .unwrap_or(false)
    });

    // ---------------- 表情互斥（扩展三十八） ----------------
    //
    // 把若干形状划进同一个组，组内权重和超过 1 时渲染层等比压回 1。
    // 脚本里 group 写 -1 即「取出该形状」（对应 `NO_EXCLUSIVE_GROUP` 哨兵）。

    let c = ctx.clone();
    engine.register_fn(
        "morph_exclusive",
        move |name: &str, index: i64, group: i64| -> bool {
            if index < 0 || group < -1 || group > u8::MAX as i64 {
                return false;
            }
            let slot = if group < 0 {
                NO_EXCLUSIVE_GROUP
            } else {
                group as u8
            };
            c.scene
                .borrow_mut()
                .set_morph_exclusive_by_name(name, index as usize, slot)
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_group", move |name: &str, index: i64| -> i64 {
        if index < 0 {
            return -1;
        }
        match c
            .scene
            .borrow()
            .morph_exclusive_by_name(name, index as usize)
        {
            Some(group) => group as i64,
            None => -1,
        }
    });

    // 归一化后的权重：与渲染层剔除吃的是同一份数，可直接断言「被压回 1」
    let c = ctx.clone();
    engine.register_fn("morph_norm", move |name: &str, index: i64| -> f64 {
        if index < 0 {
            return -1.0;
        }
        match c
            .scene
            .borrow()
            .morph_normalized_weight_by_name(name, index as usize)
        {
            Some(w) => w as f64,
            None => -1.0,
        }
    });

    // ---------------- 逐实例形状掩码（扩展四十） ----------------
    //
    // 同一个网格可以有十几个实例，各用一套形状子集：掩码第 i 位 = 形状 i 启用。
    // 掩码关空的实例整段摘掉形变（`morph_count` 写 0），半掩的实例权重按掩码归零。

    // 整块写掩码，返回裁剪到「网格形状数」之后的生效值；没有绑定返回 -1
    let c = ctx.clone();
    engine.register_fn("morph_mask_set", move |name: &str, mask: i64| -> i64 {
        if !(0..=u32::MAX as i64).contains(&mask) {
            return -1;
        }
        match c
            .scene
            .borrow_mut()
            .set_morph_mask_by_name(name, mask as u32)
        {
            Some(effective) => effective as i64,
            None => -1,
        }
    });

    let c = ctx.clone();
    engine.register_fn("morph_mask", move |name: &str| -> i64 {
        match c.scene.borrow().morph_mask_by_name(name) {
            Some(mask) => mask as i64,
            None => -1,
        }
    });

    // 单个形状开 / 关：脚本里写这个比算位运算直观
    let c = ctx.clone();
    engine.register_fn(
        "morph_show",
        move |name: &str, index: i64, enabled: bool| -> bool {
            if index < 0 {
                return false;
            }
            c.scene
                .borrow_mut()
                .set_morph_shape_enabled_by_name(name, index as usize, enabled)
        },
    );

    let c = ctx.clone();
    engine.register_fn("morph_visible", move |name: &str, index: i64| -> bool {
        if index < 0 {
            return false;
        }
        c.scene
            .borrow()
            .morph_shape_enabled_by_name(name, index as usize)
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

    // ---------------- 动画混合树（扩展十八 A） ----------------
    // 主片段是「当前动作」，层是同时进行的附加动作（上半身攻击、呼吸叠加）。
    // 层用名字寻址（`layer` 传空串 = 第一层）；遮罩用逗号串表达：
    // `"arm,hand"` 只作用于这两处骨骼，`"!root"` 表示除 root 外全部，
    // 空串或 `"*"` 表示整副骨架（默认）——骨骼按路径后缀匹配，`"arm"` 命中 `root/spine/arm`。
    let c = ctx.clone();
    engine.register_fn("anim_layers", move |inst: &str| -> Array {
        c.scene
            .borrow()
            .anim_layer_names(inst)
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("anim_layer_count", move |inst: &str| -> i64 {
        c.scene
            .borrow()
            .anim_layer_count(inst)
            .map(|n| n as i64)
            .unwrap_or(0)
    });

    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_info",
        move |inst: &str, layer: &str| -> String {
            c.scene
                .borrow()
                .anim_layer_report(inst, layer)
                .unwrap_or_else(|| format!("实例 `{inst}` 上没有名为 `{layer}` 的层"))
        },
    );

    // 有效权重（含进行中的过渡），0 = 层此刻完全不参与合成。
    let c = ctx.clone();
    engine.register_fn("anim_layer_weight", move |inst: &str, layer: &str| -> f64 {
        c.scene
            .borrow()
            .anim_layer_effective_weight(inst, layer)
            .map(|w| w as f64)
            .unwrap_or(0.0)
    });

    // 挂一层覆盖型（override）：命中遮罩的骨骼直接取层姿态。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.scene.borrow_mut().add_anim_layer_by_clip(
                inst,
                layer,
                clip,
                f32_of(weight),
                LayerMode::Override,
            )
        },
    );

    // 挂一层加法型（additive）：层姿态相对首帧的增量叠到主姿态上，适合呼吸 / 抖动。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_additive",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.scene.borrow_mut().add_anim_layer_by_clip(
                inst,
                layer,
                clip,
                f32_of(weight),
                LayerMode::Additive,
            )
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_weight",
        move |inst: &str, layer: &str, weight: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_weight(inst, layer, f32_of(weight))
        },
    );

    // 带过渡地改层权重：`dur` 秒内线性+平滑过渡，游戏里淡入淡出层都用这个。
    let c = ctx.clone();
    engine.register_fn(
        "fade_anim_layer",
        move |inst: &str, layer: &str, weight: f64, dur: f64| -> bool {
            c.scene
                .borrow_mut()
                .fade_anim_layer(inst, layer, f32_of(weight), dur as f32)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_mode",
        move |inst: &str, layer: &str, mode: &str| -> bool {
            let Some(mode) = LayerMode::parse(mode) else {
                println!("[script] 未知叠加模式 `{mode}`（可用：override / additive）");
                return false;
            };
            c.scene.borrow_mut().set_anim_layer_mode(inst, layer, mode)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_mask",
        move |inst: &str, layer: &str, spec: &str| -> bool {
            let mask = parse_bone_mask(spec);
            c.scene.borrow_mut().set_anim_layer_mask(inst, layer, mask)
        },
    );

    // 遮罩回显：`"*"` = 全部，其余按 `include,!exclude` 还原，便于脚本自检与日志验收。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_mask",
        move |inst: &str, layer: &str| -> String {
            let s = c.scene.borrow();
            match s.anim_layer(inst, layer) {
                Some(l) if l.mask.is_pass_all() => "*".to_string(),
                Some(l) => {
                    let mut parts = l.mask.include.clone();
                    parts.extend(l.mask.exclude.iter().map(|e| format!("!{e}")));
                    parts.join(",")
                }
                None => String::new(),
            }
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_speed",
        move |inst: &str, layer: &str, speed: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_speed(inst, layer, f32_of(speed))
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clips",
        move |inst: &str, layer: &str| -> Array {
            c.scene
                .borrow()
                .anim_layer(inst, layer)
                .map(|l| l.clip_names().into_iter().map(Dynamic::from).collect())
                .unwrap_or_default()
        },
    );

    // 层内多片段加权混合：走路层里叠加「走 / 跑」两路片段按权重过渡。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.scene
                .borrow_mut()
                .add_anim_layer_clip(inst, layer, clip, f32_of(weight))
        },
    );

    // 改层内某一路片段的权重（混合空间摘掉之后，手写权重从这里接管）。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_weight",
        move |inst: &str, layer: &str, clip: &str, weight: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_clip_weight(inst, layer, clip, f32_of(weight))
        },
    );

    // 把层的片段整体换成指定片段（层内收敛为单片段，权重保留）。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.scene.borrow_mut().set_anim_layer_clip(inst, layer, clip)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "remove_anim_layer_clip",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.scene
                .borrow_mut()
                .remove_anim_layer_clip(inst, layer, clip)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "remove_anim_layer",
        move |inst: &str, layer: &str| -> bool {
            c.scene.borrow_mut().remove_anim_layer(inst, layer)
        },
    );

    let c = ctx.clone();
    engine.register_fn("clear_anim_layers", move |inst: &str| -> bool {
        c.scene.borrow_mut().clear_anim_layers(inst)
    });

    // ---------------- 1D / 2D / 3D 混合空间（扩展四十五 / 四十九） ----------------
    //
    // 用法：给层挂一个混合空间 → 往里塞 (片段, 参数坐标) 样本 → 每帧推参数值。
    // 层内片段的权重就由「参数落在样本图上的位置」算出来，脚本不用再手算权重。
    // 三维空间（扩展四十九）把「八向移动 × 三档速度」这类原本要两层 2D 叠的表达
    // 收成一张四面体图：多了 `_3d` 三个入口，二维入口在三维空间上仍可用（前两维）。

    // 挂一维混合空间：`param` 只是参数名（日志/回显用），例如 "speed"。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_1d",
        move |inst: &str, layer: &str, param: &str| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_blend_space_1d(inst, layer, param)
        },
    );

    // 挂二维混合空间：`px` / `py` 两个参数名，例如 "move.x" / "move.y"。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_2d",
        move |inst: &str, layer: &str, px: &str, py: &str| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_blend_space_2d(inst, layer, px, py)
        },
    );

    // 挂三维混合空间（扩展四十九）：`px` / `py` / `pz` 三个参数名。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_3d",
        move |inst: &str, layer: &str, px: &str, py: &str, pz: &str| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_blend_space_3d(inst, layer, px, py, pz)
        },
    );

    // 摘掉空间，层回到手写权重（脚本里做「动态混合 / 硬编码权重」切换时用）。
    let c = ctx.clone();
    engine.register_fn(
        "clear_anim_layer_blend",
        move |inst: &str, layer: &str| -> bool {
            c.scene
                .borrow_mut()
                .clear_anim_layer_blend_space(inst, layer)
        },
    );

    // 加样本：`x` / `y` 是这块片段在参数空间里的坐标（一维只认 x）。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_blend_sample",
        move |inst: &str, layer: &str, clip: &str, x: f64, y: f64| -> bool {
            c.scene
                .borrow_mut()
                .add_anim_layer_blend_sample(inst, layer, clip, [x as f32, y as f32])
                .unwrap_or(false)
        },
    );

    // 加三维样本（扩展四十九）：`x` / `y` / `z` 是片段在参数空间里的坐标。
    let c = ctx.clone();
    engine.register_fn(
        "add_anim_layer_blend_sample_3d",
        move |inst: &str, layer: &str, clip: &str, x: f64, y: f64, z: f64| -> bool {
            c.scene
                .borrow_mut()
                .add_anim_layer_blend_sample_3d(inst, layer, clip, [x as f32, y as f32, z as f32])
                .unwrap_or(false)
        },
    );

    // 每帧推参数值（一维只认 x）：权重随参数插值 / 外插钳制在内部完成。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_value",
        move |inst: &str, layer: &str, x: f64, y: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_blend_value(inst, layer, x as f32, y as f32)
                .unwrap_or(false)
        },
    );

    // 三维推值（扩展四十九）：二维入口在三维空间上等价于 z=0，要第三维就用这个。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_blend_value_3d",
        move |inst: &str, layer: &str, x: f64, y: f64, z: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_blend_value_3d(inst, layer, x as f32, y as f32, z as f32)
                .unwrap_or(false)
        },
    );

    // 回显：`3d(move.x,move.y,speed) n=24 tet=28 drop=0 v=(0.00,1.00,1.00) w=...`，没挂空间返回空串。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend",
        move |inst: &str, layer: &str| -> String {
            let s = c.scene.borrow();
            let Some(brief) = s.anim_layer_blend_brief(inst, layer) else {
                return String::new();
            };
            let line = s
                .anim_layer_blend_weights(inst, layer)
                .unwrap_or_default()
                .iter()
                .map(|(name, w)| format!("{name}={w:.2}"))
                .collect::<Vec<_>>()
                .join(",");
            format!("{brief} w={line}")
        },
    );

    // 剖分挂起标记（扩展五十一）：批量 `add_anim_layer_blend_sample*` 之后为 `true`，
    // 下一帧 advance（或显式 `build_anim_layer_blend`）之后回到 `false`。
    // 没挂空间返回 `false`——探针里不用先判空间存不存在。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_blend_stale",
        move |inst: &str, layer: &str| -> bool {
            c.scene
                .borrow()
                .anim_layer_blend_stale(inst, layer)
                .unwrap_or(false)
        },
    );

    // 立刻提交挂起的剖分（扩展五十一）：`true` = 这次真的重建了，`false` = 本来就最新
    // （编辑器装配完马上要画线框 / 探针要读剖分表时用，省得等一帧）。
    let c = ctx.clone();
    engine.register_fn(
        "build_anim_layer_blend",
        move |inst: &str, layer: &str| -> bool {
            c.scene
                .borrow_mut()
                .build_anim_layer_blend_pending(inst, layer)
                .unwrap_or(false)
        },
    );

    // ---------------- 层内相位同步（扩展四十六） ----------------
    //
    // 用法：走 / 跑这类长短不一的片段同处一层时，开相位锁让两路按同一相位采样
    // （脚掌同时落地）；不开则沿用老口径——整层共用一个时间，短片段到末帧停住。

    // 开关相位锁：`true` = 各片段按归一化相位缩放采样。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync",
        move |inst: &str, layer: &str, enabled: bool| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_sync(inst, layer, enabled)
        },
    );

    // 按名字设策略：`"off"` / `"phase"`（大小写不敏感，`none` / `sync` 也认）。
    // 名字解析不了返回 false，层状态不动。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync_mode",
        move |inst: &str, layer: &str, mode: &str| -> bool {
            let Some(sync) = LayerSync::parse(mode) else {
                return false;
            };
            c.scene
                .borrow_mut()
                .set_anim_layer_sync_strategy(inst, layer, sync)
        },
    );

    // 回显：层是否开着相位锁（层不存在返回 false）。
    let c = ctx.clone();
    engine.register_fn("anim_layer_sync", move |inst: &str, layer: &str| -> bool {
        c.scene
            .borrow()
            .anim_layer_sync(inst, layer)
            .unwrap_or(false)
    });

    // 层相位（0..1，层不存在返回 0）。
    let c = ctx.clone();
    engine.register_fn("anim_layer_phase", move |inst: &str, layer: &str| -> f64 {
        c.scene
            .borrow()
            .anim_layer_phase(inst, layer)
            .map(f64::from)
            .unwrap_or(0.0)
    });

    // 单行探针：`sync=phase phase=0.300 dur=1.000 drift=0.000 slow=0.300/1.000,...`
    // `drift` 是各路片段相位与层相位的最大偏差，锁相位时恒为 0。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_probe",
        move |inst: &str, layer: &str| -> String {
            c.scene
                .borrow()
                .anim_layer_sync_probe(inst, layer)
                .unwrap_or_default()
        },
    );

    // ---------------- 相位偏移量与跨层对齐（扩展四十七） ----------------
    //
    // 用法：左右脚要「同样的步幅、错开半步」时给一路设 0.5 偏移；
    // 上半身要跟下半身同一相位起步时把上身的同步源指到下身的层名。

    // 设某一路片段的相位偏移：`offset` 折进 `[0,1)`（`-0.25` -> `0.75`，`1.25` -> `0.25`）。
    // `clip` 传空串表示层内第一路；片段/层不存在返回 false。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_phase_offset",
        move |inst: &str, layer: &str, clip: &str, offset: f64| -> bool {
            c.scene
                .borrow_mut()
                .set_anim_layer_clip_phase_offset(inst, layer, clip, f32_of(offset))
        },
    );

    // 回显某一路的相位偏移（没设过是 0）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_phase_offset",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.scene
                .borrow()
                .anim_layer_clip_phase_offset(inst, layer, clip)
                .map(f64::from)
                .unwrap_or(0.0)
        },
    );

    // 设跨层相位源：`source` 层名（大小写不敏感，空串 = 清除、跟回自己）。
    // 源层不存在 / 指向自己 / 成环返回 false，层状态回滚。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_sync_source",
        move |inst: &str, layer: &str, source: &str| -> bool {
            let source = if source.trim().is_empty() {
                None
            } else {
                Some(source)
            };
            c.scene
                .borrow_mut()
                .set_anim_layer_sync_source(inst, layer, source)
        },
    );

    // 回显跨层相位源（没设过返回空串）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_source",
        move |inst: &str, layer: &str| -> String {
            c.scene
                .borrow()
                .anim_layer_sync_source(inst, layer)
                .unwrap_or_default()
        },
    );

    // 跨层对齐后的有效相位（0 表示层不存在）。没挂源层时等于 `anim_layer_phase`。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_sync_phase",
        move |inst: &str, layer: &str| -> f64 {
            c.scene
                .borrow()
                .anim_layer_sync_phase(inst, layer)
                .map(f64::from)
                .unwrap_or(0.0)
        },
    );

    // 设某一路片段自带的播放速率与循环标志：速率为负倒放、0 定格首帧、非有限值按原速；
    // `looping = false` 时到末帧停住。只在层没开相位锁时影响采样。
    // `clip` 传空串表示层内第一路；片段/层不存在返回 false。
    let c = ctx.clone();
    engine.register_fn(
        "set_anim_layer_clip_playback",
        move |inst: &str, layer: &str, clip: &str, speed: f64, looping: bool| -> bool {
            c.scene.borrow_mut().set_anim_layer_clip_playback(
                inst,
                layer,
                clip,
                f32_of(speed),
                looping,
            )
        },
    );

    // 清除某一路的播放参数（回到跟着层时间轴走）。
    let c = ctx.clone();
    engine.register_fn(
        "clear_anim_layer_clip_playback",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.scene
                .borrow_mut()
                .clear_anim_layer_clip_playback(inst, layer, clip)
        },
    );

    // 回显某一路的播放速率（没设过返回 -1，因为 0 是合法的「定格首帧」）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_speed",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.scene
                .borrow()
                .anim_layer_clip_playback(inst, layer, clip)
                .map(|(speed, _)| f64::from(speed))
                .unwrap_or(-1.0)
        },
    );

    // 回显某一路的循环标志（没设过返回 true：跟层走的默认口径就是循环）。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_looping",
        move |inst: &str, layer: &str, clip: &str| -> bool {
            c.scene
                .borrow()
                .anim_layer_clip_playback(inst, layer, clip)
                .map(|(_, looping)| looping)
                .unwrap_or(true)
        },
    );

    // 某一路这一帧实际采到的时间（秒，含速率/循环折算）；层/片段不存在返回 -1。
    let c = ctx.clone();
    engine.register_fn(
        "anim_layer_clip_time",
        move |inst: &str, layer: &str, clip: &str| -> f64 {
            c.scene
                .borrow()
                .anim_layer_clip_time(inst, layer, clip)
                .map(f64::from)
                .unwrap_or(-1.0)
        },
    );

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

    // ---------------- HDR 立方体反射探针（扩展二十三） ----------------
    // 语义：`scene.environment_map` 为 `None` / 未激活时，镜面 IBL 仍走上面那套解析球谐，
    // 画面逐像素不变；一旦写入来源（等距柱状单图或立方体六面），金属 / 光滑材质改用
    // 预滤波 mip 链采样。与 set_sky 一族同规矩：每次写入都过 `sanitized()`。

    // 等距柱状单图（.hdr）。空字符串视为「没写」，整段探针被清掉。
    let c = ctx.clone();
    engine.register_fn("set_env_map", move |path: &str| -> bool {
        let settings = EnvironmentMapSettings::with_equirect(path);
        c.scene
            .borrow_mut()
            .set_environment_map(settings)
            .is_active()
    });

    // 立方体六面：按 +X -X +Y -Y +Z -Z 顺序给 6 条路径，少于 6 条整段作废（返回 false）。
    let c = ctx.clone();
    engine.register_fn("set_env_map_faces", move |paths: Array| -> bool {
        let faces: Vec<String> = paths
            .iter()
            .map(|d| d.clone().into_string().unwrap_or_default())
            .collect();
        let settings = EnvironmentMapSettings::with_faces(faces);
        c.scene
            .borrow_mut()
            .set_environment_map(settings)
            .is_active()
    });

    let c = ctx.clone();
    engine.register_fn("set_env_map_intensity", move |v: f64| -> f64 {
        let mut s = c.scene.borrow_mut();
        let mut settings = s.environment_map.clone().unwrap_or_default();
        settings.intensity = f32_of(v);
        s.set_environment_map(settings).intensity as f64
    });

    let c = ctx.clone();
    engine.register_fn("set_env_map_enabled", move |on: bool| -> bool {
        c.scene.borrow_mut().set_environment_map_enabled(on)
    });

    let c = ctx.clone();
    engine.register_fn("clear_env_map", move || -> bool {
        c.scene.borrow_mut().clear_environment_map()
    });

    let c = ctx.clone();
    engine.register_fn("has_env_map", move || -> bool {
        c.scene.borrow().environment_map_active()
    });

    // 诊断出口：来源标签（none / disabled / equirect / faces），编辑器面板可直接显示。
    let c = ctx.clone();
    engine.register_fn("env_map_source", move || -> String {
        c.scene.borrow().env_map_source()
    });

    let c = ctx.clone();
    engine.register_fn("env_map_intensity", move || -> f64 {
        c.scene
            .borrow()
            .environment_map
            .as_ref()
            .map_or(1.0, |s| s.intensity as f64)
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

    // ---------------- 屏幕空间环境光遮蔽（扩展十七） ----------------
    // 遮蔽图挂在后处理段下（`postfx.ssao`），所以这一族与 `set_postfx` 同规矩：
    // 段不存在就先实体化成缺省值再写——不让脚本为了开个遮蔽而先手动开一遍后处理。
    // 半径 / 偏置 / 强度在 core 里有硬区间，样本数夹进 [4, 16]；写入一律过
    // `sanitized()`，脚本里算出来的 NaN 不会一路灌进 uniform。

    // 一击式全量设置。`samples` 越大噪点越少、开销越高，8 是缺省平衡点。
    let c = ctx.clone();
    engine.register_fn(
        "set_ssao",
        move |enabled: bool, radius: f64, bias: f64, intensity: f64, samples: i64| {
            let mut s = c.scene.borrow_mut();
            let mut fx = s.postfx.unwrap_or_default();
            fx.ssao.enabled = enabled;
            fx.ssao.radius = f32_of(radius);
            fx.ssao.bias = f32_of(bias);
            fx.ssao.intensity = f32_of(intensity);
            fx.ssao.samples = ssao_samples_of(samples);
            s.postfx = Some(fx.sanitized());
        },
    );

    // 只切开关：已调好的半径 / 强度不动，脚本里对比开关效果时不必重复写参数。
    let c = ctx.clone();
    engine.register_fn("set_ssao_enabled", move |enabled: bool| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.ssao.enabled = enabled;
        s.postfx = Some(fx.sanitized());
    });

    // 单独调半径（世界空间单位，典型 0.2 ~ 1.5）：太小只有缝隙变脏，太大糊成一片。
    let c = ctx.clone();
    engine.register_fn("set_ssao_radius", move |radius: f64| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.ssao.radius = f32_of(radius);
        s.postfx = Some(fx.sanitized());
    });

    // 单独调强度：0 等价于关闭（AO 系数恒为 1），4 是上限。
    let c = ctx.clone();
    engine.register_fn("set_ssao_intensity", move |intensity: f64| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.ssao.intensity = f32_of(intensity);
        s.postfx = Some(fx.sanitized());
    });

    // 单独调样本数：会被夹进 [4, 16]，取偶数更好对齐。
    let c = ctx.clone();
    engine.register_fn("set_ssao_samples", move |samples: i64| {
        let mut s = c.scene.borrow_mut();
        let mut fx = s.postfx.unwrap_or_default();
        fx.ssao.samples = ssao_samples_of(samples);
        s.postfx = Some(fx.sanitized());
    });

    // 关掉遮蔽但**保留**后处理段里的曝光 / 泛光 / FXAA：与 `clear_postfx` 的区别
    // 就在于只掐这一条支线，不把整段打回直通。
    let c = ctx.clone();
    engine.register_fn("clear_ssao", move || {
        let mut s = c.scene.borrow_mut();
        if let Some(mut fx) = s.postfx {
            fx.ssao.enabled = false;
            s.postfx = Some(fx);
        }
    });

    // 只有「段存在且开了且强度非零」才算真的在算遮蔽——强度 0 时 GPU 那边整帧跳过。
    let c = ctx.clone();
    engine.register_fn("has_ssao", move || -> bool {
        c.scene
            .borrow()
            .postfx
            .map(|fx| !fx.ssao.is_neutral())
            .unwrap_or(false)
    });

    // 当前生效值快照（脚本侧断言 / 编辑器回读用），未开启时给出缺省值。
    let c = ctx.clone();
    engine.register_fn("ssao_enabled", move || -> bool {
        c.scene
            .borrow()
            .postfx
            .map(|fx| fx.ssao.enabled)
            .unwrap_or(false)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_radius", move || -> f64 {
        f64::from(c.scene.borrow().postfx.unwrap_or_default().ssao.radius)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_bias", move || -> f64 {
        f64::from(c.scene.borrow().postfx.unwrap_or_default().ssao.bias)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_intensity", move || -> f64 {
        f64::from(c.scene.borrow().postfx.unwrap_or_default().ssao.intensity)
    });

    let c = ctx.clone();
    engine.register_fn("ssao_samples", move || -> i64 {
        i64::from(c.scene.borrow().postfx.unwrap_or_default().ssao.samples)
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
    // 扩展十六起它是「各级覆盖半宽的上限」，不是逐级固定值。
    let c = ctx.clone();
    engine.register_fn("set_shadow_extent", move |half: f64| {
        c.scene.borrow_mut().light.shadow.ortho_size = f32_of(half).max(0.0);
    });

    // 扩展十六：级联级数 1..=MAX_CASCADES（1 = 单级，等价阶段 D 的老行为）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_cascades", move |count: i64| {
        let max = r3e_core::MAX_CASCADES as i64;
        c.scene.borrow_mut().light.shadow.cascade.count = count.clamp(1, max) as u32;
    });

    // 只读查询：当前级联级数。
    let c = ctx.clone();
    engine.register_fn("shadow_cascades", move || -> i64 {
        c.scene.borrow().light.shadow.cascade.count as i64
    });

    // 级间混合带宽：占该级跨度的比例 0..1。0 = 硬切（能看到级间接缝），0.1~0.3 较自然。
    let c = ctx.clone();
    engine.register_fn("set_shadow_blend", move |blend: f64| {
        c.scene.borrow_mut().light.shadow.cascade.blend = f32_of(blend).clamp(0.0, 1.0);
    });

    // 阴影覆盖的最远视距：0 = 由场景包围球自动推断，>0 = 固定值（越近越清晰）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_distance", move |distance: f64| {
        c.scene.borrow_mut().light.shadow.cascade.max_distance = f32_of(distance).max(0.0);
    });

    // 扩展二十四：级联遮挡体向光源方向的外扩距离（世界单位）。
    // 0 = 紧贴级边界（裁剪最狠，边缘处物件可能把影子投丢）；0.5~2 通常既安全又省 draw。
    let c = ctx.clone();
    engine.register_fn("set_shadow_caster_extend", move |extend: f64| {
        let max = r3e_core::MAX_CASTER_EXTEND as f64;
        c.scene.borrow_mut().light.shadow.cascade.caster_extend =
            f32_of(extend).clamp(0.0, max as f32);
    });

    // 扩展二十四：按级裁剪开关。关掉即回到「每级都画全场景」的老行为（调试对照用）。
    let c = ctx.clone();
    engine.register_fn("set_shadow_caster_cull", move |on: bool| {
        c.scene.borrow_mut().light.shadow.cascade.caster_cull = on;
    });

    // 只读查询：当前是否按级裁剪遮挡体。
    let c = ctx.clone();
    engine.register_fn("shadow_caster_cull", move || -> bool {
        c.scene.borrow().light.shadow.cascade.caster_cull
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
    // 扩展十五：局部光源（点光 / 聚光）。
    //
    // 约定与 UI 那套一致：**写入走 `light_*`，读取走 `local_light_*`**，
    // 命中判定与数值收敛全部在 `r3e_core::light` 里，脚本层只做参数搬运，
    // 因此编辑器 RPC / 场景 JSON / 脚本三条入口的语义完全同源。
    //
    // 上限 `MAX_LOCAL_LIGHTS`：满载时只拒绝「新增」，同 key 改写照常生效
    // （脚本每帧更新一盏灯的位置不会被静默丢掉）。
    // ------------------------------------------------------------------

    // 添加 / 覆盖一盏点光：位置 + 颜色 + 强度 + 半径。
    let c = ctx.clone();
    engine.register_fn(
        "light_add_point",
        move |key: &str,
              x: f64,
              y: f64,
              z: f64,
              r: f64,
              g: f64,
              b: f64,
              intensity: f64,
              range: f64|
              -> bool {
            let mut light = LocalLight::point(
                key,
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(r), f32_of(g), f32_of(b)],
                f32_of(intensity),
            );
            light.range = f32_of(range);
            c.scene.borrow_mut().upsert_local_light(light)
        },
    );

    // 添加 / 覆盖一盏聚光：位置 + 朝向 + 颜色 + 强度 + 半径 + 内外角（度）。
    let c = ctx.clone();
    engine.register_fn(
        "light_add_spot",
        move |key: &str,
              x: f64,
              y: f64,
              z: f64,
              dx: f64,
              dy: f64,
              dz: f64,
              r: f64,
              g: f64,
              b: f64,
              intensity: f64,
              range: f64,
              inner_deg: f64,
              outer_deg: f64|
              -> bool {
            let light = LocalLight::spot(
                key,
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(dx), f32_of(dy), f32_of(dz)],
                [f32_of(r), f32_of(g), f32_of(b)],
                f32_of(intensity),
                f32_of(range),
                f32_of(inner_deg),
                f32_of(outer_deg),
            );
            c.scene.borrow_mut().upsert_local_light(light)
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_remove", move |key: &str| -> bool {
        c.scene.borrow_mut().remove_local_light(key)
    });

    // 清空全部灯，返回删掉的盏数。
    let c = ctx.clone();
    engine.register_fn("light_clear", move || -> i64 {
        c.scene.borrow_mut().clear_local_lights() as i64
    });

    // 以下「改单个字段」全部走 `with_local_light`：key 不存在返回 false，
    // 不会悄悄新建一盏灯（拼错名字是最常见的脚本错误）。
    let c = ctx.clone();
    engine.register_fn(
        "light_set_pos",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.scene.borrow_mut().with_local_light(key, |l| {
                l.pos = [f32_of(x), f32_of(y), f32_of(z)];
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "light_set_color",
        move |key: &str, r: f64, g: f64, b: f64| -> bool {
            c.scene.borrow_mut().with_local_light(key, |l| {
                l.color = [f32_of(r), f32_of(g), f32_of(b)];
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_set_intensity", move |key: &str, v: f64| -> bool {
        c.scene.borrow_mut().with_local_light(key, |l| {
            l.intensity = f32_of(v);
        })
    });

    let c = ctx.clone();
    engine.register_fn("light_set_range", move |key: &str, v: f64| -> bool {
        c.scene.borrow_mut().with_local_light(key, |l| {
            l.range = f32_of(v);
        })
    });

    let c = ctx.clone();
    engine.register_fn(
        "light_set_dir",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.scene.borrow_mut().with_local_light(key, |l| {
                l.dir = [f32_of(x), f32_of(y), f32_of(z)];
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "light_set_cone",
        move |key: &str, inner_deg: f64, outer_deg: f64| -> bool {
            c.scene.borrow_mut().with_local_light(key, |l| {
                l.inner_deg = f32_of(inner_deg);
                l.outer_deg = f32_of(outer_deg);
            })
        },
    );

    // 扩展十八 C：投影开关与深度偏移。
    //
    // 「会不会真的投影」由渲染层的 `plan_local_shadows` 决定（全世界最多
    // `MAX_LOCAL_SHADOW_CASTERS` 盏、按亮度排序），这里的 `on` 只是**允许**它参与
    // 竞争——所以 `local_light_cast_shadow` 读回来是「意愿」，不是「本帧实际在投影」；
    // 想查本帧有几盏真的在投影，用只读的 `local_shadow_caster_count`。
    let c = ctx.clone();
    engine.register_fn(
        "light_set_cast_shadow",
        move |key: &str, on: bool| -> bool {
            c.scene.borrow_mut().with_local_light(key, |l| {
                l.cast_shadow = on;
            })
        },
    );

    let c = ctx.clone();
    engine.register_fn("light_set_shadow_bias", move |key: &str, v: f64| -> bool {
        c.scene.borrow_mut().with_local_light(key, |l| {
            l.shadow_bias = f32_of(v);
        })
    });

    // 总开关：关掉只是不上传（场景数据保留），打开即整体恢复。
    let c = ctx.clone();
    engine.register_fn("set_lights_enabled", move |on: bool| {
        c.scene.borrow_mut().lights_enabled = on;
    });

    let c = ctx.clone();
    engine.register_fn("lights_enabled", move || -> bool {
        c.scene.borrow().lights_enabled
    });

    // ---- 遮挡剔除（扩展二十二） ----
    // 总开关只决定「这一帧要不要做遮挡判定」：关掉后渲染层不读回深度、不建金字塔，
    // 提交序列退化成只有视锥剔除；参数与场景数据都保留，再打开即按原参数恢复。
    let c = ctx.clone();
    engine.register_fn("set_occlusion_enabled", move |on: bool| {
        c.scene.borrow_mut().set_occlusion_enabled(on);
    });

    let c = ctx.clone();
    engine.register_fn("occlusion_enabled", move || -> bool {
        c.scene.borrow().occlusion_enabled()
    });

    // 深度偏置（NDC 单位）：偏置越大越保守（越不剔），写入前统一收敛到合法区间。
    let c = ctx.clone();
    engine.register_fn("set_occlusion_bias", move |v: f64| -> f64 {
        let mut scene = c.scene.borrow_mut();
        let mut s = scene.occlusion;
        s.depth_bias = f32_of(v);
        scene.set_occlusion(s).depth_bias as f64
    });

    let c = ctx.clone();
    engine.register_fn("occlusion_bias", move || -> f64 {
        c.scene.borrow().occlusion.depth_bias as f64
    });

    // ---- 只读查询 ----

    let c = ctx.clone();
    engine.register_fn("local_light_count", move || -> i64 {
        c.scene.borrow().lights.len() as i64
    });

    let c = ctx.clone();
    engine.register_fn("local_light_keys", move || -> Array {
        c.scene
            .borrow()
            .lights
            .iter()
            .map(|l| Dynamic::from(l.key.clone()))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("local_light_exists", move |key: &str| -> bool {
        c.scene.borrow().local_light(key).is_some()
    });

    // 一盏灯的完整快照：
    // `#{ key, kind, pos, color, intensity, range, dir, inner, outer, cast_shadow, shadow_bias }`。
    // 键不存在返回空 map（`map.is_empty()` 即可判空，不必先 exists）。
    let c = ctx.clone();
    engine.register_fn("local_light", move |key: &str| -> Map {
        let scene = c.scene.borrow();
        let Some(l) = scene.local_light(key) else {
            return Map::new();
        };
        let mut m = Map::new();
        m.insert("key".into(), Dynamic::from(l.key.clone()));
        m.insert("kind".into(), Dynamic::from(l.kind.name().to_string()));
        m.insert(
            "pos".into(),
            Dynamic::from(
                l.pos
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert(
            "color".into(),
            Dynamic::from(
                l.color
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert("intensity".into(), Dynamic::from(l.intensity as f64));
        m.insert("range".into(), Dynamic::from(l.range as f64));
        m.insert(
            "dir".into(),
            Dynamic::from(
                l.dir
                    .iter()
                    .map(|v| Dynamic::from(*v as f64))
                    .collect::<Array>(),
            ),
        );
        m.insert("inner".into(), Dynamic::from(l.inner_deg as f64));
        m.insert("outer".into(), Dynamic::from(l.outer_deg as f64));
        m.insert("cast_shadow".into(), Dynamic::from(l.cast_shadow));
        m.insert("shadow_bias".into(), Dynamic::from(l.shadow_bias as f64));
        m
    });

    // 单字段便捷读取（HUD 用）：不存在时返回 0 / 空串，也能当「没有这盏灯」的信号。
    let c = ctx.clone();
    engine.register_fn("local_light_intensity", move |key: &str| -> f64 {
        c.scene
            .borrow()
            .local_light(key)
            .map(|l| l.intensity as f64)
            .unwrap_or(0.0)
    });

    let c = ctx.clone();
    engine.register_fn("local_light_kind", move |key: &str| -> String {
        c.scene
            .borrow()
            .local_light(key)
            .map(|l| l.kind.name().to_string())
            .unwrap_or_default()
    });

    // 投影意愿（场景里 `cast_shadow` 的原值），与「本帧实际投影」是两件事。
    let c = ctx.clone();
    engine.register_fn("local_light_cast_shadow", move |key: &str| -> bool {
        c.scene
            .borrow()
            .local_light(key)
            .map(|l| l.cast_shadow)
            .unwrap_or(false)
    });

    // 本帧真正参与投影的灯数（= `plan_local_shadows` 选中的盏数）。
    // 关掉局部光源时不投影，直接 0——脚本做「阴影预算是否吃满」的 HUD 很省事。
    let c = ctx.clone();
    engine.register_fn("local_shadow_caster_count", move || -> i64 {
        let scene = c.scene.borrow();
        if !scene.lights_enabled {
            return 0;
        }
        plan_local_shadows(&scene.lights).len() as i64
    });

    // 某点受到的全部局部光照度（与 GPU 同式的 CPU 参考实现）：
    // 脚本可以用它做「站在灯下才回血」这类玩法，不必自己重写衰减公式。
    let c = ctx.clone();
    engine.register_fn(
        "local_light_irradiance",
        move |x: f64, y: f64, z: f64| -> f64 {
            let scene = c.scene.borrow();
            if !scene.lights_enabled {
                return 0.0;
            }
            let p = Vec3::new(f32_of(x), f32_of(y), f32_of(z));
            scene
                .lights
                .iter()
                // `irradiance` 返回线性 RGB，取最大分量当「亮度」标量即可
                .map(|l| l.irradiance(p).max_element().max(0.0) as f64)
                .sum()
        },
    );

    // ------------------------------------------------------------------
    // UI 指针交互（只读）。
    //
    // 事件由 `app` 每帧根据真实指针输入推进（`r3e_core::ui::UiInteraction`），
    // 脚本只负责读——这样命中判定只有 core 一份实现，渲染、脚本、编辑器不会各判各的。
    //
    // 事件语义：
    // * `hovered` / `unhovered`：指针进出元素；
    // * `pressed`：在元素上按下；
    // * `clicked`：在同一元素上按下并抬起（`released` 之前）；
    // * `cancelled`：按下了但没在同一元素上抬起（移出后释放 / 元素被删 / 丢事件）；
    // * `released`：有按下捕获时抬起**总会**收到一条。
    // ------------------------------------------------------------------

    // 本帧事件列表：`[#{ kind: "clicked", key: "btn" }, ...]`，顺序即发生顺序。
    let c = ctx.clone();
    engine.register_fn("ui_events", move || -> Array {
        c.ui_interaction
            .borrow()
            .events()
            .iter()
            .map(|ev| {
                let mut m = Map::new();
                m.insert("kind".into(), Dynamic::from(ev.kind.name().to_string()));
                m.insert("key".into(), Dynamic::from(ev.key.clone()));
                Dynamic::from(m)
            })
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("ui_event_count", move || -> i64 {
        c.ui_interaction.borrow().events().len() as i64
    });

    // 当前悬停 / 按住不放的元素 key（空串 = 无）。
    let c = ctx.clone();
    engine.register_fn("ui_hover", move || -> String {
        c.ui_interaction.borrow().hover().unwrap_or("").to_string()
    });

    let c = ctx.clone();
    engine.register_fn("ui_active", move || -> String {
        c.ui_interaction.borrow().active().unwrap_or("").to_string()
    });

    // 便利判定：省掉脚本里遍历事件数组的样板代码。
    let c = ctx.clone();
    engine.register_fn("ui_clicked", move |key: &str| -> bool {
        c.ui_interaction.borrow().clicked(key)
    });

    let c = ctx.clone();
    engine.register_fn("ui_hovered", move |key: &str| -> bool {
        c.ui_interaction.borrow().hovered(key)
    });

    // 指针是否正压在 UI 上（脚本可据此屏蔽自己的「点空白处」逻辑）。
    let c = ctx.clone();
    engine.register_fn("ui_pointer_over_ui", move || -> bool {
        c.ui_interaction.borrow().pointer_over_ui()
    });

    // 屏幕坐标命中测试：返回最上层元素的 key（空串 = 没命中）。
    // 用的是最近一帧 `app` 送进来的屏幕尺寸，所以和实际渲染的矩形完全一致。
    let c = ctx.clone();
    engine.register_fn("ui_hit", move |x: f64, y: f64| -> String {
        let ui = c.ui_interaction.borrow();
        let scene = c.scene.borrow();
        hit_test_key(&scene.ui, ui.screen(), f32_of(x), f32_of(y))
            .map(str::to_string)
            .unwrap_or_default()
    });

    // ---------- 3D 屏幕射线拾取（扩展三十四） ----------
    //
    // 这三个入口是「脚本自己就能做交互」的关键：以前点选只能靠 `raycast`（要挂刚体）或
    // 手算投影矩阵，现在屏幕坐标直接换成物件。屏幕尺寸取 `app` 最近一帧送进
    // `UiInteraction` 的那份，所以和画面完全对齐，不是猜的窗口大小。

    // 脚本要自己做像素级交互（拾取 / 自绘 HUD / 判边界）就得知道画布多大。
    // 这三个读的与 `pick_*` 是同一份尺寸，运行中改窗口大小会跟着变。
    let c = ctx.clone();
    engine.register_fn("screen_width", move || -> f64 {
        c.ui_interaction.borrow().screen().0 as f64
    });
    let c = ctx.clone();
    engine.register_fn("screen_height", move || -> f64 {
        c.ui_interaction.borrow().screen().1 as f64
    });
    let c = ctx.clone();
    engine.register_fn("screen_size", move || -> Array {
        let (w, h) = c.ui_interaction.borrow().screen();
        vec![Dynamic::from(w as f64), Dynamic::from(h as f64)]
    });

    // 屏幕像素 → 最近命中的可见物件（渲染几何 AABB，不要求刚体）。
    let c = ctx.clone();
    engine.register_fn("pick_at", move |x: f64, y: f64| -> Map {
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hit = pick_from_screen(&scene, &provider, f32_of(x), f32_of(y), screen.0, screen.1);
        pick_map(&scene, hit)
    });

    // 鼠标当前位置的同一件事（少写两次 `mouse_x()`）。
    let c = ctx.clone();
    engine.register_fn("pick_under_mouse", move || -> Map {
        let (mx, my) = {
            let input = c.input.borrow();
            (input.mouse_x, input.mouse_y)
        };
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hit = pick_from_screen(&scene, &provider, mx, my, screen.0, screen.1);
        pick_map(&scene, hit)
    });

    // 同一条射线上**所有**命中的物件，按距离升序（穿透式点选 / 处理遮挡里的目标）。
    // 每项字段与 `pick_at` 完全一致，脚本用同一套取值代码即可。
    let c = ctx.clone();
    engine.register_fn("pick_all_at", move |x: f64, y: f64| -> Array {
        let screen = c.ui_interaction.borrow().screen();
        let scene = c.scene.borrow();
        let bounds = c.pick_bounds.borrow();
        let provider = PickBounds::new(&bounds);
        let hits =
            pick_all_from_screen(&scene, &provider, f32_of(x), f32_of(y), screen.0, screen.1);
        hits.into_iter()
            .map(|hit| Dynamic::from(pick_map(&scene, Some(hit))))
            .collect()
    });

    // 本帧的 UI 层级顺序里，某个 key 是否存在（脚本常用来先判存在再读事件）。
    let c = ctx.clone();
    engine.register_fn("ui_event_kind_name", move |i: i64| -> String {
        let ui = c.ui_interaction.borrow();
        let idx = i.max(0) as usize;
        ui.events()
            .get(idx)
            .map(|e| e.kind.name().to_string())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn("ui_event_key", move |i: i64| -> String {
        let ui = c.ui_interaction.borrow();
        let idx = i.max(0) as usize;
        ui.events()
            .get(idx)
            .map(|e| e.key.clone())
            .unwrap_or_default()
    });

    // 事件名的稳定取值（脚本里常写 `if k == ui_kind_clicked()` 之类的判断）。
    engine.register_fn("ui_kind_clicked", || -> String {
        UiEventKind::Clicked.name().to_string()
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

    // ------------------------------------------------------------------
    // 音频（3D 空间化，§12 第十项）
    //
    // 与上面 21 个非空间化 API 的关系是**纯叠加**：不调 `audio_spatial_attach*`
    // 的声音就是平面声，音量与历史版本逐位一致；调了之后覆盖层才给它乘距离衰减、
    // 把多普勒算出来下发成播放倍速。
    //
    // 约定：
    // * key 先 `audio_spatial_attach*` 再 `audio_play*`（注册前那一帧还是平面声）；
    // * 位置可由 `audio_spatial_set_position` 直接给，也可 `audio_spatial_attach_node`
    //   绑到场景节点，由 `app` 每帧解析节点世界坐标后灌进来；
    // * 听者默认在原点朝 -Z；`app` 每帧按**活动相机**覆盖，脚本一般不用手动设；
    // * 全程不 panic：参数非法一律收敛（NaN 坐标退原点、min>=max 自动拉开），
    //   无设备时多普勒只算不下发。
    // ------------------------------------------------------------------

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach",
        move |key: &str, min_distance: f64, max_distance: f64| -> bool {
            c.audio.borrow_mut().attach_spatial(
                key,
                SpatialSettings::new(f32_of(min_distance), f32_of(max_distance)),
            )
        },
    );

    // 带模型与多普勒强度的完整版。model 取 inverse / linear / exponential
    //（大小写不敏感，也接受 inv / lin / exp），认不出来就回退 inverse。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach_ex",
        move |key: &str,
              min_distance: f64,
              max_distance: f64,
              model: &str,
              doppler_factor: f64|
              -> bool {
            let mut settings = SpatialSettings::new(f32_of(min_distance), f32_of(max_distance));
            settings.model = DistanceModel::parse(model).unwrap_or_default();
            settings.doppler_factor = f32_of(doppler_factor);
            c.audio.borrow_mut().attach_spatial(key, settings)
        },
    );

    // 注册并绑到场景节点：位置由 app 每帧从节点世界变换取，脚本只管调参。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_attach_node",
        move |key: &str, node: &str, min_distance: f64, max_distance: f64| -> bool {
            c.audio.borrow_mut().attach_spatial_node(
                key,
                node,
                SpatialSettings::new(f32_of(min_distance), f32_of(max_distance)),
            )
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_detach", move |key: &str| -> bool {
        c.audio.borrow_mut().detach_spatial(key)
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_clear", move || {
        c.audio.borrow_mut().clear_spatial();
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_count", move || -> i64 {
        c.audio.borrow().spatial_count() as i64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_keys", move || -> Array {
        c.audio
            .borrow()
            .spatial_keys()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_position",
        move |key: &str, x: f64, y: f64, z: f64| -> bool {
            c.audio
                .borrow_mut()
                .set_source_position(key, [f32_of(x), f32_of(y), f32_of(z)])
        },
    );

    // 声源世界坐标 [x, y, z]；未注册返回空数组（脚本可据此判存在）。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_position", move |key: &str| -> Array {
        match c.audio.borrow().source_position(key) {
            Some(p) => p.iter().map(|v| Dynamic::from(*v as f64)).collect(),
            None => Array::new(),
        }
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_model",
        move |key: &str, model: &str| -> bool {
            let Some(model) = DistanceModel::parse(model) else {
                return false;
            };
            let audio = c.audio.borrow();
            let Some(mut settings) = audio.spatial_settings(key) else {
                return false;
            };
            drop(audio);
            settings.model = model;
            c.audio.borrow_mut().set_spatial_settings(key, settings)
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_model", move |key: &str| -> String {
        c.audio
            .borrow()
            .spatial_settings(key)
            .map(|s| s.model.name().to_string())
            .unwrap_or_default()
    });

    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_doppler",
        move |key: &str, factor: f64| -> bool {
            let audio = c.audio.borrow();
            let Some(mut settings) = audio.spatial_settings(key) else {
                return false;
            };
            drop(audio);
            settings.doppler_factor = f32_of(factor);
            c.audio.borrow_mut().set_spatial_settings(key, settings)
        },
    );

    let c = ctx.clone();
    engine.register_fn("audio_spatial_doppler", move |key: &str| -> f64 {
        c.audio
            .borrow()
            .spatial_settings(key)
            .map(|s| s.doppler_factor as f64)
            .unwrap_or(1.0)
    });

    // 探针三件套：当前增益 / 倍速 / 距离（未注册时增益倍速为 1.0、距离为 -1）。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_gain", move |key: &str| -> f64 {
        c.audio.borrow().spatial_gain(key) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_rate", move |key: &str| -> f64 {
        c.audio.borrow().spatial_rate(key) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_distance", move |key: &str| -> f64 {
        c.audio.borrow().spatial_distance(key).unwrap_or(-1.0) as f64
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_node", move |key: &str| -> String {
        c.audio
            .borrow()
            .spatial_node(key)
            .map(str::to_string)
            .unwrap_or_default()
    });

    // 手动喂听者位姿：position(3) + forward(3)。forward 传零向量会被退化成正朝 -Z。
    let c = ctx.clone();
    engine.register_fn(
        "audio_spatial_set_listener",
        move |x: f64, y: f64, z: f64, fx: f64, fy: f64, fz: f64| {
            let pose = r3e_audio::ListenerPose::new(
                [f32_of(x), f32_of(y), f32_of(z)],
                [f32_of(fx), f32_of(fy), f32_of(fz)],
            );
            c.audio.borrow_mut().set_listener(pose);
        },
    );

    // 听者位姿 [px, py, pz, fx, fy, fz]。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_listener", move || -> Array {
        let pose = c.audio.borrow().listener();
        pose.position
            .iter()
            .chain(pose.forward.iter())
            .map(|v| Dynamic::from(*v as f64))
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_set_enabled", move |on: bool| {
        c.audio.borrow_mut().set_spatial_enabled(on);
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_enabled", move || -> bool {
        c.audio.borrow().spatial_enabled()
    });

    // 手动推进一帧空间化（返回推进的声源数）。app 每帧自动调，脚本一般不需要；
    // 留着是为了无窗口测试与"脚本自己控制时间流速"的玩法。
    let c = ctx.clone();
    engine.register_fn("audio_spatial_tick", move |dt: f64| -> i64 {
        c.audio.borrow_mut().tick_spatial(f32_of(dt)) as i64
    });

    // 探针：后端能否实时变速（多普勒落地前提）。静默降级时 false。
    let c = ctx.clone();
    engine.register_fn("audio_supports_speed", move || -> bool {
        c.audio.borrow().supports_speed()
    });

    let c = ctx.clone();
    engine.register_fn("audio_spatial_status", move || -> String {
        c.audio.borrow().spatial_status_line()
    });

    // ---------- 场景切换与存档（扩展十八 B） ----------
    //
    // 这一组全是"申请"而不是"执行"：宿主在帧首统一处理命令，脚本再怎么调也
    // 不会在 `on_start` / `on_update` 中途把世界换掉。
    let c = ctx.clone();
    engine.register_fn("scene_switch", move |path: &str| -> bool {
        match non_empty_path(path) {
            Some(p) => queue_scene_cmd(&c, SceneCommand::switch(p)),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_back", move || -> bool {
        queue_scene_cmd(&c, SceneCommand::back())
    });

    let c = ctx.clone();
    engine.register_fn("scene_save_archive", move |path: &str| -> bool {
        // 空串 = 交给宿主挑默认位置（与 RPC `scene.archive.save` 省略 path 同义）。
        queue_scene_cmd(&c, SceneCommand::save_archive(non_empty_path(path)))
    });

    let c = ctx.clone();
    engine.register_fn("scene_load_archive", move |path: &str| -> bool {
        match non_empty_path(path) {
            Some(p) => queue_scene_cmd(&c, SceneCommand::load_archive(p)),
            None => false,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_command_pending", move || -> bool {
        c.scene_cmd.borrow().is_some()
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_names", move || -> Array {
        c.scene_stack
            .borrow()
            .names()
            .into_iter()
            .map(Dynamic::from)
            .collect()
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_depth", move || -> i64 {
        c.scene_stack.borrow().depth() as i64
    });

    // 游标位置：空栈返回 -1（而不是 0），"没有当前层"与"第 0 层"必须能区分。
    let c = ctx.clone();
    engine.register_fn("scene_stack_index", move || -> i64 {
        match c.scene_stack.borrow().current_index() {
            Some(i) => i as i64,
            None => -1,
        }
    });

    let c = ctx.clone();
    engine.register_fn("scene_stack_current", move || -> String {
        c.scene_stack
            .borrow()
            .current()
            .map(|e| e.name.clone())
            .unwrap_or_default()
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
