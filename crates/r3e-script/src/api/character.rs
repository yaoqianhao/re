//! 角色控制器（M2-B01）：`character.*` 六条命令的脚本侧绑定。
//!
//! 与 [`super::physics`] 同一分工：本文件只做「脚本参数 → 场景配置 / 能力面诉求」
//! 的搬运与形状校验，一行求解逻辑都不写——决策全在 `r3e-character`，几何全在
//! `r3e-physics::port`。`r3e-script` 因此**不需要**新增对 `r3e-character` 的依赖边
//! （卡表明写的硬约束），脚本唯一的入口就是这六个函数。
//!
//! 两条脚本侧口径（与 RPC 侧刻意不同，改之前先想清楚）：
//!
//! 1. **配置写场景、运行态不写**：`character_create` / `character_config` 写的是
//!    `Node::character`（可差分、进 `StateHash`）；`step` / `state` / `teleport`
//!    只动宿主侧控制器。这是回放金标 `385482eb033d2964` 不漂移的前提。
//! 2. **create / config 同签名、全量覆盖**：RPC 侧的 `character.config` 支持「只改给
//!    了的字段」，脚本侧做不到「没给」这种表达（Rhai 没有可选参数），于是统一全量写。
//!    要「只改一项」就先 `character_state` 读出运行态、再按脚本里记着的配置写回——
//!    与其发明一套「传 -1 表示不改」的隐式约定，不如让脚本显式写全。

use super::*;
use r3e_core::{CharacterSpec, CharacterSpecError};

/// 六个几何参数 → 校验过的 [`CharacterSpec`]。
///
/// 顺序固定为「半径 / 身高 / 坡度上限 / 台阶高度 / 吸附距离」，与命令面一致；
/// 越界一律 `Err`，不夹边界——理由见 [`CharacterSpec::validate`]。
fn spec_of(
    radius: f64,
    height: f64,
    slope_limit_deg: f64,
    step_height: f64,
    snap_distance: f64,
) -> Result<CharacterSpec, CharacterSpecError> {
    let spec = CharacterSpec {
        radius: radius as f32,
        height: height as f32,
        slope_limit_deg: slope_limit_deg as f32,
        step_height: step_height as f32,
        snap_distance: snap_distance as f32,
    };
    spec.validated()
}

/// 把配置写进节点的 `character` 段；节点不存在返回 `false`。
fn write_spec(ctx: &ScriptContext, name: &str, spec: CharacterSpec) -> bool {
    ctx.write(|scene| match scene.find_mut(name) {
        Some(node) => {
            node.character = Some(spec);
            true
        }
        None => false,
    })
}

/// 让宿主按场景里的配置建/刷新控制器；宿主没接角色实现时打印提示。
///
/// 返回值刻意**不**与 [`write_spec`] 合并：配置写成功、宿主没接，是「配置生效了但
/// 走不动」——这种半成功必须让脚本看得见（打印 + `character_stats` 的 `backend`
/// 为 `"none"`），而不是悄悄改成 `false` 让人以为参数写错了。
fn bind_host(ctx: &ScriptContext, name: &str) {
    let scene = ctx.doc.borrow();
    let Some(node) = scene.find(name) else {
        return;
    };
    if !ctx.character.bind(node) && ctx.character.backend() == "none" {
        println!("[script] character: 角色后端未接入（backend=none），配置已写入但控制器不会推进");
    }
}

/// 运行态 → 脚本 Map；`None` 时字段齐备（`found = false`），理由同 [`empty_query_map`]。
fn state_map(name: &str, st: Option<CharacterStateDto>) -> Map {
    let mut m = Map::new();
    m.insert("node".into(), Dynamic::from(name.to_string()));
    let (found, position, vy, grounded, blocked) = match st {
        Some(s) => (
            true,
            f64_array(&s.position),
            s.vertical_speed,
            s.grounded,
            s.blocked,
        ),
        None => (false, Array::new(), 0.0, false, false),
    };
    m.insert("found".into(), Dynamic::from(found));
    m.insert("position".into(), Dynamic::from(position));
    m.insert("vertical_speed".into(), Dynamic::from(vy));
    m.insert("grounded".into(), Dynamic::from(grounded));
    m.insert("blocked".into(), Dynamic::from(blocked));
    m
}

