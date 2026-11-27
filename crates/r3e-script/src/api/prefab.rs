//! Prefab（M2-E04）：`prefab_*` 六条命令的脚本侧绑定。
//!
//! # 为什么这里自己实现一遍，而不是调 `r3e-runtime::prefab_ops`
//!
//! 依赖方向不允许：`r3e-runtime` 依赖 `r3e-script`（脚本要跑在运行时里），
//! 反向加边就是环。两侧共用的是 **`r3e-core` 的 `Scene` 接口**——
//! `define_prefab` / `spawn_instance` / `instance_mut` / `sync_instance_node`，
//! 所以「模板 + 覆盖 → 节点」的收敛口径只有一份，重复的只是参数校验与错误分流
//! （两侧要打的提示文字本就不同：脚本是给人看的日志，RPC 要机器可分的错误码）。
//!
//! # 两条脚本侧口径（与 RPC 侧刻意不同，改之前先想清楚）
//!
//! 1. **`prefab_spawn` 的空 `parent` = 挂场景根**：RPC 侧用 `null` 表达「没有父」，
//!    Rhai 没有「缺席参数」这回事，只能拿空串当「不指定」。空串在两侧**都不是**
//!    合法的节点名（`Scene::spawn` 会拒），所以不会与真实父名撞车。
//! 2. **失败只打印 + 返回假值**：脚本没有 `Result` 可抛，`false` / 空串 / `-1`
//!    是唯一能表达「没成」的方式；具体是「参数错」还是「状态不答应」写在 `println!`
//!    的前缀里（`参数非法` / `状态冲突`），与 RPC 侧的 `Invalid` / `Conflict` 同义。

use super::*;
use r3e_core::prefab::{
    DefineOutcome, ObjectOverride, OverrideState, PrefabInstance, PrefabTemplate,
};
use r3e_core::ERR_UNKNOWN_TEMPLATE;

/// `prefab_spawn_many` 单次生成上限，与 `r3e-runtime` 侧同名常量对齐。
///
/// 两侧各写一份常量是**故意**的：跨 crate 引一个数字要加依赖边，为一个上限不值。
/// 真要改，两处一起改（`tests/prefab.rs` 有断言盯着 256 这个值）。
const SPAWN_MANY_LIMIT: i64 = 256;

/// 参数非法：调用方改参数就有救。
fn invalid(what: &str, detail: &str) {
    println!("[script] {what}: 参数非法（{detail}）");
}

/// 状态冲突：参数没毛病，是场景当前状态不答应。
fn conflict(what: &str, detail: &str) {
    println!("[script] {what}: 状态冲突（{detail}）");
}

/// `prefab_define`：模板文件 → 模板表。`id` 传空串表示以文件里的 `id` 为准。
fn define(ctx: &ScriptContext, id: &str, path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        invalid("prefab_define", "模板路径不能为空");
        return false;
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            invalid("prefab_define", &format!("读不到模板文件 `{path}`：{err}"));
            return false;
        }
    };
    let template: PrefabTemplate = match serde_json::from_str(&raw) {
        Ok(template) => template,
        Err(err) => {
            invalid(
                "prefab_define",
                &format!("模板文件 `{path}` 解析失败：{err}"),
            );
            return false;
        }
    };
    // 调用方点了名就必须对上：放行会让「以为登记在 A、实际在 B」静默发生，
    // 之后 spawn("A") 报未注册时现场已无迹可循。
    let want = id.trim();
    if !want.is_empty() && want != template.id {
        invalid(
            "prefab_define",
            &format!(
                "模板文件里的 id 是 `{}`，与请求的 `{want}` 不一致",
                template.id
            ),
        );
        return false;
    }
    let registered = template.id.clone();
    match ctx.write(|scene| scene.define_prefab(template)) {
        Ok(outcome) => {
            let verb = if matches!(outcome, DefineOutcome::Replaced) {
                "替换"
            } else {
                "登记"
            };
            println!("[script] prefab_define: {verb}模板 `{registered}`（{path}）");
            true
        }
        Err(err) => {
            invalid(
                "prefab_define",
                &format!("模板 `{registered}` 不合格：{err}"),
            );
            false
        }
    }
}

