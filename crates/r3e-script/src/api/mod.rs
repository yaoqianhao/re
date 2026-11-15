//! 脚本可调用的引擎 API 绑定。
//!
//! 全部函数都通过 `Rc<RefCell<..>>` 访问场景 / 物理 / 时钟，
//! 因此脚本执行与主循环渲染共享同一份真源数据，改动下一帧即生效。
//!
//! **M0-S2 拆分**：原 4070 行单体 `api.rs` 按功能域拆为 `api/` 子模块；
//! 本文件只保留共享类型（`ScriptContext` / `Clock` / `UiWriteLog`）、内部工具函数
//! 与 `bind_all` 薄壳，不承载具体绑定实现。公开路径不变、行为零改动。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glam::{Vec3, Vec4};
use r3e_audio::{Audio, DistanceModel, SpatialSettings};
use r3e_core::input::{parse_key, parse_mouse_button, parse_pad_axis, parse_pad_button};
use r3e_core::postfx::TonemapMode;
use r3e_doc::Document;

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

impl ScriptContext {
    /// 唯一的场景写入口：脚本 API 的所有写操作都必须经这里。
    ///
    /// 等价于 `doc.borrow_mut().write(..)`；集中成一处是为了让 api 层各域写法一致，
    /// 也让 doc-gate 的「禁直接 borrow_mut 拿 &mut Scene」只有这一个合法出口。
    pub(crate) fn write<R>(&self, f: impl FnOnce(&mut Scene) -> R) -> R {
        self.doc.borrow_mut().write(f)
    }
}

/// 脚本侧共享上下文。
#[derive(Clone)]
pub struct ScriptContext {
    /// 场景唯一真值：`r3e-doc` 的 Document。只读走 `doc.borrow()`（Deref 到 Scene），
    /// 写入一律走 [`ScriptContext::write`]，不允许长期持有 `&mut Scene`。
    pub doc: Rc<RefCell<Document>>,
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

/// 样本数先上下夹再转 `u8`：唯一实现在 [`r3e_core::ssao::clamp_samples`]（M0-S2 收口）。
fn ssao_samples_of(v: i64) -> u8 {
    r3e_core::ssao::clamp_samples(v)
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
fn edit(doc: &Rc<RefCell<Document>>, name: &str, f: impl FnOnce(&mut r3e_core::Node)) -> bool {
    doc.borrow_mut().write(|s| match s.find_mut(name) {
        Some(node) => {
            f(node);
            true
        }
        None => false,
    })
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

mod anim_blend;
mod anim_morph;
mod anim_sync;
mod audio;
mod basic;
mod env;
mod input;
mod light;
mod physics;
mod pick;
mod postfx;
mod query;
mod scene_flow;
mod shadow;
mod text;
mod ui;

/// 把全部脚本 API 注册到 `engine` 上。
///
/// 各域实现在同目录子模块的 `register`；本函数只做顺序调度（拆分前是 3676 行单体）。
pub fn bind_all(engine: &mut Engine, ctx: &ScriptContext) {
    engine.set_max_operations(50_000_000);
    basic::register(engine, ctx);
    anim_morph::register(engine, ctx);
    anim_blend::register(engine, ctx);
    anim_sync::register(engine, ctx);
    env::register(engine, ctx);
    postfx::register(engine, ctx);
    shadow::register(engine, ctx);
    input::register(engine, ctx);
    physics::register(engine, ctx);
    text::register(engine, ctx);
    ui::register(engine, ctx);
    light::register(engine, ctx);
    query::register(engine, ctx);
    pick::register(engine, ctx);
    audio::register(engine, ctx);
    scene_flow::register(engine, ctx);
}

/// 解析带单位的 UI 数值："24px" / "50%" / "12"（默认像素）。
/// 唯一实现在 [`r3e_core::ui_text::parse_ui_value`]（M0-S2 收口）。
fn parse_ui_value(text: &str) -> UiValue {
    r3e_core::ui_text::parse_ui_value(text)
}

/// 按 key 修改场景里的一条 UI 元素；找不到返回 false。
fn with_ui(ctx: &ScriptContext, key: &str, edit: impl FnOnce(&mut UiElement)) -> bool {
    ctx.write(|scene| match find_mut(&mut scene.ui, key) {
        Some(el) => {
            edit(el);
            true
        }
        None => false,
    })
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
    // 落盘与记账分两步：闭包内不能同时持有 ui_log 的借用。
    let upsert = ctx.write(|scene| upsert_styled(&mut scene.ui, el));
    match upsert {
        UiUpsert::Inserted => true,
        UiUpsert::Replaced => false,
        UiUpsert::Rejected => {
            let msg = format!("UI 元素数量已达上限 {MAX_UI_ELEMENTS}，key `{key}` 未写入");
            println!("[script] {msg}");
            ctx.ui_log.borrow_mut().record(msg);
            false
        }
    }
}

/// 按 key 修改场景里的一条文本；找不到返回 false（脚本可据此判断要不要 `text_add`）。
fn with_text(ctx: &ScriptContext, key: &str, edit: impl FnOnce(&mut ScreenText)) -> bool {
    ctx.write(
        |scene| match scene.texts.iter_mut().find(|t| t.key == key) {
            Some(t) => {
                edit(t);
                true
            }
            None => false,
        },
    )
}

/// 轻量 JSON → ObjectDesc：避免为了脚本再引一个 serde 依赖。
fn serde_json_lite(json: &str) -> Result<ObjectDesc, String> {
    // 借助 r3e-core 的 serde 实现：包装成单物件场景再取出
    let wrapped = format!("{{\"objects\":[{json}]}}");
    let scene = r3e_doc::parse_scene(&wrapped)?;
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