/// 注册「角色控制器」域的脚本函数（六条，与命令面一一对应）。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    // 建控制器：先校验 → 写场景 → 让宿主建。返回「配置是否已落场景」。
    let c = ctx.clone();
    engine.register_fn(
        "character_create",
        move |name: &str,
              radius: f64,
              height: f64,
              slope_limit_deg: f64,
              step_height: f64,
              snap_distance: f64|
              -> bool {
            let spec = match spec_of(radius, height, slope_limit_deg, step_height, snap_distance) {
                Ok(s) => s,
                Err(e) => {
                    println!("[script] character_create `{name}`: 配置非法（{e}）");
                    return false;
                }
            };
            if !write_spec(&c, name, spec) {
                println!("[script] character_create: 物件 `{name}` 不存在");
                return false;
            }
            bind_host(&c, name);
            true
        },
    );

    // 改配置：全量覆盖（脚本侧口径，见文件头第 2 条）。
    let c = ctx.clone();
    engine.register_fn(
        "character_config",
        move |name: &str,
              radius: f64,
              height: f64,
              slope_limit_deg: f64,
              step_height: f64,
              snap_distance: f64|
              -> bool {
            let spec = match spec_of(radius, height, slope_limit_deg, step_height, snap_distance) {
                Ok(s) => s,
                Err(e) => {
                    println!("[script] character_config `{name}`: 配置非法（{e}），保持原值");
                    return false;
                }
            };
            if !write_spec(&c, name, spec) {
                println!("[script] character_config: 物件 `{name}` 不存在");
                return false;
            }
            bind_host(&c, name);
            true
        },
    );

    // 推进一帧：返回本帧结果（不是缓存）。`dt` 非正数直接拒——`dt = 0` 会让求解器
    // 静默不动，「脚本以为在走、画面纹丝不动」是最难查的一类 bug。
    let c = ctx.clone();
    engine.register_fn(
        "character_step",
        move |name: &str, wish_x: f64, wish_z: f64, dt: f64| -> Map {
            if dt <= 0.0 || !dt.is_finite() {
                println!("[script] character_step: dt 必须为正数，收到 {dt}");
                return state_map(name, None);
            }
            let scene = c.doc.borrow();
            let Some(node) = scene.find(name) else {
                println!("[script] character_step: 物件 `{name}` 不存在");
                return state_map(name, None);
            };
            let stepped = c.character.step(node, [wish_x, wish_z], false, dt);
            if stepped.is_none() {
                println!("[script] character_step `{name}`: 没有控制器，先 character_create");
            }
            state_map(name, stepped)
        },
    );

    // 读运行态（只读，不推进）。
    let c = ctx.clone();
    engine.register_fn("character_state", move |name: &str| -> Map {
        state_map(name, c.character.state(name))
    });

    // 直接落位：脚底坐标。落位后 `grounded = false`，下一步由求解器裁决。
    let c = ctx.clone();
    engine.register_fn(
        "character_teleport",
        move |name: &str, x: f64, y: f64, z: f64| -> bool {
            if !c.character.teleport(name, [x, y, z]) {
                println!("[script] character_teleport `{name}`: 没有控制器");
                return false;
            }
            true
        },
    );

    // 累计统计 + 后端名。`backend` 必须如实带出：统计全是 0 时，
    // 「角色没动过」和「压根没接」是两件完全不同的事。
    let c = ctx.clone();
    engine.register_fn("character_stats", move || -> Map {
        let s = c.character.stats();
        let mut m = Map::new();
        m.insert("backend".into(), Dynamic::from(c.character.backend()));
        m.insert("controllers".into(), Dynamic::from(s.controllers as i64));
        m.insert("steps".into(), Dynamic::from(s.steps as i64));
        m.insert("blocked".into(), Dynamic::from(s.blocked as i64));
        m.insert("step_ups".into(), Dynamic::from(s.step_ups as i64));
        m
    });
}