/// 建一条实例（内部口径：父名已归一，`None` = 场景根）。
fn instantiate(
    ctx: &ScriptContext,
    id: &str,
    name: &str,
    parent: Option<&str>,
    what: &str,
) -> Result<(), String> {
    let id = id.trim();
    let name = name.trim();
    let mut instance = PrefabInstance::new(name, id);
    instance.parent = parent.map(str::to_string);
    let spawned = ctx.write(|scene| {
        // 模板不在先拦下来：`spawn_instance` 也会拒，但它的错误串要和「名字撞了」
        // 「父级不在」挤在一起分流，这里把最常见的一种单独说清楚。
        if scene.prefab_handle(id).is_none() {
            return Err(format!(
                "{ERR_UNKNOWN_TEMPLATE}：`{id}` 还没登记（先 prefab_define）"
            ));
        }
        scene.spawn_instance(instance)
    });
    match spawned {
        Ok(_) => {
            ctx.write(Scene::update_world_transforms);
            Ok(())
        }
        Err(err) => {
            conflict(what, &err);
            Err(err)
        }
    }
}

/// `prefab_spawn`：返回实例名（失败返回空串）。
fn spawn(ctx: &ScriptContext, id: &str, name: &str, parent: &str) -> String {
    if id.trim().is_empty() {
        invalid("prefab_spawn", "模板 ID 不能为空");
        return String::new();
    }
    let name = name.trim();
    if name.is_empty() {
        invalid("prefab_spawn", "实例名不能为空");
        return String::new();
    }
    let parent = Some(parent.trim()).filter(|value| !value.is_empty());
    match instantiate(ctx, id, name, parent, "prefab_spawn") {
        Ok(()) => name.to_string(),
        Err(_) => String::new(),
    }
}

/// `prefab_spawn_many`：按 `<prefix><序号>` 批量实例化，序号从 1 起。
///
/// 中途失败**回滚已建节点**：留一半实例在场景里，脚本拿到的既不是成功也不是
/// 干净的失败，比整体失败更难收拾。
fn spawn_many(ctx: &ScriptContext, id: &str, prefix: &str, count: i64) -> i64 {
    let prefix = prefix.trim();
    if !(1..=SPAWN_MANY_LIMIT).contains(&count) {
        invalid(
            "prefab_spawn_many",
            &format!("count 必须在 1..={SPAWN_MANY_LIMIT} 之间（收到 {count}），不静默截断"),
        );
        return 0;
    }
    if prefix.is_empty() {
        invalid("prefab_spawn_many", "prefix 不能为空");
        return 0;
    }
    let mut created: Vec<String> = Vec::with_capacity(count as usize);
    for index in 1..=count {
        let name = format!("{prefix}{index}");
        if instantiate(ctx, id, &name, None, "prefab_spawn_many").is_err() {
            ctx.write(|scene| {
                for done in created.iter().rev() {
                    scene.despawn(done);
                }
            });
            println!(
                "[script] prefab_spawn_many: 第 {index} 条失败，已回滚前 {} 条",
                created.len()
            );
            return 0;
        }
        created.push(name);
    }
    created.len() as i64
}

/// `prefab_set_override`：写一个覆盖字段并重算节点。
fn set_override(ctx: &ScriptContext, name: &str, field: &str, value: Dynamic) -> bool {
    if !ObjectOverride::FIELD_NAMES.contains(&field) {
        invalid(
            "prefab_set_override",
            &format!("字段 `{field}` 不在可覆盖集里"),
        );
        return false;
    }
    // Rhai 的 `Dynamic` → JSON：覆盖表的真源就是 JSON（`ObjectOverride` 按三态
    // 反序列化），走这条转换而不是手写 12 个 `Dynamic::as_*` 分支，字段加了类型也自动跟上。
    let value: serde_json::Value = match rhai::serde::from_dynamic(&value) {
        Ok(value) => value,
        Err(err) => {
            invalid("prefab_set_override", &format!("取值无法转成 JSON：{err}"));
            return false;
        }
    };
    let outcome = ctx.write(|scene| {
        let Some(target) = scene.instance_mut(name) else {
            return Err(format!("实例 `{name}` 不存在"));
        };
        let mut json = serde_json::to_value(&target.overrides)
            .map_err(|err| format!("覆盖表无法序列化：{err}"))?;
        json[field] = value;
        // 回读交给 `ObjectOverride` 自己的反序列化：字段类型不对、名不认识，
        // 都在这里被拒，不会留一条非法覆盖到 `resolve_overrides` 才炸。
        match serde_json::from_value::<ObjectOverride>(json) {
            Ok(overrides) => {
                target.overrides = overrides;
                scene.sync_instance_node(name);
                Ok(())
            }
            Err(err) => Err(format!("字段 `{field}` 的取值形状不对：{err}")),
        }
    });
    match outcome {
        Ok(()) => true,
        Err(detail) => {
            invalid("prefab_set_override", &detail);
            false
        }
    }
}

/// `prefab_clear_override`：把一个字段退回「继承模板」。
///
/// 与 `set_override` 镜像：序列化 → 删键 → 反序列化。删不到键说明本来就没覆盖，
/// 属正常结果（`false`），不是失败——幂等清理不该报错。
fn clear_override(ctx: &ScriptContext, name: &str, field: &str) -> bool {
    if !ObjectOverride::FIELD_NAMES.contains(&field) {
        invalid(
            "prefab_clear_override",
            &format!("字段 `{field}` 不在可覆盖集里"),
        );
        return false;
    }
    let outcome = ctx.write(|scene| {
        let Some(target) = scene.instance_mut(name) else {
            return Err(format!("实例 `{name}` 不存在"));
        };
        let mut json = serde_json::to_value(&target.overrides)
            .map_err(|err| format!("覆盖表无法序列化：{err}"))?;
        let removed = match json.as_object_mut() {
            Some(map) => map.remove(field).is_some(),
            None => return Err("覆盖表不是 JSON 对象，拒绝继续".to_string()),
        };
        match serde_json::from_value::<ObjectOverride>(json) {
            Ok(overrides) => {
                target.overrides = overrides;
                scene.sync_instance_node(name);
                Ok(removed)
            }
            Err(err) => Err(format!("覆盖表回读失败：{err}")),
        }
    });
    match outcome {
        Ok(changed) => changed,
        Err(detail) => {
            invalid("prefab_clear_override", &detail);
            false
        }
    }
}

/// `prefab_info`：模板 ID + 覆盖字段清单（只读，字段齐备时 `found = true`）。
fn info(ctx: &ScriptContext, name: &str) -> Map {
    let scene = ctx.doc.borrow();
    let mut map = Map::new();
    let Some(instance) = scene.instance(name) else {
        map.insert("found".into(), false.into());
        map.insert("name".into(), name.trim().to_string().into());
        println!("[script] prefab_info: 实例 `{name}` 不存在");
        return map;
    };
    let mut fields = Array::new();
    let mut overrides = Array::new();
    for field in ObjectOverride::FIELD_NAMES {
        // 继承态不进清单：脚本要的是「哪些字段被覆盖了」，把 12 个字段全列一遍
        // 会让调用方自己过滤，等于把口径推给脚本。
        match instance.overrides.field_state(field) {
            Some(OverrideState::Inherit) | None => continue,
            Some(state) => {
                fields.push(field.to_string().into());
                let mut pair = Map::new();
                pair.insert("field".into(), field.to_string().into());
                pair.insert("state".into(), state.as_str().into());
                overrides.push(pair.into());
            }
        }
    }
    map.insert("found".into(), true.into());
    map.insert("name".into(), instance.name.clone().into());
    map.insert("template".into(), instance.template.clone().into());
    map.insert(
        "parent".into(),
        instance
            .parent
            .clone()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    map.insert(
        "node".into(),
        scene
            .find(name)
            .map(|node| node.name.clone())
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    map.insert("count".into(), (fields.len() as i64).into());
    map.insert("fields".into(), fields.into());
    map.insert("overrides".into(), overrides.into());
    map.insert(
        "templateRegistered".into(),
        scene.prefab_handle(&instance.template).is_some().into(),
    );
    map
}

/// 注册六条绑定。
pub(super) fn register(engine: &mut Engine, ctx: &ScriptContext) {
    let c = ctx.clone();
    engine.register_fn("prefab_define", move |id: &str, path: &str| -> bool {
        define(&c, id, path)
    });

    let c = ctx.clone();
    engine.register_fn(
        "prefab_spawn",
        move |id: &str, name: &str, parent: &str| -> String { spawn(&c, id, name, parent) },
    );

    let c = ctx.clone();
    engine.register_fn(
        "prefab_spawn_many",
        move |id: &str, prefix: &str, count: i64| -> i64 { spawn_many(&c, id, prefix, count) },
    );

    let c = ctx.clone();
    engine.register_fn(
        "prefab_set_override",
        move |name: &str, field: &str, value: Dynamic| -> bool {
            set_override(&c, name, field, value)
        },
    );

    let c = ctx.clone();
    engine.register_fn(
        "prefab_clear_override",
        move |name: &str, field: &str| -> bool { clear_override(&c, name, field) },
    );

    let c = ctx.clone();
    engine.register_fn("prefab_info", move |name: &str| -> Map { info(&c, name) });
}
